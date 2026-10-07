//! L'envoi proprement dit.

use crate::compose::Outgoing;
use async_trait::async_trait;
use iris_types::{Error, Result, RfcMessageId, Timestamp};
use std::sync::{Arc, Mutex};

/// Ce qu'un envoi rapporte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOutcome {
    /// Identifiant réellement attribué au message.
    pub message_id: RfcMessageId,
    /// Message brut, à déposer dans le dossier des messages envoyés.
    pub raw: Vec<u8>,
}

/// Ce qui sait envoyer.
#[async_trait]
pub trait Mailer: Send + Sync + std::fmt::Debug {
    async fn send(&self, message: &Outgoing) -> Result<SendOutcome>;
}

/// Envoi réel, au-dessus de `lettre`.
#[derive(Debug)]
pub struct LettreMailer {
    transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
    /// The same server, signed in to by no one: for a relay that offers no way to
    /// sign in (an office's own, which trusts its network). With a login always
    /// given, such a relay could never be sent through.
    anonymous: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>,
}

/// How a mailbox signs in to its outgoing server.
#[derive(Clone, PartialEq, Eq)]
pub enum SmtpLogin {
    Password(String),
    /// An OAuth access token, for Google and Microsoft (`XOAUTH2`).
    OAuth2(String),
}

impl std::fmt::Debug for SmtpLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the secret itself, not even in debug output.
        f.write_str(match self {
            Self::Password(_) => "Password(…)",
            Self::OAuth2(_) => "OAuth2(…)",
        })
    }
}

impl LettreMailer {
    /// A sender for one server: TLS from the first byte when `tls`, else `STARTTLS`.
    pub fn new(host: &str, port: u16, tls: bool, user: &str, login: &SmtpLogin) -> Result<Self> {
        use lettre::transport::smtp::authentication::{Credentials, Mechanism};

        let (secret, mecanismes) = match login {
            SmtpLogin::Password(p) => (p, vec![Mechanism::Plain, Mechanism::Login]),
            SmtpLogin::OAuth2(t) => (t, vec![Mechanism::Xoauth2]),
        };
        let builder = || {
            if tls {
                lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(host)
            } else {
                lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::starttls_relay(host)
            }
            .map_err(|e| Error::network(format!("configuration SMTP : {e}")))
        };

        let transport = builder()?
            .port(port)
            .credentials(Credentials::new(user.into(), secret.clone()))
            .authentication(mecanismes)
            .build();
        let anonymous = builder()?.port(port).build();

        Ok(Self {
            transport,
            anonymous,
        })
    }
}

/// The server offers no way of signing in that Iris speaks: `AUTH` absent (a relay
/// that trusts its network), or only CRAM-MD5.
fn no_shared_mechanism(e: &lettre::transport::smtp::Error) -> bool {
    e.to_string()
        .to_ascii_lowercase()
        .contains("no compatible authentication mechanism")
}

fn smtp_error(e: lettre::transport::smtp::Error) -> Error {
    let texte = e.to_string();
    // Un refus permanent du serveur ne doit pas être retenté indéfiniment :
    // l'utilisateur doit corriger l'adresse ou ses identifiants.
    if e.is_permanent() || no_shared_mechanism(&e) {
        Error::Protocol {
            protocol: "SMTP",
            message: texte,
        }
    } else {
        Error::network(format!("envoi : {texte}"))
    }
}

fn domain_of(address: &str) -> String {
    address
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap_or_default()
}

