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

/// The time a tick takes to be drawn before the box says it was ticked.
fn cocher_le_temps() {
    testing::mock_elapsed_time(std::time::Duration::from_millis(400));
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

fn new_message_rouvre_en_grand_une_fenetre_reduite() {
    // Minimising takes the message down to a bar of its own (Rust keeps it), so the
    // window is free for another; New message then opens it full size.
    let f = fenetre();
    let ranges = Rc::new(RefCell::new(0));
    {
        let ranges = Rc::clone(&ranges);
        let fw = f.as_weak();
        f.on_compose_park(move || {
            *ranges.borrow_mut() += 1;
            fw.upgrade().unwrap().set_compose_open(false);
        });
    }
    composer(&f);
    taper(&f, "marie@example.com");
    clic(&bouton(&f, "Minimise"));
    assert_eq!(*ranges.borrow(), 1, "the message goes down to a bar");
    assert!(!f.get_compose_open(), "the window is free");
    clic(&bouton(&f, "Write a new message"));
    assert!(f.get_compose_open());
    assert!(
        !f.get_compose_minimised(),
        "New message ouvre la fenêtre en grand"
    );
}

fn fermer_un_message_ecrit_demande_quoi_en_faire() {
    let f = fenetre();
    let (jetes, fermes, gardes) = (
        Rc::new(RefCell::new(0)),
        Rc::new(RefCell::new(0)),
        Rc::new(RefCell::new(0)),
    );
    {
        let j = Rc::clone(&jetes);
        f.on_compose_discard(move || *j.borrow_mut() += 1);
        let d = Rc::clone(&fermes);
        f.on_compose_dismissed(move || *d.borrow_mut() += 1);
        let g = Rc::clone(&gardes);
        f.on_compose_save_draft(move || *g.borrow_mut() += 1);
    }

    // Empty: it just closes.
    composer(&f);
    clic(&bouton(&f, "Close message"));
    assert!(!f.get_compose_open());
    assert_eq!(*fermes.borrow(), 1);

    // Written: a question first, and Cancel leaves it as it was.
    composer(&f);
    taper(&f, "marie@example.com");
    clic(&bouton(&f, "Close message"));
    assert!(f.get_compose_confirm_close(), "la question s'ouvre");
    assert!(f.get_compose_open());
    clic(&bouton(&f, "Cancel"));
    assert!(!f.get_compose_confirm_close());
    assert_eq!(f.get_compose_to().as_str(), "marie@example.com");

    // Escape asks too, and a second Escape answers "no".
    echap(&f);
    assert!(f.get_compose_confirm_close(), "Échap pose la même question");
    echap(&f);
    assert!(!f.get_compose_confirm_close());
    assert!(f.get_compose_open());

    clic(&bouton(&f, "Close message"));
    clic(&bouton(&f, "Discard"));
    assert_eq!(*jetes.borrow(), 1);

    composer(&f);
    clic(&bouton(&f, "Save draft"));
    assert_eq!(*gardes.borrow(), 1, "le brouillon se garde d'un bouton");
}

fn cc_et_bcc_se_replient_sauf_ce_qui_est_rempli() {
    let f = fenetre();
    composer(&f);
    // A field shows its label on more than one element: the set is what counts.
    let champs = |f: &AppWindow| -> std::collections::BTreeSet<String> {
        testing::ElementQuery::from_root(f)
            .match_descendants()
            .match_accessible_role(testing::AccessibleRole::TextInput)
            .find_all()
            .into_iter()
            .filter_map(|e| e.accessible_label().map(|l| l.to_string()))
            .filter(|l| l == "Cc" || l == "Bcc")
            .collect()
    };
    let ensemble = |noms: &[&str]| -> std::collections::BTreeSet<String> {
        noms.iter().map(|n| n.to_string()).collect()
    };
    assert!(champs(&f).is_empty(), "repliés au départ");
    clic(&bouton(&f, "Add Cc and Bcc"));
    assert_eq!(champs(&f), ensemble(&["Cc", "Bcc"]));
    clic(&champ(&f, "Cc"));
    taper(&f, "paul@example.com");
    clic(&bouton(&f, "Fold Cc and Bcc"));
    assert_eq!(
        champs(&f),
        ensemble(&["Cc"]),
        "le champ rempli reste, le vide se replie"
    );
}

// --- Les corps peints par tuiles ---

fn corps_en_tuiles(f: &AppWindow, tuiles: usize) {
    corps_en_tuiles_de(f, tuiles, 1.0);
}

/// `tuiles` tiles, each `aspect` times as high as it is wide.
fn corps_en_tuiles_de(f: &AppWindow, tuiles: usize, aspect: f32) {
    use iris_ui::{BodyTileData, MessageData};
    let tuiles: Vec<BodyTileData> = (0..tuiles)
        .map(|_| BodyTileData {
            image: slint::Image::default(),
            ready: false,
            aspect,
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

fn a_click_on_a_painted_body_asks_for_the_link_under_it() {
    // A click that drags nothing reaches Rust with the message, the tile and where in
    // it: that is how a link of a newsletter opens. It was only ever a selection.
    let f = fenetre();
    corps_en_tuiles_de(&f, 2, 0.2);
    let clics = Rc::new(RefCell::new(Vec::new()));
    {
        let c = Rc::clone(&clics);
        f.on_body_link(move |m, t, x, y| c.borrow_mut().push((m, t, x, y)));
    }
    let tuile = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Image)
        .find_all()
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some("Message body"))
        .expect("the first tile of the body");

    clic(&tuile);

    let clics = clics.borrow();
    assert_eq!(clics.len(), 1, "one click, one question: {clics:?}");
    let (m, t, x, y) = clics[0];
    assert_eq!((m, t), (7, 0));
    assert!(
        (x - 0.5).abs() < 0.05 && (y - 0.5).abs() < 0.05,
        "the middle of the tile: {x}, {y}"
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

fn ctrl_4_opens_the_notes_and_escape_closes_their_menus() {
    let f = fenetre();
    ctrl(&f, "4");
    assert_eq!(f.get_workspace(), 4, "Ctrl+4 opens the notes");
    assert!(!f.get_notes_ctrl_held(), "Ctrl let go");
    f.set_notes_row_menu_open(true);
    echap(&f);
    assert!(!f.get_notes_row_menu_open(), "Escape closes the row's menu");
    f.set_notes_quick_open(true);
    taper(&f, "chap");
    assert_eq!(
        f.get_notes_quick_query().as_str(),
        "chap",
        "Quick open takes the keys"
    );
    echap(&f);
    assert!(!f.get_notes_quick_open(), "Escape closes Quick open");
}

fn a_note_s_line_takes_every_key_and_enter_goes_to_rust() {
    let f = fenetre();
    ctrl(&f, "4");
    let edites = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let edites = Rc::clone(&edites);
        f.on_note_block_edited(move |_, t, _| edites.borrow_mut().push(t.to_string()));
    }
    let cles = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let cles = Rc::clone(&cles);
        f.on_note_block_key(move |_, n, _, _, _, _| {
            cles.borrow_mut().push(n.to_string());
            n == "enter"
        });
    }
    f.set_note_open(true);
    f.set_note_blocks(ModelRc::new(VecModel::from(vec![iris_ui::NoteBlockData {
        kind: "paragraph".into(),
        ..Default::default()
    }])));
    f.set_note_focus(0);
    f.set_note_focus_serial(1);
    // AltGr's characters too, as an AZERTY keyboard gives them.
    taper(&f, "Hé@€");
    assert_eq!(
        edites.borrow().last().map(String::as_str),
        Some("Hé@€"),
        "the line takes the keys"
    );
    let entree = SharedString::from(slint::platform::Key::Return);
    f.window().dispatch_event(WindowEvent::KeyPressed {
        text: entree.clone(),
    });
    f.window()
        .dispatch_event(WindowEvent::KeyReleased { text: entree });
    assert!(
        cles.borrow().iter().any(|k| k == "enter"),
        "Enter is Rust's: {:?}",
        cles.borrow()
    );
}

/// A note of `n` lines of words.
fn des_lignes(f: &AppWindow, n: usize) {
    let blocs: Vec<iris_ui::NoteBlockData> = (0..n)
        .map(|k| iris_ui::NoteBlockData {
            kind: "paragraph".into(),
            source: format!("Ligne {k}").into(),
            rich: slint::StyledText::from_plain_text(&format!("Ligne {k}")),
            ..Default::default()
        })
        .collect();
    f.set_note_open(true);
    f.set_note_blocks(ModelRc::new(VecModel::from(blocs)));
}

/// The window's items made, as a frame drawn makes them.
fn construire(f: &AppWindow) {
    let _ = testing::ElementQuery::from_root(f)
        .match_descendants()
        .find_all();
}

fn a_line_made_below_the_page_keeps_the_keyboard() {
    let f = fenetre();
    ctrl(&f, "4");
    let edites = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let edites = Rc::clone(&edites);
        f.on_note_block_edited(move |i, _, _| edites.borrow_mut().push(i));
    }
    // Enter at the end of a long note: the new line is below what shows.
    des_lignes(&f, 80);
    f.set_note_focus(79);
    f.set_note_focus_serial(1);
    construire(&f);
    testing::mock_elapsed_time(std::time::Duration::from_millis(20));
    taper(&f, "x");
    assert_eq!(
        edites.borrow().last().copied(),
        Some(79),
        "the page scrolls to the new line, which keeps the keys: {:?}",
        edites.borrow()
    );
}

fn an_embedded_spreadsheet_s_cells_are_chosen_and_take_the_keys() {
    let f = fenetre();
    ctrl(&f, "4");
    let g = f.global::<iris_ui::NoteSelect>();
    let choisies = Rc::new(RefCell::new(Vec::<(i32, i32, i32)>::new()));
    {
        let (choisies, faible) = (Rc::clone(&choisies), f.as_weak());
        g.on_cell_pressed(move |i, r, c, _| {
            choisies.borrow_mut().push((i, r, c));
            // As Rust does: the cell selected, the keyboard to the cells.
            let f = faible.unwrap();
            let g = f.global::<iris_ui::NoteSelect>();
            g.set_r1(r);
            g.set_r2(r);
            g.set_c1(c);
            g.set_c2(c);
            g.set_row(r);
            g.set_col(c);
            g.set_sheet_block(i);
            g.set_cells_serial(g.get_cells_serial() + 1);
        });
    }
    let cles = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let cles = Rc::clone(&cles);
        g.on_cell_key(move |n| {
            cles.borrow_mut().push(n.to_string());
            true
        });
    }
    let cellules: Vec<SharedString> = ["Poste", "Montant", "Loyer", "800"]
        .into_iter()
        .map(SharedString::from)
        .collect();
    f.set_note_open(true);
    f.set_note_blocks(ModelRc::new(VecModel::from(vec![
        iris_ui::NoteBlockData {
            kind: "paragraph".into(),
            source: "Budget".into(),
            rich: slint::StyledText::from_plain_text("Budget"),
            ..Default::default()
        },
        iris_ui::NoteBlockData {
            kind: "table".into(),
            sheet: true,
            rows: 2,
            columns: 2,
            cells: ModelRc::new(VecModel::from(cellules)),
            widths: ModelRc::new(VecModel::from(vec![120.0f32, 120.0])),
            widths_total: 240.0,
            edges: ModelRc::new(VecModel::from(vec![120.0f32, 240.0])),
            ..Default::default()
        },
    ])));
    // The line above is being written: one click on a cell takes it.
    f.set_note_focus(0);
    f.set_note_focus_serial(1);
    let montant = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .find_all()
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some("800"))
        .expect("the cell 800 is drawn");
    clic(&montant);
    assert_eq!(
        *choisies.borrow(),
        vec![(1, 1, 1)],
        "the click chose the cell"
    );
    construire(&f);
    taper(&f, "4");
    ctrl(&f, "c");
    assert_eq!(
        *cles.borrow(),
        vec!["type:4".to_string(), "ctrl+c".to_string()],
        "the cells take the keys"
    );
}

