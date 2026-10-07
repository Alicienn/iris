//! Le moteur de synchronisation.
//!
//! Il assemble ce que les autres modules savent faire : l'ordonnanceur choisit qui
//! parle au réseau, le pool borne combien parlent à la fois, le rejeu vide le journal
//! d'opérations, et la synchronisation de dossier fait le travail.
//!
//! Sa seule règle propre, mais elle est décisive : **le journal d'opérations est
//! rejoué avant de relire le serveur**. Dans l'ordre inverse, une action locale non
//! encore transmise serait écrasée par l'état distant, et l'utilisateur verrait son
//! geste défait sous ses yeux.

use crate::folder::{sync_folder, FolderSyncOptions};
use crate::replay::replay_account;
use crate::scheduler::{ScheduleConfig, Scheduler, SyncOutcome};
use async_trait::async_trait;
use iris_imap::pool::{ConnectionPool, PoolConfig};
use iris_imap::{Connector, Credentials, Endpoint};
use iris_kernel::{Event, EventBus, SyncPhase};
use iris_store::{FolderRole, Store};
use iris_types::{AccountId, Error, Result, Timestamp};
use std::sync::Arc;

/// Ce qui sait fournir les identifiants d'un compte.
///
/// Un trait plutôt qu'un accès direct au trousseau : le moteur n'a aucune raison de
/// savoir où vivent les mots de passe, et les tests n'ont aucune raison d'en créer.
#[async_trait]
pub trait CredentialsProvider: Send + Sync + std::fmt::Debug {
    /// What the IMAP server is signed in to with.
    async fn credentials(&self, account: AccountId, email: &str) -> Result<Credentials>;

    /// What the SMTP server is signed in to with: the IMAP credentials unless the
    /// account has a login or a password of its own for sending.
    async fn smtp_credentials(&self, account: AccountId, email: &str) -> Result<Credentials> {
        self.credentials(account, email).await
    }
}

/// Réglages du moteur.
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    pub pool: PoolConfig,
    pub schedule: ScheduleConfig,
    pub folder: FolderSyncOptions,
    /// Nombre de comptes synchronisés simultanément à chaque tour.
    pub concurrency: usize,
    /// Un cycle sur combien relève les suppressions distantes.
    ///
    /// L'opération coûte une recherche sur tout le dossier : la faire à chaque tour
    /// gaspillerait l'essentiel du budget réseau pour un événement rare. A pass whose
    /// message count drops makes it anyway (`sync_folder`); this scan is the backstop
    /// for what the count cannot show.
    pub deletion_scan_every: u32,
    /// How long reaching a server and signing in may take.
    pub connect_timeout: std::time::Duration,
    /// How long one account's whole pass may take before it is given up, to be tried
    /// again at the next one. A pass brings at most `folder.max_per_pass` messages
    /// per folder and keeps what it brought, so giving up loses nothing but time.
    pub account_timeout: std::time::Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            pool: PoolConfig::default(),
            schedule: ScheduleConfig::default(),
            folder: FolderSyncOptions::default(),
            concurrency: 4,
            deletion_scan_every: 10,
            connect_timeout: std::time::Duration::from_secs(30),
            account_timeout: std::time::Duration::from_secs(10 * 60),
        }
    }
}

/// Bilan d'un tour de synchronisation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    pub accounts_synced: usize,
    pub messages_added: usize,
    /// New mail in the inboxes, outside a first sync, each message once: what is
    /// worth a notification.
    pub inbox_arrivals: usize,
    pub flags_updated: usize,
    pub messages_deleted: usize,
    pub ops_replayed: usize,
    pub failures: Vec<(AccountId, String)>,
    /// Actions the server kept refusing, given up: done here, not there.
    pub actions_refused: Vec<(AccountId, String)>,
}

impl TickReport {
    pub fn changed(&self) -> bool {
        self.messages_added > 0 || self.flags_updated > 0 || self.messages_deleted > 0
    }
}

/// Le moteur.
#[derive(Debug)]
pub struct SyncEngine {
    store: Arc<Store>,
    connector: Arc<dyn Connector>,
    credentials: Arc<dyn CredentialsProvider>,
    pool: ConnectionPool,
    bus: EventBus,
    scheduler: tokio::sync::Mutex<Scheduler>,
    config: EngineConfig,
    cycle: std::sync::atomic::AtomicU32,
    /// Index plein texte. Optionnel : sans lui, la synchronisation fonctionne, la
    /// recherche ne trouve rien — ce qui doit rester un choix explicite, pas un
    /// oubli silencieux.
    index: Option<Arc<iris_index::SearchIndex>>,
    /// Magasin de contenus, nécessaire au téléchargement des corps.
    blobs: Option<Arc<iris_blobs::BlobStore>>,
    automation: std::sync::RwLock<iris_types::AutomationSettings>,
    /// The state machine, when one is attached. The time-based passes delegate to it
    /// rather than reimplementing the rules a second time.
    workflow: Option<Arc<iris_workflow::Workflow>>,
    /// Why each account last failed, so the interface can say more than "!".
    ///
    /// A marker that reports a fault without naming it leaves the user with nothing
    /// to act on, which is exactly what the exclamation mark in the sidebar was.
    failures: std::sync::RwLock<std::collections::BTreeMap<AccountId, AccountFailure>>,
    /// The accounts whose pass is under way. Sync all and the scheduled pass could
    /// both run one account at once: its journal replayed twice, moves made twice
    /// (and on a server without MOVE, copies made twice).
    en_cours: std::sync::Mutex<std::collections::HashSet<AccountId>>,
}

/// What the connection pool counts connections under: the mailbox on its server. By
/// server name alone, every Gmail account shared three connections between them, and
/// one's sync starved the others' bodies, which gave up after half a minute. Servers
/// limit connections per mailbox.
pub(crate) fn pool_key(compte: &iris_store::Account) -> String {
    format!("{}#{}", compte.imap_host, compte.email)
}

/// An account's pass under way, until dropped.
struct PasseEnCours<'a> {
    compte: AccountId,
    tous: &'a std::sync::Mutex<std::collections::HashSet<AccountId>>,
}

impl Drop for PasseEnCours<'_> {
    fn drop(&mut self) {
        if let Ok(mut tous) = self.tous.lock() {
            tous.remove(&self.compte);
        }
    }
}

/// Why an account last failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountFailure {
    pub message: String,
    /// The credentials were refused, as opposed to the server being unreachable.
    /// The distinction decides whether retrying can ever help.
    pub needs_password: bool,
    pub at: Timestamp,
}

impl AccountFailure {
    /// A few words for a tooltip: what kind of failure, not the whole transcript.
    pub fn summary(&self) -> &'static str {
        if self.needs_password {
            "password refused"
        } else if self.certificate().is_some() {
            "certificate does not match the server name"
        } else {
            "server unreachable"
        }
    }

    /// What to put in front of the user.
    pub fn advice(&self) -> String {
        if self.needs_password {
            return "The server refused these credentials. Re-enter the password to try again."
                .into();
        }
        if let Some(mismatch) = self.certificate() {
            // Retrying cannot help here, and "usually clears on its own" would send
            // the user off to wait for something that will never happen.
            let mut conseil = format!(
                "The server's security certificate is not issued for {}, so Iris refuses \
                 the connection. Edit the account and use the server name the certificate \
                 covers",
                if mismatch.host.is_empty() {
                    "this server name"
                } else {
                    &mismatch.host
                }
            );
            match mismatch.suggestion() {
                Some(nom) => conseil.push_str(&format!(": {nom}.")),
                None => conseil.push('.'),
            }
            return conseil;
        }
        "The server could not be reached. This usually clears on its own.".into()
    }

    /// The name the certificate was checked against, and the names it covers — when
    /// this failure is a certificate issued for another name.
    pub fn certificate(&self) -> Option<CertificateMismatch> {
        CertificateMismatch::parse(&self.message)
    }
}

