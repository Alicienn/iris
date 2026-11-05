//! A chart laid out: the shapes it is drawn with (paths, in the SVG way) and its words,
//! placed in a box `WIDTH` wide and as tall as the chart needs. A window scales the
//! whole box to the room it has; colours of the theme (the grid, the words) are named,
//! not given, so the same drawing reads in light and dark.

use crate::chart::{Kind, Series};
use crate::eval::Value;
use crate::format::{display, Format, Locale};
use std::fmt::Write;

/// The box's width; its height follows the chart.
pub const WIDTH: f32 = 680.0;
/// The words' size, in the box.
const TEXTE: f32 = 12.0;

/// The series' colours when their cells give none: they read on light and dark alike.
pub const PALETTE: [u32; 10] = [
    0x5b8def, 0x4cb782, 0xf2994a, 0x9b6ef3, 0xeb5757, 0x2bb5c4, 0xe2b93b, 0xe86fb0, 0x7c8a9e,
    0xa3785a,
];

/// What a shape is painted with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Paint {
    /// A series' colour (`0xrrggbb`) and how opaque.
    Colour(u32, f32),
    /// The theme's grid lines.
    Grid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// SVG path commands, in the box.
    pub path: String,
    pub paint: Paint,
    /// A line this wide; 0: the shape is filled.
    pub stroke: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Words placed in a box `width` wide whose middle line is `y`.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub text: String,
    pub align: Align,
    pub size: f32,
    pub strong: bool,
    /// In the theme's quieter colour.
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plot {
    pub width: f32,
    pub height: f32,
    pub shapes: Vec<Shape>,
    pub labels: Vec<Label>,
    /// Nothing to draw: why.
    pub message: String,
}

impl Plot {
    fn new(height: f32) -> Self {
        Plot {
            width: WIDTH,
            height,
            ..Default::default()
        }
    }

    fn fill(&mut self, path: String, c: u32, alpha: f32) {
        self.shapes.push(Shape {
            path,
            paint: Paint::Colour(c, alpha),
            stroke: 0.0,
        });
    }

