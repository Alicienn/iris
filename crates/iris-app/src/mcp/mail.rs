//! The mail tools: mailboxes, search, lists, conversations, changes and sending.
//!
//! Reads go to the store, the index and the blob cache the window uses; every change
//! to a conversation goes through the workflow (journalled to the server, undoable
//! with Ctrl+Z, shown at once through the bus), and a message leaves through the
//! window's outbox.

use super::time::{day_start_ms, format_ms, parse_day, parse_ms};
use super::Ctx;
use iris_mcp::{Args, Tool, ToolError, ToolResult};
use iris_store::{Account, Filters, FolderRole, ListQuery, Scope, StoredMessage, ThreadRow};
use iris_types::{
    AccountId, Address, BlobId, Flags, FolderId, MessageId, RfcMessageId, ThreadId, WorkflowState,
};
use serde_json::{json, Value};
use std::collections::HashMap;

const DAY_MS: i64 = 86_400_000;
/// What a list or a search gives at most in one call.
const MAX_LIMIT: i64 = 200;

pub(super) fn tools() -> Vec<Tool> {
    vec![
        Tool::read(
            "mail_accounts",
            "Mailboxes and folders",
            "The user's mailboxes: id, email, name, enabled, group, the number of unread \
             conversations in each queue (todo, waiting, done), and each mailbox's folders \
             (path, display name, role: inbox, sent, drafts, trash, junk, archive or other). \
             Use an account's id or email, and a folder's path, in the other mail tools.",
            json!({
                "type": "object",
                "properties": {
                    "include_folders": {
                        "type": "boolean",
                        "description": "List each mailbox's folders (default true)."
                    }
                }
            }),
        ),
        Tool::read(
            "mail_search",
            "Search mail",
            "Searches every mailbox with Iris's own query language, plus structured filters \
             (all combined with AND). Returns conversations (threads), newest first: \
             thread_id, subject, from, last_activity, messages, unread, starred, \
             has_attachment, state (todo/waiting/done), snoozed_until, account, preview; \
             with `truncated` (more than 200 matched: refine) and `understood` (how Iris \
             read the query, in French).\n\
             Query language: free words are searched in subject, sender, recipients and \
             body; \"an exact phrase\" in double quotes; a leading - excludes a term \
             (-invoice, -from:bob). Filters: from:marie, to:paul@example.com, \
             subject:quote (text, a value with spaces in quotes: from:\"Marie Dupont\"); \
             account:me@example.com; is:todo, is:waiting, is:done; is:unread; is:snoozed; \
             has:attachment, has:tracker, has:unsubscribe; newer_than:7d, older_than:30d \
             (units d days, w weeks, m months of 30 days, y years; a bare number is days); \
             sort:relevance (best matches first instead of newest first). French forms \
             also work (de:, sujet:, compte:, etat:). An unknown key is searched as text.\n\
             Prefer the `folder` and `unread` arguments to in: and is:unread (they are \
             exact for conversations). To browse a folder or a queue without searching, \
             use mail_list.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "A query in Iris's language (see the description). Optional when filters are given."},
                    "text": {"type": "string", "description": "Words to find anywhere (subject, sender, recipients, body)."},
                    "from": {"type": "string", "description": "Sender name or address, or part of it."},
                    "to": {"type": "string", "description": "Recipient name or address, or part of it."},
                    "subject": {"type": "string", "description": "Words of the subject."},
                    "account": {"type": "string", "description": "Only this mailbox: its id or email."},
                    "folder": {"type": "string", "description": "Only conversations with a message in this folder: a path from mail_accounts, or a role (inbox, sent, drafts, trash, junk, archive)."},
                    "state": {"type": "string", "enum": ["todo", "waiting", "done"], "description": "Only this queue."},
                    "unread": {"type": "boolean", "description": "true: only conversations with an unread message; false: only fully read ones."},
                    "starred": {"type": "boolean", "description": "true: only starred conversations; false: only unstarred ones."},
                    "has_attachment": {"type": "boolean", "description": "true: only with an attachment; false: only without."},
                    "after": {"type": "string", "description": "Last activity on or after this day (2026-11-06, today, yesterday)."},
                    "before": {"type": "string", "description": "Last activity before this day (the day itself excluded)."},
                    "limit": {"type": "integer", "description": "How many to return (default 25, at most 200)."},
                    "offset": {"type": "integer", "description": "How many results to skip, to page (default 0)."}
                }
            }),
        ),
        Tool::read(
            "mail_list",
            "List a folder or a queue",
            "The conversations of one place, newest first, as the list in Iris shows them: \
             a queue (state todo, waiting or done: everything but the bin and junk, snoozed \
             ones hidden unless include_snoozed), a folder by its path, or a folder role \
             across mailboxes. With a folder or a role and no state, every state is listed. \
             Returns the same thread summaries as mail_search, with `has_more` for paging \
             by offset.",
            json!({
                "type": "object",
                "properties": {
                    "account": {"type": "string", "description": "Only this mailbox: its id or email. Default: all."},
                    "folder": {"type": "string", "description": "A folder path or name (from mail_accounts)."},
                    "role": {"type": "string", "enum": ["inbox", "sent", "drafts", "trash", "junk", "archive"], "description": "A folder role, across mailboxes. Ignored when folder is given."},
                    "state": {"type": "string", "enum": ["todo", "waiting", "done"], "description": "The queue. Default: todo without a folder or role, every state with one."},
                    "unread": {"type": "boolean", "description": "Only conversations with an unread message."},
                    "starred": {"type": "boolean", "description": "Only starred conversations."},
                    "has_attachment": {"type": "boolean", "description": "Only conversations with an attachment."},
                    "include_snoozed": {"type": "boolean", "description": "In a queue, also show snoozed conversations (default false)."},
                    "limit": {"type": "integer", "description": "How many to return (default 25, at most 200)."},
                    "offset": {"type": "integer", "description": "How many to skip, to page (default 0)."}
                }
            }),
        ),
        Tool::read(
            "mail_thread",
            "Read a conversation",
            "One conversation, every message oldest first: message_id, from, to, cc, date \
             (as written by the sender), received, subject, folder, account, read, starred, \
             answered, draft, attachments (index, name, mime, size) and the plain-text body \
             (HTML mail converted to text). A body not downloaded yet is fetched from the \
             server first (fetch: false to skip; the preview is given instead). Reading \
             does not mark anything read: use mail_update for that.",
            json!({
                "type": "object",
                "properties": {
                    "thread_id": {"type": "integer", "description": "The conversation's id (from mail_search or mail_list)."},
                    "max_chars": {"type": "integer", "description": "Longest body given per message, in characters (default 20000); longer ones are cut and marked body_truncated."},
                    "include_html": {"type": "boolean", "description": "Also give each message's sanitized HTML (default false)."},
                    "fetch": {"type": "boolean", "description": "Download bodies not on this computer yet (default true)."}
                },
                "required": ["thread_id"]
            }),
        ),
        Tool::read(
            "mail_message",
            "Read one message",
            "One message by its id, with the same details as in mail_thread and its whole \
             body (unless max_chars is given), plus its thread_id.",
            json!({
                "type": "object",
                "properties": {
                    "message_id": {"type": "integer", "description": "The message's id (from mail_thread)."},
                    "max_chars": {"type": "integer", "description": "Longest body given, in characters (default: all of it)."},
                    "include_html": {"type": "boolean", "description": "Also give the sanitized HTML (default false)."},
                    "fetch": {"type": "boolean", "description": "Download the body if it is not on this computer yet (default true)."}
                },
                "required": ["message_id"]
            }),
        ),
        Tool::read(
            "mail_contacts",
            "Find a correspondent",
            "People the user has had mail from whose address or name contains the text, \
             the most frequent first: email, name, messages (how many received from them), \
             last_seen. Use it to find an address before mail_send.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Part of a name or an address."},
                    "limit": {"type": "integer", "description": "How many (default 20, at most 100)."}
                },
                "required": ["query"]
            }),
        ),
        Tool::write(
            "mail_save_attachment",
            "Save an attachment",
            "Writes one attachment of a message to a file on this computer and returns its \
             path. The destination is a folder (the attachment's own name is used, with \
             \" (2)\" added if taken) or a file path (refused if it exists). The message's \
             body is downloaded first if needed.",
            json!({
                "type": "object",
                "properties": {
                    "message_id": {"type": "integer", "description": "The message's id."},
                    "index": {"type": "integer", "description": "The attachment's index, from mail_thread or mail_message."},
                    "destination": {"type": "string", "description": "An absolute folder or file path."}
                },
                "required": ["message_id", "index", "destination"]
            }),
        ),
        Tool::destructive(
            "mail_update",
            "Change conversations",
            "Changes one or more conversations exactly as the keyboard does in Iris: \
             synced to the server and undoable there with Ctrl+Z (one undo for the whole \
             call). Give the thread ids and any of: read, starred, state (todo/waiting/done), \
             snooze_until (a local time: the conversation leaves its queue until then) or \
             unsnooze, move_to (\"archive\", \"inbox\", \"bin\" or a folder path from \
             mail_accounts). Returns, per thread, what changed and any error.",
            json!({
                "type": "object",
                "properties": {
                    "threads": {"type": "array", "items": {"type": "integer"}, "description": "The conversations' ids."},
                    "read": {"type": "boolean", "description": "Mark every message read (true) or unread (false)."},
                    "starred": {"type": "boolean", "description": "Star or unstar."},
                    "state": {"type": "string", "enum": ["todo", "waiting", "done"], "description": "Move to this queue."},
                    "snooze_until": {"type": "string", "description": "Snooze until this local time, 2026-11-07T09:00 (a bare day is its midnight)."},
                    "unsnooze": {"type": "boolean", "description": "Bring a snoozed conversation back now."},
                    "move_to": {"type": "string", "description": "\"archive\" (also marks it done), \"inbox\" (back to the inbox, to do again), \"bin\" (to the trash folder, never erased), or a folder path."}
                },
                "required": ["threads"]
            }),
        ),
        Tool::write(
            "mail_send",
            "Send a message",
            "Writes and sends a plain-text message from one of the user's mailboxes, through \
             Iris's outbox: it waits there for the user's undo delay (undo_seconds), then \
             leaves and is filed in Sent. The mailbox's signature is added. With \
             reply_to_message it answers that message: it joins its conversation, and \
             subject (Re: …) and recipients (the sender, or its Reply-To) default from it; \
             the original is not quoted. Ask the user before sending anything they did not \
             ask for.",
            json!({
                "type": "object",
                "properties": {
                    "account": {"type": "string", "description": "The mailbox it is sent from: id or email. Default: the replied message's mailbox, else the first enabled one."},
                    "to": {"type": "array", "items": {"type": "string"}, "description": "Recipients: \"a@example.com\" or \"Name <a@example.com>\"."},
                    "cc": {"type": "array", "items": {"type": "string"}, "description": "Copies."},
                    "bcc": {"type": "array", "items": {"type": "string"}, "description": "Blind copies."},
                    "subject": {"type": "string", "description": "The subject. Required unless replying."},
                    "body": {"type": "string", "description": "The text of the message, plain text."},
                    "reply_to_message": {"type": "integer", "description": "The id of the message answered (from mail_thread)."},
                    "attachments": {"type": "array", "items": {"type": "string"}, "description": "Absolute paths of files to attach (25 MB in all at most)."}
                },
                "required": ["body"]
            }),
        ),
    ]
}

