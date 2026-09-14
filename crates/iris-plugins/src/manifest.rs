//! Le manifeste d'un plugin.
//!
//! Un plugin déclare **ce qu'il veut faire avant de pouvoir le faire**. C'est ce qui
//! permet de présenter à l'utilisateur une liste de permissions compréhensible au
//! moment de l'installation, plutôt qu'un avertissement générique — ou pire, rien du
//! tout.
//!
//! Les capacités sont celles du noyau : un plugin obéit exactement au même modèle de
//! permissions qu'un module interne, à ceci près que certaines lui sont
//! définitivement fermées.

use iris_kernel::{Capability, CapabilitySet, NetworkScope};
use iris_types::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Ce qu'un plugin déclare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// Fichier WebAssembly, relatif au répertoire du plugin.
    #[serde(default = "entree_par_defaut")]
    pub entry: String,
    /// Version du contrat que le plugin sait parler.
    #[serde(default = "api_par_defaut")]
    pub api_version: u32,
    /// Permissions demandées.
    #[serde(default)]
    pub permissions: Permissions,
    /// Limites, éventuellement resserrées par rapport aux valeurs par défaut.
    #[serde(default)]
    pub limits: Limits,
}

fn entree_par_defaut() -> String {
    "plugin.wasm".into()
}

fn api_par_defaut() -> u32 {
    1
}

/// Version du contrat que cet hôte sait servir.
pub const API_VERSION: u32 = 1;

/// Les permissions déclarées, sous une forme lisible par un humain.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    pub read_mail: bool,
    pub write_mail: bool,
    pub read_accounts: bool,
    pub notifications: bool,
    pub commands: bool,
    pub ui_panels: bool,
    pub storage: bool,
    /// Hôtes joignables. Une liste vide signifie « aucun accès réseau ».
    pub network: BTreeSet<String>,
}

impl Permissions {
    /// Traduit en capacités du noyau.
    pub fn to_capabilities(&self) -> CapabilitySet {
        let mut caps = Vec::new();
        if self.read_mail {
            caps.push(Capability::ReadMail);
        }
        if self.write_mail {
            caps.push(Capability::WriteMail);
        }
        if self.read_accounts {
            caps.push(Capability::ReadAccounts);
        }
        if self.notifications {
            caps.push(Capability::Notifications);
        }
        if self.commands {
            caps.push(Capability::Commands);
        }
        if self.ui_panels {
            caps.push(Capability::UiPanels);
        }
        if self.storage {
            caps.push(Capability::Storage);
        }
        if !self.network.is_empty() {
            caps.push(Capability::Network(NetworkScope::Hosts(
                self.network.clone(),
            )));
        }
        // S'abonner aux événements est implicite : un plugin qui ne peut rien
        // écouter ne sert à rien, et l'écoute seule ne révèle rien.
        caps.push(Capability::SubscribeEvents);

        CapabilitySet::granting(caps)
    }

    /// Description destinée à l'utilisateur, au moment de l'installation.
    ///
    /// Formulée en termes de conséquences, pas de noms techniques : « lire vos
    /// messages » dit ce qui est en jeu, « read_mail » ne dit rien.
    pub fn describe(&self) -> Vec<String> {
        let mut lignes = Vec::new();
        if self.read_mail {
            lignes.push("lire vos messages et leur état".to_string());
        }
        if self.write_mail {
            lignes.push("modifier l'état de vos messages et les déplacer".to_string());
        }
        if self.read_accounts {
            lignes.push("connaître la liste de vos comptes".to_string());
        }
        if self.notifications {
            lignes.push("afficher des notifications".to_string());
        }
        if self.commands {
            lignes.push("ajouter des commandes à la palette".to_string());
        }
        if self.ui_panels {
            lignes.push("ajouter des panneaux à l'interface".to_string());
        }
        if self.storage {
            lignes.push("conserver ses propres données".to_string());
        }
        if !self.network.is_empty() {
            let hotes: Vec<&str> = self.network.iter().map(String::as_str).collect();
            lignes.push(format!("se connecter à : {}", hotes.join(", ")));
        }
        lignes
    }

