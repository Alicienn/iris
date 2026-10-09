//! Goals: something to reach by a day, and what moved it.
//!
//! A goal is counted (a target of something, each one logged by hand) or made of
//! milestones ticked in order. Tasks point to the goal they move forward; nothing is
//! counted on the user's behalf.

use crate::tasks::StoredTask;
use crate::Store;
use iris_types::{Error, Result, Timestamp};
use rusqlite::{params, OptionalExtension, Row};

/// How a goal is measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GoalKind {
    /// A number of something: "10 applications".
    #[default]
    Count,
    /// Steps ticked in order.
    Milestones,
}

impl GoalKind {
    fn mot(self) -> &'static str {
        match self {
            GoalKind::Count => "count",
            GoalKind::Milestones => "milestones",
        }
    }

    fn depuis(mot: &str) -> Self {
        if mot == "milestones" {
            GoalKind::Milestones
        } else {
            GoalKind::Count
        }
    }
}

/// A goal, as it is written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewGoal {
    pub title: String,
    /// What it is for, in the user's words. Optional.
    pub why: String,
    pub kind: GoalKind,
    /// For a count: how many.
    pub target: i32,
    /// For a count: of what ("applications").
    pub unit: String,
    /// `YYYY-MM-DD`.
    pub due_day: String,
    /// `#rrggbb`.
    pub color: String,
}

/// A goal, as it is read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    pub id: i64,
    pub goal: NewGoal,
    pub created_at: Timestamp,
}

/// One logged step of a counted goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalEntry {
    pub id: i64,
    pub at: Timestamp,
    pub note: String,
}

/// A milestone of a goal, in its order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Milestone {
    pub id: i64,
    pub title: String,
    pub done_at: Option<Timestamp>,
}

fn err(quoi: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::store(format!("{quoi} : {e}"))
}

fn objectif(r: &Row<'_>) -> rusqlite::Result<Goal> {
    Ok(Goal {
        id: r.get(0)?,
        goal: NewGoal {
            title: r.get(1)?,
            why: r.get(2)?,
            kind: GoalKind::depuis(&r.get::<_, String>(3)?),
            target: r.get(4)?,
            unit: r.get(5)?,
            due_day: r.get(6)?,
            color: r.get(7)?,
        },
        created_at: Timestamp::from_millis(r.get(8)?),
    })
}

const COLONNES: &str = "id, title, why, kind, target, unit, due_day, color, created_at";

