//! The editor's keys, mouse and cursor.
//!
//! The note is one text and one selection (`ancre`, `tete`, byte offsets in it): the
//! cursor goes from block to block, a selection runs across them, formulas and tables
//! included. Each block is laid out by `editeur` once per change; the cursor, the
//! selection, Up and Down and every click are measured in those layouts. What a key
//! means for the note (Enter continuing a list, a mark around words) is still decided
//! by `touche`, block by block, as before; this module moves the cursor, types,
//! deletes, selects, and draws.

use super::editeur::{self, dessin, Action, BlocVu, Contexte, Couleurs, Polices};
use super::{
    appliquer, blocs_de, cellules, contenu, render, retenir, signature, touche, Curseur, Etat,
};
use iris_notes::block::{self, BlockKind};
use iris_notes::edit::{self, NoteEdit};
use iris_ui::{AppWindow, NoteRectData, NoteSelect, NoteViewBlock};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Two clicks closer than this, at the same place, are a double click.
const DOUBLE: Duration = Duration::from_millis(450);

/// What the mouse is doing while its button is held.
#[derive(Debug, Clone, Copy)]
enum Glisse {
    /// Selecting from the press.
    Texte,
    /// Selecting word by word, from the word pressed.
    Mots(usize, usize),
    /// Carrying an embedded spreadsheet's column edge.
    Bord {
        bloc: usize,
        colonne: usize,
        x0: f32,
    },
    /// Carrying a picture elsewhere in the note, from where it was pressed.
    Image { bloc: usize, y0: f32 },
    /// Carrying a picture's corner to resize it, from its width when pressed.
    Taille {
        bloc: usize,
        coin: u8,
        x0: f32,
        largeur: f32,
    },
}

/// The editor's state.
pub(super) struct Editeur {
    pub vue: Vec<BlocVu>,
    donnees: Vec<NoteViewBlock>,
    pub ys: Vec<f32>,
    sigs: Vec<u64>,
    pub largeur: f32,
    /// The selection: where it started and where the cursor is (the same when none).
    pub ancre: usize,
    pub tete: usize,
    /// Where Up and Down aim, across lines of different lengths.
    x_voulu: Option<f32>,
    glisse: Option<Glisse>,
    clic: Option<(Instant, f32, f32, u32)>,
    /// While a picture is carried: where it would go (the note's coordinates), or the
    /// rectangle it would take once resized.
    repere: Option<(f32, f32, f32, f32)>,
    pub modele: Rc<VecModel<NoteViewBlock>>,
}

impl Editeur {
    pub fn nouveau() -> Self {
        Self {
            vue: Vec::new(),
            donnees: Vec::new(),
            ys: Vec::new(),
            sigs: Vec::new(),
            largeur: 640.0,
            ancre: 0,
            tete: 0,
            x_voulu: None,
            glisse: None,
            clic: None,
            repere: None,
            modele: Rc::new(VecModel::default()),
        }
    }

    fn bornes(&self) -> (usize, usize) {
        (self.ancre.min(self.tete), self.ancre.max(self.tete))
    }

    /// The block at `y` (the note's coordinates).
    fn bloc_a(&self, y: f32) -> usize {
        self.ys.iter().rposition(|&h| h <= y).unwrap_or(0)
    }
}

fn argb(c: slint::Color) -> u32 {
    c.as_argb_encoded()
}

/// The colours and fonts of the theme shown.
fn theme(f: &AppWindow, e: &Etat) -> (Couleurs, Polices) {
    let t = f.global::<iris_ui::Tokens>();
    let c = Couleurs {
        texte: argb(t.get_text()),
        muet: argb(t.get_text_muted()),
        secondaire: argb(t.get_text_secondary()),
        accent: argb(t.get_accent()),
        maths: 0xff00_0000 | e.palette.math,
        plaque: argb(t.get_surface_low()),
        bordure: argb(t.get_border()),
        bordure_forte: argb(t.get_border_strong()),
        selection: (argb(t.get_accent()) & 0x00ff_ffff) | 0x4d00_0000,
        palette: e.palette.clone(),
    };
    let p = Polices {
        corps: t.get_font_family().to_string(),
        mono: t.get_font_mono().to_string(),
        titre: t.get_font_display().to_string(),
    };
    (c, p)
}

/// The block holding the cursor, or none.
fn actif(e: &Etat) -> Option<usize> {
    (e.focus >= 0).then_some(e.focus as usize)
}

