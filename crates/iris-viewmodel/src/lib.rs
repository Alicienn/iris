//! `iris-viewmodel` — le pont entre le domaine et l'interface.
//!
//! Cette couche existe pour une raison précise : **rendre l'interface testable sans
//! interface**. Tout ce qui décide de ce qui s'affiche — quelle ligne, dans quel
//! ordre, avec quelle sélection, et ce qu'une touche déclenche — vit ici, se teste
//! en une milliseconde, et ne dépend d'aucun pilote graphique.
//!
//! Elle porte aussi l'invariant n° 1 : le thread d'affichage ne fait jamais d'entrée-
//! sortie. Le vue-modèle est explicitement **bloquant** ; l'application l'exécute
//! hors du fil de rendu et ne lui transmet que des diffs déjà calculés.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod actions;
pub mod list;
pub mod search;
pub mod selection;

pub use actions::{Action, ActionOutcome, Actions};
pub use iris_workflow::{UndoEntry, Workflow};
pub use list::{ListUpdate, ThreadList, PAGE_SIZE, PREFETCH};
pub use search::{SearchState, MAX_RESULTS};
pub use selection::{Movement, Selection};

use iris_index::SearchIndex;
use iris_kernel::ViewDiff;
use iris_store::{Store, ThreadRow};
use iris_types::{AccountId, Result, ThreadId, Timestamp, WorkflowState};
use std::sync::Arc;

/// L'état affichable de l'application.
#[derive(Debug)]
pub struct ViewModel {
    store: Arc<Store>,
    lists: [ThreadList; 3],
    active_tab: WorkflowState,
    selection: Selection,
    accounts: Vec<AccountId>,
    counts: [u32; 3],
    now: Timestamp,
    /// L'index plein texte, quand il est disponible.
    index: Option<Arc<SearchIndex>>,
    /// La recherche en cours. Tant qu'elle est là, elle **remplace** la liste
    /// affichée : une recherche sans effet visible sur la colonne du milieu ne
    /// servirait à rien.
    search: Option<SearchState>,
}

/// Ce qui a changé après une mise à jour, tel que l'interface doit le traiter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewUpdate {
    pub list: ListUpdate,
    /// Les compteurs des onglets ont bougé.
    pub counts_changed: bool,
    /// La sélection a changé de fil.
    pub selection_changed: bool,
    /// On est entré ou sorti de la recherche.
    pub search_changed: bool,
}

impl ViewUpdate {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
            && !self.counts_changed
            && !self.selection_changed
            && !self.search_changed
    }
}

impl ViewModel {
    pub fn new(store: Arc<Store>, now: Timestamp) -> Self {
        Self {
            store,
            lists: [
                ThreadList::new(WorkflowState::Todo, now),
                ThreadList::new(WorkflowState::Waiting, now),
                ThreadList::new(WorkflowState::Done, now),
            ],
            active_tab: WorkflowState::Todo,
            selection: Selection::default(),
            accounts: Vec::new(),
            counts: [0; 3],
            now,
            index: None,
            search: None,
        }
    }

