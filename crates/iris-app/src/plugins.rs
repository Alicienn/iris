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
    /// Ranger un fil dans un dossier.
    File {
        plugin: String,
        thread: ThreadId,
        folder: String,
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
    /// Ce que chaque plugin a déclaré, pour l'écran des modules.
    manifests: Vec<(iris_plugins::Manifest, Option<String>)>,
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
                tracing::warn!(folder = %dir.display(), error = %e, "reading the plugins folder");
                Default::default()
            }
        };

        for (nom, raison) in &report.rejected {
            tracing::warn!(plugin = %nom, reason = %raison, "plugin rejected");
        }

        // What every loaded plugin declared, captured before the registry moves into
        // its own thread: the modules screen must be able to list them without
        // reaching across that boundary.
        let manifests: Vec<(iris_plugins::Manifest, Option<String>)> = registre
            .manifests()
            .into_iter()
            .map(|(m, reason)| (m.clone(), reason.map(str::to_string)))
            .collect();

        let (tx, rx) = std::sync::mpsc::channel();

        // Sans plugin, aucun fil : un exécuteur WebAssembly qui ne fera jamais rien
        // est de la mémoire et un fil de trop.
        if registre.is_empty() {
            return (
                Self {
                    requests: tx,
                    report,
                    manifests,
                },
                None,
            );
        }

        // `init` avant tout événement : c'est le contrat, et un plugin qui n'a pas
        // été initialisé n'a aucune raison de savoir répondre.
        //
        // Ses réglages lui sont remis **à ce moment-là**, et une seule fois. Les
        // joindre à chaque événement les ferait analyser à chaque message, pour des
        // valeurs qui ne changent pas — et un plugin qui reçoit sa configuration au
        // démarrage est un plugin dont le comportement ne peut pas dériver en cours de
        // route. Changer un réglage demande un redémarrage, comme installer un module ;
        // c'est la même contrainte, et elle a la même cause.
        for (id, reglages) in reglages_des_plugins(&dir, &registre) {
            for (rendu, resultat) in registre.dispatch_one(&id, entry_points::INIT, &reglages) {
                match resultat {
                    Ok(trace) => journaliser(&rendu, &trace),
                    Err(e) => tracing::warn!(plugin = %rendu, error = %e, "initialisation"),
                }
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
                manifests,
            },
            Some(fil),
        )
    }

    pub fn report(&self) -> &LoadReport {
        &self.report
    }

    /// The manifests of everything that loaded, with the reason any of them is out of
    /// circulation.
    ///
    /// Kept separately from the registry so the modules screen can list plugins
    /// without reaching into the thread that runs them.
    pub fn manifests(&self) -> Vec<(iris_plugins::Manifest, Option<String>)> {
        self.manifests.clone()
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
                Err(e) => tracing::warn!(plugin = %id, error = %e, "plugin call"),
            }
        }

        for (id, raison) in registre.disabled() {
            tracing::warn!(plugin = %id, reason = %raison, "plugin out of circulation");
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
        tracing::warn!(plugin = %id, permission = %refus, "permission denied");
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
            tracing::warn!(plugin = %plugin, "action with no thread, ignored");
            continue;
        };
        match action_depuis_json(brut) {
            Some(Demande::Etat(action)) => sortie.push(PluginEffect::Act {
                plugin: plugin.to_string(),
                thread,
                action,
            }),
            Some(Demande::Ranger(folder)) => sortie.push(PluginEffect::File {
                plugin: plugin.to_string(),
                thread,
                folder,
            }),
            None => tracing::warn!(plugin = %plugin, request = %brut, "unrecognised action"),
        }
    }

    sortie
}

/// Ce qu'un plugin peut demander.
///
/// Le vocabulaire est **fermé** et volontairement court. Chaque verbe ajouté est une
/// chose de plus qu'un module peut faire à votre courrier, et la question à se poser
/// n'est pas « est-ce utile » mais « accepterais-je qu'un module inconnu le fasse ».
/// Déplacer, reporter, marquer : oui, tout est réversible et visible. Envoyer,
/// supprimer définitivement, lire un secret : jamais.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Demande {
    /// Une transition du flux de travail.
    Etat(Action),
    /// Ranger dans un dossier, par son nom unifié.
    Ranger(String),
}

