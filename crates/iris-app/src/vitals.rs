//! What the application is costing, and when it last spoke to the servers.
//!
//! Two numbers in the corner of a status bar. They earn their place for the same
//! reason the pending-operations count does: an application that synchronises in the
//! background is doing things you did not ask for at moments you did not choose, and
//! the only honest answer to "what is it doing right now" is to show it.
//!
//! Memory is read from the operating system rather than tracked internally. A number
//! the application computes about itself is the number it believes; the resident set
//! is the number the machine will act on when memory runs short.

use std::time::{Duration, Instant};

/// A reading of what the process is using.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vitals {
    /// Resident memory, in mebibytes.
    pub memory_mb: f32,
    /// Share of one core, averaged since the previous reading, as a percentage.
    pub cpu_percent: f32,
}

impl Vitals {
    /// Formatted for the status bar: short enough to ignore, precise enough to act on.
    pub fn summary(&self) -> String {
        format!("{:.0} MB · {:.0}% CPU", self.memory_mb, self.cpu_percent)
    }
}

/// Samples the process, remembering enough to turn CPU time into a rate.
///
/// CPU usage is a derivative: the operating system reports how much processor time
/// the process has consumed since it started, which on its own says nothing about
/// now. Two readings and the wall-clock gap between them give the rate.
#[derive(Debug)]
pub struct VitalsReader {
    previous_cpu: Duration,
    previous_at: Instant,
    cores: f32,
}

impl Default for VitalsReader {
    fn default() -> Self {
        Self::new()
    }
}

impl VitalsReader {
    pub fn new() -> Self {
        Self {
            previous_cpu: process_cpu_time().unwrap_or_default(),
            previous_at: Instant::now(),
            cores: std::thread::available_parallelism()
                .map(|n| n.get() as f32)
                .unwrap_or(1.0),
        }
    }

    /// Takes a reading.
    ///
    /// The first call after construction reports zero CPU, because there is nothing
    /// to compare against yet. Reporting a made-up figure would be worse.
    pub fn sample(&mut self) -> Vitals {
        let now = Instant::now();
        let cpu = process_cpu_time().unwrap_or(self.previous_cpu);

        let elapsed = now.duration_since(self.previous_at);
        let used = cpu.saturating_sub(self.previous_cpu);

        // Below a tenth of a second the ratio is mostly timer noise, so the previous
        // answer stands rather than a number that jumps around for no reason.
        let percent = if elapsed.as_secs_f32() > 0.1 {
            (used.as_secs_f32() / elapsed.as_secs_f32()) * 100.0
        } else {
            0.0
        };

        self.previous_cpu = cpu;
        self.previous_at = now;

        Vitals {
            memory_mb: resident_bytes().unwrap_or(0) as f32 / (1024.0 * 1024.0),
            // Across all cores: a background sync saturating one core of eight is at
            // 100% of a core, and calling that 12% would understate what the user can
            // hear from the fan.
            cpu_percent: (percent).clamp(0.0, 100.0 * self.cores),
        }
    }
}

