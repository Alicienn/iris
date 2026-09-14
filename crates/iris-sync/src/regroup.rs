//! Subject-based regrouping, on request.
//!
//! The store already joins conversations the deterministic way: when a message cites
//! another by `Message-ID`, the two land in the same thread, whichever account they
//! arrived in. That needs no help and no option — a citation is not a guess.
//!
//! What the store deliberately does **not** do is guess. Two messages both titled
//! "Invoice", with no reference between them, may or may not be one conversation.
//! Merging them is a heuristic, and a heuristic that is wrong is worse than no
//! heuristic at all: the user loses a message inside a thread they have already read.
//!
//! So the guess lives here, it is **off by default**, and it is bounded three ways:
//!
//! - by **time**: two messages titled "Invoice" three years apart are not one
//!   conversation. The window is thirty days by default;
//! - by **account**, unless the user asks otherwise: someone running one personal and
//!   one professional mailbox usually wants the wall, not the bridge;
//! - by **history**: the pass reads recent messages, not the whole mailbox.
//!
//! It is idempotent — running it twice merges nothing the second time — and it never
//! splits: it only ever joins threads, so a conversation the user is reading does not
//! come apart under them.

use crate::engine::SyncEngine;
use iris_thread::{ThreadInput, ThreadingOptions};
use iris_types::{Result, ThreadId, Timestamp};
use std::collections::BTreeMap;

/// How many recent messages a pass considers.
const WINDOW: usize = 5_000;

/// How the subject fallback should behave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegroupOptions {
    /// How far apart two same-subject messages may be, in days.
    pub window_days: u32,
    /// Allow the fallback to join threads from different accounts.
    pub across_accounts: bool,
}

impl Default for RegroupOptions {
    fn default() -> Self {
        Self {
            window_days: 30,
            across_accounts: false,
        }
    }
}

/// What a regrouping pass produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegroupReport {
    /// Messages examined.
    pub examined: usize,
    /// Threads absorbed into another.
    pub merged: usize,
    /// Messages moved to a different thread.
    pub moved: usize,
}

impl RegroupReport {
    pub fn changed(&self) -> bool {
        self.merged > 0
    }
}

impl SyncEngine {
    /// Joins threads that share a subject within the window.
    ///
    /// This is the guess, and the caller has to ask for it. The engine provides the
    /// mechanism; whether a mailbox wants it is a setting.
    pub fn regroup_by_subject(&self, options: RegroupOptions) -> Result<RegroupReport> {
        let messages = self.store().latest_messages(WINDOW)?;
        let mut report = RegroupReport {
            examined: messages.len(),
            ..Default::default()
        };
        if messages.len() < 2 {
            return Ok(report);
        }

        let current: BTreeMap<iris_types::MessageId, ThreadId> =
            messages.iter().map(|m| (m.id, m.thread)).collect();

        let inputs: Vec<ThreadInput> = messages
            .iter()
            .map(|m| {
                let mut input = ThreadInput::new(m.id, m.account, m.received);
                if let Some(rfc) = &m.rfc_message_id {
                    input = input.with_id(rfc);
                }
                input.subject(&m.subject)
            })
            .collect();

        let groups = iris_thread::group(
            &inputs,
            ThreadingOptions {
                subject_fallback: true,
                subject_window_secs: options.window_days as i64 * 86_400,
                cross_account: options.across_accounts,
            },
        );

        for group in groups {
            // The surviving thread is the oldest one: the conversation the user has
            // been reading keeps its identity, and the newcomer joins it.
            let mut members: Vec<_> = group
                .messages
                .iter()
                .filter_map(|id| current.get(id).map(|thread| (*id, *thread)))
                .collect();
            members.sort_by_key(|(_, thread)| thread.get());

            let Some((_, target)) = members.first().copied() else {
                continue;
            };
            if members.iter().all(|(_, thread)| *thread == target) {
                continue;
            }

            let mut moved_here = 0;
            for (message, thread) in &members {
                if *thread == target {
                    continue;
                }
                if self.store().move_message_to_thread(*message, target)? {
                    moved_here += 1;
                }
            }

            if moved_here > 0 {
                report.merged += 1;
                report.moved += moved_here;
            }
        }

        if report.changed() {
            self.store().prune_empty_threads()?;
            tracing::info!(
                merged = report.merged,
                moved = report.moved,
                "threads regrouped by subject"
            );
        }
        Ok(report)
    }
}