pub(super) fn call(ctx: &Ctx, name: &str, a: &Args) -> ToolResult {
    match name {
        "mail_accounts" => accounts(ctx, a),
        "mail_search" => search(ctx, a),
        "mail_list" => list(ctx, a),
        "mail_thread" => thread(ctx, a),
        "mail_message" => message(ctx, a),
        "mail_contacts" => contacts(ctx, a),
        "mail_save_attachment" => save_attachment(ctx, a),
        "mail_update" => update(ctx, a),
        "mail_send" => send(ctx, a),
        autre => Err(ToolError::new(format!("unknown tool: {autre}"))),
    }
}

/// For `iris_overview`: the total of unread messages, and each enabled mailbox with
/// its unread conversations per queue.
pub(super) fn summary(ctx: &Ctx) -> Result<Value, ToolError> {
    let store = &ctx.services.store;
    let now = ctx.now();
    let mut comptes = Vec::new();
    for c in store.accounts()?.into_iter().filter(|c| c.enabled) {
        let [todo, waiting, done] = store.state_counts(&[c.id], Some(now), Filters::default())?;
        comptes.push(json!({
            "id": c.id.get(),
            "email": c.email,
            "unread_todo": todo,
            "unread_waiting": waiting,
            "unread_done": done,
        }));
    }
    Ok(json!({
        "unread_messages": store.unread_count()?,
        "accounts": comptes,
    }))
}