/// Every block laid out again where its text, its number, whether it is written or the
/// column's width changed (`tout`: every one), placed one under the other, and given to
/// the window.
pub(super) fn vue(f: &AppWindow, e: &mut Etat, tout: bool) {
    let Some(note) = &e.note else {
        e.ed.vue.clear();
        e.ed.donnees.clear();
        e.ed.sigs.clear();
        e.ed.ys.clear();
        e.ed.modele.set_vec(Vec::new());
        f.set_note_view_height(0.0);
        return;
    };
    if e.source {
        e.ed.modele.set_vec(Vec::new());
        return;
    }
    let (couleurs, polices) = theme(f, e);
    let echelle = f.window().scale_factor();
    let encre = [
        (couleurs.texte >> 16) as u8,
        (couleurs.texte >> 8) as u8,
        couleurs.texte as u8,
    ];
    let formule = move |latex: &str, taille: f32| {
        iris_math::render(
            latex,
            iris_math::Style {
                display: false,
                size: taille,
                scale: echelle,
                colour: encre,
            },
        )
        .ok()
    };
    let formule_bloc = move |latex: &str| {
        iris_math::render(
            latex,
            iris_math::Style {
                display: true,
                size: 16.0,
                scale: echelle,
                colour: encre,
            },
        )
        .ok()
    };
    let dir = e.espace().map(|s| s.dir().to_path_buf());
    let cx = Contexte {
        echelle,
        largeur: e.ed.largeur,
        couleurs: &couleurs,
        polices: &polices,
        dir: dir.as_deref(),
        formule: &formule,
        formule_bloc: &formule_bloc,
    };
    // While words are selected every block reads as printed: the selection covers
    // what is seen, a table as a table, as in Word.
    let (a, b) = e.ed.bornes();
    let actif = if e.reading || a != b { None } else { actif(e) };
    let (_, local_tete) = block::locate(&note.blocks, e.ed.tete.min(note.text.len()));
    let numeros = render::numbering(&note.blocks);
    let n = note.blocks.len();
    let tout = tout || e.ed.vue.len() != n;
    if tout {
        e.ed.vue.clear();
        e.ed.donnees.clear();
        e.ed.sigs.clear();
    }
    let mut changes = Vec::new();
    for (i, (b, num)) in note.blocks.iter().zip(&numeros).enumerate() {
        // The block written depends on where its cursor is (the mark it touches).
        let curseur = (actif == Some(i)).then_some(local_tete);
        let sig = signature(b, num)
            ^ curseur.map_or(0, |c| 0x9e37_79b9_7f4a_7c15 ^ (c as u64).rotate_left(29))
            ^ u64::from(e.ed.largeur.to_bits()).rotate_left(17);
        if !tout && e.ed.sigs.get(i) == Some(&sig) {
            continue;
        }
        let bv = dessin::bloc(b, num, curseur, &cx);
        let d = donnees(&bv);
        if tout {
            e.ed.vue.push(bv);
            e.ed.donnees.push(d);
            e.ed.sigs.push(sig);
        } else {
            e.ed.vue[i] = bv;
            e.ed.donnees[i] = d;
            e.ed.sigs[i] = sig;
        }
        changes.push(i);
    }
    // One under the other.
    let mut y = 0.0;
    let mut ys = Vec::with_capacity(n);
    for bv in &e.ed.vue {
        ys.push(y);
        y += bv.hauteur;
    }
    let anciens = std::mem::replace(&mut e.ed.ys, ys);
    if tout || e.ed.modele.row_count() != n {
        let lignes: Vec<NoteViewBlock> =
            e.ed.donnees
                .iter()
                .zip(&e.ed.ys)
                .map(|(d, y)| NoteViewBlock { y: *y, ..d.clone() })
                .collect();
        e.ed.modele.set_vec(lignes);
    } else {
        for i in 0..n {
            let bouge = anciens.get(i) != Some(&e.ed.ys[i]);
            if bouge || changes.contains(&i) {
                e.ed.modele.set_row_data(
                    i,
                    NoteViewBlock {
                        y: e.ed.ys[i],
                        ..e.ed.donnees[i].clone()
                    },
                );
            }
        }
    }
    f.set_note_view_height(y);
}

/// What the window draws of a block.
fn donnees(bv: &BlocVu) -> NoteViewBlock {
    NoteViewBlock {
        y: 0.0,
        h: bv.hauteur,
        runs: ModelRc::new(VecModel::from(bv.dessin.runs.clone())),
        rects: ModelRc::new(VecModel::from(bv.dessin.rects.clone())),
        images: ModelRc::new(VecModel::from(bv.dessin.images.clone())),
        icons: ModelRc::new(VecModel::from(bv.dessin.icones.clone())),
    }
}

/// A global offset's block and the offset in it.
fn situer(e: &Etat, global: usize) -> (usize, usize) {
    match &e.note {
        Some(n) => block::locate(&n.blocks, global.min(n.text.len())),
        None => (0, 0),
    }
}

fn debut(e: &Etat, i: usize) -> usize {
    e.note.as_ref().map_or(0, |n| block::start_of(&n.blocks, i))
}

/// The bytes at a block's start the cursor never goes into while it is written.
fn marque(e: &Etat, i: usize) -> usize {
    contenu(e, i).map_or(0, |(k, t)| render::prefixe(&k, &t))
}

/// An offset kept out of the hidden mark of its block.
fn hors_marque(e: &Etat, global: usize) -> usize {
    let (i, local) = situer(e, global);
    let m = marque(e, i);
    if local < m {
        debut(e, i) + m
    } else {
        global
    }
}

