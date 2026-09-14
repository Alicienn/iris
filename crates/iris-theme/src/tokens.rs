//! Les design tokens.
//!
//! Chaque groupe implémente `Default` avec les valeurs du thème livré par défaut :
//! un fichier de thème n'a donc besoin de déclarer que ce qu'il change. Écrire un
//! thème qui ne modifie que trois couleurs doit tenir en cinq lignes.

use crate::color::Color;
use iris_types::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Theme {
    pub name: String,
    /// Nom présenté à l'utilisateur.
    pub label: String,
    /// Indique au système quelle variante de décoration de fenêtre demander.
    pub dark: bool,
    pub color: ColorTokens,
    pub radius: RadiusTokens,
    pub spacing: SpacingTokens,
    pub typography: TypographyTokens,
    pub density: DensityTokens,
    pub glass: GlassTokens,
    pub motion: MotionTokens,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "sans-nom".into(),
            label: "Sans nom".into(),
            dark: true,
            color: ColorTokens::default(),
            radius: RadiusTokens::default(),
            spacing: SpacingTokens::default(),
            typography: TypographyTokens::default(),
            density: DensityTokens::default(),
            glass: GlassTokens::default(),
            motion: MotionTokens::default(),
        }
    }
}

impl Theme {
    pub fn from_toml(source: &str) -> Result<Self> {
        toml::from_str(source).map_err(|e| Error::Config(format!("thème invalide : {e}")))
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("sérialisation du thème : {e}")))
    }

    /// Vérifie qu'un thème reste utilisable.
    ///
    /// Un thème est un fichier que n'importe qui peut écrire : rien n'empêche d'y
    /// mettre du gris sur gris, un flou de mille pixels ou une hauteur de ligne
    /// nulle. Ces contrôles ne corrigent rien, ils signalent — c'est à l'auteur du
    /// thème de décider.
    pub fn lint(&self) -> Vec<String> {
        let mut avertissements = Vec::new();

        let fond = self.color.background;
        let sur_fond = |c: Color| c.over(fond);

        let contraste = sur_fond(self.color.text).contrast_ratio(fond);
        if contraste < 4.5 {
            avertissements.push(format!(
                "le texte principal n'a qu'un contraste de {contraste:.1} sur le fond \
                 (4,5 est le minimum pour du texte courant)"
            ));
        }

        let secondaire = sur_fond(self.color.text_secondary).contrast_ratio(fond);
        if secondaire < 3.0 {
            avertissements.push(format!(
                "le texte secondaire n'a qu'un contraste de {secondaire:.1} sur le fond"
            ));
        }

        if self.glass.blur > 80.0 {
            avertissements.push(format!(
                "un flou de {} pixels coûtera cher à chaque frame",
                self.glass.blur
            ));
        }

        if self.density.row_height < 24.0 {
            avertissements.push(format!(
                "une hauteur de ligne de {} pixels rend la liste inutilisable à la souris",
                self.density.row_height
            ));
        }

        if self.motion.moderate > 600.0 {
            avertissements
                .push("des animations de plus de 600 ms donnent une impression de lenteur".into());
        }

        avertissements
    }

    /// Espacement pour un multiple de l'unité de base.
    pub fn space(&self, multiple: f32) -> f32 {
        self.spacing.unit * multiple
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorTokens {
    pub background: Color,
    pub glow: Color,
    pub surface_high: Color,
    pub surface: Color,
    pub surface_low: Color,
    pub surface_hover: Color,
    pub surface_active: Color,
    pub border: Color,
    pub border_strong: Color,
    pub edge_light: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_inverse: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
}

impl Default for ColorTokens {
    fn default() -> Self {
        Self {
            background: Color::rgb(0x0a, 0x0b, 0x0d),
            glow: Color::rgba(0xff, 0xff, 0xff, 0x0d),
            surface_high: Color::rgba(0xff, 0xff, 0xff, 0x0e),
            surface: Color::rgba(0xff, 0xff, 0xff, 0x06),
            surface_low: Color::rgba(0xff, 0xff, 0xff, 0x03),
            surface_hover: Color::rgba(0xff, 0xff, 0xff, 0x10),
            surface_active: Color::rgba(0xff, 0xff, 0xff, 0x17),
            border: Color::rgba(0xff, 0xff, 0xff, 0x13),
            border_strong: Color::rgba(0xff, 0xff, 0xff, 0x26),
            edge_light: Color::rgba(0xff, 0xff, 0xff, 0x1f),
            text: Color::rgb(0xf0, 0xf2, 0xf6),
            text_secondary: Color::rgb(0x94, 0x9b, 0xab),
            text_muted: Color::rgb(0x5a, 0x60, 0x70),
            text_inverse: Color::rgb(0x0a, 0x0b, 0x0d),
            accent: Color::rgb(0xf0, 0xf2, 0xf6),
            accent_text: Color::rgb(0x0a, 0x0b, 0x0d),
            error: Color::rgb(0xe5, 0x48, 0x4d),
            warning: Color::rgb(0xd9, 0xa4, 0x41),
            success: Color::rgb(0x5c, 0xb8, 0x7a),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RadiusTokens {
    pub small: f32,
    pub medium: f32,
    pub large: f32,
    pub pill: f32,
}

impl Default for RadiusTokens {
    fn default() -> Self {
        Self {
            small: 6.0,
            medium: 9.0,
            large: 14.0,
            pill: 999.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpacingTokens {
    /// Unité de base. Toute marge de l'interface en est un multiple.
    pub unit: f32,
}

impl Default for SpacingTokens {
    fn default() -> Self {
        Self { unit: 4.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TypographyTokens {
    pub family: String,
    pub family_mono: String,
    pub size_small: f32,
    pub size_body: f32,
    pub size_title: f32,
    pub weight_body: u16,
    pub weight_bold: u16,
    /// Interlettrage, en em.
    pub tracking: f32,
    pub line_height: f32,
}

impl Default for TypographyTokens {
    fn default() -> Self {
        Self {
            family: "Inter".into(),
            family_mono: "JetBrains Mono".into(),
            size_small: 11.0,
            size_body: 13.0,
            size_title: 15.0,
            weight_body: 400,
            weight_bold: 620,
            tracking: -0.01,
            line_height: 1.45,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DensityTokens {
    pub row_height: f32,
    pub row_padding_x: f32,
}

impl Default for DensityTokens {
    fn default() -> Self {
        Self {
            row_height: 58.0,
            row_padding_x: 12.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlassTokens {
    /// Rayon de flou des panneaux fixes.
    pub blur: f32,
    /// Rayon de flou des surfaces flottantes.
    pub blur_floating: f32,
    pub opacity: f32,
    pub saturation: f32,
    /// Amplitude du bruit appliqué au verre, de 0 à 1.
    pub grain: f32,
}

impl Default for GlassTokens {
    fn default() -> Self {
        Self {
            blur: 30.0,
            blur_floating: 24.0,
            opacity: 0.86,
            saturation: 1.15,
            grain: 0.035,
        }
    }
}

impl GlassTokens {
    /// Le verre est-il désactivé ? Un flou nul permet de retomber sur des surfaces
    /// opaques, sans autre changement de thème.
    pub fn is_disabled(&self) -> bool {
        self.blur <= 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionTokens {
    pub instant: f32,
    pub quick: f32,
    pub moderate: f32,
    pub easing: String,
}

impl Default for MotionTokens {
    fn default() -> Self {
        Self {
            instant: 90.0,
            quick: 140.0,
            moderate: 220.0,
            easing: "cubic-bezier(0.2, 0, 0, 1)".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_theme_minimal_herite_de_tout_le_reste() {
        // Écrire un thème qui ne change que son nom doit suffire.
        let t = Theme::from_toml("name = \"essai\"").unwrap();
        assert_eq!(t.name, "essai");
        assert_eq!(t.radius.medium, 9.0);
        assert_eq!(t.typography.family, "Inter");
        assert_eq!(t.color.background, Color::rgb(0x0a, 0x0b, 0x0d));
    }

    #[test]
    fn un_theme_ne_change_que_ce_qu_il_declare() {
        let t = Theme::from_toml("name = \"dense\"\n[density]\nrow_height = 34.0\n").unwrap();
        assert_eq!(t.density.row_height, 34.0);
        assert_eq!(t.density.row_padding_x, 12.0, "le reste est hérité");
    }

    #[test]
    fn une_couleur_invalide_est_signalee_avec_son_nom() {
        let e = Theme::from_toml("[color]\nbackground = \"pas une couleur\"").unwrap_err();
        assert!(e.to_string().contains("couleur invalide"));
    }

    #[test]
    fn un_theme_survit_a_un_aller_retour_toml() {
        let t = Theme::from_toml(include_str!("../themes/mono.toml")).unwrap();
        let relu = Theme::from_toml(&t.to_toml().unwrap()).unwrap();
        assert_eq!(t, relu);
    }

    #[test]
    fn les_themes_livres_passent_leur_propre_verification() {
        for source in [
            include_str!("../themes/mono.toml"),
            include_str!("../themes/ice.toml"),
            include_str!("../themes/sand.toml"),
        ] {
            let t = Theme::from_toml(source).unwrap();
            assert!(t.lint().is_empty(), "« {} » : {:?}", t.name, t.lint());
        }
    }

    #[test]
    fn un_theme_illisible_est_signale() {
        let t = Theme::from_toml(
            "name = \"gris\"\n[color]\nbackground = \"#808080\"\ntext = \"#8a8a8a\"\n",
        )
        .unwrap();
        let avertissements = t.lint();
        assert!(avertissements.iter().any(|a| a.contains("texte principal")));
    }

    #[test]
    fn un_flou_excessif_est_signale() {
        let t = Theme::from_toml("name = \"flou\"\n[glass]\nblur = 200.0\n").unwrap();
        assert!(t.lint().iter().any(|a| a.contains("flou")));
    }

    #[test]
    fn une_ligne_trop_basse_est_signalee() {
        let t = Theme::from_toml("name = \"minus\"\n[density]\nrow_height = 12.0\n").unwrap();
        assert!(t.lint().iter().any(|a| a.contains("hauteur de ligne")));
    }

    #[test]
    fn le_verre_se_desactive_par_un_flou_nul() {
        let t = Theme::from_toml("name = \"plat\"\n[glass]\nblur = 0.0\n").unwrap();
        assert!(t.glass.is_disabled());
        assert!(!Theme::default().glass.is_disabled());
    }

    #[test]
    fn l_espacement_est_un_multiple_de_l_unite() {
        let t = Theme::default();
        assert_eq!(t.space(1.0), 4.0);
        assert_eq!(t.space(3.0), 12.0);
    }
}
