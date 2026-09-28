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
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
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