/// The cursor and the selection given to the window; what pops up over the cursor.
pub(super) fn montrer(f: &AppWindow, e: &mut Etat) {
    let g = f.global::<NoteSelect>();
    let mut marques: Vec<NoteRectData> = Vec::new();
    let (couleurs, _) = theme(f, e);
    // An embedded spreadsheet's cells selected.
    if let Some(ch) = e.cellules.choix {
        if let (Some(bv), Some(y0)) = (e.ed.vue.get(ch.block), e.ed.ys.get(ch.block)) {
            if let Some(gr) = &bv.grille {
                let (r1, r2, c1, c2, r, c) = cellules::bornes(&ch);
                for (k, (x, y, w, h)) in gr.cases.iter().enumerate() {
                    let (rr, cc) = (
                        (k as u32) / gr.colonnes.max(1),
                        (k as u32) % gr.colonnes.max(1),
                    );
                    if rr >= r1 && rr <= r2 && cc >= c1 && cc <= c2 {
                        marques.push(rect(
                            *x,
                            y0 + y,
                            *w,
                            *h,
                            couleurs.selection & 0x40ff_ffff,
                            0.0,
                            0,
                            0.0,
                        ));
                    }
                    if rr == r && cc == c {
                        marques.push(rect(*x, y0 + y, *w, *h, 0, 0.0, couleurs.accent, 2.0));
                        g.set_edit_x(*x);
                        g.set_edit_y(y0 + y);
                        g.set_edit_w(*w);
                        g.set_edit_h(*h);
                    }
                }
            }
        }
    }
    // A picture carried: where it would go, or the size it would take.
    if let Some((x, y, w, h)) = e.ed.repere {
        if h <= 3.0 {
            marques.push(rect(x, y, w, h.max(2.0), couleurs.accent, 1.0, 0, 0.0));
        } else {
            marques.push(rect(x, y, w, h, 0, 2.0, couleurs.accent, 2.0));
        }
    }
    let Some(i) = actif(e).filter(|_| !e.reading && e.note.is_some()) else {
        f.set_note_caret_visible(false);
        f.set_note_marks(ModelRc::new(VecModel::from(marques)));
        return;
    };
    let (a, b) = e.ed.bornes();
    // The selection, block by block.
    if a < b {
        let (ia, la) = situer(e, a);
        let (ib, lb) = situer(e, b);
        for k in ia..=ib.min(e.ed.vue.len().saturating_sub(1)) {
            let Some(bv) = e.ed.vue.get(k) else { continue };
            let n = contenu(e, k).map_or(0, |(_, t)| t.len());
            let de = if k == ia { la } else { 0 };
            let a_ = if k == ib { lb } else { n };
            // A table, a spreadsheet, a picture, a formula drawn: selected whole.
            if bv.zones.is_empty() || bv.grille.is_some() || bv.objet.is_some() {
                if de == 0 && a_ >= n {
                    marques.push(rect(
                        -4.0,
                        e.ed.ys[k],
                        e.ed.largeur + 8.0,
                        bv.hauteur,
                        couleurs.selection,
                        4.0,
                        0,
                        0.0,
                    ));
                }
                continue;
            }
            for (x, y, w, h) in bv.rectangles(de, a_) {
                marques.push(rect(
                    x,
                    e.ed.ys[k] + y,
                    w,
                    h,
                    couleurs.selection,
                    2.0,
                    0,
                    0.0,
                ));
            }
        }
    }
    f.set_note_marks(ModelRc::new(VecModel::from(marques)));
    // The cursor.
    let (bi, local) = situer(e, e.ed.tete);
    let place = e.ed.vue.get(bi).and_then(|bv| bv.curseur(local));
    match place {
        Some((x, y, h)) => {
            f.set_note_caret_x(x);
            f.set_note_caret_y(e.ed.ys.get(bi).copied().unwrap_or(0.0) + y);
            f.set_note_caret_h(h);
            f.set_note_caret_visible(e.cellules.choix.is_none());
        }
        None => f.set_note_caret_visible(false),
    }
    f.set_note_caret_serial(f.get_note_caret_serial() + 1);
    // What pops up reads the cursor as a block and offsets in it.
    let (ba, la) = situer(e, e.ed.ancre);
    e.caret = Curseur {
        block: bi,
        cursor: local,
        anchor: if ba == bi { la } else { local },
    };
    let _ = i;
    super::apres_curseur(f, e);
}

#[allow(clippy::too_many_arguments)]
fn rect(x: f32, y: f32, w: f32, h: f32, c: u32, rayon: f32, bord: u32, epais: f32) -> NoteRectData {
    NoteRectData {
        x,
        y,
        w,
        h,
        color: editeur::couleur(c),
        radius: rayon,
        border: editeur::couleur(bord),
        border_width: epais,
    }
}

/// The selection set (`ancre` to `tete`): the block holding the cursor becomes the one
/// written, laid out again with its marks, and everything is shown.
pub(super) fn placer(f: &AppWindow, e: &mut Etat, ancre: usize, tete: usize) {
    let len = e.note.as_ref().map_or(0, |n| n.text.len());
    let tete = hors_marque(e, tete.min(len));
    let ancre = hors_marque(e, ancre.min(len));
    e.ed.ancre = ancre;
    e.ed.tete = tete;
    let (bi, _) = situer(e, tete);
    e.focus = bi as i32;
    // The block written follows the cursor, mark by mark: only what changed is laid out
    // again.
    vue(f, e, false);
    montrer(f, e);
}

/// Whether block `i` is one thing rather than words: a picture or a spreadsheet,
/// selected whole by the cursor.
fn objet(e: &Etat, i: usize) -> bool {
    e.ed.vue
        .get(i)
        .is_some_and(|bv| bv.objet.is_some() || bv.grille.is_some())
}

/// The keyboard to the editor.
pub(super) fn clavier(f: &AppWindow) {
    f.set_note_canvas_focus_serial(f.get_note_canvas_focus_serial() + 1);
}

/// The selection's text (the source, marks and all), when there is one.
fn texte_choisi(e: &Etat) -> Option<String> {
    let (a, b) = e.ed.bornes();
    let note = e.note.as_ref()?;
    (a < b).then(|| note.text[a..b.min(note.text.len())].to_string())
}

/// The note's bytes `a` to `b` replaced by `texte`, the cursor at `curseur` (global).
fn remplacer_plage(f: &AppWindow, e: &mut Etat, a: usize, b: usize, texte: &str, curseur: usize) {
    let Some(note) = &e.note else { return };
    let (a, b) = (a.min(note.text.len()), b.min(note.text.len()));
    let t = format!("{}{texte}{}", &note.text[..a], &note.text[b..]);
    let blocs = blocs_de(&t);
    let (bloc, local) = block::locate(&blocs, curseur.min(t.len()));
    retenir(e, true);
    appliquer(
        f,
        e,
        NoteEdit {
            text: t,
            block: bloc,
            cursor: local,
        },
        true,
        None,
    );
}

/// The selection taken out; true when there was one.
fn effacer_selection(f: &AppWindow, e: &mut Etat) -> bool {
    let (a, b) = e.ed.bornes();
    if a == b {
        return false;
    }
    remplacer_plage(f, e, a, b, "", a);
    true
}

