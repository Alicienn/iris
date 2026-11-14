//! Colour emojis. Slint's software renderer paints every glyph in the text's one colour,
//! so an emoji came out as a white shape. A glyph of a colour font (layers coloured by a
//! palette, `COLR`/`CPAL`, as Segoe UI Emoji's) is therefore drawn here, by swash, as a
//! picture placed where parley put the glyph.

use std::cell::RefCell;
use std::collections::HashMap;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::Format;
use swash::{FontRef, TableProvider};

/// A glyph drawn: its picture, and where its top-left is from the glyph's origin on
/// the baseline (screen pixels, down positive).
#[derive(Clone)]
pub struct Glyphe {
    pub image: slint::Image,
    pub gauche: f32,
    pub haut: f32,
    pub largeur: f32,
    pub hauteur: f32,
}

/// A glyph drawn: its font's data and index, its id, its size in quarters of a pixel.
type CleGlyphe = (u64, u32, u32, u32);

thread_local! {
    /// Whether a font (its data and index) has coloured layers.
    static COULEUR: RefCell<HashMap<(u64, u32), bool>> = RefCell::new(HashMap::new());
    /// The glyphs drawn (none: the glyph has no coloured layers).
    static GLYPHES: RefCell<HashMap<CleGlyphe, Option<Glyphe>>> = RefCell::new(HashMap::new());
    static ECHELLES: RefCell<ScaleContext> = RefCell::new(ScaleContext::new());
}

/// Whether `font` draws its glyphs in colours of its own.
pub fn en_couleur(font: &parley::FontData) -> bool {
    let cle = (font.data.id(), font.index);
    if let Some(oui) = COULEUR.with_borrow(|m| m.get(&cle).copied()) {
        return oui;
    }
    let oui = FontRef::from_index(font.data.data(), font.index as usize).is_some_and(|f| {
        f.table_by_tag(swash::tag_from_bytes(b"COLR")).is_some()
            && f.table_by_tag(swash::tag_from_bytes(b"CPAL")).is_some()
    });
    COULEUR.with_borrow_mut(|m| m.insert(cle, oui));
    oui
}

/// Glyph `id` of `font` drawn in its colours, `ppem` screen pixels to the em.
pub fn glyphe(font: &parley::FontData, id: u32, ppem: f32) -> Option<Glyphe> {
    let cle = (font.data.id(), font.index, id, (ppem * 4.0).round() as u32);
    if let Some(g) = GLYPHES.with_borrow(|m| m.get(&cle).cloned()) {
        return g;
    }
    let g = dessiner(font, id, ppem);
    GLYPHES.with_borrow_mut(|m| {
        if m.len() >= 512 {
            m.clear();
        }
        m.insert(cle, g.clone());
    });
    g
}

fn dessiner(font: &parley::FontData, id: u32, ppem: f32) -> Option<Glyphe> {
    let f = FontRef::from_index(font.data.data(), font.index as usize)?;
    let id = u16::try_from(id).ok()?;
    let image = ECHELLES.with_borrow_mut(|cx| {
        let mut s = cx.builder(f).size(ppem.max(1.0)).hint(false).build();
        Render::new(&[Source::ColorOutline(0)])
            .format(Format::Alpha)
            .render(&mut s, id)
    })?;
    let p = image.placement;
    if p.width == 0 || p.height == 0 || image.data.len() < (p.width * p.height * 4) as usize {
        return None;
    }
    // The layers are blended over nothing: their colours are premultiplied.
    let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &image.data[..(p.width * p.height * 4) as usize],
        p.width,
        p.height,
    );
    Some(Glyphe {
        image: slint::Image::from_rgba8_premultiplied(pixels),
        gauche: p.left as f32,
        haut: -(p.top as f32),
        largeur: p.width as f32,
        hauteur: p.height as f32,
    })
}
