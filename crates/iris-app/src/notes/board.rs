//! A whiteboard open in Notes: its items given to the canvas, every press, drag and
//! key turned into a change of the board, the toolbar's choices, the text typed in an
//! item, copy and paste, undo, and the file written half a second after the last
//! change.
//!
//! The board is held here whole, as `iris_board` reads it; the canvas only draws. The
//! view (zoom and offset) lives in the window, which says where it is (`viewport`), so
//! that what is under the pointer and how close is "close" are measured on screen.

use super::{stem, Etat};
use iris_board::geom::{self, Pt, Rect};
use iris_board::snap::{self, Lines};
use iris_board::{route, Align, Board, Colour, End, Font, Head, Item, Kind, Shape, VAlign, Way};
use iris_board::{COLOURS, SIDES};
use iris_ui::Board as Vue;
use iris_ui::{AppWindow, BoardGuideData, BoardHandleData, BoardItemData};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::ops::Add;
use std::rc::Rc;

/// How many steps of undo a whiteboard keeps.
const ANNULATIONS: usize = 100;
/// The text sizes the toolbar's − and + go through.
const TAILLES: [f32; 16] = [
    8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 40.0, 48.0, 64.0, 80.0, 96.0, 128.0,
];
/// How far a press may move before it is a drag (pixels).
const SEUIL: f32 = 3.0;

thread_local! {
    /// What Copy took, for Paste on any board.
    static PRESSE: RefCell<Vec<Item>> = const { RefCell::new(Vec::new()) };
}

/// What the tools palette offers, as the window numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outil {
    Choisir,
    Main,
    Note,
    Forme,
    Texte,
    Fleche,
    Stylo,
    Cadre,
}

impl Outil {
    const TOUS: [Outil; 8] = [
        Outil::Choisir,
        Outil::Main,
        Outil::Note,
        Outil::Forme,
        Outil::Texte,
        Outil::Fleche,
        Outil::Stylo,
        Outil::Cadre,
    ];

    fn index(self) -> i32 {
        Outil::TOUS.iter().position(|o| *o == self).unwrap_or(0) as i32
    }
}

/// What a handle does when it is pulled.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Role {
    /// One of the eight around a box: 0 top left, clockwise.
    Redim(usize),
    /// The "+" beside a side (0 top, clockwise): an arrow starts there.
    Plus(usize),
    /// An arrow's end: its last (`true`) or its first.
    Bout(bool),
    /// An arrow's point.
    Coin(usize),
    /// The middle of an arrow's piece.
    Milieu(usize),
    /// Shown only: a point an end can tie to.
    Repere,
    /// Shown only: where the end being dragged ties.
    Cible,
}

impl Role {
    fn kind(self) -> i32 {
        match self {
            Role::Redim(_) => 0,
            Role::Repere => 2,
            Role::Bout(_) => 3,
            Role::Coin(_) => 5,
            Role::Milieu(_) => 6,
            Role::Cible => 7,
            Role::Plus(s) => 10 + s as i32,
        }
    }
}

/// The eight handles around a box, as fractions, clockwise from the top left.
const POIGNEES: [(f32, f32); 8] = [
    (0.0, 0.0),
    (0.5, 0.0),
    (1.0, 0.0),
    (1.0, 0.5),
    (1.0, 1.0),
    (0.5, 1.0),
    (0.0, 1.0),
    (0.0, 0.5),
];

/// A drag under way.
#[derive(Debug, Clone)]
enum Geste {
    /// Items carried: from where the press was, by how much so far, and the box around
    /// them when the press came (what is aligned to the others).
    Deplacer {
        ids: Vec<u64>,
        depart: Pt,
        fait: Pt,
        boite: Rect,
    },
    Redim {
        id: u64,
        poignee: usize,
        avant: Box<Item>,
    },
    /// A box drawn to choose what it covers; `avant` was chosen before (Shift).
    Cadre {
        depart: Pt,
        avant: Vec<u64>,
    },
    /// An arrow's end pulled; `cote` when it started from an item's "+".
    Bout {
        id: u64,
        fin: bool,
        cote: Option<(u64, usize)>,
    },
    Coin {
        id: u64,
        k: usize,
    },
    /// An elbow's piece moved across: its two corners, where they were.
    Segment {
        id: u64,
        k: usize,
        depart: Pt,
        origine: Vec<Pt>,
    },
    Trait {
        id: u64,
    },
    /// A new box drawn out from where the press was.
    Creer {
        id: u64,
        depart: Pt,
    },
}

/// The whiteboard open.
pub(super) struct Tableau {
    pub rel: String,
    pub board: Board,
    choix: Vec<u64>,
    outil: Outil,
    forme: Shape,
    geste: Option<Geste>,
    /// The press moved past the threshold.
    bouge: bool,
    /// Where the last press was, and whether its item was already the only one chosen;
    /// whether it was one of several chosen (a click without a drag keeps it alone).
    appui: Pt,
    deja: bool,
    reduire: bool,
    /// Where the pointer is while a drag is under way.
    pointeur: Pt,
    /// How many steps of undo there were when the press came.
    pile: usize,
    /// The item an arrow's end would tie to.
    survol: Option<u64>,
    /// The view: its zoom, and what shows (board units).
    zoom: f32,
    vue: Rect,
    undo: Vec<Board>,
    redo: Vec<Board>,
    modified: Option<i64>,
    dirty: bool,
    /// The item whose words are being typed.
    edition: Option<u64>,
    curseur: i32,
    /// How far apart the ground's dots are on screen (whole pixels): the tile shown.
    grille: u32,
    items: Rc<VecModel<BoardItemData>>,
    poignees: Vec<BoardHandleData>,
    /// The alignment guides shown while something moves.
    guides: Vec<snap::Guide>,
}

/// The ground's dots are 20 units apart, 100, 500… whichever comes first that leaves at
/// least 12 pixels between them; what that is on screen, rounded to a whole pixel.
fn ecart_points(zoom: f32) -> u32 {
    let mut pas = 20.0 * zoom.max(0.0001);
    let mut tours = 0;
    while pas < 12.0 && tours < 12 {
        pas *= 5.0;
        tours += 1;
    }
    (pas.round() as u32).clamp(12, 200)
}

/// A tile `s` pixels square with one dot in its middle: white, its edge smoothed, for
/// the window to tint. Repeated, it makes the ground's dots.
fn tuile(s: u32) -> slint::Image {
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(s, s);
    let c = s as f32 / 2.0;
    for (i, px) in buf.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = ((i as u32 % s) as f32 + 0.5, (i as u32 / s) as f32 + 0.5);
        let d = ((x - c).powi(2) + (y - c).powi(2)).sqrt();
        let a = (1.6 - d).clamp(0.0, 1.0);
        *px = slint::Rgba8Pixel {
            r: 255,
            g: 255,
            b: 255,
            a: (a * 255.0).round() as u8,
        };
    }
    slint::Image::from_rgba8(buf)
}

// --- Colours and items as the canvas draws them ----------------------------------------

/// A colour of the file, as the canvas takes it: its place in the palette (−1 none, −2
/// the custom one) and the custom colour.
fn couleur(s: &str) -> (i32, slint::Color) {
    match Colour::parse(s) {
        Colour::None => (-1, slint::Color::default()),
        Colour::Named(i) => (i as i32, slint::Color::default()),
        Colour::Rgb(v) => (
            -2,
            slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8),
        ),
    }
}

/// The room between a box's edge and its words (board units), as the canvas has it.
fn marge(kind: Kind) -> f32 {
    match kind {
        Kind::Sticky => 12.0,
        Kind::Text => 2.0,
        _ => 8.0,
    }
}

fn donnee(b: &Board, it: &Item, edition: bool, marque: i32) -> BoardItemData {
    let (fill, fill_custom) = couleur(&it.fill);
    let (stroke, stroke_custom) = couleur(&it.stroke);
    let (text_colour, text_custom) = couleur(&it.color);
    let mut d = BoardItemData {
        id: it.id as i32,
        kind: it.kind.index() as i32,
        x: it.x,
        y: it.y,
        width: it.w,
        height: it.h,
        fill,
        fill_custom,
        opacity: it.opacity,
        stroke,
        stroke_custom,
        stroke_width: it.width,
        text: it.text.as_str().into(),
        text_colour,
        text_custom,
        font: it.font.index() as i32,
        font_size: it.size,
        bold: it.bold,
        italic: it.italic,
        align: it.align.index() as i32,
        valign: it.valign.resolved(it.kind) as i32,
        marked: marque,
        editing: edition,
        ..Default::default()
    };
    match it.kind {
        Kind::Shape => {
            d.path = geom::shape_commands(it.shape, it.w, it.h).into();
            d.radius = geom::radius(it.shape, it.w, it.h);
        }
        Kind::Frame => d.radius = 4.0,
        Kind::Sticky => d.radius = 2.0,
        Kind::Text => d.radius = 2.0,
        Kind::Connector => {
            let dr = route::drawn(b, it);
            let r = dr.bounds.inflate(1.0);
            let o = Pt::new(r.x, r.y);
            d.x = r.x;
            d.y = r.y;
            d.width = r.w;
            d.height = r.h;
            d.path = dr.line.commands(o).into();
            d.heads = route::polygons(&dr.heads, o).into();
            d.label_x = dr.label.x;
            d.label_y = dr.label.y;
        }
        Kind::Ink => {
            let r = Rect::around(&it.points).unwrap_or_default().inflate(1.0);
            let o = Pt::new(r.x, r.y);
            d.x = r.x;
            d.y = r.y;
            d.width = r.w;
            d.height = r.h;
            d.path = geom::ink_commands(&it.points, o).into();
        }
    }
    d
}

