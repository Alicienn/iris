//! Ajout de comptes.
//!
//! L'objectif produit : **une adresse et un mot de passe suffisent**. La découverte
//! s'occupe du reste, et quand elle échoue, elle dit ce qu'elle a essayé au lieu de
//! renvoyer un formulaire vide.

use iris_discover::{Auth, Discovery, RealIo, ServerConfig, Source};
use iris_imap::{client::RustlsConnector, Connector, Credentials, Endpoint};
use iris_secrets::{Secret, SecretKind, SecretStore};
use iris_store::{AuthKind, NewAccount, Store};
use iris_types::{AccountId, Error, Result, Timestamp};

/// Ce qu'un ajout a produit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedAccount {
    pub id: AccountId,
    pub email: String,
    pub config: ServerConfig,
    pub source: Source,
    /// La configuration est une conjecture : l'interface doit le dire.
    pub needs_review: bool,
}

/// Ouvre une connexion et se présente, sans rien écrire.
///
/// Ce que l'écran d'ajout annonce ensuite dépend entièrement de cette réponse. Un
/// compte créé sur la seule foi d'une découverte réussie affiche « ajouté » pour une
/// adresse dont le mot de passe est faux : l'erreur n'apparaît qu'au premier cycle de
/// synchronisation, ailleurs, plus tard, et sous une forme que personne ne relie à ce
/// qui vient d'être tapé.
///
/// Seul l'IMAP est interrogé. C'est lui qui commande la lecture du courrier, donc
/// l'essentiel de ce que l'utilisateur attend ; ouvrir en plus une session SMTP
/// doublerait l'attente pour vérifier ce que beaucoup d'hébergeurs refusent de dire
/// sans envoi réel.
///
/// La session est refermée proprement : un serveur qui compte les connexions ne doit
/// pas payer nos vérifications.
pub async fn verify_login(config: &ServerConfig, password: &str) -> Result<()> {
    let endpoint = Endpoint {
        host: config.imap_host.clone(),
        port: config.imap_port,
        tls_immediate: config.imap_transport == iris_discover::Transport::Tls,
    };

    // Its login when it is not the address: a profile's `jdoe`, a host's own.
    let identifiants = Credentials::Password {
        user: config
            .imap_user
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| config.email.trim().to_lowercase()),
        password: password.to_string(),
    };

    // Within a limit: port 993 left in the clear, or a server that takes the
    // connection and says nothing, kept the screen waiting for ever.
    const LIMITE: std::time::Duration = std::time::Duration::from_secs(30);
    let mut connexion = tokio::time::timeout(
        LIMITE,
        RustlsConnector::new().connect(&endpoint, &identifiants),
    )
    .await
    .map_err(|_| {
        Error::network(format!(
            "{}:{} did not answer within {} seconds: check the server, its port and \
             whether it is encrypted",
            config.imap_host,
            config.imap_port,
            LIMITE.as_secs()
        ))
    })??;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), connexion.logout()).await;
    Ok(())
}

/// Ajoute un compte dont la configuration est fournie à la main.
///
/// Le repli quand la découverte échoue. Il existe parce qu'aucune chaîne de
/// découverte ne couvre tout, et qu'un utilisateur bloqué doit garder une porte.
pub fn add_account_manual(
    store: &Store,
    secrets: &dyn SecretStore,
    config: &ServerConfig,
    password: &str,
    group: Option<String>,
    now: Timestamp,
) -> Result<AccountId> {
    add_account_manual_with(store, secrets, config, password, None, group, now)
}

