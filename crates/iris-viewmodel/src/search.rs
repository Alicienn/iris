//! La recherche, vue depuis l'interface.
//!
//! Le langage et la planification vivent dans `iris-search` ; ce module les exécute
//! et rend des lignes prêtes à afficher, exactement comme la liste ordinaire. C'est
//! ce qui permet à la colonne du milieu de basculer en résultats sans rien changer à
//! sa manière de dessiner.
//!
//! Deux chemins, selon ce que le planificateur a décidé :
//!
//! - une requête **purement structurelle** — « les non-lus », « en attente depuis
//!   trente jours » — ne touche pas l'index : elle balaie les listes du store, qui
//!   sait déjà filtrer par état et par compte ;
//! - une requête **textuelle** passe par l'index, qui rend des fils, puis chaque fil
//!   est relu pour obtenir sa ligne et vérifié contre les filtres restants.
//!
//! Dans les deux cas le résultat est **borné** : une recherche qui ramènerait dix
//! mille lignes ne rendrait service à personne, et l'utilisateur affine.

use iris_index::SearchIndex;
use iris_search::{matches_filters, plan, Filter, Plan, Query, Subject};
use iris_store::{ListQuery, Store, ThreadRow};
use iris_types::{AccountId, Result, Timestamp, WorkflowState};
use std::sync::Arc;

/// Nombre maximal de résultats rendus.
pub const MAX_RESULTS: usize = 200;

/// Une recherche en cours.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    /// La requête telle que saisie.
    pub query: String,
    pub results: Vec<ThreadRow>,
    /// La recherche a atteint la borne : il y a probablement davantage.
    pub truncated: bool,
    /// Ce que le planificateur a compris, reformulé pour l'utilisateur.
    pub explanation: String,
    /// L'index a-t-il été sollicité ?
    pub used_index: bool,
}

impl SearchState {
    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }

    pub fn len(&self) -> usize {
        self.results.len()
    }

    /// Résumé affichable au-dessus des résultats.
    pub fn summary(&self) -> String {
        match (self.results.len(), self.truncated) {
            (0, _) => "Aucun résultat.".to_string(),
            (1, false) => "1 conversation.".to_string(),
            (n, false) => format!("{n} conversations."),
            (n, true) => format!("Plus de {n} conversations : affinez la recherche."),
        }
    }
}

/// Exécute une recherche.
pub fn run(
    store: &Store,
    index: Option<&Arc<SearchIndex>>,
    query: &str,
    accounts: &[AccountId],
    now: Timestamp,
) -> Result<SearchState> {
    // Newest first, like the lists, unless asked for the best matches first
    // (`sort:relevance`): read by date, the results fall into days (Today, Yesterday…)
    // and are read as a list; by relevance, the index's order is kept.
    let par_pertinence = query
        .split_whitespace()
        .any(|m| m.eq_ignore_ascii_case("sort:relevance"));
    let sans_tri: String = query
        .split_whitespace()
        .filter(|m| !m.eq_ignore_ascii_case("sort:relevance"))
        .collect::<Vec<_>>()
        .join(" ");
    let analysee = iris_search::parse(&sans_tri);
    if analysee.is_empty() {
        return Ok(SearchState::default());
    }

    let plan = plan(&analysee);
    let mut etat = SearchState {
        query: query.to_string(),
        explanation: analysee.describe(),
        used_index: !plan.store_only,
        ..Default::default()
    };

    let mut lignes = if plan.store_only {
        scan_store(store, &plan, accounts, now)?
    } else {
        match index {
            Some(index) => from_index(store, index, &plan, accounts, now)?,
            // Sans index, une recherche textuelle ne peut pas aboutir. Le dire vaut
            // mieux que rendre une liste vide, que l'utilisateur lirait comme
            // « aucun message ne correspond ».
            None => {
                etat.explanation =
                    "la recherche plein texte est indisponible : index absent".into();
                Vec::new()
            }
        }
    };

    if !par_pertinence {
        lignes.sort_by_key(|r| std::cmp::Reverse(r.last_activity.millis()));
    }
    etat.truncated = lignes.len() >= MAX_RESULTS;
    etat.results = lignes;
    Ok(etat)
}

