//! A whiteboard written as SVG, to be made into a picture: every item as the board
//! draws it, in the light theme, on its paper, with a margin around everything.
//!
//! SVG has no wrapping: the words are cut into lines here, each word's width estimated
//! from its letters (narrow, wide, capitals) in the font's average proportions. It is
//! close to what the board shows, not the same to the pixel.

use crate::geom::{self, Pt, Rect};
use crate::{route, Align, Board, Colour, Font, Item, Kind};
use std::fmt::Write as _;

/// The light theme's colours, in `COLOURS`'s order.
pub const LIGHT: [u32; 22] = [
    0xfff3bf, 0xffe4cc, 0xffdcd9, 0xffd9e3, 0xeedcf8, 0xd9e9ff, 0xd4f1f6, 0xd9f5e0, 0xececef,
    0xe0a800, 0xff9500, 0xff3b30, 0xff2d55, 0xaf52de, 0x007aff, 0x30b0c7, 0x34c759, 0x8e8e93,
    0xffffff, 0x1d1d1f, 0x424245, 0xd1d1d6,
];
/// The ground around the items.
const GROUND: &str = "#fafafa";
/// Room left around everything.
pub const MARGIN: f32 = 32.0;

fn colour(s: &str) -> Option<String> {
    match Colour::parse(s) {
        Colour::None => None,
        Colour::Named(i) => LIGHT.get(i).map(|v| format!("#{v:06x}")),
        Colour::Rgb(v) => Some(format!("#{v:06x}")),
    }
}

fn family(f: Font) -> &'static str {
    match f {
        Font::Sans => "Segoe UI Variable Text, Segoe UI, Arial, sans-serif",
        Font::Display => "Segoe UI Variable Display, Segoe UI, Arial, sans-serif",
        Font::Serif => "Georgia, Times New Roman, serif",
        Font::Mono => "Cascadia Mono, Consolas, Courier New, monospace",
        Font::Hand => "Segoe Print, Comic Sans MS, cursive",
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// How wide a word is at size 1, by its letters.
fn width_of(w: &str, font: Font, bold: bool) -> f32 {
    if font == Font::Mono {
        return w.chars().count() as f32 * 0.6;
    }
    let base: f32 = w
        .chars()
        .map(|c| match c {
            'i' | 'l' | 'j' | 't' | 'f' | 'r' | '.' | ',' | ':' | ';' | '\'' | '!' | '|' => 0.30,
            'm' | 'w' | 'M' | 'W' => 0.85,
            c if c.is_uppercase() => 0.65,
            ' ' => 0.28,
            _ => 0.53,
        })
        .sum();
    let k = match font {
        Font::Hand => 1.12,
        Font::Serif => 1.02,
        _ => 1.0,
    };
    let b = if bold { 1.06 } else { 1.0 };
    base * k * b
}

/// The words cut into lines no wider than `width` at `size`; the lines typed kept.
pub fn wrap(text: &str, width: f32, size: f32, font: Font, bold: bool) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut ligne = String::new();
        for mot in para.split(' ') {
            let essai = if ligne.is_empty() {
                mot.to_string()
            } else {
                format!("{ligne} {mot}")
            };
            if !ligne.is_empty() && width_of(&essai, font, bold) * size > width {
                out.push(std::mem::take(&mut ligne));
                ligne = mot.to_string();
            } else {
                ligne = essai;
            }
        }
        out.push(ligne);
    }
    out
}

