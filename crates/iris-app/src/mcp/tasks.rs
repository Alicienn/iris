//! Tasks, habits and goals, for an assistant.
//!
//! Every write goes through the store calls the Tasks tab makes, and ticking a task
//! goes through [`crate::tasks::toggle_done`], so a repeating task makes its next one
//! as a click does. Reminders are worked out again after each change, as in the tab.

use super::time;
use super::Ctx;
use chrono::{Duration, Local, NaiveDate, NaiveTime, TimeZone};
use iris_growth::habits::{Day, Schedule};
use iris_mcp::{Args, Tool, ToolError, ToolResult};
use iris_store::{Goal, GoalKind, NewGoal, NewHabit, NewTask, StoredTask};
use iris_types::Timestamp;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

// --- Shared --------------------------------------------------------------------------

fn store(ctx: &Ctx) -> &iris_store::Store {
    &ctx.services.store
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

fn day_text(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn stored_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn hhmm(m: i32) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

fn round2(x: f32) -> f64 {
    (f64::from(x) * 100.0).round() / 100.0
}

/// A time of day, `14:30` (also `9h30`, `9`), as minutes since midnight.
fn parse_hhmm(key: &str, s: &str) -> Result<i32, ToolError> {
    let t = s.trim().to_ascii_lowercase().replace('h', ":");
    let (h, m) = t.split_once(':').unwrap_or((t.as_str(), "0"));
    let m = if m.trim().is_empty() { "0" } else { m.trim() };
    match (h.trim().parse::<i32>(), m.parse::<i32>()) {
        (Ok(h), Ok(m)) if (0..24).contains(&h) && (0..60).contains(&m) => Ok(h * 60 + m),
        _ => Err(ToolError::new(format!(
            "\"{key}\" must be a time like 14:30, not \"{s}\""
        ))),
    }
}

fn not_found(what: &str, id: i64) -> ToolError {
    ToolError::new(format!("no {what} with id {id}"))
}

/// `#rrggbb`.
fn color(s: &str) -> Result<String, ToolError> {
    let c = s.trim();
    if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()) {
        Ok(c.to_ascii_lowercase())
    } else {
        Err(ToolError::new(format!(
            "\"color\" must be written #rrggbb, not \"{s}\""
        )))
    }
}

fn non_empty(key: &str, s: &str) -> Result<String, ToolError> {
    let t = s.trim();
    if t.is_empty() {
        Err(ToolError::new(format!("\"{key}\" cannot be empty")))
    } else {
        Ok(t.to_string())
    }
}

fn get_goal(ctx: &Ctx, id: i64) -> Result<Goal, ToolError> {
    store(ctx).goal(id)?.ok_or_else(|| not_found("goal", id))
}

// --- The tools -----------------------------------------------------------------------

pub(super) fn tools() -> Vec<Tool> {
    let id = json!({"type": "integer"});
    let day = json!({"type": "string", "description": "A day: 2026-11-06, today, tomorrow or yesterday."});
    let task_fields = json!({
        "title": {"type": "string"},
        "notes": {"type": "string", "description": "Free text under the title."},
        "list_id": {"type": "integer", "description": "From tasks_lists. Default: the first list (the user's main list). A subtask always sits in its task's list."},
        "due_day": day,
        "due_time": {"type": "string", "description": "HH:MM, local time. Without due_day it is today."},
        "priority": {"description": "0 none, 1 low, 2 medium, 3 high (or those words)."},
        "parent_id": {"type": "integer", "description": "Makes it a subtask (step) of that task. Only one level: a subtask has no subtasks."},
        "repeat": {"type": "string", "enum": ["daily", "weekdays", "weekly", "monthly", "yearly", "none"], "description": "Comes back once done, due on the next day of the rule. Needs a due day: today when none is given."},
        "goal_id": {"type": "integer", "description": "The goal this task moves forward."},
        "estimate": {"type": "integer", "description": "How long it takes, in minutes."},
        "remind_before": {"type": "integer", "description": "Remind this many minutes before the due time (0 at the time; a task without a time is reminded from 09:00). Default when due_time is given: 0. Negative or null: no reminder."},
        "thread_id": {"type": "integer", "description": "The mail thread the task comes from (from mail tools)."},
        "event_id": {"type": "integer", "description": "A calendar event id (from calendar tools): the task is listed under that event."},
        "event_start": {"type": "string", "description": "For a repeating event: the start of the occurrence, as calendar tools give it. Default: its first one."},
    });
    let mut update_fields = task_fields.clone();
    update_fields["id"] = id.clone();
    let habit_fields = json!({
        "title": {"type": "string"},
        "schedule": {"type": "string", "description": "daily, weekdays, weekends, a list of days (mon,wed,fri), days:1010100 (Monday first) or week:3 (any 3 days a week)."},
        "kind": {"type": "string", "enum": ["build", "quit"], "description": "build: something to do; quit: something to stop (a day is ticked when it was kept). Default build."},
        "amount": {"type": "integer", "description": "What makes a day done, e.g. 20 (pages); 0 for a plain yes/no habit."},
        "unit": {"type": "string", "description": "Of what: pages, minutes, glasses."},
        "icon": {"type": "string", "enum": crate::habits::ICONES},
        "color": {"type": "string", "description": "#rrggbb."},
        "goal_id": {"type": "integer", "description": "The goal it supports."},
        "remind_time": {"type": "string", "description": "HH:MM: reminded then if not done; empty or null for none."},
    });
    let mut habit_update = habit_fields.clone();
    habit_update["id"] = id.clone();
    vec![
        Tool::read(
            "tasks_lists",
            "Task lists",
            "The user's task lists: id, name, colour and how many open tasks each holds (subtasks \
             not counted). default_list_id is where a task goes when no list is said.",
            obj(json!({}), &[]),
        ),
        Tool::write(
            "tasks_create_list",
            "New task list",
            "Makes a task list. Returns its id, name and colour.",
            obj(
                json!({"name": {"type": "string"}, "color": {"type": "string", "description": "#rrggbb; default: the next of the palette."}}),
                &["name"],
            ),
        ),
        Tool::write(
            "tasks_rename_list",
            "Rename a task list",
            "Gives a task list a new name.",
            obj(
                json!({"id": id, "name": {"type": "string"}}),
                &["id", "name"],
            ),
        ),
        Tool::destructive(
            "tasks_delete_list",
            "Delete a task list",
            "Deletes a task list AND every task in it (done or not). The last list cannot be \
             deleted. Returns how many tasks went with it.",
            obj(json!({"id": id}), &["id"]),
        ),
        Tool::read(
            "tasks_list",
            "Find tasks",
            "Tasks matching every filter given, open ones first in the list's order (due first), \
             then (include_done) the done ones, latest first. Subtasks are included, with \
             parent_id. Each task: id, title, list, due_day, due_time, overdue, priority, \
             done/done_at, notes, subtasks {done,total}, repeat, estimate_minutes, goal, event \
             link {uid, occurrence_start}, thread_id and source (where it came from). Returns \
             {count, total, tasks}: total is how many matched before the limit.",
            obj(
                json!({
                    "list_id": id,
                    "include_done": {"type": "boolean", "description": "Default false."},
                    "due_on": {"type": "string", "description": "Only tasks due that day (2026-11-06, today, tomorrow)."},
                    "due_before": {"type": "string", "description": "Only tasks due on or before that day."},
                    "overdue": {"type": "boolean", "description": "Only open tasks past their due day or time."},
                    "goal_id": id,
                    "parent_id": {"type": "integer", "description": "Only the subtasks of that task."},
                    "thread_id": {"type": "integer", "description": "Only tasks from that mail thread."},
                    "event_id": {"type": "integer", "description": "Only tasks of that calendar event."},
                    "event_start": {"type": "string", "description": "With event_id, for a repeating event: only that occurrence's tasks."},
                    "query": {"type": "string", "description": "Words in the title or notes (case does not matter)."},
                    "limit": {"type": "integer", "description": "Default 100, at most 500."},
                }),
                &[],
            ),
        ),
        Tool::read(
            "tasks_get",
            "Read a task",
            "One task, as tasks_list gives it, with its subtasks in order.",
            obj(json!({"id": id}), &["id"]),
        ),
        Tool::write(
            "tasks_create",
            "New task",
            "Adds a task (or, with parent_id, a subtask; with event_id, a task of a calendar \
             event, due when it starts unless due_day says otherwise). Returns the task as \
             tasks_get does.",
            obj(task_fields, &["title"]),
        ),
        Tool::write(
            "tasks_update",
            "Change a task",
            "Changes the fields given and leaves the others. Send a field as null (or \"\") to \
             clear it: due_day (clears the time too), due_time, parent_id, goal_id, estimate, \
             repeat, remind_before, thread_id, event_id. Moving the due day later counts as \
             putting it off. To tick a task use tasks_complete. Returns the task.",
            obj(update_fields, &["id"]),
        ),
        Tool::write(
            "tasks_complete",
            "Tick a task",
            "Marks a task done (done: true, the default) or open again (done: false). A \
             repeating task ticked makes its next one, returned as next.",
            obj(
                json!({"id": id, "done": {"type": "boolean", "description": "Default true."}}),
                &["id"],
            ),
        ),
        Tool::destructive(
            "tasks_delete",
            "Delete a task",
            "Deletes a task and its subtasks for good (a done task can be ticked instead).",
            obj(json!({"id": id}), &["id"]),
        ),
        Tool::read(
            "habits_list",
            "Habits",
            "The habits, each with its schedule, today's status (due, done, amount, status: \
             done, partial, to do, rest…), current and best streak (in days, or weeks for so \
             many a week), and over from..to (default the last 30 days): the days ticked with \
             their amount, days_done and rate (0..1 of what was wanted).",
            obj(
                json!({
                    "habit_id": {"type": "integer", "description": "Only this habit."},
                    "from": day,
                    "to": {"type": "string", "description": "Default today."},
                }),
                &[],
            ),
        ),
        Tool::write(
            "habits_create",
            "New habit",
            "Adds a habit. Default: daily, build, no amount. Returns it as habits_list does.",
            obj(habit_fields, &["title"]),
        ),
        Tool::write(
            "habits_update",
            "Change a habit",
            "Changes the fields given. goal_id, unit and remind_time are cleared by null or \"\". \
             The days already ticked stay. Returns the habit.",
            obj(habit_update, &["id"]),
        ),
        Tool::destructive(
            "habits_delete",
            "Delete a habit",
            "Deletes a habit and every day ticked for it.",
            obj(json!({"id": id}), &["id"]),
        ),
        Tool::write(
            "habits_check",
            "Tick a habit",
            "Says how much of a habit was done on a day (today by default; never a day ahead, \
             nor before the habit was made). amount defaults to the habit's whole amount (1 for \
             a yes/no habit): less is a partial day; 0 unticks the day. Returns the habit.",
            obj(
                json!({"habit_id": id, "day": day, "amount": {"type": "integer"}}),
                &["habit_id"],
            ),
        ),
        Tool::read(
            "goals_list",
            "Goals",
            "The goals with where they stand: kind (count or milestones), target, due_day, \
             progress {done, of, fraction, status on_track|behind|reached, badge, pace (a \
             sentence), expected_by_now, behind, days_left, left} and open_tasks.",
            obj(json!({}), &[]),
        ),
        Tool::read(
            "goals_get",
            "Read a goal",
            "One goal as goals_list gives it, with its log entries (count goals), its \
             milestones in order, the tasks that move it forward and the habits that support it.",
            obj(json!({"id": id}), &["id"]),
        ),
        Tool::write(
            "goals_create",
            "New goal",
            "Adds a goal to reach by due_day (after today). kind count (default) needs target \
             (how many) and unit (of what; default times); kind milestones takes the steps' \
             titles in milestones. Returns the goal.",
            obj(
                json!({
                    "title": {"type": "string"},
                    "kind": {"type": "string", "enum": ["count", "milestones"]},
                    "target": {"type": "integer"},
                    "unit": {"type": "string"},
                    "due_day": day,
                    "why": {"type": "string", "description": "What it is for, in the user's words."},
                    "color": {"type": "string", "description": "#rrggbb."},
                    "milestones": {"type": "array", "items": {"type": "string"}},
                }),
                &["title", "due_day"],
            ),
        ),
        Tool::write(
            "goals_update",
            "Change a goal",
            "Changes the fields given: title, why, due_day (today or later), color; target and \
             unit for a count goal. Its kind does not change. Returns the goal.",
            obj(
                json!({
                    "id": id,
                    "title": {"type": "string"},
                    "why": {"type": "string"},
                    "due_day": day,
                    "target": {"type": "integer"},
                    "unit": {"type": "string"},
                    "color": {"type": "string"},
                }),
                &["id"],
            ),
        ),
        Tool::destructive(
            "goals_delete",
            "Delete a goal",
            "Deletes a goal, its log and its milestones. Its tasks and habits stay, without it.",
            obj(json!({"id": id}), &["id"]),
        ),
        Tool::write(
            "goals_log",
            "Log progress on a goal",
            "For a count goal: logs count (default 1) steps done, with an optional note (\"Sent \
             to Studio Nord\") at the time given (default now). With delete_entry_id instead, \
             removes one entry of its log. Returns the goal.",
            obj(
                json!({
                    "goal_id": id,
                    "note": {"type": "string"},
                    "count": {"type": "integer"},
                    "at": {"type": "string", "description": "2026-11-06T14:30, local time."},
                    "delete_entry_id": {"type": "integer"},
                }),
                &["goal_id"],
            ),
        ),
        Tool::write(
            "goals_milestone",
            "Change a goal's milestones",
            "For a milestones goal, one of: goal_id + add (a title) appends a milestone; id + \
             done (true/false) ticks or unticks one; id + delete: true removes one. Returns the \
             goal with its milestones.",
            obj(
                json!({
                    "goal_id": id,
                    "add": {"type": "string"},
                    "id": {"type": "integer", "description": "A milestone's id (from goals_get)."},
                    "done": {"type": "boolean"},
                    "delete": {"type": "boolean"},
                }),
                &[],
            ),
        ),
    ]
}

pub(super) fn call(ctx: &Ctx, name: &str, a: &Args) -> ToolResult {
    match name {
        "tasks_lists" => lists(ctx),
        "tasks_create_list" => create_list(ctx, a),
        "tasks_rename_list" => rename_list(ctx, a),
        "tasks_delete_list" => delete_list(ctx, a),
        "tasks_list" => list_tasks(ctx, a),
        "tasks_get" => get_task_tool(ctx, a),
        "tasks_create" => create_task(ctx, a),
        "tasks_update" => update_task(ctx, a),
        "tasks_complete" => complete_task(ctx, a),
        "tasks_delete" => delete_task(ctx, a),
        "habits_list" => list_habits(ctx, a),
        "habits_create" => create_habit(ctx, a),
        "habits_update" => update_habit(ctx, a),
        "habits_delete" => delete_habit(ctx, a),
        "habits_check" => check_habit(ctx, a),
        "goals_list" => list_goals(ctx),
        "goals_get" => get_goal_tool(ctx, a),
        "goals_create" => create_goal(ctx, a),
        "goals_update" => update_goal(ctx, a),
        "goals_delete" => delete_goal(ctx, a),
        "goals_log" => log_goal(ctx, a),
        "goals_milestone" => milestone(ctx, a),
        _ => Err(ToolError::new(format!("unknown tool: {name}"))),
    }
}

/// The open tasks due on `day` or before it, for the overview.
pub(super) fn due_summary(ctx: &Ctx, day: NaiveDate) -> Result<Value, ToolError> {
    let names = Names::load(ctx)?;
    let tasks: Vec<Value> = store(ctx)
        .open_tasks()?
        .iter()
        .filter_map(|t| {
            let d = stored_day(t.task.due_day.as_deref()?)?;
            (d <= day).then(|| {
                json!({
                    "id": t.id,
                    "title": t.task.title,
                    "list": names.lists.get(&t.task.list_id),
                    "due_day": t.task.due_day,
                    "due_time": t.task.due_minute.map(hhmm),
                    "late": d < day,
                    "priority": t.task.priority,
                    "parent_id": t.task.parent_id,
                })
            })
        })
        .collect();
    let total = tasks.len();
    Ok(json!({
        "count": total,
        "tasks": tasks.into_iter().take(50).collect::<Vec<_>>(),
    }))
}

/// The habits wanted on `day` (or done that day), done or not, for the overview.
pub(super) fn habits_summary(ctx: &Ctx, day: NaiveDate) -> Result<Value, ToolError> {
    Ok(Value::Array(
        load_habits(ctx)?
            .iter()
            .filter(|u| u.r.due_today(day) || u.r.done_on(day))
            .map(|u| {
                json!({
                    "id": u.h.id,
                    "title": u.h.habit.title,
                    "done": u.r.done_on(day),
                    "amount": u.r.amount_on(day),
                    "wanted": u.h.habit.amount.max(1),
                    "unit": u.h.habit.unit,
                    "streak": u.r.streak(day).label(),
                })
            })
            .collect(),
    ))
}

// --- Tasks ---------------------------------------------------------------------------

/// The names a task shows: its list's and its goal's.
struct Names {
    lists: BTreeMap<i64, String>,
    goals: BTreeMap<i64, String>,
}

impl Names {
    fn load(ctx: &Ctx) -> Result<Self, ToolError> {
        Ok(Self {
            lists: store(ctx)
                .task_lists()?
                .into_iter()
                .map(|l| (l.id, l.name))
                .collect(),
            goals: store(ctx)
                .goals()?
                .into_iter()
                .map(|g| (g.id, g.goal.title))
                .collect(),
        })
    }
}

fn get_task(ctx: &Ctx, id: i64) -> Result<StoredTask, ToolError> {
    store(ctx).task(id)?.ok_or_else(|| not_found("task", id))
}

fn task_json(ctx: &Ctx, t: &StoredTask, names: &Names) -> Value {
    let n = &t.task;
    let due = n.due_day.as_deref().and_then(stored_day);
    let minute = n.due_minute.map(|m| m.clamp(0, 24 * 60 - 1) as u32);
    let overdue = !t.is_done()
        && due.is_some_and(|d| iris_tasks::is_overdue(d, minute, Local::now().naive_local()));
    let mut o = Map::new();
    o.insert("id".into(), json!(t.id));
    o.insert("title".into(), json!(n.title));
    o.insert("list_id".into(), json!(n.list_id));
    o.insert("list".into(), json!(names.lists.get(&n.list_id)));
    o.insert("done".into(), json!(t.is_done()));
    o.insert("due_day".into(), json!(n.due_day));
    o.insert("due_time".into(), json!(n.due_minute.map(hhmm)));
    o.insert("overdue".into(), json!(overdue));
    o.insert("priority".into(), json!(n.priority));
    if !n.notes.is_empty() {
        o.insert("notes".into(), json!(n.notes));
    }
    if let Some(at) = t.done_at {
        o.insert("done_at".into(), json!(time::format_ms(at.millis())));
    }
    match n.parent_id {
        Some(p) => {
            o.insert("parent_id".into(), json!(p));
        }
        None => {
            let (total, done) = store(ctx).subtask_progress(t.id).unwrap_or((0, 0));
            if total > 0 {
                o.insert("subtasks".into(), json!({"done": done, "total": total}));
            }
        }
    }
    if let Some(r) = &n.repeat {
        o.insert("repeat".into(), json!(r));
    }
    if let Some(e) = n.estimate {
        o.insert("estimate_minutes".into(), json!(e));
    }
    if let Some(b) = n.remind_before {
        o.insert("remind_before_minutes".into(), json!(b));
    }
    if let Some(g) = n.goal_id {
        o.insert("goal_id".into(), json!(g));
        o.insert("goal".into(), json!(names.goals.get(&g)));
    }
    if let Some(uid) = &n.event_uid {
        let debut = n.event_start.filter(|s| *s != 0).map(time::format_ms);
        o.insert(
            "event".into(),
            json!({"uid": uid, "occurrence_start": debut}),
        );
    }
    if let Some(th) = n.thread_id {
        o.insert("thread_id".into(), json!(th));
    }
    if !n.source.is_empty() {
        o.insert("source".into(), json!(n.source));
    }
    if n.postponed > 0 {
        o.insert("postponed".into(), json!(n.postponed));
    }
    Value::Object(o)
}

fn priority(a: &Args) -> Result<Option<i32>, ToolError> {
    let faux =
        || ToolError::new("\"priority\" must be 0 (none) to 3 (high), or none, low, medium, high");
    let p: i64 = match a.get("priority") {
        None => return Ok(None),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "none" | "" => 0,
            "low" => 1,
            "medium" => 2,
            "high" => 3,
            autre => autre.parse().map_err(|_| faux())?,
        },
        Some(_) => a.opt_i64("priority")?.unwrap_or(0),
    };
    if (0..=3).contains(&p) {
        Ok(Some(p as i32))
    } else {
        Err(faux())
    }
}

