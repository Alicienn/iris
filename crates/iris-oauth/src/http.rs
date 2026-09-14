//! Le point d'accès réel, et l'écoute de la redirection.
//!
//! Le reste du module est délibérément sans réseau, pour être testable. Ce fichier
//! est la seule partie qui parle au monde, et il est aussi mince que possible.
//!
//! La redirection passe par **127.0.0.1**, jamais par un serveur distant : le code
//! d'autorisation ne quitte donc pas la machine. C'est le mode recommandé pour les
//! applications de bureau, et le seul qui n'oblige pas à héberger quelque chose.

use crate::{AuthRequest, TokenEndpoint};
use iris_types::{Error, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// Le point d'accès HTTPS d'un fournisseur.
#[derive(Debug)]
pub struct HttpEndpoint {
    client: reqwest::Client,
}

impl HttpEndpoint {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("Iris")
            .build()
            .map_err(|e| Error::other(format!("client HTTP : {e}")))?;
        Ok(Self { client })
    }
}

#[async_trait::async_trait]
impl TokenEndpoint for HttpEndpoint {
    async fn post_form(&self, url: &str, params: &[(String, String)]) -> Result<String> {
        let reponse = self
            .client
            .post(url)
            .form(params)
            .send()
            .await
            .map_err(|e| Error::other(format!("échange de jeton : {e}")))?;

        // Le corps est lu même en cas d'erreur : les fournisseurs y mettent la
        // raison du refus, et c'est précisément ce qu'il faut montrer.
        reponse
            .text()
            .await
            .map_err(|e| Error::other(format!("réponse du fournisseur : {e}")))
    }
}

/// Ce que la redirection a rapporté.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// Chemin et paramètres, tels que reçus.
    pub url: String,
}

/// Réserve le port de la redirection.
///
/// Le port est **réservé avant** l'ouverture du navigateur, et non après : entre
/// l'ouverture et l'écoute, l'utilisateur peut avoir déjà accepté, et une redirection
/// qui arrive sur un port fermé perd l'autorisation sans rien dire.
pub fn reserve_port() -> Result<(TcpListener, u16)> {
    let ecoute = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::other(format!("écoute locale : {e}")))?;
    let port = ecoute
        .local_addr()
        .map_err(|e| Error::other(format!("port local : {e}")))?
        .port();
    Ok((ecoute, port))
}

/// Attend la redirection du navigateur.
///
/// Bloquant, et volontairement : l'appelant l'exécute sur un fil dédié. Rendre cette
/// fonction asynchrone obligerait à un exécuteur pour lire trois lignes de HTTP.
pub fn wait_for_redirect(listener: TcpListener, timeout: Duration) -> Result<Redirect> {
    listener
        .set_nonblocking(false)
        .map_err(|e| Error::other(format!("écoute locale : {e}")))?;

    let echeance = std::time::Instant::now() + timeout;

    for flux in listener.incoming() {
        let mut flux = flux.map_err(|e| Error::other(format!("connexion locale : {e}")))?;
        let _ = flux.set_read_timeout(Some(Duration::from_secs(5)));

        match lire_cible(&mut flux) {
            Some(cible) if cible.contains("code=") || cible.contains("error=") => {
                repondre(&mut flux, PAGE_SUCCES);
                return Ok(Redirect { url: format!("http://127.0.0.1{cible}") });
            }
            // Les navigateurs demandent souvent /favicon.ico en même temps ; y
            // répondre poliment évite de prendre cette requête pour la bonne.
            _ => repondre(&mut flux, PAGE_ATTENTE),
        }

        if std::time::Instant::now() >= echeance {
            break;
        }
    }

    Err(Error::other("autorisation non reçue"))
}

/// Lit la cible de la requête HTTP, sans lire le corps.
fn lire_cible(flux: &mut TcpStream) -> Option<String> {
    let mut lecteur = BufReader::new(flux);
    let mut ligne = String::new();
    lecteur.read_line(&mut ligne).ok()?;

    // « GET /oauth?code=... HTTP/1.1 »
    let mut morceaux = ligne.split_whitespace();
    let methode = morceaux.next()?;
    let cible = morceaux.next()?;
    (methode == "GET").then(|| cible.to_string())
}

fn repondre(flux: &mut TcpStream, corps: &str) {
    let reponse = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{corps}",
        corps.len()
    );
    let _ = flux.write_all(reponse.as_bytes());
    let _ = flux.flush();
}

