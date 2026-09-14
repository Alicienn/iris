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

pub mod parse;
pub mod sanitize;
pub mod unsubscribe;

pub use parse::{attachment_bytes, parse, strip_tags, Parsed};
pub use sanitize::{sanitize, Sanitized, Tracker, TrackerReason};
