//! Habits, and the days they were kept.
//!
//! A habit says when it is wanted (`schedule`, read by `iris-growth`) and how much
//! makes a day done. A day is kept by the user ticking it; nothing is counted on their
//! behalf.

use crate::Store;
use iris_types::{Error, Result, Timestamp};
use rusqlite::{params, OptionalExtension, Row};

/// A habit, as it is written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewHabit {
    pub title: String,
    /// A word among the icons the interface knows: `book`, `run`, `drop`…
    pub icon: String,
    /// `#rrggbb`.
    pub color: String,
    /// Done less of: ticked when the day was kept.
    pub quit: bool,
    /// `daily`, `days:1010100`, `week:3`.
    pub schedule: String,
    /// What makes a day done; 0 for a habit without an amount.
    pub amount: i32,
    pub unit: String,
    /// The goal it supports.
    pub goal_id: Option<i64>,
    /// The minute of the day to be reminded at if not done.
    pub remind_minute: Option<i32>,
}

/// A habit, as it is read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Habit {
    pub id: i64,
    pub habit: NewHabit,
    pub created_at: Timestamp,
    /// The last day (`YYYY-MM-DD`) its reminder was given.
    pub reminded_day: Option<String>,
}

/// A day a habit was kept, and how much was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HabitCheck {
    pub habit_id: i64,
    /// `YYYY-MM-DD`.
    pub day: String,
    pub amount: i32,
}

fn err(quoi: &str) -> impl Fn(rusqlite::Error) -> Error + '_ {
    move |e| Error::store(format!("{quoi} : {e}"))
}

const COLONNES: &str = "id, title, icon, color, kind, schedule, amount, unit, goal_id, \
     remind_minute, created_at, reminded_day";

fn habitude(r: &Row<'_>) -> rusqlite::Result<Habit> {
    Ok(Habit {
        id: r.get(0)?,
        habit: NewHabit {
            title: r.get(1)?,
            icon: r.get(2)?,
            color: r.get(3)?,
            quit: r.get::<_, String>(4)? == "quit",
            schedule: r.get(5)?,
            amount: r.get(6)?,
            unit: r.get(7)?,
            goal_id: r.get(8)?,
            remind_minute: r.get(9)?,
        },
        created_at: Timestamp::from_millis(r.get(10)?),
        reminded_day: r.get(11)?,
    })
}

fn genre(quit: bool) -> &'static str {
    if quit {
        "quit"
    } else {
        "build"
    }
}

