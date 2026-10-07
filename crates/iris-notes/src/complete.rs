//! What a popup offers while typing, and what choosing it writes.
//!
//! - `[[` a note, then `#` its headings;
//! - `#` a tag already used;
//! - `@` a date by its word (`@demain`, `@friday`);
//! - `\` a LaTeX command;
//! - `/` at the start of a line, a block (`/thm`, `/table`, `/math`);
//! - `{` a colour.

use crate::edit::Edit;

/// What is being typed at the cursor, and where it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// `[[query` (a heading when the query has a `#`).
    Note {
        query: String,
        start: usize,
    },
    Tag {
        query: String,
        start: usize,
    },
    Date {
        query: String,
        start: usize,
    },
    Command {
        query: String,
        start: usize,
    },
    Block {
        query: String,
        start: usize,
    },
    Colour {
        query: String,
        start: usize,
    },
}

impl Trigger {
    pub fn query(&self) -> &str {
        match self {
            Trigger::Note { query, .. }
            | Trigger::Tag { query, .. }
            | Trigger::Date { query, .. }
            | Trigger::Command { query, .. }
            | Trigger::Block { query, .. }
            | Trigger::Colour { query, .. } => query,
        }
    }

    pub fn start(&self) -> usize {
        match self {
            Trigger::Note { start, .. }
            | Trigger::Tag { start, .. }
            | Trigger::Date { start, .. }
            | Trigger::Command { start, .. }
            | Trigger::Block { start, .. }
            | Trigger::Colour { start, .. } => *start,
        }
    }
}

/// What is being typed just before `cursor` in a block's source, if it asks for a
/// popup.
pub fn trigger_at(text: &str, cursor: usize) -> Option<Trigger> {
    let mut c = cursor.min(text.len());
    while !text.is_char_boundary(c) {
        c -= 1;
    }
    let avant = &text[..c];
    let ligne = &avant[avant.rfind('\n').map_or(0, |k| k + 1)..];

    // `[[…` not closed yet.
    if let Some(k) = avant.rfind("[[") {
        let q = &avant[k + 2..];
        if !q.contains("]]") && !q.contains('\n') && q.len() <= 120 {
            return Some(Trigger::Note {
                query: q.to_string(),
                start: k,
            });
        }
    }
    // `/word` alone at the start of a line.
    if let Some(q) = ligne.trim_start().strip_prefix('/') {
        if !q.contains(char::is_whitespace) && ligne.trim_start().len() == ligne.len() {
            return Some(Trigger::Block {
                query: q.to_string(),
                start: c - ligne.len(),
            });
        }
    }
    // The word being typed, back to a space.
    let debut_mot = ligne
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace() || *ch == '(' || *ch == '$')
        .map_or(0, |(k, ch)| k + ch.len_utf8());
    let mot = &ligne[debut_mot..];
    let start = c - ligne.len() + debut_mot;
    let mut car = mot.chars();
    match car.next() {
        Some('#') if !mot[1..].contains('#') && !mot[1..].is_empty() => Some(Trigger::Tag {
            query: mot[1..].to_string(),
            start,
        }),
        Some('@') if mot[1..].chars().all(|c| c.is_alphanumeric() || c == '-') => {
            Some(Trigger::Date {
                query: mot[1..].to_string(),
                start,
            })
        }
        Some('{') if mot[1..].chars().all(|c| c.is_alphabetic()) && !mot.contains('}') => {
            Some(Trigger::Colour {
                query: mot[1..].to_string(),
                start,
            })
        }
        _ => {
            // `\com` anywhere in the word.
            let k = mot.rfind('\\')?;
            let q = &mot[k + 1..];
            q.chars()
                .all(|c| c.is_ascii_alphabetic())
                .then(|| Trigger::Command {
                    query: q.to_string(),
                    start: start + k,
                })
        }
    }
}

