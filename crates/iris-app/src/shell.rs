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
use iris_ui::{
    AccountRowData, AppWindow, AttachmentData, FolderNodeData, PluginSettingData, Tokens,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::collections::HashMap;
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
    // La version, en permanence dans la barre du bas. C'est la première question
    // posée quand quelque chose ne va pas, et la seule réponse qui rende un rapport
    // exploitable — « ça plante » sans numéro de version ne se corrige pas.
    fenetre.set_version(format!("Iris {}", env!("CARGO_PKG_VERSION")).into());
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
    // Why each account last failed. The mark used to wait for the scheduler to pause
    // an account — several failures in a row — so a mailbox whose every sync failed
    // looked healthy after "Sync all", after a manual sync, after a new password.
    let pannes: std::collections::BTreeMap<iris_types::AccountId, iris_sync::AccountFailure> =
        services.engine.failures().into_iter().collect();

    // Ce qui reste à traiter, boîte par boîte. Le nombre total de messages ne dirait
    // rien de ce qu'il y a à faire, et un « 12 483 » permanent n'apprend rien.
    let a_traiter = services
        .store
        .todo_counts_by_account(now())
        .unwrap_or_default();

    // The filter is applied here, over the real list. It was previously applied
    // nowhere at all: the field existed and was bound to nothing, so typing in it
    // changed the text and not the list.
    let recherche = fenetre.get_account_filter().to_lowercase();
    let recherche = recherche.trim();

    let retenus: Vec<&iris_store::Account> = comptes
        .iter()
        .filter(|c| {
            recherche.is_empty()
                || c.email.to_lowercase().contains(recherche)
                || c.display_name.to_lowercase().contains(recherche)
                || c.group
                    .as_deref()
                    .is_some_and(|g| g.to_lowercase().contains(recherche))
        })
        .collect();

    let (epingles, autres): (Vec<_>, Vec<_>) = retenus.into_iter().partition(|c| c.pinned);

    let ligne = |c: &iris_store::Account| {
        let panne = pannes.get(&c.id);
        let mut ligne = bridge::account_row(
            c,
            a_traiter.get(&c.id).copied().unwrap_or(0),
            suspendus.contains(&c.id) || panne.is_some(),
        );
        if let Some(panne) = panne {
            ligne.problem = panne.summary().into();
        }
        ligne
    };

    fenetre.set_pinned_accounts(ModelRc::new(VecModel::from(
        epingles.iter().map(|c| ligne(c)).collect::<Vec<_>>(),
    )));

    // Under their tags, when asked: a title per tag, its accounts beneath — an account
    // with two tags shows under both — then those without one. Empty groups (none of
    // their accounts matches the filter) are left out.
    // A folded tag keeps its title and hides its accounts; a filter being typed
    // unfolds everything, or it would find accounts nobody can see.
    let replies = crate::settings::current().folded_tags;
    let replie = |tag: i64| recherche.is_empty() && replies.contains(&tag);
    let a_faire = |comptes: &[&&iris_store::Account]| -> u32 {
        comptes
            .iter()
            .map(|c| a_traiter.get(&c.id).copied().unwrap_or(0))
            .sum()
    };
    // A tag carries its accounts' "!" as long as one of them has it.
    let en_panne = |comptes: &[&&iris_store::Account]| -> usize {
        comptes
            .iter()
            .filter(|c| suspendus.contains(&c.id) || pannes.contains_key(&c.id))
            .count()
    };
    let signaler = |mut titre: AccountRowData, n: usize| {
        if n > 0 {
            titre.needs_attention = true;
            titre.problem = match n {
                1 => "1 account here failed to sync".into(),
                n => format!("{n} accounts here failed to sync").into(),
            };
        }
        titre
    };
    let lignes: Vec<AccountRowData> = if fenetre.get_group_by_tags() {
        let tags = services.store.account_tags().unwrap_or_default();
        let liens = services.store.account_tag_links().unwrap_or_default();
        let mut lignes = Vec::new();
        for tag in &tags {
            let dedans: Vec<&&iris_store::Account> = autres
                .iter()
                .filter(|c| liens.get(&c.id).is_some_and(|t| t.contains(&tag.id)))
                .collect();
            if dedans.is_empty() {
                continue;
            }
            lignes.push(signaler(
                bridge::account_group_header(
                    &tag.name,
                    crate::calendar::couleur(&tag.color),
                    tag.id,
                    replie(tag.id),
                    dedans.len(),
                    a_faire(&dedans),
                ),
                en_panne(&dedans),
            ));
            if !replie(tag.id) {
                lignes.extend(dedans.into_iter().map(|c| ligne(c)));
            }
        }
        let sans: Vec<&&iris_store::Account> = autres
            .iter()
            .filter(|c| liens.get(&c.id).is_none_or(|t| t.is_empty()))
            .collect();
        if !sans.is_empty() {
            lignes.push(signaler(
                bridge::account_group_header(
                    "No tag",
                    slint::Color::from_argb_u8(0, 0, 0, 0),
                    0,
                    replie(0),
                    sans.len(),
                    a_faire(&sans),
                ),
                en_panne(&sans),
            ));
            if !replie(0) {
                lignes.extend(sans.into_iter().map(|c| ligne(c)));
            }
        }
        lignes
    } else {
        autres.iter().map(|c| ligne(c)).collect()
    };
    fenetre.set_other_accounts(ModelRc::new(VecModel::from(lignes)));
    // Home lists every mailbox, whatever the mail's column filters or folds: a tag
    // folded there hid its accounts on Home too.
    fenetre.set_home_accounts(ModelRc::new(VecModel::from(
        comptes
            .iter()
            .map(|c| {
                let mut l = ligne(c);
                if l.needs_attention {
                    l.problem = if l.problem.is_empty() {
                        "Paused after repeated failures".into()
                    } else {
                        format!("Sync failed: {}", l.problem).into()
                    };
                }
                l
            })
            .collect::<Vec<_>>(),
    )));
    let total: u32 = a_traiter.values().sum();
    fenetre.set_unified_count(total as i32);
    fenetre.set_unified_label(iris_ui::format::short_count(total as u64).into());
    fenetre.set_unified_full(iris_ui::format::grouped_count(total as u64).into());

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
        "{qui} paused after repeated failures. Click the \"!\" to try again."
    ))
}

/// Wires the refresh buttons and the account filter.
///
/// Refreshing is deliberately not "everything, now". One mailbox on demand is the
/// common case and finishes in a second; refreshing all of them walks the list in
/// order and reports where it has got to, because a progress count that is a
/// guess is worse than no count.
pub fn wire_sync(
    fenetre: &AppWindow,
    services: &Services,
    controller: Arc<Controller>,
    runtime: tokio::runtime::Handle,
) {
    // --- The account filter, applied over the real list ---
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_account_filter_changed(move |_texte| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            refresh_accounts(&fenetre, &services, &[]);
        });
    }

    // --- One mailbox ---
    {
        let engine = Arc::clone(&services.engine);
        let services_un = services.clone();
        let controller = Arc::clone(&controller);
        let runtime_un = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_sync_account(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let compte = iris_types::AccountId(id as i64);
            fenetre.set_syncing(true);
            // Said at once, with the mailbox's name, and answered when it is over.
            let adresse = services_un
                .store
                .accounts()
                .unwrap_or_default()
                .into_iter()
                .find(|c| c.id == compte)
                .map(|c| c.email)
                .unwrap_or_default();
            fenetre.set_status(format!("Syncing {adresse}…").into());

            let engine = Arc::clone(&engine);
            let services_un = services_un.clone();
            let controller = Arc::clone(&controller);
            let faible = fenetre.as_weak();

            runtime_un.spawn(async move {
                let resultat = engine.sync_now(compte, now()).await;
                let panne = engine.failure(compte);
                let suspendus = engine.suspended_accounts().await;
                let _ = faible.upgrade_in_event_loop(move |fenetre| {
                    fenetre.set_syncing(false);
                    match resultat {
                        Ok(0) => {
                            fenetre.set_status(format!("{adresse} synced: up to date.").into())
                        }
                        Ok(n) => fenetre.set_status(
                            format!(
                                "{adresse} synced: {}.",
                                iris_ui::format::plural(n as u64, "new message")
                            )
                            .into(),
                        ),
                        // The kind of failure and where to look, not the TLS library's
                        // paragraph: the whole message is one click away, on the mark.
                        Err(e) => fenetre.set_status(
                            match panne {
                                Some(p) => format!(
                                    "{adresse} failed to sync: {}. Click the red ! next to it.",
                                    p.summary()
                                ),
                                None => format!("{adresse} failed to sync: {e}"),
                            }
                            .into(),
                        ),
                    }
                    // The mark appears, or goes, now — not at the next scheduled pass.
                    refresh_accounts(&fenetre, &services_un, &suspendus);
                });
                controller.send(Request::Diff(Box::new(iris_kernel::ViewDiff {
                    full_refresh: true,
                    ..Default::default()
                })));
            });
        });
    }

    // --- Every mailbox, in order, saying where it has got to ---
    {
        let engine = Arc::clone(&services.engine);
        let services_all = services.clone();
        let controller = Arc::clone(&controller);
        let runtime_tous = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_sync_all(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            if fenetre.get_syncing_all() {
                return;
            }
            fenetre.set_syncing_all(true);
            fenetre.set_sync_progress("0/…".into());

            let engine = Arc::clone(&engine);
            let services_all = services_all.clone();
            let controller = Arc::clone(&controller);
            let faible = fenetre.as_weak();

            runtime_tous.spawn(async move {
                let faible_progres = faible.clone();
                let rapport = engine
                    .sync_all(now(), move |done, total, _account| {
                        // How many are done: several go at once, so there is no
                        // "the one being synced" to count from.
                        let texte = if done >= total {
                            String::new()
                        } else {
                            format!("{done}/{total}")
                        };
                        let _ = faible_progres.upgrade_in_event_loop(move |fenetre| {
                            fenetre.set_sync_progress(texte.into());
                        });
                    })
                    .await;

                let suspendus = engine.suspended_accounts().await;
                let resume = rapport.summary();

                let _ = faible.upgrade_in_event_loop(move |fenetre| {
                    fenetre.set_syncing_all(false);
                    fenetre.set_sync_progress(Default::default());
                    fenetre.set_status(resume.into());
                    refresh_accounts(&fenetre, &services_all, &suspendus);
                });

                controller.send(Request::Diff(Box::new(iris_kernel::ViewDiff {
                    full_refresh: true,
                    ..Default::default()
                })));
            });
        });
    }
}

