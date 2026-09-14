//! La boucle d'interface.
//!
//! Elle ne contient aucune décision : elle branche les rappels de la fenêtre sur le
//! contrôleur, et pousse les instantanés dans l'autre sens. Tout ce qui pourrait être
//! discuté — quelle touche fait quoi, quelle commande apparaît, comment une date
//! s'écrit — a déjà été tranché ailleurs, et y est testé.

use crate::controller::{Controller, Request, Snapshot};
use crate::services::{now, Services};
use crate::settings::{Density, Settings};
use iris_kernel::ViewDiff;
use iris_sync::SendService;
use iris_types::ThreadId as ThreadIdent;
use iris_types::{ThreadId, WorkflowState};
use iris_ui::bridge;
use iris_ui::commands::{self, CommandKind};
use iris_ui::keymap::{KeyOutcome, Keymap};
use iris_ui::{AppWindow, Tokens};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;
use std::sync::Arc;

/// Construit la fenêtre et la relie au contrôleur.
pub fn build(services: &Services) -> iris_types::Result<AppWindow> {
    let fenetre = AppWindow::new()
        .map_err(|e| iris_types::Error::other(format!("création de la fenêtre : {e}")))?;

    bridge::apply_theme(&fenetre.global::<Tokens>(), &services.themes.active());
    // Avant le premier instantané, la liste est vide parce qu'elle n'est pas encore
    // lue — pas parce qu'il n'y a rien. Annoncer « Rien à traiter » à ce moment-là
    // serait un mensonge d'un dixième de seconde, mais un mensonge quand même.
    fenetre.set_loading(true);
    refresh_accounts(&fenetre, services, &[]);

    Ok(fenetre)
}

/// Remplit la barre latérale.
///
/// Appelée au démarrage puis à chaque tour de synchronisation : c'est par là qu'un
/// compte tombé en panne se signale, et qu'il cesse de le faire une fois réparé.
pub fn refresh_accounts(
    fenetre: &AppWindow,
    services: &Services,
    suspendus: &[iris_types::AccountId],
) {
    let comptes = services.store.accounts().unwrap_or_default();
    let suspendus: std::collections::BTreeSet<iris_types::AccountId> =
        suspendus.iter().copied().collect();

    // Ce qui reste à traiter, boîte par boîte. Le nombre total de messages ne dirait
    // rien de ce qu'il y a à faire, et un « 12 483 » permanent n'apprend rien.
    let a_traiter = services
        .store
        .todo_counts_by_account(now())
        .unwrap_or_default();

    let (epingles, autres): (Vec<_>, Vec<_>) = comptes.iter().partition(|c| c.pinned);

    let vers_modele = |liste: Vec<&iris_store::Account>| {
        ModelRc::new(VecModel::from(
            liste
                .into_iter()
                .map(|c| {
                    bridge::account_row(
                        c,
                        a_traiter.get(&c.id).copied().unwrap_or(0),
                        suspendus.contains(&c.id),
                    )
                })
                .collect::<Vec<_>>(),
        ))
    };

    fenetre.set_pinned_accounts(vers_modele(epingles));
    fenetre.set_other_accounts(vers_modele(autres));
    fenetre.set_unified_count(a_traiter.values().sum::<u32>() as i32);

    if let Some(message) = message_suspension(&comptes, &suspendus) {
        fenetre.set_status(message.into());
    }
}

/// Le message de la barre d'état quand des comptes sont en pause.
///
/// Le marqueur « ! » seul est discret : quand une boîte ne se synchronise plus,
/// l'application le dit en toutes lettres. Au-delà de trois comptes, elle compte
/// plutôt que d'énumérer — une liste de quarante adresses n'informe personne.
fn message_suspension(
    comptes: &[iris_store::Account],
    suspendus: &std::collections::BTreeSet<iris_types::AccountId>,
) -> Option<String> {
    if suspendus.is_empty() {
        return None;
    }

    let noms: Vec<&str> = comptes
        .iter()
        .filter(|c| suspendus.contains(&c.id))
        .map(|c| c.email.as_str())
        .collect();

    // Un compte suspendu que le store ne connaît pas ne peut pas être nommé ; on ne
    // laisse pas pour autant l'utilisateur sans message.
    let combien = noms.len().max(suspendus.len());
    let qui = match noms.len() {
        0 => format!("{combien} account{}", if combien > 1 { "s" } else { "" }),
        1..=3 => noms.join(", "),
        n => format!("{n} accounts"),
    };

    Some(format!(
        "{qui} paused after repeated failures — click the \"!\" to try again."
    ))
}

/// Branche la reprise d'un compte suspendu.
pub fn wire_account_recovery(
    fenetre: &AppWindow,
    engine: Arc<iris_sync::SyncEngine>,
    runtime: tokio::runtime::Handle,
) {
    let faible = fenetre.as_weak();
    fenetre.on_resume_account(move |id| {
        let Some(fenetre) = faible.upgrade() else {
            return;
        };
        let compte = iris_types::AccountId(id as i64);
        fenetre.set_status("Trying again…".into());

        let engine = Arc::clone(&engine);
        runtime.spawn(async move {
            // La reprise remet le compte dans l'ordonnanceur ; le tour suivant dira
            // si la panne a disparu. On ne promet donc rien de plus qu'un essai.
            engine.resume_account(compte, iris_sync::now_utc()).await;
        });
    });
}

/// Les commandes disponibles, celles de l'application et celles des plugins.
///
/// La liste doit pouvoir grandir : un plugin déclare ses commandes en s'exécutant,
/// c'est-à-dire après que la palette a été branchée. Un tableau figé au démarrage
/// obligerait à redémarrer pour voir une extension apparaître.
pub struct CommandBook {
    commandes: std::sync::Mutex<Vec<commands::Command>>,
    /// Ce que la palette doit envoyer aux plugins, par identifiant de commande.
    plugin_specs: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    /// Où acheminer une commande de plugin. Rempli une fois, quand le service des
    /// plugins existe — c'est-à-dire après que la palette a été branchée.
    #[allow(clippy::type_complexity)]
    sink: std::sync::OnceLock<Box<dyn Fn(&str, &str) + Send + Sync>>,
}

impl CommandBook {
    pub fn new() -> Self {
        Self {
            commandes: std::sync::Mutex::new(commands::builtin_commands()),
            plugin_specs: std::sync::Mutex::new(Default::default()),
            sink: std::sync::OnceLock::new(),
        }
    }

    /// Désigne le destinataire des commandes de plugin.
    pub fn set_sink(&self, sink: impl Fn(&str, &str) + Send + Sync + 'static) {
        let _ = self.sink.set(Box::new(sink));
    }

    /// Achemine une commande de plugin, si un destinataire est en place.
    ///
    /// Sans destinataire, la commande est perdue plutôt que mise en attente : une
    /// commande qui s'exécuterait plus tard, à un moment que l'utilisateur n'a pas
    /// choisi, serait pire qu'une commande sans effet.
    fn route(&self, plugin: &str, spec: &str) -> bool {
        match self.sink.get() {
            Some(sink) => {
                sink(plugin, spec);
                true
            }
            None => {
                tracing::warn!(plugin = %plugin, "plugin command with nowhere to go");
                false
            }
        }
    }

    /// Ajoute une commande déclarée par un plugin. Rend son intitulé.
    ///
    /// Redéclarer la même commande la remplace : un plugin qui se réinitialise ne
    /// doit pas laisser deux entrées identiques dans la palette.
    pub fn add_plugin(&self, plugin: &str, spec: &str) -> Option<String> {
        let commande = commands::Command::from_plugin(plugin, spec)?;
        let id = commande.id.clone();
        let label = commande.label.clone();

        let mut liste = self.commandes.lock().ok()?;
        liste.retain(|c| c.id != id);
        liste.push(commande);
        self.plugin_specs.lock().ok()?.insert(id, spec.to_string());
        Some(label)
    }

