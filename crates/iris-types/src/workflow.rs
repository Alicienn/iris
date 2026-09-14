//! La machine à états du workflow.
//!
//! L'état porte sur le **fil**, jamais sur le message : on ne traite pas un message
//! isolé, on traite un échange. Le report (`snooze`) est orthogonal aux trois états,
//! ce qui évite l'erreur classique consistant à en faire un quatrième état — un fil
//! reporté doit revenir exactement là où il était.

use crate::Timestamp;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    /// Demande une action de l'utilisateur.
    Todo,
    /// L'utilisateur a répondu, la balle est dans le camp d'en face.
    Waiting,
    /// Terminé. État terminal, sauf nouveau message.
    Done,
}

impl WorkflowState {
    pub const ALL: [Self; 3] = [Self::Todo, Self::Waiting, Self::Done];

    /// Identifiant stable en base et dans les fichiers de configuration.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Waiting => "waiting",
            Self::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "waiting" => Some(Self::Waiting),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    /// Entier compact pour l'index SQL directeur `(state, last_activity, thread)`.
    pub const fn as_i64(self) -> i64 {
        match self {
            Self::Todo => 0,
            Self::Waiting => 1,
            Self::Done => 2,
        }
    }

    pub const fn from_i64(v: i64) -> Option<Self> {
        match v {
            0 => Some(Self::Todo),
            1 => Some(Self::Waiting),
            2 => Some(Self::Done),
            _ => None,
        }
    }

    /// Un fil dans cet état apparaît-il dans la file de travail active ?
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Todo)
    }
}

impl fmt::Display for WorkflowState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Pourquoi une transition a eu lieu. Déterminant : chaque cause automatique est
/// individuellement désactivable, et l'historique doit dire qui a décidé quoi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionCause {
    /// Action explicite de l'utilisateur.
    Manual,
    /// Une réponse a été envoyée depuis l'application.
    ReplySent,
    /// Un nouveau message est arrivé dans le fil.
    MessageReceived,
    /// L'échéance d'un report est atteinte.
    SnoozeExpired,
    /// Un délai s'est écoulé sans réponse sur un fil en attente.
    FollowUpDue,
    /// Une règle utilisateur a agi.
    Rule,
}

impl TransitionCause {
    /// Les causes automatiques sont celles que l'utilisateur peut désactiver.
    /// `Manual` ne l'est évidemment pas, et `Rule` obéit à l'activation de la règle
    /// elle-même, pas à un réglage global.
    pub const fn is_automatic(self) -> bool {
        matches!(
            self,
            Self::ReplySent | Self::MessageReceived | Self::SnoozeExpired | Self::FollowUpDue
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::ReplySent => "reply_sent",
            Self::MessageReceived => "message_received",
            Self::SnoozeExpired => "snooze_expired",
            Self::FollowUpDue => "follow_up_due",
            Self::Rule => "rule",
        }
    }
}

/// Les automatismes, chacun activable séparément.
///
/// Choix produit validé : tout est activé par défaut, et rien n'est irréversible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationSettings {
    /// Envoyer une réponse fait passer le fil en attente.
    pub reply_marks_waiting: bool,
    /// Un nouveau message ramène un fil terminé dans la file.
    pub new_message_reopens: bool,
    /// Un fil en attente trop longtemps sans réponse revient dans la file.
    pub follow_up_enabled: bool,
    /// Délai avant relance, en jours.
    pub follow_up_days: u16,
}

impl Default for AutomationSettings {
    fn default() -> Self {
        Self {
            reply_marks_waiting: true,
            new_message_reopens: true,
            follow_up_enabled: true,
            follow_up_days: 3,
        }
    }
}

impl AutomationSettings {
    /// Tous les automatismes coupés : l'état ne change alors que sur action manuelle.
    pub const MANUAL_ONLY: Self = Self {
        reply_marks_waiting: false,
        new_message_reopens: false,
        follow_up_enabled: false,
        follow_up_days: 3,
    };

    /// Cette cause a-t-elle le droit d'agir ?
    pub const fn allows(&self, cause: TransitionCause) -> bool {
        match cause {
            TransitionCause::Manual | TransitionCause::Rule | TransitionCause::SnoozeExpired => {
                true
            }
            TransitionCause::ReplySent => self.reply_marks_waiting,
            TransitionCause::MessageReceived => self.new_message_reopens,
            TransitionCause::FollowUpDue => self.follow_up_enabled,
        }
    }
}

/// Report d'un fil : il disparaît de la vue jusqu'à l'échéance, sans changer d'état.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snooze {
    pub until: Timestamp,
    /// L'état au moment du report, restauré à l'échéance.
    pub restore_to: WorkflowState,
}

impl Snooze {
    pub const fn is_due(&self, now: Timestamp) -> bool {
        now.0 >= self.until.0
    }
}

/// Résultat de l'évaluation d'une transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionOutcome {
    /// L'état change.
    Moved {
        from: WorkflowState,
        to: WorkflowState,
    },
    /// La transition est légale mais l'état est déjà le bon.
    Unchanged,
    /// L'automatisme correspondant est désactivé.
    Disabled,
    /// La transition n'a pas de sens depuis cet état.
    Illegal,
}

impl TransitionOutcome {
    pub const fn changed(self) -> bool {
        matches!(self, Self::Moved { .. })
    }
}

