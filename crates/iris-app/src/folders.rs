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

use iris_store::{Scope, Store, UnifiedFolder};
use iris_sync::OpPayload;
use iris_types::{Error, Result, Timestamp};

/// Un nœud de l'arborescence, à plat, avec sa profondeur.
///
/// À plat parce que c'est ce que dessine une liste virtualisée : un arbre en
/// profondeur demanderait autant de modèles imbriqués que de niveaux, pour une
/// hiérarchie qui dépasse rarement deux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderNode {
    /// Ce que l'arborescence renvoie quand on clique : `role:trash`, ou un chemin.
    pub key: String,
    /// Ce qu'on affiche.
    pub name: String,
    pub depth: usize,
    pub role: iris_store::FolderRole,
    /// Un dossier de rôle, que le serveur impose et qu'on ne supprime pas.
    pub is_role: bool,
    /// Sur combien de boîtes il existe. Zéro pour un rôle qu'aucun serveur n'a.
    pub accounts: u32,
    pub threads: u32,
}

/// Les rôles montrés en permanence, dans l'ordre où on les cherche.
///
/// **Permanents** : présents même vides. Un dossier « Spam » qui disparaît quand il
/// n'y a pas de spam est un dossier qu'on croit perdu le jour où on en cherche un ;
/// et la boîte de réception ne doit jamais être absente d'une liste de dossiers.
///
/// **Dans cet ordre**, et non dans l'ordre alphabétique, qui donne « Archive,
/// Corbeille, INBOX, Indésirables » et place la boîte de réception au milieu — le seul
/// endroit où personne ne la cherche. Les deux extrémités sont les deux qu'on veut :
/// ce qui arrive en haut, ce qu'on a jeté en bas.
const ROLES: [(iris_store::FolderRole, &str); 6] = [
    (iris_store::FolderRole::Inbox, "Inbox"),
    (iris_store::FolderRole::Drafts, "Drafts"),
    (iris_store::FolderRole::Sent, "Sent"),
    (iris_store::FolderRole::Archive, "Archive"),
    (iris_store::FolderRole::Junk, "Spam"),
    (iris_store::FolderRole::Trash, "Trash"),
];

/// Le séparateur de hiérarchie.
///
/// Le point, parce que c'est celui des serveurs que nous voyons — `INBOX.Devis`. La
/// barre oblique existe ailleurs et sera lue tout aussi bien : découper sur les deux
/// coûte un caractère de plus dans un motif et évite une arborescence entièrement à
/// plat chez la moitié des hébergeurs.
const SEPARATEURS: [char; 2] = ['.', '/'];

/// Le nom lisible d'une portée.
///
/// La même table que l'arborescence, pour qu'un dossier ne porte pas deux noms sur le
/// même écran — « Spam » dans la colonne de gauche et « INBOX.spam » dans celle du
/// milieu serait deux dossiers pour l'utilisateur.
pub fn scope_name(scope: &iris_store::Scope) -> String {
    match scope {
        iris_store::Scope::Queue => String::new(),
        iris_store::Scope::Role(role) => ROLES
            .iter()
            .find(|(r, _)| r == role)
            .map(|(_, nom)| (*nom).to_string())
            .unwrap_or_else(|| role.as_str().to_string()),
        iris_store::Scope::Path(chemin) => chemin
            .rsplit(SEPARATEURS)
            .next()
            .unwrap_or(chemin)
            .to_string(),
    }
}

