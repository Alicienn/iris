//! Iris on Android.
//!
//! Android starts `NativeActivity`, which loads this library and calls
//! [`android_main`]. From there it is the same Iris as on Windows: the same services,
//! the same window (`iris_app::gui::run`), the same screens, laid out for the phone's
//! width by the interface itself. What is Android's own is the platform's answers
//! ([`platform::Android`]): a finger rather than a pointer, the system's bars over the
//! window, the vault's key, links opened by the system.
//!
//! Built for any other system, the crate is empty.

#![cfg(target_os = "android")]

mod platform;

use slint::android::AndroidApp;

/// Where Android hands over to Iris.
#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("Iris"),
    );
    // A panic must leave a line in logcat, not only a closed window.
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));

    let Some(donnees) = app.internal_data_path() else {
        log::error!("Android gave no private folder");
        return;
    };
    if let Err(e) = slint::android::init(app.clone()) {
        log::error!("Slint could not start on Android: {e}");
        return;
    }

    let chemins = iris_app::paths::Paths::under(&donnees);
    iris_app::logging::install(&chemins);
    iris_app::platform::install(Box::new(platform::Android::new(app)));

    let options = iris_app::gui::GuiOptions {
        mailto: None,
        start_in_tray: false,
        instance: None,
    };
    let resultat = iris_app::gui::run(options, || {
        let cle = platform::vault_key(&chemins)?;
        iris_app::services::Services::open(chemins.clone(), Some(cle))
    });
    if let Err(e) = resultat {
        log::error!("Iris stopped: {e}");
    }
}
