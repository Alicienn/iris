//! `iris-discover` — configuration automatique d'un compte.
//!
//! L'objectif produit est simple à énoncer et difficile à tenir : **une adresse et
//! un mot de passe doivent suffire**, y compris pour un domaine personnel. La chaîne
//! suit donc, dans l'ordre du plus fiable au plus approximatif :
//!
//! 1. la table des fournisseurs connus, embarquée, qui répond sans réseau ;
//! 2. l'autoconfiguration publiée par le domaine lui-même ;
//! 3. la base communautaire de Mozilla ;
//! 4. les enregistrements DNS `SRV`, qui sont la réponse normalisée à cette question ;
//! 5. le domaine `MX`, qui trahit souvent l'hébergeur ;
//! 6. le sondage des noms d'hôtes usuels.
//!
//! Chaque étape est court-circuitée dès qu'une réponse exploitable arrive, et la
//! source retenue est rapportée : l'utilisateur a le droit de savoir d'où vient la
//! configuration qu'on lui propose.
//!
//! Les entrées-sorties passent par trois traits, ce qui rend toute la chaîne
//! testable sans réseau.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod autoconfig;
pub mod builtin;
mod io;

pub use io::{DnsResolver, HttpFetcher, MockIo, Prober, Resolver, SrvRecord, TcpProber, Fetcher};

use async_trait::async_trait;
use iris_types::{Error, Result};
use serde::{Deserialize, Serialize};

/// Mode de chiffrement de la connexion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    /// Chiffré dès la connexion.
    Tls,
    /// Négocié après la connexion, par la commande `STARTTLS`.
    StartTls,
    /// Sans chiffrement. Jamais proposé par la découverte.
    Plain,
}

impl Transport {
    pub fn is_encrypted(self) -> bool {
        !matches!(self, Self::Plain)
    }
}

/// Méthode d'authentification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    Password,
    OAuthGoogle,
    OAuthMicrosoft,
}

/// Une configuration de compte complète.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Nom du fournisseur, quand il est connu.
    pub provider: Option<String>,
    pub email: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_transport: Transport,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_transport: Transport,
    pub auth: Auth,
    /// Contrainte à signaler à l'utilisateur, le cas échéant.
    pub note: Option<String>,
}

/// D'où provient la configuration proposée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Table embarquée : aucune requête n'a été émise.
    Builtin,
    /// Autoconfiguration servie par le domaine.
    DomainAutoconfig,
    /// Base communautaire de Mozilla.
    Ispdb,
    /// Enregistrements DNS `SRV`.
    DnsSrv,
    /// Déduit du domaine `MX`.
    MxGuess,
    /// Noms d'hôtes usuels, vérifiés par sondage.
    Probe,
}

impl Source {
    /// Peut-on présenter cette configuration comme certaine ?
    ///
    /// Les deux dernières sources sont des conjectures : l'interface doit le dire,
    /// et proposer la saisie manuelle plutôt que de laisser croire à une certitude.
    pub fn is_authoritative(self) -> bool {
        matches!(self, Self::Builtin | Self::DomainAutoconfig | Self::Ispdb | Self::DnsSrv)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Builtin => "fournisseur connu",
            Self::DomainAutoconfig => "autoconfiguration du domaine",
            Self::Ispdb => "base communautaire Mozilla",
            Self::DnsSrv => "enregistrements DNS",
            Self::MxGuess => "déduit du serveur de courrier entrant",
            Self::Probe => "noms d'hôtes usuels vérifiés",
        }
    }
}

/// Le résultat d'une découverte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub config: ServerConfig,
    pub source: Source,
    /// Étapes tentées, dans l'ordre. Précieux pour expliquer un échec.
    pub attempts: Vec<String>,
}

/// Les entrées-sorties dont la découverte a besoin.
#[async_trait]
pub trait DiscoveryIo: Send + Sync {
    async fn fetch(&self, url: &str) -> Option<String>;
    async fn srv(&self, name: &str) -> Vec<SrvRecord>;
    async fn mx(&self, domain: &str) -> Vec<String>;
    async fn probe(&self, host: &str, port: u16) -> bool;
}

/// Le moteur de découverte.
#[derive(Debug)]
pub struct Discovery<T: DiscoveryIo> {
    io: T,
}

impl<T: DiscoveryIo> Discovery<T> {
    pub fn new(io: T) -> Self {
        Self { io }
    }

