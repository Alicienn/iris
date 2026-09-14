//! Les dossiers, comme un seul jeu plutôt que comme cent.
//!
//! Le choix de conception tient en une phrase : **un dossier est un nom, pas un
//! endroit**. « Devis » sur douze boîtes est un dossier, pas douze. C'est la seule
//! vue qui tienne au-delà de quelques comptes — une arborescence qui répète la même
//! dizaine de noms pour chaque boîte est une arborescence que personne ne déplie — et
//! c'est aussi la seule qui corresponde à la manière dont on s'en sert : on range un
//! devis dans « Devis », sans se demander par quelle adresse il est arrivé.
//!
//! Deux conséquences, l'une et l'autre voulues :
//!
//! - **Créer, c'est créer partout.** Un dossier qui n'existerait que sur une boîte
//!   serait invisible depuis les autres, et rangerait le courrier à moitié.
//! - **Choisir un compte *et* un dossier croise les deux.** C'est la question « ce
//!   qu'il y a dans Devis, chez ce client-là », et c'est celle qu'on pose le plus
//!   souvent une fois qu'on a plus d'une boîte.
//!
//! La création passe par le journal d'opérations, comme toute écriture distante :
//! cent créations sont cent allers-retours, et faire attendre l'utilisateur devant
//! eux contredirait l'invariant n° 3. L'arborescence montre le dossier tout de suite ;
//! les serveurs l'apprennent ensuite.

use iris_store::{Store, UnifiedFolder};
use iris_sync::OpPayload;
use iris_types::{Error, Result, Timestamp};

/// Un nœud de l'arborescence, à plat, avec sa profondeur.
///
/// À plat parce que c'est ce que dessine une liste virtualisée : un arbre en
/// profondeur demanderait autant de modèles imbriqués que de niveaux, pour une
/// hiérarchie qui dépasse rarement deux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderNode {
    /// Le chemin complet, qui est l'identité du dossier : `INBOX.Devis.2026`.
    pub path: String,
    /// Le dernier segment, qui est ce qu'on affiche : `2026`.
    pub name: String,
    pub depth: usize,
    pub role: iris_store::FolderRole,
    /// Sur combien de boîtes il existe.
    pub accounts: u32,
    pub threads: u32,
}

/// Le séparateur de hiérarchie.
///
/// Le point, parce que c'est celui des serveurs que nous voyons — `INBOX.Devis`. La
/// barre oblique existe ailleurs et sera lue tout aussi bien : découper sur les deux
/// coûte un caractère de plus dans un motif et évite une arborescence entièrement à
/// plat chez la moitié des hébergeurs.
const SEPARATEURS: [char; 2] = ['.', '/'];

/// Construit l'arborescence affichable.
///
/// Les nœuds intermédiaires manquants sont fabriqués : un serveur peut annoncer
/// `INBOX.Devis.2026` sans annoncer `INBOX.Devis`, et sans ce rattrapage la branche
/// serait orpheline et invisible.
pub fn tree(folders: &[UnifiedFolder]) -> Vec<FolderNode> {
    let mut vus: std::collections::BTreeMap<String, FolderNode> = Default::default();

    for dossier in folders {
        let segments: Vec<&str> = dossier.path.split(SEPARATEURS).collect();

        for (i, _) in segments.iter().enumerate() {
            let chemin = segments[..=i].join(".");
            let feuille = i + 1 == segments.len();

            let noeud = vus.entry(chemin.clone()).or_insert_with(|| FolderNode {
                name: segments[i].to_string(),
                path: chemin.clone(),
                depth: i,
                role: iris_store::FolderRole::Other,
                accounts: 0,
                threads: 0,
            });

            // Seule la feuille porte les chiffres du dossier : les additionner sur
            // les parents ferait compter deux fois un message rangé dans une
            // sous-branche.
            if feuille {
                noeud.role = dossier.role;
                noeud.accounts = dossier.accounts;
                noeud.threads = dossier.threads;
            }
        }
    }

    let mut liste: Vec<FolderNode> = vus.into_values().collect();
    // Par chemin, ce qui met chaque enfant sous son parent : c'est l'ordre de
    // l'arborescence, obtenu sans la parcourir.
    liste.sort_by_key(|n| ordre(&n.path));
    liste
}

/// La clé de tri : les rôles connus d'abord, puis l'alphabet.
///
/// La boîte de réception en tête et la corbeille en queue, parce que c'est l'ordre
/// dans lequel on les cherche, et parce que « Archive, Corbeille, INBOX, Indésirables »
/// par ordre alphabétique met la boîte de réception au milieu.
fn ordre(chemin: &str) -> (u8, String) {
    let rang = match chemin.to_ascii_uppercase().as_str() {
        "INBOX" => 0,
        _ => 1,
    };
    (rang, chemin.to_ascii_lowercase())
}

