//! A habit: when it is due, what each of its days shows, how long it has been kept.
//!
//! A habit is only ever about today. A due day not done is missed and breaks the run,
//! but it is never owed: nothing piles up. The days done are the ones the user ticked,
//! with the amount they said; less than the habit's amount is a partial day, which
//! shows but does not keep a streak going.

use chrono::{Datelike, Duration, NaiveDate};
use std::collections::BTreeMap;

/// The days of the week, Monday first, as a habit's row heads them.
pub const DAY_LETTERS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];
const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// When a habit is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    Daily,
    /// On these days of the week, Monday first.
    Days([bool; 7]),
    /// So many days a week, whichever they are.
    PerWeek(u8),
}

impl Schedule {
    /// Reads `daily`, `days:1010100`, `week:3`. Anything else is every day.
    pub fn parse(texte: &str) -> Schedule {
        let t = texte.trim();
        if let Some(jours) = t.strip_prefix("days:") {
            let b: Vec<bool> = jours.chars().map(|c| c == '1').collect();
            if b.len() == 7 && b.iter().any(|x| *x) {
                let mut a = [false; 7];
                a.copy_from_slice(&b);
                return Schedule::days(a);
            }
        }
        if let Some(n) = t.strip_prefix("week:") {
            if let Ok(n) = n.parse::<u8>() {
                return Schedule::per_week(n);
            }
        }
        Schedule::Daily
    }

    /// On these days; all seven are every day, none is every day too.
    pub fn days(a: [bool; 7]) -> Schedule {
        if a.iter().all(|x| *x) || !a.iter().any(|x| *x) {
            Schedule::Daily
        } else {
            Schedule::Days(a)
        }
    }

    /// `n` days a week, between one and six; seven is every day.
    pub fn per_week(n: u8) -> Schedule {
        match n {
            0 => Schedule::PerWeek(1),
            1..=6 => Schedule::PerWeek(n),
            _ => Schedule::Daily,
        }
    }

    /// As it is stored.
    pub fn text(&self) -> String {
        match self {
            Schedule::Daily => "daily".into(),
            Schedule::Days(a) => format!(
                "days:{}",
                a.iter()
                    .map(|x| if *x { '1' } else { '0' })
                    .collect::<String>()
            ),
            Schedule::PerWeek(n) => format!("week:{n}"),
        }
    }

    /// Whether `day` is one the habit is wanted on. For so many a week, any day is.
    pub fn due_on(&self, day: NaiveDate) -> bool {
        match self {
            Schedule::Daily | Schedule::PerWeek(_) => true,
            Schedule::Days(a) => a[day.weekday().num_days_from_monday() as usize],
        }
    }

    /// "Every day", "Weekdays", "Mon, Wed, Fri", "3 times a week".
    pub fn label(&self) -> String {
        match self {
            Schedule::Daily => "Every day".into(),
            Schedule::Days(a) => {
                if *a == [true, true, true, true, true, false, false] {
                    "Weekdays".into()
                } else if *a == [false, false, false, false, false, true, true] {
                    "Weekends".into()
                } else {
                    (0..7)
                        .filter(|i| a[*i])
                        .map(|i| DAY_NAMES[i])
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            }
            Schedule::PerWeek(1) => "Once a week".into(),
            Schedule::PerWeek(2) => "Twice a week".into(),
            Schedule::PerWeek(n) => format!("{n} times a week"),
        }
    }

    /// Counted in weeks rather than in days.
    pub fn weekly(&self) -> bool {
        matches!(self, Schedule::PerWeek(_))
    }
}

/// What one day of a habit shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Day {
    Done,
    /// Some of the amount, as a fraction of it.
    Partial(f32),
    /// Wanted, and not done.
    Missed,
    /// Not wanted that day.
    Rest,
    /// Today, wanted and not done yet.
    Today,
    Future,
    /// Before the habit existed.
    Before,
}

/// How long a habit has been kept: now, and at best. In weeks for so many a week.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Streak {
    pub current: u32,
    pub best: u32,
    pub weeks: bool,
}

