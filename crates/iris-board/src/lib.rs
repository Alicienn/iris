//! `iris-board` — whiteboards, without I/O.
//!
//! A board is a list of items drawn in its order (the last on top), on a plane without
//! edges: it is as large as what is on it. Every measure is in board units, one pixel
//! at 100 %.
//!
//! - [`Item`] is any of them: a shape, a sticky note, a text, a frame, an arrow
//!   (`Connector`) or a pen stroke (`Ink`), with its colours, its type and its words;
//! - [`geom`] holds the plane's geometry: points, boxes, curves;
//! - [`route`] says where an arrow goes: its ends, tied to an item or free, and its
//!   way between them — straight, at right angles or curved — through the points the
//!   user moved;
//! - [`Board`] finds what lies under a point and writes itself as JSON (the `.board`
//!   file).

#![forbid(unsafe_code)]

pub mod geom;
pub mod route;

pub use geom::{Curve, Pt, Rect};
use serde::{Deserialize, Serialize};

/// The file's format, written in it.
pub const FORMAT: u32 = 1;

/// The colours an item can take, by name, as the file writes them. The window draws
/// each in the theme showing: a soft fill and a strong ink of nine hues, the paper, the
/// ink, a softer ink and the lines' grey. Anything else is written `#rrggbb`.
pub const COLOURS: [&str; 22] = [
    "yellow-soft",
    "orange-soft",
    "red-soft",
    "pink-soft",
    "purple-soft",
    "blue-soft",
    "teal-soft",
    "green-soft",
    "grey-soft",
    "yellow",
    "orange",
    "red",
    "pink",
    "purple",
    "blue",
    "teal",
    "green",
    "grey",
    "paper",
    "ink",
    "ink-soft",
    "line",
];

/// What a colour written in the file is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Colour {
    None,
    /// One of [`COLOURS`].
    Named(usize),
    /// `0xRRGGBB`.
    Rgb(u32),
}

impl Colour {
    pub fn parse(s: &str) -> Colour {
        let s = s.trim();
        if s.is_empty() || s == "none" {
            return Colour::None;
        }
        if let Some(i) = COLOURS.iter().position(|c| c.eq_ignore_ascii_case(s)) {
            return Colour::Named(i);
        }
        let hex = s.trim_start_matches('#');
        if hex.len() == 6 {
            if let Ok(v) = u32::from_str_radix(hex, 16) {
                return Colour::Rgb(v);
            }
        }
        Colour::None
    }

