//! VIP, Office hours and Filer: the three sorters that come with Iris.
//!
//! They were WebAssembly plugins once. They are part of Iris now, and like the rest of
//! this crate they only decide: the sync's rules pass carries out what they say, through
//! the workflow, so each of their moves can be undone like any other.
//!
//! Each one does nothing until it is given something to do. An empty list names nobody,
//! an empty set of filing lines files nothing, and the office hours start switched off:
//! moving someone's mail on the evening they update, without being asked, would be the
//! worst way to meet them.

use iris_types::Timestamp;
use serde::{Deserialize, Serialize};

/// The three sorters' settings, as the settings file keeps them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Triage {
    /// VIP: the people who matter, one address or `@domain` per line (commas and
    /// semicolons separate too). Their mail is pinned and put back to do.
    pub vip: String,
    /// Office hours: mail arriving outside them waits for the next working morning.
    pub office_hours: OfficeHours,
    /// Filer: one `pattern -> Folder` line per rule, in order.
    pub filer: String,
}

impl Triage {
    /// The VIP list, read from what was written.
    pub fn vip_list(&self) -> Vec<String> {
        list_of(&self.vip)
    }

    /// Is this sender one of the people who matter?
    pub fn is_vip(&self, from: &str) -> bool {
        self.vip_list().iter().any(|e| address_matches(from, e))
    }

    /// The filing lines that can be read, in order.
    pub fn filer_rules(&self) -> Vec<FilerRule> {
        parse_filer(&self.filer)
    }

    /// Nothing to do for any of the three.
    pub fn is_idle(&self) -> bool {
        self.vip_list().is_empty() && !self.office_hours.enabled && self.filer_rules().is_empty()
    }

    /// Brings odd values back into range: hours to 0–23, and the lines trimmed.
    pub fn sanitize(mut self) -> Self {
        self.office_hours.start = self.office_hours.start.min(23);
        self.office_hours.end = self.office_hours.end.min(23);
        self.vip = self.vip.trim().to_string();
        self.filer = self.filer.trim().to_string();
        self
    }
}

/// When the working day is, in UTC.
///
/// Iris knows no time zone for this, and inventing one would be worse than saying so:
/// the settings say the hours are UTC, and someone in Paris writes 7 for 8 in winter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OfficeHours {
    /// Off by default. The hours below are plausible from the start, so without this
    /// switch Iris would begin moving evening mail out of sight on its own.
    pub enabled: bool,
    /// The first working hour, included.
    pub start: u8,
    /// The first hour after work, excluded. Not after the start: work runs until
    /// midnight (visible, and easy to put right).
    pub end: u8,
    /// Saturday and Sunday count as outside working hours.
    pub weekend: bool,
}

impl Default for OfficeHours {
    fn default() -> Self {
        Self {
            enabled: false,
            start: 8,
            end: 18,
            weekend: true,
        }
    }
}

impl OfficeHours {
    /// How many hours until the next working hour, or `None` when this is one.
    ///
    /// `hour` is 0–23, `day` 0 (Monday) to 6 (Sunday). Pure, so Friday evening, midnight
    /// and the weekend can be checked without waiting for them.
    pub fn hours_until_open(&self, hour: i64, day: i64) -> Option<i64> {
        if !self.enabled {
            return None;
        }
        let start = i64::from(self.start.min(23));
        let end = i64::from(self.end.min(23));
        // A day that ends before it starts runs until midnight.
        let end = if end > start { end } else { 24 };

        let open = |day: i64| !(self.weekend && day >= 5);

        if open(day) && hour >= start && hour < end {
            return None;
        }
        // Before opening, the same day: wait for the start.
        if open(day) && hour < start {
            return Some(start - hour);
        }
        // Otherwise the next working day. Seven tries cover the week; past them every
        // day is closed, which a week's wait would not solve either.
        let mut wait = 24 - hour + start;
        let mut next = (day + 1).rem_euclid(7);
        for _ in 0..7 {
            if open(next) {
                return Some(wait);
            }
            wait += 24;
            next = (next + 1).rem_euclid(7);
        }
        None
    }

    /// When mail received at `received` should come back: the start of the next
    /// working hour, or `None` when it arrived during working hours.
    pub fn reopens_at(&self, received: Timestamp) -> Option<Timestamp> {
        let seconds = received.seconds();
        let hour = seconds.rem_euclid(86_400) / 3600;
        // 1 January 1970 was a Thursday.
        let day = (seconds.div_euclid(86_400) + 3).rem_euclid(7);
        let wait = self.hours_until_open(hour, day)?;
        let start_of_hour = seconds - seconds.rem_euclid(3600);
        Some(Timestamp::from_millis((start_of_hour + wait * 3600) * 1000))
    }
}

