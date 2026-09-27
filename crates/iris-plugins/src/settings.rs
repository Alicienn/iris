//! Les réglages d'un plugin.
//!
//! Un plugin utile finit toujours par avoir une question à poser : quelle adresse,
//! quel seuil, quel dossier. Sans endroit pour la poser, la réponse finit codée en dur
//! dans le WebAssembly, et le plugin devient inutilisable par quelqu'un d'autre.
//!
//! Trois décisions, chacune contre une alternative plus simple :
//!
//! - **Le plugin déclare ses réglages, il ne les invente pas.** Le manifeste liste des
//!   clés typées, avec un libellé et une valeur par défaut ; l'interface dessine ce
//!   qu'elle y trouve. L'alternative — laisser le plugin écrire ce qu'il veut — voudrait
//!   dire une interface qui ne sait pas quoi afficher tant que le plugin n'a pas
//!   tourné, et un écran de réglages vide au premier lancement.
//! - **Les valeurs vivent à côté du plugin**, dans son propre répertoire. Retirer le
//!   répertoire retire les réglages, ce qui est ce qu'on attend en supprimant un
//!   plugin ; les mettre dans la base de l'application y laisserait des orphelins.
//! - **Aucun secret.** Un plugin qui a besoin d'un mot de passe demande la permission
//!   `Secrets`, qui ne lui est jamais accordée. Un champ de réglage est un fichier en
//!   clair à côté d'un `.wasm` : le dire ici évite qu'on l'apprenne autrement.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Le type d'un réglage, qui décide du contrôle affiché.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    #[default]
    Text,
    Number,
    Toggle,
}

impl SettingKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Toggle => "toggle",
        }
    }
}

/// Un réglage déclaré par un plugin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingSpec {
    pub key: String,
    /// Ce que lit l'utilisateur. À défaut, la clé — laide, mais jamais vide.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub kind: SettingKind,
    /// La valeur si l'utilisateur n'a rien choisi.
    #[serde(default)]
    pub default: String,
}

impl SettingSpec {
    /// Le libellé, ou la clé quand il manque.
    pub fn display_label(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.key
        } else {
            &self.label
        }
    }

    /// La clé est-elle utilisable ?
    ///
    /// Refusée plutôt que nettoyée : une clé corrigée en silence ne serait plus celle
    /// que le plugin lit, et il recevrait toujours la valeur par défaut sans que rien
    /// ne l'explique.
    pub fn is_valid_key(&self) -> bool {
        !self.key.is_empty()
            && self.key.len() <= 64
            && self
                .key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    }
}

/// Les valeurs choisies, telles qu'écrites sur le disque.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingValues {
    #[serde(flatten, default)]
    pub values: BTreeMap<String, String>,
}

impl SettingValues {
    /// Lit `settings.toml` dans le répertoire du plugin.
    ///
    /// Un fichier absent ou illisible rend les valeurs par défaut. Un plugin dont les
    /// réglages ont été corrompus doit continuer à tourner : refuser de le charger
    /// pour un fichier de préférences serait perdre la fonction pour la préférence.
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join("settings.toml"))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let texte = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(dir.join("settings.toml"), texte)
    }

    /// La valeur d'une clé, ou son défaut déclaré.
    pub fn get<'a>(&'a self, spec: &'a SettingSpec) -> &'a str {
        self.values
            .get(&spec.key)
            .map(String::as_str)
            .unwrap_or(&spec.default)
    }

    /// Enregistre une valeur, ou l'efface si elle revient au défaut.
    ///
    /// Effacer plutôt que d'écrire la même chose : un fichier qui ne contient que ce
    /// qui diffère du défaut se lit d'un coup d'œil, et un plugin dont le défaut
    /// change en version suivante voit le nouveau défaut plutôt que l'ancien figé.
    pub fn set(&mut self, spec: &SettingSpec, valeur: &str) {
        if valeur == spec.default {
            self.values.remove(&spec.key);
        } else {
            self.values.insert(spec.key.clone(), valeur.to_string());
        }
    }

    /// Toutes les valeurs effectives, défauts compris.
    ///
    /// Ce qui est remis au plugin : il ne doit pas avoir à connaître ses propres
    /// défauts une seconde fois, dans son code, en risquant qu'ils divergent du
    /// manifeste.
    pub fn effective(&self, specs: &[SettingSpec]) -> BTreeMap<String, String> {
        specs
            .iter()
            .filter(|s| s.is_valid_key())
            .map(|s| (s.key.clone(), self.get(s).to_string()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: &str, defaut: &str) -> SettingSpec {
        SettingSpec {
            key: key.into(),
            default: defaut.into(),
            ..Default::default()
        }
    }

    #[test]
    fn une_valeur_absente_rend_le_defaut() {
        let v = SettingValues::default();
        assert_eq!(v.get(&spec("seuil", "10")), "10");
    }

    #[test]
    fn revenir_au_defaut_efface_l_entree() {
        // Un fichier qui ne contient que ce qui diffère du défaut se lit d'un coup
        // d'œil, et le plugin suit le nouveau défaut à la version suivante.
        let s = spec("seuil", "10");
        let mut v = SettingValues::default();
        v.set(&s, "20");
        assert_eq!(v.values.len(), 1);
        v.set(&s, "10");
        assert!(v.values.is_empty());
    }

    #[test]
    fn les_valeurs_effectives_comblent_les_trous() {
        let specs = vec![spec("a", "1"), spec("b", "2")];
        let mut v = SettingValues::default();
        v.set(&specs[1], "9");

        let effectives = v.effective(&specs);
        assert_eq!(effectives.get("a").map(String::as_str), Some("1"));
        assert_eq!(effectives.get("b").map(String::as_str), Some("9"));
    }

    #[test]
    fn une_cle_absurde_est_refusee() {
        // Nettoyée en silence, elle ne serait plus celle que le plugin lit, et il
        // recevrait toujours le défaut sans que rien ne l'explique.
        assert!(!spec("", "x").is_valid_key());
        assert!(!spec("clé avec espace", "x").is_valid_key());
        assert!(!spec("clé/chemin", "x").is_valid_key());
        assert!(spec("seuil.max_2", "x").is_valid_key());
    }

    #[test]
    fn une_cle_invalide_ne_traverse_pas() {
        let specs = vec![spec("bonne", "1"), spec("mauvaise clé", "2")];
        let effectives = SettingValues::default().effective(&specs);
        assert_eq!(effectives.len(), 1);
    }

    #[test]
    fn le_libelle_retombe_sur_la_cle() {
        assert_eq!(spec("seuil", "").display_label(), "seuil");
    }

    #[test]
    fn l_aller_retour_sur_disque() {
        let dossier = tempfile::tempdir().unwrap();
        let s = spec("adresse", "");
        let mut v = SettingValues::default();
        v.set(&s, "marie@example.com");
        v.save(dossier.path()).unwrap();

        let relu = SettingValues::load(dossier.path());
        assert_eq!(relu.get(&s), "marie@example.com");
    }

    #[test]
    fn un_fichier_corrompu_ne_casse_pas_le_plugin() {
        // Perdre la fonction pour la préférence serait le mauvais échange.
        let dossier = tempfile::tempdir().unwrap();
        std::fs::write(
            dossier.path().join("settings.toml"),
            "ceci n'est pas du toml [[[",
        )
        .unwrap();
        assert_eq!(
            SettingValues::load(dossier.path()),
            SettingValues::default()
        );
    }
}
