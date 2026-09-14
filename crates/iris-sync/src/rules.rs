//! Running the rules.
//!
//! `iris-rules` decides; this module acts. Keeping the two apart is what lets the
//! rule language be tested exhaustively without a database, and what lets the dry run
//! be trustworthy: the simulation calls exactly the same decision code as the real
//! pass, so what it promises is what happens.
//!
//! Three properties are worth stating, because each one is a bug the first naive
//! implementation would have:
//!
//! - a rule acts **once per message**. A snoozing rule that ran on every sync would
//!   push its own target forward forever, and the message would never come back;
//! - state changes go through the **workflow engine**, not straight to the store, so
//!   they are undoable and announced like any other change;
//! - a rule that fails does not stop the others. One badly written rule must not
//!   silently disable the rest of the mailbox's automation.

use crate::engine::SyncEngine;
use iris_rules::{Action, Facts, Rule};
use iris_store::StoredMessage;
use iris_types::{Flags, MessageId, Result, Timestamp};

/// What a rules pass produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulesReport {
    /// Messages examined.
    pub examined: usize,
    /// Messages at least one rule touched.
    pub affected: usize,
    /// Actions actually carried out.
    pub actions: usize,
    /// Rules that failed, with the reason.
    pub failures: Vec<(String, String)>,
}

impl RulesReport {
    pub fn changed(&self) -> bool {
        self.actions > 0
    }
}

/// Turns a stored message into the facts a rule can see.
///
/// Deliberately narrow: a rule reads the envelope, not the body. Reading bodies would
/// mean downloading every message before any rule could run, which is exactly the
/// behaviour the on-demand body policy exists to avoid.
pub fn facts_of(message: &StoredMessage) -> Facts {
    Facts {
        from_addr: message.from_addr.to_lowercase(),
        from_name: message.from_name.clone(),
        subject: message.subject.clone(),
        // The recipients are not on the stored row; a rule that needs them will get
        // them the day the row carries them, rather than the day it triggers a
        // per-message query in a loop over the whole mailbox.
        recipients: String::new(),
        flags: message.flags,
        size: message.size,
        received: message.received,
        known_correspondent: false,
        folder: String::new(),
    }
}

impl SyncEngine {
    /// Reads the enabled rules from the store.
    ///
    /// A rule whose definition no longer parses is skipped and reported, never fatal:
    /// a mailbox must keep working after a downgrade that does not understand a newer
    /// condition.
    pub fn load_rules(&self) -> (Vec<Rule>, Vec<(String, String)>) {
        let mut rules = Vec::new();
        let mut failures = Vec::new();

        let stored = match self.store().rules() {
            Ok(r) => r,
            Err(e) => return (rules, vec![("<store>".into(), e.to_string())]),
        };

        for row in stored {
            if !row.enabled {
                continue;
            }
            match serde_json::from_str::<Rule>(&row.definition) {
                Ok(rule) => rules.push(rule),
                Err(e) => failures.push((row.id, format!("unreadable definition: {e}"))),
            }
        }

        (rules, failures)
    }

    /// Applies the rules to a set of messages.
    pub fn apply_rules(&self, messages: &[MessageId], now: Timestamp) -> Result<RulesReport> {
        let (rules, failures) = self.load_rules();
        let mut report = RulesReport {
            failures,
            ..Default::default()
        };

        if rules.is_empty() {
            return Ok(report);
        }

        for id in messages {
            let Some(message) = self.store().message_by_id(*id)? else {
                continue;
            };
            report.examined += 1;

            let verdict = iris_rules::evaluate(&rules, &facts_of(&message), now);
            if verdict.actions.is_empty() {
                continue;
            }

            // Only the rules that have not already acted on this message.
            let mut acted = false;
            for rule_id in &verdict.matched {
                if self.store().rule_was_applied(rule_id, *id)? {
                    continue;
                }
                acted = true;
                self.store().record_rule_application(rule_id, *id, now)?;
            }
            if !acted {
                continue;
            }

            report.affected += 1;
            for action in &verdict.actions {
                match self.perform(action, &message, now) {
                    Ok(true) => report.actions += 1,
                    Ok(false) => {}
                    // One badly written rule must not disable the rest.
                    Err(e) => report.failures.push((format!("{action:?}"), e.to_string())),
                }
            }
        }

        Ok(report)
    }

