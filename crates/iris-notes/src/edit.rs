//! Keys made edits: a mark around a selection, a line start converted as it is typed,
//! Enter continuing a list, a block moved.
//!
//! Every function takes text and byte offsets and gives text and byte offsets back;
//! the editor applies them and parses again. Nothing here knows about the window.

use crate::block::{self, Block, BlockKind};
use crate::inline::{self, Colour, Mark};

/// Text after an edit, and the selection in it (`anchor` = `cursor` when none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub text: String,
    pub cursor: usize,
    pub anchor: usize,
}

impl Edit {
    fn at(text: String, cursor: usize) -> Self {
        Self {
            text,
            cursor,
            anchor: cursor,
        }
    }
}

fn bornes(anchor: usize, cursor: usize) -> (usize, usize) {
    (anchor.min(cursor), anchor.max(cursor))
}

/// Clamps an offset to a character boundary at or before it.
fn limite(text: &str, mut k: usize) -> usize {
    k = k.min(text.len());
    while !text.is_char_boundary(k) {
        k -= 1;
    }
    k
}

/// The delimiters of a mark typed around words.
pub fn delimiters(mark: &str) -> (&'static str, &'static str) {
    match mark {
        "bold" => ("**", "**"),
        "italic" => ("*", "*"),
        "underline" => ("__", "__"),
        "strike" => ("--", "--"),
        "highlight" => ("==", "=="),
        "code" => ("`", "`"),
        "math" => ("$", "$"),
        "sup" => ("^", "^"),
        "sub" => ("~", "~"),
        "link" => ("[[", "]]"),
        _ => ("", ""),
    }
}

/// Puts a mark around the selection, or takes it off when it is there already. With
/// nothing selected, the pair goes in with the cursor between.
pub fn toggle_mark(text: &str, anchor: usize, cursor: usize, mark: &str) -> Edit {
    let (ouvre, ferme) = delimiters(mark);
    let (s, e) = bornes(limite(text, anchor), limite(text, cursor));
    if ouvre.is_empty() {
        return Edit {
            text: text.to_string(),
            cursor,
            anchor,
        };
    }
    // Already around the selection: taken off.
    if s >= ouvre.len() && text[..s].ends_with(ouvre) && text[e..].starts_with(ferme) {
        let mut t = String::with_capacity(text.len());
        t.push_str(&text[..s - ouvre.len()]);
        t.push_str(&text[s..e]);
        t.push_str(&text[e + ferme.len()..]);
        return Edit {
            text: t,
            anchor: s - ouvre.len(),
            cursor: e - ouvre.len(),
        };
    }
    // The selection holds the marks: taken off.
    let choisi = &text[s..e];
    if choisi.len() >= ouvre.len() + ferme.len()
        && choisi.starts_with(ouvre)
        && choisi.ends_with(ferme)
    {
        let interieur = &choisi[ouvre.len()..choisi.len() - ferme.len()];
        let t = format!("{}{}{}", &text[..s], interieur, &text[e..]);
        return Edit {
            text: t,
            anchor: s,
            cursor: s + interieur.len(),
        };
    }
    let t = format!("{}{ouvre}{choisi}{ferme}{}", &text[..s], &text[e..]);
    Edit {
        text: t,
        anchor: s + ouvre.len(),
        cursor: e + ouvre.len(),
    }
}

/// The colour mark around `offset`, and its whole range (`{r}…{/}`).
pub fn colour_at(text: &str, offset: usize) -> Option<(Colour, std::ops::Range<usize>)> {
    let noeuds = inline::parse_inline(text);
    inline::marks_at(&noeuds, offset)
        .into_iter()
        .rev()
        .find_map(|(m, r)| match m {
            Mark::Colour(c) => Some((c, r)),
            _ => None,
        })
}

/// The highlight around `offset`, its colour and its whole range.
pub fn highlight_at(text: &str, offset: usize) -> Option<(Option<Colour>, std::ops::Range<usize>)> {
    let noeuds = inline::parse_inline(text);
    inline::marks_at(&noeuds, offset)
        .into_iter()
        .rev()
        .find_map(|(m, r)| match m {
            Mark::Highlight(c) => Some((c, r)),
            _ => None,
        })
}

/// Colours the selection, or the coloured words at the cursor when nothing is
/// selected; `None` takes the colour off.
pub fn set_colour(text: &str, anchor: usize, cursor: usize, colour: Option<Colour>) -> Edit {
    let (s, e) = bornes(limite(text, anchor), limite(text, cursor));
    if let Some((_, plage)) = colour_at(text, s).filter(|(_, r)| s == e || r.end >= e) {
        let src = &text[plage.clone()];
        let fin_code = src.find('}').map_or(0, |k| k + 1);
        let interieur = &src[fin_code..src.len() - 3];
        let nouveau = match colour {
            Some(c) => format!("{{{}}}{interieur}{{/}}", c.code()),
            None => interieur.to_string(),
        };
        let t = format!("{}{}{}", &text[..plage.start], nouveau, &text[plage.end..]);
        let ouvre = match colour {
            Some(c) => c.code().len() + 2,
            None => 0,
        };
        let debut = plage.start + ouvre;
        return Edit {
            text: t,
            anchor: debut,
            cursor: debut + interieur.len(),
        };
    }
    let Some(c) = colour else {
        return Edit {
            text: text.to_string(),
            cursor,
            anchor,
        };
    };
    if s == e {
        let t = format!("{}{{{}}}{{/}}{}", &text[..s], c.code(), &text[s..]);
        return Edit::at(t, s + c.code().len() + 2);
    }
    let ouvre = format!("{{{}}}", c.code());
    let t = format!("{}{ouvre}{}{{/}}{}", &text[..s], &text[s..e], &text[e..]);
    Edit {
        text: t,
        anchor: s + ouvre.len(),
        cursor: e + ouvre.len(),
    }
}

