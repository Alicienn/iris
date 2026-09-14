//! L'interface, pilotée sans écran.
//!
//! Ces tests construisent la vraie fenêtre, la remplissent, et l'interrogent **par
//! les libellés d'accessibilité** — c'est-à-dire par ce qu'un lecteur d'écran
//! annoncerait. Deux bénéfices d'un seul geste : la mise en page est vérifiée pour de
//! bon, et l'accessibilité cesse d'être une intention, puisque les tests échouent si
//! un élément cliquable perd son libellé.
//!
//! Ils ne vérifient pas l'apparence — aucune couleur, aucune position. Ce qui est
//! vérifié, c'est ce que l'interface *dit* et ce qu'elle *fait* quand on l'actionne.
//!
//! La plateforme de test de Slint est **globale et liée à un fil**, alors que le
//! harnais de Rust crée un fil par test. Ce fichier a donc son propre `main` : les
//! scénarios s'exécutent à la suite, sur le fil principal, et chacun est isolé des
//! autres par une reprise sur panique — un scénario qui échoue ne masque pas les
//! suivants, ce qui est tout l'intérêt d'en avoir plusieurs.

use i_slint_backend_testing as testing;
use iris_ui::{AccountRowData, AppWindow, MessageData, ThreadRowData};
use slint::{Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

fn fenetre() -> AppWindow {
    AppWindow::new().expect("la fenêtre doit se construire")
}

fn ligne(id: i32, de: &str, sujet: &str, non_lu: bool) -> ThreadRowData {
    ThreadRowData {
        id,
        from: de.into(),
        subject: sujet.into(),
        preview: "Aperçu".into(),
        date: "12:30".into(),
        unread: non_lu,
        flagged: false,
        has_attachment: false,
        has_tracker: false,
        snoozed: false,
        message_count: 1,
        account_tint: slint::Color::from_rgb_u8(0, 0, 0),
    }
}

fn compte(id: i32, nom: &str, a_traiter: i32, en_panne: bool) -> AccountRowData {
    AccountRowData {
        id,
        label: nom.into(),
        count: a_traiter,
        pinned: false,
        needs_attention: en_panne,
        tint: slint::Color::from_rgb_u8(0, 0, 0),
    }
}

fn modele<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

/// Retrouve un élément par ce qu'un lecteur d'écran annoncerait.
fn par_libelle(fenetre: &AppWindow, libelle: &str) -> Option<testing::ElementHandle> {
    testing::ElementHandle::find_by_accessible_label(fenetre, libelle).next()
}

/// Les libellés distincts d'un rôle donné, dans l'ordre de l'arbre.
///
/// Le parcours du harnais visite un même élément plusieurs fois — une fois par
/// chemin qui y mène dans l'arbre des composants. Ce n'est pas ce qu'un lecteur
/// d'écran voit, et compter les occurrences brutes ferait échouer les tests sur un
/// détail d'implémentation du harnais. On dédoublonne donc, en conservant l'ordre.
fn libelles(fenetre: &AppWindow, role: testing::AccessibleRole) -> Vec<String> {
    let mut vus = std::collections::BTreeSet::new();
    let mut sortie = Vec::new();

    for element in testing::ElementQuery::from_root(fenetre)
        .match_descendants()
        .match_accessible_role(role)
        .find_all()
    {
        if let Some(libelle) = element.accessible_label() {
            let libelle = libelle.to_string();
            if vus.insert(libelle.clone()) {
                sortie.push(libelle);
            }
        }
    }
    sortie
}

// --- La liste ---

fn une_conversation_s_annonce_par_qui_ecrit_de_quoi_et_quand() {
    // L'ordre suit celui de la lecture visuelle : c'est celui dans lequel on décide
    // si un message mérite qu'on s'y arrête.
    let f = fenetre();
    f.set_rows(modele(vec![ligne(7, "Marie Dupont", "Devis refonte", true)]));

    let element = par_libelle(&f, "Marie Dupont, Devis refonte, 12:30")
        .expect("la ligne doit être annoncée");
    assert_eq!(element.accessible_role(), Some(testing::AccessibleRole::ListItem));
    assert_eq!(element.accessible_description().map(|d| d.to_string()), Some("Non lu".into()));
}

fn une_conversation_lue_le_dit() {
    let f = fenetre();
    f.set_rows(modele(vec![ligne(7, "Marie", "Devis", false)]));

    let element = par_libelle(&f, "Marie, Devis, 12:30").unwrap();
    assert_eq!(element.accessible_description().map(|d| d.to_string()), Some("Lu".into()));
}

fn actionner_une_ligne_ouvre_la_bonne_conversation() {
    // Sans l'identifiant, deux lignes voisines ouvriraient la même conversation.
    let f = fenetre();
    f.set_rows(modele(vec![
        ligne(1, "Marie", "Premier", false),
        ligne(2, "Luc", "Second", false),
    ]));

    let ouvert = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let ouvert = Rc::clone(&ouvert);
        f.on_thread_selected(move |id| ouvert.borrow_mut().push(id));
    }

    par_libelle(&f, "Luc, Second, 12:30").unwrap().invoke_accessible_default_action();
    assert_eq!(*ouvert.borrow(), [2]);
}