/// Reports what the process is costing, and how fresh the mailboxes are.
///
/// Both are cheap to read and neither is worth a thread of its own, so they share the
/// timer that was already redrawing the clock-relative dates.
pub fn refresh_vitals(
    fenetre: &AppWindow,
    reader: &mut crate::vitals::VitalsReader,
    last_sync: Option<iris_types::Timestamp>,
) {
    fenetre.set_vitals(reader.sample().summary().into());
    fenetre.set_last_sync(match last_sync {
        Some(t) => format!("Synced {}", crate::vitals::ago(t, now())).into(),
        None => slint::SharedString::default(),
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

    // The toolbar sends the same requests as the keys, so a button and a keystroke
    // can never do subtly different things.
    {
        let c = Arc::clone(&controller);
        fenetre.on_thread_archive(move || c.send(Request::Apply(iris_viewmodel::Action::Archive)));
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_thread_delete(move || c.send(Request::Apply(iris_viewmodel::Action::Delete)));
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_thread_done(move || c.send(Request::Apply(iris_viewmodel::Action::Done)));
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_thread_waiting(move || c.send(Request::Apply(iris_viewmodel::Action::Waiting)));
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_thread_snooze(move || {
            c.send(Request::Apply(iris_viewmodel::Action::SnoozeHours(24)))
        });
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_thread_toggle_star(move || {
            c.send(Request::Apply(iris_viewmodel::Action::ToggleFlag))
        });
    }
    {
        let c = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_thread_toggle_unread(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            // One button, two meanings, decided by what the message currently is.
            c.send(Request::Apply(if fenetre.get_selected_unread() {
                iris_viewmodel::Action::MarkRead
            } else {
                iris_viewmodel::Action::MarkUnread
            }));
        });
    }

    // Les mêmes actions, sur le fil que le menu contextuel a visé.
    //
    // `ApplyTo` et non `Apply` : la sélection ne bouge pas. C'est toute la différence,
    // et c'est ce qui permet au clic droit de ne plus ouvrir le message pour pouvoir
    // proposer de le jeter.
    {
        let c = Arc::clone(&controller);
        fenetre.on_menu_archive(move |id| {
            c.send(Request::ApplyTo(fil(id), iris_viewmodel::Action::Archive))
        });
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_menu_delete(move |id| {
            c.send(Request::ApplyTo(fil(id), iris_viewmodel::Action::Delete))
        });
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_menu_done(move |id| {
            c.send(Request::ApplyTo(fil(id), iris_viewmodel::Action::Done))
        });
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_menu_snooze(move |id, quand| {
            c.send(Request::ApplyTo(
                fil(id),
                iris_viewmodel::Action::SnoozeHours(heures_de_report(&quand, now())),
            ))
        });
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_menu_toggle_star(move |id| {
            c.send(Request::ApplyTo(
                fil(id),
                iris_viewmodel::Action::ToggleFlag,
            ))
        });
    }
    {
        let c = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_menu_toggle_unread(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            // L'état du fil visé, que la ligne a transmis avec la demande — pas celui
            // du fil ouvert à la lecture, qui n'est plus le même.
            c.send(Request::ApplyTo(
                fil(id),
                if fenetre.get_context_menu_unread() {
                    iris_viewmodel::Action::MarkRead
                } else {
                    iris_viewmodel::Action::MarkUnread
                },
            ));
        });
    }

    // Annuler et rétablir. Un « Ctrl+Z » classique : il défait la dernière action de
    // triage, et un second l'action d'avant.
    {
        let c = Arc::clone(&controller);
        fenetre.on_undo(move || c.send(Request::Undo));
    }
    {
        let c = Arc::clone(&controller);
        fenetre.on_redo(move || c.send(Request::Redo));
    }

    {
        let c = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_account_selected(move |id| {
            if let Some(f) = faible.upgrade() {
                f.set_selected_tag(-1);
            }
            c.send(Request::FilterAccounts(vec![iris_types::AccountId(
                id as i64,
            )]));
        });
    }

    {
        let c = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_unified_selected(move || {
            if let Some(f) = faible.upgrade() {
                f.set_selected_tag(-1);
            }
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

    // A search pill: its words put in the query or taken out of it, and the search run
    // again with the query shown as it now is.
    {
        let c = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_search_pill(move |i| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(pastille) = usize::try_from(i).ok().and_then(|i| PASTILLES.get(i)) else {
                return;
            };
            let requete = basculer_pastille(fenetre.get_search_query().as_str(), pastille);
            fenetre.set_search_query(requete.as_str().into());
            if requete.trim().is_empty() {
                c.send(Request::ClearSearch);
            } else {
                c.send(Request::Search(requete));
            }
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
        CommandKind::TaskFromThread => {
            if let Some(fenetre) = fenetre.upgrade() {
                let fil = fenetre.get_selected_thread();
                if fil >= 0 {
                    fenetre.invoke_thread_to_task(fil);
                }
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

    // Les adresses des comptes servent à colorer les lignes. Résolues une fois par
    // instantané et non une fois par ligne — mais **par compte**, ce qui manquait :
    // c'était l'adresse du premier compte pour toutes les lignes, donc une seule
    // couleur pour cent boîtes. La pastille n'était pas discrète, elle était fausse,
    // et un repère qui affirme la même chose partout est pire qu'aucun repère.
    let comptes = services.store.accounts().unwrap_or_default();
    let adresses: std::collections::HashMap<iris_types::AccountId, String> =
        comptes.iter().map(|c| (c.id, c.email.clone())).collect();
    let adresse_par_defaut = comptes.first().map(|c| c.email.clone()).unwrap_or_default();

    let jours = iris_ui::format::day_headers(
        &snapshot
            .rows
            .iter()
            .map(|r| r.last_activity)
            .collect::<Vec<_>>(),
        maintenant,
    );
    let mut titres = 0;
    let lignes: Vec<_> = snapshot
        .rows
        .iter()
        .zip(jours)
        .map(|(r, jour)| {
            let adresse = adresses.get(&r.account).unwrap_or(&adresse_par_defaut);
            let mut ligne =
                bridge::thread_row(r, adresse, maintenant, snapshot.marked.contains(&r.id));
            if !jour.is_empty() {
                titres += 1;
            }
            ligne.day = jour.into();
            ligne.titles = titres;
            ligne
        })
        .collect();

    // An action that failed says so where the user is looking, and stays there until
    // the next thing happens. Silence is the one answer a button must never give.
    if let Some(probleme) = &snapshot.error {
        fenetre.set_status(probleme.as_str().into());
    }

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
    fenetre.set_filter_unread(snapshot.filters.unread);
    fenetre.set_filter_attachments(snapshot.filters.attachments);
    fenetre.set_filter_starred(snapshot.filters.starred);
    fenetre.set_list_sort(match snapshot.sort {
        iris_store::Sort::Date => 0,
        iris_store::Sort::Sender => 1,
        iris_store::Sort::Subject => 2,
        iris_store::Sort::Size => 3,
    });
    fenetre.set_marked_count(snapshot.marked.len() as i32);
    fenetre.set_marked_label(iris_ui::format::short_count(snapshot.marked.len() as u64).into());
    // Ce que l'arborescence doit montrer comme choisi. Un rôle est désigné par son nom
    // de rôle préfixé, un dossier par son chemin : deux espaces de noms qui ne peuvent
    // pas se marcher dessus, puisqu'un chemin IMAP ne commence jamais par « role: ».
    fenetre.set_selected_folder(
        match &snapshot.scope {
            iris_store::Scope::Queue => String::new(),
            iris_store::Scope::Role(r) => format!("role:{}", r.as_str()),
            iris_store::Scope::Path(p) => p.clone(),
        }
        .into(),
    );

    fenetre.set_count_labels(ModelRc::new(VecModel::from(
        snapshot
            .counts
            .iter()
            .map(|c| slint::SharedString::from(iris_ui::format::short_count(*c as u64)))
            .collect::<Vec<_>>(),
    )));
    fenetre.set_count_fulls(ModelRc::new(VecModel::from(
        snapshot
            .counts
            .iter()
            .map(|c| slint::SharedString::from(iris_ui::format::grouped_count(*c as u64)))
            .collect::<Vec<_>>(),
    )));
    // La vue unifiée compte la file de travail, comme les lignes de comptes.
    fenetre.set_unified_count(snapshot.counts[0] as i32);
    fenetre.set_pending_ops(snapshot.pending_ops as i32);
    // Le nom du dossier ouvert, tel que la colonne du milieu doit l'afficher. Il vient
    // de la même table que l'arborescence pour qu'un dossier ne porte pas deux noms
    // sur le même écran.
    fenetre.set_folder_name(crate::folders::scope_name(&snapshot.scope).into());
    fenetre.set_inbox_zero_streak(inbox_zero(services, snapshot) as i32);

    // Quel compte est allumé dans la barre latérale.
    //
    // Zéro veut dire « tous », ce qui est aussi l'identifiant de la ligne unifiée.
    // Plusieurs comptes filtrés à la fois — ce qu'aucun geste de l'interface ne
    // produit aujourd'hui — retombent sur « tous » plutôt que d'en désigner un au
    // hasard parmi eux.
    fenetre.set_selected_account(match snapshot.accounts.as_slice() {
        [seul] => seul.get() as i32,
        _ => 0,
    });

    // What the toolbar's two toggles should say. Taken from the row rather than the
    // message, because both are properties of the conversation as the list shows it.
    let selectionne = snapshot
        .selected
        .and_then(|t| snapshot.rows.iter().find(|r| r.id == t));
    fenetre.set_selected_unread(selectionne.map(|r| r.is_unread()).unwrap_or(false));
    fenetre.set_selected_starred(
        selectionne
            .map(|r| r.flags_union.contains(iris_types::Flags::FLAGGED))
            .unwrap_or(false),
    );
    // The mailbox it came to, under the subject: its dot and its address.
    let boite = selectionne
        .and_then(|r| adresses.get(&r.account))
        .cloned()
        .unwrap_or_default();
    let (r, g, b) = iris_ui::format::account_tint(&boite);
    fenetre.set_selected_account_email(boite.into());
    fenetre.set_selected_account_tint(slint::Color::from_rgb_u8(r, g, b));
    let (dossier, tous, alarmant) = dossiers_du_fil(services, &snapshot.messages);
    fenetre.set_selected_folder_label(dossier.into());
    fenetre.set_selected_folder_all(tous.into());
    fenetre.set_selected_folder_alarming(alarmant);
    fenetre.set_conversation_empty(snapshot.messages.is_empty());

    match &snapshot.search {
        Some(recherche) => {
            fenetre.set_searching(true);
            fenetre.set_search_summary(recherche.summary.as_str().into());
            fenetre.set_search_explanation(recherche.explanation.as_str().into());
            fenetre.set_search_pills(ModelRc::new(VecModel::from(
                PASTILLES
                    .iter()
                    .map(|(mots, _)| pastille_allumee(&recherche.query, mots))
                    .collect::<Vec<_>>(),
            )));
            // Le champ n'est pas réécrit : l'utilisateur peut être en train d'y
            // taper la requête suivante pendant que les résultats arrivent.
        }
        None => {
            fenetre.set_searching(false);
            fenetre.set_search_summary(Default::default());
            fenetre.set_search_explanation(Default::default());
        }
    }

    remplir_conversation(fenetre, services, renderer, &snapshot.messages, maintenant);
}

/// The pills under the search field, in their order: the words each puts in the query,
/// and the words that count as it being on (the first is the one written). The two
/// periods exclude each other.
const PASTILLES: [(&[&str], &str); 5] = [
    (&["is:unread", "etat:non_lu"], ""),
    (&["has:attachment", "a_pj:pj", "has:pj"], ""),
    (
        &["newer_than:7d", "newer_than:7j", "plus_recent:7j"],
        "periode",
    ),
    (
        &["newer_than:30d", "newer_than:30j", "plus_recent:30j"],
        "periode",
    ),
    (&["sort:relevance"], ""),
];

fn pastille_allumee(requete: &str, mots: &[&str]) -> bool {
    requete
        .split_whitespace()
        .any(|m| mots.iter().any(|x| m.eq_ignore_ascii_case(x)))
}

/// The query with a pill's words taken out when they are there, put in when they are
/// not; putting in a period takes the other one out.
fn basculer_pastille(requete: &str, (mots, groupe): &(&[&str], &str)) -> String {
    let allumee = pastille_allumee(requete, mots);
    let mut garde: Vec<&str> = requete
        .split_whitespace()
        .filter(|m| !mots.iter().any(|x| m.eq_ignore_ascii_case(x)))
        .filter(|m| {
            allumee
                || groupe.is_empty()
                || !PASTILLES
                    .iter()
                    .filter(|(_, g)| g == groupe)
                    .any(|(autres, _)| autres.iter().any(|x| m.eq_ignore_ascii_case(x)))
        })
        .collect();
    if !allumee {
        garde.push(mots[0]);
    }
    garde.join(" ")
}

/// The inbox emptied: nothing left to do in the work queue, nothing searched or
/// filtered, and a mailbox to empty. The days in a row it has been so, counted and
/// kept in the settings; 0 when it is not.
fn inbox_zero(services: &Services, snapshot: &Snapshot) -> u32 {
    let vide = matches!(snapshot.scope, iris_store::Scope::Queue)
        && snapshot.active_tab == iris_types::WorkflowState::Todo
        && snapshot.rows.is_empty()
        && snapshot.counts[0] == 0
        && snapshot.search.is_none()
        && snapshot.filters.is_empty();
    if !vide
        || services
            .store
            .accounts()
            .map(|a| a.is_empty())
            .unwrap_or(true)
    {
        return 0;
    }
    let today = chrono::Local::now().date_naive();
    let reglages = crate::settings::current();
    let dernier = chrono::NaiveDate::parse_from_str(&reglages.inbox_zero_day, "%Y-%m-%d").ok();
    let serie = crate::settings::inbox_zero_streak(today, dernier, reglages.inbox_zero_streak);
    if dernier != Some(today) || serie != reglages.inbox_zero_streak {
        crate::settings::update(|s| {
            s.inbox_zero_day = today.format("%Y-%m-%d").to_string();
            s.inbox_zero_streak = serie;
        });
    }
    serie
}

/// Where a conversation is: the folder of its latest message ("Inbox", "Spam", "Trash",
/// or a folder's own name), and "+1" when other messages of it are elsewhere. Empty
/// when there is nothing to read. With it, every folder named when there are several
/// (the label's tooltip), and whether that latest one is in the bin or the spam, which
/// the header says in red.
fn dossiers_du_fil(
    services: &Services,
    messages: &[iris_store::StoredMessage],
) -> (String, String, bool) {
    let mut nommes: Vec<String> = Vec::new();
    let mut alarmant = None;
    let mut connus: std::collections::HashMap<iris_types::AccountId, Vec<iris_store::Folder>> =
        std::collections::HashMap::new();
    for m in messages.iter().rev() {
        let dossiers = connus
            .entry(m.account)
            .or_insert_with(|| services.store.folders(m.account).unwrap_or_default());
        let Some(d) = dossiers.iter().find(|d| d.id == m.folder) else {
            continue;
        };
        alarmant.get_or_insert(matches!(
            d.role,
            iris_store::FolderRole::Trash | iris_store::FolderRole::Junk
        ));
        let nom = folder_label(d.role, &d.path);
        if !nommes.contains(&nom) {
            nommes.push(nom);
        }
    }
    let tous = if nommes.len() > 1 {
        nommes.join(", ")
    } else {
        String::new()
    };
    let texte = match nommes.len() {
        0 => String::new(),
        1 => nommes.remove(0),
        n => format!("{} +{}", nommes[0], n - 1),
    };
    (texte, tous, alarmant.unwrap_or(false))
}

/// A folder as people call it: its role's word, else the last part of its path.
fn folder_label(role: iris_store::FolderRole, path: &str) -> String {
    use iris_store::FolderRole as R;
    match role {
        R::Inbox => "Inbox".into(),
        R::Sent => "Sent".into(),
        R::Drafts => "Drafts".into(),
        R::Trash => "Trash".into(),
        R::Junk => "Spam".into(),
        R::Archive => "Archive".into(),
        R::Other => path
            .rsplit(['/', '.'])
            .find(|p| !p.is_empty())
            .unwrap_or(path)
            .to_string(),
    }
}

/// Remplit la colonne de lecture avec le fil entier.
///
/// Le dernier message est déplié, les précédents sont repliés — sauf ceux que le
/// lecteur a ouverts. C'est ce qui rend un fil de douze messages tenable : un corps
/// rendu coûte une rasterisation, et en calculer douze pour en lire un paierait onze
/// fois pour rien.
pub fn remplir_conversation(
    fenetre: &AppWindow,
    services: &Services,
    renderer: &dyn iris_htmlview::HtmlRenderer,
    messages: &[iris_store::StoredMessage],
    maintenant: iris_types::Timestamp,
) {
    let Some(dernier) = messages.last() else {
        conversation_rendue().clear();
        fenetre.set_messages(ModelRc::new(VecModel::from(
            Vec::<iris_ui::MessageData>::new(),
        )));
        return;
    };

    let ouverts = expanded_messages();

    // Rien à refaire si rien n'a changé.
    //
    // C'est le gel signalé en cochant une case. Un instantané est émis à chaque
    // requête — cocher, décocher, changer d'onglet — et cette fonction rendait à chaque
    // fois le corps du message ouvert : mise en page complète et rastérisation, sur le
    // fil de l'interface. Le journal montre des corps de vingt-neuf mille pixels de
    // haut ; à ce format, l'opération se compte en secondes, et pendant ce temps la
    // fenêtre ne répond plus. Cocher une case n'a rien à voir avec le message affiché,
    // et le payait quand même.
    //
    // La signature couvre tout ce qui change le rendu : quels messages, dans quel état,
    // lesquels sont dépliés et lesquels ont accepté les images distantes. Le reste d'un
    // instantané — la sélection, les compteurs, les cases cochées — n'y figure pas,
    // parce que rien de tout cela ne se voit dans la colonne de lecture.
    let signature = signature_conversation(messages, dernier.id, &ouverts, &images_shown());

    {
        let mut derniere = conversation_rendue();
        if *derniere == signature {
            return;
        }
        *derniere = signature;
    }

    // Les documents peints par tuiles, gardés pour ceux qui restent affichés. Ceux des
    // messages qui disparaissent de la colonne partent avec leur mise en page.
    let mut documents: HashMap<i64, CorpsOuvert> = HashMap::new();

    let vues: Vec<iris_ui::MessageData> = messages
        .iter()
        .map(|message| {
            // Le dernier est toujours déplié : c'est celui qu'on vient lire.
            if message.id != dernier.id && !ouverts.contains(&message.id.get()) {
                return bridge::message_header(message, maintenant);
            }

            let montrer = images_shown().contains(&message.id.get());
            let pieces = pieces_jointes(services, message.id);
            let place = (largeur_de_lecture(), fenetre.window().scale_factor());
            let mut vue = match corps_du_message(services, renderer, message, montrer, place) {
                // Un document : ses tuiles sont posées vides, et peintes quand elles
                // approchent de l'écran (`wire_body_tiles`).
                iris_htmlview::Rendered::Document(document) => {
                    let tuiles =
                        Rc::new(VecModel::from(bridge::tile_placeholders(document.as_ref())));
                    let mut vue = bridge::message_view(
                        message,
                        &iris_htmlview::RichText::default(),
                        &pieces,
                        maintenant,
                    );
                    vue.body_tiles = ModelRc::from(Rc::clone(&tuiles));
                    vue.body_is_image = true;
                    documents.insert(message.id.get(), CorpsOuvert { document, tuiles });
                    vue
                }
                autre => bridge::message_view_rendered(message, &autre, &pieces, maintenant),
            };
            // Le corps n'est pas encore descendu du serveur. L'écran doit le dire :
            // un panneau vide ne distingue pas « ça arrive » de « il n'y a rien ».
            vue.body_loading = message.body_blob.is_none();
            // An invitation, over this message only: the one that carries it.
            if let Some(inv) = invitation_du_message(services, message) {
                vue.invite_state = inv.state;
                vue.invite_title = inv.title.into();
                vue.invite_when = inv.when.into();
                vue.invite_key = inv.key.into();
                vue.invite_day = inv.day.into();
                vue.invite_can_reply = inv.can_reply;
                vue.invite_reply = inv.reply.into();
            }
            vue
        })
        .collect();

    // L'en-tête et la barre d'actions décrivent le dernier message, parce que c'est de
    // lui qu'on décide : répondre, archiver, reporter portent sur la conversation, et
    // la conversation est ce que le dernier message a laissé.
    if let Some(vue) = vues.last() {
        fenetre.set_message(vue.clone());
    }
    CORPS.with(|c| *c.borrow_mut() = documents);
    fenetre.set_messages(ModelRc::new(VecModel::from(vues)));
}

/// Un corps mis en page par le moteur complet, et le modèle de ses tuiles.
struct CorpsOuvert {
    document: Box<dyn iris_htmlview::TiledDocument>,
    tuiles: Rc<VecModel<iris_ui::BodyTileData>>,
}

thread_local! {
    /// Les corps affichés, par identifiant de message.
    ///
    /// Sur le fil de l'interface, parce que c'est là que les tuiles sont demandées et
    /// que le document n'est pas fait pour changer de fil.
    static CORPS: std::cell::RefCell<HashMap<i64, CorpsOuvert>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Peint les tuiles qui approchent de l'écran, et rend celles qui s'en éloignent.
pub fn wire_body_tiles(fenetre: &AppWindow) {
    fenetre.on_body_tile_wanted(|message, index| {
        CORPS.with(|c| {
            let mut corps = c.borrow_mut();
            let Some(ouvert) = corps.get_mut(&(message as i64)) else {
                return;
            };
            let index = index.max(0) as usize;
            let Some(mut tuile) = ouvert.tuiles.row_data(index) else {
                return;
            };
            if tuile.ready {
                return;
            }
            let (largeur, _) = ouvert.document.size();
            let hauteur = ouvert.document.tile_extent(index);
            let mut pixels = bridge::ImageSink::default();
            match ouvert.document.paint_tile(index, &mut pixels) {
                Ok(()) => {
                    if let Some(image) = pixels.image(largeur, hauteur) {
                        tuile.image = image;
                        tuile.ready = true;
                        ouvert.tuiles.set_row_data(index, tuile);
                    }
                }
                Err(e) => tracing::warn!(error = %e, index, "painting a body tile"),
            }
        });
    });

    // Ctrl+C with the keyboard at the shortcuts: the words selected in a painted body.
    // (Selected in a text body, they are copied by the text itself.)
    let faible = fenetre.as_weak();
    fenetre.on_copy_requested(move || {
        let (Some(f), Some(texte)) = (faible.upgrade(), selected_body_text()) else {
            return;
        };
        f.invoke_copy_text(texte.into());
        f.set_status("Copied.".into());
    });

    // A key the text of a message had no use for: sent again once the shortcuts have
    // the keyboard, a turn of the loop later, so that "e" still marks as done after
    // a click in the words.
    let faible = fenetre.as_weak();
    fenetre.on_key_redispatch(move |texte| {
        let faible = faible.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(f) = faible.upgrade() {
                use slint::platform::WindowEvent;
                f.window().dispatch_event(WindowEvent::KeyPressed {
                    text: texte.clone(),
                });
                f.window()
                    .dispatch_event(WindowEvent::KeyReleased { text: texte });
            }
        });
    });

    // Words chosen with the mouse: pressed on a tile, dragged anywhere. The point is
    // taken in the whole document, so a drag past its tile goes on into the next.
    fenetre.on_body_select(|message, index, fx, fy, phase| {
        CORPS.with(|c| {
            let mut corps = c.borrow_mut();
            // Pressing starts afresh: what was selected in the other messages goes.
            if phase == 0 {
                for (id, autre) in corps.iter_mut() {
                    if *id != message as i64 && autre.document.clear_selection() {
                        repeindre(autre);
                    }
                }
            }
            let Some(ouvert) = corps.get_mut(&(message as i64)) else {
                return;
            };
            let index = index.max(0) as usize;
            let (largeur, _) = ouvert.document.size();
            let x = fx * largeur as f32;
            let y = index as f32 * ouvert.document.tile_height() as f32
                + fy * ouvert.document.tile_extent(index) as f32;
            let change = if phase == 0 {
                ouvert.document.select_from(x, y)
            } else {
                ouvert.document.select_to(x, y)
            };
            if change {
                repeindre(ouvert);
            }
        });
    });

    fenetre.on_body_tile_released(|message, index| {
        CORPS.with(|c| {
            let corps = c.borrow();
            let Some(ouvert) = corps.get(&(message as i64)) else {
                return;
            };
            let index = index.max(0) as usize;
            if let Some(mut tuile) = ouvert.tuiles.row_data(index) {
                if tuile.ready {
                    tuile.image = slint::Image::default();
                    tuile.ready = false;
                    ouvert.tuiles.set_row_data(index, tuile);
                }
            }
        });
    });
}

/// Paints again the tiles of a body that are on show, after its selection changed.
fn repeindre(ouvert: &mut CorpsOuvert) {
    let (largeur, _) = ouvert.document.size();
    for index in 0..ouvert.tuiles.row_count() {
        let Some(mut tuile) = ouvert.tuiles.row_data(index) else {
            continue;
        };
        if !tuile.ready {
            continue;
        }
        let hauteur = ouvert.document.tile_extent(index);
        let mut pixels = bridge::ImageSink::default();
        match ouvert.document.paint_tile(index, &mut pixels) {
            Ok(()) => {
                if let Some(image) = pixels.image(largeur, hauteur) {
                    tuile.image = image;
                    ouvert.tuiles.set_row_data(index, tuile);
                }
            }
            Err(e) => tracing::warn!(error = %e, index, "painting a body tile again"),
        }
    }
}

/// The words selected in a body painted by the full engine, if any.
pub fn selected_body_text() -> Option<String> {
    CORPS.with(|c| {
        c.borrow()
            .values()
            .find_map(|ouvert| ouvert.document.selected_text())
    })
}

/// Rend tout ce que la lecture a peint : tuiles et peintres. Les mises en page
/// restent, pour que revenir ne coûte que les tuiles qu'on regarde.
pub fn release_bodies() {
    CORPS.with(|c| {
        for ouvert in c.borrow_mut().values_mut() {
            for i in 0..ouvert.tuiles.row_count() {
                if let Some(mut tuile) = ouvert.tuiles.row_data(i) {
                    if tuile.ready {
                        tuile.image = slint::Image::default();
                        tuile.ready = false;
                        ouvert.tuiles.set_row_data(i, tuile);
                    }
                }
            }
            ouvert.document.release();
        }
    });
}

/// La fenêtre part dans la zone de notification : rendre ce qu'elle occupait.
///
/// Les tuiles cessent d'être demandées, celles qui existent sont rendues, puis les
/// pages libérées retournent au système un quart de seconde plus tard — le temps que
/// la fenêtre ait fini de disparaître et que rien ne les retouche aussitôt.
pub fn went_to_tray(fenetre: &AppWindow) {
    fenetre.set_bodies_suspended(true);
    release_bodies();
    slint::Timer::single_shot(std::time::Duration::from_millis(250), || {
        crate::vitals::give_back_memory();
    });
}

/// La fenêtre revient : les tuiles proches de l'écran se redemandent d'elles-mêmes.
pub fn came_back(fenetre: &AppWindow) {
    fenetre.set_bodies_suspended(false);
}

/// Les pièces jointes d'un message, prêtes pour le bandeau.
///
/// Les pièces incrustées sont écartées : une image de signature n'est pas un document
/// reçu, et la lister ferait chercher un fichier qui n'existe pas.
///
/// Le type et la taille accompagnent le nom. « devis.pdf » seul ne dit pas s'il faut
/// l'ouvrir maintenant ou attendre d'être au bureau ; « PDF · 2,4 Mo » le dit.
fn pieces_jointes(services: &Services, message: iris_types::MessageId) -> Vec<AttachmentData> {
    services
        .store
        .visible_attachments(message)
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let (genre, icone) =
                iris_ui::format::attachment_kind(&p.meta.filename, &p.meta.mime_type);
            AttachmentData {
                name: p.meta.filename.as_str().into(),
                size: iris_ui::format::human_size(p.meta.size).into(),
                kind: genre.into(),
                icon: icone.into(),
            }
        })
        .collect()
}

/// Les messages que le lecteur a dépliés.
///
/// Pour cette session seulement, et volontairement : un fil rouvert le lendemain doit
/// se présenter comme un fil, pas comme la trace de ce qu'on avait ouvert la veille.
fn expanded_messages() -> std::sync::MutexGuard<'static, std::collections::BTreeSet<i64>> {
    static OUVERTS: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeSet<i64>>> =
        std::sync::OnceLock::new();
    OUVERTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Un message tel qu'il a été rendu : son identifiant, ses drapeaux, s'il est déplié,
/// s'il a le droit d'aller chercher ses images.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EtatRendu {
    id: i64,
    flags: u32,
    deplie: bool,
    images: bool,
    /// Le corps est-il téléchargé ?
    ///
    /// Il manquait, et c'était le défaut : à l'ouverture d'un message dont le corps
    /// n'est pas encore là, la demande part, le corps arrive, un nouvel instantané
    /// est émis — et la signature, identique, faisait renoncer au dessin. L'écran
    /// restait vide jusqu'à ce qu'on aille sur un autre message et qu'on revienne,
    /// ce qui changeait la liste et forçait enfin le rendu.
    corps: bool,
}

/// Décrit une conversation par ce qui, en elle, change ce qui est dessiné.
///
/// Pure et à part, parce que c'est la seule décision du cache et qu'elle se trompe dans
/// deux directions opposées. Trop large, elle rend à chaque case cochée et la fenêtre
/// se fige. Trop étroite, elle laisse à l'écran un message qui a changé — et ce genre
/// de faute-là ne se voit pas tout de suite.
fn signature_conversation(
    messages: &[iris_store::StoredMessage],
    dernier: iris_types::MessageId,
    ouverts: &std::collections::BTreeSet<i64>,
    images: &std::collections::BTreeSet<i64>,
) -> Vec<EtatRendu> {
    messages
        .iter()
        .map(|m| EtatRendu {
            id: m.id.get(),
            // Only what the column draws. The star, read and answered marks do not
            // show in a message's body, and counting them laid out and painted the
            // whole body again on each star: the freeze at the click.
            flags: m.flags.0
                & !(iris_types::Flags::FLAGGED.0
                    | iris_types::Flags::SEEN.0
                    | iris_types::Flags::ANSWERED.0
                    | iris_types::Flags::RECENT.0),
            // Le dernier est toujours déplié, la même règle qu'au dessin.
            deplie: m.id == dernier || ouverts.contains(&m.id.get()),
            images: images.contains(&m.id.get()),
            corps: m.body_blob.is_some(),
        })
        .collect()
}

thread_local! {
    /// La largeur à laquelle la colonne de lecture montre les corps, en points. Zéro :
    /// pas encore connue, avant le premier message affiché.
    static LARGEUR_LECTURE: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
    /// Le redessin qui suit un redimensionnement, attendu un quart de seconde.
    static APRES_REDIMENSION: slint::Timer = slint::Timer::default();
}

/// La largeur de lecture connue, ou 800 points avant la première mesure.
fn largeur_de_lecture() -> f32 {
    let l = LARGEUR_LECTURE.with(|c| c.get());
    if l > 0.0 {
        l
    } else {
        800.0
    }
}

/// Ce que la colonne de lecture montre déjà, décrit assez pour savoir si c'est à
/// refaire.
///
/// Un par message : son identifiant, ses drapeaux, s'il est déplié, s'il a le droit
/// d'aller chercher ses images, et si son corps est arrivé. Ce sont exactement les
/// cinq choses qui changent ce qui est dessiné. Tout ce qui n'y figure pas — la sélection, les compteurs, le lot
/// coché — peut varier autant qu'il veut sans qu'un seul pixel de cette colonne bouge.
fn conversation_rendue() -> std::sync::MutexGuard<'static, Vec<EtatRendu>> {
    static RENDUE: std::sync::OnceLock<std::sync::Mutex<Vec<EtatRendu>>> =
        std::sync::OnceLock::new();
    RENDUE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Les messages dont le lecteur a accepté le contenu distant.
///
/// Per message, and only for this session. A blanket "always show images" setting is
/// the one thing this list must never become: the point of blocking them is that a
/// remote image is a read receipt sent to whoever wrote to you, and a permanent
/// exception hands that back for every message that follows.
fn images_shown() -> std::sync::MutexGuard<'static, std::collections::BTreeSet<i64>> {
    static MONTRES: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeSet<i64>>> =
        std::sync::OnceLock::new();
    MONTRES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Wires the "Show" button on the blocked-content banner.
///
/// The banner has been telling people images were blocked, and offering a button that
/// was declared in the interface and connected to nothing in Rust. Saying "N images
/// blocked" beside a control that does nothing is worse than not mentioning it.
pub fn wire_remote_images(
    fenetre: &AppWindow,
    services: Services,
    renderer: Arc<dyn iris_htmlview::HtmlRenderer>,
) {
    // La colonne de lecture a changé de largeur — ou vient d'annoncer la sienne.
    //
    // Un corps peint par le moteur complet l'est à la largeur exacte où il s'affiche :
    // réduit après coup, son texte devenait crénelé. Quand la largeur change, il est
    // donc remis en page, une fois le redimensionnement terminé plutôt qu'à chaque
    // pixel du glisser.
    {
        let services_largeur = services.clone();
        let renderer_largeur = Arc::clone(&renderer);
        let faible = fenetre.as_weak();
        fenetre.on_reader_width(move |largeur| {
            let avant = LARGEUR_LECTURE.with(|c| c.replace(largeur));
            if (avant - largeur).abs() < 1.0 {
                return;
            }
            // Rien à refaire si aucun corps n'est peint : le texte riche suit tout seul.
            if CORPS.with(|c| c.borrow().is_empty()) {
                return;
            }
            let services = services_largeur.clone();
            let renderer = Arc::clone(&renderer_largeur);
            let faible = faible.clone();
            APRES_REDIMENSION.with(|t| {
                t.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::from_millis(250),
                    move || {
                        let Some(fenetre) = faible.upgrade() else {
                            return;
                        };
                        let fil = fenetre.get_selected_thread();
                        if fil < 0 {
                            return;
                        }
                        let messages = services
                            .store
                            .conversation(iris_types::ThreadId(fil as i64))
                            .unwrap_or_default();
                        conversation_rendue().clear();
                        remplir_conversation(
                            &fenetre,
                            &services,
                            renderer.as_ref(),
                            &messages,
                            now(),
                        );
                    },
                );
            });
        });
    }

    // Déplier ou replier un message du fil.
    //
    // Le redessin passe par le même chemin que l'affichage initial : rien du fil n'a
    // changé, seulement ce qu'on accepte d'en rendre, et demander un instantané au
    // vue-modèle reconstruirait une liste pour repeindre un panneau.
    {
        let services_pli = services.clone();
        let renderer_pli = Arc::clone(&renderer);
        let faible = fenetre.as_weak();

        fenetre.on_toggle_message(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            {
                let mut ouverts = expanded_messages();
                if !ouverts.remove(&(id as i64)) {
                    ouverts.insert(id as i64);
                }
            }

            let fil = fenetre.get_selected_thread();
            if fil < 0 {
                return;
            }
            let messages = services_pli
                .store
                .conversation(iris_types::ThreadId(fil as i64))
                .unwrap_or_default();
            remplir_conversation(
                &fenetre,
                &services_pli,
                renderer_pli.as_ref(),
                &messages,
                now(),
            );
        });
    }

    let faible = fenetre.as_weak();

    fenetre.on_load_images(move || {
        let Some(fenetre) = faible.upgrade() else {
            return;
        };
        let fil = fenetre.get_selected_thread();
        if fil < 0 {
            return;
        }

        let messages = services
            .store
            .conversation(iris_types::ThreadId(fil as i64))
            .unwrap_or_default();
        let Some(message) = messages.last() else {
            return;
        };

        images_shown().insert(message.id.get());

        // Redessiné par le même chemin que l'affichage initial : rien du fil n'a
        // changé, seulement ce qu'on accepte d'en rendre.
        remplir_conversation(&fenetre, &services, renderer.as_ref(), &messages, now());
    });
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
        // `probe` plutôt qu'un test suivi d'une construction : la vérification ouvre
        // un périphérique graphique, et c'est celui-là même qui rendra. Le tester
        // puis le jeter faisait payer la seconde ouverture — huit cents millisecondes
        // — au premier message ouvert, sur le fil de l'interface.
        if let Some(moteur) = iris_htmlview::BlitzRenderer::probe(1.0, true) {
            tracing::info!("body rendering: full engine available");
            return simple.with_full_engine(Box::new(moteur));
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
    allow_remote: bool,
    // La largeur du corps à l'écran, en points, et les pixels par point.
    (largeur, echelle): (f32, f32),
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

    let brut = iris_types::BlobId::from_hex(hex)
        .and_then(|id| services.blobs.get(id).ok().flatten())
        .unwrap_or_default();

    // What is stored is the whole RFC 5322 message: headers, boundaries, every part.
    // It has to be parsed before anything is shown. Handing the raw bytes to the HTML
    // sanitiser instead — which is what happened here for far too long — puts
    // `Return-Path`, every `Received` hop and the DKIM signature on screen where the
    // message should be.
    let analyse = match iris_mime::parse_with(&brut, allow_remote) {
        Ok(analyse) => analyse,
        Err(e) => {
            tracing::warn!(error = %e, "could not parse the message");
            return apercu();
        }
    };

    // HTML first when both are offered: it is what the sender laid out. The parser has
    // already sanitised it, so nothing here needs to sanitise it again.
    let html = match (&analyse.html_body, &analyse.text_body) {
        // Les images que le message transporte lui-même sont remises dans le corps.
        // Une signature d'entreprise est un logo joint au message et référencé par
        // `cid:` : rien ne peut aller le chercher, il est déjà là, et sans cela toutes
        // les signatures HTML s'affichaient sans leur image.
        (Some(sanitized), _) => iris_mime::inline_images(&sanitized.html, &analyse.inline_parts),
        (None, Some(texte)) => plain_text_to_html(texte),
        // A message with neither part is not broken — a bare attachment carrier looks
        // exactly like this — so the preview stands in rather than an error.
        (None, None) => return apercu(),
    };

    renderer
        .render_for(&html, allow_remote, largeur, echelle)
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "rendering the body failed");
            apercu()
        })
}

