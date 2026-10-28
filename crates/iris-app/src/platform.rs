//! Ce que le système d'exploitation doit savoir d'Iris.
//!
//! Trois choses, et toutes les trois passent par le registre de Windows :
//!
//! - **`mailto:`** — pour qu'une adresse cliquée dans un navigateur ouvre Iris. Il ne
//!   suffit pas de déclarer un gestionnaire : Windows ne propose une application comme
//!   client de courrier par défaut que si elle est inscrite aux trois endroits à la
//!   fois — `Clients\Mail`, `RegisteredApplications`, et sa propre clé `Capabilities`.
//!   Deux sur trois ne donnent rien du tout, ce qui rend la chose facile à croire
//!   faite et impossible à déboguer.
//! - **le démarrage à l'ouverture de session** — une valeur sous `Run`, et rien de
//!   plus. Pas de tâche planifiée : elle demanderait des droits d'administrateur pour
//!   une préférence qui appartient à l'utilisateur.
//! - **l'`AppUserModelID`** — l'identité sous laquelle les notifications s'affichent.
//!   Sans lui, une bulle Windows apparaît au nom du processus hôte, ou n'apparaît pas.
//!
//! Tout est écrit sous `HKEY_CURRENT_USER`. Jamais sous `HKEY_LOCAL_MACHINE` : celui-ci
//! exigerait une élévation de privilèges pour dire à Windows quelle application ouvre
//! un lien, ce qui est hors de proportion, et imposerait le choix aux autres comptes
//! de la machine.
//!
//! Chaque écriture est réversible, et la désinscription est testée au même titre que
//! l'inscription : une application qui sait s'installer et pas se retirer laisse
//! derrière elle un système qui ment sur ce qu'il contient.

/// L'identité de l'application auprès de Windows.
///
/// Une constante, parce que Windows la retient : la changer d'une version à l'autre
/// laisserait derrière elle des inscriptions orphelines pointant vers un nom que plus
/// rien ne revendique.
pub const APP_ID: &str = "Iris.Mail";

/// Le nom sous lequel Windows la propose à l'utilisateur.
pub const APP_NAME: &str = "Iris";

#[cfg(windows)]
mod windows_impl {
    use super::{APP_ID, APP_NAME};
    use iris_types::{Error, Result};

