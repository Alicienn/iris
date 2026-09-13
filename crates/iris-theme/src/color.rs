//! Couleurs.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Une couleur RVBA, en composantes de 0 à 255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Self = Self { r: 0, g: 0, b: 0, a: 0 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Analyse `#RGB`, `#RGBA`, `#RRGGBB` ou `#RRGGBBAA`.
    ///
    /// Le canal alpha écrit dans la notation hexadécimale est la façon la plus
    /// concise d'exprimer les surfaces de verre, qui sont toutes des blancs à faible
    /// opacité.
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.trim().trim_start_matches('#');
        let chiffre = |c: char| c.to_digit(16).map(|d| d as u8);

        let octets: Vec<u8> = match h.len() {
            3 | 4 => h
                .chars()
                .map(chiffre)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                // `#abc` équivaut à `#aabbcc`.
                .map(|d| d * 17)
                .collect(),
            6 | 8 => {
                let mut v = Vec::with_capacity(4);
                for paire in h.as_bytes().chunks_exact(2) {
                    let hi = chiffre(paire[0] as char)?;
                    let lo = chiffre(paire[1] as char)?;
                    v.push(hi * 16 + lo);
                }
                v
            }
            _ => return None,
        };

        Some(Self {
            r: octets[0],
            g: octets[1],
            b: octets[2],
            a: *octets.get(3).unwrap_or(&255),
        })
    }

    /// Composantes normalisées, telles que les attend un nuanceur.
    pub fn to_f32(self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        ]
    }

    /// Notation `#RRGGBBAA`, l'alpha omis quand il est opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// Luminance relative, selon la recommandation WCAG.
    pub fn luminance(self) -> f32 {
        let canal = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * canal(self.r) + 0.7152 * canal(self.g) + 0.0722 * canal(self.b)
    }

    /// Rapport de contraste avec une autre couleur, de 1 à 21.
    ///
    /// Sert à vérifier qu'un thème reste lisible : un thème est un fichier que
    /// n'importe qui peut écrire, et rien n'empêche d'y mettre du gris sur gris.
    pub fn contrast_ratio(self, other: Self) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (clair, sombre) = if a > b { (a, b) } else { (b, a) };
        (clair + 0.05) / (sombre + 0.05)
    }

    /// Compose cette couleur au-dessus d'une autre, en respectant son alpha.
    pub fn over(self, fond: Self) -> Self {
        let alpha = self.a as f32 / 255.0;
        let melange = |avant: u8, arriere: u8| {
            (avant as f32 * alpha + arriere as f32 * (1.0 - alpha)).round() as u8
        };
        Self {
            r: melange(self.r, fond.r),
            g: melange(self.g, fond.g),
            b: melange(self.b, fond.b),
            a: 255,
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("couleur invalide : « {s} »"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyse_des_formes_hexadecimales() {
        assert_eq!(Color::parse("#ff0000"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(Color::parse("ff0000"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(Color::parse("#f00"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(Color::parse("#ff000080"), Some(Color::rgba(255, 0, 0, 128)));
        assert_eq!(Color::parse("#f008"), Some(Color::rgba(255, 0, 0, 136)));
    }

    #[test]
    fn les_formes_invalides_sont_refusees() {
        assert_eq!(Color::parse("#12345"), None);
        assert_eq!(Color::parse("#zzzzzz"), None);
        assert_eq!(Color::parse(""), None);
        assert_eq!(Color::parse("rouge"), None);
    }

    #[test]
    fn aller_retour_hexadecimal() {
        for s in ["#0a0b0d", "#ffffff13"] {
            assert_eq!(Color::parse(s).unwrap().to_hex(), s);
        }
    }

    #[test]
    fn le_contraste_du_theme_par_defaut_est_lisible() {
        // Le texte principal sur le fond de fenêtre doit dépasser le seuil AA pour du
        // texte courant. Un thème est un fichier : cette vérification doit être
        // mécanisable.
        let fond = Color::parse("#0a0b0d").unwrap();
        let texte = Color::parse("#f0f2f6").unwrap();
        assert!(
            texte.contrast_ratio(fond) >= 4.5,
            "contraste insuffisant : {}",
            texte.contrast_ratio(fond)
        );
    }

    #[test]
    fn le_contraste_est_symetrique() {
        let a = Color::rgb(255, 255, 255);
        let b = Color::rgb(0, 0, 0);
        assert!((a.contrast_ratio(b) - b.contrast_ratio(a)).abs() < 1e-6);
        assert!((a.contrast_ratio(b) - 21.0).abs() < 0.01);
    }

    #[test]
    fn une_couleur_avec_elle_meme_a_un_contraste_de_un() {
        let c = Color::rgb(100, 120, 140);
        assert!((c.contrast_ratio(c) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn la_composition_respecte_l_alpha() {
        let noir = Color::rgb(0, 0, 0);
        let blanc_transparent = Color::rgba(255, 255, 255, 0);
        assert_eq!(blanc_transparent.over(noir), noir);

        let blanc_opaque = Color::rgb(255, 255, 255);
        assert_eq!(blanc_opaque.over(noir), blanc_opaque);

        let demi = Color::rgba(255, 255, 255, 128);
        let resultat = demi.over(noir);
        assert!((resultat.r as i32 - 128).abs() <= 1);
    }

    #[test]
    fn conversion_pour_les_nuanceurs() {
        let c = Color::rgba(255, 0, 128, 51);
        let f = c.to_f32();
        assert!((f[0] - 1.0).abs() < 1e-6);
        assert!((f[1] - 0.0).abs() < 1e-6);
        assert!((f[3] - 0.2).abs() < 0.01);
    }

    #[test]
    fn une_couleur_survit_a_un_aller_retour_json() {
        let c = Color::rgba(10, 20, 30, 40);
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Color>(&json).unwrap(), c);
    }

    #[test]
    fn une_couleur_invalide_produit_une_erreur_lisible() {
        let e = serde_json::from_str::<Color>("\"#zz\"").unwrap_err();
        assert!(e.to_string().contains("couleur invalide"));
    }
}
