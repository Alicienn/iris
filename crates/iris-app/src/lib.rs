//! `iris-app` — l'assemblage.
//!
//! Le binaire ne fait qu'appeler ce qui se trouve ici, pour que tout l'assemblage
//! reste testable : un exécutable ne se teste pas, une bibliothèque si.

#![deny(unsafe_code)]

pub mod accounts;
pub mod catalogue;
pub mod controller;
pub mod modules;
pub mod folders;
pub mod logging;
pub mod oauth;
pub mod notify;
pub mod paths;
pub mod platform;
pub mod plugins;
pub mod services;
pub mod settings;
pub mod shell;
pub mod tray;
pub mod vitals;

pub use controller::{Controller, Request, Snapshot};
pub use paths::Paths;
pub use services::{now, Services};
