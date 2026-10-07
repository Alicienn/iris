//! La connexion par fournisseur d'identité, de bout en bout.
//!
//! `iris-oauth` sait construire une URL, vérifier un jeton anti-rejeu et analyser une
//! réponse ; ce module fait le reste : ouvrir le navigateur, attendre le retour,
//! ranger les jetons, et **les renouveler avant qu'ils n'expirent**. C'est ce dernier
//! point qui distingue un compte qui marche d'un compte qui marche une heure.
//!
//! Un détail qui n'en est pas un : l'identifiant client n'est pas dans le binaire.
//! Un secret distribué à tout le monde n'est pas un secret, et surtout, celui d'Iris
//! devrait être enregistré auprès de chaque fournisseur — ce qui n'est pas quelque
//! chose qu'un fichier source peut faire. Il se configure donc, et son absence est
//! annoncée clairement plutôt que déguisée en échec d'authentification.

use iris_oauth::{HttpEndpoint, Provider, Tokens};
use iris_secrets::{Secret, SecretKind, SecretStore};
use iris_store::AuthKind;
use iris_types::{Error, Result, Timestamp};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Marge avant péremption, en secondes.
///
/// Un jeton valide encore soixante secondes ne survivra pas à une synchronisation
/// complète : on le renouvelle d'avance plutôt que d'échouer au milieu.
const MARGE_SECONDES: i64 = 120;

/// Délai maximal accordé à l'utilisateur pour autoriser.
const DELAI_AUTORISATION: Duration = Duration::from_secs(300);

/// Les identifiants clients, tels qu'ils sont configurés.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuthSettings {
    pub google_client_id: String,
    /// The secret Google gives a desktop client, and asks back even with PKCE. It is
    /// not confidential (every copy of an app carries it), so it sits in the settings
    /// beside the ID. Microsoft's public clients have none.
    pub google_client_secret: String,
    pub microsoft_client_id: String,
}

impl OAuthSettings {
    /// The client secret to send, when the provider's client has one.
    pub fn client_secret(&self, provider: Provider) -> Option<&str> {
        match provider {
            Provider::Google => Some(self.google_client_secret.trim()).filter(|s| !s.is_empty()),
            Provider::Microsoft => None,
        }
    }

    pub fn client_id(&self, provider: Provider) -> Option<&str> {
        let brut = match provider {
            Provider::Google => &self.google_client_id,
            Provider::Microsoft => &self.microsoft_client_id,
        };
        let brut = brut.trim();
        (!brut.is_empty()).then_some(brut)
    }

    pub fn is_configured(&self, provider: Provider) -> bool {
        self.client_id(provider).is_some()
    }
}

/// Le jeton d'accès, tel qu'il est rangé dans le coffre.
///
/// Le jeton nu ne suffit pas : sans sa date de péremption, on ne peut pas savoir
/// qu'il faut le renouveler, et on ne l'apprend qu'en échouant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredAccess {
    token: String,
    /// Millisecondes depuis l'époque.
    expires_at: i64,
}

/// Le fournisseur correspondant au mode d'authentification d'un compte.
pub fn provider_for(auth: AuthKind) -> Option<Provider> {
    match auth {
        AuthKind::OAuthGoogle => Some(Provider::Google),
        AuthKind::OAuthMicrosoft => Some(Provider::Microsoft),
        AuthKind::Password => None,
    }
}

/// Range les jetons dans le coffre.
///
/// Le jeton de rafraîchissement n'est écrit que s'il y en a un : le fournisseur ne
/// le renvoie pas à chaque renouvellement, et l'écraser par du vide déconnecterait
/// le compte au renouvellement suivant.
pub fn store_tokens(secrets: &dyn SecretStore, email: &str, tokens: &Tokens) -> Result<()> {
    let acces = StoredAccess {
        token: tokens.access_token.clone(),
        expires_at: tokens.expires_at.millis(),
    };
    let encode =
        serde_json::to_string(&acces).map_err(|e| Error::other(format!("jeton d'accès : {e}")))?;

    secrets.set(email, SecretKind::AccessToken, &Secret::new(encode))?;

    if let Some(refresh) = &tokens.refresh_token {
        secrets.set(
            email,
            SecretKind::RefreshToken,
            &Secret::new(refresh.clone()),
        )?;
    }
    Ok(())
}

