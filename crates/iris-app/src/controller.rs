//! Le contrôleur.
//!
//! Il fait respecter l'invariant n° 1 par sa forme même : **le vue-modèle vit dans
//! son propre fil**, et l'interface ne lui parle que par messages. Il devient
//! matériellement impossible d'exécuter une requête SQL pendant une frame — non
//! parce qu'on s'en garde, mais parce que le code qui la lance n'est pas sur ce fil.
//!
//! Les réponses repartent vers l'interface par la boucle d'événements de Slint, qui
//! est le seul point où les deux mondes se touchent.

use iris_kernel::{coalesce, EventBus, ViewDiff};
use iris_store::Store;
use iris_types::{AutomationSettings, Result, ThreadId, Timestamp, WorkflowState};
use iris_viewmodel::{Action, Actions, Movement, ViewModel};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

/// Ce que l'interface demande au vue-modèle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Charge l'état initial.
    Bootstrap,
    SelectThread(ThreadId),
    Move(Movement),
    SwitchTab(WorkflowState),
    FilterAccounts(Vec<iris_types::AccountId>),
    /// L'utilisateur a fait défiler jusqu'à cet indice.
    EnsureLoaded(usize),
    Apply(Action),
    Undo,
    /// Un lot de changements est arrivé du noyau.
    Diff(Box<ViewDiff>),
    /// Change l'instant de référence, pour les dates relatives et les reports.
    Tick(Timestamp),
    Shutdown,
}

/// Ce que le vue-modèle renvoie à l'interface.
///
/// Un instantané complet plutôt qu'un diff : à l'échelle d'une fenêtre visible —
/// quelques dizaines de lignes — le recopier coûte moins cher que de raisonner sur
/// ce qui a changé, et supprime toute une catégorie de bogues d'affichage.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub rows: Vec<iris_store::ThreadRow>,
    pub selected: Option<ThreadId>,
    pub active_tab: WorkflowState,
    pub counts: [u32; 3],
    /// Lignes chargées, et total connu. La barre de défilement les distingue :
    /// avec une pagination par curseur, on ne peut pas sauter à la millionième
    /// ligne, et prétendre le contraire produirait une barre qui ment.
    pub loaded: usize,
    pub total: u32,
    /// Message affiché dans la colonne de lecture, s'il y en a un.
    pub messages: Vec<iris_store::StoredMessage>,
    pub pending_ops: u64,
}

impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Le fil du vue-modèle.
#[derive(Debug)]
pub struct Controller {
    requests: Sender<Request>,
}

impl Controller {
    /// Démarre le fil et rend de quoi lui parler.
    ///
    /// `on_snapshot` est appelé depuis le fil du vue-modèle : c'est à l'appelant de
    /// le renvoyer vers la boucle d'interface.
    pub fn spawn(
        store: Arc<Store>,
        settings: AutomationSettings,
        now: Timestamp,
        on_snapshot: impl Fn(Snapshot) + Send + 'static,
    ) -> (Self, std::thread::JoinHandle<()>) {
        let (tx, rx) = std::sync::mpsc::channel();

        let fil = std::thread::Builder::new()
            .name("iris-viewmodel".into())
            .spawn(move || {
                run(store, settings, now, rx, on_snapshot);
            })
            .expect("création du fil du vue-modèle");

        (Self { requests: tx }, fil)
    }

    /// Envoie une requête. Ne bloque jamais l'appelant.
    pub fn send(&self, request: Request) {
        // Une erreur signifie que le fil s'est arrêté : pendant la fermeture, c'est
        // normal, et il n'y a rien à en faire.
        let _ = self.requests.send(request);
    }

    pub fn shutdown(&self) {
        self.send(Request::Shutdown);
    }
}

fn run(
    store: Arc<Store>,
    settings: AutomationSettings,
    now: Timestamp,
    requests: Receiver<Request>,
    on_snapshot: impl Fn(Snapshot),
) {
    let mut vm = ViewModel::new(Arc::clone(&store), now);
    let mut actions = Actions::new(Arc::clone(&store), settings);

    while let Ok(request) = requests.recv() {
        if request == Request::Shutdown {
            break;
        }

        match handle(&mut vm, &mut actions, request) {
            Ok(true) => on_snapshot(snapshot(&vm, &store)),
            Ok(false) => {}
            // Une erreur du vue-modèle ne doit pas emporter le fil : l'interface
            // resterait figée sans explication.
            Err(e) => tracing::error!(erreur = %e, "vue-modèle"),
        }
    }
}

