//! Goals in the Tasks tab: the side column's rings, a goal's page, all of them, the
//! nudges on Today, and the windows that make one or set time aside for it.
//!
//! Where a goal stands is read from its log (or its milestones) against a steady pace
//! from the day it was set to its day (`iris_tasks::goals`). Nothing is counted for
//! the user: a step done is logged by the user, with "Log one".

use crate::services::{now, Services};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use iris_store::{Goal, GoalKind, NewEvent, NewGoal};
use iris_tasks::goals::{chart, pace, ring, Pace, PaceStatus};
use iris_ui::{AppWindow, GoalData, GoalLogData};
use slint::{ModelRc, SharedString, VecModel};

/// The colours given to new goals, in turn.
const COULEURS: [&str; 7] = [
    "#8fa2ff", "#e8a45b", "#6fb7a4", "#e07a6a", "#b58ad6", "#5aa9c9", "#c9a14a",
];

/// How long each choice of the time window is, in minutes.
pub const LONGUEURS: [i64; 4] = [30, 60, 90, 120];

fn jour_de(texte: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(texte.trim(), "%Y-%m-%d").ok()
}

fn local(ms: i64) -> NaiveDate {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

/// A day as the log says it: "Today", "Yesterday", "Sat 26 Sep".
fn jour_lisible(d: NaiveDate, today: NaiveDate) -> String {
    match (today - d).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ => d.format("%a %-d %b").to_string(),
    }
}

/// Where a goal stands: how many done (log entries, or milestones ticked), of how many.
fn avancement(services: &Services, g: &Goal) -> (i32, i32) {
    match g.goal.kind {
        GoalKind::Count => (
            services.store.goal_entries(g.id).unwrap_or_default().len() as i32,
            g.goal.target.max(1),
        ),
        GoalKind::Milestones => {
            let m = services.store.milestones(g.id).unwrap_or_default();
            (
                m.iter().filter(|m| m.done_at.is_some()).count() as i32,
                (m.len() as i32).max(1),
            )
        }
    }
}

fn allure(services: &Services, g: &Goal, today: NaiveDate) -> (i32, i32, Pace) {
    let (fait, cible) = avancement(services, g);
    let debut = local(g.created_at.millis());
    let fin = jour_de(&g.goal.due_day).unwrap_or(today);
    let p = pace(
        fait,
        cible,
        debut,
        fin,
        today,
        g.goal.kind == GoalKind::Milestones,
    );
    (fait, cible, p)
}

/// A goal as the interface shows it.
fn donnees(services: &Services, g: &Goal, today: NaiveDate, choisi: bool) -> GoalData {
    let (fait, cible, p) = allure(services, g, today);
    let fin = jour_de(&g.goal.due_day).unwrap_or(today);
    let ouvertes = services
        .store
        .tasks_for_goal(g.id)
        .unwrap_or_default()
        .iter()
        .filter(|t| !t.is_done())
        .count();
    let unite = if g.goal.kind == GoalKind::Milestones {
        "milestones".to_string()
    } else {
        g.goal.unit.clone()
    };
    GoalData {
        id: g.id as i32,
        title: g.goal.title.as_str().into(),
        why: g.goal.why.as_str().into(),
        color: crate::calendar::couleur(&g.goal.color),
        milestones: g.goal.kind == GoalKind::Milestones,
        done: fait,
        target: cible,
        of_label: format!("of {cible} {unite}").trim().to_string().into(),
        due_label: format!("by {}", fin.format("%A %-d %B")).into(),
        due_short: fin.format("%a %-d %b").to_string().into(),
        status: match p.status {
            PaceStatus::OnTrack => 0,
            PaceStatus::Behind => 1,
            PaceStatus::Reached => 2,
        },
        badge: p.badge().into(),
        pace: p.text.as_str().into(),
        fraction: fait as f32 / cible as f32,
        expected: p.expected / cible as f32,
        ring: ring(fait as f32 / cible as f32).into(),
        open_steps: ouvertes as i32,
        selected: choisi,
    }
}