/// A choice in the popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub detail: String,
    /// What replaces what was typed from the trigger's start.
    pub insert: String,
    /// Where the cursor goes, counted from the start of `insert`.
    pub cursor: usize,
}

/// `insert` written in place of what was typed for `trigger`, up to `cursor`.
pub fn apply(text: &str, cursor: usize, trigger: &Trigger, candidate: &Candidate) -> Edit {
    let debut = trigger.start().min(text.len());
    let fin = cursor.min(text.len()).max(debut);
    // A link closed already after the cursor (pairs) is not closed twice.
    let reste = &text[fin..];
    let reste = if matches!(trigger, Trigger::Note { .. }) && candidate.insert.ends_with("]]") {
        reste.strip_prefix("]]").unwrap_or(reste)
    } else {
        reste
    };
    let t = format!("{}{}{}", &text[..debut], candidate.insert, reste);
    let c = debut + candidate.cursor.min(candidate.insert.len());
    Edit {
        text: t,
        cursor: c,
        anchor: c,
    }
}

/// The blocks the `/` menu offers: (what to type, its name, what it writes, where the
/// cursor goes in it).
pub const BLOCKS: &[(&str, &str, &str, usize)] = &[
    ("h1", "Heading 1", "# ", 2),
    ("h2", "Heading 2", "## ", 3),
    ("h3", "Heading 3", "### ", 4),
    ("list", "Bulleted list", "- ", 2),
    ("num", "Numbered list", "1. ", 3),
    ("todo", "Checkbox", "- [ ] ", 6),
    ("quote", "Quote", "> ", 2),
    ("def", "Definition", "> [!def] ", 9),
    ("thm", "Theorem", "> [!thm] ", 9),
    ("prop", "Proposition", "> [!prop] ", 10),
    ("lem", "Lemma", "> [!lem] ", 9),
    ("cor", "Corollary", "> [!cor] ", 9),
    ("proof", "Proof (folded)", "> [!proof]- ", 12),
    ("ex", "Example", "> [!example] ", 13),
    ("exo", "Exercise", "> [!exo] ", 9),
    ("sol", "Solution (folded)", "> [!sol]- ", 10),
    ("q", "Question", "> [!question] ", 14),
    ("important", "Important", "> [!important] ", 15),
    ("summary", "Summary", "> [!summary] ", 13),
    ("note", "Note", "> [!note] ", 10),
    ("tip", "Tip", "> [!tip] ", 9),
    ("warn", "Warning", "> [!warning] ", 13),
    ("math", "Maths block", "$$\n\n$$", 3),
    ("code", "Code block", "```\n\n```", 4),
    ("table", "Table", "| A | B |\n|---|---|\n|  |  |", 22),
    ("rule", "Divider", "---", 3),
    ("cols", "Two columns", ":::cols\n\n:::col\n\n:::", 8),
    ("fold", "Foldable block", ":::fold Details\n\n:::", 16),
    ("card", "Flashcard", "Question :: Answer", 0),
    ("sheet", "Spreadsheet", "![[Sheet.sheet]]", 3),
];

/// The blocks matching what was typed after `/`.
pub fn block_candidates(query: &str) -> Vec<Candidate> {
    let q = crate::fuzzy::folded(query);
    BLOCKS
        .iter()
        .filter(|(k, nom, _, _)| {
            q.is_empty() || k.starts_with(&q) || crate::fuzzy::folded(nom).contains(&q)
        })
        .map(|(_, nom, insert, c)| Candidate {
            label: nom.to_string(),
            detail: String::new(),
            insert: insert.to_string(),
            cursor: *c,
        })
        .collect()
}

/// The colours matching what was typed after `{`.
pub fn colour_candidates(query: &str) -> Vec<Candidate> {
    let q = query.to_ascii_lowercase();
    crate::inline::Colour::ALL
        .iter()
        .filter(|c| q.is_empty() || c.name().to_ascii_lowercase().starts_with(&q) || c.code() == q)
        .map(|c| Candidate {
            label: c.name().to_string(),
            detail: format!("{{{}}}", c.code()),
            insert: format!("{{{}}}{{/}}", c.code()),
            cursor: c.code().len() + 2,
        })
        .collect()
}

