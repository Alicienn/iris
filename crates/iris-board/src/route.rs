//! Where an arrow goes.
//!
//! Its ends are found first: an end tied to an item sits at its fraction of the item's
//! box and leaves it out of the side it is on; a free end has no side. Then its way:
//!
//! - **straight**: from one end to the other through the points the user placed;
//! - **elbow**: at right angles. Without points, the way is found: out of each item a
//!   little, then across. With points (the corners the user moved), they are kept, and a
//!   corner is added wherever two of them are not in line;
//! - **curved**: a smooth curve through the points, leaving and reaching each item
//!   square to its side.
//!
//! Corners can be drawn round, and each end can carry a head.

use crate::geom::{Curve, Pt, Rect, Seg};
use crate::{Board, End, Head, Item, Way};
use std::ops::{Add, Sub};

/// How far an elbow goes out of an item before it turns.
pub const STUB: f32 = 24.0;
/// How round an arrow's round corners are.
pub const ROUND: f32 = 14.0;

/// Which way an end leaves its item (none when it is free).
pub fn normal(board: &Board, e: &End) -> Option<Pt> {
    let it = board.get(e.item?)?;
    let (u, v) = (e.x, e.y);
    let sides = [
        (v * it.h, Pt::new(0.0, -1.0)),
        ((1.0 - u) * it.w, Pt::new(1.0, 0.0)),
        ((1.0 - v) * it.h, Pt::new(0.0, 1.0)),
        (u * it.w, Pt::new(-1.0, 0.0)),
    ];
    sides
        .iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, n)| *n)
}

/// An arrow's two ends on the board, each with the way it leaves its item.
pub fn ends(board: &Board, it: &Item) -> (Pt, Option<Pt>, Pt, Option<Pt>) {
    let from = it.from.unwrap_or_default();
    let to = it.to.unwrap_or_default();
    (
        board.end_point(&from),
        normal(board, &from),
        board.end_point(&to),
        normal(board, &to),
    )
}

/// Points in a row with the repeated ones and the ones in the middle of a straight run
/// taken out.
pub fn simplify(pts: &[Pt]) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::new();
    for p in pts {
        if out.last().is_some_and(|q| q.distance(*p) < 0.5) {
            continue;
        }
        out.push(*p);
        while out.len() >= 3 {
            let n = out.len();
            let (a, b, c) = (out[n - 3], out[n - 2], out[n - 1]);
            let ab = b.sub(a);
            let bc = c.sub(b);
            let cross = ab.x * bc.y - ab.y * bc.x;
            let dot = ab.x * bc.x + ab.y * bc.y;
            if cross.abs() <= 0.01 * ab.length() * bc.length() && dot >= 0.0 {
                out.remove(n - 2);
            } else {
                break;
            }
        }
    }
    out
}

fn horizontal(n: Option<Pt>, from: Pt, to: Pt) -> bool {
    match n {
        Some(n) => n.x.abs() > 0.5,
        None => (to.x - from.x).abs() >= (to.y - from.y).abs(),
    }
}

/// The way at right angles found between two ends.
pub fn auto_elbow(a: Pt, na: Option<Pt>, b: Pt, nb: Option<Pt>) -> Vec<Pt> {
    let p1 = na.map_or(a, |n| a.add(n.scale(STUB)));
    let p2 = nb.map_or(b, |n| b.add(n.scale(STUB)));
    let ha = horizontal(na, p1, p2);
    let hb = horizontal(nb, p2, p1);
    let meme = matches!((na, nb), (Some(x), Some(y)) if x == y);
    let milieu = match (ha, hb) {
        (true, true) => {
            let mx = match na {
                Some(n) if meme && n.x > 0.0 => p1.x.max(p2.x),
                Some(n) if meme && n.x < 0.0 => p1.x.min(p2.x),
                _ => (p1.x + p2.x) / 2.0,
            };
            vec![Pt::new(mx, p1.y), Pt::new(mx, p2.y)]
        }
        (false, false) => {
            let my = match na {
                Some(n) if meme && n.y > 0.0 => p1.y.max(p2.y),
                Some(n) if meme && n.y < 0.0 => p1.y.min(p2.y),
                _ => (p1.y + p2.y) / 2.0,
            };
            vec![Pt::new(p1.x, my), Pt::new(p2.x, my)]
        }
        (true, false) => vec![Pt::new(p2.x, p1.y)],
        (false, true) => vec![Pt::new(p1.x, p2.y)],
    };
    let mut pts = vec![a, p1];
    pts.extend(milieu);
    pts.push(p2);
    pts.push(b);
    simplify(&pts)
}

