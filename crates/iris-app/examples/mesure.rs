//! Ce que coûte la fenêtre, mesuré plutôt que supposé.
//!
//! Ouvre la vraie fenêtre d'Iris sur des données inventées — soixante conversations,
//! une infolettre longue dépliée — puis relève la mémoire du processus :
//!
//! 1. fenêtre ouverte, infolettre affichée ;
//! 2. fenêtre rangée dans la zone de notification, mémoire rendue.
//!
//! Le moteur de rendu de l'interface se choisit comme pour l'application :
//!
//! ```text
//! $env:SLINT_BACKEND="winit-femtovg";  cargo run --release -p iris-app --example mesure
//! $env:SLINT_BACKEND="winit-software"; cargo run --release -p iris-app --example mesure
//! ```
//!
//! Avec un chemin en argument, une capture de la fenêtre y est écrite, pour comparer
//! les rendus à l'œil.

use iris_htmlview::{BlitzRenderer, HtmlRenderer, Rendered, TiledDocument};
use iris_ui::{AppWindow, BodyTileData, MessageData, ThreadRowData};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

fn mo() -> f64 {
    iris_app::vitals::resident_bytes().unwrap_or(0) as f64 / 1048576.0
}

fn infolettre() -> String {
    let mut html = String::from("<html><body style=\"font-family:sans-serif;margin:0\">");
    for i in 0..120 {
        html.push_str(&format!(
            "<table width=\"100%\" cellpadding=\"12\"><tr><td bgcolor=\"#f4f4f4\">\
             <h2>Section {i}</h2><p>Voici le détail de ce que nous proposons ce mois-ci, \
             en quelques lignes, et ce qu'il resterait à décider.</p></td></tr></table>"
        ));
    }
    html.push_str("</body></html>");
    html
}

fn main() {
    let capture = std::env::args().nth(1);
    println!(
        "moteur de l'interface : {}",
        std::env::var("SLINT_BACKEND").unwrap_or_else(|_| "(défaut)".into())
    );
    println!("{:<40} {:>8.1} Mo", "au départ", mo());

    let fenetre = AppWindow::new().expect("fenêtre");
    fenetre
        .window()
        .set_size(slint::LogicalSize::new(1280.0, 800.0));

    let lignes: Vec<ThreadRowData> = (0..60)
        .map(|i| ThreadRowData {
            id: i,
            from: format!("Expéditeur {i}").into(),
            subject: format!("Sujet de la conversation {i}").into(),
            preview: "Un aperçu du message, sur une ligne".into(),
            date: "12:30".into(),
            unread: i % 3 == 0,
            ..Default::default()
        })
        .collect();
    fenetre.set_rows(ModelRc::new(VecModel::from(lignes)));

    // L'infolettre, mise en page et peinte par tuiles comme dans l'application.
    let moteur = BlitzRenderer::new(fenetre.window().scale_factor(), false);
    let moteur: Rc<BlitzRenderer> = Rc::new(moteur);
    let Ok(Rendered::Document(document)) = moteur.render(&infolettre(), 800.0) else {
        panic!("attendu un document");
    };
    let tuiles = Rc::new(VecModel::from(iris_ui::bridge::tile_placeholders(
        document.as_ref(),
    )));
    let document: Rc<RefCell<Box<dyn TiledDocument>>> = Rc::new(RefCell::new(document));
    {
        let (tuiles, document) = (Rc::clone(&tuiles), Rc::clone(&document));
        fenetre.on_body_tile_wanted(move |_, i| {
            let i = i as usize;
            let mut d = document.borrow_mut();
            let (l, h) = (d.size().0, d.tile_extent(i));
            let mut pixels = iris_ui::bridge::ImageSink::default();
            if d.paint_tile(i, &mut pixels).is_ok() {
                if let (Some(image), Some(mut t)) = (pixels.image(l, h), tuiles.row_data(i)) {
                    t.image = image;
                    t.ready = true;
                    tuiles.set_row_data(i, t);
                }
            }
        });
    }
    {
        let tuiles = Rc::clone(&tuiles);
        fenetre.on_body_tile_released(move |_, i| {
            if let Some(mut t) = tuiles.row_data(i as usize) {
                t.image = slint::Image::default();
                t.ready = false;
                tuiles.set_row_data(i as usize, t);
            }
        });
    }
    // Comme l'application : le corps est remis en page à la largeur que la colonne de
    // lecture annonce, et peint à cette taille exacte.
    {
        let (tuiles, document) = (Rc::clone(&tuiles), Rc::clone(&document));
        let echelle = fenetre.window().scale_factor();
        let html = infolettre();
        fenetre.on_reader_width(move |largeur| {
            let actuelle = document.borrow().size().0 as f32;
            if (actuelle - largeur * echelle).abs() < 1.0 {
                return;
            }
            if let Ok(Rendered::Document(nouveau)) =
                moteur.render_for(&html, false, largeur, echelle)
            {
                tuiles.set_vec(iris_ui::bridge::tile_placeholders(nouveau.as_ref()));
                *document.borrow_mut() = nouveau;
            }
        });
    }
    let message = MessageData {
        id: 1,
        from: "Boutique".into(),
        subject: "L'infolettre du mois".into(),
        expanded: true,
        body_is_image: true,
        body_tiles: ModelRc::from(Rc::clone(&tuiles)),
        ..Default::default()
    };
    fenetre.set_conversation_empty(false);
    fenetre.set_message(message.clone());
    fenetre.set_messages(ModelRc::new(VecModel::from(vec![message])));

    fenetre.show().expect("affichage");

    let etapes = slint::Timer::default();
    let faible = fenetre.as_weak();
    let (tuiles_fin, document_fin) = (Rc::clone(&tuiles), Rc::clone(&document));
    let mut tour = 0;
    etapes.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(3),
        move || {
            let Some(f) = faible.upgrade() else {
                return;
            };
            tour += 1;
            match tour {
                1 => {
                    let peintes = (0..tuiles_fin.row_count())
                        .filter(|i| {
                            tuiles_fin
                                .row_data(*i)
                                .is_some_and(|t: BodyTileData| t.ready)
                        })
                        .count();
                    println!(
                        "{:<40} {:>8.1} Mo   ({peintes}/{} tuiles peintes)",
                        "fenêtre ouverte, infolettre affichée",
                        mo(),
                        tuiles_fin.row_count()
                    );
                    if let Some(chemin) = &capture {
                        match f.window().take_snapshot() {
                            Ok(p) => {
                                let img = image::RgbaImage::from_raw(
                                    p.width(),
                                    p.height(),
                                    p.as_bytes().to_vec(),
                                );
                                if let Some(img) = img {
                                    let _ = img.save(chemin);
                                    println!("capture : {chemin}");
                                }
                            }
                            Err(e) => println!("capture impossible : {e}"),
                        }
                    }
                    // La zone de notification, comme `went_to_tray`.
                    let _ = f.window().hide();
                    f.set_bodies_suspended(true);
                    for i in 0..tuiles_fin.row_count() {
                        if let Some(mut t) = tuiles_fin.row_data(i) {
                            t.image = slint::Image::default();
                            t.ready = false;
                            tuiles_fin.set_row_data(i, t);
                        }
                    }
                    document_fin.borrow_mut().release();
                    iris_app::vitals::give_back_memory();
                }
                _ => {
                    println!("{:<40} {:>8.1} Mo", "dans la zone de notification", mo());
                    let _ = slint::quit_event_loop();
                }
            }
        },
    );

    slint::run_event_loop_until_quit().expect("boucle");
}
