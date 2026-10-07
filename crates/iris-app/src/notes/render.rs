//! A block of a note, as the editor draws it when it is not being edited.

use iris_notes::block::{Block, BlockKind};
use iris_notes::inline::{self, Palette};
use iris_ui::NoteBlockData;
use slint::{ModelRc, SharedString, VecModel};

/// The words of a line for Slint's `StyledText`, plain text when its markdown would
/// not be read.
pub fn rich(src: &str, palette: &Palette) -> slint::StyledText {
    let noeuds = inline::parse_inline(src);
    let md = inline::to_slint_markdown(&noeuds, palette, &math_text);
    slint::StyledText::from_markdown(&md)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(&inline::plain_text(&noeuds)))
}

/// How inline maths reads in a line: its LaTeX made Unicode where it can be.
pub fn math_text(src: &str) -> String {
    iris_notes::math::to_unicode(src)
}

/// The label and colour of a callout kind.
pub fn callout_label(kind: &str) -> (&'static str, slint::Color, bool) {
    let c = |r, g, b| slint::Color::from_rgb_u8(r, g, b);
    // (label, colour, numbered)
    match kind {
        "def" => ("Definition", c(0x2f, 0x6f, 0xde), true),
        "thm" => ("Theorem", c(0x7c, 0x3f, 0xb8), true),
        "prop" => ("Proposition", c(0x7c, 0x3f, 0xb8), true),
        "lem" => ("Lemma", c(0x8b, 0x5c, 0xc4), true),
        "cor" => ("Corollary", c(0x8b, 0x5c, 0xc4), true),
        "proof" => ("Proof", c(0x67, 0x64, 0x5f), false),
        "example" => ("Example", c(0x0b, 0x7d, 0x46), true),
        "exo" => ("Exercise", c(0xb8, 0x62, 0x0b), true),
        "sol" => ("Solution", c(0x0b, 0x7d, 0x46), false),
        "question" | "faq" | "help" => ("Question", c(0xa0, 0x7e, 0x00), false),
        "important" => ("Important", c(0xc4, 0x35, 0x2e), false),
        "summary" | "abstract" | "tldr" => ("Summary", c(0x0e, 0x8a, 0x8a), false),
        "tip" | "hint" => ("Tip", c(0x0b, 0x7d, 0x46), false),
        "warning" | "caution" | "attention" => ("Warning", c(0xb8, 0x62, 0x0b), false),
        "danger" | "error" | "failure" => ("Danger", c(0xc4, 0x35, 0x2e), false),
        "info" | "todo" => ("Info", c(0x2f, 0x6f, 0xde), false),
        "success" | "check" | "done" => ("Done", c(0x0b, 0x7d, 0x46), false),
        "quote" | "cite" => ("Quote", c(0x67, 0x64, 0x5f), false),
        "bug" => ("Bug", c(0xc4, 0x35, 0x2e), false),
        _ => ("Note", c(0x2f, 0x6f, 0xde), false),
    }
}

