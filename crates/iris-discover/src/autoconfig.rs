//! Analyse des documents d'autoconfiguration Thunderbird.
//!
//! Format publié par Mozilla et servi par la plupart des hébergeurs sérieux, soit
//! sur leur propre domaine, soit par la base communautaire. C'est le maillon le plus
//! précieux de la chaîne : quand il répond, la configuration est exacte, y compris
//! pour un domaine personnel dont nous n'avons jamais entendu parler.

use crate::{Auth, ServerConfig, Transport};
use iris_types::{Error, Result};

/// Un serveur déclaré dans le document.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Server {
    kind: String,
    hostname: String,
    port: u16,
    socket_type: String,
    auth: Vec<String>,
    /// `<username>`, variables unreplaced.
    username: Option<String>,
}

impl Server {
    fn transport(&self) -> Transport {
        match self.socket_type.to_uppercase().as_str() {
            "SSL" => Transport::Tls,
            "STARTTLS" => Transport::StartTls,
            // « plain » signifie sans chiffrement. On refuse plus loin.
            _ => Transport::Plain,
        }
    }

    fn auth_kind(&self) -> Auth {
        if self.auth.iter().any(|a| a.eq_ignore_ascii_case("OAuth2")) {
            // Le document ne dit pas quel fournisseur d'identité : la table des
            // fournisseurs connus tranche, sinon on retombe sur le mot de passe.
            Auth::Password
        } else {
            Auth::Password
        }
    }
}

/// Analyse un document `clientConfig`.
///
/// L'analyse est délibérément tolérante : ces fichiers sont écrits à la main par des
/// hébergeurs, et un attribut manquant ne doit pas faire échouer une configuration
/// par ailleurs exploitable.
pub fn parse(xml: &str, email: &str) -> Result<ServerConfig> {
    let serveurs = extract_servers(xml)?;

    // Of the servers of each kind, an encrypted one, TLS from the start first: only
    // the first was read, so a document listing STARTTLS on 143 before SSL on 993
    // gave the one that fails, and one listing a plain server first was refused
    // whole. A server announced in the clear is never taken.
    let choisir = |genre: &str| {
        let de_ce_genre = || {
            serveurs
                .iter()
                .filter(move |s| s.kind.eq_ignore_ascii_case(genre))
        };
        de_ce_genre()
            .find(|s| s.transport() == Transport::Tls)
            .or_else(|| de_ce_genre().find(|s| s.transport() == Transport::StartTls))
    };

    let a_genre = |genre: &str| serveurs.iter().any(|s| s.kind.eq_ignore_ascii_case(genre));
    if !a_genre("imap") {
        return Err(Error::Config(
            "aucun serveur IMAP dans l'autoconfiguration".into(),
        ));
    }
    if !a_genre("smtp") {
        return Err(Error::Config(
            "aucun serveur SMTP dans l'autoconfiguration".into(),
        ));
    }
    // Un serveur annoncé en clair est refusé : accepter un mot de passe en clair
    // parce qu'un fichier XML le demande serait absurde.
    let (Some(imap), Some(smtp)) = (choisir("imap"), choisir("smtp")) else {
        return Err(Error::Config(
            "l'autoconfiguration propose une connexion non chiffrée, refusée".into(),
        ));
    };

    let local = email.split('@').next().unwrap_or("").to_string();
    let adresse = email.trim().to_lowercase();
    // The login, when the document says it is not the address (`%EMAILLOCALPART%`).
    let login = |s: &Server| {
        s.username
            .as_deref()
            .map(|u| substitute(u, email, &local))
            .filter(|u| !u.is_empty())
    };
    let imap_user = login(imap).filter(|u| !u.eq_ignore_ascii_case(&adresse));
    // The sending login is kept unless it is the reading one: empty means "as for
    // reading", and one equal to the address beside a reading login of its own was
    // dropped, so that sending signed in with the reading login.
    let lecture = imap_user.clone().unwrap_or_else(|| adresse.clone());
    let smtp_user = login(smtp).filter(|u| !u.eq_ignore_ascii_case(&lecture));

    Ok(ServerConfig {
        provider: extract_display_name(xml),
        email: adresse.clone(),
        imap_host: substitute(&imap.hostname, email, &local),
        imap_port: imap.port,
        imap_transport: imap.transport(),
        smtp_host: substitute(&smtp.hostname, email, &local),
        smtp_port: smtp.port,
        smtp_transport: smtp.transport(),
        auth: imap.auth_kind(),
        note: None,
        imap_user,
        smtp_user,
    })
}

/// Remplace les variables du document. Rares mais présentes chez certains hébergeurs.
fn substitute(value: &str, email: &str, local: &str) -> String {
    let domaine = email.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
    value
        .replace("%EMAILADDRESS%", email)
        .replace("%EMAILLOCALPART%", local)
        // `imap.%EMAILDOMAIN%` was taken as a host of that name, which none is.
        .replace("%EMAILDOMAIN%", domaine)
        .trim()
        .to_string()
}

