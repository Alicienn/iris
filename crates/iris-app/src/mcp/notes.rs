//! Notes and spreadsheets for an assistant: the spaces, their files, and the `.sheet`
//! workbooks in them.
//!
//! Everything goes through `iris-vault` as the Notes place does: the same root and
//! spaces (`crate::notes::racine`, the `notes_spaces` setting), the same atomic writes,
//! a past version kept before a note is overwritten, the links rewritten when a note
//! moves, and the Recycle Bin for what is deleted. The place watches its space and
//! shows what changed here within a couple of seconds.

use super::{time, Ctx};
use iris_board::geom::{Pt, Rect};
use iris_board::{Board, Colour, End, Item, Kind as BoardKind, Shape as BoardShape, SIDES};
use iris_mcp::{Args, Tool, ToolError, ToolResult};
use iris_notes::{block, links, meta};
use iris_sheets::chart::{self, Chart, Kind, Series};
use iris_sheets::{Addr, Locale, Range, Sheet, Workbook};
use iris_vault::{EntryKind, RecycleBin, Space, Vault, WriteOutcome};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The most entries `notes_tree` gives back.
const MAX_ENTREES: usize = 2000;
/// The most cells `sheets_read` gives back.
const MAX_CELLULES: usize = 5000;
/// The most cells `sheets_write` takes in one call.
const MAX_ECRITES: usize = 20_000;

/// Where the tools find the notes: the vault, the folders elsewhere the user added,
/// and where deleted files go.
struct Notes {
    vault: Vault,
    externals: Vec<PathBuf>,
    bin: &'static dyn RecycleBin,
}

impl Notes {
    /// As the Notes place sees them.
    fn of(ctx: &Ctx) -> Result<Self, ToolError> {
        let vault = Vault::open(crate::notes::racine(&ctx.services))?;
        let externals = crate::settings::current()
            .notes_spaces
            .iter()
            .map(PathBuf::from)
            .collect();
        Ok(Self {
            vault,
            externals,
            bin: &crate::notes::bin::SystemBin,
        })
    }
}

// --- The tools ------------------------------------------------------------------------

fn space_arg() -> Value {
    json!({
        "type": "string",
        "description": "The space: its name (as notes_spaces lists it), its index, or its folder. \
                        Optional: the first space when left out."
    })
}

fn path_arg(what: &str) -> Value {
    json!({ "type": "string", "description": what })
}

pub(super) fn tools() -> Vec<Tool> {
    vec![
        Tool::read(
            "notes_spaces",
            "Note spaces",
            "The note spaces (folders of notes): name, index, folder on disk, how many notes. \
             Every other notes_ and sheets_ tool takes a space by its name.",
            json!({"type": "object", "properties": {}}),
        ),
        Tool::write(
            "notes_create_space",
            "Create a note space",
            "Creates a new, empty space in the notes folder, as Notes' \"New space\" does.",
            json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
        ),
        Tool::read(
            "notes_tree",
            "Files of a space",
            "The folders and files of a space, or of one of its folders: path (relative, with \
             /), kind (folder, note, sheet, image, other), size, last change. recursive lists \
             everything below.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "folder": path_arg("A folder of the space, e.g. \"Work/2026\". The top when left out."),
                    "recursive": { "type": "boolean", "default": false }
                }
            }),
        ),
        Tool::read(
            "notes_read",
            "Read a note",
            "A note's text, with its last change, tags, properties, headings, the notes it \
             links to and how many link to it. The path may be given without .md, or as a \
             link names it (the note's name alone). Spreadsheets: sheets_read.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The note, e.g. \"Work/Meeting.md\" or \"Meeting\".")
                },
                "required": ["path"]
            }),
        ),
        Tool::write(
            "notes_write",
            "Write a note",
            "Writes a note. mode: \"replace\" (default: creates it or replaces its text), \
             \"create\" (fails if it exists), \"append\" (at the end), \"prepend\" (at the top, \
             after its properties). .md is added unless the path ends in .md, .markdown or \
             .txt; missing folders are made. The text it had is kept in the note's history \
             (notes_versions). Read notes_syntax first: Iris has marks, callouts and embeds \
             of its own.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The note, e.g. \"Work/Meeting\"."),
                    "text": { "type": "string", "description": "Markdown, lines ended by \\n." },
                    "mode": { "type": "string", "enum": ["replace", "create", "append", "prepend"] }
                },
                "required": ["path", "text"]
            }),
        ),
        Tool::write(
            "notes_edit",
            "Edit a note",
            "Replaces exact text in a note: find must appear once (or set all to replace every \
             occurrence). Fails when it is not found, or found several times without all. \
             The previous text is kept in the note's history.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The note."),
                    "find": { "type": "string" },
                    "replace": { "type": "string" },
                    "all": { "type": "boolean", "default": false }
                },
                "required": ["path", "find", "replace"]
            }),
        ),
        Tool::write(
            "notes_create_folder",
            "Create a folder",
            "Creates a folder in a space (and the folders above it). Nothing happens when it \
             exists.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The folder, e.g. \"Work/2026\".")
                },
                "required": ["path"]
            }),
        ),
        Tool::destructive(
            "notes_move",
            "Move or rename",
            "Renames a note, spreadsheet or folder (name: the new name, its extension kept) \
             and/or moves it into another folder (folder: \"\" for the top). Every link to the \
             notes moved is rewritten in the space. A name already taken gets a number \
             (\"Name 2\"): the new path is given back.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("What to move."),
                    "name": { "type": "string", "description": "The new name, without extension." },
                    "folder": { "type": "string", "description": "The folder to move it into." }
                },
                "required": ["path"]
            }),
        ),
        Tool::destructive(
            "notes_delete",
            "Delete a note or folder",
            "Sends a note, file or folder (with everything in it) to the Recycle Bin, where \
             the user can bring it back. Ask the user first unless they asked for it.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("What to delete.")
                },
                "required": ["path"]
            }),
        ),
        Tool::read(
            "notes_search",
            "Search notes",
            "The notes holding every word of the query (case and accents aside), with the \
             first line that holds one. tag:x (or #x) keeps notes tagged x, path:x those whose \
             path holds x, is:task those with a box left to tick. Every space when space is \
             left out.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "default": 20, "maximum": 200 }
                },
                "required": ["query"]
            }),
        ),
        Tool::read(
            "notes_backlinks",
            "Notes linking here",
            "The notes that link to a note, with the line of each link.",
            json!({
                "type": "object",
                "properties": { "space": space_arg(), "path": path_arg("The note.") },
                "required": ["path"]
            }),
        ),
        Tool::read(
            "notes_tags",
            "Tags",
            "Every tag used in a space (or in every space), with how many notes have it, the \
             most used first.",
            json!({"type": "object", "properties": { "space": space_arg() }}),
        ),
        Tool::read(
            "notes_versions",
            "Note history",
            "The past versions kept of a note, newest first (version: its id, saved: when that \
             text was last written). With version, that version's text.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The note."),
                    "version": { "type": "integer" }
                },
                "required": ["path"]
            }),
        ),
        Tool::read(
            "notes_templates",
            "Note templates",
            "The templates of a space (notes in its templates folder) with their text. \
             notes_create_from_template makes a note from one.",
            json!({"type": "object", "properties": { "space": space_arg() }}),
        ),
        Tool::write(
            "notes_create_from_template",
            "New note from a template",
            "Makes a new note from a template, its {{title}} (the note's name), {{date}}, \
             {{time}} and {{course}} filled in. Fails if the note exists.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "template": { "type": "string", "description": "Its name, e.g. \"Réunion\"." },
                    "path": path_arg("The new note, e.g. \"Work/Weekly 6 Nov\"."),
                    "course": { "type": "string", "description": "What {{course}} becomes." }
                },
                "required": ["template", "path"]
            }),
        ),
        Tool::read(
            "notes_syntax",
            "Note syntax",
            "The complete syntax of Iris notes (blocks, marks, colours, callouts, links, \
             embeds of pictures, spreadsheets and charts, flashcards, templates) and of \
             spreadsheet formulas. Read it before writing a note.",
            json!({"type": "object", "properties": {}}),
        ),
        Tool::read(
            "sheets_read",
            "Read a spreadsheet",
            "A .sheet file: its sheets, the cells of one (input as typed, value computed now, \
             shown as formatted), and its charts with the line that shows each in a note.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The spreadsheet, e.g. \"Work/Budget.sheet\" (or its name)."),
                    "sheet": { "type": "string", "description": "A sheet's name; the first when left out." },
                    "range": { "type": "string", "description": "Only these cells, e.g. \"A1:D20\"." }
                },
                "required": ["path"]
            }),
        ),
        Tool::write(
            "sheets_write",
            "Write cells",
            "Writes cells of a spreadsheet, made when missing (.sheet added to the path). \
             cells: {\"A1\": \"Label\", \"B2\": 12, \"B3\": \"=SUM(B1:B2)\"}, null clears a \
             cell; or rows: a 2D array written from start (default A1), null leaving a cell \
             as it is. A formula starts with =. sheet: a sheet's name, made when missing. \
             Gives back the computed value of every cell written.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The spreadsheet, e.g. \"Work/Budget\"."),
                    "sheet": { "type": "string" },
                    "cells": { "type": "object", "additionalProperties": true },
                    "rows": { "type": "array", "items": { "type": "array" } },
                    "start": { "type": "string", "default": "A1" }
                },
                "required": ["path"]
            }),
        ),
        Tool::write(
            "sheets_add_chart",
            "Add a chart",
            "Adds a chart to a spreadsheet. kind: flow (bands from a whole to its parts, \
             through several category columns), bars, columns, donut, line. Give range (the \
             table, its first row the headers: the kind and columns are guessed from it) \
             and/or header_row, categories (columns grouping the rows; several for a flow) \
             and values (columns of numbers added up; several for columns and line; none: \
             the rows are counted). Gives the line that shows it in a note.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The spreadsheet."),
                    "name": { "type": "string", "description": "The chart's name: not a cell (A1), no ] | [ #." },
                    "kind": { "type": "string", "enum": ["flow", "bars", "columns", "donut", "line"] },
                    "sheet": { "type": "string" },
                    "range": { "type": "string", "description": "e.g. \"A1:C30\"." },
                    "header_row": { "type": "integer", "description": "The headers' row, from 1." },
                    "categories": { "type": "array", "items": { "type": "string" }, "description": "Column letters, e.g. [\"A\"]." },
                    "values": { "type": "array", "items": { "type": "string" }, "description": "Column letters, e.g. [\"B\", \"C\"]." },
                    "replace": { "type": "boolean", "default": false, "description": "Replace a chart of that name." }
                },
                "required": ["path", "name"]
            }),
        ),
        Tool::read(
            "boards_read",
            "Read a whiteboard",
            "A .board file (a whiteboard): every item in drawing order (the last on top) with \
             its id, kind (shape, sticky, text, frame, connector, ink), box (x, y, w, h in \
             pixels at 100 %), text and style; a connector's from/to are {item, x, y} (x, y \
             fractions of that item's box) or a free {x, y}.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The whiteboard, e.g. \"Work/Plan.board\" (or its name).")
                },
                "required": ["path"]
            }),
        ),
        Tool::write(
            "boards_add",
            "Add to a whiteboard",
            "Adds items to a whiteboard, made when missing (.board added to the path). Each item: \
             kind (shape, sticky, text, frame, arrow, pen); x, y (top left) and optionally w, h; \
             text; shape (rect, rounded, ellipse, diamond, triangle, parallelogram, hexagon, \
             star); fill, stroke, color (yellow, orange, red, pink, purple, blue, teal, green, \
             grey, each also -soft; paper, ink, ink-soft, line, none, or #rrggbb); font (sans, \
             display, serif, mono, hand), size, bold, italic, align (left, center, right; on \
             an arrow, where its words stand along it), valign (top, middle, bottom); an \
             optional ref naming it in this call. An arrow: from and to, each an item's id, a \
             ref, or {x, y}; it ties to the sides facing each other; way (straight, elbow, \
             curved), start and end heads (none, arrow, dot, diamond). A pen stroke: points \
             [[x, y], ...]. Gives back the new ids by ref.",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The whiteboard, e.g. \"Work/Plan\"."),
                    "items": { "type": "array", "items": { "type": "object", "additionalProperties": true } }
                },
                "required": ["path", "items"]
            }),
        ),
        Tool::destructive(
            "boards_update",
            "Change a whiteboard's items",
            "Changes items of a whiteboard by id (any field boards_add takes, x and y moving \
             it; an arrow's from/to tie it again) and deletes others (delete: [ids]; an \
             item's arrows go with it).",
            json!({
                "type": "object",
                "properties": {
                    "space": space_arg(),
                    "path": path_arg("The whiteboard."),
                    "update": { "type": "array", "items": { "type": "object", "additionalProperties": true } },
                    "delete": { "type": "array", "items": { "type": "integer" } }
                },
                "required": ["path"]
            }),
        ),
    ]
}