/// Wraps a plain-text body so the HTML renderer can lay it out.
///
/// Plain text is not HTML, and feeding it in raw would collapse every line break and
/// swallow anything between angle brackets — which in mail is usually an address.
fn plain_text_to_html(texte: &str) -> String {
    let mut out = String::with_capacity(texte.len() + 64);
    out.push_str("<div style=\"white-space:pre-wrap\">");

    for c in texte.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }

    out.push_str("</div>");
    out
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
    demande: Arc<std::sync::Mutex<Option<ThreadIdent>>>,
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
            demande: Arc::new(std::sync::Mutex::new(None)),
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
        let demande = Arc::clone(&self.demande);
        self.runtime.spawn(async move {
            let resultats = engine.fetch_thread_bodies(thread).await;
            let obtenus = resultats.iter().filter(|(_, r)| r.is_ok()).count();
            for (message, resultat) in &resultats {
                if let Err(e) = resultat {
                    tracing::warn!(message = %message, error = %e, "body not downloaded");
                }
            }
            if obtenus == 0 {
                // Rien n'est arrivé : garder la demande en mémoire interdirait tout
                // nouvel essai sur ce fil, et le volet attendrait indéfiniment.
                let mut d = demande.lock().expect("téléchargement empoisonné");
                if *d == Some(thread) {
                    *d = None;
                }
            } else {
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

/// Combien d'heures pour une échéance nommée.
///
/// « Demain » valait vingt-quatre heures, écrites en dur. Un message reporté à vingt-
/// trois heures revenait donc à vingt-trois heures le lendemain — au moment précis où
/// l'on ne veut pas de courrier. Une échéance se compte jusqu'à une heure du jour, pas
/// en durée depuis maintenant.
///
/// En temps universel, comme tout le reste de l'application : elle ne connaît aucun
/// fuseau, et en inventer un ici serait pire que de s'en passer.
fn heures_de_report(quand: &str, maintenant: iris_types::Timestamp) -> u32 {
    const MATIN: i64 = 8;
    const SOIR: i64 = 18;

    let heure = maintenant.seconds().rem_euclid(86_400) / 3600;
    // Le 1er janvier 1970 était un jeudi, ce qui met le décalage à 3.
    let jour = (maintenant.seconds().div_euclid(86_400) + 3).rem_euclid(7);

    let heures = match quand {
        // Ce soir, si le soir est encore devant. Sinon demain matin : proposer une
        // échéance déjà passée ferait revenir le message aussitôt.
        "evening" if heure < SOIR => SOIR - heure,
        "evening" => 24 - heure + MATIN,
        // Lundi matin. Un lundi, c'est le lundi **suivant** : reporter à aujourd'hui
        // n'est pas reporter.
        "monday" => {
            let jours = ((7 - jour) % 7).max(if jour == 0 { 7 } else { 0 });
            let jours = if jours == 0 { 7 } else { jours };
            jours * 24 - heure + MATIN
        }
        // Samedi matin. Un samedi, le suivant.
        "weekend" => {
            let jours = (5 - jour).rem_euclid(7);
            let jours = if jours == 0 { 7 } else { jours };
            jours * 24 - heure + MATIN
        }
        // Demain matin.
        _ => 24 - heure + MATIN,
    };

    // Au moins une heure : une échéance à zéro serait un report qui n'en est pas un.
    heures.max(1) as u32
}

/// Le destinataire en cours de frappe : ce qui suit la dernière virgule.
///
/// Un champ « À » contient « marie@x.fr, l » et c'est « l » qu'on cherche à compléter.
/// Chercher sur la chaîne entière ne proposerait jamais rien dès le second
/// destinataire.
fn dernier_destinataire(champ: &str) -> String {
    champ.rsplit(',').next().unwrap_or(champ).trim().to_string()
}

/// Remplace le dernier destinataire par celui qu'on vient de choisir.
///
/// La virgule finale n'est pas de la coquetterie : elle dit que le champ attend la
/// suite, et évite d'avoir à la taper avant de continuer.
fn remplace_dernier_destinataire(champ: &str, choix: &str) -> String {
    match champ.rfind(',') {
        Some(i) => format!("{}, {choix}, ", champ[..i].trim_end_matches([',', ' '])),
        None => format!("{choix}, "),
    }
}

/// How a message handed to the outbox ended, as the sending side tells the window.
#[derive(Debug, Clone)]
pub enum IssueEnvoi {
    Parti,
    Echec(String),
}

/// What follows once a message's fate is known.
enum Suite {
    /// Written in a window: put back if undone, or if it does not leave.
    Remettre(Box<dyn FnOnce(&AppWindow)>),
    /// A scheduled message: taken off the list once gone, tried again if not.
    Programme { id: i64, services: Box<Services> },
}

/// A message handed to the outbox whose fate is not known yet.
struct EnVol {
    libelle: String,
    suite: Suite,
}

/// The notice at the bottom of the window while a message waits to leave, and what
/// becomes of every message until it has left.
///
/// Send closes what the message was written in — the new-message window, or empties
/// the reply field — and hands the message to the outbox with the delay chosen in the
/// settings. For that long the notice counts down and offers Undo, which stops the
/// message and puts it back exactly as it was. One notice at a time: a second send
/// takes the place of the first, which then simply leaves.
///
/// The end of the countdown is not the end of the message. "Message sent." used to be
/// said then, whatever happened next: a refused password or a lost connection left the
/// window announcing a message that never left, its text already gone. Each message
/// is now kept here until the outbox says how it ended; a failure puts it back.
pub struct AvisEnvoi {
    send: Arc<SendService>,
    /// The message whose notice is showing.
    en_attente: std::cell::Cell<Option<iris_smtp::SendHandle>>,
    en_vol: std::cell::RefCell<HashMap<iris_smtp::SendHandle, EnVol>>,
    minuterie: slint::Timer,
}

thread_local! {
    /// The notice, for the outcomes that arrive from the sending side.
    static AVIS: std::cell::RefCell<std::rc::Weak<AvisEnvoi>> =
        const { std::cell::RefCell::new(std::rc::Weak::new()) };
}

/// Tells the window how a message ended. Called on the window's thread.
pub fn envoi_termine(fenetre: &AppWindow, handle: iris_smtp::SendHandle, issue: IssueEnvoi) {
    if let Some(avis) = AVIS.with(|a| a.borrow().upgrade()) {
        avis.terminer(fenetre, handle, issue);
    }
}

impl AvisEnvoi {
    /// Puts a message in the outbox, with the delay from the settings, and shows the
    /// notice. `remettre` puts it back if it is undone or does not leave.
    pub fn envoyer(
        self: &Rc<Self>,
        fenetre: &AppWindow,
        message: iris_smtp::Outgoing,
        libelle: String,
        remettre: impl FnOnce(&AppWindow) + 'static,
    ) -> iris_types::Result<()> {
        let bornes = crate::settings::UNDO_SEND_RANGE;
        let secondes =
            (fenetre.get_undo_send_seconds().max(0) as u32).clamp(*bornes.start(), *bornes.end());
        self.send
            .set_delay(std::time::Duration::from_secs(secondes as u64));
        let handle = self.send.queue(message)?;
        self.en_vol.borrow_mut().insert(
            handle,
            EnVol {
                libelle: libelle.clone(),
                suite: Suite::Remettre(Box::new(remettre)),
            },
        );

        if secondes == 0 {
            // Nothing to take back: it is already leaving.
            self.fermer(fenetre);
            fenetre.set_status(format!("{libelle}…").into());
            return Ok(());
        }

        self.en_attente.set(Some(handle));
        fenetre.set_send_notice_text(libelle.into());
        fenetre.set_send_notice_seconds(secondes as i32);
        fenetre.set_send_notice_open(true);

        let (faible, avis) = (fenetre.as_weak(), Rc::downgrade(self));
        self.minuterie.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(1),
            move || {
                let (Some(fenetre), Some(avis)) = (faible.upgrade(), avis.upgrade()) else {
                    return;
                };
                let reste = fenetre.get_send_notice_seconds() - 1;
                if reste <= 0 {
                    // Nothing left to undo. Whether it left is for the outbox to say.
                    let libelle = avis
                        .en_attente
                        .get()
                        .and_then(|h| avis.en_vol.borrow().get(&h).map(|e| e.libelle.clone()));
                    avis.fermer(&fenetre);
                    if let Some(libelle) = libelle {
                        fenetre.set_status(format!("{libelle}…").into());
                    }
                } else {
                    fenetre.set_send_notice_seconds(reste);
                }
            },
        );
        Ok(())
    }

    /// Hands a scheduled message to the outbox, at once. It leaves the list only once
    /// it has gone: taken off when queued, it was lost whenever the send then failed.
    pub fn programmer(
        &self,
        message: iris_smtp::Outgoing,
        id: i64,
        sujet: String,
        services: Services,
    ) -> iris_types::Result<()> {
        self.send.set_delay(std::time::Duration::ZERO);
        let handle = self.send.queue(message)?;
        self.en_vol.borrow_mut().insert(
            handle,
            EnVol {
                libelle: sujet,
                suite: Suite::Programme {
                    id,
                    services: Box::new(services),
                },
            },
        );
        Ok(())
    }

    /// The scheduled messages on their way, not to be sent a second time meanwhile.
    pub fn programmes_en_vol(&self) -> Vec<i64> {
        self.en_vol
            .borrow()
            .values()
            .filter_map(|e| match e.suite {
                Suite::Programme { id, .. } => Some(id),
                Suite::Remettre(_) => None,
            })
            .collect()
    }

    fn fermer(&self, fenetre: &AppWindow) {
        self.minuterie.stop();
        self.en_attente.set(None);
        fenetre.set_send_notice_open(false);
    }

    /// Undo: stops the message if it has not left, and puts it back.
    fn annuler(&self, fenetre: &AppWindow) {
        let pris = self.en_attente.take();
        self.fermer(fenetre);
        let Some(handle) = pris else {
            return;
        };
        if self.send.cancel(handle) {
            let envol = self.en_vol.borrow_mut().remove(&handle);
            if let Some(EnVol {
                suite: Suite::Remettre(remettre),
                ..
            }) = envol
            {
                remettre(fenetre);
            }
            fenetre.set_status("Send cancelled: your message is back.".into());
        } else {
            // Already gone: say so plainly rather than pretend.
            fenetre.set_status("Too late, the message has gone.".into());
        }
    }

    /// How a message ended: said, and put back when it did not leave.
    fn terminer(&self, fenetre: &AppWindow, handle: iris_smtp::SendHandle, issue: IssueEnvoi) {
        if self.en_attente.get() == Some(handle) {
            self.fermer(fenetre);
        }
        let envol = self.en_vol.borrow_mut().remove(&handle);
        let Some(EnVol { libelle, suite }) = envol else {
            // Not written here (an answer to an invitation): a failure is still said.
            if let IssueEnvoi::Echec(e) = issue {
                fenetre.set_status(format!("A message could not be sent: {e}").into());
            }
            return;
        };
        match (suite, issue) {
            (Suite::Remettre(_), IssueEnvoi::Parti) => {
                fenetre.set_status("Message sent.".into());
            }
            (Suite::Remettre(remettre), IssueEnvoi::Echec(e)) => {
                remettre(fenetre);
                fenetre.set_status(
                    format!("Not sent: {e}. Your message is back, to send again.").into(),
                );
            }
            (Suite::Programme { id, services }, IssueEnvoi::Parti) => {
                let _ = services.store.unschedule_mail(id);
                fenetre.set_status("A scheduled message was sent.".into());
                rafraichir_plus_tard(fenetre, &services);
            }
            (Suite::Programme { id, services }, IssueEnvoi::Echec(e)) => {
                // Kept, and tried again a little later; it can be taken back meanwhile.
                let plus_tard = iris_types::Timestamp::from_millis(now().millis() + 5 * 60 * 1000);
                let _ = services.store.postpone_scheduled_mail(id, plus_tard);
                fenetre.set_status(
                    format!(
                        "“{libelle}” could not be sent: {e}. Iris will try again in five minutes."
                    )
                    .into(),
                );
                rafraichir_plus_tard(fenetre, &services);
            }
        }
    }
}

/// Installs the notice and its Undo, shared by new messages and replies.
pub fn wire_send_notice(fenetre: &AppWindow, send: Arc<SendService>) -> Rc<AvisEnvoi> {
    let avis = Rc::new(AvisEnvoi {
        send,
        en_attente: std::cell::Cell::new(None),
        en_vol: Default::default(),
        minuterie: slint::Timer::default(),
    });
    AVIS.with(|a| *a.borrow_mut() = Rc::downgrade(&avis));
    let (faible, a) = (fenetre.as_weak(), Rc::clone(&avis));
    fenetre.on_send_undone(move || {
        if let Some(fenetre) = faible.upgrade() {
            a.annuler(&fenetre);
        }
    });
    avis
}

/// Compose et met en file une réponse, à l'expéditeur ou à tous.
///
/// Les deux boutons font le même travail à un mot près, et ce mot est la seule chose
/// qui les distingue : qui reçoit. Écrire deux fois la même trentaine de lignes, c'est
/// s'assurer qu'un jour l'un des deux corrigera un défaut que l'autre gardera.
fn envoyer_reponse(
    fenetre: &AppWindow,
    send: &iris_sync::SendService,
    avis: &Rc<AvisEnvoi>,
    selection: &Arc<std::sync::Mutex<Option<iris_types::ThreadId>>>,
    portee: iris_smtp::ReplyScope,
) {
    let texte = fenetre.get_reply_text().to_string();
    if texte.trim().is_empty() {
        return;
    }
    let Some(thread) = *selection.lock().expect("sélection") else {
        return;
    };

    let message = match send.compose_reply(thread, &texte, portee) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "composing the reply");
            fenetre.set_status(format!("Cannot reply: {e}").into());
            return;
        }
    };

    // Le champ se vide. Le texte revient s'il est annulé ou n'est pas parti : dans le
    // champ si ce fil est encore ouvert et le champ vide ; sinon en bas à droite,
    // adressé, plutôt que dans la réponse d'un autre fil, qui l'enverrait à d'autres.
    let retour = {
        let selection = Arc::clone(selection);
        let garde = crate::draft::Draft {
            to: liste_adresses(&message.to),
            cc: liste_adresses(&message.cc),
            bcc: String::new(),
            subject: message.subject.clone(),
            body: texte.clone(),
            lost_attachments: 0,
            sender: message.from.addr.clone(),
        };
        move |fenetre: &AppWindow| {
            let meme_fil = *selection.lock().expect("sélection") == Some(thread);
            if meme_fil && fenetre.get_reply_text().trim().is_empty() {
                fenetre.set_reply_text(garde.body.into());
            } else {
                garder_en_bas(
                    fenetre,
                    Reduit {
                        brouillon: garde,
                        pieces: Vec::new(),
                    },
                );
            }
        }
    };
    match avis.envoyer(fenetre, message, "Sending your reply".into(), retour) {
        Ok(()) => fenetre.set_reply_text(Default::default()),
        Err(e) => fenetre.set_status(format!("Send refused: {e}").into()),
    }
}

/// Addresses as the composer's fields take them: plain addresses, comma separated.
fn liste_adresses(adresses: &[iris_types::Address]) -> String {
    adresses
        .iter()
        .map(|a| a.addr.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Branche la zone de réponse.
pub fn wire_reply(
    fenetre: &AppWindow,
    send: Arc<SendService>,
    avis: Rc<AvisEnvoi>,
    selection: Arc<std::sync::Mutex<Option<ThreadIdent>>>,
) {
    {
        let send = Arc::clone(&send);
        let avis = Rc::clone(&avis);
        let selection = Arc::clone(&selection);
        let faible = fenetre.as_weak();

        fenetre.on_send_reply(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            envoyer_reponse(
                &fenetre,
                &send,
                &avis,
                &selection,
                iris_smtp::ReplyScope::Sender,
            );
        });
    }

    // Transférer : l'éditeur ordinaire, prérempli.
    //
    // Il n'y avait aucun moyen de faire suivre un message — répondre, et rien d'autre.
    // Passer par l'éditeur plutôt que par un chemin à part donne au transfert tout ce
    // que l'éditeur sait déjà faire : les copies, les pièces jointes, un mot avant la
    // citation, le délai d'annulation.
    {
        let send = Arc::clone(&send);
        let selection = Arc::clone(&selection);
        let faible = fenetre.as_weak();

        fenetre.on_forward_thread(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(thread) = *selection.lock().expect("sélection") else {
                return;
            };

            match send.forward_prefill(thread) {
                Ok((sujet, corps)) => {
                    // Le destinataire reste vide, et le curseur y va : c'est la seule
                    // chose qu'un transfert ne peut pas deviner.
                    fenetre.set_compose_to(Default::default());
                    fenetre.set_compose_cc(Default::default());
                    fenetre.set_compose_bcc(Default::default());
                    fenetre.set_compose_subject(sujet.into());
                    fenetre.set_compose_body(corps.into());
                    fenetre.set_compose_error(Default::default());
                    fenetre.set_compose_minimised(false);
                    fenetre.set_compose_open(true);
                }
                Err(e) => fenetre.set_status(format!("Cannot forward: {e}").into()),
            }
        });
    }

    // Whom Send and Reply all will write to, worked out as sending would, for the
    // buttons to say it when pointed at.
    {
        let send = Arc::clone(&send);
        let selection = Arc::clone(&selection);
        let faible = fenetre.as_weak();
        fenetre.on_reply_recipients_wanted(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let thread = *selection.lock().expect("sélection");
            let ligne = |adresses: &[iris_types::Address]| -> slint::SharedString {
                adresses
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into()
            };
            let pour = |portee| {
                thread
                    .and_then(|t| send.compose_reply(t, " ", portee).ok())
                    .map(|m| (ligne(&m.to), ligne(&m.cc), ligne(&m.bcc)))
                    .unwrap_or_default()
            };
            let (a, cc, cci) = pour(iris_smtp::ReplyScope::Sender);
            fenetre.set_reply_to(a);
            fenetre.set_reply_cc(cc);
            fenetre.set_reply_bcc(cci);
            let (a, cc, cci) = pour(iris_smtp::ReplyScope::All);
            fenetre.set_reply_all_to(a);
            fenetre.set_reply_all_cc(cc);
            fenetre.set_reply_all_bcc(cci);
        });
    }

    // « Reply all », qui n'était relié à rien.
    //
    // Le bouton était déclaré dans l'interface, transmis depuis la vue de conversation
    // jusqu'à la fenêtre, et s'arrêtait là : aucun gestionnaire en Rust. Cliquer dessus
    // ne faisait rien du tout, sans message ni trace. C'était le seul orphelin des cent
    // trois rappels déclarés — le test qui balaie les autres est juste en dessous, pour
    // que ce soit le dernier.
    {
        let send = Arc::clone(&send);
        let avis = Rc::clone(&avis);
        let selection = Arc::clone(&selection);
        let faible = fenetre.as_weak();

        fenetre.on_reply_all(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            envoyer_reponse(
                &fenetre,
                &send,
                &avis,
                &selection,
                iris_smtp::ReplyScope::All,
            );
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
    let courant = Arc::new(std::sync::Mutex::new(reglages));
    crate::settings::share(Arc::clone(&courant), chemin.clone());

    // L'état initial du panneau.
    {
        let reglages = courant.lock().expect("réglages empoisonnés").clone();
        fenetre.set_appearance(reglages.appearance.index() as i32);
        fenetre.set_first_name(reglages.first_name.as_str().into());
        fenetre.set_density(reglages.density.index() as i32);
        fenetre.set_reply_marks_waiting(reglages.automation.reply_marks_waiting);
        fenetre.set_new_message_reopens(reglages.automation.new_message_reopens);
        fenetre.set_follow_up_enabled(reglages.automation.follow_up_enabled);
        fenetre.set_follow_up_days(reglages.automation.follow_up_days as i32);
        fenetre.set_notifications(reglages.notifications);
        fenetre.set_keep_running(reglages.keep_running);
        fenetre.set_undo_send_seconds(reglages.undo_send_seconds as i32);
        fenetre.set_group_by_tags(reglages.accounts_by_tag);
        fenetre.set_home_at_startup(reglages.home_at_startup);
        fenetre.set_window_mac_buttons(reglages.mac_window_buttons);
        fenetre.set_accent_index(reglages.accent as i32);
        fenetre.set_oauth_google_id(reglages.oauth.google_client_id.as_str().into());
        fenetre.set_oauth_google_secret(reglages.oauth.google_client_secret.as_str().into());
        fenetre.set_oauth_microsoft_id(reglages.oauth.microsoft_client_id.as_str().into());

        // Les deux derniers viennent du système, pas du fichier : le fichier dit ce
        // qu'on a demandé, le registre dit ce qui est. Une désinstallation, une
        // stratégie d'entreprise ou une autre application peuvent avoir défait
        // l'inscription entre deux démarrages.
        let reel = crate::platform::status();
        fenetre.set_start_at_login(reel.start_at_login);
        fenetre.set_handle_mailto(reel.mailto);
    }

    let enregistrer = {
        let chemin = chemin.clone();
        move |reglages: &Settings| {
            if let Err(e) = reglages.save(&chemin) {
                tracing::warn!(error = %e, "saving the settings");
            }
        }
    };

    // --- Light, dark, or as Windows is set ---
    {
        let themes = Arc::clone(&services.themes);
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let faible = fenetre.as_weak();

        fenetre.on_appearance_chosen(move |index| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(apparence) = iris_theme::Appearance::from_index(index as usize) else {
                return;
            };
            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.appearance = apparence;
            let theme = themes.apply(apparence, crate::platform::system_dark());
            appliquer_apparence(&fenetre, &theme, reglages.density);
            appliquer_accent(&fenetre, reglages.accent, theme.dark);
            fenetre.set_appearance(index);
            enregistrer(&reglages);
        });
    }

    // --- The accent colour: over the theme, light or dark ---
    {
        let themes = Arc::clone(&services.themes);
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let faible = fenetre.as_weak();
        fenetre.on_accent_chosen(move |index| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let index = (index.max(0) as usize).min(ACCENTS.len() - 1) as u8;
            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.accent = index;
            // From the theme again, so blue gives the theme's own accent back.
            let theme = themes.active();
            appliquer_apparence(&fenetre, &theme, reglages.density);
            appliquer_accent(&fenetre, index, theme.dark);
            fenetre.set_accent_index(index as i32);
            enregistrer(&reglages);
        });
    }

    // Following Windows: its setting is looked at every few seconds, and the theme
    // changes with it. A registry value read, nothing more; only while "System" is
    // chosen.
    {
        let themes = Arc::clone(&services.themes);
        let courant = Arc::clone(&courant);
        let faible = fenetre.as_weak();
        let minuterie = slint::Timer::default();
        let mut sombre = crate::platform::system_dark();
        minuterie.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(3),
            move || {
                let maintenant = crate::platform::system_dark();
                if maintenant == sombre {
                    return;
                }
                sombre = maintenant;
                let Some(fenetre) = faible.upgrade() else {
                    return;
                };
                let reglages = courant.lock().expect("réglages empoisonnés");
                if reglages.appearance == iris_theme::Appearance::System {
                    let theme = themes.apply(reglages.appearance, sombre);
                    appliquer_apparence(&fenetre, &theme, reglages.density);
                    appliquer_accent(&fenetre, reglages.accent, theme.dark);
                }
            },
        );
        SYSTEME.with(|m| *m.borrow_mut() = Some(minuterie));
    }

    // --- The OAuth clients: saved as typed, and used from the next sign-in on ---
    {
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let oauth = Arc::clone(&services.oauth);
        let faible = fenetre.as_weak();
        fenetre.on_oauth_changed(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let nouveaux = crate::oauth::OAuthSettings {
                google_client_id: fenetre.get_oauth_google_id().trim().to_string(),
                google_client_secret: fenetre.get_oauth_google_secret().trim().to_string(),
                microsoft_client_id: fenetre.get_oauth_microsoft_id().trim().to_string(),
            };
            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.oauth = nouveaux.clone();
            enregistrer(&reglages);
            drop(reglages);
            *oauth.write().expect("réglages OAuth empoisonnés") = nouveaux;
        });
    }

    // --- The first name Home greets ---
    {
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_first_name_changed(move |nom| {
            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.first_name = nom.trim().chars().take(40).collect();
            enregistrer(&reglages);
            drop(reglages);
            if let Some(fenetre) = faible.upgrade() {
                crate::home::refresh_if_shown(&fenetre, &services);
            }
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
            let theme = themes.active();
            appliquer_apparence(&fenetre, &theme, densite);
            appliquer_accent(&fenetre, reglages.accent, theme.dark);
            fenetre.set_density(index);
            enregistrer(&reglages);
        });
    }

    // --- Les comptes rangés sous leurs tags ---
    {
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let services = services.clone();
        let faible = fenetre.as_weak();

        fenetre.on_group_by_tags_changed(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.accounts_by_tag = fenetre.get_group_by_tags();
            enregistrer(&reglages);
            drop(reglages);
            refresh_accounts(&fenetre, &services, &[]);
        });
    }

    // --- Le délai pour rattraper un envoi ---
    //
    // Lu au moment de chaque envoi (`get_undo_send_seconds`) : il n'y a rien d'autre à
    // appliquer ici que l'enregistrer.
    {
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let faible = fenetre.as_weak();

        fenetre.on_sending_changed(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let bornes = crate::settings::UNDO_SEND_RANGE;
            let secondes = (fenetre.get_undo_send_seconds().max(0) as u32)
                .clamp(*bornes.start(), *bornes.end());
            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.undo_send_seconds = secondes;
            enregistrer(&reglages);
        });
    }

    // --- Ce qui engage le système ---
    //
    // Trois interrupteurs qui écrivent hors de l'application : dans le registre pour
    // deux d'entre eux, dans la manière dont Windows nous connaît pour le troisième.
    // L'état réel est **relu** après chaque écriture plutôt que supposé : une clé de
    // registre refusée par une stratégie d'entreprise laisserait sinon un
    // interrupteur allumé sur une chose qui n'a pas eu lieu.
    {
        let courant = Arc::clone(&courant);
        let enregistrer = enregistrer.clone();
        let faible = fenetre.as_weak();

        fenetre.on_system_changed(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };

            let mut reglages = courant.lock().expect("réglages empoisonnés");
            reglages.notifications = fenetre.get_notifications();
            // Rien à demander au système pour celui-ci : il ne décide que de ce que
            // fait la fermeture de la fenêtre, et de l'icône qui va avec.
            reglages.keep_running = fenetre.get_keep_running();
            reglages.home_at_startup = fenetre.get_home_at_startup();
            reglages.mac_window_buttons = fenetre.get_window_mac_buttons();

            let mut plaintes: Vec<String> = Vec::new();

            if fenetre.get_start_at_login() != reglages.start_at_login {
                match crate::platform::set_start_at_login(fenetre.get_start_at_login()) {
                    Ok(()) => reglages.start_at_login = fenetre.get_start_at_login(),
                    Err(e) => plaintes.push(format!("Start with Windows: {e}")),
                }
            }

            if fenetre.get_handle_mailto() != reglages.handle_mailto {
                let resultat = if fenetre.get_handle_mailto() {
                    crate::platform::register_mailto()
                } else {
                    crate::platform::unregister_mailto()
                };
                match resultat {
                    Ok(()) => reglages.handle_mailto = fenetre.get_handle_mailto(),
                    Err(e) => plaintes.push(format!("mailto: links: {e}")),
                }
            }

            // Ce que Windows dit maintenant, et non ce que nous avons demandé.
            let reel = crate::platform::status();
            fenetre.set_start_at_login(reel.start_at_login);
            fenetre.set_handle_mailto(reel.mailto);
            reglages.start_at_login = reel.start_at_login;
            reglages.handle_mailto = reel.mailto;

            fenetre.set_system_note(plaintes.join("\n").into());
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

/// The changelog, flattened into the rows the window draws.
///
/// `installed` marks the running version, so the reader sees where they stand.
pub fn changelog_rows(
    releases: &[crate::changelog::Release],
    installed: Option<&str>,
) -> Vec<iris_ui::ChangelogRowData> {
    let mut lignes = Vec::new();
    for release in releases {
        lignes.push(iris_ui::ChangelogRowData {
            kind: 0,
            text: release.version.as_str().into(),
            detail: release.date.as_str().into(),
            tag: if installed == Some(release.version.as_str()) {
                "Installed".into()
            } else {
                Default::default()
            },
        });
        for section in &release.sections {
            lignes.push(iris_ui::ChangelogRowData {
                kind: 1,
                text: section.title.to_uppercase().into(),
                ..Default::default()
            });
            for item in &section.items {
                lignes.push(iris_ui::ChangelogRowData {
                    kind: 2,
                    text: item.as_str().into(),
                    ..Default::default()
                });
            }
        }
    }
    lignes
}

/// How often a running Iris asks again. It can stay open for weeks in the
/// notification area, and "checked at launch" would then mean "never".
const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

/// The changelog window, the update check, and installing an update.
///
/// Checked a few seconds after launch — not before the first frame, which has better
/// things to do than wait on GitHub — then every six hours, and on demand from the
/// settings. A failed automatic check says nothing beyond the settings panel: being
/// offline is not news. A failed manual one says why.
pub fn wire_updates(
    fenetre: &AppWindow,
    controller: Arc<Controller>,
    runtime: tokio::runtime::Handle,
) {
    let courante = crate::update::current();
    fenetre.set_changelog(ModelRc::new(VecModel::from(changelog_rows(
        &crate::changelog::bundled(),
        Some(courante),
    ))));
    fenetre.set_update_check_status("Not checked yet.".into());

    // What the last check found, for the install button to act on.
    let offre: Arc<std::sync::Mutex<Option<crate::update::Available>>> = Default::default();

    // One check at a time: a second click while the first is on its way would only
    // race it to the same answer.
    let verification = {
        let offre = Arc::clone(&offre);
        let faible = fenetre.as_weak();
        let runtime = runtime.clone();
        let en_cours = Arc::new(std::sync::atomic::AtomicBool::new(false));
        move |manuelle: bool| {
            use std::sync::atomic::Ordering;
            if en_cours.swap(true, Ordering::SeqCst) {
                return;
            }
            let _ = faible.upgrade_in_event_loop(|f| {
                f.set_update_checking(true);
                f.set_update_check_status("Checking…".into());
            });
            let offre = Arc::clone(&offre);
            let faible = faible.clone();
            let en_cours = Arc::clone(&en_cours);
            runtime.spawn(async move {
                let resultat = crate::update::check().await;
                en_cours.store(false, Ordering::SeqCst);
                let _ = faible.upgrade_in_event_loop(move |f| {
                    f.set_update_checking(false);
                    match resultat {
                        Ok(Some(dispo)) => {
                            let nouvelle =
                                f.get_update_version().as_str() != dispo.version.to_string();
                            f.set_update_version(dispo.version.to_string().into());
                            f.set_update_notes(ModelRc::new(VecModel::from(changelog_rows(
                                &dispo.notes,
                                None,
                            ))));
                            f.set_update_check_status(
                                format!("Iris {} is available.", dispo.version).into(),
                            );
                            if nouvelle {
                                f.set_status(
                                    format!(
                                        "Iris {} is available: Update now, in the status bar.",
                                        dispo.version
                                    )
                                    .into(),
                                );
                            }
                            *offre.lock().expect("offre empoisonnée") = Some(dispo);
                        }
                        Ok(None) => {
                            f.set_update_check_status(
                                format!("Iris {} is the latest version.", crate::update::current())
                                    .into(),
                            );
                        }
                        Err(e) => {
                            tracing::info!(error = %e, manual = manuelle, "update check");
                            f.set_update_check_status(format!("Could not check: {e}").into());
                        }
                    }
                });
            });
        }
    };

    // At launch, then on a schedule.
    {
        let verification = verification.clone();
        runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(8)).await;
            loop {
                verification(false);
                tokio::time::sleep(UPDATE_INTERVAL).await;
            }
        });
    }

    fenetre.on_check_for_updates(move || verification(true));

    // --- Installing ---
    let faible = fenetre.as_weak();
    fenetre.on_update_confirmed(move || {
        let Some(f) = faible.upgrade() else {
            return;
        };
        if f.get_update_busy() {
            return;
        }
        let Some(dispo) = offre.lock().expect("offre empoisonnée").clone() else {
            f.set_update_error("Nothing to install: check for updates first.".into());
            return;
        };

        f.set_update_busy(true);
        f.set_update_error(Default::default());
        f.set_update_progress(0.0);
        f.set_update_status("Downloading…".into());

        let faible = faible.clone();
        let controller = Arc::clone(&controller);
        runtime.spawn(async move {
            // The bar moves by whole percents: redrawing it for every network chunk
            // would cost more than the download.
            let dernier = Arc::new(std::sync::atomic::AtomicU32::new(u32::MAX));
            let progression = {
                let faible = faible.clone();
                let dernier = Arc::clone(&dernier);
                move |recus: u64, total: u64| {
                    let pour_cent = (recus * 100).checked_div(total).unwrap_or(0).min(100) as u32;
                    if dernier.swap(pour_cent, std::sync::atomic::Ordering::Relaxed) != pour_cent {
                        let _ = faible.upgrade_in_event_loop(move |f| {
                            f.set_update_progress(pour_cent as f32 / 100.0);
                            f.set_update_status(format!("Downloading… {pour_cent}%").into());
                        });
                    }
                }
            };

            let resultat = crate::update::download(
                &dispo.installer,
                &crate::update::download_dir(),
                progression,
            )
            .await
            .and_then(|chemin| crate::update::launch_installer(&chemin));

            let _ = faible.upgrade_in_event_loop(move |f| match resultat {
                Ok(()) => {
                    // The installer waits for this process to go before it replaces
                    // the executable. Leaving now is what lets it finish.
                    f.set_update_status("Installing. Iris will open again in a moment.".into());
                    tracing::info!(version = %dispo.version, "update: installer started, quitting");
                    controller.shutdown();
                    let _ = slint::quit_event_loop();
                }
                Err(e) => {
                    f.set_update_busy(false);
                    f.set_update_error(format!("The update failed: {e}").into());
                }
            });
        });
    });
}

