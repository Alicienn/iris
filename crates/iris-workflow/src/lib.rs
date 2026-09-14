//! `iris-workflow` — la machine à états, branchée sur le store et le bus.
//!
//! La logique de transition elle-même est pure et vit dans `iris-types`. Cette crate
//! lui ajoute les trois choses qui demandent un état :
//!
//! - **la persistance** : l'état est écrit avant toute publication, pour qu'un
//!   abonné ne puisse jamais lire une base qui contredit l'événement qu'il reçoit ;
//! - **l'annulation** : toute transition est empilée et rejouable à l'envers. C'est
//!   ce qui rend le triage au clavier utilisable, parce qu'une erreur ne coûte rien ;
//! - **le temps** : réveil des reports échus et relance des fils sans réponse.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use iris_kernel::{Event, EventBus};
use iris_store::Store;
use iris_types::{
    transition, AutomationSettings, Error, Result, Snooze, ThreadId, Timestamp, TransitionCause,
    TransitionOutcome, WorkflowState,
};
use std::sync::{Arc, Mutex, RwLock};

/// Profondeur de la pile d'annulation.
///
/// Assez pour rattraper une séance de triage entière, assez peu pour que la pile ne
/// devienne pas un journal parallèle.
const UNDO_DEPTH: usize = 100;

/// Une transition annulable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UndoEntry {
    pub thread: ThreadId,
    pub from: WorkflowState,
    pub to: WorkflowState,
    pub cause: TransitionCause,
    pub at: Timestamp,
}

/// Le moteur de workflow.
#[derive(Debug)]
pub struct Workflow {
    store: Arc<Store>,
    bus: EventBus,
    settings: RwLock<AutomationSettings>,
    undo: Mutex<Vec<UndoEntry>>,
}

impl Workflow {
    pub fn new(store: Arc<Store>, bus: EventBus, settings: AutomationSettings) -> Self {
        Self {
            store,
            bus,
            settings: RwLock::new(settings),
            undo: Mutex::new(Vec::new()),
        }
    }

    pub fn settings(&self) -> AutomationSettings {
        *self.settings.read().expect("réglages empoisonnés")
    }

    pub fn set_settings(&self, s: AutomationSettings) {
        *self.settings.write().expect("réglages empoisonnés") = s;
    }

    /// Applique une cause à un fil.
    ///
    /// L'écriture précède la publication : un abonné qui relit la base en recevant
    /// l'événement doit y trouver le nouvel état, jamais l'ancien.
    pub fn apply(
        &self,
        thread: ThreadId,
        cause: TransitionCause,
        target: Option<WorkflowState>,
        now: Timestamp,
    ) -> Result<TransitionOutcome> {
        let Some(row) = self.store.thread_row(thread)? else {
            return Err(Error::store(format!("fil {thread} introuvable")));
        };

        let outcome = transition(row.state, cause, target, &self.settings());

        if let TransitionOutcome::Moved { from, to } = outcome {
            self.store.set_thread_state(thread, to)?;
            self.push_undo(UndoEntry {
                thread,
                from,
                to,
                cause,
                at: now,
            });
            self.bus.publish(Event::ThreadStateChanged {
                thread,
                from,
                to,
                cause,
            });
        }

        Ok(outcome)
    }

    /// Raccourci pour l'action manuelle, la plus fréquente.
    pub fn set_state(
        &self,
        thread: ThreadId,
        state: WorkflowState,
        now: Timestamp,
    ) -> Result<TransitionOutcome> {
        self.apply(thread, TransitionCause::Manual, Some(state), now)
    }

    /// Annule la dernière transition.
    ///
    /// L'annulation n'est **pas** empilée : sans cette règle, annuler puis annuler à
    /// nouveau rejouerait l'action au lieu de remonter dans l'historique.
    pub fn undo(&self) -> Result<Option<UndoEntry>> {
        let Some(entry) = self.pop_undo() else {
            return Ok(None);
        };

        // Le fil a pu disparaître entre-temps ; l'annulation est alors sans objet.
        if self.store.thread_row(entry.thread)?.is_none() {
            return Ok(None);
        }

        self.store.set_thread_state(entry.thread, entry.from)?;
        self.bus.publish(Event::ThreadStateChanged {
            thread: entry.thread,
            from: entry.to,
            to: entry.from,
            cause: TransitionCause::Manual,
        });
        Ok(Some(entry))
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.lock().map(|u| u.len()).unwrap_or(0)
    }

