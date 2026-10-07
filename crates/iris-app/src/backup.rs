//! One's calendars and tasks kept safe, and taken elsewhere.
//!
//! Mail comes back from its servers; the calendars, tasks and goals made in Iris exist
//! on this computer only. Each day Iris copies them to a small file of their own in
//! `backups` beside the base, and keeps the last fourteen (a few hundred kilobytes
//! each). One of them puts everything back as it was that day, after what is there
//! now has itself been kept. And they can be written out as iCalendar files, which
//! every other calendar and task application imports.

use crate::services::{now, Services};
use chrono::{Local, NaiveDate, TimeZone};
use iris_store::BackupInfo;
use iris_types::{Error, Result};
use iris_ui::AppWindow;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// How many daily backups are kept.
pub const KEEP: usize = 14;

const PREFIXE: &str = "personal-";

/// Where the backups are.
pub fn folder(services: &Services) -> PathBuf {
    services.paths.data.join("backups")
}

fn nom_du_jour(jour: NaiveDate) -> String {
    format!("{PREFIXE}{}.db", jour.format("%Y-%m-%d"))
}

/// The daily backups there are, newest first.
pub fn list(services: &Services) -> Vec<PathBuf> {
    let mut fichiers: Vec<PathBuf> = std::fs::read_dir(folder(services))
        .map(|lecture| {
            lecture
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(PREFIXE) && n.ends_with(".db"))
                })
                .collect()
        })
        .unwrap_or_default();
    // The day is in the name, written so that names sort as days do.
    fichiers.sort();
    fichiers.reverse();
    fichiers
}

/// Today's backup, made now (again, if there is one already); the oldest beyond
/// [`KEEP`] removed.
pub fn back_up_now(services: &Services) -> Result<PathBuf> {
    let dossier = folder(services);
    std::fs::create_dir_all(&dossier)?;
    let chemin = dossier.join(nom_du_jour(Local::now().date_naive()));
    services.store.back_up_personal(&chemin, now())?;
    for vieux in list(services).into_iter().skip(KEEP) {
        if let Err(e) = std::fs::remove_file(&vieux) {
            tracing::warn!(error = %e, file = %vieux.display(), "removing an old backup");
        }
    }
    Ok(chemin)
}

/// Today's backup, unless it is made already. True when one was made.
pub fn daily(services: &Services) -> Result<bool> {
    let aujourd_hui = folder(services).join(nom_du_jour(Local::now().date_naive()));
    if aujourd_hui.exists() {
        return Ok(false);
    }
    back_up_now(services).map(|_| true)
}

/// Puts the backup at `path` back, after keeping what is there now beside the daily
/// ones (`before-restore-…`), so a restore can itself be undone.
pub fn restore(services: &Services, path: &Path) -> Result<BackupInfo> {
    iris_store::backup_info(path)?;
    let dossier = folder(services);
    std::fs::create_dir_all(&dossier)?;
    let avant = dossier.join(format!(
        "before-restore-{}.db",
        Local::now().format("%Y-%m-%d-%H%M%S")
    ));
    services.store.back_up_personal(&avant, now())?;
    services.store.restore_personal(path)
}

/// "Last backup today at 14:05. The last 14 days are kept."
pub fn status(services: &Services) -> String {
    let derniere = list(services)
        .first()
        .and_then(|p| iris_store::backup_info(p).ok());
    match derniere {
        Some(info) => format!(
            "Last backup {}. The last {KEEP} days are kept.",
            quand(info.made_at.millis())
        ),
        None => format!("Every day, Iris keeps a copy of them; the last {KEEP} days."),
    }
}

/// "today at 14:05", "on 7 October at 09:12".
fn quand(ms: i64) -> String {
    let Some(t) = Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    if t.date_naive() == Local::now().date_naive() {
        format!("today at {}", t.format("%H:%M"))
    } else {
        format!("on {} at {}", t.format("%-d %B"), t.format("%H:%M"))
    }
}