/// One filing line: a pattern and where it sends the thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilerRule {
    /// Looked for in the subject and the attachments' names, whatever the case.
    pub pattern: String,
    /// The folder, by the name the folders column shows. It is never created.
    pub folder: String,
}

/// Reads the filing lines: `pattern -> Folder`, one per line.
///
/// Spaces around the arrow, blank lines and `#` comments are tolerated, and `→` works
/// as well as `->` (a French keyboard makes it). A line without an arrow, a pattern or
/// a folder is skipped on its own: a typo on the third line must not switch off the
/// first two.
pub fn parse_filer(text: &str) -> Vec<FilerRule> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let (pattern, folder) = line.split_once("->").or_else(|| line.split_once('→'))?;
            let (pattern, folder) = (pattern.trim(), folder.trim());
            (!pattern.is_empty() && !folder.is_empty()).then(|| FilerRule {
                pattern: pattern.to_string(),
                folder: folder.to_string(),
            })
        })
        .collect()
}

/// Where a message should be filed, if anywhere: the first line that matches wins.
///
/// The order written decides, rather than "the most precise", which would ask people to
/// guess what Iris finds precise. A substring and not a regular expression: what does
/// not show here moves mail, and a substring does exactly what it says.
pub fn filer_destination<'a>(
    subject: &str,
    attachments: &[String],
    rules: &'a [FilerRule],
) -> Option<&'a str> {
    rules
        .iter()
        .find(|r| {
            contains_ci(subject, &r.pattern)
                || attachments.iter().any(|a| contains_ci(a, &r.pattern))
        })
        .map(|r| r.folder.as_str())
}

