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
use iris_ui::{AccountRowData, AppWindow, MessageData, PluginRowData, RuleRowData, ThreadRowData};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
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
        marked: false,
        message_count: 1,
        account_tint: slint::Color::from_rgb_u8(0, 0, 0),
        initials: "M".into(),
        sender_tint: slint::Color::from_rgb_u8(0, 0, 0),
    }
}

fn compte(id: i32, nom: &str, a_traiter: i32, en_panne: bool) -> AccountRowData {
    AccountRowData {
        id,
        label: nom.into(),
        count: a_traiter,
        count_label: iris_ui::format::short_count(a_traiter.max(0) as u64).into(),
        count_full: iris_ui::format::grouped_count(a_traiter.max(0) as u64).into(),
        pinned: false,
        needs_attention: en_panne,
        problem: Default::default(),
        tint: slint::Color::from_rgb_u8(0, 0, 0),
        ..Default::default()
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
    f.set_rows(modele(vec![ligne(
        7,
        "Marie Dupont",
        "Devis refonte",
        true,
    )]));

    let element =
        par_libelle(&f, "Marie Dupont, Devis refonte, 12:30").expect("la ligne doit être annoncée");
    assert_eq!(
        element.accessible_role(),
        Some(testing::AccessibleRole::ListItem)
    );
    assert_eq!(
        element.accessible_description().map(|d| d.to_string()),
        Some("Unread".into())
    );
}

fn une_conversation_lue_le_dit() {
    let f = fenetre();
    f.set_rows(modele(vec![ligne(7, "Marie", "Devis", false)]));

    let element = par_libelle(&f, "Marie, Devis, 12:30").unwrap();
    assert_eq!(
        element.accessible_description().map(|d| d.to_string()),
        Some("Read".into())
    );
}

fn la_case_a_cocher_occupe_toujours_sa_colonne() {
    // Elle n'était dessinée qu'au survol, et sa propre zone tactile volait ce survol à
    // la ligne : la condition redevenait fausse, la case disparaissait, la ligne
    // récupérait le survol, la case revenait — une boucle à la fréquence de l'écran, et
    // vingt-trois pixels de contenu qui se décalaient à chaque aller-retour.
    //
    // Elle est maintenant toujours là, et seule son opacité change. Un test
    // d'accessibilité ne survole rien : que la case existe ici est exactement la preuve
    // que sa présence ne dépend plus du curseur.
    let f = fenetre();
    f.set_rows(modele(vec![ligne(7, "Marie", "Devis", false)]));

    let case = par_libelle(&f, "Select this conversation").expect("la case à cocher");
    assert_eq!(case.accessible_checked(), Some(false));
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

    par_libelle(&f, "Luc, Second, 12:30")
        .unwrap()
        .invoke_accessible_default_action();
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

    let textes = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_type_name("Text")
        .find_all();
    let dit_vide = textes
        .iter()
        .filter_map(|t| t.accessible_label())
        .any(|l| l.contains("Nothing to do"));
    assert!(
        !dit_vide,
        "l'application ne doit rien affirmer avant d'avoir lu"
    );
}

// --- Les onglets ---

/// Les onglets des files, sans ceux de la barre de titre (Mail, Calendar), qui ont
/// le même rôle et désignent autre chose.
fn onglets_des_files(f: &AppWindow) -> Vec<String> {
    libelles(f, testing::AccessibleRole::Tab)
        .into_iter()
        .filter(|l| l != "Mail" && l != "Calendar" && l != "Tasks")
        .collect()
}

fn les_trois_files_sont_annoncees_avec_leur_compte() {
    let f = fenetre();
    f.set_counts(modele(vec![12, 3, 40]));

    let onglets = onglets_des_files(&f);
    assert_eq!(onglets, ["To do", "Waiting", "Done"]);

    let a_traiter = par_libelle(&f, "To do").unwrap();
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

    par_libelle(&f, "Waiting")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*choisi.borrow(), [1]);
}

fn l_onglet_actif_est_annonce_comme_selectionne() {
    let f = fenetre();
    f.set_active_tab(2);

    assert_eq!(
        par_libelle(&f, "Done").unwrap().accessible_item_selected(),
        Some(true)
    );
    assert_eq!(
        par_libelle(&f, "To do").unwrap().accessible_item_selected(),
        Some(false)
    );
}

fn les_onglets_disparaissent_pendant_une_recherche() {
    // Les résultats ne sont pas rangés par file : un onglet allumé au-dessus d'eux
    // désignerait la mauvaise liste.
    let f = fenetre();
    assert_eq!(onglets_des_files(&f).len(), 3);

    f.set_searching(true);
    assert!(
        onglets_des_files(&f).is_empty(),
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

    par_libelle(&f, "Leave search")
        .expect("le bouton de sortie doit exister")
        .invoke_accessible_default_action();

    assert_eq!(*quitte.borrow(), 1);
    assert_eq!(f.get_search_query(), "", "le champ est vidé en même temps");
}

fn le_bouton_de_sortie_n_existe_pas_hors_recherche() {
    let f = fenetre();
    assert!(par_libelle(&f, "Leave search").is_none());
}

fn la_barre_de_recherche_est_nommee() {
    let f = fenetre();
    assert!(par_libelle(&f, "Search").is_some());
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
        Some("Paused after repeated failures".into())
    );
}

fn un_compte_dont_la_synchro_a_echoue_dit_pourquoi() {
    // Un seul échec suffit : attendre la mise en pause laissait le compte muet
    // pendant plusieurs tours, alors que chaque synchronisation échouait.
    let f = fenetre();
    let mut c = compte(3, "moi@exemple.fr", 0, true);
    c.problem = "password refused".into();
    f.set_other_accounts(modele(vec![c]));

    let ligne = par_libelle(&f, "moi@exemple.fr").unwrap();
    assert_eq!(
        ligne.accessible_description().map(|d| d.to_string()),
        Some("Sync failed: password refused".into())
    );
    assert!(
        par_libelle(&f, "Sync failed: password refused. Click for details.").is_some(),
        "le point d'exclamation porte la raison"
    );
}

