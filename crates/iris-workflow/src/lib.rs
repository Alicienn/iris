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
/// - Archive takes what is in the inbox (and, outside Gmail, in folders of one's
///   own); on Gmail that is exactly "remove the Inbox label".
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
        Destination::Role(R::Archive) if gmail => from == R::Inbox,
        Destination::Role(R::Archive) => matches!(from, R::Inbox | R::Other),
        Destination::Role(R::Trash) => from != R::Trash,
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
}

impl Workflow {
    pub fn new(store: Arc<Store>, bus: EventBus, settings: AutomationSettings) -> Self {
        Self {
            store,
            bus,
            settings: RwLock::new(settings),
            undo: Mutex::new(Vec::new()),
            redo: Mutex::new(Vec::new()),
        }
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
            };
            self.store.set_thread_state(thread, to)?;
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

        let updated = last.flags.set(Flags::FLAGGED, flagged);
        if updated == last.flags {
            return Ok(false);
        }

        self.store.set_message_flags(last.id, updated)?;
        self.journal_flags(
            last.account,
            last.folder,
            &[last.uid],
            Flags::FLAGGED,
            flagged,
            now,
        )?;

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
        for (account, items) in per_account {
            let dossiers = self.store.folders(account)?;
            let trouve = match destination {
                Destination::Role(role) => dossiers.iter().find(|f| f.role == role).cloned(),
                // Un compte sans ce dossier est ignoré, pas fatal : le fil est peut-être
                // à cheval sur deux boîtes dont une seule a « Devis ».
                Destination::Path(chemin) => match dossiers.iter().find(|f| f.path == chemin) {
                    Some(f) => Some(f.clone()),
                    None => continue,
                },
            };

            let Some(target) = trouve else {
                // No such folder on this server: say so rather than pretend. Silently
                // marking the thread done would lose the mail on the next sync.
                return Err(Error::Config(format!(
                    "this account has no {} folder",
                    destination.describe()
                )));
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

        if moved == 0 && before.state == WorkflowState::Done && !mis_de_cote {
            return Ok(false);
        }

        // Locally the thread leaves the queue at once; the server hears about it when
        // the journal replays. That is invariant 3: nothing waits for the network.
        let previous = before.state;
        self.store.set_thread_state(thread, WorkflowState::Done)?;
        self.record_undo(before);
        self.bus.publish(Event::ThreadStateChanged {
            thread,
            from: previous,
            to: WorkflowState::Done,
            cause: TransitionCause::Manual,
        });
        Ok(true)
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
        self.restore(entry, now, true)
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
        self.restore(entry, now, false)
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.lock().map(|r| r.len()).unwrap_or(0)
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
        // The thread may be gone by now, in which case there is nothing to restore.
        let Some(current) = self.store.thread_row(entry.thread)? else {
            return Ok(None);
        };

        // L'état d'avant, capturé avant d'écrire : c'est ce que le geste inverse
        // rejouera. Le prendre après restaurerait ce qu'on vient d'installer.
        let mut inverse = self.snapshot(entry.thread, now, !entry.flags.is_empty())?;
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

    pub fn undo_depth(&self) -> usize {
        self.undo.lock().map(|u| u.len()).unwrap_or(0)
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
        let issue =
            self.apply_recorded(thread, TransitionCause::MessageReceived, None, now, false)?;
        if issue.changed() && self.settings().new_message_reopens {
            // Out of the bin's shadow too: a deleted thread someone answers is back.
            self.store.set_thread_put_aside(thread, None)?;
        }
        Ok(issue)
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
        })
    }

    fn record_undo(&self, entry: UndoEntry) {
        // Une nouvelle action ferme l'avenir qu'un rétablissement aurait rejoué.
        // Refaire après avoir fait autre chose appliquerait une action dans un monde
        // qui a changé sous elle.
        if let Ok(mut redo) = self.redo.lock() {
            redo.clear();
        }
        self.push(&self.undo, entry);
    }

    fn push(&self, pile: &Mutex<Vec<UndoEntry>>, entry: UndoEntry) {
        if let Ok(mut stack) = pile.lock() {
            if stack.len() == UNDO_DEPTH {
                stack.remove(0);
            }
            stack.push(entry);
        }
    }

    fn pop_undo(&self) -> Option<UndoEntry> {
        self.undo.lock().ok()?.pop()
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

        // Construite comme le déplacement, et pour la même raison : celle-ci se
        // trouvait correcte, l'autre non, et rien dans le code ne disait laquelle des
        // deux formes écrites à la main était la bonne.
        let charge = iris_store::OpPayload::SetFlags {
            folder: path,
            uids: uids.to_vec(),
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
    fn archiving_without_an_archive_folder_says_so() {
        // Marking it done anyway would lose the mail at the next sync.
        let f = fixture();
        let thread = f.thread();

        let error = f.workflow.archive(thread, t(1)).unwrap_err().to_string();
        assert!(error.contains("archive"), "got: {error}");
        assert_eq!(f.state(thread), WorkflowState::Todo, "nothing moved");
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
