//! The marks of a line: what makes words bold, coloured, linked.
//!
//! Three layers are read. Markdown's own (`**bold**`, `*italic*`, `` `code` ``,
//! `[text](url)`); Obsidian's (`[[links]]`, `==highlight==`, `#tags`); and Iris's,
//! short to type by hand: `__underline__`, `--strikethrough--` (`~~` also read),
//! `{r}colour{/}`, `x^2^`, `H~2~O`, `$maths$`, `^[footnote]`, `@2026-10-08`.
//!
//! A mark needs something other than a space just inside it, so `a -- b` stays a dash
//! and `2 * 3` a product; `*`, `__`, `--`, `^` and `~` do not open inside a word, so
//! `2*3*4`, `snake__case` and `a--b` stay as they are. `\` before any punctuation
//! keeps it literal. Marks do not cross one another: the first to close wins.
//!
//! The result is a tree ([`Node`]) with the source range of each part, written out
//! for Slint's `StyledText` ([`to_slint_markdown`]) or as HTML ([`to_html`]).

use std::ops::Range;

/// The seven colours text can wear, by the letter typed in `{r}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Colour {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Grey,
}

impl Colour {
    pub const ALL: [Colour; 7] = [
        Colour::Red,
        Colour::Orange,
        Colour::Yellow,
        Colour::Green,
        Colour::Blue,
        Colour::Purple,
        Colour::Grey,
    ];

    /// `r`, `red`, `rouge`: the letter, the English and the French names.
    pub fn from_code(code: &str) -> Option<Colour> {
        Some(match code.trim().to_ascii_lowercase().as_str() {
            "r" | "red" | "rouge" => Colour::Red,
            "o" | "orange" => Colour::Orange,
            "y" | "j" | "yellow" | "jaune" => Colour::Yellow,
            "g" | "v" | "green" | "vert" => Colour::Green,
            "b" | "blue" | "bleu" => Colour::Blue,
            "p" | "purple" | "violet" => Colour::Purple,
            "n" | "grey" | "gray" | "gris" => Colour::Grey,
            _ => return None,
        })
    }

    /// The letter written in a mark.
    pub fn code(self) -> &'static str {
        match self {
            Colour::Red => "r",
            Colour::Orange => "o",
            Colour::Yellow => "y",
            Colour::Green => "g",
            Colour::Blue => "b",
            Colour::Purple => "p",
            Colour::Grey => "n",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Colour::Red => "Red",
            Colour::Orange => "Orange",
            Colour::Yellow => "Yellow",
            Colour::Green => "Green",
            Colour::Blue => "Blue",
            Colour::Purple => "Purple",
            Colour::Grey => "Grey",
        }
    }

    pub fn index(self) -> usize {
        Colour::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// A mark over part of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    /// `==x==`, or `=={g}x==` in a colour.
    Highlight(Option<Colour>),
    Colour(Colour),
    Sup,
    Sub,
    Code,
    /// `$…$`: the LaTeX source is the node's text.
    Math,
    /// `[[target]]`, `[[target|label]]`, `[[target#heading]]`; `embed` for `![[…]]`.
    NoteLink {
        target: String,
        embed: bool,
    },
    WebLink {
        url: String,
    },
    Tag(String),
    Date(chrono::NaiveDate),
    /// `^[text]`: the footnote's text is its children.
    Footnote,
}

/// A part of a line: plain text, or a mark over parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Text {
        text: String,
        range: Range<usize>,
    },
    Mark {
        mark: Mark,
        children: Vec<Node>,
        /// The whole mark in the source, its delimiters included.
        range: Range<usize>,
    },
}

/// A run of text with every mark over it, for whoever wants it flat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub marks: Vec<Mark>,
    pub range: Range<usize>,
}

/// The colours a line is drawn in, as `0xRRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub colours: [u32; 7],
    pub highlight: [u32; 7],
    /// A highlight with no colour of its own.
    pub highlight_default: u32,
    pub tag: u32,
    pub math: u32,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            colours: [
                0xe03e3e, 0xd9730d, 0xc29b00, 0x0f9d58, 0x2f6fde, 0x9b51e0, 0x7d7a75,
            ],
            highlight: [
                0xc4352e, 0xb8620b, 0xa07e00, 0x0b7d46, 0x2559b8, 0x7c3fb8, 0x67645f,
            ],
            highlight_default: 0xb8860b,
            tag: 0x2f6fde,
            math: 0x5b4fc4,
        }
    }
}

/// Reads the marks of `src`.
pub fn parse_inline(src: &str) -> Vec<Node> {
    let mut sortie = Vec::new();
    analyser(src, 0, src.len(), &mut sortie);
    sortie
}

/// Every run of text with its marks, in order.
pub fn spans(nodes: &[Node]) -> Vec<Span> {
    let mut sortie = Vec::new();
    aplatir(nodes, &mut Vec::new(), &mut sortie);
    sortie
}

