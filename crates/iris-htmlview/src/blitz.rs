//! Le moteur HTML complet, au-dessus de Blitz.
//!
//! Pour les infolettres bâties en tableaux imbriqués, où la mise en page **fait
//! partie du message** et où le texte riche ne suffit plus.
//!
//! Trois décisions, chacune prise contre ce que coûtait la précédente :
//!
//! - **peint sur le processeur**, par vello_cpu. La version d'avant ouvrait un device
//!   wgpu à elle, à côté de celui de l'interface : une seconde pile graphique pour
//!   dessiner un message à l'ouverture. Ce que le pilote réservait pour elle
//!   n'apparaissait dans aucun compteur de tas, et c'était la plus grosse part de la
//!   mémoire de l'application ouverte ;
//! - **mis en page une fois, peint par tuiles**. Le document est gardé ; une tuile de
//!   [`TILE_HEIGHT`] points n'est peinte que quand l'interface la demande, c'est-à-dire
//!   quand elle approche de l'écran. Une image de la hauteur du message coûtait, pour
//!   une infolettre de vingt mille pixels, quatre-vingts mégaoctets lus en haut ;
//! - **bornée avant d'être analysée**. Un message démesuré ou pathologique — vingt mille
//!   balises, quatre mégaoctets de HTML — n'entre pas dans le moteur : le texte riche
//!   le montre en entier, sans rien mettre en page.
//!
//! Le fond est blanc, en thème clair comme en sombre : une infolettre est dessinée pour
//! une page blanche, et la recolorer inventerait des contrastes que l'expéditeur n'a
//! jamais vus. Les messages écrits par des personnes, eux, passent par le texte riche,
//! qui suit le thème.

use crate::{HtmlRenderer, PixelSink, Rendered, TiledDocument};
use anyrender::ImageRenderer;
use anyrender_vello_cpu::{ImageCacheConfig, VelloCpuImageRenderer};
use blitz_dom::{DocumentConfig, FontContext, StyleThreading};
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use iris_types::{Error, Result};
use std::sync::{Arc, Mutex};

/// Hauteur d'une tuile, en points.
pub const TILE_HEIGHT: u32 = 512;

/// Largeurs minimale et maximale de mise en page, en points.
const MIN_WIDTH: u32 = 320;

/// La largeur sous laquelle un message n'est plus mis en page, mais réduit.
pub const MIN_READING_WIDTH: u32 = 640;
const MAX_WIDTH: u32 = 2_400;

/// Au-delà, le message n'entre pas dans le moteur.
///
/// Ce ne sont pas des limites de confort : ce sont celles au-delà desquelles un
/// document peut faire durer la mise en page des secondes, ou la mémoire des centaines
/// de mégaoctets. Aucune infolettre réelle ne les approche.
const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_TAGS: usize = 20_000;
/// Hauteur maximale d'un document mis en page, en points : six cents écrans.
const MAX_LAYOUT_HEIGHT: f32 = 400_000.0;

/// What the engine gets wrong about mail, set right at the lowest priority.
///
/// A table in `border-collapse: collapse` whose borders are `none` still computes
/// their default *medium* width, and the engine paints it and lays it out as a gap:
/// every layout table of a signature or a newsletter came out as a black grid. A
/// border nobody asked for gets no width here; anything the message sets itself —
/// a `border` attribute, a style — outranks a user-agent rule and keeps its border.
const MAIL_UA_CSS: &str = "table, thead, tbody, tfoot, tr, td, th { border-width: 0; }";

/// Le cache des images converties pour le peintre.
///
/// Le défaut de vello_cpu est de 64 Mo, calibré pour un navigateur. Un message affiche
/// quelques images à la fois, déjà réduites à 1600 pixels de côté par le chargeur.
const IMAGE_CACHE: ImageCacheConfig = ImageCacheConfig {
    max_age: 8,
    max_bytes: 16 * 1024 * 1024,
    prune_interval: 4,
};

/// Le moteur.
pub struct BlitzRenderer {
    scale: f32,
    /// Les polices du système, lues une fois et prêtées à chaque document.
    ///
    /// Chaque document en construisait un jeu à lui : l'inventaire des polices
    /// installées, refait à chaque message ouvert.
    fonts: Mutex<Option<FontContext>>,
}