    /// Découvre la configuration d'une adresse.
    pub async fn discover(&self, email: &str) -> Result<Discovered> {
        let email = email.trim().to_lowercase();
        let domain = email
            .rsplit_once('@')
            .map(|(_, d)| d.to_string())
            .filter(|d| !d.is_empty())
            .ok_or_else(|| Error::Config(format!("adresse invalide : « {email} »")))?;

        let mut attempts = Vec::new();

        // 1. Table embarquée. Aucune requête, réponse immédiate.
        attempts.push("fournisseurs connus".into());
        if let Some(p) = builtin::lookup(&domain) {
            return Ok(Discovered {
                config: p.to_config(&email),
                source: Source::Builtin,
                attempts,
            });
        }

        // 2. Autoconfiguration servie par le domaine lui-même.
        for url in [
            format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
            format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
        ] {
            attempts.push(url.clone());
            if let Some(xml) = self.io.fetch(&url).await {
                if let Ok(config) = autoconfig::parse(&xml, &email) {
                    return Ok(Discovered {
                        config,
                        source: Source::DomainAutoconfig,
                        attempts,
                    });
                }
            }
        }

        // 3. Base communautaire.
        let ispdb = format!("https://autoconfig.thunderbird.net/v1.1/{domain}");
        attempts.push(ispdb.clone());
        if let Some(xml) = self.io.fetch(&ispdb).await {
            if let Ok(config) = autoconfig::parse(&xml, &email) {
                return Ok(Discovered { config, source: Source::Ispdb, attempts });
            }
        }

        // 4. Enregistrements DNS SRV, la réponse normalisée à cette question.
        attempts.push(format!("_imaps._tcp.{domain}"));
        let imaps = self.io.srv(&format!("_imaps._tcp.{domain}")).await;
        let submissions = self.io.srv(&format!("_submissions._tcp.{domain}")).await;
        let submission = self.io.srv(&format!("_submission._tcp.{domain}")).await;

        if let Some(imap) = best(&imaps) {
            let (smtp_host, smtp_port, smtp_transport) = match (best(&submissions), best(&submission))
            {
                (Some(s), _) => (s.target.clone(), s.port, Transport::Tls),
                (None, Some(s)) => (s.target.clone(), s.port, Transport::StartTls),
                // Un SRV entrant sans SRV sortant : on complète par la convention.
                (None, None) => (format!("smtp.{domain}"), 587, Transport::StartTls),
            };
            return Ok(Discovered {
                config: ServerConfig {
                    provider: None,
                    email: email.clone(),
                    imap_host: imap.target.clone(),
                    imap_port: imap.port,
                    imap_transport: Transport::Tls,
                    smtp_host,
                    smtp_port,
                    smtp_transport,
                    auth: Auth::Password,
                    note: None,
                },
                source: Source::DnsSrv,
                attempts,
            });
        }

        // 5. Le domaine MX trahit souvent l'hébergeur.
        attempts.push(format!("MX {domain}"));
        for mx in self.io.mx(&domain).await {
            if let Some(p) = builtin::lookup(&mx) {
                let mut config = p.to_config(&email);
                config.note = Some(format!(
                    "Configuration déduite du serveur de courrier entrant ({mx}). \
                     Vérifiez-la avant de l'enregistrer."
                ));
                return Ok(Discovered { config, source: Source::MxGuess, attempts });
            }
        }

        // 6. Sondage des noms d'hôtes usuels.
        attempts.push("sondage des noms usuels".into());
        if let Some(config) = self.probe_conventional(&email, &domain).await {
            return Ok(Discovered { config, source: Source::Probe, attempts });
        }

        Err(Error::Config(format!(
            "impossible de configurer « {domain} » automatiquement ({} pistes essayées) ; \
             la saisie manuelle reste possible",
            attempts.len()
        )))
    }

    /// Sonde les conventions de nommage les plus répandues.
    async fn probe_conventional(&self, email: &str, domain: &str) -> Option<ServerConfig> {
        let candidats_imap = [
            format!("imap.{domain}"),
            format!("mail.{domain}"),
            format!("imap4.{domain}"),
            domain.to_string(),
        ];

        let (imap_host, imap_port, imap_transport) = 'trouve: {
            for hote in &candidats_imap {
                if self.io.probe(hote, 993).await {
                    break 'trouve (hote.clone(), 993, Transport::Tls);
                }
                if self.io.probe(hote, 143).await {
                    break 'trouve (hote.clone(), 143, Transport::StartTls);
                }
            }
            return None;
        };

        let candidats_smtp =
            [format!("smtp.{domain}"), format!("mail.{domain}"), domain.to_string()];