pub(super) fn call(ctx: &Ctx, name: &str, a: &Args) -> ToolResult {
    if name == "notes_syntax" {
        return Ok(Value::String(syntax()));
    }
    let notes = Notes::of(ctx)?;
    run(ctx, &notes, name, a)
}

/// One tool over these notes; the screen told after a change.
fn run(ctx: &Ctx, n: &Notes, name: &str, a: &Args) -> ToolResult {
    let r = match name {
        "notes_spaces" => list_spaces(n),
        "notes_create_space" => create_space(n, a),
        "notes_tree" => tree(n, a),
        "notes_read" => read(n, a),
        "notes_write" => write(n, a),
        "notes_edit" => edit(n, a),
        "notes_create_folder" => create_folder(n, a),
        "notes_move" => move_to(n, a),
        "notes_delete" => delete(n, a),
        "notes_search" => search(n, a),
        "notes_backlinks" => backlinks(n, a),
        "notes_tags" => tags(n, a),
        "notes_versions" => versions(n, a),
        "notes_templates" => templates(n, a),
        "notes_create_from_template" => from_template(n, a),
        "notes_syntax" => Ok(Value::String(syntax())),
        "sheets_read" => sheets_read(n, a),
        "sheets_write" => sheets_write(n, a),
        "sheets_add_chart" => add_chart(n, a),
        "boards_read" => boards_read(n, a),
        "boards_add" => boards_add(n, a),
        "boards_update" => boards_update(n, a),
        autre => return Err(ToolError::new(format!("unknown tool: {autre}"))),
    };
    let ecrit = matches!(
        name,
        "notes_create_space"
            | "notes_write"
            | "notes_edit"
            | "notes_create_folder"
            | "notes_move"
            | "notes_delete"
            | "notes_create_from_template"
            | "sheets_write"
            | "sheets_add_chart"
            | "boards_add"
            | "boards_update"
    );
    if ecrit && r.is_ok() {
        ctx.refresh();
    }
    r
}

// --- Spaces and paths -----------------------------------------------------------------

fn all_spaces(n: &Notes) -> Result<Vec<Space>, ToolError> {
    Ok(n.vault.spaces(&n.externals)?)
}

/// The space an argument names, the first when it names none.
fn pick(n: &Notes, a: &Args) -> Result<Space, ToolError> {
    let espaces = all_spaces(n)?;
    match a.opt_str("space")? {
        None => espaces
            .into_iter()
            .next()
            .ok_or_else(|| ToolError::new("there is no note space")),
        Some(voulu) => find_space(espaces, &voulu),
    }
}

fn find_space(mut espaces: Vec<Space>, voulu: &str) -> Result<Space, ToolError> {
    let v = voulu.trim();
    let k = espaces
        .iter()
        .position(|s| s.config.name.eq_ignore_ascii_case(v))
        .or_else(|| v.parse::<usize>().ok().filter(|i| *i < espaces.len()))
        .or_else(|| espaces.iter().position(|s| s.dir() == Path::new(v)));
    match k {
        Some(i) => Ok(espaces.swap_remove(i)),
        None => {
            let noms: Vec<String> = espaces.iter().map(|s| s.config.name.clone()).collect();
            Err(ToolError::new(format!(
                "no space \"{v}\"; the spaces are: {}",
                noms.join(", ")
            )))
        }
    }
}

/// A path as the AI wrote it, made a path of the space (`/` between parts). Names
/// Windows refuses, `..` and hidden names (`.iris`) are refused.
fn clean(raw: &str) -> Result<String, ToolError> {
    let r = raw.trim().replace('\\', "/");
    let mut parties = Vec::new();
    for p in r.split('/') {
        if p.is_empty() || p == "." {
            continue;
        }
        let refuse = p.starts_with('.')
            || p.ends_with(['.', ' '])
            || p.chars()
                .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || c.is_control());
        if refuse {
            return Err(ToolError::new(format!(
                "\"{p}\" cannot be a name here (no leading dot, no < > : \" | ? *)"
            )));
        }
        parties.push(p);
    }
    Ok(parties.join("/"))
}

/// A path that names something.
fn clean_file(raw: &str) -> Result<String, ToolError> {
    let r = clean(raw)?;
    if r.is_empty() {
        return Err(ToolError::new("give a path inside the space"));
    }
    Ok(r)
}

/// A note's path to write: `.md` added unless it is already a note's extension.
fn note_path(raw: &str) -> Result<String, ToolError> {
    let r = clean_file(raw)?;
    match EntryKind::of(&r) {
        EntryKind::Note => Ok(r),
        EntryKind::Sheet => Err(ToolError::new(
            "this is a spreadsheet: write it with sheets_write",
        )),
        EntryKind::Board => Err(ToolError::new(
            "this is a whiteboard: change it with boards_add and boards_update",
        )),
        _ => Ok(format!("{r}.md")),
    }
}

/// Something that is in the space: the path as given, with `.md`, or the note a link
/// of that name would open.
fn existing(s: &Space, raw: &str) -> Result<String, ToolError> {
    let rel = clean_file(raw)?;
    if s.path(&rel)?.exists() {
        return Ok(rel);
    }
    let md = format!("{rel}.md");
    if s.path(&md)?.is_file() {
        return Ok(md);
    }
    if let Some(r) = links::resolve(&rel, &s.notes()) {
        return Ok(r);
    }
    Err(ToolError::new(format!(
        "nothing at \"{rel}\" in the space \"{}\" (notes_tree lists it)",
        s.config.name
    )))
}

/// A note that is in the space.
fn existing_note(s: &Space, raw: &str) -> Result<String, ToolError> {
    let rel = existing(s, raw)?;
    let fichier = s.path(&rel)?.is_file();
    match EntryKind::of(&rel) {
        EntryKind::Note if fichier => Ok(rel),
        EntryKind::Sheet => Err(ToolError::new(format!(
            "\"{rel}\" is a spreadsheet: read it with sheets_read"
        ))),
        EntryKind::Board => Err(ToolError::new(format!(
            "\"{rel}\" is a whiteboard: read it with boards_read"
        ))),
        _ => Err(ToolError::new(format!("\"{rel}\" is not a note"))),
    }
}

fn kind_name(k: EntryKind) -> &'static str {
    match k {
        EntryKind::Folder => "folder",
        EntryKind::Note => "note",
        EntryKind::Sheet => "sheet",
        EntryKind::Board => "board",
        EntryKind::Image => "image",
        EntryKind::Other => "other",
    }
}

/// The folder a path is in ("" for the top).
fn parent_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(p, _)| p)
}

// --- Spaces ---------------------------------------------------------------------------

fn list_spaces(n: &Notes) -> ToolResult {
    let espaces = all_spaces(n)?;
    let liste: Vec<Value> = espaces
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "index": i,
                "name": s.config.name,
                "folder": s.dir().display().to_string(),
                "notes": s.notes().len(),
                "external": !s.dir().starts_with(n.vault.root()),
            })
        })
        .collect();
    Ok(json!({
        "root": n.vault.root().display().to_string(),
        "spaces": liste,
    }))
}

fn create_space(n: &Notes, a: &Args) -> ToolResult {
    let s = n.vault.create_space(&a.str("name")?)?;
    Ok(json!({
        "name": s.config.name,
        "folder": s.dir().display().to_string(),
    }))
}

// --- Files ----------------------------------------------------------------------------

fn tree(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let dossier = clean(&a.opt_str("folder")?.unwrap_or_default())?;
    let recursif = a.bool_or("recursive", false)?;
    if !dossier.is_empty() && !s.path(&dossier)?.is_dir() {
        return Err(ToolError::new(format!(
            "no folder \"{dossier}\" in this space"
        )));
    }
    let prefixe = if dossier.is_empty() {
        String::new()
    } else {
        format!("{dossier}/")
    };
    let profondeur = if dossier.is_empty() {
        0
    } else {
        dossier.split('/').count()
    };
    // The tree is walked from the top: the folders above are opened to reach it.
    let mut ouverts: HashSet<String> = HashSet::new();
    let mut chemin = String::new();
    for p in dossier.split('/').filter(|p| !p.is_empty()) {
        if !chemin.is_empty() {
            chemin.push('/');
        }
        chemin.push_str(p);
        ouverts.insert(chemin.clone());
    }
    if recursif {
        ouverts.extend(s.folders().into_iter().filter(|f| f.starts_with(&prefixe)));
    }
    let mut entrees = Vec::new();
    let mut tronque = false;
    for e in s.tree(&ouverts)? {
        if !e.rel.starts_with(&prefixe) || (!recursif && e.depth != profondeur) {
            continue;
        }
        if entrees.len() >= MAX_ENTREES {
            tronque = true;
            break;
        }
        let mut v = json!({
            "path": e.rel,
            "kind": kind_name(e.kind),
            "modified": time::format_ms(e.modified),
        });
        if e.kind == EntryKind::Folder {
            v["has_children"] = json!(e.has_children);
        } else if let Ok(m) = std::fs::metadata(s.path(&e.rel)?) {
            v["size"] = json!(m.len());
        }
        entrees.push(v);
    }
    Ok(json!({
        "space": s.config.name,
        "folder": dossier,
        "entries": entrees,
        "truncated": tronque,
    }))
}

