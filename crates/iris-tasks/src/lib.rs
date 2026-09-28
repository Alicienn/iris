//! Les tâches, sans entrée-sortie.
//!
//! Ce qu'il faut savoir d'une tâche pour l'afficher et la créer, et qui n'a besoin ni
//! de la base ni de la fenêtre :
//!
//! - [`quick`] lit une saisie rapide — « demain 9h appeler Marie #Travail !! » — en
//!   titre, échéance, liste et priorité ;
//! - [`due`] dit quand une tâche est due, en mots, et dans quelle section elle se
//!   range.

pub mod due;
pub mod quick;

pub use due::{due_label, is_overdue, remind_at, section, Section, REMINDERS};
pub use quick::{parse, QuickAdd};

/// L'échéance d'une tâche : un jour, et une heure si elle en a une.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Due {
    pub day: chrono::NaiveDate,
    /// Minutes depuis minuit. `None` : n'importe quand dans la journée.
    pub minute: Option<u32>,
}

/// Les priorités, de la plus basse à la plus haute. 0 : aucune.
pub const PRIORITIES: [&str; 4] = ["None", "Low", "Medium", "High"];
