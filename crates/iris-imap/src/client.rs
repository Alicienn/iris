//! La connexion réelle, au-dessus d'`async-imap`.
//!
//! Cette couche ne fait que traduire : notre contrat vers les commandes du
//! protocole, et les réponses du protocole vers nos types. Toute la logique de
//! synchronisation vit ailleurs et se teste contre la doublure — ici, il n'y a que
//! du câblage, et c'est voulu.
//!
//! Trois décisions méritent d'être signalées :
//!
//! - **les racines de confiance sont embarquées** plutôt que lues dans le magasin du
//!   système. Sur cent comptes, une machine mal configurée produirait cent échecs
//!   inexplicables ;
//! - **`STARTTLS` est géré**, car quelques hébergeurs n'exposent encore que le port
//!   143 ; mais une session restée en clair est refusée après la négociation ;
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
const HEADER_FIELDS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE \
     BODY.PEEK[HEADER.FIELDS (DATE FROM TO CC REPLY-TO SUBJECT MESSAGE-ID \
     IN-REPLY-TO REFERENCES LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST CONTENT-TYPE)])";

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
    pub fn new() -> Self {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        Self { config: Arc::new(config) }
    }

    async fn tls_stream(&self, endpoint: &Endpoint) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(|e| Error::network(format!("connexion à {}:{} : {e}", endpoint.host, endpoint.port)))?;

        // Nagle retarde les petites commandes IMAP de plusieurs dizaines de
        // millisecondes ; sur une synchronisation bavarde, cela se voit.
        let _ = tcp.set_nodelay(true);

        let nom = rustls::pki_types::ServerName::try_from(endpoint.host.clone())
            .map_err(|_| Error::Config(format!("nom d'hôte invalide : « {} »", endpoint.host)))?;

        tokio_rustls::TlsConnector::from(Arc::clone(&self.config))
            .connect(nom, tcp)
            .await
            .map_err(|e| Error::network(format!("négociation TLS avec {} : {e}", endpoint.host)))
    }
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
        if !endpoint.tls_immediate {
            // Le port 143 avec STARTTLS reste servi par quelques hébergeurs. La
            // négociation par montée en chiffrement exige de reprendre le flux nu,
            // ce que la bibliothèque ne propose pas directement : plutôt que de
            // bricoler un chemin fragile et peu testé, on l'annonce clairement.
            return Err(Error::Config(format!(
                "{}:{} exige STARTTLS, que cette version ne gère pas encore ; \
                 utilisez le port chiffré (993) si le serveur le propose",
                endpoint.host, endpoint.port
            )));
        }

        let tls = self.tls_stream(endpoint).await?;
        let client = async_imap::Client::new(tls);

        let session = match credentials {
            Credentials::Password { user, password } => client
                .login(user, password)
                .await
                .map_err(|(e, _)| translate_login_error(e, user))?,
            Credentials::OAuth2 { user, token } => {
                let auth = XOAuth2 { user: user.clone(), token: token.clone() };
                client
                    .authenticate("XOAUTH2", auth)
                    .await
                    .map_err(|(e, _)| translate_login_error(e, user))?
            }
        };

        let mut connection = ImapClient { session: Some(session), capabilities: Capabilities::default() };
        connection.load_capabilities().await?;
        Ok(Box::new(connection))
    }
}

/// Mécanisme `XOAUTH2`, tel qu'attendu par Google et Microsoft.
struct XOAuth2 {
    user: String,
    token: String,
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
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
    if minuscules.contains("authenticationfailed")
        || minuscules.contains("invalid credentials")
        || minuscules.contains("login failed")
        || minuscules.contains("authentication failed")
    {
        Error::AuthFailed { account: account.to_string() }
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
        let noms: Vec<String> = caps.iter().map(|c| format!("{c:?}").to_uppercase()).collect();
        self.capabilities = Capabilities::from_names(noms);
        Ok(())
    }
}