// --- Reading ---

fn accounts(ctx: &Ctx, a: &Args) -> ToolResult {
    let store = &ctx.services.store;
    let avec_dossiers = a.bool_or("include_folders", true)?;
    let now = ctx.now();
    let mut sortie = Vec::new();
    for c in store.accounts()? {
        let [todo, waiting, done] = store.state_counts(&[c.id], Some(now), Filters::default())?;
        let mut v = json!({
            "id": c.id.get(),
            "email": c.email,
            "name": c.display_name,
            "enabled": c.enabled,
            "group": c.group,
            "pinned": c.pinned,
            "unread_conversations": {"todo": todo, "waiting": waiting, "done": done},
        });
        if avec_dossiers {
            let dossiers: Vec<Value> = store
                .folders(c.id)?
                .iter()
                .map(|f| {
                    json!({
                        "path": f.path,
                        "name": iris_store::display_path(&f.path),
                        "role": f.role.as_str(),
                    })
                })
                .collect();
            v["folders"] = Value::Array(dossiers);
        }
        sortie.push(v);
    }
    Ok(json!({ "accounts": sortie }))
}

fn search(ctx: &Ctx, a: &Args) -> ToolResult {
    let mut termes: Vec<String> = Vec::new();
    if let Some(q) = non_empty(a.opt_str("query")?) {
        termes.push(q);
    }
    if let Some(t) = non_empty(a.opt_str("text")?) {
        termes.push(t);
    }
    for cle in ["from", "to", "subject"] {
        if let Some(v) = non_empty(a.opt_str(cle)?) {
            termes.push(filter_term(cle, &v));
        }
    }
    if let Some(s) = opt_state(a, "state")? {
        termes.push(format!("is:{}", s.as_str()));
    }
    match a.opt_bool("has_attachment")? {
        Some(true) => termes.push("has:attachment".into()),
        Some(false) => termes.push("-has:attachment".into()),
        None => {}
    }

    // Days, translated into the language's day filters (a little wider), then applied
    // exactly to the results.
    let now_ms = ctx.now().millis();
    let apres = match non_empty(a.opt_str("after")?) {
        Some(s) => Some(day_start_ms(parse_day(&s)?)?),
        None => None,
    };
    let avant = match non_empty(a.opt_str("before")?) {
        Some(s) => Some(day_start_ms(parse_day(&s)?)?),
        None => None,
    };
    if let Some(debut) = apres {
        let jours = (now_ms - debut + DAY_MS - 1).div_euclid(DAY_MS).max(0);
        termes.push(format!("newer_than:{jours}"));
    }
    if let Some(fin) = avant {
        let jours = (now_ms - fin).div_euclid(DAY_MS);
        if jours > 0 {
            termes.push(format!("older_than:{jours}"));
        }
    }

    let compte = opt_account(ctx, a, "account")?;
    let dossier = non_empty(a.opt_str("folder")?);
    let non_lu = a.opt_bool("unread")?;
    let etoile = a.opt_bool("starred")?;

    if termes.is_empty() {
        if compte.is_none() && dossier.is_none() && non_lu.is_none() && etoile.is_none() {
            return Err(ToolError::new(
                "give a query or at least one filter; to browse a folder or a queue, use mail_list",
            ));
        }
        // Only filters applied after: a filter that lets everything through, for the
        // search to have something to run.
        termes.push("newer_than:36500".into());
    }
    let requete = termes.join(" ");

    let comptes: Vec<AccountId> = compte.iter().map(|c| c.id).collect();
    let etat = iris_viewmodel::search::run(
        &ctx.services.store,
        Some(&ctx.services.index),
        &requete,
        &comptes,
        ctx.now(),
    )?;

    let lieux = Places::load(ctx)?;
    let mut trouves: Vec<&ThreadRow> = Vec::new();
    for r in &etat.results {
        if non_lu.is_some_and(|u| (r.unread_count > 0) != u) {
            continue;
        }
        if etoile.is_some_and(|s| r.flags_union.contains(Flags::FLAGGED) != s) {
            continue;
        }
        let quand = r.last_activity.millis();
        if apres.is_some_and(|d| quand < d) || avant.is_some_and(|f| quand >= f) {
            continue;
        }
        if let Some(d) = &dossier {
            if !lieux.thread_in_folder(ctx, r.id, d)? {
                continue;
            }
        }
        trouves.push(r);
    }

    let (offset, limit) = page(a)?;
    let resultats: Vec<Value> = trouves
        .iter()
        .skip(offset)
        .take(limit)
        .map(|r| lieux.thread_json(r))
        .collect();
    Ok(json!({
        "query": requete,
        "understood": etat.explanation,
        "matched": trouves.len(),
        "truncated": etat.truncated,
        "offset": offset,
        "results": resultats,
    }))
}

