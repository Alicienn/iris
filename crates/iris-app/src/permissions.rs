//! What a phone's system holds back from Iris: its notifications and running behind
//! the screen. Asked once, at the end of the setup (`SetupPermissionsPanel`); shown
//! after that in Settings › Notifications, with the way to Android's own page.
//!
//! A computer asks nothing: `Platform::notifications_allowed` is `None` there, and
//! none of this shows.

use crate::platform;
use crate::services::Services;
use iris_ui::AppWindow;
use slint::ComponentHandle;

/// The file whose presence says the setup's questions were put once.
const ASKED: &str = "permissions-asked";

pub fn wire_permissions(f: &AppWindow, services: &Services) {
    let marque = services.paths.data.join(ASKED);
    let sur_un_telephone = platform::current().notifications_allowed().is_some();
    f.set_ask_permissions(sur_un_telephone && !marque.exists());
    lire(f);

    {
        let faible = f.as_weak();
        f.on_permissions_refresh(move || {
            if let Some(f) = faible.upgrade() {
                lire(&f);
            }
        });
    }
    {
        let faible = f.as_weak();
        f.on_permissions_allow_notifications(move || {
            let faible = faible.clone();
            platform::current().ask_notifications(Box::new(move |oui| {
                let _ = faible.upgrade_in_event_loop(move |f| {
                    lire(&f);
                    if oui {
                        f.set_notifications(true);
                        f.invoke_system_changed();
                        // Allowed: on to the battery's question.
                        if f.get_setup_permissions_step() == 0 {
                            f.set_setup_permissions_step(1);
                            lire(&f);
                        }
                    } else {
                        // Android said no: the page shows the way to its settings.
                        f.set_notifications_refused(true);
                    }
                });
            }));
        });
    }
    // Settings › Updates: the minutes measured, copied to be pasted in a message.
    crate::energy::start();
    {
        let faible = f.as_weak();
        f.on_copy_energy_report(move || {
            let copie = platform::current().clipboard_set(&crate::energy::report());
            if let Some(f) = faible.upgrade() {
                f.set_energy_note(
                    if copie {
                        "Copied: paste it in a message."
                    } else {
                        "It could not be copied."
                    }
                    .into(),
                );
            }
        });
    }
    f.on_permissions_open_notification_settings(|| {
        platform::current().open_notification_settings();
    });
    f.on_permissions_open_background_settings(|| {
        platform::current().open_background_settings();
    });
    {
        let faible = f.as_weak();
        f.on_permissions_finished(move || {
            if let Err(e) = std::fs::write(&marque, b"") {
                tracing::warn!(error = %e, "remembering the setup's questions");
            }
            if let Some(f) = faible.upgrade() {
                f.set_ask_permissions(false);
            }
        });
    }
}

/// Reads what the system allows now into the window.
fn lire(f: &AppWindow) {
    let p = platform::current();
    let notifications = p.notifications_allowed();
    let fond = p.background_allowed();
    f.set_notifications_blocked(notifications == Some(false));
    f.set_background_limited(fond == Some(false));
    f.set_background_allowed(fond != Some(false));
    // Allowed since (back from Android's settings): the refusal's steps go, and the
    // setup's first question, already answered, gives way to the second.
    if notifications == Some(true) {
        f.set_notifications_refused(false);
        if f.get_setup_permissions_open() && f.get_setup_permissions_step() == 0 {
            f.set_setup_permissions_step(1);
        }
    }
}
