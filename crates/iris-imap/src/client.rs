//! La connexion réelle, au-dessus d'`async-imap`.
//!
//! Cette couche ne fait que traduire : notre contrat vers les commandes du
//! protocole, et les réponses du protocole vers nos types. Toute la logique de
//! synchronisation vit ailleurs et se teste contre la doublure — ici, il n'y a que
//! du câblage, et c'est voulu.
//!
//! Trois décisions méritent d'être signalées :
//!
//! - **la confiance est celle du système**, comme pour l'envoi, avec les racines
//!   embarquées pour repli : une autorité d'entreprise installée sur le poste servait
//!   à envoyer et était refusée à la lecture ;
//! - **`STARTTLS` est géré**, car quelques hébergeurs n'exposent encore que le port
//!   143 ; un serveur qui refuse de chiffrer est refusé, rien n'est dit en clair
//!   au-delà de l'accueil et de `STARTTLS` ;
//! - **les noms de dossiers sont décodés** de l'UTF-7 modifié à leur arrivée et
//!   réencodés à chaque commande (`utf7`) ;
//! - **les en-têtes demandés sont limités** à ce que la liste affiche. Demander
//!   `BODY[]` à la synchronisation initiale téléchargerait des gigaoctets.

use crate::{
    Capabilities, Connector, Credentials, Endpoint, FolderKind, IdleOutcome, ImapConnection,
    RawMessage, RemoteFolder, SelectedFolder, UidRange,
};
use async_trait::async_trait;
use futures::StreamExt;
use iris_types::{Error, Flags, Result, Timestamp};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;

/// Champs demandés à la synchronisation des en-têtes.
///
/// `ENVELOPE` fournirait déjà l'essentiel, mais pas `References`, sans lequel le
/// regroupement en fils est impossible. On demande donc explicitement les en-têtes
/// utiles, et eux seuls.
///
/// The spam filter's headers are asked for too (they were not, and the check that
/// reads them never ran), and the first two kilobytes of the text: the list's preview
/// is made from them, and the structure they show keeps an HTML newsletter from
/// looking as if it carried a file.
const HEADER_FIELDS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE \
     BODY.PEEK[HEADER.FIELDS (DATE FROM TO CC REPLY-TO SUBJECT MESSAGE-ID \
     IN-REPLY-TO REFERENCES LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST CONTENT-TYPE \
     X-SPAM-FLAG X-SPAM-STATUS X-SPAM)] BODY.PEEK[TEXT]<0.2048>)";

// Avec la variante tokio d'async-imap, les flux tokio sont attendus tels quels :
// aucun adaptateur n'est nécessaire.
type Stream = tokio_rustls::client::TlsStream<TcpStream>;
type Session = async_imap::Session<Stream>;

/// Établit les connexions réelles.
#[derive(Debug, Clone)]
pub struct RustlsConnector {
    config: Arc<rustls::ClientConfig>,
}

impl RustlsConnector {
    /// Trusts what the system trusts, as sending does (`lettre` with the platform
    /// verifier): a company's own authority, installed with its profile, was trusted
    /// to send mail and refused for reading it. The embedded roots remain the fallback
    /// when the system's store cannot be used.
    pub fn new() -> Self {
        use rustls_platform_verifier::BuilderVerifierExt;

        let config = rustls::ClientConfig::builder()
            .with_platform_verifier()
            .map(|b| b.with_no_client_auth())
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "system certificate store unusable, embedded roots used");
                let mut roots = rustls::RootCertStore::empty();
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth()
            });

        Self {
            config: Arc::new(config),
        }
    }

    async fn tls_stream(
        &self,
        endpoint: &Endpoint,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let tcp = connecter(&endpoint.host, endpoint.port).await?;

        // Nagle retarde les petites commandes IMAP de plusieurs dizaines de
        // millisecondes ; sur une synchronisation bavarde, cela se voit.
        let _ = tcp.set_nodelay(true);

        self.wrap(endpoint, tcp).await
    }

    async fn wrap(
        &self,
        endpoint: &Endpoint,
        tcp: TcpStream,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let nom = rustls::pki_types::ServerName::try_from(endpoint.host.clone())
            .map_err(|_| Error::Config(format!("nom d'hôte invalide : « {} »", endpoint.host)))?;

        tokio_rustls::TlsConnector::from(Arc::clone(&self.config))
            .connect(nom, tcp)
            .await
            .map_err(|e| Error::network(format!("négociation TLS avec {} : {e}", endpoint.host)))
    }

    /// Port 143: the greeting and `STARTTLS` in the clear, then TLS over the same
    /// connection. Nothing else is said in the clear, and a server that will not
    /// upgrade is refused rather than sent a password openly.
    ///
    /// Profiles, the Proton Bridge preset and discovery's fallback all produce this,
    /// and every one of those accounts failed with "needs STARTTLS, which this version
    /// does not handle yet".
    async fn starttls_stream(
        &self,
        endpoint: &Endpoint,
    ) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let tcp = connecter(&endpoint.host, endpoint.port).await?;
        let _ = tcp.set_nodelay(true);
        let mut lecteur = BufReader::new(tcp);
        let reseau =
            |e: std::io::Error| Error::network(format!("STARTTLS avec {} : {e}", endpoint.host));

        let mut ligne = Vec::new();
        lecteur
            .read_until(b'\n', &mut ligne)
            .await
            .map_err(reseau)?;
        let accueil = String::from_utf8_lossy(&ligne).to_ascii_uppercase();
        if !accueil.starts_with("* OK") {
            return Err(Error::network(format!(
                "{} did not greet as an IMAP server: {}",
                endpoint.host,
                accueil.trim()
            )));
        }

        lecteur
            .get_mut()
            .write_all(b"S1 STARTTLS\r\n")
            .await
            .map_err(reseau)?;
        loop {
            ligne.clear();
            let lu = lecteur
                .read_until(b'\n', &mut ligne)
                .await
                .map_err(reseau)?;
            if lu == 0 {
                return Err(Error::network(format!(
                    "{} closed the connection during STARTTLS",
                    endpoint.host
                )));
            }
            let reponse = String::from_utf8_lossy(&ligne).to_ascii_uppercase();
            if let Some(etat) = reponse.strip_prefix("S1 ") {
                if etat.starts_with("OK") {
                    break;
                }
                return Err(Error::Config(format!(
                    "{}:{} will not encrypt the connection (STARTTLS refused): use its \
                     encrypted port, usually 993",
                    endpoint.host, endpoint.port
                )));
            }
        }
        // Anything already sent after the answer would be read as if it came over
        // TLS: a known way to slip commands in (CVE-2011-0411 and its kin).
        if !lecteur.buffer().is_empty() {
            return Err(Error::network(format!(
                "{} sent data before the TLS handshake",
                endpoint.host
            )));
        }
        self.wrap(endpoint, lecteur.into_inner()).await
    }
}

