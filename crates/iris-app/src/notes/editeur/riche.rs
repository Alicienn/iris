//! A line of a note made styled text for the editor: what is shown, in which style,
//! where each part comes from in the source, and the formulas drawn inside it.
//!
//! The line being written shows its source, every mark in it dimmed and the words
//! between in their style (`**gras**` bold, its stars grey), so what is typed is seen to
//! be read. The other lines show their result: marks gone, formulas drawn, links in
//! the accent colour. Either way a map ties each shown byte to the source, for the
//! cursor and the mouse.

use iris_notes::inline::{self, Mark, Node, Palette};
use std::ops::Range;

/// How a run of text is drawn. Parley carries it as each style's brush.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pinceau {
    /// `0xAARRGGBB`.
    pub couleur: u32,
    pub gras: bool,
    pub italique: bool,
    /// 0: the text's font, 1: fixed-width, 2: the display font.
    pub famille: u8,
    /// A background (a highlight, inline code), `0` for none.
    pub fond: u32,
    pub souligne: bool,
    pub barre: bool,
    /// Smaller than the text around it (inline code, raised text), in hundredths.
    pub echelle: u8,
}

impl Pinceau {
    pub fn taille(&self, base: f32) -> f32 {
        if self.echelle == 0 {
            base
        } else {
            base * f32::from(self.echelle) / 100.0
        }
    }
}

/// The colours the editor draws with, as `0xAARRGGBB`.
#[derive(Clone, Debug, Default)]
pub struct Couleurs {
    pub texte: u32,
    pub muet: u32,
    pub secondaire: u32,
    pub accent: u32,
    pub maths: u32,
    pub plaque: u32,
    pub bordure: u32,
    pub bordure_forte: u32,
    pub selection: u32,
    pub palette: Palette,
}

impl Couleurs {
    fn rgb(c: u32) -> u32 {
        0xff00_0000 | (c & 0x00ff_ffff)
    }

    pub fn teinte(&self, c: inline::Colour) -> u32 {
        Self::rgb(self.palette.colours[c.index()])
    }

    /// A highlight's background: its colour, light enough to read through.
    pub fn surligne(&self, c: Option<inline::Colour>) -> u32 {
        let base = c.map_or(self.palette.highlight_default, |c| {
            self.palette.highlight[c.index()]
        });
        0x4000_0000 | (base & 0x00ff_ffff)
    }
}

/// Something drawn inside a line: a formula's picture, or a link's icon at the end of
/// its bubble. Where it goes in the shown text, its size in logical pixels, and its
/// baseline from its top.
pub struct Formule {
    pub index: usize,
    pub image: Option<slint::Image>,
    /// An icon of `Icons` instead of a picture, on a bubble of this colour.
    pub icone: Option<(&'static str, u32)>,
    pub largeur: f32,
    pub hauteur: f32,
    pub ligne_de_base: f32,
}

/// A part of what is shown and the part of the source it comes from. Equal lengths:
/// byte for byte; otherwise (a link's label, a formula) only its ends match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Troncon {
    pub affiche: Range<usize>,
    pub source: Range<usize>,
}

/// A line made styled text.
#[derive(Default)]
pub struct Riche {
    pub texte: String,
    pub styles: Vec<(Range<usize>, Pinceau)>,
    pub formules: Vec<Formule>,
    pub carte: Vec<Troncon>,
    /// The links in what is shown: their range and where they go (`iris-note:`, a web
    /// address, `iris-tag:`…).
    pub liens: Vec<(Range<usize>, String)>,
}

impl Riche {
    fn pousser(&mut self, texte: &str, source: Range<usize>, p: Pinceau) {
        if texte.is_empty() {
            return;
        }
        let debut = self.texte.len();
        self.texte.push_str(texte);
        let fin = self.texte.len();
        self.styles.push((debut..fin, p));
        self.carte.push(Troncon {
            affiche: debut..fin,
            source,
        });
    }