fn un_compte_sain_annonce_ce_qu_il_reste_a_traiter() {
    let f = fenetre();
    f.set_other_accounts(modele(vec![compte(3, "moi@exemple.fr", 12, false)]));

    let ligne = par_libelle(&f, "moi@exemple.fr").unwrap();
    assert_eq!(
        ligne.accessible_description().map(|d| d.to_string()),
        Some("12 to do".into())
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

    par_libelle(&f, "Fix casse@exemple.fr")
        .expect("le marqueur doit être actionnable")
        .invoke_accessible_default_action();

    assert_eq!(*repris.borrow(), [42]);
}

fn un_compte_sain_n_offre_pas_de_reprise() {
    let f = fenetre();
    f.set_other_accounts(modele(vec![compte(3, "sain@exemple.fr", 0, false)]));
    assert!(par_libelle(&f, "Fix sain@exemple.fr").is_none());
}

fn ajouter_un_compte_est_atteignable_depuis_la_barre_laterale() {
    let f = fenetre();
    assert!(!f.get_add_account_open());

    par_libelle(&f, "Add an account")
        .expect("le bouton d'ajout doit exister")
        .invoke_accessible_default_action();

    assert!(f.get_add_account_open(), "l'écran d'ajout doit s'ouvrir");
}

// --- Les pièces jointes ---

fn piece(nom: &str, genre: &str, taille: &str) -> iris_ui::AttachmentData {
    iris_ui::AttachmentData {
        name: nom.into(),
        kind: genre.into(),
        size: taille.into(),
        icon: "paperclip".into(),
    }
}

fn une_piece_jointe_s_enregistre_par_son_nom() {
    let f = fenetre();
    // Shown on the card of the message being read.
    let lu = MessageData {
        attachments: modele(vec![
            piece("devis.pdf", "PDF", "2,4 Mo"),
            piece("plan.png", "Image", "180 ko"),
        ]),
        expanded: true,
        ..Default::default()
    };
    f.set_message(lu.clone());
    f.set_messages(modele(vec![lu]));
    f.set_conversation_empty(false);

    let enregistres = Rc::new(RefCell::new(Vec::<i32>::new()));
    let ouverts = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let enregistres = Rc::clone(&enregistres);
        f.on_save_attachment(move |i| enregistres.borrow_mut().push(i));
        let ouverts = Rc::clone(&ouverts);
        f.on_open_attachment(move |i| ouverts.borrow_mut().push(i));
    }

    // La pastille ouvre : neuf fois sur dix on veut regarder le fichier, pas le garder.
    let pastille = par_libelle(&f, "Open plan.png").expect("la pastille doit être un bouton");
    // Le type et la taille sont annoncés avec le nom : « plan.png » seul ne dit pas
    // s'il faut l'ouvrir maintenant ou attendre d'être au bureau.
    assert_eq!(
        pastille.accessible_description().map(|d| d.to_string()),
        Some("Image, 180 ko".into())
    );
    pastille.invoke_accessible_default_action();
    assert_eq!(
        *ouverts.borrow(),
        [1],
        "le rang doit désigner le bon fichier"
    );

    // Et l'enregistrement garde son bouton, atteignable sans souris : il n'apparaît
    // qu'au survol, mais il est **toujours** dans l'arbre — le cacher par un `if` le
    // retirerait du clavier et des lecteurs d'écran en même temps que de l'écran.
    let garder = par_libelle(&f, "Save plan.png").expect("le bouton d'enregistrement existe");
    garder.invoke_accessible_default_action();
    assert_eq!(*enregistres.borrow(), [1]);
}

// --- Les panneaux ---

fn les_reglages_n_existent_pas_avant_d_etre_ouverts() {
    // Un panneau construit en permanence coûterait sa mise en page à chaque frame.
    let f = fenetre();
    assert!(par_libelle(&f, "Close settings").is_none());

    f.set_settings_open(true);
    assert!(par_libelle(&f, "Close settings").is_some());
}

fn fermer_les_reglages_les_ferme() {
    let f = fenetre();
    f.set_settings_open(true);

    par_libelle(&f, "Close settings")
        .unwrap()
        .invoke_accessible_default_action();
    assert!(!f.get_settings_open());
}

fn les_densites_sont_proposees_et_la_courante_est_marquee() {
    let f = fenetre();
    f.set_settings_open(true);
    f.set_density(0);

    assert_eq!(
        par_libelle(&f, "Compact")
            .unwrap()
            .accessible_item_selected(),
        Some(true)
    );
    assert_eq!(
        par_libelle(&f, "Normal")
            .unwrap()
            .accessible_item_selected(),
        Some(false)
    );
}

fn changer_de_densite_est_rapporte() {
    let f = fenetre();
    f.set_settings_open(true);

    let choisi = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let choisi = Rc::clone(&choisi);
        f.on_density_chosen(move |i| choisi.borrow_mut().push(i));
    }

    par_libelle(&f, "Comfortable")
        .unwrap()
        .invoke_accessible_default_action();
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

    par_libelle(&f, "Configure manually")
        .expect("la porte de secours doit être visible d'emblée")
        .invoke_accessible_default_action();
    assert_eq!(*demande.borrow(), 1);
}

fn la_porte_de_secours_disparait_une_fois_franchie() {
    let f = fenetre();
    f.set_add_account_open(true);
    f.set_add_account_manual(true);

    assert!(par_libelle(&f, "Configure manually").is_none());
    assert!(
        par_libelle(&f, "IMAP server").is_some(),
        "les champs prennent sa place"
    );
}

fn le_bouton_d_ajout_change_de_nom_selon_le_mode() {
    let f = fenetre();
    f.set_add_account_open(true);
    assert!(par_libelle(&f, "Add").is_some());

    f.set_add_account_manual(true);
    assert!(par_libelle(&f, "Save").is_some());
}

fn pendant_la_recherche_de_configuration_le_bouton_est_inactif() {
    // Sinon un double clic lancerait deux découvertes sur le même compte.
    let f = fenetre();
    f.set_add_account_open(true);
    f.set_add_account_busy(true);

    let bouton = par_libelle(&f, "Add").unwrap();
    assert_eq!(bouton.accessible_enabled(), Some(false));
}

fn les_champs_de_l_ecran_d_ajout_sont_nommes() {
    // Un champ de mot de passe sans nom est un champ qu'un lecteur d'écran annonce
    // « champ de saisie », ce qui n'aide personne.
    let f = fenetre();
    f.set_add_account_open(true);

    assert!(par_libelle(&f, "Email address").is_some());
    assert!(par_libelle(&f, "Password").is_some());
}

// --- La confirmation qui passe ---

fn aucune_confirmation_tant_qu_il_n_y_a_rien_a_confirmer() {
    // Une bulle vide reste une bulle : elle prendrait la place et le regard.
    let f = fenetre();
    assert!(
        par_libelle(&f, "contact@example.com added and working.").is_none(),
        "rien ne doit flotter sur une fenêtre au repos"
    );
}

fn une_confirmation_est_lisible_par_dessus_l_application() {
    let f = fenetre();
    f.set_toast("contact@example.com added and working.".into());

    assert!(
        textes(&f).contains(&"contact@example.com added and working.".to_string()),
        "la confirmation doit être affichée, pas seulement rangée dans une propriété"
    );
}

fn la_confirmation_survit_a_la_fermeture_de_l_ecran_d_ajout() {
    // C'est tout son intérêt : le panneau se ferme au moment où le compte est
    // accepté, et ce qui reste à l'écran doit dire pourquoi il s'est fermé.
    let f = fenetre();
    f.set_add_account_open(true);
    f.set_toast("contact@example.com added and working.".into());
    f.set_add_account_open(false);

    assert!(textes(&f).contains(&"contact@example.com added and working.".to_string()));
}

// --- The modules screen ---

fn rule(id: &str, name: &str, enabled: bool, applied: i32) -> RuleRowData {
    RuleRowData {
        id: id.into(),
        name: name.into(),
        enabled,
        summary: "When from news.example, mark as done".into(),
        applied,
    }
}

fn plugin(name: &str, permissions: &str, disabled: &str) -> PluginRowData {
    PluginRowData {
        id: "sorter".into(),
        name: name.into(),
        version: "1.0.0".into(),
        description: "Sorts newsletters".into(),
        permissions: permissions.into(),
        disabled_reason: disabled.into(),
    }
}

/// Every visible text on screen, for the checks that look for a sentence.
fn textes(f: &AppWindow) -> Vec<String> {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_type_name("Text")
        .find_all()
        .into_iter()
        .filter_map(|t| t.accessible_label().map(|l| l.to_string()))
        .collect()
}

fn les_modules_n_existent_pas_avant_d_etre_ouverts() {
    let f = fenetre();
    assert!(par_libelle(&f, "Add rule").is_none());

    f.set_modules_open(true);
    assert!(par_libelle(&f, "Add rule").is_some());
}