/// Highlights the selection (in a colour, or the default one), or recolours the
/// highlight at the cursor; `on == false` takes it off.
pub fn set_highlight(
    text: &str,
    anchor: usize,
    cursor: usize,
    colour: Option<Colour>,
    on: bool,
) -> Edit {
    let (s, e) = bornes(limite(text, anchor), limite(text, cursor));
    if let Some((_, plage)) = highlight_at(text, s).filter(|(_, r)| s == e || r.end >= e) {
        let src = &text[plage.clone()];
        let saut = if src[2..].starts_with('{') {
            src[2..].find('}').map_or(2, |k| 2 + k + 1)
        } else {
            2
        };
        let interieur = &src[saut..src.len() - 2];
        let nouveau = match (on, colour) {
            (false, _) => interieur.to_string(),
            (true, Some(c)) => format!("=={{{}}}{interieur}==", c.code()),
            (true, None) => format!("=={interieur}=="),
        };
        let ouvre = nouveau.len() - interieur.len() - if on { 2 } else { 0 };
        let t = format!("{}{}{}", &text[..plage.start], nouveau, &text[plage.end..]);
        return Edit {
            text: t,
            anchor: plage.start + ouvre,
            cursor: plage.start + ouvre + interieur.len(),
        };
    }
    if !on {
        return Edit {
            text: text.to_string(),
            cursor,
            anchor,
        };
    }
    let ouvre = match colour {
        Some(c) => format!("=={{{}}}", c.code()),
        None => "==".to_string(),
    };
    let t = format!("{}{ouvre}{}=={}", &text[..s], &text[s..e], &text[e..]);
    Edit {
        text: t,
        anchor: s + ouvre.len(),
        cursor: e + ouvre.len(),
    }
}

/// The callout kinds a line start makes, from what is typed after `>`: Obsidian's
/// names where it has one, so the notes read well there too.
pub fn callout_kind(typed: &str) -> Option<&'static str> {
    Some(match typed.to_ascii_lowercase().as_str() {
        "def" => "def",
        "thm" | "th" => "thm",
        "prop" => "prop",
        "lem" => "lem",
        "cor" => "cor",
        "preuve" | "proof" | "dem" => "proof",
        "ex" => "example",
        "exo" => "exo",
        "sol" => "sol",
        "q" => "question",
        "!" => "important",
        "res" | "sum" => "summary",
        "note" => "note",
        "tip" => "tip",
        "warn" | "att" => "warning",
        _ => return None,
    })
}

/// What typing a space at the start of a line turns it into: `[] ` a checkbox,
/// `>def ` a callout, `>-thm ` a folded one. `cursor` is just after the space typed.
pub fn line_start_conversion(text: &str, cursor: usize) -> Option<Edit> {
    let cursor = limite(text, cursor);
    let debut = &text[..cursor];
    let indent_len = debut.len() - debut.trim_start_matches([' ', '\t']).len();
    let indent = &debut[..indent_len];
    let tape = &debut[indent_len..];
    let reste = &text[cursor..];
    let remplace = |par: String| {
        let t = format!("{indent}{par}{reste}");
        Edit::at(t, indent.len() + par.len())
    };
    match tape {
        "[] " | "[ ] " => return Some(remplace("- [ ] ".into())),
        "[x] " | "[X] " => return Some(remplace("- [x] ".into())),
        _ => {}
    }
    let apres = tape.strip_prefix('>')?.strip_suffix(' ')?;
    let (plie, nom) = match apres.strip_prefix('-') {
        Some(n) => (true, n),
        None => (false, apres),
    };
    let genre = callout_kind(nom)?;
    Some(remplace(format!(
        "> [!{genre}]{} ",
        if plie { "-" } else { "" }
    )))
}

/// The signs two characters make as they are typed, the longer first: `->` an arrow,
/// `=>` an implication, `!=`, `<=`, `>=`; `<->` and `<=>` both ways.
const SIGNES: &[(&str, &str)] = &[
    ("←>", "↔"),
    ("≤>", "⇔"),
    ("->", "→"),
    ("<-", "←"),
    ("=>", "⇒"),
    ("!=", "≠"),
    ("<=", "≤"),
    (">=", "≥"),
];

