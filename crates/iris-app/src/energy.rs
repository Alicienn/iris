//! What Iris spends, measured rather than guessed: once a minute, the CPU each of its
//! threads used (named: Slint's window, tokio's workers, the discovery, the sync…) and
//! the memory it holds, as a line of the log and in a short history that Settings ›
//! Updates copies to share ("Copy the energy report"). A phone that warms up shows
//! there which thread keeps it awake.
//!
//! Read from `/proc/self/task/*/stat` (Linux, Android); elsewhere nothing is sampled.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often a line is written.
const PERIOD: Duration = Duration::from_secs(60);
/// The lines kept for the report: two hours.
const KEPT: usize = 120;

static HISTORY: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Starts the sampling on a thread of its own, once.
pub fn start() {
    if !cfg!(any(target_os = "linux", target_os = "android")) {
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("iris-energy".into())
            .spawn(|| {
                let mut avant = threads();
                let mut quand = Instant::now();
                loop {
                    std::thread::sleep(PERIOD);
                    let maintenant = threads();
                    let ecoule = quand.elapsed();
                    quand = Instant::now();
                    let ligne = line(&avant, &maintenant, ecoule, memory_mb());
                    tracing::info!(target: "energy", "{ligne}");
                    let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
                    if h.len() == KEPT {
                        h.pop_front();
                    }
                    h.push_back(format!("{} {ligne}", chrono::Local::now().format("%H:%M")));
                    avant = maintenant;
                }
            });
    });
}

/// The report Settings copies: the version, then the minutes kept, newest last.
pub fn report() -> String {
    let h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let mut texte = format!(
        "Iris {} energy report ({} {})\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if h.is_empty() {
        texte.push_str("No minute measured yet: keep Iris open a few minutes, then copy again.\n");
    }
    for l in h.iter() {
        texte.push_str(l);
        texte.push('\n');
    }
    texte
}

/// CPU time used by each thread so far, in clock ticks, by `name/tid`.
fn threads() -> HashMap<String, u64> {
    let mut v = HashMap::new();
    let Ok(dossier) = std::fs::read_dir("/proc/self/task") else {
        return v;
    };
    for e in dossier.flatten() {
        let tid = e.file_name().to_string_lossy().to_string();
        if let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) {
            if let Some((nom, ticks)) = parse_stat(&stat) {
                v.insert(format!("{nom}/{tid}"), ticks);
            }
        }
    }
    v
}

/// The name and the user + system ticks of a `stat` line: `tid (name) S … utime stime`.
/// The name sits in parentheses and may hold spaces; the fields count after it.
fn parse_stat(stat: &str) -> Option<(String, u64)> {
    let ouvre = stat.find('(')?;
    let ferme = stat.rfind(')')?;
    let nom = stat.get(ouvre + 1..ferme)?.to_string();
    let reste: Vec<&str> = stat.get(ferme + 2..)?.split_whitespace().collect();
    // After the name: state (0), …, utime (11), stime (12).
    let u: u64 = reste.get(11)?.parse().ok()?;
    let s: u64 = reste.get(12)?.parse().ok()?;
    Some((nom, u + s))
}

/// The resident memory, in megabytes (`/proc/self/statm`, in pages of 4 KiB).
fn memory_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)
                .and_then(|p| p.parse::<u64>().ok())
        })
        .map_or(0, |pages| pages * 4 / 1024)
}

/// One minute in a line: the whole CPU, then the busiest threads by name (threads of
/// one name summed), the memory and how many threads run.
fn line(
    avant: &HashMap<String, u64>,
    apres: &HashMap<String, u64>,
    ecoule: Duration,
    memoire_mb: u64,
) -> String {
    // Linux counts in hundredths of a second (USER_HZ).
    let secondes = ecoule.as_secs_f64().max(1.0);
    let mut par_nom: HashMap<&str, u64> = HashMap::new();
    let mut total = 0u64;
    for (cle, &t) in apres {
        let d = t.saturating_sub(avant.get(cle).copied().unwrap_or(0));
        total += d;
        let nom = cle.rsplit_once('/').map_or(cle.as_str(), |(n, _)| n);
        *par_nom.entry(nom).or_default() += d;
    }
    let pct = |ticks: u64| ticks as f64 / secondes;
    let mut tri: Vec<(&str, u64)> = par_nom.into_iter().filter(|(_, d)| *d > 0).collect();
    tri.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let premiers = tri
        .iter()
        .take(5)
        .map(|(n, d)| format!("{n} {:.1}%", pct(*d)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "cpu {:.1}% over {:.0} s; busiest: {}; memory {} MB; {} threads",
        pct(total),
        secondes,
        if premiers.is_empty() {
            "none"
        } else {
            &premiers
        },
        memoire_mb,
        apres.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_threads_stat_line_gives_its_name_and_ticks() {
        let l = "4321 (tokio-runtime-w) S 1 2 3 4 5 6 7 8 9 10 120 30 0 0 20 0 1 0";
        assert_eq!(parse_stat(l), Some(("tokio-runtime-w".to_string(), 150)));
        // A name with a space and a parenthesis in it.
        let l = "7 (a (b) c) R 1 2 3 4 5 6 7 8 9 10 5 5 0 0";
        assert_eq!(parse_stat(l), Some(("a (b) c".to_string(), 10)));
    }

    #[test]
    fn a_minute_reads_as_a_line_busiest_first() {
        let avant = HashMap::from([
            ("slint/1".to_string(), 100),
            ("tokio/2".to_string(), 50),
            ("tokio/3".to_string(), 50),
        ]);
        let apres = HashMap::from([
            ("slint/1".to_string(), 160),
            ("tokio/2".to_string(), 80),
            ("tokio/3".to_string(), 70),
        ]);
        assert_eq!(
            line(&avant, &apres, Duration::from_secs(60), 92),
            "cpu 1.8% over 60 s; busiest: slint 1.0%, tokio 0.8%; memory 92 MB; 3 threads"
        );
    }
}