/// The goal Home shows: the first still under way, in the goals' order.
pub fn for_home(services: &Services) -> Option<GoalData> {
    let today = Local::now().date_naive();
    services
        .store
        .goals()
        .unwrap_or_default()
        .iter()
        .map(|g| donnees(services, g, today, false))
        .find(|g| g.status != 2)
}

/// Goal `id` as the interface shows it, for a habit that supports it.
pub fn for_id(services: &Services, id: i64) -> Option<GoalData> {
    let today = Local::now().date_naive();
    services
        .store
        .goal(id)
        .ok()
        .flatten()
        .map(|g| donnees(services, &g, today, false))
}

/// Should Today point to this goal? Behind and near its day or well behind, or due
/// within the week.
fn a_pousser(p: &Pace) -> bool {
    p.status != PaceStatus::Reached
        && ((p.status == PaceStatus::Behind && (p.days_left <= 21 || p.behind >= 2))
            || p.days_left <= 7)
}

/// Fills everything about goals. `montre`: the goal whose page is open; `sur_today`:
/// Today is shown, which lists the goals that need something.
pub fn remplir(
    f: &AppWindow,
    services: &Services,
    montre: Option<i64>,
    tous: bool,
    sur_today: bool,
    revue: bool,
) {
    let today = Local::now().date_naive();
    let objectifs = services.store.goals().unwrap_or_default();

    f.set_goals(ModelRc::new(VecModel::from(
        objectifs
            .iter()
            .map(|g| donnees(services, g, today, montre == Some(g.id)))
            .collect::<Vec<_>>(),
    )));
    f.set_goal_names(ModelRc::new(VecModel::from(
        std::iter::once(SharedString::from("None"))
            .chain(
                objectifs
                    .iter()
                    .map(|g| SharedString::from(g.goal.title.as_str())),
            )
            .collect::<Vec<_>>(),
    )));
    // On Today, the goals that need something this week; in the week's review, every
    // goal not reached yet.
    f.set_goal_nudges(ModelRc::new(VecModel::from(if sur_today {
        objectifs
            .iter()
            .filter(|g| a_pousser(&allure(services, g, today).2))
            .take(3)
            .map(|g| donnees(services, g, today, false))
            .collect::<Vec<_>>()
    } else if revue {
        objectifs
            .iter()
            .map(|g| donnees(services, g, today, false))
            .filter(|d| d.status != 2)
            .take(3)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    })));

    let page = objectifs.iter().find(|g| Some(g.id) == montre);
    f.set_task_page(match (page, tous) {
        (Some(_), _) => 1,
        (None, true) => 2,
        _ => 0,
    });
    let Some(g) = page else { return };
    f.set_goal(donnees(services, g, today, true));

    let debut = local(g.created_at.millis());
    let fin = jour_de(&g.goal.due_day).unwrap_or(today);
    let duree = (fin - debut).num_days().max(1) as f32;
    let fraction = |d: NaiveDate| (d - debut).num_days() as f32 / duree;

    let (journal, pas): (Vec<GoalLogData>, Vec<f32>) = match g.goal.kind {
        GoalKind::Count => {
            let entrees = services.store.goal_entries(g.id).unwrap_or_default();
            let pas = entrees
                .iter()
                .map(|e| fraction(local(e.at.millis())))
                .collect();
            let journal = entrees
                .iter()
                .enumerate()
                .rev()
                .map(|(i, e)| GoalLogData {
                    id: e.id as i32,
                    number: (i + 1).to_string().into(),
                    title: if e.note.trim().is_empty() {
                        "Progress".into()
                    } else {
                        e.note.as_str().into()
                    },
                    when: jour_lisible(local(e.at.millis()), today).into(),
                    done: false,
                })
                .collect();
            (journal, pas)
        }
        GoalKind::Milestones => {
            let m = services.store.milestones(g.id).unwrap_or_default();
            let prochaine = m.iter().position(|m| m.done_at.is_none());
            let pas = m
                .iter()
                .filter_map(|m| m.done_at)
                .map(|t| fraction(local(t.millis())))
                .collect();
            let journal = m
                .iter()
                .enumerate()
                .map(|(i, m)| GoalLogData {
                    id: m.id as i32,
                    number: (i + 1).to_string().into(),
                    title: m.title.as_str().into(),
                    when: match m.done_at {
                        Some(t) => {
                            let jour = local(t.millis());
                            let mot = jour_lisible(jour, today);
                            if (today - jour).num_days() <= 1 {
                                format!("Done {}", mot.to_lowercase()).into()
                            } else {
                                format!("Done {mot}").into()
                            }
                        }
                        None if Some(i) == prochaine => "Next".into(),
                        None => SharedString::default(),
                    },
                    done: m.done_at.is_some(),
                })
                .collect();
            (journal, pas)
        }
    };
    let (_, cible) = avancement(services, g);
    let (chemin, nx, ny) = chart(&pas, cible, fraction(today));
    f.set_goal_log(ModelRc::new(VecModel::from(journal)));
    f.set_goal_chart(chemin.into());
    f.set_goal_now_x(nx);
    f.set_goal_now_y(ny);
    f.set_goal_start_label(debut.format("%a %-d %b").to_string().into());
}

