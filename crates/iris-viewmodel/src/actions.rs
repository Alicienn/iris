//! Les actions de l'utilisateur.
//!
//! Invariant n° 3 en pratique : **rien n'attend le réseau**. Chaque action écrit
//! localement, journalise ce qu'il faudra transmettre, et rend la main. L'interface
//! n'affiche jamais de sablier pour un geste de l'utilisateur ; au pire, une action
//! reste un moment en attente de réconciliation, ce qui ne se voit pas.
//!
//! Le corollaire est que **tout doit être annulable**, puisque tout est appliqué
//! avant confirmation.

use iris_store::{OpKind, Store};
use iris_types::{
    transition, AutomationSettings, Error, Result, Snooze, ThreadId, Timestamp, TransitionCause,
    TransitionOutcome, WorkflowState,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Une action déclenchée par l'utilisateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Marquer traité.
    Done,
    /// Remettre dans la file.
    Todo,
    /// Mettre en attente.
    Waiting,
    /// Reporter de N heures.
    SnoozeHours(u32),
    /// Annuler un report.
    Unsnooze,
    MarkRead,
    MarkUnread,
    ToggleFlag,
}

impl Action {
    /// Touche associée, telle qu'affichée dans l'aide et la palette.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Self::Done => Some("e"),
            Self::Todo => Some("u"),
            Self::Waiting => Some("w"),
            Self::SnoozeHours(_) => Some("s"),
            Self::MarkRead => Some("r"),
            Self::MarkUnread => Some("Maj+r"),
            Self::ToggleFlag => Some("f"),
            Self::Unsnooze => None,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Done => "Marquer traité".into(),
            Self::Todo => "Remettre à traiter".into(),
            Self::Waiting => "Mettre en attente".into(),
            Self::SnoozeHours(h) if h % 24 == 0 => format!("Reporter de {} jours", h / 24),
            Self::SnoozeHours(h) => format!("Reporter de {h} heures"),
            Self::Unsnooze => "Annuler le report".into(),
            Self::MarkRead => "Marquer comme lu".into(),
            Self::MarkUnread => "Marquer comme non lu".into(),
            Self::ToggleFlag => "Épingler".into(),
        }
    }
}

/// Ce qu'une action a produit, et comment la défaire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionOutcome {
    pub thread: ThreadId,
    pub action: Action,
    /// De quoi revenir en arrière.
    pub undo: UndoRecord,
    /// L'action a effectivement changé quelque chose.
    pub changed: bool,
}

/// L'état à restaurer pour annuler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoRecord {
    pub thread: ThreadId,
    pub state: WorkflowState,
    pub snooze_until: Option<Timestamp>,
    pub snooze_restore: Option<WorkflowState>,
}

/// Applique les actions et tient la pile d'annulation.
#[derive(Debug)]
pub struct Actions {
    store: Arc<Store>,
    settings: AutomationSettings,
    undo: Vec<UndoRecord>,
    max_undo: usize,
}

impl Actions {
    pub fn new(store: Arc<Store>, settings: AutomationSettings) -> Self {
        Self {
            store,
            settings,
            undo: Vec::new(),
            max_undo: 100,
        }
    }

    pub fn settings(&self) -> AutomationSettings {
        self.settings
    }