/// Le nom d'un dossier est-il utilisable ?
///
/// Refusé plutôt que corrigé : un nom qu'on nettoie en silence donne un dossier qui
/// ne s'appelle pas comme ce que l'utilisateur a tapé, et il le cherchera sous le nom
/// qu'il a écrit.
pub fn validate(name: &str) -> std::result::Result<String, String> {
    let nom = name.trim();

    if nom.is_empty() {
        return Err("The folder needs a name.".into());
    }
    if nom.len() > 200 {
        return Err("That name is too long.".into());
    }
    // Les séparateurs sont réservés à la hiérarchie, et les caractères de contrôle
    // sont interdits par le protocole lui-même.
    if nom.contains(SEPARATEURS) {
        return Err("A name cannot contain \".\" or \"/\".".into());
    }
    if nom.contains(['"', '\\', '%', '*']) || nom.chars().any(char::is_control) {
        return Err("A name cannot contain \" \\ % or *.".into());
    }

    Ok(nom.to_string())
}

/// Demande la création d'un dossier sur toutes les boîtes qui ne l'ont pas.
///
/// Renvoie le nombre de comptes à qui la demande a été adressée. Zéro n'est pas une
/// erreur : il veut dire que le dossier existe déjà partout, ce qui est exactement
/// l'état recherché.
pub fn create_everywhere(store: &Store, parent: Option<&str>, name: &str, now: Timestamp) -> Result<usize> {
    let nom = validate(name).map_err(Error::Config)?;

    let chemin = match parent.map(str::trim).filter(|p| !p.is_empty()) {
        Some(parent) => format!("{parent}.{nom}"),
        // Sans parent choisi, sous la boîte de réception : c'est là que les serveurs
        // qui imposent un préfixe attendent les dossiers de l'utilisateur, et créer à
        // la racine échoue chez eux sans que rien ne le laisse prévoir.
        None => format!("INBOX.{nom}"),
    };

    let comptes = store.accounts_without_folder(&chemin)?;
    for compte in &comptes {
        let charge = OpPayload::CreateFolder {
            folder: chemin.clone(),
        };
        iris_sync::enqueue(store, *compte, &charge, now)?;
    }

    Ok(comptes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_store::FolderRole;

    fn dossier(path: &str, threads: u32) -> UnifiedFolder {
        UnifiedFolder {
            path: path.into(),
            role: FolderRole::Other,
            accounts: 2,
            threads,
        }
    }

    #[test]
    fn la_boite_de_reception_vient_en_premier() {
        // Par ordre alphabétique elle serait au milieu, entre « Archive » et
        // « Trash », ce qui est le seul endroit où personne ne la cherche.
        let arbre = tree(&[dossier("Archive", 1), dossier("INBOX", 5), dossier("Trash", 2)]);
        assert_eq!(arbre[0].path, "INBOX");
    }

    #[test]
    fn la_hierarchie_donne_sa_profondeur_a_chaque_noeud() {
        let arbre = tree(&[dossier("INBOX", 3), dossier("INBOX.Devis", 7)]);
        let devis = arbre.iter().find(|n| n.path == "INBOX.Devis").unwrap();
        assert_eq!(devis.depth, 1);
        assert_eq!(devis.name, "Devis", "on affiche le segment, pas le chemin");
    }

    #[test]
    fn un_parent_manquant_est_fabrique() {
        // Un serveur peut annoncer la feuille sans la branche. Sans ce rattrapage la
        // branche entière serait invisible.
        let arbre = tree(&[dossier("INBOX.Devis.2026", 4)]);
        assert!(arbre.iter().any(|n| n.path == "INBOX.Devis"));
        assert_eq!(
            arbre.iter().find(|n| n.path == "INBOX.Devis").unwrap().threads,
            0,
            "un parent fabriqué ne compte rien pour son propre compte"
        );
    }

    #[test]
    fn les_chiffres_ne_remontent_pas_dans_les_parents() {
        // Additionner les enfants ferait compter deux fois un message rangé dans une
        // sous-branche, et un total qui ne correspond à aucune liste est pire que pas
        // de total du tout.
        let arbre = tree(&[dossier("INBOX", 10), dossier("INBOX.Devis", 4)]);
        let inbox = arbre.iter().find(|n| n.path == "INBOX").unwrap();
        assert_eq!(inbox.threads, 10);
    }

    #[test]
    fn la_barre_oblique_est_lue_comme_le_point() {
        let arbre = tree(&[dossier("INBOX/Devis", 1)]);
        assert!(arbre.iter().any(|n| n.name == "Devis" && n.depth == 1));
    }

    #[test]
    fn un_nom_vide_est_refuse() {
        assert!(validate("   ").is_err());
    }

    #[test]
    fn un_separateur_dans_le_nom_est_refuse() {
        // Corriger en silence donnerait un dossier qui ne porte pas le nom tapé, et
        // l'utilisateur le chercherait sous celui qu'il a écrit.
        assert!(validate("Devis.2026").is_err());
        assert!(validate("Devis/2026").is_err());
    }

    #[test]
    fn les_caracteres_reserves_du_protocole_sont_refuses() {
        for mauvais in ["Devis\"", "Devis\\", "Dev%is", "Dev*is"] {
            assert!(validate(mauvais).is_err(), "« {mauvais} » aurait dû être refusé");
        }
    }

    #[test]
    fn un_nom_correct_est_conserve_tel_quel_aux_espaces_pres() {
        assert_eq!(validate("  Devis clients  ").unwrap(), "Devis clients");
    }
}
