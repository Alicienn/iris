//! The window at every width.
//!
//! The same interface runs on a PC and on a phone: below 600 pixels it lays itself out
//! in one column, from 600 to 840 in two, and from 840 as it always has. These
//! scenarios drive it the way `saisie` does — real sizes, real pointer and key events —
//! at a phone's 390 by 844, a foldable's 700 by 820 and the desktop's 1280 by 800.

use i_slint_backend_testing as testing;
use iris_ui::{AppWindow, Layout, WidthClass};
use slint::ComponentHandle;

fn fenetre_de(w: f32, h: f32) -> AppWindow {
    let f = AppWindow::new().expect("la fenêtre doit se construire");
    f.window().set_size(slint::LogicalSize::new(w, h));
    f.show().expect("la fenêtre doit s'afficher");
    f
}

fn redimensionner(f: &AppWindow, w: f32, h: f32) {
    f.window().set_size(slint::LogicalSize::new(w, h));
    slint::platform::update_timers_and_animations();
}

fn la_largeur_choisit_la_classe() {
    let f = fenetre_de(1280.0, 800.0);
    assert_eq!(f.global::<Layout>().get_width_class(), WidthClass::Expanded);
    redimensionner(&f, 700.0, 820.0);
    assert_eq!(f.global::<Layout>().get_width_class(), WidthClass::Medium);
    redimensionner(&f, 390.0, 844.0);
    assert_eq!(f.global::<Layout>().get_width_class(), WidthClass::Compact);
    assert!(f.global::<Layout>().get_compact());
    let f = fenetre_de(390.0, 844.0);
    assert_eq!(
        f.global::<Layout>().get_width_class(),
        WidthClass::Compact,
        "a window opened narrow knows it at once"
    );
}

fn redimensionner_garde_le_composer_ouvert() {
    let f = fenetre_de(1280.0, 800.0);
    f.set_compose_open(true);
    f.set_compose_to("marie@example.com".into());
    f.set_compose_body("Bonjour".into());
    redimensionner(&f, 390.0, 844.0);
    assert!(
        f.get_compose_open(),
        "a narrower window keeps the message open"
    );
    assert_eq!(f.get_compose_body(), "Bonjour");
    redimensionner(&f, 1280.0, 800.0);
    assert!(f.get_compose_open());
    assert_eq!(f.get_compose_to(), "marie@example.com");
}

fn element(f: &AppWindow, role: testing::AccessibleRole, libelle: &str) -> testing::ElementHandle {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(role)
        .find_all()
        .into_iter()
        .find(|b| b.accessible_label().as_deref() == Some(libelle))
        .unwrap_or_else(|| panic!("nothing named « {libelle} »"))
}

fn un_formulaire_remplit_l_ecran_etroit_et_une_question_monte_du_bas() {
    let f = fenetre_de(390.0, 844.0);
    f.set_add_account_open(true);
    slint::platform::update_timers_and_animations();
    let adresse = element(&f, testing::AccessibleRole::TextInput, "Email address");
    // A card would be inset and no wider than 440: a page puts the field at the
    // screen's margin, the whole width less the margins.
    assert!(
        adresse.absolute_position().x < 32.0 && adresse.size().width > 390.0 - 64.0,
        "a form is a page the width of the screen: field at {:?}, {:?}",
        adresse.absolute_position(),
        adresse.size()
    );
    assert!(f.get_modal_open());
    f.set_add_account_open(false);

    f.set_empty_folder_open(true);
    slint::platform::update_timers_and_animations();
    let vider = element(&f, testing::AccessibleRole::Button, "Empty");
    assert!(
        vider.absolute_position().y > 844.0 / 2.0,
        "a question rises from the bottom: its button is in the lower half"
    );
    assert!(f.get_modal_open());

    redimensionner(&f, 1280.0, 800.0);
    assert!(
        f.get_empty_folder_open(),
        "widened, it stays open as a card"
    );
    let vider = element(&f, testing::AccessibleRole::Button, "Empty");
    assert!(vider.absolute_position().x > 400.0, "centred card");
}

fn les_lieux_sont_en_bas_sur_un_telephone_et_a_gauche_sur_un_pliable() {
    let f = fenetre_de(390.0, 844.0);
    let agenda = element(&f, testing::AccessibleRole::Tab, "Calendar");
    assert!(
        agenda.absolute_position().y > 760.0,
        "the tab bar is at the foot: {:?}",
        agenda.absolute_position()
    );
    agenda.mock_single_click(slint::platform::PointerEventButton::Left);
    slint::platform::update_timers_and_animations();
    assert_eq!(f.get_workspace(), 1, "a tab opens its place");

    let f = fenetre_de(700.0, 820.0);
    let agenda = element(&f, testing::AccessibleRole::Tab, "Calendar");
    assert!(
        agenda.absolute_position().x < 80.0,
        "the rail is at the left"
    );
}

