//! `iris-calendar` — l'agenda, sans entrée-sortie.
//!
//! Quatre choses, chacune testable seule :
//!
//! - [`ics`] lit un fichier iCalendar (RFC 5545) — celui d'un abonnement, ou celui
//!   joint à une invitation — et en tire des [`Event`] ;
//! - [`recur`] déroule les récurrences sur une période : « tous les lundis » devient
//!   des lundis précis, exceptions et occurrences modifiées comprises ;
//! - [`layout`] dispose ces occurrences pour l'écran : une grille de mois, des
//!   colonnes de semaine où deux réunions simultanées se partagent la largeur ;
//! - [`link`] reconnaît un lien d'abonnement (`webcal://`, `https://…/basic.ics`).
//!
//! Le temps est en millisecondes UTC partout, comme dans le reste d'Iris. Le fuseau
//! n'intervient qu'aux deux bords : à la lecture, pour convertir ce que le fichier
//! exprime dans le sien, et à l'affichage, pour montrer l'heure de celui qui regarde.

#![forbid(unsafe_code)]

pub mod ics;
pub mod layout;
pub mod link;
pub mod recur;
pub mod time;

/// Un événement, tel que le fichier le décrit.
///
/// Une récurrence est **un** événement portant sa règle ; ses occurrences n'existent
/// qu'à l'affichage, déroulées pour la période montrée. Les stocker une à une ferait
/// d'une réunion hebdomadaire sans fin un nombre infini de lignes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Event {
    /// L'identifiant que le fichier donne, stable d'une mise à jour à l'autre.
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub location: String,
    /// Début et fin, en millisecondes UTC. Pour un événement « toute la journée », la
    /// date est posée à minuit UTC et la fin est exclusive : un jour = [J, J+1[.
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    /// Le fuseau du fichier (nom IANA), pour dérouler une récurrence à la bonne heure
    /// de part et d'autre d'un changement d'heure.
    pub tzid: Option<String>,
    /// La règle de récurrence, telle qu'écrite (`FREQ=WEEKLY;BYDAY=MO`).
    pub rrule: Option<String>,
    /// Les occurrences retirées de la règle.
    pub exdates: Vec<i64>,
    /// Pour une occurrence modifiée : celle qu'elle remplace.
    pub recurrence_id: Option<i64>,
    /// Annulé par l'organisateur : gardé pour remplacer, jamais montré.
    pub cancelled: bool,
    /// Le rappel que le fichier propose, en minutes avant le début.
    pub reminder_minutes: Option<i32>,
}

/// Une occurrence à montrer : un événement, à une date précise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    /// L'indice de l'événement dans la liste fournie au déroulement.
    pub event: usize,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
}

const JOUR_MS: i64 = 86_400_000;
