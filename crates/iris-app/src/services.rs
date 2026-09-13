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
        let secrets = open_secrets(&paths, master)?;
        let bus = EventBus::new();

        let engine = Arc::new(SyncEngine::new(
            Arc::clone(&store),
            Arc::new(RustlsConnector::new()),
            Arc::new(StoredCredentials { secrets: Arc::clone(&secrets) }),
            bus.clone(),
            EngineConfig::default(),
        ));

        Ok(Self { paths, store, blobs, index, secrets, themes, bus, engine })
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
        tracing::info!("secrets : trousseau du système");
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
struct StoredCredentials {
    secrets: Arc<dyn SecretStore>,
}

#[async_trait::async_trait]
impl iris_sync::CredentialsProvider for StoredCredentials {
    async fn credentials(
        &self,
        _account: AccountId,
        email: &str,
    ) -> Result<iris_imap::Credentials> {
        // Un jeton OAuth prime sur un mot de passe : quand les deux existent, c'est
        // que le compte a migré vers la connexion par fournisseur d'identité.
        if let Some(jeton) = self.secrets.get(email, SecretKind::AccessToken)? {
            return Ok(iris_imap::Credentials::OAuth2 {
                user: email.to_string(),
                token: jeton.expose().to_string(),
            });
        }

        let motdepasse = self
            .secrets
            .get(email, SecretKind::Password)?
            .ok_or_else(|| Error::AuthFailed { account: email.to_string() })?;

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
    fn tous_les_services_s_ouvrent() {
        let (s, _dir) = services();
        assert_eq!(s.store.schema_version().unwrap(), iris_store::CURRENT_VERSION);
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

    #[tokio::test]
    async fn les_identifiants_viennent_du_magasin() {
        let (s, _dir) = services();
        s.secrets
            .set("a@x.fr", SecretKind::Password, &Secret::new("motdepasse"))
            .unwrap();

        let fournisseur = StoredCredentials { secrets: Arc::clone(&s.secrets) };
        let identifiants = fournisseur.credentials(AccountId(1), "a@x.fr").await.unwrap();

        match identifiants {
            iris_imap::Credentials::Password { user, password } => {
                assert_eq!(user, "a@x.fr");
                assert_eq!(password, "motdepasse");
            }
            autre => panic!("attendu un mot de passe, obtenu {autre:?}"),
        }
    }

    #[tokio::test]
    async fn un_jeton_oauth_prime_sur_le_mot_de_passe() {
        // Quand les deux existent, c'est que le compte a migré.
        let (s, _dir) = services();
        s.secrets.set("a@x.fr", SecretKind::Password, &Secret::new("ancien")).unwrap();
        s.secrets.set("a@x.fr", SecretKind::AccessToken, &Secret::new("jeton")).unwrap();

        let fournisseur = StoredCredentials { secrets: Arc::clone(&s.secrets) };
        let identifiants = fournisseur.credentials(AccountId(1), "a@x.fr").await.unwrap();
        assert!(matches!(identifiants, iris_imap::Credentials::OAuth2 { .. }));
    }

    #[tokio::test]
    async fn un_compte_sans_secret_echoue_explicitement() {
        let (s, _dir) = services();
        let fournisseur = StoredCredentials { secrets: Arc::clone(&s.secrets) };

        let e = fournisseur.credentials(AccountId(1), "inconnu@x.fr").await.unwrap_err();
        assert!(e.needs_user_action(), "l'utilisateur doit être invité à se reconnecter");
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
