//! Revising flashcards: when each comes back (SM-2, as Anki started).
//!
//! A card is a question and its answer, from a note (`Question :: Answer`, or a
//! `> [!question]` callout). Each time it is revised it is graded — Again, Hard, Good,
//! Easy — and comes back after an interval that grows while it is known and starts
//! over when it is not.

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};

/// What is known of a card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardState {
    /// When it is due.
    pub due: NaiveDate,
    /// Days until the next time, as it last stood.
    pub interval: u32,
    /// How easy it is (2.5 to start, 1.3 at least).
    pub ease: f32,
    /// How many times in a row it was known.
    pub reps: u32,
}

impl CardState {
    pub fn new(today: NaiveDate) -> Self {
        Self {
            due: today,
            interval: 0,
            ease: 2.5,
            reps: 0,
        }
    }
}

/// How a card went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    Again,
    Hard,
    Good,
    Easy,
}

impl Grade {
    pub fn from_index(i: i32) -> Option<Grade> {
        Some(match i {
            0 => Grade::Again,
            1 => Grade::Hard,
            2 => Grade::Good,
            3 => Grade::Easy,
            _ => return None,
        })
    }
}

/// The card's state after being graded `grade` today.
pub fn grade(state: &CardState, grade: Grade, today: NaiveDate) -> CardState {
    let mut s = state.clone();
    match grade {
        Grade::Again => {
            s.reps = 0;
            s.interval = 1;
            s.ease = (s.ease - 0.2).max(1.3);
        }
        Grade::Hard => {
            s.interval = ((s.interval.max(1) as f32) * 1.2).ceil() as u32;
            s.ease = (s.ease - 0.15).max(1.3);
            s.reps += 1;
        }
        Grade::Good => {
            s.interval = match s.reps {
                0 => 1,
                1 => 6,
                _ => ((s.interval.max(1) as f32) * s.ease).round() as u32,
            };
            s.reps += 1;
        }
        Grade::Easy => {
            s.interval = match s.reps {
                0 => 4,
                _ => ((s.interval.max(1) as f32) * s.ease * 1.3).round() as u32,
            };
            s.ease += 0.15;
            s.reps += 1;
        }
    }
    s.interval = s.interval.clamp(1, 3650);
    s.due = today + Duration::days(s.interval as i64);
    s
}

/// A card's key: its note and its question, so a card keeps its history while the
/// rest of the note changes.
pub fn card_key(note: &str, question: &str) -> String {
    format!("{note}|{}", question.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jour(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    #[test]
    fn known_cards_come_back_later_and_later() {
        let s = CardState::new(jour(1));
        let s = grade(&s, Grade::Good, jour(1));
        assert_eq!((s.interval, s.due), (1, jour(2)));
        let s = grade(&s, Grade::Good, jour(2));
        assert_eq!((s.interval, s.due), (6, jour(8)));
        let s = grade(&s, Grade::Good, jour(8));
        assert_eq!(s.interval, 15);
    }

    #[test]
    fn a_forgotten_card_starts_over() {
        let mut s = CardState::new(jour(1));
        for _ in 0..3 {
            s = grade(&s, Grade::Good, jour(1));
        }
        let s = grade(&s, Grade::Again, jour(5));
        assert_eq!((s.reps, s.interval, s.due), (0, 1, jour(6)));
        assert!(s.ease < 2.5);
        assert_eq!(Grade::from_index(3), Some(Grade::Easy));
    }
}