fn aplatir(nodes: &[Node], marques: &mut Vec<Mark>, sortie: &mut Vec<Span>) {
    for n in nodes {
        match n {
            Node::Text { text, range } => sortie.push(Span {
                text: text.clone(),
                marks: marques.clone(),
                range: range.clone(),
            }),
            Node::Mark { mark, children, .. } => {
                marques.push(mark.clone());
                aplatir(children, marques, sortie);
                marques.pop();
            }
        }
    }
}

/// The innermost marks over byte `offset`, innermost last, with their whole ranges.
pub fn marks_at(nodes: &[Node], offset: usize) -> Vec<(Mark, Range<usize>)> {
    let mut sortie = Vec::new();
    chercher(nodes, offset, &mut sortie);
    sortie
}

fn chercher(nodes: &[Node], offset: usize, sortie: &mut Vec<(Mark, Range<usize>)>) {
    for n in nodes {
        if let Node::Mark {
            mark,
            children,
            range,
        } = n
        {
            if range.start <= offset && offset <= range.end {
                sortie.push((mark.clone(), range.clone()));
                chercher(children, offset, sortie);
                return;
            }
        }
    }
}

/// The plain text of a line, marks taken off.
pub fn plain_text(nodes: &[Node]) -> String {
    spans(nodes).into_iter().map(|s| s.text).collect()
}

// --- The scanner ---------------------------------------------------------------------

/// Characters `\` makes literal.
fn echappable(c: char) -> bool {
    c.is_ascii_punctuation()
}

fn mot(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric())
}

fn espace(c: Option<char>) -> bool {
    c.is_none_or(char::is_whitespace)
}

fn avant(src: &str, i: usize) -> Option<char> {
    src[..i].chars().next_back()
}

fn apres(src: &str, i: usize) -> Option<char> {
    src[i..].chars().next()
}

/// Reads `src[debut..fin]` into `sortie`.
fn analyser(src: &str, debut: usize, fin: usize, sortie: &mut Vec<Node>) {
    let mut texte_depuis = debut;
    let mut i = debut;
    let pousser_texte = |de: usize, a: usize, sortie: &mut Vec<Node>| {
        if a > de {
            if let Some(Node::Text { text, range }) = sortie.last_mut() {
                if range.end == de {
                    text.push_str(&src[de..a]);
                    range.end = a;
                    return;
                }
            }
            sortie.push(Node::Text {
                text: src[de..a].to_string(),
                range: de..a,
            });
        }
    };

    while i < fin {
        let reste = &src[i..fin];
        let c = reste.chars().next().unwrap_or(' ');

        // `\*`: the character itself.
        if c == '\\' {
            if let Some(suivant) = reste[1..].chars().next() {
                if echappable(suivant) {
                    pousser_texte(texte_depuis, i, sortie);
                    let l = suivant.len_utf8();
                    sortie.push(Node::Text {
                        text: suivant.to_string(),
                        range: i + 1..i + 1 + l,
                    });
                    i += 1 + l;
                    texte_depuis = i;
                    continue;
                }
            }
        }

        if let Some((noeud, longueur)) = reconnaitre(src, i, fin) {
            pousser_texte(texte_depuis, i, sortie);
            sortie.push(noeud);
            i += longueur;
            texte_depuis = i;
            continue;
        }
        i += c.len_utf8();
    }
    pousser_texte(texte_depuis, fin, sortie);
}

/// A mark starting at `i`, and how long it is in the source.
fn reconnaitre(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    let reste = &src[i..fin];
    let c = reste.chars().next()?;
    match c {
        '`' => code(src, i, fin),
        '$' => maths(src, i, fin),
        '!' if reste.starts_with("![[") => {
            lien_note(src, i + 1, fin, true).map(|(n, l)| (n, l + 1))
        }
        '[' if reste.starts_with("[[") => lien_note(src, i, fin, false),
        '[' => lien_web(src, i, fin),
        '^' if reste.starts_with("^[") => note_de_bas(src, i, fin),
        '^' => entoure(src, i, fin, "^", Mark::Sup, true, true),
        '{' => couleur(src, i, fin),
        '=' if reste.starts_with("==") => surligne(src, i, fin),
        '*' if reste.starts_with("**") => entoure(src, i, fin, "**", Mark::Bold, true, false),
        '*' => entoure(src, i, fin, "*", Mark::Italic, false, false),
        '_' if reste.starts_with("__") => entoure(src, i, fin, "__", Mark::Underline, false, false),
        '-' if reste.starts_with("--") && !reste.starts_with("---") => {
            entoure(src, i, fin, "--", Mark::Strike, false, false)
        }
        '~' if reste.starts_with("~~") => entoure(src, i, fin, "~~", Mark::Strike, true, false),
        '~' => entoure(src, i, fin, "~", Mark::Sub, true, true),
        '#' => etiquette(src, i, fin),
        '@' => date(src, i, fin),
        'h' if reste.starts_with("https://") || reste.starts_with("http://") => {
            adresse(src, i, fin)
        }
        _ => None,
    }
}