/// A server whose certificate is issued for other names than the one dialled.
///
/// The usual cause on shared hosting: `mail.example.com` points at a machine whose
/// certificate only names the host's own servers. The connection is refused, rightly,
/// and the fix is to dial one of the names the certificate does cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateMismatch {
    /// The name that was dialled. Empty when the message did not say.
    pub host: String,
    /// The names the certificate is valid for.
    pub valid_for: Vec<String>,
}

impl CertificateMismatch {
    /// Read from the TLS library's message: `certificate not valid for name "x";
    /// certificate is only valid for DnsName("a"), DnsName("b")`.
    pub fn parse(message: &str) -> Option<Self> {
        let bas = message.to_ascii_lowercase();
        if !bas.contains("certificate") || !bas.contains("valid for") {
            return None;
        }
        let host = message
            .split("valid for name \"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap_or_default()
            .to_string();
        let valid_for = message
            .split("DnsName(\"")
            .skip(1)
            .filter_map(|r| r.split('"').next())
            .map(str::to_string)
            .collect();
        Some(Self { host, valid_for })
    }

    /// The name to dial instead: the one that names the machine itself rather than one
    /// of the services hosted on it (`autoconfig.`, `cpanel.` and the like).
    pub fn suggestion(&self) -> Option<&str> {
        const SERVICES: [&str; 10] = [
            "autoconfig.",
            "autodiscover.",
            "cpanel.",
            "cpcalendars.",
            "cpcontacts.",
            "webdisk.",
            "webmail.",
            "whm.",
            "www.",
            "*.",
        ];
        self.valid_for
            .iter()
            .map(String::as_str)
            .find(|n| !SERVICES.iter().any(|s| n.starts_with(s)))
    }
}

impl SyncEngine {
    pub fn new(
        store: Arc<Store>,
        connector: Arc<dyn Connector>,
        credentials: Arc<dyn CredentialsProvider>,
        bus: EventBus,
        config: EngineConfig,
    ) -> Self {
        Self {
            store,
            connector,
            credentials,
            pool: ConnectionPool::new(config.pool),
            bus,
            scheduler: tokio::sync::Mutex::new(Scheduler::new(config.schedule)),
            config,
            cycle: std::sync::atomic::AtomicU32::new(0),
            index: None,
            blobs: None,
            automation: std::sync::RwLock::new(iris_types::AutomationSettings::default()),
            workflow: None,
            failures: std::sync::RwLock::new(std::collections::BTreeMap::new()),
            en_cours: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Réglages des automatismes du workflow.
    ///
    /// Ils vivent ici parce que l'envoi et la relance en dépendent tous les deux, et
    /// qu'une seconde copie divergerait.
    pub fn automation(&self) -> iris_types::AutomationSettings {
        *self.automation.read().expect("réglages empoisonnés")
    }

    pub fn set_automation(&self, settings: iris_types::AutomationSettings) {
        *self.automation.write().expect("réglages empoisonnés") = settings;
    }

    /// Branche l'index plein texte.
    pub fn with_index(mut self, index: Arc<iris_index::SearchIndex>) -> Self {
        self.index = Some(index);
        self
    }

    /// Branche le magasin de contenus.
    /// Attaches the state machine used by the time-based passes.
    pub fn with_workflow(mut self, workflow: Arc<iris_workflow::Workflow>) -> Self {
        self.workflow = Some(workflow);
        self
    }

    pub(crate) fn workflow(&self) -> Option<&Arc<iris_workflow::Workflow>> {
        self.workflow.as_ref()
    }

    pub fn with_blobs(mut self, blobs: Arc<iris_blobs::BlobStore>) -> Self {
        self.blobs = Some(blobs);
        self
    }

    pub(crate) fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub(crate) fn blobs(&self) -> Option<&Arc<iris_blobs::BlobStore>> {
        self.blobs.as_ref()
    }

    #[allow(dead_code)]
    pub(crate) fn bus(&self) -> &EventBus {
        &self.bus
    }

    pub(crate) fn config(&self) -> &EngineConfig {
        &self.config
    }

    pub(crate) fn pool(&self) -> &ConnectionPool {
        &self.pool
    }

    pub(crate) fn connector(&self) -> &Arc<dyn Connector> {
        &self.connector
    }

    /// Coordonnées de connexion d'un compte.
    pub(crate) fn endpoint_for(&self, account: &iris_store::Account) -> Endpoint {
        if account.imap_tls {
            Endpoint::tls(&account.imap_host, account.imap_port)
        } else {
            Endpoint::starttls(&account.imap_host, account.imap_port)
        }
    }

    pub(crate) async fn credentials_for(
        &self,
        account: &iris_store::Account,
    ) -> Result<Credentials> {
        self.credentials
            .credentials(account.id, &account.email)
            .await
    }

    /// Indexe un lot de messages à partir de leurs seuls en-têtes.
    ///
    /// Le corps n'est pas encore là : on indexe ce qu'on a — sujet, expéditeur,
    /// aperçu —, ce qui rend déjà la plupart des recherches fructueuses, et le corps
    /// viendra enrichir l'entrée à l'ouverture.
    fn index_headers(&self, messages: &[iris_store::StoredMessage]) -> Result<usize> {
        let Some(index) = &self.index else {
            return Ok(0);
        };

        for m in messages {
            index.add(&iris_index::IndexedMessage {
                message: m.id,
                thread: m.thread,
                account: m.account,
                subject: m.subject.clone(),
                from: format!("{} {}", m.from_name, m.from_addr),
                recipients: String::new(),
                body: m.preview.clone(),
                received: m.received,
                has_attachment: m.flags.contains(iris_types::Flags::HAS_ATTACHMENT),
            })?;
        }
        index.commit()?;
        Ok(messages.len())
    }

    /// Recense les pièces jointes que le corps vient de révéler.
    ///
    /// L'enveloppe ne les connaît pas : seul le corps complet dit ce qu'un message
    /// contient. Un échec d'analyse n'est pas propagé — le corps est téléchargé et
    /// lisible, et perdre la liste des pièces jointes ne justifie pas de rendre
    /// l'ouverture du message impossible.
    pub(crate) fn record_attachments(&self, message: &iris_store::StoredMessage, raw: &[u8]) {
        let analyse = match iris_mime::parse(raw) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(message = %message.id, error = %e, "parsing attachments");
                return;
            }
        };

        if let Err(e) = self
            .store()
            .record_attachments(message.id, &analyse.attachments)
        {
            tracing::warn!(message = %message.id, error = %e, "recording attachments");
        }
        // What the headers alone could only guess (attachment, tracker), and the
        // preview, from the whole message now that it is here.
        if let Err(e) =
            self.store()
                .set_body_facts(message.id, &analyse.preview, analyse.derived_flags)
        {
            tracing::warn!(message = %message.id, error = %e, "recording what the body says");
        }
    }