    fn push_undo(&self, entry: UndoEntry) {
        if let Ok(mut u) = self.undo.lock() {
            if u.len() == UNDO_DEPTH {
                u.remove(0);
            }
            u.push(entry);
        }
    }

    fn pop_undo(&self) -> Option<UndoEntry> {
        self.undo.lock().ok()?.pop()
    }

    /// Reporte un fil : il quitte la vue sans changer d'état.
    pub fn snooze(&self, thread: ThreadId, until: Timestamp) -> Result<bool> {
        let Some(row) = self.store.thread_row(thread)? else {
            return Ok(false);
        };
        let snooze = Snooze {
            until,
            restore_to: row.state,
        };
        let ok = self.store.snooze_thread(thread, snooze)?;
        if ok {
            self.bus.publish(Event::ThreadSnoozed { thread, until });
        }
        Ok(ok)
    }

    pub fn unsnooze(&self, thread: ThreadId) -> Result<bool> {
        let ok = self.store.clear_snooze(thread)?;
        if ok {
            self.bus.publish(Event::ThreadUnsnoozed { thread });
        }
        Ok(ok)
    }

    /// Réveille les fils dont le report est échu, en restaurant leur état.
    ///
    /// Retourne le nombre de fils réveillés.
    pub fn wake_due_snoozes(&self, now: Timestamp) -> Result<usize> {
        let dus = self.store.due_snoozes(now)?;
        let mut reveilles = 0;

        for (thread, restore_to) in dus {
            self.store.clear_snooze(thread)?;
            self.bus.publish(Event::ThreadUnsnoozed { thread });

            // Le report restaure l'état, il ne le décide pas : si le fil est déjà
            // dans le bon état, il n'y a rien d'autre à faire que le rendre visible.
            let outcome = self.apply(
                thread,
                TransitionCause::SnoozeExpired,
                Some(restore_to),
                now,
            )?;
            let _ = outcome;
            reveilles += 1;
        }

        Ok(reveilles)
    }

    /// Ramène dans la file les fils en attente depuis trop longtemps.
    ///
    /// Retourne le nombre de fils relancés.
    pub fn run_follow_ups(&self, now: Timestamp, limit: u32) -> Result<usize> {
        let settings = self.settings();
        if !settings.follow_up_enabled {
            return Ok(0);
        }

        let candidats =
            self.store
                .threads_needing_follow_up(now, settings.follow_up_days, limit)?;

        let mut relances = 0;
        for thread in candidats {
            if self
                .apply(thread, TransitionCause::FollowUpDue, None, now)?
                .changed()
            {
                relances += 1;
            }
        }
        Ok(relances)
    }

    /// À appeler lorsqu'une réponse vient d'être envoyée dans un fil.
    pub fn on_reply_sent(&self, thread: ThreadId, now: Timestamp) -> Result<TransitionOutcome> {
        self.apply(thread, TransitionCause::ReplySent, None, now)
    }

