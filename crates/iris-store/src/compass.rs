//! The compass: the areas of one's life, their monthly scores, the cycles of weeks and
//! the goals chosen for each, and a page of one's own words (the vision).
//!
//! An area gathers goals, habits and task lists; nothing is filed into one on the
//! user's behalf. A score is given by hand, once a month.

use crate::Store;
use iris_types::{Error, Result, Timestamp};
use rusqlite::{params, OptionalExtension};

/// An area of one's life: "Health", "Studies & career".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Area {
    pub id: i64,
    pub name: String,
    /// A word among the habits' icons.
    pub icon: String,
    /// `#rrggbb`.
    pub color: String,
    /// The hours a month one wants to give it, in minutes; 0 for none said.
    pub wanted_minutes: i32,
}

/// A cycle of weeks with the goals chosen for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCycle {
    pub id: i64,
    pub name: String,
    /// `YYYY-MM-DD`, a Monday.
    pub start_day: String,
    pub weeks: i32,
    pub goals: Vec<i64>,
}

fn err(quoi: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::store(format!("{quoi} : {e}"))
}

/// What an area holds: its goals, habits or task lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaOf {
    Goal,
    Habit,
    TaskList,
}

impl AreaOf {
    fn table(self) -> &'static str {
        match self {
            AreaOf::Goal => "goals",
            AreaOf::Habit => "habits",
            AreaOf::TaskList => "task_lists",
        }
    }
}

