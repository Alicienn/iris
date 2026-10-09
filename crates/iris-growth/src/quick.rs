//! A habit typed in one line: "Read 20 pages every day", "Run 3 times a week",
//! "Spanish 15 min on weekdays", "No phone in bed".
//!
//! Conservative, as the tasks' quick entry is: what is not understood stays in the
//! name. English and French. Three-letter day names are read only after "on" ("on
//! mon and thu"): alone, "mon" is also a French word.

use crate::habits::Schedule;

/// What a line says.
#[derive(Debug, Clone, PartialEq)]
pub struct QuickHabit {
    pub title: String,
    pub schedule: Schedule,
    /// How much makes a day done; 0 for none.
    pub amount: i32,
    pub unit: String,
    /// "No phone in bed": ticked when the day was kept.
    pub quit: bool,
    /// The icon the words suggest.
    pub icon: &'static str,
}

const QUOTIDIEN: &[&[&str]] = &[
    &["every", "day"],
    &["each", "day"],
    &["a", "day"],
    &["per", "day"],
    &["everyday"],
    &["daily"],
    &["tous", "les", "jours"],
    &["chaque", "jour"],
    &["par", "jour"],
];
const EN_SEMAINE: &[&[&str]] = &[
    &["on", "weekdays"],
    &["every", "weekday"],
    &["weekdays"],
    &["en", "semaine"],
];
const LE_WEEKEND: &[&[&str]] = &[
    &["on", "weekends"],
    &["at", "weekends"],
    &["weekends"],
    &["le", "week-end"],
    &["le", "weekend"],
];

/// A day's name, Monday 0. `apres_on`: three letters are a day only after "on".
fn jour(mot: &str, apres_on: bool) -> Option<usize> {
    const LONGS: [[&str; 2]; 7] = [
        ["monday", "lundi"],
        ["tuesday", "mardi"],
        ["wednesday", "mercredi"],
        ["thursday", "jeudi"],
        ["friday", "vendredi"],
        ["saturday", "samedi"],
        ["sunday", "dimanche"],
    ];
    const COURTS: [&[&str]; 7] = [
        &["mon"],
        &["tue", "tues"],
        &["wed"],
        &["thu", "thur", "thurs"],
        &["fri"],
        &["sat"],
        &["sun"],
    ];
    let m = mot.trim_end_matches('s');
    let m = if m.is_empty() { mot } else { m };
    (0..7).find(|i| {
        LONGS[*i].contains(&mot)
            || LONGS[*i].iter().any(|l| l.trim_end_matches('s') == m)
            || (apres_on && COURTS[*i].contains(&m))
    })
}

fn nombre(mot: &str) -> Option<u32> {
    match mot {
        "once" | "une" => Some(1),
        "twice" => Some(2),
        _ => mot.parse().ok(),
    }
}

/// The icon a habit's words suggest.
pub fn guess_icon(texte: &str) -> &'static str {
    let t = texte.to_lowercase();
    let a = |mots: &[&str]| mots.iter().any(|m| t.contains(m));
    if a(&[
        "phone",
        "screen",
        "téléphone",
        "telephone",
        "écran",
        "social",
    ]) {
        "phone"
    } else if a(&["read", "book", "lire", "livre", "lecture"]) {
        "book"
    } else if a(&[
        "run", "jog", "walk", "step", "courir", "course", "marche", "km",
    ]) {
        "run"
    } else if a(&["water", "drink", "eau", "boire", "glass", "verre"]) {
        "drop"
    } else if a(&[
        "spanish", "english", "german", "italian", "language", "espagnol", "anglais", "langue",
        "vocab",
    ]) {
        "chat"
    } else if a(&["sleep", "bed", "dormir", "lit", "coucher"]) {
        "moon"
    } else if a(&[
        "gym",
        "sport",
        "workout",
        "yoga",
        "stretch",
        "push-up",
        "pushup",
        "muscu",
        "étirement",
        "swim",
        "nager",
    ]) {
        "heart"
    } else if a(&["meditat", "breath", "respir", "calm"]) {
        "leaf"
    } else if a(&["write", "journal", "écrire", "draw", "dessin"]) {
        "pencil"
    } else if a(&["piano", "guitar", "music", "musique", "sing", "chant"]) {
        "music"
    } else if a(&["save", "budget", "money", "argent", "épargne", "spend"]) {
        "coin"
    } else if a(&[
        "study",
        "revise",
        "révis",
        "learn",
        "apprendre",
        "course",
        "cours",
    ]) {
        "cap"
    } else if a(&["morning", "wake", "lever", "réveil"]) {
        "sun"
    } else {
        "sparkle"
    }
}

