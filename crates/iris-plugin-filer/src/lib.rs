//! **Filer** — les pièces jointes vont là où vont les pièces jointes.
//!
//! Une règle est une ligne : `motif -> Dossier`. Le motif est cherché dans le sujet et
//! dans le nom des pièces jointes ; à la première règle qui correspond, le fil est
//! rangé.
//!
//! Ce module est le seul des trois qui *déplace* du courrier, et c'est pourquoi il est
//! le plus prudent des trois :
//!
//! - **la première règle gagne**, dans l'ordre écrit. Une règle qui gagnerait « la
//!   plus précise » demanderait à l'utilisateur de deviner ce que le module trouve
//!   précis ; l'ordre d'une liste, lui, se lit ;
//! - **rien par défaut**. La liste vide est vide, pas peuplée d'exemples qui rangeraient
//!   du vrai courrier au premier lancement ;
//! - **le dossier n'est pas créé**. Si le nom ne correspond à aucun dossier, l'hôte
//!   ignore la demande, et le message reste où il est. Un module qui peut fabriquer des
//!   dossiers peut fabriquer une arborescence entière pendant une première
//!   synchronisation.
//!
//! ## Pourquoi une sous-chaîne et non une expression régulière
//!
//! Parce que la moitié des expressions régulières écrites à la main sont fausses d'une
//! manière qui ne se voit pas — et ici, ce qui ne se voit pas déplace du courrier. Une
//! sous-chaîne, insensible à la casse, se relit et fait exactement ce qu'elle dit.

#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use iris_plugin_sdk as sdk;

/// Un motif et sa destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regle {
    /// Cherché dans le sujet et le nom des pièces jointes, sans égard à la casse.
    pub motif: String,
    /// Le nom du dossier, tel qu'il apparaît dans la colonne des dossiers.
    pub dossier: String,
}

static mut REGLES: Option<Vec<Regle>> = None;

/// # Safety
///
/// Appelée par l'hôte, une seule fois, avant tout événement.
#[no_mangle]
pub unsafe extern "C" fn iris_init(ptr: *const u8, len: usize) {
    let reglages = sdk::charge(ptr, len);
    let brut = sdk::champ_texte(reglages, "rules").unwrap_or_default();
    let regles = lit_les_regles(&brut);

    if regles.is_empty() {
        sdk::log("filer: no rules — nothing is filed");
    }
    REGLES = Some(regles);
}

/// # Safety
///
/// Appelée par l'hôte avec une charge qu'il a écrite via `iris_alloc`.
#[no_mangle]
pub unsafe extern "C" fn iris_on_event(ptr: *const u8, len: usize) {
    let charge = sdk::charge(ptr, len);
    let regles = match &*core::ptr::addr_of!(REGLES) {
        Some(r) => r.as_slice(),
        None => return,
    };

    let sujet = sdk::champ_texte(charge, "sujet").unwrap_or_default();
    let pieces = sdk::champ_liste(charge, "pieces");

    let Some(dossier) = destination(&sujet, &pieces, regles) else {
        return;
    };

    // Le nom du dossier est recopié tel quel dans le JSON. Un guillemet ou une barre
    // oblique inverse dans un nom de dossier casserait la charge en silence — et la
    // demande serait alors mal lue, non pas ignorée, ce qui est pire.
    let mut json = String::from("{\"action\":\"move\",\"folder\":\"");
    json.push_str(&echappe(dossier));
    json.push_str("\"}");
    sdk::act(&json);
}

/// Où ce message doit aller, s'il doit aller quelque part.
///
/// Pure et publique : c'est toute la décision du module, et la seule chose qui puisse
/// ranger du courrier au mauvais endroit.
pub fn destination<'a>(sujet: &str, pieces: &[String], regles: &'a [Regle]) -> Option<&'a str> {
    regles
        .iter()
        .find(|r| {
            sdk::contient_insensible(sujet, &r.motif)
                || pieces.iter().any(|p| sdk::contient_insensible(p, &r.motif))
        })
        .map(|r| r.dossier.as_str())
}