impl Tableau {
    fn nouveau(rel: &str, board: Board, modified: Option<i64>) -> Tableau {
        Tableau {
            rel: rel.to_string(),
            board,
            choix: Vec::new(),
            outil: Outil::Choisir,
            forme: Shape::Rounded,
            geste: None,
            bouge: false,
            appui: Pt::default(),
            deja: false,
            reduire: false,
            pointeur: Pt::default(),
            pile: 0,
            survol: None,
            zoom: 1.0,
            vue: Rect::new(0.0, 0.0, 800.0, 600.0),
            undo: Vec::new(),
            redo: Vec::new(),
            modified,
            dirty: false,
            edition: None,
            curseur: 0,
            grille: 0,
            items: Rc::new(VecModel::default()),
            poignees: Vec::new(),
            guides: Vec::new(),
        }
    }

    /// How close is close: a few pixels on screen, in board units.
    fn tol(&self, px: f32) -> f32 {
        px / self.zoom.max(0.001)
    }

    /// The board kept for Undo, before a change.
    fn retenir(&mut self) {
        self.undo.push(self.board.clone());
        if self.undo.len() > ANNULATIONS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The last step kept dropped when nothing changed after all.
    fn oublier_si_pareil(&mut self) {
        if self.undo.last() == Some(&self.board) {
            self.undo.pop();
        }
    }

    fn un_seul(&self) -> Option<&Item> {
        match self.choix.as_slice() {
            [id] => self.board.get(*id),
            _ => None,
        }
    }

    /// The handles shown, each with what pulling it does.
    fn reperes(&self) -> Vec<(Pt, Role)> {
        let mut v = Vec::new();
        if self.edition.is_some() {
            return v;
        }
        // The item an arrow's end would tie to: its sides' middles.
        let tirage = matches!(self.geste, Some(Geste::Bout { .. }));
        if tirage || (self.outil == Outil::Fleche && self.geste.is_none()) {
            if let Some(it) = self.survol.and_then(|id| self.board.get(id)) {
                if it.kind == Kind::Connector {
                    // Another arrow: its middle and the points it goes through.
                    let c = route::curve(&self.board, it);
                    for t in route::anchors(&self.board, it) {
                        v.push((c.at(t), Role::Repere));
                    }
                } else {
                    for (u, w) in SIDES {
                        v.push((it.at(u, w), Role::Repere));
                    }
                }
            }
        }
        if let Some(Geste::Bout { id, fin, .. }) = &self.geste {
            if let Some(it) = self.board.get(*id) {
                let e = if *fin { it.to } else { it.from };
                if let Some(e) = e.filter(|e| e.item.is_some()) {
                    v.push((self.board.end_point(&e), Role::Cible));
                }
            }
            return v;
        }
        if self.geste.is_some() && self.bouge {
            return v;
        }
        let Some(it) = self.un_seul() else {
            return v;
        };
        match it.kind {
            Kind::Connector => {
                for e in [it.from, it.to].into_iter().flatten() {
                    if let Some(o) = e.item.and_then(|i| self.board.get(i)) {
                        for (u, w) in SIDES {
                            v.push((o.at(u, w), Role::Repere));
                        }
                    }
                }
                let pts = route::through(&self.board, it);
                for (k, m) in route::midpoints(&self.board, it).into_iter().enumerate() {
                    v.push((m, Role::Milieu(k)));
                }
                let coins: Vec<Pt> = match it.way {
                    Way::Elbow => pts[1..pts.len().saturating_sub(1).max(1)].to_vec(),
                    _ => it.points.clone(),
                };
                for (k, p) in coins.into_iter().enumerate() {
                    v.push((p, Role::Coin(k)));
                }
                if let (Some(a), Some(b)) = (it.from, it.to) {
                    v.push((self.board.end_point(&a), Role::Bout(false)));
                    v.push((self.board.end_point(&b), Role::Bout(true)));
                }
            }
            Kind::Ink => {}
            _ if it.locked => {}
            _ => {
                for (i, (u, w)) in POIGNEES.iter().enumerate() {
                    v.push((it.at(*u, *w), Role::Redim(i)));
                }
                if it.kind != Kind::Frame {
                    for (s, (u, w)) in SIDES.iter().enumerate() {
                        v.push((it.at(*u, *w), Role::Plus(s)));
                    }
                }
            }
        }
        v
    }

    /// The handle under `p`, the "+" ones set out of their side as the canvas has them.
    fn repere_sous(&self, p: Pt) -> Option<Role> {
        let tol = self.tol(9.0);
        let mut meilleur: Option<(f32, Role)> = None;
        for (q, role) in self.reperes() {
            if matches!(role, Role::Repere | Role::Cible) {
                continue;
            }
            let q = match role {
                Role::Plus(0) => Pt::new(q.x, q.y - self.tol(18.0)),
                Role::Plus(1) => Pt::new(q.x + self.tol(18.0), q.y),
                Role::Plus(2) => Pt::new(q.x, q.y + self.tol(18.0)),
                Role::Plus(_) => Pt::new(q.x - self.tol(18.0), q.y),
                _ => q,
            };
            let d = q.distance(p);
            // A corner beats the middle it sits on; an end beats both.
            let rang = match role {
                Role::Bout(_) => 0.0,
                Role::Coin(_) => 0.1,
                _ => 0.2,
            };
            if d <= tol && meilleur.is_none_or(|(m, _)| d + rang < m) {
                meilleur = Some((d + rang, role));
            }
        }
        meilleur.map(|(_, r)| r)
    }
}

// --- What the window shows -------------------------------------------------------------

fn montrer(f: &AppWindow, t: &mut Tableau) {
    let g = f.global::<Vue>();
    t.choix.retain(|id| t.board.get(*id).is_some());
    // What is chosen, marked one by one when there are several; what a chosen frame
    // carries along, marked fainter.
    let plusieurs = t.choix.len() > 1;
    let mut portes: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for id in &t.choix {
        if t.board.get(*id).is_some_and(|i| i.kind == Kind::Frame) {
            for h in t.board.held_by(*id) {
                if !t.choix.contains(&h) {
                    portes.insert(h);
                }
            }
        }
    }
    let marque = |id: u64| -> i32 {
        if plusieurs && t.choix.contains(&id) {
            1
        } else if portes.contains(&id) {
            2
        } else {
            0
        }
    };
    let neufs: Vec<BoardItemData> = t
        .board
        .items
        .iter()
        .map(|it| donnee(&t.board, it, t.edition == Some(it.id), marque(it.id)))
        .collect();
    g.set_sel_count(t.choix.len() as i32);
    g.set_sel_carried(portes.len() as i32);
    // The alignment guides.
    let guides: Vec<BoardGuideData> = t
        .guides
        .iter()
        .map(|gd| BoardGuideData {
            vertical: gd.vertical,
            at: gd.at,
            from: gd.from,
            to: gd.to,
        })
        .collect();
    if !guides.is_empty() || g.get_guides().row_count() > 0 {
        g.set_guides(ModelRc::new(VecModel::from(guides)));
    }
    if t.items.row_count() == neufs.len() {
        for (i, d) in neufs.into_iter().enumerate() {
            if t.items.row_data(i).as_ref() != Some(&d) {
                t.items.set_row_data(i, d);
            }
        }
    } else {
        t.items.set_vec(neufs);
    }
    montrer_poignees(f, t);
    // What is chosen, and its look for the toolbar.
    let boites: Vec<Rect> = t
        .choix
        .iter()
        .filter_map(|id| t.board.get(*id))
        .map(|it| t.board.bbox(it))
        .collect();
    let sel = boites.iter().copied().reduce(|a, b| a.union(b));
    let kinds: Vec<Kind> = t
        .choix
        .iter()
        .filter_map(|id| t.board.get(*id).map(|i| i.kind))
        .collect();
    let sel_kind = match kinds.as_slice() {
        [] => -1,
        [k, rest @ ..] if rest.iter().all(|r| r == k) => k.index() as i32,
        _ => 6,
    };
    g.set_sel_kind(sel_kind);
    if let Some(r) = sel {
        g.set_sel_x(r.x);
        g.set_sel_y(r.y);
        g.set_sel_width(r.w);
        g.set_sel_height(r.h);
    }
    g.set_sel_box(sel_kind != 4 && sel_kind >= 0);
    if let Some(it) = t.choix.first().and_then(|id| t.board.get(*id)) {
        let (fill, fc) = couleur(&it.fill);
        let (stroke, sc) = couleur(&it.stroke);
        let (texte, tc) = couleur(&it.color);
        g.set_fill(fill);
        g.set_fill_custom(fc);
        g.set_stroke(stroke);
        g.set_stroke_custom(sc);
        g.set_text_colour(texte);
        g.set_text_custom(tc);
        g.set_opacity((it.opacity * 100.0).round() as i32);
        g.set_stroke_width(it.width);
        g.set_font(it.font.index() as i32);
        g.set_font_size(it.size.round() as i32);
        g.set_bold(it.bold);
        g.set_italic(it.italic);
        g.set_align(it.align.index() as i32);
        g.set_valign(it.valign.resolved(it.kind) as i32);
        g.set_shape(it.shape.index() as i32);
        g.set_way(it.way.index() as i32);
        g.set_rounded(it.rounded);
        g.set_head_start(it.start.index() as i32);
        g.set_head_end(it.end.index() as i32);
        g.set_locked(it.locked);
    }
    let b = t.board.bounds().unwrap_or_default();
    g.set_bounds_x(b.x);
    g.set_bounds_y(b.y);
    g.set_bounds_width(b.w);
    g.set_bounds_height(b.h);
    g.set_tool(t.outil.index());
    g.set_shape_tool(t.forme.index() as i32);
    g.set_can_undo(!t.undo.is_empty());
    g.set_can_redo(!t.redo.is_empty());
    g.set_busy(t.geste.is_some() && t.bouge);
    match &t.geste {
        Some(Geste::Cadre { depart, .. }) if t.bouge => {
            let r = Rect::spanning(*depart, t.pointeur);
            g.set_marquee(true);
            g.set_marquee_x(r.x);
            g.set_marquee_y(r.y);
            g.set_marquee_width(r.w);
            g.set_marquee_height(r.h);
        }
        _ => g.set_marquee(false),
    }
    montrer_edition(f, t);
}

fn montrer_poignees(f: &AppWindow, t: &mut Tableau) {
    let v: Vec<BoardHandleData> = t
        .reperes()
        .into_iter()
        .map(|(p, r)| BoardHandleData {
            x: p.x,
            y: p.y,
            kind: r.kind(),
        })
        .collect();
    if v != t.poignees {
        f.global::<Vue>()
            .set_handles(ModelRc::new(VecModel::from(v.clone())));
        t.poignees = v;
    }
}

/// Where an item's words are typed (board units): its box less its margin, a frame's
/// title above it, an arrow's words halfway along.
fn boite_texte(b: &Board, it: &Item) -> Rect {
    match it.kind {
        Kind::Frame => Rect::new(it.x, it.y - 24.0, it.w.max(160.0), 20.0),
        Kind::Connector => {
            let m = route::curve(b, it).at(route::label_at(it));
            let w = 160.0;
            let h = it.size * 1.6;
            Rect::new(m.x - w / 2.0, m.y - h / 2.0, w, h)
        }
        _ => it.rect().inflate(-marge(it.kind)),
    }
}

fn montrer_edition(f: &AppWindow, t: &Tableau) {
    let g = f.global::<Vue>();
    let Some(it) = t.edition.and_then(|id| t.board.get(id)) else {
        g.set_editing(false);
        return;
    };
    let r = boite_texte(&t.board, it);
    g.set_edit_x(r.x);
    g.set_edit_y(r.y);
    g.set_edit_width(r.w);
    g.set_edit_height(r.h);
    g.set_edit_font(it.font.index() as i32);
    g.set_edit_size(if it.kind == Kind::Frame {
        12.0
    } else {
        it.size
    });
    g.set_edit_bold(it.bold || it.kind == Kind::Frame);
    g.set_edit_italic(it.italic);
    g.set_edit_align(match it.kind {
        Kind::Frame => 0,
        Kind::Connector => 1,
        _ => it.align.index() as i32,
    });
    g.set_edit_valign(match it.kind {
        Kind::Frame | Kind::Connector => 1,
        _ => it.valign.resolved(it.kind) as i32,
    });
    let (c, cc) = couleur(&it.color);
    g.set_edit_colour(c);
    g.set_edit_custom(cc);
    g.set_editing(true);
}

// --- Changes ----------------------------------------------------------------------------

fn change(f: &AppWindow, t: &mut Tableau) {
    t.dirty = true;
    f.set_note_status(SharedString::default());
    montrer(f, t);
    super::planifier_ecriture(f);
}

/// The words of `id` opened for typing.
fn editer(f: &AppWindow, t: &mut Tableau, id: u64) {
    let Some(it) = t.board.get(id) else { return };
    if it.kind == Kind::Ink {
        return;
    }
    let texte = it.text.clone();
    t.retenir();
    t.edition = Some(id);
    t.choix = vec![id];
    let g = f.global::<Vue>();
    g.set_edit_text(texte.into());
    g.set_edit_serial(g.get_edit_serial() + 1);
    montrer(f, t);
}

/// Typing done: an empty text goes, the rest is kept.
fn finir_edition(f: &AppWindow, t: &mut Tableau) {
    let Some(id) = t.edition.take() else { return };
    if t.board
        .get(id)
        .is_some_and(|i| i.kind == Kind::Text && i.text.trim().is_empty())
    {
        t.board.remove(&[id]);
        t.choix.retain(|c| *c != id);
    }
    t.oublier_si_pareil();
    f.global::<Vue>().set_editing(false);
    change(f, t);
}

/// A new item of the tool's kind with its box `r`.
fn creer(t: &mut Tableau, r: Rect) -> u64 {
    let id = t.board.take_id();
    let it = match t.outil {
        Outil::Note => Item::sticky(id, r),
        Outil::Texte => Item::text(id, r),
        Outil::Cadre => Item::frame(id, r),
        _ => {
            let mut it = Item::shape(id, t.forme, r);
            if t.forme == Shape::Ellipse || t.forme == Shape::Diamond {
                it.fill = "yellow-soft".into();
                it.stroke = "yellow".into();
            }
            it
        }
    };
    if it.kind == Kind::Frame {
        t.board.items.insert(0, it);
    } else {
        t.board.items.push(it);
    }
    id
}

/// The size a box takes when the tool is only clicked.
fn taille_usuelle(outil: Outil, forme: Shape) -> (f32, f32) {
    match outil {
        Outil::Note => (160.0, 160.0),
        Outil::Texte => (200.0, 30.0),
        Outil::Cadre => (480.0, 320.0),
        _ => match forme {
            Shape::Ellipse | Shape::Star | Shape::Hexagon => (130.0, 90.0),
            Shape::Diamond | Shape::Triangle => (140.0, 100.0),
            _ => (150.0, 72.0),
        },
    }
}

/// Click on an item's "+": a copy of it beyond that side, tied to it by an arrow.
fn prolonger(t: &mut Tableau, source: u64, cote: usize, fleche: u64) -> Option<u64> {
    let it = t.board.get(source)?.clone();
    let ecart = 80.0;
    let (dx, dy) = match cote {
        0 => (0.0, -(it.h + ecart)),
        1 => (it.w + ecart, 0.0),
        2 => (0.0, it.h + ecart),
        _ => (-(it.w + ecart), 0.0),
    };
    let id = t.board.take_id();
    let mut copie = it.clone();
    copie.id = id;
    copie.x += dx;
    copie.y += dy;
    copie.text.clear();
    copie.locked = false;
    t.board.items.push(copie);
    let (u, v) = SIDES[(cote + 2) % 4];
    if let Some(a) = t.board.get_mut(fleche) {
        a.to = Some(End::on(id, u, v));
    }
    Some(id)
}

/// The arrow `id`'s corners made its own points (an elbow's way found until now).
fn fixer_coins(t: &mut Tableau, id: u64) {
    let Some(it) = t.board.get(id) else { return };
    if it.way != Way::Elbow || !it.points.is_empty() {
        return;
    }
    let pts = route::through(&t.board, it);
    let interieur = if pts.len() > 2 {
        pts[1..pts.len() - 1].to_vec()
    } else {
        Vec::new()
    };
    if let Some(it) = t.board.get_mut(id) {
        it.points = interieur;
    }
}

fn presser(f: &AppWindow, t: &mut Tableau, p: Pt, shift: bool, droit: bool) {
    t.appui = p;
    t.pointeur = p;
    t.bouge = false;
    t.deja = false;
    t.reduire = false;
    t.geste = None;
    if t.edition.is_some() {
        finir_edition(f, t);
    }
    t.pile = t.undo.len();
    // A right press only chooses what it is on, for the menu.
    if droit {
        match t.board.hit(p, t.tol(6.0)) {
            Some(id) if !t.choix.contains(&id) => t.choix = vec![id],
            Some(_) => {}
            None => t.choix.clear(),
        }
        montrer(f, t);
        return;
    }
    match t.outil {
        Outil::Main => {}
        Outil::Choisir => {
            if let Some(role) = t.repere_sous(p) {
                let Some(id) = t.choix.first().copied() else {
                    return;
                };
                t.retenir();
                t.geste = match role {
                    Role::Redim(i) => t.board.get(id).cloned().map(|avant| Geste::Redim {
                        id,
                        poignee: i,
                        avant: Box::new(avant),
                    }),
                    Role::Plus(s) => {
                        let fid = t.board.take_id();
                        let (u, v) = SIDES[s];
                        let mut a = Item::connector(fid, End::on(id, u, v), End::free(p));
                        a.way = Way::Elbow;
                        a.rounded = true;
                        t.board.items.push(a);
                        t.choix = vec![fid];
                        Some(Geste::Bout {
                            id: fid,
                            fin: true,
                            cote: Some((id, s)),
                        })
                    }
                    Role::Bout(fin) => Some(Geste::Bout {
                        id,
                        fin,
                        cote: None,
                    }),
                    Role::Coin(k) => {
                        fixer_coins(t, id);
                        Some(Geste::Coin { id, k })
                    }
                    Role::Milieu(k) => commencer_segment(t, id, k, p),
                    Role::Repere | Role::Cible => None,
                };
                montrer(f, t);
                return;
            }
            match t.board.hit(p, t.tol(6.0)) {
                Some(id) => {
                    t.deja = t.choix == [id];
                    t.reduire = !shift && t.choix.len() > 1 && t.choix.contains(&id);
                    if shift {
                        if let Some(i) = t.choix.iter().position(|c| *c == id) {
                            t.choix.remove(i);
                        } else {
                            t.choix.push(id);
                        }
                    } else if !t.choix.contains(&id) {
                        t.choix = vec![id];
                    }
                    let mut ids: Vec<u64> = t
                        .choix
                        .iter()
                        .copied()
                        .filter(|i| t.board.get(*i).is_some_and(|it| !it.locked))
                        .collect();
                    // A frame carries what it holds.
                    for c in ids.clone() {
                        if t.board.get(c).is_some_and(|i| i.kind == Kind::Frame) {
                            for h in t.board.held_by(c) {
                                if !ids.contains(&h) {
                                    ids.push(h);
                                }
                            }
                        }
                    }
                    if !ids.is_empty() {
                        t.retenir();
                        let boite = ids
                            .iter()
                            .filter_map(|i| t.board.get(*i))
                            .filter(|i| i.kind != Kind::Connector)
                            .map(|i| t.board.bbox(i))
                            .reduce(|a, b| a.union(b))
                            .unwrap_or_default();
                        t.geste = Some(Geste::Deplacer {
                            ids,
                            depart: p,
                            fait: Pt::default(),
                            boite,
                        });
                    }
                }
                None => {
                    let avant = if shift { t.choix.clone() } else { Vec::new() };
                    t.choix = avant.clone();
                    t.geste = Some(Geste::Cadre { depart: p, avant });
                }
            }
        }
        Outil::Note | Outil::Forme | Outil::Texte | Outil::Cadre => {
            t.retenir();
            let id = creer(t, Rect::new(p.x, p.y, 0.0, 0.0));
            t.choix = vec![id];
            t.geste = Some(Geste::Creer { id, depart: p });
        }
        Outil::Fleche => {
            t.retenir();
            let id = t.board.take_id();
            let depart = t.board.attach(p, t.tol(14.0), &[]);
            t.board
                .items
                .push(Item::connector(id, depart, End::free(p)));
            t.choix = vec![id];
            t.geste = Some(Geste::Bout {
                id,
                fin: true,
                cote: None,
            });
        }
        Outil::Stylo => {
            t.retenir();
            let id = t.board.take_id();
            t.board.items.push(Item::ink(id, vec![p]));
            t.choix.clear();
            t.geste = Some(Geste::Trait { id });
        }
    }
    montrer(f, t);
}

/// An arrow's piece taken by its middle: an elbow's piece moves across; a straight or
/// curved arrow gets a point there, which is then carried.
fn commencer_segment(t: &mut Tableau, id: u64, k: usize, p: Pt) -> Option<Geste> {
    let it = t.board.get(id)?;
    if it.way != Way::Elbow {
        let it = t.board.get_mut(id)?;
        let k = k.min(it.points.len());
        it.points.insert(k, p);
        return Some(Geste::Coin { id, k });
    }
    let (a, na, b, nb) = route::ends(&t.board, it);
    let full = route::through(&t.board, it);
    let n = full.len();
    if k + 1 >= n {
        return None;
    }
    let mut interieur: Vec<Pt> = full[1..n - 1].to_vec();
    let mut k = k;
    // A piece out of an end: a corner is added just out of the item, so the piece can
    // move without the end.
    if k + 2 == n {
        let t_pt = nb.map_or(b.lerp(full[n - 2], 0.3), |d| b.add(d.scale(route::STUB)));
        interieur.push(t_pt);
    }
    if k == 0 {
        let s = na.map_or(a.lerp(full[1], 0.3), |d| a.add(d.scale(route::STUB)));
        interieur.insert(0, s);
        k = 1;
    }
    let it = t.board.get_mut(id)?;
    it.points = interieur.clone();
    Some(Geste::Segment {
        id,
        k,
        depart: p,
        origine: interieur,
    })
}

fn tirer(f: &AppWindow, t: &mut Tableau, p: Pt, shift: bool, ctrl: bool) {
    if t.geste.is_none() {
        return;
    }
    if !t.bouge && p.distance(t.appui) * t.zoom < SEUIL {
        return;
    }
    t.bouge = true;
    t.pointeur = p;
    let accroche = t.tol(14.0);
    // What is aligned to: the boxes in the view and a little around it. Ctrl moves
    // freely.
    let pres = t.vue.inflate(t.vue.w.max(t.vue.h) * 0.25);
    let aimant = t.tol(6.0);
    t.guides.clear();
    match t.geste.clone() {
        Some(Geste::Deplacer {
            ids,
            depart,
            fait,
            boite,
        }) => {
            let (mut dx, mut dy) = (p.x - depart.x, p.y - depart.y);
            if !ctrl && boite.w + boite.h > 0.0 {
                let r = Rect::new(boite.x + dx, boite.y + dy, boite.w, boite.h);
                let (sx, sy, g) =
                    snap::align(&t.board, &ids, r, Lines::ALL, Lines::ALL, aimant, pres);
                dx += sx;
                dy += sy;
                t.guides = g;
            }
            t.board.translate(&ids, dx - fait.x, dy - fait.y);
            t.geste = Some(Geste::Deplacer {
                ids,
                depart,
                fait: Pt::new(dx, dy),
                boite,
            });
        }
        Some(Geste::Redim { id, poignee, avant }) => {
            let (u, v) = POIGNEES[poignee];
            let r = avant.rect();
            let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.right(), r.bottom());
            if u == 0.0 {
                x0 = p.x;
            } else if u == 1.0 {
                x1 = p.x;
            }
            if v == 0.0 {
                y0 = p.y;
            } else if v == 1.0 {
                y1 = p.y;
            }
            let mut n = Rect::spanning(Pt::new(x0, y0), Pt::new(x1, y1));
            // Shift on a corner keeps the proportions.
            if shift && u != 0.5 && v != 0.5 && r.w > 0.0 && r.h > 0.0 {
                let k = (n.w / r.w).max(n.h / r.h);
                let (w, h) = (r.w * k, r.h * k);
                n.x = if u == 0.0 { r.right() - w } else { r.x };
                n.y = if v == 0.0 { r.bottom() - h } else { r.y };
                n.w = w;
                n.h = h;
            } else if !ctrl {
                // The edge pulled meets the edges and middles around it (pulled past the
                // other edge, the box turned over and the pulled edge is the other side).
                let bord = |f: f32, debut: f32, fin: f32| {
                    if f == 0.0 {
                        if debut <= fin {
                            Lines::START
                        } else {
                            Lines::END
                        }
                    } else if f == 1.0 {
                        if fin >= debut {
                            Lines::END
                        } else {
                            Lines::START
                        }
                    } else {
                        Lines::NONE
                    }
                };
                let across = bord(u, x0, x1);
                let down = bord(v, y0, y1);
                (n, t.guides) = resized(&t.board, id, n, across, down, aimant, pres);
            }
            n.w = n.w.max(8.0);
            n.h = n.h.max(8.0);
            if let Some(it) = t.board.get_mut(id) {
                it.set_rect(n);
            }
        }
        Some(Geste::Cadre { depart, avant }) => {
            let r = Rect::spanning(depart, p);
            let mut choix = avant;
            for id in t.board.inside(r) {
                if !choix.contains(&id) {
                    choix.push(id);
                }
            }
            t.choix = choix;
        }
        Some(Geste::Bout { id, fin, cote }) => {
            let mut sauf = vec![id];
            if let Some((source, _)) = cote {
                sauf.push(source);
            }
            // An arrow others are tied to does not tie to an arrow itself: ties never
            // chain.
            let porte = t.board.items.iter().any(|c| {
                [c.from, c.to]
                    .into_iter()
                    .flatten()
                    .any(|e| e.item == Some(id))
            });
            if porte {
                sauf.extend(
                    t.board
                        .items
                        .iter()
                        .filter(|c| c.kind == Kind::Connector)
                        .map(|c| c.id),
                );
            }
            // Ctrl keeps the end free, wherever it is dropped.
            let mut e = if ctrl {
                End::free(p)
            } else {
                t.board.attach(p, accroche, &sauf)
            };
            // A free end nearly level with the other end, or nearly above it: put in
            // line, so the arrow is straight.
            if e.item.is_none() && !ctrl {
                let autre = t
                    .board
                    .get(id)
                    .and_then(|a| if fin { a.from } else { a.to });
                if let Some(o) = autre.map(|o| t.board.end_point(&o)) {
                    let pres = t.tol(8.0);
                    if (e.y - o.y).abs() <= pres {
                        e.y = o.y;
                    }
                    if (e.x - o.x).abs() <= pres {
                        e.x = o.x;
                    }
                }
            }
            t.survol = e.item.or_else(|| {
                t.board.hit(p, accroche).filter(|i| {
                    *i != id && t.board.get(*i).is_some_and(|it| peut_tenir(&t.board, it))
                })
            });
            if let Some(it) = t.board.get_mut(id) {
                if fin {
                    it.to = Some(e);
                } else {
                    it.from = Some(e);
                }
            }
        }
        Some(Geste::Coin { id, k }) => {
            let elbow = t.board.get(id).is_some_and(|i| i.way == Way::Elbow);
            if let Some(it) = t.board.get_mut(id) {
                if k < it.points.len() {
                    if elbow {
                        let old = it.points[k];
                        let n = it.points.len();
                        // The corners on either side follow, so the pieces stay square.
                        for j in [k.wrapping_sub(1), k + 1] {
                            if j < n {
                                let q = it.points[j];
                                if (q.y - old.y).abs() < 0.5 {
                                    it.points[j].y = p.y;
                                } else if (q.x - old.x).abs() < 0.5 {
                                    it.points[j].x = p.x;
                                }
                            }
                        }
                    }
                    it.points[k] = p;
                }
            }
        }
        Some(Geste::Segment {
            id,
            k,
            depart,
            origine,
        }) => {
            let (dx, dy) = (p.x - depart.x, p.y - depart.y);
            if let Some(it) = t.board.get_mut(id) {
                // The piece runs from interior point k − 1 to k (`origine` has no ends).
                let (i0, i1) = (k - 1, k);
                if let (Some(a), Some(b)) = (origine.get(i0), origine.get(i1)) {
                    let level = (a.y - b.y).abs() < 0.5;
                    for i in [i0, i1] {
                        if let (Some(o), Some(q)) = (origine.get(i), it.points.get_mut(i)) {
                            if level {
                                q.y = o.y + dy;
                            } else {
                                q.x = o.x + dx;
                            }
                        }
                    }
                }
            }
        }
        Some(Geste::Trait { id }) => {
            let min = t.tol(1.5);
            if let Some(it) = t.board.get_mut(id) {
                if it.points.last().is_none_or(|q| q.distance(p) >= min) {
                    it.points.push(p);
                }
            }
        }
        Some(Geste::Creer { id, depart }) => {
            let mut r = Rect::spanning(depart, p);
            if shift {
                let c = r.w.max(r.h);
                r.x = if p.x < depart.x {
                    depart.x - c
                } else {
                    depart.x
                };
                r.y = if p.y < depart.y {
                    depart.y - c
                } else {
                    depart.y
                };
                r.w = c;
                r.h = c;
            } else if !ctrl {
                // The corner being drawn meets the edges and middles around it.
                let across = if p.x >= depart.x {
                    Lines::END
                } else {
                    Lines::START
                };
                let down = if p.y >= depart.y {
                    Lines::END
                } else {
                    Lines::START
                };
                (r, t.guides) = resized(&t.board, id, r, across, down, aimant, pres);
            }
            if let Some(it) = t.board.get_mut(id) {
                it.set_rect(r);
            }
        }
        None => {}
    }
    montrer(f, t);
}

