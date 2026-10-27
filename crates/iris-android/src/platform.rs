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

    /// Back on the first page: Iris goes behind, as Android's own apps do, and keeps
    /// its state (`moveTaskToBack`, not `finish`).
    fn leave(&self) {
        let r = (|| -> std::result::Result<(), String> {
            let contexte = ndk_context::android_context();
            // SAFETY: the VM and the activity Android gave the process.
            let vm = unsafe { jni::JavaVM::from_raw(contexte.vm().cast()) }
                .map_err(|e| e.to_string())?;
            let activite = unsafe { JObject::from_raw(contexte.context().cast()) };
            let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
            env.call_method(&activite, "moveTaskToBack", "(Z)Z", &[JValue::Bool(1)])
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(e) = r {
            tracing::warn!(error = %e, "leaving on Back");
        }
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
/// private folder, which no other app can read.
///
/// The key is kept wrapped by an AES-256-GCM key of the Android Keystore
/// (`vault-key.sealed`: the 12-byte IV, then the sealed key), which never leaves the
/// phone's secure hardware: a copy of the app's files elsewhere does not open the
/// vault. A key kept in clear by the first preview (`vault-key`) is sealed and removed.
pub fn vault_key(chemins: &iris_app::paths::Paths) -> Result<Secret> {
    let scelle = chemins.data.join("vault-key.sealed");
    let clair = chemins.data.join("vault-key");
    let erreur = |e: String| Error::other(format!("the vault's key: {e}"));
    if let Ok(octets) = std::fs::read(&scelle) {
        let cle = keystore::open(&octets).map_err(erreur)?;
        return Ok(Secret::new(hex::encode(cle)));
    }
    let cle: Vec<u8> = match std::fs::read_to_string(&clair) {
        Ok(t) if t.trim().len() == 64 => {
            hex::decode(t.trim()).map_err(|e| erreur(e.to_string()))?
        }
        _ => {
            let mut octets = [0u8; 32];
            getrandom::fill(&mut octets).map_err(|e| erreur(e.to_string()))?;
            octets.to_vec()
        }
    };
    let ferme = keystore::seal(&cle).map_err(erreur)?;
    std::fs::create_dir_all(&chemins.data)
        .and_then(|_| std::fs::write(&scelle, &ferme))
        .map_err(|e| erreur(e.to_string()))?;
    let _ = std::fs::remove_file(&clair);
    Ok(Secret::new(hex::encode(cle)))
}

/// The Android Keystore, over JNI.
mod keystore {
    use jni::objects::{JObject, JValue};
    use jni::JNIEnv;

    const ALIAS: &str = "iris-vault";
    const TRANSFORMATION: &str = "AES/GCM/NoPadding";

    fn env_et<T>(f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<T>) -> Result<T, String> {
        let contexte = ndk_context::android_context();
        // SAFETY: the VM Android gave the process; it lives as long as it.
        let vm =
            unsafe { jni::JavaVM::from_raw(contexte.vm().cast()) }.map_err(|e| e.to_string())?;
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let r = f(&mut env);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        r.map_err(|e| e.to_string())
    }

    /// The Keystore's key, made the first time.
    fn key<'a>(env: &mut JNIEnv<'a>) -> jni::errors::Result<JObject<'a>> {
        let nom = env.new_string("AndroidKeyStore")?;
        let magasin = env
            .call_static_method(
                "java/security/KeyStore",
                "getInstance",
                "(Ljava/lang/String;)Ljava/security/KeyStore;",
                &[JValue::Object(&nom)],
            )?
            .l()?;
        let rien = JObject::null();
        env.call_method(
            &magasin,
            "load",
            "(Ljava/security/KeyStore$LoadStoreParameter;)V",
            &[JValue::Object(&rien)],
        )?;
        let alias = env.new_string(ALIAS)?;
        let existe = env
            .call_method(
                &magasin,
                "containsAlias",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&alias)],
            )?
            .z()?;
        if existe {
            return env
                .call_method(
                    &magasin,
                    "getKey",
                    "(Ljava/lang/String;[C)Ljava/security/Key;",
                    &[JValue::Object(&alias), JValue::Object(&rien)],
                )?
                .l();
        }
        // PURPOSE_ENCRYPT | PURPOSE_DECRYPT, GCM, no padding, 256 bits.
        let constructeur = env.new_object(
            "android/security/keystore/KeyGenParameterSpec$Builder",
            "(Ljava/lang/String;I)V",
            &[JValue::Object(&alias), JValue::Int(1 | 2)],
        )?;
        let gcm = env.new_string("GCM")?;
        let modes = env.new_object_array(1, "java/lang/String", &gcm)?;
        env.call_method(
            &constructeur,
            "setBlockModes",
            "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            &[JValue::Object(&modes)],
        )?;
        let sans = env.new_string("NoPadding")?;
        let bourrages = env.new_object_array(1, "java/lang/String", &sans)?;
        env.call_method(
            &constructeur,
            "setEncryptionPaddings",
            "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            &[JValue::Object(&bourrages)],
        )?;
        env.call_method(
            &constructeur,
            "setKeySize",
            "(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            &[JValue::Int(256)],
        )?;
        let spec = env
            .call_method(
                &constructeur,
                "build",
                "()Landroid/security/keystore/KeyGenParameterSpec;",
                &[],
            )?
            .l()?;
        let aes = env.new_string("AES")?;
        let generateur = env
            .call_static_method(
                "javax/crypto/KeyGenerator",
                "getInstance",
                "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
                &[JValue::Object(&aes), JValue::Object(&nom)],
            )?
            .l()?;
        env.call_method(
            &generateur,
            "init",
            "(Ljava/security/spec/AlgorithmParameterSpec;)V",
            &[JValue::Object(&spec)],
        )?;
        env.call_method(
            &generateur,
            "generateKey",
            "()Ljavax/crypto/SecretKey;",
            &[],
        )?
        .l()
    }

    fn cipher<'a>(env: &mut JNIEnv<'a>) -> jni::errors::Result<JObject<'a>> {
        let t = env.new_string(TRANSFORMATION)?;
        env.call_static_method(
            "javax/crypto/Cipher",
            "getInstance",
            "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
            &[JValue::Object(&t)],
        )?
        .l()
    }

    /// The IV (12 bytes), then the sealed bytes.
    pub fn seal(clair: &[u8]) -> Result<Vec<u8>, String> {
        env_et(|env| {
            let cle = key(env)?;
            let c = cipher(env)?;
            // Cipher.ENCRYPT_MODE: the Keystore picks the IV.
            env.call_method(
                &c,
                "init",
                "(ILjava/security/Key;)V",
                &[JValue::Int(1), JValue::Object(&cle)],
            )?;
            let iv = env.call_method(&c, "getIV", "()[B", &[])?.l()?;
            let iv = env.convert_byte_array(jni::objects::JByteArray::from(iv))?;
            let entree = env.byte_array_from_slice(clair)?;
            let sortie = env
                .call_method(&c, "doFinal", "([B)[B", &[JValue::Object(&entree)])?
                .l()?;
            let sortie = env.convert_byte_array(jni::objects::JByteArray::from(sortie))?;
            let mut v = iv;
            v.extend(sortie);
            Ok(v)
        })
    }

    pub fn open(scelle: &[u8]) -> Result<Vec<u8>, String> {
        if scelle.len() < 12 + 16 {
            return Err("a sealed key too short".into());
        }
        env_et(|env| {
            let cle = key(env)?;
            let c = cipher(env)?;
            let iv = env.byte_array_from_slice(&scelle[..12])?;
            let spec = env.new_object(
                "javax/crypto/spec/GCMParameterSpec",
                "(I[B)V",
                &[JValue::Int(128), JValue::Object(&iv)],
            )?;
            // Cipher.DECRYPT_MODE.
            env.call_method(
                &c,
                "init",
                "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[JValue::Int(2), JValue::Object(&cle), JValue::Object(&spec)],
            )?;
            let entree = env.byte_array_from_slice(&scelle[12..])?;
            let sortie = env
                .call_method(&c, "doFinal", "([B)[B", &[JValue::Object(&entree)])?
                .l()?;
            env.convert_byte_array(jni::objects::JByteArray::from(sortie))
        })
    }
}
