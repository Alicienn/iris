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

/// The block a link's `#…` names: `thm-2` the second theorem (as numbered), else a
/// heading by its words.
pub fn ancre_bloc(blocks: &[Block], ancre: &str) -> Option<usize> {
    let ancre = ancre.trim();
    if let Some((genre, n)) = ancre.rsplit_once('-') {
        if let Ok(n) = n.trim().parse::<u32>() {
            let (label, _, numerote) = callout_label(genre.trim());
            if numerote {
                let voulu = format!("{label} {n}");
                if let Some(i) = numbering(blocks).iter().position(|x| *x == voulu) {
                    return Some(i);
                }
            }
        }
    }
    let plat = |s: &str| s.trim().replace(['-', '_'], " ").to_lowercase();
    blocks.iter().position(|b| {
        matches!(b.kind, BlockKind::Heading(_))
            && plat(b.content().trim_start_matches('#')) == plat(ancre)
    })
}

/// The file an embed names, in the space at `dir`: `![[a.png|50%]]` looks in the
/// space, then in its attachments.
pub fn embedded_file(target: &str, dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let nom = target.split('|').next().unwrap_or(target).trim();
    let nom = nom.split('#').next().unwrap_or(nom);
    if nom.is_empty() || nom.contains("..") || nom.contains(':') {
        return None;
    }
    let directe = dir.join(nom);
    if directe.is_file() {
        return Some(directe);
    }
    let rangee = dir.join(iris_vault::ATTACHMENTS).join(nom);
    rangee.is_file().then_some(rangee)
}

/// The widest a picture is kept in memory, in pixels: the column is narrower.
const LARGEUR_MAX: u32 = 1600;

/// A picture file, decoded and made no wider than the column needs.
pub fn picture(path: &std::path::Path) -> Option<slint::Image> {
    let img = image::open(path).ok()?;
    let img = if img.width() > LARGEUR_MAX {
        img.resize(
            LARGEUR_MAX,
            LARGEUR_MAX * img.height() / img.width().max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
        slint::Rgba8Pixel,
    >::clone_from_slice(
        rgba.as_raw(), w, h
    )))
}

/// Whether a file name is a picture the editor draws.
/// Whether `![[…]]` embeds a spreadsheet.
pub fn est_tableur(target: &str) -> bool {
    let nom = target.split(['|', '#']).next().unwrap_or(target).trim();
    nom.to_ascii_lowercase().ends_with(".sheet")
}

/// The most of a spreadsheet a note shows when no range is given.
const TABLEAU_LIGNES: u32 = 30;
const TABLEAU_COLONNES: u32 = 12;

/// The values of a spreadsheet embedded with `![[Budget.sheet]]` or
/// `![[Budget.sheet#A1:D10]]`, as a table: its cells row by row, and how many columns.
pub fn tableau_insere(target: &str, dir: &std::path::Path) -> Option<(Vec<String>, usize)> {
    let sans_taille = target.split('|').next().unwrap_or(target);
    let (nom, plage) = match sans_taille.split_once('#') {
        Some((n, p)) => (n.trim(), iris_sheets::Range::parse(p.trim())),
        None => (sans_taille.trim(), None),
    };
    let chemin = embedded_file(nom, dir)?;
    let book = iris_sheets::Workbook::from_json(&std::fs::read_to_string(chemin).ok()?).ok()?;
    let valeurs = book.compute();
    let feuille = book.sheets.first()?;
    let plage = match plage {
        Some(p) => p,
        None => {
            let (cols, rows) = feuille.extent();
            if cols == 0 || rows == 0 {
                return None;
            }
            iris_sheets::Range::new(
                iris_sheets::Addr::new(0, 0),
                iris_sheets::Addr::new(
                    cols.min(TABLEAU_COLONNES) - 1,
                    rows.min(TABLEAU_LIGNES) - 1,
                ),
            )
        }
    };
    if plage.len() > u64::from(TABLEAU_LIGNES * TABLEAU_COLONNES) * 4 {
        return None;
    }
    let l = super::sheet::locale();
    let cellules: Vec<String> = plage
        .cells()
        .map(|a| iris_sheets::format::display(&valeurs.get(&book, 0, a), &feuille.format(a), &l))
        .collect();
    Some((cellules, plage.width() as usize))
}

