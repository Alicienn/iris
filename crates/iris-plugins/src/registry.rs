//! Le registre des plugins installés.
//!
//! Un plugin est un répertoire contenant un manifeste et un binaire. Le registre
//! parcourt le dossier d'installation, charge ce qu'il peut, et **rapporte ce qu'il
//! n'a pas pu charger** : un plugin qui disparaît sans explication est la pire des
//! situations pour celui qui vient de l'installer.

use crate::host::Plugin;
use crate::manifest::Manifest;
use crate::MANIFEST_FILE;
use iris_types::Result;
use std::collections::BTreeMap;
use std::path::Path;

/// Ce qu'un chargement a produit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    pub loaded: Vec<String>,
    /// Répertoire et raison, pour chaque plugin écarté.
    pub rejected: Vec<(String, String)>,
}

impl LoadReport {
    pub fn summary(&self) -> String {
        match (self.loaded.len(), self.rejected.len()) {
            (n, 0) => format!("{n} plugin(s) chargé(s)."),
            (0, m) => format!("Aucun plugin chargé, {m} écarté(s)."),
            (n, m) => format!("{n} plugin(s) chargé(s), {m} écarté(s)."),
        }
    }
}

/// Les plugins installés.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins: BTreeMap<String, Plugin>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Charge tous les plugins d'un répertoire.
    ///
    /// Un plugin illisible est écarté, jamais fatal : l'application démarre avec ce
    /// qui fonctionne.
    pub fn load_dir(&mut self, dir: impl AsRef<Path>) -> Result<LoadReport> {
        let dir = dir.as_ref();
        let mut rapport = LoadReport::default();

        if !dir.exists() {
            return Ok(rapport);
        }

        for entree in std::fs::read_dir(dir)? {
            let chemin = entree?.path();
            if !chemin.is_dir() {
                continue;
            }
            let nom = chemin
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();

            match self.load_one(&chemin) {
                Ok(id) => rapport.loaded.push(id),
                Err(e) => {
                    tracing::warn!(plugin = %nom, erreur = %e, "plugin écarté");
                    rapport.rejected.push((nom, e.to_string()));
                }
            }
        }

        Ok(rapport)
    }

    /// Charge un plugin depuis son répertoire.
    pub fn load_one(&mut self, dir: &Path) -> Result<String> {
        let source = std::fs::read_to_string(dir.join(MANIFEST_FILE))?;
        let manifeste = Manifest::from_toml(&source)?;

        // Le chemin d'entrée a déjà été validé par le manifeste ; on le résout tout
        // de même relativement au répertoire du plugin, sans jamais le concaténer
        // avec autre chose.
        let binaire = std::fs::read(dir.join(&manifeste.entry))?;

        let id = manifeste.id.clone();
        if self.plugins.contains_key(&id) {
            return Err(iris_types::Error::Plugin {
                plugin: id,
                message: "un plugin de cet identifiant est déjà chargé".into(),
            });
        }

        let plugin = Plugin::load(manifeste, &binaire)?;
        self.plugins.insert(id.clone(), plugin);
        Ok(id)
    }

    /// Installe un plugin déjà en mémoire. Sert aux tests et à l'installation à chaud.
    pub fn insert(&mut self, plugin: Plugin) -> Result<()> {
        let id = plugin.id().to_string();
        if self.plugins.contains_key(&id) {
            return Err(iris_types::Error::Plugin {
                plugin: id,
                message: "déjà chargé".into(),
            });
        }
        self.plugins.insert(id, plugin);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.plugins.remove(id).is_some()
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Plugin> {
        self.plugins.get_mut(id)
    }

    pub fn ids(&self) -> Vec<&str> {
        self.plugins.keys().map(String::as_str).collect()
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Plugins actuellement hors circuit, avec la raison.
    pub fn disabled(&self) -> Vec<(&str, &str)> {
        self.plugins
            .values()
            .filter_map(|p| p.disabled_reason().map(|r| (p.id(), r)))
            .collect()
    }

    /// Diffuse un événement à tous les plugins actifs.
    ///
    /// Un plugin en échec n'interrompt pas la diffusion : les autres doivent recevoir
    /// l'événement même si l'un d'eux se comporte mal.
    pub fn dispatch(&mut self, export: &str, payload: &str) -> Vec<(String, Result<crate::CallTrace>)> {
        let ids: Vec<String> = self
            .plugins
            .values()
            .filter(|p| !p.is_disabled())
            .map(|p| p.id().to_string())
            .collect();

        ids.into_iter()
            .filter_map(|id| {
                let plugin = self.plugins.get_mut(&id)?;
                Some((id, plugin.call(export, payload)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wasm_minimal() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                 (import "iris" "log" (func $log (param i32 i32)))
                 (import "iris" "act" (func $act (param i32 i32) (result i32)))
                 (import "iris" "add_command" (func $add (param i32 i32) (result i32)))
                 (import "iris" "notify" (func $notify (param i32 i32) (result i32)))
                 (memory (export "memory") 1)
                 (global $next (mut i32) (i32.const 1024))
                 (func (export "iris_alloc") (param i32) (result i32)
                   (local $p i32)
                   (local.set $p (global.get $next))
                   (global.set $next (i32.add (global.get $next) (local.get 0)))
                   (local.get $p))
                 (func (export "iris_on_event") (param i32 i32) (result i32)
                   (call $log (local.get 0) (local.get 1))
                   (i32.const 0)))"#,
        )
        .unwrap()
    }

    fn wasm_fautif() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                 (import "iris" "log" (func $log (param i32 i32)))
                 (import "iris" "act" (func $act (param i32 i32) (result i32)))
                 (import "iris" "add_command" (func $add (param i32 i32) (result i32)))
                 (import "iris" "notify" (func $notify (param i32 i32) (result i32)))
                 (memory (export "memory") 1)
                 (func (export "iris_alloc") (param i32) (result i32) (i32.const 1024))
                 (func (export "iris_on_event") (param i32 i32) (result i32) (unreachable)))"#,
        )
        .unwrap()
    }

    /// Écrit un plugin sur le disque.
    fn installer(racine: &Path, id: &str, wasm: &[u8]) {
        let dir = racine.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"1.0\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("plugin.wasm"), wasm).unwrap();
    }

    #[test]
    fn un_repertoire_absent_ne_charge_rien() {
        let mut r = PluginRegistry::new();
        let rapport = r.load_dir("/chemin/qui/n/existe/pas").unwrap();
        assert!(rapport.loaded.is_empty());
        assert!(r.is_empty());
    }

    #[test]
    fn les_plugins_d_un_repertoire_sont_charges() {
        let dir = tempfile::tempdir().unwrap();
        installer(dir.path(), "un", &wasm_minimal());
        installer(dir.path(), "deux", &wasm_minimal());

        let mut r = PluginRegistry::new();
        let rapport = r.load_dir(dir.path()).unwrap();

        assert_eq!(rapport.loaded.len(), 2);
        assert!(rapport.rejected.is_empty());
        assert_eq!(r.ids(), ["deux", "un"]);
    }

    #[test]
    fn un_plugin_illisible_est_ecarte_sans_empecher_les_autres() {
        // Un plugin qui disparaît sans explication est la pire des situations pour
        // celui qui vient de l'installer : on le rapporte.
        let dir = tempfile::tempdir().unwrap();
        installer(dir.path(), "bon", &wasm_minimal());

        let casse = dir.path().join("casse");
        std::fs::create_dir_all(&casse).unwrap();
        std::fs::write(casse.join(MANIFEST_FILE), "id = = pas du toml").unwrap();

        let mut r = PluginRegistry::new();
        let rapport = r.load_dir(dir.path()).unwrap();

        assert_eq!(rapport.loaded, ["bon"]);
        assert_eq!(rapport.rejected.len(), 1);
        assert_eq!(rapport.rejected[0].0, "casse");
        assert!(rapport.summary().contains("1 plugin(s) chargé(s), 1 écarté(s)"));
    }

    #[test]
    fn un_manifeste_sans_binaire_est_ecarte() {
        let dir = tempfile::tempdir().unwrap();
        let sans = dir.path().join("sans-binaire");
        std::fs::create_dir_all(&sans).unwrap();
        std::fs::write(
            sans.join(MANIFEST_FILE),
            "id = \"sans-binaire\"\nname = \"A\"\nversion = \"1\"\n",
        )
        .unwrap();

        let mut r = PluginRegistry::new();
        let rapport = r.load_dir(dir.path()).unwrap();
        assert_eq!(rapport.rejected.len(), 1);
    }

    #[test]
    fn deux_plugins_de_meme_identifiant_ne_coexistent_pas() {
        let dir = tempfile::tempdir().unwrap();
        installer(dir.path(), "double", &wasm_minimal());

        let mut r = PluginRegistry::new();
        r.load_dir(dir.path()).unwrap();
        let e = r.load_one(&dir.path().join("double")).unwrap_err();
        assert!(e.to_string().contains("déjà chargé"));
    }

    #[test]
    fn les_fichiers_isoles_sont_ignores() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "rien").unwrap();

        let mut r = PluginRegistry::new();
        assert!(r.load_dir(dir.path()).unwrap().loaded.is_empty());
    }

    #[test]
    fn la_diffusion_atteint_tous_les_plugins() {
        let dir = tempfile::tempdir().unwrap();
        installer(dir.path(), "un", &wasm_minimal());
        installer(dir.path(), "deux", &wasm_minimal());

        let mut r = PluginRegistry::new();
        r.load_dir(dir.path()).unwrap();

        let resultats = r.dispatch(crate::entry_points::ON_EVENT, "{\"x\":1}");
        assert_eq!(resultats.len(), 2);
        for (_, resultat) in &resultats {
            assert_eq!(resultat.as_ref().unwrap().logs, ["{\"x\":1}"]);
        }
    }

    #[test]
    fn un_plugin_fautif_n_interrompt_pas_la_diffusion() {
        let dir = tempfile::tempdir().unwrap();
        installer(dir.path(), "bon", &wasm_minimal());
        installer(dir.path(), "fautif", &wasm_fautif());

        let mut r = PluginRegistry::new();
        r.load_dir(dir.path()).unwrap();

        let resultats = r.dispatch(crate::entry_points::ON_EVENT, "{}");
        assert_eq!(resultats.len(), 2);

        let bon = resultats.iter().find(|(id, _)| id == "bon").unwrap();
        assert!(bon.1.is_ok(), "le plugin sain doit recevoir l'événement");
        let fautif = resultats.iter().find(|(id, _)| id == "fautif").unwrap();
        assert!(fautif.1.is_err());
    }

    #[test]
    fn un_plugin_repetitivement_fautif_sort_de_la_diffusion() {
        let dir = tempfile::tempdir().unwrap();
        installer(dir.path(), "fautif", &wasm_fautif());

        let mut r = PluginRegistry::new();
        r.load_dir(dir.path()).unwrap();

        for _ in 0..3 {
            r.dispatch(crate::entry_points::ON_EVENT, "{}");
        }
        assert_eq!(r.disabled().len(), 1);
        assert!(r.dispatch(crate::entry_points::ON_EVENT, "{}").is_empty());
    }

    #[test]
    fn un_plugin_peut_etre_retire() {
        let dir = tempfile::tempdir().unwrap();
        installer(dir.path(), "un", &wasm_minimal());

        let mut r = PluginRegistry::new();
        r.load_dir(dir.path()).unwrap();

        assert!(r.remove("un"));
        assert!(r.is_empty());
        assert!(!r.remove("un"));
    }

    #[test]
    fn le_resume_est_lisible() {
        let r = LoadReport::default();
        assert_eq!(r.summary(), "0 plugin(s) chargé(s).");
    }
}
