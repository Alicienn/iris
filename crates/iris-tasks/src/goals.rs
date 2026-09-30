//! Where a goal stands, and the shapes that show it.
//!
//! The pace is a straight line from the day the goal was set to its due day: at any
//! day, that line says how far along a steady pace would be. Ahead of it or on it is
//! on track; half a step and more behind is behind. Nothing here guesses: the numbers
//! are what the user logged.

use chrono::NaiveDate;

/// Where a goal stands against its pace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaceStatus {
    OnTrack,
    Behind,
    Reached,
}

/// A goal's pace, in numbers and in a sentence.
#[derive(Debug, Clone, PartialEq)]
pub struct Pace {
    pub status: PaceStatus,
    /// Where a steady pace would be today, in the goal's units.
    pub expected: f32,
    /// How many steps behind that pace, rounded up; 0 when on track.
    pub behind: i32,
    pub days_left: i64,
    pub left: i32,
    /// "4 to go in 17 days: about 2 a week."
    pub text: String,
}

impl Pace {
    /// The word on the badge: "On track", "2 behind", "Reached".
    pub fn badge(&self) -> String {
        match self.status {
            PaceStatus::Reached => "Reached".into(),
            PaceStatus::OnTrack => "On track".into(),
            PaceStatus::Behind => format!("{} behind", self.behind.max(1)),
        }
    }
}

/// The pace of a goal with `done` of `target`, set on `created`, due on `due`.
/// `milestones`: counted in milestones rather than in units.
pub fn pace(
    done: i32,
    target: i32,
    created: NaiveDate,
    due: NaiveDate,
    today: NaiveDate,
    milestones: bool,
) -> Pace {
    let target = target.max(1);
    let total = (due - created).num_days().max(1) as f32;
    let spent = ((today - created).num_days() as f32).clamp(0.0, total);
    let expected = target as f32 * spent / total;
    let days_left = (due - today).num_days().max(0);
    let left = (target - done).max(0);
    let status = if done >= target {
        PaceStatus::Reached
    } else if done as f32 + 0.5 >= expected {
        PaceStatus::OnTrack
    } else {
        PaceStatus::Behind
    };
    let behind = if status == PaceStatus::Behind {
        (expected - done as f32).ceil() as i32
    } else {
        0
    };
    let text = if status == PaceStatus::Reached {
        "Reached.".to_string()
    } else if days_left == 0 {
        format!("Due today, {left} to go.")
    } else if milestones {
        format!(
            "{left} milestone{} left in {days_left} day{}.",
            if left > 1 { "s" } else { "" },
            if days_left > 1 { "s" } else { "" }
        )
    } else {
        let par_semaine = left as f32 / days_left as f32 * 7.0;
        let rythme = if par_semaine < 1.5 {
            "1".to_string()
        } else {
            format!("{}", par_semaine.round() as i32)
        };
        format!(
            "{left} to go in {days_left} day{}: about {rythme} a week.",
            if days_left > 1 { "s" } else { "" }
        )
    };
    Pace {
        status,
        expected,
        behind,
        days_left,
        left,
        text,
    }
}

