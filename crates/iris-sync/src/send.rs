//! L'envoi d'une réponse.
//!
//! C'est le geste qui fait avancer le workflow, et il enchaîne quatre choses qui
//! doivent toutes réussir pour que l'utilisateur soit servi :
//!
//! 1. **composer** correctement — destinataires, sujet, et surtout la chaîne
//!    `References`, sans laquelle la conversation se casse chez le destinataire ;
//! 2. **différer** l'envoi de dix secondes, pendant lesquelles un seul geste retient
//!    le message ;
//! 3. **déposer** une copie dans les messages envoyés, faute de quoi le message
//!    serait invisible depuis le téléphone et l'utilisateur croirait ne pas l'avoir
//!    envoyé ;
//! 4. **faire passer le fil en attente**, ce qui est le sens même du geste.
//!
//! Les deux derniers points ne conditionnent pas le premier : un dépôt qui échoue ne
//! doit pas faire croire que le message n'est pas parti — il l'est.

use crate::engine::SyncEngine;
use iris_kernel::{Event, EventBus};
use iris_smtp::{Mailer, Outbox, OutboxEvent, Outgoing, ReplyScope, ReplyTarget, SendHandle};
use iris_store::FolderRole;
use iris_types::{
    Address, Error, Flags, Result, RfcMessageId, ThreadId, Timestamp, TransitionCause,
};
use std::sync::Arc;

/// Le service d'envoi.
#[derive(Debug)]
pub struct SendService {
    engine: Arc<SyncEngine>,
    outbox: Arc<Outbox>,
    bus: EventBus,
}

impl SendService {
    pub fn new(engine: Arc<SyncEngine>, outbox: Arc<Outbox>, bus: EventBus) -> Self {
        Self { engine, outbox, bus }
    }

    pub fn outbox(&self) -> &Arc<Outbox> {
        &self.outbox
    }

    /// Prépare une réponse au dernier message d'un fil.
    ///
    /// La composition est séparée de l'envoi : l'interface peut ainsi montrer les
    /// destinataires et le sujet avant que quoi que ce soit ne parte.
    pub fn compose_reply(
        &self,
        thread: ThreadId,
        body: &str,
        scope: ReplyScope,
    ) -> Result<Outgoing> {
        let messages = self.engine.store().thread_messages(thread)?;
        let dernier = messages
            .last()
            .ok_or_else(|| Error::store(format!("fil {thread} vide")))?;

        let compte = self
            .engine
            .store()
            .account(dernier.account)?
            .ok_or_else(|| Error::store("compte du fil introuvable"))?;

        // Le corps cité vient de ce qui est déjà téléchargé. S'il ne l'est pas,
        // l'aperçu suffit : citer trois lignes vaut mieux que faire attendre le
        // réseau au moment où l'utilisateur veut écrire.
        let corps_original = self.original_body(dernier);

        let cible = ReplyTarget {
            message_id: dernier.rfc_message_id.clone().map(RfcMessageId),
            references: self.reference_chain(dernier),
            subject: dernier.subject.clone(),
            from: vec![Address { name: none_if_empty(&dernier.from_name), addr: dernier.from_addr.clone() }],
            to: vec![],
            cc: vec![],
            reply_to: vec![],
            date: dernier.received,
            text_body: corps_original,
        };

        let identite = Address {
            name: none_if_empty(&compte.display_name),
            addr: compte.email.clone(),
        };

        let mut reponse = iris_smtp::reply(&cible, &identite, scope);
        // Le texte de l'utilisateur passe devant la citation, qui suit.
        reponse.text_body = format!("{body}{}", reponse.text_body);
        Ok(reponse)
    }

    /// Met une réponse en file. Elle partira après le délai d'annulation.
    pub fn queue(&self, message: Outgoing) -> Result<SendHandle> {
        message.validate().map_err(Error::Config)?;
        Ok(self.outbox.queue(message))
    }

    /// Retient un message encore en attente.
    pub fn cancel(&self, handle: SendHandle) -> bool {
        self.outbox.cancel(handle)
    }

    /// Traite ce qui suit un envoi réussi : dépôt dans les messages envoyés et
    /// passage du fil en attente.
    pub async fn on_sent(
        &self,
        thread: ThreadId,
        account: iris_types::AccountId,
        raw: &[u8],
        now: Timestamp,
    ) -> SentOutcome {
        let mut bilan = SentOutcome::default();

        match self.append_to_sent(account, raw).await {
            Ok(true) => bilan.archived = true,
            Ok(false) => bilan.note = Some("aucun dossier « envoyés » connu".into()),
            // Un dépôt raté ne remet pas l'envoi en cause : le message est parti.
            Err(e) => {
                tracing::warn!(erreur = %e, "dépôt dans les messages envoyés");
                bilan.note = Some(format!("copie non déposée : {e}"));
            }
        }

        match self.mark_waiting(thread, now) {
            Ok(change) => bilan.moved_to_waiting = change,
            Err(e) => tracing::warn!(erreur = %e, "passage en attente"),
        }

        bilan
    }

