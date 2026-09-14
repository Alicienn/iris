//! Recognising mail the server has already judged.
//!
//! Iris does not classify spam itself. Mail servers have been doing it for decades
//! with corpora we do not have, and a second opinion computed from the subject line
//! would be worse than the first while looking equally confident.
//!
//! What we do instead is **read the verdict the server already wrote down**. Nearly
//! every filter marks its findings in one of two ways: a header, or a prefix stapled
//! to the subject. Both are trivial to read, and neither guesses.
//!
//! The bar is deliberately high. A message wrongly hidden as spam is worse than a
//! spam message left in the queue: one costs a moment's annoyance, the other loses
//! correspondence. So only explicit markers count — never a hunch about wording.

/// Subject prefixes filters staple on. Compared case-insensitively.
///
/// The list covers SpamAssassin and the shapes hosting providers wrap around it.
/// Anything unrecognised is not spam, which is the safe direction to be wrong in.
const SUBJECT_MARKERS: &[&str] = &[
    "***spam***",
    "***potentiel-spam***",
    "***potential-spam***",
    "[spam]",
    "[spam?]",
    "[likely-spam]",
    "[probable spam]",
    "{spam}",
    "spam:",
];

/// Headers whose presence, with an affirmative value, settles it.
const AFFIRMATIVE_HEADERS: &[&str] = &["x-spam-flag", "x-spam"];

/// Does the subject carry a filter's mark?
///
/// The marker is looked for at the start and anywhere after a reply prefix, because
/// "Re: ***SPAM*** …" is what a thread looks like once somebody has answered one.
pub fn subject_is_tagged(subject: &str) -> bool {
    let lower = subject.trim().to_lowercase();
    SUBJECT_MARKERS
        .iter()
        .any(|marker| lower.starts_with(marker) || lower.contains(marker))
}

/// Does a header block carry an affirmative spam verdict?
///
/// Reads `X-Spam-Flag: YES` and the `X-Spam-Status: Yes, score=…` form. A numeric
/// score on its own is deliberately ignored: the threshold belongs to whoever
/// configured the filter, and second-guessing it here would hide mail they chose to
/// let through.
pub fn headers_say_spam(headers: &str) -> bool {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_lowercase();
        let value = value.trim().to_lowercase();

        if AFFIRMATIVE_HEADERS.contains(&name.as_str()) && affirmative(&value) {
            return true;
        }
        if name == "x-spam-status" && affirmative(value.split(',').next().unwrap_or("")) {
            return true;
        }
    }
    false
}

fn affirmative(value: &str) -> bool {
    matches!(value.trim(), "yes" | "true" | "1")
}

/// Strips the marker so the subject reads normally once it is filed as spam.
///
/// The tab it sits in already says it is spam; repeating that on every row is noise
/// that pushes the actual subject off the end of the line.
pub fn strip_marker(subject: &str) -> String {
    let trimmed = subject.trim();
    let lower = trimmed.to_lowercase();

    for marker in SUBJECT_MARKERS {
        if let Some(rest) = lower.strip_prefix(marker) {
            let cut = trimmed.len() - rest.len();
            return trimmed[cut..].trim_start().to_string();
        }
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_common_markers_are_recognised() {
        for subject in [
            "***SPAM*** Cheap watches",
            "***Potentiel-SPAM*** Virement DGFiP retourné",
            "[SPAM] Hello",
            "{Spam} Hello",
            "SPAM: Hello",
        ] {
            assert!(subject_is_tagged(subject), "missed: {subject}");
        }
    }

    #[test]
    fn the_case_of_the_marker_does_not_matter() {
        assert!(subject_is_tagged("***potentiel-spam*** hello"));
        assert!(subject_is_tagged("***POTENTIEL-SPAM*** hello"));
    }

    #[test]
    fn a_marker_survives_a_reply_prefix() {
        // Once somebody answers one, the thread subject looks like this.
        assert!(subject_is_tagged("Re: ***SPAM*** Cheap watches"));
    }

    #[test]
    fn ordinary_mail_is_not_spam() {
        // The bar is high on purpose: hiding real correspondence is the worse error.
        for subject in [
            "Quote for the hydraulic cylinders",
            "Re: invoice 2024-03",
            "Spammers are annoying",
            "Notre politique anti-spam",
            "",
        ] {
            assert!(!subject_is_tagged(subject), "wrongly flagged: {subject}");
        }
    }

    #[test]
    fn an_affirmative_header_settles_it() {
        assert!(headers_say_spam("X-Spam-Flag: YES"));
        assert!(headers_say_spam("x-spam-flag: yes"));
        assert!(headers_say_spam(
            "X-Spam-Status: Yes, score=9.1 required=5.0"
        ));
    }

    #[test]
    fn a_negative_header_is_not_spam() {
        // This is the header the message in the screenshot actually carried.
        assert!(!headers_say_spam(
            "X-Spam-Status: No, score=1.9 required=5.0"
        ));
        assert!(!headers_say_spam("X-Spam-Flag: NO"));
    }

    #[test]
    fn a_score_without_a_verdict_is_not_a_verdict() {
        // The threshold belongs to whoever configured the filter.
        assert!(!headers_say_spam("X-Spam-Score: 19"));
        assert!(!headers_say_spam("X-Spam-Bar: +++++"));
    }

    #[test]
    fn headers_that_mention_spam_in_passing_are_ignored() {
        assert!(!headers_say_spam(
            "X-Ham-Report: Spam detection software running on scan.jabatus.fr"
        ));
        assert!(!headers_say_spam("Subject: our anti-spam policy"));
    }

    #[test]
    fn the_marker_is_stripped_for_display() {
        // The tab already says it is spam; repeating it pushes the real subject off
        // the end of the row.
        assert_eq!(
            strip_marker("***Potentiel-SPAM*** Virement DGFiP retourné"),
            "Virement DGFiP retourné"
        );
        assert_eq!(strip_marker("[SPAM] Hello"), "Hello");
    }

    #[test]
    fn a_subject_without_a_marker_is_left_alone() {
        assert_eq!(
            strip_marker("  Quote for the cylinders  "),
            "Quote for the cylinders"
        );
    }

    #[test]
    fn stripping_a_subject_that_is_only_a_marker_leaves_nothing() {
        assert_eq!(strip_marker("***SPAM***"), "");
    }
}