/// The instant a window of `days` ends, counted back from `now`.
pub fn window_start(now: Timestamp, days: u32) -> Timestamp {
    Timestamp::from_millis(now.millis() - days as i64 * 86_400_000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, StaticCredentials, SyncEngine};
    use iris_imap::fake::FakeServer;
    use iris_imap::Connector;
    use iris_kernel::EventBus;
    use iris_store::{FolderRole, NewAccount, NewMessage, Store};
    use iris_types::{AccountId, Flags, FolderId, MessageId};
    use std::sync::Arc;

    struct Fixture {
        engine: SyncEngine,
        store: Arc<Store>,
        work: (AccountId, FolderId),
        personal: (AccountId, FolderId),
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());

        let account = |email: &str| {
            let id = store
                .create_account(&NewAccount::new(email, "i", "s"), Timestamp::EPOCH)
                .unwrap();
            let folder = store.upsert_folder(id, "INBOX", FolderRole::Inbox).unwrap();
            (id, folder)
        };
        let work = account("me@work.fr");
        let personal = account("me@home.fr");

        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::new(FakeServer::default()) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            EventBus::new(),
            EngineConfig::default(),
        );

        Fixture {
            engine,
            store,
            work,
            personal,
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        fn message(
            &self,
            (account, folder): (AccountId, FolderId),
            subject: &str,
            days_ago: i64,
        ) -> MessageId {
            let uid = self.uid.get();
            self.uid.set(uid + 1);
            let received = Timestamp::from_millis(NOW - days_ago * 86_400_000);

            self.store
                .insert_message(&NewMessage {
                    account,
                    folder,
                    uid,
                    rfc_message_id: Some(format!("<m{uid}@x>")),
                    in_reply_to: None,
                    references: vec![],
                    subject: subject.into(),
                    from_name: "Someone".into(),
                    from_addr: "someone@example.com".into(),
                    recipients_json: "[]".into(),
                    date: received,
                    received,
                    size: 10,
                    flags: Flags::NONE,
                    preview: String::new(),
                })
                .unwrap()
                .message
        }

        /// A reply that cites another message, the deterministic way.
        fn reply_to(
            &self,
            (account, folder): (AccountId, FolderId),
            cited: &str,
            subject: &str,
        ) -> MessageId {
            let uid = self.uid.get();
            self.uid.set(uid + 1);
            self.store
                .insert_message(&NewMessage {
                    account,
                    folder,
                    uid,
                    rfc_message_id: Some(format!("<m{uid}@x>")),
                    in_reply_to: Some(cited.into()),
                    references: vec![cited.into()],
                    subject: subject.into(),
                    from_name: "Someone".into(),
                    from_addr: "someone@example.com".into(),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(NOW),
                    received: Timestamp::from_millis(NOW),
                    size: 10,
                    flags: Flags::NONE,
                    preview: String::new(),
                })
                .unwrap()
                .message
        }

        fn thread_of(&self, message: MessageId) -> ThreadId {
            self.store.message_by_id(message).unwrap().unwrap().thread
        }

        fn thread_count(&self) -> usize {
            self.store
                .list_threads(&iris_store::ListQuery::new(
                    iris_types::WorkflowState::Todo,
                    100,
                ))
                .unwrap()
                .len()
        }
    }

    const NOW: i64 = 1_700_000_000_000;

    #[test]
    fn a_citation_already_joins_threads_without_this_pass() {
        // The deterministic case is the store's job, and it does it across accounts:
        // a citation is not a guess.
        let f = fixture();
        let first = f.message(f.work, "Contract", 1);
        let rfc = f
            .store
            .message_by_id(first)
            .unwrap()
            .unwrap()
            .rfc_message_id
            .unwrap();
        let answer = f.reply_to(f.personal, &rfc, "Re: Contract");

        assert_eq!(
            f.thread_of(first),
            f.thread_of(answer),
            "no regrouping pass should be needed for this"
        );
    }

    #[test]
    fn same_subject_within_the_window_is_joined_on_request() {
        let f = fixture();
        let a = f.message(f.work, "Invoice", 10);
        let b = f.message(f.work, "Invoice", 2);
        assert_ne!(f.thread_of(a), f.thread_of(b), "the store does not guess");

        let report = f
            .engine
            .regroup_by_subject(RegroupOptions::default())
            .unwrap();
        assert_eq!(report.merged, 1);
        assert_eq!(f.thread_of(a), f.thread_of(b));
    }

    #[test]
    fn same_subject_outside_the_window_stays_apart() {
        // Two messages titled "Invoice" three years apart are not one conversation.
        let f = fixture();
        let a = f.message(f.work, "Invoice", 800);
        let b = f.message(f.work, "Invoice", 1);

        let report = f
            .engine
            .regroup_by_subject(RegroupOptions::default())
            .unwrap();
        assert_eq!(report.merged, 0);
        assert_ne!(f.thread_of(a), f.thread_of(b));
    }

    #[test]
    fn the_window_is_configurable() {
        let f = fixture();
        let a = f.message(f.work, "Invoice", 60);
        let b = f.message(f.work, "Invoice", 1);

        assert_eq!(
            f.engine
                .regroup_by_subject(RegroupOptions::default())
                .unwrap()
                .merged,
            0
        );
        assert_eq!(
            f.engine
                .regroup_by_subject(RegroupOptions {
                    window_days: 90,
                    ..Default::default()
                })
                .unwrap()
                .merged,
            1
        );
        assert_eq!(f.thread_of(a), f.thread_of(b));
    }

    #[test]
    fn the_guess_stays_inside_one_account_by_default() {
        // Someone running a personal and a professional mailbox usually wants the
        // wall, not the bridge.
        let f = fixture();
        let a = f.message(f.work, "Invoice", 5);
        let b = f.message(f.personal, "Invoice", 1);

        let report = f
            .engine
            .regroup_by_subject(RegroupOptions::default())
            .unwrap();
        assert_eq!(report.merged, 0);
        assert_ne!(f.thread_of(a), f.thread_of(b));
    }

    #[test]
    fn the_wall_can_be_taken_down_deliberately() {
        let f = fixture();
        let a = f.message(f.work, "Invoice", 5);
        let b = f.message(f.personal, "Invoice", 1);

        let report = f
            .engine
            .regroup_by_subject(RegroupOptions {
                across_accounts: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(report.merged, 1);
        assert_eq!(f.thread_of(a), f.thread_of(b));
    }

    #[test]
    fn different_subjects_are_never_joined() {
        let f = fixture();
        let a = f.message(f.work, "Invoice", 5);
        let b = f.message(f.work, "Holidays", 1);

        assert_eq!(
            f.engine
                .regroup_by_subject(RegroupOptions::default())
                .unwrap()
                .merged,
            0
        );
        assert_ne!(f.thread_of(a), f.thread_of(b));
    }

    #[test]
    fn the_oldest_thread_survives() {
        // The conversation the user has been reading keeps its identity.
        let f = fixture();
        let a = f.message(f.work, "Invoice", 10);
        let original = f.thread_of(a);
        f.message(f.work, "Invoice", 1);

        f.engine
            .regroup_by_subject(RegroupOptions::default())
            .unwrap();
        assert_eq!(f.thread_of(a), original);
    }

    #[test]
    fn running_the_pass_twice_merges_nothing_the_second_time() {
        let f = fixture();
        f.message(f.work, "Invoice", 10);
        f.message(f.work, "Invoice", 1);

        assert_eq!(
            f.engine
                .regroup_by_subject(RegroupOptions::default())
                .unwrap()
                .merged,
            1
        );
        assert_eq!(
            f.engine
                .regroup_by_subject(RegroupOptions::default())
                .unwrap()
                .merged,
            0
        );
    }

    #[test]
    fn the_emptied_thread_is_pruned() {
        // Leaving it behind would show a blank row in the list.
        let f = fixture();
        f.message(f.work, "Invoice", 10);
        f.message(f.work, "Invoice", 1);
        assert_eq!(f.thread_count(), 2);

        f.engine
            .regroup_by_subject(RegroupOptions::default())
            .unwrap();
        assert_eq!(f.thread_count(), 1);
    }

    #[test]
    fn the_merged_thread_counts_both_messages() {
        let f = fixture();
        let a = f.message(f.work, "Invoice", 10);
        f.message(f.work, "Invoice", 1);

        f.engine
            .regroup_by_subject(RegroupOptions::default())
            .unwrap();
        let row = f.store.thread_row(f.thread_of(a)).unwrap().unwrap();
        assert_eq!(row.message_count, 2);
    }

    #[test]
    fn an_empty_mailbox_is_not_an_error() {
        let f = fixture();
        assert_eq!(
            f.engine
                .regroup_by_subject(RegroupOptions::default())
                .unwrap(),
            RegroupReport::default()
        );
    }

    #[test]
    fn the_window_start_is_computed_from_days() {
        let now = Timestamp::from_millis(10 * 86_400_000);
        assert_eq!(window_start(now, 3).millis(), 7 * 86_400_000);
    }
}
