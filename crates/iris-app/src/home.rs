//! The Home screen: the day at a glance.
//!
//! A greeting and the date, a line to start with, four figures — unread, the queue,
//! what arrived today, the tasks due — the tasks coming up and the week's events.
//! Everything is read from the base when Home is shown, and again after each sync
//! while it stays on screen: it costs a handful of counts, and nothing runs while
//! another workspace is showing.

use crate::services::{now, Services};
use chrono::{Datelike, Duration, Local, NaiveDate, Timelike};
use iris_tasks::{due_label, is_overdue};
use iris_ui::{AppWindow, HomeItemData, HomeStatData};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

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

fn nombre(n: u32) -> String {
    iris_ui::format::grouped_count(n as u64)
}

fn pluriel(n: u32, un: &str, plusieurs: &str) -> String {
    if n == 1 {
        format!("1 {un}")
    } else {
        format!("{} {plusieurs}", nombre(n))
    }
}

fn titre_jour(jour: NaiveDate, today: NaiveDate) -> String {
    if jour == today {
        "TODAY".into()
    } else if jour == today + Duration::days(1) {
        "TOMORROW".into()
    } else {
        jour.format("%A %-d %b").to_string().to_uppercase()
    }
}

fn en_tete(titre: &str, rouge: bool) -> HomeItemData {
    HomeItemData {
        title: titre.into(),
        header: true,
        overdue: rouge,
        ..Default::default()
    }
}

/// The tasks worth seeing first: late, today's (done ones struck through), then the
/// next ones with a date. At most `max` of them.
fn taches(services: &Services, max: usize) -> Vec<HomeItemData> {
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
            ..Default::default()
        }
    };

    let mut lignes = Vec::new();
    let mut compte = 0;
    let mut section = |titre: &str,
                       rouge: bool,
                       dedans: Vec<&iris_store::StoredTask>,
                       lignes: &mut Vec<HomeItemData>| {
        let dedans: Vec<&iris_store::StoredTask> = dedans
            .into_iter()
            .take(max.saturating_sub(compte))
            .collect();
        if dedans.is_empty() {
            return;
        }
        lignes.push(en_tete(titre, rouge));
        compte += dedans.len();
        lignes.extend(dedans.into_iter().map(ligne));
    };
    let en_retard: Vec<&iris_store::StoredTask> = ouvertes
        .iter()
        .filter(|t| jour(&t.task).is_some_and(|j| j < today))
        .collect();
    let du_jour: Vec<&iris_store::StoredTask> = ouvertes
        .iter()
        .filter(|t| jour(&t.task) == Some(today))
        .chain(faites_aujourdhui.iter())
        .collect();
    let a_venir: Vec<&iris_store::StoredTask> = ouvertes
        .iter()
        .filter(|t| jour(&t.task).is_some_and(|j| j > today))
        .collect();
    section("LATE", true, en_retard, &mut lignes);
    section("TODAY", false, du_jour, &mut lignes);
    section("COMING UP", false, a_venir, &mut lignes);
    lignes
}

/// The week's events, under the title of their day.
fn evenements(services: &Services, max: usize) -> (Vec<HomeItemData>, usize) {
    let today = Local::now().date_naive();
    let tous = crate::calendar::upcoming(services, today, 7);
    let aujourdhui = tous.iter().filter(|u| u.day == today).count();
    let mut lignes = Vec::new();
    let mut jour_courant = None;
    for u in tous.iter().take(max) {
        if jour_courant != Some(u.day) {
            lignes.push(en_tete(&titre_jour(u.day, today), false));
            jour_courant = Some(u.day);
        }
        lignes.push(HomeItemData {
            key: u.key.as_str().into(),
            title: u.title.as_str().into(),
            meta: if u.time.is_empty() {
                "All day".into()
            } else {
                u.time.as_str().into()
            },
            color: crate::calendar::couleur(&u.color),
            past: u.past,
            ..Default::default()
        });
    }
    (lignes, aujourdhui)
}

