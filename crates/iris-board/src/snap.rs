//! Alignment while moving, resizing or drawing: the edges and the middle of what moves
//! are drawn to the edges and middles of the boxes around, when they come within a few
//! pixels, and a guide shows each line they now share. Only boxes near the view count,
//! and only the nearest line on each axis is taken, so it helps without pulling.

use crate::geom::Rect;
use crate::{Board, Kind};

/// A line two boxes now share: upright at `x = at` (`vertical`) or level at `y = at`,
/// drawn from `from` to `to` along it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    pub vertical: bool,
    pub at: f32,
    pub from: f32,
    pub to: f32,
}

/// Which lines of the moving box are drawn: its left, middle and right (or top, middle,
/// bottom) — all three when it moves, the edge pulled when it is resized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lines {
    pub start: bool,
    pub middle: bool,
    pub end: bool,
}

impl Lines {
    pub const ALL: Lines = Lines {
        start: true,
        middle: true,
        end: true,
    };
    pub const NONE: Lines = Lines {
        start: false,
        middle: false,
        end: false,
    };
    pub const START: Lines = Lines {
        start: true,
        middle: false,
        end: false,
    };
    pub const END: Lines = Lines {
        start: false,
        middle: false,
        end: true,
    };
}

/// The boxes others align to: every box, sticky note, text and frame not moving and
/// near the view.
fn targets(board: &Board, except: &[u64], near: Rect) -> Vec<Rect> {
    board
        .items
        .iter()
        .filter(|i| i.kind.boxed() || i.kind == Kind::Ink)
        .filter(|i| !except.contains(&i.id))
        .map(|i| board.bbox(i))
        .filter(|r| r.intersects(near))
        .collect()
}

/// The values of a box on one axis: start, middle, end.
fn values(r: &Rect, vertical: bool) -> [f32; 3] {
    if vertical {
        [r.x, r.x + r.w / 2.0, r.right()]
    } else {
        [r.y, r.y + r.h / 2.0, r.bottom()]
    }
}

/// The nearest shift within `tol` that puts one of the moving box's lines on one of a
/// target's, on one axis; and the guides along it.
fn axis(r: Rect, lines: Lines, cibles: &[Rect], tol: f32, vertical: bool) -> (f32, Vec<Guide>) {
    let mine = values(&r, vertical);
    let pris = [lines.start, lines.middle, lines.end];
    let mut meilleur: Option<f32> = None;
    for c in cibles {
        for v in values(c, vertical) {
            for (k, m) in mine.iter().enumerate() {
                if !pris[k] {
                    continue;
                }
                let d = v - m;
                if d.abs() <= tol && meilleur.is_none_or(|b| d.abs() < b.abs() - 1e-4) {
                    meilleur = Some(d);
                }
            }
        }
    }
    let Some(d) = meilleur else {
        return (0.0, Vec::new());
    };
    // Every line now shared, each guide running over the moving box and those it meets.
    let moved = if vertical {
        Rect::new(r.x + d, r.y, r.w, r.h)
    } else {
        Rect::new(r.x, r.y + d, r.w, r.h)
    };
    let mine = values(&moved, vertical);
    let mut guides: Vec<Guide> = Vec::new();
    for (k, m) in mine.iter().enumerate() {
        if !pris[k] {
            continue;
        }
        let (mut from, mut to) = if vertical {
            (moved.y, moved.bottom())
        } else {
            (moved.x, moved.right())
        };
        let mut touche = false;
        for c in cibles {
            if values(c, vertical).iter().any(|v| (v - m).abs() < 0.5) {
                touche = true;
                let (a, b) = if vertical {
                    (c.y, c.bottom())
                } else {
                    (c.x, c.right())
                };
                from = from.min(a);
                to = to.max(b);
            }
        }
        if touche && !guides.iter().any(|g| (g.at - m).abs() < 0.5) {
            guides.push(Guide {
                vertical,
                at: *m,
                from,
                to,
            });
        }
    }
    (d, guides)
}

/// The shift `(dx, dy)` that aligns the moving box `r` (already moved by the pointer)
/// with the boxes near the view, `except` the items moving; and the guides to show.
pub fn align(
    board: &Board,
    except: &[u64],
    r: Rect,
    across: Lines,
    down: Lines,
    tol: f32,
    near: Rect,
) -> (f32, f32, Vec<Guide>) {
    let cibles = targets(board, except, near);
    let (dx, mut g) = axis(r, across, &cibles, tol, true);
    let (dy, g2) = axis(r, down, &cibles, tol, false);
    // The guides drawn where the box ends up on both axes.
    for gd in &mut g {
        if dy != 0.0 {
            gd.from = gd.from.min(r.y + dy);
            gd.to = gd.to.max(r.bottom() + dy);
        }
    }
    g.extend(g2);
    (dx, dy, g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Item, Shape};

    fn deux() -> Board {
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
            Rect::new(300.0, 103.0, 80.0, 50.0),
        ));
        b
    }

    #[test]
    fn a_box_moved_near_anothers_top_lines_up_with_it() {
        let b = deux();
        let near = Rect::new(-1000.0, -1000.0, 3000.0, 3000.0);
        // The second box moved to y = 4: its top is drawn to the first one's.
        let r = Rect::new(300.0, 4.0, 80.0, 50.0);
        let (dx, dy, g) = align(&b, &[2], r, Lines::ALL, Lines::ALL, 6.0, near);
        assert_eq!(dx, 0.0);
        assert_eq!(dy, -4.0);
        assert!(g
            .iter()
            .any(|g| !g.vertical && g.at == 0.0 && g.from == 0.0 && g.to == 380.0));
    }

    #[test]
    fn middles_line_up_and_far_boxes_do_not_pull() {
        let b = deux();
        let near = Rect::new(-1000.0, -1000.0, 3000.0, 3000.0);
        // Its middle (x 52) near the first box's middle (x 50).
        let r = Rect::new(12.0, 200.0, 80.0, 50.0);
        let (dx, _, g) = align(&b, &[2], r, Lines::ALL, Lines::ALL, 6.0, near);
        assert_eq!(dx, -2.0);
        assert!(g.iter().any(|g| g.vertical && g.at == 50.0));
        // Out of the view: nothing.
        let loin = Rect::new(2000.0, 2000.0, 10.0, 10.0);
        let (dx, dy, g) = align(&b, &[2], r, Lines::ALL, Lines::ALL, 6.0, loin);
        assert_eq!((dx, dy), (0.0, 0.0));
        assert!(g.is_empty());
    }

    #[test]
    fn a_resized_edge_aligns_alone() {
        let b = deux();
        let near = Rect::new(-1000.0, -1000.0, 3000.0, 3000.0);
        // The right edge pulled to 97: drawn to 100.
        let r = Rect::new(20.0, 200.0, 77.0, 30.0);
        let (dx, dy, _) = align(&b, &[2], r, Lines::END, Lines::NONE, 6.0, near);
        assert_eq!((dx, dy), (3.0, 0.0));
    }
}
