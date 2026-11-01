//! Les notifications d'arrivée.
//!
//! Le point de tout ce qui synchronise en arrière-plan : sans notification, une
//! synchronisation d'arrière-plan est du travail que personne ne voit, et autant ne
//! pas la faire.
//!
//! Ce sont les bulles natives de Windows, sous l'identité d'Iris. Cette identité —
//! l'`AppUserModelID` — doit être portée par un raccourci du menu Démarrer, ce qui
//! veut dire qu'elles ne fonctionnent **que sur une copie installée**. C'est une
//! contrainte de Windows, pas un choix ; le contournement connu consiste à emprunter
//! l'identifiant de PowerShell pour faire apparaître quelque chose, et une
//! notification qui ment sur qui l'envoie est pire que pas de notification.
//!
//! Sur un exécutable posé n'importe où, l'appel échoue proprement et [`show`] rend
//! `false`. L'appelant peut alors le dire, ce qui vaut mieux qu'un silence que
//! l'utilisateur prendrait pour une panne.
//!
//! Ce qu'elles ne font pas, volontairement :
//!
//! - **une par message.** Vingt messages arrivés pendant une réunion feraient vingt
//!   bulles, et la vingtième chasserait la première. Une seule les résume.
//! - **le contenu.** L'expéditeur et le sujet, jamais l'aperçu : la bulle s'affiche
//!   sur un écran qu'on partage parfois.
//! - **le courrier indésirable.** Prévenir de l'arrivée de ce qu'on a filtré serait
//!   annuler le filtre.
//!
//! A click opens what the bubble is about — the conversation, the event, the task —
//! through a link of Iris's own scheme ([`Target`]), Iris running or not.

/// Ce dont il faut prévenir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arrival {
    /// Combien de messages, tous comptes confondus.
    pub count: usize,
    /// L'expéditeur du plus récent, déjà mis en forme.
    pub sender: String,
    /// Son sujet.
    pub subject: String,
    /// The conversation of the most recent one.
    pub thread: i64,
}

impl Arrival {
    /// Le titre et le corps de la bulle.
    ///
    /// Un message : qui écrit, et à quel sujet. Plusieurs : combien, et le plus
    /// récent — le nombre seul ne dit pas s'il faut s'interrompre, et le plus récent
    /// seul cache qu'il y en a dix-neuf autres.
    pub fn message(&self) -> (String, String) {
        match self.count {
            0 | 1 => (self.sender.clone(), truncate(&self.subject, 120)),
            n => (
                format!("{n} new messages"),
                format!("Latest from {}", self.sender),
            ),
        }
    }

    /// What a click on the bubble opens: the message when there is one, the mail when
    /// there are several — the bubble names only the latest, and opening it alone
    /// would hide the nineteen others.
    pub fn target(&self) -> Target {
        match self.count {
            0 | 1 if self.thread > 0 => Target::Thread(self.thread),
            _ => Target::Mail,
        }
    }
}

/// The scheme of the links a notification opens Iris with.
///
/// Not `iris:`, which other programs called Iris could claim as well: a click on a
/// bubble would then open one of them.
pub const SCHEME: &str = "iris-mail";

/// What a notification is about, and so what a click on it opens.
///
/// It travels as a link (`iris-mail:thread/12`): Windows starts the program that
/// owns the scheme with it, which reaches a closed Iris as well as a running one —
/// a second launch hands it over to the first (`single`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A conversation, by its id.
    Thread(i64),
    /// The mail itself.
    Mail,
    /// An occurrence of an event, by the calendar's key (`<event id>:<start ms>`).
    Event(String),
    /// A task, by its id.
    Task(i64),
    /// A habit, by its id.
    Habit(i64),
}

impl Target {
    /// The link a click hands over.
    pub fn link(&self) -> String {
        let chemin = match self {
            Target::Thread(id) => format!("thread/{id}"),
            Target::Mail => "mail".to_string(),
            Target::Event(cle) => format!("event/{cle}"),
            Target::Task(id) => format!("task/{id}"),
            Target::Habit(id) => format!("habit/{id}"),
        };
        format!("{SCHEME}:{chemin}")
    }