fn a_heading_is_typed_without_its_hashes() {
    let f = fenetre();
    ctrl(&f, "4");
    let edites = Rc::new(RefCell::new(Vec::<(String, i32)>::new()));
    {
        let edites = Rc::clone(&edites);
        f.on_note_block_edited(move |_, t, c| edites.borrow_mut().push((t.to_string(), c)));
    }
    f.set_note_open(true);
    f.set_note_blocks(ModelRc::new(VecModel::from(vec![iris_ui::NoteBlockData {
        kind: "heading".into(),
        level: 2,
        source: "Limites".into(),
        prefix: "## ".into(),
        prefix_bytes: 3,
        ..Default::default()
    }])));
    // The cursor after "## " in the whole source: the start of the words shown.
    f.set_note_focus_cursor(3);
    f.set_note_focus_anchor(3);
    f.set_note_focus(0);
    f.set_note_focus_serial(1);
    taper(&f, "Les ");
    let dernier = edites.borrow().last().cloned();
    assert_eq!(
        dernier,
        Some(("## Les Limites".to_string(), 7)),
        "the field holds the words; Rust gets the whole line"
    );
}

fn a_new_note_s_title_is_selected_and_typing_names_it() {
    let f = fenetre();
    ctrl(&f, "4");
    let titres = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let titres = Rc::clone(&titres);
        f.on_note_title_edited(move |t| titres.borrow_mut().push(t.to_string()));
    }
    // A new note where none was open: its title field is made with the note.
    f.set_note_title("Untitled".into());
    f.set_note_title_pending(true);
    f.set_note_open(true);
    taper(&f, "Cours");
    assert_eq!(
        titres.borrow().last().map(String::as_str),
        Some("Cours"),
        "typing replaces the selected title: {:?}",
        titres.borrow()
    );
    assert!(!f.get_note_title_pending(), "the title was taken");
}

fn a_spreadsheet_takes_the_keys_and_its_menu_owns_them_until_escape() {
    let f = fenetre();
    ctrl(&f, "4");
    let g = f.global::<iris_ui::SheetGrid>();
    let cles = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let cles = Rc::clone(&cles);
        g.on_key(move |t, _, _| {
            cles.borrow_mut().push(t.to_string());
            true
        });
    }
    f.set_note_sheet_open(true);
    g.set_focus_serial(g.get_focus_serial() + 1);
    taper(&f, "=@");
    assert_eq!(*cles.borrow(), vec!["=".to_string(), "@".to_string()]);
    g.set_menu("more".into());
    taper(&f, "x");
    assert_eq!(cles.borrow().len(), 2, "nothing passes an open menu");
    echap(&f);
    assert_eq!(g.get_menu().as_str(), "", "Escape closes the menu");
    taper(&f, "y");
    assert_eq!(cles.borrow().last().map(String::as_str), Some("y"));
}

fn the_notes_panels_close_with_escape() {
    let f = fenetre();
    ctrl(&f, "4");
    f.set_notes_history_open(true);
    echap(&f);
    assert!(!f.get_notes_history_open(), "Escape closes the history");
    f.set_notes_graph_open(true);
    echap(&f);
    assert!(!f.get_notes_graph_open(), "Escape closes the graph");
    f.set_notes_review_open(true);
    echap(&f);
    assert!(!f.get_notes_review_open(), "Escape closes the revision");
}

fn connecting_calendars_opens_in_the_address_and_escape_closes() {
    let f = fenetre();
    f.set_workspace(1);
    f.set_connect_open(true);
    taper(&f, "a@example.com");
    assert_eq!(f.get_connect_server().as_str(), "a@example.com");
    echap(&f);
    assert!(!f.get_connect_open(), "Escape closes it");
}

