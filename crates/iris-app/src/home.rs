//! The Home screen: the day at a glance.
//!
//! A dial of the day (the Iris mark grown into a clock of the 24 hours, today's events
//! as arcs of their calendar's colour, the time gone by, the present), a greeting, one
//! sentence that says what is waiting, and three columns: the latest mail of the
//! queue, today's events on a line down the hours, the tasks due. Everything is read
//! from the base when Home is shown, and again after each sync or each minute while it
//! stays on screen: a handful of small queries, and nothing while another workspace is
//! showing.

use crate::controller::Controller;
use crate::services::{now, Services};
use chrono::{Local, NaiveDate, Timelike};
use iris_tasks::{due_label, is_overdue};
use iris_ui::{AppWindow, DialArcData, HomeItemData, ThreadRowData};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lines to start the day with. Short, and old enough to belong to everyone.
pub const QUOTES: &[(&str, &str)] = &[
    ("Well begun is half done.", "Aristotle"),
    (
        "The secret of getting ahead is getting started.",
        "Mark Twain",
    ),
    (
        "Simplicity is the ultimate sophistication.",
        "Leonardo da Vinci",
    ),
    (
        "It is not that we have a short time to live, but that we waste a lot of it.",
        "Seneca",
    ),
    (
        "The journey of a thousand miles begins with one step.",
        "Lao Tzu",
    ),
    (
        "Nature does not hurry, yet everything is accomplished.",
        "Lao Tzu",
    ),
    (
        "Do what you can, with what you have, where you are.",
        "Theodore Roosevelt",
    ),
    (
        "Our life is frittered away by detail. Simplify, simplify.",
        "Henry David Thoreau",
    ),
    (
        "Write it on your heart that every day is the best day in the year.",
        "Ralph Waldo Emerson",
    ),
    ("Lost time is never found again.", "Benjamin Franklin"),
    (
        "Energy and persistence conquer all things.",
        "Benjamin Franklin",
    ),
    ("Well done is better than well said.", "Benjamin Franklin"),
    (
        "Knowing is not enough; we must apply.",
        "Johann Wolfgang von Goethe",
    ),
    (
        "Great things are done by a series of small things brought together.",
        "Vincent van Gogh",
    ),
    (
        "Patience is bitter, but its fruit is sweet.",
        "Jean-Jacques Rousseau",
    ),
    (
        "Very little is needed to make a happy life.",
        "Marcus Aurelius",
    ),
    (
        "The happiness of your life depends upon the quality of your thoughts.",
        "Marcus Aurelius",
    ),
    (
        "Waste no more time arguing what a good man should be. Be one.",
        "Marcus Aurelius",
    ),
    (
        "Luck is what happens when preparation meets opportunity.",
        "Seneca",
    ),
    (
        "He who has a why to live can bear almost any how.",
        "Friedrich Nietzsche",
    ),
    (
        "It does not matter how slowly you go as long as you do not stop.",
        "Confucius",
    ),
    (
        "The man who moves a mountain begins by carrying away small stones.",
        "Confucius",
    ),
    ("Begin at once to live.", "Seneca"),
    ("Quality is not an act, it is a habit.", "Aristotle"),
    (
        "Nothing is particularly hard if you divide it into small jobs.",
        "Henry Ford",
    ),
    ("Keep your face always toward the sunshine.", "Walt Whitman"),
    (
        "Be not afraid of going slowly; be afraid only of standing still.",
        "Chinese proverb",
    ),
];

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