fn les_reglages_d_un_module_remplacent_la_liste_au_lieu_de_s_empiler() {
    // Ils s'ouvraient dans une seconde fenêtre modale, déclarée avant celle des
    // modules : le dernier frère passant devant, l'écran des modules recouvrait
    // entièrement ce qu'il venait d'ouvrir, et cliquer « Settings » n'avait l'air de
    // rien faire du tout.
    let f = fenetre();
    f.set_modules_open(true);
    f.set_plugins(modele(vec![plugin("Sorter", "Can read mail", "")]));

    assert!(par_libelle(&f, "Add rule").is_some(), "la liste est là");

    f.set_plugin_settings_name("Sorter".into());
    f.set_plugin_settings_open(true);

    assert!(
        par_libelle(&f, "Add rule").is_none(),
        "la liste cède la place au lieu de rester dessous"
    );
    assert!(
        par_libelle(&f, "‹ Modules").is_some(),
        "et l'on peut remonter d'un cran sans tout fermer"
    );

    f.set_plugin_settings_open(false);
    assert!(par_libelle(&f, "Add rule").is_some(), "la liste revient");
}

fn une_regle_dit_ce_qu_elle_fait() {
    // A rule you cannot read at a glance is a rule you switch off.
    let f = fenetre();
    f.set_modules_open(true);
    f.set_rules(modele(vec![rule("r1", "Newsletters", true, 12)]));

    let ligne = par_libelle(&f, "Newsletters").expect("the rule must be listed");
    assert_eq!(
        ligne.accessible_description().map(|d| d.to_string()),
        Some("When from news.example, mark as done".into())
    );
}

fn une_regle_se_desactive_sans_etre_supprimee() {
    let f = fenetre();
    f.set_modules_open(true);
    f.set_rules(modele(vec![rule("r1", "Newsletters", true, 0)]));

    let bascules = Rc::new(RefCell::new(Vec::<(String, bool)>::new()));
    {
        let bascules = Rc::clone(&bascules);
        f.on_rule_toggled(move |id, on| bascules.borrow_mut().push((id.to_string(), on)));
    }

    par_libelle(&f, "Enable Newsletters")
        .expect("the switch must be reachable")
        .invoke_accessible_default_action();

    assert_eq!(*bascules.borrow(), [("r1".to_string(), false)]);
}

fn une_regle_peut_etre_essayee_avant_d_etre_crue() {
    // Nobody dares write a rule that archives mail without knowing what it touches.
    let f = fenetre();
    f.set_modules_open(true);
    f.set_rules(modele(vec![rule("r1", "Newsletters", true, 0)]));

    let essais = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let essais = Rc::clone(&essais);
        f.on_rule_simulated(move |id| essais.borrow_mut().push(id.to_string()));
    }

    par_libelle(&f, "Try it")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*essais.borrow(), ["r1"]);
}

fn le_resultat_de_l_essai_est_affiche() {
    let f = fenetre();
    f.set_modules_open(true);
    f.set_simulation("42 messages out of 500 would be affected.".into());

    assert!(
        textes(&f).iter().any(|l| l.contains("42 messages")),
        "the dry run result must be visible"
    );
}

fn une_regle_se_supprime() {
    let f = fenetre();
    f.set_modules_open(true);
    f.set_rules(modele(vec![rule("r1", "Newsletters", true, 0)]));

    let supprimees = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let supprimees = Rc::clone(&supprimees);
        f.on_rule_removed(move |id| supprimees.borrow_mut().push(id.to_string()));
    }

    par_libelle(&f, "Delete")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*supprimees.borrow(), ["r1"]);
}

fn sans_regle_l_ecran_explique_au_lieu_de_rester_vide() {
    let f = fenetre();
    f.set_modules_open(true);
    f.set_rules(modele(Vec::<RuleRowData>::new()));

    assert!(textes(&f).iter().any(|l| l.contains("No rules yet")));
}

fn les_permissions_d_un_plugin_sont_visibles_sans_clic() {
    // A plugin's power is the thing worth knowing about it; burying it is how people
    // end up running code they never agreed to.
    let f = fenetre();
    f.set_modules_open(true);
    f.set_plugins(modele(vec![plugin(
        "Sorter",
        "Can read mail, change mail",
        "",
    )]));

    let ligne = par_libelle(&f, "Sorter").expect("the plugin must be listed");
    assert_eq!(
        ligne.accessible_description().map(|d| d.to_string()),
        Some("Can read mail, change mail".into())
    );
}

fn un_plugin_hors_circuit_dit_pourquoi() {
    let f = fenetre();
    f.set_modules_open(true);
    f.set_plugins(modele(vec![plugin(
        "Sorter",
        "Can read mail",
        "ran out of fuel",
    )]));

    let ligne = par_libelle(&f, "Sorter").unwrap();
    assert_eq!(
        ligne.accessible_description().map(|d| d.to_string()),
        Some("Out of circulation: ran out of fuel".into())
    );
}

fn sans_plugin_l_ecran_dit_ou_en_installer_un() {
    // A folder is a distribution channel that needs no store.
    let f = fenetre();
    f.set_modules_open(true);
    f.set_plugin_folder("/home/me/iris/plugins".into());

    assert!(textes(&f)
        .iter()
        .any(|l| l.contains("/home/me/iris/plugins")));
}

fn ajouter_une_regle_est_atteignable() {
    let f = fenetre();
    f.set_modules_open(true);

    let ajouts = Rc::new(RefCell::new(0));
    {
        let ajouts = Rc::clone(&ajouts);
        f.on_rule_added(move || *ajouts.borrow_mut() += 1);
    }

    par_libelle(&f, "Add rule")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*ajouts.borrow(), 1);
    assert!(par_libelle(&f, "From sender or domain").is_some());
}

// --- Appearance ---

fn les_trois_apparences_sont_proposees() {
    // As Windows is set, light, or dark: nothing else.
    let f = fenetre();
    f.set_settings_open(true);
    for nom in ["System", "Light", "Dark"] {
        assert!(par_libelle(&f, nom).is_some(), "« {nom} » manque");
    }
}

fn l_apparence_choisie_est_marquee() {
    let f = fenetre();
    f.set_settings_open(true);
    f.set_appearance(2);

    assert_eq!(
        par_libelle(&f, "Dark").unwrap().accessible_item_selected(),
        Some(true)
    );
    assert_eq!(
        par_libelle(&f, "System")
            .unwrap()
            .accessible_item_selected(),
        Some(false)
    );
}

fn choisir_une_apparence_est_rapporte() {
    let f = fenetre();
    f.set_settings_open(true);

    let choisies = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let choisies = Rc::clone(&choisies);
        f.on_appearance_chosen(move |i| choisies.borrow_mut().push(i));
    }

    par_libelle(&f, "Light")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*choisies.borrow(), [1]);
}

fn le_prenom_se_regle_dans_les_reglages() {
    let f = fenetre();
    f.set_settings_open(true);
    f.set_first_name("Camille".into());
    assert!(par_libelle(&f, "Your first name").is_some());
    assert_eq!(f.get_first_name(), "Camille");
}

// --- Writing a new message ---

fn ecrire_un_message_n_existe_pas_avant_d_etre_demande() {
    let f = fenetre();
    assert!(par_libelle(&f, "Send").is_none());

    f.set_compose_open(true);
    assert!(par_libelle(&f, "Send").is_some());
}

fn les_champs_d_un_nouveau_message_sont_nommes() {
    let f = fenetre();
    f.set_compose_open(true);

    assert!(par_libelle(&f, "To").is_some());
    assert!(par_libelle(&f, "Subject").is_some());
    assert!(par_libelle(&f, "Message").is_some());
}

