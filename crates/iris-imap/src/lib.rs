//! `iris-imap` — accès aux boîtes distantes.
//!
//! Le protocole lui-même est délégué à une bibliothèque éprouvée ; ce que cette
//! crate apporte est ce qui manque toujours autour :
//!
//! - un **contrat** ([`ImapConnection`]) qui expose exactement ce dont la
//!   synchronisation a besoin, et rien d'autre. Le reste de l'application ignore
//!   jusqu'au nom de la bibliothèque employée, et une doublure permet de tester la
//!   logique de synchronisation sans serveur ;
//! - un **pool borné** ([`pool::ConnectionPool`]), sans lequel cent comptes
//!   ouvriraient cent connexions permanentes et se feraient bannir ;
//! - la traduction des **capacités** du serveur en décisions de synchronisation :
//!   `CONDSTORE` et `QRESYNC` changent radicalement le coût d'une mise à jour, et
//!   leur absence impose un repli qu'il vaut mieux prévoir que subir.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod client;
pub mod fake;
pub mod pool;
pub mod utf7;

use async_trait::async_trait;
use iris_types::{Flags, Result, Timestamp};
use std::time::Duration;

/// Coordonnées d'un serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Chiffré dès la connexion, ou négocié par `STARTTLS`.
    pub tls_immediate: bool,
}

impl Endpoint {
    pub fn tls(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            tls_immediate: true,
        }
    }

    pub fn starttls(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            tls_immediate: false,
        }
    }
}

/// De quoi s'authentifier.
///
/// Its `Debug` shows the login, never the password or the token: a log line or a
/// panic message must not carry a secret.
#[derive(Clone)]
pub enum Credentials {
    Password {
        user: String,
        password: String,
    },
    /// Jeton d'accès OAuth2, présenté par le mécanisme `XOAUTH2`.
    OAuth2 {
        user: String,
        token: String,
    },
}

impl Credentials {
    pub fn user(&self) -> &str {
        match self {
            Self::Password { user, .. } | Self::OAuth2 { user, .. } => user,
        }
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (genre, secret) = match self {
            Self::Password { .. } => ("Password", "password"),
            Self::OAuth2 { .. } => ("OAuth2", "token"),
        };
        f.debug_struct(genre)
            .field("user", &self.user())
            .field(secret, &"<hidden>")
            .finish()
    }
}

#[cfg(test)]
mod credentials_tests {
    use super::Credentials;

    #[test]
    fn printing_credentials_never_shows_the_secret() {
        let mot_de_passe = Credentials::Password {
            user: "moi@example.com".into(),
            password: "s3cret-pw".into(),
        };
        let jeton = Credentials::OAuth2 {
            user: "moi@example.com".into(),
            token: "ya29.jeton".into(),
        };
        let texte = format!("{mot_de_passe:?} {jeton:?}");
        assert!(texte.contains("moi@example.com"));
        assert!(!texte.contains("s3cret-pw") && !texte.contains("ya29.jeton"));
    }
}

/// Ce que le serveur sait faire.
///
/// Chacune de ces capacités change une décision de synchronisation ; les recenser
/// une fois à la connexion évite de les redemander à chaque cycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Suivi des modifications par numéro de séquence. Permet de ne demander que ce
    /// qui a changé au lieu de relire tous les drapeaux.
    pub condstore: bool,
    /// Resynchronisation rapide après déconnexion, suppressions comprises.
    pub qresync: bool,
    /// Notification poussée des nouveautés.
    pub idle: bool,
    /// Déplacement atomique. Sans lui, il faut copier puis supprimer, ce qui n'est
    /// pas atomique et peut laisser un doublon.
    pub r#move: bool,
    /// L'UID attribué à un message ajouté est retourné.
    pub uidplus: bool,
}

impl Capabilities {
    /// La synchronisation incrémentale est-elle possible ?
    pub fn supports_incremental(&self) -> bool {
        self.condstore
    }

    /// Analyse la liste de capacités annoncée par le serveur.
    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut c = Self::default();
        for name in names {
            match name.as_ref().to_uppercase().as_str() {
                "CONDSTORE" => c.condstore = true,
                // QRESYNC implique CONDSTORE, que certains serveurs n'annoncent
                // alors pas séparément.
                "QRESYNC" => {
                    c.qresync = true;
                    c.condstore = true;
                }
                "IDLE" => c.idle = true,
                "MOVE" => c.r#move = true,
                "UIDPLUS" => c.uidplus = true,
                _ => {}
            }
        }
        c
    }
}

/// Rôle d'un dossier distant, déduit de ses attributs spéciaux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKind {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    Other,
    /// Le dossier ne peut pas contenir de messages.
    NoSelect,
}

/// Un dossier tel qu'annoncé par le serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFolder {
    /// Lisible : décodé de l'UTF-7 modifié du protocole.
    pub path: String,
    pub kind: FolderKind,
    /// Ce qui sépare un dossier de ses enfants chez ce serveur (`/` chez Gmail et
    /// iCloud, `.` chez Courier et bien des Dovecot). `None` quand il ne le dit pas.
    pub delimiter: Option<char>,
}

