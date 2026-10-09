//! Growth: the habits, and the goals that moved here from Tasks.
//!
//! The habits' page shows the week (or the month, or the year) as a circle a day; a
//! circle ticks its day, space ticks today. A goal's page is the tasks' own, drawn
//! beside Growth's column: choosing a goal here hands it to the tasks (`goal:3`), which
//! fill it as before. Nothing is counted on the user's behalf: the days kept are the
//! ones ticked (`iris_growth::habits`).

use crate::services::{now, Services};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone, Timelike};
use iris_growth::habits::{monday, month_of, Day, Schedule, DAY_LETTERS};
use iris_store::{Goal, NewHabit};
use iris_ui::{AppWindow, HabitCellData, HabitData, HabitDetailData};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// The colours a habit can take, in the order the window offers them; new habits
/// take them in turn.
pub const COULEURS: [&str; 8] = [
    "#ff9500", "#30b0c7", "#34c759", "#ff2d55", "#af52de", "#007aff", "#5856d6", "#8e8e93",
];

/// The words of the icons, in the order the window offers them (`HabitIcons.words`).
pub const ICONES: [&str; 14] = [
    "book", "run", "drop", "chat", "heart", "leaf", "moon", "phone", "pencil", "music", "coin",
    "cap", "sun", "sparkle",
];

const MOIS: [&str; 12] = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"];

/// What Growth shows. On the display thread.
struct Etat {
    /// 0 the week, 1 the month, 2 the year.
    mode: i32,
    /// The habit open on the right.
    choisie: Option<i64>,
    /// The habits in the order shown, for the arrows.
    ordre: Vec<i64>,
    /// The habit the window edits; `None` for a new one.
    editee: Option<i64>,
    /// The days ticked in the window, Monday first.
    jours: [bool; 7],
}

/// A habit and its days, as the rules read it.
struct Une {
    h: iris_store::Habit,
    regles: iris_growth::Habit,
}