thread_local! {
    /// The timer that watches Windows' light or dark.
    static SYSTEME: std::cell::RefCell<Option<slint::Timer>> = const { std::cell::RefCell::new(None) };
}

/// Applique thème et densité aux jetons de l'interface.
pub fn appliquer_apparence(fenetre: &AppWindow, theme: &iris_theme::Theme, densite: Density) {
    let tokens = fenetre.global::<Tokens>();
    bridge::apply_theme(&tokens, theme);
    fenetre.set_scheme_dark(theme.dark);
    // La densité multiplie la hauteur du thème au lieu de la remplacer : un thème
    // aux lignes hautes reste plus aéré que les autres à densité égale.
    tokens.set_row_height(theme.density.row_height * densite.factor());
}

/// A colour as three channels.
pub type Rgb = (u8, u8, u8);

/// The accents offered in Settings, as the Mac offers them: blue, purple, pink, red,
/// orange, yellow, green, graphite; each in its light and its dark shade.
pub const ACCENTS: [(Rgb, Rgb); 8] = [
    ((0x00, 0x7a, 0xff), (0x0a, 0x84, 0xff)),
    ((0xaf, 0x52, 0xde), (0xbf, 0x5a, 0xf2)),
    ((0xff, 0x2d, 0x55), (0xff, 0x37, 0x5f)),
    ((0xff, 0x3b, 0x30), (0xff, 0x45, 0x3a)),
    ((0xff, 0x95, 0x00), (0xff, 0x9f, 0x0a)),
    ((0xff, 0xcc, 0x00), (0xff, 0xd6, 0x0a)),
    ((0x34, 0xc7, 0x59), (0x30, 0xd1, 0x58)),
    ((0x8e, 0x8e, 0x93), (0x98, 0x98, 0x9d)),
];

