//! Typing and clicking in a panel open over the rest of the window.
//!
//! The accessibility tests next door invoke actions directly; these go through the
//! window the way a person does — a pointer pressed and released at a position, keys
//! pressed one at a time — because the faults they guard against live exactly there:
//! a field that does not take the click meant for it, a key that lands on the mail
//! behind a dialog.

use i_slint_backend_testing as testing;
use iris_ui::AppWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

fn fenetre() -> AppWindow {
    let f = AppWindow::new().expect("la fenêtre doit se construire");
    f.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    f.show().expect("la fenêtre doit s'afficher");
    f
}

/// Un élément par son rôle, pour distinguer un bouton d'un texte qui porte le même nom.
fn bouton(f: &AppWindow, libelle: &str) -> testing::ElementHandle {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().as_deref() == Some(libelle))
        .unwrap_or_else(|| panic!("aucun bouton nommé « {libelle} »"))
}

/// Un champ de saisie par son nom : l'étiquette posée à côté porte le même texte.
fn champ(f: &AppWindow, libelle: &str) -> testing::ElementHandle {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::TextInput)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().as_deref() == Some(libelle))
        .unwrap_or_else(|| panic!("aucun champ nommé « {libelle} »"))
}

fn clic(e: &testing::ElementHandle) {
    e.mock_single_click(PointerEventButton::Left);
}

fn taper(f: &AppWindow, texte: &str) {
    for c in texte.chars() {
        let text = SharedString::from(c.to_string());
        f.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        f.window().dispatch_event(WindowEvent::KeyReleased { text });
    }
}

fn composer(f: &AppWindow) {
    f.set_compose_open(true);
    f.set_compose_minimised(false);
}

// --- Les raccourcis ne traversent pas une fenêtre ouverte ---

fn une_touche_ne_touche_pas_le_courrier_derriere_la_fenetre() {
    // La fenêtre de rédaction ouverte et aucun champ cliqué : « e » marquait comme
    // traité le message affiché derrière elle.
    let f = fenetre();
    let touches = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let touches = Rc::clone(&touches);
        f.on_key_pressed(move |t| touches.borrow_mut().push(t.to_string()));
    }
    let traites = Rc::new(RefCell::new(0));
    {
        let traites = Rc::clone(&traites);
        f.on_thread_done(move || *traites.borrow_mut() += 1);
    }
    composer(&f);

    taper(&f, "e");
    f.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Delete.into(),
    });

    assert!(
        touches.borrow().is_empty(),
        "raccourcis transmis sous la fenêtre : {:?}",
        touches.borrow()
    );
    assert_eq!(*traites.borrow(), 0);
}

fn la_fenetre_de_redaction_s_ouvre_dans_le_champ_a() {
    // Ouvrir pour écrire, c'est vouloir écrire : la première touche va au
    // destinataire, pas à la liste derrière.
    let f = fenetre();
    composer(&f);
    taper(&f, "marie");
    assert_eq!(f.get_compose_to().as_str(), "marie");
}

// --- Taper ---

fn l_arobase_s_ecrit_dans_le_champ_a() {
    let f = fenetre();
    composer(&f);
    clic(&champ(&f, "To"));
    taper(&f, "marie@example.com");
    assert_eq!(f.get_compose_to().as_str(), "marie@example.com");
}

// --- Cliquer ---

fn cliquer_une_suggestion_la_choisit() {
    let f = fenetre();
    let choix = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let choix = Rc::clone(&choix);
        f.on_compose_recipient_picked(move |champ, c| {
            choix.borrow_mut().push(format!("{champ}:{c}"))
        });
    }
    composer(&f);
    clic(&champ(&f, "To"));
    taper(&f, "ma");
    f.set_compose_suggestions(ModelRc::new(VecModel::from(vec![SharedString::from(
        "Marie <marie@example.com>",
    )])));

    clic(&bouton(&f, "Marie <marie@example.com>"));
    assert_eq!(*choix.borrow(), ["to:Marie <marie@example.com>"]);
}