/// A box being resized or drawn, its moving edges aligned with the boxes around.
fn resized(
    b: &Board,
    id: u64,
    mut n: Rect,
    across: Lines,
    down: Lines,
    tol: f32,
    pres: Rect,
) -> (Rect, Vec<snap::Guide>) {
    let (sx, sy, g) = snap::align(b, &[id], n, across, down, tol, pres);
    if across == Lines::START {
        n.x += sx;
        n.w -= sx;
    } else if across == Lines::END {
        n.w += sx;
    }
    if down == Lines::START {
        n.y += sy;
        n.h -= sy;
    } else if down == Lines::END {
        n.h += sy;
    }
    (n, g)
}

fn lacher(f: &AppWindow, t: &mut Tableau, p: Pt) {
    t.guides.clear();
    let Some(geste) = t.geste.take() else {
        montrer(f, t);
        return;
    };
    let bouge = t.bouge;
    t.bouge = false;
    t.survol = None;
    match geste {
        // A corner or a piece only clicked: the arrow as it was (a press on a piece's
        // middle had already put a point there, to carry).
        Geste::Coin { .. } | Geste::Segment { .. } if !bouge => {
            if t.undo.len() > t.pile {
                if let Some(b) = t.undo.pop() {
                    t.board = b;
                }
            }
            montrer(f, t);
            return;
        }
        Geste::Deplacer { .. } if !bouge => {
            // A click on one of several chosen: that one alone. A second click on the
            // only box chosen opens its words (an arrow's double-click bends it instead).
            if let Some(id) = t.board.hit(p, t.tol(6.0)) {
                if t.deja && t.board.get(id).is_some_and(|i| i.kind.boxed()) {
                    t.oublier_si_pareil();
                    editer(f, t, id);
                    return;
                }
                if t.reduire {
                    t.choix = vec![id];
                }
            }
        }
        Geste::Creer { id, depart } => {
            let (w, h) = taille_usuelle(t.outil, t.forme);
            let petit = t.board.get(id).is_some_and(|i| i.w < 8.0 || i.h < 8.0);
            if petit || !bouge {
                if let Some(it) = t.board.get_mut(id) {
                    if it.kind == Kind::Text {
                        it.set_rect(Rect::new(depart.x, depart.y - h / 2.0, w, h));
                    } else {
                        it.set_rect(Rect::new(depart.x - w / 2.0, depart.y - h / 2.0, w, h));
                    }
                }
            }
            let kind = t.board.get(id).map(|i| i.kind);
            t.outil = Outil::Choisir;
            // A sticky note, a text, a frame: its words typed at once, one step of undo
            // with it.
            if matches!(kind, Some(Kind::Sticky | Kind::Text | Kind::Frame)) {
                let texte = t.board.get(id).map(|i| i.text.clone()).unwrap_or_default();
                t.edition = Some(id);
                t.choix = vec![id];
                let g = f.global::<Vue>();
                g.set_edit_text(texte.into());
                g.set_edit_serial(g.get_edit_serial() + 1);
                change(f, t);
                return;
            }
        }
        Geste::Bout { id, cote, .. } => {
            let court = t.board.get(id).is_some_and(|a| {
                let (x, _, y, _) = route::ends(&t.board, a);
                x.distance(y) < t.tol(6.0)
            });
            if court {
                match cote {
                    // A click on "+": a copy of the item beyond, tied to it.
                    Some((source, s)) if !bouge => {
                        prolonger(t, source, s, id);
                        t.choix = vec![id];
                    }
                    _ => {
                        t.board.remove(&[id]);
                        t.choix.clear();
                    }
                }
            }
            if t.outil == Outil::Fleche {
                t.outil = Outil::Choisir;
            }
        }
        Geste::Trait { id } => {
            let min = t.tol(1.0);
            if let Some(it) = t.board.get_mut(id) {
                it.points = geom::thin(&it.points, min);
            }
        }
        Geste::Segment { id, .. } | Geste::Coin { id, .. } => {
            // Points that fell in line with their neighbours go.
            if let Some(it) = t.board.get(id) {
                if it.way == Way::Elbow {
                    let pts = route::through(&t.board, it);
                    let simple = route::simplify(&pts);
                    let interieur = if simple.len() > 2 {
                        simple[1..simple.len() - 1].to_vec()
                    } else {
                        Vec::new()
                    };
                    if let Some(it) = t.board.get_mut(id) {
                        it.points = interieur;
                    }
                }
            }
        }
        _ => {}
    }
    // A step kept at the press: dropped when nothing changed, else the change is kept.
    if t.undo.len() > t.pile {
        if t.undo.last() == Some(&t.board) {
            t.undo.pop();
            montrer(f, t);
        } else {
            change(f, t);
        }
    } else {
        montrer(f, t);
    }
}

