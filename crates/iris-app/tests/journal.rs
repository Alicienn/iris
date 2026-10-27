//! The journal starts even when another logger is already there.
//!
//! On Android, `iris-android` installs Android's logger before Iris's journal; the
//! journal's `init` then panicked ("Unable to install global subscriber:
//! SetLoggerError"), and Iris closed at launch on every phone (6.4.0). A test of its
//! own: it installs process-wide loggers.

struct Muet;

impl log::Log for Muet {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, _: &log::Record) {}
    fn flush(&self) {}
}

#[test]
fn the_journal_starts_after_another_logger() {
    log::set_logger(&Muet).expect("the first logger");
    let dossier = tempfile::tempdir().unwrap();
    let chemins = iris_app::paths::Paths::under(dossier.path());
    // Panicked before the fix.
    iris_app::logging::install(&chemins);
    tracing::warn!("the journal writes");
}
