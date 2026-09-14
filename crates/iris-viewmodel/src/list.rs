//! La fenêtre de lignes.
//!
//! Invariant n° 2 : rien n'est chargé en entier. La liste ne détient qu'un préfixe
//! des lignes — celles que l'utilisateur a pu atteindre en faisant défiler — et
//! l'étend page par page, par curseur. Une boîte d'un million de messages n'occupe
//! donc que la mémoire de ce qui a été regardé.
//!
//! Un mot sur la barre de défilement : avec une pagination par curseur, on ne peut
//! pas sauter directement à la millionième ligne, parce qu'on ignore où elle
//! commence. La liste expose donc le nombre de lignes **chargées** et un compteur
//! total séparé. C'est le compromis honnête — celui que font tous les systèmes qui
//! refusent de payer un `OFFSET` d'un million de lignes à chaque saut.

use iris_store::{ListCursor, ListQuery, Store, ThreadRow};
use iris_types::{AccountId, Result, ThreadId, Timestamp, WorkflowState};
use std::collections::BTreeSet;

/// Nombre de lignes chargées par page.
///
/// Assez pour couvrir un écran haute résolution en une requête, assez peu pour que
/// la première page arrive immédiatement.
pub const PAGE_SIZE: u32 = 60;

/// Lignes chargées au-delà du visible, pour que le défilement ne bute jamais sur du
/// vide.
pub const PREFETCH: usize = 40;

/// Une liste paginée pour un état de workflow donné.
#[derive(Debug)]
pub struct ThreadList {
    state: WorkflowState,
    accounts: Vec<AccountId>,
    /// Une file de travail, ou un dossier.
    ///
    /// La liste des indésirables était une quatrième liste à part, avec son onglet.
    /// Elle a disparu : les indésirables sont un dossier, et un dossier est une portée
    /// comme une autre. Une liste de moins à tenir cohérente avec les trois autres.
    scope: iris_store::Scope,
    /// Préfixe chargé, dans l'ordre d'affichage.
    rows: Vec<ThreadRow>,
    cursor: Option<ListCursor>,
    /// Le store n'a plus rien à donner.
    exhausted: bool,
    page_size: u32,
    /// Nombre total de fils dans cet état, tous non chargés compris.
    total: u32,
    /// Instant de référence pour masquer les fils reportés.
    now: Timestamp,
}

/// Ce qu'une mise à jour a changé, pour que l'interface ne redessine que l'utile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListUpdate {
    /// Indices des lignes dont le contenu a changé.
    pub changed_rows: Vec<usize>,
    /// La composition ou l'ordre de la liste a changé : tout est à redessiner.
    pub reordered: bool,
    /// Nombre de lignes ajoutées à la fin.
    pub appended: usize,
}

impl ListUpdate {
    pub fn is_empty(&self) -> bool {
        self.changed_rows.is_empty() && !self.reordered && self.appended == 0
    }
}

impl ThreadList {
    pub fn new(state: WorkflowState, now: Timestamp) -> Self {
        Self {
            state,
            accounts: Vec::new(),
            scope: iris_store::Scope::Queue,
            rows: Vec::new(),
            cursor: None,
            exhausted: false,
            page_size: PAGE_SIZE,
            total: 0,
            now,
        }
    }

    pub fn with_page_size(mut self, size: u32) -> Self {
        self.page_size = size.max(1);
        self
    }

    pub fn state(&self) -> WorkflowState {
        self.state
    }

    /// Nombre de lignes actuellement chargées.
    pub fn loaded(&self) -> usize {
        self.rows.len()
    }

    /// Nombre total de fils dans cet état, connu par comptage.
    pub fn total(&self) -> u32 {
        self.total
    }

    pub fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    pub fn row(&self, index: usize) -> Option<&ThreadRow> {
        self.rows.get(index)
    }

    pub fn rows(&self) -> &[ThreadRow] {
        &self.rows
    }

    pub fn index_of(&self, thread: ThreadId) -> Option<usize> {
        self.rows.iter().position(|r| r.id == thread)
    }

    /// Restreint la liste à certains comptes. Une liste vide signifie « tous ».
    pub fn set_accounts(&mut self, accounts: Vec<AccountId>) -> bool {
        if self.accounts == accounts {
            return false;
        }
        self.accounts = accounts;
        true
    }

    /// La portée de départ, à la construction.
    pub fn in_scope(mut self, scope: iris_store::Scope) -> Self {
        self.scope = scope;
        self
    }

    pub fn set_scope(&mut self, scope: iris_store::Scope) -> bool {
        if self.scope == scope {
            return false;
        }
        self.scope = scope;
        true
    }

