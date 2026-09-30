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
use iris_viewmodel::{Action, Actions, Movement, ViewModel, Workflow};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// Ce que l'interface demande au vue-modèle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Charge l'état initial.
    Bootstrap,
    SelectThread(ThreadId),
    Move(Movement),
    SwitchTab(WorkflowState),
    /// Montrer une file de travail, ou un dossier.
    ///
    /// Les indésirables avaient leur propre requête et leur propre liste. Ils sont un
    /// dossier maintenant, et un dossier passe par ici comme les autres.
    ShowScope(iris_store::Scope),
    FilterAccounts(Vec<iris_types::AccountId>),
    /// Cocher ou décocher un fil.
    ToggleMark(ThreadId),
    /// Cocher tout ce qui va de l'ancre jusqu'ici.
    ExtendMark(ThreadId),
    /// Cocher tout ce qui est chargé.
    MarkAll,
    /// Les filtres rapides de la liste.
    SetFilters(iris_store::Filters),
    /// The order of the lists: date, sender, subject or size.
    SetSort(iris_store::Sort),
    /// Tout décocher.
    ClearMarks,
    /// Agir sur le lot coché, ou à défaut sur la ligne courante.
    ApplyToMarked(Action),
    /// Ranger le lot coché — ou la ligne courante — dans un dossier.
    ///
    /// Le glisser-déposer et l'entrée « Déplacer vers » du menu passent tous deux par
    /// ici : deux gestes, une seule règle sur ce qu'ils atteignent.
    MoveMarkedToFolder(String),
    /// Ranger un fil désigné. C'est par là que passent les plugins de classement :
    /// ils agissent sur ce qu'on leur a montré, pas sur ce que l'utilisateur regarde
    /// au moment où ils répondent.
    MoveThreadToFolder(ThreadId, String),
    /// L'utilisateur a fait défiler jusqu'à cet indice.
    EnsureLoaded(usize),
    Apply(Action),
    /// Agit sur un fil désigné plutôt que sur la sélection. C'est par là que
    /// passent les plugins : ils agissent sur ce qu'on leur a montré, pas sur ce que
    /// l'utilisateur regarde au moment où ils répondent.
    ApplyTo(iris_types::ThreadId, Action),
    Undo,
    /// Refait ce que la dernière annulation a défait.
    Redo,
    /// Change les automatismes du flux de travail.
    SetAutomation(AutomationSettings),
    /// Cherche. Une requête vide quitte la recherche.
    Search(String),
    /// Quitte la recherche et revient à la file.
    ClearSearch,
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
    /// Combien de conversations le serveur a jugées indésirables. Le décompte du
    /// dossier « Spam » de l'arborescence.
    pub spam_count: u32,
    /// La recherche en cours, s'il y en a une. Les lignes en sont alors issues, et
    /// les compteurs d'onglets continuent de décrire les files, pas les résultats.
    pub search: Option<SearchSummary>,
    /// Les fils cochés. L'interface s'en sert pour marquer les lignes et pour
    /// décider si la barre d'actions groupées a lieu d'être.
    pub marked: std::collections::BTreeSet<ThreadId>,
    /// Ce que la colonne du milieu montre : une file, ou un dossier.
    pub scope: iris_store::Scope,
    /// Les filtres rapides allumés. L'interface en dessine les pastilles ; elle ne les
    /// mémorise pas, pour qu'il n'y ait qu'une seule idée de ce qui est actif.
    pub filters: iris_store::Filters,
    /// The order the lists are in.
    pub sort: iris_store::Sort,
    /// Les comptes montrés. Vide signifie « tous ».
    ///
    /// L'interface s'en sert pour marquer la bonne ligne dans la barre latérale.
    /// Cliquer un compte filtrait bien la liste, et « All accounts » restait allumé :
    /// l'écran désignait une vue qui n'était pas celle affichée.
    pub accounts: Vec<iris_types::AccountId>,
    /// What went wrong with the last request, if anything did.
    ///
    /// An action that fails has to say so. This was a `tracing::error!` and nothing
    /// else, which in a release build with no console means the button simply did
    /// nothing — and a button that does nothing is indistinguishable from a button
    /// that is not wired up. The user cannot tell "your server has no Trash folder"
    /// from "we forgot to implement this", and both look like the application is
    /// broken.
    pub error: Option<String>,
}