fn read(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = existing_note(&s, &a.str("path")?)?;
    let (texte, modifie) = s.read(&rel)?;
    let notes = s.notes();
    let mut proprietes = Map::new();
    for (k, v) in meta::properties(&texte) {
        proprietes.insert(k, Value::String(v));
    }
    let sortants: Vec<Value> = links::links(&texte)
        .into_iter()
        .map(|l| {
            let vers = if l.target.trim().is_empty() {
                Some(rel.clone())
            } else if l.target.contains(':') {
                None
            } else {
                links::resolve(&l.target, &notes)
            };
            json!({
                "target": l.target,
                "heading": l.heading,
                "label": l.label,
                "embed": l.embed,
                "resolves_to": vers,
            })
        })
        .collect();
    let mut venant: Vec<String> = s.backlinks(&rel).into_iter().map(|(p, _)| p).collect();
    venant.dedup();
    let titres: Vec<Value> = links::outline(&texte)
        .into_iter()
        .map(|(niveau, t, _)| json!({ "level": niveau, "text": t }))
        .collect();
    Ok(json!({
        "space": s.config.name,
        "path": rel,
        "modified": time::format_ms(modifie),
        "text": texte,
        "tags": meta::tags(&texte),
        "properties": proprietes,
        "headings": titres,
        "links": sortants,
        "backlinks": venant.len(),
        "words": meta::word_count(&texte),
        "flashcards": meta::flashcards(&texte).len(),
    }))
}

