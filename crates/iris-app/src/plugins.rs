//! Les plugins, en fonctionnement.
//!
//! Le bac à sable, le manifeste et les permissions vivent dans `iris-plugins` ; ce
//! module les fait tourner dans l'application. Il tient trois promesses :
//!
//! - **un plugin ne bloque jamais l'affichage** : le registre vit dans son propre
//!   fil, et l'interface ne lui parle que par messages, exactement comme au
//!   vue-modèle. Un plugin qui part en boucle consomme son carburant, pas une frame ;
//! - **un plugin ne voit que ce qu'on lui montre** : les événements du bus sont
//!   traduits en charges JSON restreintes, jamais passés tels quels. Un plugin ne
//!   reçoit pas d'identifiant de compte qu'il pourrait recouper, ni de corps de
//!   message ;
//! - **ce qu'un plugin demande est réexaminé** : ses actions reviennent sous forme
//!   d'intentions, que l'application applique avec ses propres règles. Le plugin
//!   propose, l'hôte dispose.
//!
//! Un plugin en échec répété est mis hors circuit par le registre. Ce module se
//! contente de le dire.

use crate::controller::{Controller, Request};
use iris_kernel::{Event, EventBus};
use iris_plugins::{entry_points, LoadReport, PluginRegistry, MANIFEST_FILE};
use iris_store::Store;
use iris_types::{Flags, MessageId, ThreadId};
use iris_viewmodel::Action;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// Nombre maximal de messages détaillés par événement d'arrivée.
///
/// Une première synchronisation apporte des milliers de messages d'un coup. Les
/// donner tous à chaque plugin transformerait le démarrage en travail de fond
/// interminable, pour un intérêt nul : un plugin qui trie les infolettres les
/// rattrapera au passage suivant.
const MAX_PAR_LOT: usize = 32;

/// Ce qu'un plugin demande à l'application de faire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEffect {
    /// Changer l'état d'un fil.
    Act {
        plugin: String,
        thread: ThreadId,
        action: Action,
    },
    /// Ajouter une commande à la palette.
    Command { plugin: String, spec: String },
    /// Afficher un message.
    Notify { plugin: String, message: String },
}

/// Ce que l'application demande aux plugins.
#[derive(Debug, Clone)]
enum PluginRequest {
    Event {
        payload: String,
        thread: Option<ThreadId>,
    },
    Command {
        spec: String,
        thread: Option<ThreadId>,
    },
    Shutdown,
}

/// Le fil des plugins.
#[derive(Debug)]
pub struct PluginService {
    requests: Sender<PluginRequest>,
    /// Ce que le chargement a donné, pour l'afficher une fois au démarrage.
    report: LoadReport,
}

impl PluginService {
    /// Charge les plugins d'un répertoire et démarre leur fil.
    ///
    /// Le chargement est synchrone : il faut savoir tout de suite ce qui est en
    /// place, notamment pour le dire dans le journal. L'exécution, elle, ne l'est
    /// jamais.
    pub fn spawn(
        dir: impl AsRef<Path>,
        store: Arc<Store>,
        on_effect: impl Fn(PluginEffect) + Send + 'static,
    ) -> (Self, Option<std::thread::JoinHandle<()>>) {
        let dir = dir.as_ref().to_path_buf();
        let mut registre = PluginRegistry::new();

        let report = match registre.load_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(dossier = %dir.display(), erreur = %e, "lecture des plugins");
                Default::default()
            }
        };

        for (nom, raison) in &report.rejected {
            tracing::warn!(plugin = %nom, raison = %raison, "plugin écarté");
        }

        let (tx, rx) = std::sync::mpsc::channel();

        // Sans plugin, aucun fil : un exécuteur WebAssembly qui ne fera jamais rien
        // est de la mémoire et un fil de trop.
        if registre.is_empty() {
            return (
                Self {
                    requests: tx,
                    report,
                },
                None,
            );
        }

        // `init` avant tout événement : c'est le contrat, et un plugin qui n'a pas
        // été initialisé n'a aucune raison de savoir répondre.
        for (id, resultat) in registre.dispatch(entry_points::INIT, "{}") {
            match resultat {
                Ok(trace) => journaliser(&id, &trace),
                Err(e) => tracing::warn!(plugin = %id, erreur = %e, "initialisation"),
            }
        }

        let fil = std::thread::Builder::new()
            .name("iris-plugins".into())
            .spawn(move || run(registre, store, rx, on_effect))
            .expect("création du fil des plugins");

        (
            Self {
                requests: tx,
                report,
            },
            Some(fil),
        )
    }

    pub fn report(&self) -> &LoadReport {
        &self.report
    }

    /// Transmet un événement du bus, s'il intéresse les plugins.
    pub fn notify(&self, event: &Event, store: &Store) {
        let Some((payload, thread)) = payload_pour(event, store) else {
            return;
        };
        let _ = self.requests.send(PluginRequest::Event { payload, thread });
    }

    /// Invoque une commande fournie par un plugin.
    pub fn invoke(&self, spec: &str, thread: Option<ThreadId>) {
        let _ = self.requests.send(PluginRequest::Command {
            spec: spec.to_string(),
            thread,
        });
    }

    pub fn shutdown(&self) {
        let _ = self.requests.send(PluginRequest::Shutdown);
    }
}