/// Lit le jeton d'accès rangé, s'il est encore valable.
///
/// Rend `None` quand il n'y en a pas, ou quand il expire trop tôt pour être utile.
pub fn valid_access_token(
    secrets: &dyn SecretStore,
    email: &str,
    now: Timestamp,
) -> Result<Option<String>> {
    let Some(brut) = secrets.get(email, SecretKind::AccessToken)? else {
        return Ok(None);
    };

    // Les coffres d'avant ce format contiennent le jeton nu, sans échéance. Rien ne le
    // renouvelait une fois périmé : il est tenu pour expiré, et le jeton de
    // rafraîchissement en donne un neuf, réécrit au bon format.
    let Ok(acces) = serde_json::from_str::<StoredAccess>(brut.expose()) else {
        return Ok(None);
    };

    if acces.expires_at - now.millis() < MARGE_SECONDES * 1000 {
        return Ok(None);
    }
    Ok(Some(acces.token))
}

/// Renouvelle le jeton d'accès à partir du jeton de rafraîchissement.
pub async fn refresh_access(
    secrets: &dyn SecretStore,
    settings: &OAuthSettings,
    provider: Provider,
    email: &str,
    now: Timestamp,
) -> Result<String> {
    let client_id = settings.client_id(provider).ok_or_else(|| {
        Error::Config(format!(
            "aucun identifiant client {} n'est configuré : la connexion à ce compte est \
             impossible tant qu'il manque",
            provider.label()
        ))
    })?;

    let refresh = secrets
        .get(email, SecretKind::RefreshToken)?
        .ok_or_else(|| Error::AuthFailed {
            account: email.to_string(),
        })?;

    let endpoint = HttpEndpoint::new()?;
    let jetons = iris_oauth::refresh(
        &endpoint,
        provider,
        client_id,
        settings.client_secret(provider),
        refresh.expose(),
        now,
    )
    .await?;

    store_tokens(secrets, email, &jetons)?;
    Ok(jetons.access_token)
}

/// Conduit une autorisation complète, du navigateur au coffre.
///
/// Bloquante par endroits — l'attente de la redirection l'est — donc appelée depuis
/// une tâche, jamais depuis la boucle d'interface.
pub async fn authorize(
    secrets: Arc<dyn SecretStore>,
    settings: &OAuthSettings,
    provider: Provider,
    email: &str,
    now: Timestamp,
) -> Result<Tokens> {
    authorize_with(secrets, settings, provider, email, now, &[]).await
}

/// The same, asking also for `extra` (Google's calendars, when they are connected).
pub async fn authorize_with(
    secrets: Arc<dyn SecretStore>,
    settings: &OAuthSettings,
    provider: Provider,
    email: &str,
    now: Timestamp,
    extra: &[&str],
) -> Result<Tokens> {
    let client_id = settings.client_id(provider).ok_or_else(|| {
        Error::Config(format!(
            "aucun identifiant client {} n'est configuré",
            provider.label()
        ))
    })?;

    // Loopback is checked before the browser opens. Sending someone to their provider,
    // watching them type their password, and only then discovering that the answer has
    // nowhere to land is the worst possible order to find this out.
    if !iris_oauth::loopback_works() {
        return Err(Error::Config(
            "this machine cannot open a local connection, which browser sign-in needs.              A firewall or security policy is usually the cause."
                .into(),
        ));
    }

    // The port is reserved before opening the browser: between the two the user may
    // already have accepted, and a redirect arriving on a closed port loses the
    // authorisation without a word.
    let (ecoute, port) = iris_oauth::reserve_port()?;

    let entropie = entropie();
    let demande = iris_oauth::begin_with(provider, client_id, port, Some(email), &entropie, extra);

    iris_oauth::open_browser(&demande)?;

    // L'attente est bloquante : elle part sur un fil dédié, pour ne pas immobiliser
    // l'exécuteur pendant que l'utilisateur cherche son mot de passe.
    let etat = demande.state.clone();
    let redirection = tokio::task::spawn_blocking(move || {
        iris_oauth::wait_for_redirect_from(ecoute, DELAI_AUTORISATION, &etat)
    })
    .await
    .map_err(|e| Error::other(format!("attente de l'autorisation : {e}")))??;

    let code = iris_oauth::parse_redirect(&redirection.url, &demande.state)?;

    let endpoint = HttpEndpoint::new()?;
    let jetons = iris_oauth::exchange_code(
        &endpoint,
        &demande,
        client_id,
        settings.client_secret(provider),
        &code,
        now,
    )
    .await?;

    // Sans jeton de rafraîchissement, le compte se déconnecte en silence au bout
    // d'une heure. Le dire tout de suite vaut mieux que le découvrir demain.
    if jetons.refresh_token.is_none() {
        tracing::warn!(
            account = %email,
            "aucun jeton de rafraîchissement reçu : l'accès expirera sans réautorisation"
        );
    }

    // Signed in as someone else in the browser: said, and nothing kept. The tokens
    // were stored under the address typed, and every sync then failed as a refused
    // password.
    signed_in_as(email, jetons.email.as_deref())?;

    store_tokens(secrets.as_ref(), email, &jetons)?;
    Ok(jetons)
}

