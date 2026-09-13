//! `iris-theme` — les design tokens et leur rechargement à chaud.
//!
//! Un thème est **un fichier, pas du code**. Couleurs, rayons, espacements,
//! typographie, densité, durées d'animation et paramètres du verre vivent dans un
//! document TOML que l'application relit sans redémarrer.
//!
//! Trois thèmes sont livrés — `mono` (défaut), `ice`, `sand` — et ne sont rien de
//! plus que trois fichiers parmi d'autres. C'est la preuve, dès la première version,
//! que le système de tokens fonctionne : si le thème par défaut avait le moindre
//! privilège dans le code, la modularité annoncée serait une fiction.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod color;
mod tokens;
mod watch;

pub use color::Color;
pub use tokens::{
    ColorTokens, DensityTokens, GlassTokens, MotionTokens, RadiusTokens, SpacingTokens, Theme,
    TypographyTokens,
};
pub use watch::ThemeWatcher;

use iris_types::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Les thèmes livrés avec l'application.
const BUILTIN: &[(&str, &str)] = &[
    ("mono", include_str!("../themes/mono.toml")),
    ("ice", include_str!("../themes/ice.toml")),
    ("sand", include_str!("../themes/sand.toml")),
];

pub const DEFAULT_THEME: &str = "mono";

/// Le registre des thèmes disponibles et du thème actif.
#[derive(Debug)]
pub struct ThemeRegistry {
    themes: RwLock<BTreeMap<String, Arc<Theme>>>,
    active: RwLock<Arc<Theme>>,
    /// Répertoire des thèmes de l'utilisateur, surveillé si présent.
    user_dir: Option<PathBuf>,
}

impl ThemeRegistry {
    /// Registre ne contenant que les thèmes livrés.
    pub fn builtin() -> Result<Self> {
        let mut themes = BTreeMap::new();
        for (name, source) in BUILTIN {
            let theme = Theme::from_toml(source)
                .map_err(|e| Error::Config(format!("thème livré « {name} » : {e}")))?;
            themes.insert((*name).to_string(), Arc::new(theme));
        }

        let active = Arc::clone(
            themes
                .get(DEFAULT_THEME)
                .ok_or_else(|| Error::Config("thème par défaut absent".into()))?,
        );

        Ok(Self { themes: RwLock::new(themes), active: RwLock::new(active), user_dir: None })
    }

