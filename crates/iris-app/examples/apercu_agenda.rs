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
use slint::Model as _;

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

    let long_debut = a(lundi + Duration::days(4), 16, 0);
    let long_titre = services
        .store
        .insert_event(
            perso,
            &ev(
                "Comité de pilotage trimestriel avec la direction financière et les responsables des trois agences régionales",
                long_debut,
                long_debut + 3_600_000,
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
                    f.invoke_editor_picker_requested(0);
                }
                4 => {
                    capture(&f, sortie.join("agenda-editeur-date.png"));
                    f.set_event_editor_open(false);
                    f.invoke_calendar_event_opened(format!("{long_titre}:{long_debut}").into());
                }
                5 => {
                    // Opened from elsewhere: in the middle. Then as a click in the
                    // grid opens it: beside the event.
                    capture(&f, sortie.join("agenda-detail.png"));
                    f.set_event_detail_open(false);
                    f.set_event_anchored(true);
                    f.set_event_anchor_left(860.0);
                    f.set_event_anchor_right(990.0);
                    f.set_event_anchor_top(420.0);
                    f.invoke_calendar_event_opened(format!("{long_titre}:{long_debut}").into());
                }
                6 => {
                    capture(&f, sortie.join("agenda-detail-carte.png"));
                    f.set_event_detail_open(false);
                    f.invoke_calendar_mode_chosen(0);
                    let club = f.get_calendars().row_data(1).unwrap();
                    f.set_calendar_menu_cal(club);
                    f.set_calendar_menu_x(120.0);
                    f.set_calendar_menu_y(420.0);
                    f.set_calendar_menu_open(true);
                }
                7 => {
                    capture(&f, sortie.join("agenda-menu.png"));
                    f.set_calendar_menu_open(false);
                    f.set_calendar_delete_open(true);
                }
                8 => {
                    capture(&f, sortie.join("agenda-supprimer.png"));
                    f.set_calendar_delete_open(false);
                    f.set_settings_open(true);
                }
                9 => {
                    capture(&f, sortie.join("reglages.png"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