// The only place in the application that calls into the operating system directly.
// The crate forbids unsafe everywhere else; reading your own process's memory and CPU
// time has no safe interface in the standard library, and inventing the numbers
// instead would defeat the point of showing them.
#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use std::time::Duration;

    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct MemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }

    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(process: isize, counters: *mut MemoryCounters, cb: u32) -> i32;
    }

    fn to_duration(t: &FileTime) -> Duration {
        // FILETIME counts hundred-nanosecond intervals.
        let ticks = ((t.high as u64) << 32) | t.low as u64;
        Duration::from_nanos(ticks * 100)
    }

    pub fn cpu_time() -> Option<Duration> {
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();

        // SAFETY: four out-parameters, all owned by this stack frame and all of the
        // size the API expects. The handle is the pseudo-handle for the current
        // process, which needs no closing.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
        };
        if ok == 0 {
            return None;
        }
        Some(to_duration(&kernel) + to_duration(&user))
    }

    pub fn resident_bytes() -> Option<u64> {
        let mut counters = MemoryCounters {
            cb: std::mem::size_of::<MemoryCounters>() as u32,
            ..Default::default()
        };

        // SAFETY: one out-parameter owned by this frame, with its size passed
        // alongside it exactly as the API requires.
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut counters,
                std::mem::size_of::<MemoryCounters>() as u32,
            )
        };
        if ok == 0 {
            return None;
        }
        Some(counters.working_set_size as u64)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::time::Duration;

    pub fn cpu_time() -> Option<Duration> {
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        // The comm field can contain spaces and parentheses, so fields are counted
        // from after the closing parenthesis rather than from the start.
        let rest = &stat[stat.rfind(')')? + 2..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let utime: u64 = fields.get(11)?.parse().ok()?;
        let stime: u64 = fields.get(12)?.parse().ok()?;
        let hz = 100.0;
        Some(Duration::from_secs_f64((utime + stime) as f64 / hz))
    }

    pub fn resident_bytes() -> Option<u64> {
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        Some(pages * 4096)
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use std::time::Duration;

    pub fn cpu_time() -> Option<Duration> {
        None
    }
    pub fn resident_bytes() -> Option<u64> {
        None
    }
}

fn process_cpu_time() -> Option<Duration> {
    platform::cpu_time()
}

/// L'ensemble résident du processus, en octets.
///
/// Public parce que la décomposition mémoire en a besoin elle aussi, et que deux
/// lectures du même compteur finiraient par diverger.
pub fn resident_bytes() -> Option<u64> {
    platform::resident_bytes()
}

/// How long ago something happened, in words.
///
/// "3 min ago" rather than a timestamp: the question a last-sync line answers is
/// "is this current", and nobody answers that by subtracting clock times in their head.
pub fn ago(then: iris_types::Timestamp, now: iris_types::Timestamp) -> String {
    let seconds = (now.millis() - then.millis()) / 1000;

    match seconds {
        // A clock that went backwards — a resync, a timezone change — should not
        // produce "in -3 minutes".
        i64::MIN..=-1 => "just now".into(),
        0..=44 => "just now".into(),
        // The thresholds follow the convention every other application uses: minutes
        // up to three quarters of an hour, then hours, then days. "60 min ago" is
        // arithmetic the reader should not have to do.
        45..=2_699 => format!("{} min ago", (seconds + 30) / 60),
        2_700..=79_199 => format!("{} h ago", (seconds + 1800) / 3600),
        _ => format!("{} d ago", (seconds + 43_200) / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_types::Timestamp;

    #[test]
    // Only where a reading is implemented; elsewhere it reports nothing on purpose,
    // and the status bar leaves the figure out.
    #[cfg(any(windows, target_os = "linux"))]
    fn a_reading_reports_some_memory() {
        // The process is running, so it is using memory. Zero would mean the platform
        // call failed silently.
        let mut reader = VitalsReader::new();
        let vitals = reader.sample();
        assert!(vitals.memory_mb > 0.0, "got {} MB", vitals.memory_mb);
    }

    #[test]
    fn the_first_reading_claims_no_cpu_rather_than_inventing_one() {
        let mut reader = VitalsReader::new();
        let vitals = reader.sample();
        assert!(vitals.cpu_percent >= 0.0);
    }

    #[test]
    fn cpu_is_a_rate_not_a_total() {
        // Two readings a moment apart must not report the whole lifetime of the
        // process as if it had happened in that moment.
        let mut reader = VitalsReader::new();
        reader.sample();
        std::thread::sleep(std::time::Duration::from_millis(120));
        let vitals = reader.sample();

        assert!(
            vitals.cpu_percent < 100.0 * 64.0,
            "implausible: {}",
            vitals.cpu_percent
        );
    }

    #[test]
    fn the_summary_is_short_enough_for_a_status_bar() {
        let v = Vitals {
            memory_mb: 166.4,
            cpu_percent: 3.2,
        };
        assert_eq!(v.summary(), "166 MB · 3% CPU");
    }

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_millis(secs * 1000)
    }

    #[test]
    fn recent_activity_reads_as_just_now() {
        assert_eq!(ago(t(1000), t(1000)), "just now");
        assert_eq!(ago(t(1000), t(1030)), "just now");
    }

    #[test]
    fn minutes_hours_and_days_are_rounded_to_the_nearest() {
        assert_eq!(ago(t(0), t(60)), "1 min ago");
        assert_eq!(ago(t(0), t(90)), "2 min ago");
        assert_eq!(ago(t(0), t(2_400)), "40 min ago");
        assert_eq!(ago(t(0), t(3600)), "1 h ago");
        assert_eq!(ago(t(0), t(86_400)), "1 d ago");
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_produce_a_negative_age() {
        // Resyncs and timezone changes do this, and "in -3 minutes" helps nobody.
        assert_eq!(ago(t(2000), t(1000)), "just now");
    }
}
