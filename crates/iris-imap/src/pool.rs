//! Le pool de connexions.
//!
//! C'est le vrai mur de scalabilité du projet. Cent comptes en attente permanente,
//! ce sont cent connexions TCP, cent fils d'exécution côté serveur, et un
//! bannissement quasi certain : la plupart des serveurs plafonnent à trois ou dix
//! connexions simultanées par compte, Gmail à quinze.
//!
//! Le pool impose donc **deux plafonds** : un global, qui borne la consommation de la
//! machine, et un par serveur, qui respecte les quotas du fournisseur. Les deux sont
//! nécessaires : le global seul se ferait bannir sur un serveur, le second seul
//! laisserait cent serveurs ouvrir cent connexions.

use iris_types::{Error, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Réglages du pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    /// Connexions vivantes, tous comptes confondus.
    pub max_total: usize,
    /// Connexions simultanées vers un même serveur.
    pub max_per_server: usize,
    /// Délai d'attente d'une place avant abandon.
    pub acquire_timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            // Seize connexions couvrent confortablement les comptes actifs ; les
            // autres sont interrogés par sondage périodique.
            max_total: 16,
            // Trois est le plancher observé chez les hébergeurs mutualisés. Rester
            // en dessous du plafond réel évite d'être rejeté quand un autre client
            // de l'utilisateur est connecté en même temps.
            max_per_server: 3,
            acquire_timeout: Duration::from_secs(30),
        }
    }
}

/// Statistiques d'exploitation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub in_use: usize,
    pub granted: u64,
    /// Nombre de fois où une demande a dû attendre une place.
    pub waited: u64,
    /// Nombre de fois où l'attente a expiré.
    pub timeouts: u64,
}

/// Le pool.
#[derive(Debug)]
pub struct ConnectionPool {
    config: PoolConfig,
    global: Arc<Semaphore>,
    per_server: Mutex<HashMap<String, Arc<Semaphore>>>,
    granted: AtomicU64,
    waited: AtomicU64,
    timeouts: AtomicU64,
}

impl ConnectionPool {
    pub fn new(config: PoolConfig) -> Self {
        Self {
            global: Arc::new(Semaphore::new(config.max_total)),
            config,
            per_server: Mutex::new(HashMap::new()),
            granted: AtomicU64::new(0),
            waited: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
        }
    }

    /// Réserve une place pour ce serveur.
    ///
    /// Les deux plafonds sont pris **dans le même ordre partout** — global puis
    /// serveur. Un ordre variable entre appelants produirait un interblocage dès que
    /// les deux plafonds sont atteints simultanément.
    pub async fn acquire(&self, server: &str) -> Result<Lease> {
        let serveur = server.trim().to_lowercase();
        let par_serveur = self.semaphore_for(&serveur);

        // On note l'attente avant de la subir, pour que la statistique reflète la
        // contention même si l'attente échoue.
        if self.global.available_permits() == 0 || par_serveur.available_permits() == 0 {
            self.waited.fetch_add(1, Ordering::Relaxed);
        }

        let global = self.acquire_one(Arc::clone(&self.global)).await?;
        let local = self.acquire_one(par_serveur).await?;

        self.granted.fetch_add(1, Ordering::Relaxed);
        Ok(Lease {
            _global: global,
            _local: local,
            server: serveur,
        })
    }

    async fn acquire_one(&self, sem: Arc<Semaphore>) -> Result<OwnedSemaphorePermit> {
        match tokio::time::timeout(self.config.acquire_timeout, sem.acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(Error::network("pool de connexions fermé")),
            Err(_) => {
                self.timeouts.fetch_add(1, Ordering::Relaxed);
                Err(Error::Throttled {
                    retry_after_secs: 5,
                })
            }
        }
    }

    /// Tente de réserver sans attendre.
    ///
    /// Utilisé par l'ordonnanceur pour les comptes de faible priorité : mieux vaut
    /// les reporter au prochain cycle que faire patienter un compte actif.
    pub fn try_acquire(&self, server: &str) -> Option<Lease> {
        let serveur = server.trim().to_lowercase();
        let par_serveur = self.semaphore_for(&serveur);

        let global = Arc::clone(&self.global).try_acquire_owned().ok()?;
        let local = par_serveur.try_acquire_owned().ok()?;

        self.granted.fetch_add(1, Ordering::Relaxed);
        Some(Lease {
            _global: global,
            _local: local,
            server: serveur,
        })
    }

    fn semaphore_for(&self, server: &str) -> Arc<Semaphore> {
        let mut map = self.per_server.lock().expect("pool empoisonné");
        Arc::clone(
            map.entry(server.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(self.config.max_per_server))),
        )
    }

    pub fn stats(&self) -> PoolStats {
        PoolStats {
            in_use: self.config.max_total - self.global.available_permits(),
            granted: self.granted.load(Ordering::Relaxed),
            waited: self.waited.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
        }
    }

    pub fn config(&self) -> PoolConfig {
        self.config
    }

    /// Places encore libres pour ce serveur.
    pub fn available_for(&self, server: &str) -> usize {
        let serveur = server.trim().to_lowercase();
        let local = self.semaphore_for(&serveur).available_permits();
        local.min(self.global.available_permits())
    }
}

/// Une place réservée. La libération est automatique à la destruction.
#[derive(Debug)]
pub struct Lease {
    _global: OwnedSemaphorePermit,
    _local: OwnedSemaphorePermit,
    server: String,
}