/// [`add_account_manual`], with a password of its own for sending (a profile's
/// `OutgoingPassword`).
pub fn add_account_manual_with(
    store: &Store,
    secrets: &dyn SecretStore,
    config: &ServerConfig,
    password: &str,
    smtp_password: Option<&str>,
    group: Option<String>,
    now: Timestamp,
) -> Result<AccountId> {
    let email = config.email.trim().to_lowercase();

    // Before anything is written. The secret is kept under the address: written for
    // an account that already exists, then cleaned up when the store refused the
    // duplicate, it deleted that account's own password.
    if store.account_by_email(&email)?.is_some() {
        return Err(Error::Config(format!("le compte « {email} » existe déjà")));
    }

    secrets.set(&email, SecretKind::Password, &Secret::new(password))?;
    if let Some(envoi) = smtp_password.filter(|p| !p.is_empty()) {
        if let Err(e) = secrets.set(&email, SecretKind::SmtpPassword, &Secret::new(envoi)) {
            let _ = secrets.delete(&email, SecretKind::Password);
            return Err(e);
        }
    }

    let nouveau = NewAccount {
        email: email.clone(),
        display_name: String::new(),
        imap_host: config.imap_host.clone(),
        imap_port: config.imap_port,
        imap_tls: config.imap_transport == iris_discover::Transport::Tls,
        smtp_host: config.smtp_host.clone(),
        smtp_port: config.smtp_port,
        smtp_tls: config.smtp_transport == iris_discover::Transport::Tls,
        auth: AuthKind::Password,
        group,
        imap_user: config.imap_user.clone().unwrap_or_default(),
        smtp_user: config.smtp_user.clone().unwrap_or_default(),
    };

    store.create_account(&nouveau, now).inspect_err(|_| {
        let _ = secrets.delete(&email, SecretKind::Password);
        let _ = secrets.delete(&email, SecretKind::SmtpPassword);
    })
}

/// Réécrit un compte existant : son adresse, ses serveurs, son mot de passe.
///
/// The password is optional because the two reasons to open this screen are not the
/// same. A provider that has moved its servers needs the hosts changed and the
/// password left alone; a password that has been rotated needs the opposite. Asking
/// for both every time would mean retyping a working password to fix a hostname.
pub fn update_account_manual(
    store: &Store,
    secrets: &dyn SecretStore,
    id: AccountId,
    config: &ServerConfig,
    password: Option<&str>,
    now: Timestamp,
) -> Result<()> {
    let email = config.email.trim().to_lowercase();

    let ancien = store
        .account(id)?
        .ok_or_else(|| Error::Config(format!("le compte {id} n'existe plus")))?;

    // Une autre boîte porte déjà cette adresse : la refuser vaut mieux que créer deux
    // comptes indiscernables dans une liste de cent.
    if email != ancien.email {
        if let Some(autre) = store.account_by_email(&email)? {
            if autre.id != id {
                return Err(Error::Config(format!("le compte « {email} » existe déjà")));
            }
        }
    }

    // L'adresse est la clé du coffre : changer l'une sans déplacer l'autre laisserait
    // le compte sans mot de passe au prochain démarrage. Every secret moves, the
    // tokens of an account signed in with Google included: they stayed under the old
    // address, and the account could not sign in any more.
    if email != ancien.email {
        for nature in SecretKind::ALL {
            if let Some(secret) = secrets.get(&ancien.email, nature)? {
                secrets.set(&email, nature, &secret)?;
            }
        }
    }
    if let Some(motdepasse) = password.filter(|p| !p.is_empty()) {
        set_password(secrets, &email, motdepasse)?;
    }

    store.update_account_servers(
        id,
        &iris_store::AccountServers {
            email: email.clone(),
            imap_host: config.imap_host.clone(),
            imap_port: config.imap_port,
            imap_tls: config.imap_transport == iris_discover::Transport::Tls,
            smtp_host: config.smtp_host.clone(),
            smtp_port: config.smtp_port,
            smtp_tls: config.smtp_transport == iris_discover::Transport::Tls,
            imap_user: config.imap_user.clone().unwrap_or_default(),
            // Not on the screen, so not in what it gives: kept as it was. Editing an
            // account whose profile signs in to send as `jdoe-smtp` emptied it, and
            // every message was refused after.
            smtp_user: config
                .smtp_user
                .clone()
                .unwrap_or_else(|| ancien.smtp_user.clone()),
        },
    )?;

    // L'ancienne entrée du coffre ne sert plus à rien et porte encore un mot de passe
    // valable : la laisser serait laisser traîner un secret que plus personne ne lit.
    if email != ancien.email {
        for nature in SecretKind::ALL {
            let _ = secrets.delete(&ancien.email, nature);
        }
    }

    store.touch_account(id, now)?;
    Ok(())
}

