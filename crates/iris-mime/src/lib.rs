//! `iris-mime` — analyse et assainissement des messages.
//!
//! Deux responsabilités, volontairement séparées :
//!
//! - [`parse`] transforme un message brut en structure exploitable, en calculant une
//!   fois pour toutes ce dont la liste aura besoin (aperçu, drapeaux dérivés) ;
//! - [`sanitize`] rend un corps HTML sûr à afficher, en recensant au passage ce qui
//!   aurait appelé le réseau et ce qui cherchait à pister le lecteur.
//!
//! Le principe qui gouverne cette crate : **le contenu d'un message est hostile par
//! défaut**, et rien de ce qu'il demande n'est accordé sans examen.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod cid;
pub mod parse;
pub mod sanitize;
pub mod unsubscribe;

pub mod spam;

pub use cid::inline_images;
pub use parse::{attachment_bytes, parse, parse_with, strip_tags, InlinePart, Parsed};
pub use sanitize::{sanitize, sanitize_with, Sanitized, Tracker, TrackerReason};
pub use spam::{headers_say_spam, strip_marker, subject_is_tagged};
