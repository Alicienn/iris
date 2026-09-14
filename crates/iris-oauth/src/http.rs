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
            .map_err(|e| Error::other(format!("HTTP client: {e}")))?;
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
            .map_err(|e| Error::other(format!("token exchange: {e}")))?;

        // Le corps est lu même en cas d'erreur : les fournisseurs y mettent la
        // raison du refus, et c'est précisément ce qu'il faut montrer.
        reponse
            .text()
            .await
            .map_err(|e| Error::other(format!("provider response: {e}")))
    }
}

/// Ce que la redirection a rapporté.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// Chemin et paramètres, tels que reçus.
    pub url: String,
}

/// Checks that this machine can talk to itself over TCP.
///
/// The whole redirect flow rests on it. When loopback is blocked — a firewall policy,
/// a filtering driver, a locked-down corporate image — the browser opens, the user
/// authorises, and the answer lands nowhere. Failing here instead means the user is
/// told before they hand over their password, not after.
pub fn loopback_works() -> bool {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        return false;
    };
    let Ok(address) = listener.local_addr() else {
        return false;
    };

    // A short timeout on purpose: the default connect timeout on a blocked loopback is
    // twenty seconds, which is far too long to spend on a preflight check.
    TcpStream::connect_timeout(&address, PREFLIGHT_TIMEOUT).is_ok()
}

/// How long the preflight check waits before declaring loopback unusable.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_millis(500);

/// Reserves the port the redirect will come back to.
///
/// The port is taken **before** the browser opens, not after: between opening and
/// listening the user may already have approved, and a redirect arriving on a closed
/// port loses the authorisation without a word.
pub fn reserve_port() -> Result<(TcpListener, u16)> {
    let ecoute = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::other(format!("local listener: {e}")))?;
    let port = ecoute
        .local_addr()
        .map_err(|e| Error::other(format!("local port: {e}")))?
        .port();
    Ok((ecoute, port))
}

/// Waits for the browser to come back.
///
/// Blocking, deliberately: the caller runs it on a dedicated thread. Making it
/// asynchronous would mean an executor to read three lines of HTTP.
///
/// The deadline is real. A blocking `accept()` would honour the timeout only *after*
/// a connection arrives — so a user who closes the browser instead of authorising
/// would leave the thread waiting for ever, holding a port. Polling costs one wake-up
/// every hundred milliseconds and makes the timeout mean what it says.
pub fn wait_for_redirect(listener: TcpListener, timeout: Duration) -> Result<Redirect> {
    listener
        .set_nonblocking(true)
        .map_err(|e| Error::other(format!("local listener: {e}")))?;

    let deadline = std::time::Instant::now() + timeout;

    while std::time::Instant::now() < deadline {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            Err(e) => return Err(Error::other(format!("local connection: {e}"))),
        };

        // The accepted socket inherits non-blocking mode; reading a request line is
        // simpler when it does not.
        stream
            .set_nonblocking(false)
            .map_err(|e| Error::other(format!("local connection: {e}")))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

        match request_target(&mut stream) {
            Some(target) if target.contains("code=") || target.contains("error=") => {
                respond(&mut stream, SUCCESS_PAGE);
                return Ok(Redirect {
                    url: format!("http://127.0.0.1{target}"),
                });
            }
            // Browsers often ask for /favicon.ico at the same time; answering it
            // politely avoids mistaking that request for the real one.
            _ => respond(&mut stream, WAITING_PAGE),
        }
    }

    Err(Error::other("authorisation was not received in time"))
}

/// How often the listener is polled while waiting.
///
/// Short enough that the redirect feels instant, long enough that a five-minute wait
/// costs three thousand cheap syscalls rather than a busy loop.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Reads the target of the HTTP request, without reading a body.
fn request_target(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;

    // "GET /oauth?code=... HTTP/1.1"
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    (method == "GET").then(|| target.to_string())
}

fn respond(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// The page the user sees when they come back.
///
/// Plain, and with no external resource: it renders offline and asks nothing of
/// anyone. Above all it says what to do next, which a blank page does not.
const SUCCESS_PAGE: &str = "<!doctype html><meta charset=\"utf-8\">\
<title>Iris</title>\
<style>body{font-family:system-ui,sans-serif;background:#111;color:#eee;\
display:flex;align-items:center;justify-content:center;height:100vh;margin:0}\
div{text-align:center}p{color:#999}</style>\
<div><h1>Compte autorisé</h1><p>Vous pouvez fermer cet onglet et revenir à Iris.</p></div>";

const WAITING_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>Iris</title>";

/// Ouvre l'URL d'autorisation dans le navigateur de l'utilisateur.
///
/// Le navigateur du système, jamais une fenêtre intégrée : l'utilisateur doit voir
/// la barre d'adresse de son fournisseur pour savoir à qui il donne son mot de
/// passe. Une page de connexion affichée dans notre fenêtre est indiscernable d'une
/// contrefaçon.
pub fn open_browser(request: &AuthRequest) -> Result<()> {
    webbrowser::open(&request.url).map_err(|e| Error::other(format!("opening the browser: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Whether this machine can talk to itself, checked once.
    ///
    /// Every test below drives a real socket, which is the point: the code under test
    /// is a tiny HTTP server. On a machine where loopback is blocked they cannot run
    /// at all, and a red suite would then say something false about the code. They
    /// announce the skip rather than passing quietly, because a test that silently
    /// does nothing is worse than one that fails.
    fn loopback_or_skip(test: &str) -> bool {
        use std::sync::OnceLock;
        static AVAILABLE: OnceLock<bool> = OnceLock::new();

        if *AVAILABLE.get_or_init(loopback_works) {
            return true;
        }
        eprintln!("SKIPPED {test}: this machine cannot open a loopback connection");
        false
    }

    #[test]
    fn le_port_est_reserve_avant_tout() {
        if !loopback_or_skip("le_port_est_reserve_avant_tout") {
            return;
        }
        // Entre l'ouverture du navigateur et l'écoute, l'utilisateur peut avoir déjà
        // accepté : le port doit être pris d'avance.
        let (ecoute, port) = reserve_port().unwrap();
        assert!(port > 0);
        assert!(
            TcpStream::connect(("127.0.0.1", port)).is_ok(),
            "le port écoute déjà"
        );
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
        if !loopback_or_skip("la_redirection_est_captee_avec_son_code") {
            return;
        }
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
        assert!(
            vue.contains("Compte autorisé"),
            "l'utilisateur doit savoir que c'est fini"
        );
    }

    #[test]
    fn un_refus_du_fournisseur_est_capte_aussi() {
        if !loopback_or_skip("un_refus_du_fournisseur_est_capte_aussi") {
            return;
        }
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
        if !loopback_or_skip("une_requete_parasite_ne_termine_pas_l_attente") {
            return;
        }
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
    fn the_deadline_is_honoured_when_nobody_comes_back() {
        // A blocking accept would wait for ever here, holding a thread and a port.
        // That is exactly how a hung process appeared during development.
        let (listener, _port) = reserve_port().unwrap();

        let start = std::time::Instant::now();
        let result = wait_for_redirect(listener, Duration::from_millis(300));
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "no authorisation should have been reported"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "the wait must end near its deadline, took {elapsed:?}"
        );
    }

    #[test]
    fn une_requete_qui_n_est_pas_un_get_est_ignoree() {
        if !loopback_or_skip("une_requete_qui_n_est_pas_un_get_est_ignoree") {
            return;
        }
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