fn list(ctx: &Ctx, a: &Args) -> ToolResult {
    let store = &ctx.services.store;
    let comptes: Vec<AccountId> = opt_account(ctx, a, "account")?
        .iter()
        .map(|c| c.id)
        .collect();
    let portee = if let Some(f) = non_empty(a.opt_str("folder")?) {
        Scope::Path(f)
    } else if let Some(r) = non_empty(a.opt_str("role")?) {
        Scope::Role(parse_role(&r)?)
    } else {
        Scope::Queue
    };
    let etats = match opt_state(a, "state")? {
        Some(s) => vec![s],
        None if portee == Scope::Queue => vec![WorkflowState::Todo],
        None => WorkflowState::ALL.to_vec(),
    };
    let filtres = Filters {
        unread: a.bool_or("unread", false)?,
        attachments: a.bool_or("has_attachment", false)?,
        starred: a.bool_or("starred", false)?,
    };
    let caches = portee == Scope::Queue && !a.bool_or("include_snoozed", false)?;
    let (offset, limit) = page(a)?;
    // One more than asked, to know whether there are more.
    let voulu = (offset + limit + 1) as u32;

    let mut lignes: Vec<ThreadRow> = Vec::new();
    for etat in etats {
        let mut q = ListQuery::new(etat, voulu)
            .for_accounts(comptes.clone())
            .filtered(filtres);
        q.scope = portee.clone();
        if caches {
            q = q.hiding_snoozed(ctx.now());
        }
        lignes.extend(store.list_threads(&q)?);
    }
    lignes.sort_by_key(|r| std::cmp::Reverse((r.last_activity.millis(), r.id.get())));

    let lieux = Places::load(ctx)?;
    let resultats: Vec<Value> = lignes
        .iter()
        .skip(offset)
        .take(limit)
        .map(|r| lieux.thread_json(r))
        .collect();
    Ok(json!({
        "offset": offset,
        "has_more": lignes.len() > offset + limit,
        "results": resultats,
    }))
}

fn thread(ctx: &Ctx, a: &Args) -> ToolResult {
    let store = &ctx.services.store;
    let id = ThreadId(a.i64("thread_id")?);
    let ligne = store
        .thread_row(id)?
        .ok_or_else(|| ToolError::new(format!("no conversation {}", id.get())))?;
    let options = BodyOptions {
        max_chars: max_chars(a, 20_000)?,
        include_html: a.bool_or("include_html", false)?,
        fetch: a.bool_or("fetch", true)?,
    };
    let lieux = Places::load(ctx)?;
    let messages: Vec<Value> = store
        .conversation(id)?
        .iter()
        .map(|m| lieux.message_json(ctx, m, &options))
        .collect::<Result<_, _>>()?;
    Ok(json!({
        "thread": lieux.thread_json(&ligne),
        "messages": messages,
    }))
}

fn message(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = MessageId(a.i64("message_id")?);
    let m = ctx
        .services
        .store
        .message_by_id(id)?
        .ok_or_else(|| ToolError::new(format!("no message {}", id.get())))?;
    let options = BodyOptions {
        max_chars: max_chars(a, usize::MAX)?,
        include_html: a.bool_or("include_html", false)?,
        fetch: a.bool_or("fetch", true)?,
    };
    let lieux = Places::load(ctx)?;
    let mut v = lieux.message_json(ctx, &m, &options)?;
    v["thread_id"] = json!(m.thread.get());
    Ok(v)
}

fn contacts(ctx: &Ctx, a: &Args) -> ToolResult {
    let fragment = a.str("query")?;
    let limit = a.opt_i64("limit")?.unwrap_or(20).clamp(1, 100) as u32;
    let gens: Vec<Value> = ctx
        .services
        .store
        .contacts_like(&fragment, limit)?
        .iter()
        .map(|c| {
            json!({
                "email": c.address,
                "name": c.display,
                "as_recipient": c.to_header(),
                "messages": c.seen,
                "last_seen": (c.last_seen.millis() > 0).then(|| format_ms(c.last_seen.millis())),
            })
        })
        .collect();
    Ok(json!({ "contacts": gens }))
}

fn save_attachment(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = MessageId(a.i64("message_id")?);
    let rang = usize::try_from(a.i64("index")?)
        .map_err(|_| ToolError::new("argument \"index\" must be 0 or more"))?;
    let destination = std::path::PathBuf::from(a.str("destination")?.trim());
    if !destination.is_absolute() {
        return Err(ToolError::new("destination must be an absolute path"));
    }
    let m = ctx
        .services
        .store
        .message_by_id(id)?
        .ok_or_else(|| ToolError::new(format!("no message {}", id.get())))?;
    let brut = raw_body(ctx, &m, true)?
        .ok_or_else(|| ToolError::new("the message's body could not be had"))?;
    let analyse = iris_mime::parse(&brut)?;
    let (position, meta) = analyse
        .attachments
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.inline)
        .nth(rang)
        .ok_or_else(|| ToolError::new(format!("message {} has no attachment {rang}", id.get())))?;
    let octets = iris_mime::attachment_bytes(&brut, position)
        .ok_or_else(|| ToolError::new("the attachment could not be read from the message"))?;

    let chemin = if destination.is_dir() {
        free_path(&destination, &safe_name(&meta.filename))
    } else {
        if destination.exists() {
            return Err(ToolError::new(format!(
                "{} already exists: give another name or a folder",
                destination.display()
            )));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        destination
    };
    std::fs::write(&chemin, &octets)?;
    Ok(json!({
        "path": chemin.display().to_string(),
        "name": meta.filename,
        "mime": meta.mime_type,
        "bytes": octets.len(),
    }))
}

