//! Calendars kept with a server (CalDAV): the accounts, and what a sync reads and
//! writes.
//!
//! The store keeps the server's side of each object — its address, its tag, its text
//! as last read — beside the events Iris shows, and what was changed or deleted here
//! and not sent yet. It talks to no server: `iris-app` does, through `iris-caldav`.

use crate::calendar::{evenement, inserer, NewEvent, StoredEvent, COLONNES_EV};
use crate::Store;
use iris_types::{Error, Result, Timestamp};
use rusqlite::{params, OptionalExtension, Row};
use std::collections::HashMap;

/// A server calendars are kept with, and who signs in there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarAccount {
    pub id: i64,
    pub name: String,
    /// What was typed, made an address: where to start looking.
    pub server: String,
    /// Where its calendars are, once found.
    pub home: Option<String>,
    pub username: String,
    /// The mailbox whose sign-in it uses (Google), instead of a password of its own.
    pub mail_account: Option<i64>,
    pub last_sync: Option<Timestamp>,
    pub last_error: Option<String>,
}

/// One object to send: an event (and its changed occurrences, under one UID) changed
/// here, with what the server had of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingObject {
    pub calendar_id: i64,
    pub uid: String,
    /// Its address there, `None` when it is new.
    pub href: Option<String>,
    pub etag: Option<String>,
    /// The object as last read, to send back what Iris does not keep.
    pub remote_ics: Option<String>,
    /// The event first, then its changed occurrences.
    pub events: Vec<StoredEvent>,
}

/// What is stored of an object of the server: its tag, and whether it was changed here
/// since.
pub type ObjectState = (Option<String>, bool);

/// An object deleted here, to delete there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    pub id: i64,
    pub calendar_id: i64,
    pub href: String,
    pub etag: Option<String>,
}

fn err(quoi: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::store(format!("{quoi} : {e}"))
}

const COLONNES_COMPTE: &str =
    "id, name, server, home, username, mail_account, last_sync, last_error";

fn compte(r: &Row<'_>) -> rusqlite::Result<CalendarAccount> {
    Ok(CalendarAccount {
        id: r.get(0)?,
        name: r.get(1)?,
        server: r.get(2)?,
        home: r.get(3)?,
        username: r.get(4)?,
        mail_account: r.get(5)?,
        last_sync: r.get::<_, Option<i64>>(6)?.map(Timestamp::from_millis),
        last_error: r.get(7)?,
    })
}

