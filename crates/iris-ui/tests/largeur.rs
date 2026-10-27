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