/// What an arrow's end can tie to: a box, or an arrow not itself tied to an arrow.
fn peut_tenir(b: &Board, it: &Item) -> bool {
    it.kind.boxed() || (it.kind == Kind::Connector && !b.on_a_line(it))
}

/// Whether `p` is on an arrow's words (the patch they stand on).
fn sur_les_mots(b: &Board, id: u64, p: Pt) -> bool {
    let Some(it) = b.get(id) else { return false };
    if it.text.trim().is_empty() {
        return false;
    }
    let m = route::curve(b, it).at(route::label_at(it));
    let plus_longue = it
        .text
        .lines()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as f32;
    let lignes = it.text.lines().count().max(1) as f32;
    let w = plus_longue * it.size * 0.55 + 14.0;
    let h = lignes * it.size * 1.35 + 6.0;
    Rect::new(m.x - w / 2.0, m.y - h / 2.0, w, h).contains(p)
}

/// A point added to the arrow `id` at `p`, in the piece it falls on: the arrow then
/// goes through it, and it can be dragged as any other.
fn ajouter_point(t: &mut Tableau, id: u64, p: Pt) -> Option<usize> {
    fixer_coins(t, id);
    let it = t.board.get(id)?;
    let full = route::through(&t.board, it);
    let k = full
        .windows(2)
        .enumerate()
        .map(|(k, w)| (k, geom::segment_distance(w[0], w[1], p)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0, |(k, _)| k);
    let it = t.board.get_mut(id)?;
    let k = k.min(it.points.len());
    it.points.insert(k, p);
    Some(k)
}

fn double_clic(f: &AppWindow, t: &mut Tableau, p: Pt) {
    t.geste = None;
    t.bouge = false;
    if t.edition.is_some() {
        return;
    }
    // A double-click on an arrow's point takes it away.
    if let Some(Role::Coin(k)) = t.repere_sous(p) {
        if let Some(id) = t.choix.first().copied() {
            t.retenir();
            fixer_coins(t, id);
            if let Some(it) = t.board.get_mut(id) {
                if k < it.points.len() {
                    it.points.remove(k);
                }
            }
            change(f, t);
            return;
        }
    }
    if t.outil != Outil::Choisir {
        return;
    }
    match t.board.hit(p, t.tol(6.0)) {
        // On an arrow: its words when on them, a new point to bend it anywhere else.
        Some(id) if t.board.get(id).is_some_and(|i| i.kind == Kind::Connector) => {
            if sur_les_mots(&t.board, id, p) {
                editer(f, t, id);
            } else {
                t.retenir();
                ajouter_point(t, id, p);
                t.choix = vec![id];
                change(f, t);
            }
        }
        Some(id) => editer(f, t, id),
        None => {
            // On the empty board: a text, typed at once.
            t.retenir();
            t.outil = Outil::Texte;
            let (w, h) = taille_usuelle(Outil::Texte, t.forme);
            let id = creer(t, Rect::new(p.x, p.y - h / 2.0, w, h));
            t.outil = Outil::Choisir;
            t.choix = vec![id];
            t.edition = Some(id);
            let g = f.global::<Vue>();
            g.set_edit_text(SharedString::default());
            g.set_edit_serial(g.get_edit_serial() + 1);
            change(f, t);
        }
    }
}

fn survoler(f: &AppWindow, t: &mut Tableau, p: Pt) {
    let g = f.global::<Vue>();
    let curseur = match t.outil {
        Outil::Choisir => match t.repere_sous(p) {
            Some(Role::Redim(i)) => match i {
                0 | 4 => 3,
                2 | 6 => 4,
                1 | 5 => 5,
                _ => 6,
            },
            Some(_) => 8,
            None => {
                if t.board
                    .hit(p, t.tol(6.0))
                    .and_then(|i| t.board.get(i))
                    .is_some_and(|i| !i.locked)
                {
                    1
                } else {
                    0
                }
            }
        },
        Outil::Main => 0,
        _ => 2,
    };
    if curseur != t.curseur {
        t.curseur = curseur;
        g.set_cursor(curseur);
    }
    if t.outil == Outil::Fleche {
        let s = t
            .board
            .hit(p, t.tol(14.0))
            .filter(|i| t.board.get(*i).is_some_and(|it| peut_tenir(&t.board, it)));
        if s != t.survol {
            t.survol = s;
            montrer_poignees(f, t);
        }
    }
}

// --- The toolbar's choices and the keys ------------------------------------------------

/// A colour as the menu sends it: its place in the palette, `none`, or `#rrggbb`.
fn couleur_choisie(v: &str) -> Option<String> {
    if v == "none" {
        return Some("none".into());
    }
    if let Ok(i) = v.parse::<usize>() {
        return COLOURS.get(i).map(|s| s.to_string());
    }
    let hex = v.trim();
    let hex = if hex.starts_with('#') {
        hex.to_string()
    } else {
        format!("#{hex}")
    };
    match Colour::parse(&hex) {
        Colour::Rgb(_) => Some(hex.to_ascii_lowercase()),
        _ => None,
    }
}

/// Every chosen item (the one typed in, while typing) changed by `fx`.
fn appliquer(f: &AppWindow, t: &mut Tableau, fx: impl Fn(&mut Item)) {
    let ids: Vec<u64> = match t.edition {
        Some(id) => vec![id],
        None => t.choix.clone(),
    };
    if ids.is_empty() {
        return;
    }
    t.retenir();
    for id in ids {
        if let Some(it) = t.board.get_mut(id) {
            fx(it);
        }
    }
    t.oublier_si_pareil();
    change(f, t);
}

fn copier(t: &Tableau) {
    let items: Vec<Item> = t
        .board
        .items
        .iter()
        .filter(|i| t.choix.contains(&i.id))
        .cloned()
        .collect();
    if !items.is_empty() {
        PRESSE.with(|p| *p.borrow_mut() = items);
    }
}

fn coller(f: &AppWindow, t: &mut Tableau) {
    let items = PRESSE.with(|p| p.borrow().clone());
    if items.is_empty() {
        return;
    }
    let b = Board {
        items: items.clone(),
        ..Board::default()
    };
    let boite = b.bounds().unwrap_or_default();
    // Beside where they were when that shows; in the middle of the view otherwise.
    let (dx, dy) = if t.vue.intersects(boite) {
        (24.0, 24.0)
    } else {
        let c = t.vue.center();
        let m = boite.center();
        (c.x - m.x, c.y - m.y)
    };
    t.retenir();
    t.choix = t.board.paste(&items, dx, dy);
    PRESSE.with(|p| {
        // The next paste lands a step further.
        for it in p.borrow_mut().iter_mut() {
            match it.kind {
                Kind::Ink | Kind::Connector => {
                    for q in &mut it.points {
                        q.x += 24.0;
                        q.y += 24.0;
                    }
                    for e in [&mut it.from, &mut it.to].into_iter().flatten() {
                        if e.item.is_none() {
                            e.x += 24.0;
                            e.y += 24.0;
                        }
                    }
                }
                _ => {
                    it.x += 24.0;
                    it.y += 24.0;
                }
            }
        }
    });
    change(f, t);
}

fn annuler(f: &AppWindow, t: &mut Tableau, refaire: bool) {
    let (depuis, vers) = if refaire {
        (&mut t.redo, &mut t.undo)
    } else {
        (&mut t.undo, &mut t.redo)
    };
    let Some(b) = depuis.pop() else { return };
    vers.push(std::mem::replace(&mut t.board, b));
    t.edition = None;
    t.geste = None;
    change(f, t);
}

fn action(f: &AppWindow, t: &mut Tableau, quoi: &str) {
    let (verbe, valeur) = quoi.split_once(':').unwrap_or((quoi, ""));
    let entier = valeur.parse::<usize>().unwrap_or(0);
    match verbe {
        "tool" => {
            if t.edition.is_some() {
                finir_edition(f, t);
            }
            t.outil = Outil::TOUS.get(entier).copied().unwrap_or(Outil::Choisir);
            t.survol = None;
            montrer(f, t);
        }
        "shape-tool" => {
            t.forme = Shape::from_index(entier);
            t.outil = Outil::Forme;
            montrer(f, t);
        }
        "fill" => {
            if let Some(c) = couleur_choisie(valeur) {
                appliquer(f, t, |it| {
                    if it.kind.boxed() {
                        it.fill = c.clone();
                    }
                });
            }
        }
        "stroke" => {
            if let Some(c) = couleur_choisie(valeur) {
                appliquer(f, t, |it| it.stroke = c.clone());
            }
        }
        "colour" => {
            if let Some(c) = couleur_choisie(valeur) {
                appliquer(f, t, |it| it.color = c.clone());
            }
        }
        "opacity" => {
            let o = (entier as f32 / 100.0).clamp(0.05, 1.0);
            appliquer(f, t, |it| it.opacity = o);
        }
        "font" => {
            let font = Font::from_index(entier);
            appliquer(f, t, |it| it.font = font);
        }
        "size" => {
            if let Ok(s) = valeur.parse::<f32>() {
                let s = s.clamp(4.0, 400.0);
                appliquer(f, t, |it| it.size = s);
            }
        }
        "size+" | "size-" => {
            let plus = verbe == "size+";
            appliquer(f, t, |it| {
                let s = it.size;
                it.size = if plus {
                    TAILLES.iter().copied().find(|x| *x > s + 0.01).unwrap_or(s)
                } else {
                    TAILLES
                        .iter()
                        .rev()
                        .copied()
                        .find(|x| *x < s - 0.01)
                        .unwrap_or(s)
                };
            });
        }
        "bold" => {
            let b = !t
                .choix
                .first()
                .and_then(|i| t.board.get(*i))
                .is_some_and(|i| i.bold);
            appliquer(f, t, |it| it.bold = b);
        }
        "italic" => {
            let b = !t
                .choix
                .first()
                .and_then(|i| t.board.get(*i))
                .is_some_and(|i| i.italic);
            appliquer(f, t, |it| it.italic = b);
        }
        // Across (along, on an arrow): one given, or the next one.
        "align" => {
            let a = if valeur.is_empty() {
                let a = t
                    .choix
                    .first()
                    .and_then(|i| t.board.get(*i))
                    .map_or(Align::Center, |i| i.align);
                Align::from_index((a.index() + 1) % 3)
            } else {
                Align::from_index(entier)
            };
            appliquer(f, t, |it| it.align = a);
        }
        "valign" => {
            let v = VAlign::from_index(entier);
            appliquer(f, t, |it| {
                if it.kind.boxed() {
                    it.valign = v;
                }
            });
        }
        "width" => {
            if let Ok(w) = valeur.parse::<f32>() {
                let w = w.clamp(0.5, 40.0);
                appliquer(f, t, |it| it.width = w);
            }
        }
        "shape" => {
            let s = Shape::from_index(entier);
            appliquer(f, t, |it| {
                if it.kind == Kind::Shape {
                    it.shape = s;
                }
            });
        }
        "way" => {
            let w = Way::from_index(entier);
            appliquer(f, t, |it| {
                if it.kind == Kind::Connector && it.way != w {
                    // Points placed for one way mean nothing for another.
                    if it.way == Way::Elbow || w == Way::Elbow {
                        it.points.clear();
                    }
                    it.way = w;
                }
            });
        }
        "rounded" => {
            let r = !t
                .choix
                .first()
                .and_then(|i| t.board.get(*i))
                .is_some_and(|i| i.rounded);
            appliquer(f, t, |it| it.rounded = r);
        }
        "start" | "end" => {
            let h = Head::from_index(entier);
            let fin = verbe == "end";
            appliquer(f, t, |it| {
                if fin {
                    it.end = h;
                } else {
                    it.start = h;
                }
            });
        }
        "lock" => {
            let l = !t
                .choix
                .first()
                .and_then(|i| t.board.get(*i))
                .is_some_and(|i| i.locked);
            appliquer(f, t, |it| it.locked = l);
        }
        "front" | "back" => {
            if !t.choix.is_empty() {
                t.retenir();
                let ids = t.choix.clone();
                t.board.restack(&ids, verbe == "front");
                t.oublier_si_pareil();
                change(f, t);
            }
        }
        "duplicate" => {
            if !t.choix.is_empty() {
                t.retenir();
                let ids = t.choix.clone();
                t.choix = t.board.duplicate(&ids, 24.0, 24.0);
                change(f, t);
            }
        }
        "delete" => {
            let ids: Vec<u64> = t
                .choix
                .iter()
                .copied()
                .filter(|i| t.board.get(*i).is_some_and(|it| !it.locked))
                .collect();
            if !ids.is_empty() {
                t.retenir();
                t.board.remove(&ids);
                t.choix.clear();
                change(f, t);
            }
        }
        "copy" => copier(t),
        "cut" => {
            copier(t);
            action(f, t, "delete");
        }
        "paste" => coller(f, t),
        "select-all" => {
            t.choix = t.board.items.iter().map(|i| i.id).collect();
            montrer(f, t);
        }
        "undo" => annuler(f, t, false),
        "redo" => annuler(f, t, true),
        "edit" => {
            if let Some(id) = t.un_seul().map(|i| i.id) {
                editer(f, t, id);
            }
        }
        "add-sticky" | "add-text" => {
            let avant = t.outil;
            t.outil = if verbe == "add-sticky" {
                Outil::Note
            } else {
                Outil::Texte
            };
            let (w, h) = taille_usuelle(t.outil, t.forme);
            t.retenir();
            let p = t.appui;
            let id = creer(t, Rect::new(p.x - w / 2.0, p.y - h / 2.0, w, h));
            t.outil = avant;
            t.edition = Some(id);
            t.choix = vec![id];
            let g = f.global::<Vue>();
            g.set_edit_text(SharedString::default());
            g.set_edit_serial(g.get_edit_serial() + 1);
            change(f, t);
        }
        _ => {}
    }
}

fn touche(f: &AppWindow, t: &mut Tableau, texte: &str, shift: bool, ctrl: bool) -> bool {
    use slint::platform::Key;
    let k = |key: Key| -> SharedString { key.into() };
    let s = SharedString::from(texte);
    if s == k(Key::Delete) || s == k(Key::Backspace) {
        action(f, t, "delete");
        return true;
    }
    if s == k(Key::Escape) {
        if t.outil != Outil::Choisir {
            t.outil = Outil::Choisir;
        } else {
            t.choix.clear();
        }
        t.survol = None;
        montrer(f, t);
        return true;
    }
    if s == k(Key::Return) || s == k(Key::F2) {
        action(f, t, "edit");
        return true;
    }
    for (key, dx, dy) in [
        (Key::LeftArrow, -1.0, 0.0),
        (Key::RightArrow, 1.0, 0.0),
        (Key::UpArrow, 0.0, -1.0),
        (Key::DownArrow, 0.0, 1.0),
    ] {
        if s == k(key) {
            let pas = if shift { 10.0 } else { 1.0 };
            let ids: Vec<u64> = t
                .choix
                .iter()
                .copied()
                .filter(|i| t.board.get(*i).is_some_and(|it| !it.locked))
                .collect();
            if ids.is_empty() {
                return false;
            }
            t.retenir();
            t.board.translate(&ids, dx * pas, dy * pas);
            change(f, t);
            return true;
        }
    }
    let lettre = texte.to_lowercase();
    if ctrl {
        let quoi = match lettre.as_str() {
            "z" if shift => "redo",
            "z" => "undo",
            "y" => "redo",
            "a" => "select-all",
            "c" => "copy",
            "x" => "cut",
            "v" => "paste",
            "d" => "duplicate",
            "b" => "bold",
            "i" => "italic",
            "l" => "lock",
            "]" => "front",
            "[" => "back",
            _ => return false,
        };
        action(f, t, quoi);
        return true;
    }
    let outil = match lettre.as_str() {
        "v" => Outil::Choisir,
        "h" => Outil::Main,
        "n" => Outil::Note,
        "s" => Outil::Forme,
        "r" => {
            t.forme = Shape::Rect;
            Outil::Forme
        }
        "o" => {
            t.forme = Shape::Ellipse;
            Outil::Forme
        }
        "t" => Outil::Texte,
        "a" | "l" => Outil::Fleche,
        "p" => Outil::Stylo,
        "f" => Outil::Cadre,
        _ => return false,
    };
    t.outil = outil;
    t.survol = None;
    montrer(f, t);
    true
}

// --- Opening and writing ---------------------------------------------------------------

/// Opens the whiteboard `rel` of the space.
pub(super) fn ouvrir(f: &AppWindow, e: &mut Etat, rel: &str) {
    let Some(espace) = e.espace() else { return };
    let (texte, modified) = match espace.read(rel) {
        Ok(x) => x,
        Err(err) => {
            f.set_status(format!("Could not open the whiteboard: {err}").into());
            return;
        }
    };
    let board = if texte.trim().is_empty() {
        Board::default()
    } else {
        match Board::from_json(&texte) {
            Ok(b) => b,
            Err(err) => {
                f.set_status(format!("This whiteboard could not be read: {err}").into());
                return;
            }
        }
    };
    let mut t = Tableau::nouveau(rel, board, Some(modified));
    let g = f.global::<Vue>();
    g.set_menu(SharedString::default());
    g.set_items(ModelRc::from(t.items.clone()));
    montrer(f, &mut t);
    e.board = Some(t);
    f.set_note_board_title(stem(rel).into());
    f.set_note_board_open(true);
    g.set_fit_serial(g.get_fit_serial() + 1);
    g.set_focus_serial(g.get_focus_serial() + 1);
}

/// Writes the whiteboard if it changed.
pub(super) fn ecrire(f: &AppWindow, e: &mut Etat) {
    let Some(espace) = e.spaces.get(e.space).cloned() else {
        return;
    };
    let Some(t) = &mut e.board else { return };
    if !t.dirty {
        return;
    }
    match espace.write(&t.rel, &t.board.to_json(), t.modified) {
        Ok(iris_vault::WriteOutcome::Written(m)) => {
            t.modified = Some(m);
            t.dirty = false;
            f.set_note_status("Saved".into());
        }
        Ok(iris_vault::WriteOutcome::Conflict { saved_as, modified }) => {
            t.modified = Some(modified);
            t.dirty = false;
            f.set_toast(
                format!(
                    "This whiteboard was changed elsewhere too: that version is kept as “{}”.",
                    stem(&saved_as)
                )
                .into(),
            );
        }
        Err(err) => f.set_status(format!("Could not save the whiteboard: {err}").into()),
    }
}

/// Closes the whiteboard, its typing kept and the file written first.
pub(super) fn fermer(f: &AppWindow, e: &mut Etat) {
    if let Some(t) = &mut e.board {
        if t.edition.take().is_some() {
            t.dirty = true;
        }
    }
    ecrire(f, e);
    e.board = None;
    f.set_note_board_open(false);
    let g = f.global::<Vue>();
    g.set_menu(SharedString::default());
    g.set_editing(false);
}

/// The whiteboard `rel` made into a PNG where the user says: the one open as it is now,
/// else its file.
pub(super) fn exporter_png(f: &AppWindow, e: &mut Etat, rel: &str) {
    let board = match &e.board {
        Some(t) if t.rel == rel => t.board.clone(),
        _ => {
            let Some(espace) = e.espace() else { return };
            match espace
                .read(rel)
                .map_err(|e| e.to_string())
                .and_then(|(t, _)| Board::from_json(&t).map_err(|e| e.to_string()))
            {
                Ok(b) => b,
                Err(err) => {
                    f.set_status(format!("Could not read the whiteboard: {err}").into());
                    return;
                }
            }
        }
    };
    let Some((svg, boite)) = iris_board::svg::to_svg(&board) else {
        f.set_toast("This whiteboard is empty: there is nothing to export.".into());
        return;
    };
    let nom = stem(rel);
    let Some(chemin) = crate::dialogue::FileDialog::new()
        .set_title("Export the whiteboard as an image")
        .set_file_name(format!("{nom}.png"))
        .add_filter("PNG image", &["png"])
        .save_file()
    else {
        return;
    };
    match png(&svg, boite).and_then(|o| std::fs::write(&chemin, o).map_err(|e| e.to_string())) {
        Ok(()) => f.set_toast(format!("“{nom}” is exported.").into()),
        Err(err) => f.set_status(format!("Could not export the whiteboard: {err}").into()),
    }
}

/// The fonts the board uses, from the system's folder, for the picture's words.
fn polices(db: &mut resvg::usvg::fontdb::Database) {
    let dossier =
        std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()))
            .join("Fonts");
    for nom in [
        "SegUIVar.ttf",
        "segoeui.ttf",
        "segoeuib.ttf",
        "segoeuii.ttf",
        "georgia.ttf",
        "georgiab.ttf",
        "georgiai.ttf",
        "CascadiaMono.ttf",
        "consola.ttf",
        "segoepr.ttf",
        "segoeprb.ttf",
        "arial.ttf",
    ] {
        if let Ok(octets) = std::fs::read(dossier.join(nom)) {
            db.load_font_data(octets);
        }
    }
    db.set_sans_serif_family("Segoe UI");
}

