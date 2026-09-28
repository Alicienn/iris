//! Le déroulement des récurrences sur une période.

use crate::{Event, Occurrence};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashSet;

/// Au-delà, une règle est jugée pathologique pour la période : une réunion « toutes
/// les minutes » n'a rien à faire dans une grille de mois.
const MAX_PAR_EVENEMENT: u16 = 1_000;

/// Les occurrences de `events` qui touchent `[from, to[`, triées par début.
///
/// - un événement simple apparaît s'il chevauche la période ;
/// - une règle est déroulée dans le fuseau de l'événement, pour qu'une réunion à 10 h
///   reste à 10 h de part et d'autre d'un changement d'heure ;
/// - les dates exclues (`EXDATE`) sont retirées ;
/// - une occurrence modifiée (même `UID`, avec `RECURRENCE-ID`) remplace celle qu'elle
///   désigne, là où elle a été déplacée ;
/// - un événement annulé n'apparaît pas, et efface l'occurrence qu'il désigne.
pub fn occurrences(events: &[Event], from: i64, to: i64) -> Vec<Occurrence> {
    // Les occurrences remplacées : (uid, instant d'origine).
    let remplacees: HashSet<(&str, i64)> = events
        .iter()
        .filter_map(|e| e.recurrence_id.map(|r| (e.uid.as_str(), r)))
        .collect();

    let mut sortie = Vec::new();
    for (i, e) in events.iter().enumerate() {
        if e.cancelled {
            continue;
        }
        let duree = (e.end - e.start).max(0);
        match (&e.rrule, e.recurrence_id) {
            (Some(regle), None) => {
                for debut in expand(e, regle, from - duree.max(1), to) {
                    if e.exdates.contains(&debut) || remplacees.contains(&(e.uid.as_str(), debut)) {
                        continue;
                    }
                    let fin = debut + duree;
                    if chevauche(debut, fin, from, to) {
                        sortie.push(Occurrence {
                            event: i,
                            start: debut,
                            end: fin,
                            all_day: e.all_day,
                        });
                    }
                }
            }
            _ => {
                if chevauche(e.start, e.end, from, to) {
                    sortie.push(Occurrence {
                        event: i,
                        start: e.start,
                        end: e.end,
                        all_day: e.all_day,
                    });
                }
            }
        }
    }
    sortie.sort_by_key(|o| (o.start, !o.all_day, o.end));
    sortie
}

/// Une durée nulle compte comme un instant : elle chevauche si elle tombe dedans.
fn chevauche(debut: i64, fin: i64, from: i64, to: i64) -> bool {
    debut < to && (fin > from || (fin == debut && debut >= from))
}

/// Les débuts d'occurrence d'une règle entre `apres` et `avant`.
///
/// Une règle que la bibliothèque refuse — il y en a d'exotiques dans la nature — ne
/// donne que son premier jour, plutôt que de faire disparaître l'événement.
fn expand(e: &Event, regle: &str, apres: i64, avant: i64) -> Vec<i64> {
    let (dtstart, tz) = match e.tzid.as_deref().and_then(crate::time::resolve_tz) {
        Some(tz) if !e.all_day => {
            let local = tz
                .timestamp_millis_opt(e.start)
                .single()
                .map(|t| t.naive_local());
            match local {
                Some(l) => (
                    format!("DTSTART;TZID={}:{}", tz.name(), l.format("%Y%m%dT%H%M%S")),
                    rrule::Tz::Tz(tz),
                ),
                None => (utc_dtstart(e.start), rrule::Tz::UTC),
            }
        }
        _ => (utc_dtstart(e.start), rrule::Tz::UTC),
    };
    let texte = format!("{dtstart}\nRRULE:{}", normalise_until(regle));

    let ensemble = match texte.parse::<rrule::RRuleSet>() {
        Ok(s) => s,
        Err(err) => {
            tracing_like(&format!("règle illisible « {regle} » : {err}"));
            return vec![e.start];
        }
    };
    let borne = |ms: i64| -> Option<DateTime<rrule::Tz>> {
        Utc.timestamp_millis_opt(ms)
            .single()
            .map(|t| t.with_timezone(&tz))
    };
    let (Some(a), Some(b)) = (borne(apres), borne(avant)) else {
        return Vec::new();
    };
    ensemble
        .after(a)
        .before(b)
        .all(MAX_PAR_EVENEMENT)
        .dates
        .into_iter()
        .map(|d| d.timestamp_millis())
        .collect()
}

fn utc_dtstart(ms: i64) -> String {
    let t = Utc.timestamp_millis_opt(ms).single().unwrap_or_default();
    format!("DTSTART:{}", t.format("%Y%m%dT%H%M%SZ"))
}

