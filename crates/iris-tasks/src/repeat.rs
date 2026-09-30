//! Tasks that come back: when the next one is due.

use chrono::{Datelike, Duration, Months, NaiveDate, Weekday};

/// The ways a task repeats, in the order the detail panel offers them, with their
/// stored word and their label. The first is "does not repeat".
pub const REPEATS: [(&str, &str); 6] = [
    ("", "Does not repeat"),
    ("daily", "Every day"),
    ("weekdays", "Every weekday"),
    ("weekly", "Every week"),
    ("monthly", "Every month"),
    ("yearly", "Every year"),
];

/// The index in [`REPEATS`] of a stored rule; 0 for none or one unknown.
pub fn index(rule: Option<&str>) -> usize {
    REPEATS
        .iter()
        .position(|(mot, _)| Some(*mot) == rule && !mot.is_empty())
        .unwrap_or(0)
}

/// "Every week", or "" when it does not repeat.
pub fn label(rule: Option<&str>) -> &'static str {
    match index(rule) {
        0 => "",
        i => REPEATS[i].1,
    }
}

/// The day the next one is due, after one due on `due` was done `today`.
///
/// One step of the rule from the day it was due; a task done late does not come back
/// in the past, so steps are taken until the day is after today. A month from the 31st
/// lands on the month's last day.
pub fn next_day(rule: &str, due: NaiveDate, today: NaiveDate) -> Option<NaiveDate> {
    let pas = |d: NaiveDate| -> Option<NaiveDate> {
        Some(match rule {
            "daily" => d + Duration::days(1),
            "weekdays" => {
                let mut n = d + Duration::days(1);
                while matches!(n.weekday(), Weekday::Sat | Weekday::Sun) {
                    n += Duration::days(1);
                }
                n
            }
            "weekly" => d + Duration::days(7),
            "monthly" => d.checked_add_months(Months::new(1))?,
            "yearly" => d.checked_add_months(Months::new(12))?,
            _ => return None,
        })
    };
    let mut n = pas(due)?;
    // Late: to the first one after today, with a bound for a date far in the past.
    for _ in 0..2000 {
        if n > today {
            return Some(n);
        }
        n = pas(n)?;
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn each_rule_steps_as_it_says() {
        let jeudi = d("2026-10-01");
        assert_eq!(next_day("daily", jeudi, jeudi), Some(d("2026-10-02")));
        assert_eq!(next_day("weekly", jeudi, jeudi), Some(d("2026-10-08")));
        assert_eq!(next_day("monthly", jeudi, jeudi), Some(d("2026-11-01")));
        assert_eq!(next_day("yearly", jeudi, jeudi), Some(d("2027-10-01")));
        // Friday's comes back on Monday.
        let vendredi = d("2026-10-02");
        assert_eq!(
            next_day("weekdays", vendredi, vendredi),
            Some(d("2026-10-05"))
        );
        assert_eq!(next_day("never", jeudi, jeudi), None);
    }

    #[test]
    fn a_month_from_the_31st_lands_on_the_last_day() {
        assert_eq!(
            next_day("monthly", d("2026-01-31"), d("2026-01-31")),
            Some(d("2026-02-28"))
        );
    }

    #[test]
    fn done_late_it_comes_back_after_today() {
        // Due every Monday, done on Thursday ten days later: next Monday, not a past
        // one.
        assert_eq!(
            next_day("weekly", d("2026-09-21"), d("2026-10-01")),
            Some(d("2026-10-05"))
        );
    }

    #[test]
    fn rules_are_read_back() {
        assert_eq!(index(Some("weekly")), 3);
        assert_eq!(index(None), 0);
        assert_eq!(index(Some("")), 0);
        assert_eq!(label(Some("monthly")), "Every month");
        assert_eq!(label(None), "");
    }
}
