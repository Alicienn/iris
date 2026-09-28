//! Un aperçu des tags d'adresses, sur des données inventées, capturé en images.
//!
//! ```text
//! $env:SLINT_BACKEND="winit-software"; cargo run -p iris-app --example apercu_tags -- <dossier>
//! ```
//!
//! Écrit `tags-colonne.png` (comptes rangés par tag), `tags-menu.png` (le menu d'un
//! compte et ses tags) et `tags-gestion.png` (la fenêtre des tags).

use iris_app::services::{now, Services};
use iris_store::NewAccount;
use slint::{ComponentHandle, ModelRc, VecModel};

fn capture(f: &iris_ui::AppWindow, chemin: std::path::PathBuf) {
    match f.window().take_snapshot() {
        Ok(p) => {
            if let Some(img) =
                image::RgbaImage::from_raw(p.width(), p.height(), p.as_bytes().to_vec())
            {
                let _ = img.save(&chemin);
                println!("capture : {}", chemin.display());
            }
        }
        Err(e) => println!("capture impossible : {e}"),
    }
}

fn main() {
    let sortie = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let dir = tempfile::tempdir().unwrap();
    let services = Services::open(
        iris_app::paths::Paths::under(dir.path()),
        Some(iris_secrets::Secret::new("apercu")),
    )
    .unwrap();

    let compte = |adresse: &str| {
        services
            .store
            .create_account(
                &NewAccount::new(adresse, "imap.example.com", "smtp.example.com"),
                now(),
            )
            .unwrap()
    };
    let contact = compte("contact@atelier.example.com");
    let factures = compte("factures@atelier.example.com");
    let moi = compte("marie@example.com");
    let club = compte("bureau@club.example.com");
    compte("archives@example.com");

    let clients = services
        .store
        .create_account_tag("Clients", iris_app::tags::PALETTE[2], now())
        .unwrap();
    let perso = services
        .store
        .create_account_tag("Personal", iris_app::tags::PALETTE[0], now())
        .unwrap();
    let asso = services
        .store
        .create_account_tag("Club", iris_app::tags::PALETTE[3], now())
        .unwrap();
    for (c, t) in [
        (contact, clients),
        (factures, clients),
        (moi, perso),
        (club, asso),
    ] {
        services.store.set_account_tagged(c, t, true).unwrap();
    }

    let f = iris_ui::AppWindow::new().unwrap();
    f.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    f.set_group_by_tags(true);
    iris_app::shell::refresh_accounts(&f, &services, &[]);
    f.show().unwrap();

    let etapes = slint::Timer::default();
    let faible = f.as_weak();
    let mut tour = 0;
    etapes.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(700),
        move || {
            let Some(f) = faible.upgrade() else { return };
            tour += 1;
            match tour {
                2 => {
                    capture(&f, sortie.join("tags-colonne.png"));
                    f.set_account_menu_label("contact@atelier.example.com".into());
                    f.set_account_menu_tags(ModelRc::new(VecModel::from(
                        iris_app::tags::menu_tags(&services, contact, ""),
                    )));
                    f.set_account_menu_x(120.0);
                    f.set_account_menu_y(120.0);
                    f.set_account_tags_open(true);
                    f.set_account_menu_open(true);
                }
                3 => {
                    capture(&f, sortie.join("tags-menu.png"));
                    f.set_account_menu_open(false);
                    f.invoke_tags_requested();
                    f.set_account_tags(ModelRc::new(VecModel::from(
                        services
                            .store
                            .account_tags()
                            .unwrap()
                            .iter()
                            .map(|t| iris_ui::AccountTagData {
                                id: t.id as i32,
                                name: t.name.as_str().into(),
                                color: iris_app::calendar::couleur(&t.color),
                                count: t.accounts as i32,
                                checked: false,
                            })
                            .collect::<Vec<_>>(),
                    )));
                    f.set_tags_open(true);
                }
                4 => {
                    capture(&f, sortie.join("tags-gestion.png"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
