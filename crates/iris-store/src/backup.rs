//! What exists only here, kept apart: calendars and events with what is set beside
//! them (notes, colours, call links, invitation answers), task lists and tasks, goals.
//!
//! Mail comes back from its servers; these do not, and a damaged or lost base took
//! them with it. A backup is a small SQLite file holding copies of those tables and a
//! line saying when and from which schema. Restoring empties the same tables and fills
//! them from it, column by column: a backup made by an older version restores into a
//! newer one, its missing columns taking their defaults.

use crate::{sql_err, Store};
use iris_types::{Error, Result, Timestamp};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Parents before children: copied and restored in this order, emptied in the other.
const TABLES: &[&str] = &[
    // The accounts calendars are kept with (their passwords stay in the vault).
    "calendar_accounts",
    "calendars",
    "calendar_events",
    "calendar_tombstones",
    "event_notes",
    "event_colors",
    "event_links",
    "invite_replies",
    "task_lists",
    "goals",
    "goal_entries",
    "goal_milestones",
    "tasks",
];

/// What a backup holds, to say so before restoring it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupInfo {
    pub made_at: Timestamp,
    pub events: i64,
    pub tasks: i64,
}

impl Store {
    /// Writes a backup of one's own data to `path`, replacing what is there. Written
    /// beside it first and renamed, so a backup cut short never stands for one.
    pub fn back_up_personal(&self, path: &Path, now: Timestamp) -> Result<()> {
        let provisoire = path.with_extension("partial");
        let _ = std::fs::remove_file(&provisoire);
        self.with_conn(|c| {
            attach(c, &provisoire)?;
            let copie = copy_out(c, now);
            detach(c);
            copie
        })?;
        std::fs::rename(&provisoire, path).map_err(|e| {
            let _ = std::fs::remove_file(&provisoire);
            Error::store(format!("sauvegarde : {e}"))
        })
    }

    /// Replaces one's own data with that of the backup at `path`, in one transaction:
    /// all of it or nothing.
    pub fn restore_personal(&self, path: &Path) -> Result<BackupInfo> {
        let info = backup_info(path)?;
        self.with_conn(|c| {
            attach(c, path)?;
            let retour = copy_in(c);
            detach(c);
            retour
        })?;
        Ok(info)
    }
}

/// The tables into the attached backup, with when and from which schema.
fn copy_out(c: &Connection, now: Timestamp) -> Result<()> {
    let tx = c
        .unchecked_transaction()
        .map_err(|e| sql_err("sauvegarde", e))?;
    for table in TABLES {
        tx.execute_batch(&format!(
            "CREATE TABLE sauvegarde.{table} AS SELECT * FROM main.{table};"
        ))
        .map_err(|e| sql_err("sauvegarde", e))?;
    }
    let version: i64 = tx
        .query_row("PRAGMA main.user_version", [], |r| r.get(0))
        .map_err(|e| sql_err("sauvegarde", e))?;
    tx.execute_batch("CREATE TABLE sauvegarde.about (schema INTEGER, made_at INTEGER);")
        .map_err(|e| sql_err("sauvegarde", e))?;
    tx.execute(
        "INSERT INTO sauvegarde.about (schema, made_at) VALUES (?1, ?2)",
        params![version, now.millis()],
    )
    .map_err(|e| sql_err("sauvegarde", e))?;
    tx.commit().map_err(|e| sql_err("sauvegarde", e))
}

/// The tables emptied and filled from the attached backup, by the columns both have.
fn copy_in(c: &Connection) -> Result<()> {
    let tx = c
        .unchecked_transaction()
        .map_err(|e| sql_err("restauration", e))?;
    for table in TABLES.iter().rev() {
        tx.execute(&format!("DELETE FROM main.{table}"), [])
            .map_err(|e| sql_err("restauration", e))?;
    }
    for table in TABLES {
        let ici = columns(&tx, "main", table)?;
        let la = columns(&tx, "sauvegarde", table)?;
        // A table the backup's version did not have yet stays empty.
        let communes: Vec<String> = ici
            .into_iter()
            .filter(|col| la.contains(col))
            .map(|col| format!("\"{}\"", col.replace('"', "\"\"")))
            .collect();
        if communes.is_empty() {
            continue;
        }
        let liste = communes.join(", ");
        tx.execute(
            &format!("INSERT INTO main.{table} ({liste}) SELECT {liste} FROM sauvegarde.{table}"),
            [],
        )
        .map_err(|e| sql_err("restauration", e))?;
    }
    tx.commit().map_err(|e| sql_err("restauration", e))
}