fn un_clic_sur_un_autre_champ_y_entre_du_premier_coup() {
    // Le premier clic ne servait qu'à quitter le champ en cours ; il en fallait un
    // second pour entrer dans celui qu'on visait.
    let f = fenetre();
    composer(&f);
    clic(&champ(&f, "To"));
    taper(&f, "ma");
    f.set_compose_suggestions(ModelRc::new(VecModel::from(vec![SharedString::from(
        "Marie <marie@example.com>",
    )])));

    clic(&champ(&f, "Subject"));
    taper(&f, "Devis");
    assert_eq!(f.get_compose_subject().as_str(), "Devis");
    assert_eq!(f.get_compose_to().as_str(), "ma");
}

fn un_clic_d_un_champ_a_l_autre_sans_suggestions() {
    let f = fenetre();
    composer(&f);
    clic(&champ(&f, "Subject"));
    taper(&f, "a");
    clic(&champ(&f, "To"));
    taper(&f, "b");
    assert_eq!(f.get_compose_subject().as_str(), "a");
    assert_eq!(f.get_compose_to().as_str(), "b");
}

// --- Les corps peints par tuiles ---

fn corps_en_tuiles(f: &AppWindow, tuiles: usize) {
    use iris_ui::{BodyTileData, MessageData};
    let tuiles: Vec<BodyTileData> = (0..tuiles)
        .map(|_| BodyTileData {
            image: slint::Image::default(),
            ready: false,
            aspect: 1.0,
        })
        .collect();
    let message = MessageData {
        id: 7,
        from: "Boutique".into(),
        subject: "Infolettre".into(),
        expanded: true,
        body_is_image: true,
        body_tiles: ModelRc::new(VecModel::from(tuiles)),
        ..Default::default()
    };
    f.set_conversation_empty(false);
    f.set_message(message.clone());
    f.set_messages(ModelRc::new(VecModel::from(vec![message])));
    // Un tour de la boucle d'interface : la mise en page se fait, et les rappels
    // « changed » partent, comme ils partent dans l'application après chaque image.
    let position = slint::LogicalPosition::new(1000.0, 400.0);
    f.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    slint::platform::update_timers_and_animations();
}

fn journal_des_tuiles(f: &AppWindow) -> Rc<RefCell<Vec<(i32, i32, bool)>>> {
    let journal = Rc::new(RefCell::new(Vec::new()));
    {
        let j = Rc::clone(&journal);
        f.on_body_tile_wanted(move |m, t| j.borrow_mut().push((m, t, true)));
    }
    {
        let j = Rc::clone(&journal);
        f.on_body_tile_released(move |m, t| j.borrow_mut().push((m, t, false)));
    }
    journal
}

fn seules_les_tuiles_proches_de_l_ecran_sont_demandees() {
    // Quarante tuiles, chacune aussi haute que large : un message de vingt mille
    // pixels. Seules celles d'un écran autour de ce qu'on voit doivent être peintes.
    let f = fenetre();
    let journal = journal_des_tuiles(&f);
    corps_en_tuiles(&f, 40);

    let demandees: Vec<i32> = journal
        .borrow()
        .iter()
        .filter(|e| e.2)
        .map(|e| e.1)
        .collect();
    assert!(
        demandees.contains(&0),
        "la première est demandée : {demandees:?}"
    );
    assert!(
        demandees.len() < 8,
        "{} tuiles demandées sur 40 : {demandees:?}",
        demandees.len()
    );
    assert!(!demandees.contains(&39));
}

fn defiler_demande_les_suivantes_et_rend_les_precedentes() {
    let f = fenetre();
    let journal = journal_des_tuiles(&f);
    corps_en_tuiles(&f, 40);
    journal.borrow_mut().clear();

    // La molette au-dessus de la colonne de lecture, plusieurs crans.
    let position = slint::LogicalPosition::new(1000.0, 400.0);
    f.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    for _ in 0..40 {
        f.window().dispatch_event(WindowEvent::PointerScrolled {
            position,
            delta_x: 0.0,
            delta_y: -300.0,
        });
    }

    let j = journal.borrow();
    assert!(
        j.iter().any(|e| e.2 && e.1 > 5),
        "des tuiles plus bas sont demandées : {j:?}"
    );
    assert!(
        j.iter().any(|e| !e.2 && e.1 == 0),
        "la première est rendue : {j:?}"
    );
}

