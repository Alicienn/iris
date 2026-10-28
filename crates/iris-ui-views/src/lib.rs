//! The window, its screens and their components, as Slint generates them from
//! `crates/iris-ui/ui`. Nothing is written here by hand: `iris-ui` re-exports all of it
//! beside the Rust that feeds it.
//!
//! A crate of its own because the generated code is the heaviest thing the workspace
//! compiles. Inside `iris-ui` it was compiled twice in a test build (the library, and
//! again for its unit tests), and the two did not fit side by side in a CI runner's
//! memory (7.0.0); here it is compiled once.

// Types et composants engendrés à partir des fichiers `.slint`.
slint::include_modules!();
