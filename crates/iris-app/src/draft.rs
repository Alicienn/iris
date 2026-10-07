//! Le brouillon en cours, gardé sur disque.
//!
//! Fermer l'éditeur laissait le texte dans les propriétés de l'interface : il
//! réapparaissait tant que l'application tournait, et disparaissait avec elle. Quelqu'un
//! qui écrit un message, ferme la fenêtre pour vérifier une adresse, puis quitte, avait
//! perdu son message — sans avertissement, parce que rien ne savait qu'il y en avait un.
//!
//! ## Ce que ceci n'est pas
//!
//! Ce n'est **pas** le dossier « Drafts » du serveur. Y déposer le brouillon demanderait
//! une opération `APPEND` que le journal ne connaît pas encore, et le brouillon
//! apparaîtrait alors sur les autres appareils — ce qui est le bon comportement final.
//! En attendant, un fichier local empêche la perte, qui est le vrai dégât ; il ne
//! prétend pas faire le reste.
//!
//! Un seul brouillon à la fois, parce que l'éditeur n'en ouvre qu'un. Le jour où il en
//! ouvrira plusieurs, ce fichier deviendra une table.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Ce qu'on garde d'un message en cours d'écriture.
///
/// Les pièces jointes n'y sont pas : ce sont des octets déjà lus en mémoire, parfois
/// des dizaines de mégaoctets, et les recopier à chaque fermeture pour un fichier qui
/// se réécrit souvent coûterait plus que ce qu'il protège. Leur absence est dite à la
/// réouverture plutôt que découverte à l'envoi.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
    /// Combien de pièces jointes ont été perdues en chemin. Zéro le plus souvent.
    pub lost_attachments: u32,
    /// The address it is to leave from: a mailbox's own, or one of its aliases.
    ///
    /// Kept as an address, not as a place in the sender list: that place was forgotten
    /// on restart and shifted when an alias came or went, and the message then left
    /// from another mailbox. Empty in drafts kept before it existed.
    pub sender: String,
}

impl Draft {
    /// Y a-t-il quelque chose à garder ?
    ///
    /// Un éditeur ouvert puis refermé sans rien écrire ne doit pas laisser de fichier :
    /// il se rouvrirait « vide mais restauré », ce qui inquiète pour rien.
    pub fn is_empty(&self) -> bool {
        self.to.trim().is_empty()
            && self.cc.trim().is_empty()
            && self.bcc.trim().is_empty()
            && self.subject.trim().is_empty()
            && self.body.trim().is_empty()
    }