/// What the syncs brought, in one line.
pub fn sync_summary(arrived: u64) -> String {
    match arrived {
        0 => "No new mail since you opened Iris".into(),
        1 => "1 new message since you opened Iris".into(),
        n => format!("{n} new messages since you opened Iris"),
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

/// The sentence under the greeting: what is waiting, in plain words.
pub fn summary(unread: usize, due: usize, events_left: usize, evening: bool) -> String {
    let mut parties = vec![if unread == 0 {
        "no unread mail".to_string()
    } else {
        pluriel(unread, "unread message", "unread messages")
    }];
    if due > 0 {
        parties.push(format!(
            "{} today",
            pluriel(due, "task to do", "tasks to do")
        ));
    }
    if events_left > 0 {
        let quand = if evening { "this evening" } else { "today" };
        parties.push(if events_left == 1 {
            format!("one more event {quand}")
        } else {
            format!("{events_left} more events {quand}")
        });
    }
    if unread == 0 && due == 0 && events_left == 0 {
        return "Nothing is waiting for you.".into();
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

// --- The dial ------------------------------------------------------------------------

/// The dial is drawn in a square of this side; the hours run on a circle of this radius.
const DIAL: f64 = 212.0;
const RAYON: f64 = 88.0;

/// A point of the hours' circle: `fraction` of the day, midnight at the top, clockwise.
pub fn dial_point(fraction: f64) -> (f64, f64) {
    let a = fraction.clamp(0.0, 1.0) * std::f64::consts::TAU;
    (DIAL / 2.0 + RAYON * a.sin(), DIAL / 2.0 - RAYON * a.cos())
}

/// The arc of the hours' circle between two fractions of the day, as SVG path commands.
pub fn dial_arc(from: f64, to: f64) -> String {
    let (from, to) = (from.clamp(0.0, 1.0), to.clamp(0.0, 1.0));
    // A whole circle is not an arc: stop a hair short of it.
    let to = to.min(from + 0.9995);
    let ((x1, y1), (x2, y2)) = (dial_point(from), dial_point(to));
    let grand = if to - from > 0.5 { 1 } else { 0 };
    format!("M {x1:.2} {y1:.2} A {RAYON} {RAYON} 0 {grand} 1 {x2:.2} {y2:.2}")
}

/// How long something lasts, in words: "30 min", "1 h", "1 h 30".
fn duree(ms: i64) -> String {
    let minutes = (ms / 60_000).max(0);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

// --- The columns -----------------------------------------------------------------------

fn sous_titre(titre: &str) -> HomeItemData {
    HomeItemData {
        title: titre.into(),
        header: true,
        ..Default::default()
    }
}

/// The tasks worth seeing first: late and today's (done ones struck through), then
/// under "Next" the following ones with a date. At most `max` tasks.
fn taches(services: &Services, max: usize) -> (Vec<HomeItemData>, usize) {
    let maintenant = Local::now().naive_local();
    let today = maintenant.date();
    let jour = |t: &iris_store::NewTask| {
        t.due_day
            .as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    };
    let minute = |t: &iris_store::NewTask| t.due_minute.map(|m| m.clamp(0, 1439) as u32);
    let ouvertes: Vec<iris_store::StoredTask> = services
        .store
        .open_tasks()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.task.parent_id.is_none() && jour(&t.task).is_some())
        .collect();
    let faites_aujourdhui: Vec<iris_store::StoredTask> = services
        .store
        .done_tasks(None, 50)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| jour(&t.task) == Some(today))
        .collect();

    let ligne = |t: &iris_store::StoredTask| {
        let j = jour(&t.task).unwrap_or(today);
        HomeItemData {
            id: t.id as i32,
            title: t.task.title.as_str().into(),
            meta: due_label(j, minute(&t.task), today).into(),
            overdue: !t.is_done() && is_overdue(j, minute(&t.task), maintenant),
            done: t.is_done(),
            priority: t.task.priority,
            ..Default::default()
        }
    };

    let maintenant_dus: Vec<&iris_store::StoredTask> = ouvertes
        .iter()
        .filter(|t| jour(&t.task).is_some_and(|j| j <= today))
        .collect();
    let dues = maintenant_dus.len();
    let mut lignes: Vec<HomeItemData> = maintenant_dus
        .into_iter()
        .chain(faites_aujourdhui.iter())
        .take(max)
        .map(ligne)
        .collect();
    let reste = max.saturating_sub(lignes.len());
    let suivantes: Vec<HomeItemData> = ouvertes
        .iter()
        .filter(|t| jour(&t.task).is_some_and(|j| j > today))
        .take(reste)
        .map(ligne)
        .collect();
    if !suivantes.is_empty() {
        if !lignes.is_empty() {
            lignes.push(sous_titre("Next"));
        }
        lignes.extend(suivantes);
    }
    (lignes, dues)
}

/// What the dial and the Today column show.
struct Journee {
    arcs: Vec<DialArcData>,
    aujourdhui: Vec<HomeItemData>,
    plus_tard: Vec<HomeItemData>,
    total: usize,
    restants: usize,
}

fn journee(services: &Services, maintenant: chrono::DateTime<Local>) -> Journee {
    let today = maintenant.date_naive();
    let minuit = crate::calendar::local_midnight_ms(today);
    let jour_ms = 86_400_000.0;
    let instant = maintenant.timestamp_millis();
    let tous = crate::calendar::upcoming(services, today, 7);
    let (du_jour, apres): (Vec<_>, Vec<_>) = tous.into_iter().partition(|u| u.day == today);

    let fraction = |ms: i64| (ms - minuit) as f64 / jour_ms;
    let arcs = du_jour
        .iter()
        .filter(|u| !u.all_day)
        .map(|u| {
            let (a, b) = (fraction(u.start), fraction(u.end));
            // A short event still shows: at least a quarter of an hour of arc.
            DialArcData {
                path: dial_arc(a, b.max(a + 0.0104)).into(),
                color: crate::calendar::couleur(&u.color),
            }
        })
        .collect();

    let mut aujourdhui = Vec::new();
    let mut maintenant_pose = false;
    for u in &du_jour {
        if !u.all_day && !maintenant_pose && u.start > instant {
            aujourdhui.push(HomeItemData {
                now: true,
                meta: maintenant.format("%H:%M").to_string().into(),
                ..Default::default()
            });
            maintenant_pose = true;
        }
        let mut hint = if u.all_day {
            String::new()
        } else {
            duree(u.end - u.start)
        };
        if !u.location.trim().is_empty() {
            if !hint.is_empty() {
                hint.push_str(", ");
            }
            hint.push_str(u.location.trim());
        }
        aujourdhui.push(HomeItemData {
            key: u.key.as_str().into(),
            title: u.title.as_str().into(),
            hint: hint.into(),
            meta: if u.all_day {
                "All day".into()
            } else {
                u.time.as_str().into()
            },
            color: crate::calendar::couleur(&u.color),
            past: u.past,
            ..Default::default()
        });
    }
    let restants = du_jour
        .iter()
        .filter(|u| !u.all_day && u.start > instant)
        .count();

    let demain = today.succ_opt().unwrap_or(today);
    let plus_tard = apres
        .iter()
        .take(4)
        .map(|u| {
            let jour = if u.day == demain {
                "Tomorrow".to_string()
            } else {
                u.day.format("%a").to_string()
            };
            HomeItemData {
                key: u.key.as_str().into(),
                title: u.title.as_str().into(),
                meta: if u.time.is_empty() {
                    jour
                } else {
                    format!("{jour} {}", u.time)
                }
                .into(),
                color: crate::calendar::couleur(&u.color),
                ..Default::default()
            }
        })
        .collect();

    Journee {
        arcs,
        aujourdhui,
        plus_tard,
        total: du_jour.len(),
        restants,
    }
}

/// The latest conversations of the queue, the most recent first.
fn courrier(services: &Services, max: u32) -> Vec<ThreadRowData> {
    let requete = iris_store::ListQuery {
        state: iris_types::WorkflowState::Todo,
        accounts: Vec::new(),
        hide_snoozed_until: Some(now()),
        limit: max,
        after: None,
        scope: iris_store::Scope::Queue,
        filters: Default::default(),
    };
    services
        .store
        .list_threads(&requete)
        .unwrap_or_default()
        .iter()
        .map(|t| iris_ui::bridge::thread_row(t, "", now(), false))
        .collect()
}

/// Fills the Home screen.
pub fn refresh(f: &AppWindow, services: &Services) {
    let maintenant = Local::now();
    f.set_home_greeting(greeting(maintenant.hour()).into());
    f.set_home_date(maintenant.format("%A %-d %B").to_string().into());
    f.set_home_time(maintenant.format("%H:%M").to_string().into());
    f.set_home_weekday(maintenant.format("%A").to_string().into());
    f.set_home_sync_summary(sync_summary(ARRIVEES.load(Ordering::Relaxed)).into());

    // The dial: what is gone of the day, the present, the events.
    let jour_ms = 86_400_000.0;
    let fraction = (maintenant.timestamp_millis()
        - crate::calendar::local_midnight_ms(maintenant.date_naive())) as f64
        / jour_ms;
    let (x, y) = dial_point(fraction);
    f.set_home_dial_elapsed(if fraction > 0.002 {
        dial_arc(0.0, fraction).into()
    } else {
        Default::default()
    });
    f.set_home_dial_now_x(x as f32);
    f.set_home_dial_now_y(y as f32);

    let j = journee(services, maintenant);
    f.set_home_dial_arcs(ModelRc::new(VecModel::from(j.arcs)));
    f.set_home_events(ModelRc::new(VecModel::from(j.aujourdhui)));
    f.set_home_later(ModelRc::new(VecModel::from(j.plus_tard)));
    f.set_home_events_count(
        match j.total {
            0 => "Nothing today".to_string(),
            n => format!("{} today", pluriel(n, "event", "events")),
        }
        .into(),
    );

    let non_lus = services.store.unread_count().unwrap_or(0) as usize;
    f.set_home_mail(ModelRc::new(VecModel::from(courrier(services, 5))));
    f.set_home_mail_count(
        match non_lus {
            0 => "All read".to_string(),
            n => format!("{} unread", nombre(n)),
        }
        .into(),
    );

    let (lignes, dues) = taches(services, 6);
    f.set_home_tasks(ModelRc::new(VecModel::from(lignes)));
    f.set_home_tasks_count(
        match dues {
            0 => "Nothing due".to_string(),
            n => format!("{n} due today"),
        }
        .into(),
    );

    f.set_home_summary(summary(non_lus, dues, j.restants, maintenant.hour() >= 17).into());
}

thread_local! {
    static CITATION: Cell<usize> = const { Cell::new(0) };
    static MINUTERIE: std::cell::RefCell<Option<slint::Timer>> =
        const { std::cell::RefCell::new(None) };
}

/// Draws a quote other than the one showing.
fn nouvelle_citation(f: &AppWindow) {
    let graine = now().millis().unsigned_abs() as usize;
    let actuelle = CITATION.with(Cell::get);
    let mut i = graine % QUOTES.len();
    if i == actuelle {
        i = (i + 1) % QUOTES.len();
    }
    CITATION.with(|c| c.set(i));
    let (texte, auteur) = QUOTES[i];
    f.set_home_quote(texte.into());
    f.set_home_quote_author(auteur.into());
}

/// Refreshes Home if it is the workspace showing, after a sync.
pub fn refresh_if_shown(f: &AppWindow, services: &Services) {
    if f.get_workspace() == 3 {
        refresh(f, services);
    }
}

fn vers(f: &AppWindow, workspace: i32) {
    f.set_workspace(workspace);
    f.invoke_workspace_changed(workspace);
}

pub fn wire_home(f: &AppWindow, services: &Services, controller: Arc<Controller>) {
    nouvelle_citation(f);
    let redessiner = {
        let (faible, services) = (f.as_weak(), services.clone());
        Rc::new(move || {
            if let Some(f) = faible.upgrade() {
                refresh(&f, &services);
            }
        })
    };
    {
        let (redessiner, faible) = (Rc::clone(&redessiner), f.as_weak());
        crate::workspace::follow(f, move |w| {
            if w == 3 {
                if let Some(f) = faible.upgrade() {
                    nouvelle_citation(&f);
                }
                redessiner();
            }
        });
    }
    {
        let faible = f.as_weak();
        f.on_home_new_quote(move || {
            if let Some(f) = faible.upgrade() {
                nouvelle_citation(&f);
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
        let (services, faible) = (services.clone(), f.as_weak());
        f.on_home_mail_opened(move |fil| {
            if let Some(f) = faible.upgrade() {
                crate::tasks::open_thread(&f, &services, &controller, fil as i64);
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
            if let Ok(Some(t)) = services.store.task(id as i64) {
                let fait = if t.is_done() { None } else { Some(now()) };
                let _ = services.store.set_task_done(t.id, fait);
            }
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
    fn quotes_are_short_and_signed() {
        for (texte, auteur) in QUOTES {
            assert!(texte.len() <= 90, "{texte}");
            assert!(!auteur.is_empty());
            assert!(!texte.contains('—'), "no em-dash: {texte}");
        }
    }

    #[test]
    fn the_sync_line_counts_what_arrived() {
        assert_eq!(sync_summary(0), "No new mail since you opened Iris");
        assert_eq!(sync_summary(1), "1 new message since you opened Iris");
        assert_eq!(sync_summary(12), "12 new messages since you opened Iris");
    }

    #[test]
    fn the_sentence_says_what_is_waiting() {
        assert_eq!(
            summary(4, 2, 1, true),
            "4 unread messages, 2 tasks to do today and one more event this evening."
        );
        assert_eq!(summary(1, 0, 0, false), "1 unread message.");
        assert_eq!(
            summary(0, 1, 3, false),
            "No unread mail, 1 task to do today and 3 more events today."
        );
        assert_eq!(summary(0, 0, 0, false), "Nothing is waiting for you.");
    }

    #[test]
    fn the_dial_runs_clockwise_from_midnight_at_the_top() {
        let (x, y) = dial_point(0.0);
        assert!((x - 106.0).abs() < 1e-9 && (y - 18.0).abs() < 1e-9);
        let (x, y) = dial_point(0.25);
        assert!(
            (x - 194.0).abs() < 1e-9 && (y - 106.0).abs() < 1e-9,
            "06:00 on the right"
        );
        let (x, y) = dial_point(0.5);
        assert!(
            (x - 106.0).abs() < 1e-9 && (y - 194.0).abs() < 1e-9,
            "noon at the bottom"
        );
        assert!(
            dial_arc(0.0, 0.7).contains(" 0 1 1 "),
            "past half the day: the long way"
        );
        assert!(dial_arc(0.1, 0.2).contains(" 0 0 1 "));
    }

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(duree(30 * 60_000), "30 min");
        assert_eq!(duree(60 * 60_000), "1 h");
        assert_eq!(duree(90 * 60_000), "1 h 30");
    }

    #[test]
    fn home_fills_from_an_empty_base() {
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
        for (titre, j) in [("Call the plumber", jour(0)), ("Book the train", jour(2))] {
            services
                .store
                .insert_task(
                    &iris_store::NewTask {
                        list_id: liste,
                        title: titre.into(),
                        due_day: j,
                        ..Default::default()
                    },
                    now(),
                )
                .unwrap();
        }
        let (lignes, dues) = taches(&services, 6);
        assert_eq!(dues, 1);
        assert_eq!(lignes.len(), 3, "today's, a title, the next one");
        assert_eq!(lignes[0].title, "Call the plumber");
        assert!(lignes[1].header);
        assert!(courrier(&services, 5).is_empty());
    }
}
