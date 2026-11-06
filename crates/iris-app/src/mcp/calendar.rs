//! The calendar tools: calendars, occurrences over a range, a day or a week, one
//! event with its notes and subtasks, and making, changing, moving and deleting events.
//!
//! Everything goes through the store as the calendar view does: the same calendars
//! are writable, a repeating event is unrolled the same way, and an event kept with a
//! server is marked to be sent there (the CalDAV sync pushes it within a minute).

use super::time::{day_start_ms, format_ms, parse_day, parse_ms, today};
use super::Ctx;
use chrono::{Datelike, Duration, Local, NaiveDate, Utc, Weekday};
use iris_calendar::{layout, Occurrence};
use iris_mcp::{Args, Tool, ToolError, ToolResult};
use iris_store::{NewEvent, StoredCalendar, StoredEvent};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
const DEFAULT_LIMIT: i64 = 200;
const MAX_LIMIT: i64 = 1_000;

/// Keeps two events made in the same millisecond apart.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn tools() -> Vec<Tool> {
    let occurrence_start = json!({
        "type": "string",
        "description": "Which occurrence of a repeating event: its occurrence_start exactly as \
                        calendar_events / calendar_day / calendar_week return it."
    });
    vec![
        Tool::read(
            "calendar_calendars",
            "List calendars",
            "Every calendar: id, name, colour (#rrggbb), writable (new events can go in it), \
             events_editable (its events can be changed or deleted), visible (hidden calendars' \
             events are left out of calendar_events/day/week), account (the CalDAV account it \
             syncs with, if any), subscription (a read-from-a-link calendar), default (where \
             calendar_create_event puts an event when no calendar_id is given).",
            json!({"type": "object", "properties": {}}),
        ),
        Tool::read(
            "calendar_events",
            "Events in a range",
            "The occurrences (repeating events unrolled) of the visible calendars between `from` \
             and `to`, in order. Times are local ISO (2026-11-06T14:30); a bare day as `to` \
             includes that whole day. Defaults: from today, for 7 days. Each occurrence: \
             event_id, calendar_id, calendar, uid, title, start, end (all-day events give days, \
             end being the last day, inclusive), all_day, location, description, recurring (the \
             RRULE, or null), changed_occurrence, occurrence_start (identifies this occurrence \
             for calendar_set_note / calendar_delete_event / calendar_event), meeting_link, \
             colour, note (of this occurrence), reminder_minutes, subtasks (how many).",
            json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string", "description": "Start, local time or day. Default today."},
                    "to": {"type": "string", "description": "End (exclusive time, or inclusive day). Default from + 7 days."},
                    "calendar_id": {"type": "array", "items": {"type": "integer"}, "description": "Only these calendars (one id or a list)."},
                    "query": {"type": "string", "description": "Only occurrences whose title, location, description or note contain this text (case-insensitive)."},
                    "limit": {"type": "integer", "description": "At most this many (default 200, at most 1000)."}
                }
            }),
        ),
        Tool::read(
            "calendar_day",
            "One day's events",
            "The occurrences of one day (default today), all-day ones first, then by start. Same \
             fields as calendar_events.",
            json!({
                "type": "object",
                "properties": {
                    "day": {"type": "string", "description": "2026-11-06, today, tomorrow or yesterday."}
                }
            }),
        ),
        Tool::read(
            "calendar_week",
            "One week's events",
            "The 7 days of the week holding `day` (default today), each with its occurrences \
             (same fields as calendar_events). An event over several days is listed on each.",
            json!({
                "type": "object",
                "properties": {
                    "day": {"type": "string", "description": "Any day of the week wanted. Default today."},
                    "week_starts": {"type": "string", "description": "monday (default), sunday or saturday."}
                }
            }),
        ),
        Tool::read(
            "calendar_event",
            "One event in full",
            "One event by event_id, with everything: the occurrence fields of calendar_events, \
             plus subtasks (id, title, done: tasks made for this occurrence, created with \
             tasks_create and an event_id), series_start (a repeating event's first occurrence), \
             excluded_occurrences, timezone, cancelled, editable. For a repeating event, give \
             occurrence_start to read that occurrence's note and subtasks; without it, the \
             first occurrence is read.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "The event_id."},
                    "occurrence_start": occurrence_start.clone()
                },
                "required": ["id"]
            }),
        ),
        Tool::write(
            "calendar_create_event",
            "Create an event",
            "Adds an event and returns it (as calendar_event does). Timed: start and end are \
             local times (2026-11-06T14:30), end defaults to start + 1 hour. All day: \
             all_day=true, start and end are days, end inclusive (defaults to start). The \
             calendar must be writable (calendar_calendars); default: the user's own. rrule is \
             an RFC 5545 rule without the RRULE: prefix (FREQ=WEEKLY;BYDAY=MO, \
             FREQ=DAILY;COUNT=5, FREQ=MONTHLY;UNTIL=20271231T000000Z). colour is #rrggbb and \
             overrides the calendar's. note is a private note kept in Iris (for a repeating \
             event, on its first occurrence). To add subtasks, call tasks_create with the \
             returned event_id.",
            json!({
                "type": "object",
                "properties": {
                    "calendar_id": {"type": "integer"},
                    "title": {"type": "string"},
                    "start": {"type": "string"},
                    "end": {"type": "string"},
                    "all_day": {"type": "boolean"},
                    "location": {"type": "string"},
                    "description": {"type": "string", "description": "Synced with the event (invitees on a shared calendar see it)."},
                    "rrule": {"type": "string"},
                    "reminder_minutes": {"type": "integer", "description": "Minutes before the start; 0 at the start."},
                    "meeting_link": {"type": "string", "description": "A video call address."},
                    "colour": {"type": "string", "description": "#rrggbb"},
                    "note": {"type": "string"}
                },
                "required": ["title", "start"]
            }),
        ),
        Tool::write(
            "calendar_update_event",
            "Change or move an event",
            "Changes the fields given and leaves the others; null or \"\" clears an optional \
             field (location, description, rrule, reminder_minutes, meeting_link, colour). \
             Moving is changing start (the length is kept when end is not given) and/or end; \
             all-day events take days, end inclusive. calendar_id moves it to another writable \
             calendar, with its note, colour and link. A repeating event is one series: its \
             start/end are those of its first occurrence (series_start in calendar_event), and \
             changing them moves every occurrence, so it is refused unless whole_series=true. \
             To move a single occurrence, delete it (calendar_delete_event with \
             occurrence_start) and create a separate event. Returns the event.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "The event_id."},
                    "title": {"type": "string"},
                    "start": {"type": "string"},
                    "end": {"type": "string"},
                    "all_day": {"type": "boolean"},
                    "location": {"type": ["string", "null"]},
                    "description": {"type": ["string", "null"]},
                    "rrule": {"type": ["string", "null"]},
                    "reminder_minutes": {"type": ["integer", "null"]},
                    "calendar_id": {"type": "integer"},
                    "meeting_link": {"type": ["string", "null"]},
                    "colour": {"type": ["string", "null"]},
                    "whole_series": {"type": "boolean", "description": "Needed to change the times of a repeating event."}
                },
                "required": ["id"]
            }),
        ),
        Tool::destructive(
            "calendar_delete_event",
            "Delete an event",
            "Deletes an event (a repeating one: the whole series, with its notes). With \
             occurrence_start, only that occurrence of a repeating event is taken out. A \
             changed occurrence (changed_occurrence=true) is taken out alone. The deletion \
             syncs to the calendar's server.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "The event_id."},
                    "occurrence_start": occurrence_start.clone()
                },
                "required": ["id"]
            }),
        ),
        Tool::write(
            "calendar_set_note",
            "Write an event's note",
            "Sets the private note Iris keeps on an event (not synced, not seen by invitees); an \
             empty note clears it. A repeating event has one note per occurrence: \
             occurrence_start is then required.",
            json!({
                "type": "object",
                "properties": {
                    "id": {"type": "integer", "description": "The event_id."},
                    "occurrence_start": occurrence_start,
                    "note": {"type": "string"}
                },
                "required": ["id", "note"]
            }),
        ),
    ]
}