/// La page que voit l'utilisateur au retour.
///
/// Sobre et sans ressource externe : elle s'affiche hors ligne, et ne demande rien à
/// personne. Elle dit surtout quoi faire ensuite, ce qu'une page blanche ne fait pas.
const PAGE_SUCCES: &str = "<!doctype html><meta charset=\"utf-8\">\
<title>Iris</title>\
<style>body{font-family:system-ui,sans-serif;background:#111;color:#eee;\
display:flex;align-items:center;justify-content:center;height:100vh;margin:0}\
div{text-align:center}p{color:#999}</style>\
<div><h1>Compte autorisé</h1><p>Vous pouvez fermer cet onglet et revenir à Iris.</p></div>";

const PAGE_ATTENTE: &str = "<!doctype html><meta charset=\"utf-8\"><title>Iris</title>";

/// Ouvre l'URL d'autorisation dans le navigateur de l'utilisateur.
///
/// Le navigateur du système, jamais une fenêtre intégrée : l'utilisateur doit voir
/// la barre d'adresse de son fournisseur pour savoir à qui il donne son mot de
/// passe. Une page de connexion affichée dans notre fenêtre est indiscernable d'une
/// contrefaçon.
pub fn open_browser(request: &AuthRequest) -> Result<()> {
    webbrowser::open(&request.url)
        .map_err(|e| Error::other(format!("ouverture du navigateur : {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn le_port_est_reserve_avant_tout() {
        // Entre l'ouverture du navigateur et l'écoute, l'utilisateur peut avoir déjà
        // accepté : le port doit être pris d'avance.
        let (ecoute, port) = reserve_port().unwrap();
        assert!(port > 0);
        assert!(TcpStream::connect(("127.0.0.1", port)).is_ok(), "le port écoute déjà");
        drop(ecoute);
    }

    #[test]
    fn deux_reservations_ne_se_marchent_pas_dessus() {
        let (_a, port_a) = reserve_port().unwrap();
        let (_b, port_b) = reserve_port().unwrap();
        assert_ne!(port_a, port_b);
    }

    #[test]
    fn la_redirection_est_captee_avec_son_code() {
        let (ecoute, port) = reserve_port().unwrap();

        let client = std::thread::spawn(move || {
            let mut flux = TcpStream::connect(("127.0.0.1", port)).unwrap();
            flux.write_all(b"GET /oauth?code=abc123&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut reponse = String::new();
            let _ = flux.read_to_string(&mut reponse);
            reponse
        });

        let redirection = wait_for_redirect(ecoute, Duration::from_secs(5)).unwrap();
        assert!(redirection.url.contains("code=abc123"));
        assert!(redirection.url.contains("state=xyz"));

        let vue = client.join().unwrap();
        assert!(vue.contains("Compte autorisé"), "l'utilisateur doit savoir que c'est fini");
    }

    #[test]
    fn un_refus_du_fournisseur_est_capte_aussi() {
        // Sinon l'application attendrait indéfiniment une autorisation refusée.
        let (ecoute, port) = reserve_port().unwrap();

        std::thread::spawn(move || {
            let mut flux = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = flux.write_all(b"GET /oauth?error=access_denied HTTP/1.1\r\n\r\n");
            let mut poubelle = Vec::new();
            let _ = flux.read_to_end(&mut poubelle);
        });

        let redirection = wait_for_redirect(ecoute, Duration::from_secs(5)).unwrap();
        assert!(redirection.url.contains("error=access_denied"));
    }

    #[test]
    fn une_requete_parasite_ne_termine_pas_l_attente() {
        // Les navigateurs demandent /favicon.ico : la prendre pour la redirection
        // ferait échouer une autorisation parfaitement valide.
        let (ecoute, port) = reserve_port().unwrap();

        std::thread::spawn(move || {
            let mut parasite = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = parasite.write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n");
            let mut poubelle = Vec::new();
            let _ = parasite.read_to_end(&mut poubelle);

            let mut vraie = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = vraie.write_all(b"GET /oauth?code=bon HTTP/1.1\r\n\r\n");
            let _ = vraie.read_to_end(&mut Vec::new());
        });

        let redirection = wait_for_redirect(ecoute, Duration::from_secs(5)).unwrap();
        assert!(redirection.url.contains("code=bon"));
    }

    #[test]
    fn une_requete_qui_n_est_pas_un_get_est_ignoree() {
        let (ecoute, port) = reserve_port().unwrap();

        std::thread::spawn(move || {
            let mut flux = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = flux.write_all(b"POST /oauth?code=x HTTP/1.1\r\n\r\n");
            let _ = flux.read_to_end(&mut Vec::new());

            let mut vraie = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = vraie.write_all(b"GET /oauth?code=bon HTTP/1.1\r\n\r\n");
            let _ = vraie.read_to_end(&mut Vec::new());
        });

        let redirection = wait_for_redirect(ecoute, Duration::from_secs(5)).unwrap();
        assert!(redirection.url.contains("code=bon"));
    }
}