    /// Reads a link back; `None` for anything that is not one of ours.
    ///
    /// Strict: any page in a browser can open an `iris-mail:` link, so nothing but
    /// what Iris writes itself is taken.
    pub fn parse(lien: &str) -> Option<Target> {
        let lien = lien.trim();
        let (schema, reste) = lien.split_once(':')?;
        if !schema.eq_ignore_ascii_case(SCHEME) {
            return None;
        }
        // The shell sometimes adds a slash at the end, or two after the scheme.
        let reste = reste.trim_start_matches('/').trim_end_matches('/');
        let id = |t: &str| t.parse::<i64>().ok().filter(|n| *n > 0);
        if reste == "mail" {
            return Some(Target::Mail);
        }
        let (genre, valeur) = reste.split_once('/')?;
        match genre {
            "thread" => id(valeur).map(Target::Thread),
            "task" => id(valeur).map(Target::Task),
            "habit" => id(valeur).map(Target::Habit),
            "event" => {
                let (evenement, debut) = valeur.split_once(':')?;
                id(evenement)?;
                debut.parse::<i64>().ok()?;
                Some(Target::Event(valeur.to_string()))
            }
            _ => None,
        }
    }
}

/// The bubble as Windows reads it: a title, a line, and the link a click opens.
///
/// `activationType="protocol"`: a click starts the link's program, the only
/// activation an application without a COM server of its own receives whether it is
/// running or not. Without a target the bubble opens nothing.
pub fn toast_xml(titre: &str, corps: &str, longue: bool, cible: Option<&Target>) -> String {
    let lancement = cible
        .map(|c| {
            format!(
                " launch=\"{}\" activationType=\"protocol\"",
                echapper(&c.link())
            )
        })
        .unwrap_or_default();
    let duree = if longue { "long" } else { "short" };
    format!(
        "<toast{lancement} duration=\"{duree}\"><visual><binding template=\"ToastGeneric\">\
         <text>{}</text><text>{}</text></binding></visual></toast>",
        echapper(titre),
        echapper(corps)
    )
}

/// Text placed in XML: a subject with an ampersand made the whole bubble invalid.
fn echapper(texte: &str) -> String {
    let mut s = String::with_capacity(texte.len());
    for c in texte.chars() {
        match c {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '"' => s.push_str("&quot;"),
            '\'' => s.push_str("&apos;"),
            // Control characters are not allowed in XML 1.0 at all.
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => s.push(' '),
            c => s.push(c),
        }
    }
    s
}

/// Opens what a notification is about, as Home and the search do.
///
/// On the interface thread. The window is brought forward by the caller.
pub fn open(f: &iris_ui::AppWindow, cible: &Target) {
    match cible {
        Target::Thread(id) => f.invoke_home_thread_opened(*id as i32),
        Target::Mail => f.invoke_home_open("mail".into()),
        Target::Event(cle) => f.invoke_home_event_opened(cle.as_str().into()),
        Target::Task(id) => f.invoke_home_task_opened(*id as i32),
        Target::Habit(id) => f.invoke_home_habit_opened(*id as i32),
    }
}

pub use plateforme::{show, show_text};

/// Coupe proprement, sur une frontière de caractère.
fn truncate(texte: &str, max: usize) -> String {
    if texte.chars().count() <= max {
        return texte.to_string();
    }
    let court: String = texte.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", court.trim_end())
}

#[cfg(windows)]
mod plateforme {
    use super::{Arrival, Target};

    /// Affiche la bulle.
    ///
    /// L'`AppUserModelID` est celui d'Iris. Windows n'affiche une bulle que pour une
    /// identité qu'il connaît, c'est-à-dire une application dont un raccourci du menu
    /// Démarrer porte cet identifiant — ce que fait l'installateur. Lancée depuis un
    /// exécutable posé n'importe où, l'appel échoue proprement et rien ne s'affiche :
    /// c'est le comportement voulu, et c'est aussi celui qu'il faut expliquer plutôt
    /// que d'emprunter l'identité de PowerShell pour faire apparaître quelque chose.
    pub fn show(arrivee: &Arrival) -> bool {
        let (titre, corps) = arrivee.message();
        montrer(&titre, &corps, false, Some(&arrivee.target()))
    }

    /// Une bulle quelconque : un titre, une ligne. Les rappels d'agenda passent par là.
    pub fn show_text(titre: &str, corps: &str, cible: Option<&Target>) -> bool {
        montrer(titre, corps, true, cible)
    }

    /// The bubble, written as XML and handed to WinRT ourselves: the crate this went
    /// through had no way to say what a click opens. No `<audio>`: the system's own
    /// sound, which the user can turn off for Iris in Windows' settings.
    fn montrer(titre: &str, corps: &str, longue: bool, cible: Option<&Target>) -> bool {
        // Only a link Windows can follow: without the scheme registered, a click
        // would ask which application opens "iris-mail:".
        let cible = cible.filter(|_| crate::platform::scheme_registered());
        let xml = super::toast_xml(titre, corps, longue, cible);
        match afficher(&xml) {
            Ok(()) => true,
            Err(e) => {
                tracing::debug!(error = %e, "notification");
                false
            }
        }
    }