/// Block `i`'s source made `nouveau`, the cursor at `curseur` in it: what typing does
/// (a sign just typed made, `->` into `→`).
pub(super) fn editer_bloc(f: &AppWindow, e: &mut Etat, i: usize, nouveau: &str, curseur: usize) {
    let Some(note) = &e.note else { return };
    if i >= note.blocks.len() {
        return;
    }
    let signe = match contenu(e, i) {
        Some((kind, avant))
            if !matches!(
                kind,
                BlockKind::Code { .. } | BlockKind::Math | BlockKind::Table | BlockKind::Properties
            ) && nouveau.len() == avant.len() + 1 =>
        {
            edit::typographic(nouveau, curseur)
        }
        _ => None,
    };
    let Some(note) = &e.note else { return };
    let ed = edit::replace_block(&note.blocks, i, nouveau, curseur);
    let bloc = ed.block;
    retenir(e, false);
    appliquer(f, e, ed, false, None);
    if let Some(r) = signe {
        // Only while the line is still what was typed (its block the same).
        if let Some(note) = e
            .note
            .as_ref()
            .filter(|n| n.blocks.get(bloc).is_some_and(|b| b.content() == nouveau))
        {
            let ed = edit::replace_block(&note.blocks, bloc, &r.text, r.cursor);
            retenir(e, true);
            appliquer(f, e, ed, true, None);
        }
    }
}

/// A character (or several, pasted plain) typed at the cursor.
fn taper(f: &AppWindow, e: &mut Etat, texte: &str) {
    effacer_selection(f, e);
    let (i, c) = situer(e, e.ed.tete);
    // What a key means here first: a line start converted, a pair closed.
    if texte.chars().count() == 1 && touche(f, e, i, texte, c, c) {
        return;
    }
    let Some((_, content)) = contenu(e, i) else {
        return;
    };
    let c = c.min(content.len());
    let nouveau = format!("{}{texte}{}", &content[..c], &content[c..]);
    editer_bloc(f, e, i, &nouveau, c + texte.len());
}

/// The character boundary before or after `k` in `t`.
fn voisin(t: &str, k: usize, avant: bool) -> usize {
    if avant {
        t[..k.min(t.len())]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    } else {
        t[k.min(t.len())..]
            .chars()
            .next()
            .map_or(t.len(), |c| k + c.len_utf8())
    }
}

/// The start or end of the word before or after `k`, as Ctrl and an arrow goes.
fn mot_voisin(t: &str, k: usize, avant: bool) -> usize {
    let est_mot = |c: char| c.is_alphanumeric() || c == '_';
    if avant {
        let mut i = k.min(t.len());
        let debut = &t[..i];
        let mut it = debut.char_indices().rev().peekable();
        // Spaces, then the word.
        while let Some(&(j, c)) = it.peek() {
            if est_mot(c) {
                break;
            }
            i = j;
            it.next();
        }
        while let Some(&(j, c)) = it.peek() {
            if !est_mot(c) {
                break;
            }
            i = j;
            it.next();
        }
        i
    } else {
        let mut i = k.min(t.len());
        let mut it = t[i..].chars().peekable();
        while let Some(&c) = it.peek() {
            if est_mot(c) {
                break;
            }
            i += c.len_utf8();
            it.next();
        }
        while let Some(&c) = it.peek() {
            if !est_mot(c) {
                break;
            }
            i += c.len_utf8();
            it.next();
        }
        i
    }
}

/// The word around `k`.
fn mot_autour(t: &str, k: usize) -> (usize, usize) {
    let est_mot = |c: char| c.is_alphanumeric() || c == '_';
    let k = k.min(t.len());
    let mut a = k;
    for (j, c) in t[..k].char_indices().rev() {
        if !est_mot(c) {
            break;
        }
        a = j;
    }
    let mut b = k;
    for c in t[k..].chars() {
        if !est_mot(c) {
            break;
        }
        b += c.len_utf8();
    }
    if a == b {
        // Not on a word: the character.
        b = voisin(t, k, false);
    }
    (a, b)
}

/// Left or Right: a character (or a word, Ctrl), past a block's hidden mark to the
/// block beside; the selection collapsed to its side, or grown (Shift).
fn horizontal(f: &AppWindow, e: &mut Etat, gauche: bool, etendre: bool, par_mot: bool) {
    let Some(note) = &e.note else { return };
    let t = note.text.clone();
    let (a, b) = e.ed.bornes();
    e.ed.x_voulu = None;
    if !etendre && a != b {
        let p = if gauche { a } else { b };
        placer(f, e, p, p);
        return;
    }
    let mut p = e.ed.tete;
    let (i, local) = situer(e, p);
    if gauche && local <= marque(e, i) && i > 0 {
        // Into the line above, at its end.
        let n = contenu(e, i - 1).map_or(0, |(_, t)| t.len());
        p = debut(e, i - 1) + n;
    } else if !gauche
        && contenu(e, i).is_some_and(|(_, c)| local >= c.len())
        && i + 1 < note.blocks.len()
    {
        p = debut(e, i + 1) + marque(e, i + 1);
    } else if par_mot {
        p = mot_voisin(&t, p, gauche);
    } else {
        p = voisin(&t, p, gauche);
    }
    let ancre = if etendre { e.ed.ancre } else { p };
    placer(f, e, ancre, p);
}