/// `UNTIL` doit avoir la forme de `DTSTART` : une date seule devient la fin de ce jour,
/// en UTC. C'est l'écart que les fichiers réels commettent le plus souvent.
fn normalise_until(regle: &str) -> String {
    regle
        .split(';')
        .map(|partie| match partie.split_once('=') {
            Some((cle, valeur)) if cle.eq_ignore_ascii_case("UNTIL") => {
                let v = valeur.trim();
                if v.len() == 8 && v.chars().all(|c| c.is_ascii_digit()) {
                    format!("UNTIL={v}T235959Z")
                } else if !v.ends_with('Z') && !v.ends_with('z') {
                    format!("UNTIL={v}Z")
                } else {
                    partie.to_string()
                }
            }
            _ => partie.to_string(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Ce crate n'a pas de journal ; une règle refusée ne vaut pas d'en imposer un.
fn tracing_like(_message: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ics::parse;

    fn ms(s: &str) -> i64 {
        crate::time::parse_moment(s, None, false, None)
            .unwrap()
            .to_millis()
    }

    fn hebdo() -> Event {
        Event {
            uid: "hebdo".into(),
            summary: "Point".into(),
            start: ms("20260907T080000Z"),
            end: ms("20260907T090000Z"),
            tzid: Some("Europe/Paris".into()),
            rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
            ..Default::default()
        }
    }

    #[test]
    fn une_regle_hebdomadaire_donne_un_lundi_par_semaine() {
        let o = occurrences(&[hebdo()], ms("20260901T000000Z"), ms("20261001T000000Z"));
        assert_eq!(o.len(), 4, "7, 14, 21 et 28 septembre");
        assert!(o
            .windows(2)
            .all(|w| w[1].start - w[0].start == 7 * 86_400_000));
    }

    #[test]
    fn l_heure_locale_tient_a_travers_le_changement_d_heure() {
        // 10 h à Paris : 8 h UTC en été, 9 h UTC en hiver (après le 25 octobre).
        let o = occurrences(&[hebdo()], ms("20261019T000000Z"), ms("20261103T000000Z"));
        let heures: Vec<i64> = o.iter().map(|x| (x.start / 3_600_000) % 24).collect();
        assert_eq!(heures, vec![8, 9, 9], "19 et 26 octobre, 2 novembre");
    }

    #[test]
    fn une_date_exclue_disparait() {
        let mut e = hebdo();
        e.exdates.push(ms("20260914T080000Z"));
        let o = occurrences(&[e], ms("20260901T000000Z"), ms("20261001T000000Z"));
        assert_eq!(o.len(), 3);
    }

    #[test]
    fn une_occurrence_deplacee_remplace_celle_d_origine() {
        let deplacee = Event {
            uid: "hebdo".into(),
            summary: "Point (décalé)".into(),
            start: ms("20260915T130000Z"),
            end: ms("20260915T140000Z"),
            recurrence_id: Some(ms("20260914T080000Z")),
            ..Default::default()
        };
        let evenements = vec![hebdo(), deplacee];
        let o = occurrences(&evenements, ms("20260913T000000Z"), ms("20260917T000000Z"));
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].event, 1, "c'est la version déplacée qui apparaît");
    }

    #[test]
    fn un_evenement_annule_n_apparait_pas() {
        let mut e = hebdo();
        e.rrule = None;
        e.cancelled = true;
        assert!(occurrences(&[e], 0, i64::MAX / 2).is_empty());
    }

    #[test]
    fn until_en_date_seule_est_accepte() {
        let mut e = hebdo();
        e.rrule = Some("FREQ=DAILY;UNTIL=20260910".into());
        let o = occurrences(&[e], ms("20260901T000000Z"), ms("20261001T000000Z"));
        assert_eq!(o.len(), 4, "7, 8, 9 et 10 septembre");
    }

    #[test]
    fn une_regle_illisible_garde_le_premier_jour() {
        let mut e = hebdo();
        e.rrule = Some("FREQ=SOMETIMES".into());
        let o = occurrences(&[e], ms("20260901T000000Z"), ms("20261001T000000Z"));
        assert_eq!(o.len(), 1);
    }

    #[test]
    fn un_jour_entier_recurrent_reste_a_minuit() {
        let f = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:anniv\r\nDTSTART;VALUE=DATE:20200412\r\nRRULE:FREQ=YEARLY\r\nSUMMARY:Anniversaire\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let e = parse(f).unwrap().events;
        let o = occurrences(&e, ms("20260101T000000Z"), ms("20270101T000000Z"));
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].start % 86_400_000, 0);
        assert!(o[0].all_day);
    }

    #[test]
    fn un_evenement_simple_hors_periode_est_ignore() {
        let mut e = hebdo();
        e.rrule = None;
        assert!(occurrences(&[e], ms("20261001T000000Z"), ms("20261101T000000Z")).is_empty());
    }
}
