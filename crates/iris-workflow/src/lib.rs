//! `iris-workflow` — the one state machine, wired to the store and the bus.
//!
//! The transition rules themselves are pure and live in `iris-types`. This crate adds
//! the four things that need state:
//!
//! - **persistence**: the state is written before anything is published, so a
//!   subscriber that re-reads the database on receiving an event can never find it
//!   contradicting that event;
//! - **undo**: every reversible action is recorded with the shape the thread had
//!   before it. That is what makes keyboard triage usable — a mistake costs nothing;
//! - **replay**: flag changes are journalled so the server eventually hears about
//!   them, batched per folder rather than one command per message;
//! - **time**: waking due snoozes and following up on threads nobody answered.
//!
//! There is deliberately **one** implementation of each of those. Having the undo
//! stack in two places, or the snooze wake-up in two places, is not redundancy: it is
//! two behaviours that drift apart until they disagree, and then nobody knows which
//! one the user saw.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use iris_kernel::{Event, EventBus};
use iris_store::FolderRole;
use iris_store::{OpKind, Store};
use iris_types::{
    transition, AccountId, AutomationSettings, Error, Flags, FolderId, OpId, Result, Snooze,
    ThreadId, Timestamp, TransitionCause, TransitionOutcome, WorkflowState,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

/// How many actions can be undone.
///
/// Enough to walk back a whole triage session, few enough that the stack does not
/// become a second journal.
const UNDO_DEPTH: usize = 100;

/// How many actions a stack holds: a batch's entries count as one.
fn actions_in(stack: &[UndoEntry]) -> usize {
    let mut groupes = std::collections::HashSet::new();
    stack
        .iter()
        .filter(|e| e.group == 0 || groupes.insert(e.group))
        .count()
}

/// The shape a thread had before an action, so the action can be reversed.
///
/// It records more than the state: undoing a snooze that only restored the state
/// would leave the thread hidden, which is not what the user asked to undo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry {
    pub thread: ThreadId,
    pub state: WorkflowState,
    pub snoozed_until: Option<Timestamp>,
    /// Per-message flags, so read/unread and starring are reversible too.
    pub flags: Vec<(iris_types::MessageId, Flags)>,
    /// The moves the action asked of the server, so undoing it takes them back
    /// there too. Undo changed the state here only: an archived or binned thread
    /// stayed archived or binned on the server, and vanished again at the next sync.
    pub moves: Vec<MoveRecord>,
    pub at: Timestamp,
    /// The action on several threads it was part of (0: one thread). Undone and redone
    /// together: a hundred and fifty threads archived at once took a hundred
    /// Ctrl+Z, and the fifty first could never be undone.
    pub group: u64,
}

/// One message moved by an action, as undo needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveRecord {
    /// Journalled as `op`, perhaps already carried out by the server.
    Made {
        account: AccountId,
        op: OpId,
        from: String,
        /// Its UID in `from`, when known (not after a move by Message-ID).
        uid: Option<u32>,
        to: String,
        message_id: Option<String>,
    },
    /// Taken back before the server heard of it.
    Withdrawn {
        account: AccountId,
        from: String,
        uid: Option<u32>,
        to: String,
        message_id: Option<String>,
    },
}

impl MoveRecord {
    /// The mailbox and the `Message-ID` of the message moved, when it has one.
    pub fn account_and_message_id(&self) -> Option<(AccountId, &str)> {
        match self {
            Self::Made {
                account,
                message_id,
                ..
            }
            | Self::Withdrawn {
                account,
                message_id,
                ..
            } => message_id.as_deref().map(|id| (*account, id)),
        }
    }
}

/// What an action did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub thread: ThreadId,
    pub changed: bool,
    pub outcome: Option<TransitionOutcome>,
}

/// Où un déplacement envoie le courrier.
///
/// Deux façons de nommer un dossier, et la distinction compte. Un **rôle** est le même
/// sur tous les serveurs et doit exister : archiver sans dossier d'archives est une
/// erreur qu'il faut dire. Un **chemin** est un dossier que quelqu'un a créé, et un
/// compte qui ne l'a pas est simplement un compte qui ne l'a pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination<'a> {
    Role(iris_store::FolderRole),
    Path(&'a str),
}

impl Destination<'_> {
    fn describe(&self) -> &str {
        match self {
            Self::Role(role) => role.as_str(),
            Self::Path(chemin) => chemin,
        }
    }
}

/// Whether the mailbox on this IMAP host is Gmail, whose folders are labels.
fn is_gmail(imap_host: &str) -> bool {
    let hote = imap_host.trim_end_matches('.').to_ascii_lowercase();
    hote == "imap.gmail.com" || hote == "imap.googlemail.com"
}

/// Whether a copy of a message in a folder of this role goes with the thread.
///
/// Every copy used to, wherever it sat: archiving took one's sent replies out of
/// Sent and drafts into the archive, and on Gmail, where a folder is a label, moving
/// the label copies to All Mail stripped the labels Gmail's own archive keeps.
///
/// - Archive takes what is in the inbox, nothing filed elsewhere; on Gmail that is
///   exactly "remove the Inbox label".
/// - Delete takes everything but what was sent and drafts.
/// - Moving to a folder takes everything but what was sent and drafts; on Gmail, what
///   is in the inbox, the bin or the junk (moving the All Mail copy files the
///   conversation under the label without taking anything else away).
fn moves_copy(destination: Destination<'_>, gmail: bool, from: FolderRole) -> bool {
    use FolderRole as R;
    if matches!(from, R::Sent | R::Drafts) {
        return false;
    }
    match destination {
        // Out of the inbox, and only that: a message filed in a folder of one's own
        // (`Clients/ACME`) is filed already, and went to the archive with the reply.
        Destination::Role(R::Archive) => from == R::Inbox,
        Destination::Role(R::Trash) => from != R::Trash,
        // Back to the inbox: out of the bin and the junk, the rest stays filed.
        Destination::Role(R::Inbox) => matches!(from, R::Trash | R::Junk),
        Destination::Role(_) => true,
        Destination::Path(_) if gmail => matches!(from, R::Inbox | R::Trash | R::Junk),
        Destination::Path(_) => true,
    }
}

/// The workflow engine.
#[derive(Debug)]
pub struct Workflow {
    store: Arc<Store>,
    bus: EventBus,
    settings: RwLock<AutomationSettings>,
    undo: Mutex<Vec<UndoEntry>>,
    /// Ce qu'une annulation a défait, pour pouvoir le refaire.
    ///
    /// La pile est **vidée par toute nouvelle action**, comme partout ailleurs :
    /// rétablir après avoir fait autre chose entre-temps rejouerait une action dans un
    /// monde qui a changé sous elle, et le résultat ne serait celui qu'attend personne.
    redo: Mutex<Vec<UndoEntry>>,
    /// The last batch number given.
    batches: std::sync::atomic::AtomicU64,
}

thread_local! {
    /// The batch under way on this thread (0: none), and whether the rules make it.
    /// Per thread: a pass's rules and the user's own action at the same moment are
    /// two actions, not one.
    static LOT: std::cell::Cell<(u64, bool)> = const { std::cell::Cell::new((0, false)) };
}

