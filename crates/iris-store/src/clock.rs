//! The clock that orders changes between devices.
//!
//! A hybrid logical clock: the wall clock in milliseconds, a counter for changes within
//! one millisecond (or while the wall clock lags behind one already seen), and the
//! device that made the change. Written as `0001790000000000.0000000000.<device>` — fixed
//! widths, so that comparing two clocks as strings compares them as
//! `(milliseconds, counter, device)`, which SQLite does without help.
//!
//! A clock never goes backwards: each tick is greater than every tick before it and
//! every clock observed from another device, so a phone ten minutes behind that has
//! seen the computer's changes still writes after them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Debug)]
pub struct Clock {
    device: Mutex<String>,
    state: Mutex<(u64, u32)>,
    merging: AtomicBool,
}

impl Clock {
    pub fn new(device: String) -> Self {
        Self {
            device: Mutex::new(device),
            state: Mutex::new((0, 0)),
            merging: AtomicBool::new(false),
        }
    }

    /// A clock greater than every one returned or observed before.
    pub fn tick(&self, now_ms: u64) -> String {
        let mut etat = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *etat = if now_ms > etat.0 {
            (now_ms, 0)
        } else {
            (etat.0, etat.1.saturating_add(1))
        };
        format(etat.0, etat.1, &self.device())
    }

    /// A tick at the wall clock's time.
    pub fn now(&self) -> String {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.tick(ms)
    }

    /// Takes in a clock from another device: the next tick comes after it.
    pub fn observe(&self, remote: &str) {
        let Some((ms, compteur, _)) = parse(remote) else {
            return;
        };
        let mut etat = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if (ms, compteur) > *etat {
            *etat = (ms, compteur);
        }
    }

    pub fn device(&self) -> String {
        self.device
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The device id is read from the base after the migrations, once the clock is
    /// already registered with the connection.
    pub(crate) fn set_device(&self, device: String) {
        *self.device.lock().unwrap_or_else(|e| e.into_inner()) = device;
    }

    /// While a merge writes what another device sent, the triggers neither stamp the
    /// rows with a new clock nor write tombstones: the rows keep the other's.
    pub fn merging(&self) -> bool {
        self.merging.load(Ordering::SeqCst)
    }

    pub(crate) fn set_merging(&self, on: bool) {
        self.merging.store(on, Ordering::SeqCst);
    }
}

fn format(ms: u64, compteur: u32, device: &str) -> String {
    format!("{ms:016}.{compteur:010}.{device}")
}

/// A clock's three parts, or `None` for what is not one (the empty clock of a row
/// never stamped, among others).
pub fn parse(clock: &str) -> Option<(u64, u32, &str)> {
    let (ms, reste) = clock.split_once('.')?;
    let (compteur, device) = reste.split_once('.')?;
    if ms.len() != 16 || compteur.len() != 10 {
        return None;
    }
    Some((ms.parse().ok()?, compteur.parse().ok()?, device))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_increase_while_the_wall_clock_stands_still() {
        let c = Clock::new("aa".into());
        let a = c.tick(1_000);
        let b = c.tick(1_000);
        let d = c.tick(999);
        assert!(a < b && b < d, "{a} {b} {d}");
        assert!(c.tick(2_000) > d);
    }

    #[test]
    fn a_clock_seen_from_the_future_is_passed() {
        let c = Clock::new("aa".into());
        c.tick(1_000);
        let loin = format(9_000_000, 7, "bb");
        c.observe(&loin);
        assert!(c.tick(1_001) > loin);
    }

    #[test]
    fn a_device_ten_minutes_behind_writes_after_what_it_saw() {
        let pc = Clock::new("pc".into());
        let tel = Clock::new("phone".into());
        let ecrit_au_pc = pc.tick(1_790_000_600_000);
        tel.observe(&ecrit_au_pc);
        let ecrit_au_tel = tel.tick(1_790_000_000_000);
        assert!(ecrit_au_tel > ecrit_au_pc);
    }

    #[test]
    fn string_order_is_the_order_of_the_parts() {
        let mut v = [
            format(20, 0, "a"),
            format(3, 9, "z"),
            format(3, 10, "a"),
            format(3, 10, "b"),
            format(100, 0, "a"),
        ];
        v.sort();
        let parts: Vec<_> = v.iter().map(|s| parse(s).unwrap()).collect();
        let mut tries = parts.clone();
        tries.sort();
        assert_eq!(parts, tries);
        assert!(parse("").is_none());
        assert!(parse("12.3.a").is_none());
    }
}
