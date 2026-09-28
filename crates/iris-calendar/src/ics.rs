//! Lecture d'un fichier iCalendar.
//!
//! Ce qui compte pour un agenda et rien d'autre : les événements (`VEVENT`), leur
//! heure, leur règle, leurs exceptions, et le premier rappel. Les tâches, les journaux
//! et les disponibilités sont ignorés ; un événement illisible est écarté sans
//! emporter les autres — un abonnement de cinq cents rendez-vous ne doit pas
//! disparaître pour une date mal écrite.

use crate::time::{parse_duration, parse_moment, resolve_tz};
use crate::{Event, JOUR_MS};
use icalendar::parser::{read_calendar, unfold, Component, Property};

/// Un calendrier lu : son nom, s'il en déclare un, et ses événements.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub name: Option<String>,
    pub events: Vec<Event>,
    /// Combien d'événements ont été écartés parce qu'illisibles.
    pub skipped: usize,
}

/// Lit le texte d'un fichier `.ics`.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let deplie = unfold(text);
    let calendrier = read_calendar(&deplie).map_err(|e| {
        // L'erreur de l'analyseur cite le texte ; on n'en garde que la première ligne.
        format!(
            "not an iCalendar file ({})",
            e.lines().next().unwrap_or_default()
        )
    })?;

    let prop = |nom: &str| {
        calendrier
            .properties
            .iter()
            .find(|p| p.name.as_str().eq_ignore_ascii_case(nom))
            .map(|p| unescape(p.val.as_str()))
    };
    let name = prop("X-WR-CALNAME").filter(|n| !n.trim().is_empty());
    let defaut = prop("X-WR-TIMEZONE").and_then(|n| resolve_tz(&n));

    let mut resultat = Parsed {
        name,
        ..Default::default()
    };
    collect(&calendrier.components, defaut, &mut resultat);
    Ok(resultat)
}

fn collect(components: &[Component<'_>], defaut: Option<chrono_tz::Tz>, out: &mut Parsed) {
    for c in components {
        if c.name.as_str().eq_ignore_ascii_case("VEVENT") {
            match event(c, defaut) {
                Some(e) => out.events.push(e),
                None => out.skipped += 1,
            }
        } else if c.name.as_str().eq_ignore_ascii_case("VCALENDAR") {
            // Certains fichiers emboîtent plusieurs calendriers.
            collect(&c.components, defaut, out);
        }
    }
}

fn find<'a>(c: &'a Component<'_>, nom: &str) -> Option<&'a Property<'a>> {
    c.properties
        .iter()
        .find(|p| p.name.as_str().eq_ignore_ascii_case(nom))
}

fn param<'a>(p: &'a Property<'_>, nom: &str) -> Option<&'a str> {
    p.params
        .iter()
        .find(|x| x.key.as_str().eq_ignore_ascii_case(nom))
        .and_then(|x| x.val.as_ref())
        .map(|v| v.as_str())
}

fn is_date(p: &Property<'_>) -> bool {
    param(p, "VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE"))
}

fn moment(p: &Property<'_>, defaut: Option<chrono_tz::Tz>) -> Option<crate::time::Moment> {
    parse_moment(p.val.as_str(), param(p, "TZID"), is_date(p), defaut)
}

fn event(c: &Component<'_>, defaut: Option<chrono_tz::Tz>) -> Option<Event> {
    let debut_prop = find(c, "DTSTART")?;
    let debut = moment(debut_prop, defaut)?;
    let all_day = debut.is_date();
    let start = debut.to_millis();

    let end = if let Some(fin) = find(c, "DTEND").and_then(|p| moment(p, defaut)) {
        fin.to_millis()
    } else if let Some(d) = find(c, "DURATION").and_then(|p| parse_duration(p.val.as_str())) {
        start + d
    } else if all_day {
        start + JOUR_MS
    } else {
        start
    };
    // Une fin avant le début est une erreur de l'expéditeur ; l'événement reste, à
    // durée nulle, plutôt que de disparaître.
    let end = end.max(start);

    let tzid = match debut {
        crate::time::Moment::Zoned(_, tz) => Some(tz.name().to_string()),
        _ => None,
    };

    let texte = |nom: &str| {
        find(c, nom)
            .map(|p| unescape(p.val.as_str()))
            .unwrap_or_default()
    };

    let exdates = c
        .properties
        .iter()
        .filter(|p| p.name.as_str().eq_ignore_ascii_case("EXDATE"))
        .flat_map(|p| {
            let tz = param(p, "TZID");
            let date = is_date(p);
            p.val
                .as_str()
                .split(',')
                .filter_map(move |v| parse_moment(v, tz, date, defaut))
                .map(|m| m.to_millis())
                .collect::<Vec<_>>()
        })
        .collect();

    let reminder_minutes = c
        .components
        .iter()
        .filter(|a| a.name.as_str().eq_ignore_ascii_case("VALARM"))
        .find_map(|a| find(a, "TRIGGER"))
        .and_then(|t| parse_duration(t.val.as_str()))
        .map(|ms| (-ms / 60_000) as i32)
        .filter(|m| *m >= 0);

    Some(Event {
        uid: texte("UID"),
        summary: texte("SUMMARY"),
        description: texte("DESCRIPTION"),
        location: texte("LOCATION"),
        start,
        end,
        all_day,
        tzid,
        rrule: find(c, "RRULE")
            .map(|p| p.val.as_str().trim().to_string())
            .filter(|r| !r.is_empty()),
        exdates,
        recurrence_id: find(c, "RECURRENCE-ID")
            .and_then(|p| moment(p, defaut))
            .map(|m| m.to_millis()),
        cancelled: find(c, "STATUS")
            .is_some_and(|p| p.val.as_str().eq_ignore_ascii_case("CANCELLED")),
        reminder_minutes,
    })
}

