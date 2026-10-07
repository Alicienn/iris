//! La file d'envoi, avec son délai d'annulation.
//!
//! Envoyer immédiatement est une erreur d'ergonomie : la faute de frappe, la pièce
//! jointe oubliée et le mauvais destinataire se remarquent dans les secondes qui
//! suivent le clic, jamais avant. Un message part donc après un délai — cinq secondes
//! par défaut, réglable — pendant lequel un seul geste le retient.
//!
//! Le délai est **avant l'envoi, pas après** : rappeler un message déjà parti est
//! impossible, et prétendre le contraire serait mentir à l'utilisateur.

use crate::compose::Outgoing;
use crate::transport::{Mailer, SendOutcome};
use iris_types::Error;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Délai d'annulation par défaut.
pub const DEFAULT_DELAY: Duration = Duration::from_secs(5);

/// Référence à un message en attente d'envoi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SendHandle(pub u64);

/// Ce qui arrive à un message de la file.
#[derive(Debug)]
pub enum OutboxEvent {
    Queued {
        handle: SendHandle,
        subject: String,
    },
    Cancelled {
        handle: SendHandle,
    },
    Sent {
        handle: SendHandle,
        outcome: SendOutcome,
    },
    Failed {
        handle: SendHandle,
        error: Error,
    },
}

impl OutboxEvent {
    pub fn handle(&self) -> SendHandle {
        match self {
            Self::Queued { handle, .. }
            | Self::Cancelled { handle }
            | Self::Sent { handle, .. }
            | Self::Failed { handle, .. } => *handle,
        }
    }
}

/// La file d'envoi.
#[derive(Debug)]
pub struct Outbox {
    mailer: Arc<dyn Mailer>,
    /// En millisecondes. Réglable en marche : le délai se choisit dans les réglages,
    /// et chaque message part avec celui qui valait quand on l'a envoyé.
    delay: AtomicU64,
    pending: Arc<Mutex<HashMap<SendHandle, oneshot::Sender<()>>>>,
    next: AtomicU64,
    events: mpsc::UnboundedSender<OutboxEvent>,
    /// L'exécuteur sur lequel les envois partent.
    ///
    /// Porté par la file plutôt que déduit de l'appelant, et c'est ce qui a coûté un
    /// plantage : `queue` faisait `tokio::spawn`, qui exige d'être appelé **depuis**
    /// un exécuteur. Le bouton « Send » l'appelait depuis le fil de l'interface, où il
    /// n'y en a aucun, et l'application disparaissait au clic avec « there is no
    /// reactor running ». Rien dans la signature ne disait qu'il fallait un exécuteur ;
    /// maintenant, il est impossible d'en construire une sans.
    runtime: tokio::runtime::Handle,
}