fn un_calendrier(f: &AppWindow, abonne: bool) {
    f.set_workspace(1);
    f.set_calendars(ModelRc::new(VecModel::from(vec![iris_ui::CalendarData {
        id: 7,
        name: "Club".into(),
        color: slint::Color::from_rgb_u8(0x4f, 0x8c, 0xff),
        visible: true,
        subscribed: abonne,
        deletable: true,
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
    assert!(cochees.borrow().is_empty(), "the tick is drawn first");
    cocher_le_temps();
    assert_eq!(*cochees.borrow(), [5]);
    assert_eq!(*choisies.borrow(), 0, "cocher n'ouvre pas la tâche");
}

fn centre(e: &testing::ElementHandle) -> slint::LogicalPosition {
    let (pos, taille) = (e.absolute_position(), e.size());
    slint::LogicalPosition::new(pos.x + taille.width / 2.0, pos.y + taille.height / 2.0)
}

/// Le bouton enfoncé sur `de`, la souris menée pas à pas jusqu'à `a`, puis relâchée.
fn porter(f: &AppWindow, de: slint::LogicalPosition, a: slint::LogicalPosition) {
    let bouton = PointerEventButton::Left;
    f.window()
        .dispatch_event(WindowEvent::PointerMoved { position: de });
    f.window().dispatch_event(WindowEvent::PointerPressed {
        position: de,
        button: bouton,
    });
    for i in 1..=10 {
        let t = i as f32 / 10.0;
        let position =
            slint::LogicalPosition::new(de.x + (a.x - de.x) * t, de.y + (a.y - de.y) * t);
        f.window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        slint::platform::update_timers_and_animations();
    }
    f.window().dispatch_event(WindowEvent::PointerReleased {
        position: a,
        button: bouton,
    });
    slint::platform::update_timers_and_animations();
}

fn une_tache_portee_sur_une_liste_y_va() {
    let f = fenetre();
    f.set_workspace(2);
    des_lieux(&f);
    une_tache(&f);
    let deposees = Rc::new(RefCell::new(Vec::<(i32, String)>::new()));
    let choisies = Rc::new(RefCell::new(0));
    {
        let d = Rc::clone(&deposees);
        f.on_task_dropped(move |id, cle| d.borrow_mut().push((id, cle.to_string())));
        let c = Rc::clone(&choisies);
        f.on_task_row_selected(move |_| *c.borrow_mut() += 1);
    }

    let tache = centre(&par_role(
        &f,
        testing::AccessibleRole::ListItem,
        "Payer le loyer",
    ));
    let courses = centre(&par_role(&f, testing::AccessibleRole::Button, "Courses"));
    porter(&f, tache, courses);
    assert_eq!(
        *deposees.borrow(),
        [(5, "list:2".to_string())],
        "lâchée sur « Courses », la tâche y va"
    );
    assert_eq!(*choisies.borrow(), 0, "porter n'ouvre pas la tâche");

    // Lâchée ailleurs que sur une liste, elle reste où elle est.
    let a_venir = centre(&par_role(&f, testing::AccessibleRole::Button, "upcoming"));
    porter(&f, tache, a_venir);
    assert_eq!(deposees.borrow().len(), 1, "« upcoming » ne prend rien");

    // Un simple clic, sans bouger, la choisit toujours.
    clic(&par_role(
        &f,
        testing::AccessibleRole::ListItem,
        "Payer le loyer",
    ));
    assert_eq!(*choisies.borrow(), 1);
}

fn une_tache_se_reordonne_en_la_glissant() {
    let f = fenetre();
    f.set_workspace(2);
    let tache = |id: i32, titre: &str| iris_ui::TaskRowData {
        kind: 0,
        id,
        title: titre.into(),
        ..Default::default()
    };
    f.set_task_rows(ModelRc::new(VecModel::from(vec![
        tache(5, "Payer le loyer"),
        tache(6, "Appeler Marie"),
    ])));
    let places = Rc::new(RefCell::new(Vec::<(i32, i32, bool)>::new()));
    {
        let p = Rc::clone(&places);
        f.on_task_reordered(move |a, b, dessus| p.borrow_mut().push((a, b, dessus)));
    }
    let premiere = par_role(&f, testing::AccessibleRole::ListItem, "Payer le loyer");
    let seconde = par_role(&f, testing::AccessibleRole::ListItem, "Appeler Marie");
    let (pos, taille) = (seconde.absolute_position(), seconde.size());
    // Let go on the lower half of the second: the first goes below it.
    let bas = slint::LogicalPosition::new(pos.x + taille.width / 2.0, pos.y + taille.height - 6.0);
    porter(&f, centre(&premiere), bas);
    assert_eq!(*places.borrow(), [(5, 6, false)]);
}

fn les_taches_terminees_se_suppriment_d_un_clic() {
    let f = fenetre();
    f.set_workspace(2);
    f.set_task_rows(ModelRc::new(VecModel::from(vec![
        iris_ui::TaskRowData {
            kind: 4,
            title: "COMPLETED".into(),
            ..Default::default()
        },
        iris_ui::TaskRowData {
            kind: 0,
            id: 7,
            title: "Arroser".into(),
            done: true,
            ..Default::default()
        },
    ])));
    let vidages = Rc::new(RefCell::new(0));
    {
        let v = Rc::clone(&vidages);
        f.on_task_clear_completed(move || *v.borrow_mut() += 1);
    }
    clic(&bouton(&f, "Delete completed tasks"));
    assert_eq!(*vidages.borrow(), 1);
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

// --- Les comptes et leurs tags ---

fn deux_comptes(f: &AppWindow) {
    let compte = |id: i32, nom: &str| iris_ui::AccountRowData {
        id,
        label: nom.into(),
        ..Default::default()
    };
    f.set_other_accounts(ModelRc::new(VecModel::from(vec![
        compte(3, "a@example.com"),
        compte(4, "b@example.com"),
    ])));
}

fn ligne_de_compte(f: &AppWindow, nom: &str) -> testing::ElementHandle {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().as_deref() == Some(nom))
        .unwrap_or_else(|| panic!("aucun compte nommé « {nom} »"))
}

fn un_second_clic_droit_ouvre_le_menu_de_l_autre_compte() {
    let f = fenetre();
    deux_comptes(&f);
    let demandes = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let d = Rc::clone(&demandes);
        let fw = f.as_weak();
        f.on_account_menu_requested(move |id| {
            d.borrow_mut().push(id);
            fw.upgrade().unwrap().set_account_menu_open(true);
        });
        let fw = f.as_weak();
        f.on_account_menu_dismissed(move || fw.upgrade().unwrap().set_account_menu_open(false));
    }

    ligne_de_compte(&f, "a@example.com").mock_single_click(PointerEventButton::Right);
    assert_eq!(*demandes.borrow(), [3]);
    assert!(f.get_account_menu_open());

    // The menu's veil covers the second row; a right click on its visible part — its
    // left edge, clear of the menu opened at the first row's centre — still reaches it.
    let b = ligne_de_compte(&f, "b@example.com");
    let (pos, taille) = (b.absolute_position(), b.size());
    let point = slint::LogicalPosition::new(pos.x + 6.0, pos.y + taille.height / 2.0);
    f.window().dispatch_event(WindowEvent::PointerPressed {
        position: point,
        button: PointerEventButton::Right,
    });
    f.window().dispatch_event(WindowEvent::PointerReleased {
        position: point,
        button: PointerEventButton::Right,
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(
        *demandes.borrow(),
        [3, 4],
        "le second clic droit ouvre le menu de l'autre"
    );
    assert!(f.get_account_menu_open());
}

fn les_tags_d_un_compte_se_cherchent_dans_son_menu() {
    let f = fenetre();
    f.set_account_menu_label("a@example.com".into());
    f.set_account_menu_tags(ModelRc::new(VecModel::from(vec![
        iris_ui::AccountTagData {
            id: 1,
            name: "Clients".into(),
            ..Default::default()
        },
        iris_ui::AccountTagData {
            id: 2,
            name: "Perso".into(),
            checked: true,
            ..Default::default()
        },
    ])));
    f.set_account_menu_open(true);
    let cherches = Rc::new(RefCell::new(Vec::<String>::new()));
    let coches = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let c = Rc::clone(&cherches);
        f.on_account_tag_search_changed(move |t| c.borrow_mut().push(t.to_string()));
        let c = Rc::clone(&coches);
        f.on_account_tag_toggled(move |id| c.borrow_mut().push(id));
    }

    clic(&bouton(&f, "Tags"));
    assert!(f.get_account_tags_open(), "l'entrée ouvre le sous-menu");
    taper(&f, "cli");
    assert_eq!(
        f.get_account_tag_search().as_str(),
        "cli",
        "la recherche prend la frappe"
    );
    assert_eq!(cherches.borrow().last().map(String::as_str), Some("cli"));

    clic(&par_role(&f, testing::AccessibleRole::Checkbox, "Clients"));
    assert_eq!(*coches.borrow(), [1]);
    assert!(f.get_account_menu_open(), "cocher laisse le menu ouvert");

    echap(&f);
    assert!(
        !f.get_account_tags_open(),
        "Échap replie d'abord le sous-menu"
    );
    assert!(f.get_account_menu_open());
}

fn les_tags_se_gerent_depuis_le_bas_de_la_colonne() {
    let f = fenetre();
    let demandes = Rc::new(RefCell::new(0));
    let crees = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let d = Rc::clone(&demandes);
        let fw = f.as_weak();
        f.on_tags_requested(move || {
            *d.borrow_mut() += 1;
            fw.upgrade().unwrap().set_tags_open(true);
        });
        let c = Rc::clone(&crees);
        f.on_tag_created(move |t| c.borrow_mut().push(t.to_string()));
    }
    clic(&bouton(&f, "Manage tags"));
    assert_eq!(*demandes.borrow(), 1);
    taper(&f, "Clients\n");
    assert_eq!(
        *crees.borrow(),
        ["Clients"],
        "la fenêtre s'ouvre dans le nom du tag"
    );

    let touches = Rc::new(RefCell::new(0));
    {
        let t = Rc::clone(&touches);
        f.on_key_pressed(move |_| *t.borrow_mut() += 1);
    }
    f.set_tags_new_name("".into());
    echap(&f);
    assert!(!f.get_tags_open(), "Échap ferme la fenêtre des tags");
}

fn le_regroupement_par_tag_s_allume_au_dessus_de_la_liste() {
    let f = fenetre();
    deux_comptes(&f);
    let changes = Rc::new(RefCell::new(0));
    {
        let c = Rc::clone(&changes);
        f.on_group_by_tags_changed(move || *c.borrow_mut() += 1);
    }
    assert!(f.get_group_by_tags(), "grouped by tag from the start");
    clic(&bouton(&f, "Show accounts in one list"));
    assert!(!f.get_group_by_tags());
    assert_eq!(*changes.borrow(), 1);
    clic(&bouton(&f, "Group accounts by tag"));
    assert!(f.get_group_by_tags());
}

fn un_nouveau_calendrier_s_ouvre_dans_son_nom() {
    let f = fenetre();
    un_calendrier(&f, false);
    let crees = Rc::new(RefCell::new(Vec::<(i32, String)>::new()));
    {
        let c = Rc::clone(&crees);
        f.on_calendar_rename_confirmed(move |id, nom| c.borrow_mut().push((id, nom.to_string())));
    }
    clic(&bouton(&f, "New calendar"));
    assert!(f.get_calendar_rename_open());
    taper(&f, "Sport\n");
    assert_eq!(
        *crees.borrow(),
        [(-1, "Sport".to_string())],
        "-1 : un calendrier à créer"
    );
}

fn le_dernier_calendrier_perso_ne_se_supprime_pas() {
    let f = fenetre();
    f.set_workspace(1);
    f.set_calendars(ModelRc::new(VecModel::from(vec![iris_ui::CalendarData {
        id: 1,
        name: "Personal".into(),
        visible: true,
        deletable: false,
        ..Default::default()
    }])));
    ligne_de_calendrier(&f, "Personal").mock_single_click(PointerEventButton::Right);
    assert!(f.get_calendar_menu_open());
    let supprimer = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .any(|b| b.accessible_label().as_deref() == Some("Delete calendar…"));
    assert!(
        !supprimer,
        "pas de suppression pour le seul calendrier à soi"
    );
}

// --- Home, and back and forward ---

fn avec_modificateur(f: &AppWindow, modificateur: slint::platform::Key, touche: SharedString) {
    let m = SharedString::from(modificateur);
    f.window()
        .dispatch_event(WindowEvent::KeyPressed { text: m.clone() });
    f.window().dispatch_event(WindowEvent::KeyPressed {
        text: touche.clone(),
    });
    f.window()
        .dispatch_event(WindowEvent::KeyReleased { text: touche });
    f.window()
        .dispatch_event(WindowEvent::KeyReleased { text: m });
}

fn le_nom_d_iris_mene_a_l_accueil_qui_garde_le_courrier_a_l_abri() {
    let f = fenetre();
    let touches = Rc::new(RefCell::new(Vec::<String>::new()));
    let espaces = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let t = Rc::clone(&touches);
        f.on_key_pressed(move |k| t.borrow_mut().push(k.to_string()));
        let e = Rc::clone(&espaces);
        f.on_workspace_changed(move |w| e.borrow_mut().push(w));
    }
    clic(&par_role(&f, testing::AccessibleRole::Tab, "Home"));
    assert_eq!(f.get_workspace(), 3, "the switcher opens Home");
    assert_eq!(*espaces.borrow(), [3]);

    taper(&f, "e#");
    assert!(
        touches.borrow().is_empty(),
        "no key reaches the mail behind Home: {:?}",
        touches.borrow()
    );

    ctrl(&f, "1");
    assert_eq!(f.get_workspace(), 0);
    ctrl(&f, "0");
    assert_eq!(f.get_workspace(), 3, "Ctrl+0 is Home");
}

fn l_accueil_mene_a_ce_qu_il_montre() {
    let f = fenetre();
    f.set_workspace(3);
    // The one thing next: an event first.
    f.set_home_next(ModelRc::new(VecModel::from(vec![iris_ui::HomeItemData {
        key: "4:1000".into(),
        title: "Design review".into(),
        meta: "14:00, in 40 min".into(),
        ..Default::default()
    }])));
    let ouverts = Rc::new(RefCell::new(Vec::<String>::new()));
    let evenements = Rc::new(RefCell::new(Vec::<String>::new()));
    let cochees = Rc::new(RefCell::new(Vec::<i32>::new()));
    let taches = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let o = Rc::clone(&ouverts);
        f.on_home_open(move |t| o.borrow_mut().push(t.to_string()));
        let e = Rc::clone(&evenements);
        f.on_home_event_opened(move |k| e.borrow_mut().push(k.to_string()));
        let c = Rc::clone(&cochees);
        f.on_home_task_toggled(move |id| c.borrow_mut().push(id));
        let t = Rc::clone(&taches);
        f.on_home_task_opened(move |id| t.borrow_mut().push(id));
    }
    clic(&bouton(&f, "Open Mail"));
    clic(&bouton(&f, "Open Tasks"));
    clic(&bouton(&f, "Open Calendar"));
    assert_eq!(*ouverts.borrow(), ["mail", "tasks", "calendar"]);
    clic(&bouton(&f, "Next: Design review"));
    assert_eq!(*evenements.borrow(), ["4:1000"], "an event opens from Home");

    // Then a task: its box ticks it, the rest of the line opens it.
    f.set_home_next(ModelRc::new(VecModel::from(vec![iris_ui::HomeItemData {
        id: 9,
        title: "Call the plumber".into(),
        meta: "15:00, in 1 h".into(),
        ..Default::default()
    }])));
    clic(&par_role(
        &f,
        testing::AccessibleRole::Checkbox,
        "Call the plumber",
    ));
    cocher_le_temps();
    assert_eq!(*cochees.borrow(), [9], "the box ticks the task");
    assert!(taches.borrow().is_empty(), "ticking does not open it");
    clic(&bouton(&f, "Next: Call the plumber"));
    assert_eq!(*taches.borrow(), [9]);
}

fn retour_et_avant_aux_boutons_et_au_clavier() {
    let f = fenetre();
    let retours = Rc::new(RefCell::new(0));
    let avances = Rc::new(RefCell::new(0));
    let pas = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
    {
        let r = Rc::clone(&retours);
        f.on_nav_back(move || *r.borrow_mut() += 1);
        let a = Rc::clone(&avances);
        f.on_nav_forward(move || *a.borrow_mut() += 1);
        let p = Rc::clone(&pas);
        f.on_navigated(move |k, v| p.borrow_mut().push((k.to_string(), v.to_string())));
    }
    // Nowhere to go yet: the buttons do nothing.
    clic(&bouton(&f, "Back"));
    assert_eq!(*retours.borrow(), 0);

    f.set_can_go_back(true);
    f.set_can_go_forward(true);
    clic(&bouton(&f, "Back"));
    clic(&bouton(&f, "Forward"));
    assert_eq!((*retours.borrow(), *avances.borrow()), (1, 1));

    avec_modificateur(
        &f,
        slint::platform::Key::Alt,
        SharedString::from(slint::platform::Key::LeftArrow),
    );
    assert_eq!(*retours.borrow(), 2, "Alt+Left goes back");

    // A choice in the interface is a step of the history.
    deux_comptes(&f);
    f.set_group_by_tags(false);
    clic(&ligne_de_compte(&f, "b@example.com"));
    assert_eq!(
        pas.borrow().last(),
        Some(&("account".to_string(), "4".to_string()))
    );

    // Not from an open window.
    composer(&f);
    avec_modificateur(
        &f,
        slint::platform::Key::Alt,
        SharedString::from(slint::platform::Key::LeftArrow),
    );
    assert_eq!(*retours.borrow(), 2);
}

// --- The accounts under their tags ---

fn deux_tags(f: &AppWindow) {
    let compte = |id: i32, nom: &str| iris_ui::AccountRowData {
        id,
        label: nom.into(),
        ..Default::default()
    };
    f.set_other_accounts(ModelRc::new(VecModel::from(vec![
        iris_ui::AccountRowData {
            id: -1,
            label: "Clients".into(),
            header: true,
            tag_id: 5,
            members: 1,
            ..Default::default()
        },
        compte(3, "a@example.com"),
        iris_ui::AccountRowData {
            id: -1,
            label: "Personal".into(),
            header: true,
            tag_id: 6,
            members: 1,
            folded: true,
            ..Default::default()
        },
    ])));
}

fn un_tag_se_choisit_et_se_replie() {
    let f = fenetre();
    deux_tags(&f);
    let choisis = Rc::new(RefCell::new(Vec::<i32>::new()));
    let plies = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let c = Rc::clone(&choisis);
        let fw = f.as_weak();
        f.on_tag_selected(move |t| {
            c.borrow_mut().push(t);
            fw.upgrade().unwrap().set_selected_tag(t);
        });
        let p = Rc::clone(&plies);
        f.on_tag_fold_toggled(move |t| p.borrow_mut().push(t));
    }
    clic(&ligne_de_compte(&f, "Clients"));
    assert_eq!(*choisis.borrow(), [5], "the title shows the tag's mail");
    assert!(
        ligne_de_compte(&f, "Clients")
            .accessible_item_selected()
            .unwrap_or(false),
        "the tag is marked as the place shown"
    );
    assert!(!ligne_de_compte(&f, "a@example.com")
        .accessible_item_selected()
        .unwrap_or(false));

    // The chevron, at the left of the title, folds instead.
    let titre = ligne_de_compte(&f, "Personal");
    let (pos, taille) = (titre.absolute_position(), titre.size());
    let point = slint::LogicalPosition::new(pos.x + 10.0, pos.y + taille.height / 2.0);
    f.window().dispatch_event(WindowEvent::PointerPressed {
        position: point,
        button: PointerEventButton::Left,
    });
    f.window().dispatch_event(WindowEvent::PointerReleased {
        position: point,
        button: PointerEventButton::Left,
    });
    assert_eq!(*plies.borrow(), [6]);
    assert_eq!(*choisis.borrow(), [5], "folding does not open the tag");
    // And the chevron is a button of its own for a screen reader.
    bouton(&f, "Unfold Personal");
    bouton(&f, "Fold Clients");
}

fn les_tags_se_rangent_a_la_souris() {
    let f = fenetre();
    let tag = |id: i32, nom: &str| iris_ui::AccountTagData {
        id,
        name: nom.into(),
        ..Default::default()
    };
    f.set_account_tags(ModelRc::new(VecModel::from(vec![
        tag(1, "Alpha"),
        tag(2, "Beta"),
        tag(3, "Gamma"),
    ])));
    f.set_tags_open(true);
    let deplaces = Rc::new(RefCell::new(Vec::<(i32, i32)>::new()));
    {
        let d = Rc::clone(&deplaces);
        f.on_tag_moved(move |id, i| d.borrow_mut().push((id, i)));
    }
    // Gamma's handle, carried above Alpha.
    // One field per row, found once per row whatever the query returns.
    let mut noms: Vec<testing::ElementHandle> = Vec::new();
    for c in testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::TextInput)
        .find_all()
        .into_iter()
        .filter(|c| c.accessible_label().as_deref() == Some("Tag name"))
    {
        let y = c.absolute_position().y;
        if !noms
            .iter()
            .any(|n| (n.absolute_position().y - y).abs() < 1.0)
        {
            noms.push(c);
        }
    }
    noms.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
    assert_eq!(noms.len(), 3);
    let poignee = |e: &testing::ElementHandle| {
        let (pos, taille) = (e.absolute_position(), e.size());
        // The handle sits left of the colour, left of the name.
        slint::LogicalPosition::new(pos.x - 36.0, pos.y + taille.height / 2.0)
    };
    let de = poignee(&noms[2]);
    let a = slint::LogicalPosition::new(de.x, noms[0].absolute_position().y - 4.0);
    porter(&f, de, a);
    assert_eq!(*deplaces.borrow(), [(3, 0)], "Gamma goes first");
}

