//! Growth, without input or output.
//!
//! What the Growth place needs to know and that needs neither the base nor the
//! window:
//!
//! - [`habits`] says when a habit is due, what each day of it shows, its streaks and
//!   its rate;
//! - [`quick`] reads a habit typed in one line: "Read 20 pages every day".
//!
//! Nothing here counts on the user's behalf: the days done are the ones ticked.

#![forbid(unsafe_code)]

pub mod habits;
pub mod quick;

pub use habits::{Day, Habit, Schedule, Streak};
pub use quick::{parse, QuickHabit};