fn la_conversation_selectionnee_est_annoncee_comme_telle() {
    let f = fenetre();
    f.set_rows(modele(vec![
        ligne(1, "Marie", "Premier", false),
        ligne(2, "Luc", "Second", false),
    ]));
    f.set_selected_thread(2);

    let premiere = par_libelle(&f, "Marie, Premier, 12:30").unwrap();
    let seconde = par_libelle(&f, "Luc, Second, 12:30").unwrap();

    assert_eq!(premiere.accessible_item_selected(), Some(false));
    assert_eq!(seconde.accessible_item_selected(), Some(true));
}

fn une_liste_vide_ne_ment_pas_pendant_le_chargement() {
    // « Rien à traiter » avant la première lecture serait un mensonge d'un dixième
    // de seconde, mais un mensonge quand même.
    let f = fenetre();
    f.set_rows(modele(Vec::<ThreadRowData>::new()));
    f.set_loading(true);

    let textes = testing::ElementQuery::from_root(&f).match_descendants()
        .match_type_name("Text")
        .find_all();
    let dit_vide = textes
        .iter()
        .filter_map(|t| t.accessible_label())
        .any(|l| l.contains("Rien à traiter"));
    assert!(!dit_vide, "l'application ne doit rien affirmer avant d'avoir lu");
}

// --- Les onglets ---

fn les_trois_files_sont_annoncees_avec_leur_compte() {
    let f = fenetre();
    f.set_counts(modele(vec![12, 3, 40]));

    let onglets = libelles(&f, testing::AccessibleRole::Tab);
    assert_eq!(onglets, ["À traiter", "En attente", "Traité"]);

    let a_traiter = par_libelle(&f, "À traiter").unwrap();
    assert_eq!(
        a_traiter.accessible_description().map(|d| d.to_string()),
        Some("12 conversations".into())
    );
}

fn actionner_un_onglet_change_de_file() {
    let f = fenetre();
    let choisi = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let choisi = Rc::clone(&choisi);
        f.on_tab_selected(move |i| choisi.borrow_mut().push(i));
    }

    par_libelle(&f, "En attente").unwrap().invoke_accessible_default_action();
    assert_eq!(*choisi.borrow(), [1]);
}

fn l_onglet_actif_est_annonce_comme_selectionne() {
    let f = fenetre();
    f.set_active_tab(2);

    assert_eq!(par_libelle(&f, "Traité").unwrap().accessible_item_selected(), Some(true));
    assert_eq!(par_libelle(&f, "À traiter").unwrap().accessible_item_selected(), Some(false));
}

fn les_onglets_disparaissent_pendant_une_recherche() {
    // Les résultats ne sont pas rangés par file : un onglet allumé au-dessus d'eux
    // désignerait la mauvaise liste.
    let f = fenetre();
    assert_eq!(libelles(&f, testing::AccessibleRole::Tab).len(), 3);

    f.set_searching(true);
    assert!(
        libelles(&f, testing::AccessibleRole::Tab).is_empty(),
        "aucun onglet ne doit subsister pendant une recherche"
    );
}

// --- La recherche ---

fn quitter_la_recherche_est_atteignable() {
    let f = fenetre();
    f.set_searching(true);

    let quitte = Rc::new(RefCell::new(0));
    {
        let quitte = Rc::clone(&quitte);
        f.on_search_cleared(move || *quitte.borrow_mut() += 1);
    }

    par_libelle(&f, "Quitter la recherche")
        .expect("le bouton de sortie doit exister")
        .invoke_accessible_default_action();

    assert_eq!(*quitte.borrow(), 1);
    assert_eq!(f.get_search_query(), "", "le champ est vidé en même temps");
}

