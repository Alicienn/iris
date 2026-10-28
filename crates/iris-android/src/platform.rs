//! What Android answers when Iris asks about the system.

use iris_app::platform::{Platform, UpdateChannel};
use iris_secrets::Secret;
use iris_types::{Error, Result};
use jni::objects::{JObject, JString};
use jni::{jni_sig, jni_str, Env, JValue, JavaVM};
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
        let r = avec_activite(|env, activite| {
            env.call_method(
                activite,
                jni_str!("moveTaskToBack"),
                jni_sig!("(Z)Z"),
                &[JValue::Bool(true)],
            )?;
            Ok(())
        });
        if let Err(e) = r {
            tracing::warn!(error = %e, "leaving on Back");
        }
    }

    /// Android's clipboard: a link for a note, a copied cell, what Iris copies itself.
    fn clipboard_set(&self, text: &str) -> bool {
        match presse_papiers(Some(text)) {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(error = %e, "the clipboard");
                false
            }
        }
    }

    fn clipboard_get(&self) -> Option<String> {
        presse_papiers(None).ok().flatten()
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
static VERROU: std::sync::Mutex<Option<jni::refs::Global<JObject<'static>>>> =
    std::sync::Mutex::new(None);

fn verrou_multicast(on: bool) -> std::result::Result<(), String> {
    avec_activite(|env, activite| {
        let mut garde = VERROU.lock().unwrap_or_else(|e| e.into_inner());
        if !on {
            if let Some(v) = garde.take() {
                env.call_method(v.as_obj(), jni_str!("release"), jni_sig!("()V"), &[])?;
            }
            return Ok(());
        }
        if garde.is_some() {
            return Ok(());
        }
        let nom = env.new_string("wifi")?;
        let wifi = env
            .call_method(
                activite,
                jni_str!("getSystemService"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                &[JValue::Object(&nom)],
            )?
            .l()?;
        let etiquette = env.new_string("iris")?;
        let verrou = env
            .call_method(
                &wifi,
                jni_str!("createMulticastLock"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;"),
                &[JValue::Object(&etiquette)],
            )?
            .l()?;
        env.call_method(&verrou, jni_str!("acquire"), jni_sig!("()V"), &[])?;
        *garde = Some(env.new_global_ref(&verrou)?);
        Ok(())
    })
}

/// Runs `f` with the thread attached to Java and the activity Android gave the
/// process. jni 0.22 attaches for the call, returns a Java exception thrown in it as
/// an error, and drops the local references made there.
fn avec_activite<T>(
    f: impl FnOnce(&mut Env, &JObject) -> jni::errors::Result<T>,
) -> std::result::Result<T, String> {
    let contexte = ndk_context::android_context();
    // SAFETY: the VM Android gave the process; Slint's backend has made it the
    // singleton already, this only covers the case where it has not.
    let vm =
        JavaVM::singleton().unwrap_or_else(|_| unsafe { JavaVM::from_raw(contexte.vm().cast()) });
    vm.attach_current_thread(|env| {
        // SAFETY: the activity Android gave the process, alive as long as it.
        let activite = unsafe { JObject::from_raw(env, contexte.context().cast()) };
        f(env, &activite)
    })
    .map_err(|e: jni::errors::Error| e.to_string())
}

/// Hands a web link to the system, as a tap on it in any app would (`ACTION_VIEW`).
fn ouvrir_dans_le_systeme(url: &str) -> std::result::Result<(), String> {
    avec_activite(|env, activite| {
        let texte = env.new_string(url)?;
        let uri = env
            .call_static_method(
                jni_str!("android/net/Uri"),
                jni_str!("parse"),
                jni_sig!("(Ljava/lang/String;)Landroid/net/Uri;"),
                &[JValue::Object(&texte)],
            )?
            .l()?;
        let action = env.new_string("android.intent.action.VIEW")?;
        let intention = env.new_object(
            jni_str!("android/content/Intent"),
            jni_sig!("(Ljava/lang/String;Landroid/net/Uri;)V"),
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        env.call_method(
            activite,
            jni_str!("startActivity"),
            jni_sig!("(Landroid/content/Intent;)V"),
            &[JValue::Object(&intention)],
        )?;
        Ok(())
    })
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
    use jni::objects::{JByteArray, JObject};
    use jni::{jni_sig, jni_str, Env, JValue};

    const ALIAS: &str = "iris-vault";
    const TRANSFORMATION: &str = "AES/GCM/NoPadding";

    fn env_et<T>(f: impl FnOnce(&mut Env) -> jni::errors::Result<T>) -> Result<T, String> {
        // A Java exception thrown inside comes back as an error: jni catches it.
        super::avec_activite(|env, _| f(env))
    }

    /// The Keystore's key, made the first time.
    fn key<'a>(env: &mut Env<'a>) -> jni::errors::Result<JObject<'a>> {
        let nom = env.new_string("AndroidKeyStore")?;
        let magasin = env
            .call_static_method(
                jni_str!("java/security/KeyStore"),
                jni_str!("getInstance"),
                jni_sig!("(Ljava/lang/String;)Ljava/security/KeyStore;"),
                &[JValue::Object(&nom)],
            )?
            .l()?;
        let rien = JObject::null();
        env.call_method(
            &magasin,
            jni_str!("load"),
            jni_sig!("(Ljava/security/KeyStore$LoadStoreParameter;)V"),
            &[JValue::Object(&rien)],
        )?;
        let alias = env.new_string(ALIAS)?;
        let existe = env
            .call_method(
                &magasin,
                jni_str!("containsAlias"),
                jni_sig!("(Ljava/lang/String;)Z"),
                &[JValue::Object(&alias)],
            )?
            .z()?;
        if existe {
            return env
                .call_method(
                    &magasin,
                    jni_str!("getKey"),
                    jni_sig!("(Ljava/lang/String;[C)Ljava/security/Key;"),
                    &[JValue::Object(&alias), JValue::Object(&rien)],
                )?
                .l();
        }
        // PURPOSE_ENCRYPT | PURPOSE_DECRYPT, GCM, no padding, 256 bits.
        let constructeur = env.new_object(
            jni_str!("android/security/keystore/KeyGenParameterSpec$Builder"),
            jni_sig!("(Ljava/lang/String;I)V"),
            &[JValue::Object(&alias), JValue::Int(1 | 2)],
        )?;
        let gcm = env.new_string("GCM")?;
        let modes = env.new_object_array(1, jni_str!("java/lang/String"), &gcm)?;
        env.call_method(
            &constructeur,
            jni_str!("setBlockModes"),
            jni_sig!(
                "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;"
            ),
            &[JValue::Object(&modes)],
        )?;
        let sans = env.new_string("NoPadding")?;
        let bourrages = env.new_object_array(1, jni_str!("java/lang/String"), &sans)?;
        env.call_method(
            &constructeur,
            jni_str!("setEncryptionPaddings"),
            jni_sig!(
                "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;"
            ),
            &[JValue::Object(&bourrages)],
        )?;
        env.call_method(
            &constructeur,
            jni_str!("setKeySize"),
            jni_sig!("(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;"),
            &[JValue::Int(256)],
        )?;
        let spec = env
            .call_method(
                &constructeur,
                jni_str!("build"),
                jni_sig!("()Landroid/security/keystore/KeyGenParameterSpec;"),
                &[],
            )?
            .l()?;
        let aes = env.new_string("AES")?;
        let generateur = env
            .call_static_method(
                jni_str!("javax/crypto/KeyGenerator"),
                jni_str!("getInstance"),
                jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;"),
                &[JValue::Object(&aes), JValue::Object(&nom)],
            )?
            .l()?;
        env.call_method(
            &generateur,
            jni_str!("init"),
            jni_sig!("(Ljava/security/spec/AlgorithmParameterSpec;)V"),
            &[JValue::Object(&spec)],
        )?;
        env.call_method(
            &generateur,
            jni_str!("generateKey"),
            jni_sig!("()Ljavax/crypto/SecretKey;"),
            &[],
        )?
        .l()
    }

    fn cipher<'a>(env: &mut Env<'a>) -> jni::errors::Result<JObject<'a>> {
        let t = env.new_string(TRANSFORMATION)?;
        env.call_static_method(
            jni_str!("javax/crypto/Cipher"),
            jni_str!("getInstance"),
            jni_sig!("(Ljava/lang/String;)Ljavax/crypto/Cipher;"),
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
                jni_str!("init"),
                jni_sig!("(ILjava/security/Key;)V"),
                &[JValue::Int(1), JValue::Object(&cle)],
            )?;
            let iv = env
                .call_method(&c, jni_str!("getIV"), jni_sig!("()[B"), &[])?
                .l()?;
            let iv = env.cast_local::<JByteArray>(iv)?;
            let iv = env.convert_byte_array(&iv)?;
            let entree = env.byte_array_from_slice(clair)?;
            let sortie = env
                .call_method(
                    &c,
                    jni_str!("doFinal"),
                    jni_sig!("([B)[B"),
                    &[JValue::Object(&entree)],
                )?
                .l()?;
            let sortie = env.cast_local::<JByteArray>(sortie)?;
            let sortie = env.convert_byte_array(&sortie)?;
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
                jni_str!("javax/crypto/spec/GCMParameterSpec"),
                jni_sig!("(I[B)V"),
                &[JValue::Int(128), JValue::Object(&iv)],
            )?;
            // Cipher.DECRYPT_MODE.
            env.call_method(
                &c,
                jni_str!("init"),
                jni_sig!("(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V"),
                &[JValue::Int(2), JValue::Object(&cle), JValue::Object(&spec)],
            )?;
            let entree = env.byte_array_from_slice(&scelle[12..])?;
            let sortie = env
                .call_method(
                    &c,
                    jni_str!("doFinal"),
                    jni_sig!("([B)[B"),
                    &[JValue::Object(&entree)],
                )?
                .l()?;
            let sortie = env.cast_local::<JByteArray>(sortie)?;
            env.convert_byte_array(&sortie)
        })
    }
}