/// What restoring `info` would do, asked before it does.
fn question(info: &BackupInfo) -> String {
    format!(
        "Replace your calendars, tasks and goals with those of the backup made {} \
         ({} events, {} tasks)? What is there now is kept as a backup first.",
        quand(info.made_at.millis()),
        info.events,
        info.tasks
    )
}

/// One's own calendars and the tasks, as `.ics` files in `dir`: one per calendar,
/// and `Tasks.ics`. Gives how many files were written.
pub fn export_to(services: &Services, dir: &Path) -> Result<usize> {
    let maintenant = now().millis();
    let mut ecrits = 0;
    let mut noms_pris: Vec<String> = Vec::new();
    for agenda in services.store.calendars()? {
        if agenda.is_subscription() {
            continue;
        }
        let evenements: Vec<iris_calendar::Event> = services
            .store
            .events_of_calendar(agenda.id)?
            .iter()
            .map(|e| crate::calendar::to_domain(&e.event))
            .collect();
        let nom = nom_de_fichier(&agenda.name, &mut noms_pris);
        std::fs::write(
            dir.join(format!("{nom}.ics")),
            iris_calendar::write::calendar(&agenda.name, &evenements, maintenant),
        )?;
        ecrits += 1;
    }

    let listes: std::collections::HashMap<i64, String> = services
        .store
        .task_lists()?
        .into_iter()
        .map(|l| (l.id, l.name))
        .collect();
    let taches: Vec<iris_calendar::write::Todo> = services
        .store
        .all_tasks()?
        .into_iter()
        .map(|t| iris_calendar::write::Todo {
            uid: todo_uid(t.id),
            summary: t.task.title.clone(),
            description: t.task.notes.clone(),
            due: t
                .task
                .due_day
                .as_deref()
                .and_then(|j| NaiveDate::parse_from_str(j, "%Y-%m-%d").ok())
                .map(|j| (j, t.task.due_minute.map(|m| m.max(0) as u32))),
            completed: t.done_at.map(|d| d.millis()),
            priority: t.task.priority.clamp(0, 3) as u8,
            category: listes.get(&t.task.list_id).cloned().unwrap_or_default(),
            parent_uid: t.task.parent_id.map(todo_uid),
            repeat: t.task.repeat.clone(),
            created: t.created_at.millis(),
        })
        .collect();
    let nom = nom_de_fichier("Tasks", &mut noms_pris);
    std::fs::write(
        dir.join(format!("{nom}.ics")),
        iris_calendar::write::todos("Tasks", &taches, maintenant),
    )?;
    Ok(ecrits + 1)
}

/// A task's identifier in a file: its own, apart from the event that books it.
fn todo_uid(id: i64) -> String {
    format!("todo-{id}@iris")
}

/// A name Windows accepts for a file, and not one already given.
fn nom_de_fichier(nom: &str, pris: &mut Vec<String>) -> String {
    let propre: String = nom
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let propre = propre.trim_end_matches(['.', ' ']).to_string();
    let base = if propre.is_empty() {
        "Calendar".to_string()
    } else {
        propre
    };
    let mut nom = base.clone();
    let mut n = 2;
    while pris.iter().any(|p| p.eq_ignore_ascii_case(&nom)) {
        nom = format!("{base} ({n})");
        n += 1;
    }
    pris.push(nom.clone());
    nom
}

