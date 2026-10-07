//! `iris-notes` — notes in Markdown, without I/O.
//!
//! - [`block`] cuts a note into blocks and joins them back byte for byte: opening and
//!   saving a note never changes what was not edited;
//! - [`inline`] reads the marks of a line (Markdown, Obsidian's, and Iris's own short
//!   ones) into spans, and writes them for Slint's `StyledText` or as HTML;
//! - [`edit`] turns keys into edits: a mark around a selection, a line start converted,
//!   a list continued.

#![forbid(unsafe_code)]

pub mod block;
pub mod complete;
pub mod edit;
pub mod fuzzy;
pub mod graph;
pub mod html;
pub mod inline;
pub mod links;
pub mod math;
pub mod meta;
pub mod paste;
pub mod review;
pub mod template;

pub use block::{join, parse, Block, BlockKind};