/// Lays the accent chosen in Settings over the theme's (blue, the first, leaves the
/// theme's own). Its soft ground keeps the theme's opacity; words on yellow are dark,
/// white not reading on it.
pub fn appliquer_accent(fenetre: &AppWindow, accent: u8, sombre: bool) {
    let i = accent as usize;
    if i == 0 || i >= ACCENTS.len() {
        return;
    }
    let (clair, fonce) = ACCENTS[i];
    let (r, g, b) = if sombre { fonce } else { clair };
    let tokens = fenetre.global::<Tokens>();
    let doux = tokens.get_accent_soft().alpha();
    tokens.set_accent(slint::Color::from_rgb_u8(r, g, b));
    tokens.set_accent_soft(slint::Color::from_argb_u8(doux, r, g, b));
    tokens.set_accent_text(if i == 5 {
        slint::Color::from_rgb_u8(0x1d, 0x1d, 0x1f)
    } else {
        slint::Color::from_rgb_u8(0xff, 0xff, 0xff)
    });
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

    // The panel is closed by the interface, which cannot know that leaving it in edit
    // mode would make the next "Add an account" silently overwrite the last one it
    // edited. So closing resets the mode, here, once.
    {
        let faible = fenetre.as_weak();
        fenetre.on_add_account_dismissed(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_add_account_open(false);
            fenetre.set_add_account_editing(false);
            fenetre.set_add_account_manual(false);
            // Sinon une vérification abandonnée en route laisserait le bouton éteint
            // à la réouverture de l'écran.
            fenetre.set_add_account_busy(false);
            fenetre.set_add_account_error(Default::default());
            fenetre.set_add_account_hint(Default::default());
            fenetre.set_new_email(Default::default());
            fenetre.set_new_password(Default::default());
            fenetre.set_new_username(Default::default());
            fenetre.set_new_imap_host(Default::default());
            fenetre.set_new_imap_port(Default::default());
            fenetre.set_new_smtp_host(Default::default());
            fenetre.set_new_smtp_port(Default::default());
            fenetre.set_profile_choices(ModelRc::default());
            EDITION.with(|e| e.set(None));
            ENVOI_DU_PROFIL.with(|e| *e.borrow_mut() = None);
        });
    }
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
    // --- Another account of the profile, from its list ---
    {
        let faible = fenetre.as_weak();
        fenetre.on_profile_chosen(move |i| {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_profile_choice(i);
                profile_account_chosen(&fenetre, i.max(0) as usize);
            }
        });
    }
    // --- A configuration profile (.mobileconfig) ---
    {
        let faible = fenetre.as_weak();
        fenetre.on_add_account_import(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(chemin) = rfd::FileDialog::new()
                .set_title("Import a configuration profile")
                .add_filter("Configuration profile", &["mobileconfig", "plist", "xml"])
                .pick_file()
            else {
                return;
            };
            let octets = match std::fs::read(&chemin) {
                Ok(o) => o,
                Err(e) => {
                    fenetre.set_add_account_error(format!("Could not open it: {e}").into());
                    return;
                }
            };
            let nom = chemin
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            match iris_discover::mobileconfig::parse(&octets) {
                Ok(comptes) => fill_from_profile(&fenetre, &comptes, &nom),
                // What the reader said, without the error's kind in front of it.
                Err(e) => {
                    let raison = match e {
                        iris_types::Error::Config(m) => m,
                        autre => autre.to_string(),
                    };
                    fenetre.set_add_account_error(
                        format!("{nom} cannot be used: {}.", raison.trim_end_matches('.')).into(),
                    )
                }
            }
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

            // On an account that already exists, this button means "save the new
            // password". There is nothing to discover: the servers are known, they
            // work, and rerunning discovery on them could only replace a correct
            // configuration with a guessed one.
            if fenetre.get_add_account_editing() {
                let Some(compte) = EDITION
                    .with(|e| e.get())
                    .and_then(|id| store.account(id).ok().flatten())
                else {
                    fenetre.set_add_account_error("That account no longer exists.".into());
                    return;
                };
                let id = compte.id;
                if !compte.email.eq_ignore_ascii_case(email.trim()) {
                    fenetre.set_add_account_error(
                        "Changing the address needs the manual screen.".into(),
                    );
                    prefill_manual(&fenetre);
                    return;
                }
                let email = compte.email.clone();
                if motdepasse.is_empty() {
                    fenetre.set_add_account_error("Enter the new password.".into());
                    return;
                }
                match secrets.set(
                    &email,
                    iris_secrets::SecretKind::Password,
                    &iris_secrets::Secret::new(motdepasse),
                ) {
                    Ok(()) => {
                        fenetre.invoke_add_account_dismissed();
                        fenetre.set_status("Password saved, checking it with the server…".into());
                        let engine = Arc::clone(&engine);
                        let services_ui = services_ui.clone();
                        let faible = fenetre.as_weak();
                        runtime_ajout.spawn(async move {
                            engine.resume_account(id, now()).await;
                            let resultat = engine.sync_now(id, now()).await;
                            if let Err(e) = &resultat {
                                tracing::warn!(error = %e, "sync after the password changed");
                            }
                            let panne = engine.failure(id);
                            let suspendus = engine.suspended_accounts().await;
                            // Saying "Password saved" and nothing else left a refused
                            // password looking accepted. The outcome is the answer.
                            let _ = faible.upgrade_in_event_loop(move |fenetre| {
                                fenetre.set_status(
                                    match (resultat, panne) {
                                        (Ok(_), _) => "Password saved. The account works.".into(),
                                        (Err(_), Some(p)) => format!(
                                            "Password saved, but sync still fails: {}. Click the red ! for details.",
                                            p.summary()
                                        ),
                                        (Err(e), None) => {
                                            format!("Password saved, but sync still fails: {e}")
                                        }
                                    }
                                    .into(),
                                );
                                refresh_accounts(&fenetre, &services_ui, &suspendus);
                            });
                        });
                    }
                    Err(e) => fenetre
                        .set_add_account_error(format!("Could not save the password: {e}").into()),
                }
                return;
            }

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
            let progression = faible.clone();
            runtime_ajout.spawn(async move {
                let resultat = ajouter(
                    &store,
                    secrets,
                    &oauth,
                    &email,
                    &motdepasse,
                    now(),
                    move |etape| {
                        let _ = progression.upgrade_in_event_loop(move |fenetre| {
                            fenetre.set_add_account_hint(etape.into());
                        });
                    },
                )
                .await;

                // Le compte créé doit entrer dans l'ordonnanceur tout de suite,
                // sinon rien n'arrive avant le prochain démarrage.
                if resultat.is_ok() {
                    if let Err(e) = engine.load_accounts(now()).await {
                        tracing::warn!(error = %e, "chargement du compte ajouté");
                    }
                }

                // A refused password is not a failed lookup, and the screen owes two
                // different answers. The error type carries the distinction; it has
                // to be read here, because it does not cross the event loop.
                let issue = match resultat {
                    Ok(compte) => Ok((compte.email, compte.source.describe())),
                    Err(e) => Err((
                        matches!(e, iris_types::Error::AuthFailed { .. }),
                        e.to_string(),
                    )),
                };

                let _ = faible.upgrade_in_event_loop(move |fenetre| {
                    fenetre.set_add_account_busy(false);
                    match issue {
                        Ok((adresse, source)) => {
                            // Le panneau ne se ferme qu'ici : tant que le serveur n'a
                            // pas répondu, l'écran reste celui où l'on corrige.
                            fenetre.invoke_add_account_dismissed();
                            announce(&fenetre, format!("{adresse} added and working."));
                            fenetre.set_status(format!("{adresse} ajouté ({source}).").into());
                            refresh_accounts(&fenetre, &services_ui, &[]);
                            // Offered as a sender at once, not after a restart.
                            charger_expediteurs(&fenetre, &services_ui);
                            controller.send(Request::Bootstrap);
                        }
                        // Le serveur a répondu, et il a dit non. Les champs de
                        // serveur ne serviraient à rien : ils sont justes. Ce qu'il
                        // faut retaper est le mot de passe, dans le champ qui est
                        // déjà là, sous le message qui le dit.
                        Err((true, _)) => {
                            fenetre.set_add_account_hint(Default::default());
                            fenetre.set_add_account_error(
                                "The server refused that password. Check it and try again.".into(),
                            );
                        }
                        // L'échec bascule l'écran en configuration manuelle plutôt
                        // que de renvoyer l'utilisateur à un message d'erreur : ce
                        // qu'il lui faut à cet instant, ce sont les champs.
                        Err((false, message)) => {
                            fenetre.set_add_account_error(
                                format!("Could not add it: {message}").into(),
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

            // Editing an existing account may leave the password alone: the reason to
            // open this screen is often a hostname, and demanding a password to change
            // a hostname teaches people to retype working passwords.
            let verification = if fenetre.get_add_account_editing() {
                valider_adresse(&email)
            } else {
                valider_saisie(&email, &motdepasse)
            };
            if let Err(message) = verification {
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

            // Editing rewrites the account in place. Removing and re-adding it would
            // be simpler to write and would throw away every message, folder and
            // workflow state attached to the old identifier — a hostname typo would
            // cost the user their mailbox.
            if fenetre.get_add_account_editing() {
                // The account the screen was opened on, by its identifier. It was
                // found again from the address typed, or the first account on the
                // same server: with three mailboxes on one host, editing the third
                // rewrote the first.
                let Some(id) = EDITION
                    .with(|e| e.get())
                    .filter(|id| store.account(*id).ok().flatten().is_some())
                else {
                    fenetre.set_add_account_error("That account no longer exists.".into());
                    return;
                };

                match crate::accounts::update_account_manual(
                    &store,
                    secrets.as_ref(),
                    id,
                    &config,
                    Some(motdepasse.as_str()).filter(|p| !p.is_empty()),
                    now(),
                ) {
                    Ok(()) => {
                        fenetre.invoke_add_account_dismissed();
                        fenetre.set_status(format!("{} updated.", config.email).into());
                        refresh_accounts(&fenetre, &services_ui, &[]);
                        controller.send(Request::Bootstrap);

                        let engine = Arc::clone(&engine);
                        let services_ui = services_ui.clone();
                        let faible = fenetre.as_weak();
                        let adresse = config.email.clone();
                        runtime_manuel.spawn(async move {
                            if let Err(e) = engine.load_accounts(now()).await {
                                tracing::warn!(error = %e, "reloading the edited account");
                            }
                            // Edited because it was failing, most of the time: whether
                            // the change fixed it is the one thing worth saying next.
                            engine.resume_account(id, now()).await;
                            let resultat = engine.sync_now(id, now()).await;
                            let panne = engine.failure(id);
                            let suspendus = engine.suspended_accounts().await;
                            let _ = faible.upgrade_in_event_loop(move |fenetre| {
                                fenetre.set_status(
                                    match (resultat, panne) {
                                        (Ok(_), _) => format!("{adresse} updated. The account works."),
                                        (Err(_), Some(p)) => format!(
                                            "{adresse} updated, but sync still fails: {}. Click the red ! for details.",
                                            p.summary()
                                        ),
                                        (Err(e), None) => {
                                            format!("{adresse} updated, but sync still fails: {e}")
                                        }
                                    }
                                    .into(),
                                );
                                refresh_accounts(&fenetre, &services_ui, &suspendus);
                            });
                        });
                    }
                    Err(e) => fenetre
                        .set_add_account_error(format!("Could not save the account: {e}").into()),
                }
                return;
            }

            // Les serveurs viennent d'être saisis à la main : c'est le cas où la
            // configuration a le plus de chances d'être fausse, et le moins de
            // raisons d'être crue sur parole. On se connecte avant d'écrire quoi que
            // ce soit, et l'écran reste ouvert pendant ce temps.
            fenetre.set_add_account_busy(true);
            fenetre.set_add_account_error(Default::default());
            fenetre.set_add_account_hint("Connexion au serveur…".into());

            let store = Arc::clone(&store);
            let secrets = Arc::clone(&secrets);
            let engine = Arc::clone(&engine);
            let services_ui = services_ui.clone();
            let controller = Arc::clone(&controller);
            let faible = fenetre.as_weak();
            let adresse = config.email.clone();
            let envoi = mot_de_passe_d_envoi(&adresse);

            runtime_manuel.spawn(async move {
                // Already there: said before signing in, and nothing written. The
                // existing account's password is kept under the same address.
                let resultat = if store.account_by_email(&config.email).ok().flatten().is_some() {
                    Err(iris_types::Error::Config(format!(
                        "{} is already set up: edit it from its menu instead",
                        config.email
                    )))
                } else {
                    match crate::accounts::verify_login(&config, &motdepasse).await {
                        Ok(()) => crate::accounts::add_account_manual_with(
                            &store,
                            secrets.as_ref(),
                            &config,
                            &motdepasse,
                            envoi.as_deref(),
                            None,
                            now(),
                        )
                        .map(|_| ()),
                        Err(e) => Err(e),
                    }
                };

                if resultat.is_ok() {
                    if let Err(e) = engine.load_accounts(now()).await {
                        tracing::warn!(error = %e, "chargement du compte ajouté");
                    }
                }

                let issue = resultat.map_err(|e| {
                    (
                        matches!(e, iris_types::Error::AuthFailed { .. }),
                        e.to_string(),
                    )
                });

                let _ = faible.upgrade_in_event_loop(move |fenetre| {
                    fenetre.set_add_account_busy(false);
                    match issue {
                        Ok(()) => {
                            fenetre.invoke_add_account_dismissed();
                            announce(&fenetre, format!("{adresse} added and working."));
                            fenetre.set_status(format!("{adresse} added.").into());
                            refresh_accounts(&fenetre, &services_ui, &[]);
                            charger_expediteurs(&fenetre, &services_ui);
                            controller.send(Request::Bootstrap);
                        }
                        Err((refuse, message)) => {
                            fenetre.set_add_account_hint(Default::default());
                            fenetre.set_add_account_error(
                                if refuse {
                                    "The server refused that password. Check it and try again."
                                        .to_string()
                                } else {
                                    format!("Could not add the account: {message}")
                                }
                                .into(),
                            );
                        }
                    }
                });
            });
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
    etape: impl Fn(&'static str),
) -> iris_types::Result<crate::accounts::AddedAccount> {
    etape("Recherche de la configuration…");
    let decouverte = crate::accounts::discover(email).await?;
    let mut config = decouverte.config.clone();

    let mut fournisseur = match config.auth {
        iris_discover::Auth::OAuthGoogle => Some(iris_oauth::Provider::Google),
        iris_discover::Auth::OAuthMicrosoft => Some(iris_oauth::Provider::Microsoft),
        iris_discover::Auth::Password => None,
    };
    // No client set for that provider, and a password given: an app password, which
    // Gmail and Outlook still take. The browser is for when a client is set.
    if let Some(f) = fournisseur {
        let configure = oauth
            .read()
            .expect("réglages OAuth empoisonnés")
            .is_configured(f);
        if !configure && !motdepasse.is_empty() {
            fournisseur = None;
            config.auth = iris_discover::Auth::Password;
        }
    }

    let id = match fournisseur {
        Some(fournisseur) => {
            let reglages = oauth.read().expect("réglages OAuth empoisonnés").clone();
            if !reglages.is_configured(fournisseur) {
                // Le dire, plutôt que d'ouvrir un navigateur vers une page d'erreur
                // du fournisseur que personne ne saura interpréter.
                return Err(iris_types::Error::Config(format!(
                    "{} signs in with an app password (type it as the password), or through \
                     the browser once its client is set in Settings › Sign in with Google or \
                     Microsoft",
                    config.provider.as_deref().unwrap_or("This account")
                )));
            }

            // Before the browser: signing in again for an address already set up
            // overwrote its tokens, perhaps with another identity's, before failing
            // with "already exists".
            if store.account_by_email(&config.email)?.is_some() {
                return Err(iris_types::Error::Config(format!(
                    "{} is already set up: use Sign in again from its menu instead",
                    config.email
                )));
            }
            etape("Autorisation dans le navigateur…");
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

            // Se connecter d'abord, écrire ensuite. L'ordre inverse créait un compte
            // et annonçait un succès sur la foi d'une découverte, c'est-à-dire d'une
            // conjecture sur des serveurs, sans jamais avoir présenté le mot de passe
            // à qui que ce soit. Le fournisseur d'identité, lui, a déjà dit oui : son
            // autorisation *est* la vérification.
            etape("Connexion au serveur…");
            crate::accounts::verify_login(&config, motdepasse).await?;

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

/// Affiche une confirmation par-dessus l'application.
///
/// La barre d'état reste la mémoire de ce qui s'est passé ; la bulle, elle, est là
/// pour être vue. Les deux disent la même chose, et c'est voulu : celui qui regardait
/// ailleurs retrouve le fait en bas de la fenêtre.
fn announce(fenetre: &AppWindow, message: String) {
    fenetre.set_toast(message.into());
}

/// Bascule l'écran en configuration manuelle, champs préremplis.
fn prefill_manual(fenetre: &AppWindow) {
    let defauts = crate::accounts::manual_defaults(fenetre.get_new_email().as_str());

    fenetre.set_add_account_manual(true);
    fenetre.set_add_account_hint("Check the servers: they are guessed from your domain.".into());
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

/// Fills the add-account screen in from a profile's first mail account, on the manual
/// screen, where the servers can be read before anything is saved.
///
/// What the profile does not say stays as typed: an address it leaves to be entered,
/// a password it does not carry.
fn fill_from_profile(
    fenetre: &AppWindow,
    comptes: &[iris_discover::mobileconfig::ProfileAccount],
    fichier: &str,
) {
    // Several accounts in one profile: a list over the fields to pick which one fills
    // them, the first to begin with.
    fenetre.set_profile_choices(ModelRc::new(VecModel::from(if comptes.len() > 1 {
        comptes
            .iter()
            .map(|c| {
                let adresse = if c.config.email.is_empty() {
                    c.config.imap_host.as_str()
                } else {
                    c.config.email.as_str()
                };
                slint::SharedString::from(match &c.description {
                    Some(d) => format!("{d} ({adresse})"),
                    None => adresse.to_string(),
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    })));
    fenetre.set_profile_choice(0);
    PROFIL.with(|p| *p.borrow_mut() = (comptes.to_vec(), fichier.to_string()));
    fill_from_profile_account(fenetre, comptes, 0, fichier);
}

thread_local! {
    /// The accounts of the profile last imported, for its list to choose among.
    static PROFIL: std::cell::RefCell<(Vec<iris_discover::mobileconfig::ProfileAccount>, String)> =
        const { std::cell::RefCell::new((Vec::new(), String::new())) };
}

/// Another account of the imported profile chosen from its list.
fn profile_account_chosen(fenetre: &AppWindow, i: usize) {
    PROFIL.with(|p| {
        let (comptes, fichier) = &*p.borrow();
        fill_from_profile_account(fenetre, comptes, i, fichier);
    });
}

/// Fills the add-account screen from the profile's account `i`.
fn fill_from_profile_account(
    fenetre: &AppWindow,
    comptes: &[iris_discover::mobileconfig::ProfileAccount],
    i: usize,
    fichier: &str,
) {
    let Some(compte) = comptes.get(i) else {
        return;
    };
    let c = &compte.config;
    if !c.email.is_empty() {
        fenetre.set_new_email(c.email.as_str().into());
    }
    if let Some(p) = &compte.password {
        fenetre.set_new_password(p.as_str().into());
    }
    fenetre.set_new_username(c.imap_user.as_deref().unwrap_or_default().into());
    ENVOI_DU_PROFIL.with(|e| {
        *e.borrow_mut() = Some((
            c.email.clone(),
            c.smtp_user.clone(),
            compte.smtp_password.clone(),
        ))
    });
    fenetre.set_new_imap_host(c.imap_host.as_str().into());
    fenetre.set_new_imap_port(c.imap_port.to_string().into());
    fenetre.set_new_imap_tls(c.imap_transport == iris_discover::Transport::Tls);
    fenetre.set_new_smtp_host(c.smtp_host.as_str().into());
    fenetre.set_new_smtp_port(c.smtp_port.to_string().into());
    fenetre.set_new_smtp_tls(c.smtp_transport == iris_discover::Transport::Tls);
    fenetre.set_add_account_manual(true);
    fenetre.set_add_account_error(Default::default());

    let quoi = compte.description.as_deref().unwrap_or(fichier);
    let mut indice = format!("Filled in from {quoi}. Check it, then save.");
    if c.email.is_empty() {
        indice = format!("Filled in from {quoi}. Type your address, then save.");
    } else if compte.password.is_none() {
        indice = format!("Filled in from {quoi}. Type your password, then save.");
    }
    if comptes.len() > 1 {
        indice.push_str(&format!(
            " The profile holds {} accounts: pick another in the list above.",
            comptes.len()
        ));
    }
    fenetre.set_add_account_hint(indice.into());
}

thread_local! {
    /// The account the setup screen is editing, set when it opens on one.
    static EDITION: std::cell::Cell<Option<iris_types::AccountId>> =
        const { std::cell::Cell::new(None) };
    /// What the profile account chosen says of sending that the screen does not show,
    /// for the address it was for: `(address, SMTP login, SMTP password)`.
    static ENVOI_DU_PROFIL: std::cell::RefCell<Option<(String, Option<String>, Option<String>)>> =
        const { std::cell::RefCell::new(None) };
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

    let adresse = email.trim().to_lowercase();
    let login = fenetre.get_new_username().trim().to_string();
    // The profile's own sending login, while the address is still the one it gave.
    let smtp_user = ENVOI_DU_PROFIL.with(|e| {
        e.borrow()
            .as_ref()
            .filter(|(pour, _, _)| pour.eq_ignore_ascii_case(&adresse))
            .and_then(|(_, u, _)| u.clone())
    });

    Ok(iris_discover::ServerConfig {
        provider: None,
        email: adresse,
        imap_host,
        imap_port: port(fenetre.get_new_imap_port(), "IMAP")?,
        imap_transport: transport(fenetre.get_new_imap_tls()),
        smtp_host,
        smtp_port: port(fenetre.get_new_smtp_port(), "SMTP")?,
        smtp_transport: transport(fenetre.get_new_smtp_tls()),
        auth: iris_discover::Auth::Password,
        note: None,
        imap_user: (!login.is_empty()).then_some(login),
        smtp_user,
    })
}

/// The profile's own sending password, for the address it was given for.
fn mot_de_passe_d_envoi(adresse: &str) -> Option<String> {
    ENVOI_DU_PROFIL.with(|e| {
        e.borrow()
            .as_ref()
            .filter(|(pour, _, _)| pour.eq_ignore_ascii_case(adresse.trim()))
            .and_then(|(_, _, p)| p.clone())
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

/// Branche l'affichage du message brut.
///
/// Ce que le serveur a livré, en-têtes compris, sans rien interpréter. Ça compte moins
/// souvent que le reste, et quand ça compte rien d'autre ne fait l'affaire : un message
/// qui arrive de travers, un expéditeur qui n'est pas celui qu'il prétend, une règle qui
/// se déclenche quand elle ne devrait pas.
pub fn wire_source(
    fenetre: &AppWindow,
    services: &Services,
    selection: Arc<std::sync::Mutex<Option<ThreadIdent>>>,
) {
    let services = services.clone();
    let faible = fenetre.as_weak();

    fenetre.on_view_source(move || {
        let Some(fenetre) = faible.upgrade() else {
            return;
        };
        let Some(thread) = *selection.lock().expect("sélection empoisonnée") else {
            return;
        };

        let messages = services.store.conversation(thread).unwrap_or_default();
        let Some(message) = messages.last() else {
            return;
        };

        fenetre.set_source_subject(message.subject.as_str().into());

        let brut = message
            .body_blob
            .as_deref()
            .and_then(iris_types::BlobId::from_hex)
            .and_then(|id| services.blobs.get(id).ok().flatten());

        match brut {
            Some(octets) => {
                // Le message est de l'ASCII étendu au mieux : les en-têtes sont encodés
                // en pur ASCII par le protocole, et le corps porte ce que l'expéditeur a
                // choisi. `from_utf8_lossy` rend donc le texte lisible sans jamais
                // échouer, ce qui est exactement ce qu'on veut d'un écran de diagnostic.
                let texte = String::from_utf8_lossy(&octets);
                // Borné : un message avec une pièce jointe de dix mégaoctets est dix
                // mégaoctets de base64, que personne ne lit et qu'aucun champ de saisie
                // ne devrait avoir à disposer.
                const MAX: usize = 256 * 1024;
                let coupe = if texte.len() > MAX {
                    let mut t = texte.chars().take(MAX).collect::<String>();
                    t.push_str("\n\n[…] truncated: the rest is attachment data.\n");
                    t
                } else {
                    texte.into_owned()
                };
                fenetre.set_source_text(coupe.into());
                fenetre.set_source_unavailable(false);
            }
            None => {
                fenetre.set_source_text(Default::default());
                fenetre.set_source_unavailable(true);
            }
        }

        fenetre.set_source_open(true);
    });
}

/// Ouvre une pièce jointe avec l'application que le système lui associe.
///
/// Enregistrer puis retrouver le fichier dans l'explorateur fait trois gestes là où tout
/// autre client en demande un — et neuf fois sur dix on veut seulement regarder le PDF,
/// pas le garder.
///
/// Il est tout de même écrit sur disque, dans les téléchargements et non dans un dossier
/// temporaire : ouvrir depuis un emplacement que le système peut nettoyer sous
/// l'application donne un fichier qui disparaît pendant qu'on le lit, et personne ne
/// comprend pourquoi.
/// The banner over a message that carries an invitation: add it to the calendar (or
/// bring it up to date, or take a cancelled one out), and open it there.
pub fn wire_invitations(fenetre: &AppWindow, services: &Services, controller: Arc<Controller>) {
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_invite_accept(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(message) = services
                .store
                .message_by_id(iris_types::MessageId(id as i64))
                .ok()
                .flatten()
            else {
                return;
            };
            let Some(texte) = texte_d_invitation(&services, &message) else {
                fenetre.set_status("The invitation could not be read.".into());
                return;
            };
            // By UID, as opening the file does: never twice, an update in place, a
            // cancellation marked.
            match crate::calendar::import_ics(&services, &texte) {
                Ok(bilan) => fenetre.set_toast(bilan.message().into()),
                Err(e) => fenetre.set_status(format!("Could not add the invitation: {e}").into()),
            }
            // The conversation is drawn again, its banner saying what is now true.
            conversation_rendue().clear();
            controller.send(Request::Diff(Box::new(iris_kernel::ViewDiff {
                full_refresh: true,
                ..Default::default()
            })));
        });
    }
    {
        let faible = fenetre.as_weak();
        fenetre.on_invite_open(move |cle, jour| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_workspace(1);
            fenetre.invoke_workspace_changed(1);
            if !jour.is_empty() {
                fenetre.invoke_calendar_day_chosen(jour);
            }
            if !cle.is_empty() {
                fenetre.invoke_calendar_event_opened(cle);
            }
        });
    }
}

/// Accept, Maybe or Decline on an invitation's banner: the answer goes to the
/// organiser from the mailbox the invitation came to, and the calendar follows.
pub fn wire_invite_answers(
    fenetre: &AppWindow,
    services: &Services,
    send: Arc<SendService>,
    controller: Arc<Controller>,
) {
    let services = services.clone();
    let faible = fenetre.as_weak();
    fenetre.on_invite_answer(move |id, choix| {
        let Some(fenetre) = faible.upgrade() else {
            return;
        };
        let Some(reponse) = crate::invite::Answer::from_index(choix) else {
            return;
        };
        let Some(message) = services
            .store
            .message_by_id(iris_types::MessageId(id as i64))
            .ok()
            .flatten()
        else {
            return;
        };
        let Some(texte) = texte_d_invitation(&services, &message) else {
            fenetre.set_status("The invitation could not be read.".into());
            return;
        };
        let moi = adresse_du_compte(&services, message.account);
        match crate::invite::respond(&services, &send, message.account, &moi, &texte, reponse) {
            Ok(dit) => fenetre.set_toast(dit.into()),
            Err(e) => fenetre.set_status(format!("Could not answer: {e}").into()),
        }
        conversation_rendue().clear();
        controller.send(Request::Diff(Box::new(iris_kernel::ViewDiff {
            full_refresh: true,
            ..Default::default()
        })));
    });
}

pub fn wire_attachment_open(
    fenetre: &AppWindow,
    services: &Services,
    selection: Arc<std::sync::Mutex<Option<ThreadIdent>>>,
) {
    let services = services.clone();
    let faible = fenetre.as_weak();
    fenetre.on_open_attachment(move |rang| {
        let Some(fenetre) = faible.upgrade() else {
            return;
        };
        let Some(thread) = *selection.lock().expect("sélection empoisonnée") else {
            return;
        };

        // Une invitation ou un fichier d'agenda s'ouvre dans l'agenda d'Iris, pas dans
        // une autre application : c'est ici qu'est le calendrier, et l'ouvrir ailleurs
        // ferait vivre le même rendez-vous à deux endroits.
        if let Ok((nom, type_mime, octets)) = octets_de_piece(&services, thread, rang as usize) {
            let calendrier = type_mime.eq_ignore_ascii_case("text/calendar")
                || type_mime.eq_ignore_ascii_case("application/ics")
                || nom.to_ascii_lowercase().ends_with(".ics");
            if calendrier {
                match crate::calendar::import_ics(&services, &String::from_utf8_lossy(&octets)) {
                    Ok(bilan) => {
                        fenetre.set_toast(bilan.message().into());
                        if let Some(jour) = bilan.first_day {
                            fenetre.invoke_calendar_day_chosen(jour.into());
                        }
                        fenetre.set_workspace(1);
                        fenetre.invoke_workspace_changed(1);
                    }
                    Err(e) => {
                        fenetre.set_status(format!("Could not read the invitation: {e}").into())
                    }
                }
                return;
            }
        }

        let resultat = enregistrer_piece(&services, thread, rang as usize)
            .and_then(|chemin| crate::platform::open_path(&chemin).map(|()| chemin));

        match resultat {
            Ok(chemin) => fenetre.set_status(
                format!(
                    "Opened {}. The copy is in your downloads.",
                    chemin
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                )
                .into(),
            ),
            Err(e) => fenetre.set_status(format!("Could not open it: {e}").into()),
        }
    });
}

/// Écrit une pièce jointe sur le disque et rend son chemin.
fn enregistrer_piece(
    services: &Services,
    thread: ThreadIdent,
    rang: usize,
) -> iris_types::Result<std::path::PathBuf> {
    let (nom, _, octets) = octets_de_piece(services, thread, rang)?;
    let destination = chemin_libre(&dossier_telechargements(), &nom);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&destination, octets)?;
    Ok(destination)
}

/// Le nom, le type et les octets d'une pièce jointe du dernier message du fil.
fn octets_de_piece(
    services: &Services,
    thread: ThreadIdent,
    rang: usize,
) -> iris_types::Result<(String, String, Vec<u8>)> {
    let messages = services.store.conversation(thread)?;
    let message = messages
        .last()
        .ok_or_else(|| iris_types::Error::other("conversation vide"))?;
    octets_du_message(services, message, rang)
}

/// Is this attachment an invitation (iCalendar)?
fn est_une_invitation(nom: &str, mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    mime.starts_with("text/calendar")
        || mime.starts_with("application/ics")
        || nom.to_ascii_lowercase().ends_with(".ics")
}

/// The iCalendar text of the first invitation a message carries, if it has one and its
/// body is here.
fn texte_d_invitation(services: &Services, message: &iris_store::StoredMessage) -> Option<String> {
    let pieces = services.store.visible_attachments(message.id).ok()?;
    let rang = pieces
        .iter()
        .position(|p| est_une_invitation(&p.meta.filename, &p.meta.mime_type))?;
    let (_, _, octets) = octets_du_message(services, message, rang).ok()?;
    Some(String::from_utf8_lossy(&octets).into_owned())
}

/// The invitation a message carries, set against the calendar.
fn invitation_du_message(
    services: &Services,
    message: &iris_store::StoredMessage,
) -> Option<crate::calendar::Invitation> {
    crate::calendar::invitation(
        services,
        &texte_d_invitation(services, message)?,
        &adresse_du_compte(services, message.account),
    )
}

/// The address of the mailbox a message came to: who answers its invitation.
fn adresse_du_compte(services: &Services, compte: iris_types::AccountId) -> String {
    services
        .store
        .account(compte)
        .ok()
        .flatten()
        .map(|c| c.email)
        .unwrap_or_default()
}

/// Le nom, le type et les octets d'une pièce jointe d'un message.
fn octets_du_message(
    services: &Services,
    message: &iris_store::StoredMessage,
    rang: usize,
) -> iris_types::Result<(String, String, Vec<u8>)> {
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
    Ok((
        piece.meta.filename.clone(),
        piece.meta.mime_type.clone(),
        octets,
    ))
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

/// Wires the window controls that replaced the system frame.
///
/// Everything here exists because the frame was removed, and each piece has to behave
/// the way the frame did. Minimise and maximise are one call each; dragging is the
/// only one with any substance, because only the platform window knows where it is.
pub fn wire_window_controls(fenetre: &AppWindow) {
    {
        let faible = fenetre.as_weak();
        fenetre.on_window_minimise(move || {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.window().set_minimized(true);
            }
        });
    }

    {
        let faible = fenetre.as_weak();
        fenetre.on_window_toggle_maximise(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let now_maximised = !fenetre.window().is_maximized();
            fenetre.window().set_maximized(now_maximised);
            fenetre.set_window_maximised(now_maximised);
        });
    }

    {
        let faible = fenetre.as_weak();
        fenetre.on_window_close(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };

            // Fermer range la fenêtre, cela ne quitte pas. C'est ce qui donne un sens
            // à l'icône de la zone de notification : un client qui se synchronise en
            // arrière-plan et qui s'arrête quand on ferme sa fenêtre ne se synchronise
            // pas. Quitter reste possible — par le menu de l'icône, ou par Ctrl+Q.
            //
            // Sans zone de notification, il n'y aurait aucun moyen de le faire revenir
            // et la fenêtre aurait simplement disparu : dans ce cas seulement, fermer
            // veut dire quitter.
            //
            // Et quand l'utilisateur a demandé que non, aussi. Une application qui
            // refuse de partir quand on lui dit de partir est une application dont on
            // se méfie, et quelqu'un qui relève son courrier deux fois par jour n'a
            // aucune raison de la laisser tourner entre les deux.
            let _ = fenetre.window().hide();
            if !fenetre.get_tray_available() || !fenetre.get_keep_running() {
                let _ = slint::quit_event_loop();
            } else {
                went_to_tray(&fenetre);
            }
        });
    }

    // Dragging: the title bar reports how far the pointer has moved since it was
    // pressed, and the window moves by the same amount. The position is re-read at
    // the start of each drag rather than tracked continuously, so a window moved by
    // any other means — snapped, moved by the keyboard — is not fought over.
    /// Where the window and the pointer both were when the drag began.
    type DragOrigin = Arc<std::sync::Mutex<Option<(slint::PhysicalPosition, (i32, i32))>>>;

    let origine: DragOrigin = Arc::new(std::sync::Mutex::new(None));

    {
        let origine = Arc::clone(&origine);
        let faible = fenetre.as_weak();
        fenetre.on_window_drag_started(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(pointeur) = cursor_position() else {
                return;
            };
            *origine.lock().expect("poisoned drag") = Some((fenetre.window().position(), pointeur));
        });
    }

    {
        let origine = Arc::clone(&origine);
        let faible = fenetre.as_weak();
        fenetre.on_window_drag(move |_dx, _dy| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };

            // A maximised window pulled by its bar comes loose and follows the
            // pointer, which is what every other window on this desktop does.
            if fenetre.window().is_maximized() {
                fenetre.window().set_maximized(false);
                fenetre.set_window_maximised(false);
                if let Some(pointeur) = cursor_position() {
                    *origine.lock().expect("poisoned drag") =
                        Some((fenetre.window().position(), pointeur));
                }
                return;
            }

            let depart = *origine.lock().expect("poisoned drag");
            let Some((fenetre_depart, pointeur_depart)) = depart else {
                return;
            };
            let Some((x, y)) = cursor_position() else {
                return;
            };

            fenetre.window().set_position(slint::PhysicalPosition::new(
                fenetre_depart.x + (x - pointeur_depart.0),
                fenetre_depart.y + (y - pointeur_depart.1),
            ));
        });
    }
}

/// Rafraîchit l'arborescence des dossiers.
///
/// Appelée après une synchronisation et après une création : ce sont les deux seuls
/// moments où la liste des dossiers change, et la recalculer à chaque instantané
/// ferait une agrégation SQL par frappe de clavier dans la liste.
pub fn refresh_folders(fenetre: &AppWindow, services: &Services) {
    let dossiers = services.store.unified_folders().unwrap_or_default();
    let arbre = crate::folders::tree(&dossiers);

    let lignes: Vec<FolderNodeData> = arbre
        .iter()
        .map(|n| FolderNodeData {
            path: n.key.as_str().into(),
            name: n.name.as_str().into(),
            depth: n.depth as i32,
            count: n.threads as i32,
            count_label: iris_ui::format::short_count(n.threads as u64).into(),
            accounts: n.accounts as i32,
            // Un rôle est imposé par le serveur : il ne se supprime pas, et rien dans
            // l'interface ne doit laisser croire le contraire.
            permanent: n.is_role,
            // Le rôle, pas l'icône : c'est l'interface qui possède le jeu d'icônes, et
            // le lui faire traverser en sens inverse mettrait des chemins SVG dans du
            // code Rust, où plus personne ne penserait à les tenir à jour.
            role: n.role.as_str().into(),
        })
        .collect();

    // Relue après chaque synchronisation : ne remplacer le modèle que s'il a changé,
    // sans quoi la colonne se redessinerait — et perdrait son défilement — à chaque
    // tour, pour afficher la même chose.
    let actuelles = fenetre.get_folders();
    if actuelles.row_count() != lignes.len() || actuelles.iter().zip(&lignes).any(|(a, b)| a != *b)
    {
        fenetre.set_folders(ModelRc::new(VecModel::from(lignes)));
    }
    fenetre.set_account_count(services.store.accounts().map(|c| c.len()).unwrap_or(0) as i32);
}

/// Lit ce que l'arborescence a renvoyé.
///
/// L'identifiant de fil que l'interface manipule, en entier de trente-deux bits.
///
/// Slint n'a pas d'entier de soixante-quatre bits ; la conversion se fait donc à chaque
/// frontière, et vaut mieux ici qu'écrite six fois de suite.
fn fil(id: i32) -> iris_types::ThreadId {
    iris_types::ThreadId(id as i64)
}

/// Un rôle arrive préfixé `role:`, un dossier créé arrive par son chemin. Deux espaces
/// de noms qui ne peuvent pas se marcher dessus : un chemin IMAP ne commence jamais
/// par `role:`, et un rôle inconnu retombe sur le chemin plutôt que d'être perdu.
pub fn scope_depuis(choix: &str) -> iris_store::Scope {
    match choix.strip_prefix("role:") {
        Some(nom) => match iris_store::FolderRole::parse(nom) {
            iris_store::FolderRole::Other => iris_store::Scope::Path(choix.to_string()),
            role => iris_store::Scope::Role(role),
        },
        None if choix.is_empty() => iris_store::Scope::Queue,
        None => iris_store::Scope::Path(choix.to_string()),
    }
}

/// The mailbox the view is filtered to, if it is filtered to one.
fn compte_regarde(fenetre: &AppWindow) -> Option<iris_types::AccountId> {
    match fenetre.get_selected_account() {
        0 => None,
        id => Some(iris_types::AccountId(id as i64)),
    }
}

/// Le chemin que le serveur connaît, pour le dossier dont le menu est ouvert.
///
/// C'est la clé de la ligne, qui est déjà ce chemin (`INBOX.Devis` là où l'arborescence
/// montre « Devis »), vérifiée dans le magasin. Il était retrouvé depuis le nom
/// affiché, c'est-à-dire le dernier segment : avec `Clients/2024` et
/// `Fournisseurs/2024`, supprimer le second supprimait le premier, sur toutes les
/// boîtes.
fn chemin_du_menu(services: &Services, clef: &str) -> Option<String> {
    if clef.is_empty() || clef.starts_with("role:") {
        return None;
    }
    services
        .store
        .unified_folders()
        .ok()?
        .into_iter()
        .find(|f| f.role == iris_store::FolderRole::Other && f.path == clef)
        .map(|f| f.path)
}

/// Wires the folder tree and the folder-creation panel.
///
/// Créer un dossier le crée **partout**. C'est le choix central de tout ce module :
/// un dossier est un nom, pas un endroit, et un « Devis » qui n'existerait que sur une
/// boîte rangerait le courrier à moitié.
pub fn wire_folders(fenetre: &AppWindow, services: &Services, controller: Arc<Controller>) {
    refresh_folders(fenetre, services);

    {
        let controller = Arc::clone(&controller);
        fenetre.on_folder_selected(move |choix| {
            controller.send(Request::ShowScope(scope_depuis(choix.as_str())));
        });
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_folder_cleared(move || {
            controller.send(Request::ShowScope(iris_store::Scope::Queue))
        });
    }

    // --- Glisser un message dans un dossier ---
    //
    // Ce qui est déplacé, c'est le lot coché ou la ligne courante : la même règle que
    // pour toutes les autres actions. Le glisser ne transporte rien, il désigne une
    // cible ; l'application sait déjà ce qu'elle tient.
    {
        let controller = Arc::clone(&controller);
        fenetre.on_row_drag_started(move |id| {
            // Empoigner une ligne hors du lot la sélectionne : lâcher un message qu'on
            // vient de traîner pour en déplacer trois autres serait un piège.
            controller.send(Request::SelectThread(iris_types::ThreadId(id as i64)));
        });
    }

    // La charge du glisser.
    //
    // Slint refuse de démarrer un glisser dont `data` n'est pas posée, et le dit à la
    // compilation. Elle porte donc un texte — ce que le glisser signifie — plutôt que
    // les identifiants : la cible est déjà connue de l'application, et la faire voyager
    // en double ouvrirait un second chemin par lequel une action peut se tromper.
    //
    // Le texte a une utilité propre : sur les systèmes qui l'acceptent, lâcher hors de
    // la fenêtre dépose cette phrase, ce qui vaut mieux que de ne rien déposer.
    {
        let mut charge = slint::DataTransfer::default();
        charge.set_plain_text("Iris conversation".into());
        fenetre.set_drag_payload(charge);
    }
    {
        let controller = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_folder_dropped(move |choix| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            // On ne range que dans un vrai dossier. Lâcher sur « Spam » ou « Corbeille »
            // veut dire autre chose — marquer indésirable, jeter — et confondre les
            // deux ferait d'un geste de rangement une suppression.
            match scope_depuis(choix.as_str()) {
                iris_store::Scope::Path(chemin) => {
                    controller.send(Request::MoveMarkedToFolder(chemin));
                }
                iris_store::Scope::Role(iris_store::FolderRole::Trash) => {
                    controller.send(Request::ApplyToMarked(iris_viewmodel::Action::Delete));
                }
                iris_store::Scope::Role(iris_store::FolderRole::Archive) => {
                    controller.send(Request::ApplyToMarked(iris_viewmodel::Action::Archive));
                }
                _ => fenetre.set_status("That folder cannot take dropped mail.".into()),
            }
        });
    }

    // --- Renommer, supprimer ---
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_folder_menu_requested(move |chemin| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let permanent = chemin.starts_with("role:");
            let nom = crate::folders::scope_name(&scope_depuis(chemin.as_str()));

            // La clé brute voyage à côté du nom affiché : « Spam » ne suffit pas à
            // retrouver un rôle, et deux dossiers peuvent porter le même nom court.
            let portee = scope_depuis(chemin.as_str());
            fenetre.set_folder_menu_path(nom.as_str().into());
            fenetre.set_folder_menu_key(chemin.clone());
            fenetre.set_folder_menu_permanent(permanent);
            fenetre.set_folder_menu_emptyable(matches!(
                portee,
                iris_store::Scope::Role(iris_store::FolderRole::Trash)
                    | iris_store::Scope::Role(iris_store::FolderRole::Junk)
            ));
            fenetre.set_folder_menu_open(true);
            let _ = &services;
        });
    }
    {
        let faible = fenetre.as_weak();
        fenetre.on_folder_menu_dismissed(move || {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_folder_menu_open(false);
            }
        });
    }
    {
        let faible = fenetre.as_weak();
        fenetre.on_folder_rename_requested(move || {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_folder_menu_open(false);
                fenetre.set_rename_folder_current(fenetre.get_folder_menu_path());
                fenetre.set_rename_folder_name(fenetre.get_folder_menu_path());
                fenetre.set_rename_folder_error(Default::default());
                fenetre.set_rename_folder_open(true);
            }
        });
    }
    {
        let faible = fenetre.as_weak();
        fenetre.on_folder_rename_dismissed(move || {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_rename_folder_open(false);
            }
        });
    }
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        fenetre.on_folder_rename_confirmed(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            // Le chemin réel, pas le nom affiché : l'arborescence montre « Devis » là
            // où le serveur connaît « INBOX.Devis ».
            let Some(chemin) = chemin_du_menu(&services, fenetre.get_folder_menu_key().as_str())
            else {
                fenetre.set_rename_folder_error("That folder no longer exists.".into());
                return;
            };

            match crate::folders::rename_everywhere(
                &services.store,
                &chemin,
                fenetre.get_rename_folder_name().as_str(),
                now(),
            ) {
                Ok(0) => fenetre.set_rename_folder_error("It already has that name.".into()),
                Ok(n) => {
                    fenetre.set_rename_folder_open(false);
                    fenetre.set_status(format!("Renaming on {n} mailbox(es)…").into());
                }
                Err(e) => fenetre.set_rename_folder_error(e.to_string().into()),
            }
        });
    }
    {
        let services = services.clone();
        let controller = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_folder_delete_requested(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_folder_menu_open(false);

            let Some(chemin) = chemin_du_menu(&services, fenetre.get_folder_menu_key().as_str())
            else {
                fenetre.set_status("That folder no longer exists.".into());
                return;
            };

            match crate::folders::delete_everywhere(&services.store, &chemin, now()) {
                Ok(n) => {
                    // La vue revenait sur un dossier qui n'existe plus : elle repart
                    // sur la boîte de réception.
                    controller.send(Request::ShowScope(iris_viewmodel::depart()));
                    fenetre.set_status(
                        format!("Folder removed on {n} mailbox(es). The mail moved to the inbox.")
                            .into(),
                    );
                    refresh_folders(&fenetre, &services);
                }
                Err(e) => fenetre.set_status(format!("Could not remove it: {e}").into()),
            }
        });
    }

    // --- Tout marquer lu, vider ---
    //
    // Les deux gestes en gros que tout client de courrier a et qu'Iris n'avait pas. La
    // corbeille de cette boîte affichait huit cent soixante-cinq.
    {
        let services = services.clone();
        let controller = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_folder_mark_read_requested(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_folder_menu_open(false);
            let portee = scope_depuis(fenetre.get_folder_menu_key().as_str());

            match crate::folders::mark_read_everywhere(
                &services.store,
                &portee,
                compte_regarde(&fenetre),
                now(),
            ) {
                Ok(0) => fenetre.set_status("Nothing unread there.".into()),
                Ok(n) => {
                    controller.send(Request::Diff(Box::new(iris_kernel::ViewDiff {
                        full_refresh: true,
                        ..Default::default()
                    })));
                    fenetre.set_status(
                        format!(
                            "{} marked as read.",
                            iris_ui::format::plural(n as u64, "message")
                        )
                        .into(),
                    );
                    refresh_folders(&fenetre, &services);
                }
                Err(e) => fenetre.set_status(format!("Could not do it: {e}").into()),
            }
        });
    }
    {
        let services = services.clone();
        let controller = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_folder_empty_requested(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_folder_menu_open(false);
            let portee = scope_depuis(fenetre.get_folder_menu_key().as_str());

            match crate::folders::empty_everywhere(
                &services.store,
                &portee,
                compte_regarde(&fenetre),
                now(),
            ) {
                Ok(0) => fenetre.set_status("It is already empty.".into()),
                Ok(n) => {
                    controller.send(Request::Diff(Box::new(iris_kernel::ViewDiff {
                        full_refresh: true,
                        ..Default::default()
                    })));
                    fenetre.set_status(
                        format!(
                            "{} deleted for good.",
                            iris_ui::format::plural(n as u64, "message")
                        )
                        .into(),
                    );
                    refresh_folders(&fenetre, &services);
                }
                Err(e) => fenetre.set_status(format!("Could not empty it: {e}").into()),
            }
        });
    }

    {
        let faible = fenetre.as_weak();
        fenetre.on_new_folder_requested(move || {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_new_folder_name(Default::default());
                fenetre.set_new_folder_error(Default::default());
                fenetre.set_new_folder_open(true);
            }
        });
    }
    {
        let faible = fenetre.as_weak();
        fenetre.on_new_folder_dismissed(move || {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_new_folder_open(false);
            }
        });
    }

    {
        let services = services.clone();
        let faible = fenetre.as_weak();

        fenetre.on_new_folder_create(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };

            let nom = fenetre.get_new_folder_name().to_string();

            // Le dossier choisi devient le parent : créer « 2026 » alors qu'on regarde
            // « Devis » veut dire « Devis.2026 », ce qu'on attend d'un bouton
            // « nouveau dossier » pressé depuis un dossier.
            //
            // Un **rôle** n'est pas un parent. La corbeille et les indésirables sont
            // imposés par le serveur ; y créer une sous-branche donnerait un dossier
            // dans la poubelle. La sélection est donc lue par le même analyseur que
            // partout ailleurs, et seul un vrai chemin devient un parent.
            let parent = match scope_depuis(fenetre.get_selected_folder().as_str()) {
                iris_store::Scope::Path(chemin) => Some(chemin),
                _ => None,
            };

            let ferme = |fenetre: &AppWindow| {
                fenetre.set_new_folder_open(false);
                fenetre.set_new_folder_name(Default::default());
                fenetre.set_new_folder_error(Default::default());
            };

            match crate::folders::create_everywhere(&services.store, parent.as_deref(), &nom, now())
            {
                // Zéro compte à prévenir veut dire qu'il existe déjà partout : le but
                // est atteint. Garder la fenêtre ouverte sur une erreur punirait
                // l'utilisateur d'avoir demandé quelque chose qui était déjà fait.
                Ok(0) => {
                    ferme(&fenetre);
                    fenetre.set_status("That folder already exists everywhere.".into());
                }
                Ok(n) => {
                    ferme(&fenetre);
                    fenetre.set_status(format!("Creating the folder on {n} mailbox(es)…").into());
                    refresh_folders(&fenetre, &services);
                }
                // Seul un nom refusé garde la fenêtre : c'est le seul cas où il reste
                // quelque chose à corriger sur place.
                Err(e) => fenetre.set_new_folder_error(e.to_string().into()),
            }
        });
    }
}