/// The points the user placed, kept, with a corner added wherever two in a row are not
/// in line, so every piece is level or upright.
pub fn orthogonal(a: Pt, na: Option<Pt>, points: &[Pt], b: Pt) -> Vec<Pt> {
    let mut out = vec![a];
    let premier = points.first().copied().unwrap_or(b);
    let mut h = horizontal(na, a, premier);
    for q in points.iter().copied().chain(std::iter::once(b)) {
        let last = *out.last().unwrap_or(&a);
        let (dx, dy) = ((q.x - last.x).abs(), (q.y - last.y).abs());
        if dx > 0.5 && dy > 0.5 {
            out.push(if h {
                Pt::new(q.x, last.y)
            } else {
                Pt::new(last.x, q.y)
            });
        } else if dx > 0.5 || dy > 0.5 {
            h = dy > 0.5;
        }
        out.push(q);
    }
    simplify(&out)
}

/// The points a straight or elbow arrow goes through, its ends included; a curved
/// arrow's points to pass through.
pub fn through(board: &Board, it: &Item) -> Vec<Pt> {
    let (a, na, b, nb) = ends(board, it);
    match it.way {
        Way::Elbow if it.points.is_empty() => auto_elbow(a, na, b, nb),
        Way::Elbow => orthogonal(a, na, &it.points, b),
        Way::Straight | Way::Curved => {
            let mut v = vec![a];
            v.extend(it.points.iter().copied());
            v.push(b);
            v
        }
    }
}

/// A line through points with its corners rounded by up to `r`.
pub fn rounded(pts: &[Pt], r: f32) -> Curve {
    let mut c = Curve {
        start: pts.first().copied().unwrap_or_default(),
        segs: Vec::new(),
    };
    if pts.len() < 3 {
        c.segs.extend(pts.iter().skip(1).map(|p| Seg::Line(*p)));
        return c;
    }
    for i in 1..pts.len() - 1 {
        let (p, q, s) = (pts[i - 1], pts[i], pts[i + 1]);
        let (d1, d2) = (q.sub(p), s.sub(q));
        let rr = r.min(d1.length() / 2.0).min(d2.length() / 2.0);
        c.segs.push(Seg::Line(q.sub(d1.unit().scale(rr))));
        c.segs.push(Seg::Quad(q, q.add(d2.unit().scale(rr))));
    }
    c.segs.push(Seg::Line(pts[pts.len() - 1]));
    c
}

/// A smooth curve through points, leaving the first along `na` and reaching the last
/// against `nb`.
pub fn smooth(pts: &[Pt], na: Option<Pt>, nb: Option<Pt>) -> Curve {
    let n = pts.len();
    let mut c = Curve {
        start: pts.first().copied().unwrap_or_default(),
        segs: Vec::new(),
    };
    if n < 2 {
        return c;
    }
    // The way the curve goes at each point (Catmull-Rom), the ends square to their
    // items.
    let mut m: Vec<Pt> = Vec::with_capacity(n);
    for i in 0..n {
        let t = if i == 0 {
            let l = pts[0].distance(pts[1]);
            match na {
                Some(d) => d.scale(l.max(40.0)),
                None => pts[1].sub(pts[0]),
            }
        } else if i == n - 1 {
            let l = pts[n - 2].distance(pts[n - 1]);
            match nb {
                Some(d) => d.scale(-l.max(40.0)),
                None => pts[n - 1].sub(pts[n - 2]),
            }
        } else {
            pts[i + 1].sub(pts[i - 1]).scale(0.5)
        };
        m.push(t);
    }
    // Two ends square to their items, with nothing between: a little more swing.
    if n == 2 && (na.is_some() || nb.is_some()) {
        for t in &mut m {
            *t = t.scale(1.2);
        }
    }
    for i in 0..n - 1 {
        c.segs.push(Seg::Cubic(
            pts[i].add(m[i].scale(1.0 / 3.0)),
            pts[i + 1].sub(m[i + 1].scale(1.0 / 3.0)),
            pts[i + 1],
        ));
    }
    c
}

/// The arrow's line on the board.
pub fn curve(board: &Board, it: &Item) -> Curve {
    let pts = through(board, it);
    match it.way {
        Way::Curved => {
            let (_, na, _, nb) = ends(board, it);
            smooth(&pts, na, nb)
        }
        _ if it.rounded => rounded(&pts, ROUND),
        _ => Curve::polyline(&pts),
    }
}

