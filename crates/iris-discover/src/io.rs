//! Les entrées-sorties de la découverte, et leurs doublures de test.
//!
//! Trois opérations suffisent : récupérer un document, interroger le DNS, et vérifier
//! qu'un port répond. Les isoler derrière un trait rend toute la chaîne testable sans
//! réseau — c'est indispensable, car une découverte a par nature des dizaines de
//! chemins d'échec qu'on ne peut pas reproduire contre de vrais serveurs.

use crate::DiscoveryIo;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Un enregistrement `SRV`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvRecord {
    pub target: String,
    pub port: u16,
    pub priority: u16,
    pub weight: u16,
}

impl SrvRecord {
    pub fn new(target: impl Into<String>, port: u16, priority: u16, weight: u16) -> Self {
        // Les cibles DNS se terminent par un point ; il n'a pas sa place dans une
        // configuration présentée à l'utilisateur.
        let target = target.into().trim_end_matches('.').to_string();
        Self { target, port, priority, weight }
    }
}

/// Récupération d'un document par HTTP.
#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn get(&self, url: &str) -> Option<String>;
}

/// Interrogation du DNS.
#[async_trait]
pub trait Resolver: Send + Sync {
    async fn srv(&self, name: &str) -> Vec<SrvRecord>;
    async fn mx(&self, domain: &str) -> Vec<String>;
}

/// Vérification qu'un port accepte les connexions.
#[async_trait]
pub trait Prober: Send + Sync {
    async fn probe(&self, host: &str, port: u16) -> bool;
}

// --- Implémentations réelles ---

/// Client HTTP de la découverte.
///
/// Les délais sont courts et le nombre de redirections borné : une découverte qui
/// prend dix secondes par étape est inutilisable quand on ajoute cent boîtes.
#[derive(Debug)]
pub struct HttpFetcher {
    client: reqwest::Client,
}

impl HttpFetcher {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent("Iris/0.1 (autoconfiguration)")
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Fetcher for HttpFetcher {
    async fn get(&self, url: &str) -> Option<String> {
        let reponse = self.client.get(url).send().await.ok()?;
        if !reponse.status().is_success() {
            return None;
        }
        // Beaucoup d'hébergeurs servent une page d'erreur HTML en 200 : un document
        // qui ne commence pas comme du XML n'a pas à être analysé.
        let corps = reponse.text().await.ok()?;
        let debut = corps.trim_start();
        (debut.starts_with("<?xml") || debut.starts_with("<clientConfig")).then_some(corps)
    }
}

/// Résolveur DNS.
#[derive(Debug)]
pub struct DnsResolver {
    inner: hickory_resolver::TokioResolver,
}

impl DnsResolver {
    pub fn from_system() -> iris_types::Result<Self> {
        let inner = hickory_resolver::Resolver::builder_tokio()
            .map_err(|e| iris_types::Error::network(format!("résolveur DNS : {e}")))?
            .build();
        Ok(Self { inner })
    }
}

#[async_trait]
impl Resolver for DnsResolver {
    async fn srv(&self, name: &str) -> Vec<SrvRecord> {
        match self.inner.srv_lookup(name).await {
            Ok(reponse) => reponse
                .iter()
                .map(|r| {
                    SrvRecord::new(r.target().to_utf8(), r.port(), r.priority(), r.weight())
                })
                .collect(),
            // Une absence d'enregistrement est le cas normal, pas une erreur.
            Err(_) => Vec::new(),
        }
    }

    async fn mx(&self, domain: &str) -> Vec<String> {
        match self.inner.mx_lookup(domain).await {
            Ok(reponse) => {
                let mut records: Vec<_> = reponse.iter().collect();
                records.sort_by_key(|r| r.preference());
                records
                    .iter()
                    .map(|r| r.exchange().to_utf8().trim_end_matches('.').to_string())
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }
}

/// Sonde TCP.
#[derive(Debug, Default)]
pub struct TcpProber;

#[async_trait]
impl Prober for TcpProber {
    async fn probe(&self, host: &str, port: u16) -> bool {
        // Deux secondes : au-delà, le port est considéré comme fermé. Sonder huit
        // combinaisons à dix secondes chacune ferait attendre une minute et demie.
        let adresse = format!("{host}:{port}");
        matches!(
            tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(adresse))
                .await,
            Ok(Ok(_))
        )
    }
}

/// Assemble les trois implémentations réelles.
#[derive(Debug)]
pub struct RealIo {
    pub fetcher: HttpFetcher,
    pub resolver: Option<DnsResolver>,
    pub prober: TcpProber,
}

impl RealIo {
    pub fn new() -> Self {
        // Un résolveur indisponible ne doit pas empêcher la découverte : les étapes
        // DNS sont simplement sautées.
        let resolver = DnsResolver::from_system()
            .inspect_err(|e| tracing::warn!(erreur = %e, "DNS indisponible"))
            .ok();
        Self { fetcher: HttpFetcher::new(), resolver, prober: TcpProber }
    }
}

impl Default for RealIo {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DiscoveryIo for RealIo {
    async fn fetch(&self, url: &str) -> Option<String> {
        self.fetcher.get(url).await
    }