    pub fn set_now(&mut self, now: Timestamp) {
        self.now = now;
    }

    fn query(&self) -> ListQuery {
        let mut base = ListQuery::new(self.state, self.page_size)
            .for_accounts(self.accounts.clone())
            .hiding_snoozed(self.now);
        base.scope = self.scope.clone();
        base
    }

    /// Charge ce qu'il faut pour que l'indice demandé soit disponible.
    ///
    /// Charge en plus une marge de préchargement : sans elle, chaque frame de
    /// défilement déclencherait une requête, et l'utilisateur verrait la liste
    /// bégayer en atteignant le bas.
    pub fn ensure_loaded(&mut self, store: &Store, index: usize) -> Result<usize> {
        let cible = index + PREFETCH;
        let mut charges = 0;

        while self.rows.len() <= cible && !self.exhausted {
            charges += self.load_page(store)?;
        }
        Ok(charges)
    }

    /// Charge la page suivante.
    fn load_page(&mut self, store: &Store) -> Result<usize> {
        if self.exhausted {
            return Ok(0);
        }

        let mut requete = self.query();
        if let Some(c) = self.cursor {
            requete = requete.after(c);
        }

        let page = store.list_threads(&requete)?;
        if page.len() < self.page_size as usize {
            self.exhausted = true;
        }
        if let Some(dernier) = page.last() {
            self.cursor = Some(dernier.cursor());
        }

        let n = page.len();
        self.rows.extend(page);
        Ok(n)
    }

    /// Recharge la liste depuis le début.
    ///
    /// Le préfixe déjà chargé est reconstitué à la même longueur : sans cela, un
    /// simple changement d'état ferait remonter la vue en haut de la liste.
    pub fn reload(&mut self, store: &Store) -> Result<()> {
        let longueur = self.rows.len().max(self.page_size as usize);
        self.rows.clear();
        self.cursor = None;
        self.exhausted = false;

        while self.rows.len() < longueur && !self.exhausted {
            self.load_page(store)?;
        }
        self.refresh_total(store)?;
        Ok(())
    }

    pub fn refresh_total(&mut self, store: &Store) -> Result<()> {
        // Dans un dossier, le total des files ne veut rien dire : la barre de
        // défilement décrirait une liste qui n'est pas celle qu'on regarde. On compte
        // alors ce qui est chargé, et la liste s'allonge en défilant.
        if self.scope != iris_store::Scope::Queue {
            self.total = self.rows.len() as u32;
            return Ok(());
        }
        let counts = store.state_counts(&self.accounts, Some(self.now))?;
        self.total = counts[self.state.as_i64() as usize];
        Ok(())
    }

    /// Applique un lot de changements.
    ///
    /// Deux traitements bien distincts : un fil dont seul le contenu a changé est
    /// relu en place, ce qui ne coûte qu'une lecture d'index ; un changement d'ordre
    /// ou de composition impose de recharger. Confondre les deux ferait recharger la
    /// liste entière à chaque message marqué comme lu.
    pub fn apply(
        &mut self,
        store: &Store,
        full_refresh: bool,
        touched_threads: &BTreeSet<ThreadId>,
        reordered_states: &BTreeSet<WorkflowState>,
    ) -> Result<ListUpdate> {
        let mut update = ListUpdate::default();

        if full_refresh || reordered_states.contains(&self.state) {
            self.reload(store)?;
            update.reordered = true;
            return Ok(update);
        }

        for thread in touched_threads {
            let Some(index) = self.index_of(*thread) else {
                continue;
            };
            match store.thread_row(*thread)? {
                Some(ligne) => {
                    // Un fil qui a changé d'état n'appartient plus à cette liste.
                    if ligne.state != self.state {
                        self.reload(store)?;
                        update.reordered = true;
                        return Ok(update);
                    }
                    self.rows[index] = ligne;
                    update.changed_rows.push(index);
                }
                None => {
                    // Le fil a disparu : la composition change.
                    self.reload(store)?;
                    update.reordered = true;
                    return Ok(update);
                }
            }
        }

        if !update.changed_rows.is_empty() {
            self.refresh_total(store)?;
        }
        Ok(update)
    }

