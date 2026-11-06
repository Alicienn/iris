//! `iris mcp`: what an assistant starts. It relays its standard input and output to
//! the running Iris.
//!
//! Iris not running: the bridge starts it in the notification area and waits for its
//! server. The setting off: nothing is started, and the assistant is told, in its
//! own protocol, where to turn it on — a server that only failed would leave the
//! user reading a client's log to find out why.
//!
//! Nothing but the protocol may reach the standard output: the bridge logs nothing
//! there, and writes its own trouble to the standard error (which clients keep).

use super::{endpoint_path, read_endpoint, Endpoint};
use crate::paths::Paths;
use iris_mcp::{Args, Handler, Tool, ToolError, ToolResult};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// How long a starting Iris has to open its server.
const START_WAIT: Duration = Duration::from_secs(45);

pub fn run(paths: &Paths) -> iris_types::Result<()> {
    let reglages = crate::settings::Settings::load(paths.settings());
    if !reglages.mcp_enabled {
        return unavailable(
            "Iris's assistant access is off. Ask the user to turn on Settings › System › \
             \"Let an AI assistant use Iris\" in Iris, then reconnect.",
        );
    }

    let fichier = endpoint_path(paths);
    let flux = match connect(&fichier) {
        Some(f) => Some(f),
        None => {
            start_iris();
            let debut = Instant::now();
            loop {
                if let Some(f) = connect(&fichier) {
                    break Some(f);
                }
                if debut.elapsed() > START_WAIT {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    };
    let Some(flux) = flux else {
        return unavailable(
            "Iris did not answer. Ask the user to open Iris (and check Settings › System › \
             \"Let an AI assistant use Iris\"), then reconnect.",
        );
    };
    relay(flux).map_err(iris_types::Error::from)
}

/// The running server, its token sent.
fn connect(fichier: &std::path::Path) -> Option<TcpStream> {
    let Endpoint { port, token } = read_endpoint(fichier)?;
    let mut flux =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2)).ok()?;
    flux.write_all(format!("{token}\n").as_bytes()).ok()?;
    let _ = flux.set_nodelay(true);
    Some(flux)
}

/// Iris in the notification area, on its own: closing the assistant must not take it
/// along, and it must not hold the assistant's pipes.
fn start_iris() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut commande = std::process::Command::new(exe);
    commande
        .arg("--tray")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        commande.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    if let Err(e) = commande.spawn() {
        eprintln!("iris mcp: could not start Iris: {e}");
    }
}

/// Both ways until either side stops.
fn relay(flux: TcpStream) -> std::io::Result<()> {
    let mut vers_iris = flux.try_clone()?;
    let mut depuis_iris = flux;

    // Iris → the assistant, each piece flushed at once: a client waits on its line.
    let retour = std::thread::spawn(move || {
        let mut sortie = std::io::stdout().lock();
        let mut tampon = [0u8; 16 * 1024];
        loop {
            match depuis_iris.read(&mut tampon) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if sortie.write_all(&tampon[..n]).is_err() || sortie.flush().is_err() {
                        break;
                    }
                }
            }
        }
    });

    // The assistant → Iris.
    let mut entree = std::io::stdin().lock();
    let mut tampon = [0u8; 16 * 1024];
    loop {
        match entree.read(&mut tampon) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if vers_iris.write_all(&tampon[..n]).is_err() {
                    break;
                }
            }
        }
    }
    // The assistant is gone: Iris closes its side, which ends the other thread.
    let _ = vers_iris.shutdown(std::net::Shutdown::Write);
    let _ = retour.join();
    Ok(())
}

/// Speaks MCP itself, with one tool that says why Iris cannot be reached.
fn unavailable(raison: &str) -> iris_types::Result<()> {
    eprintln!("iris mcp: {raison}");
    let entree = std::io::stdin().lock();
    let sortie = std::io::stdout().lock();
    iris_mcp::serve(
        entree,
        sortie,
        &Unavailable {
            raison: raison.to_string(),
        },
    )
    .map_err(iris_types::Error::from)
}

struct Unavailable {
    raison: String,
}

impl Handler for Unavailable {
    fn tools(&self) -> Vec<Tool> {
        vec![Tool::read(
            "iris_status",
            "Why Iris cannot be reached",
            "Iris's tools are not available right now. Call this to learn why and what \
             the user should do.",
            json!({"type": "object", "properties": {}}),
        )]
    }

    fn call(&self, _name: &str, _args: &Args) -> ToolResult {
        Err(ToolError::new(self.raison.clone()))
    }

    fn instructions(&self) -> String {
        self.raison.clone()
    }
}