fn sur_un_telephone_la_loupe_ouvre_la_recherche_sur_toute_la_barre() {
    let f = fenetre_de(390.0, 844.0);
    element(&f, testing::AccessibleRole::Button, "Search")
        .mock_single_click(slint::platform::PointerEventButton::Left);
    slint::platform::update_timers_and_animations();
    let champs: Vec<_> = testing::ElementQuery::from_root(&f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::TextInput)
        .find_all()
        .into_iter()
        .filter(|e| e.accessible_label().as_deref() == Some("Search"))
        .map(|e| (e.absolute_position(), e.size()))
        .collect();
    assert!(
        // From the left edge to the window's own buttons (a narrow PC window).
        champs
            .iter()
            .any(|(p, s)| p.x < 40.0 && s.width > 150.0 && p.y < 120.0),
        "a field spans the bar at the top: {champs:?}"
    );
}

fn le_retour_d_android_est_remis_a_l_application() {
    let f = fenetre_de(390.0, 844.0);
    let retours = std::rc::Rc::new(std::cell::Cell::new(0));
    {
        let r = std::rc::Rc::clone(&retours);
        f.on_system_back(move || r.set(r.get() + 1));
    }
    let retour = slint::SharedString::from(slint::platform::Key::Back);
    f.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed {
            text: retour.clone(),
        });
    f.window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased { text: retour });
    assert_eq!(retours.get(), 1, "Back goes to nav::system_back");
}

/// The buttons of that name actually laid out (wider than nothing).
fn boutons_vus(f: &AppWindow, libelle: &str) -> Vec<(slint::LogicalPosition, slint::LogicalSize)> {
    testing::ElementQuery::from_root(f)
        .match_descendants()
        .match_accessible_role(testing::AccessibleRole::Button)
        .find_all()
        .into_iter()
        .filter(|b| b.accessible_label().as_deref() == Some(libelle) && b.size().width > 0.0)
        .map(|b| (b.absolute_position(), b.size()))
        .collect()
}

fn sur_un_telephone_une_tache_ouverte_est_une_page() {
    let f = fenetre_de(390.0, 844.0);
    f.set_workspace(2);
    f.set_task_has_detail(true);
    slint::platform::update_timers_and_animations();
    // The task's own button sits at the right of a panel as wide as the screen.
    let plus_tard = boutons_vus(&f, "Move it later");
    assert!(
        plus_tard.iter().any(|(p, _)| p.x > 250.0),
        "the task fills the screen: {plus_tard:?}"
    );
    // Closed: the list is back.
    f.set_task_has_detail(false);
    slint::platform::update_timers_and_animations();
    assert!(boutons_vus(&f, "Move it later").is_empty());
}

fn sur_un_telephone_une_habitude_ouverte_est_une_page() {
    let f = fenetre_de(390.0, 844.0);
    f.set_workspace(2);
    f.set_tasks_habits(true);
    f.set_habits_has_detail(true);
    slint::platform::update_timers_and_animations();
    let modifier = boutons_vus(&f, "Edit habit");
    assert!(
        modifier.iter().any(|(p, _)| p.x > 250.0),
        "the habit fills the screen: {modifier:?}"
    );
}

fn sur_un_ecran_tactile_une_note_a_sa_barre_de_mise_en_forme() {
    let f = fenetre_de(390.0, 844.0);
    f.global::<Layout>().set_touch_input(true);
    f.set_workspace(4);
    slint::platform::update_timers_and_animations();
    let _ = boutons_vus(&f, "Bold");
    f.set_note_open(true);
    f.set_note_reading(false);
    // Opening a note names it: the tree steps aside.
    f.set_note_title("Chapitre 2".into());
    let touches = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    {
        let t = std::rc::Rc::clone(&touches);
        f.on_note_canvas_key(move |k| {
            t.borrow_mut().push(k.to_string());
            true
        });
    }
    slint::platform::update_timers_and_animations();
    element(&f, testing::AccessibleRole::Button, "Bold")
        .mock_single_click(slint::platform::PointerEventButton::Left);
    assert_eq!(
        *touches.borrow(),
        ["ctrl+b"],
        "the same name as the shortcut"
    );

    // Reading: no bar.
    f.set_note_reading(true);
    slint::platform::update_timers_and_animations();
    assert!(boutons_vus(&f, "Bold").is_empty());
}