#[async_trait]
impl Mailer for LettreMailer {
    async fn send(&self, message: &Outgoing) -> Result<SendOutcome> {
        use lettre::AsyncTransport;

        message.validate().map_err(Error::Config)?;

        // The sender's own domain names the message: the login's gave `@iris.local`
        // for a login without one, or let out the name of an internal domain.
        let domaine = domain_of(&message.from.addr);
        let (courrier, message_id, brut) = sent_and_kept(message, &domaine)?;

        match self.transport.send(courrier.clone()).await {
            Ok(_) => {}
            // Nothing to sign in with: tried as the relay expects, without a login.
            Err(e) if no_shared_mechanism(&e) => {
                self.anonymous
                    .send(courrier)
                    .await
                    .map_err(|anonyme| Error::Protocol {
                        protocol: "SMTP",
                        message: format!(
                            "{anonyme} (the server offers no sign-in Iris can use, \
                             such as PLAIN or LOGIN, and refused the message without one)"
                        ),
                    })?;
            }
            Err(e) => return Err(smtp_error(e)),
        }

        Ok(SendOutcome {
            message_id,
            raw: brut,
        })
    }
}

/// Le message tel qu'il s'écrit dans un dossier : pour un brouillon, déposé plutôt
/// qu'envoyé. Accepte un message sans destinataire.
///
/// Bcc is kept: a draft is not sent, and reopened without it the blind copies were
/// gone.
pub fn message_bytes(message: &Outgoing) -> Result<Vec<u8>> {
    let domaine = domain_of(&message.from.addr);
    build_lettre_message_with(message, &domaine, true).map(|(m, _)| m.formatted())
}

/// The message that goes out, its id, and the bytes kept in Sent.
///
/// The kept copy has the Bcc line the one sent has not, as every client's Sent folder
/// does: who was copied blind is the sender's to know. Both carry the same
/// `Message-ID` and `Date`, or the copy would not be the message sent.
fn sent_and_kept(
    message: &Outgoing,
    domain: &str,
) -> Result<(lettre::Message, RfcMessageId, Vec<u8>)> {
    if message.bcc.is_empty() {
        let (courrier, id) = build_lettre_message(message, domain)?;
        let brut = courrier.formatted();
        return Ok((courrier, id, brut));
    }
    let mut fige = message.clone();
    if fige.date == Timestamp::EPOCH {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        fige.date = Timestamp::from_millis(ms);
    }
    let (courrier, id) = build_lettre_message(&fige, domain)?;
    fige.message_id = Some(id.clone());
    let (copie, _) = build_lettre_message_with(&fige, domain, true)?;
    Ok((courrier, id, copie.formatted()))
}

/// Traduit notre message vers celui de `lettre`, sans `Bcc` : il ne sert qu'à
/// l'enveloppe, et l'écrire le montrerait à tous les destinataires.
fn build_lettre_message(
    message: &Outgoing,
    domain: &str,
) -> Result<(lettre::Message, RfcMessageId)> {
    build_lettre_message_with(message, domain, false)
}