// --- Changing ---

/// Where `move_to` sends a conversation.
enum Move {
    Archive,
    Inbox,
    Bin,
    Folder(String),
}

fn update(ctx: &Ctx, a: &Args) -> ToolResult {
    let ids = a.i64_list("threads")?;
    if ids.is_empty() {
        return Err(ToolError::new(
            "missing argument \"threads\" (a list of thread ids)",
        ));
    }
    let lu = a.opt_bool("read")?;
    let etoile = a.opt_bool("starred")?;
    let etat = opt_state(a, "state")?;
    let reveil = match non_empty(a.opt_str("snooze_until")?) {
        Some(s) => Some(iris_types::Timestamp::from_millis(parse_ms(&s)?)),
        None => None,
    };
    let reveiller = a.bool_or("unsnooze", false)?;
    let deplacement = match non_empty(a.opt_str("move_to")?) {
        None => None,
        Some(s) => Some(match s.to_ascii_lowercase().as_str() {
            "archive" => Move::Archive,
            "inbox" => Move::Inbox,
            "bin" | "trash" | "delete" => Move::Bin,
            _ => Move::Folder(resolve_folder(ctx, &s)?),
        }),
    };
    let rien = lu.is_none() && etoile.is_none() && etat.is_none() && reveil.is_none();
    if rien && !reveiller && deplacement.is_none() {
        return Err(ToolError::new(
            "nothing to do: give read, starred, state, snooze_until, unsnooze or move_to",
        ));
    }

    let store = &ctx.services.store;
    let wf = &ctx.services.workflow;
    let now = ctx.now();
    let mut bilans = Vec::new();
    let mut reussis = 0;

    // One action for undo, however many threads, as a multiple selection in the list.
    wf.begin_batch();
    for id in ids {
        let fil = ThreadId(id);
        let mut faits: Vec<String> = Vec::new();
        let mut erreurs: Vec<String> = Vec::new();
        match store.thread_row(fil) {
            Ok(Some(_)) => {}
            Ok(None) => {
                bilans.push(json!({"thread_id": id, "error": format!("no conversation {id}")}));
                continue;
            }
            Err(e) => {
                bilans.push(json!({"thread_id": id, "error": e.to_string()}));
                continue;
            }
        }
        let mut noter = |quoi: &str, r: iris_types::Result<bool>| match r {
            Ok(true) => faits.push(quoi.to_string()),
            Ok(false) => faits.push(format!("{quoi} (already so)")),
            Err(e) => erreurs.push(format!("{quoi}: {e}")),
        };
        if let Some(l) = lu {
            noter(if l { "read" } else { "unread" }, wf.set_read(fil, l, now));
        }
        if let Some(s) = etoile {
            noter(
                if s { "starred" } else { "unstarred" },
                wf.set_flagged(fil, s, now),
            );
        }
        if let Some(s) = etat {
            noter(
                &format!("state {}", s.as_str()),
                wf.set_state(fil, s, now).map(|o| o.changed()),
            );
        }
        if let Some(t) = reveil {
            noter(
                &format!("snoozed until {}", format_ms(t.millis())),
                wf.snooze(fil, t, now),
            );
        }
        if reveiller {
            noter("unsnoozed", wf.unsnooze(fil, now));
        }
        match &deplacement {
            Some(Move::Archive) => noter("archived", wf.archive(fil, now)),
            Some(Move::Inbox) => noter("back to the inbox", wf.back_to_inbox(fil, now)),
            Some(Move::Bin) => noter("moved to the bin", wf.delete(fil, now)),
            Some(Move::Folder(chemin)) => noter(
                &format!("moved to {}", iris_store::display_path(chemin)),
                wf.move_to_folder(fil, chemin, now),
            ),
            None => {}
        }
        if erreurs.is_empty() {
            reussis += 1;
        }
        bilans.push(json!({"thread_id": id, "done": faits, "errors": erreurs}));
    }
    wf.end_batch();

    if reussis == 0 {
        let raisons: Vec<String> = bilans.iter().map(|b| b.to_string()).collect();
        return Err(ToolError::new(format!(
            "nothing changed: {}",
            raisons.join("; ")
        )));
    }
    Ok(json!({
        "results": bilans,
        "note": "Undoable in Iris with Ctrl+Z.",
    }))
}

