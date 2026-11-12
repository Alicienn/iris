//! The Model Context Protocol, as Iris speaks it to an AI client.
//!
//! An AI client (Claude Desktop, Claude Code…) starts `iris mcp` and talks to it on
//! its standard input and output: one JSON-RPC 2.0 message per line. This crate knows
//! that conversation and nothing of Iris: the tools come from a [`Handler`], which the
//! application implements over its own services.
//!
//! Only what a tools server needs is spoken: `initialize`, `ping`, `tools/list` and
//! `tools/call`. Notifications (`notifications/initialized`, cancellations) are read
//! and answered with nothing, as the protocol wants.

#![deny(unsafe_code)]

pub mod args;

pub use args::Args;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

/// The protocol revisions understood, newest first. A client asking for one of them
/// gets it; any other gets the newest.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// One tool offered to the AI.
#[derive(Debug, Clone)]
pub struct Tool {
    /// `mail_search`, `calendar_create_event`…: what the AI calls.
    pub name: &'static str,
    /// A short human title, shown by clients that ask before a tool runs.
    pub title: &'static str,
    /// What it does, for the AI: when to use it and what it returns.
    pub description: &'static str,
    /// A JSON Schema object for its arguments.
    pub input_schema: Value,
    /// Reads only: a client may run it without asking.
    pub read_only: bool,
    /// May remove or overwrite something the user has.
    pub destructive: bool,
}

impl Tool {
    /// A tool that only reads.
    pub fn read(
        name: &'static str,
        title: &'static str,
        description: &'static str,
        input_schema: Value,
    ) -> Self {
        Self {
            name,
            title,
            description,
            input_schema,
            read_only: true,
            destructive: false,
        }
    }

    /// A tool that adds or changes, without removing anything.
    pub fn write(
        name: &'static str,
        title: &'static str,
        description: &'static str,
        input_schema: Value,
    ) -> Self {
        Self {
            name,
            title,
            description,
            input_schema,
            read_only: false,
            destructive: false,
        }
    }

    /// A tool that may remove, move away or overwrite.
    pub fn destructive(
        name: &'static str,
        title: &'static str,
        description: &'static str,
        input_schema: Value,
    ) -> Self {
        Self {
            name,
            title,
            description,
            input_schema,
            read_only: false,
            destructive: true,
        }
    }

    /// How `tools/list` presents it.
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "title": self.title,
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "openWorldHint": false,
            },
        })
    }
}

/// What went wrong in a tool, told to the AI so it can correct itself (a missing
/// argument, an unknown id) rather than as a protocol failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<E: std::error::Error> From<E> for ToolError {
    fn from(e: E) -> Self {
        Self(e.to_string())
    }
}

pub type ToolResult = Result<Value, ToolError>;

/// The tools behind a server.
pub trait Handler: Send + Sync {
    /// Every tool offered.
    fn tools(&self) -> Vec<Tool>;
    /// Runs one. `args` is the object the AI sent (`{}` when it sent none).
    fn call(&self, name: &str, args: &Args) -> ToolResult;
    /// Told to the AI once, when it connects: what Iris is and how to use the tools.
    fn instructions(&self) -> String;
    /// The server's name and version, for `initialize`.
    fn server_info(&self) -> (&'static str, &'static str) {
        ("iris", env!("CARGO_PKG_VERSION"))
    }
}

/// JSON-RPC error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Serves one client until its input ends: reads a message per line, writes each
/// answer as one line and flushes it.
///
/// A tool call runs on its own thread: one that waits on the network (a body to
/// download, a message to send) must not hold up `ping`, `tools/list` or the next
/// call, or the assistant sees timeouts on questions that were quick. Everything
/// else is answered in order, as it is read.
pub fn serve(
    reader: impl BufRead,
    writer: impl Write + Send,
    handler: &dyn Handler,
) -> io::Result<()> {
    let writer = std::sync::Mutex::new(writer);
    let repondre = |ligne: &str| -> io::Result<()> {
        if let Some(reponse) = handle_line(ligne, handler) {
            let mut texte = reponse.to_string();
            texte.push('\n');
            let mut w = writer.lock().unwrap_or_else(|e| e.into_inner());
            w.write_all(texte.as_bytes())?;
            w.flush()?;
        }
        Ok(())
    };
    std::thread::scope(|s| {
        for ligne in reader.lines() {
            let ligne = ligne?;
            if ligne.trim().is_empty() {
                continue;
            }
            if ligne.contains("\"tools/call\"") {
                let repondre = &repondre;
                s.spawn(move || {
                    let _ = repondre(&ligne);
                });
            } else {
                repondre(&ligne)?;
            }
        }
        Ok(())
    })
}

/// The answer to one line, or nothing for a notification.
pub fn handle_line(line: &str, handler: &dyn Handler) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(m) => m,
        Err(e) => {
            return Some(error(
                Value::Null,
                PARSE_ERROR,
                &format!("parse error: {e}"),
            ))
        }
    };
    match message {
        // A batch (2025-03-26): each answered, the notifications left out.
        Value::Array(lot) => {
            let reponses: Vec<Value> = lot
                .into_iter()
                .filter_map(|m| handle_message(m, handler))
                .collect();
            (!reponses.is_empty()).then_some(Value::Array(reponses))
        }
        m => handle_message(m, handler),
    }
}

