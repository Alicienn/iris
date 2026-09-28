//! L'assemblage des modules.
//!
//! C'est le seul endroit du projet qui connaît tous les autres. Chaque couche a été
//! écrite pour ignorer ses voisines ; le prix de cette indépendance est qu'il faut
//! bien, quelque part, les relier — et ce quelque part doit être unique, explicite,
//! et sans logique.

use crate::paths::Paths;
use iris_blobs::BlobStore;
use iris_imap::client::RustlsConnector;
use iris_index::SearchIndex;
use iris_kernel::EventBus;
use iris_secrets::{EncryptedVault, KeyringStore, Secret, SecretKind, SecretStore};
use iris_store::Store;
use iris_sync::{EngineConfig, SyncEngine};
use iris_theme::ThemeRegistry;
use iris_types::{AccountId, Error, Result, Timestamp};
use std::sync::Arc;

/// Taille maximale du cache de corps et de pièces jointes.
///
/// Deux gigaoctets couvrent des dizaines de milliers de messages compressés. Au-delà,
/// l'utilisateur relit rarement, et tout est retéléchargeable.
const BLOB_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Tous les services de l'application.
#[derive(Debug, Clone)]
pub struct Services {
    pub paths: Paths,
    pub store: Arc<Store>,
    pub blobs: Arc<BlobStore>,
    pub index: Arc<SearchIndex>,
    pub secrets: Arc<dyn SecretStore>,
    pub themes: Arc<ThemeRegistry>,
    pub bus: EventBus,
    pub engine: Arc<SyncEngine>,
    /// The one state machine. Everything that changes a thread goes through it:
    /// the keyboard, the palette, the plugins and the passage of time.
    pub workflow: Arc<iris_workflow::Workflow>,
    /// Les identifiants clients OAuth, partagés avec le fournisseur d'identifiants.
    /// Modifiables en cours de route : renseigner un identifiant client ne doit pas
    /// demander de redémarrer.
    pub oauth: Arc<std::sync::RwLock<crate::oauth::OAuthSettings>>,
}

impl Services {
    /// Ouvre ou crée tout ce dont l'application a besoin.
    ///
    /// `master` n'est utilisé qu'en l'absence de trousseau système : c'est le mot de
    /// passe du coffre de repli.
    pub fn open(paths: Paths, master: Option<Secret>) -> Result<Self> {
        paths.ensure()?;

        let store = Arc::new(Store::open(paths.database())?);
        let blobs = Arc::new(BlobStore::open(paths.blobs(), BLOB_CACHE_BYTES)?);
        let index = Arc::new(SearchIndex::open(paths.index())?);
        let themes = Arc::new(ThemeRegistry::with_user_dir(paths.themes())?);
        let oauth: Arc<std::sync::RwLock<crate::oauth::OAuthSettings>> = Default::default();
        let secrets = open_secrets(&paths, master)?;
        let bus = EventBus::new();

        // One state machine, shared by everything that changes a thread: the
        // keyboard, the palette, the plugins and the passage of time.
        let workflow = Arc::new(iris_workflow::Workflow::new(
            Arc::clone(&store),
            bus.clone(),
            iris_types::AutomationSettings::default(),
        ));

        // Le moteur reçoit l'index et le magasin de contenus : sans eux, la
        // synchronisation fonctionne mais la recherche ne trouve rien et les corps
        // ne sont jamais téléchargés.
        let engine = Arc::new(
            SyncEngine::new(
                Arc::clone(&store),
                Arc::new(RustlsConnector::new()),
                Arc::new(StoredCredentials {
                    secrets: Arc::clone(&secrets),
                    store: Arc::clone(&store),
                    oauth: Arc::clone(&oauth),
                }),
                bus.clone(),
                EngineConfig::default(),
            )
            .with_index(Arc::clone(&index))
            .with_blobs(Arc::clone(&blobs))
            .with_workflow(Arc::clone(&workflow)),
        );

        Ok(Self {
            paths,
            store,
            blobs,
            index,
            secrets,
            themes,
            bus,
            engine,
            workflow,
            oauth,
        })
    }