fn send(ctx: &Ctx, a: &Args) -> ToolResult {
    let envoi = ctx
        .send
        .as_ref()
        .ok_or_else(|| ToolError::new("no mailbox can send: add a mailbox in Iris first"))?;
    let store = &ctx.services.store;

    let parent = match a.opt_i64("reply_to_message")? {
        Some(id) => Some(
            store
                .message_by_id(MessageId(id))?
                .ok_or_else(|| ToolError::new(format!("no message {id}")))?,
        ),
        None => None,
    };

    let compte = match opt_account(ctx, a, "account")? {
        Some(c) => c,
        None => {
            let du_parent = match &parent {
                Some(p) => store.account(p.account)?.filter(|c| c.enabled),
                None => None,
            };
            match du_parent {
                Some(c) => c,
                None => store
                    .accounts()?
                    .into_iter()
                    .find(|c| c.enabled)
                    .ok_or_else(|| ToolError::new("no enabled mailbox to send from"))?,
            }
        }
    };

    let mut to = a.str_list("to")?;
    let cc = a.str_list("cc")?;
    let bcc = a.str_list("bcc")?;
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        if let Some(p) = &parent {
            // Its Reply-To when it has one, else its sender.
            let (_, reponse) = store.message_extras(p.id)?;
            let adresses = addresses_of(&reponse);
            to = if adresses.is_empty() {
                vec![Address {
                    name: (!p.from_name.trim().is_empty()).then(|| p.from_name.clone()),
                    addr: p.from_addr.clone(),
                }
                .to_string()]
            } else {
                adresses.iter().map(Address::to_string).collect()
            };
        }
    }

    let sujet = match (non_empty(a.opt_str("subject")?), &parent) {
        (Some(s), _) => s,
        (None, Some(p)) => re_subject(&p.subject),
        (None, None) => return Err(ToolError::new("missing argument \"subject\"")),
    };

    let mut pieces = Vec::new();
    for chemin in a.str_list("attachments")? {
        let chemin = std::path::PathBuf::from(chemin.trim());
        let contenu = std::fs::read(&chemin)
            .map_err(|e| ToolError::new(format!("{}: {e}", chemin.display())))?;
        pieces.push(iris_smtp::Attachment {
            filename: chemin
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            mime_type: crate::shell::mime_for(&chemin),
            content: contenu,
        });
    }

    let brouillon = iris_sync::Draft {
        account: compte.id,
        to: to.join(", "),
        cc: cc.join(", "),
        bcc: bcc.join(", "),
        subject: sujet,
        body: a.str("body")?,
        attachments: pieces,
    };
    let mut message = envoi.compose_full(&brouillon)?;

    // Into the conversation of the message answered: its id, and the chain before it.
    if let Some(p) = &parent {
        if let Some(rfc) = &p.rfc_message_id {
            let mut vus = std::collections::HashSet::new();
            vus.insert(rfc.clone());
            let chaine: Vec<RfcMessageId> = store
                .thread_messages(p.thread)?
                .into_iter()
                .filter(|m| m.id != p.id)
                .filter_map(|m| m.rfc_message_id)
                .filter(|id| vus.insert(id.clone()))
                .map(RfcMessageId)
                .collect();
            let id = RfcMessageId(rfc.clone());
            message.references = iris_smtp::build_references(&chaine, Some(&id));
            message.in_reply_to = Some(id);
        }
    }

    let bornes = crate::settings::UNDO_SEND_RANGE;
    let secondes = crate::settings::current()
        .undo_send_seconds
        .clamp(*bornes.start(), *bornes.end());
    let resume = json!({
        "queued": true,
        "from": message.from.to_string(),
        "to": message.to.iter().map(Address::to_string).collect::<Vec<_>>(),
        "cc": message.cc.iter().map(Address::to_string).collect::<Vec<_>>(),
        "bcc": message.bcc.iter().map(Address::to_string).collect::<Vec<_>>(),
        "subject": message.subject,
        "attachments": message.attachments.len(),
        "in_reply_to": message.in_reply_to.as_ref().map(|i| i.as_str().to_string()),
        "undo_seconds": secondes,
    });
    envoi.set_delay(std::time::Duration::from_secs(u64::from(secondes)));
    let avis = format!(
        "Your assistant is sending \u{201c}{}\u{201d}.",
        message.subject
    );
    envoi.queue(message)?;
    // Said in the window too: a message leaving should never go unseen.
    let _ = ctx
        .window
        .upgrade_in_event_loop(move |f| f.set_toast(avis.into()));
    Ok(resume)
}

// --- Shared ---

/// How bodies are given.
struct BodyOptions {
    max_chars: usize,
    include_html: bool,
    fetch: bool,
}

/// The mailboxes' addresses and folders, read once per call.
struct Places {
    emails: HashMap<AccountId, String>,
    folders: HashMap<FolderId, (String, FolderRole)>,
}

impl Places {
    fn load(ctx: &Ctx) -> Result<Self, ToolError> {
        let store = &ctx.services.store;
        let mut emails = HashMap::new();
        let mut folders = HashMap::new();
        for c in store.accounts()? {
            for f in store.folders(c.id)? {
                folders.insert(f.id, (f.path, f.role));
            }
            emails.insert(c.id, c.email);
        }
        Ok(Self { emails, folders })
    }

    fn email(&self, id: AccountId) -> &str {
        self.emails.get(&id).map(String::as_str).unwrap_or_default()
    }

    /// Has the conversation a message in this folder (a path, a name or a role)?
    fn thread_in_folder(
        &self,
        ctx: &Ctx,
        thread: ThreadId,
        folder: &str,
    ) -> Result<bool, ToolError> {
        let role = FolderRole::parse(&folder.to_ascii_lowercase());
        Ok(ctx.services.store.thread_messages(thread)?.iter().any(|m| {
            match self.folders.get(&m.folder) {
                Some((chemin, r)) => {
                    (role != FolderRole::Other && *r == role)
                        || iris_store::same_folder(chemin, folder)
                        || iris_store::display_path(chemin)
                            .eq_ignore_ascii_case(iris_store::display_path(folder))
                }
                None => false,
            }
        }))
    }