    /// Filtre les commandes pour la palette.
    pub fn filter(&self, query: &str, has_thread: bool) -> Vec<commands::Command> {
        let liste = self.commandes.lock().expect("commandes empoisonnées");
        commands::filter(&liste, query, has_thread)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn find(&self, id: &str) -> Option<commands::Command> {
        let liste = self.commandes.lock().ok()?;
        liste.iter().find(|c| c.id == id).cloned()
    }

    pub fn len(&self) -> usize {
        self.commandes.lock().map(|l| l.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl std::fmt::Debug for CommandBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandBook")
            .field("commandes", &self.len())
            .field("destinataire", &self.sink.get().is_some())
            .finish()
    }
}

impl Default for CommandBook {
    fn default() -> Self {
        Self::new()
    }
}

/// Branche les rappels de la fenêtre sur le contrôleur.
///
/// Rend le carnet de commandes, pour que les plugins puissent y ajouter les leurs
/// une fois qu'ils tournent.
pub fn wire_callbacks(
    fenetre: &AppWindow,
    controller: Arc<Controller>,
    keymap: Keymap,
) -> Arc<CommandBook> {
    let commandes = Arc::new(CommandBook::new());

    // Palette : la liste est recalculée à chaque frappe, côté Rust.
    {
        let commandes = Arc::clone(&commandes);
        let faible = fenetre.as_weak();
        fenetre.on_palette_query_changed(move |requete| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let a_un_fil = fenetre.get_selected_thread() >= 0;
            let filtrees: Vec<_> = commandes
                .filter(requete.as_str(), a_un_fil)
                .iter()
                .map(bridge::command_row)
                .collect();
            fenetre.set_commands(ModelRc::new(VecModel::from(filtrees)));
        });
    }

    {
        let c = Arc::clone(&controller);
        fenetre.on_thread_selected(move |id| {
            c.send(Request::SelectThread(ThreadId(id as i64)));
        });
    }

    {
        let c = Arc::clone(&controller);
        fenetre.on_tab_selected(move |index| {
            if let Some(etat) = WorkflowState::from_i64(index as i64) {
                c.send(Request::SwitchTab(etat));
            }
        });
    }

    {
        let c = Arc::clone(&controller);
        fenetre.on_account_selected(move |id| {
            c.send(Request::FilterAccounts(vec![iris_types::AccountId(
                id as i64,
            )]));
        });
    }

    {
        let c = Arc::clone(&controller);
        fenetre.on_unified_selected(move || {
            c.send(Request::FilterAccounts(Vec::new()));
        });
    }

    {
        let c = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_scrolled_near_end(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            // On demande un peu au-delà de ce qui est chargé : le préchargement du
            // vue-modèle fait le reste.
            c.send(Request::EnsureLoaded(fenetre.get_rows().row_count()));
        });
    }

    {
        let c = Arc::clone(&controller);
        fenetre.on_search_submitted(move |requete| {
            c.send(Request::Search(requete.to_string()));
        });
    }

    {
        let c = Arc::clone(&controller);
        fenetre.on_search_cleared(move || {
            c.send(Request::ClearSearch);
        });
    }

    {
        let c = Arc::clone(&controller);
        let commandes = Arc::clone(&commandes);
        let faible = fenetre.as_weak();
        fenetre.on_command_invoked(move |id| {
            let Some(commande) = commandes.find(id.as_str()) else {
                return;
            };
            dispatch(&c, &commande.kind, &faible, &commandes);
        });
    }

    {
        let c = Arc::clone(&controller);
        let carnet = Arc::clone(&commandes);
        let faible = fenetre.as_weak();
        fenetre.on_key_pressed(move |touche| match keymap.resolve(touche.as_str()) {
            KeyOutcome::Move(m) => c.send(Request::Move(m)),
            KeyOutcome::Command(kind) => dispatch(&c, &kind, &faible, &carnet),
            KeyOutcome::OpenPalette | KeyOutcome::Ignored => {}
        });
    }

    commandes
}

/// Reconnaît une commande de plugin, pour les tests et les appelants curieux.
pub fn plugin_command(kind: &CommandKind) -> Option<(&str, &str)> {
    match kind {
        CommandKind::Plugin { plugin, spec } => Some((plugin, spec)),
        _ => None,
    }
}

fn dispatch(
    controller: &Controller,
    kind: &CommandKind,
    fenetre: &slint::Weak<AppWindow>,
    carnet: &CommandBook,
) {
    match kind {
        CommandKind::Plugin { plugin, spec } => {
            carnet.route(plugin, spec);
        }
        CommandKind::Thread(action) => controller.send(Request::Apply(*action)),
        CommandKind::SwitchTab(state) => controller.send(Request::SwitchTab(*state)),
        CommandKind::SelectAccount(id) => {
            controller.send(Request::FilterAccounts(vec![iris_types::AccountId(*id)]))
        }
        CommandKind::UnifiedView => controller.send(Request::FilterAccounts(Vec::new())),
        CommandKind::Undo => controller.send(Request::Undo),
        CommandKind::Quit => {
            controller.shutdown();
            let _ = slint::quit_event_loop();
        }
        // Chercher, c'est mettre le curseur dans la barre : le champ est déjà à
        // l'écran, l'ouvrir ailleurs ferait deux endroits pour la même chose.
        CommandKind::Search => {
            if let Some(fenetre) = fenetre.upgrade() {
                fenetre.invoke_focus_search();
            }
        }
        CommandKind::AddAccount => {
            if let Some(fenetre) = fenetre.upgrade() {
                fenetre.set_add_account_open(true);
            }
        }
        CommandKind::Compose => {
            if let Some(fenetre) = fenetre.upgrade() {
                fenetre.set_compose_open(true);
            }
        }
        CommandKind::Modules => {
            if let Some(fenetre) = fenetre.upgrade() {
                fenetre.set_modules_open(true);
            }
        }
        CommandKind::Settings => {
            if let Some(fenetre) = fenetre.upgrade() {
                fenetre.set_settings_open(true);
            }
        }
        // Le rechargement des thèmes appartient à la surveillance de fichiers, qui
        // le fait déjà toute seule ; la commande n'a rien à ajouter.
        CommandKind::Reload => {}
    }
}

