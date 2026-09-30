//! When to do a task: the free stretches of a day, and the days "later" means.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

/// The working day slots are looked for in, in minutes since midnight.
pub const DAY_START: i32 = 8 * 60;
pub const DAY_END: i32 = 20 * 60;

/// Up to `max` starts (minutes since midnight) where `length` minutes fit between the
/// `busy` stretches (start, end), from `from` (rounded up to a quarter hour) to the
/// end of the working day. Starts step by the length, at least half an hour apart.
pub fn free_slots(busy: &[(i32, i32)], length: i32, from: i32, max: usize) -> Vec<i32> {
    let length = length.max(5);
    let mut occupe: Vec<(i32, i32)> = busy.iter().copied().filter(|(a, b)| b > a).collect();
    occupe.sort();
    let mut a = ((from.max(DAY_START) + 14) / 15) * 15;
    let pas = length.max(30);
    let mut sortie = Vec::new();
    let prendre = |debut: i32, fin: i32, sortie: &mut Vec<i32>| {
        let mut m = debut;
        while m + length <= fin && sortie.len() < max {
            sortie.push(m);
            m += pas;
        }
    };
    for (debut, fin) in occupe {
        if debut > a {
            prendre(a, debut.min(DAY_END), &mut sortie);
        }
        a = a.max(fin);
    }
    prendre(a, DAY_END, &mut sortie);
    sortie
}

/// Where "later" puts a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Later {
    Tomorrow,
    /// The coming Saturday (a week on, from a Saturday).
    Weekend,
    /// The coming Monday.
    NextWeek,
    /// No date at all.
    Someday,
}

impl Later {
    pub fn from_word(mot: &str) -> Option<Self> {
        Some(match mot {
            "tomorrow" => Later::Tomorrow,
            "weekend" => Later::Weekend,
            "nextweek" => Later::NextWeek,
            "someday" => Later::Someday,
            _ => return None,
        })
    }

    /// The day it lands on, `None` for Someday.
    pub fn day(self, today: NaiveDate) -> Option<NaiveDate> {
        let prochain = |jour: Weekday| {
            (1..=7)
                .map(|k| today + Duration::days(k))
                .find(|d| d.weekday() == jour)
        };
        match self {
            Later::Tomorrow => Some(today + Duration::days(1)),
            Later::Weekend => prochain(Weekday::Sat),
            Later::NextWeek => prochain(Weekday::Mon),
            Later::Someday => None,
        }
    }

    /// "tomorrow", "Saturday", "next Monday", "Someday".
    pub fn label(self) -> &'static str {
        match self {
            Later::Tomorrow => "tomorrow",
            Later::Weekend => "Saturday",
            Later::NextWeek => "next Monday",
            Later::Someday => "Someday",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_fall_between_what_is_there() {
        // 9:00–10:00 and 12:30–14:00 busy, 45 minutes wanted, from 8:00.
        let s = free_slots(&[(540, 600), (750, 840)], 45, 0, 20);
        assert_eq!(s[0], 480, "8:00 fits before 9:00");
        assert!(!s.contains(&540), "not during the first");
        assert!(s.contains(&600), "10:00, right after it");
        // None overlaps 12:30–14:00: each ends before it or starts after it.
        assert!(s.iter().all(|m| m + 45 <= 750 || *m >= 840));
        assert!(s.iter().all(|m| m + 45 <= DAY_END));
    }

    #[test]
    fn slots_start_from_now_on_a_quarter_hour() {
        let s = free_slots(&[], 30, 14 * 60 + 7, 3);
        assert_eq!(s, [855, 885, 915]);
    }

    #[test]
    fn a_full_day_has_no_slot() {
        assert!(free_slots(&[(0, 24 * 60)], 30, 0, 8).is_empty());
    }

    #[test]
    fn later_lands_on_the_days_it_says() {
        let jeudi = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        assert_eq!(
            Later::Tomorrow.day(jeudi),
            NaiveDate::from_ymd_opt(2026, 10, 2)
        );
        assert_eq!(
            Later::Weekend.day(jeudi),
            NaiveDate::from_ymd_opt(2026, 10, 3)
        );
        assert_eq!(
            Later::NextWeek.day(jeudi),
            NaiveDate::from_ymd_opt(2026, 10, 5)
        );
        assert_eq!(Later::Someday.day(jeudi), None);
        let samedi = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        assert_eq!(
            Later::Weekend.day(samedi),
            NaiveDate::from_ymd_opt(2026, 10, 10)
        );
        assert_eq!(Later::from_word("nextweek"), Some(Later::NextWeek));
    }
}
