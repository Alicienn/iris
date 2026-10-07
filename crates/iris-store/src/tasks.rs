//! Les tâches : des listes, et ce qu'il y a à faire dedans.

use crate::Store;
use iris_types::{Error, Result, Timestamp};
use rusqlite::{params, OptionalExtension, Row};

/// Une liste de tâches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskList {
    pub id: i64,
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
}

/// Une tâche, telle qu'elle s'écrit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewTask {
    pub list_id: i64,
    /// La tâche dont celle-ci est une étape.
    pub parent_id: Option<i64>,
    pub title: String,
    pub notes: String,
    /// `YYYY-MM-DD`.
    pub due_day: Option<String>,
    /// Minutes depuis minuit.
    pub due_minute: Option<i32>,
    /// Combien de minutes avant l'échéance rappeler ; `None` : pas de rappel.
    pub remind_before: Option<i32>,
    /// L'instant du rappel, en millisecondes, déduit des trois champs précédents.
    pub remind_at: Option<i64>,
    /// 0 aucune, 1 basse, 2 moyenne, 3 haute.
    pub priority: i32,
    /// Le fil de courrier d'où vient la tâche.
    pub thread_id: Option<i64>,
    /// De quoi nommer ce fil sans le relire : « Marie: Devis refonte ».
    pub source: String,
    /// The event the task was added from: its identifier…
    pub event_uid: Option<String>,
    /// …and the start of the occurrence (0 when the event does not repeat).
    pub event_start: Option<i64>,
    /// The goal it moves forward.
    pub goal_id: Option<i64>,
    /// How long it takes, in minutes.
    pub estimate: Option<i32>,
    /// How many times it was put off to a later day.
    pub postponed: i32,
    /// How it comes back once done: "daily", "weekdays", "weekly", "monthly",
    /// "yearly"; `None` when it does not.
    pub repeat: Option<String>,
}

/// Une tâche, telle qu'elle se relit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredTask {
    pub id: i64,
    pub task: NewTask,
    pub done_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

impl StoredTask {
    pub fn is_done(&self) -> bool {
        self.done_at.is_some()
    }
}

fn err(quoi: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::store(format!("{quoi} : {e}"))
}

pub(crate) const COLONNES: &str =
    "id, list_id, parent_id, title, notes, due_day, due_minute, remind_before, \
     remind_at, priority, thread_id, source, done_at, created_at, event_uid, event_start, \
     goal_id, estimate, postponed, repeat";

pub(crate) fn tache(r: &Row<'_>) -> rusqlite::Result<StoredTask> {
    Ok(StoredTask {
        id: r.get(0)?,
        task: NewTask {
            list_id: r.get(1)?,
            parent_id: r.get(2)?,
            title: r.get(3)?,
            notes: r.get(4)?,
            due_day: r.get(5)?,
            due_minute: r.get(6)?,
            remind_before: r.get(7)?,
            remind_at: r.get(8)?,
            priority: r.get(9)?,
            thread_id: r.get(10)?,
            source: r.get(11)?,
            event_uid: r.get(14)?,
            event_start: r.get(15)?,
            goal_id: r.get(16)?,
            estimate: r.get(17)?,
            postponed: r.get(18)?,
            repeat: r.get(19)?,
        },
        done_at: r.get::<_, Option<i64>>(12)?.map(Timestamp::from_millis),
        created_at: Timestamp::from_millis(r.get(13)?),
    })
}

/// L'ordre d'une liste : ce qui est dû d'abord, à l'heure dite pour celles qui en ont
/// une, puis l'ordre choisi à la main (glissé), qui est celui de la création tant
/// qu'on n'y a pas touché.
const ORDRE: &str = "due_day IS NULL, due_day, due_minute IS NULL, due_minute, position, \
     priority DESC, id";