pub fn is_picture(nom: &str) -> bool {
    let n = nom.split('|').next().unwrap_or(nom).to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"]
        .iter()
        .any(|e| n.ends_with(e))
}

/// How a display formula is drawn: its LaTeX in, a picture and its pixels per logical
/// pixel out (`None`: shown as text).
pub type Formula<'a> = &'a dyn Fn(&str) -> Option<(slint::Image, f32)>;

/// A formula drawn with `iris-math` in `colour`, at the screen's `scale`.
pub fn formula_picture(
    latex: &str,
    colour: slint::Color,
    scale: f32,
) -> Option<(slint::Image, f32)> {
    let p = iris_math::render(
        latex,
        iris_math::Style {
            display: true,
            size: 16.0,
            scale,
            colour: [colour.red(), colour.green(), colour.blue()],
        },
    )
    .ok()?;
    Some((
        slint::Image::from_rgba8(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &p.rgba, p.width, p.height,
            ),
        ),
        scale.max(1.0),
    ))
}

/// How many bytes of a one-line block are the mark that made it — `## `, `- `, `3. `,
/// `- [ ] `, `> `, with the indent before it — hidden while the line is typed, as
/// Notion and Obsidian convert them. 0 for a block typed as it is shown.
pub fn prefixe(kind: &BlockKind, content: &str) -> usize {
    if content.contains('\n') {
        return 0;
    }
    let retrait = content.len() - content.trim_start_matches([' ', '\t']).len();
    let r = &content[retrait..];
    let n = match kind {
        BlockKind::Heading(_) => {
            let dieses = r.bytes().take_while(|b| *b == b'#').count();
            let espaces = r[dieses..].bytes().take_while(|b| *b == b' ').count();
            if dieses > 0 && espaces > 0 {
                dieses + espaces
            } else {
                0
            }
        }
        BlockKind::Task { .. } => {
            let marque = ["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "+ [ ] "]
                .iter()
                .find(|m| r.starts_with(**m));
            marque.map_or(0, |m| m.len())
        }
        BlockKind::Bullet { .. } => {
            if ["- ", "* ", "+ "].iter().any(|m| r.starts_with(m)) {
                2
            } else {
                0
            }
        }
        BlockKind::Numbered { .. } => {
            let chiffres = r.bytes().take_while(u8::is_ascii_digit).count();
            let reste = &r[chiffres..];
            if chiffres > 0 && (reste.starts_with(". ") || reste.starts_with(") ")) {
                chiffres + 2
            } else {
                0
            }
        }
        BlockKind::Quote if !r.starts_with("> [!") => {
            if r.starts_with("> ") {
                2
            } else if r.starts_with('>') {
                1
            } else {
                0
            }
        }
        _ => 0,
    };
    if n == 0 {
        0
    } else {
        retrait + n
    }
}

/// A block as the editor shows it. `number`: what [`numbering`] gave it; `dir`: the
/// space, for the pictures it embeds; `formula` draws display maths.
pub fn render(
    block: &Block,
    number: &str,
    palette: &Palette,
    dir: Option<&std::path::Path>,
    formula: Formula<'_>,
) -> NoteBlockData {
    let content = block.content();
    // What is edited: the words, without the mark that made the block (`# `, `- `…),
    // drawn as the block is (a heading's size, a bullet, a box) while it is typed.
    let p = prefixe(&block.kind, content);
    let mut d = NoteBlockData {
        source: content[p..].into(),
        prefix: content[..p].into(),
        prefix_bytes: p as i32,
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
            // Done: struck through, unless there is nothing to strike.
            d.rich = if *done && !w.trim().is_empty() {
                mots(&format!("--{w}--"))
            } else {
                mots(&w)
            };
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
            let latex = interieur(content);
            if let Some((image, ratio)) = formula(&latex) {
                d.picture = image;
                d.has_picture = true;
                d.ratio = ratio;
            }
            d.title = latex.into();
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
            d.rows = cellules.len().checked_div(colonnes).unwrap_or(0) as i32;
            d.columns = colonnes as i32;
            d.cells = ModelRc::new(VecModel::from(
                cellules
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            ));
        }
        BlockKind::Embed { target } if est_tableur(target) => {
            d.title = target.as_str().into();
            match dir.and_then(|d| tableau_insere(target, d)) {
                Some((cellules, colonnes)) => {
                    d.kind = "table".into();
                    d.rows = cellules.len().checked_div(colonnes).unwrap_or(0) as i32;
                    d.columns = colonnes as i32;
                    d.cells = ModelRc::new(VecModel::from(
                        cellules
                            .into_iter()
                            .map(SharedString::from)
                            .collect::<Vec<_>>(),
                    ));
                }
                None => {
                    d.kind = "embed".into();
                    d.rich = slint::StyledText::from_plain_text(&format!("⧉ {target}"));
                }
            }
        }
        BlockKind::Embed { target } => {
            d.kind = "embed".into();
            d.title = target.as_str().into();
            d.rich = slint::StyledText::from_plain_text(&format!("⧉ {target}"));
            if is_picture(target) {
                if let Some(image) = dir
                    .and_then(|d| embedded_file(target, d))
                    .and_then(|p| picture(&p))
                {
                    d.picture = image;
                    d.has_picture = true;
                }
            }
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
    fn the_marks_that_make_a_block_are_hidden_while_typing() {
        let p = |s: &str| {
            let b = parse(s).remove(0);
            let n = prefixe(&b.kind, b.content());
            (b.content()[..n].to_string(), b.content()[n..].to_string())
        };
        assert_eq!(p("## Limites"), ("## ".into(), "Limites".into()));
        assert_eq!(p("- item"), ("- ".into(), "item".into()));
        assert_eq!(p("  - sous-point"), ("  - ".into(), "sous-point".into()));
        assert_eq!(p("12. douze"), ("12. ".into(), "douze".into()));
        assert_eq!(p("- [x] fait"), ("- [x] ".into(), "fait".into()));
        assert_eq!(p("> citation"), ("> ".into(), "citation".into()));
        assert_eq!(p("Un paragraphe"), (String::new(), "Un paragraphe".into()));
        // Several lines, a callout: typed as they are.
        assert_eq!(p("> a\n> b").0, "");
        assert_eq!(p("> [!def] Limite\n> x").0, "");
    }

    #[test]
    fn anchors_name_numbered_callouts_and_headings() {
        let b = parse("# Limites\n\n> [!thm] A\n> x\n\n## Suites et séries\n\n> [!thm] B\n> y\n");
        let pos = |a: &str| ancre_bloc(&b, a).map(|i| b[i].content().trim_end().to_string());
        assert_eq!(pos("thm-2").as_deref(), Some("> [!thm] B\n> y"));
        assert_eq!(pos("thm-1").as_deref(), Some("> [!thm] A\n> x"));
        assert_eq!(
            pos("suites-et-séries").as_deref(),
            Some("## Suites et séries")
        );
        assert_eq!(pos("Limites").as_deref(), Some("# Limites"));
        assert_eq!(pos("thm-9"), None);
    }

    #[test]
    fn a_spreadsheet_embedded_shows_its_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = iris_sheets::Workbook::default();
        let s = &mut b.sheets[0];
        s.set_input(iris_sheets::Addr::new(0, 0), "Item");
        s.set_input(iris_sheets::Addr::new(1, 0), "Price");
        s.set_input(iris_sheets::Addr::new(0, 1), "Book");
        s.set_input(iris_sheets::Addr::new(1, 1), "12");
        s.set_input(iris_sheets::Addr::new(1, 2), "=B2*2");
        std::fs::write(dir.path().join("Budget.sheet"), b.to_json()).unwrap();
        assert!(est_tableur("Budget.sheet"));
        assert!(est_tableur("Budget.sheet#A1:B2|50%"));
        assert!(!est_tableur("Budget.png"));
        let (cellules, colonnes) = tableau_insere("Budget.sheet", dir.path()).unwrap();
        assert_eq!(colonnes, 2);
        assert_eq!(cellules, ["Item", "Price", "Book", "12", "", "24"]);
        let (cellules, colonnes) = tableau_insere("Budget.sheet#B1:B2", dir.path()).unwrap();
        assert_eq!(colonnes, 1);
        assert_eq!(cellules, ["Price", "12"]);
        assert!(tableau_insere("Missing.sheet", dir.path()).is_none());
    }

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