/// Interroge la chaîne de découverte, sans rien créer.
///
/// Séparé de la création parce qu'un compte OAuth ne peut pas être créé avant que
/// l'utilisateur ait autorisé : il faut d'abord savoir *quel* fournisseur demander.
pub async fn discover(email: &str) -> Result<iris_discover::Discovered> {
    let email = email.trim().to_lowercase();
    Discovery::new(RealIo::new()).discover(&email).await
}

/// Crée un compte dont l'autorisation est déjà dans le coffre.
///
/// Aucun mot de passe n'est écrit : un compte OAuth n'en a pas, et en enregistrer un
/// vide laisserait croire à un secret là où il n'y en a pas.
pub fn add_account_oauth(
    store: &Store,
    config: &ServerConfig,
    group: Option<String>,
    now: Timestamp,
) -> Result<AccountId> {
    let email = config.email.trim().to_lowercase();

    if store.account_by_email(&email)?.is_some() {
        return Err(Error::Config(format!("le compte « {email} » existe déjà")));
    }

    let auth = match config.auth {
        Auth::OAuthGoogle => AuthKind::OAuthGoogle,
        Auth::OAuthMicrosoft => AuthKind::OAuthMicrosoft,
        // Appelé sur une configuration par mot de passe, c'est une erreur de
        // programmation : la dire vaut mieux que créer un compte qui n'ouvrira pas.
        Auth::Password => {
            return Err(Error::Config(
                "ce compte n'utilise pas de fournisseur d'identité".into(),
            ))
        }
    };

    store.create_account(
        &NewAccount {
            email,
            display_name: String::new(),
            imap_host: config.imap_host.clone(),
            imap_port: config.imap_port,
            imap_tls: config.imap_transport == iris_discover::Transport::Tls,
            smtp_host: config.smtp_host.clone(),
            smtp_port: config.smtp_port,
            smtp_tls: config.smtp_transport == iris_discover::Transport::Tls,
            auth,
            group,
            imap_user: String::new(),
            smtp_user: String::new(),
        },
        now,
    )
}

/// La configuration proposée quand la découverte n'a rien trouvé.
///
/// Ce ne sont pas des devinettes gratuites : `imap.domaine` et `smtp.domaine` en TLS
/// sont la convention que suit la grande majorité des hébergeurs. Un formulaire
/// vide obligerait à tout taper ; un formulaire prérempli ne demande que de corriger
/// ce qui diffère.
pub fn manual_defaults(email: &str) -> ServerConfig {
    let email = email.trim().to_lowercase();
    let domaine = email
        .rsplit_once('@')
        .map(|(_, d)| d.to_string())
        .unwrap_or_default();

    ServerConfig {
        provider: None,
        imap_host: if domaine.is_empty() {
            String::new()
        } else {
            format!("imap.{domaine}")
        },
        imap_port: 993,
        imap_transport: iris_discover::Transport::Tls,
        smtp_host: if domaine.is_empty() {
            String::new()
        } else {
            format!("smtp.{domaine}")
        },
        smtp_port: 465,
        smtp_transport: iris_discover::Transport::Tls,
        auth: Auth::Password,
        note: None,
        email,
        imap_user: None,
        smtp_user: None,
    }
}

/// A new password for a mailbox, typed where Iris shows one password.
///
/// Sending prefers a profile's own sending password when there is one: a password
/// changed on its own left sending on the old one, refused for ever.
pub fn set_password(secrets: &dyn SecretStore, email: &str, password: &str) -> Result<()> {
    secrets.set(email, SecretKind::Password, &Secret::new(password))?;
    if secrets.get(email, SecretKind::SmtpPassword)?.is_some() {
        secrets.set(email, SecretKind::SmtpPassword, &Secret::new(password))?;
    }
    Ok(())
}

/// Supprime un compte, ses messages et ses secrets.
pub fn remove_account(store: &Store, secrets: &dyn SecretStore, id: AccountId) -> Result<bool> {
    let Some(compte) = store.account(id)? else {
        return Ok(false);
    };

    // Les secrets d'abord : un compte supprimé dont le mot de passe traînerait encore
    // dans le trousseau serait une fuite silencieuse.
    for nature in SecretKind::ALL {
        let _ = secrets.delete(&compte.email, nature);
    }
    store.delete_account(id)
}

#[cfg(test)]
mod tests_manuel {
    use super::*;