/// Up or Down a line, aiming at the same left, across blocks.
fn vertical(f: &AppWindow, e: &mut Etat, haut: bool, etendre: bool, lignes: usize) {
    let (bi, local) = situer(e, e.ed.tete);
    let Some((x, y, h)) = e.ed.vue.get(bi).and_then(|bv| bv.curseur(local)) else {
        return;
    };
    let y0 = e.ed.ys.get(bi).copied().unwrap_or(0.0);
    let xg = *e.ed.x_voulu.get_or_insert(x);
    let total: f32 =
        e.ed.ys.last().copied().unwrap_or(0.0) + e.ed.vue.last().map_or(0.0, |b| b.hauteur);
    let mut yc = if haut { y0 + y - 1.0 } else { y0 + y + h + 1.0 };
    let mut cible = None;
    let mut restantes = lignes.max(1);
    for _ in 0..(400 * lignes.max(1)) {
        if yc < 0.0 {
            cible = Some(0);
            break;
        }
        if yc >= total {
            cible = Some(e.note.as_ref().map_or(0, |n| n.text.len()));
            break;
        }
        let bj = e.ed.bloc_a(yc);
        let Some(bv) = e.ed.vue.get(bj) else { break };
        let yj = e.ed.ys[bj];
        let Some(lj) = bv.point(xg, yc - yj) else {
            // A picture or a spreadsheet on the way: the cursor stops on it, whole.
            if objet(e, bj) && bj != bi {
                cible = Some(debut(e, bj));
                break;
            }
            yc = if haut {
                yj - 1.0
            } else {
                yj + bv.hauteur + 1.0
            };
            continue;
        };
        let (_, ly, lh) = bv.curseur(lj).unwrap_or((0.0, 0.0, h));
        let ligne_y = yj + ly;
        if (ligne_y - (y0 + y)).abs() < 1.0 {
            // The same line still (a block's margin): further.
            yc = if haut {
                (ligne_y - 1.0).min(yc - 2.0)
            } else {
                (ligne_y + lh + 1.0).max(yc + 2.0)
            };
            continue;
        }
        cible = Some(debut(e, bj) + lj);
        restantes -= 1;
        if restantes == 0 {
            break;
        }
        yc = if haut {
            ligne_y - 1.0
        } else {
            ligne_y + lh + 1.0
        };
    }
    let Some(p) = cible else { return };
    let ancre = if etendre { e.ed.ancre } else { p };
    let x_garde = e.ed.x_voulu;
    placer(f, e, ancre, p);
    e.ed.x_voulu = x_garde;
}

/// Home and End: the line's start or end (Ctrl: the note's).
fn bout(f: &AppWindow, e: &mut Etat, fin: bool, etendre: bool, note_entiere: bool) {
    let len = e.note.as_ref().map_or(0, |n| n.text.len());
    let p = if note_entiere {
        if fin {
            len
        } else {
            0
        }
    } else {
        let (bi, local) = situer(e, e.ed.tete);
        let Some(bv) = e.ed.vue.get(bi) else { return };
        let Some((_, y, h)) = bv.curseur(local) else {
            return;
        };
        let x = if fin { 1.0e6 } else { -1.0e6 };
        debut(e, bi) + bv.point(x, y + h / 2.0).unwrap_or(local)
    };
    e.ed.x_voulu = None;
    let ancre = if etendre { e.ed.ancre } else { p };
    placer(f, e, ancre, p);
}

/// A key while the cursor is on picture or spreadsheet `i`: Backspace and Delete take it
/// away, Enter or a character starts a line under it, the arrows leave it, Ctrl+C and
/// Ctrl+X copy its line. `None`: the key means what it always does.
fn sur_objet(f: &AppWindow, e: &mut Etat, i: usize, bas: &str, nom: &str) -> Option<bool> {
    let n = e.note.as_ref().map_or(0, |n| n.blocks.len());
    let fin = debut(e, i) + contenu(e, i).map_or(0, |(_, t)| t.len());
    match bas {
        "backspace" | "delete" => {
            let note = e.note.as_ref()?;
            let ed = edit::replace_blocks(&note.blocks, i, i, "");
            retenir(e, true);
            appliquer(f, e, ed, true, None);
            Some(true)
        }
        "enter" => {
            remplacer_plage(f, e, fin, fin, "\n", fin + 1);
            Some(true)
        }
        "up" | "left" => {
            if i > 0 {
                let p = debut(e, i - 1) + contenu(e, i - 1).map_or(0, |(_, t)| t.len());
                placer(f, e, p, p);
            }
            Some(true)
        }
        "down" | "right" => {
            if i + 1 < n {
                let p = debut(e, i + 1) + marque(e, i + 1);
                placer(f, e, p, p);
            }
            Some(true)
        }
        "ctrl+c" | "ctrl+x" => {
            if let (Some((_, t)), Ok(mut p)) = (contenu(e, i), arboard::Clipboard::new()) {
                let _ = p.set_text(t);
            }
            if bas == "ctrl+x" {
                return sur_objet(f, e, i, "delete", nom);
            }
            Some(true)
        }
        _ => {
            let t = nom.strip_prefix("type:")?;
            if t.chars()
                .any(|c| c.is_control() || ('\u{f700}'..='\u{f8ff}').contains(&c))
            {
                return Some(false);
            }
            // Typing on it writes a new line under it.
            let ajout = format!("\n{t}");
            remplacer_plage(f, e, fin, fin, &ajout, fin + ajout.len());
            Some(true)
        }
    }
}

/// The clipboard's text.
fn presse_papiers() -> String {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut p| p.get_text().ok())
        .unwrap_or_default()
}