fn envoyer_est_inactif_sans_destinataire() {
    // Nothing can be done with a message that has nowhere to go.
    let f = fenetre();
    f.set_compose_open(true);
    f.set_compose_to("".into());

    assert_eq!(
        par_libelle(&f, "Send").unwrap().accessible_enabled(),
        Some(false)
    );

    f.set_compose_to("marie@x.fr".into());
    assert_eq!(
        par_libelle(&f, "Send").unwrap().accessible_enabled(),
        Some(true)
    );
}

fn envoyer_declenche_l_envoi() {
    let f = fenetre();
    f.set_compose_open(true);
    f.set_compose_to("marie@x.fr".into());

    let envois = Rc::new(RefCell::new(0));
    {
        let envois = Rc::clone(&envois);
        f.on_compose_send(move || *envois.borrow_mut() += 1);
    }

    par_libelle(&f, "Send")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*envois.borrow(), 1);
}

fn pendant_le_delai_un_avis_propose_d_annuler() {
    // Send closes the window; the notice at the bottom holds the one way back.
    let f = fenetre();
    assert!(
        par_libelle(&f, "Undo send").is_none(),
        "no notice before a send"
    );

    f.set_send_notice_text("Sending “Devis”".into());
    f.set_send_notice_seconds(5);
    f.set_send_notice_open(true);
    let undo = par_libelle(&f, "Undo send").expect("the notice offers Undo");

    let annulations = Rc::new(RefCell::new(0));
    {
        let annulations = Rc::clone(&annulations);
        f.on_send_undone(move || *annulations.borrow_mut() += 1);
    }

    undo.invoke_accessible_default_action();
    assert_eq!(*annulations.borrow(), 1);
}

fn l_expediteur_se_choisit() {
    // With a hundred mailboxes, sending from the wrong one is the mistake that costs,
    // so the sender is a choice rather than whichever account happened to be first.
    let f = fenetre();
    f.set_compose_open(true);
    f.set_compose_senders(modele(vec![
        SharedString::from("me@work.example"),
        SharedString::from("me@home.example"),
    ]));

    assert!(par_libelle(&f, "From").is_some());
}

fn les_copies_restent_pliees_jusqu_a_ce_qu_on_les_demande() {
    // Most messages have neither, and two empty fields at the top of every draft is
    // furniture between the writer and the writing.
    let f = fenetre();
    f.set_compose_open(true);

    assert!(par_libelle(&f, "Cc").is_none());
    assert!(par_libelle(&f, "Add Cc and Bcc").is_some());

    f.set_compose_show_cc(true);
    assert!(par_libelle(&f, "Cc").is_some());
    assert!(par_libelle(&f, "Bcc").is_some());
}

fn une_piece_jointe_se_retire() {
    let f = fenetre();
    f.set_compose_open(true);
    f.set_compose_attachments(modele(vec![SharedString::from("plan.pdf")]));

    let retires = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let retires = Rc::clone(&retires);
        f.on_compose_remove_attachment(move |i| retires.borrow_mut().push(i));
    }

    par_libelle(&f, "Remove plan.pdf")
        .expect("an attachment must be removable")
        .invoke_accessible_default_action();
    assert_eq!(*retires.borrow(), [0]);
}

fn reduire_la_fenetre_ne_jette_pas_le_brouillon() {
    // Losing what somebody was writing is not a recoverable mistake.
    let f = fenetre();
    f.set_compose_open(true);
    f.set_compose_body("half a sentence".into());

    par_libelle(&f, "Minimise")
        .expect("the draft must be tuckable")
        .invoke_accessible_default_action();

    assert!(f.get_compose_open(), "the draft is still there");
    assert_eq!(f.get_compose_body(), "half a sentence");
}

fn une_erreur_de_composition_est_montree() {
    let f = fenetre();
    f.set_compose_open(true);
    f.set_compose_error("\"oops\" is not an email address".into());

    assert!(textes(&f)
        .iter()
        .any(|l| l.contains("not an email address")));
}

fn abandonner_un_message_est_possible() {
    let f = fenetre();
    f.set_compose_open(true);
    f.on_compose_dismissed({
        let fw = f.as_weak();
        move || fw.upgrade().unwrap().set_compose_open(false)
    });
    f.on_compose_discard({
        let fw = f.as_weak();
        move || fw.upgrade().unwrap().set_compose_open(false)
    });

    // Nothing written: closing is enough.
    par_libelle(&f, "Close message")
        .unwrap()
        .invoke_accessible_default_action();
    assert!(!f.get_compose_open());

    // Something written: the question offers to discard it.
    f.set_compose_open(true);
    f.set_compose_subject("Devis".into());
    par_libelle(&f, "Close message")
        .unwrap()
        .invoke_accessible_default_action();
    assert!(f.get_compose_confirm_close());
    par_libelle(&f, "Discard")
        .unwrap()
        .invoke_accessible_default_action();
    assert!(!f.get_compose_open());
}

// --- The window frame we draw ourselves ---

fn les_boutons_de_fenetre_sont_nommes() {
    // Replacing the system frame means replacing everything it gave a screen reader.
    let f = fenetre();
    assert!(par_libelle(&f, "Minimise").is_some());
    assert!(par_libelle(&f, "Close").is_some());
    assert!(par_libelle(&f, "Maximise").is_some());
}

fn le_bouton_d_agrandissement_dit_ce_qu_il_va_faire() {
    // "Restore" and "Maximise" are different actions; a button that does not say
    // which one it will do is a guess.
    let f = fenetre();
    f.set_window_maximised(true);
    assert!(par_libelle(&f, "Restore").is_some());
    assert!(par_libelle(&f, "Maximise").is_none());
}

fn fermer_la_fenetre_est_rapporte() {
    let f = fenetre();
    let fermetures = Rc::new(RefCell::new(0));
    {
        let fermetures = Rc::clone(&fermetures);
        f.on_window_close(move || *fermetures.borrow_mut() += 1);
    }

    par_libelle(&f, "Close")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*fermetures.borrow(), 1);
}

// --- The reading toolbar ---

fn la_barre_d_actions_est_absente_sans_conversation() {
    let f = fenetre();
    f.set_conversation_empty(true);
    assert!(par_libelle(&f, "Archive").is_none());
}

fn message(id: i32, de: &str, deplie: bool) -> MessageData {
    MessageData {
        id,
        from: de.into(),
        from_address: "x@y.fr".into(),
        initials: "HD".into(),
        tint: Default::default(),
        to: SharedString::new(),
        date: "12:30".into(),
        subject: "Devis".into(),
        preview: "Bonjour…".into(),
        expanded: deplie,
        blocks: modele(Vec::new()),
        attachments: modele(Vec::<iris_ui::AttachmentData>::new()),
        blocked_images: 0,
        has_tracker: false,
        body_is_image: false,
        body_tiles: Default::default(),
        body_loading: false,
    }
}

fn un_corps_qui_n_est_pas_encore_arrive_le_dit() {
    // C'était le seul écran que l'application n'a pas le droit de produire : un
    // panneau blanc qui ne distingue pas « ça charge » de « il n'y a rien ».
    let f = fenetre();
    f.set_conversation_empty(false);

    let mut attendu = message(1, "Huile Direct", true);
    attendu.body_loading = true;
    f.set_messages(modele(vec![attendu]));

    assert!(
        textes(&f).iter().any(|t| t.contains("Loading")),
        "l'attente doit être annoncée, en toutes lettres"
    );
}