impl Store {
    /// The areas, in their order.
    pub fn areas(&self) -> Result<Vec<Area>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, name, icon, color, wanted_minutes FROM areas ORDER BY position, id",
                )
                .map_err(err("lecture des domaines"))?;
            let lignes = stmt
                .query_map([], |r| {
                    Ok(Area {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        icon: r.get(2)?,
                        color: r.get(3)?,
                        wanted_minutes: r.get(4)?,
                    })
                })
                .map_err(err("lecture des domaines"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des domaines"));
            lignes
        })
    }

    pub fn create_area(&self, name: &str, icon: &str, color: &str) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO areas (name, icon, color, position) VALUES (?1, ?2, ?3, \
                 (SELECT COALESCE(MAX(position), 0) + 1 FROM areas))",
                params![name, icon, color],
            )
            .map_err(err("création d'un domaine"))?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn update_area(&self, a: &Area) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE areas SET name = ?2, icon = ?3, color = ?4, wanted_minutes = ?5 \
                 WHERE id = ?1",
                params![a.id, a.name, a.icon, a.color, a.wanted_minutes.max(0)],
            )
            .map(|_| ())
            .map_err(err("mise à jour d'un domaine"))
        })
    }

    /// Deletes an area and its scores; its goals, habits and lists stay, without it.
    pub fn delete_area(&self, id: i64) -> Result<()> {
        self.with_tx(|tx| {
            for table in ["goals", "habits", "task_lists"] {
                tx.execute(
                    &format!("UPDATE {table} SET area_id = NULL WHERE area_id = ?1"),
                    [id],
                )
                .map_err(err("suppression d'un domaine"))?;
            }
            tx.execute("DELETE FROM area_scores WHERE area_id = ?1", [id])
                .map_err(err("suppression d'un domaine"))?;
            tx.execute("DELETE FROM areas WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'un domaine"))
        })
    }

    /// The area a goal, a habit or a task list belongs to.
    pub fn area_of(&self, what: AreaOf, id: i64) -> Result<Option<i64>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("SELECT area_id FROM {} WHERE id = ?1", what.table()),
                [id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(err("domaine"))
        })
    }

    pub fn set_area_of(&self, what: AreaOf, id: i64, area: Option<i64>) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                &format!("UPDATE {} SET area_id = ?2 WHERE id = ?1", what.table()),
                params![id, area],
            )
            .map(|_| ())
            .map_err(err("domaine"))
        })
    }

    /// Every (thing, area) pair of one kind: which goals, habits or lists are in an
    /// area.
    pub fn areas_of(&self, what: AreaOf) -> Result<Vec<(i64, i64)>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT id, area_id FROM {} WHERE area_id IS NOT NULL",
                    what.table()
                ))
                .map_err(err("domaines"))?;
            let lignes = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(err("domaines"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("domaines"));
            lignes
        })
    }

    /// The scores given in `month` (`YYYY-MM`), by area.
    pub fn area_scores(&self, month: &str) -> Result<Vec<(i64, i32)>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare("SELECT area_id, score FROM area_scores WHERE month = ?1")
                .map_err(err("notes des domaines"))?;
            let lignes = stmt
                .query_map([month], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(err("notes des domaines"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("notes des domaines"));
            lignes
        })
    }

    /// The last month scores were given in, up to `month` included.
    pub fn last_scored_month(&self, month: &str) -> Result<Option<String>> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT MAX(month) FROM area_scores WHERE month <= ?1",
                [month],
                |r| r.get::<_, Option<String>>(0),
            )
            .map_err(err("notes des domaines"))
        })
    }

    /// Area `area`'s score for `month`, 1 to 10; 0 takes it back.
    pub fn set_area_score(&self, area: i64, month: &str, score: i32, at: Timestamp) -> Result<()> {
        self.with_conn(|c| {
            if score <= 0 {
                c.execute(
                    "DELETE FROM area_scores WHERE area_id = ?1 AND month = ?2",
                    params![area, month],
                )
            } else {
                c.execute(
                    "INSERT INTO area_scores (area_id, month, score, at) VALUES (?1, ?2, ?3, ?4) \
                     ON CONFLICT (area_id, month) DO UPDATE SET score = excluded.score, \
                     at = excluded.at",
                    params![area, month, score.min(10), at.millis()],
                )
            }
            .map(|_| ())
            .map_err(err("note d'un domaine"))
        })
    }

    /// The cycles, the latest first.
    pub fn cycles(&self) -> Result<Vec<StoredCycle>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare("SELECT id, name, start_day, weeks FROM cycles ORDER BY start_day DESC, id DESC")
                .map_err(err("lecture des cycles"))?;
            let mut cycles = stmt
                .query_map([], |r| {
                    Ok(StoredCycle {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        start_day: r.get(2)?,
                        weeks: r.get(3)?,
                        goals: Vec::new(),
                    })
                })
                .map_err(err("lecture des cycles"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des cycles"))?;
            let mut stmt = c
                .prepare("SELECT goal_id FROM cycle_goals WHERE cycle_id = ?1 ORDER BY goal_id")
                .map_err(err("lecture des cycles"))?;
            for cy in &mut cycles {
                cy.goals = stmt
                    .query_map([cy.id], |r| r.get(0))
                    .map_err(err("lecture des cycles"))?
                    .collect::<rusqlite::Result<Vec<i64>>>()
                    .map_err(err("lecture des cycles"))?;
            }
            Ok(cycles)
        })
    }

    /// Makes a cycle, or changes cycle `id`, with its goals.
    pub fn save_cycle(
        &self,
        id: Option<i64>,
        name: &str,
        start_day: &str,
        weeks: i32,
        goals: &[i64],
        now: Timestamp,
    ) -> Result<i64> {
        self.with_tx(|tx| {
            let id = match id {
                Some(id) => {
                    tx.execute(
                        "UPDATE cycles SET name = ?2, start_day = ?3, weeks = ?4 WHERE id = ?1",
                        params![id, name, start_day, weeks.clamp(1, 52)],
                    )
                    .map_err(err("enregistrement d'un cycle"))?;
                    id
                }
                None => {
                    tx.execute(
                        "INSERT INTO cycles (name, start_day, weeks, created_at) \
                         VALUES (?1, ?2, ?3, ?4)",
                        params![name, start_day, weeks.clamp(1, 52), now.millis()],
                    )
                    .map_err(err("enregistrement d'un cycle"))?;
                    tx.last_insert_rowid()
                }
            };
            tx.execute("DELETE FROM cycle_goals WHERE cycle_id = ?1", [id])
                .map_err(err("enregistrement d'un cycle"))?;
            for g in goals {
                tx.execute(
                    "INSERT OR IGNORE INTO cycle_goals (cycle_id, goal_id) VALUES (?1, ?2)",
                    params![id, g],
                )
                .map_err(err("enregistrement d'un cycle"))?;
            }
            Ok(id)
        })
    }

    pub fn delete_cycle(&self, id: i64) -> Result<()> {
        self.with_tx(|tx| {
            tx.execute("DELETE FROM cycle_goals WHERE cycle_id = ?1", [id])
                .map_err(err("suppression d'un cycle"))?;
            tx.execute("DELETE FROM cycles WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'un cycle"))
        })
    }

    /// One's own words: who one wants to become.
    pub fn vision(&self) -> Result<String> {
        self.with_conn(|c| {
            c.query_row("SELECT text FROM vision WHERE id = 1", [], |r| r.get(0))
                .optional()
                .map(Option::unwrap_or_default)
                .map_err(err("vision"))
        })
    }

    pub fn set_vision(&self, text: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO vision (id, text) VALUES (1, ?1) \
                 ON CONFLICT (id) DO UPDATE SET text = excluded.text",
                [text],
            )
            .map(|_| ())
            .map_err(err("vision"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NewGoal, NewHabit};

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    #[test]
    fn six_areas_come_first_and_can_be_changed() {
        let s = store();
        let a = s.areas().unwrap();
        assert_eq!(a.len(), 6);
        assert_eq!(a[0].name, "Studies & career");
        let mut sante = a[1].clone();
        sante.wanted_minutes = 8 * 60;
        s.update_area(&sante).unwrap();
        assert_eq!(s.areas().unwrap()[1].wanted_minutes, 480);
        let id = s.create_area("Family", "heart", "#ff2d55").unwrap();
        assert_eq!(s.areas().unwrap().last().unwrap().id, id);
    }

    #[test]
    fn a_score_a_month_and_the_last_month_scored() {
        let s = store();
        let a = s.areas().unwrap()[0].id;
        s.set_area_score(a, "2026-09", 6, Timestamp::EPOCH).unwrap();
        s.set_area_score(a, "2026-10", 8, Timestamp::EPOCH).unwrap();
        s.set_area_score(a, "2026-10", 7, Timestamp::EPOCH).unwrap();
        assert_eq!(s.area_scores("2026-10").unwrap(), [(a, 7)]);
        assert_eq!(
            s.last_scored_month("2026-09").unwrap().as_deref(),
            Some("2026-09")
        );
        s.set_area_score(a, "2026-10", 0, Timestamp::EPOCH).unwrap();
        assert!(s.area_scores("2026-10").unwrap().is_empty());
    }

    #[test]
    fn what_an_area_holds_stays_when_it_goes() {
        let s = store();
        let a = s.areas().unwrap()[1].id;
        let g = s
            .create_goal(
                &NewGoal {
                    title: "Run a 10 km".into(),
                    target: 5,
                    due_day: "2026-12-01".into(),
                    color: "#34c759".into(),
                    ..Default::default()
                },
                Timestamp::EPOCH,
            )
            .unwrap();
        let h = s
            .create_habit(
                &NewHabit {
                    title: "Run".into(),
                    schedule: "week:3".into(),
                    color: "#34c759".into(),
                    ..Default::default()
                },
                Timestamp::EPOCH,
            )
            .unwrap();
        s.set_area_of(AreaOf::Goal, g, Some(a)).unwrap();
        s.set_area_of(AreaOf::Habit, h, Some(a)).unwrap();
        assert_eq!(s.area_of(AreaOf::Goal, g).unwrap(), Some(a));
        assert_eq!(s.areas_of(AreaOf::Habit).unwrap(), [(h, a)]);
        s.delete_area(a).unwrap();
        assert_eq!(s.area_of(AreaOf::Goal, g).unwrap(), None);
        assert!(s.goal(g).unwrap().is_some());
    }

    #[test]
    fn a_cycle_keeps_its_goals_and_the_vision_its_words() {
        let s = store();
        let g = s
            .create_goal(
                &NewGoal {
                    title: "10 applications".into(),
                    target: 10,
                    due_day: "2026-11-29".into(),
                    color: "#af52de".into(),
                    ..Default::default()
                },
                Timestamp::EPOCH,
            )
            .unwrap();
        let c = s
            .save_cycle(
                None,
                "Land the internship",
                "2026-09-07",
                12,
                &[g],
                Timestamp::EPOCH,
            )
            .unwrap();
        let cy = &s.cycles().unwrap()[0];
        assert_eq!((cy.id, cy.weeks, cy.goals.clone()), (c, 12, vec![g]));
        s.save_cycle(Some(c), "Land it", "2026-09-07", 12, &[], Timestamp::EPOCH)
            .unwrap();
        assert!(s.cycles().unwrap()[0].goals.is_empty());
        s.delete_cycle(c).unwrap();
        assert!(s.cycles().unwrap().is_empty());
        assert_eq!(s.vision().unwrap(), "");
        s.set_vision("Calm, curious, useful.").unwrap();
        assert_eq!(s.vision().unwrap(), "Calm, curious, useful.");
    }
}