/// How long one address of a server is given to take the connection.
const PAR_ADRESSE: std::time::Duration = std::time::Duration::from_secs(8);

/// Reaches `host:port` over TCP: its IPv4 addresses first, then IPv6, each for a few
/// seconds only.
///
/// One `connect` on the name tried the addresses one after the other, each for as long
/// as Windows waits (about twenty seconds). On a network whose IPv6 goes nowhere,
/// Gmail's first addresses were IPv6: two of them used up the half-minute a sign-in is
/// given, and the account failed with "did not answer" although its IPv4 answers at
/// once.
async fn connecter(host: &str, port: u16) -> Result<TcpStream> {
    let echec =
        |e: &dyn std::fmt::Display| Error::network(format!("connexion à {host}:{port} : {e}"));
    let mut adresses: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| echec(&e))?
        .collect();
    adresses.sort_by_key(|a| a.is_ipv6());
    let mut derniere = None;
    for adresse in adresses {
        match tokio::time::timeout(PAR_ADRESSE, TcpStream::connect(adresse)).await {
            Ok(Ok(tcp)) => return Ok(tcp),
            Ok(Err(e)) => derniere = Some(e.to_string()),
            Err(_) => derniere = Some(format!("{adresse} did not answer")),
        }
    }
    Err(echec(
        &derniere.unwrap_or_else(|| "no address for this name".to_string()),
    ))
}

impl Default for RustlsConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Connector for RustlsConnector {
    async fn connect(
        &self,
        endpoint: &Endpoint,
        credentials: &Credentials,
    ) -> Result<Box<dyn ImapConnection>> {
        // After STARTTLS the greeting has already been read, in the clear, and the
        // server says nothing more until spoken to.
        let (tls, accueil_lu) = if endpoint.tls_immediate {
            (self.tls_stream(endpoint).await?, false)
        } else {
            (self.starttls_stream(endpoint).await?, true)
        };
        let session = sign_in(tls, credentials, accueil_lu).await?;

        let mut connection = ImapClient {
            session: Some(session),
            capabilities: Capabilities::default(),
        };
        connection.load_capabilities().await?;
        Ok(Box::new(connection))
    }
}

/// Reads the server's greeting, then signs in.
///
/// `async_imap::Client::new` leaves the greeting (`* OK Gimap ready…`) unread. `LOGIN`
/// never noticed: it skips untagged lines on its way to its own answer. `AUTHENTICATE`
/// did: it took the greeting for the end of the exchange, never answered the server's
/// `+`, and waited with Gmail until the connection timed out. Every account signed in
/// with Google failed that way, as "did not answer".
async fn sign_in<T>(
    stream: T,
    credentials: &Credentials,
    greeting_read: bool,
) -> Result<async_imap::Session<T>>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    use async_imap::imap_proto::{Response, Status};

    let mut client = async_imap::Client::new(stream);
    if !greeting_read {
        let accueil = client
            .read_response()
            .await
            .map_err(|e| Error::network(format!("greeting from the server: {e}")))?
            .ok_or_else(|| Error::network("the server closed the connection before greeting"))?;
        if let Response::Data {
            status: Status::Bye,
            information,
            ..
        } = accueil.parsed()
        {
            return Err(Error::network(format!(
                "the server refused the connection: {}",
                information.as_deref().unwrap_or("no reason given")
            )));
        }
    }

    match credentials {
        Credentials::Password { user, password } => client
            .login(user, password)
            .await
            .map_err(|(e, _)| translate_login_error(e, user)),
        Credentials::OAuth2 { user, token } => {
            let auth = XOAuth2 {
                user: user.clone(),
                token: token.clone(),
                sent: false,
            };
            client
                .authenticate("XOAUTH2", auth)
                .await
                .map_err(|(e, _)| translate_login_error(e, user))
        }
    }
}

/// Mécanisme `XOAUTH2`, tel qu'attendu par Google et Microsoft.
struct XOAuth2 {
    user: String,
    token: String,
    /// The token has gone. A second challenge is the server's reason for refusing it
    /// (base64 JSON), and SASL wants an empty line back, not the token again.
    sent: bool,
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        if std::mem::replace(&mut self.sent, true) {
            return String::new();
        }
        format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.token)
    }
}

/// Distingue un refus d'identifiants d'une panne réseau.
///
/// La distinction commande le comportement de l'ordonnanceur : une panne se retente,
/// un mot de passe faux doit remonter à l'utilisateur au lieu d'être martelé.
fn translate_login_error(e: async_imap::error::Error, account: &str) -> Error {
    let texte = e.to_string();
    let minuscules = texte.to_lowercase();
    // Servers phrase this a dozen ways and agree on none of them. Getting the
    // classification wrong costs the user either a password prompt they do not need
    // or an account retried for ever against a password that will never work.
    const REFUSALS: &[&str] = &[
        "authenticationfailed",
        "authentication failed",
        "invalid credentials",
        "login failed",
        "invalid login",
        "invalid user",
        "invalid password",
        "incorrect password",
        "bad username or password",
        "username and password not accepted",
        "authentication unsuccessful",
        "login denied",
        "auth failed",
        "[authenticationfailed]",
        "webalias",
        "application-specific password required",
    ];

    if REFUSALS.iter().any(|m| minuscules.contains(m)) {
        Error::AuthFailed {
            account: account.to_string(),
        }
    } else {
        Error::network(format!("connexion de {account} : {texte}"))
    }
}