fn run(
    mut registre: PluginRegistry,
    store: Arc<Store>,
    requests: Receiver<PluginRequest>,
    on_effect: impl Fn(PluginEffect),
) {
    let _ = &store;

    while let Ok(requete) = requests.recv() {
        let (export, payload, thread) = match requete {
            PluginRequest::Event { payload, thread } => (entry_points::ON_EVENT, payload, thread),
            PluginRequest::Command { spec, thread } => (entry_points::ON_COMMAND, spec, thread),
            PluginRequest::Shutdown => break,
        };

        for (id, resultat) in registre.dispatch(export, &payload) {
            match resultat {
                Ok(trace) => {
                    journaliser(&id, &trace);
                    for effet in effets(&id, &trace, thread) {
                        on_effect(effet);
                    }
                }
                // L'échec d'un plugin est une nouvelle sans gravité : le registre
                // compte les fautes et finira par le mettre hors circuit.
                Err(e) => tracing::warn!(plugin = %id, erreur = %e, "appel de plugin"),
            }
        }

        for (id, raison) in registre.disabled() {
            tracing::warn!(plugin = %id, raison = %raison, "plugin hors circuit");
        }
    }
}

/// Journalise ce qu'un plugin a écrit et ce qui lui a été refusé.
fn journaliser(id: &str, trace: &iris_plugins::CallTrace) {
    for ligne in &trace.logs {
        tracing::info!(plugin = %id, "{ligne}");
    }
    for refus in &trace.denied {
        // Un refus se dit : sinon l'auteur du plugin cherche longtemps pourquoi
        // son code « ne fait rien ».
        tracing::warn!(plugin = %id, permission = %refus, "permission refusée");
    }
}

/// Traduit les actions brutes d'un plugin en intentions que l'hôte comprend.
///
/// Une action inconnue est ignorée, jamais devinée : exécuter approximativement ce
/// qu'un plugin a demandé est pire que de ne rien faire.
fn effets(
    plugin: &str,
    trace: &iris_plugins::CallTrace,
    thread: Option<ThreadId>,
) -> Vec<PluginEffect> {
    let mut sortie = Vec::new();

    for brut in &trace.actions {
        if let Some(spec) = brut.strip_prefix("command:") {
            sortie.push(PluginEffect::Command {
                plugin: plugin.to_string(),
                spec: spec.to_string(),
            });
            continue;
        }
        if let Some(message) = brut.strip_prefix("notify:") {
            sortie.push(PluginEffect::Notify {
                plugin: plugin.to_string(),
                message: message.to_string(),
            });
            continue;
        }

        // Une action porte sur le fil de l'événement en cours. Sans fil, elle n'a
        // pas de cible : un plugin ne choisit pas sur quoi il agit.
        let Some(thread) = thread else {
            tracing::warn!(plugin = %plugin, "action sans fil, ignorée");
            continue;
        };
        match action_depuis_json(brut) {
            Some(action) => sortie.push(PluginEffect::Act {
                plugin: plugin.to_string(),
                thread,
                action,
            }),
            None => tracing::warn!(plugin = %plugin, demande = %brut, "action non reconnue"),
        }
    }

    sortie
}

/// Lit `{"action":"done"}` et compagnie.
fn action_depuis_json(brut: &str) -> Option<Action> {
    let valeur: serde_json::Value = serde_json::from_str(brut).ok()?;
    match valeur.get("action")?.as_str()? {
        "done" | "traite" => Some(Action::Done),
        "todo" | "a_traiter" => Some(Action::Todo),
        "waiting" | "en_attente" => Some(Action::Waiting),
        _ => None,
    }
}

