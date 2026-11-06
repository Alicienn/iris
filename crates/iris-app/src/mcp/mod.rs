//! Iris for an AI assistant: the MCP server.
//!
//! An assistant (Claude Desktop, Claude Code, any MCP client) starts `iris mcp`, which
//! is only a bridge ([`bridge`]): it relays the assistant's standard input and output
//! to the Iris that is running, over a socket on 127.0.0.1 that this module opens
//! while Settings › System › "Let an AI assistant use Iris" is on.
//!
//! Why the running Iris answers and not the bridge: the search index is held by one
//! process (Tantivy's writer lock), the database is written by the sync engine, and a
//! message is sent through the outbox the window shows. A second process would find
//! the index taken and change data behind the window's back. Inside, every write goes
//! through the same store, workflow and outbox as a click, and the screen follows.
//!
//! Only this user's programs reach the socket: it listens on the loopback only, and
//! a connection must first send the token written beside the port in
//! `cache/mcp-endpoint.json`, under the user's own profile.
//!
//! The tools are grouped by place: [`mail`], [`calendar`], [`tasks`] (tasks, habits,
//! goals) and [`notes`] (spaces, notes, spreadsheets).

pub mod bridge;
mod calendar;
mod mail;
mod notes;
mod tasks;
pub mod time;

use crate::services::Services;
use iris_mcp::{Args, Handler, Tool, ToolError, ToolResult};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// What every tool works with.
pub(crate) struct Ctx {
    pub services: Services,
    /// The application's runtime: a tool that must wait on the network (a body to
    /// download, a message to send) blocks its connection's thread on it.
    pub runtime: tokio::runtime::Handle,
    /// The outbox the window uses, when a mailbox can send.
    pub send: Option<Arc<iris_sync::SendService>>,
    pub window: slint::Weak<iris_ui::AppWindow>,
}

impl Ctx {
    /// The place on screen draws itself again: what a tool changed shows at once.
    /// Mail needs none of it (the workflow tells the list through the bus).
    pub fn refresh(&self) {
        let _ = self.window.upgrade_in_event_loop(|f| {
            f.invoke_workspace_changed(f.get_workspace());
        });
    }

    pub fn now(&self) -> iris_types::Timestamp {
        crate::services::now()
    }
}

/// The file the bridge finds the server by.
pub fn endpoint_path(paths: &crate::paths::Paths) -> std::path::PathBuf {
    paths.cache.join("mcp-endpoint.json")
}

static CONTEXTE: OnceLock<Arc<Ctx>> = OnceLock::new();
static SERVEUR: Mutex<Option<Running>> = Mutex::new(None);

/// Called once the window, the services and the outbox exist: keeps what the tools
/// need, wires the settings' button and starts the server if the setting is on.
pub fn wire_mcp(
    fenetre: &iris_ui::AppWindow,
    services: &Services,
    runtime: tokio::runtime::Handle,
    send: Option<Arc<iris_sync::SendService>>,
    enabled: bool,
) {
    let _ = CONTEXTE.set(Arc::new(Ctx {
        services: services.clone(),
        runtime,
        send,
        window: fenetre.as_weak(),
    }));
    {
        let faible = fenetre.as_weak();
        let services = services.clone();
        fenetre.on_mcp_copy_setup(move || {
            let Some(f) = faible.upgrade() else {
                return;
            };
            let message = match copy_setup(&services) {
                Ok(()) => "Setup copied. Paste it into your assistant's MCP settings \
                           (claude_desktop_config.json for Claude Desktop)."
                    .to_string(),
                Err(e) => format!("Could not copy the setup: {e}"),
            };
            f.set_toast(message.into());
        });
    }
    set_enabled(enabled);
}

/// Starts or stops the server, and says so under the toggle.
pub fn set_enabled(on: bool) {
    let Some(ctx) = CONTEXTE.get() else {
        return;
    };
    let mut serveur = SERVEUR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ancien) = serveur.take() {
        ancien.stop();
    }
    let statut = if on {
        match Running::start(Arc::clone(ctx)) {
            Ok(r) => {
                *serveur = Some(r);
                "On. An assistant connected to Iris can read and change your mail, \
                 calendar, tasks and notes while Iris runs."
                    .to_string()
            }
            Err(e) => {
                tracing::warn!(error = %e, "starting the MCP server");
                format!("Could not start: {e}")
            }
        }
    } else {
        "Off. Assistants cannot reach Iris.".to_string()
    };
    let _ = ctx.window.upgrade_in_event_loop(move |f| {
        f.set_mcp_status(statut.into());
    });
}