/// A key while the editor has the keyboard. True when it was used.
pub(super) fn touche_editeur(f: &AppWindow, e: &mut Etat, nom: &str) -> bool {
    // An embedded spreadsheet's cells take the keys while they are selected.
    if e.cellules.choix.is_some() {
        let pris = cellules::touche(f, e, nom);
        montrer(f, e);
        return pris;
    }
    if e.note.is_none() || e.reading {
        return false;
    }
    if e.focus < 0 {
        // No cursor: an arrow or a key puts it at the top.
        if matches!(nom, "up" | "down" | "left" | "right") || nom.starts_with("type:") {
            placer(f, e, 0, 0);
            return true;
        }
        return false;
    }
    let (i, c) = situer(e, e.ed.tete);
    let (a, b) = e.ed.bornes();
    let (ia, _) = situer(e, a);
    let (ib, _) = situer(e, b);
    let dans_le_bloc = ia == ib;
    let ancre_locale = if dans_le_bloc {
        e.ed.ancre - debut(e, i)
    } else {
        c
    };
    // The list over the cursor takes the keys that choose in it.
    if e.completion.is_some() {
        let k = match nom {
            "up" => Some("popup-up"),
            "down" => Some("popup-down"),
            "enter" | "tab" => Some("popup-accept"),
            "escape" => Some("popup-close"),
            _ => None,
        };
        if let Some(k) = k {
            if touche(f, e, i, k, c, ancre_locale) {
                montrer(f, e);
                return true;
            }
        }
    }
    let bas = if nom.starts_with("type:") {
        nom.to_string()
    } else {
        nom.to_lowercase()
    };
    let etendre = bas.contains("shift+");
    let ctrl = bas.starts_with("ctrl+");
    let base = bas
        .trim_start_matches("ctrl+")
        .trim_start_matches("alt+")
        .trim_start_matches("shift+");
    let alt = bas.contains("alt+");
    // The cursor on a picture or a spreadsheet: it is one thing.
    if a == b && objet(e, i) {
        if let Some(pris) = sur_objet(f, e, i, &bas, nom) {
            return pris;
        }
    }
    if !alt {
        match base {
            "left" | "right" => {
                horizontal(f, e, base == "left", etendre, ctrl);
                return true;
            }
            "up" | "down" => {
                vertical(f, e, base == "up", etendre, 1);
                return true;
            }
            "pageup" | "pagedown" => {
                vertical(f, e, base == "pageup", etendre, 20);
                return true;
            }
            "home" | "end" => {
                bout(f, e, base == "end", etendre, ctrl);
                return true;
            }
            _ => {}
        }
    }
    if let Some(t) = nom.strip_prefix("type:") {
        let mut car = t.chars();
        if let Some(ch) = car.next() {
            if ch.is_control() || ('\u{f700}'..='\u{f8ff}').contains(&ch) {
                return false;
            }
        }
        taper(f, e, t);
        return true;
    }
    match bas.as_str() {
        "escape" => {
            if e.completion.is_some() || f.get_note_bubble() || !f.get_note_card().is_empty() {
                super::fermer_popups(f, e);
            } else if a != b {
                placer(f, e, e.ed.tete, e.ed.tete);
            } else {
                super::sans_focus(f, e);
                vue(f, e, false);
                montrer(f, e);
            }
            return true;
        }
        "ctrl+a" => {
            let len = e.note.as_ref().map_or(0, |n| n.text.len());
            placer(f, e, 0, len);
            return true;
        }
        "ctrl+c" => {
            if let Some(t) = texte_choisi(e) {
                if let Ok(mut p) = arboard::Clipboard::new() {
                    let _ = p.set_text(t);
                }
            }
            return true;
        }
        "ctrl+x" => {
            if let Some(t) = texte_choisi(e) {
                if let Ok(mut p) = arboard::Clipboard::new() {
                    let _ = p.set_text(t);
                }
                effacer_selection(f, e);
            }
            return true;
        }
        "ctrl+v" => {
            // A selection across blocks goes first; a picture, a web address over
            // words, formatted text are the note's own paste; plain text is typed.
            if !dans_le_bloc {
                effacer_selection(f, e);
            }
            let (i, c) = situer(e, e.ed.tete);
            let (a, b) = e.ed.bornes();
            let anc = if a != b { e.ed.ancre - debut(e, i) } else { c };
            if touche(f, e, i, "ctrl+v", c, anc) {
                return true;
            }
            let t = presse_papiers();
            if !t.is_empty() {
                let (a, b) = e.ed.bornes();
                let t = t.replace("\r\n", "\n");
                remplacer_plage(f, e, a, b, &t, a + t.len());
            }
            return true;
        }
        "backspace" | "ctrl+backspace" => {
            if effacer_selection(f, e) {
                return true;
            }
            let m = marque(e, i);
            if c <= m {
                // At the line's start: its mark goes, then it joins the line above.
                touche(f, e, i, "backspace-start", c, c);
                return true;
            }
            let Some((_, content)) = contenu(e, i) else {
                return true;
            };
            let k = if ctrl {
                mot_voisin(&content, c, true).max(m)
            } else {
                voisin(&content, c, true).max(m)
            };
            let nouveau = format!("{}{}", &content[..k], &content[c.min(content.len())..]);
            editer_bloc(f, e, i, &nouveau, k);
            return true;
        }
        "delete" | "ctrl+delete" => {
            if effacer_selection(f, e) {
                return true;
            }
            let Some((_, content)) = contenu(e, i) else {
                return true;
            };
            if c >= content.len() {
                touche(f, e, i, "delete", c, c);
                return true;
            }
            let k = if ctrl {
                mot_voisin(&content, c, false)
            } else {
                voisin(&content, c, false)
            };
            let nouveau = format!("{}{}", &content[..c], &content[k..]);
            editer_bloc(f, e, i, &nouveau, c);
            return true;
        }
        _ => {}
    }
    // Across blocks: what acts on whole blocks.
    if !dans_le_bloc {
        let premier = ia;
        let dernier = if b == debut(e, ib) && ib > ia {
            ib - 1
        } else {
            ib
        };
        return super::blocs::sur_blocs(f, e, &bas, premier, dernier);
    }
    // What a key means for the note, block by block, as before.
    let pris = touche(f, e, i, nom, c, ancre_locale);
    if pris {
        montrer(f, e);
    }
    pris
}

