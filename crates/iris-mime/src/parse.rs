//! Analyse d'un message brut.

use crate::{sanitize, unsubscribe};
use iris_types::{Address, AttachmentMeta, Flags, RfcMessageId, Timestamp, Unsubscribe};
use mail_parser::{MessageParser, MimeHeaders};

/// Longueur de l'aperçu affiché dans la liste.
///
/// Calculé une seule fois, à la synchronisation : le défilement ne doit jamais
/// déclencher d'analyse.
const PREVIEW_LEN: usize = 200;

/// Un message analysé, prêt à être stocké.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub rfc_message_id: Option<RfcMessageId>,
    pub in_reply_to: Option<RfcMessageId>,
    pub references: Vec<RfcMessageId>,
    pub subject: String,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub date: Timestamp,
    /// Corps texte, tel quel.
    pub text_body: Option<String>,
    /// Corps HTML, déjà assaini.
    pub html_body: Option<sanitize::Sanitized>,
    pub attachments: Vec<AttachmentMeta>,
    pub unsubscribe: Option<Unsubscribe>,
    /// Aperçu court, dérivé du corps texte ou du HTML.
    pub preview: String,
    /// Drapeaux déduits du contenu : pièce jointe, traqueur, désabonnement.
    pub derived_flags: Flags,
}

impl Parsed {
    /// Le texte à indexer : le corps texte, ou le HTML dépouillé de ses balises.
    pub fn indexable_text(&self) -> String {
        if let Some(t) = &self.text_body {
            return t.clone();
        }
        self.html_body
            .as_ref()
            .map(|h| strip_tags(&h.html))
            .unwrap_or_default()
    }
}

/// Analyse un message brut au format RFC 5322.
pub fn parse(raw: &[u8]) -> iris_types::Result<Parsed> {
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| iris_types::Error::parse("message illisible"))?;

    let subject = message.subject().unwrap_or_default().to_string();

    let from = addresses(message.from());
    let to = addresses(message.to());
    let cc = addresses(message.cc());
    let reply_to = addresses(message.reply_to());

    let rfc_message_id = message.message_id().and_then(RfcMessageId::parse);
    let in_reply_to = header_ids(&message, "In-Reply-To").into_iter().next();
    let references = header_ids(&message, "References");

    let date = message
        .date()
        .map(|d| Timestamp::from_millis(d.to_timestamp() * 1000))
        .unwrap_or(Timestamp::EPOCH);

    // On ne retient un corps texte que s'il provient d'une vraie partie `text/plain`.
    // L'analyseur sait convertir le HTML en texte, mais il colle les mots des blocs
    // voisins (« Titre » + « Contenu » donne « TitreContenu »), ce qui rendrait ces
    // mots introuvables à la recherche. Pour le HTML, notre propre extraction est
    // plus fiable.
    let text_body = message.text_bodies().find_map(|part| {
        let plain = part
            .content_type()
            .map(|c| {
                c.ctype().eq_ignore_ascii_case("text")
                    && c.subtype().is_none_or(|s| s.eq_ignore_ascii_case("plain"))
            })
            // Sans en-tête de type, le RFC 2045 impose text/plain par défaut.
            .unwrap_or(true);
        plain
            .then(|| part.text_contents().map(str::to_string))
            .flatten()
    });
    let html_body = message.body_html(0).map(|c| sanitize::sanitize(&c));

    let attachments: Vec<AttachmentMeta> = message
        .attachments()
        .map(|part| {
            let filename = part
                .attachment_name()
                .map(str::to_string)
                .unwrap_or_else(|| "sans-nom".to_string());
            AttachmentMeta {
                mime_type: part
                    .content_type()
                    .map(|c| match c.subtype() {
                        Some(sub) => format!("{}/{}", c.ctype(), sub),
                        None => c.ctype().to_string(),
                    })
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
                size: part.contents().len() as u64,
                // Une partie référencée par `cid:` depuis le corps n'est pas une
                // pièce jointe pour l'utilisateur, même si elle en est une pour MIME.
                inline: part.content_id().is_some()
                    || part
                        .content_disposition()
                        .is_some_and(|d| d.ctype().eq_ignore_ascii_case("inline")),
                filename,
                blob: None,
            }
        })
        .collect();

    let unsubscribe = unsubscribe::parse(
        header_text(&message, "List-Unsubscribe").as_deref(),
        header_text(&message, "List-Unsubscribe-Post").as_deref(),
    );

    let preview = build_preview(text_body.as_deref(), html_body.as_ref());

    let mut derived_flags = Flags::NONE;
    if attachments.iter().any(|a| !a.inline) {
        derived_flags = derived_flags.with(Flags::HAS_ATTACHMENT);
    }
    if html_body.as_ref().is_some_and(|h| h.is_tracked()) {
        derived_flags = derived_flags.with(Flags::HAS_TRACKER);
    }
    if unsubscribe.is_some() {
        derived_flags = derived_flags.with(Flags::UNSUBSCRIBABLE);
    }

    Ok(Parsed {
        rfc_message_id,
        in_reply_to,
        references,
        subject,
        from,
        to,
        cc,
        reply_to,
        date,
        text_body,
        html_body,
        attachments,
        unsubscribe,
        preview,
        derived_flags,
    })
}