/// `delim … delim` around inner marks. `dans_un_mot`: may open and close inside a
/// word (`**`, `~~`); `sans_espace`: may hold no space at all (`^2^`, `~2~`).
fn entoure(
    src: &str,
    i: usize,
    fin: usize,
    delim: &str,
    mark: Mark,
    dans_un_mot: bool,
    sans_espace: bool,
) -> Option<(Node, usize)> {
    let debut_interieur = i + delim.len();
    if espace(apres(src, debut_interieur)) {
        return None;
    }
    if !dans_un_mot && mot(avant(src, i)) {
        return None;
    }
    // Not the start of a longer run of the same character: `***` is bold then italic.
    let premier = delim.chars().next()?;
    if delim.len() == 1 && apres(src, debut_interieur) == Some(premier) {
        return None;
    }
    let mut k = debut_interieur;
    while k < fin {
        let r = &src[k..fin];
        // Code and maths are not looked into for a closing mark.
        if r.starts_with('`') {
            if let Some((_, l)) = code(src, k, fin) {
                k += l;
                continue;
            }
        }
        if let Some(apres_barre) = r.strip_prefix('\\') {
            k += 1 + apres_barre.chars().next().map_or(0, char::len_utf8);
            continue;
        }
        let ch = r.chars().next()?;
        if sans_espace && ch.is_whitespace() {
            return None;
        }
        if r.starts_with(delim)
            && k > debut_interieur
            && !espace(avant(src, k))
            && (dans_un_mot || !mot(apres(src, k + delim.len())))
            && !(delim.len() == 1 && apres(src, k + 1) == Some(premier))
            && !(delim == "--" && r.starts_with("---"))
        {
            let mut enfants = Vec::new();
            if matches!(mark, Mark::Sup | Mark::Sub) {
                enfants.push(Node::Text {
                    text: src[debut_interieur..k].to_string(),
                    range: debut_interieur..k,
                });
            } else {
                analyser(src, debut_interieur, k, &mut enfants);
            }
            let fin_mark = k + delim.len();
            return Some((
                Node::Mark {
                    mark,
                    children: enfants,
                    range: i..fin_mark,
                },
                fin_mark - i,
            ));
        }
        k += ch.len_utf8();
    }
    None
}

/// `` `code` `` (or ``` `` a ` b `` ```): as many backticks to close as opened.
fn code(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    let r = &src[i..fin];
    let n = r.chars().take_while(|c| *c == '`').count();
    let delim = &r[..n];
    let corps = &r[n..];
    let pos = corps.find(delim)?;
    // Exactly as many: a longer run does not close.
    if corps[pos + n..].starts_with('`') {
        return None;
    }
    let debut = i + n;
    let texte = &src[debut..debut + pos];
    Some((
        Node::Mark {
            mark: Mark::Code,
            children: vec![Node::Text {
                text: texte.to_string(),
                range: debut..debut + pos,
            }],
            range: i..debut + pos + n,
        },
        n + pos + n,
    ))
}