    /// À appeler lorsqu'un nouveau message rejoint un fil existant.
    pub fn on_message_received(
        &self,
        thread: ThreadId,
        now: Timestamp,
    ) -> Result<TransitionOutcome> {
        self.apply(thread, TransitionCause::MessageReceived, None, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_kernel::EventKind;
    use iris_store::{FolderRole, NewAccount, NewMessage};
    use iris_types::{AccountId, Flags, FolderId};

    struct Fixture {
        workflow: Workflow,
        store: Arc<Store>,
        bus: EventBus,
        account: AccountId,
        folder: FolderId,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        let account = store
            .create_account(
                &NewAccount::new("a@x.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();
        let folder = store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        let bus = EventBus::new();
        let workflow = Workflow::new(
            Arc::clone(&store),
            bus.clone(),
            AutomationSettings::default(),
        );
        Fixture {
            workflow,
            store,
            bus,
            account,
            folder,
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        fn thread_at(&self, millis: i64) -> ThreadId {
            let uid = self.uid.get();
            self.uid.set(uid + 1);
            self.store
                .insert_message(&NewMessage {
                    account: self.account,
                    folder: self.folder,
                    uid,
                    rfc_message_id: Some(format!("m{uid}@x")),
                    in_reply_to: None,
                    references: vec![],
                    subject: format!("Sujet {uid}"),
                    from_name: "Marie".into(),
                    from_addr: "marie@example.com".into(),
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

        fn state(&self, t: ThreadId) -> WorkflowState {
            self.store.thread_row(t).unwrap().unwrap().state
        }
    }

    fn t(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn une_action_manuelle_change_l_etat_et_publie() {
        let f = fixture();
        let mut abonne = f.bus.subscribe_kind(EventKind::Workflow);
        let fil = f.thread_at(1000);

        let out = f
            .workflow
            .set_state(fil, WorkflowState::Done, t(2000))
            .unwrap();
        assert_eq!(
            out,
            TransitionOutcome::Moved {
                from: WorkflowState::Todo,
                to: WorkflowState::Done
            }
        );
        assert_eq!(f.state(fil), WorkflowState::Done);

        let evenements = abonne.drain();
        assert_eq!(evenements.len(), 1);
    }

    #[test]
    fn l_etat_est_ecrit_avant_d_etre_publie() {
        // Un abonné qui relit la base en recevant l'événement doit y trouver le
        // nouvel état.
        let f = fixture();
        let fil = f.thread_at(1000);
        let mut abonne = f.bus.subscribe();

        f.workflow
            .set_state(fil, WorkflowState::Done, t(2000))
            .unwrap();

        assert!(!abonne.drain().is_empty());
        assert_eq!(f.state(fil), WorkflowState::Done);
    }

    #[test]
    fn une_transition_sans_effet_ne_publie_rien() {
        let f = fixture();
        let fil = f.thread_at(1000);
        let mut abonne = f.bus.subscribe();

        let out = f
            .workflow
            .set_state(fil, WorkflowState::Todo, t(2000))
            .unwrap();
        assert_eq!(out, TransitionOutcome::Unchanged);
        assert!(abonne.drain().is_empty());
        assert_eq!(f.workflow.undo_depth(), 0);
    }

    #[test]
    fn repondre_met_le_fil_en_attente() {
        let f = fixture();
        let fil = f.thread_at(1000);
        f.workflow.on_reply_sent(fil, t(2000)).unwrap();
        assert_eq!(f.state(fil), WorkflowState::Waiting);
    }

    #[test]
    fn l_automatisme_desactive_ne_change_rien() {
        let f = fixture();
        f.workflow.set_settings(AutomationSettings {
            reply_marks_waiting: false,
            ..Default::default()
        });
        let fil = f.thread_at(1000);

        let out = f.workflow.on_reply_sent(fil, t(2000)).unwrap();
        assert_eq!(out, TransitionOutcome::Disabled);
        assert_eq!(f.state(fil), WorkflowState::Todo);
    }

    #[test]
    fn l_annulation_restaure_l_etat_precedent() {
        let f = fixture();
        let fil = f.thread_at(1000);
        f.workflow
            .set_state(fil, WorkflowState::Done, t(2000))
            .unwrap();

        let annule = f.workflow.undo().unwrap().unwrap();
        assert_eq!(annule.thread, fil);
        assert_eq!(f.state(fil), WorkflowState::Todo);
    }

    #[test]
    fn annuler_deux_fois_remonte_dans_l_historique() {
        // L'annulation ne s'empile pas elle-même : sinon la seconde annulation
        // rejouerait la première action au lieu de remonter.
        let f = fixture();
        let fil = f.thread_at(1000);
        f.workflow
            .set_state(fil, WorkflowState::Waiting, t(1))
            .unwrap();
        f.workflow
            .set_state(fil, WorkflowState::Done, t(2))
            .unwrap();

        f.workflow.undo().unwrap();
        assert_eq!(f.state(fil), WorkflowState::Waiting);
        f.workflow.undo().unwrap();
        assert_eq!(f.state(fil), WorkflowState::Todo);
        assert!(f.workflow.undo().unwrap().is_none());
    }

    #[test]
    fn la_pile_d_annulation_est_bornee() {
        let f = fixture();
        let fil = f.thread_at(1000);
        for i in 0..(UNDO_DEPTH + 50) {
            let cible = if i % 2 == 0 {
                WorkflowState::Done
            } else {
                WorkflowState::Todo
            };
            f.workflow.set_state(fil, cible, t(i as i64)).unwrap();
        }
        assert_eq!(f.workflow.undo_depth(), UNDO_DEPTH);
    }

    #[test]
    fn annuler_sur_un_fil_disparu_ne_fait_rien() {
        let f = fixture();
        let fil = f.thread_at(1000);
        f.workflow
            .set_state(fil, WorkflowState::Done, t(2000))
            .unwrap();
        f.store.delete_messages_by_uid(f.folder, &[1]).unwrap();

        assert!(f.workflow.undo().unwrap().is_none());
    }

    #[test]
    fn agir_sur_un_fil_inexistant_est_une_erreur_explicite() {
        let f = fixture();
        let e = f
            .workflow
            .set_state(ThreadId(999), WorkflowState::Done, t(0))
            .unwrap_err();
        assert!(e.to_string().contains("introuvable"));
    }

    #[test]
    fn le_report_conserve_l_etat_puis_le_restaure() {
        let f = fixture();
        let fil = f.thread_at(1000);
        f.workflow
            .set_state(fil, WorkflowState::Waiting, t(1))
            .unwrap();

        assert!(f.workflow.snooze(fil, t(5000)).unwrap());
        assert_eq!(
            f.state(fil),
            WorkflowState::Waiting,
            "le report ne change pas l'état"
        );

        assert_eq!(f.workflow.wake_due_snoozes(t(4999)).unwrap(), 0);
        assert_eq!(f.workflow.wake_due_snoozes(t(5000)).unwrap(), 1);
        assert_eq!(f.state(fil), WorkflowState::Waiting);
        assert!(f
            .store
            .thread_row(fil)
            .unwrap()
            .unwrap()
            .snoozed_until
            .is_none());
    }

    #[test]
    fn annuler_un_report_le_retire_de_la_file_des_echeances() {
        let f = fixture();
        let fil = f.thread_at(1000);
        f.workflow.snooze(fil, t(5000)).unwrap();
        assert!(f.workflow.unsnooze(fil).unwrap());
        assert_eq!(f.workflow.wake_due_snoozes(t(9999)).unwrap(), 0);
    }

    #[test]
    fn la_relance_ramene_les_fils_en_attente_trop_anciens() {
        let f = fixture();
        let vieux = f.thread_at(1_000);
        let recent = f.thread_at(500_000_000);
        for fil in [vieux, recent] {
            f.workflow
                .set_state(fil, WorkflowState::Waiting, t(0))
                .unwrap();
        }

        let now = t(500_000_000);
        assert_eq!(f.workflow.run_follow_ups(now, 10).unwrap(), 1);
        assert_eq!(f.state(vieux), WorkflowState::Todo);
        assert_eq!(f.state(recent), WorkflowState::Waiting);
    }

    #[test]
    fn la_relance_desactivee_ne_fait_rien() {
        let f = fixture();
        f.workflow.set_settings(AutomationSettings {
            follow_up_enabled: false,
            ..Default::default()
        });
        let fil = f.thread_at(1_000);
        f.workflow
            .set_state(fil, WorkflowState::Waiting, t(0))
            .unwrap();

        assert_eq!(f.workflow.run_follow_ups(t(500_000_000), 10).unwrap(), 0);
        assert_eq!(f.state(fil), WorkflowState::Waiting);
    }

    #[test]
    fn un_nouveau_message_rouvre_un_fil_termine() {
        let f = fixture();
        let fil = f.thread_at(1000);
        f.workflow
            .set_state(fil, WorkflowState::Done, t(1))
            .unwrap();

        f.workflow.on_message_received(fil, t(2)).unwrap();
        assert_eq!(f.state(fil), WorkflowState::Todo);
    }

    #[test]
    fn repondre_a_un_fil_termine_ne_le_rouvre_pas() {
        let f = fixture();
        let fil = f.thread_at(1000);
        f.workflow
            .set_state(fil, WorkflowState::Done, t(1))
            .unwrap();

        let out = f.workflow.on_reply_sent(fil, t(2)).unwrap();
        assert_eq!(out, TransitionOutcome::Unchanged);
        assert_eq!(f.state(fil), WorkflowState::Done);
    }
}