fn repeat_rule(s: &str) -> Result<Option<String>, ToolError> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() || s == "none" {
        return Ok(None);
    }
    if iris_tasks::repeat::REPEATS
        .iter()
        .any(|(mot, _)| !mot.is_empty() && *mot == s)
    {
        Ok(Some(s))
    } else {
        Err(ToolError::new(format!(
            "\"repeat\" must be daily, weekdays, weekly, monthly, yearly or none, not \"{s}\""
        )))
    }
}

fn estimate(a: &Args) -> Result<Option<i32>, ToolError> {
    match a.opt_i64("estimate")? {
        None => Ok(None),
        Some(m) if (1..=10_000).contains(&m) => Ok(Some(m as i32)),
        Some(_) => Err(ToolError::new(
            "\"estimate\" is in minutes, from 1 to 10000",
        )),
    }
}

fn check_goal(ctx: &Ctx, id: i64) -> Result<i64, ToolError> {
    get_goal(ctx, id).map(|g| g.id)
}

/// The parent a subtask may have: a task that is not itself a subtask.
fn parent_task(ctx: &Ctx, id: i64) -> Result<StoredTask, ToolError> {
    let p = get_task(ctx, id)?;
    if p.task.parent_id.is_some() {
        return Err(ToolError::new(format!(
            "task {id} is itself a subtask: subtasks have no subtasks"
        )));
    }
    Ok(p)
}