/// The pointer pressed at `(x, y)` (the editor's coordinates).
pub(super) fn presse(f: &AppWindow, e: &mut Etat, x: f32, y: f32, shift: bool, ctrl: bool) {
    if e.note.is_none() || e.ed.vue.is_empty() {
        return;
    }
    let bi = e.ed.bloc_a(y);
    let yb = y - e.ed.ys[bi];
    // A box to tick, a fold arrow, a cell, a column's edge, a button.
    if let Some(cible) = e.ed.vue[bi].cible(x, yb).cloned() {
        match cible.action {
            Action::Cocher => {
                super::cocher(f, e, bi);
                return;
            }
            Action::Plier => {
                super::plier(f, e, bi);
                return;
            }
            Action::Cellule(r, c) => {
                cellules::choisir(f, e, bi, r, c, shift);
                montrer(f, e);
                return;
            }
            Action::Bord(k) => {
                e.ed.glisse = Some(Glisse::Bord {
                    bloc: bi,
                    colonne: k,
                    x0: x,
                });
                return;
            }
            Action::OuvrirTableur => {
                super::option_tableur(f, e, bi, "open");
                return;
            }
            Action::CouperMots => {
                super::option_tableur(f, e, bi, "clip");
                return;
            }
            Action::Image => {
                if !e.reading {
                    cellules::quitter(f, e);
                    let p = debut(e, bi);
                    placer(f, e, p, p);
                    e.ed.glisse = Some(Glisse::Image { bloc: bi, y0: y });
                    clavier(f);
                }
                return;
            }
            Action::Poignee(coin) => {
                let largeur = e.ed.vue[bi].objet.map_or(100.0, |o| o.2);
                e.ed.glisse = Some(Glisse::Taille {
                    bloc: bi,
                    coin,
                    x0: x,
                    largeur,
                });
                return;
            }
        }
    }
    // A link: a note's bubble with a click; any other with Ctrl, or a click while
    // reading.
    if let Some(lien) = e.ed.vue[bi].lien(x, yb) {
        if ctrl || e.reading || lien.starts_with("iris-note:") {
            f.set_notes_ctrl_held(false);
            super::suivre(f, e, &lien);
            return;
        }
    }
    if e.reading {
        return;
    }
    cellules::quitter(f, e);
    let local = e.ed.vue[bi].point(x, yb).unwrap_or(e.ed.vue[bi].marque);
    let global = debut(e, bi) + local;
    // Double and triple clicks.
    let maintenant = Instant::now();
    let n = match e.ed.clic {
        Some((t, px, py, n))
            if maintenant - t < DOUBLE && (px - x).abs() < 5.0 && (py - y).abs() < 5.0 =>
        {
            n % 3 + 1
        }
        _ => 1,
    };
    e.ed.clic = Some((maintenant, x, y, n));
    e.ed.x_voulu = None;
    match n {
        2 => {
            let t = e.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
            let (a, b) = mot_autour(&t, global);
            e.ed.glisse = Some(Glisse::Mots(a, b));
            placer(f, e, a, b);
        }
        3 => {
            let m = marque(e, bi);
            let n = contenu(e, bi).map_or(0, |(_, t)| t.len());
            e.ed.glisse = None;
            placer(f, e, debut(e, bi) + m, debut(e, bi) + n);
        }
        _ => {
            e.ed.glisse = Some(Glisse::Texte);
            let ancre = if shift && e.focus >= 0 {
                e.ed.ancre
            } else {
                global
            };
            placer(f, e, ancre, global);
        }
    }
    clavier(f);
}

/// The offset under a point, wherever it is in the note.
fn sous(e: &Etat, x: f32, y: f32) -> Option<usize> {
    if e.ed.vue.is_empty() {
        return None;
    }
    let bi = e.ed.bloc_a(y.max(0.0));
    let bv = &e.ed.vue[bi];
    let local = bv.point(x, y - e.ed.ys[bi]).unwrap_or(bv.marque);
    Some(debut(e, bi) + local)
}

/// The pointer carried with its button held.
pub(super) fn glisse(f: &AppWindow, e: &mut Etat, x: f32, y: f32) {
    match e.ed.glisse {
        Some(Glisse::Texte) => {
            if let Some(p) = sous(e, x, y) {
                if p != e.ed.tete {
                    let a = e.ed.ancre;
                    placer(f, e, a, p);
                }
            }
        }
        Some(Glisse::Mots(a, b)) => {
            if let Some(p) = sous(e, x, y) {
                let t = e.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
                let (ma, mb) = mot_autour(&t, p);
                if p < a {
                    placer(f, e, b, ma);
                } else {
                    placer(f, e, a, mb.max(b));
                }
            }
        }
        Some(Glisse::Image { bloc, y0 }) => {
            // Where it would go: the gap between the blocks nearest the pointer.
            if (y - y0).abs() < 6.0 {
                e.ed.repere = None;
            } else {
                let j = insertion(e, y);
                let yj = e.ed.ys.get(j).copied().unwrap_or_else(|| hauteur_totale(e));
                e.ed.repere =
                    (j != bloc && j != bloc + 1).then_some((0.0, yj - 1.0, e.ed.largeur, 2.0));
            }
            montrer(f, e);
        }
        Some(Glisse::Taille {
            bloc,
            coin,
            x0,
            largeur,
        }) => {
            if let Some((ox, oy, ow, oh)) = e.ed.vue.get(bloc).and_then(|b| b.objet) {
                let w = taille_tiree(coin, x - x0, largeur, e.ed.largeur);
                let h = oh * w / ow.max(1.0);
                let x = if coin % 2 == 0 { ox + ow - w } else { ox };
                e.ed.repere = Some((x, e.ed.ys[bloc] + oy, w, h));
                montrer(f, e);
            }
        }
        _ => {}
    }
}

/// A picture's width as its corner `coin` is carried by `dx` (the right corners widen
/// it to the right, the left ones to the left), between 32 pixels and the column.
fn taille_tiree(coin: u8, dx: f32, largeur: f32, colonne: f32) -> f32 {
    let signe = if coin % 2 == 1 { 1.0 } else { -1.0 };
    (largeur + signe * dx).clamp(32.0, colonne.max(32.0))
}

