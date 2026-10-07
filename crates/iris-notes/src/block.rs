//! A note as blocks, cut and joined without losing a byte.
//!
//! The editor works line by line, as one writes: each line is a block of its own —
//! a paragraph, a heading, a list item, a task — except what only makes sense whole,
//! which is one block over several lines: the properties at the top, a fenced code
//! block, a `$$` maths block, a quote or a callout (its `>` lines), a table, a `:::`
//! container.
//!
//! Each block keeps its exact source, line endings included, so that `join(parse(x))`
//! is `x` for any text: CRLF files, trailing spaces, no final newline, Markdown that
//! is not quite Markdown. Saving a note writes back only what was typed.

/// What a block is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    /// An empty line.
    Blank,
    Paragraph,
    /// `#` to `######`.
    Heading(u8),
    /// `- `, `* `, `+ `, by how deep it is nested.
    Bullet {
        indent: u8,
    },
    /// `1. ` or `1) `.
    Numbered {
        indent: u8,
        n: u32,
    },
    /// `- [ ] ` or `- [x] `.
    Task {
        indent: u8,
        done: bool,
    },
    /// `>` lines that are not a callout.
    Quote,
    /// `> [!kind] Title`, folded with `-`, unfolded with `+`.
    Callout {
        kind: String,
        folded: Option<bool>,
        title: String,
    },
    /// `$$ … $$`.
    Math,
    /// A fenced block, and its language.
    Code {
        lang: String,
    },
    /// `---`, `***`, `___`.
    Rule,
    /// `| a | b |` lines under a header and a separator.
    Table,
    /// `![[file]]` or `![alt](url)` alone on its line.
    Embed {
        target: String,
    },
    /// `:::kind Title` … `:::`.
    Container {
        kind: String,
        title: String,
    },
    /// The YAML between `---` lines at the very top.
    Properties,
    /// `Question :: Answer`.
    Flashcard,
}

impl BlockKind {
    /// Whether the block runs over several lines, where Enter adds a line rather than
    /// making a new block.
    pub fn is_multiline(&self) -> bool {
        matches!(
            self,
            BlockKind::Quote
                | BlockKind::Callout { .. }
                | BlockKind::Math
                | BlockKind::Code { .. }
                | BlockKind::Table
                | BlockKind::Container { .. }
                | BlockKind::Properties
        )
    }
}

/// A block and its exact source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// The source, with its final line ending when it has one.
    pub text: String,
}

impl Block {
    /// The source without its final line ending.
    pub fn content(&self) -> &str {
        &self.text[..self.text.len() - self.line_ending().len()]
    }

    /// `"\r\n"`, `"\n"`, or `""` for a last line with none.
    pub fn line_ending(&self) -> &str {
        if self.text.ends_with("\r\n") {
            "\r\n"
        } else if self.text.ends_with('\n') {
            "\n"
        } else {
            ""
        }
    }
}

/// The text back, exactly.
pub fn join(blocks: &[Block]) -> String {
    let mut sortie = String::with_capacity(blocks.iter().map(|b| b.text.len()).sum());
    for b in blocks {
        sortie.push_str(&b.text);
    }
    sortie
}

/// Where block `index` starts in the joined text.
pub fn start_of(blocks: &[Block], index: usize) -> usize {
    blocks[..index.min(blocks.len())]
        .iter()
        .map(|b| b.text.len())
        .sum()
}

/// The block holding byte `offset` of the joined text, and the offset within it. An
/// offset at the very end is in the last block.
pub fn locate(blocks: &[Block], offset: usize) -> (usize, usize) {
    let mut debut = 0;
    for (i, b) in blocks.iter().enumerate() {
        let fin = debut + b.content().len();
        if offset <= fin {
            return (i, offset.saturating_sub(debut));
        }
        debut += b.text.len();
    }
    match blocks.last() {
        Some(b) => (blocks.len() - 1, b.content().len()),
        None => (0, 0),
    }
}

/// Cuts a note into blocks.
pub fn parse(text: &str) -> Vec<Block> {
    let lignes: Vec<&str> = text.split_inclusive('\n').collect();
    let mut blocs = Vec::new();
    let mut i = 0;
    while i < lignes.len() {
        let (kind, n) = classer(&lignes, i);
        let n = n.max(1).min(lignes.len() - i);
        blocs.push(Block {
            kind,
            text: lignes[i..i + n].concat(),
        });
        i += n;
    }
    blocs
}