fn le_bouton_de_sortie_n_existe_pas_hors_recherche() {
    let f = fenetre();
    assert!(par_libelle(&f, "Quitter la recherche").is_none());
}

fn la_barre_de_recherche_est_nommee() {
    let f = fenetre();
    assert!(par_libelle(&f, "Rechercher").is_some());
}

// --- Les comptes ---

fn un_compte_en_panne_le_dit_a_voix_haute() {
    // Un lecteur d'écran ne voit pas le point d'exclamation, et c'est précisément
    // l'information qui presse.
    let f = fenetre();
    f.set_other_accounts(modele(vec![compte(3, "moi@exemple.fr", 0, true)]));

    let ligne = par_libelle(&f, "moi@exemple.fr").unwrap();
    assert_eq!(
        ligne.accessible_description().map(|d| d.to_string()),
        Some("En pause après des échecs répétés".into())
    );
}

fn un_compte_sain_annonce_ce_qu_il_reste_a_traiter() {
    let f = fenetre();
    f.set_other_accounts(modele(vec![compte(3, "moi@exemple.fr", 12, false)]));

    let ligne = par_libelle(&f, "moi@exemple.fr").unwrap();
    assert_eq!(
        ligne.accessible_description().map(|d| d.to_string()),
        Some("12 à traiter".into())
    );
}

fn la_reprise_d_un_compte_en_panne_est_un_bouton() {
    // Signaler une panne sans offrir le geste qui la répare oblige à chercher
    // ailleurs ce qui est déjà là.
    let f = fenetre();
    f.set_other_accounts(modele(vec![compte(42, "casse@exemple.fr", 0, true)]));

    let repris = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let repris = Rc::clone(&repris);
        f.on_resume_account(move |id| repris.borrow_mut().push(id));
    }

    par_libelle(&f, "Réessayer casse@exemple.fr")
        .expect("le marqueur doit être actionnable")
        .invoke_accessible_default_action();

    assert_eq!(*repris.borrow(), [42]);
}

fn un_compte_sain_n_offre_pas_de_reprise() {
    let f = fenetre();
    f.set_other_accounts(modele(vec![compte(3, "sain@exemple.fr", 0, false)]));
    assert!(par_libelle(&f, "Réessayer sain@exemple.fr").is_none());
}

fn ajouter_un_compte_est_atteignable_depuis_la_barre_laterale() {
    let f = fenetre();
    assert!(!f.get_add_account_open());

    par_libelle(&f, "Ajouter un compte")
        .expect("le bouton d'ajout doit exister")
        .invoke_accessible_default_action();

    assert!(f.get_add_account_open(), "l'écran d'ajout doit s'ouvrir");
}

// --- Les pièces jointes ---

fn une_piece_jointe_s_enregistre_par_son_nom() {
    let f = fenetre();
    let mut message = MessageData::default();
    message.attachments = modele(vec![
        SharedString::from("devis.pdf"),
        SharedString::from("plan.png"),
    ]);
    f.set_message(message);
    f.set_conversation_empty(false);

    let enregistres = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let enregistres = Rc::clone(&enregistres);
        f.on_save_attachment(move |i| enregistres.borrow_mut().push(i));
    }

    par_libelle(&f, "Enregistrer plan.png")
        .expect("la pastille doit être un bouton")
        .invoke_accessible_default_action();

    assert_eq!(*enregistres.borrow(), [1], "le rang doit désigner le bon fichier");
}

// --- Les panneaux ---

fn les_reglages_n_existent_pas_avant_d_etre_ouverts() {
    // Un panneau construit en permanence coûterait sa mise en page à chaque frame.
    let f = fenetre();
    assert!(par_libelle(&f, "Fermer les réglages").is_none());

    f.set_settings_open(true);
    assert!(par_libelle(&f, "Fermer les réglages").is_some());
}

fn fermer_les_reglages_les_ferme() {
    let f = fenetre();
    f.set_settings_open(true);

    par_libelle(&f, "Fermer les réglages").unwrap().invoke_accessible_default_action();
    assert!(!f.get_settings_open());
}

fn les_densites_sont_proposees_et_la_courante_est_marquee() {
    let f = fenetre();
    f.set_settings_open(true);
    f.set_density(0);

    assert_eq!(par_libelle(&f, "Compacte").unwrap().accessible_item_selected(), Some(true));
    assert_eq!(par_libelle(&f, "Normale").unwrap().accessible_item_selected(), Some(false));
}