    /// Remplace l'entrée d'index d'un message par une entrée incluant son corps.
    pub(crate) fn reindex_with_body(
        &self,
        message: &iris_store::StoredMessage,
        raw: &[u8],
    ) -> Result<()> {
        let Some(index) = &self.index else {
            return Ok(());
        };

        // Le texte indexé est celui de l'analyse, jamais le HTML brut : indexer des
        // balises remplirait l'index de bruit et ferait remonter n'importe quel
        // message sur une recherche de « table » ou de « span ».
        let texte = iris_mime::parse(raw)
            .map(|p| p.indexable_text())
            .unwrap_or_else(|_| String::from_utf8_lossy(raw).into_owned());

        index.add(&iris_index::IndexedMessage {
            message: message.id,
            thread: message.thread,
            account: message.account,
            subject: message.subject.clone(),
            from: format!("{} {}", message.from_name, message.from_addr),
            recipients: String::new(),
            body: texte,
            received: message.received,
            has_attachment: message.flags.contains(iris_types::Flags::HAS_ATTACHMENT),
        })?;
        index.commit()?;
        Ok(())
    }

    /// Inscrit à l'ordonnancement tous les comptes connus du store.
    pub async fn load_accounts(&self, now: Timestamp) -> Result<usize> {
        let comptes = self.store.accounts()?;
        let mut ordonnanceur = self.scheduler.lock().await;
        let mut inscrits = 0;

        // Disabled or removed since: out of the schedule. Accounts were only ever
        // added, so one switched off kept syncing (a refused password kept being
        // tried, which is how servers lock accounts) and a removed one kept failing.
        let actifs: std::collections::BTreeSet<AccountId> =
            comptes.iter().filter(|c| c.enabled).map(|c| c.id).collect();
        for id in ordonnanceur.registered() {
            if !actifs.contains(&id) {
                ordonnanceur.remove(id);
            }
        }

        for c in &comptes {
            if !c.enabled {
                continue;
            }
            ordonnanceur.register(c.id, &c.imap_host, c.pinned, now);
            inscrits += 1;
        }
        Ok(inscrits)
    }

    /// Désigne le compte que l'utilisateur regarde.
    pub async fn set_active_account(&self, account: Option<AccountId>, now: Timestamp) {
        self.scheduler.lock().await.set_active(account, now);
        if let Some(id) = account {
            self.bus.publish(Event::AccountFocused(id));
        }
    }

    /// The mailbox whose inbox is watched for news: the one on screen, else the first
    /// switched on.
    pub async fn watched_account(&self) -> Option<AccountId> {
        if let Some(actif) = self.scheduler.lock().await.active() {
            return Some(actif);
        }
        self.store
            .accounts()
            .ok()?
            .into_iter()
            .find(|c| c.enabled)
            .map(|c| c.id)
    }

    /// Waits on a mailbox's inbox for news, the IMAP `IDLE` way, and syncs it as soon
    /// as some comes. Polled only, new mail took up to two minutes to show for the
    /// mailbox on screen. Returns `Ok` at once when the server cannot `IDLE`, and an
    /// error when the wait ends otherwise.
    pub async fn watch_inbox(&self, account: AccountId) -> Result<()> {
        let compte = self
            .store
            .account(account)?
            .filter(|c| c.enabled)
            .ok_or_else(|| Error::store(format!("account {account} not watched")))?;
        let point = self.endpoint_for(&compte);
        let identifiants = self.credentials_for(&compte).await?;
        let mut conn = tokio::time::timeout(
            self.config.connect_timeout,
            self.connector.connect(&point, &identifiants),
        )
        .await
        .map_err(|_| Error::network(format!("{} did not answer", compte.imap_host)))??;
        if !conn.capabilities().idle {
            let _ = conn.logout().await;
            return Ok(());
        }
        let boite = self
            .store
            .folder_for_role(account, FolderRole::Inbox)?
            .map(|f| f.path)
            .unwrap_or_else(|| "INBOX".to_string());
        conn.select(&boite).await?;
        loop {
            // Under the half hour servers allow an idle connection.
            match conn.idle(std::time::Duration::from_secs(25 * 60)).await? {
                iris_imap::IdleOutcome::Changed => {
                    let maintenant = now_utc();
                    let resultat = self.sync_account_bounded(account, maintenant, false).await;
                    self.note_failure(account, resultat.as_ref().err());
                }
                iris_imap::IdleOutcome::TimedOut => {}
                iris_imap::IdleOutcome::Disconnected => {
                    return Err(Error::network("the server ended the wait for news"))
                }
            }
        }
    }

    pub async fn resume_account(&self, account: AccountId, now: Timestamp) {
        self.scheduler.lock().await.resume(account, now);
    }

    pub async fn suspended_accounts(&self) -> Vec<AccountId> {
        self.scheduler.lock().await.suspended()
    }

    /// Durée avant le prochain travail utile.
    pub async fn next_wakeup(&self, now: Timestamp) -> Option<std::time::Duration> {
        self.scheduler.lock().await.next_wakeup(now)
    }

    /// Synchronises one account now, whatever the schedule had planned.
    ///
    /// The schedule exists so a hundred mailboxes do not all wake at once; it is not
    /// a reason to make someone wait when they have asked for one of them.
    pub async fn sync_now(&self, account: AccountId, now: Timestamp) -> Result<usize> {
        let compte = self
            .store
            .account(account)?
            .ok_or_else(|| Error::store(format!("account {account} not found")))?;

        // A manual refresh looks for deletions too: it is the gesture someone makes
        // precisely when they suspect the local copy has drifted.
        let resultat = self.sync_account_bounded(compte.id, now, true).await;
        self.note_failure(account, resultat.as_ref().err());
        let rapport = resultat?;
        self.scheduler.lock().await.resume(account, now);
        Ok(rapport.added)
    }

    /// Synchronises every enabled account, reporting progress as it goes: `progress(done,
    /// total, account)` after each one, and `(0, total, "")` first.
    ///
    /// A few at a time (`concurrency`), each within `account_timeout`. They went one
    /// after the other, and a server that stopped answering held back every mailbox
    /// after it, with the count stuck on the one it hung on. Not all at once either: a
    /// hundred mailboxes is a hundred TLS handshakes, and the pool would queue them.
    pub async fn sync_all(
        &self,
        now: Timestamp,
        mut progress: impl FnMut(usize, usize, &str),
    ) -> SyncAllReport {
        use futures::stream::{self, StreamExt};
        let comptes: Vec<_> = self
            .store
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.enabled)
            .collect();

        let total = comptes.len();
        let mut rapport = SyncAllReport {
            total,
            ..Default::default()
        };

        progress(0, total, "");
        let mut en_cours = stream::iter(comptes)
            .map(|compte| async move {
                let resultat = self.sync_account_bounded(compte.id, now, true).await;
                (compte, resultat)
            })
            .buffer_unordered(self.config.concurrency.max(1));