    /// Libère les lignes au-delà d'une limite.
    ///
    /// Une session longue peut faire défiler des dizaines de milliers de lignes ;
    /// les garder toutes contredirait l'invariant de mémoire. On tronque par le bas,
    /// jamais par le haut, pour ne pas invalider les indices déjà affichés.
    pub fn trim(&mut self, keep: usize) -> usize {
        if self.rows.len() <= keep {
            return 0;
        }
        let retirees = self.rows.len() - keep;
        self.rows.truncate(keep);
        self.cursor = self.rows.last().map(ThreadRow::cursor);
        // On peut de nouveau charger la suite.
        self.exhausted = false;
        retirees
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_store::{FolderRole, NewAccount, NewMessage};
    use iris_types::{Flags, FolderId};

    struct Fixture {
        store: Store,
        account: AccountId,
        folder: FolderId,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
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
                    preview: "aperçu".into(),
                })
                .unwrap()
                .thread
        }

        fn seed(&self, n: i64) -> Vec<ThreadId> {
            (0..n).map(|i| self.thread(1000 + i)).collect()
        }
    }

    fn liste() -> ThreadList {
        ThreadList::new(WorkflowState::Todo, Timestamp::from_millis(10_000)).with_page_size(10)
    }

    #[test]
    fn une_liste_neuve_est_vide() {
        let l = liste();
        assert_eq!(l.loaded(), 0);
        assert!(!l.is_exhausted());
        assert!(l.row(0).is_none());
    }

    #[test]
    fn le_chargement_couvre_l_indice_demande_et_la_marge() {
        let f = fixture();
        f.seed(200);
        let mut l = liste();

        l.ensure_loaded(&f.store, 0).unwrap();
        assert!(
            l.loaded() >= PREFETCH,
            "la marge de préchargement doit être couverte"
        );
        assert!(l.loaded() < 200, "mais pas toute la liste");
    }

    #[test]
    fn rien_n_est_charge_en_entier() {
        // L'invariant central : la mémoire suit ce qui est affiché.
        let f = fixture();
        f.seed(1000);
        let mut l = liste();
        l.ensure_loaded(&f.store, 5).unwrap();

        assert!(
            l.loaded() <= 60,
            "{} lignes chargées, c'est trop",
            l.loaded()
        );
    }

    #[test]
    fn le_defilement_etend_progressivement_la_fenetre() {
        let f = fixture();
        f.seed(200);
        let mut l = liste();

        l.ensure_loaded(&f.store, 0).unwrap();
        let premier = l.loaded();
        l.ensure_loaded(&f.store, 100).unwrap();

        assert!(l.loaded() > premier);
        assert!(l.row(100).is_some());
    }

    #[test]
    fn la_fin_de_liste_est_detectee() {
        let f = fixture();
        f.seed(15);
        let mut l = liste();

        l.ensure_loaded(&f.store, 100).unwrap();
        assert!(l.is_exhausted());
        assert_eq!(l.loaded(), 15);
        assert!(l.row(15).is_none());
    }

    #[test]
    fn les_lignes_sont_dans_l_ordre_de_la_liste() {
        let f = fixture();
        f.seed(5);
        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();

        let dates: Vec<i64> = l.rows().iter().map(|r| r.last_activity.millis()).collect();
        let mut trie = dates.clone();
        trie.sort_by(|a, b| b.cmp(a));
        assert_eq!(dates, trie, "du plus récent au plus ancien");
    }

    #[test]
    fn le_total_est_connu_meme_sans_tout_charger() {
        let f = fixture();
        f.seed(200);
        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();
        l.refresh_total(&f.store).unwrap();

        assert_eq!(l.total(), 200);
        assert!(l.loaded() < 200);
    }

    #[test]
    fn un_fil_modifie_est_relu_en_place() {
        // Marquer un message comme lu ne doit pas recharger la liste entière.
        let f = fixture();
        let fils = f.seed(20);
        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();
        let charges = l.loaded();

        let message = f.store.thread_messages(fils[5]).unwrap()[0].id;
        f.store.set_message_flags(message, Flags::SEEN).unwrap();

        let touches: BTreeSet<ThreadId> = [fils[5]].into_iter().collect();
        let update = l
            .apply(&f.store, false, &touches, &BTreeSet::new())
            .unwrap();

        assert!(!update.reordered, "aucun rechargement ne doit avoir lieu");
        assert_eq!(update.changed_rows.len(), 1);
        assert_eq!(l.loaded(), charges);
    }

    #[test]
    fn un_changement_d_ordre_recharge_la_liste() {
        let f = fixture();
        f.seed(20);
        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();

        let reordonnes: BTreeSet<WorkflowState> = [WorkflowState::Todo].into_iter().collect();
        let update = l
            .apply(&f.store, false, &BTreeSet::new(), &reordonnes)
            .unwrap();

        assert!(update.reordered);
    }

    #[test]
    fn un_reordonnancement_d_une_autre_file_ne_nous_concerne_pas() {
        let f = fixture();
        f.seed(20);
        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();

        let autre: BTreeSet<WorkflowState> = [WorkflowState::Done].into_iter().collect();
        let update = l.apply(&f.store, false, &BTreeSet::new(), &autre).unwrap();

        assert!(update.is_empty());
    }

    #[test]
    fn un_fil_qui_change_d_etat_provoque_un_rechargement() {
        // Il n'appartient plus à cette liste : le laisser en place afficherait une
        // ligne qui ne devrait plus s'y trouver.
        let f = fixture();
        let fils = f.seed(20);
        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();

        f.store
            .set_thread_state(fils[3], WorkflowState::Done)
            .unwrap();
        let touches: BTreeSet<ThreadId> = [fils[3]].into_iter().collect();
        let update = l
            .apply(&f.store, false, &touches, &BTreeSet::new())
            .unwrap();

        assert!(update.reordered);
        assert!(l.index_of(fils[3]).is_none());
    }

    #[test]
    fn un_fil_disparu_provoque_un_rechargement() {
        let f = fixture();
        f.seed(20);
        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();
        let premier = l.row(0).unwrap().id;

        f.store.delete_messages_by_uid(f.folder, &[20]).unwrap();
        let touches: BTreeSet<ThreadId> = [premier].into_iter().collect();
        let update = l
            .apply(&f.store, false, &touches, &BTreeSet::new())
            .unwrap();

        assert!(update.reordered);
    }

    #[test]
    fn un_rafraichissement_complet_recharge() {
        let f = fixture();
        f.seed(20);
        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();

        let update = l
            .apply(&f.store, true, &BTreeSet::new(), &BTreeSet::new())
            .unwrap();
        assert!(update.reordered);
        assert!(l.loaded() > 0);
    }

    #[test]
    fn le_rechargement_conserve_la_profondeur_atteinte() {
        // Sans cela, un simple changement d'état ferait remonter la vue en haut.
        let f = fixture();
        f.seed(200);
        let mut l = liste();
        l.ensure_loaded(&f.store, 100).unwrap();
        let profondeur = l.loaded();

        l.reload(&f.store).unwrap();
        assert!(l.loaded() >= profondeur, "{} < {profondeur}", l.loaded());
    }

    #[test]
    fn les_fils_reportes_sont_masques() {
        let f = fixture();
        let fils = f.seed(5);
        f.store
            .snooze_thread(
                fils[0],
                iris_types::Snooze {
                    until: Timestamp::from_millis(999_999),
                    restore_to: WorkflowState::Todo,
                },
            )
            .unwrap();

        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();
        assert_eq!(l.loaded(), 4);
        assert!(l.index_of(fils[0]).is_none());
    }

    #[test]
    fn le_filtre_par_compte_signale_un_changement() {
        let mut l = liste();
        assert!(l.set_accounts(vec![AccountId(1)]));
        assert!(
            !l.set_accounts(vec![AccountId(1)]),
            "même filtre, aucun changement"
        );
        assert!(l.set_accounts(vec![]));
    }

    #[test]
    fn le_filtre_par_compte_restreint_apres_rechargement() {
        let f = fixture();
        f.seed(5);
        let autre = f
            .store
            .create_account(
                &NewAccount::new("b@x.fr", "i", "s"),
                Timestamp::from_millis(0),
            )
            .unwrap();

        let mut l = liste();
        l.ensure_loaded(&f.store, 0).unwrap();
        assert_eq!(l.loaded(), 5);

        l.set_accounts(vec![autre]);
        l.reload(&f.store).unwrap();
        assert_eq!(l.loaded(), 0, "l'autre compte n'a aucun fil");
    }

    #[test]
    fn la_troncature_libere_la_memoire_et_permet_de_recharger() {
        let f = fixture();
        f.seed(200);
        let mut l = liste();
        l.ensure_loaded(&f.store, 150).unwrap();
        let avant = l.loaded();

        let retirees = l.trim(50);
        assert_eq!(retirees, avant - 50);
        assert_eq!(l.loaded(), 50);

        // Le curseur suit : la suite se recharge sans trou ni doublon.
        l.ensure_loaded(&f.store, 100).unwrap();
        let identifiants: BTreeSet<ThreadId> = l.rows().iter().map(|r| r.id).collect();
        assert_eq!(
            identifiants.len(),
            l.loaded(),
            "aucun doublon après rechargement"
        );
    }

    #[test]
    fn la_troncature_ne_fait_rien_si_la_liste_est_deja_courte() {
        let mut l = liste();
        assert_eq!(l.trim(100), 0);
    }
}