impl Streak {
    /// "12 days", "1 day", "3 weeks".
    pub fn label(&self) -> String {
        let (n, mot) = (self.current, if self.weeks { "week" } else { "day" });
        format!("{n} {mot}{}", if n == 1 { "" } else { "s" })
    }
}

/// A habit and its days, as much as the rules need.
#[derive(Debug, Clone, PartialEq)]
pub struct Habit {
    pub schedule: Schedule,
    /// How much makes a day done: 20 (pages). 0 for a habit without an amount.
    pub amount: i32,
    /// The day it was made; nothing before it counts.
    pub created: NaiveDate,
    /// Each day ticked, with the amount done (1 for a habit without an amount).
    pub checks: BTreeMap<NaiveDate, i32>,
}

/// The Monday of `d`'s week.
pub fn monday(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

impl Habit {
    fn wanted(&self) -> i32 {
        self.amount.max(1)
    }

    pub fn amount_on(&self, d: NaiveDate) -> i32 {
        self.checks.get(&d).copied().unwrap_or(0)
    }

    pub fn done_on(&self, d: NaiveDate) -> bool {
        self.amount_on(d) >= self.wanted()
    }

    fn partial_on(&self, d: NaiveDate) -> bool {
        let a = self.amount_on(d);
        a > 0 && a < self.wanted()
    }

    /// Days done in the week of `d`.
    pub fn done_in_week(&self, d: NaiveDate) -> u32 {
        let m = monday(d);
        let w = self.wanted();
        self.checks
            .range(m..=m + Duration::days(6))
            .filter(|(_, a)| **a >= w)
            .count() as u32
    }

    /// Whether today asks for it: a due day not done, or a week not reached yet.
    pub fn due_today(&self, today: NaiveDate) -> bool {
        match self.schedule {
            Schedule::PerWeek(n) => self.done_on(today) || self.done_in_week(today) < n as u32,
            s => s.due_on(today),
        }
    }

    /// What day `d` shows, seen from `today`.
    pub fn day(&self, d: NaiveDate, today: NaiveDate) -> Day {
        if d > today {
            return Day::Future;
        }
        if d < self.created {
            return Day::Before;
        }
        if self.done_on(d) {
            return Day::Done;
        }
        if self.partial_on(d) {
            return Day::Partial(self.amount_on(d) as f32 / self.wanted() as f32);
        }
        match self.schedule {
            Schedule::PerWeek(n) => {
                if d == today && self.done_in_week(d) < n as u32 {
                    Day::Today
                } else {
                    Day::Rest
                }
            }
            s if !s.due_on(d) => Day::Rest,
            _ if d == today => Day::Today,
            _ => Day::Missed,
        }
    }

    /// The run under way and the longest one.
    pub fn streak(&self, today: NaiveDate) -> Streak {
        match self.schedule {
            Schedule::PerWeek(n) => self.weeks_streak(n as u32, today),
            _ => self.days_streak(today),
        }
    }

    fn days_streak(&self, today: NaiveDate) -> Streak {
        // Today not done yet breaks nothing: the run is counted from yesterday.
        let mut d = if self.done_on(today) {
            today
        } else {
            today - Duration::days(1)
        };
        let mut current = 0;
        while d >= self.created {
            if self.schedule.due_on(d) {
                if !self.done_on(d) {
                    break;
                }
                current += 1;
            }
            d -= Duration::days(1);
        }
        let (mut run, mut best) = (0, 0);
        let mut d = self.created;
        while d <= today {
            if self.schedule.due_on(d) {
                if self.done_on(d) {
                    run += 1;
                    best = best.max(run);
                } else if d < today {
                    run = 0;
                }
            }
            d += Duration::days(1);
        }
        Streak {
            current,
            best: best.max(current),
            weeks: false,
        }
    }

    fn weeks_streak(&self, n: u32, today: NaiveDate) -> Streak {
        let cette = monday(today);
        let debut = monday(self.created);
        // The week under way counts once reached, and breaks nothing before.
        let mut current = u32::from(self.done_in_week(cette) >= n);
        let mut w = cette - Duration::days(7);
        while w >= debut {
            if self.done_in_week(w) < n {
                break;
            }
            current += 1;
            w -= Duration::days(7);
        }
        let (mut run, mut best) = (0, 0);
        let mut w = debut;
        while w <= cette {
            if self.done_in_week(w) >= n {
                run += 1;
                best = best.max(run);
            } else if w < cette {
                run = 0;
            }
            w += Duration::days(7);
        }
        Streak {
            current,
            best: best.max(current),
            weeks: true,
        }
    }

    /// How much of what was wanted between `from` and `to` was done, as a fraction;
    /// `None` when nothing was wanted (before the habit, or all rest days). Today not
    /// done yet is not counted against it.
    pub fn rate_between(&self, from: NaiveDate, to: NaiveDate, today: NaiveDate) -> Option<f32> {
        let from = from.max(self.created);
        let to = to.min(today);
        if from > to {
            return None;
        }
        let mut voulus = 0.0f32;
        let mut faits = 0.0f32;
        let jours = (to - from).num_days() + 1;
        match self.schedule {
            Schedule::PerWeek(n) => {
                voulus = n as f32 * jours as f32 / 7.0;
                let w = self.wanted();
                faits = self
                    .checks
                    .range(from..=to)
                    .filter(|(_, a)| **a >= w)
                    .count() as f32;
            }
            s => {
                let mut d = from;
                while d <= to {
                    if s.due_on(d) && !(d == today && !self.done_on(d)) {
                        voulus += 1.0;
                        if self.done_on(d) {
                            faits += 1.0;
                        }
                    }
                    d += Duration::days(1);
                }
            }
        }
        if voulus > 0.0 {
            Some((faits / voulus).min(1.0))
        } else {
            None
        }
    }

    /// Over the last thirty days, today included.
    pub fn rate(&self, today: NaiveDate) -> Option<f32> {
        self.rate_between(today - Duration::days(29), today, today)
    }

    /// Days done between `from` and `to`.
    pub fn done_between(&self, from: NaiveDate, to: NaiveDate) -> u32 {
        let w = self.wanted();
        if from > to {
            return 0;
        }
        self.checks
            .range(from..=to)
            .filter(|(_, a)| **a >= w)
            .count() as u32
    }
}

/// The first and last day of `d`'s month.
pub fn month_of(d: NaiveDate) -> (NaiveDate, NaiveDate) {
    let premier = d.with_day(1).unwrap_or(d);
    let suivant = if premier.month() == 12 {
        NaiveDate::from_ymd_opt(premier.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(premier.year(), premier.month() + 1, 1)
    };
    (
        premier,
        suivant.map(|s| s - Duration::days(1)).unwrap_or(premier),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn habitude(schedule: Schedule, depuis: &str, faits: &[&str]) -> Habit {
        Habit {
            schedule,
            amount: 0,
            created: d(depuis),
            checks: faits.iter().map(|j| (d(j), 1)).collect(),
        }
    }

    // 2026-10-09 is a Friday.
    const VENDREDI: &str = "2026-10-09";

    #[test]
    fn schedules_are_written_and_read_back() {
        for s in [
            Schedule::Daily,
            Schedule::Days([true, false, true, false, true, false, false]),
            Schedule::PerWeek(3),
        ] {
            assert_eq!(Schedule::parse(&s.text()), s);
        }
        assert_eq!(Schedule::parse("days:1111111"), Schedule::Daily);
        assert_eq!(Schedule::parse("week:7"), Schedule::Daily);
        assert_eq!(Schedule::parse("nonsense"), Schedule::Daily);
        assert_eq!(
            Schedule::Days([true, true, true, true, true, false, false]).label(),
            "Weekdays"
        );
        assert_eq!(
            Schedule::Days([true, false, true, false, true, false, false]).label(),
            "Mon, Wed, Fri"
        );
        assert_eq!(Schedule::PerWeek(3).label(), "3 times a week");
    }

    #[test]
    fn today_not_done_yet_breaks_no_streak() {
        let h = habitude(
            Schedule::Daily,
            "2026-10-01",
            &["2026-10-06", "2026-10-07", "2026-10-08"],
        );
        let s = h.streak(d(VENDREDI));
        assert_eq!((s.current, s.best), (3, 3));
        assert_eq!(h.day(d(VENDREDI), d(VENDREDI)), Day::Today);
        assert_eq!(h.day(d("2026-10-05"), d(VENDREDI)), Day::Missed);
        assert_eq!(h.day(d("2026-09-30"), d(VENDREDI)), Day::Before);
        assert_eq!(h.day(d("2026-10-10"), d(VENDREDI)), Day::Future);
    }

    #[test]
    fn a_missed_day_ends_the_run_but_the_best_is_kept() {
        let h = habitude(
            Schedule::Daily,
            "2026-10-01",
            &[
                "2026-10-01",
                "2026-10-02",
                "2026-10-03",
                "2026-10-04",
                "2026-10-07",
                "2026-10-09",
            ],
        );
        let s = h.streak(d(VENDREDI));
        assert_eq!((s.current, s.best), (1, 4));
        assert_eq!(s.label(), "1 day");
    }

    #[test]
    fn rest_days_are_skipped() {
        // Weekdays only: the weekend between does not break the run.
        let h = habitude(
            Schedule::Days([true, true, true, true, true, false, false]),
            "2026-09-28",
            &[
                "2026-10-01",
                "2026-10-02",
                "2026-10-05",
                "2026-10-06",
                "2026-10-07",
                "2026-10-08",
            ],
        );
        assert_eq!(h.streak(d(VENDREDI)).current, 6);
        assert_eq!(h.day(d("2026-10-03"), d(VENDREDI)), Day::Rest);
    }

    #[test]
    fn so_many_a_week_counts_weeks() {
        let h = habitude(
            Schedule::PerWeek(2),
            "2026-09-21",
            &[
                "2026-09-21",
                "2026-09-23",
                "2026-09-28",
                "2026-10-02",
                "2026-10-05",
            ],
        );
        // Two weeks reached, this one under way with one of two: still two.
        let s = h.streak(d(VENDREDI));
        assert_eq!((s.current, s.weeks), (2, true));
        assert_eq!(s.label(), "2 weeks");
        assert_eq!(h.done_in_week(d(VENDREDI)), 1);
        assert!(h.due_today(d(VENDREDI)));
        assert_eq!(h.day(d(VENDREDI), d(VENDREDI)), Day::Today);
        assert_eq!(h.day(d("2026-10-06"), d(VENDREDI)), Day::Rest);
    }

    #[test]
    fn less_than_the_amount_is_partial() {
        let mut h = habitude(Schedule::Daily, "2026-10-01", &[]);
        h.amount = 20;
        h.checks.insert(d("2026-10-08"), 20);
        h.checks.insert(d(VENDREDI), 10);
        assert_eq!(h.day(d(VENDREDI), d(VENDREDI)), Day::Partial(0.5));
        assert_eq!(h.day(d("2026-10-08"), d(VENDREDI)), Day::Done);
        assert_eq!(
            h.streak(d(VENDREDI)).current,
            1,
            "a partial today counts as not yet"
        );
    }

    #[test]
    fn the_rate_leaves_today_out_until_it_is_done() {
        let h = habitude(
            Schedule::Daily,
            "2026-10-05",
            &["2026-10-05", "2026-10-06", "2026-10-08"],
        );
        // Four days wanted (5 to 8), three done; today is not counted yet.
        assert_eq!(h.rate(d(VENDREDI)), Some(0.75));
        assert_eq!(
            habitude(Schedule::Daily, "2026-10-10", &[]).rate(d(VENDREDI)),
            None
        );
    }

    #[test]
    fn the_month_of_a_day() {
        assert_eq!(
            month_of(d("2026-02-14")),
            (d("2026-02-01"), d("2026-02-28"))
        );
        assert_eq!(
            month_of(d("2026-12-31")),
            (d("2026-12-01"), d("2026-12-31"))
        );
    }
}