fn changer_de_densite_est_rapporte() {
    let f = fenetre();
    f.set_settings_open(true);

    let choisi = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let choisi = Rc::clone(&choisi);
        f.on_density_chosen(move |i| choisi.borrow_mut().push(i));
    }

    par_libelle(&f, "Confortable").unwrap().invoke_accessible_default_action();
    assert_eq!(*choisi.borrow(), [2]);
}

fn l_ecran_d_ajout_propose_la_porte_de_secours_avant_l_echec() {
    // Quelqu'un qui sait déjà que son serveur est exotique n'a pas à attendre que la
    // découverte se trompe.
    let f = fenetre();
    f.set_add_account_open(true);

    let demande = Rc::new(RefCell::new(0));
    {
        let demande = Rc::clone(&demande);
        f.on_add_account_manual_requested(move || *demande.borrow_mut() += 1);
    }

    par_libelle(&f, "Configurer à la main")
        .expect("la porte de secours doit être visible d'emblée")
        .invoke_accessible_default_action();
    assert_eq!(*demande.borrow(), 1);
}

fn la_porte_de_secours_disparait_une_fois_franchie() {
    let f = fenetre();
    f.set_add_account_open(true);
    f.set_add_account_manual(true);

    assert!(par_libelle(&f, "Configurer à la main").is_none());
    assert!(par_libelle(&f, "Serveur IMAP").is_some(), "les champs prennent sa place");
}

fn le_bouton_d_ajout_change_de_nom_selon_le_mode() {
    let f = fenetre();
    f.set_add_account_open(true);
    assert!(par_libelle(&f, "Ajouter").is_some());

    f.set_add_account_manual(true);
    assert!(par_libelle(&f, "Enregistrer").is_some());
}

fn pendant_la_recherche_de_configuration_le_bouton_est_inactif() {
    // Sinon un double clic lancerait deux découvertes sur le même compte.
    let f = fenetre();
    f.set_add_account_open(true);
    f.set_add_account_busy(true);

    let bouton = par_libelle(&f, "Ajouter").unwrap();
    assert_eq!(bouton.accessible_enabled(), Some(false));
}

fn les_champs_de_l_ecran_d_ajout_sont_nommes() {
    // Un champ de mot de passe sans nom est un champ qu'un lecteur d'écran annonce
    // « champ de saisie », ce qui n'aide personne.
    let f = fenetre();
    f.set_add_account_open(true);

    assert!(par_libelle(&f, "Adresse").is_some());
    assert!(par_libelle(&f, "Mot de passe").is_some());
}

// --- Ce qui doit rester vrai partout ---

fn aucun_bouton_ne_reste_sans_nom() {
    // C'est le test qui empêche l'accessibilité de se dégrader : un bouton ajouté
    // sans libellé fait échouer la suite, au lieu d'être découvert par un
    // utilisateur qui n'y voit pas.
    let f = fenetre();
    f.set_rows(modele(vec![ligne(1, "Marie", "Devis", true)]));
    f.set_other_accounts(modele(vec![compte(1, "moi@exemple.fr", 2, true)]));
    f.set_searching(true);
    f.set_settings_open(true);

    let anonymes: Vec<_> = testing::ElementQuery::from_root(&f).match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .filter(|b| b.accessible_label().map(|l| l.trim().is_empty()).unwrap_or(true))
        .collect();

    assert!(anonymes.is_empty(), "{} bouton(s) sans libellé", anonymes.len());
}

fn la_fenetre_se_construit_sans_donnees() {
    // Au tout premier lancement, tous les modèles sont vides : rien ne doit
    // supposer qu'il existe au moins une ligne.
    let f = fenetre();
    assert_eq!(f.get_rows().row_count(), 0);
    assert!(f.get_conversation_empty());
}

// --- Le harnais ---

