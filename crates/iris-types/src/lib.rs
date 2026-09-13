//! `iris-types` — le contrat partagé par toutes les couches d'Iris.
//!
//! Cette crate ne dépend de rien et ne fait aucune entrée-sortie. Elle définit les
//! identifiants, l'enveloppe d'un message, la machine à états du workflow et le type
//! d'erreur commun. Toute autre crate en dépend ; elle ne dépend d'aucune autre.
//!
//! La règle qui la gouverne : **si un type est partagé par deux couches, il vit ici**.
//! Sinon il reste privé à sa crate.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod address;
mod envelope;
mod error;
mod ids;
mod workflow;

pub use address::Address;
pub use envelope::{AttachmentMeta, Envelope, Flags, Timestamp, Unsubscribe};
pub use error::{Error, Result};
pub use ids::{AccountId, BlobId, FolderId, MessageId, OpId, RfcMessageId, ThreadId, Uid};
pub use workflow::{
    transition, AutomationSettings, Snooze, TransitionCause, TransitionOutcome, WorkflowState,
};