/// The dates a word names, from `today`: today, tomorrow, the days of the week (the
/// next one), in English and in French.
pub fn date_candidates(query: &str, today: chrono::NaiveDate) -> Vec<Candidate> {
    use chrono::{Datelike, Duration};
    let q = crate::fuzzy::folded(query);
    let jours = [
        ("monday", "lundi"),
        ("tuesday", "mardi"),
        ("wednesday", "mercredi"),
        ("thursday", "jeudi"),
        ("friday", "vendredi"),
        ("saturday", "samedi"),
        ("sunday", "dimanche"),
    ];
    let mut mots: Vec<(String, chrono::NaiveDate)> = vec![
        ("today".into(), today),
        ("aujourdhui".into(), today),
        ("tomorrow".into(), today + Duration::days(1)),
        ("demain".into(), today + Duration::days(1)),
        ("yesterday".into(), today - Duration::days(1)),
        ("hier".into(), today - Duration::days(1)),
    ];
    for (i, (en, fr)) in jours.iter().enumerate() {
        let ecart = (i as i64 - today.weekday().num_days_from_monday() as i64 + 7) % 7;
        let jour = today + Duration::days(if ecart == 0 { 7 } else { ecart });
        mots.push((en.to_string(), jour));
        mots.push((fr.to_string(), jour));
    }
    let mut vus = std::collections::HashSet::new();
    let mut sortie = Vec::new();
    // A date typed whole is offered as it is.
    if let Ok(d) = chrono::NaiveDate::parse_from_str(query, "%Y-%m-%d") {
        mots.insert(0, (query.to_string(), d));
    }
    for (mot, jour) in mots {
        if !(q.is_empty() || mot.starts_with(&q)) || !vus.insert(jour) {
            continue;
        }
        let texte = jour.format("%Y-%m-%d").to_string();
        sortie.push(Candidate {
            label: mot,
            detail: jour.format("%A %-d %B %Y").to_string(),
            insert: format!("@{texte}"),
            cursor: texte.len() + 1,
        });
    }
    sortie
}

/// The notes matching what was typed after `[[` (the space's notes, relative).
pub fn note_candidates(query: &str, notes: &[String], max: usize) -> Vec<Candidate> {
    let noms: Vec<&str> = notes.iter().map(|n| crate::links::stem(n)).collect();
    let mut sortie: Vec<Candidate> = crate::fuzzy::rank(query, &noms, max)
        .into_iter()
        .map(|i| {
            let nom = crate::links::link_name(&notes[i], notes);
            Candidate {
                label: noms[i].to_string(),
                detail: notes[i]
                    .rsplit_once('/')
                    .map(|(d, _)| d.replace('/', " › "))
                    .unwrap_or_default(),
                insert: format!("[[{nom}]]"),
                cursor: nom.len() + 4,
            }
        })
        .collect();
    // What was typed, as a note to make.
    let q = query.trim();
    if !q.is_empty() && !noms.iter().any(|n| n.eq_ignore_ascii_case(q)) {
        sortie.push(Candidate {
            label: format!("New note “{q}”"),
            detail: String::new(),
            insert: format!("[[{q}]]"),
            cursor: q.len() + 4,
        });
    }
    sortie
}

/// The tags matching what was typed after `#`, among those already used.
pub fn tag_candidates(query: &str, tags: &[String], max: usize) -> Vec<Candidate> {
    let refs: Vec<&str> = tags.iter().map(String::as_str).collect();
    crate::fuzzy::rank(query, &refs, max)
        .into_iter()
        .map(|i| Candidate {
            label: format!("#{}", tags[i]),
            detail: String::new(),
            insert: format!("#{} ", tags[i]),
            cursor: tags[i].len() + 2,
        })
        .collect()
}