/// What a mail thread lends a task: "Marie: Quote", as the Tasks tab writes it.
fn thread_source(ctx: &Ctx, id: i64) -> Result<String, ToolError> {
    let fil = store(ctx)
        .thread_row(iris_types::ThreadId(id))?
        .ok_or_else(|| not_found("mail thread", id))?;
    let sujet = if fil.subject.trim().is_empty() {
        "(No subject)".to_string()
    } else {
        fil.subject.trim().to_string()
    };
    Ok(format!("{}: {}", fil.from_display, sujet))
}

/// A calendar event's occurrence, as its tasks are keyed.
struct EventLink {
    uid: String,
    /// The occurrence key: its start for a repeating event, 0 otherwise.
    occurrence: i64,
    /// When this occurrence starts.
    start: i64,
    all_day: bool,
    title: String,
}

/// An occurrence's start. An all-day event's days sit at midnight UTC: a bare day is
/// read there.
fn occurrence_ms(s: &str, all_day: bool) -> Result<i64, ToolError> {
    let s = s.trim();
    if all_day && s.len() == 10 {
        let d = time::parse_day(s)?;
        return Ok(d.and_time(NaiveTime::MIN).and_utc().timestamp_millis());
    }
    time::parse_ms(s)
}

fn event_link(ctx: &Ctx, id: i64, a: &Args) -> Result<EventLink, ToolError> {
    let e = store(ctx)
        .event(id)?
        .ok_or_else(|| not_found("calendar event", id))?
        .event;
    let repete = e.rrule.is_some();
    let start = match (repete, a.opt_str("event_start")?) {
        (true, Some(s)) => occurrence_ms(&s, e.all_day)?,
        _ => e.start_ms,
    };
    Ok(EventLink {
        occurrence: if repete { start } else { 0 },
        start,
        all_day: e.all_day,
        title: e.summary,
        uid: e.uid,
    })
}

/// When a task of this occurrence is due, as the calendar's panel sets it: the day
/// alone for an all-day event, else its local start.
fn event_due(l: &EventLink) -> (String, Option<i32>) {
    if l.all_day {
        let d = chrono::DateTime::from_timestamp_millis(l.start)
            .map(|d| d.date_naive())
            .unwrap_or_else(time::today);
        (day_text(d), None)
    } else {
        let t = Local
            .timestamp_millis_opt(l.start)
            .earliest()
            .map(|d| d.naive_local())
            .unwrap_or_default();
        (
            day_text(t.date()),
            Some((t.time() - NaiveTime::MIN).num_minutes() as i32),
        )
    }
}