/// What the characters just before `cursor` become once typed (`->` → `→`), outside
/// code and inline maths. `None`: they stay.
pub fn typographic(content: &str, cursor: usize) -> Option<Edit> {
    let cursor = limite(content, cursor);
    let avant = &content[..cursor];
    let ligne = &avant[avant.rfind('\n').map_or(0, |k| k + 1)..];
    // In `code` or `$maths$` being typed: as typed.
    let dollars = ligne.matches('$').count() - ligne.matches("\\$").count();
    if dollars % 2 == 1 || ligne.matches('`').count() % 2 == 1 {
        return None;
    }
    let (de, vers) = SIGNES.iter().find(|(de, _)| avant.ends_with(de))?;
    // `\->` keeps the characters.
    if avant[..avant.len() - de.len()].ends_with('\\') {
        return None;
    }
    let debut = cursor - de.len();
    let t = format!("{}{vers}{}", &content[..debut], &content[cursor..]);
    Some(Edit::at(t, debut + vers.len()))
}

/// `[words](address)`: the selection made a link to `url` (the clipboard's, when it
/// holds one), or the pair with the cursor where to type.
pub fn web_link(content: &str, anchor: usize, cursor: usize, url: Option<&str>) -> Edit {
    let (s, e) = bornes(limite(content, anchor), limite(content, cursor));
    let mots = &content[s..e];
    let adresse = url.unwrap_or("");
    let t = format!("{}[{mots}]({adresse}){}", &content[..s], &content[e..]);
    // Words and an address: after the link; words only: in the parentheses; nothing:
    // in the brackets.
    let c = if mots.is_empty() {
        s + 1
    } else if adresse.is_empty() {
        s + mots.len() + 3
    } else {
        s + mots.len() + adresse.len() + 4
    };
    Edit::at(t, c)
}

/// Enter at the end of a line that opens a block and is not closed — `$$`, ` ``` `,
/// ` ```rust `, `~~~`, `:::fold Title` — makes the whole block, closed, the cursor on
/// the empty line inside, as the `/` menu writes it.
pub fn close_fence(content: &str, cursor: usize) -> Option<Edit> {
    if cursor < content.len() || content.contains('\n') {
        return None;
    }
    let t = content.trim_end();
    let retrait = &t[..t.len() - t.trim_start().len()];
    let l = t.trim_start();
    let fermeture = if l == "$$" {
        "$$".to_string()
    } else if l.starts_with("```") || l.starts_with("~~~") {
        let c = l.chars().next()?;
        let barriere: String = l.chars().take_while(|x| *x == c).collect();
        // A run of the fence's character after its language is no fence.
        if l[barriere.len()..].contains(c) {
            return None;
        }
        barriere
    } else if let Some(genre) = l.strip_prefix(":::") {
        if genre.trim().is_empty() || genre.starts_with(':') {
            return None;
        }
        ":::".to_string()
    } else {
        return None;
    };
    let texte = format!("{t}\n{retrait}\n{retrait}{fermeture}");
    Some(Edit::at(texte, t.len() + 1 + retrait.len()))
}

/// What Enter does in a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enter {
    /// The block is cut at the cursor: `stays` keeps its place, `goes` makes the next
    /// block, with the cursor at `cursor` in it.
    Split {
        stays: String,
        goes: String,
        cursor: usize,
    },
    /// A new line inside the block (code, maths, a quote).
    Insert(Edit),
}

/// The prefix of a list item, and the next one's: `- `, `  1. ` → `  2. `,
/// `- [x] ` → `- [ ] `.
fn prefixe_de_liste(content: &str) -> Option<(usize, String)> {
    let indent_len = content.len() - content.trim_start_matches([' ', '\t']).len();
    let indent = &content[..indent_len];
    let t = &content[indent_len..];
    for marque in ["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] "] {
        if t.starts_with(marque) {
            let suivant = format!("{indent}{}[ ] ", &marque[..2]);
            return Some((indent_len + marque.len(), suivant));
        }
    }
    for marque in ["- ", "* ", "+ "] {
        if t.starts_with(marque) {
            return Some((indent_len + 2, format!("{indent}{marque}")));
        }
    }
    let chiffres = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=9).contains(&chiffres) {
        let apres = &t[chiffres..];
        if let Some(sep) = [". ", ") "].into_iter().find(|s| apres.starts_with(s)) {
            let n: u32 = t[..chiffres].parse().unwrap_or(0);
            return Some((indent_len + chiffres + 2, format!("{indent}{}{sep}", n + 1)));
        }
    }
    None
}