impl std::fmt::Debug for BlitzRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlitzRenderer")
            .field("scale", &self.scale)
            .finish()
    }
}

impl BlitzRenderer {
    /// `scale` : l'échelle de l'écran. `dark` n'a plus d'effet — voir le module — et
    /// reste dans la signature pour ne pas imposer de changement aux appelants.
    pub fn new(scale: f32, _dark: bool) -> Self {
        Self {
            scale: scale.clamp(0.5, 4.0),
            fonts: Mutex::new(None),
        }
    }

    /// Le moteur est-il utilisable ici ?
    ///
    /// Sur le processeur, il l'est toujours : il n'y a plus de périphérique à sonder.
    /// La fonction reste, parce que la question reste légitime pour l'appelant.
    pub fn probe(scale: f32, dark: bool) -> Option<Self> {
        Some(Self::new(scale, dark))
    }

    fn fonts(&self) -> FontContext {
        let mut garde = self.fonts.lock().unwrap_or_else(|e| e.into_inner());
        garde.get_or_insert_with(FontContext::default).clone()
    }
}

/// Le message est-il raisonnable pour le moteur ?
fn admissible(html: &str) -> std::result::Result<(), String> {
    if html.len() > MAX_HTML_BYTES {
        return Err(format!("{} octets de HTML", html.len()));
    }
    let balises = html.bytes().filter(|b| *b == b'<').count();
    if balises > MAX_TAGS {
        return Err(format!("{balises} balises"));
    }
    Ok(())
}

impl HtmlRenderer for BlitzRenderer {
    fn render(&self, sanitized_html: &str, width: f32) -> Result<Rendered> {
        self.render_with(sanitized_html, width, false)
    }

    fn render_with(
        &self,
        sanitized_html: &str,
        width: f32,
        allow_remote: bool,
    ) -> Result<Rendered> {
        self.lay_out(sanitized_html, width, self.scale, allow_remote)
    }

    /// Mis en page sur la largeur du panneau, mais jamais sous [`MIN_READING_WIDTH`]
    /// points : une infolettre est dessinée pour six cents pixels de large, et la
    /// serrer davantage casserait ses tableaux. Plus étroit que cela, le document est
    /// mis en page à cette largeur puis **peint en plus petit**, directement à la taille
    /// affichée : les pixels ne sont jamais redimensionnés après coup.
    fn render_for(
        &self,
        sanitized_html: &str,
        allow_remote: bool,
        display_width: f32,
        device_scale: f32,
    ) -> Result<Rendered> {
        let affichee = display_width.max(1.0);
        let mise_en_page = affichee.max(MIN_READING_WIDTH as f32);
        let echelle = (device_scale * affichee / mise_en_page).clamp(0.25, 4.0);
        self.lay_out(sanitized_html, mise_en_page, echelle, allow_remote)
    }

    fn name(&self) -> &'static str {
        "Blitz"
    }

    fn is_full_fidelity(&self) -> bool {
        true
    }
}

impl BlitzRenderer {
    /// Met en page `width` points, peints à `scale` pixels par point.
    fn lay_out(
        &self,
        sanitized_html: &str,
        width: f32,
        scale: f32,
        allow_remote: bool,
    ) -> Result<Rendered> {
        admissible(sanitized_html).map_err(|e| {
            Error::other(format!("message trop lourd pour le moteur complet ({e})"))
        })?;

        let largeur = (width.round() as u32).clamp(MIN_WIDTH, MAX_WIDTH);
        let largeur_px = (largeur as f32 * scale).round() as u32;
        let tuile_px = (TILE_HEIGHT as f32 * scale).round() as u32;

        let viewport = Viewport::new(largeur_px, tuile_px, scale, ColorScheme::Light);
        let mut document = HtmlDocument::from_html(
            sanitized_html,
            DocumentConfig {
                viewport: Some(viewport),
                net_provider: Some(Arc::new(crate::fetch::MailNetProvider::new(allow_remote))),
                font_ctx: Some(self.fonts()),
                ua_stylesheets: Some(vec![
                    blitz_dom::DEFAULT_CSS.to_owned(),
                    MAIL_UA_CSS.to_owned(),
                ]),
                // Pas de réserve de fils pour mettre en forme un message : elle se
                // créerait au premier et ne rendrait jamais sa mémoire.
                style_threading: StyleThreading::Sequential,
                ..Default::default()
            },
        );

        tables_grow_with_their_content(&mut document);

        // Les ressources déjà arrivées — les images du message sont servies sans
        // réseau, pendant l'analyse — sont remises au document. Une feuille de style
        // peut en demander d'autres ; trois tours suffisent au courrier, et la borne
        // empêche une page construite en boucle de nous y enfermer.
        for _ in 0..3 {
            document.handle_messages();
        }
        document.resolve(0.0);

        let hauteur = document.root_element().final_layout().size.height;
        if !hauteur.is_finite() || hauteur > MAX_LAYOUT_HEIGHT {
            return Err(Error::other(format!(
                "document de {hauteur} points : rendu en texte riche"
            )));
        }
        let hauteur_px = ((hauteur * scale).ceil() as u32).max(1);

        Ok(Rendered::Document(Box::new(BlitzDocument {
            document,
            scale,
            width: largeur_px,
            height: hauteur_px,
            tile: tuile_px,
            painter: None,
        })))
    }
}

