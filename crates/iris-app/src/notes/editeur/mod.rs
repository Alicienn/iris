//! The note's editor: every block laid out here by parley — the library Slint itself
//! lays its text out with — and drawn by Slint as words placed where they were measured,
//! rectangles, pictures and icons. Rust therefore knows where each character is: the
//! cursor, the selection across blocks, a click, Up and Down all come from these
//! layouts, and formulas are drawn inside the lines as pictures.
//!
//! A block is laid out once per change of its text, of whether it is the block being
//! written, or of the column's width; what does not change is kept (`Vue`).

pub mod dessin;
pub mod riche;

use iris_ui::{NoteIconData, NoteImageData, NoteRectData, NoteRunData};
use parley::{
    Affinity, Alignment, AlignmentOptions, Cursor, FontContext, InlineBox, InlineBoxKind,
    LayoutContext, PositionedLayoutItem, Selection, StyleProperty,
};
pub use riche::{Couleurs, Pinceau, Riche};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

/// A laid-out text, in physical pixels (the screen's scale).
pub type Mise = parley::Layout<Pinceau>;

thread_local! {
    /// The fonts, found as Slint finds them (the system's, shared).
    static POLICES: RefCell<FontContext> = RefCell::new(FontContext {
        collection: parley::fontique::Collection::new(parley::fontique::CollectionOptions {
            shared: true,
            system_fonts: true,
        }),
        source_cache: parley::fontique::SourceCache::new_shared(),
    });
    static MISES: RefCell<LayoutContext<Pinceau>> = RefCell::new(LayoutContext::new());
    /// Each font's line height as a multiple of its size, as Slint computes it.
    static RAPPORTS: RefCell<HashMap<String, f32>> = RefCell::new(HashMap::new());
}

/// The fonts the editor writes with, by family name (the theme's).
#[derive(Clone, Debug, Default)]
pub struct Polices {
    pub corps: String,
    pub mono: String,
    pub titre: String,
}

impl Polices {
    fn nom(&self, famille: u8) -> &str {
        match famille {
            1 => &self.mono,
            2 => &self.titre,
            _ => &self.corps,
        }
    }
}

/// The families Slint asks for: the one named, then the generic ones it falls back to.
fn familles(nom: &str) -> parley::FontFamily<'_> {
    parley::FontFamily::List(Cow::Owned(vec![
        parley::FontFamilyName::named(nom),
        parley::FontFamilyName::Generic(parley::GenericFamily::SansSerif),
        parley::FontFamilyName::Generic(parley::GenericFamily::SystemUi),
    ]))
}

/// A font's natural line height over its size: (ascent + descent + leading) / size.
fn rapport(nom: &str) -> f32 {
    if let Some(r) = RAPPORTS.with_borrow(|m| m.get(nom).copied()) {
        return r;
    }
    let r = MISES
        .with_borrow_mut(|lc| {
            POLICES.with_borrow_mut(|fc| {
                let mut b = lc.ranged_builder(fc, "Hg", 1.0, false);
                b.push_default(familles(nom));
                b.push_default(StyleProperty::FontSize(100.0));
                b.push_default(StyleProperty::LineHeight(
                    parley::LineHeight::MetricsRelative(1.0),
                ));
                let mut l = b.build("Hg");
                l.break_all_lines(None);
                let hauteur = l.lines().next().map(|ligne| ligne.metrics().line_height);
                hauteur.map(|h| h / 100.0)
            })
        })
        .filter(|r| r.is_finite() && *r > 0.5)
        .unwrap_or(1.33);
    RAPPORTS.with_borrow_mut(|m| m.insert(nom.to_string(), r));
    r
}

/// What a block draws, in its own coordinates (logical pixels).
#[derive(Default)]
pub struct Dessin {
    pub runs: Vec<NoteRunData>,
    pub rects: Vec<NoteRectData>,
    pub images: Vec<NoteImageData>,
    pub icones: Vec<NoteIconData>,
}

pub fn couleur(argb: u32) -> slint::Color {
    slint::Color::from_argb_encoded(argb)
}