/// Enter at `cursor` in a block of `kind` whose source is `content`.
pub fn on_enter(kind: &BlockKind, content: &str, cursor: usize) -> Enter {
    let cursor = limite(content, cursor);
    match kind {
        BlockKind::Code { .. }
        | BlockKind::Math
        | BlockKind::Table
        | BlockKind::Properties
        | BlockKind::Container { .. } => {
            // The new line keeps the indentation of the one it leaves.
            let ligne = content[..cursor].rsplit('\n').next().unwrap_or("");
            let indent: String = ligne
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let t = format!("{}\n{indent}{}", &content[..cursor], &content[cursor..]);
            Enter::Insert(Edit::at(t, cursor + 1 + indent.len()))
        }
        BlockKind::Quote | BlockKind::Callout { .. } => {
            let debut_ligne = content[..cursor].rfind('\n').map_or(0, |k| k + 1);
            let ligne = &content[debut_ligne..];
            let ligne = ligne.split('\n').next().unwrap_or("");
            // An empty `>` line ends the quote.
            if ligne.trim() == ">" && debut_ligne > 0 {
                let fin_ligne = debut_ligne + ligne.len();
                let stays = format!("{}{}", &content[..debut_ligne - 1], &content[fin_ligne..]);
                return Enter::Split {
                    stays,
                    goes: String::new(),
                    cursor: 0,
                };
            }
            let t = format!("{}\n> {}", &content[..cursor], &content[cursor..]);
            Enter::Insert(Edit::at(t, cursor + 3))
        }
        _ => {
            // At the start of a heading's words (its `## ` is hidden): an empty line
            // above, the heading kept whole, rather than its words made a paragraph.
            if let BlockKind::Heading(_) = kind {
                let retrait = content.len() - content.trim_start_matches([' ', '\t']).len();
                let r = &content[retrait..];
                let dieses = r.bytes().take_while(|b| *b == b'#').count();
                let marque =
                    retrait + dieses + r[dieses..].bytes().take_while(|b| *b == b' ').count();
                if cursor <= marque && !content[marque..].trim().is_empty() {
                    return Enter::Split {
                        stays: String::new(),
                        goes: content.to_string(),
                        cursor: marque,
                    };
                }
            }
            if let Some((longueur, suivant)) = prefixe_de_liste(content) {
                let longueur = longueur.min(content.len());
                // An empty item ends the list: its prefix goes.
                if content[longueur..].trim().is_empty() && cursor >= longueur {
                    return Enter::Split {
                        stays: String::new(),
                        goes: String::new(),
                        cursor: 0,
                    };
                }
                // At the start of the item's words: an empty item above, this one kept
                // whole (a ticked box stays ticked).
                if cursor <= longueur {
                    let marque = &content[..longueur];
                    let vide = if marque.contains("[x]") || marque.contains("[X]") {
                        suivant
                    } else {
                        marque.to_string()
                    };
                    return Enter::Split {
                        stays: vide,
                        goes: content.to_string(),
                        cursor: longueur,
                    };
                }
                return Enter::Split {
                    stays: content[..cursor].to_string(),
                    goes: format!("{suivant}{}", &content[cursor..]),
                    cursor: suivant.len(),
                };
            }
            Enter::Split {
                stays: content[..cursor].to_string(),
                goes: content[cursor..].to_string(),
                cursor: 0,
            }
        }
    }
}

/// `x` → `# x` → `## x` → `### x` → `x`.
pub fn cycle_heading(content: &str) -> String {
    let dieses = content.chars().take_while(|c| *c == '#').count();
    let corps = content[dieses..].trim_start();
    match dieses {
        0 => format!("# {corps}"),
        1 | 2 => format!("{} {corps}", "#".repeat(dieses + 1)),
        _ => corps.to_string(),
    }
}

/// Nests a list item one level deeper, or one less (`out`). Other lines are left.
pub fn indent(content: &str, out: bool) -> String {
    if prefixe_de_liste(content).is_none() {
        return content.to_string();
    }
    if out {
        if let Some(r) = content.strip_prefix("  ") {
            return r.to_string();
        }
        if let Some(r) = content.strip_prefix('\t') {
            return r.to_string();
        }
        content.to_string()
    } else {
        format!("  {content}")
    }
}

/// Ticks a checkbox or unticks it; a line that is not one becomes one.
pub fn toggle_task(content: &str) -> String {
    let indent_len = content.len() - content.trim_start_matches([' ', '\t']).len();
    let (indent, t) = content.split_at(indent_len);
    for (de, vers) in [
        ("- [ ] ", "- [x] "),
        ("- [x] ", "- [ ] "),
        ("- [X] ", "- [ ] "),
    ] {
        if let Some(r) = t.strip_prefix(de) {
            return format!("{indent}{vers}{r}");
        }
    }
    for marque in ["- ", "* ", "+ "] {
        if let Some(r) = t.strip_prefix(marque) {
            return format!("{indent}- [ ] {r}");
        }
    }
    format!("{indent}- [ ] {t}")
}

/// A character typed with pairs in mind: around a selection it wraps it; before a
/// space or the end of the line it brings its closing one; typed just before the
/// same closing character it steps over it. `None`: type it as usual.
pub fn auto_pair(content: &str, anchor: usize, cursor: usize, typed: char) -> Option<Edit> {
    let (s, e) = bornes(limite(content, anchor), limite(content, cursor));
    let fermant = match typed {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '$' if s == e && !content[e..].starts_with('$') => {
            // On an empty line `$` is typed alone (`$$` there opens a maths block, which
            // a pair would make at once, the lines below drawn as maths); after an odd
            // number of them, it closes the formula.
            let ouverts = content[..e].matches('$').count() - content[..e].matches("\\$").count();
            if content.is_empty() || ouverts % 2 == 1 {
                return None;
            }
            '$'
        }
        '$' => '$',
        '`' => '`',
        ')' | ']' | '}' => {
            return (s == e && content[e..].starts_with(typed))
                .then(|| Edit::at(content.to_string(), e + 1));
        }
        _ => return None,
    };
    if s < e {
        let t = format!(
            "{}{typed}{}{fermant}{}",
            &content[..s],
            &content[s..e],
            &content[e..]
        );
        return Some(Edit {
            text: t,
            anchor: s + 1,
            cursor: e + 1,
        });
    }
    // `$` before a `$`: steps over the one already there.
    if typed == fermant && content[e..].starts_with(typed) {
        return Some(Edit::at(content.to_string(), e + 1));
    }
    let suivant = content[e..].chars().next();
    if suivant.is_some_and(|c| !c.is_whitespace() && !")]}".contains(c)) {
        return None;
    }
    let t = format!("{}{typed}{fermant}{}", &content[..e], &content[e..]);
    Some(Edit::at(t, e + 1))
}

