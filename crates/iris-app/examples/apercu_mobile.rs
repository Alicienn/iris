//! Iris at a phone's size, on invented data, captured as images.
//!
//! ```text
//! $env:SLINT_BACKEND="winit-software"; cargo run -p iris-app --no-default-features --example apercu_mobile -- <folder> [width] [height]
//! ```
//!
//! 412 by 892 by default, with the touch sizes on, as the mockups in
//! `..\Iris-design\android\` are drawn; `IRIS_THEME=dark` for the dark theme. Writes
//! `home.png`, `mail.png`, `calendar.png`, `tasks.png`, `notes.png`, `settings.png` and
//! `sheet-question.png` (a question rising from the bottom).

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
    let mut args = std::env::args().skip(1);
    let sortie = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let largeur: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(412.0);
    let hauteur: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(892.0);

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
    for (titre, j, m, p) in [
        ("Renew the car insurance", jour(-1), None, 3),
        ("Send the quote to Marie", jour(0), Some(14 * 60), 2),
        ("Water the balcony plants", jour(0), Some(19 * 60), 0),
        ("Find 5 more companies in Lyon", jour(0), None, 0),
    ] {
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
            .unwrap();
    }

    let cal = services.store.calendars().unwrap()[0].id;
    let a = |h: u32, m: u32| {
        iris_calendar::time::zoned_millis(today.and_hms_opt(h, m, 0).unwrap(), &Local)
    };
    for (uid, titre, debut, duree) in [
        ("standup", "Team stand-up", a(9, 30), 30),
        ("appel", "Call · Atelier façade", a(10, 30), 60),
        ("dej", "Lunch with Paul", a(12, 30), 60),
        ("dentiste", "Dentist", a(16, 0), 60),
    ] {
        services
            .store
            .insert_event(
                cal,
                &NewEvent {
                    uid: uid.into(),
                    summary: titre.into(),
                    start_ms: debut,
                    end_ms: debut + duree * 60_000,
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
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
    f.window()
        .set_size(slint::LogicalSize::new(largeur, hauteur));
    let layout = f.global::<iris_ui::Layout>();
    layout.set_touch_input(true);
    layout.set_inset_top(36.0);
    layout.set_inset_bottom(22.0);
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

    let aller = |f: &iris_ui::AppWindow, lieu: i32| {
        f.set_workspace(lieu);
        f.invoke_workspace_changed(lieu);
    };
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
                1 => f.set_home_mail_count("12".into()),
                2 => {
                    capture(&f, sortie.join("home.png"));
                    aller(&f, 0);
                }
                3 => {
                    capture(&f, sortie.join("mail.png"));
                    aller(&f, 1);
                }
                4 => {
                    capture(&f, sortie.join("calendar.png"));
                    aller(&f, 2);
                }
                5 => {
                    capture(&f, sortie.join("tasks.png"));
                    aller(&f, 4);
                }
                6 => {
                    capture(&f, sortie.join("notes.png"));
                    aller(&f, 3);
                }
                7 => f.set_settings_open(true),
                8 => {
                    capture(&f, sortie.join("settings.png"));
                    f.set_settings_open(false);
                    aller(&f, 0);
                    f.set_empty_folder_open(true);
                }
                9 => {
                    capture(&f, sortie.join("sheet-question.png"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
