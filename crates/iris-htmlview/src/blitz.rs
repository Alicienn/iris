//! Le moteur HTML complet, au-dessus de Blitz.
//!
//! Pour les infolettres bâties en tableaux imbriqués, où la mise en page **fait
//! partie du message** et où le texte riche ne suffit plus.
//!
//! Le choix d'intégration mérite d'être expliqué, parce qu'il n'est pas celui qu'on
//! attendrait. Blitz dessine dans Vello, qui dessine dans wgpu ; Slint sait importer
//! une texture wgpu, et il aurait été possible de partager un device. Nous ne le
//! faisons pas : **Blitz rend dans une image, et l'interface affiche cette image**.
//!
//! Trois raisons :
//!
//! - un corps de message se redessine **quand on l'ouvre**, pas à chaque frame. Le
//!   coût d'une relecture GPU vers la mémoire est payé une fois par message, ce qui
//!   est indolore, là où le partage de device coûterait un couplage permanent ;
//! - ce couplage lierait nos versions de wgpu à celles de deux projets tiers
//!   indépendants. Elles coïncident aujourd'hui ; elles divergeront ;
//! - le rendu peut se faire **hors du fil d'affichage**, ce que l'invariant n° 1
//!   exige de toute façon.
//!
//! Le moteur est optionnel et ne peut pas devenir obligatoire : sur une machine sans
//! GPU exploitable, sa construction échoue et l'appelant retombe sur le texte riche.

use crate::{HtmlRenderer, Rendered};
use anyrender::ImageRenderer;
use anyrender_vello::VelloImageRenderer;
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use iris_types::{Error, Result};
use std::sync::Mutex;

/// Hauteur maximale rendue, en pixels.
///
/// Une infolettre déraisonnable ne doit pas faire allouer une texture de plusieurs
/// centaines de mégaoctets. Au-delà, le message est tronqué — et il l'était déjà à
/// l'écran.
const MAX_HEIGHT: u32 = 20_000;

/// Largeurs minimale et maximale de rendu.
const MIN_WIDTH: u32 = 320;
const MAX_WIDTH: u32 = 2_400;

/// Le moteur.
pub struct BlitzRenderer {
    /// Le rendeur est coûteux à construire — il ouvre un device GPU — et n'est pas
    /// partageable entre fils : on le garde, sous verrou.
    renderer: Mutex<Option<VelloImageRenderer>>,
    scale: f32,
    dark: bool,
}

impl std::fmt::Debug for BlitzRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlitzRenderer")
            .field("scale", &self.scale)
            .field("dark", &self.dark)
            .finish()
    }
}

impl BlitzRenderer {
    /// Construit le moteur sans encore ouvrir de device.
    pub fn new(scale: f32, dark: bool) -> Self {
        Self {
            renderer: Mutex::new(None),
            scale: scale.clamp(0.5, 4.0),
            dark,
        }
    }

    /// Vérifie que la machine peut réellement rendre.
    ///
    /// À appeler une fois au démarrage : mieux vaut savoir tout de suite qu'on
    /// restera en texte riche que de le découvrir à l'ouverture d'un message.
    pub fn is_available() -> bool {
        build_renderer(64, 64).is_some()
    }
}

/// Construit un rendeur, en interceptant l'échec.
///
/// La bibliothèque interrompt le programme quand aucun device n'est disponible ;
/// nous ne pouvons pas laisser l'absence de GPU emporter l'application.
fn build_renderer(width: u32, height: u32) -> Option<VelloImageRenderer> {
    std::panic::catch_unwind(|| VelloImageRenderer::new(width, height)).ok()
}

impl HtmlRenderer for BlitzRenderer {
    fn render(&self, sanitized_html: &str, width: f32) -> Result<Rendered> {
        let largeur = (width.round() as u32).clamp(MIN_WIDTH, MAX_WIDTH);

        // 1. Analyse et mise en page. Aucun fournisseur réseau n'est installé : les
        //    ressources distantes ont déjà été neutralisées par l'assainissement, et
        //    le moteur ne doit surtout pas les rechercher lui-même.
        let viewport = Viewport::new(
            largeur,
            MAX_HEIGHT,
            self.scale,
            if self.dark {
                ColorScheme::Dark
            } else {
                ColorScheme::Light
            },
        );
        let mut document = HtmlDocument::from_html(
            sanitized_html,
            DocumentConfig {
                viewport: Some(viewport),
                ..Default::default()
            },
        );
        document.resolve(0.0);

        // 2. Hauteur réelle du contenu, bornée.
        let hauteur_contenu = document.root_element().final_layout.size.height;
        let hauteur = ((hauteur_contenu * self.scale).ceil() as u32).clamp(1, MAX_HEIGHT);

        // 3. Rendu dans une image.
        let mut garde = self
            .renderer
            .lock()
            .map_err(|_| Error::other("moteur de rendu HTML empoisonné"))?;

        let rendeur = match garde.as_mut() {
            Some(r) => {
                r.resize(largeur, hauteur);
                r
            }
            None => {
                let nouveau = build_renderer(largeur, hauteur).ok_or_else(|| {
                    Error::other("aucun périphérique graphique compatible pour le rendu HTML")
                })?;
                garde.insert(nouveau)
            }
        };

        let mut pixels = Vec::new();
        let echelle = self.scale as f64;
        rendeur.render_to_vec(
            |scene| paint_scene(scene, &document, echelle, largeur, hauteur),
            &mut pixels,
        );

        Ok(Rendered::Texture {
            width: largeur,
            height: hauteur,
            rgba: pixels,
        })
    }