    /// Ces permissions exigent-elles un accord explicite ?
    pub fn needs_consent(&self) -> bool {
        self.write_mail || self.notifications || !self.network.is_empty()
    }
}

/// Les limites d'exécution.
///
/// Elles existent pour que la question « ce plugin peut-il figer l'application ? »
/// ait une réponse définitive, qui est non.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Mémoire maximale, en pages de 64 kio.
    pub memory_pages: u32,
    /// Budget d'exécution par appel, en unités de « carburant » wasmtime.
    ///
    /// Une unité correspond grossièrement à une instruction ; dix millions
    /// représentent une fraction de milliseconde de travail utile, et une boucle
    /// infinie les épuise instantanément.
    pub fuel_per_call: u64,
    /// Nombre d'échecs consécutifs au-delà duquel le plugin est désactivé.
    pub failure_threshold: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_pages: 256,
            fuel_per_call: 10_000_000,
            failure_threshold: 3,
        }
    }
}

impl Limits {
    pub fn memory_bytes(&self) -> usize {
        self.memory_pages as usize * 64 * 1024
    }
}

impl Manifest {
    /// Analyse un manifeste TOML.
    pub fn from_toml(source: &str) -> Result<Self> {
        let manifeste: Self = toml::from_str(source).map_err(|e| Error::Plugin {
            plugin: "?".into(),
            message: format!("manifeste illisible : {e}"),
        })?;
        manifeste.validate()?;
        Ok(manifeste)
    }

    /// Vérifie qu'un manifeste est exploitable.
    pub fn validate(&self) -> Result<()> {
        let refuser = |message: String| {
            Err(Error::Plugin {
                plugin: self.id.clone(),
                message,
            })
        };

        if self.id.trim().is_empty() {
            return refuser("identifiant vide".into());
        }
        // L'identifiant sert de nom de répertoire et de clé : le restreindre évite
        // qu'un plugin puisse s'échapper de son propre dossier.
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return refuser(format!(
                "identifiant « {} » : seuls les caractères alphanumériques, « - », \
                 « _ » et « . » sont admis",
                self.id
            ));
        }
        if self.name.trim().is_empty() {
            return refuser("nom vide".into());
        }
        if self.api_version != API_VERSION {
            return refuser(format!(
                "contrat en version {} ; cette version d'Iris sert la version {API_VERSION}",
                self.api_version
            ));
        }
        if self.entry.contains("..") || self.entry.starts_with('/') || self.entry.contains('\\') {
            return refuser(format!("chemin d'entrée « {} » refusé", self.entry));
        }
        if self.limits.memory_pages == 0 || self.limits.fuel_per_call == 0 {
            return refuser("limites nulles : le plugin ne pourrait rien faire".into());
        }