    fn afficher(xml: &str) -> windows::core::Result<()> {
        use windows::core::HSTRING;
        use windows::Data::Xml::Dom::XmlDocument;
        use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let bulle = ToastNotification::CreateToastNotification(&doc)?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
            crate::platform::APP_ID,
        ))?
        .Show(&bulle)
    }
}

#[cfg(not(windows))]
mod plateforme {
    use super::{Arrival, Target};

    pub fn show(_arrivee: &Arrival) -> bool {
        false
    }

    pub fn show_text(_titre: &str, _corps: &str, _cible: Option<&Target>) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrivee(count: usize) -> Arrival {
        Arrival {
            count,
            sender: "Marie Dupont".into(),
            subject: "Devis refonte du site".into(),
            thread: 42,
        }
    }

    #[test]
    fn un_message_annonce_qui_ecrit_et_a_quel_sujet() {
        let (titre, corps) = arrivee(1).message();
        assert_eq!(titre, "Marie Dupont");
        assert_eq!(corps, "Devis refonte du site");
    }

    #[test]
    fn plusieurs_messages_sont_resumes_en_une_bulle() {
        // Vingt messages arrivés pendant une réunion feraient vingt bulles, et la
        // vingtième chasserait la première.
        let (titre, corps) = arrivee(20).message();
        assert_eq!(titre, "20 new messages");
        assert!(corps.contains("Marie Dupont"), "le plus récent est nommé");
    }

    #[test]
    fn un_sujet_interminable_est_coupe() {
        let a = Arrival {
            count: 1,
            sender: "X".into(),
            subject: "a".repeat(400),
            thread: 1,
        };
        let (_, corps) = a.message();
        assert!(corps.chars().count() <= 120);
        assert!(corps.ends_with('…'));
    }

    #[test]
    fn la_coupe_tombe_sur_un_caractere_entier() {
        // Couper sur un octet au milieu d'un caractère accentué produirait une chaîne
        // invalide, et la bulle afficherait un losange à la place.
        let a = Arrival {
            count: 1,
            sender: "X".into(),
            subject: "é".repeat(200),
            thread: 1,
        };
        let (_, corps) = a.message();
        assert!(corps.chars().all(|c| c == 'é' || c == '…'));
    }

    #[test]
    fn zero_se_comporte_comme_un() {
        // Le compteur vient du moteur ; s'il arrive à zéro c'est un défaut ailleurs,
        // et afficher « 0 new messages » le rendrait visible à l'utilisateur plutôt
        // qu'au journal.
        let (titre, _) = arrivee(0).message();
        assert_eq!(titre, "Marie Dupont");
    }

    #[test]
    fn one_arrival_opens_its_conversation_several_open_the_mail() {
        assert_eq!(arrivee(1).target(), Target::Thread(42));
        assert_eq!(arrivee(20).target(), Target::Mail);
    }

    #[test]
    fn every_target_comes_back_from_its_link() {
        for t in [
            Target::Thread(12),
            Target::Mail,
            Target::Event("7:1767225600000".into()),
            Target::Task(9),
            Target::Habit(3),
        ] {
            assert_eq!(Target::parse(&t.link()), Some(t.clone()), "{}", t.link());
        }
        assert_eq!(Target::Thread(12).link(), "iris-mail:thread/12");
    }

    #[test]
    fn a_link_the_shell_decorated_is_still_read() {
        assert_eq!(
            Target::parse("IRIS-MAIL://thread/12/"),
            Some(Target::Thread(12))
        );
    }

    #[test]
    fn anything_else_is_refused() {
        // A page in a browser can open these links: only what Iris writes is taken.
        for lien in [
            "mailto:ann@example.com",
            "iris-mail:",
            "iris-mail:thread/",
            "iris-mail:thread/-3",
            "iris-mail:thread/12; rm",
            "iris-mail:event/7",
            "iris-mail:event/x:12",
            "iris-mail:settings/1",
            "https://example.com/thread/12",
        ] {
            assert_eq!(Target::parse(lien), None, "{lien}");
        }
    }

    #[test]
    fn the_bubble_carries_its_link_and_escapes_its_text() {
        let xml = toast_xml(
            "Ann & Bob <ann@example.com>",
            "\"Quotes\"",
            false,
            Some(&Target::Thread(12)),
        );
        assert!(xml.contains(r#"launch="iris-mail:thread/12" activationType="protocol""#));
        assert!(xml.contains("Ann &amp; Bob &lt;ann@example.com&gt;"));
        assert!(xml.contains("&quot;Quotes&quot;"));
        assert!(xml.contains(r#"duration="short""#));
        let sans = toast_xml("T", "B", true, None);
        assert!(!sans.contains("launch"));
        assert!(sans.contains(r#"duration="long""#));
    }
}
