//! `iris-app` — l'assemblage.
//!
//! Le binaire ne fait qu'appeler ce qui se trouve ici, pour que tout l'assemblage
//! reste testable : un exécutable ne se teste pas, une bibliothèque si.

#![deny(unsafe_code)]

pub mod accounts;
pub mod controller;
pub mod modules;
pub mod oauth;
pub mod paths;
pub mod plugins;
pub mod services;
pub mod settings;
pub mod shell;
pub mod vitals;

pub use controller::{Controller, Request, Snapshot};
pub use paths::Paths;
pub use services::{now, Services};