/// `$x$`: not `$$`, nothing but a non-space just inside, and not money (`$5 and $6`:
/// a closing `$` followed by a digit does not close). Spaces just inside are allowed
/// when what is inside holds a LaTeX command (`$ \forall x \in E $`): money never does.
fn maths(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    let r = &src[i..fin];
    if r.starts_with("$$") {
        return None;
    }
    let ouvre_sur_espace = espace(apres(src, i + 1));
    let mut k = i + 1;
    while k < fin {
        let ch = src[k..].chars().next()?;
        if ch == '\\' {
            k += 1 + src[k + 1..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        if ch == '$' {
            let commande = src[i + 1..k]
                .split('\\')
                .skip(1)
                .any(|s| s.starts_with(|c: char| c.is_ascii_alphabetic()));
            if ((ouvre_sur_espace || espace(avant(src, k))) && !commande)
                || apres(src, k + 1).is_some_and(|c| c.is_ascii_digit())
                || src[i + 1..k].trim().is_empty()
            {
                return None;
            }
            return Some((
                Node::Mark {
                    mark: Mark::Math,
                    children: vec![Node::Text {
                        text: src[i + 1..k].to_string(),
                        range: i + 1..k,
                    }],
                    range: i..k + 1,
                },
                k + 1 - i,
            ));
        }
        k += ch.len_utf8();
    }
    None
}

/// `[[target]]`, `[[target|label]]`; `embed` for `![[…]]` (`i` at the first `[`).
fn lien_note(src: &str, i: usize, fin: usize, embed: bool) -> Option<(Node, usize)> {
    let r = &src[i..fin];
    let interieur = &r[2..];
    let pos = interieur.find("]]")?;
    let dedans = &interieur[..pos];
    if dedans.trim().is_empty() || dedans.contains('\n') || dedans.contains("[[") {
        return None;
    }
    let debut = i + 2;
    let (cible, libelle) = match dedans.split_once('|') {
        Some((c, l)) => (c, Some((l, debut + c.len() + 1))),
        None => (dedans, None),
    };
    let (texte, plage) = match libelle {
        Some((l, d)) => (l.to_string(), d..d + l.len()),
        None => {
            // `Note#Heading` reads "Note › Heading"; `#Heading` in the same note, the
            // heading alone.
            let affiche = match cible.split_once('#') {
                Some(("", h)) => h.to_string(),
                Some((n, h)) => format!("{n} › {h}"),
                None => cible.to_string(),
            };
            (affiche, debut..debut + cible.len())
        }
    };
    Some((
        Node::Mark {
            mark: Mark::NoteLink {
                target: cible.trim().to_string(),
                embed,
            },
            children: vec![Node::Text {
                text: texte,
                range: plage,
            }],
            range: if embed {
                i - 1..i + 2 + pos + 2
            } else {
                i..i + 2 + pos + 2
            },
        },
        2 + pos + 2,
    ))
}

/// `[text](url)`.
fn lien_web(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    let r = &src[i..fin];
    let fin_texte = r.find("](")?;
    if fin_texte == 0 || r[1..fin_texte].contains('[') {
        return None;
    }
    let apres_url = &r[fin_texte + 2..];
    let fin_url = apres_url.find(')')?;
    let url = apres_url[..fin_url].trim();
    if url.contains(char::is_whitespace) {
        return None;
    }
    let mut enfants = Vec::new();
    analyser(src, i + 1, i + fin_texte, &mut enfants);
    let longueur = fin_texte + 2 + fin_url + 1;
    Some((
        Node::Mark {
            mark: Mark::WebLink {
                url: url.to_string(),
            },
            children: enfants,
            range: i..i + longueur,
        },
        longueur,
    ))
}

/// A bare `https://…` address, its trailing punctuation left out.
fn adresse(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    if mot(avant(src, i)) {
        return None;
    }
    let r = &src[i..fin];
    let mut longueur = r.find(char::is_whitespace).unwrap_or(r.len());
    while longueur > 0 && r[..longueur].ends_with(['.', ',', ';', ':', ')', '!', '?', '\'', '"']) {
        longueur -= 1;
    }
    let url = &r[..longueur];
    if url.len() <= "https://".len() {
        return None;
    }
    Some((
        Node::Mark {
            mark: Mark::WebLink {
                url: url.to_string(),
            },
            children: vec![Node::Text {
                text: url.to_string(),
                range: i..i + longueur,
            }],
            range: i..i + longueur,
        },
        longueur,
    ))
}

/// `^[text]`.
fn note_de_bas(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    let r = &src[i..fin];
    let mut profondeur = 0;
    for (k, ch) in r.char_indices().skip(1) {
        match ch {
            '[' => profondeur += 1,
            ']' => {
                profondeur -= 1;
                if profondeur == 0 {
                    let mut enfants = Vec::new();
                    analyser(src, i + 2, i + k, &mut enfants);
                    return Some((
                        Node::Mark {
                            mark: Mark::Footnote,
                            children: enfants,
                            range: i..i + k + 1,
                        },
                        k + 1,
                    ));
                }
            }
            _ => {}
        }
    }
    None
}

/// `{r}…{/}`: a colour, up to its `{/}`.
fn couleur(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    let r = &src[i..fin];
    let fermeture = r.find('}')?;
    let teinte = Colour::from_code(&r[1..fermeture])?;
    let debut = i + fermeture + 1;
    let pos = src[debut..fin].find("{/}")?;
    let mut enfants = Vec::new();
    analyser(src, debut, debut + pos, &mut enfants);
    let longueur = fermeture + 1 + pos + 3;
    Some((
        Node::Mark {
            mark: Mark::Colour(teinte),
            children: enfants,
            range: i..i + longueur,
        },
        longueur,
    ))
}

/// `==x==`, or `=={g}x==`.
fn surligne(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    let r = &src[i..fin];
    let (teinte, saut) = match r[2..].strip_prefix('{') {
        Some(apres) => match apres.find('}') {
            Some(f) => match Colour::from_code(&apres[..f]) {
                Some(c) => (Some(c), 2 + f + 2),
                None => (None, 2),
            },
            None => (None, 2),
        },
        None => (None, 2),
    };
    let debut = i + saut;
    if espace(apres(src, debut)) {
        return None;
    }
    let pos = src[debut..fin].find("==")?;
    if pos == 0 || espace(avant(src, debut + pos)) {
        return None;
    }
    let mut enfants = Vec::new();
    analyser(src, debut, debut + pos, &mut enfants);
    let longueur = saut + pos + 2;
    Some((
        Node::Mark {
            mark: Mark::Highlight(teinte),
            children: enfants,
            range: i..i + longueur,
        },
        longueur,
    ))
}

/// `#tag`, `#maths/analyse`: after a space or the line's start, not only digits.
fn etiquette(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    if !espace(avant(src, i)) && avant(src, i) != Some('(') {
        return None;
    }
    let r = &src[i + 1..fin];
    let longueur: usize = r
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
        .map(char::len_utf8)
        .sum();
    let nom = r[..longueur].trim_end_matches(['/', '-']);
    if nom.is_empty() || nom.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let l = nom.len() + 1;
    Some((
        Node::Mark {
            mark: Mark::Tag(nom.to_string()),
            children: vec![Node::Text {
                text: src[i..i + l].to_string(),
                range: i..i + l,
            }],
            range: i..i + l,
        },
        l,
    ))
}

/// `@2026-10-08`.
fn date(src: &str, i: usize, fin: usize) -> Option<(Node, usize)> {
    if mot(avant(src, i)) {
        return None;
    }
    let r = src.get(i + 1..(i + 11).min(fin))?;
    let jour = chrono::NaiveDate::parse_from_str(r, "%Y-%m-%d").ok()?;
    if mot(apres(src, i + 11)) {
        return None;
    }
    Some((
        Node::Mark {
            mark: Mark::Date(jour),
            children: vec![Node::Text {
                text: src[i..i + 11].to_string(),
                range: i..i + 11,
            }],
            range: i..i + 11,
        },
        11,
    ))
}

// --- Writing out ----------------------------------------------------------------------

/// Superscript characters, for what Slint cannot raise.
fn exposant(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        'a' => 'ᵃ',
        'b' => 'ᵇ',
        'c' => 'ᶜ',
        'd' => 'ᵈ',
        'e' => 'ᵉ',
        'f' => 'ᶠ',
        'g' => 'ᵍ',
        'h' => 'ʰ',
        'j' => 'ʲ',
        'k' => 'ᵏ',
        'l' => 'ˡ',
        'm' => 'ᵐ',
        'o' => 'ᵒ',
        'p' => 'ᵖ',
        'r' => 'ʳ',
        's' => 'ˢ',
        't' => 'ᵗ',
        'u' => 'ᵘ',
        'v' => 'ᵛ',
        'w' => 'ʷ',
        'x' => 'ˣ',
        'y' => 'ʸ',
        'z' => 'ᶻ',
        _ => return None,
    })
}