impl Dessin {
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: u32, rayon: f32) {
        self.rects.push(NoteRectData {
            x,
            y,
            w,
            h,
            color: couleur(c),
            radius: rayon,
            border: couleur(0),
            border_width: 0.0,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn cadre(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fond: u32,
        bord: u32,
        epais: f32,
        rayon: f32,
    ) {
        self.rects.push(NoteRectData {
            x,
            y,
            w,
            h,
            color: couleur(fond),
            radius: rayon,
            border: couleur(bord),
            border_width: epais,
        });
    }

    pub fn icone(&mut self, x: f32, y: f32, taille: f32, nom: &str, teinte: u32) {
        self.icones.push(NoteIconData {
            x,
            y,
            size: taille,
            name: nom.into(),
            tint: couleur(teinte),
        });
    }

    /// Words in one style, their top-left at `(x, y)`.
    pub fn mot(&mut self, x: f32, y: f32, texte: &str, taille: f32, p: Pinceau, polices: &Polices) {
        let h = taille * rapport(polices.nom(p.famille));
        self.runs.push(NoteRunData {
            x,
            y,
            h,
            text: texte.into(),
            size: taille,
            weight: if p.gras { 700 } else { 400 },
            italic: p.italique,
            family: i32::from(p.famille),
            color: couleur(p.couleur),
        });
    }
}

/// How the editor lays out: the screen's scale, the column's width (logical), the
/// colours and fonts.
pub struct Contexte<'a> {
    pub echelle: f32,
    pub largeur: f32,
    pub couleurs: &'a Couleurs,
    pub polices: &'a Polices,
    pub dir: Option<&'a std::path::Path>,
    /// A formula in a line, at a size.
    pub formule: riche::Dessinateur<'a>,
    /// A `$$` block's formula.
    pub formule_bloc: &'a dyn Fn(&str) -> Option<iris_math::Picture>,
}

/// A laid-out text of a block, where it is in the block, and how its shown text maps
/// to the block's source.
pub struct Zone {
    pub x: f32,
    pub y: f32,
    pub mise: Mise,
    pub riche: Riche,
    /// The lines moved down (logical) to make room for a formula hanging below one of
    /// them: for each line, how far it was moved.
    pub decalages: Vec<f32>,
    pub hauteur: f32,
    echelle: f32,
}

impl Zone {
    /// The line at `y` (zone coordinates, logical), and how far it was moved.
    fn ligne_a(&self, y: f32) -> (usize, f32) {
        let e = self.echelle;
        let mut derniere = (0, 0.0);
        for (i, ligne) in self.mise.lines().enumerate() {
            let s = self.decalages.get(i).copied().unwrap_or(0.0);
            let m = ligne.metrics();
            let bas = m.block_max_coord / e + s;
            derniere = (i, s);
            if y < bas {
                return derniere;
            }
        }
        derniere
    }

    /// How far the line at `y_physique` (the layout's own coordinates) was moved.
    fn decalage_de_ligne(&self, y_physique: f64) -> f32 {
        for (i, ligne) in self.mise.lines().enumerate() {
            let m = ligne.metrics();
            if y_physique < m.block_max_coord as f64 - 0.5 || i + 1 == self.decalages.len() {
                return self.decalages.get(i).copied().unwrap_or(0.0);
            }
        }
        self.decalages.last().copied().unwrap_or(0.0)
    }

    /// The shown offset under a point (zone coordinates, logical).
    pub fn point(&self, x: f32, y: f32) -> usize {
        let e = self.echelle;
        let (_, s) = self.ligne_a(y);
        Cursor::from_point(&self.mise, x * e, (y - s).max(0.0) * e).index()
    }

    /// The cursor at a shown offset: its left, top and height (zone coordinates).
    pub fn curseur(&self, affiche: usize) -> (f32, f32, f32) {
        let e = self.echelle;
        let c = Cursor::from_byte_index(
            &self.mise,
            affiche.min(self.riche.texte.len()),
            Affinity::Downstream,
        );
        let g = c.geometry(&self.mise, 1.0);
        let s = self.decalage_de_ligne((g.y0 + g.y1) / 2.0);
        let h = ((g.y1 - g.y0) as f32 / e).max(8.0);
        (g.x0 as f32 / e, g.y0 as f32 / e + s, h)
    }

    /// The rectangles covering shown offsets `a` to `b` (zone coordinates).
    pub fn rectangles(&self, a: usize, b: usize) -> Vec<(f32, f32, f32, f32)> {
        let e = self.echelle;
        let n = self.riche.texte.len();
        let sel = Selection::new(
            Cursor::from_byte_index(&self.mise, a.min(n), Affinity::Downstream),
            Cursor::from_byte_index(&self.mise, b.min(n), Affinity::Upstream),
        );
        sel.geometry(&self.mise)
            .into_iter()
            .map(|(g, ligne)| {
                let s = self.decalages.get(ligne).copied().unwrap_or(0.0);
                (
                    g.x0 as f32 / e,
                    g.y0 as f32 / e + s,
                    ((g.x1 - g.x0) as f32 / e).max(3.0),
                    (g.y1 - g.y0) as f32 / e,
                )
            })
            .collect()
    }

    /// The smallest and largest source offsets the zone shows.
    pub fn etendue(&self) -> (usize, usize) {
        let debut = self
            .riche
            .carte
            .iter()
            .map(|t| t.source.start)
            .min()
            .unwrap_or(0);
        let fin = self
            .riche
            .carte
            .iter()
            .map(|t| t.source.end)
            .max()
            .unwrap_or(0);
        (debut, fin)
    }
}

/// `riche` laid out `largeur` wide (logical) at `(x, y)` in its block, in `taille`
/// with `base`'s font, its words, backgrounds, lines and formulas added to `d`.
#[allow(clippy::too_many_arguments)]
pub fn poser(
    riche: Riche,
    taille: f32,
    famille: u8,
    x: f32,
    y: f32,
    largeur: f32,
    centre: bool,
    cx: &Contexte,
    d: &mut Dessin,
) -> Zone {
    let e = cx.echelle;
    let nom = cx.polices.nom(famille).to_string();
    let r = rapport(&nom);
    let mise: Mise = MISES.with_borrow_mut(|lc| {
        POLICES.with_borrow_mut(|fc| {
            let mut b = lc.ranged_builder(fc, &riche.texte, e, false);
            b.push_default(familles(&nom));
            b.push_default(StyleProperty::FontSize(taille));
            b.push_default(StyleProperty::LineHeight(
                parley::LineHeight::FontSizeRelative(r),
            ));
            b.push_default(StyleProperty::WordBreak(parley::WordBreak::Normal));
            b.push_default(StyleProperty::OverflowWrap(parley::OverflowWrap::Anywhere));
            for (plage, p) in &riche.styles {
                if plage.is_empty() || plage.end > riche.texte.len() {
                    continue;
                }
                b.push(StyleProperty::Brush(*p), plage.clone());
                if p.gras {
                    b.push(
                        StyleProperty::FontWeight(parley::FontWeight::BOLD),
                        plage.clone(),
                    );
                }
                if p.italique {
                    b.push(
                        StyleProperty::FontStyle(parley::FontStyle::Italic),
                        plage.clone(),
                    );
                }
                if p.famille != famille {
                    b.push(familles(cx.polices.nom(p.famille)), plage.clone());
                }
                if p.echelle != 0 {
                    b.push(StyleProperty::FontSize(p.taille(taille)), plage.clone());
                }
            }
            for (k, f) in riche.formules.iter().enumerate() {
                b.push_inline_box(InlineBox {
                    id: k as u64,
                    kind: InlineBoxKind::InFlow,
                    index: f.index.min(riche.texte.len()),
                    width: f.largeur * e,
                    // Above the baseline only; what hangs below is made room for
                    // after (`decalages`).
                    height: f.ligne_de_base.max(1.0) * e,
                });
            }
            let mut l = b.build(&riche.texte);
            l.break_all_lines(Some(largeur.max(10.0) * e));
            l.align(
                if centre {
                    Alignment::Center
                } else {
                    Alignment::Start
                },
                AlignmentOptions::default(),
            );
            l
        })
    });

    // Room under the lines whose formulas hang below them.
    let mut decalages = Vec::new();
    let mut cumul = 0.0f32;
    for ligne in mise.lines() {
        decalages.push(cumul);
        let m = ligne.metrics();
        let sous = (m.block_max_coord - m.baseline) / e;
        let mut pend = 0.0f32;
        for item in ligne.items() {
            if let PositionedLayoutItem::InlineBox(b) = item {
                if let Some(f) = riche.formules.get(b.id as usize) {
                    pend = pend.max(f.hauteur - f.ligne_de_base);
                }
            }
        }
        cumul += (pend - sous).max(0.0);
    }

    // What is drawn.
    for (i, ligne) in mise.lines().enumerate() {
        let s = decalages.get(i).copied().unwrap_or(0.0);
        let m = ligne.metrics();
        let haut = m.block_min_coord / e + s;
        let bas = m.block_max_coord / e + s;
        for item in ligne.items() {
            match item {
                PositionedLayoutItem::GlyphRun(gr) => {
                    let run = gr.run();
                    let p = gr.style().brush;
                    let plage = run.text_range();
                    let texte = riche
                        .texte
                        .get(plage)
                        .unwrap_or("")
                        .trim_end_matches(['\n', '\r']);
                    let x0 = x + gr.offset() / e;
                    let avance = gr.advance() / e;
                    if p.fond != 0 && !texte.is_empty() {
                        d.rect(
                            x0 - 1.0,
                            y + haut + 1.0,
                            avance + 2.0,
                            bas - haut - 2.0,
                            p.fond,
                            3.0,
                        );
                    }
                    if texte.trim().is_empty() {
                        continue;
                    }
                    let mt = run.metrics();
                    let t = run.font_size() / e;
                    let base = y + s + gr.baseline() / e;
                    let lh = t * rapport(cx.polices.nom(p.famille));
                    let asc = mt.ascent / e;
                    let desc = mt.descent / e;
                    let top = base - asc - (lh - asc - desc) / 2.0;
                    d.mot(x0, top, texte, t, p, cx.polices);
                    if p.souligne {
                        let ep = (mt.underline_size / e).max(1.0);
                        d.rect(
                            x0,
                            base - mt.underline_offset / e,
                            avance,
                            ep,
                            p.couleur,
                            0.0,
                        );
                    }
                    if p.barre {
                        let ep = (mt.strikethrough_size / e).max(1.0);
                        d.rect(
                            x0,
                            base - mt.strikethrough_offset / e,
                            avance,
                            ep,
                            p.couleur,
                            0.0,
                        );
                    }
                }
                PositionedLayoutItem::InlineBox(b) => {
                    if let Some(f) = riche.formules.get(b.id as usize) {
                        d.images.push(NoteImageData {
                            x: x + b.x / e,
                            y: y + s + b.y / e,
                            w: f.largeur,
                            h: f.hauteur,
                            image: f.image.clone(),
                        });
                    }
                }
            }
        }
    }
    let hauteur = mise.height() / e + cumul;
    // An empty text still has a line, for the cursor.
    let hauteur = if riche.texte.is_empty() {
        hauteur.max(taille * r)
    } else {
        hauteur
    };
    Zone {
        x,
        y,
        mise,
        riche,
        decalages,
        hauteur,
        echelle: e,
    }
}

/// The width of a word in a size and font (logical), for marks drawn beside the text.
pub fn largeur_de(texte: &str, taille: f32, famille: u8, cx: &Contexte) -> f32 {
    let nom = cx.polices.nom(famille).to_string();
    MISES.with_borrow_mut(|lc| {
        POLICES.with_borrow_mut(|fc| {
            let mut b = lc.ranged_builder(fc, texte, cx.echelle, false);
            b.push_default(familles(&nom));
            b.push_default(StyleProperty::FontSize(taille));
            let mut l = b.build(texte);
            l.break_all_lines(None);
            l.width() / cx.echelle
        })
    })
}

/// What a click on a block can do besides placing the cursor.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Tick or untick the checkbox.
    Cocher,
    /// Fold or unfold the callout.
    Plier,
    /// An embedded spreadsheet's cell (row, column as shown).
    Cellule(u32, u32),
    /// An embedded spreadsheet's column edge (the column it ends).
    Bord(usize),
    /// An embedded spreadsheet's buttons.
    OuvrirTableur,
    CouperMots,
}

