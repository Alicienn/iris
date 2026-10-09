//! `iris-math` — LaTeX formulas drawn as pictures.
//!
//! `latex-rust` lays the formula out as TeX does and paints it with STIX Two Math,
//! which it carries: no JavaScript, no web view, no font to install. What comes out is
//! decoded to RGBA here, the size and colour of the text around it, and the last
//! formulas drawn are kept so a note scrolled up and down does not draw them again.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Mutex;

/// A formula drawn: its pixels, RGBA, `width * height * 4` bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// The formula's baseline, in pixels from the top: where the text around it sits.
    pub baseline: u32,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}×{})", self.width, self.height)
    }
}

/// How a formula is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// On a line of its own (`$$`), or inside a line (`$`).
    pub display: bool,
    /// The size of the text around it, in logical pixels.
    pub size: f32,
    /// The screen's pixels per logical pixel.
    pub scale: f32,
    pub colour: [u8; 3],
}

/// How many formulas are kept drawn.
const GARDEES: usize = 256;

/// A formula and how it was drawn.
type Cle = (String, bool, u32, u32, [u8; 3]);
/// The formulas drawn last.
type Gardees = Mutex<HashMap<Cle, Picture>>;

fn cache() -> &'static Gardees {
    static CACHE: std::sync::OnceLock<Gardees> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Draws a formula. An error says what LaTeX could not read.
pub fn render(latex: &str, style: Style) -> Result<Picture, String> {
    let latex = latex.trim();
    if latex.is_empty() {
        return Err("empty formula".into());
    }
    let cle: Cle = (
        latex.to_string(),
        style.display,
        (style.size * 100.0) as u32,
        (style.scale * 100.0) as u32,
        style.colour,
    );
    if let Some(p) = cache().lock().ok().and_then(|c| c.get(&cle).cloned()) {
        return Ok(p);
    }
    let image = dessiner(latex, style)?;
    if let Ok(mut c) = cache().lock() {
        if c.len() >= GARDEES {
            c.clear();
        }
        c.insert(cle, image.clone());
    }
    Ok(image)
}

fn dessiner(latex: &str, style: Style) -> Result<Picture, String> {
    use latex_rust::font::MathFont;
    use latex_rust::render::png::{PngBackground, PngOptions};
    let police = MathFont::stix_two_math().map_err(|e| format!("{e:?}"))?;
    let mut options = PngOptions::new();
    // Points at 72 per inch, the screen at 96 logical pixels per inch.
    options.font_size_pt = latex_rust::Dim::parse(&format!("{:.2}", style.size * 0.75));
    options.dpi = latex_rust::Dim::parse(&format!("{:.0}", 96.0 * style.scale.max(0.5)));
    options.color = latex_rust::Color::rgb(style.colour[0], style.colour[1], style.colour[2]);
    options.background = PngBackground::Transparent;
    options.display = style.display;
    // Laid out here rather than by `latex_to_png`, for the height above the baseline.
    let arbre = latex_rust::parse(latex)
        .map_err(|e| lisible(&format!("{e:?}")))
        .and_then(|ast| {
            latex_rust::layout(
                &ast,
                &police,
                if style.display {
                    latex_rust::MathStyle::Display
                } else {
                    latex_rust::MathStyle::Text
                },
            )
            .map_err(|e| lisible(&format!("{e:?}")))
        })?;
    let png = latex_rust::render_png(&arbre, &police, &options)
        .map_err(|e| lisible(&format!("{e:?}")))?;
    let decode = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    let (width, height) = decode.dimensions();
    // An em is the text's size in pixels (`size` × 0.75 pt at 96 × `scale` dpi).
    let em = style.size * (96.0 * style.scale.max(0.5)).round() / 96.0;
    let haut = f32::from_bits(arbre.height.to_ieee32_bits());
    let baseline = ((haut * em).round().max(0.0) as u32).min(height);
    Ok(Picture {
        width,
        height,
        rgba: decode.into_raw(),
        baseline,
    })
}

/// An error made short enough for a line under the formula.
fn lisible(e: &str) -> String {
    let premiere = e.lines().next().unwrap_or(e);
    premiere.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> Style {
        Style {
            display: true,
            size: 15.0,
            scale: 1.0,
            colour: [0, 0, 0],
        }
    }

    #[test]
    fn a_fraction_is_drawn() {
        let p = render(r"\frac{a+b}{2} = \int_0^1 f(x)\,dx", style()).unwrap();
        assert!(p.width > 20 && p.height > 10, "{p:?}");
        // A fraction stands on its bar: there is ink below the baseline.
        assert!(p.baseline > 0 && p.baseline < p.height, "{}", p.baseline);
        assert_eq!(p.rgba.len(), (p.width * p.height * 4) as usize);
        // Some pixels are ink.
        assert!(p.rgba.chunks(4).any(|px| px[3] > 0));
        // Drawn again: the same, from the cache.
        assert_eq!(
            render(r"\frac{a+b}{2} = \int_0^1 f(x)\,dx", style()).unwrap(),
            p
        );
    }

    #[test]
    fn nothing_is_not_a_formula() {
        assert!(render("  ", style()).is_err());
    }
}
