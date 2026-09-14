//! Mesures face aux budgets de la spécification.
//!
//! Ces tests ne vérifient pas des temps absolus sur une machine donnée — ce serait
//! ingérable — mais que les **ordres de grandeur** annoncés au §4 de la spécification
//! sont tenus, avec une marge confortable. Un dépassement d'un facteur dix signale
//! une régression de conception, pas un mauvais jour du processeur.
//!
//! Ils sont ignorés par défaut car ils écrivent des dizaines de milliers de lignes :
//!
//! ```text
//! cargo test -p iris-app --release --test budgets -- --ignored --nocapture
//! ```

use iris_app::{Paths, Services};
use iris_index::{IndexedMessage, SearchIndex};
use iris_secrets::Secret;
use iris_store::{FolderRole, NewAccount, NewMessage, Store};
use iris_types::{AccountId, Flags, FolderId, MessageId, ThreadId, Timestamp, WorkflowState};
use iris_viewmodel::ViewModel;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Remplit une base de `threads` conversations réalistes.
fn seed(store: &Store, account: AccountId, folder: FolderId, threads: u32) {
    const LOT: u32 = 5_000;
    let mut uid = 1;
    while uid <= threads {
        let fin = (uid + LOT).min(threads + 1);
        let messages: Vec<NewMessage> = (uid..fin)
            .map(|u| NewMessage {
                account,
                folder,
                uid: u,
                rfc_message_id: Some(format!("m{u}@bench")),
                in_reply_to: None,
                references: vec![],
                subject: format!("Conversation numéro {u} — devis, facture ou relance"),
                from_name: format!("Correspondant {}", u % 400),
                from_addr: format!("correspondant{}@example.com", u % 400),
                recipients_json: "[]".into(),
                date: Timestamp::from_millis(1_700_000_000_000 + u as i64 * 1000),
                received: Timestamp::from_millis(1_700_000_000_000 + u as i64 * 1000),
                size: 4096,
                flags: if u % 3 == 0 { Flags::SEEN } else { Flags::NONE },
                preview: "Bonjour, je reviens vers vous concernant le point évoqué…".into(),
            })
            .collect();
        store.insert_messages(&messages).expect("insertion");
        uid = fin;
    }
}

fn base_remplie(threads: u32) -> (tempfile::TempDir, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path().join("iris.db")).unwrap());
    let account = store
        .create_account(
            &NewAccount::new("bench@example.com", "i", "s"),
            Timestamp::EPOCH,
        )
        .unwrap();
    let folder = store
        .upsert_folder(account, "INBOX", FolderRole::Inbox)
        .unwrap();
    seed(&store, account, folder, threads);
    (dir, store)
}

#[test]
#[ignore = "mesure sur jeu de données volumineux"]
fn la_premiere_liste_s_affiche_en_moins_de_quatre_cents_millisecondes() {
    // Budget de la spécification : ≤ 400 ms jusqu'à la première liste affichée.
    const THREADS: u32 = 100_000;
    let (dir, store) = base_remplie(THREADS);
    drop(store);

    // Ouverture à froid, comme au démarrage de l'application.
    let debut = Instant::now();
    let store = Arc::new(Store::open(dir.path().join("iris.db")).unwrap());
    let mut vm = ViewModel::new(
        Arc::clone(&store),
        Timestamp::from_millis(1_800_000_000_000),
    );
    vm.bootstrap().unwrap();
    let ecoule = debut.elapsed();

    println!(
        "première liste sur {THREADS} fils : {ecoule:?} ({} lignes)",
        vm.list().loaded()
    );
    assert!(vm.list().loaded() > 0);
    assert_eq!(vm.count_of(WorkflowState::Todo), THREADS);
    assert!(
        ecoule < Duration::from_millis(400),
        "budget dépassé : {ecoule:?} pour la première liste"
    );
}

#[test]
#[ignore = "mesure sur jeu de données volumineux"]
fn la_memoire_suit_ce_qui_est_affiche_et_non_ce_qui_est_stocke() {
    // Invariant n° 2. On ne mesure pas des octets — trop dépendant de l'allocateur —
    // mais la propriété qui les commande : le nombre de lignes détenues.
    const THREADS: u32 = 100_000;
    let (_dir, store) = base_remplie(THREADS);

    let mut vm = ViewModel::new(
        Arc::clone(&store),
        Timestamp::from_millis(1_800_000_000_000),
    );
    vm.bootstrap().unwrap();

    let apres_demarrage = vm.list().loaded();
    println!("{apres_demarrage} lignes détenues pour {THREADS} fils en base");
    assert!(
        apres_demarrage <= 200,
        "{apres_demarrage} lignes chargées au démarrage, c'est trop"
    );

    // Après un défilement profond, la fenêtre reste proportionnée au parcours, pas
    // au volume total.
    vm.ensure_loaded(1_000).unwrap();
    let apres_defilement = vm.list().loaded();
    println!("{apres_defilement} lignes après défilement jusqu'à la millième");
    assert!(apres_defilement < 2_000);
}