        Ok(())
    }

    /// Les capacités effectivement accordables à ce plugin.
    ///
    /// Le trousseau et la gestion des comptes ne le sont jamais : un plugin capable
    /// de lire les mots de passe n'est plus un plugin.
    pub fn granted_capabilities(&self) -> CapabilitySet {
        self.permissions.to_capabilities().sandboxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXEMPLE: &str = r#"
        id = "desabonnement-auto"
        name = "Désabonnement automatique"
        version = "1.0.0"
        description = "Se désabonne des infolettres jamais ouvertes."

        [permissions]
        read_mail = true
        write_mail = true
        network = ["unsubscribe.example.com"]

        [limits]
        fuel_per_call = 5000000
    "#;

    #[test]
    fn un_manifeste_complet_est_analyse() {
        let m = Manifest::from_toml(EXEMPLE).unwrap();
        assert_eq!(m.id, "desabonnement-auto");
        assert_eq!(m.entry, "plugin.wasm", "valeur par défaut");
        assert_eq!(m.api_version, API_VERSION);
        assert_eq!(m.limits.fuel_per_call, 5_000_000);
        assert_eq!(
            m.limits.memory_pages, 256,
            "le reste des limites est hérité"
        );
    }

    #[test]
    fn les_permissions_deviennent_des_capacites() {
        let m = Manifest::from_toml(EXEMPLE).unwrap();
        let caps = m.granted_capabilities();

        assert!(caps.allows(&Capability::ReadMail));
        assert!(caps.allows(&Capability::WriteMail));
        assert!(caps.allows(&Capability::SubscribeEvents), "implicite");
        assert!(caps.allows(&Capability::Network(NetworkScope::hosts([
            "unsubscribe.example.com"
        ]))));
    }

    #[test]
    fn un_plugin_n_obtient_jamais_le_trousseau() {
        // Un plugin capable de lire les mots de passe n'est plus un plugin.
        let m = Manifest::from_toml(
            "id = \"curieux\"\nname = \"Curieux\"\nversion = \"1\"\n\
             [permissions]\nread_mail = true\n",
        )
        .unwrap();
        let caps = m.granted_capabilities();

        assert!(!caps.allows(&Capability::Secrets));
        assert!(!caps.allows(&Capability::WriteAccounts));
    }

    #[test]
    fn un_reseau_non_declare_n_est_pas_accorde() {
        let m = Manifest::from_toml("id = \"a\"\nname = \"A\"\nversion = \"1\"\n").unwrap();
        assert!(!m
            .granted_capabilities()
            .allows(&Capability::Network(NetworkScope::Any)));
    }

    #[test]
    fn les_permissions_se_decrivent_en_consequences() {
        // « lire vos messages » dit ce qui est en jeu, « read_mail » ne dit rien.
        let m = Manifest::from_toml(EXEMPLE).unwrap();
        let lignes = m.permissions.describe();

        assert!(lignes.iter().any(|l| l.contains("lire vos messages")));
        assert!(lignes.iter().any(|l| l.contains("unsubscribe.example.com")));
        assert!(!lignes.iter().any(|l| l.contains("read_mail")));
    }

    #[test]
    fn les_permissions_sensibles_exigent_un_accord() {
        let m = Manifest::from_toml(EXEMPLE).unwrap();
        assert!(m.permissions.needs_consent());

        let discret = Manifest::from_toml(
            "id = \"a\"\nname = \"A\"\nversion = \"1\"\n[permissions]\nread_mail = true\n",
        )
        .unwrap();
        assert!(!discret.permissions.needs_consent());
    }

    #[test]
    fn un_identifiant_avec_des_caracteres_de_chemin_est_refuse() {
        // Il sert de nom de répertoire : sans cette garde, un plugin pourrait
        // s'échapper de son propre dossier.
        for mauvais in ["../evade", "a/b", "a\\b", ""] {
            let source = format!("id = \"{mauvais}\"\nname = \"A\"\nversion = \"1\"\n");
            assert!(
                Manifest::from_toml(&source).is_err(),
                "« {mauvais} » aurait dû être refusé"
            );
        }
    }

    #[test]
    fn un_chemin_d_entree_qui_remonte_est_refuse() {
        for mauvais in ["../autre.wasm", "/etc/passwd", "sous\\dossier.wasm"] {
            let source =
                format!("id = \"a\"\nname = \"A\"\nversion = \"1\"\nentry = \"{mauvais}\"\n");
            assert!(
                Manifest::from_toml(&source).is_err(),
                "« {mauvais} » aurait dû être refusé"
            );
        }
    }

    #[test]
    fn une_version_de_contrat_inconnue_est_refusee() {
        let e =
            Manifest::from_toml("id = \"a\"\nname = \"A\"\nversion = \"1\"\napi_version = 99\n")
                .unwrap_err();
        assert!(e.to_string().contains("version 99"));
    }

    #[test]
    fn des_limites_nulles_sont_refusees() {
        let e = Manifest::from_toml(
            "id = \"a\"\nname = \"A\"\nversion = \"1\"\n[limits]\nfuel_per_call = 0\n",
        )
        .unwrap_err();
        assert!(e.to_string().contains("limites nulles"));
    }

    #[test]
    fn la_memoire_se_convertit_en_octets() {
        assert_eq!(Limits::default().memory_bytes(), 256 * 64 * 1024);
    }

    #[test]
    fn un_manifeste_survit_a_un_aller_retour() {
        let m = Manifest::from_toml(EXEMPLE).unwrap();
        let relu = Manifest::from_toml(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(m, relu);
    }
}