/// Ce que l'interface doit dire de la recherche en cours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSummary {
    pub query: String,
    /// Le décompte, en une phrase.
    pub summary: String,
    /// Ce que la requête a été comprise vouloir dire.
    pub explanation: String,
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
        workflow: Arc<Workflow>,
        now: Timestamp,
        on_snapshot: impl Fn(Snapshot) + Send + 'static,
    ) -> (Self, std::thread::JoinHandle<()>) {
        Self::spawn_with_index(store, None, workflow, now, on_snapshot)
    }

    /// Même chose, avec l'index plein texte.
    ///
    /// Sans lui la recherche ne sait faire que du structurel — les non-lus, les
    /// vieux fils — et le dit à l'utilisateur plutôt que de rendre une liste vide.
    pub fn spawn_with_index(
        store: Arc<Store>,
        index: Option<Arc<iris_index::SearchIndex>>,
        workflow: Arc<Workflow>,
        now: Timestamp,
        on_snapshot: impl Fn(Snapshot) + Send + 'static,
    ) -> (Self, std::thread::JoinHandle<()>) {
        let (tx, rx) = std::sync::mpsc::channel();

        let fil = std::thread::Builder::new()
            .name("iris-viewmodel".into())
            .spawn(move || {
                run(store, index, workflow, now, rx, on_snapshot);
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
    index: Option<Arc<iris_index::SearchIndex>>,
    workflow: Arc<Workflow>,
    now: Timestamp,
    requests: Receiver<Request>,
    on_snapshot: impl Fn(Snapshot),
) {
    let mut vm = ViewModel::new(Arc::clone(&store), now);
    if let Some(index) = index {
        vm = vm.with_index(index);
    }
    let mut actions = Actions::new(workflow);

    while let Ok(request) = requests.recv() {
        if request == Request::Shutdown {
            break;
        }

        match handle(&mut vm, &mut actions, request) {
            Ok(true) => on_snapshot(snapshot(&vm, &store)),
            Ok(false) => {}
            // Une erreur du vue-modèle ne doit pas emporter le fil : l'interface
            // resterait figée sans explication. It is also carried out to the status
            // bar, so the explanation reaches the person rather than the log file.
            Err(e) => {
                tracing::error!(error = %e, "vue-modèle");
                let mut instantane = snapshot(&vm, &store);
                instantane.error = Some(e.to_string());
                on_snapshot(instantane);
            }
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
        Request::ShowScope(scope) => Ok(!vm.set_scope(scope)?.is_empty()),
        Request::FilterAccounts(accounts) => Ok(!vm.set_accounts_filter(accounts)?.is_empty()),
        Request::ToggleMark(thread) => {
            vm.toggle_mark(thread);
            Ok(true)
        }
        Request::ExtendMark(thread) => {
            vm.extend_mark_to(thread);
            Ok(true)
        }
        Request::SetFilters(f) => Ok(!vm.set_filters(f)?.is_empty()),
        Request::SetSort(s) => Ok(!vm.set_sort(s)?.is_empty()),
        Request::MarkAll => {
            vm.mark_all_visible();
            Ok(true)
        }
        Request::ClearMarks => Ok(vm.clear_marks()),
        Request::MoveThreadToFolder(fil, chemin) => {
            let maintenant = Timestamp::from_millis(now_millis());
            if !actions.move_to_folder(fil, &chemin, maintenant)? {
                return Ok(false);
            }
            let mut diff = ViewDiff::default();
            diff.threads.insert(fil);
            diff.full_refresh = true;
            vm.apply_diff(&diff)?;
            Ok(true)
        }
        Request::MoveMarkedToFolder(chemin) => {
            let cibles = vm.selection().targets();
            if cibles.is_empty() {
                return Ok(false);
            }

            let maintenant = Timestamp::from_millis(now_millis());
            let mut touches = 0;
            for fil in &cibles {
                if actions.move_to_folder(*fil, &chemin, maintenant)? {
                    touches += 1;
                }
            }
            if touches == 0 {
                return Ok(false);
            }

            let mut diff = ViewDiff::default();
            diff.threads.extend(cibles.iter().copied());
            diff.full_refresh = true;
            vm.apply_diff(&diff)?;

            let restants: std::collections::BTreeSet<ThreadId> =
                vm.visible_threads().into_iter().collect();
            vm.selection_mut().retain_marks(|t| restants.contains(&t));
            Ok(true)
        }
        Request::ApplyToMarked(action) => {
            let cibles = vm.selection().targets();
            if cibles.is_empty() {
                return Ok(false);
            }

            // Un lot passe par `apply_many`, qui pose **une** entrée d'annulation pour
            // l'ensemble : cinquante messages archivés d'un geste doivent revenir d'un
            // geste, et non de cinquante.
            let touches =
                actions.apply_many(&cibles, action, Timestamp::from_millis(now_millis()))?;
            if touches == 0 {
                return Ok(false);
            }

            let mut diff = ViewDiff::default();
            diff.threads.extend(cibles.iter().copied());
            diff.lists.insert(vm.active_tab());
            vm.apply_diff(&diff)?;

            // Ce qui a quitté la liste quitte le lot : le garder ferait porter
            // l'action suivante sur des lignes que personne ne voit.
            let restants: std::collections::BTreeSet<ThreadId> =
                vm.visible_threads().into_iter().collect();
            vm.selection_mut().retain_marks(|t| restants.contains(&t));
            Ok(true)
        }
        Request::EnsureLoaded(index) => Ok(vm.ensure_loaded(index)? > 0),
        Request::Apply(action) => {
            let Some(thread) = vm.selection().thread() else {
                return Ok(false);
            };
            appliquer(vm, actions, thread, action)
        }
        Request::ApplyTo(thread, action) => appliquer(vm, actions, thread, action),
        Request::Undo | Request::Redo => {
            let maintenant = Timestamp::from_millis(now_millis());
            let resultat = if request == Request::Undo {
                actions.undo(maintenant)?
            } else {
                actions.redo(maintenant)?
            };
            let Some(record) = resultat else {
                return Ok(false);
            };
            let mut diff = ViewDiff::default();
            diff.threads.insert(record.thread);
            diff.lists.insert(vm.active_tab());
            vm.apply_diff(&diff)?;
            Ok(true)
        }
        Request::SetAutomation(settings) => {
            actions.set_settings(settings);
            Ok(false)
        }
        Request::Search(query) => Ok(!vm.search(&query)?.is_empty()),
        Request::ClearSearch => Ok(!vm.clear_search().is_empty()),
        Request::Diff(diff) => {
            // Le fil ouvert peut ne figurer dans aucune liste — ouvert depuis une
            // recherche, par exemple. Le diff ne change alors aucune ligne, et sans
            // cette clause le corps arrivé entre-temps ne serait jamais dessiné.
            let lu = vm
                .selection()
                .thread()
                .is_some_and(|t| diff.threads.contains(&t));
            Ok(!vm.apply_diff(&diff)?.is_empty() || lu)
        }
        Request::Tick(now) => {
            vm.set_now(now);
            Ok(false)
        }
        Request::Shutdown => Ok(false),
    }
}

/// Applique une action à un fil désigné.
///
/// Chemin unique, partagé par le clavier, la palette et les plugins : l'action passe
/// ensuite par le même diff qu'un changement venu du réseau, pour qu'il n'y ait
/// qu'une seule façon de mettre la vue à jour.
fn appliquer(
    vm: &mut ViewModel,
    actions: &mut Actions,
    thread: ThreadId,
    action: Action,
) -> Result<bool> {
    let resultat = actions.apply(thread, action, Timestamp::from_millis(now_millis()))?;
    if !resultat.changed {
        return Ok(false);
    }

    let mut diff = ViewDiff::default();
    diff.threads.insert(thread);
    diff.lists.insert(vm.active_tab());
    vm.apply_diff(&diff)?;
    Ok(true)
}

fn snapshot(vm: &ViewModel, store: &Store) -> Snapshot {
    let messages = vm
        .selection()
        .thread()
        .and_then(|t| store.thread_messages(t).ok())
        .unwrap_or_default();

    let recherche = vm.search_state().map(|s| SearchSummary {
        query: s.query.clone(),
        summary: s.summary(),
        explanation: s.explanation.clone(),
    });

    // Pendant une recherche, les résultats sont tous là : « chargé » et « total »
    // se confondent, et la barre de défilement dit la vérité sans mentir sur une
    // suite qui n'existe pas.
    let (charge, total) = match vm.search_state() {
        Some(s) => (s.len(), s.len() as u32),
        None => (vm.list().loaded(), vm.list().total()),
    };

    Snapshot {
        error: None,
        marked: vm.selection().marked().clone(),
        scope: vm.scope().clone(),
        filters: vm.filters(),
        sort: vm.sort(),
        accounts: vm.accounts_filter().to_vec(),
        rows: vm.rows().to_vec(),
        selected: vm.selection().thread(),
        active_tab: vm.active_tab(),
        counts: vm.counts(),
        loaded: charge,
        total,
        messages,
        pending_ops: store.pending_op_count().unwrap_or(0),
        spam_count: vm.spam_count(),
        search: recherche,
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Builds a workflow engine over a store, with default settings.
///
/// The application builds it in `Services`; tests and small callers use this so they
/// get the same wiring instead of inventing their own.
pub fn default_workflow(store: Arc<Store>) -> Arc<Workflow> {
    Arc::new(Workflow::new(
        store,
        EventBus::new(),
        AutomationSettings::default(),
    ))
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
    fn demarrer(
        store: Arc<Store>,
    ) -> (
        Controller,
        mpsc::Receiver<Snapshot>,
        std::thread::JoinHandle<()>,
    ) {
        let (tx, rx) = mpsc::channel();
        let workflow = default_workflow(Arc::clone(&store));
        let (controller, fil) =
            Controller::spawn(store, workflow, Timestamp::from_millis(10_000), move |s| {
                let _ = tx.send(s);
            });
        (controller, rx, fil)
    }

    fn attendre(rx: &mpsc::Receiver<Snapshot>) -> Snapshot {
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("un instantané")
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
            let s = rx
                .recv_timeout(restant)
                .expect("un instantané satisfaisant la condition");
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
            default_workflow(Arc::clone(&f.store)),
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
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err());

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn le_changement_d_onglet_recharge_la_liste() {
        let f = fixture();
        let fil_id = f.thread();
        f.thread();
        f.store
            .set_thread_state(fil_id, WorkflowState::Done)
            .unwrap();

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
        c.send(Request::Diff(Box::new(ViewDiff {
            full_refresh: true,
            ..Default::default()
        })));
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
        c.send(Request::Diff(Box::new(ViewDiff {
            full_refresh: true,
            ..Default::default()
        })));
        let apres = attendre_que(&rx, |s| s.rows.len() == 2);
        assert_eq!(apres.rows.len(), 2);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn une_recherche_remplace_les_lignes_de_l_instantane() {
        let f = fixture();
        let fil_id = f.thread();
        f.thread();
        f.store
            .set_thread_state(fil_id, WorkflowState::Done)
            .unwrap();

        let (c, rx, fil) = demarrer(Arc::clone(&f.store));
        c.send(Request::Bootstrap);
        assert_eq!(attendre(&rx).rows.len(), 1);

        c.send(Request::Search("etat:traite".into()));
        let s = attendre_que(&rx, |s| s.search.is_some());

        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.rows[0].id, fil_id);
        let recherche = s.search.expect("un résumé de recherche");
        assert_eq!(recherche.query, "etat:traite");
        assert_eq!(recherche.summary, "1 conversation.");
        assert_eq!(recherche.explanation, "traité");

        // Les compteurs continuent de décrire les files, pas les résultats.
        assert_eq!(s.counts[0], 1);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn quitter_la_recherche_rend_la_file() {
        let f = fixture();
        f.thread();
        f.thread();

        let (c, rx, fil) = demarrer(Arc::clone(&f.store));
        c.send(Request::Bootstrap);
        attendre(&rx);

        c.send(Request::Search("etat:traite".into()));
        attendre_que(&rx, |s| s.search.is_some());

        c.send(Request::ClearSearch);
        let s = attendre_que(&rx, |s| s.search.is_none());
        assert_eq!(s.rows.len(), 2);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn une_recherche_vide_ne_change_rien() {
        // Sinon appuyer sur Entrée dans une barre vide redessinerait l'écran.
        let f = fixture();
        f.thread();
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        attendre(&rx);

        c.send(Request::Search("  ".into()));
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err());

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn trier_depuis_les_resultats_les_met_a_jour() {
        let f = fixture();
        let a = f.thread();
        f.thread();

        let (c, rx, fil) = demarrer(Arc::clone(&f.store));
        c.send(Request::Bootstrap);
        attendre(&rx);

        c.send(Request::Search("etat:a_traiter".into()));
        let s = attendre_que(&rx, |s| s.search.is_some());
        assert_eq!(s.rows.len(), 2);

        c.send(Request::SelectThread(a));
        c.send(Request::Apply(Action::Done));
        let apres = attendre_que(&rx, |s| s.rows.len() == 1);

        assert!(apres.search.is_some(), "on reste dans la recherche");
        assert_ne!(apres.rows[0].id, a);

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn changer_les_automatismes_change_le_comportement_des_actions() {
        // Un panneau de réglages qui enregistre bien et ne change rien serait pire
        // qu'absent.
        let f = fixture();
        let fil_id = f.thread();
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        attendre(&rx);

        // Par défaut, répondre met en attente ; sans l'automatisme, l'état ne bouge
        // plus tout seul.
        c.send(Request::SetAutomation(AutomationSettings::MANUAL_ONLY));
        c.send(Request::Apply(Action::Waiting));
        attendre_que(&rx, |s| s.counts[1] == 1);

        assert_eq!(
            f.store.thread_row(fil_id).unwrap().unwrap().state,
            WorkflowState::Waiting,
            "l'action manuelle reste possible"
        );

        c.shutdown();
        fil.join().unwrap();
    }

    #[test]
    fn changer_les_automatismes_n_emet_pas_d_instantane() {
        // Rien n'a bougé à l'écran : redessiner serait du bruit.
        let f = fixture();
        f.thread();
        let (c, rx, fil) = demarrer(Arc::clone(&f.store));

        c.send(Request::Bootstrap);
        attendre(&rx);

        c.send(Request::SetAutomation(AutomationSettings::MANUAL_ONLY));
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err());

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
