//! L'onglet des tâches : ce qu'il y a à faire, venu du courrier ou d'ailleurs.
//!
//! Quatre vues et les listes de l'utilisateur. « Today » rassemble les tâches dues
//! ou en retard ; « All tasks », toutes. Une tâche faite reste, barrée, dans la vue
//! d'où on l'a cochée, jusqu'à ce qu'on vide les tâches terminées. Une conversation
//! devient une tâche d'une touche (`T`), et la tâche la rouvre d'un clic. Une tâche se
//! porte à la souris sur une liste, ou sur Today.

use crate::controller::{Controller, Request};
use crate::services::{now, Services};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use iris_store::{NewTask, StoredTask, TaskList};
use iris_tasks::{due_label, is_overdue, remind_at, section, Section, REMINDERS};
use iris_types::ThreadId;
use iris_ui::{
    AppWindow, HomeItemData, SubtaskData, TaskDetailData, TaskOverviewData, TaskPlaceData,
    TaskRowData, TaskTokenData,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Les couleurs données aux nouvelles listes, tour à tour.
const COULEURS: [&str; 8] = [
    "#5b8def", "#e0795b", "#4fb286", "#b67be6", "#e3b341", "#e0608c", "#3fb1c9", "#8a9a5b",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vue {
    Today,
    Upcoming,
    Anytime,
    Mail,
    List(i64),
    /// A goal's page: where it stands, and the steps toward it.
    Goal(i64),
    /// All the goals.
    Goals,
    /// The week in review: what was done, put off or left late, what comes next week,
    /// and the goals under way.
    Week,
}

impl Vue {
    fn cle(self) -> String {
        match self {
            Vue::Today => "today".into(),
            Vue::Upcoming => "upcoming".into(),
            Vue::Anytime => "anytime".into(),
            Vue::Mail => "mail".into(),
            Vue::List(id) => format!("list:{id}"),
            Vue::Goal(id) => format!("goal:{id}"),
            Vue::Goals => "goals".into(),
            Vue::Week => "week".into(),
        }
    }

    fn depuis(cle: &str) -> Option<Vue> {
        Some(match cle {
            "today" => Vue::Today,
            "upcoming" => Vue::Upcoming,
            "anytime" => Vue::Anytime,
            "mail" => Vue::Mail,
            "goals" => Vue::Goals,
            "week" => Vue::Week,
            autre => match autre.strip_prefix("goal:") {
                Some(id) => Vue::Goal(id.parse().ok()?),
                None => Vue::List(autre.strip_prefix("list:")?.parse().ok()?),
            },
        })
    }
}

/// Ce que l'onglet montre. Sur le fil de l'interface.
struct Etat {
    vue: Vue,
    /// La tâche ouverte à droite.
    choisie: Option<i64>,
    /// La tâche dont les champs du panneau ont été remplis : on ne les réécrit pas
    /// pendant qu'on y tape.
    remplie: Option<i64>,
    /// Le mois du petit calendrier, quand il est ouvert.
    mois: Option<NaiveDate>,
    /// Les tâches de la colonne, dans l'ordre, pour les flèches.
    ordre: Vec<i64>,
    listes: Vec<TaskList>,
    /// What was deleted, most recent last, each with its subtasks: Ctrl+Z puts the
    /// last of them back.
    supprimees: Vec<Vec<StoredTask>>,
    /// The task Later is open for.
    plus_tard: Option<i64>,
    /// The free stretches offered for the task shown, in minutes since midnight.
    creneaux: Vec<i32>,
}

/// A task and its subtasks as they are before a delete, to put them back.
fn avec_ses_etapes(services: &Services, id: i64) -> Vec<StoredTask> {
    let Ok(Some(t)) = services.store.task(id) else {
        return Vec::new();
    };
    let mut tout = vec![t];
    tout.extend(services.store.subtasks(id).unwrap_or_default());
    tout
}

/// Shows a task in the Tasks tab, among all of them, opened on the right.
pub fn show_task(f: &AppWindow, id: i64) {
    // One step back for the whole move: the view first, so the change of workspace
    // that follows lands on the same place.
    crate::nav::note(f, "tasks", "anytime");
    f.set_workspace(2);
    f.invoke_workspace_changed(2);
    f.invoke_task_place_chosen("anytime".into());
    f.invoke_task_row_selected(TaskRowData {
        kind: 0,
        id: id as i32,
        ..Default::default()
    });
}

// --- Le temps ----------------------------------------------------------------------

fn maintenant_local() -> NaiveDateTime {
    Local::now().naive_local()
}

/// Une heure locale en millisecondes. Une heure qui n'existe pas (le saut de
/// printemps) prend la suivante qui existe.
fn vers_ms(t: NaiveDateTime) -> i64 {
    (0..3)
        .find_map(|h| {
            Local
                .from_local_datetime(&(t + Duration::hours(h)))
                .earliest()
                .map(|d| d.timestamp_millis())
        })
        .unwrap_or(0)
}

fn jour(t: &NewTask) -> Option<NaiveDate> {
    t.due_day
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

fn minute(t: &NewTask) -> Option<u32> {
    t.due_minute.map(|m| m.clamp(0, 24 * 60 - 1) as u32)
}

/// Recalcule l'instant du rappel après un changement d'échéance ou de rappel.
fn recalculer_rappel(t: &mut NewTask) {
    t.remind_at = match (jour(t), t.remind_before) {
        (Some(j), Some(avant)) => Some(vers_ms(remind_at(j, minute(t), avant as i64))),
        _ => None,
    };
}

/// Lit une heure : `9:30`, `09:30`, `9h30`, `9h`, `9`.
fn lire_heure(texte: &str) -> Option<u32> {
    let t = texte.trim().to_ascii_lowercase().replace('h', ":");
    let (h, m) = match t.split_once(':') {
        Some((h, m)) => (h.trim(), if m.trim().is_empty() { "0" } else { m.trim() }),
        None => (t.as_str(), "0"),
    };
    let heure = NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)?;
    Some(((heure - NaiveTime::MIN).num_minutes()) as u32)
}

fn heure_texte(m: Option<u32>) -> String {
    m.map(|m| format!("{:02}:{:02}", m / 60, m % 60))
        .unwrap_or_default()
}

fn premier_du_mois(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(chrono::Datelike::year(&d), chrono::Datelike::month(&d), 1).unwrap_or(d)
}

fn plus_mois(d: NaiveDate, n: i32) -> NaiveDate {
    let premier = premier_du_mois(d);
    if n >= 0 {
        premier + chrono::Months::new(n as u32)
    } else {
        premier - chrono::Months::new((-n) as u32)
    }
}

// --- L'affichage ---------------------------------------------------------------------

fn titre_vue(vue: Vue, listes: &[TaskList]) -> String {
    match vue {
        Vue::Today => "Today".into(),
        Vue::Upcoming => "Upcoming".into(),
        Vue::Anytime => "All tasks".into(),
        Vue::Mail => "From mail".into(),
        Vue::List(id) => listes
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.name.clone())
            .unwrap_or_default(),
        Vue::Goal(_) => String::new(),
        Vue::Goals => "Goals".into(),
        Vue::Week => "This week".into(),
    }
}

fn indication(vue: Vue) -> &'static str {
    match vue {
        Vue::Today => "Add a task for today, e.g. “Call Marie at 3pm !!”",
        Vue::Goal(_) => "Add a step toward this goal",
        _ => "Add a task, e.g. “tomorrow 9am Send the quote #Work”",
    }
}

/// Does a task belong to a view? The same rule for the tasks to do and for the
/// completed ones: a task done from Today stays in Today, struck through, until the
/// completed tasks are cleared from there.
fn dans_la_vue(vue: Vue, t: &StoredTask, today: NaiveDate) -> bool {
    match vue {
        Vue::Today => jour(&t.task).is_some_and(|j| j <= today),
        Vue::Upcoming => jour(&t.task).is_some_and(|j| j > today),
        Vue::Anytime => true,
        Vue::Mail => t.task.thread_id.is_some(),
        Vue::List(id) => t.task.list_id == id,
        Vue::Goal(id) => t.task.goal_id == Some(id),
        // The review lays out its own sections; nothing belongs to it as such.
        Vue::Goals | Vue::Week => false,
    }
}

/// The completed tasks of a view, the most recently done first.
fn terminees(services: &Services, vue: Vue, today: NaiveDate) -> Vec<StoredTask> {
    services
        .store
        .done_tasks(None, 500)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| dans_la_vue(vue, t, today))
        .collect()
}

fn section_ligne(titre: &str, compte: usize, rouge: bool) -> TaskRowData {
    TaskRowData {
        kind: 1,
        title: titre.into(),
        meta: if compte > 0 {
            compte.to_string().into()
        } else {
            SharedString::default()
        },
        overdue: rouge,
        ..Default::default()
    }
}