/// Opens the new-goal window, due in a month.
pub fn ouvrir_nouveau(f: &AppWindow) {
    let dans_un_mois = Local::now().date_naive() + Duration::days(30);
    f.set_goal_new_title(SharedString::default());
    f.set_goal_new_kind(0);
    f.set_goal_new_target("10".into());
    f.set_goal_new_unit(SharedString::default());
    f.set_goal_new_due(dans_un_mois.format("%Y-%m-%d").to_string().into());
    f.set_goal_new_why(SharedString::default());
    f.set_goal_new_error(SharedString::default());
    f.set_goal_new_editing(false);
    f.set_goal_new_open(true);
}

/// Opens the same window on goal `id`, its fields filled in.
pub fn ouvrir_edition(f: &AppWindow, services: &Services, id: i64) {
    let Some(g) = services.store.goal(id).ok().flatten() else {
        return;
    };
    let jalons = g.goal.kind == GoalKind::Milestones;
    f.set_goal_new_title(g.goal.title.as_str().into());
    f.set_goal_new_kind(jalons as i32);
    f.set_goal_new_target(g.goal.target.to_string().into());
    f.set_goal_new_unit(g.goal.unit.as_str().into());
    f.set_goal_new_due(g.goal.due_day.as_str().into());
    f.set_goal_new_why(g.goal.why.as_str().into());
    f.set_goal_new_error(SharedString::default());
    f.set_goal_new_editing(true);
    f.set_goal_new_open(true);
}

/// A new title for goal `id`, typed over the old one on its page.
pub fn renommer(services: &Services, id: i64, titre: &str) -> Result<(), String> {
    let titre = titre.trim();
    if titre.is_empty() {
        return Err("A goal needs a name.".into());
    }
    let g = services
        .store
        .goal(id)
        .ok()
        .flatten()
        .ok_or("This goal no longer exists.")?;
    let mut change = g.goal.clone();
    change.title = titre.to_string();
    services
        .store
        .update_goal(id, &change)
        .map_err(|e| e.to_string())
}

pub fn dans_jours(f: &AppWindow, jours: i64) {
    let d = Local::now().date_naive() + Duration::days(jours);
    f.set_goal_new_due(d.format("%Y-%m-%d").to_string().into());
}

