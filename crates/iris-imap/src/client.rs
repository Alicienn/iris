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
use async_imap::imap_proto::{AttributeValue, MailboxDatum, MessageSection, Response, SectionPath};
use async_trait::async_trait;
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
        let (session, capabilities) = self.open(endpoint, credentials).await?;
        Ok(Box::new(ImapClient {
            session: Some(session),
            capabilities,
            reopen: Some(Reopen {
                connector: self.clone(),
                endpoint: endpoint.clone(),
                credentials: credentials.clone(),
            }),
            selected: None,
            broken: false,
        }))
    }
}

impl RustlsConnector {
    /// A signed-in session and what its server can do.
    async fn open(
        &self,
        endpoint: &Endpoint,
        credentials: &Credentials,
    ) -> Result<(Session, Capabilities)> {
        // After STARTTLS the greeting has already been read, in the clear, and the
        // server says nothing more until spoken to.
        let (tls, accueil_lu) = if endpoint.tls_immediate {
            (self.tls_stream(endpoint).await?, false)
        } else {
            (self.starttls_stream(endpoint).await?, true)
        };
        let mut session = sign_in(tls, credentials, accueil_lu).await?;
        let caps = session
            .capabilities()
            .await
            .map_err(|e| protocol_error("lecture des capacités", e))?;
        let capabilities = Capabilities::from_names(caps.iter().filter_map(capability_name));
        Ok((session, capabilities))
    }
}

/// What a connection needs to be opened again.
#[derive(Debug, Clone)]
struct Reopen {
    connector: RustlsConnector,
    endpoint: Endpoint,
    credentials: Credentials,
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
    use async_imap::imap_proto::Status;

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
        // Office 365's refusal of an OAuth token: `NO AUTHENTICATE failed.` It passed
        // for a network failure, and signing in again was never offered.
        "authenticate failed",
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
    /// How to open it again once it is broken; `None` where it was not opened here.
    reopen: Option<Reopen>,
    /// The folder last selected, selected again on a new connection.
    selected: Option<String>,
    /// Cut, or holding an answer the library could not read: every later read on it
    /// fails the same way (the bytes stay in its buffer), so the next command opens a
    /// new one first. Going on with a dead connection made every following folder
    /// "empty", and the pass still ended as a success.
    broken: bool,
}

/// Words put in the error for an answer the library could not read.
const UNREADABLE: &str = "an answer the IMAP library could not read";

impl ImapClient {
    fn session(&mut self) -> Result<&mut Session> {
        self.session.as_mut().ok_or_else(|| Error::Protocol {
            protocol: "IMAP",
            message: "session en cours d'attente".into(),
        })
    }

    /// Opens the connection again when the last command broke it, and selects the
    /// folder that was selected.
    async fn revive(&mut self) -> Result<()> {
        if !self.broken {
            return Ok(());
        }
        let Some(r) = self.reopen.clone() else {
            return Err(Error::network("the connection to the server was lost"));
        };
        // Dropped, not logged out of: a broken stream answers nothing.
        self.session = None;
        let (session, capabilities) = r.connector.open(&r.endpoint, &r.credentials).await?;
        self.session = Some(session);
        self.capabilities = capabilities;
        self.broken = false;
        tracing::info!(host = %r.endpoint.host, "connection opened again");
        if let Some(chemin) = self.selected.clone() {
            let commande = select_command(&chemin, capabilities.condstore);
            let session = self.session()?;
            let resultat = run_checked(session, "sélection", &commande, |_| {}).await;
            self.note(&resultat);
            resultat?;
        }
        Ok(())
    }

    /// Remembers that a command broke the connection.
    fn note<T>(&mut self, resultat: &Result<T>) {
        if let Err(e) = resultat {
            if breaks_connection(e) {
                self.broken = true;
            }
        }
    }

    /// Runs a command whose whole answer is read here, its verdict checked.
    async fn run(
        &mut self,
        what: &str,
        command: &str,
        each: impl FnMut(&Response<'_>) + Send,
    ) -> Result<()> {
        self.revive().await?;
        let session = self.session()?;
        let resultat = run_checked(session, what, command, each).await;
        self.note(&resultat);
        resultat
    }

    /// `UID SEARCH`, its UIDs in order.
    async fn search(&mut self, what: &str, criteria: &str) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        self.run(what, &format!("UID SEARCH {criteria}"), |r| {
            if let Response::MailboxData(MailboxDatum::Search(uids)) = r {
                out.extend_from_slice(uids);
            }
        })
        .await?;
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }

    /// The flags a `UID FETCH … (UID FLAGS)` command answers with.
    async fn flags_of(&mut self, what: &str, command: &str) -> Result<Vec<(u32, Flags)>> {
        let mut out = Vec::new();
        self.run(what, command, |r| {
            let Response::Fetch(_, attrs) = r else { return };
            let f = fetched(attrs);
            if let (Some(uid), Some(flags)) = (f.uid, f.flags) {
                out.push((uid, flags));
            }
        })
        .await?;
        Ok(out)
    }
}

/// The server's way of saying the name is taken.
fn says_already_exists(e: &Error) -> bool {
    let dit = e.to_string().to_lowercase();
    dit.contains("alreadyexists") || dit.contains("already exists")
}

/// The server's way of saying there is no such folder.
fn says_nonexistent(e: &Error) -> bool {
    let dit = e.to_string().to_lowercase();
    dit.contains("nonexistent") || dit.contains("does not exist") || dit.contains("doesn't exist")
}

/// A cut connection, or one left unreadable: either way it cannot be used again.
fn breaks_connection(e: &Error) -> bool {
    match e {
        Error::Network(_) | Error::Io(_) => true,
        Error::Protocol { message, .. } => message.contains(UNREADABLE),
        _ => false,
    }
}

/// Sends a command and reads its whole answer, up to and including the server's
/// verdict, which is checked.
///
/// async-imap's own readers drop that verdict for `FETCH`, `SEARCH`, `STORE` and
/// `EXPUNGE`, and take a connection closed mid-answer for the end of it. A refused or
/// cut `UID SEARCH` read as "no message left", and the folder was emptied here; a
/// `FETCH` the server refused half-way read as a whole one, and what it left out was
/// never asked for again; a refused `STORE` counted as done.
async fn run_checked(
    session: &mut Session,
    what: &str,
    command: &str,
    mut each: impl FnMut(&Response<'_>) + Send,
) -> Result<()> {
    use async_imap::error::Error as E;
    use async_imap::imap_proto::Status;

    let tag = tokio::time::timeout(SILENCE, session.run_command(command))
        .await
        .map_err(|_| silent(what))?
        .map_err(|e| protocol_error(what, e))?;
    loop {
        let lu = tokio::time::timeout(SILENCE, session.read_response())
            .await
            .map_err(|_| silent(what))?;
        let reponse = match lu {
            Ok(Some(r)) => r,
            Ok(None) => {
                return Err(Error::network(format!(
                    "{what} : the server closed the connection before answering"
                )))
            }
            Err(e) if e.to_string().contains("during parsing of") => {
                return Err(Error::Protocol {
                    protocol: "IMAP",
                    message: format!("{what} : {UNREADABLE}"),
                })
            }
            Err(e) => return Err(Error::network(format!("{what} : {e}"))),
        };
        match reponse.parsed() {
            Response::Done {
                tag: t,
                status,
                code,
                information,
            } if *t == tag => {
                if *status == Status::Ok {
                    return Ok(());
                }
                let texte = format!("code: {code:?}, info: {information:?}");
                let refus = if *status == Status::No {
                    E::No(texte)
                } else {
                    E::Bad(texte)
                };
                return Err(protocol_error(what, refus));
            }
            Response::Data {
                status: Status::Bye,
                information,
                ..
            } => {
                return Err(Error::network(format!(
                    "{what} : the server is closing the connection ({})",
                    information.as_deref().unwrap_or("no reason given")
                )))
            }
            autre => each(autre),
        }
    }
}

/// How long a server may say nothing in the middle of an answer. Only the whole
/// account's pass had a limit (ten minutes): a server that went quiet held its pass,
/// and the round every other mailbox waited for, that long.
const SILENCE: Duration = Duration::from_secs(120);

fn silent(what: &str) -> Error {
    Error::network(format!(
        "{what} : the server said nothing for {} seconds",
        SILENCE.as_secs()
    ))
}

/// A name as an IMAP quoted string.
fn quoted(name: &str) -> String {
    let mut sortie = String::with_capacity(name.len() + 2);
    sortie.push('"');
    for c in name.chars().filter(|c| !matches!(c, '\r' | '\n')) {
        if matches!(c, '"' | '\\') {
            sortie.push('\\');
        }
        sortie.push(c);
    }
    sortie.push('"');
    sortie
}

/// `SELECT`, asking for change numbers where the server keeps them.
fn select_command(path: &str, condstore: bool) -> String {
    let nom = quoted(&crate::utf7::encode(path));
    if condstore {
        format!("SELECT {nom} (CONDSTORE)")
    } else {
        format!("SELECT {nom}")
    }
}

/// UIDs as a sequence set, runs written as ranges: `3:7,9,12:14`.
fn uid_set(uids: &[u32]) -> String {
    let mut tries = uids.to_vec();
    tries.sort_unstable();
    tries.dedup();
    let mut morceaux: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tries.len() {
        let debut = tries[i];
        let mut fin = debut;
        while i + 1 < tries.len() && tries[i + 1] == fin + 1 {
            i += 1;
            fin = tries[i];
        }
        morceaux.push(if debut == fin {
            debut.to_string()
        } else {
            format!("{debut}:{fin}")
        });
        i += 1;
    }
    morceaux.join(",")
}

/// One message's answer to a `FETCH`.
#[derive(Debug, Default)]
struct Fetched {
    uid: Option<u32>,
    flags: Option<Flags>,
    date: Option<Timestamp>,
    size: Option<u32>,
    header: Option<Vec<u8>>,
    text: Option<Vec<u8>>,
    body: Option<Vec<u8>>,
}

/// Reads what a `FETCH` answer carries.
fn fetched(attrs: &[AttributeValue<'_>]) -> Fetched {
    let mut f = Fetched::default();
    for a in attrs {
        match a {
            AttributeValue::Uid(uid) => f.uid = Some(*uid),
            AttributeValue::Flags(noms) => {
                f.flags = Some(translate_flags(
                    noms.iter()
                        .map(|n| async_imap::types::Flag::from(n.as_ref())),
                ))
            }
            AttributeValue::InternalDate(date) => {
                f.date = chrono::DateTime::parse_from_str(date.trim(), "%d-%b-%Y %H:%M:%S %z")
                    .ok()
                    .map(|d| Timestamp::from_millis(d.timestamp_millis()))
            }
            AttributeValue::Rfc822Size(taille) => f.size = Some(*taille),
            AttributeValue::BodySection {
                section: Some(SectionPath::Full(MessageSection::Header)),
                data: Some(d),
                ..
            }
            | AttributeValue::Rfc822Header(Some(d)) => f.header = Some(d.to_vec()),
            AttributeValue::BodySection {
                section: Some(SectionPath::Full(MessageSection::Text)),
                data: Some(d),
                ..
            }
            | AttributeValue::Rfc822Text(Some(d)) => f.text = Some(d.to_vec()),
            AttributeValue::BodySection {
                section: None,
                data: Some(d),
                ..
            }
            | AttributeValue::Rfc822(Some(d)) => f.body = Some(d.to_vec()),
            _ => {}
        }
    }
    f
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
            F::Custom(ref k)
                if k.eq_ignore_ascii_case("$NotJunk") || k.eq_ignore_ascii_case("NonJunk") =>
            {
                out.with(Flags::NOT_JUNK)
            }
            // Les autres mots-clés propres au serveur ne nous concernent pas.
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
    if flags.contains(Flags::NOT_JUNK) {
        noms.push("$NotJunk");
    }
    format!("({})", noms.join(" "))
}

/// A folder in another user's or a shared namespace, by the names servers give them.
fn is_shared(name: &str) -> bool {
    let premier = name
        .split(['/', '.'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        premier.as_str(),
        "shared folders" | "other users" | "public folders" | "#shared" | "#public" | "#users"
    )
}

/// A capability's own name, `CONDSTORE`, `MOVE`… Read through `Debug` it was
/// `ATOM("CONDSTORE")`, which matched nothing: no server was ever seen to have
/// CONDSTORE, MOVE or UIDPLUS, and every move ended in a bare `EXPUNGE`.
fn capability_name(c: &async_imap::types::Capability) -> Option<&str> {
    use async_imap::types::Capability as C;
    match c {
        C::Imap4rev1 => Some("IMAP4REV1"),
        C::Atom(nom) => Some(nom.as_str()),
        C::Auth(_) => None,
    }
}

/// Un répertoire de travail du serveur, que LIST montre comme une boîte.
///
/// Quand Dovecot range le courrier à la racine du répertoire personnel, ses propres
/// fichiers y deviennent des dossiers : `dovecot.lda-dupes.locks` (les verrous de la
/// détection de doublons à la livraison) et `sieve` (les scripts de filtrage). Aucun
/// ne contient de courrier, et le serveur les recrée à chaque livraison : les montrer
/// offrait une suppression qui ne tenait jamais.
///
/// Only Dovecot's own file names are hidden: a folder the user named `dovecot`, or
/// `dovecot-notes`, is theirs.
fn is_server_internal(name: &str) -> bool {
    const FICHIERS: [&str; 6] = ["lda-dupes", "index", "list", "sieve", "svbin", "mailbox"];
    const PREFIXES: [&str; 5] = [
        "dovecot-uidlist",
        "dovecot-keywords",
        "dovecot-uidvalidity",
        "dovecot-acl",
        "dovecot-virtual",
    ];
    let mut segments = name
        .split(['.', '/'])
        .skip_while(|s| s.eq_ignore_ascii_case("INBOX"));
    let premier = segments.next().unwrap_or("");
    let interne = match premier {
        "dovecot" => segments.next().is_some_and(|s| FICHIERS.contains(&s)),
        _ => PREFIXES.iter().any(|p| premier.starts_with(p)),
    };
    interne || name == "sieve"
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
            A::Archive => return Some(FolderKind::Archive),
            // Gmail's All Mail, where archived mail lives; elsewhere a virtual
            // folder, often read-only, which `list_folders` leaves out.
            A::All => return Some(FolderKind::Archive),
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
    // Accents folded, written as one character or as a letter and a mark (a Mac's
    // `envoyés` arrives decomposed): `Envoyes`, `Éléments supprimés` and their kin
    // were not recognised.
    let n: String = name
        .to_lowercase()
        .chars()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' | 'ä' => 'a',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            autre => autre,
        })
        .collect();
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
        "sent" | "sent items" | "sent messages" | "sent mail" | "envoyes" | "elements envoyes"
        | "messages envoyes" => FolderKind::Sent,
        "drafts" | "draft" | "brouillons" | "brouillon" => FolderKind::Drafts,
        "trash" | "deleted items" | "deleted messages" | "deleted" | "bin" | "corbeille"
        | "elements supprimes" | "messages supprimes" => FolderKind::Trash,
        "junk"
        | "spam"
        | "junk e-mail"
        | "junk email"
        | "junk mail"
        | "indesirables"
        | "courrier indesirable"
        | "pourriel"
        | "pourriels" => FolderKind::Junk,
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
        // Read to the server's verdict: a list cut after `INBOX` passed for the whole
        // of it, and every other folder was forgotten here with its mail.
        let mut listes = Vec::new();
        self.run("liste des dossiers", "LIST \"\" \"*\"", |r| {
            let Response::MailboxData(MailboxDatum::List {
                name_attributes,
                delimiter,
                name,
            }) = r
            else {
                return;
            };
            let special = special_use(name_attributes);
            let lisible = crate::utf7::decode(name);
            // `\All` outside Gmail: a view over every folder (Dovecot's virtual All,
            // Fastmail's), often read-only, which Archive then tried to move into.
            let tout_virtuel = name_attributes
                .iter()
                .any(|a| matches!(a, async_imap::types::NameAttribute::All))
                && !lisible.starts_with("[Gmail]/")
                && !lisible.starts_with("[Google Mail]/");
            if special == Some(FolderKind::NoSelect)
                || is_virtual(name_attributes)
                || tout_virtuel
                || is_server_internal(&lisible)
            {
                return;
            }
            listes.push((lisible, special, delimiter.as_deref().map(str::to_string)));
        })
        .await?;

        // Other people's and shared folders, only those subscribed to: a shared
        // mailbox of thousands of messages was synced in full because the server
        // listed it. One's own folders all stay, subscribed or not.
        if listes.iter().any(|(nom, _, _)| is_shared(nom)) {
            let mut abonnes = std::collections::HashSet::new();
            let lu = self
                .run("abonnements", "LSUB \"\" \"*\"", |r| {
                    if let Response::MailboxData(MailboxDatum::List { name, .. }) = r {
                        abonnes.insert(crate::utf7::decode(name));
                    }
                })
                .await;
            // Not answered: everything is kept, as before.
            if lu.is_ok() {
                listes.retain(|(nom, _, _)| !is_shared(nom) || abonnes.contains(nom));
            }
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
        use async_imap::imap_proto::{ResponseCode, Status};

        let commande = select_command(path, self.capabilities.condstore);
        let (mut validite, mut suivant, mut nombre, mut modseq) = (0, None, 0, 0);
        self.run(
            &format!("sélection de « {path} »"),
            &commande,
            |r| match r {
                Response::Data {
                    status: Status::Ok,
                    code: Some(code),
                    ..
                } => match code {
                    ResponseCode::UidValidity(v) => validite = *v,
                    ResponseCode::UidNext(n) => suivant = Some(*n),
                    ResponseCode::HighestModSeq(m) => modseq = *m,
                    _ => {}
                },
                Response::MailboxData(MailboxDatum::Exists(n)) => nombre = *n,
                _ => {}
            },
        )
        .await?;
        self.selected = Some(path.to_string());

        // A server that does not say what the next UID will be (a few do not) had
        // nothing ever fetched: the next UID was taken as 1, so nothing lay below it.
        // The highest UID there is says it as well.
        let suivant = match suivant {
            Some(n) => n,
            None if nombre > 0 => {
                let mut plus_haut = 0;
                self.run("plus grand UID", "UID FETCH * (UID)", |r| {
                    if let Response::Fetch(_, attrs) = r {
                        if let Some(uid) = fetched(attrs).uid {
                            plus_haut = plus_haut.max(uid);
                        }
                    }
                })
                .await?;
                plus_haut + 1
            }
            None => 1,
        };

        Ok(SelectedFolder {
            uid_validity: validite,
            uid_next: suivant,
            exists: nombre,
            highest_modseq: modseq,
        })
    }

    async fn fetch_envelopes(&mut self, range: UidRange) -> Result<Vec<RawMessage>> {
        let mut out = Vec::new();
        let commande = format!("UID FETCH {} {HEADER_FIELDS}", range.to_sequence());
        self.run("récupération des en-têtes", &commande, |r| {
            let Response::Fetch(_, attrs) = r else { return };
            let f = fetched(attrs);
            // Une réponse sans UID est inexploitable : nous n'aurions aucun moyen de
            // la relier à quoi que ce soit.
            let Some(uid) = f.uid else { return };
            // Outside what was asked: the server telling of a change elsewhere.
            if uid < range.from || uid > range.to {
                return;
            }
            out.push(RawMessage {
                uid,
                flags: f.flags.unwrap_or(Flags::NONE),
                internal_date: f.date.unwrap_or(Timestamp::EPOCH),
                size: f.size.unwrap_or(0) as u64,
                content: headers_and_start(f.header.as_deref(), f.text.as_deref()),
            });
        })
        .await?;
        Ok(out)
    }

    async fn fetch_body(&mut self, uid: u32) -> Result<Vec<u8>> {
        let mut corps = None;
        self.run(
            "récupération du corps",
            &format!("UID FETCH {uid} (UID BODY.PEEK[])"),
            |r| {
                let Response::Fetch(_, attrs) = r else { return };
                let f = fetched(attrs);
                if f.uid == Some(uid) || (f.uid.is_none() && corps.is_none()) {
                    if let Some(b) = f.body {
                        corps = Some(b);
                    }
                }
            },
        )
        .await?;
        corps.ok_or_else(|| Error::Protocol {
            protocol: "IMAP",
            message: format!("corps de l'UID {uid} absent"),
        })
    }

    async fn existing_uids(&mut self, range: UidRange) -> Result<Vec<u32>> {
        let mut out = self
            .search("recherche des UID", &format!("UID {}", range.to_sequence()))
            .await?;
        out.retain(|u| *u >= range.from && *u <= range.to);
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
        self.search(
            "recherche du message",
            &format!("HEADER Message-ID \"<{propre}>\""),
        )
        .await
    }

    async fn flags_changed_since(&mut self, modseq: u64) -> Result<Vec<(u32, Flags)>> {
        if !self.capabilities.condstore {
            return Err(Error::Protocol {
                protocol: "IMAP",
                message: "CONDSTORE non disponible sur ce serveur".into(),
            });
        }
        self.flags_of(
            "drapeaux modifiés",
            &format!("UID FETCH 1:* (UID FLAGS) (CHANGEDSINCE {modseq})"),
        )
        .await
    }

    async fn fetch_flags(&mut self, range: UidRange) -> Result<Vec<(u32, Flags)>> {
        let mut out = self
            .flags_of(
                "lecture des drapeaux",
                &format!("UID FETCH {} (UID FLAGS)", range.to_sequence()),
            )
            .await?;
        out.retain(|(u, _)| *u >= range.from && *u <= range.to);
        Ok(out)
    }

    async fn expunge(&mut self, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        if self.capabilities.uidplus {
            return self
                .run("purge", &format!("UID EXPUNGE {}", uid_set(uids)), |_| {})
                .await;
        }

        // Without UIDPLUS only the whole folder can be purged, and with it whatever
        // another client had only marked deleted (Thunderbird's and Outlook's "mark as
        // deleted" mode), gone for good. Those are unmarked for the time of the purge,
        // then marked again.
        let mut autres = self.search("purge", "DELETED").await?;
        autres.retain(|u| !uids.contains(u));
        self.store_flags(&autres, Flags::DELETED, false).await?;
        let purge = self.run("purge", "EXPUNGE", |_| {}).await;
        let remis = self.store_flags(&autres, Flags::DELETED, true).await;
        purge?;
        remis
    }

    async fn store_flags(&mut self, uids: &[u32], flags: Flags, add: bool) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let commande = if add {
            "+FLAGS.SILENT"
        } else {
            "-FLAGS.SILENT"
        };
        // Read to its verdict: a refused STORE counted as done, and the action left
        // the journal although the server had not carried it out.
        self.run(
            "modification des drapeaux",
            &format!(
                "UID STORE {} {commande} {}",
                uid_set(uids),
                flags_to_names(flags)
            ),
            |_| {},
        )
        .await
    }

    async fn create_folder(&mut self, path: &str) -> Result<()> {
        let nom = quoted(&crate::utf7::encode(path));
        let resultat = match self
            .run("création du dossier", &format!("CREATE {nom}"), |_| {})
            .await
        {
            // « ALREADYEXISTS », ou n'importe laquelle des formulations que les
            // serveurs emploient pour la même chose. Le but est atteint : le dossier
            // est là. Remonter une erreur ferait échouer un rejeu qui a réussi.
            Err(e) if says_already_exists(&e) => Ok(()),
            autre => autre,
        };
        // Subscribed too: clients that show only subscribed folders (many phones,
        // Thunderbird by default) never showed one created here. A refusal is not
        // worth failing for.
        if resultat.is_ok() {
            let _ = self
                .run("abonnement", &format!("SUBSCRIBE {nom}"), |_| {})
                .await;
        }
        resultat
    }

    async fn rename_folder(&mut self, from: &str, to: &str) -> Result<()> {
        let de = quoted(&crate::utf7::encode(from));
        let vers = quoted(&crate::utf7::encode(to));
        match self
            .run(
                "renommage du dossier",
                &format!("RENAME {de} {vers}"),
                |_| {},
            )
            .await
        {
            Ok(()) => {
                // The subscription follows the name.
                let _ = self
                    .run("abonnement", &format!("UNSUBSCRIBE {de}"), |_| {})
                    .await;
                let _ = self
                    .run("abonnement", &format!("SUBSCRIBE {vers}"), |_| {})
                    .await;
                Ok(())
            }
            // La source a disparu sous ce nom-là : le rejeu repasse sur un renommage
            // fait. Échouer bloquerait la file du compte sur une opération qui n'a plus
            // d'objet.
            Err(e) if says_nonexistent(&e) => Ok(()),
            // The new name taken: done already only if the old one is gone. Renaming
            // onto a folder of the same name counted as done, and nothing was renamed.
            Err(e) if says_already_exists(&e) => {
                let mut source_la = false;
                self.run("liste des dossiers", &format!("LIST \"\" {de}"), |r| {
                    if matches!(r, Response::MailboxData(MailboxDatum::List { .. })) {
                        source_la = true;
                    }
                })
                .await?;
                if source_la {
                    Err(Error::Config(format!(
                        "a folder named “{to}” already exists"
                    )))
                } else {
                    Ok(())
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn delete_folder(&mut self, path: &str) -> Result<()> {
        let nom = quoted(&crate::utf7::encode(path));
        match self
            .run("suppression du dossier", &format!("DELETE {nom}"), |_| {})
            .await
        {
            Ok(()) => {
                // A subscription to a folder that is gone shows as a broken folder in
                // other clients.
                let _ = self
                    .run("abonnement", &format!("UNSUBSCRIBE {nom}"), |_| {})
                    .await;
                Ok(())
            }
            Err(e) if says_nonexistent(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn move_messages(&mut self, uids: &[u32], target: &str) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let sequence = uid_set(uids);
        let atomique = self.capabilities.r#move;
        let cible = quoted(&crate::utf7::encode(target));

        if atomique {
            return self
                .run(
                    "déplacement",
                    &format!("UID MOVE {sequence} {cible}"),
                    |_| {},
                )
                .await;
        }

        // Sans MOVE : copier, marquer supprimé, purger. Ce n'est pas atomique — une
        // coupure entre les deux laisse un doublon — mais c'est le seul chemin
        // disponible, et le journal d'opérations rendra l'ensemble rejouable.
        self.run("copie", &format!("UID COPY {sequence} {cible}"), |_| {})
            .await?;

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
        self.revive().await?;
        let session = self.session()?;

        let resultat = session
            .append(&nom, Some(&noms), None, raw)
            .await
            .map_err(|e| protocol_error(&format!("dépôt dans « {folder} »"), e));
        self.note(&resultat);
        resultat?;

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
        self.revive().await?;
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
    fn uids_go_out_as_ranges() {
        assert_eq!(uid_set(&[9, 3, 4, 5, 12, 13, 4]), "3:5,9,12:13");
        assert_eq!(uid_set(&[7]), "7");
    }

    #[test]
    fn a_name_is_quoted_with_its_quotes_escaped() {
        assert_eq!(quoted("Devis"), "\"Devis\"");
        assert_eq!(quoted("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn a_fetch_answer_is_read_whole() {
        use std::borrow::Cow;
        let attrs = vec![
            AttributeValue::Uid(42),
            AttributeValue::Flags(vec![Cow::Borrowed("\\Seen"), Cow::Borrowed("$Label1")]),
            AttributeValue::InternalDate(Cow::Borrowed(" 7-Oct-2026 09:30:00 +0200")),
            AttributeValue::Rfc822Size(1234),
            AttributeValue::BodySection {
                section: Some(SectionPath::Full(MessageSection::Header)),
                index: None,
                data: Some(Cow::Borrowed(b"Subject: Devis\r\n\r\n")),
            },
            AttributeValue::BodySection {
                section: Some(SectionPath::Full(MessageSection::Text)),
                index: Some(0),
                data: Some(Cow::Borrowed(b"Bonjour")),
            },
        ];
        let f = fetched(&attrs);
        assert_eq!(f.uid, Some(42));
        assert_eq!(f.flags, Some(Flags::SEEN));
        assert_eq!(f.size, Some(1234));
        assert_eq!(f.date, Some(Timestamp::from_millis(1_791_358_200_000)));
        assert_eq!(
            headers_and_start(f.header.as_deref(), f.text.as_deref()),
            b"Subject: Devis\r\n\r\nBonjour"
        );
    }

    #[test]
    fn a_server_s_capabilities_are_read_by_their_names() {
        use async_imap::types::Capability as C;
        let annonce = [
            C::Imap4rev1,
            C::Atom("CONDSTORE".into()),
            C::Atom("MOVE".into()),
            C::Atom("UIDPLUS".into()),
            C::Atom("IDLE".into()),
            C::Auth("PLAIN".into()),
        ];
        let c = Capabilities::from_names(annonce.iter().filter_map(capability_name));
        assert!(c.condstore && c.r#move && c.uidplus && c.idle, "{c:?}");
        assert!(!c.qresync);
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
    fn a_folder_the_user_named_dovecot_is_shown() {
        assert!(!is_server_internal("dovecot"));
        assert!(!is_server_internal("dovecot-notes"));
        assert!(!is_server_internal("INBOX.dovecot.Factures"));
        assert!(is_server_internal("dovecot-uidlist"));
        assert!(is_server_internal("dovecot.index.log"));
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
        // Accents written or not, and decomposed as a Mac writes them.
        assert_eq!(kind_by_name("Envoyes", None), FolderKind::Sent);
        assert_eq!(kind_by_name("Envoye\u{301}s", None), FolderKind::Sent);
        assert_eq!(kind_by_name("Messages supprimés", None), FolderKind::Trash);
        assert_eq!(kind_by_name("Pourriels", None), FolderKind::Junk);
        assert_eq!(kind_by_name("Junk Mail", None), FolderKind::Junk);
        assert_eq!(kind_by_name("Brouillon", None), FolderKind::Drafts);
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