pub(super) fn call(ctx: &Ctx, name: &str, a: &Args) -> ToolResult {
    match name {
        "calendar_calendars" => calendars(ctx),
        "calendar_events" => events(ctx, a),
        "calendar_day" => day(ctx, a),
        "calendar_week" => week(ctx, a),
        "calendar_event" => event(ctx, a),
        "calendar_create_event" => create_event(ctx, a),
        "calendar_update_event" => update_event(ctx, a),
        "calendar_delete_event" => delete_event(ctx, a),
        "calendar_set_note" => set_note(ctx, a),
        _ => Err(ToolError::new(format!("unknown tool: {name}"))),
    }
}

/// A day's occurrences, short: for the overview.
pub(super) fn day_summary(ctx: &Ctx, day: NaiveDate) -> Result<Value, ToolError> {
    let book = Book::load(ctx)?;
    let (stored, mut occ) = between(ctx, day_start_ms(day)?, next_day_ms(day)?)?;
    occ.sort_by_key(|o| (!o.all_day, o.start));
    Ok(Value::Array(
        occ.iter()
            .map(|o| {
                let s = &stored[o.event];
                json!({
                    "id": s.id,
                    "title": s.event.summary,
                    "start": out_start(o.start, o.all_day),
                    "end": out_end(o.start, o.end, o.all_day),
                    "all_day": o.all_day,
                    "location": s.event.location,
                    "link": book.link(s),
                })
            })
            .collect(),
    ))
}

// --- Reading -----------------------------------------------------------------------

/// What every occurrence is shown with, read once per call.
struct Book {
    calendars: HashMap<i64, StoredCalendar>,
    colours: HashMap<(i64, String), String>,
    links: HashMap<(i64, String), String>,
}

impl Book {
    fn load(ctx: &Ctx) -> Result<Self, ToolError> {
        let store = &ctx.services.store;
        Ok(Self {
            calendars: store.calendars()?.into_iter().map(|c| (c.id, c)).collect(),
            colours: store.event_colors()?,
            links: store.event_links()?,
        })
    }

    fn calendar_name(&self, id: i64) -> &str {
        self.calendars
            .get(&id)
            .map(|c| c.name.as_str())
            .unwrap_or_default()
    }

    /// Its own colour, else its calendar's.
    fn colour(&self, s: &StoredEvent) -> String {
        self.colours
            .get(&(s.calendar_id, s.event.uid.clone()))
            .or_else(|| self.calendars.get(&s.calendar_id).map(|c| &c.color))
            .cloned()
            .unwrap_or_default()
    }

    fn link(&self, s: &StoredEvent) -> Option<String> {
        crate::calendar::lien_de_visio(&self.links, s.calendar_id, &s.event)
    }
}

