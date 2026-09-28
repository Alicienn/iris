//! La saisie rapide : une ligne tapée d'un trait, en français ou en anglais.
//!
//! « demain 9h appeler Marie #Travail !! » donne la tâche « appeler Marie », due demain
//! à neuf heures, dans la liste Travail, de priorité moyenne. Ce qui n'est pas reconnu
//! reste dans le titre : mieux vaut un mot de trop dans un titre qu'un mot perdu.
//!
//! Prudent par construction. Un nombre seul n'est jamais une date (« acheter 3
//! pommes »), ni une heure sans signe qui le dise (`9h`, `9:30`, `9pm`, « à 9 » en fin
//! de phrase). Les abréviations anglaises des jours ne sont pas lues : « mon » est aussi
//! un possessif, et « appeler mon frère » n'a rien d'un lundi.
//!
//! - **Dates** : today / aujourd'hui, tomorrow / demain, après-demain, un jour de la
//!   semaine (« lundi », « next friday »), next week / la semaine prochaine, next
//!   month / le mois prochain, this weekend / ce week-end, in 3 days / dans 2
//!   semaines, `28/09`, `28/09/2026`, `2026-09-28`, « 28 sept », « 1er octobre »,
//!   « September 28 ».
//! - **Heures** : `9h`, `9h30`, `9:30`, `21:00`, `9am`, `9:30 pm`, « at 9 » / « à 9 »,
//!   noon / midi, tonight / ce soir.
//! - **Liste** : `#Nom`. **Priorité** : `!`, `!!`, `!!!` (basse à haute), ou `!1` (haute)
//!   à `!3` (basse).

use crate::Due;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Weekday};

/// Ce qu'une saisie rapide veut dire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickAdd {
    pub title: String,
    pub due: Option<Due>,
    /// 0 aucune, 1 basse, 2 moyenne, 3 haute.
    pub priority: u8,
    /// Le nom écrit après `#`, tel quel. À l'appelant de le rapprocher d'une liste.
    pub list: Option<String>,
}

/// Lit une saisie rapide, à l'instant `now` (qui décide de ce que « lundi » veut dire).
pub fn parse(input: &str, now: NaiveDateTime) -> QuickAdd {
    let brut: Vec<&str> = input.split_whitespace().collect();
    let cles: Vec<String> = brut.iter().map(|t| cle(t)).collect();
    let mut pris = vec![false; brut.len()];
    let today = now.date();

    // Priorité et liste : des marques qui ne ressemblent à aucun mot.
    let mut priority = 0u8;
    let mut list = None;
    for (i, t) in brut.iter().enumerate() {
        if !t.is_empty() && t.len() <= 3 && t.chars().all(|c| c == '!') {
            priority = t.len() as u8;
            pris[i] = true;
        } else if let Some(n) = t.strip_prefix('!').and_then(|n| n.parse::<u8>().ok()) {
            if (1..=3).contains(&n) {
                priority = 4 - n;
                pris[i] = true;
            }
        } else if let Some(nom) = t.strip_prefix('#') {
            let nom = nom.trim_end_matches([',', ';', '.']);
            if !nom.is_empty() && list.is_none() {
                list = Some(nom.to_string());
                pris[i] = true;
            }
        }
    }

    // La date : la première trouvée.
    let mut date = None;
    let mut minute = None;
    for i in 0..cles.len() {
        if pris[i] {
            continue;
        }
        if let Some((jour, n, soir)) = date_a(&cles, &pris, i, today) {
            for p in pris.iter_mut().skip(i).take(n) {
                *p = true;
            }
            if i > 0 && !pris[i - 1] && LIENS_DATE.contains(&cles[i - 1].as_str()) {
                pris[i - 1] = true;
            }
            date = Some(jour);
            if soir {
                minute = Some(19 * 60);
            }
            break;
        }
    }

    // L'heure : la première trouvée.
    for i in 0..cles.len() {
        if pris[i] {
            continue;
        }
        if let Some((m, n)) = heure_a(&cles, &pris, i) {
            for p in pris.iter_mut().skip(i).take(n) {
                *p = true;
            }
            if i > 0 && !pris[i - 1] && LIENS_HEURE.contains(&cles[i - 1].as_str()) {
                pris[i - 1] = true;
            }
            minute = Some(m);
            break;
        }
    }

    let title = brut
        .iter()
        .zip(&pris)
        .filter(|(_, p)| !**p)
        .map(|(t, _)| *t)
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.trim_matches(|c: char| c == ',' || c == ';' || c.is_whitespace());

    // Rien d'autre qu'une date : c'est un titre, pas une échéance.
    if title.is_empty() {
        return QuickAdd {
            title: input.trim().to_string(),
            due: None,
            priority,
            list,
        };
    }

    // Une heure sans jour : aujourd'hui si elle n'est pas passée, sinon demain.
    let due = match (date, minute) {
        (Some(day), minute) => Some(Due { day, minute }),
        (None, Some(m)) => {
            let maintenant = (now.time() - NaiveTime::MIN).num_minutes();
            let day = if m as i64 > maintenant {
                today
            } else {
                today + Duration::days(1)
            };
            Some(Due {
                day,
                minute: Some(m),
            })
        }
        (None, None) => None,
    };

    QuickAdd {
        title: title.to_string(),
        due,
        priority,
        list,
    }
}