    pub fn write(self) -> String {
        match self {
            Colour::None => "none".into(),
            Colour::Named(i) => COLOURS.get(i).copied().unwrap_or("ink").into(),
            Colour::Rgb(v) => format!("#{v:06x}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Shape,
    Sticky,
    Text,
    /// A titled area; moving it moves what lies inside.
    Frame,
    /// An arrow or a line between two ends.
    Connector,
    /// A stroke of the pen.
    Ink,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Shape,
        Kind::Sticky,
        Kind::Text,
        Kind::Frame,
        Kind::Connector,
        Kind::Ink,
    ];

    pub fn index(self) -> usize {
        Kind::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// An item with a box one can type in and tie arrows to.
    pub fn boxed(self) -> bool {
        matches!(self, Kind::Shape | Kind::Sticky | Kind::Text | Kind::Frame)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    Rect,
    #[default]
    Rounded,
    Ellipse,
    Diamond,
    Triangle,
    Parallelogram,
    Hexagon,
    Star,
}

impl Shape {
    pub const ALL: [Shape; 8] = [
        Shape::Rect,
        Shape::Rounded,
        Shape::Ellipse,
        Shape::Diamond,
        Shape::Triangle,
        Shape::Parallelogram,
        Shape::Hexagon,
        Shape::Star,
    ];

    pub fn index(self) -> usize {
        Shape::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Shape {
        Shape::ALL.get(i).copied().unwrap_or_default()
    }
}

/// An arrow's way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Way {
    #[default]
    Straight,
    /// At right angles.
    Elbow,
    Curved,
}

impl Way {
    pub const ALL: [Way; 3] = [Way::Straight, Way::Elbow, Way::Curved];
    pub fn index(self) -> usize {
        Way::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }
    pub fn from_index(i: usize) -> Way {
        Way::ALL.get(i).copied().unwrap_or_default()
    }
}

/// What an arrow's end is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Head {
    #[default]
    None,
    Arrow,
    Dot,
    Diamond,
}

impl Head {
    pub const ALL: [Head; 4] = [Head::None, Head::Arrow, Head::Dot, Head::Diamond];
    pub fn index(self) -> usize {
        Head::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }
    pub fn from_index(i: usize) -> Head {
        Head::ALL.get(i).copied().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Font {
    #[default]
    Sans,
    Display,
    Serif,
    Mono,
    Hand,
}

impl Font {
    pub const ALL: [Font; 5] = [
        Font::Sans,
        Font::Display,
        Font::Serif,
        Font::Mono,
        Font::Hand,
    ];
    pub fn index(self) -> usize {
        Font::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }
    pub fn from_index(i: usize) -> Font {
        Font::ALL.get(i).copied().unwrap_or_default()
    }
    pub fn name(self) -> &'static str {
        match self {
            Font::Sans => "Sans",
            Font::Display => "Display",
            Font::Serif => "Serif",
            Font::Mono => "Mono",
            Font::Hand => "Handwriting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

impl Align {
    pub const ALL: [Align; 3] = [Align::Left, Align::Center, Align::Right];
    pub fn index(self) -> usize {
        Align::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }
    pub fn from_index(i: usize) -> Align {
        Align::ALL.get(i).copied().unwrap_or_default()
    }
}

/// An arrow's end: tied to an item, at `(x, y)` as fractions of its box (0 to 1), or
/// free, at `(x, y)` on the board.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct End {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<u64>,
    pub x: f32,
    pub y: f32,
}

impl End {
    pub fn free(p: Pt) -> End {
        End {
            item: None,
            x: p.x,
            y: p.y,
        }
    }

    pub fn on(item: u64, u: f32, v: f32) -> End {
        End {
            item: Some(item),
            x: u.clamp(0.0, 1.0),
            y: v.clamp(0.0, 1.0),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_one(v: &f32) -> bool {
    (*v - 1.0).abs() < f32::EPSILON
}

/// Anything on the board. The fields an item's kind has no use for keep their default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    pub id: u64,
    pub kind: Kind,
    pub shape: Shape,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    pub fill: String,
    /// The fill's opacity, 0 to 1.
    #[serde(skip_serializing_if = "is_one")]
    pub opacity: f32,
    pub stroke: String,
    pub width: f32,
    /// The text's colour.
    pub color: String,
    pub font: Font,
    pub size: f32,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    pub align: Align,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<End>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<End>,
    pub way: Way,
    /// An arrow's corners drawn round.
    #[serde(skip_serializing_if = "is_false")]
    pub rounded: bool,
    pub start: Head,
    pub end: Head,
    /// An arrow's points the user placed (none: its way is found), or a stroke's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Pt>,
    /// Neither moved nor resized.
    #[serde(skip_serializing_if = "is_false")]
    pub locked: bool,
}

impl Default for Item {
    fn default() -> Self {
        Item {
            id: 0,
            kind: Kind::Shape,
            shape: Shape::Rounded,
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
            text: String::new(),
            fill: "none".into(),
            opacity: 1.0,
            stroke: "none".into(),
            width: 1.5,
            color: "ink".into(),
            font: Font::Sans,
            size: 14.0,
            bold: false,
            italic: false,
            align: Align::Center,
            from: None,
            to: None,
            way: Way::Straight,
            rounded: false,
            start: Head::None,
            end: Head::None,
            points: Vec::new(),
            locked: false,
        }
    }
}

impl Item {
    /// A shape as the toolbar makes it: a soft blue fill, a blue border.
    pub fn shape(id: u64, shape: Shape, r: Rect) -> Item {
        Item {
            id,
            kind: Kind::Shape,
            shape,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            fill: "blue-soft".into(),
            stroke: "blue".into(),
            ..Item::default()
        }
    }

    pub fn sticky(id: u64, r: Rect) -> Item {
        Item {
            id,
            kind: Kind::Sticky,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            fill: "yellow-soft".into(),
            align: Align::Left,
            ..Item::default()
        }
    }

    pub fn text(id: u64, r: Rect) -> Item {
        Item {
            id,
            kind: Kind::Text,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            size: 18.0,
            align: Align::Left,
            ..Item::default()
        }
    }

    pub fn frame(id: u64, r: Rect) -> Item {
        Item {
            id,
            kind: Kind::Frame,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            text: "Frame".into(),
            fill: "paper".into(),
            stroke: "line".into(),
            width: 1.0,
            size: 12.0,
            align: Align::Left,
            ..Item::default()
        }
    }

    pub fn connector(id: u64, from: End, to: End) -> Item {
        Item {
            id,
            kind: Kind::Connector,
            stroke: "ink-soft".into(),
            width: 1.6,
            size: 12.0,
            from: Some(from),
            to: Some(to),
            end: Head::Arrow,
            ..Item::default()
        }
    }

    pub fn ink(id: u64, points: Vec<Pt>) -> Item {
        Item {
            id,
            kind: Kind::Ink,
            stroke: "ink".into(),
            width: 3.0,
            points,
            ..Item::default()
        }
    }

    /// Its box, for the kinds that have one.
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, self.h)
    }

    pub fn set_rect(&mut self, r: Rect) {
        self.x = r.x;
        self.y = r.y;
        self.w = r.w;
        self.h = r.h;
    }

    /// The point `(u, v)` of its box, as fractions.
    pub fn at(&self, u: f32, v: f32) -> Pt {
        Pt::new(self.x + u * self.w, self.y + v * self.h)
    }
}

/// A whiteboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Board {
    pub format: u32,
    /// The id the next item takes.
    pub next: u64,
    /// In the order they are drawn: the last on top.
    pub items: Vec<Item>,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            format: FORMAT,
            next: 1,
            items: Vec::new(),
        }
    }
}

/// How close, in board units, an arrow's end comes to a side's middle to be tied there.
pub const SNAP: f32 = 12.0;

impl Board {
    pub fn from_json(text: &str) -> Result<Board, serde_json::Error> {
        let mut b: Board = serde_json::from_str(text)?;
        b.repair();
        Ok(b)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    /// Ids made unique and `next` past them; arrows tied to nothing set free.
    fn repair(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let mut max = 0;
        for it in &self.items {
            max = max.max(it.id);
        }
        for it in &mut self.items {
            if it.id == 0 || !seen.insert(it.id) {
                max += 1;
                it.id = max;
                seen.insert(it.id);
            }
        }
        self.next = self.next.max(max + 1);
        let ids: std::collections::HashSet<u64> = self.items.iter().map(|i| i.id).collect();
        let boxes: std::collections::HashMap<u64, Rect> = self
            .items
            .iter()
            .filter(|i| i.kind.boxed())
            .map(|i| (i.id, i.rect()))
            .collect();
        for it in &mut self.items {
            for end in [&mut it.from, &mut it.to].into_iter().flatten() {
                if let Some(id) = end.item {
                    if !ids.contains(&id) || !boxes.contains_key(&id) {
                        end.item = None;
                    }
                }
            }
            if it.kind == Kind::Connector {
                it.from.get_or_insert_with(End::default);
                it.to.get_or_insert_with(End::default);
            }
            if !(0.0..=1.0).contains(&it.opacity) {
                it.opacity = 1.0;
            }
            if it.size.is_nan() || it.size <= 0.0 {
                it.size = 14.0;
            }
            if it.w < 0.0 {
                it.x += it.w;
                it.w = -it.w;
            }
            if it.h < 0.0 {
                it.y += it.h;
                it.h = -it.h;
            }
        }
    }

    /// A fresh id.
    pub fn take_id(&mut self) -> u64 {
        let max = self.items.iter().map(|i| i.id).max().unwrap_or(0);
        let id = self.next.max(max + 1);
        self.next = id + 1;
        id
    }

    pub fn index(&self, id: u64) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    pub fn get(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// Where an end is on the board.
    pub fn end_point(&self, e: &End) -> Pt {
        match e.item.and_then(|id| self.get(id)) {
            Some(it) => it.at(e.x, e.y),
            None => Pt::new(e.x, e.y),
        }
    }

    /// The box an item takes on the board, an arrow's and a stroke's included.
    pub fn bbox(&self, it: &Item) -> Rect {
        match it.kind {
            Kind::Connector => route::curve(self, it).bounds(),
            Kind::Ink => Rect::around(&it.points).unwrap_or_default(),
            _ => it.rect(),
        }
    }

    /// The box around everything.
    pub fn bounds(&self) -> Option<Rect> {
        self.items
            .iter()
            .map(|i| self.bbox(i))
            .reduce(|a, b| a.union(b))
    }

    /// The topmost item under `p`, `tol` the room around thin things (board units). A
    /// frame is taken only by its title or its edge, so what it holds stays clickable
    /// and an empty part of it starts a selection.
    pub fn hit(&self, p: Pt, tol: f32) -> Option<u64> {
        for it in self.items.iter().rev() {
            let hit = match it.kind {
                Kind::Connector => route::curve(self, it).distance(p) <= tol + it.width / 2.0,
                Kind::Ink => geom::polyline_distance(&it.points, p) <= tol + it.width / 2.0,
                Kind::Frame => {
                    let r = it.rect();
                    let titre = Rect::new(r.x, r.y - 22.0, r.w.max(80.0), 22.0);
                    let dedans = r.inflate(tol).contains(p) && !r.inflate(-tol).contains(p);
                    titre.contains(p) || dedans
                }
                Kind::Shape => geom::in_shape(it.shape, it.rect().inflate(tol / 2.0), p),
                _ => it.rect().inflate(tol / 2.0).contains(p),
            };
            if hit {
                return Some(it.id);
            }
        }
        None
    }

    /// The items whose whole box lies in `r`.
    pub fn inside(&self, r: Rect) -> Vec<u64> {
        self.items
            .iter()
            .filter(|i| r.contains_rect(self.bbox(i)))
            .map(|i| i.id)
            .collect()
    }

    /// Where an arrow's end dropped at `p` ties: the middle of a side within `snap`, or
    /// the outline where the line from the item's centre through `p` crosses it, when
    /// `p` is on the item; free otherwise. `except` is never tied to (the arrow itself,
    /// the item at its other end when the arrow would loop on it is still allowed).
    pub fn attach(&self, p: Pt, snap: f32, except: &[u64]) -> End {
        for it in self.items.iter().rev() {
            if !it.kind.boxed() || except.contains(&it.id) {
                continue;
            }
            let r = it.rect();
            if !r.inflate(snap).contains(p) {
                continue;
            }
            // A frame ties by its edge only, as it is picked.
            if it.kind == Kind::Frame && r.inflate(-snap).contains(p) {
                continue;
            }
            for (u, v) in SIDES {
                if it.at(u, v).distance(p) <= snap {
                    return End::on(it.id, u, v);
                }
            }
            if r.contains(p) || it.kind == Kind::Frame {
                let q = geom::outline_toward(it.shape, it.kind, r, p);
                let (u, v) = if r.w > 0.0 && r.h > 0.0 {
                    ((q.x - r.x) / r.w, (q.y - r.y) / r.h)
                } else {
                    (0.5, 0.5)
                };
                return End::on(it.id, u, v);
            }
        }
        End::free(p)
    }

    /// Items removed, with the arrows tied to them.
    pub fn remove(&mut self, ids: &[u64]) {
        let tied = |e: &Option<End>| e.and_then(|e| e.item).is_some_and(|i| ids.contains(&i));
        self.items.retain(|i| {
            !ids.contains(&i.id) && !(i.kind == Kind::Connector && (tied(&i.from) || tied(&i.to)))
        });
    }

    /// Items moved by `(dx, dy)`: their boxes, a stroke's points, a free end's place,
    /// and the points of an arrow whose ends both move (or are free and chosen).
    pub fn translate(&mut self, ids: &[u64], dx: f32, dy: f32) {
        let moved: std::collections::HashSet<u64> = ids.iter().copied().collect();
        for it in &mut self.items {
            let chosen = moved.contains(&it.id);
            match it.kind {
                Kind::Ink => {
                    if chosen {
                        for p in &mut it.points {
                            p.x += dx;
                            p.y += dy;
                        }
                    }
                }
                Kind::Connector => {
                    let follows = |e: &Option<End>| match e {
                        Some(End { item: Some(i), .. }) => moved.contains(i),
                        _ => chosen,
                    };
                    let (a, b) = (follows(&it.from), follows(&it.to));
                    for e in [&mut it.from, &mut it.to].into_iter().flatten() {
                        if e.item.is_none() && chosen {
                            e.x += dx;
                            e.y += dy;
                        }
                    }
                    if a && b {
                        for p in &mut it.points {
                            p.x += dx;
                            p.y += dy;
                        }
                    }
                }
                _ => {
                    if chosen {
                        it.x += dx;
                        it.y += dy;
                    }
                }
            }
        }
    }

    /// What a frame holds: the items whose box lies within it, arrows and strokes too.
    pub fn held_by(&self, frame: u64) -> Vec<u64> {
        let Some(f) = self.get(frame) else {
            return Vec::new();
        };
        let r = f.rect();
        self.items
            .iter()
            .filter(|i| i.id != frame && r.contains_rect(self.bbox(i)))
            .map(|i| i.id)
            .collect()
    }

    /// Items brought to the top (`front`) or sent to the bottom, keeping their order.
    /// Frames stay under everything that is not a frame.
    pub fn restack(&mut self, ids: &[u64], front: bool) {
        let (mut pris, reste): (Vec<Item>, Vec<Item>) =
            self.items.drain(..).partition(|i| ids.contains(&i.id));
        let mut items = reste;
        if front {
            items.append(&mut pris);
        } else {
            pris.append(&mut items);
            items = pris;
        }
        // Frames first, the rest in their order.
        let (cadres, autres): (Vec<Item>, Vec<Item>) =
            items.into_iter().partition(|i| i.kind == Kind::Frame);
        self.items = cadres;
        self.items.extend(autres);
    }

    /// A copy of items under fresh ids, moved by `(dx, dy)`, the arrows between them
    /// tied to the copies, an arrow tied to an item not copied set free where it was.
    pub fn duplicate(&mut self, ids: &[u64], dx: f32, dy: f32) -> Vec<u64> {
        let copies: Vec<Item> = self
            .items
            .iter()
            .filter(|i| ids.contains(&i.id))
            .cloned()
            .collect();
        self.paste(&copies, dx, dy)
    }

    /// Items (from a copy) added under fresh ids, moved by `(dx, dy)`.
    pub fn paste(&mut self, items: &[Item], dx: f32, dy: f32) -> Vec<u64> {
        let mut map = std::collections::HashMap::new();
        for it in items {
            let id = self.take_id();
            map.insert(it.id, id);
        }
        let mut nouveaux = Vec::new();
        for it in items {
            let mut c = it.clone();
            c.id = map[&it.id];
            match c.kind {
                Kind::Ink => {
                    for p in &mut c.points {
                        p.x += dx;
                        p.y += dy;
                    }
                }
                Kind::Connector => {
                    for e in [&mut c.from, &mut c.to].into_iter().flatten() {
                        match e.item.and_then(|i| map.get(&i)) {
                            Some(n) => e.item = Some(*n),
                            None => {
                                let p = match e.item.and_then(|i| self.get(i)) {
                                    Some(src) => src.at(e.x, e.y),
                                    None => Pt::new(e.x, e.y),
                                };
                                *e = End::free(Pt::new(p.x + dx, p.y + dy));
                            }
                        }
                    }
                    for p in &mut c.points {
                        p.x += dx;
                        p.y += dy;
                    }
                }
                _ => {
                    c.x += dx;
                    c.y += dy;
                }
            }
            nouveaux.push(c);
        }
        let ids: Vec<u64> = nouveaux.iter().map(|i| i.id).collect();
        let (cadres, autres): (Vec<Item>, Vec<Item>) =
            nouveaux.into_iter().partition(|i| i.kind == Kind::Frame);
        // New frames go under everything, the rest on top.
        let mut items = cadres;
        items.append(&mut self.items);
        items.extend(autres);
        self.items = items;
        self.restack(&[], true);
        ids
    }
}

/// The middles of a box's sides, as fractions: top, right, bottom, left.
pub const SIDES: [(f32, f32); 4] = [(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)];

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Board {
        let mut b = Board::default();
        let a = b.take_id();
        b.items.push(Item::shape(
            a,
            Shape::Rounded,
            Rect::new(0.0, 0.0, 100.0, 50.0),
        ));
        let c = b.take_id();
        b.items.push(Item::shape(
            c,
            Shape::Ellipse,
            Rect::new(300.0, 0.0, 100.0, 50.0),
        ));
        let k = b.take_id();
        b.items.push(Item::connector(
            k,
            End::on(a, 1.0, 0.5),
            End::on(c, 0.0, 0.5),
        ));
        b
    }

    #[test]
    fn a_board_survives_its_file() {
        let b = board();
        let again = Board::from_json(&b.to_json()).unwrap();
        assert_eq!(b, again);
    }

    #[test]
    fn an_empty_file_reads() {
        let b = Board::from_json("{}").unwrap();
        assert!(b.items.is_empty());
        assert_eq!(b.next, 1);
    }

    #[test]
    fn ids_are_made_unique_and_ends_on_nothing_set_free() {
        let b = Board::from_json(
            r#"{"items":[{"id":3,"kind":"shape"},{"id":3,"kind":"sticky"},
               {"id":9,"kind":"connector","from":{"item":42,"x":1,"y":2},"to":{"x":5,"y":6}}]}"#,
        )
        .unwrap();
        assert_ne!(b.items[0].id, b.items[1].id);
        assert!(b.next > 9);
        assert_eq!(b.items[2].from.unwrap().item, None);
    }

    #[test]
    fn colours_read_and_write() {
        assert_eq!(Colour::parse("blue-soft"), Colour::Named(5));
        assert_eq!(Colour::parse("#FF8800"), Colour::Rgb(0xff8800));
        assert_eq!(Colour::parse("none"), Colour::None);
        assert_eq!(Colour::parse("bogus"), Colour::None);
        assert_eq!(Colour::Rgb(0xff8800).write(), "#ff8800");
        assert_eq!(Colour::Named(19).write(), "ink");
    }

    #[test]
    fn an_end_ties_to_the_middle_of_a_side_or_the_outline() {
        let b = board();
        let e = b.attach(Pt::new(98.0, 27.0), SNAP, &[]);
        assert_eq!(e.item, Some(1));
        assert_eq!((e.x, e.y), (1.0, 0.5));
        // Inside, away from a side's middle: on the outline toward the point.
        let e = b.attach(Pt::new(80.0, 10.0), SNAP, &[]);
        assert_eq!(e.item, Some(1));
        assert!(e.x == 1.0 || e.y == 0.0, "{e:?}");
        // Out of everything: free.
        let e = b.attach(Pt::new(200.0, 200.0), SNAP, &[]);
        assert_eq!(e.item, None);
        assert_eq!((e.x, e.y), (200.0, 200.0));
    }

    #[test]
    fn the_topmost_item_is_hit_and_an_arrow_by_its_line() {
        let b = board();
        assert_eq!(b.hit(Pt::new(50.0, 25.0), 4.0), Some(1));
        assert_eq!(b.hit(Pt::new(200.0, 25.0), 4.0), Some(3));
        assert_eq!(b.hit(Pt::new(200.0, 100.0), 4.0), None);
    }

    #[test]
    fn removing_an_item_removes_its_arrows() {
        let mut b = board();
        b.remove(&[1]);
        assert_eq!(b.items.len(), 1);
        assert_eq!(b.items[0].id, 2);
    }

    #[test]
    fn moving_both_ends_moves_an_arrows_points() {
        let mut b = board();
        b.items[2].way = Way::Elbow;
        b.items[2].points = vec![Pt::new(150.0, 25.0)];
        b.translate(&[1, 2], 10.0, 5.0);
        assert_eq!(b.items[0].x, 10.0);
        assert_eq!(b.items[2].points[0], Pt::new(160.0, 30.0));
        // One end only: the points stay.
        b.translate(&[1], 10.0, 0.0);
        assert_eq!(b.items[2].points[0], Pt::new(160.0, 30.0));
    }

    #[test]
    fn a_copy_ties_its_arrows_to_the_copies() {
        let mut b = board();
        let ids = b.duplicate(&[1, 2, 3], 20.0, 20.0);
        assert_eq!(ids.len(), 3);
        let arrow = b.get(ids[2]).unwrap();
        assert_eq!(arrow.from.unwrap().item, Some(ids[0]));
        // Copied alone, an arrow's ends are set free where they were.
        let ids = b.duplicate(&[3], 0.0, 0.0);
        let arrow = b.get(ids[0]).unwrap();
        assert_eq!(arrow.from.unwrap().item, None);
        assert_eq!(arrow.from.unwrap().x, 100.0);
    }

    #[test]
    fn a_frame_holds_what_lies_inside_and_stays_under() {
        let mut b = board();
        let f = b.take_id();
        b.paste(
            &[Item::frame(f, Rect::new(-20.0, -20.0, 200.0, 120.0))],
            0.0,
            0.0,
        );
        let frame = b.items.iter().find(|i| i.kind == Kind::Frame).unwrap().id;
        assert_eq!(b.items[0].id, frame);
        assert_eq!(b.held_by(frame), vec![1]);
        b.restack(&[frame], true);
        assert_eq!(b.items[0].id, frame);
    }
}