/// Recopie un instantané dans la fenêtre.
///
/// Appelée depuis la boucle d'interface, jamais depuis le fil du vue-modèle : c'est
/// le seul endroit où les deux mondes se touchent.
pub fn apply_snapshot(
    fenetre: &AppWindow,
    services: &Services,
    renderer: &dyn iris_htmlview::HtmlRenderer,
    snapshot: &Snapshot,
) {
    let maintenant = now();
    fenetre.set_loading(false);

    // Les adresses des comptes servent à colorer les lignes ; on les résout une fois
    // par instantané, pas une fois par ligne.
    let comptes = services.store.accounts().unwrap_or_default();
    let adresse_par_defaut = comptes.first().map(|c| c.email.clone()).unwrap_or_default();

    let lignes: Vec<_> = snapshot
        .rows
        .iter()
        .map(|r| bridge::thread_row(r, &adresse_par_defaut, maintenant))
        .collect();

    fenetre.set_rows(ModelRc::new(VecModel::from(lignes)));
    fenetre.set_selected_thread(snapshot.selected.map(|t| t.get() as i32).unwrap_or(-1));
    fenetre.set_active_tab(snapshot.active_tab.as_i64() as i32);
    fenetre.set_counts(ModelRc::new(VecModel::from(
        snapshot
            .counts
            .iter()
            .map(|c| *c as i32)
            .collect::<Vec<_>>(),
    )));
    // La vue unifiée compte la file de travail, comme les lignes de comptes.
    fenetre.set_unified_count(snapshot.counts[0] as i32);
    fenetre.set_pending_ops(snapshot.pending_ops as i32);
    fenetre.set_conversation_empty(snapshot.messages.is_empty());

    match &snapshot.search {
        Some(recherche) => {
            fenetre.set_searching(true);
            fenetre.set_search_summary(recherche.summary.as_str().into());
            fenetre.set_search_explanation(recherche.explanation.as_str().into());
            // Le champ n'est pas réécrit : l'utilisateur peut être en train d'y
            // taper la requête suivante pendant que les résultats arrivent.
        }
        None => {
            fenetre.set_searching(false);
            fenetre.set_search_summary(Default::default());
            fenetre.set_search_explanation(Default::default());
        }
    }

    if let Some(message) = snapshot.messages.last() {
        let corps = corps_du_message(services, renderer, message);
        // Les pièces incrustées sont écartées : une image de signature n'est pas un
        // document reçu, et la lister ferait chercher un fichier qui n'existe pas.
        let pieces: Vec<String> = services
            .store
            .visible_attachments(message.id)
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.meta.filename)
            .collect();
        fenetre.set_message(bridge::message_view_rendered(
            message, &corps, &pieces, maintenant,
        ));
    }
}

/// Construit le moteur de rendu des corps de message.
///
/// Le moteur complet n'est retenu que s'il est réellement utilisable : sur une
/// machine sans périphérique graphique compatible, l'application reste pleinement
/// fonctionnelle en texte riche, et le dit une fois au démarrage plutôt que de
/// laisser un panneau vide l'expliquer à chaque message.
pub fn build_renderer() -> iris_htmlview::AdaptiveRenderer {
    let simple = iris_htmlview::AdaptiveRenderer::new(Box::new(iris_htmlview::RichTextRenderer));

    #[cfg(feature = "blitz")]
    {
        if iris_htmlview::BlitzRenderer::is_available() {
            tracing::info!("body rendering: full engine available");
            return simple.with_full_engine(Box::new(iris_htmlview::BlitzRenderer::new(1.0, true)));
        }
        tracing::info!("body rendering: rich text only (no graphics device)");
    }

    simple
}

/// Charge et rend le corps d'un message.
///
/// Le corps n'est lu qu'à l'ouverture, et seulement s'il a déjà été téléchargé :
/// afficher une conversation ne doit jamais attendre le réseau.
fn corps_du_message(
    services: &Services,
    renderer: &dyn iris_htmlview::HtmlRenderer,
    message: &iris_store::StoredMessage,
) -> iris_htmlview::Rendered {
    let apercu = || {
        iris_htmlview::Rendered::Blocks(iris_htmlview::RichText {
            blocks: vec![iris_htmlview::Block::Paragraph(vec![
                iris_htmlview::Inline::plain(message.preview.clone()),
            ])],
            blocked_images: 0,
        })
    };

    let Some(hex) = &message.body_blob else {
        // Pas encore téléchargé : on affiche l'aperçu, qui est toujours là.
        return apercu();
    };

    let corps = iris_types::BlobId::from_hex(hex)
        .and_then(|id| services.blobs.get(id).ok().flatten())
        .unwrap_or_default();

    let texte = String::from_utf8_lossy(&corps);
    let assaini = iris_mime::sanitize(&texte);

    renderer.render(&assaini.html, 800.0).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "rendu du corps en échec");
        apercu()
    })
}

/// Enveloppe permettant de renvoyer un instantané vers la boucle d'interface.
pub fn snapshot_sink(
    fenetre: &AppWindow,
    services: Services,
    renderer: Arc<dyn iris_htmlview::HtmlRenderer>,
    bodies: Arc<BodyLoader>,
) -> impl Fn(Snapshot) + Send + 'static {
    let faible = fenetre.as_weak();
    move |snapshot| {
        // Le corps manquant est demandé avant même de dessiner : l'aperçu s'affiche
        // tout de suite, le texte complet le remplace dès qu'il arrive.
        bodies.request_if_needed(&snapshot);

        let services = services.clone();
        let renderer = Arc::clone(&renderer);
        // `upgrade_in_event_loop` est le passage obligé : toucher la fenêtre depuis
        // un autre fil est une faute que Slint refuse à l'exécution.
        let _ = faible.upgrade_in_event_loop(move |fenetre| {
            apply_snapshot(&fenetre, &services, renderer.as_ref(), &snapshot);
        });
    }
}

/// Télécharge les corps des conversations ouvertes.
///
/// Il retient le dernier fil demandé : sans cette mémoire, chaque instantané
/// relancerait le téléchargement, et rafraîchir la liste martèlerait le serveur.
#[derive(Debug)]
pub struct BodyLoader {
    engine: Arc<iris_sync::SyncEngine>,
    controller: Arc<Controller>,
    runtime: tokio::runtime::Handle,
    demande: std::sync::Mutex<Option<ThreadIdent>>,
}

impl BodyLoader {
    pub fn new(
        engine: Arc<iris_sync::SyncEngine>,
        controller: Arc<Controller>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            engine,
            controller,
            runtime,
            demande: std::sync::Mutex::new(None),
        }
    }

    /// Demande le corps du fil affiché, s'il en manque un.
    pub fn request_if_needed(&self, snapshot: &Snapshot) {
        let Some(thread) = snapshot.selected else {
            return;
        };

        // Rien à faire si tous les corps sont là.
        if snapshot.messages.iter().all(|m| m.body_blob.is_some()) {
            return;
        }

        {
            let mut demande = self.demande.lock().expect("téléchargement empoisonné");
            if *demande == Some(thread) {
                return;
            }
            *demande = Some(thread);
        }

        let engine = Arc::clone(&self.engine);
        let controller = Arc::clone(&self.controller);
        self.runtime.spawn(async move {
            let resultats = engine.fetch_thread_bodies(thread).await;
            let obtenus = resultats.iter().filter(|(_, r)| r.is_ok()).count();
            for (message, resultat) in &resultats {
                if let Err(e) = resultat {
                    tracing::warn!(message = %message, error = %e, "body not downloaded");
                }
            }
            if obtenus > 0 {
                // Un diff ciblé plutôt qu'un rafraîchissement : seul ce fil a changé.
                let mut diff = ViewDiff::default();
                diff.threads.insert(thread);
                controller.send(Request::Diff(Box::new(diff)));
            }
        });
    }

    /// Oublie la dernière demande, pour autoriser un nouvel essai.
    pub fn reset(&self) {
        *self.demande.lock().expect("téléchargement empoisonné") = None;
    }
}