impl Lease {
    pub fn server(&self) -> &str {
        &self.server
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(total: usize, per_server: usize) -> ConnectionPool {
        ConnectionPool::new(PoolConfig {
            max_total: total,
            max_per_server: per_server,
            acquire_timeout: Duration::from_millis(50),
        })
    }

    #[tokio::test]
    async fn une_place_est_accordee_puis_liberee() {
        let p = pool(4, 2);
        {
            let bail = p.acquire("imap.example.com").await.unwrap();
            assert_eq!(bail.server(), "imap.example.com");
            assert_eq!(p.stats().in_use, 1);
        }
        assert_eq!(
            p.stats().in_use,
            0,
            "la place doit être rendue à la destruction"
        );
    }

    #[tokio::test]
    async fn le_plafond_par_serveur_est_respecte() {
        let p = pool(10, 2);
        let _a = p.acquire("imap.example.com").await.unwrap();
        let _b = p.acquire("imap.example.com").await.unwrap();

        // Le troisième dépasse le quota du serveur, alors que le global est libre.
        assert!(p.acquire("imap.example.com").await.is_err());
        // Un autre serveur reste servi.
        assert!(p.acquire("imap.autre.fr").await.is_ok());
    }

    #[tokio::test]
    async fn le_plafond_global_est_respecte() {
        let p = pool(2, 5);
        let _a = p.acquire("un.fr").await.unwrap();
        let _b = p.acquire("deux.fr").await.unwrap();

        // Le quota par serveur est libre, mais la machine est à sa limite.
        assert!(p.acquire("trois.fr").await.is_err());
    }

    #[tokio::test]
    async fn le_nom_de_serveur_est_normalise() {
        // Sans normalisation, « IMAP.Example.com » et « imap.example.com »
        // consommeraient deux quotas distincts sur le même serveur.
        let p = pool(10, 1);
        let _a = p.acquire("IMAP.Example.COM").await.unwrap();
        assert!(p.acquire("  imap.example.com ").await.is_err());
    }

    #[tokio::test]
    async fn l_expiration_est_signalee_comme_temporaire() {
        let p = pool(1, 1);
        let _a = p.acquire("x.fr").await.unwrap();

        let e = p.acquire("y.fr").await.unwrap_err();
        assert!(e.is_transient(), "l'ordonnanceur doit pouvoir réessayer");
        assert_eq!(e.retry_after_secs(), Some(5));
        assert_eq!(p.stats().timeouts, 1);
    }

    #[tokio::test]
    async fn la_tentative_sans_attente_echoue_immediatement() {
        let p = pool(1, 1);
        let _a = p.acquire("x.fr").await.unwrap();

        let debut = std::time::Instant::now();
        assert!(p.try_acquire("y.fr").is_none());
        assert!(
            debut.elapsed() < Duration::from_millis(10),
            "aucune attente"
        );
    }

    #[tokio::test]
    async fn une_place_liberee_debloque_une_attente() {
        let p = Arc::new(pool(1, 1));
        let bail = p.acquire("x.fr").await.unwrap();

        let p2 = Arc::clone(&p);
        let attente = tokio::spawn(async move { p2.acquire("x.fr").await.map(|_| ()) });

        tokio::time::sleep(Duration::from_millis(5)).await;
        drop(bail);

        assert!(attente.await.unwrap().is_ok());
        assert!(p.stats().waited > 0, "la contention doit être visible");
    }

    #[tokio::test]
    async fn les_places_disponibles_tiennent_compte_des_deux_plafonds() {
        let p = pool(2, 5);
        assert_eq!(
            p.available_for("x.fr"),
            2,
            "le global est le plus contraignant"
        );

        let q = pool(10, 1);
        assert_eq!(
            q.available_for("x.fr"),
            1,
            "le quota serveur est le plus contraignant"
        );
    }

    #[tokio::test]
    async fn les_statistiques_comptent_les_octrois() {
        let p = pool(4, 4);
        for _ in 0..3 {
            let _bail = p.acquire("x.fr").await.unwrap();
        }
        assert_eq!(p.stats().granted, 3);
        assert_eq!(p.stats().in_use, 0);
    }

    #[tokio::test]
    async fn cent_comptes_ne_produisent_jamais_plus_que_le_plafond() {
        // La propriété centrale du pool, éprouvée sous concurrence réelle.
        let p = Arc::new(ConnectionPool::new(PoolConfig {
            max_total: 8,
            max_per_server: 8,
            acquire_timeout: Duration::from_secs(5),
        }));
        let maximum = Arc::new(AtomicU64::new(0));

        let mut taches = Vec::new();
        for i in 0..100 {
            let p = Arc::clone(&p);
            let maximum = Arc::clone(&maximum);
            taches.push(tokio::spawn(async move {
                let _bail = p.acquire(&format!("serveur{}.fr", i % 4)).await.unwrap();
                let courant = p.stats().in_use as u64;
                maximum.fetch_max(courant, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }));
        }
        for t in taches {
            t.await.unwrap();
        }

        assert!(
            maximum.load(Ordering::Relaxed) <= 8,
            "le plafond global a été dépassé : {} connexions simultanées",
            maximum.load(Ordering::Relaxed)
        );
        assert_eq!(p.stats().granted, 100);
        assert_eq!(p.stats().in_use, 0);
    }
}