fn main() {
    testing::init_no_event_loop();

    let scenarios: Vec<(&str, fn())> = vec![
        ("une_conversation_s_annonce_par_qui_ecrit_de_quoi_et_quand", une_conversation_s_annonce_par_qui_ecrit_de_quoi_et_quand as fn()),
        ("une_conversation_lue_le_dit", une_conversation_lue_le_dit as fn()),
        ("actionner_une_ligne_ouvre_la_bonne_conversation", actionner_une_ligne_ouvre_la_bonne_conversation as fn()),
        ("la_conversation_selectionnee_est_annoncee_comme_telle", la_conversation_selectionnee_est_annoncee_comme_telle as fn()),
        ("une_liste_vide_ne_ment_pas_pendant_le_chargement", une_liste_vide_ne_ment_pas_pendant_le_chargement as fn()),
        ("les_trois_files_sont_annoncees_avec_leur_compte", les_trois_files_sont_annoncees_avec_leur_compte as fn()),
        ("actionner_un_onglet_change_de_file", actionner_un_onglet_change_de_file as fn()),
        ("l_onglet_actif_est_annonce_comme_selectionne", l_onglet_actif_est_annonce_comme_selectionne as fn()),
        ("les_onglets_disparaissent_pendant_une_recherche", les_onglets_disparaissent_pendant_une_recherche as fn()),
        ("quitter_la_recherche_est_atteignable", quitter_la_recherche_est_atteignable as fn()),
        ("le_bouton_de_sortie_n_existe_pas_hors_recherche", le_bouton_de_sortie_n_existe_pas_hors_recherche as fn()),
        ("la_barre_de_recherche_est_nommee", la_barre_de_recherche_est_nommee as fn()),
        ("un_compte_en_panne_le_dit_a_voix_haute", un_compte_en_panne_le_dit_a_voix_haute as fn()),
        ("un_compte_sain_annonce_ce_qu_il_reste_a_traiter", un_compte_sain_annonce_ce_qu_il_reste_a_traiter as fn()),
        ("la_reprise_d_un_compte_en_panne_est_un_bouton", la_reprise_d_un_compte_en_panne_est_un_bouton as fn()),
        ("un_compte_sain_n_offre_pas_de_reprise", un_compte_sain_n_offre_pas_de_reprise as fn()),
        ("ajouter_un_compte_est_atteignable_depuis_la_barre_laterale", ajouter_un_compte_est_atteignable_depuis_la_barre_laterale as fn()),
        ("une_piece_jointe_s_enregistre_par_son_nom", une_piece_jointe_s_enregistre_par_son_nom as fn()),
        ("les_reglages_n_existent_pas_avant_d_etre_ouverts", les_reglages_n_existent_pas_avant_d_etre_ouverts as fn()),
        ("fermer_les_reglages_les_ferme", fermer_les_reglages_les_ferme as fn()),
        ("les_densites_sont_proposees_et_la_courante_est_marquee", les_densites_sont_proposees_et_la_courante_est_marquee as fn()),
        ("changer_de_densite_est_rapporte", changer_de_densite_est_rapporte as fn()),
        ("l_ecran_d_ajout_propose_la_porte_de_secours_avant_l_echec", l_ecran_d_ajout_propose_la_porte_de_secours_avant_l_echec as fn()),
        ("la_porte_de_secours_disparait_une_fois_franchie", la_porte_de_secours_disparait_une_fois_franchie as fn()),
        ("le_bouton_d_ajout_change_de_nom_selon_le_mode", le_bouton_d_ajout_change_de_nom_selon_le_mode as fn()),
        ("pendant_la_recherche_de_configuration_le_bouton_est_inactif", pendant_la_recherche_de_configuration_le_bouton_est_inactif as fn()),
        ("les_champs_de_l_ecran_d_ajout_sont_nommes", les_champs_de_l_ecran_d_ajout_sont_nommes as fn()),
        ("aucun_bouton_ne_reste_sans_nom", aucun_bouton_ne_reste_sans_nom as fn()),
        ("la_fenetre_se_construit_sans_donnees", la_fenetre_se_construit_sans_donnees as fn()),
    ];

    let total = scenarios.len();
    let mut echecs = Vec::new();

    for (nom, scenario) in scenarios {
        // Chaque scénario est isolé : un échec ne doit pas masquer les suivants.
        let resultat = std::panic::catch_unwind(std::panic::AssertUnwindSafe(scenario));
        match resultat {
            Ok(()) => println!("ok    {nom}"),
            Err(_) => {
                println!("ÉCHEC {nom}");
                echecs.push(nom);
            }
        }
    }

    println!();
    if echecs.is_empty() {
        println!("interface : {total} scénarios, aucun échec");
    } else {
        println!("interface : {} échec(s) sur {total}", echecs.len());
        for nom in &echecs {
            println!("  - {nom}");
        }
        std::process::exit(1);
    }
}