/// Words in a box, as the board lays them: wrapped, aligned across and down.
fn words(s: &mut String, it: &Item, r: Rect, centred: Option<(Pt, f32)>) {
    if it.text.trim().is_empty() {
        return;
    }
    let couleur = colour(&it.color).unwrap_or_else(|| "#1d1d1f".into());
    let poids = if it.bold {
        700
    } else if it.kind == Kind::Shape {
        500
    } else {
        400
    };
    let style = if it.italic { "italic" } else { "normal" };
    let hauteur = it.size * 1.33;
    let (lignes, x, anchor, y0) = match centred {
        // An arrow's words, on one patch of the ground at a point.
        Some((p, largeur)) => {
            let lignes = wrap(&it.text, largeur, it.size, it.font, it.bold);
            let h = hauteur * lignes.len() as f32;
            let w = lignes
                .iter()
                .map(|l| width_of(l, it.font, it.bold) * it.size)
                .fold(0.0, f32::max);
            let _ = write!(
                s,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="4" fill="{GROUND}"/>"#,
                geom::n(p.x - w / 2.0 - 5.0),
                geom::n(p.y - h / 2.0 - 1.0),
                geom::n(w + 10.0),
                geom::n(h + 2.0)
            );
            (lignes, p.x, "middle", p.y - h / 2.0)
        }
        None => {
            let lignes = wrap(&it.text, r.w, it.size, it.font, it.bold);
            let h = hauteur * lignes.len() as f32;
            let (x, anchor) = match it.align {
                Align::Left => (r.x, "start"),
                Align::Center => (r.x + r.w / 2.0, "middle"),
                Align::Right => (r.right(), "end"),
            };
            let y0 = match it.valign.resolved(it.kind) {
                0 => r.y,
                1 => r.y + (r.h - h) / 2.0,
                _ => r.bottom() - h,
            };
            (lignes, x, anchor, y0)
        }
    };
    let _ = write!(
        s,
        r#"<text font-family="{}" font-size="{}" font-weight="{poids}" font-style="{style}" fill="{couleur}" text-anchor="{anchor}">"#,
        family(it.font),
        geom::n(it.size)
    );
    for (k, l) in lignes.iter().enumerate() {
        // The baseline, about four fifths of a line down.
        let y = y0 + hauteur * k as f32 + it.size * 1.02;
        let _ = write!(
            s,
            r#"<tspan x="{}" y="{}">{}</tspan>"#,
            geom::n(x),
            geom::n(y),
            escape(l)
        );
    }
    s.push_str("</text>");
}

fn margin_of(kind: Kind) -> f32 {
    match kind {
        Kind::Sticky => 12.0,
        Kind::Text => 2.0,
        _ => 8.0,
    }
}