fn indice(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'h' => 'ₕ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'p' => 'ₚ',
        'r' => 'ᵣ',
        's' => 'ₛ',
        't' => 'ₜ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        'x' => 'ₓ',
        _ => return None,
    })
}

/// `x` raised or lowered with Unicode where every character has a raised or lowered
/// form, else `^x` / `_x` as typed in maths.
pub fn raise(text: &str, haut: bool) -> String {
    let conv: Option<String> = text
        .chars()
        .map(|c| if haut { exposant(c) } else { indice(c) })
        .collect();
    conv.unwrap_or_else(|| {
        let signe = if haut { '^' } else { '_' };
        if text.chars().count() > 1 {
            format!("{signe}({text})")
        } else {
            format!("{signe}{text}")
        }
    })
}

/// Text as Slint's markdown reads it literally: every ASCII punctuation escaped, and
/// Slint's interpolation placeholder taken out.
fn echapper_md(s: &str) -> String {
    let mut sortie = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if c == '\u{e541}' {
            continue;
        }
        if c.is_ascii_punctuation() {
            sortie.push('\\');
        }
        sortie.push(c);
    }
    sortie
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

/// A link target safe in a markdown link: spaces and parentheses encoded.
pub fn encode_target(s: &str) -> String {
    let mut sortie = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' | b'#' | b':' => {
                sortie.push(b as char)
            }
            _ => sortie.push_str(&format!("%{b:02X}")),
        }
    }
    sortie
}

/// [`encode_target`] undone.
pub fn decode_target(s: &str) -> String {
    let octets = s.as_bytes();
    let mut sortie = Vec::with_capacity(octets.len());
    let mut k = 0;
    while k < octets.len() {
        if octets[k] == b'%'
            && k + 2 < octets.len()
            && octets[k + 1].is_ascii_hexdigit()
            && octets[k + 2].is_ascii_hexdigit()
        {
            let chiffre = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
            sortie.push(chiffre(octets[k + 1]) * 16 + chiffre(octets[k + 2]));
            k += 3;
            continue;
        }
        sortie.push(octets[k]);
        k += 1;
    }
    String::from_utf8_lossy(&sortie).into_owned()
}

/// How inline maths reads in a line that cannot draw it: its LaTeX made Unicode.
pub type MathText<'a> = &'a dyn Fn(&str) -> String;