fn un_corps_arrive_mais_vide_le_dit_aussi() {
    // L'autre moitié du même défaut : le corps est là et ne donne rien à lire.
    let f = fenetre();
    f.set_conversation_empty(false);
    f.set_messages(modele(vec![message(1, "Huile Direct", true)]));

    let vus = textes(&f);
    assert!(
        vus.iter().any(|t| t.contains("nothing to display")),
        "un corps vide doit se dire : {vus:?}"
    );
    assert!(
        !vus.iter().any(|t| t.contains("Loading")),
        "et ne doit pas prétendre attendre"
    );
}

fn un_fil_montre_tous_ses_messages() {
    // Il n'en montrait qu'un : le dernier. Un échange de douze en cachait onze
    // pendant que la liste affichait « 12 » à côté du sujet — le compte promettait
    // une conversation, la colonne livrait un message.
    let f = fenetre();
    f.set_conversation_empty(false);
    f.set_messages(modele(vec![
        message(1, "Marie", false),
        message(2, "Luc", false),
        message(3, "Marie", true),
    ]));

    for qui in ["Marie", "Luc"] {
        assert!(
            par_libelle(&f, &format!("Expand {qui}, 12:30")).is_some()
                || par_libelle(&f, &format!("Collapse {qui}, 12:30")).is_some(),
            "« {qui} » doit apparaître dans le fil"
        );
    }
}

fn un_message_replie_s_annonce_comme_tel() {
    let f = fenetre();
    f.set_conversation_empty(false);
    f.set_messages(modele(vec![
        message(1, "Marie", false),
        message(2, "Luc", true),
    ]));

    let replie = par_libelle(&f, "Expand Marie, 12:30").expect("l'en-tête replié");
    assert_eq!(replie.accessible_expanded(), Some(false));

    let deplie = par_libelle(&f, "Collapse Luc, 12:30").expect("l'en-tête déplié");
    assert_eq!(deplie.accessible_expanded(), Some(true));
}

fn deplier_un_message_est_rapporte() {
    let f = fenetre();
    f.set_conversation_empty(false);
    // Deux messages, parce que la poignée n'existe que là. Le test en posait un seul :
    // il vérifiait le pliage sur le seul fil où le pliage n'a pas de sens.
    f.set_messages(modele(vec![
        message(7, "Marie", false),
        message(8, "Luc", true),
    ]));

    let demandes = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let demandes = Rc::clone(&demandes);
        f.on_toggle_message(move |id| demandes.borrow_mut().push(id));
    }

    par_libelle(&f, "Expand Marie, 12:30")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*demandes.borrow(), [7]);
}

fn un_fil_d_un_seul_message_ne_repete_pas_son_en_tete() {
    // Le cas le plus fréquent de tous. L'en-tête y répétait mot pour mot le titre situé
    // trois lignes plus haut — même nom, même heure — et son chevron proposait de
    // replier le seul message du fil, c'est-à-dire de masquer tout ce qu'on venait
    // d'ouvrir.
    let f = fenetre();
    f.set_conversation_empty(false);
    f.set_messages(modele(vec![message(7, "Marie", true)]));

    assert!(
        par_libelle(&f, "Collapse Marie, 12:30").is_none(),
        "un message seul n'a pas de poignée de pliage"
    );

    // Et le corps est bien là : ce qui disparaît est l'en-tête, pas le message.
    f.set_messages(modele(vec![
        message(7, "Marie", true),
        message(8, "Luc", true),
    ]));
    assert!(
        par_libelle(&f, "Collapse Marie, 12:30").is_some(),
        "à deux, la poignée revient"
    );
}

fn le_compte_choisi_est_celui_qui_est_allume() {
    // Cliquer un compte filtrait bien la liste et « All accounts » restait allumé :
    // l'écran désignait une vue qui n'était pas celle affichée.
    let f = fenetre();
    f.set_other_accounts(modele(vec![compte(4, "moi@exemple.fr", 2, false)]));

    f.set_selected_account(0);
    assert_eq!(
        par_libelle(&f, "All accounts")
            .unwrap()
            .accessible_item_selected(),
        Some(true)
    );

    f.set_selected_account(4);
    assert_eq!(
        par_libelle(&f, "All accounts")
            .unwrap()
            .accessible_item_selected(),
        Some(false),
        "la vue unifiée s'éteint"
    );
    assert_eq!(
        par_libelle(&f, "moi@exemple.fr")
            .unwrap()
            .accessible_item_selected(),
        Some(true),
        "et le compte choisi s'allume"
    );
}

fn la_version_est_toujours_affichee() {
    // C'est la première question posée quand quelque chose ne va pas, et la seule
    // réponse qui rende un rapport exploitable.
    let f = fenetre();
    f.set_version("Iris 0.1.0".into());

    let textes = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_type_name("Text")
        .find_all();
    assert!(
        textes
            .iter()
            .filter_map(|t| t.accessible_label())
            .any(|l| l.contains("Iris 0.1.0")),
        "le numéro de version doit être lisible dans la barre du bas"
    );
}

fn chaque_action_de_lecture_est_atteignable() {
    // All of these existed already, reachable only by a key nobody had been told
    // about.
    let f = fenetre();
    f.set_conversation_empty(false);

    for label in [
        "Mark as done",
        "Archive",
        "Snooze…",
        "Move to Waiting",
        "Make it a task",
        "Star",
        "Delete",
        "Read full screen",
        "More actions",
    ] {
        assert!(par_libelle(&f, label).is_some(), "missing: {label}");
    }
}

fn archiver_est_rapporte() {
    let f = fenetre();
    f.set_conversation_empty(false);

    let archives = Rc::new(RefCell::new(0));
    {
        let archives = Rc::clone(&archives);
        f.on_thread_archive(move || *archives.borrow_mut() += 1);
    }

    par_libelle(&f, "Archive")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*archives.borrow(), 1);
}

fn le_bouton_de_lecture_dit_ce_qu_il_va_faire() {
    // One entry, two meanings, decided by what the message currently is. It lives
    // in the toolbar's More menu.
    let f = fenetre();
    f.set_conversation_empty(false);
    assert!(par_libelle(&f, "Mark as read").is_none(), "only once the menu opens");

    par_libelle(&f, "More actions")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(f.get_reader_menu(), "more");

    f.set_selected_unread(true);
    assert!(par_libelle(&f, "Mark as read").is_some());

    f.set_selected_unread(false);
    assert!(par_libelle(&f, "Mark as unread").is_some());
}

fn reporter_demande_jusqu_a_quand() {
    // Later today, tomorrow morning, the weekend, next week: the one asked for goes
    // to the conversation being read.
    let f = fenetre();
    f.set_conversation_empty(false);
    f.set_selected_thread(42);
    let reports = Rc::new(RefCell::new(Vec::<(i32, String)>::new()));
    {
        let r = Rc::clone(&reports);
        f.on_menu_snooze(move |id, quand| r.borrow_mut().push((id, quand.to_string())));
    }

    par_libelle(&f, "Snooze…")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(f.get_reader_menu(), "snooze");
    par_libelle(&f, "This weekend")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*reports.borrow(), [(42, "weekend".to_string())]);
    assert_eq!(f.get_reader_menu(), "", "picking closes the menu");
}

fn lire_en_plein_ecran_ecarte_les_colonnes() {
    let f = fenetre();
    f.set_conversation_empty(false);
    par_libelle(&f, "Read full screen")
        .unwrap()
        .invoke_accessible_default_action();
    assert!(f.get_reading_focus());
    par_libelle(&f, "Leave full screen")
        .unwrap()
        .invoke_accessible_default_action();
    assert!(!f.get_reading_focus());
}

