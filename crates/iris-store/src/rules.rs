//! Stored rules, and what they have already touched.
//!
//! The rule definitions themselves are opaque here: the store keeps their JSON, their
//! order and whether they are enabled, and hands them back. Deciding what a rule means
//! belongs to `iris-rules`, which is pure and can therefore be tested without a
//! database — the separation is what keeps the rule language easy to change.
//!
//! What this module does own is the record of **what has already been applied**.
//! Without it a rule that snoozes would push its own target forward on every
//! synchronisation, and the message would never come back. Rules act once per message.

use crate::{sql_err, Store};
use iris_types::{MessageId, Result, Timestamp};

/// A rule as stored: ordering and enablement, plus an opaque definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub position: u32,
    /// Serialised `iris_rules::Rule`.
    pub definition: String,
}

impl Store {
    /// Every rule, in the order the user arranged them.
    ///
    /// Order matters: a rule can stop the ones after it, so "first match wins" is a
    /// decision the user makes by dragging, not one the database makes by accident.
    pub fn rules(&self) -> Result<Vec<StoredRule>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached(
                    "SELECT id, name, enabled, position, definition
                     FROM rules ORDER BY position, id",
                )
                .map_err(|e| sql_err("prepare", e))?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(StoredRule {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        enabled: row.get::<_, i64>(2)? != 0,
                        position: row.get::<_, i64>(3)? as u32,
                        definition: row.get(4)?,
                    })
                })
                .map_err(|e| sql_err("read rules", e))?;

            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| sql_err("read a rule", e))
        })
    }

    /// Inserts or replaces a rule.
    pub fn upsert_rule(&self, rule: &StoredRule) -> Result<()> {
        self.with_conn(|c| {
            c.prepare_cached(
                "INSERT INTO rules (id, name, enabled, position, definition)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                     name = excluded.name,
                     enabled = excluded.enabled,
                     position = excluded.position,
                     definition = excluded.definition",
            )
            .map_err(|e| sql_err("prepare", e))?
            .execute(rusqlite::params![
                rule.id,
                rule.name,
                rule.enabled as i64,
                rule.position as i64,
                rule.definition,
            ])
            .map_err(|e| sql_err("write rule", e))?;
            Ok(())
        })
    }

    /// Removes a rule, and forgets where it was applied.
    ///
    /// Forgetting matters: a rule recreated with the same identifier is a new rule in
    /// the user's mind, and it should get a fresh look at the mailbox.
    pub fn delete_rule(&self, id: &str) -> Result<bool> {
        self.with_conn(|c| {
            c.prepare_cached("DELETE FROM rule_applications WHERE rule_id = ?1")
                .map_err(|e| sql_err("prepare", e))?
                .execute([id])
                .map_err(|e| sql_err("forget applications", e))?;

            let removed = c
                .prepare_cached("DELETE FROM rules WHERE id = ?1")
                .map_err(|e| sql_err("prepare", e))?
                .execute([id])
                .map_err(|e| sql_err("delete rule", e))?;
            Ok(removed > 0)
        })
    }

    /// Reorders the rules, in the given order.
    pub fn reorder_rules(&self, ids: &[String]) -> Result<()> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare_cached("UPDATE rules SET position = ?2 WHERE id = ?1")
                .map_err(|e| sql_err("prepare", e))?;
            for (position, id) in ids.iter().enumerate() {
                stmt.execute(rusqlite::params![id, position as i64])
                    .map_err(|e| sql_err("reorder rules", e))?;
            }
            Ok(())
        })
    }

    /// Has this rule already acted on this message?
    pub fn rule_was_applied(&self, rule: &str, message: MessageId) -> Result<bool> {
        self.with_conn(|c| {
            let count: i64 = c
                .prepare_cached(
                    "SELECT count(*) FROM rule_applications
                     WHERE rule_id = ?1 AND message_id = ?2",
                )
                .map_err(|e| sql_err("prepare", e))?
                .query_row(rusqlite::params![rule, message.get()], |r| r.get(0))
                .map_err(|e| sql_err("read application", e))?;
            Ok(count > 0)
        })
    }

    /// Records that a rule acted on a message.
    pub fn record_rule_application(
        &self,
        rule: &str,
        message: MessageId,
        at: Timestamp,
    ) -> Result<()> {
        self.with_conn(|c| {
            c.prepare_cached(
                "INSERT OR IGNORE INTO rule_applications (rule_id, message_id, applied_at)
                 VALUES (?1, ?2, ?3)",
            )
            .map_err(|e| sql_err("prepare", e))?
            .execute(rusqlite::params![rule, message.get(), at.millis()])
            .map_err(|e| sql_err("record application", e))?;
            Ok(())
        })
    }

    /// How many messages a rule has touched, for the rules screen.
    pub fn rule_application_count(&self, rule: &str) -> Result<u64> {
        self.with_conn(|c| {
            let count: i64 = c
                .prepare_cached("SELECT count(*) FROM rule_applications WHERE rule_id = ?1")
                .map_err(|e| sql_err("prepare", e))?
                .query_row([rule], |r| r.get(0))
                .map_err(|e| sql_err("count applications", e))?;
            Ok(count as u64)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FolderRole, NewAccount, NewMessage};
    use iris_types::Flags;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    fn rule(id: &str, position: u32) -> StoredRule {
        StoredRule {
            id: id.into(),
            name: format!("Rule {id}"),
            enabled: true,
            position,
            definition: r#"{"kind":"test"}"#.into(),
        }
    }

    fn a_message(store: &Store) -> MessageId {
        let account = store
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), Timestamp::EPOCH)
            .unwrap();
        let folder = store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        store
            .insert_message(&NewMessage {
                account,
                folder,
                uid: 1,
                rfc_message_id: Some("m1@x".into()),
                in_reply_to: None,
                references: vec![],
                subject: "Subject".into(),
                from_name: "Marie".into(),
                from_addr: "marie@x.fr".into(),
                recipients_json: "[]".into(),
                date: Timestamp::EPOCH,
                received: Timestamp::EPOCH,
                size: 10,
                flags: Flags::NONE,
                preview: String::new(),
            })
            .unwrap()
            .message
    }

    #[test]
    fn rules_come_back_in_the_order_the_user_arranged() {
        // A rule can stop the ones after it: the order is a decision, not an accident.
        let s = store();
        s.upsert_rule(&rule("c", 2)).unwrap();
        s.upsert_rule(&rule("a", 0)).unwrap();
        s.upsert_rule(&rule("b", 1)).unwrap();

        let ids: Vec<_> = s.rules().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn a_rule_can_be_edited_in_place() {
        let s = store();
        s.upsert_rule(&rule("a", 0)).unwrap();

        let mut updated = rule("a", 0);
        updated.name = "Renamed".into();
        updated.enabled = false;
        s.upsert_rule(&updated).unwrap();

        let stored = s.rules().unwrap();
        assert_eq!(stored.len(), 1, "editing must not create a second rule");
        assert_eq!(stored[0].name, "Renamed");
        assert!(!stored[0].enabled);
    }

    #[test]
    fn reordering_rewrites_the_positions() {
        let s = store();
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            s.upsert_rule(&rule(id, i as u32)).unwrap();
        }

        s.reorder_rules(&["c".into(), "a".into(), "b".into()])
            .unwrap();
        let ids: Vec<_> = s.rules().unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    #[test]
    fn a_rule_acts_once_per_message() {
        // Otherwise a snoozing rule would push its own target forward on every
        // synchronisation, and the message would never come back.
        let s = store();
        let message = a_message(&s);
        s.upsert_rule(&rule("a", 0)).unwrap();

        assert!(!s.rule_was_applied("a", message).unwrap());
        s.record_rule_application("a", message, Timestamp::EPOCH)
            .unwrap();
        assert!(s.rule_was_applied("a", message).unwrap());
    }

    #[test]
    fn recording_the_same_application_twice_is_harmless() {
        let s = store();
        let message = a_message(&s);

        s.record_rule_application("a", message, Timestamp::EPOCH)
            .unwrap();
        s.record_rule_application("a", message, Timestamp::EPOCH)
            .unwrap();
        assert_eq!(s.rule_application_count("a").unwrap(), 1);
    }

    #[test]
    fn deleting_a_rule_forgets_where_it_acted() {
        // A rule recreated with the same identifier is a new rule in the user's mind,
        // and deserves a fresh look at the mailbox.
        let s = store();
        let message = a_message(&s);
        s.upsert_rule(&rule("a", 0)).unwrap();
        s.record_rule_application("a", message, Timestamp::EPOCH)
            .unwrap();

        assert!(s.delete_rule("a").unwrap());
        assert!(!s.rule_was_applied("a", message).unwrap());
        assert!(s.rules().unwrap().is_empty());
    }

    #[test]
    fn deleting_a_rule_that_does_not_exist_says_so() {
        let s = store();
        assert!(!s.delete_rule("ghost").unwrap());
    }

    #[test]
    fn deleting_a_message_forgets_the_rules_that_touched_it() {
        let s = store();
        let message = a_message(&s);
        s.record_rule_application("a", message, Timestamp::EPOCH)
            .unwrap();

        let account = s.accounts().unwrap()[0].id;
        let folder = s.folders(account).unwrap()[0].id;
        s.delete_messages_by_uid(folder, &[1]).unwrap();

        assert_eq!(s.rule_application_count("a").unwrap(), 0);
    }

    #[test]
    fn a_fresh_database_has_no_rules() {
        assert!(store().rules().unwrap().is_empty());
    }
}