/// `text` made to end its lines as the note does.
fn with_eol(text: &str, eol: &str) -> String {
    if eol == "\r\n" {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

fn appended(old: &str, text: &str) -> String {
    let eol = iris_notes::edit::line_ending_of(old);
    let mut t = old.to_string();
    if !t.is_empty() && !t.ends_with('\n') {
        t.push_str(eol);
    }
    t.push_str(&with_eol(text, eol));
    t
}

/// `text` at the top of `old`, after its properties.
fn prepended(old: &str, text: &str) -> String {
    let eol = iris_notes::edit::line_ending_of(old);
    let tete = block::parse(old)
        .first()
        .filter(|b| b.kind == block::BlockKind::Properties)
        .map_or(0, |b| b.text.len());
    let (avant, apres) = old.split_at(tete);
    let mut t = avant.to_string();
    if !t.is_empty() && !t.ends_with('\n') {
        t.push_str(eol);
    }
    t.push_str(&with_eol(text, eol));
    if !t.ends_with('\n') && !apres.is_empty() {
        t.push_str(eol);
    }
    t.push_str(apres);
    t
}

/// Writes a note as the editor does: the text it had kept as a past version first,
/// then the file written whole, a change made elsewhere meanwhile kept beside it.
fn save_note(s: &Space, rel: &str, text: &str, before: Option<&(String, i64)>) -> ToolResult {
    let mut version = false;
    if let Some((ancien, quand)) = before {
        if ancien == text {
            return Ok(json!({ "path": rel, "unchanged": true }));
        }
        if !ancien.trim().is_empty() {
            version = s.save_version(rel, ancien, *quand).is_ok();
        }
    }
    let mut v = json!({ "path": rel, "created": before.is_none(), "version_kept": version });
    match s.write(rel, text, before.map(|b| b.1))? {
        WriteOutcome::Written(m) => v["modified"] = json!(time::format_ms(m)),
        WriteOutcome::Conflict { saved_as, modified } => {
            v["modified"] = json!(time::format_ms(modified));
            v["conflict_copy"] = json!(saved_as);
        }
    }
    Ok(v)
}

/// The note's text and time when it is a file.
fn current(s: &Space, rel: &str) -> Result<Option<(String, i64)>, ToolError> {
    let p = s.path(rel)?;
    if p.is_dir() {
        return Err(ToolError::new(format!("\"{rel}\" is a folder")));
    }
    Ok(if p.is_file() {
        Some(s.read(rel)?)
    } else {
        None
    })
}

fn write(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = note_path(&a.str("path")?)?;
    let texte = a.str("text")?;
    let mode = a
        .opt_str("mode")?
        .unwrap_or_else(|| "replace".into())
        .trim()
        .to_ascii_lowercase();
    let avant = current(&s, &rel)?;
    let nouveau = match (mode.as_str(), &avant) {
        ("create", Some(_)) => {
            return Err(ToolError::new(format!(
                "\"{rel}\" exists: use mode replace, append or prepend"
            )))
        }
        ("create" | "replace", _) | ("append" | "prepend", None) => texte,
        ("append", Some((ancien, _))) => appended(ancien, &texte),
        ("prepend", Some((ancien, _))) => prepended(ancien, &texte),
        (autre, _) => {
            return Err(ToolError::new(format!(
                "mode \"{autre}\" is not one of replace, create, append, prepend"
            )))
        }
    };
    let mut v = save_note(&s, &rel, &nouveau, avant.as_ref())?;
    v["space"] = json!(s.config.name);
    Ok(v)
}

fn edit(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = existing_note(&s, &a.str("path")?)?;
    let avant = s.read(&rel)?;
    let eol = iris_notes::edit::line_ending_of(&avant.0);
    let cherche = with_eol(&a.str("find")?, eol);
    let remplace = with_eol(&a.str("replace")?, eol);
    let tout = a.bool_or("all", false)?;
    if cherche.is_empty() {
        return Err(ToolError::new("find is empty"));
    }
    let fois = avant.0.matches(cherche.as_str()).count();
    if fois == 0 {
        return Err(ToolError::new(
            "find was not found in the note (it must match exactly: read the note first)",
        ));
    }
    if fois > 1 && !tout {
        return Err(ToolError::new(format!(
            "find is in the note {fois} times: give more text around it, or set all"
        )));
    }
    let nouveau = if tout {
        avant.0.replace(cherche.as_str(), &remplace)
    } else {
        avant.0.replacen(cherche.as_str(), &remplace, 1)
    };
    let mut v = save_note(&s, &rel, &nouveau, Some(&avant))?;
    v["replacements"] = json!(if tout { fois } else { 1 });
    Ok(v)
}

fn create_folder(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = clean_file(&a.str("path")?)?;
    let p = s.path(&rel)?;
    if p.is_file() {
        return Err(ToolError::new(format!("\"{rel}\" is a file")));
    }
    let existait = p.is_dir();
    std::fs::create_dir_all(&p)?;
    Ok(json!({ "path": rel, "created": !existait }))
}

fn move_to(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = existing(&s, &a.str("path")?)?;
    let nom = a.opt_str("name")?.filter(|x| !x.trim().is_empty());
    let dossier = a.opt_str("folder")?.map(|d| clean(&d)).transpose()?;
    if nom.is_none() && dossier.is_none() {
        return Err(ToolError::new("give name, folder, or both"));
    }
    let avant = s.notes();
    let mut nouveau = rel.clone();
    if let Some(nom) = nom {
        // The extension is kept by the vault: one typed with the name is not doubled.
        let nom = nom.trim();
        let est_dossier = s.path(&nouveau)?.is_dir();
        let nom = match nouveau.rsplit_once('.') {
            Some((_, ext)) if !est_dossier && !ext.contains('/') => {
                let suffixe = format!(".{ext}");
                let k = nom.len().saturating_sub(suffixe.len());
                if k > 0 && nom.is_char_boundary(k) && nom[k..].eq_ignore_ascii_case(&suffixe) {
                    &nom[..k]
                } else {
                    nom
                }
            }
            _ => nom,
        };
        nouveau = s.rename(&nouveau, nom)?;
    }
    if let Some(d) = dossier {
        if !d.is_empty() && s.path(&d)?.is_file() {
            return Err(ToolError::new(format!("\"{d}\" is a file, not a folder")));
        }
        if parent_of(&nouveau) != d {
            nouveau = s.move_to(&nouveau, &d)?;
        }
    }
    let changees = if nouveau == rel {
        Vec::new()
    } else {
        s.relink(&Space::moves_of(&avant, &rel, &nouveau), &avant)?
    };
    Ok(json!({
        "from": rel,
        "path": nouveau,
        "links_rewritten_in": changees,
    }))
}

fn delete(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = existing(&s, &a.str("path")?)?;
    s.delete(&rel, n.bin)?;
    Ok(json!({ "deleted": rel, "to": "the Recycle Bin" }))
}

fn search(n: &Notes, a: &Args) -> ToolResult {
    let requete = a.str("query")?;
    let max = a.opt_i64("limit")?.unwrap_or(20).clamp(1, 200) as usize;
    let espaces = match a.opt_str("space")? {
        Some(v) => vec![find_space(all_spaces(n)?, &v)?],
        None => all_spaces(n)?,
    };
    let mut trouves = Vec::new();
    for s in &espaces {
        if trouves.len() >= max {
            break;
        }
        for (chemin, ligne) in s.search(&requete, max - trouves.len()) {
            trouves.push(json!({ "space": s.config.name, "path": chemin, "line": ligne }));
        }
    }
    Ok(json!({ "results": trouves }))
}

fn backlinks(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = existing_note(&s, &a.str("path")?)?;
    let liste: Vec<Value> = s
        .backlinks(&rel)
        .into_iter()
        .map(|(p, l)| json!({ "path": p, "line": l }))
        .collect();
    Ok(json!({ "path": rel, "backlinks": liste }))
}

/// Tags and how many notes have each, templates aside (as `Space::all_tags` counts).
fn tag_counts(s: &Space, compte: &mut BTreeMap<String, usize>) {
    let modeles = format!("{}/", s.config.templates);
    for rel in s.notes().into_iter().filter(|r| !r.starts_with(&modeles)) {
        if let Ok((texte, _)) = s.read(&rel) {
            for t in meta::tags(&texte) {
                *compte.entry(t).or_insert(0) += 1;
            }
        }
    }
}

fn tags(n: &Notes, a: &Args) -> ToolResult {
    let espaces = match a.opt_str("space")? {
        Some(v) => vec![find_space(all_spaces(n)?, &v)?],
        None => all_spaces(n)?,
    };
    let mut compte = BTreeMap::new();
    for s in &espaces {
        tag_counts(s, &mut compte);
    }
    let mut liste: Vec<(String, usize)> = compte.into_iter().collect();
    liste.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    let liste: Vec<Value> = liste
        .into_iter()
        .map(|(t, k)| json!({ "tag": t, "notes": k }))
        .collect();
    Ok(json!({ "tags": liste }))
}

fn versions(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = existing_note(&s, &a.str("path")?)?;
    if let Some(quand) = a.opt_i64("version")? {
        let texte = s
            .read_version(&rel, quand)
            .map_err(|_| ToolError::new(format!("no version {quand} of \"{rel}\"")))?;
        return Ok(json!({
            "path": rel,
            "version": quand,
            "saved": time::format_ms(quand),
            "text": texte,
        }));
    }
    let liste: Vec<Value> = s
        .versions(&rel)
        .into_iter()
        .map(|(quand, octets)| {
            json!({ "version": quand, "saved": time::format_ms(quand), "bytes": octets })
        })
        .collect();
    Ok(json!({ "path": rel, "versions": liste }))
}

fn templates(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let liste: Vec<Value> = s
        .templates()
        .into_iter()
        .map(|(nom, rel)| {
            let texte = s.read(&rel).map(|(t, _)| t).unwrap_or_default();
            json!({ "name": nom, "path": rel, "text": texte })
        })
        .collect();
    Ok(json!({
        "space": s.config.name,
        "folder": s.config.templates,
        "templates": liste,
        "variables": ["{{title}}", "{{date}}", "{{time}}", "{{course}}", "{{cursor}}"],
    }))
}

fn from_template(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let voulu = a.str("template")?;
    let modeles = s.templates();
    let Some((_, chemin)) = modeles
        .iter()
        .find(|(nom, _)| nom.eq_ignore_ascii_case(voulu.trim()))
    else {
        let noms: Vec<&str> = modeles.iter().map(|(nom, _)| nom.as_str()).collect();
        return Err(ToolError::new(format!(
            "no template \"{voulu}\"; the templates are: {}",
            if noms.is_empty() {
                "none".to_string()
            } else {
                noms.join(", ")
            }
        )));
    };
    let (modele, _) = s.read(chemin)?;
    let rel = note_path(&a.str("path")?)?;
    if current(&s, &rel)?.is_some() {
        return Err(ToolError::new(format!("\"{rel}\" exists")));
    }
    let maintenant = chrono::Local::now();
    let (texte, _) = iris_notes::template::expand(
        &modele,
        &iris_notes::template::Values {
            title: links::stem(&rel).to_string(),
            date: maintenant.format("%Y-%m-%d").to_string(),
            time: maintenant.format("%H:%M").to_string(),
            course: a.opt_str("course")?.unwrap_or_default(),
        },
    );
    let mut v = save_note(&s, &rel, &texte, None)?;
    v["text"] = json!(texte);
    Ok(v)
}

// --- Spreadsheets ---------------------------------------------------------------------

/// A spreadsheet's path: `.sheet` added when missing.
fn sheet_path(raw: &str) -> Result<String, ToolError> {
    let r = clean_file(raw)?;
    Ok(if EntryKind::of(&r) == EntryKind::Sheet {
        r
    } else {
        format!("{r}.sheet")
    })
}

/// A spreadsheet of the space: by its path, else by its name wherever it is.
fn find_sheet(s: &Space, raw: &str) -> Result<String, ToolError> {
    let rel = sheet_path(raw)?;
    if s.path(&rel)?.is_file() {
        return Ok(rel);
    }
    let nom = rel.rsplit('/').next().unwrap_or(&rel).to_lowercase();
    let ouverts: HashSet<String> = s.folders().into_iter().collect();
    let trouves: Vec<String> = s
        .tree(&ouverts)?
        .into_iter()
        .filter(|e| e.kind == EntryKind::Sheet)
        .map(|e| e.rel)
        .filter(|r| r.rsplit('/').next().unwrap_or(r).to_lowercase() == nom)
        .collect();
    match trouves.as_slice() {
        [un] => Ok(un.clone()),
        [] => Err(ToolError::new(format!(
            "no spreadsheet \"{rel}\" in the space \"{}\"",
            s.config.name
        ))),
        plusieurs => Err(ToolError::new(format!(
            "several spreadsheets are named so: {}",
            plusieurs.join(", ")
        ))),
    }
}

fn load_book(s: &Space, rel: &str) -> Result<(Workbook, i64), ToolError> {
    let (texte, modifie) = s.read(rel)?;
    let book = Workbook::from_json(&texte)
        .map_err(|e| ToolError::new(format!("\"{rel}\" cannot be read as a spreadsheet: {e}")))?;
    Ok((book, modifie))
}

/// The sheet an argument names, the first when it names none.
fn sheet_index(book: &Workbook, a: &Args) -> Result<usize, ToolError> {
    match a.opt_str("sheet")? {
        None => Ok(0),
        Some(nom) => book.sheet_index(nom.trim()).ok_or_else(|| {
            let noms: Vec<&str> = book.sheets.iter().map(|s| s.name.as_str()).collect();
            ToolError::new(format!(
                "no sheet \"{nom}\"; the sheets are: {}",
                noms.join(", ")
            ))
        }),
    }
}

/// A sheet's name as a workbook takes it (see `Workbook::rename_sheet`).
fn valid_sheet_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.contains(['\'', '!', '[', ']', '*', '?', '/', '\\', ':'])
}

/// A cell's value for the AI: a number, a text, a truth, null, or an error's text.
fn value_json(v: &iris_sheets::Value) -> Value {
    match v {
        iris_sheets::Value::Empty => Value::Null,
        iris_sheets::Value::Number(x) => {
            serde_json::Number::from_f64(*x).map_or(Value::Null, Value::Number)
        }
        iris_sheets::Value::Text(t) => Value::String(t.clone()),
        iris_sheets::Value::Bool(b) => Value::Bool(*b),
        iris_sheets::Value::Error(e) => Value::String(e.text().to_string()),
    }
}

/// What a chart reads now, in short.
fn series_json(book: &Workbook, values: &iris_sheets::Values, c: &Chart) -> Value {
    match chart::series(book, values, c, &Locale::english()) {
        Series::Categories { names, items, .. } => json!({
            "series": names,
            "categories": items
                .iter()
                .map(|i| json!({ "label": i.label, "values": i.values }))
                .collect::<Vec<_>>(),
        }),
        Series::Flow {
            nodes,
            links: bandes,
            ..
        } => json!({
            "nodes": nodes
                .iter()
                .map(|n| json!({ "label": n.label, "step": n.depth, "value": n.value }))
                .collect::<Vec<_>>(),
            "bands": bandes
                .iter()
                .map(|l| json!({ "from": l.from, "to": l.to, "value": l.value }))
                .collect::<Vec<_>>(),
        }),
        Series::Empty(pourquoi) => json!({ "empty": pourquoi }),
    }
}

fn chart_json(book: &Workbook, values: &iris_sheets::Values, rel: &str, c: &Chart) -> Value {
    let lettres =
        |v: &[u32]| -> Vec<String> { v.iter().map(|k| iris_sheets::col_name(*k)).collect() };
    json!({
        "name": c.name,
        "kind": c.kind.key(),
        "sheet": c.sheet,
        "header_row": c.header + 1,
        "categories": lettres(&c.groups),
        "values": lettres(&c.values),
        "embed": format!("![[{rel}#{}]]", c.name),
        "data": series_json(book, values, c),
    })
}

fn sheets_read(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = find_sheet(&s, &a.str("path")?)?;
    let (book, modifie) = load_book(&s, &rel)?;
    let k = sheet_index(&book, a)?;
    let plage = a
        .opt_str("range")?
        .map(|r| {
            Range::parse(&r)
                .ok_or_else(|| ToolError::new(format!("\"{r}\" is not a range (A1:D20)")))
        })
        .transpose()?;
    let values = book.compute();
    let l = Locale::english();
    let feuille = &book.sheets[k];
    let mut cellules = Map::new();
    let mut tronque = false;
    for (adresse, cellule) in &feuille.cells {
        if plage.is_some_and(|p| !p.contains(*adresse)) || cellule.input.is_empty() {
            continue;
        }
        if cellules.len() >= MAX_CELLULES {
            tronque = true;
            break;
        }
        let v = values.get(&book, k, *adresse);
        let montre = iris_sheets::format::display(&v, &cellule.format, &l);
        let mut c = json!({ "input": cellule.input, "value": value_json(&v) });
        if montre != cellule.input {
            c["shown"] = json!(montre);
        }
        cellules.insert(adresse.name(), c);
    }
    let feuilles: Vec<Value> = book
        .sheets
        .iter()
        .map(|f| {
            let (colonnes, lignes) = f.extent();
            json!({ "name": f.name, "columns": colonnes, "rows": lignes })
        })
        .collect();
    let graphiques: Vec<Value> = book
        .charts
        .iter()
        .map(|c| chart_json(&book, &values, &rel, c))
        .collect();
    Ok(json!({
        "space": s.config.name,
        "path": rel,
        "modified": time::format_ms(modifie),
        "sheets": feuilles,
        "sheet": feuille.name,
        "cells": cellules,
        "truncated": tronque,
        "charts": graphiques,
        "embed_table": format!("![[{rel}]]"),
    }))
}

/// What a cell is given, as typed in it.
fn cell_input(v: &Value, ou: &str) -> Result<String, ToolError> {
    match v {
        Value::Null => Ok(String::new()),
        Value::String(t) => Ok(t.clone()),
        Value::Number(x) => Ok(x.to_string()),
        Value::Bool(b) => Ok(if *b { "TRUE" } else { "FALSE" }.to_string()),
        _ => Err(ToolError::new(format!(
            "{ou}: a cell takes a text, a number, true/false or null"
        ))),
    }
}

fn sheets_write(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = sheet_path(&a.str("path")?)?;
    let p = s.path(&rel)?;
    if p.is_dir() {
        return Err(ToolError::new(format!("\"{rel}\" is a folder")));
    }
    let existait = p.is_file();
    let (mut book, modifie) = if existait {
        let (b, m) = load_book(&s, &rel)?;
        (b, Some(m))
    } else {
        (Workbook::default(), None)
    };
    let mut feuille_creee = false;
    let k = match a.opt_str("sheet")? {
        None => 0,
        Some(nom) => match book.sheet_index(nom.trim()) {
            Some(i) => i,
            None => {
                let nom = nom.trim();
                if !valid_sheet_name(nom) {
                    return Err(ToolError::new(format!(
                        "\"{nom}\" cannot name a sheet (no ' ! [ ] * ? / \\ :)"
                    )));
                }
                feuille_creee = true;
                if !existait {
                    // A new file: its one empty sheet takes the name.
                    book.sheets[0].name = nom.to_string();
                    0
                } else {
                    book.sheets.push(Sheet::new(nom));
                    book.sheets.len() - 1
                }
            }
        },
    };

    let mut ecrites: Vec<(Addr, String)> = Vec::new();
    if let Some(cells) = a.get("cells") {
        let Some(objet) = cells.as_object() else {
            return Err(ToolError::new(
                "cells must be an object: {\"A1\": \"text\", \"B2\": \"=A1*2\"}",
            ));
        };
        for (nom, v) in objet {
            let adresse = Addr::parse(nom.trim())
                .ok_or_else(|| ToolError::new(format!("\"{nom}\" is not a cell (A1)")))?;
            ecrites.push((adresse, cell_input(v, nom)?));
        }
    }
    if let Some(rows) = a.get("rows") {
        let Some(lignes) = rows.as_array() else {
            return Err(ToolError::new("rows must be an array of arrays"));
        };
        let debut = a.opt_str("start")?.unwrap_or_else(|| "A1".into());
        let origine = Addr::parse(debut.trim())
            .ok_or_else(|| ToolError::new(format!("start \"{debut}\" is not a cell (A1)")))?;
        for (r, ligne) in lignes.iter().enumerate() {
            let Some(ligne) = ligne.as_array() else {
                return Err(ToolError::new(format!("rows[{r}] is not an array")));
            };
            for (c, v) in ligne.iter().enumerate() {
                if v.is_null() {
                    continue;
                }
                let adresse = origine
                    .offset(c as i64, r as i64)
                    .ok_or_else(|| ToolError::new("rows go past the sheet's edge"))?;
                ecrites.push((adresse, cell_input(v, &adresse.name())?));
            }
        }
    }
    if ecrites.is_empty() && !feuille_creee {
        return Err(ToolError::new("give cells or rows to write"));
    }
    if ecrites.len() > MAX_ECRITES {
        return Err(ToolError::new(format!(
            "at most {MAX_ECRITES} cells in one call"
        )));
    }
    for (adresse, entree) in &ecrites {
        book.sheets[k].set_input(*adresse, entree);
    }
    let resultat = s.write(&rel, &book.to_json(), modifie)?;
    let values = book.compute();
    let mut valeurs = Map::new();
    let mut erreurs = Vec::new();
    for (adresse, entree) in &ecrites {
        let v = values.get(&book, k, *adresse);
        if let iris_sheets::Value::Error(e) = &v {
            erreurs.push(json!({ "cell": adresse.name(), "input": entree, "error": e.text() }));
        }
        valeurs.insert(adresse.name(), value_json(&v));
    }
    let mut v = json!({
        "space": s.config.name,
        "path": rel,
        "created": !existait,
        "sheet": book.sheets[k].name,
        "written": ecrites.len(),
        "values": valeurs,
        "errors": erreurs,
        "embed_table": format!("![[{rel}]]"),
    });
    if let WriteOutcome::Conflict { saved_as, .. } = resultat {
        v["conflict_copy"] = json!(saved_as);
    }
    Ok(v)
}

fn kind_of(k: &str) -> Result<Kind, ToolError> {
    let k = k.trim().to_ascii_lowercase();
    Kind::ALL.into_iter().find(|x| x.key() == k).ok_or_else(|| {
        ToolError::new(format!(
            "kind \"{k}\" is not one of flow, bars, columns, donut, line"
        ))
    })
}

/// Columns by their letters (`B`), or by a cell of them (`B1`).
fn columns(liste: &[String]) -> Result<Vec<u32>, ToolError> {
    liste
        .iter()
        .map(|c| {
            let c = c.trim();
            iris_sheets::cell::col_index(c)
                .or_else(|| Addr::parse(c).map(|a| a.col))
                .ok_or_else(|| ToolError::new(format!("\"{c}\" is not a column (A, B, AA…)")))
        })
        .collect()
}

fn add_chart(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = find_sheet(&s, &a.str("path")?)?;
    let (mut book, modifie) = load_book(&s, &rel)?;
    let nom = a.str("name")?.trim().to_string();
    if !chart::valid_name(&nom) {
        return Err(ToolError::new(
            "a chart's name cannot be empty, look like a cell (A1), or hold ] | [ #",
        ));
    }
    let deja = book.chart_index(&nom);
    if deja.is_some() && !a.bool_or("replace", false)? {
        return Err(ToolError::new(format!(
            "a chart is already named \"{nom}\": set replace, or choose another name"
        )));
    }
    let k = sheet_index(&book, a)?;
    let kind = a.opt_str("kind")?.map(|x| kind_of(&x)).transpose()?;
    let values = book.compute();
    let mut c = match a.opt_str("range")? {
        Some(r) => {
            let plage = Range::parse(&r)
                .ok_or_else(|| ToolError::new(format!("\"{r}\" is not a range (A1:C20)")))?;
            chart::guess(&book, &values, k, plage)
        }
        None => Chart {
            name: String::new(),
            kind: kind.unwrap_or(Kind::Bars),
            sheet: book.sheets[k].name.clone(),
            header: 0,
            groups: Vec::new(),
            values: Vec::new(),
            at: None,
        },
    };
    if let Some(h) = a.opt_i64("header_row")? {
        if h < 1 {
            return Err(ToolError::new("header_row counts from 1"));
        }
        c.header = (h - 1) as u32;
    }
    let categories = a.str_list("categories")?;
    if !categories.is_empty() {
        c.groups = columns(&categories)?;
    }
    if a.has("values") {
        c.values = columns(&a.str_list("values")?)?;
    }
    if c.groups.is_empty() {
        return Err(ToolError::new(
            "give range, or categories (the columns the rows are grouped by)",
        ));
    }
    if let Some(kind) = kind {
        c.kind = kind;
    }
    c.name = nom.clone();
    c.sheet = book.sheets[k].name.clone();
    let (groupes, series) = (c.groups.len(), c.values.len());
    c.fit();
    let mut remarques = Vec::new();
    if c.groups.len() < groupes {
        remarques.push("only a flow goes through several category columns: the first is kept");
    }
    if c.values.len() < series {
        remarques.push("only columns and line draw several series: the first is kept");
    }
    match deja {
        // Replaced: its card stays where it was put on the canvas.
        Some(i) => {
            c.at = book.charts[i].at;
            book.charts[i] = c.clone();
        }
        None => book.charts.push(c.clone()),
    }
    let resultat = s.write(&rel, &book.to_json(), Some(modifie))?;
    let values = book.compute();
    let mut v = chart_json(&book, &values, &rel, &c);
    v["path"] = json!(rel);
    v["notes"] = json!(remarques);
    v["how_to_show"] = json!(format!(
        "Put ![[{rel}#{nom}]] alone on a line of a note of this space."
    ));
    if let WriteOutcome::Conflict { saved_as, .. } = resultat {
        v["conflict_copy"] = json!(saved_as);
    }
    Ok(v)
}

// --- Whiteboards ----------------------------------------------------------------------

/// The most items `boards_add` takes in one call.
const MAX_ITEMS: usize = 2000;

/// A whiteboard's path: `.board` added when missing.
fn board_path(raw: &str) -> Result<String, ToolError> {
    let r = clean_file(raw)?;
    Ok(if EntryKind::of(&r) == EntryKind::Board {
        r
    } else {
        format!("{r}.board")
    })
}

/// A whiteboard of the space: by its path, else by its name wherever it is.
fn find_board(s: &Space, raw: &str) -> Result<String, ToolError> {
    let rel = board_path(raw)?;
    if s.path(&rel)?.is_file() {
        return Ok(rel);
    }
    let nom = rel.rsplit('/').next().unwrap_or(&rel).to_lowercase();
    let ouverts: HashSet<String> = s.folders().into_iter().collect();
    let trouves: Vec<String> = s
        .tree(&ouverts)?
        .into_iter()
        .filter(|e| e.kind == EntryKind::Board)
        .map(|e| e.rel)
        .filter(|r| r.rsplit('/').next().unwrap_or(r).to_lowercase() == nom)
        .collect();
    match trouves.as_slice() {
        [un] => Ok(un.clone()),
        [] => Err(ToolError::new(format!(
            "no whiteboard \"{rel}\" in the space \"{}\"",
            s.config.name
        ))),
        plusieurs => Err(ToolError::new(format!(
            "several whiteboards are named so: {}",
            plusieurs.join(", ")
        ))),
    }
}

fn load_board(s: &Space, rel: &str) -> Result<(Board, i64), ToolError> {
    let (texte, modifie) = s.read(rel)?;
    let b = if texte.trim().is_empty() {
        Board::default()
    } else {
        Board::from_json(&texte)
            .map_err(|e| ToolError::new(format!("\"{rel}\" cannot be read as a whiteboard: {e}")))?
    };
    Ok((b, modifie))
}

fn boards_read(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = find_board(&s, &a.str("path")?)?;
    let (b, modifie) = load_board(&s, &rel)?;
    let bornes = b
        .bounds()
        .map(|r| json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h }));
    Ok(json!({
        "space": s.config.name,
        "path": rel,
        "modified": time::format_ms(modifie),
        "items": serde_json::to_value(&b.items)?,
        "bounds": bornes,
    }))
}