// --- Tasks: a delete is undone ---

fn une_tache_supprimee_revient_par_ctrl_z() {
    let f = fenetre();
    f.set_workspace(2);
    une_tache(&f);
    f.set_task_has_detail(true);
    f.set_task_detail_title("Payer le loyer".into());
    let supprimees = Rc::new(RefCell::new(0));
    let annulations = Rc::new(RefCell::new(0));
    let annulations_courrier = Rc::new(RefCell::new(0));
    {
        let s = Rc::clone(&supprimees);
        let fw = f.as_weak();
        f.on_task_delete(move || {
            *s.borrow_mut() += 1;
            fw.upgrade().unwrap().set_task_has_detail(false);
        });
        let a = Rc::clone(&annulations);
        f.on_task_undo(move || *a.borrow_mut() += 1);
        let a = Rc::clone(&annulations_courrier);
        f.on_undo(move || *a.borrow_mut() += 1);
    }
    // The cursor in the title, then the button: the panel closes with the task.
    clic(&champ(&f, "Task title"));
    clic(&bouton(&f, "Delete task"));
    assert_eq!(*supprimees.borrow(), 1);
    ctrl(&f, "z");
    assert_eq!(*annulations.borrow(), 1, "Ctrl+Z brings the task back");
    assert_eq!(
        *annulations_courrier.borrow(),
        0,
        "and not a mail action behind"
    );
}

// --- An event's tasks ---