/// Makes every table height a minimum, as browsers do.
///
/// Signature tools and newsletter builders write on each table, row and cell the
/// height they measured in their own editor. A browser treats it as a minimum and
/// lets the row grow when the text needs more room (other fonts, a narrower
/// column). The engine takes it literally: the text spilled out of its cell and
/// the next block — a banner, the quoted message — was laid over it.
///
/// So `height` becomes `min-height`, whether it comes from a style or an attribute,
/// and `max-height`, which browsers ignore on tables, goes.
fn tables_grow_with_their_content(document: &mut blitz_dom::BaseDocument) {
    use blitz_dom::{local_name, ns, QualName};

    let mut changes = Vec::new();
    for (id, node) in document.tree().iter() {
        let Some(element) = node.element_data() else {
            continue;
        };
        if !matches!(
            element.name.local.as_ref(),
            "table" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th"
        ) {
            continue;
        }
        let style = element.attr(local_name!("style"));
        let attribute = element.attr(local_name!("height")).and_then(pixels);
        let converted = style.and_then(minimum_heights);
        if converted.is_none() && attribute.is_none() {
            continue;
        }
        let mut nouveau = converted.unwrap_or_else(|| style.unwrap_or_default().to_owned());
        if let Some(px) = attribute {
            // First, so that a height the style gives wins, as it would have.
            nouveau = format!("min-height:{px}px;{nouveau}");
        }
        changes.push((id, nouveau, attribute.is_some()));
    }

    let mut mutation = document.mutate();
    for (id, style, sans_attribut) in changes {
        mutation.set_attribute(id, QualName::new(None, ns!(), local_name!("style")), &style);
        if sans_attribut {
            mutation.clear_attribute(id, QualName::new(None, ns!(), local_name!("height")));
        }
    }
}

/// A declaration list with `height` turned into `min-height` and `max-height`
/// removed; `None` when it has neither.
fn minimum_heights(style: &str) -> Option<String> {
    let mut change = false;
    let declarations: Vec<String> = style
        .split(';')
        .filter_map(|declaration| {
            let Some((propriete, valeur)) = declaration.split_once(':') else {
                return Some(declaration.to_owned());
            };
            match propriete.trim().to_ascii_lowercase().as_str() {
                "height" => {
                    change = true;
                    Some(format!("min-height:{valeur}"))
                }
                "max-height" => {
                    change = true;
                    None
                }
                _ => Some(declaration.to_owned()),
            }
        })
        .collect();
    change.then(|| declarations.join(";"))
}

/// The pixels of a `height` attribute; `None` for a percentage or anything else.
fn pixels(valeur: &str) -> Option<f32> {
    let valeur = valeur.trim();
    let nombre = valeur.strip_suffix("px").unwrap_or(valeur).trim();
    nombre
        .parse::<f32>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0)
}

/// Un message mis en page, prêt à être peint.
struct BlitzDocument {
    document: HtmlDocument,
    scale: f32,
    width: u32,
    height: u32,
    tile: u32,
    /// Le peintre, et la taille de tuile pour laquelle il est construit.
    painter: Option<(VelloCpuImageRenderer, u32)>,
}

impl std::fmt::Debug for BlitzDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlitzDocument")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("painter", &self.painter.is_some())
            .finish()
    }
}

