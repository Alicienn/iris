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
    let mut l = vec!["BEGIN:VEVENT".to_string(), format!("UID:{}", texte(&e.uid))];
    l.push(format!("DTSTAMP:{}", utc(stamp_ms)));
    if let Some(id) = e.recurrence_id {
        l.push(format!("RECURRENCE-ID{}", moment(e, id)));
    }
    l.extend(proprietes(e));
    l.extend(alarme(e));
    l.push("END:VEVENT".into());
    l
}

/// How a moment of this event is written, after the property's name: a date for a
/// day, the local time in its named zone, else UTC.
fn moment(e: &Event, ms: i64) -> String {
    if e.all_day {
        let jour = DateTime::<Utc>::from_timestamp_millis(ms)
            .map(|t| t.date_naive())
            .unwrap_or_default();
        return format!(";VALUE=DATE:{}", jour.format("%Y%m%d"));
    }
    match e
        .tzid
        .as_deref()
        .and_then(resolve_tz)
        .and_then(|tz| tz.timestamp_millis_opt(ms).single())
    {
        Some(t) => format!(
            ";TZID={}:{}",
            t.timezone().name(),
            t.format("%Y%m%dT%H%M%S")
        ),
        None => format!(":{}", utc(ms)),
    }
}

/// The properties Iris keeps of an event, as lines: its time, its words, its rule.
fn proprietes(e: &Event) -> Vec<String> {
    let mut l = vec![format!("DTSTART{}", moment(e, e.start))];
    if e.end > e.start {
        l.push(format!("DTEND{}", moment(e, e.end)));
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
        l.push(format!("EXDATE{}", moment(e, *x)));
    }
    if e.cancelled {
        l.push("STATUS:CANCELLED".into());
    }
    l
}

/// Its reminder, as an alarm.
fn alarme(e: &Event) -> Vec<String> {
    match e.reminder_minutes {
        Some(minutes) => vec![
            "BEGIN:VALARM".into(),
            "ACTION:DISPLAY".into(),
            format!("DESCRIPTION:{}", texte(&e.summary)),
            format!("TRIGGER:-PT{}M", minutes.max(0)),
            "END:VALARM".into(),
        ],
        None => Vec::new(),
    }
}

/// The properties [`proprietes`] writes, which a patched event loses for Iris's own.
const GEREES: &[&str] = &[
    "DTSTART",
    "DTEND",
    "DURATION",
    "SUMMARY",
    "DESCRIPTION",
    "LOCATION",
    "RRULE",
    "EXDATE",
    "DTSTAMP",
    "LAST-MODIFIED",
    "SEQUENCE",
];

/// An object read from a server, with Iris's changes: its events (the one that repeats
/// and its changed occurrences, under one UID) given back as `events` now are.
///
/// Everything Iris does not keep stays as the server wrote it — attendees, organiser,
/// categories, its own time zones, other properties. Only the properties Iris keeps are
/// replaced, the sequence counted up and the stamps renewed; an occurrence Iris no
/// longer has is left out, one it added is added. The alarms stay unless the reminder
/// changed.
pub fn patch_object(original: &str, events: &[&Event], stamp_ms: i64) -> String {
    let deplie = original
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "");
    let lignes: Vec<&str> = deplie
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty())
        .collect();
    let mut sortie: Vec<String> = Vec::new();
    let mut servis = vec![false; events.len()];
    let mut i = 0;
    while i < lignes.len() {
        let ligne = lignes[i];
        if ligne.eq_ignore_ascii_case("BEGIN:VEVENT") {
            // The whole event, its alarms included.
            let mut fin = i + 1;
            let mut profondeur = 0;
            while fin < lignes.len() {
                let l = lignes[fin];
                if commence(l, "BEGIN:") {
                    profondeur += 1;
                } else if l.eq_ignore_ascii_case("END:VEVENT") && profondeur == 0 {
                    break;
                } else if commence(l, "END:") {
                    profondeur -= 1;
                }
                fin += 1;
            }
            let bloc = &lignes[i..=fin.min(lignes.len() - 1)];
            let rid = bloc.iter().find_map(|l| {
                let (nom, params, valeur) = decouper(l);
                (nom == "RECURRENCE-ID").then(|| {
                    let tz = params
                        .iter()
                        .find(|(k, _)| k == "TZID")
                        .map(|(_, v)| v.as_str());
                    let date = params
                        .iter()
                        .any(|(k, v)| k == "VALUE" && v.eq_ignore_ascii_case("DATE"));
                    crate::time::parse_moment(valeur, tz, date, None).map(|m| m.to_millis())
                })
            });
            let rid = rid.flatten();
            if let Some(n) = events.iter().position(|e| e.recurrence_id == rid) {
                servis[n] = true;
                sortie.extend(corriger(bloc, events[n], stamp_ms));
            }
            i = fin + 1;
            continue;
        }
        if ligne.eq_ignore_ascii_case("END:VCALENDAR") {
            for (n, e) in events.iter().enumerate() {
                if !servis[n] {
                    sortie.extend(vevent(e, stamp_ms));
                    servis[n] = true;
                }
            }
        }
        sortie.push(ligne.to_string());
        i += 1;
    }
    finir(sortie)
}

