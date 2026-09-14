//! Ajout de comptes.
//!
//! L'objectif produit : **une adresse et un mot de passe suffisent**. La découverte
//! s'occupe du reste, et quand elle échoue, elle dit ce qu'elle a essayé au lieu de
//! renvoyer un formulaire vide.
//!
//! Deux entrées : une par compte, et un import en lot, parce qu'ajouter cent boîtes
//! une par une n'est pas une expérience, c'est une punition.

use iris_discover::{Auth, Discovery, RealIo, ServerConfig, Source};
use iris_secrets::{Secret, SecretKind, SecretStore};
use iris_store::{AuthKind, NewAccount, Store};
use iris_types::{AccountId, Error, Result, Timestamp};
use std::sync::Arc;

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

/// Une ligne d'import en lot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkEntry {
    pub email: String,
    pub password: String,
    /// Groupe libre, pour ranger cent boîtes.
    pub group: Option<String>,
}

/// Résultat d'un import en lot.
#[derive(Debug, Clone, Default)]
pub struct BulkReport {
    pub added: Vec<AddedAccount>,
    pub failed: Vec<(String, String)>,
}

impl BulkReport {
    pub fn summary(&self) -> String {
        match (self.added.len(), self.failed.len()) {
            (n, 0) => format!("{n} compte(s) ajouté(s)."),
            (0, m) => format!("Aucun compte ajouté, {m} échec(s)."),
            (n, m) => format!("{n} compte(s) ajouté(s), {m} échec(s)."),
        }
    }
}

/// Analyse un fichier d'import.
///
/// Format volontairement trivial : `adresse;mot de passe;groupe`, une ligne par
/// compte, `#` pour les commentaires. Demander un format structuré pour une liste de
/// mots de passe serait une coquetterie.
pub fn parse_bulk(contents: &str) -> (Vec<BulkEntry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut erreurs = Vec::new();

    for (numero, ligne) in contents.lines().enumerate() {
        let ligne = ligne.trim();
        if ligne.is_empty() || ligne.starts_with('#') {
            continue;
        }

        let champs: Vec<&str> = ligne.split(';').map(str::trim).collect();
        match champs.as_slice() {
            [email, password, reste @ ..] if !email.is_empty() && !password.is_empty() => {
                entries.push(BulkEntry {
                    email: email.to_lowercase(),
                    password: (*password).to_string(),
                    group: reste
                        .first()
                        .map(|g| g.to_string())
                        .filter(|g| !g.is_empty()),
                });
            }
            _ => erreurs.push(format!(
                "ligne {} : « {ligne} » n'est pas exploitable",
                numero + 1
            )),
        }
    }

    (entries, erreurs)
}

/// Ajoute un compte à partir de son adresse et de son mot de passe.
pub async fn add_account(
    store: &Store,
    secrets: &dyn SecretStore,
    email: &str,
    password: &str,
    group: Option<String>,
    now: Timestamp,
) -> Result<AddedAccount> {
    let email = email.trim().to_lowercase();

    if store.account_by_email(&email)?.is_some() {
        return Err(Error::Config(format!("le compte « {email} » existe déjà")));
    }

    let decouverte = Discovery::new(RealIo::new()).discover(&email).await?;
    let config = decouverte.config.clone();

    // Le mot de passe est enregistré **avant** le compte : si le trousseau refuse,
    // mieux vaut n'avoir rien créé qu'un compte inutilisable.
    secrets.set(&email, SecretKind::Password, &Secret::new(password))?;

    let nouveau = NewAccount {
        email: email.clone(),
        display_name: String::new(),
        imap_host: config.imap_host.clone(),
        imap_port: config.imap_port,
        imap_tls: config.imap_transport == iris_discover::Transport::Tls,
        smtp_host: config.smtp_host.clone(),
        smtp_port: config.smtp_port,
        smtp_tls: config.smtp_transport == iris_discover::Transport::Tls,
        auth: match config.auth {
            Auth::Password => AuthKind::Password,
            Auth::OAuthGoogle => AuthKind::OAuthGoogle,
            Auth::OAuthMicrosoft => AuthKind::OAuthMicrosoft,
        },
        group,
    };

    let id = match store.create_account(&nouveau, now) {
        Ok(id) => id,
        Err(e) => {
            // On ne laisse pas un secret orphelin derrière soi.
            let _ = secrets.delete(&email, SecretKind::Password);
            return Err(e);
        }
    };

    Ok(AddedAccount {
        id,
        email,
        needs_review: !decouverte.source.is_authoritative(),
        source: decouverte.source,
        config,
    })
}

