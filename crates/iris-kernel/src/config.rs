//! Configuration typée et rechargeable.
//!
//! La configuration est un arbre de valeurs partagé, remplacé d'un bloc lors d'un
//! rechargement. Les lecteurs prennent un instantané : personne ne peut observer une
//! configuration à moitié remplacée, et un rechargement n'interrompt aucune lecture
//! en cours.

use iris_types::{Error, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::{Arc, RwLock};

/// Instantané immuable de la configuration.
#[derive(Debug, Clone, Default)]
pub struct ConfigSnapshot {
    root: Arc<Value>,
    /// Incrémenté à chaque rechargement, permet de détecter un changement sans
    /// comparer l'arbre entier.
    pub generation: u64,
}

impl ConfigSnapshot {
    /// Lit une valeur désignée par un chemin pointé, par exemple `sync.pool_size`.
    ///
    /// Retourne `None` si le chemin n'existe pas, une erreur si la valeur existe mais
    /// ne correspond pas au type attendu — cette distinction est essentielle pour
    /// pouvoir appliquer un défaut sans masquer une faute de frappe de l'utilisateur.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<Option<T>> {
        let Some(v) = self.lookup(path) else {
            return Ok(None);
        };
        serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| Error::Config(format!("« {path} » : {e}")))
    }

    /// Comme `get`, avec repli sur la valeur par défaut du type.
    pub fn get_or_default<T: DeserializeOwned + Default>(&self, path: &str) -> Result<T> {
        Ok(self.get(path)?.unwrap_or_default())
    }

    /// Comme `get`, avec repli sur une valeur fournie.
    pub fn get_or<T: DeserializeOwned>(&self, path: &str, fallback: T) -> Result<T> {
        Ok(self.get(path)?.unwrap_or(fallback))
    }

    pub fn contains(&self, path: &str) -> bool {
        self.lookup(path).is_some()
    }

    fn lookup(&self, path: &str) -> Option<&Value> {
        let mut cur = self.root.as_ref();
        for segment in path.split('.') {
            cur = cur.get(segment)?;
        }
        Some(cur)
    }

    pub fn as_value(&self) -> &Value {
        &self.root
    }
}

/// Poignée partagée sur la configuration courante.
#[derive(Debug, Clone, Default)]
pub struct Config {
    inner: Arc<RwLock<ConfigSnapshot>>,
}

impl Config {
    pub fn new(root: Value) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ConfigSnapshot {
                root: Arc::new(root),
                generation: 1,
            })),
        }
    }

    /// Construit la configuration à partir d'un document TOML.
    ///
    /// Le TOML est converti en un unique modèle de valeur interne : le reste du
    /// noyau n'a ainsi qu'un seul format à connaître, quel que soit le format des
    /// fichiers sur le disque.
    pub fn from_toml(text: &str) -> Result<Self> {
        let value: Value =
            toml::from_str(text).map_err(|e| Error::Config(format!("TOML invalide : {e}")))?;
        Ok(Self::new(value))
    }

    /// Instantané courant. Bon marché : une copie de pointeur.
    pub fn snapshot(&self) -> ConfigSnapshot {
        self.inner
            .read()
            .expect("configuration empoisonnée")
            .clone()
    }

    /// Remplace intégralement la configuration et incrémente la génération.
    pub fn replace(&self, root: Value) -> u64 {
        let mut guard = self.inner.write().expect("configuration empoisonnée");
        guard.root = Arc::new(root);
        guard.generation += 1;
        guard.generation
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<Option<T>> {
        self.snapshot().get(path)
    }

    pub fn get_or<T: DeserializeOwned>(&self, path: &str, fallback: T) -> Result<T> {
        self.snapshot().get_or(path, fallback)
    }

    pub fn generation(&self) -> u64 {
        self.inner
            .read()
            .expect("configuration empoisonnée")
            .generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> Config {
        Config::new(json!({
            "sync": { "pool_size": 16, "poll_min_secs": 60 },
            "ui": { "theme": "mono", "density": "compact" }
        }))
    }

    #[test]
    fn lecture_par_chemin_pointe() {
        let c = cfg();
        assert_eq!(c.get::<u32>("sync.pool_size").unwrap(), Some(16));
        assert_eq!(
            c.get::<String>("ui.theme").unwrap().as_deref(),
            Some("mono")
        );
    }

    #[test]
    fn un_chemin_absent_donne_none_pas_une_erreur() {
        let c = cfg();
        assert_eq!(c.get::<u32>("sync.inexistant").unwrap(), None);
        assert_eq!(c.get::<u32>("rien.du.tout").unwrap(), None);
    }

    #[test]
    fn un_type_incorrect_est_une_erreur_pas_un_defaut() {
        // Distinction capitale : une faute de frappe dans le type ne doit pas être
        // silencieusement remplacée par une valeur par défaut.
        let c = cfg();
        let e = c.get::<String>("sync.pool_size").unwrap_err();
        assert!(e.to_string().contains("sync.pool_size"));
    }

    #[test]
    fn le_repli_s_applique_seulement_a_l_absence() {
        let c = cfg();
        assert_eq!(c.get_or("sync.pool_size", 4u32).unwrap(), 16);
        assert_eq!(c.get_or("sync.absent", 4u32).unwrap(), 4);
    }

    #[test]
    fn un_instantane_survit_au_rechargement() {
        let c = cfg();
        let avant = c.snapshot();
        c.replace(json!({ "ui": { "theme": "sand" } }));

        // Le lecteur qui détient un instantané continue de voir l'ancienne valeur :
        // aucune lecture en cours n'est perturbée par un rechargement.
        assert_eq!(
            avant.get::<String>("ui.theme").unwrap().as_deref(),
            Some("mono")
        );
        assert_eq!(
            c.get::<String>("ui.theme").unwrap().as_deref(),
            Some("sand")
        );
    }

    #[test]
    fn la_generation_augmente_a_chaque_rechargement() {
        let c = cfg();
        assert_eq!(c.generation(), 1);
        assert_eq!(c.replace(json!({})), 2);
        assert_eq!(c.generation(), 2);
    }

    #[test]
    fn chargement_depuis_du_toml() {
        let c = Config::from_toml(
            r#"
            [sync]
            pool_size = 8
            adaptive = true

            [ui]
            theme = "ice"
            "#,
        )
        .unwrap();
        assert_eq!(c.get::<u32>("sync.pool_size").unwrap(), Some(8));
        assert_eq!(c.get::<bool>("sync.adaptive").unwrap(), Some(true));
        assert_eq!(c.get::<String>("ui.theme").unwrap().as_deref(), Some("ice"));
    }

    #[test]
    fn un_toml_invalide_est_signale_clairement() {
        let e = Config::from_toml("ceci n'est pas = = du toml").unwrap_err();
        assert!(e.to_string().contains("TOML invalide"));
    }

    #[test]
    fn contains_distingue_present_et_absent() {
        let s = cfg().snapshot();
        assert!(s.contains("ui.theme"));
        assert!(!s.contains("ui.couleur"));
    }
}