/// Traite une requête. Retourne `true` si un nouvel instantané doit être émis.
fn handle(vm: &mut ViewModel, actions: &mut Actions, request: Request) -> Result<bool> {
    match request {
        Request::Bootstrap => {
            vm.bootstrap()?;
            Ok(true)
        }
        Request::SelectThread(t) => Ok(vm.select(t)),
        Request::Move(m) => vm.move_selection(m),
        Request::SwitchTab(state) => Ok(!vm.set_tab(state)?.is_empty()),
        Request::FilterAccounts(accounts) => Ok(!vm.set_accounts_filter(accounts)?.is_empty()),
        Request::EnsureLoaded(index) => Ok(vm.ensure_loaded(index)? > 0),
        Request::Apply(action) => {
            let Some(thread) = vm.selection().thread() else { return Ok(false) };
            let resultat = actions.apply(thread, action, Timestamp::from_millis(now_millis()))?;
            if !resultat.changed {
                return Ok(false);
            }
            // L'action a modifié l'état : la liste doit être recomposée. On passe par
            // le même chemin qu'un diff venu du réseau, pour qu'il n'y ait qu'une
            // seule façon de mettre la vue à jour.
            let mut diff = ViewDiff::default();
            diff.threads.insert(thread);
            diff.lists.insert(vm.active_tab());
            vm.apply_diff(&diff)?;
            Ok(true)
        }
        Request::Undo => {
            let Some(record) = actions.undo()? else { return Ok(false) };
            let mut diff = ViewDiff::default();
            diff.threads.insert(record.thread);
            diff.lists.insert(vm.active_tab());
            vm.apply_diff(&diff)?;
            Ok(true)
        }
        Request::Diff(diff) => Ok(!vm.apply_diff(&diff)?.is_empty()),
        Request::Tick(now) => {
            vm.set_now(now);
            Ok(false)
        }
        Request::Shutdown => Ok(false),
    }
}

