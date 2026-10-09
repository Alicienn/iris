//! Un aperçu de la Croissance (habitudes, objectifs) et de l'accueil, sur des données
//! inventées, capturé en images.
//!
//! ```text
//! $env:SLINT_BACKEND="winit-software"; cargo run -p iris-app --no-default-features --example apercu_croissance -- <dossier>
//! ```
//!
//! Écrit `habitudes.png`, `habitudes-mois.png`, `habitudes-annee.png`,
//! `habitude-nouvelle.png`, `croissance-objectif.png` et `accueil-habitudes.png`.

use chrono::{Datelike, Duration, Local, NaiveDate};
use iris_app::services::Services;
use iris_store::{NewGoal, NewHabit};
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
    let runtime = iris_app::services::runtime().unwrap();
    let (controller, _fil) = iris_app::controller::Controller::spawn_with_index(
        std::sync::Arc::clone(&services.store),
        None,
        std::sync::Arc::clone(&services.workflow),
        iris_app::services::now(),
        |_| {},
    );
    let controller = std::sync::Arc::new(controller);

    let aujourdhui = Local::now().date_naive();
    let il_y_a = |n: i64| {
        iris_types::Timestamp::from_millis(iris_calendar::time::zoned_millis(
            (aujourdhui - Duration::days(n))
                .and_hms_opt(9, 0, 0)
                .unwrap(),
            &Local,
        ))
    };
    let jour = |d: NaiveDate| d.format("%Y-%m-%d").to_string();

    let objectif = |titre: &str, cible: i32, unite: &str, depuis: i64, dans: i64, couleur: &str| {
        services
            .store
            .create_goal(
                &NewGoal {
                    title: titre.into(),
                    target: cible,
                    unit: unite.into(),
                    due_day: jour(aujourdhui + Duration::days(dans)),
                    color: couleur.into(),
                    ..Default::default()
                },
                il_y_a(depuis),
            )
            .unwrap()
    };
    let stages = objectif("10 applications", 10, "applications", 20, 18, "#af52de");
    for j in [18, 12, 9, 6, 3, 1] {
        services.store.log_goal(stages, "", il_y_a(j)).unwrap();
    }
    let livres = objectif("Read 12 books", 12, "books", 240, 80, "#ff9500");
    for j in 0..7 {
        services
            .store
            .log_goal(livres, "", il_y_a(200 - j * 25))
            .unwrap();
    }
    let course = objectif("Run a 10 km", 5, "runs of 5 km", 30, 50, "#34c759");
    services.store.log_goal(course, "", il_y_a(10)).unwrap();
    services.store.log_goal(course, "", il_y_a(3)).unwrap();

    let habitude =
        |h: NewHabit, depuis: i64| services.store.create_habit(&h, il_y_a(depuis)).unwrap();
    let lire = habitude(
        NewHabit {
            title: "Read".into(),
            icon: "book".into(),
            color: "#ff9500".into(),
            schedule: "daily".into(),
            amount: 20,
            unit: "pages".into(),
            goal_id: Some(livres),
            remind_minute: Some(21 * 60 + 30),
            ..Default::default()
        },
        200,
    );
    let eau = habitude(
        NewHabit {
            title: "Drink water".into(),
            icon: "drop".into(),
            color: "#30b0c7".into(),
            schedule: "daily".into(),
            amount: 6,
            unit: "glasses".into(),
            ..Default::default()
        },
        60,
    );
    let courir = habitude(
        NewHabit {
            title: "Run".into(),
            icon: "run".into(),
            color: "#34c759".into(),
            schedule: "week:3".into(),
            goal_id: Some(course),
            ..Default::default()
        },
        40,
    );
    let espagnol = habitude(
        NewHabit {
            title: "Spanish".into(),
            icon: "chat".into(),
            color: "#ff2d55".into(),
            schedule: "days:1111100".into(),
            amount: 15,
            unit: "min".into(),
            ..Default::default()
        },
        30,
    );
    let telephone = habitude(
        NewHabit {
            title: "No phone in bed".into(),
            icon: "phone".into(),
            color: "#8e8e93".into(),
            quit: true,
            schedule: "daily".into(),
            ..Default::default()
        },
        25,
    );

    // The days kept: reading most days and the last twelve in a row, water every day
    // but one partly, running three times most weeks, Spanish on weekdays bar one.
    let mut graine: u32 = 7;
    let mut hasard = || {
        graine = graine.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (graine >> 16) % 100
    };
    for k in 1..200 {
        let d = aujourdhui - Duration::days(k);
        if k <= 12 || hasard() < 78 {
            services.store.set_habit_check(lire, &jour(d), 20).unwrap();
        }
    }
    services
        .store
        .set_habit_check(lire, &jour(aujourdhui), 20)
        .unwrap();
    for k in 0..4 {
        let d = aujourdhui - Duration::days(k);
        services
            .store
            .set_habit_check(eau, &jour(d), if k == 2 { 3 } else { 6 })
            .unwrap();
    }
    for k in 1..40 {
        let d = aujourdhui - Duration::days(k);
        if matches!(d.weekday().num_days_from_monday(), 0 | 2 | 4) && hasard() < 85 {
            services.store.set_habit_check(courir, &jour(d), 1).unwrap();
        }
    }
    for k in 1..30 {
        let d = aujourdhui - Duration::days(k);
        if d.weekday().num_days_from_monday() < 5 && k != 2 {
            services
                .store
                .set_habit_check(espagnol, &jour(d), 15)
                .unwrap();
        }
    }
    for k in 1..12 {
        if k != 2 {
            let d = aujourdhui - Duration::days(k);
            services
                .store
                .set_habit_check(telephone, &jour(d), 1)
                .unwrap();
        }
    }

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
    iris_app::growth::wire_growth(&f, &services);
    iris_app::home::wire_home(&f, &services, controller);
    f.set_workspace(iris_app::growth::WORKSPACE);
    f.invoke_workspace_changed(iris_app::growth::WORKSPACE);
    f.show().unwrap();

    let suffixe = if sombre { "-sombre" } else { "" };
    let nom = move |n: &str| sortie.join(format!("{n}{suffixe}.png"));
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
                1 => f.invoke_habit_selected(lire as i32),
                2 => {
                    capture(&f, nom("habitudes"));
                    f.invoke_habit_detail_closed();
                    f.invoke_growth_mode_chosen(1);
                }
                3 => {
                    capture(&f, nom("habitudes-mois"));
                    f.invoke_growth_mode_chosen(2);
                }
                4 => {
                    capture(&f, nom("habitudes-annee"));
                    f.invoke_growth_mode_chosen(0);
                    f.invoke_habit_new_requested();
                }
                5 => {
                    capture(&f, nom("habitude-nouvelle"));
                    f.set_habit_new_open(false);
                    f.invoke_growth_place_chosen(format!("goal:{stages}").into());
                }
                6 => {
                    capture(&f, nom("croissance-objectif"));
                    f.set_workspace(3);
                    f.invoke_workspace_changed(3);
                }
                7 => {
                    capture(&f, nom("accueil-habitudes"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
