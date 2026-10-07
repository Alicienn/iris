//! Mail written now to leave later.
//!
//! The store keeps what the application gives it, the draft as JSON, with the two
//! things a list of them shows (to whom, about what) beside it. It does not read the
//! payload: composing and sending belong to the application.

use crate::Store;
use iris_types::{AccountId, Error, Result, Timestamp};
use rusqlite::{params, Row};

/// A message waiting for its time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledMail {
    pub id: i64,
    pub account: AccountId,
    pub send_at: Timestamp,
    pub to_line: String,
    pub subject: String,
    /// The draft, as the application wrote it.
    pub payload: String,
}

fn err(quoi: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::store(format!("{quoi} : {e}"))
}

fn ligne(r: &Row<'_>) -> rusqlite::Result<ScheduledMail> {
    Ok(ScheduledMail {
        id: r.get(0)?,
        account: AccountId(r.get(1)?),
        send_at: Timestamp::from_millis(r.get(2)?),
        to_line: r.get(3)?,
        subject: r.get(4)?,
        payload: r.get(5)?,
    })
}

const COLONNES: &str = "id, account_id, send_at, to_line, subject, payload";

impl Store {
    /// Keeps a message to send at `send_at`; its identifier.
    pub fn schedule_mail(
        &self,
        account: AccountId,
        send_at: Timestamp,
        to_line: &str,
        subject: &str,
        payload: &str,
        now: Timestamp,
    ) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO scheduled_mail (account_id, send_at, to_line, subject, payload, \
                 created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    account.get(),
                    send_at.millis(),
                    to_line,
                    subject,
                    payload,
                    now.millis()
                ],
            )
            .map_err(err("envoi différé"))?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Every message waiting, the soonest first.
    pub fn scheduled_mail(&self) -> Result<Vec<ScheduledMail>> {
        self.scheduled_where("1", params![])
    }

    /// Every message waiting, without its draft: what their list shows. Read every
    /// half minute on the window's thread, the drafts (attachments included) made it
    /// stall.
    pub fn scheduled_mail_list(&self) -> Result<Vec<ScheduledMail>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, account_id, send_at, to_line, subject, '' FROM scheduled_mail
                     ORDER BY send_at, id",
                )
                .map_err(err("envois différés"))?;
            let lignes = stmt
                .query_map([], ligne)
                .map_err(err("envois différés"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("envois différés"));
            lignes
        })
    }

    /// The messages whose time has come.
    pub fn due_scheduled_mail(&self, now: Timestamp) -> Result<Vec<ScheduledMail>> {
        self.scheduled_where("send_at <= ?1", [now.millis()])
    }

    /// One of them, by its identifier.
    pub fn scheduled_mail_by_id(&self, id: i64) -> Result<Option<ScheduledMail>> {
        Ok(self.scheduled_where("id = ?1", [id])?.into_iter().next())
    }

    /// How many messages of a mailbox wait to be sent: what removing it throws away.
    pub fn scheduled_mail_count(&self, account: AccountId) -> Result<usize> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT count(*) FROM scheduled_mail WHERE account_id = ?1",
                [account.get()],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as usize)
            .map_err(err("envois différés"))
        })
    }

    fn scheduled_where<P: rusqlite::Params>(
        &self,
        condition: &str,
        args: P,
    ) -> Result<Vec<ScheduledMail>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM scheduled_mail WHERE {condition} ORDER BY send_at, id"
                ))
                .map_err(err("envois différés"))?;
            let lignes = stmt
                .query_map(args, ligne)
                .map_err(err("envois différés"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("envois différés"));
            lignes
        })
    }

    /// Takes a message out of the waiting ones: sent, or taken back.
    pub fn unschedule_mail(&self, id: i64) -> Result<bool> {
        self.with_conn(|c| {
            c.execute("DELETE FROM scheduled_mail WHERE id = ?1", [id])
                .map(|n| n > 0)
                .map_err(err("envoi différé"))
        })
    }

    /// Moves a waiting message to a later time: it did not leave, and is tried again.
    pub fn postpone_scheduled_mail(&self, id: i64, send_at: Timestamp) -> Result<bool> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE scheduled_mail SET send_at = ?1 WHERE id = ?2",
                rusqlite::params![send_at.millis(), id],
            )
            .map(|n| n > 0)
            .map_err(err("envoi différé"))
        })
    }
}