impl TiledDocument for BlitzDocument {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn tile_height(&self) -> u32 {
        self.tile
    }

    fn paint_tile(&mut self, index: usize, pixels: &mut dyn PixelSink) -> Result<()> {
        let hauteur = self.tile_extent(index);
        if hauteur == 0 {
            return Err(Error::other(format!("pas de tuile {index}")));
        }

        // Le peintre ne change de taille que pour la dernière tuile, plus courte.
        let (peintre, taille) = self.painter.get_or_insert_with(|| {
            (
                VelloCpuImageRenderer::with_image_cache_config(self.width, hauteur, IMAGE_CACHE),
                hauteur,
            )
        });
        if *taille != hauteur {
            peintre.resize(self.width, hauteur);
            *taille = hauteur;
        }
        // `render` adds to what the painter already holds: without this, every tile
        // also carried the drawing of the tiles painted before it, and a reply showed
        // printed over the message it quotes.
        peintre.reset();

        // Le document défile jusqu'à la tuile ; le peintre écarte de lui-même ce qui
        // sort du cadre, si bien qu'une tuile ne coûte que ce qu'elle montre.
        let haut = index as f64 * self.tile as f64 / self.scale as f64;
        self.document
            .set_viewport_scroll(blitz_dom::Point { x: 0.0, y: haut });

        let tampon = pixels.rgba(self.width, hauteur);
        let attendu = self.width as usize * hauteur as usize * 4;
        if tampon.len() != attendu {
            return Err(Error::other(format!(
                "tampon de {} octets pour une tuile qui en demande {attendu}",
                tampon.len()
            )));
        }

        let (largeur, echelle) = (self.width, self.scale as f64);
        let document = &mut self.document;
        peintre.render(
            |scene| paint_scene(scene, document, echelle, largeur, hauteur, 0, 0),
            tampon,
        );
        sur_fond_blanc(tampon);
        Ok(())
    }

    fn release(&mut self) {
        self.painter = None;
    }

    fn select_from(&mut self, x: f32, y: f32) -> bool {
        let avait = self.document.has_text_selection();
        let (x, y) = (x / self.scale, y / self.scale);
        match self.document.find_text_position(x, y) {
            Some((noeud, decalage)) => self
                .document
                .set_text_selection(noeud, decalage, noeud, decalage),
            None => self.document.clear_text_selection(),
        }
        avait
    }

    fn select_to(&mut self, x: f32, y: f32) -> bool {
        let avant = self.document.get_text_selection_ranges();
        let (x, y) = (x / self.scale, y / self.scale);
        if !self.document.extend_text_selection_to_point(x, y) {
            return false;
        }
        self.document.get_text_selection_ranges() != avant
    }

    fn clear_selection(&mut self) -> bool {
        let avait = self.document.has_text_selection();
        self.document.clear_text_selection();
        avait
    }

    fn selected_text(&self) -> Option<String> {
        self.document.get_selected_text()
    }

    /// The `<a href>` around what is under the point: the hit is a word, an image or a
    /// box inside the link, so the link is found by going up from it.
    fn link_at(&self, x: f32, y: f32) -> Option<String> {
        use blitz_dom::local_name;

        let touche = self.document.hit(x / self.scale, y / self.scale)?;
        let mut noeud = Some(touche.node_id);
        while let Some(id) = noeud {
            let n = self.document.get_node(id)?;
            if n.element_data()
                .is_some_and(|el| el.name.local == local_name!("a"))
            {
                if let Some(href) = n.attr(local_name!("href")) {
                    return crate::openable_link(href).map(str::to_owned);
                }
            }
            noeud = n.parent;
        }
        None
    }
}