    /// Carries out a single action. Returns whether anything changed.
    fn perform(&self, action: &Action, message: &StoredMessage, now: Timestamp) -> Result<bool> {
        let Some(workflow) = self.workflow() else {
            return Ok(false);
        };
        let thread = message.thread;

        match action {
            Action::SetState(state) => Ok(workflow.set_state(thread, *state, now)?.changed()),
            Action::MarkSeen => workflow.set_read(thread, true, now),
            Action::Flag => workflow.set_flagged(thread, true, now),
            Action::SnoozeDays(days) => {
                let until = Timestamp::from_millis(now.millis() + *days as i64 * 86_400_000);
                workflow.snooze(thread, until, now)
            }
            // Moving between folders and notifying need collaborators the engine does
            // not have. Saying so once is better than pretending the rule worked.
            Action::MoveTo(folder) => {
                tracing::info!(folder = %folder, "rule action not supported yet: move");
                Ok(false)
            }
            Action::Notify => {
                tracing::info!(
                    subject = %message.subject,
                    "rule action not supported yet: notify"
                );
                Ok(false)
            }
        }
    }

    /// Runs the rules over recent history without changing anything.
    ///
    /// Nobody dares write a rule that archives mail without knowing what it will
    /// touch. The dry run is what turns an alarming feature into a trustworthy one,
    /// and it is only trustworthy because it calls the same decision code.
    pub fn simulate_rules(
        &self,
        candidate: Option<&Rule>,
        limit: usize,
        now: Timestamp,
    ) -> Result<iris_rules::Simulation> {
        let rules = match candidate {
            Some(rule) => vec![rule.clone()],
            None => self.load_rules().0,
        };

        let messages = self.store().latest_messages(limit)?;
        let facts: Vec<Facts> = messages.iter().map(facts_of).collect();

        Ok(iris_rules::simulate(&rules, &facts, now, 20))
    }
}

