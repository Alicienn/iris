//! Les tags des adresses : les gérer, et les poser sur une boîte.
//!
//! Deux endroits. La fenêtre « Tags », ouverte depuis le bas de la colonne des
//! comptes, les crée, les renomme, les recolore et les supprime. Le menu d'un compte
//! — clic droit — les coche et les décoche pour cette boîte, avec une recherche en
//! tête de liste qui peut aussi en créer un à la volée.

use crate::services::{now, Services};
use crate::shell::refresh_accounts;
use iris_types::AccountId;
use iris_ui::{AccountTagData, AppWindow};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};

/// Les couleurs proposées pour un tag.
pub const PALETTE: [&str; 8] = [
    "#5b8def", "#e0795b", "#4fb286", "#b67be6", "#e3b341", "#e0608c", "#3fb1c9", "#8a9a5b",
];

fn donnee(t: &iris_store::AccountTag, coche: bool) -> AccountTagData {
    AccountTagData {
        id: t.id as i32,
        name: t.name.as_str().into(),
        color: crate::calendar::couleur(&t.color),
        count: t.accounts as i32,
        checked: coche,
    }
}

/// Les tags du menu d'un compte : cochés s'il les porte, filtrés par la recherche.
pub fn menu_tags(services: &Services, compte: AccountId, recherche: &str) -> Vec<AccountTagData> {
    let recherche = recherche.trim().to_lowercase();
    let siens = services
        .store
        .account_tag_links()
        .unwrap_or_default()
        .remove(&compte)
        .unwrap_or_default();
    services
        .store
        .account_tags()
        .unwrap_or_default()
        .iter()
        .filter(|t| recherche.is_empty() || t.name.to_lowercase().contains(&recherche))
        .map(|t| donnee(t, siens.contains(&t.id)))
        .collect()
}

/// Remplit la fenêtre de gestion.
fn remplir(f: &AppWindow, services: &Services) {
    f.set_account_tags(ModelRc::new(VecModel::from(
        services
            .store
            .account_tags()
            .unwrap_or_default()
            .iter()
            .map(|t| donnee(t, false))
            .collect::<Vec<_>>(),
    )));
}

/// La couleur d'un nouveau tag : la première que personne n'a encore, sinon la suite.
fn couleur_suivante(services: &Services) -> &'static str {
    let prises: Vec<String> = services
        .store
        .account_tags()
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.color)
        .collect();
    PALETTE
        .iter()
        .find(|c| !prises.iter().any(|p| p.eq_ignore_ascii_case(c)))
        .copied()
        .unwrap_or(PALETTE[prises.len() % PALETTE.len()])
}

/// Relit le menu d'un compte ouvert, avec la recherche en cours.
fn remplir_menu(f: &AppWindow, services: &Services, sujet: &Mutex<Option<AccountId>>) {
    if let Some(compte) = *sujet.lock().expect("poisoned account menu") {
        f.set_account_menu_tags(ModelRc::new(VecModel::from(menu_tags(
            services,
            compte,
            f.get_account_tag_search().as_str(),
        ))));
    }
}