/// A line without its list mark or heading signs: what its words are.
pub fn words_of(kind: &BlockKind, content: &str) -> String {
    let t = content.trim_start();
    match kind {
        BlockKind::Heading(n) => t[*n as usize..].trim_start().to_string(),
        BlockKind::Bullet { .. } => t
            .get(1..)
            .map(|r| r.strip_prefix(' ').unwrap_or(r))
            .unwrap_or("")
            .to_string(),
        BlockKind::Task { .. } => t.get(5..).map(str::trim_start).unwrap_or("").to_string(),
        BlockKind::Numbered { .. } => {
            let chiffres = t.chars().take_while(|c| c.is_ascii_digit()).count();
            t.get(chiffres + 1..)
                .map(str::trim_start)
                .unwrap_or("")
                .to_string()
        }
        BlockKind::Quote => sans_chevrons(content, 0),
        BlockKind::Callout { .. } => sans_chevrons(content, 1),
        BlockKind::Flashcard => t.replacen(" :: ", " → ", 1),
        BlockKind::Container { .. } => {
            let lignes: Vec<&str> = content.lines().collect();
            let n = lignes.len();
            lignes
                .iter()
                .enumerate()
                .filter(|(i, l)| *i > 0 && !(*i + 1 == n && l.trim() == ":::"))
                .map(|(_, l)| {
                    if l.trim().starts_with(":::col") {
                        ""
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => content.to_string(),
    }
}

/// The lines of a quote, their `>` taken off, from line `skip`.
fn sans_chevrons(content: &str, skip: usize) -> String {
    content
        .lines()
        .skip(skip)
        .map(|l| {
            let l = l.trim_start();
            let l = l.strip_prefix('>').unwrap_or(l);
            l.strip_prefix(' ').unwrap_or(l)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A fenced block's inside, its fences off.
fn interieur(content: &str) -> String {
    let lignes: Vec<&str> = content.lines().collect();
    if lignes.len() <= 1 {
        return content
            .trim()
            .trim_start_matches("$$")
            .trim_end_matches("$$")
            .trim()
            .to_string();
    }
    let fin = if lignes.last().is_some_and(|l| {
        l.trim().starts_with("```")
            || l.trim().starts_with("~~~")
            || l.trim().ends_with("$$")
            || l.trim() == "---"
    }) {
        lignes.len() - 1
    } else {
        lignes.len()
    };
    let mut corps: Vec<&str> = lignes[1..fin.max(1)].to_vec();
    // `b $$` closing on a line with maths before it.
    if let Some(derniere) = lignes.last() {
        let d = derniere.trim();
        if d.ends_with("$$") && d != "$$" && lignes.len() > 1 {
            corps.push(d.trim_end_matches("$$").trim());
        }
    }
    // `$$ a` on the first line, or `b $$` on the last: their maths count.
    if let Some(premier) = lignes.first() {
        let p = premier.trim().trim_start_matches("$$").trim();
        if !p.is_empty()
            && !premier.trim().starts_with("```")
            && !premier.trim().starts_with("~~~")
            && premier.trim() != "---"
        {
            corps.insert(0, p);
        }
    }
    corps.join("\n")
}

/// The cells of a table, row by row, the separator left out, and how many columns.
pub fn table_cells(content: &str) -> (Vec<String>, usize) {
    let mut lignes: Vec<Vec<String>> = Vec::new();
    for (i, l) in content.lines().enumerate() {
        let t = l.trim();
        if i == 1 && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
            continue;
        }
        let t = t.strip_prefix('|').unwrap_or(t);
        let t = t.strip_suffix('|').unwrap_or(t);
        lignes.push(
            t.split('|')
                .map(|c| inline::plain_text(&inline::parse_inline(c.trim())))
                .collect(),
        );
    }
    let colonnes = lignes.iter().map(Vec::len).max().unwrap_or(0);
    let mut cellules = Vec::with_capacity(lignes.len() * colonnes);
    for mut l in lignes {
        l.resize(colonnes, String::new());
        cellules.extend(l);
    }
    (cellules, colonnes)
}

/// What a callout of a kind is numbered, counting from the top of the note.
pub fn numbering(blocks: &[Block]) -> Vec<String> {
    let mut compteurs: std::collections::HashMap<String, u32> = Default::default();
    blocks
        .iter()
        .map(|b| match &b.kind {
            BlockKind::Callout { kind, .. } => {
                let (label, _, numerote) = callout_label(kind);
                if numerote {
                    let n = compteurs.entry(label.to_string()).or_insert(0);
                    *n += 1;
                    format!("{label} {n}")
                } else {
                    label.to_string()
                }
            }
            BlockKind::Numbered { n, .. } => format!("{n}."),
            _ => String::new(),
        })
        .collect()
}

/// A block as the editor shows it. `number`: what [`numbering`] gave it.
pub fn render(block: &Block, number: &str, palette: &Palette) -> NoteBlockData {
    let content = block.content();
    let mut d = NoteBlockData {
        source: content.into(),
        number: number.into(),
        ..Default::default()
    };
    let mots = |s: &str| rich(s, palette);
    match &block.kind {
        BlockKind::Blank => d.kind = "blank".into(),
        BlockKind::Paragraph => {
            d.kind = "paragraph".into();
            d.rich = mots(content);
        }
        BlockKind::Heading(n) => {
            d.kind = "heading".into();
            d.level = *n as i32;
            d.rich = mots(&words_of(&block.kind, content));
        }
        BlockKind::Bullet { indent } => {
            d.kind = "bullet".into();
            d.indent = *indent as i32;
            d.rich = mots(&words_of(&block.kind, content));
        }
        BlockKind::Numbered { indent, .. } => {
            d.kind = "numbered".into();
            d.indent = *indent as i32;
            d.rich = mots(&words_of(&block.kind, content));
        }
        BlockKind::Task { indent, done } => {
            d.kind = "task".into();
            d.indent = *indent as i32;
            d.done = *done;
            let w = words_of(&block.kind, content);
            d.rich = mots(&if *done { format!("--{w}--") } else { w });
            if *done && w.trim().is_empty() {
                d.rich = mots("");
            }
        }
        BlockKind::Quote => {
            d.kind = "quote".into();
            d.rich = mots(&words_of(&block.kind, content));
        }
        BlockKind::Callout {
            kind,
            folded,
            title,
        } => {
            d.kind = "callout".into();
            let (_, teinte, _) = callout_label(kind);
            d.tint = teinte;
            d.title = title.as_str().into();
            d.folded = folded.unwrap_or(false);
            d.rich = mots(&words_of(&block.kind, content));
        }
        BlockKind::Math => {
            d.kind = "math".into();
            d.title = interieur(content).into();
        }
        BlockKind::Code { lang } => {
            d.kind = "code".into();
            d.title = interieur(content).into();
            d.number = lang.as_str().into();
        }
        BlockKind::Rule => d.kind = "rule".into(),
        BlockKind::Table => {
            d.kind = "table".into();
            let (cellules, colonnes) = table_cells(content);
            d.rows = if colonnes == 0 {
                0
            } else {
                (cellules.len() / colonnes) as i32
            };
            d.columns = colonnes as i32;
            d.cells = ModelRc::new(VecModel::from(
                cellules
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            ));
        }
        BlockKind::Embed { target } => {
            d.kind = "embed".into();
            d.title = target.as_str().into();
            d.rich = slint::StyledText::from_plain_text(&format!("⧉ {target}"));
        }
        BlockKind::Container { kind, title } => {
            d.kind = "container".into();
            d.title = if title.is_empty() {
                kind.as_str().into()
            } else {
                title.as_str().into()
            };
            d.rich = mots(&words_of(&block.kind, content));
        }
        BlockKind::Properties => {
            d.kind = "properties".into();
            d.title = interieur(content).into();
        }
        BlockKind::Flashcard => {
            d.kind = "flashcard".into();
            d.rich = mots(&words_of(&block.kind, content));
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_notes::block::parse;

    #[test]
    fn words_lose_their_marks() {
        let k = |s: &str| parse(s).remove(0).kind;
        assert_eq!(words_of(&k("## Titre"), "## Titre"), "Titre");
        assert_eq!(words_of(&k("  - item"), "  - item"), "item");
        assert_eq!(words_of(&k("- [x] fait"), "- [x] fait"), "fait");
        assert_eq!(words_of(&k("12. douze"), "12. douze"), "douze");
        assert_eq!(words_of(&k("> a\n> b"), "> a\n> b"), "a\nb");
        assert_eq!(
            words_of(&k("> [!def] T\n> corps"), "> [!def] T\n> corps"),
            "corps"
        );
        assert_eq!(words_of(&k("Q :: R"), "Q :: R"), "Q → R");
    }

    #[test]
    fn fenced_blocks_lose_their_fences() {
        assert_eq!(interieur("```rust\nfn x() {}\n```"), "fn x() {}");
        assert_eq!(interieur("$$\n\\int f\n$$"), "\\int f");
        assert_eq!(interieur("$$ x^2 $$"), "x^2");
        assert_eq!(interieur("---\na: 1\n---"), "a: 1");
    }

    #[test]
    fn tables_give_their_cells() {
        let (c, n) = table_cells("| a | **b** |\n|---|---|\n| 1 | 2 |\n| 3 |");
        assert_eq!(n, 2);
        assert_eq!(c, ["a", "b", "1", "2", "3", ""]);
    }

    #[test]
    fn callouts_are_numbered_by_kind() {
        let blocs = parse("> [!def] A\n\n> [!thm] B\n\n> [!def] C\n\n> [!proof] D\n");
        let n = numbering(&blocs);
        let non_vides: Vec<&str> = n
            .iter()
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            non_vides,
            ["Definition 1", "Theorem 1", "Definition 2", "Proof"]
        );
    }
}
