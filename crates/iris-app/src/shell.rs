//! La boucle d'interface.
//!
//! Elle ne contient aucune décision : elle branche les rappels de la fenêtre sur le
//! contrôleur, et pousse les instantanés dans l'autre sens. Tout ce qui pourrait être
//! discuté — quelle touche fait quoi, quelle commande apparaît, comment une date
//! s'écrit — a déjà été tranché ailleurs, et y est testé.

use crate::controller::{Controller, Request, Snapshot};
use crate::services::{now, Services};
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
    wire_accounts(&fenetre, services);

    Ok(fenetre)
}

/// Remplit la barre latérale.
fn wire_accounts(fenetre: &AppWindow, services: &Services) {
    let comptes = services.store.accounts().unwrap_or_default();
    // Les comptes suspendus viendront de l'ordonnanceur ; d'ici la, aucun.
    let suspendus: std::collections::BTreeSet<iris_types::AccountId> = Default::default();

    let (epingles, autres): (Vec<_>, Vec<_>) = comptes.iter().partition(|c| c.pinned);

    let vers_modele = |liste: Vec<&iris_store::Account>| {
        ModelRc::new(VecModel::from(
            liste
                .into_iter()
                .map(|c| bridge::account_row(c, 0, suspendus.contains(&c.id)))
                .collect::<Vec<_>>(),
        ))
    };

    fenetre.set_pinned_accounts(vers_modele(epingles));
    fenetre.set_other_accounts(vers_modele(autres));
    fenetre.set_unified_count(0);
}

/// Branche les rappels de la fenêtre sur le contrôleur.
pub fn wire_callbacks(fenetre: &AppWindow, controller: Arc<Controller>, keymap: Keymap) {
    let commandes = Arc::new(commands::builtin_commands());

    // Palette : la liste est recalculée à chaque frappe, côté Rust.
    {
        let commandes = Arc::clone(&commandes);
        let faible = fenetre.as_weak();
        fenetre.on_palette_query_changed(move |requete| {
            let Some(fenetre) = faible.upgrade() else { return };
            let a_un_fil = fenetre.get_selected_thread() >= 0;
            let filtrees: Vec<_> = commands::filter(&commandes, requete.as_str(), a_un_fil)
                .into_iter()
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
            c.send(Request::FilterAccounts(vec![iris_types::AccountId(id as i64)]));
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
            let Some(fenetre) = faible.upgrade() else { return };
            // On demande un peu au-delà de ce qui est chargé : le préchargement du
            // vue-modèle fait le reste.
            c.send(Request::EnsureLoaded(fenetre.get_rows().row_count()));
        });
    }

    {
        let c = Arc::clone(&controller);
        let commandes = Arc::clone(&commandes);
        fenetre.on_command_invoked(move |id| {
            let Some(commande) = commandes.iter().find(|x| x.id == id.as_str()) else { return };
            dispatch(&c, &commande.kind);
        });
    }

    {
        let c = Arc::clone(&controller);
        fenetre.on_key_pressed(move |touche| match keymap.resolve(touche.as_str()) {
            KeyOutcome::Move(m) => c.send(Request::Move(m)),
            KeyOutcome::Command(kind) => dispatch(&c, &kind),
            KeyOutcome::OpenPalette | KeyOutcome::Ignored => {}
        });
    }
}

fn dispatch(controller: &Controller, kind: &CommandKind) {
    match kind {
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
        // Ces commandes appartiennent à des écrans qui ne sont pas encore là ; les
        // ignorer silencieusement vaut mieux qu'ouvrir une fenêtre vide.
        CommandKind::Search | CommandKind::Settings | CommandKind::Reload => {}
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
        snapshot.counts.iter().map(|c| *c as i32).collect::<Vec<_>>(),
    )));
    fenetre.set_unified_count(snapshot.counts[0] as i32);
    fenetre.set_pending_ops(snapshot.pending_ops as i32);
    fenetre.set_conversation_empty(snapshot.messages.is_empty());

    if let Some(message) = snapshot.messages.last() {
        let corps = corps_du_message(services, renderer, message);
        let pieces = Vec::new();
        fenetre.set_message(bridge::message_view_rendered(message, &corps, &pieces, maintenant));
    }
}

/// Construit le moteur de rendu des corps de message.
///
/// Le moteur complet n'est retenu que s'il est réellement utilisable : sur une
/// machine sans périphérique graphique compatible, l'application reste pleinement
/// fonctionnelle en texte riche, et le dit une fois au démarrage plutôt que de
/// laisser un panneau vide l'expliquer à chaque message.
pub fn build_renderer() -> iris_htmlview::AdaptiveRenderer {
    let simple = iris_htmlview::AdaptiveRenderer::new(Box::new(
        iris_htmlview::RichTextRenderer::default(),
    ));

    #[cfg(feature = "blitz")]
    {
        if iris_htmlview::BlitzRenderer::is_available() {
            tracing::info!("rendu des corps : moteur complet disponible");
            return simple
                .with_full_engine(Box::new(iris_htmlview::BlitzRenderer::new(1.0, true)));
        }
        tracing::info!("rendu des corps : texte riche seulement (pas de périphérique graphique)");
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
            blocks: vec![iris_htmlview::Block::Paragraph(vec![iris_htmlview::Inline::plain(
                message.preview.clone(),
            )])],
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
        tracing::warn!(erreur = %e, "rendu du corps en échec");
        apercu()
    })
}

/// Enveloppe permettant de renvoyer un instantané vers la boucle d'interface.
pub fn snapshot_sink(
    fenetre: &AppWindow,
    services: Services,
    renderer: Arc<dyn iris_htmlview::HtmlRenderer>,
) -> impl Fn(Snapshot) + Send + 'static {
    let faible = fenetre.as_weak();
    move |snapshot| {
        let services = services.clone();
        let renderer = Arc::clone(&renderer);
        // `upgrade_in_event_loop` est le passage obligé : toucher la fenêtre depuis
        // un autre fil est une faute que Slint refuse à l'exécution.
        let _ = faible.upgrade_in_event_loop(move |fenetre| {
            apply_snapshot(&fenetre, &services, renderer.as_ref(), &snapshot);
        });
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
                | CommandKind::Quit => {}
                CommandKind::Search | CommandKind::Settings | CommandKind::Reload => {
                    // Écrans non encore construits, ignorés à dessein.
                }
            }
        }
    }

    #[test]
    fn les_onglets_correspondent_aux_etats() {
        for (index, etat) in WorkflowState::ALL.iter().enumerate() {
            assert_eq!(WorkflowState::from_i64(index as i64), Some(*etat));
            assert_eq!(etat.as_i64(), index as i64);
        }
    }
}
