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
}

pub use windows_impl::{
    register_mailto, set_start_at_login, status, system_dark, unregister_mailto, Registration,
};

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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Par l'explorateur plutôt que par `cmd /c start` : ce dernier passe le chemin
        // à un interpréteur de commandes, où une esperluette dans un nom de fichier
        // devient un séparateur d'instructions.
        const SANS_FENETRE: u32 = 0x0800_0000;
        std::process::Command::new("explorer.exe")
            .arg(path)
            .creation_flags(SANS_FENETRE)
            .spawn()
            .map(|_| ())
            .map_err(|e| iris_types::Error::other(format!("ouverture : {e}")))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(iris_types::Error::other(
            "opening files is only wired up on Windows",
        ))
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
                match u8::from_str_radix(&texte[i + 1..i + 3], 16) {
                    Ok(o) => {
                        out.push(o);
                        i += 3;
                    }
                    // Un pourcent qui n'introduit pas deux chiffres hexadécimaux est
                    // un pourcent. Le supprimer perdrait un caractère du sujet.
                    Err(_) => {
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