fn une_tache_s_ajoute_a_un_evenement() {
    let f = fenetre();
    f.set_workspace(1);
    f.set_event_detail(iris_ui::EventDetailData {
        title: "Quarterly review".into(),
        ..Default::default()
    });
    f.set_event_detail_open(true);
    f.set_event_tasks(ModelRc::new(VecModel::from(vec![iris_ui::SubtaskData {
        id: 4,
        title: "Print the slides".into(),
        done: false,
    }])));
    let ajoutees = Rc::new(RefCell::new(Vec::<String>::new()));
    let cochees = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let a = Rc::clone(&ajoutees);
        f.on_event_task_added(move |t| a.borrow_mut().push(t.to_string()));
        let c = Rc::clone(&cochees);
        f.on_event_task_toggled(move |id| c.borrow_mut().push(id));
    }
    clic(&champ(&f, "Add a task for this event"));
    taper(&f, "Book the room\n");
    assert_eq!(*ajoutees.borrow(), ["Book the room"]);
    clic(&par_role(
        &f,
        testing::AccessibleRole::Checkbox,
        "Print the slides",
    ));
    cocher_le_temps();
    assert_eq!(*cochees.borrow(), [4]);
}

// --- Fields keep one line's height ---

fn un_champ_d_une_ligne_ne_grandit_pas() {
    let f = fenetre();
    f.set_add_account_open(true);
    for nom in ["Email address", "Password"] {
        let h = champ(&f, nom).size().height;
        assert!(h <= 40.0, "{nom} stands {h} px tall");
    }
}

// --- A row's hover buttons ---

fn les_boutons_d_une_ligne_agissent_sur_elle_sans_l_ouvrir() {
    let f = fenetre();
    f.set_rows(ModelRc::new(VecModel::from(vec![iris_ui::ThreadRowData {
        id: 7,
        from: "Agnès Joly".into(),
        subject: "Réunion de jeudi".into(),
        date: "12:05".into(),
        ..Default::default()
    }])));
    let faits = Rc::new(RefCell::new(Vec::<i32>::new()));
    let reportes = Rc::new(RefCell::new(Vec::<(i32, String)>::new()));
    let archives = Rc::new(RefCell::new(Vec::<i32>::new()));
    let ouverts = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let v = Rc::clone(&faits);
        f.on_menu_done(move |id| v.borrow_mut().push(id));
        let v = Rc::clone(&reportes);
        f.on_menu_snooze(move |id, q| v.borrow_mut().push((id, q.to_string())));
        let v = Rc::clone(&archives);
        f.on_menu_archive(move |id| v.borrow_mut().push(id));
        let v = Rc::clone(&ouverts);
        f.on_thread_selected(move |id| v.borrow_mut().push(id));
    }

    let rangee = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .find(|e| {
            e.accessible_label()
                .is_some_and(|l| l.starts_with("Agnès Joly"))
        })
        .expect("la ligne");
    // No buttons until the pointer is on the row.
    assert!(testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .iter()
        .all(|b| b.accessible_label().as_deref() != Some("Mark this conversation as done")));
    f.window().dispatch_event(WindowEvent::PointerMoved {
        position: centre(&rangee),
    });

    clic(&bouton(&f, "Mark this conversation as done"));
    clic(&bouton(&f, "Snooze this conversation until tomorrow"));
    clic(&bouton(&f, "Archive this conversation"));
    assert_eq!(*faits.borrow(), [7]);
    assert_eq!(*reportes.borrow(), [(7, "tomorrow".to_string())]);
    assert_eq!(*archives.borrow(), [7]);
    assert!(
        ouverts.borrow().is_empty(),
        "a button on the row does not open it"
    );

    // Anywhere else on the row, a click opens it.
    clic(&rangee);
    assert_eq!(*ouverts.borrow(), [7]);
}

fn clic_droit(f: &AppWindow, point: slint::LogicalPosition) {
    f.window().dispatch_event(WindowEvent::PointerPressed {
        position: point,
        button: PointerEventButton::Right,
    });
    f.window().dispatch_event(WindowEvent::PointerReleased {
        position: point,
        button: PointerEventButton::Right,
    });
    slint::platform::update_timers_and_animations();
}

fn un_clic_droit_sous_les_titres_de_jour_vise_la_bonne_ligne() {
    let f = fenetre();
    let ligne = |id: i32, de: &str, jour: &str, titres: i32| iris_ui::ThreadRowData {
        id,
        from: de.into(),
        subject: "Sujet".into(),
        date: "12:05".into(),
        day: jour.into(),
        titles: titres,
        ..Default::default()
    };
    f.set_rows(ModelRc::new(VecModel::from(vec![
        ligne(7, "Agnès Joly", "Today", 1),
        ligne(8, "Bruno Petit", "Yesterday", 2),
        ligne(9, "Carole Roy", "", 2),
        ligne(10, "Denis Vidal", "Earlier", 3),
    ])));
    let rangee = |nom: &str| {
        testing::ElementQuery::from_root(&f)
            .match_descendants()
            .match_accessible_role(testing::AccessibleRole::ListItem)
            .find_all()
            .into_iter()
            .find(|e| e.accessible_label().is_some_and(|l| l.starts_with(nom)))
            .expect("la ligne")
    };
    let bord = |nom: &str| {
        let r = rangee(nom);
        let (pos, taille) = (r.absolute_position(), r.size());
        slint::LogicalPosition::new(pos.x + 40.0, pos.y + taille.height / 2.0)
    };

    clic_droit(&f, centre(&rangee("Carole Roy")));
    assert_eq!(f.get_context_menu_thread(), 9);

    // The menu is open: a right click on another row goes through its veil, and the
    // list finds the row from the point, day titles counted.
    clic_droit(&f, bord("Denis Vidal"));
    assert_eq!(
        f.get_context_menu_thread(),
        10,
        "the row under three titles"
    );
    clic_droit(&f, bord("Bruno Petit"));
    assert_eq!(f.get_context_menu_thread(), 8, "the row under two titles");
}

fn un_clic_dans_les_taches_n_atteint_pas_le_courrier_dessous() {
    let f = fenetre();
    // Where New message is, in the mail.
    let nouveau = bouton(&f, "Write a new message");
    let point = centre(&nouveau);
    for ecran in [2, 1, 3] {
        f.set_workspace(ecran);
        // Home has a New message of its own in its toolbar: a click on it is meant
        // to write.
        if ecran == 3 {
            let propre = bouton(&f, "New message");
            let (p, t) = (propre.absolute_position(), propre.size());
            if point.x >= p.x
                && point.x < p.x + t.width
                && point.y >= p.y
                && point.y < p.y + t.height
            {
                continue;
            }
        }
        cliquer_a(&f, point);
        assert!(
            !f.get_compose_open(),
            "workspace {ecran}: the click reached New message under it"
        );
    }
    // Back in the mail, the button is there and works.
    f.set_workspace(0);
    cliquer_a(&f, point);
    assert!(f.get_compose_open());
}

fn la_palette_montre_tout_puis_filtre_a_chaque_touche() {
    // Ctrl+K opened on an empty panel and typing changed nothing: what was typed never
    // reached the list.
    let f = fenetre();
    let demandes = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let v = Rc::clone(&demandes);
        f.on_palette_query_changed(move |q| v.borrow_mut().push(q.to_string()));
    }
    ctrl(&f, "k");
    assert!(f.get_palette_open());
    assert_eq!(
        demandes.borrow().first().map(String::as_str),
        Some(""),
        "opened, the whole list is asked for"
    );
    taper(&f, "ar");
    assert_eq!(
        demandes.borrow().last().map(String::as_str),
        Some("ar"),
        "each key asks again"
    );
}

fn un_objectif_se_renomme_par_son_titre_et_se_modifie_au_stylo() {
    let f = fenetre();
    f.set_workspace(2);
    f.set_task_page(1);
    f.set_goal(iris_ui::GoalData {
        id: 3,
        title: "Send 10 applications".into(),
        target: 10,
        ..Default::default()
    });
    let noms = Rc::new(RefCell::new(Vec::<String>::new()));
    let modifs = Rc::new(RefCell::new(0));
    {
        let v = Rc::clone(&noms);
        f.on_goal_renamed(move |t| v.borrow_mut().push(t.to_string()));
        let v = Rc::clone(&modifs);
        f.on_goal_edit_requested(move || *v.borrow_mut() += 1);
    }

    // A click on the title types over it: the old one selected, the first key the
    // new one's.
    clic(&bouton(&f, "Rename Send 10 applications"));
    assert!(champ(&f, "Goal title")
        .accessible_value()
        .is_some_and(|v| v == "Send 10 applications"));
    taper(&f, "Send 12 applications\n");
    assert_eq!(*noms.borrow(), ["Send 12 applications"]);
    assert!(!champ_existe(&f, "Goal title"), "Enter closes the field");

    // Escape leaves the old title.
    clic(&bouton(&f, "Rename Send 10 applications"));
    taper(&f, "Something else");
    echap(&f);
    assert_eq!(noms.borrow().len(), 1);
    assert!(!champ_existe(&f, "Goal title"));

    // Leaving the field keeps what was typed, as Enter does; here by the pencil, which
    // then opens every field.
    clic(&bouton(&f, "Rename Send 10 applications"));
    taper(&f, "Send 15 applications");
    clic(&bouton(&f, "Edit goal"));
    assert_eq!(
        noms.borrow().last().map(String::as_str),
        Some("Send 15 applications")
    );
    assert!(!champ_existe(&f, "Goal title"));
    assert_eq!(*modifs.borrow(), 1);
}

fn un_profil_s_importe_depuis_l_ajout_de_compte() {
    let f = fenetre();
    let demandes = Rc::new(RefCell::new(0));
    {
        let d = Rc::clone(&demandes);
        f.on_add_account_import(move || *d.borrow_mut() += 1);
    }
    f.set_add_account_open(true);
    clic(&bouton(
        &f,
        "Import a configuration profile (.mobileconfig)",
    ));
    assert_eq!(*demandes.borrow(), 1);
    assert!(f.get_add_account_open(), "the window stays, to be checked");

    // Not when editing an account: its servers are already known.
    f.set_add_account_editing(true);
    assert!(testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .iter()
        .all(|b| b.accessible_label().as_deref()
            != Some("Import a configuration profile (.mobileconfig)")));
}