/// The rows of *Settings › Your calendars and tasks*, and the daily backup.
pub fn wire_backup(fenetre: &AppWindow, services: &Services, runtime: tokio::runtime::Handle) {
    fenetre.set_backup_status(status(services).into());

    // Once a day: looked at an hour after launch and every hour after, which costs a
    // directory read, so an Iris left open for days still makes one each day.
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                let s = services.clone();
                let fait = tokio::task::spawn_blocking(move || daily(&s))
                    .await
                    .unwrap_or_else(|e| Err(Error::other(e.to_string())));
                match fait {
                    Ok(true) => {
                        let texte = status(&services);
                        let _ = faible.upgrade_in_event_loop(move |f| {
                            f.set_backup_status(texte.into());
                        });
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!(error = %e, "daily backup"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
    }

    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_backup_now(move || {
            let Some(f) = faible.upgrade() else {
                return;
            };
            match back_up_now(&services) {
                Ok(_) => f.set_toast("Your calendars and tasks are backed up.".into()),
                Err(e) => f.set_status(format!("Could not back up: {e}").into()),
            }
            f.set_backup_status(status(&services).into());
        });
    }
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_backup_open_folder(move || {
            let Some(f) = faible.upgrade() else {
                return;
            };
            let dossier = folder(&services);
            let ouvert = std::fs::create_dir_all(&dossier)
                .map_err(Error::from)
                .and_then(|_| crate::platform::open_path(&dossier));
            if let Err(e) = ouvert {
                f.set_status(format!("Could not open the folder: {e}").into());
            }
        });
    }

    // The backup chosen, between the question and the answer.
    let choisie: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    {
        let (services, choisie) = (services.clone(), Rc::clone(&choisie));
        let faible = fenetre.as_weak();
        fenetre.on_restore_choose(move || {
            let Some(f) = faible.upgrade() else {
                return;
            };
            let dossier = folder(&services);
            let _ = std::fs::create_dir_all(&dossier);
            let Some(chemin) = rfd::FileDialog::new()
                .set_title("Restore a backup")
                .set_directory(&dossier)
                .add_filter("Iris backup", &["db"])
                .pick_file()
            else {
                return;
            };
            match iris_store::backup_info(&chemin) {
                Ok(info) => {
                    f.set_restore_question(question(&info).into());
                    *choisie.borrow_mut() = Some(chemin);
                }
                Err(e) => f.set_status(format!("Could not restore: {e}").into()),
            }
        });
    }
    {
        let choisie = Rc::clone(&choisie);
        let faible = fenetre.as_weak();
        fenetre.on_restore_cancelled(move || {
            choisie.borrow_mut().take();
            if let Some(f) = faible.upgrade() {
                f.set_restore_question("".into());
            }
        });
    }
    {
        let (services, choisie) = (services.clone(), Rc::clone(&choisie));
        let faible = fenetre.as_weak();
        fenetre.on_restore_confirmed(move || {
            let Some(f) = faible.upgrade() else {
                return;
            };
            f.set_restore_question("".into());
            let Some(chemin) = choisie.borrow_mut().take() else {
                return;
            };
            match restore(&services, &chemin) {
                Ok(info) => {
                    f.set_toast(
                        format!(
                            "Restored: your calendars and tasks are as they were {}.",
                            quand(info.made_at.millis())
                        )
                        .into(),
                    );
                    // The place shown draws itself again; the others do on arrival.
                    f.invoke_workspace_changed(f.get_workspace());
                }
                Err(e) => f.set_status(format!("Could not restore: {e}").into()),
            }
            f.set_backup_status(status(&services).into());
        });
    }
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_export_ics(move || {
            let Some(f) = faible.upgrade() else {
                return;
            };
            let Some(dossier) = rfd::FileDialog::new()
                .set_title("Export your calendars and tasks")
                .pick_folder()
            else {
                return;
            };
            match export_to(&services, &dossier) {
                Ok(n) => {
                    f.set_toast(format!("{n} files saved in {}.", nom_affiche(&dossier)).into())
                }
                Err(e) => f.set_status(format!("Could not export: {e}").into()),
            }
        });
    }
}

fn nom_affiche(dossier: &Path) -> String {
    dossier
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dossier.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_ones_windows_accepts_and_never_twice() {
        let mut pris = Vec::new();
        assert_eq!(
            nom_de_fichier("Work: Q4/Plans?", &mut pris),
            "Work_ Q4_Plans_"
        );
        assert_eq!(nom_de_fichier("Personal", &mut pris), "Personal");
        assert_eq!(nom_de_fichier("personal", &mut pris), "personal (2)");
        assert_eq!(nom_de_fichier("  ", &mut pris), "Calendar");
        assert_eq!(nom_de_fichier("Trip.", &mut pris), "Trip");
    }
}