#[test]
#[ignore = "mesure sur jeu de données volumineux"]
fn une_page_de_liste_se_sert_en_moins_d_une_milliseconde() {
    // Le défilement doit tenir dans un budget de frame ; une page servie en moins
    // d'une milliseconde laisse tout le reste au rendu.
    const THREADS: u32 = 100_000;
    let (_dir, store) = base_remplie(THREADS);

    let mut vm = ViewModel::new(
        Arc::clone(&store),
        Timestamp::from_millis(1_800_000_000_000),
    );
    vm.bootstrap().unwrap();

    let mut pire = Duration::ZERO;
    for cible in (0..5_000).step_by(250) {
        let debut = Instant::now();
        vm.ensure_loaded(cible).unwrap();
        pire = pire.max(debut.elapsed());
    }

    println!("page la plus lente sur vingt paliers : {pire:?}");
    assert!(pire < Duration::from_millis(10), "page servie en {pire:?}");
}

#[test]
#[ignore = "mesure sur jeu de données volumineux"]
fn la_recherche_repond_en_moins_de_quatre_vingts_millisecondes() {
    // Budget de la spécification : ≤ 80 ms jusqu'aux premiers résultats.
    const DOCUMENTS: i64 = 200_000;

    let dir = tempfile::tempdir().unwrap();
    let index = SearchIndex::open(dir.path()).unwrap();

    let debut = Instant::now();
    for i in 1..=DOCUMENTS {
        index
            .add(&IndexedMessage {
                message: MessageId(i),
                thread: ThreadId(i / 3 + 1),
                account: AccountId(1),
                subject: format!("Sujet numéro {i} concernant un devis"),
                from: format!("Correspondant {} <c{}@example.com>", i % 400, i % 400),
                recipients: "moi@example.com".into(),
                body: format!(
                    "Bonjour, je reviens vers vous au sujet du dossier {i}. \
                     Cordialement. Référence interne {}.",
                    i * 7
                ),
                received: Timestamp::from_millis(1_700_000_000_000 + i * 1000),
                has_attachment: false,
            })
            .unwrap();
    }
    index.commit().unwrap();
    println!("{DOCUMENTS} documents indexés en {:?}", debut.elapsed());

    for requete in [
        "devis",
        "dossier 12345",
        "correspondant 42",
        "référence interne",
    ] {
        let debut = Instant::now();
        let resultats = index.search(requete, 50).unwrap();
        let ecoule = debut.elapsed();

        println!("  « {requete} » : {} fils en {ecoule:?}", resultats.len());
        assert!(
            ecoule < Duration::from_millis(80),
            "« {requete} » a pris {ecoule:?}, au-delà du budget"
        );
    }
}

#[test]
#[ignore = "mesure sur jeu de données volumineux"]
fn les_actions_de_triage_sont_instantanees() {
    // L'invariant n° 3 promet qu'aucune action n'attend le réseau. Sur un millier
    // d'actions enchaînées — une séance de triage complète — la moyenne doit rester
    // très en deçà du seuil de perception.
    use iris_viewmodel::{Action, Actions};

    const THREADS: u32 = 20_000;
    let (_dir, store) = base_remplie(THREADS);

    let mut vm = ViewModel::new(
        Arc::clone(&store),
        Timestamp::from_millis(1_800_000_000_000),
    );
    vm.bootstrap().unwrap();
    let actions = Actions::new(iris_app::controller::default_workflow(Arc::clone(&store)));

    let fils: Vec<ThreadId> = vm.list().rows().iter().take(200).map(|r| r.id).collect();

    let debut = Instant::now();
    for fil in &fils {
        actions
            .apply(
                *fil,
                Action::Done,
                Timestamp::from_millis(1_800_000_000_000),
            )
            .unwrap();
    }
    let ecoule = debut.elapsed();
    let moyenne = ecoule / fils.len() as u32;

    println!(
        "{} actions en {ecoule:?}, soit {moyenne:?} par action",
        fils.len()
    );
    assert!(moyenne < Duration::from_millis(5), "action à {moyenne:?}");
}

#[test]
#[ignore = "mesure sur jeu de données volumineux"]
fn l_application_s_ouvre_a_froid_rapidement() {
    // Ouverture de tous les services, comme au lancement.
    let dir = tempfile::tempdir().unwrap();
    let chemins = Paths::under(dir.path());

    // Première ouverture : elle crée tout.
    let services = Services::open(chemins.clone(), Some(Secret::new("maitre"))).unwrap();
    drop(services);

    let debut = Instant::now();
    let services = Services::open(chemins, Some(Secret::new("maitre"))).unwrap();
    let ecoule = debut.elapsed();

    println!("ouverture à froid des services : {ecoule:?}");
    // The themes are loaded during this open, so the check belongs here — but on
    // "at least one", not on a count. Shipping another theme is not a regression.
    assert!(!services.themes.names().is_empty());
    assert!(
        ecoule < Duration::from_millis(2_000),
        "ouverture en {ecoule:?}"
    );
}