fn echap_quitte_la_lecture_en_plein_ecran() {
    let f = fenetre();
    f.set_conversation_empty(false);
    clic(&bouton(&f, "Read full screen"));
    assert!(f.get_reading_focus());
    echap(&f);
    assert!(!f.get_reading_focus(), "Escape brings the columns back");
}

// --- An event typed straight into the week ---

fn une_semaine(f: &AppWindow) {
    un_calendrier(f, false);
    f.set_calendar_mode(1);
    f.set_calendar_week_days(ModelRc::new(VecModel::from(
        (0..7)
            .map(|i| iris_ui::WeekDayData {
                name: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i].into(),
                day: (28 + i).to_string().into(),
                date: format!("2026-09-{:02}", 28 + i).into(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
    )));
}

/// A point in Monday's column, in the hours shown when the week opens.
fn un_creneau_du_lundi(f: &AppWindow) -> slint::LogicalPosition {
    let precedent = bouton(f, "Previous");
    // The grid starts under the header, the day names and the all-day row; Monday's
    // column starts after the side column and the hours' margin.
    slint::LogicalPosition::new(
        precedent.absolute_position().x - 620.0 + 80.0,
        precedent.absolute_position().y + 260.0,
    )
}

fn cliquer_a(f: &AppWindow, position: slint::LogicalPosition) {
    f.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    f.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    f.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

/// Presses at `de`, moves the pointer in steps to `a`, lets go there.
fn glisser(f: &AppWindow, de: slint::LogicalPosition, a: slint::LogicalPosition) {
    f.window()
        .dispatch_event(WindowEvent::PointerMoved { position: de });
    f.window().dispatch_event(WindowEvent::PointerPressed {
        position: de,
        button: PointerEventButton::Left,
    });
    for k in 1..=6 {
        let t = k as f32 / 6.0;
        f.window().dispatch_event(WindowEvent::PointerMoved {
            position: slint::LogicalPosition::new(de.x + (a.x - de.x) * t, de.y + (a.y - de.y) * t),
        });
    }
    f.window().dispatch_event(WindowEvent::PointerReleased {
        position: a,
        button: PointerEventButton::Left,
    });
    slint::platform::update_timers_and_animations();
}

fn un_evenement_se_glisse_et_s_etire_sans_s_ouvrir() {
    let f = fenetre();
    une_semaine(&f);
    let evenement = |cle: &str, titre: &str, jour: i32, movable: bool| iris_ui::TimedEventData {
        key: cle.into(),
        title: titre.into(),
        time: "09:00 – 10:00".into(),
        day: jour,
        lanes: 1,
        top: 540.0 / 1440.0,
        height: 60.0 / 1440.0,
        movable,
        ..Default::default()
    };
    f.set_calendar_week_events(ModelRc::new(VecModel::from(vec![
        evenement("5:0", "Atelier", 0, true),
        evenement("6:0", "Match", 1, false),
    ])));
    let deplaces = Rc::new(RefCell::new(Vec::<(String, i32, i32)>::new()));
    let etires = Rc::new(RefCell::new(Vec::<(String, i32)>::new()));
    let en_cours = Rc::new(RefCell::new(Vec::<(String, i32)>::new()));
    let ouverts = Rc::new(RefCell::new(0));
    {
        let v = Rc::clone(&deplaces);
        f.on_calendar_event_moved(move |k, d, m| v.borrow_mut().push((k.to_string(), d, m)));
        let v = Rc::clone(&etires);
        f.on_calendar_event_resized(move |k, m| v.borrow_mut().push((k.to_string(), m)));
        let v = Rc::clone(&en_cours);
        f.on_calendar_event_stretching(move |k, m| v.borrow_mut().push((k.to_string(), m)));
        let v = Rc::clone(&ouverts);
        f.on_calendar_event_opened(move |_| *v.borrow_mut() += 1);
    }
    let bloc = |titre: &str| {
        testing::ElementQuery::from_root(&f)
            .match_descendants()
            .match_accessible_role(testing::AccessibleRole::Button)
            .find_all()
            .into_iter()
            .find(|b| b.accessible_label().is_some_and(|l| l.ends_with(titre)))
            .expect("l'événement")
    };

    // An hour lower (52 px an hour): moved by 60 minutes, the same day, not opened.
    let b = bloc("Atelier");
    let (pos, taille) = (b.absolute_position(), b.size());
    let milieu = slint::LogicalPosition::new(pos.x + taille.width / 2.0, pos.y + 12.0);
    glisser(
        &f,
        milieu,
        slint::LogicalPosition::new(milieu.x, milieu.y + 52.0),
    );
    assert_eq!(*deplaces.borrow(), [("5:0".to_string(), 0, 60)]);
    assert_eq!(*ouverts.borrow(), 0, "a drag is not a click");

    // By its lower edge, half an hour further: stretched by 30 minutes.
    let b = bloc("Atelier");
    let (pos, taille) = (b.absolute_position(), b.size());
    let bord = slint::LogicalPosition::new(pos.x + taille.width / 2.0, pos.y + taille.height - 2.0);
    glisser(&f, bord, slint::LogicalPosition::new(bord.x, bord.y + 26.0));
    assert_eq!(*etires.borrow(), [("5:0".to_string(), 30)]);
    // While it was stretched, the week was asked to follow it, not only at the end.
    assert!(
        en_cours.borrow().contains(&("5:0".to_string(), 30)),
        "stretching is reported as it goes: {:?}",
        en_cours.borrow()
    );

    // One that cannot move (a subscription, a repeat) stays put.
    let b = bloc("Match");
    let (pos, taille) = (b.absolute_position(), b.size());
    let milieu = slint::LogicalPosition::new(pos.x + taille.width / 2.0, pos.y + 12.0);
    glisser(
        &f,
        milieu,
        slint::LogicalPosition::new(milieu.x, milieu.y + 52.0),
    );
    assert_eq!(deplaces.borrow().len(), 1);

    // And a plain click still opens it.
    clic(&bloc("Atelier"));
    assert_eq!(*ouverts.borrow(), 1);
}

fn la_fiche_d_un_evenement_s_ouvre_a_cote_de_lui() {
    let f = fenetre();
    une_semaine(&f);
    f.set_calendar_week_events(ModelRc::new(VecModel::from(vec![
        iris_ui::TimedEventData {
            key: "5:0".into(),
            title: "Atelier".into(),
            time: "09:00 – 10:00".into(),
            day: 2,
            lanes: 1,
            top: 540.0 / 1440.0,
            height: 60.0 / 1440.0,
            ..Default::default()
        },
    ])));
    let bloc = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().is_some_and(|l| l.ends_with("Atelier")))
        .expect("l'événement");
    let (pos, taille) = (bloc.absolute_position(), bloc.size());
    cliquer_a(
        &f,
        slint::LogicalPosition::new(pos.x + taille.width / 2.0, pos.y + 10.0),
    );
    // The card is placed from these: the event where it is on screen, the grid's
    // scroll counted.
    assert!(f.get_event_anchored());
    assert!(
        (f.get_event_anchor_top() - pos.y).abs() < 2.0,
        "anchor top {} for an event at {}",
        f.get_event_anchor_top(),
        pos.y
    );
    assert!((f.get_event_anchor_left() - pos.x).abs() < 2.0);
    assert!((f.get_event_anchor_right() - (pos.x + taille.width)).abs() < 2.0);
}

fn un_clic_droit_sur_un_evenement_ouvre_son_menu_qui_garde_le_clavier() {
    let f = fenetre();
    une_semaine(&f);
    f.set_calendar_week_events(ModelRc::new(VecModel::from(vec![
        iris_ui::TimedEventData {
            key: "5:0".into(),
            title: "Atelier".into(),
            time: "09:00 – 10:00".into(),
            day: 1,
            lanes: 1,
            top: 540.0 / 1440.0,
            height: 60.0 / 1440.0,
            movable: true,
            ..Default::default()
        },
    ])));
    f.set_calendar_palette(ModelRc::new(VecModel::from(vec![
        slint::Color::from_rgb_u8(0x5b, 0x8d, 0xef),
        slint::Color::from_rgb_u8(0xe0, 0x79, 0x5b),
        slint::Color::from_rgb_u8(0x4f, 0xb2, 0x86),
    ])));
    let demandes = Rc::new(RefCell::new(Vec::<String>::new()));
    let couleurs = Rc::new(RefCell::new(Vec::<i32>::new()));
    let touches = Rc::new(RefCell::new(0));
    {
        let (v, fw) = (Rc::clone(&demandes), f.as_weak());
        f.on_calendar_event_menu(move |k, x, y| {
            v.borrow_mut().push(k.to_string());
            // As Rust does: the event chosen, the menu opened where asked.
            let f = fw.upgrade().unwrap();
            f.set_event_detail(iris_ui::EventDetailData {
                key: k,
                title: "Atelier".into(),
                editable: true,
                ..Default::default()
            });
            f.set_event_menu_x(x);
            f.set_event_menu_y(y);
            f.set_event_menu_open(true);
        });
        let v = Rc::clone(&couleurs);
        f.on_event_color_chosen(move |i| v.borrow_mut().push(i));
        let v = Rc::clone(&touches);
        f.on_key_pressed(move |_| *v.borrow_mut() += 1);
    }
    let bloc = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().is_some_and(|l| l.ends_with("Atelier")))
        .expect("l'événement");
    clic_droit(&f, centre(&bloc));
    assert_eq!(*demandes.borrow(), ["5:0"]);
    assert!(f.get_event_menu_open());
    // Near the pointer, not somewhere down the scrolled day.
    assert!((f.get_event_menu_y() - centre(&bloc).y).abs() < 2.0);

    // An open menu owns the keyboard; a swatch gives the event its colour.
    taper(&f, "t");
    assert_eq!(*touches.borrow(), 0);
    clic(&bouton(&f, "Event colour 3"));
    assert_eq!(*couleurs.borrow(), [2]);
    assert!(!f.get_event_menu_open());

    // Escape closes it.
    clic_droit(&f, centre(&bloc));
    echap(&f);
    assert!(!f.get_event_menu_open());
}

fn une_tache_se_pose_sur_la_semaine() {
    let f = fenetre();
    une_semaine(&f);
    f.set_calendar_to_plan(ModelRc::new(VecModel::from(vec![iris_ui::PlanTaskData {
        id: 42,
        title: "Relire le devis".into(),
        meta: "45 min".into(),
        minutes: 45,
        ..Default::default()
    }])));
    let poses = Rc::new(RefCell::new(Vec::<(i32, String, i32)>::new()));
    {
        let v = Rc::clone(&poses);
        f.on_calendar_task_planned(move |id, d, m| v.borrow_mut().push((id, d.to_string(), m)));
    }
    let tache = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::ListItem)
        .find_all()
        .into_iter()
        .find(|e| e.accessible_label().as_deref() == Some("Relire le devis"))
        .expect("la tâche à planifier");
    // A mark at 10:00 on Wednesday, to aim at: let go on its top edge, the task lands
    // at 10:00 that day, the grid's scroll counted.
    f.set_calendar_week_events(ModelRc::new(VecModel::from(vec![
        iris_ui::TimedEventData {
            key: "1:0".into(),
            title: "Repère".into(),
            time: "10:00 – 11:00".into(),
            day: 2,
            lanes: 1,
            top: 600.0 / 1440.0,
            height: 60.0 / 1440.0,
            ..Default::default()
        },
    ])));
    let repere = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().is_some_and(|l| l.ends_with("Repère")))
        .expect("le repère");
    let pos = repere.absolute_position();
    glisser(
        &f,
        centre(&tache),
        slint::LogicalPosition::new(pos.x + 10.0, pos.y + 3.0),
    );
    let poses = poses.borrow();
    assert_eq!(poses.len(), 1, "let go on the week, it is booked there");
    assert_eq!(poses[0], (42, "2026-09-30".to_string(), 600));
}

