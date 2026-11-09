//! The plane: points, boxes, the curves arrows and strokes are drawn with, and the
//! outlines of the shapes, written as path commands for the window.

use crate::{Kind, Shape};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::ops::{Add, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Add for Pt {
    type Output = Pt;
    fn add(self, o: Pt) -> Pt {
        Pt::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Pt {
    type Output = Pt;
    fn sub(self, o: Pt) -> Pt {
        Pt::new(self.x - o.x, self.y - o.y)
    }
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Pt {
        Pt { x, y }
    }

    pub fn scale(self, k: f32) -> Pt {
        Pt::new(self.x * k, self.y * k)
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    pub fn distance(self, o: Pt) -> f32 {
        self.sub(o).length()
    }

    /// The same direction, one unit long (zero stays zero).
    pub fn unit(self) -> Pt {
        let l = self.length();
        if l < 1e-6 {
            Pt::default()
        } else {
            self.scale(1.0 / l)
        }
    }

    pub fn lerp(self, o: Pt, t: f32) -> Pt {
        Pt::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
}

/// A box: its top left corner and its size (never negative).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// The box with corners `a` and `b`, whichever way round.
    pub fn spanning(a: Pt, b: Pt) -> Rect {
        Rect::new(
            a.x.min(b.x),
            a.y.min(b.y),
            (a.x - b.x).abs(),
            (a.y - b.y).abs(),
        )
    }

    /// The box around points; none for none.
    pub fn around(points: &[Pt]) -> Option<Rect> {
        let first = points.first()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn center(&self) -> Pt {
        Pt::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn union(self, o: Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(
            x,
            y,
            self.right().max(o.right()) - x,
            self.bottom().max(o.bottom()) - y,
        )
    }

    /// Grown by `d` on every side (shrunk when negative, never below nothing).
    pub fn inflate(self, d: f32) -> Rect {
        let w = (self.w + 2.0 * d).max(0.0);
        let h = (self.h + 2.0 * d).max(0.0);
        let c = self.center();
        Rect::new(c.x - w / 2.0, c.y - h / 2.0, w, h)
    }

    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }

    pub fn contains_rect(&self, o: Rect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }

    pub fn intersects(&self, o: Rect) -> bool {
        self.x <= o.right() && o.x <= self.right() && self.y <= o.bottom() && o.y <= self.bottom()
    }
}

/// A piece of a curve, from where the one before ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Line(Pt),
    /// A control point, then the end.
    Quad(Pt, Pt),
    /// Two control points, then the end.
    Cubic(Pt, Pt, Pt),
}

impl Seg {
    pub fn end(&self) -> Pt {
        match *self {
            Seg::Line(p) | Seg::Quad(_, p) | Seg::Cubic(_, _, p) => p,
        }
    }
}

/// A line made of pieces, from `start`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Curve {
    pub start: Pt,
    pub segs: Vec<Seg>,
}

const PAS: usize = 16;

impl Curve {
    pub fn polyline(points: &[Pt]) -> Curve {
        Curve {
            start: points.first().copied().unwrap_or_default(),
            segs: points.iter().skip(1).map(|p| Seg::Line(*p)).collect(),
        }
    }

    pub fn end(&self) -> Pt {
        self.segs.last().map_or(self.start, |s| s.end())
    }

    /// The curve as points close enough together to stand for it.
    pub fn flatten(&self) -> Vec<Pt> {
        let mut out = vec![self.start];
        let mut cur = self.start;
        for s in &self.segs {
            match *s {
                Seg::Line(p) => out.push(p),
                Seg::Quad(c, p) => {
                    for i in 1..=PAS {
                        let t = i as f32 / PAS as f32;
                        let a = cur.lerp(c, t);
                        let b = c.lerp(p, t);
                        out.push(a.lerp(b, t));
                    }
                }
                Seg::Cubic(c1, c2, p) => {
                    for i in 1..=PAS {
                        let t = i as f32 / PAS as f32;
                        let a = cur.lerp(c1, t);
                        let b = c1.lerp(c2, t);
                        let c = c2.lerp(p, t);
                        let ab = a.lerp(b, t);
                        let bc = b.lerp(c, t);
                        out.push(ab.lerp(bc, t));
                    }
                }
            }
            cur = s.end();
        }
        out
    }

    pub fn bounds(&self) -> Rect {
        Rect::around(&self.flatten()).unwrap_or_default()
    }

    pub fn distance(&self, p: Pt) -> f32 {
        polyline_distance(&self.flatten(), p)
    }

    /// Where the curve is halfway along.
    pub fn midpoint(&self) -> Pt {
        let pts = self.flatten();
        let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        let mut reste = total / 2.0;
        for w in pts.windows(2) {
            let l = w[0].distance(w[1]);
            if l >= reste && l > 0.0 {
                return w[0].lerp(w[1], reste / l);
            }
            reste -= l;
        }
        pts.last().copied().unwrap_or(self.start)
    }

    /// The way the curve goes as it reaches its end (`at_end`) or leaves its start,
    /// pointing out of the curve, one unit long.
    pub fn direction(&self, at_end: bool) -> Pt {
        let pts = self.flatten();
        let (tip, others): (Pt, Vec<Pt>) = if at_end {
            (
                *pts.last().unwrap_or(&self.start),
                pts.iter().rev().skip(1).copied().collect(),
            )
        } else {
            (self.start, pts.iter().skip(1).copied().collect())
        };
        others
            .iter()
            .find(|p| p.distance(tip) > 0.5)
            .map_or(Pt::new(1.0, 0.0), |p| tip.sub(*p).unit())
    }

    /// Path commands, with `o` as the origin.
    pub fn commands(&self, o: Pt) -> String {
        let mut s = String::new();
        let p = |q: Pt| (q.x - o.x, q.y - o.y);
        let (x, y) = p(self.start);
        let _ = write!(s, "M {} {}", n(x), n(y));
        for seg in &self.segs {
            match *seg {
                Seg::Line(a) => {
                    let (x, y) = p(a);
                    let _ = write!(s, " L {} {}", n(x), n(y));
                }
                Seg::Quad(c, a) => {
                    let (cx, cy) = p(c);
                    let (x, y) = p(a);
                    let _ = write!(s, " Q {} {} {} {}", n(cx), n(cy), n(x), n(y));
                }
                Seg::Cubic(c1, c2, a) => {
                    let (ax, ay) = p(c1);
                    let (bx, by) = p(c2);
                    let (x, y) = p(a);
                    let _ = write!(
                        s,
                        " C {} {} {} {} {} {}",
                        n(ax),
                        n(ay),
                        n(bx),
                        n(by),
                        n(x),
                        n(y)
                    );
                }
            }
        }
        s
    }
}

/// A number as path commands want it: two decimals at most.
pub fn n(v: f32) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// How far `p` is from the segment `a`–`b`.
pub fn segment_distance(a: Pt, b: Pt, p: Pt) -> f32 {
    let ab = b.sub(a);
    let l2 = ab.x * ab.x + ab.y * ab.y;
    if l2 < 1e-9 {
        return a.distance(p);
    }
    let t = (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / l2).clamp(0.0, 1.0);
    a.lerp(b, t).distance(p)
}

pub fn polyline_distance(points: &[Pt], p: Pt) -> f32 {
    match points {
        [] => f32::INFINITY,
        [a] => a.distance(p),
        _ => points
            .windows(2)
            .map(|w| segment_distance(w[0], w[1], p))
            .fold(f32::INFINITY, f32::min),
    }
}

/// Whether `p` lies in the shape drawn in box `r`.
pub fn in_shape(shape: Shape, r: Rect, p: Pt) -> bool {
    if !r.contains(p) {
        return false;
    }
    let c = r.center();
    let (rx, ry) = (r.w / 2.0, r.h / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        return true;
    }
    let (dx, dy) = ((p.x - c.x) / rx, (p.y - c.y) / ry);
    match shape {
        Shape::Ellipse => dx * dx + dy * dy <= 1.0,
        Shape::Diamond => dx.abs() + dy.abs() <= 1.0,
        _ => true,
    }
}

/// Where the line from the centre of the shape in `r` through `p` crosses its outline.
pub fn outline_toward(shape: Shape, kind: Kind, r: Rect, p: Pt) -> Pt {
    let c = r.center();
    let d = p.sub(c);
    let (rx, ry) = (r.w / 2.0, r.h / 2.0);
    if d.length() < 1e-3 || rx <= 0.0 || ry <= 0.0 {
        return Pt::new(c.x, r.y);
    }
    let (ax, ay) = (d.x.abs(), d.y.abs());
    let t = match (kind, shape) {
        (Kind::Shape, Shape::Ellipse) => 1.0 / ((d.x / rx).powi(2) + (d.y / ry).powi(2)).sqrt(),
        (Kind::Shape, Shape::Diamond) => 1.0 / (ax / rx + ay / ry),
        _ => {
            let tx = if ax > 1e-6 { rx / ax } else { f32::INFINITY };
            let ty = if ay > 1e-6 { ry / ay } else { f32::INFINITY };
            tx.min(ty)
        }
    };
    c.add(d.scale(t))
}

/// A box's corners drawn round by this much (board units).
pub fn radius(shape: Shape, w: f32, h: f32) -> f32 {
    match shape {
        Shape::Rounded => (w.min(h) * 0.2).min(12.0),
        _ => 0.0,
    }
}

/// The outline of a shape in a `w` × `h` box, from its top left corner; nothing for
/// the boxes the window draws itself (square and rounded).
pub fn shape_commands(shape: Shape, w: f32, h: f32) -> String {
    let (w2, h2) = (w / 2.0, h / 2.0);
    let poly = |pts: &[(f32, f32)]| -> String {
        let mut s = String::new();
        for (i, (x, y)) in pts.iter().enumerate() {
            let _ = write!(s, "{} {} {} ", if i == 0 { "M" } else { "L" }, n(*x), n(*y));
        }
        s.push('Z');
        s
    };
    match shape {
        Shape::Rect | Shape::Rounded => String::new(),
        Shape::Ellipse => format!(
            "M 0 {h2} A {w2} {h2} 0 1 0 {w} {h2} A {w2} {h2} 0 1 0 0 {h2} Z",
            h2 = n(h2),
            w2 = n(w2),
            w = n(w)
        ),
        Shape::Diamond => poly(&[(w2, 0.0), (w, h2), (w2, h), (0.0, h2)]),
        Shape::Triangle => poly(&[(w2, 0.0), (w, h), (0.0, h)]),
        Shape::Parallelogram => {
            let s = (w * 0.2).min(h * 0.6);
            poly(&[(s, 0.0), (w, 0.0), (w - s, h), (0.0, h)])
        }
        Shape::Hexagon => {
            let s = (w * 0.25).min(h * 0.5);
            poly(&[
                (s, 0.0),
                (w - s, 0.0),
                (w, h2),
                (w - s, h),
                (s, h),
                (0.0, h2),
            ])
        }
        Shape::Star => {
            let mut pts = Vec::new();
            for i in 0..10 {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
                let k = if i % 2 == 0 { 1.0 } else { 0.42 };
                pts.push((w2 + a.cos() * w2 * k, h2 * 1.1 + a.sin() * h2 * 1.1 * k));
            }
            // Fitted to the box: the star's points reach its top and its bottom.
            let (y0, y1) = pts
                .iter()
                .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
            let k = if y1 > y0 { h / (y1 - y0) } else { 1.0 };
            let pts: Vec<(f32, f32)> = pts.iter().map(|(x, y)| (*x, (y - y0) * k)).collect();
            poly(&pts)
        }
    }
}

/// A pen stroke drawn smooth through its points, with `o` as the origin.
pub fn ink_commands(points: &[Pt], o: Pt) -> String {
    let p = |q: Pt| (n(q.x - o.x), n(q.y - o.y));
    match points {
        [] => String::new(),
        [a] => {
            let (x, y) = p(*a);
            format!("M {x} {y} L {x} {y}")
        }
        [a, b] => {
            let (x, y) = p(*a);
            let (x2, y2) = p(*b);
            format!("M {x} {y} L {x2} {y2}")
        }
        _ => {
            let (x, y) = p(points[0]);
            let mut s = format!("M {x} {y}");
            for w in points.windows(2).skip(1) {
                let m = w[0].lerp(w[1], 0.5);
                let (cx, cy) = p(w[0]);
                let (mx, my) = p(m);
                let _ = write!(s, " Q {cx} {cy} {mx} {my}");
            }
            let (x, y) = p(*points.last().unwrap_or(&points[0]));
            let _ = write!(s, " L {x} {y}");
            s
        }
    }
}

/// A stroke's points thinned: each at least `min` from the one kept before it.
pub fn thin(points: &[Pt], min: f32) -> Vec<Pt> {
    let mut out: Vec<Pt> = Vec::new();
    for (i, p) in points.iter().enumerate() {
        let last = i + 1 == points.len();
        match out.last() {
            Some(q) if q.distance(*p) < min && !last => {}
            _ => out.push(*p),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_short() {
        assert_eq!(n(3.0), "3");
        assert_eq!(n(2.456), "2.46");
        assert_eq!(n(-0.5), "-0.5");
    }

    #[test]
    fn a_curve_writes_its_commands_from_an_origin() {
        let c = Curve::polyline(&[Pt::new(10.0, 10.0), Pt::new(20.0, 10.0)]);
        assert_eq!(c.commands(Pt::new(10.0, 0.0)), "M 0 10 L 10 10");
        assert_eq!(c.midpoint(), Pt::new(15.0, 10.0));
        assert_eq!(c.direction(true), Pt::new(1.0, 0.0));
        assert_eq!(c.direction(false), Pt::new(-1.0, 0.0));
    }

    #[test]
    fn the_outline_is_reached_from_the_centre() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let q = outline_toward(Shape::Rect, Kind::Shape, r, Pt::new(90.0, 25.0));
        assert_eq!(q, Pt::new(100.0, 25.0));
        let q = outline_toward(Shape::Ellipse, Kind::Shape, r, Pt::new(50.0, 40.0));
        assert!((q.y - 50.0).abs() < 0.01);
        let q = outline_toward(Shape::Diamond, Kind::Shape, r, Pt::new(60.0, 25.0));
        assert!((q.x - 100.0).abs() < 0.01);
    }

    #[test]
    fn shapes_have_outlines_and_inside() {
        assert!(shape_commands(Shape::Rect, 10.0, 10.0).is_empty());
        assert!(shape_commands(Shape::Diamond, 10.0, 10.0).starts_with("M 5 0"));
        assert!(shape_commands(Shape::Star, 100.0, 100.0).ends_with('Z'));
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!(in_shape(Shape::Ellipse, r, Pt::new(50.0, 50.0)));
        assert!(!in_shape(Shape::Ellipse, r, Pt::new(2.0, 2.0)));
        assert!(in_shape(Shape::Rect, r, Pt::new(2.0, 2.0)));
    }

    #[test]
    fn a_stroke_is_thinned_but_keeps_its_ends() {
        let pts = [
            Pt::new(0.0, 0.0),
            Pt::new(0.5, 0.0),
            Pt::new(3.0, 0.0),
            Pt::new(3.2, 0.0),
        ];
        assert_eq!(thin(&pts, 2.0), vec![pts[0], pts[2], pts[3]]);
    }
}