    /// Registre chargeant en plus les thèmes d'un répertoire utilisateur.
    ///
    /// Un thème utilisateur portant le nom d'un thème livré le remplace : c'est ce
    /// qui permet de retoucher `mono` sans le renommer.
    pub fn with_user_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let mut registry = Self::builtin()?;
        registry.user_dir = Some(dir.as_ref().to_path_buf());
        registry.reload_user_themes()?;
        Ok(registry)
    }

    /// Relit les thèmes du répertoire utilisateur.
    ///
    /// Un fichier illisible est signalé et ignoré : une faute de frappe dans un thème
    /// ne doit jamais empêcher l'application de démarrer, elle doit seulement laisser
    /// le thème précédent en place.
    pub fn reload_user_themes(&self) -> Result<Vec<String>> {
        let Some(dir) = &self.user_dir else { return Ok(Vec::new()) };
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut charges = Vec::new();
        let mut erreurs = Vec::new();

        for entree in std::fs::read_dir(dir)? {
            let chemin = entree?.path();
            if chemin.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let source = match std::fs::read_to_string(&chemin) {
                Ok(s) => s,
                Err(e) => {
                    erreurs.push(format!("{} : {e}", chemin.display()));
                    continue;
                }
            };
            match Theme::from_toml(&source) {
                Ok(theme) => {
                    let nom = theme.name.clone();
                    self.themes
                        .write()
                        .expect("registre empoisonné")
                        .insert(nom.clone(), Arc::new(theme));
                    charges.push(nom);
                }
                Err(e) => erreurs.push(format!("{} : {e}", chemin.display())),
            }
        }

        for e in &erreurs {
            tracing::warn!(erreur = %e, "thème ignoré");
        }

        // Le thème actif a pu être rechargé : on le remplace par sa nouvelle version.
        let nom_actif = self.active().name.clone();
        if let Some(nouveau) = self.themes.read().expect("registre").get(&nom_actif) {
            *self.active.write().expect("thème actif") = Arc::clone(nouveau);
        }

        Ok(charges)
    }

    /// Le thème actif. Copie de pointeur : appelable à chaque frame sans coût.
    pub fn active(&self) -> Arc<Theme> {
        Arc::clone(&self.active.read().expect("thème actif empoisonné"))
    }

    /// Change le thème actif.
    pub fn set_active(&self, name: &str) -> Result<Arc<Theme>> {
        let themes = self.themes.read().expect("registre empoisonné");
        let theme = themes
            .get(name)
            .ok_or_else(|| Error::Config(format!("thème « {name} » inconnu")))?;
        let theme = Arc::clone(theme);
        *self.active.write().expect("thème actif empoisonné") = Arc::clone(&theme);
        Ok(theme)
    }

    /// Noms des thèmes disponibles, dans l'ordre alphabétique.
    pub fn names(&self) -> Vec<String> {
        self.themes.read().expect("registre empoisonné").keys().cloned().collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<Theme>> {
        self.themes.read().expect("registre empoisonné").get(name).map(Arc::clone)
    }

    pub fn user_dir(&self) -> Option<&Path> {
        self.user_dir.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_trois_themes_livres_se_chargent() {
        let r = ThemeRegistry::builtin().unwrap();
        assert_eq!(r.names(), ["ice", "mono", "sand"]);
    }

    #[test]
    fn le_theme_par_defaut_est_mono() {
        let r = ThemeRegistry::builtin().unwrap();
        assert_eq!(r.active().name, "mono");
        assert!(r.active().dark);
    }

    #[test]
    fn le_theme_par_defaut_n_a_pas_de_teinte() {
        // Propriété du thème « mono » : la hiérarchie ne repose que sur la
        // luminosité. L'accent n'est pas mathématiquement gris — un blanc très
        // légèrement froid se lit mieux sur fond sombre — mais l'écart entre canaux
        // doit rester imperceptible.
        let r = ThemeRegistry::builtin().unwrap();
        let a = r.active().color.accent;
        let ecart = [a.r, a.g, a.b].iter().max().unwrap() - [a.r, a.g, a.b].iter().min().unwrap();
        assert!(ecart <= 8, "l'accent doit rester neutre, écart de {ecart} entre canaux");
    }

    #[test]
    fn les_deux_autres_themes_ont_un_accent_teinte() {
        let r = ThemeRegistry::builtin().unwrap();
        for nom in ["ice", "sand"] {
            let a = r.get(nom).unwrap().color.accent;
            assert!(a.r != a.b, "« {nom} » doit avoir un accent teinté");
        }
    }

    #[test]
    fn changer_de_theme_change_l_actif() {
        let r = ThemeRegistry::builtin().unwrap();
        r.set_active("sand").unwrap();
        assert_eq!(r.active().name, "sand");
    }

    #[test]
    fn un_theme_inconnu_est_refuse() {
        let r = ThemeRegistry::builtin().unwrap();
        let e = r.set_active("fluo").unwrap_err();
        assert!(e.to_string().contains("inconnu"));
        assert_eq!(r.active().name, "mono", "l'actif ne change pas en cas d'échec");
    }

    #[test]
    fn un_theme_utilisateur_est_charge() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("nuit.toml"),
            "name = \"nuit\"\nlabel = \"Nuit\"\ndark = true\n",
        )
        .unwrap();

        let r = ThemeRegistry::with_user_dir(dir.path()).unwrap();
        assert!(r.names().contains(&"nuit".to_string()));
        // Les valeurs absentes retombent sur celles du thème par défaut.
        assert_eq!(r.get("nuit").unwrap().radius.medium, 9.0);
    }

    #[test]
    fn un_theme_utilisateur_remplace_un_theme_livre_du_meme_nom() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mono.toml"),
            "name = \"mono\"\nlabel = \"Mono retouché\"\n[density]\nrow_height = 40.0\n",
        )
        .unwrap();

        let r = ThemeRegistry::with_user_dir(dir.path()).unwrap();
        assert_eq!(r.get("mono").unwrap().density.row_height, 40.0);
        assert_eq!(r.active().density.row_height, 40.0, "l'actif suit le rechargement");
    }

    #[test]
    fn un_theme_illisible_est_ignore_sans_bloquer_le_demarrage() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("casse.toml"), "ceci n'est = = pas du toml").unwrap();
        std::fs::write(dir.path().join("bon.toml"), "name = \"bon\"\n").unwrap();

        let r = ThemeRegistry::with_user_dir(dir.path()).unwrap();
        assert!(r.names().contains(&"bon".to_string()));
        assert!(!r.names().contains(&"casse".to_string()));
        assert_eq!(r.active().name, "mono");
    }

    #[test]
    fn un_repertoire_absent_n_est_pas_une_erreur() {
        let dir = tempfile::tempdir().unwrap();
        let inexistant = dir.path().join("pas-la");
        let r = ThemeRegistry::with_user_dir(&inexistant).unwrap();
        assert_eq!(r.names().len(), 3);
    }

    #[test]
    fn les_fichiers_non_toml_sont_ignores() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "name = \"pirate\"").unwrap();
        let r = ThemeRegistry::with_user_dir(dir.path()).unwrap();
        assert_eq!(r.names().len(), 3);
    }
}