impl Store {
    pub fn create_calendar_account(
        &self,
        name: &str,
        server: &str,
        username: &str,
        mail_account: Option<i64>,
        now: Timestamp,
    ) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO calendar_accounts (name, server, username, mail_account, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![name, server, username, mail_account, now.millis()],
            )
            .map_err(err("compte d'agenda"))?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn calendar_accounts(&self) -> Result<Vec<CalendarAccount>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES_COMPTE} FROM calendar_accounts ORDER BY id"
                ))
                .map_err(err("comptes d'agenda"))?;
            let lignes = stmt
                .query_map([], compte)
                .map_err(err("comptes d'agenda"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("comptes d'agenda"));
            lignes
        })
    }

    pub fn calendar_account(&self, id: i64) -> Result<Option<CalendarAccount>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("SELECT {COLONNES_COMPTE} FROM calendar_accounts WHERE id = ?1"),
                [id],
                compte,
            )
            .optional()
            .map_err(err("compte d'agenda"))
        })
    }

    pub fn set_calendar_account_home(&self, id: i64, home: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE calendar_accounts SET home = ?2 WHERE id = ?1",
                params![id, home],
            )
            .map(|_| ())
            .map_err(err("compte d'agenda"))
        })
    }

    /// The outcome of a sync: when, and what went wrong if anything did.
    pub fn set_calendar_account_sync(
        &self,
        id: i64,
        at: Timestamp,
        error: Option<&str>,
    ) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE calendar_accounts SET last_sync = ?2, last_error = ?3 WHERE id = ?1",
                params![id, at.millis(), error],
            )
            .map(|_| ())
            .map_err(err("compte d'agenda"))
        })
    }

    /// Removes an account, its calendars and their events from this computer. The
    /// server keeps them.
    pub fn delete_calendar_account(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM calendar_accounts WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("compte d'agenda"))
        })
    }

    /// A calendar of the account, found at `url`: made the first time (with the
    /// server's name and colour), kept after (renamed or recoloured here, it stays
    /// so). Gives its id.
    pub fn upsert_remote_calendar(
        &self,
        account: i64,
        url: &str,
        name: &str,
        color: &str,
        read_only: bool,
        now: Timestamp,
    ) -> Result<i64> {
        self.with_conn(|c| {
            let existant: Option<i64> = c
                .query_row(
                    "SELECT id FROM calendars WHERE account_id = ?1 AND remote_href = ?2",
                    params![account, url],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err("calendrier distant"))?;
            if let Some(id) = existant {
                c.execute(
                    "UPDATE calendars SET read_only = ?2 WHERE id = ?1",
                    params![id, read_only as i64],
                )
                .map_err(err("calendrier distant"))?;
                return Ok(id);
            }
            c.execute(
                "INSERT INTO calendars (name, color, account_id, remote_href, read_only, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![name, color, account, url, read_only as i64, now.millis()],
            )
            .map_err(err("calendrier distant"))?;
            Ok(c.last_insert_rowid())
        })
    }

    /// The account's calendars no longer on its server leave this computer too.
    pub fn remove_remote_calendars_except(&self, account: i64, urls: &[String]) -> Result<usize> {
        let tous: Vec<(i64, String)> = self.with_conn(|c| {
            let mut stmt = c
                .prepare("SELECT id, remote_href FROM calendars WHERE account_id = ?1")
                .map_err(err("calendriers distants"))?;
            let lignes = stmt
                .query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(err("calendriers distants"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("calendriers distants"));
            lignes
        })?;
        let mut partis = 0;
        for (id, url) in tous {
            if !urls.contains(&url) {
                self.delete_calendar(id)?;
                partis += 1;
            }
        }
        Ok(partis)
    }

    /// The tag the server gave the whole calendar when it was last read.
    pub fn set_remote_ctag(&self, calendar: i64, ctag: Option<&str>) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE calendars SET etag = ?2 WHERE id = ?1",
                params![calendar, ctag],
            )
            .map(|_| ())
            .map_err(err("calendrier distant"))
        })
    }

    /// Every object stored for a calendar: its tag, and whether it was changed here
    /// since.
    pub fn remote_objects(&self, calendar: i64) -> Result<HashMap<String, ObjectState>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT href, max(etag), max(dirty) FROM calendar_events \
                     WHERE calendar_id = ?1 AND href IS NOT NULL GROUP BY href",
                )
                .map_err(err("objets distants"))?;
            let lignes = stmt
                .query_map([calendar], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        (r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)? != 0),
                    ))
                })
                .map_err(err("objets distants"))?
                .collect::<rusqlite::Result<HashMap<_, _>>>()
                .map_err(err("objets distants"));
            lignes
        })
    }

    /// The server's version of an object, in place of what was stored of it: its
    /// events, with its address, tag and text. Nothing of it is to be sent.
    pub fn store_remote_object(
        &self,
        calendar: i64,
        href: &str,
        etag: Option<&str>,
        ics: &str,
        events: &[NewEvent],
        now: Timestamp,
    ) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM calendar_events WHERE calendar_id = ?1 AND href = ?2",
                params![calendar, href],
            )
            .map_err(err("objet distant"))?;
            for e in events {
                // Its UID, made here and sent with no answer read yet: the same event.
                tx.execute(
                    "DELETE FROM calendar_events WHERE calendar_id = ?1 AND uid = ?2 \
                     AND href IS NULL AND recurrence_id IS ?3",
                    params![calendar, e.uid, e.recurrence_id],
                )
                .map_err(err("objet distant"))?;
                let id = inserer(tx, calendar, e, now)?;
                tx.execute(
                    "UPDATE calendar_events SET href = ?2, etag = ?3, remote_ics = ?4, dirty = 0 \
                     WHERE id = ?1",
                    params![id, href, etag, ics],
                )
                .map_err(err("objet distant"))?;
            }
            Ok(())
        })
    }

    /// An object gone from the server leaves this computer too.
    pub fn remove_remote_object(&self, calendar: i64, href: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM calendar_events WHERE calendar_id = ?1 AND href = ?2",
                params![calendar, href],
            )
            .map(|_| ())
            .map_err(err("objet distant"))
        })
    }

    /// The objects of a calendar changed here and not sent yet.
    pub fn outgoing_objects(&self, calendar: i64) -> Result<Vec<OutgoingObject>> {
        self.with_conn(|c| {
            let uids: Vec<String> = {
                let mut stmt = c
                    .prepare(
                        "SELECT DISTINCT uid FROM calendar_events \
                         WHERE calendar_id = ?1 AND dirty = 1",
                    )
                    .map_err(err("objets à envoyer"))?;
                let lignes = stmt
                    .query_map([calendar], |r| r.get(0))
                    .map_err(err("objets à envoyer"))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(err("objets à envoyer"))?;
                lignes
            };
            let mut sortie = Vec::new();
            for uid in uids {
                let mut stmt = c
                    .prepare(&format!(
                        "SELECT {COLONNES_EV}, href, etag, remote_ics FROM calendar_events \
                         WHERE calendar_id = ?1 AND uid = ?2 \
                         ORDER BY recurrence_id IS NOT NULL, recurrence_id"
                    ))
                    .map_err(err("objets à envoyer"))?;
                let lignes = stmt
                    .query_map(params![calendar, uid], |r| {
                        Ok((
                            evenement(r)?,
                            r.get::<_, Option<String>>(15)?,
                            r.get::<_, Option<String>>(16)?,
                            r.get::<_, Option<String>>(17)?,
                        ))
                    })
                    .map_err(err("objets à envoyer"))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(err("objets à envoyer"))?;
                let serveur = lignes.iter().find(|l| l.1.is_some());
                sortie.push(OutgoingObject {
                    calendar_id: calendar,
                    uid: uid.clone(),
                    href: serveur.and_then(|l| l.1.clone()),
                    etag: serveur.and_then(|l| l.2.clone()),
                    remote_ics: serveur.and_then(|l| l.3.clone()),
                    events: lignes.into_iter().map(|l| l.0).collect(),
                });
            }
            Ok(sortie)
        })
    }

    /// An object sent: its events now at `href`, tagged `etag`, as `ics`.
    pub fn mark_object_sent(
        &self,
        calendar: i64,
        uid: &str,
        href: &str,
        etag: Option<&str>,
        ics: &str,
    ) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE calendar_events SET href = ?3, etag = ?4, remote_ics = ?5, dirty = 0 \
                 WHERE calendar_id = ?1 AND uid = ?2",
                params![calendar, uid, href, etag, ics],
            )
            .map(|_| ())
            .map_err(err("objet envoyé"))
        })
    }

    /// The objects of a calendar deleted here, to delete there.
    pub fn tombstones(&self, calendar: i64) -> Result<Vec<Tombstone>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, calendar_id, href, etag FROM calendar_tombstones \
                     WHERE calendar_id = ?1 ORDER BY id",
                )
                .map_err(err("suppressions à envoyer"))?;
            let lignes = stmt
                .query_map([calendar], |r| {
                    Ok(Tombstone {
                        id: r.get(0)?,
                        calendar_id: r.get(1)?,
                        href: r.get(2)?,
                        etag: r.get(3)?,
                    })
                })
                .map_err(err("suppressions à envoyer"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("suppressions à envoyer"));
            lignes
        })
    }

    pub fn clear_tombstone(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM calendar_tombstones WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppressions à envoyer"))
        })
    }

    /// The calendar accounts with something changed here and not sent yet.
    pub fn calendar_accounts_with_changes(&self) -> Result<Vec<i64>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT DISTINCT k.account_id FROM calendars k WHERE k.account_id IS NOT NULL \
                     AND (EXISTS (SELECT 1 FROM calendar_events e \
                                  WHERE e.calendar_id = k.id AND e.dirty = 1) \
                          OR EXISTS (SELECT 1 FROM calendar_tombstones t \
                                     WHERE t.calendar_id = k.id))",
                )
                .map_err(err("changements à envoyer"))?;
            let lignes = stmt
                .query_map([], |r| r.get(0))
                .map_err(err("changements à envoyer"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("changements à envoyer"));
            lignes
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(uid: &str, titre: &str) -> NewEvent {
        NewEvent {
            uid: uid.into(),
            summary: titre.into(),
            start_ms: 1_000,
            end_ms: 2_000,
            ..Default::default()
        }
    }

    #[test]
    fn changes_here_are_sent_and_the_servers_are_not() {
        let s = Store::in_memory().unwrap();
        let t = Timestamp::from_millis;
        let compte = s
            .create_calendar_account(
                "Fastmail",
                "https://dav.example.com/",
                "a@example.com",
                None,
                t(0),
            )
            .unwrap();
        let cal = s
            .upsert_remote_calendar(
                compte,
                "https://dav.example.com/cal/",
                "Work",
                "#3a87ad",
                false,
                t(0),
            )
            .unwrap();
        assert_eq!(
            s.upsert_remote_calendar(
                compte,
                "https://dav.example.com/cal/",
                "Other",
                "#000000",
                true,
                t(1)
            )
            .unwrap(),
            cal,
            "found again, not made twice"
        );
        assert!(s.calendar(cal).unwrap().unwrap().read_only);

        // From the server: nothing to send.
        s.store_remote_object(
            cal,
            "/cal/a.ics",
            Some("\"1\""),
            "BEGIN:VCALENDAR",
            &[ev("a", "A")],
            t(2),
        )
        .unwrap();
        assert!(s.outgoing_objects(cal).unwrap().is_empty());
        assert_eq!(
            s.remote_objects(cal).unwrap()["/cal/a.ics"],
            (Some("\"1\"".to_string()), false)
        );

        // Made here: to send, with no address yet.
        s.insert_event(cal, &ev("b", "B"), t(3)).unwrap();
        let sortants = s.outgoing_objects(cal).unwrap();
        assert_eq!(sortants.len(), 1);
        assert_eq!(
            (sortants[0].uid.as_str(), sortants[0].href.as_deref()),
            ("b", None)
        );
        s.mark_object_sent(cal, "b", "/cal/b.ics", Some("\"9\""), "X")
            .unwrap();
        assert!(s.outgoing_objects(cal).unwrap().is_empty());
        assert_eq!(
            s.calendar_accounts_with_changes().unwrap(),
            Vec::<i64>::new()
        );

        // Changed here: to send, with what the server had.
        let a = s.find_event(cal, "a", None).unwrap().unwrap();
        s.update_event(a, cal, &ev("a", "A changed"), t(4)).unwrap();
        let sortants = s.outgoing_objects(cal).unwrap();
        assert_eq!(sortants[0].href.as_deref(), Some("/cal/a.ics"));
        assert_eq!(sortants[0].remote_ics.as_deref(), Some("BEGIN:VCALENDAR"));
        assert_eq!(s.calendar_accounts_with_changes().unwrap(), vec![compte]);

        // Deleted here: deleted there.
        let b = s.find_event(cal, "b", None).unwrap().unwrap();
        s.delete_event(b).unwrap();
        let tombes = s.tombstones(cal).unwrap();
        assert_eq!(tombes[0].href, "/cal/b.ics");
        assert_eq!(tombes[0].etag.as_deref(), Some("\"9\""));
        s.clear_tombstone(tombes[0].id).unwrap();

        // An account removed takes its calendars with it.
        s.delete_calendar_account(compte).unwrap();
        assert!(s.calendar(cal).unwrap().is_none());
    }

    #[test]
    fn a_local_calendar_has_nothing_to_send() {
        let s = Store::in_memory().unwrap();
        let local = s.calendars().unwrap()[0].id;
        s.insert_event(local, &ev("x", "X"), Timestamp::from_millis(0))
            .unwrap();
        assert!(s.outgoing_objects(local).unwrap().is_empty());
        let x = s.find_event(local, "x", None).unwrap().unwrap();
        s.delete_event(x).unwrap();
        assert!(s.tombstones(local).unwrap().is_empty());
    }
}
