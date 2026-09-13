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

// Le code engendré par Slint contient des blocs non sûrs qu'il annote lui-même ;
// « forbid » interdirait cette annotation. Notre propre code, lui, n'en contient
// aucun — c'est ce que vérifie « deny », qui reste levable ligne par ligne mais
// n'a jamais eu à l'être.
#![deny(unsafe_code)]

pub mod bridge;
pub mod commands;
pub mod format;
pub mod keymap;

pub use commands::{builtin_commands, Command, CommandKind};
pub use keymap::{KeyOutcome, Keymap};

// Types et composants engendrés à partir des fichiers `.slint`.
slint::include_modules!();