/// Compose la charge JSON d'un événement, et le fil qu'elle concerne.
///
/// Tous les événements ne partent pas : un plugin n'a rien à faire des phases de
/// synchronisation ni des rechargements de thème, et chaque événement transmis est
/// du carburant dépensé.
fn payload_pour(event: &Event, store: &Store) -> Option<(String, Option<ThreadId>)> {
    match event {
        Event::MessagesAdded { ids, .. } => {
            let message = *ids.first()?;
            let detail = detail_message(message, store)?;
            let thread = detail.1;
            let mut charge = detail.0;
            if ids.len() > 1 {
                charge["autres"] = serde_json::json!(ids.len().min(MAX_PAR_LOT) - 1);
            }
            Some((charge.to_string(), Some(thread)))
        }
        Event::ThreadStateChanged {
            thread, from, to, ..
        } => Some((
            serde_json::json!({
                "event": "thread-state-changed",
                "thread": thread.get(),
                "de": from.as_str(),
                "vers": to.as_str(),
            })
            .to_string(),
            Some(*thread),
        )),
        _ => None,
    }
}

/// Ce qu'un plugin apprend d'un message.
///
/// Volontairement pauvre : l'expéditeur, le sujet, et des étiquettes. Pas de corps,
/// pas de destinataires, pas d'identifiant de compte — un plugin de tri n'en a pas
/// besoin, et ce qu'on ne transmet pas ne peut pas fuir.
fn detail_message(id: MessageId, store: &Store) -> Option<(serde_json::Value, ThreadId)> {
    let message = store.message_by_id(id).ok().flatten()?;

    // Les étiquettes ne sont présentes que lorsqu'elles s'appliquent : un plugin qui
    // cherche « unsubscribe » dans la charge doit pouvoir s'y fier.
    let mut etiquettes = Vec::new();
    if message.flags.contains(Flags::UNSUBSCRIBABLE) {
        etiquettes.push("unsubscribe");
    }
    if message.flags.contains(Flags::HAS_ATTACHMENT) {
        etiquettes.push("attachment");
    }
    if message.flags.contains(Flags::HAS_TRACKER) {
        etiquettes.push("tracker");
    }
    if !message.flags.contains(Flags::SEEN) {
        etiquettes.push("unread");
    }

    Some((
        serde_json::json!({
            "event": "message-added",
            "thread": message.thread.get(),
            "de": message.from_addr,
            "sujet": message.subject,
            "etiquettes": etiquettes,
        }),
        message.thread,
    ))
}

/// Relie le bus aux plugins.
pub async fn pump(bus: &EventBus, store: Arc<Store>, service: Arc<PluginService>) {
    let mut abonne = bus.subscribe_view();
    while let Some(event) = abonne.recv().await {
        service.notify(&event, &store);
    }
}

/// Applique l'effet demandé par un plugin.
pub fn apply_effect(effect: &PluginEffect, controller: &Controller) -> Option<String> {
    match effect {
        PluginEffect::Act {
            plugin,
            thread,
            action,
        } => {
            tracing::info!(plugin = %plugin, fil = %thread, action = ?action, "action de plugin");
            controller.send(Request::ApplyTo(*thread, *action));
            None
        }
        // Une notification de plugin est attribuée : l'utilisateur doit savoir qui
        // lui parle, sans quoi l'application porte le chapeau.
        PluginEffect::Notify { plugin, message } => Some(format!("{plugin} : {message}")),
        PluginEffect::Command { plugin, spec } => {
            tracing::info!(plugin = %plugin, commande = %spec, "commande proposée");
            None
        }
    }
}

/// Le répertoire d'un plugin contient-il un manifeste ?
pub fn looks_like_plugin(dir: &Path) -> bool {
    dir.join(MANIFEST_FILE).is_file()
}