/// Calcule l'effet d'une cause sur un état, sans rien muter.
///
/// Fonction pure : c'est la totalité de la logique du workflow, et elle est donc
/// testable exhaustivement sans base de données ni réseau.
pub fn transition(
    current: WorkflowState,
    cause: TransitionCause,
    target: Option<WorkflowState>,
    settings: &AutomationSettings,
) -> TransitionOutcome {
    if !settings.allows(cause) {
        return TransitionOutcome::Disabled;
    }

    let to = match cause {
        // L'utilisateur et les règles désignent explicitement la cible.
        TransitionCause::Manual | TransitionCause::Rule => match target {
            Some(t) => t,
            None => return TransitionOutcome::Illegal,
        },
        // Répondre à un fil terminé ne le rouvre pas : c'est un post-scriptum, pas
        // une attente. Répondre depuis n'importe quel autre état met en attente.
        TransitionCause::ReplySent => match current {
            WorkflowState::Done => return TransitionOutcome::Unchanged,
            _ => WorkflowState::Waiting,
        },
        // Un nouveau message ramène dans la file, quel que soit l'état précédent.
        TransitionCause::MessageReceived => WorkflowState::Todo,
        // La relance ne concerne que les fils en attente.
        TransitionCause::FollowUpDue => match current {
            WorkflowState::Waiting => WorkflowState::Todo,
            _ => return TransitionOutcome::Illegal,
        },
        // L'échéance d'un report restaure l'état, elle ne le décide pas.
        TransitionCause::SnoozeExpired => match target {
            Some(t) => t,
            None => return TransitionOutcome::Illegal,
        },
    };

    if to == current {
        TransitionOutcome::Unchanged
    } else {
        TransitionOutcome::Moved { from: current, to }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use WorkflowState::*;

    fn on() -> AutomationSettings {
        AutomationSettings::default()
    }

    #[test]
    fn repondre_met_en_attente() {
        let r = transition(Todo, TransitionCause::ReplySent, None, &on());
        assert_eq!(
            r,
            TransitionOutcome::Moved {
                from: Todo,
                to: Waiting
            }
        );
    }

    #[test]
    fn repondre_a_un_fil_termine_ne_le_rouvre_pas() {
        // Un remerciement final ne doit pas ramener le fil dans la file de travail.
        let r = transition(Done, TransitionCause::ReplySent, None, &on());
        assert_eq!(r, TransitionOutcome::Unchanged);
    }

    #[test]
    fn un_nouveau_message_ramene_dans_la_file() {
        for from in [Waiting, Done] {
            let r = transition(from, TransitionCause::MessageReceived, None, &on());
            assert_eq!(r, TransitionOutcome::Moved { from, to: Todo });
        }
    }

    #[test]
    fn un_nouveau_message_sur_un_fil_deja_a_traiter_ne_change_rien() {
        let r = transition(Todo, TransitionCause::MessageReceived, None, &on());
        assert_eq!(r, TransitionOutcome::Unchanged);
    }

    #[test]
    fn la_relance_ne_concerne_que_les_fils_en_attente() {
        assert_eq!(
            transition(Waiting, TransitionCause::FollowUpDue, None, &on()),
            TransitionOutcome::Moved {
                from: Waiting,
                to: Todo
            }
        );
        assert_eq!(
            transition(Done, TransitionCause::FollowUpDue, None, &on()),
            TransitionOutcome::Illegal
        );
        assert_eq!(
            transition(Todo, TransitionCause::FollowUpDue, None, &on()),
            TransitionOutcome::Illegal
        );
    }

    #[test]
    fn chaque_automatisme_est_desactivable_independamment() {
        let mut s = on();
        s.reply_marks_waiting = false;
        assert_eq!(
            transition(Todo, TransitionCause::ReplySent, None, &s),
            TransitionOutcome::Disabled
        );
        // Les autres continuent de fonctionner.
        assert!(transition(Done, TransitionCause::MessageReceived, None, &s).changed());
    }

    #[test]
    fn le_mode_manuel_bloque_tous_les_automatismes() {
        let s = AutomationSettings::MANUAL_ONLY;
        for cause in [
            TransitionCause::ReplySent,
            TransitionCause::MessageReceived,
            TransitionCause::FollowUpDue,
        ] {
            assert_eq!(
                transition(Todo, cause, Some(Done), &s),
                TransitionOutcome::Disabled
            );
        }
        // Mais l'action manuelle passe toujours.
        assert!(transition(Todo, TransitionCause::Manual, Some(Done), &s).changed());
    }

    #[test]
    fn une_action_manuelle_sans_cible_est_illegale() {
        assert_eq!(
            transition(Todo, TransitionCause::Manual, None, &on()),
            TransitionOutcome::Illegal
        );
    }

    #[test]
    fn le_report_restaure_l_etat_precedent() {
        let s = Snooze {
            until: Timestamp::from_millis(1_000),
            restore_to: Waiting,
        };
        assert!(!s.is_due(Timestamp::from_millis(999)));
        assert!(s.is_due(Timestamp::from_millis(1_000)));
        assert_eq!(
            transition(
                Todo,
                TransitionCause::SnoozeExpired,
                Some(s.restore_to),
                &on()
            ),
            TransitionOutcome::Moved {
                from: Todo,
                to: Waiting
            }
        );
    }

    #[test]
    fn les_etats_font_un_aller_retour_texte_et_entier() {
        for st in WorkflowState::ALL {
            assert_eq!(WorkflowState::parse(st.as_str()), Some(st));
            assert_eq!(WorkflowState::from_i64(st.as_i64()), Some(st));
        }
        assert_eq!(WorkflowState::parse("inconnu"), None);
        assert_eq!(WorkflowState::from_i64(7), None);
    }

    #[test]
    fn seul_a_traiter_est_actif() {
        assert!(Todo.is_active());
        assert!(!Waiting.is_active());
        assert!(!Done.is_active());
    }
}
