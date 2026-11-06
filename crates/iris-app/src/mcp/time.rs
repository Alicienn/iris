//! Times as an assistant writes them: the user's local time, ISO 8601.
//!
//! `2026-11-06T14:30` is read in the computer's time zone; an offset or `Z` is taken
//! as written. A day is `2026-11-06`. What the tools give back is local too, with its
//! offset, so it reads the same both ways.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use iris_mcp::ToolError;

/// Today, here.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// A day: `2026-11-06`, or `today`, `tomorrow`, `yesterday`.
pub fn parse_day(s: &str) -> Result<NaiveDate, ToolError> {
    let s = s.trim();
    match s.to_ascii_lowercase().as_str() {
        "today" => return Ok(today()),
        "tomorrow" => return Ok(today() + Duration::days(1)),
        "yesterday" => return Ok(today() - Duration::days(1)),
        _ => {}
    }
    // A full time given where a day is wanted: its day.
    let jour = s.get(..10).unwrap_or(s);
    NaiveDate::parse_from_str(jour, "%Y-%m-%d")
        .map_err(|_| ToolError::new(format!("\"{s}\" is not a day (write 2026-11-06)")))
}

/// An instant, in milliseconds since 1970 (UTC). A bare day is its midnight here.
pub fn parse_ms(s: &str) -> Result<i64, ToolError> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.timestamp_millis());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, format) {
            return local_ms(n);
        }
    }
    if let Ok(jour) = parse_day(s) {
        return day_start_ms(jour);
    }
    Err(ToolError::new(format!(
        "\"{s}\" is not a time (write 2026-11-06T14:30, local time)"
    )))
}

/// A local wall-clock time, as an instant. A time skipped by a clock change moves
/// to the next one that exists.
pub fn local_ms(n: NaiveDateTime) -> Result<i64, ToolError> {
    match Local.from_local_datetime(&n) {
        chrono::LocalResult::Single(t) => Ok(t.timestamp_millis()),
        chrono::LocalResult::Ambiguous(t, _) => Ok(t.timestamp_millis()),
        chrono::LocalResult::None => Local
            .from_local_datetime(&(n + Duration::hours(1)))
            .earliest()
            .map(|t| t.timestamp_millis())
            .ok_or_else(|| ToolError::new("that time does not exist here")),
    }
}

/// Midnight of a day, here.
pub fn day_start_ms(jour: NaiveDate) -> Result<i64, ToolError> {
    local_ms(jour.and_time(NaiveTime::MIN))
}

/// An instant as local time with its offset: `2026-11-06T14:30:00+01:00`.
pub fn format_ms(ms: i64) -> String {
    match Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
            t.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
        }
        chrono::LocalResult::None => String::new(),
    }
}

/// The local day an instant falls on.
pub fn day_of_ms(ms: i64) -> NaiveDate {
    match Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.date_naive(),
        chrono::LocalResult::None => today(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_times_go_and_come_back() {
        let ms = parse_ms("2026-11-06T14:30").unwrap();
        assert!(format_ms(ms).starts_with("2026-11-06T14:30:00"));
        assert_eq!(parse_ms(&format_ms(ms)).unwrap(), ms);
        assert_eq!(parse_ms("2026-11-06T13:30:00Z").unwrap() % 60_000, 0);
    }

    #[test]
    fn a_day_is_its_midnight() {
        let ms = parse_ms("2026-11-06").unwrap();
        assert!(format_ms(ms).starts_with("2026-11-06T00:00:00"));
        assert_eq!(day_of_ms(ms), parse_day("2026-11-06").unwrap());
        assert!(parse_day("06/11/2026").is_err());
    }
}