fn lists(ctx: &Ctx) -> ToolResult {
    let listes = store(ctx).task_lists()?;
    let ouvertes = store(ctx).open_tasks()?;
    let lignes: Vec<Value> = listes
        .iter()
        .map(|l| {
            let n = ouvertes
                .iter()
                .filter(|t| t.task.list_id == l.id && t.task.parent_id.is_none())
                .count();
            json!({"id": l.id, "name": l.name, "color": l.color, "open": n})
        })
        .collect();
    Ok(json!({
        "default_list_id": listes.first().map(|l| l.id),
        "lists": lignes,
    }))
}

fn create_list(ctx: &Ctx, a: &Args) -> ToolResult {
    let name = non_empty("name", &a.str("name")?)?;
    let listes = store(ctx).task_lists()?;
    if let Some(l) = listes.iter().find(|l| l.name.eq_ignore_ascii_case(&name)) {
        return Err(ToolError::new(format!(
            "a list named \"{}\" already exists (id {})",
            l.name, l.id
        )));
    }
    let couleur = match a.opt_str("color")? {
        Some(c) => color(&c)?,
        None => crate::tasks::COULEURS[listes.len() % crate::tasks::COULEURS.len()].to_string(),
    };
    let id = store(ctx).create_task_list(&name, &couleur, ctx.now())?;
    ctx.refresh();
    Ok(json!({"id": id, "name": name, "color": couleur}))
}

fn rename_list(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let name = non_empty("name", &a.str("name")?)?;
    if !store(ctx).task_lists()?.iter().any(|l| l.id == id) {
        return Err(not_found("task list", id));
    }
    store(ctx).rename_task_list(id, &name)?;
    ctx.refresh();
    Ok(json!({"id": id, "name": name}))
}

fn delete_list(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let listes = store(ctx).task_lists()?;
    let liste = listes
        .iter()
        .find(|l| l.id == id)
        .ok_or_else(|| not_found("task list", id))?;
    if listes.len() == 1 {
        return Err(ToolError::new(
            "this is the only list: it cannot be deleted",
        ));
    }
    let n = store(ctx)
        .all_tasks()?
        .iter()
        .filter(|t| t.task.list_id == id)
        .count();
    store(ctx).delete_task_list(id)?;
    ctx.refresh();
    Ok(json!({"deleted": id, "name": liste.name, "tasks_deleted": n}))
}

fn list_tasks(ctx: &Ctx, a: &Args) -> ToolResult {
    let mut taches = store(ctx).open_tasks()?;
    if a.bool_or("include_done", false)? {
        let mut faites: Vec<StoredTask> = store(ctx)
            .all_tasks()?
            .into_iter()
            .filter(StoredTask::is_done)
            .collect();
        faites.sort_by(|x, y| y.done_at.cmp(&x.done_at));
        taches.extend(faites);
    }
    let liste = a.opt_i64("list_id")?;
    let due_on = a
        .opt_str("due_on")?
        .map(|s| time::parse_day(&s))
        .transpose()?;
    let due_before = a
        .opt_str("due_before")?
        .map(|s| time::parse_day(&s))
        .transpose()?;
    let en_retard = a.bool_or("overdue", false)?;
    let objectif = a.opt_i64("goal_id")?;
    let parent = a.opt_i64("parent_id")?;
    let fil = a.opt_i64("thread_id")?;
    let evenement = match a.opt_i64("event_id")? {
        Some(id) => {
            let l = event_link(ctx, id, a)?;
            // A repeating event without an occurrence named: all of its tasks.
            let une = a.has("event_start") || l.occurrence == 0;
            Some((l.uid, une.then_some(l.occurrence)))
        }
        None => None,
    };
    let mots = a
        .opt_str("query")?
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    let limite = a.opt_i64("limit")?.unwrap_or(100).clamp(1, 500) as usize;
    let maintenant = Local::now().naive_local();

    let gardees: Vec<&StoredTask> = taches
        .iter()
        .filter(|t| {
            let n = &t.task;
            let jour = n.due_day.as_deref().and_then(stored_day);
            if liste.is_some_and(|l| n.list_id != l)
                || objectif.is_some_and(|g| n.goal_id != Some(g))
                || parent.is_some_and(|p| n.parent_id != Some(p))
                || fil.is_some_and(|f| n.thread_id != Some(f))
            {
                return false;
            }
            if let Some(d) = due_on {
                if jour != Some(d) {
                    return false;
                }
            }
            if let Some(d) = due_before {
                if !jour.is_some_and(|j| j <= d) {
                    return false;
                }
            }
            if en_retard {
                let minute = n.due_minute.map(|m| m.clamp(0, 24 * 60 - 1) as u32);
                let late = !t.is_done()
                    && jour.is_some_and(|j| iris_tasks::is_overdue(j, minute, maintenant));
                if !late {
                    return false;
                }
            }
            if let Some((uid, occ)) = &evenement {
                if n.event_uid.as_deref() != Some(uid.as_str()) {
                    return false;
                }
                if occ.is_some_and(|o| n.event_start.unwrap_or(0) != o) {
                    return false;
                }
            }
            if let Some(q) = &mots {
                if !n.title.to_lowercase().contains(q) && !n.notes.to_lowercase().contains(q) {
                    return false;
                }
            }
            true
        })
        .collect();
    let names = Names::load(ctx)?;
    let total = gardees.len();
    let rendues: Vec<Value> = gardees
        .into_iter()
        .take(limite)
        .map(|t| task_json(ctx, t, &names))
        .collect();
    Ok(json!({"count": rendues.len(), "total": total, "tasks": rendues}))
}

fn get_task_tool(ctx: &Ctx, a: &Args) -> ToolResult {
    let t = get_task(ctx, a.i64("id")?)?;
    let names = Names::load(ctx)?;
    let mut v = task_json(ctx, &t, &names);
    if t.task.parent_id.is_none() {
        v["subtasks"] = Value::Array(
            store(ctx)
                .subtasks(t.id)?
                .iter()
                .map(|s| task_json(ctx, s, &names))
                .collect(),
        );
    }
    Ok(v)
}

/// The task as it reads now, for the answer.
fn task_now(ctx: &Ctx, id: i64) -> ToolResult {
    let t = get_task(ctx, id)?;
    Ok(task_json(ctx, &t, &Names::load(ctx)?))
}

fn create_task(ctx: &Ctx, a: &Args) -> ToolResult {
    let title = non_empty("title", &a.str("title")?)?;
    let listes = store(ctx).task_lists()?;
    let parent = a
        .opt_i64("parent_id")?
        .map(|p| parent_task(ctx, p))
        .transpose()?;
    let list_id = match (&parent, a.opt_i64("list_id")?) {
        (Some(p), _) => p.task.list_id,
        (None, Some(l)) => {
            if !listes.iter().any(|x| x.id == l) {
                return Err(not_found("task list", l));
            }
            l
        }
        (None, None) => listes.first().map(|l| l.id).ok_or_else(|| {
            ToolError::new("there is no task list: make one with tasks_create_list")
        })?,
    };
    let mut t = NewTask {
        list_id,
        parent_id: parent.as_ref().map(|p| p.id),
        title,
        notes: a.opt_str("notes")?.unwrap_or_default(),
        priority: priority(a)?.unwrap_or(0),
        estimate: estimate(a)?,
        ..Default::default()
    };
    if let Some(e) = a.opt_i64("event_id")? {
        let l = event_link(ctx, e, a)?;
        let (jour, minute) = event_due(&l);
        t.due_day = Some(jour);
        t.due_minute = minute;
        t.source = format!("Event: {}", l.title);
        t.event_uid = Some(l.uid);
        t.event_start = Some(l.occurrence);
    }
    if let Some(d) = a.opt_str("due_day")?.filter(|s| !s.trim().is_empty()) {
        t.due_day = Some(day_text(time::parse_day(&d)?));
        t.due_minute = None;
    }
    let heure = a.opt_str("due_time")?.filter(|s| !s.trim().is_empty());
    if let Some(h) = &heure {
        t.due_minute = Some(parse_hhmm("due_time", h)?);
        if t.due_day.is_none() {
            t.due_day = Some(day_text(time::today()));
        }
    }
    if let Some(g) = a.opt_i64("goal_id")? {
        t.goal_id = Some(check_goal(ctx, g)?);
    }
    if let Some(r) = a.opt_str("repeat")? {
        t.repeat = repeat_rule(&r)?;
        if t.repeat.is_some() && t.due_day.is_none() {
            t.due_day = Some(day_text(time::today()));
        }
    }
    if let Some(th) = a.opt_i64("thread_id")? {
        t.source = thread_source(ctx, th)?;
        t.thread_id = Some(th);
    }
    // A time said is an appointment: reminded at that time, as the add bar does.
    t.remind_before = match a.opt_i64("remind_before")? {
        Some(m) if m < 0 => None,
        Some(m) => Some(m.min(60 * 24 * 30) as i32),
        None if a.has("remind_before") => None,
        None => heure.as_ref().map(|_| 0),
    };
    crate::tasks::recalculer_rappel(&mut t);
    let id = store(ctx).insert_task(&t, ctx.now())?;
    ctx.refresh();
    task_now(ctx, id)
}

