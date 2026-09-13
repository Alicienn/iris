//! `iris-kernel` — le noyau modulaire.
//!
//! Le noyau ignore ce qu'est un mail. Il fournit quatre choses, et rien d'autre :
//!
//! - un [bus d'événements](bus::EventBus) typé et multi-abonnés ;
//! - la [coalescence](coalesce) de ces événements en diffs d'affichage bornés ;
//! - un [registre de modules](module::ModuleRegistry) avec cycle de vie et isolation
//!   des pannes ;
//! - un modèle de [capacités](capability::Capability) commun aux modules internes et
//!   aux plugins.
//!
//! Tout le reste d'Iris — synchronisation, workflow, recherche, thème, interface —
//! est un module posé sur cette base.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod bus;
pub mod capability;
pub mod coalesce;
pub mod config;
pub mod event;
pub mod module;

pub use bus::{EventBus, Subscription};
pub use capability::{Capability, CapabilitySet, NetworkScope};
pub use coalesce::{ViewDiff, WINDOW};
pub use config::{Config, ConfigSnapshot};
pub use event::{Event, EventKind, SyncPhase};
pub use module::{
    LifecycleReport, Module, ModuleContext, ModuleManifest, ModuleRegistry, ModuleState,
};