/// A place of a block that does something when clicked.
#[derive(Debug, Clone)]
pub struct Cible {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub action: Action,
    /// The pointer over it: 1 a hand, 2 a column's edge, 3 an arrow.
    pub curseur: i32,
}

/// An embedded spreadsheet's cells as drawn: how many rows and columns, each cell's
/// rectangle (row by row), its columns' widths in the sheet and the factor they were
/// narrowed by.
#[derive(Debug, Clone, Default)]
pub struct Grille {
    pub lignes: u32,
    pub colonnes: u32,
    pub cases: Vec<(f32, f32, f32, f32)>,
    pub largeurs: Vec<f32>,
    pub echelle: f32,
}

/// A block laid out.
pub struct BlocVu {
    pub hauteur: f32,
    pub dessin: Dessin,
    pub zones: Vec<Zone>,
    pub cibles: Vec<Cible>,
    pub grille: Option<Grille>,
    /// The bytes at its start hidden while it is written (`## `, `- `), where the cursor
    /// never goes.
    pub marque: usize,
}

impl BlocVu {
    /// The zone that shows a block-source offset, and where in it.
    pub fn zone_de(&self, local: usize) -> Option<(usize, usize)> {
        let mut meilleure: Option<(usize, usize, usize)> = None;
        for (k, z) in self.zones.iter().enumerate() {
            let (a, b) = z.etendue();
            let distance = a.saturating_sub(local).max(local.saturating_sub(b));
            if meilleure.is_none_or(|(_, _, d)| distance < d) {
                meilleure = Some((k, z.riche.vers_affiche(local.clamp(a, b)), distance));
            }
            if distance == 0 {
                break;
            }
        }
        meilleure.map(|(k, aff, _)| (k, aff))
    }

