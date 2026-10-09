//! The compass: the areas of one's life, how they are scored, and the cycles of twelve
//! weeks goals are chosen for.
//!
//! The wheel is drawn as stroked and filled paths on a 240-unit grid, its centre in the
//! middle: one axis an area, the first at twelve o'clock, clockwise. Scores go from 0
//! to 10 and are given by hand once a month.

use chrono::{Duration, NaiveDate};

/// The wheel's grid: its side, and the radius a score of 10 reaches.
pub const WHEEL: f32 = 240.0;
pub const RADIUS: f32 = 86.0;

fn nombre(x: f32) -> String {
    let s = format!("{x:.1}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// Where axis `i` of `n` is at `value` (0–10, may go past for the labels).
pub fn point(i: usize, n: usize, value: f32) -> (f32, f32) {
    let n = n.max(1) as f32;
    let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / n;
    let r = RADIUS * value / 10.0;
    (WHEEL / 2.0 + r * a.cos(), WHEEL / 2.0 + r * a.sin())
}

/// A closed shape through each axis at its score.
pub fn polygon(values: &[f32]) -> String {
    if values.len() < 3 {
        return String::new();
    }
    let n = values.len();
    let mut d = String::new();
    for (i, v) in values.iter().enumerate() {
        let (x, y) = point(i, n, v.clamp(0.0, 10.0));
        d.push_str(&format!(
            "{} {} {} ",
            if i == 0 { "M" } else { "L" },
            nombre(x),
            nombre(y)
        ));
    }
    d.push('Z');
    d
}

/// The grid behind the scores: rings at 2.5, 5, 7.5 and 10, and the spokes.
pub fn grid(n: usize) -> String {
    if n < 3 {
        return String::new();
    }
    let mut d = String::new();
    for niveau in [2.5, 5.0, 7.5, 10.0] {
        d.push_str(&polygon(&vec![niveau; n]));
        d.push(' ');
    }
    for i in 0..n {
        let (x, y) = point(i, n, 10.0);
        d.push_str(&format!(
            "M {} {} L {} {} ",
            nombre(WHEEL / 2.0),
            nombre(WHEEL / 2.0),
            nombre(x),
            nombre(y)
        ));
    }
    d.trim_end().to_string()
}

/// A cycle of weeks, from the Monday it starts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cycle {
    pub start: NaiveDate,
    pub weeks: u32,
}

impl Cycle {
    /// Its last day, a Sunday.
    pub fn end(&self) -> NaiveDate {
        self.start + Duration::days(7 * self.weeks.max(1) as i64 - 1)
    }

    /// The week under way, from 1; `None` before or after the cycle.
    pub fn week(&self, today: NaiveDate) -> Option<u32> {
        if today < self.start || today > self.end() {
            return None;
        }
        Some(((today - self.start).num_days() / 7) as u32 + 1)
    }

    /// Each week: 1 gone, 2 under way, 0 to come.
    pub fn segments(&self, today: NaiveDate) -> Vec<i32> {
        (0..self.weeks.max(1))
            .map(|w| {
                let debut = self.start + Duration::days(7 * w as i64);
                let fin = debut + Duration::days(6);
                if today > fin {
                    1
                } else if today >= debut {
                    2
                } else {
                    0
                }
            })
            .collect()
    }

    /// How far into the cycle `today` is, from 0 to 1.
    pub fn elapsed(&self, today: NaiveDate) -> f32 {
        let total = (self.end() - self.start).num_days() + 1;
        let fait = ((today - self.start).num_days() + 1).clamp(0, total);
        fait as f32 / total as f32
    }
}

/// A month as it is stored: `2026-10`.
pub fn month_key(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn the_first_axis_points_up_and_ten_reaches_the_radius() {
        let (x, y) = point(0, 6, 10.0);
        assert!((x - 120.0).abs() < 0.01 && (y - (120.0 - RADIUS)).abs() < 0.01);
        let (x, _) = point(1, 4, 10.0);
        assert!(
            (x - (120.0 + RADIUS)).abs() < 0.01,
            "clockwise: the second to the right"
        );
    }

    #[test]
    fn a_polygon_closes_and_needs_three_axes() {
        let p = polygon(&[10.0, 10.0, 10.0, 10.0]);
        assert!(p.starts_with("M 120 34 L 206 120"), "{p}");
        assert!(p.ends_with('Z'));
        assert_eq!(polygon(&[5.0, 5.0]), "");
        assert_eq!(grid(6).matches('Z').count(), 4, "four rings");
    }

    #[test]
    fn a_cycle_knows_its_week() {
        let c = Cycle {
            start: d("2026-09-07"),
            weeks: 12,
        };
        assert_eq!(c.end(), d("2026-11-29"));
        assert_eq!(c.week(d("2026-10-09")), Some(5));
        assert_eq!(c.week(d("2026-09-06")), None);
        let s = c.segments(d("2026-10-09"));
        assert_eq!(&s[..6], &[1, 1, 1, 1, 2, 0]);
        assert_eq!(month_key(d("2026-10-09")), "2026-10");
    }
}