fn en_attente_depuis_la_barre() {
    let f = fenetre();
    f.set_conversation_empty(false);
    let fois = Rc::new(RefCell::new(0));
    {
        let n = Rc::clone(&fois);
        f.on_thread_waiting(move || *n.borrow_mut() += 1);
    }
    par_libelle(&f, "Move to Waiting")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*fois.borrow(), 1);
}

// --- Les dossiers ---

fn les_indesirables_ne_sont_plus_un_onglet() {
    // Ils en avaient un, ce qui en faisait une quatrième étape du travail alors que
    // c'est un endroit où le courrier est rangé — comme la corbeille, qui n'a jamais
    // eu d'onglet. Ils vivent dans l'arborescence maintenant.
    let f = fenetre();
    let onglets = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Tab)
        .find_all();

    assert!(
        !onglets
            .iter()
            .any(|o| o.accessible_label().as_deref() == Some("Spam")),
        "aucun onglet ne doit s'appeler Spam"
    );
}

fn un_dossier_ouvert_remplace_les_onglets_par_son_nom() {
    // Laisser « À faire » allumé au-dessus du contenu de la corbeille désignerait une
    // liste qui n'est pas celle qu'on regarde.
    let f = fenetre();
    assert!(par_libelle(&f, "To do").is_some());

    f.set_folder_name("Trash".into());
    assert!(
        par_libelle(&f, "To do").is_none(),
        "les onglets cèdent la place"
    );
    assert!(par_libelle(&f, "Back to the queues").is_some());
}

fn quitter_un_dossier_est_rapporte() {
    let f = fenetre();
    f.set_folder_name("Trash".into());

    let sorties = Rc::new(RefCell::new(0));
    {
        let sorties = Rc::clone(&sorties);
        f.on_folder_cleared(move || *sorties.borrow_mut() += 1);
    }

    par_libelle(&f, "Back to the queues")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*sorties.borrow(), 1);
}

fn beaucoup_de_pieces_jointes_sont_comptees_pas_empilees() {
    // Douze pastilles sur trois rangs mangeraient la moitié du volet de lecture pour
    // un message qu'on est venu lire.
    let f = fenetre();
    f.set_conversation_empty(false);
    let lu = MessageData {
        attachments: modele(
            (0..12)
                .map(|i| piece(&format!("f{i}.pdf"), "PDF", "1 Mo"))
                .collect(),
        ),
        expanded: true,
        ..Default::default()
    };
    f.set_message(lu.clone());
    f.set_messages(modele(vec![lu]));

    assert!(
        par_libelle(&f, "Open f0.pdf").is_some(),
        "les premières sont là"
    );
    assert!(
        par_libelle(&f, "Open f11.pdf").is_none(),
        "les dernières sont comptées, pas dessinées"
    );

    let tout = par_libelle(&f, "Show all 12 attachments").expect("le décompte est un bouton");
    tout.invoke_accessible_default_action();
    // Row after row under the first: the next ones are there (the last ones are
    // further down the column, past the fold of this window).
    assert!(
        par_libelle(&f, "Save f4.pdf").is_some(),
        "et il les montre toutes"
    );
    assert!(par_libelle(&f, "Show all 12 attachments").is_none());
}

// --- Le menu contextuel d'un compte ---

fn le_menu_d_un_compte_n_existe_pas_avant_d_etre_ouvert() {
    let f = fenetre();
    assert!(par_libelle(&f, "Change password…").is_none());
}

fn le_menu_d_un_compte_offre_de_changer_le_mot_de_passe() {
    // C'était le trou : changer un mot de passe exigeait que la boîte tombe en panne
    // d'abord, pour que le marqueur d'alerte apparaisse. L'application demandait de
    // casser quelque chose avant d'accepter qu'on le répare.
    let f = fenetre();
    f.set_account_menu_open(true);
    f.set_account_menu_label("moi@exemple.fr".into());

    for attendu in [
        "Sync now",
        "Change password…",
        "Edit account…",
        "Disable",
        "Remove from Iris…",
    ] {
        assert!(
            par_libelle(&f, attendu).is_some(),
            "« {attendu} » manque au menu du compte"
        );
    }
}

fn choisir_dans_le_menu_d_un_compte_est_rapporte() {
    let f = fenetre();
    f.set_account_menu_open(true);

    let appels = Rc::new(RefCell::new(0));
    {
        let appels = Rc::clone(&appels);
        f.on_account_menu_password(move || *appels.borrow_mut() += 1);
    }

    par_libelle(&f, "Change password…")
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(*appels.borrow(), 1);
}

fn un_compte_epingle_propose_de_le_desepingler() {
    // Un menu qui propose « Épingler » sur un compte déjà épinglé ment sur l'état
    // qu'il décrit, et c'est le seul endroit où cet état est visible.
    let f = fenetre();
    f.set_account_menu_open(true);
    f.set_account_menu_pinned(true);
    assert!(par_libelle(&f, "Unpin").is_some());

    f.set_account_menu_pinned(false);
    assert!(par_libelle(&f, "Pin to the top").is_some());
}

// --- Les compteurs ---

fn un_grand_compteur_est_abrege() {
    // « 1250 » ne tient pas dans une pastille, et l'écart avec « 1240 » n'apprend
    // rien à personne. La valeur exacte reste à un survol.
    let f = fenetre();
    f.set_counts(modele(vec![1250, 0, 0]));
    f.set_count_labels(modele(vec!["1.2k".into()]));
    f.set_count_fulls(modele(vec!["1 250".into()]));

    let onglet = par_libelle(&f, "To do").expect("l'onglet À faire");
    assert_eq!(
        onglet.accessible_description().as_deref(),
        Some("1250 conversations"),
        "le lecteur d'écran garde le nombre exact"
    );
}

// --- Changelog et mises à jour ---

fn le_changelog_est_toujours_atteignable() {
    let f = fenetre();
    let bouton = par_libelle(&f, "What changed in each version").expect("le bouton Changelog");
    bouton.invoke_accessible_default_action();
    assert!(f.get_changelog_open());
}

fn le_bouton_de_mise_a_jour_n_existe_que_s_il_y_a_une_mise_a_jour() {
    // Un « Update now » permanent qui ne fait rien apprend à ne plus regarder.
    let f = fenetre();
    assert!(par_libelle(&f, "Iris 0.3.0 is available").is_none());

    f.set_update_version("0.3.0".into());
    let bouton = par_libelle(&f, "Iris 0.3.0 is available").expect("le bouton Update now");
    bouton.invoke_accessible_default_action();
    assert!(
        f.get_update_open(),
        "le clic ouvre la confirmation, il n'installe rien"
    );
}