    pub fn set_settings(&mut self, settings: AutomationSettings) {
        self.settings = settings;
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Applique une action à un fil.
    pub fn apply(
        &mut self,
        thread: ThreadId,
        action: Action,
        now: Timestamp,
    ) -> Result<ActionOutcome> {
        let ligne = self
            .store
            .thread_row(thread)?
            .ok_or_else(|| Error::store(format!("fil {thread} introuvable")))?;

        let avant = UndoRecord {
            thread,
            state: ligne.state,
            snooze_until: ligne.snoozed_until,
            snooze_restore: None,
        };

        let change = match action {
            Action::Done => self.set_state(thread, ligne.state, WorkflowState::Done)?,
            Action::Todo => self.set_state(thread, ligne.state, WorkflowState::Todo)?,
            Action::Waiting => self.set_state(thread, ligne.state, WorkflowState::Waiting)?,
            Action::SnoozeHours(h) => {
                let echeance = Timestamp::from_millis(now.millis() + h as i64 * 3_600_000);
                self.store.snooze_thread(
                    thread,
                    Snooze {
                        until: echeance,
                        restore_to: ligne.state,
                    },
                )?
            }
            Action::Unsnooze => self.store.clear_snooze(thread)?,
            Action::MarkRead => self.set_read(thread, true, now)?,
            Action::MarkUnread => self.set_read(thread, false, now)?,
            Action::ToggleFlag => {
                let epingle = ligne.flags_union.contains(iris_types::Flags::FLAGGED);
                self.set_flagged(thread, !epingle, now)?
            }
        };

        if change {
            self.push_undo(avant.clone());
        }

        Ok(ActionOutcome {
            thread,
            action,
            undo: avant,
            changed: change,
        })
    }

    fn set_state(&self, thread: ThreadId, from: WorkflowState, to: WorkflowState) -> Result<bool> {
        // Même une action manuelle passe par la machine à états : c'est elle qui
        // définit ce qui est légal, et la contourner créerait une seconde vérité.
        let resultat = transition(from, TransitionCause::Manual, Some(to), &self.settings);
        match resultat {
            TransitionOutcome::Moved { .. } => {
                self.store.set_thread_state(thread, to)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Marque tous les messages d'un fil comme lus ou non lus.
    fn set_read(&self, thread: ThreadId, read: bool, now: Timestamp) -> Result<bool> {
        let messages = self.store.thread_messages(thread)?;
        let mut change = false;
        let mut par_dossier: std::collections::BTreeMap<
            (iris_types::AccountId, iris_types::FolderId),
            Vec<u32>,
        > = Default::default();

        for m in &messages {
            let deja = m.flags.contains(iris_types::Flags::SEEN);
            if deja == read {
                continue;
            }
            let nouveaux = m.flags.set(iris_types::Flags::SEEN, read);
            self.store.set_message_flags(m.id, nouveaux)?;
            par_dossier
                .entry((m.account, m.folder))
                .or_default()
                .push(m.uid);
            change = true;
        }

        // Une seule opération journalisée par dossier : cinquante messages lus d'un
        // coup ne doivent pas produire cinquante commandes IMAP.
        for ((account, folder), uids) in par_dossier {
            self.journal_flags(account, folder, &uids, iris_types::Flags::SEEN, read, now)?;
        }

        Ok(change)
    }

    fn set_flagged(&self, thread: ThreadId, flagged: bool, now: Timestamp) -> Result<bool> {
        let messages = self.store.thread_messages(thread)?;
        let Some(dernier) = messages.last() else {
            return Ok(false);
        };

        let nouveaux = dernier.flags.set(iris_types::Flags::FLAGGED, flagged);
        if nouveaux == dernier.flags {
            return Ok(false);
        }
        self.store.set_message_flags(dernier.id, nouveaux)?;
        self.journal_flags(
            dernier.account,
            dernier.folder,
            &[dernier.uid],
            iris_types::Flags::FLAGGED,
            flagged,
            now,
        )?;
        Ok(true)
    }

    /// Inscrit au journal ce qu'il faudra transmettre au serveur.
    fn journal_flags(
        &self,
        account: iris_types::AccountId,
        folder: iris_types::FolderId,
        uids: &[u32],
        flags: iris_types::Flags,
        add: bool,
        now: Timestamp,
    ) -> Result<()> {
        let chemin = self
            .store
            .folders(account)?
            .into_iter()
            .find(|f| f.id == folder)
            .map(|f| f.path)
            .unwrap_or_default();

        let mut tries = uids.to_vec();
        tries.sort_unstable();
        let charge = serde_json::json!({
            "op": "set_flags",
            "folder": chemin,
            "uids": tries,
            "flags": flags.0,
            "add": add,
        });
        let clef = format!(
            "{account}:flags:{chemin}:{}:{}:{add}",
            tries
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            flags.0
        );

        self.store
            .enqueue_op(account, OpKind::SetFlags, &charge.to_string(), &clef, now)?;
        Ok(())
    }

    fn push_undo(&mut self, record: UndoRecord) {
        if self.undo.len() == self.max_undo {
            self.undo.remove(0);
        }
        self.undo.push(record);
    }

    /// Défait la dernière action.
    pub fn undo(&mut self) -> Result<Option<UndoRecord>> {
        let Some(record) = self.undo.pop() else {
            return Ok(None);
        };

        if self.store.thread_row(record.thread)?.is_none() {
            return Ok(None);
        }

        self.store.set_thread_state(record.thread, record.state)?;
        match record.snooze_until {
            Some(echeance) => {
                self.store.snooze_thread(
                    record.thread,
                    Snooze {
                        until: echeance,
                        restore_to: record.state,
                    },
                )?;
            }
            None => {
                self.store.clear_snooze(record.thread)?;
            }
        }
        Ok(Some(record))
    }

    /// Applique une action à plusieurs fils, pour le triage en lot.
    pub fn apply_many(
        &mut self,
        threads: &[ThreadId],
        action: Action,
        now: Timestamp,
    ) -> Result<usize> {
        let mut changes = 0;
        for t in threads {
            // Un fil disparu ne doit pas interrompre le traitement des autres.
            match self.apply(*t, action, now) {
                Ok(o) if o.changed => changes += 1,
                Ok(_) => {}
                Err(e) => tracing::warn!(fil = %t, erreur = %e, "action ignorée"),
            }
        }
        Ok(changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_store::{FolderRole, NewAccount, NewMessage};
    use iris_types::{AccountId, Flags, FolderId};

    struct Fixture {
        store: Arc<Store>,
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
        Fixture {
            store,
            account,
            folder,
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        fn thread(&self) -> ThreadId {
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
                    subject: "Sujet".into(),
                    from_name: "Marie".into(),
                    from_addr: "marie@x.fr".into(),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(1000 * uid as i64),
                    received: Timestamp::from_millis(1000 * uid as i64),
                    size: 10,
                    flags: Flags::NONE,
                    preview: String::new(),
                })
                .unwrap()
                .thread
        }

        fn actions(&self) -> Actions {
            Actions::new(Arc::clone(&self.store), AutomationSettings::default())
        }

        fn state(&self, t: ThreadId) -> WorkflowState {
            self.store.thread_row(t).unwrap().unwrap().state
        }
    }

    fn t(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn marquer_traite_change_l_etat_sans_attendre_le_reseau() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();

        let o = a.apply(fil, Action::Done, t(0)).unwrap();
        assert!(o.changed);
        assert_eq!(f.state(fil), WorkflowState::Done);
    }

    #[test]
    fn une_action_sans_effet_n_empile_rien() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();

        let o = a.apply(fil, Action::Todo, t(0)).unwrap();
        assert!(!o.changed, "le fil est déjà à traiter");
        assert_eq!(a.undo_depth(), 0);
    }

    #[test]
    fn l_annulation_restaure_l_etat() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();

        a.apply(fil, Action::Done, t(0)).unwrap();
        let restaure = a.undo().unwrap().unwrap();

        assert_eq!(restaure.state, WorkflowState::Todo);
        assert_eq!(f.state(fil), WorkflowState::Todo);
        assert_eq!(a.undo_depth(), 0);
    }

    #[test]
    fn l_annulation_remonte_l_historique() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();

        a.apply(fil, Action::Waiting, t(0)).unwrap();
        a.apply(fil, Action::Done, t(1)).unwrap();

        a.undo().unwrap();
        assert_eq!(f.state(fil), WorkflowState::Waiting);
        a.undo().unwrap();
        assert_eq!(f.state(fil), WorkflowState::Todo);
        assert!(a.undo().unwrap().is_none());
    }

    #[test]
    fn le_report_masque_puis_restaure_l_etat() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();
        a.apply(fil, Action::Waiting, t(0)).unwrap();

        a.apply(fil, Action::SnoozeHours(24), t(0)).unwrap();
        let ligne = f.store.thread_row(fil).unwrap().unwrap();
        assert_eq!(ligne.snoozed_until, Some(t(86_400_000)));
        assert_eq!(
            ligne.state,
            WorkflowState::Waiting,
            "le report ne change pas l'état"
        );

        a.apply(fil, Action::Unsnooze, t(0)).unwrap();
        assert!(f
            .store
            .thread_row(fil)
            .unwrap()
            .unwrap()
            .snoozed_until
            .is_none());
    }

    #[test]
    fn l_annulation_retablit_le_report() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();

        a.apply(fil, Action::SnoozeHours(3), t(0)).unwrap();
        a.apply(fil, Action::Unsnooze, t(0)).unwrap();
        a.undo().unwrap();

        assert_eq!(
            f.store.thread_row(fil).unwrap().unwrap().snoozed_until,
            Some(t(10_800_000))
        );
    }