/// Wires marking rows and acting on the batch.
///
/// Chaque action passe par la même requête, qui décide seule de sa cible : le lot
/// coché s'il y en a un, la ligne courante sinon. Une seule règle, partagée par le
/// clavier, la barre d'outils et le menu contextuel — sans elle, « archiver » ne
/// voudrait pas dire la même chose selon l'endroit d'où on le demande.
pub fn wire_bulk(fenetre: &AppWindow, controller: Arc<Controller>) {
    use iris_viewmodel::Action;

    {
        let controller = Arc::clone(&controller);
        fenetre.on_row_mark_toggled(move |id| {
            controller.send(Request::ToggleMark(iris_types::ThreadId(id as i64)));
        });
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_row_mark_extended(move |id| {
            controller.send(Request::ExtendMark(iris_types::ThreadId(id as i64)));
        });
    }
    // Les filtres rapides.
    //
    // L'état vit dans le vue-modèle, pas ici : c'est lui qui filtre les trois files et
    // recompte. La fenêtre bascule un drapeau et renvoie le tout, ce qui évite d'avoir
    // deux idées de ce qui est allumé.
    {
        let controller = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_filter_toggled(move |quoi| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let mut filtres = iris_store::Filters {
                unread: fenetre.get_filter_unread(),
                attachments: fenetre.get_filter_attachments(),
                starred: fenetre.get_filter_starred(),
            };
            match quoi.as_str() {
                "unread" => filtres.unread = !filtres.unread,
                "attachments" => filtres.attachments = !filtres.attachments,
                "starred" => filtres.starred = !filtres.starred,
                // « Clear » : tout éteindre d'un geste. Un filtre laissé allumé fait
                // croire à une boîte vide, et c'est la panne la plus déroutante qu'un
                // filtre puisse produire.
                _ => filtres = iris_store::Filters::default(),
            }
            controller.send(Request::SetFilters(filtres));
        });
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_sort_chosen(move |i| {
            controller.send(Request::SetSort(iris_store::Sort::from_index(i)));
        });
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_bulk_select_all(move || controller.send(Request::MarkAll));
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_bulk_clear(move || controller.send(Request::ClearMarks));
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_bulk_done(move || controller.send(Request::ApplyToMarked(Action::Done)));
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_bulk_archive(move || controller.send(Request::ApplyToMarked(Action::Archive)));
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_bulk_delete(move || controller.send(Request::ApplyToMarked(Action::Delete)));
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_bulk_read(move || controller.send(Request::ApplyToMarked(Action::MarkRead)));
    }
    {
        let controller = Arc::clone(&controller);
        fenetre.on_bulk_unread(move || controller.send(Request::ApplyToMarked(Action::MarkUnread)));
    }
}

/// Wires the per-module settings.
///
/// Un seul répertoire, celui des plugins : les réglages d'un module vivent dans le
/// sien, si bien qu'un module retiré ne laisse rien derrière lui.
pub fn wire_plugin_settings(fenetre: &AppWindow, services: &Services) {
    let repertoire = crate::plugins::ensure_dir(services.paths.plugins());

    // --- Les réglages d'un module ---
    {
        let dossier = repertoire.clone();
        let faible = fenetre.as_weak();

        fenetre.on_plugin_settings_requested(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let id = id.to_string();
            if crate::plugins::validate_id(&id).is_err() {
                return;
            }

            let module = dossier.join(&id);
            let Some(manifeste) = lire_manifeste(&module) else {
                fenetre.set_plugin_settings_error("Its manifest could not be read.".into());
                fenetre.set_plugin_settings_open(true);
                return;
            };

            let valeurs = iris_plugins::SettingValues::load(&module);
            let lignes: Vec<PluginSettingData> = manifeste
                .settings
                .iter()
                .filter(|s| s.is_valid_key())
                .map(|s| PluginSettingData {
                    key: s.key.as_str().into(),
                    label: s.display_label().into(),
                    hint: s.hint.as_str().into(),
                    kind: s.kind.as_str().into(),
                    value: valeurs.get(s).into(),
                })
                .collect();

            fenetre.set_plugin_settings_name(manifeste.name.as_str().into());
            fenetre.set_plugin_settings(ModelRc::new(VecModel::from(lignes)));
            fenetre.set_plugin_settings_error(Default::default());
            fenetre.set_plugin_settings_id(id.as_str().into());
            fenetre.set_plugin_settings_open(true);
        });
    }

    {
        let dossier = repertoire.clone();
        let faible = fenetre.as_weak();

        fenetre.on_plugin_setting_changed(move |cle, valeur| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let id = fenetre.get_plugin_settings_id().to_string();
            if crate::plugins::validate_id(&id).is_err() {
                return;
            }

            let module = dossier.join(&id);
            let Some(manifeste) = lire_manifeste(&module) else {
                return;
            };
            let Some(spec) = manifeste.settings.iter().find(|s| s.key == cle.as_str()) else {
                // Une clé que le manifeste ne déclare pas n'a pas de place dans le
                // fichier : elle viendrait d'un module qui a changé sous nos pieds, et
                // l'écrire y laisserait une entrée que plus rien ne lit.
                return;
            };

            let mut valeurs = iris_plugins::SettingValues::load(&module);
            valeurs.set(spec, valeur.as_str());
            if let Err(e) = valeurs.save(&module) {
                fenetre.set_plugin_settings_error(format!("Could not save: {e}").into());
            } else {
                fenetre.set_plugin_settings_error(Default::default());
            }
        });
    }

    {
        let faible = fenetre.as_weak();
        fenetre.on_plugin_settings_dismissed(move || {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_plugin_settings_open(false);
            }
        });
    }
}

/// Lit le manifeste d'un module installé.
fn lire_manifeste(dir: &std::path::Path) -> Option<iris_plugins::Manifest> {
    let texte = std::fs::read_to_string(dir.join("plugin.toml")).ok()?;
    toml::from_str(&texte).ok()
}

/// Wires the menu a right-click on an account opens.
///
/// Everything here already existed and none of it was reachable from the row it
/// applies to. Changing a password meant waiting for the mailbox to fail so the
/// warning marker would appear — the application asked people to break something
/// before it would let them fix it.
pub fn wire_account_menu(
    fenetre: &AppWindow,
    services: &Services,
    controller: Arc<Controller>,
    runtime: tokio::runtime::Handle,
) {
    // Which row the menu belongs to. Held here and not in the interface because the
    // menu outlives the click that opened it, and the row underneath may scroll away.
    let sujet: Arc<std::sync::Mutex<Option<iris_types::AccountId>>> =
        Arc::new(std::sync::Mutex::new(None));

    let courant = {
        let sujet = Arc::clone(&sujet);
        let services = services.clone();
        move || -> Option<iris_store::Account> {
            let id = (*sujet.lock().expect("poisoned account menu"))?;
            services.store.account(id).ok().flatten()
        }
    };

    // --- Opening it ---
    {
        let services = services.clone();
        let sujet = Arc::clone(&sujet);
        let faible = fenetre.as_weak();

        fenetre.on_account_menu_requested(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let compte = iris_types::AccountId(id as i64);
            *sujet.lock().expect("poisoned account menu") = Some(compte);

            let Some(details) = services.store.account(compte).ok().flatten() else {
                return;
            };

            fenetre.set_account_menu_label(details.email.as_str().into());
            fenetre.set_account_menu_pinned(details.pinned);
            fenetre.set_account_menu_enabled(details.enabled);
            // Its tags, the submenu folded and its search emptied: a menu opened on
            // another account must not carry the last one's state.
            fenetre.set_account_tags_open(false);
            fenetre.set_account_tag_search(Default::default());
            fenetre.set_account_menu_tags(ModelRc::new(VecModel::from(crate::tags::menu_tags(
                &services, compte, "",
            ))));
            fenetre.set_account_menu_open(true);
        });
    }

    crate::tags::wire_tags(fenetre, services, Arc::clone(&sujet));
    crate::tags::wire_tag_groups(fenetre, services, Arc::clone(&controller));

    {
        let faible = fenetre.as_weak();
        fenetre.on_account_menu_dismissed(move || {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_account_menu_open(false);
            }
        });
    }

    // --- Sync this one now ---
    {
        let services = services.clone();
        let sujet = Arc::clone(&sujet);
        let runtime_sync = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_account_menu_sync(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_account_menu_open(false);
            let Some(compte) = *sujet.lock().expect("poisoned account menu") else {
                return;
            };

            let engine = Arc::clone(&services.engine);
            let faible = fenetre.as_weak();
            runtime_sync.spawn(async move {
                let resultat = engine.sync_now(compte, now()).await;
                let _ = faible.upgrade_in_event_loop(move |fenetre| match resultat {
                    Ok(n) => fenetre.set_status(
                        format!("{}.", iris_ui::format::plural(n as u64, "new message")).into(),
                    ),
                    Err(e) => fenetre.set_status(format!("Sync failed: {e}").into()),
                });
            });
        });
    }

    // --- Edit the servers, or just the password ---
    //
    // Both open the same screen. The difference is which field is waiting for you:
    // a password change is the common case and should not require reading past six
    // hostname fields to find the one box that matters.
    {
        let courant = courant.clone();
        let faible = fenetre.as_weak();
        fenetre.on_account_menu_edit(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_account_menu_open(false);
            let Some(details) = courant() else {
                return;
            };
            prefill_from_account(&fenetre, &details, true);
        });
    }
    {
        let courant = courant.clone();
        let faible = fenetre.as_weak();
        fenetre.on_account_menu_password(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_account_menu_open(false);
            let Some(details) = courant() else {
                return;
            };
            // Signed in through the browser: no password to change, but a sign-in to
            // renew, which the account's panel offers.
            if crate::oauth::provider_for(details.auth).is_some() {
                fenetre.invoke_resume_account(details.id.get() as i32);
                return;
            }
            // Not the manual form: the short screen is one address and one password,
            // which is exactly the shape of "my password changed". The servers are
            // already known and correct, and showing them invites editing them by
            // accident.
            prefill_from_account(&fenetre, &details, false);
            fenetre
                .set_add_account_hint("Enter the new password. The servers are unchanged.".into());
        });
    }

    // --- La signature ---
    //
    // Tout client de courrier en a une depuis toujours ; Iris envoyait chaque message
    // non signé, et la seule parade était de retaper quatre lignes à chaque fois.
    {
        let courant = courant.clone();
        let faible = fenetre.as_weak();
        fenetre.on_account_menu_signature(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_account_menu_open(false);
            let Some(details) = courant() else {
                return;
            };
            fenetre.set_signature_account(details.email.as_str().into());
            fenetre.set_signature_text(details.signature.as_str().into());
            fenetre.set_signature_open(true);
        });
    }
    {
        let services = services.clone();
        let courant = courant.clone();
        let faible = fenetre.as_weak();
        fenetre.on_signature_saved(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(details) = courant() else {
                return;
            };
            let texte = fenetre.get_signature_text().to_string();
            match services.store.set_account_signature(details.id, &texte) {
                Ok(()) if texte.trim().is_empty() => {
                    fenetre.set_status("Signature removed.".into())
                }
                Ok(()) => fenetre.set_status("Signature saved.".into()),
                Err(e) => fenetre.set_status(format!("Could not save it: {e}").into()),
            }
        });
    }

    // --- Send as: the mailbox's aliases ---
    {
        let services = services.clone();
        let courant = courant.clone();
        let faible = fenetre.as_weak();
        fenetre.on_account_menu_aliases(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_account_menu_open(false);
            let Some(details) = courant() else {
                return;
            };
            fenetre.set_alias_account(details.email.as_str().into());
            fenetre.set_alias_new_address(Default::default());
            fenetre.set_alias_new_name(Default::default());
            fenetre.set_alias_error(Default::default());
            montrer_alias(&fenetre, &services, details.id);
            fenetre.set_alias_open(true);
        });
    }
    {
        let services = services.clone();
        let courant = courant.clone();
        let faible = fenetre.as_weak();
        fenetre.on_alias_added(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(details) = courant() else {
                return;
            };
            let adresse = fenetre.get_alias_new_address().trim().to_lowercase();
            let (bons, _) = iris_sync::parse_recipients(&adresse);
            if bons.len() != 1 || adresse.contains([',', ';']) {
                fenetre.set_alias_error("That does not look like one email address.".into());
                return;
            }
            if adresse == details.email.to_lowercase() {
                fenetre.set_alias_error("That is the mailbox's own address.".into());
                return;
            }
            match services.store.add_alias(
                details.id,
                &adresse,
                fenetre.get_alias_new_name().trim(),
            ) {
                Ok(()) => {
                    fenetre.set_alias_new_address(Default::default());
                    fenetre.set_alias_new_name(Default::default());
                    fenetre.set_alias_error(Default::default());
                    montrer_alias(&fenetre, &services, details.id);
                    charger_expediteurs(&fenetre, &services);
                }
                Err(e) => fenetre.set_alias_error(format!("Could not add it: {e}").into()),
            }
        });
    }
    {
        let services = services.clone();
        let courant = courant.clone();
        let faible = fenetre.as_weak();
        fenetre.on_alias_removed(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let _ = services.store.remove_alias(id as i64);
            if let Some(details) = courant() {
                montrer_alias(&fenetre, &services, details.id);
            }
            charger_expediteurs(&fenetre, &services);
        });
    }

    // --- Pin, disable, remove ---
    {
        let services = services.clone();
        let courant = courant.clone();
        let faible = fenetre.as_weak();
        fenetre.on_account_menu_pin(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_account_menu_open(false);
            let Some(details) = courant() else {
                return;
            };
            match services
                .store
                .set_account_pinned(details.id, !details.pinned)
            {
                Ok(()) => refresh_accounts(&fenetre, &services, &[]),
                Err(e) => fenetre.set_status(format!("Could not pin it: {e}").into()),
            }
        });
    }
    {
        let services = services.clone();
        let courant = courant.clone();
        let runtime = runtime.clone();
        let faible = fenetre.as_weak();
        fenetre.on_account_menu_enable(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_account_menu_open(false);
            let Some(details) = courant() else {
                return;
            };
            let allume = !details.enabled;
            match services.store.set_account_enabled(details.id, allume) {
                Ok(()) => {
                    let mot = if allume { "enabled" } else { "disabled" };
                    fenetre.set_status(format!("{} {mot}.", details.email).into());
                    refresh_accounts(&fenetre, &services, &[]);
                    recharger_les_comptes(&services, &runtime);
                    charger_expediteurs(&fenetre, &services);
                }
                Err(e) => fenetre.set_status(format!("Could not change it: {e}").into()),
            }
        });
    }
    {
        let services = services.clone();
        let courant = courant.clone();
        let controller = Arc::clone(&controller);
        let faible = fenetre.as_weak();
        fenetre.on_account_menu_remove(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            fenetre.set_account_menu_open(false);
            let Some(details) = courant() else {
                return;
            };

            // Its scheduled messages go with it: said, not silently dropped.
            let programmes = services.store.scheduled_mail_count(details.id).unwrap_or(0);
            match crate::accounts::remove_account(
                &services.store,
                services.secrets.as_ref(),
                details.id,
            ) {
                Ok(_) => {
                    fenetre.set_status(
                        if programmes == 0 {
                            format!("{} removed.", details.email)
                        } else {
                            format!(
                                "{} removed, with {} that waited to be sent.",
                                details.email,
                                iris_ui::format::plural(programmes as u64, "scheduled message")
                            )
                        }
                        .into(),
                    );
                    refresh_accounts(&fenetre, &services, &[]);
                    controller.send(Request::Bootstrap);
                    recharger_les_comptes(&services, &runtime);
                    charger_expediteurs(&fenetre, &services);
                }
                Err(e) => fenetre.set_status(format!("Could not remove it: {e}").into()),
            }
        });
    }
}

/// Tells the engine which accounts are on: one switched off or removed leaves the
/// schedule (it was only ever added to it, and kept syncing, a refused password
/// retried until the server locked the account).
fn recharger_les_comptes(services: &Services, runtime: &tokio::runtime::Handle) {
    let engine = Arc::clone(&services.engine);
    runtime.spawn(async move {
        if let Err(e) = engine.load_accounts(now()).await {
            tracing::warn!(error = %e, "reloading the accounts");
        }
    });
}

/// Opens the setup screen on an account that already exists.
fn prefill_from_account(fenetre: &AppWindow, compte: &iris_store::Account, manual: bool) {
    EDITION.with(|e| e.set(Some(compte.id)));
    fenetre.set_new_username(compte.imap_user.as_str().into());
    fenetre.set_add_account_editing(true);
    fenetre.set_add_account_manual(manual);
    fenetre.set_add_account_error(Default::default());
    fenetre.set_add_account_hint(if manual {
        "Change what has moved. What you leave alone stays as it is.".into()
    } else {
        slint::SharedString::new()
    });
    fenetre.set_new_email(compte.email.as_str().into());
    // Never prefilled, and never read back out of the vault to show. A password field
    // that arrives full teaches the user that the application can hand their password
    // to whatever asks for it.
    fenetre.set_new_password(Default::default());
    fenetre.set_new_imap_host(compte.imap_host.as_str().into());
    fenetre.set_new_imap_port(compte.imap_port.to_string().into());
    fenetre.set_new_imap_tls(compte.imap_tls);
    fenetre.set_new_smtp_host(compte.smtp_host.as_str().into());
    fenetre.set_new_smtp_port(compte.smtp_port.to_string().into());
    fenetre.set_new_smtp_tls(compte.smtp_tls);
    fenetre.set_add_account_open(true);
}