/// The line for Slint's `StyledText`: what it can draw (bold, italic, strikethrough,
/// code, links, underline, colours) as it draws it; highlight as coloured text, since
/// it has no background; raised and lowered text in Unicode; maths through `math`.
/// Links point to `iris-note:`, `iris-tag:`, `iris-date:` or the web address.
pub fn to_slint_markdown(nodes: &[Node], palette: &Palette, math: MathText<'_>) -> String {
    let mut sortie = String::new();
    ecrire_md(nodes, palette, math, &mut sortie);
    sortie
}

fn ecrire_md(nodes: &[Node], palette: &Palette, math: MathText<'_>, sortie: &mut String) {
    let mut precedent_marque = false;
    for n in nodes {
        match n {
            Node::Text { text, .. } => {
                sortie.push_str(&echapper_md(text));
                precedent_marque = false;
            }
            Node::Mark { mark, children, .. } => {
                // Two marks side by side: a zero-width space keeps `**a****b**` from
                // reading as one run of stars.
                if precedent_marque {
                    sortie.push('\u{200b}');
                }
                precedent_marque = true;
                let interieur = |sortie: &mut String| ecrire_md(children, palette, math, sortie);
                match mark {
                    Mark::Bold => {
                        sortie.push_str("**");
                        interieur(sortie);
                        sortie.push_str("**");
                    }
                    Mark::Italic => {
                        sortie.push('*');
                        interieur(sortie);
                        sortie.push('*');
                    }
                    Mark::Strike => {
                        sortie.push_str("~~");
                        interieur(sortie);
                        sortie.push_str("~~");
                    }
                    Mark::Underline => {
                        sortie.push_str("<u>");
                        interieur(sortie);
                        sortie.push_str("</u>");
                    }
                    Mark::Colour(c) => {
                        sortie.push_str(&format!(
                            "<font color=\"{}\">",
                            hex(palette.colours[c.index()])
                        ));
                        interieur(sortie);
                        sortie.push_str("</font>");
                    }
                    Mark::Highlight(c) => {
                        let teinte =
                            c.map_or(palette.highlight_default, |c| palette.highlight[c.index()]);
                        sortie.push_str(&format!("<font color=\"{}\">**", hex(teinte)));
                        interieur(sortie);
                        sortie.push_str("**</font>");
                    }
                    Mark::Sup | Mark::Sub => {
                        let brut = plain_text(children);
                        sortie.push_str(&echapper_md(&raise(&brut, *mark == Mark::Sup)));
                    }
                    Mark::Code => {
                        let brut = plain_text(children).replace('\u{e541}', "");
                        let barrieres = if brut.contains('`') { "`` " } else { "`" };
                        let fermeture = if brut.contains('`') { " ``" } else { "`" };
                        sortie.push_str(barrieres);
                        sortie.push_str(&brut);
                        sortie.push_str(fermeture);
                    }
                    Mark::Math => {
                        let brut = plain_text(children);
                        sortie.push_str(&format!("<font color=\"{}\">*", hex(palette.math)));
                        sortie.push_str(&echapper_md(&math(&brut)));
                        sortie.push_str("*</font>");
                    }
                    Mark::NoteLink { target, .. } => {
                        sortie.push('[');
                        interieur(sortie);
                        sortie.push_str(&format!("](iris-note:{})", encode_target(target)));
                    }
                    Mark::WebLink { url } => {
                        sortie.push('[');
                        interieur(sortie);
                        sortie.push_str(&format!("]({})", encode_url(url)));
                    }
                    Mark::Tag(nom) => {
                        sortie.push('[');
                        interieur(sortie);
                        sortie.push_str(&format!("](iris-tag:{})", encode_target(nom)));
                    }
                    Mark::Date(jour) => {
                        sortie.push('[');
                        interieur(sortie);
                        sortie.push_str(&format!("](iris-date:{})", jour.format("%Y-%m-%d")));
                    }
                    Mark::Footnote => {
                        // A small mark in the line; its text shows when pointed at.
                        sortie.push_str(&format!(
                            "[\\*](iris-footnote:{})",
                            encode_target(&plain_text(children))
                        ));
                    }
                }
            }
        }
    }
}

/// A web address as a markdown link target: what would end it encoded.
fn encode_url(url: &str) -> String {
    url.replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E")
}

/// What HTML for a note needs from outside: how maths is drawn, where a note link
/// goes.
pub trait HtmlHooks {
    /// The HTML of inline maths, `src` being its LaTeX.
    fn math(&self, src: &str) -> String {
        format!("<code class=\"math\">{}</code>", escape_html(src))
    }
    /// Where a link to the note `target` points.
    fn note_href(&self, target: &str) -> String {
        format!("iris-note:{}", encode_target(target))
    }
}

/// The default hooks: maths as code, links to `iris-note:`.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainHooks;
impl HtmlHooks for PlainHooks {}

pub fn escape_html(s: &str) -> String {
    let mut sortie = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => sortie.push_str("&amp;"),
            '<' => sortie.push_str("&lt;"),
            '>' => sortie.push_str("&gt;"),
            '"' => sortie.push_str("&quot;"),
            c => sortie.push(c),
        }
    }
    sortie
}