    /// Dépose une copie dans le dossier des messages envoyés.
    async fn append_to_sent(&self, account: iris_types::AccountId, raw: &[u8]) -> Result<bool> {
        let Some(dossier) = self
            .engine
            .store()
            .folders(account)?
            .into_iter()
            .find(|f| f.role == FolderRole::Sent)
        else {
            return Ok(false);
        };

        let compte = self
            .engine
            .store()
            .account(account)?
            .ok_or_else(|| Error::store("compte introuvable"))?;

        let _place = self.engine.pool().acquire(&compte.imap_host).await?;
        let identifiants = self.engine.credentials_for(&compte).await?;
        let point = self.engine.endpoint_for(&compte);

        let mut conn = self.engine.connector().connect(&point, &identifiants).await?;
        // Un message qu'on vient d'écrire est lu : le marquer autrement ferait
        // apparaître un non-lu dans ses propres messages envoyés.
        conn.append(&dossier.path, raw, Flags::SEEN).await?;
        let _ = conn.logout().await;

        Ok(true)
    }

    /// Fait passer le fil en attente, si l'automatisme est actif.
    fn mark_waiting(&self, thread: ThreadId, _now: Timestamp) -> Result<bool> {
        let Some(ligne) = self.engine.store().thread_row(thread)? else { return Ok(false) };

        let resultat = iris_types::transition(
            ligne.state,
            TransitionCause::ReplySent,
            None,
            &self.engine.automation(),
        );

        if let iris_types::TransitionOutcome::Moved { from, to } = resultat {
            self.engine.store().set_thread_state(thread, to)?;
            self.bus.publish(Event::ThreadStateChanged {
                thread,
                from,
                to,
                cause: TransitionCause::ReplySent,
            });
            return Ok(true);
        }
        Ok(false)
    }

    /// Corps du message auquel on répond, tel qu'on peut le citer maintenant.
    fn original_body(&self, message: &iris_store::StoredMessage) -> String {
        let depuis_cache = message
            .body_blob
            .as_deref()
            .and_then(iris_types::BlobId::from_hex)
            .and_then(|id| self.engine.blobs().and_then(|b| b.get(id).ok().flatten()));

        match depuis_cache {
            Some(brut) => iris_mime::parse(&brut)
                .map(|p| {
                    p.text_body.clone().unwrap_or_else(|| {
                        p.html_body.as_ref().map(|h| iris_mime::strip_tags(&h.html)).unwrap_or_default()
                    })
                })
                .unwrap_or_else(|_| message.preview.clone()),
            None => message.preview.clone(),
        }
    }

    /// Reconstitue la chaîne `References` du fil.
    ///
    /// On la rebâtit à partir des messages connus plutôt que de la relire depuis le
    /// dernier message : sur un fil recollé de plusieurs sources, notre vue est plus
    /// complète que celle d'un seul en-tête.
    fn reference_chain(&self, dernier: &iris_store::StoredMessage) -> Vec<RfcMessageId> {
        let Ok(messages) = self.engine.store().thread_messages(dernier.thread) else {
            return Vec::new();
        };
        messages
            .iter()
            .filter(|m| m.id != dernier.id)
            .filter_map(|m| m.rfc_message_id.clone().map(RfcMessageId))
            .collect()
    }
}

fn none_if_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Ce qui a suivi un envoi.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SentOutcome {
    /// Une copie a été déposée dans les messages envoyés.
    pub archived: bool,
    /// Le fil est passé en attente.
    pub moved_to_waiting: bool,
    /// Précision à montrer à l'utilisateur, le cas échéant.
    pub note: Option<String>,
}