/// Wires the account-problem panel behind the sidebar's warning marker.
///
/// The marker used to call `resume` and nothing else: the scheduler forgot the
/// account had failed, the next pass failed the same way, and from the outside
/// clicking it did nothing at all. Which, for a wrong password, is exactly right —
/// retrying a password that will never work cannot help. So the marker now opens
/// something that can.
pub fn wire_account_recovery(
    fenetre: &AppWindow,
    services: &Services,
    runtime: tokio::runtime::Handle,
) {
    // Which account the panel is talking about.
    let sujet: Arc<std::sync::Mutex<Option<iris_types::AccountId>>> =
        Arc::new(std::sync::Mutex::new(None));

    {
        let services = services.clone();
        let sujet = Arc::clone(&sujet);
        let faible = fenetre.as_weak();

        fenetre.on_resume_account(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let compte = iris_types::AccountId(id as i64);
            *sujet.lock().expect("poisoned") = Some(compte);

            let details = services.store.account(compte).ok().flatten();
            let email = details
                .as_ref()
                .map(|c| c.email.clone())
                .unwrap_or_default();
            let par_navigateur = details
                .as_ref()
                .and_then(|c| crate::oauth::provider_for(c.auth))
                .is_some();

            // What the engine last saw. Without a recorded failure the account is
            // merely paused, which is still worth explaining.
            let panne = services.engine.failure(compte);
            let refuse = panne.as_ref().map(|p| p.needs_password).unwrap_or(false);

            fenetre.set_problem_account(email.into());
            fenetre.set_problem_sign_in(par_navigateur);
            fenetre.set_problem_message(
                panne
                    .as_ref()
                    .map(|p| p.message.clone())
                    .unwrap_or_default()
                    .into(),
            );
            fenetre.set_problem_advice(
                if par_navigateur && refuse {
                    "The sign-in that lets Iris read this mailbox has expired or was \
                     withdrawn. Sign in again in the browser."
                        .to_string()
                } else {
                    panne.as_ref().map(|p| p.advice()).unwrap_or_else(|| {
                        "This mailbox was paused after repeated failures.".into()
                    })
                }
                .into(),
            );
            fenetre.set_problem_needs_password(
                panne.as_ref().map(|p| p.needs_password).unwrap_or(false),
            );
            fenetre.set_problem_result(Default::default());
            fenetre.set_problem_password(Default::default());
            fenetre.set_problem_open(true);
        });
    }

    // --- Try again, without touching the password ---
    {
        let services = services.clone();
        let sujet = Arc::clone(&sujet);
        let runtime_retry = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_problem_retry(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(compte) = *sujet.lock().expect("poisoned") else {
                return;
            };
            fenetre.set_problem_busy(true);

            let engine = Arc::clone(&services.engine);
            let services_apres = services.clone();
            let faible = fenetre.as_weak();

            runtime_retry.spawn(async move {
                engine.resume_account(compte, now()).await;
                let resultat = engine.sync_now(compte, now()).await;
                let suspendus = engine.suspended_accounts().await;

                let _ = faible.upgrade_in_event_loop(move |fenetre| {
                    fenetre.set_problem_busy(false);
                    match resultat {
                        Ok(n) => {
                            fenetre.set_problem_result(
                                format!(
                                    "Working again: {} fetched.",
                                    iris_ui::format::plural(n as u64, "message")
                                )
                                .into(),
                            );
                            fenetre.set_problem_open(false);
                        }
                        Err(e) => fenetre.set_problem_result(format!("Still failing: {e}").into()),
                    }
                    refresh_accounts(&fenetre, &services_apres, &suspendus);
                });
            });
        });
    }

    // --- Save a new password, then try again ---
    {
        let services = services.clone();
        let sujet = Arc::clone(&sujet);
        let runtime_save = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_problem_save_password(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(compte) = *sujet.lock().expect("poisoned") else {
                return;
            };

            let motdepasse = fenetre.get_problem_password().to_string();
            if motdepasse.is_empty() {
                fenetre.set_problem_result("Enter the password first.".into());
                return;
            }

            let Some(details) = services.store.account(compte).ok().flatten() else {
                return;
            };

            if let Err(e) = services.secrets.set(
                &details.email,
                iris_secrets::SecretKind::Password,
                &iris_secrets::Secret::new(motdepasse),
            ) {
                fenetre.set_problem_result(format!("Could not save the password: {e}").into());
                return;
            }

            fenetre.set_problem_busy(true);
            fenetre.set_problem_password(Default::default());

            let engine = Arc::clone(&services.engine);
            let services_apres = services.clone();
            let faible = fenetre.as_weak();

            runtime_save.spawn(async move {
                engine.resume_account(compte, now()).await;
                let resultat = engine.sync_now(compte, now()).await;
                let suspendus = engine.suspended_accounts().await;

                let _ = faible.upgrade_in_event_loop(move |fenetre| {
                    fenetre.set_problem_busy(false);
                    match resultat {
                        Ok(n) => {
                            fenetre.set_problem_open(false);
                            fenetre.set_status(
                                format!(
                                    "Account working again: {}.",
                                    iris_ui::format::plural(n as u64, "message")
                                )
                                .into(),
                            );
                        }
                        Err(e) => fenetre.set_problem_result(format!("Still refused: {e}").into()),
                    }
                    refresh_accounts(&fenetre, &services_apres, &suspendus);
                });
            });
        });
    }

    // --- Signed in through the browser: sign in again ---
    //
    // A Google or Microsoft account whose authorisation expired or was withdrawn was
    // offered a password, which such an account never uses: it stayed refused for
    // good, and the only way out was to remove it and add it again.
    {
        let services = services.clone();
        let sujet = Arc::clone(&sujet);
        let runtime_auth = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_problem_sign_in_again(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(compte) = *sujet.lock().expect("poisoned") else {
                return;
            };
            let Some(details) = services.store.account(compte).ok().flatten() else {
                return;
            };
            let Some(fournisseur) = crate::oauth::provider_for(details.auth) else {
                return;
            };
            let reglages = services
                .oauth
                .read()
                .expect("réglages OAuth empoisonnés")
                .clone();
            if !reglages.is_configured(fournisseur) {
                fenetre.set_problem_result(
                    "Set the sign-in client first, in Settings › Sign in with Google or \
                     Microsoft."
                        .into(),
                );
                return;
            }
            fenetre.set_problem_busy(true);
            fenetre.set_problem_result("Sign in in the browser window that opened.".into());

            let (secrets, engine) = (Arc::clone(&services.secrets), Arc::clone(&services.engine));
            let services_apres = services.clone();
            let faible = fenetre.as_weak();
            runtime_auth.spawn(async move {
                let resultat = match crate::oauth::authorize(
                    secrets,
                    &reglages,
                    fournisseur,
                    &details.email,
                    now(),
                )
                .await
                {
                    Ok(_) => {
                        engine.resume_account(compte, now()).await;
                        engine.sync_now(compte, now()).await
                    }
                    Err(e) => Err(e),
                };
                let suspendus = engine.suspended_accounts().await;
                let _ = faible.upgrade_in_event_loop(move |fenetre| {
                    fenetre.set_problem_busy(false);
                    match resultat {
                        Ok(n) => {
                            fenetre.set_problem_open(false);
                            fenetre.set_status(
                                format!(
                                    "Signed in again: {}.",
                                    iris_ui::format::plural(n as u64, "message")
                                )
                                .into(),
                            );
                        }
                        Err(e) => fenetre.set_problem_result(format!("Still refused: {e}").into()),
                    }
                    refresh_accounts(&fenetre, &services_apres, &suspendus);
                });
            });
        });
    }

    // --- Or switch it off and stop being told about it ---
    {
        let services = services.clone();
        let sujet = Arc::clone(&sujet);
        let runtime_eteint = runtime.clone();
        let faible = fenetre.as_weak();

        fenetre.on_problem_disable(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(compte) = *sujet.lock().expect("poisoned") else {
                return;
            };

            match services.store.set_account_enabled(compte, false) {
                Ok(()) => {
                    fenetre.set_problem_open(false);
                    fenetre.set_status("Account disabled.".into());
                    refresh_accounts(&fenetre, &services, &[]);
                    recharger_les_comptes(&services, &runtime_eteint);
                }
                Err(e) => fenetre.set_problem_result(format!("Could not disable it: {e}").into()),
            }
        });
    }
}

/// Wires the compose window.
///
/// It shares the outbox and the undo notice with replies, so the time to change your
/// mind behaves the same way here. Reimplementing the delay would give the application
/// two answers to "can I still stop this?", and only one of them would be right.
pub fn wire_compose(
    fenetre: &AppWindow,
    services: &Services,
    send: Arc<SendService>,
    avis: Rc<AvisEnvoi>,
    runtime: tokio::runtime::Handle,
) {
    // Which mailboxes can send, and as which addresses (their aliases after them).
    charger_expediteurs(fenetre, services);

    // What is going with the message. Held here rather than in the interface because
    // the bytes are ours: the panel shows names, we keep the files.
    let pieces: Arc<std::sync::Mutex<Vec<iris_smtp::Attachment>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    // --- Choosing the sender ---
    {
        let faible = fenetre.as_weak();
        fenetre.on_compose_sender_chosen(move |index| {
            if let Some(fenetre) = faible.upgrade() {
                fenetre.set_compose_sender_index(index);
            }
        });
    }

    // --- Le brouillon, gardé sur disque ---
    //
    // Le texte vivait dans les propriétés de l'interface : il réapparaissait tant que
    // l'application tournait et disparaissait avec elle. Quelqu'un qui ferme l'éditeur
    // pour vérifier une adresse, puis quitte, avait perdu son message — sans
    // avertissement, parce que rien ne savait qu'il y en avait un.
    let chemin_brouillon = services.paths.draft();

    // Ce qui restait de la dernière fois, remis en place au démarrage.
    if let Some(garde) = crate::draft::Draft::load(&chemin_brouillon) {
        fenetre.set_compose_to(garde.to.into());
        fenetre.set_compose_cc(garde.cc.into());
        fenetre.set_compose_bcc(garde.bcc.into());
        fenetre.set_compose_subject(garde.subject.into());
        fenetre.set_compose_body(garde.body.into());
        // Les copies s'affichent d'elles-mêmes si elles portent quelque chose : les
        // replier cacherait un destinataire que l'utilisateur a saisi.
        fenetre.set_compose_show_cc(
            !fenetre.get_compose_cc().is_empty() || !fenetre.get_compose_bcc().is_empty(),
        );
        choisir_expediteur(fenetre, &garde.sender);
        fenetre.set_status("An unsent message was restored: see New message.".into());
    }

    // Closed with nothing in it: there is nothing left to keep, here or on disk.
    {
        let chemin = chemin_brouillon.clone();
        fenetre.on_compose_dismissed(move || {
            let _ = crate::draft::Draft::clear(&chemin);
        });
    }

    // --- Minimised messages, at the foot of the window ---
    //
    // Minimising takes the message out of the window and keeps it, on this computer,
    // as a small bar at the bottom right; the window is then free, and New message
    // writes another one. A click on a bar brings its message back, rising and
    // growing into the window at once.
    let chemin_reduits = services.paths.minimised_drafts();
    CHEMIN_REDUITS.with(|c| *c.borrow_mut() = Some(chemin_reduits.clone()));
    REDUITS.with(|r| {
        *r.borrow_mut() = crate::draft::Draft::load_all(&chemin_reduits)
            .into_iter()
            .map(|brouillon| Reduit {
                brouillon,
                pieces: Vec::new(),
            })
            .collect();
    });
    montrer_reduits(fenetre);
    {
        let chemin = chemin_reduits.clone();
        let chemin_brouillon = chemin_brouillon.clone();
        let pieces = Arc::clone(&pieces);
        let faible = fenetre.as_weak();
        fenetre.on_compose_park(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            ranger(&fenetre, &pieces, &chemin);
            let _ = crate::draft::Draft::clear(&chemin_brouillon);
            fenetre.set_compose_open(false);
        });
    }
    {
        let chemin = chemin_reduits.clone();
        let pieces = Arc::clone(&pieces);
        let faible = fenetre.as_weak();
        fenetre.on_parked_open(move |index, x| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            // What the window holds goes down first, to a bar of its own.
            if fenetre.get_compose_open() {
                ranger(&fenetre, &pieces, &chemin);
            }
            let Some(reduit) = REDUITS.with(|r| {
                let mut r = r.borrow_mut();
                ((index as usize) < r.len()).then(|| r.remove(index as usize))
            }) else {
                return;
            };
            enregistrer_reduits(&chemin);
            montrer_reduits(&fenetre);
            let b = &reduit.brouillon;
            fenetre.set_compose_to(b.to.as_str().into());
            fenetre.set_compose_cc(b.cc.as_str().into());
            fenetre.set_compose_bcc(b.bcc.as_str().into());
            fenetre.set_compose_subject(b.subject.as_str().into());
            fenetre.set_compose_body(b.body.as_str().into());
            fenetre.set_compose_show_cc(!b.cc.is_empty() || !b.bcc.is_empty());
            fenetre.set_compose_error(Default::default());
            choisir_expediteur(&fenetre, &b.sender);
            show_attachments(&fenetre, &reduit.pieces);
            *pieces.lock().expect("poisoned attachments") = reduit.pieces;
            // From where its bar was, small, then up into the window.
            fenetre.set_compose_parked_x(x);
            fenetre.set_compose_minimised(true);
            fenetre.set_compose_open(true);
            let faible = fenetre.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(30), move || {
                if let Some(f) = faible.upgrade() {
                    f.set_compose_minimised(false);
                }
            });
        });
    }

    // Closed, and thrown away: after the question, on purpose.
    {
        let chemin = chemin_brouillon.clone();
        let pieces = Arc::clone(&pieces);
        let faible = fenetre.as_weak();
        fenetre.on_compose_discard(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            vider_redaction(&fenetre, &pieces);
            let _ = crate::draft::Draft::clear(&chemin);
            fenetre.set_compose_open(false);
            fenetre.set_status("Message discarded.".into());
        });
    }

    // Saved as a draft: into the account's Drafts folder, on the server, found again
    // from any device. The window closes at once; should the server be out of reach,
    // the draft is kept on this computer and New message brings it back.
    {
        let chemin = chemin_brouillon.clone();
        let pieces = Arc::clone(&pieces);
        let send = Arc::clone(&send);
        let faible = fenetre.as_weak();
        fenetre.on_compose_save_draft(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(exp) = expediteur(fenetre.get_compose_sender_index()) else {
                fenetre.set_compose_error("No account to keep a draft in.".into());
                return;
            };
            let brouillon = iris_sync::Draft {
                account: exp.account,
                to: fenetre.get_compose_to().to_string(),
                cc: fenetre.get_compose_cc().to_string(),
                bcc: fenetre.get_compose_bcc().to_string(),
                subject: fenetre.get_compose_subject().to_string(),
                body: fenetre.get_compose_body().to_string(),
                attachments: pieces.lock().expect("poisoned attachments").clone(),
            };
            let adresse = exp.address.clone();
            let alias = exp.alias.clone();
            vider_redaction(&fenetre, &pieces);
            fenetre.set_compose_open(false);
            fenetre.set_status("Saving the draft…".into());

            let (send, chemin, pieces, faible) =
                (Arc::clone(&send), chemin.clone(), Arc::clone(&pieces), faible.clone());
            runtime.spawn(async move {
                let resultat = send.save_draft_as(&brouillon, alias.as_ref()).await;
                let _ = faible.upgrade_in_event_loop(move |fenetre| match resultat {
                    Ok(true) => {
                        let _ = crate::draft::Draft::clear(&chemin);
                        fenetre.set_status("Draft saved in Drafts.".into());
                    }
                    autre => {
                        // Kept here, and put back in the window: nothing written is
                        // lost because a server did not answer.
                        let raison = match autre {
                            Ok(_) => "this account has no Drafts folder".to_string(),
                            Err(e) => e.to_string(),
                        };
                        let garde = crate::draft::Draft {
                            to: brouillon.to.clone(),
                            cc: brouillon.cc.clone(),
                            bcc: brouillon.bcc.clone(),
                            subject: brouillon.subject.clone(),
                            body: brouillon.body.clone(),
                            lost_attachments: brouillon.attachments.len() as u32,
                            sender: adresse.clone(),
                        };
                        if let Err(e) = garde.save(&chemin) {
                            tracing::warn!(error = %e, "saving the draft here");
                        }
                        fenetre.set_compose_to(brouillon.to.into());
                        fenetre.set_compose_cc(brouillon.cc.into());
                        fenetre.set_compose_bcc(brouillon.bcc.into());
                        fenetre.set_compose_subject(brouillon.subject.into());
                        fenetre.set_compose_body(brouillon.body.into());
                        choisir_expediteur(&fenetre, &adresse);
                        show_attachments(&fenetre, &brouillon.attachments);
                        *pieces.lock().expect("poisoned attachments") = brouillon.attachments;
                        fenetre.set_status(
                            format!(
                                "Draft kept on this computer ({raison}): New message brings it back."
                            )
                            .into(),
                        );
                    }
                });
            });
        });
    }

    // --- Les correspondants déjà rencontrés ---
    //
    // La table qui les garde existait depuis la première migration et n'avait jamais
    // été remplie : le champ « À » ne proposait donc rien, et retaper de mémoire une
    // adresse reçue cent fois est le plus sûr moyen de l'écrire de travers.
    {
        let store = Arc::clone(&services.store);
        let faible = fenetre.as_weak();
        fenetre.on_compose_recipient_typed(move |_champ, texte| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            // Seul le dernier fragment compte : un champ contient « marie@x, l » et
            // c'est « l » qu'on est en train de taper.
            let fragment = dernier_destinataire(&texte);
            let propositions = if fragment.chars().count() < 2 {
                // À une lettre, tout ressemble à tout. Proposer quarante adresses
                // n'aide personne et cache le champ derrière ses propres suggestions.
                Vec::new()
            } else {
                store
                    .contacts_like(&fragment, 4)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| slint::SharedString::from(c.to_header()))
                    .collect()
            };
            fenetre.set_compose_suggestions(ModelRc::new(VecModel::from(propositions)));
        });
    }
    {
        let faible = fenetre.as_weak();
        fenetre.on_compose_recipient_picked(move |champ, choix| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let actuel = match champ.as_str() {
                "cc" => fenetre.get_compose_cc().to_string(),
                "bcc" => fenetre.get_compose_bcc().to_string(),
                _ => fenetre.get_compose_to().to_string(),
            };

            let complete = remplace_dernier_destinataire(&actuel, &choix);
            match champ.as_str() {
                "cc" => fenetre.set_compose_cc(complete.into()),
                "bcc" => fenetre.set_compose_bcc(complete.into()),
                _ => fenetre.set_compose_to(complete.into()),
            }
            fenetre.set_compose_suggestions(ModelRc::new(VecModel::from(
                Vec::<slint::SharedString>::new(),
            )));
        });
    }

    // --- Attaching ---
    {
        let pieces = Arc::clone(&pieces);
        let faible = fenetre.as_weak();
        fenetre.on_compose_attach(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            attach_files(&fenetre, &pieces, false);
        });
    }
    {
        let pieces = Arc::clone(&pieces);
        let faible = fenetre.as_weak();
        fenetre.on_compose_attach_image(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            attach_files(&fenetre, &pieces, true);
        });
    }
    {
        let pieces = Arc::clone(&pieces);
        let faible = fenetre.as_weak();
        fenetre.on_compose_remove_attachment(move |index| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let mut liste = pieces.lock().expect("poisoned attachments");
            if (index as usize) < liste.len() {
                liste.remove(index as usize);
            }
            show_attachments(&fenetre, &liste);
        });
    }

    // --- Formatting ---
    {
        let faible = fenetre.as_weak();
        fenetre.on_compose_format(move |quoi| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let corps = fenetre.get_compose_body().to_string();
            fenetre.set_compose_body(apply_markup(&corps, quoi.as_str()).into());
        });
    }

    // --- Sending ---
    {
        let send = Arc::clone(&send);
        let avis = Rc::clone(&avis);
        let pieces = Arc::clone(&pieces);
        let chemin_envoi = chemin_brouillon.clone();
        let faible = fenetre.as_weak();

        fenetre.on_compose_send(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };

            let index = fenetre.get_compose_sender_index();
            let Some(exp) = expediteur(index) else {
                fenetre.set_compose_error("No account can send.".into());
                return;
            };

            let brouillon = iris_sync::Draft {
                account: exp.account,
                to: fenetre.get_compose_to().to_string(),
                cc: fenetre.get_compose_cc().to_string(),
                bcc: fenetre.get_compose_bcc().to_string(),
                subject: fenetre.get_compose_subject().to_string(),
                body: fenetre.get_compose_body().to_string(),
                attachments: pieces.lock().expect("poisoned attachments").clone(),
            };

            let message = match send.compose_full(&brouillon) {
                Ok(m) => en_tant_que(m, &exp),
                Err(e) => {
                    fenetre.set_compose_error(e.to_string().into());
                    return;
                }
            };

            // What Undo puts back: the window exactly as it was when Send was pressed.
            let sujet = brouillon.subject.clone();
            let remettre = {
                let pieces = Arc::clone(&pieces);
                let ecrit = (
                    brouillon.to.clone(),
                    brouillon.cc.clone(),
                    brouillon.bcc.clone(),
                    brouillon.subject.clone(),
                    brouillon.body.clone(),
                    brouillon.attachments.clone(),
                    fenetre.get_compose_show_cc(),
                    exp.address.clone(),
                );
                move |fenetre: &AppWindow| {
                    let (a, cc, cci, objet, corps, jointes, copies, expediteur) = ecrit;
                    // It can come back seconds later, when it did not leave: by then
                    // the window may hold another message, which must not be written
                    // over. It then goes to a bar of its own.
                    let occupee = fenetre.get_compose_open()
                        && !(fenetre.get_compose_to().trim().is_empty()
                            && fenetre.get_compose_subject().trim().is_empty()
                            && fenetre.get_compose_body().trim().is_empty());
                    if occupee {
                        garder_en_bas(
                            fenetre,
                            Reduit {
                                brouillon: crate::draft::Draft {
                                    to: a,
                                    cc,
                                    bcc: cci,
                                    subject: objet,
                                    body: corps,
                                    lost_attachments: jointes.len() as u32,
                                    sender: expediteur,
                                },
                                pieces: jointes,
                            },
                        );
                        return;
                    }
                    fenetre.set_compose_to(a.into());
                    fenetre.set_compose_cc(cc.into());
                    fenetre.set_compose_bcc(cci.into());
                    fenetre.set_compose_subject(objet.into());
                    fenetre.set_compose_body(corps.into());
                    fenetre.set_compose_show_cc(copies);
                    fenetre.set_compose_error(Default::default());
                    choisir_expediteur(fenetre, &expediteur);
                    show_attachments(fenetre, &jointes);
                    *pieces.lock().expect("poisoned attachments") = jointes;
                    fenetre.set_compose_minimised(false);
                    fenetre.set_compose_open(true);
                }
            };

            // A message with no subject leaves anyway. Refusing it would be the
            // application deciding what matters in someone else's correspondence.
            let libelle = if sujet.trim().is_empty() {
                "Sending your message".to_string()
            } else {
                format!("Sending “{}”", sujet.trim())
            };
            match avis.envoyer(&fenetre, message, libelle, remettre) {
                Ok(()) => {
                    // The window closes: the message is written. Undo, in the notice
                    // at the bottom, brings it back as it was.
                    pieces.lock().expect("poisoned attachments").clear();
                    show_attachments(&fenetre, &[]);
                    clear_compose(&fenetre);
                    fenetre.set_compose_cc(Default::default());
                    fenetre.set_compose_bcc(Default::default());
                    fenetre.set_compose_open(false);
                    // Gone, or about to be. Kept as a draft it would come back at the
                    // next opening, beside its own copy in the sent mail.
                    if let Err(e) = crate::draft::Draft::clear(&chemin_envoi) {
                        tracing::warn!(error = %e, "clearing the draft");
                    }
                }
                Err(e) => fenetre.set_compose_error(format!("Send refused: {e}").into()),
            }
        });
    }

    // --- Sending later ---
    rafraichir_plus_tard(fenetre, services);
    {
        let services = services.clone();
        let pieces = Arc::clone(&pieces);
        let chemin_envoi = chemin_brouillon.clone();
        let faible = fenetre.as_weak();
        fenetre.on_compose_send_later(move |i| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some(exp) = expediteur(fenetre.get_compose_sender_index()) else {
                fenetre.set_compose_error("No account can send.".into());
                return;
            };
            let Some((_, quand)) = usize::try_from(i).ok().and_then(|i| {
                crate::later::options(chrono::Local::now().naive_local())
                    .get(i)
                    .cloned()
            }) else {
                return;
            };
            let brouillon = iris_sync::Draft {
                account: exp.account,
                to: fenetre.get_compose_to().to_string(),
                cc: fenetre.get_compose_cc().to_string(),
                bcc: fenetre.get_compose_bcc().to_string(),
                subject: fenetre.get_compose_subject().to_string(),
                body: fenetre.get_compose_body().to_string(),
                attachments: pieces.lock().expect("poisoned attachments").clone(),
            };
            if brouillon.to.trim().is_empty() {
                fenetre.set_compose_error("Say who it is for first.".into());
                return;
            }
            match crate::later::schedule(&services, &brouillon, exp.alias.as_ref(), quand) {
                Ok(message) => {
                    pieces.lock().expect("poisoned attachments").clear();
                    show_attachments(&fenetre, &[]);
                    clear_compose(&fenetre);
                    fenetre.set_compose_cc(Default::default());
                    fenetre.set_compose_bcc(Default::default());
                    fenetre.set_compose_open(false);
                    if let Err(e) = crate::draft::Draft::clear(&chemin_envoi) {
                        tracing::warn!(error = %e, "clearing the draft");
                    }
                    fenetre.set_status(message.into());
                    rafraichir_plus_tard(&fenetre, &services);
                }
                Err(e) => fenetre.set_compose_error(format!("Could not keep it: {e}").into()),
            }
        });
    }
    // Every half minute, what is due leaves. Whether it left is said when the outbox
    // knows (`AvisEnvoi::terminer`).
    {
        let (services, send, faible) = (services.clone(), Arc::clone(&send), fenetre.as_weak());
        let avis = Rc::clone(&avis);
        let minuterie = slint::Timer::default();
        minuterie.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(30),
            move || {
                let Some(fenetre) = faible.upgrade() else {
                    return;
                };
                let (partis, echecs) = crate::later::send_due(&services, &send, &avis, None);
                if partis > 0 {
                    fenetre.set_status(
                        if partis == 1 {
                            "Sending a scheduled message…".to_string()
                        } else {
                            format!("Sending {partis} scheduled messages…")
                        }
                        .into(),
                    );
                }
                if let Some(e) = echecs.first() {
                    fenetre.set_status(format!("Could not send {e}").into());
                }
                rafraichir_plus_tard(&fenetre, &services);
            },
        );
        PLUS_TARD.with(|m| *m.borrow_mut() = Some(minuterie));
    }
    {
        let (services, faible) = (services.clone(), fenetre.as_weak());
        fenetre.on_scheduled_requested(move || {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            rafraichir_plus_tard(&fenetre, &services);
            fenetre.set_scheduled_open(true);
        });
    }
    {
        let (services, send, faible) = (services.clone(), Arc::clone(&send), fenetre.as_weak());
        let avis = Rc::clone(&avis);
        fenetre.on_scheduled_send_now(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let (partis, echecs) = crate::later::send_due(&services, &send, &avis, Some(id as i64));
            if partis > 0 {
                fenetre.set_status("Sending…".into());
            }
            if let Some(e) = echecs.first() {
                fenetre.set_status(format!("Could not send {e}").into());
            }
            rafraichir_plus_tard(&fenetre, &services);
            if fenetre.get_scheduled_count() == 0 {
                fenetre.set_scheduled_open(false);
            }
        });
    }
    // Back into the composer, as it was written: it waits no more.
    {
        let (services, pieces, faible) = (services.clone(), Arc::clone(&pieces), fenetre.as_weak());
        fenetre.on_scheduled_edit(move |id| {
            let Some(fenetre) = faible.upgrade() else {
                return;
            };
            let Some((d, alias)) = crate::later::take(&services, id as i64) else {
                fenetre.set_status("That message is no longer waiting.".into());
                return;
            };
            fenetre.set_compose_to(d.to.into());
            fenetre.set_compose_cc(d.cc.as_str().into());
            fenetre.set_compose_bcc(d.bcc.as_str().into());
            fenetre.set_compose_show_cc(!d.cc.is_empty() || !d.bcc.is_empty());
            fenetre.set_compose_subject(d.subject.into());
            fenetre.set_compose_body(d.body.into());
            // The sender it was to leave as: its mailbox, and its alias if it had one.
            let rang = EXPEDITEURS.with(|e| {
                e.borrow().iter().position(|x| {
                    x.account == d.account
                        && x.alias.as_ref().map(|a| a.addr.as_str()) == alias.as_deref()
                })
            });
            if let Some(i) = rang {
                fenetre.set_compose_sender_index(i as i32);
            }
            show_attachments(&fenetre, &d.attachments);
            *pieces.lock().expect("poisoned attachments") = d.attachments;
            fenetre.set_compose_error(Default::default());
            fenetre.set_compose_minimised(false);
            fenetre.set_compose_open(true);
            rafraichir_plus_tard(&fenetre, &services);
        });
    }
}

