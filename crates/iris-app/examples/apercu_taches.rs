//! Un aperçu de l'onglet des tâches, sur des données inventées, capturé en images.
//!
//! ```text
//! $env:SLINT_BACKEND="winit-software"; cargo run -p iris-app --example apercu_taches -- <dossier>
//! ```
//!
//! Écrit `taches-aujourdhui.png`, `taches-liste.png`, `taches-date.png`, puis les
//! objectifs : `objectif.png`, `objectif-jalons.png`, `objectifs.png`,
//! `objectif-nouveau.png` et `objectif-temps.png`.

use chrono::{Duration, Local};
use iris_app::services::Services;
use iris_store::{NewEvent, NewTask};
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

    let t = iris_types::Timestamp::EPOCH;
    let aujourdhui = Local::now().date_naive();
    let jour = |n: i64| {
        Some(
            (aujourdhui + Duration::days(n))
                .format("%Y-%m-%d")
                .to_string(),
        )
    };
    let perso = services.store.task_lists().unwrap()[0].id;
    let travail = services
        .store
        .create_task_list("Work", "#4fb286", t)
        .unwrap();
    let courses = services
        .store
        .create_task_list("Groceries", "#e0795b", t)
        .unwrap();

    let ajoute = |list, titre: &str, jour: Option<String>, minute: Option<i32>, p: i32| {
        services
            .store
            .insert_task(
                &NewTask {
                    list_id: list,
                    title: titre.into(),
                    due_day: jour,
                    due_minute: minute,
                    priority: p,
                    ..Default::default()
                },
                t,
            )
            .unwrap()
    };
    ajoute(perso, "Renew the car insurance", jour(-2), None, 3);
    let devis = ajoute(
        travail,
        "Send the quote to Marie",
        jour(0),
        Some(15 * 60),
        2,
    );
    ajoute(perso, "Call the plumber", jour(0), None, 0);
    let poste = ajoute(perso, "Post the parcel", jour(0), None, 0);
    // Done at 08:40 today, and a few more earlier this week, for the week's bars.
    let a = |j: i64, h: u32, m: u32| {
        iris_types::Timestamp::from_millis(iris_calendar::time::zoned_millis(
            (aujourdhui + Duration::days(j))
                .and_hms_opt(h, m, 0)
                .unwrap(),
            &Local,
        ))
    };
    services
        .store
        .set_task_done(poste, Some(a(0, 8, 40)))
        .unwrap();
    let lundi = -(chrono::Datelike::weekday(&aujourdhui).num_days_from_monday() as i64);
    for (k, (titre, j)) in [
        ("Pay the rent", lundi),
        ("Water the plants", lundi),
        ("Book the dentist", lundi + 1),
        ("Answer Paul", lundi + 1),
        ("Order ink", lundi + 1),
        ("Return the drill", lundi + 2),
    ]
    .into_iter()
    .enumerate()
    {
        if j <= 0 {
            let id = ajoute(perso, titre, None, None, 0);
            services
                .store
                .set_task_done(id, Some(a(j, 10, k as u32)))
                .unwrap();
        }
    }
    ajoute(
        travail,
        "Prepare Thursday's review",
        jour(1),
        Some(9 * 60),
        1,
    );
    ajoute(travail, "Book the train to Lyon", jour(3), None, 0);
    ajoute(travail, "Send the VAT return", jour(7), None, 2);
    // One that comes back every week.
    let plantes = ajoute(perso, "Water the balcony plants", jour(0), Some(19 * 60), 0);
    let mut p = services.store.task(plantes).unwrap().unwrap().task;
    p.repeat = Some("weekly".into());
    services.store.update_task(plantes, &p, t).unwrap();
    ajoute(perso, "Dentist", jour(12), Some(10 * 60 + 30), 0);
    ajoute(courses, "Bread", None, None, 0);
    ajoute(courses, "Olive oil", None, None, 0);
    ajoute(perso, "Read the lease again", None, None, 0);
    for (i, etape) in [
        "Check the figures",
        "Ask Paul for the logo",
        "Export as PDF",
    ]
    .iter()
    .enumerate()
    {
        let id = services
            .store
            .insert_task(
                &NewTask {
                    list_id: travail,
                    parent_id: Some(devis),
                    title: (*etape).into(),
                    ..Default::default()
                },
                t,
            )
            .unwrap();
        if i == 0 {
            services.store.set_task_done(id, Some(t)).unwrap();
        }
    }
    let mut d = services.store.task(devis).unwrap().unwrap().task;
    d.notes = "Marie wants the two options side by side.".into();
    d.source = "Marie Martin: Quote for the new site".into();
    d.thread_id = Some(1);
    services.store.update_task(devis, &d, t).unwrap();

    // Goals: one counted, on track; one in milestones; one behind.
    let objectif = |titre: &str,
                    jalons: bool,
                    cible: i32,
                    unite: &str,
                    depuis: i64,
                    dans: i64,
                    couleur: &str| {
        services
            .store
            .create_goal(
                &iris_store::NewGoal {
                    title: titre.into(),
                    why: if cible == 10 {
                        "End-of-studies internship in Lyon, April to September.".into()
                    } else {
                        String::new()
                    },
                    kind: if jalons {
                        iris_store::GoalKind::Milestones
                    } else {
                        iris_store::GoalKind::Count
                    },
                    target: cible,
                    unit: unite.into(),
                    due_day: jour(dans).unwrap(),
                    color: couleur.into(),
                },
                a(depuis, 9, 0),
            )
            .unwrap()
    };
    let stages = objectif(
        "Send 10 internship applications",
        false,
        10,
        "applications",
        -13,
        18,
        "#5b7cf0",
    );
    for (j, note) in [
        (-11, "Studio Nord"),
        (-8, "Atelier Martin"),
        (-4, "Façades Gauthier"),
        (-1, "Called Bloc Studio, they want a mail"),
    ] {
        services.store.log_goal(stages, note, a(j, 11, 0)).unwrap();
    }
    let site = objectif("Launch the new website", true, 1, "", -20, 25, "#e8a45b");
    for (k, m) in ["Brief", "Wireframes", "Design", "Content", "Launch"]
        .iter()
        .enumerate()
    {
        let id = services.store.add_milestone(site, m).unwrap();
        if k < 2 {
            services
                .store
                .set_milestone_done(id, Some(a(-15 + k as i64 * 6, 10, 0)))
                .unwrap();
        }
    }
    let livres = objectif(
        "Read 4 books this term",
        false,
        4,
        "books",
        -40,
        30,
        "#6fb7a4",
    );
    services
        .store
        .log_goal(livres, "The Old Man and the Sea", a(-30, 20, 0))
        .unwrap();
    let mut coince = 0;
    for (titre, j, duree, reporte) in [
        ("Find 5 more companies in Lyon", 0, 45, 3),
        ("Update the CV with the last project", 1, 60, 0),
        ("Write the application to Bloc Studio", 2, 30, 0),
    ] {
        let id = ajoute(perso, titre, jour(j), None, 0);
        let mut t = services.store.task(id).unwrap().unwrap().task;
        t.goal_id = Some(stages);
        t.estimate = Some(duree);
        t.postponed = reporte;
        services
            .store
            .update_task(id, &t, iris_types::Timestamp::EPOCH)
            .unwrap();
        if reporte > 0 {
            coince = id;
        }
    }

    let cal = services.store.calendars().unwrap()[0].id;
    let midi =
        iris_calendar::time::zoned_millis(aujourdhui.and_hms_opt(12, 30, 0).unwrap(), &Local);
    services
        .store
        .insert_event(
            cal,
            &NewEvent {
                uid: "dej".into(),
                summary: "Lunch with Paul".into(),
                start_ms: midi,
                end_ms: midi + 3_600_000,
                ..Default::default()
            },
            t,
        )
        .unwrap();

    let f = iris_ui::AppWindow::new().unwrap();
    f.window().set_size(slint::LogicalSize::new(1280.0, 800.0));
    iris_app::calendar::wire_calendar(&f, &services, runtime.handle().clone());
    iris_app::tasks::wire_tasks(&f, &services, controller);
    f.set_workspace(2);
    f.invoke_workspace_changed(2);
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
                    f.invoke_task_place_chosen("today".into());
                    f.invoke_task_row_selected(iris_ui::TaskRowData {
                        kind: 0,
                        id: devis as i32,
                        ..Default::default()
                    });
                    let ligne = "Call Marie about the logo tomorrow 9am #Work !!!";
                    f.set_task_add_text(ligne.into());
                    f.invoke_task_add_edited(ligne.into());
                }
                2 => {
                    capture(&f, sortie.join("taches-aujourdhui.png"));
                    f.set_task_add_text("".into());
                    f.invoke_task_add_edited("".into());
                    f.invoke_task_place_chosen(format!("list:{travail}").into());
                    f.invoke_task_row_selected(iris_ui::TaskRowData {
                        kind: 0,
                        id: devis as i32,
                        ..Default::default()
                    });
                }
                3 => {
                    capture(&f, sortie.join("taches-liste.png"));
                    f.invoke_task_picker_toggled();
                }
                4 => {
                    capture(&f, sortie.join("taches-date.png"));
                    f.invoke_task_detail_closed();
                    f.invoke_task_place_chosen(format!("goal:{stages}").into());
                }
                5 => {
                    capture(&f, sortie.join("objectif.png"));
                    f.invoke_task_place_chosen(format!("goal:{site}").into());
                }
                6 => {
                    capture(&f, sortie.join("objectif-jalons.png"));
                    f.invoke_task_place_chosen("goals".into());
                }
                7 => {
                    capture(&f, sortie.join("objectifs.png"));
                    f.invoke_goal_new_requested();
                }
                8 => {
                    capture(&f, sortie.join("objectif-nouveau.png"));
                    f.set_goal_new_open(false);
                    f.invoke_task_place_chosen(format!("goal:{stages}").into());
                    f.invoke_goal_time_requested();
                }
                9 => {
                    capture(&f, sortie.join("objectif-temps.png"));
                    f.set_goal_time_open(false);
                    f.invoke_task_row_selected(iris_ui::TaskRowData {
                        kind: 0,
                        id: coince as i32,
                        ..Default::default()
                    });
                }
                10 => {
                    capture(&f, sortie.join("taches-coince.png"));
                    // Where "Give it a slot" puts it.
                    f.set_task_slot_x(930.0);
                    f.set_task_slot_y(220.0);
                    f.invoke_task_slot_requested();
                }
                11 => {
                    capture(&f, sortie.join("taches-creneau.png"));
                    f.set_task_slot_open(false);
                    f.invoke_task_detail_closed();
                    f.invoke_task_place_chosen("week".into());
                }
                12 => {
                    capture(&f, sortie.join("taches-semaine.png"));
                    f.invoke_task_place_chosen(format!("goal:{stages}").into());
                    f.invoke_goal_edit_requested();
                }
                13 => {
                    capture(&f, sortie.join("objectif-modifier.png"));
                    f.set_goal_new_open(false);
                    f.invoke_task_place_chosen("today".into());
                    f.invoke_task_row_selected(iris_ui::TaskRowData {
                        kind: 0,
                        id: plantes as i32,
                        ..Default::default()
                    });
                }
                14 => {
                    capture(&f, sortie.join("taches-repete.png"));
                    let _ = slint::quit_event_loop();
                }
                _ => {}
            }
        },
    );
    slint::run_event_loop_until_quit().unwrap();
}