/// Reads the goal window and makes the goal, or saves goal `edite` with what it says
/// (keeping how it is measured and its colour); the goal's id, or what is wrong.
pub fn creer(f: &AppWindow, services: &Services, edite: Option<i64>) -> Result<i64, String> {
    let titre = f.get_goal_new_title().trim().to_string();
    if titre.is_empty() {
        return Err("Say what you want to reach.".into());
    }
    let jalons = f.get_goal_new_kind() == 1;
    let cible = if jalons {
        1
    } else {
        f.get_goal_new_target()
            .trim()
            .parse::<i32>()
            .ok()
            .filter(|n| (1..=100_000).contains(n))
            .ok_or("The target should be a number, like 10.")?
    };
    let jour = jour_de(&f.get_goal_new_due()).ok_or("The date should look like 2026-10-30.")?;
    // A goal being changed may keep today as its day; a new one looks ahead.
    if jour < Local::now().date_naive() || (edite.is_none() && jour == Local::now().date_naive()) {
        return Err("The date should be after today.".into());
    }
    let n = services.store.goals().map(|g| g.len()).unwrap_or(0);
    let unite = f.get_goal_new_unit().trim().to_string();
    if let Some(id) = edite {
        let ancien = services
            .store
            .goal(id)
            .ok()
            .flatten()
            .ok_or("This goal no longer exists.")?;
        let mut change = ancien.goal.clone();
        change.title = titre;
        change.why = f.get_goal_new_why().trim().to_string();
        change.due_day = jour.format("%Y-%m-%d").to_string();
        if change.kind == GoalKind::Count {
            change.target = cible;
            if !unite.is_empty() {
                change.unit = unite;
            }
        }
        services
            .store
            .update_goal(id, &change)
            .map_err(|e| e.to_string())?;
        return Ok(id);
    }
    let objectif = NewGoal {
        title: titre,
        why: f.get_goal_new_why().trim().to_string(),
        kind: if jalons {
            GoalKind::Milestones
        } else {
            GoalKind::Count
        },
        target: cible,
        unit: if unite.is_empty() && !jalons {
            "times".into()
        } else {
            unite
        },
        due_day: jour.format("%Y-%m-%d").to_string(),
        color: COULEURS[n % COULEURS.len()].into(),
    };
    services
        .store
        .create_goal(&objectif, now())
        .map_err(|e| e.to_string())
}

/// Opens the window to set time aside for a goal: on the next weekday, at 18:00, an
/// hour.
pub fn ouvrir_temps(f: &AppWindow) {
    let demain = (Local::now().date_naive() + Duration::days(1))
        .weekday()
        .num_days_from_monday();
    f.set_goal_time_days(ModelRc::new(VecModel::from(
        (0..7).map(|i| i == demain.min(4)).collect::<Vec<_>>(),
    )));
    f.set_goal_time_start("18:00".into());
    f.set_goal_time_length(1);
    f.set_goal_time_error(SharedString::default());
    f.set_goal_time_open(true);
}

pub fn basculer_jour(f: &AppWindow, i: i32) {
    let mut jours: Vec<bool> = (0..7)
        .map(|k| slint::Model::row_data(&f.get_goal_time_days(), k).unwrap_or(false))
        .collect();
    if let Some(j) = jours.get_mut(i as usize) {
        *j = !*j;
    }
    f.set_goal_time_days(ModelRc::new(VecModel::from(jours)));
}