/// Lit le réglage : une règle par ligne, `motif -> Dossier`.
///
/// Ce que le format tolère, il le tolère exprès : les espaces autour de la flèche, les
/// lignes vides, et les lignes de commentaire commençant par `#`. Ce qu'il refuse — un
/// motif vide, un dossier vide, une ligne sans flèche — il le refuse en silence, ligne
/// par ligne : une faute de frappe sur la troisième règle ne doit pas désactiver les
/// deux premières.
pub fn lit_les_regles(brut: &str) -> Vec<Regle> {
    brut.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|ligne| {
            // `->` est cherché en premier ; `→` existe parce qu'un clavier français le
            // produit et qu'une règle refusée pour cause de flèche typographique serait
            // refusée sans que rien ne l'explique.
            let (motif, dossier) = ligne.split_once("->").or_else(|| ligne.split_once('→'))?;
            let motif = motif.trim();
            let dossier = dossier.trim();
            (!motif.is_empty() && !dossier.is_empty()).then(|| Regle {
                motif: motif.to_string(),
                dossier: dossier.to_string(),
            })
        })
        .collect()
}

/// Échappe ce qui casserait une chaîne JSON.
///
/// Le module compose son JSON à la main, faute de sérialiseur ; c'est acceptable pour
/// deux champs, à condition que celui qui vient de l'utilisateur soit traité comme tel.
fn echappe(texte: &str) -> String {
    let mut sortie = String::with_capacity(texte.len());
    for c in texte.chars() {
        match c {
            '"' => sortie.push_str("\\\""),
            '\\' => sortie.push_str("\\\\"),
            // Les caractères de contrôle n'ont rien à faire dans un nom de dossier et
            // ne sont pas représentables tels quels dans une chaîne JSON.
            c if (c as u32) < 0x20 => sortie.push(' '),
            c => sortie.push(c),
        }
    }
    sortie
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;

    fn regles() -> Vec<Regle> {
        lit_les_regles("facture -> Comptabilité\n#un commentaire\ncontrat → Juridique\n")
    }

    fn sans_pieces() -> Vec<String> {
        Vec::new()
    }

    #[test]
    fn le_sujet_declenche_la_regle() {
        assert_eq!(
            destination("Votre facture de mars", &sans_pieces(), &regles()),
            Some("Comptabilité")
        );
    }

    #[test]
    fn le_nom_d_une_piece_jointe_suffit() {
        let pieces = vec!["facture-2026-03.pdf".to_string()];
        assert_eq!(
            destination("Bonjour", &pieces, &regles()),
            Some("Comptabilité"),
            "le sujet ne dit rien, la pièce jointe dit tout"
        );
    }

    #[test]
    fn rien_ne_correspond_rien_ne_bouge() {
        assert_eq!(destination("Déjeuner ?", &sans_pieces(), &regles()), None);
    }

    #[test]
    fn sans_regle_rien_ne_bouge() {
        // Le défaut du module : la liste vide est vide, pas peuplée d'exemples qui
        // rangeraient du vrai courrier au premier lancement.
        assert_eq!(destination("Votre facture", &sans_pieces(), &[]), None);
    }

    #[test]
    fn la_premiere_regle_gagne() {
        // L'ordre écrit décide. « La plus précise » demanderait à l'utilisateur de
        // deviner ce que le module trouve précis.
        let r = lit_les_regles("facture -> A\nfacture de mars -> B");
        assert_eq!(
            destination("facture de mars", &sans_pieces(), &r),
            Some("A")
        );
    }

    #[test]
    fn la_casse_est_sans_effet() {
        assert_eq!(
            destination("FACTURE", &sans_pieces(), &regles()),
            Some("Comptabilité")
        );
    }

    #[test]
    fn la_fleche_typographique_marche_aussi() {
        assert_eq!(
            destination("un contrat", &sans_pieces(), &regles()),
            Some("Juridique")
        );
    }

    #[test]
    fn les_lignes_fautives_sont_ignorees_une_par_une() {
        // Une faute de frappe sur la deuxième règle ne doit pas désactiver la première.
        let r = lit_les_regles("facture -> Compta\npas de fleche ici\n-> Vide\nmotif ->\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].dossier, "Compta");
    }

    #[test]
    fn les_espaces_et_les_lignes_vides_sont_tolérés() {
        let r = lit_les_regles("\n   facture   ->   Compta / 2026   \n\n");
        assert_eq!(r[0].motif, "facture");
        assert_eq!(r[0].dossier, "Compta / 2026");
    }

    #[test]
    fn un_nom_de_dossier_avec_guillemet_ne_casse_pas_la_charge() {
        // Le module compose son JSON à la main ; le champ qui vient de l'utilisateur
        // doit être traité comme tel, sans quoi la demande est mal lue plutôt
        // qu'ignorée.
        assert_eq!(
            echappe(r#"Dossier "spécial"\x"#),
            r#"Dossier \"spécial\"\\x"#
        );
    }
}
