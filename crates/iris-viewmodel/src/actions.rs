//! What the user can do to a thread.
//!
//! This module owns the **vocabulary** — which gestures exist, what they are called,
//! which key triggers them — and nothing else. The behaviour lives in
//! `iris-workflow`, which is the single place where a thread changes.
//!
//! The split matters. An action has two lives: one in the interface, where it needs a
//! label, a key and a place in the palette, and one in the domain, where it needs a
//! transaction, a journal entry and an undo record. Keeping both in one type meant
//! the domain half existed twice — once here and once in `iris-workflow` — with two
//! undo stacks that could disagree about what "undo" meant.
//!
//! Invariant 3 still holds through the engine: **nothing waits for the network**.
//! Every action writes locally, journals what must be sent, and returns.

use iris_types::{Result, ThreadId, Timestamp};
use iris_workflow::{UndoEntry, Workflow};
use std::sync::Arc;

/// A gesture the user can make on a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Mark as done.
    Done,
    /// Put back in the work queue.
    Todo,
    /// Move to waiting.
    Waiting,
    /// Snooze for N hours.
    SnoozeHours(u32),
    /// Cancel a snooze.
    Unsnooze,
    MarkRead,
    MarkUnread,
    ToggleFlag,
    /// Move out of the inbox and off the queue.
    Archive,
    /// Move to the bin. Never an erasure.
    Delete,
}

impl Action {
    /// The key that triggers it, as shown in help and in the palette.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Self::Done => Some("e"),
            Self::Todo => Some("u"),
            Self::Waiting => Some("w"),
            Self::SnoozeHours(_) => Some("s"),
            Self::MarkRead => Some("r"),
            Self::MarkUnread => Some("Shift+r"),
            Self::ToggleFlag => Some("f"),
            Self::Archive => Some("a"),
            // Shift, because deleting is the one action here that reaches for a
            // different folder on the server. A single letter is too easy to hit.
            Self::Delete => Some("Shift+3"),
            Self::Unsnooze => None,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Done => "Mark as done".into(),
            Self::Todo => "Move back to inbox".into(),
            Self::Waiting => "Mark as waiting".into(),
            Self::SnoozeHours(h) if h % 24 == 0 => format!("Snooze for {} days", h / 24),
            Self::SnoozeHours(h) => format!("Snooze for {h} hours"),
            Self::Unsnooze => "Cancel snooze".into(),
            Self::MarkRead => "Mark as read".into(),
            Self::MarkUnread => "Mark as unread".into(),
            Self::ToggleFlag => "Star".into(),
            Self::Archive => "Archive".into(),
            Self::Delete => "Delete".into(),
        }
    }

    /// Does this action need a thread to act on?
    ///
    /// All of them do today. The method exists so the palette can ask rather than
    /// assume, because the first action that does not will be easy to miss.
    pub fn needs_thread(self) -> bool {
        true
    }
}

/// What an action did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionOutcome {
    pub thread: ThreadId,
    pub action: Action,
    /// Whether anything actually changed. An action that changes nothing must not
    /// redraw the interface, and must not fill the undo stack with no-ops.
    pub changed: bool,
}

/// Applies user actions by delegating to the workflow engine.
#[derive(Debug)]
pub struct Actions {
    workflow: Arc<Workflow>,
}

impl Actions {
    pub fn new(workflow: Arc<Workflow>) -> Self {
        Self { workflow }
    }

    pub fn workflow(&self) -> &Arc<Workflow> {
        &self.workflow
    }

    pub fn settings(&self) -> iris_types::AutomationSettings {
        self.workflow.settings()
    }

    pub fn set_settings(&self, settings: iris_types::AutomationSettings) {
        self.workflow.set_settings(settings);
    }

    pub fn undo_depth(&self) -> usize {
        self.workflow.undo_depth()
    }