    /// L'état des trois inscriptions.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Registration {
        /// Iris est proposée par Windows comme client de courrier.
        pub mailto: bool,
        /// Iris démarre à l'ouverture de session.
        pub start_at_login: bool,
    }

    /// Où en sont les inscriptions.
    pub fn status() -> Registration {
        Registration {
            mailto: reg::exists(reg::HKCU, &format!(r"Software\Clients\Mail\{APP_NAME}")),
            start_at_login: reg::has_value(
                reg::HKCU,
                r"Software\Microsoft\Windows\CurrentVersion\Run",
                APP_NAME,
            ),
        }
    }

    /// Le chemin de l'exécutable en cours, entre guillemets et prêt pour une commande.
    fn commande(argument: &str) -> Result<String> {
        let exe = std::env::current_exe()
            .map_err(|e| Error::other(format!("chemin de l'exécutable : {e}")))?;
        let exe = exe.to_string_lossy();
        Ok(if argument.is_empty() {
            format!("\"{exe}\"")
        } else {
            format!("\"{exe}\" {argument}")
        })
    }

    /// Inscrit Iris comme client de courrier possible.
    ///
    /// « Possible », pas « par défaut » : Windows réserve le choix du défaut à
    /// l'utilisateur, dans ses propres réglages, et une application qui essaierait de
    /// se l'attribuer serait bloquée — à juste titre. Ce que fait cette fonction est
    /// de rendre Iris **choisissable**, ce qu'elle n'était pas.
    pub fn register_mailto() -> Result<()> {
        let ouvrir = commande("\"%1\"")?;
        let icone = format!("{},0", commande("")?.trim_matches('"'));

        // 1. La classe de protocole : ce qui sait ouvrir un « mailto: ».
        let classe = format!(r"Software\Classes\{APP_ID}.Mailto");
        reg::write(reg::HKCU, &classe, "", "Iris URL")?;
        reg::write(reg::HKCU, &classe, "URL Protocol", "")?;
        reg::write(reg::HKCU, &format!(r"{classe}\DefaultIcon"), "", &icone)?;
        reg::write(
            reg::HKCU,
            &format!(r"{classe}\shell\open\command"),
            "",
            &ouvrir,
        )?;

        // 2. Les capacités : ce que l'application déclare savoir faire. C'est cette
        //    clé que lit l'écran « Applications par défaut ».
        let client = format!(r"Software\Clients\Mail\{APP_NAME}");
        reg::write(reg::HKCU, &client, "", APP_NAME)?;
        reg::write(
            reg::HKCU,
            &format!(r"{client}\shell\open\command"),
            "",
            &commande("")?,
        )?;
        reg::write(reg::HKCU, &format!(r"{client}\DefaultIcon"), "", &icone)?;

        let capacites = format!(r"{client}\Capabilities");
        reg::write(reg::HKCU, &capacites, "ApplicationName", APP_NAME)?;
        reg::write(
            reg::HKCU,
            &capacites,
            "ApplicationDescription",
            "A mail client for people with more than one mailbox",
        )?;
        reg::write(
            reg::HKCU,
            &format!(r"{capacites}\URLAssociations"),
            "mailto",
            &format!("{APP_ID}.Mailto"),
        )?;

        // 3. L'annuaire : sans cette ligne, les capacités ci-dessus ne sont lues par
        //    personne, et l'application n'apparaît nulle part. C'est l'étape qu'on
        //    oublie, et celle qui fait croire que les deux autres n'ont pas marché.
        reg::write(
            reg::HKCU,
            r"Software\RegisteredApplications",
            APP_NAME,
            &format!(r"Software\Clients\Mail\{APP_NAME}\Capabilities"),
        )?;

        Ok(())
    }

    /// Retire tout ce que `register_mailto` a écrit.
    pub fn unregister_mailto() -> Result<()> {
        reg::delete_value(reg::HKCU, r"Software\RegisteredApplications", APP_NAME);
        reg::delete_tree(reg::HKCU, &format!(r"Software\Clients\Mail\{APP_NAME}"));
        reg::delete_tree(reg::HKCU, &format!(r"Software\Classes\{APP_ID}.Mailto"));
        Ok(())
    }

    /// Whether Windows asks applications to be dark (Settings › Personalisation ›
    /// Colours › "Choose your app mode"). Absent, as on older systems: light.
    pub fn system_dark() -> bool {
        reg::read_dword(
            reg::HKCU,
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "AppsUseLightTheme",
        ) == Some(0)
    }

    /// How the user writes numbers and dates (Settings › Time & language › Region):
    /// the decimal sign, the thousands sign, and whether the day comes first.
    pub fn number_style() -> (char, char, bool) {
        const REGION: &str = r"Control Panel\International";
        let signe = |nom: &str, defaut: char| {
            reg::read_string(reg::HKCU, REGION, nom)
                .and_then(|s| s.chars().next())
                .unwrap_or(defaut)
        };
        let jour_avant = reg::read_string(reg::HKCU, REGION, "sShortDate")
            .is_none_or(|f| f.to_ascii_lowercase().starts_with('d'));
        (signe("sDecimal", '.'), signe("sThousand", ','), jour_avant)
    }

    /// Démarrer, ou non, à l'ouverture de session.
    ///
    /// Une valeur sous `Run`, et rien d'autre. Une tâche planifiée demanderait une
    /// élévation de privilèges pour une préférence qui n'engage que cet utilisateur,
    /// et survivrait à la désinstallation.
    pub fn set_start_at_login(enabled: bool) -> Result<()> {
        const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

        if enabled {
            // `--tray` : démarrer à l'ouverture de session veut dire se tenir prêt,
            // pas ouvrir une fenêtre par-dessus ce que l'utilisateur est en train de
            // faire dans les premières secondes de sa session.
            reg::write(reg::HKCU, RUN, APP_NAME, &commande("--tray")?)
        } else {
            reg::delete_value(reg::HKCU, RUN, APP_NAME);
            Ok(())
        }
    }

    /// Les appels au registre, réduits à ce dont nous avons besoin.
    ///
    /// Quatre fonctions et pas une bibliothèque : ce module écrit une dizaine de
    /// valeurs de chaînes sous une seule ruche, et une caisse de plus dans l'arbre de
    /// dépendances pour cela serait payer cher un confort qu'on n'utiliserait pas.
    mod reg {
        use iris_types::{Error, Result};
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::ERROR_SUCCESS;
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW,
            RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE,
            REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
        };

        pub const HKCU: HKEY = HKEY_CURRENT_USER;

        /// Une chaîne large terminée par un zéro, comme l'API les attend.
        fn wide(s: &str) -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        }

        #[allow(unsafe_code)]
        pub fn write(hive: HKEY, path: &str, name: &str, value: &str) -> Result<()> {
            let chemin = wide(path);
            let nom = wide(name);
            let contenu = wide(value);
            let mut cle: HKEY = std::ptr::null_mut();

            // SÛRETÉ : les trois tampons vivent jusqu'à la fin de la fonction, et
            // `cle` est refermée sur tous les chemins de sortie.
            let ouverture = unsafe {
                RegCreateKeyExW(
                    hive,
                    chemin.as_ptr(),
                    0,
                    std::ptr::null_mut(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_WRITE,
                    std::ptr::null(),
                    &mut cle,
                    std::ptr::null_mut(),
                )
            };
            if ouverture != ERROR_SUCCESS {
                return Err(Error::other(format!(
                    "registre : impossible d'ouvrir « {path} » ({ouverture})"
                )));
            }

            let ecriture = unsafe {
                RegSetValueExW(
                    cle,
                    if name.is_empty() {
                        std::ptr::null()
                    } else {
                        nom.as_ptr()
                    },
                    0,
                    REG_SZ,
                    contenu.as_ptr().cast(),
                    (contenu.len() * 2) as u32,
                )
            };
            unsafe { RegCloseKey(cle) };

            if ecriture != ERROR_SUCCESS {
                return Err(Error::other(format!(
                    "registre : écriture de « {path}\\{name} » refusée ({ecriture})"
                )));
            }
            Ok(())
        }

        /// La valeur est-elle présente ?
        ///
        /// Sa présence, pas son contenu : les deux seules choses qu'on demande au
        /// registre ici sont « est-ce inscrit » et « ne l'est plus », et lire la
        /// chaîne pour la jeter demanderait un second appel et un tampon.
        #[allow(unsafe_code)]
        pub fn has_value(hive: HKEY, path: &str, name: &str) -> bool {
            let chemin = wide(path);
            let nom = wide(name);
            let mut cle: HKEY = std::ptr::null_mut();

            // SÛRETÉ : même contrat que `write`.
            if unsafe { RegOpenKeyExW(hive, chemin.as_ptr(), 0, KEY_READ, &mut cle) }
                != ERROR_SUCCESS
            {
                return false;
            }

            let mut taille: u32 = 0;
            let interroge = unsafe {
                RegQueryValueExW(
                    cle,
                    nom.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut taille,
                )
            };
            unsafe { RegCloseKey(cle) };

            interroge == ERROR_SUCCESS
        }

        /// A number (`REG_DWORD`), if the value is there and is one.
        #[allow(unsafe_code)]
        pub fn read_dword(hive: HKEY, path: &str, name: &str) -> Option<u32> {
            let chemin = wide(path);
            let nom = wide(name);
            let mut cle: HKEY = std::ptr::null_mut();

            // SÛRETÉ : même contrat que `write` ; `valeur` et `taille` vivent jusqu'au
            // retour, et le tampon fait exactement les quatre octets annoncés.
            if unsafe { RegOpenKeyExW(hive, chemin.as_ptr(), 0, KEY_READ, &mut cle) }
                != ERROR_SUCCESS
            {
                return None;
            }
            let mut valeur: u32 = 0;
            let mut taille: u32 = 4;
            let mut genre: u32 = 0;
            let lu = unsafe {
                RegQueryValueExW(
                    cle,
                    nom.as_ptr(),
                    std::ptr::null(),
                    &mut genre,
                    (&mut valeur as *mut u32).cast(),
                    &mut taille,
                )
            };
            unsafe { RegCloseKey(cle) };
            (lu == ERROR_SUCCESS && genre == REG_DWORD).then_some(valeur)
        }

        pub fn exists(hive: HKEY, path: &str) -> bool {
            open_exists(hive, path)
        }

        #[allow(unsafe_code)]
        fn open_exists(hive: HKEY, path: &str) -> bool {
            let chemin = wide(path);
            let mut cle: HKEY = std::ptr::null_mut();
            // SÛRETÉ : `chemin` vit jusqu'au retour ; la clé est refermée si elle
            // s'est ouverte.
            let ouverte = unsafe { RegOpenKeyExW(hive, chemin.as_ptr(), 0, KEY_READ, &mut cle) }
                == ERROR_SUCCESS;
            if ouverte {
                unsafe { RegCloseKey(cle) };
            }
            ouverte
        }

        #[allow(unsafe_code)]
        pub fn delete_value(hive: HKEY, path: &str, name: &str) {
            let chemin = wide(path);
            let nom = wide(name);
            let mut cle: HKEY = std::ptr::null_mut();

            // SÛRETÉ : idem. L'échec est ignoré : ce qui n'existe pas est déjà retiré.
            if unsafe { RegOpenKeyExW(hive, chemin.as_ptr(), 0, KEY_WRITE, &mut cle) }
                == ERROR_SUCCESS
            {
                unsafe {
                    RegDeleteValueW(cle, nom.as_ptr());
                    RegCloseKey(cle);
                }
            }
        }

        /// A text (`REG_SZ`), if the value is there and is one.
        #[allow(unsafe_code)]
        pub fn read_string(hive: HKEY, path: &str, name: &str) -> Option<String> {
            let chemin = wide(path);
            let nom = wide(name);
            let mut cle: HKEY = std::ptr::null_mut();
            // SÛRETÉ : même contrat que `read_dword` ; le tampon fait les octets annoncés.
            if unsafe { RegOpenKeyExW(hive, chemin.as_ptr(), 0, KEY_READ, &mut cle) }
                != ERROR_SUCCESS
            {
                return None;
            }
            let mut tampon = [0u16; 64];
            let mut taille: u32 = (tampon.len() * 2) as u32;
            let mut genre: u32 = 0;
            let lu = unsafe {
                RegQueryValueExW(
                    cle,
                    nom.as_ptr(),
                    std::ptr::null(),
                    &mut genre,
                    tampon.as_mut_ptr().cast(),
                    &mut taille,
                )
            };
            unsafe { RegCloseKey(cle) };
            if lu != ERROR_SUCCESS || genre != REG_SZ {
                return None;
            }
            let n = (taille as usize / 2).min(tampon.len());
            let texte = String::from_utf16_lossy(&tampon[..n]);
            Some(texte.trim_end_matches('\0').to_string())
        }

        #[allow(unsafe_code)]
        pub fn delete_tree(hive: HKEY, path: &str) {
            let chemin = wide(path);
            // SÛRETÉ : `chemin` vit jusqu'au retour. Un échec veut dire que l'arbre
            // n'était pas là, ce qui est le résultat recherché.
            unsafe {
                RegDeleteTreeW(hive, chemin.as_ptr());
            }
        }
    }
}