/// Les petits mots qui annoncent une date, et partent avec elle.
const LIENS_DATE: [&str; 7] = ["on", "le", "pour", "for", "by", "due", "d'ici"];
/// Ceux qui annoncent une heure.
const LIENS_HEURE: [&str; 5] = ["at", "a", "vers", "around", "@"];

/// Un mot pour la comparaison : en minuscules, sans accents, sans ponctuation finale.
fn cle(t: &str) -> String {
    let mut s = String::with_capacity(t.len());
    for c in t.chars().flat_map(char::to_lowercase) {
        s.push(match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            '’' => '\'',
            c => c,
        });
    }
    s.trim_end_matches([',', ';', ':', ')', '(']).to_string()
}

fn libre<'a>(cles: &'a [String], pris: &[bool], i: usize) -> Option<&'a str> {
    (i < cles.len() && !pris[i]).then(|| cles[i].as_str())
}

/// Les mots `i`, `i+1`… sont-ils exactement `mots` ?
fn suite(cles: &[String], pris: &[bool], i: usize, mots: &[&str]) -> bool {
    mots.iter()
        .enumerate()
        .all(|(k, m)| libre(cles, pris, i + k) == Some(*m))
}

fn jour_de_semaine(m: &str) -> Option<Weekday> {
    Some(match m {
        "monday" | "lundi" => Weekday::Mon,
        "tuesday" | "mardi" => Weekday::Tue,
        "wednesday" | "mercredi" => Weekday::Wed,
        "thursday" | "jeudi" => Weekday::Thu,
        "friday" | "vendredi" => Weekday::Fri,
        "saturday" | "samedi" => Weekday::Sat,
        "sunday" | "dimanche" => Weekday::Sun,
        _ => return None,
    })
}

fn mois(m: &str) -> Option<u32> {
    let m = m.trim_end_matches('.');
    Some(match m {
        "jan" | "janv" | "janvier" | "january" => 1,
        "feb" | "fev" | "fevr" | "fevrier" | "february" => 2,
        "mar" | "mars" | "march" => 3,
        "apr" | "avr" | "avril" | "april" => 4,
        "may" | "mai" => 5,
        "jun" | "juin" | "june" => 6,
        "jul" | "juil" | "juillet" | "july" => 7,
        "aug" | "aout" | "august" => 8,
        "sep" | "sept" | "septembre" | "september" => 9,
        "oct" | "octobre" | "october" => 10,
        "nov" | "novembre" | "november" => 11,
        "dec" | "decembre" | "december" => 12,
        _ => return None,
    })
}

