//! The Home screen: the day, and nothing else.
//!
//! The date, a greeting (with the first name set in the settings), one sentence on
//! what is waiting, the one thing next (the coming event or the next task with an
//! hour) and three ways in with their counts. Read from the base when Home is shown,
//! and again after each sync or each minute while it stays on screen: a handful of
//! small queries, and nothing while another workspace is showing.

use crate::controller::Controller;
use crate::services::{now, Services};
use chrono::{Local, NaiveDate, Timelike};
use iris_tasks::is_overdue;
use iris_ui::{AppWindow, HomeItemData};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Mail that arrived since Iris was opened, counted by the sync loop.
static ARRIVEES: AtomicU64 = AtomicU64::new(0);

/// Called by the sync loop with what each round brought.
pub fn count_arrivals(n: u64) {
    ARRIVEES.fetch_add(n, Ordering::Relaxed);
}

/// "Good morning", by the hour.
pub fn greeting(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=22 => "Good evening",
        _ => "Working late",
    }
}

/// "Good evening, Camille", or without a name when none is set.
pub fn greeting_for(hour: u32, first_name: &str) -> String {
    match first_name.trim() {
        "" => greeting(hour).to_string(),
        nom => format!("{}, {nom}", greeting(hour)),
    }
}

fn nombre(n: usize) -> String {
    iris_ui::format::grouped_count(n as u64)
}

fn pluriel(n: usize, un: &str, plusieurs: &str) -> String {
    if n == 1 {
        format!("1 {un}")
    } else {
        format!("{} {plusieurs}", nombre(n))
    }
}

/// The sentence under the greeting: "12 conversations to answer and 5 tasks (2 late)."
pub fn summary(to_answer: usize, due: usize, late: usize, events_left: usize) -> String {
    if to_answer == 0 && due == 0 && events_left == 0 {
        return "Nothing is waiting for you.".into();
    }
    let mut parties = vec![if to_answer == 0 {
        "no mail to answer".to_string()
    } else {
        format!(
            "{} to answer",
            pluriel(to_answer, "conversation", "conversations")
        )
    }];
    if due > 0 {
        let mut taches = pluriel(due, "task", "tasks");
        if late > 0 {
            taches.push_str(&format!(" ({late} late)"));
        }
        parties.push(taches);
    }
    if events_left > 0 {
        parties.push(pluriel(events_left, "more event", "more events"));
    }
    let phrase = match parties.as_slice() {
        [seule] => seule.clone(),
        [debut @ .., fin] => format!("{} and {fin}", debut.join(", ")),
        [] => String::new(),
    };
    let mut lettres = phrase.chars();
    match lettres.next() {
        Some(p) => format!("{}{}.", p.to_uppercase(), lettres.as_str()),
        None => phrase,
    }
}

/// How long until something, in words: "in 25 min", "in 1 h 40".
pub fn in_how_long(ms: i64) -> String {
    let minutes = (ms / 60_000).max(1);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("in {m} min"),
        (h, 0) => format!("in {h} h"),
        (h, m) => format!("in {h} h {m:02}"),
    }
}

/// One thing of the day, and where it falls in it (minutes since midnight; late ones
/// first, those without an hour last).
struct Prochain {
    rang: i64,
    ligne: HomeItemData,
}

/// What Home shows of the day.
pub struct Journee {
    /// The things of the day, late tasks first, `max` at most.
    pub next: Vec<HomeItemData>,
    /// The one thing next: the event under way or coming, or the next task with an
    /// hour, whichever comes first. What is late is not "next".
    pub upcoming: Option<HomeItemData>,
    /// Open tasks due today or late, and how many of them are late.
    pub due: usize,
    pub late: usize,
    /// Today's events, and those still to come.
    pub events: usize,
    pub events_left: usize,
}

