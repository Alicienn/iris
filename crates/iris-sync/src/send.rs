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
use iris_smtp::{Outbox, OutboxEvent, Outgoing, ReplyScope, ReplyTarget, SendHandle};
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
    /// The mailbox each queued message is from, to file it in that mailbox's Sent
    /// folder once it has gone.
    senders: std::sync::Mutex<std::collections::HashMap<SendHandle, iris_types::AccountId>>,
}

impl SendService {
    pub fn new(engine: Arc<SyncEngine>, outbox: Arc<Outbox>, bus: EventBus) -> Self {
        Self {
            engine,
            outbox,
            bus,
            senders: Default::default(),
        }
    }

    pub fn outbox(&self) -> &Arc<Outbox> {
        &self.outbox
    }

    /// Prepares a brand new message.
    ///
    /// Recipients are parsed from what the user typed, which is a comma or
    /// semicolon separated list, because that is what everyone types and refusing it
    /// would teach nothing. An address that does not parse is reported by name rather
    /// than silently dropped: a message quietly sent to three of four people is worse
    /// than one that refuses to go.
    /// Everything a written message can carry.
    ///
    /// A struct rather than eight arguments: the next field to arrive should not
    /// change the shape of every call site, and eight positional strings is a
    /// swapped pair waiting to happen.
    pub fn compose_full(&self, draft: &Draft) -> Result<Outgoing> {
        let compte = self
            .engine
            .store()
            .account(draft.account)?
            .ok_or_else(|| Error::store(format!("account {} not found", draft.account)))?;

        let mut recipients = Vec::new();
        let mut copies = Vec::new();
        let mut blind = Vec::new();

        for (champ, texte, cible) in [
            ("To", &draft.to, &mut recipients),
            ("Cc", &draft.cc, &mut copies),
            ("Bcc", &draft.bcc, &mut blind),
        ] {
            let (bons, mauvais) = parse_recipients(texte);
            if let Some(faux) = mauvais.first() {
                return Err(Error::Config(format!(
                    "{champ}: \"{faux}\" is not an email address"
                )));
            }
            *cible = bons;
        }

        if recipients.is_empty() && copies.is_empty() && blind.is_empty() {
            return Err(Error::Config("no recipient".into()));
        }

        let from = if compte.display_name.trim().is_empty() {
            iris_types::Address::new(compte.email.clone())
        } else {
            iris_types::Address::named(compte.display_name.clone(), compte.email.clone())
        };

        let mut message = Outgoing::new(from, recipients, draft.subject.trim());
        message.cc = copies;
        message.bcc = blind;
        message.text_body = avec_signature(&draft.body, &compte.signature);
        message.attachments = draft.attachments.clone();
        message.date = crate::engine::now_utc();
        Ok(message)
    }

    /// A message still being written, as it is kept in the Drafts folder.
    ///
    /// Lenient where sending is strict: an address half typed is left out rather than
    /// refused, and no recipient at all is fine. The signature is not added: it comes
    /// with the send, and a draft reopened would otherwise carry it twice.
    pub fn compose_draft(&self, draft: &Draft) -> Result<Outgoing> {
        let compte = self
            .engine
            .store()
            .account(draft.account)?
            .ok_or_else(|| Error::store(format!("account {} not found", draft.account)))?;
        let from = if compte.display_name.trim().is_empty() {
            iris_types::Address::new(compte.email.clone())
        } else {
            iris_types::Address::named(compte.display_name.clone(), compte.email.clone())
        };
        let mut message = Outgoing::new(from, parse_recipients(&draft.to).0, draft.subject.trim());
        message.cc = parse_recipients(&draft.cc).0;
        message.bcc = parse_recipients(&draft.bcc).0;
        message.text_body = draft.body.clone();
        message.attachments = draft.attachments.clone();
        message.date = crate::engine::now_utc();
        Ok(message)
    }

    /// Keeps a message being written in its account's Drafts folder, on the server:
    /// found again there from any device. `Ok(false)` when the account has no such
    /// folder.
    pub async fn save_draft(&self, draft: &Draft) -> Result<bool> {
        let message = self.compose_draft(draft)?;
        let brut = iris_smtp::message_bytes(&message)?;
        self.append_to(
            draft.account,
            FolderRole::Drafts,
            &brut,
            Flags(Flags::DRAFT.0 | Flags::SEEN.0),
        )
        .await
    }