#[cfg(not(windows))]
mod windows_impl {
    use iris_types::Result;

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct Registration {
        pub mailto: bool,
        pub start_at_login: bool,
    }

    pub fn status() -> Registration {
        Registration::default()
    }
    pub fn register_mailto() -> Result<()> {
        Ok(())
    }
    pub fn unregister_mailto() -> Result<()> {
        Ok(())
    }
    pub fn set_start_at_login(_enabled: bool) -> Result<()> {
        Ok(())
    }
    pub fn system_dark() -> bool {
        false
    }
    pub fn number_style() -> (char, char, bool) {
        ('.', ',', false)
    }
}

pub use windows_impl::{
    register_mailto, set_start_at_login, status, unregister_mailto, Registration,
};

/// The system shows its dark appearance.
pub fn system_dark() -> bool {
    current().system_dark()
}

/// The decimal separator, the thousands separator, and whether the day comes first.
pub fn number_style() -> (char, char, bool) {
    current().number_style()
}

/// Ouvre un fichier avec l'application que le système lui associe.
///
/// Enregistrer une pièce jointe puis aller la chercher dans l'explorateur fait trois
/// gestes là où tout autre client en demande un. Ce qui est ouvert est un fichier que
/// nous venons d'écrire nous-mêmes, à un chemin que nous avons choisi — jamais une
/// adresse ni une commande venue du message.
///
/// C'est le système qui décide avec quoi : Iris ne connaît aucun format et n'a pas à
/// deviner. Un `.exe` reçu en pièce jointe est ouvert comme le ferait un double-clic
/// dans l'explorateur, avec les mêmes garde-fous — SmartScreen, la marque de
/// provenance — et c'est le bon endroit pour cette décision, parce qu'elle est déjà
/// prise là par quelqu'un dont c'est le métier.
pub fn open_path(path: &std::path::Path) -> iris_types::Result<()> {
    current().open_path(path)
}

