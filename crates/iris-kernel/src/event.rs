//! Les événements qui circulent sur le bus.
//!
//! Un seul type énuméré plutôt qu'un bus générique par message : le nombre
//! d'événements du domaine est fini et connu, et une énumération fermée permet au
//! compilateur de vérifier que chaque abonné traite les cas qui le concernent. Les
//! extensions passent par la variante `Custom`, qui est le seul point ouvert.

use iris_types::{
    AccountId, FolderId, MessageId, ThreadId, Timestamp, TransitionCause, WorkflowState,
};
use std::sync::Arc;

/// Étape courante de la synchronisation d'un compte, telle qu'affichée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPhase {
    Connecting,
    ListingFolders,
    FetchingHeaders { done: u32, total: u32 },
    Reconciling,
    Idle,
}

#[derive(Debug, Clone)]
pub enum Event {
    // --- Comptes ---
    AccountAdded(AccountId),
    AccountRemoved(AccountId),
    /// Le compte est devenu celui que regarde l'utilisateur. L'ordonnanceur de
    /// synchronisation s'en sert pour réattribuer ses connexions.
    AccountFocused(AccountId),

    // --- Messages ---
    /// De nouveaux messages sont arrivés. Ne transporte que des identifiants : le
    /// contenu se lit dans le store, et un événement ne doit jamais être un
    /// véhicule de données volumineuses.
    MessagesAdded { account: AccountId, folder: FolderId, ids: Arc<[MessageId]> },
    MessagesRemoved { account: AccountId, folder: FolderId, ids: Arc<[MessageId]> },
    FlagsChanged { message: MessageId, thread: ThreadId },

    // --- Fils et workflow ---
    ThreadStateChanged {
        thread: ThreadId,
        from: WorkflowState,
        to: WorkflowState,
        cause: TransitionCause,
    },
    ThreadSnoozed { thread: ThreadId, until: Timestamp },
    ThreadUnsnoozed { thread: ThreadId },

    // --- Synchronisation ---
    SyncPhaseChanged { account: AccountId, phase: SyncPhase },
    SyncFailed { account: AccountId, message: Arc<str>, transient: bool },

    // --- Présentation ---
    ThemeReloaded { name: Arc<str> },
    IndexUpdated { documents: u64 },

    // --- Extensions ---
    /// Émis par un plugin. `source` est l'identifiant du plugin, ce qui permet de
    /// tracer qui a produit quoi et de filtrer par origine.
    Custom { source: Arc<str>, name: Arc<str>, payload: Arc<serde_json::Value> },
}

/// Famille d'un événement, pour s'abonner sans énumérer chaque variante.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    Account,
    Messages,
    Workflow,
    Sync,
    Presentation,
    Custom,
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Self::AccountAdded(_) | Self::AccountRemoved(_) | Self::AccountFocused(_) => {
                EventKind::Account
            }
            Self::MessagesAdded { .. }
            | Self::MessagesRemoved { .. }
            | Self::FlagsChanged { .. } => EventKind::Messages,
            Self::ThreadStateChanged { .. }
            | Self::ThreadSnoozed { .. }
            | Self::ThreadUnsnoozed { .. } => EventKind::Workflow,
            Self::SyncPhaseChanged { .. } | Self::SyncFailed { .. } => EventKind::Sync,
            Self::ThemeReloaded { .. } | Self::IndexUpdated { .. } => EventKind::Presentation,
            Self::Custom { .. } => EventKind::Custom,
        }
    }

    /// Compte concerné, lorsque l'événement en désigne un.
    pub fn account(&self) -> Option<AccountId> {
        match self {
            Self::AccountAdded(a) | Self::AccountRemoved(a) | Self::AccountFocused(a) => Some(*a),
            Self::MessagesAdded { account, .. }
            | Self::MessagesRemoved { account, .. }
            | Self::SyncPhaseChanged { account, .. }
            | Self::SyncFailed { account, .. } => Some(*account),
            _ => None,
        }
    }

    /// Cet événement modifie-t-il ce que l'utilisateur voit à l'écran ?
    ///
    /// Sert à décider si l'interface doit être réveillée. Une phase de
    /// synchronisation intermédiaire, par exemple, n'a pas à provoquer de nouvelle
    /// frame en dehors de l'indicateur de progression.
    pub fn affects_view(&self) -> bool {
        !matches!(self, Self::SyncPhaseChanged { phase: SyncPhase::Reconciling, .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chaque_evenement_a_une_famille() {
        assert_eq!(Event::AccountAdded(AccountId(1)).kind(), EventKind::Account);
        assert_eq!(
            Event::ThreadUnsnoozed { thread: ThreadId(1) }.kind(),
            EventKind::Workflow
        );
        assert_eq!(
            Event::IndexUpdated { documents: 3 }.kind(),
            EventKind::Presentation
        );
    }

    #[test]
    fn le_compte_concerne_est_extrait() {
        let e = Event::MessagesAdded {
            account: AccountId(7),
            folder: FolderId(1),
            ids: Arc::from(vec![MessageId(1)]),
        };
        assert_eq!(e.account(), Some(AccountId(7)));
        assert_eq!(Event::IndexUpdated { documents: 1 }.account(), None);
    }

    #[test]
    fn la_reconciliation_ne_reveille_pas_l_interface() {
        let muet = Event::SyncPhaseChanged {
            account: AccountId(1),
            phase: SyncPhase::Reconciling,
        };
        let visible = Event::SyncPhaseChanged {
            account: AccountId(1),
            phase: SyncPhase::FetchingHeaders { done: 1, total: 10 },
        };
        assert!(!muet.affects_view());
        assert!(visible.affects_view());
    }

    #[test]
    fn les_evenements_ne_transportent_pas_de_donnees_volumineuses() {
        // Les identifiants sont partagés par pointeur : diffuser un lot de 10 000
        // messages à N abonnés ne doit pas copier 10 000 entiers N fois.
        let ids: Arc<[MessageId]> = Arc::from(vec![MessageId(1), MessageId(2)]);
        let e = Event::MessagesAdded {
            account: AccountId(1),
            folder: FolderId(1),
            ids: Arc::clone(&ids),
        };
        let copie = e.clone();
        match copie {
            Event::MessagesAdded { ids: autres, .. } => {
                assert!(Arc::ptr_eq(&ids, &autres));
            }
            _ => unreachable!(),
        }
    }
}
