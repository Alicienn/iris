//! `iris-app` — l'assemblage.
//!
//! Le binaire ne fait qu'appeler ce qui se trouve ici, pour que tout l'assemblage
//! reste testable : un exécutable ne se teste pas, une bibliothèque si.

#![deny(unsafe_code)]

pub mod accounts;
pub mod backup;
pub mod caldav;
pub mod calendar;
pub mod changelog;
pub mod controller;
pub mod draft;
pub mod folders;
pub mod goals;
pub mod home;
pub mod invite;
pub mod later;
pub mod logging;
pub mod memory;
pub mod modules;
pub mod nav;
pub mod notes;
pub mod notify;
pub mod oauth;
pub mod paths;
pub mod platform;
pub mod plugins;
pub mod sending;
pub mod services;
pub mod settings;
pub mod shell;
pub mod single;
pub mod tags;
pub mod tasks;
pub mod tray;
pub mod unsubscribe;
pub mod update;
pub mod visio;
pub mod vitals;
pub mod workspace;

pub use controller::{Controller, Request, Snapshot};
pub use paths::Paths;
pub use services::{now, Services};
