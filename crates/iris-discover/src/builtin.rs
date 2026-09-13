//! Table des fournisseurs connus.
//!
//! Un instantané des configurations les plus courantes, embarqué dans le binaire.
//! Il couvre la grande majorité des comptes réels et permet de configurer une boîte
//! **sans le moindre appel réseau**, ce qui compte quand on en ajoute cent d'affilée.
//!
//! Cette table n'est pas exhaustive et n'a pas à l'être : elle est le premier maillon
//! d'une chaîne qui se poursuit par l'autoconfiguration, le DNS et le sondage.

use crate::{Auth, ServerConfig, Transport};

/// Une entrée de la table.
#[derive(Debug)]
pub struct Provider {
    /// Domaines desservis, y compris les alias historiques.
    pub domains: &'static [&'static str],
    pub label: &'static str,
    pub imap_host: &'static str,
    pub imap_port: u16,
    pub imap_transport: Transport,
    pub smtp_host: &'static str,
    pub smtp_port: u16,
    pub smtp_transport: Transport,
    pub auth: Auth,
    /// Message à afficher quand le fournisseur impose une contrainte.
    pub note: Option<&'static str>,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        domains: &["gmail.com", "googlemail.com"],
        label: "Gmail",
        imap_host: "imap.gmail.com",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.gmail.com",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::OAuthGoogle,
        note: Some("Gmail exige une connexion par compte Google ; les mots de passe simples sont refusés."),
    },
    Provider {
        domains: &["outlook.com", "hotmail.com", "hotmail.fr", "live.com", "live.fr", "msn.com"],
        label: "Outlook",
        imap_host: "outlook.office365.com",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp-mail.outlook.com",
        smtp_port: 587,
        smtp_transport: Transport::StartTls,
        auth: Auth::OAuthMicrosoft,
        note: Some("Microsoft exige une connexion par compte ; les mots de passe simples sont refusés."),
    },
    Provider {
        domains: &["yahoo.com", "yahoo.fr", "ymail.com"],
        label: "Yahoo",
        imap_host: "imap.mail.yahoo.com",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.mail.yahoo.com",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: Some("Yahoo demande un mot de passe d'application dédié."),
    },
    Provider {
        domains: &["free.fr"],
        label: "Free",
        imap_host: "imap.free.fr",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.free.fr",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: None,
    },
    Provider {
        domains: &["orange.fr", "wanadoo.fr"],
        label: "Orange",
        imap_host: "imap.orange.fr",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.orange.fr",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: None,
    },
    Provider {
        domains: &["sfr.fr", "neuf.fr"],
        label: "SFR",
        imap_host: "imap.sfr.fr",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.sfr.fr",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: None,
    },
    Provider {
        domains: &["laposte.net"],
        label: "La Poste",
        imap_host: "imap.laposte.net",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.laposte.net",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: None,
    },
    Provider {
        domains: &["ovh.net", "ovh.com"],
        label: "OVHcloud",
        imap_host: "ssl0.ovh.net",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "ssl0.ovh.net",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: None,
    },
    Provider {
        domains: &["infomaniak.com", "ik.me", "etik.com"],
        label: "Infomaniak",
        imap_host: "mail.infomaniak.com",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "mail.infomaniak.com",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: None,
    },
    Provider {
        domains: &["fastmail.com", "fastmail.fm"],
        label: "Fastmail",
        imap_host: "imap.fastmail.com",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.fastmail.com",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: Some("Fastmail demande un mot de passe d'application dédié."),
    },
    Provider {
        domains: &["proton.me", "protonmail.com", "pm.me"],
        label: "Proton Mail",
        imap_host: "127.0.0.1",
        imap_port: 1143,
        imap_transport: Transport::StartTls,
        smtp_host: "127.0.0.1",
        smtp_port: 1025,
        smtp_transport: Transport::StartTls,
        auth: Auth::Password,
        note: Some("Proton Mail passe obligatoirement par son pont local, qui doit être installé et lancé."),
    },
    Provider {
        domains: &["zoho.com", "zoho.eu"],
        label: "Zoho Mail",
        imap_host: "imap.zoho.eu",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.zoho.eu",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: None,
    },
    Provider {
        domains: &["gmx.com", "gmx.fr", "gmx.de", "gmx.net"],
        label: "GMX",
        imap_host: "imap.gmx.net",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "mail.gmx.net",
        smtp_port: 465,
        smtp_transport: Transport::Tls,
        auth: Auth::Password,
        note: None,
    },
    Provider {
        domains: &["icloud.com", "me.com", "mac.com"],
        label: "iCloud",
        imap_host: "imap.mail.me.com",
        imap_port: 993,
        imap_transport: Transport::Tls,
        smtp_host: "smtp.mail.me.com",
        smtp_port: 587,
        smtp_transport: Transport::StartTls,
        auth: Auth::Password,
        note: Some("iCloud demande un mot de passe d'application dédié."),
    },
];