    /// The cursor at a block-source offset: left, top, height (block coordinates).
    pub fn curseur(&self, local: usize) -> Option<(f32, f32, f32)> {
        let (k, aff) = self.zone_de(local)?;
        let z = &self.zones[k];
        let (x, y, h) = z.curseur(aff);
        Some((z.x + x, z.y + y, h))
    }

    /// The block-source offset under a point (block coordinates), from the zone nearest.
    pub fn point(&self, x: f32, y: f32) -> Option<usize> {
        let z = self.zones.iter().min_by(|a, b| {
            let d = |z: &Zone| {
                let dy = if y < z.y {
                    z.y - y
                } else if y > z.y + z.hauteur {
                    y - z.y - z.hauteur
                } else {
                    0.0
                };
                let dx = if x < z.x { z.x - x } else { 0.0 };
                dy * 4.0 + dx
            };
            d(a).partial_cmp(&d(b)).unwrap_or(std::cmp::Ordering::Equal)
        })?;
        let aff = z.point(x - z.x, y - z.y);
        Some(z.riche.vers_source(aff).max(self.marque))
    }

    /// The link under a point, when there is one.
    pub fn lien(&self, x: f32, y: f32) -> Option<String> {
        for z in &self.zones {
            if y < z.y || y > z.y + z.hauteur || x < z.x {
                continue;
            }
            let aff = z.point(x - z.x, y - z.y);
            // `from_point` gives the nearest edge: the character after it, or before.
            for a in [aff, aff.saturating_sub(1)] {
                if let Some(l) = z.riche.lien(a) {
                    // Only over the words themselves, not the room after the line.
                    let (cx, _, _) = z.curseur(a);
                    let (fx, _, _) = z.curseur(a + 1);
                    let px = x - z.x;
                    if px >= cx.min(fx) - 1.0 && px <= cx.max(fx) + 1.0 {
                        return Some(l.to_string());
                    }
                }
            }
        }
        None
    }

    /// The rectangles covering block-source offsets `a` to `b` (block coordinates).
    pub fn rectangles(&self, a: usize, b: usize) -> Vec<(f32, f32, f32, f32)> {
        let mut sortie = Vec::new();
        for z in &self.zones {
            let (debut, fin) = z.etendue();
            if b < debut || a > fin {
                continue;
            }
            let (da, db) = (
                z.riche.vers_affiche(a.max(debut)),
                z.riche.vers_affiche(b.min(fin)),
            );
            if db > da {
                sortie.extend(
                    z.rectangles(da, db)
                        .into_iter()
                        .map(|(x, y, w, h)| (z.x + x, z.y + y, w, h)),
                );
            } else if a <= debut && b >= fin {
                // An empty line selected: a sliver says so.
                let (x, y, h) = z.curseur(0);
                sortie.push((z.x + x, z.y + y, 5.0, h));
            }
        }
        sortie
    }

    /// The cible under a point.
    pub fn cible(&self, x: f32, y: f32) -> Option<&Cible> {
        self.cibles
            .iter()
            .rev()
            .find(|c| x >= c.x && x <= c.x + c.w && y >= c.y && y <= c.y + c.h)
    }
}
