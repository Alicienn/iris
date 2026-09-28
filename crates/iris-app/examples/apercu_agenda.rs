//! Un aperçu de l'agenda, sur des données inventées, capturé en images.
//!
//! ```text
//! $env:SLINT_BACKEND="winit-software"; cargo run -p iris-app --example apercu_agenda -- <dossier>
//! ```
//!
//! Écrit `agenda-mois.png`, `agenda-semaine.png` et `agenda-editeur.png` dans le dossier.

use chrono::{Datelike, Duration, Local, NaiveDate};
use iris_app::services::Services;
use iris_store::NewEvent;
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

fn a(jour: NaiveDate, h: u32, m: u32) -> i64 {
    iris_calendar::time::zoned_millis(jour.and_hms_opt(h, m, 0).unwrap(), &Local)
}

fn main() {
    let sortie = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let dir = tempfile::tempdir().unwrap();
    let services = Services::open(
        iris_app::paths::Paths::under(dir.path()),
        Some(iris_secrets::Secret::new("apercu")),
    )
    .unwrap();
    let runtime = iris_app::services::runtime().unwrap();

    let aujourdhui = Local::now().date_naive();
    let lundi = aujourdhui - Duration::days(aujourdhui.weekday().num_days_from_monday() as i64);
    let perso = services.store.calendars().unwrap()[0].id;
    let club = services
        .store
        .create_calendar(
            "Club de voile",
            "#4fb286",
            Some("https://example.com/club.ics"),
            iris_types::Timestamp::EPOCH,
        )
        .unwrap();
    let ev = |titre: &str, debut: i64, fin: i64| NewEvent {
        uid: titre.into(),
        summary: titre.into(),
        start_ms: debut,
        end_ms: fin,
        ..Default::default()
    };
    let t = iris_types::Timestamp::EPOCH;
    let mut point = ev("Point d'équipe", a(lundi, 10, 0), a(lundi, 11, 0));
    point.rrule = Some("FREQ=WEEKLY;BYDAY=MO".into());
    point.tzid = iana_time_zone::get_timezone().ok();
    point.reminder_minutes = Some(15);
    point.location = "Salle 3".into();
    services.store.insert_event(perso, &point, t).unwrap();
    services
        .store
        .insert_event(
            perso,
            &ev(
                "Dentiste",
                a(lundi + Duration::days(1), 14, 30),
                a(lundi + Duration::days(1), 15, 15),
            ),
            t,
        )
        .unwrap();
    services
        .store
        .insert_event(
            perso,
            &ev(
                "Appel client",
                a(lundi + Duration::days(2), 9, 0),
                a(lundi + Duration::days(2), 10, 30),
            ),
            t,
        )
        .unwrap();
    services
        .store
        .insert_event(
            perso,
            &ev(
                "Revue de code",
                a(lundi + Duration::days(2), 9, 30),
                a(lundi + Duration::days(2), 10, 0),
            ),
            t,
        )
        .unwrap();
    services
        .store
        .insert_event(
            perso,
            &ev(
                "Déjeuner avec Marie",
                a(lundi + Duration::days(3), 12, 30),
                a(lundi + Duration::days(3), 14, 0),
            ),
            t,
        )
        .unwrap();
    let debut_regate =
        iris_calendar::layout::local_midnight(lundi + Duration::days(5), &chrono::Utc);
    let mut regate = ev(
        "Régate d'automne",
        debut_regate,
        debut_regate + 2 * 86_400_000,
    );
    regate.all_day = true;
    services.store.insert_event(club, &regate, t).unwrap();
    services
        .store
        .insert_event(
            club,
            &ev(
                "Entraînement",
                a(lundi + Duration::days(3), 18, 0),
                a(lundi + Duration::days(3), 20, 0),
            ),
            t,
        )
        .unwrap();

    let f = iris_ui::AppWindow::new().unwrap();
    f.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    iris_app::calendar::wire_calendar(&f, &services, runtime.handle().clone());
    f.set_workspace(1);
    f.invoke_workspace_changed(1);
    f.show().unwrap();

    let etapes = slint::Timer::default();
    let faible = f.as_weak();
    let mut tour = 0;
    etapes.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(800),
        move || {
            let Some(f) = faible.upgrade() else { return };
            tour += 1;
            match tour {
                1 => {
                    capture(&f, sortie.join("agenda-mois.png"));
                    f.invoke_calendar_mode_chosen(1);
                }
                2 => {
                    capture(&f, sortie.join("agenda-semaine.png"));
                    f.invoke_calendar_new_event("".into(), -1);
                }
                3 => {
                    capture(&f, sortie.join("agenda-editeur.png"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