/// The day, from the calendars and the tasks.
pub fn day(services: &Services, max: usize) -> Journee {
    let maintenant = Local::now();
    let today = maintenant.date_naive();
    let instant = maintenant.timestamp_millis();
    let mut tout: Vec<Prochain> = Vec::new();
    // The next thing, with when it starts in milliseconds.
    let mut suivant: Option<(i64, HomeItemData)> = None;
    let mut garder = |debut: i64, ligne: HomeItemData| {
        if suivant.as_ref().is_none_or(|(d, _)| debut < *d) {
            suivant = Some((debut, ligne));
        }
    };
    let quand = |debut: i64, heure: &str| {
        if debut <= instant {
            "Now".to_string()
        } else {
            format!("{heure}, {}", in_how_long(debut - instant))
        }
    };

    // Today's events still to come or under way.
    let evenements: Vec<crate::calendar::Upcoming> = crate::calendar::upcoming(services, today, 1)
        .into_iter()
        .filter(|u| u.day == today)
        .collect();
    let restants = evenements
        .iter()
        .filter(|u| !u.all_day && u.start > instant)
        .count();
    for u in evenements.iter().filter(|u| !u.all_day && u.end > instant) {
        let debut = chrono::DateTime::from_timestamp_millis(u.start)
            .map(|d| d.with_timezone(&Local))
            .map(|d| (d.hour() * 60 + d.minute()) as i64)
            .unwrap_or(0);
        let detail = if u.start <= instant {
            "now".to_string()
        } else if u.start - instant < 3 * 3_600_000 {
            in_how_long(u.start - instant)
        } else {
            u.location.trim().to_string()
        };
        let ligne = HomeItemData {
            key: u.key.as_str().into(),
            title: u.title.as_str().into(),
            meta: u.time.as_str().into(),
            hint: detail.into(),
            color: crate::calendar::couleur(&u.color),
            ..Default::default()
        };
        garder(
            u.start,
            HomeItemData {
                meta: quand(u.start, &u.time).into(),
                ..ligne.clone()
            },
        );
        tout.push(Prochain { rang: debut, ligne });
    }

    // The tasks due today or late.
    let jour = |t: &iris_store::NewTask| {
        t.due_day
            .as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    };
    let dues: Vec<iris_store::StoredTask> = services
        .store
        .open_tasks()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.task.parent_id.is_none() && jour(&t.task).is_some_and(|j| j <= today))
        .collect();
    for t in &dues {
        let j = jour(&t.task).unwrap_or(today);
        let minute = t.task.due_minute.map(|m| m.clamp(0, 1439) as u32);
        let en_retard = is_overdue(j, minute, maintenant.naive_local());
        let (rang, heure) = match (j < today, minute) {
            // From another day: before everything, named by its day.
            (true, _) => (-1, j.format("%a").to_string()),
            (false, Some(m)) => (m as i64, format!("{:02}:{:02}", m / 60, m % 60)),
            // Today, no hour: after the ones that have one.
            (false, None) => (24 * 60, String::new()),
        };
        let ligne = HomeItemData {
            id: t.id as i32,
            title: t.task.title.as_str().into(),
            meta: heure.clone().into(),
            hint: if en_retard {
                "late".into()
            } else {
                Default::default()
            },
            overdue: en_retard,
            priority: t.task.priority,
            ..Default::default()
        };
        if let (false, false, Some(m)) = (en_retard, j < today, minute) {
            let debut = crate::calendar::local_midnight_ms(today) + m as i64 * 60_000;
            garder(
                debut,
                HomeItemData {
                    meta: quand(debut, &heure).into(),
                    ..ligne.clone()
                },
            );
        }
        tout.push(Prochain { rang, ligne });
    }

    let late = tout.iter().filter(|p| p.ligne.overdue).count();
    // Late first, then by the hour, those without one last.
    tout.sort_by_key(|p| p.rang);
    Journee {
        next: tout.into_iter().take(max).map(|p| p.ligne).collect(),
        upcoming: suivant.map(|(_, l)| l),
        due: dues.len(),
        late,
        events: evenements.len(),
        events_left: restants,
    }
}

/// Fills the Home screen.
pub fn refresh(f: &AppWindow, services: &Services) {
    let maintenant = Local::now();
    let nom = crate::settings::current().first_name;
    f.set_home_greeting(greeting_for(maintenant.hour(), &nom).into());
    f.set_home_date(maintenant.format("%A %-d %B").to_string().into());

    let j = day(services, 3);
    let a_traiter = to_answer(services);
    // Beside each way in, what waits there; nothing when nothing does.
    let compte = |n: usize| if n == 0 { String::new() } else { nombre(n) };

    f.set_home_next(ModelRc::new(VecModel::from(
        j.upcoming.into_iter().collect::<Vec<_>>(),
    )));
    f.set_home_mail_count(compte(a_traiter).into());
    f.set_home_tasks_count(compte(j.due).into());
    f.set_home_events_count(compte(j.events).into());
    f.set_home_summary(summary(a_traiter, j.due, j.late, j.events_left).into());
    f.set_tasks_badge(j.due as i32);
}

/// The conversations in the To do queue, over every mailbox.
fn to_answer(services: &Services) -> usize {
    services
        .store
        .todo_counts_by_account(now())
        .map(|c| c.values().map(|n| *n as usize).sum())
        .unwrap_or(0)
}

/// Refreshes Home if it is the workspace showing, after a sync; the rail's count of
/// tasks in any case.
pub fn refresh_if_shown(f: &AppWindow, services: &Services) {
    if f.get_workspace() == 3 {
        refresh(f, services);
    } else {
        update_badges(f, services);
    }
}

/// The rail's count of tasks due today or late.
pub fn update_badges(f: &AppWindow, services: &Services) {
    let today = Local::now().date_naive();
    let dues = services
        .store
        .open_tasks()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            t.task.parent_id.is_none()
                && t.task
                    .due_day
                    .as_deref()
                    .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                    .is_some_and(|j| j <= today)
        })
        .count();
    f.set_tasks_badge(dues as i32);
}

fn vers(f: &AppWindow, workspace: i32) {
    f.set_workspace(workspace);
    f.invoke_workspace_changed(workspace);
}

thread_local! {
    static MINUTERIE: std::cell::RefCell<Option<slint::Timer>> =
        const { std::cell::RefCell::new(None) };
}