fn calendars(ctx: &Ctx) -> ToolResult {
    let defaut = crate::calendar::own_calendar(&ctx.services).map(|c| c.id);
    let liste: Vec<Value> = ctx
        .services
        .store
        .calendars()?
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.name,
                "colour": c.color,
                "writable": c.is_writable(),
                "events_editable": c.events_editable(),
                "visible": c.visible,
                "account": c.account_id.map(|a| crate::caldav::account_name(&ctx.services, a)),
                "subscription": c.is_subscription(),
                "local": c.is_local(),
                "default": defaut == Some(c.id),
                "last_error": c.last_error,
            })
        })
        .collect();
    Ok(json!({ "calendars": liste }))
}

fn events(ctx: &Ctx, a: &Args) -> ToolResult {
    let from = match a.opt_str("from")? {
        Some(t) => parse_ms(&t)?,
        None => day_start_ms(today())?,
    };
    let to = match a.opt_str("to")? {
        Some(t) => range_end(&t)?,
        None => from + 7 * DAY_MS,
    };
    if to <= from {
        return Err(ToolError::new("\"to\" must come after \"from\""));
    }
    let mut ids = a.i64_list("calendar_id")?;
    ids.extend(a.i64_list("calendar_ids")?);
    let query = a
        .opt_str("query")?
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    let limit = a
        .opt_i64("limit")?
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT) as usize;

    let book = Book::load(ctx)?;
    let (stored, occ) = between(ctx, from, to)?;
    let mut sortie = Vec::new();
    let mut tronque = false;
    for o in &occ {
        let s = &stored[o.event];
        if !ids.is_empty() && !ids.contains(&s.calendar_id) {
            continue;
        }
        let v = occurrence_json(ctx, &book, s, o);
        if let Some(q) = &query {
            let trouve = ["title", "location", "description", "note"]
                .iter()
                .any(|k| {
                    v[*k]
                        .as_str()
                        .is_some_and(|t| t.to_lowercase().contains(q.as_str()))
                });
            if !trouve {
                continue;
            }
        }
        if sortie.len() == limit {
            tronque = true;
            break;
        }
        sortie.push(v);
    }
    Ok(json!({
        "from": format_ms(from),
        "to": format_ms(to),
        "count": sortie.len(),
        "truncated": tronque,
        "occurrences": sortie,
    }))
}

fn day(ctx: &Ctx, a: &Args) -> ToolResult {
    let jour = match a.opt_str("day")? {
        Some(t) => parse_day(&t)?,
        None => today(),
    };
    let book = Book::load(ctx)?;
    let (stored, mut occ) = between(ctx, day_start_ms(jour)?, next_day_ms(jour)?)?;
    occ.sort_by_key(|o| (!o.all_day, o.start));
    let liste: Vec<Value> = occ
        .iter()
        .map(|o| occurrence_json(ctx, &book, &stored[o.event], o))
        .collect();
    Ok(json!({
        "day": jour.to_string(),
        "weekday": jour.format("%A").to_string(),
        "occurrences": liste,
    }))
}

fn week(ctx: &Ctx, a: &Args) -> ToolResult {
    let jour = match a.opt_str("day")? {
        Some(t) => parse_day(&t)?,
        None => today(),
    };
    let premier: Weekday = match a.opt_str("week_starts")? {
        Some(t) => t.trim().parse().map_err(|_| {
            ToolError::new(format!(
                "\"{t}\" is not a weekday (write monday, sunday or saturday)"
            ))
        })?,
        None => Weekday::Mon,
    };
    let recul = (7 + i64::from(jour.weekday().num_days_from_monday())
        - i64::from(premier.num_days_from_monday()))
        % 7;
    let debut = jour - Duration::days(recul);
    let fin = debut + Duration::days(7);

    let book = Book::load(ctx)?;
    let (stored, mut occ) = between(ctx, day_start_ms(debut)?, day_start_ms(fin)?)?;
    occ.sort_by_key(|o| (!o.all_day, o.start));
    let mut jours: Vec<(NaiveDate, Vec<Value>)> = (0..7)
        .map(|i| (debut + Duration::days(i), Vec::new()))
        .collect();
    for o in &occ {
        let (premier_jour, dernier_jour) = layout::days_covered(o, &Local);
        let v = occurrence_json(ctx, &book, &stored[o.event], o);
        for (d, liste) in &mut jours {
            if premier_jour <= *d && *d <= dernier_jour {
                liste.push(v.clone());
            }
        }
    }
    Ok(json!({
        "week_start": debut.to_string(),
        "days": jours
            .into_iter()
            .map(|(d, liste)| json!({
                "day": d.to_string(),
                "weekday": d.format("%A").to_string(),
                "occurrences": liste,
            }))
            .collect::<Vec<_>>(),
    }))
}

fn event(ctx: &Ctx, a: &Args) -> ToolResult {
    let s = load_event(ctx, a.i64("id")?)?;
    let occurrence = parse_occurrence(a, &s)?;
    event_json(ctx, &s, occurrence)
}