// --- L'agenda ---

fn ctrl(f: &AppWindow, touche: &str) {
    let ctrl = SharedString::from(slint::platform::Key::Control);
    f.window()
        .dispatch_event(WindowEvent::KeyPressed { text: ctrl.clone() });
    taper(f, touche);
    f.window()
        .dispatch_event(WindowEvent::KeyReleased { text: ctrl });
}

fn ctrl_2_ouvre_l_agenda_et_ses_touches_ne_touchent_pas_le_courrier() {
    let f = fenetre();
    let touches = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let touches = Rc::clone(&touches);
        f.on_key_pressed(move |t| touches.borrow_mut().push(t.to_string()));
    }
    let aujourd_hui = Rc::new(RefCell::new(0));
    {
        let a = Rc::clone(&aujourd_hui);
        f.on_calendar_today(move || *a.borrow_mut() += 1);
    }

    ctrl(&f, "2");
    assert_eq!(f.get_workspace(), 1, "Ctrl+2 ouvre l'agenda");
    // La touche Ctrl elle-même est passée avant la bascule ; on ne compte que la suite.
    touches.borrow_mut().clear();

    taper(&f, "et");
    assert!(
        touches.borrow().is_empty(),
        "aucune touche n'atteint le courrier : {:?}",
        touches.borrow()
    );
    assert_eq!(*aujourd_hui.borrow(), 1, "« t » ramène à aujourd'hui");

    ctrl(&f, "1");
    assert_eq!(f.get_workspace(), 0);
}

fn l_editeur_d_evenement_s_ouvre_dans_le_titre() {
    let f = fenetre();
    f.set_workspace(1);
    f.set_event_editor_open(true);
    taper(&f, "Dentiste");
    assert_eq!(f.get_editor_title().as_str(), "Dentiste");
}

fn l_abonnement_s_ouvre_dans_le_lien() {
    let f = fenetre();
    f.set_workspace(1);
    f.set_subscribe_open(true);
    taper(&f, "webcal://example.com/a.ics");
    assert_eq!(f.get_subscribe_url().as_str(), "webcal://example.com/a.ics");
}

fn un_calendrier(f: &AppWindow, abonne: bool) {
    f.set_workspace(1);
    f.set_calendars(ModelRc::new(VecModel::from(vec![iris_ui::CalendarData {
        id: 7,
        name: "Club".into(),
        color: slint::Color::from_rgb_u8(0x4f, 0x8c, 0xff),
        visible: true,
        subscribed: abonne,
        ..Default::default()
    }])));
}

fn ligne_de_calendrier(f: &AppWindow, nom: &str) -> testing::ElementHandle {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Checkbox)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().as_deref() == Some(nom))
        .unwrap_or_else(|| panic!("aucun calendrier nommé « {nom} »"))
}

fn echap(f: &AppWindow) {
    let t = SharedString::from(slint::platform::Key::Escape);
    f.window()
        .dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
    f.window()
        .dispatch_event(WindowEvent::KeyReleased { text: t });
}

fn un_clic_droit_sur_un_calendrier_ouvre_son_menu_qui_garde_le_clavier() {
    let f = fenetre();
    un_calendrier(&f, true);
    let aujourd_hui = Rc::new(RefCell::new(0));
    {
        let a = Rc::clone(&aujourd_hui);
        f.on_calendar_today(move || *a.borrow_mut() += 1);
    }

    ligne_de_calendrier(&f, "Club").mock_single_click(PointerEventButton::Right);
    assert!(f.get_calendar_menu_open(), "le clic droit ouvre le menu");
    assert_eq!(f.get_calendar_menu_cal().name.as_str(), "Club");
    assert!(
        f.get_calendars().row_data(0).is_some_and(|c| c.visible),
        "le clic droit ne masque pas le calendrier"
    );

    taper(&f, "t");
    assert_eq!(*aujourd_hui.borrow(), 0, "le menu ouvert garde le clavier");
    echap(&f);
    assert!(!f.get_calendar_menu_open(), "Échap ferme le menu");
}