/// One event of an object, its kept properties replaced by Iris's.
fn corriger(bloc: &[&str], e: &Event, stamp_ms: i64) -> Vec<String> {
    let reminder_d_origine = {
        let mut dans = false;
        let mut trouve = None;
        for l in bloc {
            if l.eq_ignore_ascii_case("BEGIN:VALARM") {
                dans = true;
            } else if l.eq_ignore_ascii_case("END:VALARM") {
                dans = false;
            } else if dans && trouve.is_none() {
                let (nom, _, valeur) = decouper(l);
                if nom == "TRIGGER" {
                    trouve = crate::time::parse_duration(valeur)
                        .map(|ms| (-ms / 60_000) as i32)
                        .filter(|m| *m >= 0);
                }
            }
        }
        trouve
    };
    let garder_alarmes = reminder_d_origine == e.reminder_minutes;
    let mut sequence = 0i64;
    let mut l: Vec<String> = Vec::new();
    let mut profondeur = 0;
    let mut dans_alarme = false;
    for ligne in bloc.iter().skip(1) {
        if ligne.eq_ignore_ascii_case("END:VEVENT") && profondeur == 0 {
            break;
        }
        if commence(ligne, "BEGIN:") {
            profondeur += 1;
            dans_alarme = ligne.eq_ignore_ascii_case("BEGIN:VALARM");
            if dans_alarme && !garder_alarmes {
                continue;
            }
            l.push(ligne.to_string());
            continue;
        }
        if commence(ligne, "END:") {
            profondeur -= 1;
            let etait = dans_alarme;
            dans_alarme = false;
            if etait && !garder_alarmes {
                continue;
            }
            l.push(ligne.to_string());
            continue;
        }
        if profondeur > 0 {
            if !dans_alarme || garder_alarmes {
                l.push(ligne.to_string());
            }
            continue;
        }
        let (nom, _, valeur) = decouper(ligne);
        if nom == "SEQUENCE" {
            sequence = valeur.trim().parse().unwrap_or(0);
        }
        // Its status stays unless Iris cancels it or brings it back.
        let statut_gere =
            nom == "STATUS" && (e.cancelled || valeur.eq_ignore_ascii_case("CANCELLED"));
        if GEREES.contains(&nom.as_str()) || statut_gere {
            continue;
        }
        l.push(ligne.to_string());
    }
    let mut sortie = vec!["BEGIN:VEVENT".to_string()];
    sortie.extend(l);
    sortie.push(format!("DTSTAMP:{}", utc(stamp_ms)));
    sortie.push(format!("LAST-MODIFIED:{}", utc(stamp_ms)));
    sortie.push(format!("SEQUENCE:{}", sequence + 1));
    sortie.extend(proprietes(e));
    if !garder_alarmes {
        sortie.extend(alarme(e));
    }
    sortie.push("END:VEVENT".into());
    sortie
}