fn snapshot(vm: &ViewModel, store: &Store) -> Snapshot {
    let messages = vm
        .selection()
        .thread()
        .and_then(|t| store.thread_messages(t).ok())
        .unwrap_or_default();

    Snapshot {
        rows: vm.list().rows().to_vec(),
        selected: vm.selection().thread(),
        active_tab: vm.active_tab(),
        counts: vm.counts(),
        loaded: vm.list().loaded(),
        total: vm.list().total(),
        messages,
        pending_ops: store.pending_op_count().unwrap_or(0),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Relie le bus du noyau au contrôleur.
///
/// Les événements passent d'abord par la coalescence : sans elle, une
/// synchronisation de cent comptes enverrait des milliers de requêtes au vue-modèle,
/// qui les traiterait toutes pour un seul état final.
pub async fn pump(bus: &EventBus, controller: Arc<Controller>) {
    let mut lots = coalesce::spawn(bus.subscribe_view(), coalesce::WINDOW);
    while let Some(diff) = lots.recv().await {
        controller.send(Request::Diff(Box::new(diff)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_store::{FolderRole, NewAccount, NewMessage};
    use iris_types::{AccountId, Flags, FolderId};
    use std::sync::mpsc;

    struct Fixture {
        store: Arc<Store>,
        account: AccountId,
        folder: FolderId,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Arc::new(Store::in_memory().unwrap());
        let account = store
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), Timestamp::from_millis(0))
            .unwrap();
        let folder = store.upsert_folder(account, "INBOX", FolderRole::Inbox).unwrap();
        Fixture { store, account, folder, uid: std::cell::Cell::new(1) }
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
                    subject: format!("Sujet {uid}"),
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
    }

    /// Démarre un contrôleur et rend le canal des instantanés.
    fn demarrer(store: Arc<Store>) -> (Controller, mpsc::Receiver<Snapshot>, std::thread::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel();
        let (controller, fil) = Controller::spawn(
            store,
            AutomationSettings::default(),
            Timestamp::from_millis(10_000),
            move |s| {
                let _ = tx.send(s);
            },
        );
        (controller, rx, fil)
    }

    fn attendre(rx: &mpsc::Receiver<Snapshot>) -> Snapshot {
        rx.recv_timeout(std::time::Duration::from_secs(5)).expect("un instantané")
    }

    /// Attend un instantane satisfaisant une condition.
    ///
    /// Plusieurs requetes peuvent produire chacune leur instantane ; attendre le
    /// premier venu rendrait le test dependant de leur ordre d'arrivee.
    fn attendre_que(
        rx: &mpsc::Receiver<Snapshot>,
        condition: impl Fn(&Snapshot) -> bool,
    ) -> Snapshot {
        let echeance = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let restant = echeance.saturating_duration_since(std::time::Instant::now());
            let s = rx.recv_timeout(restant).expect("un instantané satisfaisant la condition");
            if condition(&s) {
                return s;
            }
        }
    }

    #[test]
    fn le_demarrage_produit_un_instantane() {
        let f = fixture();
        for _ in 0..5 {
            f.thread();
        }
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        let s = attendre(&rx);

        assert_eq!(s.rows.len(), 5);
        assert_eq!(s.counts[0], 5);
        assert!(s.selected.is_some());

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn le_vue_modele_vit_dans_son_propre_fil() {
        // C'est la forme du contrôleur qui garantit l'invariant, pas la discipline
        // de l'appelant : le code qui interroge la base ne s'exécute pas ici.
        let f = fixture();
        f.thread();
        let fil_appelant = std::thread::current().id();

        let (tx, rx) = mpsc::channel();
        let (c, fil) = Controller::spawn(
            Arc::clone(&f.store),
            AutomationSettings::default(),
            Timestamp::from_millis(0),
            move |_| {
                let _ = tx.send(std::thread::current().id());
            },
        );

        c.send(Request::Bootstrap);
        let fil_travail = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_ne!(fil_travail, fil_appelant);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn une_action_recompose_la_liste() {
        let f = fixture();
        for _ in 0..3 {
            f.thread();
        }
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        let avant = attendre(&rx);
        assert_eq!(avant.rows.len(), 3);

        c.send(Request::Apply(Action::Done));
        let apres = attendre(&rx);

        assert_eq!(apres.rows.len(), 2, "le fil traité quitte la file");
        assert_eq!(apres.counts[2], 1);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn l_annulation_remet_le_fil_dans_la_file() {
        let f = fixture();
        for _ in 0..2 {
            f.thread();
        }
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        attendre(&rx);
        c.send(Request::Apply(Action::Done));
        attendre(&rx);

        c.send(Request::Undo);
        let s = attendre(&rx);
        assert_eq!(s.rows.len(), 2);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn une_action_sans_effet_n_emet_pas_d_instantane() {
        // Émettre un instantané pour rien ferait redessiner l'interface sans raison.
        let f = fixture();
        f.thread();
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        attendre(&rx);

        c.send(Request::Apply(Action::Todo));
        assert!(rx.recv_timeout(std::time::Duration::from_millis(200)).is_err());

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn le_changement_d_onglet_recharge_la_liste() {
        let f = fixture();
        let fil_id = f.thread();
        f.thread();
        f.store.set_thread_state(fil_id, WorkflowState::Done).unwrap();

        let (c, rx, fil) = demarrer(Arc::clone(&f.store));
        c.send(Request::Bootstrap);
        attendre(&rx);

        c.send(Request::SwitchTab(WorkflowState::Done));
        let s = attendre(&rx);
        assert_eq!(s.active_tab, WorkflowState::Done);
        assert_eq!(s.rows.len(), 1);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn l_instantane_porte_les_messages_du_fil_selectionne() {
        let f = fixture();
        let fil_id = f.thread();
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        let s = attendre(&rx);

        assert_eq!(s.selected, Some(fil_id));
        assert_eq!(s.messages.len(), 1);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn un_diff_venu_du_noyau_met_la_vue_a_jour() {
        let f = fixture();
        f.thread();
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        attendre(&rx);

        f.thread();
        c.send(Request::Diff(Box::new(ViewDiff { full_refresh: true, ..Default::default() })));
        let s = attendre(&rx);
        assert_eq!(s.rows.len(), 2);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn une_erreur_ne_tue_pas_le_fil() {
        // Sinon l'interface resterait figee sans explication.
        let f = fixture();
        f.thread();
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        assert_eq!(attendre(&rx).rows.len(), 1);

        // Selectionner un fil inexistant, puis agir dessus : l'action echoue.
        c.send(Request::SelectThread(ThreadId(9999)));
        c.send(Request::Apply(Action::Done));

        // Le fil repond toujours : un nouveau message finit par apparaitre.
        f.thread();
        c.send(Request::Diff(Box::new(ViewDiff { full_refresh: true, ..Default::default() })));
        let apres = attendre_que(&rx, |s| s.rows.len() == 2);
        assert_eq!(apres.rows.len(), 2);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn l_arret_termine_le_fil() {
        let f = fixture();
        let (c, _rx, fil) = demarrer(Arc::clone(&f.store));
        c.shutdown();
        fil.join().expect("le fil doit se terminer proprement");
    }

    #[test]
    fn perdre_le_controleur_termine_aussi_le_fil() {
        let f = fixture();
        let (c, _rx, fil) = demarrer(Arc::clone(&f.store));
        drop(c);
        fil.join().expect("la fermeture du canal doit suffire");
    }
}
