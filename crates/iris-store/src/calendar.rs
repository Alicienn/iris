//! L'agenda : calendriers et événements.

use crate::Store;
use iris_types::{Error, Result, Timestamp};
use rusqlite::{params, OptionalExtension, Row};

/// Un calendrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCalendar {
    pub id: i64,
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    /// L'adresse d'un abonnement ; `None` pour un calendrier local.
    pub source_url: Option<String>,
    pub visible: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_sync: Option<Timestamp>,
    pub last_error: Option<String>,
}

impl StoredCalendar {
    pub fn is_subscription(&self) -> bool {
        self.source_url.is_some()
    }
}

/// Un événement, tel qu'il s'écrit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewEvent {
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub all_day: bool,
    pub tzid: Option<String>,
    pub rrule: Option<String>,
    pub exdates: Vec<i64>,
    pub recurrence_id: Option<i64>,
    pub cancelled: bool,
    pub reminder_minutes: Option<i32>,
}

/// Un événement, tel qu'il se relit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub id: i64,
    pub calendar_id: i64,
    pub event: NewEvent,
}

const COLONNES_CAL: &str =
    "id, name, color, source_url, visible, etag, last_modified, last_sync, last_error";

fn calendrier(r: &Row<'_>) -> rusqlite::Result<StoredCalendar> {
    Ok(StoredCalendar {
        id: r.get(0)?,
        name: r.get(1)?,
        color: r.get(2)?,
        source_url: r.get(3)?,
        visible: r.get::<_, i64>(4)? != 0,
        etag: r.get(5)?,
        last_modified: r.get(6)?,
        last_sync: r.get::<_, Option<i64>>(7)?.map(Timestamp::from_millis),
        last_error: r.get(8)?,
    })
}

const COLONNES_EV: &str =
    "id, calendar_id, uid, summary, description, location, start_ms, end_ms, \
     all_day, tzid, rrule, exdates, recurrence_id, cancelled, reminder_minutes";

fn evenement(r: &Row<'_>) -> rusqlite::Result<StoredEvent> {
    let exdates: String = r.get(11)?;
    Ok(StoredEvent {
        id: r.get(0)?,
        calendar_id: r.get(1)?,
        event: NewEvent {
            uid: r.get(2)?,
            summary: r.get(3)?,
            description: r.get(4)?,
            location: r.get(5)?,
            start_ms: r.get(6)?,
            end_ms: r.get(7)?,
            all_day: r.get::<_, i64>(8)? != 0,
            tzid: r.get(9)?,
            rrule: r.get(10)?,
            exdates: exdates
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect(),
            recurrence_id: r.get(12)?,
            cancelled: r.get::<_, i64>(13)? != 0,
            reminder_minutes: r.get(14)?,
        },
    })
}

fn err(quoi: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::store(format!("{quoi} : {e}"))
}

fn inserer(tx: &rusqlite::Connection, calendar: i64, e: &NewEvent, now: Timestamp) -> Result<i64> {
    let exdates = e
        .exdates
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    tx.execute(
        "INSERT INTO calendar_events (calendar_id, uid, summary, description, location, start_ms, \
         end_ms, all_day, tzid, rrule, exdates, recurrence_id, cancelled, reminder_minutes, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            calendar,
            e.uid,
            e.summary,
            e.description,
            e.location,
            e.start_ms,
            e.end_ms,
            e.all_day as i64,
            e.tzid,
            e.rrule,
            exdates,
            e.recurrence_id,
            e.cancelled as i64,
            e.reminder_minutes,
            now.millis()
        ],
    )
    .map_err(err("écriture d'un événement"))?;
    Ok(tx.last_insert_rowid())
}