fn desktop_open_path(path: &std::path::Path) -> iris_types::Result<()> {
    #[cfg(windows)]
    {
        shell_open(path.as_os_str())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(iris_types::Error::other(
            "opening files is only wired up on Windows",
        ))
    }
}

/// Opens a web link (a video call, a link in a message) in the browser, or the app
/// that claims it.
///
/// Only `https://` and `http://` links, whole: anything else is refused rather than
/// handed to the system, which would run a file or a program just as readily.
pub fn open_url(url: &str) -> iris_types::Result<()> {
    let url = url.trim();
    let web = ["https://", "http://"].iter().any(|p| {
        url.len() > p.len()
            && url
                .get(..p.len())
                .is_some_and(|debut| debut.eq_ignore_ascii_case(p))
    });
    if !web
        || url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"')
    {
        return Err(iris_types::Error::other(format!("not a web link: {url}")));
    }
    current().open_url(url)
}

fn desktop_open_url(url: &str) -> iris_types::Result<()> {
    #[cfg(windows)]
    {
        shell_open(std::ffi::OsStr::new(url))
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err(iris_types::Error::other(
            "opening links is only wired up on Windows",
        ))
    }
}

/// What differs from one system to the next, in one place.
///
/// The interface and the core never ask which system they run on: they ask this. The
/// desktop's answers are the code above (the shell, the registry, the toast); Android
/// answers through its own implementation, installed at start by `iris-android`; iOS
/// would add a third. Every method has the desktop's answer as its default, so a new
/// one costs nothing to the systems that do not need it.
pub trait Platform: Send + Sync {
    /// Opens a web link, already checked to be `http(s)://` by [`open_url`].
    fn open_url(&self, url: &str) -> iris_types::Result<()> {
        desktop_open_url(url)
    }
    /// Opens a file Iris wrote, with the app the system gives it.
    fn open_path(&self, path: &std::path::Path) -> iris_types::Result<()> {
        desktop_open_path(path)
    }
    fn system_dark(&self) -> bool {
        windows_impl::system_dark()
    }
    /// The decimal separator, the thousands separator, and whether the day comes first.
    fn number_style(&self) -> (char, char, bool) {
        windows_impl::number_style()
    }
    /// Shows a notification; false when it could not be shown.
    fn notify(&self, notice: &Notice) -> bool {
        let _ = notice;
        false
    }
    /// The main input is a finger.
    fn touch(&self) -> bool {
        false
    }
    /// A folder Iris keeps can be shown in the system's file manager.
    fn folders(&self) -> bool {
        true
    }
    /// What the system draws over the window, in logical pixels: (top, bottom).
    fn insets(&self) -> (f32, f32) {
        (0.0, 0.0)
    }
    /// The fonts to use instead of the theme's (text, display), where the theme's are
    /// not to be had.
    fn font_override(&self) -> Option<(String, String)> {
        None
    }
    /// DNS servers for account discovery; empty: the system's configuration.
    fn dns_servers(&self) -> Vec<std::net::IpAddr> {
        Vec::new()
    }
    /// Mail is fetched while Iris is not on screen (background work, notifications).
    fn has_background(&self) -> bool {
        false
    }
    /// Schedules the background work and the alarms.
    fn schedule(&self, plan: &BackgroundPlan) {
        let _ = plan;
    }
    /// The device is on a local network another Iris may share (Wi-Fi on a phone).
    fn local_network(&self) -> bool {
        true
    }
    /// Asks the system to let multicast in (discovery on a phone's Wi-Fi).
    fn multicast(&self, on: bool) {
        let _ = on;
    }
    /// A QR code can be read with the camera here.
    fn can_scan(&self) -> bool {
        false
    }
    /// Scans a QR code with the camera.
    fn scan_qr(&self, done: Box<dyn FnOnce(Scan) + Send>) {
        done(Scan::Cancelled)
    }
    /// Files are chosen with `pick_files` here, the system's picker answering later,
    /// rather than with a dialog that waits.
    fn picks_files(&self) -> bool {
        false
    }
    /// Opens the system's picker for files (images only, or any), and gives the files
    /// chosen, copied where Iris can read them; none when cancelled.
    fn pick_files(&self, images_only: bool, done: Box<dyn FnOnce(Vec<std::path::PathBuf>) + Send>) {
        let _ = images_only;
        done(Vec::new())
    }
    /// The link Iris was opened with since the last call (a pairing code read by the
    /// phone's own camera), once.
    fn take_link(&self) -> Option<String> {
        None
    }
    /// How a new version reaches this copy.
    fn update_channel(&self) -> UpdateChannel {
        UpdateChannel::Installer
    }
    /// Back with nothing left to close or go back to: on a phone, the app before.
    fn leave(&self) {}
    /// Puts text on the system's clipboard where Iris has no clipboard of its own
    /// (Android: `presse` asks here). False when it could not.
    fn clipboard_set(&self, text: &str) -> bool {
        let _ = text;
        false
    }
    /// The clipboard's text, read the same way.
    fn clipboard_get(&self) -> Option<String> {
        None
    }
}