/// Branche la zone de réponse.
pub fn wire_reply(
    fenetre: &AppWindow,
    send: Arc<SendService>,
    selection: Arc<std::sync::Mutex<Option<ThreadIdent>>>,
) {
    let en_cours: Arc<std::sync::Mutex<Option<iris_smtp::SendHandle>>> =
        Arc::new(std::sync::Mutex::new(None));

    {
        let send = Arc::clone(&send);
        let en_cours = Arc::clone(&en_cours);
        let selection = Arc::clone(&selection);
        let faible = fenetre.as_weak();

        fenetre.on_send_reply(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let texte = fenetre.get_reply_text().to_string();
            if texte.trim().is_empty() {
                return;
            }
            let Some(thread) = *selection.lock().expect("sélection") else {
                return;
            };

            let message = match send.compose_reply(thread, &texte, iris_smtp::ReplyScope::Sender) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "composing the reply");
                    fenetre.set_status(format!("Cannot reply: {e}").into());
                    return;
                }
            };

            match send.queue(message) {
                Ok(handle) => {
                    *en_cours.lock().expect("envoi") = Some(handle);
                    // Le bouton devient un bouton d'annulation, au même endroit :
                    // le geste de rattrapage est immédiat.
                    fenetre.set_sending(true);
                    fenetre.set_undo_seconds(send.status().delay_secs as i32);
                    fenetre.set_reply_text(Default::default());
                }
                Err(e) => fenetre.set_status(format!("Send refused: {e}").into()),
            }
        });
    }

    {
        let send = Arc::clone(&send);
        let en_cours = Arc::clone(&en_cours);
        let faible = fenetre.as_weak();

        fenetre.on_cancel_send(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let handle = en_cours.lock().expect("envoi").take();

            match handle.map(|h| send.cancel(h)) {
                Some(true) => {
                    fenetre.set_sending(false);
                    fenetre.set_status("Send cancelled.".into());
                }
                // Déjà parti : le dire franchement plutôt que faire semblant.
                Some(false) => {
                    fenetre.set_sending(false);
                    fenetre.set_status("Too late — the message has gone.".into());
                }
                None => fenetre.set_sending(false),
            }
        });
    }
}

/// Branche l'écran des réglages.
///
/// Chaque changement est appliqué **et enregistré** immédiatement : un panneau de
/// réglages avec un bouton « Valider » invite à se demander si l'on a bien validé,
/// et cette question ne devrait pas exister pour trois cases à cocher.
pub fn wire_settings(
    fenetre: &AppWindow,
    services: &Services,
    controller: Arc<Controller>,
    engine: Arc<iris_sync::SyncEngine>,
    runtime: tokio::runtime::Handle,
    reglages: Settings,
    chemin: std::path::PathBuf,
) {
    let noms = services.themes.names();
    let courant = Arc::new(std::sync::Mutex::new(reglages));

    // L'état initial du panneau.
    {
        let reglages = courant.lock().expect("réglages empoisonnés").clone();
        fenetre.set_themes(ModelRc::new(VecModel::from(
            noms.iter()
                .filter_map(|n| services.themes.get(n).map(|t| theme_swatch(n, &t)))
                .collect::<Vec<_>>(),
        )));
        fenetre
            .set_active_theme(noms.iter().position(|n| *n == reglages.theme).unwrap_or(0) as i32);
        fenetre.set_density(reglages.density.index() as i32);
        fenetre.set_reply_marks_waiting(reglages.automation.reply_marks_waiting);
        fenetre.set_new_message_reopens(reglages.automation.new_message_reopens);
        fenetre.set_follow_up_enabled(reglages.automation.follow_up_enabled);
        fenetre.set_follow_up_days(reglages.automation.follow_up_days as i32);
    }

    let enregistrer = {
        let chemin = chemin.clone();
        move |reglages: &Settings| {
            if let Err(e) = reglages.save(&chemin) {
                tracing::warn!(error = %e, "saving the settings");
            }
        }
    };

    // --- Le thème ---
    {
        let noms = noms.clone();
        let themes = Arc::clone(&services.themes);
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let faible = fenetre.as_weak();

        fenetre.on_theme_chosen(move |index| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(nom) = noms.get(index as usize) else {
                return;
            };

            let theme = match themes.set_active(nom) {
                Ok(t) => t,
                Err(e) => {
                    // Un thème qui refuse de se charger laisse l'ancien en place :
                    // mieux vaut l'apparence précédente qu'un écran à moitié peint.
                    tracing::warn!(theme = %nom, error = %e, "theme refused");
                    fenetre.set_status(format!("Theme \"{nom}\" could not be read.").into());
                    return;
                }
            };

            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.theme = nom.clone();
            appliquer_apparence(&fenetre, &theme, reglages.density);
            fenetre.set_active_theme(index);
            enregistrer(&reglages);
        });
    }

    // --- La densité ---
    {
        let themes = Arc::clone(&services.themes);
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let faible = fenetre.as_weak();

        fenetre.on_density_chosen(move |index| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(densite) = Density::from_index(index as usize) else {
                return;
            };

            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.density = densite;
            appliquer_apparence(&fenetre, &themes.active(), densite);
            fenetre.set_density(index);
            enregistrer(&reglages);
        });
    }

    // --- Les automatismes ---
    {
        let courant = Arc::clone(&courant);
        let faible = fenetre.as_weak();

        fenetre.on_automation_changed(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };

            let automatismes = iris_types::AutomationSettings {
                reply_marks_waiting: fenetre.get_reply_marks_waiting(),
                new_message_reopens: fenetre.get_new_message_reopens(),
                follow_up_enabled: fenetre.get_follow_up_enabled(),
                follow_up_days: fenetre.get_follow_up_days().clamp(1, 365) as u16,
            };

            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.automation = automatismes;
            enregistrer(&reglages);

            // Les deux moteurs qui obéissent à ces réglages : celui des actions de
            // l'utilisateur, et celui du temps. Les oublier ferait un panneau qui
            // sauvegarde bien et ne change rien.
            controller.send(Request::SetAutomation(automatismes));
            let engine = Arc::clone(&engine);
            runtime.spawn(async move {
                engine.set_automation(automatismes);
            });
        });
    }
}

/// A theme, reduced to what the picker draws.
///
/// Three colours and a label. Showing the theme is what stops people trying each one
/// to find out what it looks like, and every trial repaints the whole window.
fn theme_swatch(name: &str, theme: &iris_theme::Theme) -> iris_ui::ThemeSwatchData {
    let colour = |c: iris_theme::Color| slint::Color::from_argb_u8(c.a, c.r, c.g, c.b);

    iris_ui::ThemeSwatchData {
        name: name.into(),
        label: if theme.label.trim().is_empty() {
            name.into()
        } else {
            theme.label.as_str().into()
        },
        // The surface is drawn over the background, so a translucent surface shown on
        // its own would be nearly invisible: it is flattened against the ground first.
        background: colour(theme.color.background),
        surface: colour(theme.color.surface_high),
        accent: colour(theme.color.accent),
        text: colour(theme.color.text),
    }
}

/// Applique thème et densité aux jetons de l'interface.
pub fn appliquer_apparence(fenetre: &AppWindow, theme: &iris_theme::Theme, densite: Density) {
    let tokens = fenetre.global::<Tokens>();
    bridge::apply_theme(&tokens, theme);
    // La densité multiplie la hauteur du thème au lieu de la remplacer : un thème
    // aux lignes hautes reste plus aéré que les autres à densité égale.
    tokens.set_row_height(theme.density.row_height * densite.factor());
}