/// Whether a message looks like one a rule would want to see.
///
/// Used to skip the pass entirely when nothing new has arrived.
pub fn worth_examining(flags: Flags) -> bool {
    !flags.contains(Flags::DELETED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, StaticCredentials, SyncEngine};
    use iris_imap::fake::FakeServer;
    use iris_imap::Connector;
    use iris_kernel::EventBus;
    use iris_rules::{Condition, Rule};
    use iris_store::{FolderRole, NewAccount, NewMessage, Store, StoredRule};
    use iris_types::{AutomationSettings, WorkflowState};
    use iris_workflow::Workflow;
    use std::sync::Arc;

    struct Fixture {
        engine: SyncEngine,
        store: Arc<Store>,
        workflow: Arc<Workflow>,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        let account = store
            .create_account(
                &NewAccount::new("me@x.fr", "imap.x.fr", "s"),
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
            bus,
            EngineConfig::default(),
        )
        .with_workflow(Arc::clone(&workflow));

        Fixture {
            engine,
            store,
            workflow,
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        fn message(&self, from: &str, subject: &str, flags: Flags) -> MessageId {
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
                    subject: subject.into(),
                    from_name: "Sender".into(),
                    from_addr: from.into(),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(1000 * uid as i64),
                    received: Timestamp::from_millis(1000 * uid as i64),
                    size: 1024,
                    flags,
                    preview: String::new(),
                })
                .unwrap()
                .message
        }

        fn install(&self, rule: &Rule, position: u32) {
            self.store
                .upsert_rule(&StoredRule {
                    id: rule.id.clone(),
                    name: rule.name.clone(),
                    enabled: rule.enabled,
                    position,
                    definition: serde_json::to_string(rule).unwrap(),
                })
                .unwrap();
        }

        fn state_of(&self, message: MessageId) -> WorkflowState {
            let thread = self.store.message_by_id(message).unwrap().unwrap().thread;
            self.store.thread_row(thread).unwrap().unwrap().state
        }
    }

    fn now() -> Timestamp {
        Timestamp::from_millis(1_000_000_000)
    }

    fn newsletter_rule() -> Rule {
        Rule::new("newsletters", "Archive newsletters")
            .when(Condition::FromDomain("news.example".into()))
            .then(Action::SetState(WorkflowState::Done))
    }

    #[test]
    fn a_matching_rule_acts() {
        let f = fixture();
        f.install(&newsletter_rule(), 0);
        let message = f.message("weekly@news.example", "This week", Flags::NONE);

        let report = f.engine.apply_rules(&[message], now()).unwrap();
        assert_eq!(report.affected, 1);
        assert_eq!(f.state_of(message), WorkflowState::Done);
    }

    #[test]
    fn a_message_no_rule_matches_is_left_alone() {
        let f = fixture();
        f.install(&newsletter_rule(), 0);
        let message = f.message("marie@client.fr", "Quote", Flags::NONE);

        let report = f.engine.apply_rules(&[message], now()).unwrap();
        assert_eq!(report.affected, 0);
        assert_eq!(f.state_of(message), WorkflowState::Todo);
    }

    #[test]
    fn a_rule_acts_only_once_on_a_message() {
        // A snoozing rule that ran on every sync would push its own target forward
        // forever, and the message would never come back.
        let f = fixture();
        f.install(
            &Rule::new("snooze", "Defer")
                .when(Condition::FromDomain("news.example".into()))
                .then(Action::SnoozeDays(3)),
            0,
        );
        let message = f.message("weekly@news.example", "This week", Flags::NONE);

        assert_eq!(f.engine.apply_rules(&[message], now()).unwrap().affected, 1);
        let first = f
            .store
            .thread_row(f.store.message_by_id(message).unwrap().unwrap().thread)
            .unwrap()
            .unwrap()
            .snoozed_until;

        let later = Timestamp::from_millis(now().millis() + 3_600_000);
        assert_eq!(f.engine.apply_rules(&[message], later).unwrap().affected, 0);
        let second = f
            .store
            .thread_row(f.store.message_by_id(message).unwrap().unwrap().thread)
            .unwrap()
            .unwrap()
            .snoozed_until;

        assert_eq!(first, second, "the deadline must not move");
    }

    #[test]
    fn a_disabled_rule_does_nothing() {
        let f = fixture();
        let mut rule = newsletter_rule();
        rule.enabled = false;
        f.install(&rule, 0);
        let message = f.message("weekly@news.example", "This week", Flags::NONE);

        assert_eq!(f.engine.apply_rules(&[message], now()).unwrap().affected, 0);
        assert_eq!(f.state_of(message), WorkflowState::Todo);
    }

    #[test]
    fn an_unreadable_rule_is_reported_not_fatal() {
        // A mailbox must keep working after a downgrade that does not understand a
        // newer condition.
        let f = fixture();
        f.store
            .upsert_rule(&StoredRule {
                id: "broken".into(),
                name: "Broken".into(),
                enabled: true,
                position: 0,
                definition: "{ not json".into(),
            })
            .unwrap();
        f.install(&newsletter_rule(), 1);
        let message = f.message("weekly@news.example", "This week", Flags::NONE);

        let report = f.engine.apply_rules(&[message], now()).unwrap();
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.affected, 1, "the readable rule still runs");
    }

    #[test]
    fn state_changes_go_through_the_workflow_and_are_undoable() {
        // A rule writing straight to the store would produce a change the user cannot
        // take back and plugins never hear about.
        let f = fixture();
        f.install(&newsletter_rule(), 0);
        let message = f.message("weekly@news.example", "This week", Flags::NONE);

        f.engine.apply_rules(&[message], now()).unwrap();
        assert_eq!(f.state_of(message), WorkflowState::Done);

        f.workflow.undo(now()).unwrap();
        assert_eq!(f.state_of(message), WorkflowState::Todo);
    }

    #[test]
    fn a_marking_rule_marks_the_thread_read() {
        let f = fixture();
        f.install(
            &Rule::new("read", "Mark read")
                .when(Condition::SubjectContains("newsletter".into()))
                .then(Action::MarkSeen),
            0,
        );
        let message = f.message("a@b.fr", "Our newsletter", Flags::NONE);

        f.engine.apply_rules(&[message], now()).unwrap();
        assert!(f
            .store
            .message_by_id(message)
            .unwrap()
            .unwrap()
            .flags
            .contains(Flags::SEEN));
    }

    #[test]
    fn with_no_rules_the_pass_examines_nothing() {
        let f = fixture();
        let message = f.message("a@b.fr", "Hello", Flags::NONE);

        let report = f.engine.apply_rules(&[message], now()).unwrap();
        assert_eq!(report.examined, 0);
        assert!(!report.changed());
    }

    #[test]
    fn a_message_that_vanished_does_not_stop_the_pass() {
        let f = fixture();
        f.install(&newsletter_rule(), 0);
        let real = f.message("weekly@news.example", "This week", Flags::NONE);

        let report = f
            .engine
            .apply_rules(&[MessageId(9999), real], now())
            .unwrap();
        assert_eq!(report.examined, 1);
        assert_eq!(report.affected, 1);
    }

    #[test]
    fn the_dry_run_counts_without_changing_anything() {
        // Nobody dares write a rule that archives mail without knowing what it will
        // touch.
        let f = fixture();
        for i in 0..5 {
            f.message("weekly@news.example", &format!("Issue {i}"), Flags::NONE);
        }
        f.message("marie@client.fr", "Quote", Flags::NONE);

        let simulation = f
            .engine
            .simulate_rules(Some(&newsletter_rule()), 100, now())
            .unwrap();

        assert_eq!(simulation.examined, 6);
        assert_eq!(simulation.affected, 5);
        assert!(simulation.summary().contains('5'));

        // And nothing moved.
        let all_todo = f
            .store
            .list_threads(&iris_store::ListQuery::new(WorkflowState::Todo, 100))
            .unwrap();
        assert_eq!(all_todo.len(), 6);
    }

    #[test]
    fn the_dry_run_keeps_a_bounded_sample() {
        // On a million messages we want the exact count and twenty examples, not a
        // million lines.
        let f = fixture();
        for i in 0..60 {
            f.message("weekly@news.example", &format!("Issue {i}"), Flags::NONE);
        }

        let simulation = f
            .engine
            .simulate_rules(Some(&newsletter_rule()), 100, now())
            .unwrap();
        assert_eq!(simulation.affected, 60);
        assert!(simulation.sample.len() <= 20);
    }

    #[test]
    fn the_dry_run_uses_the_installed_rules_when_none_is_given() {
        let f = fixture();
        f.install(&newsletter_rule(), 0);
        f.message("weekly@news.example", "This week", Flags::NONE);

        let simulation = f.engine.simulate_rules(None, 100, now()).unwrap();
        assert_eq!(simulation.affected, 1);
    }

    #[test]
    fn the_facts_a_rule_sees_do_not_include_the_body() {
        // Reading bodies would mean downloading every message before any rule could
        // run, which is what the on-demand policy exists to avoid.
        let f = fixture();
        let message = f.message("Marie@Client.FR", "Quote", Flags::NONE);
        let stored = f.store.message_by_id(message).unwrap().unwrap();

        let facts = facts_of(&stored);
        assert_eq!(
            facts.from_addr, "marie@client.fr",
            "addresses are lowercased"
        );
        assert_eq!(facts.subject, "Quote");
    }
}