/// The occurrences touching `[from, to[`, by when they start, with the events they
/// come from (`Occurrence::event` indexes them).
fn between(
    ctx: &Ctx,
    from: i64,
    to: i64,
) -> Result<(Vec<StoredEvent>, Vec<Occurrence>), ToolError> {
    // A day of margin on each side: an all-day event sits at midnight UTC.
    let stored = ctx
        .services
        .store
        .events_for_range(from - DAY_MS, to + DAY_MS)?;
    let domaine: Vec<iris_calendar::Event> = stored
        .iter()
        .map(|s| crate::calendar::to_domain(&s.event))
        .collect();
    let mut occ: Vec<Occurrence> =
        iris_calendar::recur::occurrences(&domaine, from - DAY_MS, to + DAY_MS)
            .into_iter()
            .filter(|o| {
                let (a, b) = span(o);
                overlaps(a, b, from, to)
            })
            .collect();
    occ.sort_by_key(|o| (span(o).0, !o.all_day, o.start));
    Ok((stored, occ))
}

/// When an occurrence lies here: an all-day one from the local midnight of its first
/// day to that of the day after its last (its days are read in UTC, where it sits).
fn span(o: &Occurrence) -> (i64, i64) {
    if o.all_day {
        let (premier, dernier) = layout::days_covered(o, &Local);
        (
            layout::local_midnight(premier, &Local),
            layout::local_midnight(dernier + Duration::days(1), &Local),
        )
    } else {
        (o.start, o.end)
    }
}

/// An empty event counts as an instant: it overlaps when it falls inside.
fn overlaps(start: i64, end: i64, from: i64, to: i64) -> bool {
    start < to && (end > from || (end == start && start >= from))
}

/// Where the note and subtasks of an occurrence are kept: its start for a repeating
/// event, 0 for one that does not repeat (so moving it keeps them).
fn note_key(e: &NewEvent, occurrence_start: i64) -> i64 {
    if e.rrule.is_some() {
        occurrence_start
    } else {
        0
    }
}

fn occurrence_json(ctx: &Ctx, book: &Book, s: &StoredEvent, o: &Occurrence) -> Value {
    let store = &ctx.services.store;
    let e = &s.event;
    let cle = note_key(e, o.start);
    json!({
        "event_id": s.id,
        "calendar_id": s.calendar_id,
        "calendar": book.calendar_name(s.calendar_id),
        "uid": e.uid,
        "title": e.summary,
        "start": out_start(o.start, o.all_day),
        "end": out_end(o.start, o.end, o.all_day),
        "all_day": o.all_day,
        "location": e.location,
        "description": e.description,
        "recurring": e.rrule,
        "changed_occurrence": e.recurrence_id.is_some(),
        "occurrence_start": format_ms(o.start),
        "meeting_link": book.link(s),
        "colour": book.colour(s),
        "note": store.event_note(s.calendar_id, &e.uid, cle).unwrap_or_default(),
        "reminder_minutes": e.reminder_minutes,
        "subtasks": store.tasks_for_event(&e.uid, cle).map(|t| t.len()).unwrap_or(0),
    })
}

/// One event in full, at one occurrence (its first by default).
fn event_json(ctx: &Ctx, s: &StoredEvent, occurrence: Option<i64>) -> ToolResult {
    let store = &ctx.services.store;
    let book = Book::load(ctx)?;
    let e = &s.event;
    let debut = occurrence.unwrap_or(e.start_ms);
    let o = Occurrence {
        event: 0,
        start: debut,
        end: debut + (e.end_ms - e.start_ms).max(0),
        all_day: e.all_day,
    };
    let mut v = occurrence_json(ctx, &book, s, &o);
    let taches: Vec<Value> = store
        .tasks_for_event(&e.uid, note_key(e, debut))?
        .iter()
        .map(|t| json!({ "id": t.id, "title": t.task.title, "done": t.is_done() }))
        .collect();
    v["subtasks"] = Value::Array(taches);
    v["series_start"] = match e.rrule {
        Some(_) => Value::String(out_start(e.start_ms, e.all_day)),
        None => Value::Null,
    };
    v["excluded_occurrences"] = e.exdates.iter().map(|ms| format_ms(*ms)).collect();
    v["timezone"] = json!(e.tzid);
    v["cancelled"] = json!(e.cancelled);
    v["editable"] = json!(book
        .calendars
        .get(&s.calendar_id)
        .is_some_and(StoredCalendar::events_editable));
    Ok(v)
}

fn load_event(ctx: &Ctx, id: i64) -> Result<StoredEvent, ToolError> {
    ctx.services
        .store
        .event(id)?
        .ok_or_else(|| ToolError::new(format!("no event with id {id}")))
}

/// The `occurrence_start` argument, checked against the event. A single event has
/// one occurrence, its own: any value reads as it.
fn parse_occurrence(a: &Args, s: &StoredEvent) -> Result<Option<i64>, ToolError> {
    let e = &s.event;
    let ms = match a.get("occurrence_start") {
        None => return Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .ok_or_else(|| ToolError::new("\"occurrence_start\" must be a time"))?,
        Some(Value::String(t)) if t.trim().is_empty() => return Ok(None),
        Some(Value::String(t)) => {
            let t = t.trim();
            if e.all_day && !t.contains(':') {
                utc_midnight(parse_day(t)?)
            } else {
                parse_ms(t)?
            }
        }
        Some(_) => return Err(ToolError::new("\"occurrence_start\" must be a time")),
    };
    if e.rrule.is_none() || e.recurrence_id.is_some() {
        return Ok(Some(e.start_ms));
    }
    let domaine = [crate::calendar::to_domain(e)];
    let existe = ms == e.start_ms
        || iris_calendar::recur::occurrences(&domaine, ms, ms + 1)
            .iter()
            .any(|o| o.start == ms);
    if existe {
        Ok(Some(ms))
    } else {
        Err(ToolError::new(format!(
            "event {} has no occurrence starting {}: use occurrence_start as calendar_events \
             returns it",
            s.id,
            format_ms(ms)
        )))
    }
}