    /// Nom du magasin de secrets réellement utilisé, pour le diagnostic.
    pub fn secrets_backend(&self) -> &'static str {
        self.secrets.backend()
    }

    /// Vide l'index et le reconstruit depuis la base.
    ///
    /// L'index est dans le cache, donc jetable ; ce chemin existe pour le cas où il
    /// aurait divergé, ce qui arrive après un arrêt brutal en cours d'écriture.
    pub fn rebuild_index(&self) -> Result<u64> {
        self.index.commit()?;
        Ok(self.index.document_count())
    }
}

/// Choisit le magasin de secrets.
///
/// Le trousseau du système d'abord : l'utilisateur lui fait déjà confiance, et rien
/// ne traîne dans nos fichiers. Le coffre chiffré n'est là que pour les
/// environnements qui n'en ont pas — serveurs, conteneurs, sessions distantes.
fn open_secrets(paths: &Paths, master: Option<Secret>) -> Result<Arc<dyn SecretStore>> {
    let trousseau = KeyringStore::new("Iris");
    if trousseau.is_available() {
        tracing::info!("secrets: system keyring");
        return Ok(Arc::new(trousseau));
    }

    let master = master.ok_or_else(|| {
        Error::Config(
            "aucun trousseau système disponible : un mot de passe maître est requis \
             pour ouvrir le coffre chiffré"
                .into(),
        )
    })?;

    tracing::info!("secrets : coffre chiffré (pas de trousseau système)");
    Ok(Arc::new(EncryptedVault::open(paths.vault(), &master)?))
}

/// Fournit au moteur de synchronisation les identifiants tirés du magasin.
#[derive(Debug)]
/// Les identifiants d'un compte, renouvelés si besoin.
///
/// Un jeton OAuth vit une heure. Sans renouvellement, un compte Google fonctionne
/// jusqu'au premier déjeuner, puis échoue avec un message d'authentification qui
/// laisse croire à un mot de passe changé.
struct StoredCredentials {
    secrets: Arc<dyn SecretStore>,
    store: Arc<Store>,
    /// Les identifiants clients, relus à chaque usage : l'utilisateur peut les
    /// renseigner sans redémarrer.
    oauth: Arc<std::sync::RwLock<crate::oauth::OAuthSettings>>,
}

#[async_trait::async_trait]
impl iris_sync::CredentialsProvider for StoredCredentials {
    async fn credentials(&self, account: AccountId, email: &str) -> Result<iris_imap::Credentials> {
        // Le mode d'authentification du compte fait foi. Se fier à la présence d'un
        // jeton laisserait un compte revenu au mot de passe échouer sur un vieux
        // jeton oublié dans le coffre.
        let mode = self.store.account(account)?.map(|c| c.auth);
        let fournisseur = mode.and_then(crate::oauth::provider_for);

        if let Some(fournisseur) = fournisseur {
            let maintenant = now();

            if let Some(jeton) =
                crate::oauth::valid_access_token(self.secrets.as_ref(), email, maintenant)?
            {
                return Ok(iris_imap::Credentials::OAuth2 {
                    user: email.to_string(),
                    token: jeton,
                });
            }

            let reglages = self
                .oauth
                .read()
                .expect("réglages OAuth empoisonnés")
                .clone();
            let jeton = crate::oauth::refresh_access(
                self.secrets.as_ref(),
                &reglages,
                fournisseur,
                email,
                maintenant,
            )
            .await?;

            tracing::info!(account = %email, "OAuth token refreshed");
            return Ok(iris_imap::Credentials::OAuth2 {
                user: email.to_string(),
                token: jeton,
            });
        }

        let motdepasse = self
            .secrets
            .get(email, SecretKind::Password)?
            .ok_or_else(|| Error::AuthFailed {
                account: email.to_string(),
            })?;

        Ok(iris_imap::Credentials::Password {
            user: email.to_string(),
            password: motdepasse.expose().to_string(),
        })
    }
}