/// Reads one line into a habit.
pub fn parse(texte: &str) -> QuickHabit {
    let mots: Vec<&str> = texte.split_whitespace().collect();
    let bas: Vec<String> = mots
        .iter()
        .map(|m| {
            m.trim_matches(|c: char| c == ',' || c == '.' || c == ';' || c == '!')
                .to_lowercase()
        })
        .collect();
    let mut pris = vec![false; mots.len()];

    // A phrase among `phrases`, not taken yet: taken, and true.
    let prendre = |pris: &mut Vec<bool>, phrases: &[&[&str]]| -> bool {
        for phrase in phrases {
            let n = phrase.len();
            if n > bas.len() {
                continue;
            }
            for i in 0..=bas.len() - n {
                if (0..n).all(|k| !pris[i + k] && bas[i + k] == phrase[k]) {
                    (i..i + n).for_each(|k| pris[k] = true);
                    return true;
                }
            }
        }
        false
    };

    let mut schedule = None;
    if prendre(&mut pris, EN_SEMAINE) {
        schedule = Some(Schedule::Days([true, true, true, true, true, false, false]));
    } else if prendre(&mut pris, LE_WEEKEND) {
        schedule = Some(Schedule::Days([
            false, false, false, false, false, true, true,
        ]));
    }

    // So many a week: "3 times a week", "3x a week", "3 a week", "3/week",
    // "twice a week", "3 fois par semaine".
    if schedule.is_none() {
        for i in 0..bas.len() {
            if pris[i] {
                continue;
            }
            let libre = |k: usize| k < bas.len() && !pris[k];
            let mot = |k: usize| bas.get(k).map(String::as_str).unwrap_or("");
            let (n, longueur) = if let Some(n) = mot(i)
                .strip_suffix("/week")
                .or_else(|| mot(i).strip_suffix("/wk"))
                .and_then(nombre)
            {
                (Some(n), 1)
            } else if let Some(n) = mot(i).strip_suffix('x').and_then(nombre) {
                if (mot(i + 1) == "a" || mot(i + 1) == "per") && mot(i + 2) == "week" {
                    (Some(n), 3)
                } else {
                    (None, 0)
                }
            } else if let Some(n) = nombre(mot(i)) {
                if matches!(mot(i + 1), "times" | "time" | "x")
                    && matches!(mot(i + 2), "a" | "per")
                    && mot(i + 3) == "week"
                {
                    (Some(n), 4)
                } else if matches!(mot(i + 1), "a" | "per") && mot(i + 2) == "week" {
                    (Some(n), 3)
                } else if mot(i + 1) == "fois" && mot(i + 2) == "par" && mot(i + 3) == "semaine" {
                    (Some(n), 4)
                } else {
                    (None, 0)
                }
            } else {
                (None, 0)
            };
            if let Some(n) = n.filter(|n| (1..=7).contains(n)) {
                if (i..i + longueur).all(libre) {
                    (i..i + longueur).for_each(|k| pris[k] = true);
                    schedule = Some(Schedule::per_week(n as u8));
                    break;
                }
            }
        }
    }

    // Named days: "on mon, wed and fri", "on Mondays", "lundi et jeudi".
    if schedule.is_none() {
        let mut jours = [false; 7];
        let mut apres_on = false;
        let mut vus = Vec::new();
        for i in 0..bas.len() {
            if pris[i] {
                continue;
            }
            if bas[i] == "on" || bas[i] == "le" || bas[i] == "les" {
                apres_on = true;
                continue;
            }
            if let Some(j) = jour(&bas[i], apres_on) {
                jours[j] = true;
                vus.push(i);
                // "on", and the "and" between days, go with them.
                if i > 0
                    && matches!(
                        bas[i - 1].as_str(),
                        "on" | "and" | "et" | "&" | "le" | "les"
                    )
                {
                    vus.push(i - 1);
                }
            } else if !matches!(bas[i].as_str(), "and" | "et" | "&") {
                apres_on = false;
            }
        }
        if jours.iter().any(|j| *j) {
            vus.into_iter().for_each(|k| pris[k] = true);
            schedule = Some(Schedule::days(jours));
        }
    }

    if schedule.is_none() && prendre(&mut pris, QUOTIDIEN) {
        schedule = Some(Schedule::Daily);
    }
    // "20 pages a day" leaves "a day" for the schedule; said that way it is daily.
    let _ = prendre(&mut pris, QUOTIDIEN);

    // An amount: a number and its unit, "20 pages", "15 min", "6 glasses of".
    let mut amount = 0;
    let mut unit = String::new();
    for i in 0..bas.len() {
        if pris[i] {
            continue;
        }
        let (n, u, longueur) = if let Ok(n) = bas[i].parse::<i32>() {
            match bas.get(i + 1) {
                Some(u) if !pris[i + 1] && u.chars().all(char::is_alphabetic) => {
                    (Some(n), u.clone(), 2)
                }
                _ => (None, String::new(), 0),
            }
        } else {
            // "15min", "5km".
            let chiffres: String = bas[i].chars().take_while(char::is_ascii_digit).collect();
            let reste = &bas[i][chiffres.len()..];
            if !chiffres.is_empty() && !reste.is_empty() && reste.chars().all(char::is_alphabetic) {
                (chiffres.parse().ok(), reste.to_string(), 1)
            } else {
                (None, String::new(), 0)
            }
        };
        if let Some(n) = n.filter(|n| (1..=100_000).contains(n)) {
            if matches!(u.as_str(), "times" | "time" | "x" | "fois") || jour(&u, true).is_some() {
                continue;
            }
            amount = n;
            unit = u;
            (i..i + longueur).for_each(|k| pris[k] = true);
            let suite = i + longueur;
            if suite < bas.len() && matches!(bas[suite].as_str(), "of" | "de" | "d'") {
                pris[suite] = true;
            }
            break;
        }
    }

    let reste: Vec<&str> = mots
        .iter()
        .zip(&pris)
        .filter(|(_, p)| !**p)
        .map(|(m, _)| *m)
        .collect();
    let mut title = reste
        .join(" ")
        .trim_matches(|c: char| c == ',' || c == '.' || c == ';' || c.is_whitespace())
        .to_string();
    if title.is_empty() {
        title = texte.trim().to_string();
    }
    let mut lettres = title.chars();
    if let Some(p) = lettres.next() {
        title = p.to_uppercase().chain(lettres).collect();
    }
    let premier = bas.first().map(String::as_str).unwrap_or("");
    let quit = matches!(
        premier,
        "no" | "stop" | "less" | "quit" | "fewer" | "moins" | "arrêter"
    ) || (bas.len() > 1 && premier == "pas" && bas[1] == "de");

    QuickHabit {
        icon: guess_icon(texte),
        title,
        schedule: schedule.unwrap_or(Schedule::Daily),
        amount,
        unit,
        quit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_amount_and_every_day() {
        let h = parse("Read 20 pages every day");
        assert_eq!(h.title, "Read");
        assert_eq!((h.amount, h.unit.as_str()), (20, "pages"));
        assert_eq!(h.schedule, Schedule::Daily);
        assert_eq!(h.icon, "book");
        assert!(!h.quit);
        assert_eq!(
            parse("read 20 pages a day").title,
            "Read",
            "a capital, and 'a day'"
        );
    }

    #[test]
    fn so_many_a_week() {
        for (t, n) in [
            ("Run 3 times a week", 3),
            ("Run 3x a week", 3),
            ("Run 3 a week", 3),
            ("Run twice a week", 2),
            ("Courir 3 fois par semaine", 3),
        ] {
            let h = parse(t);
            assert_eq!(h.schedule, Schedule::PerWeek(n), "{t}");
            assert_eq!(h.icon, "run", "{t}");
            assert!(h.title == "Run" || h.title == "Courir", "{t}: {}", h.title);
        }
    }

    #[test]
    fn days_by_their_names() {
        let h = parse("Spanish 15 min on weekdays");
        assert_eq!(h.title, "Spanish");
        assert_eq!((h.amount, h.unit.as_str()), (15, "min"));
        assert_eq!(
            h.schedule,
            Schedule::Days([true, true, true, true, true, false, false])
        );
        let h = parse("Gym on mon, wed and fri");
        assert_eq!(h.title, "Gym");
        assert_eq!(
            h.schedule,
            Schedule::Days([true, false, true, false, true, false, false])
        );
        let h = parse("Piano le lundi et jeudi");
        assert_eq!(h.title, "Piano");
        assert_eq!(
            h.schedule,
            Schedule::Days([true, false, false, true, false, false, false])
        );
    }

    #[test]
    fn less_of_and_what_is_not_understood() {
        let h = parse("No phone in bed");
        assert!(h.quit);
        assert_eq!(h.title, "No phone in bed");
        assert_eq!(h.icon, "phone");
        assert_eq!(h.schedule, Schedule::Daily);
        let h = parse("Drink 6 glasses of water");
        assert_eq!(h.title, "Drink water");
        assert_eq!((h.amount, h.unit.as_str()), (6, "glasses"));
        assert_eq!(h.icon, "drop");
        let h = parse("Call grandma");
        assert_eq!(h.title, "Call grandma");
        assert_eq!((h.amount, h.schedule), (0, Schedule::Daily));
        // "mon" alone is a French word, not Monday.
        assert_eq!(parse("Ranger mon bureau").title, "Ranger mon bureau");
    }
}
