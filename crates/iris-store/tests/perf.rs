//! Mesures sur jeu de données synthétique.
//!
//! Ces tests vérifient une **propriété de complexité**, pas un temps absolu : la
//! page la plus lointaine d'une liste doit coûter le même ordre de grandeur que la
//! première. C'est ce que garantit la pagination par curseur, et ce que `OFFSET`
//! rendrait impossible.
//!
//! Ils sont ignorés par défaut car ils écrivent des dizaines de milliers de lignes.
//! Pour les exécuter :
//!
//! ```text
//! cargo test -p iris-store --release -- --ignored --nocapture
//! ```

use iris_store::{Filters, FolderRole, ListQuery, NewAccount, NewMessage, Store};
use iris_types::{AccountId, Flags, FolderId, Timestamp, WorkflowState};
use std::time::Instant;

/// Remplit la base avec `threads` fils d'un message chacun.
fn seed(store: &Store, account: AccountId, folder: FolderId, threads: u32) {
    const BATCH: u32 = 5_000;
    let mut uid = 1u32;
    while uid <= threads {
        let end = (uid + BATCH).min(threads + 1);
        let lot: Vec<NewMessage> = (uid..end)
            .map(|u| NewMessage {
                account,
                folder,
                uid: u,
                rfc_message_id: Some(format!("m{u}@bench")),
                in_reply_to: None,
                references: vec![],
                subject: format!("Sujet numéro {u}"),
                from_name: format!("Expéditeur {}", u % 500),
                from_addr: format!("exp{}@example.com", u % 500),
                recipients_json: "[]".into(),
                date: Timestamp::from_millis(1_700_000_000_000 + u as i64 * 1000),
                received: Timestamp::from_millis(1_700_000_000_000 + u as i64 * 1000),
                size: 2048,
                flags: if u % 3 == 0 { Flags::SEEN } else { Flags::NONE },
                preview: "Un aperçu de contenu représentatif d'un message réel.".into(),
            })
            .collect();
        store.insert_messages(&lot).expect("insertion du lot");
        uid = end;
    }
}

fn fixture(threads: u32) -> (Store, Instant) {
    let store = Store::in_memory().expect("base");
    let account = store
        .create_account(
            &NewAccount::new("bench@example.com", "imap", "smtp"),
            Timestamp::from_millis(0),
        )
        .expect("compte");
    let folder = store
        .upsert_folder(account, "INBOX", FolderRole::Inbox)
        .expect("dossier");

    let start = Instant::now();
    seed(&store, account, folder, threads);
    (store, start)
}

#[test]
#[ignore = "jeu de données volumineux"]
fn la_derniere_page_coute_autant_que_la_premiere() {
    const THREADS: u32 = 100_000;
    const PAGE: u32 = 50;

    let (store, start) = fixture(THREADS);
    println!("{THREADS} fils insérés en {:?}", start.elapsed());
    assert_eq!(store.message_count().unwrap(), THREADS as u64);

    // Première page.
    let t0 = Instant::now();
    let premiere = store
        .list_threads(&ListQuery::new(WorkflowState::Todo, PAGE))
        .unwrap();
    let cout_premiere = t0.elapsed();
    assert_eq!(premiere.len() as u32, PAGE);

    // Descente jusqu'à la fin de la liste, page après page.
    let mut q = ListQuery::new(WorkflowState::Todo, PAGE);
    let mut pages = 0;
    let mut derniere_page_cout = std::time::Duration::ZERO;
    loop {
        let t = Instant::now();
        let page = store.list_threads(&q).unwrap();
        let cout = t.elapsed();
        if page.is_empty() {
            break;
        }
        derniere_page_cout = cout;
        q = q.after(page.last().unwrap().cursor());
        pages += 1;
    }

    println!(
        "première page : {cout_premiere:?} · dernière page ({pages}ᵉ) : {derniere_page_cout:?}"
    );
    assert_eq!(pages, (THREADS / PAGE) as usize);

    // La marge est large à dessein : on teste une complexité, pas une horloge.
    let plafond = cout_premiere.max(std::time::Duration::from_micros(200)) * 20;
    assert!(
        derniere_page_cout < plafond,
        "la dernière page ({derniere_page_cout:?}) doit rester du même ordre que la \
         première ({cout_premiere:?}) : la pagination par curseur ne doit pas dégrader"
    );
}

#[test]
#[ignore = "jeu de données volumineux"]
fn les_compteurs_restent_immediats_a_grande_echelle() {
    const THREADS: u32 = 100_000;
    let (store, _) = fixture(THREADS);

    let t = Instant::now();
    let counts = store.state_counts(&[], None, Filters::default()).unwrap();
    let cout = t.elapsed();

    println!("compteurs sur {THREADS} fils : {cout:?}");
    assert_eq!(counts[0], THREADS);
    assert!(
        cout < std::time::Duration::from_millis(150),
        "les compteurs d'onglets sont recalculés à chaque diff : ils doivent rester \
         immédiats, ici {cout:?}"
    );
}

#[test]
#[ignore = "jeu de données volumineux"]
fn le_filtre_par_compte_ne_degrade_pas_la_liste() {
    const THREADS: u32 = 50_000;
    let (store, _) = fixture(THREADS);
    let compte = store.accounts().unwrap()[0].id;

    let t = Instant::now();
    let page = store
        .list_threads(&ListQuery::new(WorkflowState::Todo, 50).for_accounts(vec![compte]))
        .unwrap();
    let cout = t.elapsed();

    println!("page filtrée par compte : {cout:?}");
    assert_eq!(page.len(), 50);
    assert!(cout < std::time::Duration::from_millis(50));
}

#[test]
#[ignore = "jeu de données volumineux"]
fn l_insertion_reste_lineaire() {
    // Test de non-régression sur un défaut réel : le rattachement d'une réponse
    // arrivée avant son original interroge `messages.in_reply_to`. Sans index sur
    // cette colonne, chaque insertion balaie toute la table et la synchronisation
    // devient quadratique — mesuré à 192 s pour 100 000 messages, contre moins d'une
    // seconde avec l'index.
    const PETIT: u32 = 10_000;
    const GRAND: u32 = 100_000;

    let (_, t_petit) = fixture(PETIT);
    let petit = t_petit.elapsed();
    let (_, t_grand) = fixture(GRAND);
    let grand = t_grand.elapsed();

    let facteur = grand.as_secs_f64() / petit.as_secs_f64().max(1e-6);
    println!(
        "{PETIT} : {petit:?} · {GRAND} : {grand:?} · facteur {facteur:.1}× pour 10× le volume"
    );

    // Linéaire donnerait 10, quadratique 100. Le seuil laisse de la marge pour le
    // bruit de mesure tout en restant loin d'une dégradation quadratique.
    assert!(
        facteur < 25.0,
        "l'insertion dégrade non linéairement ({facteur:.1}× pour 10× le volume) : \
         un index manque probablement sur un chemin de rattachement"
    );
}