fn la_confirmation_de_mise_a_jour_propose_d_installer_ou_d_attendre() {
    let f = fenetre();
    f.set_update_version("0.3.0".into());
    f.set_update_open(true);

    let confirme = Rc::new(RefCell::new(false));
    {
        let confirme = Rc::clone(&confirme);
        f.on_update_confirmed(move || *confirme.borrow_mut() = true);
    }
    // Par son rôle : « Update now » est aussi écrit dans la barre d'état, sur un
    // bouton dont le nom est l'infobulle.
    testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().as_deref() == Some("Update now"))
        .expect("le bouton de confirmation")
        .invoke_accessible_default_action();
    assert!(*confirme.borrow());

    par_libelle(&f, "Later")
        .expect("le bouton pour attendre")
        .invoke_accessible_default_action();
    assert!(!f.get_update_open());
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

    let anonymes: Vec<_> = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .filter(|b| {
            b.accessible_label()
                .map(|l| l.trim().is_empty())
                .unwrap_or(true)
        })
        .collect();

    assert!(
        anonymes.is_empty(),
        "{} bouton(s) sans libellé",
        anonymes.len()
    );
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
        (
            "une_conversation_s_annonce_par_qui_ecrit_de_quoi_et_quand",
            une_conversation_s_annonce_par_qui_ecrit_de_quoi_et_quand as fn(),
        ),
        (
            "une_conversation_lue_le_dit",
            une_conversation_lue_le_dit as fn(),
        ),
        (
            "la_case_a_cocher_occupe_toujours_sa_colonne",
            la_case_a_cocher_occupe_toujours_sa_colonne as fn(),
        ),
        (
            "actionner_une_ligne_ouvre_la_bonne_conversation",
            actionner_une_ligne_ouvre_la_bonne_conversation as fn(),
        ),
        (
            "la_conversation_selectionnee_est_annoncee_comme_telle",
            la_conversation_selectionnee_est_annoncee_comme_telle as fn(),
        ),
        (
            "une_liste_vide_ne_ment_pas_pendant_le_chargement",
            une_liste_vide_ne_ment_pas_pendant_le_chargement as fn(),
        ),
        (
            "les_trois_files_sont_annoncees_avec_leur_compte",
            les_trois_files_sont_annoncees_avec_leur_compte as fn(),
        ),
        (
            "actionner_un_onglet_change_de_file",
            actionner_un_onglet_change_de_file as fn(),
        ),
        (
            "l_onglet_actif_est_annonce_comme_selectionne",
            l_onglet_actif_est_annonce_comme_selectionne as fn(),
        ),
        (
            "les_onglets_disparaissent_pendant_une_recherche",
            les_onglets_disparaissent_pendant_une_recherche as fn(),
        ),
        (
            "quitter_la_recherche_est_atteignable",
            quitter_la_recherche_est_atteignable as fn(),
        ),
        (
            "le_bouton_de_sortie_n_existe_pas_hors_recherche",
            le_bouton_de_sortie_n_existe_pas_hors_recherche as fn(),
        ),
        (
            "la_barre_de_recherche_est_nommee",
            la_barre_de_recherche_est_nommee as fn(),
        ),
        (
            "un_compte_en_panne_le_dit_a_voix_haute",
            un_compte_en_panne_le_dit_a_voix_haute as fn(),
        ),
        (
            "un_compte_sain_annonce_ce_qu_il_reste_a_traiter",
            un_compte_sain_annonce_ce_qu_il_reste_a_traiter as fn(),
        ),
        (
            "la_reprise_d_un_compte_en_panne_est_un_bouton",
            la_reprise_d_un_compte_en_panne_est_un_bouton as fn(),
        ),
        (
            "un_compte_sain_n_offre_pas_de_reprise",
            un_compte_sain_n_offre_pas_de_reprise as fn(),
        ),
        (
            "ajouter_un_compte_est_atteignable_depuis_la_barre_laterale",
            ajouter_un_compte_est_atteignable_depuis_la_barre_laterale as fn(),
        ),
        (
            "une_piece_jointe_s_enregistre_par_son_nom",
            une_piece_jointe_s_enregistre_par_son_nom as fn(),
        ),
        (
            "les_reglages_n_existent_pas_avant_d_etre_ouverts",
            les_reglages_n_existent_pas_avant_d_etre_ouverts as fn(),
        ),
        (
            "fermer_les_reglages_les_ferme",
            fermer_les_reglages_les_ferme as fn(),
        ),
        (
            "les_densites_sont_proposees_et_la_courante_est_marquee",
            les_densites_sont_proposees_et_la_courante_est_marquee as fn(),
        ),
        (
            "changer_de_densite_est_rapporte",
            changer_de_densite_est_rapporte as fn(),
        ),
        (
            "l_ecran_d_ajout_propose_la_porte_de_secours_avant_l_echec",
            l_ecran_d_ajout_propose_la_porte_de_secours_avant_l_echec as fn(),
        ),
        (
            "la_porte_de_secours_disparait_une_fois_franchie",
            la_porte_de_secours_disparait_une_fois_franchie as fn(),
        ),
        (
            "le_bouton_d_ajout_change_de_nom_selon_le_mode",
            le_bouton_d_ajout_change_de_nom_selon_le_mode as fn(),
        ),
        (
            "pendant_la_recherche_de_configuration_le_bouton_est_inactif",
            pendant_la_recherche_de_configuration_le_bouton_est_inactif as fn(),
        ),
        (
            "les_champs_de_l_ecran_d_ajout_sont_nommes",
            les_champs_de_l_ecran_d_ajout_sont_nommes as fn(),
        ),
        (
            "les_boutons_de_fenetre_sont_nommes",
            les_boutons_de_fenetre_sont_nommes as fn(),
        ),
        (
            "le_bouton_d_agrandissement_dit_ce_qu_il_va_faire",
            le_bouton_d_agrandissement_dit_ce_qu_il_va_faire as fn(),
        ),
        (
            "fermer_la_fenetre_est_rapporte",
            fermer_la_fenetre_est_rapporte as fn(),
        ),
        (
            "la_barre_d_actions_est_absente_sans_conversation",
            la_barre_d_actions_est_absente_sans_conversation as fn(),
        ),
        (
            "chaque_action_de_lecture_est_atteignable",
            chaque_action_de_lecture_est_atteignable as fn(),
        ),
        ("archiver_est_rapporte", archiver_est_rapporte as fn()),
        (
            "le_bouton_de_lecture_dit_ce_qu_il_va_faire",
            le_bouton_de_lecture_dit_ce_qu_il_va_faire as fn(),
        ),
        (
            "reporter_demande_jusqu_a_quand",
            reporter_demande_jusqu_a_quand as fn(),
        ),
        (
            "lire_en_plein_ecran_ecarte_les_colonnes",
            lire_en_plein_ecran_ecarte_les_colonnes as fn(),
        ),
        ("en_attente_depuis_la_barre", en_attente_depuis_la_barre as fn()),
        (
            "les_indesirables_ne_sont_plus_un_onglet",
            les_indesirables_ne_sont_plus_un_onglet as fn(),
        ),
        (
            "un_dossier_ouvert_remplace_les_onglets_par_son_nom",
            un_dossier_ouvert_remplace_les_onglets_par_son_nom as fn(),
        ),
        (
            "quitter_un_dossier_est_rapporte",
            quitter_un_dossier_est_rapporte as fn(),
        ),
        (
            "les_trois_apparences_sont_proposees",
            les_trois_apparences_sont_proposees as fn(),
        ),
        (
            "l_apparence_choisie_est_marquee",
            l_apparence_choisie_est_marquee as fn(),
        ),
        (
            "choisir_une_apparence_est_rapporte",
            choisir_une_apparence_est_rapporte as fn(),
        ),
        (
            "le_prenom_se_regle_dans_les_reglages",
            le_prenom_se_regle_dans_les_reglages as fn(),
        ),
        (
            "ecrire_un_message_n_existe_pas_avant_d_etre_demande",
            ecrire_un_message_n_existe_pas_avant_d_etre_demande as fn(),
        ),
        (
            "les_champs_d_un_nouveau_message_sont_nommes",
            les_champs_d_un_nouveau_message_sont_nommes as fn(),
        ),
        (
            "envoyer_est_inactif_sans_destinataire",
            envoyer_est_inactif_sans_destinataire as fn(),
        ),
        (
            "envoyer_declenche_l_envoi",
            envoyer_declenche_l_envoi as fn(),
        ),
        (
            "pendant_le_delai_un_avis_propose_d_annuler",
            pendant_le_delai_un_avis_propose_d_annuler as fn(),
        ),
        ("l_expediteur_se_choisit", l_expediteur_se_choisit as fn()),
        (
            "les_copies_restent_pliees_jusqu_a_ce_qu_on_les_demande",
            les_copies_restent_pliees_jusqu_a_ce_qu_on_les_demande as fn(),
        ),
        (
            "une_piece_jointe_se_retire",
            une_piece_jointe_se_retire as fn(),
        ),
        (
            "reduire_la_fenetre_ne_jette_pas_le_brouillon",
            reduire_la_fenetre_ne_jette_pas_le_brouillon as fn(),
        ),
        (
            "une_erreur_de_composition_est_montree",
            une_erreur_de_composition_est_montree as fn(),
        ),
        (
            "abandonner_un_message_est_possible",
            abandonner_un_message_est_possible as fn(),
        ),
        (
            "les_modules_n_existent_pas_avant_d_etre_ouverts",
            les_modules_n_existent_pas_avant_d_etre_ouverts as fn(),
        ),
        (
            "les_reglages_d_un_module_remplacent_la_liste_au_lieu_de_s_empiler",
            les_reglages_d_un_module_remplacent_la_liste_au_lieu_de_s_empiler as fn(),
        ),
        (
            "une_regle_dit_ce_qu_elle_fait",
            une_regle_dit_ce_qu_elle_fait as fn(),
        ),
        (
            "une_regle_se_desactive_sans_etre_supprimee",
            une_regle_se_desactive_sans_etre_supprimee as fn(),
        ),
        (
            "une_regle_peut_etre_essayee_avant_d_etre_crue",
            une_regle_peut_etre_essayee_avant_d_etre_crue as fn(),
        ),
        (
            "le_resultat_de_l_essai_est_affiche",
            le_resultat_de_l_essai_est_affiche as fn(),
        ),
        ("une_regle_se_supprime", une_regle_se_supprime as fn()),
        (
            "sans_regle_l_ecran_explique_au_lieu_de_rester_vide",
            sans_regle_l_ecran_explique_au_lieu_de_rester_vide as fn(),
        ),
        (
            "les_permissions_d_un_plugin_sont_visibles_sans_clic",
            les_permissions_d_un_plugin_sont_visibles_sans_clic as fn(),
        ),
        (
            "un_plugin_hors_circuit_dit_pourquoi",
            un_plugin_hors_circuit_dit_pourquoi as fn(),
        ),
        (
            "sans_plugin_l_ecran_dit_ou_en_installer_un",
            sans_plugin_l_ecran_dit_ou_en_installer_un as fn(),
        ),
        (
            "ajouter_une_regle_est_atteignable",
            ajouter_une_regle_est_atteignable as fn(),
        ),
        (
            "le_menu_d_un_compte_n_existe_pas_avant_d_etre_ouvert",
            le_menu_d_un_compte_n_existe_pas_avant_d_etre_ouvert as fn(),
        ),
        (
            "le_menu_d_un_compte_offre_de_changer_le_mot_de_passe",
            le_menu_d_un_compte_offre_de_changer_le_mot_de_passe as fn(),
        ),
        (
            "choisir_dans_le_menu_d_un_compte_est_rapporte",
            choisir_dans_le_menu_d_un_compte_est_rapporte as fn(),
        ),
        (
            "un_compte_epingle_propose_de_le_desepingler",
            un_compte_epingle_propose_de_le_desepingler as fn(),
        ),
        (
            "un_grand_compteur_est_abrege",
            un_grand_compteur_est_abrege as fn(),
        ),
        (
            "beaucoup_de_pieces_jointes_sont_comptees_pas_empilees",
            beaucoup_de_pieces_jointes_sont_comptees_pas_empilees as fn(),
        ),
        (
            "un_corps_qui_n_est_pas_encore_arrive_le_dit",
            un_corps_qui_n_est_pas_encore_arrive_le_dit as fn(),
        ),
        (
            "un_corps_arrive_mais_vide_le_dit_aussi",
            un_corps_arrive_mais_vide_le_dit_aussi as fn(),
        ),
        (
            "un_fil_montre_tous_ses_messages",
            un_fil_montre_tous_ses_messages as fn(),
        ),
        (
            "un_message_replie_s_annonce_comme_tel",
            un_message_replie_s_annonce_comme_tel as fn(),
        ),
        (
            "deplier_un_message_est_rapporte",
            deplier_un_message_est_rapporte as fn(),
        ),
        (
            "un_fil_d_un_seul_message_ne_repete_pas_son_en_tete",
            un_fil_d_un_seul_message_ne_repete_pas_son_en_tete as fn(),
        ),
        (
            "le_compte_choisi_est_celui_qui_est_allume",
            le_compte_choisi_est_celui_qui_est_allume as fn(),
        ),
        (
            "aucune_confirmation_tant_qu_il_n_y_a_rien_a_confirmer",
            aucune_confirmation_tant_qu_il_n_y_a_rien_a_confirmer as fn(),
        ),
        (
            "une_confirmation_est_lisible_par_dessus_l_application",
            une_confirmation_est_lisible_par_dessus_l_application as fn(),
        ),
        (
            "la_confirmation_survit_a_la_fermeture_de_l_ecran_d_ajout",
            la_confirmation_survit_a_la_fermeture_de_l_ecran_d_ajout as fn(),
        ),
        (
            "la_version_est_toujours_affichee",
            la_version_est_toujours_affichee as fn(),
        ),
        (
            "aucun_bouton_ne_reste_sans_nom",
            aucun_bouton_ne_reste_sans_nom as fn(),
        ),
        (
            "la_fenetre_se_construit_sans_donnees",
            la_fenetre_se_construit_sans_donnees as fn(),
        ),
        (
            "un_compte_dont_la_synchro_a_echoue_dit_pourquoi",
            un_compte_dont_la_synchro_a_echoue_dit_pourquoi as fn(),
        ),
        (
            "le_changelog_est_toujours_atteignable",
            le_changelog_est_toujours_atteignable as fn(),
        ),
        (
            "le_bouton_de_mise_a_jour_n_existe_que_s_il_y_a_une_mise_a_jour",
            le_bouton_de_mise_a_jour_n_existe_que_s_il_y_a_une_mise_a_jour as fn(),
        ),
        (
            "la_confirmation_de_mise_a_jour_propose_d_installer_ou_d_attendre",
            la_confirmation_de_mise_a_jour_propose_d_installer_ou_d_attendre as fn(),
        ),
    ];

    let total = scenarios.len();
    let mut echecs = Vec::new();

    for (nom, scenario) in scenarios {
        // Chaque scénario est isolé : un échec ne doit pas masquer les suivants.
        let resultat = std::panic::catch_unwind(std::panic::AssertUnwindSafe(scenario));
        match resultat {
            Ok(()) => println!("ok    {nom}"),
            Err(_) => {
                println!("FAIL  {nom}");
                echecs.push(nom);
            }
        }
    }

    println!();
    if echecs.is_empty() {
        println!("interface: {total} scenarios, no failures");
    } else {
        println!("interface: {} failure(s) out of {total}", echecs.len());
        for nom in &echecs {
            println!("  - {nom}");
        }
        std::process::exit(1);
    }
}