        let (smtp_host, smtp_port, smtp_transport) = 'trouve: {
            for hote in &candidats_smtp {
                if self.io.probe(hote, 465).await {
                    break 'trouve (hote.clone(), 465, Transport::Tls);
                }
                if self.io.probe(hote, 587).await {
                    break 'trouve (hote.clone(), 587, Transport::StartTls);
                }
            }
            // Un serveur entrant sans serveur sortant joignable reste utile : on
            // propose la convention la plus courante et l'utilisateur corrigera.
            (format!("smtp.{domain}"), 587, Transport::StartTls)
        };

        Some(ServerConfig {
            provider: None,
            email: email.to_string(),
            imap_host,
            imap_port,
            imap_transport,
            smtp_host,
            smtp_port,
            smtp_transport,
            auth: Auth::Password,
            note: Some(
                "Configuration devinée à partir des noms d'hôtes usuels. \
                 Vérifiez-la avant de l'enregistrer."
                    .into(),
            ),
        })
    }
}

/// Le meilleur enregistrement `SRV` : priorité la plus basse, puis poids le plus fort.
fn best(records: &[SrvRecord]) -> Option<&SrvRecord> {
    records
        .iter()
        // Un SRV de cible « . » signifie explicitement « ce service n'existe pas ici ».
        .filter(|r| r.target != "." && !r.target.is_empty())
        .min_by_key(|r| (r.priority, std::cmp::Reverse(r.weight)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUTOCONFIG: &str = r#"<clientConfig>
        <emailProvider id="x">
        <displayName>Hébergeur X</displayName>
        <incomingServer type="imap"><hostname>imap.hebergeur.fr</hostname>
          <port>993</port><socketType>SSL</socketType></incomingServer>
        <outgoingServer type="smtp"><hostname>smtp.hebergeur.fr</hostname>
          <port>587</port><socketType>STARTTLS</socketType></outgoingServer>
        </emailProvider></clientConfig>"#;

    #[tokio::test]
    async fn un_fournisseur_connu_repond_sans_reseau() {
        let io = MockIo::default();
        let d = Discovery::new(io.clone());

        let r = d.discover("marie@gmail.com").await.unwrap();
        assert_eq!(r.source, Source::Builtin);
        assert_eq!(r.config.imap_host, "imap.gmail.com");
        assert_eq!(io.fetches(), 0, "aucune requête ne doit être émise");
        assert_eq!(r.attempts.len(), 1);
    }

    #[tokio::test]
    async fn l_autoconfiguration_du_domaine_est_prioritaire() {
        let mut io = MockIo::default();
        io.add_fetch(
            "https://autoconfig.mondomaine.fr/mail/config-v1.1.xml",
            AUTOCONFIG,
        );
        let d = Discovery::new(io);

        let r = d.discover("moi@mondomaine.fr").await.unwrap();
        assert_eq!(r.source, Source::DomainAutoconfig);
        assert_eq!(r.config.imap_host, "imap.hebergeur.fr");
        assert!(r.source.is_authoritative());
    }

    #[tokio::test]
    async fn le_chemin_bien_connu_est_essaye_ensuite() {
        let mut io = MockIo::default();
        io.add_fetch(
            "https://mondomaine.fr/.well-known/autoconfig/mail/config-v1.1.xml",
            AUTOCONFIG,
        );
        let d = Discovery::new(io);

        let r = d.discover("moi@mondomaine.fr").await.unwrap();
        assert_eq!(r.source, Source::DomainAutoconfig);
    }

    #[tokio::test]
    async fn la_base_communautaire_prend_le_relais() {
        let mut io = MockIo::default();
        io.add_fetch("https://autoconfig.thunderbird.net/v1.1/mondomaine.fr", AUTOCONFIG);
        let d = Discovery::new(io);

        let r = d.discover("moi@mondomaine.fr").await.unwrap();
        assert_eq!(r.source, Source::Ispdb);
    }

    #[tokio::test]
    async fn les_enregistrements_srv_sont_exploites() {
        let mut io = MockIo::default();
        io.add_srv("_imaps._tcp.mondomaine.fr", SrvRecord::new("imap.serveur.fr", 993, 10, 5));
        io.add_srv(
            "_submissions._tcp.mondomaine.fr",
            SrvRecord::new("smtp.serveur.fr", 465, 10, 5),
        );
        let d = Discovery::new(io);

        let r = d.discover("moi@mondomaine.fr").await.unwrap();
        assert_eq!(r.source, Source::DnsSrv);
        assert_eq!(r.config.imap_host, "imap.serveur.fr");
        assert_eq!(r.config.smtp_port, 465);
        assert_eq!(r.config.smtp_transport, Transport::Tls);
    }

    #[tokio::test]
    async fn la_priorite_srv_la_plus_basse_gagne() {
        let mut io = MockIo::default();
        io.add_srv("_imaps._tcp.x.fr", SrvRecord::new("secours.x.fr", 993, 20, 5));
        io.add_srv("_imaps._tcp.x.fr", SrvRecord::new("principal.x.fr", 993, 10, 5));
        let d = Discovery::new(io);

        let r = d.discover("moi@x.fr").await.unwrap();
        assert_eq!(r.config.imap_host, "principal.x.fr");
    }

    #[tokio::test]
    async fn a_priorite_egale_le_poids_le_plus_fort_gagne() {
        let mut io = MockIo::default();
        io.add_srv("_imaps._tcp.x.fr", SrvRecord::new("faible.x.fr", 993, 10, 1));
        io.add_srv("_imaps._tcp.x.fr", SrvRecord::new("fort.x.fr", 993, 10, 90));
        let d = Discovery::new(io);

        assert_eq!(d.discover("moi@x.fr").await.unwrap().config.imap_host, "fort.x.fr");
    }

    #[tokio::test]
    async fn un_srv_de_cible_point_signifie_service_absent() {
        // Le RFC 2782 en fait une déclaration explicite d'absence.
        let mut io = MockIo::default();
        io.add_srv("_imaps._tcp.x.fr", SrvRecord::new(".", 0, 0, 0));
        io.add_probe("imap.x.fr", 993);
        let d = Discovery::new(io);

        let r = d.discover("moi@x.fr").await.unwrap();
        assert_eq!(r.source, Source::Probe, "le SRV ne doit pas être retenu");
    }

    #[tokio::test]
    async fn le_domaine_mx_trahit_l_hebergeur() {
        let mut io = MockIo::default();
        io.add_mx("entreprise.fr", "aspmx.l.google.com.gmail.com");
        let d = Discovery::new(io);

        let r = d.discover("contact@entreprise.fr").await.unwrap();
        assert_eq!(r.source, Source::MxGuess);
        assert_eq!(r.config.imap_host, "imap.gmail.com");
        assert!(r.config.note.unwrap().contains("Vérifiez"));
        assert!(!r.source.is_authoritative(), "une déduction n'est pas une certitude");
    }

    #[tokio::test]
    async fn le_sondage_trouve_les_noms_usuels() {
        let mut io = MockIo::default();
        io.add_probe("mail.mondomaine.fr", 993);
        io.add_probe("mail.mondomaine.fr", 465);
        let d = Discovery::new(io);

        let r = d.discover("moi@mondomaine.fr").await.unwrap();
        assert_eq!(r.source, Source::Probe);
        assert_eq!(r.config.imap_host, "mail.mondomaine.fr");
        assert_eq!(r.config.smtp_host, "mail.mondomaine.fr");
        assert_eq!(r.config.smtp_port, 465);
    }

    #[tokio::test]
    async fn un_serveur_entrant_sans_sortant_reste_exploitable() {
        let mut io = MockIo::default();
        io.add_probe("imap.mondomaine.fr", 993);
        let d = Discovery::new(io);

        let r = d.discover("moi@mondomaine.fr").await.unwrap();
        assert_eq!(r.config.imap_host, "imap.mondomaine.fr");
        assert_eq!(r.config.smtp_host, "smtp.mondomaine.fr");
        assert_eq!(r.config.smtp_port, 587);
    }

    #[tokio::test]
    async fn un_echec_complet_explique_ce_qui_a_ete_tente() {
        let d = Discovery::new(MockIo::default());
        let e = d.discover("moi@nulle-part.invalid").await.unwrap_err();
        let message = e.to_string();
        assert!(message.contains("nulle-part.invalid"));
        assert!(message.contains("pistes essayées"));
        assert!(message.contains("saisie manuelle"));
    }

    #[tokio::test]
    async fn une_adresse_sans_arobase_est_refusee() {
        let d = Discovery::new(MockIo::default());
        assert!(d.discover("pas-une-adresse").await.is_err());
        assert!(d.discover("vide@").await.is_err());
    }

    #[tokio::test]
    async fn une_autoconfiguration_illisible_ne_bloque_pas_la_suite() {
        let mut io = MockIo::default();
        io.add_fetch("https://autoconfig.x.fr/mail/config-v1.1.xml", "<pas du xml valide");
        io.add_probe("imap.x.fr", 993);
        let d = Discovery::new(io);

        let r = d.discover("moi@x.fr").await.unwrap();
        assert_eq!(r.source, Source::Probe);
    }

    #[test]
    fn les_sources_devinees_ne_sont_pas_presentees_comme_certaines() {
        assert!(Source::Builtin.is_authoritative());
        assert!(Source::DnsSrv.is_authoritative());
        assert!(!Source::MxGuess.is_authoritative());
        assert!(!Source::Probe.is_authoritative());
    }

    #[test]
    fn le_transport_en_clair_n_est_pas_chiffre() {
        assert!(Transport::Tls.is_encrypted());
        assert!(Transport::StartTls.is_encrypted());
        assert!(!Transport::Plain.is_encrypted());
    }
}
