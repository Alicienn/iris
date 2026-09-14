//! Le plugin d'exemple, de bout en bout.
//!
//! Les tests unitaires vérifient chaque maillon ; celui-ci vérifie qu'ils sont
//! attachés. Il installe le plugin livré avec le projet — compilé depuis son
//! WebAssembly textuel, exactement comme un plugin distribué — lui envoie un vrai
//! événement issu d'un vrai message du store, et regarde le fil changer d'état.
//!
//! C'est le seul test qui puisse dire « les plugins tournent ». Sans lui, chaque
//! moitié de la chaîne pourrait être correcte sans que rien ne se passe.

use iris_app::controller::{Controller, Request, Snapshot};
use iris_app::plugins::{apply_effect, PluginEffect, PluginService};
use iris_kernel::Event;
use iris_store::{FolderRole, NewAccount, NewMessage, Store};
use iris_types::{Flags, Timestamp, WorkflowState};
use std::sync::mpsc;
use std::sync::Arc;

/// Installe le plugin d'exemple dans un répertoire temporaire.
fn installer_l_exemple(racine: &std::path::Path) {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../iris-plugins/examples/marquer-infolettres");

    let dossier = racine.join("marquer-infolettres");
    std::fs::create_dir_all(&dossier).unwrap();
    std::fs::copy(source.join("plugin.toml"), dossier.join("plugin.toml")).unwrap();

    let wat = std::fs::read_to_string(source.join("plugin.wat")).unwrap();
    let wasm = wat::parse_str(&wat).expect("le plugin d'exemple doit être du WebAssembly valide");
    std::fs::write(dossier.join("plugin.wasm"), wasm).unwrap();
}

struct Fixture {
    store: Arc<Store>,
    account: iris_types::AccountId,
    folder: iris_types::FolderId,
    uid: std::cell::Cell<u32>,
}

fn fixture() -> Fixture {
    let store = Arc::new(Store::in_memory().unwrap());
    let account = store
        .create_account(&NewAccount::new("a@x.fr", "i", "s"), Timestamp::EPOCH)
        .unwrap();
    let folder = store.upsert_folder(account, "INBOX", FolderRole::Inbox).unwrap();
    Fixture { store, account, folder, uid: std::cell::Cell::new(1) }
}

impl Fixture {
    fn message(&self, sujet: &str, flags: Flags) -> (iris_types::MessageId, iris_types::ThreadId) {
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
                from_name: "Boutique".into(),
                from_addr: "info@boutique.fr".into(),
                recipients_json: "[]".into(),
                date: Timestamp::EPOCH,
                received: Timestamp::EPOCH,
                size: 100,
                flags,
                preview: String::new(),
            })
            .unwrap();
        (insere.message, insere.thread)
    }

    fn arrivee(&self, id: iris_types::MessageId) -> Event {
        Event::MessagesAdded {
            account: self.account,
            folder: self.folder,
            ids: Arc::from(vec![id]),
        }
    }

    fn etat(&self, fil: iris_types::ThreadId) -> WorkflowState {
        self.store.thread_row(fil).unwrap().unwrap().state
    }
}

/// Attend un effet, ou échoue au bout de cinq secondes.
fn attendre(rx: &mpsc::Receiver<PluginEffect>) -> PluginEffect {
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("un effet de plugin")
}

#[test]
fn le_plugin_d_exemple_marque_une_infolettre_comme_traitee() {
    let dir = tempfile::tempdir().unwrap();
    installer_l_exemple(dir.path());

    let f = fixture();
    let (message, fil) = f.message("Offre du mois", Flags::UNSUBSCRIBABLE);

    let (tx, rx) = mpsc::channel();
    let (service, thread) = PluginService::spawn(dir.path(), Arc::clone(&f.store), move |e| {
        let _ = tx.send(e);
    });
    assert_eq!(service.report().loaded, ["marquer-infolettres"], "le plugin doit charger");
    let thread = thread.expect("un fil de plugins");

    service.notify(&f.arrivee(message), &f.store);

    match attendre(&rx) {
        PluginEffect::Act { plugin, thread: cible, action } => {
            assert_eq!(plugin, "marquer-infolettres");
            assert_eq!(cible, fil);
            assert_eq!(action, iris_viewmodel::Action::Done);
        }
        autre => panic!("attendu une action, obtenu {autre:?}"),
    }

    service.shutdown();
    thread.join().unwrap();
}