fn build_lettre_message_with(
    message: &Outgoing,
    domain: &str,
    keep_bcc: bool,
) -> Result<(lettre::Message, RfcMessageId)> {
    use lettre::message::{header, Mailbox, MultiPart, SinglePart};

    let vers_mailbox = |a: &iris_types::Address| -> Result<Mailbox> {
        let adresse: lettre::Address = a
            .addr
            .parse()
            .map_err(|e| Error::Config(format!("adresse « {} » : {e}", a.addr)))?;
        Ok(Mailbox::new(a.name.clone(), adresse))
    };

    let expediteur = vers_mailbox(&message.from)?;
    let mut builder = lettre::Message::builder().from(expediteur.clone());
    if keep_bcc {
        builder = builder.keep_bcc();
    }

    // A draft may have nobody to send to yet. `lettre` derives its envelope from the
    // recipients and refuses a message without one; given an envelope of its own —
    // to the sender, never used since a draft is stored, not sent — it builds it.
    if message.to.is_empty() && message.cc.is_empty() && message.bcc.is_empty() {
        let envelope = lettre::address::Envelope::new(
            Some(expediteur.email.clone()),
            vec![expediteur.email.clone()],
        )
        .map_err(|e| Error::Config(format!("enveloppe : {e}")))?;
        builder = builder.envelope(envelope);
    }

    for a in &message.to {
        builder = builder.to(vers_mailbox(a)?);
    }
    for a in &message.cc {
        builder = builder.cc(vers_mailbox(a)?);
    }
    for a in &message.bcc {
        builder = builder.bcc(vers_mailbox(a)?);
    }

    builder = builder.subject(&message.subject);
    if message.date != Timestamp::EPOCH {
        let depuis = std::time::Duration::from_millis(message.date.millis().max(0) as u64);
        builder = builder.date(std::time::UNIX_EPOCH + depuis);
    }

    let message_id = message
        .message_id
        .clone()
        .unwrap_or_else(|| crate::compose::generate_message_id(domain, message.to.len() as u64));
    builder = builder.message_id(Some(format!("<{}>", message_id.as_str())));

    if let Some(parent) = &message.in_reply_to {
        builder = builder.in_reply_to(format!("<{}>", parent.as_str()));
    }
    if !message.references.is_empty() {
        let chaine = message
            .references
            .iter()
            .map(|r| format!("<{}>", r.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        builder = builder.references(chaine);
    }

    // An answer to an invitation: the text, and beside it as an alternative the iTIP
    // part itself, `text/calendar; method=REPLY`, as Outlook and Google send and
    // read it.
    if let Some(ics) = &message.calendar_reply {
        let type_ics: header::ContentType = "text/calendar; method=REPLY; charset=UTF-8"
            .parse()
            .map_err(|e| Error::Config(format!("type de l'invitation : {e}")))?;
        let reponse = MultiPart::alternative()
            .singlepart(
                SinglePart::builder()
                    .header(header::ContentType::TEXT_PLAIN)
                    .body(message.text_body.clone()),
            )
            .singlepart(SinglePart::builder().header(type_ics).body(ics.clone()));
        let courrier = builder
            .multipart(reponse)
            .map_err(|e| Error::Config(format!("construction du message : {e}")))?;
        return Ok((courrier, message_id));
    }

    // Le corps : texte seul, ou alternative texte + HTML. Une alternative texte est
    // toujours jointe, car un message uniquement HTML est souvent classé indésirable.
    let corps = match (&message.html_body, message.attachments.is_empty()) {
        // As a part, not a bare body: `body` writes no `Content-Type` and no
        // `MIME-Version`, which reads as US-ASCII, and "é" arrived as "Ã©".
        (None, true) => {
            return builder
                .singlepart(
                    SinglePart::builder()
                        .header(header::ContentType::TEXT_PLAIN)
                        .body(message.text_body.clone()),
                )
                .map(|m| (m, message_id))
                .map_err(|e| Error::Config(format!("construction du message : {e}")));
        }
        (Some(html), _) => MultiPart::alternative()
            .singlepart(
                SinglePart::builder()
                    .header(header::ContentType::TEXT_PLAIN)
                    .body(message.text_body.clone()),
            )
            .singlepart(
                SinglePart::builder()
                    .header(header::ContentType::TEXT_HTML)
                    .body(html.clone()),
            ),
        (None, false) => MultiPart::mixed().singlepart(
            SinglePart::builder()
                .header(header::ContentType::TEXT_PLAIN)
                .body(message.text_body.clone()),
        ),
    };

    let mut assemble = if message.attachments.is_empty() {
        corps
    } else {
        let mut mixte = MultiPart::mixed().multipart(corps);
        for piece in &message.attachments {
            let type_mime: header::ContentType = piece
                .mime_type
                .parse()
                .unwrap_or(header::ContentType::TEXT_PLAIN);
            mixte = mixte.singlepart(
                lettre::message::Attachment::new(piece.filename.clone())
                    .body(piece.content.clone(), type_mime),
            );
        }
        mixte
    };

    // Emprunt inutile évité : `multipart` consomme la valeur.
    let courrier = builder
        .multipart(std::mem::replace(&mut assemble, MultiPart::mixed().build()))
        .map_err(|e| Error::Config(format!("construction du message : {e}")))?;

    Ok((courrier, message_id))
}

/// Expéditeur simulé.
#[derive(Debug, Clone, Default)]
pub struct FakeMailer {
    sent: Arc<Mutex<Vec<Outgoing>>>,
    /// Erreur à renvoyer au prochain envoi, puis effacée.
    next_error: Arc<Mutex<Option<Error>>>,
}

impl FakeMailer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sent(&self) -> Vec<Outgoing> {
        self.sent.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }

    pub fn fail_next(&self, error: Error) {
        *self.next_error.lock().unwrap() = Some(error);
    }
}

#[async_trait]
impl Mailer for FakeMailer {
    async fn send(&self, message: &Outgoing) -> Result<SendOutcome> {
        if let Some(e) = self.next_error.lock().unwrap().take() {
            return Err(e);
        }
        message.validate().map_err(Error::Config)?;

        self.sent.lock().unwrap().push(message.clone());
        let message_id = message
            .message_id
            .clone()
            .unwrap_or_else(|| RfcMessageId(format!("simule-{}@iris.test", self.count())));

        Ok(SendOutcome {
            message_id,
            raw: format!("Subject: {}\r\n\r\n{}", message.subject, message.text_body).into_bytes(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_types::Address;

    fn message() -> Outgoing {
        Outgoing::new(
            Address::named("Moi", "moi@example.com"),
            vec![Address::new("marie@example.com")],
            "Devis",
        )
        .body("Bonjour Marie")
    }

    #[test]
    fn a_draft_keeps_its_blind_copies_a_sent_message_does_not() {
        let mut m = message();
        m.bcc = vec![Address::new("secret@example.com")];
        let brouillon = String::from_utf8(message_bytes(&m).unwrap()).unwrap();
        assert!(brouillon.contains("secret@example.com"), "{brouillon}");
        let (envoi, _) = build_lettre_message(&m, "example.com").unwrap();
        let envoi = String::from_utf8(envoi.formatted()).unwrap();
        assert!(!envoi.contains("secret@example.com"), "{envoi}");
    }

    #[test]
    fn the_sent_copy_keeps_the_blind_copies_and_the_message_s_id() {
        let mut m = message();
        m.bcc = vec![Address::new("secret@example.com")];
        let (envoye, id, copie) = sent_and_kept(&m, "example.com").unwrap();
        let envoye = String::from_utf8(envoye.formatted()).unwrap();
        let copie = String::from_utf8(copie).unwrap();
        assert!(!envoye.contains("secret@example.com"), "{envoye}");
        assert!(copie.contains("secret@example.com"), "{copie}");
        assert!(copie.contains(id.as_str()) && envoye.contains(id.as_str()));
        let date = |t: &str| {
            t.lines()
                .find(|l| l.starts_with("Date:"))
                .map(str::to_string)
        };
        assert_eq!(date(&envoye), date(&copie));
    }

    #[test]
    fn a_plain_text_message_says_it_is_utf8() {
        let m = message().body("Voilà le devis, à très vite");
        let brut = String::from_utf8(message_bytes(&m).unwrap()).unwrap();
        assert!(brut.contains("MIME-Version: 1.0"), "{brut}");
        assert!(
            brut.contains("Content-Type: text/plain; charset=utf-8"),
            "{brut}"
        );
    }

    #[test]
    fn a_draft_without_recipient_is_still_a_message() {
        let brouillon =
            Outgoing::new(Address::new("moi@example.com"), vec![], "Idée").body("À reprendre");
        let brut = String::from_utf8(message_bytes(&brouillon).unwrap()).unwrap();
        assert!(brut.contains("Subject: "), "{brut}");
        assert!(brut.contains("From: moi@example.com"), "{brut}");
        assert!(!brut.contains("\r\nTo:"), "no recipient invented: {brut}");
    }

    #[tokio::test]
    async fn l_expediteur_simule_conserve_les_messages() {
        let m = FakeMailer::new();
        m.send(&message()).await.unwrap();
        assert_eq!(m.count(), 1);
        assert_eq!(m.sent()[0].subject, "Devis");
    }

    #[tokio::test]
    async fn un_message_invalide_est_refuse_avant_l_envoi() {
        let m = FakeMailer::new();
        let vide = Outgoing::new(Address::new("moi@example.com"), vec![], "Sujet");
        assert!(m.send(&vide).await.is_err());
        assert_eq!(m.count(), 0, "rien ne doit partir");
    }

    #[tokio::test]
    async fn une_erreur_programmee_ne_frappe_qu_une_fois() {
        let m = FakeMailer::new();
        m.fail_next(Error::network("serveur injoignable"));

        assert!(m.send(&message()).await.is_err());
        assert!(m.send(&message()).await.is_ok());
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn le_message_construit_porte_les_en_tetes_de_fil() {
        let mut m = message();
        m.in_reply_to = RfcMessageId::parse("parent@example.com");
        m.references = vec![
            RfcMessageId("racine@example.com".into()),
            RfcMessageId("parent@example.com".into()),
        ];

        let (courrier, _) = build_lettre_message(&m, "example.com").unwrap();
        let brut = String::from_utf8_lossy(&courrier.formatted()).to_string();

        assert!(brut.contains("In-Reply-To: <parent@example.com>"));
        assert!(brut.contains("racine@example.com"));
        assert!(brut.contains("Subject: Devis"));
    }

    #[test]
    fn un_identifiant_est_engendre_s_il_manque() {
        let (_, id) = build_lettre_message(&message(), "example.com").unwrap();
        assert!(id.as_str().ends_with("@example.com"));
    }

    #[test]
    fn un_identifiant_fourni_est_respecte() {
        let mut m = message();
        m.message_id = RfcMessageId::parse("choisi@example.com");
        let (_, id) = build_lettre_message(&m, "example.com").unwrap();
        assert_eq!(id.as_str(), "choisi@example.com");
    }

    #[test]
    fn un_corps_html_est_accompagne_de_son_alternative_texte() {
        // Un message uniquement HTML est souvent classé indésirable.
        let mut m = message();
        m.html_body = Some("<p>Bonjour Marie</p>".into());

        let (courrier, _) = build_lettre_message(&m, "example.com").unwrap();
        let brut = String::from_utf8_lossy(&courrier.formatted()).to_string();

        assert!(brut.contains("multipart/alternative"));
        assert!(brut.contains("text/plain"));
        assert!(brut.contains("text/html"));
    }

    #[test]
    fn an_invitation_answer_is_the_message_s_own_calendar_part() {
        // As Outlook and Google send and read it; an attachment named `reply.ics`
        // left the organiser's calendar unaware of the answer.
        let mut m = message();
        m.calendar_reply = Some("BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nEND:VCALENDAR\r\n".into());

        let (courrier, _) = build_lettre_message(&m, "example.com").unwrap();
        let brut = String::from_utf8_lossy(&courrier.formatted()).to_string();

        assert!(brut.contains("multipart/alternative"), "{brut}");
        assert!(brut.contains("text/calendar"), "{brut}");
        assert!(brut.to_ascii_lowercase().contains("method=reply"), "{brut}");
        assert!(!brut.contains("attachment"), "{brut}");
    }

    #[test]
    fn une_piece_jointe_est_incluse() {
        let mut m = message();
        m.attachments.push(crate::compose::Attachment {
            filename: "devis.pdf".into(),
            mime_type: "application/pdf".into(),
            content: b"%PDF-1.4".to_vec(),
        });

        let (courrier, _) = build_lettre_message(&m, "example.com").unwrap();
        let brut = String::from_utf8_lossy(&courrier.formatted()).to_string();

        assert!(brut.contains("multipart/mixed"));
        assert!(brut.contains("devis.pdf"));
    }

    #[test]
    fn une_adresse_invalide_est_signalee_avec_son_texte() {
        let mut m = message();
        m.to = vec![Address::new("pas valide du tout")];
        let e = build_lettre_message(&m, "example.com").unwrap_err();
        assert!(e.to_string().contains("pas valide du tout"));
    }

    #[test]
    fn le_domaine_se_deduit_de_l_adresse() {
        assert_eq!(domain_of("moi@example.com"), "example.com");
        assert_eq!(domain_of("sans-arobase"), "");
    }
}