/// Reads "18:00", "18h", "18".
fn heure(texte: &str) -> Option<NaiveTime> {
    let t = texte.trim().to_ascii_lowercase().replace('h', ":");
    let (h, m) = match t.split_once(':') {
        Some((h, m)) => (h.trim(), if m.trim().is_empty() { "0" } else { m.trim() }),
        None => (t.as_str(), "0"),
    };
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

/// Blocks the time: a weekly event on the chosen days, from the next of them until the
/// goal's day, in the first calendar of one's own. Returns what to tell the user.
pub fn bloquer(f: &AppWindow, services: &Services, goal: i64) -> Result<String, String> {
    let g = services
        .store
        .goal(goal)
        .ok()
        .flatten()
        .ok_or("This goal no longer exists.")?;
    let jours: Vec<usize> = (0..7)
        .filter(|k| slint::Model::row_data(&f.get_goal_time_days(), *k).unwrap_or(false))
        .collect();
    if jours.is_empty() {
        return Err("Choose at least one day.".into());
    }
    let debut = heure(&f.get_goal_time_start()).ok_or("The time should look like 18:00.")?;
    let minutes = LONGUEURS[f.get_goal_time_length().clamp(0, 3) as usize];
    let fin_objectif = jour_de(&g.goal.due_day).ok_or("This goal has no date.")?;
    let today = Local::now().date_naive();
    // The first chosen day from tomorrow on.
    let premier = (1..=7)
        .map(|k| today + Duration::days(k))
        .find(|d| jours.contains(&(d.weekday().num_days_from_monday() as usize)))
        .ok_or("No day to start on.")?;
    if premier > fin_objectif {
        return Err("The goal's day comes before the first of these.".into());
    }
    let calendrier = crate::calendar::own_calendar(services)
        .ok_or("There is no calendar of your own to put it in.")?;
    const JOURS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
    let byday = jours
        .iter()
        .map(|k| JOURS[*k])
        .collect::<Vec<_>>()
        .join(",");
    let jusqu = fin_objectif.format("%Y%m%dT235959").to_string();
    let a = iris_calendar::time::zoned_millis(premier.and_time(debut), &Local);
    let evenement = NewEvent {
        uid: format!("{}-goal-{}@iris", now().millis(), goal),
        summary: g.goal.title.clone(),
        description: String::new(),
        location: String::new(),
        start_ms: a,
        end_ms: a + minutes * 60_000,
        all_day: false,
        tzid: iana_time_zone::get_timezone().ok(),
        rrule: Some(format!("FREQ=WEEKLY;BYDAY={byday};UNTIL={jusqu}")),
        exdates: Vec::new(),
        recurrence_id: None,
        cancelled: false,
        reminder_minutes: Some(10),
    };
    services
        .store
        .insert_event(calendrier.id, &evenement, now())
        .map_err(|e| e.to_string())?;
    const NOMS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    let noms = jours
        .iter()
        .map(|k| NOMS[*k])
        .collect::<Vec<_>>()
        .join(", ");
    let fin = debut + Duration::minutes(minutes);
    Ok(format!(
        "Blocked {noms} {}–{} until {} in {}.",
        debut.format("%H:%M"),
        fin.format("%H:%M"),
        fin_objectif.format("%a %-d %b"),
        calendrier.name
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_goal_is_renamed_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let s = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("maitre")),
        )
        .unwrap();
        let g = NewGoal {
            title: "Send 10 applications".into(),
            why: "An internship".into(),
            kind: GoalKind::Count,
            target: 10,
            unit: "applications".into(),
            due_day: "2026-10-30".into(),
            color: "#4f8cff".into(),
        };
        let id = s.store.create_goal(&g, now()).unwrap();
        renommer(&s, id, "  Send 12 applications ").unwrap();
        let apres = s.store.goal(id).unwrap().unwrap().goal;
        assert_eq!(apres.title, "Send 12 applications");
        assert_eq!(
            (apres.target, apres.due_day.as_str()),
            (10, "2026-10-30"),
            "only the name changes"
        );
        assert!(renommer(&s, id, "   ").is_err(), "a goal keeps a name");
    }

    #[test]
    fn the_hour_reads_as_people_write_it() {
        assert_eq!(heure("18:00"), NaiveTime::from_hms_opt(18, 0, 0));
        assert_eq!(heure("7h30"), NaiveTime::from_hms_opt(7, 30, 0));
        assert_eq!(heure("9"), NaiveTime::from_hms_opt(9, 0, 0));
        assert_eq!(heure("25:00"), None);
    }

    #[test]
    fn a_goal_well_behind_or_due_this_week_is_pointed_to() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let loin_et_a_jour = pace(
            5,
            10,
            d("2026-10-01"),
            d("2026-12-01"),
            d("2026-10-31"),
            false,
        );
        assert!(!a_pousser(&loin_et_a_jour));
        let bientot = pace(
            5,
            10,
            d("2026-10-01"),
            d("2026-10-20"),
            d("2026-10-15"),
            false,
        );
        assert!(a_pousser(&bientot));
        let atteint = pace(
            10,
            10,
            d("2026-10-01"),
            d("2026-10-20"),
            d("2026-10-19"),
            false,
        );
        assert!(!a_pousser(&atteint));
    }

    #[test]
    fn days_in_the_log_are_near_or_named() {
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(jour_lisible(d("2026-09-30"), d("2026-09-30")), "Today");
        assert_eq!(jour_lisible(d("2026-09-29"), d("2026-09-30")), "Yesterday");
        assert_eq!(jour_lisible(d("2026-09-26"), d("2026-09-30")), "Sat 26 Sep");
    }
}