pub fn wire_home(f: &AppWindow, services: &Services, _controller: Arc<Controller>) {
    let redessiner = {
        let (faible, services) = (f.as_weak(), services.clone());
        Rc::new(move || {
            if let Some(f) = faible.upgrade() {
                refresh(&f, &services);
            }
        })
    };
    {
        let redessiner = Rc::clone(&redessiner);
        crate::workspace::follow(f, move |w| {
            if w == 3 {
                redessiner();
            }
        });
    }
    {
        let faible = f.as_weak();
        f.on_home_open(move |cible| {
            let Some(f) = faible.upgrade() else { return };
            match cible.as_str() {
                "tasks" => {
                    crate::nav::note(&f, "tasks", "today");
                    vers(&f, 2);
                    f.invoke_task_place_chosen("today".into());
                }
                "calendar" => vers(&f, 1),
                _ => vers(&f, 0),
            }
        });
    }
    {
        let faible = f.as_weak();
        f.on_home_task_opened(move |id| {
            if let Some(f) = faible.upgrade() {
                crate::tasks::show_task(&f, id as i64);
            }
        });
    }
    {
        let (services, redessiner) = (services.clone(), Rc::clone(&redessiner));
        f.on_home_task_toggled(move |id| {
            crate::tasks::toggle_done(&services, id as i64);
            redessiner();
        });
    }
    {
        let faible = f.as_weak();
        f.on_home_event_opened(move |cle| {
            let Some(f) = faible.upgrade() else { return };
            vers(&f, 1);
            // Its week first, then the event over it.
            if let Some(debut) = cle.split_once(':').and_then(|(_, d)| d.parse::<i64>().ok()) {
                let jour = chrono::DateTime::from_timestamp_millis(debut)
                    .map(|d| d.with_timezone(&Local).date_naive())
                    .unwrap_or_else(|| Local::now().date_naive());
                f.invoke_calendar_day_chosen(jour.format("%Y-%m-%d").to_string().into());
            }
            f.invoke_calendar_event_opened(cle);
        });
    }

    // Kept current while it shows: the hour turns, tasks fall due.
    let minuterie = slint::Timer::default();
    {
        let (faible, redessiner) = (f.as_weak(), Rc::clone(&redessiner));
        minuterie.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(60),
            move || {
                if let Some(f) = faible.upgrade() {
                    if f.get_workspace() == 3 && f.window().is_visible() {
                        redessiner();
                    }
                }
            },
        );
    }
    MINUTERIE.with(|m| *m.borrow_mut() = Some(minuterie));
    update_badges(f, services);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_greeting_follows_the_hour() {
        assert_eq!(greeting(7), "Good morning");
        assert_eq!(greeting(13), "Good afternoon");
        assert_eq!(greeting(20), "Good evening");
        assert_eq!(greeting(2), "Working late");
    }

    #[test]
    fn the_greeting_carries_the_first_name() {
        assert_eq!(greeting_for(20, "Camille"), "Good evening, Camille");
        assert_eq!(greeting_for(20, "  "), "Good evening");
    }

    #[test]
    fn the_sentence_says_what_is_waiting() {
        assert_eq!(
            summary(12, 5, 2, 0),
            "12 conversations to answer and 5 tasks (2 late)."
        );
        assert_eq!(summary(1, 0, 0, 0), "1 conversation to answer.");
        assert_eq!(
            summary(0, 1, 0, 3),
            "No mail to answer, 1 task and 3 more events."
        );
        assert_eq!(summary(0, 0, 0, 0), "Nothing is waiting for you.");
    }

    #[test]
    fn a_wait_reads_as_people_say_it() {
        assert_eq!(in_how_long(25 * 60_000), "in 25 min");
        assert_eq!(in_how_long(60 * 60_000), "in 1 h");
        assert_eq!(in_how_long(100 * 60_000), "in 1 h 40");
    }

    #[test]
    fn the_day_puts_late_tasks_first_and_keeps_three() {
        let dir = tempfile::tempdir().unwrap();
        let services = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("test")),
        )
        .unwrap();
        let liste = services.store.task_lists().unwrap()[0].id;
        let today = Local::now().date_naive();
        let jour = |n: i64| {
            Some(
                (today + chrono::Duration::days(n))
                    .format("%Y-%m-%d")
                    .to_string(),
            )
        };
        for (titre, j, m) in [
            ("Buy bread", jour(0), None),
            ("Renew the insurance", jour(-2), None),
            ("Call the plumber", jour(0), Some(23 * 60 + 59)),
            ("Book the train", jour(3), None),
            ("Water the plants", jour(0), None),
        ] {
            services
                .store
                .insert_task(
                    &iris_store::NewTask {
                        list_id: liste,
                        title: titre.into(),
                        due_day: j,
                        due_minute: m,
                        ..Default::default()
                    },
                    now(),
                )
                .unwrap();
        }
        let d = day(&services, 3);
        assert_eq!(d.due, 4, "the next days are not due");
        assert_eq!(d.next.len(), 3);
        assert_eq!(d.next[0].title, "Renew the insurance");
        assert!(d.next[0].overdue);
        assert_eq!(d.next[1].title, "Call the plumber");
    }
}