/// Writes `text` on the clipboard (`ClipboardManager.setPrimaryClip`), or with `None`
/// reads what is there as text.
fn presse_papiers(text: Option<&str>) -> std::result::Result<Option<String>, String> {
    avec_activite(|env, activite| {
        let nom = env.new_string("clipboard")?;
        let gestionnaire = env
            .call_method(
                activite,
                jni_str!("getSystemService"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                &[JValue::Object(&nom)],
            )?
            .l()?;
        if let Some(text) = text {
            let etiquette = env.new_string("Iris")?;
            let contenu = env.new_string(text)?;
            let clip = env
                .call_static_method(
                    jni_str!("android/content/ClipData"),
                    jni_str!("newPlainText"),
                    jni_sig!(
                        "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;"
                    ),
                    &[JValue::Object(&etiquette), JValue::Object(&contenu)],
                )?
                .l()?;
            env.call_method(
                &gestionnaire,
                jni_str!("setPrimaryClip"),
                jni_sig!("(Landroid/content/ClipData;)V"),
                &[JValue::Object(&clip)],
            )?;
            return Ok(None);
        }
        let clip = env
            .call_method(
                &gestionnaire,
                jni_str!("getPrimaryClip"),
                jni_sig!("()Landroid/content/ClipData;"),
                &[],
            )?
            .l()?;
        if clip.is_null() {
            return Ok(None);
        }
        let element = env
            .call_method(
                &clip,
                jni_str!("getItemAt"),
                jni_sig!("(I)Landroid/content/ClipData$Item;"),
                &[JValue::Int(0)],
            )?
            .l()?;
        let texte = env
            .call_method(
                &element,
                jni_str!("coerceToText"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/CharSequence;"),
                &[JValue::Object(activite)],
            )?
            .l()?;
        if texte.is_null() {
            return Ok(None);
        }
        let chaine = env
            .call_method(
                &texte,
                jni_str!("toString"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        let chaine = env.cast_local::<JString>(chaine)?;
        Ok(Some(chaine.try_to_string(env)?))
    })
}
