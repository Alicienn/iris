//! A conversation's queue between devices: To do, Waiting or Done, and its snooze.
//!
//! A thread's id is local, so what travels (`thread_states`) is keyed by the mailbox's
//! address and the `Message-ID` of the thread's newest message when the state was
//! decided. The other device applies it to the thread that holds that message, unless
//! newer mail came in there since: a new message outweighs a decision made before it
//! (one's own reply does not).
//! Only what the person decides is written (by hand, a reply sent, a snooze): what each
//! device works out on its own (a new message reopening, a rule, a follow-up) happens on
//! both, and written, the later fetch on one device undid a decision made on the other.

use crate::{sql_err, Store};
use iris_types::{MessageId, Result, ThreadId, Timestamp, WorkflowState};
use rusqlite::{params, Connection, OptionalExtension};

/// A thread whose queue another device changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdoptedQueue {
    pub thread: ThreadId,
    pub from: WorkflowState,
    pub to: WorkflowState,
    /// The snooze changed: `Some(until)` snoozed, `None` woken.
    pub snooze: Option<Option<Timestamp>>,
}

/// One row of `thread_states`, as merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueueRow {
    pub account: String,
    pub message_id: String,
    pub state: i64,
    pub snooze_until: Option<i64>,
    pub snooze_restore: Option<i64>,
}

impl Store {
    /// The thread's queue and snooze as they are now, to travel with the next exchange.
    /// Nothing for a thread whose newest message has no `Message-ID`: the other device
    /// could not find it.
    pub fn remember_thread_queue(&self, thread: ThreadId) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO thread_states (account, message_id, state, snooze_until, snooze_restore)
                 SELECT lower(a.email), m.rfc_message_id, t.state, t.snooze_until, t.snooze_restore
                 FROM threads t
                 JOIN messages m ON m.id = (SELECT n.id FROM messages n WHERE n.thread_id = t.id
                                            ORDER BY n.received DESC, n.id DESC LIMIT 1)
                 JOIN accounts a ON a.id = m.account_id
                 WHERE t.id = ?1 AND m.rfc_message_id IS NOT NULL
                 ON CONFLICT(account, message_id) DO UPDATE SET
                     state = excluded.state,
                     snooze_until = excluded.snooze_until,
                     snooze_restore = excluded.snooze_restore
                 WHERE thread_states.state IS NOT excluded.state
                    OR thread_states.snooze_until IS NOT excluded.snooze_until
                    OR thread_states.snooze_restore IS NOT excluded.snooze_restore",
                [thread.get()],
            )
            .map(|_| ())
            .map_err(|e| sql_err("a thread's queue to travel", e))
        })
    }

    /// The queues another device decided for these newly fetched messages, applied:
    /// mail that reached this device after the other one had dealt with it.
    pub fn adopt_queues_of(&self, messages: &[MessageId]) -> Result<Vec<AdoptedQueue>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        self.with_tx(|tx| {
            let mut faits = Vec::new();
            for m in messages {
                let ligne = tx
                    .query_row(
                        "SELECT s.account, s.message_id, s.state, s.snooze_until, s.snooze_restore
                         FROM messages m
                         JOIN accounts a ON a.id = m.account_id
                         JOIN thread_states s ON s.account = lower(a.email)
                                             AND s.message_id = m.rfc_message_id
                         WHERE m.id = ?1",
                        [m.get()],
                        |r| {
                            Ok(QueueRow {
                                account: r.get(0)?,
                                message_id: r.get(1)?,
                                state: r.get(2)?,
                                snooze_until: r.get(3)?,
                                snooze_restore: r.get(4)?,
                            })
                        },
                    )
                    .optional()
                    .map_err(|e| sql_err("a queue from another device", e))?;
                if let Some(ligne) = ligne {
                    if let Some(a) = adopt(tx, &ligne)? {
                        if !faits.iter().any(|f: &AdoptedQueue| f.thread == a.thread) {
                            faits.push(a);
                        }
                    }
                }
            }
            Ok(faits)
        })
    }
}