    fn name(&self) -> &'static str {
        "Blitz"
    }

    fn is_full_fidelity(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Les tests de rendu exigent un périphérique graphique. Sur une machine qui n'en
    /// a pas — une intégration continue sans GPU, par exemple —, ils s'abstiennent
    /// plutôt que d'échouer : leur absence de GPU n'est pas un défaut du code.
    fn moteur() -> Option<BlitzRenderer> {
        BlitzRenderer::is_available().then(|| BlitzRenderer::new(1.0, true))
    }

    #[test]
    fn un_document_simple_est_rendu() {
        let Some(m) = moteur() else {
            eprintln!("aucun périphérique graphique : test ignoré");
            return;
        };

        let rendu = m.render("<p>Bonjour Marie</p>", 800.0).unwrap();
        match rendu {
            Rendered::Texture {
                width,
                height,
                rgba,
            } => {
                assert_eq!(width, 800);
                assert!(height > 0);
                assert_eq!(rgba.len(), (width * height * 4) as usize);
            }
            autre => panic!("attendu une image, obtenu {autre:?}"),
        }
    }

    #[test]
    fn la_hauteur_suit_le_contenu() {
        let Some(m) = moteur() else { return };

        let court = m.render("<p>Une ligne</p>", 800.0).unwrap();
        let long = m
            .render(&format!("<p>{}</p>", "Une ligne<br>".repeat(60)), 800.0)
            .unwrap();

        let hauteur = |r: &Rendered| match r {
            Rendered::Texture { height, .. } => *height,
            _ => 0,
        };
        assert!(
            hauteur(&long) > hauteur(&court),
            "le document long doit produire une image plus haute"
        );
    }

    #[test]
    fn la_largeur_est_bornee() {
        let Some(m) = moteur() else { return };

        for demandee in [10.0, 100_000.0] {
            let rendu = m.render("<p>x</p>", demandee).unwrap();
            if let Rendered::Texture { width, .. } = rendu {
                assert!(
                    (MIN_WIDTH..=MAX_WIDTH).contains(&width),
                    "largeur {width} hors bornes pour une demande de {demandee}"
                );
            }
        }
    }

    #[test]
    fn une_infolettre_en_tableaux_est_rendue() {
        // C'est le cas qui justifie l'existence de ce moteur.
        let Some(m) = moteur() else { return };

        let html = r##"<table width="600" cellpadding="10">
              <tr><td bgcolor="#eeeeee"><b>Nos offres</b></td></tr>
              <tr><td><table><tr><td>Article</td><td>12 €</td></tr></table></td></tr>
            </table>"##;

        let rendu = m.render(html, 800.0).unwrap();
        assert!(matches!(rendu, Rendered::Texture { .. }));
    }

    #[test]
    fn un_document_vide_ne_fait_pas_echouer_le_rendu() {
        let Some(m) = moteur() else { return };
        assert!(m.render("", 800.0).is_ok());
    }

    #[test]
    fn le_moteur_se_declare_fidele() {
        let m = BlitzRenderer::new(1.0, true);
        assert!(m.is_full_fidelity());
        assert_eq!(m.name(), "Blitz");
    }

    #[test]
    fn l_echelle_est_bornee_a_la_construction() {
        // Une échelle absurde produirait une texture démesurée.
        assert_eq!(BlitzRenderer::new(0.01, true).scale, 0.5);
        assert_eq!(BlitzRenderer::new(99.0, true).scale, 4.0);
    }

    #[test]
    fn l_absence_de_peripherique_ne_fait_pas_paniquer() {
        // On ne peut pas simuler l'absence de GPU, mais on peut vérifier que la
        // détection elle-même est sans danger et répétable.
        let _ = BlitzRenderer::is_available();
        let _ = BlitzRenderer::is_available();
    }
}