/// Fait tourner la file d'envoi et réagit à ses événements.
///
/// Le suivi vit ici plutôt que dans l'interface : ce qui doit arriver après un envoi
/// doit arriver même si la fenêtre est fermée entre-temps.
pub async fn pump_outbox(
    service: Arc<SendService>,
    mut events: tokio::sync::mpsc::UnboundedReceiver<OutboxEvent>,
    context: Arc<dyn SendContext>,
) {
    while let Some(evenement) = events.recv().await {
        match evenement {
            OutboxEvent::Sent { handle, outcome } => {
                let Some((thread, account)) = context.resolve(handle) else { continue };
                let bilan = service
                    .on_sent(thread, account, &outcome.raw, crate::engine::now_utc())
                    .await;
                context.finished(handle, Ok(bilan));
            }
            OutboxEvent::Failed { handle, error } => {
                tracing::warn!(erreur = %error, "envoi en échec");
                context.finished(handle, Err(error));
            }
            OutboxEvent::Cancelled { handle } => context.cancelled(handle),
            OutboxEvent::Queued { .. } => {}
        }
    }
}

/// Ce que l'appelant doit fournir pour relier un envoi à son fil.
pub trait SendContext: Send + Sync + std::fmt::Debug {
    /// Fil et compte associés à un envoi.
    fn resolve(&self, handle: SendHandle) -> Option<(ThreadId, iris_types::AccountId)>;
    fn finished(&self, handle: SendHandle, outcome: Result<SentOutcome>);
    fn cancelled(&self, handle: SendHandle);
}

/// Contexte en mémoire, suffisant pour une fenêtre.
#[derive(Debug, Default)]
pub struct InMemorySendContext {
    entries: std::sync::Mutex<std::collections::BTreeMap<SendHandle, (ThreadId, iris_types::AccountId)>>,
    finished: std::sync::Mutex<Vec<(SendHandle, std::result::Result<SentOutcome, String>)>>,
    cancelled: std::sync::Mutex<Vec<SendHandle>>,
}

impl InMemorySendContext {
    pub fn register(&self, handle: SendHandle, thread: ThreadId, account: iris_types::AccountId) {
        self.entries.lock().unwrap().insert(handle, (thread, account));
    }

    pub fn finished_count(&self) -> usize {
        self.finished.lock().unwrap().len()
    }

    pub fn last_outcome(&self) -> Option<std::result::Result<SentOutcome, String>> {
        self.finished.lock().unwrap().last().map(|(_, r)| r.clone())
    }

    pub fn cancelled_count(&self) -> usize {
        self.cancelled.lock().unwrap().len()
    }
}

impl SendContext for InMemorySendContext {
    fn resolve(&self, handle: SendHandle) -> Option<(ThreadId, iris_types::AccountId)> {
        self.entries.lock().unwrap().get(&handle).copied()
    }

    fn finished(&self, handle: SendHandle, outcome: Result<SentOutcome>) {
        self.finished
            .lock()
            .unwrap()
            .push((handle, outcome.map_err(|e| e.to_string())));
    }

    fn cancelled(&self, handle: SendHandle) {
        self.cancelled.lock().unwrap().push(handle);
    }
}

/// État affichable de la file d'envoi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxStatus {
    pub pending: usize,
    pub delay_secs: u64,
}

impl SendService {
    pub fn status(&self) -> OutboxStatus {
        OutboxStatus {
            pending: self.outbox.pending_count(),
            delay_secs: self.outbox.delay().as_secs(),
        }
    }
}