/// When the backup at `path` was made and how much it holds; an error if it is not one.
pub fn backup_info(path: &Path) -> Result<BackupInfo> {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| Error::store("this file is not an Iris backup"))?;
    let made_at: Option<i64> = c
        .query_row("SELECT made_at FROM about", [], |r| r.get(0))
        .optional()
        .map_err(|_| Error::store("this file is not an Iris backup"))?;
    let made_at = made_at.ok_or_else(|| Error::store("this file is not an Iris backup"))?;
    let compte = |table: &str| -> i64 {
        c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap_or(0)
    };
    Ok(BackupInfo {
        made_at: Timestamp::from_millis(made_at),
        events: compte("calendar_events"),
        tasks: compte("tasks"),
    })
}

fn attach(c: &Connection, path: &Path) -> Result<()> {
    c.execute(
        "ATTACH DATABASE ?1 AS sauvegarde",
        [path.to_string_lossy().as_ref()],
    )
    .map(|_| ())
    .map_err(|e| sql_err("sauvegarde", e))
}

fn detach(c: &Connection) {
    if let Err(e) = c.execute_batch("DETACH DATABASE sauvegarde") {
        tracing::warn!(error = %e, "detaching a backup");
    }
}

/// The columns of `schema.table`, none when it does not exist there.
fn columns(c: &Connection, schema: &str, table: &str) -> Result<Vec<String>> {
    let mut stmt = c
        .prepare(&format!("PRAGMA {schema}.table_info({table})"))
        .map_err(|e| sql_err("restauration", e))?;
    let noms = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| sql_err("restauration", e))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| sql_err("restauration", e))?;
    Ok(noms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewEvent, NewTask};

    fn evenement(titre: &str) -> NewEvent {
        NewEvent {
            uid: format!("{titre}@example.com"),
            summary: titre.into(),
            start_ms: 1_000,
            end_ms: 2_000,
            ..Default::default()
        }
    }

    #[test]
    fn a_backup_brings_back_what_was_deleted_and_drops_what_came_after() {
        let dossier = std::env::temp_dir().join(format!(
            "iris-sauvegarde-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dossier).unwrap();
        let fichier = dossier.join("personal.db");
        let t = Timestamp::from_millis;

        let s = Store::in_memory().unwrap();
        let agenda = s.calendars().unwrap()[0].id;
        s.insert_event(agenda, &evenement("Dentiste"), t(1))
            .unwrap();
        let liste = s.task_lists().unwrap()[0].id;
        s.insert_task(
            &NewTask {
                list_id: liste,
                title: "Renew the passport".into(),
                ..Default::default()
            },
            t(1),
        )
        .unwrap();

        s.back_up_personal(&fichier, t(42)).unwrap();
        let info = backup_info(&fichier).unwrap();
        assert_eq!((info.made_at, info.events, info.tasks), (t(42), 1, 1));

        // Lost after the backup, and something new.
        let dentiste = s.events_for_range(0, 10_000).unwrap()[0].id;
        s.delete_event(dentiste).unwrap();
        s.insert_event(agenda, &evenement("Later"), t(2)).unwrap();

        s.restore_personal(&fichier).unwrap();
        let titres: Vec<String> = s
            .events_for_range(0, 10_000)
            .unwrap()
            .into_iter()
            .map(|e| e.event.summary)
            .collect();
        assert_eq!(titres, ["Dentiste"]);
        assert_eq!(s.open_tasks().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dossier);
    }

    #[test]
    fn a_file_that_is_not_a_backup_is_refused() {
        let fichier = std::env::temp_dir().join(format!("iris-pas-une-{}.db", std::process::id()));
        std::fs::write(&fichier, b"hello").unwrap();
        assert!(backup_info(&fichier).is_err());
        let _ = std::fs::remove_file(&fichier);
    }
}