pub fn wire_tags(f: &AppWindow, services: &Services, sujet: Arc<Mutex<Option<AccountId>>>) {
    f.set_tag_palette(ModelRc::new(VecModel::from(
        PALETTE
            .iter()
            .map(|c| crate::calendar::couleur(c))
            .collect::<Vec<_>>(),
    )));

    // --- La fenêtre de gestion ---
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_tags_requested(move || {
            let Some(f) = faible.upgrade() else { return };
            remplir(&f, &services);
            f.set_tags_error(SharedString::default());
            f.set_tags_new_name(SharedString::default());
            f.set_tags_open(true);
        });
    }
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_tag_created(move |nom| {
            let Some(f) = faible.upgrade() else { return };
            match services
                .store
                .create_account_tag(&nom, couleur_suivante(&services), now())
            {
                Ok(_) => {
                    f.set_tags_error(SharedString::default());
                    f.set_tags_new_name(SharedString::default());
                    remplir(&f, &services);
                    refresh_accounts(&f, &services, &[]);
                }
                Err(e) => f.set_tags_error(message(&e).into()),
            }
        });
    }
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_tag_renamed(move |id, nom| {
            let Some(f) = faible.upgrade() else { return };
            match services.store.rename_account_tag(id as i64, &nom) {
                Ok(()) => {
                    f.set_tags_error(SharedString::default());
                    // The row is updated in place, not rebuilt: rebuilding would take
                    // the cursor out of the field being typed in.
                    let modele = f.get_account_tags();
                    for i in 0..modele.row_count() {
                        if let Some(mut t) = modele.row_data(i) {
                            if t.id == id {
                                t.name = nom.trim().into();
                                modele.set_row_data(i, t);
                            }
                        }
                    }
                    refresh_accounts(&f, &services, &[]);
                }
                Err(e) => f.set_tags_error(message(&e).into()),
            }
        });
    }
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_tag_color_chosen(move |id, i| {
            let Some(f) = faible.upgrade() else { return };
            let Some(hex) = PALETTE.get(i.max(0) as usize) else {
                return;
            };
            let _ = services.store.set_account_tag_color(id as i64, hex);
            remplir(&f, &services);
            refresh_accounts(&f, &services, &[]);
        });
    }
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_tag_deleted(move |id| {
            let Some(f) = faible.upgrade() else { return };
            let _ = services.store.delete_account_tag(id as i64);
            remplir(&f, &services);
            refresh_accounts(&f, &services, &[]);
        });
    }

    // --- Le menu d'un compte ---
    {
        let (services, sujet, faible) = (services.clone(), Arc::clone(&sujet), f.as_weak());
        f.on_account_tag_toggled(move |id| {
            let Some(f) = faible.upgrade() else { return };
            let Some(compte) = *sujet.lock().expect("poisoned account menu") else {
                return;
            };
            let porte = services
                .store
                .account_tag_links()
                .unwrap_or_default()
                .get(&compte)
                .is_some_and(|t| t.contains(&(id as i64)));
            let _ = services.store.set_account_tagged(compte, id as i64, !porte);
            remplir_menu(&f, &services, &sujet);
            refresh_accounts(&f, &services, &[]);
        });
    }
    {
        let (services, sujet, faible) = (services.clone(), Arc::clone(&sujet), f.as_weak());
        f.on_account_tag_search_changed(move |_| {
            if let Some(f) = faible.upgrade() {
                remplir_menu(&f, &services, &sujet);
            }
        });
    }
    {
        let (services, sujet, faible) = (services.clone(), Arc::clone(&sujet), f.as_weak());
        f.on_account_tag_created(move |nom| {
            let Some(f) = faible.upgrade() else { return };
            let Some(compte) = *sujet.lock().expect("poisoned account menu") else {
                return;
            };
            match services
                .store
                .create_account_tag(&nom, couleur_suivante(&services), now())
            {
                Ok(tag) => {
                    let _ = services.store.set_account_tagged(compte, tag, true);
                    f.set_account_tag_search(SharedString::default());
                    remplir_menu(&f, &services, &sujet);
                    refresh_accounts(&f, &services, &[]);
                }
                Err(e) => f.set_status(message(&e).into()),
            }
        });
    }
}

/// The accounts under a tag's title: those carrying it, or for 0 those carrying none.
pub fn accounts_of_tag(services: &Services, tag: i64) -> Vec<AccountId> {
    let liens = services.store.account_tag_links().unwrap_or_default();
    services
        .store
        .accounts()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| match liens.get(&c.id) {
            Some(t) if tag != 0 => t.contains(&tag),
            Some(t) => t.is_empty(),
            None => tag == 0,
        })
        .map(|c| c.id)
        .collect()
}