/// A line without its line ending.
fn nu(ligne: &str) -> &str {
    ligne.trim_end_matches(['\n', '\r'])
}

/// The kind of the block starting at line `i`, and how many lines it takes.
fn classer(lignes: &[&str], i: usize) -> (BlockKind, usize) {
    let l = nu(lignes[i]);
    let t = l.trim_start();

    if t.is_empty() {
        return (BlockKind::Blank, 1);
    }

    // The properties: `---` on the first line, up to the next `---`.
    if i == 0 && l.trim_end() == "---" {
        if let Some(fin) =
            (1..lignes.len()).find(|&k| matches!(nu(lignes[k]).trim_end(), "---" | "..."))
        {
            return (BlockKind::Properties, fin + 1);
        }
    }

    // A fenced code block, up to its closing fence.
    if let Some(barriere) = ouverture_de_code(t) {
        let lang = t.trim_start_matches(barriere.0).trim().to_string();
        let fin = (i + 1..lignes.len()).find(|&k| {
            let f = nu(lignes[k]).trim();
            f.starts_with(barriere.0)
                && f.chars().take_while(|c| *c == barriere.1).count() >= barriere.0.len()
                && f.trim_start_matches(barriere.1).trim().is_empty()
        });
        return (
            BlockKind::Code { lang },
            fin.map_or(lignes.len() - i, |f| f - i + 1),
        );
    }

    // Display maths: `$$` alone, or `$$ … $$` on one line.
    if let Some(reste) = t.strip_prefix("$$") {
        let reste = reste.trim_end();
        if reste.len() >= 2 && reste.ends_with("$$") {
            return (BlockKind::Math, 1);
        }
        let fin = (i + 1..lignes.len()).find(|&k| nu(lignes[k]).trim_end().ends_with("$$"));
        return (BlockKind::Math, fin.map_or(lignes.len() - i, |f| f - i + 1));
    }

    // A container: `:::kind Title`, up to its `:::`, nested ones counted.
    if let Some(entete) = t.strip_prefix(":::") {
        let entete = entete.trim();
        if !entete.is_empty() && !entete.starts_with(':') {
            let (kind, title) = match entete.split_once(char::is_whitespace) {
                Some((k, r)) => (k.to_string(), r.trim().to_string()),
                None => (entete.to_string(), String::new()),
            };
            let mut profondeur = 1;
            let mut fin = None;
            for (k, ligne) in lignes.iter().enumerate().skip(i + 1) {
                let f = nu(ligne).trim();
                if f == ":::" {
                    profondeur -= 1;
                    if profondeur == 0 {
                        fin = Some(k);
                        break;
                    }
                } else if f.starts_with(":::") && f.len() > 3 && f != ":::col" {
                    profondeur += 1;
                }
            }
            return (
                BlockKind::Container { kind, title },
                fin.map_or(lignes.len() - i, |f| f - i + 1),
            );
        }
    }

    // A quote or a callout: its `>` lines together.
    if est_citation(l) {
        let n = lignes[i..]
            .iter()
            .take_while(|x| est_citation(nu(x)))
            .count();
        return (callout(t).unwrap_or(BlockKind::Quote), n);
    }

    // A table: `|` lines, the second a separator.
    if t.starts_with('|') && i + 1 < lignes.len() && est_separateur(nu(lignes[i + 1])) {
        let n = lignes[i..]
            .iter()
            .take_while(|x| nu(x).trim_start().starts_with('|'))
            .count();
        return (BlockKind::Table, n.max(2));
    }

    (ligne_seule(l), 1)
}

/// The fence opening a code block, and its character.
fn ouverture_de_code(t: &str) -> Option<(&'static str, char)> {
    if t.starts_with("```") {
        Some(("```", '`'))
    } else if t.starts_with("~~~") {
        Some(("~~~", '~'))
    } else {
        None
    }
}

fn est_citation(l: &str) -> bool {
    let espaces = l.len() - l.trim_start_matches(' ').len();
    espaces <= 3 && l.trim_start().starts_with('>')
}

