//! Le bus d'événements.
//!
//! Diffusion multi-abonnés, asynchrone, sans que l'émetteur connaisse ses abonnés.
//! Un abonné lent ne bloque jamais l'émetteur : il perd des événements et en est
//! informé. C'est un choix délibéré — pendant une synchronisation de 100 comptes,
//! bloquer le producteur parce qu'un panneau d'interface est occupé serait la
//! garantie d'une application qui se fige.

use crate::event::{Event, EventKind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Nombre d'événements conservés pour les abonnés en retard.
///
/// Dimensionné pour absorber une rafale de synchronisation complète sans perte dans
/// les cas normaux, sans immobiliser trop de mémoire.
const CAPACITY: usize = 1024;

#[derive(Debug)]
struct Inner {
    tx: broadcast::Sender<Event>,
    published: AtomicU64,
}

/// Poignée de publication, clonable et partageable entre tâches.
#[derive(Debug, Clone)]
pub struct EventBus {
    inner: Arc<Inner>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { inner: Arc::new(Inner { tx, published: AtomicU64::new(0) }) }
    }

    /// Publie un événement. Ne bloque jamais, même sans abonné.
    pub fn publish(&self, event: Event) {
        self.inner.published.fetch_add(1, Ordering::Relaxed);
        // Une erreur signifie « aucun abonné », ce qui est un état normal au
        // démarrage et pendant l'arrêt.
        let _ = self.inner.tx.send(event);
    }

    /// Abonnement à tous les événements.
    pub fn subscribe(&self) -> Subscription {
        Subscription { rx: self.inner.tx.subscribe(), filter: Filter::All, lagged: 0 }
    }

    /// Abonnement restreint à une famille.
    pub fn subscribe_kind(&self, kind: EventKind) -> Subscription {
        Subscription { rx: self.inner.tx.subscribe(), filter: Filter::Kind(kind), lagged: 0 }
    }

    /// Abonnement restreint aux événements qui modifient l'affichage.
    pub fn subscribe_view(&self) -> Subscription {
        Subscription { rx: self.inner.tx.subscribe(), filter: Filter::View, lagged: 0 }
    }

    /// Nombre total d'événements publiés depuis la création.
    pub fn published_count(&self) -> u64 {
        self.inner.published.load(Ordering::Relaxed)
    }

    pub fn subscriber_count(&self) -> usize {
        self.inner.tx.receiver_count()
    }
}

#[derive(Debug, Clone, Copy)]
enum Filter {
    All,
    Kind(EventKind),
    View,
}

impl Filter {
    fn accepts(self, e: &Event) -> bool {
        match self {
            Self::All => true,
            Self::Kind(k) => e.kind() == k,
            Self::View => e.affects_view(),
        }
    }
}

/// Un abonnement. Chaque abonné possède son propre curseur de lecture.
#[derive(Debug)]
pub struct Subscription {
    rx: broadcast::Receiver<Event>,
    filter: Filter,
    lagged: u64,
}

impl Subscription {
    /// Attend l'événement suivant qui satisfait le filtre.
    ///
    /// Retourne `None` quand le bus est définitivement fermé. Les événements perdus
    /// pour cause de retard sont comptés puis la lecture reprend : perdre des
    /// événements est préférable à bloquer le producteur, mais doit rester visible.
    pub async fn recv(&mut self) -> Option<Event> {
        loop {
            match self.rx.recv().await {
                Ok(e) if self.filter.accepts(&e) => return Some(e),
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    self.lagged += n;
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Récupère sans attendre les événements déjà disponibles.
    pub fn drain(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => {
                    if self.filter.accepts(&e) {
                        out.push(e);
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(n)) => self.lagged += n,
                Err(_) => break,
            }
        }
        out
    }

    /// Nombre d'événements manqués parce que cet abonné n'a pas suivi le rythme.
    ///
    /// Une valeur non nulle est un signal d'alerte : l'abonné doit être allégé, ou
    /// son traitement déplacé hors du chemin critique.
    pub fn lagged(&self) -> u64 {
        self.lagged
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_types::{AccountId, ThreadId, Timestamp};

    fn evt(n: i64) -> Event {
        Event::AccountAdded(AccountId(n))
    }

    #[tokio::test]
    async fn tous_les_abonnes_recoivent_chaque_evenement() {
        let bus = EventBus::new();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();

        bus.publish(evt(1));

        assert!(matches!(a.recv().await, Some(Event::AccountAdded(AccountId(1)))));
        assert!(matches!(b.recv().await, Some(Event::AccountAdded(AccountId(1)))));
    }

    #[tokio::test]
    async fn publier_sans_abonne_ne_panique_pas() {
        let bus = EventBus::new();
        bus.publish(evt(1));
        assert_eq!(bus.published_count(), 1);
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn un_abonnement_par_famille_filtre() {
        let bus = EventBus::new();
        let mut workflow = bus.subscribe_kind(EventKind::Workflow);

        bus.publish(evt(1));
        bus.publish(Event::ThreadUnsnoozed { thread: ThreadId(9) });

        // Le premier événement n'est pas de la bonne famille : il est ignoré.
        assert!(matches!(
            workflow.recv().await,
            Some(Event::ThreadUnsnoozed { thread: ThreadId(9) })
        ));
    }

    #[tokio::test]
    async fn l_abonnement_vue_ignore_les_etapes_invisibles() {
        use crate::event::SyncPhase;
        let bus = EventBus::new();
        let mut vue = bus.subscribe_view();

        bus.publish(Event::SyncPhaseChanged {
            account: AccountId(1),
            phase: SyncPhase::Reconciling,
        });
        bus.publish(Event::ThreadSnoozed {
            thread: ThreadId(2),
            until: Timestamp::from_millis(1),
        });

        assert!(matches!(vue.recv().await, Some(Event::ThreadSnoozed { .. })));
    }

    #[tokio::test]
    async fn un_abonne_en_retard_perd_des_evenements_mais_repart() {
        let bus = EventBus::new();
        let mut lent = bus.subscribe();

        // Deux fois la capacité du tampon : le début est nécessairement écrasé.
        for i in 0..(CAPACITY as i64 * 2) {
            bus.publish(evt(i));
        }

        let recu = lent.recv().await.expect("le bus reste ouvert");
        assert!(lent.lagged() > 0, "la perte doit être comptabilisée");
        // On reprend quelque part dans la fenêtre encore disponible, pas au début.
        match recu {
            Event::AccountAdded(AccountId(n)) => assert!(n >= CAPACITY as i64),
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn drain_ne_bloque_pas_quand_il_n_y_a_rien() {
        let bus = EventBus::new();
        let mut s = bus.subscribe();
        assert!(s.drain().is_empty());

        bus.publish(evt(1));
        bus.publish(evt(2));
        assert_eq!(s.drain().len(), 2);
        assert!(s.drain().is_empty());
    }

    #[tokio::test]
    async fn fermer_le_bus_termine_les_abonnements() {
        let bus = EventBus::new();
        let mut s = bus.subscribe();
        drop(bus);
        assert!(s.recv().await.is_none());
    }
}