/// The line as HTML, highlight as a real background.
pub fn to_html(nodes: &[Node], palette: &Palette, hooks: &dyn HtmlHooks) -> String {
    let mut sortie = String::new();
    ecrire_html(nodes, palette, hooks, &mut sortie);
    sortie
}

fn ecrire_html(nodes: &[Node], palette: &Palette, hooks: &dyn HtmlHooks, sortie: &mut String) {
    for n in nodes {
        match n {
            Node::Text { text, .. } => sortie.push_str(&escape_html(text)),
            Node::Mark { mark, children, .. } => {
                let (ouvre, ferme): (String, &str) = match mark {
                    Mark::Bold => ("<strong>".into(), "</strong>"),
                    Mark::Italic => ("<em>".into(), "</em>"),
                    Mark::Underline => ("<u>".into(), "</u>"),
                    Mark::Strike => ("<s>".into(), "</s>"),
                    Mark::Sup => ("<sup>".into(), "</sup>"),
                    Mark::Sub => ("<sub>".into(), "</sub>"),
                    Mark::Highlight(c) => (
                        format!(
                            "<mark style=\"background:{}33;color:inherit\">",
                            hex(c.map_or(palette.highlight_default, |c| palette.highlight
                                [c.index()]))
                        ),
                        "</mark>",
                    ),
                    Mark::Colour(c) => (
                        format!("<span style=\"color:{}\">", hex(palette.colours[c.index()])),
                        "</span>",
                    ),
                    Mark::Code => {
                        sortie.push_str(&format!(
                            "<code>{}</code>",
                            escape_html(&plain_text(children))
                        ));
                        continue;
                    }
                    Mark::Math => {
                        sortie.push_str(&hooks.math(&plain_text(children)));
                        continue;
                    }
                    Mark::NoteLink { target, .. } => (
                        format!("<a href=\"{}\">", escape_html(&hooks.note_href(target))),
                        "</a>",
                    ),
                    Mark::WebLink { url } => (format!("<a href=\"{}\">", escape_html(url)), "</a>"),
                    Mark::Tag(_) => (
                        format!("<span class=\"tag\" style=\"color:{}\">", hex(palette.tag)),
                        "</span>",
                    ),
                    Mark::Date(_) => ("<span class=\"date\">".into(), "</span>"),
                    Mark::Footnote => ("<sup class=\"footnote\" title=\"".into(), "\">*</sup>"),
                };
                if *mark == Mark::Footnote {
                    sortie.push_str(&ouvre);
                    sortie.push_str(&escape_html(&plain_text(children)));
                    sortie.push_str(ferme);
                    continue;
                }
                sortie.push_str(&ouvre);
                ecrire_html(children, palette, hooks, sortie);
                sortie.push_str(ferme);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marques(src: &str) -> Vec<(String, Vec<Mark>)> {
        spans(&parse_inline(src))
            .into_iter()
            .map(|s| (s.text, s.marks))
            .collect()
    }

    fn seul(src: &str) -> Vec<Mark> {
        let s = marques(src);
        assert_eq!(s.len(), 1, "{src}: {s:?}");
        s[0].1.clone()
    }

    #[test]
    fn each_mark_is_read() {
        assert_eq!(seul("**x**"), vec![Mark::Bold]);
        assert_eq!(seul("*x*"), vec![Mark::Italic]);
        assert_eq!(seul("__x__"), vec![Mark::Underline]);
        assert_eq!(seul("--x--"), vec![Mark::Strike]);
        assert_eq!(seul("~~x~~"), vec![Mark::Strike]);
        assert_eq!(seul("==x=="), vec![Mark::Highlight(None)]);
        assert_eq!(seul("=={g}x=="), vec![Mark::Highlight(Some(Colour::Green))]);
        assert_eq!(seul("{r}x{/}"), vec![Mark::Colour(Colour::Red)]);
        assert_eq!(seul("{rouge}x{/}"), vec![Mark::Colour(Colour::Red)]);
        assert_eq!(seul("`x`"), vec![Mark::Code]);
        assert_eq!(seul("$x^2$"), vec![Mark::Math]);
        assert_eq!(
            seul("#maths/analyse"),
            vec![Mark::Tag("maths/analyse".into())]
        );
        assert_eq!(
            seul("@2026-10-08"),
            vec![Mark::Date(
                chrono::NaiveDate::from_ymd_opt(2026, 10, 8).unwrap()
            )]
        );
        assert_eq!(seul("^[note]"), vec![Mark::Footnote]);
    }

    #[test]
    fn raised_and_lowered() {
        let s = marques("x^2^ et H~2~O");
        assert_eq!(s[1], ("2".into(), vec![Mark::Sup]));
        assert_eq!(s[3], ("2".into(), vec![Mark::Sub]));
        assert_eq!(raise("2n", true), "²ⁿ");
        assert_eq!(raise("ij", false), "ᵢⱼ");
        assert_eq!(raise("Ω", true), "^Ω");
    }

    #[test]
    fn links_and_their_labels() {
        let n =
            parse_inline("voir [[Limites|ici]] et [[Suites#Cauchy]], [web](https://example.com/a)");
        let s = spans(&n);
        assert_eq!(s[1].text, "ici");
        assert_eq!(
            s[1].marks,
            vec![Mark::NoteLink {
                target: "Limites".into(),
                embed: false
            }]
        );
        assert_eq!(s[3].text, "Suites › Cauchy");
        assert_eq!(
            s[5].marks,
            vec![Mark::WebLink {
                url: "https://example.com/a".into()
            }]
        );
        let bare = marques("lien https://example.com/x. fin");
        assert_eq!(bare[1].0, "https://example.com/x");
    }

    #[test]
    fn marks_nest() {
        assert_eq!(seul("**__x__**"), vec![Mark::Bold, Mark::Underline]);
        let s = marques("{b}**a** b{/}");
        assert_eq!(
            s[0],
            ("a".into(), vec![Mark::Colour(Colour::Blue), Mark::Bold])
        );
        assert_eq!(s[1], (" b".into(), vec![Mark::Colour(Colour::Blue)]));
    }

    #[test]
    fn what_is_not_a_mark_stays_text() {
        for texte in [
            "a -- b",
            "2 * 3",
            "2*3*4",
            "snake__case__name",
            "a--b--c",
            "x ** y",
            "--- rule-like",
            "$5 and $6",
            "$ x$",
            "## not here",
            "a#b",
            "#123",
            "mail@2026-10-08",
            "{z}pas une couleur{/}",
            "[[ ]]",
            "[texte](avec espace)",
        ] {
            let s = marques(texte);
            assert!(s.iter().all(|(_, m)| m.is_empty()), "{texte} read as {s:?}");
            assert_eq!(plain_text(&parse_inline(texte)), texte);
        }
    }

    #[test]
    fn maths_and_code_keep_their_insides() {
        assert_eq!(marques("$a*b*c$")[0].0, "a*b*c");
        // Spaces inside, with a command: maths. Money, never.
        assert_eq!(
            seul(r"$ \forall x \in \mathbb{R}, \exists \lambda \in [3,4]$"),
            vec![Mark::Math]
        );
        assert!(seul("it costs $ 5 and $ 6").is_empty());
        assert!(seul("$5 and $6").is_empty());
        assert!(seul("$ $").is_empty());
        assert_eq!(marques("`**x**`")[0].0, "**x**");
        assert_eq!(marques("``a ` b``")[0].0, "a ` b");
        // The `**` inside code does not close the bold around it.
        let s = marques("**a `**` b**");
        assert!(
            s.iter().all(|(_, m)| m.first() == Some(&Mark::Bold)),
            "{s:?}"
        );
        assert_eq!(s[1], ("**".into(), vec![Mark::Bold, Mark::Code]));
    }

    #[test]
    fn escapes_are_literal() {
        let s = marques(r"\*pas\* \__non\__");
        assert!(s.iter().all(|(_, m)| m.is_empty()));
        assert_eq!(plain_text(&parse_inline(r"\*x\*")), "*x*");
    }

    #[test]
    fn ranges_point_into_the_source() {
        let src = "un {r}mot{/} ici";
        let n = parse_inline(src);
        let s = spans(&n);
        assert_eq!(&src[s[1].range.clone()], "mot");
        let m = marks_at(&n, 7);
        assert_eq!(m[0].0, Mark::Colour(Colour::Red));
        assert_eq!(&src[m[0].1.clone()], "{r}mot{/}");
    }

    #[test]
    fn slint_markdown_is_literal_where_text_is() {
        let p = Palette::default();
        let un = |s: &str| s.to_string();
        let md = to_slint_markdown(&parse_inline("1. <b>**gras**</b> #tag"), &p, &un);
        assert!(md.starts_with("1\\. \\<b\\>**gras**\\<\\/b\\>"), "{md}");
        assert!(md.contains("[\\#tag](iris-tag:tag)"), "{md}");
        let md = to_slint_markdown(&parse_inline("__s__ {r}x{/}"), &p, &un);
        assert!(md.contains("<u>s</u>"));
        assert!(md.contains("<font color=\"#e03e3e\">x</font>"), "{md}");
        let md = to_slint_markdown(&parse_inline("[[Une note]]"), &p, &un);
        assert!(md.contains("(iris-note:Une%20note)"), "{md}");
        assert_eq!(decode_target("Une%20note"), "Une note");
    }

    #[test]
    fn html_escapes_and_highlights() {
        let p = Palette::default();
        let html = to_html(&parse_inline("<i> ==x== __u__"), &p, &PlainHooks);
        assert!(html.starts_with("&lt;i&gt; <mark"), "{html}");
        assert!(html.contains("<u>u</u>"));
    }
}