        let mut faits = 0;
        while let Some((compte, resultat)) = en_cours.next().await {
            faits += 1;
            self.note_failure(compte.id, resultat.as_ref().err());
            match resultat {
                Ok(bilan) => {
                    rapport.synced += 1;
                    rapport.added += bilan.added;
                    rapport.refused += bilan.refused.len();
                    // Working: back on the schedule if it had been set aside. "Sync
                    // all" never resumed anyone, and an account left suspended stayed
                    // so although it had just synced.
                    self.scheduler.lock().await.resume(compte.id, now);
                }
                // One unreachable server must not stop the other ninety-nine.
                // `note_failure` has logged it.
                Err(e) => {
                    rapport.failed.push((compte.email.clone(), e.to_string()));
                }
            }
            progress(faits, total, &compte.email);
        }
        rapport
    }

    /// One account's pass, given up after `account_timeout`. Dropping the pass drops
    /// its connection, and with it the pool's place it held.
    async fn sync_account_bounded(
        &self,
        account: AccountId,
        now: Timestamp,
        detect_deletions: bool,
    ) -> Result<AccountReport> {
        // Already under way: that pass does it.
        let Some(_passe) = self.begin_pass(account) else {
            return Ok(AccountReport::default());
        };
        let limite = self.config.account_timeout;
        match tokio::time::timeout(limite, self.sync_account(account, now, detect_deletions)).await
        {
            Ok(resultat) => resultat,
            Err(_) => Err(Error::Network(format!(
                "the server stopped answering (nothing for {} minutes); it will be tried again",
                limite.as_secs() / 60
            ))),
        }
    }