/// Expéditeur retenu pour un compte, construit à la demande.
pub fn mailer_for(
    account: &iris_store::Account,
    password: &str,
) -> Result<Arc<dyn Mailer>> {
    let expediteur = if account.smtp_tls {
        iris_smtp::LettreMailer::tls(&account.smtp_host, account.smtp_port, &account.email, password)?
    } else {
        iris_smtp::LettreMailer::starttls(
            &account.smtp_host,
            account.smtp_port,
            &account.email,
            password,
        )?
    };
    Ok(Arc::new(expediteur))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, StaticCredentials};
    use iris_imap::fake::FakeServer;
    use iris_imap::{Connector, FolderKind};
    use iris_smtp::FakeMailer;
    use iris_store::{NewAccount, Store};
    use std::time::Duration;

    struct Fixture {
        service: Arc<SendService>,
        engine: Arc<SyncEngine>,
        store: Arc<Store>,
        server: Arc<FakeServer>,
        mailer: Arc<FakeMailer>,
        events: tokio::sync::mpsc::UnboundedReceiver<OutboxEvent>,
        bus: EventBus,
        _dir: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        store
            .create_account(
                &NewAccount::new("moi@example.com", "imap.x.fr", "smtp.x.fr"),
                Timestamp::EPOCH,
            )
            .unwrap();

        let server = Arc::new(FakeServer::default());
        server.add_folder("Sent", FolderKind::Sent);
        server.deliver(
            "INBOX",
            b"Subject: Devis refonte\r\nFrom: Marie <marie@example.com>\r\n\
              Message-ID: <origine@x>\r\n\r\nBonjour, voici le devis.\r\n",
            Flags::NONE,
        );

        let dir = tempfile::tempdir().unwrap();
        let blobs =
            Arc::new(iris_blobs::BlobStore::open(dir.path().join("blobs"), 1 << 20).unwrap());

        let bus = EventBus::new();
        let engine = Arc::new(
            SyncEngine::new(
                Arc::clone(&store),
                Arc::clone(&server) as Arc<dyn Connector>,
                Arc::new(StaticCredentials::new("motdepasse")),
                bus.clone(),
                EngineConfig::default(),
            )
            .with_blobs(blobs),
        );

        let mailer = Arc::new(FakeMailer::new());
        let (outbox, events) =
            Outbox::new(Arc::clone(&mailer) as Arc<dyn Mailer>, Duration::from_secs(10));
        let service = Arc::new(SendService::new(
            Arc::clone(&engine),
            Arc::new(outbox),
            bus.clone(),
        ));

        Fixture { service, engine, store, server, mailer, events, bus, _dir: dir }
    }

    impl Fixture {
        async fn synchroniser(&self) {
            self.engine.load_accounts(Timestamp::EPOCH).await.unwrap();
            self.engine.tick(Timestamp::EPOCH).await;
        }

        /// Synchronise puis télécharge les corps, comme le fait l'ouverture d'un fil.
        async fn ouvrir_le_fil(&self) {
            self.synchroniser().await;
            self.engine.fetch_thread_bodies(ThreadId(1)).await;
        }
    }

    #[tokio::test]
    async fn une_reponse_est_composee_avec_sa_chaine_de_references() {
        // Sans References correcte, la conversation se casse chez le destinataire.
        let f = fixture();
        f.ouvrir_le_fil().await;

        let reponse = f
            .service
            .compose_reply(ThreadId(1), "C'est parfait, merci.", ReplyScope::Sender)
            .unwrap();

        assert_eq!(reponse.to[0].addr, "marie@example.com");
        assert_eq!(reponse.subject, "Re: Devis refonte");
        assert_eq!(reponse.in_reply_to.as_ref().unwrap().as_str(), "origine@x");
        assert!(reponse.text_body.starts_with("C'est parfait, merci."));
        assert!(reponse.text_body.contains("> "), "le message d'origine doit être cité");
    }

    #[tokio::test(start_paused = true)]
    async fn une_reponse_part_apres_le_delai() {
        let mut f = fixture();
        f.synchroniser().await;
        let reponse = f.service.compose_reply(ThreadId(1), "Merci.", ReplyScope::Sender).unwrap();

        f.service.queue(reponse).unwrap();
        assert_eq!(f.mailer.count(), 0, "rien ne part immédiatement");

        // On attend l'événement plutôt qu'une durée : le test dit alors ce qui doit
        // arriver, pas combien de temps il faut patienter.
        assert!(matches!(f.events.recv().await, Some(OutboxEvent::Queued { .. })));
        match f.events.recv().await {
            Some(OutboxEvent::Sent { .. }) => {}
            autre => panic!("attendu un envoi, obtenu {autre:?}"),
        }
        assert_eq!(f.mailer.count(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn une_reponse_annulee_ne_part_pas() {
        let mut f = fixture();
        f.synchroniser().await;
        let reponse = f.service.compose_reply(ThreadId(1), "Oups.", ReplyScope::Sender).unwrap();

        let handle = f.service.queue(reponse).unwrap();
        assert!(f.service.cancel(handle));

        assert!(matches!(f.events.recv().await, Some(OutboxEvent::Queued { .. })));
        match f.events.recv().await {
            Some(OutboxEvent::Cancelled { .. }) => {}
            autre => panic!("attendu une annulation, obtenu {autre:?}"),
        }
        assert_eq!(f.mailer.count(), 0);
    }

    #[tokio::test]
    async fn l_envoi_depose_une_copie_et_met_le_fil_en_attente() {
        let f = fixture();
        f.synchroniser().await;

        let bilan = f
            .service
            .on_sent(ThreadId(1), iris_types::AccountId(1), b"Subject: Re\r\n\r\nx", Timestamp::EPOCH)
            .await;

        assert!(bilan.archived, "une copie doit être déposée");
        assert!(bilan.moved_to_waiting);
        assert_eq!(f.server.message_count("Sent"), 1);
        assert_eq!(
            f.store.thread_row(ThreadId(1)).unwrap().unwrap().state,
            iris_types::WorkflowState::Waiting
        );
    }

    #[tokio::test]
    async fn le_passage_en_attente_est_annonce() {
        let f = fixture();
        f.synchroniser().await;
        let mut abonne = f.bus.subscribe_kind(iris_kernel::EventKind::Workflow);

        f.service
            .on_sent(ThreadId(1), iris_types::AccountId(1), b"x", Timestamp::EPOCH)
            .await;

        let evenements = abonne.drain();
        assert!(evenements.iter().any(|e| matches!(
            e,
            Event::ThreadStateChanged { cause: TransitionCause::ReplySent, .. }
        )));
    }

    #[tokio::test]
    async fn un_depot_impossible_ne_remet_pas_l_envoi_en_cause() {
        // Le message est parti : le dire autrement serait un mensonge.
        let store = Arc::new(Store::in_memory().unwrap());
        store
            .create_account(&NewAccount::new("moi@x.fr", "imap.x.fr", "s"), Timestamp::EPOCH)
            .unwrap();

        let server = Arc::new(FakeServer::default()); // pas de dossier « Sent »
        server.deliver("INBOX", b"Subject: A\r\nMessage-ID: <a@x>\r\n\r\nx\r\n", Flags::NONE);

        let bus = EventBus::new();
        let engine = Arc::new(SyncEngine::new(
            Arc::clone(&store),
            Arc::clone(&server) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            bus.clone(),
            EngineConfig::default(),
        ));
        engine.load_accounts(Timestamp::EPOCH).await.unwrap();
        engine.tick(Timestamp::EPOCH).await;

        let mailer = Arc::new(FakeMailer::new());
        let (outbox, _rx) =
            Outbox::new(Arc::clone(&mailer) as Arc<dyn Mailer>, Duration::from_secs(1));
        let service = SendService::new(engine, Arc::new(outbox), bus);

        let bilan = service
            .on_sent(ThreadId(1), iris_types::AccountId(1), b"x", Timestamp::EPOCH)
            .await;

        assert!(!bilan.archived);
        assert!(bilan.note.unwrap().contains("envoyés"));
        assert!(bilan.moved_to_waiting, "le workflow avance quand même");
    }

    #[tokio::test]
    async fn composer_sans_corps_telecharge_reste_possible() {
        // Citer trois lignes d'aperçu vaut mieux que faire attendre le réseau au
        // moment où l'utilisateur veut écrire.
        let f = fixture();
        f.synchroniser().await;

        let reponse = f.service.compose_reply(ThreadId(1), "Merci.", ReplyScope::Sender).unwrap();
        assert!(reponse.text_body.starts_with("Merci."));
        assert!(reponse.validate().is_ok());
    }

    #[tokio::test]
    async fn repondre_a_un_fil_vide_est_refuse() {
        let f = fixture();
        let e = f.service.compose_reply(ThreadId(999), "x", ReplyScope::Sender).unwrap_err();
        assert!(e.to_string().contains("vide") || e.to_string().contains("introuvable"));
    }

    #[tokio::test]
    async fn un_message_invalide_n_entre_pas_dans_la_file() {
        let f = fixture();
        f.synchroniser().await;

        let mut reponse =
            f.service.compose_reply(ThreadId(1), "x", ReplyScope::Sender).unwrap();
        reponse.to.clear();
        reponse.cc.clear();

        assert!(f.service.queue(reponse).is_err());
        assert_eq!(f.service.status().pending, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn le_suivi_relie_l_envoi_a_son_fil() {
        let f = fixture();
        f.synchroniser().await;

        let context = Arc::new(InMemorySendContext::default());
        let reponse = f.service.compose_reply(ThreadId(1), "Merci.", ReplyScope::Sender).unwrap();
        let handle = f.service.queue(reponse).unwrap();
        context.register(handle, ThreadId(1), iris_types::AccountId(1));

        let service = Arc::clone(&f.service);
        let ctx = Arc::clone(&context) as Arc<dyn SendContext>;
        tokio::spawn(pump_outbox(service, f.events, ctx));

        // Sous horloge suspendue, `sleep` avance le temps dès que toutes les tâches
        // sont au repos : l'attente est donc immédiate et déterministe.
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if context.finished_count() > 0 {
                break;
            }
        }

        assert_eq!(context.finished_count(), 1);
        let bilan = context.last_outcome().unwrap().unwrap();
        assert!(bilan.moved_to_waiting);
    }

    #[tokio::test]
    async fn l_etat_de_la_file_est_consultable() {
        let f = fixture();
        let etat = f.service.status();
        assert_eq!(etat.pending, 0);
        assert_eq!(etat.delay_secs, 10);
    }
}