/// "30 min", "1 h", "1 h 30".
fn duree(ms: i64) -> String {
    let minutes = (ms / 60_000).max(1);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

/// Around the tasks of a view: how far it has got and, on Today, the day itself and
/// the week behind it.
fn apercu(
    services: &Services,
    vue: Vue,
    total: usize,
    faites: usize,
    maintenant: NaiveDateTime,
) -> TaskOverviewData {
    let today = maintenant.date();
    let instant = vers_ms(maintenant);

    // What the calendars hold today, the next one in bold.
    let mut suivant = false;
    let agenda: Vec<HomeItemData> = if vue == Vue::Today {
        crate::calendar::upcoming(services, today, 1)
            .into_iter()
            .filter(|u| u.day == today)
            .take(5)
            .map(|u| {
                let a_venir = !u.all_day && u.start > instant;
                let premier = a_venir && !suivant;
                suivant |= a_venir;
                let mut detail = if premier {
                    crate::home::in_how_long(u.start - instant)
                } else if u.all_day {
                    String::new()
                } else {
                    duree(u.end - u.start)
                };
                if !u.location.trim().is_empty() {
                    if !detail.is_empty() {
                        detail.push_str(", ");
                    }
                    detail.push_str(u.location.trim());
                }
                HomeItemData {
                    key: u.key.as_str().into(),
                    title: u.title.as_str().into(),
                    meta: if u.all_day {
                        "All day".into()
                    } else {
                        u.time.as_str().into()
                    },
                    hint: detail.into(),
                    color: crate::calendar::couleur(&u.color),
                    past: u.past,
                    now: premier,
                    ..Default::default()
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    // Done this week, Monday to Sunday.
    let lundi = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let mut jours = [0usize; 7];
    for t in services.store.done_tasks(None, 500).unwrap_or_default() {
        let Some(fait) = t.done_at else { continue };
        let Some(j) = Local
            .timestamp_millis_opt(fait.0)
            .single()
            .map(|d| d.date_naive())
        else {
            continue;
        };
        let ecart = (j - lundi).num_days();
        if (0..7).contains(&ecart) {
            jours[ecart as usize] += 1;
        }
    }
    let plus = jours.iter().copied().max().unwrap_or(0).max(1) as f32;

    TaskOverviewData {
        progress: if total > 0 {
            format!("{faites} of {total} done").into()
        } else {
            SharedString::default()
        },
        fraction: if total > 0 {
            faites as f32 / total as f32
        } else {
            0.0
        },
        show_day: vue == Vue::Today,
        weekday: today.format("%a").to_string().into(),
        day: today.format("%-d").to_string().into(),
        month: today.format("%B").to_string().into(),
        agenda: ModelRc::new(VecModel::from(agenda)),
        week_done: jours.iter().sum::<usize>() as i32,
        week_bars: ModelRc::new(VecModel::from(
            jours.iter().map(|&n| n as f32 / plus).collect::<Vec<_>>(),
        )),
        week_today: today.weekday().num_days_from_monday() as i32,
    }
}

/// What the add line understood so far: a date, a list, a priority.
fn jetons(texte: &str, listes: &[TaskList], maintenant: NaiveDateTime) -> Vec<TaskTokenData> {
    if texte.trim().is_empty() {
        return Vec::new();
    }
    let q = iris_tasks::parse(texte, maintenant);
    let mut jetons = Vec::new();
    if let Some(d) = q.due {
        jetons.push(TaskTokenData {
            kind: 0,
            text: due_label(d.day, d.minute, maintenant.date()).into(),
            ..Default::default()
        });
    }
    if let Some(nom) = &q.list {
        let liste = listes.iter().find(|l| l.name.eq_ignore_ascii_case(nom));
        jetons.push(TaskTokenData {
            kind: 1,
            text: liste.map_or(nom.as_str(), |l| l.name.as_str()).into(),
            color: crate::calendar::couleur(
                liste.map_or(COULEURS[listes.len() % COULEURS.len()], |l| {
                    l.color.as_str()
                }),
            ),
            ..Default::default()
        });
    }
    if q.priority > 0 {
        let p = q.priority.min(3);
        jetons.push(TaskTokenData {
            kind: 2,
            text: ["", "Low", "Medium", "High"][p as usize].into(),
            level: p as i32,
            ..Default::default()
        });
    }
    jetons
}

/// Une tâche en ligne de colonne.
fn ligne_tache(
    services: &Services,
    t: &StoredTask,
    etat: &Etat,
    avec_liste: bool,
    maintenant: NaiveDateTime,
) -> TaskRowData {
    let (faites, total) = services
        .store
        .subtask_progress(t.id)
        .map(|(total, faites)| (faites, total))
        .unwrap_or((0, 0));
    let liste = etat.listes.iter().find(|l| l.id == t.task.list_id);
    let objectif = t
        .task
        .goal_id
        .and_then(|g| services.store.goal(g).ok().flatten());
    let (meta, retard) = match jour(&t.task) {
        Some(j) => (
            due_label(j, minute(&t.task), maintenant.date()),
            !t.is_done() && is_overdue(j, minute(&t.task), maintenant),
        ),
        None => (String::new(), false),
    };
    TaskRowData {
        kind: 0,
        id: t.id as i32,
        key: SharedString::default(),
        title: t.task.title.as_str().into(),
        meta: meta.into(),
        overdue: retard,
        done: t.is_done(),
        priority: t.task.priority,
        list: if avec_liste {
            liste.map(|l| l.name.as_str()).unwrap_or("").into()
        } else {
            SharedString::default()
        },
        color: crate::calendar::couleur(liste.map(|l| l.color.as_str()).unwrap_or("")),
        from_mail: t.task.thread_id.is_some(),
        has_notes: !t.task.notes.trim().is_empty(),
        progress: if total > 0 {
            format!("{faites} of {total}").into()
        } else {
            SharedString::default()
        },
        selected: etat.choisie == Some(t.id),
        estimate: t
            .task
            .estimate
            .map(iris_tasks::goals::duration_label)
            .unwrap_or_default()
            .into(),
        // The goal, except on its own page, where it goes without saying.
        goal: objectif
            .as_ref()
            .filter(|g| etat.vue != Vue::Goal(g.id))
            .map(|g| g.goal.title.as_str())
            .unwrap_or("")
            .into(),
        goal_color: objectif
            .as_ref()
            .map(|g| crate::calendar::couleur(&g.goal.color))
            .unwrap_or_default(),
        postponed: t.task.postponed,
        repeats: iris_tasks::repeat::label(t.task.repeat.as_deref()).into(),
    }
}

/// The identifier of the calendar event booked for a task.
fn uid_de_creneau(tache: i64) -> String {
    format!("task-{tache}@iris")
}

/// The first calendar of one's own: where slots are booked.
fn calendrier_local(services: &Services) -> Option<iris_store::StoredCalendar> {
    services
        .store
        .calendars()
        .unwrap_or_default()
        .into_iter()
        .find(|c| !c.is_subscription())
}

/// The slot booked for a task, if there is one: (event id, start, end) in ms.
fn creneau_reserve(services: &Services, tache: i64) -> Option<(i64, i64, i64)> {
    let cal = calendrier_local(services)?;
    let id = services
        .store
        .find_event(cal.id, &uid_de_creneau(tache), None)
        .ok()
        .flatten()?;
    let e = services.store.event(id).ok().flatten()?;
    Some((id, e.event.start_ms, e.event.end_ms))
}

fn heure_locale(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.format("%H:%M").to_string())
        .unwrap_or_default()
}

fn hm(minutes: i32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Books a slot for a task: an event of its length in the first calendar of one's
/// own (the one already booked is moved), and the task's day and hour set to it.
fn reserver(services: &Services, id: i64, jour: NaiveDate, minute: i32) -> Result<String, String> {
    let t = services
        .store
        .task(id)
        .ok()
        .flatten()
        .ok_or("This task no longer exists.")?;
    let cal = calendrier_local(services).ok_or("There is no calendar of your own to put it in.")?;
    let longueur = t.task.estimate.unwrap_or(30).max(5) as i64;
    let debut = jour.and_time(NaiveTime::MIN) + Duration::minutes(minute as i64);
    let a = vers_ms(debut);
    let uid = uid_de_creneau(id);
    let evenement = iris_store::NewEvent {
        uid: uid.clone(),
        summary: t.task.title.clone(),
        start_ms: a,
        end_ms: a + longueur * 60_000,
        tzid: iana_time_zone::get_timezone().ok(),
        ..Default::default()
    };
    match services.store.find_event(cal.id, &uid, None).ok().flatten() {
        Some(existant) => services
            .store
            .update_event(existant, cal.id, &evenement, now()),
        None => services
            .store
            .insert_event(cal.id, &evenement, now())
            .map(|_| ()),
    }
    .map_err(|e| e.to_string())?;
    modifier(services, id, |t| {
        t.due_day = Some(jour.format("%Y-%m-%d").to_string());
        t.due_minute = Some(minute);
        // Tied to the event it booked, so the event lists it — unless it came from
        // another event, which it keeps.
        if t.event_uid.is_none() {
            t.event_uid = Some(uid.clone());
            t.event_start = Some(0);
        }
    });
    Ok(format!(
        "Blocked {}–{} in {}.",
        hm(minute),
        hm(minute + longueur as i32),
        cal.name
    ))
}

/// Books a task's slot from the calendar, where it was dropped on the week.
pub(crate) fn book(
    services: &Services,
    id: i64,
    jour: NaiveDate,
    minute: i32,
) -> Result<String, String> {
    reserver(services, id, jour, minute)
}

/// A task's slot was dragged in the calendar: the task's day and hour follow it. The
/// event's uid names the task (`task-{id}@iris`); any other event is not a slot.
pub(crate) fn slot_moved(services: &Services, uid: &str, debut_ms: i64) {
    let Some(id) = uid
        .strip_prefix("task-")
        .and_then(|r| r.strip_suffix("@iris"))
        .and_then(|n| n.parse::<i64>().ok())
    else {
        return;
    };
    let Some(debut) = Local
        .timestamp_millis_opt(debut_ms)
        .earliest()
        .map(|d| d.naive_local())
    else {
        return;
    };
    modifier(services, id, |t| {
        t.due_day = Some(debut.date().format("%Y-%m-%d").to_string());
        t.due_minute = Some((debut.time() - NaiveTime::MIN).num_minutes() as i32);
    });
}

/// Today's tasks (late ones included) with no hour and no slot yet, for the
/// calendar's side column: the late first, then by priority; six at most.
pub fn to_plan(services: &Services) -> Vec<iris_ui::PlanTaskData> {
    let today = maintenant_local().date();
    let couleurs: std::collections::HashMap<i64, String> = services
        .store
        .task_lists()
        .unwrap_or_default()
        .into_iter()
        .map(|l| (l.id, l.color))
        .collect();
    let mut taches: Vec<_> = services
        .store
        .open_tasks()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            t.task.parent_id.is_none()
                && t.task.due_minute.is_none()
                && jour(&t.task).is_some_and(|j| j <= today)
        })
        .collect();
    taches.sort_by_key(|t| (jour(&t.task), -t.task.priority, t.id));
    taches
        .into_iter()
        .take(6)
        .map(|t| {
            let en_retard = jour(&t.task).is_some_and(|j| j < today);
            let longueur = t
                .task
                .estimate
                .map(iris_tasks::goals::duration_label)
                .unwrap_or_default();
            iris_ui::PlanTaskData {
                id: t.id as i32,
                title: t.task.title.as_str().into(),
                meta: match (longueur.is_empty(), en_retard) {
                    (true, false) => String::new(),
                    (true, true) => "late".into(),
                    (false, false) => longueur,
                    (false, true) => format!("{longueur}, late"),
                }
                .into(),
                color: crate::calendar::couleur(
                    couleurs
                        .get(&t.task.list_id)
                        .map(String::as_str)
                        .unwrap_or(""),
                ),
                minutes: t.task.estimate.unwrap_or(30).max(5),
            }
        })
        .collect()
}

/// Takes a task's booked slot off the calendar. Its day and hour stay.
fn liberer(services: &Services, id: i64) {
    if let Some((evenement, _, _)) = creneau_reserve(services, id) {
        let _ = services.store.delete_event(evenement);
    }
    let uid = uid_de_creneau(id);
    modifier(services, id, |t| {
        if t.event_uid.as_deref() == Some(uid.as_str()) {
            t.event_uid = None;
            t.event_start = None;
        }
    });
}

/// Fills the slot picker for a task: its day (the due day, or today), what the day
/// holds, and where its length fits. Returns the starts offered.
fn creneaux(f: &AppWindow, services: &Services, id: i64) -> Vec<i32> {
    let Some(t) = services.store.task(id).ok().flatten() else {
        return Vec::new();
    };
    let maintenant = maintenant_local();
    let today = maintenant.date();
    let jour_vise = jour(&t.task).filter(|j| *j >= today).unwrap_or(today);
    let longueur = t.task.estimate.unwrap_or(30);
    let minute_de = |ms: i64| {
        Local
            .timestamp_millis_opt(ms)
            .single()
            .map(|d| {
                let d = d.naive_local();
                if d.date() < jour_vise {
                    0
                } else if d.date() > jour_vise {
                    24 * 60
                } else {
                    (d.time().num_seconds_from_midnight() / 60) as i32
                }
            })
            .unwrap_or(0)
    };
    let propre = uid_de_creneau(id);
    // Its own booking does not stand in its way.
    let deja = creneau_reserve(services, id).map(|(ev, _, _)| format!("{ev}:"));
    let mut occupe: Vec<(i32, i32, String)> = crate::calendar::upcoming(services, jour_vise, 1)
        .into_iter()
        .filter(|u| !u.all_day)
        .filter(|u| deja.as_deref().is_none_or(|p| !u.key.starts_with(p)))
        .map(|u| (minute_de(u.start), minute_de(u.end), u.title))
        .collect();
    // The other tasks of that day that have an hour.
    for autre in services.store.open_tasks().unwrap_or_default() {
        if autre.id == id || autre.task.event_uid.as_deref() == Some(propre.as_str()) {
            continue;
        }
        if jour(&autre.task) == Some(jour_vise) {
            if let Some(m) = autre.task.due_minute {
                occupe.push((
                    m,
                    m + autre.task.estimate.unwrap_or(30),
                    autre.task.title.clone(),
                ));
            }
        }
    }
    occupe.sort();
    let depuis = if jour_vise == today {
        (maintenant.time().num_seconds_from_midnight() / 60) as i32
    } else {
        0
    };
    let libres = iris_tasks::slots::free_slots(
        &occupe.iter().map(|(a, b, _)| (*a, *b)).collect::<Vec<_>>(),
        longueur,
        depuis,
        9,
    );
    f.set_task_slot_day(
        if jour_vise == today {
            "today".to_string()
        } else {
            jour_vise.format("%A %-d %B").to_string()
        }
        .into(),
    );
    f.set_task_slot_need(
        format!(
            "{} needed · between your events",
            iris_tasks::goals::duration_label(longueur)
        )
        .into(),
    );
    f.set_task_slot_busy(ModelRc::new(VecModel::from(
        occupe
            .iter()
            .filter(|(_, b, _)| *b > depuis)
            .take(4)
            .map(|(a, b, titre)| SharedString::from(format!("{}–{}  {titre}", hm(*a), hm(*b))))
            .collect::<Vec<_>>(),
    )));
    f.set_task_slots(ModelRc::new(VecModel::from(
        libres
            .iter()
            .map(|m| SharedString::from(format!("{}–{}", hm(*m), hm(m + longueur))))
            .collect::<Vec<_>>(),
    )));
    f.set_task_slot_open(true);
    libres
}

/// The day a slot picker is about: the task's due day from today on, or today.
fn jour_des_creneaux(services: &Services, id: i64) -> NaiveDate {
    let today = maintenant_local().date();
    services
        .store
        .task(id)
        .ok()
        .flatten()
        .and_then(|t| jour(&t.task))
        .filter(|j| *j >= today)
        .unwrap_or(today)
}

/// Recalcule tout ce que l'onglet affiche.
fn rafraichir(f: &AppWindow, services: &Services, etat: &mut Etat) {
    let maintenant = maintenant_local();
    let today = maintenant.date();
    etat.listes = services.store.task_lists().unwrap_or_default();
    if let Vue::List(id) = etat.vue {
        if !etat.listes.iter().any(|l| l.id == id) {
            etat.vue = Vue::Today;
        }
    }
    if let Vue::Goal(id) = etat.vue {
        if services.store.goal(id).ok().flatten().is_none() {
            etat.vue = Vue::Goals;
        }
    }
    // The goals: the side column, a goal's page or all of them, the nudges on Today.
    crate::goals::remplir(
        f,
        services,
        match etat.vue {
            Vue::Goal(id) => Some(id),
            _ => None,
        },
        etat.vue == Vue::Goals,
        etat.vue == Vue::Today,
        etat.vue == Vue::Week,
    );

    let ouvertes = services.store.open_tasks().unwrap_or_default();
    let dessus: Vec<&StoredTask> = ouvertes
        .iter()
        .filter(|t| t.task.parent_id.is_none())
        .collect();
    let compte = |vue: Vue| dessus.iter().filter(|t| dans_la_vue(vue, t, today)).count();

    // --- La colonne de gauche.
    let mut lieux = vec![
        (
            Vue::Today,
            "Today",
            iris_ui_icone::SOLEIL,
            compte(Vue::Today),
        ),
        (
            Vue::Upcoming,
            "Upcoming",
            iris_ui_icone::AGENDA,
            compte(Vue::Upcoming),
        ),
        (
            Vue::Anytime,
            "All tasks",
            iris_ui_icone::LISTE,
            compte(Vue::Anytime),
        ),
        (
            Vue::Mail,
            "From mail",
            iris_ui_icone::COURRIER,
            compte(Vue::Mail),
        ),
        // Not a count of tasks: a look back at the week, and ahead at the next.
        (Vue::Week, "This week", iris_ui_icone::SEMAINE, 0),
    ]
    .into_iter()
    .map(|(vue, nom, icone, compte)| TaskPlaceData {
        key: vue.cle().into(),
        name: nom.into(),
        icon: icone.into(),
        count: compte as i32,
        selected: etat.vue == vue,
        ..Default::default()
    })
    .collect::<Vec<_>>();
    for l in &etat.listes {
        lieux.push(TaskPlaceData {
            key: Vue::List(l.id).cle().into(),
            name: l.name.as_str().into(),
            color: crate::calendar::couleur(&l.color),
            count: dessus.iter().filter(|t| t.task.list_id == l.id).count() as i32,
            selected: etat.vue == Vue::List(l.id),
            is_list: true,
            list_id: l.id as i32,
            ..Default::default()
        });
    }
    f.set_task_places(ModelRc::new(VecModel::from(lieux)));
    f.set_task_lists(ModelRc::new(VecModel::from(
        etat.listes
            .iter()
            .map(|l| SharedString::from(l.name.as_str()))
            .collect::<Vec<_>>(),
    )));

    // --- La colonne du milieu.
    let mut lignes = Vec::new();
    let par_sections =
        |taches: Vec<&StoredTask>, avec_liste: bool, lignes: &mut Vec<TaskRowData>| {
            for s in Section::ALL {
                let dedans: Vec<&&StoredTask> = taches
                    .iter()
                    .filter(|t| section(jour(&t.task), today) == s)
                    .collect();
                if dedans.is_empty() {
                    continue;
                }
                lignes.push(section_ligne(
                    s.title(),
                    dedans.len(),
                    s == Section::Overdue,
                ));
                for t in dedans {
                    lignes.push(ligne_tache(services, t, etat, avec_liste, maintenant));
                }
            }
        };

    let de_la_vue: Vec<&StoredTask> = dessus
        .iter()
        .copied()
        .filter(|t| dans_la_vue(etat.vue, t, today))
        .collect();
    let a_faire = de_la_vue.len();
    match etat.vue {
        Vue::Upcoming => {
            // Un titre par jour sur la semaine qui vient, puis le reste.
            for k in 1..=7 {
                let j = today + Duration::days(k);
                let dedans: Vec<&&StoredTask> =
                    dessus.iter().filter(|t| jour(&t.task) == Some(j)).collect();
                if dedans.is_empty() {
                    continue;
                }
                let titre = if k == 1 {
                    "Tomorrow".to_string()
                } else {
                    j.format("%A %-d %b").to_string()
                };
                lignes.push(section_ligne(&titre, dedans.len(), false));
                for t in dedans {
                    lignes.push(ligne_tache(services, t, etat, true, maintenant));
                }
            }
            let plus_tard: Vec<&StoredTask> = dessus
                .iter()
                .copied()
                .filter(|t| jour(&t.task).is_some_and(|j| j > today + Duration::days(7)))
                .collect();
            if !plus_tard.is_empty() {
                lignes.push(section_ligne("Later", plus_tard.len(), false));
                for t in plus_tard {
                    lignes.push(ligne_tache(services, t, etat, true, maintenant));
                }
            }
        }
        Vue::Week => revue_de_la_semaine(services, etat, &dessus, maintenant, &mut lignes),
        // Today, All tasks, From mail and the lists: by section, from late to later.
        vue => par_sections(de_la_vue, !matches!(vue, Vue::List(_)), &mut lignes),
    }

    // What was done here stays here, struck through, until cleared.
    let faites = terminees(services, etat.vue, today);
    if !faites.is_empty() {
        lignes.push(TaskRowData {
            kind: 4,
            ..section_ligne("Done", faites.len(), false)
        });
        for t in &faites {
            lignes.push(ligne_tache(
                services,
                t,
                etat,
                !matches!(etat.vue, Vue::List(_)),
                maintenant,
            ));
        }
    }

    // On Today, "Today" goes without saying: the hour, or when it fits.
    if etat.vue == Vue::Today {
        for l in lignes.iter_mut().filter(|l| l.kind == 0 && !l.done) {
            if let Some(reste) = l.meta.strip_prefix("Today") {
                l.meta = match reste.trim() {
                    "" => "Anytime".into(),
                    heure => heure.into(),
                };
            }
        }
    }

    etat.ordre = lignes
        .iter()
        .filter(|l| l.kind == 0)
        .map(|l| l.id as i64)
        .collect();
    f.set_task_overview(apercu(
        services,
        etat.vue,
        a_faire + faites.len(),
        faites.len(),
        maintenant,
    ));
    f.set_tasks_title(titre_vue(etat.vue, &etat.listes).into());
    f.set_tasks_subtitle(match etat.vue {
        Vue::Today => today.format("%A %-d %B").to_string().into(),
        Vue::Week => {
            let lundi = lundi_de(today);
            format!(
                "{} – {}",
                lundi.format("%a %-d %b"),
                (lundi + Duration::days(6)).format("%a %-d %b")
            )
            .into()
        }
        _ => SharedString::default(),
    });
    f.set_task_add_hint(indication(etat.vue).into());
    f.set_task_rows(ModelRc::new(VecModel::from(lignes)));
    remplir_detail(f, services, etat, maintenant);
}

fn lundi_de(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

/// The week in review, as sections of the list: what was done since Monday, what was
/// put off, what is late, and what is due next week. Each task once, in the first
/// section it belongs to.
fn revue_de_la_semaine(
    services: &Services,
    etat: &Etat,
    ouvertes: &[&StoredTask],
    maintenant: NaiveDateTime,
    lignes: &mut Vec<TaskRowData>,
) {
    let today = maintenant.date();
    let lundi = lundi_de(today);
    let lundi_suivant = lundi + Duration::days(7);
    let local = |ms: i64| {
        Local
            .timestamp_millis_opt(ms)
            .single()
            .map(|d| d.date_naive())
    };

    let faites: Vec<StoredTask> = services
        .store
        .done_tasks(None, 500)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            t.done_at
                .and_then(|d| local(d.millis()))
                .is_some_and(|j| j >= lundi)
        })
        .collect();
    let en_retard: Vec<&StoredTask> = ouvertes
        .iter()
        .copied()
        .filter(|t| jour(&t.task).is_some_and(|j| j < today))
        .collect();
    let repoussees: Vec<&StoredTask> = ouvertes
        .iter()
        .copied()
        .filter(|t| t.task.postponed > 0 && !en_retard.iter().any(|r| r.id == t.id))
        .collect();
    let semaine_prochaine: Vec<&StoredTask> = ouvertes
        .iter()
        .copied()
        .filter(|t| {
            jour(&t.task)
                .is_some_and(|j| j >= lundi_suivant && j < lundi_suivant + Duration::days(7))
                && !repoussees.iter().any(|r| r.id == t.id)
        })
        .collect();

    let mut section = |titre: &str, taches: &[&StoredTask], rouge: bool| {
        if taches.is_empty() {
            return;
        }
        lignes.push(section_ligne(titre, taches.len(), rouge));
        for t in taches {
            lignes.push(ligne_tache(services, t, etat, true, maintenant));
        }
    };
    section("Late", &en_retard, true);
    section("Put off", &repoussees, false);
    section("Next week", &semaine_prochaine, false);
    let faites: Vec<&StoredTask> = faites.iter().collect();
    section("Done this week", &faites, false);
    if lignes.is_empty() {
        lignes.push(section_ligne(
            "Nothing late, nothing put off: a clear week",
            0,
            false,
        ));
    }
}

/// Le panneau de droite : la tâche choisie, ou rien.
fn remplir_detail(f: &AppWindow, services: &Services, etat: &mut Etat, maintenant: NaiveDateTime) {
    let Some(t) = etat
        .choisie
        .and_then(|id| services.store.task(id).ok().flatten())
    else {
        etat.choisie = None;
        etat.remplie = None;
        etat.mois = None;
        f.set_task_has_detail(false);
        f.set_task_picker_open(false);
        return;
    };

    // Les champs qu'on tape ne se remplissent qu'en changeant de tâche.
    if etat.remplie != Some(t.id) {
        f.set_task_detail_title(t.task.title.as_str().into());
        f.set_task_detail_notes(t.task.notes.as_str().into());
        f.set_task_detail_time(heure_texte(minute(&t.task)).into());
        etat.remplie = Some(t.id);
    }

    let etapes = services.store.subtasks(t.id).unwrap_or_default();
    // The day alone: its hour has a field of its own beside it.
    let (libelle, retard) = match jour(&t.task) {
        Some(j) => (
            due_label(j, None, maintenant.date()),
            !t.is_done() && is_overdue(j, minute(&t.task), maintenant),
        ),
        None => (String::new(), false),
    };
    f.set_task_detail(TaskDetailData {
        id: t.id as i32,
        done: t.is_done(),
        has_due: t.task.due_day.is_some(),
        due_label: libelle.into(),
        overdue: retard,
        list_index: etat
            .listes
            .iter()
            .position(|l| l.id == t.task.list_id)
            .unwrap_or(0) as i32,
        reminder_index: REMINDERS
            .iter()
            .position(|(_, m)| *m == t.task.remind_before.map(i64::from))
            .unwrap_or(0) as i32,
        priority: t.task.priority,
        goal_index: t
            .task
            .goal_id
            .and_then(|g| {
                services
                    .store
                    .goals()
                    .unwrap_or_default()
                    .iter()
                    .position(|x| x.id == g)
            })
            .map(|i| i as i32 + 1)
            .unwrap_or(0),
        source: t.task.source.as_str().into(),
        from_mail: t.task.thread_id.is_some(),
        // A slot booked for it is not an event it came from.
        from_event: t
            .task
            .event_uid
            .as_deref()
            .is_some_and(|u| u != uid_de_creneau(t.id)),
        estimate: t.task.estimate.unwrap_or(0),
        postponed: t.task.postponed,
        repeat_index: iris_tasks::repeat::index(t.task.repeat.as_deref()) as i32,
        slot: creneau_reserve(services, t.id)
            .map(|(_, a, b)| format!("{}–{}", heure_locale(a), heure_locale(b)))
            .unwrap_or_default()
            .into(),
        steps_done: etapes.iter().filter(|e| e.is_done()).count() as i32,
        subtasks: ModelRc::new(VecModel::from(
            etapes
                .iter()
                .map(|e| SubtaskData {
                    id: e.id as i32,
                    title: e.task.title.as_str().into(),
                    done: e.is_done(),
                })
                .collect::<Vec<_>>(),
        )),
    });
    f.set_task_has_detail(true);

    // Le petit calendrier, ouvert sur le mois de l'échéance.
    match etat.mois {
        Some(mois) => {
            let choisi = jour(&t.task).unwrap_or(maintenant.date());
            f.set_task_picker_title(mois.format("%B %Y").to_string().into());
            f.set_task_picker_cells(ModelRc::new(VecModel::from(crate::calendar::mini_cells(
                choisi, mois,
            ))));
            f.set_task_picker_open(true);
        }
        None => f.set_task_picker_open(false),
    }
}

/// Les noms d'icônes de la colonne de gauche, en chemins SVG.
mod iris_ui_icone {
    pub const SOLEIL: &str = "M 12 8 A 4 4 0 1 0 12 16 A 4 4 0 1 0 12 8 Z M 12 2.5 V 4.8 M 12 19.2 V 21.5 M 2.5 12 H 4.8 M 19.2 12 H 21.5 M 5.3 5.3 L 6.9 6.9 M 17.1 17.1 L 18.7 18.7 M 5.3 18.7 L 6.9 17.1 M 17.1 6.9 L 18.7 5.3";
    pub const AGENDA: &str = "M 5.5 5 H 18.5 A 2 2 0 0 1 20.5 7 V 18.5 A 2 2 0 0 1 18.5 20.5 H 5.5 A 2 2 0 0 1 3.5 18.5 V 7 A 2 2 0 0 1 5.5 5 Z M 3.5 10 H 20.5 M 8 3 V 7 M 16 3 V 7";
    pub const LISTE: &str =
        "M 12 3.5 A 8.5 8.5 0 1 0 12 20.5 A 8.5 8.5 0 1 0 12 3.5 Z M 8 12.3 L 10.8 15 L 16 9.3";
    pub const COURRIER: &str = "M 5.0 5.5 H 19.0 A 1.5 1.5 0 0 1 20.5 7.0 V 17.0 A 1.5 1.5 0 0 1 19.0 18.5 H 5.0 A 1.5 1.5 0 0 1 3.5 17.0 V 7.0 A 1.5 1.5 0 0 1 5.0 5.5 Z M 3.5 8 L 12 13.5 L 20.5 8";
    /// Bars of a week, rising: the review.
    pub const SEMAINE: &str =
        "M 4 20.5 H 20 M 6.5 17 V 13 M 10.5 17 V 9 M 14.5 17 V 11 M 18.5 17 V 5.5";
}

// --- Écrire ----------------------------------------------------------------------------

/// Crée une tâche depuis la ligne de saisie rapide.
fn ajouter(services: &Services, etat: &mut Etat, texte: &str) -> Option<(i64, String)> {
    let maintenant = maintenant_local();
    let q = iris_tasks::parse(texte, maintenant);
    let today = maintenant.date();

    // `#Nom` : une liste existante, ou une nouvelle.
    let mut message = None;
    let liste = match &q.list {
        Some(nom) => match etat
            .listes
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(nom))
        {
            Some(l) => l.id,
            None => {
                let couleur = COULEURS[etat.listes.len() % COULEURS.len()];
                let id = services.store.create_task_list(nom, couleur, now()).ok()?;
                message = Some(format!("List “{nom}” created."));
                id
            }
        },
        None => match etat.vue {
            Vue::List(id) => id,
            _ => etat.listes.first()?.id,
        },
    };
    // On a goal's page, what is added is a step toward it.
    let objectif = match etat.vue {
        Vue::Goal(id) => Some(id),
        _ => None,
    };

    // Sans date écrite, la vue en donne une : « Today » est aujourd'hui.
    let echeance = q.due.map(|d| (d.day, d.minute)).or(match etat.vue {
        Vue::Today => Some((today, None)),
        _ => None,
    });
    let mut t = NewTask {
        list_id: liste,
        title: q.title.clone(),
        due_day: echeance.map(|(j, _)| j.format("%Y-%m-%d").to_string()),
        due_minute: echeance.and_then(|(_, m)| m).map(|m| m as i32),
        // Une heure dite est un rendez-vous : on le rappelle à l'heure.
        remind_before: echeance.and_then(|(_, m)| m).map(|_| 0),
        priority: q.priority as i32,
        goal_id: objectif,
        ..Default::default()
    };
    recalculer_rappel(&mut t);
    let id = services.store.insert_task(&t, now()).ok()?;

    let ou = match echeance {
        Some((j, m)) => format!("Added, due {}.", due_label(j, m, today)),
        None => "Added.".to_string(),
    };
    Some((id, message.map(|m| format!("{m} {ou}")).unwrap_or(ou)))
}

/// Change une tâche et l'enregistre, rappel recalculé.
fn modifier(services: &Services, id: i64, change: impl FnOnce(&mut NewTask)) -> bool {
    let Ok(Some(t)) = services.store.task(id) else {
        return false;
    };
    let mut nouvelle = t.task.clone();
    change(&mut nouvelle);
    recalculer_rappel(&mut nouvelle);
    if nouvelle == t.task {
        return false;
    }
    services.store.update_task(id, &nouvelle, now()).is_ok()
}

fn basculer(services: &Services, id: i64) {
    toggle_done(services, id);
}

/// Ticks a task, or unticks it. A repeating task ticked makes its next one, due on the
/// next day of its rule: the same title, list, hour, reminder, length, goal and rule,
/// without its subtasks or what it came from. Unticked and ticked again, it does not
/// make a second one.
pub(crate) fn toggle_done(services: &Services, id: i64) {
    let Ok(Some(t)) = services.store.task(id) else {
        return;
    };
    if t.is_done() {
        let _ = services.store.set_task_done(id, None);
        return;
    }
    let _ = services.store.set_task_done(id, Some(now()));
    let (Some(regle), Some(du)) = (t.task.repeat.as_deref(), jour(&t.task)) else {
        return;
    };
    let Some(suivant) = iris_tasks::repeat::next_day(regle, du, maintenant_local().date()) else {
        return;
    };
    let jour_suivant = suivant.format("%Y-%m-%d").to_string();
    let deja = services
        .store
        .open_tasks()
        .unwrap_or_default()
        .into_iter()
        .any(|o| {
            o.task.title == t.task.title
                && o.task.list_id == t.task.list_id
                && o.task.repeat == t.task.repeat
                && o.task.due_day.as_deref() == Some(jour_suivant.as_str())
        });
    if deja {
        return;
    }
    let mut prochaine = NewTask {
        list_id: t.task.list_id,
        title: t.task.title.clone(),
        notes: t.task.notes.clone(),
        due_day: Some(jour_suivant),
        due_minute: t.task.due_minute,
        remind_before: t.task.remind_before,
        priority: t.task.priority,
        goal_id: t.task.goal_id,
        estimate: t.task.estimate,
        repeat: t.task.repeat.clone(),
        ..Default::default()
    };
    recalculer_rappel(&mut prochaine);
    let _ = services.store.insert_task(&prochaine, now());
}

/// Une conversation devient une tâche, dans la première liste.
pub fn task_from_thread(services: &Services, thread: i64) -> Result<String, String> {
    if !services
        .store
        .tasks_for_thread(thread)
        .unwrap_or_default()
        .is_empty()
    {
        return Ok("This conversation is already in your tasks.".into());
    }
    let fil = services
        .store
        .thread_row(ThreadId(thread))
        .map_err(|e| e.to_string())?
        .ok_or("This conversation no longer exists.")?;
    let liste = services
        .store
        .task_lists()
        .map_err(|e| e.to_string())?
        .first()
        .map(|l| l.id)
        .ok_or("There is no list to add it to.")?;
    let sujet = if fil.subject.trim().is_empty() {
        "(No subject)".to_string()
    } else {
        fil.subject.trim().to_string()
    };
    let t = NewTask {
        list_id: liste,
        title: sujet.clone(),
        thread_id: Some(thread),
        source: format!("{}: {}", fil.from_display, sujet),
        ..Default::default()
    };
    services
        .store
        .insert_task(&t, now())
        .map_err(|e| e.to_string())?;
    Ok(format!("Added to your tasks: {sujet} (Ctrl+3)."))
}

/// Les rappels arrivés à l'heure, en notifications.
fn rappels(services: &Services) -> bool {
    let echus = services.store.due_task_reminders(now()).unwrap_or_default();
    let today = maintenant_local().date();
    for t in &echus {
        let corps = match jour(&t.task) {
            Some(j) => format!("Due {}", due_label(j, minute(&t.task), today)),
            None => "Task reminder".into(),
        };
        crate::notify::show_text(&t.task.title, &corps);
        let _ = services.store.mark_task_reminded(t.id);
    }
    !echus.is_empty()
}

thread_local! {
    static MINUTERIES: RefCell<Vec<slint::Timer>> = const { RefCell::new(Vec::new()) };
}

// --- Le câblage ------------------------------------------------------------------------

pub fn wire_tasks(f: &AppWindow, services: &Services, controller: Arc<Controller>) {
    let etat = Rc::new(RefCell::new(Etat {
        vue: Vue::Today,
        choisie: None,
        remplie: None,
        mois: None,
        ordre: Vec::new(),
        listes: Vec::new(),
        supprimees: Vec::new(),
        plus_tard: None,
        creneaux: Vec::new(),
    }));
    f.set_task_reminders(ModelRc::new(VecModel::from(
        REMINDERS
            .iter()
            .map(|(n, _)| SharedString::from(*n))
            .collect::<Vec<_>>(),
    )));
    f.set_task_repeats(ModelRc::new(VecModel::from(
        iris_tasks::repeat::REPEATS
            .iter()
            .map(|(_, n)| SharedString::from(*n))
            .collect::<Vec<_>>(),
    )));

    let redessiner = {
        let (faible, services, etat) = (f.as_weak(), services.clone(), Rc::clone(&etat));
        Rc::new(move || {
            if let Some(f) = faible.upgrade() {
                rafraichir(&f, &services, &mut etat.borrow_mut());
            }
        })
    };

    // Ce qu'on tape dans le titre et les notes s'enregistre à chaque touche ; la
    // colonne ne se redessine qu'une fois la frappe posée.
    let plus_tard = {
        let minuterie = Rc::new(slint::Timer::default());
        let redessiner = Rc::clone(&redessiner);
        Rc::new(move || {
            let redessiner = Rc::clone(&redessiner);
            minuterie.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(400),
                move || redessiner(),
            );
        })
    };

    // L'onglet se remplit quand on y arrive.
    {
        let redessiner = Rc::clone(&redessiner);
        crate::workspace::follow(f, move |w| {
            if w == 2 {
                redessiner();
            }
        });
    }

    // Chaque geste reçoit ses propres copies de ce qu'il touche. Les noms viennent de
    // l'appel : ceux qu'une macro inventerait resteraient invisibles au corps.
    macro_rules! geste {
        ($installer:ident, [$s:ident, $e:ident, $r:ident, $f:ident, $c:ident], || $corps:expr) => {
            geste!($installer, [$s, $e, $r, $f, $c], | | $corps)
        };
        ($installer:ident, [$s:ident, $e:ident, $r:ident, $f:ident, $c:ident], |$($arg:ident),*| $corps:expr) => {{
            let ($s, $e, $r, $c, faible) =
                ($s.clone(), Rc::clone(&$e), Rc::clone(&$r), Arc::clone(&$c), $f.as_weak());
            $f.$installer(move |$($arg),*| {
                let Some($f) = faible.upgrade() else { return };
                #[allow(unused_variables)]
                let ($s, $e, $r, $c, $f) = (&$s, &$e, &$r, &$c, &$f);
                $corps
            });
        }};
    }

    geste!(
        on_task_place_chosen,
        [services, etat, redessiner, f, controller],
        |cle| {
            if let Some(v) = Vue::depuis(&cle) {
                let mut e = etat.borrow_mut();
                e.vue = v;
                e.mois = None;
            }
            redessiner();
        }
    );

    // --- Goals ---
    let objectif_montre = |etat: &Rc<RefCell<Etat>>| match etat.borrow().vue {
        Vue::Goal(id) => Some(id),
        _ => None,
    };
    geste!(
        on_goal_logged,
        [services, etat, redessiner, f, controller],
        |note| {
            if let Some(g) = objectif_montre(etat) {
                let _ = services.store.log_goal(g, note.trim(), now());
                f.set_goal_note(SharedString::default());
                redessiner();
            }
        }
    );
    geste!(
        on_goal_entry_removed,
        [services, etat, redessiner, f, controller],
        |id| {
            let _ = services.store.delete_goal_entry(id as i64);
            redessiner();
        }
    );
    geste!(
        on_milestone_toggled,
        [services, etat, redessiner, f, controller],
        |id| {
            if let Some(g) = objectif_montre(etat) {
                let fait = services
                    .store
                    .milestones(g)
                    .unwrap_or_default()
                    .iter()
                    .find(|m| m.id == id as i64)
                    .is_some_and(|m| m.done_at.is_some());
                let _ = services
                    .store
                    .set_milestone_done(id as i64, if fait { None } else { Some(now()) });
                redessiner();
            }
        }
    );
    geste!(
        on_milestone_added,
        [services, etat, redessiner, f, controller],
        |titre| {
            if let Some(g) = objectif_montre(etat) {
                if !titre.trim().is_empty() {
                    let _ = services.store.add_milestone(g, titre.trim());
                    redessiner();
                }
            }
        }
    );
    geste!(
        on_task_goal_chosen,
        [services, etat, redessiner, f, controller],
        |index| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            let objectifs = services.store.goals().unwrap_or_default();
            let choisi = (index > 0)
                .then(|| objectifs.get(index as usize - 1).map(|g| g.id))
                .flatten();
            if modifier(services, id, |t| t.goal_id = choisi) {
                redessiner();
            }
        }
    );
    geste!(
        on_goal_new_requested,
        [services, etat, redessiner, f, controller],
        || crate::goals::ouvrir_nouveau(f)
    );
    geste!(
        on_goal_new_in_days,
        [services, etat, redessiner, f, controller],
        |jours| crate::goals::dans_jours(f, jours as i64)
    );
    geste!(
        on_goal_new_confirmed,
        [services, etat, redessiner, f, controller],
        || match crate::goals::creer(
            f,
            services,
            f.get_goal_new_editing()
                .then(|| objectif_montre(etat))
                .flatten()
        ) {
            Ok(_) if f.get_goal_new_editing() => {
                f.set_goal_new_open(false);
                f.set_goal_new_editing(false);
                redessiner();
            }
            Ok(id) => {
                f.set_goal_new_open(false);
                let cle = Vue::Goal(id).cle();
                {
                    let mut e = etat.borrow_mut();
                    e.vue = Vue::Goal(id);
                    e.choisie = None;
                }
                crate::nav::note(f, "tasks", &cle);
                redessiner();
            }
            Err(message) => f.set_goal_new_error(message.into()),
        }
    );
    geste!(
        on_goal_edit_requested,
        [services, etat, redessiner, f, controller],
        || {
            if let Some(g) = objectif_montre(etat) {
                crate::goals::ouvrir_edition(f, services, g);
            }
        }
    );
    geste!(
        on_goal_renamed,
        [services, etat, redessiner, f, controller],
        |titre| {
            let Some(g) = objectif_montre(etat) else {
                return;
            };
            match crate::goals::renommer(services, g, &titre) {
                Ok(()) => redessiner(),
                Err(message) => f.set_status(message.into()),
            }
        }
    );
    geste!(
        on_goal_time_requested,
        [services, etat, redessiner, f, controller],
        || crate::goals::ouvrir_temps(f)
    );
    geste!(
        on_goal_time_day_toggled,
        [services, etat, redessiner, f, controller],
        |i| crate::goals::basculer_jour(f, i)
    );
    geste!(
        on_goal_time_confirmed,
        [services, etat, redessiner, f, controller],
        || {
            let Some(g) = objectif_montre(etat) else {
                return;
            };
            match crate::goals::bloquer(f, services, g) {
                Ok(message) => {
                    f.set_goal_time_open(false);
                    f.set_status(message.into());
                }
                Err(message) => f.set_goal_time_error(message.into()),
            }
        }
    );
    geste!(
        on_goal_delete_confirmed,
        [services, etat, redessiner, f, controller],
        || {
            if let Some(g) = objectif_montre(etat) {
                let _ = services.store.delete_goal(g);
                etat.borrow_mut().vue = Vue::Goals;
                f.set_status("Goal deleted. Its steps stay in your tasks.".into());
                redessiner();
            }
        }
    );

    // --- When to do a task ---
    geste!(
        on_task_later_requested,
        [services, etat, redessiner, f, controller],
        |id| {
            let Some(t) = services.store.task(id as i64).ok().flatten() else {
                return;
            };
            let titre: String = t.task.title.chars().take(28).collect();
            let titre = if t.task.title.chars().count() > 28 {
                format!("{titre}…")
            } else {
                titre
            };
            etat.borrow_mut().plus_tard = Some(id as i64);
            f.set_task_later_title(titre.into());
            f.set_task_later_open(true);
        }
    );
    geste!(
        on_task_later_chosen,
        [services, etat, redessiner, f, controller],
        |mot| {
            let Some(id) = etat.borrow_mut().plus_tard.take() else {
                return;
            };
            let Some(quand) = iris_tasks::slots::Later::from_word(&mot) else {
                return;
            };
            let jour_neuf = quand.day(maintenant_local().date());
            // The slot booked was for the old day: it goes.
            liberer(services, id);
            let change = modifier(services, id, |t| {
                t.due_day = jour_neuf.map(|j| j.format("%Y-%m-%d").to_string());
                // Tomorrow keeps its hour; the others start the day free.
                if quand != iris_tasks::slots::Later::Tomorrow || jour_neuf.is_none() {
                    t.due_minute = None;
                }
                t.postponed += 1;
            });
            if change {
                f.set_status(format!("Moved to {}.", quand.label()).into());
            }
            redessiner();
        }
    );
    geste!(
        on_task_later_pick_date,
        [services, etat, redessiner, f, controller],
        || {
            let Some(id) = etat.borrow_mut().plus_tard.take() else {
                return;
            };
            // Counted too: picking a later day is putting it off.
            modifier(services, id, |t| t.postponed += 1);
            {
                let mut e = etat.borrow_mut();
                e.choisie = Some(id);
                e.remplie = None;
                e.mois = Some(premier_du_mois(jour_des_creneaux(services, id)));
            }
            redessiner();
        }
    );
    geste!(
        on_task_estimate_chosen,
        [services, etat, redessiner, f, controller],
        |minutes| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            if modifier(services, id, |t| {
                t.estimate = (minutes > 0).then_some(minutes);
            }) {
                redessiner();
            }
        }
    );
    geste!(
        on_task_estimate_typed,
        [services, etat, redessiner, f, controller],
        |texte| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            match iris_tasks::goals::parse_duration(&texte) {
                Some(m) => {
                    modifier(services, id, |t| t.estimate = Some(m));
                    redessiner();
                }
                None => f.set_status("A length looks like 45m, 1h or 1h20.".into()),
            }
        }
    );
    geste!(
        on_task_slot_requested,
        [services, etat, redessiner, f, controller],
        || {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            let offerts = creneaux(f, services, id);
            etat.borrow_mut().creneaux = offerts;
        }
    );
    geste!(
        on_task_slot_chosen,
        [services, etat, redessiner, f, controller],
        |i| {
            let (id, minute) = {
                let e = etat.borrow();
                (e.choisie, e.creneaux.get(i as usize).copied())
            };
            let (Some(id), Some(minute)) = (id, minute) else {
                return;
            };
            f.set_task_slot_open(false);
            match reserver(services, id, jour_des_creneaux(services, id), minute) {
                Ok(message) => f.set_status(message.into()),
                Err(message) => f.set_status(message.into()),
            }
            redessiner();
        }
    );
    geste!(
        on_task_slot_typed,
        [services, etat, redessiner, f, controller],
        |texte| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            let Some(minute) = lire_heure(&texte) else {
                f.set_status("A time looks like 16:30.".into());
                return;
            };
            f.set_task_slot_open(false);
            match reserver(services, id, jour_des_creneaux(services, id), minute as i32) {
                Ok(message) => f.set_status(message.into()),
                Err(message) => f.set_status(message.into()),
            }
            redessiner();
        }
    );
    geste!(
        on_task_unbook,
        [services, etat, redessiner, f, controller],
        || {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            liberer(services, id);
            f.set_status("Slot removed from your calendar.".into());
            redessiner();
        }
    );

    geste!(
        on_task_add,
        [services, etat, redessiner, f, controller],
        |texte| {
            let resultat = ajouter(services, &mut etat.borrow_mut(), &texte);
            if let Some((_, message)) = resultat {
                f.set_task_add_text(SharedString::default());
                f.set_task_add_tokens(ModelRc::default());
                f.set_status(message.into());
            }
            redessiner();
        }
    );

    geste!(
        on_task_add_edited,
        [services, etat, redessiner, f, controller],
        |texte| {
            let lus = jetons(&texte, &etat.borrow().listes, maintenant_local());
            f.set_task_add_tokens(ModelRc::new(VecModel::from(lus)));
        }
    );

    geste!(
        on_task_row_selected,
        [services, etat, redessiner, f, controller],
        |ligne| {
            if ligne.kind != 0 {
                return;
            }
            let mut e = etat.borrow_mut();
            if e.choisie != Some(ligne.id as i64) {
                e.choisie = Some(ligne.id as i64);
                e.mois = None;
            }
            drop(e);
            redessiner();
        }
    );

    // The completed tasks of the view, cleared from its Completed title.
    geste!(
        on_task_clear_completed,
        [services, etat, redessiner, f, controller],
        || {
            let vue = etat.borrow().vue;
            let faites = terminees(services, vue, maintenant_local().date());
            let mut parties = Vec::new();
            for t in &faites {
                parties.extend(avec_ses_etapes(services, t.id));
                let _ = services.store.delete_task(t.id);
            }
            let mut e = etat.borrow_mut();
            if e.choisie.is_some_and(|c| faites.iter().any(|t| t.id == c)) {
                e.choisie = None;
            }
            if !parties.is_empty() {
                e.supprimees.push(parties);
            }
            drop(e);
            f.set_status(
                match faites.len() {
                    1 => "1 completed task deleted. Ctrl+Z brings it back.".to_string(),
                    n => format!("{n} completed tasks deleted. Ctrl+Z brings them back."),
                }
                .into(),
            );
            redessiner();
        }
    );

    // A task carried onto a list moves there; onto Today, it becomes due today.
    geste!(
        on_task_dropped,
        [services, etat, redessiner, f, controller],
        |id, cle| {
            let (id, today) = (id as i64, maintenant_local().date());
            let message = match Vue::depuis(&cle) {
                Some(Vue::List(liste)) => {
                    let nom = etat
                        .borrow()
                        .listes
                        .iter()
                        .find(|l| l.id == liste)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    modifier(services, id, |t| t.list_id = liste)
                        .then(|| format!("Moved to {nom}."))
                }
                Some(Vue::Today) => modifier(services, id, |t| {
                    t.due_day = Some(today.format("%Y-%m-%d").to_string())
                })
                .then(|| "Due today.".to_string()),
                _ => None,
            };
            if let Some(m) = message {
                f.set_status(m.into());
            }
            redessiner();
        }
    );

    geste!(
        on_task_toggled,
        [services, etat, redessiner, f, controller],
        |id| {
            basculer(services, id as i64);
            redessiner();
        }
    );
    geste!(
        on_task_toggle_selected,
        [services, etat, redessiner, f, controller],
        || {
            let choisie = etat.borrow().choisie;
            if let Some(id) = choisie {
                basculer(services, id);
                redessiner();
            }
        }
    );

    geste!(
        on_task_move,
        [services, etat, redessiner, f, controller],
        |sens| {
            let mut e = etat.borrow_mut();
            if e.ordre.is_empty() {
                return;
            }
            let i = match e.choisie.and_then(|c| e.ordre.iter().position(|x| *x == c)) {
                Some(i) => (i as i64 + sens as i64).clamp(0, e.ordre.len() as i64 - 1) as usize,
                None => 0,
            };
            e.choisie = Some(e.ordre[i]);
            e.mois = None;
            drop(e);
            redessiner();
        }
    );

    {
        let (services, etat, plus_tard) =
            (services.clone(), Rc::clone(&etat), Rc::clone(&plus_tard));
        f.on_task_title_edited(move |texte| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            let texte = texte.trim().to_string();
            if !texte.is_empty() && modifier(&services, id, |t| t.title = texte) {
                plus_tard();
            }
        });
    }
    {
        let (services, etat, plus_tard) =
            (services.clone(), Rc::clone(&etat), Rc::clone(&plus_tard));
        f.on_task_notes_edited(move |texte| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            if modifier(&services, id, |t| t.notes = texte.to_string()) {
                plus_tard();
            }
        });
    }
    {
        let (services, etat, plus_tard) =
            (services.clone(), Rc::clone(&etat), Rc::clone(&plus_tard));
        f.on_task_time_edited(move |texte| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            // Vide : toute la journée. Illisible : on attend la suite de la frappe.
            let m = if texte.trim().is_empty() {
                None
            } else {
                match lire_heure(&texte) {
                    Some(m) => Some(m as i32),
                    None => return,
                }
            };
            if modifier(&services, id, |t| t.due_minute = m) {
                plus_tard();
            }
        });
    }

    geste!(
        on_task_picker_toggled,
        [services, etat, redessiner, f, controller],
        || {
            let mut e = etat.borrow_mut();
            let Some(id) = e.choisie else { return };
            e.mois = match e.mois {
                Some(_) => None,
                None => {
                    let j = services
                        .store
                        .task(id)
                        .ok()
                        .flatten()
                        .and_then(|t| jour(&t.task))
                        .unwrap_or(maintenant_local().date());
                    Some(premier_du_mois(j))
                }
            };
            drop(e);
            redessiner();
        }
    );
    geste!(
        on_task_picker_navigate,
        [services, etat, redessiner, f, controller],
        |n| {
            let mut e = etat.borrow_mut();
            e.mois = e.mois.map(|m| plus_mois(m, n));
            drop(e);
            redessiner();
        }
    );
    geste!(
        on_task_picker_chosen,
        [services, etat, redessiner, f, controller],
        |date| {
            let mut e = etat.borrow_mut();
            let (Some(id), Ok(j)) = (e.choisie, NaiveDate::parse_from_str(&date, "%Y-%m-%d"))
            else {
                return;
            };
            e.mois = None;
            drop(e);
            modifier(services, id, |t| {
                t.due_day = Some(j.format("%Y-%m-%d").to_string())
            });
            redessiner();
        }
    );
    geste!(
        on_task_due_cleared,
        [services, etat, redessiner, f, controller],
        || {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            modifier(services, id, |t| {
                t.due_day = None;
                t.due_minute = None;
                t.remind_before = None;
            });
            f.set_task_detail_time(SharedString::default());
            redessiner();
        }
    );
    geste!(
        on_task_reminder_chosen,
        [services, etat, redessiner, f, controller],
        |i| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            let avant = REMINDERS.get(i.max(0) as usize).and_then(|(_, m)| *m);
            modifier(services, id, |t| t.remind_before = avant.map(|m| m as i32));
            redessiner();
        }
    );
    geste!(
        on_task_repeat_chosen,
        [services, etat, redessiner, f, controller],
        |i| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            let regle = iris_tasks::repeat::REPEATS
                .get(i.max(0) as usize)
                .map(|(mot, _)| *mot)
                .filter(|mot| !mot.is_empty())
                .map(str::to_string);
            modifier(services, id, |t| t.repeat = regle);
            redessiner();
        }
    );
    geste!(
        on_task_priority_chosen,
        [services, etat, redessiner, f, controller],
        |p| {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            modifier(services, id, |t| t.priority = p.clamp(0, 3));
            redessiner();
        }
    );
    geste!(
        on_task_list_chosen,
        [services, etat, redessiner, f, controller],
        |i| {
            let e = etat.borrow();
            let (Some(id), Some(liste)) =
                (e.choisie, e.listes.get(i.max(0) as usize).map(|l| l.id))
            else {
                return;
            };
            drop(e);
            modifier(services, id, |t| t.list_id = liste);
            redessiner();
        }
    );

    geste!(
        on_task_step_added,
        [services, etat, redessiner, f, controller],
        |texte| {
            let Some(parent) = etat.borrow().choisie else {
                return;
            };
            let Ok(Some(p)) = services.store.task(parent) else {
                return;
            };
            let _ = services.store.insert_task(
                &NewTask {
                    list_id: p.task.list_id,
                    parent_id: Some(parent),
                    title: texte.trim().to_string(),
                    ..Default::default()
                },
                now(),
            );
            redessiner();
        }
    );
    geste!(
        on_task_step_toggled,
        [services, etat, redessiner, f, controller],
        |id| {
            basculer(services, id as i64);
            redessiner();
        }
    );
    geste!(
        on_task_step_removed,
        [services, etat, redessiner, f, controller],
        |id| {
            let partie = avec_ses_etapes(services, id as i64);
            if services.store.delete_task(id as i64).is_ok() && !partie.is_empty() {
                etat.borrow_mut().supprimees.push(partie);
            }
            redessiner();
        }
    );

    geste!(
        on_task_open_mail,
        [services, etat, redessiner, f, controller],
        || {
            let Some(id) = etat.borrow().choisie else {
                return;
            };
            if let Some(fil) = services
                .store
                .task(id)
                .ok()
                .flatten()
                .and_then(|t| t.task.thread_id)
            {
                open_thread(f, services, controller, fil);
            }
        }
    );

    geste!(
        on_task_delete,
        [services, etat, redessiner, f, controller],
        || {
            let mut e = etat.borrow_mut();
            let Some(id) = e.choisie else { return };
            let partie = avec_ses_etapes(services, id);
            let titre = partie
                .first()
                .map(|t| t.task.title.clone())
                .unwrap_or_default();
            // The panel closes with the task. It used to open the next one, which
            // nobody had asked to look at: the column is there to pick it.
            e.choisie = None;
            e.mois = None;
            if services.store.delete_task(id).is_ok() && !partie.is_empty() {
                e.supprimees.push(partie);
            }
            drop(e);
            f.set_status(format!("Task deleted: {titre}. Ctrl+Z brings it back.").into());
            redessiner();
        }
    );
    // Ctrl+Z in Tasks: the last delete comes undone, and the task is shown again.
    geste!(
        on_task_undo,
        [services, etat, redessiner, f, controller],
        || {
            let Some(partie) = etat.borrow_mut().supprimees.pop() else {
                f.set_status("Nothing to undo.".into());
                return;
            };
            let remises = services.store.restore_tasks(&partie, now()).unwrap_or(0);
            let dessus: Vec<&StoredTask> = partie
                .iter()
                .filter(|t| !partie.iter().any(|p| Some(p.id) == t.task.parent_id))
                .collect();
            if remises == 0 {
                f.set_status("That task's list is gone: it cannot come back.".into());
            } else if let [seule] = dessus.as_slice() {
                // A task put back is shown, where it was: the panel reopens on it.
                etat.borrow_mut().choisie = Some(seule.id);
                f.set_status(format!("Restored: {}.", seule.task.title).into());
            } else {
                f.set_status(format!("{} tasks restored.", dessus.len()).into());
            }
            redessiner();
        }
    );

    geste!(
        on_task_detail_closed,
        [services, etat, redessiner, f, controller],
        || {
            let mut e = etat.borrow_mut();
            e.choisie = None;
            e.mois = None;
            drop(e);
            redessiner();
        }
    );

    geste!(
        on_task_list_name_confirmed,
        [services, etat, redessiner, f, controller],
        |nouvelle, id, nom| {
            let nom = nom.trim().to_string();
            if nom.is_empty() {
                f.set_task_list_name_error("A list needs a name.".into());
                return;
            }
            if nouvelle {
                let couleur = COULEURS[etat.borrow().listes.len() % COULEURS.len()];
                if let Ok(id) = services.store.create_task_list(&nom, couleur, now()) {
                    etat.borrow_mut().vue = Vue::List(id);
                }
            } else {
                let _ = services.store.rename_task_list(id as i64, &nom);
            }
            f.set_task_list_name_error(SharedString::default());
            f.set_task_list_name_open(false);
            redessiner();
        }
    );
    geste!(
        on_task_list_delete_confirmed,
        [services, etat, redessiner, f, controller],
        |id| {
            let id = id as i64;
            let listes = services.store.task_lists().unwrap_or_default();
            if listes.len() <= 1 {
                return;
            }
            let nom = listes
                .iter()
                .find(|l| l.id == id)
                .map(|l| l.name.clone())
                .unwrap_or_default();
            let _ = services.store.delete_task_list(id);
            let mut e = etat.borrow_mut();
            if e.vue == Vue::List(id) {
                e.vue = Vue::Today;
            }
            drop(e);
            f.set_status(format!("List {nom} deleted.").into());
            redessiner();
        }
    );

    geste!(
        on_thread_to_task,
        [services, etat, redessiner, f, controller],
        |fil| {
            match task_from_thread(services, fil as i64) {
                Ok(message) => f.set_status(message.into()),
                Err(e) => f.set_status(format!("Could not add it: {e}").into()),
            }
            if f.get_workspace() == 2 {
                redessiner();
            }
        }
    );

    // Les rappels, toutes les trente secondes ; l'onglet ouvert se tient à jour à la
    // minute (« en retard » change avec l'heure).
    let minuterie = slint::Timer::default();
    {
        let (services, redessiner, faible) =
            (services.clone(), Rc::clone(&redessiner), f.as_weak());
        let mut tours = 0u32;
        minuterie.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(30),
            move || {
                let donnes = rappels(&services);
                tours += 1;
                if let Some(f) = faible.upgrade() {
                    if f.get_workspace() == 2
                        && f.window().is_visible()
                        && (donnes || tours % 2 == 0)
                    {
                        redessiner();
                    }
                }
            },
        );
    }
    MINUTERIES.with(|m| m.borrow_mut().push(minuterie));
}