    #[test]
    fn marquer_lu_journalise_une_seule_operation_par_dossier() {
        // Cinquante messages lus d'un coup ne doivent pas produire cinquante
        // commandes.
        let f = fixture();
        let racine = f.thread();
        for i in 2..=6 {
            f.store
                .insert_message(&NewMessage {
                    account: f.account,
                    folder: f.folder,
                    uid: 100 + i,
                    rfc_message_id: Some(format!("r{i}@x")),
                    in_reply_to: Some("m1@x".into()),
                    references: vec!["m1@x".into()],
                    subject: "Re: Sujet".into(),
                    from_name: "Marie".into(),
                    from_addr: "marie@x.fr".into(),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(2000 + i as i64),
                    received: Timestamp::from_millis(2000 + i as i64),
                    size: 10,
                    flags: Flags::NONE,
                    preview: String::new(),
                })
                .unwrap();
        }

        let mut a = f.actions();
        assert!(a.apply(racine, Action::MarkRead, t(0)).unwrap().changed);

        assert_eq!(f.store.thread_row(racine).unwrap().unwrap().unread_count, 0);
        assert_eq!(f.store.pending_op_count().unwrap(), 1);
    }

    #[test]
    fn marquer_lu_deux_fois_ne_change_rien_la_seconde_fois() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();