fn update_task(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let avant = get_task(ctx, id)?;
    let mut t = avant.task.clone();
    if let Some(s) = a.opt_str("title")? {
        t.title = non_empty("title", &s)?;
    }
    if a.has("notes") {
        t.notes = a.opt_str("notes")?.unwrap_or_default();
    }
    if a.has("parent_id") {
        match a.opt_i64("parent_id")? {
            None => t.parent_id = None,
            Some(p) => {
                if p == id {
                    return Err(ToolError::new("a task cannot be its own subtask"));
                }
                let parent = parent_task(ctx, p)?;
                if !store(ctx).subtasks(id)?.is_empty() {
                    return Err(ToolError::new(format!(
                        "task {id} has subtasks of its own: it cannot become a subtask"
                    )));
                }
                t.parent_id = Some(p);
                t.list_id = parent.task.list_id;
            }
        }
    }
    if let Some(l) = a.opt_i64("list_id")? {
        if !store(ctx).task_lists()?.iter().any(|x| x.id == l) {
            return Err(not_found("task list", l));
        }
        if t.parent_id.is_some() && l != t.list_id {
            return Err(ToolError::new(
                "a subtask stays in its task's list: move its parent task instead",
            ));
        }
        t.list_id = l;
    }
    if a.has("due_day") {
        match a.opt_str("due_day")?.filter(|s| !s.trim().is_empty()) {
            None => {
                t.due_day = None;
                t.due_minute = None;
            }
            Some(d) => {
                let nouveau = day_text(time::parse_day(&d)?);
                // Moved to a later day: put off, as Later counts it.
                if t.due_day.as_ref().is_some_and(|ancien| nouveau > *ancien) {
                    t.postponed += 1;
                }
                t.due_day = Some(nouveau);
            }
        }
    }
    if a.has("due_time") {
        match a.opt_str("due_time")?.filter(|s| !s.trim().is_empty()) {
            None => t.due_minute = None,
            Some(h) => {
                t.due_minute = Some(parse_hhmm("due_time", &h)?);
                if t.due_day.is_none() {
                    t.due_day = Some(day_text(time::today()));
                }
            }
        }
    }
    if a.has("priority") {
        t.priority = priority(a)?.unwrap_or(0);
    }
    if a.has("goal_id") {
        t.goal_id = match a.opt_i64("goal_id")? {
            None => None,
            Some(g) => Some(check_goal(ctx, g)?),
        };
    }
    if a.has("estimate") {
        t.estimate = estimate(a)?;
    }
    if a.has("repeat") {
        t.repeat = match a.opt_str("repeat")? {
            None => None,
            Some(r) => repeat_rule(&r)?,
        };
        if t.repeat.is_some() && t.due_day.is_none() {
            t.due_day = Some(day_text(time::today()));
        }
    }
    if a.has("remind_before") {
        t.remind_before = match a.opt_i64("remind_before")? {
            Some(m) if m >= 0 => Some(m.min(60 * 24 * 30) as i32),
            _ => None,
        };
    }
    if a.has("thread_id") {
        match a.opt_i64("thread_id")? {
            None => {
                if t.thread_id.is_some() {
                    t.source.clear();
                }
                t.thread_id = None;
            }
            Some(th) => {
                t.source = thread_source(ctx, th)?;
                t.thread_id = Some(th);
            }
        }
    }
    if a.has("event_id") {
        match a.opt_i64("event_id")? {
            None => {
                if t.event_uid.is_some() && t.source.starts_with("Event: ") {
                    t.source.clear();
                }
                t.event_uid = None;
                t.event_start = None;
            }
            Some(e) => {
                let l = event_link(ctx, e, a)?;
                t.source = format!("Event: {}", l.title);
                t.event_uid = Some(l.uid);
                t.event_start = Some(l.occurrence);
            }
        }
    }
    crate::tasks::recalculer_rappel(&mut t);
    if t != avant.task {
        store(ctx).update_task(id, &t, ctx.now())?;
        ctx.refresh();
    }
    task_now(ctx, id)
}

fn complete_task(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let fait = a.bool_or("done", true)?;
    let avant = get_task(ctx, id)?;
    if avant.is_done() != fait {
        crate::tasks::toggle_done(&ctx.services, id);
        ctx.refresh();
    }
    let names = Names::load(ctx)?;
    let mut r = json!({"task": task_json(ctx, &get_task(ctx, id)?, &names)});
    if fait && !avant.is_done() && avant.task.repeat.is_some() {
        let suivante = store(ctx).open_tasks()?.into_iter().find(|o| {
            o.id != id
                && o.task.title == avant.task.title
                && o.task.list_id == avant.task.list_id
                && o.task.repeat == avant.task.repeat
                && o.task.due_day > avant.task.due_day
        });
        if let Some(s) = suivante {
            r["next"] = task_json(ctx, &s, &names);
        }
    }
    Ok(r)
}

fn delete_task(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let t = get_task(ctx, id)?;
    let etapes = store(ctx).subtasks(id)?.len();
    store(ctx).delete_task(id)?;
    ctx.refresh();
    Ok(json!({"deleted": id, "title": t.task.title, "subtasks_deleted": etapes}))
}

// --- Habits --------------------------------------------------------------------------

/// A habit and its days, as the rules read it.
struct HabitDays {
    h: iris_store::Habit,
    r: iris_growth::Habit,
}

fn load_habits(ctx: &Ctx) -> Result<Vec<HabitDays>, ToolError> {
    let mut jours: BTreeMap<i64, BTreeMap<NaiveDate, i32>> = BTreeMap::new();
    for c in store(ctx).habit_checks()? {
        if let Some(d) = stored_day(&c.day) {
            jours.entry(c.habit_id).or_default().insert(d, c.amount);
        }
    }
    Ok(store(ctx)
        .habits()?
        .into_iter()
        .map(|h| HabitDays {
            r: iris_growth::Habit {
                schedule: Schedule::parse(&h.habit.schedule),
                amount: h.habit.amount,
                created: time::day_of_ms(h.created_at.millis()),
                checks: jours.remove(&h.id).unwrap_or_default(),
            },
            h,
        })
        .collect())
}

fn find_habit(ctx: &Ctx, id: i64) -> Result<HabitDays, ToolError> {
    load_habits(ctx)?
        .into_iter()
        .find(|u| u.h.id == id)
        .ok_or_else(|| not_found("habit", id))
}

fn day_word(d: Day) -> &'static str {
    match d {
        Day::Done => "done",
        Day::Partial(_) => "partial",
        Day::Missed => "missed",
        Day::Rest => "rest",
        Day::Today => "to do",
        Day::Future => "future",
        Day::Before => "before the habit",
    }
}

