//! Writing iCalendar (RFC 5545): a calendar's events, and tasks, as other applications
//! read them.
//!
//! What Iris keeps is what is written: the time, the rule and its exceptions, the
//! changed occurrences, the place, the first reminder. A timed event in a named zone
//! keeps it (`TZID`), so a weekly meeting stays at ten across a change of hour; the
//! zone is named, not described (`VTIMEZONE`), as every calendar in use reads the IANA
//! names. Anything else is in UTC, or a bare date for a day.

use crate::time::resolve_tz;
use crate::Event;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};

/// A task, as a `VTODO`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Todo {
    pub uid: String,
    pub summary: String,
    pub description: String,
    /// The day it is due, and the minute of that day when it has an hour (local time).
    pub due: Option<(NaiveDate, Option<u32>)>,
    /// When it was done, in UTC milliseconds.
    pub completed: Option<i64>,
    /// 0 none, 1 low, 2 medium, 3 high.
    pub priority: u8,
    /// Its list.
    pub category: String,
    /// The task it is a step of.
    pub parent_uid: Option<String>,
    /// Iris's word for how it repeats (`daily`, `weekdays`, `weekly`…).
    pub repeat: Option<String>,
    pub created: i64,
}

/// A whole calendar named `name`, with its events (repeating ones, their exceptions
/// and their changed occurrences included).
pub fn calendar(name: &str, events: &[Event], stamp_ms: i64) -> String {
    let mut lignes = entete(name);
    for e in events {
        lignes.extend(vevent(e, stamp_ms));
    }
    lignes.push("END:VCALENDAR".into());
    finir(lignes)
}

/// One event alone, as one object of a calendar server holds it: the event and, under
/// the same UID, its changed occurrences.
pub fn object(events: &[&Event], stamp_ms: i64) -> String {
    let mut lignes = vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//Iris//Iris Mail//EN".into(),
    ];
    for e in events {
        lignes.extend(vevent(e, stamp_ms));
    }
    lignes.push("END:VCALENDAR".into());
    finir(lignes)
}

/// Tasks, as a list named `name`.
pub fn todos(name: &str, todos: &[Todo], stamp_ms: i64) -> String {
    let mut lignes = entete(name);
    for t in todos {
        lignes.push("BEGIN:VTODO".into());
        lignes.push(format!("UID:{}", texte(&t.uid)));
        lignes.push(format!("DTSTAMP:{}", utc(stamp_ms)));
        lignes.push(format!("CREATED:{}", utc(t.created)));
        lignes.push(format!("SUMMARY:{}", texte(&t.summary)));
        if !t.description.trim().is_empty() {
            lignes.push(format!("DESCRIPTION:{}", texte(&t.description)));
        }
        match t.due {
            Some((jour, None)) => lignes.push(format!("DUE;VALUE=DATE:{}", jour.format("%Y%m%d"))),
            Some((jour, Some(minute))) => lignes.push(format!(
                "DUE:{}T{:02}{:02}00",
                jour.format("%Y%m%d"),
                minute / 60,
                minute % 60
            )),
            None => {}
        }
        match t.completed {
            Some(quand) => {
                lignes.push("STATUS:COMPLETED".into());
                lignes.push(format!("COMPLETED:{}", utc(quand)));
            }
            None => lignes.push("STATUS:NEEDS-ACTION".into()),
        }
        // 1 is the highest in the format, 9 the lowest.
        match t.priority {
            3 => lignes.push("PRIORITY:1".into()),
            2 => lignes.push("PRIORITY:5".into()),
            1 => lignes.push("PRIORITY:9".into()),
            _ => {}
        }
        if !t.category.trim().is_empty() {
            lignes.push(format!("CATEGORIES:{}", texte(&t.category)));
        }
        if let Some(parent) = &t.parent_uid {
            lignes.push(format!("RELATED-TO:{}", texte(parent)));
        }
        if let Some(regle) = t.repeat.as_deref().and_then(rrule_of_repeat) {
            lignes.push(format!("RRULE:{regle}"));
        }
        lignes.push("END:VTODO".into());
    }
    lignes.push("END:VCALENDAR".into());
    finir(lignes)
}

/// The rule of one of Iris's ways to repeat.
pub fn rrule_of_repeat(word: &str) -> Option<&'static str> {
    Some(match word {
        "daily" => "FREQ=DAILY",
        "weekdays" => "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
        "weekly" => "FREQ=WEEKLY",
        "monthly" => "FREQ=MONTHLY",
        "yearly" => "FREQ=YEARLY",
        _ => return None,
    })
}

fn entete(name: &str) -> Vec<String> {
    vec![
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".into(),
        "PRODID:-//Iris//Iris Mail//EN".into(),
        "CALSCALE:GREGORIAN".into(),
        format!("X-WR-CALNAME:{}", texte(name)),
    ]
}