impl Store {
    /// The goals, in their order: the soonest due first among those of equal rank.
    pub fn goals(&self) -> Result<Vec<Goal>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM goals ORDER BY position, due_day, id"
                ))
                .map_err(err("lecture des objectifs"))?;
            let lignes = stmt
                .query_map([], objectif)
                .map_err(err("lecture des objectifs"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des objectifs"));
            lignes
        })
    }

    pub fn goal(&self, id: i64) -> Result<Option<Goal>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("SELECT {COLONNES} FROM goals WHERE id = ?1"),
                [id],
                objectif,
            )
            .optional()
            .map_err(err("lecture d'un objectif"))
        })
    }

    pub fn create_goal(&self, g: &NewGoal, now: Timestamp) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO goals (title, why, kind, target, unit, due_day, color, position, \
                 created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, \
                 (SELECT COALESCE(MAX(position), 0) + 1 FROM goals), ?8)",
                params![
                    g.title,
                    g.why,
                    g.kind.mot(),
                    g.target.max(1),
                    g.unit,
                    g.due_day,
                    g.color,
                    now.millis()
                ],
            )
            .map_err(err("création d'un objectif"))?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn update_goal(&self, id: i64, g: &NewGoal) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE goals SET title = ?2, why = ?3, kind = ?4, target = ?5, unit = ?6, \
                 due_day = ?7, color = ?8 WHERE id = ?1",
                params![
                    id,
                    g.title,
                    g.why,
                    g.kind.mot(),
                    g.target.max(1),
                    g.unit,
                    g.due_day,
                    g.color
                ],
            )
            .map(|_| ())
            .map_err(err("mise à jour d'un objectif"))
        })
    }

    /// Deletes a goal, its log and its milestones. Its tasks and habits stay, without it.
    pub fn delete_goal(&self, id: i64) -> Result<()> {
        self.with_tx(|tx| {
            // The foreign key would do it; saying so here does not depend on it.
            tx.execute("UPDATE tasks SET goal_id = NULL WHERE goal_id = ?1", [id])
                .map_err(err("suppression d'un objectif"))?;
            tx.execute("UPDATE habits SET goal_id = NULL WHERE goal_id = ?1", [id])
                .map_err(err("suppression d'un objectif"))?;
            tx.execute("DELETE FROM goals WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'un objectif"))
        })
    }

    /// The log of a counted goal, oldest first.
    pub fn goal_entries(&self, goal: i64) -> Result<Vec<GoalEntry>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare("SELECT id, at, note FROM goal_entries WHERE goal_id = ?1 ORDER BY at, id")
                .map_err(err("journal d'un objectif"))?;
            let lignes = stmt
                .query_map([goal], |r| {
                    Ok(GoalEntry {
                        id: r.get(0)?,
                        at: Timestamp::from_millis(r.get(1)?),
                        note: r.get(2)?,
                    })
                })
                .map_err(err("journal d'un objectif"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("journal d'un objectif"));
            lignes
        })
    }

    pub fn log_goal(&self, goal: i64, note: &str, at: Timestamp) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO goal_entries (goal_id, at, note) VALUES (?1, ?2, ?3)",
                params![goal, at.millis(), note],
            )
            .map_err(err("journal d'un objectif"))?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn delete_goal_entry(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM goal_entries WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("journal d'un objectif"))
        })
    }

    /// The milestones of a goal, in order.
    pub fn milestones(&self, goal: i64) -> Result<Vec<Milestone>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, title, done_at FROM goal_milestones WHERE goal_id = ?1 \
                     ORDER BY position, id",
                )
                .map_err(err("étapes d'un objectif"))?;
            let lignes = stmt
                .query_map([goal], |r| {
                    Ok(Milestone {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        done_at: r.get::<_, Option<i64>>(2)?.map(Timestamp::from_millis),
                    })
                })
                .map_err(err("étapes d'un objectif"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("étapes d'un objectif"));
            lignes
        })
    }

    pub fn add_milestone(&self, goal: i64, title: &str) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO goal_milestones (goal_id, title, position) VALUES (?1, ?2, \
                 (SELECT COALESCE(MAX(position), 0) + 1 FROM goal_milestones WHERE goal_id = ?1))",
                params![goal, title],
            )
            .map_err(err("étapes d'un objectif"))?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn set_milestone_done(&self, id: i64, done: Option<Timestamp>) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE goal_milestones SET done_at = ?2 WHERE id = ?1",
                params![id, done.map(|t| t.millis())],
            )
            .map(|_| ())
            .map_err(err("étapes d'un objectif"))
        })
    }

    pub fn delete_milestone(&self, id: i64) -> Result<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM goal_milestones WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("étapes d'un objectif"))
        })
    }

    /// The tasks that move a goal forward, done or not, without their subtasks.
    pub fn tasks_for_goal(&self, goal: i64) -> Result<Vec<StoredTask>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {} FROM tasks WHERE goal_id = ?1 AND parent_id IS NULL \
                     ORDER BY done_at IS NOT NULL, due_day IS NULL, due_day, position, id",
                    crate::tasks::COLONNES
                ))
                .map_err(err("tâches d'un objectif"))?;
            let lignes = stmt
                .query_map([goal], crate::tasks::tache)
                .map_err(err("tâches d'un objectif"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("tâches d'un objectif"));
            lignes
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewTask;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    fn t(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    fn candidatures() -> NewGoal {
        NewGoal {
            title: "Send 10 internship applications".into(),
            kind: GoalKind::Count,
            target: 10,
            unit: "applications".into(),
            due_day: "2026-10-17".into(),
            color: "#8fa2ff".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_goal_is_written_and_read_back() {
        let s = store();
        let id = s.create_goal(&candidatures(), t(5)).unwrap();
        let g = s.goal(id).unwrap().unwrap();
        assert_eq!(g.goal, candidatures());
        assert_eq!(g.created_at, t(5));
        assert_eq!(s.goals().unwrap().len(), 1);
    }

    #[test]
    fn its_log_counts_in_order_and_an_entry_can_go() {
        let s = store();
        let g = s.create_goal(&candidatures(), t(0)).unwrap();
        let a = s.log_goal(g, "Studio Nord", t(20)).unwrap();
        s.log_goal(g, "", t(10)).unwrap();
        let log = s.goal_entries(g).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[1].note, "Studio Nord", "oldest first");
        s.delete_goal_entry(a).unwrap();
        assert_eq!(s.goal_entries(g).unwrap().len(), 1);
    }

    #[test]
    fn milestones_keep_their_order_and_tick() {
        let s = store();
        let g = s
            .create_goal(
                &NewGoal {
                    kind: GoalKind::Milestones,
                    ..candidatures()
                },
                t(0),
            )
            .unwrap();
        let a = s.add_milestone(g, "Draft").unwrap();
        s.add_milestone(g, "Review").unwrap();
        s.set_milestone_done(a, Some(t(3))).unwrap();
        let m = s.milestones(g).unwrap();
        assert_eq!(m[0].title, "Draft");
        assert_eq!(m[0].done_at, Some(t(3)));
        assert_eq!(m[1].done_at, None);
        assert_eq!(s.goal(g).unwrap().unwrap().goal.kind, GoalKind::Milestones);
    }

    #[test]
    fn a_deleted_goal_leaves_its_tasks() {
        let s = store();
        let g = s.create_goal(&candidatures(), t(0)).unwrap();
        s.log_goal(g, "", t(1)).unwrap();
        let l = s.task_lists().unwrap()[0].id;
        let tache = s
            .insert_task(
                &NewTask {
                    list_id: l,
                    title: "Write to Atelier".into(),
                    goal_id: Some(g),
                    estimate: Some(45),
                    ..Default::default()
                },
                t(2),
            )
            .unwrap();
        assert_eq!(s.tasks_for_goal(g).unwrap().len(), 1);
        s.delete_goal(g).unwrap();
        let reste = s.task(tache).unwrap().unwrap();
        assert_eq!(reste.task.goal_id, None);
        assert_eq!(reste.task.estimate, Some(45), "the rest of it is kept");
        assert!(s.goal_entries(g).unwrap().is_empty());
    }

    #[test]
    fn a_task_counts_how_often_it_was_put_off() {
        let s = store();
        let l = s.task_lists().unwrap()[0].id;
        let id = s
            .insert_task(
                &NewTask {
                    list_id: l,
                    title: "Tax form".into(),
                    ..Default::default()
                },
                t(0),
            )
            .unwrap();
        let mut tache = s.task(id).unwrap().unwrap().task;
        tache.postponed += 1;
        tache.due_day = Some("2026-10-02".into());
        s.update_task(id, &tache, t(1)).unwrap();
        assert_eq!(s.task(id).unwrap().unwrap().task.postponed, 1);
    }
}