    /// Several lines joined: `autre` after a line break, its source moved by `decalage`.
    pub fn ajouter_ligne(&mut self, autre: Riche, decalage: usize, p: Pinceau) {
        if !self.texte.is_empty() || !self.carte.is_empty() {
            let debut = self.texte.len();
            self.texte.push('\n');
            self.styles.push((debut..debut + 1, p));
        }
        let base = self.texte.len();
        self.texte.push_str(&autre.texte);
        self.styles.extend(
            autre
                .styles
                .into_iter()
                .map(|(r, p)| (r.start + base..r.end + base, p)),
        );
        self.formules
            .extend(autre.formules.into_iter().map(|mut f| {
                f.index += base;
                f
            }));
        self.carte.extend(autre.carte.into_iter().map(|t| Troncon {
            affiche: t.affiche.start + base..t.affiche.end + base,
            source: t.source.start + decalage..t.source.end + decalage,
        }));
        // An empty line still has a place, for the cursor.
        if autre.texte.is_empty() {
            self.carte.push(Troncon {
                affiche: base..base,
                source: decalage..decalage,
            });
        }
        self.liens.extend(
            autre
                .liens
                .into_iter()
                .map(|(r, l)| (r.start + base..r.end + base, l)),
        );
    }

    /// The source offset a shown offset comes from.
    pub fn vers_source(&self, affiche: usize) -> usize {
        let mut meilleur: Option<&Troncon> = None;
        for t in &self.carte {
            if t.affiche.start <= affiche && affiche <= t.affiche.end {
                // Inside a part: byte for byte, or one of its ends.
                if t.affiche.len() == t.source.len() {
                    return t.source.start + (affiche - t.affiche.start);
                }
                return if affiche == t.affiche.end && affiche != t.affiche.start {
                    t.source.end
                } else {
                    t.source.start
                };
            }
            if t.affiche.end <= affiche {
                meilleur = Some(t);
            }
        }
        meilleur.map_or(0, |t| t.source.end)
    }

    /// Where a source offset is shown; a mark's hidden character goes to the nearest
    /// shown one.
    pub fn vers_affiche(&self, source: usize) -> usize {
        let mut apres: Option<&Troncon> = None;
        let mut avant: Option<&Troncon> = None;
        for t in &self.carte {
            if t.source.start <= source && source <= t.source.end {
                if t.affiche.len() == t.source.len() {
                    return t.affiche.start + (source - t.source.start);
                }
                return if source == t.source.end && source != t.source.start {
                    t.affiche.end
                } else {
                    t.affiche.start
                };
            }
            if t.source.end <= source {
                avant = Some(t);
            } else if apres.is_none() {
                apres = Some(t);
            }
        }
        match (avant, apres) {
            (_, Some(t)) => t.affiche.start,
            (Some(t), None) => t.affiche.end,
            (None, None) => 0,
        }
    }

    /// The link at a shown offset.
    pub fn lien(&self, affiche: usize) -> Option<&str> {
        self.liens
            .iter()
            .find(|(r, _)| r.start <= affiche && affiche < r.end)
            .map(|(_, l)| l.as_str())
    }
}

/// A mark's style over the style around it.
fn appliquer(mark: &Mark, p: Pinceau, c: &Couleurs) -> Pinceau {
    let mut q = p;
    match mark {
        Mark::Bold => q.gras = true,
        Mark::Italic => q.italique = true,
        Mark::Underline => q.souligne = true,
        Mark::Strike => q.barre = true,
        Mark::Highlight(teinte) => q.fond = c.surligne(*teinte),
        Mark::Colour(teinte) => q.couleur = c.teinte(*teinte),
        Mark::Code => {
            q.famille = 1;
            q.fond = (c.plaque & 0x00ff_ffff) | 0xff00_0000;
            q.echelle = 90;
        }
        Mark::Math => q.couleur = c.maths,
        Mark::NoteLink { .. } | Mark::WebLink { .. } | Mark::Tag(_) | Mark::Date(_) => {
            q.couleur = c.accent;
        }
        Mark::Footnote => q.couleur = c.accent,
        Mark::Sup | Mark::Sub => q.echelle = 80,
    }
    q
}