/// The SVG drawn at twice the board's size (no side over 8000 pixels), as PNG bytes.
fn png(svg: &str, boite: Rect) -> Result<Vec<u8>, String> {
    use resvg::{tiny_skia, usvg};
    let mut opt = usvg::Options {
        font_family: "Segoe UI".into(),
        ..usvg::Options::default()
    };
    polices(opt.fontdb_mut());
    let arbre = usvg::Tree::from_str(svg, &opt).map_err(|e| e.to_string())?;
    let cote = boite.w.max(boite.h).max(1.0);
    let k = 2.0_f32.min(8000.0 / cote);
    let (w, h) = (
        ((boite.w * k).ceil() as u32).max(1),
        ((boite.h * k).ceil() as u32).max(1),
    );
    let mut pixels =
        tiny_skia::Pixmap::new(w, h).ok_or_else(|| "the picture would be too large".to_string())?;
    resvg::render(
        &arbre,
        tiny_skia::Transform::from_scale(k, k),
        &mut pixels.as_mut(),
    );
    // Tiny-skia keeps its colours premultiplied: given back as they are.
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for p in pixels.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let image = image::RgbaImage::from_raw(w, h, rgba).ok_or("the picture could not be made")?;
    let mut sortie = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut sortie, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(sortie.into_inner())
}