/// The middles of an arrow's pieces, where dragging bends it: on the polyline for
/// straight and elbow arrows, halfway along each curve for a curved one.
pub fn midpoints(board: &Board, it: &Item) -> Vec<Pt> {
    let pts = through(board, it);
    match it.way {
        Way::Curved => {
            let (_, na, _, nb) = ends(board, it);
            let c = smooth(&pts, na, nb);
            let mut cur = c.start;
            c.segs
                .iter()
                .map(|s| {
                    let piece = Curve {
                        start: cur,
                        segs: vec![*s],
                    };
                    cur = s.end();
                    piece.midpoint()
                })
                .collect()
        }
        _ => pts.windows(2).map(|w| w[0].lerp(w[1], 0.5)).collect(),
    }
}

/// How large an arrow's head is, for a line this wide.
pub fn head_size(width: f32) -> f32 {
    7.0 + 2.2 * width
}

/// A head drawn at `tip`, the line arriving along `d` (one unit long, pointing out).
pub fn head(kind: Head, tip: Pt, d: Pt, s: f32) -> Vec<Pt> {
    let perp = Pt::new(-d.y, d.x);
    match kind {
        Head::None => Vec::new(),
        Head::Arrow => vec![
            tip,
            tip.sub(d.scale(s)).add(perp.scale(s * 0.5)),
            tip.sub(d.scale(s)).sub(perp.scale(s * 0.5)),
        ],
        Head::Diamond => vec![
            tip,
            tip.sub(d.scale(s * 0.6)).add(perp.scale(s * 0.38)),
            tip.sub(d.scale(s * 1.2)),
            tip.sub(d.scale(s * 0.6)).sub(perp.scale(s * 0.38)),
        ],
        Head::Dot => {
            let c = tip.sub(d.scale(s * 0.3));
            (0..12)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 12.0;
                    c.add(Pt::new(a.cos(), a.sin()).scale(s * 0.32))
                })
                .collect()
        }
    }
}

/// How far the line stops short of a head, so its end does not show past it.
fn trim(kind: Head, s: f32) -> f32 {
    match kind {
        Head::None => 0.0,
        Head::Arrow => s * 0.7,
        Head::Diamond => s * 1.0,
        Head::Dot => s * 0.3,
    }
}

/// The curve with its start and end pulled back along its way.
fn shortened(mut c: Curve, start: f32, end: f32) -> Curve {
    if end > 0.0 {
        let d = c.direction(true);
        let v = d.scale(-end);
        if let Some(last) = c.segs.last_mut() {
            *last = match *last {
                Seg::Line(p) => Seg::Line(p.add(v)),
                Seg::Quad(k, p) => Seg::Quad(k.add(v), p.add(v)),
                Seg::Cubic(k1, k2, p) => Seg::Cubic(k1, k2.add(v), p.add(v)),
            };
        }
    }
    if start > 0.0 {
        let d = c.direction(false);
        let v = d.scale(-start);
        c.start = c.start.add(v);
        if let Some(first) = c.segs.first_mut() {
            *first = match *first {
                Seg::Cubic(k1, k2, p) => Seg::Cubic(k1.add(v), k2, p),
                Seg::Quad(k, p) => Seg::Quad(k.add(v), p),
                s => s,
            };
        }
    }
    c
}

/// An arrow as it is drawn: its line (stopped short of its heads), its heads as filled
/// outlines, and the box around both.
pub struct Drawn {
    pub line: Curve,
    pub heads: Vec<Vec<Pt>>,
    pub bounds: Rect,
    /// Halfway along, where its words go.
    pub label: Pt,
}

pub fn drawn(board: &Board, it: &Item) -> Drawn {
    let c = curve(board, it);
    let s = head_size(it.width);
    let mut heads = Vec::new();
    if it.end != Head::None {
        heads.push(head(it.end, c.end(), c.direction(true), s));
    }
    if it.start != Head::None {
        heads.push(head(it.start, c.start, c.direction(false), s));
    }
    let label = c.midpoint();
    let line = shortened(c, trim(it.start, s), trim(it.end, s));
    let mut bounds = line.bounds();
    for h in &heads {
        if let Some(r) = Rect::around(h) {
            bounds = bounds.union(r);
        }
    }
    Drawn {
        line,
        heads,
        bounds,
        label,
    }
}