fn handle_message(message: Value, handler: &dyn Handler) -> Option<Value> {
    let Some(objet) = message.as_object() else {
        return Some(error(
            Value::Null,
            INVALID_REQUEST,
            "not a JSON-RPC message",
        ));
    };
    let id = objet.get("id").cloned();
    let Some(methode) = objet.get("method").and_then(Value::as_str) else {
        // A response from the client (we never ask it anything): nothing to say.
        return None;
    };
    let params = objet.get("params").cloned().unwrap_or(Value::Null);

    // A notification wants no answer, whatever it is.
    let id = id?;

    let resultat = match methode {
        "initialize" => Ok(initialize(&params, handler)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": handler.tools().iter().map(Tool::to_json).collect::<Vec<_>>(),
        })),
        "tools/call" => call(&params, handler),
        // Asked by some clients whatever the capabilities say: empty, not an error.
        "resources/list" => Ok(json!({ "resources": [] })),
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        autre => Err((METHOD_NOT_FOUND, format!("unknown method: {autre}"))),
    };
    Some(match resultat {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, m)) => error(id, code, &m),
    })
}

fn initialize(params: &Value, handler: &dyn Handler) -> Value {
    let demandee = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == demandee)
        .copied()
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    let (nom, version_serveur) = handler.server_info();
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": nom, "title": "Iris", "version": version_serveur },
        "instructions": handler.instructions(),
    })
}

fn call(params: &Value, handler: &dyn Handler) -> Result<Value, (i64, String)> {
    let Some(nom) = params.get("name").and_then(Value::as_str) else {
        return Err((INVALID_PARAMS, "tools/call needs a name".into()));
    };
    if !handler.tools().iter().any(|t| t.name == nom) {
        return Err((INVALID_PARAMS, format!("unknown tool: {nom}")));
    }
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => Value::Object(Default::default()),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err((INVALID_PARAMS, "arguments must be an object".into())),
    };
    let args = Args::new(arguments);
    Ok(match handler.call(nom, &args) {
        Ok(valeur) => tool_result(valeur, false),
        Err(e) => tool_result(Value::String(e.0), true),
    })
}

/// A tool's answer: its value as text (what every client reads) and, when it is an
/// object, as structured content too.
pub fn tool_result(value: Value, is_error: bool) -> Value {
    let texte = match &value {
        Value::String(s) => s.clone(),
        v => serde_json::to_string_pretty(v).unwrap_or_default(),
    };
    let mut r = json!({
        "content": [{ "type": "text", "text": texte }],
        "isError": is_error,
    });
    if value.is_object() && !is_error {
        r["structuredContent"] = value;
    }
    r
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Handler for Echo {
        fn tools(&self) -> Vec<Tool> {
            vec![Tool::read(
                "echo",
                "Echo",
                "Says back its text.",
                json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
            )]
        }
        fn call(&self, name: &str, args: &Args) -> ToolResult {
            assert_eq!(name, "echo");
            Ok(json!({ "text": args.str("text")? }))
        }
        fn instructions(&self) -> String {
            "An echo.".into()
        }
    }

    fn ask(line: &str) -> Value {
        handle_line(line, &Echo).expect("an answer")
    }

    #[test]
    fn initialize_agrees_on_a_version_and_says_what_it_offers() {
        let r = ask(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        );
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert_eq!(r["result"]["instructions"], "An echo.");

        let r = ask(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn a_notification_gets_no_answer() {
        assert!(handle_line(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &Echo
        )
        .is_none());
    }

    #[test]
    fn tools_are_listed_with_their_schema_and_hints() {
        let r = ask(r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#);
        let outil = &r["result"]["tools"][0];
        assert_eq!(outil["name"], "echo");
        assert_eq!(outil["inputSchema"]["required"][0], "text");
        assert_eq!(outil["annotations"]["readOnlyHint"], true);
        assert_eq!(r["id"], "a");
    }

    #[test]
    fn a_call_answers_text_and_structure() {
        let r = ask(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}"#,
        );
        assert_eq!(r["result"]["isError"], false);
        assert_eq!(r["result"]["structuredContent"]["text"], "hi");
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("hi"));
    }

    #[test]
    fn a_tool_error_is_told_to_the_ai_not_the_protocol() {
        let r = ask(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{}}}"#,
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("text"));
    }

    #[test]
    fn unknown_tools_methods_and_garbage_are_errors() {
        let r = ask(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}"#);
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = ask(r#"{"jsonrpc":"2.0","id":5,"method":"sampling/whatever"}"#);
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r = ask("{not json");
        assert_eq!(r["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn serve_answers_line_by_line() {
        let entree = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n"
        );
        let mut sortie = Vec::new();
        serve(entree.as_bytes(), &mut sortie, &Echo).unwrap();
        let lignes: Vec<&str> = std::str::from_utf8(&sortie).unwrap().lines().collect();
        assert_eq!(lignes.len(), 2);
        assert!(lignes[0].contains(r#""id":1"#));
    }
}