/// The tag titles of the accounts column: a click shows their accounts' mail, the
/// chevron folds them; and the order of the tags, dragged in their window.
pub fn wire_tag_groups(
    f: &AppWindow,
    services: &Services,
    controller: Arc<crate::controller::Controller>,
) {
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_tag_selected(move |tag| {
            let Some(f) = faible.upgrade() else { return };
            let comptes = accounts_of_tag(&services, tag as i64);
            if comptes.is_empty() {
                return;
            }
            f.set_selected_tag(tag);
            controller.send(crate::controller::Request::FilterAccounts(comptes));
        });
    }
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_tag_fold_toggled(move |tag| {
            let Some(f) = faible.upgrade() else { return };
            let tag = tag as i64;
            crate::settings::update(|s| {
                if let Some(i) = s.folded_tags.iter().position(|t| *t == tag) {
                    s.folded_tags.remove(i);
                } else {
                    s.folded_tags.push(tag);
                }
            });
            refresh_accounts(&f, &services, &[]);
        });
    }
    {
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_tag_moved(move |id, index| {
            let Some(f) = faible.upgrade() else { return };
            let _ = services
                .store
                .move_account_tag(id as i64, index.max(0) as usize);
            remplir(&f, &services);
            refresh_accounts(&f, &services, &[]);
        });
    }
}

/// Ce qu'une erreur de tag dit à l'utilisateur, sans le préfixe technique.
fn message(e: &iris_types::Error) -> String {
    match e {
        iris_types::Error::Config(m) => m.clone(),
        autre => autre.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services() -> (tempfile::TempDir, Services) {
        let dir = tempfile::tempdir().unwrap();
        let s = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("test")),
        )
        .unwrap();
        (dir, s)
    }

    #[test]
    fn the_menu_ticks_the_account_s_tags_and_filters_by_the_search() {
        let (_d, s) = services();
        let compte = s
            .store
            .create_account(
                &iris_store::NewAccount::new(
                    "a@example.com",
                    "imap.example.com",
                    "smtp.example.com",
                ),
                now(),
            )
            .unwrap();
        let clients = s
            .store
            .create_account_tag("Clients", "#4fb286", now())
            .unwrap();
        s.store
            .create_account_tag("Perso", "#e0795b", now())
            .unwrap();
        s.store.set_account_tagged(compte, clients, true).unwrap();

        let tous = menu_tags(&s, compte, "");
        assert_eq!(tous.len(), 2);
        assert!(tous.iter().find(|t| t.name == "Clients").unwrap().checked);
        assert!(!tous.iter().find(|t| t.name == "Perso").unwrap().checked);
        assert_eq!(menu_tags(&s, compte, " per ").len(), 1);
    }

    #[test]
    fn a_tag_s_title_gathers_its_accounts_and_no_tag_the_rest() {
        let (_d, s) = services();
        let compte = |a: &str| {
            s.store
                .create_account(
                    &iris_store::NewAccount::new(a, "imap.example.com", "smtp.example.com"),
                    now(),
                )
                .unwrap()
        };
        let (a, b, c) = (
            compte("a@example.com"),
            compte("b@example.com"),
            compte("c@example.com"),
        );
        let clients = s
            .store
            .create_account_tag("Clients", "#4fb286", now())
            .unwrap();
        s.store.set_account_tagged(a, clients, true).unwrap();
        s.store.set_account_tagged(b, clients, true).unwrap();
        assert_eq!(accounts_of_tag(&s, clients), [a, b]);
        assert_eq!(accounts_of_tag(&s, 0), [c]);
    }

    #[test]
    fn a_new_tag_takes_a_colour_nobody_has_yet() {
        let (_d, s) = services();
        s.store.create_account_tag("A", PALETTE[0], now()).unwrap();
        assert_eq!(couleur_suivante(&s), PALETTE[1]);
    }
}