/// L'état d'un dossier au moment de sa sélection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedFolder {
    /// Un changement de cette valeur invalide tous les UID connus du dossier.
    pub uid_validity: u32,
    pub uid_next: u32,
    pub exists: u32,
    /// Plus grand numéro de modification connu, si `CONDSTORE` est disponible.
    pub highest_modseq: u64,
}

/// Un intervalle d'UID, bornes comprises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UidRange {
    pub from: u32,
    pub to: u32,
}

impl UidRange {
    pub fn new(from: u32, to: u32) -> Self {
        // Un intervalle inversé est presque toujours un calcul d'UID erroné ; le
        // remettre à l'endroit évite une requête vide et silencieuse.
        if from <= to {
            Self { from, to }
        } else {
            Self { from: to, to: from }
        }
    }

    /// Tout ce que le dossier contient.
    pub const ALL: Self = Self {
        from: 1,
        to: u32::MAX,
    };

    /// Tout ce qui est arrivé depuis un UID connu.
    pub fn since(uid: u32) -> Self {
        Self {
            from: uid.saturating_add(1),
            to: u32::MAX,
        }
    }

    pub fn len(&self) -> u64 {
        (self.to as u64).saturating_sub(self.from as u64) + 1
    }

    pub fn is_empty(&self) -> bool {
        self.from > self.to
    }

    /// Notation IMAP, où `*` désigne la borne supérieure du dossier.
    pub fn to_sequence(self) -> String {
        let fin = if self.to == u32::MAX {
            "*".to_string()
        } else {
            self.to.to_string()
        };
        format!("{}:{}", self.from, fin)
    }

    /// Découpe l'intervalle en tranches d'au plus `size` UID.
    ///
    /// Demander cent mille en-têtes en une seule commande fait grossir le tampon de
    /// réception sans borne et retarde le premier affichage : la liste doit se
    /// remplir progressivement.
    pub fn chunks(self, size: u32) -> Vec<UidRange> {
        if self.is_empty() || size == 0 {
            return Vec::new();
        }
        // Un intervalle ouvert ne peut pas être découpé à l'avance.
        if self.to == u32::MAX {
            return vec![self];
        }

        let mut out = Vec::new();
        let mut debut = self.from;
        while debut <= self.to {
            let fin = debut.saturating_add(size - 1).min(self.to);
            out.push(UidRange {
                from: debut,
                to: fin,
            });
            if fin == u32::MAX {
                break;
            }
            debut = fin + 1;
        }
        out
    }
}

/// Un message brut, tel que rapporté par le serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    pub uid: u32,
    pub flags: Flags,
    /// Date de dépôt sur le serveur. Fait foi pour le tri.
    pub internal_date: Timestamp,
    pub size: u64,
    /// En-têtes seuls, ou message complet selon la commande émise.
    pub content: Vec<u8>,
}

/// Ce qu'une attente `IDLE` a rapporté.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    /// Le dossier a changé : il faut resynchroniser.
    Changed,
    /// Le délai est écoulé sans nouvelle.
    TimedOut,
    /// Le serveur a coupé la connexion.
    Disconnected,
}

/// Une connexion établie à une boîte.
#[async_trait]
pub trait ImapConnection: Send + std::fmt::Debug {
    fn capabilities(&self) -> Capabilities;

    async fn list_folders(&mut self) -> Result<Vec<RemoteFolder>>;

    async fn select(&mut self, path: &str) -> Result<SelectedFolder>;

    /// En-têtes d'un intervalle d'UID. Ne télécharge jamais les corps.
    async fn fetch_envelopes(&mut self, range: UidRange) -> Result<Vec<RawMessage>>;

    /// Message complet, corps compris.
    async fn fetch_body(&mut self, uid: u32) -> Result<Vec<u8>>;

    /// UID encore présents dans l'intervalle. Sert à détecter les suppressions.
    async fn existing_uids(&mut self, range: UidRange) -> Result<Vec<u32>>;

    /// The UIDs, in the folder selected, of the message with this `Message-ID`
    /// (without its angle brackets).
    async fn find_message_id(&mut self, message_id: &str) -> Result<Vec<u32>>;

    /// Drapeaux modifiés depuis un numéro de modification donné.
    ///
    /// Exige `CONDSTORE`. C'est l'opération qui rend une synchronisation périodique
    /// négligeable au lieu de relire tout le dossier.
    async fn flags_changed_since(&mut self, modseq: u64) -> Result<Vec<(u32, Flags)>>;

    /// Les drapeaux de tout un intervalle, sans `CONDSTORE`.
    ///
    /// Le repli des serveurs qui ne disent pas ce qui a changé (Exchange, Courier,
    /// bien des hébergeurs) : sans lui, un message lu sur le téléphone restait non lu
    /// ici pour toujours.
    async fn fetch_flags(&mut self, range: UidRange) -> Result<Vec<(u32, Flags)>>;

    async fn store_flags(&mut self, uids: &[u32], flags: Flags, add: bool) -> Result<()>;