    /// Marks an account's pass as under way; `None` if one already is.
    fn begin_pass(&self, account: AccountId) -> Option<PasseEnCours<'_>> {
        let mut tous = self.en_cours.lock().ok()?;
        if !tous.insert(account) {
            return None;
        }
        Some(PasseEnCours {
            compte: account,
            tous: &self.en_cours,
        })
    }

    /// Why this account last failed, if it did.
    pub fn failure(&self, account: AccountId) -> Option<AccountFailure> {
        self.failures.read().ok()?.get(&account).cloned()
    }

    /// Every account currently in trouble.
    pub fn failures(&self) -> Vec<(AccountId, AccountFailure)> {
        self.failures
            .read()
            .map(|f| f.iter().map(|(k, v)| (*k, v.clone())).collect())
            .unwrap_or_default()
    }

    /// Records a failure, or clears one when the account works again.
    pub(crate) fn note_failure(&self, account: AccountId, error: Option<&Error>) {
        let Ok(mut failures) = self.failures.write() else {
            return;
        };
        match error {
            Some(e) => {
                // Into the log once per new reason. The scheduled pass logged nothing,
                // so an account that only ever failed in the background left no trace
                // of why — a Google account stuck at sign-in among them.
                let message = e.to_string();
                if failures.get(&account).map(|f| &f.message) != Some(&message) {
                    tracing::warn!(account = %account, error = %message, "sync failed");
                }
                failures.insert(
                    account,
                    AccountFailure {
                        message,
                        needs_password: e.needs_user_action(),
                        at: now_utc(),
                    },
                );
            }
            // Success clears it. A stale complaint about a problem that has gone is
            // as misleading as no complaint about one that has not.
            None => {
                failures.remove(&account);
            }
        }
    }

    pub fn pool_stats(&self) -> iris_imap::pool::PoolStats {
        self.pool.stats()
    }

    /// Exécute un tour : synchronise les comptes dus, dans l'ordre de priorité.
    pub async fn tick(&self, now: Timestamp) -> TickReport {
        let dus = {
            let mut ordonnanceur = self.scheduler.lock().await;
            ordonnanceur.due(now)
        };

        let cycle = self
            .cycle
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let releve_suppressions =
            self.config.deletion_scan_every > 0 && cycle % self.config.deletion_scan_every == 0;

        let mut rapport = TickReport::default();

        // The accounts due, side by side rather than one after the other: a server that
        // hangs holds back only itself, and only until `account_timeout`. The loop that
        // calls `tick` waits for it, so a pass that never ended stopped every later
        // pass, and the "syncing" mark with it.
        use futures::stream::{self, StreamExt};
        let mut en_cours = stream::iter(dus.into_iter().take(self.config.concurrency))
            .map(|account| async move {
                (
                    account,
                    self.sync_account_bounded(account, now, releve_suppressions)
                        .await,
                )
            })
            .buffer_unordered(self.config.concurrency.max(1));

        while let Some((account, resultat)) = en_cours.next().await {
            // The scheduled pass records outcomes too, so an account that only ever
            // fails in the background still has something to show the user.
            self.note_failure(account, resultat.as_ref().err());

            match resultat {
                Ok(bilan) => {
                    rapport.accounts_synced += 1;
                    rapport.messages_added += bilan.added;
                    rapport.flags_updated += bilan.flags_updated;
                    rapport.messages_deleted += bilan.deleted;
                    rapport.ops_replayed += bilan.ops_replayed;
                    rapport.inbox_arrivals += bilan.inbox_arrivals;
                    rapport
                        .actions_refused
                        .extend(bilan.refused.iter().map(|m| (account, m.clone())));

                    let resultat = if bilan.added > 0 || bilan.flags_updated > 0 {
                        SyncOutcome::Changed {
                            messages: bilan.added as u32,
                        }
                    } else {
                        SyncOutcome::Unchanged
                    };
                    self.scheduler.lock().await.record(account, resultat, now);
                }
                Err(e) => {
                    let resultat = if e.needs_user_action() {
                        SyncOutcome::NeedsAttention
                    } else {
                        SyncOutcome::TransientFailure
                    };
                    self.scheduler.lock().await.record(account, resultat, now);

                    self.bus.publish(Event::SyncFailed {
                        account,
                        message: Arc::from(e.to_string().as_str()),
                        transient: e.is_transient(),
                    });
                    rapport.failures.push((account, e.to_string()));
                }
            }
        }

        rapport
    }

    /// Synchronise un compte : rejeu du journal, puis relecture des dossiers.
    async fn sync_account(
        &self,
        account: AccountId,
        now: Timestamp,
        detect_deletions: bool,
    ) -> Result<AccountReport> {
        let compte = self
            .store
            .account(account)?
            .ok_or_else(|| Error::store(format!("compte {account} introuvable")))?;

        let _place = self.pool.acquire(&pool_key(&compte)).await?;

        self.publish_phase(account, SyncPhase::Connecting);
        let endpoint = if compte.imap_tls {
            Endpoint::tls(&compte.imap_host, compte.imap_port)
        } else {
            Endpoint::starttls(&compte.imap_host, compte.imap_port)
        };
        // Reaching the server and signing in, within `connect_timeout`: a server that
        // takes the connection and then says nothing is the commonest way to hang.
        let limite = self.config.connect_timeout;
        let mut conn = tokio::time::timeout(limite, async {
            let identifiants = self.credentials.credentials(account, &compte.email).await?;
            self.connector.connect(&endpoint, &identifiants).await
        })
        .await
        .map_err(|_| {
            Error::Network(format!(
                "{} did not answer within {} seconds",
                compte.imap_host,
                limite.as_secs()
            ))
        })??;

        let mut bilan = AccountReport::default();

        // Le journal d'abord. Dans l'ordre inverse, une action locale non encore
        // transmise serait écrasée par l'état distant.
        let a_traiter = self.store.pending_ops_for(account, now, 100)?;
        if !a_traiter.is_empty() {
            let r = replay_account(conn.as_mut(), &self.store, &a_traiter, now).await?;
            bilan.ops_replayed = r.applied;
            bilan.refused = r.refused;
        }

        // Découverte des dossiers.
        self.publish_phase(account, SyncPhase::ListingFolders);
        let distants = conn.list_folders().await?;
        for d in &distants {
            self.store
                .upsert_folder(account, &d.path, translate_kind(d.kind))?;
        }
        // What separates a folder from its children here, for folders created and
        // renamed from Iris (`/` at Gmail, where a `.` made a label `INBOX.Devis`).
        if let Some(separateur) = distants.iter().find_map(|d| d.delimiter) {
            if compte.folder_delimiter != Some(separateur) {
                self.store.set_folder_delimiter(account, separateur)?;
            }
        }

        // Et ce que le serveur ne liste plus, on l'oublie.
        //
        // Seul un SELECT refusé en `[NONEXISTENT]` faisait disparaître un dossier. Un
        // dossier devenu une simple branche, supprimé ailleurs, ou qu'on a appris à
        // ignorer, restait donc dans la copie locale pour toujours — et revenait dans
        // la colonne après chaque suppression. La garde : une liste sans boîte de
        // réception n'est pas une réponse à laquelle on confie un effacement.
        if distants
            .iter()
            .any(|d| d.path.eq_ignore_ascii_case("INBOX"))
        {
            let listes: std::collections::HashSet<&str> =
                distants.iter().map(|d| d.path.as_str()).collect();
            for local in self.store.folders(account)? {
                if listes.contains(local.path.as_str()) {
                    continue;
                }
                // Vidé d'abord, pour que les fils soient recalculés au passage.
                self.store.clear_folder(local.id)?;
                self.store.forget_folder(account, &local.path)?;
                tracing::info!(
                    account = %account, folder = %local.path,
                    "folder dropped: the server no longer lists it"
                );
            }
        }

        // Synchronisation, boîte de réception d'abord : c'est ce que l'utilisateur
        // regarde, et il ne doit pas attendre que « Archives 2019 » soit relu.
        let mut dossiers = self.store.folders(account)?;
        dossiers.sort_by_key(|d| match d.role {
            FolderRole::Inbox => 0,
            FolderRole::Sent => 1,
            FolderRole::Drafts => 2,
            _ => 3,
        });

        // Flag changes made here that the replay could not deliver: the server's
        // flags are not read back over them this pass.
        let drapeaux_en_attente = self
            .store
            .pending_ops_for(account, Timestamp::from_millis(i64::MAX), 100)?
            .iter()
            .any(|op| op.kind == iris_store::OpKind::SetFlags);
        let options = FolderSyncOptions {
            detect_deletions,
            keep_local_flags: drapeaux_en_attente,
            ..self.config.folder
        };
        let mut ajoutes_par_dossier = Vec::new();
        // The first network failure of the pass, said at its end. Swallowed per folder,
        // the pass ended as a success, the account's error was cleared, and no back-off
        // applied: an account that could not fetch anything looked synchronised.
        let mut panne: Option<Error> = None;

        for (index, dossier) in dossiers.iter().enumerate() {
            self.publish_phase(
                account,
                SyncPhase::FetchingHeaders {
                    done: index as u32,
                    total: dossiers.len() as u32,
                },
            );

            match sync_folder(conn.as_mut(), &self.store, account, dossier, options).await {
                Ok(r) => {
                    bilan.added += r.added;
                    bilan.inbox_arrivals += r.inbox_arrivals;
                    bilan.flags_updated += r.flags_updated;
                    bilan.deleted += r.deleted;
                    // A message that arrives in a thread may bring it back to the
                    // queue (the "a new message reopens the thread" setting).
                    if let Some(workflow) = &self.workflow {
                        for fil in &r.arrivals {
                            if let Err(e) = workflow.on_message_received(*fil, now) {
                                tracing::warn!(error = %e, "reopening a thread");
                            }
                        }
                        for fil in &r.back_in_inbox {
                            if let Err(e) = workflow.on_back_in_inbox(*fil, now) {
                                tracing::warn!(error = %e, "a thread back in the inbox");
                            }
                        }
                    }
                    if let Err(e) = self.unindex(&r.removed_messages) {
                        tracing::warn!(error = %e, "taking deleted mail out of the index");
                    }
                    if r.added > 0 {
                        ajoutes_par_dossier.push(dossier.id);
                        // L'indexation suit immédiatement l'insertion : un message
                        // visible dans la liste mais introuvable à la recherche est
                        // un défaut que l'utilisateur mettra sur le compte de la
                        // recherche, pas sur celui de la synchronisation.
                        if let Err(e) = self.index_new_messages(&r.new_messages) {
                            tracing::warn!(error = %e, "indexing");
                        }
                        // Rules run on arrival, not on a schedule: a rule that
                        // archives a newsletter should do it before the user sees
                        // the newsletter, otherwise it only tidies up after them.
                        self.run_rules_on_new(&r.fresh_messages, now);
                    }
                }
                // Un dossier illisible — droits insuffisants, boîte partagée
                // disparue — ne doit pas condamner le compte entier.
                Err(e) => {
                    // Sauf quand le serveur dit qu'il n'existe pas : alors on l'oublie.
                    //
                    // Sans cela, un dossier supprimé — par Iris, par un autre client,
                    // par l'administrateur — reste dans la copie locale pour toujours.
                    // Il continue d'apparaître dans la colonne, et chaque
                    // synchronisation retente de le sélectionner : le journal se
                    // remplit du même « Mailbox doesn't exist » à chaque tour, et rien
                    // ne le nettoie jamais.
                    //
                    // Reconnu au code de réponse IMAP, `NONEXISTENT`, et non au texte
                    // qui l'accompagne : celui-là change d'un serveur à l'autre. Une
                    // panne de réseau ou un refus de droits ne dit pas cela et ne fait
                    // donc rien disparaître — c'est la distinction qui compte, parce
                    // qu'oublier un dossier pour cause de coupure effacerait sa copie
                    // locale d'un simple câble débranché.
                    let disparu = e.to_string().contains("[NONEXISTENT]");
                    tracing::warn!(
                        account = %account, folder = %dossier.path, error = %e,
                        gone = disparu, "folder skipped"
                    );
                    // The connection is opened again by the next command, so one
                    // folder's failure does not end the pass; a second means the
                    // server itself is out of reach, and the pass stops there.
                    if e.is_transient() {
                        if panne.is_some() {
                            break;
                        }
                        panne = Some(e);
                        continue;
                    }
                    if disparu {
                        if let Err(e) = self.store.clear_folder(dossier.id) {
                            tracing::warn!(error = %e, "vidage du dossier disparu");
                        }
                        match self.store.forget_folder(account, &dossier.path) {
                            Ok(true) => tracing::info!(
                                account = %account, folder = %dossier.path,
                                "folder dropped: the server no longer has it"
                            ),
                            Ok(false) => {}
                            Err(e) => tracing::warn!(error = %e, "oubli du dossier disparu"),
                        }
                    }
                }
            }
        }

        self.publish_phase(account, SyncPhase::Idle);
        self.store.touch_account(account, now)?;

        if bilan.added > 0 {
            for folder in ajoutes_par_dossier {
                self.bus.publish(Event::MessagesAdded {
                    account,
                    folder,
                    // Les identifiants précis sont dans le store ; l'événement ne
                    // transporte pas de données volumineuses.
                    ids: Arc::from(Vec::new()),
                });
            }
        }

        let _ = conn.logout().await;
        match panne {
            Some(e) => Err(e),
            None => Ok(bilan),
        }
    }

    /// Indexe les messages d'un dossier qui n'ont pas encore de corps.
    ///
    /// Réindexer une entrée existante la remplace : repasser sur un message déjà
    /// indexé est sans effet, ce qui rend l'opération sûre à répéter.
    fn index_new_messages(&self, ids: &[iris_types::MessageId]) -> Result<usize> {
        if self.index.is_none() || ids.is_empty() {
            return Ok(0);
        }
        let mut messages = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(m) = self.store.message_by_id(*id)? {
                messages.push(m);
            }
        }
        self.index_headers(&messages)
    }

    /// Takes deleted messages out of the search index.
    fn unindex(&self, ids: &[iris_types::MessageId]) -> Result<()> {
        let Some(index) = &self.index else {
            return Ok(());
        };
        if ids.is_empty() {
            return Ok(());
        }
        for id in ids {
            index.remove_message(*id)?;
        }
        index.commit()?;
        Ok(())
    }

    /// Runs the rules over what a folder just received.
    ///
    /// Failures are logged, never propagated: a rule engine problem must not make a
    /// folder look unsynchronisable.
    ///
    /// The pass's new mail only, all of it: the folder's 500 newest were read again
    /// at every arrival, so beyond 500 arrivals the oldest were never examined, and a
    /// copy coming back from a move, a new row, ran the rules a second time.
    fn run_rules_on_new(&self, fresh: &[iris_types::MessageId], now: Timestamp) {
        if self.workflow.is_none() || fresh.is_empty() {
            return;
        }
        let mut ids = Vec::with_capacity(fresh.len());
        for id in fresh {
            match self.store.message_by_id(*id) {
                Ok(Some(m)) if crate::rules::worth_examining(m.flags) => ids.push(m.id),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "reading for the rules");
                    return;
                }
            }
        }

        // One undo for all the pass's rules, and redo left alone.
        if let Some(w) = &self.workflow {
            w.begin_automatic_batch();
        }
        let resultat = self.apply_rules(&ids, now);
        if let Some(w) = &self.workflow {
            w.end_batch();
        }
        match resultat {
            Ok(report) if report.changed() => tracing::info!(
                affected = report.affected,
                actions = report.actions,
                "rules applied"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "applying the rules"),
        }
    }

    fn publish_phase(&self, account: AccountId, phase: SyncPhase) {
        self.bus.publish(Event::SyncPhaseChanged { account, phase });
    }
}