/// `> [!def] Title`, `> [!thm]- Title`.
fn callout(t: &str) -> Option<BlockKind> {
    let r = t.strip_prefix('>')?.trim_start();
    let r = r.strip_prefix("[!")?;
    let (kind, apres) = r.split_once(']')?;
    if kind.is_empty() || kind.contains(char::is_whitespace) {
        return None;
    }
    let (folded, titre) = match apres.chars().next() {
        Some('-') => (Some(true), &apres[1..]),
        Some('+') => (Some(false), &apres[1..]),
        _ => (None, apres),
    };
    Some(BlockKind::Callout {
        kind: kind.to_ascii_lowercase(),
        folded,
        title: titre.trim().to_string(),
    })
}

fn est_separateur(l: &str) -> bool {
    let t = l.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// How deep a list item is: two spaces (or a tab) a level.
fn profondeur(l: &str) -> u8 {
    let mut largeur = 0usize;
    for c in l.chars() {
        match c {
            ' ' => largeur += 1,
            '\t' => largeur += 2,
            _ => break,
        }
    }
    (largeur / 2).min(8) as u8
}

/// A block of one line.
fn ligne_seule(l: &str) -> BlockKind {
    let t = l.trim_start();

    // `#` to `######`, then a space or nothing: `#tag` is not a heading.
    let dieses = t.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&dieses) {
        let reste = &t[dieses..];
        if reste.is_empty() || reste.starts_with(' ') {
            return BlockKind::Heading(dieses as u8);
        }
    }

    if est_regle(t) {
        return BlockKind::Rule;
    }

    let indent = profondeur(l);
    if let Some(reste) = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
    {
        if let Some(apres) = reste.strip_prefix('[') {
            let mut c = apres.chars();
            if let (Some(coche), Some(']')) = (c.next(), c.next()) {
                if matches!(coche, ' ' | 'x' | 'X')
                    && (c.as_str().is_empty() || c.as_str().starts_with(' '))
                {
                    return BlockKind::Task {
                        indent,
                        done: coche != ' ',
                    };
                }
            }
        }
        return BlockKind::Bullet { indent };
    }
    if matches!(t, "-" | "*" | "+") {
        return BlockKind::Bullet { indent };
    }

    let chiffres = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&chiffres) {
        let reste = &t[chiffres..];
        if reste.starts_with(". ") || reste.starts_with(") ") || reste == "." || reste == ")" {
            return BlockKind::Numbered {
                indent,
                n: t[..chiffres].parse().unwrap_or(1),
            };
        }
    }

    let tt = t.trim_end();
    if let Some(cible) = tt.strip_prefix("![[").and_then(|r| r.strip_suffix("]]")) {
        if !cible.contains("]]") {
            return BlockKind::Embed {
                target: cible.to_string(),
            };
        }
    }
    if tt.starts_with("![") && tt.ends_with(')') {
        if let Some((_, url)) = tt.split_once("](") {
            return BlockKind::Embed {
                target: url.trim_end_matches(')').to_string(),
            };
        }
    }

    if t.contains(" :: ") && !t.starts_with('`') {
        return BlockKind::Flashcard;
    }

    BlockKind::Paragraph
}