/// Montre une conversation dans le courrier.
pub fn open_thread(f: &AppWindow, services: &Services, controller: &Controller, fil: i64) {
    let Ok(Some(ligne)) = services.store.thread_row(ThreadId(fil)) else {
        f.set_status("This conversation no longer exists.".into());
        return;
    };
    f.set_workspace(0);
    f.invoke_workspace_changed(0);
    controller.send(Request::SwitchTab(ligne.state));
    controller.send(Request::SelectThread(ThreadId(fil)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_have_keys_that_come_back() {
        for v in [
            Vue::Today,
            Vue::Upcoming,
            Vue::Anytime,
            Vue::Mail,
            Vue::List(7),
            Vue::Goal(3),
            Vue::Goals,
        ] {
            assert_eq!(Vue::depuis(&v.cle()), Some(v));
        }
        assert_eq!(Vue::depuis("list:x"), None);
        assert_eq!(Vue::depuis("goal:x"), None);
    }

    #[test]
    fn a_repeating_task_done_makes_its_next_one_once() {
        let dir = tempfile::tempdir().unwrap();
        let services = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("test")),
        )
        .unwrap();
        let liste = services.store.task_lists().unwrap()[0].id;
        let today = maintenant_local().date();
        let id = services
            .store
            .insert_task(
                &NewTask {
                    list_id: liste,
                    title: "Water the plants".into(),
                    due_day: Some(today.format("%Y-%m-%d").to_string()),
                    due_minute: Some(9 * 60),
                    estimate: Some(15),
                    repeat: Some("weekly".into()),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();

        toggle_done(&services, id);
        let ouvertes = services.store.open_tasks().unwrap();
        assert_eq!(ouvertes.len(), 1, "the next one, and only it, is open");
        let prochaine = &ouvertes[0].task;
        let dans_une_semaine = (today + Duration::days(7)).format("%Y-%m-%d").to_string();
        assert_eq!(
            prochaine.due_day.as_deref(),
            Some(dans_une_semaine.as_str())
        );
        assert_eq!(
            (
                prochaine.due_minute,
                prochaine.estimate,
                prochaine.repeat.as_deref()
            ),
            (Some(9 * 60), Some(15), Some("weekly"))
        );

        // Unticked and ticked again: no second copy.
        toggle_done(&services, id);
        toggle_done(&services, id);
        assert_eq!(services.store.open_tasks().unwrap().len(), 1);
    }

    #[test]
    fn a_slot_is_booked_moved_and_taken_back() {
        let dir = tempfile::tempdir().unwrap();
        let services = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("test")),
        )
        .unwrap();
        let liste = services.store.task_lists().unwrap()[0].id;
        let id = services
            .store
            .insert_task(
                &NewTask {
                    list_id: liste,
                    title: "Write to Atelier".into(),
                    estimate: Some(45),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        let jour = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
        let message = reserver(&services, id, jour, 14 * 60).unwrap();
        assert!(message.starts_with("Blocked 14:00–14:45"), "{message}");
        let (_, a, b) = creneau_reserve(&services, id).expect("booked");
        assert_eq!(b - a, 45 * 60_000, "as long as the task takes");
        let t = services.store.task(id).unwrap().unwrap().task;
        assert_eq!(t.due_day.as_deref(), Some("2026-10-02"));
        assert_eq!(t.due_minute, Some(840));
        assert_eq!(t.event_uid.as_deref(), Some(uid_de_creneau(id).as_str()));

        // Booked again: moved, not doubled.
        reserver(&services, id, jour, 16 * 60).unwrap();
        let cal = calendrier_local(&services).unwrap();
        assert_eq!(services.store.calendar_event_count(cal.id).unwrap(), 1);

        liberer(&services, id);
        assert!(creneau_reserve(&services, id).is_none());
        let t = services.store.task(id).unwrap().unwrap().task;
        assert_eq!(t.event_uid, None);
        assert_eq!(t.due_minute, Some(960), "its hour stays");
    }

    #[test]
    fn times_are_read_loosely() {
        assert_eq!(lire_heure("9"), Some(540));
        assert_eq!(lire_heure("9h30"), Some(570));
        assert_eq!(lire_heure("21:05"), Some(1265));
        assert_eq!(lire_heure("25:00"), None);
        assert_eq!(heure_texte(Some(570)), "09:30");
    }

    #[test]
    fn a_reminder_follows_the_due_date() {
        let mut t = NewTask {
            due_day: Some("2026-09-28".into()),
            due_minute: Some(600),
            remind_before: Some(15),
            ..Default::default()
        };
        recalculer_rappel(&mut t);
        let attendu = vers_ms(
            NaiveDate::from_ymd_opt(2026, 9, 28)
                .unwrap()
                .and_hms_opt(9, 45, 0)
                .unwrap(),
        );
        assert_eq!(t.remind_at, Some(attendu));
        t.due_day = None;
        recalculer_rappel(&mut t);
        assert_eq!(t.remind_at, None);
    }

    #[test]
    fn a_conversation_becomes_a_task_once() {
        let dir = tempfile::tempdir().unwrap();
        let services = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("test")),
        )
        .unwrap();
        // Pas de fil : l'erreur le dit, rien n'est créé.
        assert!(task_from_thread(&services, 12345).is_err());
        assert!(services.store.open_tasks().unwrap().is_empty());
    }
}