/// « 28 », « 1er », « 3rd » : un quantième.
fn quantieme(m: &str) -> Option<u32> {
    let chiffres = m
        .strip_suffix("er")
        .or_else(|| m.strip_suffix("st"))
        .or_else(|| m.strip_suffix("nd"))
        .or_else(|| m.strip_suffix("rd"))
        .or_else(|| m.strip_suffix("th"))
        .unwrap_or(m);
    let n: u32 = chiffres.parse().ok()?;
    (1..=31).contains(&n).then_some(n)
}

fn annee(m: Option<&str>) -> Option<i32> {
    let n: i32 = m?.parse().ok()?;
    (1970..=2200).contains(&n).then_some(n)
}

/// Le prochain `jour` strictement après `today`.
fn prochain(today: NaiveDate, jour: Weekday) -> NaiveDate {
    let ecart = (jour.num_days_from_monday() as i64
        - today.weekday().num_days_from_monday() as i64)
        .rem_euclid(7);
    today + Duration::days(if ecart == 0 { 7 } else { ecart })
}

fn plus_mois(d: NaiveDate, n: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + n;
    let (a, m) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    (1..=d.day())
        .rev()
        .find_map(|j| NaiveDate::from_ymd_opt(a, m, j))
        .unwrap_or(d)
}

/// Un jour et un mois sans année : cette année, ou la suivante s'il est passé.
fn sans_annee(today: NaiveDate, m: u32, j: u32) -> Option<NaiveDate> {
    let cette = NaiveDate::from_ymd_opt(today.year(), m, j)?;
    if cette < today {
        NaiveDate::from_ymd_opt(today.year() + 1, m, j)
    } else {
        Some(cette)
    }
}

/// Une date qui commence au mot `i` : le jour, le nombre de mots, et si c'est « ce
/// soir ».
fn date_a(
    cles: &[String],
    pris: &[bool],
    i: usize,
    today: NaiveDate,
) -> Option<(NaiveDate, usize, bool)> {
    let m = libre(cles, pris, i)?;
    let suivant = libre(cles, pris, i + 1);

    // Plusieurs mots d'abord : « après demain » n'est pas « demain ».
    let expressions: [(&[&str], NaiveDate); 11] = [
        (&["day", "after", "tomorrow"], today + Duration::days(2)),
        (&["apres", "demain"], today + Duration::days(2)),
        (&["next", "week"], prochain(today, Weekday::Mon)),
        (
            &["la", "semaine", "prochaine"],
            prochain(today, Weekday::Mon),
        ),
        (&["semaine", "prochaine"], prochain(today, Weekday::Mon)),
        (&["next", "month"], plus_mois(today.with_day(1)?, 1)),
        (
            &["le", "mois", "prochain"],
            plus_mois(today.with_day(1)?, 1),
        ),
        (&["mois", "prochain"], plus_mois(today.with_day(1)?, 1)),
        (&["this", "weekend"], week_end(today)),
        (&["ce", "week-end"], week_end(today)),
        (&["ce", "weekend"], week_end(today)),
    ];
    for (mots, jour) in expressions {
        if suite(cles, pris, i, mots) {
            return Some((jour, mots.len(), false));
        }
    }
    if suite(cles, pris, i, &["ce", "soir"]) {
        return Some((today, 2, true));
    }

    // « in 3 days », « dans une semaine ».
    if matches!(m, "in" | "dans") {
        let n = match suivant? {
            "a" | "an" | "one" | "un" | "une" => 1,
            s => s.parse::<i64>().ok().filter(|n| (1..=366).contains(n))?,
        };
        let jour = match libre(cles, pris, i + 2)? {
            "day" | "days" | "jour" | "jours" => today + Duration::days(n),
            "week" | "weeks" | "semaine" | "semaines" => today + Duration::weeks(n),
            "month" | "months" | "mois" => plus_mois(today, n as i32),
            _ => return None,
        };
        return Some((jour, 3, false));
    }

    // Un mot.
    let seul = match m {
        "today" | "aujourd'hui" | "aujourdhui" | "auj" => Some(today),
        "tomorrow" | "demain" | "tmr" | "tmrw" => Some(today + Duration::days(1)),
        "apres-demain" => Some(today + Duration::days(2)),
        "weekend" | "week-end" => Some(week_end(today)),
        _ => None,
    };
    if let Some(jour) = seul {
        return Some((jour, 1, false));
    }
    if m == "tonight" {
        return Some((today, 1, true));
    }

    // Un jour de la semaine, précédé de « next » ou suivi de « prochain ».
    if m == "next" {
        if let Some(j) = suivant.and_then(jour_de_semaine) {
            return Some((prochain(today, j), 2, false));
        }
    }
    if let Some(j) = jour_de_semaine(m) {
        let n = if matches!(suivant, Some("prochain" | "prochaine")) {
            2
        } else {
            1
        };
        return Some((prochain(today, j), n, false));
    }

    // Des chiffres : 2026-09-28, 28/09, 28/09/2026, 28.09.2026.
    if let Ok(d) = NaiveDate::parse_from_str(m, "%Y-%m-%d") {
        return Some((d, 1, false));
    }
    if let Some(d) = date_chiffree(m, today) {
        return Some((d, 1, false));
    }

    // « 28 sept (2026) », « 1er octobre ».
    if let (Some(j), Some(mo)) = (quantieme(m), suivant.and_then(mois)) {
        return Some(match annee(libre(cles, pris, i + 2)) {
            Some(a) => (NaiveDate::from_ymd_opt(a, mo, j)?, 3, false),
            None => (sans_annee(today, mo, j)?, 2, false),
        });
    }
    // « September 28 (2026) ».
    if let (Some(mo), Some(j)) = (mois(m), suivant.and_then(quantieme)) {
        return Some(match annee(libre(cles, pris, i + 2)) {
            Some(a) => (NaiveDate::from_ymd_opt(a, mo, j)?, 3, false),
            None => (sans_annee(today, mo, j)?, 2, false),
        });
    }
    None
}