/// Construit l'arborescence affichable.
///
/// Deux moitiés, et la séparation est le fond de l'affaire.
///
/// **Les rôles d'abord**, un par ligne, quel que soit le nombre de dossiers réels
/// derrière. Cette boîte a deux dossiers d'indésirables — `INBOX.Junk`, vide, et
/// `INBOX.spam`, qui porte tout — parce que le serveur en a créé deux. L'utilisateur
/// n'en a qu'un en tête, et l'arborescence doit décrire ce qu'il a en tête : une ligne
/// « Spam ». Le nom vient de nous et non du serveur, ce qui règle du même coup les
/// `spam` en minuscule au milieu de `Sent` et de `Trash`.
///
/// **Puis ce que quelqu'un a créé**, en arborescence, avec sa hiérarchie. C'est là que
/// l'indentation a un sens ; sur les rôles elle n'en avait aucun, et montrer `Archive`
/// et `Trash` décalés sous `INBOX` parce que le serveur les nomme `INBOX.Archive` était
/// une vérité de protocole affichée à quelqu'un qui n'a pas à la connaître.
pub fn tree(folders: &[UnifiedFolder]) -> Vec<FolderNode> {
    let mut liste = Vec::with_capacity(folders.len() + ROLES.len());

    // --- Les rôles, toujours, dans l'ordre où on les cherche ---
    for (role, nom) in ROLES {
        let concernes: Vec<&UnifiedFolder> = folders.iter().filter(|f| f.role == role).collect();

        liste.push(FolderNode {
            key: format!("role:{}", role.as_str()),
            name: nom.to_string(),
            depth: 0,
            role,
            is_role: true,
            // Le nombre de boîtes qui ont ce rôle, pas la somme des dossiers : deux
            // dossiers d'indésirables sur la même boîte, cela reste une boîte.
            accounts: concernes.iter().map(|f| f.accounts).max().unwrap_or(0),
            threads: concernes.iter().map(|f| f.threads).sum(),
        });
    }

    // --- Puis les dossiers créés, en arborescence ---
    let mut vus: std::collections::BTreeMap<String, FolderNode> = Default::default();

    for dossier in folders
        .iter()
        .filter(|f| f.role == iris_store::FolderRole::Other)
    {
        // Le préfixe imposé par le serveur est retiré de l'affichage : `INBOX.Devis`
        // se lit « Devis ». Il reste dans la clé, qui est ce qui interroge la base.
        //
        // Chaque segment garde où il finit dans le chemin réel, pour que la clé d'un
        // parent fabriqué soit un préfixe de ce chemin. Elle était reconstruite en
        // `INBOX.` + points : chez un serveur qui sépare par `/` et ne préfixe rien,
        // cela désignait un dossier qui n'existe nulle part, que la suppression ne
        // trouvait pas — et qui revenait aussitôt.
        let chemin = dossier.path.as_str();
        let mut segments: Vec<(&str, usize)> = Vec::new();
        let mut debut = 0;
        for (i, c) in chemin.char_indices() {
            if SEPARATEURS.contains(&c) {
                segments.push((&chemin[debut..i], i));
                debut = i + c.len_utf8();
            }
        }
        segments.push((&chemin[debut..], chemin.len()));
        segments.retain(|(s, _)| !s.eq_ignore_ascii_case("INBOX"));

        for (i, (segment, fin)) in segments.iter().enumerate() {
            let affiche: Vec<&str> = segments[..=i].iter().map(|(s, _)| *s).collect();
            let feuille = i + 1 == segments.len();

            let noeud = vus.entry(affiche.join(".")).or_insert_with(|| FolderNode {
                name: segment.to_string(),
                key: chemin[..*fin].to_string(),
                depth: i,
                role: iris_store::FolderRole::Other,
                is_role: false,
                accounts: 0,
                threads: 0,
            });

            // Seule la feuille porte les chiffres du dossier : les additionner sur
            // les parents ferait compter deux fois un message rangé dans une
            // sous-branche.
            if feuille {
                noeud.accounts = dossier.accounts;
                noeud.threads = dossier.threads;
            }
        }
    }

    // `BTreeMap` rend ses valeurs dans l'ordre de ses clés, et les clés sont les
    // chemins affichés : « Devis » précède « Devis.2026 ». Chaque enfant sort donc
    // sous son parent, sans qu'on ait eu à parcourir l'arbre ni à trier ensuite.
    liste.extend(vus.into_values());
    liste
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

/// Renomme un dossier, sur toutes les boîtes qui l'ont.
///
/// Le nouveau nom garde le parent de l'ancien : renommer « Devis » en « Offres » sous
/// `INBOX` donne `INBOX.Offres`, pas `INBOX.Devis.Offres` ni `Offres` à la racine.
pub fn rename_everywhere(store: &Store, path: &str, name: &str, now: Timestamp) -> Result<usize> {
    let nom = validate(name).map_err(Error::Config)?;

    let parent = path.rsplit_once(SEPARATEURS).map(|(p, _)| p.to_string());
    let cible = match &parent {
        Some(p) => format!("{p}.{nom}"),
        None => nom,
    };
    if cible == path {
        return Ok(0);
    }

    let comptes = store.accounts_with_folder(path)?;
    for compte in &comptes {
        let charge = iris_store::OpPayload::RenameFolder {
            folder: path.to_string(),
            target: cible.clone(),
        };
        iris_sync::enqueue(store, *compte, &charge, now)?;
    }

    Ok(comptes.len())
}

/// Supprime un dossier, **en gardant ce qu'il contient**.
///
/// C'est la contrainte qui décide de tout le reste. `DELETE` sur un dossier plein
/// détruit son contenu sur le serveur ; personne ne s'attend à perdre du courrier en
/// rangeant ses dossiers. Le courrier est donc déplacé vers la boîte de réception
/// **avant**, par des opérations enfilées devant la suppression — le journal les rejoue
/// dans l'ordre par compte, ce qui garantit que le dossier est vide quand son tour
/// arrive.
///
/// Un rôle ne se supprime pas : la corbeille, les indésirables et la boîte de réception
/// appartiennent au serveur, et les retirer d'ici les ferait revenir à la
/// synchronisation suivante en donnant l'impression que la suppression a échoué.
/// Marque lu tout ce que contient un dossier, sur toutes les boîtes qui l'ont.
///
/// L'autre moitié du travail après une semaine d'absence : ouvrir deux cents messages
/// un par un pour éteindre une pastille n'est pas du triage, et tout client de courrier
/// sait le faire depuis toujours.
///
/// Comme le reste, par le journal : le drapeau est posé localement et l'ordre part vers
/// le serveur, en un lot par cinquante plutôt qu'un par message.
pub fn mark_read_everywhere(store: &Store, scope: &Scope, now: Timestamp) -> Result<usize> {
    let mut touches = 0usize;

    for compte in store.accounts()? {
        for dossier in store.folders(compte.id)? {
            if !concerne(scope, &dossier) {
                continue;
            }

            let non_lus = store.folder_unread_uids(dossier.id)?;
            if non_lus.is_empty() {
                continue;
            }

            // Localement d'abord : la pastille doit s'éteindre au clic, pas à la
            // synchronisation suivante.
            store.mark_folder_read(dossier.id)?;

            for lot in non_lus.chunks(50) {
                let charge = iris_store::OpPayload::SetFlags {
                    folder: dossier.path.clone(),
                    uids: lot.to_vec(),
                    flags: iris_types::Flags::SEEN.0,
                    add: true,
                };
                iris_sync::enqueue(store, compte.id, &charge, now)?;
            }
            touches += non_lus.len();
        }
    }

    Ok(touches)
}

/// Jette tout ce que contient un dossier, sur toutes les boîtes qui l'ont.
///
/// Seulement la corbeille et les indésirables. « Vider la boîte de réception » n'est pas
/// une commande, c'est un accident : ces deux dossiers-là sont les seuls dont le contenu
/// a déjà été décidé, et vider ailleurs supprimerait du courrier que personne n'a jugé.
pub fn empty_everywhere(store: &Store, scope: &Scope, now: Timestamp) -> Result<usize> {
    if !videable(scope) {
        return Err(Error::Config(
            "only the bin and the junk folder can be emptied".into(),
        ));
    }

    let mut jetes = 0usize;

    for compte in store.accounts()? {
        for dossier in store.folders(compte.id)? {
            if !concerne(scope, &dossier) {
                continue;
            }

            let uids = store.folder_uids(dossier.id)?;
            if uids.is_empty() {
                continue;
            }

            for lot in uids.chunks(50) {
                let charge = iris_store::OpPayload::Delete {
                    folder: dossier.path.clone(),
                    uids: lot.to_vec(),
                };
                iris_sync::enqueue(store, compte.id, &charge, now)?;
            }

            // Localement tout de suite, comme pour la suppression d'un dossier : la
            // corbeille doit se vider sous les yeux, pas à la prochaine passe.
            store.clear_folder(dossier.id)?;
            jetes += uids.len();
        }
    }

    Ok(jetes)
}

/// Le dossier est-il celui que la portée désigne ?
fn concerne(scope: &Scope, dossier: &iris_store::Folder) -> bool {
    match scope {
        Scope::Role(role) => dossier.role == *role,
        Scope::Path(chemin) => dossier.path == *chemin,
        // « Toutes les files » ne désigne aucun dossier, et un geste qui viderait tout
        // parce qu'on n'a rien choisi serait le pire de cet écran.
        Scope::Queue => false,
    }
}

/// Vider n'a de sens que là où le contenu est déjà jugé.
fn videable(scope: &Scope) -> bool {
    matches!(
        scope,
        Scope::Role(iris_store::FolderRole::Trash) | Scope::Role(iris_store::FolderRole::Junk)
    )
}

pub fn delete_everywhere(store: &Store, path: &str, now: Timestamp) -> Result<usize> {
    let comptes = store.accounts_with_folder(path)?;

    // Un dossier qu'aucune boîte ne porte : le parent qu'on a fabriqué pour ranger des
    // sous-dossiers. Annoncer « supprimé sur 0 boîte » pour le voir aussitôt revenir
    // était le pire des deux mondes.
    if comptes.is_empty() {
        return Err(Error::Config(
            "this folder only groups the ones inside it; delete those instead".into(),
        ));
    }

    for compte in &comptes {
        let dossiers = store.folders(*compte)?;
        let Some(source) = dossiers.iter().find(|f| f.path == path) else {
            continue;
        };
        if source.role != iris_store::FolderRole::Other {
            return Err(Error::Config(
                "that folder belongs to the server and cannot be removed".into(),
            ));
        }

        // Où va le courrier. La boîte de réception : c'est l'endroit d'où il vient et
        // celui où on ira le rechercher. Le mettre à la corbeille serait interpréter
        // « je ne veux plus de ce dossier » comme « je ne veux plus de ce courrier ».
        let Some(refuge) = dossiers
            .iter()
            .find(|f| f.role == iris_store::FolderRole::Inbox)
        else {
            return Err(Error::Config(
                "this account has no inbox to move the mail into".into(),
            ));
        };

        // Un déplacement par lot de cinquante : une opération par message ferait
        // autant d'entrées de journal que le dossier a de courrier, et un dossier de
        // huit cents messages produirait huit cents allers-retours là où seize
        // suffisent.
        for lot in store.folder_uids(source.id)?.chunks(50) {
            let charge = iris_store::OpPayload::Move {
                folder: path.to_string(),
                uids: lot.to_vec(),
                target: refuge.path.clone(),
            };
            iris_sync::enqueue(store, *compte, &charge, now)?;
        }

        let charge = iris_store::OpPayload::DeleteFolder {
            folder: path.to_string(),
        };
        iris_sync::enqueue(store, *compte, &charge, now)?;

        // Et localement, tout de suite.
        //
        // Ce bloc manquait, et c'est tout le défaut : le serveur recevait bien les deux
        // ordres, mais la colonne des dossiers lit la copie locale, où la ligne restait
        // pour toujours. Le dossier supprimé revenait à chaque ouverture, et chaque
        // synchronisation essayait ensuite de sélectionner une boîte que le serveur
        // avait déjà retirée.
        //
        // Le vidage précède l'oubli pour que les fils soient recalculés au passage :
        // sans lui, la cascade emporterait les messages sans que personne ne remette à
        // jour les agrégats du fil, et la liste montrerait des conversations dont le
        // compteur ne correspond plus à rien.
        store.clear_folder(source.id)?;
        store.forget_folder(*compte, path)?;
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

    fn avec_role(path: &str, role: FolderRole, threads: u32) -> UnifiedFolder {
        UnifiedFolder {
            path: path.into(),
            role,
            accounts: 2,
            threads,
        }
    }

    fn noeud<'a>(arbre: &'a [FolderNode], nom: &str) -> &'a FolderNode {
        arbre
            .iter()
            .find(|n| n.name == nom)
            .unwrap_or_else(|| panic!("« {nom} » manque à l'arborescence"))
    }

    #[test]
    fn les_roles_sont_la_meme_quand_aucun_serveur_ne_les_a() {
        // Un dossier « Spam » qui disparaît quand il n'y a pas de spam est un dossier
        // qu'on croit perdu le jour où on en cherche un.
        let arbre = tree(&[]);
        for attendu in ["Inbox", "Drafts", "Sent", "Archive", "Spam", "Trash"] {
            assert!(
                arbre.iter().any(|n| n.name == attendu),
                "« {attendu} » doit être là même vide"
            );
        }
    }

    #[test]
    fn la_boite_de_reception_vient_en_premier_et_la_corbeille_en_dernier() {
        // Par ordre alphabétique la boîte de réception serait au milieu, entre
        // « Archive » et « Trash » : le seul endroit où personne ne la cherche.
        let arbre = tree(&[]);
        assert_eq!(arbre[0].name, "Inbox");
        assert_eq!(arbre[ROLES.len() - 1].name, "Trash");
    }

    #[test]
    fn deux_dossiers_d_indesirables_ne_font_qu_une_ligne() {
        // Ce serveur en a deux : « INBOX.Junk », vide, et « INBOX.spam », qui porte
        // tout. L'utilisateur n'en a qu'un en tête, et c'est ce qu'il faut montrer.
        let arbre = tree(&[
            avec_role("INBOX.Junk", FolderRole::Junk, 0),
            avec_role("INBOX.spam", FolderRole::Junk, 173),
        ]);

        let spam: Vec<&FolderNode> = arbre.iter().filter(|n| n.role == FolderRole::Junk).collect();
        assert_eq!(spam.len(), 1, "une seule ligne pour les deux dossiers");
        assert_eq!(spam[0].name, "Spam", "le nom vient de nous, pas du serveur");
        assert_eq!(spam[0].threads, 173, "et il porte le total des deux");
    }

    #[test]
    fn un_role_est_designe_par_son_role_et_non_par_un_chemin() {
        let arbre = tree(&[avec_role("INBOX.spam", FolderRole::Junk, 3)]);
        assert_eq!(noeud(&arbre, "Spam").key, "role:junk");
        assert!(noeud(&arbre, "Spam").is_role);
    }

    #[test]
    fn les_dossiers_crees_viennent_apres_les_roles() {
        let arbre = tree(&[dossier("INBOX.Devis", 7)]);
        let position = arbre.iter().position(|n| n.name == "Devis").unwrap();
        assert!(position >= ROLES.len(), "les rôles d'abord, toujours");
        assert!(!noeud(&arbre, "Devis").is_role);
    }

    #[test]
    fn le_prefixe_du_serveur_disparait_de_l_affichage() {
        // « INBOX.Devis » se lit « Devis ». Le préfixe est une vérité de protocole,
        // affichée à quelqu'un qui n'a pas à la connaître — et elle décalait tous les
        // dossiers d'un cran sous une boîte de réception dont ils ne dépendent pas.
        let arbre = tree(&[dossier("INBOX.Devis", 7)]);
        let devis = noeud(&arbre, "Devis");
        assert_eq!(devis.depth, 0, "premier niveau à l'écran");
        assert_eq!(devis.key, "INBOX.Devis", "mais le vrai chemin interroge la base");
    }

    #[test]
    fn la_hierarchie_creee_garde_sa_profondeur() {
        let arbre = tree(&[dossier("INBOX.Devis", 3), dossier("INBOX.Devis.2026", 7)]);
        assert_eq!(noeud(&arbre, "Devis").depth, 0);
        assert_eq!(noeud(&arbre, "2026").depth, 1);
    }

    #[test]
    fn un_parent_manquant_est_fabrique() {
        // Un serveur peut annoncer la feuille sans la branche. Sans ce rattrapage la
        // branche entière serait invisible.
        let arbre = tree(&[dossier("INBOX.Devis.2026", 4)]);
        assert_eq!(
            noeud(&arbre, "Devis").threads,
            0,
            "un parent fabriqué ne compte rien pour son propre compte"
        );
        assert_eq!(noeud(&arbre, "2026").threads, 4);
    }

    #[test]
    fn un_enfant_sort_sous_son_parent() {
        let arbre = tree(&[dossier("INBOX.Devis.2026", 1), dossier("INBOX.Devis", 1)]);
        let parent = arbre.iter().position(|n| n.name == "Devis").unwrap();
        let enfant = arbre.iter().position(|n| n.name == "2026").unwrap();
        assert!(parent < enfant);
    }

    #[test]
    fn les_chiffres_ne_remontent_pas_dans_les_parents() {
        // Additionner les enfants ferait compter deux fois un message rangé dans une
        // sous-branche, et un total qui ne correspond à aucune liste est pire que pas
        // de total du tout.
        let arbre = tree(&[dossier("INBOX.Devis", 10), dossier("INBOX.Devis.2026", 4)]);
        assert_eq!(noeud(&arbre, "Devis").threads, 10);
    }

    #[test]
    fn la_barre_oblique_est_lue_comme_le_point() {
        let arbre = tree(&[dossier("INBOX/Devis", 1)]);
        assert_eq!(noeud(&arbre, "Devis").depth, 0);
    }

    #[test]
    fn un_parent_fabrique_porte_un_prefixe_du_chemin_reel() {
        // Sa clé était `INBOX.` + points quel que soit le serveur : un dossier qui
        // n'existait nulle part, que la suppression ne trouvait pas.
        let arbre = tree(&[dossier("Projets/Devis/2026", 1)]);
        assert_eq!(noeud(&arbre, "Projets").key, "Projets");
        assert_eq!(noeud(&arbre, "Devis").key, "Projets/Devis");
        assert_eq!(noeud(&arbre, "2026").key, "Projets/Devis/2026");

        let arbre = tree(&[dossier("INBOX.Devis.2026", 1)]);
        assert_eq!(noeud(&arbre, "Devis").key, "INBOX.Devis");
    }

    #[test]
    fn supprimer_un_parent_fabrique_est_refuse() {
        let store = Store::in_memory().unwrap();
        assert!(delete_everywhere(&store, "Projets", Timestamp::from_millis(0)).is_err());
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

    #[test]
    fn supprimer_un_dossier_le_retire_aussi_de_la_copie_locale() {
        // Le défaut : les deux opérations partaient bien dans le journal, et rien
        // n'était fait ici. La colonne des dossiers lit la copie locale — le dossier
        // « supprimé » revenait donc à chaque ouverture, et chaque synchronisation
        // essayait ensuite de sélectionner une boîte que le serveur avait déjà retirée.
        let store = Store::in_memory().unwrap();
        let maintenant = Timestamp::from_millis(0);
        let compte = store
            .create_account(
                &iris_store::NewAccount::new("a@x.fr", "i", "s"),
                maintenant,
            )
            .unwrap();
        store
            .upsert_folder(compte, "INBOX", FolderRole::Inbox)
            .unwrap();
        store
            .upsert_folder(compte, "INBOX.Devis", FolderRole::Other)
            .unwrap();

        assert_eq!(delete_everywhere(&store, "INBOX.Devis", maintenant).unwrap(), 1);

        let restants: Vec<_> = store
            .folders(compte)
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(restants, ["INBOX"], "le dossier doit disparaître d'ici aussi");
    }

    #[test]
    fn un_dossier_du_serveur_ne_se_supprime_pas() {
        // La boîte de réception, la corbeille et les indésirables appartiennent au
        // serveur. Les retirer localement les ferait revenir à la synchronisation
        // suivante, en ayant perdu ce qu'ils contenaient entre-temps.
        let store = Store::in_memory().unwrap();
        let maintenant = Timestamp::from_millis(0);
        let compte = store
            .create_account(
                &iris_store::NewAccount::new("a@x.fr", "i", "s"),
                maintenant,
            )
            .unwrap();
        store
            .upsert_folder(compte, "INBOX", FolderRole::Inbox)
            .unwrap();

        assert!(delete_everywhere(&store, "INBOX", maintenant).is_err());
        assert_eq!(store.folders(compte).unwrap().len(), 1);
    }
}