fn supprimer_un_calendrier_demande_confirmation() {
    let f = fenetre();
    un_calendrier(&f, false);
    let supprimes = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let s = Rc::clone(&supprimes);
        f.on_calendar_delete_confirmed(move |id| s.borrow_mut().push(id));
    }

    ligne_de_calendrier(&f, "Club").mock_single_click(PointerEventButton::Right);
    clic(&bouton(&f, "Delete calendar…"));
    assert!(
        f.get_calendar_delete_open(),
        "la suppression passe par une confirmation"
    );
    assert!(
        supprimes.borrow().is_empty(),
        "rien n'est supprimé avant la réponse"
    );

    clic(&bouton(&f, "Delete"));
    assert_eq!(*supprimes.borrow(), [7]);
    assert!(!f.get_calendar_delete_open());
}

fn renommer_un_calendrier_s_ouvre_dans_le_nom() {
    let f = fenetre();
    un_calendrier(&f, false);
    let renommes = Rc::new(RefCell::new(Vec::<(i32, String)>::new()));
    {
        let r = Rc::clone(&renommes);
        f.on_calendar_rename_confirmed(move |id, nom| r.borrow_mut().push((id, nom.to_string())));
    }

    ligne_de_calendrier(&f, "Club").mock_single_click(PointerEventButton::Right);
    clic(&bouton(&f, "Rename…"));
    assert!(f.get_calendar_rename_open());
    assert_eq!(
        f.get_calendar_rename_name().as_str(),
        "Club",
        "le nom actuel est proposé"
    );

    taper(&f, "Club de voile\n");
    assert_eq!(
        *renommes.borrow(),
        [(7, "Club de voile".to_string())],
        "le nom proposé est sélectionné : la frappe le remplace"
    );
}

fn la_date_d_un_evenement_se_choisit_dans_un_petit_mois() {
    let f = fenetre();
    f.set_workspace(1);
    f.set_event_editor_open(true);
    f.set_editor_start_label("Mon 28 Sep 2026".into());
    let demandes = Rc::new(RefCell::new(Vec::<i32>::new()));
    let choisis = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let d = Rc::clone(&demandes);
        let fw = f.as_weak();
        f.on_editor_picker_requested(move |w| {
            d.borrow_mut().push(w);
            // What the application does: open it, with a month of days.
            let f = fw.upgrade().unwrap();
            f.set_editor_picker_cells(ModelRc::new(VecModel::from(
                (0..42)
                    .map(|i| iris_ui::MonthCellData {
                        day: ((i % 30) + 1).to_string().into(),
                        date: format!("2026-10-{:02}", (i % 30) + 1).into(),
                        in_month: true,
                        ..Default::default()
                    })
                    .collect::<Vec<_>>(),
            )));
            f.set_editor_picker(w);
        });
        let c = Rc::clone(&choisis);
        f.on_editor_picker_chosen(move |d| c.borrow_mut().push(d.to_string()));
    }

    clic(&bouton(&f, "Start date"));
    assert_eq!(
        *demandes.borrow(),
        [0],
        "cliquer la date demande le petit mois"
    );
    assert_eq!(f.get_editor_picker(), 0);

    clic(&bouton(&f, "2026-10-05"));
    assert_eq!(
        *choisis.borrow(),
        ["2026-10-05"],
        "un clic sur le jour le choisit"
    );

    // Open, it covers nothing: the title takes the first click and the keys.
    clic(&champ(&f, "Title"));
    taper(&f, "Dentiste");
    assert_eq!(f.get_editor_title().as_str(), "Dentiste");

    echap(&f);
    assert_eq!(
        f.get_editor_picker(),
        -1,
        "Échap replie le petit mois d'abord"
    );
    assert!(f.get_event_editor_open(), "et laisse l'événement ouvert");
}