/// Cherche un fournisseur par domaine.
///
/// La recherche remonte les sous-domaines : `mail.example.gmail.com` n'existe pas,
/// mais une entreprise hébergée chez un fournisseur peut utiliser un sous-domaine.
pub fn lookup(domain: &str) -> Option<&'static Provider> {
    let d = domain.trim().trim_end_matches('.').to_lowercase();
    if d.is_empty() {
        return None;
    }

    PROVIDERS.iter().find(|p| {
        p.domains
            .iter()
            .any(|known| d == *known || d.ends_with(&format!(".{known}")))
    })
}

impl Provider {
    pub fn to_config(&self, email: &str) -> ServerConfig {
        ServerConfig {
            provider: Some(self.label.to_string()),
            email: email.trim().to_lowercase(),
            imap_host: self.imap_host.to_string(),
            imap_port: self.imap_port,
            imap_transport: self.imap_transport,
            smtp_host: self.smtp_host.to_string(),
            smtp_port: self.smtp_port,
            smtp_transport: self.smtp_transport,
            auth: self.auth,
            note: self.note.map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_fournisseurs_courants_sont_reconnus() {
        assert_eq!(lookup("gmail.com").unwrap().label, "Gmail");
        assert_eq!(lookup("free.fr").unwrap().label, "Free");
        assert_eq!(lookup("hotmail.fr").unwrap().label, "Outlook");
    }

    #[test]
    fn la_recherche_ignore_la_casse_et_le_point_final() {
        // Un domaine pleinement qualifié se termine par un point.
        assert!(lookup("GMAIL.COM.").is_some());
        assert!(lookup("  Gmail.Com  ").is_some());
    }

    #[test]
    fn un_sous_domaine_remonte_au_fournisseur() {
        assert_eq!(lookup("mail.ovh.net").unwrap().label, "OVHcloud");
    }

    #[test]
    fn un_domaine_voisin_n_est_pas_confondu() {
        // « notgmail.com » ne doit pas être pris pour Gmail.
        assert!(lookup("notgmail.com").is_none());
    }

    #[test]
    fn un_domaine_inconnu_ne_donne_rien() {
        assert!(lookup("mon-domaine-perso.fr").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn les_fournisseurs_a_contrainte_l_annoncent() {
        // L'utilisateur doit savoir pourquoi son mot de passe est refusé.
        assert!(lookup("gmail.com").unwrap().note.is_some());
        assert!(lookup("proton.me").unwrap().note.unwrap().contains("pont"));
        assert!(lookup("free.fr").unwrap().note.is_none());
    }

    #[test]
    fn la_conversion_en_configuration_normalise_l_adresse() {
        let c = lookup("gmail.com").unwrap().to_config("  Marie@Gmail.COM ");
        assert_eq!(c.email, "marie@gmail.com");
        assert_eq!(c.imap_host, "imap.gmail.com");
        assert_eq!(c.auth, Auth::OAuthGoogle);
    }

    #[test]
    fn aucun_domaine_n_est_declare_deux_fois() {
        // Un doublon rendrait la résolution dépendante de l'ordre de la table.
        let mut vus = std::collections::BTreeSet::new();
        for p in PROVIDERS {
            for d in p.domains {
                assert!(vus.insert(*d), "domaine « {d} » déclaré deux fois");
            }
        }
    }

    #[test]
    fn tous_les_ports_sont_plausibles() {
        for p in PROVIDERS {
            assert!(
                [143, 993, 1143].contains(&p.imap_port),
                "port IMAP inattendu pour {}",
                p.label
            );
            assert!(
                [25, 465, 587, 1025].contains(&p.smtp_port),
                "port SMTP inattendu pour {}",
                p.label
            );
        }
    }
}