/// Whether a line starts with `prefixe`, whatever the case (and never cutting a
/// character).
fn commence(ligne: &str, prefixe: &str) -> bool {
    ligne
        .get(..prefixe.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(prefixe))
}

/// A content line: its name in capitals, its parameters, its value. Quoted parameter
/// values may hold `:` and `;`.
fn decouper(ligne: &str) -> (String, Vec<(String, String)>, &str) {
    let mut guillemets = false;
    let mut deux_points = None;
    for (i, c) in ligne.char_indices() {
        match c {
            '"' => guillemets = !guillemets,
            ':' if !guillemets => {
                deux_points = Some(i);
                break;
            }
            _ => {}
        }
    }
    let Some(dp) = deux_points else {
        return (ligne.to_ascii_uppercase(), Vec::new(), "");
    };
    let tete = &ligne[..dp];
    let valeur = &ligne[dp + 1..];
    let mut morceaux = tete.split(';');
    let nom = morceaux.next().unwrap_or("").to_ascii_uppercase();
    let params = morceaux
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.to_ascii_uppercase(), v.trim_matches('"').to_string()))
        })
        .collect();
    (nom, params, valeur)
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

    const REUNION: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\n\
BEGIN:VEVENT\r\nUID:r@example.com\r\nDTSTAMP:20260901T000000Z\r\nSEQUENCE:2\r\n\
DTSTART:20260907T080000Z\r\nDTEND:20260907T090000Z\r\nRRULE:FREQ=WEEKLY\r\nSUMMARY:Old\r\n\
ORGANIZER:mailto:boss@example.com\r\nATTENDEE;CN=\"Doe, J\":mailto:j@example.com\r\n\
BEGIN:VALARM\r\nACTION:AUDIO\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:r@example.com\r\nRECURRENCE-ID:20260914T080000Z\r\n\
DTSTART:20260914T100000Z\r\nDTEND:20260914T110000Z\r\nSUMMARY:Moved\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn a_patched_object_keeps_what_iris_does_not_know() {
        let mut e = crate::ics::parse(REUNION).unwrap().events.remove(0);
        e.summary = "New".into();
        e.start += 3_600_000;
        e.end += 3_600_000;
        let ecrit = patch_object(REUNION, &[&e], 0);
        assert!(ecrit.contains("ORGANIZER:mailto:boss@example.com"));
        assert!(ecrit.contains("ATTENDEE;CN=\"Doe, J\":mailto:j@example.com"));
        assert!(
            ecrit.contains("ACTION:AUDIO"),
            "the reminder did not change"
        );
        assert!(ecrit.contains("SEQUENCE:3"));
        assert!(!ecrit.contains("SUMMARY:Old"));
        assert!(
            !ecrit.contains("SUMMARY:Moved"),
            "an occurrence Iris no longer has"
        );
        assert_eq!(crate::ics::parse(&ecrit).unwrap().events, vec![e]);
    }

    #[test]
    fn a_patched_object_takes_a_new_occurrence_and_a_new_reminder() {
        let relus = crate::ics::parse(REUNION).unwrap().events;
        let mut maitre = relus[0].clone();
        maitre.reminder_minutes = Some(30);
        let mut change = relus[1].clone();
        change.summary = "Moved again".into();
        let nouvelle = Event {
            recurrence_id: Some(1_789_977_600_000),
            start: 1_789_984_800_000,
            end: 1_789_988_400_000,
            summary: "Third".into(),
            rrule: None,
            ..maitre.clone()
        };
        let ecrit = patch_object(REUNION, &[&maitre, &change, &nouvelle], 0);
        assert!(!ecrit.contains("ACTION:AUDIO"), "the reminder changed");
        assert!(ecrit.contains("TRIGGER:-PT30M"));
        let relu = crate::ics::parse(&ecrit).unwrap().events;
        assert_eq!(relu.len(), 3);
        assert_eq!(relu[1].summary, "Moved again");
        assert_eq!(relu[2].recurrence_id, Some(1_789_977_600_000));
        assert!(ecrit.split("\r\n").all(|l| l.len() <= 75));
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