/// Branche l'écran d'ajout de compte.
///
/// La découverte d'abord : dans la grande majorité des cas, l'adresse et le mot de
/// passe suffisent. Les champs de serveur n'apparaissent que si elle échoue — ou si
/// l'utilisateur les demande, parce que quelqu'un qui sait déjà que son serveur est
/// exotique n'a pas à attendre qu'on se trompe.
pub fn wire_account_setup(
    fenetre: &AppWindow,
    services: &Services,
    controller: Arc<Controller>,
    runtime: tokio::runtime::Handle,
) {
    let oauth_reglages = Arc::clone(&services.oauth);
    // --- Passer à la main sans attendre l'échec ---
    {
        let faible = fenetre.as_weak();
        fenetre.on_add_account_manual_requested(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            prefill_manual(&fenetre);
        });
    }

    // --- La découverte ---
    {
        let store = Arc::clone(&services.store);
        let secrets = Arc::clone(&services.secrets);
        let engine = Arc::clone(&services.engine);
        let services_ui = services.clone();
        let controller = Arc::clone(&controller);
        let runtime_ajout = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_add_account_discover(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let email = fenetre.get_new_email().to_string();
            let motdepasse = fenetre.get_new_password().to_string();

            // Le mot de passe n'est exigé qu'après la découverte : un compte Google
            // n'en a pas, et le réclamer d'avance apprendrait à l'utilisateur à
            // taper son mot de passe principal dans une application tierce.
            if let Err(message) = valider_adresse(&email) {
                fenetre.set_add_account_error(message.into());
                return;
            }

            fenetre.set_add_account_busy(true);
            fenetre.set_add_account_error(Default::default());
            fenetre.set_add_account_hint("Recherche de la configuration…".into());

            let store = Arc::clone(&store);
            let secrets = Arc::clone(&secrets);
            let engine = Arc::clone(&engine);
            let services_ui = services_ui.clone();
            let controller = Arc::clone(&controller);
            let faible = fenetre.as_weak();

            let oauth = Arc::clone(&oauth_reglages);
            runtime_ajout.spawn(async move {
                let resultat = ajouter(&store, secrets, &oauth, &email, &motdepasse, now()).await;

                // Le compte créé doit entrer dans l'ordonnanceur tout de suite,
                // sinon rien n'arrive avant le prochain démarrage.
                if resultat.is_ok() {
                    if let Err(e) = engine.load_accounts(now()).await {
                        tracing::warn!(error = %e, "chargement du compte ajouté");
                    }
                }

                let _ = faible.upgrade_in_event_loop(move |fenetre| {
                    fenetre.set_add_account_busy(false);
                    match resultat {
                        Ok(compte) => {
                            fenetre.set_add_account_open(false);
                            fenetre.set_new_email(Default::default());
                            fenetre.set_new_password(Default::default());
                            fenetre.set_status(
                                format!("{} ajouté ({}).", compte.email, compte.source.describe())
                                    .into(),
                            );
                            refresh_accounts(&fenetre, &services_ui, &[]);
                            controller.send(Request::Bootstrap);
                        }
                        // L'échec bascule l'écran en configuration manuelle plutôt
                        // que de renvoyer l'utilisateur à un message d'erreur : ce
                        // qu'il lui faut à cet instant, ce sont les champs.
                        Err(e) => {
                            fenetre.set_add_account_error(
                                format!("No configuration found: {e}").into(),
                            );
                            prefill_manual(&fenetre);
                        }
                    }
                });
            });
        });
    }

    // --- L'enregistrement manuel ---
    {
        let store = Arc::clone(&services.store);
        let secrets = Arc::clone(&services.secrets);
        let engine = Arc::clone(&services.engine);
        let services_ui = services.clone();
        let controller = Arc::clone(&controller);
        let runtime_manuel = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_add_account_save(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let email = fenetre.get_new_email().to_string();
            let motdepasse = fenetre.get_new_password().to_string();

            if let Err(message) = valider_saisie(&email, &motdepasse) {
                fenetre.set_add_account_error(message.into());
                return;
            }

            let config = match config_saisie(&fenetre, &email) {
                Ok(c) => c,
                Err(message) => {
                    fenetre.set_add_account_error(message.into());
                    return;
                }
            };

            match crate::accounts::add_account_manual(
                &store,
                secrets.as_ref(),
                &config,
                &motdepasse,
                None,
                now(),
            ) {
                Ok(_) => {
                    fenetre.set_add_account_open(false);
                    fenetre.set_add_account_manual(false);
                    fenetre.set_add_account_error(Default::default());
                    fenetre.set_new_email(Default::default());
                    fenetre.set_new_password(Default::default());
                    fenetre.set_status(format!("{} added.", config.email).into());
                    refresh_accounts(&fenetre, &services_ui, &[]);
                    controller.send(Request::Bootstrap);

                    let engine = Arc::clone(&engine);
                    runtime_manuel.spawn(async move {
                        if let Err(e) = engine.load_accounts(now()).await {
                            tracing::warn!(error = %e, "chargement du compte ajouté");
                        }
                    });
                }
                Err(e) => {
                    fenetre.set_add_account_error(format!("Could not add the account: {e}").into())
                }
            }
        });
    }
}

/// Ajoute un compte : découverte, puis la porte d'entrée qui convient.
///
/// Un fournisseur d'identité passe par le navigateur ; tout le reste par le mot de
/// passe. Le choix n'appartient pas à l'utilisateur : il appartient au serveur, et
/// lui demander de deviner serait lui demander de connaître la politique de son
/// hébergeur.
async fn ajouter(
    store: &iris_store::Store,
    secrets: Arc<dyn iris_secrets::SecretStore>,
    oauth: &Arc<std::sync::RwLock<crate::oauth::OAuthSettings>>,
    email: &str,
    motdepasse: &str,
    maintenant: iris_types::Timestamp,
) -> iris_types::Result<crate::accounts::AddedAccount> {
    let decouverte = crate::accounts::discover(email).await?;
    let config = decouverte.config.clone();

    let fournisseur = match config.auth {
        iris_discover::Auth::OAuthGoogle => Some(iris_oauth::Provider::Google),
        iris_discover::Auth::OAuthMicrosoft => Some(iris_oauth::Provider::Microsoft),
        iris_discover::Auth::Password => None,
    };

    let id = match fournisseur {
        Some(fournisseur) => {
            let reglages = oauth.read().expect("réglages OAuth empoisonnés").clone();
            if !reglages.is_configured(fournisseur) {
                // Le dire, plutôt que d'ouvrir un navigateur vers une page d'erreur
                // du fournisseur que personne ne saura interpréter.
                return Err(iris_types::Error::Config(format!(
                    "{} exige une connexion par navigateur, et aucun identifiant client                      n'est configuré pour ce fournisseur",
                    config.provider.as_deref().unwrap_or("ce compte")
                )));
            }

            crate::oauth::authorize(
                Arc::clone(&secrets),
                &reglages,
                fournisseur,
                &config.email,
                maintenant,
            )
            .await?;

            crate::accounts::add_account_oauth(store, &config, None, maintenant)?
        }
        None => {
            if motdepasse.is_empty() {
                return Err(iris_types::Error::Config("The password is empty.".into()));
            }
            if store.account_by_email(&config.email)?.is_some() {
                return Err(iris_types::Error::Config(format!(
                    "le compte « {} » existe déjà",
                    config.email
                )));
            }
            crate::accounts::add_account_manual(
                store,
                secrets.as_ref(),
                &config,
                motdepasse,
                None,
                maintenant,
            )?
        }
    };

    Ok(crate::accounts::AddedAccount {
        id,
        email: config.email.clone(),
        needs_review: !decouverte.source.is_authoritative(),
        source: decouverte.source,
        config,
    })
}