// --- Whole-note operations ------------------------------------------------------------

/// A whole note after an operation on its blocks, and where the cursor goes: the
/// block's index and the offset in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEdit {
    pub text: String,
    pub block: usize,
    pub cursor: usize,
}

/// The line ending the note mostly uses.
pub fn line_ending_of(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn avec_fin(content: &str, fin: &str) -> String {
    format!("{content}{fin}")
}

/// Block `i`'s source replaced by `content` (its line ending kept).
pub fn replace_block(blocks: &[Block], i: usize, content: &str, cursor: usize) -> NoteEdit {
    let mut t = String::new();
    for (k, b) in blocks.iter().enumerate() {
        if k == i {
            t.push_str(content);
            t.push_str(b.line_ending());
        } else {
            t.push_str(&b.text);
        }
    }
    let global = block::start_of(blocks, i) + cursor;
    let nouveaux = block::parse(&t);
    let (bloc, local) = block::locate(&nouveaux, global);
    NoteEdit {
        text: t,
        block: bloc,
        cursor: local,
    }
}

/// Block `i` cut in two: `stays` in its place, `goes` after it.
pub fn split_block(blocks: &[Block], i: usize, stays: &str, goes: &str, cursor: usize) -> NoteEdit {
    let le = line_ending_of(&block::join(blocks));
    let mut t = String::new();
    for (k, b) in blocks.iter().enumerate() {
        if k == i {
            t.push_str(&avec_fin(stays, le));
            t.push_str(goes);
            t.push_str(if b.line_ending().is_empty() { "" } else { le });
        } else {
            t.push_str(&b.text);
        }
    }
    let global = block::start_of(blocks, i) + stays.len() + le.len() + cursor;
    let nouveaux = block::parse(&t);
    // Enter at the very end: the line made is empty and after every block; it is the
    // one past the last (the editor shows an empty line there).
    let (bloc, local) = if global >= t.len() && t.ends_with('\n') {
        (nouveaux.len(), 0)
    } else {
        block::locate(&nouveaux, global)
    };
    NoteEdit {
        text: t,
        block: bloc,
        cursor: local,
    }
}

/// Block `i` joined to the one before (Backspace at its start), the cursor where they
/// meet. `None` for the first block.
pub fn join_with_previous(blocks: &[Block], i: usize) -> Option<NoteEdit> {
    if i == 0 || i >= blocks.len() {
        return None;
    }
    let precedent = &blocks[i - 1];
    let mut t = String::new();
    for (k, b) in blocks.iter().enumerate() {
        if k == i - 1 {
            t.push_str(b.content());
        } else {
            t.push_str(&b.text);
        }
    }
    let global = block::start_of(blocks, i - 1) + precedent.content().len();
    let nouveaux = block::parse(&t);
    let (bloc, local) = block::locate(&nouveaux, global);
    Some(NoteEdit {
        text: t,
        block: bloc,
        cursor: local,
    })
}

/// Block `i` moved one place up or down, with its line ending sorted out when it was
/// or becomes the last.
pub fn move_block(blocks: &[Block], i: usize, up: bool) -> Option<NoteEdit> {
    let j = if up { i.checked_sub(1)? } else { i + 1 };
    if j >= blocks.len() || i >= blocks.len() {
        return None;
    }
    let le = line_ending_of(&block::join(blocks));
    let mut contenus: Vec<String> = blocks.iter().map(|b| b.content().to_string()).collect();
    let derniere_fin = blocks
        .last()
        .map(|b| b.line_ending().to_string())
        .unwrap_or_default();
    contenus.swap(i, j);
    let n = contenus.len();
    let mut t = String::new();
    for (k, c) in contenus.iter().enumerate() {
        t.push_str(c);
        t.push_str(if k + 1 == n { &derniere_fin } else { le });
    }
    let nouveaux = block::parse(&t);
    let global = contenus[..j]
        .iter()
        .map(|c| c.len() + le.len())
        .sum::<usize>();
    let (bloc, local) = block::locate(&nouveaux, global);
    Some(NoteEdit {
        text: t,
        block: bloc,
        cursor: local,
    })
}

/// Block `i` written twice; the cursor in the copy.
pub fn duplicate_block(blocks: &[Block], i: usize) -> NoteEdit {
    let le = line_ending_of(&block::join(blocks));
    let mut t = String::new();
    for (k, b) in blocks.iter().enumerate() {
        if k == i {
            t.push_str(&avec_fin(b.content(), le));
        }
        t.push_str(&b.text);
    }
    let global = block::start_of(blocks, i) + blocks[i].content().len() + le.len();
    let nouveaux = block::parse(&t);
    let (bloc, local) = block::locate(&nouveaux, global);
    NoteEdit {
        text: t,
        block: bloc,
        cursor: local,
    }
}

// --- Blocks selected whole -------------------------------------------------------------

/// Where `global` falls in `text` once parsed again: past the end of a text ending with
/// a line break, the empty line after the last block (as [`split_block`] does).
fn place(text: &str, global: usize) -> (usize, usize) {
    let nouveaux = block::parse(text);
    if global >= text.len() && text.ends_with('\n') {
        (nouveaux.len(), 0)
    } else {
        block::locate(&nouveaux, global)
    }
}

/// The source of blocks `first` to `last`, without the last one's line ending: what
/// copying them gives.
pub fn blocks_text(blocks: &[Block], first: usize, last: usize) -> String {
    let last = last.min(blocks.len().saturating_sub(1));
    if first > last || blocks.is_empty() {
        return String::new();
    }
    let mut t = String::new();
    for (k, b) in blocks[first..=last].iter().enumerate() {
        if first + k == last {
            t.push_str(b.content());
        } else {
            t.push_str(&b.text);
        }
    }
    t
}

/// Blocks `first` to `last` replaced by `text` (nothing: taken out), the cursor at the
/// end of what went in.
pub fn replace_blocks(blocks: &[Block], first: usize, last: usize, text: &str) -> NoteEdit {
    let last = last.min(blocks.len().saturating_sub(1));
    let le = line_ending_of(&block::join(blocks));
    let mut t = String::new();
    let mut global = 0;
    for (k, b) in blocks.iter().enumerate() {
        if k < first || k > last {
            t.push_str(&b.text);
            continue;
        }
        if k == first && !text.is_empty() {
            t.push_str(text);
            global = t.len();
            // The line ending of the last block replaced, or one when it had none and
            // something follows.
            let fin = blocks[last].line_ending();
            t.push_str(if fin.is_empty() && last + 1 < blocks.len() {
                le
            } else {
                fin
            });
        } else if k == first {
            global = t.len();
        }
    }
    let (bloc, local) = place(&t, global);
    NoteEdit {
        text: t,
        block: bloc,
        cursor: local,
    }
}

/// Blocks `first` to `last` moved one place up or down together; the cursor at the
/// start of the first of them, where it lands.
pub fn move_blocks(blocks: &[Block], first: usize, last: usize, up: bool) -> Option<NoteEdit> {
    if first > last || last >= blocks.len() {
        return None;
    }
    if (up && first == 0) || (!up && last + 1 >= blocks.len()) {
        return None;
    }
    let le = line_ending_of(&block::join(blocks));
    let mut contenus: Vec<String> = blocks.iter().map(|b| b.content().to_string()).collect();
    let derniere_fin = blocks
        .last()
        .map(|b| b.line_ending().to_string())
        .unwrap_or_default();
    let nouveau_premier = if up {
        contenus[first - 1..=last].rotate_left(1);
        first - 1
    } else {
        contenus[first..=last + 1].rotate_right(1);
        first + 1
    };
    let n = contenus.len();
    let mut t = String::new();
    let mut global = 0;
    for (k, c) in contenus.iter().enumerate() {
        if k == nouveau_premier {
            global = t.len();
        }
        t.push_str(c);
        t.push_str(if k + 1 == n { &derniere_fin } else { le });
    }
    let (bloc, local) = place(&t, global);
    Some(NoteEdit {
        text: t,
        block: bloc,
        cursor: local,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_are_made_as_they_are_typed() {
        let e = typographic("a -> b", 4).unwrap();
        assert_eq!(e.text, "a → b");
        assert_eq!(e.cursor, 2 + "→".len());
        assert_eq!(typographic("x >=", 4).unwrap().text, "x ≥");
        assert_eq!(typographic("A ≤>", "A ≤>".len()).unwrap().text, "A ⇔");
        assert!(typographic("$a->", 4).is_none(), "in maths, as typed");
        assert!(typographic("`a->", 4).is_none(), "in code, as typed");
        assert!(typographic(r"a \->", 5).is_none());
        assert!(typographic("a - b", 5).is_none());
    }

    #[test]
    fn a_web_link_is_made_around_words() {
        let e = web_link("voir ici", 5, 8, Some("https://example.com"));
        assert_eq!(e.text, "voir [ici](https://example.com)");
        assert_eq!(e.cursor, e.text.len());
        let e = web_link("voir ici", 5, 8, None);
        assert_eq!(e.text, "voir [ici]()");
        assert_eq!(e.cursor, 11);
        let e = web_link("a", 1, 1, None);
        assert_eq!((e.text.as_str(), e.cursor), ("a[]()", 2));
    }

    #[test]
    fn enter_after_an_opening_line_closes_its_block() {
        let e = close_fence("$$", 2).unwrap();
        assert_eq!(e.text, "$$\n\n$$");
        assert_eq!(e.cursor, 3);
        let e = close_fence("```rust", 7).unwrap();
        assert_eq!(e.text, "```rust\n\n```");
        assert_eq!(e.cursor, 8);
        assert_eq!(close_fence("~~~~", 4).unwrap().text, "~~~~\n\n~~~~");
        assert_eq!(
            close_fence(":::fold Plus", 12).unwrap().text,
            ":::fold Plus\n\n:::"
        );
        assert!(close_fence("$$x$$", 5).is_none());
        assert!(close_fence("```a```", 7).is_none());
        assert!(close_fence(":::", 3).is_none());
        assert!(close_fence("$$", 1).is_none(), "only at the line's end");
    }

    #[test]
    fn enter_at_the_start_of_a_heading_or_an_item_keeps_it_whole() {
        assert_eq!(
            on_enter(&BlockKind::Heading(2), "## Titre", 3),
            Enter::Split {
                stays: String::new(),
                goes: "## Titre".into(),
                cursor: 3
            }
        );
        assert_eq!(
            on_enter(
                &BlockKind::Task {
                    indent: 0,
                    done: true
                },
                "- [x] fait",
                6
            ),
            Enter::Split {
                stays: "- [ ] ".into(),
                goes: "- [x] fait".into(),
                cursor: 6
            }
        );
        assert_eq!(
            on_enter(&BlockKind::Numbered { indent: 0, n: 3 }, "3. trois", 3),
            Enter::Split {
                stays: "3. ".into(),
                goes: "3. trois".into(),
                cursor: 3
            }
        );
    }

    #[test]
    fn a_dollar_opens_a_formula_or_a_maths_block() {
        assert!(auto_pair("", 0, 0, '$').is_none(), "`$$` can be typed");
        assert!(auto_pair("$", 1, 1, '$').is_none());
        assert!(auto_pair("$x", 2, 2, '$').is_none(), "it closes `$x`");
        assert_eq!(auto_pair("a ", 2, 2, '$').unwrap().text, "a $$");
    }

    #[test]
    fn blocks_selected_whole_are_copied_replaced_and_moved() {
        let blocs = block::parse("# A\n$$\nx\n$$\n- b\nc");
        assert_eq!(blocks_text(&blocs, 1, 2), "$$\nx\n$$\n- b");
        assert_eq!(blocks_text(&blocs, 3, 3), "c");

        let e = replace_blocks(&blocs, 1, 2, "");
        assert_eq!(e.text, "# A\nc");
        assert_eq!((e.block, e.cursor), (1, 0));
        let e = replace_blocks(&blocs, 1, 2, "new");
        assert_eq!(e.text, "# A\nnew\nc");
        assert_eq!((e.block, e.cursor), (1, 3));
        // The last block, which had no line ending, replaced: none added.
        let e = replace_blocks(&blocs, 3, 3, "d");
        assert_eq!(e.text, "# A\n$$\nx\n$$\n- b\nd");
        // Everything taken out.
        let e = replace_blocks(&blocs, 0, 3, "");
        assert_eq!(e.text, "");

        let e = move_blocks(&blocs, 1, 2, true).unwrap();
        assert_eq!(e.text, "$$\nx\n$$\n- b\n# A\nc");
        assert_eq!(e.block, 0);
        let e = move_blocks(&blocs, 0, 1, false).unwrap();
        assert_eq!(e.text, "- b\n# A\n$$\nx\n$$\nc");
        assert_eq!(e.block, 1);
        assert!(move_blocks(&blocs, 0, 1, true).is_none());
        assert!(move_blocks(&blocs, 2, 3, false).is_none());
    }

    #[test]
    fn a_mark_wraps_and_unwraps_a_selection() {
        let e = toggle_mark("un mot ici", 3, 6, "bold");
        assert_eq!(e.text, "un **mot** ici");
        assert_eq!((e.anchor, e.cursor), (5, 8));
        let e = toggle_mark(&e.text, e.anchor, e.cursor, "bold");
        assert_eq!(e.text, "un mot ici");
        assert_eq!((e.anchor, e.cursor), (3, 6));
        let e = toggle_mark("un __mot__", 3, 10, "underline");
        assert_eq!(e.text, "un mot");
    }

    #[test]
    fn a_mark_with_nothing_selected_brings_its_pair() {
        let e = toggle_mark("ab", 1, 1, "strike");
        assert_eq!(e.text, "a----b");
        assert_eq!(e.cursor, 3);
    }

    #[test]
    fn colours_are_set_changed_and_taken_off() {
        let e = set_colour("un mot", 3, 6, Some(Colour::Red));
        assert_eq!(e.text, "un {r}mot{/}");
        let e = set_colour(&e.text, 7, 7, Some(Colour::Blue));
        assert_eq!(e.text, "un {b}mot{/}");
        let e = set_colour(&e.text, 7, 7, None);
        assert_eq!(e.text, "un mot");
        assert_eq!(colour_at("a {g}b{/}", 6).map(|c| c.0), Some(Colour::Green));
    }

    #[test]
    fn highlights_take_a_colour() {
        let e = set_highlight("x mot", 2, 5, None, true);
        assert_eq!(e.text, "x ==mot==");
        let e = set_highlight(&e.text, 5, 5, Some(Colour::Green), true);
        assert_eq!(e.text, "x =={g}mot==");
        let e = set_highlight(&e.text, 7, 7, None, false);
        assert_eq!(e.text, "x mot");
    }

    #[test]
    fn line_starts_become_blocks() {
        assert_eq!(line_start_conversion("[] ", 3).unwrap().text, "- [ ] ");
        assert_eq!(
            line_start_conversion("  [x] fait", 6).unwrap().text,
            "  - [x] fait"
        );
        let e = line_start_conversion(">def Limite", 5).unwrap();
        assert_eq!(e.text, "> [!def] Limite");
        assert_eq!(e.cursor, 9);
        assert_eq!(
            line_start_conversion(">-preuve ", 9).unwrap().text,
            "> [!proof]- "
        );
        assert_eq!(
            line_start_conversion(">! ", 3).unwrap().text,
            "> [!important] "
        );
        assert!(line_start_conversion(">rien ", 6).is_none());
        assert!(line_start_conversion("texte ", 6).is_none());
    }

    #[test]
    fn enter_continues_lists_and_ends_them() {
        assert_eq!(
            on_enter(&BlockKind::Bullet { indent: 0 }, "- un", 4),
            Enter::Split {
                stays: "- un".into(),
                goes: "- ".into(),
                cursor: 2
            }
        );
        assert_eq!(
            on_enter(&BlockKind::Numbered { indent: 1, n: 9 }, "  9. neuf", 9),
            Enter::Split {
                stays: "  9. neuf".into(),
                goes: "  10. ".into(),
                cursor: 6
            }
        );
        assert_eq!(
            on_enter(
                &BlockKind::Task {
                    indent: 0,
                    done: true
                },
                "- [x] fait",
                10
            ),
            Enter::Split {
                stays: "- [x] fait".into(),
                goes: "- [ ] ".into(),
                cursor: 6
            }
        );
        // An empty item: the list ends there.
        assert_eq!(
            on_enter(&BlockKind::Bullet { indent: 0 }, "- ", 2),
            Enter::Split {
                stays: String::new(),
                goes: String::new(),
                cursor: 0
            }
        );
        assert_eq!(
            on_enter(&BlockKind::Paragraph, "abcd", 2),
            Enter::Split {
                stays: "ab".into(),
                goes: "cd".into(),
                cursor: 0
            }
        );
    }

    #[test]
    fn enter_inside_code_and_quotes_stays_in_the_block() {
        let Enter::Insert(e) = on_enter(
            &BlockKind::Code {
                lang: String::new(),
            },
            "```\n  x",
            7,
        ) else {
            panic!("a new line")
        };
        assert_eq!(e.text, "```\n  x\n  ");
        let Enter::Insert(e) = on_enter(&BlockKind::Quote, "> a", 3) else {
            panic!("a new line")
        };
        assert_eq!(e.text, "> a\n> ");
        assert_eq!(
            on_enter(&BlockKind::Quote, "> a\n>", 5),
            Enter::Split {
                stays: "> a".into(),
                goes: String::new(),
                cursor: 0
            }
        );
    }

    #[test]
    fn headings_cycle_and_lists_nest() {
        assert_eq!(cycle_heading("Titre"), "# Titre");
        assert_eq!(cycle_heading("# Titre"), "## Titre");
        assert_eq!(cycle_heading("### Titre"), "Titre");
        assert_eq!(indent("- a", false), "  - a");
        assert_eq!(indent("  - a", true), "- a");
        assert_eq!(indent("texte", false), "texte");
        assert_eq!(toggle_task("- [ ] a"), "- [x] a");
        assert_eq!(toggle_task("  - [x] a"), "  - [ ] a");
        assert_eq!(toggle_task("- a"), "- [ ] a");
        assert_eq!(toggle_task("a"), "- [ ] a");
    }

    #[test]
    fn pairs_close_themselves() {
        assert_eq!(auto_pair("x", 1, 1, '(').unwrap().text, "x()");
        assert_eq!(auto_pair("ab", 0, 2, '$').unwrap().text, "$ab$");
        assert_eq!(auto_pair("()", 1, 1, ')').unwrap().cursor, 2);
        assert!(
            auto_pair("ab", 0, 0, '(').is_none(),
            "before a word: as usual"
        );
        assert_eq!(auto_pair("$x$", 2, 2, '$').unwrap().cursor, 3);
    }

    #[test]
    fn whole_note_operations_keep_the_rest() {
        let blocs = block::parse("a\nbc\nd");
        let e = split_block(&blocs, 1, "b", "c", 0);
        assert_eq!(e.text, "a\nb\nc\nd");
        assert_eq!((e.block, e.cursor), (2, 0));

        // Enter at the end of the last line: the cursor on the empty line after it.
        let e = split_block(&block::parse("a"), 0, "a", "", 0);
        assert_eq!(e.text, "a\n");
        assert_eq!((e.block, e.cursor), (1, 0));

        let e = join_with_previous(&block::parse("a\nb\n"), 1).unwrap();
        assert_eq!(e.text, "ab\n");
        assert_eq!((e.block, e.cursor), (0, 1));

        let e = move_block(&block::parse("a\nb\nc"), 2, true).unwrap();
        assert_eq!(e.text, "a\nc\nb");
        assert_eq!(e.block, 1);

        let e = duplicate_block(&block::parse("a\r\nb\r\n"), 0);
        assert_eq!(e.text, "a\r\na\r\nb\r\n");
        assert_eq!(e.block, 1);

        let e = replace_block(&block::parse("a\nb\n"), 1, "# B", 3);
        assert_eq!(e.text, "a\n# B\n");
        assert_eq!((e.block, e.cursor), (1, 3));
    }
}
