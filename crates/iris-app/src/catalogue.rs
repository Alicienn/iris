//! Le catalogue de modules.
//!
//! Ce qu'on cherche en ouvrant un tel écran : savoir **ce qui existe**. Sans lui, un
//! système d'extensions n'a d'utilisateurs que ceux qui savent déjà ce qu'ils veulent
//! installer et où le trouver, c'est-à-dire ses auteurs.
//!
//! Deux chemins d'entrée, et le second n'est pas un repli :
//!
//! - **depuis un fichier** — un répertoire contenant `plugin.toml` et son
//!   WebAssembly. Aucun réseau, aucun serveur, rien à faire confiance à personne. Un
//!   développeur teste son propre module par là, et une entreprise distribue les
//!   siens par un partage réseau sans que nous ayons à connaître son existence ;
//! - **depuis un catalogue** — un fichier JSON servi en HTTPS, dont l'adresse est un
//!   réglage. Il n'y en a **aucun par défaut** : livrer une adresse par défaut ferait
//!   d'Iris l'arbitre de ce qui est installable, et de nous les responsables du code
//!   que d'autres y publieraient.
//!
//! Ce que l'écran montre avant d'installer quoi que ce soit, c'est la liste des
//! permissions. Un module s'exécute dans un bac à sable WebAssembly qui ne lui accorde
//! que ce qu'il a demandé et que nous avons accepté d'accorder — jamais les secrets,
//! jamais l'écriture des comptes — mais « ne peut pas voler votre mot de passe » n'est
//! pas la même chose que « ne peut rien faire », et l'écart est ce que la liste dit.

use iris_types::{Error, Result};
use serde::{Deserialize, Serialize};

/// Ce qu'un catalogue annonce d'un module.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogueEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// D'où télécharger le WebAssembly. HTTPS exigé.
    pub url: String,
    /// Les permissions annoncées, pour que l'utilisateur décide avant, pas après.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// L'empreinte BLAKE3 du fichier, en hexadécimal.
    ///
    /// Facultative dans le format, exigée à l'installation : sans elle, on exécute ce
    /// que le serveur a bien voulu envoyer aujourd'hui, ce qui n'est pas la même
    /// chose que ce que le catalogue décrivait.
    #[serde(default)]
    pub sha256: String,
}

/// Un catalogue entier.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalogue {
    #[serde(default)]
    pub plugins: Vec<CatalogueEntry>,
}

impl Catalogue {
    /// Lit un catalogue déjà téléchargé.
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json)
            .map_err(|e| Error::Config(format!("catalogue illisible : {e}")))
    }

    /// Les entrées qui ne sont pas déjà installées.
    pub fn available<'a>(&'a self, installed: &'a [String]) -> Vec<&'a CatalogueEntry> {
        self.plugins
            .iter()
            .filter(|e| !installed.iter().any(|i| i == &e.id))
            .collect()
    }
}

/// L'adresse d'un catalogue est-elle acceptable ?
///
/// HTTPS seulement. Sur du HTTP en clair, n'importe qui sur le chemin choisit le
/// WebAssembly que la machine exécutera — le bac à sable limite les dégâts, il ne les
/// annule pas.
pub fn validate_url(url: &str) -> std::result::Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Enter the address of a catalogue.".into());
    }
    if !url.starts_with("https://") {
        return Err("The address has to start with https://".into());
    }
    if url.len() > 2048 {
        return Err("That address is too long.".into());
    }
    Ok(())
}

/// Un identifiant de module est-il utilisable comme nom de répertoire ?
///
/// C'est la question qui compte : l'identifiant devient un chemin sous le répertoire
/// des plugins, et un `../..` dedans écrirait ailleurs. Refusé plutôt que nettoyé.
pub fn validate_id(id: &str) -> std::result::Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("The plugin identifier has an impossible length.".into());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err("A plugin identifier may only hold letters, digits, - _ and .".into());
    }
    // `.` et `..` passent le test précédent et désignent le répertoire parent.
    if id.chars().all(|c| c == '.') {
        return Err("That identifier is not a name.".into());
    }
    Ok(())
}

