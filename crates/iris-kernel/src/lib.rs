//! `iris-kernel` — le noyau.
//!
//! Le noyau ignore ce qu'est un mail. Il fournit deux choses, et rien d'autre :
//!
//! - un [bus d'événements](bus::EventBus) typé et multi-abonnés ;
//! - la [coalescence](coalesce) de ces événements en diffs d'affichage bornés.
//!
//! Tout le reste d'Iris — synchronisation, workflow, recherche, thème, interface —
//! parle par ce bus.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod bus;
pub mod coalesce;
pub mod event;

pub use bus::{EventBus, Subscription};
pub use coalesce::{ViewDiff, WINDOW};
pub use event::{Event, EventKind, SyncPhase};