impl Outbox {
    /// Crée la file et rend le flux d'événements.
    pub fn new(
        mailer: Arc<dyn Mailer>,
        delay: Duration,
        runtime: tokio::runtime::Handle,
    ) -> (Self, mpsc::UnboundedReceiver<OutboxEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                mailer,
                delay: AtomicU64::new(delay.as_millis() as u64),
                pending: Arc::new(Mutex::new(HashMap::new())),
                next: AtomicU64::new(1),
                events: tx,
                runtime,
            },
            rx,
        )
    }

    pub fn delay(&self) -> Duration {
        Duration::from_millis(self.delay.load(Ordering::Relaxed))
    }

    /// Change le délai des prochains messages ; ceux déjà en file gardent le leur.
    pub fn set_delay(&self, delay: Duration) {
        self.delay
            .store(delay.as_millis() as u64, Ordering::Relaxed);
    }

    /// Met un message en file. Il partira après le délai, sauf annulation.
    pub fn queue(&self, message: Outgoing) -> SendHandle {
        let handle = SendHandle(self.next.fetch_add(1, Ordering::Relaxed));
        let (annuler_tx, annuler_rx) = oneshot::channel();

        self.pending
            .lock()
            .expect("file empoisonnée")
            .insert(handle, annuler_tx);
        let _ = self.events.send(OutboxEvent::Queued {
            handle,
            subject: message.subject.clone(),
        });

        let mailer = Arc::clone(&self.mailer);
        let pending = Arc::clone(&self.pending);
        let events = self.events.clone();
        let delay = self.delay();

        self.runtime.spawn(async move {
            // L'annulation réveille plus tôt, l'échéance à l'heure. Ni l'un ni l'autre
            // ne décide : quand les deux étaient prêts ensemble, `select!` tirait au
            // sort, et un message déclaré « annulé » à l'écran partait quand même.
            tokio::select! {
                biased;
                _ = annuler_rx => {}
                _ = tokio::time::sleep(delay) => {}
            }

            // Ce qui décide, c'est qui retire le message de la file, sous le verrou
            // que `cancel` prend aussi : exactement l'un des deux le trouve. Passé ce
            // point, plus rien ne peut être retenu.
            let annule = pending
                .lock()
                .expect("file empoisonnée")
                .remove(&handle)
                .is_none();

            if annule {
                let _ = events.send(OutboxEvent::Cancelled { handle });
                return;
            }

            match mailer.send(&message).await {
                Ok(outcome) => {
                    let _ = events.send(OutboxEvent::Sent { handle, outcome });
                }
                Err(error) => {
                    let _ = events.send(OutboxEvent::Failed { handle, error });
                }
            }
        });

        handle
    }

    /// Retient un message encore en attente.
    ///
    /// Retourne `false` s'il est déjà parti — auquel cas il faut le dire à
    /// l'utilisateur, et non faire semblant.
    ///
    /// L'avoir retiré de la file suffit : l'envoi ne part que s'il l'y trouve encore.
    /// Le signal ne sert qu'à réveiller la tâche avant l'échéance.
    pub fn cancel(&self, handle: SendHandle) -> bool {
        let envoyeur = self
            .pending
            .lock()
            .expect("file empoisonnée")
            .remove(&handle);
        match envoyeur {
            Some(tx) => {
                let _ = tx.send(());
                true
            }
            None => false,
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("file empoisonnée").len()
    }

    pub fn is_pending(&self, handle: SendHandle) -> bool {
        self.pending
            .lock()
            .expect("file empoisonnée")
            .contains_key(&handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FakeMailer;
    use iris_types::Address;

    fn message(sujet: &str) -> Outgoing {
        Outgoing::new(
            Address::new("moi@example.com"),
            vec![Address::new("marie@example.com")],
            sujet,
        )
        .body("Bonjour")
    }

    fn file(
        delay: Duration,
    ) -> (
        Outbox,
        mpsc::UnboundedReceiver<OutboxEvent>,
        Arc<FakeMailer>,
    ) {
        let mailer = Arc::new(FakeMailer::new());
        let (outbox, rx) = Outbox::new(
            Arc::clone(&mailer) as Arc<dyn Mailer>,
            delay,
            tokio::runtime::Handle::current(),
        );
        (outbox, rx, mailer)
    }

    #[tokio::test(start_paused = true)]
    async fn un_message_part_apres_le_delai() {
        let (outbox, mut evenements, mailer) = file(DEFAULT_DELAY);
        let h = outbox.queue(message("Devis"));

        assert!(matches!(
            evenements.recv().await,
            Some(OutboxEvent::Queued { .. })
        ));
        assert_eq!(mailer.count(), 0, "rien ne doit partir immédiatement");

        tokio::time::advance(Duration::from_secs(11)).await;

        match evenements.recv().await {
            Some(OutboxEvent::Sent { handle, .. }) => assert_eq!(handle, h),
            autre => panic!("attendu un envoi, obtenu {autre:?}"),
        }
        assert_eq!(mailer.count(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn une_annulation_pendant_le_delai_retient_le_message() {
        let (outbox, mut evenements, mailer) = file(DEFAULT_DELAY);
        let h = outbox.queue(message("Devis"));
        let _ = evenements.recv().await;

        tokio::time::advance(Duration::from_secs(3)).await;
        assert!(outbox.cancel(h));

        match evenements.recv().await {
            Some(OutboxEvent::Cancelled { handle }) => assert_eq!(handle, h),
            autre => panic!("attendu une annulation, obtenu {autre:?}"),
        }

        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(mailer.count(), 0, "le message ne doit jamais partir");
    }

    #[test]
    fn une_annulation_acceptee_retient_le_message_meme_a_l_echeance() {
        // Le délai est nul : l'échéance est prête avant même que la tâche ne
        // démarre. L'annulation passe avant qu'elle ne s'exécute, sur un exécuteur
        // qui ne tourne pas encore. L'écran dira « annulé » : rien ne doit partir.
        let executeur = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let mailer = Arc::new(FakeMailer::new());
        let (outbox, mut evenements) = Outbox::new(
            Arc::clone(&mailer) as Arc<dyn Mailer>,
            Duration::ZERO,
            executeur.handle().clone(),
        );

        let h = outbox.queue(message("Devis"));
        assert!(outbox.cancel(h));

        executeur.block_on(async {
            assert!(matches!(
                evenements.recv().await,
                Some(OutboxEvent::Queued { .. })
            ));
            match evenements.recv().await {
                Some(OutboxEvent::Cancelled { handle }) => assert_eq!(handle, h),
                autre => panic!("attendu une annulation, obtenu {autre:?}"),
            }
        });
        assert_eq!(mailer.count(), 0, "un message déclaré annulé ne part pas");
    }

    #[tokio::test(start_paused = true)]
    async fn annuler_un_message_deja_parti_echoue_franchement() {
        // Prétendre rappeler un message déjà envoyé serait mentir à l'utilisateur.
        let (outbox, mut evenements, _) = file(Duration::from_secs(1));
        let h = outbox.queue(message("Devis"));
        let _ = evenements.recv().await;

        tokio::time::advance(Duration::from_secs(2)).await;
        let _ = evenements.recv().await;

        assert!(!outbox.cancel(h));
    }

    #[tokio::test(start_paused = true)]
    async fn plusieurs_messages_coexistent() {
        let (outbox, mut evenements, mailer) = file(DEFAULT_DELAY);
        let a = outbox.queue(message("Premier"));
        let b = outbox.queue(message("Second"));
        let c = outbox.queue(message("Troisième"));

        for _ in 0..3 {
            let _ = evenements.recv().await;
        }
        assert_eq!(outbox.pending_count(), 3);

        assert!(outbox.cancel(b));
        tokio::time::advance(Duration::from_secs(11)).await;

        // Deux envois et une annulation, dans un ordre non garanti.
        let mut envoyes = Vec::new();
        let mut annules = Vec::new();
        for _ in 0..3 {
            match evenements.recv().await {
                Some(OutboxEvent::Sent { handle, .. }) => envoyes.push(handle),
                Some(OutboxEvent::Cancelled { handle }) => annules.push(handle),
                autre => panic!("événement inattendu : {autre:?}"),
            }
        }
        envoyes.sort();
        assert_eq!(envoyes, [a, c]);
        assert_eq!(annules, [b]);
        assert_eq!(mailer.count(), 2);
        assert_eq!(outbox.pending_count(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn un_echec_d_envoi_est_rapporte() {
        let (outbox, mut evenements, mailer) = file(Duration::from_secs(1));
        mailer.fail_next(Error::network("serveur injoignable"));

        outbox.queue(message("Devis"));
        let _ = evenements.recv().await;
        tokio::time::advance(Duration::from_secs(2)).await;

        match evenements.recv().await {
            Some(OutboxEvent::Failed { error, .. }) => assert!(error.is_transient()),
            autre => panic!("attendu un échec, obtenu {autre:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn un_delai_nul_envoie_sans_attendre() {
        // Réglage possible pour qui trouve le délai pénible.
        let (outbox, mut evenements, mailer) = file(Duration::ZERO);
        outbox.queue(message("Devis"));
        let _ = evenements.recv().await;

        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(matches!(
            evenements.recv().await,
            Some(OutboxEvent::Sent { .. })
        ));
        assert_eq!(mailer.count(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn l_etat_d_attente_est_consultable() {
        let (outbox, mut evenements, _) = file(DEFAULT_DELAY);
        let h = outbox.queue(message("Devis"));
        let _ = evenements.recv().await;

        assert!(outbox.is_pending(h));
        outbox.cancel(h);
        assert!(!outbox.is_pending(h));
    }

    #[tokio::test(start_paused = true)]
    async fn annuler_deux_fois_ne_produit_qu_une_annulation() {
        let (outbox, mut evenements, _) = file(DEFAULT_DELAY);
        let h = outbox.queue(message("Devis"));
        let _ = evenements.recv().await;

        assert!(outbox.cancel(h));
        assert!(!outbox.cancel(h));
    }

    #[test]
    fn mettre_en_file_depuis_un_fil_ordinaire_ne_fait_pas_paniquer() {
        // C'est exactement ce que fait le bouton « Send ».
        //
        // L'interface tourne sur le fil principal, où il n'y a aucun exécuteur, et
        // `queue` faisait `tokio::spawn` — qui exige d'être appelé depuis un exécuteur
        // et panique sinon. Cliquer « Send » faisait donc disparaître l'application,
        // avec « there is no reactor running » dans le journal et rien à l'écran.
        //
        // Tous les autres tests de ce fichier sont `#[tokio::test]`, c'est-à-dire
        // exactement le contexte que l'application n'a pas : ils ne pouvaient pas le
        // voir. Celui-ci est volontairement synchrone, et n'a pas d'autre raison
        // d'être.
        let executeur = tokio::runtime::Runtime::new().unwrap();
        let mailer = Arc::new(FakeMailer::new());
        let (outbox, mut evenements) = Outbox::new(
            Arc::clone(&mailer) as Arc<dyn Mailer>,
            DEFAULT_DELAY,
            executeur.handle().clone(),
        );

        let h = outbox.queue(message("Devis"));

        assert!(outbox.is_pending(h), "le message est bien en attente");
        assert!(matches!(
            evenements.try_recv(),
            Ok(OutboxEvent::Queued { .. })
        ));
    }
}