/// Le répertoire des plugins, créé au besoin.
pub fn ensure_dir(dir: PathBuf) -> PathBuf {
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_plugins::CallTrace;
    use iris_store::{FolderRole, NewAccount, NewMessage};
    use iris_types::Timestamp;

    fn trace(actions: &[&str]) -> CallTrace {
        CallTrace {
            logs: Vec::new(),
            actions: actions.iter().map(|s| s.to_string()).collect(),
            denied: Vec::new(),
        }
    }

    #[test]
    fn une_action_connue_devient_une_intention() {
        let effets = effets("p", &trace(&["{\"action\":\"done\"}"]), Some(ThreadId(4)));
        assert_eq!(
            effets,
            [PluginEffect::Act {
                plugin: "p".into(),
                thread: ThreadId(4),
                action: Action::Done
            }]
        );
    }

    #[test]
    fn les_deux_langues_sont_acceptees() {
        assert_eq!(
            action_depuis_json("{\"action\":\"traite\"}"),
            Some(Action::Done)
        );
        assert_eq!(
            action_depuis_json("{\"action\":\"waiting\"}"),
            Some(Action::Waiting)
        );
        assert_eq!(
            action_depuis_json("{\"action\":\"a_traiter\"}"),
            Some(Action::Todo)
        );
    }

    #[test]
    fn une_action_inconnue_est_ignoree_et_pas_devinee() {
        // Exécuter approximativement ce qu'un plugin demande est pire que rien.
        assert_eq!(action_depuis_json("{\"action\":\"supprimer-tout\"}"), None);
        assert!(effets("p", &trace(&["{\"action\":\"???\"}"]), Some(ThreadId(1))).is_empty());
    }

    #[test]
    fn une_action_illisible_ne_fait_pas_paniquer() {
        assert_eq!(action_depuis_json("pas du json"), None);
        assert!(effets("p", &trace(&["<<<"]), Some(ThreadId(1))).is_empty());
    }

    #[test]
    fn une_action_sans_fil_n_a_pas_de_cible() {
        // Un plugin ne choisit pas sur quoi il agit : il agit sur ce qu'on lui a
        // montré.
        assert!(effets("p", &trace(&["{\"action\":\"done\"}"]), None).is_empty());
    }

    #[test]
    fn une_commande_et_une_notification_passent_sans_fil() {
        let effets = effets(
            "p",
            &trace(&["command:{\"id\":\"x\"}", "notify:bonjour"]),
            None,
        );
        assert_eq!(
            effets,
            [
                PluginEffect::Command {
                    plugin: "p".into(),
                    spec: "{\"id\":\"x\"}".into()
                },
                PluginEffect::Notify {
                    plugin: "p".into(),
                    message: "bonjour".into()
                },
            ]
        );
    }

    #[test]
    fn une_notification_est_attribuee_a_son_plugin() {
        // Sans le nom, l'application porte le chapeau de ce qu'un plugin raconte.
        let (c, _rx, fil) = controleur_muet();
        let message = apply_effect(
            &PluginEffect::Notify {
                plugin: "tri".into(),
                message: "3 triés".into(),
            },
            &c,
        );
        assert_eq!(message.as_deref(), Some("tri : 3 triés"));
        c.shutdown();
        fil.join().unwrap();
    }

    fn controleur_muet() -> (
        Controller,
        std::sync::mpsc::Receiver<()>,
        std::thread::JoinHandle<()>,
    ) {
        let store = Arc::new(Store::in_memory().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let (c, fil) = Controller::spawn(store, Default::default(), Timestamp::EPOCH, move |_| {
            let _ = tx.send(());
        });
        (c, rx, fil)
    }

    /// Un store avec un message porteur de l'étiquette demandée.
    fn store_avec(flags: Flags) -> (Arc<Store>, MessageId, ThreadId) {
        let store = Arc::new(Store::in_memory().unwrap());
        let compte = store
            .create_account(&NewAccount::new("a@x.fr", "i", "s"), Timestamp::EPOCH)
            .unwrap();
        let dossier = store
            .upsert_folder(compte, "INBOX", FolderRole::Inbox)
            .unwrap();
        let insere = store
            .insert_message(&NewMessage {
                account: compte,
                folder: dossier,
                uid: 1,
                rfc_message_id: Some("m1@x".into()),
                in_reply_to: None,
                references: vec![],
                subject: "Offre du mois".into(),
                from_name: "Boutique".into(),
                from_addr: "info@boutique.fr".into(),
                recipients_json: "[]".into(),
                date: Timestamp::EPOCH,
                received: Timestamp::EPOCH,
                size: 100,
                flags,
                preview: "corps secret".into(),
            })
            .unwrap();
        (store, insere.message, insere.thread)
    }

    #[test]
    fn une_infolettre_est_etiquetee_comme_telle() {
        let (store, id, _) = store_avec(Flags::UNSUBSCRIBABLE);
        let (charge, _) = detail_message(id, &store).unwrap();
        assert!(charge.to_string().contains("unsubscribe"));
    }

    #[test]
    fn une_etiquette_absente_n_apparait_pas() {
        // Un plugin qui cherche « unsubscribe » dans la charge doit pouvoir s'y
        // fier : la clé toujours présente ferait correspondre tous les messages.
        let (store, id, _) = store_avec(Flags::SEEN);
        let (charge, _) = detail_message(id, &store).unwrap();
        assert!(!charge.to_string().contains("unsubscribe"));
        assert!(!charge.to_string().contains("unread"));
    }

    #[test]
    fn la_charge_ne_transporte_ni_corps_ni_compte() {
        // Ce qu'on ne transmet pas ne peut pas fuir.
        let (store, id, _) = store_avec(Flags::NONE);
        let texte = detail_message(id, &store).unwrap().0.to_string();
        assert!(!texte.contains("corps secret"));
        assert!(!texte.contains("compte"));
        assert!(
            texte.contains("info@boutique.fr"),
            "l'expéditeur, lui, est utile"
        );
    }

    #[test]
    fn un_message_disparu_ne_produit_pas_de_charge() {
        let (store, _, _) = store_avec(Flags::NONE);
        assert!(detail_message(MessageId(999), &store).is_none());
    }

    #[test]
    fn seuls_les_evenements_utiles_partent() {
        // Chaque événement transmis est du carburant dépensé.
        let (store, id, fil) = store_avec(Flags::NONE);
        let compte = store.accounts().unwrap()[0].id;
        let dossier = store.folders(compte).unwrap()[0].id;

        let arrivee = Event::MessagesAdded {
            account: compte,
            folder: dossier,
            ids: Arc::from(vec![id]),
        };
        assert_eq!(payload_pour(&arrivee, &store).unwrap().1, Some(fil));

        let bruit = Event::ThemeReloaded {
            name: Arc::from("mono"),
        };
        assert!(payload_pour(&bruit, &store).is_none());

        let phase = Event::SyncPhaseChanged {
            account: compte,
            phase: iris_kernel::event::SyncPhase::Idle,
        };
        assert!(payload_pour(&phase, &store).is_none());
    }

    #[test]
    fn un_changement_d_etat_est_transmis_avec_son_fil() {
        let (store, _, fil) = store_avec(Flags::NONE);
        let event = Event::ThreadStateChanged {
            thread: fil,
            from: iris_types::WorkflowState::Todo,
            to: iris_types::WorkflowState::Done,
            cause: iris_types::TransitionCause::Manual,
        };

        let (charge, cible) = payload_pour(&event, &store).unwrap();
        assert_eq!(cible, Some(fil));
        assert!(charge.contains("thread-state-changed"));
    }

    #[test]
    fn un_dossier_sans_plugin_ne_demarre_pas_de_fil() {
        // Un exécuteur WebAssembly qui ne fera jamais rien est un fil de trop.
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::in_memory().unwrap());
        let (service, fil) = PluginService::spawn(dir.path(), store, |_| {});

        assert!(fil.is_none());
        assert_eq!(service.report().loaded.len(), 0);
        // Le service reste utilisable : notifier ne doit pas paniquer.
        service.shutdown();
    }

    #[test]
    fn un_dossier_absent_n_est_pas_une_erreur() {
        let store = Arc::new(Store::in_memory().unwrap());
        let (service, fil) = PluginService::spawn("n/existe/pas", store, |_| {});
        assert!(fil.is_none());
        assert!(service.report().rejected.is_empty());
    }

    #[test]
    fn un_plugin_illisible_est_ecarte_sans_empecher_le_demarrage() {
        let dir = tempfile::tempdir().unwrap();
        let mauvais = dir.path().join("casse");
        std::fs::create_dir_all(&mauvais).unwrap();
        std::fs::write(mauvais.join(MANIFEST_FILE), "ceci = = n'est pas du toml").unwrap();

        let store = Arc::new(Store::in_memory().unwrap());
        let (service, _) = PluginService::spawn(dir.path(), store, |_| {});

        assert_eq!(service.report().rejected.len(), 1, "écarté, et signalé");
        assert!(service.report().loaded.is_empty());
    }

    #[test]
    fn un_repertoire_de_plugin_se_reconnait_a_son_manifeste() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!looks_like_plugin(dir.path()));
        std::fs::write(dir.path().join(MANIFEST_FILE), "id = \"x\"").unwrap();
        assert!(looks_like_plugin(dir.path()));
    }
}