/// The line being written: its source as it is, each mark dimmed, the words between
/// in their style. `src` is the line; shown offsets are its offsets.
pub fn source(src: &str, base: Pinceau, c: &Couleurs) -> Riche {
    let noeuds = inline::parse_inline(src);
    let mut styles: Vec<(Range<usize>, Pinceau)> = Vec::new();
    fn parcourir(
        noeuds: &[Node],
        p: Pinceau,
        c: &Couleurs,
        styles: &mut Vec<(Range<usize>, Pinceau)>,
    ) {
        for n in noeuds {
            match n {
                Node::Text { range, .. } => styles.push((range.clone(), p)),
                Node::Mark {
                    mark,
                    children,
                    range,
                } => {
                    let q = appliquer(mark, p, c);
                    // The mark's own characters (`**`, `[[`, `{r}`), dimmed.
                    let mut muet = q;
                    muet.couleur = c.muet;
                    muet.fond = 0;
                    muet.souligne = false;
                    let mut couverts: Vec<Range<usize>> = Vec::new();
                    fn couvrir(n: &[Node], out: &mut Vec<Range<usize>>) {
                        for x in n {
                            match x {
                                Node::Text { range, .. } => out.push(range.clone()),
                                Node::Mark { range, .. } => out.push(range.clone()),
                            }
                        }
                    }
                    couvrir(children, &mut couverts);
                    couverts.sort_by_key(|r| r.start);
                    let mut k = range.start;
                    for r in &couverts {
                        if r.start > k {
                            styles.push((k..r.start, muet));
                        }
                        k = k.max(r.end);
                    }
                    if k < range.end {
                        styles.push((k..range.end, muet));
                    }
                    // A link's label is shown as written: its target is what is between
                    // the brackets.
                    match mark {
                        Mark::NoteLink { .. } | Mark::Tag(_) | Mark::Date(_) => {
                            for r in &couverts {
                                styles.push((r.clone(), q));
                            }
                        }
                        _ => parcourir(children, q, c, styles),
                    }
                }
            }
        }
    }
    parcourir(&noeuds, base, c, &mut styles);
    styles.sort_by_key(|(r, _)| r.start);
    // What no node covers (a `\` that escapes) is dimmed too.
    let mut complets: Vec<(Range<usize>, Pinceau)> = Vec::with_capacity(styles.len() + 4);
    let mut muet = base;
    muet.couleur = c.muet;
    let mut k = 0;
    for (r, p) in styles {
        if r.start < k {
            continue;
        }
        if r.start > k {
            complets.push((k..r.start, muet));
        }
        k = r.end;
        complets.push((r, p));
    }
    if k < src.len() {
        complets.push((k..src.len(), muet));
    }
    Riche {
        texte: src.to_string(),
        styles: complets,
        formules: Vec::new(),
        carte: vec![Troncon {
            affiche: 0..src.len(),
            source: 0..src.len(),
        }],
        liens: Vec::new(),
    }
}

/// How a formula in a line is drawn: its LaTeX, the text's size; `None` when it cannot
/// be (it is then written in Unicode).
pub type Dessinateur<'a> = &'a dyn Fn(&str, f32) -> Option<iris_math::Picture>;

/// A line as it reads: marks gone, formulas drawn, links in the accent colour.
pub fn rendu(
    src: &str,
    base: Pinceau,
    taille: f32,
    echelle: f32,
    c: &Couleurs,
    formule: Dessinateur<'_>,
) -> Riche {
    let noeuds = inline::parse_inline(src);
    let mut r = Riche::default();
    let rien: &[Range<usize>] = &[];
    ecrire(
        &noeuds, base, taille, echelle, c, formule, rien, src, &mut r,
    );
    r
}

/// The line being written: each mark as it reads, except the one the cursor touches
/// (`curseur`, at its edge or inside), which shows its source — its signs dimmed, its
/// words in their style — so it can be changed; moving away converts it again.
pub fn ecriture(
    src: &str,
    base: Pinceau,
    curseur: usize,
    taille: f32,
    echelle: f32,
    c: &Couleurs,
    formule: Dessinateur<'_>,
) -> Riche {
    let noeuds = inline::parse_inline(src);
    // The marks the cursor touches, outermost first.
    let ouvertes: Vec<Range<usize>> = inline::marks_at(&noeuds, curseur)
        .into_iter()
        .map(|(_, r)| r)
        .collect();
    let mut r = Riche::default();
    ecrire(
        &noeuds, base, taille, echelle, c, formule, &ouvertes, src, &mut r,
    );
    r
}

