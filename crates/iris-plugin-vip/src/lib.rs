//! **VIP** — les gens qui ne se perdent jamais.
//!
//! Tout le reste d'Iris sert à faire sortir du courrier de la file : marquer traité,
//! archiver, reporter, ranger. Rien ne dit « pas celui-là ». Ce module est ce
//! contre-poids, et c'est pourquoi il vient en premier des trois : son absence ne se
//! remarque pas dans une liste de fonctions, elle se remarque le jour où le message
//! qui comptait est passé entre deux gestes de triage.
//!
//! Ce qu'il fait, à l'arrivée d'un message d'une personne nommée :
//!
//! - il l'**épingle**, pour qu'il se retrouve à l'œil dans une liste dense ;
//! - il le **remet à traiter**, ce qui le ramène dans la file si une règle l'en avait
//!   déjà sorti.
//!
//! Ce qu'il ne fait pas : notifier. Une notification par message important, sur une
//! liste de dix personnes importantes, redevient du bruit — et l'application a déjà
//! une bulle d'arrivée qui ne ment pas sur le volume.
//!
//! ## La liste est à vous
//!
//! Elle est vide par défaut, et le module ne fait alors rien du tout. Deviner qui
//! compte pour quelqu'un — les gens à qui il répond vite, ceux de son domaine — serait
//! une inférence sur ses relations, faite sans qu'il l'ait demandée, à partir de son
//! courrier. Une liste qu'on écrit soi-même est une liste dont on répond.

#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use iris_plugin_sdk as sdk;

/// Les adresses et domaines qui comptent, lus une fois à l'initialisation.
static mut LISTE: Option<Vec<String>> = None;

/// Ce que l'hôte nous remet au chargement.
///
/// # Safety
///
/// Appelée par l'hôte, une seule fois, avant tout événement.
#[no_mangle]
pub unsafe extern "C" fn iris_init(ptr: *const u8, len: usize) {
    let reglages = sdk::charge(ptr, len);
    let brut = sdk::champ_texte(reglages, "people").unwrap_or_default();
    let liste = sdk::liste_de_reglage(&brut);

    if liste.is_empty() {
        sdk::log("vip: no one on the list — nothing to do");
    }
    LISTE = Some(liste);
}

/// Un message est arrivé.
///
/// # Safety
///
/// Appelée par l'hôte avec une charge qu'il a écrite via `iris_alloc`.
#[no_mangle]
pub unsafe extern "C" fn iris_on_event(ptr: *const u8, len: usize) {
    let charge = sdk::charge(ptr, len);
    let liste = match &*core::ptr::addr_of!(LISTE) {
        Some(l) => l.as_slice(),
        None => return,
    };

    let Some(expediteur) = sdk::champ_texte(charge, "de") else {
        return;
    };

    if !est_important(&expediteur, liste) {
        return;
    }

    // Épingler d'abord, remettre à traiter ensuite. L'ordre compte pour ce que
    // l'utilisateur voit si la seconde action échoue : un message épinglé mais resté
    // classé se retrouve ; un message ramené dans la file sans marque se noie.
    sdk::act(r#"{"action":"star"}"#);
    sdk::act(r#"{"action":"todo"}"#);
}

/// Cet expéditeur est-il sur la liste ?
///
/// Séparé des points d'entrée pour être testable sans WebAssembly : c'est la seule
/// décision du module, et une décision de tri qu'on ne peut vérifier qu'en s'envoyant
/// un message n'est pas vérifiée.
pub fn est_important(expediteur: &str, liste: &[String]) -> bool {
    liste.iter().any(|e| sdk::adresse_correspond(expediteur, e))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn liste() -> Vec<String> {
        vec![
            "marie@client.fr".to_string(),
            "@direction.example".to_string(),
        ]
    }

    #[test]
    fn une_adresse_nommee_est_importante() {
        assert!(est_important("marie@client.fr", &liste()));
    }

    #[test]
    fn un_domaine_nomme_couvre_les_siens() {
        assert!(est_important("qui-que-ce-soit@direction.example", &liste()));
    }

    #[test]
    fn le_reste_ne_l_est_pas() {
        assert!(!est_important("infolettre@boutique.com", &liste()));
    }

    #[test]
    fn une_liste_vide_ne_designe_personne() {
        // Le défaut du module. Deviner qui compte serait une inférence sur les
        // relations de quelqu'un, faite à partir de son courrier et sans sa demande.
        assert!(!est_important("marie@client.fr", &[]));
    }

    #[test]
    fn la_casse_de_l_adresse_est_sans_effet() {
        assert!(est_important("Marie@Client.FR", &liste()));
    }

    #[test]
    fn un_domaine_qui_se_termine_pareil_n_est_pas_le_meme() {
        // « faux-direction.example » se termine par « direction.example » sans en
        // être : c'est l'erreur qu'une comparaison de suffixe naïve commettrait, et
        // c'est aussi comme cela qu'on usurpe un domaine.
        assert!(!est_important("x@faux-direction.example", &liste()));
    }
}