    /// Relit le brouillon. Un fichier absent ou illisible n'en rend aucun.
    ///
    /// Illisible veut dire perdu, et il n'y a rien à faire de plus : refuser d'ouvrir
    /// l'éditeur parce qu'un fichier de brouillon est corrompu empêcherait d'écrire le
    /// message suivant pour protéger un message qu'on ne peut de toute façon plus lire.
    pub fn load(path: impl AsRef<Path>) -> Option<Self> {
        let texte = std::fs::read_to_string(path).ok()?;
        let brouillon: Self = serde_json::from_str(&texte).ok()?;
        (!brouillon.is_empty()).then_some(brouillon)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        if self.is_empty() {
            return Self::clear(path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let texte = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, texte)
    }

    /// The messages minimised to the foot of the window, in their order. A missing or
    /// unreadable file holds none.
    pub fn load_all(path: impl AsRef<Path>) -> Vec<Self> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|texte| serde_json::from_str::<Vec<Self>>(&texte).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|d| !d.is_empty())
            .collect()
    }

    /// Keeps the minimised messages; none left removes the file.
    pub fn save_all(drafts: &[Self], path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        if drafts.is_empty() {
            return Self::clear(path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let texte = serde_json::to_string_pretty(drafts)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, texte)
    }

    /// What names it at the foot of the window: its subject, else to whom, else
    /// "New message".
    pub fn title(&self) -> String {
        if !self.subject.trim().is_empty() {
            self.subject.trim().to_string()
        } else if !self.to.trim().is_empty() {
            format!("To {}", self.to.trim())
        } else {
            "New message".to_string()
        }
    }

    /// Efface le brouillon : le message est parti, ou l'utilisateur l'a abandonné.
    pub fn clear(path: impl AsRef<Path>) -> std::io::Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            // Déjà absent : c'est l'état voulu.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fichier() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("draft.json");
        (d, p)
    }

    fn brouillon() -> Draft {
        Draft {
            to: "marie@x.fr".into(),
            subject: "Devis".into(),
            body: "Bonjour,".into(),
            ..Default::default()
        }
    }

    #[test]
    fn un_brouillon_survit_a_la_fermeture() {
        // Le défaut : le texte vivait dans les propriétés de l'interface, donc il
        // disparaissait avec l'application. Quelqu'un qui ferme l'éditeur pour vérifier
        // une adresse, puis quitte, avait perdu son message.
        let (_d, chemin) = fichier();
        brouillon().save(&chemin).unwrap();
        assert_eq!(Draft::load(&chemin), Some(brouillon()));
    }

    #[test]
    fn un_editeur_ouvert_et_referme_a_vide_ne_laisse_rien() {
        // Sinon il se rouvrirait « vide mais restauré », ce qui inquiète pour rien.
        let (_d, chemin) = fichier();
        Draft::default().save(&chemin).unwrap();
        assert!(Draft::load(&chemin).is_none());
        assert!(!chemin.exists());
    }

    #[test]
    fn enregistrer_du_vide_efface_ce_qui_etait_la() {
        // Le cas qui compte : on efface tout dans l'éditeur, puis on ferme. Garder
        // l'ancien texte le ferait revenir de lui-même au prochain message.
        let (_d, chemin) = fichier();
        brouillon().save(&chemin).unwrap();
        Draft::default().save(&chemin).unwrap();
        assert!(Draft::load(&chemin).is_none());
    }

    #[test]
    fn un_fichier_illisible_n_empeche_pas_d_ecrire() {
        // Refuser d'ouvrir l'éditeur pour protéger un brouillon qu'on ne peut plus lire
        // empêcherait d'écrire le message suivant.
        let (_d, chemin) = fichier();
        std::fs::write(&chemin, "ceci n'est pas du json").unwrap();
        assert!(Draft::load(&chemin).is_none());
    }

    #[test]
    fn effacer_un_brouillon_absent_n_est_pas_une_erreur() {
        let (_d, chemin) = fichier();
        assert!(Draft::clear(&chemin).is_ok());
    }

    #[test]
    fn minimised_messages_come_back_in_their_order() {
        let (_d, chemin) = fichier();
        let second = Draft {
            to: "paul@example.com".into(),
            ..Default::default()
        };
        Draft::save_all(&[brouillon(), Draft::default(), second.clone()], &chemin).unwrap();
        assert_eq!(
            Draft::load_all(&chemin),
            vec![brouillon(), second.clone()],
            "an empty one is not kept"
        );
        assert_eq!(brouillon().title(), "Devis");
        assert_eq!(second.title(), "To paul@example.com");
        Draft::save_all(&[], &chemin).unwrap();
        assert!(!chemin.exists(), "none left, no file");
    }

    #[test]
    fn the_sender_survives_a_restart() {
        // Lost, the message left from the first mailbox instead of the chosen one.
        let (_d, chemin) = fichier();
        let depuis_b = Draft {
            sender: "contact@example.com".into(),
            ..brouillon()
        };
        Draft::save_all(std::slice::from_ref(&depuis_b), &chemin).unwrap();
        assert_eq!(Draft::load_all(&chemin)[0].sender, "contact@example.com");

        // A draft kept before the field existed still opens, with no sender.
        std::fs::write(&chemin, r#"[{"to":"marie@x.fr","subject":"Devis"}]"#).unwrap();
        assert_eq!(Draft::load_all(&chemin)[0].sender, "");
    }

    #[test]
    fn un_espace_ne_compte_pas_comme_un_message() {
        let presque = Draft {
            body: "   \n  ".into(),
            ..Default::default()
        };
        assert!(presque.is_empty());
    }
}