fn habit_json(u: &HabitDays, today: NaiveDate, from: NaiveDate, to: NaiveDate) -> Value {
    let h = &u.h.habit;
    let serie = u.r.streak(today);
    let voulu = h.amount.max(1);
    let jours: Vec<Value> =
        u.r.checks
            .range(from..=to)
            .map(|(d, n)| json!({"day": day_text(*d), "amount": n, "done": *n >= voulu}))
            .collect();
    let mut v = json!({
        "id": u.h.id,
        "title": h.title,
        "kind": if h.quit { "quit" } else { "build" },
        "icon": h.icon,
        "color": h.color,
        "schedule": h.schedule,
        "schedule_label": u.r.schedule.label(),
        "amount": h.amount,
        "unit": h.unit,
        "goal_id": h.goal_id,
        "remind_time": h.remind_minute.map(hhmm),
        "created": day_text(u.r.created),
        "today": {
            "due": u.r.due_today(today),
            "done": u.r.done_on(today),
            "amount": u.r.amount_on(today),
            "status": day_word(u.r.day(today, today)),
        },
        "streak": {
            "current": serie.current,
            "best": serie.best,
            "unit": if serie.weeks { "weeks" } else { "days" },
        },
        "range": {
            "from": day_text(from),
            "to": day_text(to),
            "days_done": u.r.done_between(from, to),
            "rate": u.r.rate_between(from, to, today).map(round2),
        },
        "checks": jours,
    });
    if let Schedule::PerWeek(n) = u.r.schedule {
        v["this_week"] = json!({"done": u.r.done_in_week(today), "wanted": n});
    }
    v
}

/// The habit as it reads now, over its last week.
fn habit_now(ctx: &Ctx, id: i64) -> ToolResult {
    let today = time::today();
    Ok(habit_json(
        &find_habit(ctx, id)?,
        today,
        today - Duration::days(6),
        today,
    ))
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// A schedule as an assistant may write it, as it is stored.
fn parse_schedule(s: &str) -> Result<String, ToolError> {
    let t = s.trim().to_ascii_lowercase();
    let faux = || {
        ToolError::new(format!(
            "\"schedule\" must be daily, weekdays, weekends, a list of days (mon,wed,fri), \
             days:1010100 (Monday first) or week:3 (times a week), not \"{s}\""
        ))
    };
    let regle = match t.as_str() {
        "daily" | "every day" | "everyday" => Schedule::Daily,
        "weekdays" => Schedule::days([true, true, true, true, true, false, false]),
        "weekends" => Schedule::days([false, false, false, false, false, true, true]),
        _ => {
            if let Some(bits) = t.strip_prefix("days:") {
                let bits = bits.trim();
                if bits.len() != 7 || !bits.chars().all(|c| c == '0' || c == '1') {
                    return Err(faux());
                }
                let mut a = [false; 7];
                for (i, c) in bits.chars().enumerate() {
                    a[i] = c == '1';
                }
                if !a.iter().any(|x| *x) {
                    return Err(faux());
                }
                Schedule::days(a)
            } else if let Some(n) = t.strip_prefix("week:") {
                let n: u8 = n
                    .trim()
                    .parse()
                    .ok()
                    .filter(|n| (1..=7).contains(n))
                    .ok_or_else(faux)?;
                Schedule::per_week(n)
            } else {
                let mut a = [false; 7];
                for mot in t
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|m| !m.is_empty())
                {
                    let i = DAYS
                        .iter()
                        .position(|d| mot.starts_with(*d))
                        .ok_or_else(faux)?;
                    a[i] = true;
                }
                if !a.iter().any(|x| *x) {
                    return Err(faux());
                }
                Schedule::days(a)
            }
        }
    };
    Ok(regle.text())
}

/// Writes the fields given onto `h`.
fn apply_habit(ctx: &Ctx, a: &Args, h: &mut NewHabit) -> Result<(), ToolError> {
    if let Some(t) = a.opt_str("title")? {
        h.title = non_empty("title", &t)?;
    }
    if let Some(s) = a.opt_str("schedule")? {
        h.schedule = parse_schedule(&s)?;
    }
    if let Some(k) = a.opt_str("kind")? {
        h.quit = match k.trim().to_ascii_lowercase().as_str() {
            "build" => false,
            "quit" => true,
            _ => return Err(ToolError::new("\"kind\" must be build or quit")),
        };
    }
    if let Some(i) = a.opt_str("icon")? {
        let i = i.trim().to_ascii_lowercase();
        if !crate::habits::ICONES.contains(&i.as_str()) {
            return Err(ToolError::new(format!(
                "\"icon\" must be one of {}",
                crate::habits::ICONES.join(", ")
            )));
        }
        h.icon = i;
    }
    if let Some(c) = a.opt_str("color")? {
        h.color = color(&c)?;
    }
    if a.has("amount") {
        let n = a.opt_i64("amount")?.unwrap_or(0);
        if !(0..=100_000).contains(&n) {
            return Err(ToolError::new("\"amount\" must be from 0 to 100000"));
        }
        h.amount = n as i32;
    }
    if a.has("unit") {
        h.unit = a.opt_str("unit")?.unwrap_or_default().trim().to_string();
    }
    if a.has("goal_id") {
        h.goal_id = match a.opt_i64("goal_id")? {
            None => None,
            Some(g) => Some(check_goal(ctx, g)?),
        };
    }
    if a.has("remind_time") {
        h.remind_minute = match a.opt_str("remind_time")?.filter(|s| !s.trim().is_empty()) {
            None => None,
            Some(s) => Some(parse_hhmm("remind_time", &s)?),
        };
    }
    // As the habit window: no amount, no unit.
    if h.amount == 0 {
        h.unit.clear();
    }
    Ok(())
}

fn list_habits(ctx: &Ctx, a: &Args) -> ToolResult {
    let today = time::today();
    let to = match a.opt_str("to")? {
        Some(s) => time::parse_day(&s)?,
        None => today,
    };
    let from = match a.opt_str("from")? {
        Some(s) => time::parse_day(&s)?,
        None => to - Duration::days(29),
    };
    if from > to {
        return Err(ToolError::new("\"from\" comes after \"to\""));
    }
    if (to - from).num_days() > 400 {
        return Err(ToolError::new("ask for 400 days at most"));
    }
    let seule = a.opt_i64("habit_id")?;
    let habitudes: Vec<Value> = load_habits(ctx)?
        .iter()
        .filter(|u| seule.is_none_or(|id| u.h.id == id))
        .map(|u| habit_json(u, today, from, to))
        .collect();
    if let Some(id) = seule {
        if habitudes.is_empty() {
            return Err(not_found("habit", id));
        }
    }
    Ok(json!({"today": day_text(today), "habits": habitudes}))
}

fn create_habit(ctx: &Ctx, a: &Args) -> ToolResult {
    non_empty("title", &a.str("title")?)?;
    let n = store(ctx).habits()?.len();
    let mut h = NewHabit {
        icon: "sparkle".into(),
        color: crate::habits::COULEURS[n % crate::habits::COULEURS.len()].into(),
        schedule: Schedule::Daily.text(),
        ..Default::default()
    };
    apply_habit(ctx, a, &mut h)?;
    let id = store(ctx).create_habit(&h, ctx.now())?;
    ctx.refresh();
    habit_now(ctx, id)
}

fn update_habit(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let avant = store(ctx)
        .habit(id)?
        .ok_or_else(|| not_found("habit", id))?;
    let mut h = avant.habit.clone();
    apply_habit(ctx, a, &mut h)?;
    if h != avant.habit {
        store(ctx).update_habit(id, &h)?;
        ctx.refresh();
    }
    habit_now(ctx, id)
}

fn delete_habit(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let h = store(ctx)
        .habit(id)?
        .ok_or_else(|| not_found("habit", id))?;
    store(ctx).delete_habit(id)?;
    ctx.refresh();
    Ok(json!({"deleted": id, "title": h.habit.title}))
}

fn check_habit(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("habit_id")?;
    let u = find_habit(ctx, id)?;
    let today = time::today();
    let jour = match a.opt_str("day")? {
        Some(s) => time::parse_day(&s)?,
        None => today,
    };
    if jour > today {
        return Err(ToolError::new("a habit cannot be ticked for a day to come"));
    }
    if jour < u.r.created {
        return Err(ToolError::new(format!(
            "habit {id} was made on {}: days before it cannot be ticked",
            day_text(u.r.created)
        )));
    }
    let combien = match a.opt_i64("amount")? {
        Some(n) if (0..=100_000).contains(&n) => n as i32,
        Some(_) => return Err(ToolError::new("\"amount\" must be from 0 to 100000")),
        None => u.h.habit.amount.max(1),
    };
    store(ctx).set_habit_check(id, &day_text(jour), combien)?;
    ctx.refresh();
    let u = find_habit(ctx, id)?;
    let mut v = habit_json(&u, today, jour.min(today - Duration::days(6)), today);
    v["checked"] = json!({
        "day": day_text(jour),
        "amount": u.r.amount_on(jour),
        "done": u.r.done_on(jour),
    });
    Ok(v)
}