    fn store() -> Store {
        Store::in_memory().unwrap()
    }

    fn config_oauth(email: &str, auth: Auth) -> ServerConfig {
        ServerConfig {
            provider: Some("Test".into()),
            email: email.into(),
            imap_host: "imap.test".into(),
            imap_port: 993,
            imap_transport: iris_discover::Transport::Tls,
            smtp_host: "smtp.test".into(),
            smtp_port: 465,
            smtp_transport: iris_discover::Transport::Tls,
            auth,
            note: None,
            imap_user: None,
            smtp_user: None,
        }
    }

    #[test]
    fn un_compte_oauth_se_cree_sans_mot_de_passe() {
        // En enregistrer un vide laisserait croire à un secret là où il n'y en a pas.
        let s = store();
        let id = add_account_oauth(
            &s,
            &config_oauth("moi@gmail.com", Auth::OAuthGoogle),
            None,
            Timestamp::EPOCH,
        )
        .unwrap();

        let compte = s.account(id).unwrap().unwrap();
        assert_eq!(compte.auth, AuthKind::OAuthGoogle);
        assert_eq!(compte.email, "moi@gmail.com");
    }

    #[test]
    fn microsoft_est_reconnu_aussi() {
        let s = store();
        let id = add_account_oauth(
            &s,
            &config_oauth("moi@outlook.com", Auth::OAuthMicrosoft),
            None,
            Timestamp::EPOCH,
        )
        .unwrap();
        assert_eq!(
            s.account(id).unwrap().unwrap().auth,
            AuthKind::OAuthMicrosoft
        );
    }

    #[test]
    fn creer_un_compte_par_mot_de_passe_par_cette_porte_est_refuse() {
        // Le compte s'ouvrirait sans jamais pouvoir s'authentifier.
        let s = store();
        let erreur = add_account_oauth(
            &s,
            &config_oauth("moi@x.fr", Auth::Password),
            None,
            Timestamp::EPOCH,
        )
        .unwrap_err()
        .to_string();
        assert!(
            erreur.contains("fournisseur d'identité"),
            "obtenu : {erreur}"
        );
    }

    #[test]
    fn un_compte_oauth_en_double_est_refuse() {
        let s = store();
        let config = config_oauth("moi@gmail.com", Auth::OAuthGoogle);
        add_account_oauth(&s, &config, None, Timestamp::EPOCH).unwrap();

        assert!(add_account_oauth(&s, &config, None, Timestamp::EPOCH).is_err());
    }

    #[test]
    fn les_valeurs_par_defaut_suivent_la_convention_du_domaine() {
        let config = manual_defaults("Marie@Exemple.FR");
        assert_eq!(config.email, "marie@exemple.fr");
        assert_eq!(config.imap_host, "imap.exemple.fr");
        assert_eq!(config.imap_port, 993);
        assert_eq!(config.smtp_host, "smtp.exemple.fr");
        assert_eq!(config.smtp_port, 465);
    }

    #[test]
    fn le_chiffrement_est_le_defaut() {
        // Proposer du clair par défaut ferait de l'oubli une fuite.
        let config = manual_defaults("a@x.fr");
        assert_eq!(config.imap_transport, iris_discover::Transport::Tls);
        assert_eq!(config.smtp_transport, iris_discover::Transport::Tls);
    }