/// A new whiteboard in `folder`.
pub(super) fn nouveau(f: &AppWindow, e: &mut Etat, folder: &str) -> Option<String> {
    let espace = e.espace()?;
    match espace.create_file(
        folder,
        "Untitled",
        "board",
        Board::default().to_json().as_bytes(),
    ) {
        Ok(rel) => Some(rel),
        Err(err) => {
            f.set_status(format!("Could not create the whiteboard: {err}").into());
            None
        }
    }
}

// --- The window's callbacks ------------------------------------------------------------

pub(super) fn wire(f: &AppWindow, etat: &Rc<RefCell<Etat>>) {
    let g = f.global::<Vue>();
    macro_rules! geste {
        ($installer:ident, |$f:ident, $t:ident $(, $arg:ident)*| $corps:block) => {{
            let (etat, faible) = (Rc::clone(etat), f.as_weak());
            g.$installer(move |$($arg),*| {
                let Some($f) = faible.upgrade() else { return };
                let Ok(mut guard) = etat.try_borrow_mut() else { return };
                let Some($t) = guard.board.as_mut() else { return };
                $corps
            });
        }};
    }
    geste!(on_viewport, |f, t, px, py, zoom, w, h| {
        t.zoom = zoom.max(0.001);
        t.vue = Rect::new(-px / t.zoom, -py / t.zoom, w / t.zoom, h / t.zoom);
        // The dots' tile made again only when their spacing on screen changes.
        let s = ecart_points(t.zoom);
        if s != t.grille {
            t.grille = s;
            let g = f.global::<Vue>();
            g.set_grid_tile(tuile(s));
            g.set_grid_size(s as i32);
        }
    });
    geste!(on_pointer, |f, t, kind, button, x, y, shift, ctrl| {
        let p = Pt::new(x, y);
        match kind {
            0 => presser(&f, t, p, shift, button == 1),
            1 if button < 0 => survoler(&f, t, p),
            1 => tirer(&f, t, p, shift, ctrl),
            2 => lacher(&f, t, p),
            3 => double_clic(&f, t, p),
            _ => {}
        }
    });
    geste!(on_action, |f, t, quoi| {
        action(&f, t, quoi.as_str());
    });
    geste!(on_edit_changed, |f, t, texte, hauteur| {
        let Some(id) = t.edition else { return };
        if let Some(it) = t.board.get_mut(id) {
            it.text = texte.to_string();
            // A sticky note or a text grows with its words.
            if matches!(it.kind, Kind::Sticky | Kind::Text) {
                let besoin = hauteur + 2.0 * marge(it.kind);
                if besoin > it.h {
                    it.h = besoin;
                }
            }
        }
        t.dirty = true;
        montrer(&f, t);
        super::planifier_ecriture(&f);
    });
    geste!(on_edit_done, |f, t, texte| {
        if let Some(it) = t.edition.and_then(|id| t.board.get_mut(id)) {
            it.text = texte.to_string();
        }
        finir_edition(&f, t);
        let g = f.global::<Vue>();
        g.set_focus_serial(g.get_focus_serial() + 1);
    });
    {
        let (etat, faible) = (Rc::clone(etat), f.as_weak());
        g.on_key(move |texte, shift, ctrl| {
            let Some(f) = faible.upgrade() else {
                return false;
            };
            let Ok(mut guard) = etat.try_borrow_mut() else {
                return false;
            };
            let Some(t) = guard.board.as_mut() else {
                return false;
            };
            touche(&f, t, texte.as_str(), shift, ctrl)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dots_stay_at_least_twelve_pixels_apart() {
        assert_eq!(ecart_points(1.0), 20);
        assert_eq!(ecart_points(0.85), 17);
        // Closer than 12 pixels: every fifth dot.
        assert_eq!(ecart_points(0.5), 50);
        assert_eq!(ecart_points(0.01), 25);
        for z in [0.013, 0.07, 0.3, 0.59, 2.0, 8.0] {
            assert!(ecart_points(z) >= 12, "{z}");
        }
        let t = tuile(20);
        assert_eq!((t.size().width, t.size().height), (20, 20));
    }

    #[test]
    fn colours_from_the_menu() {
        assert_eq!(couleur_choisie("5").as_deref(), Some("blue-soft"));
        assert_eq!(couleur_choisie("none").as_deref(), Some("none"));
        assert_eq!(couleur_choisie("#3A7BD5").as_deref(), Some("#3a7bd5"));
        assert_eq!(couleur_choisie("3a7bd5").as_deref(), Some("#3a7bd5"));
        assert_eq!(couleur_choisie("blue"), None);
        assert_eq!(couleur_choisie("99"), None);
    }

    #[test]
    fn an_item_is_given_to_the_canvas_with_its_colours_and_outline() {
        let mut b = Board::default();
        let id = b.take_id();
        b.items.push(Item::shape(
            id,
            Shape::Diamond,
            Rect::new(0.0, 0.0, 100.0, 60.0),
        ));
        let d = donnee(&b, &b.items[0], false, 0);
        assert_eq!(d.kind, 0);
        assert_eq!(d.fill, 5);
        assert_eq!(d.stroke, 14);
        assert!(d.path.starts_with("M 50 0"));
        // A shape's words stand in its middle unless told otherwise.
        assert_eq!(d.valign, 1);
        let a = b.take_id();
        b.items.push(Item::connector(
            a,
            End::on(id, 1.0, 0.5),
            End::free(Pt::new(300.0, 30.0)),
        ));
        let d = donnee(&b, &b.items[1], false, 1);
        assert_eq!(d.kind, 4);
        assert_eq!(d.marked, 1);
        assert!(d.x <= 100.0 && d.x + d.width >= 300.0);
        assert!(!d.heads.is_empty());
    }

    #[test]
    fn a_point_is_added_to_the_piece_it_falls_on() {
        let mut b = Board::default();
        let a = b.take_id();
        b.items.push(Item::connector(
            a,
            End::free(Pt::new(0.0, 0.0)),
            End::free(Pt::new(200.0, 0.0)),
        ));
        let mut t = Tableau::nouveau("x.board", b, None);
        assert_eq!(ajouter_point(&mut t, a, Pt::new(100.0, 30.0)), Some(0));
        assert_eq!(ajouter_point(&mut t, a, Pt::new(150.0, 10.0)), Some(1));
        assert_eq!(
            t.board.get(a).unwrap().points,
            vec![Pt::new(100.0, 30.0), Pt::new(150.0, 10.0)]
        );
        // Its words, when it has some, are where a double-click edits rather than bends.
        t.board.get_mut(a).unwrap().text = "yes".into();
        let m = route::curve(&t.board, t.board.get(a).unwrap()).at(0.5);
        assert!(sur_les_mots(&t.board, a, m));
        assert!(!sur_les_mots(&t.board, a, Pt::new(0.0, 0.0)));
    }

    #[test]
    fn a_resized_edge_meets_its_neighbour() {
        let mut b = Board::default();
        let x = b.take_id();
        b.items.push(Item::shape(
            x,
            Shape::Rect,
            Rect::new(0.0, 0.0, 100.0, 50.0),
        ));
        let y = b.take_id();
        b.items.push(Item::shape(
            y,
            Shape::Rect,
            Rect::new(0.0, 100.0, 96.0, 50.0),
        ));
        let pres = Rect::new(-500.0, -500.0, 1000.0, 1000.0);
        let (n, g) = resized(
            &b,
            y,
            Rect::new(0.0, 100.0, 96.0, 50.0),
            Lines::END,
            Lines::NONE,
            6.0,
            pres,
        );
        assert_eq!(n.w, 100.0);
        assert!(g.iter().any(|g| g.vertical && g.at == 100.0));
    }

    #[test]
    fn a_shape_shows_eight_handles_and_four_pluses() {
        let mut b = Board::default();
        let id = b.take_id();
        b.items.push(Item::shape(
            id,
            Shape::Rect,
            Rect::new(0.0, 0.0, 100.0, 60.0),
        ));
        let mut t = Tableau::nouveau("x.board", b, None);
        t.choix = vec![id];
        let r = t.reperes();
        assert_eq!(
            r.iter()
                .filter(|(_, r)| matches!(r, Role::Redim(_)))
                .count(),
            8
        );
        assert_eq!(
            r.iter().filter(|(_, r)| matches!(r, Role::Plus(_))).count(),
            4
        );
        // The "+" on the right stands 18 pixels out of the side.
        assert_eq!(t.repere_sous(Pt::new(118.0, 30.0)), Some(Role::Plus(1)));
        assert_eq!(t.repere_sous(Pt::new(100.0, 60.0)), Some(Role::Redim(4)));
    }

    #[test]
    fn a_click_on_a_plus_makes_a_tied_copy() {
        let mut b = Board::default();
        let id = b.take_id();
        b.items.push(Item::shape(
            id,
            Shape::Rect,
            Rect::new(0.0, 0.0, 100.0, 60.0),
        ));
        let mut t = Tableau::nouveau("x.board", b, None);
        let a = t.board.take_id();
        t.board.items.push(Item::connector(
            a,
            End::on(id, 1.0, 0.5),
            End::free(Pt::new(118.0, 30.0)),
        ));
        let copie = prolonger(&mut t, id, 1, a).unwrap();
        let c = t.board.get(copie).unwrap();
        assert_eq!(c.x, 180.0);
        assert_eq!(t.board.get(a).unwrap().to.unwrap().item, Some(copie));
    }

    #[test]
    fn an_elbows_end_piece_gets_a_corner_to_move_by() {
        let mut b = Board::default();
        let x = b.take_id();
        b.items.push(Item::shape(
            x,
            Shape::Rect,
            Rect::new(0.0, 0.0, 100.0, 50.0),
        ));
        let y = b.take_id();
        b.items.push(Item::shape(
            y,
            Shape::Rect,
            Rect::new(300.0, 200.0, 100.0, 50.0),
        ));
        let a = b.take_id();
        let mut arrow = Item::connector(a, End::on(x, 1.0, 0.5), End::on(y, 0.5, 0.0));
        arrow.way = Way::Elbow;
        b.items.push(arrow);
        let mut t = Tableau::nouveau("x.board", b, None);
        // The first piece, from the left box to the corner, taken by its middle.
        let g = commencer_segment(&mut t, a, 0, Pt::new(225.0, 25.0)).unwrap();
        let Geste::Segment { k, origine, .. } = g else {
            panic!("{g:?}")
        };
        assert_eq!(k, 1);
        assert_eq!(origine[0], Pt::new(124.0, 25.0));
        assert_eq!(t.board.get(a).unwrap().points.len(), 2);
    }
}