impl Store {
    /// The habits, in their order.
    pub fn habits(&self) -> Result<Vec<Habit>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLONNES} FROM habits WHERE archived_at IS NULL \
                     ORDER BY position, id"
                ))
                .map_err(err("lecture des habitudes"))?;
            let lignes = stmt
                .query_map([], habitude)
                .map_err(err("lecture des habitudes"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("lecture des habitudes"));
            lignes
        })
    }

    pub fn habit(&self, id: i64) -> Result<Option<Habit>> {
        self.with_conn(|c| {
            c.query_row(
                &format!("SELECT {COLONNES} FROM habits WHERE id = ?1"),
                [id],
                habitude,
            )
            .optional()
            .map_err(err("lecture d'une habitude"))
        })
    }

    pub fn create_habit(&self, h: &NewHabit, now: Timestamp) -> Result<i64> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO habits (title, icon, color, kind, schedule, amount, unit, goal_id, \
                 remind_minute, position, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, \
                 (SELECT COALESCE(MAX(position), 0) + 1 FROM habits), ?10)",
                params![
                    h.title,
                    h.icon,
                    h.color,
                    genre(h.quit),
                    h.schedule,
                    h.amount.max(0),
                    h.unit,
                    h.goal_id,
                    h.remind_minute,
                    now.millis()
                ],
            )
            .map_err(err("création d'une habitude"))?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn update_habit(&self, id: i64, h: &NewHabit) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE habits SET title = ?2, icon = ?3, color = ?4, kind = ?5, schedule = ?6, \
                 amount = ?7, unit = ?8, goal_id = ?9, remind_minute = ?10 WHERE id = ?1",
                params![
                    id,
                    h.title,
                    h.icon,
                    h.color,
                    genre(h.quit),
                    h.schedule,
                    h.amount.max(0),
                    h.unit,
                    h.goal_id,
                    h.remind_minute
                ],
            )
            .map(|_| ())
            .map_err(err("mise à jour d'une habitude"))
        })
    }

    /// Deletes a habit and the days it was kept.
    pub fn delete_habit(&self, id: i64) -> Result<()> {
        self.with_tx(|tx| {
            // The foreign key would do it; saying so here does not depend on it.
            tx.execute("DELETE FROM habit_checks WHERE habit_id = ?1", [id])
                .map_err(err("suppression d'une habitude"))?;
            tx.execute("DELETE FROM habits WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("suppression d'une habitude"))
        })
    }

    /// Every day kept, of every habit, oldest first.
    pub fn habit_checks(&self) -> Result<Vec<HabitCheck>> {
        self.with_conn(|c| {
            let mut stmt = c
                .prepare("SELECT habit_id, day, amount FROM habit_checks ORDER BY habit_id, day")
                .map_err(err("jours des habitudes"))?;
            let lignes = stmt
                .query_map([], |r| {
                    Ok(HabitCheck {
                        habit_id: r.get(0)?,
                        day: r.get(1)?,
                        amount: r.get(2)?,
                    })
                })
                .map_err(err("jours des habitudes"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err("jours des habitudes"));
            lignes
        })
    }

    /// What was done of habit `id` on `day`: an amount, or nothing (0 or less).
    pub fn set_habit_check(&self, id: i64, day: &str, amount: i32) -> Result<()> {
        self.with_conn(|c| {
            if amount <= 0 {
                c.execute(
                    "DELETE FROM habit_checks WHERE habit_id = ?1 AND day = ?2",
                    params![id, day],
                )
            } else {
                c.execute(
                    "INSERT INTO habit_checks (habit_id, day, amount) VALUES (?1, ?2, ?3) \
                     ON CONFLICT (habit_id, day) DO UPDATE SET amount = excluded.amount",
                    params![id, day, amount],
                )
            }
            .map(|_| ())
            .map_err(err("jour d'une habitude"))
        })
    }

    /// Says habit `id`'s reminder was given on `day`.
    pub fn set_habit_reminded(&self, id: i64, day: &str) -> Result<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE habits SET reminded_day = ?2 WHERE id = ?1",
                params![id, day],
            )
            .map(|_| ())
            .map_err(err("rappel d'une habitude"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewGoal;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    fn lire() -> NewHabit {
        NewHabit {
            title: "Read".into(),
            icon: "book".into(),
            color: "#ff9500".into(),
            schedule: "daily".into(),
            amount: 20,
            unit: "pages".into(),
            remind_minute: Some(21 * 60 + 30),
            ..Default::default()
        }
    }

    #[test]
    fn a_habit_is_written_and_read_back() {
        let s = store();
        let id = s.create_habit(&lire(), Timestamp::from_millis(5)).unwrap();
        let h = s.habit(id).unwrap().unwrap();
        assert_eq!(h.habit, lire());
        assert_eq!(h.created_at, Timestamp::from_millis(5));
        let mut moins = lire();
        moins.title = "No phone in bed".into();
        moins.quit = true;
        s.update_habit(id, &moins).unwrap();
        assert!(s.habit(id).unwrap().unwrap().habit.quit);
        assert_eq!(s.habits().unwrap().len(), 1);
    }

    #[test]
    fn a_day_is_ticked_changed_and_unticked() {
        let s = store();
        let id = s.create_habit(&lire(), Timestamp::EPOCH).unwrap();
        s.set_habit_check(id, "2026-10-09", 20).unwrap();
        s.set_habit_check(id, "2026-10-09", 12).unwrap();
        s.set_habit_check(id, "2026-10-08", 20).unwrap();
        let jours = s.habit_checks().unwrap();
        assert_eq!(jours.len(), 2);
        assert_eq!((jours[1].day.as_str(), jours[1].amount), ("2026-10-09", 12));
        s.set_habit_check(id, "2026-10-09", 0).unwrap();
        assert_eq!(s.habit_checks().unwrap().len(), 1);
        s.set_habit_reminded(id, "2026-10-09").unwrap();
        assert_eq!(
            s.habit(id).unwrap().unwrap().reminded_day.as_deref(),
            Some("2026-10-09")
        );
    }

    #[test]
    fn a_deleted_habit_takes_its_days_and_a_deleted_goal_leaves_the_habit() {
        let s = store();
        let g = s
            .create_goal(
                &NewGoal {
                    title: "Read 12 books".into(),
                    target: 12,
                    due_day: "2026-12-31".into(),
                    color: "#ff9500".into(),
                    ..Default::default()
                },
                Timestamp::EPOCH,
            )
            .unwrap();
        let id = s
            .create_habit(
                &NewHabit {
                    goal_id: Some(g),
                    ..lire()
                },
                Timestamp::EPOCH,
            )
            .unwrap();
        s.set_habit_check(id, "2026-10-09", 20).unwrap();
        s.delete_goal(g).unwrap();
        assert_eq!(s.habit(id).unwrap().unwrap().habit.goal_id, None);
        s.delete_habit(id).unwrap();
        assert!(s.habit(id).unwrap().is_none());
        assert!(s.habit_checks().unwrap().is_empty());
    }
}
