//! The work that time does.
//!
//! Two automatisms have no trigger: they depend only on what the clock says. A snooze
//! comes due; a thread has been waiting for an answer too long. Without a loop that
//! wakes them, they are two promises the application never keeps — and the user only
//! notices on the day they were counting on one.
//!
//! The rules themselves live in `iris-workflow`, which is the single state machine.
//! This module only decides **when** to ask, and reports what happened.
//! Reimplementing the wake-up here would give the application two versions of the
//! same behaviour, free to drift apart until nobody knows which one the user saw.
//!
//! The pass is **idempotent**: running it twice in the same second produces nothing
//! more. That is what makes it safe to call on every synchronisation tick without
//! thinking about it.

use crate::engine::SyncEngine;
use iris_types::{Result, Timestamp};

/// What a pass produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaintenanceReport {
    /// Threads whose snooze came due.
    pub woken: usize,
    /// Threads brought back for lack of an answer.
    pub followed_up: usize,
    /// Orphaned message bodies removed.
    pub purged_bodies: usize,
}

impl MaintenanceReport {
    pub fn changed(&self) -> bool {
        self.woken > 0 || self.followed_up > 0
    }
}

/// How many follow-ups a single pass may raise.
///
/// After a long absence, hundreds of threads can come due at once. Raising them all
/// would dump an avalanche into the work queue; spreading them leaves the user a list
/// they can actually look at.
const MAX_FOLLOW_UPS: u32 = 50;

impl SyncEngine {
    /// Wakes due snoozes and runs the follow-ups that are owed.
    ///
    /// Without a workflow engine attached the pass does nothing and says so with an
    /// empty report: the caller is a background loop that must not fail because an
    /// optional collaborator is missing.
    pub fn run_maintenance(&self, now: Timestamp) -> Result<MaintenanceReport> {
        // Acknowledged operations only serve diagnosis; a week is plenty, and without
        // a purge the journal grows for ever.
        const WEEK_MS: i64 = 7 * 24 * 3600 * 1000;
        self.store()
            .purge_completed_ops(Timestamp::from_millis(now.millis() - WEEK_MS))?;
        // A message moved comes back at the next pass; one gone a month is gone.
        self.store()
            .purge_thread_ghosts(Timestamp::from_millis(now.millis() - 4 * WEEK_MS))?;

        let Some(workflow) = self.workflow() else {
            return Ok(MaintenanceReport::default());
        };

        Ok(MaintenanceReport {
            woken: workflow.wake_due_snoozes(now)?,
            followed_up: workflow.run_follow_ups(now, MAX_FOLLOW_UPS)?,
            purged_bodies: 0,
        })
    }