// --- Les tâches ---

fn des_lieux(f: &AppWindow) {
    let mut lieux: Vec<iris_ui::TaskPlaceData> = ["today", "upcoming", "anytime", "mail"]
        .iter()
        .map(|k| iris_ui::TaskPlaceData {
            key: (*k).into(),
            name: (*k).into(),
            ..Default::default()
        })
        .collect();
    for (id, nom) in [(1, "My tasks"), (2, "Courses")] {
        lieux.push(iris_ui::TaskPlaceData {
            key: format!("list:{id}").into(),
            name: nom.into(),
            is_list: true,
            list_id: id,
            count: 2,
            ..Default::default()
        });
    }
    f.set_task_places(ModelRc::new(VecModel::from(lieux)));
}

fn une_tache(f: &AppWindow) {
    f.set_task_rows(ModelRc::new(VecModel::from(vec![iris_ui::TaskRowData {
        kind: 0,
        id: 5,
        title: "Payer le loyer".into(),
        ..Default::default()
    }])));
}

fn par_role(f: &AppWindow, role: testing::AccessibleRole, libelle: &str) -> testing::ElementHandle {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(role)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().as_deref() == Some(libelle))
        .unwrap_or_else(|| panic!("rien de « {libelle} »"))
}

fn ctrl_3_ouvre_les_taches_et_n_ecrit_une_tache() {
    let f = fenetre();
    let touches = Rc::new(RefCell::new(Vec::<String>::new()));
    let ajouts = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let t = Rc::clone(&touches);
        f.on_key_pressed(move |k| t.borrow_mut().push(k.to_string()));
        let a = Rc::clone(&ajouts);
        f.on_task_add(move |texte| a.borrow_mut().push(texte.to_string()));
    }

    ctrl(&f, "3");
    assert_eq!(f.get_workspace(), 2, "Ctrl+3 ouvre les tâches");
    touches.borrow_mut().clear();

    taper(&f, "e");
    assert!(
        touches.borrow().is_empty(),
        "« e » ne touche pas le courrier caché"
    );

    taper(&f, "n");
    taper(&f, "Pain demain\n");
    assert_eq!(
        *ajouts.borrow(),
        ["Pain demain"],
        "« n » met le curseur dans la saisie"
    );
}

fn une_tache_se_choisit_et_se_coche_au_clavier() {
    let f = fenetre();
    f.set_workspace(2);
    une_tache(&f);
    let choisies = Rc::new(RefCell::new(Vec::<i32>::new()));
    let cochees = Rc::new(RefCell::new(0));
    let mouvements = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let c = Rc::clone(&choisies);
        f.on_task_row_selected(move |r| c.borrow_mut().push(r.id));
        let c = Rc::clone(&cochees);
        f.on_task_toggle_selected(move || *c.borrow_mut() += 1);
        let m = Rc::clone(&mouvements);
        f.on_task_move(move |n| m.borrow_mut().push(n));
    }

    clic(&par_role(
        &f,
        testing::AccessibleRole::ListItem,
        "Payer le loyer",
    ));
    assert_eq!(*choisies.borrow(), [5]);

    // Le clic a rendu le clavier aux raccourcis de l'onglet.
    taper(&f, " ");
    assert_eq!(*cochees.borrow(), 1, "espace coche la tâche choisie");
    taper(&f, "j");
    assert_eq!(*mouvements.borrow(), [1], "j descend");
}

fn la_case_d_une_tache_la_coche_sans_la_choisir() {
    let f = fenetre();
    f.set_workspace(2);
    une_tache(&f);
    let cochees = Rc::new(RefCell::new(Vec::<i32>::new()));
    let choisies = Rc::new(RefCell::new(0));
    {
        let c = Rc::clone(&cochees);
        f.on_task_toggled(move |id| c.borrow_mut().push(id));
        let c = Rc::clone(&choisies);
        f.on_task_row_selected(move |_| *c.borrow_mut() += 1);
    }
    clic(&par_role(
        &f,
        testing::AccessibleRole::Checkbox,
        "Payer le loyer",
    ));
    assert_eq!(*cochees.borrow(), [5]);
    assert_eq!(*choisies.borrow(), 0, "cocher n'ouvre pas la tâche");
}