/// What a scan of a QR code came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scan {
    /// The code's text.
    Read(String),
    /// Closed without a code.
    Cancelled,
    /// The camera could not be used (no permission, the scanner not installed); why, as
    /// the system said it.
    Failed(String),
}

/// A notification, as Iris asks for it; each system draws it its own way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Stable for one thing, so that a newer notice replaces an older one.
    pub id: String,
    pub title: String,
    pub body: String,
    pub kind: NoticeKind,
}

/// What a notification is about, and so which actions it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    Mail {
        thread: i64,
    },
    Event {
        calendar: i64,
        uid: String,
        join: Option<String>,
    },
    Task {
        task: i64,
    },
    Sending,
    Text,
}

/// The background work a system should run for Iris.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackgroundPlan {
    pub every_minutes: u32,
    pub instant: bool,
    pub wifi_only: bool,
    /// Exact alarms: (Unix milliseconds, an id).
    pub alarms: Vec<(i64, String)>,
}

/// How a new version reaches this copy of Iris.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateChannel {
    /// The signed installer from the release page (Windows).
    Installer,
    /// The signed APK from the release page (Android outside a store).
    Apk,
    /// A store updates it; Iris does not check.
    Store,
}

/// The desktop: Windows, and the stubs elsewhere.
pub struct Desktop;