fn un_clic_sur_un_creneau_libre_s_ecrit_directement() {
    let f = fenetre();
    une_semaine(&f);
    let crees = Rc::new(RefCell::new(Vec::<(String, i32, String)>::new()));
    let touches = Rc::new(RefCell::new(0));
    {
        let c = Rc::clone(&crees);
        f.on_calendar_quick_event(move |d, m, t| {
            c.borrow_mut().push((d.to_string(), m, t.to_string()))
        });
        let t = Rc::clone(&touches);
        f.on_key_pressed(move |_| *t.borrow_mut() += 1);
    }

    cliquer_a(&f, un_creneau_du_lundi(&f));
    // Rule 3: the first key goes to the title, "t" does not jump to today.
    taper(&f, "Dentist\n");
    let crees = crees.borrow();
    assert_eq!(crees.len(), 1, "Enter saves the event");
    assert_eq!(crees[0].0, "2026-09-28");
    assert_eq!(crees[0].2, "Dentist");
    assert_eq!(crees[0].1 % 30, 0, "on a half hour");
    assert_eq!(*touches.borrow(), 0, "no key reached the shortcuts");
}

fn echap_abandonne_le_creneau_tape() {
    let f = fenetre();
    une_semaine(&f);
    let crees = Rc::new(RefCell::new(0));
    {
        let c = Rc::clone(&crees);
        f.on_calendar_quick_event(move |_, _, _| *c.borrow_mut() += 1);
    }
    cliquer_a(&f, un_creneau_du_lundi(&f));
    assert!(champ_existe(&f, "New event title"));
    taper(&f, "Oops");
    echap(&f);
    assert!(
        !champ_existe(&f, "New event title"),
        "Escape drops the line"
    );
    assert_eq!(*crees.borrow(), 0);
}

// --- Goals ---

fn un_nouvel_objectif_s_ecrit_des_l_ouverture() {
    let f = fenetre();
    f.set_workspace(2);
    let touches = Rc::new(RefCell::new(0));
    let crees = Rc::new(RefCell::new(0));
    {
        let t = Rc::clone(&touches);
        f.on_key_pressed(move |_| *t.borrow_mut() += 1);
        let fw = f.as_weak();
        f.on_goal_new_requested(move || fw.upgrade().unwrap().set_goal_new_open(true));
        let c = Rc::clone(&crees);
        f.on_goal_new_confirmed(move || *c.borrow_mut() += 1);
    }
    f.invoke_goal_new_requested();
    // Rule 3: the first key is the goal's; rule 2: none reaches the shortcuts.
    taper(&f, "Run 100 km\n");
    assert_eq!(f.get_goal_new_title(), "Run 100 km");
    assert_eq!(*crees.borrow(), 1, "Enter creates it");
    assert_eq!(*touches.borrow(), 0);
    echap(&f);
    assert!(!f.get_goal_new_open(), "Escape closes the window");
}

fn faire_du_temps_s_ouvre_dans_l_heure() {
    let f = fenetre();
    f.set_workspace(2);
    f.set_task_page(1);
    f.set_goal_time_start("18:00".into());
    f.set_goal_time_open(true);
    // Opened selected: the first key replaces the hour.
    taper(&f, "7:30");
    assert_eq!(f.get_goal_time_start(), "7:30");
    let jours = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let j = Rc::clone(&jours);
        f.on_goal_time_day_toggled(move |i| j.borrow_mut().push(i));
    }
    clic(&case_a_cocher(&f, "Wed"));
    assert_eq!(*jours.borrow(), [2], "one click on a day toggles it");
    echap(&f);
    assert!(!f.get_goal_time_open());
}

// --- When to do a task ---

fn une_tache_ouverte(f: &AppWindow) {
    f.set_workspace(2);
    une_tache(f);
    f.set_task_detail(iris_ui::TaskDetailData {
        id: 5,
        ..Default::default()
    });
    f.set_task_has_detail(true);
}

fn l_remet_a_plus_tard_et_le_menu_garde_le_clavier() {
    let f = fenetre();
    une_tache_ouverte(&f);
    let demandes = Rc::new(RefCell::new(Vec::<i32>::new()));
    let choix = Rc::new(RefCell::new(Vec::<String>::new()));
    let touches = Rc::new(RefCell::new(0));
    {
        let fw = f.as_weak();
        let d = Rc::clone(&demandes);
        f.on_task_later_requested(move |id| {
            d.borrow_mut().push(id);
            fw.upgrade().unwrap().set_task_later_open(true);
        });
        let c = Rc::clone(&choix);
        f.on_task_later_chosen(move |q| c.borrow_mut().push(q.to_string()));
        let t = Rc::clone(&touches);
        f.on_task_toggle_selected(move || *t.borrow_mut() += 1);
    }
    taper(&f, "l");
    assert_eq!(*demandes.borrow(), [5], "L puts off the task shown");
    assert!(f.get_task_later_open());
    taper(&f, " ");
    assert_eq!(
        *touches.borrow(),
        0,
        "the space bar does not tick the task behind"
    );
    echap(&f);
    assert!(!f.get_task_later_open(), "Escape closes it");

    taper(&f, "l");
    clic(&bouton(&f, "This weekend"));
    assert_eq!(*choix.borrow(), ["weekend"]);
    assert!(!f.get_task_later_open(), "a choice closes it");
}

fn la_duree_se_choisit_d_un_clic() {
    let f = fenetre();
    une_tache_ouverte(&f);
    let durees = Rc::new(RefCell::new(Vec::<i32>::new()));
    {
        let d = Rc::clone(&durees);
        f.on_task_estimate_chosen(move |m| d.borrow_mut().push(m));
    }
    clic(&case_a_cocher(&f, "1 h"));
    assert_eq!(*durees.borrow(), [60]);
}

fn case_a_cocher(f: &AppWindow, libelle: &str) -> testing::ElementHandle {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Checkbox)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().as_deref() == Some(libelle))
        .unwrap_or_else(|| panic!("aucune case nommée « {libelle} »"))
}

fn champ_existe(f: &AppWindow, libelle: &str) -> bool {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::TextInput)
        .find_all()
        .iter()
        .any(|c| c.accessible_label().as_deref() == Some(libelle))
}

fn le_menu_de_report_garde_le_clavier() {
    // An open menu owns the keyboard: "e" does not mark the thread behind it as done,
    // and Escape closes it.
    let f = fenetre();
    f.set_conversation_empty(false);
    let touches = Rc::new(RefCell::new(0));
    {
        let t = Rc::clone(&touches);
        f.on_key_pressed(move |_| *t.borrow_mut() += 1);
    }
    clic(&bouton(&f, "Snooze…"));
    assert_eq!(f.get_reader_menu(), "snooze");
    taper(&f, "e");
    assert_eq!(*touches.borrow(), 0);
    echap(&f);
    assert_eq!(f.get_reader_menu(), "");
}

