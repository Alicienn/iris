//! Un aperçu de l'accueil, sur des données inventées, capturé en images.
//!
//! ```text
//! $env:SLINT_BACKEND="winit-software"; cargo run -p iris-app --example apercu_accueil -- <dossier>
//! ```
//!
//! Écrit `accueil.png`, `accueil-synchro.png` (pendant une synchronisation) et
//! `evenement-taches.png` (un événement et ses tâches).

use chrono::{Duration, Local};
use iris_app::services::{now, Services};
use iris_store::{NewAccount, NewEvent, NewTask};
use slint::ComponentHandle;

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
    for a in ["contact@atelier.example.com", "marie@example.com"] {
        services
            .store
            .create_account(
                &NewAccount::new(a, "imap.example.com", "smtp.example.com"),
                now(),
            )
            .unwrap();
    }

    let today = Local::now().date_naive();
    let jour = |n: i64| Some((today + Duration::days(n)).format("%Y-%m-%d").to_string());
    let liste = services.store.task_lists().unwrap()[0].id;
    let tache = |titre: &str, j: Option<String>, m: Option<i32>, p: i32| {
        services
            .store
            .insert_task(
                &NewTask {
                    list_id: liste,
                    title: titre.into(),
                    due_day: j,
                    due_minute: m,
                    priority: p,
                    ..Default::default()
                },
                now(),
            )
            .unwrap()
    };
    tache("Renew the car insurance", jour(-1), None, 3);
    tache("Send the quote to Marie", jour(0), Some(15 * 60), 2);
    let fait = tache("Post the parcel", jour(0), None, 0);
    services.store.set_task_done(fait, Some(now())).unwrap();
    tache("Prepare Thursday's review", jour(1), Some(9 * 60), 1);
    tache("Book the train to Lyon", jour(3), None, 0);

    let cal = services.store.calendars().unwrap()[0].id;
    let a = |j: i64, h: u32, m: u32| {
        iris_calendar::time::zoned_millis(
            (today + Duration::days(j)).and_hms_opt(h, m, 0).unwrap(),
            &Local,
        )
    };
    let mut premier = None;
    for (uid, titre, debut, duree) in [
        ("standup", "Team stand-up", a(0, 9, 30), 30),
        ("dej", "Lunch with Paul", a(0, 12, 30), 60),
        ("yoga", "Yoga", a(0, 18, 0), 60),
        ("dentiste", "Dentist", a(1, 10, 30), 45),
        ("revue", "Quarterly review", a(3, 14, 0), 90),
    ] {
        let id = services
            .store
            .insert_event(
                cal,
                &NewEvent {
                    uid: uid.into(),
                    summary: titre.into(),
                    location: if uid == "yoga" {
                        "Studio Nord".into()
                    } else {
                        String::new()
                    },
                    start_ms: debut,
                    end_ms: debut + duree * 60_000,
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        if uid == "revue" {
            premier = Some(format!("{id}:{debut}"));
        }
    }
    iris_app::home::count_arrivals(12);

    let runtime = iris_app::services::runtime().unwrap();
    let (controller, _fil) = iris_app::controller::Controller::spawn_with_index(
        std::sync::Arc::clone(&services.store),
        None,
        std::sync::Arc::clone(&services.workflow),
        now(),
        |_| {},
    );
    let controller = std::sync::Arc::new(controller);

    let f = iris_ui::AppWindow::new().unwrap();
    f.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    // IRIS_THEME=dark captures the dark theme.
    let sombre = std::env::var("IRIS_THEME").as_deref() == Ok("dark");
    let theme = services.themes.apply(
        if sombre {
            iris_theme::Appearance::Dark
        } else {
            iris_theme::Appearance::Light
        },
        false,
    );
    iris_app::shell::appliquer_apparence(&f, &theme, iris_app::settings::Density::Normal);
    iris_app::calendar::wire_calendar(&f, &services, runtime.handle().clone());
    iris_app::tasks::wire_tasks(&f, &services, std::sync::Arc::clone(&controller));
    iris_app::home::wire_home(&f, &services, controller);
    iris_app::nav::wire_navigation(&f, iris_app::nav::Place::start(3, 1));
    f.set_workspace(3);
    f.invoke_workspace_changed(3);
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
                // The sample base has no mail: the count a real one would show.
                1 => f.set_home_mail_count("12".into()),
                2 => {
                    capture(&f, sortie.join("accueil.png"));
                    f.set_syncing_all(true);
                    f.set_sync_progress("2 of 5".into());
                }
                3 => {
                    capture(&f, sortie.join("accueil-synchro.png"));
                    f.set_syncing_all(false);
                    if let Some(cle) = &premier {
                        f.invoke_home_event_opened(cle.as_str().into());
                        f.invoke_event_task_added("Print the slides".into());
                        f.invoke_event_task_added("Book the room".into());
                    }
                }
                4 => {
                    capture(&f, sortie.join("evenement-taches.png"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