/// Filled outlines as path commands, with `o` as the origin.
pub fn polygons(polys: &[Vec<Pt>], o: Pt) -> String {
    let mut s = String::new();
    for p in polys.iter().filter(|p| p.len() > 1) {
        let c = Curve::polyline(p);
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(&c.commands(o));
        s.push_str(" Z");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Item, Shape};

    fn deux(way: Way) -> (Board, u64) {
        let mut b = Board::default();
        let a = b.take_id();
        b.items.push(Item::shape(
            a,
            Shape::Rect,
            Rect::new(0.0, 0.0, 100.0, 50.0),
        ));
        let c = b.take_id();
        b.items.push(Item::shape(
            c,
            Shape::Rect,
            Rect::new(300.0, 200.0, 100.0, 50.0),
        ));
        let k = b.take_id();
        let mut arrow = Item::connector(k, End::on(a, 1.0, 0.5), End::on(c, 0.5, 0.0));
        arrow.way = way;
        b.items.push(arrow);
        (b, k)
    }

    #[test]
    fn an_end_leaves_by_its_side() {
        let (b, k) = deux(Way::Straight);
        let (a, na, z, nz) = ends(&b, b.get(k).unwrap());
        assert_eq!(a, Pt::new(100.0, 25.0));
        assert_eq!(na, Some(Pt::new(1.0, 0.0)));
        assert_eq!(z, Pt::new(350.0, 200.0));
        assert_eq!(nz, Some(Pt::new(0.0, -1.0)));
    }

    #[test]
    fn an_elbow_turns_once_from_a_side_to_a_top() {
        let (b, k) = deux(Way::Elbow);
        let pts = through(&b, b.get(k).unwrap());
        assert_eq!(
            pts,
            vec![
                Pt::new(100.0, 25.0),
                Pt::new(350.0, 25.0),
                Pt::new(350.0, 200.0)
            ]
        );
    }

    #[test]
    fn an_elbow_between_two_right_sides_goes_round() {
        let pts = auto_elbow(
            Pt::new(0.0, 0.0),
            Some(Pt::new(1.0, 0.0)),
            Pt::new(10.0, 100.0),
            Some(Pt::new(1.0, 0.0)),
        );
        assert_eq!(
            pts,
            vec![
                Pt::new(0.0, 0.0),
                Pt::new(34.0, 0.0),
                Pt::new(34.0, 100.0),
                Pt::new(10.0, 100.0)
            ]
        );
    }

    #[test]
    fn a_moved_corner_is_kept_and_the_way_stays_square() {
        let pts = orthogonal(
            Pt::new(0.0, 0.0),
            Some(Pt::new(1.0, 0.0)),
            &[Pt::new(50.0, 80.0)],
            Pt::new(100.0, 80.0),
        );
        assert_eq!(
            pts,
            vec![
                Pt::new(0.0, 0.0),
                Pt::new(50.0, 0.0),
                Pt::new(50.0, 80.0),
                Pt::new(100.0, 80.0)
            ]
        );
        for w in pts.windows(2) {
            assert!(w[0].x == w[1].x || w[0].y == w[1].y);
        }
    }

    #[test]
    fn round_corners_stay_within_their_pieces() {
        let c = rounded(
            &[Pt::new(0.0, 0.0), Pt::new(10.0, 0.0), Pt::new(10.0, 100.0)],
            ROUND,
        );
        assert_eq!(c.segs[0], Seg::Line(Pt::new(5.0, 0.0)));
        assert_eq!(c.segs[1], Seg::Quad(Pt::new(10.0, 0.0), Pt::new(10.0, 5.0)));
        assert_eq!(c.end(), Pt::new(10.0, 100.0));
    }

    #[test]
    fn a_curve_passes_through_its_points() {
        let (mut b, k) = deux(Way::Curved);
        b.get_mut(k).unwrap().points = vec![Pt::new(200.0, 0.0)];
        let c = curve(&b, b.get(k).unwrap());
        assert_eq!(c.segs.len(), 2);
        assert_eq!(c.segs[0].end(), Pt::new(200.0, 0.0));
        assert_eq!(midpoints(&b, b.get(k).unwrap()).len(), 2);
    }

    #[test]
    fn a_head_sits_on_the_end_and_the_line_stops_short() {
        let (b, k) = deux(Way::Straight);
        let d = drawn(&b, b.get(k).unwrap());
        assert_eq!(d.heads.len(), 1);
        assert_eq!(d.heads[0][0], Pt::new(350.0, 200.0));
        assert!(d.line.end().distance(Pt::new(350.0, 200.0)) > 1.0);
        assert!(d.bounds.contains(Pt::new(350.0, 200.0)));
        assert!(polygons(&d.heads, Pt::default()).ends_with('Z'));
    }
}