impl Platform for Desktop {}

static PLATFORM: std::sync::OnceLock<Box<dyn Platform>> = std::sync::OnceLock::new();

/// Installs the platform, once, at start. A second call is a mistake and panics: two
/// answers to "what system is this" would be worse than none.
pub fn install(platform: Box<dyn Platform>) {
    if PLATFORM.set(platform).is_err() {
        panic!("the platform is installed once");
    }
}

/// The platform installed at start, or the desktop's.
pub fn current() -> &'static dyn Platform {
    PLATFORM.get_or_init(|| Box::new(Desktop)).as_ref()
}

/// Hands a file or a web link to the shell, as a double-click would.
///
/// Through `ShellExecuteW`, never `explorer.exe <target>`: Explorer reads its command
/// line as its own switches, split at commas, and a link it cannot make sense of (a
/// tracking link with a comma in it, a long one) opens the Documents folder instead.
/// Nor `cmd /c start`, where an ampersand ends the command. Here the target is one
/// string, passed as it is.
#[cfg(windows)]
#[allow(unsafe_code)]
fn shell_open(target: &std::ffi::OsStr) -> iris_types::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let cible: Vec<u16> = target.encode_wide().chain(std::iter::once(0)).collect();
    let verbe: Vec<u16> = "open\0".encode_utf16().collect();
    // The shell may hand the target to an extension that needs COM on this thread.
    // SAFETY: no reserved pointer; balanced by `CoUninitialize` below when it succeeded
    // (S_OK or S_FALSE), left alone when the thread already runs another model.
    let com = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    // SAFETY: both strings are zero-terminated and outlive the call; no window, no
    // parameters, no directory.
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verbe.as_ptr(),
            cible.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if com >= 0 {
        // SAFETY: pairs with the successful `CoInitializeEx` above, on the same thread.
        unsafe { CoUninitialize() };
    }
    // Above 32 is success; at or below, one of the shell's error codes.
    if code > 32 {
        Ok(())
    } else {
        Err(iris_types::Error::other(format!(
            "the system could not open it (code {code})"
        )))
    }
}