/// `---`, `***`, `___`, with spaces between if one likes, three at least.
fn est_regle(t: &str) -> bool {
    let sans: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    sans.len() >= 3
        && (sans.chars().all(|c| c == '-')
            || sans.chars().all(|c| c == '*')
            || sans.chars().all(|c| c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORPUS: &[&str] = &[
        "",
        "a",
        "a\n",
        "a\r\nb\r\n",
        "a  \n\n\n  b\t\n",
        "# Titre\nTexte\n## Sous-titre",
        "#tag pas un titre\n####### sept\n",
        "- un\n  - deux\n\t- trois\n* quatre\n+ cinq\n",
        "1. un\n2) deux\n10. dix\n",
        "- [ ] à faire\n- [x] fait\n  - [X] aussi\n- [y] non\n",
        "> citation\n> suite\n\n> autre\n",
        "> [!def] Limite\n> Une suite converge…\n>\n> fin\nhors\n",
        "> [!thm]- Replié\n> x\n> [!note]+ Ouvert\n",
        "```rust\nfn main() {}\n# pas un titre\n```\napres\n",
        "````\n```\ndedans\n```\n````\n",
        "~~~\nsans fin",
        "$$\n\\int_0^1 f\n$$\n$$x$$\n$$ y $$\n",
        "$$ sans fin\nx",
        "| a | b |\n|---|:-:|\n| 1 | 2 |\n|x\nplus\n",
        "|pas une table|\nligne\n",
        ":::cols\na\n:::col\nb\n:::\n:::fold Détails\n:::cols\nx\n:::\n:::\n",
        ":::\nrien\n",
        "---\ntitle: X\ntags: [a]\n---\ncorps\n",
        "texte\n---\npas des propriétés\n",
        "***\n_ _ _\n--x--\n",
        "![[image.png|50%]]\n![alt](https://example.com/a.png)\n![[a]] et texte\n",
        "Question :: Réponse\n`code :: pas une carte`\n",
        "é\u{301}\r\n😀\n",
    ];

    #[test]
    fn every_text_comes_back_byte_for_byte() {
        for &texte in CORPUS {
            let blocs = parse(texte);
            assert_eq!(join(&blocs), texte, "{texte:?} → {blocs:?}");
            assert!(blocs.iter().all(|b| !b.text.is_empty()));
        }
    }

    fn kinds(texte: &str) -> Vec<BlockKind> {
        parse(texte).into_iter().map(|b| b.kind).collect()
    }

    #[test]
    fn line_blocks_are_told_apart() {
        use BlockKind::*;
        assert_eq!(
            kinds("# T\n#tag\n- a\n  - b\n1. c\n- [x] d\n\n---\nPlain\nQ :: A\n"),
            vec![
                Heading(1),
                Paragraph,
                Bullet { indent: 0 },
                Bullet { indent: 1 },
                Numbered { indent: 0, n: 1 },
                Task {
                    indent: 0,
                    done: true
                },
                Blank,
                Rule,
                Paragraph,
                Flashcard,
            ]
        );
    }

    #[test]
    fn several_lines_make_one_block_where_they_must() {
        let blocs = parse("> [!def]- Limite\n> texte\nsuite\n```py\nx\n```\n$$\na\n$$\n");
        assert_eq!(
            blocs[0].kind,
            BlockKind::Callout {
                kind: "def".into(),
                folded: Some(true),
                title: "Limite".into()
            }
        );
        assert_eq!(blocs[0].text, "> [!def]- Limite\n> texte\n");
        assert_eq!(blocs[1].kind, BlockKind::Paragraph);
        assert_eq!(blocs[2].kind, BlockKind::Code { lang: "py".into() });
        assert_eq!(blocs[2].text, "```py\nx\n```\n");
        assert_eq!(blocs[3].kind, BlockKind::Math);
        assert_eq!(blocs[3].text, "$$\na\n$$\n");
    }

    #[test]
    fn properties_only_at_the_very_top() {
        assert_eq!(kinds("---\na: 1\n---\nx\n")[0], BlockKind::Properties);
        assert_eq!(kinds("x\n---\na\n---\n")[1], BlockKind::Rule);
    }

    #[test]
    fn containers_nest() {
        let blocs = parse(":::fold Plus\n:::cols\na\n:::col\nb\n:::\n:::\nfin\n");
        assert_eq!(blocs.len(), 2);
        assert_eq!(
            blocs[0].kind,
            BlockKind::Container {
                kind: "fold".into(),
                title: "Plus".into()
            }
        );
    }

    #[test]
    fn content_and_line_endings() {
        let blocs = parse("a\r\nb");
        assert_eq!(blocs[0].content(), "a");
        assert_eq!(blocs[0].line_ending(), "\r\n");
        assert_eq!(blocs[1].content(), "b");
        assert_eq!(blocs[1].line_ending(), "");
    }

    #[test]
    fn offsets_are_found_in_their_block() {
        let blocs = parse("ab\ncd\n");
        assert_eq!(start_of(&blocs, 1), 3);
        assert_eq!(locate(&blocs, 0), (0, 0));
        assert_eq!(locate(&blocs, 2), (0, 2));
        assert_eq!(locate(&blocs, 4), (1, 1));
        assert_eq!(locate(&blocs, 99), (1, 2));
    }

    #[test]
    fn a_long_note_parses_fast() {
        let ligne = "- un élément de liste avec **du gras** et $x^2$\n";
        let texte = ligne.repeat(5_000);
        let debut = std::time::Instant::now();
        let blocs = parse(&texte);
        assert_eq!(blocs.len(), 5_000);
        assert!(debut.elapsed() < std::time::Duration::from_millis(200));
    }
}