/// Retire les échappements du type TEXT : `\n`, `\,`, `\;`, `\\`.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(autre) => out.push(autre),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLUX: &str = "BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Example//Test//EN\r\n\
X-WR-CALNAME:Équipe\r\n\
X-WR-TIMEZONE:Europe/Paris\r\n\
BEGIN:VEVENT\r\n\
UID:hebdo@example.com\r\n\
DTSTART;TZID=Europe/Paris:20260907T100000\r\n\
DTEND;TZID=Europe/Paris:20260907T110000\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
EXDATE;TZID=Europe/Paris:20260914T100000\r\n\
SUMMARY:Point d'équipe\\, lundi\r\n\
DESCRIPTION:Ordre du jour :\\n1. Chiffres\\n2. Divers\r\n\
LOCATION:Salle 3\r\n\
BEGIN:VALARM\r\n\
TRIGGER:-PT15M\r\n\
ACTION:DISPLAY\r\n\
END:VALARM\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:ferie@example.com\r\n\
DTSTART;VALUE=DATE:20261101\r\n\
SUMMARY:Toussaint\r\n\
END:VEVENT\r\n\
BEGIN:VEVENT\r\n\
UID:casse@example.com\r\n\
DTSTART:pas une date\r\n\
SUMMARY:Illisible\r\n\
END:VEVENT\r\n\
BEGIN:VTODO\r\n\
UID:tache@example.com\r\n\
SUMMARY:Une tâche\r\n\
END:VTODO\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn un_flux_se_lit_avec_son_nom_et_ses_evenements() {
        let p = parse(FLUX).unwrap();
        assert_eq!(p.name.as_deref(), Some("Équipe"));
        assert_eq!(p.events.len(), 2, "le VTODO et l'illisible sont écartés");
        assert_eq!(p.skipped, 1);

        let hebdo = &p.events[0];
        assert_eq!(hebdo.summary, "Point d'équipe, lundi");
        assert_eq!(hebdo.description, "Ordre du jour :\n1. Chiffres\n2. Divers");
        assert_eq!(hebdo.location, "Salle 3");
        assert_eq!(hebdo.end - hebdo.start, 3_600_000);
        assert_eq!(hebdo.tzid.as_deref(), Some("Europe/Paris"));
        assert_eq!(hebdo.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO"));
        assert_eq!(hebdo.exdates.len(), 1);
        assert_eq!(hebdo.reminder_minutes, Some(15));
        assert!(!hebdo.all_day);
    }

    #[test]
    fn un_jour_entier_sans_fin_dure_un_jour() {
        let p = parse(FLUX).unwrap();
        let ferie = &p.events[1];
        assert!(ferie.all_day);
        assert_eq!(ferie.end - ferie.start, JOUR_MS);
    }

    #[test]
    fn un_texte_qui_n_est_pas_un_calendrier_est_refuse() {
        assert!(parse("<html>Not found</html>").is_err());
    }

    #[test]
    fn une_duree_remplace_la_fin_absente() {
        let f = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20260928T090000Z\r\nDURATION:PT45M\r\nSUMMARY:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let e = &parse(f).unwrap().events[0];
        assert_eq!(e.end - e.start, 45 * 60_000);
    }

    #[test]
    fn une_ligne_repliee_est_recollee() {
        let f = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20260928T090000Z\r\nSUMMARY:Une réunion au titre\r\n  très long\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert_eq!(
            parse(f).unwrap().events[0].summary,
            "Une réunion au titre très long"
        );
    }
}