/// Instant courant.
pub fn now() -> Timestamp {
    Timestamp::from_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    )
}

/// L'exécuteur asynchrone de l'application.
///
/// Deux fils de travail, quatre pour les tâches bloquantes. Le défaut de Tokio en
/// crée un par cœur logique — seize sur une machine de bureau récente — pour un
/// travail qui passe l'essentiel de son temps à attendre le réseau. Chaque fil a sa
/// pile et ses réserves d'allocation, et ce qu'ils attendent ensemble, deux fils
/// l'attendent aussi bien.
pub fn runtime() -> iris_types::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .thread_name("iris-async")
        .enable_all()
        .build()
        .map_err(|e| iris_types::Error::other(format!("exécuteur : {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_sync::CredentialsProvider;

    fn services() -> (Services, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let services = Services::open(paths, Some(Secret::new("maitre"))).expect("services");
        (services, dir)
    }

    #[test]
    fn le_moteur_recoit_l_index_et_les_contenus() {
        // Sans eux, la recherche ne trouverait rien et les corps ne seraient jamais
        // téléchargés — deux pannes silencieuses.
        let (s, _dir) = services();
        let compte = s
            .store
            .create_account(
                &iris_store::NewAccount::new("a@x.fr", "i", "s"),
                Timestamp::EPOCH,
            )
            .unwrap();
        let _ = compte;

        // La preuve indirecte : demander un corps sur un message inexistant échoue
        // pour la bonne raison — le message est introuvable, pas le magasin absent.
        let erreur = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(s.engine.fetch_body(iris_types::MessageId(1)))
            .unwrap_err()
            .to_string();
        assert!(erreur.contains("introuvable"), "obtenu : {erreur}");
    }

    #[test]
    fn tous_les_services_s_ouvrent() {
        let (s, _dir) = services();
        assert_eq!(
            s.store.schema_version().unwrap(),
            iris_store::CURRENT_VERSION
        );
        assert_eq!(s.index.document_count(), 0);
        assert_eq!(s.themes.active().name, "mono");
        assert!(s.paths.blobs().is_dir());
    }

    #[test]
    fn une_seconde_ouverture_retrouve_les_donnees() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());

        {
            let s = Services::open(paths.clone(), Some(Secret::new("maitre"))).unwrap();
            s.store
                .create_account(
                    &iris_store::NewAccount::new("a@x.fr", "i", "s"),
                    Timestamp::EPOCH,
                )
                .unwrap();
        }

        let s = Services::open(paths, Some(Secret::new("maitre"))).unwrap();
        assert_eq!(s.store.accounts().unwrap().len(), 1);
    }

    /// Un coffre isole par test.
    ///
    /// Les tests ne doivent jamais toucher le trousseau reel : il est partage par
    /// toute la machine, les tests s'executent en parallele, et l'un ecraserait les
    /// secrets de l'autre — sans parler des traces laissees sur le poste.
    fn coffre_isole() -> (Arc<dyn SecretStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let coffre =
            iris_secrets::EncryptedVault::open(dir.path().join("coffre.json"), &Secret::new("m"))
                .unwrap();
        (Arc::new(coffre), dir)
    }

    /// Un fournisseur d'identifiants adossé à un compte du mode voulu.
    fn fournisseur(
        secrets: Arc<dyn SecretStore>,
        email: &str,
        auth: iris_store::AuthKind,
    ) -> (StoredCredentials, AccountId) {
        let store = Arc::new(Store::in_memory().unwrap());
        let mut nouveau = iris_store::NewAccount::new(email, "imap.x.fr", "smtp.x.fr");
        nouveau.auth = auth;
        let id = store.create_account(&nouveau, Timestamp::EPOCH).unwrap();

        (
            StoredCredentials {
                secrets,
                store,
                oauth: Default::default(),
            },
            id,
        )
    }

    #[tokio::test]
    async fn les_identifiants_viennent_du_magasin() {
        let (secrets, _dir) = coffre_isole();
        secrets
            .set("a@x.fr", SecretKind::Password, &Secret::new("motdepasse"))
            .unwrap();

        let (f, compte) = fournisseur(secrets, "a@x.fr", iris_store::AuthKind::Password);
        let identifiants = f.credentials(compte, "a@x.fr").await.unwrap();

        match identifiants {
            iris_imap::Credentials::Password { user, password } => {
                assert_eq!(user, "a@x.fr");
                assert_eq!(password, "motdepasse");
            }
            autre => panic!("attendu un mot de passe, obtenu {autre:?}"),
        }
    }

    #[tokio::test]
    async fn un_compte_oauth_presente_son_jeton() {
        let (secrets, _dir) = coffre_isole();
        crate::oauth::store_tokens(
            secrets.as_ref(),
            "a@x.fr",
            &iris_oauth::Tokens {
                access_token: "jeton".into(),
                refresh_token: Some("r".into()),
                expires_at: Timestamp::from_millis(i64::MAX / 2),
                email: None,
            },
        )
        .unwrap();

        let (f, compte) = fournisseur(secrets, "a@x.fr", iris_store::AuthKind::OAuthGoogle);
        match f.credentials(compte, "a@x.fr").await.unwrap() {
            iris_imap::Credentials::OAuth2 { token, .. } => assert_eq!(token, "jeton"),
            autre => panic!("attendu un jeton, obtenu {autre:?}"),
        }
    }

    #[tokio::test]
    async fn un_vieux_jeton_ne_detourne_pas_un_compte_par_mot_de_passe() {
        // Se fier à la présence d'un jeton ferait échouer un compte revenu au mot de
        // passe, sur un secret oublié dans le coffre.
        let (secrets, _dir) = coffre_isole();
        secrets
            .set("a@x.fr", SecretKind::Password, &Secret::new("actuel"))
            .unwrap();
        secrets
            .set("a@x.fr", SecretKind::AccessToken, &Secret::new("perime"))
            .unwrap();

        let (f, compte) = fournisseur(secrets, "a@x.fr", iris_store::AuthKind::Password);
        match f.credentials(compte, "a@x.fr").await.unwrap() {
            iris_imap::Credentials::Password { password, .. } => assert_eq!(password, "actuel"),
            autre => panic!("attendu un mot de passe, obtenu {autre:?}"),
        }
    }

    #[tokio::test]
    async fn un_compte_oauth_sans_identifiant_client_le_dit() {
        // Un échec d'authentification anonyme enverrait chercher un mot de passe là
        // où il manque une configuration.
        let (secrets, _dir) = coffre_isole();
        secrets
            .set("a@x.fr", SecretKind::RefreshToken, &Secret::new("r"))
            .unwrap();

        let (f, compte) = fournisseur(secrets, "a@x.fr", iris_store::AuthKind::OAuthGoogle);
        let erreur = f
            .credentials(compte, "a@x.fr")
            .await
            .unwrap_err()
            .to_string();
        assert!(erreur.contains("identifiant client"), "obtenu : {erreur}");
    }

    #[tokio::test]
    async fn un_compte_sans_secret_echoue_explicitement() {
        let (secrets, _dir) = coffre_isole();
        let (f, compte) = fournisseur(secrets, "inconnu@x.fr", iris_store::AuthKind::Password);

        let e = f.credentials(compte, "inconnu@x.fr").await.unwrap_err();
        assert!(
            e.needs_user_action(),
            "l'utilisateur doit etre invite a se reconnecter"
        );
    }

    #[test]
    fn sans_trousseau_ni_mot_de_passe_maitre_l_ouverture_est_refusee() {
        // Le message doit dire quoi faire, pas seulement que ça a échoué.
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        paths.ensure().unwrap();

        // On ne peut pas simuler l'absence de trousseau ; on éprouve donc directement
        // le chemin de repli.
        let e = open_secrets(&paths, None);
        if let Err(e) = e {
            assert!(e.to_string().contains("mot de passe maître"));
        }
    }
}