/// An item's kind as the tools name it.
fn board_kind(k: &str) -> Result<BoardKind, ToolError> {
    Ok(match k.trim().to_ascii_lowercase().as_str() {
        "shape" => BoardKind::Shape,
        "sticky" | "note" | "sticky_note" => BoardKind::Sticky,
        "text" => BoardKind::Text,
        "frame" => BoardKind::Frame,
        "arrow" | "connector" | "line" => BoardKind::Connector,
        "pen" | "ink" | "stroke" => BoardKind::Ink,
        autre => {
            return Err(ToolError::new(format!(
                "\"{autre}\" is not a kind of item (shape, sticky, text, frame, arrow, pen)"
            )))
        }
    })
}

/// The fields given to an item laid over it: everything `boards_add` takes but its
/// kind, its ref, its id and an arrow's ends.
fn board_fields(it: &mut Item, o: &Map<String, Value>) -> Result<(), ToolError> {
    let mut v = serde_json::to_value(&*it)?;
    let Some(champs) = v.as_object_mut() else {
        return Err(ToolError::new("this item cannot be changed"));
    };
    for (k, val) in o {
        match k.as_str() {
            "ref" | "kind" | "id" | "from" | "to" => {}
            "x" | "y" | "w" | "h" | "size" | "width" | "opacity" => {
                if !val.is_number() {
                    return Err(ToolError::new(format!("\"{k}\" must be a number")));
                }
                champs.insert(k.clone(), val.clone());
            }
            "text" | "bold" | "italic" | "rounded" | "locked" => {
                champs.insert(k.clone(), val.clone());
            }
            "fill" | "stroke" | "color" => {
                let c = val
                    .as_str()
                    .ok_or_else(|| ToolError::new(format!("\"{k}\" must be a colour's name")))?
                    .trim()
                    .to_ascii_lowercase();
                if c != "none" && Colour::parse(&c) == Colour::None {
                    return Err(ToolError::new(format!(
                        "\"{c}\" is not a colour (yellow, blue-soft, ink, none, #3a7bd5…)"
                    )));
                }
                champs.insert(k.clone(), json!(c));
            }
            "shape" | "font" | "way" | "start" | "end" | "align" | "valign" => {
                let s = val
                    .as_str()
                    .ok_or_else(|| ToolError::new(format!("\"{k}\" must be a word")))?
                    .trim()
                    .to_ascii_lowercase();
                let s = match (k.as_str(), s.as_str()) {
                    (_, "handwriting") => "hand".to_string(),
                    ("align", "centre" | "middle") => "center".to_string(),
                    ("valign", "center" | "centre") => "middle".to_string(),
                    (_, "square") => "rect".to_string(),
                    (_, "circle" | "oval") => "ellipse".to_string(),
                    _ => s,
                };
                champs.insert(k.clone(), json!(s));
            }
            "points" => {
                let liste = val
                    .as_array()
                    .ok_or_else(|| ToolError::new("\"points\" must be a list of [x, y]"))?;
                let mut pts = Vec::new();
                for p in liste {
                    let (x, y) = match p {
                        Value::Array(xy) if xy.len() == 2 => (xy[0].as_f64(), xy[1].as_f64()),
                        Value::Object(m) => (
                            m.get("x").and_then(Value::as_f64),
                            m.get("y").and_then(Value::as_f64),
                        ),
                        _ => (None, None),
                    };
                    match (x, y) {
                        (Some(x), Some(y)) => pts.push(json!({ "x": x, "y": y })),
                        _ => return Err(ToolError::new("\"points\" must be a list of [x, y]")),
                    }
                }
                champs.insert(k.clone(), Value::Array(pts));
            }
            autre => {
                return Err(ToolError::new(format!(
                    "\"{autre}\" is not a field of a whiteboard's item"
                )))
            }
        }
    }
    let id = it.id;
    let kind = it.kind;
    *it = serde_json::from_value(v).map_err(|e| ToolError::new(format!("item {id}: {e}")))?;
    it.id = id;
    it.kind = kind;
    Ok(())
}