fn extract_display_name(xml: &str) -> Option<String> {
    inner_text(xml, "displayName")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Extrait les blocs `incomingServer` et `outgoingServer`.
fn extract_servers(xml: &str) -> Result<Vec<Server>> {
    let mut out = Vec::new();

    for balise in ["incomingServer", "outgoingServer"] {
        let mut depuis = 0;
        while let Some((bloc, suite)) = next_block(xml, balise, depuis) {
            depuis = suite;

            let kind = attribute(&bloc, "type").unwrap_or_else(|| {
                // Un `outgoingServer` sans type est un SMTP : le format ne prévoit
                // rien d'autre.
                if balise == "outgoingServer" {
                    "smtp".into()
                } else {
                    String::new()
                }
            });

            let Some(hostname) = inner_text(&bloc, "hostname") else {
                continue;
            };
            let Some(port) = inner_text(&bloc, "port").and_then(|p| p.trim().parse().ok()) else {
                continue;
            };

            out.push(Server {
                kind,
                hostname: hostname.trim().to_string(),
                port,
                socket_type: inner_text(&bloc, "socketType").unwrap_or_default(),
                auth: all_inner_texts(&bloc, "authentication"),
                username: inner_text(&bloc, "username").map(|u| u.trim().to_string()),
            });
        }
    }

    if out.is_empty() {
        return Err(Error::Config(
            "document d'autoconfiguration vide ou illisible".into(),
        ));
    }
    Ok(out)
}

/// Retourne le prochain bloc `<balise ...>…</balise>` à partir d'une position.
fn next_block(xml: &str, tag: &str, from: usize) -> Option<(String, usize)> {
    let ouvrant = format!("<{tag}");
    let fermant = format!("</{tag}>");
    let debut = xml.get(from..)?.find(&ouvrant)? + from;
    let fin = xml.get(debut..)?.find(&fermant)? + debut + fermant.len();
    Some((xml.get(debut..fin)?.to_string(), fin))
}

/// Contenu du premier élément portant ce nom.
fn inner_text(xml: &str, tag: &str) -> Option<String> {
    let ouvrant = format!("<{tag}>");
    let fermant = format!("</{tag}>");
    let debut = xml.find(&ouvrant)? + ouvrant.len();
    let fin = xml.get(debut..)?.find(&fermant)? + debut;
    Some(decode_entities(xml.get(debut..fin)?))
}

fn all_inner_texts(xml: &str, tag: &str) -> Vec<String> {
    let ouvrant = format!("<{tag}>");
    let fermant = format!("</{tag}>");
    let mut out = Vec::new();
    let mut depuis = 0;
    while let Some(rel) = xml[depuis..].find(&ouvrant) {
        let debut = depuis + rel + ouvrant.len();
        let Some(rel_fin) = xml[debut..].find(&fermant) else {
            break;
        };
        out.push(decode_entities(&xml[debut..debut + rel_fin]));
        depuis = debut + rel_fin;
    }
    out
}

/// Valeur d'un attribut dans une balise ouvrante.
fn attribute(bloc: &str, name: &str) -> Option<String> {
    let motif = format!("{name}=\"");
    let debut = bloc.find(&motif)? + motif.len();
    let fin = bloc[debut..].find('"')? + debut;
    Some(bloc[debut..fin].to_string())
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXEMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="example.com">
    <domain>example.com</domain>
    <displayName>Exemple Hébergement</displayName>
    <incomingServer type="imap">
      <hostname>imap.example.com</hostname>
      <port>993</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.com</hostname>
      <port>587</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn un_document_complet_est_analyse() {
        let c = parse(EXEMPLE, "marie@example.com").unwrap();
        assert_eq!(c.imap_host, "imap.example.com");
        assert_eq!(c.imap_port, 993);
        assert_eq!(c.imap_transport, Transport::Tls);
        assert_eq!(c.smtp_host, "smtp.example.com");
        assert_eq!(c.smtp_port, 587);
        assert_eq!(c.smtp_transport, Transport::StartTls);
        assert_eq!(c.provider.as_deref(), Some("Exemple Hébergement"));
    }

    #[test]
    fn les_variables_sont_substituees() {
        let xml = EXEMPLE.replace("imap.example.com", "%EMAILLOCALPART%.imap.example.com");
        let c = parse(&xml, "marie@example.com").unwrap();
        assert_eq!(c.imap_host, "marie.imap.example.com");
    }

    #[test]
    fn une_connexion_en_clair_est_refusee() {
        // Accepter un mot de passe en clair parce qu'un fichier XML le demande
        // serait absurde.
        let xml = EXEMPLE.replace(
            "<socketType>SSL</socketType>",
            "<socketType>plain</socketType>",
        );
        let e = parse(&xml, "marie@example.com").unwrap_err();
        assert!(e.to_string().contains("non chiffrée"));
    }

    #[test]
    fn un_document_sans_imap_est_refuse() {
        let xml = r#"<clientConfig><outgoingServer type="smtp">
            <hostname>smtp.x.fr</hostname><port>465</port><socketType>SSL</socketType>
            </outgoingServer></clientConfig>"#;
        let e = parse(xml, "a@x.fr").unwrap_err();
        assert!(e.to_string().contains("IMAP"));
    }

    #[test]
    fn un_document_sans_smtp_est_refuse() {
        let xml = r#"<clientConfig><incomingServer type="imap">
            <hostname>imap.x.fr</hostname><port>993</port><socketType>SSL</socketType>
            </incomingServer></clientConfig>"#;
        let e = parse(xml, "a@x.fr").unwrap_err();
        assert!(e.to_string().contains("SMTP"));
    }

    #[test]
    fn un_document_vide_est_refuse() {
        let e = parse("<clientConfig></clientConfig>", "a@x.fr").unwrap_err();
        assert!(e.to_string().contains("vide ou illisible"));
    }

    #[test]
    fn un_serveur_pop_est_ignore_au_profit_de_l_imap() {
        // Beaucoup d'hébergeurs annoncent les deux ; nous ne parlons pas POP.
        let xml = r#"<clientConfig>
            <incomingServer type="pop3"><hostname>pop.x.fr</hostname><port>995</port>
              <socketType>SSL</socketType></incomingServer>
            <incomingServer type="imap"><hostname>imap.x.fr</hostname><port>993</port>
              <socketType>SSL</socketType></incomingServer>
            <outgoingServer type="smtp"><hostname>smtp.x.fr</hostname><port>465</port>
              <socketType>SSL</socketType></outgoingServer>
        </clientConfig>"#;
        let c = parse(xml, "a@x.fr").unwrap();
        assert_eq!(c.imap_host, "imap.x.fr");
    }

    #[test]
    fn un_port_illisible_ecarte_le_serveur() {
        let xml = EXEMPLE.replace("<port>993</port>", "<port>abc</port>");
        assert!(parse(&xml, "a@x.fr").is_err());
    }

    #[test]
    fn les_entites_xml_sont_decodees() {
        let xml = EXEMPLE.replace("Exemple Hébergement", "Marie &amp; Cie");
        let c = parse(&xml, "a@x.fr").unwrap();
        assert_eq!(c.provider.as_deref(), Some("Marie & Cie"));
    }

    #[test]
    fn the_encrypted_server_is_chosen_whatever_comes_first() {
        // Only the first was read: STARTTLS on 143 listed before SSL on 993 gave the
        // one that fails; a plain one listed first refused the whole document.
        let xml = r#"<clientConfig>
            <incomingServer type="imap"><hostname>imap.x.fr</hostname><port>143</port>
              <socketType>plain</socketType></incomingServer>
            <incomingServer type="imap"><hostname>imap.x.fr</hostname><port>143</port>
              <socketType>STARTTLS</socketType></incomingServer>
            <incomingServer type="imap"><hostname>imap.x.fr</hostname><port>993</port>
              <socketType>SSL</socketType></incomingServer>
            <outgoingServer type="smtp"><hostname>smtp.x.fr</hostname><port>587</port>
              <socketType>STARTTLS</socketType></outgoingServer>
        </clientConfig>"#;
        let c = parse(xml, "a@x.fr").unwrap();
        assert_eq!((c.imap_port, c.imap_transport), (993, Transport::Tls));
        assert_eq!(c.smtp_transport, Transport::StartTls);
    }

    #[test]
    fn a_login_that_is_not_the_address_is_kept() {
        let xml = EXEMPLE.replacen(
            "<username>%EMAILADDRESS%</username>",
            "<username>%EMAILLOCALPART%</username>",
            1,
        );
        let c = parse(&xml, "marie@example.com").unwrap();
        assert_eq!(c.imap_user.as_deref(), Some("marie"));
        // The address, beside a reading login of its own: kept, or sending would sign
        // in as "marie".
        assert_eq!(c.smtp_user.as_deref(), Some("marie@example.com"));
    }

    #[test]
    fn the_domain_is_put_in_for_its_variable() {
        let xml = EXEMPLE.replacen("imap.example.com", "imap.%EMAILDOMAIN%", 1);
        let c = parse(&xml, "marie@example.com").unwrap();
        assert_eq!(c.imap_host, "imap.example.com");
    }

    #[test]
    fn l_adresse_est_normalisee() {
        let c = parse(EXEMPLE, "  Marie@Example.COM ").unwrap();
        assert_eq!(c.email, "marie@example.com");
    }
}
