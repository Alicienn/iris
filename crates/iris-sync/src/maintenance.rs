//! Le travail que fait le temps.
//!
//! Deux automatismes n'ont pas de déclencheur : ils dépendent uniquement de l'heure
//! qu'il est. Un report arrive à échéance, un fil attend une réponse depuis trop
//! longtemps. Sans une boucle qui les réveille, ce sont deux promesses que
//! l'application ne tient jamais — et l'utilisateur ne s'en aperçoit que le jour où
//! il attendait quelque chose.
//!
//! Le passage est **idempotent** : le repasser deux fois dans la même seconde ne
//! produit rien de plus. C'est ce qui permet de l'appeler à chaque tour de
//! synchronisation sans réfléchir.

use crate::engine::SyncEngine;
use iris_kernel::Event;
use iris_types::{transition, Result, Timestamp, TransitionCause, TransitionOutcome};

/// Ce qu'un passage a produit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaintenanceReport {
    /// Fils dont le report est arrivé à échéance.
    pub woken: usize,
    /// Fils ramenés dans la file faute de réponse.
    pub followed_up: usize,
    /// Contenus orphelins supprimés.
    pub purged_bodies: usize,
}

impl MaintenanceReport {
    pub fn changed(&self) -> bool {
        self.woken > 0 || self.followed_up > 0
    }
}

/// Nombre maximal de relances par passage.
///
/// Après une longue absence, des centaines de fils peuvent être dus d'un coup. Les
/// traiter tous ferait remonter une avalanche dans la file de travail ; les étaler
/// laisse à l'utilisateur une liste qu'il peut regarder.
const MAX_FOLLOW_UPS: u32 = 50;

impl SyncEngine {
    /// Réveille les reports échus et lance les relances dues.
    pub fn run_maintenance(&self, now: Timestamp) -> Result<MaintenanceReport> {
        Ok(MaintenanceReport {
            woken: self.wake_due_snoozes(now)?,
            followed_up: self.run_follow_ups(now)?,
            purged_bodies: 0,
        })
    }

    /// Rend visibles les fils dont le report est arrivé à échéance.
    ///
    /// L'état est **restauré**, pas décidé : un fil reporté depuis « en attente »
    /// revient en attente, pas dans la file de travail. Le report met de côté, il ne
    /// requalifie pas.
    pub fn wake_due_snoozes(&self, now: Timestamp) -> Result<usize> {
        let dus = self.store().due_snoozes(now)?;
        let mut reveilles = 0;

        for (thread, restore_to) in dus {
            self.store().clear_snooze(thread)?;
            self.bus().publish(Event::ThreadUnsnoozed { thread });

            let Some(ligne) = self.store().thread_row(thread)? else {
                continue;
            };
            let resultat = transition(
                ligne.state,
                TransitionCause::SnoozeExpired,
                Some(restore_to),
                &self.automation(),
            );

            if let TransitionOutcome::Moved { from, to } = resultat {
                self.store().set_thread_state(thread, to)?;
                self.bus().publish(Event::ThreadStateChanged {
                    thread,
                    from,
                    to,
                    cause: TransitionCause::SnoozeExpired,
                });
            }
            reveilles += 1;
        }

        Ok(reveilles)
    }

    /// Ramène dans la file les fils en attente depuis trop longtemps.
    pub fn run_follow_ups(&self, now: Timestamp) -> Result<usize> {
        let reglages = self.automation();
        if !reglages.follow_up_enabled {
            return Ok(0);
        }

        let candidats =
            self.store()
                .threads_needing_follow_up(now, reglages.follow_up_days, MAX_FOLLOW_UPS)?;

        let mut relances = 0;
        for thread in candidats {
            let Some(ligne) = self.store().thread_row(thread)? else {
                continue;
            };
            let resultat = transition(ligne.state, TransitionCause::FollowUpDue, None, &reglages);

            if let TransitionOutcome::Moved { from, to } = resultat {
                self.store().set_thread_state(thread, to)?;
                self.bus().publish(Event::ThreadStateChanged {
                    thread,
                    from,
                    to,
                    cause: TransitionCause::FollowUpDue,
                });
                relances += 1;
            }
        }

        Ok(relances)
    }