impl Store {
    pub fn task_lists(&self) -> Result<Vec<TaskList>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare("SELECT id, name, color FROM task_lists ORDER BY position, id")
                .map_err(err("lecture des listes"))?;
            let lignes = stmt
                .query_map([], |r| {
                    Ok(TaskList {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        color: r.get(2)?,
                    })
                })
                .map_err(err("lecture des listes"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des listes"));
            lignes
        })
    }

    pub fn create_task_list(&self, name: &str, color: &str, now: Timestamp) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO task_lists (name, color, position, created_at) VALUES \
                 (?1, ?2, (SELECT COALESCE(MAX(position), 0) + 1 FROM task_lists), ?3)",
                params![name, color, now.millis()],
            )
            .map_err(err("création d'une liste"))?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn rename_task_list(&self, id: i64, name: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE task_lists SET name = ?2 WHERE id = ?1",
                params![id, name],
            )
            .map(|_| ())
            .map_err(err("renommage d'une liste"))
        })
    }

    /// Supprime une liste et toutes ses tâches.
    pub fn delete_task_list(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM task_lists WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'une liste"))
        })
    }

    /// Toutes les tâches à faire, sous-tâches comprises, dans l'ordre d'une liste.
    pub fn open_tasks(&self) -> Result<Vec<StoredTask>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM tasks WHERE done_at IS NULL ORDER BY {ORDRE}"
                ))
                .map_err(err("lecture des tâches"))?;
            let lignes = stmt
                .query_map([], tache)
                .map_err(err("lecture des tâches"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des tâches"));
            lignes
        })
    }

    /// Les tâches faites le plus récemment, d'une liste ou de toutes, sans les étapes.
    /// Every task, done or not, steps included: parents before their steps.
    pub fn all_tasks(&self) -> Result<Vec<StoredTask>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM tasks ORDER BY parent_id IS NOT NULL, list_id, position, id"
                ))
                .map_err(err("lecture des tâches"))?;
            let lignes = stmt
                .query_map([], tache)
                .map_err(err("lecture des tâches"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des tâches"));
            lignes
        })
    }

    pub fn done_tasks(&self, list: Option<i64>, limit: u32) -> Result<Vec<StoredTask>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM tasks WHERE done_at IS NOT NULL AND parent_id IS NULL \
                     AND (?1 IS NULL OR list_id = ?1) ORDER BY done_at DESC LIMIT ?2"
                ))
                .map_err(err("lecture des tâches faites"))?;
            let lignes = stmt
                .query_map(params![list, limit as i64], tache)
                .map_err(err("lecture des tâches faites"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des tâches faites"));
            lignes
        })
    }

    /// Les étapes d'une tâche, faites ou non, dans l'ordre où elles ont été ajoutées.
    pub fn subtasks(&self, parent: i64) -> Result<Vec<StoredTask>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM tasks WHERE parent_id = ?1 ORDER BY position, id"
                ))
                .map_err(err("lecture des étapes"))?;
            let lignes = stmt
                .query_map([parent], tache)
                .map_err(err("lecture des étapes"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des étapes"));
            lignes
        })
    }

    /// Le nombre d'étapes d'une tâche, et combien sont faites.
    pub fn subtask_progress(&self, parent: i64) -> Result<(u32, u32)> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT COUNT(*), COUNT(done_at) FROM tasks WHERE parent_id = ?1",
                [parent],
                |r| Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?)),
            )
            .map_err(err("progression des étapes"))
        })
    }

    pub fn task(&self, id: i64) -> Result<Option<StoredTask>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("SELECT {COLONNES} FROM tasks WHERE id = ?1"),
                [id],
                tache,
            )
            .optional()
            .map_err(err("lecture d'une tâche"))
        })
    }

    /// Les tâches à faire venues d'un fil de courrier.
    pub fn tasks_for_thread(&self, thread: i64) -> Result<Vec<StoredTask>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM tasks WHERE thread_id = ?1 AND done_at IS NULL \
                     ORDER BY {ORDRE}"
                ))
                .map_err(err("tâches d'un fil"))?;
            let lignes = stmt
                .query_map([thread], tache)
                .map_err(err("tâches d'un fil"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("tâches d'un fil"));
            lignes
        })
    }

    pub fn insert_task(&self, t: &NewTask, now: Timestamp) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO tasks (list_id, parent_id, title, notes, due_day, due_minute, \
                 remind_before, remind_at, priority, thread_id, source, position, created_at, \
                 updated_at, event_uid, event_start, goal_id, estimate, postponed, repeat) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, \
                 (SELECT COALESCE(MAX(position), 0) + 1 FROM tasks), ?12, ?12, ?13, ?14, ?15, \
                 ?16, ?17, ?18)",
                params![
                    t.list_id,
                    t.parent_id,
                    t.title,
                    t.notes,
                    t.due_day,
                    t.due_minute,
                    t.remind_before,
                    t.remind_at,
                    t.priority,
                    t.thread_id,
                    t.source,
                    now.millis(),
                    t.event_uid,
                    t.event_start,
                    t.goal_id,
                    t.estimate,
                    t.postponed,
                    t.repeat
                ],
            )
            .map_err(err("écriture d'une tâche"))?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Puts back tasks that were deleted, under their own identifiers, done or not as
    /// they were: what Ctrl+Z needs after a delete. A task comes before its subtasks.
    /// A list deleted since takes its tasks with it: those are not restored.
    pub fn restore_tasks(&self, tasks: &[StoredTask], now: Timestamp) -> Result<usize> {
        self.with_tx(|tx| {
            let mut remises = 0;
            for s in tasks {
                let t = &s.task;
                let fait = tx
                    .execute(
                        "INSERT OR IGNORE INTO tasks (id, list_id, parent_id, title, notes, \
                         due_day, due_minute, remind_before, remind_at, reminded, priority, \
                         thread_id, source, position, created_at, updated_at, done_at, \
                         event_uid, event_start, goal_id, estimate, postponed, repeat) \
                         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11, ?12, \
                         (SELECT COALESCE(MAX(position), 0) + 1 FROM tasks), ?13, ?14, ?15, \
                         ?16, ?17, (SELECT id FROM goals WHERE id = ?18), ?19, ?20, ?21 \
                         WHERE EXISTS (SELECT 1 FROM task_lists WHERE id = ?2)",
                        params![
                            s.id,
                            t.list_id,
                            t.parent_id,
                            t.title,
                            t.notes,
                            t.due_day,
                            t.due_minute,
                            t.remind_before,
                            t.remind_at,
                            t.priority,
                            t.thread_id,
                            t.source,
                            s.created_at.millis(),
                            now.millis(),
                            s.done_at.map(|d| d.millis()),
                            t.event_uid,
                            t.event_start,
                            t.goal_id,
                            t.estimate,
                            t.postponed,
                            t.repeat
                        ],
                    )
                    .map_err(err("restauration d'une tâche"))?;
                remises += fait;
            }
            Ok(remises)
        })
    }

    /// The tasks added from an event: its identifier and occurrence, done or not.
    pub fn tasks_for_event(&self, uid: &str, start: i64) -> Result<Vec<StoredTask>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM tasks WHERE event_uid = ?1 AND event_start = ?2 \
                     AND parent_id IS NULL ORDER BY position, id"
                ))
                .map_err(err("tâches d'un événement"))?;
            let lignes = stmt
                .query_map(params![uid, start], tache)
                .map_err(err("tâches d'un événement"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("tâches d'un événement"));
            lignes
        })
    }

    /// Réécrit une tâche. Un rappel déplacé sera redonné.
    /// Puts tasks in the order given, among themselves: their places are renumbered
    /// from the first of them, one after the other.
    pub fn set_task_positions(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        self.with_conn(|c| {
            let liste = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let debut: i64 = c
                .query_row(
                    &format!("SELECT COALESCE(MIN(position), 0) FROM tasks WHERE id IN ({liste})"),
                    rusqlite::params_from_iter(ids.iter()),
                    |r| r.get(0),
                )
                .map_err(|e| Error::store(format!("ordre des tâches : {e}")))?;
            for (i, id) in ids.iter().enumerate() {
                c.execute(
                    "UPDATE tasks SET position = ?1 WHERE id = ?2",
                    rusqlite::params![debut + i as i64, id],
                )
                .map_err(|e| Error::store(format!("ordre des tâches : {e}")))?;
            }
            Ok(())
        })
    }

    pub fn update_task(&self, id: i64, t: &NewTask, now: Timestamp) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE tasks SET list_id = ?2, parent_id = ?3, title = ?4, notes = ?5, \
                 due_day = ?6, due_minute = ?7, remind_before = ?8, \
                 reminded = CASE WHEN remind_at IS ?9 THEN reminded ELSE 0 END, remind_at = ?9, \
                 priority = ?10, thread_id = ?11, source = ?12, updated_at = ?13, \
                 event_uid = ?14, event_start = ?15, goal_id = ?16, estimate = ?17, \
                 postponed = ?18, repeat = ?19 WHERE id = ?1",
                params![
                    id,
                    t.list_id,
                    t.parent_id,
                    t.title,
                    t.notes,
                    t.due_day,
                    t.due_minute,
                    t.remind_before,
                    t.remind_at,
                    t.priority,
                    t.thread_id,
                    t.source,
                    now.millis(),
                    t.event_uid,
                    t.event_start,
                    t.goal_id,
                    t.estimate,
                    t.postponed,
                    t.repeat
                ],
            )
            .map_err(err("mise à jour d'une tâche"))?;
            // Les étapes suivent leur tâche d'une liste à l'autre.
            tx.execute(
                "UPDATE tasks SET list_id = ?2 WHERE parent_id = ?1",
                params![id, t.list_id],
            )
            .map_err(err("mise à jour des étapes"))?;
            Ok(())
        })
    }

    /// Coche ou décoche une tâche.
    pub fn set_task_done(&self, id: i64, done: Option<Timestamp>) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE tasks SET done_at = ?2 WHERE id = ?1",
                params![id, done.map(|t| t.millis())],
            )
            .map(|_| ())
            .map_err(err("état d'une tâche"))
        })
    }

    /// Supprime une tâche et ses étapes.
    pub fn delete_task(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM tasks WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'une tâche"))
        })
    }

    /// Les rappels arrivés à l'heure et pas encore donnés.
    pub fn due_task_reminders(&self, now: Timestamp) -> Result<Vec<StoredTask>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM tasks WHERE reminded = 0 AND done_at IS NULL \
                     AND remind_at IS NOT NULL AND remind_at <= ?1 ORDER BY remind_at"
                ))
                .map_err(err("rappels"))?;
            let lignes = stmt
                .query_map([now.millis()], tache)
                .map_err(err("rappels"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("rappels"));
            lignes
        })
    }

    pub fn mark_task_reminded(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("UPDATE tasks SET reminded = 1 WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("rappel donné"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    fn t(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    fn nouvelle(list: i64, titre: &str) -> NewTask {
        NewTask {
            list_id: list,
            title: titre.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_first_list_is_there_from_the_start() {
        let s = store();
        let listes = s.task_lists().unwrap();
        assert_eq!(listes.len(), 1);
        assert_eq!(listes[0].name, "My tasks");
    }

    #[test]
    fn open_tasks_come_due_first_then_in_the_order_given() {
        let s = store();
        let l = s.task_lists().unwrap()[0].id;
        let sans = s.insert_task(&nouvelle(l, "sans date"), t(1)).unwrap();
        let mut tard = nouvelle(l, "tard");
        tard.due_day = Some("2026-10-10".into());
        let tard = s.insert_task(&tard, t(2)).unwrap();
        let mut tot = nouvelle(l, "tôt");
        tot.due_day = Some("2026-09-28".into());
        tot.due_minute = Some(600);
        let tot = s.insert_task(&tot, t(3)).unwrap();
        let mut urgent = nouvelle(l, "urgent");
        urgent.due_day = Some("2026-10-10".into());
        urgent.priority = 3;
        let urgent = s.insert_task(&urgent, t(4)).unwrap();

        let ordre: Vec<i64> = s.open_tasks().unwrap().iter().map(|x| x.id).collect();
        assert_eq!(ordre, [tot, tard, urgent, sans], "the same day: as created");

        // Dragged above: the order given is kept.
        s.set_task_positions(&[urgent, tard]).unwrap();
        let ordre: Vec<i64> = s.open_tasks().unwrap().iter().map(|x| x.id).collect();
        assert_eq!(ordre, [tot, urgent, tard, sans]);
    }

    #[test]
    fn done_tasks_leave_the_open_list() {
        let s = store();
        let l = s.task_lists().unwrap()[0].id;
        let id = s.insert_task(&nouvelle(l, "a"), t(1)).unwrap();
        s.set_task_done(id, Some(t(50))).unwrap();
        assert!(s.open_tasks().unwrap().is_empty());
        let faites = s.done_tasks(Some(l), 10).unwrap();
        assert_eq!(faites[0].id, id);
        assert_eq!(faites[0].done_at, Some(t(50)));
        s.set_task_done(id, None).unwrap();
        assert_eq!(s.open_tasks().unwrap().len(), 1);
    }

    #[test]
    fn subtasks_follow_their_task() {
        let s = store();
        let l = s.task_lists().unwrap()[0].id;
        let autre = s.create_task_list("Travail", "#4fb286", t(1)).unwrap();
        let parent = s.insert_task(&nouvelle(l, "déménager"), t(1)).unwrap();
        let mut etape = nouvelle(l, "cartons");
        etape.parent_id = Some(parent);
        let etape = s.insert_task(&etape, t(2)).unwrap();
        s.set_task_done(etape, Some(t(3))).unwrap();
        assert_eq!(s.subtask_progress(parent).unwrap(), (1, 1));

        let mut p = s.task(parent).unwrap().unwrap().task;
        p.list_id = autre;
        s.update_task(parent, &p, t(4)).unwrap();
        assert_eq!(s.task(etape).unwrap().unwrap().task.list_id, autre);

        s.delete_task(parent).unwrap();
        assert!(
            s.task(etape).unwrap().is_none(),
            "les étapes partent avec leur tâche"
        );
    }

    #[test]
    fn a_reminder_is_given_once_and_again_when_moved() {
        let s = store();
        let l = s.task_lists().unwrap()[0].id;
        let mut a = nouvelle(l, "appeler");
        a.remind_at = Some(1_000);
        let id = s.insert_task(&a, t(1)).unwrap();
        assert!(s.due_task_reminders(t(999)).unwrap().is_empty());
        assert_eq!(s.due_task_reminders(t(1_000)).unwrap().len(), 1);
        s.mark_task_reminded(id).unwrap();
        assert!(s.due_task_reminders(t(2_000)).unwrap().is_empty());

        // Même heure : rien à redonner. Heure changée : le rappel revient.
        s.update_task(id, &a, t(2)).unwrap();
        assert!(s.due_task_reminders(t(2_000)).unwrap().is_empty());
        a.remind_at = Some(1_500);
        s.update_task(id, &a, t(3)).unwrap();
        assert_eq!(s.due_task_reminders(t(2_000)).unwrap().len(), 1);
    }

    #[test]
    fn deleting_a_list_deletes_its_tasks() {
        let s = store();
        let autre = s.create_task_list("Courses", "#e5484d", t(1)).unwrap();
        let id = s.insert_task(&nouvelle(autre, "pain"), t(1)).unwrap();
        s.delete_task_list(autre).unwrap();
        assert!(s.task(id).unwrap().is_none());
        assert_eq!(s.task_lists().unwrap().len(), 1);
    }

    #[test]
    fn a_deleted_task_comes_back_with_its_subtasks() {
        let s = store();
        let l = s.task_lists().unwrap()[0].id;
        let parent = s.insert_task(&nouvelle(l, "déménager"), t(1)).unwrap();
        let mut etape = nouvelle(l, "cartons");
        etape.parent_id = Some(parent);
        let etape = s.insert_task(&etape, t(2)).unwrap();
        s.set_task_done(etape, Some(t(3))).unwrap();

        let mut avant = vec![s.task(parent).unwrap().unwrap()];
        avant.extend(s.subtasks(parent).unwrap());
        s.delete_task(parent).unwrap();
        assert!(s.task(etape).unwrap().is_none());

        assert_eq!(s.restore_tasks(&avant, t(9)).unwrap(), 2);
        assert_eq!(s.task(parent).unwrap().unwrap().task.title, "déménager");
        assert_eq!(s.task(etape).unwrap().unwrap().done_at, Some(t(3)));
        // Twice: nothing more.
        assert_eq!(s.restore_tasks(&avant, t(9)).unwrap(), 0);
    }

    #[test]
    fn an_event_keeps_its_tasks_by_identifier_and_occurrence() {
        let s = store();
        let l = s.task_lists().unwrap()[0].id;
        let mut a = nouvelle(l, "Préparer les slides");
        a.event_uid = Some("revue@example.com".into());
        a.event_start = Some(1_000);
        let id = s.insert_task(&a, t(1)).unwrap();
        let mut b = a.clone();
        b.event_start = Some(2_000);
        s.insert_task(&b, t(2)).unwrap();
        let liees = s.tasks_for_event("revue@example.com", 1_000).unwrap();
        assert_eq!(liees.len(), 1);
        assert_eq!(liees[0].id, id);
        s.set_task_done(id, Some(t(5))).unwrap();
        assert_eq!(
            s.tasks_for_event("revue@example.com", 1_000).unwrap().len(),
            1,
            "done, it is still listed on the event"
        );
    }

    #[test]
    fn tasks_remember_the_thread_they_came_from() {
        let s = store();
        let l = s.task_lists().unwrap()[0].id;
        let mut a = nouvelle(l, "Répondre au devis");
        a.thread_id = Some(42);
        a.source = "Marie: Devis".into();
        s.insert_task(&a, t(1)).unwrap();
        let liees = s.tasks_for_thread(42).unwrap();
        assert_eq!(liees.len(), 1);
        assert_eq!(liees[0].task.source, "Marie: Devis");
    }
}
