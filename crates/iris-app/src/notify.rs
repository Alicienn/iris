//! Les notifications d'arrivée.
//!
//! Le point de tout ce qui synchronise en arrière-plan : sans notification, une
//! synchronisation d'arrière-plan est du travail que personne ne voit, et autant ne
//! pas la faire.
//!
//! Ce sont les bulles natives de Windows, sous l'identité d'Iris. Cette identité —
//! l'`AppUserModelID` — doit être portée par un raccourci du menu Démarrer, ce qui
//! veut dire qu'elles ne fonctionnent **que sur une copie installée**. C'est une
//! contrainte de Windows, pas un choix ; le contournement connu consiste à emprunter
//! l'identifiant de PowerShell pour faire apparaître quelque chose, et une
//! notification qui ment sur qui l'envoie est pire que pas de notification.
//!
//! Sur un exécutable posé n'importe où, l'appel échoue proprement et [`show`] rend
//! `false`. L'appelant peut alors le dire, ce qui vaut mieux qu'un silence que
//! l'utilisateur prendrait pour une panne.
//!
//! Ce qu'elles ne font pas, volontairement :
//!
//! - **une par message.** Vingt messages arrivés pendant une réunion feraient vingt
//!   bulles, et la vingtième chasserait la première. Une seule les résume.
//! - **le contenu.** L'expéditeur et le sujet, jamais l'aperçu : la bulle s'affiche
//!   sur un écran qu'on partage parfois.
//! - **le courrier indésirable.** Prévenir de l'arrivée de ce qu'on a filtré serait
//!   annuler le filtre.

/// Ce dont il faut prévenir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrival {
    /// Combien de messages, tous comptes confondus.
    pub count: usize,
    /// L'expéditeur du plus récent, déjà mis en forme.
    pub sender: String,
    /// Son sujet.
    pub subject: String,
}

impl Arrival {
    /// Le titre et le corps de la bulle.
    ///
    /// Un message : qui écrit, et à quel sujet. Plusieurs : combien, et le plus
    /// récent — le nombre seul ne dit pas s'il faut s'interrompre, et le plus récent
    /// seul cache qu'il y en a dix-neuf autres.
    pub fn message(&self) -> (String, String) {
        match self.count {
            0 | 1 => (self.sender.clone(), truncate(&self.subject, 120)),
            n => (
                format!("{n} new messages"),
                format!("Latest from {}", self.sender),
            ),
        }
    }
}

pub use plateforme::{show, show_text};

/// Coupe proprement, sur une frontière de caractère.
fn truncate(texte: &str, max: usize) -> String {
    if texte.chars().count() <= max {
        return texte.to_string();
    }
    let court: String = texte.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", court.trim_end())
}

#[cfg(windows)]
mod plateforme {
    use super::Arrival;

    /// Affiche la bulle.
    ///
    /// L'`AppUserModelID` est celui d'Iris. Windows n'affiche une bulle que pour une
    /// identité qu'il connaît, c'est-à-dire une application dont un raccourci du menu
    /// Démarrer porte cet identifiant — ce que fait l'installateur. Lancée depuis un
    /// exécutable posé n'importe où, l'appel échoue proprement et rien ne s'affiche :
    /// c'est le comportement voulu, et c'est aussi celui qu'il faut expliquer plutôt
    /// que d'emprunter l'identité de PowerShell pour faire apparaître quelque chose.
    pub fn show(arrivee: &Arrival) -> bool {
        use tauri_winrt_notification::{Duration, Sound, Toast};

        let (titre, corps) = arrivee.message();

        Toast::new(crate::platform::APP_ID)
            .title(&titre)
            .text1(&corps)
            // Muette : un client de courrier qui fait du bruit à chaque arrivée est un
            // client de courrier qu'on finit par couper, et couper la bulle emporte
            // l'information avec le bruit.
            .sound(Some(Sound::Default))
            .duration(Duration::Short)
            .show()
            .is_ok()
    }

    /// Une bulle quelconque : un titre, une ligne. Les rappels d'agenda passent par là.
    pub fn show_text(titre: &str, corps: &str) -> bool {
        use tauri_winrt_notification::{Duration, Sound, Toast};
        Toast::new(crate::platform::APP_ID)
            .title(titre)
            .text1(corps)
            .sound(Some(Sound::Default))
            .duration(Duration::Long)
            .show()
            .is_ok()
    }
}

#[cfg(not(windows))]
mod plateforme {
    use super::Arrival;

    pub fn show(_arrivee: &Arrival) -> bool {
        false
    }

    pub fn show_text(_titre: &str, _corps: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrivee(count: usize) -> Arrival {
        Arrival {
            count,
            sender: "Marie Dupont".into(),
            subject: "Devis refonte du site".into(),
        }
    }

    #[test]
    fn un_message_annonce_qui_ecrit_et_a_quel_sujet() {
        let (titre, corps) = arrivee(1).message();
        assert_eq!(titre, "Marie Dupont");
        assert_eq!(corps, "Devis refonte du site");
    }

    #[test]
    fn plusieurs_messages_sont_resumes_en_une_bulle() {
        // Vingt messages arrivés pendant une réunion feraient vingt bulles, et la
        // vingtième chasserait la première.
        let (titre, corps) = arrivee(20).message();
        assert_eq!(titre, "20 new messages");
        assert!(corps.contains("Marie Dupont"), "le plus récent est nommé");
    }

    #[test]
    fn un_sujet_interminable_est_coupe() {
        let a = Arrival {
            count: 1,
            sender: "X".into(),
            subject: "a".repeat(400),
        };
        let (_, corps) = a.message();
        assert!(corps.chars().count() <= 120);
        assert!(corps.ends_with('…'));
    }

    #[test]
    fn la_coupe_tombe_sur_un_caractere_entier() {
        // Couper sur un octet au milieu d'un caractère accentué produirait une chaîne
        // invalide, et la bulle afficherait un losange à la place.
        let a = Arrival {
            count: 1,
            sender: "X".into(),
            subject: "é".repeat(200),
        };
        let (_, corps) = a.message();
        assert!(corps.chars().all(|c| c == 'é' || c == '…'));
    }

    #[test]
    fn zero_se_comporte_comme_un() {
        // Le compteur vient du moteur ; s'il arrive à zéro c'est un défaut ailleurs,
        // et afficher « 0 new messages » le rendrait visible à l'utilisateur plutôt
        // qu'au journal.
        let (titre, _) = arrivee(0).message();
        assert_eq!(titre, "Marie Dupont");
    }
}