    fn grid(&mut self, path: String) {
        self.shapes.push(Shape {
            path,
            paint: Paint::Grid,
            stroke: 0.0,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        x: f32,
        y: f32,
        width: f32,
        text: &str,
        align: Align,
        strong: bool,
        muted: bool,
    ) {
        self.labels.push(Label {
            x,
            y,
            width: width.max(0.0),
            text: text.to_string(),
            align,
            size: TEXTE,
            strong,
            muted,
        });
    }
}

/// About how wide `t` is at `size` (the words are cut where they do not fit anyway).
fn largeur(t: &str, size: f32) -> f32 {
    t.chars().count() as f32 * size * 0.56
}

fn colour_of(own: Option<u32>, k: usize) -> u32 {
    own.unwrap_or(PALETTE[k % PALETTE.len()])
}

/// A rectangle with rounded corners.
fn rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> String {
    let (w, h) = (w.max(0.0), h.max(0.0));
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.01 {
        return format!("M {x:.2} {y:.2} H {:.2} V {:.2} H {x:.2} Z", x + w, y + h);
    }
    format!(
        "M {:.2} {y:.2} H {:.2} A {r:.2} {r:.2} 0 0 1 {:.2} {:.2} V {:.2} A {r:.2} {r:.2} 0 0 1 {:.2} {:.2} H {:.2} A {r:.2} {r:.2} 0 0 1 {x:.2} {:.2} V {:.2} A {r:.2} {r:.2} 0 0 1 {:.2} {y:.2} Z",
        x + r,
        x + w - r,
        x + w,
        y + r,
        y + h - r,
        x + w - r,
        y + h,
        x + r,
        y + h - r,
        y + r,
        x + r,
    )
}

/// A number as its column shows them.
fn nombre(v: f64, f: &Format, l: &Locale) -> String {
    display(&Value::Number(v), f, l)
}

/// Round steps for an axis from `min` to `max`: about five of them.
fn graduations(min: f64, max: f64) -> (f64, f64, f64) {
    let etendue = (max - min).max(1e-9);
    let brut = etendue / 5.0;
    let puissance = 10f64.powi(brut.log10().floor() as i32);
    let pas = [1.0, 2.0, 2.5, 5.0, 10.0]
        .into_iter()
        .map(|m| m * puissance)
        .find(|p| *p >= brut)
        .unwrap_or(10.0 * puissance);
    let bas = (min / pas).floor() * pas;
    let haut = (max / pas).ceil() * pas;
    (bas, haut.max(bas + pas), pas)
}

/// `series` drawn as a chart of `kind`.
pub fn plot(kind: Kind, series: &Series, l: &Locale) -> Plot {
    match series {
        Series::Empty(pourquoi) => Plot {
            width: WIDTH,
            height: 120.0,
            message: pourquoi.clone(),
            ..Default::default()
        },
        Series::Flow {
            nodes,
            links,
            format,
        } => flux(nodes, links, format, l),
        Series::Categories {
            names,
            items,
            format,
        } => {
            if items.is_empty() {
                return Plot {
                    width: WIDTH,
                    height: 120.0,
                    message: "Nothing under the headers yet.".into(),
                    ..Default::default()
                };
            }
            match kind {
                Kind::Donut => anneau(items, format, l),
                Kind::Columns | Kind::Line => axes(kind, names, items, format, l),
                _ => barres(items, format, l),
            }
        }
    }
}

// --- A flow ----------------------------------------------------------------------------

fn flux(
    nodes: &[crate::chart::Node],
    links: &[crate::chart::Link],
    f: &Format,
    l: &Locale,
) -> Plot {
    const NOEUD: f32 = 10.0;
    const ECART: f32 = 12.0;
    let profondeurs = nodes.iter().map(|n| n.depth).max().unwrap_or(0) + 1;
    // Each column's steps, largest first ("Other" last).
    let mut colonnes: Vec<Vec<usize>> = vec![Vec::new(); profondeurs];
    for (k, n) in nodes.iter().enumerate() {
        colonnes[n.depth].push(k);
    }
    for c in &mut colonnes {
        c.sort_by(|&a, &b| {
            (nodes[a].label == "Other")
                .cmp(&(nodes[b].label == "Other"))
                .then(nodes[b].value.total_cmp(&nodes[a].value))
        });
    }
    let le_plus = colonnes.iter().map(Vec::len).max().unwrap_or(1);
    let hauteur = (le_plus as f32 * 40.0).clamp(240.0, 560.0);
    let mut p = Plot::new(hauteur);
    let etiquette = |k: usize| format!("{}  {}", nodes[k].label, nombre(nodes[k].value, f, l));
    // The last column's words need room on its right.
    let droite = colonnes
        .last()
        .map(|c| {
            c.iter()
                .map(|&k| largeur(&etiquette(k), TEXTE))
                .fold(0.0, f32::max)
        })
        .unwrap_or(0.0)
        .clamp(60.0, WIDTH * 0.3)
        + 10.0;
    let pas_x = if profondeurs > 1 {
        (WIDTH - droite - NOEUD) / (profondeurs - 1) as f32
    } else {
        0.0
    };
    // One scale for every column, the fullest fitting.
    let echelle = colonnes
        .iter()
        .filter(|c| !c.is_empty())
        .map(|c| {
            let total: f64 = c.iter().map(|&k| nodes[k].value).sum();
            (hauteur - 8.0 - (c.len() as f32 - 1.0) * ECART) / (total.max(1e-9) as f32)
        })
        .fold(f32::INFINITY, f32::min);
    let echelle = if echelle.is_finite() {
        echelle.max(0.0)
    } else {
        0.0
    };
    // Where each step stands: x, top, height.
    let mut place = vec![(0.0f32, 0.0f32, 0.0f32); nodes.len()];
    for (d, c) in colonnes.iter().enumerate() {
        let total: f32 = c
            .iter()
            .map(|&k| (nodes[k].value as f32 * echelle).max(1.5))
            .sum::<f32>()
            + (c.len() as f32 - 1.0).max(0.0) * ECART;
        let mut y = (hauteur - total) / 2.0;
        for &k in c {
            let h = (nodes[k].value as f32 * echelle).max(1.5);
            place[k] = (d as f32 * pas_x, y, h);
            y += h + ECART;
        }
    }
    // A label takes one colour everywhere it stands.
    let mut teintes: Vec<String> = Vec::new();
    let teinte = |k: usize, teintes: &mut Vec<String>| -> u32 {
        let i = teintes
            .iter()
            .position(|t| *t == nodes[k].label)
            .unwrap_or_else(|| {
                teintes.push(nodes[k].label.clone());
                teintes.len() - 1
            });
        colour_of(nodes[k].colour, i)
    };
    let couleurs: Vec<u32> = (0..nodes.len()).map(|k| teinte(k, &mut teintes)).collect();
    // The bands: leaving each step in the order they arrive, arriving in the order they
    // leave, so they never cross at a step.
    let mut depart = vec![0.0f32; nodes.len()];
    let mut arrivee = vec![0.0f32; nodes.len()];
    let mut ordre: Vec<usize> = (0..links.len()).collect();
    ordre.sort_by(|&a, &b| {
        let (la, lb) = (&links[a], &links[b]);
        place[la.from]
            .1
            .total_cmp(&place[lb.from].1)
            .then(place[la.to].1.total_cmp(&place[lb.to].1))
    });
    let mut bandes = Vec::new();
    for &i in &ordre {
        let lk = &links[i];
        let t = lk.value as f32 * echelle;
        let (_, y0, _) = place[lk.from];
        let s0 = y0 + depart[lk.from];
        depart[lk.from] += t;
        bandes.push((i, s0, t));
    }
    let mut ordre_arrivee = bandes.clone();
    ordre_arrivee.sort_by(|a, b| {
        let (la, lb) = (&links[a.0], &links[b.0]);
        place[la.to]
            .1
            .total_cmp(&place[lb.to].1)
            .then(a.1.total_cmp(&b.1))
    });
    for (i, s0, t) in ordre_arrivee {
        let lk = &links[i];
        let (x0, _, _) = place[lk.from];
        let (x1, y1, _) = place[lk.to];
        let d0 = y1 + arrivee[lk.to];
        arrivee[lk.to] += t;
        let (a, b) = (x0 + NOEUD, x1);
        let m = (a + b) / 2.0;
        let mut chemin = String::new();
        let _ = write!(
            chemin,
            "M {a:.2} {s0:.2} C {m:.2} {s0:.2} {m:.2} {d0:.2} {b:.2} {d0:.2} L {b:.2} {:.2} C {m:.2} {:.2} {m:.2} {:.2} {a:.2} {:.2} Z",
            d0 + t,
            d0 + t,
            s0 + t,
            s0 + t
        );
        p.fill(chemin, couleurs[lk.to], 0.32);
    }
    // The steps over the bands, and their words.
    for (k, &(x, y, h)) in place.iter().enumerate() {
        p.fill(rect(x, y, NOEUD, h, 2.0), couleurs[k], 1.0);
        let d = nodes[k].depth;
        let mot_x = x + NOEUD + 6.0;
        let place_mots = if d + 1 < profondeurs {
            pas_x - NOEUD - 12.0
        } else {
            WIDTH - mot_x
        };
        let milieu = y + h / 2.0;
        if h >= 30.0 {
            p.text(
                mot_x,
                milieu - 8.0,
                place_mots,
                &nodes[k].label,
                Align::Left,
                true,
                false,
            );
            p.text(
                mot_x,
                milieu + 8.0,
                place_mots,
                &nombre(nodes[k].value, f, l),
                Align::Left,
                false,
                true,
            );
        } else {
            p.text(
                mot_x,
                milieu,
                place_mots,
                &etiquette(k),
                Align::Left,
                false,
                false,
            );
        }
    }
    p
}

// --- Bars ------------------------------------------------------------------------------

fn barres(items: &[crate::chart::Category], f: &Format, l: &Locale) -> Plot {
    const LIGNE: f32 = 30.0;
    const BARRE: f32 = 18.0;
    let gauche = items
        .iter()
        .map(|c| largeur(&c.label, TEXTE))
        .fold(0.0, f32::max)
        .clamp(40.0, WIDTH * 0.32)
        + 12.0;
    let valeurs: Vec<String> = items.iter().map(|c| nombre(c.values[0], f, l)).collect();
    let droite = valeurs
        .iter()
        .map(|v| largeur(v, TEXTE))
        .fold(0.0, f32::max)
        + 14.0;
    let max = items
        .iter()
        .map(|c| c.values[0])
        .fold(0.0, f64::max)
        .max(1e-9);
    let mut p = Plot::new(items.len() as f32 * LIGNE + 8.0);
    let place = WIDTH - gauche - droite;
    for (k, c) in items.iter().enumerate() {
        let milieu = 4.0 + k as f32 * LIGNE + LIGNE / 2.0;
        let w = (c.values[0].max(0.0) / max) as f32 * place;
        // A track under the bar, and the bar.
        p.grid(rect(gauche, milieu - BARRE / 2.0, place, BARRE, 4.0));
        if w > 0.5 {
            p.fill(
                rect(gauche, milieu - BARRE / 2.0, w.max(4.0), BARRE, 4.0),
                colour_of(c.colour, k),
                1.0,
            );
        }
        p.text(
            0.0,
            milieu,
            gauche - 12.0,
            &c.label,
            Align::Right,
            false,
            false,
        );
        p.text(
            gauche + w + 6.0,
            milieu,
            droite,
            &valeurs[k],
            Align::Left,
            true,
            false,
        );
    }
    p
}

// --- A donut ---------------------------------------------------------------------------

fn anneau(items: &[crate::chart::Category], f: &Format, l: &Locale) -> Plot {
    let total: f64 = items.iter().map(|c| c.values[0].max(0.0)).sum();
    let hauteur = (items.len() as f32 * 26.0 + 24.0).max(240.0);
    let mut p = Plot::new(hauteur);
    let r = 104.0f32;
    let ri = r * 0.62;
    let (cx, cy) = (r + 12.0, hauteur / 2.0);
    let point = |rayon: f32, a: f32| (cx + rayon * a.cos(), cy + rayon * a.sin());
    let mut a = -std::f32::consts::FRAC_PI_2;
    for (k, c) in items.iter().enumerate() {
        let part = if total > 0.0 {
            (c.values[0].max(0.0) / total) as f32
        } else {
            0.0
        };
        if part <= 0.0 {
            continue;
        }
        // A whole ring is two halves: an arc cannot end where it starts.
        let morceaux = if part > 0.999 { 2 } else { 1 };
        let pas = part * std::f32::consts::TAU / morceaux as f32;
        for _ in 0..morceaux {
            let b = a + pas;
            let grand = u8::from(pas > std::f32::consts::PI);
            let (x0, y0) = point(r, a);
            let (x1, y1) = point(r, b);
            let (x2, y2) = point(ri, b);
            let (x3, y3) = point(ri, a);
            p.fill(
                format!(
                    "M {x0:.2} {y0:.2} A {r:.2} {r:.2} 0 {grand} 1 {x1:.2} {y1:.2} L {x2:.2} {y2:.2} A {ri:.2} {ri:.2} 0 {grand} 0 {x3:.2} {y3:.2} Z"
                ),
                colour_of(c.colour, k),
                1.0,
            );
            a = b;
        }
    }
    p.text(
        cx - ri,
        cy - 8.0,
        ri * 2.0,
        &nombre(total, f, l),
        Align::Center,
        true,
        false,
    );
    p.labels.last_mut().unwrap().size = 20.0;
    p.text(
        cx - ri,
        cy + 14.0,
        ri * 2.0,
        "Total",
        Align::Center,
        false,
        true,
    );
    // The legend: a swatch, the name, the value and its share.
    let gauche = cx + r + 36.0;
    let haut = cy - items.len() as f32 * 26.0 / 2.0;
    for (k, c) in items.iter().enumerate() {
        let milieu = haut + k as f32 * 26.0 + 13.0;
        p.fill(
            rect(gauche, milieu - 5.0, 10.0, 10.0, 3.0),
            colour_of(c.colour, k),
            1.0,
        );
        let part = if total > 0.0 {
            c.values[0] / total * 100.0
        } else {
            0.0
        };
        let chiffres = format!("{}  ·  {:.0} %", nombre(c.values[0], f, l), part);
        let w = largeur(&chiffres, TEXTE) + 8.0;
        p.text(
            gauche + 18.0,
            milieu,
            WIDTH - gauche - 18.0 - w,
            &c.label,
            Align::Left,
            false,
            false,
        );
        p.text(WIDTH - w, milieu, w, &chiffres, Align::Right, false, true);
    }
    p
}

// --- Columns and lines -----------------------------------------------------------------

fn axes(
    kind: Kind,
    names: &[String],
    items: &[crate::chart::Category],
    f: &Format,
    l: &Locale,
) -> Plot {
    let hauteur = 300.0;
    let mut p = Plot::new(hauteur);
    let series = names.len().max(1);
    let tout = items.iter().flat_map(|c| c.values.iter().copied());
    let (min, max) = tout.fold((0.0f64, 0.0f64), |(a, b), v| (a.min(v), b.max(v)));
    let (bas, haut, pas) = graduations(min, if max <= min { min + 1.0 } else { max });
    // A legend above when there are several series.
    let legende = if series > 1 { 26.0 } else { 0.0 };
    if series > 1 {
        let mut x = 0.0;
        for (s, n) in names.iter().enumerate() {
            p.fill(
                rect(x, 8.0, 10.0, 10.0, 3.0),
                PALETTE[s % PALETTE.len()],
                1.0,
            );
            let w = largeur(n, TEXTE) + 8.0;
            p.text(x + 16.0, 13.0, w, n, Align::Left, false, false);
            x += w + 30.0;
        }
    }
    let graduees: Vec<f64> = {
        let mut v = Vec::new();
        let mut t = bas;
        while t <= haut + pas * 0.01 && v.len() < 12 {
            v.push(t);
            t += pas;
        }
        v
    };
    let textes: Vec<String> = graduees.iter().map(|t| nombre(*t, f, l)).collect();
    let gauche = textes.iter().map(|t| largeur(t, TEXTE)).fold(0.0, f32::max) + 12.0;
    let (top, bottom) = (legende + 10.0, hauteur - 26.0);
    let y = |v: f64| bottom - ((v - bas) / (haut - bas)) as f32 * (bottom - top);
    for (t, texte) in graduees.iter().zip(&textes) {
        let yy = y(*t);
        p.grid(rect(gauche, yy - 0.5, WIDTH - gauche, 1.0, 0.0));
        p.text(0.0, yy, gauche - 8.0, texte, Align::Right, false, true);
    }
    let n = items.len();
    let bande = (WIDTH - gauche) / n as f32;
    // The categories' names under the axis, one in so many when they are too many.
    let tous = ((largeur("00/00/0000", TEXTE) + 8.0) / bande)
        .ceil()
        .max(1.0) as usize;
    for (k, c) in items.iter().enumerate() {
        if k % tous == 0 {
            p.text(
                gauche + k as f32 * bande - bande * (tous as f32 - 1.0) / 2.0,
                hauteur - 12.0,
                bande * tous as f32,
                &c.label,
                Align::Center,
                false,
                true,
            );
        }
    }
    let zero = y(0.0f64.clamp(bas, haut));
    if kind == Kind::Columns {
        let groupe = bande * 0.72;
        let w = groupe / series as f32;
        let ecrire = n * series <= 16;
        for (k, c) in items.iter().enumerate() {
            for s in 0..series {
                let v = c.values.get(s).copied().unwrap_or(0.0);
                let x = gauche + k as f32 * bande + (bande - groupe) / 2.0 + s as f32 * w;
                let (y0, y1) = (zero.min(y(v)), zero.max(y(v)));
                let teinte = if series == 1 {
                    colour_of(c.colour, 0)
                } else {
                    PALETTE[s % PALETTE.len()]
                };
                p.fill(
                    rect(x + 1.0, y0, w - 2.0, (y1 - y0).max(1.0), 3.0),
                    teinte,
                    1.0,
                );
                if ecrire {
                    p.text(
                        x - 10.0,
                        y0 - 9.0,
                        w + 20.0,
                        &nombre(v, f, l),
                        Align::Center,
                        false,
                        true,
                    );
                }
            }
        }
    } else {
        let px = |k: usize| gauche + k as f32 * bande + bande / 2.0;
        for s in 0..series {
            let teinte = PALETTE[s % PALETTE.len()];
            let mut chemin = String::new();
            for (k, c) in items.iter().enumerate() {
                let v = c.values.get(s).copied().unwrap_or(0.0);
                let _ = write!(
                    chemin,
                    "{} {:.2} {:.2} ",
                    if k == 0 { "M" } else { "L" },
                    px(k),
                    y(v)
                );
            }
            // One series: the area under it, faint.
            if series == 1 && n > 1 {
                let aire = format!(
                    "{chemin}L {:.2} {zero:.2} L {:.2} {zero:.2} Z",
                    px(n - 1),
                    px(0)
                );
                p.fill(aire, teinte, 0.12);
            }
            p.shapes.push(Shape {
                path: chemin,
                paint: Paint::Colour(teinte, 1.0),
                stroke: 2.2,
            });
            if n <= 40 {
                for (k, c) in items.iter().enumerate() {
                    let v = c.values.get(s).copied().unwrap_or(0.0);
                    let (cx, cy) = (px(k), y(v));
                    p.fill(
                        format!("M {:.2} {cy:.2} A 3.5 3.5 0 1 0 {:.2} {cy:.2} A 3.5 3.5 0 1 0 {:.2} {cy:.2} Z", cx - 3.5, cx + 3.5, cx - 3.5),
                        teinte,
                        1.0,
                    );
                }
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::{Category, Link, Node};

    fn cat(label: &str, v: f64) -> Category {
        Category {
            label: label.into(),
            values: vec![v],
            colour: None,
        }
    }

    #[test]
    fn round_steps_for_an_axis() {
        assert_eq!(graduations(0.0, 9.0), (0.0, 10.0, 2.0));
        assert_eq!(graduations(0.0, 230.0), (0.0, 250.0, 50.0));
        let (bas, haut, _) = graduations(-3.0, 7.0);
        assert!(bas <= -3.0 && haut >= 7.0);
    }

    #[test]
    fn a_flow_s_bands_fill_its_steps() {
        let nodes = vec![
            Node {
                label: "All".into(),
                depth: 0,
                value: 10.0,
                colour: None,
            },
            Node {
                label: "Sent".into(),
                depth: 1,
                value: 7.0,
                colour: Some(0xe8710a),
            },
            Node {
                label: "Refused".into(),
                depth: 1,
                value: 3.0,
                colour: None,
            },
        ];
        let links = vec![
            Link {
                from: 0,
                to: 1,
                value: 7.0,
            },
            Link {
                from: 0,
                to: 2,
                value: 3.0,
            },
        ];
        let p = flux(&nodes, &links, &Format::default(), &Locale::english());
        // Two bands, three steps, and the steps' words.
        assert_eq!(p.shapes.len(), 5);
        assert!(p
            .labels
            .iter()
            .any(|l| l.text == "Sent" || l.text.starts_with("Sent ")));
        // A step keeps its cells' colour, and its band arrives in it.
        assert!(p
            .shapes
            .iter()
            .any(|s| s.paint == Paint::Colour(0xe8710a, 1.0)));
        assert!(p
            .shapes
            .iter()
            .any(|s| s.paint == Paint::Colour(0xe8710a, 0.32)));
        assert!(p.height >= 240.0 && p.width == WIDTH);
    }

    #[test]
    fn every_kind_draws_categories() {
        let items = vec![cat("Paris", 3.0), cat("Lyon", 1.0)];
        let s = Series::Categories {
            names: vec!["Count".into()],
            items,
            format: Format::default(),
        };
        for k in [Kind::Bars, Kind::Columns, Kind::Donut, Kind::Line] {
            let p = plot(k, &s, &Locale::english());
            assert!(p.message.is_empty(), "{k:?}");
            assert!(!p.shapes.is_empty(), "{k:?}");
            assert!(p.labels.iter().any(|l| l.text.contains("Paris")), "{k:?}");
        }
        // A donut's middle says the total.
        let p = plot(Kind::Donut, &s, &Locale::english());
        assert!(p.labels.iter().any(|l| l.text == "4"));
    }

    #[test]
    fn nothing_to_draw_says_why() {
        let p = plot(
            Kind::Bars,
            &Series::Empty("Pick a column of categories.".into()),
            &Locale::english(),
        );
        assert!(p.shapes.is_empty());
        assert_eq!(p.message, "Pick a column of categories.");
    }
}
