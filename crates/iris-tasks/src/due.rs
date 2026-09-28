//! Quand une tâche est due : en mots, en section, en rappel.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};

/// Où une tâche se range dans une liste, selon son échéance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Overdue,
    Today,
    Tomorrow,
    /// Les cinq jours qui suivent demain.
    NextDays,
    Later,
    NoDate,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Overdue,
        Section::Today,
        Section::Tomorrow,
        Section::NextDays,
        Section::Later,
        Section::NoDate,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Overdue => "Overdue",
            Section::Today => "Today",
            Section::Tomorrow => "Tomorrow",
            Section::NextDays => "Next days",
            Section::Later => "Later",
            Section::NoDate => "No date",
        }
    }
}

/// La section d'une tâche due `day` (ou sans date), vue depuis `today`.
pub fn section(day: Option<NaiveDate>, today: NaiveDate) -> Section {
    let Some(day) = day else {
        return Section::NoDate;
    };
    match (day - today).num_days() {
        d if d < 0 => Section::Overdue,
        0 => Section::Today,
        1 => Section::Tomorrow,
        2..=6 => Section::NextDays,
        _ => Section::Later,
    }
}

/// Une échéance en mots : « Today 09:30 », « Tomorrow », « Friday », « Mon 12 Oct »,
/// « 3 Jan 2027 ».
pub fn due_label(day: NaiveDate, minute: Option<u32>, today: NaiveDate) -> String {
    let jour = match (day - today).num_days() {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        2..=6 => day.format("%A").to_string(),
        _ if day.year() == today.year() => day.format("%a %-d %b").to_string(),
        _ => day.format("%-d %b %Y").to_string(),
    };
    match minute {
        Some(m) => format!("{jour} {:02}:{:02}", m / 60, m % 60),
        None => jour,
    }
}

/// Une tâche est-elle en retard à `now` ? Sans heure, elle l'est le lendemain.
pub fn is_overdue(day: NaiveDate, minute: Option<u32>, now: NaiveDateTime) -> bool {
    let today = now.date();
    if day != today {
        return day < today;
    }
    match minute {
        Some(m) => (now.time() - NaiveTime::MIN).num_minutes() > m as i64,
        None => false,
    }
}

/// Les rappels proposés, en minutes avant l'échéance.
pub const REMINDERS: [(&str, Option<i64>); 5] = [
    ("No reminder", None),
    ("At the due time", Some(0)),
    ("15 minutes before", Some(15)),
    ("1 hour before", Some(60)),
    ("1 day before", Some(1440)),
];

/// L'heure où l'on rappelle une tâche : `before` minutes avant son échéance. Une
/// tâche sans heure est rappelée à partir de neuf heures, le début d'une journée.
pub fn remind_at(day: NaiveDate, minute: Option<u32>, before: i64) -> NaiveDateTime {
    let m = minute.unwrap_or(9 * 60);
    let heure = NaiveTime::from_hms_opt(m / 60, m % 60, 0).unwrap_or(NaiveTime::MIN);
    day.and_time(heure) - Duration::minutes(before)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn sections_follow_the_calendar() {
        let today = d("2026-09-28");
        assert_eq!(section(Some(d("2026-09-27")), today), Section::Overdue);
        assert_eq!(section(Some(today), today), Section::Today);
        assert_eq!(section(Some(d("2026-09-29")), today), Section::Tomorrow);
        assert_eq!(section(Some(d("2026-10-04")), today), Section::NextDays);
        assert_eq!(section(Some(d("2026-10-05")), today), Section::Later);
        assert_eq!(section(None, today), Section::NoDate);
    }

    #[test]
    fn labels_say_the_day_as_people_do() {
        let today = d("2026-09-28");
        assert_eq!(due_label(today, Some(9 * 60 + 5), today), "Today 09:05");
        assert_eq!(due_label(d("2026-09-29"), None, today), "Tomorrow");
        assert_eq!(due_label(d("2026-09-27"), None, today), "Yesterday");
        assert_eq!(due_label(d("2026-10-02"), None, today), "Friday");
        assert_eq!(due_label(d("2026-10-12"), None, today), "Mon 12 Oct");
        assert_eq!(due_label(d("2027-01-03"), None, today), "3 Jan 2027");
    }

    #[test]
    fn a_task_without_a_time_is_late_only_the_next_day() {
        let now = d("2026-09-28").and_hms_opt(23, 0, 0).unwrap();
        assert!(!is_overdue(d("2026-09-28"), None, now));
        assert!(is_overdue(d("2026-09-28"), Some(22 * 60), now));
        assert!(is_overdue(d("2026-09-27"), None, now));
        assert!(!is_overdue(d("2026-09-29"), Some(0), now));
    }

    #[test]
    fn reminders_come_before_the_due_time() {
        assert_eq!(
            remind_at(d("2026-09-28"), Some(10 * 60), 15),
            d("2026-09-28").and_hms_opt(9, 45, 0).unwrap()
        );
        assert_eq!(
            remind_at(d("2026-09-28"), None, 0),
            d("2026-09-28").and_hms_opt(9, 0, 0).unwrap()
        );
        assert_eq!(
            remind_at(d("2026-09-28"), None, 1440),
            d("2026-09-27").and_hms_opt(9, 0, 0).unwrap()
        );
    }
}