fn vevent(e: &Event, stamp_ms: i64) -> Vec<String> {
    let zone = e.tzid.as_deref().and_then(resolve_tz);
    // How a moment of this event is written, after the property's name.
    let moment = |ms: i64| -> String {
        if e.all_day {
            let jour = DateTime::<Utc>::from_timestamp_millis(ms)
                .map(|t| t.date_naive())
                .unwrap_or_default();
            format!(";VALUE=DATE:{}", jour.format("%Y%m%d"))
        } else if let Some(tz) = zone {
            let local = tz.timestamp_millis_opt(ms).single();
            match local {
                Some(t) => format!(";TZID={}:{}", tz.name(), t.format("%Y%m%dT%H%M%S")),
                None => format!(":{}", utc(ms)),
            }
        } else {
            format!(":{}", utc(ms))
        }
    };

    let mut l = vec!["BEGIN:VEVENT".to_string(), format!("UID:{}", texte(&e.uid))];
    l.push(format!("DTSTAMP:{}", utc(stamp_ms)));
    if let Some(id) = e.recurrence_id {
        l.push(format!("RECURRENCE-ID{}", moment(id)));
    }
    l.push(format!("DTSTART{}", moment(e.start)));
    if e.end > e.start {
        l.push(format!("DTEND{}", moment(e.end)));
    }
    l.push(format!("SUMMARY:{}", texte(&e.summary)));
    if !e.description.trim().is_empty() {
        l.push(format!("DESCRIPTION:{}", texte(&e.description)));
    }
    if !e.location.trim().is_empty() {
        l.push(format!("LOCATION:{}", texte(&e.location)));
    }
    if let Some(regle) = e.rrule.as_deref().filter(|r| !r.trim().is_empty()) {
        l.push(format!("RRULE:{}", regle.trim()));
    }
    for x in &e.exdates {
        l.push(format!("EXDATE{}", moment(*x)));
    }
    if e.cancelled {
        l.push("STATUS:CANCELLED".into());
    }
    if let Some(minutes) = e.reminder_minutes {
        l.push("BEGIN:VALARM".into());
        l.push("ACTION:DISPLAY".into());
        l.push(format!("DESCRIPTION:{}", texte(&e.summary)));
        l.push(format!("TRIGGER:-PT{}M", minutes.max(0)));
        l.push("END:VALARM".into());
    }
    l.push("END:VEVENT".into());
    l
}

fn utc(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

/// Escapes a TEXT value: `\`, `;`, `,` and line breaks.
fn texte(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Lines folded at 75 octets, never inside a character, ended by CRLF.
fn finir(lignes: Vec<String>) -> String {
    let mut sortie = String::new();
    for ligne in lignes {
        let mut compte = 0;
        for c in ligne.chars() {
            let n = c.len_utf8();
            if compte + n > 75 {
                sortie.push_str("\r\n ");
                compte = 1;
            }
            sortie.push(c);
            compte += n;
        }
        sortie.push_str("\r\n");
    }
    sortie
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hebdo() -> Event {
        Event {
            uid: "hebdo@example.com".into(),
            summary: "Point d'équipe, lundi".into(),
            description: "Ordre du jour :\n1. Chiffres".into(),
            location: "Salle 3".into(),
            // 2026-09-07 10:00 in Paris.
            start: 1_788_768_000_000,
            end: 1_788_771_600_000,
            tzid: Some("Europe/Paris".into()),
            rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
            exdates: vec![1_789_372_800_000],
            reminder_minutes: Some(15),
            ..Default::default()
        }
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let ecrit = calendar("Équipe", &[hebdo()], 0);
        let relu = crate::ics::parse(&ecrit).unwrap();
        assert_eq!(relu.name.as_deref(), Some("Équipe"));
        assert_eq!(relu.events, vec![hebdo()]);
    }

    #[test]
    fn a_day_is_written_as_a_date() {
        let jour = Event {
            uid: "ferie@example.com".into(),
            summary: "Toussaint".into(),
            start: 1_793_491_200_000,
            end: 1_793_577_600_000,
            all_day: true,
            ..Default::default()
        };
        let ecrit = calendar("x", std::slice::from_ref(&jour), 0);
        assert!(ecrit.contains("DTSTART;VALUE=DATE:20261101\r\n"));
        assert_eq!(crate::ics::parse(&ecrit).unwrap().events, vec![jour]);
    }

    #[test]
    fn long_lines_are_folded_and_read_back_whole() {
        let mut e = hebdo();
        e.description = "é".repeat(120);
        let ecrit = calendar("x", std::slice::from_ref(&e), 0);
        assert!(ecrit.split("\r\n").all(|l| l.len() <= 75));
        assert_eq!(
            crate::ics::parse(&ecrit).unwrap().events[0].description,
            e.description
        );
    }

    #[test]
    fn tasks_are_written_with_their_day_state_and_list() {
        let t = Todo {
            uid: "task-1@iris".into(),
            summary: "Renew the passport".into(),
            due: Some((
                NaiveDate::from_ymd_opt(2026, 10, 8).unwrap(),
                Some(9 * 60 + 30),
            )),
            completed: Some(0),
            priority: 3,
            category: "Admin".into(),
            repeat: Some("weekdays".into()),
            ..Default::default()
        };
        let ecrit = todos("Tasks", &[t], 0);
        for attendu in [
            "BEGIN:VTODO",
            "DUE:20261008T093000",
            "STATUS:COMPLETED",
            "PRIORITY:1",
            "CATEGORIES:Admin",
            "RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
        ] {
            assert!(ecrit.contains(attendu), "{attendu} in {ecrit}");
        }
    }
}