// --- Times -------------------------------------------------------------------------

fn utc_midnight(d: NaiveDate) -> i64 {
    layout::local_midnight(d, &Utc)
}

fn utc_day(ms: i64) -> NaiveDate {
    layout::local_date(ms, &Utc)
}

fn next_day_ms(d: NaiveDate) -> Result<i64, ToolError> {
    day_start_ms(d + Duration::days(1))
}

/// The end of a range: a bare day includes all of it.
fn range_end(t: &str) -> Result<i64, ToolError> {
    let t = t.trim();
    if t.contains(':') {
        parse_ms(t)
    } else {
        next_day_ms(parse_day(t)?)
    }
}

fn out_start(start: i64, all_day: bool) -> String {
    if all_day {
        utc_day(start).to_string()
    } else {
        format_ms(start)
    }
}

/// An all-day event's end is its last day, not the exclusive day after.
fn out_end(start: i64, end: i64, all_day: bool) -> String {
    if all_day {
        utc_day((end - 1).max(start)).to_string()
    } else {
        format_ms(end)
    }
}

/// The first and last day of an event: in UTC for an all-day one, here otherwise.
fn days_of(e: &NewEvent) -> (NaiveDate, NaiveDate) {
    let o = Occurrence {
        event: 0,
        start: e.start_ms,
        end: e.end_ms,
        all_day: e.all_day,
    };
    layout::days_covered(&o, &Local)
}

/// An all-day event's instants: midnight UTC of its first day, to that of the day
/// after its last.
fn all_day_span(first: NaiveDate, last: NaiveDate) -> Result<(i64, i64), ToolError> {
    if last < first {
        return Err(ToolError::new("the event ends before it starts"));
    }
    Ok((utc_midnight(first), utc_midnight(last + Duration::days(1))))
}

fn local_tz() -> Option<String> {
    iana_time_zone::get_timezone().ok()
}

// --- Values ------------------------------------------------------------------------

fn parse_colour(t: &str) -> Result<String, ToolError> {
    let h = t.trim().trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(format!("#{}", h.to_ascii_lowercase()))
    } else {
        Err(ToolError::new(format!(
            "\"{t}\" is not a colour (write #rrggbb, e.g. #5b8def)"
        )))
    }
}

/// An RFC 5545 rule, upper case, without its `RRULE:` prefix; empty is none.
fn parse_rrule(t: &str) -> Result<Option<String>, ToolError> {
    let r = t.trim().to_ascii_uppercase();
    let r = r.strip_prefix("RRULE:").unwrap_or(&r).trim().to_string();
    if r.is_empty() {
        return Ok(None);
    }
    let connue = matches!(
        r.split(';').find_map(|p| p.trim().strip_prefix("FREQ=")),
        Some("DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY")
    );
    if connue {
        Ok(Some(r))
    } else {
        Err(ToolError::new(format!(
            "\"{t}\" is not a rule Iris repeats (write FREQ=DAILY, FREQ=WEEKLY;BYDAY=MO,WE, \
             FREQ=MONTHLY;COUNT=6, FREQ=YEARLY…)"
        )))
    }
}

fn parse_reminder(a: &Args) -> Result<Option<i32>, ToolError> {
    match a.opt_i64("reminder_minutes")? {
        None => Ok(None),
        Some(m) if (0..=40_320).contains(&m) => Ok(Some(m as i32)),
        Some(_) => Err(ToolError::new(
            "\"reminder_minutes\" must be between 0 and 40320 (four weeks)",
        )),
    }
}

/// An optional text argument, trimmed; `None` when absent.
fn text(a: &Args, key: &str) -> Result<Option<String>, ToolError> {
    Ok(a.opt_str(key)?.map(|t| t.trim().to_string()))
}