// --- Goals ---------------------------------------------------------------------------

fn goal_json(ctx: &Ctx, g: &Goal, today: NaiveDate) -> Value {
    let (fait, cible, p) = crate::goals::allure(&ctx.services, g, today);
    let jalons = g.goal.kind == GoalKind::Milestones;
    let ouvertes = store(ctx)
        .tasks_for_goal(g.id)
        .map(|v| v.iter().filter(|t| !t.is_done()).count())
        .unwrap_or(0);
    json!({
        "id": g.id,
        "title": g.goal.title,
        "why": g.goal.why,
        "kind": if jalons { "milestones" } else { "count" },
        "target": if jalons { Value::Null } else { json!(g.goal.target) },
        "unit": if jalons { "milestones" } else { g.goal.unit.as_str() },
        "due_day": g.goal.due_day,
        "color": g.goal.color,
        "created": day_text(time::day_of_ms(g.created_at.millis())),
        "progress": {
            "done": fait,
            "of": cible,
            "fraction": round2(fait as f32 / cible.max(1) as f32),
            "status": match p.status {
                iris_tasks::goals::PaceStatus::OnTrack => "on_track",
                iris_tasks::goals::PaceStatus::Behind => "behind",
                iris_tasks::goals::PaceStatus::Reached => "reached",
            },
            "badge": p.badge(),
            "pace": p.text,
            "expected_by_now": round2(p.expected),
            "behind": p.behind,
            "days_left": p.days_left,
            "left": p.left,
        },
        "open_tasks": ouvertes,
    })
}

/// The goal as it reads now, with its log or milestones.
fn goal_now(ctx: &Ctx, id: i64) -> ToolResult {
    let g = get_goal(ctx, id)?;
    let mut v = goal_json(ctx, &g, time::today());
    match g.goal.kind {
        GoalKind::Count => {
            v["entries"] = Value::Array(
                store(ctx)
                    .goal_entries(id)?
                    .iter()
                    .map(|e| json!({"id": e.id, "at": time::format_ms(e.at.millis()), "note": e.note}))
                    .collect(),
            );
        }
        GoalKind::Milestones => {
            v["milestones"] = Value::Array(
                store(ctx)
                    .milestones(id)?
                    .iter()
                    .map(|m| {
                        json!({
                            "id": m.id,
                            "title": m.title,
                            "done": m.done_at.is_some(),
                            "done_at": m.done_at.map(|t| time::format_ms(t.millis())),
                        })
                    })
                    .collect(),
            );
        }
    }
    Ok(v)
}

fn list_goals(ctx: &Ctx) -> ToolResult {
    let today = time::today();
    let objectifs: Vec<Value> = store(ctx)
        .goals()?
        .iter()
        .map(|g| goal_json(ctx, g, today))
        .collect();
    Ok(json!({"today": day_text(today), "goals": objectifs}))
}

fn get_goal_tool(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let mut v = goal_now(ctx, id)?;
    let names = Names::load(ctx)?;
    v["tasks"] = Value::Array(
        store(ctx)
            .tasks_for_goal(id)?
            .iter()
            .map(|t| task_json(ctx, t, &names))
            .collect(),
    );
    v["habits"] = Value::Array(
        store(ctx)
            .habits()?
            .iter()
            .filter(|h| h.habit.goal_id == Some(id))
            .map(|h| {
                json!({
                    "id": h.id,
                    "title": h.habit.title,
                    "schedule": Schedule::parse(&h.habit.schedule).label(),
                })
            })
            .collect(),
    );
    Ok(v)
}

fn goal_target(a: &Args) -> Result<i32, ToolError> {
    let n = a.i64("target")?;
    if (1..=100_000).contains(&n) {
        Ok(n as i32)
    } else {
        Err(ToolError::new("\"target\" must be from 1 to 100000"))
    }
}

fn create_goal(ctx: &Ctx, a: &Args) -> ToolResult {
    let title = non_empty("title", &a.str("title")?)?;
    let jalons = match a
        .opt_str("kind")?
        .map(|k| k.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("") | Some("count") => false,
        Some("milestones") => true,
        Some(_) => return Err(ToolError::new("\"kind\" must be count or milestones")),
    };
    let jour = time::parse_day(&a.str("due_day")?)?;
    if jour <= time::today() {
        return Err(ToolError::new("\"due_day\" must be after today"));
    }
    let target = if jalons { 1 } else { goal_target(a)? };
    let unite = a.opt_str("unit")?.unwrap_or_default().trim().to_string();
    let n = store(ctx).goals()?.len();
    let objectif = NewGoal {
        title,
        why: a.opt_str("why")?.unwrap_or_default().trim().to_string(),
        kind: if jalons {
            GoalKind::Milestones
        } else {
            GoalKind::Count
        },
        target,
        unit: if unite.is_empty() && !jalons {
            "times".into()
        } else {
            unite
        },
        due_day: day_text(jour),
        color: match a.opt_str("color")? {
            Some(c) => color(&c)?,
            None => crate::goals::COULEURS[n % crate::goals::COULEURS.len()].into(),
        },
    };
    let etapes: Vec<String> = a
        .str_list("milestones")?
        .into_iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .collect();
    if !jalons && !etapes.is_empty() {
        return Err(ToolError::new(
            "milestones are for a goal of kind milestones",
        ));
    }
    let id = store(ctx).create_goal(&objectif, ctx.now())?;
    for m in &etapes {
        store(ctx).add_milestone(id, m)?;
    }
    ctx.refresh();
    goal_now(ctx, id)
}

fn update_goal(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let avant = get_goal(ctx, id)?;
    let mut g = avant.goal.clone();
    let jalons = g.kind == GoalKind::Milestones;
    if let Some(t) = a.opt_str("title")? {
        g.title = non_empty("title", &t)?;
    }
    if a.has("why") {
        g.why = a.opt_str("why")?.unwrap_or_default().trim().to_string();
    }
    if let Some(d) = a.opt_str("due_day")? {
        let jour = time::parse_day(&d)?;
        if jour < time::today() {
            return Err(ToolError::new("\"due_day\" cannot be in the past"));
        }
        g.due_day = day_text(jour);
    }
    if a.get("target").is_some() {
        if jalons {
            return Err(ToolError::new(
                "a milestones goal has no target: add milestones with goals_milestone",
            ));
        }
        g.target = goal_target(a)?;
    }
    if let Some(u) = a.opt_str("unit")?.filter(|u| !u.trim().is_empty()) {
        if jalons {
            return Err(ToolError::new("a milestones goal has no unit"));
        }
        g.unit = u.trim().to_string();
    }
    if let Some(c) = a.opt_str("color")? {
        g.color = color(&c)?;
    }
    if g != avant.goal {
        store(ctx).update_goal(id, &g)?;
        ctx.refresh();
    }
    goal_now(ctx, id)
}

fn delete_goal(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("id")?;
    let g = get_goal(ctx, id)?;
    store(ctx).delete_goal(id)?;
    ctx.refresh();
    Ok(json!({"deleted": id, "title": g.goal.title}))
}

fn log_goal(ctx: &Ctx, a: &Args) -> ToolResult {
    let id = a.i64("goal_id")?;
    let g = get_goal(ctx, id)?;
    if let Some(e) = a.opt_i64("delete_entry_id")? {
        if !store(ctx).goal_entries(id)?.iter().any(|x| x.id == e) {
            return Err(ToolError::new(format!("goal {id} has no log entry {e}")));
        }
        store(ctx).delete_goal_entry(e)?;
        ctx.refresh();
        return goal_now(ctx, id);
    }
    if g.goal.kind == GoalKind::Milestones {
        return Err(ToolError::new(format!(
            "goal {id} is made of milestones: tick them with goals_milestone"
        )));
    }
    let note = a.opt_str("note")?.unwrap_or_default().trim().to_string();
    let combien = a.opt_i64("count")?.unwrap_or(1);
    if !(1..=1000).contains(&combien) {
        return Err(ToolError::new("\"count\" must be from 1 to 1000"));
    }
    let quand = match a.opt_str("at")? {
        Some(s) => Timestamp::from_millis(time::parse_ms(&s)?),
        None => ctx.now(),
    };
    for _ in 0..combien {
        store(ctx).log_goal(id, &note, quand)?;
    }
    ctx.refresh();
    goal_now(ctx, id)
}