/// Le samedi qui vient, ou aujourd'hui si c'en est un.
fn week_end(today: NaiveDate) -> NaiveDate {
    if today.weekday() == Weekday::Sat {
        today
    } else {
        prochain(today, Weekday::Sat)
    }
}

/// `28/09`, `28/09/26`, `28/09/2026`, `28.09.2026`. Le point exige l'année : « 1.5 »
/// est un nombre bien plus souvent qu'un premier mai.
fn date_chiffree(m: &str, today: NaiveDate) -> Option<NaiveDate> {
    let (sep, parties): (char, Vec<&str>) = if m.contains('/') {
        ('/', m.split('/').collect())
    } else if m.contains('.') {
        ('.', m.split('.').collect())
    } else {
        return None;
    };
    let j: u32 = parties.first()?.parse().ok()?;
    let mo: u32 = parties.get(1)?.parse().ok()?;
    if parties.first()?.len() > 2 || parties.get(1)?.len() > 2 {
        return None;
    }
    match parties.get(2) {
        None if sep == '/' => sans_annee(today, mo, j),
        None => None,
        Some(a) if a.len() == 2 => NaiveDate::from_ymd_opt(2000 + a.parse::<i32>().ok()?, mo, j),
        Some(a) if a.len() == 4 => NaiveDate::from_ymd_opt(a.parse().ok()?, mo, j),
        Some(_) => None,
    }
    .filter(|_| parties.len() <= 3)
}

/// Une heure écrite comme telle : `9h`, `9h30`, `9:30`, `9am`, `9:30pm`, midi.
fn heure_seule(m: &str) -> Option<u32> {
    if matches!(m, "noon" | "midi") {
        return Some(12 * 60);
    }
    let (corps, apres_midi) = if let Some(c) = m.strip_suffix("am") {
        (c, Some(false))
    } else if let Some(c) = m.strip_suffix("pm") {
        (c, Some(true))
    } else {
        (m, None)
    };
    let (h, mi, marque) = if let Some((h, mi)) = corps.split_once(['h', ':']) {
        (h, if mi.is_empty() { "0" } else { mi }, true)
    } else {
        (corps, "0", apres_midi.is_some())
    };
    if !marque || h.is_empty() || h.len() > 2 || mi.len() > 2 {
        return None;
    }
    let (mut h, mi): (u32, u32) = (h.parse().ok()?, mi.parse().ok()?);
    if mi >= 60 {
        return None;
    }
    match apres_midi {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            h = h % 12 + if pm { 12 } else { 0 };
        }
        None if h > 23 => return None,
        None => {}
    }
    Some(h * 60 + mi)
}

