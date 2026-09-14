//! Les réglages de l'utilisateur.
//!
//! Trois choses seulement, parce que ce sont les trois que l'utilisateur a demandé à
//! pouvoir changer : **le thème**, **la densité** de la liste, et **les automatismes**
//! du flux de travail. Tout le reste est décidé par le thème lui-même, qui est un
//! fichier, et se modifie en le rouvrant.
//!
//! Le fichier vit dans le répertoire de configuration, séparé des données : le perdre
//! ne coûte que de refaire trois clics.
//!
//! Un fichier illisible ne bloque pas le démarrage. Refuser d'ouvrir l'application
//! parce qu'une virgule manque dans un réglage cosmétique serait une punition
//! disproportionnée ; on repart des valeurs par défaut et on le dit dans le journal.

use iris_types::{AutomationSettings, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// La densité de la liste.
///
/// Elle multiplie la hauteur de ligne du thème plutôt que de la remplacer : un thème
/// qui a choisi des lignes hautes reste plus aéré que les autres à densité égale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// Le maximum de lignes à l'écran : pour trier vite.
    Compact,
    #[default]
    Normal,
    /// Pour lire longtemps.
    Comfortable,
}

impl Density {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Normal, Self::Comfortable];

    pub fn factor(self) -> f32 {
        match self {
            Self::Compact => 0.72,
            Self::Normal => 1.0,
            Self::Comfortable => 1.2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compacte",
            Self::Normal => "Normale",
            Self::Comfortable => "Confortable",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Compact => 0,
            Self::Normal => 1,
            Self::Comfortable => 2,
        }
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// Les réglages, tels qu'ils sont écrits sur le disque.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Nom du thème actif.
    pub theme: String,
    pub density: Density,
    pub automation: AutomationSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "mono".into(),
            density: Density::default(),
            automation: AutomationSettings::default(),
        }
    }
}

impl Settings {
    /// Lit les réglages. Un fichier absent ou illisible rend les valeurs par défaut.
    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let Ok(texte) = std::fs::read_to_string(path) else {
            return Self::default();
        };

        match toml::from_str(&texte) {
            Ok(reglages) => Self::sanitize(reglages),
            Err(e) => {
                tracing::warn!(fichier = %path.display(), erreur = %e, "réglages illisibles");
                Self::default()
            }
        }
    }

    /// Écrit les réglages, en créant le répertoire au besoin.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let texte = toml::to_string_pretty(self)
            .map_err(|e| iris_types::Error::Config(format!("réglages : {e}")))?;
        std::fs::write(path, texte)?;
        Ok(())
    }

    /// Ramène les valeurs aberrantes dans le domaine du raisonnable.
    ///
    /// Un délai de relance de zéro jour ramènerait tous les fils en attente au
    /// premier passage de maintenance ; un délai de dix ans est une désactivation
    /// déguisée, qui a déjà son interrupteur.
    fn sanitize(mut self) -> Self {
        self.automation.follow_up_days = self.automation.follow_up_days.clamp(1, 365);
        if self.theme.trim().is_empty() {
            self.theme = Self::default().theme;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fichier() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("config").join("iris.toml");
        (dir, chemin)
    }

    #[test]
    fn un_fichier_absent_rend_les_valeurs_par_defaut() {
        let (_d, chemin) = fichier();
        assert_eq!(Settings::load(&chemin), Settings::default());
    }

    #[test]
    fn les_reglages_font_l_aller_retour() {
        let (_d, chemin) = fichier();
        let reglages = Settings {
            theme: "ice".into(),
            density: Density::Compact,
            automation: AutomationSettings::MANUAL_ONLY,
        };

        reglages.save(&chemin).unwrap();
        assert_eq!(Settings::load(&chemin), reglages);
    }

    #[test]
    fn un_fichier_illisible_ne_bloque_pas_le_demarrage() {
        // Refuser d'ouvrir l'application pour une virgule manquante dans un réglage
        // cosmétique serait disproportionné.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "ceci = = n'est pas du toml").unwrap();

        assert_eq!(Settings::load(&chemin), Settings::default());
    }

    #[test]
    fn un_delai_de_relance_nul_est_ramene_a_un_jour() {
        // Sinon tous les fils en attente reviendraient au premier passage.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "[automation]\nfollow_up_days = 0\n").unwrap();

        assert_eq!(Settings::load(&chemin).automation.follow_up_days, 1);
    }

    #[test]
    fn un_delai_absurde_est_borne() {
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "[automation]\nfollow_up_days = 5000\n").unwrap();

        assert_eq!(Settings::load(&chemin).automation.follow_up_days, 365);
    }

    #[test]
    fn un_theme_vide_revient_au_defaut() {
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "theme = \"  \"\n").unwrap();

        assert_eq!(Settings::load(&chemin).theme, "mono");
    }

    #[test]
    fn un_fichier_partiel_garde_les_defauts_pour_le_reste() {
        // Ajouter un réglage plus tard ne doit pas invalider les fichiers existants.
        let (_d, chemin) = fichier();
        std::fs::create_dir_all(chemin.parent().unwrap()).unwrap();
        std::fs::write(&chemin, "theme = \"sand\"\n").unwrap();

        let reglages = Settings::load(&chemin);
        assert_eq!(reglages.theme, "sand");
        assert_eq!(reglages.density, Density::Normal);
        assert_eq!(reglages.automation, AutomationSettings::default());
    }

    #[test]
    fn l_ecriture_cree_le_repertoire_manquant() {
        let (_d, chemin) = fichier();
        assert!(!chemin.parent().unwrap().exists());
        Settings::default().save(&chemin).unwrap();
        assert!(chemin.exists());
    }

    #[test]
    fn la_densite_multiplie_au_lieu_de_remplacer() {
        // Un thème aux lignes hautes doit rester plus aéré que les autres à densité
        // égale.
        assert!(Density::Compact.factor() < Density::Normal.factor());
        assert!(Density::Comfortable.factor() > Density::Normal.factor());
        assert_eq!(Density::Normal.factor(), 1.0);
    }

    #[test]
    fn les_densites_font_l_aller_retour_par_leur_indice() {
        for densite in Density::ALL {
            assert_eq!(Density::from_index(densite.index()), Some(densite));
        }
        assert_eq!(Density::from_index(9), None);
    }
}