/// Bascule l'écran en configuration manuelle, champs préremplis.
fn prefill_manual(fenetre: &AppWindow) {
    let defauts = crate::accounts::manual_defaults(fenetre.get_new_email().as_str());

    fenetre.set_add_account_manual(true);
    fenetre.set_add_account_hint("Check the servers — they are guessed from your domain.".into());
    // Ce que l'utilisateur a déjà tapé n'est pas écrasé : une bascule qui efface la
    // saisie punit celui qui avait deviné juste.
    if fenetre.get_new_imap_host().is_empty() {
        fenetre.set_new_imap_host(defauts.imap_host.as_str().into());
        fenetre.set_new_imap_port(defauts.imap_port.to_string().into());
    }
    if fenetre.get_new_smtp_host().is_empty() {
        fenetre.set_new_smtp_host(defauts.smtp_host.as_str().into());
        fenetre.set_new_smtp_port(defauts.smtp_port.to_string().into());
    }
}

/// Vérifie l'adresse, sans réseau.
fn valider_adresse(email: &str) -> std::result::Result<(), String> {
    if !email.contains('@') || email.trim().len() < 3 {
        return Err("That does not look like an email address.".into());
    }
    Ok(())
}

/// Vérifie ce qu'exige la configuration manuelle : une adresse et un mot de passe.
fn valider_saisie(email: &str, motdepasse: &str) -> std::result::Result<(), String> {
    valider_adresse(email)?;
    if motdepasse.is_empty() {
        return Err("The password is empty.".into());
    }
    Ok(())
}

/// Compose la configuration à partir des champs saisis.
fn config_saisie(
    fenetre: &AppWindow,
    email: &str,
) -> std::result::Result<iris_discover::ServerConfig, String> {
    let port = |texte: slint::SharedString, quoi: &str| {
        texte
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| format!("The {quoi} port is not a valid number."))
    };

    let imap_host = fenetre.get_new_imap_host().trim().to_string();
    let smtp_host = fenetre.get_new_smtp_host().trim().to_string();
    if imap_host.is_empty() || smtp_host.is_empty() {
        return Err("Both servers are required.".into());
    }

    let transport = |chiffre: bool| {
        if chiffre {
            iris_discover::Transport::Tls
        } else {
            iris_discover::Transport::Plain
        }
    };

    Ok(iris_discover::ServerConfig {
        provider: None,
        email: email.trim().to_lowercase(),
        imap_host,
        imap_port: port(fenetre.get_new_imap_port(), "IMAP")?,
        imap_transport: transport(fenetre.get_new_imap_tls()),
        smtp_host,
        smtp_port: port(fenetre.get_new_smtp_port(), "SMTP")?,
        smtp_transport: transport(fenetre.get_new_smtp_tls()),
        auth: iris_discover::Auth::Password,
        note: None,
    })
}

/// Branche l'enregistrement des pièces jointes.
///
/// Le fichier va dans le dossier de téléchargements du système, sans boîte de
/// dialogue : à ce stade l'utilisateur a déjà cliqué sur ce qu'il voulait, et lui
/// demander où le mettre ajouterait un geste à une décision déjà prise. La barre
/// d'état dit où le fichier a atterri.
pub fn wire_attachments(
    fenetre: &AppWindow,
    services: &Services,
    selection: Arc<std::sync::Mutex<Option<ThreadIdent>>>,
) {
    let services = services.clone();
    let faible = fenetre.as_weak();

    fenetre.on_save_attachment(move |rang| {
        let Some(fenetre) = faible.upgrade() else {
            return;
        };
        let Some(thread) = *selection.lock().expect("sélection empoisonnée") else {
            return;
        };

        match enregistrer_piece(&services, thread, rang as usize) {
            Ok(chemin) => fenetre.set_status(format!("Saved to {}", chemin.display()).into()),
            Err(e) => fenetre.set_status(format!("Could not save: {e}").into()),
        }
    });
}

/// Écrit une pièce jointe sur le disque et rend son chemin.
fn enregistrer_piece(
    services: &Services,
    thread: ThreadIdent,
    rang: usize,
) -> iris_types::Result<std::path::PathBuf> {
    let messages = services.store.thread_messages(thread)?;
    let message = messages
        .last()
        .ok_or_else(|| iris_types::Error::other("conversation vide"))?;

    let pieces = services.store.visible_attachments(message.id)?;
    let piece = pieces
        .get(rang)
        .ok_or_else(|| iris_types::Error::other("pièce jointe introuvable"))?;

    // Les octets viennent du message brut, jamais d'une copie : c'est ce qui évite
    // de stocker deux fois toutes les pièces jointes de la boîte.
    let blob = message
        .body_blob
        .as_deref()
        .and_then(iris_types::BlobId::from_hex)
        .ok_or_else(|| iris_types::Error::other("le corps n'est pas encore téléchargé"))?;

    let brut = services
        .blobs
        .get(blob)?
        .ok_or_else(|| iris_types::Error::other("contenu absent du cache"))?;

    let octets = iris_mime::attachment_bytes(&brut, piece.index)
        .ok_or_else(|| iris_types::Error::other("pièce jointe absente du message"))?;

    let destination = chemin_libre(&dossier_telechargements(), &piece.meta.filename);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&destination, octets)?;
    Ok(destination)
}

/// Le dossier de téléchargements de l'utilisateur, ou son dossier personnel.
fn dossier_telechargements() -> std::path::PathBuf {
    directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
        .or_else(|| directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Trouve un nom libre dans le dossier.
///
/// Écraser un fichier existant du même nom ferait perdre à l'utilisateur la première
/// version sans le prévenir — deux factures s'appellent souvent « facture.pdf ».
fn chemin_libre(dossier: &std::path::Path, nom: &str) -> std::path::PathBuf {
    let nom = nom_sur(nom);
    let candidat = dossier.join(&nom);
    if !candidat.exists() {
        return candidat;
    }

    let chemin = std::path::Path::new(&nom);
    let tronc = chemin
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("piece-jointe");
    let extension = chemin.extension().and_then(|s| s.to_str());

    for n in 2..1000 {
        let essai = match extension {
            Some(ext) => dossier.join(format!("{tronc} ({n}).{ext}")),
            None => dossier.join(format!("{tronc} ({n})")),
        };
        if !essai.exists() {
            return essai;
        }
    }
    candidat
}

/// Rend un nom de fichier inoffensif.
///
/// Le nom vient d'un message reçu : rien n'empêche un expéditeur d'y mettre
/// « ../../autre-chose ». On ne garde que le dernier segment, débarrassé des
/// séparateurs — un fichier écrit hors du dossier choisi serait une faille, pas une
/// commodité.
fn nom_sur(nom: &str) -> String {
    let dernier = nom.rsplit(['/', '\\']).next().unwrap_or(nom).trim();
    let nettoye: String = dernier
        .chars()
        .filter(|c| !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'))
        .collect();

    match nettoye.trim_matches('.').trim() {
        "" => "piece-jointe".to_string(),
        propre => propre.to_string(),
    }
}

/// Wires the compose window.
///
/// It shares the outbox with replies, so the ten-second window to change your mind
/// works the same way here. Reimplementing the delay would give the application two
/// answers to "can I still stop this?", and only one of them would be right.
pub fn wire_compose(
    fenetre: &AppWindow,
    services: &Services,
    send: Arc<SendService>,
    account: iris_types::AccountId,
) {
    let pending: Arc<std::sync::Mutex<Option<iris_smtp::SendHandle>>> =
        Arc::new(std::sync::Mutex::new(None));

    // Which mailbox this leaves from, shown from the start: with a hundred accounts,
    // sending from the wrong one is the mistake that costs.
    if let Ok(Some(compte)) = services.store.account(account) {
        fenetre.set_compose_sender(format!("from {}", compte.email).into());
    }

    {
        let send = Arc::clone(&send);
        let pending = Arc::clone(&pending);
        let faible = fenetre.as_weak();

        fenetre.on_compose_send(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };

            let message = match send.compose_new(
                account,
                fenetre.get_compose_to().as_str(),
                fenetre.get_compose_subject().as_str(),
                fenetre.get_compose_body().as_str(),
            ) {
                Ok(message) => message,
                Err(e) => {
                    fenetre.set_compose_error(e.to_string().into());
                    return;
                }
            };

            // A message with no subject leaves anyway. Refusing it would be the
            // application deciding what matters in someone else's correspondence.
            match send.queue(message) {
                Ok(handle) => {
                    *pending.lock().expect("poisoned send") = Some(handle);
                    fenetre.set_compose_error(Default::default());
                    fenetre.set_compose_sending(true);
                    fenetre.set_compose_undo_seconds(send.status().delay_secs as i32);
                }
                Err(e) => fenetre.set_compose_error(format!("Send refused: {e}").into()),
            }
        });
    }

    {
        let send = Arc::clone(&send);
        let pending = Arc::clone(&pending);
        let faible = fenetre.as_weak();

        fenetre.on_compose_cancel(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let handle = pending.lock().expect("poisoned send").take();

            match handle.map(|h| send.cancel(h)) {
                Some(true) => {
                    fenetre.set_compose_sending(false);
                    fenetre.set_status("Send cancelled — your message is still here.".into());
                }
                // Already gone: say so plainly rather than pretend.
                Some(false) => {
                    fenetre.set_compose_sending(false);
                    fenetre.set_compose_open(false);
                    clear_compose(&fenetre);
                    fenetre.set_status("Too late — the message has gone.".into());
                }
                None => fenetre.set_compose_sending(false),
            }
        });
    }
}