        assert!(a.apply(fil, Action::MarkRead, t(0)).unwrap().changed);
        assert!(!a.apply(fil, Action::MarkRead, t(1)).unwrap().changed);
    }

    #[test]
    fn l_epinglage_bascule() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();

        a.apply(fil, Action::ToggleFlag, t(0)).unwrap();
        assert!(f
            .store
            .thread_row(fil)
            .unwrap()
            .unwrap()
            .flags_union
            .contains(Flags::FLAGGED));

        a.apply(fil, Action::ToggleFlag, t(1)).unwrap();
        assert!(!f
            .store
            .thread_row(fil)
            .unwrap()
            .unwrap()
            .flags_union
            .contains(Flags::FLAGGED));
    }

    #[test]
    fn agir_sur_un_fil_inexistant_est_une_erreur_explicite() {
        let f = fixture();
        let mut a = f.actions();
        assert!(a.apply(ThreadId(999), Action::Done, t(0)).is_err());
    }

    #[test]
    fn annuler_sur_un_fil_disparu_ne_fait_rien() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();
        a.apply(fil, Action::Done, t(0)).unwrap();

        f.store.delete_messages_by_uid(f.folder, &[1]).unwrap();
        assert!(a.undo().unwrap().is_none());
    }

    #[test]
    fn le_traitement_en_lot_ignore_les_fils_disparus() {
        let f = fixture();
        let fils: Vec<_> = (0..3).map(|_| f.thread()).collect();
        let mut a = f.actions();

        let mut cibles = fils.clone();
        cibles.push(ThreadId(999));

        assert_eq!(a.apply_many(&cibles, Action::Done, t(0)).unwrap(), 3);
        for fil in fils {
            assert_eq!(f.state(fil), WorkflowState::Done);
        }
    }

    #[test]
    fn la_pile_d_annulation_est_bornee() {
        let f = fixture();
        let fil = f.thread();
        let mut a = f.actions();

        for i in 0..150 {
            let cible = if i % 2 == 0 {
                Action::Done
            } else {
                Action::Todo
            };
            a.apply(fil, cible, t(i)).unwrap();
        }
        assert_eq!(a.undo_depth(), 100);
    }

    #[test]
    fn les_libelles_sont_lisibles() {
        assert_eq!(Action::SnoozeHours(24).label(), "Reporter de 1 jours");
        assert_eq!(Action::SnoozeHours(3).label(), "Reporter de 3 heures");
        assert_eq!(Action::Done.label(), "Marquer traité");
        assert_eq!(Action::Done.key(), Some("e"));
    }
}