/// The note's height.
fn hauteur_totale(e: &Etat) -> f32 {
    e.ed.ys.last().copied().unwrap_or(0.0) + e.ed.vue.last().map_or(0.0, |b| b.hauteur)
}

/// The block a picture dropped at `y` goes before (the number of blocks: after the
/// last).
fn insertion(e: &Etat, y: f32) -> usize {
    for (k, (yk, bv)) in e.ed.ys.iter().zip(&e.ed.vue).enumerate() {
        if y < yk + bv.hauteur / 2.0 {
            return k;
        }
    }
    e.ed.vue.len()
}

/// The pointer let go.
pub(super) fn lache(f: &AppWindow, e: &mut Etat, x: f32, y: f32) {
    let repere = e.ed.repere.take();
    match e.ed.glisse.take() {
        Some(Glisse::Bord { bloc, colonne, x0 }) => {
            let Some(g) = e.ed.vue.get(bloc).and_then(|b| b.grille.clone()) else {
                return;
            };
            let largeur =
                g.largeurs.get(colonne).copied().unwrap_or(100.0) + (x - x0) / g.echelle.max(0.05);
            super::regler_colonne(f, e, bloc, colonne, largeur.max(30.0));
        }
        // A picture dropped elsewhere: its line moves there.
        Some(Glisse::Image { bloc, .. }) if repere.is_some() => {
            let Some(note) = &e.note else { return };
            // The empty line the editor adds after a final line break is no block.
            let n = note.blocks.len()
                - usize::from(
                    note.text.ends_with('\n')
                        && note.blocks.last().is_some_and(|b| b.text.is_empty()),
                );
            let j = insertion(e, y).min(n);
            let Some(ed) = edit::move_block_to(&note.blocks[..n], bloc, j) else {
                montrer(f, e);
                return;
            };
            retenir(e, true);
            appliquer(f, e, ed, true, None);
        }
        // A picture resized: the width written in its embed (`![[a.png|320]]`).
        Some(Glisse::Taille {
            bloc,
            coin,
            x0,
            largeur,
        }) => {
            let w = taille_tiree(coin, x - x0, largeur, e.ed.largeur).round();
            if (w - largeur).abs() >= 1.0 {
                super::largeur_image(f, e, bloc, w);
            } else {
                montrer(f, e);
            }
        }
        _ => {
            if repere.is_some() {
                montrer(f, e);
            }
        }
    }
}

/// The pointer over the note: its shape says what a click does.
pub(super) fn survol(f: &AppWindow, e: &Etat, x: f32, y: f32) {
    if e.ed.vue.is_empty() {
        return;
    }
    let bi = e.ed.bloc_a(y.max(0.0));
    let yb = y - e.ed.ys[bi];
    let bv = &e.ed.vue[bi];
    let forme = if let Some(c) = bv.cible(x, yb) {
        c.curseur
    } else if bv
        .lien(x, yb)
        .is_some_and(|l| e.reading || f.get_notes_ctrl_held() || l.starts_with("iris-note:"))
    {
        1
    } else if e.reading {
        3
    } else {
        0
    };
    if f.get_note_canvas_cursor() != forme {
        f.set_note_canvas_cursor(forme);
    }
}

/// The window's calls for the editor.
pub(super) fn wire(f: &AppWindow, etat: &Rc<RefCell<Etat>>) {
    f.set_note_view(ModelRc::from(Rc::clone(&etat.borrow().ed.modele)));
    macro_rules! geste {
        ($installer:ident, |$f:ident, $e:ident $(, $arg:ident)*| $corps:block) => {{
            let (etat, faible) = (Rc::clone(etat), f.as_weak());
            f.$installer(move |$($arg),*| {
                let Some($f) = faible.upgrade() else { return };
                let Ok(mut guard) = etat.try_borrow_mut() else { return };
                let $e: &mut Etat = &mut guard;
                $corps
            });
        }};
    }
    geste!(on_note_canvas_width, |f, e, largeur| {
        if (largeur - e.ed.largeur).abs() > 0.5 {
            e.ed.largeur = largeur;
            vue(&f, e, true);
            montrer(&f, e);
        }
    });
    geste!(on_note_canvas_pressed, |f, e, x, y, shift, ctrl| {
        presse(&f, e, x, y, shift, ctrl);
    });
    geste!(on_note_canvas_dragged, |f, e, x, y| {
        glisse(&f, e, x, y);
    });
    geste!(on_note_canvas_released, |f, e, x, y| {
        lache(&f, e, x, y);
    });
    geste!(on_note_canvas_hovered, |f, e, x, y| {
        survol(&f, e, x, y);
    });
    {
        let (etat, faible) = (Rc::clone(etat), f.as_weak());
        f.on_note_canvas_key(move |nom| {
            let Some(f) = faible.upgrade() else {
                return false;
            };
            let Ok(mut e) = etat.try_borrow_mut() else {
                return false;
            };
            touche_editeur(&f, &mut e, &nom)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_found_around_and_beside() {
        let t = "un mot, deux";
        assert_eq!(mot_autour(t, 4), (3, 6));
        assert_eq!(mot_voisin(t, 12, true), 8);
        assert_eq!(mot_voisin(t, 3, false), 6);
        assert_eq!(voisin("é", 0, false), 2);
        assert_eq!(voisin("aé", 3, true), 1);
    }

    #[test]
    fn blocks_are_unchanged_by_the_editor_s_helpers() {
        // The editor's whole-note replacement keeps the rest of the text.
        let t = "a\nb\nc";
        let blocs: Vec<iris_notes::block::Block> = block::parse(t);
        assert_eq!(block::locate(&blocs, 2), (1, 0));
    }
}