/// Recherche purement structurelle : on balaie les listes du store.
fn scan_store(
    store: &Store,
    plan: &Plan,
    accounts: &[AccountId],
    now: Timestamp,
) -> Result<Vec<ThreadRow>> {
    // Un filtre d'état restreint les files à parcourir ; sans lui, les trois y
    // passent.
    let etats: Vec<WorkflowState> = plan
        .store_filters
        .iter()
        .find_map(|(f, negated)| match f {
            Filter::State(s) if !negated => Some(vec![*s]),
            _ => None,
        })
        .unwrap_or_else(|| WorkflowState::ALL.to_vec());

    let mut out = Vec::new();
    for etat in etats {
        let mut requete = ListQuery::new(etat, 200).for_accounts(accounts.to_vec());
        loop {
            let page = store.list_threads(&requete)?;
            let fin = page.len() < 200;

            for ligne in &page {
                if out.len() >= MAX_RESULTS {
                    return Ok(out);
                }
                if row_matches(store, ligne, plan, now)? {
                    out.push(ligne.clone());
                }
            }

            match page.last() {
                Some(dernier) if !fin => requete = requete.after(dernier.cursor()),
                _ => break,
            }
        }
    }

    out.sort_by_key(|r| std::cmp::Reverse(r.last_activity.millis()));
    Ok(out)
}

/// Recherche textuelle : l'index rend des fils, le store rend leurs lignes.
fn from_index(
    store: &Store,
    index: &Arc<SearchIndex>,
    plan: &Plan,
    accounts: &[AccountId],
    now: Timestamp,
) -> Result<Vec<ThreadRow>> {
    let hits = index.search_filtered(&plan.index_query, MAX_RESULTS, accounts)?;

    let mut out = Vec::with_capacity(hits.len());
    for hit in hits {
        // Un fil présent dans l'index mais absent du store a été supprimé entre
        // l'indexation et maintenant : l'index rattrapera, la liste ne doit pas
        // afficher un fantôme.
        let Some(ligne) = store.thread_row(hit.thread)? else {
            continue;
        };
        // Thrown away (put in the bin here, or all in a bin or spam folder): not a
        // result, as it is not in the queues. A conversation deleted from the results
        // stayed in them, and the delete looked as if it had done nothing.
        if store.thread_is_binned(ligne.id)? {
            continue;
        }
        if row_matches(store, &ligne, plan, now)? {
            out.push(ligne);
        }
    }
    // L'ordre de pertinence de l'index est conservé : c'est ce que l'utilisateur
    // attend d'une recherche, à la différence d'une liste.
    Ok(out)
}

/// Vérifie une ligne contre les filtres structurels.
fn row_matches(store: &Store, row: &ThreadRow, plan: &Plan, now: Timestamp) -> Result<bool> {
    if plan.store_filters.is_empty() {
        return Ok(true);
    }

    // Le nom du compte n'est chargé que si un filtre le demande : sur deux cents
    // lignes, une requête par ligne se verrait.
    let besoin_compte = plan
        .store_filters
        .iter()
        .any(|(f, _)| matches!(f, Filter::Account(_)));

    let adresse = if besoin_compte {
        store
            .thread_accounts(row.id)?
            .first()
            .and_then(|a| store.account(*a).ok().flatten())
            .map(|c| c.email)
            .unwrap_or_default()
    } else {
        String::new()
    };

    Ok(matches_filters(
        &plan.store_filters,
        Subject {
            flags: row.flags_union,
            state: row.state,
            received: row.last_activity,
            // La taille du fil n'est pas connue de la ligne ; un filtre de taille
            // porte sur un message, et ne s'applique donc pas ici.
            size: 0,
            account: &adresse,
            folder: "",
            snoozed: row.snoozed_until.is_some(),
        },
        now,
    ))
}