/// Splits a list setting on commas, semicolons and line breaks, dropping blanks.
pub fn list_of(text: &str) -> Vec<String> {
    text.split([',', '\n', ';'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect()
}

/// Does an address match a list entry?
///
/// An entry starting with `@` is a domain and covers the addresses ending with it; any
/// other is an address, compared whole. Case does not count on either side.
pub fn address_matches(address: &str, entry: &str) -> bool {
    let address = address.trim().to_lowercase();
    let entry = entry.trim().to_lowercase();
    if entry.is_empty() {
        return false;
    }
    if entry.starts_with('@') {
        // With its `@`: "x@fake-direction.example" ends with "direction.example" without
        // being on that domain, which is how a domain gets spoofed.
        return entry.len() > 1 && address.ends_with(&entry);
    }
    address == entry
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    !needle.is_empty() && haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONDAY: i64 = 0;
    const FRIDAY: i64 = 4;
    const SATURDAY: i64 = 5;
    const SUNDAY: i64 = 6;

    fn vip() -> Triage {
        Triage {
            vip: "marie@client.example\n@direction.example".into(),
            ..Triage::default()
        }
    }

    fn on() -> OfficeHours {
        OfficeHours {
            enabled: true,
            ..OfficeHours::default()
        }
    }

    #[test]
    fn a_named_address_matters() {
        assert!(vip().is_vip("marie@client.example"));
        assert!(vip().is_vip("Marie@Client.EXAMPLE"), "case does not count");
    }

    #[test]
    fn a_named_domain_covers_its_addresses() {
        assert!(vip().is_vip("anyone@direction.example"));
        assert!(!vip().is_vip("newsletter@shop.example"));
    }

    #[test]
    fn a_domain_ending_the_same_is_not_the_same() {
        assert!(!vip().is_vip("x@fake-direction.example"));
    }

    #[test]
    fn an_empty_list_names_nobody() {
        assert!(!Triage::default().is_vip("marie@client.example"));
        assert!(!address_matches("a@b.example", "@"));
    }

    #[test]
    fn the_list_takes_commas_and_semicolons_too() {
        let t = Triage {
            vip: "a@x.example, b@x.example;\n\n c@x.example ,".into(),
            ..Triage::default()
        };
        assert_eq!(t.vip_list().len(), 3);
    }

    #[test]
    fn switched_off_nothing_waits() {
        let off = OfficeHours::default();
        assert!(!off.enabled);
        for day in 0..7 {
            for hour in 0..24 {
                assert_eq!(off.hours_until_open(hour, day), None);
            }
        }
    }

    #[test]
    fn during_the_hours_nothing_waits() {
        assert_eq!(on().hours_until_open(10, MONDAY), None);
        assert_eq!(on().hours_until_open(8, MONDAY), None);
        assert_eq!(on().hours_until_open(17, FRIDAY), None);
    }

    #[test]
    fn early_morning_waits_for_the_day_to_open() {
        assert_eq!(on().hours_until_open(6, MONDAY), Some(2));
        assert_eq!(
            on().hours_until_open(0, MONDAY),
            Some(8),
            "midnight is outside"
        );
    }

    #[test]
    fn the_evening_waits_for_the_next_morning() {
        assert_eq!(on().hours_until_open(23, MONDAY), Some(9));
    }

    #[test]
    fn friday_evening_skips_the_weekend() {
        assert_eq!(on().hours_until_open(19, FRIDAY), Some(13 + 48));
        assert_eq!(on().hours_until_open(10, SATURDAY), Some(22 + 24));
        assert_eq!(on().hours_until_open(10, SUNDAY), Some(22));
    }

    #[test]
    fn an_open_weekend_skips_nothing() {
        let all_week = OfficeHours {
            weekend: false,
            ..on()
        };
        assert_eq!(all_week.hours_until_open(10, SATURDAY), None);
        assert_eq!(all_week.hours_until_open(23, SATURDAY), Some(9));
    }

    #[test]
    fn an_end_not_after_the_start_works_until_midnight() {
        let backwards = OfficeHours {
            start: 18,
            end: 8,
            weekend: false,
            ..on()
        };
        for hour in 0..18 {
            assert_eq!(backwards.hours_until_open(hour, MONDAY), Some(18 - hour));
        }
        let whole_day = OfficeHours {
            start: 0,
            end: 0,
            weekend: false,
            ..on()
        };
        for hour in 0..24 {
            assert_eq!(whole_day.hours_until_open(hour, MONDAY), None);
        }
    }

    #[test]
    fn a_wait_never_passes_a_week() {
        for day in 0..7 {
            for hour in 0..24 {
                if let Some(h) = on().hours_until_open(hour, day) {
                    assert!(h > 0 && h <= 24 * 7, "{hour} h, day {day} -> {h} h");
                }
            }
        }
    }

    #[test]
    fn mail_comes_back_at_the_start_of_the_working_hour() {
        // Monday 5 January 1970, 23:40 UTC: day 4 since the epoch.
        let monday_late = Timestamp::from_millis(((4 * 24 + 23) * 3600 + 40 * 60) * 1000);
        let tuesday_eight = Timestamp::from_millis(((5 * 24 + 8) * 3600) * 1000);
        assert_eq!(on().reopens_at(monday_late), Some(tuesday_eight));

        let monday_ten = Timestamp::from_millis(((4 * 24 + 10) * 3600) * 1000);
        assert_eq!(on().reopens_at(monday_ten), None);
    }

    fn filer() -> Vec<FilerRule> {
        parse_filer("invoice -> Accounts\n# a comment\ncontract → Legal\n")
    }

    #[test]
    fn the_subject_files_the_thread() {
        assert_eq!(
            filer_destination("Your March invoice", &[], &filer()),
            Some("Accounts")
        );
        assert_eq!(
            filer_destination("INVOICE", &[], &filer()),
            Some("Accounts")
        );
        assert_eq!(
            filer_destination("a contract", &[], &filer()),
            Some("Legal")
        );
    }

    #[test]
    fn an_attachment_name_is_enough() {
        let attachments = vec!["invoice-2026-03.pdf".to_string()];
        assert_eq!(
            filer_destination("Hello", &attachments, &filer()),
            Some("Accounts")
        );
    }

    #[test]
    fn nothing_matching_nothing_moves() {
        assert_eq!(filer_destination("Lunch?", &[], &filer()), None);
        assert_eq!(filer_destination("Your invoice", &[], &[]), None);
    }

    #[test]
    fn the_first_line_wins() {
        let rules = parse_filer("invoice -> A\nmarch invoice -> B");
        assert_eq!(filer_destination("march invoice", &[], &rules), Some("A"));
    }

    #[test]
    fn a_faulty_line_is_skipped_on_its_own() {
        let rules = parse_filer("invoice -> Accounts\nno arrow here\n-> Empty\npattern ->\n");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].folder, "Accounts");

        let spaced = parse_filer("\n   invoice   ->   Accounts / 2026   \n\n");
        assert_eq!(spaced[0].pattern, "invoice");
        assert_eq!(spaced[0].folder, "Accounts / 2026");
    }

    #[test]
    fn by_default_the_three_do_nothing() {
        assert!(Triage::default().is_idle());
        assert!(!vip().is_idle());
    }

    #[test]
    fn hours_out_of_range_come_back_into_it() {
        let t = Triage {
            office_hours: OfficeHours {
                start: 40,
                end: 99,
                ..on()
            },
            ..Triage::default()
        }
        .sanitize();
        assert_eq!((t.office_hours.start, t.office_hours.end), (23, 23));
    }
}