/// Lit `{"action":"done"}` et compagnie.
fn action_depuis_json(brut: &str) -> Option<Demande> {
    let valeur: serde_json::Value = serde_json::from_str(brut).ok()?;

    match valeur.get("action")?.as_str()? {
        "done" | "traite" => Some(Demande::Etat(Action::Done)),
        "todo" | "a_traiter" => Some(Demande::Etat(Action::Todo)),
        "waiting" | "en_attente" => Some(Demande::Etat(Action::Waiting)),
        "star" | "epingler" => Some(Demande::Etat(Action::ToggleFlag)),
        "read" | "lu" => Some(Demande::Etat(Action::MarkRead)),
        "unread" | "non_lu" => Some(Demande::Etat(Action::MarkUnread)),
        // Le report en heures. Borné à un an : un plugin qui demande dix mille heures
        // ne reporte pas, il fait disparaître, et la différence compte.
        "snooze" | "reporter" => {
            let heures = valeur.get("hours").and_then(|h| h.as_u64()).unwrap_or(24);
            Some(Demande::Etat(Action::SnoozeHours(
                heures.clamp(1, 24 * 365) as u32,
            )))
        }
        "move" | "ranger" => {
            let dossier = valeur.get("folder")?.as_str()?.trim();
            // Un chemin vide rangerait « quelque part », ce qui n'existe pas.
            (!dossier.is_empty()).then(|| Demande::Ranger(dossier.to_string()))
        }
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

/// Les réglages de chaque plugin, en JSON, prêts pour son initialisation.
///
/// Les valeurs effectives — celles que l'utilisateur a choisies, complétées par les
/// défauts que le manifeste déclare. Un plugin n'a pas à connaître ses propres défauts
/// une seconde fois, dans son code, en risquant qu'ils divergent du manifeste.
fn reglages_des_plugins(dir: &Path, registre: &PluginRegistry) -> Vec<(String, String)> {
    registre
        .manifests()
        .into_iter()
        .map(|(manifeste, _)| {
            let valeurs = iris_plugins::SettingValues::load(&dir.join(&manifeste.id));
            let effectives = valeurs.effective(&manifeste.settings);
            (
                manifeste.id.clone(),
                serde_json::to_string(&effectives).unwrap_or_else(|_| "{}".into()),
            )
        })
        .collect()
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

    // Les noms des pièces jointes, sans leur contenu.
    //
    // Un trieur a besoin de savoir qu'il s'agit d'une facture ; il n'a pas besoin de
    // la lire. La distinction est le principe de tout ce qui traverse cette frontière :
    // ce qu'on ne transmet pas ne peut pas fuir.
    let pieces: Vec<String> = store
        .visible_attachments(id)
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.meta.filename)
        .collect();

    Some((
        serde_json::json!({
            "event": "message-added",
            "thread": message.thread.get(),
            "de": message.from_addr,
            "sujet": message.subject,
            "etiquettes": etiquettes,
            "pieces": pieces,
            // L'heure d'arrivée, décomposée. Un plugin qui reporte le courrier reçu
            // hors des heures de bureau ne doit pas avoir à refaire un calendrier en
            // WebAssembly pour savoir quel jour on est.
            "recu": message.received.millis(),
            "heure": heure_du_jour(message.received),
            "jour": jour_de_semaine(message.received),
        }),
        message.thread,
    ))
}

/// L'heure locale d'un instant, de 0 à 23.
///
/// En temps universel, faute d'un fuseau : Iris n'en connaît aucun, et en inventer un
/// serait pire que de le dire. Un plugin qui règle des heures de bureau les règle donc
/// en UTC, ce que son écran de réglages doit annoncer.
fn heure_du_jour(t: iris_types::Timestamp) -> i64 {
    t.seconds().rem_euclid(86_400) / 3600
}

/// Le jour de la semaine, de 0 (lundi) à 6 (dimanche).
///
/// Le 1er janvier 1970 était un jeudi, ce qui met le décalage à 3.
fn jour_de_semaine(t: iris_types::Timestamp) -> i64 {
    (t.seconds().div_euclid(86_400) + 3).rem_euclid(7)
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
            tracing::info!(plugin = %plugin, thread = %thread, action = ?action, "plugin action");
            controller.send(Request::ApplyTo(*thread, *action));
            None
        }
        PluginEffect::File {
            plugin,
            thread,
            folder,
        } => {
            tracing::info!(plugin = %plugin, thread = %thread, folder = %folder, "plugin filing");
            controller.send(Request::MoveThreadToFolder(*thread, folder.clone()));
            None
        }
        // Une notification de plugin est attribuée : l'utilisateur doit savoir qui
        // lui parle, sans quoi l'application porte le chapeau.
        PluginEffect::Notify { plugin, message } => Some(format!("{plugin} : {message}")),
        PluginEffect::Command { plugin, spec } => {
            tracing::info!(plugin = %plugin, commande = %spec, "command offered");
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
            Some(Demande::Etat(Action::Done))
        );
        assert_eq!(
            action_depuis_json("{\"action\":\"waiting\"}"),
            Some(Demande::Etat(Action::Waiting))
        );
        assert_eq!(
            action_depuis_json("{\"action\":\"a_traiter\"}"),
            Some(Demande::Etat(Action::Todo))
        );
    }

    #[test]
    fn ranger_exige_un_dossier() {
        assert_eq!(
            action_depuis_json("{\"action\":\"move\",\"folder\":\"Compta\"}"),
            Some(Demande::Ranger("Compta".into()))
        );
        // Un chemin vide rangerait « quelque part », ce qui n'existe pas.
        assert_eq!(
            action_depuis_json("{\"action\":\"move\",\"folder\":\"  \"}"),
            None
        );
        assert_eq!(action_depuis_json("{\"action\":\"move\"}"), None);
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
        let workflow = crate::controller::default_workflow(Arc::clone(&store));
        let (c, fil) = Controller::spawn(store, workflow, Timestamp::EPOCH, move |_| {
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