fn texte_du_jour(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn lire_jour(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn local(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

fn aujourdhui() -> NaiveDate {
    Local::now().date_naive()
}

fn charger(services: &Services) -> Vec<Une> {
    let mut jours: BTreeMap<i64, BTreeMap<NaiveDate, i32>> = BTreeMap::new();
    for c in services.store.habit_checks().unwrap_or_default() {
        if let Some(d) = lire_jour(&c.day) {
            jours.entry(c.habit_id).or_default().insert(d, c.amount);
        }
    }
    services
        .store
        .habits()
        .unwrap_or_default()
        .into_iter()
        .map(|h| Une {
            regles: iris_growth::Habit {
                schedule: Schedule::parse(&h.habit.schedule),
                amount: h.habit.amount,
                created: local(h.created_at.millis()),
                checks: jours.remove(&h.id).unwrap_or_default(),
            },
            h,
        })
        .collect()
}

/// 0 every day, 1 some days, 2 less of.
fn groupe(u: &Une) -> usize {
    if u.h.habit.quit {
        2
    } else if u.regles.schedule == Schedule::Daily {
        0
    } else {
        1
    }
}

const GROUPES: [&str; 3] = ["Every day", "Some days", "Less of"];

fn mesure(u: &Une) -> String {
    format!("{} {}", u.h.habit.amount, u.h.habit.unit)
        .trim()
        .to_string()
}

/// Under its name: "20 pages a day · Read 12 books", "3 a week", "15 min, weekdays".
fn sous_titre(u: &Une, objectifs: &[Goal]) -> String {
    let combien = u.h.habit.amount > 0;
    let rythme = match u.regles.schedule {
        Schedule::Daily if combien => format!("{} a day", mesure(u)),
        Schedule::Daily => String::new(),
        Schedule::Days(_) if combien => {
            format!(
                "{}, {}",
                mesure(u),
                u.regles.schedule.label().to_lowercase()
            )
        }
        Schedule::Days(_) => u.regles.schedule.label(),
        Schedule::PerWeek(n) if combien => format!("{n} a week, {}", mesure(u)),
        Schedule::PerWeek(n) => format!("{n} a week"),
    };
    let but =
        u.h.habit
            .goal_id
            .and_then(|g| objectifs.iter().find(|o| o.id == g))
            .map(|o| o.goal.title.clone());
    match (rythme.is_empty(), but) {
        (_, None) => rythme,
        (true, Some(b)) => b,
        (false, Some(b)) => format!("{rythme} · {b}"),
    }
}

/// The panel's sentence: "20 pages a day.", "On weekdays.", "3 times a week, any days."
fn phrase(u: &Une) -> String {
    let combien = u.h.habit.amount > 0;
    let mut s = match u.regles.schedule {
        Schedule::Daily if combien => format!("{} a day.", mesure(u)),
        Schedule::Daily => "Every day.".to_string(),
        Schedule::Days(_) => {
            let jours = u.regles.schedule.label();
            let jours = if jours == "Weekdays" || jours == "Weekends" {
                jours.to_lowercase()
            } else {
                jours
            };
            if combien {
                format!("{} on {jours}.", mesure(u))
            } else {
                format!("On {jours}.")
            }
        }
        Schedule::PerWeek(_) if combien => {
            format!("{}, {} each time.", u.regles.schedule.label(), mesure(u))
        }
        Schedule::PerWeek(_) => format!("{}, any days.", u.regles.schedule.label()),
    };
    if u.h.habit.quit {
        s.push_str(" Ticked when the day was kept.");
    }
    s
}

fn cellule(u: &Une, d: NaiveDate, today: NaiveDate) -> HabitCellData {
    let (state, fraction, mot) = match u.regles.day(d, today) {
        Day::Done => (1, 1.0, "done"),
        Day::Partial(f) => (2, f, "partly done"),
        Day::Missed => (0, 0.0, "missed"),
        Day::Rest => (3, 0.0, "rest day"),
        Day::Today => (4, 0.0, "to do"),
        Day::Future => (5, 0.0, "to come"),
        Day::Before => (6, 0.0, "before the habit"),
    };
    let cochable = d <= today && d >= u.regles.created;
    HabitCellData {
        state,
        fraction,
        day: if cochable {
            texte_du_jour(d).into()
        } else {
            SharedString::default()
        },
        label: format!("{}, {mot}", d.format("%a %-d %b")).into(),
    }
}

/// The circles of a row: the week, the month's days, or the year's months.
fn cellules(u: &Une, mode: i32, today: NaiveDate) -> Vec<HabitCellData> {
    match mode {
        1 => {
            let (premier, dernier) = month_of(today);
            let mut d = premier;
            let mut v = Vec::new();
            while d <= dernier {
                v.push(cellule(u, d, today));
                d += Duration::days(1);
            }
            v
        }
        2 => (1..=12)
            .map(|m| {
                let premier = NaiveDate::from_ymd_opt(today.year(), m, 1).unwrap_or(today);
                let (premier, dernier) = month_of(premier);
                let nom = premier.format("%B");
                if premier > today {
                    HabitCellData {
                        state: 5,
                        label: format!("{nom}, to come").into(),
                        ..Default::default()
                    }
                } else if dernier < u.regles.created {
                    HabitCellData {
                        state: 6,
                        label: format!("{nom}, before the habit").into(),
                        ..Default::default()
                    }
                } else {
                    let taux = u
                        .regles
                        .rate_between(premier, dernier, today)
                        .unwrap_or(0.0);
                    HabitCellData {
                        state: 1,
                        fraction: taux,
                        label: format!("{nom}, {}%", (taux * 100.0).round() as i32).into(),
                        ..Default::default()
                    }
                }
            })
            .collect(),
        _ => {
            let lundi = monday(today);
            (0..7)
                .map(|k| cellule(u, lundi + Duration::days(k), today))
                .collect()
        }
    }
}

/// At the right of a row: the streak (and a flame), or this week for so many a week.
fn serie(u: &Une, today: NaiveDate) -> (String, bool, bool) {
    if let Schedule::PerWeek(n) = u.regles.schedule {
        return (
            format!("{} of {n}", u.regles.done_in_week(today)),
            false,
            true,
        );
    }
    let s = u.regles.streak(today);
    if s.current > 0 {
        (s.label(), true, true)
    } else if s.best > 0 {
        (format!("best {}", s.best), false, false)
    } else {
        (s.label(), true, false)
    }
}

fn ligne(
    u: &Une,
    mode: i32,
    today: NaiveDate,
    objectifs: &[Goal],
    choisie: Option<i64>,
) -> HabitData {
    let (streak, flame, hot) = serie(u, today);
    HabitData {
        kind: 0,
        id: u.h.id as i32,
        title: u.h.habit.title.as_str().into(),
        subtitle: sous_titre(u, objectifs).into(),
        icon: u.h.habit.icon.as_str().into(),
        color: crate::calendar::couleur(&u.h.habit.color),
        cells: ModelRc::new(VecModel::from(cellules(u, mode, today))),
        streak: streak.into(),
        flame,
        hot,
        done_today: u.regles.done_on(today),
        due_today: u.regles.due_today(today),
        selected: choisie == Some(u.h.id),
    }
}

/// The head of each column of circles, and which one is today.
fn reperes(mode: i32, today: NaiveDate) -> (Vec<SharedString>, i32) {
    match mode {
        1 => {
            let (premier, dernier) = month_of(today);
            let n = dernier.day();
            (
                (1..=n)
                    .map(|j| {
                        if j % 7 == 1 {
                            SharedString::from(j.to_string())
                        } else {
                            SharedString::default()
                        }
                    })
                    .collect(),
                (today - premier).num_days() as i32,
            )
        }
        2 => (
            MOIS.iter().map(|m| SharedString::from(*m)).collect(),
            today.month0() as i32,
        ),
        _ => (
            DAY_LETTERS.iter().map(|m| SharedString::from(*m)).collect(),
            today.weekday().num_days_from_monday() as i32,
        ),
    }
}

/// "5 – 11 October", "28 Sep – 4 Oct".
fn semaine_en_mots(today: NaiveDate) -> String {
    let lundi = monday(today);
    let dimanche = lundi + Duration::days(6);
    if lundi.month() == dimanche.month() {
        format!("{} – {}", lundi.day(), dimanche.format("%-d %B"))
    } else {
        format!("{} – {}", lundi.format("%-d %b"), dimanche.format("%-d %b"))
    }
}

/// How many of today's habits are done, of how many are due.
fn du_jour(habitudes: &[Une], today: NaiveDate) -> (usize, usize) {
    let dues: Vec<&Une> = habitudes
        .iter()
        .filter(|u| u.regles.due_today(today))
        .collect();
    (
        dues.iter().filter(|u| u.regles.done_on(today)).count(),
        dues.len(),
    )
}

fn detail(services: &Services, u: &Une, today: NaiveDate) -> HabitDetailData {
    let s = u.regles.streak(today);
    let debut = monday(today) - Duration::days(7 * 25);
    let grille: Vec<HabitCellData> = (0..182)
        .map(|k| cellule(u, debut + Duration::days(k), today))
        .collect();
    let mut mois: Vec<SharedString> = Vec::new();
    let mut d = month_of(debut).0;
    while d <= today {
        mois.push(d.format("%b").to_string().into());
        d = month_of(d).1 + Duration::days(1);
    }
    while mois.len() > 6 {
        mois.remove(0);
    }
    let but =
        u.h.habit
            .goal_id
            .and_then(|g| crate::goals::for_id(services, g));
    let rythme = if u.h.habit.amount > 0 {
        format!("{}, {}", u.regles.schedule.label(), mesure(u))
    } else {
        u.regles.schedule.label()
    };
    HabitDetailData {
        id: u.h.id as i32,
        title: u.h.habit.title.as_str().into(),
        icon: u.h.habit.icon.as_str().into(),
        color: crate::calendar::couleur(&u.h.habit.color),
        sentence: phrase(u).into(),
        streak: s.current as i32,
        streak_label: if s.weeks {
            "weeks in a row".into()
        } else {
            "days in a row".into()
        },
        best: s.best as i32,
        rate: u
            .regles
            .rate(today)
            .map(|r| format!("{}%", (r * 100.0).round() as i32))
            .unwrap_or_else(|| "—".into())
            .into(),
        grid: ModelRc::new(VecModel::from(grille)),
        grid_count: {
            let n = u.regles.done_between(debut, today);
            format!("{n} day{}", if n == 1 { "" } else { "s" }).into()
        },
        months: ModelRc::new(VecModel::from(mois)),
        has_goal: but.is_some(),
        goal_id: but.as_ref().map(|g| g.id).unwrap_or(-1),
        goal_title: but.as_ref().map(|g| g.title.clone()).unwrap_or_default(),
        goal_color: but.as_ref().map(|g| g.color).unwrap_or_default(),
        goal_line: but
            .as_ref()
            .map(|g| format!("{} {} · {}", g.done, g.of_label, g.badge.to_lowercase()).into())
            .unwrap_or_default(),
        schedule: rythme.into(),
        reminder: match u.h.habit.remind_minute {
            Some(m) => format!("{:02}:{:02}, if not done", m / 60, m % 60).into(),
            None => "None".into(),
        },
        kind: if u.h.habit.quit {
            "Do less".into()
        } else {
            "Do more".into()
        },
        amount: u.h.habit.amount,
        unit: u.h.habit.unit.as_str().into(),
        today_amount: u.regles.amount_on(today),
    }
}

/// Fills the habits' page, the column's count and the panel.
fn remplir(f: &AppWindow, services: &Services, etat: &mut Etat) {
    let today = aujourdhui();
    let habitudes = charger(services);
    let objectifs = services.store.goals().unwrap_or_default();
    if let Some(id) = etat.choisie {
        if !habitudes.iter().any(|u| u.h.id == id) {
            etat.choisie = None;
        }
    }

    let mut lignes = Vec::new();
    etat.ordre.clear();
    for g in 0..3 {
        let du_groupe: Vec<&Une> = habitudes.iter().filter(|u| groupe(u) == g).collect();
        if du_groupe.is_empty() {
            continue;
        }
        lignes.push(HabitData {
            kind: 1,
            title: GROUPES[g].into(),
            streak: if g == 1 && etat.mode == 0 {
                "This week".into()
            } else {
                "Streak".into()
            },
            ..Default::default()
        });
        for u in du_groupe {
            etat.ordre.push(u.h.id);
            lignes.push(ligne(u, etat.mode, today, &objectifs, etat.choisie));
        }
    }
    f.set_growth_mode(etat.mode);
    f.set_growth_rows(ModelRc::new(VecModel::from(lignes)));
    let (reperes, ici) = reperes(etat.mode, today);
    f.set_growth_labels(ModelRc::new(VecModel::from(reperes)));
    f.set_growth_today(ici);
    match etat.mode {
        1 => {
            f.set_growth_title(today.format("%B").to_string().into());
            f.set_growth_dates(today.format("%Y").to_string().into());
        }
        2 => {
            f.set_growth_title(today.format("%Y").to_string().into());
            f.set_growth_dates(SharedString::default());
        }
        _ => {
            f.set_growth_title("This week".into());
            f.set_growth_dates(semaine_en_mots(today).into());
        }
    }
    f.set_growth_note("A missed day never piles up: a habit is only ever about today.".into());
    let (faites, dues) = du_jour(&habitudes, today);
    f.set_growth_count(if dues == 0 {
        SharedString::default()
    } else {
        format!("{faites} of {dues} today").into()
    });
    f.set_growth_habits_count(if dues == 0 {
        SharedString::default()
    } else {
        format!("{faites} of {dues}").into()
    });

    match etat
        .choisie
        .and_then(|id| habitudes.iter().find(|u| u.h.id == id))
    {
        Some(u) => {
            f.set_growth_detail(detail(services, u, today));
            f.set_growth_has_detail(true);
        }
        None => f.set_growth_has_detail(false),
    }
}

/// Home's widget: today's habits (four at most), "2 of 4", the longest streak running.
pub fn fill_home(f: &AppWindow, services: &Services) {
    let today = aujourdhui();
    let habitudes = charger(services);
    let objectifs = services.store.goals().unwrap_or_default();
    let dues: Vec<&Une> = habitudes
        .iter()
        .filter(|u| u.regles.due_today(today))
        .collect();
    f.set_home_habits(ModelRc::new(VecModel::from(
        dues.iter()
            .take(4)
            .map(|u| ligne(u, 0, today, &objectifs, None))
            .collect::<Vec<_>>(),
    )));
    let (faites, n) = du_jour(&habitudes, today);
    f.set_home_habits_figure(if n == 0 {
        SharedString::default()
    } else {
        format!("{faites} of {n}").into()
    });
    let meilleure = habitudes
        .iter()
        .filter(|u| !u.regles.schedule.weekly())
        .map(|u| (u.regles.streak(today).current, u))
        .filter(|(n, _)| *n >= 2)
        .max_by_key(|(n, _)| *n);
    f.set_home_habits_foot(match meilleure {
        Some((n, u)) => format!("{} {n} days in a row", u.h.habit.title).into(),
        None => SharedString::default(),
    });
}

/// Ticks today for habit `id`, or unticks it.
pub fn toggle_today(services: &Services, id: i64) {
    basculer(services, id, aujourdhui());
}

fn basculer(services: &Services, id: i64, jour: NaiveDate) {
    let Some(u) = charger(services).into_iter().find(|u| u.h.id == id) else {
        return;
    };
    if jour > aujourdhui() || jour < u.regles.created {
        return;
    }
    let fait = if u.regles.done_on(jour) {
        0
    } else {
        u.h.habit.amount.max(1)
    };
    let _ = services
        .store
        .set_habit_check(id, &texte_du_jour(jour), fait);
}

/// Opens Growth on `place` ("habits", "goals", "goal:3"), as one step back.
pub fn open(f: &AppWindow, place: &str) {
    crate::nav::note(f, "growth", place);
    f.invoke_growth_place_chosen(place.into());
    if f.get_workspace() != WORKSPACE {
        f.set_workspace(WORKSPACE);
        f.invoke_workspace_changed(WORKSPACE);
    }
}

/// Growth's place in the window: after Home (3) and Notes (4).
pub const WORKSPACE: i32 = 5;

/// "21:30", "21h30", "9" read as a minute of the day; empty is none.
fn lire_heure(texte: &str) -> Result<Option<i32>, ()> {
    let t = texte.trim().to_ascii_lowercase().replace('h', ":");
    if t.is_empty() {
        return Ok(None);
    }
    let (h, m) = match t.split_once(':') {
        Some((h, m)) => (h.trim().to_string(), m.trim().to_string()),
        None => (t.clone(), "0".into()),
    };
    let h: i32 = h.parse().map_err(|_| ())?;
    let m: i32 = if m.is_empty() {
        0
    } else {
        m.parse().map_err(|_| ())?
    };
    if (0..24).contains(&h) && (0..60).contains(&m) {
        Ok(Some(h * 60 + m))
    } else {
        Err(())
    }
}

fn ouvrir_fenetre(f: &AppWindow, etat: &mut Etat, services: &Services, h: Option<&NewHabit>) {
    let n = services.store.habits().map(|v| v.len()).unwrap_or(0);
    let defaut = NewHabit {
        icon: "sparkle".into(),
        color: COULEURS[n % COULEURS.len()].into(),
        schedule: "daily".into(),
        ..Default::default()
    };
    let h = h.unwrap_or(&defaut);
    let schedule = Schedule::parse(&h.schedule);
    f.set_habit_new_title(h.title.as_str().into());
    f.set_habit_new_icon(ICONES.iter().position(|i| *i == h.icon).unwrap_or(13) as i32);
    f.set_habit_new_color(
        COULEURS
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&h.color))
            .unwrap_or(0) as i32,
    );
    f.set_habit_new_kind(h.quit as i32);
    let (often, jours, fois) = match schedule {
        Schedule::Daily => (0, [true, true, true, true, true, false, false], 3),
        Schedule::Days(a) => (1, a, 3),
        Schedule::PerWeek(n) => (2, [true, true, true, true, true, false, false], n),
    };
    etat.jours = jours;
    f.set_habit_new_often(often);
    f.set_habit_new_days(ModelRc::new(VecModel::from(jours.to_vec())));
    f.set_habit_new_times(fois.to_string().into());
    f.set_habit_new_amount(if h.amount > 0 {
        h.amount.to_string().into()
    } else {
        SharedString::default()
    });
    f.set_habit_new_unit(h.unit.as_str().into());
    let objectifs = services.store.goals().unwrap_or_default();
    f.set_habit_new_goal(
        h.goal_id
            .and_then(|g| objectifs.iter().position(|o| o.id == g))
            .map(|i| i as i32 + 1)
            .unwrap_or(0),
    );
    f.set_habit_new_remind(match h.remind_minute {
        Some(m) => format!("{:02}:{:02}", m / 60, m % 60).into(),
        None => SharedString::default(),
    });
    f.set_habit_new_error(SharedString::default());
    f.set_habit_new_editing(etat.editee.is_some());
    f.set_habit_new_open(true);
}

/// Reads the habit window into a habit, or says what is wrong.
fn lire_fenetre(f: &AppWindow, etat: &Etat, services: &Services) -> Result<NewHabit, String> {
    let title = f.get_habit_new_title().trim().to_string();
    if title.is_empty() {
        return Err("Say what the habit is.".into());
    }
    let schedule = match f.get_habit_new_often() {
        1 => {
            if !etat.jours.iter().any(|j| *j) {
                return Err("Choose at least one day.".into());
            }
            Schedule::days(etat.jours)
        }
        2 => {
            let n: u8 = f
                .get_habit_new_times()
                .trim()
                .parse()
                .ok()
                .filter(|n| (1..=7).contains(n))
                .ok_or("Times a week should be a number from 1 to 7.")?;
            Schedule::per_week(n)
        }
        _ => Schedule::Daily,
    };
    let texte = f.get_habit_new_amount();
    let amount = if texte.trim().is_empty() {
        0
    } else {
        texte
            .trim()
            .parse::<i32>()
            .ok()
            .filter(|n| (1..=100_000).contains(n))
            .ok_or("The amount should be a number, like 20.")?
    };
    let remind_minute = lire_heure(&f.get_habit_new_remind())
        .map_err(|_| "The reminder should look like 21:30.")?;
    let objectifs = services.store.goals().unwrap_or_default();
    let goal_id = match f.get_habit_new_goal() {
        i if i >= 1 => objectifs.get(i as usize - 1).map(|g| g.id),
        _ => None,
    };
    Ok(NewHabit {
        title,
        icon: ICONES
            .get(f.get_habit_new_icon().max(0) as usize)
            .copied()
            .unwrap_or("sparkle")
            .into(),
        color: COULEURS
            .get(f.get_habit_new_color().max(0) as usize)
            .copied()
            .unwrap_or(COULEURS[0])
            .into(),
        quit: f.get_habit_new_kind() == 1,
        schedule: schedule.text(),
        amount,
        unit: if amount > 0 {
            f.get_habit_new_unit().trim().to_string()
        } else {
            String::new()
        },
        goal_id,
        remind_minute,
    })
}

/// Habits due today, not done, past their reminder, not reminded yet today: a
/// notification each, once.
fn rappels(services: &Services) -> bool {
    let maintenant = Local::now();
    let today = maintenant.date_naive();
    let minute = (maintenant.hour() * 60 + maintenant.minute()) as i32;
    let jour = texte_du_jour(today);
    let mut fait = false;
    for u in charger(services) {
        let Some(m) = u.h.habit.remind_minute else {
            continue;
        };
        if minute < m
            || u.h.reminded_day.as_deref() == Some(jour.as_str())
            || !u.regles.due_today(today)
            || u.regles.done_on(today)
        {
            continue;
        }
        crate::notify::show_text(&u.h.habit.title, "Not done yet today.");
        let _ = services.store.set_habit_reminded(u.h.id, &jour);
        fait = true;
    }
    fait
}

thread_local! {
    static MINUTERIE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

pub fn wire_growth(f: &AppWindow, services: &Services) {
    let etat = Rc::new(RefCell::new(Etat {
        mode: 0,
        choisie: None,
        ordre: Vec::new(),
        editee: None,
        jours: [true, true, true, true, true, false, false],
    }));
    f.set_habit_colors(ModelRc::new(VecModel::from(
        COULEURS
            .iter()
            .map(|c| crate::calendar::couleur(c))
            .collect::<Vec<_>>(),
    )));

    let redessiner = {
        let (faible, services, etat) = (f.as_weak(), services.clone(), Rc::clone(&etat));
        Rc::new(move || {
            if let Some(f) = faible.upgrade() {
                remplir(&f, &services, &mut etat.borrow_mut());
            }
        })
    };

    // Growth fills itself when it shows.
    {
        let redessiner = Rc::clone(&redessiner);
        crate::workspace::follow(f, move |w| {
            if w == WORKSPACE {
                redessiner();
            }
        });
    }

    // Each gesture gets its own copies of what it touches. The names come from the
    // call: those a macro made up would stay invisible to the body.
    macro_rules! geste {
        ([$s:ident, $e:ident, $r:ident, $f:ident], $installer:ident, || $corps:expr) => {
            geste!([$s, $e, $r, $f], $installer, | | $corps)
        };
        ([$s:ident, $e:ident, $r:ident, $f:ident], $installer:ident, |$($arg:ident),*| $corps:expr) => {{
            let ($s, $e, $r, faible) = ($s.clone(), Rc::clone(&$e), Rc::clone(&$r), $f.as_weak());
            $f.$installer(move |$($arg),*| {
                let Some($f) = faible.upgrade() else { return };
                #[allow(unused_variables)]
                let ($s, $e, $r, $f) = (&$s, &$e, &$r, &$f);
                $corps
            });
        }};
    }

    geste!(
        [services, etat, redessiner, f],
        on_growth_place_chosen,
        |cle| {
            if cle == "habits" {
                f.set_growth_page(0);
            } else {
                f.set_growth_page(1);
                f.invoke_task_place_chosen(cle);
            }
            redessiner();
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_growth_mode_chosen,
        |m| {
            etat.borrow_mut().mode = m.clamp(0, 2);
            redessiner();
        }
    );
    geste!([services, etat, redessiner, f], on_habit_selected, |id| {
        etat.borrow_mut().choisie = Some(id as i64);
        redessiner();
    });
    geste!(
        [services, etat, redessiner, f],
        on_habit_detail_closed,
        || {
            etat.borrow_mut().choisie = None;
            redessiner();
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_habit_cell_clicked,
        |id, jour| {
            if let Some(j) = lire_jour(&jour) {
                basculer(services, id as i64, j);
            }
            redessiner();
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_habit_toggle_selected,
        || {
            let choisie = etat.borrow().choisie;
            if let Some(id) = choisie {
                toggle_today(services, id);
                redessiner();
            }
        }
    );
    geste!([services, etat, redessiner, f], on_habit_move, |n| {
        {
            let mut e = etat.borrow_mut();
            if e.ordre.is_empty() {
                return;
            }
            let ici = e.choisie.and_then(|c| e.ordre.iter().position(|i| *i == c));
            let suivant = match ici {
                None if n > 0 => 0,
                None => e.ordre.len() - 1,
                Some(i) => (i as i32 + n).clamp(0, e.ordre.len() as i32 - 1) as usize,
            };
            e.choisie = Some(e.ordre[suivant]);
        }
        redessiner();
    });
    geste!(
        [services, etat, redessiner, f],
        on_habit_quick_add,
        |texte| {
            let texte = texte.trim().to_string();
            if texte.is_empty() {
                let mut e = etat.borrow_mut();
                e.editee = None;
                ouvrir_fenetre(f, &mut e, services, None);
                return;
            }
            let lu = iris_growth::parse(&texte);
            let n = services.store.habits().map(|v| v.len()).unwrap_or(0);
            let h = NewHabit {
                title: lu.title.clone(),
                icon: lu.icon.into(),
                color: COULEURS[n % COULEURS.len()].into(),
                quit: lu.quit,
                schedule: lu.schedule.text(),
                amount: lu.amount,
                unit: lu.unit.clone(),
                goal_id: None,
                remind_minute: None,
            };
            match services.store.create_habit(&h, now()) {
                Ok(id) => {
                    f.set_growth_add_text(SharedString::default());
                    f.set_status(
                        format!(
                            "Added “{}”: {}.",
                            lu.title,
                            lu.schedule.label().to_lowercase()
                        )
                        .into(),
                    );
                    etat.borrow_mut().choisie = Some(id);
                }
                Err(e) => f.set_status(format!("Could not add the habit: {e}").into()),
            }
            redessiner();
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_habit_new_requested,
        || {
            let mut e = etat.borrow_mut();
            e.editee = None;
            ouvrir_fenetre(f, &mut e, services, None);
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_habit_edit_requested,
        || {
            let mut e = etat.borrow_mut();
            let Some(id) = e.choisie else { return };
            let Some(h) = services.store.habit(id).ok().flatten() else {
                return;
            };
            e.editee = Some(id);
            ouvrir_fenetre(f, &mut e, services, Some(&h.habit));
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_habit_new_day_toggled,
        |i| {
            let mut e = etat.borrow_mut();
            if let Some(j) = e.jours.get_mut(i as usize) {
                *j = !*j;
            }
            f.set_habit_new_days(ModelRc::new(VecModel::from(e.jours.to_vec())));
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_habit_new_confirmed,
        || {
            let lu = lire_fenetre(f, &etat.borrow(), services);
            let h = match lu {
                Ok(h) => h,
                Err(m) => {
                    f.set_habit_new_error(m.into());
                    return;
                }
            };
            let editee = etat.borrow().editee;
            let resultat = match editee {
                Some(id) => services.store.update_habit(id, &h).map(|_| id),
                None => services.store.create_habit(&h, now()),
            };
            match resultat {
                Ok(id) => {
                    f.set_habit_new_open(false);
                    etat.borrow_mut().choisie = Some(id);
                    redessiner();
                }
                Err(e) => f.set_habit_new_error(e.to_string().into()),
            }
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_habit_delete_confirmed,
        || {
            let choisie = etat.borrow_mut().choisie.take();
            if let Some(id) = choisie {
                match services.store.delete_habit(id) {
                    Ok(()) => f.set_status("Habit deleted.".into()),
                    Err(e) => f.set_status(format!("Could not delete the habit: {e}").into()),
                }
            }
            redessiner();
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_habit_goal_opened,
        |id| {
            open(f, &format!("goal:{id}"));
        }
    );
    geste!([services, etat, redessiner, f], on_habit_amount_step, |n| {
        let choisie = etat.borrow().choisie;
        let Some(id) = choisie else { return };
        let Some(u) = charger(services).into_iter().find(|u| u.h.id == id) else {
            return;
        };
        let voulu = u.h.habit.amount.max(1);
        let pas = if voulu >= 8 { voulu / 4 } else { 1 };
        let today = aujourdhui();
        let nouveau = (u.regles.amount_on(today) + n * pas).clamp(0, voulu * 10);
        let _ = services
            .store
            .set_habit_check(id, &texte_du_jour(today), nouveau);
        redessiner();
    });

    // Reminders, looked at every half minute; the page kept current while it shows.
    let minuterie = slint::Timer::default();
    {
        let (faible, services, redessiner) =
            (f.as_weak(), services.clone(), Rc::clone(&redessiner));
        minuterie.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(30),
            move || {
                let rappele = rappels(&services);
                if let Some(f) = faible.upgrade() {
                    if rappele && f.get_workspace() == WORKSPACE {
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
    fn the_hour_of_a_reminder_reads_as_people_write_it() {
        assert_eq!(lire_heure("21:30"), Ok(Some(21 * 60 + 30)));
        assert_eq!(lire_heure("7h"), Ok(Some(7 * 60)));
        assert_eq!(lire_heure(" "), Ok(None));
        assert_eq!(lire_heure("25:00"), Err(()));
    }

    #[test]
    fn a_week_says_its_days() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(semaine_en_mots(d("2026-10-09")), "5 – 11 October");
        assert_eq!(semaine_en_mots(d("2026-10-01")), "28 Sep – 4 Oct");
    }

    #[test]
    fn the_icons_are_the_windows_own() {
        // The window offers them in this order (`HabitIcons.words` in icons.slint).
        let source = include_str!("../../iris-ui/ui/base/icons.slint");
        let debut = source.find("out property <[string]> words").unwrap();
        let fin = debut + source[debut..].find("];").unwrap();
        let mots: Vec<&str> = source[debut..fin].split('"').skip(1).step_by(2).collect();
        assert_eq!(mots, ICONES);
    }

    #[test]
    fn a_habit_is_ticked_and_unticked_today_and_never_tomorrow() {
        let dir = tempfile::tempdir().unwrap();
        let s = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("maitre")),
        )
        .unwrap();
        let id = s
            .store
            .create_habit(
                &NewHabit {
                    title: "Read".into(),
                    icon: "book".into(),
                    color: COULEURS[0].into(),
                    schedule: "daily".into(),
                    amount: 20,
                    unit: "pages".into(),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        toggle_today(&s, id);
        let jours = s.store.habit_checks().unwrap();
        assert_eq!((jours.len(), jours[0].amount), (1, 20), "the whole amount");
        toggle_today(&s, id);
        assert!(s.store.habit_checks().unwrap().is_empty());
        basculer(&s, id, aujourdhui() + Duration::days(1));
        assert!(s.store.habit_checks().unwrap().is_empty(), "not tomorrow");
    }
}