/// Une connexion établie.
#[derive(Debug)]
pub struct ImapClient {
    /// Absente seulement le temps d'une attente `IDLE`, qui consomme la session.
    session: Option<Session>,
    capabilities: Capabilities,
}

impl ImapClient {
    fn session(&mut self) -> Result<&mut Session> {
        self.session.as_mut().ok_or_else(|| Error::Protocol {
            protocol: "IMAP",
            message: "session en cours d'attente".into(),
        })
    }

    async fn load_capabilities(&mut self) -> Result<()> {
        let session = self.session()?;
        let caps = session
            .capabilities()
            .await
            .map_err(|e| protocol_error("lecture des capacités", e))?;
        let noms: Vec<String> = caps
            .iter()
            .map(|c| format!("{c:?}").to_uppercase())
            .collect();
        self.capabilities = Capabilities::from_names(noms);
        Ok(())
    }
}

/// Traduit une erreur de la bibliothèque en disant si elle vaut d'être retentée.
///
/// Tout devenait `Protocol`, que rien ne retente : une connexion coupée au milieu du
/// rejeu faisait abandonner pour de bon l'action de l'utilisateur. Un flux rompu, et
/// les refus que la RFC 5530 déclare passagers, sont donc des pannes réseau.
fn protocol_error(quoi: &str, e: async_imap::error::Error) -> Error {
    use async_imap::error::Error as E;
    let message = format!("{quoi} : {e}");
    match &e {
        E::Io(_) | E::ConnectionLost => Error::Network(message),
        E::No(texte) | E::Bad(texte) => {
            let code = texte.to_ascii_uppercase();
            if code.contains("[THROTTLED]") || code.contains("[LIMIT]") {
                Error::Throttled {
                    retry_after_secs: 60,
                }
            } else if code.contains("[UNAVAILABLE]") || code.contains("[INUSE]") {
                Error::Network(message)
            } else {
                Error::Protocol {
                    protocol: "IMAP",
                    message,
                }
            }
        }
        _ => Error::Protocol {
            protocol: "IMAP",
            message,
        },
    }
}

/// The headers, a blank line, and the start of the text, as one message to parse.
fn headers_and_start(headers: Option<&[u8]>, text: Option<&[u8]>) -> Vec<u8> {
    let mut sortie = headers.map(<[u8]>::to_vec).unwrap_or_default();
    if let Some(debut) = text.filter(|t| !t.is_empty()) {
        if !sortie.ends_with(b"\r\n\r\n") && !sortie.ends_with(b"\n\n") {
            sortie.extend_from_slice(b"\r\n");
        }
        sortie.extend_from_slice(debut);
    }
    sortie
}

/// Traduit les drapeaux du protocole vers les nôtres.
fn translate_flags<'a>(flags: impl Iterator<Item = async_imap::types::Flag<'a>>) -> Flags {
    use async_imap::types::Flag as F;
    let mut out = Flags::NONE;
    for f in flags {
        out = match f {
            F::Seen => out.with(Flags::SEEN),
            F::Answered => out.with(Flags::ANSWERED),
            F::Flagged => out.with(Flags::FLAGGED),
            F::Draft => out.with(Flags::DRAFT),
            F::Deleted => out.with(Flags::DELETED),
            F::Recent => out.with(Flags::RECENT),
            // Les mots-clés propres au serveur ne nous concernent pas.
            _ => out,
        };
    }
    out
}

/// Construit la liste des drapeaux à poser ou retirer.
fn flags_to_names(flags: Flags) -> String {
    let mut noms = Vec::new();
    if flags.contains(Flags::SEEN) {
        noms.push("\\Seen");
    }
    if flags.contains(Flags::ANSWERED) {
        noms.push("\\Answered");
    }
    if flags.contains(Flags::FLAGGED) {
        noms.push("\\Flagged");
    }
    if flags.contains(Flags::DRAFT) {
        noms.push("\\Draft");
    }
    if flags.contains(Flags::DELETED) {
        noms.push("\\Deleted");
    }
    format!("({})", noms.join(" "))
}

/// Un répertoire de travail du serveur, que LIST montre comme une boîte.
///
/// Quand Dovecot range le courrier à la racine du répertoire personnel, ses propres
/// fichiers y deviennent des dossiers : `dovecot.lda-dupes.locks` (les verrous de la
/// détection de doublons à la livraison) et `sieve` (les scripts de filtrage). Aucun
/// ne contient de courrier, et le serveur les recrée à chaque livraison : les montrer
/// offrait une suppression qui ne tenait jamais.
fn is_server_internal(name: &str) -> bool {
    let premier = name
        .split(['.', '/'])
        .find(|s| !s.eq_ignore_ascii_case("INBOX"))
        .unwrap_or("");
    premier == "dovecot" || premier.starts_with("dovecot-") || name == "sieve"
}