    /// Applies an action to a thread.
    pub fn apply(&self, thread: ThreadId, action: Action, now: Timestamp) -> Result<ActionOutcome> {
        let changed = match action {
            Action::Done => self
                .workflow
                .set_state(thread, iris_types::WorkflowState::Done, now)?
                .changed(),
            Action::Todo => self
                .workflow
                .set_state(thread, iris_types::WorkflowState::Todo, now)?
                .changed(),
            Action::Waiting => self
                .workflow
                .set_state(thread, iris_types::WorkflowState::Waiting, now)?
                .changed(),
            Action::SnoozeHours(hours) => {
                let until = Timestamp::from_millis(now.millis() + hours as i64 * 3_600_000);
                self.workflow.snooze(thread, until, now)?
            }
            Action::Unsnooze => self.workflow.unsnooze(thread, now)?,
            Action::MarkRead => self.workflow.set_read(thread, true, now)?,
            Action::MarkUnread => self.workflow.set_read(thread, false, now)?,
            Action::Archive => self.workflow.archive(thread, now)?,
            Action::Delete => self.workflow.delete(thread, now)?,
            Action::ToggleFlag => {
                let starred = self
                    .workflow
                    .store()
                    .thread_row(thread)?
                    .map(|r| r.flags_union.contains(iris_types::Flags::FLAGGED))
                    .unwrap_or(false);
                self.workflow.set_flagged(thread, !starred, now)?
            }
        };

        Ok(ActionOutcome {
            thread,
            action,
            changed,
        })
    }

    /// Undoes the last action.
    pub fn undo(&self, now: Timestamp) -> Result<Option<UndoEntry>> {
        self.workflow.undo(now)
    }