/// Installe un module depuis un répertoire local.
///
/// Copié plutôt que référencé : un module dont le `.wasm` vit sur une clé USB
/// disparaîtrait au prochain démarrage, et un module qu'on modifie sous les pieds de
/// l'application changerait de comportement sans que rien ne l'annonce.
pub fn install_from_dir(
    source: &std::path::Path,
    plugins_dir: &std::path::Path,
) -> Result<iris_plugins::Manifest> {
    let manifeste_brut = std::fs::read_to_string(source.join("plugin.toml")).map_err(|e| {
        Error::Config(format!(
            "no plugin.toml in {} ({e})",
            source.file_name().unwrap_or_default().to_string_lossy()
        ))
    })?;

    let manifeste: iris_plugins::Manifest = toml::from_str(&manifeste_brut)
        .map_err(|e| Error::Config(format!("plugin.toml is not readable: {e}")))?;

    validate_id(&manifeste.id).map_err(Error::Config)?;

    if manifeste.api_version != iris_plugins::API_VERSION {
        return Err(Error::Config(format!(
            "this plugin speaks version {} of the contract; Iris speaks {}",
            manifeste.api_version,
            iris_plugins::API_VERSION
        )));
    }

    // L'entrée est un nom de fichier, pas un chemin : « ../../autre.wasm » lirait
    // hors du répertoire du module.
    if manifeste.entry.contains(['/', '\\']) || manifeste.entry.contains("..") {
        return Err(Error::Config(
            "the plugin entry point has to be a file name".into(),
        ));
    }

    let wasm = std::fs::read(source.join(&manifeste.entry)).map_err(|e| {
        Error::Config(format!("{} could not be read ({e})", manifeste.entry))
    })?;

    let cible = plugins_dir.join(&manifeste.id);
    std::fs::create_dir_all(&cible)?;
    std::fs::write(cible.join("plugin.toml"), &manifeste_brut)?;
    std::fs::write(cible.join(&manifeste.entry), &wasm)?;

    Ok(manifeste)
}