    /// Efface pour de bon ces messages du dossier sélectionné, déjà marqués
    /// `\Deleted`.
    ///
    /// `UID EXPUNGE` quand le serveur annonce `UIDPLUS` ; sinon `EXPUNGE`, qui efface
    /// aussi ce que d'autres y ont marqué supprimé. Ne sert qu'à vider la corbeille et
    /// les indésirables, où c'est ce qui est demandé.
    async fn expunge(&mut self, uids: &[u32]) -> Result<()>;

    async fn move_messages(&mut self, uids: &[u32], target: &str) -> Result<()>;

    /// Crée un dossier.
    ///
    /// Réussir silencieusement s'il existe déjà fait partie du contrat : créer un
    /// dossier « partout » veut dire le demander à des serveurs dont certains l'ont
    /// déjà, et un refus qu'on a provoqué soi-même ressemble à une panne dans le
    /// journal. C'est l'idempotence qu'exige le rejeu.
    async fn create_folder(&mut self, path: &str) -> Result<()>;

    /// Renomme un dossier. Réussir quand la cible porte déjà ce nom.
    async fn rename_folder(&mut self, from: &str, to: &str) -> Result<()>;

    /// Supprime un dossier.
    ///
    /// `DELETE` sur un dossier plein détruit son contenu : l'appelant le vide d'abord
    /// (`delete_emptied_folder` dans le rejeu). Réussir quand il n'existe déjà plus,
    /// comme la création.
    async fn delete_folder(&mut self, path: &str) -> Result<()>;

    /// Dépose un message dans un dossier.
    ///
    /// Sert à conserver ce qu'on envoie : sans cela, un message parti depuis Iris
    /// serait invisible depuis le téléphone, et l'utilisateur croirait ne pas
    /// l'avoir envoyé. Retourne l'UID attribué quand le serveur le communique.
    async fn append(&mut self, folder: &str, raw: &[u8], flags: Flags) -> Result<Option<u32>>;

    /// Attend une notification du serveur.
    async fn idle(&mut self, timeout: Duration) -> Result<IdleOutcome>;

    async fn logout(&mut self) -> Result<()>;
}

/// Ce qui sait ouvrir une connexion.
#[async_trait]
pub trait Connector: Send + Sync + std::fmt::Debug {
    async fn connect(
        &self,
        endpoint: &Endpoint,
        credentials: &Credentials,
    ) -> Result<Box<dyn ImapConnection>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qresync_implique_condstore() {
        // Certains serveurs n'annoncent que QRESYNC ; en déduire l'absence de
        // CONDSTORE ferait retomber sur une resynchronisation complète inutile.
        let c = Capabilities::from_names(["QRESYNC", "IDLE"]);
        assert!(c.qresync);
        assert!(c.condstore);
        assert!(c.supports_incremental());
    }

    #[test]
    fn les_capacites_inconnues_sont_ignorees() {
        let c = Capabilities::from_names(["IMAP4rev1", "LITERAL+", "XQUOTA"]);
        assert_eq!(c, Capabilities::default());
        assert!(!c.supports_incremental());
    }

    #[test]
    fn les_capacites_sont_insensibles_a_la_casse() {
        let c = Capabilities::from_names(["condstore", "Idle", "MOVE"]);
        assert!(c.condstore && c.idle && c.r#move);
    }

    #[test]
    fn un_intervalle_inverse_est_remis_a_l_endroit() {
        assert_eq!(UidRange::new(50, 10), UidRange { from: 10, to: 50 });
    }

    #[test]
    fn la_notation_de_sequence_utilise_l_etoile() {
        assert_eq!(UidRange::new(10, 20).to_sequence(), "10:20");
        assert_eq!(UidRange::ALL.to_sequence(), "1:*");
        assert_eq!(UidRange::since(42).to_sequence(), "43:*");
    }

    #[test]
    fn le_decoupage_borne_la_taille_des_requetes() {
        let tranches = UidRange::new(1, 250).chunks(100);
        assert_eq!(tranches.len(), 3);
        assert_eq!(tranches[0], UidRange { from: 1, to: 100 });
        assert_eq!(tranches[2], UidRange { from: 201, to: 250 });
        // Aucun UID perdu ni compté deux fois.
        assert_eq!(tranches.iter().map(|c| c.len()).sum::<u64>(), 250);
    }

    #[test]
    fn un_intervalle_ouvert_ne_se_decoupe_pas() {
        // On ne connaît pas sa borne supérieure avant la réponse du serveur.
        assert_eq!(UidRange::ALL.chunks(100), vec![UidRange::ALL]);
    }

    #[test]
    fn un_decoupage_de_taille_nulle_ne_boucle_pas() {
        assert!(UidRange::new(1, 100).chunks(0).is_empty());
    }

    #[test]
    fn un_intervalle_d_un_seul_uid_reste_entier() {
        let tranches = UidRange::new(7, 7).chunks(100);
        assert_eq!(tranches, vec![UidRange { from: 7, to: 7 }]);
        assert_eq!(tranches[0].len(), 1);
    }
}
