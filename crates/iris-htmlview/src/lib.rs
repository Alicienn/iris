//! `iris-htmlview` — rendu du corps des messages.
//!
//! Le corps d'un mail est du HTML arbitraire, et c'est le seul endroit de
//! l'application où nous ne maîtrisons pas le contenu. Deux implémentations
//! coexistent derrière un même trait :
//!
//! - [`RichTextRenderer`], intégralement en Rust, sans moteur, sans GPU : il
//!   transforme le HTML assaini en une suite de blocs que l'interface sait dessiner.
//!   Il couvre parfaitement les messages écrits par des humains, qui sont la
//!   quasi-totalité de ce qu'on lit vraiment, et il ne peut pas échouer ;
//! - un moteur HTML complet, pour les infolettres bâties en tableaux imbriqués, où
//!   la mise en page fait partie du message.
//!
//! Le trait existe pour que le choix du second reste réversible. C'est le point le
//! plus incertain du projet : y attacher le reste de l'application serait une faute.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

#[cfg(feature = "blitz")]
mod blitz;
mod richtext;

#[cfg(feature = "blitz")]
pub use blitz::BlitzRenderer;
pub use richtext::{Block, Inline, RichText, RichTextRenderer};

use iris_types::Result;

/// Ce qu'un moteur de rendu produit.
#[derive(Debug, Clone, PartialEq)]
pub enum Rendered {
    /// Une suite de blocs, dessinés par l'interface elle-même.
    Blocks(RichText),
    /// Une image déjà composée, à afficher telle quelle.
    Texture {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

impl Rendered {
    pub fn as_blocks(&self) -> Option<&RichText> {
        match self {
            Self::Blocks(b) => Some(b),
            Self::Texture { .. } => None,
        }
    }
}

/// Un moteur de rendu de corps de message.
pub trait HtmlRenderer: std::fmt::Debug + Send + Sync {
    /// Rend un corps **déjà assaini**.
    ///
    /// L'assainissement n'est pas la responsabilité du moteur : il a lieu une fois, à
    /// l'analyse, et un moteur qui recevrait du HTML brut pourrait exécuter ce que
    /// l'assainissement aurait retiré.
    fn render(&self, sanitized_html: &str, width: f32) -> Result<Rendered>;

    /// Nom du moteur, pour le diagnostic et les réglages.
    fn name(&self) -> &'static str;

    /// Le moteur restitue-t-il fidèlement une mise en page complexe ?
    ///
    /// L'interface s'en sert pour proposer une solution de repli quand un message
    /// s'affiche mal.
    fn is_full_fidelity(&self) -> bool;
}

/// Choisit un moteur selon la complexité du message.
///
/// Un message écrit par un humain n'a pas besoin d'un moteur de rendu complet ; une
/// infolettre en tableaux imbriqués, si. Mesurer plutôt que deviner évite de payer le
/// prix du second pour la quasi-totalité des messages.
#[derive(Debug)]
pub struct AdaptiveRenderer {
    simple: Box<dyn HtmlRenderer>,
    complete: Option<Box<dyn HtmlRenderer>>,
    threshold: ComplexityThreshold,
}

/// Seuils au-delà desquels un message est jugé « mis en page ».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComplexityThreshold {
    pub max_tables: usize,
    pub max_nesting: usize,
    pub max_inline_styles: usize,
}

impl Default for ComplexityThreshold {
    fn default() -> Self {
        // Un tableau isolé reste lisible en texte riche ; deux tableaux imbriqués
        // trahissent une mise en page conçue au pixel.
        Self {
            max_tables: 1,
            max_nesting: 2,
            max_inline_styles: 12,
        }
    }
}

impl AdaptiveRenderer {
    pub fn new(simple: Box<dyn HtmlRenderer>) -> Self {
        Self {
            simple,
            complete: None,
            threshold: ComplexityThreshold::default(),
        }
    }

    pub fn with_full_engine(mut self, engine: Box<dyn HtmlRenderer>) -> Self {
        self.complete = Some(engine);
        self
    }

    /// Le message a-t-il une mise en page qui mérite un moteur complet ?
    pub fn needs_full_engine(&self, html: &str) -> bool {
        let mesure = measure_complexity(html);
        mesure.tables > self.threshold.max_tables
            || mesure.table_nesting > self.threshold.max_nesting
            || mesure.inline_styles > self.threshold.max_inline_styles
    }
}

impl HtmlRenderer for AdaptiveRenderer {
    fn render(&self, sanitized_html: &str, width: f32) -> Result<Rendered> {
        match &self.complete {
            Some(moteur) if self.needs_full_engine(sanitized_html) => {
                // Un moteur complet peut échouer sur du HTML tordu ; le repli sur le
                // texte riche vaut toujours mieux qu'un panneau vide.
                match moteur.render(sanitized_html, width) {
                    Ok(r) => Ok(r),
                    Err(e) => {
                        tracing::warn!(erreur = %e, "moteur complet en échec, repli sur le texte riche");
                        self.simple.render(sanitized_html, width)
                    }
                }
            }
            _ => self.simple.render(sanitized_html, width),
        }
    }

