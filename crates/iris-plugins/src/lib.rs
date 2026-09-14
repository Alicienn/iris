//! `iris-plugins` — l'extensibilité par WebAssembly.
//!
//! Le cœur d'Iris est fait de crates compilées, pour la performance. Cette couche
//! ajoute par-dessus des extensions **tierces**, exécutées en bac à sable, et la
//! question qu'elle doit trancher est simple : *un plugin peut-il nuire ?*
//!
//! La réponse tient en trois bornes, chacune vérifiée par un test qui met réellement
//! un plugin en faute : le **carburant** (une boucle infinie s'arrête d'elle-même),
//! la **mémoire** (un plugin gourmand est refusé, pas la machine), et les
//! **capacités** — les mêmes que celles des modules internes, moins celles qu'un
//! plugin n'obtiendra jamais.
//!
//! Un plugin défaillant est **désactivé, jamais fatal**. C'est la même règle que pour
//! les modules du noyau, et pour la même raison : une extension en panne ne doit pas
//! empêcher de lire son courrier.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod host;
pub mod manifest;
pub mod settings;
mod registry;

pub use host::{CallTrace, Plugin};
pub use manifest::{Limits, Manifest, Permissions, API_VERSION};
pub use registry::{LoadReport, PluginRegistry};
pub use settings::{SettingKind, SettingSpec, SettingValues};

/// Fichier de manifeste attendu dans le répertoire d'un plugin.
pub const MANIFEST_FILE: &str = "plugin.toml";

/// Points d'entrée que l'hôte sait appeler.
pub mod entry_points {
    /// Appelé une fois au chargement.
    pub const INIT: &str = "iris_init";
    /// Appelé pour chaque événement auquel le plugin est abonné.
    pub const ON_EVENT: &str = "iris_on_event";
    /// Appelé quand une commande fournie par le plugin est invoquée.
    pub const ON_COMMAND: &str = "iris_on_command";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_points_d_entree_sont_prefixes() {
        // Le préfixe évite toute collision avec les exports d'une bibliothèque
        // standard embarquée par le plugin.
        for point in [
            entry_points::INIT,
            entry_points::ON_EVENT,
            entry_points::ON_COMMAND,
        ] {
            assert!(
                point.starts_with("iris_"),
                "« {point} » devrait être préfixé"
            );
        }
    }

    #[test]
    fn la_version_du_contrat_est_exposee() {
        assert_eq!(API_VERSION, 1);
    }
}