    /// Removes bodies no message claims any more.
    pub fn purge_bodies(&self) -> Result<usize> {
        let Some(blobs) = self.blobs() else {
            return Ok(0);
        };
        crate::body::purge_orphan_bodies(self.store(), blobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, StaticCredentials, SyncEngine};
    use iris_imap::fake::FakeServer;
    use iris_imap::Connector;
    use iris_kernel::{Event, EventBus, EventKind};
    use iris_store::{FolderRole, NewAccount, NewMessage, Store};
    use iris_types::{AutomationSettings, Flags, Snooze, ThreadId, TransitionCause, WorkflowState};
    use iris_workflow::Workflow;
    use std::sync::Arc;

    struct Fixture {
        engine: SyncEngine,
        store: Arc<Store>,
        workflow: Arc<Workflow>,
        bus: EventBus,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        let account = store
            .create_account(
                &NewAccount::new("a@x.fr", "imap.x.fr", "s"),
                Timestamp::EPOCH,
            )
            .unwrap();
        store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();

        let bus = EventBus::new();
        let workflow = Arc::new(Workflow::new(
            Arc::clone(&store),
            bus.clone(),
            AutomationSettings::default(),
        ));

        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::new(FakeServer::default()) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            bus.clone(),
            EngineConfig::default(),
        )
        .with_workflow(Arc::clone(&workflow));

        Fixture {
            engine,
            store,
            workflow,
            bus,
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        fn thread(&self, millis: i64) -> ThreadId {
            let uid = self.uid.get();
            self.uid.set(uid + 1);
            let account = self.store.accounts().unwrap()[0].id;
            let folder = self.store.folders(account).unwrap()[0].id;

            self.store
                .insert_message(&NewMessage {
                    account,
                    folder,
                    uid,
                    rfc_message_id: Some(format!("m{uid}@x")),
                    in_reply_to: None,
                    references: vec![],
                    subject: format!("Subject {uid}"),
                    from_name: "Marie".into(),
                    from_addr: "marie@x.fr".into(),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(millis),
                    received: Timestamp::from_millis(millis),
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

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_millis(secs * 1000)
    }

    #[test]
    fn a_due_snooze_is_woken() {
        let f = fixture();
        let thread = f.thread(1000);
        f.store
            .snooze_thread(
                thread,
                Snooze {
                    until: t(5000),
                    restore_to: WorkflowState::Todo,
                },
            )
            .unwrap();

        assert_eq!(f.engine.run_maintenance(t(4999)).unwrap().woken, 0);
        assert_eq!(f.engine.run_maintenance(t(5000)).unwrap().woken, 1);
        assert!(f
            .store
            .thread_row(thread)
            .unwrap()
            .unwrap()
            .snoozed_until
            .is_none());
    }

    #[test]
    fn the_snooze_restores_the_state_rather_than_deciding_one() {
        let f = fixture();
        let thread = f.thread(1000);
        f.workflow
            .set_state(thread, WorkflowState::Waiting, t(1))
            .unwrap();
        f.workflow.snooze(thread, t(5000), t(2)).unwrap();

        f.engine.run_maintenance(t(6000)).unwrap();
        assert_eq!(f.state(thread), WorkflowState::Waiting);
    }

    #[test]
    fn a_wake_up_is_announced_on_the_bus() {
        let f = fixture();
        let thread = f.thread(1000);
        f.store
            .snooze_thread(
                thread,
                Snooze {
                    until: t(1),
                    restore_to: WorkflowState::Done,
                },
            )
            .unwrap();
        let mut subscriber = f.bus.subscribe_kind(EventKind::Workflow);

        f.engine.run_maintenance(t(100)).unwrap();
        let events = subscriber.drain();

        assert!(events
            .iter()
            .any(|e| matches!(e, Event::ThreadUnsnoozed { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            Event::ThreadStateChanged {
                cause: TransitionCause::SnoozeExpired,
                ..
            }
        )));
    }

    #[test]
    fn a_thread_nobody_answered_is_brought_back() {
        let f = fixture();
        let thread = f.thread(1_000);
        f.store
            .set_thread_state(thread, WorkflowState::Waiting)
            .unwrap();

        let later = t(1 + 4 * 86_400);
        assert_eq!(f.engine.run_maintenance(later).unwrap().followed_up, 1);
        assert_eq!(f.state(thread), WorkflowState::Todo);
    }

    #[test]
    fn a_thread_answered_recently_is_left_alone() {
        let f = fixture();
        let thread = f.thread(1_000_000_000);
        f.store
            .set_thread_state(thread, WorkflowState::Waiting)
            .unwrap();

        let report = f
            .engine
            .run_maintenance(Timestamp::from_millis(1_000_100_000))
            .unwrap();
        assert_eq!(report.followed_up, 0);
    }

    #[test]
    fn follow_ups_can_be_turned_off() {
        let f = fixture();
        f.workflow.set_settings(AutomationSettings {
            follow_up_enabled: false,
            ..Default::default()
        });
        let thread = f.thread(1_000);
        f.store
            .set_thread_state(thread, WorkflowState::Waiting)
            .unwrap();

        assert_eq!(f.engine.run_maintenance(t(999_999)).unwrap().followed_up, 0);
        assert_eq!(f.state(thread), WorkflowState::Waiting);
    }

    #[test]
    fn the_pass_is_idempotent() {
        // It is called on every synchronisation tick; running it again must produce
        // nothing more.
        let f = fixture();
        let thread = f.thread(1_000);
        f.store
            .set_thread_state(thread, WorkflowState::Waiting)
            .unwrap();

        let later = t(1 + 10 * 86_400);
        assert_eq!(f.engine.run_maintenance(later).unwrap().followed_up, 1);
        assert_eq!(f.engine.run_maintenance(later).unwrap().followed_up, 0);
    }

    #[test]
    fn follow_ups_are_spread_out() {
        // After a long absence, an avalanche would make the queue unreadable.
        let f = fixture();
        for i in 0..80 {
            let thread = f.thread(1_000 + i);
            f.store
                .set_thread_state(thread, WorkflowState::Waiting)
                .unwrap();
        }

        let later = t(1 + 30 * 86_400);
        assert_eq!(
            f.engine.run_maintenance(later).unwrap().followed_up,
            MAX_FOLLOW_UPS as usize
        );
        assert_eq!(
            f.engine.run_maintenance(later).unwrap().followed_up,
            30,
            "the rest comes on the next pass"
        );
    }

    #[test]
    fn with_nothing_to_do_the_pass_is_silent() {
        let f = fixture();
        f.thread(1000);
        assert!(!f.engine.run_maintenance(t(2000)).unwrap().changed());
    }

    #[test]
    fn without_a_workflow_the_pass_does_nothing_instead_of_failing() {
        // The caller is a background loop; it must not die because an optional
        // collaborator is missing.
        let store = Arc::new(Store::in_memory().unwrap());
        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::new(FakeServer::default()) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            EventBus::new(),
            EngineConfig::default(),
        );

        assert_eq!(
            engine.run_maintenance(t(1)).unwrap(),
            MaintenanceReport::default()
        );
    }

    #[test]
    fn purging_without_a_blob_store_does_nothing() {
        let f = fixture();
        assert_eq!(f.engine.purge_bodies().unwrap(), 0);
    }
}