/// Ce qu'un `mailto:` demande d'écrire.
///
/// Analysé ici plutôt que dans l'écran de composition, parce que c'est une grammaire
/// et non une saisie : `mailto:` accepte plusieurs destinataires, un sujet, un corps,
/// des copies, et encode le tout en pourcents. Un écran qui essaierait de le lire au
/// vol se tromperait sur le premier sujet contenant une espace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MailtoRequest {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    pub body: String,
}

impl MailtoRequest {
    /// Lit une adresse `mailto:`.
    ///
    /// Renvoie `None` pour tout ce qui n'en est pas une : ce sont les arguments de
    /// ligne de commande qui arrivent ici, et confondre un chemin de fichier avec un
    /// destinataire ouvrirait un brouillon adressé à un dossier.
    pub fn parse(url: &str) -> Option<Self> {
        let reste = url
            .strip_prefix("mailto:")
            .or_else(|| url.strip_prefix("MAILTO:"))?;

        let (destinataires, requete) = match reste.split_once('?') {
            Some((d, q)) => (d, Some(q)),
            None => (reste, None),
        };

        let mut demande = Self {
            to: decode(destinataires),
            ..Default::default()
        };

        for paire in requete.into_iter().flat_map(|q| q.split('&')) {
            let Some((cle, valeur)) = paire.split_once('=') else {
                continue;
            };
            let valeur = decode(valeur);
            // Insensible à la casse : la moitié des liens du web écrivent « Subject ».
            match cle.to_ascii_lowercase().as_str() {
                "to" => pousser(&mut demande.to, &valeur),
                "cc" => pousser(&mut demande.cc, &valeur),
                "bcc" => pousser(&mut demande.bcc, &valeur),
                "subject" => demande.subject = valeur,
                "body" => demande.body = valeur,
                // Le reste du RFC 6068 — « in-reply-to », les en-têtes libres — est
                // ignoré volontairement : les honorer laisserait une page web écrire
                // les en-têtes d'un message envoyé depuis votre adresse.
                _ => {}
            }
        }

        Some(demande)
    }
}

fn pousser(champ: &mut String, valeur: &str) {
    if valeur.is_empty() {
        return;
    }
    if !champ.is_empty() {
        champ.push_str(", ");
    }
    champ.push_str(valeur);
}