/// The LaTeX commands matching what was typed after `\`.
pub fn command_candidates(query: &str, max: usize) -> Vec<Candidate> {
    crate::math::COMMANDS
        .iter()
        .filter(|(nom, _)| nom.starts_with(query))
        .take(max)
        .map(|(nom, apercu)| {
            let (insert, cursor) = match *nom {
                "frac" => ("\\frac{}{}".to_string(), 6),
                "sqrt" | "vec" | "hat" | "bar" | "text" | "mathbb" | "mathcal" | "mathrm"
                | "overline" | "tilde" | "dot" | "operatorname" => {
                    (format!("\\{nom}{{}}"), nom.len() + 2)
                }
                _ => (format!("\\{nom}"), nom.len() + 1),
            };
            Candidate {
                label: format!("\\{nom}"),
                detail: apercu.to_string(),
                insert,
                cursor,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_are_seen_at_the_cursor() {
        assert_eq!(
            trigger_at("voir [[Lim", 10),
            Some(Trigger::Note {
                query: "Lim".into(),
                start: 5
            })
        );
        assert!(trigger_at("voir [[Lim]] ok", 15).is_none());
        assert_eq!(
            trigger_at("/th", 3),
            Some(Trigger::Block {
                query: "th".into(),
                start: 0
            })
        );
        assert!(
            trigger_at("a /th", 5).is_none(),
            "only at the start of a line"
        );
        assert_eq!(
            trigger_at("cours #ana", 10),
            Some(Trigger::Tag {
                query: "ana".into(),
                start: 6
            })
        );
        assert_eq!(
            trigger_at("rdv @dem", 8),
            Some(Trigger::Date {
                query: "dem".into(),
                start: 4
            })
        );
        assert_eq!(
            trigger_at("$x \\al", 6),
            Some(Trigger::Command {
                query: "al".into(),
                start: 3
            })
        );
        assert_eq!(
            trigger_at("un {ro", 6),
            Some(Trigger::Colour {
                query: "ro".into(),
                start: 3
            })
        );
        assert!(trigger_at("texte simple", 12).is_none());
    }

    #[test]
    fn choosing_writes_in_place() {
        let t = trigger_at("voir [[Lim]]", 10).unwrap();
        let c = Candidate {
            label: "Limites".into(),
            detail: String::new(),
            insert: "[[Limites]]".into(),
            cursor: 11,
        };
        let e = apply("voir [[Lim]]", 10, &t, &c);
        assert_eq!(
            e.text, "voir [[Limites]]",
            "the closing brackets typed are not doubled"
        );
        assert_eq!(e.cursor, 16);
        let t = trigger_at("/thm", 4).unwrap();
        let b = &block_candidates("thm")[0];
        let e = apply("/thm", 4, &t, b);
        assert_eq!(e.text, "> [!thm] ");
    }

    #[test]
    fn dates_by_their_word() {
        let jeudi = chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        let c = date_candidates("dem", jeudi);
        assert_eq!(c[0].insert, "@2026-10-09");
        let c = date_candidates("lun", jeudi);
        assert_eq!(c[0].insert, "@2026-10-12");
        let c = date_candidates("jeudi", jeudi);
        assert_eq!(c[0].insert, "@2026-10-15", "the next one, not today");
    }

    #[test]
    fn notes_offer_a_new_one_too() {
        let n = vec!["Analyse/Limites.md".to_string(), "Accueil.md".to_string()];
        let c = note_candidates("lim", &n, 5);
        assert_eq!(c[0].insert, "[[Limites]]");
        assert_eq!(c.last().unwrap().label, "New note “lim”");
        assert!(command_candidates("fr", 5)
            .iter()
            .any(|c| c.insert == "\\frac{}{}"));
        assert_eq!(colour_candidates("r")[0].insert, "{r}{/}");
    }
}