/// Compose une image prémultipliée sur du blanc, en place.
///
/// Le peintre rend des pixels prémultipliés, transparents là où le message n'a pas de
/// fond ; l'interface attend des pixels ordinaires. Poser l'ensemble sur une page
/// blanche règle les deux : une couleur prémultipliée plus le blanc qui manque à son
/// opacité donne la couleur affichée, et l'image devient opaque.
fn sur_fond_blanc(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let manque = 255 - pixel[3];
        pixel[0] = pixel[0].saturating_add(manque);
        pixel[1] = pixel[1].saturating_add(manque);
        pixel[2] = pixel[2].saturating_add(manque);
        pixel[3] = 255;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moteur() -> BlitzRenderer {
        BlitzRenderer::new(1.0, false)
    }

    fn document(html: &str) -> Box<dyn TiledDocument> {
        match moteur().render(html, 800.0).unwrap() {
            Rendered::Document(d) => d,
            autre => panic!("attendu un document, obtenu {autre:?}"),
        }
    }

    #[test]
    fn un_document_simple_est_mis_en_page_a_la_largeur_demandee() {
        let d = document("<p>Bonjour Marie</p>");
        let (largeur, hauteur) = d.size();
        assert_eq!(largeur, 800);
        assert!(hauteur > 0);
        assert_eq!(d.tile_count(), 1);
    }

    #[test]
    fn une_tuile_est_peinte_opaque_dans_le_tampon_de_l_appelant() {
        let mut d = document("<p style='color:#000'>Bonjour</p>");
        let mut pixels = crate::VecSink::default();
        d.paint_tile(0, &mut pixels).unwrap();
        let (largeur, _) = d.size();
        assert_eq!(pixels.0.len(), (largeur * d.tile_extent(0) * 4) as usize);
        assert!(
            pixels.0.chunks_exact(4).all(|p| p[3] == 255),
            "opaque partout"
        );
        assert!(
            pixels.0.chunks_exact(4).any(|p| p[0] < 128),
            "le texte noir est bien dessiné"
        );
    }

    #[test]
    fn un_long_message_se_decoupe_en_tuiles_peintes_une_a_une() {
        let mut d = document(&format!("<p>{}</p>", "Une ligne<br>".repeat(400)));
        assert!(d.tile_count() > 3, "{} tuiles", d.tile_count());
        let derniere = d.tile_count() - 1;
        let mut pixels = crate::VecSink::default();
        d.paint_tile(derniere, &mut pixels).unwrap();
        d.paint_tile(1, &mut pixels).unwrap();
        assert!(d.paint_tile(d.tile_count(), &mut pixels).is_err());
    }

    #[test]
    fn une_tuile_ne_garde_rien_de_la_precedente() {
        // A black band at the very top, then nothing but white for several tiles.
        // Tile 1 is painted after tile 0 with the same painter: if the painter kept
        // tile 0's drawing, the band shows again at the top of tile 1 — the text of
        // a reply printed over the message it quotes.
        let mut d = document(
            "<body style='margin:0'><div style='height:100px;background:#000'></div>             <div style='height:2000px'></div></body>",
        );
        assert!(d.tile_count() >= 3);
        let mut pixels = crate::VecSink::default();
        d.paint_tile(0, &mut pixels).unwrap();
        assert!(
            pixels.0.chunks_exact(4).any(|p| p[0] < 16),
            "the band is in tile 0"
        );
        for tuile in [1, 2] {
            d.paint_tile(tuile, &mut pixels).unwrap();
            assert!(
                pixels.0.chunks_exact(4).all(|p| p[0] > 240),
                "tile {tuile} must be blank"
            );
        }
    }

    /// A plain red square, 8 × 8.
    const ROUGE: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEklEQVR4nGP8z4AdMOEQH6QSAM1BAQ/oQeJvAAAAAElFTkSuQmCC";

    /// The rows of tile 0 holding red pixels, and those holding dark ones.
    fn rangees(html: &str) -> (Vec<u32>, Vec<u32>) {
        let mut d = document(html);
        let mut pixels = crate::VecSink::default();
        d.paint_tile(0, &mut pixels).unwrap();
        let largeur = d.size().0 as usize;
        let (mut rouges, mut sombres) = (Vec::new(), Vec::new());
        for (y, ligne) in pixels.0.chunks_exact(largeur * 4).enumerate() {
            let px = || ligne.chunks_exact(4);
            if px().any(|p| p[0] > 200 && p[1] < 60 && p[2] < 60) {
                rouges.push(y as u32);
            }
            if px().any(|p| p[0] < 90 && p[1] < 90 && p[2] < 90) {
                sombres.push(y as u32);
            }
        }
        (rouges, sombres)
    }

    #[test]
    fn une_image_dans_une_ligne_de_hauteur_nulle_ne_monte_pas_sur_le_texte() {
        // What signature tools write around every picture: a block with
        // `line-height:0`. The picture must still push what follows down and stay
        // below what precedes it, as in every mail client.
        let (rouges, sombres) = rangees(&format!(
            "<body style='margin:0;font:16px sans-serif;color:#000'>             <div>Above</div>             <div style='line-height:0px'><img src='{ROUGE}' width='100' height='60'></div>             <div>Below</div></body>"
        ));
        let (haut, bas) = (rouges[0], *rouges.last().unwrap());
        assert!(
            bas - haut >= 55,
            "the picture is drawn whole: {haut}..{bas}"
        );
        assert!(
            sombres.iter().all(|y| *y < haut || *y > bas),
            "no text inside the picture's rows {haut}..{bas}: {sombres:?}"
        );
        assert!(sombres.iter().any(|y| *y < haut) && sombres.iter().any(|y| *y > bas));
    }

    #[test]
    fn les_hauteurs_de_tableau_deviennent_des_minimums() {
        assert_eq!(
            minimum_heights("width:150px; height:139.88px; max-height:120px").as_deref(),
            Some("width:150px;min-height:139.88px")
        );
        assert_eq!(minimum_heights("line-height:16px; min-height:3px"), None);
        assert_eq!(
            minimum_heights("background:url(data:image/png;base64,AAAA);Height:2px").as_deref(),
            Some("background:url(data:image/png;base64,AAAA);min-height:2px")
        );
        assert_eq!(pixels(" 40 "), Some(40.0));
        assert_eq!(pixels("12px"), Some(12.0));
        assert_eq!(pixels("100%"), None);
    }

    #[test]
    fn un_tableau_trop_petit_pour_son_texte_grandit() {
        // Signature tools give their tables the height they measured in their own
        // editor. In a browser a table height is a minimum; if the text needs more,
        // the table grows and what follows moves down instead of covering it.
        let (rouges, sombres) = rangees(&format!(
            "<body style='margin:0;font:16px sans-serif;color:#000'>             <table cellspacing='0' cellpadding='0' style='height:20px'><tr>             <td style='height:20px'>One<br>Two<br>Three<br>Four</td></tr></table>             <table height='10'><tr><td height='10'>Five<br>Six</td></tr></table>             <div><img src='{ROUGE}' width='100' height='40'></div></body>"
        ));
        let haut = rouges[0];
        assert!(
            sombres.iter().all(|y| *y < haut),
            "the text ends above the picture at {haut}: {sombres:?}"
        );
        assert!(
            sombres.iter().filter(|y| **y > 60).count() > 0,
            "all six lines are laid out"
        );
    }

    #[test]
    fn un_tableau_sans_bordure_n_est_pas_encadre() {
        // `border-collapse: collapse` with no border set: nothing is drawn.
        let (_, sombres) = rangees(
            "<body style='margin:0'><table style='border-collapse:collapse'>             <tr><td style='width:300px;height:40px'></td></tr></table></body>",
        );
        assert!(sombres.is_empty(), "rows with a border: {sombres:?}");
    }

    #[test]
    fn une_vraie_bordure_reste_dessinee() {
        let (_, sombres) = rangees(
            "<body style='margin:0'><table border='1' style='border-collapse:collapse'>             <tr><td style='width:300px;height:40px'></td></tr></table>             <table style='margin-top:20px'><tr>             <td style='border:2px solid #000;width:300px;height:40px'></td></tr></table></body>",
        );
        assert!(
            sombres.len() >= 4,
            "both tables keep their border: {sombres:?}"
        );
    }

    #[test]
    fn a_link_is_found_under_its_words_and_nowhere_else() {
        let d = document(
            "<body style='margin:0;font:16px sans-serif'>             <div style='height:40px'><a href='https://example.com/offer?a=1&amp;b=2'>             <span>The offer</span></a></div>             <div style='height:40px'><a href='javascript:alert(1)'>Not this</a></div>             <div style='height:40px'>No link here</div></body>",
        );
        assert_eq!(
            d.link_at(10.0, 10.0).as_deref(),
            Some("https://example.com/offer?a=1&b=2")
        );
        assert_eq!(d.link_at(10.0, 50.0), None, "a script is never a link");
        assert_eq!(d.link_at(10.0, 90.0), None);
    }

    #[test]
    fn la_hauteur_suit_le_contenu() {
        let court = document("<p>Une ligne</p>").size().1;
        let long = document(&format!("<p>{}</p>", "Une ligne<br>".repeat(60)))
            .size()
            .1;
        assert!(long > court);
    }

    #[test]
    fn la_largeur_est_bornee() {
        for demandee in [10.0, 100_000.0] {
            if let Rendered::Document(d) = moteur().render("<p>x</p>", demandee).unwrap() {
                assert!((MIN_WIDTH..=MAX_WIDTH).contains(&d.size().0));
            }
        }
    }

    #[test]
    fn l_echelle_multiplie_les_pixels() {
        let d = match BlitzRenderer::new(2.0, false)
            .render("<p>x</p>", 800.0)
            .unwrap()
        {
            Rendered::Document(d) => d,
            _ => unreachable!(),
        };
        assert_eq!(d.size().0, 1600);
        assert_eq!(d.tile_height(), 1024);
    }

    #[test]
    fn une_infolettre_en_tableaux_est_rendue() {
        let html = r##"<table width="600" cellpadding="10">
              <tr><td bgcolor="#eeeeee"><b>Nos offres</b></td></tr>
              <tr><td><table><tr><td>Article</td><td>12 €</td></tr></table></td></tr>
            </table>"##;
        let mut d = document(html);
        d.paint_tile(0, &mut crate::VecSink::default()).unwrap();
    }

    #[test]
    fn un_document_vide_ne_fait_pas_echouer_le_rendu() {
        assert!(moteur().render("", 800.0).is_ok());
    }

    #[test]
    fn un_message_demesure_est_refuse_avant_l_analyse() {
        let enorme = "<b>x</b>".repeat(MAX_TAGS);
        assert!(moteur().render(&enorme, 800.0).is_err());
    }

    #[test]
    fn liberer_le_peintre_ne_perd_pas_le_document() {
        let mut d = document("<p>Bonjour</p>");
        let mut pixels = crate::VecSink::default();
        d.paint_tile(0, &mut pixels).unwrap();
        d.release();
        d.paint_tile(0, &mut pixels).unwrap();
    }

    #[test]
    fn un_tampon_trop_court_est_refuse_plutot_que_rempli_a_moitie() {
        #[derive(Default)]
        struct Avare(Vec<u8>);
        impl crate::PixelSink for Avare {
            fn rgba(&mut self, _w: u32, _h: u32) -> &mut [u8] {
                self.0.resize(16, 0);
                &mut self.0
            }
        }
        let mut d = document("<p>Bonjour</p>");
        let erreur = d
            .paint_tile(0, &mut Avare::default())
            .unwrap_err()
            .to_string();
        assert!(erreur.contains("tampon"), "obtenu : {erreur}");
    }

    #[test]
    fn le_blanc_comble_la_transparence() {
        let mut p = [0u8, 0, 0, 0, 100, 0, 0, 255, 50, 50, 50, 128];
        sur_fond_blanc(&mut p);
        assert_eq!(&p[0..4], &[255, 255, 255, 255], "transparent → blanc");
        assert_eq!(&p[4..8], &[100, 0, 0, 255], "opaque inchangé");
        assert_eq!(&p[8..12], &[177, 177, 177, 255]);
    }

    #[test]
    fn un_panneau_large_met_en_page_a_sa_largeur_et_a_l_echelle_de_l_ecran() {
        let Rendered::Document(d) = moteur().render_for("<p>x</p>", false, 900.0, 1.5).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(d.size().0, 1350, "900 points à 1,5 pixel par point");
    }

    #[test]
    fn un_panneau_etroit_est_peint_en_plus_petit_sans_casser_la_mise_en_page() {
        // 500 points de large : mise en page à 640, peinte sur 500 × 2 pixels.
        let Rendered::Document(d) = moteur().render_for("<p>x</p>", false, 500.0, 2.0).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(d.size().0, 1000);
    }

    #[test]
    fn le_moteur_se_declare_fidele() {
        let m = moteur();
        assert!(m.is_full_fidelity());
        assert_eq!(m.name(), "Blitz");
    }

    #[test]
    fn l_echelle_est_bornee_a_la_construction() {
        assert_eq!(BlitzRenderer::new(0.01, true).scale, 0.5);
        assert_eq!(BlitzRenderer::new(99.0, true).scale, 4.0);
    }
}