    #[test]
    fn une_adresse_sans_arobase_ne_devine_rien() {
        let config = manual_defaults("pas-une-adresse");
        assert!(config.imap_host.is_empty());
        assert!(config.smtp_host.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_secrets::EncryptedVault;

    fn coffre(dir: &std::path::Path) -> EncryptedVault {
        EncryptedVault::open(dir.join("coffre.json"), &Secret::new("maitre")).unwrap()
    }

    #[test]
    fn l_ajout_manuel_enregistre_compte_et_secret() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        let coffre = coffre(dir.path());

        let config = ServerConfig {
            provider: None,
            email: "moi@mondomaine.fr".into(),
            imap_host: "imap.mondomaine.fr".into(),
            imap_port: 993,
            imap_transport: iris_discover::Transport::Tls,
            smtp_host: "smtp.mondomaine.fr".into(),
            smtp_port: 587,
            smtp_transport: iris_discover::Transport::StartTls,
            auth: Auth::Password,
            note: None,
            imap_user: None,
            smtp_user: None,
        };

        let id =
            add_account_manual(&store, &coffre, &config, "secret", None, Timestamp::EPOCH).unwrap();

        let compte = store.account(id).unwrap().unwrap();
        assert_eq!(compte.imap_host, "imap.mondomaine.fr");
        assert!(compte.imap_tls);
        assert!(!compte.smtp_tls, "STARTTLS n'est pas du TLS direct");

        let secret = coffre
            .get("moi@mondomaine.fr", SecretKind::Password)
            .unwrap()
            .unwrap();
        assert_eq!(secret.expose(), "secret");
    }

    #[test]
    fn adding_an_account_twice_leaves_the_first_one_s_password_alone() {
        // The secret is kept under the address. Written for the duplicate, then
        // cleaned up when the store refused it, it deleted the existing account's
        // password: re-importing a profile broke the mailbox it was for.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        let coffre = coffre(dir.path());

        let config = ServerConfig {
            provider: None,
            email: "moi@x.fr".into(),
            imap_host: "i".into(),
            imap_port: 993,
            imap_transport: iris_discover::Transport::Tls,
            smtp_host: "s".into(),
            smtp_port: 465,
            smtp_transport: iris_discover::Transport::Tls,
            auth: Auth::Password,
            note: None,
            imap_user: None,
            smtp_user: None,
        };

        add_account_manual(&store, &coffre, &config, "un", None, Timestamp::EPOCH).unwrap();

        assert!(
            add_account_manual(&store, &coffre, &config, "deux", None, Timestamp::EPOCH).is_err()
        );
        let garde = coffre
            .get("moi@x.fr", SecretKind::Password)
            .unwrap()
            .unwrap();
        assert_eq!(garde.expose(), "un", "the first account keeps its password");
    }

    #[test]
    fn a_profile_s_logins_and_sending_password_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        let coffre = coffre(dir.path());
        let config = ServerConfig {
            provider: None,
            email: "jdoe@corp.example.com".into(),
            imap_host: "mail.corp.example.com".into(),
            imap_port: 993,
            imap_transport: iris_discover::Transport::Tls,
            smtp_host: "mail.corp.example.com".into(),
            smtp_port: 587,
            smtp_transport: iris_discover::Transport::StartTls,
            auth: Auth::Password,
            note: None,
            imap_user: Some("CORP\\jdoe".into()),
            smtp_user: None,
        };
        let id = add_account_manual_with(
            &store,
            &coffre,
            &config,
            "lecture",
            Some("envoi"),
            None,
            Timestamp::EPOCH,
        )
        .unwrap();

        let compte = store.account(id).unwrap().unwrap();
        assert_eq!(compte.imap_login(), "CORP\\jdoe");
        assert_eq!(compte.smtp_login(), "CORP\\jdoe");
        let envoi = coffre
            .get("jdoe@corp.example.com", SecretKind::SmtpPassword)
            .unwrap()
            .unwrap();
        assert_eq!(envoi.expose(), "envoi");
    }

    #[test]
    fn supprimer_un_compte_efface_aussi_ses_secrets() {
        // Un mot de passe qui traînerait dans le trousseau après suppression serait
        // une fuite silencieuse.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        let coffre = coffre(dir.path());

        let config = ServerConfig {
            provider: None,
            email: "moi@x.fr".into(),
            imap_host: "i".into(),
            imap_port: 993,
            imap_transport: iris_discover::Transport::Tls,
            smtp_host: "s".into(),
            smtp_port: 465,
            smtp_transport: iris_discover::Transport::Tls,
            auth: Auth::Password,
            note: None,
            imap_user: None,
            smtp_user: None,
        };
        let id =
            add_account_manual(&store, &coffre, &config, "secret", None, Timestamp::EPOCH).unwrap();

        assert!(remove_account(&store, &coffre, id).unwrap());
        assert!(coffre
            .get("moi@x.fr", SecretKind::Password)
            .unwrap()
            .is_none());
        assert!(store.account(id).unwrap().is_none());
    }

    #[test]
    fn supprimer_un_compte_inexistant_ne_fait_rien() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        assert!(!remove_account(&store, &coffre(dir.path()), AccountId(999)).unwrap());
    }
}