/// Empties the compose window once a message is safely away.
pub fn clear_compose(fenetre: &AppWindow) {
    fenetre.set_compose_to(Default::default());
    fenetre.set_compose_subject(Default::default());
    fenetre.set_compose_body(Default::default());
    fenetre.set_compose_error(Default::default());
    fenetre.set_compose_sending(false);
}

/// Wires the modules screen: rules and plugins.
///
/// The list is rebuilt from the store after every change rather than patched in
/// place. It holds tens of rows, and a screen that recomputes itself cannot show
/// something the database does not contain.
pub fn wire_modules(
    fenetre: &AppWindow,
    services: &Services,
    plugins: Vec<crate::modules::PluginView>,
) {
    fenetre.set_plugin_folder(services.paths.plugins().display().to_string().into());
    fenetre.set_plugins(ModelRc::new(VecModel::from(
        plugins.iter().map(plugin_row).collect::<Vec<_>>(),
    )));
    refresh_rules(fenetre, services);

    // --- Enable or disable ---
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_rule_toggled(move |id, enabled| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Ok(rules) = services.store.rules() else {
                return;
            };
            let Some(mut rule) = rules.into_iter().find(|r| r.id == id.as_str()) else {
                return;
            };

            rule.enabled = enabled;
            match services.store.upsert_rule(&rule) {
                Ok(()) => refresh_rules(&fenetre, &services),
                Err(e) => fenetre.set_status(format!("Could not save the rule: {e}").into()),
            }
        });
    }

    // --- Delete ---
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_rule_removed(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            match services.store.delete_rule(id.as_str()) {
                Ok(true) => {
                    fenetre.set_status("Rule deleted.".into());
                    refresh_rules(&fenetre, &services);
                }
                Ok(false) => {}
                Err(e) => fenetre.set_status(format!("Could not delete the rule: {e}").into()),
            }
        });
    }

    // --- Add ---
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_rule_added(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let position = services.store.rules().map(|r| r.len() as u32).unwrap_or(0);

            let rule = match crate::modules::quick_rule(
                fenetre.get_new_rule_name().as_str(),
                fenetre.get_new_rule_sender().as_str(),
                position,
            ) {
                Ok(rule) => rule,
                Err(message) => {
                    fenetre.set_simulation(message.into());
                    return;
                }
            };

            match services.store.upsert_rule(&rule) {
                Ok(()) => {
                    fenetre.set_new_rule_name(Default::default());
                    fenetre.set_new_rule_sender(Default::default());
                    // The dry run runs on its own after adding: the first question
                    // anyone has about a new rule is what it would have caught.
                    show_simulation(&fenetre, &services, &rule.id);
                    refresh_rules(&fenetre, &services);
                }
                Err(e) => fenetre.set_simulation(format!("Could not save: {e}").into()),
            }
        });
    }

    // --- Dry run ---
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_rule_simulated(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            show_simulation(&fenetre, &services, id.as_str());
        });
    }

    // --- Reload the plugin folder ---
    {
        let faible = fenetre.as_weak();
        fenetre.on_plugins_reloaded(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            // Loading WebAssembly into a running host mid-session is a restart-shaped
            // problem; saying so is better than pretending to reload and doing
            // nothing.
            fenetre
                .set_status("Plugins are loaded at startup — restart to pick up changes.".into());
        });
    }
}

/// A rule row, ready to draw.
///
/// The conversion lives here rather than in the bridge because the view types belong
/// to the application: the interface crate does not know what a rule is, and should
/// not have to.
fn rule_row(view: &crate::modules::RuleView) -> iris_ui::RuleRowData {
    iris_ui::RuleRowData {
        id: view.id.as_str().into(),
        name: view.name.as_str().into(),
        enabled: view.enabled,
        summary: view.summary.as_str().into(),
        applied: view.applied.min(i32::MAX as u64) as i32,
    }
}

/// A plugin row, ready to draw.
fn plugin_row(view: &crate::modules::PluginView) -> iris_ui::PluginRowData {
    iris_ui::PluginRowData {
        id: view.id.as_str().into(),
        name: view.name.as_str().into(),
        version: view.version.as_str().into(),
        description: view.description.as_str().into(),
        permissions: view.permissions.as_str().into(),
        disabled_reason: view.disabled_reason.as_str().into(),
    }
}

/// Rebuilds the rule list from the store.
fn refresh_rules(fenetre: &AppWindow, services: &Services) {
    let views = crate::modules::rule_views(&services.store).unwrap_or_default();
    fenetre.set_rules(ModelRc::new(VecModel::from(
        views.iter().map(rule_row).collect::<Vec<_>>(),
    )));
}

/// Runs one rule over recent history and reports what it would have touched.
fn show_simulation(fenetre: &AppWindow, services: &Services, id: &str) {
    let rules = match services.store.rules() {
        Ok(r) => r,
        Err(e) => {
            fenetre.set_simulation(format!("Could not read the rules: {e}").into());
            return;
        }
    };

    let Some(row) = rules.into_iter().find(|r| r.id == id) else {
        return;
    };
    let rule: iris_rules::Rule = match serde_json::from_str(&row.definition) {
        Ok(rule) => rule,
        Err(e) => {
            fenetre.set_simulation(format!("This rule cannot be read: {e}").into());
            return;
        }
    };

    match services.engine.simulate_rules(Some(&rule), 5_000, now()) {
        Ok(simulation) => {
            let mut text = simulation.summary();
            // A count on its own is not evidence. Two examples are.
            for hit in simulation.sample.iter().take(2) {
                text.push_str(&format!("\n  · {} — {}", hit.from, hit.subject));
            }
            fenetre.set_simulation(text.into());
        }
        Err(e) => fenetre.set_simulation(format!("Dry run failed: {e}").into()),
    }
}