    fn thread_json(&self, r: &ThreadRow) -> Value {
        json!({
            "thread_id": r.id.get(),
            "subject": r.subject,
            "from": r.from_display,
            "last_activity": format_ms(r.last_activity.millis()),
            "messages": r.message_count,
            "unread": r.unread_count,
            "starred": r.flags_union.contains(Flags::FLAGGED),
            "has_attachment": r.has_attachment(),
            "state": r.state.as_str(),
            "snoozed_until": r.snoozed_until.map(|t| format_ms(t.millis())),
            "account": self.email(r.account),
            "account_id": r.account.get(),
            "preview": r.preview,
        })
    }

    fn message_json(
        &self,
        ctx: &Ctx,
        m: &StoredMessage,
        options: &BodyOptions,
    ) -> Result<Value, ToolError> {
        let (brut, erreur) = match raw_body(ctx, m, options.fetch) {
            Ok(b) => (b, None),
            Err(e) => (None, Some(e.0)),
        };
        let analyse = brut.as_deref().and_then(|b| iris_mime::parse(b).ok());

        let (to, cc): (Vec<String>, Vec<String>) = match &analyse {
            Some(p) => (
                p.to.iter().map(Address::to_string).collect(),
                p.cc.iter().map(Address::to_string).collect(),
            ),
            None => {
                let (copies, _) = ctx.services.store.message_extras(m.id)?;
                (
                    addresses_of(&m.recipients_json)
                        .iter()
                        .map(Address::to_string)
                        .collect(),
                    addresses_of(&copies)
                        .iter()
                        .map(Address::to_string)
                        .collect(),
                )
            }
        };
        let de = Address {
            name: (!m.from_name.trim().is_empty()).then(|| m.from_name.clone()),
            addr: m.from_addr.clone(),
        };
        let dossier = self.folders.get(&m.folder);

        let mut v = json!({
            "message_id": m.id.get(),
            "from": de.to_string(),
            "to": to,
            "cc": cc,
            "subject": m.subject,
            "date": format_ms(m.date.millis()),
            "received": format_ms(m.received.millis()),
            "account": self.email(m.account),
            "folder": dossier.map(|(p, _)| p.as_str()),
            "folder_role": dossier.map(|(_, r)| r.as_str()),
            "read": m.flags.contains(Flags::SEEN),
            "starred": m.flags.contains(Flags::FLAGGED),
            "answered": m.flags.contains(Flags::ANSWERED),
            "draft": m.flags.contains(Flags::DRAFT),
        });

        match &analyse {
            Some(p) => {
                let (texte, coupe) = cut(&p.indexable_text(), options.max_chars);
                v["body"] = json!(texte);
                v["body_truncated"] = json!(coupe);
                v["attachments"] = Value::Array(
                    p.attachments
                        .iter()
                        .filter(|x| !x.inline)
                        .enumerate()
                        .map(|(i, x)| {
                            json!({"index": i, "name": x.filename, "mime": x.mime_type, "size": x.size})
                        })
                        .collect(),
                );
                if options.include_html {
                    v["html"] = json!(p
                        .html_body
                        .as_ref()
                        .filter(|_| !p.html_is_text)
                        .map(|h| h.html.as_str()));
                }
            }
            None => {
                v["body"] = Value::Null;
                v["preview"] = json!(m.preview);
                let pourquoi = erreur.unwrap_or_else(|| "the body is not downloaded yet".into());
                v["body_error"] = json!(pourquoi);
                v["attachments"] = Value::Array(
                    ctx.services
                        .store
                        .visible_attachments(m.id)?
                        .iter()
                        .enumerate()
                        .map(|(i, x)| {
                            json!({"index": i, "name": x.meta.filename, "mime": x.meta.mime_type, "size": x.meta.size})
                        })
                        .collect(),
                );
            }
        }
        Ok(v)
    }
}

/// A message as received, from the cache, or downloaded when `fetch` allows.
fn raw_body(ctx: &Ctx, m: &StoredMessage, fetch: bool) -> Result<Option<Vec<u8>>, ToolError> {
    if let Some(id) = m.body_blob.as_deref().and_then(BlobId::from_hex) {
        if let Some(brut) = ctx.services.blobs.get(id)? {
            return Ok(Some(brut));
        }
    }
    if !fetch {
        return Ok(None);
    }
    // A mailbox that does not answer must not hold the assistant until it gives up.
    let recu = ctx
        .runtime
        .block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_secs(25),
                ctx.services.engine.fetch_body(m.id),
            )
            .await
        })
        .map_err(|_| {
            ToolError::new("The mailbox did not send the message in time. Try again.")
        })??;
    Ok(ctx.services.blobs.get(recu.blob)?)
}

/// A mailbox by its id or email.
fn find_account(ctx: &Ctx, spec: &str) -> Result<Account, ToolError> {
    let spec = spec.trim();
    let id = spec.parse::<i64>().ok();
    ctx.services
        .store
        .accounts()?
        .into_iter()
        .find(|c| Some(c.id.get()) == id || c.email.eq_ignore_ascii_case(spec))
        .ok_or_else(|| {
            ToolError::new(format!(
                "no mailbox \"{spec}\": give an id or an email from mail_accounts"
            ))
        })
}

fn opt_account(ctx: &Ctx, a: &Args, key: &str) -> Result<Option<Account>, ToolError> {
    match non_empty(a.opt_str(key)?) {
        Some(s) => find_account(ctx, &s).map(Some),
        None => Ok(None),
    }
}

fn opt_state(a: &Args, key: &str) -> Result<Option<WorkflowState>, ToolError> {
    match non_empty(a.opt_str(key)?) {
        Some(s) => WorkflowState::parse(&s.to_ascii_lowercase())
            .map(Some)
            .ok_or_else(|| ToolError::new(format!("\"{key}\" must be todo, waiting or done"))),
        None => Ok(None),
    }
}

