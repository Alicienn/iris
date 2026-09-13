//! Coalescence des événements en diffs d'affichage.
//!
//! Invariant n° 4 de la spécification : une synchronisation de 100 comptes ne doit
//! pas réveiller l'interface mille fois par seconde. Les événements sont donc
//! accumulés par fenêtres de 16 ms — la durée d'une frame — et fusionnés en un lot
//! minimal décrivant *ce qu'il faut redessiner*, pas *ce qui s'est passé*.
//!
//! Le lot est en outre **borné** : au-delà d'un certain nombre de fils touchés, il
//! est plus économique d'invalider la liste entière que d'énumérer les lignes. Cela
//! garantit un coût de traitement constant côté interface, quelle que soit
//! l'intensité de la synchronisation.

use crate::bus::Subscription;
use crate::event::Event;
use iris_types::{AccountId, ThreadId, WorkflowState};
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::sync::mpsc;

/// Durée d'une fenêtre de coalescence : une frame à 60 Hz.
pub const WINDOW: Duration = Duration::from_millis(16);

/// Au-delà de ce nombre de fils touchés, on invalide la liste au lieu de les lister.
const MAX_THREADS: usize = 256;

/// Ce que l'interface doit redessiner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewDiff {
    /// Tout est à refaire : au-delà d'un certain volume, c'est le moins coûteux.
    pub full_refresh: bool,
    /// Fils dont le contenu affiché a changé.
    pub threads: BTreeSet<ThreadId>,
    /// Files dont l'ordre ou la composition a changé.
    pub lists: BTreeSet<WorkflowState>,
    /// Comptes dont les compteurs ont bougé.
    pub accounts: BTreeSet<AccountId>,
    /// Nombre d'événements fusionnés dans ce lot, pour le diagnostic.
    pub merged: u32,
}

impl ViewDiff {
    pub fn is_empty(&self) -> bool {
        !self.full_refresh
            && self.threads.is_empty()
            && self.lists.is_empty()
            && self.accounts.is_empty()
    }

    /// Intègre un événement au lot en cours.
    pub fn absorb(&mut self, event: &Event) {
        self.merged = self.merged.saturating_add(1);

        if self.full_refresh {
            // Rien à préciser : tout est déjà invalidé.
            if let Some(a) = event.account() {
                self.accounts.insert(a);
            }
            return;
        }

        match event {
            Event::MessagesAdded { account, .. } | Event::MessagesRemoved { account, .. } => {
                self.accounts.insert(*account);
                // L'arrivée ou le départ de messages réordonne la file active.
                self.lists.insert(WorkflowState::Todo);
            }
            Event::FlagsChanged { thread, .. } => {
                self.threads.insert(*thread);
            }
            Event::ThreadStateChanged { thread, from, to, .. } => {
                self.threads.insert(*thread);
                self.lists.insert(*from);
                self.lists.insert(*to);
            }
            Event::ThreadSnoozed { thread, .. } | Event::ThreadUnsnoozed { thread } => {
                self.threads.insert(*thread);
                // Un report retire ou remet une ligne : la file entière se décale.
                self.lists.insert(WorkflowState::Todo);
                self.lists.insert(WorkflowState::Waiting);
            }
            Event::AccountAdded(a) | Event::AccountRemoved(a) | Event::AccountFocused(a) => {
                self.accounts.insert(*a);
                self.full_refresh = true;
            }
            Event::ThemeReloaded { .. } => {
                // Chaque pixel dépend des tokens.
                self.full_refresh = true;
            }
            Event::SyncPhaseChanged { account, .. } | Event::SyncFailed { account, .. } => {
                self.accounts.insert(*account);
            }
            Event::IndexUpdated { .. } | Event::Custom { .. } => {}
        }

        if self.threads.len() > MAX_THREADS {
            self.degrade();
        }
    }

    /// Remplace une énumération devenue trop longue par une invalidation globale.
    fn degrade(&mut self) {
        self.full_refresh = true;
        self.threads.clear();
        self.lists.clear();
    }

    fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

/// Transforme un flux d'événements en un flux de lots, un par fenêtre au maximum.
///
/// Le premier événement d'une période calme est attendu sans limite de temps ; une
/// fois arrivé, la fenêtre s'ouvre et tout ce qui suit pendant 16 ms est fusionné
/// avec lui. Une application au repos ne consomme donc aucun temps processeur.
pub fn spawn(mut sub: Subscription, window: Duration) -> mpsc::Receiver<ViewDiff> {
    let (tx, rx) = mpsc::channel(8);

    tokio::spawn(async move {
        loop {
            // Attente passive du premier événement du lot.
            let Some(first) = sub.recv().await else { break };

            let mut batch = ViewDiff::default();
            batch.absorb(&first);

            let deadline = tokio::time::Instant::now() + window;
            loop {
                match tokio::time::timeout_at(deadline, sub.recv()).await {
                    Ok(Some(e)) => batch.absorb(&e),
                    // Bus fermé : on émet ce qu'on a puis on s'arrête.
                    Ok(None) => {
                        let _ = tx.send(batch.take()).await;
                        return;
                    }
                    Err(_) => break,
                }
            }

            if !batch.is_empty() && tx.send(batch.take()).await.is_err() {
                break; // plus personne n'écoute
            }
        }
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::event::SyncPhase;
    use iris_types::{FolderId, MessageId, Timestamp, TransitionCause};
    use std::sync::Arc;

    fn state_change(t: i64, from: WorkflowState, to: WorkflowState) -> Event {
        Event::ThreadStateChanged {
            thread: ThreadId(t),
            from,
            to,
            cause: TransitionCause::Manual,
        }
    }

    #[test]
    fn un_lot_vide_est_vide() {
        assert!(ViewDiff::default().is_empty());
    }

    #[test]
    fn les_memes_fils_ne_sont_comptes_qu_une_fois() {
        let mut d = ViewDiff::default();
        for _ in 0..50 {
            d.absorb(&Event::FlagsChanged { message: MessageId(1), thread: ThreadId(3) });
        }
        assert_eq!(d.threads.len(), 1);
        assert_eq!(d.merged, 50, "le nombre d'événements fusionnés reste visible");
    }

    #[test]
    fn un_changement_d_etat_invalide_les_deux_files() {
        let mut d = ViewDiff::default();
        d.absorb(&state_change(1, WorkflowState::Todo, WorkflowState::Waiting));
        assert!(d.lists.contains(&WorkflowState::Todo));
        assert!(d.lists.contains(&WorkflowState::Waiting));
        assert!(d.threads.contains(&ThreadId(1)));
    }

    #[test]
    fn au_dela_du_seuil_le_lot_degenere_en_rafraichissement_complet() {
        // Propriété de performance : le coût côté interface reste borné même si la
        // synchronisation touche des dizaines de milliers de fils.
        let mut d = ViewDiff::default();
        for i in 0..(MAX_THREADS as i64 + 10) {
            d.absorb(&Event::FlagsChanged { message: MessageId(i), thread: ThreadId(i) });
        }
        assert!(d.full_refresh);
        assert!(d.threads.is_empty(), "l'énumération est abandonnée");
        assert!(!d.is_empty());
    }

    #[test]
    fn apres_degradation_les_comptes_restent_suivis() {
        let mut d = ViewDiff::default();
        d.absorb(&Event::ThemeReloaded { name: Arc::from("mono") });
        assert!(d.full_refresh);
        d.absorb(&Event::MessagesAdded {
            account: AccountId(4),
            folder: FolderId(1),
            ids: Arc::from(vec![MessageId(1)]),
        });
        assert!(d.accounts.contains(&AccountId(4)));
    }

    #[test]
    fn un_evenement_sans_effet_visuel_ne_remplit_pas_le_lot() {
        let mut d = ViewDiff::default();
        d.absorb(&Event::IndexUpdated { documents: 10 });
        assert!(d.is_empty());
        assert_eq!(d.merged, 1);
    }

    #[test]
    fn une_erreur_de_synchronisation_ne_touche_que_le_compte() {
        let mut d = ViewDiff::default();
        d.absorb(&Event::SyncFailed {
            account: AccountId(2),
            message: Arc::from("timeout"),
            transient: true,
        });
        assert_eq!(d.accounts.len(), 1);
        assert!(d.threads.is_empty());
        assert!(!d.full_refresh);
    }

    #[tokio::test(start_paused = true)]
    async fn mille_evenements_produisent_un_seul_lot() {
        let bus = EventBus::new();
        let mut lots = spawn(bus.subscribe(), WINDOW);

        for i in 0..1000 {
            bus.publish(Event::FlagsChanged {
                message: MessageId(i),
                thread: ThreadId(i % 5),
            });
        }

        let lot = lots.recv().await.expect("un lot");
        // Cinq fils distincts, mille événements : c'est exactement l'économie visée.
        assert_eq!(lot.threads.len(), 5);
        assert_eq!(lot.merged, 1000);
    }

    #[tokio::test(start_paused = true)]
    async fn deux_rafales_espacees_produisent_deux_lots() {
        let bus = EventBus::new();
        let mut lots = spawn(bus.subscribe(), WINDOW);

        bus.publish(state_change(1, WorkflowState::Todo, WorkflowState::Done));
        let premier = lots.recv().await.unwrap();
        assert!(premier.threads.contains(&ThreadId(1)));

        tokio::time::sleep(Duration::from_millis(100)).await;

        bus.publish(state_change(2, WorkflowState::Todo, WorkflowState::Done));
        let second = lots.recv().await.unwrap();
        assert!(second.threads.contains(&ThreadId(2)));
        assert!(!second.threads.contains(&ThreadId(1)), "les lots sont indépendants");
    }

    #[tokio::test(start_paused = true)]
    async fn une_application_au_repos_n_emet_rien() {
        let bus = EventBus::new();
        let mut lots = spawn(bus.subscribe(), WINDOW);

        // Un événement sans effet visuel ne doit pas produire de frame.
        bus.publish(Event::SyncPhaseChanged {
            account: AccountId(1),
            phase: SyncPhase::Idle,
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Celui-ci porte bien un compte, donc un lot minimal est légitime.
        let lot = lots.recv().await.unwrap();
        assert_eq!(lot.accounts.len(), 1);
        assert!(lot.threads.is_empty());

        // Puis plus rien pendant une longue période.
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(lots.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn le_report_reordonne_les_deux_files_concernees() {
        let bus = EventBus::new();
        let mut lots = spawn(bus.subscribe(), WINDOW);
        bus.publish(Event::ThreadSnoozed {
            thread: ThreadId(1),
            until: Timestamp::from_millis(10),
        });
        let lot = lots.recv().await.unwrap();
        assert!(lot.lists.contains(&WorkflowState::Todo));
        assert!(lot.lists.contains(&WorkflowState::Waiting));
        assert!(!lot.lists.contains(&WorkflowState::Done));
    }
}