    /// Applies an action to several threads, for batch triage.
    pub fn apply_many(
        &self,
        threads: &[ThreadId],
        action: Action,
        now: Timestamp,
    ) -> Result<usize> {
        let mut changes = 0;
        for thread in threads {
            // A thread that vanished must not stop the others being processed.
            match self.apply(*thread, action, now) {
                Ok(outcome) if outcome.changed => changes += 1,
                Ok(_) => {}
                Err(e) => tracing::warn!(thread = %thread, error = %e, "action skipped"),
            }
        }
        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_kernel::EventBus;
    use iris_store::{FolderRole, NewAccount, NewMessage, Store};
    use iris_types::{AccountId, AutomationSettings, Flags, FolderId, WorkflowState};

    struct Fixture {
        actions: Actions,
        store: Arc<Store>,
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

        let workflow = Arc::new(Workflow::new(
            Arc::clone(&store),
            EventBus::new(),
            AutomationSettings::default(),
        ));

        Fixture {
            actions: Actions::new(workflow),
            store,
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
    }

    fn now() -> Timestamp {
        Timestamp::from_millis(1_000_000)
    }

    #[test]
    fn each_action_has_a_label() {
        // An action without a label is an action nobody can find in the palette.
        for action in [
            Action::Done,
            Action::Todo,
            Action::Waiting,
            Action::SnoozeHours(3),
            Action::Unsnooze,
            Action::MarkRead,
            Action::MarkUnread,
            Action::ToggleFlag,
            Action::Archive,
            Action::Delete,
        ] {
            assert!(!action.label().is_empty(), "{action:?} has no label");
        }
    }

    #[test]
    fn a_snooze_of_whole_days_is_worded_in_days() {
        // "Snooze for 48 hours" is arithmetic the reader should not have to do.
        assert_eq!(Action::SnoozeHours(48).label(), "Snooze for 2 days");
        assert_eq!(Action::SnoozeHours(3).label(), "Snooze for 3 hours");
    }

    #[test]
    fn marking_done_moves_the_thread() {
        let f = fixture();
        let thread = f.thread();

        let outcome = f.actions.apply(thread, Action::Done, now()).unwrap();
        assert!(outcome.changed);
        assert_eq!(f.state(thread), WorkflowState::Done);
    }

    #[test]
    fn an_action_that_changes_nothing_says_so() {
        // Redrawing for nothing is noise, and so is an undo entry for nothing.
        let f = fixture();
        let thread = f.thread();

        let outcome = f.actions.apply(thread, Action::Todo, now()).unwrap();
        assert!(!outcome.changed);
        assert_eq!(f.actions.undo_depth(), 0);
    }

    #[test]
    fn undo_reverses_the_last_action() {
        let f = fixture();
        let thread = f.thread();

        f.actions.apply(thread, Action::Done, now()).unwrap();
        assert!(f.actions.undo(now()).unwrap().is_some());
        assert_eq!(f.state(thread), WorkflowState::Todo);
    }

    #[test]
    fn snoozing_hides_the_thread_and_undo_brings_it_back() {
        let f = fixture();
        let thread = f.thread();

        f.actions
            .apply(thread, Action::SnoozeHours(24), now())
            .unwrap();
        assert!(f
            .store
            .thread_row(thread)
            .unwrap()
            .unwrap()
            .snoozed_until
            .is_some());

        f.actions.undo(now()).unwrap();
        assert!(f
            .store
            .thread_row(thread)
            .unwrap()
            .unwrap()
            .snoozed_until
            .is_none());
    }

    #[test]
    fn starring_toggles() {
        let f = fixture();
        let thread = f.thread();

        f.actions.apply(thread, Action::ToggleFlag, now()).unwrap();
        assert!(f
            .store
            .thread_row(thread)
            .unwrap()
            .unwrap()
            .flags_union
            .contains(Flags::FLAGGED));

        f.actions.apply(thread, Action::ToggleFlag, now()).unwrap();
        assert!(!f
            .store
            .thread_row(thread)
            .unwrap()
            .unwrap()
            .flags_union
            .contains(Flags::FLAGGED));
    }

    #[test]
    fn reading_marks_every_message_in_the_thread() {
        let f = fixture();
        let thread = f.thread();

        assert!(
            f.actions
                .apply(thread, Action::MarkRead, now())
                .unwrap()
                .changed
        );
        assert!(f
            .store
            .thread_messages(thread)
            .unwrap()
            .iter()
            .all(|m| m.flags.contains(Flags::SEEN)));
    }

    #[test]
    fn batch_triage_counts_what_it_changed() {
        let f = fixture();
        let threads: Vec<_> = (0..5).map(|_| f.thread()).collect();

        let changed = f.actions.apply_many(&threads, Action::Done, now()).unwrap();
        assert_eq!(changed, 5);
    }

    #[test]
    fn a_missing_thread_does_not_stop_the_batch() {
        let f = fixture();
        let mut threads: Vec<_> = (0..3).map(|_| f.thread()).collect();
        threads.insert(1, ThreadId(9999));

        let changed = f.actions.apply_many(&threads, Action::Done, now()).unwrap();
        assert_eq!(changed, 3, "the three real threads are still processed");
    }

    #[test]
    fn settings_reach_the_engine() {
        // The settings screen writes here; the engine must be the one that reads.
        let f = fixture();
        f.actions.set_settings(AutomationSettings::MANUAL_ONLY);
        assert_eq!(f.actions.settings(), AutomationSettings::MANUAL_ONLY);
        assert_eq!(
            f.actions.workflow().settings(),
            AutomationSettings::MANUAL_ONLY
        );
    }

    #[test]
    fn there_is_a_single_undo_stack() {
        // Two stacks would disagree about what "undo" means.
        let f = fixture();
        let thread = f.thread();

        f.actions.apply(thread, Action::Done, now()).unwrap();
        assert_eq!(f.actions.undo_depth(), 1);
        assert_eq!(f.actions.workflow().undo_depth(), 1);

        f.actions.workflow().undo(now()).unwrap();
        assert_eq!(f.actions.undo_depth(), 0);
    }
}