fn parse_role(s: &str) -> Result<FolderRole, ToolError> {
    match FolderRole::parse(&s.trim().to_ascii_lowercase()) {
        FolderRole::Other => Err(ToolError::new(
            "\"role\" must be inbox, sent, drafts, trash, junk or archive",
        )),
        r => Ok(r),
    }
}

/// A folder `move_to` names, as Iris knows it: its path or its shown name.
fn resolve_folder(ctx: &Ctx, s: &str) -> Result<String, ToolError> {
    let s = s.trim();
    ctx.services
        .store
        .unified_folders()?
        .into_iter()
        .find(|f| {
            let nom = iris_store::display_path(&f.path);
            f.path == s
                || iris_store::same_folder(&f.path, s)
                || nom.eq_ignore_ascii_case(iris_store::display_path(s))
        })
        .map(|f| f.path)
        .ok_or_else(|| {
            ToolError::new(format!(
                "no folder \"{s}\": use archive, inbox, bin or a folder path from mail_accounts"
            ))
        })
}

/// `offset` and `limit` of a page.
fn page(a: &Args) -> Result<(usize, usize), ToolError> {
    let limit = a.opt_i64("limit")?.unwrap_or(25).clamp(1, MAX_LIMIT) as usize;
    let offset = a.opt_i64("offset")?.unwrap_or(0).clamp(0, 10_000) as usize;
    Ok((offset, limit))
}

fn max_chars(a: &Args, default: usize) -> Result<usize, ToolError> {
    Ok(match a.opt_i64("max_chars")? {
        Some(n) => usize::try_from(n.max(0)).unwrap_or(default),
        None => default,
    })
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// `from:marie`, or `from:"Marie Dupont"` when the value has spaces.
fn filter_term(key: &str, value: &str) -> String {
    let v = value.replace('"', "");
    if v.contains(char::is_whitespace) {
        format!("{key}:\"{v}\"")
    } else {
        format!("{key}:{v}")
    }
}

/// Addresses kept as JSON (`[{"name":…,"addr":…}]`); nothing when they do not read.
fn addresses_of(json: &str) -> Vec<Address> {
    serde_json::from_str(json).unwrap_or_default()
}

fn re_subject(subject: &str) -> String {
    let s = subject.trim();
    if s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("re:")) {
        s.to_string()
    } else {
        format!("Re: {s}")
    }
}

/// At most `max` characters, and whether some were left out.
fn cut(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((i, _)) => (format!("{}…", &text[..i]), true),
        None => (text.to_string(), false),
    }
}

/// A file name that cannot leave its folder.
fn safe_name(name: &str) -> String {
    let nom: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let nom = nom.trim().trim_matches('.').to_string();
    if nom.is_empty() {
        "attachment".into()
    } else {
        nom
    }
}

/// `name`, or `name (2)`… when taken.
fn free_path(folder: &std::path::Path, name: &str) -> std::path::PathBuf {
    let candidat = folder.join(name);
    if !candidat.exists() {
        return candidat;
    }
    let chemin = std::path::Path::new(name);
    let tronc = chemin
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string());
    let extension = chemin.extension().map(|e| e.to_string_lossy().to_string());
    for n in 2..10_000 {
        let essai = match &extension {
            Some(ext) => folder.join(format!("{tronc} ({n}).{ext}")),
            None => folder.join(format!("{tronc} ({n})")),
        };
        if !essai.exists() {
            return essai;
        }
    }
    candidat
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_named_once_and_described_by_an_object_schema() {
        let outils = tools();
        let mut noms: Vec<&str> = outils.iter().map(|t| t.name).collect();
        assert!(noms.iter().all(|n| n.starts_with("mail_")));
        noms.sort_unstable();
        let avant = noms.len();
        noms.dedup();
        assert_eq!(noms.len(), avant);
        for t in &outils {
            assert_eq!(t.input_schema["type"], "object", "{}", t.name);
            assert!(t.input_schema["properties"].is_object(), "{}", t.name);
        }
    }

    #[test]
    fn an_empty_store_has_no_mailbox() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        let v = call(&ctx, "mail_accounts", &Args::default()).unwrap();
        assert_eq!(v["accounts"], json!([]));
        let s = summary(&ctx).unwrap();
        assert_eq!(s["accounts"], json!([]));
    }

    #[test]
    fn an_unknown_thread_is_an_error_not_a_panic() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        let a = Args::new(json!({"threads": [424242], "read": true}));
        assert!(call(&ctx, "mail_update", &a).is_err());
        let a = Args::new(json!({"thread_id": 424242}));
        assert!(call(&ctx, "mail_thread", &a).is_err());
        assert!(call(&ctx, "mail_nothing", &Args::default()).is_err());
    }

    #[test]
    fn sending_without_an_outbox_says_so() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        let a = Args::new(json!({"to": ["a@example.com"], "subject": "Hi", "body": "Hello"}));
        let e = call(&ctx, "mail_send", &a).unwrap_err();
        assert!(e.0.contains("no mailbox can send"));
    }

    #[test]
    fn small_helpers_keep_their_word() {
        assert_eq!(filter_term("from", "Marie Dupont"), "from:\"Marie Dupont\"");
        assert_eq!(filter_term("from", "marie"), "from:marie");
        assert_eq!(re_subject("Re: Quote"), "Re: Quote");
        assert_eq!(re_subject("Quote"), "Re: Quote");
        assert_eq!(cut("abcdef", 3), ("abc…".to_string(), true));
        assert_eq!(cut("abc", 3), ("abc".to_string(), false));
        assert_eq!(safe_name("../a/b.pdf"), "_a_b.pdf");
        assert_eq!(safe_name(""), "attachment");
    }
}