/// The board as an SVG document, and the box it shows (board units). Nothing on the
/// board: none.
pub fn to_svg(board: &Board) -> Option<(String, Rect)> {
    let mut boite = board.bounds()?;
    // A frame's title stands above it.
    for it in board.items.iter().filter(|i| i.kind == Kind::Frame) {
        boite = boite.union(Rect::new(it.x, it.y - 24.0, it.w, 24.0));
    }
    let boite = boite.inflate(MARGIN);
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}">"#,
        x = geom::n(boite.x),
        y = geom::n(boite.y),
        w = geom::n(boite.w),
        h = geom::n(boite.h)
    );
    let _ = write!(
        s,
        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{GROUND}"/>"#,
        geom::n(boite.x),
        geom::n(boite.y),
        geom::n(boite.w),
        geom::n(boite.h)
    );
    for it in &board.items {
        let fill = colour(&it.fill)
            .map(|c| format!(r#"fill="{c}" fill-opacity="{}""#, geom::n(it.opacity)))
            .unwrap_or_else(|| r#"fill="none""#.into());
        let stroke = colour(&it.stroke)
            .map(|c| format!(r#"stroke="{c}" stroke-width="{}""#, geom::n(it.width)))
            .unwrap_or_else(|| r#"stroke="none""#.into());
        let r = it.rect();
        match it.kind {
            Kind::Shape | Kind::Frame | Kind::Sticky | Kind::Text => {
                let path = if it.kind == Kind::Shape {
                    geom::shape_commands(it.shape, it.w, it.h)
                } else {
                    String::new()
                };
                let rayon = match it.kind {
                    Kind::Shape => geom::radius(it.shape, it.w, it.h),
                    Kind::Frame => 4.0,
                    _ => 2.0,
                };
                let trait_ = if it.kind == Kind::Sticky {
                    r#"stroke="none""#.to_string()
                } else {
                    stroke
                };
                if it.kind == Kind::Sticky {
                    let _ = write!(
                        s,
                        r##"<rect x="{}" y="{}" width="{}" height="{}" rx="2" fill="#0f0f19" fill-opacity="0.08"/>"##,
                        geom::n(r.x),
                        geom::n(r.y + 2.0),
                        geom::n(r.w),
                        geom::n(r.h)
                    );
                }
                if path.is_empty() {
                    let _ = write!(
                        s,
                        r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" {fill} {trait_}/>"#,
                        geom::n(r.x),
                        geom::n(r.y),
                        geom::n(r.w),
                        geom::n(r.h),
                        geom::n(rayon)
                    );
                } else {
                    let _ = write!(
                        s,
                        r#"<path transform="translate({} {})" d="{path}" {fill} {trait_} stroke-linejoin="round"/>"#,
                        geom::n(r.x),
                        geom::n(r.y)
                    );
                }
                if it.kind == Kind::Frame {
                    let _ = write!(
                        s,
                        r##"<text x="{}" y="{}" font-family="{}" font-size="12" font-weight="600" fill="#6e6e73">{}</text>"##,
                        geom::n(r.x),
                        geom::n(r.y - 8.0),
                        family(Font::Sans),
                        escape(&it.text)
                    );
                } else {
                    words(&mut s, it, r.inflate(-margin_of(it.kind)), None);
                }
            }
            Kind::Connector => {
                let d = route::drawn(board, it);
                let couleur = colour(&it.stroke).unwrap_or_else(|| "none".into());
                let _ = write!(
                    s,
                    r#"<path d="{}" fill="none" stroke="{couleur}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                    d.line.commands(Pt::default()),
                    geom::n(it.width)
                );
                let tetes = route::polygons(&d.heads, Pt::default());
                if !tetes.is_empty() {
                    let _ = write!(s, r#"<path d="{tetes}" fill="{couleur}"/>"#);
                }
                words(&mut s, it, r, Some((d.label, 160.0)));
            }
            Kind::Ink => {
                let couleur = colour(&it.stroke).unwrap_or_else(|| "none".into());
                let _ = write!(
                    s,
                    r#"<path d="{}" fill="none" stroke="{couleur}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                    geom::ink_commands(&it.points, Pt::default()),
                    geom::n(it.width)
                );
            }
        }
    }
    s.push_str("</svg>");
    Some((s, boite))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{End, Shape};

    #[test]
    fn words_are_cut_into_lines_that_fit() {
        let l = wrap(
            "Check pricing with sales before the beta",
            120.0,
            14.0,
            Font::Sans,
            false,
        );
        assert!(l.len() >= 2, "{l:?}");
        assert_eq!(l.join(" "), "Check pricing with sales before the beta");
        assert_eq!(wrap("a\nb", 100.0, 14.0, Font::Sans, false), vec!["a", "b"]);
    }

    #[test]
    fn a_board_is_written_with_its_items_and_a_margin() {
        assert!(to_svg(&Board::default()).is_none());
        let mut b = Board::default();
        let a = b.take_id();
        let mut it = Item::shape(a, Shape::Diamond, Rect::new(0.0, 0.0, 100.0, 60.0));
        it.text = "Go & <see>".into();
        b.items.push(it);
        let c = b.take_id();
        b.items
            .push(Item::sticky(c, Rect::new(200.0, 0.0, 160.0, 160.0)));
        let k = b.take_id();
        b.items.push(Item::connector(
            k,
            End::on(a, 1.0, 0.5),
            End::on(c, 0.0, 0.5),
        ));
        let (svg, boite) = to_svg(&b).unwrap();
        assert_eq!(boite, Rect::new(-32.0, -32.0, 424.0, 224.0));
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("Go &amp; &lt;see&gt;"));
        assert!(svg.contains("#d9e9ff") && svg.contains("#fff3bf"));
        assert_eq!(svg.matches("<path").count(), 3);
    }
}