/// Fills the Home screen.
pub fn refresh(f: &AppWindow, services: &Services) {
    let maintenant = Local::now();
    let today = maintenant.date_naive();
    f.set_home_greeting(greeting(maintenant.hour()).into());
    f.set_home_date(maintenant.format("%A %-d %B").to_string().into());
    f.set_home_sync_summary(sync_summary(ARRIVEES.load(Ordering::Relaxed)).into());

    // The figures.
    let minuit = crate::calendar::local_midnight_ms(today);
    let lundi = crate::calendar::local_midnight_ms(
        today - Duration::days(today.weekday().num_days_from_monday() as i64),
    );
    let chiffres = services
        .store
        .mail_stats(
            iris_types::Timestamp::from_millis(minuit),
            iris_types::Timestamp::from_millis(lundi),
        )
        .unwrap_or_default();
    let non_lus = services.store.unread_count().unwrap_or(0);
    let a_traiter: u32 = services
        .store
        .todo_counts_by_account(now())
        .unwrap_or_default()
        .values()
        .sum();
    let comptes = services
        .store
        .accounts()
        .unwrap_or_default()
        .iter()
        .filter(|c| c.enabled)
        .count() as u32;
    let taches_ouvertes = services.store.open_tasks().unwrap_or_default();
    let dues = taches_ouvertes
        .iter()
        .filter(|t| {
            t.task.parent_id.is_none()
                && t.task
                    .due_day
                    .as_deref()
                    .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                    .is_some_and(|j| j <= today)
        })
        .count() as u32;
    let faites_semaine = services
        .store
        .done_tasks(None, 500)
        .unwrap_or_default()
        .iter()
        .filter(|t| t.done_at.is_some_and(|d| d.millis() >= lundi))
        .count() as u32;
    let (evts, evts_du_jour) = evenements(services, 14);

    let bleu = slint::Color::from_rgb_u8(0x5b, 0x8d, 0xef);
    let vert = slint::Color::from_rgb_u8(0x4f, 0xb2, 0x86);
    let ambre = slint::Color::from_rgb_u8(0xe3, 0xb3, 0x41);
    let rose = slint::Color::from_rgb_u8(0xe0, 0x79, 0x5b);
    f.set_home_stats(ModelRc::new(VecModel::from(vec![
        HomeStatData {
            value: nombre(non_lus).into(),
            label: "Unread".into(),
            hint: format!("across {}", pluriel(comptes, "account", "accounts")).into(),
            target: "mail".into(),
            tint: bleu,
        },
        HomeStatData {
            value: nombre(a_traiter).into(),
            label: "To do in your mail".into(),
            hint: "conversations waiting for you".into(),
            target: "mail".into(),
            tint: rose,
        },
        HomeStatData {
            value: nombre(chiffres.received_today).into(),
            label: "Received today".into(),
            hint: format!(
                "{} this week, {} sent",
                nombre(chiffres.received_week),
                nombre(chiffres.sent_week)
            )
            .into(),
            target: "mail".into(),
            tint: vert,
        },
        HomeStatData {
            value: nombre(dues).into(),
            label: if dues == 1 {
                "Task for today"
            } else {
                "Tasks for today"
            }
            .into(),
            hint: format!(
                "{} done this week, {} today",
                nombre(faites_semaine),
                pluriel(evts_du_jour as u32, "event", "events")
            )
            .into(),
            target: "tasks".into(),
            tint: ambre,
        },
    ])));

    f.set_home_tasks(ModelRc::new(VecModel::from(taches(services, 8))));
    f.set_home_events(ModelRc::new(VecModel::from(evts)));
}

thread_local! {
    static CITATION: Cell<usize> = const { Cell::new(0) };
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
    f.set_home_quote_author(auteur.to_uppercase().into());
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

pub fn wire_home(f: &AppWindow, services: &Services) {
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

thread_local! {
    static MINUTERIE: std::cell::RefCell<Option<slint::Timer>> = const { std::cell::RefCell::new(None) };
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
    fn home_fills_from_an_empty_base() {
        let dir = tempfile::tempdir().unwrap();
        let services = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("test")),
        )
        .unwrap();
        let liste = services.store.task_lists().unwrap()[0].id;
        let today = Local::now().date_naive();
        services
            .store
            .insert_task(
                &iris_store::NewTask {
                    list_id: liste,
                    title: "Call the plumber".into(),
                    due_day: Some(today.format("%Y-%m-%d").to_string()),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        let lignes = taches(&services, 8);
        assert_eq!(lignes.len(), 2, "a title and the task");
        assert!(lignes[0].header);
        assert_eq!(lignes[1].title, "Call the plumber");
    }
}