fn nombre(x: f32) -> String {
    // Two decimals are plenty on a 24-unit grid, and keep the path short.
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A ring filled to `fraction`, as path commands on a 16-unit grid (radius 6.5): the
/// arc from twelve o'clock, clockwise. Empty below a hair, whole from 1.
pub fn ring(fraction: f32) -> String {
    let f = fraction.clamp(0.0, 1.0);
    if f <= 0.001 {
        return String::new();
    }
    if f >= 0.999 {
        return "M 8 1.5 A 6.5 6.5 0 1 1 8 14.5 A 6.5 6.5 0 1 1 8 1.5".into();
    }
    let a = f * std::f32::consts::TAU;
    let (x, y) = (8.0 + 6.5 * a.sin(), 8.0 - 6.5 * a.cos());
    format!(
        "M 8 1.5 A 6.5 6.5 0 {} 1 {} {}",
        if f > 0.5 { 1 } else { 0 },
        nombre(x),
        nombre(y)
    )
}

/// The width and height of the pace chart's grid, and its margin.
pub const CHART_W: f32 = 520.0;
pub const CHART_H: f32 = 120.0;
const CHART_P: f32 = 6.0;

/// The pace chart: a step up at each logged day, from the day the goal was set to
/// today (or its due day, if past). `steps` are fractions of the goal's span, in any
/// order. Returns the path and where today is (x, y on the grid).
pub fn chart(steps: &[f32], target: i32, today: f32) -> (String, f32, f32) {
    let target = target.max(1) as f32;
    let x = |t: f32| CHART_P + t.clamp(0.0, 1.0) * (CHART_W - 2.0 * CHART_P);
    let y = |n: f32| CHART_H - CHART_P - (n.min(target) / target) * (CHART_H - 2.0 * CHART_P);
    let mut points: Vec<f32> = steps.to_vec();
    points.sort_by(|a, b| a.total_cmp(b));
    let mut d = format!("M {} {}", nombre(x(0.0)), nombre(y(0.0)));
    let mut n = 0.0;
    for t in points.iter().filter(|t| **t <= today.max(0.0) + 0.0001) {
        n += 1.0;
        d.push_str(&format!(" H {} V {}", nombre(x(*t)), nombre(y(n))));
    }
    let nx = x(today.min(1.0));
    d.push_str(&format!(" H {}", nombre(nx)));
    (d, nx, y(n))
}

/// Reads a length of time: "45", "45m", "45 min", "1h", "1h20", "1 h 20", "1.5h".
pub fn parse_duration(texte: &str) -> Option<i32> {
    let t: String = texte
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let t = t
        .trim_end_matches("min")
        .trim_end_matches("mn")
        .trim_end_matches('m');
    if t.is_empty() {
        return None;
    }
    if let Some((h, m)) = t.split_once('h') {
        let heures: f32 = h.replace(',', ".").parse().ok()?;
        let minutes: i32 = if m.is_empty() { 0 } else { m.parse().ok()? };
        if !(0..60).contains(&minutes) {
            return None;
        }
        let total = (heures * 60.0).round() as i32 + minutes;
        return (total > 0 && total <= 24 * 60).then_some(total);
    }
    let minutes: i32 = t.parse().ok()?;
    (minutes > 0 && minutes <= 24 * 60).then_some(minutes)
}

/// A length of time as it is shown: "45 min", "1 h", "1 h 20".
pub fn duration_label(minutes: i32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn halfway_with_half_done_is_on_track() {
        let p = pace(
            5,
            10,
            d("2026-10-01"),
            d("2026-10-21"),
            d("2026-10-11"),
            false,
        );
        assert_eq!(p.status, PaceStatus::OnTrack);
        assert_eq!(p.expected, 5.0);
        assert_eq!(p.badge(), "On track");
    }

    #[test]
    fn behind_says_by_how_many() {
        let p = pace(
            2,
            10,
            d("2026-10-01"),
            d("2026-10-21"),
            d("2026-10-11"),
            false,
        );
        assert_eq!(p.status, PaceStatus::Behind);
        assert_eq!(p.behind, 3);
        assert_eq!(p.badge(), "3 behind");
        assert_eq!(p.text, "8 to go in 10 days: about 6 a week.");
    }

    #[test]
    fn reached_is_reached_whatever_the_day() {
        let p = pace(
            10,
            10,
            d("2026-10-01"),
            d("2026-10-21"),
            d("2026-10-02"),
            false,
        );
        assert_eq!(p.status, PaceStatus::Reached);
        assert_eq!(p.text, "Reached.");
    }

    #[test]
    fn the_last_day_and_milestones_have_their_own_words() {
        let p = pace(
            1,
            3,
            d("2026-10-01"),
            d("2026-10-05"),
            d("2026-10-05"),
            true,
        );
        assert_eq!(p.text, "Due today, 2 to go.");
        let p = pace(
            1,
            3,
            d("2026-10-01"),
            d("2026-10-21"),
            d("2026-10-02"),
            true,
        );
        assert_eq!(p.text, "2 milestones left in 19 days.");
    }

    #[test]
    fn a_slow_pace_is_about_one_a_week() {
        let p = pace(
            0,
            2,
            d("2026-10-01"),
            d("2026-12-01"),
            d("2026-10-01"),
            false,
        );
        assert!(p.text.ends_with("about 1 a week."), "{}", p.text);
    }

    #[test]
    fn the_ring_goes_round() {
        assert_eq!(ring(0.0), "");
        assert_eq!(ring(0.5), "M 8 1.5 A 6.5 6.5 0 0 1 8 14.5");
        assert!(ring(0.75).contains(" 0 1 1 1.5 8"), "{}", ring(0.75));
        assert!(ring(1.0).starts_with("M 8 1.5 A"));
    }

    #[test]
    fn the_chart_steps_up_at_each_entry_until_today() {
        let (path, nx, ny) = chart(&[0.5, 0.25, 0.9], 4, 0.6);
        assert_eq!(
            path.matches(" V ").count(),
            2,
            "the one after today is not drawn"
        );
        assert_eq!(nx, 6.0 + 0.6 * 508.0);
        assert_eq!(ny, 120.0 - 6.0 - 0.5 * 108.0);
    }

    #[test]
    fn durations_read_as_people_write_them() {
        for (t, m) in [
            ("45", 45),
            ("45m", 45),
            ("45 min", 45),
            ("1h", 60),
            ("1h20", 80),
            ("1 h 20", 80),
            ("1.5h", 90),
            ("2H", 120),
        ] {
            assert_eq!(parse_duration(t), Some(m), "{t}");
        }
        for t in ["", "h", "abc", "1h75", "0", "30h"] {
            assert_eq!(parse_duration(t), None, "{t}");
        }
        assert_eq!(duration_label(45), "45 min");
        assert_eq!(duration_label(60), "1 h");
        assert_eq!(duration_label(80), "1 h 20");
    }
}
