//! What a note says about itself: its tags, its dates, its properties, its words.

use crate::block::{self, BlockKind};
use crate::inline::{self, Mark, Node};

/// The properties at the top of a note (`key: value` between `---` lines), in order.
/// Lists (`tags: [a, b]`) are kept as written.
pub fn properties(text: &str) -> Vec<(String, String)> {
    let blocs = block::parse(text);
    let Some(premier) = blocs.first().filter(|b| b.kind == BlockKind::Properties) else {
        return Vec::new();
    };
    premier
        .content()
        .lines()
        .skip(1)
        .filter(|l| !matches!(l.trim(), "---" | "..."))
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let k = k.trim();
            (!k.is_empty() && !k.starts_with('-')).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

/// The tags of a note, without `#`, each once: those in its words and those in its
/// properties (`tags: [a, b]` or `tags: a, b`).
pub fn tags(text: &str) -> Vec<String> {
    let mut sortie: Vec<String> = Vec::new();
    let mut ajouter = |t: &str| {
        let t = t.trim().trim_start_matches('#').trim_matches(['"', '\'']);
        if !t.is_empty() && !sortie.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            sortie.push(t.to_string());
        }
    };
    for (k, v) in properties(text) {
        if k.eq_ignore_ascii_case("tags") || k.eq_ignore_ascii_case("tag") {
            for t in v.trim_matches(['[', ']']).split(',') {
                ajouter(t);
            }
        }
    }
    for b in block::parse(text) {
        if matches!(
            b.kind,
            BlockKind::Code { .. } | BlockKind::Math | BlockKind::Properties
        ) {
            continue;
        }
        for m in marques(&inline::parse_inline(b.content())) {
            if let Mark::Tag(t) = m {
                ajouter(&t);
            }
        }
    }
    sortie
}

/// Every mark of a line, nested ones included.
fn marques(noeuds: &[Node]) -> Vec<Mark> {
    let mut sortie = Vec::new();
    for n in noeuds {
        if let Node::Mark { mark, children, .. } = n {
            sortie.push(mark.clone());
            sortie.extend(marques(children));
        }
    }
    sortie
}

/// How many words a note has, its marks left out.
pub fn word_count(text: &str) -> usize {
    block::parse(text)
        .iter()
        .filter(|b| !matches!(b.kind, BlockKind::Properties))
        .map(|b| {
            inline::plain_text(&inline::parse_inline(b.content()))
                .split_whitespace()
                .filter(|w| w.chars().any(char::is_alphanumeric))
                .count()
        })
        .sum()
}

/// The flashcards of a note: (question, answer) from `Question :: Answer` lines and
/// from `> [!question]` callouts (their title the question, their words the answer).
pub fn flashcards(text: &str) -> Vec<(String, String)> {
    let mut sortie = Vec::new();
    for b in block::parse(text) {
        match &b.kind {
            BlockKind::Flashcard => {
                if let Some((q, r)) = b.content().split_once(" :: ") {
                    sortie.push((q.trim().to_string(), r.trim().to_string()));
                }
            }
            BlockKind::Callout { kind, title, .. } if kind == "question" => {
                let corps: Vec<&str> = b
                    .content()
                    .lines()
                    .skip(1)
                    .map(|l| l.trim_start().trim_start_matches('>').trim())
                    .collect();
                sortie.push((title.clone(), corps.join("\n").trim().to_string()));
            }
            _ => {}
        }
    }
    sortie
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_tags_and_words() {
        let t = "---\ntitle: Limites\ntags: [maths, analyse]\n---\nUn #cours sur les #maths, `#pas` ici.\n";
        assert_eq!(
            properties(t),
            vec![
                ("title".to_string(), "Limites".to_string()),
                ("tags".to_string(), "[maths, analyse]".to_string())
            ]
        );
        assert_eq!(tags(t), ["maths", "analyse", "cours"]);
        assert_eq!(word_count("**Deux** mots\n- et trois — encore\n"), 5);
    }

    #[test]
    fn cards_from_lines_and_callouts() {
        let t = "Capitale :: Paris\n> [!question] 2+2 ?\n> 4\n";
        assert_eq!(
            flashcards(t),
            vec![
                ("Capitale".to_string(), "Paris".to_string()),
                ("2+2 ?".to_string(), "4".to_string())
            ]
        );
    }
}
