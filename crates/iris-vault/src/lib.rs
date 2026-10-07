//! `iris-vault` — notes on the disk.
//!
//! The notes are the user's files: Markdown in a visible folder, `%USERPROFILE%\Iris`
//! by default, one folder per **space**. A space keeps its settings in `.iris\space.json`
//! (hidden, as Obsidian keeps `.obsidian`), its pasted files in `_Fichiers` and its
//! templates in `_Modèles`. Any other folder — an Obsidian vault, one inside OneDrive —
//! can be a space too.
//!
//! Files are written whole and atomically (a temporary file renamed over the old), so
//! a crash never leaves half a note. A note changed on the disk since it was read is
//! not overwritten blindly: the other version is kept beside it. Deleting goes through
//! a [`RecycleBin`]: the Windows one in the application, never a plain removal.

#![forbid(unsafe_code)]

pub mod space;

pub use space::{
    Entry, EntryKind, RecycleBin, Space, SpaceConfig, WriteOutcome, ATTACHMENTS, TEMPLATES,
};

use iris_types::{Error, Result};
use std::path::{Path, PathBuf};

/// The folder that holds the spaces.
#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    /// Opens the root, making it if it is not there yet.
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The spaces: every visible folder of the root, then the folders elsewhere the
    /// user added (`externals`), those still there. The first time, a space named
    /// *Notes* is made so there is always somewhere to write.
    pub fn spaces(&self, externals: &[PathBuf]) -> Result<Vec<Space>> {
        let mut dossiers: Vec<PathBuf> = std::fs::read_dir(&self.root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|p| !space::cache(p))
            .collect();
        dossiers.sort_by_key(|p| space::cle_de_tri(&space::nom(p)));
        if dossiers.is_empty() && externals.is_empty() {
            return Ok(vec![self.create_space("Notes")?]);
        }
        let mut espaces: Vec<Space> = dossiers
            .into_iter()
            .map(Space::open)
            .collect::<Result<_>>()?;
        for e in externals {
            if e.is_dir() && !espaces.iter().any(|s| s.dir() == e.as_path()) {
                espaces.push(Space::open(e.clone())?);
            }
        }
        Ok(espaces)
    }

    /// A new space in the root, with its folders and its templates.
    pub fn create_space(&self, name: &str) -> Result<Space> {
        let nom = space::nom_valide(name)?;
        let dir = space::libre(&self.root, &nom, "");
        std::fs::create_dir_all(&dir)?;
        let mut espace = Space::open(dir)?;
        espace.config.name = space::nom(espace.dir());
        espace.save_config()?;
        espace.make_defaults()?;
        Ok(espace)
    }

    /// Opens a folder anywhere as a space (it gets its `.iris` the first time).
    pub fn add_external(&self, dir: &Path) -> Result<Space> {
        if !dir.is_dir() {
            return Err(Error::other("this folder does not exist"));
        }
        let espace = Space::open(dir.to_path_buf())?;
        espace.save_config()?;
        Ok(espace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_space_is_made_with_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path().join("Iris")).unwrap();
        let espaces = v.spaces(&[]).unwrap();
        assert_eq!(espaces.len(), 1);
        let s = &espaces[0];
        assert_eq!(s.config.name, "Notes");
        assert!(s.dir().join(".iris").join("space.json").is_file());
        assert!(s.dir().join(ATTACHMENTS).is_dir());
        assert!(s.dir().join(TEMPLATES).join("Cours.md").is_file());
        // Found again, not made twice.
        assert_eq!(v.spaces(&[]).unwrap().len(), 1);
        v.create_space("Cours").unwrap();
        let noms: Vec<String> = v
            .spaces(&[])
            .unwrap()
            .into_iter()
            .map(|s| s.config.name)
            .collect();
        assert_eq!(noms, ["Cours", "Notes"]);
    }

    #[test]
    fn a_folder_elsewhere_is_a_space() {
        let dir = tempfile::tempdir().unwrap();
        let ailleurs = dir.path().join("Coffre");
        std::fs::create_dir_all(&ailleurs).unwrap();
        std::fs::write(ailleurs.join("a.md"), "x").unwrap();
        let v = Vault::open(dir.path().join("Iris")).unwrap();
        v.create_space("Perso").unwrap();
        let s = v.add_external(&ailleurs).unwrap();
        assert_eq!(s.config.name, "Coffre");
        let espaces = v.spaces(std::slice::from_ref(&ailleurs)).unwrap();
        assert_eq!(espaces.len(), 2);
    }
}