impl Store {
    /// Tous les calendriers, locaux d'abord, puis par nom.
    pub fn calendars(&self) -> Result<Vec<StoredCalendar>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES_CAL} FROM calendars \
                     ORDER BY source_url IS NOT NULL, name COLLATE NOCASE"
                ))
                .map_err(err("lecture des calendriers"))?;
            let lignes = stmt
                .query_map([], calendrier)
                .map_err(err("lecture des calendriers"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des calendriers"));
            lignes
        })
    }

    pub fn calendar(&self, id: i64) -> Result<Option<StoredCalendar>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("SELECT {COLONNES_CAL} FROM calendars WHERE id = ?1"),
                [id],
                calendrier,
            )
            .optional()
            .map_err(err("lecture d'un calendrier"))
        })
    }

    pub fn create_calendar(
        &self,
        name: &str,
        color: &str,
        source_url: Option<&str>,
        now: Timestamp,
    ) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO calendars (name, color, source_url, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![name, color, source_url, now.millis()],
            )
            .map_err(err("création d'un calendrier"))?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn update_calendar(&self, id: i64, name: &str, color: &str, visible: bool) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE calendars SET name = ?2, color = ?3, visible = ?4 WHERE id = ?1",
                params![id, name, color, visible as i64],
            )
            .map(|_| ())
            .map_err(err("mise à jour d'un calendrier"))
        })
    }

    /// Note le résultat d'une relecture d'abonnement.
    pub fn set_calendar_sync(
        &self,
        id: i64,
        etag: Option<&str>,
        last_modified: Option<&str>,
        at: Timestamp,
        error: Option<&str>,
    ) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE calendars SET etag = COALESCE(?2, etag), \
                 last_modified = COALESCE(?3, last_modified), last_sync = ?4, last_error = ?5 \
                 WHERE id = ?1",
                params![id, etag, last_modified, at.millis(), error],
            )
            .map(|_| ())
            .map_err(err("état d'un abonnement"))
        })
    }

    /// Supprime un calendrier et tous ses événements.
    pub fn delete_calendar(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM calendars WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'un calendrier"))
        })
    }

    /// Remplace tous les événements d'un calendrier, d'un bloc.
    ///
    /// C'est la relecture d'un abonnement : le fichier publié est la vérité entière, et
    /// comparer ligne à ligne ne dirait rien de plus que le fichier lui-même. Dans une
    /// transaction, pour qu'un agenda à moitié remplacé ne s'affiche jamais.
    pub fn replace_calendar_events(
        &self,
        calendar: i64,
        events: &[NewEvent],
        now: Timestamp,
    ) -> Result<usize> {
        self.with_tx(|tx| {
            tx.execute(
                "DELETE FROM calendar_events WHERE calendar_id = ?1",
                [calendar],
            )
            .map_err(err("vidage d'un calendrier"))?;
            for e in events {
                inserer(tx, calendar, e, now)?;
            }
            Ok(events.len())
        })
    }

    pub fn insert_event(&self, calendar: i64, event: &NewEvent, now: Timestamp) -> Result<i64> {
        self.with_conn(|c| inserer(c, calendar, event, now))
    }

    pub fn update_event(&self, id: i64, calendar: i64, e: &NewEvent, now: Timestamp) -> Result<()> {
        let exdates = e
            .exdates
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        self.with_conn(|c| {
            c.execute(
                "UPDATE calendar_events SET calendar_id = ?2, summary = ?3, description = ?4, \
                 location = ?5, start_ms = ?6, end_ms = ?7, all_day = ?8, tzid = ?9, rrule = ?10, \
                 exdates = ?11, reminder_minutes = ?12, updated_at = ?13 WHERE id = ?1",
                params![
                    id,
                    calendar,
                    e.summary,
                    e.description,
                    e.location,
                    e.start_ms,
                    e.end_ms,
                    e.all_day as i64,
                    e.tzid,
                    e.rrule,
                    exdates,
                    e.reminder_minutes,
                    now.millis()
                ],
            )
            .map(|_| ())
            .map_err(err("mise à jour d'un événement"))
        })
    }

    pub fn delete_event(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM calendar_events WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'un événement"))
        })
    }

    pub fn event(&self, id: i64) -> Result<Option<StoredEvent>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("SELECT {COLONNES_EV} FROM calendar_events WHERE id = ?1"),
                [id],
                evenement,
            )
            .optional()
            .map_err(err("lecture d'un événement"))
        })
    }

    /// Ce qu'il faut pour afficher `[from, to[` : les événements simples qui la
    /// touchent, toutes les règles qui commencent avant sa fin, et toutes les
    /// occurrences modifiées — elles effacent celle qu'elles remplacent, où qu'elle soit.
    /// Seulement pour les calendriers visibles.
    pub fn events_for_range(&self, from: i64, to: i64) -> Result<Vec<StoredEvent>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {} FROM calendar_events e JOIN calendars k ON k.id = e.calendar_id \
                     WHERE k.visible = 1 AND ( \
                        (e.rrule IS NULL AND e.start_ms < ?2 AND e.end_ms >= ?1) \
                        OR (e.rrule IS NOT NULL AND e.start_ms < ?2) \
                        OR e.recurrence_id IS NOT NULL) \
                     ORDER BY e.start_ms",
                    COLONNES_EV
                        .split(", ")
                        .map(|c| format!("e.{c}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .map_err(err("lecture de l'agenda"))?;
            let lignes = stmt
                .query_map(params![from, to], evenement)
                .map_err(err("lecture de l'agenda"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture de l'agenda"));
            lignes
        })
    }

    /// L'événement d'un calendrier qui porte cet identifiant — et cette occurrence
    /// remplacée, le cas échéant. C'est ce qui fait qu'une invitation mise à jour
    /// remplace la précédente au lieu de s'y ajouter.
    pub fn find_event(
        &self,
        calendar: i64,
        uid: &str,
        recurrence_id: Option<i64>,
    ) -> Result<Option<i64>> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT id FROM calendar_events WHERE calendar_id = ?1 AND uid = ?2                  AND recurrence_id IS ?3",
                params![calendar, uid, recurrence_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err("recherche d'un événement"))
        })
    }

    /// Marque un événement annulé, ou le rétablit.
    pub fn set_event_cancelled(&self, id: i64, cancelled: bool) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE calendar_events SET cancelled = ?2 WHERE id = ?1",
                params![id, cancelled as i64],
            )
            .map(|_| ())
            .map_err(err("annulation d'un événement"))
        })
    }

    /// Combien d'événements porte un calendrier.
    pub fn calendar_event_count(&self, calendar: i64) -> Result<u32> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM calendar_events WHERE calendar_id = ?1",
                [calendar],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u32)
            .map_err(err("compte d'un calendrier"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(uid: &str, start: i64, end: i64) -> NewEvent {
        NewEvent {
            uid: uid.into(),
            summary: uid.into(),
            start_ms: start,
            end_ms: end,
            ..Default::default()
        }
    }

    #[test]
    fn un_calendrier_local_existe_d_emblee() {
        let s = Store::in_memory().unwrap();
        let c = s.calendars().unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "Personal");
        assert!(!c[0].is_subscription());
    }

    #[test]
    fn un_evenement_s_ecrit_se_relit_et_se_supprime() {
        let s = Store::in_memory().unwrap();
        let cal = s.calendars().unwrap()[0].id;
        let mut e = ev("a", 1_000, 2_000);
        e.exdates = vec![5, 6];
        e.reminder_minutes = Some(10);
        let id = s.insert_event(cal, &e, Timestamp::EPOCH).unwrap();
        assert_eq!(s.event(id).unwrap().unwrap().event, e);
        s.delete_event(id).unwrap();
        assert!(s.event(id).unwrap().is_none());
    }

    #[test]
    fn la_periode_ramene_ce_qui_la_touche_et_les_regles() {
        let s = Store::in_memory().unwrap();
        let cal = s.calendars().unwrap()[0].id;
        s.insert_event(cal, &ev("dedans", 150, 160), Timestamp::EPOCH)
            .unwrap();
        s.insert_event(cal, &ev("avant", 10, 20), Timestamp::EPOCH)
            .unwrap();
        s.insert_event(cal, &ev("apres", 500, 600), Timestamp::EPOCH)
            .unwrap();
        let mut regle = ev("regle", 0, 5);
        regle.rrule = Some("FREQ=DAILY".into());
        s.insert_event(cal, &regle, Timestamp::EPOCH).unwrap();

        let uids: Vec<String> = s
            .events_for_range(100, 200)
            .unwrap()
            .into_iter()
            .map(|e| e.event.uid)
            .collect();
        assert_eq!(uids, vec!["regle", "dedans"]);
    }

    #[test]
    fn un_calendrier_masque_ne_ramene_rien() {
        let s = Store::in_memory().unwrap();
        let cal = s.calendars().unwrap()[0].id;
        s.insert_event(cal, &ev("a", 150, 160), Timestamp::EPOCH)
            .unwrap();
        s.update_calendar(cal, "Personal", "#000000", false)
            .unwrap();
        assert!(s.events_for_range(100, 200).unwrap().is_empty());
    }

    #[test]
    fn un_abonnement_est_remplace_en_bloc_et_supprime_avec_ses_evenements() {
        let s = Store::in_memory().unwrap();
        let cal = s
            .create_calendar(
                "Fériés",
                "#aa3355",
                Some("https://example.com/f.ics"),
                Timestamp::EPOCH,
            )
            .unwrap();
        s.replace_calendar_events(cal, &[ev("a", 1, 2), ev("b", 3, 4)], Timestamp::EPOCH)
            .unwrap();
        s.replace_calendar_events(cal, &[ev("c", 5, 6)], Timestamp::EPOCH)
            .unwrap();
        assert_eq!(s.calendar_event_count(cal).unwrap(), 1);

        s.set_calendar_sync(cal, Some("\"v1\""), None, Timestamp::from_millis(9), None)
            .unwrap();
        let c = s.calendar(cal).unwrap().unwrap();
        assert_eq!(c.etag.as_deref(), Some("\"v1\""));
        assert!(c.is_subscription());

        s.delete_calendar(cal).unwrap();
        assert_eq!(s.calendar_event_count(cal).unwrap(), 0);
    }
}