/// Ajoute plusieurs comptes.
///
/// Un échec n'interrompt pas le lot : sur cent lignes, une adresse mal orthographiée
/// ne doit pas condamner les quatre-vingt-dix-neuf autres.
pub async fn add_bulk(
    store: Arc<Store>,
    secrets: Arc<dyn SecretStore>,
    entries: &[BulkEntry],
    now: Timestamp,
) -> BulkReport {
    let mut rapport = BulkReport::default();

    for entree in entries {
        match add_account(
            &store,
            secrets.as_ref(),
            &entree.email,
            &entree.password,
            entree.group.clone(),
            now,
        )
        .await
        {
            Ok(compte) => rapport.added.push(compte),
            Err(e) => rapport.failed.push((entree.email.clone(), e.to_string())),
        }
    }

    rapport
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
    let email = config.email.trim().to_lowercase();
    secrets.set(&email, SecretKind::Password, &Secret::new(password))?;

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
    };

    store.create_account(&nouveau, now).inspect_err(|_| {
        let _ = secrets.delete(&email, SecretKind::Password);
    })
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
    }
}

/// Supprime un compte, ses messages et ses secrets.
pub fn remove_account(store: &Store, secrets: &dyn SecretStore, id: AccountId) -> Result<bool> {
    let Some(compte) = store.account(id)? else {
        return Ok(false);
    };

    // Les secrets d'abord : un compte supprimé dont le mot de passe traînerait encore
    // dans le trousseau serait une fuite silencieuse.
    for nature in [
        SecretKind::Password,
        SecretKind::AccessToken,
        SecretKind::RefreshToken,
    ] {
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
    fn l_import_analyse_les_lignes_completes() {
        let (entrees, erreurs) = parse_bulk(
            "# mes comptes\n\
             contact@example.com;motdepasse;Clients\n\
             \n\
             facturation@example.com;autre\n",
        );

        assert!(erreurs.is_empty());
        assert_eq!(entrees.len(), 2);
        assert_eq!(entrees[0].group.as_deref(), Some("Clients"));
        assert_eq!(entrees[1].group, None);
    }

    #[test]
    fn l_import_signale_les_lignes_inexploitables_sans_tout_rejeter() {
        // Sur cent lignes, une faute ne doit pas condamner les autres.
        let (entrees, erreurs) = parse_bulk(
            "bon@example.com;motdepasse\n\
             ligne sans separateur\n\
             ;motdepasse\n\
             autre@example.com;motdepasse\n",
        );

        assert_eq!(entrees.len(), 2);
        assert_eq!(erreurs.len(), 2);
        assert!(erreurs[0].contains("ligne 2"));
    }

    #[test]
    fn l_import_ignore_les_commentaires_et_les_lignes_vides() {
        let (entrees, erreurs) = parse_bulk("# rien\n\n   \n# encore\n");
        assert!(entrees.is_empty());
        assert!(erreurs.is_empty());
    }

    #[test]
    fn l_import_normalise_les_adresses() {
        let (entrees, _) = parse_bulk("  Contact@Example.COM ; motdepasse \n");
        assert_eq!(entrees[0].email, "contact@example.com");
        assert_eq!(entrees[0].password, "motdepasse");
    }

    #[test]
    fn un_mot_de_passe_contenant_des_espaces_est_conserve() {
        let (entrees, _) = parse_bulk("a@x.fr;mot de passe long\n");
        assert_eq!(entrees[0].password, "mot de passe long");
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
    fn un_compte_en_double_ne_laisse_pas_de_secret_orphelin() {
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
        };

        add_account_manual(&store, &coffre, &config, "un", None, Timestamp::EPOCH).unwrap();
        coffre.delete("moi@x.fr", SecretKind::Password).unwrap();

        // Second ajout : le store refuse, le secret ne doit pas survivre.
        assert!(
            add_account_manual(&store, &coffre, &config, "deux", None, Timestamp::EPOCH).is_err()
        );
        assert!(coffre
            .get("moi@x.fr", SecretKind::Password)
            .unwrap()
            .is_none());
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

    #[test]
    fn le_resume_d_import_est_lisible() {
        let mut r = BulkReport::default();
        assert!(r.summary().contains("0 compte"));

        r.failed.push(("a@x.fr".into(), "échec".into()));
        assert!(r.summary().starts_with("Aucun compte ajouté"));
    }
}