    async fn srv(&self, name: &str) -> Vec<SrvRecord> {
        match &self.resolver {
            Some(r) => r.srv(name).await,
            None => Vec::new(),
        }
    }

    async fn mx(&self, domain: &str) -> Vec<String> {
        match &self.resolver {
            Some(r) => r.mx(domain).await,
            None => Vec::new(),
        }
    }

    async fn probe(&self, host: &str, port: u16) -> bool {
        self.prober.probe(host, port).await
    }
}

// --- Doublure de test ---

/// Entrées-sorties simulées, pour éprouver la chaîne sans réseau.
#[derive(Debug, Clone, Default)]
pub struct MockIo {
    fetches: Arc<Mutex<HashMap<String, String>>>,
    srvs: Arc<Mutex<HashMap<String, Vec<SrvRecord>>>>,
    mxs: Arc<Mutex<HashMap<String, Vec<String>>>>,
    ports: Arc<Mutex<Vec<(String, u16)>>>,
    fetch_count: Arc<AtomicUsize>,
    probe_count: Arc<AtomicUsize>,
}

impl MockIo {
    pub fn add_fetch(&mut self, url: &str, body: &str) {
        self.fetches.lock().unwrap().insert(url.to_string(), body.to_string());
    }

    pub fn add_srv(&mut self, name: &str, record: SrvRecord) {
        self.srvs.lock().unwrap().entry(name.to_string()).or_default().push(record);
    }

    pub fn add_mx(&mut self, domain: &str, exchange: &str) {
        self.mxs.lock().unwrap().entry(domain.to_string()).or_default().push(exchange.to_string());
    }

    pub fn add_probe(&mut self, host: &str, port: u16) {
        self.ports.lock().unwrap().push((host.to_string(), port));
    }

    /// Nombre de requêtes HTTP émises, pour vérifier qu'une étape a bien été évitée.
    pub fn fetches(&self) -> usize {
        self.fetch_count.load(Ordering::Relaxed)
    }

    pub fn probes(&self) -> usize {
        self.probe_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl DiscoveryIo for MockIo {
    async fn fetch(&self, url: &str) -> Option<String> {
        self.fetch_count.fetch_add(1, Ordering::Relaxed);
        self.fetches.lock().unwrap().get(url).cloned()
    }

    async fn srv(&self, name: &str) -> Vec<SrvRecord> {
        self.srvs.lock().unwrap().get(name).cloned().unwrap_or_default()
    }

    async fn mx(&self, domain: &str) -> Vec<String> {
        self.mxs.lock().unwrap().get(domain).cloned().unwrap_or_default()
    }

    async fn probe(&self, host: &str, port: u16) -> bool {
        self.probe_count.fetch_add(1, Ordering::Relaxed);
        self.ports.lock().unwrap().contains(&(host.to_string(), port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_point_final_des_cibles_dns_est_retire() {
        let r = SrvRecord::new("imap.example.com.", 993, 10, 5);
        assert_eq!(r.target, "imap.example.com");
    }

    #[tokio::test]
    async fn la_doublure_compte_les_appels() {
        let mut io = MockIo::default();
        io.add_fetch("https://x", "corps");

        assert_eq!(io.fetch("https://x").await.as_deref(), Some("corps"));
        assert!(io.fetch("https://y").await.is_none());
        assert_eq!(io.fetches(), 2);
    }

    #[tokio::test]
    async fn la_doublure_ne_repond_qu_aux_ports_declares() {
        let mut io = MockIo::default();
        io.add_probe("imap.x.fr", 993);

        assert!(io.probe("imap.x.fr", 993).await);
        assert!(!io.probe("imap.x.fr", 143).await);
        assert!(!io.probe("autre.x.fr", 993).await);
        assert_eq!(io.probes(), 3);
    }
}
