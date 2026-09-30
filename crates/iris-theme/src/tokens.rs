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
    pub motion: MotionTokens,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "unnamed".into(),
            label: "Unnamed".into(),
            dark: false,
            color: ColorTokens::default(),
            radius: RadiusTokens::default(),
            spacing: SpacingTokens::default(),
            typography: TypographyTokens::default(),
            density: DensityTokens::default(),
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

        // Muted text still has to be read: dates, counts, second lines.
        let discret = sur_fond(self.color.text_muted).contrast_ratio(fond);
        if discret < 3.0 {
            avertissements.push(format!(
                "le texte discret n'a qu'un contraste de {discret:.1} sur le fond"
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
    /// The window's ground.
    pub background: Color,
    /// The side columns: accounts, calendars, task lists, the rail.
    pub surface_low: Color,
    /// Lists, the reader, cards.
    pub surface: Color,
    /// Raised above the rest.
    pub surface_high: Color,
    /// What floats: menus, popovers, windows.
    pub panel: Color,
    pub surface_hover: Color,
    pub surface_active: Color,
    pub border: Color,
    pub border_strong: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    /// Placeholders, what is past, what cannot be used.
    pub text_faint: Color,
    pub text_inverse: Color,
    /// The one colour for what to act on.
    pub accent: Color,
    /// The accent as a ground: the chosen row, a filter that is on.
    pub accent_soft: Color,
    pub accent_text: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
}

impl Default for ColorTokens {
    /// The light theme's values.
    fn default() -> Self {
        Self {
            background: Color::rgb(0xf7, 0xf7, 0xf6),
            surface_low: Color::rgb(0xf0, 0xf0, 0xee),
            surface: Color::rgb(0xff, 0xff, 0xff),
            surface_high: Color::rgb(0xff, 0xff, 0xff),
            panel: Color::rgb(0xff, 0xff, 0xff),
            surface_hover: Color::rgba(0x14, 0x14, 0x1e, 0x0b),
            surface_active: Color::rgba(0x14, 0x14, 0x1e, 0x13),
            border: Color::rgb(0xe4, 0xe4, 0xe2),
            border_strong: Color::rgb(0xd3, 0xd3, 0xd0),
            text: Color::rgb(0x18, 0x18, 0x1b),
            text_secondary: Color::rgb(0x4a, 0x4a, 0x52),
            text_muted: Color::rgb(0x6e, 0x6e, 0x77),
            text_faint: Color::rgb(0xa9, 0xa9, 0xb0),
            text_inverse: Color::rgb(0xff, 0xff, 0xff),
            accent: Color::rgb(0x2c, 0x62, 0xe8),
            accent_soft: Color::rgba(0x2c, 0x62, 0xe8, 0x17),
            accent_text: Color::rgb(0xff, 0xff, 0xff),
            error: Color::rgb(0xcc, 0x35, 0x27),
            warning: Color::rgb(0xa8, 0x66, 0x0c),
            success: Color::rgb(0x1d, 0x8a, 0x4e),
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
            large: 12.0,
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
            // « Segoe UI Variable Text », et non « Inter ».
            //
            // Inter était nommée dans les cinq thèmes et n'est installée sur aucune
            // machine Windows : l'application dessinait donc depuis toujours avec la
            // police de repli, et le jeton décrivait une intention que personne ne
            // voyait. Segoe UI Variable Text est livrée avec Windows 11, dessinée pour
            // les tailles d'interface — de douze à vingt-quatre pixels, exactement la
            // plage d'ici — et plus étroite qu'Inter, donc le changement resserre au
            // lieu de faire déborder.
            family: "Segoe UI Variable Text".into(),
            family_mono: "Cascadia Mono".into(),
            size_small: 12.0,
            size_body: 13.5,
            size_title: 15.0,
            weight_body: 400,
            weight_bold: 600,
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
            // Two lines and a face, with room around them: the mail list's row.
            row_height: 56.0,
            row_padding_x: 14.0,
        }
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
        assert_eq!(t.typography.family, "Segoe UI Variable Text");
        assert_eq!(t.color.background, Color::rgb(0xf7, 0xf7, 0xf6));
    }

    #[test]
    fn un_theme_ne_change_que_ce_qu_il_declare() {
        let t = Theme::from_toml("name = \"dense\"\n[density]\nrow_height = 34.0\n").unwrap();
        assert_eq!(t.density.row_height, 34.0);
        assert_eq!(t.density.row_padding_x, 14.0, "le reste est hérité");
    }

    #[test]
    fn une_couleur_invalide_est_signalee_avec_son_nom() {
        let e = Theme::from_toml("[color]\nbackground = \"pas une couleur\"").unwrap_err();
        assert!(e.to_string().contains("couleur invalide"));
    }

    #[test]
    fn un_theme_survit_a_un_aller_retour_toml() {
        let t = Theme::from_toml(include_str!("../themes/dark.toml")).unwrap();
        let relu = Theme::from_toml(&t.to_toml().unwrap()).unwrap();
        assert_eq!(t, relu);
    }

    #[test]
    fn le_theme_clair_est_la_valeur_par_defaut() {
        // The defaults are the light theme's: a file that says nothing is light.
        let t = Theme::from_toml(include_str!("../themes/light.toml")).unwrap();
        assert_eq!(t.color, ColorTokens::default());
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
    fn une_ligne_trop_basse_est_signalee() {
        let t = Theme::from_toml("name = \"minus\"\n[density]\nrow_height = 12.0\n").unwrap();
        assert!(t.lint().iter().any(|a| a.contains("hauteur de ligne")));
    }

    #[test]
    fn l_espacement_est_un_multiple_de_l_unite() {
        let t = Theme::default();
        assert_eq!(t.space(1.0), 4.0);
        assert_eq!(t.space(3.0), 12.0);
    }
}