    /// Supprime les contenus dont plus aucun message ne se réclame.
    pub fn purge_bodies(&self) -> Result<usize> {
        let Some(blobs) = self.blobs() else {
            return Ok(0);
        };
        crate::body::purge_orphan_bodies(self.store(), blobs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, StaticCredentials, SyncEngine};
    use iris_imap::fake::FakeServer;
    use iris_imap::Connector;
    use iris_kernel::{EventBus, EventKind};
    use iris_store::{FolderRole, NewAccount, NewMessage, Store};
    use iris_types::{AutomationSettings, Flags, Snooze, ThreadId, WorkflowState};
    use std::sync::Arc;

    struct Fixture {
        engine: SyncEngine,
        store: Arc<Store>,
        bus: EventBus,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        let compte = store
            .create_account(
                &NewAccount::new("a@x.fr", "imap.x.fr", "s"),
                Timestamp::EPOCH,
            )
            .unwrap();
        store
            .upsert_folder(compte, "INBOX", FolderRole::Inbox)
            .unwrap();

        let bus = EventBus::new();
        let engine = SyncEngine::new(
            Arc::clone(&store),
            Arc::new(FakeServer::default()) as Arc<dyn Connector>,
            Arc::new(StaticCredentials::new("p")),
            bus.clone(),
            EngineConfig::default(),
        );

        Fixture {
            engine,
            store,
            bus,
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        fn fil(&self, millis: i64) -> ThreadId {
            let uid = self.uid.get();
            self.uid.set(uid + 1);
            let compte = self.store.accounts().unwrap()[0].id;
            let dossier = self.store.folders(compte).unwrap()[0].id;

            self.store
                .insert_message(&NewMessage {
                    account: compte,
                    folder: dossier,
                    uid,
                    rfc_message_id: Some(format!("m{uid}@x")),
                    in_reply_to: None,
                    references: vec![],
                    subject: format!("Sujet {uid}"),
                    from_name: "Marie".into(),
                    from_addr: "marie@x.fr".into(),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(millis),
                    received: Timestamp::from_millis(millis),
                    size: 10,
                    flags: Flags::NONE,
                    preview: String::new(),
                })
                .unwrap()
                .thread
        }

        fn etat(&self, t: ThreadId) -> WorkflowState {
            self.store.thread_row(t).unwrap().unwrap().state
        }
    }

    fn t(secs: i64) -> Timestamp {
        Timestamp::from_millis(secs * 1000)
    }

    #[test]
    fn un_report_echu_est_reveille() {
        let f = fixture();
        let fil = f.fil(1000);
        f.store
            .snooze_thread(
                fil,
                Snooze {
                    until: t(5000),
                    restore_to: WorkflowState::Todo,
                },
            )
            .unwrap();

        assert_eq!(f.engine.run_maintenance(t(4999)).unwrap().woken, 0);
        assert_eq!(f.engine.run_maintenance(t(5000)).unwrap().woken, 1);
        assert!(f
            .store
            .thread_row(fil)
            .unwrap()
            .unwrap()
            .snoozed_until
            .is_none());
    }

    #[test]
    fn le_report_restaure_l_etat_au_lieu_de_le_decider() {
        // Un fil reporté depuis « en attente » revient en attente : le report met de
        // côté, il ne requalifie pas.
        let f = fixture();
        let fil = f.fil(1000);
        f.store
            .set_thread_state(fil, WorkflowState::Waiting)
            .unwrap();
        f.store
            .snooze_thread(
                fil,
                Snooze {
                    until: t(5000),
                    restore_to: WorkflowState::Waiting,
                },
            )
            .unwrap();

        f.engine.run_maintenance(t(6000)).unwrap();
        assert_eq!(f.etat(fil), WorkflowState::Waiting);
    }

    #[test]
    fn un_reveil_est_annonce_sur_le_bus() {
        let f = fixture();
        let fil = f.fil(1000);
        f.store
            .snooze_thread(
                fil,
                Snooze {
                    until: t(1),
                    restore_to: WorkflowState::Done,
                },
            )
            .unwrap();
        let mut abonne = f.bus.subscribe_kind(EventKind::Workflow);

        f.engine.run_maintenance(t(100)).unwrap();
        let evenements = abonne.drain();

        assert!(evenements
            .iter()
            .any(|e| matches!(e, Event::ThreadUnsnoozed { .. })));
        assert!(evenements.iter().any(|e| matches!(
            e,
            Event::ThreadStateChanged {
                cause: TransitionCause::SnoozeExpired,
                ..
            }
        )));
    }

    #[test]
    fn un_fil_sans_reponse_est_relance() {
        let f = fixture();
        let fil = f.fil(1_000);
        f.store
            .set_thread_state(fil, WorkflowState::Waiting)
            .unwrap();

        // Trois jours plus tard, le délai par défaut est dépassé.
        let plus_tard = t(1 + 4 * 86_400);
        assert_eq!(f.engine.run_maintenance(plus_tard).unwrap().followed_up, 1);
        assert_eq!(f.etat(fil), WorkflowState::Todo);
    }

    #[test]
    fn un_fil_recemment_repondu_n_est_pas_relance() {
        let f = fixture();
        let fil = f.fil(1_000_000_000);
        f.store
            .set_thread_state(fil, WorkflowState::Waiting)
            .unwrap();

        let rapport = f
            .engine
            .run_maintenance(Timestamp::from_millis(1_000_100_000))
            .unwrap();
        assert_eq!(rapport.followed_up, 0);
    }

    #[test]
    fn la_relance_desactivee_ne_fait_rien() {
        let f = fixture();
        f.engine.set_automation(AutomationSettings {
            follow_up_enabled: false,
            ..Default::default()
        });
        let fil = f.fil(1_000);
        f.store
            .set_thread_state(fil, WorkflowState::Waiting)
            .unwrap();

        assert_eq!(f.engine.run_maintenance(t(999_999)).unwrap().followed_up, 0);
        assert_eq!(f.etat(fil), WorkflowState::Waiting);
    }

    #[test]
    fn le_passage_est_idempotent() {
        // Il est appelé à chaque tour de synchronisation : le repasser ne doit rien
        // produire de plus.
        let f = fixture();
        let fil = f.fil(1_000);
        f.store
            .set_thread_state(fil, WorkflowState::Waiting)
            .unwrap();

        let plus_tard = t(1 + 10 * 86_400);
        assert_eq!(f.engine.run_maintenance(plus_tard).unwrap().followed_up, 1);
        assert_eq!(f.engine.run_maintenance(plus_tard).unwrap().followed_up, 0);
    }

    #[test]
    fn les_relances_sont_etalees() {
        // Après une longue absence, une avalanche de relances rendrait la file
        // illisible.
        let f = fixture();
        for i in 0..80 {
            let fil = f.fil(1_000 + i);
            f.store
                .set_thread_state(fil, WorkflowState::Waiting)
                .unwrap();
        }

        let plus_tard = t(1 + 30 * 86_400);
        let premier = f.engine.run_maintenance(plus_tard).unwrap();
        assert_eq!(premier.followed_up, MAX_FOLLOW_UPS as usize);

        let second = f.engine.run_maintenance(plus_tard).unwrap();
        assert_eq!(second.followed_up, 30, "le reste vient au passage suivant");
    }

    #[test]
    fn sans_rien_a_faire_le_passage_est_muet() {
        let f = fixture();
        f.fil(1000);
        let rapport = f.engine.run_maintenance(t(2000)).unwrap();
        assert!(!rapport.changed());
    }

    #[test]
    fn la_purge_sans_magasin_ne_fait_rien() {
        let f = fixture();
        assert_eq!(f.engine.purge_bodies().unwrap(), 0);
    }
}
