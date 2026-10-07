//! A note as a web page of its own: its formulas drawn and its pictures carried inside,
//! so the one file can be kept, sent, or printed as PDF from a browser.

use iris_notes::html::PageHooks;
use iris_notes::inline::{escape_html, HtmlHooks, Palette};
use std::path::Path;

/// Bytes as base64, for `data:` addresses.
fn base64(octets: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut sortie = String::with_capacity(octets.len().div_ceil(3) * 4);
    for bloc in octets.chunks(3) {
        let n = (u32::from(bloc[0]) << 16)
            | (u32::from(*bloc.get(1).unwrap_or(&0)) << 8)
            | u32::from(*bloc.get(2).unwrap_or(&0));
        for (k, decalage) in [18, 12, 6, 0].into_iter().enumerate() {
            if k <= bloc.len() {
                sortie.push(ALPHABET[(n >> decalage) as usize & 63] as char);
            } else {
                sortie.push('=');
            }
        }
    }
    sortie
}

/// A formula drawn, as an `<img>`; its LaTeX as code when it cannot be drawn.
fn formule(latex: &str, display: bool) -> String {
    let dessin = iris_math::render(
        latex,
        iris_math::Style {
            display,
            size: 16.0,
            scale: 2.0,
            colour: [0x1d, 0x1d, 0x1f],
        },
    );
    let png = dessin.ok().and_then(|p| {
        let image = image::RgbaImage::from_raw(p.width, p.height, p.rgba)?;
        let mut octets = std::io::Cursor::new(Vec::new());
        image.write_to(&mut octets, image::ImageFormat::Png).ok()?;
        Some((octets.into_inner(), p.width, p.height))
    });
    match png {
        // Drawn at twice the size, shown at the size of the text.
        Some((octets, w, h)) => format!(
            "<img class=\"{}\" alt=\"{}\" width=\"{}\" height=\"{}\" src=\"data:image/png;base64,{}\">",
            if display { "math" } else { "math-inline" },
            escape_html(latex),
            w / 2,
            h / 2,
            base64(&octets)
        ),
        None => format!("<code class=\"math\">{}</code>", escape_html(latex)),
    }
}

struct Page<'a> {
    dir: &'a Path,
}

impl HtmlHooks for Page<'_> {
    fn math(&self, src: &str) -> String {
        formule(src, false)
    }
}

impl PageHooks for Page<'_> {
    fn display_math(&self, latex: &str) -> String {
        format!("<p class=\"math\">{}</p>", formule(latex, true))
    }

    fn embed(&self, target: &str) -> String {
        let fichier = super::render::embedded_file(target, self.dir);
        let nom = target.split('|').next().unwrap_or(target).trim();
        match fichier {
            Some(p) if super::render::is_picture(nom) => {
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("png")
                    .to_ascii_lowercase();
                let type_mime = match ext.as_str() {
                    "jpg" | "jpeg" => "image/jpeg",
                    "svg" => "image/svg+xml",
                    "gif" => "image/gif",
                    "webp" => "image/webp",
                    "bmp" => "image/bmp",
                    _ => "image/png",
                };
                match std::fs::read(&p) {
                    Ok(octets) => format!(
                        "<p><img alt=\"{}\" src=\"data:{type_mime};base64,{}\"></p>",
                        escape_html(nom),
                        base64(&octets)
                    ),
                    Err(_) => format!("<p class=\"embed\">{}</p>", escape_html(nom)),
                }
            }
            _ => format!("<p class=\"embed\">{}</p>", escape_html(nom)),
        }
    }
}

/// The page of a note's text. `print`: the browser's print window opens with it.
pub fn page(text: &str, title: &str, dir: &Path, palette: &Palette, print: bool) -> String {
    let mut html = iris_notes::html::note_to_html(text, title, palette, &Page { dir });
    if print {
        let script = "<script>window.addEventListener('load',()=>setTimeout(()=>window.print(),300));</script>";
        match html.rfind("</body>") {
            Some(k) => html.insert_str(k, script),
            None => html.push_str(script),
        }
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_as_everyone_writes_it() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_page_for_printing_asks_to_print() {
        let dir = std::env::temp_dir();
        let p = page(
            "# Titre\n\nUn mot.\n",
            "Titre",
            &dir,
            &Palette::default(),
            true,
        );
        assert!(p.contains("window.print()"));
        assert!(p.contains("Un mot."));
        let p = page("Un mot.\n", "Titre", &dir, &Palette::default(), false);
        assert!(!p.contains("window.print()"));
    }
}