fn addresses(addr: Option<&mail_parser::Address<'_>>) -> Vec<Address> {
    let Some(addr) = addr else { return Vec::new() };
    addr.iter()
        .filter_map(|a| {
            let email = a.address()?;
            Some(Address {
                name: a
                    .name()
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty()),
                addr: email.trim().to_string(),
            })
        })
        .collect()
}

/// Lit un en-tête sous sa forme brute.
///
/// Indispensable pour `List-Unsubscribe` : l'analyseur y reconnaît une liste
/// d'adresses et en extrait « https://… » comme s'il s'agissait d'une adresse
/// électronique, ce qui perd la distinction entre lien web et mailto.
fn header_text(message: &mail_parser::Message<'_>, name: &str) -> Option<String> {
    message
        .header_raw(name)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Extrait les identifiants d'un en-tête qui en contient une liste.
fn header_ids(message: &mail_parser::Message<'_>, name: &str) -> Vec<RfcMessageId> {
    let Some(header) = message.header(name) else {
        return Vec::new();
    };

    match header {
        mail_parser::HeaderValue::Text(t) => split_ids(t),
        mail_parser::HeaderValue::TextList(list) => {
            list.iter().flat_map(|t| split_ids(t)).collect()
        }
        _ => Vec::new(),
    }
}

fn split_ids(raw: &str) -> Vec<RfcMessageId> {
    // Les identifiants peuvent être séparés par des espaces, des virgules, ou collés
    // entre chevrons. On accepte les trois formes rencontrées dans la nature.
    if raw.contains('<') {
        raw.split('<')
            .filter_map(|part| part.split('>').next())
            .filter_map(RfcMessageId::parse)
            .collect()
    } else {
        raw.split([',', ' ', '\t', '\r', '\n'])
            .filter_map(RfcMessageId::parse)
            .collect()
    }
}

/// Construit l'aperçu affiché dans la liste.
fn build_preview(text: Option<&str>, html: Option<&sanitize::Sanitized>) -> String {
    let brut = match text {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => html.map(|h| strip_tags(&h.html)).unwrap_or_default(),
    };

    let nettoye: String = brut
        .lines()
        // Les lignes citées polluent l'aperçu : dans un fil, l'aperçu montrerait la
        // réponse précédente au lieu du nouveau contenu.
        .filter(|l| !l.trim_start().starts_with('>'))
        .collect::<Vec<_>>()
        .join(" ");

    let mots: String = nettoye.split_whitespace().collect::<Vec<_>>().join(" ");
    if mots.chars().count() <= PREVIEW_LEN {
        mots
    } else {
        let coupe: String = mots.chars().take(PREVIEW_LEN).collect();
        format!("{}…", coupe.trim_end())
    }
}

/// Retire les balises d'un fragment HTML pour n'en garder que le texte.
pub fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut dans_balise = false;
    for c in html.chars() {
        match c {
            '<' => dans_balise = true,
            '>' => {
                dans_balise = false;
                out.push(' ');
            }
            _ if !dans_balise => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Décode les entités les plus courantes. Les autres sont laissées telles quelles :
/// un aperçu approximatif vaut mieux qu'une table de mille entrées.
fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Extrait le contenu d'une pièce jointe, par son rang.
///
/// Les octets ne sont pas dupliqués dans la base : le message brut est déjà dans le
/// magasin de contenus, et il les contient. On les en ressort au moment où
/// l'utilisateur enregistre le fichier, ce qui coûte une analyse et économise autant
/// d'octets que le message en pèse.
///
/// Le rang est celui de `Parsed::attachments`, dans le même ordre.
pub fn attachment_bytes(raw: &[u8], index: usize) -> Option<Vec<u8>> {
    let message = MessageParser::default().parse(raw)?;
    let part = message.attachments().nth(index)?;
    Some(part.contents().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &[u8] = b"From: Marie Vasseur <marie@example.com>\r\n\
To: moi@example.com\r\n\
Subject: Devis refonte\r\n\
Message-ID: <abc123@example.com>\r\n\
Date: Mon, 13 Sep 2027 10:00:00 +0200\r\n\
\r\n\
Bonjour,\r\n\
Je reviens vers vous concernant le devis.\r\n";

    #[test]
    fn les_champs_essentiels_sont_extraits() {
        let p = parse(SIMPLE).unwrap();
        assert_eq!(p.subject, "Devis refonte");
        assert_eq!(p.from[0].addr, "marie@example.com");
        assert_eq!(p.from[0].name.as_deref(), Some("Marie Vasseur"));
        assert_eq!(p.to[0].addr, "moi@example.com");
        assert_eq!(
            p.rfc_message_id.as_ref().unwrap().as_str(),
            "abc123@example.com"
        );
        assert!(p.date.millis() > 0);
    }

    #[test]
    fn l_apercu_est_construit_a_partir_du_texte() {
        let p = parse(SIMPLE).unwrap();
        assert!(p.preview.starts_with("Bonjour, Je reviens vers vous"));
        assert!(!p.preview.contains('\n'));
    }

    #[test]
    fn les_lignes_citees_sont_exclues_de_l_apercu() {
        // Sans cette règle, l'aperçu d'une réponse montrerait le message précédent.
        let raw = b"Subject: Re: Devis\r\n\r\n> Bonjour, voici le devis\r\n> Cordialement\r\nC'est parfait, merci.\r\n";
        let p = parse(raw).unwrap();
        assert_eq!(p.preview, "C'est parfait, merci.");
    }

    #[test]
    fn l_apercu_est_tronque() {
        let corps = "mot ".repeat(300);
        let raw = format!("Subject: Long\r\n\r\n{corps}");
        let p = parse(raw.as_bytes()).unwrap();
        assert!(p.preview.chars().count() <= PREVIEW_LEN + 1);
        assert!(p.preview.ends_with('…'));
    }

    #[test]
    fn la_chaine_de_references_est_extraite() {
        let raw = b"Subject: Re: Devis\r\n\
In-Reply-To: <parent@example.com>\r\n\
References: <racine@example.com> <parent@example.com>\r\n\
\r\n\
corps\r\n";
        let p = parse(raw).unwrap();
        assert_eq!(
            p.in_reply_to.as_ref().unwrap().as_str(),
            "parent@example.com"
        );
        let refs: Vec<_> = p.references.iter().map(|r| r.as_str()).collect();
        assert_eq!(refs, ["racine@example.com", "parent@example.com"]);
    }

    #[test]
    fn un_message_sans_identifiant_ne_recolle_rien() {
        let raw = b"Subject: Anonyme\r\n\r\ncorps\r\n";
        let p = parse(raw).unwrap();
        assert!(p.rfc_message_id.is_none());
        assert!(p.in_reply_to.is_none());
        assert!(p.references.is_empty());
    }

    #[test]
    fn le_html_est_assaini_a_l_analyse() {
        let raw = b"Subject: Infolettre\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<p>Bonjour</p><script>voler()</script><img src=\"https://p.example/o.gif\" width=\"1\" height=\"1\">\r\n";
        let p = parse(raw).unwrap();
        let html = p.html_body.as_ref().unwrap();
        assert!(!html.html.contains("script"));
        assert!(html.is_tracked());
        assert!(p.derived_flags.contains(iris_types::Flags::HAS_TRACKER));
    }

    #[test]
    fn le_desabonnement_est_extrait_et_signale() {
        let raw = b"Subject: Infolettre\r\n\
List-Unsubscribe: <https://exemple.fr/unsub>\r\n\
List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\
\r\n\
corps\r\n";
        let p = parse(raw).unwrap();
        assert_eq!(
            p.unsubscribe,
            Some(Unsubscribe::OneClick {
                url: "https://exemple.fr/unsub".into()
            })
        );
        assert!(p.derived_flags.contains(iris_types::Flags::UNSUBSCRIBABLE));
    }

    #[test]
    fn une_piece_jointe_est_recensee_et_signalee() {
        let raw = b"Subject: Devis\r\n\
Content-Type: multipart/mixed; boundary=\"sep\"\r\n\
\r\n\
--sep\r\n\
Content-Type: text/plain\r\n\
\r\n\
Voici le devis.\r\n\
--sep\r\n\
Content-Type: application/pdf; name=\"devis.pdf\"\r\n\
Content-Disposition: attachment; filename=\"devis.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--sep--\r\n";
        let p = parse(raw).unwrap();
        assert_eq!(p.attachments.len(), 1);
        assert_eq!(p.attachments[0].filename, "devis.pdf");
        assert!(!p.attachments[0].inline);
        assert!(p.derived_flags.contains(iris_types::Flags::HAS_ATTACHMENT));
    }

    #[test]
    fn une_image_embarquee_n_est_pas_une_piece_jointe_pour_l_utilisateur() {
        let raw = b"Subject: Signature\r\n\
Content-Type: multipart/related; boundary=\"sep\"\r\n\
\r\n\
--sep\r\n\
Content-Type: text/html\r\n\
\r\n\
<img src=\"cid:logo\">\r\n\
--sep\r\n\
Content-Type: image/png\r\n\
Content-ID: <logo>\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
iVBORw0KGgo=\r\n\
--sep--\r\n";
        let p = parse(raw).unwrap();
        assert!(
            p.attachments.iter().all(|a| a.inline) || p.attachments.is_empty(),
            "un logo de signature ne doit pas apparaître comme pièce jointe"
        );
        assert!(!p.derived_flags.contains(iris_types::Flags::HAS_ATTACHMENT));
    }

    #[test]
    fn un_sujet_encode_est_decode() {
        let raw = b"Subject: =?UTF-8?B?RMOpdmlzIHJlZm9udGU=?=\r\n\r\ncorps\r\n";
        let p = parse(raw).unwrap();
        assert_eq!(p.subject, "Dévis refonte");
    }

    #[test]
    fn le_corps_texte_synthetise_depuis_le_html_est_ignore() {
        // L'analyseur colle les mots des blocs voisins, ce qui rendrait
        // « Titre » et « Contenu » introuvables : on prefere notre propre
        // extraction, qui les separe.
        let raw = concat!(
            "Subject: Infolettre\r\n",
            "Content-Type: text/html\r\n",
            "\r\n",
            "<h1>Titre</h1><p>Contenu important</p>\r\n",
        )
        .as_bytes();
        let p = parse(raw).unwrap();
        assert!(
            p.text_body.is_none(),
            "aucune partie text/plain dans ce message"
        );
        assert!(p.indexable_text().contains("Titre"));
        assert!(p.indexable_text().contains("Contenu"));
    }

    #[test]
    fn le_texte_indexable_retombe_sur_le_html() {
        let raw = b"Subject: Infolettre\r\n\
Content-Type: text/html\r\n\
\r\n\
<h1>Titre</h1><p>Contenu&nbsp;important</p>\r\n";
        let p = parse(raw).unwrap();
        let texte = p.indexable_text();
        assert!(texte.contains("Titre"));
        assert!(texte.contains("Contenu important"));
        assert!(!texte.contains('<'));
    }

    #[test]
    fn un_message_vide_ne_fait_pas_paniquer() {
        let p = parse(b"\r\n").unwrap();
        assert_eq!(p.subject, "");
        assert!(p.from.is_empty());
        assert_eq!(p.preview, "");
    }

    #[test]
    fn retrait_des_balises_et_des_entites() {
        assert_eq!(strip_tags("<p>a&nbsp;b</p><p>c</p>"), "a b c");
        assert_eq!(
            strip_tags("<b>gras</b>et<i>italique</i>"),
            "gras et italique"
        );
        assert_eq!(strip_tags("5 &lt; 7 &amp;&amp; 8 &gt; 3"), "5 < 7 && 8 > 3");
    }
}