/// The setup an MCP client takes: this executable, `mcp`, and the data folder when
/// it is not the usual one.
fn setup_json(exe: &std::path::Path) -> Value {
    let mut serveur = json!({
        "command": exe.display().to_string(),
        "args": ["mcp"],
    });
    if let Some(racine) = std::env::var_os("IRIS_ROOT") {
        serveur["env"] = json!({ "IRIS_ROOT": racine.to_string_lossy() });
    }
    json!({ "mcpServers": { "iris": serveur } })
}

fn copy_setup(_services: &Services) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let texte = serde_json::to_string_pretty(&setup_json(&exe)).map_err(|e| e.to_string())?;
    let mut presse = crate::presse::Clipboard::new().map_err(|e| e.to_string())?;
    presse.set_text(texte).map_err(|e| e.to_string())
}

/// The server while it listens.
struct Running {
    arret: Arc<AtomicBool>,
    port: u16,
    fichier: std::path::PathBuf,
    connexions: Arc<Mutex<Vec<TcpStream>>>,
}

impl Running {
    fn start(ctx: Arc<Ctx>) -> std::io::Result<Self> {
        let ecoute = TcpListener::bind(("127.0.0.1", 0))?;
        let port = ecoute.local_addr()?.port();
        let jeton = token()?;
        let fichier = endpoint_path(&ctx.services.paths);
        write_endpoint(&fichier, port, &jeton)?;

        let arret = Arc::new(AtomicBool::new(false));
        let connexions: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let outils: Arc<IrisTools> = Arc::new(IrisTools { ctx });
        {
            let arret = Arc::clone(&arret);
            let connexions = Arc::clone(&connexions);
            std::thread::Builder::new()
                .name("iris-mcp".into())
                .spawn(move || {
                    for entrant in ecoute.incoming() {
                        if arret.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(flux) = entrant else {
                            continue;
                        };
                        if let Ok(copie) = flux.try_clone() {
                            connexions
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .push(copie);
                        }
                        let outils = Arc::clone(&outils);
                        let jeton = jeton.clone();
                        let _ = std::thread::Builder::new()
                            .name("iris-mcp-client".into())
                            .spawn(move || {
                                if let Err(e) = client(flux, &jeton, outils.as_ref()) {
                                    tracing::debug!(error = %e, "MCP connection ended");
                                }
                            });
                    }
                })?;
        }
        tracing::info!(port, "MCP server listening");
        Ok(Self {
            arret,
            port,
            fichier,
            connexions,
        })
    }

    fn stop(self) {
        self.arret.store(true, Ordering::SeqCst);
        // `incoming` waits for a connection: one wakes it to see the flag.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        for c in self
            .connexions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
        let _ = std::fs::remove_file(&self.fichier);
        tracing::info!("MCP server stopped");
    }
}

/// One assistant: its token first, then the protocol until it leaves.
fn client(flux: TcpStream, jeton: &str, outils: &IrisTools) -> std::io::Result<()> {
    flux.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let mut lecteur = BufReader::new(flux.try_clone()?);
    let mut premiere = String::new();
    lecteur.read_line(&mut premiere)?;
    if !same(premiere.trim().as_bytes(), jeton.as_bytes()) {
        return Ok(());
    }
    flux.set_read_timeout(None)?;
    let mut ecrivain = flux;
    iris_mcp::serve(lecteur, &mut ecrivain, outils)?;
    ecrivain.flush()
}

/// Compares in a time that does not tell how much matched.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn token() -> std::io::Result<String> {
    use ring::rand::SecureRandom;
    let mut octets = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut octets)
        .map_err(|_| std::io::Error::other("no randomness"))?;
    Ok(hex::encode(octets))
}

fn write_endpoint(fichier: &std::path::Path, port: u16, jeton: &str) -> std::io::Result<()> {
    if let Some(dossier) = fichier.parent() {
        std::fs::create_dir_all(dossier)?;
    }
    let texte = json!({ "port": port, "token": jeton, "pid": std::process::id() });
    let provisoire = fichier.with_extension("tmp");
    std::fs::write(&provisoire, texte.to_string())?;
    std::fs::rename(&provisoire, fichier)
}

/// Where the bridge finds the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

pub fn read_endpoint(fichier: &std::path::Path) -> Option<Endpoint> {
    let texte = std::fs::read_to_string(fichier).ok()?;
    let v: Value = serde_json::from_str(&texte).ok()?;
    Some(Endpoint {
        port: u16::try_from(v.get("port")?.as_u64()?).ok()?,
        token: v.get("token")?.as_str()?.to_string(),
    })
}

/// Every tool, behind one handler.
struct IrisTools {
    ctx: Arc<Ctx>,
}