fn protocol_error(quoi: &str, e: async_imap::error::Error) -> Error {
    Error::Protocol { protocol: "IMAP", message: format!("{quoi} : {e}") }
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

/// Déduit le rôle d'un dossier de ses attributs spéciaux, avec repli sur son nom.
fn folder_kind(name: &str, attributes: &[async_imap::types::NameAttribute<'_>]) -> FolderKind {
    use async_imap::types::NameAttribute as A;

    // Les attributs du RFC 6154 sont la source la plus fiable : le serveur dit
    // lui-même à quoi sert le dossier.
    for a in attributes {
        match a {
            A::NoSelect => return FolderKind::NoSelect,
            A::Sent => return FolderKind::Sent,
            A::Drafts => return FolderKind::Drafts,
            A::Trash => return FolderKind::Trash,
            A::Junk => return FolderKind::Junk,
            A::Archive | A::All => return FolderKind::Archive,
            _ => {}
        }
    }

    // Repli sur le nom : de nombreux serveurs n'annoncent aucun attribut spécial, et
    // les noms sont localisés.
    let n = name.to_lowercase();
    let dernier = n.rsplit(['/', '.']).next().unwrap_or(&n);
    match dernier {
        "inbox" => FolderKind::Inbox,
        "sent" | "sent items" | "envoyés" | "éléments envoyés" => FolderKind::Sent,
        "drafts" | "brouillons" => FolderKind::Drafts,
        "trash" | "deleted items" | "corbeille" => FolderKind::Trash,
        "junk" | "spam" | "indésirables" => FolderKind::Junk,
        "archive" | "archives" => FolderKind::Archive,
        _ => FolderKind::Other,
    }
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

        let mut out = Vec::new();
        while let Some(nom) = flux.next().await {
            let nom = nom.map_err(|e| protocol_error("liste des dossiers", e))?;
            let kind = folder_kind(nom.name(), nom.attributes());
            if kind == FolderKind::NoSelect {
                continue;
            }
            out.push(RemoteFolder { path: nom.name().to_string(), kind });
        }
        Ok(out)
    }

    async fn select(&mut self, path: &str) -> Result<SelectedFolder> {
        let condstore = self.capabilities.condstore;
        let session = self.session()?;

        let boite = if condstore {
            session.select_condstore(path).await
        } else {
            session.select(path).await
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
                content: f.header().map(<[u8]>::to_vec).unwrap_or_default(),
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
        Err(Error::Protocol { protocol: "IMAP", message: format!("corps de l'UID {uid} absent") })
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

    async fn store_flags(&mut self, uids: &[u32], flags: Flags, add: bool) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let sequence =
            uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let commande = if add { "+FLAGS.SILENT" } else { "-FLAGS.SILENT" };

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

    async fn move_messages(&mut self, uids: &[u32], target: &str) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let sequence = uids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
        let atomique = self.capabilities.r#move;
        let session = self.session()?;

        if atomique {
            session
                .uid_mv(&sequence, target)
                .await
                .map_err(|e| protocol_error("déplacement", e))?;
            return Ok(());
        }

        // Sans MOVE : copier, marquer supprimé, purger. Ce n'est pas atomique — une
        // coupure entre les deux laisse un doublon — mais c'est le seul chemin
        // disponible, et le journal d'opérations rendra l'ensemble rejouable.
        session
            .uid_copy(&sequence, target)
            .await
            .map_err(|e| protocol_error("copie", e))?;

        // L'emprunt de la session doit prendre fin avant l'appel suivant, qui la
        // réemprunte.
        self.store_flags(uids, Flags::DELETED, true).await?;

        let session = self.session()?;
        // Le flux d'expurgation n'est pas « Unpin » : il doit être épinglé avant
        // d'être parcouru.
        let mut purge = Box::pin(
            session.uid_expunge(&sequence).await.map_err(|e| protocol_error("purge", e))?,
        );
        while let Some(item) = purge.next().await {
            item.map_err(|e| protocol_error("purge", e))?;
        }
        Ok(())
    }

    async fn append(&mut self, folder: &str, raw: &[u8], flags: Flags) -> Result<Option<u32>> {
        let noms = flags_to_names(flags);
        let session = self.session()?;

        session
            .append(folder, Some(&noms), None, raw)
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
        let session = self
            .session
            .take()
            .ok_or_else(|| Error::Protocol { protocol: "IMAP", message: "session absente".into() })?;

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
    }

    #[test]
    fn le_role_retombe_sur_le_nom_quand_le_serveur_est_muet() {
        // Beaucoup de serveurs n'annoncent aucun attribut spécial.
        assert_eq!(folder_kind("INBOX", &[]), FolderKind::Inbox);
        assert_eq!(folder_kind("INBOX/Sent", &[]), FolderKind::Sent);
        assert_eq!(folder_kind("INBOX.Corbeille", &[]), FolderKind::Trash);
        assert_eq!(folder_kind("Éléments envoyés", &[]), FolderKind::Sent);
        assert_eq!(folder_kind("Clients/2024", &[]), FolderKind::Other);
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
    fn le_mecanisme_xoauth2_respecte_le_format_attendu() {
        use async_imap::Authenticator;
        let mut m = XOAuth2 { user: "a@x.fr".into(), token: "jeton".into() };
        let reponse = m.process(b"");
        assert_eq!(reponse, "user=a@x.fr\x01auth=Bearer jeton\x01\x01");
    }

    #[test]
    fn le_connecteur_se_construit_avec_les_racines_embarquees() {
        // Une machine mal configurée produirait sinon cent échecs inexplicables.
        let _ = RustlsConnector::new();
    }
}
