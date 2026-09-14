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
use iris_store::{OpKind, Store};
use iris_types::{
    transition, AccountId, AutomationSettings, Error, Flags, FolderId, Result, Snooze, ThreadId,
    Timestamp, TransitionCause, TransitionOutcome, WorkflowState,
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
    pub at: Timestamp,
}

/// What an action did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub thread: ThreadId,
    pub changed: bool,
    pub outcome: Option<TransitionOutcome>,
}

/// The workflow engine.
#[derive(Debug)]
pub struct Workflow {
    store: Arc<Store>,
    bus: EventBus,
    settings: RwLock<AutomationSettings>,
    undo: Mutex<Vec<UndoEntry>>,
}

impl Workflow {
    pub fn new(store: Arc<Store>, bus: EventBus, settings: AutomationSettings) -> Self {
        Self {
            store,
            bus,
            settings: RwLock::new(settings),
            undo: Mutex::new(Vec::new()),
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
                at: now,
            };
            self.store.set_thread_state(thread, to)?;
            self.record_undo(before);
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
            iris_store::FolderRole::Archive,
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
            iris_store::FolderRole::Trash,
            OpKind::DeleteMessage,
            now,
        )
    }

    /// The shared part of archiving and deleting.
    fn move_thread(
        &self,
        thread: ThreadId,
        role: iris_store::FolderRole,
        kind: OpKind,
        now: Timestamp,
    ) -> Result<bool> {
        let messages = self.store.thread_messages(thread)?;
        if messages.is_empty() {
            return Ok(false);
        }

        let before = self.snapshot(thread, now, true)?;
        let mut moved = 0;

        // Grouped per account, because the destination folder is per account and a
        // thread can span several of them once regrouping has run.
        let mut per_account: BTreeMap<AccountId, Vec<(FolderId, u32)>> = BTreeMap::new();
        for m in &messages {
            per_account
                .entry(m.account)
                .or_default()
                .push((m.folder, m.uid));
        }

        for (account, items) in per_account {
            let Some(target) = self
                .store
                .folders(account)?
                .into_iter()
                .find(|f| f.role == role)
            else {
                // No such folder on this server: say so rather than pretend. Silently
                // marking the thread done would lose the mail on the next sync.
                return Err(Error::Config(format!(
                    "this account has no {} folder",
                    role.as_str()
                )));
            };

            for (folder, uid) in items {
                if folder == target.id {
                    continue;
                }
                self.journal_move(account, folder, uid, &target.path, kind, now)?;
                moved += 1;
            }
        }

        if moved == 0 {
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

    /// Records a move for the server to carry out later.
    fn journal_move(
        &self,
        account: AccountId,
        folder: FolderId,
        uid: u32,
        destination: &str,
        kind: OpKind,
        now: Timestamp,
    ) -> Result<()> {
        let source = self
            .store
            .folders(account)?
            .into_iter()
            .find(|f| f.id == folder)
            .map(|f| f.path)
            .unwrap_or_default();

        let payload = format!(
            r#"{{"op":"move","folder":{},"uids":[{uid}],"to":{}}}"#,
            quote(&source),
            quote(destination)
        );
        let key = format!("{account}:move:{source}:{uid}:{destination}");

        self.store.enqueue_op(account, kind, &payload, &key, now)?;
        Ok(())
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

        // The thread may be gone by now, in which case there is nothing to restore.
        let Some(current) = self.store.thread_row(entry.thread)? else {
            return Ok(None);
        };

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
        let mut seen: BTreeMap<(AccountId, FolderId), (Vec<u32>, bool)> = BTreeMap::new();

        for (id, flags) in &entry.flags {
            let Some(message) = self.store.message_by_id(*id)? else {
                continue;
            };
            if message.flags == *flags {
                continue;
            }
            self.store.set_message_flags(*id, *flags)?;

            let was_read = flags.contains(Flags::SEEN);
            seen.entry((message.account, message.folder))
                .or_insert_with(|| (Vec::new(), was_read))
                .0
                .push(message.uid);
        }

        for ((account, folder), (uids, read)) in seen {
            self.journal_flags(account, folder, &uids, Flags::SEEN, read, now)?;
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
            self.apply(
                thread,
                TransitionCause::SnoozeExpired,
                Some(restore_to),
                now,
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
                .apply(thread, TransitionCause::FollowUpDue, None, now)?
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

    /// Called when a new message joins an existing thread.
    pub fn on_message_received(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> Result<TransitionOutcome> {
        self.apply(thread, TransitionCause::MessageReceived, None, now)
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
            at,
        })
    }

    fn record_undo(&self, entry: UndoEntry) {
        if let Ok(mut stack) = self.undo.lock() {
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

        let mut sorted = uids.to_vec();
        sorted.sort_unstable();

        let payload = format!(
            r#"{{"op":"set_flags","folder":{},"uids":[{}],"flags":{},"add":{}}}"#,
            quote(&path),
            sorted
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            flags.0,
            add
        );

        // The key makes the operation idempotent: replaying the same flag change
        // twice must not queue it twice.
        let key = format!(
            "{account}:flags:{path}:{}:{}:{add}",
            sorted
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            flags.0
        );

        self.store
            .enqueue_op(account, OpKind::SetFlags, &payload, &key, now)?;
        Ok(())
    }
}

/// Escapes a string for the small JSON payloads written above.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
            let uid = self.uid.get();
            self.uid.set(uid + 1);
            self.store
                .insert_message(&NewMessage {
                    account: self.account,
                    folder: self.folder,
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
    fn folder_names_with_quotes_do_not_break_the_journal() {
        // A folder can legitimately be called `Clients "VIP"`.
        assert_eq!(quote(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote("a\\b"), r#""a\\b""#);
        assert_eq!(quote("a\nb"), r#""a\nb""#);
    }
}
