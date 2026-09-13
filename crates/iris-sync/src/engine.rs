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
    async fn credentials(&self, account: AccountId, email: &str) -> Result<Credentials>;
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
    /// gaspillerait l'essentiel du budget réseau pour un événement rare.
    pub deletion_scan_every: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            pool: PoolConfig::default(),
            schedule: ScheduleConfig::default(),
            folder: FolderSyncOptions::default(),
            concurrency: 4,
            deletion_scan_every: 10,
        }
    }
}

/// Bilan d'un tour de synchronisation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    pub accounts_synced: usize,
    pub messages_added: usize,
    pub flags_updated: usize,
    pub messages_deleted: usize,
    pub ops_replayed: usize,
    pub failures: Vec<(AccountId, String)>,
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
        }
    }

    /// Inscrit à l'ordonnancement tous les comptes connus du store.
    pub async fn load_accounts(&self, now: Timestamp) -> Result<usize> {
        let comptes = self.store.accounts()?;
        let mut ordonnanceur = self.scheduler.lock().await;
        let mut inscrits = 0;

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

    pub fn pool_stats(&self) -> iris_imap::pool::PoolStats {
        self.pool.stats()
    }

    /// Exécute un tour : synchronise les comptes dus, dans l'ordre de priorité.
    pub async fn tick(&self, now: Timestamp) -> TickReport {
        let dus = {
            let mut ordonnanceur = self.scheduler.lock().await;
            ordonnanceur.due(now)
        };

        let cycle = self.cycle.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let releve_suppressions = self.config.deletion_scan_every > 0
            && cycle % self.config.deletion_scan_every == 0;

        let mut rapport = TickReport::default();

        for account in dus.into_iter().take(self.config.concurrency) {
            match self.sync_account(account, now, releve_suppressions).await {
                Ok(bilan) => {
                    rapport.accounts_synced += 1;
                    rapport.messages_added += bilan.added;
                    rapport.flags_updated += bilan.flags_updated;
                    rapport.messages_deleted += bilan.deleted;
                    rapport.ops_replayed += bilan.ops_replayed;

                    let resultat = if bilan.added > 0 || bilan.flags_updated > 0 {
                        SyncOutcome::Changed { messages: bilan.added as u32 }
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

        let _place = self.pool.acquire(&compte.imap_host).await?;

        self.publish_phase(account, SyncPhase::Connecting);
        let identifiants = self.credentials.credentials(account, &compte.email).await?;
        let endpoint = if compte.imap_tls {
            Endpoint::tls(&compte.imap_host, compte.imap_port)
        } else {
            Endpoint::starttls(&compte.imap_host, compte.imap_port)
        };
        let mut conn = self.connector.connect(&endpoint, &identifiants).await?;

        let mut bilan = AccountReport::default();

        // Le journal d'abord. Dans l'ordre inverse, une action locale non encore
        // transmise serait écrasée par l'état distant.
        let en_attente = self.store.pending_ops(now, 100)?;
        let a_traiter: Vec<_> =
            en_attente.into_iter().filter(|o| o.account == account).collect();
        if !a_traiter.is_empty() {
            let r = replay_account(conn.as_mut(), &self.store, &a_traiter, now).await?;
            bilan.ops_replayed = r.applied;
        }

        // Découverte des dossiers.
        self.publish_phase(account, SyncPhase::ListingFolders);
        let distants = conn.list_folders().await?;
        for d in &distants {
            self.store.upsert_folder(account, &d.path, translate_kind(d.kind))?;
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

        let options = FolderSyncOptions { detect_deletions, ..self.config.folder };
        let mut ajoutes_par_dossier = Vec::new();

        for (index, dossier) in dossiers.iter().enumerate() {
            self.publish_phase(
                account,
                SyncPhase::FetchingHeaders { done: index as u32, total: dossiers.len() as u32 },
            );

            match sync_folder(conn.as_mut(), &self.store, account, dossier, options).await {
                Ok(r) => {
                    bilan.added += r.added;
                    bilan.flags_updated += r.flags_updated;
                    bilan.deleted += r.deleted;
                    if r.added > 0 {
                        ajoutes_par_dossier.push(dossier.id);
                    }
                }
                // Un dossier illisible — droits insuffisants, boîte partagée
                // disparue — ne doit pas condamner le compte entier.
                Err(e) => tracing::warn!(
                    compte = %account, dossier = %dossier.path, erreur = %e,
                    "dossier ignoré"
                ),
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
        Ok(bilan)
    }

    fn publish_phase(&self, account: AccountId, phase: SyncPhase) {
        self.bus.publish(Event::SyncPhaseChanged { account, phase });
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AccountReport {
    added: usize,
    flags_updated: usize,
    deleted: usize,
    ops_replayed: usize,
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

/// Fournisseur d'identifiants simulé.
#[derive(Debug, Clone)]
pub struct StaticCredentials {
    pub password: String,
}

impl StaticCredentials {
    pub fn new(password: impl Into<String>) -> Self {
        Self { password: password.into() }
    }
}

#[async_trait]
impl CredentialsProvider for StaticCredentials {
    async fn credentials(&self, _account: AccountId, email: &str) -> Result<Credentials> {
        Ok(Credentials::Password { user: email.to_string(), password: self.password.clone() })
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
        format!("Subject: Message {n}\r\nFrom: Marie <marie@example.com>\r\n\
                 Message-ID: <m{n}@x>\r\n\r\nCorps du message.\r\n")
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
            .create_account(&NewAccount::new("moi@example.com", "imap.x.fr", "smtp.x.fr"), t(0))
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

        Fixture { engine, store, server, bus, account }
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
        assert!(message.flags.contains(Flags::SEEN), "l'action locale doit survivre");
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
        assert!(abonne.drain().iter().any(|e| matches!(e, Event::SyncFailed { transient: true, .. })));
        assert!(f.engine.suspended_accounts().await.is_empty(), "un échec isolé ne suspend pas");
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
            Event::SyncPhaseChanged { phase: SyncPhase::Connecting, .. }
        )));
        assert!(etapes.iter().any(|e| matches!(
            e,
            Event::SyncPhaseChanged { phase: SyncPhase::Idle, .. }
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
            EngineConfig { concurrency: 1, ..Default::default() },
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

    #[tokio::test]
    async fn le_pool_borne_les_connexions_simultanees() {
        let store = Arc::new(Store::in_memory().unwrap());
        for i in 0..10 {
            store
                .create_account(&NewAccount::new(format!("c{i}@x.fr"), "imap.x.fr", "s"), t(0))
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
        assert_eq!(engine.pool_stats().in_use, 0, "toutes les places sont rendues");
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
    async fn le_releve_des_suppressions_est_periodique() {
        // Le faire à chaque tour gaspillerait l'essentiel du budget réseau.
        let f = fixture_with(EngineConfig { deletion_scan_every: 3, ..Default::default() });
        for i in 1..=4 {
            f.server.deliver("INBOX", &message(i), Flags::NONE);
        }
        f.engine.load_accounts(t(0)).await.unwrap();
        f.engine.tick(t(0)).await;

        f.server.remove("INBOX", 2);

        // Tours 1 et 2 : pas de relevé.
        f.engine.tick(t(100_000)).await;
        f.engine.tick(t(200_000)).await;
        assert_eq!(f.store.message_count().unwrap(), 4);

        // Tour 3 : relevé.
        let r = f.engine.tick(t(300_000)).await;
        assert_eq!(r.messages_deleted, 1);
    }
}