const INSTRUCTIONS: &str = "Iris is the user's mail client, calendar, task manager \
(with habits and goals) and notes app, all on this computer. These tools act on the \
user's real data: what you change shows in Iris at once and syncs to their servers.\n\
\n\
- Start with iris_overview for today's picture and the ids you will need.\n\
- Mail: mail_search (Iris's query language and filters), mail_list for a folder, \
mail_thread to read a conversation, mail_update to mark, star, file, snooze or bin, \
mail_send to write (it waits in the outbox a few seconds, like a click on Send).\n\
- Calendar: calendar_day, calendar_week or calendar_events for a range; \
calendar_create_event, calendar_update_event (moving is changing start/end), \
calendar_delete_event, calendar_set_note; an event's subtasks are tasks with event_id.\n\
- Tasks: tasks_list, tasks_create, tasks_update, tasks_complete; habits_* and goals_*.\n\
- Notes: notes_spaces, then notes_tree, notes_read, notes_write, notes_edit. Read \
notes_syntax before writing a note: Iris has its own marks, callouts, embeds and \
charts. Spreadsheets: sheets_read, sheets_write, sheets_add_chart.\n\
\n\
Times are the user's local time, written 2026-11-06T14:30; days are 2026-11-06. \
Ask the user before sending mail or deleting anything they did not ask you to.";

impl Handler for IrisTools {
    fn tools(&self) -> Vec<Tool> {
        let mut t = vec![overview_tool()];
        t.extend(mail::tools());
        t.extend(calendar::tools());
        t.extend(tasks::tools());
        t.extend(notes::tools());
        t
    }

    fn call(&self, name: &str, args: &Args) -> ToolResult {
        let ctx = self.ctx.as_ref();
        let resultat = if name == "iris_overview" {
            overview(ctx, args)
        } else if name.starts_with("mail_") {
            mail::call(ctx, name, args)
        } else if name.starts_with("calendar_") {
            calendar::call(ctx, name, args)
        } else if name.starts_with("tasks_")
            || name.starts_with("habits_")
            || name.starts_with("goals_")
        {
            tasks::call(ctx, name, args)
        } else if name.starts_with("notes_") || name.starts_with("sheets_") {
            notes::call(ctx, name, args)
        } else {
            Err(ToolError::new(format!("unknown tool: {name}")))
        };
        if let Err(e) = &resultat {
            tracing::debug!(tool = name, error = %e, "MCP tool failed");
        }
        resultat
    }

    fn instructions(&self) -> String {
        INSTRUCTIONS.to_string()
    }
}

fn overview_tool() -> Tool {
    Tool::read(
        "iris_overview",
        "Today in Iris",
        "Today's picture: the mailboxes with their unread counts, today's events, the \
         tasks due today or late, and the habits to do today. Start here.",
        json!({"type": "object", "properties": {}}),
    )
}

fn overview(ctx: &Ctx, _args: &Args) -> ToolResult {
    let jour = time::today();
    Ok(json!({
        "now": time::format_ms(ctx.now().millis()),
        "today": jour.to_string(),
        "mail": mail::summary(ctx)?,
        "events_today": calendar::day_summary(ctx, jour)?,
        "tasks_due": tasks::due_summary(ctx, jour)?,
        "habits_today": tasks::habits_summary(ctx, jour)?,
    }))
}

/// A context over fresh, empty services, for the tools' tests: no window, no outbox.
/// Keep the three together: the folder holds the data, the runtime the handle.
#[cfg(test)]
pub(crate) fn test_ctx() -> (Ctx, tempfile::TempDir, tokio::runtime::Runtime) {
    let dossier = tempfile::tempdir().expect("temporary folder");
    let paths = crate::paths::Paths::under(dossier.path());
    let services = Services::open(paths, Some(iris_secrets::Secret::new("m"))).expect("services");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let ctx = Ctx {
        services,
        runtime: runtime.handle().clone(),
        send: None,
        window: slint::Weak::default(),
    };
    (ctx, dossier, runtime)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setup_names_this_program_and_its_command() {
        let v = setup_json(std::path::Path::new("C:\\Iris\\iris.exe"));
        assert_eq!(v["mcpServers"]["iris"]["command"], "C:\\Iris\\iris.exe");
        assert_eq!(v["mcpServers"]["iris"]["args"][0], "mcp");
    }

    #[test]
    fn the_endpoint_goes_and_comes_back() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("mcp-endpoint.json");
        write_endpoint(&f, 4242, "abc").unwrap();
        assert_eq!(
            read_endpoint(&f),
            Some(Endpoint {
                port: 4242,
                token: "abc".into()
            })
        );
    }

    #[test]
    fn tokens_compare_whole() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
        assert_eq!(token().unwrap().len(), 64);
    }
}