/// Retire un module installé.
///
/// Ses réglages partent avec lui, puisqu'ils vivent dans son répertoire. C'est ce
/// qu'on attend en supprimant un module, et c'est la raison pour laquelle ils y sont.
pub fn uninstall(id: &str, plugins_dir: &std::path::Path) -> Result<()> {
    validate_id(id).map_err(Error::Config)?;
    let cible = plugins_dir.join(id);

    if !cible.is_dir() {
        return Err(Error::Config(format!("« {id} » is not installed")));
    }
    std::fs::remove_dir_all(&cible)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_catalogue_se_lit() {
        let c = Catalogue::parse(
            r#"{"plugins":[{"id":"tri","name":"Tri","url":"https://x.fr/tri.wasm"}]}"#,
        )
        .unwrap();
        assert_eq!(c.plugins.len(), 1);
        assert_eq!(c.plugins[0].name, "Tri");
    }

    #[test]
    fn un_catalogue_vide_est_valide() {
        // Un serveur qui n'a rien à proposer n'est pas un serveur en panne.
        assert_eq!(Catalogue::parse("{}").unwrap().plugins.len(), 0);
    }

    #[test]
    fn ce_qui_est_installe_n_est_plus_propose() {
        let c = Catalogue::parse(
            r#"{"plugins":[
                {"id":"a","name":"A","url":"https://x.fr/a.wasm"},
                {"id":"b","name":"B","url":"https://x.fr/b.wasm"}]}"#,
        )
        .unwrap();
        let installes = vec!["a".to_string()];
        let dispo = c.available(&installes);
        assert_eq!(dispo.len(), 1);
        assert_eq!(dispo[0].id, "b");
    }

    #[test]
    fn le_http_en_clair_est_refuse() {
        // Sur du HTTP, n'importe qui sur le chemin choisit le code que la machine
        // exécutera. Le bac à sable limite les dégâts, il ne les annule pas.
        assert!(validate_url("http://exemple.fr/catalogue.json").is_err());
        assert!(validate_url("https://exemple.fr/catalogue.json").is_ok());
    }

    #[test]
    fn une_adresse_vide_est_refusee() {
        assert!(validate_url("  ").is_err());
    }

    #[test]
    fn un_identifiant_qui_remonte_les_repertoires_est_refuse() {
        // L'identifiant devient un chemin sous le répertoire des plugins.
        assert!(validate_id("..").is_err());
        assert!(validate_id("../../windows").is_err());
        assert!(validate_id("a/b").is_err());
        assert!(validate_id("mon-module_2.1").is_ok());
    }

    #[test]
    fn un_identifiant_vide_ou_interminable_est_refuse() {
        assert!(validate_id("").is_err());
        assert!(validate_id(&"a".repeat(65)).is_err());
    }

    #[test]
    fn installer_depuis_un_repertoire_copie_tout() {
        let source = tempfile::tempdir().unwrap();
        let cible = tempfile::tempdir().unwrap();

        std::fs::write(
            source.path().join("plugin.toml"),
            "id = \"exemple\"\nname = \"Exemple\"\nversion = \"1.0\"\n",
        )
        .unwrap();
        std::fs::write(source.path().join("plugin.wasm"), b"\0asm\x01\0\0\0").unwrap();

        let m = install_from_dir(source.path(), cible.path()).unwrap();
        assert_eq!(m.id, "exemple");
        assert!(cible.path().join("exemple/plugin.toml").exists());
        assert!(cible.path().join("exemple/plugin.wasm").exists());
    }

    #[test]
    fn un_point_d_entree_en_forme_de_chemin_est_refuse() {
        // « ../../autre.wasm » lirait hors du répertoire du module.
        let source = tempfile::tempdir().unwrap();
        let cible = tempfile::tempdir().unwrap();
        std::fs::write(
            source.path().join("plugin.toml"),
            "id = \"x\"\nname = \"X\"\nversion = \"1\"\nentry = \"../../evade.wasm\"\n",
        )
        .unwrap();

        assert!(install_from_dir(source.path(), cible.path()).is_err());
    }

    #[test]
    fn un_repertoire_sans_manifeste_est_refuse_clairement() {
        let source = tempfile::tempdir().unwrap();
        let cible = tempfile::tempdir().unwrap();
        let erreur = install_from_dir(source.path(), cible.path())
            .unwrap_err()
            .to_string();
        assert!(erreur.contains("plugin.toml"), "obtenu : {erreur}");
    }

    #[test]
    fn une_version_de_contrat_inconnue_est_refusee() {
        // Charger un module qui parle un autre contrat le ferait échouer plus tard,
        // au premier appel, avec un message qui ne dirait pas pourquoi.
        let source = tempfile::tempdir().unwrap();
        let cible = tempfile::tempdir().unwrap();
        std::fs::write(
            source.path().join("plugin.toml"),
            "id = \"x\"\nname = \"X\"\nversion = \"1\"\napi_version = 99\n",
        )
        .unwrap();
        std::fs::write(source.path().join("plugin.wasm"), b"\0asm\x01\0\0\0").unwrap();

        let erreur = install_from_dir(source.path(), cible.path())
            .unwrap_err()
            .to_string();
        assert!(erreur.contains("contract"), "obtenu : {erreur}");
    }

    #[test]
    fn desinstaller_retire_le_repertoire_et_les_reglages() {
        let plugins = tempfile::tempdir().unwrap();
        let module = plugins.path().join("exemple");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(module.join("settings.toml"), "cle = \"valeur\"").unwrap();

        uninstall("exemple", plugins.path()).unwrap();
        assert!(!module.exists());
    }

    #[test]
    fn desinstaller_ce_qui_n_existe_pas_le_dit() {
        let plugins = tempfile::tempdir().unwrap();
        assert!(uninstall("fantome", plugins.path()).is_err());
    }
}