/// Checks the identity the provider vouched for against the address typed.
fn signed_in_as(typed: &str, vouched: Option<&str>) -> Result<()> {
    match vouched {
        Some(qui) if !qui.trim().eq_ignore_ascii_case(typed.trim()) => Err(Error::Config(format!(
            "you signed in as {qui} in the browser, not as {typed}: sign in again as {typed}"
        ))),
        _ => Ok(()),
    }
}

/// De l'aléa pour le vérificateur PKCE et le jeton anti-rejeu.
///
/// `RandomState` est ensemencé par le système d'exploitation à chaque construction :
/// c'est la source d'aléa que la bibliothèque standard expose sans dépendance, et
/// elle est faite pour résister à un adversaire qui choisirait les entrées. Le
/// vérificateur ne vit que le temps d'un échange, mais il doit être imprévisible
/// pendant ce temps-là, sans quoi PKCE ne protège plus rien.
fn entropie() -> Vec<u8> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let mut graine = Vec::with_capacity(48);
    for i in 0..6u64 {
        let mut hacheur = RandomState::new().build_hasher();
        hacheur.write_u64(i);
        graine.extend_from_slice(&hacheur.finish().to_le_bytes());
    }
    graine
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_secrets::EncryptedVault;

    /// Un coffre isolé : les tests ne touchent jamais le trousseau de la machine.
    fn coffre() -> (Arc<dyn SecretStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let vault =
            EncryptedVault::open(dir.path().join("coffre.json"), &Secret::new("maitre")).unwrap();
        (Arc::new(vault), dir)
    }

    fn jetons(acces: &str, refresh: Option<&str>, expire_dans: i64) -> Tokens {
        Tokens {
            access_token: acces.into(),
            refresh_token: refresh.map(|r| r.to_string()),
            expires_at: Timestamp::from_millis(expire_dans * 1000),
            email: None,
        }
    }

    #[test]
    fn un_jeton_valide_est_rendu() {
        let (coffre, _d) = coffre();
        store_tokens(
            coffre.as_ref(),
            "a@x.fr",
            &jetons("acces", Some("refresh"), 3600),
        )
        .unwrap();

        let lu = valid_access_token(coffre.as_ref(), "a@x.fr", Timestamp::EPOCH).unwrap();
        assert_eq!(lu.as_deref(), Some("acces"));
    }

    #[test]
    fn un_jeton_bientot_perime_est_refuse() {
        // Un jeton valide encore soixante secondes ne survivrait pas à une
        // synchronisation complète.
        let (coffre, _d) = coffre();
        store_tokens(coffre.as_ref(), "a@x.fr", &jetons("acces", None, 60)).unwrap();

        let lu = valid_access_token(coffre.as_ref(), "a@x.fr", Timestamp::EPOCH).unwrap();
        assert_eq!(lu, None, "il faut le renouveler avant de s'en servir");
    }

    #[test]
    fn un_jeton_deja_perime_est_refuse() {
        let (coffre, _d) = coffre();
        store_tokens(coffre.as_ref(), "a@x.fr", &jetons("acces", None, 0)).unwrap();

        let lu =
            valid_access_token(coffre.as_ref(), "a@x.fr", Timestamp::from_millis(10_000)).unwrap();
        assert_eq!(lu, None);
    }

    #[test]
    fn un_coffre_sans_jeton_ne_ment_pas() {
        let (coffre, _d) = coffre();
        assert_eq!(
            valid_access_token(coffre.as_ref(), "a@x.fr", Timestamp::EPOCH).unwrap(),
            None
        );
    }

    #[test]
    fn a_bare_token_of_the_old_format_is_renewed() {
        // With no expiry it was used for ever, and the account failed for good once
        // it lapsed. Taken as expired, the refresh token gives a fresh one, with no
        // sign-in asked of the user.
        let (coffre, _d) = coffre();
        coffre
            .set("a@x.fr", SecretKind::AccessToken, &Secret::new("jeton-nu"))
            .unwrap();

        let lu = valid_access_token(coffre.as_ref(), "a@x.fr", Timestamp::EPOCH).unwrap();
        assert_eq!(lu, None);
    }

    #[test]
    fn signing_in_as_someone_else_is_refused() {
        assert!(signed_in_as("alice@gmail.com", Some("Alice@Gmail.com")).is_ok());
        assert!(signed_in_as("alice@gmail.com", None).is_ok());
        let e = signed_in_as("alice@gmail.com", Some("bob@gmail.com")).unwrap_err();
        assert!(e.to_string().contains("bob@gmail.com"), "{e}");
    }

    #[test]
    fn le_jeton_de_rafraichissement_n_est_pas_ecrase_par_du_vide() {
        // Le fournisseur ne le renvoie pas à chaque renouvellement.
        let (coffre, _d) = coffre();
        store_tokens(
            coffre.as_ref(),
            "a@x.fr",
            &jetons("a1", Some("refresh"), 3600),
        )
        .unwrap();
        store_tokens(coffre.as_ref(), "a@x.fr", &jetons("a2", None, 3600)).unwrap();

        let refresh = coffre
            .get("a@x.fr", SecretKind::RefreshToken)
            .unwrap()
            .unwrap();
        assert_eq!(refresh.expose(), "refresh");
    }

    #[test]
    fn sans_identifiant_client_la_configuration_le_dit() {
        let reglages = OAuthSettings::default();
        assert!(!reglages.is_configured(Provider::Google));
        assert_eq!(reglages.client_id(Provider::Google), None);
    }

    #[test]
    fn un_identifiant_client_vide_ou_blanc_ne_compte_pas() {
        let reglages = OAuthSettings {
            google_client_id: "   ".into(),
            ..Default::default()
        };
        assert!(!reglages.is_configured(Provider::Google));
        assert!(!reglages.is_configured(Provider::Microsoft));
    }

    #[test]
    fn un_identifiant_client_configure_est_rendu_sans_espaces() {
        let reglages = OAuthSettings {
            google_client_id: "  abc.apps.googleusercontent.com  ".into(),
            ..Default::default()
        };
        assert_eq!(
            reglages.client_id(Provider::Google),
            Some("abc.apps.googleusercontent.com")
        );
    }

    #[tokio::test]
    async fn sans_identifiant_client_le_renouvellement_echoue_clairement() {
        // Un échec d'authentification anonyme enverrait chercher un mot de passe
        // là où il manque une configuration.
        let (coffre, _d) = coffre();
        coffre
            .set("a@x.fr", SecretKind::RefreshToken, &Secret::new("r"))
            .unwrap();

        let erreur = refresh_access(
            coffre.as_ref(),
            &OAuthSettings::default(),
            Provider::Google,
            "a@x.fr",
            Timestamp::EPOCH,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(erreur.contains("identifiant client"), "obtenu : {erreur}");
    }

    #[tokio::test]
    async fn sans_jeton_de_rafraichissement_le_renouvellement_est_impossible() {
        let (coffre, _d) = coffre();
        let reglages = OAuthSettings {
            google_client_id: "client".into(),
            ..Default::default()
        };

        let erreur = refresh_access(
            coffre.as_ref(),
            &reglages,
            Provider::Google,
            "a@x.fr",
            Timestamp::EPOCH,
        )
        .await
        .unwrap_err();
        assert!(matches!(erreur, Error::AuthFailed { .. }));
    }

    #[test]
    fn le_fournisseur_se_deduit_du_mode_d_authentification() {
        assert_eq!(provider_for(AuthKind::OAuthGoogle), Some(Provider::Google));
        assert_eq!(
            provider_for(AuthKind::OAuthMicrosoft),
            Some(Provider::Microsoft)
        );
        assert_eq!(provider_for(AuthKind::Password), None);
    }

    #[test]
    fn l_entropie_n_est_pas_deux_fois_la_meme() {
        // Un vérificateur PKCE prévisible annulerait la protection qu'il apporte.
        let a = entropie();
        let b = entropie();
        assert_ne!(a, b);
        assert!(a.len() >= 32);
    }
}