/// Modèle vide, pour initialiser une liste avant le premier instantané.
pub fn empty_model<T: Clone + 'static>() -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(Vec::<T>::new())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_ui::commands::builtin_commands;

    #[test]
    fn chaque_commande_est_traitee_par_le_repartiteur() {
        // Une commande sans traitement serait une entrée de palette qui ne fait rien,
        // ce qui est pire que son absence.
        for commande in builtin_commands() {
            match commande.kind {
                CommandKind::Thread(_)
                | CommandKind::SwitchTab(_)
                | CommandKind::SelectAccount(_)
                | CommandKind::UnifiedView
                | CommandKind::Undo
                | CommandKind::Search
                | CommandKind::AddAccount
                | CommandKind::Modules
                | CommandKind::Compose
                | CommandKind::Plugin { .. }
                | CommandKind::Quit => {}
                CommandKind::Settings | CommandKind::Reload => {}
            }
        }
    }

    fn compte(id: i64, email: &str) -> iris_store::Account {
        iris_store::Account {
            id: iris_types::AccountId(id),
            email: email.into(),
            display_name: email.into(),
            imap_host: "i".into(),
            imap_port: 993,
            imap_tls: true,
            smtp_host: "s".into(),
            smtp_port: 465,
            smtp_tls: true,
            auth: iris_store::AuthKind::Password,
            group: None,
            enabled: true,
            pinned: false,
            created_at: iris_types::Timestamp::EPOCH,
            last_activity_at: iris_types::Timestamp::EPOCH,
        }
    }

    fn ensemble(ids: &[i64]) -> std::collections::BTreeSet<iris_types::AccountId> {
        ids.iter().map(|i| iris_types::AccountId(*i)).collect()
    }

    #[test]
    fn sans_compte_suspendu_la_barre_ne_dit_rien() {
        let comptes = vec![compte(1, "a@x.fr")];
        assert!(message_suspension(&comptes, &ensemble(&[])).is_none());
    }

    #[test]
    fn un_compte_suspendu_est_nomme() {
        let comptes = vec![compte(1, "a@x.fr"), compte(2, "b@x.fr")];
        let message = message_suspension(&comptes, &ensemble(&[2])).unwrap();
        assert!(message.starts_with("b@x.fr paused"));
        assert!(message.contains("try again"));
    }

    #[test]
    fn au_dela_de_trois_comptes_on_compte_au_lieu_d_enumerer() {
        // Une liste de quarante adresses dans une barre d'état n'informe personne.
        let comptes: Vec<_> = (1..=5).map(|i| compte(i, &format!("c{i}@x.fr"))).collect();
        let message = message_suspension(&comptes, &ensemble(&[1, 2, 3, 4, 5])).unwrap();
        assert!(message.starts_with("5 accounts paused"), "got: {message}");
    }

    #[test]
    fn un_compte_suspendu_inconnu_du_store_est_quand_meme_signale() {
        // Sinon la panne resterait muette au moment où elle est la plus étrange.
        let message = message_suspension(&[], &ensemble(&[7])).unwrap();
        assert!(message.starts_with("1 account paused"), "got: {message}");
    }

    #[test]
    fn un_nom_de_fichier_hostile_est_ramene_a_son_dernier_segment() {
        // Rien n'empêche un expéditeur d'appeler sa pièce jointe « ../../passwd ».
        assert_eq!(nom_sur("../../etc/passwd"), "passwd");
        assert_eq!(nom_sur("..\\..\\windows\\system32\\x.dll"), "x.dll");
        assert_eq!(nom_sur("devis.pdf"), "devis.pdf");
    }

    #[test]
    fn un_nom_vide_ou_uniquement_ponctue_recoit_un_nom_de_secours() {
        assert_eq!(nom_sur(""), "piece-jointe");
        assert_eq!(nom_sur("..."), "piece-jointe");
        assert_eq!(nom_sur("   "), "piece-jointe");
        assert_eq!(nom_sur("/"), "piece-jointe");
    }

    #[test]
    fn les_caracteres_interdits_sont_retires() {
        assert_eq!(nom_sur("fact:ure?.pdf"), "facture.pdf");
    }

    #[test]
    fn un_fichier_existant_n_est_pas_ecrase() {
        // Deux factures s'appellent souvent « facture.pdf ».
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("facture.pdf"), b"premiere").unwrap();

        let libre = chemin_libre(dir.path(), "facture.pdf");
        assert_eq!(libre.file_name().unwrap(), "facture (2).pdf");
    }

    #[test]
    fn le_premier_enregistrement_garde_son_nom() {
        let dir = tempfile::tempdir().unwrap();
        let libre = chemin_libre(dir.path(), "devis.pdf");
        assert_eq!(libre.file_name().unwrap(), "devis.pdf");
    }

    #[test]
    fn un_fichier_sans_extension_est_numerote_aussi() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("LISEZMOI"), b"x").unwrap();

        let libre = chemin_libre(dir.path(), "LISEZMOI");
        assert_eq!(libre.file_name().unwrap(), "LISEZMOI (2)");
    }

    #[test]
    fn le_carnet_accueille_une_commande_de_plugin() {
        // Un plugin déclare ses commandes en s'exécutant, donc après que la palette
        // a été branchée : une liste figée obligerait à redémarrer.
        let carnet = CommandBook::new();
        let avant = carnet.len();

        let label = carnet.add_plugin("tri", r#"{"id":"vider","label":"Vider"}"#);
        assert_eq!(label.as_deref(), Some("Vider"));
        assert_eq!(carnet.len(), avant + 1);
        assert!(carnet.find("plugin:tri:vider").is_some());
    }

    #[test]
    fn redeclarer_une_commande_la_remplace() {
        // Un plugin qui se réinitialise ne doit pas laisser deux entrées identiques.
        let carnet = CommandBook::new();
        carnet.add_plugin("tri", r#"{"id":"x","label":"Ancien"}"#);
        carnet.add_plugin("tri", r#"{"id":"x","label":"Nouveau"}"#);

        assert_eq!(carnet.find("plugin:tri:x").unwrap().label, "Nouveau");
        assert_eq!(carnet.filter("Ancien", false).len(), 0);
    }

    #[test]
    fn une_declaration_invalide_n_entre_pas_dans_le_carnet() {
        let carnet = CommandBook::new();
        let avant = carnet.len();
        assert!(carnet.add_plugin("tri", "n'importe quoi").is_none());
        assert_eq!(carnet.len(), avant);
    }

    #[test]
    fn une_commande_de_plugin_se_reconnait() {
        let carnet = CommandBook::new();
        carnet.add_plugin("tri", r#"{"id":"vider","label":"Vider"}"#);
        let commande = carnet.find("plugin:tri:vider").unwrap();

        let (plugin, spec) = plugin_command(&commande.kind).expect("une commande de plugin");
        assert_eq!(plugin, "tri");
        assert!(spec.contains("vider"));

        let ordinaire = carnet.find("app.quit").unwrap();
        assert!(plugin_command(&ordinaire.kind).is_none());
    }

    #[test]
    fn une_adresse_sans_arobase_est_refusee_sans_reseau() {
        // Interroger un serveur DNS pour découvrir que « bob » n'est pas une adresse
        // ferait attendre pour rien.
        assert!(valider_saisie("bob", "x").is_err());
        assert!(valider_saisie("bob@exemple.fr", "x").is_ok());
    }

    #[test]
    fn un_mot_de_passe_vide_est_refuse() {
        let erreur = valider_saisie("bob@exemple.fr", "").unwrap_err();
        assert!(erreur.contains("password"));
    }

    #[test]
    fn les_onglets_correspondent_aux_etats() {
        for (index, etat) in WorkflowState::ALL.iter().enumerate() {
            assert_eq!(WorkflowState::from_i64(index as i64), Some(*etat));
            assert_eq!(etat.as_i64(), index as i64);
        }
    }
}