/// An arrow's end as given: an item (by id or ref) or a point.
enum BoardEnd {
    Item(u64),
    Point(Pt),
}

fn board_end(v: &Value, refs: &HashMap<String, u64>) -> Result<BoardEnd, ToolError> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .map(BoardEnd::Item)
            .ok_or_else(|| ToolError::new("an item's id is a whole number")),
        Value::String(s) => refs
            .get(s)
            .copied()
            .or_else(|| s.trim().parse::<u64>().ok())
            .map(BoardEnd::Item)
            .ok_or_else(|| ToolError::new(format!("no item named \"{s}\" in this call"))),
        Value::Array(xy) if xy.len() == 2 => match (xy[0].as_f64(), xy[1].as_f64()) {
            (Some(x), Some(y)) => Ok(BoardEnd::Point(Pt::new(x as f32, y as f32))),
            _ => Err(ToolError::new("a point is [x, y]")),
        },
        Value::Object(m) => match (
            m.get("x").and_then(Value::as_f64),
            m.get("y").and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => Ok(BoardEnd::Point(Pt::new(x as f32, y as f32))),
            _ => Err(ToolError::new("a point is {\"x\": …, \"y\": …}")),
        },
        _ => Err(ToolError::new(
            "an arrow's end is an item's id, a ref, or a point",
        )),
    }
}

/// An arrow's ends tied to the middles of the sides its items turn to each other.
fn tie(b: &Board, from: &BoardEnd, to: &BoardEnd) -> Result<(End, End), ToolError> {
    let boite = |e: &BoardEnd| -> Result<Option<Item>, ToolError> {
        match e {
            BoardEnd::Item(id) => match b.get(*id) {
                Some(it) if it.kind.boxed() => Ok(Some(it.clone())),
                Some(_) => Err(ToolError::new(format!(
                    "item {id} is an arrow or a stroke: an arrow ties to a shape, a sticky note, a text or a frame"
                ))),
                None => Err(ToolError::new(format!("no item {id} on this whiteboard"))),
            },
            BoardEnd::Point(_) => Ok(None),
        }
    };
    let centre = |e: &BoardEnd, it: &Option<Item>| match (e, it) {
        (_, Some(it)) => it.rect().center(),
        (BoardEnd::Point(p), None) => *p,
        (BoardEnd::Item(_), None) => Pt::default(),
    };
    let (ia, ib) = (boite(from)?, boite(to)?);
    let (ca, cb) = (centre(from, &ia), centre(to, &ib));
    let bout = |e: &BoardEnd, it: &Option<Item>, vers: Pt| match (e, it) {
        (_, Some(it)) => {
            let (u, v) = SIDES
                .iter()
                .copied()
                .min_by(|x, y| {
                    it.at(x.0, x.1)
                        .distance(vers)
                        .total_cmp(&it.at(y.0, y.1).distance(vers))
                })
                .unwrap_or((0.5, 0.5));
            End::on(it.id, u, v)
        }
        (BoardEnd::Point(p), None) => End::free(*p),
        (BoardEnd::Item(_), None) => End::default(),
    };
    Ok((bout(from, &ia, cb), bout(to, &ib, ca)))
}

/// The size a box takes when none is given.
fn board_size(k: BoardKind) -> (f32, f32) {
    match k {
        BoardKind::Sticky => (160.0, 160.0),
        BoardKind::Text => (200.0, 30.0),
        BoardKind::Frame => (480.0, 320.0),
        _ => (150.0, 72.0),
    }
}

fn boards_add(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = board_path(&a.str("path")?)?;
    let p = s.path(&rel)?;
    if p.is_dir() {
        return Err(ToolError::new(format!("\"{rel}\" is a folder")));
    }
    let existait = p.is_file();
    let (mut b, modifie) = if existait {
        let (b, m) = load_board(&s, &rel)?;
        (b, Some(m))
    } else {
        (Board::default(), None)
    };
    let Some(Value::Array(liste)) = a.get("items") else {
        return Err(ToolError::new("give items: a list of objects"));
    };
    if liste.is_empty() {
        return Err(ToolError::new("give at least one item"));
    }
    if liste.len() > MAX_ITEMS {
        return Err(ToolError::new(format!(
            "at most {MAX_ITEMS} items in one call"
        )));
    }
    let mut refs: HashMap<String, u64> = HashMap::new();
    let mut nouveaux = Vec::new();
    let mut fleches = Vec::new();
    for (i, v) in liste.iter().enumerate() {
        let Value::Object(o) = v else {
            return Err(ToolError::new(format!("item {i} is not an object")));
        };
        let kind = board_kind(o.get("kind").and_then(Value::as_str).unwrap_or("shape"))?;
        let id = b.take_id();
        if let Some(r) = o.get("ref").and_then(Value::as_str) {
            refs.insert(r.to_string(), id);
        }
        let (w, h) = board_size(kind);
        let r = Rect::new(0.0, 0.0, w, h);
        let mut it = match kind {
            BoardKind::Shape => Item::shape(id, BoardShape::Rounded, r),
            BoardKind::Sticky => Item::sticky(id, r),
            BoardKind::Text => Item::text(id, r),
            BoardKind::Frame => Item::frame(id, r),
            BoardKind::Connector => Item::connector(id, End::default(), End::default()),
            BoardKind::Ink => Item::ink(id, Vec::new()),
        };
        board_fields(&mut it, o)?;
        if kind == BoardKind::Ink && it.points.is_empty() {
            return Err(ToolError::new(format!(
                "item {i}: a pen stroke needs points"
            )));
        }
        if kind == BoardKind::Connector {
            let (Some(de), Some(vers)) = (o.get("from"), o.get("to")) else {
                return Err(ToolError::new(format!(
                    "item {i}: an arrow needs from and to"
                )));
            };
            fleches.push((id, de.clone(), vers.clone()));
        }
        nouveaux.push(it);
    }
    let ids: Vec<u64> = nouveaux.iter().map(|i| i.id).collect();
    // Frames under everything, the rest on top in the order given.
    for it in nouveaux {
        if it.kind == BoardKind::Frame {
            b.items.insert(0, it);
        } else {
            b.items.push(it);
        }
    }
    for (id, de, vers) in fleches {
        let (fa, fb) = tie(&b, &board_end(&de, &refs)?, &board_end(&vers, &refs)?)?;
        if let Some(it) = b.get_mut(id) {
            it.from = Some(fa);
            it.to = Some(fb);
        }
    }
    let resultat = s.write(&rel, &b.to_json(), modifie)?;
    let mut v = json!({
        "space": s.config.name,
        "path": rel,
        "created": !existait,
        "ids": ids,
        "refs": refs,
    });
    if let WriteOutcome::Conflict { saved_as, .. } = resultat {
        v["conflict_copy"] = json!(saved_as);
    }
    Ok(v)
}

fn boards_update(n: &Notes, a: &Args) -> ToolResult {
    let s = pick(n, a)?;
    let rel = find_board(&s, &a.str("path")?)?;
    let (mut b, modifie) = load_board(&s, &rel)?;
    let mut changes = 0;
    if let Some(Value::Array(liste)) = a.get("update") {
        for (i, v) in liste.iter().enumerate() {
            let Value::Object(o) = v else {
                return Err(ToolError::new(format!("update {i} is not an object")));
            };
            let id = o
                .get("id")
                .and_then(Value::as_u64)
                .ok_or_else(|| ToolError::new(format!("update {i} needs the item's id")))?;
            let Some(mut it) = b.get(id).cloned() else {
                return Err(ToolError::new(format!("no item {id} on this whiteboard")));
            };
            board_fields(&mut it, o)?;
            if it.kind == BoardKind::Connector && (o.contains_key("from") || o.contains_key("to")) {
                let actuel = |e: Option<End>| match e {
                    Some(End { item: Some(i), .. }) => BoardEnd::Item(i),
                    Some(e) => BoardEnd::Point(Pt::new(e.x, e.y)),
                    None => BoardEnd::Point(Pt::default()),
                };
                let refs = HashMap::new();
                let de = match o.get("from") {
                    Some(v) => board_end(v, &refs)?,
                    None => actuel(it.from),
                };
                let vers = match o.get("to") {
                    Some(v) => board_end(v, &refs)?,
                    None => actuel(it.to),
                };
                let (fa, fb) = tie(&b, &de, &vers)?;
                it.from = Some(fa);
                it.to = Some(fb);
                it.points.clear();
            }
            if let Some(place) = b.get_mut(id) {
                *place = it;
            }
            changes += 1;
        }
    }
    let mut supprimes = Vec::new();
    if let Some(Value::Array(liste)) = a.get("delete") {
        for v in liste {
            let id = v
                .as_u64()
                .ok_or_else(|| ToolError::new("delete takes items' ids"))?;
            if b.get(id).is_none() {
                return Err(ToolError::new(format!("no item {id} on this whiteboard")));
            }
            supprimes.push(id);
        }
    }
    if changes == 0 && supprimes.is_empty() {
        return Err(ToolError::new("give update or delete"));
    }
    let avant = b.items.len();
    b.remove(&supprimes);
    let resultat = s.write(&rel, &b.to_json(), Some(modifie))?;
    let mut v = json!({
        "space": s.config.name,
        "path": rel,
        "updated": changes,
        "deleted": avant - b.items.len(),
    });
    if let WriteOutcome::Conflict { saved_as, .. } = resultat {
        v["conflict_copy"] = json!(saved_as);
    }
    Ok(v)
}