/// The source of `plage` shown as written, styled as the line written is.
fn pousser_source(r: &mut Riche, src: &str, plage: Range<usize>, base: Pinceau, c: &Couleurs) {
    let s = source(src, base, c);
    for (rg, p) in s.styles {
        let a = rg.start.max(plage.start);
        let b = rg.end.min(plage.end);
        if a < b {
            r.pousser(&src[a..b], a..b, p);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn ecrire(
    noeuds: &[Node],
    p: Pinceau,
    taille: f32,
    echelle: f32,
    c: &Couleurs,
    formule: Dessinateur<'_>,
    ouvertes: &[Range<usize>],
    src: &str,
    r: &mut Riche,
) {
    for n in noeuds {
        match n {
            Node::Text { text, range } => r.pousser(text, range.clone(), p),
            Node::Mark { range, .. } if ouvertes.first() == Some(range) => {
                // The mark the cursor is at: written out.
                pousser_source(r, src, range.clone(), p, c);
            }
            Node::Mark {
                mark,
                children,
                range,
            } => {
                let q = appliquer(mark, p, c);
                let debut = r.texte.len();
                match mark {
                    Mark::Math => {
                        let latex = inline::plain_text(children);
                        match formule(&latex, taille).filter(|_| !latex.trim().is_empty()) {
                            Some(dessin) => {
                                let index = r.texte.len();
                                let e = echelle.max(1.0);
                                r.formules.push(Formule {
                                    index,
                                    largeur: dessin.width as f32 / e,
                                    hauteur: dessin.height as f32 / e,
                                    ligne_de_base: dessin.baseline as f32 / e,
                                    icone: None,
                                    image: Some(
                                        slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                                            slint::Rgba8Pixel,
                                        >::clone_from_slice(
                                            &dessin.rgba,
                                            dessin.width,
                                            dessin.height,
                                        )),
                                    ),
                                });
                                r.carte.push(Troncon {
                                    affiche: index..index,
                                    source: range.clone(),
                                });
                            }
                            None => {
                                let texte = iris_notes::math::to_unicode(&latex);
                                r.pousser(&texte, range.clone(), q);
                            }
                        }
                    }
                    Mark::Code => {
                        let texte = inline::plain_text(children);
                        let source = children
                            .first()
                            .map(|n| match n {
                                Node::Text { range, .. } | Node::Mark { range, .. } => {
                                    range.clone()
                                }
                            })
                            .unwrap_or_else(|| range.clone());
                        r.pousser(&texte, source, q);
                    }
                    Mark::Sup | Mark::Sub => {
                        let brut = inline::plain_text(children);
                        let texte = inline::raise(&brut, *mark == Mark::Sup);
                        r.pousser(&texte, range.clone(), p);
                    }
                    Mark::Footnote => {
                        r.pousser("*", range.clone(), q);
                        r.liens.push((
                            debut..r.texte.len(),
                            format!(
                                "iris-footnote:{}",
                                inline::encode_target(&inline::plain_text(children))
                            ),
                        ));
                        continue;
                    }
                    // A link to a note: a bubble, its name and an arrow out, a click
                    // away from the note.
                    Mark::NoteLink { embed: false, .. } => {
                        let mut bulle = q;
                        bulle.fond = (c.accent & 0x00ff_ffff) | 0x2600_0000;
                        let interieures = ouvertes.get(1..).unwrap_or(&[]);
                        ecrire(
                            children,
                            bulle,
                            taille,
                            echelle,
                            c,
                            formule,
                            interieures,
                            src,
                            r,
                        );
                        r.formules.push(Formule {
                            index: r.texte.len(),
                            image: None,
                            icone: Some(("open-external", bulle.fond)),
                            largeur: taille * 0.95,
                            hauteur: taille * 0.8,
                            ligne_de_base: taille * 0.8,
                        });
                    }
                    _ => {
                        let interieures = ouvertes.get(1..).unwrap_or(&[]);
                        ecrire(
                            children,
                            q,
                            taille,
                            echelle,
                            c,
                            formule,
                            interieures,
                            src,
                            r,
                        )
                    }
                }
                let fin = r.texte.len();
                let cible = match mark {
                    Mark::NoteLink { target, .. } => {
                        Some(format!("iris-note:{}", inline::encode_target(target)))
                    }
                    Mark::WebLink { url } => Some(url.clone()),
                    Mark::Tag(nom) => Some(format!("iris-tag:{}", inline::encode_target(nom))),
                    Mark::Date(jour) => Some(format!("iris-date:{}", jour.format("%Y-%m-%d"))),
                    _ => None,
                };
                if let Some(cible) = cible {
                    if fin > debut {
                        r.liens.push((debut..fin, cible));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn couleurs() -> Couleurs {
        Couleurs {
            texte: 0xff00_0000,
            muet: 0xff88_8888,
            accent: 0xff00_00ff,
            maths: 0xff55_00aa,
            ..Default::default()
        }
    }

    #[test]
    fn the_line_written_dims_its_marks() {
        let c = couleurs();
        let r = source("a **b** c", Pinceau::default(), &c);
        assert_eq!(r.texte, "a **b** c");
        let style_de = |k: usize| {
            r.styles
                .iter()
                .find(|(rg, _)| rg.start <= k && k < rg.end)
                .map(|(_, p)| *p)
                .unwrap()
        };
        assert_eq!(style_de(2).couleur, c.muet, "the stars are dimmed");
        assert!(style_de(4).gras, "the word is bold");
        assert!(!style_de(0).gras);
        // Every byte has a style, once.
        let total: usize = r.styles.iter().map(|(rg, _)| rg.len()).sum();
        assert_eq!(total, r.texte.len());
    }

    #[test]
    fn a_line_read_maps_back_to_its_source() {
        let c = couleurs();
        let rien = |_: &str, _: f32| None;
        let r = rendu(
            "a **gras** [[Note|ici]] b",
            Pinceau::default(),
            15.0,
            1.0,
            &c,
            &rien,
        );
        assert_eq!(r.texte, "a gras ici b");
        // `r` of "gras" is byte 5 of the source.
        assert_eq!(r.vers_source(3), 5);
        assert_eq!(r.vers_affiche(4), 2);
        // A hidden star goes to the nearest shown character.
        assert_eq!(r.vers_affiche(2), 2);
        assert_eq!(r.lien(8).map(|l| l.starts_with("iris-note:")), Some(true));
        assert_eq!(r.vers_source(r.texte.len()), 25);
    }

    #[test]
    fn only_the_mark_the_cursor_touches_shows_its_signs() {
        let c = couleurs();
        let rien = |_: &str, _: f32| None;
        let src = "a **b** c **d**";
        let r = ecriture(src, Pinceau::default(), 4, 15.0, 1.0, &c, &rien);
        assert_eq!(r.texte, "a **b** c d", "in the first bold: its stars show");
        let r = ecriture(src, Pinceau::default(), 9, 15.0, 1.0, &c, &rien);
        assert_eq!(r.texte, "a b c d", "between them: both read as bold");
        // The cursor still maps to the source.
        assert_eq!(r.vers_source(r.vers_affiche(8)), 8);
    }

    #[test]
    fn a_formula_is_drawn_inside_the_line() {
        let c = couleurs();
        let dessin = |_: &str, _: f32| {
            Some(iris_math::Picture {
                width: 20,
                height: 10,
                rgba: vec![0; 800],
                baseline: 8,
            })
        };
        let r = rendu("soit $x$ ici", Pinceau::default(), 15.0, 1.0, &c, &dessin);
        assert_eq!(r.texte, "soit  ici");
        assert_eq!(r.formules.len(), 1);
        assert_eq!(r.formules[0].index, 5);
        // A click on it goes into the formula's source.
        assert_eq!(r.vers_source(5), 5);
    }
}
