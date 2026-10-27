//! What Android answers when Iris asks about the system.

use iris_app::platform::{Platform, UpdateChannel};
use iris_secrets::Secret;
use iris_types::{Error, Result};
use jni::objects::{JObject, JValue};
use slint::android::AndroidApp;

pub struct Android {
    app: AndroidApp,
}

impl Android {
    pub fn new(app: AndroidApp) -> Self {
        Self { app }
    }

    /// How many physical pixels a logical one is: the screen's density over 160.
    fn echelle(&self) -> f32 {
        self.app
            .config()
            .density()
            .map(|dpi| dpi as f32 / 160.0)
            .filter(|e| *e > 0.0)
            .unwrap_or(1.0)
    }
}

impl Platform for Android {
    fn open_url(&self, url: &str) -> Result<()> {
        ouvrir_dans_le_systeme(url).map_err(|e| Error::other(format!("opening the link: {e}")))
    }

    fn open_path(&self, path: &std::path::Path) -> Result<()> {
        Err(Error::other(format!(
            "opening files is not wired on Android yet: {}",
            path.display()
        )))
    }

    fn system_dark(&self) -> bool {
        // Configuration.UI_MODE_NIGHT_YES. Read by its name: the type lives in the `ndk`
        // crate, which android-activity does not hand on.
        format!("{:?}", self.app.config().ui_mode_night()) == "Yes"
    }

    fn number_style(&self) -> (char, char, bool) {
        (',', ' ', true)
    }

    fn touch(&self) -> bool {
        true
    }

    /// The status bar above the content, the gesture bar below it, in logical pixels.
    fn insets(&self) -> (f32, f32) {
        let contenu = self.app.content_rect();
        let hauteur = self
            .app
            .native_window()
            .map(|w| w.height())
            .unwrap_or(contenu.bottom);
        let e = self.echelle();
        (
            contenu.top.max(0) as f32 / e,
            (hauteur - contenu.bottom).max(0) as f32 / e,
        )
    }

    /// Segoe UI is Windows' and cannot be shipped here: the system's own sans-serif.
    fn font_override(&self) -> Option<(String, String)> {
        Some(("sans-serif".into(), "sans-serif".into()))
    }

    fn update_channel(&self) -> UpdateChannel {
        UpdateChannel::Apk
    }

    /// A Wi-Fi drops multicast to save the battery unless an app holds a lock: without
    /// it the other devices' announces never arrive.
    fn multicast(&self, on: bool) {
        if let Err(e) = verrou_multicast(on) {
            tracing::warn!(error = %e, "the Wi-Fi's multicast lock");
        }
    }
}

/// The one multicast lock of the process, held while Iris looks for other devices.
static VERROU: std::sync::Mutex<Option<jni::objects::GlobalRef>> = std::sync::Mutex::new(None);

fn verrou_multicast(on: bool) -> std::result::Result<(), String> {
    let contexte = ndk_context::android_context();
    // SAFETY: the VM and the activity Android gave the process; both live as long as it.
    let vm = unsafe { jni::JavaVM::from_raw(contexte.vm().cast()) }.map_err(|e| e.to_string())?;
    let activite = unsafe { JObject::from_raw(contexte.context().cast()) };
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let mut garde = VERROU.lock().unwrap_or_else(|e| e.into_inner());
    if !on {
        if let Some(v) = garde.take() {
            env.call_method(v.as_obj(), "release", "()V", &[])
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    if garde.is_some() {
        return Ok(());
    }
    let nom = env.new_string("wifi").map_err(|e| e.to_string())?;
    let wifi = env
        .call_method(
            &activite,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&nom)],
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let etiquette = env.new_string("iris").map_err(|e| e.to_string())?;
    let verrou = env
        .call_method(
            &wifi,
            "createMulticastLock",
            "(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;",
            &[JValue::Object(&etiquette)],
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    env.call_method(&verrou, "acquire", "()V", &[])
        .map_err(|e| e.to_string())?;
    *garde = Some(env.new_global_ref(verrou).map_err(|e| e.to_string())?);
    Ok(())
}

/// Hands a web link to the system, as a tap on it in any app would (`ACTION_VIEW`).
fn ouvrir_dans_le_systeme(url: &str) -> std::result::Result<(), String> {
    let contexte = ndk_context::android_context();
    // SAFETY: the VM and the activity Android gave the process; both live as long as it.
    let vm = unsafe { jni::JavaVM::from_raw(contexte.vm().cast()) }.map_err(|e| e.to_string())?;
    let activite = unsafe { JObject::from_raw(contexte.context().cast()) };
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let texte = env.new_string(url).map_err(|e| e.to_string())?;
    let uri = env
        .call_static_method(
            "android/net/Uri",
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&texte)],
        )
        .and_then(|v| v.l())
        .map_err(|e| e.to_string())?;
    let action = env
        .new_string("android.intent.action.VIEW")
        .map_err(|e| e.to_string())?;
    let intention = env
        .new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )
        .map_err(|e| e.to_string())?;
    env.call_method(
        &activite,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&intention)],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The key of Iris's encrypted vault: 32 random bytes, made once and kept in the app's
/// private folder, which no other app can read. (The keystore will wrap it later.)
pub fn vault_key(chemins: &iris_app::paths::Paths) -> Result<Secret> {
    let fichier = chemins.data.join("vault-key");
    if let Ok(cle) = std::fs::read_to_string(&fichier) {
        let cle = cle.trim();
        if cle.len() == 64 {
            return Ok(Secret::new(cle));
        }
    }
    let mut octets = [0u8; 32];
    getrandom::fill(&mut octets).map_err(|e| Error::other(format!("random bytes: {e}")))?;
    let cle = hex::encode(octets);
    std::fs::create_dir_all(&chemins.data)
        .and_then(|_| std::fs::write(&fichier, &cle))
        .map_err(|e| Error::other(format!("writing the vault's key: {e}")))?;
    Ok(Secret::new(cle))
}