// --- The syntax -----------------------------------------------------------------------

fn syntax() -> String {
    format!(
        "{SYNTAX}\nFunctions: {}.\n",
        iris_sheets::eval::FUNCTIONS.join(", ")
    )
}

/// What Iris's parsers read (`iris-notes` block.rs, inline.rs, links.rs, meta.rs,
/// template.rs; the app's render.rs for callouts and embeds; `iris-sheets`).
const SYNTAX: &str = r##"# Iris notes: the syntax

A note is a Markdown file (`.md`) in a space. Iris reads Markdown, Obsidian's additions and short marks of its own. Each line is a block of its own, except the blocks that run over several lines: properties, fenced code, `$$` maths, quotes and callouts, tables, `:::` containers. Iris keeps a file's bytes as written: write lines ended by `\n`.

## Properties (front matter)

Only on the very first line: `---`, then `key: value` lines, closed by `---` (or `...`).

```
---
course: Analysis
date: 2026-11-06
tags: [maths, limits]
---
```

Values are kept as written. `tags:` (or `tag:`), as `[a, b]` or `a, b`, adds tags to the note. A note made from a calendar event carries `event: <key>` and `place: …`.

## Blocks

- Headings: `# ` to `###### ` (a space after the signs). `#word` is a tag, not a heading.
- Paragraph: any other line. An empty line is an empty block.
- Bullets: `- `, `* ` or `+ `. Nest with 2 spaces (or a tab) per level, up to 8 levels.
- Numbered: `1. ` or `1) `. The first item's number counts; the next items of the list are numbered on from it, whatever they say. A list goes on over an empty line; any other block ends it.
- Tasks: `- [ ] to do`, `- [x] done` (also `*`/`+`, `X`). A task line holding `[[task:ID]]` stays ticked together with that Iris task.
- Divider: `---`, `***` or `___` (three or more, spaces allowed). `---` on the first line opens properties when another `---` closes them.
- Quote: lines starting with `>` (up to 3 spaces before it); consecutive lines are one block.
- Fenced code: ```` ```lang ```` … ```` ``` ```` (or `~~~`), closed by a fence at least as long. A fence never closed is a line of its own.
- Display maths: `$$` on its own line, LaTeX lines, then a line ending with `$$`; or `$$ x^2 $$` on one line. Drawn as TeX does (fractions, roots, matrices, Greek…).
- Table: a line starting with `|`, then a separator line of `|`, `-`, `:` and spaces, then `|` rows:

```
| Item | Price |
|---|---|
| Book | 12 |
```

- Flashcard: `Question :: Answer` on one line (a space on each side of `::`, the line not starting with a backtick). Revised with spaced repetition.
- Embed: `![[file]]` or `![alt](url)` alone on its line (see Embeds).

## Callouts

The first line `> [!kind] Title`, the next `> ` lines its body. `> [!kind]- Title` is shown folded, `> [!kind]+ Title` unfolded but foldable. The kind is one word, any case.

```
> [!thm] Bolzano–Weierstrass
> Every bounded sequence of real numbers has a convergent subsequence.
```

| Kind | Shown as | Numbered |
|---|---|---|
| `def` | Definition | yes |
| `thm` | Theorem | yes |
| `prop` | Proposition | yes |
| `lem` | Lemma | yes |
| `cor` | Corollary | yes |
| `proof` | Proof | |
| `example` | Example | yes |
| `exo` | Exercise | yes |
| `sol` | Solution | |
| `question`, `faq`, `help` | Question (`question` is also a flashcard: the title the question, the body the answer) | |
| `important` | Important | |
| `summary`, `abstract`, `tldr` | Summary | |
| `tip`, `hint` | Tip | |
| `warning`, `caution`, `attention` | Warning | |
| `danger`, `error`, `failure` | Danger | |
| `info`, `todo` | Info | |
| `success`, `check`, `done` | Done | |
| `quote`, `cite` | Quote | |
| `bug` | Bug | |
| `note`, and any other kind | Note | |

Numbered kinds count through the note by label: "Theorem 1", "Theorem 2". A link `[[Note#thm-2]]` goes to the second theorem (kind, `-`, number).

## Containers

`:::kind Title` on its own line, the content, then `:::` alone on a line; containers inside are counted. Any word is a kind; Iris draws the container as one framed block titled by its title (or its kind). `:::col` lines inside separate parts and are not shown. The editor offers:

```
:::cols
Left part
:::col
Right part
:::
```

and `:::fold Details` … `:::`. A container never closed is not one.

## Marks in a line

| Write | Gives |
|---|---|
| `**bold**` | bold (may sit inside a word) |
| `*italic*` or `_italic_` | italic (not inside a word) |
| `__underline__` | underline (Iris; not inside a word) |
| `--strike--` or `~~strike~~` | strikethrough (`--` not inside a word, `---` never) |
| `==highlight==` | highlight |
| `=={g}text==` | highlight in a colour (codes below) |
| `{r}red text{/}` | coloured text, up to its `{/}` (colours nest) |
| `x^2^` | superscript (no space inside) |
| `H~2~O` | subscript (no space inside) |
| `` `code` `` | code (as many backticks to close as to open) |
| `$\frac{a}{b}$` | inline maths (LaTeX) |
| `^[a footnote]` | footnote |
| `[[Note]]` | link to a note |
| `[text](https://example.com)` | web link (no space in the address) |
| `https://example.com` | a bare address is a link |
| `#tag`, `#area/sub` | tag |
| `@2026-11-06` | a date (opens that day in the calendar) |
| `\*` | the character itself (any punctuation) |

Rules: a mark needs something other than a space just inside it (`a -- b` stays a dash, `2 * 3` a product). Marks do not cross one another: the first to close wins; they nest (`**{r}bold red{/}**`). Inside code and maths nothing else is read.

Colours (`{code}` … `{/}`, and `=={code}…==`):

| Colour | Codes |
|---|---|
| Red | `r`, `red`, `rouge` |
| Orange | `o`, `orange` |
| Yellow | `y`, `j`, `yellow`, `jaune` |
| Green | `g`, `v`, `green`, `vert` |
| Blue | `b`, `blue`, `bleu` |
| Purple | `p`, `purple`, `violet` |
| Grey | `n`, `grey`, `gray`, `gris` |

Inline maths: `$…$` with no space just inside, unless it holds a LaTeX command (`$ \forall x $`); a closing `$` followed by a digit does not close (`$5 and $6` is money).

Tags: after a space, the line's start or `(`; letters, digits, `_`, `-`, `/`; not digits only (`#2` is not a tag). Tags in code, maths and properties' text are not read as tags (but `tags:` is).

## Links

- `[[Note]]` by its name alone when enough; `[[Folder/Note]]` when two notes share a name. A link finds the exact path first, then the shortest path ending with it, case aside. `.md` is not written.
- `[[Note#Heading]]` a heading (by its words), `[[#Heading]]` one in the same note, `[[Note#thm-2]]` a numbered callout.
- `[[Note|label]]` shows the label.
- Links are rewritten everywhere when a note is renamed or moved in Iris (notes_move does it too).
- To the rest of Iris: `[[mail:ID]]`, `[[task:ID]]`, `[[event:KEY]]`, with `|label` if wanted.
- In the editor, typing `@` and a note's name offers notes and writes `[[Note]]`: in a file, write the link.

## Embeds (alone on their line)

A file is found by its path from the top of the space, then in the space's attachments folder (`_Fichiers`).

- Picture (png, jpg, jpeg, gif, webp, bmp, svg): `![[_Fichiers/photo.png]]`, `![[photo.png|300]]` (width in pixels, 16 or more), `![[photo.png|300x200]]` (fitted in a box), or `![alt](https://…)`.
- Spreadsheet as a table: `![[Budget.sheet]]` shows its first sheet's used cells (at most 30 rows by 12 columns); `![[Budget.sheet#A1:D10]]` a range of the first sheet; `|clip` cuts long text at the column's width instead of wrapping it (`![[Budget.sheet#A1:D10|clip]]`). The cells shown are computed when the note is shown.
- Chart: `![[Budget.sheet#Chart name]]` (a name that is not a range). The spreadsheet by its path in the space, or by its file name wherever it is. sheets_add_chart gives the exact line.
- Any other file or note: `![[Other note]]` shows as a reference card, not its content.

## Templates

Notes in the space's templates folder (`_Modèles`). A new note made from one has `{{title}}` (the new note's name), `{{date}}` (2026-11-06), `{{time}}` (14:30) and `{{course}}` filled in; `{{cursor}}` marks where the cursor goes and is taken out.

## Spreadsheets (`.sheet`)

A `.sheet` file is JSON written by Iris: change it with sheets_write and sheets_add_chart, never notes_write. A cell holds what was typed: a number (`3.5`, `3,5`, `1 234`, `12%`, `-4e3`), `TRUE`/`FALSE`, a text, or a formula starting with `=`.

Formulas: numbers, `"text"`, `TRUE`/`FALSE`, references `A1`, `$A$1`, ranges `A1:B3`, other sheets `Sheet2!A1`, `'My sheet'!A1:B3`; operators by Excel's precedence: comparisons (`=`, `<>`, `<`, `>`, `<=`, `>=`), `&`, `+ -`, `* /`, `^`, the sign, `%`; arguments separated by `,` or `;`. Errors: `#DIV/0!`, `#REF!`, `#NAME?`, `#VALUE!`, `#NUM!`, `#N/A`, `#CYCLE!`, `#ERROR!` (a formula that cannot be read).