    pub fn compose_new(
        &self,
        account: iris_types::AccountId,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<Outgoing> {
        let compte = self
            .engine
            .store()
            .account(account)?
            .ok_or_else(|| Error::store(format!("account {account} not found")))?;

        let (recipients, rejected) = parse_recipients(to);
        if let Some(bad) = rejected.first() {
            return Err(Error::Config(format!("\"{bad}\" is not an email address")));
        }
        if recipients.is_empty() {
            return Err(Error::Config("no recipient".into()));
        }

        let from = if compte.display_name.trim().is_empty() {
            iris_types::Address::new(compte.email.clone())
        } else {
            iris_types::Address::named(compte.display_name.clone(), compte.email.clone())
        };

        let mut message = Outgoing::new(from, recipients, subject.trim());
        message.text_body = body.to_string();
        message.date = crate::engine::now_utc();
        Ok(message)
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
            from: vec![Address {
                name: none_if_empty(&dernier.from_name),
                addr: dernier.from_addr.clone(),
            }],
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
        // Le texte de l'utilisateur passe devant la citation, qui suit. La signature
        // s'intercale entre les deux : sous ce qu'on vient d'écrire, au-dessus de ce
        // qu'on cite. La mettre tout en bas la placerait après le message de
        // l'interlocuteur, où personne ne la cherche.
        reponse.text_body = format!(
            "{}{}",
            avec_signature(body, &compte.signature),
            reponse.text_body
        );
        Ok(reponse)
    }

    /// De quoi préremplir l'éditeur pour transférer un fil : le sujet et la citation.
    ///
    /// Transférer passe par l'éditeur ordinaire plutôt que par un chemin à lui. Un
    /// transfert a besoin exactement de ce que l'éditeur sait déjà faire — choisir un
    /// destinataire, en mettre en copie, joindre un fichier, écrire un mot avant la
    /// citation — et un second chemin d'envoi serait un second endroit où les erreurs
    /// se corrigent une fois sur deux.
    ///
    /// Rend `(sujet, corps)`. Les destinataires restent vides : c'est la seule chose
    /// qu'un transfert ne peut pas deviner, et la seule qu'il faut donc demander.
    pub fn forward_prefill(&self, thread: ThreadId) -> Result<(String, String)> {
        let messages = self.engine.store().thread_messages(thread)?;
        let dernier = messages
            .last()
            .ok_or_else(|| Error::store(format!("fil {thread} vide")))?;

        let compte = self
            .engine
            .store()
            .account(dernier.account)?
            .ok_or_else(|| Error::store("compte du fil introuvable"))?;

        let cible = ReplyTarget {
            message_id: dernier.rfc_message_id.clone().map(RfcMessageId),
            references: self.reference_chain(dernier),
            subject: dernier.subject.clone(),
            from: vec![Address {
                name: none_if_empty(&dernier.from_name),
                addr: dernier.from_addr.clone(),
            }],
            to: vec![],
            cc: vec![],
            reply_to: vec![],
            date: dernier.received,
            text_body: self.original_body(dernier),
        };

        let identite = Address {
            name: none_if_empty(&compte.display_name),
            addr: compte.email.clone(),
        };

        let transfert = iris_smtp::forward(&cible, &identite, vec![]);
        Ok((transfert.subject, transfert.text_body))
    }

    /// Met une réponse en file. Elle partira après le délai d'annulation.
    pub fn queue(&self, message: Outgoing) -> Result<SendHandle> {
        message.validate().map_err(Error::Config)?;
        let compte = sender_account(self.engine.store(), &message.from.addr)
            .ok()
            .map(|c| c.id);
        // Held across the queueing: with no delay the message can be gone, and
        // `pump_outbox` asking whose it was, before the answer is written down.
        let mut expediteurs = self.senders.lock().expect("senders poisoned");
        let handle = self.outbox.queue(message);
        if let Some(compte) = compte {
            expediteurs.insert(handle, compte);
        }
        Ok(handle)
    }

    /// The mailbox a queued message is from, forgotten as it is read.
    fn take_sender(&self, handle: SendHandle) -> Option<iris_types::AccountId> {
        self.senders
            .lock()
            .expect("senders poisoned")
            .remove(&handle)
    }

    /// Files a message that has gone in its mailbox's Sent folder.
    pub async fn file_sent(&self, account: iris_types::AccountId, raw: &[u8]) -> SentOutcome {
        let mut bilan = SentOutcome::default();
        match self.append_to_sent(account, raw).await {
            Ok(true) => bilan.archived = true,
            Ok(false) => bilan.note = Some("aucun dossier « envoyés » connu".into()),
            // Un dépôt raté ne remet pas l'envoi en cause : le message est parti.
            Err(e) => {
                tracing::warn!(error = %e, "dépôt dans les messages envoyés");
                bilan.note = Some(format!("copie non déposée : {e}"));
            }
        }
        bilan
    }

    /// Retient un message encore en attente.
    pub fn cancel(&self, handle: SendHandle) -> bool {
        self.outbox.cancel(handle)
    }

    /// Le délai d'annulation des prochains messages.
    pub fn set_delay(&self, delay: std::time::Duration) {
        self.outbox.set_delay(delay);
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
        let mut bilan = self.file_sent(account, raw).await;

        match self.mark_waiting(thread, now) {
            Ok(change) => bilan.moved_to_waiting = change,
            Err(e) => tracing::warn!(error = %e, "passage en attente"),
        }

        bilan
    }

    /// Dépose une copie dans le dossier des messages envoyés.
    async fn append_to_sent(&self, account: iris_types::AccountId, raw: &[u8]) -> Result<bool> {
        // Gmail files what its SMTP server sends in Sent Mail itself: a copy of our own
        // would be a second one. The next pass brings its copy down.
        if let Some(compte) = self.engine.store().account(account)? {
            if server_files_sent_mail(&compte.imap_host) {
                return Ok(true);
            }
        }
        // Un message qu'on vient d'écrire est lu : le marquer autrement ferait
        // apparaître un non-lu dans ses propres messages envoyés.
        self.append_to(account, FolderRole::Sent, raw, Flags::SEEN)
            .await
    }

    /// Dépose un message dans le dossier d'un rôle donné. `Ok(false)` si le compte
    /// n'a pas ce dossier.
    async fn append_to(
        &self,
        account: iris_types::AccountId,
        role: FolderRole,
        raw: &[u8],
        flags: Flags,
    ) -> Result<bool> {
        let Some(dossier) = self
            .engine
            .store()
            .folders(account)?
            .into_iter()
            .find(|f| f.role == role)
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

        let mut conn = self
            .engine
            .connector()
            .connect(&point, &identifiants)
            .await?;
        conn.append(&dossier.path, raw, flags).await?;
        let _ = conn.logout().await;

        Ok(true)
    }

    /// Fait passer le fil en attente, si l'automatisme est actif.
    fn mark_waiting(&self, thread: ThreadId, _now: Timestamp) -> Result<bool> {
        let Some(ligne) = self.engine.store().thread_row(thread)? else {
            return Ok(false);
        };

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
                        p.html_body
                            .as_ref()
                            .map(|h| iris_mime::strip_tags(&h.html))
                            .unwrap_or_default()
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

/// Ajoute la signature au bas d'un corps de message.
///
/// Le séparateur est `-- ` suivi d'un retour à la ligne : c'est la convention, vieille
/// de quarante ans et comprise par tout le monde, qui dit à un autre client où le
/// message s'arrête et où la signature commence. Sans elle, une réponse cite la
/// signature comme si elle faisait partie du texte.
///
/// L'espace après les deux tirets n'est pas une coquille : c'est ce que la convention
/// exige, et un client sur deux ne reconnaît pas la ligne sans lui.
fn avec_signature(corps: &str, signature: &str) -> String {
    if signature.trim().is_empty() {
        return corps.to_string();
    }
    // Le séparateur déjà là veut dire que l'utilisateur l'a écrit lui-même, ou qu'on
    // repasse sur un brouillon signé. Deux signatures valent moins que zéro.
    if corps.contains("\n-- \n") || corps.starts_with("-- \n") {
        return corps.to_string();
    }

    let corps = corps.trim_end_matches('\n');
    format!("{corps}\n\n-- \n{}\n", signature.trim_end_matches('\n'))
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
                let expediteur = service.take_sender(handle);
                // Tied to a thread, it goes to Waiting too. Otherwise — which is how
                // every message was queued, so none was ever filed — it is filed in
                // the Sent folder of the mailbox it is from.
                let bilan = match (context.resolve(handle), expediteur) {
                    (Some((thread, account)), _) => {
                        service
                            .on_sent(thread, account, &outcome.raw, crate::engine::now_utc())
                            .await
                    }
                    (None, Some(account)) => service.file_sent(account, &outcome.raw).await,
                    (None, None) => continue,
                };
                context.finished(handle, Ok(bilan));
            }
            OutboxEvent::Failed { handle, error } => {
                service.take_sender(handle);
                tracing::warn!(error = %error, "envoi en échec");
                context.finished(handle, Err(error));
            }
            OutboxEvent::Cancelled { handle } => {
                service.take_sender(handle);
                context.cancelled(handle);
            }
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
    entries:
        std::sync::Mutex<std::collections::BTreeMap<SendHandle, (ThreadId, iris_types::AccountId)>>,
    finished: std::sync::Mutex<Vec<(SendHandle, std::result::Result<SentOutcome, String>)>>,
    cancelled: std::sync::Mutex<Vec<SendHandle>>,
}

impl InMemorySendContext {
    pub fn register(&self, handle: SendHandle, thread: ThreadId, account: iris_types::AccountId) {
        self.entries
            .lock()
            .unwrap()
            .insert(handle, (thread, account));
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

/// The mailbox that sends as `address`: its own, or the one it is an alias of.
pub fn sender_account(store: &iris_store::Store, address: &str) -> Result<iris_store::Account> {
    let adresse = address.trim();
    let comptes = store.accounts()?;
    if let Some(compte) = comptes
        .iter()
        .find(|c| c.email.eq_ignore_ascii_case(adresse))
    {
        return Ok(compte.clone());
    }
    let proprietaire = store
        .aliases()?
        .into_iter()
        .find(|a| a.address.eq_ignore_ascii_case(adresse))
        .map(|a| a.account);
    proprietaire
        .and_then(|id| comptes.into_iter().find(|c| c.id == id))
        .ok_or_else(|| Error::Config(format!("no mailbox sends as {adresse}")))
}

/// Whether the mailbox on `imap_host` keeps a copy of what is sent without being given
/// one. Gmail does, for every message its SMTP server takes.
fn server_files_sent_mail(imap_host: &str) -> bool {
    let hote = imap_host.trim_end_matches('.').to_ascii_lowercase();
    hote == "imap.gmail.com" || hote == "imap.googlemail.com"
}

/// A message being written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub account: iris_types::AccountId,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<iris_smtp::Attachment>,
}

impl Draft {
    /// An empty draft for one mailbox.
    pub fn new(account: iris_types::AccountId) -> Self {
        Self {
            account,
            to: String::new(),
            cc: String::new(),
            bcc: String::new(),
            subject: String::new(),
            body: String::new(),
            attachments: Vec::new(),
        }
    }
}

/// Splits what the user typed into addresses, and says which ones made no sense.
///
/// Commas and semicolons both separate, because both are typed and neither is wrong.
/// Whitespace around an address is trimmed, and an empty entry — a trailing comma —
/// is ignored rather than reported: it is a typing artefact, not a mistake.
pub fn parse_recipients(input: &str) -> (Vec<iris_types::Address>, Vec<String>) {
    let mut good = Vec::new();
    let mut bad = Vec::new();

    for piece in input.split([',', ';']) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }

        // "Marie <marie@x.fr>" is what a mail client offers when you pick from a
        // list, so it must round-trip through a field the user can also type into.
        let address = match (piece.rfind('<'), piece.rfind('>')) {
            (Some(open), Some(close)) if close > open + 1 => iris_types::Address::named(
                piece[..open].trim().trim_matches('"'),
                piece[open + 1..close].trim(),
            ),
            _ => iris_types::Address::new(piece),
        };
        if address.looks_valid() {
            good.push(address);
        } else {
            bad.push(piece.to_string());
        }
    }

    (good, bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmail_keeps_its_own_sent_copy() {
        assert!(server_files_sent_mail("imap.gmail.com"));
        assert!(server_files_sent_mail("IMAP.GMAIL.COM."));
        assert!(!server_files_sent_mail("mail.example.com"));
        assert!(!server_files_sent_mail("imap.gmail.com.example.com"));
    }

    #[test]
    fn une_signature_est_separee_par_la_convention() {
        // « -- » suivi d'une espace et d'un retour à la ligne : quarante ans d'usage, et
        // c'est ce qui dit à un autre client où le message s'arrête. Sans, une réponse
        // cite la signature comme si elle faisait partie du texte.
        let corps = avec_signature("Bonjour,", "Marie\n01 23 45 67 89");
        assert_eq!(corps, "Bonjour,\n\n-- \nMarie\n01 23 45 67 89\n");
        assert!(
            corps.contains("\n-- \n"),
            "l'espace après les tirets compte"
        );
    }

    #[test]
    fn sans_signature_le_corps_ne_bouge_pas() {
        // Le défaut. Personne ne veut découvrir une signature inventée par le programme
        // au bas d'un message déjà parti.
        assert_eq!(avec_signature("Bonjour,", ""), "Bonjour,");
        assert_eq!(avec_signature("Bonjour,", "   \n "), "Bonjour,");
    }

    #[test]
    fn on_ne_signe_pas_deux_fois() {
        // Le cas du brouillon repris, et celui de quelqu'un qui écrit son séparateur
        // lui-même. Deux signatures valent moins que zéro.
        let deja = "Bonjour,\n\n-- \nMarie\n";
        assert_eq!(avec_signature(deja, "Marie"), deja);
    }

    #[test]
    fn la_signature_ne_laisse_pas_de_lignes_vides_en_trop() {
        // Un éditeur laisse volontiers deux ou trois retours à la ligne à la fin ; les
        // empiler sous le séparateur ferait flotter la signature au milieu de rien.
        assert_eq!(
            avec_signature("Bonjour,\n\n\n", "Marie"),
            "Bonjour,\n\n-- \nMarie\n"
        );
    }

    use crate::engine::{EngineConfig, StaticCredentials};
    use iris_imap::fake::FakeServer;
    use iris_imap::{Connector, FolderKind};
    use iris_smtp::{FakeMailer, Mailer};
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
        server.add_folder("Drafts", FolderKind::Drafts);
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
        let (outbox, events) = Outbox::new(
            Arc::clone(&mailer) as Arc<dyn Mailer>,
            Duration::from_secs(10),
            tokio::runtime::Handle::current(),
        );
        let service = Arc::new(SendService::new(
            Arc::clone(&engine),
            Arc::new(outbox),
            bus.clone(),
        ));

        Fixture {
            service,
            engine,
            store,
            server,
            mailer,
            events,
            bus,
            _dir: dir,
        }
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
        assert!(
            reponse.text_body.contains("> "),
            "le message d'origine doit être cité"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn une_reponse_part_apres_le_delai() {
        let mut f = fixture();
        f.synchroniser().await;
        let reponse = f
            .service
            .compose_reply(ThreadId(1), "Merci.", ReplyScope::Sender)
            .unwrap();

        f.service.queue(reponse).unwrap();
        assert_eq!(f.mailer.count(), 0, "rien ne part immédiatement");

        // On attend l'événement plutôt qu'une durée : le test dit alors ce qui doit
        // arriver, pas combien de temps il faut patienter.
        assert!(matches!(
            f.events.recv().await,
            Some(OutboxEvent::Queued { .. })
        ));
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
        let reponse = f
            .service
            .compose_reply(ThreadId(1), "Oups.", ReplyScope::Sender)
            .unwrap();

        let handle = f.service.queue(reponse).unwrap();
        assert!(f.service.cancel(handle));

        assert!(matches!(
            f.events.recv().await,
            Some(OutboxEvent::Queued { .. })
        ));
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
            .on_sent(
                ThreadId(1),
                iris_types::AccountId(1),
                b"Subject: Re\r\n\r\nx",
                Timestamp::EPOCH,
            )
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
    async fn a_draft_is_kept_in_the_drafts_folder_even_half_written() {
        let f = fixture();
        f.synchroniser().await;

        let mut brouillon = Draft::new(iris_types::AccountId(1));
        // Half typed: kept out, not refused.
        brouillon.to = "mar".into();
        brouillon.subject = "Devis".into();
        brouillon.body = "À finir".into();
        assert!(f.service.save_draft(&brouillon).await.unwrap());
        assert_eq!(f.server.message_count("Drafts"), 1);
        assert_eq!(f.mailer.count(), 0, "a draft is not sent");
    }

    #[tokio::test]
    async fn le_passage_en_attente_est_annonce() {
        let f = fixture();
        f.synchroniser().await;
        let mut abonne = f.bus.subscribe_kind(iris_kernel::EventKind::Workflow);

        f.service
            .on_sent(
                ThreadId(1),
                iris_types::AccountId(1),
                b"x",
                Timestamp::EPOCH,
            )
            .await;

        let evenements = abonne.drain();
        assert!(evenements.iter().any(|e| matches!(
            e,
            Event::ThreadStateChanged {
                cause: TransitionCause::ReplySent,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn un_depot_impossible_ne_remet_pas_l_envoi_en_cause() {
        // Le message est parti : le dire autrement serait un mensonge.
        let store = Arc::new(Store::in_memory().unwrap());
        store
            .create_account(
                &NewAccount::new("moi@x.fr", "imap.x.fr", "s"),
                Timestamp::EPOCH,
            )
            .unwrap();

        let server = Arc::new(FakeServer::default()); // pas de dossier « Sent »
        server.deliver(
            "INBOX",
            b"Subject: A\r\nMessage-ID: <a@x>\r\n\r\nx\r\n",
            Flags::NONE,
        );

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
        let (outbox, _rx) = Outbox::new(
            Arc::clone(&mailer) as Arc<dyn Mailer>,
            Duration::from_secs(1),
            tokio::runtime::Handle::current(),
        );
        let service = SendService::new(engine, Arc::new(outbox), bus);

        let bilan = service
            .on_sent(
                ThreadId(1),
                iris_types::AccountId(1),
                b"x",
                Timestamp::EPOCH,
            )
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

        let reponse = f
            .service
            .compose_reply(ThreadId(1), "Merci.", ReplyScope::Sender)
            .unwrap();
        assert!(reponse.text_body.starts_with("Merci."));
        assert!(reponse.validate().is_ok());
    }

    #[tokio::test]
    async fn repondre_a_un_fil_vide_est_refuse() {
        let f = fixture();
        let e = f
            .service
            .compose_reply(ThreadId(999), "x", ReplyScope::Sender)
            .unwrap_err();
        assert!(e.to_string().contains("vide") || e.to_string().contains("introuvable"));
    }

    #[tokio::test]
    async fn un_message_invalide_n_entre_pas_dans_la_file() {
        let f = fixture();
        f.synchroniser().await;

        let mut reponse = f
            .service
            .compose_reply(ThreadId(1), "x", ReplyScope::Sender)
            .unwrap();
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
        let reponse = f
            .service
            .compose_reply(ThreadId(1), "Merci.", ReplyScope::Sender)
            .unwrap();
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

    #[tokio::test(start_paused = true)]
    async fn a_message_sent_lands_in_its_mailbox_sent_folder() {
        // Nothing registered it with a thread, as nothing in the application does: it
        // was sent, and never filed anywhere.
        let f = fixture();
        f.synchroniser().await;
        assert_eq!(f.server.message_count("Sent"), 0);

        let context = Arc::new(InMemorySendContext::default());
        let message = Outgoing::new(
            Address::new("Moi@Example.com"),
            vec![Address::new("someone@example.net")],
            "Hello",
        );
        f.service.queue(message).unwrap();

        let service = Arc::clone(&f.service);
        let ctx = Arc::clone(&context) as Arc<dyn SendContext>;
        tokio::spawn(pump_outbox(service, f.events, ctx));
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if context.finished_count() > 0 {
                break;
            }
        }

        assert_eq!(context.finished_count(), 1);
        assert!(context.last_outcome().unwrap().unwrap().archived);
        assert_eq!(f.server.message_count("Sent"), 1);
    }

    #[test]
    fn an_alias_is_sent_by_the_mailbox_it_belongs_to() {
        let store = Store::in_memory().unwrap();
        let compte = store
            .create_account(
                &NewAccount::new("moi@example.com", "imap.x.fr", "smtp.x.fr"),
                Timestamp::EPOCH,
            )
            .unwrap();
        store
            .add_alias(compte, "ventes@example.com", "Ventes")
            .unwrap();

        assert_eq!(
            sender_account(&store, " Ventes@example.com ").unwrap().id,
            compte
        );
        assert!(sender_account(&store, "autre@example.com").is_err());
    }

    #[tokio::test]
    async fn l_etat_de_la_file_est_consultable() {
        let f = fixture();
        let etat = f.service.status();
        assert_eq!(etat.pending, 0);
        assert_eq!(etat.delay_secs, 10);
    }

    // --- Composing a new message ---

    #[test]
    fn recipients_are_split_on_commas_and_semicolons() {
        // Both are typed by real people, and neither is wrong.
        let (good, bad) = parse_recipients("a@x.fr, b@x.fr; c@x.fr");
        assert_eq!(good.len(), 3);
        assert!(bad.is_empty());
    }

    #[test]
    fn whitespace_and_trailing_separators_are_forgiven() {
        // A trailing comma is a typing artefact, not a mistake.
        let (good, bad) = parse_recipients("  a@x.fr ,, b@x.fr ,");
        assert_eq!(good.len(), 2);
        assert!(bad.is_empty());
    }

    #[test]
    fn a_named_address_keeps_its_name() {
        let (good, _) = parse_recipients("Marie Dupont <marie@x.fr>");
        assert_eq!(good[0].addr, "marie@x.fr");
        assert_eq!(good[0].display(), "Marie Dupont");
    }

    #[test]
    fn a_nonsense_recipient_is_named_rather_than_dropped() {
        // A message quietly sent to three of four people is worse than one that
        // refuses to go.
        let (good, bad) = parse_recipients("a@x.fr, not-an-address");
        assert_eq!(good.len(), 1);
        assert_eq!(bad, ["not-an-address"]);
    }

    #[tokio::test]
    async fn a_new_message_is_composed_from_the_account() {
        let f = fixture();
        let account = f.store.accounts().unwrap()[0].id;

        let message = f
            .service
            .compose_new(account, "marie@x.fr", "Quote", "Here it is.")
            .unwrap();

        assert_eq!(message.to.len(), 1);
        assert_eq!(message.subject, "Quote");
        assert_eq!(message.text_body, "Here it is.");
        assert!(message.validate().is_ok());
    }

    #[tokio::test]
    async fn a_new_message_without_a_recipient_is_refused() {
        let f = fixture();
        let account = f.store.accounts().unwrap()[0].id;

        let error = f
            .service
            .compose_new(account, "   ", "Quote", "Body")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no recipient"), "got: {error}");
    }

    #[tokio::test]
    async fn a_bad_recipient_stops_the_whole_message() {
        let f = fixture();
        let account = f.store.accounts().unwrap()[0].id;

        let error = f
            .service
            .compose_new(account, "marie@x.fr, oops", "Quote", "Body")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("oops"),
            "the offending address must be named: {error}"
        );
    }

    #[tokio::test]
    async fn a_new_message_is_not_a_reply_to_anything() {
        // Threading on an unrelated Message-ID would drop the message into someone
        // else's conversation.
        let f = fixture();
        let account = f.store.accounts().unwrap()[0].id;

        let message = f
            .service
            .compose_new(account, "marie@x.fr", "Quote", "Body")
            .unwrap();
        assert!(message.in_reply_to.is_none());
        assert!(message.references.is_empty());
    }

    #[tokio::test]
    async fn composing_for_an_unknown_account_fails_clearly() {
        let f = fixture();
        let error = f
            .service
            .compose_new(iris_types::AccountId(999), "a@x.fr", "S", "B")
            .unwrap_err()
            .to_string();
        assert!(error.contains("not found"), "got: {error}");
    }
}