/// Applies a queue another device decided to the thread here that holds its message.
/// Nothing when no thread holds it, when newer mail came in since, or when the thread
/// already says the same.
pub(crate) fn adopt(c: &Connection, row: &QueueRow) -> Result<Option<AdoptedQueue>> {
    let Some(to) = WorkflowState::from_i64(row.state) else {
        return Ok(None);
    };
    let fil: Option<(i64, i64)> = c
        .query_row(
            "SELECT m.thread_id, max(m.received) FROM messages m
             JOIN accounts a ON a.id = m.account_id
             WHERE m.rfc_message_id = ?2 AND lower(a.email) = ?1
             GROUP BY m.thread_id
             ORDER BY max(m.received) DESC LIMIT 1",
            params![row.account, row.message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| sql_err("the thread of a queue", e))?;
    let Some((fil, recu)) = fil else {
        return Ok(None);
    };
    // One's own reply does not count: its copy in Sent comes down after the reply
    // set the thread Waiting, on both devices.
    let plus_recent: bool = c
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM messages WHERE thread_id = ?1 AND received > ?2
                            AND rfc_message_id IS NOT ?3
                            AND lower(from_addr) NOT IN (
                                SELECT lower(email) FROM accounts
                                UNION SELECT lower(address) FROM account_aliases))",
            params![fil, recu, row.message_id],
            |r| r.get(0),
        )
        .map_err(|e| sql_err("newer mail in the thread", e))?;
    if plus_recent {
        return Ok(None);
    }
    let avant: Option<(i64, Option<i64>, Option<i64>)> = c
        .query_row(
            "SELECT state, snooze_until, snooze_restore FROM threads WHERE id = ?1",
            [fil],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| sql_err("a thread's queue", e))?;
    let Some((etat, jusqua, retour)) = avant else {
        return Ok(None);
    };
    if etat == row.state && jusqua == row.snooze_until && retour == row.snooze_restore {
        return Ok(None);
    }
    c.execute(
        "UPDATE threads SET state = ?2, snooze_until = ?3, snooze_restore = ?4 WHERE id = ?1",
        params![fil, row.state, row.snooze_until, row.snooze_restore],
    )
    .map_err(|e| sql_err("a queue from another device", e))?;
    Ok(Some(AdoptedQueue {
        thread: ThreadId(fil),
        from: WorkflowState::from_i64(etat).unwrap_or(WorkflowState::Todo),
        to,
        snooze: (jusqua != row.snooze_until).then(|| row.snooze_until.map(Timestamp::from_millis)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::MergeReport;

    fn sql(s: &Store, q: &str) {
        s.with_conn(|c| c.execute_batch(q).map_err(|e| sql_err(q, e)))
            .unwrap();
    }

    fn nombre(s: &Store, q: &str) -> i64 {
        s.with_conn(|c| c.query_row(q, [], |r| r.get(0)).map_err(|e| sql_err(q, e)))
            .unwrap()
    }

    fn envoyer(a: &Store, b: &Store) -> MergeReport {
        let depuis = b.peer_last_clock(&a.device_id()).unwrap();
        let set = a.changes_since(&depuis).unwrap();
        b.apply_changes(&set, &depuis).unwrap()
    }

    /// A mailbox with one thread of the given messages (Message-ID, received).
    fn boite(s: &Store, email: &str, messages: &[(&str, i64)]) -> ThreadId {
        sql(
            s,
            &format!(
                "INSERT INTO accounts (email, imap_host, imap_port, smtp_host, smtp_port, created_at)
                     VALUES ('{email}', 'imap.example.com', 993, 'smtp.example.com', 465, 0);
                 INSERT INTO folders (account_id, path, role) VALUES (1, 'INBOX', 'inbox');
                 INSERT INTO threads (subject_norm) VALUES ('devis');"
            ),
        );
        for (uid, (id, recu)) in messages.iter().enumerate() {
            arrive(s, uid as i64 + 1, id, *recu);
        }
        ThreadId(1)
    }

    fn arrive(s: &Store, uid: i64, id: &str, recu: i64) {
        sql(
            s,
            &format!(
                "INSERT INTO messages (account_id, folder_id, thread_id, uid, rfc_message_id, received)
                     VALUES (1, 1, 1, {uid}, '{id}', {recu});"
            ),
        );
    }

    fn etat(s: &Store) -> i64 {
        nombre(s, "SELECT state FROM threads WHERE id = 1")
    }

    #[test]
    fn a_conversation_moved_to_done_on_one_device_is_done_on_the_other() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        let fil = boite(
            &a,
            "marie@example.com",
            &[("m1@example.com", 1), ("m2@example.com", 2)],
        );
        boite(
            &b,
            "Marie@Example.com",
            &[("m1@example.com", 1), ("m2@example.com", 2)],
        );
        a.set_thread_state(fil, WorkflowState::Done).unwrap();
        a.remember_thread_queue(fil).unwrap();

        let r = envoyer(&a, &b);
        assert_eq!(r.queues.len(), 1, "{r:?}");
        assert_eq!(etat(&b), WorkflowState::Done.as_i64());

        // And back to Waiting, then snoozed, on B: A follows.
        b.set_thread_state(fil, WorkflowState::Waiting).unwrap();
        b.snooze_thread(
            fil,
            iris_types::Snooze {
                until: Timestamp::from_millis(5_000),
                restore_to: WorkflowState::Waiting,
            },
        )
        .unwrap();
        b.remember_thread_queue(fil).unwrap();
        envoyer(&b, &a);
        assert_eq!(etat(&a), WorkflowState::Waiting.as_i64());
        assert_eq!(
            nombre(&a, "SELECT snooze_until FROM threads WHERE id = 1"),
            5_000
        );
        assert!(
            envoyer(&a, &b).queues.is_empty(),
            "nothing goes back and forth"
        );
    }

    #[test]
    fn newer_mail_here_outweighs_a_decision_made_before_it() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        let fil = boite(&a, "marie@example.com", &[("m1@example.com", 1)]);
        boite(
            &b,
            "marie@example.com",
            &[("m1@example.com", 1), ("m2@example.com", 2)],
        );
        a.set_thread_state(fil, WorkflowState::Done).unwrap();
        a.remember_thread_queue(fil).unwrap();
        envoyer(&a, &b);
        assert_eq!(
            etat(&b),
            WorkflowState::Todo.as_i64(),
            "m2 is still to read"
        );
    }

    #[test]
    fn ones_own_reply_fetched_since_does_not_hold_the_queue_back() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        let fil = boite(&a, "marie@example.com", &[("m1@example.com", 1)]);
        boite(&b, "marie@example.com", &[("m1@example.com", 1)]);
        sql(
            &b,
            "INSERT INTO messages (account_id, folder_id, thread_id, uid, rfc_message_id, received, from_addr)
                 VALUES (1, 1, 1, 9, 'r1@example.com', 5, 'Marie@example.com');",
        );
        a.set_thread_state(fil, WorkflowState::Waiting).unwrap();
        a.remember_thread_queue(fil).unwrap();
        envoyer(&a, &b);
        assert_eq!(etat(&b), WorkflowState::Waiting.as_i64());
    }

    #[test]
    fn mail_that_arrives_after_the_other_device_dealt_with_it_takes_its_queue() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        let fil = boite(
            &a,
            "marie@example.com",
            &[("m1@example.com", 1), ("m2@example.com", 2)],
        );
        boite(&b, "marie@example.com", &[("m1@example.com", 1)]);
        a.set_thread_state(fil, WorkflowState::Waiting).unwrap();
        a.remember_thread_queue(fil).unwrap();
        envoyer(&a, &b);
        assert_eq!(etat(&b), WorkflowState::Todo.as_i64(), "m2 is not here yet");

        arrive(&b, 2, "m2@example.com", 2);
        let nouveau = MessageId(nombre(&b, "SELECT max(id) FROM messages"));
        let faits = b.adopt_queues_of(&[nouveau]).unwrap();
        assert_eq!(faits.len(), 1);
        assert_eq!(faits[0].to, WorkflowState::Waiting);
        assert_eq!(etat(&b), WorkflowState::Waiting.as_i64());
    }

    #[test]
    fn what_was_decided_before_the_update_travels_to_a_device_already_paired() {
        let (a, b) = (Store::in_memory().unwrap(), Store::in_memory().unwrap());
        let fil = boite(&a, "marie@example.com", &[("m1@example.com", 1)]);
        boite(&b, "marie@example.com", &[("m1@example.com", 1)]);
        envoyer(&a, &b);
        // Decided in 7.8, before the queue travelled.
        a.set_thread_state(fil, WorkflowState::Done).unwrap();
        sql(
            &a,
            "DELETE FROM thread_states; DELETE FROM sync_tombstones;",
        );
        a.with_conn(|c| {
            c.execute_batch(crate::migrations::BACKFILL_V31)
                .map_err(|e| sql_err("backfill", e))
        })
        .unwrap();
        sql(&b, "UPDATE sync_peers SET last_clock = '';");
        envoyer(&a, &b);
        assert_eq!(etat(&b), WorkflowState::Done.as_i64());
    }
}