/// Reformule une requête pour l'afficher.
pub fn describe(query: &str) -> String {
    let analysee: Query = iris_search::parse(query);
    analysee.describe()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_index::IndexedMessage;
    use iris_store::{FolderRole, NewAccount, NewMessage};
    use iris_types::{Flags, FolderId, MessageId, ThreadId};

    struct Fixture {
        store: Store,
        index: Arc<SearchIndex>,
        account: AccountId,
        folder: FolderId,
        uid: std::cell::Cell<u32>,
    }

    fn fixture() -> Fixture {
        let store = Store::in_memory().unwrap();
        let account = store
            .create_account(
                &NewAccount::new("moi@example.com", "i", "s"),
                Timestamp::EPOCH,
            )
            .unwrap();
        let folder = store
            .upsert_folder(account, "INBOX", FolderRole::Inbox)
            .unwrap();
        Fixture {
            store,
            index: Arc::new(SearchIndex::in_memory().unwrap()),
            account,
            folder,
            uid: std::cell::Cell::new(1),
        }
    }

    impl Fixture {
        /// Crée un fil et l'indexe, comme le ferait la synchronisation.
        fn message(&self, sujet: &str, expediteur: &str, corps: &str, flags: Flags) -> ThreadId {
            let uid = self.uid.get();
            self.uid.set(uid + 1);

            let insere = self
                .store
                .insert_message(&NewMessage {
                    account: self.account,
                    folder: self.folder,
                    uid,
                    rfc_message_id: Some(format!("m{uid}@x")),
                    in_reply_to: None,
                    references: vec![],
                    subject: sujet.into(),
                    from_name: expediteur.into(),
                    from_addr: format!("{}@example.com", expediteur.to_lowercase()),
                    recipients_json: "[]".into(),
                    date: Timestamp::from_millis(1000 * uid as i64),
                    received: Timestamp::from_millis(1000 * uid as i64),
                    size: 1024,
                    flags,
                    preview: corps.into(),
                })
                .unwrap();

            self.index
                .add(&IndexedMessage {
                    message: insere.message,
                    thread: insere.thread,
                    account: self.account,
                    subject: sujet.into(),
                    from: expediteur.into(),
                    recipients: "moi@example.com".into(),
                    body: corps.into(),
                    received: Timestamp::from_millis(1000 * uid as i64),
                    has_attachment: flags.contains(Flags::HAS_ATTACHMENT),
                })
                .unwrap();
            self.index.commit().unwrap();

            insere.thread
        }

        fn chercher(&self, requete: &str) -> SearchState {
            run(&self.store, Some(&self.index), requete, &[], now()).unwrap()
        }
    }

    fn now() -> Timestamp {
        Timestamp::from_millis(10_000_000)
    }

    #[test]
    fn une_requete_vide_ne_cherche_rien() {
        let f = fixture();
        f.message("Devis", "Marie", "Bonjour", Flags::NONE);

        let etat = f.chercher("   ");
        assert!(etat.is_empty());
        assert_eq!(etat.summary(), "Aucun résultat.");
    }

    #[test]
    fn une_recherche_textuelle_trouve_par_le_sujet() {
        let f = fixture();
        f.message("Devis refonte", "Marie", "Le montant proposé", Flags::NONE);
        f.message("Facture mars", "Luc", "Paiement reçu", Flags::NONE);

        let etat = f.chercher("refonte");
        assert_eq!(etat.len(), 1);
        assert!(etat.used_index);
        assert_eq!(etat.summary(), "1 conversation.");
    }

    #[test]
    fn une_recherche_textuelle_trouve_par_le_corps() {
        let f = fixture();
        f.message(
            "Sujet neutre",
            "Marie",
            "La formule du forfait annuel",
            Flags::NONE,
        );

        assert_eq!(f.chercher("forfait").len(), 1);
    }

    #[test]
    fn une_recherche_structurelle_evite_l_index() {
        // « les non-lus » n'a rien à faire dans un moteur plein texte.
        let f = fixture();
        f.message("Un", "Marie", "x", Flags::NONE);
        f.message("Deux", "Luc", "y", Flags::SEEN);

        let etat = f.chercher("is:unread");
        assert!(!etat.used_index);
        assert_eq!(etat.len(), 1);
    }

    #[test]
    fn les_filtres_se_combinent_avec_le_texte() {
        let f = fixture();
        f.message("Devis refonte", "Marie", "montant", Flags::NONE);
        f.message("Devis refonte", "Luc", "montant", Flags::SEEN);

        let etat = f.chercher("devis is:unread");
        assert_eq!(etat.len(), 1, "seul le non-lu doit sortir");
    }

    #[test]
    fn un_filtre_d_etat_restreint_les_files_parcourues() {
        let f = fixture();
        let a = f.message("Un", "Marie", "x", Flags::NONE);
        f.message("Deux", "Luc", "y", Flags::NONE);
        f.store.set_thread_state(a, WorkflowState::Done).unwrap();

        assert_eq!(f.chercher("etat:traite").len(), 1);
        assert_eq!(f.chercher("etat:a_traiter").len(), 1);
    }

    #[test]
    fn une_recherche_sans_correspondance_le_dit() {
        let f = fixture();
        f.message("Devis", "Marie", "x", Flags::NONE);

        let etat = f.chercher("zzzzinexistant");
        assert!(etat.is_empty());
        assert_eq!(etat.summary(), "Aucun résultat.");
    }

    #[test]
    fn la_requete_est_reformulee_pour_l_utilisateur() {
        let f = fixture();
        let etat = f.chercher("de:marie is:unread devis");
        assert!(etat.explanation.contains("de « marie »"));
        assert!(etat.explanation.contains("non lu"));
    }

    #[test]
    fn les_resultats_sont_bornes() {
        // Dix mille lignes ne rendraient service à personne.
        let f = fixture();
        for i in 0..(MAX_RESULTS + 50) {
            f.message(&format!("Devis {i}"), "Marie", "montant", Flags::NONE);
        }

        let etat = f.chercher("is:unread");
        assert_eq!(etat.len(), MAX_RESULTS);
        assert!(etat.truncated);
        assert!(etat.summary().contains("affinez"));
    }

    #[test]
    fn un_fil_supprime_apres_indexation_ne_devient_pas_un_fantome() {
        let f = fixture();
        f.message("Devis refonte", "Marie", "montant", Flags::NONE);
        assert_eq!(f.chercher("refonte").len(), 1);

        // Le message disparaît, mais l'index ne le sait pas encore.
        f.store.delete_messages_by_uid(f.folder, &[1]).unwrap();
        assert_eq!(f.chercher("refonte").len(), 0);
    }

    #[test]
    fn sans_index_une_recherche_textuelle_le_dit() {
        // Rendre une liste vide se lirait comme « aucun message ne correspond ».
        let f = fixture();
        f.message("Devis", "Marie", "x", Flags::NONE);

        let etat = run(&f.store, None, "devis", &[], now()).unwrap();
        assert!(etat.is_empty());
        assert!(etat.explanation.contains("indisponible"));
    }

    #[test]
    fn sans_index_une_recherche_structurelle_fonctionne_quand_meme() {
        let f = fixture();
        f.message("Devis", "Marie", "x", Flags::NONE);

        let etat = run(&f.store, None, "is:unread", &[], now()).unwrap();
        assert_eq!(etat.len(), 1);
    }

    #[test]
    fn le_filtre_par_compte_est_evalue() {
        let f = fixture();
        f.message("Devis", "Marie", "x", Flags::NONE);

        assert_eq!(f.chercher("compte:moi@example.com is:unread").len(), 1);
        assert_eq!(f.chercher("compte:autre@example.com is:unread").len(), 0);
    }

    #[test]
    fn un_fil_reporte_est_trouvable_par_son_etat() {
        let f = fixture();
        let fil = f.message("Devis", "Marie", "x", Flags::NONE);
        f.store
            .snooze_thread(
                fil,
                iris_types::Snooze {
                    until: Timestamp::from_millis(99_000_000),
                    restore_to: WorkflowState::Todo,
                },
            )
            .unwrap();

        assert_eq!(f.chercher("is:snoozed").len(), 1);
    }

    #[test]
    fn la_reformulation_est_accessible_sans_executer() {
        assert!(describe("de:marie").contains("de « marie »"));
        assert_eq!(describe(""), "recherche vide");
    }

    #[test]
    fn un_message_indexe_sans_ligne_ne_fait_pas_echouer() {
        let f = fixture();
        f.index
            .add(&IndexedMessage {
                message: MessageId(999),
                thread: ThreadId(999),
                account: f.account,
                subject: "Fantôme".into(),
                from: "personne".into(),
                recipients: String::new(),
                body: "introuvable".into(),
                received: Timestamp::EPOCH,
                has_attachment: false,
            })
            .unwrap();
        f.index.commit().unwrap();

        assert!(f.chercher("fantôme").is_empty());
    }
}