fn un_nouveau_dossier_s_ouvre_dans_son_nom_et_choisit_ses_boites() {
    let f = fenetre();
    f.set_new_folder_mailboxes(ModelRc::new(VecModel::from(vec![
        iris_ui::FolderMailboxData {
            id: 1,
            name: "Work".into(),
            email: "a@example.com".into(),
            checked: true,
            ..Default::default()
        },
        iris_ui::FolderMailboxData {
            id: 2,
            name: "b@example.com".into(),
            email: "b@example.com".into(),
            ..Default::default()
        },
    ])));
    f.set_new_folder_total(2);
    f.set_new_folder_chosen(1);
    let coches = Rc::new(RefCell::new(Vec::<i32>::new()));
    let tous = Rc::new(RefCell::new(0));
    let cherches = Rc::new(RefCell::new(Vec::<String>::new()));
    let fermes = Rc::new(RefCell::new(0));
    let touches = Rc::new(RefCell::new(0));
    {
        let c = Rc::clone(&coches);
        f.on_new_folder_mailbox_toggled(move |id| c.borrow_mut().push(id));
        let t = Rc::clone(&tous);
        f.on_new_folder_select_all_toggled(move || *t.borrow_mut() += 1);
        let c = Rc::clone(&cherches);
        f.on_new_folder_search_changed(move |t| c.borrow_mut().push(t.to_string()));
        let fe = Rc::clone(&fermes);
        f.on_new_folder_dismissed(move || *fe.borrow_mut() += 1);
        let t = Rc::clone(&touches);
        f.on_key_pressed(move |_| *t.borrow_mut() += 1);
    }
    f.set_new_folder_open(true);

    taper(&f, "Devis");
    assert_eq!(
        f.get_new_folder_name().as_str(),
        "Devis",
        "the first key writes the name"
    );

    clic(&par_role(
        &f,
        testing::AccessibleRole::Checkbox,
        "b@example.com",
    ));
    assert_eq!(*coches.borrow(), [2], "one click ticks the mailbox");
    clic(&par_role(
        &f,
        testing::AccessibleRole::Checkbox,
        "Select all",
    ));
    assert_eq!(*tous.borrow(), 1);

    clic(&champ(&f, "Search mailboxes"));
    taper(&f, "b");
    assert_eq!(
        f.get_new_folder_search().as_str(),
        "b",
        "one click puts the cursor in the search"
    );
    assert_eq!(cherches.borrow().last().map(String::as_str), Some("b"));
    assert_eq!(*touches.borrow(), 0, "nothing reaches the mail behind");

    echap(&f);
    assert_eq!(*fermes.borrow(), 1, "Escape closes the window");
}

fn main() {
    testing::init_no_event_loop();

    let scenarios: Vec<(&str, fn())> = vec![
        (
            "un_nouveau_dossier_s_ouvre_dans_son_nom_et_choisit_ses_boites",
            un_nouveau_dossier_s_ouvre_dans_son_nom_et_choisit_ses_boites,
        ),
        (
            "les_boutons_d_une_ligne_agissent_sur_elle_sans_l_ouvrir",
            les_boutons_d_une_ligne_agissent_sur_elle_sans_l_ouvrir,
        ),
        (
            "un_clic_droit_sous_les_titres_de_jour_vise_la_bonne_ligne",
            un_clic_droit_sous_les_titres_de_jour_vise_la_bonne_ligne,
        ),
        (
            "un_clic_dans_les_taches_n_atteint_pas_le_courrier_dessous",
            un_clic_dans_les_taches_n_atteint_pas_le_courrier_dessous,
        ),
        (
            "un_objectif_se_renomme_par_son_titre_et_se_modifie_au_stylo",
            un_objectif_se_renomme_par_son_titre_et_se_modifie_au_stylo,
        ),
        (
            "un_evenement_se_glisse_et_s_etire_sans_s_ouvrir",
            un_evenement_se_glisse_et_s_etire_sans_s_ouvrir,
        ),
        (
            "la_fiche_d_un_evenement_s_ouvre_a_cote_de_lui",
            la_fiche_d_un_evenement_s_ouvre_a_cote_de_lui,
        ),
        (
            "un_clic_droit_sur_un_evenement_ouvre_son_menu_qui_garde_le_clavier",
            un_clic_droit_sur_un_evenement_ouvre_son_menu_qui_garde_le_clavier,
        ),
        (
            "une_tache_se_pose_sur_la_semaine",
            une_tache_se_pose_sur_la_semaine,
        ),
        (
            "un_profil_s_importe_depuis_l_ajout_de_compte",
            un_profil_s_importe_depuis_l_ajout_de_compte,
        ),
        (
            "echap_quitte_la_lecture_en_plein_ecran",
            echap_quitte_la_lecture_en_plein_ecran,
        ),
        (
            "le_menu_de_report_garde_le_clavier",
            le_menu_de_report_garde_le_clavier,
        ),
        (
            "un_clic_sur_un_creneau_libre_s_ecrit_directement",
            un_clic_sur_un_creneau_libre_s_ecrit_directement,
        ),
        (
            "echap_abandonne_le_creneau_tape",
            echap_abandonne_le_creneau_tape,
        ),
        (
            "un_nouvel_objectif_s_ecrit_des_l_ouverture",
            un_nouvel_objectif_s_ecrit_des_l_ouverture,
        ),
        (
            "faire_du_temps_s_ouvre_dans_l_heure",
            faire_du_temps_s_ouvre_dans_l_heure,
        ),
        (
            "l_remet_a_plus_tard_et_le_menu_garde_le_clavier",
            l_remet_a_plus_tard_et_le_menu_garde_le_clavier,
        ),
        (
            "la_duree_se_choisit_d_un_clic",
            la_duree_se_choisit_d_un_clic,
        ),
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
            "ctrl_4_opens_the_notes_and_escape_closes_their_menus",
            ctrl_4_opens_the_notes_and_escape_closes_their_menus,
        ),
        (
            "a_note_s_line_takes_every_key_and_enter_goes_to_rust",
            a_note_s_line_takes_every_key_and_enter_goes_to_rust,
        ),
        (
            "a_line_made_below_the_page_keeps_the_keyboard",
            a_line_made_below_the_page_keeps_the_keyboard,
        ),
        (
            "an_embedded_spreadsheet_s_cells_are_chosen_and_take_the_keys",
            an_embedded_spreadsheet_s_cells_are_chosen_and_take_the_keys,
        ),
        (
            "a_heading_is_typed_without_its_hashes",
            a_heading_is_typed_without_its_hashes,
        ),
        (
            "a_new_note_s_title_is_selected_and_typing_names_it",
            a_new_note_s_title_is_selected_and_typing_names_it,
        ),
        (
            "a_spreadsheet_takes_the_keys_and_its_menu_owns_them_until_escape",
            a_spreadsheet_takes_the_keys_and_its_menu_owns_them_until_escape,
        ),
        (
            "the_notes_panels_close_with_escape",
            the_notes_panels_close_with_escape,
        ),
        (
            "connecting_calendars_opens_in_the_address_and_escape_closes",
            connecting_calendars_opens_in_the_address_and_escape_closes,
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
            "une_tache_portee_sur_une_liste_y_va",
            une_tache_portee_sur_une_liste_y_va,
        ),
        (
            "une_tache_se_reordonne_en_la_glissant",
            une_tache_se_reordonne_en_la_glissant,
        ),
        (
            "la_palette_montre_tout_puis_filtre_a_chaque_touche",
            la_palette_montre_tout_puis_filtre_a_chaque_touche,
        ),
        (
            "les_taches_terminees_se_suppriment_d_un_clic",
            les_taches_terminees_se_suppriment_d_un_clic,
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
        (
            "un_second_clic_droit_ouvre_le_menu_de_l_autre_compte",
            un_second_clic_droit_ouvre_le_menu_de_l_autre_compte,
        ),
        (
            "new_message_rouvre_en_grand_une_fenetre_reduite",
            new_message_rouvre_en_grand_une_fenetre_reduite,
        ),
        (
            "fermer_un_message_ecrit_demande_quoi_en_faire",
            fermer_un_message_ecrit_demande_quoi_en_faire,
        ),
        (
            "cc_et_bcc_se_replient_sauf_ce_qui_est_rempli",
            cc_et_bcc_se_replient_sauf_ce_qui_est_rempli,
        ),
        (
            "les_tags_d_un_compte_se_cherchent_dans_son_menu",
            les_tags_d_un_compte_se_cherchent_dans_son_menu,
        ),
        (
            "les_tags_se_gerent_depuis_le_bas_de_la_colonne",
            les_tags_se_gerent_depuis_le_bas_de_la_colonne,
        ),
        (
            "le_regroupement_par_tag_s_allume_au_dessus_de_la_liste",
            le_regroupement_par_tag_s_allume_au_dessus_de_la_liste,
        ),
        (
            "un_nouveau_calendrier_s_ouvre_dans_son_nom",
            un_nouveau_calendrier_s_ouvre_dans_son_nom,
        ),
        (
            "le_dernier_calendrier_perso_ne_se_supprime_pas",
            le_dernier_calendrier_perso_ne_se_supprime_pas,
        ),
        (
            "le_nom_d_iris_mene_a_l_accueil_qui_garde_le_courrier_a_l_abri",
            le_nom_d_iris_mene_a_l_accueil_qui_garde_le_courrier_a_l_abri,
        ),
        (
            "l_accueil_mene_a_ce_qu_il_montre",
            l_accueil_mene_a_ce_qu_il_montre,
        ),
        (
            "retour_et_avant_aux_boutons_et_au_clavier",
            retour_et_avant_aux_boutons_et_au_clavier,
        ),
        (
            "un_tag_se_choisit_et_se_replie",
            un_tag_se_choisit_et_se_replie,
        ),
        (
            "les_tags_se_rangent_a_la_souris",
            les_tags_se_rangent_a_la_souris,
        ),
        (
            "une_tache_supprimee_revient_par_ctrl_z",
            une_tache_supprimee_revient_par_ctrl_z,
        ),
        (
            "une_tache_s_ajoute_a_un_evenement",
            une_tache_s_ajoute_a_un_evenement,
        ),
        (
            "un_champ_d_une_ligne_ne_grandit_pas",
            un_champ_d_une_ligne_ne_grandit_pas,
        ),
        (
            "a_click_on_a_painted_body_asks_for_the_link_under_it",
            a_click_on_a_painted_body_asks_for_the_link_under_it,
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