/// Une heure qui commence au mot `i` : minutes depuis minuit, et nombre de mots.
fn heure_a(cles: &[String], pris: &[bool], i: usize) -> Option<(u32, usize)> {
    let m = libre(cles, pris, i)?;
    if let Some(minutes) = heure_seule(m) {
        return Some((minutes, 1));
    }
    // « 9 pm ».
    if let (Ok(h), Some(ampm @ ("am" | "pm"))) = (m.parse::<u32>(), libre(cles, pris, i + 1)) {
        return heure_seule(&format!("{h}{ampm}")).map(|minutes| (minutes, 2));
    }
    // « at 9 », « à 9 » : seulement si rien ne suit qui en ferait un nombre ordinaire
    // (« look at 3 options »).
    if i > 0 && LIENS_HEURE.contains(&cles[i - 1].as_str()) && !pris[i - 1] {
        let h: u32 = m.parse().ok().filter(|h| *h <= 23)?;
        let fin = match cles.get(i + 1) {
            None => true,
            Some(s) => pris[i + 1] || s.starts_with('#') || s.starts_with('!'),
        };
        if fin {
            // Sans am/pm, « à 3 » est l'après-midi : personne ne se donne rendez-vous
            // à trois heures du matin.
            let h = if (1..=7).contains(&h) { h + 12 } else { h };
            return Some((h * 60, 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lundi 28 septembre 2026, 10 h 00.
    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 28)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn due(s: &str) -> Option<(NaiveDate, Option<u32>)> {
        parse(s, now()).due.map(|d| (d.day, d.minute))
    }

    #[test]
    fn a_full_line_in_french() {
        let q = parse("demain 9h appeler Marie #Travail !!", now());
        assert_eq!(q.title, "appeler Marie");
        assert_eq!(
            q.due,
            Some(Due {
                day: d("2026-09-29"),
                minute: Some(540)
            })
        );
        assert_eq!(q.list.as_deref(), Some("Travail"));
        assert_eq!(q.priority, 2);
    }

    #[test]
    fn a_full_line_in_english() {
        let q = parse("Send the contract tomorrow at 9am #Work !1", now());
        assert_eq!(q.title, "Send the contract");
        assert_eq!(
            q.due,
            Some(Due {
                day: d("2026-09-29"),
                minute: Some(540)
            })
        );
        assert_eq!(q.list.as_deref(), Some("Work"));
        assert_eq!(q.priority, 3);
    }

    #[test]
    fn relative_days() {
        assert_eq!(due("Pay rent today"), Some((d("2026-09-28"), None)));
        assert_eq!(
            due("Payer le loyer aujourd'hui"),
            Some((d("2026-09-28"), None))
        );
        assert_eq!(due("Rappeler après-demain"), Some((d("2026-09-30"), None)));
        assert_eq!(due("Rappeler après demain"), Some((d("2026-09-30"), None)));
        assert_eq!(
            due("Call day after tomorrow"),
            Some((d("2026-09-30"), None))
        );
        assert_eq!(due("Bilan dans 3 jours"), Some((d("2026-10-01"), None)));
        assert_eq!(due("Review in 2 weeks"), Some((d("2026-10-12"), None)));
        assert_eq!(
            due("Relance dans une semaine"),
            Some((d("2026-10-05"), None))
        );
        assert_eq!(due("Budget next month"), Some((d("2026-10-01"), None)));
        assert_eq!(
            due("Budget le mois prochain"),
            Some((d("2026-10-01"), None))
        );
        assert_eq!(
            due("Ranger le garage ce week-end"),
            Some((d("2026-10-03"), None))
        );
    }

    #[test]
    fn weekdays_are_the_next_one() {
        // Lundi : « lundi » est le suivant, pas aujourd'hui.
        assert_eq!(due("Réunion lundi"), Some((d("2026-10-05"), None)));
        assert_eq!(
            due("Réunion vendredi prochain"),
            Some((d("2026-10-02"), None))
        );
        assert_eq!(due("Demo next friday"), Some((d("2026-10-02"), None)));
        assert_eq!(due("Standup on wednesday"), Some((d("2026-09-30"), None)));
        assert_eq!(parse("Standup on wednesday", now()).title, "Standup");
        assert_eq!(
            due("Point la semaine prochaine"),
            Some((d("2026-10-05"), None))
        );
    }

    #[test]
    fn written_dates() {
        assert_eq!(due("Dentiste 12/10"), Some((d("2026-10-12"), None)));
        assert_eq!(due("Dentiste le 12/10/2027"), Some((d("2027-10-12"), None)));
        assert_eq!(due("Dentiste 12.10.2026"), Some((d("2026-10-12"), None)));
        assert_eq!(due("Deadline 2026-11-02"), Some((d("2026-11-02"), None)));
        assert_eq!(
            due("Anniversaire 1er octobre"),
            Some((d("2026-10-01"), None))
        );
        assert_eq!(
            due("Impôts 15 sept"),
            Some((d("2027-09-15"), None)),
            "passé : l'an prochain"
        );
        assert_eq!(due("Launch September 30"), Some((d("2026-09-30"), None)));
        assert_eq!(due("Launch 3rd Dec 2026"), Some((d("2026-12-03"), None)));
        assert_eq!(
            parse("Anniversaire le 1er octobre", now()).title,
            "Anniversaire"
        );
    }

    #[test]
    fn times() {
        assert_eq!(due("Appel 14h30"), Some((d("2026-09-28"), Some(870))));
        assert_eq!(due("Call 9:15pm"), Some((d("2026-09-28"), Some(1275))));
        assert_eq!(due("Call at 9 pm"), Some((d("2026-09-28"), Some(1260))));
        assert_eq!(due("Déjeuner midi"), Some((d("2026-09-28"), Some(720))));
        // Déjà passée aujourd'hui : demain.
        assert_eq!(due("Café 8h"), Some((d("2026-09-29"), Some(480))));
        // « à 3 » : l'après-midi.
        assert_eq!(due("Appeler Paul à 3"), Some((d("2026-09-28"), Some(900))));
        assert_eq!(parse("Appeler Paul à 3", now()).title, "Appeler Paul");
        assert_eq!(due("Film ce soir"), Some((d("2026-09-28"), Some(1140))));
        assert_eq!(
            due("Dîner ce soir 20h"),
            Some((d("2026-09-28"), Some(1200)))
        );
    }

    #[test]
    fn ordinary_numbers_stay_in_the_title() {
        assert_eq!(parse("Acheter 3 pommes", now()).title, "Acheter 3 pommes");
        assert_eq!(due("Acheter 3 pommes"), None);
        assert_eq!(parse("Look at 3 options", now()).title, "Look at 3 options");
        assert_eq!(due("Look at 3 options"), None);
        assert_eq!(parse("Appeler mon frère", now()).title, "Appeler mon frère");
        assert_eq!(due("Appeler mon frère"), None);
        assert_eq!(due("Version 1.5 à publier"), None);
        assert_eq!(due("May the best win"), None);
    }

    #[test]
    fn a_date_alone_is_a_title() {
        let q = parse("demain", now());
        assert_eq!(q.title, "demain");
        assert_eq!(q.due, None);
    }

    #[test]
    fn priorities() {
        assert_eq!(parse("a !", now()).priority, 1);
        assert_eq!(parse("a !!!", now()).priority, 3);
        assert_eq!(parse("a !2", now()).priority, 2);
        assert_eq!(parse("a", now()).priority, 0);
        assert_eq!(parse("Wow! a", now()).title, "Wow! a");
    }
}