impl Store {
    /// What was answered to the invitation `uid`: "ACCEPTED", "TENTATIVE", "DECLINED".
    pub fn invite_reply(&self, uid: &str) -> Result<Option<String>> {
        self.with_conn(|c| {
            use rusqlite::OptionalExtension;
            c.query_row(
                "SELECT partstat FROM invite_replies WHERE uid = ?1",
                [uid],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(err("réponse à une invitation"))
        })
    }

    /// Forgets the answer to the invitation `uid`: it did not reach the organiser.
    pub fn clear_invite_reply(&self, uid: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM invite_replies WHERE uid = ?1", [uid])
                .map(|_| ())
                .map_err(err("réponse à une invitation"))
        })
    }

    /// Keeps the answer given to the invitation `uid`, in place of an earlier one.
    pub fn set_invite_reply(&self, uid: &str, partstat: &str, now: Timestamp) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO invite_replies (uid, partstat, replied_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (uid) DO UPDATE SET partstat = excluded.partstat, \
                 replied_at = excluded.replied_at",
                params![uid, partstat, now.millis()],
            )
            .map(|_| ())
            .map_err(err("réponse à une invitation"))
        })
    }
}

/// An address a mailbox sends as, besides its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub id: i64,
    pub account: AccountId,
    pub address: String,
    pub name: String,
}

impl Store {
    /// The addresses every mailbox sends as besides its own, by mailbox then address.
    pub fn aliases(&self) -> Result<Vec<Alias>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, account_id, address, name FROM account_aliases \
                     ORDER BY account_id, address",
                )
                .map_err(err("alias"))?;
            let lignes = stmt
                .query_map([], |r| {
                    Ok(Alias {
                        id: r.get(0)?,
                        account: AccountId(r.get(1)?),
                        address: r.get(2)?,
                        name: r.get(3)?,
                    })
                })
                .map_err(err("alias"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("alias"));
            lignes
        })
    }

    /// A new address for a mailbox to send as; nothing when it has it already.
    pub fn add_alias(&self, account: AccountId, address: &str, name: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT OR IGNORE INTO account_aliases (account_id, address, name) \
                 VALUES (?1, ?2, ?3)",
                params![account.get(), address.trim().to_lowercase(), name.trim()],
            )
            .map(|_| ())
            .map_err(err("alias"))
        })
    }

    pub fn remove_alias(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM account_aliases WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("alias"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewAccount;

    #[test]
    fn a_mailbox_sends_as_its_aliases_once_each() {
        let s = Store::in_memory().unwrap();
        let t = Timestamp::from_millis(0);
        let compte = s
            .create_account(&NewAccount::new("a@example.com", "i", "s"), t)
            .unwrap();
        s.add_alias(compte, " Support@Example.com ", "Support")
            .unwrap();
        s.add_alias(compte, "support@example.com", "").unwrap();
        let alias = s.aliases().unwrap();
        assert_eq!(alias.len(), 1, "once");
        assert_eq!(
            (alias[0].address.as_str(), alias[0].name.as_str()),
            ("support@example.com", "Support")
        );
        s.remove_alias(alias[0].id).unwrap();
        assert!(s.aliases().unwrap().is_empty());
    }

    #[test]
    fn a_message_waits_for_its_time_then_leaves_the_list() {
        let s = Store::in_memory().unwrap();
        let t = |ms: i64| Timestamp::from_millis(ms);
        let compte = s
            .create_account(&NewAccount::new("a@example.com", "i", "s"), t(0))
            .unwrap();
        let tard = s
            .schedule_mail(compte, t(9_000), "b@example.com", "Later", "{}", t(1))
            .unwrap();
        let tot = s
            .schedule_mail(compte, t(5_000), "c@example.com", "Sooner", "{}", t(1))
            .unwrap();
        let tous = s.scheduled_mail().unwrap();
        assert_eq!(
            tous.iter().map(|m| m.id).collect::<Vec<_>>(),
            [tot, tard],
            "the soonest first"
        );
        assert!(s.due_scheduled_mail(t(4_999)).unwrap().is_empty());
        assert_eq!(s.due_scheduled_mail(t(5_000)).unwrap()[0].subject, "Sooner");
        assert!(s.unschedule_mail(tot).unwrap());
        assert!(!s.unschedule_mail(tot).unwrap(), "only once");
        assert_eq!(s.scheduled_mail().unwrap().len(), 1);
    }

    #[test]
    fn a_message_that_did_not_leave_waits_again() {
        let s = Store::in_memory().unwrap();
        let t = |ms: i64| Timestamp::from_millis(ms);
        let compte = s
            .create_account(&NewAccount::new("a@example.com", "i", "s"), t(0))
            .unwrap();
        let id = s
            .schedule_mail(compte, t(5_000), "b@example.com", "Devis", "{}", t(1))
            .unwrap();
        assert!(s.postpone_scheduled_mail(id, t(8_000)).unwrap());
        assert!(s.due_scheduled_mail(t(5_000)).unwrap().is_empty());
        assert_eq!(s.due_scheduled_mail(t(8_000)).unwrap()[0].id, id);
    }
}