fn new_uid(ctx: &Ctx) -> String {
    format!(
        "{}-{}-{}@iris",
        ctx.now().millis(),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

// --- Writing -----------------------------------------------------------------------

fn create_event(ctx: &Ctx, a: &Args) -> ToolResult {
    let store = &ctx.services.store;
    let calendrier = match a.opt_i64("calendar_id")? {
        Some(id) => writable_calendar(ctx, id)?,
        None => crate::calendar::own_calendar(&ctx.services)
            .ok_or_else(|| ToolError::new("there is no calendar to put it in"))?,
    };
    let titre = a.str("title")?.trim().to_string();
    if titre.is_empty() {
        return Err(ToolError::new("\"title\" must not be empty"));
    }
    let all_day = a.bool_or("all_day", false)?;
    let (start_ms, end_ms) = if all_day {
        let premier = parse_day(&a.str("start")?)?;
        let dernier = match a.opt_str("end")? {
            Some(t) => parse_day(&t)?,
            None => premier,
        };
        all_day_span(premier, dernier)?
    } else {
        let debut = parse_ms(&a.str("start")?)?;
        let fin = match a.opt_str("end")? {
            Some(t) => parse_ms(&t)?,
            None => debut + HOUR_MS,
        };
        if fin < debut {
            return Err(ToolError::new("the event ends before it starts"));
        }
        (debut, fin)
    };
    let rrule = match a.opt_str("rrule")? {
        Some(r) => parse_rrule(&r)?,
        None => None,
    };
    let couleur = match a.opt_str("colour")? {
        Some(c) if !c.trim().is_empty() => Some(parse_colour(&c)?),
        _ => None,
    };
    let evenement = NewEvent {
        uid: new_uid(ctx),
        summary: titre,
        description: text(a, "description")?.unwrap_or_default(),
        location: text(a, "location")?.unwrap_or_default(),
        start_ms,
        end_ms,
        all_day,
        tzid: if all_day { None } else { local_tz() },
        rrule,
        exdates: Vec::new(),
        recurrence_id: None,
        cancelled: false,
        reminder_minutes: parse_reminder(a)?,
    };
    let id = store.insert_event(calendrier.id, &evenement, ctx.now())?;
    if let Some(lien) = text(a, "meeting_link")?.filter(|l| !l.is_empty()) {
        store.set_event_link(calendrier.id, &evenement.uid, &lien)?;
    }
    if let Some(c) = &couleur {
        store.set_event_color(calendrier.id, &evenement.uid, Some(c.as_str()))?;
    }
    if let Some(note) = a.opt_str("note")?.filter(|n| !n.trim().is_empty()) {
        store.set_event_note(
            calendrier.id,
            &evenement.uid,
            note_key(&evenement, evenement.start_ms),
            &note,
            ctx.now(),
        )?;
    }
    ctx.refresh();
    event_json(ctx, &load_event(ctx, id)?, None)
}

/// A calendar new events can go in.
fn writable_calendar(ctx: &Ctx, id: i64) -> Result<StoredCalendar, ToolError> {
    let c = ctx
        .services
        .store
        .calendar(id)?
        .ok_or_else(|| ToolError::new(format!("no calendar with id {id}")))?;
    if c.is_writable() {
        Ok(c)
    } else {
        Err(ToolError::new(format!(
            "calendar \"{}\" is read-only or a subscription: pick one with writable: true \
             (calendar_calendars)",
            c.name
        )))
    }
}

/// The event, if its calendar lets it be changed.
fn editable_event(ctx: &Ctx, id: i64) -> Result<StoredEvent, ToolError> {
    let s = load_event(ctx, id)?;
    let editable = ctx
        .services
        .store
        .calendar(s.calendar_id)?
        .is_some_and(|c| c.events_editable());
    if editable {
        Ok(s)
    } else {
        Err(ToolError::new(
            "this event's calendar is read-only: it cannot be changed here",
        ))
    }
}

fn update_event(ctx: &Ctx, a: &Args) -> ToolResult {
    let store = &ctx.services.store;
    let s = editable_event(ctx, a.i64("id")?)?;
    let ancien = &s.event;
    let vers = match a.opt_i64("calendar_id")? {
        Some(c) if c != s.calendar_id => writable_calendar(ctx, c)?.id,
        _ => s.calendar_id,
    };
    let deplace = vers != s.calendar_id;

    let mut e = ancien.clone();
    if a.has("title") {
        let titre = text(a, "title")?.unwrap_or_default();
        if titre.is_empty() {
            return Err(ToolError::new("\"title\" must not be empty"));
        }
        e.summary = titre;
    }
    if a.has("description") {
        e.description = text(a, "description")?.unwrap_or_default();
    }
    if a.has("location") {
        e.location = text(a, "location")?.unwrap_or_default();
    }
    if a.has("rrule") {
        e.rrule = match a.opt_str("rrule")? {
            Some(r) => parse_rrule(&r)?,
            None => None,
        };
    }
    if a.has("reminder_minutes") {
        e.reminder_minutes = parse_reminder(a)?;
    }

    // The times: what is not given is kept, the length with it.
    let all_day = a.opt_bool("all_day")?.unwrap_or(ancien.all_day);
    let debut_donne = a.opt_str("start")?;
    let fin_donnee = a.opt_str("end")?;
    if all_day {
        let (premier_avant, dernier_avant) = days_of(ancien);
        let premier = match &debut_donne {
            Some(t) => parse_day(t)?,
            None => premier_avant,
        };
        let dernier = match &fin_donnee {
            Some(t) => parse_day(t)?,
            None => premier + (dernier_avant - premier_avant),
        };
        (e.start_ms, e.end_ms) = all_day_span(premier, dernier)?;
        e.tzid = None;
    } else {
        if ancien.all_day && debut_donne.is_none() {
            return Err(ToolError::new(
                "give \"start\" (and \"end\") to make an all-day event a timed one",
            ));
        }
        let debut = match &debut_donne {
            Some(t) => parse_ms(t)?,
            None => ancien.start_ms,
        };
        let fin = match (&fin_donnee, &debut_donne) {
            (Some(t), _) => parse_ms(t)?,
            (None, Some(_)) if ancien.all_day => debut + HOUR_MS,
            (None, Some(_)) => debut + (ancien.end_ms - ancien.start_ms).max(0),
            (None, None) => ancien.end_ms,
        };
        if fin < debut {
            return Err(ToolError::new("the event ends before it starts"));
        }
        e.start_ms = debut;
        e.end_ms = fin;
        if e.tzid.is_none() {
            e.tzid = local_tz();
        }
    }
    e.all_day = all_day;
    let heures_changees =
        (e.start_ms, e.end_ms, e.all_day) != (ancien.start_ms, ancien.end_ms, ancien.all_day);
    if heures_changees && ancien.rrule.is_some() && !a.bool_or("whole_series", false)? {
        return Err(ToolError::new(
            "this event repeats: changing its start or end moves every occurrence. Pass \
             whole_series=true to do that (start/end are then the first occurrence's), or \
             delete the one occurrence and create a separate event",
        ));
    }

    // Its colour and link, as asked, or carried to the calendar it moves to.
    let cle = (s.calendar_id, ancien.uid.clone());
    let couleurs = store.event_colors()?;
    let liens = store.event_links()?;
    let couleur: Option<Option<String>> = if a.has("colour") {
        Some(match a.opt_str("colour")? {
            Some(c) if !c.trim().is_empty() => Some(parse_colour(&c)?),
            _ => None,
        })
    } else if deplace {
        Some(couleurs.get(&cle).cloned())
    } else {
        None
    };
    let lien: Option<String> = if a.has("meeting_link") {
        Some(text(a, "meeting_link")?.unwrap_or_default())
    } else if deplace {
        Some(liens.get(&cle).cloned().unwrap_or_default())
    } else {
        None
    };

    if deplace {
        store.move_event_notes(s.calendar_id, &ancien.uid, Some(vers))?;
    }
    store.update_event(s.id, vers, &e, ctx.now())?;
    if deplace {
        if couleurs.contains_key(&cle) {
            store.set_event_color(s.calendar_id, &ancien.uid, None)?;
        }
        if liens.contains_key(&cle) {
            store.set_event_link(s.calendar_id, &ancien.uid, "")?;
        }
    }
    if let Some(c) = couleur {
        store.set_event_color(vers, &e.uid, c.as_deref())?;
    }
    if let Some(l) = lien {
        store.set_event_link(vers, &e.uid, &l)?;
    }
    // A task's slot carries the task's hour with it.
    if heures_changees && !e.all_day {
        crate::tasks::slot_moved(&ctx.services, &e.uid, e.start_ms);
    }
    ctx.refresh();
    event_json(ctx, &load_event(ctx, s.id)?, None)
}

fn delete_event(ctx: &Ctx, a: &Args) -> ToolResult {
    let store = &ctx.services.store;
    let s = editable_event(ctx, a.i64("id")?)?;
    let occurrence = parse_occurrence(a, &s)?;
    let e = &s.event;
    let fait = match (occurrence, &e.rrule, e.recurrence_id) {
        // One occurrence of a series: taken out of its rule.
        (Some(ms), Some(_), None) => {
            let mut serie = e.clone();
            if !serie.exdates.contains(&ms) {
                serie.exdates.push(ms);
                serie.exdates.sort_unstable();
            }
            store.update_event(s.id, s.calendar_id, &serie, ctx.now())?;
            store.set_event_note(s.calendar_id, &e.uid, ms, "", ctx.now())?;
            json!({
                "deleted": "occurrence",
                "event_id": s.id,
                "title": e.summary,
                "occurrence_start": format_ms(ms),
            })
        }
        // A changed occurrence: cancelled, so the series does not put it back.
        (_, _, Some(_)) => {
            store.set_event_cancelled(s.id, true)?;
            json!({
                "deleted": "occurrence",
                "event_id": s.id,
                "title": e.summary,
                "occurrence_start": format_ms(e.start_ms),
            })
        }
        _ => {
            // A series goes with its changed occurrences and its notes.
            if e.rrule.is_some() {
                for autre in store.events_of_calendar(s.calendar_id)? {
                    if autre.event.uid == e.uid && autre.event.recurrence_id.is_some() {
                        store.delete_event(autre.id)?;
                    }
                }
            }
            store.move_event_notes(s.calendar_id, &e.uid, None)?;
            store.delete_event(s.id)?;
            let quoi = if e.rrule.is_some() { "series" } else { "event" };
            json!({
                "deleted": quoi,
                "event_id": s.id,
                "title": e.summary,
            })
        }
    };
    ctx.refresh();
    Ok(fait)
}

fn set_note(ctx: &Ctx, a: &Args) -> ToolResult {
    let s = load_event(ctx, a.i64("id")?)?;
    if !a.has("note") {
        return Err(ToolError::new("missing argument \"note\""));
    }
    let note = a.opt_str("note")?.unwrap_or_default();
    let occurrence = parse_occurrence(a, &s)?;
    if s.event.rrule.is_some() && occurrence.is_none() {
        return Err(ToolError::new(
            "this event repeats and each occurrence has its own note: give occurrence_start",
        ));
    }
    let debut = occurrence.unwrap_or(s.event.start_ms);
    ctx.services.store.set_event_note(
        s.calendar_id,
        &s.event.uid,
        note_key(&s.event, debut),
        &note,
        ctx.now(),
    )?;
    ctx.refresh();
    Ok(json!({
        "event_id": s.id,
        "occurrence_start": format_ms(debut),
        "note": note.trim(),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::test_ctx;
    use super::*;

    fn run(ctx: &Ctx, name: &str, v: Value) -> Value {
        call(ctx, name, &Args::new(v)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn titles(v: &Value) -> Vec<String> {
        v["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["title"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn an_event_is_made_read_moved_and_deleted() {
        let (ctx, _dossier, _rt) = test_ctx();
        let cal = ctx
            .services
            .store
            .create_calendar("Work", "#4fb286", None, ctx.now())
            .unwrap();
        let fait = run(
            &ctx,
            "calendar_create_event",
            json!({"calendar_id": cal, "title": "Planning", "start": "2026-03-10T09:00",
                   "location": "Room 2", "colour": "#E0795B", "note": "Bring the figures"}),
        );
        let id = fait["event_id"].as_i64().unwrap();
        assert_eq!(fait["calendar_id"].as_i64(), Some(cal));
        assert!(fait["end"]
            .as_str()
            .unwrap()
            .starts_with("2026-03-10T10:00"));
        assert_eq!(fait["colour"], "#e0795b");
        assert_eq!(fait["note"], "Bring the figures");

        let jour = run(&ctx, "calendar_day", json!({"day": "2026-03-10"}));
        assert_eq!(titles(&jour), vec!["Planning"]);
        assert_eq!(jour["occurrences"][0]["location"], "Room 2");

        // Moved: its length is kept, its note follows.
        let deplace = run(
            &ctx,
            "calendar_update_event",
            json!({"id": id, "start": "2026-03-11T14:00"}),
        );
        assert!(deplace["start"]
            .as_str()
            .unwrap()
            .starts_with("2026-03-11T14:00"));
        assert!(deplace["end"]
            .as_str()
            .unwrap()
            .starts_with("2026-03-11T15:00"));
        assert_eq!(deplace["note"], "Bring the figures");
        assert!(titles(&run(&ctx, "calendar_day", json!({"day": "2026-03-10"}))).is_empty());
        assert_eq!(
            titles(&run(&ctx, "calendar_day", json!({"day": "2026-03-11"}))),
            vec!["Planning"]
        );

        let efface = run(&ctx, "calendar_delete_event", json!({"id": id}));
        assert_eq!(efface["deleted"], "event");
        assert!(titles(&run(&ctx, "calendar_day", json!({"day": "2026-03-11"}))).is_empty());
        assert!(call(&ctx, "calendar_event", &Args::new(json!({"id": id}))).is_err());
    }

    #[test]
    fn a_series_has_a_note_per_occurrence_and_loses_one_occurrence_alone() {
        let (ctx, _dossier, _rt) = test_ctx();
        let fait = run(
            &ctx,
            "calendar_create_event",
            json!({"title": "Standup", "start": "2026-03-02T08:00", "end": "2026-03-02T08:15",
                   "rrule": "FREQ=WEEKLY"}),
        );
        let id = fait["event_id"].as_i64().unwrap();
        assert_eq!(fait["recurring"], "FREQ=WEEKLY");

        let semaine = run(&ctx, "calendar_week", json!({"day": "2026-03-11"}));
        assert_eq!(semaine["week_start"], "2026-03-09");
        let lundi = &semaine["days"][0]["occurrences"][0];
        assert_eq!(lundi["title"], "Standup");
        let occurrence = lundi["occurrence_start"].as_str().unwrap().to_string();

        run(
            &ctx,
            "calendar_set_note",
            json!({"id": id, "occurrence_start": occurrence, "note": "Demo day"}),
        );
        let lu = run(
            &ctx,
            "calendar_event",
            json!({"id": id, "occurrence_start": occurrence}),
        );
        assert_eq!(lu["note"], "Demo day");
        assert_eq!(run(&ctx, "calendar_event", json!({"id": id}))["note"], "");

        // Its times are the whole series': refused unless said so.
        assert!(call(
            &ctx,
            "calendar_update_event",
            &Args::new(json!({"id": id, "start": "2026-03-09T09:00"}))
        )
        .is_err());

        let mars = json!({"from": "2026-03-01", "to": "2026-03-31", "query": "standup"});
        assert_eq!(run(&ctx, "calendar_events", mars.clone())["count"], 5);
        let efface = run(
            &ctx,
            "calendar_delete_event",
            json!({"id": id, "occurrence_start": occurrence}),
        );
        assert_eq!(efface["deleted"], "occurrence");
        assert_eq!(run(&ctx, "calendar_events", mars)["count"], 4);
        assert_eq!(
            titles(&run(&ctx, "calendar_day", json!({"day": "2026-03-16"}))),
            vec!["Standup"]
        );
    }

    #[test]
    fn an_all_day_event_stays_on_its_own_days() {
        let (ctx, _dossier, _rt) = test_ctx();
        let fait = run(
            &ctx,
            "calendar_create_event",
            json!({"title": "Offsite", "start": "2026-03-10", "end": "2026-03-11",
                   "all_day": true}),
        );
        assert_eq!(fait["start"], "2026-03-10");
        assert_eq!(fait["end"], "2026-03-11");
        for (jour, attendu) in [
            ("2026-03-09", 0),
            ("2026-03-10", 1),
            ("2026-03-11", 1),
            ("2026-03-12", 0),
        ] {
            let v = run(&ctx, "calendar_day", json!({"day": jour}));
            assert_eq!(titles(&v).len(), attendu, "{jour}");
        }
        let resume = day_summary(&ctx, parse_day("2026-03-10").unwrap()).unwrap();
        assert_eq!(resume[0]["title"], "Offsite");
        assert_eq!(resume[0]["all_day"], true);
    }

    #[test]
    fn bad_values_are_told_back() {
        assert!(parse_colour("blue").is_err());
        assert_eq!(parse_colour("5B8DEF").unwrap(), "#5b8def");
        assert_eq!(
            parse_rrule("RRULE:freq=weekly;byday=mo")
                .unwrap()
                .as_deref(),
            Some("FREQ=WEEKLY;BYDAY=MO")
        );
        assert!(parse_rrule("every monday").is_err());
        assert_eq!(parse_rrule(" ").unwrap(), None);
    }
}
