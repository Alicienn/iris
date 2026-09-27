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

/// Hauteur maximale d'une texture, en pixels.
///
/// **Ce n'est pas un réglage de confort, c'est une limite matérielle.** Elle valait
/// 20 000, et une carte graphique qui n'accepte pas plus de 8 192 refusait la demande
/// — wgpu répond à un refus par une panique, et le programme s'arrêtait. Deux fois,
/// sur deux messages longs :
///
/// ```text
/// In Device::create_texture
///   Dimension Y value 20000 exceeds the limit of 8192
/// ```
///
/// 8 192 est la garantie de la quasi-totalité du matériel de bureau, et le minimum
/// exigé par la spécification WebGPU au niveau « default ». Une carte plus généreuse
/// n'y perd rien : ce qui dépasse ne va pas au moteur complet, il va au texte riche,
/// qui n'a pas de texture et défile sans limite.
const MAX_HEIGHT: u32 = 8_192;

/// La mise en page dépasse-t-elle ce qu'une texture peut porter ?
///
/// Séparé du rendu pour être testable sans carte graphique : c'est la décision qui a
/// planté l'application, et elle doit pouvoir être vérifiée là où il n'y a pas de GPU.
fn trop_haut(hauteur_contenu: f32, scale: f32) -> bool {
    (hauteur_contenu * scale).ceil() as u32 > MAX_HEIGHT
}

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

    /// Vérifie que la machine peut réellement rendre, **et garde ce qu'elle a ouvert**.
    ///
    /// À appeler une fois au démarrage : mieux vaut savoir tout de suite qu'on
    /// restera en texte riche que de le découvrir à l'ouverture d'un message.
    ///
    /// La vérification ouvrait un périphérique graphique minuscule puis le jetait, et
    /// le premier message ouvert en rouvrait un autre. Deux ouvertures pour un seul
    /// usage : la seconde coûte huit cents millisecondes sur le fil de l'interface,
    /// au moment précis où quelqu'un vient de cliquer sur un message. Le périphérique
    /// de la sonde est donc conservé, et c'est lui qui rendra. Le gain en mémoire est
    /// mince — le pilote réutilisait déjà ses réserves — mais le temps, lui, est bien
    /// payé deux fois.
    pub fn probe(scale: f32, dark: bool) -> Option<Self> {
        let sonde = build_renderer(64, 64)?;
        Some(Self {
            renderer: Mutex::new(Some(sonde)),
            scale: scale.clamp(0.5, 4.0),
            dark,
        })
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
    fn render(
        &self,
        sanitized_html: &str,
        width: f32,
        pixels: &mut dyn crate::PixelSink,
    ) -> Result<Rendered> {
        self.render_with(sanitized_html, width, false, pixels)
    }

    fn render_with(
        &self,
        sanitized_html: &str,
        width: f32,
        allow_remote: bool,
        pixels: &mut dyn crate::PixelSink,
    ) -> Result<Rendered> {
        let largeur = (width.round() as u32).clamp(MIN_WIDTH, MAX_WIDTH);

        // 1. Analyse et mise en page. Les ressources passent par notre chargeur, qui
        //    accepte les images incrustées sans réseau et ne sort du programme que
        //    lorsque le lecteur a explicitement demandé les images distantes.
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
        // Le chargeur de ressources. Il ne va sur le réseau que si le lecteur l'a
        // demandé pour ce message ; sans lui, aucune image ne s'affichait, pas même
        // celles que le message transportait lui-même.
        let (fournisseur, en_attente) = crate::fetch::MailNetProvider::new(allow_remote);
        let mut document = HtmlDocument::from_html(
            sanitized_html,
            DocumentConfig {
                viewport: Some(viewport),
                net_provider: Some(fournisseur),
                ..Default::default()
            },
        );

        // Les ressources arrivées pendant l'analyse sont remises au document
        // maintenant, parce que le fournisseur ne peut pas le faire : il est appelé
        // pendant sa construction. Une feuille de style peut en demander d'autres, et
        // ces autres à leur tour ; trois tours suffisent à tout ce qui ressemble à du
        // courrier, et la borne empêche une page bâtie en boucle de nous y enfermer.
        for _ in 0..3 {
            let arrivees: Vec<_> = std::mem::take(
                &mut *en_attente.lock().unwrap_or_else(|e| e.into_inner()),
            );
            if arrivees.is_empty() {
                break;
            }
            for ressource in arrivees {
                document.load_resource(ressource);
            }
        }

        document.resolve(0.0);

        // 2. Hauteur réelle du contenu.
        //
        // Trop haute pour une texture : on rend la main plutôt que de tronquer. Le
        // moteur adaptatif retombe alors sur le texte riche, qui n'a pas de texture et
        // défile sans limite — un message long y est **entier**, ce qu'il ne serait pas
        // ici. Tronquer serait le seul cas où l'application déciderait toute seule que
        // la fin d'un message ne mérite pas d'être lue.
        let hauteur_contenu = document.root_element().final_layout.size.height;
        if trop_haut(hauteur_contenu, self.scale) {
            return Err(Error::other(format!(
                "message trop long pour une texture ({} px) : rendu en texte riche",
                (hauteur_contenu * self.scale).ceil() as u32
            )));
        }
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

        // Le tampon de l'appelant, demandé maintenant : la hauteur ne se connaît
        // qu'après la mise en page, et c'est tout l'intérêt de ne le réclamer qu'ici.
        let tampon = pixels.rgba(largeur, hauteur);
        let attendu = (largeur as usize) * (hauteur as usize) * 4;
        if tampon.len() != attendu {
            // Peindre dans un tampon plus court écrirait du blanc sur la fin de
            // l'image, ou pire ; le dire vaut mieux que de le dessiner.
            return Err(Error::other(format!(
                "tampon de {} octets pour une image qui en demande {attendu}",
                tampon.len()
            )));
        }

        let echelle = self.scale as f64;
        rendeur.render(
            |scene| paint_scene(scene, &document, echelle, largeur, hauteur),
            tampon,
        );

        Ok(Rendered::Texture {
            width: largeur,
            height: hauteur,
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
        BlitzRenderer::probe(1.0, true)
    }

    /// Le tampon des tests.
    fn tampon() -> crate::VecSink {
        crate::VecSink::default()
    }

    #[test]
    fn la_limite_de_texture_est_celle_du_materiel() {
        // Elle valait 20 000. Une carte qui s'arrête à 8 192 refusait la demande, wgpu
        // répondait au refus par une panique, et l'application s'arrêtait — deux fois,
        // sur deux messages longs. 8 192 est le plancher garanti par la spécification.
        assert_eq!(MAX_HEIGHT, 8_192);
    }

    #[test]
    fn un_document_trop_haut_est_refuse_avant_le_gpu() {
        // Refusé, pas tronqué : le moteur adaptatif retombe sur le texte riche, où le
        // message est entier. Le tronquer serait le seul endroit de l'application qui
        // déciderait tout seul que la fin d'un message ne mérite pas d'être lue.
        assert!(trop_haut(20_000.0, 1.0));
        assert!(trop_haut(8_193.0, 1.0));
        assert!(!trop_haut(8_192.0, 1.0));
        assert!(!trop_haut(600.0, 1.0));
    }

    #[test]
    fn l_echelle_compte_dans_la_limite() {
        // C'est la texture qui est bornée, pas la mise en page : sur un écran à 200 %,
        // un document de 5 000 points en fait 10 000 en pixels.
        assert!(trop_haut(5_000.0, 2.0));
        assert!(!trop_haut(5_000.0, 1.0));
    }

    #[test]
    fn un_document_simple_est_rendu() {
        let Some(m) = moteur() else {
            eprintln!("aucun périphérique graphique : test ignoré");
            return;
        };

        let mut pixels = tampon();
        let rendu = m.render("<p>Bonjour Marie</p>", 800.0, &mut pixels).unwrap();
        match rendu {
            Rendered::Texture { width, height } => {
                assert_eq!(width, 800);
                assert!(height > 0);
                // Les pixels sont chez l'appelant, et ils y sont en entier.
                assert_eq!(pixels.0.len(), (width * height * 4) as usize);
            }
            autre => panic!("attendu une image, obtenu {autre:?}"),
        }
    }

    #[test]
    fn la_hauteur_suit_le_contenu() {
        let Some(m) = moteur() else { return };

        let mut pixels = tampon();
        let court = m.render("<p>Une ligne</p>", 800.0, &mut pixels).unwrap();
        let long = m
            .render(
                &format!("<p>{}</p>", "Une ligne<br>".repeat(60)),
                800.0,
                &mut pixels,
            )
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
            let rendu = m.render("<p>x</p>", demandee, &mut tampon()).unwrap();
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

        let rendu = m.render(html, 800.0, &mut tampon()).unwrap();
        assert!(matches!(rendu, Rendered::Texture { .. }));
    }

    #[test]
    fn un_document_vide_ne_fait_pas_echouer_le_rendu() {
        let Some(m) = moteur() else { return };
        assert!(m.render("", 800.0, &mut tampon()).is_ok());
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
        let _ = BlitzRenderer::probe(1.0, true);
        let _ = BlitzRenderer::probe(1.0, true);
    }

    #[test]
    fn la_sonde_garde_le_peripherique_qu_elle_a_ouvert() {
        // C'est tout son objet : le premier message ouvert ne doit pas payer une
        // seconde ouverture, qui se compte en centaines de millisecondes.
        let Some(m) = BlitzRenderer::probe(1.0, true) else {
            eprintln!("aucun périphérique graphique : test ignoré");
            return;
        };
        assert!(
            m.renderer.lock().unwrap().is_some(),
            "la sonde doit conserver son rendeur"
        );
    }

    #[test]
    fn un_tampon_trop_court_est_refuse_plutot_que_rempli_a_moitie() {
        // Peindre dans un tampon plus court écrirait sur la fin de l'image.
        #[derive(Default)]
        struct Avare(Vec<u8>);
        impl crate::PixelSink for Avare {
            fn rgba(&mut self, _w: u32, _h: u32) -> &mut [u8] {
                self.0.resize(16, 0);
                &mut self.0
            }
        }

        let Some(m) = moteur() else { return };
        let erreur = m
            .render("<p>Bonjour</p>", 800.0, &mut Avare::default())
            .unwrap_err()
            .to_string();
        assert!(erreur.contains("tampon"), "obtenu : {erreur}");
    }
}
