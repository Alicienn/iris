//! `iris-ui` — l'interface.
//!
//! La fenêtre est décrite en Slint ; ce module ne fait que la nourrir. Toute la
//! logique d'affichage — quelle ligne, dans quel ordre, ce qu'une touche déclenche —
//! vit dans `iris-viewmodel` et se teste sans fenêtre.
//!
//! Ce qui reste ici est de trois natures, et chacune est testable seule :
//!
//! - la **mise en forme** ([`format`]) : dates relatives, teintes de compte, tailles
//!   lisibles. Calculée une fois, jamais pendant le rendu ;
//! - les **commandes** ([`commands`]) et les **raccourcis** ([`keymap`]) : des
//!   données, pas du code d'interface, donc reconfigurables ;
//! - le **pont** ([`bridge`]) : de la conversion de type, jamais de décision.

// No unsafe code of our own. (The code Slint generates, which annotates its own unsafe
// blocks, now lives in iris-ui-views; `deny` rather than `forbid` is kept from then.)
#![deny(unsafe_code)]

pub mod bridge;
pub mod commands;
pub mod format;
pub mod keymap;
pub mod recipients;

pub use commands::{builtin_commands, Command, CommandKind};
pub use keymap::{KeyOutcome, Keymap};

// The window, its screens and components, generated from `ui/` in a crate of their own
// (iris-ui-views) and re-exported here, so that everything keeps naming them
// `iris_ui::…` and `crate::…`.
pub use iris_ui_views::*;