fn t_fait_d_une_conversation_une_tache() {
    let f = fenetre();
    f.set_selected_thread(42);
    let fils = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let t = Rc::clone(&fils);
        f.on_thread_to_task(move |id| t.borrow_mut().push(id));
    }
    taper(&f, "t");
    assert_eq!(*fils.borrow(), [42]);

    // Pas depuis une fenêtre ouverte.
    composer(&f);
    taper(&f, "t");
    assert_eq!(
        *fils.borrow(),
        [42],
        "la touche va au champ, pas au courrier"
    );
}

fn une_liste_se_renomme_et_se_supprime_apres_confirmation() {
    let f = fenetre();
    f.set_workspace(2);
    des_lieux(&f);
    let renommees = Rc::new(RefCell::new(Vec::<(bool, i32, String)>::new()));
    let supprimees = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let r = Rc::clone(&renommees);
        f.on_task_list_name_confirmed(move |n, id, nom| {
            r.borrow_mut().push((n, id, nom.to_string()))
        });
        let s = Rc::clone(&supprimees);
        f.on_task_list_delete_confirmed(move |id| s.borrow_mut().push(id));
    }

    par_role(&f, testing::AccessibleRole::Button, "Courses")
        .mock_single_click(PointerEventButton::Right);
    assert!(
        f.get_task_list_menu_open(),
        "clic droit sur une liste : son menu"
    );
    clic(&bouton(&f, "Rename…"));
    taper(&f, "Marché\n");
    assert_eq!(
        *renommees.borrow(),
        [(false, 2, "Marché".to_string())],
        "le nom est remplacé"
    );

    f.set_task_list_name_open(false);
    par_role(&f, testing::AccessibleRole::Button, "Courses")
        .mock_single_click(PointerEventButton::Right);
    clic(&bouton(&f, "Delete list…"));
    assert!(f.get_task_list_delete_open());
    assert!(supprimees.borrow().is_empty());
    clic(&bouton(&f, "Delete"));
    assert_eq!(*supprimees.borrow(), [2]);
}

fn une_nouvelle_liste_s_ouvre_dans_son_nom() {
    let f = fenetre();
    f.set_workspace(2);
    des_lieux(&f);
    let creees = Rc::new(RefCell::new(Vec::<(bool, String)>::new()));
    {
        let c = Rc::clone(&creees);
        f.on_task_list_name_confirmed(move |n, _, nom| c.borrow_mut().push((n, nom.to_string())));
    }
    clic(&bouton(&f, "New list"));
    taper(&f, "Vacances\n");
    assert_eq!(*creees.borrow(), [(true, "Vacances".to_string())]);
}

fn le_titre_d_une_tache_prend_le_premier_clic() {
    let f = fenetre();
    f.set_workspace(2);
    f.set_task_has_detail(true);
    f.set_task_detail_title("Loyer".into());
    let titres = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let t = Rc::clone(&titres);
        f.on_task_title_edited(move |x| t.borrow_mut().push(x.to_string()));
    }
    clic(&champ(&f, "Add a task"));
    taper(&f, "x");
    clic(&champ(&f, "Task title"));
    taper(&f, "!");
    assert_eq!(f.get_task_add_text().as_str(), "x");
    assert!(
        f.get_task_detail_title().ends_with('!'),
        "{}",
        f.get_task_detail_title()
    );
    assert!(!titres.borrow().is_empty(), "chaque frappe est enregistrée");
}