/// Décodage des pourcents, avec `+` pour l'espace.
fn decode(texte: &str) -> String {
    let octets = texte.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(octets.len());
    let mut i = 0;

    while i < octets.len() {
        match octets[i] {
            b'%' if i + 2 < octets.len() => {
                // Read on the bytes: "%€" would slice the text inside the euro sign.
                let chiffre = |o: u8| (o as char).to_digit(16);
                match (chiffre(octets[i + 1]), chiffre(octets[i + 2])) {
                    (Some(fort), Some(faible)) => {
                        out.push((fort * 16 + faible) as u8);
                        i += 3;
                    }
                    // Un pourcent qui n'introduit pas deux chiffres hexadécimaux est
                    // un pourcent. Le supprimer perdrait un caractère du sujet.
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            o => {
                out.push(o);
                i += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Telephone(std::sync::Mutex<Vec<String>>);
    impl Platform for Telephone {
        fn open_url(&self, url: &str) -> iris_types::Result<()> {
            self.0.lock().unwrap().push(url.to_string());
            Ok(())
        }
        fn touch(&self) -> bool {
            true
        }
        fn insets(&self) -> (f32, f32) {
            (24.0, 16.0)
        }
    }

    #[test]
    fn the_desktop_answers_when_nothing_was_installed() {
        let p = current();
        assert!(!p.touch());
        assert_eq!(p.insets(), (0.0, 0.0));
        assert!(!p.has_background());
        assert_eq!(p.update_channel(), UpdateChannel::Installer);
    }

    #[test]
    fn a_platform_of_its_own_answers_for_itself() {
        let t = Telephone(std::sync::Mutex::new(Vec::new()));
        t.open_url("https://example.com").unwrap();
        assert_eq!(*t.0.lock().unwrap(), ["https://example.com"]);
        assert!(t.touch());
        assert_eq!(t.insets(), (24.0, 16.0));
        assert_eq!(t.dns_servers(), Vec::<std::net::IpAddr>::new());
    }

    #[test]
    fn only_web_links_reach_the_platform() {
        assert!(open_url("file:///C:/Windows/notepad.exe").is_err());
        assert!(open_url("https://exa mple.com").is_err());
    }

    #[test]
    fn une_adresse_nue() {
        let m = MailtoRequest::parse("mailto:marie@example.com").unwrap();
        assert_eq!(m.to, "marie@example.com");
        assert!(m.subject.is_empty());
    }

    #[test]
    fn le_sujet_et_le_corps_sont_decodes() {
        let m = MailtoRequest::parse(
            "mailto:a@x.fr?subject=Devis%20refonte&body=Bonjour%2C%0ACi-joint.",
        )
        .unwrap();
        assert_eq!(m.subject, "Devis refonte");
        assert_eq!(m.body, "Bonjour,\nCi-joint.");
    }

    #[test]
    fn le_plus_vaut_une_espace() {
        let m = MailtoRequest::parse("mailto:a@x.fr?subject=deux+mots").unwrap();
        assert_eq!(m.subject, "deux mots");
    }

    #[test]
    fn a_percent_before_a_wide_character_stays_a_percent() {
        let m = MailtoRequest::parse("mailto:a@example.com?subject=50%€").unwrap();
        assert_eq!(m.subject, "50%€");
    }

    #[test]
    fn plusieurs_destinataires() {
        let m = MailtoRequest::parse("mailto:a@x.fr,b@y.fr?cc=c@z.fr").unwrap();
        assert_eq!(m.to, "a@x.fr,b@y.fr");
        assert_eq!(m.cc, "c@z.fr");
    }

    #[test]
    fn un_to_en_parametre_s_ajoute_a_celui_du_chemin() {
        let m = MailtoRequest::parse("mailto:a@x.fr?to=b@y.fr").unwrap();
        assert_eq!(m.to, "a@x.fr, b@y.fr");
    }

    #[test]
    fn la_casse_du_parametre_est_ignoree() {
        // La moitié des liens du web écrivent « Subject ».
        let m = MailtoRequest::parse("mailto:a@x.fr?Subject=Salut").unwrap();
        assert_eq!(m.subject, "Salut");
    }

    #[test]
    fn les_en_tetes_libres_sont_ignorees() {
        // Les honorer laisserait une page web écrire les en-têtes d'un message parti
        // de votre adresse.
        let m =
            MailtoRequest::parse("mailto:a@x.fr?from=usurpateur@mal.fr&reply-to=x@y.fr").unwrap();
        assert_eq!(m.to, "a@x.fr");
        assert!(m.cc.is_empty() && m.bcc.is_empty());
    }

    #[test]
    fn ce_qui_n_est_pas_un_mailto_est_refuse() {
        // Ce sont des arguments de ligne de commande qui arrivent ici : confondre un
        // chemin avec un destinataire ouvrirait un brouillon adressé à un dossier.
        assert!(MailtoRequest::parse("C:\\Users\\alici\\note.txt").is_none());
        assert!(MailtoRequest::parse("https://example.com").is_none());
        assert!(MailtoRequest::parse("run").is_none());
    }

    #[test]
    fn un_pourcent_isole_survit() {
        let m = MailtoRequest::parse("mailto:a@x.fr?subject=100%25%20ou%20100%").unwrap();
        assert_eq!(m.subject, "100% ou 100%");
    }

    #[test]
    fn un_mailto_vide_reste_lisible() {
        let m = MailtoRequest::parse("mailto:").unwrap();
        assert!(m.to.is_empty());
    }

    #[test]
    fn l_utf8_percent_encode_revient_entier() {
        let m = MailtoRequest::parse("mailto:a@x.fr?subject=%C3%A9t%C3%A9").unwrap();
        assert_eq!(m.subject, "été");
    }
}