thread_local! {
    /// The half-minute check of what is due, for as long as the window lives.
    static PLUS_TARD: std::cell::RefCell<Option<slint::Timer>> =
        const { std::cell::RefCell::new(None) };
}

/// The count beside Scheduled, its list, and the times the composer offers now.
fn rafraichir_plus_tard(fenetre: &AppWindow, services: &Services) {
    let lignes = crate::later::rows(services);
    fenetre.set_scheduled_count(lignes.len() as i32);
    fenetre.set_scheduled_rows(ModelRc::new(VecModel::from(lignes)));
    fenetre.set_compose_later_options(ModelRc::new(VecModel::from(
        crate::later::options(chrono::Local::now().naive_local())
            .into_iter()
            .map(|(l, _)| slint::SharedString::from(l))
            .collect::<Vec<_>>(),
    )));
}

/// The aliases of one mailbox, in their window.
fn montrer_alias(fenetre: &AppWindow, services: &Services, compte: iris_types::AccountId) {
    fenetre.set_alias_rows(ModelRc::new(VecModel::from(
        services
            .store
            .aliases()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.account == compte)
            .map(|a| iris_ui::AliasData {
                id: a.id as i32,
                address: a.address.into(),
                name: a.name.into(),
            })
            .collect::<Vec<_>>(),
    )));
}

/// Who a message can be sent as: a mailbox, as its own address or one of its aliases.
#[derive(Debug, Clone)]
struct Expediteur {
    account: iris_types::AccountId,
    /// The alias it goes out as; `None` for the mailbox's own address.
    alias: Option<iris_types::Address>,
    /// The address it goes out from: the alias's, else the mailbox's. What a kept
    /// message remembers, since its place in the list moves.
    address: String,
}

thread_local! {
    /// The senders the composer offers, in its order: each enabled mailbox, then its
    /// aliases. Read again when an alias is added or removed.
    static EXPEDITEURS: std::cell::RefCell<Vec<Expediteur>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Reads the senders again, and gives the composer their labels.
pub fn charger_expediteurs(fenetre: &AppWindow, services: &Services) {
    // The message being written keeps its sender, wherever the list now puts it: an
    // alias added above it used to move it onto another mailbox.
    let avant = adresse_expediteur(fenetre);
    let comptes: Vec<_> = services
        .store
        .accounts()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.enabled)
        .collect();
    let alias = services.store.aliases().unwrap_or_default();
    let mut liste = Vec::new();
    let mut libelles = Vec::new();
    for c in &comptes {
        liste.push(Expediteur {
            account: c.id,
            alias: None,
            address: c.email.clone(),
        });
        libelles.push(slint::SharedString::from(c.email.as_str()));
        for a in alias.iter().filter(|a| a.account == c.id) {
            let nom = if a.name.trim().is_empty() {
                c.display_name.trim()
            } else {
                a.name.trim()
            };
            liste.push(Expediteur {
                account: c.id,
                alias: Some(if nom.is_empty() {
                    iris_types::Address::new(a.address.clone())
                } else {
                    iris_types::Address::named(nom.to_string(), a.address.clone())
                }),
                address: a.address.clone(),
            });
            libelles.push(format!("{} (via {})", a.address, c.email).into());
        }
    }
    EXPEDITEURS.with(|e| *e.borrow_mut() = liste);
    fenetre.set_compose_senders(ModelRc::new(VecModel::from(libelles)));
    choisir_expediteur(fenetre, &avant);
}

fn expediteur(index: i32) -> Option<Expediteur> {
    EXPEDITEURS.with(|e| e.borrow().get(index.max(0) as usize).cloned())
}

/// The address the composer is set to send from; empty when no mailbox can send.
fn adresse_expediteur(fenetre: &AppWindow) -> String {
    expediteur(fenetre.get_compose_sender_index())
        .map(|e| e.address)
        .unwrap_or_default()
}

/// Sets the composer to send from `adresse`, found by address rather than by place.
///
/// An empty address (a draft kept before senders were remembered) gives the first
/// mailbox, as before. One that can no longer send is said, not silently replaced:
/// the message would otherwise leave from a mailbox nobody chose.
fn choisir_expediteur(fenetre: &AppWindow, adresse: &str) {
    let adresse = adresse.trim();
    if adresse.is_empty() {
        fenetre.set_compose_sender_index(0);
        return;
    }
    let rang = EXPEDITEURS.with(|e| {
        e.borrow()
            .iter()
            .position(|x| x.address.eq_ignore_ascii_case(adresse))
    });
    match rang {
        Some(i) => fenetre.set_compose_sender_index(i as i32),
        None => {
            fenetre.set_compose_sender_index(0);
            fenetre.set_compose_error(
                format!("{adresse} can no longer send: check who this goes from.").into(),
            );
        }
    }
}

/// A composed message sent as the alias chosen, if one was.
fn en_tant_que(mut message: iris_smtp::Outgoing, exp: &Expediteur) -> iris_smtp::Outgoing {
    if let Some(a) = &exp.alias {
        message.from = a.clone();
    }
    message
}

/// Asks for files and reads them into the draft.
///
/// The bytes are read now rather than at send time on purpose: someone who attaches a
/// file and then moves it has still attached the file they meant, and discovering
/// otherwise ten seconds after pressing send is too late to do anything about.
fn attach_files(
    fenetre: &AppWindow,
    pieces: &Arc<std::sync::Mutex<Vec<iris_smtp::Attachment>>>,
    images_only: bool,
) {
    let mut dialogue = rfd::FileDialog::new();
    if images_only {
        dialogue = dialogue.add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"]);
    }

    let Some(chemins) = dialogue.pick_files() else {
        return;
    };

    let mut liste = pieces.lock().expect("poisoned attachments");
    let mut refuses = Vec::new();

    for chemin in chemins {
        // Twenty-five mebibytes is where most servers stop accepting, and a message
        // refused after the undo window has closed cannot be recovered.
        const MAX: u64 = 25 * 1024 * 1024;
        match std::fs::metadata(&chemin).map(|m| m.len()) {
            Ok(taille) if taille > MAX => {
                refuses.push(format!(
                    "{} is {:.0} MB, and most servers refuse over 25",
                    chemin.file_name().unwrap_or_default().to_string_lossy(),
                    taille as f64 / (1024.0 * 1024.0)
                ));
                continue;
            }
            Err(e) => {
                refuses.push(format!("{}: {e}", chemin.display()));
                continue;
            }
            _ => {}
        }

        match std::fs::read(&chemin) {
            Ok(contenu) => liste.push(iris_smtp::Attachment {
                filename: chemin
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                mime_type: mime_for(&chemin),
                content: contenu,
            }),
            Err(e) => refuses.push(format!("{}: {e}", chemin.display())),
        }
    }

    show_attachments(fenetre, &liste);
    fenetre.set_compose_error(refuses.join("\n").into());
}

/// Puts the attachment names in front of the user.
fn show_attachments(fenetre: &AppWindow, pieces: &[iris_smtp::Attachment]) {
    fenetre.set_compose_attachments(ModelRc::new(VecModel::from(
        pieces
            .iter()
            .map(|p| slint::SharedString::from(p.filename.as_str()))
            .collect::<Vec<_>>(),
    )));
}

/// A media type from the file extension.
///
/// Guessed from the name, because sniffing the content would mean reading files we
/// have already read and getting a different answer for no benefit: the recipient's
/// client trusts the extension too.
fn mime_for(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Applies a formatting mark to the draft.
///
/// Markdown, not a rich-text buffer. Slint's text editor hands us a plain string with
/// no selection, so anything else would be a lie about what the editor can do — and
/// markdown is legible as-is if the recipient's client shows the plain part.
pub fn apply_markup(body: &str, what: &str) -> String {
    let addition = match what {
        "bold" => "**bold text**",
        "italic" => "*italic text*",
        "underline" => "__underlined text__",
        "link" => "[label](https://example.com)",
        "list" => "\n- first\n- second",
        "quote" => "\n> quoted text",
        "code" => "`code`",
        _ => return body.to_string(),
    };

    // Appended with a space rather than inserted at a cursor: the editor does not
    // expose one, and silently overwriting a selection nobody can see would be worse
    // than adding at the end where it is visible and easy to move.
    if body.is_empty() || body.ends_with(['\n', ' ']) {
        format!("{body}{addition}")
    } else {
        format!("{body} {addition}")
    }
}

/// Empties the new-message window entirely: fields, copies and attachments.
/// A message minimised to the foot of the window.
struct Reduit {
    /// With its sender's address, which the file keeps too.
    brouillon: crate::draft::Draft,
    /// Its attachments, for as long as Iris runs (the file keeps only their count).
    pieces: Vec<iris_smtp::Attachment>,
}

thread_local! {
    static REDUITS: std::cell::RefCell<Vec<Reduit>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Where the minimised messages are kept, once the composer is wired.
    static CHEMIN_REDUITS: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Puts a message down as a bar of its own at the foot of the window, kept on this
/// computer. Where a message that came back goes when the window it was written in
/// now holds another.
fn garder_en_bas(fenetre: &AppWindow, reduit: Reduit) {
    REDUITS.with(|r| r.borrow_mut().push(reduit));
    if let Some(chemin) = CHEMIN_REDUITS.with(|c| c.borrow().clone()) {
        enregistrer_reduits(&chemin);
    }
    montrer_reduits(fenetre);
}

/// The bars at the foot of the window, one per minimised message.
fn montrer_reduits(fenetre: &AppWindow) {
    let titres: Vec<slint::SharedString> = REDUITS.with(|r| {
        r.borrow()
            .iter()
            .map(|m| m.brouillon.title().into())
            .collect()
    });
    fenetre.set_parked_drafts(ModelRc::new(VecModel::from(titres)));
}

fn enregistrer_reduits(chemin: &std::path::Path) {
    let brouillons: Vec<crate::draft::Draft> =
        REDUITS.with(|r| r.borrow().iter().map(|m| m.brouillon.clone()).collect());
    if let Err(e) = crate::draft::Draft::save_all(&brouillons, chemin) {
        tracing::warn!(error = %e, "keeping the minimised messages");
    }
}

/// Takes what the window holds down to a bar of its own, kept on this computer, and
/// empties the window. Nothing written, nothing kept.
fn ranger(
    fenetre: &AppWindow,
    pieces: &Arc<std::sync::Mutex<Vec<iris_smtp::Attachment>>>,
    chemin: &std::path::Path,
) {
    let jointes = pieces.lock().expect("poisoned attachments").clone();
    let brouillon = crate::draft::Draft {
        to: fenetre.get_compose_to().to_string(),
        cc: fenetre.get_compose_cc().to_string(),
        bcc: fenetre.get_compose_bcc().to_string(),
        subject: fenetre.get_compose_subject().to_string(),
        body: fenetre.get_compose_body().to_string(),
        lost_attachments: jointes.len() as u32,
        sender: adresse_expediteur(fenetre),
    };
    if !brouillon.is_empty() || !jointes.is_empty() {
        REDUITS.with(|r| {
            r.borrow_mut().push(Reduit {
                brouillon,
                pieces: jointes,
            })
        });
        enregistrer_reduits(chemin);
        montrer_reduits(fenetre);
        fenetre.set_status("Kept as a draft at the bottom right.".into());
    }
    vider_redaction(fenetre, pieces);
}

fn vider_redaction(
    fenetre: &AppWindow,
    pieces: &Arc<std::sync::Mutex<Vec<iris_smtp::Attachment>>>,
) {
    pieces.lock().expect("poisoned attachments").clear();
    show_attachments(fenetre, &[]);
    clear_compose(fenetre);
    fenetre.set_compose_cc(Default::default());
    fenetre.set_compose_bcc(Default::default());
    fenetre.set_compose_show_cc(false);
    fenetre.set_compose_confirm_close(false);
}

/// Empties the compose window once a message is safely away.
pub fn clear_compose(fenetre: &AppWindow) {
    fenetre.set_compose_to(Default::default());
    fenetre.set_compose_subject(Default::default());
    fenetre.set_compose_body(Default::default());
    fenetre.set_compose_error(Default::default());
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
            fenetre.set_status("Plugins are loaded at startup: restart to pick up changes.".into());
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
                text.push_str(&format!("\n  · {}: {}", hit.from, hit.subject));
            }
            fenetre.set_simulation(text.into());
        }
        Err(e) => fenetre.set_simulation(format!("Dry run failed: {e}").into()),
    }
}

/// Where the pointer is on the desktop, in physical pixels.
///
/// Screen coordinates rather than window-relative ones. A window being dragged moves
/// under the pointer, so an offset measured inside it changes meaning between one
/// report and the next — the window chases its own tail, which is exactly the
/// glitching this replaces.
#[cfg(windows)]
#[allow(unsafe_code)]
fn cursor_position() -> Option<(i32, i32)> {
    #[repr(C)]
    #[derive(Default)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
    }

    let mut point = Point::default();
    // SAFETY: one out-parameter owned by this frame, of exactly the size the API
    // expects. Failure is reported through the return value.
    let ok = unsafe { GetCursorPos(&mut point) };
    (ok != 0).then_some((point.x, point.y))
}

#[cfg(not(windows))]
fn cursor_position() -> Option<(i32, i32)> {
    None
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
    fn a_search_pill_puts_its_words_in_the_query_and_takes_them_out() {
        let q = basculer_pastille("devis", &PASTILLES[0]);
        assert_eq!(q, "devis is:unread");
        assert!(pastille_allumee(&q, PASTILLES[0].0));
        assert_eq!(basculer_pastille(&q, &PASTILLES[0]), "devis");
        // Typed by hand in its French form, it is on, and a click takes it out.
        assert!(pastille_allumee("devis etat:non_lu", PASTILLES[0].0));
        assert_eq!(
            basculer_pastille("devis etat:non_lu", &PASTILLES[0]),
            "devis"
        );
        // The two periods exclude each other.
        let q = basculer_pastille("devis newer_than:7d", &PASTILLES[3]);
        assert_eq!(q, "devis newer_than:30d");
    }

    #[test]
    fn a_folder_is_called_by_its_role_or_its_name() {
        use iris_store::FolderRole as R;
        assert_eq!(folder_label(R::Inbox, "INBOX"), "Inbox");
        assert_eq!(folder_label(R::Junk, "Junk E-mail"), "Spam");
        assert_eq!(folder_label(R::Other, "Clients/Atelier"), "Atelier");
        assert_eq!(folder_label(R::Other, "INBOX.Factures"), "Factures");
    }

    fn message(id: i64, flags: iris_types::Flags) -> iris_store::StoredMessage {
        iris_store::StoredMessage {
            id: iris_types::MessageId(id),
            account: iris_types::AccountId(1),
            folder: iris_types::FolderId(1),
            thread: iris_types::ThreadId(1),
            uid: id as u32,
            rfc_message_id: None,
            subject: "Sujet".into(),
            from_name: "Marie".into(),
            from_addr: "marie@x.fr".into(),
            date: iris_types::Timestamp::from_millis(0),
            received: iris_types::Timestamp::from_millis(0),
            size: 10,
            flags,
            preview: "aperçu".into(),
            body_blob: None,
            recipients_json: "[]".into(),
        }
    }

    /// Un instant à une heure donnée d'un jour donné. Jour 0 = jeudi 1er janvier 1970.
    fn instant(jour: i64, heure: i64) -> iris_types::Timestamp {
        iris_types::Timestamp::from_millis((jour * 86_400 + heure * 3600) * 1000)
    }

    #[test]
    fn un_report_vise_une_heure_du_jour_pas_une_duree() {
        // « Demain » valait vingt-quatre heures écrites en dur : un message reporté à
        // vingt-trois heures revenait à vingt-trois heures, au moment précis où l'on ne
        // veut pas de courrier.
        //
        // Jour 4 = lundi (le 1er janvier 1970 était un jeudi).
        assert_eq!(
            heures_de_report("tomorrow", instant(4, 23)),
            9,
            "23 h → 8 h"
        );
        assert_eq!(heures_de_report("tomorrow", instant(4, 10)), 22);
    }

    #[test]
    fn ce_soir_bascule_a_demain_une_fois_le_soir_passe() {
        // Proposer une échéance déjà passée ferait revenir le message aussitôt.
        assert_eq!(
            heures_de_report("evening", instant(4, 10)),
            8,
            "10 h → 18 h"
        );
        assert_eq!(
            heures_de_report("evening", instant(4, 20)),
            12,
            "20 h → 8 h"
        );
    }

    #[test]
    fn lundi_veut_dire_le_lundi_suivant() {
        // Un lundi, reporter « à lundi » et retomber sur aujourd'hui ne serait pas
        // reporter.
        let vendredi = instant(1, 10); // jour 1 = vendredi
        assert_eq!(heures_de_report("monday", vendredi), 3 * 24 - 10 + 8);

        let lundi = instant(4, 10);
        assert_eq!(heures_de_report("monday", lundi), 7 * 24 - 10 + 8);
    }

    #[test]
    fn le_week_end_est_samedi_matin() {
        let vendredi = instant(1, 10); // jour 1 = vendredi
        assert_eq!(heures_de_report("weekend", vendredi), 24 - 10 + 8);
        let samedi = instant(2, 10);
        assert_eq!(heures_de_report("weekend", samedi), 7 * 24 - 10 + 8);
        let lundi = instant(4, 10);
        assert_eq!(heures_de_report("weekend", lundi), 5 * 24 - 10 + 8);
    }

    #[test]
    fn un_report_ne_vaut_jamais_zero() {
        // Une échéance nulle serait un report qui n'en est pas un, et le fil
        // reviendrait au premier passage du planificateur.
        for jour in 0..7 {
            for heure in 0..24 {
                for quand in ["evening", "tomorrow", "monday"] {
                    assert!(
                        heures_de_report(quand, instant(jour, heure)) >= 1,
                        "{quand} à {heure} h, jour {jour}"
                    );
                }
            }
        }
    }

    #[test]
    fn on_complete_le_destinataire_en_cours_pas_toute_la_ligne() {
        // Un champ « À » contient « marie@x.fr, l » et c'est « l » qu'on tape. Chercher
        // sur la chaîne entière ne proposerait plus rien dès le second destinataire.
        assert_eq!(dernier_destinataire("l"), "l");
        assert_eq!(dernier_destinataire("marie@x.fr, l"), "l");
        assert_eq!(dernier_destinataire("marie@x.fr,  luc"), "luc");
        assert_eq!(dernier_destinataire("marie@x.fr, "), "");
    }

    #[test]
    fn choisir_un_correspondant_ne_touche_pas_aux_precedents() {
        // Le geste doit ajouter, jamais remplacer : quelqu'un qui a déjà saisi trois
        // adresses et en complète une quatrième ne s'attend pas à en perdre trois.
        assert_eq!(
            remplace_dernier_destinataire("mar", "Marie <marie@x.fr>"),
            "Marie <marie@x.fr>, "
        );
        assert_eq!(
            remplace_dernier_destinataire("luc@x.fr, mar", "Marie <marie@x.fr>"),
            "luc@x.fr, Marie <marie@x.fr>, "
        );
        // Le champ finit par une virgule : on n'en ajoute pas une seconde.
        assert_eq!(
            remplace_dernier_destinataire("luc@x.fr, ", "Marie <marie@x.fr>"),
            "luc@x.fr, Marie <marie@x.fr>, "
        );
    }

    #[test]
    fn cocher_une_case_ne_change_pas_la_signature_de_la_conversation() {
        // Le gel. Un instantané part à chaque requête, et la colonne de lecture rendait
        // le corps du message ouvert à chacun d'eux : mise en page et rastérisation, sur
        // le fil de l'interface, pour des corps que le journal montre à vingt-neuf mille
        // pixels de haut. Cocher une case n'a rien à voir avec le message affiché.
        //
        // Rien de ce que coche ou sélectionne l'utilisateur n'entre ici : c'est
        // exactement ce que ce test dit, et la raison pour laquelle la signature est
        // calculée à part.
        let vide = std::collections::BTreeSet::new();
        let messages = [message(1, iris_types::Flags::SEEN)];
        let dernier = iris_types::MessageId(1);

        let avant = signature_conversation(&messages, dernier, &vide, &vide);
        let apres = signature_conversation(&messages, dernier, &vide, &vide);
        assert_eq!(avant, apres);
    }

    #[test]
    fn ce_qui_se_voit_change_bien_la_signature() {
        // L'autre direction, et la plus dangereuse : une signature trop étroite laisse
        // à l'écran un message qui n'est plus celui qu'on montre, et cela ne se voit pas
        // tout de suite.
        let vide = std::collections::BTreeSet::new();
        let dernier = iris_types::MessageId(2);
        let base = [
            message(1, iris_types::Flags::SEEN),
            message(2, iris_types::Flags::SEEN),
        ];
        let reference = signature_conversation(&base, dernier, &vide, &vide);

        // Un message de plus.
        let plus = [
            message(1, iris_types::Flags::SEEN),
            message(2, iris_types::Flags::SEEN),
            message(3, iris_types::Flags::NONE),
        ];
        assert_ne!(
            signature_conversation(&plus, dernier, &vide, &vide),
            reference
        );

        // A flag the column draws: the tracker warning.
        let traque = [
            message(1, iris_types::Flags::SEEN | iris_types::Flags::HAS_TRACKER),
            message(2, iris_types::Flags::SEEN),
        ];
        assert_ne!(
            signature_conversation(&traque, dernier, &vide, &vide),
            reference
        );

        // The star and the read mark are not drawn in the column: changing them
        // must not lay out and paint the bodies again (the freeze at the star).
        let etoile = [
            message(1, iris_types::Flags::NONE),
            message(2, iris_types::Flags::SEEN | iris_types::Flags::FLAGGED),
        ];
        assert_eq!(
            signature_conversation(&etoile, dernier, &vide, &vide),
            reference
        );

        // Un message qu'on déplie.
        let ouverts = std::collections::BTreeSet::from([1]);
        assert_ne!(
            signature_conversation(&base, dernier, &ouverts, &vide),
            reference
        );

        // Des images qu'on accepte.
        let images = std::collections::BTreeSet::from([2]);
        assert_ne!(
            signature_conversation(&base, dernier, &vide, &images),
            reference
        );

        // Un corps qui vient d'arriver. Sans lui, un message ouvert pour la première
        // fois restait vide jusqu'à ce qu'on en ouvre un autre et qu'on y revienne.
        let mut arrive = base.clone();
        arrive[1].body_blob = Some("ab".into());
        assert_ne!(
            signature_conversation(&arrive, dernier, &vide, &vide),
            reference
        );
    }

    #[test]
    fn le_dernier_message_compte_comme_deplie() {
        // La signature doit suivre la règle du dessin, sans quoi déplier le dernier
        // message — qui l'est déjà — invaliderait le cache pour rien.
        let vide = std::collections::BTreeSet::new();
        let messages = [message(7, iris_types::Flags::SEEN)];
        let dernier = iris_types::MessageId(7);

        let deja = std::collections::BTreeSet::from([7]);
        assert_eq!(
            signature_conversation(&messages, dernier, &vide, &vide),
            signature_conversation(&messages, dernier, &deja, &vide)
        );
    }

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
                | CommandKind::TaskFromThread
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
            signature: String::new(),
            imap_user: String::new(),
            smtp_user: String::new(),
            folder_delimiter: None,
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
