//! Les emplacements de données.
//!
//! Trois répertoires, séparés à dessein :
//!
//! - **données** : la base, les thèmes de l'utilisateur, les plugins. Ce qu'il perdra
//!   s'il n'en fait pas de copie ;
//! - **cache** : les corps de messages et l'index. Entièrement reconstructible ;
//! - **configuration** : les réglages.
//!
//! La distinction n'est pas cosmétique : elle dit à l'utilisateur — et aux
//! utilitaires de sauvegarde du système — ce qui mérite d'être sauvegardé.

use iris_types::{Error, Result};
use std::path::{Path, PathBuf};

/// Les chemins de l'application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
    pub cache: PathBuf,
    pub config: PathBuf,
}

impl Paths {
    /// Les emplacements standard du système.
    ///
    /// `IRIS_ROOT` les remplace tous les trois par des sous-répertoires d'une racine
    /// choisie. C'est la porte qui manquait pour *mesurer* : décomposer la mémoire
    /// d'une vraie boîte demande de l'ouvrir, et l'ouvrir pendant que l'application
    /// tourne fait se disputer deux processus sur la même base. Une copie sous une
    /// autre racine répond à la même question sans toucher au courrier de personne.
    pub fn system() -> Result<Self> {
        if let Some(racine) = std::env::var_os("IRIS_ROOT") {
            return Ok(Self::under(racine));
        }

        let dirs = directories::ProjectDirs::from("fr", "Iris", "Iris")
            .ok_or_else(|| Error::Config("répertoire personnel introuvable".into()))?;

        Ok(Self {
            data: dirs.data_dir().to_path_buf(),
            cache: dirs.cache_dir().to_path_buf(),
            config: dirs.config_dir().to_path_buf(),
        })
    }

    /// Tout sous une racine unique. Utile aux tests et à une installation portable.
    pub fn under(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self {
            data: root.join("data"),
            cache: root.join("cache"),
            config: root.join("config"),
        }
    }

    /// Crée les répertoires manquants.
    pub fn ensure(&self) -> Result<()> {
        for chemin in [
            &self.data,
            &self.cache,
            &self.config,
            &self.themes(),
            &self.blobs(),
        ] {
            std::fs::create_dir_all(chemin)?;
        }
        Ok(())
    }

    pub fn database(&self) -> PathBuf {
        self.data.join("iris.db")
    }

    pub fn vault(&self) -> PathBuf {
        self.data.join("secrets.json")
    }

    /// Thèmes de l'utilisateur, surveillés pour le rechargement à chaud.
    pub fn themes(&self) -> PathBuf {
        self.data.join("themes")
    }

    pub fn plugins(&self) -> PathBuf {
        self.data.join("plugins")
    }

    /// Corps et pièces jointes. Dans le cache : tout est retéléchargeable.
    pub fn blobs(&self) -> PathBuf {
        self.cache.join("blobs")
    }

    /// Index plein texte. Dans le cache : reconstructible depuis la base.
    pub fn index(&self) -> PathBuf {
        self.cache.join("index")
    }

    pub fn settings(&self) -> PathBuf {
        self.config.join("iris.toml")
    }

    /// Le message en cours d'écriture, s'il y en a un.
    ///
    /// Dans les données et non dans le cache : un brouillon n'est pas reconstructible.
    pub fn draft(&self) -> PathBuf {
        self.data.join("draft.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_chemins_sous_une_racine_sont_separes() {
        let p = Paths::under("/tmp/iris");
        assert!(p.data.ends_with("data"));
        assert!(p.cache.ends_with("cache"));
        assert!(p.config.ends_with("config"));
    }

    #[test]
    fn ce_qui_est_reconstructible_vit_dans_le_cache() {
        // La distinction dit aux utilitaires de sauvegarde ce qui mérite d'être
        // sauvegardé.
        let p = Paths::under("/tmp/iris");
        assert!(p.blobs().starts_with(&p.cache));
        assert!(p.index().starts_with(&p.cache));
    }

    #[test]
    fn ce_qui_est_irremplacable_vit_dans_les_donnees() {
        let p = Paths::under("/tmp/iris");
        assert!(p.database().starts_with(&p.data));
        assert!(p.vault().starts_with(&p.data));
        assert!(p.themes().starts_with(&p.data));
        assert!(p.plugins().starts_with(&p.data));
    }

    #[test]
    fn la_creation_est_idempotente() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::under(dir.path());
        p.ensure().unwrap();
        p.ensure().unwrap();

        assert!(p.themes().is_dir());
        assert!(p.blobs().is_dir());
    }

    #[test]
    fn les_emplacements_systeme_sont_disponibles() {
        let p = Paths::system().expect("emplacements du système");
        assert!(p.data.is_absolute());
        assert_ne!(p.data, p.cache, "les données et le cache doivent différer");
    }
}