impl Workflow {
    pub fn new(store: Arc<Store>, bus: EventBus, settings: AutomationSettings) -> Self {
        Self {
            store,
            bus,
            settings: RwLock::new(settings),
            undo: Mutex::new(Vec::new()),
            redo: Mutex::new(Vec::new()),
            batches: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// What follows on this thread, until `end_batch`, is one action for undo.
    pub fn begin_batch(&self) {
        self.begin(false);
    }

    /// The same, for what the rules do on their own: one undo for a whole pass, and
    /// the redo stack left as it was. Each change a rule made was an entry of its own
    /// and emptied redo, so Ctrl+Z went through the rules before reaching the user's
    /// own last action, and what they had undone could not be redone.
    pub fn begin_automatic_batch(&self) {
        self.begin(true);
    }

    fn begin(&self, automatique: bool) {
        let n = self
            .batches
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        LOT.with(|l| l.set((n, automatique)));
    }

    pub fn end_batch(&self) {
        LOT.with(|l| l.set((0, false)));
    }

    pub fn settings(&self) -> AutomationSettings {
        *self.settings.read().expect("poisoned settings")
    }

    pub fn set_settings(&self, s: AutomationSettings) {
        *self.settings.write().expect("poisoned settings") = s;
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    // --- State ---

    /// Applies a cause to a thread.
    ///
    /// The write comes before the publish: a subscriber that re-reads the database on
    /// receiving the event must find the new state there, never the old one.
    pub fn apply(
        &self,
        thread: ThreadId,
        cause: TransitionCause,
        target: Option<WorkflowState>,
        now: Timestamp,
    ) -> Result<TransitionOutcome> {
        self.apply_recorded(thread, cause, target, now, true)
    }

    /// `apply`, recorded for undo or not. What happens on its own (a reply arriving,
    /// a snooze or a follow-up coming due) is not: Ctrl+Z after a sync undid that
    /// instead of what the user had just done.
    fn apply_recorded(
        &self,
        thread: ThreadId,
        cause: TransitionCause,
        target: Option<WorkflowState>,
        now: Timestamp,
        recorded: bool,
    ) -> Result<TransitionOutcome> {
        let Some(row) = self.store.thread_row(thread)? else {
            return Err(Error::store(format!("thread {thread} not found")));
        };

        let outcome = transition(row.state, cause, target, &self.settings());

        if let TransitionOutcome::Moved { from, to } = outcome {
            // The snapshot is taken before the write: taken after, it would record
            // the new state and undo would restore what the user just asked for.
            //
            // The row was read a few lines above to decide the transition; reading it
            // again here would double the cost of the most frequent action in the
            // application for a value already in hand.
            let before = UndoEntry {
                thread,
                state: row.state,
                snoozed_until: row.snoozed_until,
                flags: Vec::new(),
                moves: Vec::new(),
                at: now,
                group: 0,
            };
            self.store.set_thread_state(thread, to)?;
            // Done is done: a snooze left on it woke later and brought it back.
            if to == WorkflowState::Done && row.snoozed_until.is_some() {
                self.store.clear_snooze(thread)?;
            }
            if recorded {
                self.record_undo(before);
            }
            self.bus.publish(Event::ThreadStateChanged {
                thread,
                from,
                to,
                cause,
            });
        }

        Ok(outcome)
    }

    /// The manual action, which is the most frequent one.
    pub fn set_state(
        &self,
        thread: ThreadId,
        state: WorkflowState,
        now: Timestamp,
    ) -> Result<TransitionOutcome> {
        self.apply(thread, TransitionCause::Manual, Some(state), now)
    }

    // --- Snoozing ---

    /// Snoozes a thread: it leaves the view without changing state.
    pub fn snooze(&self, thread: ThreadId, until: Timestamp, now: Timestamp) -> Result<bool> {
        let Some(row) = self.store.thread_row(thread)? else {
            return Ok(false);
        };
        // Same reasoning as `apply`: the row is already here.
        let before = UndoEntry {
            thread,
            state: row.state,
            snoozed_until: row.snoozed_until,
            flags: Vec::new(),
            moves: Vec::new(),
            at: now,
            group: 0,
        };

        let changed = self.store.snooze_thread(
            thread,
            Snooze {
                until,
                restore_to: row.state,
            },
        )?;

        if changed {
            self.record_undo(before);
            self.bus.publish(Event::ThreadSnoozed { thread, until });
        }
        Ok(changed)
    }

    pub fn unsnooze(&self, thread: ThreadId, now: Timestamp) -> Result<bool> {
        let before = self.snapshot(thread, now, false)?;
        let changed = self.store.clear_snooze(thread)?;
        if changed {
            self.record_undo(before);
            self.bus.publish(Event::ThreadUnsnoozed { thread });
        }
        Ok(changed)
    }

    // --- Flags ---

    /// Marks every message in a thread read or unread.
    pub fn set_read(&self, thread: ThreadId, read: bool, now: Timestamp) -> Result<bool> {
        let before = self.snapshot(thread, now, true)?;
        let messages = self.store.thread_messages(thread)?;

        let mut changed = false;
        let mut per_folder: BTreeMap<(AccountId, FolderId), Vec<u32>> = BTreeMap::new();

        for m in &messages {
            if m.flags.contains(Flags::SEEN) == read {
                continue;
            }
            self.store
                .set_message_flags(m.id, m.flags.set(Flags::SEEN, read))?;
            per_folder
                .entry((m.account, m.folder))
                .or_default()
                .push(m.uid);
            changed = true;
        }

        // One journalled operation per folder: fifty messages marked read at once
        // must not produce fifty IMAP commands.
        for ((account, folder), uids) in per_folder {
            self.journal_flags(account, folder, &uids, Flags::SEEN, read, now)?;
        }

        if changed {
            self.record_undo(before);
            if let Some(m) = messages.last() {
                self.bus.publish(Event::FlagsChanged {
                    message: m.id,
                    thread,
                });
            }
        }
        Ok(changed)
    }

    /// Stars or unstars a thread, through its most recent message.
    pub fn set_flagged(&self, thread: ThreadId, flagged: bool, now: Timestamp) -> Result<bool> {
        let before = self.snapshot(thread, now, true)?;
        let messages = self.store.thread_messages(thread)?;
        let Some(last) = messages.last() else {
            return Ok(false);
        };

        // Starring marks the newest message. Unstarring takes the star off every
        // message that has one: the thread shows starred if any is, and unstarring
        // the newest alone left a star on an older one, so it could not be undone.
        let cibles: Vec<&iris_store::StoredMessage> = if flagged {
            vec![last]
        } else {
            messages
                .iter()
                .filter(|m| m.flags.contains(Flags::FLAGGED))
                .collect()
        };
        let mut par_dossier: BTreeMap<(AccountId, FolderId), Vec<u32>> = BTreeMap::new();
        for m in &cibles {
            let updated = m.flags.set(Flags::FLAGGED, flagged);
            if updated == m.flags {
                continue;
            }
            self.store.set_message_flags(m.id, updated)?;
            par_dossier
                .entry((m.account, m.folder))
                .or_default()
                .push(m.uid);
        }
        if par_dossier.is_empty() {
            return Ok(false);
        }
        for ((account, folder), uids) in par_dossier {
            self.journal_flags(account, folder, &uids, Flags::FLAGGED, flagged, now)?;
        }

        self.record_undo(before);
        self.bus.publish(Event::FlagsChanged {
            message: last.id,
            thread,
        });
        Ok(true)
    }

    /// Moves every message in a thread to a folder, and marks the thread done.
    ///
    /// Archiving is two things at once, and both are expected: the mail leaves the
    /// inbox on the server, and the conversation leaves the queue here. Doing only
    /// the second would let the next sync put it straight back.
    pub fn archive(&self, thread: ThreadId, now: Timestamp) -> Result<bool> {
        self.move_thread(
            thread,
            Destination::Role(iris_store::FolderRole::Archive),
            OpKind::MoveMessage,
            now,
        )
    }

    /// Moves every message in a thread to the bin.
    ///
    /// Deleting means moving to the trash folder, never erasing. A client that
    /// destroys mail on a keystroke is a client nobody can afford to use quickly, and
    /// the whole point of keyboard triage is speed.
    pub fn delete(&self, thread: ThreadId, now: Timestamp) -> Result<bool> {
        self.move_thread(
            thread,
            Destination::Role(iris_store::FolderRole::Trash),
            OpKind::DeleteMessage,
            now,
        )
    }

    /// Brings a thread back to the inbox: its messages in the bin or the junk folder
    /// go back there on the server, it is no longer set aside or marked junk, and it
    /// is to do again.
    ///
    /// "Back to inbox" changed the state only. A deleted or junk thread stayed in its
    /// folder and set aside, and the queue, which leaves those out, never showed it.
    pub fn back_to_inbox(&self, thread: ThreadId, now: Timestamp) -> Result<bool> {
        self.move_thread(
            thread,
            Destination::Role(iris_store::FolderRole::Inbox),
            OpKind::MoveMessage,
            now,
        )
    }

    /// Range un fil dans un dossier nommé.
    ///
    /// Le glisser-déposer, et l'entrée « Déplacer vers » du menu. Le dossier est
    /// désigné par son nom unifié : le fil peut porter des messages sur plusieurs
    /// boîtes, et chacune range dans **son** dossier de ce nom. C'est ce que veut dire
    /// « un dossier est un nom, pas un endroit », appliqué au geste plutôt qu'à
    /// l'arborescence.
    ///
    /// Un compte qui n'a pas ce dossier voit ses messages rester où ils sont. Refuser
    /// le déplacement entier pour une boîte qui manque à l'appel punirait les autres.
    pub fn move_to_folder(&self, thread: ThreadId, path: &str, now: Timestamp) -> Result<bool> {
        self.move_thread(thread, Destination::Path(path), OpKind::MoveMessage, now)
    }

    /// The shared part of archiving and deleting.
    fn move_thread(
        &self,
        thread: ThreadId,
        destination: Destination<'_>,
        kind: OpKind,
        now: Timestamp,
    ) -> Result<bool> {
        let messages = self.store.thread_messages(thread)?;
        if messages.is_empty() {
            return Ok(false);
        }

        let mut before = self.snapshot(thread, now, true)?;
        let mut moved = 0;

        // Grouped per account, because the destination folder is per account and a
        // thread can span several of them once regrouping has run.
        let mut per_account: BTreeMap<AccountId, Vec<(FolderId, u32, Option<String>)>> =
            BTreeMap::new();
        for m in &messages {
            per_account.entry(m.account).or_default().push((
                m.folder,
                m.uid,
                m.rfc_message_id.clone(),
            ));
        }

        // Every account's destination is found before anything is journalled: one
        // without the folder used to fail after the earlier accounts' moves were
        // queued, and the thread then stayed where it was here while its mail left on
        // the server.
        let mut plans = Vec::new();
        // Mailboxes without that folder: their mail stays, the others' goes. One of
        // them failed the whole thread.
        let mut sans_dossier = 0;
        for (account, items) in per_account {
            let dossiers = self.store.folders(account)?;
            let trouve = match destination {
                // No archive folder (OVH, Gandi…): made, as other clients do. Archive
                // failed on such mailboxes, for good.
                Destination::Role(FolderRole::Archive) => {
                    match self.store.folder_for_role(account, FolderRole::Archive)? {
                        Some(f) => Some(f),
                        None => Some(self.make_archive_folder(account, &dossiers, now)?),
                    }
                }
                Destination::Role(role) => self.store.folder_for_role(account, role)?,
                // Un compte sans ce dossier est ignoré, pas fatal : le fil est peut-être
                // à cheval sur deux boîtes dont une seule a « Devis ».
                // By the name it is shown under: `Devis` here, `INBOX.Devis` there.
                Destination::Path(chemin) => {
                    match dossiers.iter().find(|f| f.path == chemin).or_else(|| {
                        dossiers
                            .iter()
                            .find(|f| iris_store::same_folder(&f.path, chemin))
                    }) {
                        Some(f) => Some(f.clone()),
                        None => continue,
                    }
                }
            };

            let Some(target) = trouve else {
                sans_dossier += 1;
                continue;
            };

            let gmail = self
                .store
                .account(account)?
                .is_some_and(|c| is_gmail(&c.imap_host));
            let role_de = |folder: FolderId| {
                dossiers
                    .iter()
                    .find(|f| f.id == folder)
                    .map(|f| f.role)
                    .unwrap_or(FolderRole::Other)
            };
            let mut choisis: Vec<_> = items
                .iter()
                .filter(|(folder, _, _)| {
                    *folder != target.id && moves_copy(destination, gmail, role_de(*folder))
                })
                .cloned()
                .collect();
            // A Gmail conversation already out of the inbox lives in All Mail: that
            // copy is the one to file under a label.
            if choisis.is_empty() && gmail && matches!(destination, Destination::Path(_)) {
                choisis = items
                    .iter()
                    .filter(|(folder, _, _)| role_de(*folder) == FolderRole::Archive)
                    .cloned()
                    .collect();
            }
            plans.push((account, target, choisis));
        }
        // No mailbox has it: said rather than pretended. Silently marking the thread
        // done would lose the mail on the next sync.
        if plans.is_empty() && sans_dossier > 0 {
            return Err(Error::Config(format!(
                "this account has no {} folder",
                destination.describe()
            )));
        }
        // Dropped on a folder none of its mailboxes has: nothing can move, and it was
        // marked done all the same.
        if plans.is_empty() {
            if let Destination::Path(chemin) = destination {
                return Err(Error::Config(format!(
                    "the mailbox of this conversation has no folder “{chemin}”"
                )));
            }
        }

        // Moved back to the inbox (out of the bin or the junk folder, most often): a
        // rescue, not a filing. It went to Done, and a thread deleted before kept its
        // mark, so it never came back to the queue.
        let vers_la_boite = matches!(destination, Destination::Role(FolderRole::Inbox))
            || matches!(destination, Destination::Path(_))
                && plans
                    .iter()
                    .any(|(_, cible, _)| cible.role == FolderRole::Inbox);
        // A rescue says it is not junk: the mark the spam filter's headers left on one
        // message hid the whole thread, even once back in the inbox. The server is
        // told with `$NotJunk`, set before the move so that it goes with the message,
        // and that the next sync does not mark it junk again from its headers.
        if vers_la_boite {
            for m in &messages {
                if !m.flags.contains(Flags::SPAM) {
                    continue;
                }
                self.store
                    .set_message_flags(m.id, m.flags.without(Flags::SPAM).with(Flags::NOT_JUNK))?;
                let Some(dossier) = self
                    .store
                    .folders(m.account)?
                    .into_iter()
                    .find(|f| f.id == m.folder)
                else {
                    continue;
                };
                let charge = iris_store::OpPayload::SetFlags {
                    folder: dossier.path,
                    uids: vec![m.uid],
                    flags: Flags::NOT_JUNK.0,
                    add: true,
                };
                self.store.enqueue_op(
                    m.account,
                    charge.kind(),
                    &charge.to_json(),
                    &charge.idempotency_key(m.account),
                    now,
                )?;
            }
        }

        for (account, target, choisis) in plans {
            for (folder, uid, message_id) in choisis {
                let (op, from) =
                    self.journal_move(account, folder, uid, &target.path, kind, now)?;
                before.moves.push(MoveRecord::Made {
                    account,
                    op,
                    from,
                    uid: Some(uid),
                    to: target.path.clone(),
                    message_id,
                });
                moved += 1;
            }
        }

        // Nothing to move is not the same as nothing to do. A thread whose messages
        // already sit in the bin — which is most of a mailbox that has been triaged
        // elsewhere — produced no journal entry, returned "unchanged", and so the
        // Delete button did nothing at all, over and over, with no way to tell why.
        // The thread still has to leave the queue: that is what the user asked for,
        // and the server has nothing left to be told.
        // Deleting also puts the thread aside at once. Its messages stay in their
        // folder until the journal replays; a thread already done kept its state, so
        // it stayed in the Done tab and Delete looked like it did nothing.
        let mis_de_cote = matches!(
            destination,
            Destination::Role(iris_store::FolderRole::Trash)
        ) && self.store.set_thread_put_aside(thread, Some(now))?;
        // Out of the bin to anywhere else: no longer set aside.
        if !matches!(
            destination,
            Destination::Role(iris_store::FolderRole::Trash)
        ) {
            self.store.set_thread_put_aside(thread, None)?;
        }

        let etat = if vers_la_boite {
            WorkflowState::Todo
        } else {
            WorkflowState::Done
        };
        if moved == 0 && before.state == etat && !mis_de_cote {
            return Ok(false);
        }

        // A snooze does not outlive the thread leaving: it woke up later and brought
        // an archived or deleted thread back into To do.
        self.store.clear_snooze(thread)?;

        // Locally the thread leaves the queue at once; the server hears about it when
        // the journal replays. That is invariant 3: nothing waits for the network.
        let previous = before.state;
        self.store.set_thread_state(thread, etat)?;
        self.record_undo(before);
        self.bus.publish(Event::ThreadStateChanged {
            thread,
            from: previous,
            to: etat,
            cause: TransitionCause::Manual,
        });
        Ok(true)
    }

    /// Makes an archive folder on a mailbox that has none: `Archives`, beside the
    /// others (under `INBOX.` where they all are), created on the server by the
    /// journal ahead of the moves into it.
    fn make_archive_folder(
        &self,
        account: AccountId,
        dossiers: &[iris_store::Folder],
        now: Timestamp,
    ) -> Result<iris_store::Folder> {
        let separateur = self
            .store
            .account(account)?
            .and_then(|c| c.folder_delimiter)
            .unwrap_or('.');
        let sous_la_boite = format!("INBOX{separateur}");
        let autres: Vec<_> = dossiers
            .iter()
            .filter(|f| f.role != FolderRole::Inbox)
            .collect();
        let chemin =
            if !autres.is_empty() && autres.iter().all(|f| f.path.starts_with(&sous_la_boite)) {
                format!("{sous_la_boite}Archives")
            } else {
                "Archives".to_string()
            };
        let charge = iris_store::OpPayload::CreateFolder {
            folder: chemin.clone(),
        };
        self.store.enqueue_op(
            account,
            charge.kind(),
            &charge.to_json(),
            &charge.idempotency_key(account),
            now,
        )?;
        let id = self
            .store
            .upsert_folder(account, &chemin, FolderRole::Archive)?;
        self.store
            .folders(account)?
            .into_iter()
            .find(|f| f.id == id)
            .ok_or_else(|| Error::store("archive folder not kept"))
    }

    /// Records a move for the server to carry out later: the operation, and the
    /// folder it moves from.
    fn journal_move(
        &self,
        account: AccountId,
        folder: FolderId,
        uid: u32,
        destination: &str,
        kind: OpKind,
        now: Timestamp,
    ) -> Result<(OpId, String)> {
        let source = self
            .store
            .folders(account)?
            .into_iter()
            .find(|f| f.id == folder)
            .map(|f| f.path)
            .unwrap_or_default();

        // Construite, pas écrite. La version précédente formatait le JSON à la main
        // et posait `"to"` là où le rejeu lisait `"target"` : chaque déplacement était
        // jugé illisible et abandonné, le fil quittait la file localement, et le
        // serveur n'en a jamais rien su.
        let charge = iris_store::OpPayload::Move {
            folder: source.clone(),
            uids: vec![uid],
            target: destination.to_string(),
        };
        let key = charge.idempotency_key(account);

        let op = self
            .store
            .enqueue_op(account, kind, &charge.to_json(), &key, now)?;
        Ok((op, source))
    }

    /// Takes back one move of an undone action, and says how to take that back in
    /// turn (for redo). A move the server has not heard of is withdrawn; one it has
    /// carried out is reversed, the message found by its `Message-ID` where it went.
    fn reverse_move(&self, record: MoveRecord, now: Timestamp) -> Result<Option<MoveRecord>> {
        let enfiler = |account: AccountId, charge: iris_store::OpPayload| {
            self.store.enqueue_op(
                account,
                charge.kind(),
                &charge.to_json(),
                &charge.idempotency_key(account),
                now,
            )
        };
        match record {
            MoveRecord::Made {
                account,
                op,
                from,
                uid,
                to,
                message_id,
            } => {
                if self.store.withdraw_op(op)? {
                    return Ok(Some(MoveRecord::Withdrawn {
                        account,
                        from,
                        uid,
                        to,
                        message_id,
                    }));
                }
                // Carried out already, and nothing to find the message by.
                let Some(id) = message_id else {
                    return Ok(None);
                };
                let op = enfiler(
                    account,
                    iris_store::OpPayload::MoveByMessageId {
                        folder: to.clone(),
                        message_ids: vec![id.clone()],
                        target: from.clone(),
                    },
                )?;
                Ok(Some(MoveRecord::Made {
                    account,
                    op,
                    from: to,
                    uid: None,
                    to: from,
                    message_id: Some(id),
                }))
            }
            MoveRecord::Withdrawn {
                account,
                from,
                uid,
                to,
                message_id,
            } => {
                let charge = match (uid, &message_id) {
                    (Some(uid), _) => iris_store::OpPayload::Move {
                        folder: from.clone(),
                        uids: vec![uid],
                        target: to.clone(),
                    },
                    (None, Some(id)) => iris_store::OpPayload::MoveByMessageId {
                        folder: from.clone(),
                        message_ids: vec![id.clone()],
                        target: to.clone(),
                    },
                    (None, None) => return Ok(None),
                };
                let op = enfiler(account, charge)?;
                Ok(Some(MoveRecord::Made {
                    account,
                    op,
                    from,
                    uid,
                    to,
                    message_id,
                }))
            }
        }
    }

    // --- Undo ---

    /// Reverses the last action.
    ///
    /// The reversal is **not** pushed onto the stack: without that rule, undoing
    /// twice would replay the action instead of walking further back.
    pub fn undo(&self, now: Timestamp) -> Result<Option<UndoEntry>> {
        let Some(entry) = self.pop_undo() else {
            return Ok(None);
        };
        // The rest of its batch goes with it.
        let reste = self.pop_rest_of(&self.undo, entry.group);
        let premier = self.restore(entry, now, true)?;
        for autre in reste {
            self.restore(autre, now, true)?;
        }
        Ok(premier)
    }

    /// Refait ce que la dernière annulation a défait.
    ///
    /// Exactement la même mécanique, dans l'autre sens. Une entrée décrit « remets le
    /// fil dans cet état » : annuler et rétablir sont la même opération, appliquée à
    /// des instantanés pris à deux moments. Écrire deux implémentations en ferait deux
    /// choses qui finiraient par ne plus se répondre.
    pub fn redo(&self, now: Timestamp) -> Result<Option<UndoEntry>> {
        let Some(entry) = self.redo.lock().ok().and_then(|mut r| r.pop()) else {
            return Ok(None);
        };
        let reste = self.pop_rest_of(&self.redo, entry.group);
        let premier = self.restore(entry, now, false)?;
        for autre in reste {
            self.restore(autre, now, false)?;
        }
        Ok(premier)
    }

    /// How many actions can be redone, a batch counting as one.
    pub fn redo_depth(&self) -> usize {
        self.redo.lock().map(|r| actions_in(&r)).unwrap_or(0)
    }

    /// Remet un fil dans l'état décrit, et empile l'inverse.
    ///
    /// `vers_redo` dit dans quelle pile va l'instantané de l'état courant : annuler
    /// alimente la pile de rétablissement, rétablir alimente celle d'annulation. C'est
    /// la seule différence entre les deux.
    fn restore(
        &self,
        entry: UndoEntry,
        now: Timestamp,
        vers_redo: bool,
    ) -> Result<Option<UndoEntry>> {
        // The thread may be gone by now: once the server has carried out the move, the
        // sync drops the old copies and the moved ones come back under a new thread.
        // Undo found nothing and did nothing, a minute after an archive. The thread is
        // found again by the moved messages' `Message-ID`; failing that, the server is
        // still told, and the next sync brings the mail back.
        let (entry, current) = match self.store.thread_row(entry.thread)? {
            Some(current) => (entry, current),
            None => {
                let retrouve = entry.moves.iter().find_map(|d| {
                    let (account, id) = d.account_and_message_id()?;
                    self.store.thread_of_message_id(account, id).ok().flatten()
                });
                match retrouve.and_then(|t| self.store.thread_row(t).ok().flatten()) {
                    Some(current) => (
                        UndoEntry {
                            thread: current.id,
                            // Its messages are new rows: their flags are not these.
                            flags: Vec::new(),
                            ..entry
                        },
                        current,
                    ),
                    None => {
                        for deplacement in entry.moves.iter().cloned() {
                            self.reverse_move(deplacement, now)?;
                        }
                        return Ok(None);
                    }
                }
            }
        };

        // L'état d'avant, capturé avant d'écrire : c'est ce que le geste inverse
        // rejouera. Le prendre après restaurerait ce qu'on vient d'installer.
        let mut inverse = self.snapshot(entry.thread, now, !entry.flags.is_empty())?;
        // Redone, or undone again, with the rest of its batch.
        inverse.group = entry.group;
        // The server is told too: the moves the action asked for are withdrawn or
        // reversed, and how to do them again goes with the inverse.
        for deplacement in entry.moves.iter().cloned() {
            if let Some(retour) = self.reverse_move(deplacement, now)? {
                inverse.moves.push(retour);
            }
        }
        self.push(if vers_redo { &self.redo } else { &self.undo }, inverse);

        // Undoing a deletion brings the thread back into its queue.
        self.store.set_thread_put_aside(entry.thread, None)?;

        if current.state != entry.state {
            self.store.set_thread_state(entry.thread, entry.state)?;
            self.bus.publish(Event::ThreadStateChanged {
                thread: entry.thread,
                from: current.state,
                to: entry.state,
                cause: TransitionCause::Manual,
            });
        }

        match entry.snoozed_until {
            Some(until) => {
                self.store.snooze_thread(
                    entry.thread,
                    Snooze {
                        until,
                        restore_to: entry.state,
                    },
                )?;
            }
            None => {
                self.store.clear_snooze(entry.thread)?;
            }
        }

        self.restore_flags(&entry, now)?;
        Ok(Some(entry))
    }

    /// How many actions can be undone, a batch counting as one.
    pub fn undo_depth(&self) -> usize {
        self.undo.lock().map(|u| actions_in(&u)).unwrap_or(0)
    }

    /// Puts the per-message flags back, and journals the reversal.
    fn restore_flags(&self, entry: &UndoEntry, now: Timestamp) -> Result<()> {
        // Per folder, per flag, per direction: read and starred both. The star was
        // put back here only, and came back at the next flag change; and a folder's
        // messages all took the first one's read state.
        let mut a_dire: BTreeMap<(AccountId, FolderId, u32, bool), Vec<u32>> = BTreeMap::new();

        for (id, flags) in &entry.flags {
            let Some(message) = self.store.message_by_id(*id)? else {
                continue;
            };
            if message.flags == *flags {
                continue;
            }
            self.store.set_message_flags(*id, *flags)?;

            for drapeau in [Flags::SEEN, Flags::FLAGGED] {
                let voulu = flags.contains(drapeau);
                if message.flags.contains(drapeau) != voulu {
                    a_dire
                        .entry((message.account, message.folder, drapeau.0, voulu))
                        .or_default()
                        .push(message.uid);
                }
            }
        }

        for ((account, folder, drapeau, poser), uids) in a_dire {
            self.journal_flags(account, folder, &uids, Flags(drapeau), poser, now)?;
        }
        Ok(())
    }

    // --- Time ---

    /// Wakes threads whose snooze has come due, restoring their state.
    ///
    /// The snooze **restores** a state, it does not decide one: a thread snoozed from
    /// "waiting" comes back to waiting. Snoozing sets aside, it does not requalify.
    pub fn wake_due_snoozes(&self, now: Timestamp) -> Result<usize> {
        let due = self.store.due_snoozes(now)?;
        let mut woken = 0;

        for (thread, restore_to) in due {
            self.store.clear_snooze(thread)?;
            self.bus.publish(Event::ThreadUnsnoozed { thread });
            self.apply_recorded(
                thread,
                TransitionCause::SnoozeExpired,
                Some(restore_to),
                now,
                false,
            )?;
            woken += 1;
        }

        Ok(woken)
    }

    /// Brings back threads that have been waiting for an answer too long.
    pub fn run_follow_ups(&self, now: Timestamp, limit: u32) -> Result<usize> {
        let settings = self.settings();
        if !settings.follow_up_enabled {
            return Ok(0);
        }

        let candidates =
            self.store
                .threads_needing_follow_up(now, settings.follow_up_days, limit)?;

        let mut followed = 0;
        for thread in candidates {
            if self
                .apply_recorded(thread, TransitionCause::FollowUpDue, None, now, false)?
                .changed()
            {
                followed += 1;
            }
        }
        Ok(followed)
    }

    /// Called when a reply has just been sent in a thread.
    pub fn on_reply_sent(&self, thread: ThreadId, now: Timestamp) -> Result<TransitionOutcome> {
        self.apply(thread, TransitionCause::ReplySent, None, now)
    }

    /// Called when a new message joins an existing thread (sync calls it for each
    /// message that arrives, not for copies a move or a label made).
    ///
    /// Nothing called it, so the setting did nothing: a reply to a thread marked done
    /// stayed in Done, and one to a deleted thread stayed hidden for good.
    pub fn on_message_received(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> Result<TransitionOutcome> {
        // Snoozed, and answered: the answer is what it waited for. It stayed hidden
        // until the snooze ran out.
        if self.settings().new_message_reopens {
            if let Some(row) = self.store.thread_row(thread)? {
                if row.snoozed_until.is_some() {
                    self.store.clear_snooze(thread)?;
                    self.bus.publish(Event::ThreadUnsnoozed { thread });
                }
            }
        }
        let issue =
            self.apply_recorded(thread, TransitionCause::MessageReceived, None, now, false)?;
        if issue.changed() && self.settings().new_message_reopens {
            // Out of the bin's shadow too: a deleted thread someone answers is back.
            self.store.set_thread_put_aside(thread, None)?;
        }
        Ok(issue)
    }

    /// Mail of this thread was put back in the inbox elsewhere (moved there on a
    /// phone, Gmail's Inbox label put back): it is to do again, no longer set aside or
    /// snoozed. It kept the Done its move had given it, and stayed out of sight. Not
    /// for undo: the user did not do it here.
    pub fn on_back_in_inbox(&self, thread: ThreadId, _now: Timestamp) -> Result<bool> {
        let Some(row) = self.store.thread_row(thread)? else {
            return Ok(false);
        };
        let ecarte = row.put_aside;
        if row.state == WorkflowState::Todo && !ecarte && row.snoozed_until.is_none() {
            return Ok(false);
        }
        self.store.set_thread_put_aside(thread, None)?;
        self.store.clear_snooze(thread)?;
        if row.state != WorkflowState::Todo {
            self.store.set_thread_state(thread, WorkflowState::Todo)?;
            self.bus.publish(Event::ThreadStateChanged {
                thread,
                from: row.state,
                to: WorkflowState::Todo,
                cause: TransitionCause::MessageReceived,
            });
        }
        Ok(true)
    }

    // --- Internals ---

    /// Captures what an action could change, before it changes it.
    ///
    /// `with_flags` decides whether the per-message flags are read. They are only
    /// needed by the two actions that touch them, and reading them costs a query plus
    /// one row per message in the thread — on a fifty-message thread, for a state
    /// change that cannot alter a single flag. Measured on a hundred thousand
    /// threads, capturing them unconditionally made triage four times slower.
    fn snapshot(&self, thread: ThreadId, at: Timestamp, with_flags: bool) -> Result<UndoEntry> {
        let row = self
            .store
            .thread_row(thread)?
            .ok_or_else(|| Error::store(format!("thread {thread} not found")))?;

        let flags = if with_flags {
            self.store
                .thread_messages(thread)?
                .into_iter()
                .map(|m| (m.id, m.flags))
                .collect()
        } else {
            Vec::new()
        };

        Ok(UndoEntry {
            thread,
            state: row.state,
            snoozed_until: row.snoozed_until,
            flags,
            moves: Vec::new(),
            at,
            group: 0,
        })
    }

    fn record_undo(&self, mut entry: UndoEntry) {
        let (lot, automatique) = LOT.with(|l| l.get());
        // Une nouvelle action ferme l'avenir qu'un rétablissement aurait rejoué.
        // Refaire après avoir fait autre chose appliquerait une action dans un monde
        // qui a changé sous elle. Not one the rules made on their own.
        if !automatique {
            if let Ok(mut redo) = self.redo.lock() {
                redo.clear();
            }
        }
        entry.group = lot;
        self.push(&self.undo, entry);
    }

    /// Pushes onto a stack that keeps `UNDO_DEPTH` actions, a batch counting as one.
    fn push(&self, pile: &Mutex<Vec<UndoEntry>>, entry: UndoEntry) {
        if let Ok(mut stack) = pile.lock() {
            stack.push(entry);
            while actions_in(&stack) > UNDO_DEPTH {
                let premier = stack.remove(0);
                if premier.group != 0 {
                    stack.retain(|e| e.group != premier.group);
                }
            }
        }
    }

    fn pop_undo(&self) -> Option<UndoEntry> {
        self.undo.lock().ok()?.pop()
    }

    /// The entries still on `pile` that belong to the same batch as `group`, newest
    /// first, taken off it.
    fn pop_rest_of(&self, pile: &Mutex<Vec<UndoEntry>>, group: u64) -> Vec<UndoEntry> {
        let mut reste = Vec::new();
        if group == 0 {
            return reste;
        }
        if let Ok(mut stack) = pile.lock() {
            while stack.last().is_some_and(|e| e.group == group) {
                reste.extend(stack.pop());
            }
        }
        reste
    }

    /// Records what the server will have to be told.
    fn journal_flags(
        &self,
        account: AccountId,
        folder: FolderId,
        uids: &[u32],
        flags: Flags,
        add: bool,
        now: Timestamp,
    ) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }

        let path = self
            .store
            .folders(account)?
            .into_iter()
            .find(|f| f.id == folder)
            .map(|f| f.path)
            .unwrap_or_default();

        // A message a journalled move has sent elsewhere keeps its old UID here until
        // the next sync: the change goes to where it went, by its Message-ID.
        let mut restants = Vec::new();
        let mut ailleurs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for &uid in uids {
            let parti = match self.store.moved_to(account, &path, uid)? {
                Some(cible) => self
                    .store
                    .message_by_uid(folder, uid)?
                    .and_then(|id| self.store.message_by_id(id).ok().flatten())
                    .and_then(|m| m.rfc_message_id)
                    .map(|mid| (cible, mid)),
                None => None,
            };
            match parti {
                Some((cible, mid)) => ailleurs.entry(cible).or_default().push(mid),
                None => restants.push(uid),
            }
        }
        for (cible, message_ids) in ailleurs {
            let charge = iris_store::OpPayload::SetFlagsByMessageId {
                folder: cible,
                message_ids,
                flags: flags.0,
                add,
            };
            self.store.enqueue_op(
                account,
                OpKind::SetFlags,
                &charge.to_json(),
                &charge.idempotency_key(account),
                now,
            )?;
        }
        if restants.is_empty() {
            return Ok(());
        }

        // Construite comme le déplacement, et pour la même raison : celle-ci se
        // trouvait correcte, l'autre non, et rien dans le code ne disait laquelle des
        // deux formes écrites à la main était la bonne.
        let charge = iris_store::OpPayload::SetFlags {
            folder: path,
            uids: restants,
            flags: flags.0,
            add,
        };

        // La clé rend l'opération idempotente : rejouer deux fois le même changement
        // de drapeaux ne doit pas l'enfiler deux fois.
        let key = charge.idempotency_key(account);

        self.store
            .enqueue_op(account, OpKind::SetFlags, &charge.to_json(), &key, now)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_kernel::EventKind;
    use iris_store::{FolderRole, NewAccount, NewMessage};

    struct Fixture {
        workflow: Workflow,
        store: Arc<Store>,
        bus: EventBus,
        account: AccountId,
        folder: FolderId,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        let account = store
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), Timestamp::EPOCH)
            .unwrap();
        let folder = store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        let bus = EventBus::new();
        let workflow = Workflow::new(
            Arc::clone(&store),
            bus.clone(),
            AutomationSettings::default(),
        );

        Fixture {
            workflow,
            store,
            bus,
            account,
            folder,
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        fn thread(&self) -> ThreadId {
            self.thread_in(self.folder)
        }

        /// Un fil dont le message se trouve dans le dossier indiqué.
        ///
        /// Une boîte déjà triée ailleurs a la plupart de ses messages hors de la
        /// boîte de réception, et ce qui s'y passe n'est pas ce qui se passe dans
        /// l'INBOX : c'est le cas que les tests doivent pouvoir écrire.
        fn thread_in(&self, folder: FolderId) -> ThreadId {
            let uid = self.uid.get();
            self.uid.set(uid + 1);
            self.store
                .insert_message(&NewMessage {
                    account: self.account,
                    folder,
                    uid,
                    rfc_message_id: Some(format!("m{uid}@x")),
                    in_reply_to: None,
                    references: vec![],
                    subject: format!("Subject {uid}"),
                    from_name: "Marie".into(),
                    from_addr: "marie@x.fr".into(),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(1000 * uid as i64),
                    received: Timestamp::from_millis(1000 * uid as i64),
                    size: 10,
                    flags: Flags::NONE,
                    preview: String::new(),
                })
                .unwrap()
                .thread
        }

        fn state(&self, t: ThreadId) -> WorkflowState {
            self.store.thread_row(t).unwrap().unwrap().state
        }

        fn row(&self, t: ThreadId) -> iris_store::ThreadRow {
            self.store.thread_row(t).unwrap().unwrap()
        }
    }

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_millis(secs * 1000)
    }

    #[test]
    fn a_manual_action_moves_the_thread() {
        let f = fixture();
        let thread = f.thread();

        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();
        assert_eq!(f.state(thread), WorkflowState::Done);
    }

    #[test]
    fn the_state_is_written_before_the_event_is_published() {
        // A subscriber re-reading the database must never find it contradicting the
        // event it just received.
        let f = fixture();
        let thread = f.thread();
        let mut subscriber = f.bus.subscribe_kind(EventKind::Workflow);

        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();

        let events = subscriber.drain();
        assert!(!events.is_empty(), "the change must be announced");
        assert_eq!(f.state(thread), WorkflowState::Done);
    }

    #[test]
    fn undo_puts_the_thread_back() {
        let f = fixture();
        let thread = f.thread();

        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();
        assert!(f.workflow.undo(t(2)).unwrap().is_some());
        assert_eq!(f.state(thread), WorkflowState::Todo);
    }

    #[test]
    fn undoing_twice_walks_further_back_instead_of_replaying() {
        let f = fixture();
        let thread = f.thread();

        f.workflow
            .set_state(thread, WorkflowState::Waiting, t(1))
            .unwrap();
        f.workflow
            .set_state(thread, WorkflowState::Done, t(2))
            .unwrap();

        f.workflow.undo(t(3)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Waiting);
        f.workflow.undo(t(4)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Todo);
    }

    #[test]
    fn redo_puts_back_what_undo_took_away() {
        let f = fixture();
        let thread = f.thread();

        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();
        assert_eq!(f.state(thread), WorkflowState::Done);

        f.workflow.undo(t(2)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Todo);

        f.workflow.redo(t(3)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Done);
    }

    #[test]
    fn redo_on_an_empty_stack_is_not_an_error() {
        let f = fixture();
        assert!(f.workflow.redo(t(1)).unwrap().is_none());
    }

    #[test]
    fn a_new_action_closes_the_future_redo_would_have_replayed() {
        // Rétablir après avoir fait autre chose rejouerait une action dans un monde
        // qui a changé sous elle, et le résultat n'est celui qu'attend personne.
        let f = fixture();
        let thread = f.thread();

        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();
        f.workflow.undo(t(2)).unwrap();
        assert_eq!(f.workflow.redo_depth(), 1);

        f.workflow
            .set_state(thread, WorkflowState::Waiting, t(3))
            .unwrap();
        assert_eq!(f.workflow.redo_depth(), 0);
        assert!(f.workflow.redo(t(4)).unwrap().is_none());
        assert_eq!(f.state(thread), WorkflowState::Waiting);
    }

    #[test]
    fn undo_and_redo_walk_the_same_history_both_ways() {
        let f = fixture();
        let thread = f.thread();

        f.workflow
            .set_state(thread, WorkflowState::Waiting, t(1))
            .unwrap();
        f.workflow
            .set_state(thread, WorkflowState::Done, t(2))
            .unwrap();

        f.workflow.undo(t(3)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Waiting);
        f.workflow.undo(t(4)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Todo);

        f.workflow.redo(t(5)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Waiting);
        f.workflow.redo(t(6)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Done);
    }

    #[test]
    fn undo_on_an_empty_stack_is_not_an_error() {
        let f = fixture();
        assert!(f.workflow.undo(t(1)).unwrap().is_none());
    }

    #[test]
    fn undoing_a_snooze_makes_the_thread_visible_again() {
        // Restoring only the state would leave the thread hidden, which is not what
        // the user asked to undo.
        let f = fixture();
        let thread = f.thread();

        f.workflow.snooze(thread, t(9000), t(1)).unwrap();
        assert!(f.row(thread).snoozed_until.is_some());

        f.workflow.undo(t(2)).unwrap();
        assert!(f.row(thread).snoozed_until.is_none());
    }

    #[test]
    fn undoing_a_read_marks_the_messages_unread_again() {
        let f = fixture();
        let thread = f.thread();

        f.workflow.set_read(thread, true, t(1)).unwrap();
        assert!(f.store.thread_messages(thread).unwrap()[0]
            .flags
            .contains(Flags::SEEN));

        f.workflow.undo(t(2)).unwrap();
        assert!(!f.store.thread_messages(thread).unwrap()[0]
            .flags
            .contains(Flags::SEEN));
    }

    #[test]
    fn marking_a_thread_read_queues_one_operation_per_folder() {
        // Fifty messages read at once must not produce fifty IMAP commands.
        let f = fixture();
        let thread = f.thread();
        let before = f.store.pending_op_count().unwrap();

        f.workflow.set_read(thread, true, t(1)).unwrap();
        assert_eq!(f.store.pending_op_count().unwrap(), before + 1);
    }

    #[test]
    fn marking_read_twice_changes_nothing_the_second_time() {
        let f = fixture();
        let thread = f.thread();

        assert!(f.workflow.set_read(thread, true, t(1)).unwrap());
        assert!(!f.workflow.set_read(thread, true, t(2)).unwrap());
    }

    #[test]
    fn starring_uses_the_most_recent_message() {
        let f = fixture();
        let thread = f.thread();

        assert!(f.workflow.set_flagged(thread, true, t(1)).unwrap());
        let messages = f.store.thread_messages(thread).unwrap();
        assert!(messages.last().unwrap().flags.contains(Flags::FLAGGED));
    }

    #[test]
    fn a_due_snooze_is_woken_and_the_state_restored() {
        let f = fixture();
        let thread = f.thread();
        f.workflow
            .set_state(thread, WorkflowState::Waiting, t(1))
            .unwrap();
        f.workflow.snooze(thread, t(5000), t(2)).unwrap();

        assert_eq!(f.workflow.wake_due_snoozes(t(4999)).unwrap(), 0);
        assert_eq!(f.workflow.wake_due_snoozes(t(5000)).unwrap(), 1);

        assert!(f.row(thread).snoozed_until.is_none());
        assert_eq!(
            f.state(thread),
            WorkflowState::Waiting,
            "snoozing sets aside, it does not requalify"
        );
    }

    #[test]
    fn a_thread_nobody_answered_comes_back() {
        let f = fixture();
        let thread = f.thread();
        f.workflow
            .set_state(thread, WorkflowState::Waiting, t(1))
            .unwrap();

        let later = t(1 + 4 * 86_400);
        assert_eq!(f.workflow.run_follow_ups(later, 50).unwrap(), 1);
        assert_eq!(f.state(thread), WorkflowState::Todo);
    }

    #[test]
    fn follow_ups_can_be_turned_off() {
        let f = fixture();
        f.workflow.set_settings(AutomationSettings {
            follow_up_enabled: false,
            ..Default::default()
        });
        let thread = f.thread();
        f.workflow
            .set_state(thread, WorkflowState::Waiting, t(1))
            .unwrap();

        assert_eq!(f.workflow.run_follow_ups(t(999_999), 50).unwrap(), 0);
    }

    #[test]
    fn follow_ups_are_spread_over_several_passes() {
        // After a long absence, an avalanche would make the queue unreadable.
        let f = fixture();
        for _ in 0..12 {
            let thread = f.thread();
            f.workflow
                .set_state(thread, WorkflowState::Waiting, t(1))
                .unwrap();
        }

        let later = t(1 + 30 * 86_400);
        assert_eq!(f.workflow.run_follow_ups(later, 5).unwrap(), 5);
        assert_eq!(f.workflow.run_follow_ups(later, 5).unwrap(), 5);
        assert_eq!(f.workflow.run_follow_ups(later, 5).unwrap(), 2);
    }

    #[test]
    fn sending_a_reply_moves_the_thread_to_waiting() {
        let f = fixture();
        let thread = f.thread();

        f.workflow.on_reply_sent(thread, t(1)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Waiting);
    }

    #[test]
    fn a_new_message_reopens_a_finished_thread() {
        let f = fixture();
        let thread = f.thread();
        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();

        f.workflow.on_message_received(thread, t(2)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Todo);
    }

    #[test]
    fn every_automatic_move_can_be_turned_off() {
        // The user asked for each automatism to be individually switchable.
        let f = fixture();
        f.workflow.set_settings(AutomationSettings::MANUAL_ONLY);
        let thread = f.thread();
        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();

        f.workflow.on_message_received(thread, t(2)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Done);
    }

    #[test]
    fn acting_on_a_missing_thread_is_an_error_not_a_panic() {
        let f = fixture();
        assert!(f
            .workflow
            .set_state(ThreadId(9999), WorkflowState::Done, t(1))
            .is_err());
    }

    #[test]
    fn undoing_a_thread_that_vanished_is_a_no_op() {
        let f = fixture();
        let thread = f.thread();
        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();

        f.store.delete_messages_by_uid(f.folder, &[1]).unwrap();
        assert!(f.workflow.undo(t(2)).unwrap().is_none());
    }

    #[test]
    fn the_undo_stack_is_bounded() {
        let f = fixture();
        let thread = f.thread();

        for i in 0..(UNDO_DEPTH + 20) {
            let target = if i % 2 == 0 {
                WorkflowState::Done
            } else {
                WorkflowState::Todo
            };
            f.workflow.set_state(thread, target, t(i as i64)).unwrap();
        }
        assert_eq!(f.workflow.undo_depth(), UNDO_DEPTH);
    }

    #[test]
    fn a_state_change_does_not_read_the_messages_it_cannot_touch() {
        // Capturing per-message flags for a state change made triage four times
        // slower on a large mailbox, for information the undo could never use.
        let f = fixture();
        let thread = f.thread();

        f.workflow
            .set_state(thread, WorkflowState::Done, t(1))
            .unwrap();

        // The undo entry is still complete for what the action can reverse.
        let entry = f.workflow.undo(t(2)).unwrap().unwrap();
        assert!(entry.flags.is_empty(), "no flags were at risk");
        assert_eq!(f.state(thread), WorkflowState::Todo);
    }

    #[test]
    fn a_flag_change_still_captures_them() {
        let f = fixture();
        let thread = f.thread();

        f.workflow.set_read(thread, true, t(1)).unwrap();
        let entry = f.workflow.undo(t(2)).unwrap().unwrap();
        assert!(!entry.flags.is_empty(), "the flags must be recoverable");
    }

    #[test]
    fn archiving_moves_the_mail_and_clears_the_queue() {
        // Both halves matter: without the move, the next sync puts it straight back.
        let f = fixture();
        f.store
            .upsert_folder(f.account, "Archive", FolderRole::Archive)
            .unwrap();
        let thread = f.thread();
        let before = f.store.pending_op_count().unwrap();

        assert!(f.workflow.archive(thread, t(1)).unwrap());
        assert_eq!(f.state(thread), WorkflowState::Done);
        assert_eq!(
            f.store.pending_op_count().unwrap(),
            before + 1,
            "the server has to be told"
        );
    }

    #[test]
    fn deleting_moves_to_the_bin_rather_than_erasing() {
        // A client that destroys mail on a keystroke is one nobody can use quickly.
        let f = fixture();
        f.store
            .upsert_folder(f.account, "Trash", FolderRole::Trash)
            .unwrap();
        let thread = f.thread();

        assert!(f.workflow.delete(thread, t(1)).unwrap());
        assert_eq!(
            f.store.thread_messages(thread).unwrap().len(),
            1,
            "the message still exists locally until the server confirms"
        );
    }

    #[test]
    fn deleting_a_thread_already_in_the_bin_still_clears_the_queue() {
        // The case that made the button look broken. A thread whose messages have
        // moved to the bin behind our back — triaged in webmail, on a phone — has
        // nothing left to send the server, and the action used to report "nothing
        // changed" on that basis. It still has to leave the queue: that is what was
        // asked, and the mail is already where it was asked to go.
        let f = fixture();
        let bin = f
            .store
            .upsert_folder(f.account, "Trash", FolderRole::Trash)
            .unwrap();
        let thread = f.thread_in(bin);
        f.store
            .set_thread_state(thread, WorkflowState::Todo)
            .unwrap();
        let before = f.store.pending_op_count().unwrap();

        assert!(
            f.workflow.delete(thread, t(1)).unwrap(),
            "the thread has to leave the queue"
        );
        assert_eq!(f.state(thread), WorkflowState::Done);
        assert_eq!(
            f.store.pending_op_count().unwrap(),
            before,
            "the server has nothing to do: the message is already there"
        );
    }

    #[test]
    fn deleting_it_a_second_time_changes_nothing() {
        // Once it is both in the bin and out of the queue there is nothing left to
        // do, and saying otherwise would push an undo entry for an action that did
        // not happen.
        let f = fixture();
        let bin = f
            .store
            .upsert_folder(f.account, "Trash", FolderRole::Trash)
            .unwrap();
        let thread = f.thread_in(bin);
        f.store
            .set_thread_state(thread, WorkflowState::Todo)
            .unwrap();

        assert!(f.workflow.delete(thread, t(1)).unwrap());
        assert!(!f.workflow.delete(thread, t(2)).unwrap());
    }

    #[test]
    fn deleting_a_done_thread_takes_it_out_of_the_done_tab() {
        // The fault users hit: in Done, Delete did nothing visible. The state was
        // already Done and the message stays in its folder until the server replays
        // the move, so the thread stayed on screen.
        let f = fixture();
        f.store
            .upsert_folder(f.account, "Trash", FolderRole::Trash)
            .unwrap();
        let thread = f.thread();
        f.store
            .set_thread_state(thread, WorkflowState::Done)
            .unwrap();
        let fait = |f: &Fixture| {
            f.store
                .list_threads(&iris_store::ListQuery {
                    state: WorkflowState::Done,
                    accounts: Vec::new(),
                    hide_snoozed_until: None,
                    limit: 50,
                    after: None,
                    scope: iris_store::Scope::Queue,
                    filters: iris_store::Filters::default(),
                    sort: iris_store::Sort::Date,
                    offset: 0,
                })
                .unwrap()
                .iter()
                .any(|r| r.id == thread)
        };
        assert!(fait(&f));

        assert!(f.workflow.delete(thread, t(1)).unwrap(), "it is a change");
        assert!(!fait(&f), "gone from Done at once");

        f.workflow.undo(t(2)).unwrap();
        assert!(fait(&f), "undo brings it back");
    }

    #[test]
    fn mail_that_arrives_in_the_bin_keeps_its_own_state() {
        // Une version précédente le créait « terminé » pour le tenir hors de la file.
        // C'était mettre un endroit dans un état : « Terminé » veut dire « je m'en
        // suis occupé », et la corbeille y noyait tout ce qui l'était vraiment. C'est
        // la requête de liste qui écarte maintenant, et le test qui compte est du côté
        // du magasin, là où la requête vit.
        let f = fixture();
        let bin = f
            .store
            .upsert_folder(f.account, "Trash", FolderRole::Trash)
            .unwrap();

        assert_eq!(f.state(f.thread_in(bin)), WorkflowState::Todo);
        assert_eq!(f.state(f.thread()), WorkflowState::Todo);
    }

    #[test]
    fn archiving_without_an_archive_folder_makes_one() {
        // OVH and Gandi have none: archive failed there for good.
        let f = fixture();
        let thread = f.thread();

        assert!(f.workflow.archive(thread, t(1)).unwrap());
        assert_eq!(f.state(thread), WorkflowState::Done);
        let archives = f
            .store
            .folders(f.account)
            .unwrap()
            .into_iter()
            .find(|d| d.role == FolderRole::Archive)
            .expect("an archive folder");
        let demandes: Vec<String> = f
            .store
            .pending_ops(t(1_000_000), 10)
            .unwrap()
            .into_iter()
            .map(|o| o.payload)
            .collect();
        assert!(demandes[0].contains("create_folder"), "{demandes:?}");
        assert!(
            demandes[1].contains(&format!("\"target\":\"{}\"", archives.path)),
            "{demandes:?}"
        );
    }

    #[test]
    fn archiving_an_empty_thread_does_nothing() {
        let f = fixture();
        assert!(!f.workflow.archive(ThreadId(9999), t(1)).unwrap());
    }

    #[test]
    fn archiving_twice_queues_one_operation() {
        // The journal key is the same both times, so the server is told once.
        let f = fixture();
        f.store
            .upsert_folder(f.account, "Archive", FolderRole::Archive)
            .unwrap();
        let thread = f.thread();

        f.workflow.archive(thread, t(1)).unwrap();
        let after_first = f.store.pending_op_count().unwrap();
        f.workflow.archive(thread, t(2)).unwrap();

        assert_eq!(f.store.pending_op_count().unwrap(), after_first);
    }

    #[test]
    fn archiving_is_undoable() {
        let f = fixture();
        f.store
            .upsert_folder(f.account, "Archive", FolderRole::Archive)
            .unwrap();
        let thread = f.thread();
        f.workflow
            .set_state(thread, WorkflowState::Waiting, t(1))
            .unwrap();

        f.workflow.archive(thread, t(2)).unwrap();
        f.workflow.undo(t(3)).unwrap();

        assert_eq!(
            f.state(thread),
            WorkflowState::Waiting,
            "undo returns it where it was, not to the default"
        );
    }

    #[test]
    fn undoing_an_archive_takes_the_move_back_on_the_server_too() {
        // Undo changed the state here only: the server kept the thread archived, and
        // it vanished again at the next sync.
        let f = fixture();
        f.store
            .upsert_folder(f.account, "Archive", FolderRole::Archive)
            .unwrap();
        let thread = f.thread();
        let avant = f.store.pending_op_count().unwrap();

        f.workflow.archive(thread, t(1)).unwrap();
        assert_eq!(f.store.pending_op_count().unwrap(), avant + 1);
        f.workflow.undo(t(2)).unwrap();
        assert_eq!(
            f.store.pending_op_count().unwrap(),
            avant,
            "not sent yet: withdrawn"
        );

        // Sent already: the move is reversed, the message found by its Message-ID.
        f.workflow.archive(thread, t(3)).unwrap();
        for op in f.store.pending_ops(t(4), 10).unwrap() {
            f.store.complete_op(op.id).unwrap();
        }
        f.workflow.undo(t(5)).unwrap();
        let retour = f.store.pending_ops(t(6), 10).unwrap();
        assert_eq!(retour.len(), 1);
        assert!(
            matches!(
                iris_store::OpPayload::parse(&retour[0].payload).unwrap(),
                iris_store::OpPayload::MoveByMessageId { ref target, .. } if target == "INBOX"
            ),
            "{}",
            retour[0].payload
        );
    }

    #[test]
    fn archiving_leaves_one_s_sent_replies_in_sent() {
        let f = fixture();
        f.store
            .upsert_folder(f.account, "Archive", FolderRole::Archive)
            .unwrap();
        let envoyes = f
            .store
            .upsert_folder(f.account, "Sent", FolderRole::Sent)
            .unwrap();
        let thread = f.thread();
        f.store
            .insert_message(&NewMessage {
                account: f.account,
                folder: envoyes,
                uid: 500,
                rfc_message_id: Some("reponse@x".into()),
                in_reply_to: Some(
                    f.store.thread_messages(thread).unwrap()[0]
                        .rfc_message_id
                        .clone()
                        .unwrap(),
                ),
                references: vec![],
                subject: "Re".into(),
                from_name: String::new(),
                from_addr: "a@x.fr".into(),
                recipients_json: "[]".into(),
                date: t(1),
                received: t(1),
                size: 1,
                flags: Flags::SEEN,
                preview: String::new(),
            })
            .unwrap();
        let avant = f.store.pending_op_count().unwrap();

        f.workflow.archive(thread, t(2)).unwrap();
        assert_eq!(
            f.store.pending_op_count().unwrap(),
            avant + 1,
            "the inbox copy only"
        );
    }

    #[test]
    fn what_the_workflow_writes_the_replay_can_read() {
        // Ce test remplace celui d'un échappeur JSON écrit à la main. Il posait la
        // mauvaise question : l'échappement était correct, et le champ de destination
        // s'appelait `to` là où le rejeu lisait `target`. Chaque déplacement était
        // abandonné en silence. La question qui compte n'est pas « la chaîne est-elle
        // bien échappée » mais « l'autre côté sait-il la lire ».
        //
        // Un dossier peut légitimement s'appeler `Clients "VIP"`.
        let f = fixture();
        let bin = f
            .store
            .upsert_folder(f.account, r#"Clients "VIP""#, FolderRole::Trash)
            .unwrap();
        let _ = bin;

        let thread = f.thread();
        f.workflow.delete(thread, t(1)).unwrap();
        f.workflow.set_read(thread, true, t(2)).unwrap();

        let en_attente = f.store.pending_ops(t(3), 20).unwrap();
        assert!(
            !en_attente.is_empty(),
            "il doit y avoir quelque chose à rejouer"
        );

        for op in en_attente {
            iris_store::OpPayload::parse(&op.payload).unwrap_or_else(|e| {
                panic!("le rejeu ne saurait pas lire ce que nous écrivons : {e}")
            });
        }
    }
}