#[test]
fn un_message_ordinaire_laisse_le_plugin_indifferent() {
    // Un plugin qui agit sur tout serait un plugin qu'on désinstalle.
    let dir = tempfile::tempdir().unwrap();
    installer_l_exemple(dir.path());

    let f = fixture();
    let (message, _) = f.message("Devis pour la refonte", Flags::NONE);

    let (tx, rx) = mpsc::channel();
    let (service, thread) = PluginService::spawn(dir.path(), Arc::clone(&f.store), move |e| {
        let _ = tx.send(e);
    });
    let thread = thread.expect("un fil de plugins");

    service.notify(&f.arrivee(message), &f.store);
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(500)).is_err(),
        "aucune action ne doit être demandée"
    );

    service.shutdown();
    thread.join().unwrap();
}

#[test]
fn l_action_du_plugin_change_reellement_l_etat_du_fil() {
    // La chaîne complète : événement, plugin, effet, contrôleur, base.
    let dir = tempfile::tempdir().unwrap();
    installer_l_exemple(dir.path());

    let f = fixture();
    let (message, fil) = f.message("Notre infolettre", Flags::UNSUBSCRIBABLE);
    assert_eq!(f.etat(fil), WorkflowState::Todo);

    let (tx_snap, rx_snap) = mpsc::channel::<Snapshot>();
    let (controller, fil_vm) = Controller::spawn(
        Arc::clone(&f.store),
        Default::default(),
        Timestamp::EPOCH,
        move |s| {
            let _ = tx_snap.send(s);
        },
    );
    let controller = Arc::new(controller);
    controller.send(Request::Bootstrap);
    rx_snap.recv_timeout(std::time::Duration::from_secs(5)).unwrap();

    let controller_effets = Arc::clone(&controller);
    let (service, fil_plugins) =
        PluginService::spawn(dir.path(), Arc::clone(&f.store), move |effet| {
            apply_effect(&effet, &controller_effets);
        });
    let fil_plugins = fil_plugins.expect("un fil de plugins");

    service.notify(&f.arrivee(message), &f.store);

    // On attend l'instantané qui montre la file vidée : c'est la preuve que
    // l'action est allée jusqu'à la base et est remontée jusqu'à la vue.
    let echeance = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let restant = echeance.saturating_duration_since(std::time::Instant::now());
        let s = rx_snap.recv_timeout(restant).expect("un instantané après l'action du plugin");
        if s.rows.is_empty() {
            break;
        }
    }

    assert_eq!(f.etat(fil), WorkflowState::Done, "le plugin a bien trié le fil");

    service.shutdown();
    fil_plugins.join().unwrap();
    controller.shutdown();
    fil_vm.join().unwrap();
}

#[test]
fn un_plugin_ne_bloque_pas_le_fil_qui_le_sollicite() {
    // L'appel est asynchrone par construction : `notify` rend la main tout de suite,
    // même si le plugin travaille. C'est ce qui protège l'affichage.
    let dir = tempfile::tempdir().unwrap();
    installer_l_exemple(dir.path());

    let f = fixture();
    let (message, _) = f.message("Infolettre", Flags::UNSUBSCRIBABLE);

    let (tx, rx) = mpsc::channel();
    let (service, thread) = PluginService::spawn(dir.path(), Arc::clone(&f.store), move |e| {
        let _ = tx.send(e);
    });
    let thread = thread.expect("un fil de plugins");

    let depart = std::time::Instant::now();
    for _ in 0..50 {
        service.notify(&f.arrivee(message), &f.store);
    }
    let ecoule = depart.elapsed();

    assert!(
        ecoule < std::time::Duration::from_millis(100),
        "cinquante notifications ont pris {ecoule:?} : l'appelant attend le plugin"
    );

    // Et le travail se fait quand même.
    assert!(matches!(attendre(&rx), PluginEffect::Act { .. }));

    service.shutdown();
    thread.join().unwrap();
}