/// What a full pass over every mailbox produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncAllReport {
    pub total: usize,
    pub synced: usize,
    pub added: usize,
    /// Accounts that could not be reached, with the reason.
    pub failed: Vec<(String, String)>,
    /// Actions the servers kept refusing, given up.
    pub refused: usize,
}

impl SyncAllReport {
    /// One line for the status bar.
    pub fn summary(&self) -> String {
        let ligne = self.summary_of_accounts();
        match self.refused {
            0 => ligne,
            n => format!("{ligne} {}", refused_line(n)),
        }
    }

    fn summary_of_accounts(&self) -> String {
        // The accounts that failed, by name when there are few: "1 account could not
        // be reached" sends the reader looking through the list for which one.
        let qui = match self.failed.len() {
            0 => String::new(),
            1..=3 => self
                .failed
                .iter()
                .map(|(email, _)| email.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            f => format!("{f} accounts"),
        };
        match (self.added, self.failed.len()) {
            (0, 0) => "Up to date.".into(),
            (n, 0) => format!("{n} new message(s)."),
            (0, _) => format!("Sync failed for {qui}. See the red ! in the account list."),
            (n, _) => format!(
                "{n} new message(s). Sync failed for {qui}. See the red ! in the account list."
            ),
        }
    }
}

/// What to tell of actions a server kept refusing: they show as done here, and the
/// next sync shows the mail as the server has it.
pub fn refused_line(n: usize) -> String {
    format!(
        "The server refused {} you made; your mail now shows as the server has it.",
        if n == 1 {
            "an action".to_string()
        } else {
            format!("{n} actions")
        }
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AccountReport {
    added: usize,
    /// New mail in the inbox, worth telling of.
    inbox_arrivals: usize,
    flags_updated: usize,
    deleted: usize,
    ops_replayed: usize,
    /// Why the server refused the actions given up this pass.
    refused: Vec<String>,
}

fn translate_kind(kind: iris_imap::FolderKind) -> FolderRole {
    use iris_imap::FolderKind as K;
    match kind {
        K::Inbox => FolderRole::Inbox,
        K::Sent => FolderRole::Sent,
        K::Drafts => FolderRole::Drafts,
        K::Trash => FolderRole::Trash,
        K::Junk => FolderRole::Junk,
        K::Archive => FolderRole::Archive,
        K::Other | K::NoSelect => FolderRole::Other,
    }
}

/// Instant courant, en temps universel.
pub fn now_utc() -> Timestamp {
    Timestamp::from_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    )
}

/// Fournisseur d'identifiants simulé.
#[derive(Debug, Clone)]
pub struct StaticCredentials {
    pub password: String,
}

impl StaticCredentials {
    pub fn new(password: impl Into<String>) -> Self {
        Self {
            password: password.into(),
        }
    }
}

#[async_trait]
impl CredentialsProvider for StaticCredentials {
    async fn credentials(&self, _account: AccountId, email: &str) -> Result<Credentials> {
        Ok(Credentials::Password {
            user: email.to_string(),
            password: self.password.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_imap::fake::FakeServer;
    use iris_imap::FolderKind;
    use iris_kernel::EventKind;
    use iris_store::NewAccount;
    use iris_types::Flags;

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_millis(secs * 1000)
    }

    fn message(n: u32) -> Vec<u8> {
        format!(
            "Subject: Message {n}\r\nFrom: Marie <marie@example.com>\r\n\
                 Message-ID: <m{n}@x>\r\n\r\nCorps du message.\r\n"
        )
        .into_bytes()
    }

    struct Fixture {
        engine: SyncEngine,
        store: Arc<Store>,
        server: Arc<FakeServer>,
        bus: EventBus,
        account: AccountId,
    }

    fn fixture() -> Fixture {
        fixture_with(EngineConfig::default())
    }

    fn fixture_with(config: EngineConfig) -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        let account = store
            .create_account(
                &NewAccount::new("moi@example.com", "imap.x.fr", "smtp.x.fr"),
                t(0),
            )
            .unwrap();

        let server = Arc::new(FakeServer::default());
        let bus = EventBus::new();
        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::clone(&server) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("motdepasse")),
            bus.clone(),
            config,
        );

        Fixture {
            engine,
            store,
            server,
            bus,
            account,
        }
    }

    #[tokio::test]
    async fn un_compte_inscrit_est_synchronise_au_premier_tour() {
        let f = fixture();
        f.server.deliver("INBOX", &message(1), Flags::NONE);
        f.server.deliver("INBOX", &message(2), Flags::NONE);

        assert_eq!(f.engine.load_accounts(t(0)).await.unwrap(), 1);
        let r = f.engine.tick(t(0)).await;

        assert_eq!(r.accounts_synced, 1);
        assert_eq!(r.messages_added, 2);
        assert!(r.failures.is_empty());
        assert_eq!(f.store.message_count().unwrap(), 2);
    }

    #[tokio::test]
    async fn les_dossiers_distants_sont_enregistres() {
        let f = fixture();
        f.server.add_folder("Archive", FolderKind::Archive);
        f.server.add_folder("Sent", FolderKind::Sent);

        f.engine.load_accounts(t(0)).await.unwrap();
        f.engine.tick(t(0)).await;

        let dossiers = f.store.folders(f.account).unwrap();
        assert_eq!(dossiers.len(), 3);
        assert!(dossiers.iter().any(|d| d.role == FolderRole::Archive));
    }

    #[test]
    fn un_certificat_emis_pour_un_autre_nom_propose_le_bon() {
        // The message rustls gives on shared hosting, as a user met it.
        let panne = AccountFailure {
            message: "network: négociation TLS avec mail.example.fr : invalid peer certificate: \
                      certificate not valid for name \"mail.example.fr\"; certificate is only \
                      valid for DnsName(\"autoconfig.host7.example.net\"), \
                      DnsName(\"autodiscover.host7.example.net\"), DnsName(\"host7.example.net\"), \
                      DnsName(\"cpanel.host7.example.net\")"
                .into(),
            needs_password: false,
            at: t(0),
        };
        let ecart = panne.certificate().expect("reconnu comme un certificat");
        assert_eq!(ecart.host, "mail.example.fr");
        assert_eq!(ecart.valid_for.len(), 4);
        assert_eq!(ecart.suggestion(), Some("host7.example.net"));
        assert!(panne.advice().contains("host7.example.net"));
        assert_eq!(
            panne.summary(),
            "certificate does not match the server name"
        );

        let reseau = AccountFailure {
            message: "network: connection refused".into(),
            needs_password: false,
            at: t(0),
        };
        assert!(reseau.certificate().is_none());
        assert_eq!(reseau.summary(), "server unreachable");
    }

    #[test]
    fn le_bilan_nomme_les_comptes_en_echec() {
        let rapport = SyncAllReport {
            total: 3,
            synced: 2,
            added: 0,
            failed: vec![("a@example.com".into(), "boom".into())],
            refused: 0,
        };
        assert!(rapport.summary().contains("a@example.com"));
    }

    #[tokio::test]
    async fn un_dossier_que_le_serveur_ne_liste_plus_est_oublie() {
        // Sans cela, un dossier supprimé ailleurs ou devenu simple branche restait
        // dans la colonne pour toujours, et revenait après chaque suppression.
        let f = fixture();
        f.store
            .upsert_folder(f.account, "dovecot/lda-dupes/locks", FolderRole::Other)
            .unwrap();

        f.engine.load_accounts(t(0)).await.unwrap();
        f.engine.tick(t(0)).await;

        let chemins: Vec<_> = f
            .store
            .folders(f.account)
            .unwrap()
            .into_iter()
            .map(|d| d.path)
            .collect();
        assert_eq!(chemins, ["INBOX"]);
    }

    #[tokio::test]
    async fn le_journal_est_rejoue_avant_la_relecture() {
        // Dans l'ordre inverse, l'action locale serait écrasée par l'état distant et
        // l'utilisateur verrait son geste défait sous ses yeux.
        let f = fixture();
        f.server.deliver("INBOX", &message(1), Flags::NONE);
        f.engine.load_accounts(t(0)).await.unwrap();
        f.engine.tick(t(0)).await;

        crate::replay::enqueue(
            &f.store,
            f.account,
            &crate::replay::OpPayload::SetFlags {
                folder: "INBOX".into(),
                uids: vec![1],
                flags: Flags::SEEN.0,
                add: true,
            },
            t(10),
        )
        .unwrap();

        let r = f.engine.tick(t(1000)).await;
        assert_eq!(r.ops_replayed, 1);

        let message = &f.store.thread_messages(iris_types::ThreadId(1)).unwrap()[0];
        assert!(
            message.flags.contains(Flags::SEEN),
            "l'action locale doit survivre"
        );
    }

    #[tokio::test]
    async fn un_compte_desactive_n_est_pas_synchronise() {
        let f = fixture();
        f.store.set_account_enabled(f.account, false).unwrap();
        assert_eq!(f.engine.load_accounts(t(0)).await.unwrap(), 0);
        assert_eq!(f.engine.tick(t(0)).await.accounts_synced, 0);
    }

    #[tokio::test]
    async fn un_serveur_injoignable_est_signale_et_retente_plus_tard() {
        let f = fixture();
        f.server.refuse_connections(true);
        f.engine.load_accounts(t(0)).await.unwrap();

        let mut abonne = f.bus.subscribe_kind(EventKind::Sync);
        let r = f.engine.tick(t(0)).await;

        assert_eq!(r.failures.len(), 1);
        assert!(abonne.drain().iter().any(|e| matches!(
            e,
            Event::SyncFailed {
                transient: true,
                ..
            }
        )));
        assert!(
            f.engine.suspended_accounts().await.is_empty(),
            "un échec isolé ne suspend pas"
        );
    }

    #[tokio::test]
    async fn les_echecs_repetes_finissent_par_suspendre_le_compte() {
        let f = fixture();
        f.server.refuse_connections(true);
        f.engine.load_accounts(t(0)).await.unwrap();

        for i in 0..10 {
            f.engine.tick(t(i * 100_000)).await;
        }
        assert_eq!(f.engine.suspended_accounts().await, [f.account]);
    }

    #[tokio::test]
    async fn un_compte_suspendu_reprend_apres_intervention() {
        let f = fixture();
        f.server.refuse_connections(true);
        f.engine.load_accounts(t(0)).await.unwrap();
        for i in 0..10 {
            f.engine.tick(t(i * 100_000)).await;
        }

        f.server.refuse_connections(false);
        f.engine.resume_account(f.account, t(2_000_000)).await;
        f.server.deliver("INBOX", &message(1), Flags::NONE);

        let r = f.engine.tick(t(2_000_000)).await;
        assert_eq!(r.accounts_synced, 1);
        assert_eq!(r.messages_added, 1);
    }

    #[tokio::test]
    async fn les_etapes_de_synchronisation_sont_publiees() {
        let f = fixture();
        f.engine.load_accounts(t(0)).await.unwrap();
        let mut abonne = f.bus.subscribe_kind(EventKind::Sync);

        f.engine.tick(t(0)).await;
        let etapes = abonne.drain();

        assert!(etapes.iter().any(|e| matches!(
            e,
            Event::SyncPhaseChanged {
                phase: SyncPhase::Connecting,
                ..
            }
        )));
        assert!(etapes.iter().any(|e| matches!(
            e,
            Event::SyncPhaseChanged {
                phase: SyncPhase::Idle,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn le_compte_regarde_est_synchronise_en_priorite() {
        let store = Arc::new(Store::in_memory().unwrap());
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(
                store
                    .create_account(
                        &NewAccount::new(format!("c{i}@x.fr"), "imap.x.fr", "s"),
                        t(0),
                    )
                    .unwrap(),
            );
        }

        let server = Arc::new(FakeServer::default());
        let bus = EventBus::new();
        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::clone(&server) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            bus.clone(),
            EngineConfig {
                concurrency: 1,
                ..Default::default()
            },
        );

        engine.load_accounts(t(0)).await.unwrap();
        engine.set_active_account(Some(ids[3]), t(0)).await;

        let mut abonne = bus.subscribe_kind(EventKind::Sync);
        engine.tick(t(0)).await;

        let premier = abonne
            .drain()
            .into_iter()
            .find_map(|e| e.account())
            .expect("au moins un compte synchronisé");
        assert_eq!(premier, ids[3]);
    }

    /// A server that takes the connection and never answers, and a working one for
    /// every other host.
    #[derive(Debug)]
    struct Muet(Arc<FakeServer>);

    #[async_trait::async_trait]
    impl Connector for Muet {
        async fn connect(
            &self,
            endpoint: &iris_imap::Endpoint,
            credentials: &iris_imap::Credentials,
        ) -> Result<Box<dyn iris_imap::ImapConnection>> {
            if endpoint.host == "muet.example.com" {
                std::future::pending::<()>().await;
            }
            self.0.connect(endpoint, credentials).await
        }
    }

    #[tokio::test]
    async fn a_server_that_never_answers_holds_back_no_other_account() {
        let store = Arc::new(Store::in_memory().unwrap());
        let muet = store
            .create_account(
                &NewAccount::new("a@example.com", "muet.example.com", "s"),
                t(0),
            )
            .unwrap();
        for i in 0..3 {
            store
                .create_account(
                    &NewAccount::new(format!("b{i}@example.com"), "imap.example.com", "s"),
                    t(0),
                )
                .unwrap();
        }
        let server = Arc::new(FakeServer::default());
        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::new(Muet(Arc::clone(&server))) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            EventBus::new(),
            EngineConfig {
                connect_timeout: std::time::Duration::from_millis(200),
                account_timeout: std::time::Duration::from_secs(5),
                ..Default::default()
            },
        );
        engine.load_accounts(t(0)).await.unwrap();

        // Everything, by hand: the silent one is given up, the others are done.
        let rapport = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            engine.sync_all(t(0), |_, _, _| {}),
        )
        .await
        .expect("sync_all came back");
        assert_eq!(rapport.synced, 3);
        assert_eq!(rapport.failed.len(), 1);
        assert!(rapport.failed[0].1.contains("did not answer"));
        // And it says so where the user looks.
        assert!(engine.failure(muet).is_some());

        // The background pass comes back too.
        let tour = tokio::time::timeout(std::time::Duration::from_secs(10), engine.tick(t(0)))
            .await
            .expect("tick came back");
        assert!(tour.failures.iter().all(|(a, _)| *a == muet));
    }

    #[tokio::test]
    async fn le_pool_borne_les_connexions_simultanees() {
        let store = Arc::new(Store::in_memory().unwrap());
        for i in 0..10 {
            store
                .create_account(
                    &NewAccount::new(format!("c{i}@x.fr"), "imap.x.fr", "s"),
                    t(0),
                )
                .unwrap();
        }

        let server = Arc::new(FakeServer::default());
        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::clone(&server) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            EventBus::new(),
            EngineConfig {
                pool: PoolConfig {
                    max_total: 2,
                    max_per_server: 2,
                    acquire_timeout: std::time::Duration::from_secs(5),
                },
                concurrency: 10,
                ..Default::default()
            },
        );

        engine.load_accounts(t(0)).await.unwrap();
        let r = engine.tick(t(0)).await;

        assert_eq!(r.accounts_synced, 10, "tous finissent par passer");
        assert_eq!(
            engine.pool_stats().in_use,
            0,
            "toutes les places sont rendues"
        );
    }

    #[tokio::test]
    async fn un_second_tour_immediat_ne_refait_rien() {
        let f = fixture();
        f.server.deliver("INBOX", &message(1), Flags::NONE);
        f.engine.load_accounts(t(0)).await.unwrap();
        f.engine.tick(t(0)).await;

        let r = f.engine.tick(t(1)).await;
        assert_eq!(r.accounts_synced, 0, "l'intervalle n'est pas écoulé");
    }

    #[tokio::test]
    async fn le_prochain_reveil_est_connu() {
        let f = fixture();
        f.engine.load_accounts(t(0)).await.unwrap();
        f.engine.tick(t(0)).await;

        let attente = f.engine.next_wakeup(t(0)).await.unwrap();
        assert!(attente > std::time::Duration::ZERO);
    }

    #[tokio::test]
    async fn un_dossier_illisible_ne_condamne_pas_le_compte() {
        let f = fixture();
        f.server.add_folder("Partagé", FolderKind::Other);
        f.server.deliver("INBOX", &message(1), Flags::NONE);
        f.engine.load_accounts(t(0)).await.unwrap();
        f.engine.tick(t(0)).await;

        // Le dossier disparaît côté serveur, mais reste connu localement.
        let r = f.engine.tick(t(100_000)).await;
        assert!(r.failures.is_empty());
    }

    #[tokio::test]
    async fn les_messages_synchronises_sont_indexes() {
        // Un message visible dans la liste mais introuvable à la recherche est un
        // défaut que l'utilisateur imputera à la recherche.
        let store = Arc::new(Store::in_memory().unwrap());
        store
            .create_account(&NewAccount::new("moi@example.com", "imap.x.fr", "s"), t(0))
            .unwrap();

        let index = Arc::new(iris_index::SearchIndex::in_memory().unwrap());
        let server = Arc::new(FakeServer::default());
        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::clone(&server) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            EventBus::new(),
            EngineConfig::default(),
        )
        .with_index(Arc::clone(&index));

        server.deliver(
            "INBOX",
            b"Subject: Devis refonte\r\nFrom: Marie <marie@x.fr>\r\nMessage-ID: <a@x>\r\n\r\nCorps.\r\n",
            Flags::NONE,
        );

        engine.load_accounts(t(0)).await.unwrap();
        engine.tick(t(0)).await;
        index.commit().unwrap();

        assert_eq!(index.search("refonte", 10).unwrap().len(), 1);
        assert_eq!(index.search("marie", 10).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn sans_index_la_synchronisation_fonctionne_quand_meme() {
        let f = fixture();
        f.server.deliver("INBOX", &message(1), Flags::NONE);
        f.engine.load_accounts(t(0)).await.unwrap();

        let r = f.engine.tick(t(0)).await;
        assert_eq!(r.messages_added, 1);
    }

    #[tokio::test]
    async fn a_remote_deletion_is_seen_on_the_next_pass_not_the_next_scan() {
        // The periodic scan came one pass in ten, and a quiet account is visited once
        // an hour: a message binned from a phone stayed in Inbox here for most of a day.
        let f = fixture_with(EngineConfig {
            deletion_scan_every: 3,
            ..Default::default()
        });
        for i in 1..=4 {
            f.server.deliver("INBOX", &message(i), Flags::NONE);
        }
        f.engine.load_accounts(t(0)).await.unwrap();
        f.engine.tick(t(0)).await;

        f.server.remove("INBOX", 2);

        // Cycle 1: not a scan cycle.
        let r = f.engine.tick(t(400_000)).await;
        assert_eq!(r.accounts_synced, 1);
        assert_eq!(r.messages_deleted, 1);
        assert_eq!(f.store.message_count().unwrap(), 3);
    }
}