/// The goal a milestone belongs to, and the milestone.
fn find_milestone(
    ctx: &Ctx,
    id: i64,
    goal: Option<i64>,
) -> Result<(i64, iris_store::Milestone), ToolError> {
    let objectifs: Vec<i64> = match goal {
        Some(g) => vec![get_goal(ctx, g)?.id],
        None => store(ctx).goals()?.iter().map(|g| g.id).collect(),
    };
    for g in objectifs {
        if let Some(m) = store(ctx).milestones(g)?.into_iter().find(|m| m.id == id) {
            return Ok((g, m));
        }
    }
    Err(not_found("milestone", id))
}

fn milestone(ctx: &Ctx, a: &Args) -> ToolResult {
    if let Some(titre) = a.opt_str("add")? {
        let titre = non_empty("add", &titre)?;
        let gid = a.i64("goal_id")?;
        let g = get_goal(ctx, gid)?;
        if g.goal.kind != GoalKind::Milestones {
            return Err(ToolError::new(format!(
                "goal {gid} is counted: log progress with goals_log"
            )));
        }
        store(ctx).add_milestone(gid, &titre)?;
        ctx.refresh();
        return goal_now(ctx, gid);
    }
    let Some(mid) = a.opt_i64("id")? else {
        return Err(ToolError::new(
            "give goal_id and add (a title), or id with done or delete",
        ));
    };
    let (gid, _) = find_milestone(ctx, mid, a.opt_i64("goal_id")?)?;
    if a.bool_or("delete", false)? {
        store(ctx).delete_milestone(mid)?;
    } else if let Some(fait) = a.opt_bool("done")? {
        store(ctx).set_milestone_done(mid, fait.then(|| ctx.now()))?;
    } else {
        return Err(ToolError::new(
            "with id, give done (true/false) or delete: true",
        ));
    }
    ctx.refresh();
    goal_now(ctx, gid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ctx: &Ctx, name: &str, args: Value) -> Value {
        call(ctx, name, &Args::new(args)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    fn id_of(v: &Value) -> i64 {
        v["id"].as_i64().expect("an id")
    }

    #[test]
    fn every_tool_is_answered() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        for t in tools() {
            let r = call(&ctx, t.name, &Args::default());
            if let Err(e) = r {
                assert!(
                    !e.0.starts_with("unknown tool"),
                    "{} is not dispatched",
                    t.name
                );
            }
        }
    }

    #[test]
    fn a_task_is_made_ticked_and_listed() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        let liste = id_of(&run(&ctx, "tasks_create_list", json!({"name": "Work"})));
        let t = run(
            &ctx,
            "tasks_create",
            json!({"title": "Send the report", "list_id": liste, "due_day": "today",
                   "due_time": "16:00", "priority": "high"}),
        );
        let id = id_of(&t);
        assert_eq!(t["list"], "Work");
        assert_eq!(t["due_time"], "16:00");
        assert_eq!(t["priority"], 3);
        assert_eq!(t["remind_before_minutes"], 0);

        let etape = run(
            &ctx,
            "tasks_create",
            json!({"title": "Check the figures", "parent_id": id}),
        );
        assert_eq!(etape["list_id"], liste, "a subtask sits in its task's list");
        assert_eq!(
            run(&ctx, "tasks_get", json!({"id": id}))["subtasks"][0]["title"],
            "Check the figures"
        );

        // Without a list: the first one.
        let ailleurs = run(&ctx, "tasks_create", json!({"title": "Buy bread"}));
        assert_ne!(ailleurs["list_id"], liste);

        let ouvertes = run(
            &ctx,
            "tasks_list",
            json!({"list_id": liste, "parent_id": null}),
        );
        assert_eq!(ouvertes["count"], 2);
        run(&ctx, "tasks_complete", json!({"id": id}));
        let restent = run(&ctx, "tasks_list", json!({"list_id": liste}));
        assert_eq!(restent["count"], 1, "only the subtask stays open");
        let toutes = run(
            &ctx,
            "tasks_list",
            json!({"list_id": liste, "include_done": true}),
        );
        assert_eq!(toutes["count"], 2);
        assert_eq!(toutes["tasks"][1]["done"], true);

        let listes = run(&ctx, "tasks_lists", json!({}));
        assert_eq!(listes["lists"].as_array().map(Vec::len), Some(2));

        let change = run(
            &ctx,
            "tasks_update",
            json!({"id": ailleurs["id"], "due_day": "2026-12-01", "notes": "the round one"}),
        );
        assert_eq!(change["due_day"], "2026-12-01");
        let sans = run(
            &ctx,
            "tasks_update",
            json!({"id": ailleurs["id"], "due_day": null}),
        );
        assert_eq!(sans["due_day"], Value::Null);
        assert_eq!(sans["notes"], "the round one");
    }

    #[test]
    fn a_repeating_task_ticked_makes_the_next() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        let t = run(
            &ctx,
            "tasks_create",
            json!({"title": "Water the plants", "repeat": "daily"}),
        );
        assert_eq!(t["due_day"], day_text(time::today()));
        let r = run(&ctx, "tasks_complete", json!({"id": t["id"]}));
        assert_eq!(r["task"]["done"], true);
        assert_eq!(
            r["next"]["due_day"],
            day_text(time::today() + Duration::days(1))
        );
    }

    #[test]
    fn a_habit_ticked_today_shows_done() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        let h = run(
            &ctx,
            "habits_create",
            json!({"title": "Read", "schedule": "daily", "amount": 20, "unit": "pages"}),
        );
        let id = id_of(&h);
        assert_eq!(h["today"]["done"], false);
        run(&ctx, "habits_check", json!({"habit_id": id}));
        let liste = run(&ctx, "habits_list", json!({}));
        let une = &liste["habits"][0];
        assert_eq!(une["today"]["done"], true);
        assert_eq!(une["today"]["amount"], 20);
        assert_eq!(une["streak"]["current"], 1);
        assert!(call(
            &ctx,
            "habits_check",
            &Args::new(json!({"habit_id": id, "day": "tomorrow"}))
        )
        .is_err());
        let jour = habits_summary(&ctx, time::today()).unwrap();
        assert_eq!(jour[0]["done"], true);

        assert_eq!(parse_schedule("mon, wed, fri").unwrap(), "days:1010100");
        assert_eq!(parse_schedule("week:3").unwrap(), "week:3");
        assert!(parse_schedule("sometimes").is_err());
    }

    #[test]
    fn a_goal_is_logged_and_its_milestones_ticked() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        let dans_un_mois = day_text(time::today() + Duration::days(30));
        let g = run(
            &ctx,
            "goals_create",
            json!({"title": "Send 3 applications", "target": 3, "unit": "applications",
                   "due_day": dans_un_mois}),
        );
        let id = id_of(&g);
        run(
            &ctx,
            "goals_log",
            json!({"goal_id": id, "note": "Studio Nord"}),
        );
        let lu = run(&ctx, "goals_get", json!({"id": id}));
        assert_eq!(lu["progress"]["done"], 1);
        assert_eq!(lu["progress"]["of"], 3);
        assert_eq!(lu["entries"][0]["note"], "Studio Nord");

        let m = run(
            &ctx,
            "goals_create",
            json!({"title": "Thesis", "kind": "milestones", "due_day": dans_un_mois,
                   "milestones": ["Draft", "Review"]}),
        );
        let premier = m["milestones"][0]["id"].clone();
        let apres = run(
            &ctx,
            "goals_milestone",
            json!({"id": premier, "done": true}),
        );
        assert_eq!(apres["progress"]["done"], 1);
        assert_eq!(apres["progress"]["of"], 2);
        assert_eq!(
            run(&ctx, "goals_list", json!({}))["goals"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
    }
}