Charts read a table: a row of headers and every row under it. They group the rows by category columns and add up a column of numbers, or count the rows when given none. Kinds: `flow` (bands from a whole to its parts through several category columns; a category written `Sent > Interview` is two steps; two columns naming each other chain their bands), `bars`, `columns` (several series), `donut`, `line` (several series). They take the colour of their cells' text.
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// Deletes for good: the tests have no Recycle Bin to fill.
    struct Away;

    impl RecycleBin for Away {
        fn throw(&self, path: &Path) -> iris_types::Result<()> {
            if path.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
            Ok(())
        }
    }

    /// Notes in the test's own folder: no setting, no environment variable.
    fn notes_in(dir: &Path) -> Notes {
        Notes {
            vault: Vault::open(dir.join("notes")).expect("vault"),
            externals: Vec::new(),
            bin: &Away,
        }
    }

    fn ask(ctx: &Ctx, n: &Notes, name: &str, args: Value) -> ToolResult {
        run(ctx, n, name, &Args::new(args))
    }

    #[test]
    fn a_note_is_written_read_edited_searched_moved_and_deleted() {
        let (ctx, dir, _rt) = super::super::test_ctx();
        let n = notes_in(dir.path());

        let s = ask(&ctx, &n, "notes_create_space", json!({"name": "Cours"})).unwrap();
        assert_eq!(s["name"], "Cours");
        let l = ask(&ctx, &n, "notes_spaces", json!({})).unwrap();
        assert_eq!(l["spaces"][0]["name"], "Cours");

        let w = ask(
            &ctx,
            &n,
            "notes_write",
            json!({"space": "Cours", "path": "Analyse/Limites", "text": "# Limites\n\nUne suite #maths converge.\n"}),
        )
        .unwrap();
        assert_eq!(w["path"], "Analyse/Limites.md");
        assert_eq!(w["created"], true);
        assert!(ask(
            &ctx,
            &n,
            "notes_write",
            json!({"space": "Cours", "path": "Analyse/Limites", "text": "x", "mode": "create"}),
        )
        .is_err());

        let r = ask(
            &ctx,
            &n,
            "notes_read",
            json!({"space": "Cours", "path": "Limites"}),
        )
        .unwrap();
        assert_eq!(r["path"], "Analyse/Limites.md");
        assert_eq!(r["text"], "# Limites\n\nUne suite #maths converge.\n");
        assert_eq!(r["tags"][0], "maths");

        let e = ask(
            &ctx,
            &n,
            "notes_edit",
            json!({"space": "Cours", "path": "Analyse/Limites.md", "find": "converge", "replace": "diverge"}),
        )
        .unwrap();
        assert_eq!(e["replacements"], 1);
        assert!(ask(
            &ctx,
            &n,
            "notes_edit",
            json!({"space": "Cours", "path": "Analyse/Limites.md", "find": "absent", "replace": "x"}),
        )
        .is_err());
        let v = ask(
            &ctx,
            &n,
            "notes_versions",
            json!({"space": "Cours", "path": "Analyse/Limites.md"}),
        )
        .unwrap();
        assert_eq!(v["versions"].as_array().map(Vec::len), Some(1));

        let a = ask(
            &ctx,
            &n,
            "notes_write",
            json!({"space": "Cours", "path": "Analyse/Limites.md", "text": "- [ ] exercice 3\n", "mode": "append"}),
        )
        .unwrap();
        assert_eq!(a["created"], false);
        let r = ask(
            &ctx,
            &n,
            "notes_read",
            json!({"space": "Cours", "path": "Analyse/Limites.md"}),
        )
        .unwrap();
        assert_eq!(
            r["text"],
            "# Limites\n\nUne suite #maths diverge.\n- [ ] exercice 3\n"
        );

        let f = ask(&ctx, &n, "notes_search", json!({"query": "diverge"})).unwrap();
        assert_eq!(f["results"][0]["path"], "Analyse/Limites.md");
        assert_eq!(f["results"][0]["space"], "Cours");
        let t = ask(&ctx, &n, "notes_tags", json!({})).unwrap();
        assert_eq!(t["tags"][0]["tag"], "maths");

        // Moved and renamed: the links follow.
        ask(
            &ctx,
            &n,
            "notes_write",
            json!({"space": "Cours", "path": "Index", "text": "Voir [[Limites]].\n"}),
        )
        .unwrap();
        let m = ask(
            &ctx,
            &n,
            "notes_move",
            json!({"space": "Cours", "path": "Analyse/Limites.md", "name": "Suites.md", "folder": ""}),
        )
        .unwrap();
        assert_eq!(m["path"], "Suites.md");
        let r = ask(
            &ctx,
            &n,
            "notes_read",
            json!({"space": "Cours", "path": "Index.md"}),
        )
        .unwrap();
        assert_eq!(r["text"], "Voir [[Suites]].\n");
        let b = ask(
            &ctx,
            &n,
            "notes_backlinks",
            json!({"space": "Cours", "path": "Suites"}),
        )
        .unwrap();
        assert_eq!(b["backlinks"][0]["path"], "Index.md");

        let tr = ask(&ctx, &n, "notes_tree", json!({"space": "Cours"})).unwrap();
        let chemins: Vec<&str> = tr["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["path"].as_str())
            .collect();
        assert!(chemins.contains(&"Suites.md") && chemins.contains(&"Analyse"));

        ask(
            &ctx,
            &n,
            "notes_delete",
            json!({"space": "Cours", "path": "Suites.md"}),
        )
        .unwrap();
        assert!(ask(
            &ctx,
            &n,
            "notes_read",
            json!({"space": "Cours", "path": "Suites.md"})
        )
        .is_err());
        assert!(ask(
            &ctx,
            &n,
            "notes_read",
            json!({"space": "Cours", "path": "../x.md"})
        )
        .is_err());
    }

    #[test]
    fn a_spreadsheet_is_written_computed_and_charted() {
        let (ctx, dir, _rt) = super::super::test_ctx();
        let n = notes_in(dir.path());
        ask(&ctx, &n, "notes_create_space", json!({"name": "Travail"})).unwrap();

        let w = ask(
            &ctx,
            &n,
            "sheets_write",
            json!({"path": "Budget", "cells": {"A1": "Item", "B1": "Price", "A2": "Book", "B2": 12, "A3": "Pen", "B3": "=B2*2"}}),
        )
        .unwrap();
        assert_eq!(w["path"], "Budget.sheet");
        assert_eq!(w["values"]["B3"].as_f64(), Some(24.0));

        let r = ask(&ctx, &n, "sheets_read", json!({"path": "Budget.sheet"})).unwrap();
        assert_eq!(r["cells"]["B3"]["input"], "=B2*2");
        assert_eq!(r["cells"]["B3"]["value"].as_f64(), Some(24.0));

        let c = ask(
            &ctx,
            &n,
            "sheets_add_chart",
            json!({"path": "Budget", "name": "Spending", "kind": "bars", "range": "A1:B3"}),
        )
        .unwrap();
        assert_eq!(c["embed"], "![[Budget.sheet#Spending]]");
        let r = ask(&ctx, &n, "sheets_read", json!({"path": "Budget"})).unwrap();
        assert_eq!(r["charts"][0]["name"], "Spending");
        assert!(ask(&ctx, &n, "notes_read", json!({"path": "Budget.sheet"})).is_err());
    }

    #[test]
    fn a_whiteboard_is_drawn_tied_changed_and_cleared() {
        let (ctx, dir, _rt) = super::super::test_ctx();
        let n = notes_in(dir.path());
        ask(&ctx, &n, "notes_create_space", json!({"name": "Travail"})).unwrap();

        let w = ask(
            &ctx,
            &n,
            "boards_add",
            json!({"path": "Plan", "items": [
                {"ref": "idea", "kind": "shape", "x": 0, "y": 0, "text": "Idea"},
                {"ref": "go", "kind": "shape", "shape": "diamond", "x": 300, "y": 0, "fill": "yellow-soft"},
                {"kind": "arrow", "from": "idea", "to": "go", "way": "elbow"},
                {"kind": "sticky", "x": 0, "y": 200, "text": "Ask sales"},
                {"kind": "frame", "x": -40, "y": -40, "w": 600, "h": 400, "text": "Launch"}
            ]}),
        )
        .unwrap();
        assert_eq!(w["path"], "Plan.board");
        assert_eq!(w["created"], true);
        let idea = w["refs"]["idea"].as_u64().unwrap();
        let go = w["refs"]["go"].as_u64().unwrap();

        let r = ask(&ctx, &n, "boards_read", json!({"path": "Plan"})).unwrap();
        let items = r["items"].as_array().unwrap();
        assert_eq!(items.len(), 5);
        // The frame lies under everything.
        assert_eq!(items[0]["kind"], "frame");
        let arrow = items.iter().find(|i| i["kind"] == "connector").unwrap();
        assert_eq!(arrow["from"]["item"].as_u64(), Some(idea));
        assert_eq!(arrow["from"]["x"].as_f64(), Some(1.0));
        assert_eq!(arrow["to"]["item"].as_u64(), Some(go));
        assert_eq!(arrow["to"]["x"].as_f64(), Some(0.0));

        assert!(ask(
            &ctx,
            &n,
            "boards_add",
            json!({"path": "Plan", "items": [{"kind": "shape", "fill": "glitter"}]})
        )
        .is_err());
        assert!(ask(&ctx, &n, "notes_read", json!({"path": "Plan.board"})).is_err());

        let u = ask(
            &ctx,
            &n,
            "boards_update",
            json!({"path": "Plan", "update": [{"id": idea, "fill": "#3A7BD5", "x": 20}], "delete": [go]}),
        )
        .unwrap();
        assert_eq!(u["updated"], 1);
        // The diamond, and the arrow tied to it.
        assert_eq!(u["deleted"], 2);
        let r = ask(&ctx, &n, "boards_read", json!({"path": "Plan.board"})).unwrap();
        let items = r["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        let idee = items
            .iter()
            .find(|i| i["id"].as_u64() == Some(idea))
            .unwrap();
        assert_eq!(idee["fill"], "#3a7bd5");
        assert_eq!(idee["x"].as_f64(), Some(20.0));
    }

    #[test]
    fn the_syntax_is_there_and_the_tools_are_named_for_their_place() {
        let (ctx, _dir, _rt) = super::super::test_ctx();
        let s = call(&ctx, "notes_syntax", &Args::default()).unwrap();
        let texte = s.as_str().unwrap();
        assert!(texte.contains("[!thm]") && texte.contains("SUM"));
        let outils = tools();
        let mut noms: Vec<&str> = outils.iter().map(|t| t.name).collect();
        assert!(noms.iter().all(|n| n.starts_with("notes_")
            || n.starts_with("sheets_")
            || n.starts_with("boards_")));
        noms.sort_unstable();
        noms.dedup();
        assert_eq!(noms.len(), outils.len());
    }
}