    fn name(&self) -> &'static str {
        "adaptatif"
    }

    fn is_full_fidelity(&self) -> bool {
        self.complete.as_ref().is_some_and(|m| m.is_full_fidelity())
    }
}

/// Mesure de la complexité de mise en page d'un fragment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Complexity {
    pub tables: usize,
    pub table_nesting: usize,
    pub inline_styles: usize,
    pub images: usize,
}

/// Compte les indices de mise en page d'un fragment HTML.
pub fn measure_complexity(html: &str) -> Complexity {
    let minuscules = html.to_lowercase();
    let mut mesure = Complexity::default();

    let mut profondeur = 0usize;
    let mut position = 0usize;
    while position < minuscules.len() {
        let Some(rel) = minuscules[position..].find('<') else {
            break;
        };
        let debut = position + rel;
        let reste = &minuscules[debut..];

        if reste.starts_with("<table") {
            mesure.tables += 1;
            profondeur += 1;
            mesure.table_nesting = mesure.table_nesting.max(profondeur);
        } else if reste.starts_with("</table") {
            profondeur = profondeur.saturating_sub(1);
        } else if reste.starts_with("<img") {
            mesure.images += 1;
        }
        position = debut + 1;
    }

    mesure.inline_styles = minuscules.matches("style=").count();
    mesure
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct MoteurComplet {
        echoue: bool,
    }

    impl HtmlRenderer for MoteurComplet {
        fn render(&self, _html: &str, _width: f32) -> Result<Rendered> {
            if self.echoue {
                return Err(iris_types::Error::other("moteur en panne"));
            }
            Ok(Rendered::Texture {
                width: 800,
                height: 600,
                rgba: vec![0; 4],
            })
        }
        fn name(&self) -> &'static str {
            "complet"
        }
        fn is_full_fidelity(&self) -> bool {
            true
        }
    }

    fn adaptatif(echoue: bool) -> AdaptiveRenderer {
        AdaptiveRenderer::new(Box::new(RichTextRenderer))
            .with_full_engine(Box::new(MoteurComplet { echoue }))
    }

    #[test]
    fn un_message_simple_reste_en_texte_riche() {
        // La quasi-totalité de ce qu'on lit vraiment.
        let r = adaptatif(false);
        let html = "<p>Bonjour,</p><p>Voici le devis demandé.</p>";
        assert!(!r.needs_full_engine(html));
        assert!(r.render(html, 800.0).unwrap().as_blocks().is_some());
    }

    #[test]
    fn une_infolettre_en_tableaux_imbriques_appelle_le_moteur_complet() {
        let r = adaptatif(false);
        let html = "<table><tr><td><table><tr><td>Contenu</td></tr></table></td></tr></table>";
        assert!(r.needs_full_engine(html));
        assert!(matches!(
            r.render(html, 800.0).unwrap(),
            Rendered::Texture { .. }
        ));
    }

    #[test]
    fn un_tableau_isole_reste_lisible_en_texte_riche() {
        let r = adaptatif(false);
        let html = "<table><tr><td>Une</td><td>ligne</td></tr></table>";
        assert!(!r.needs_full_engine(html));
    }

    #[test]
    fn un_moteur_complet_en_panne_retombe_sur_le_texte_riche() {
        // Un panneau vide serait pire qu'un rendu approximatif.
        let r = adaptatif(true);
        let html = "<table><tr><td><table><tr><td>x</td></tr></table></td></tr></table>";
        assert!(r.render(html, 800.0).unwrap().as_blocks().is_some());
    }

    #[test]
    fn sans_moteur_complet_tout_passe_par_le_texte_riche() {
        let r = AdaptiveRenderer::new(Box::new(RichTextRenderer));
        let html = "<table><tr><td><table><tr><td>x</td></tr></table></td></tr></table>";
        assert!(r.render(html, 800.0).unwrap().as_blocks().is_some());
        assert!(!r.is_full_fidelity());
    }

    #[test]
    fn la_complexite_se_mesure() {
        let mesure = measure_complexity(
            r#"<table><tr><td style="x"><table><tr><td><img src="a"></td></tr></table></td></tr></table>"#,
        );
        assert_eq!(mesure.tables, 2);
        assert_eq!(mesure.table_nesting, 2);
        assert_eq!(mesure.images, 1);
        assert_eq!(mesure.inline_styles, 1);
    }

    #[test]
    fn un_fragment_vide_a_une_complexite_nulle() {
        assert_eq!(measure_complexity(""), Complexity::default());
        assert_eq!(measure_complexity("Bonjour"), Complexity::default());
    }

    #[test]
    fn beaucoup_de_styles_en_ligne_trahissent_une_mise_en_page() {
        let r = adaptatif(false);
        let html: String = (0..20)
            .map(|i| format!("<div style=\"color:#{i:03}\">x</div>"))
            .collect();
        assert!(r.needs_full_engine(&html));
    }

    #[test]
    fn un_tableau_non_ferme_ne_fait_pas_boucler_la_mesure() {
        let mesure = measure_complexity("<table><tr><td>oublié");
        assert_eq!(mesure.tables, 1);
    }
}