    /// Attache l'index plein texte.
    ///
    /// Il reste facultatif : sans lui, les recherches structurelles fonctionnent
    /// encore, et le vue-modèle se teste sans monter de moteur d'indexation.
    pub fn with_index(mut self, index: Arc<SearchIndex>) -> Self {
        self.index = Some(index);
        self
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn active_tab(&self) -> WorkflowState {
        self.active_tab
    }

    pub fn counts(&self) -> [u32; 3] {
        self.counts
    }

    pub fn count_of(&self, state: WorkflowState) -> u32 {
        self.counts[state.as_i64() as usize]
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn accounts_filter(&self) -> &[AccountId] {
        &self.accounts
    }

    fn list_mut(&mut self, state: WorkflowState) -> &mut ThreadList {
        &mut self.lists[state.as_i64() as usize]
    }

    pub fn list(&self) -> &ThreadList {
        &self.lists[self.active_tab.as_i64() as usize]
    }

    pub fn list_of(&self, state: WorkflowState) -> &ThreadList {
        &self.lists[state.as_i64() as usize]
    }

    // --- Ce qui est réellement affiché ---
    //
    // Pendant une recherche, la colonne du milieu montre les résultats et non la
    // file. Toute la navigation passe par ces trois méthodes, pour qu'il n'existe pas
    // un chemin qui oublierait la recherche et sélectionnerait une ligne invisible.

    /// Les lignes affichées : les résultats de recherche, ou la file active.
    pub fn rows(&self) -> &[ThreadRow] {
        match &self.search {
            Some(s) => &s.results,
            None => self.list().rows(),
        }
    }

    fn row_at(&self, index: usize) -> Option<&ThreadRow> {
        self.rows().get(index)
    }

    fn index_of(&self, thread: ThreadId) -> Option<usize> {
        match &self.search {
            Some(s) => s.results.iter().position(|r| r.id == thread),
            None => self.list().index_of(thread),
        }
    }

    /// La recherche en cours, s'il y en a une.
    pub fn search_state(&self) -> Option<&SearchState> {
        self.search.as_ref()
    }

    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// Lance une recherche. Une requête vide en sort.
    pub fn search(&mut self, query: &str) -> Result<ViewUpdate> {
        if query.trim().is_empty() {
            return Ok(self.clear_search());
        }

        let etat = search::run(
            &self.store,
            self.index.as_ref(),
            query,
            &self.accounts,
            self.now,
        )?;
        self.search = Some(etat);
        self.select_first();

        Ok(ViewUpdate {
            list: ListUpdate {
                reordered: true,
                ..Default::default()
            },
            counts_changed: false,
            selection_changed: true,
            search_changed: true,
        })
    }

    /// Quitte la recherche et revient à la file.
    pub fn clear_search(&mut self) -> ViewUpdate {
        if self.search.take().is_none() {
            return ViewUpdate::default();
        }
        self.select_first();
        ViewUpdate {
            list: ListUpdate {
                reordered: true,
                ..Default::default()
            },
            counts_changed: false,
            selection_changed: true,
            search_changed: true,
        }
    }

    /// Rejoue la recherche courante sur l'état actuel du store.
    ///
    /// Sans cela, trier depuis les résultats laisserait à l'écran des lignes qui ne
    /// correspondent plus : un fil marqué traité y resterait affiché comme à traiter.
    fn refresh_search(&mut self) -> Result<bool> {
        let Some(courante) = &self.search else {
            return Ok(false);
        };
        let requete = courante.query.clone();
        let etat = search::run(
            &self.store,
            self.index.as_ref(),
            &requete,
            &self.accounts,
            self.now,
        )?;
        let change = etat.results != courante.results;
        self.search = Some(etat);
        Ok(change)
    }

    /// Charge l'état initial : la première page de l'onglet actif et les compteurs.
    ///
    /// Seul l'onglet visible est chargé. Précharger les trois multiplierait par trois
    /// le temps avant le premier affichage, pour deux listes que l'utilisateur ne
    /// regarde pas.
    pub fn bootstrap(&mut self) -> Result<()> {
        self.refresh_counts()?;
        let store = Arc::clone(&self.store);
        let onglet = self.active_tab;
        self.list_mut(onglet).ensure_loaded(&store, 0)?;
        self.select_first();
        Ok(())
    }

    pub fn refresh_counts(&mut self) -> Result<bool> {
        let nouveaux = self.store.state_counts(Some(self.now))?;
        let change = nouveaux != self.counts;
        self.counts = nouveaux;
        Ok(change)
    }

    /// Change d'onglet. La liste correspondante est chargée à la demande.
    pub fn set_tab(&mut self, state: WorkflowState) -> Result<ViewUpdate> {
        if self.active_tab == state {
            return Ok(ViewUpdate::default());
        }
        self.active_tab = state;
        // Changer d'onglet est une sortie de recherche : les résultats ne sont pas
        // rangés par file, et les garder afficherait la mauvaise chose sous le
        // mauvais titre.
        let recherche = self.search.take().is_some();

        let store = Arc::clone(&self.store);
        self.list_mut(state).ensure_loaded(&store, 0)?;
        self.select_first();

        Ok(ViewUpdate {
            list: ListUpdate {
                reordered: true,
                ..Default::default()
            },
            counts_changed: false,
            selection_changed: true,
            search_changed: recherche,
        })
    }

    /// Restreint l'affichage à certains comptes.
    pub fn set_accounts_filter(&mut self, accounts: Vec<AccountId>) -> Result<ViewUpdate> {
        if self.accounts == accounts {
            return Ok(ViewUpdate::default());
        }
        self.accounts = accounts.clone();

        let store = Arc::clone(&self.store);
        for liste in &mut self.lists {
            liste.set_accounts(accounts.clone());
            liste.reload(&store)?;
        }
        // Le filtre par compte s'applique aussi aux résultats : rejouer la recherche
        // vaut mieux que la fermer sans prévenir.
        self.refresh_search()?;
        self.select_first();
        self.refresh_counts()?;

        Ok(ViewUpdate {
            list: ListUpdate {
                reordered: true,
                ..Default::default()
            },
            counts_changed: true,
            selection_changed: true,
            search_changed: false,
        })
    }

    /// Garantit que l'indice demandé est chargé. Appelé au défilement.
    pub fn ensure_loaded(&mut self, index: usize) -> Result<usize> {
        let store = Arc::clone(&self.store);
        let onglet = self.active_tab;
        self.list_mut(onglet).ensure_loaded(&store, index)
    }

    /// Applique un lot de diffs venu du noyau.
    pub fn apply_diff(&mut self, diff: &ViewDiff) -> Result<ViewUpdate> {
        let store = Arc::clone(&self.store);
        let onglet = self.active_tab;

        let liste = self.lists[onglet.as_i64() as usize].apply(
            &store,
            diff.full_refresh,
            &diff.threads,
            &diff.lists,
        )?;

        // Les listes inactives sont invalidées sans être rechargées : elles le seront
        // quand l'utilisateur y viendra. Recharger trois listes à chaque diff
        // reviendrait à payer trois fois pour une seule vue.
        for etat in WorkflowState::ALL {
            if etat != onglet && (diff.full_refresh || diff.lists.contains(&etat)) {
                self.lists[etat.as_i64() as usize].trim(0);
            }
        }

        let recherche = self.refresh_search()?;
        let compteurs = self.refresh_counts()?;
        let selection = self.reconcile_selection();

        Ok(ViewUpdate {
            list: ListUpdate {
                reordered: liste.reordered || recherche,
                ..liste
            },
            counts_changed: compteurs,
            selection_changed: selection,
            search_changed: false,
        })
    }

    // --- Sélection ---

    pub fn select_first(&mut self) {
        let premier = self.row_at(0).map(|r| r.id);
        self.selection.set(premier);
        self.selection.remember_index(0);
    }

    pub fn select(&mut self, thread: ThreadId) -> bool {
        // La position est mémorisée en même temps que le fil : c'est elle qui permet
        // de reprendre au même endroit quand le fil quitte la liste.
        if let Some(index) = self.index_of(thread) {
            self.selection.remember_index(index);
        }
        self.selection.set(Some(thread))
    }

    /// Déplace la sélection. Charge les lignes nécessaires en chemin.
    pub fn move_selection(&mut self, movement: Movement) -> Result<bool> {
        let courant = self.selection.thread().and_then(|t| self.index_of(t));

        let cible = match (courant, movement) {
            (None, Movement::Next) | (None, Movement::First) => 0,
            (None, _) => 0,
            (Some(i), Movement::Next) => i + 1,
            (Some(i), Movement::Previous) => i.saturating_sub(1),
            (Some(_), Movement::First) => 0,
            (Some(i), Movement::PageDown) => i + 20,
            (Some(i), Movement::PageUp) => i.saturating_sub(20),
            (Some(_), Movement::Last) => usize::MAX,
        };

        // Les résultats de recherche sont bornés et déjà tous en mémoire : rien à
        // charger, et « aller à la fin » désigne la dernière ligne trouvée.
        if self.search.is_some() {
            let dernier = self.rows().len().saturating_sub(1);
            let cible = cible.min(dernier);
            let Some(id) = self.row_at(cible).map(|r| r.id) else {
                return Ok(false);
            };
            self.selection.remember_index(cible);
            return Ok(self.selection.set(Some(id)));
        }

        if cible == usize::MAX {
            // Aller à la fin oblige à tout charger : c'est un geste rare et
            // explicite, et l'utilisateur en accepte le coût.
            let store = Arc::clone(&self.store);
            let onglet = self.active_tab;
            while !self.lists[onglet.as_i64() as usize].is_exhausted() {
                let charge = self.lists[onglet.as_i64() as usize].loaded();
                self.lists[onglet.as_i64() as usize].ensure_loaded(&store, charge + 1)?;
            }
            let dernier = self.list().loaded().saturating_sub(1);
            let id = self.list().row(dernier).map(|r| r.id);
            self.selection.remember_index(dernier);
            return Ok(self.selection.set(id));
        }

        self.ensure_loaded(cible)?;
        match self.list().row(cible).map(|r| r.id) {
            Some(id) => {
                self.selection.remember_index(cible);
                Ok(self.selection.set(Some(id)))
            }
            // Au-delà de la fin : la sélection ne bouge pas plutôt que de disparaître.
            None => Ok(false),
        }
    }

    /// Remet la sélection sur une ligne existante après un rechargement.
    fn reconcile_selection(&mut self) -> bool {
        let Some(courant) = self.selection.thread() else {
            let premier = self.row_at(0).map(|r| r.id);
            return self.selection.set(premier);
        };

        if let Some(index) = self.index_of(courant) {
            self.selection.remember_index(index);
            return false;
        }

        // Le fil sélectionné a quitté la liste : on reprend à la position la plus
        // proche plutôt que de tout désélectionner, ce qui interromprait le triage
        // au clavier à chaque action.
        let position = self
            .selection
            .last_index()
            .min(self.rows().len().saturating_sub(1));
        let remplacant = self.row_at(position).map(|r| r.id);
        self.selection.remember_index(position);
        self.selection.set(remplacant)
    }

    /// La ligne actuellement sélectionnée.
    pub fn selected_row(&self) -> Option<&ThreadRow> {
        let thread = self.selection.thread()?;
        let index = self.index_of(thread)?;
        self.row_at(index)
    }

    pub fn set_now(&mut self, now: Timestamp) {
        self.now = now;
        for liste in &mut self.lists {
            liste.set_now(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_store::{FolderRole, NewAccount, NewMessage};
    use iris_types::{Flags, FolderId};

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
        fn thread(&self, millis: i64) -> ThreadId {
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
                    from_name: format!("Exp {uid}"),
                    from_addr: "exp@example.com".into(),
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

        fn seed(&self, n: i64) -> Vec<ThreadId> {
            (0..n).map(|i| self.thread(1000 + i)).collect()
        }

        fn vm(&self) -> ViewModel {
            ViewModel::new(Arc::clone(&self.store), Timestamp::from_millis(10_000))
        }
    }

    #[test]
    fn le_demarrage_charge_l_onglet_actif_et_selectionne_la_premiere_ligne() {
        let f = fixture();
        f.seed(10);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        assert_eq!(vm.active_tab(), WorkflowState::Todo);
        assert!(vm.list().loaded() > 0);
        assert!(vm.selection().thread().is_some());
        assert_eq!(vm.count_of(WorkflowState::Todo), 10);
    }

    #[test]
    fn le_demarrage_ne_charge_pas_les_onglets_invisibles() {
        // Les précharger triplerait le temps avant le premier affichage.
        let f = fixture();
        f.seed(10);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        assert_eq!(vm.list_of(WorkflowState::Waiting).loaded(), 0);
        assert_eq!(vm.list_of(WorkflowState::Done).loaded(), 0);
    }

    #[test]
    fn une_boite_vide_ne_selectionne_rien() {
        let f = fixture();
        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        assert!(vm.selection().thread().is_none());
        assert!(vm.selected_row().is_none());
    }

    #[test]
    fn changer_d_onglet_charge_la_liste_correspondante() {
        let f = fixture();
        let fils = f.seed(5);
        f.store
            .set_thread_state(fils[0], WorkflowState::Done)
            .unwrap();

        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        let update = vm.set_tab(WorkflowState::Done).unwrap();

        assert!(update.list.reordered);
        assert_eq!(vm.list().loaded(), 1);
        assert_eq!(vm.selection().thread(), Some(fils[0]));
    }

    #[test]
    fn changer_pour_l_onglet_courant_ne_fait_rien() {
        let f = fixture();
        f.seed(3);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        assert!(vm.set_tab(WorkflowState::Todo).unwrap().is_empty());
    }

    #[test]
    fn la_navigation_au_clavier_suit_la_liste() {
        let f = fixture();
        let fils = f.seed(10);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        // Les fils sont affichés du plus récent au plus ancien.
        let attendu: Vec<ThreadId> = fils.iter().rev().copied().collect();
        assert_eq!(vm.selection().thread(), Some(attendu[0]));

        vm.move_selection(Movement::Next).unwrap();
        assert_eq!(vm.selection().thread(), Some(attendu[1]));

        vm.move_selection(Movement::Previous).unwrap();
        assert_eq!(vm.selection().thread(), Some(attendu[0]));
    }

    #[test]
    fn la_navigation_ne_sort_pas_de_la_liste() {
        let f = fixture();
        f.seed(3);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        vm.move_selection(Movement::Previous).unwrap();
        assert_eq!(
            vm.list().index_of(vm.selection().thread().unwrap()),
            Some(0)
        );

        for _ in 0..10 {
            vm.move_selection(Movement::Next).unwrap();
        }
        assert_eq!(
            vm.list().index_of(vm.selection().thread().unwrap()),
            Some(2)
        );
    }

    #[test]
    fn la_navigation_charge_les_lignes_en_chemin() {
        let f = fixture();
        f.seed(300);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        let charge_au_depart = vm.list().loaded();

        for _ in 0..5 {
            vm.move_selection(Movement::PageDown).unwrap();
        }
        assert!(vm.list().loaded() > charge_au_depart);
        assert!(vm.selection().thread().is_some());
    }

    #[test]
    fn aller_a_la_fin_charge_tout_et_selectionne_la_derniere() {
        let f = fixture();
        let fils = f.seed(150);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        vm.move_selection(Movement::Last).unwrap();
        assert_eq!(vm.selection().thread(), Some(fils[0]), "le plus ancien");
        assert!(vm.list().is_exhausted());
    }

    #[test]
    fn un_diff_cible_ne_recharge_pas_la_liste() {
        let f = fixture();
        let fils = f.seed(20);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        let mut diff = ViewDiff::default();
        diff.threads.insert(fils[10]);

        let update = vm.apply_diff(&diff).unwrap();
        assert!(!update.list.reordered);
    }

    #[test]
    fn un_diff_global_recharge_et_met_a_jour_les_compteurs() {
        let f = fixture();
        f.seed(5);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        f.seed(3);
        let diff = ViewDiff {
            full_refresh: true,
            ..Default::default()
        };
        let update = vm.apply_diff(&diff).unwrap();

        assert!(update.list.reordered);
        assert!(update.counts_changed);
        assert_eq!(vm.count_of(WorkflowState::Todo), 8);
    }

    #[test]
    fn la_selection_survit_a_la_disparition_de_son_fil() {
        // Sans cela, le triage au clavier s'interromprait à chaque action.
        let f = fixture();
        let _ = f.seed(10);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        vm.move_selection(Movement::Next).unwrap();
        vm.move_selection(Movement::Next).unwrap();
        let selectionne = vm.selection().thread().unwrap();
        let position = vm.list().index_of(selectionne).unwrap();

        f.store
            .set_thread_state(selectionne, WorkflowState::Done)
            .unwrap();
        let mut diff = ViewDiff::default();
        diff.lists.insert(WorkflowState::Todo);
        let update = vm.apply_diff(&diff).unwrap();

        assert!(update.selection_changed);
        let nouveau = vm
            .selection()
            .thread()
            .expect("une ligne reste sélectionnée");
        assert_eq!(
            vm.list().index_of(nouveau),
            Some(position),
            "la sélection reste à la même position dans la liste"
        );
    }

    #[test]
    fn les_onglets_invisibles_sont_invalides_sans_etre_recharges() {
        // Recharger trois listes à chaque diff paierait trois fois pour une vue.
        let f = fixture();
        let fils = f.seed(5);
        f.store
            .set_thread_state(fils[0], WorkflowState::Waiting)
            .unwrap();

        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        vm.set_tab(WorkflowState::Waiting).unwrap();
        vm.set_tab(WorkflowState::Todo).unwrap();
        assert!(vm.list_of(WorkflowState::Waiting).loaded() > 0);

        let diff = ViewDiff {
            full_refresh: true,
            ..Default::default()
        };
        vm.apply_diff(&diff).unwrap();

        assert_eq!(
            vm.list_of(WorkflowState::Waiting).loaded(),
            0,
            "invalidée, pas rechargée"
        );
        assert!(vm.list().loaded() > 0, "l'onglet actif, lui, est rechargé");
    }

    #[test]
    fn le_filtre_par_compte_recharge_toutes_les_listes() {
        let f = fixture();
        f.seed(5);
        let autre = f
            .store
            .create_account(
                &NewAccount::new("b@x.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();

        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        assert_eq!(vm.list().loaded(), 5);

        let update = vm.set_accounts_filter(vec![autre]).unwrap();
        assert!(update.list.reordered);
        assert_eq!(vm.list().loaded(), 0);
        assert_eq!(vm.accounts_filter(), [autre]);
    }

    #[test]
    fn appliquer_deux_fois_le_meme_filtre_ne_fait_rien() {
        let f = fixture();
        f.seed(3);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        vm.set_accounts_filter(vec![f.account]).unwrap();
        assert!(vm.set_accounts_filter(vec![f.account]).unwrap().is_empty());
    }

    // --- La recherche vue depuis le vue-modèle ---

    #[test]
    fn une_recherche_remplace_la_liste_affichee() {
        let f = fixture();
        f.seed(5);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        assert_eq!(vm.rows().len(), 5);

        // « Sujet 1 » n'existe que sur le premier fil.
        let update = vm.search("etat:a_traiter").unwrap();
        assert!(update.search_changed);
        assert!(vm.is_searching());
        assert_eq!(vm.rows().len(), 5);
    }

    #[test]
    fn quitter_la_recherche_rend_la_file() {
        let f = fixture();
        f.seed(4);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        vm.search("etat:traite").unwrap();
        assert_eq!(vm.rows().len(), 0, "aucun fil traité");

        let update = vm.clear_search();
        assert!(update.search_changed);
        assert!(!vm.is_searching());
        assert_eq!(vm.rows().len(), 4);
    }

    #[test]
    fn une_requete_vide_sort_de_la_recherche() {
        let f = fixture();
        f.seed(3);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        vm.search("etat:traite").unwrap();
        vm.search("   ").unwrap();
        assert!(!vm.is_searching());
    }

    #[test]
    fn quitter_une_recherche_inexistante_ne_fait_rien() {
        let f = fixture();
        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        assert!(vm.clear_search().is_empty());
    }

    #[test]
    fn la_navigation_suit_les_resultats_et_pas_la_file() {
        // Sans cela, une flèche dans les résultats sélectionnerait une ligne
        // invisible, prise dans la file d'en dessous.
        let f = fixture();
        let fils = f.seed(10);
        for fil in &fils[..8] {
            f.store.set_thread_state(*fil, WorkflowState::Done).unwrap();
        }

        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        vm.search("etat:a_traiter").unwrap();
        assert_eq!(vm.rows().len(), 2);

        vm.move_selection(Movement::Next).unwrap();
        let second = vm.selection().thread().unwrap();
        assert_eq!(vm.rows()[1].id, second);

        // Au-delà du dernier résultat, la sélection ne s'échappe pas.
        for _ in 0..5 {
            vm.move_selection(Movement::Next).unwrap();
        }
        assert_eq!(vm.selection().thread(), Some(second));
    }

    #[test]
    fn aller_a_la_fin_des_resultats_ne_charge_rien() {
        let f = fixture();
        f.seed(6);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        vm.search("etat:a_traiter").unwrap();

        vm.move_selection(Movement::Last).unwrap();
        let dernier = vm.rows().last().unwrap().id;
        assert_eq!(vm.selection().thread(), Some(dernier));
    }

    #[test]
    fn trier_depuis_les_resultats_les_met_a_jour() {
        // Sinon un fil marqué traité resterait affiché comme à traiter.
        let f = fixture();
        let fils = f.seed(3);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        vm.search("etat:a_traiter").unwrap();
        assert_eq!(vm.rows().len(), 3);

        f.store
            .set_thread_state(fils[0], WorkflowState::Done)
            .unwrap();
        let mut diff = ViewDiff::default();
        diff.threads.insert(fils[0]);
        vm.apply_diff(&diff).unwrap();

        assert_eq!(vm.rows().len(), 2, "le fil traité quitte les résultats");
        assert!(vm.is_searching(), "on reste dans la recherche");
    }

    #[test]
    fn changer_d_onglet_quitte_la_recherche() {
        // Les résultats ne sont pas rangés par file : les garder afficherait la
        // mauvaise chose sous le mauvais titre.
        let f = fixture();
        f.seed(3);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        vm.search("etat:a_traiter").unwrap();

        let update = vm.set_tab(WorkflowState::Done).unwrap();
        assert!(update.search_changed);
        assert!(!vm.is_searching());
    }

    #[test]
    fn la_ligne_selectionnee_vient_des_resultats() {
        let f = fixture();
        let fils = f.seed(4);
        f.store
            .set_thread_state(fils[3], WorkflowState::Waiting)
            .unwrap();

        let mut vm = f.vm();
        vm.bootstrap().unwrap();
        vm.search("etat:en_attente").unwrap();

        let ligne = vm.selected_row().expect("un résultat sélectionné");
        assert_eq!(ligne.id, fils[3]);
        assert_eq!(ligne.state, WorkflowState::Waiting);
    }

    #[test]
    fn sans_index_la_recherche_structurelle_marche_encore() {
        // Le vue-modèle se teste sans monter de moteur d'indexation.
        let f = fixture();
        f.seed(3);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        vm.search("is:unread").unwrap();
        assert_eq!(vm.rows().len(), 3);
    }

    #[test]
    fn la_ligne_selectionnee_est_accessible() {
        let f = fixture();
        f.seed(3);
        let mut vm = f.vm();
        vm.bootstrap().unwrap();

        let ligne = vm.selected_row().expect("une ligne sélectionnée");
        assert_eq!(ligne.id, vm.selection().thread().unwrap());
    }
}