fn sans_compte_iris_s_ouvre_sur_la_bienvenue() {
    let f = fenetre_de(390.0, 844.0);
    f.set_first_run(true);
    slint::platform::update_timers_and_animations();
    assert!(f.get_modal_open(), "the welcome page owns the keyboard");
    element(
        &f,
        testing::AccessibleRole::Button,
        "Add your first account",
    )
    .mock_single_click(slint::platform::PointerEventButton::Left);
    assert!(!f.get_first_run());
    assert!(f.get_add_account_open(), "the account setup opens");

    let f = fenetre_de(390.0, 844.0);
    f.set_first_run(true);
    element(
        &f,
        testing::AccessibleRole::Button,
        "Set up from another device",
    )
    .mock_single_click(slint::platform::PointerEventButton::Left);
    slint::platform::update_timers_and_animations();
    assert!(f.get_settings_open());
    assert_eq!(f.get_settings_group(), "Devices", "straight to Devices");
    assert!(!boutons_vus(&f, "Pair a device…").is_empty());
}

fn sur_un_telephone_les_reglages_sont_des_pages() {
    let f = fenetre_de(390.0, 844.0);
    f.set_settings_open(true);
    slint::platform::update_timers_and_animations();
    // The list of groups, none of their settings yet.
    assert!(
        !boutons_vus(&f, "Appearance").is_empty(),
        "the groups are listed"
    );
    assert!(
        boutons_vus(&f, "Blue accent").is_empty(),
        "no setting on the list"
    );

    let apparence = element(&f, testing::AccessibleRole::Button, "Appearance");
    apparence.mock_single_click(slint::platform::PointerEventButton::Left);
    slint::platform::update_timers_and_animations();
    assert_eq!(f.get_settings_group(), "Appearance");
    assert!(
        !boutons_vus(&f, "Blue accent").is_empty(),
        "the group's page"
    );
    assert!(!boutons_vus(&f, "Back to settings").is_empty());

    // Escape (Android's Back) goes back to the list, then closes.
    let echap = slint::SharedString::from(slint::platform::Key::Escape);
    for _ in 0..2 {
        f.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: echap.clone(),
            });
        f.window()
            .dispatch_event(slint::platform::WindowEvent::KeyReleased {
                text: echap.clone(),
            });
        if f.get_settings_group().is_empty() {
            break;
        }
    }
    assert_eq!(f.get_settings_group(), "");
    assert!(
        f.get_settings_open(),
        "the first Escape went back to the list"
    );

    // A wide window shows every group at once, as before.
    let large = fenetre_de(1280.0, 800.0);
    large.set_settings_open(true);
    assert!(!boutons_vus(&large, "Blue accent").is_empty());
}

fn main() {
    testing::init_no_event_loop();

    let scenarios: Vec<(&str, fn())> = vec![
        ("la_largeur_choisit_la_classe", la_largeur_choisit_la_classe),
        (
            "redimensionner_garde_le_composer_ouvert",
            redimensionner_garde_le_composer_ouvert,
        ),
        (
            "un_formulaire_remplit_l_ecran_etroit_et_une_question_monte_du_bas",
            un_formulaire_remplit_l_ecran_etroit_et_une_question_monte_du_bas,
        ),
        (
            "les_lieux_sont_en_bas_sur_un_telephone_et_a_gauche_sur_un_pliable",
            les_lieux_sont_en_bas_sur_un_telephone_et_a_gauche_sur_un_pliable,
        ),
        (
            "sur_un_telephone_la_loupe_ouvre_la_recherche_sur_toute_la_barre",
            sur_un_telephone_la_loupe_ouvre_la_recherche_sur_toute_la_barre,
        ),
        (
            "le_retour_d_android_est_remis_a_l_application",
            le_retour_d_android_est_remis_a_l_application,
        ),
        (
            "sur_un_telephone_une_tache_ouverte_est_une_page",
            sur_un_telephone_une_tache_ouverte_est_une_page,
        ),
        (
            "sur_un_telephone_une_habitude_ouverte_est_une_page",
            sur_un_telephone_une_habitude_ouverte_est_une_page,
        ),
        (
            "sur_un_ecran_tactile_une_note_a_sa_barre_de_mise_en_forme",
            sur_un_ecran_tactile_une_note_a_sa_barre_de_mise_en_forme,
        ),
        (
            "sans_compte_iris_s_ouvre_sur_la_bienvenue",
            sans_compte_iris_s_ouvre_sur_la_bienvenue,
        ),
        (
            "sur_un_telephone_les_reglages_sont_des_pages",
            sur_un_telephone_les_reglages_sont_des_pages,
        ),
    ];

    let mut echecs = Vec::new();
    for (nom, scenario) in scenarios {
        match std::panic::catch_unwind(scenario) {
            Ok(()) => println!("ok   {nom}"),
            Err(_) => {
                println!("FAIL {nom}");
                echecs.push(nom);
            }
        }
    }
    assert!(echecs.is_empty(), "scénarios en échec : {echecs:?}");
}