/// Déduit le rôle d'un dossier de ses attributs spéciaux, avec repli sur son nom.
#[cfg(test)]
fn folder_kind(name: &str, attributes: &[async_imap::types::NameAttribute<'_>]) -> FolderKind {
    special_use(attributes).unwrap_or_else(|| kind_by_name(name, None))
}

/// A view the server builds from a flag, holding copies of mail kept elsewhere:
/// Gmail's Starred (`\Flagged`) and Important, Dovecot's virtual Flagged.
///
/// Synced as folders, every starred or important message was stored once more, counted
/// unread once more, and moved out of them by Archive, which on Gmail takes the star
/// or the importance away. The star is a flag on the message; nothing is lost by
/// leaving the view out.
fn is_virtual(attributes: &[async_imap::types::NameAttribute<'_>]) -> bool {
    use async_imap::types::NameAttribute as A;
    attributes.iter().any(|a| match a {
        A::Flagged => true,
        A::Extension(s) => s.eq_ignore_ascii_case("\\Important"),
        _ => false,
    })
}

/// Le rôle que le serveur annonce lui-même, s'il en annonce un.
fn special_use(attributes: &[async_imap::types::NameAttribute<'_>]) -> Option<FolderKind> {
    use async_imap::types::NameAttribute as A;

    // Les attributs du RFC 6154 sont la source la plus fiable : le serveur dit
    // lui-même à quoi sert le dossier.
    for a in attributes {
        match a {
            A::NoSelect => return Some(FolderKind::NoSelect),
            // RFC 5258 : un nom qui n'existe que parce qu'il a des enfants. Aussi
            // impossible à sélectionner qu'un `\Noselect`, et Dovecot l'emploie à sa
            // place pour les branches de son arborescence.
            A::Extension(s) if s.eq_ignore_ascii_case("\\NonExistent") => {
                return Some(FolderKind::NoSelect)
            }
            A::Sent => return Some(FolderKind::Sent),
            A::Drafts => return Some(FolderKind::Drafts),
            A::Trash => return Some(FolderKind::Trash),
            A::Junk => return Some(FolderKind::Junk),
            A::Archive | A::All => return Some(FolderKind::Archive),
            _ => {}
        }
    }
    None
}

/// Repli sur le nom : de nombreux serveurs n'annoncent aucun attribut spécial, et les
/// noms sont localisés.
///
/// Seul un dossier à la racine, ou juste sous le préfixe personnel (`INBOX.`) ou sous
/// `[Gmail]/`, peut recevoir un rôle ainsi. Le dernier segment suffisait : un libellé
/// `Clients/Trash` devenait la corbeille (Supprimer y rangeait le courrier), un
/// `X/Spam` voyait tout son contenu caché comme indésirable, un `Archives/Inbox`
/// rejoignait la boîte de réception.
fn kind_by_name(name: &str, delimiter: Option<&str>) -> FolderKind {
    let n = name.to_lowercase();
    if n == "inbox" {
        return FolderKind::Inbox;
    }
    let separateurs: Vec<char> = match delimiter.and_then(|d| d.chars().next()) {
        Some(c) => vec![c],
        None => vec!['/', '.'],
    };
    let mut parties: Vec<&str> = n.split(separateurs.as_slice()).collect();
    if parties.len() == 2 && matches!(parties[0], "inbox" | "[gmail]" | "[google mail]") {
        parties.remove(0);
    }
    let [seul] = parties.as_slice() else {
        return FolderKind::Other;
    };
    match *seul {
        "sent"
        | "sent items"
        | "sent messages"
        | "sent mail"
        | "envoyés"
        | "éléments envoyés"
        | "messages envoyés" => FolderKind::Sent,
        "drafts" | "draft" | "brouillons" => FolderKind::Drafts,
        "trash"
        | "deleted items"
        | "deleted messages"
        | "deleted"
        | "bin"
        | "corbeille"
        | "éléments supprimés" => FolderKind::Trash,
        "junk"
        | "spam"
        | "junk e-mail"
        | "junk email"
        | "indésirables"
        | "courrier indésirable" => FolderKind::Junk,
        "archive" | "archives" => FolderKind::Archive,
        _ => FolderKind::Other,
    }
}

/// Les rôles des dossiers d'une liste.
///
/// Un rôle que le serveur annonce par attribut n'est deviné pour aucun autre dossier :
/// sur Gmail, un libellé personnel « Trash » à la racine passait, par l'ordre
/// alphabétique, devant `[Gmail]/Trash`.
fn assign_kinds(listed: &[(String, Option<FolderKind>, Option<String>)]) -> Vec<FolderKind> {
    let annonces: Vec<FolderKind> = listed.iter().filter_map(|(_, k, _)| *k).collect();
    listed
        .iter()
        .map(|(nom, special, delim)| match special {
            Some(k) => *k,
            None => match kind_by_name(nom, delim.as_deref()) {
                k if k != FolderKind::Inbox && annonces.contains(&k) => FolderKind::Other,
                k => k,
            },
        })
        .collect()
}

#[async_trait]
impl ImapConnection for ImapClient {
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    async fn list_folders(&mut self) -> Result<Vec<RemoteFolder>> {
        let session = self.session()?;
        let mut flux = session
            .list(Some(""), Some("*"))
            .await
            .map_err(|e| protocol_error("liste des dossiers", e))?;

        let mut listes = Vec::new();
        while let Some(nom) = flux.next().await {
            let nom = nom.map_err(|e| protocol_error("liste des dossiers", e))?;
            let special = special_use(nom.attributes());
            let lisible = crate::utf7::decode(nom.name());
            if special == Some(FolderKind::NoSelect)
                || is_virtual(nom.attributes())
                || is_server_internal(&lisible)
            {
                continue;
            }
            listes.push((lisible, special, nom.delimiter().map(str::to_string)));
        }
        let roles = assign_kinds(&listes);
        Ok(listes
            .into_iter()
            .zip(roles)
            .map(|((path, _, delim), kind)| RemoteFolder {
                path,
                kind,
                delimiter: delim.and_then(|d| d.chars().next()),
            })
            .collect())
    }

    async fn select(&mut self, path: &str) -> Result<SelectedFolder> {
        let condstore = self.capabilities.condstore;
        let nom = crate::utf7::encode(path);
        let session = self.session()?;

        let boite = if condstore {
            session.select_condstore(&nom).await
        } else {
            session.select(&nom).await
        }
        .map_err(|e| protocol_error(&format!("sélection de « {path} »"), e))?;

        Ok(SelectedFolder {
            uid_validity: boite.uid_validity.unwrap_or(0),
            uid_next: boite.uid_next.unwrap_or(1),
            exists: boite.exists,
            highest_modseq: boite.highest_modseq.unwrap_or(0),
        })
    }

    async fn fetch_envelopes(&mut self, range: UidRange) -> Result<Vec<RawMessage>> {
        let session = self.session()?;
        let mut flux = session
            .uid_fetch(range.to_sequence(), HEADER_FIELDS)
            .await
            .map_err(|e| protocol_error("récupération des en-têtes", e))?;

        let mut out = Vec::new();
        while let Some(item) = flux.next().await {
            let f = item.map_err(|e| protocol_error("récupération des en-têtes", e))?;
            // Une réponse sans UID est inexploitable : nous n'aurions aucun moyen de
            // la relier à quoi que ce soit.
            let Some(uid) = f.uid else { continue };

            out.push(RawMessage {
                uid,
                flags: translate_flags(f.flags()),
                internal_date: f
                    .internal_date()
                    .map(|d| Timestamp::from_millis(d.timestamp_millis()))
                    .unwrap_or(Timestamp::EPOCH),
                size: f.size.unwrap_or(0) as u64,
                content: headers_and_start(f.header(), f.text()),
            });
        }
        Ok(out)
    }

    async fn fetch_body(&mut self, uid: u32) -> Result<Vec<u8>> {
        let session = self.session()?;
        let mut flux = session
            .uid_fetch(uid.to_string(), "(BODY.PEEK[])")
            .await
            .map_err(|e| protocol_error("récupération du corps", e))?;

        while let Some(item) = flux.next().await {
            let f = item.map_err(|e| protocol_error("récupération du corps", e))?;
            if let Some(corps) = f.body() {
                return Ok(corps.to_vec());
            }
        }
        Err(Error::Protocol {
            protocol: "IMAP",
            message: format!("corps de l'UID {uid} absent"),
        })
    }

    async fn existing_uids(&mut self, range: UidRange) -> Result<Vec<u32>> {
        let session = self.session()?;
        let uids = session
            .uid_search(format!("UID {}", range.to_sequence()))
            .await
            .map_err(|e| protocol_error("recherche des UID", e))?;

        let mut out: Vec<u32> = uids.into_iter().collect();
        out.sort_unstable();
        Ok(out)
    }

    async fn find_message_id(&mut self, message_id: &str) -> Result<Vec<u32>> {
        // Quotes and backslashes cannot be in an IMAP quoted string unescaped, and
        // have no business in a Message-ID: dropped rather than escaped.
        let propre: String = message_id
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .chars()
            .filter(|c| !matches!(c, '"' | '\\') && !c.is_control())
            .collect();
        if propre.is_empty() {
            return Ok(Vec::new());
        }
        let session = self.session()?;
        let uids = session
            .uid_search(format!("HEADER Message-ID \"<{propre}>\""))
            .await
            .map_err(|e| protocol_error("recherche du message", e))?;
        let mut out: Vec<u32> = uids.into_iter().collect();
        out.sort_unstable();
        Ok(out)
    }

    async fn flags_changed_since(&mut self, modseq: u64) -> Result<Vec<(u32, Flags)>> {
        if !self.capabilities.condstore {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: "CONDSTORE non disponible sur ce serveur".into(),
            });
        }

        let session = self.session()?;
        let mut flux = session
            .uid_fetch("1:*", format!("(UID FLAGS) (CHANGEDSINCE {modseq})"))
            .await
            .map_err(|e| protocol_error("drapeaux modifiés", e))?;

        let mut out = Vec::new();
        while let Some(item) = flux.next().await {
            let f = item.map_err(|e| protocol_error("drapeaux modifiés", e))?;
            if let Some(uid) = f.uid {
                out.push((uid, translate_flags(f.flags())));
            }
        }
        Ok(out)
    }

    async fn fetch_flags(&mut self, range: UidRange) -> Result<Vec<(u32, Flags)>> {
        let session = self.session()?;
        let mut flux = session
            .uid_fetch(range.to_sequence(), "(UID FLAGS)")
            .await
            .map_err(|e| protocol_error("lecture des drapeaux", e))?;

        let mut out = Vec::new();
        while let Some(item) = flux.next().await {
            let f = item.map_err(|e| protocol_error("lecture des drapeaux", e))?;
            if let Some(uid) = f.uid {
                out.push((uid, translate_flags(f.flags())));
            }
        }
        Ok(out)
    }

    async fn expunge(&mut self, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let uidplus = self.capabilities.uidplus;
        let sequence = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let session = self.session()?;
        if uidplus {
            let mut purge = Box::pin(
                session
                    .uid_expunge(&sequence)
                    .await
                    .map_err(|e| protocol_error("purge", e))?,
            );
            while let Some(item) = purge.next().await {
                item.map_err(|e| protocol_error("purge", e))?;
            }
        } else {
            let mut purge = Box::pin(
                session
                    .expunge()
                    .await
                    .map_err(|e| protocol_error("purge", e))?,
            );
            while let Some(item) = purge.next().await {
                item.map_err(|e| protocol_error("purge", e))?;
            }
        }
        Ok(())
    }

    async fn store_flags(&mut self, uids: &[u32], flags: Flags, add: bool) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let sequence = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let commande = if add {
            "+FLAGS.SILENT"
        } else {
            "-FLAGS.SILENT"
        };

        let session = self.session()?;
        let mut flux = session
            .uid_store(sequence, format!("{commande} {}", flags_to_names(flags)))
            .await
            .map_err(|e| protocol_error("modification des drapeaux", e))?;

        // La réponse doit être consommée entièrement, sinon la commande suivante lit
        // les reliquats de celle-ci et le flux se désynchronise.
        while let Some(item) = flux.next().await {
            item.map_err(|e| protocol_error("modification des drapeaux", e))?;
        }
        Ok(())
    }

    async fn create_folder(&mut self, path: &str) -> Result<()> {
        let nom = crate::utf7::encode(path);
        let session = self.session()?;
        let resultat = match session.create(&nom).await {
            Ok(()) => Ok(()),
            // « ALREADYEXISTS », ou n'importe laquelle des formulations que les
            // serveurs emploient pour la même chose. Le but est atteint : le dossier
            // est là. Remonter une erreur ferait échouer un rejeu qui a réussi.
            Err(e) => {
                let dit = e.to_string().to_lowercase();
                if dit.contains("alreadyexists") || dit.contains("already exists") {
                    Ok(())
                } else {
                    Err(protocol_error("création du dossier", e))
                }
            }
        };
        // Subscribed too: clients that show only subscribed folders (many phones,
        // Thunderbird by default) never showed one created here. A refusal is not
        // worth failing for.
        if resultat.is_ok() {
            let _ = session.subscribe(&nom).await;
        }
        resultat
    }

    async fn rename_folder(&mut self, from: &str, to: &str) -> Result<()> {
        let (de, vers) = (crate::utf7::encode(from), crate::utf7::encode(to));
        let session = self.session()?;
        match session.rename(&de, &vers).await {
            Ok(()) => {
                // The subscription follows the name.
                let _ = session.unsubscribe(&de).await;
                let _ = session.subscribe(&vers).await;
                Ok(())
            }
            Err(e) => {
                // Déjà renommé — le rejeu repasse — ou la source a disparu sous ce
                // nom-là. Dans les deux cas le but est atteint et échouer ferait
                // bloquer la file du compte sur une opération qui n'a plus d'objet.
                let dit = e.to_string().to_lowercase();
                if dit.contains("alreadyexists")
                    || dit.contains("already exists")
                    || dit.contains("nonexistent")
                {
                    Ok(())
                } else {
                    Err(protocol_error("renommage du dossier", e))
                }
            }
        }
    }

    async fn delete_folder(&mut self, path: &str) -> Result<()> {
        let nom = crate::utf7::encode(path);
        let session = self.session()?;
        match session.delete(&nom).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let dit = e.to_string().to_lowercase();
                if dit.contains("nonexistent") || dit.contains("does not exist") {
                    Ok(())
                } else {
                    Err(protocol_error("suppression du dossier", e))
                }
            }
        }
    }

    async fn move_messages(&mut self, uids: &[u32], target: &str) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let sequence = uids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let atomique = self.capabilities.r#move;
        let cible = crate::utf7::encode(target);
        let session = self.session()?;

        if atomique {
            session
                .uid_mv(&sequence, &cible)
                .await
                .map_err(|e| protocol_error("déplacement", e))?;
            return Ok(());
        }

        // Sans MOVE : copier, marquer supprimé, purger. Ce n'est pas atomique — une
        // coupure entre les deux laisse un doublon — mais c'est le seul chemin
        // disponible, et le journal d'opérations rendra l'ensemble rejouable.
        session
            .uid_copy(&sequence, &cible)
            .await
            .map_err(|e| protocol_error("copie", e))?;

        // L'emprunt de la session doit prendre fin avant l'appel suivant, qui la
        // réemprunte.
        self.store_flags(uids, Flags::DELETED, true).await?;

        // `UID EXPUNGE` sans UIDPLUS était refusé : le déplacement était abandonné et
        // l'original restait, marqué supprimé, à côté de sa copie. `expunge` retombe
        // alors sur `EXPUNGE`.
        self.expunge(uids).await
    }

    async fn append(&mut self, folder: &str, raw: &[u8], flags: Flags) -> Result<Option<u32>> {
        let noms = flags_to_names(flags);
        let nom = crate::utf7::encode(folder);
        let session = self.session()?;

        session
            .append(&nom, Some(&noms), None, raw)
            .await
            .map_err(|e| protocol_error(&format!("dépôt dans « {folder} »"), e))?;

        // Le serveur ne renvoie l'UID attribué que s'il annonce UIDPLUS, et notre
        // bibliothèque ne l'expose pas ici. L'absence n'est pas un échec : la
        // prochaine synchronisation du dossier retrouvera le message.
        Ok(None)
    }

    async fn idle(&mut self, timeout: Duration) -> Result<IdleOutcome> {
        if !self.capabilities.idle {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: "IDLE non disponible sur ce serveur".into(),
            });
        }

        // L'attente consomme la session : on la retire, puis on la rend. Si l'attente
        // échoue, la connexion est perdue et l'appelant devra se reconnecter.
        let session = self.session.take().ok_or_else(|| Error::Protocol {
            protocol: "IMAP",
            message: "session absente".into(),
        })?;

        let mut handle = session.idle();
        if let Err(e) = handle.init().await {
            return Err(protocol_error("ouverture de IDLE", e));
        }

        let (attente, _interrupteur) = handle.wait_with_timeout(timeout);
        let resultat = attente.await;

        match handle.done().await {
            Ok(session) => self.session = Some(session),
            Err(e) => return Err(protocol_error("fermeture de IDLE", e)),
        }

        use async_imap::extensions::idle::IdleResponse;
        Ok(match resultat {
            Ok(IdleResponse::NewData(_)) => IdleOutcome::Changed,
            Ok(IdleResponse::Timeout) => IdleOutcome::TimedOut,
            Ok(IdleResponse::ManualInterrupt) => IdleOutcome::TimedOut,
            Err(_) => IdleOutcome::Disconnected,
        })
    }

    async fn logout(&mut self) -> Result<()> {
        if let Some(session) = self.session.as_mut() {
            let _ = session.logout().await;
        }
        self.session = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_drapeaux_se_traduisent_dans_les_deux_sens() {
        use async_imap::types::Flag as F;
        let traduits = translate_flags([F::Seen, F::Answered].into_iter());
        assert!(traduits.contains(Flags::SEEN));
        assert!(traduits.contains(Flags::ANSWERED));
        assert!(!traduits.contains(Flags::DRAFT));

        let noms = flags_to_names(Flags::SEEN | Flags::FLAGGED);
        assert!(noms.contains("\\Seen"));
        assert!(noms.contains("\\Flagged"));
    }

    #[test]
    fn les_mots_cles_propres_au_serveur_sont_ignores() {
        use async_imap::types::Flag as F;
        let traduits = translate_flags([F::Custom("$Important".into()), F::Seen].into_iter());
        assert_eq!(traduits, Flags::SEEN);
    }

    #[test]
    fn un_ensemble_vide_de_drapeaux_reste_valide() {
        assert_eq!(flags_to_names(Flags::NONE), "()");
    }

    #[test]
    fn le_role_d_un_dossier_se_deduit_de_ses_attributs() {
        use async_imap::types::NameAttribute as A;
        assert_eq!(folder_kind("Peu importe", &[A::Sent]), FolderKind::Sent);
        assert_eq!(folder_kind("Peu importe", &[A::Trash]), FolderKind::Trash);
        assert_eq!(folder_kind("Tout", &[A::All]), FolderKind::Archive);
        assert_eq!(folder_kind("Racine", &[A::NoSelect]), FolderKind::NoSelect);
        assert_eq!(
            folder_kind("Branche", &[A::Extension("\\NonExistent".into())]),
            FolderKind::NoSelect
        );
    }

    #[test]
    fn les_repertoires_de_dovecot_ne_sont_pas_des_dossiers() {
        assert!(is_server_internal("dovecot/lda-dupes/locks"));
        assert!(is_server_internal("dovecot.lda-dupes.locks"));
        assert!(is_server_internal("INBOX.dovecot.lda-dupes.locks"));
        assert!(is_server_internal("sieve"));
        assert!(!is_server_internal("INBOX.Devis"));
        assert!(!is_server_internal("Clients/sieve"));
    }

    #[test]
    fn le_role_retombe_sur_le_nom_quand_le_serveur_est_muet() {
        // Beaucoup de serveurs n'annoncent aucun attribut spécial.
        assert_eq!(folder_kind("INBOX", &[]), FolderKind::Inbox);
        assert_eq!(folder_kind("INBOX/Sent", &[]), FolderKind::Sent);
        assert_eq!(folder_kind("INBOX.Corbeille", &[]), FolderKind::Trash);
        assert_eq!(folder_kind("Éléments envoyés", &[]), FolderKind::Sent);
        assert_eq!(folder_kind("Clients/2024", &[]), FolderKind::Other);
        assert_eq!(folder_kind("Sent Messages", &[]), FolderKind::Sent);
        assert_eq!(folder_kind("Deleted Messages", &[]), FolderKind::Trash);
        assert_eq!(folder_kind("[Gmail]/Spam", &[]), FolderKind::Junk);
    }

    #[test]
    fn un_dossier_range_dans_un_autre_ne_prend_pas_de_role() {
        // Un libellé « Clients/Trash » devenait la corbeille : Supprimer y rangeait le
        // courrier. « X/Spam » cachait tout son contenu, « Archives/Inbox » rejoignait
        // la boîte de réception.
        assert_eq!(folder_kind("Clients/Trash", &[]), FolderKind::Other);
        assert_eq!(folder_kind("Factures.Spam", &[]), FolderKind::Other);
        assert_eq!(folder_kind("Archives/Inbox", &[]), FolderKind::Other);
        assert_eq!(folder_kind("INBOX.Clients.Archive", &[]), FolderKind::Other);
        // Le séparateur du serveur décide : sur Gmail, un point est une lettre.
        assert_eq!(kind_by_name("john.doe", Some("/")), FolderKind::Other);
        assert_eq!(kind_by_name("INBOX.Trash", Some(".")), FolderKind::Trash);
    }

    #[test]
    fn un_role_annonce_par_le_serveur_n_est_devine_pour_aucun_autre() {
        // Sur Gmail, un libellé « Trash » à la racine passait devant [Gmail]/Trash.
        let liste = vec![
            ("INBOX".to_string(), None, Some("/".to_string())),
            ("Trash".to_string(), None, Some("/".to_string())),
            (
                "[Gmail]/Trash".to_string(),
                Some(FolderKind::Trash),
                Some("/".to_string()),
            ),
            ("Spam".to_string(), None, Some("/".to_string())),
        ];
        assert_eq!(
            assign_kinds(&liste),
            [
                FolderKind::Inbox,
                FolderKind::Other,
                FolderKind::Trash,
                FolderKind::Junk
            ]
        );
    }

    #[test]
    fn les_champs_demandes_couvrent_le_threading() {
        // Sans References ni In-Reply-To, aucun regroupement en fils n'est possible.
        assert!(HEADER_FIELDS.contains("REFERENCES"));
        assert!(HEADER_FIELDS.contains("IN-REPLY-TO"));
        assert!(HEADER_FIELDS.contains("MESSAGE-ID"));
        // Et surtout : jamais le corps.
        assert!(HEADER_FIELDS.contains("HEADER.FIELDS"));
        assert!(!HEADER_FIELDS.contains("BODY.PEEK[]"));
    }

    #[test]
    fn un_refus_d_identifiants_ne_se_confond_pas_avec_une_panne() {
        // La distinction commande le comportement de l'ordonnanceur.
        let auth = translate_login_error(
            async_imap::error::Error::Bad("AUTHENTICATIONFAILED".into()),
            "a@x.fr",
        );
        assert!(auth.needs_user_action());
        assert!(!auth.is_transient());

        let reseau = translate_login_error(
            async_imap::error::Error::Bad("server busy".into()),
            "a@x.fr",
        );
        assert!(reseau.is_transient());
    }

    #[test]
    fn une_connexion_coupee_se_retente_un_refus_definitif_non() {
        // Le rejeu abandonne pour de bon ce qui n'est pas passager : une coupure
        // réseau classée « protocole » perdait l'action de l'utilisateur.
        use async_imap::error::Error as E;
        let coupe = protocol_error("déplacement", E::ConnectionLost);
        assert!(coupe.is_transient(), "{coupe}");
        let io = protocol_error(
            "déplacement",
            E::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
        );
        assert!(io.is_transient(), "{io}");
        let occupe = protocol_error("déplacement", E::No("[INUSE] mailbox locked".into()));
        assert!(occupe.is_transient(), "{occupe}");
        let freine = protocol_error("déplacement", E::No("[THROTTLED] slow down".into()));
        assert!(freine.is_transient(), "{freine}");

        let absent = protocol_error("sélection", E::No("[NONEXISTENT] no such".into()));
        assert!(!absent.is_transient());
        // Le moteur reconnaît un dossier disparu à ce code : il doit survivre.
        assert!(absent.to_string().contains("[NONEXISTENT]"));
    }

    #[test]
    fn le_mecanisme_xoauth2_respecte_le_format_attendu() {
        use async_imap::Authenticator;
        let mut m = XOAuth2 {
            user: "a@x.fr".into(),
            token: "jeton".into(),
            sent: false,
        };
        let reponse = m.process(b"");
        assert_eq!(reponse, "user=a@x.fr\x01auth=Bearer jeton\x01\x01");
        // The server's reason for a refusal is answered with an empty line.
        assert_eq!(m.process(b"{\"status\":\"400\"}"), "");
    }

    /// Plays the server's side of a sign-in, as Gmail does: a greeting, a `+` after
    /// `AUTHENTICATE`, then `answer` once the client has replied. Returns the lines the
    /// client sent.
    async fn gmail_server(
        stream: tokio::io::DuplexStream,
        answer_to_token: &'static [&'static str],
    ) -> Vec<String> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (lecture, mut ecriture) = tokio::io::split(stream);
        let mut lignes = BufReader::new(lecture).lines();
        let mut recues = Vec::new();

        ecriture
            .write_all(b"* OK Gimap ready for requests from 192.0.2.1\r\n")
            .await
            .unwrap();
        let commande = lignes.next_line().await.unwrap().unwrap();
        let etiquette = commande.split(' ').next().unwrap().to_string();
        recues.push(commande);
        ecriture.write_all(b"+ \r\n").await.unwrap();

        for reponse in answer_to_token {
            recues.push(lignes.next_line().await.unwrap().unwrap_or_default());
            let reponse = reponse.replace("TAG", &etiquette);
            ecriture.write_all(reponse.as_bytes()).await.unwrap();
        }
        recues
    }

    #[tokio::test]
    async fn a_google_account_signs_in_past_the_greeting() {
        let (client, serveur) = tokio::io::duplex(4096);
        let serveur = tokio::spawn(gmail_server(
            serveur,
            &["* CAPABILITY IMAP4rev1 IDLE\r\nTAG OK a@gmail.com authenticated (Success)\r\n"],
        ));

        let identifiants = Credentials::OAuth2 {
            user: "a@example.com".into(),
            token: "jeton".into(),
        };
        let session = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sign_in(client, &identifiants, false),
        )
        .await
        .expect("the sign-in waited for an answer it had already been given");
        assert!(session.is_ok(), "{:?}", session.err());

        let recues = serveur.await.unwrap();
        assert!(recues[0].ends_with("AUTHENTICATE XOAUTH2"));
        assert!(!recues[1].is_empty(), "the token was sent");
    }

    #[tokio::test]
    async fn a_refused_google_token_says_so() {
        let (client, serveur) = tokio::io::duplex(4096);
        let serveur = tokio::spawn(gmail_server(
            serveur,
            &[
                "+ eyJzdGF0dXMiOiI0MDAiLCJzY2hlbWVzIjoiQmVhcmVyIn0=\r\n",
                "TAG NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n",
            ],
        ));

        let identifiants = Credentials::OAuth2 {
            user: "a@example.com".into(),
            token: "perime".into(),
        };
        let resultat = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sign_in(client, &identifiants, false),
        )
        .await
        .expect("a refusal must come back, not hang");
        let Err(erreur) = resultat else {
            panic!("the token was refused");
        };
        assert!(erreur.needs_user_action(), "{erreur}");

        let recues = serveur.await.unwrap();
        assert_eq!(recues[2], "", "the reason is answered with an empty line");
    }

    #[tokio::test]
    async fn a_password_account_still_signs_in() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (client, serveur) = tokio::io::duplex(4096);
        let serveur = tokio::spawn(async move {
            let (lecture, mut ecriture) = tokio::io::split(serveur);
            let mut lignes = BufReader::new(lecture).lines();
            ecriture
                .write_all(b"* OK Dovecot ready.\r\n")
                .await
                .unwrap();
            let commande = lignes.next_line().await.unwrap().unwrap();
            let etiquette = commande.split(' ').next().unwrap().to_string();
            ecriture
                .write_all(format!("{etiquette} OK Logged in\r\n").as_bytes())
                .await
                .unwrap();
            commande
        });

        let identifiants = Credentials::Password {
            user: "a@example.com".into(),
            password: "secret".into(),
        };
        let session = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sign_in(client, &identifiants, false),
        )
        .await
        .unwrap();
        assert!(session.is_ok(), "{:?}", session.err());
        assert!(serveur.await.unwrap().contains("LOGIN"));
    }

    #[tokio::test]
    async fn a_server_that_will_not_encrypt_is_refused() {
        // Port 143 is upgraded with STARTTLS; one that refuses is never sent the
        // password in the clear.
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let ecoute = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = ecoute.local_addr().unwrap().port();
        let serveur = tokio::spawn(async move {
            let (flux, _) = ecoute.accept().await.unwrap();
            let (lecture, mut ecriture) = tokio::io::split(flux);
            let mut lignes = BufReader::new(lecture).lines();
            ecriture.write_all(b"* OK ready\r\n").await.unwrap();
            let commande = lignes.next_line().await.unwrap().unwrap();
            ecriture
                .write_all(b"S1 NO STARTTLS not available\r\n")
                .await
                .unwrap();
            commande
        });

        let erreur = RustlsConnector::new()
            .connect(
                &Endpoint::starttls("127.0.0.1", port),
                &Credentials::Password {
                    user: "a@example.com".into(),
                    password: "secret".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(erreur.to_string().contains("STARTTLS refused"), "{erreur}");
        assert_eq!(serveur.await.unwrap(), "S1 STARTTLS");
    }

    #[test]
    fn le_connecteur_se_construit_avec_les_racines_embarquees() {
        // Une machine mal configurée produirait sinon cent échecs inexplicables.
        let _ = RustlsConnector::new();
    }
}