fn main() {
    testing::init_no_event_loop();

    let scenarios: Vec<(&str, fn())> = vec![
        (
            "une_touche_ne_touche_pas_le_courrier_derriere_la_fenetre",
            une_touche_ne_touche_pas_le_courrier_derriere_la_fenetre,
        ),
        (
            "la_fenetre_de_redaction_s_ouvre_dans_le_champ_a",
            la_fenetre_de_redaction_s_ouvre_dans_le_champ_a,
        ),
        (
            "l_arobase_s_ecrit_dans_le_champ_a",
            l_arobase_s_ecrit_dans_le_champ_a,
        ),
        (
            "cliquer_une_suggestion_la_choisit",
            cliquer_une_suggestion_la_choisit,
        ),
        (
            "un_clic_sur_un_autre_champ_y_entre_du_premier_coup",
            un_clic_sur_un_autre_champ_y_entre_du_premier_coup,
        ),
        (
            "un_clic_d_un_champ_a_l_autre_sans_suggestions",
            un_clic_d_un_champ_a_l_autre_sans_suggestions,
        ),
        (
            "ctrl_2_ouvre_l_agenda_et_ses_touches_ne_touchent_pas_le_courrier",
            ctrl_2_ouvre_l_agenda_et_ses_touches_ne_touchent_pas_le_courrier,
        ),
        (
            "l_editeur_d_evenement_s_ouvre_dans_le_titre",
            l_editeur_d_evenement_s_ouvre_dans_le_titre,
        ),
        (
            "l_abonnement_s_ouvre_dans_le_lien",
            l_abonnement_s_ouvre_dans_le_lien,
        ),
        (
            "seules_les_tuiles_proches_de_l_ecran_sont_demandees",
            seules_les_tuiles_proches_de_l_ecran_sont_demandees,
        ),
        (
            "defiler_demande_les_suivantes_et_rend_les_precedentes",
            defiler_demande_les_suivantes_et_rend_les_precedentes,
        ),
        (
            "un_clic_droit_sur_un_calendrier_ouvre_son_menu_qui_garde_le_clavier",
            un_clic_droit_sur_un_calendrier_ouvre_son_menu_qui_garde_le_clavier,
        ),
        (
            "supprimer_un_calendrier_demande_confirmation",
            supprimer_un_calendrier_demande_confirmation,
        ),
        (
            "renommer_un_calendrier_s_ouvre_dans_le_nom",
            renommer_un_calendrier_s_ouvre_dans_le_nom,
        ),
        (
            "la_date_d_un_evenement_se_choisit_dans_un_petit_mois",
            la_date_d_un_evenement_se_choisit_dans_un_petit_mois,
        ),
        (
            "ctrl_3_ouvre_les_taches_et_n_ecrit_une_tache",
            ctrl_3_ouvre_les_taches_et_n_ecrit_une_tache,
        ),
        (
            "une_tache_se_choisit_et_se_coche_au_clavier",
            une_tache_se_choisit_et_se_coche_au_clavier,
        ),
        (
            "la_case_d_une_tache_la_coche_sans_la_choisir",
            la_case_d_une_tache_la_coche_sans_la_choisir,
        ),
        (
            "t_fait_d_une_conversation_une_tache",
            t_fait_d_une_conversation_une_tache,
        ),
        (
            "une_liste_se_renomme_et_se_supprime_apres_confirmation",
            une_liste_se_renomme_et_se_supprime_apres_confirmation,
        ),
        (
            "une_nouvelle_liste_s_ouvre_dans_son_nom",
            une_nouvelle_liste_s_ouvre_dans_son_nom,
        ),
        (
            "le_titre_d_une_tache_prend_le_premier_clic",
            le_titre_d_une_tache_prend_le_premier_clic,
        ),
    ];

    let mut echecs = Vec::new();
    for (nom, scenario) in &scenarios {
        match std::panic::catch_unwind(scenario) {
            Ok(()) => println!("ok    {nom}"),
            Err(e) => {
                let raison = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                println!("FAIL  {nom}: {raison}");
                echecs.push(*nom);
            }
        }
    }
    if !echecs.is_empty() {
        panic!("{} scénario(s) en échec : {echecs:?}", echecs.len());
    }
    println!("saisie: {} scenarios, no failures", scenarios.len());
}
