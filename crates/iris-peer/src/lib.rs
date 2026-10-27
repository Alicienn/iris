//! Two copies of Iris on one network.
//!
//! A computer shows a QR code holding an [`Invite`]; a phone scans it, connects, and
//! both prove they hold the invite's key ([`Session`]). They then exchange what each
//! has changed since they last met ([`protocol`]): records, secrets, notes. The merge
//! itself is `iris-store`'s; this crate carries it, sealed, and knows nothing of SQL or
//! of the interface. Spec: `docs/superpowers/specs/2026-10-26-iris-sync-design.md`.

#![forbid(unsafe_code)]

mod discovery;
mod invite;
pub mod notes;
pub mod protocol;
mod session;

pub use discovery::{Discovery, Seen};
pub use invite::{random_key, Invite};
pub use protocol::{exchange, pack, unpack, ManifestEntry, Msg, NoteCopy, Side, Summary, Wants};
pub use session::{check_digits, Session};

/// What can go wrong between two devices.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    #[error("this is not an Iris pairing code: {0}")]
    Invite(String),
    #[error("the network: {0}")]
    Io(#[from] std::io::Error),
    #[error("the other device does not hold the same key")]
    Refused,
    #[error("a message could not be read: {0}")]
    Message(String),
    #[error("the exchange: {0}")]
    Exchange(String),
}
