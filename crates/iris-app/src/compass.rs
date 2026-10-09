//! The compass, in Growth: the areas of one's life, scored by hand once a month, the
//! time they got this month against the time wanted, and the cycle of twelve weeks
//! under way with the goals chosen for it (`iris_growth::compass`).
//!
//! An area's time is what was done for it, as the user said: the tasks ticked this
//! month in its lists or for its goals (their length, half an hour when not said), and
//! the time blocked in the calendar for its goals ("Make time for it").

use crate::services::{now, Services};
use chrono::{Datelike, Duration, Local, NaiveDate};
use iris_growth::compass::{grid, month_key, point, polygon, Cycle};
use iris_store::{Area, AreaOf, StoredCycle};
use iris_ui::{AppWindow, AreaData, CycleRowData, PickData};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// What the compass's windows hold. On the display thread.
#[derive(Default)]
struct Etat {
    /// The area the window edits; `None` for a new one.
    domaine: Option<i64>,
    /// What can belong to it: (kind, id, ticked).
    choix: Vec<(AreaOf, i64, bool)>,
    /// The cycle the window edits; `None` for a new one.
    cycle: Option<i64>,
    /// The goals it can hold: (id, ticked).
    buts: Vec<(i64, bool)>,
}

fn aujourdhui() -> NaiveDate {
    Local::now().date_naive()
}

fn lire_jour(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

fn mois_precedent(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d) - Duration::days(1)
}

/// "18 h", "45 min", "1 h 30".
fn heures(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

/// The word an area goes by on the wheel: its first.
fn court(nom: &str) -> String {
    nom.split(|c: char| c.is_whitespace() || c == '&' || c == ',')
        .find(|m| !m.is_empty())
        .unwrap_or(nom)
        .to_string()
}

/// The minutes given to each area this month, up to now.
fn temps_du_mois(services: &Services, today: NaiveDate) -> HashMap<i64, i64> {
    let listes: HashMap<i64, i64> = services
        .store
        .areas_of(AreaOf::TaskList)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let buts: HashMap<i64, i64> = services
        .store
        .areas_of(AreaOf::Goal)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut temps: HashMap<i64, i64> = HashMap::new();
    let debut_jour = today.with_day(1).unwrap_or(today);
    let debut = crate::calendar::local_midnight_ms(debut_jour);
    let fin = now().millis();
    for t in services.store.done_tasks(None, 5000).unwrap_or_default() {
        let Some(fait) = t.done_at else { continue };
        if fait.millis() < debut {
            continue;
        }
        let domaine = t
            .task
            .goal_id
            .and_then(|g| buts.get(&g))
            .or_else(|| listes.get(&t.task.list_id));
        if let Some(a) = domaine {
            *temps.entry(*a).or_default() += t.task.estimate.unwrap_or(30) as i64;
        }
    }
    // The time blocked for goals, their occurrences gone by this month.
    let evenements = services
        .store
        .events_for_range(debut - 86_400_000, fin)
        .unwrap_or_default();
    let blocs: Vec<(usize, i64)> = evenements
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let uid = &e.event.uid;
            let g = uid.strip_suffix("@iris")?.rsplit_once("-goal-")?.1;
            let a = buts.get(&g.parse::<i64>().ok()?)?;
            Some((i, *a))
        })
        .collect();
    if !blocs.is_empty() {
        let domaine: Vec<iris_calendar::Event> = evenements
            .iter()
            .map(|e| crate::calendar::to_domain(&e.event))
            .collect();
        for o in iris_calendar::recur::occurrences(&domaine, debut, fin) {
            if let Some((_, a)) = blocs.iter().find(|(i, _)| *i == o.event) {
                if o.end <= fin && !o.all_day {
                    *temps.entry(*a).or_default() += (o.end - o.start) / 60_000;
                }
            }
        }
    }
    temps
}

fn notes(services: &Services, mois: NaiveDate) -> HashMap<i64, i32> {
    services
        .store
        .area_scores(&month_key(mois))
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// The cycle under way, and its rank among all of them.
fn en_cours(cycles: &[StoredCycle], today: NaiveDate) -> Option<(usize, &StoredCycle, Cycle)> {
    let mut tous: Vec<&StoredCycle> = cycles.iter().collect();
    tous.sort_by(|a, b| a.start_day.cmp(&b.start_day));
    tous.iter().enumerate().find_map(|(i, c)| {
        let cy = Cycle {
            start: lire_jour(&c.start_day)?,
            weeks: c.weeks.max(1) as u32,
        };
        cy.week(today).map(|_| (i + 1, *c, cy))
    })
}

/// What was planned by now and done, for a cycle's goals: done out of where a steady
/// pace would be.
fn planifie(services: &Services, c: &StoredCycle) -> Option<f32> {
    let (mut fait, mut prevu) = (0.0f32, 0.0f32);
    for g in &c.goals {
        if let Some(d) = crate::goals::for_id(services, *g) {
            fait += d.done as f32;
            prevu += d.expected * d.target as f32;
        }
    }
    if prevu < 0.5 {
        None
    } else {
        Some((fait / prevu).min(1.0))
    }
}

fn dates(c: &Cycle) -> String {
    format!(
        "{} – {}",
        c.start.format("%-d %b"),
        c.end().format("%-d %b")
    )
}

/// Fills the compass, and its row in Growth's column.
pub fn remplir(f: &AppWindow, services: &Services) {
    let today = aujourdhui();
    let domaines = services.store.areas().unwrap_or_default();
    let maintenant = notes(services, today);
    let avant = notes(services, mois_precedent(today));
    let temps = temps_du_mois(services, today);
    let buts_par_domaine: Vec<(i64, i64)> =
        services.store.areas_of(AreaOf::Goal).unwrap_or_default();
    let habitudes = services.store.habits().unwrap_or_default();
    let habitudes_par_domaine: Vec<(i64, i64)> =
        services.store.areas_of(AreaOf::Habit).unwrap_or_default();
    let n = domaines.len();

    let lignes: Vec<AreaData> = domaines
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let note = maintenant.get(&a.id).copied().unwrap_or(0);
            let (dx, dy) = point(i, n, note as f32);
            let (lx, ly) = point(i, n, 12.4);
            let buts: Vec<iris_ui::GoalData> = buts_par_domaine
                .iter()
                .filter(|(_, d)| *d == a.id)
                .filter_map(|(g, _)| crate::goals::for_id(services, *g))
                .filter(|g| g.status != 2)
                .collect();
            let siennes: Vec<&str> = habitudes_par_domaine
                .iter()
                .filter(|(_, d)| *d == a.id)
                .filter_map(|(h, _)| habitudes.iter().find(|x| x.id == *h))
                .map(|h| h.habit.title.as_str())
                .collect();
            let en_retard = buts.iter().find(|g| g.status == 1);
            let (ligne, alerte) = match (en_retard, buts.first()) {
                (Some(g), _) => (format!("{} · {}", g.title, g.badge.to_lowercase()), true),
                (None, Some(g)) => {
                    let mut l = format!("{} · {}/{}", g.title, g.done, g.target);
                    if let Some(h) = siennes.first() {
                        l.push_str(&format!(" · {h}"));
                    }
                    (l, false)
                }
                (None, None) if !siennes.is_empty() => (siennes.join(", "), false),
                _ => ("Nothing in it yet".into(), false),
            };
            let fait = temps.get(&a.id).copied().unwrap_or(0);
            let voulu = a.wanted_minutes as i64;
            let echelle = fait.max(voulu).max(1) as f32 / 0.9;
            AreaData {
                id: a.id as i32,
                name: a.name.as_str().into(),
                short: court(&a.name).into(),
                icon: a.icon.as_str().into(),
                color: crate::calendar::couleur(&a.color),
                score: note,
                score_label: if note > 0 {
                    note.to_string().into()
                } else {
                    "–".into()
                },
                last_score: avant.get(&a.id).copied().unwrap_or(0),
                line: ligne.into(),
                warn: alerte,
                hours: if fait > 0 {
                    format!("{} this month", heures(fait)).into()
                } else {
                    "No time this month".into()
                },
                wanted: if voulu > 0 {
                    format!("wanted {}", heures(voulu)).into()
                } else {
                    SharedString::default()
                },
                fraction: if fait > 0 { fait as f32 / echelle } else { 0.0 },
                mark: if voulu > 0 {
                    voulu as f32 / echelle
                } else {
                    -1.0
                },
                dot_x: dx,
                dot_y: dy,
                label_x: lx,
                label_y: ly,
            }
        })
        .collect();

    let valeurs = |m: &HashMap<i64, i32>| -> Vec<f32> {
        domaines
            .iter()
            .map(|a| m.get(&a.id).copied().unwrap_or(0) as f32)
            .collect()
    };
    f.set_compass_wheel_grid(grid(n).into());
    f.set_compass_wheel_now(if maintenant.is_empty() {
        SharedString::default()
    } else {
        polygon(&valeurs(&maintenant)).into()
    });
    f.set_compass_wheel_before(if avant.is_empty() {
        SharedString::default()
    } else {
        polygon(&valeurs(&avant)).into()
    });
    f.set_compass_wheel_scored(if maintenant.is_empty() {
        "Not scored this month".into()
    } else {
        format!("Scored in {}", today.format("%B")).into()
    });
    f.set_compass_areas(ModelRc::new(VecModel::from(lignes)));
    f.set_compass_month(today.format("%B").to_string().into());

    // The cycle under way, the column's row, the list of cycles.
    let cycles = services.store.cycles().unwrap_or_default();
    match en_cours(&cycles, today) {
        Some((rang, c, cy)) => {
            let semaine = cy.week(today).unwrap_or(1);
            f.set_compass_has_cycle(true);
            f.set_compass_cycle_name(format!("Cycle {rang} — {}", c.name).into());
            f.set_compass_cycle_sub(
                format!(
                    "Week {semaine} of {} · {} goal{}",
                    cy.weeks,
                    c.goals.len(),
                    if c.goals.len() == 1 { "" } else { "s" }
                )
                .into(),
            );
            f.set_compass_cycle_segments(ModelRc::new(VecModel::from(cy.segments(today))));
            match planifie(services, c) {
                Some(p) => {
                    f.set_compass_cycle_score(format!("{}%", (p * 100.0).round() as i32).into());
                    f.set_compass_cycle_score_label("of what you planned, done".into());
                }
                None => {
                    f.set_compass_cycle_score(SharedString::default());
                    f.set_compass_cycle_score_label(SharedString::default());
                }
            }
            f.set_compass_sub(format!("Cycle {rang} · {}", dates(&cy)).into());
            f.set_growth_compass_label(format!("Cycle {rang}").into());
            f.set_growth_compass_count(format!("week {semaine}/{}", cy.weeks).into());
        }
        None => {
            f.set_compass_has_cycle(false);
            f.set_compass_sub(SharedString::default());
            f.set_growth_compass_label("Areas of life".into());
            f.set_growth_compass_count(SharedString::default());
        }
    }
    let mut tries: Vec<&StoredCycle> = cycles.iter().collect();
    tries.sort_by(|a, b| a.start_day.cmp(&b.start_day));
    let rangs: HashMap<i64, usize> = tries
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id, i + 1))
        .collect();
    f.set_compass_cycles(ModelRc::new(VecModel::from(
        cycles
            .iter()
            .filter_map(|c| {
                let cy = Cycle {
                    start: lire_jour(&c.start_day)?,
                    weeks: c.weeks.max(1) as u32,
                };
                let etat = if today > cy.end() {
                    2
                } else if today >= cy.start {
                    1
                } else {
                    0
                };
                Some(CycleRowData {
                    id: c.id as i32,
                    name: format!("Cycle {} — {}", rangs.get(&c.id).unwrap_or(&0), c.name).into(),
                    dates: dates(&cy).into(),
                    goals: format!(
                        "{} goal{}",
                        c.goals.len(),
                        if c.goals.len() == 1 { "" } else { "s" }
                    )
                    .into(),
                    state: etat,
                    score: if etat == 0 {
                        "to come".into()
                    } else {
                        planifie(services, c)
                            .map(|p| format!("{}%", (p * 100.0).round() as i32))
                            .unwrap_or_default()
                            .into()
                    },
                })
            })
            .collect::<Vec<_>>(),
    )));
    // Not under the cursor of someone writing it.
    if f.get_compass_tab() != 1 {
        f.set_compass_vision(services.store.vision().unwrap_or_default().into());
    }
}

/// The column's row alone, for when Growth shows something else.
pub fn remplir_colonne(f: &AppWindow, services: &Services) {
    let today = aujourdhui();
    let cycles = services.store.cycles().unwrap_or_default();
    match en_cours(&cycles, today) {
        Some((rang, _, cy)) => {
            f.set_growth_compass_label(format!("Cycle {rang}").into());
            f.set_growth_compass_count(
                format!("week {}/{}", cy.week(today).unwrap_or(1), cy.weeks).into(),
            );
        }
        None => {
            f.set_growth_compass_label("Areas of life".into());
            f.set_growth_compass_count(SharedString::default());
        }
    }
}

fn choix_du_domaine(services: &Services, domaine: Option<i64>) -> Vec<(AreaOf, i64, bool)> {
    let mut v = Vec::new();
    for (genre, ids) in [
        (
            AreaOf::Goal,
            services
                .store
                .goals()
                .unwrap_or_default()
                .into_iter()
                .map(|g| g.id)
                .collect::<Vec<_>>(),
        ),
        (
            AreaOf::Habit,
            services
                .store
                .habits()
                .unwrap_or_default()
                .into_iter()
                .map(|h| h.id)
                .collect(),
        ),
        (
            AreaOf::TaskList,
            services
                .store
                .task_lists()
                .unwrap_or_default()
                .into_iter()
                .map(|l| l.id)
                .collect(),
        ),
    ] {
        for id in ids {
            let a = services.store.area_of(genre, id).ok().flatten();
            v.push((genre, id, domaine.is_some() && a == domaine));
        }
    }
    v
}

fn montrer_choix(f: &AppWindow, services: &Services, choix: &[(AreaOf, i64, bool)]) {
    let buts = services.store.goals().unwrap_or_default();
    let habitudes = services.store.habits().unwrap_or_default();
    let listes = services.store.task_lists().unwrap_or_default();
    let autres: HashMap<(i32, i64), i64> = [AreaOf::Goal, AreaOf::Habit, AreaOf::TaskList]
        .into_iter()
        .enumerate()
        .flat_map(|(k, g)| {
            services
                .store
                .areas_of(g)
                .unwrap_or_default()
                .into_iter()
                .map(move |(id, a)| ((k as i32, id), a))
        })
        .collect();
    let domaines = services.store.areas().unwrap_or_default();
    f.set_area_edit_picks(ModelRc::new(VecModel::from(
        choix
            .iter()
            .map(|(genre, id, on)| {
                let (kind, nom, couleur) = match genre {
                    AreaOf::Goal => {
                        let g = buts.iter().find(|g| g.id == *id);
                        (
                            0,
                            g.map(|g| g.goal.title.clone()),
                            g.map(|g| g.goal.color.clone()),
                        )
                    }
                    AreaOf::Habit => {
                        let h = habitudes.iter().find(|h| h.id == *id);
                        (
                            1,
                            h.map(|h| h.habit.title.clone()),
                            h.map(|h| h.habit.color.clone()),
                        )
                    }
                    AreaOf::TaskList => {
                        let l = listes.iter().find(|l| l.id == *id);
                        (2, l.map(|l| l.name.clone()), l.map(|l| l.color.clone()))
                    }
                };
                // Where it is now, when not here: "in Health".
                let ailleurs = if *on {
                    String::new()
                } else {
                    autres
                        .get(&(kind, *id))
                        .and_then(|a| domaines.iter().find(|d| d.id == *a))
                        .map(|d| format!("in {}", d.name))
                        .unwrap_or_default()
                };
                PickData {
                    id: *id as i32,
                    name: nom.unwrap_or_default().into(),
                    hint: ailleurs.into(),
                    color: crate::calendar::couleur(&couleur.unwrap_or_default()),
                    on: *on,
                    kind,
                }
            })
            .collect::<Vec<_>>(),
    )));
}

fn ouvrir_domaine(f: &AppWindow, services: &Services, etat: &mut Etat, a: Option<&Area>) {
    etat.domaine = a.map(|a| a.id);
    etat.choix = choix_du_domaine(services, etat.domaine);
    montrer_choix(f, services, &etat.choix);
    let n = services.store.areas().map(|v| v.len()).unwrap_or(0);
    f.set_area_edit_new(a.is_none());
    f.set_area_edit_name(a.map(|a| a.name.clone()).unwrap_or_default().into());
    f.set_area_edit_icon(
        a.and_then(|a| crate::growth::ICONES.iter().position(|i| *i == a.icon))
            .unwrap_or(13) as i32,
    );
    f.set_area_edit_color(
        a.and_then(|a| {
            crate::growth::COULEURS
                .iter()
                .position(|c| c.eq_ignore_ascii_case(&a.color))
        })
        .unwrap_or(n % crate::growth::COULEURS.len()) as i32,
    );
    f.set_area_edit_hours(match a.map(|a| a.wanted_minutes) {
        Some(m) if m > 0 => {
            if m % 60 == 0 {
                (m / 60).to_string().into()
            } else {
                format!("{:.1}", m as f32 / 60.0).into()
            }
        }
        _ => SharedString::default(),
    });
    f.set_area_edit_error(SharedString::default());
    f.set_area_edit_open(true);
}

fn ouvrir_cycle(f: &AppWindow, services: &Services, etat: &mut Etat, c: Option<&StoredCycle>) {
    etat.cycle = c.map(|c| c.id);
    let buts = services.store.goals().unwrap_or_default();
    etat.buts = buts
        .iter()
        .map(|g| (g.id, c.is_some_and(|c| c.goals.contains(&g.id))))
        .collect();
    let today = aujourdhui();
    let lundi = iris_growth::habits::monday(today);
    let debut = c
        .map(|c| c.start_day.clone())
        .unwrap_or_else(|| lundi.format("%Y-%m-%d").to_string());
    f.set_cycle_edit_new(c.is_none());
    f.set_cycle_edit_name(c.map(|c| c.name.clone()).unwrap_or_default().into());
    f.set_cycle_edit_start(debut.into());
    f.set_cycle_edit_weeks(c.map(|c| c.weeks).unwrap_or(12).to_string().into());
    montrer_buts(f, services, &etat.buts);
    f.set_cycle_edit_error(SharedString::default());
    f.set_cycle_edit_open(true);
}

fn montrer_buts(f: &AppWindow, services: &Services, buts: &[(i64, bool)]) {
    let tous = services.store.goals().unwrap_or_default();
    f.set_cycle_edit_goals(ModelRc::new(VecModel::from(
        buts.iter()
            .filter_map(|(id, on)| {
                let g = tous.iter().find(|g| g.id == *id)?;
                Some(PickData {
                    id: *id as i32,
                    name: g.goal.title.as_str().into(),
                    hint: format!("by {}", g.goal.due_day).into(),
                    color: crate::calendar::couleur(&g.goal.color),
                    on: *on,
                    kind: 0,
                })
            })
            .collect::<Vec<_>>(),
    )));
}

pub fn wire_compass(f: &AppWindow, services: &Services) {
    let etat = Rc::new(RefCell::new(Etat::default()));
    let redessiner = {
        let (faible, services) = (f.as_weak(), services.clone());
        Rc::new(move || {
            if let Some(f) = faible.upgrade() {
                remplir(&f, &services);
            }
        })
    };
    {
        let (faible, services) = (f.as_weak(), services.clone());
        crate::workspace::follow(f, move |w| {
            if w == crate::growth::WORKSPACE {
                if let Some(f) = faible.upgrade() {
                    remplir_colonne(&f, &services);
                }
            }
        });
    }

    // The names come from the call: those a macro made up would stay invisible.
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
        on_compass_tab_chosen,
        |t| {
            f.set_compass_tab(t.clamp(0, 2));
            redessiner();
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_compass_score_requested,
        || {
            redessiner();
            f.set_compass_score_open(true);
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_area_scored,
        |id, note| {
            let _ = services
                .store
                .set_area_score(id as i64, &month_key(aujourdhui()), note, now());
            redessiner();
        }
    );
    geste!([services, etat, redessiner, f], on_area_opened, |id| {
        let domaines = services.store.areas().unwrap_or_default();
        if let Some(a) = domaines.iter().find(|a| a.id == id as i64) {
            ouvrir_domaine(f, services, &mut etat.borrow_mut(), Some(a));
        }
    });
    geste!([services, etat, redessiner, f], on_area_new, || {
        ouvrir_domaine(f, services, &mut etat.borrow_mut(), None);
    });
    geste!([services, etat, redessiner, f], on_area_pick_toggled, |i| {
        let mut e = etat.borrow_mut();
        if let Some(c) = e.choix.get_mut(i as usize) {
            c.2 = !c.2;
        }
        montrer_choix(f, services, &e.choix);
    });
    geste!(
        [services, etat, redessiner, f],
        on_area_edit_confirmed,
        || {
            let nom = f.get_area_edit_name().trim().to_string();
            if nom.is_empty() {
                f.set_area_edit_error("Give the area a name.".into());
                return;
            }
            let texte = f.get_area_edit_hours().trim().replace(',', ".");
            let minutes = if texte.is_empty() {
                0
            } else {
                match texte.parse::<f32>() {
                    Ok(h) if (0.0..=744.0).contains(&h) => (h * 60.0).round() as i32,
                    _ => {
                        f.set_area_edit_error("Hours a month should be a number, like 8.".into());
                        return;
                    }
                }
            };
            let icone = crate::growth::ICONES
                .get(f.get_area_edit_icon().max(0) as usize)
                .copied()
                .unwrap_or("sparkle");
            let couleur = crate::growth::COULEURS
                .get(f.get_area_edit_color().max(0) as usize)
                .copied()
                .unwrap_or(crate::growth::COULEURS[0]);
            let e = etat.borrow();
            let id = match e.domaine {
                Some(id) => services
                    .store
                    .update_area(&Area {
                        id,
                        name: nom,
                        icon: icone.into(),
                        color: couleur.into(),
                        wanted_minutes: minutes,
                    })
                    .map(|_| id),
                None => services
                    .store
                    .create_area(&nom, icone, couleur)
                    .and_then(|id| {
                        services
                            .store
                            .update_area(&Area {
                                id,
                                name: nom.clone(),
                                icon: icone.into(),
                                color: couleur.into(),
                                wanted_minutes: minutes,
                            })
                            .map(|_| id)
                    }),
            };
            let id = match id {
                Ok(id) => id,
                Err(err) => {
                    f.set_area_edit_error(err.to_string().into());
                    return;
                }
            };
            for (genre, chose, on) in &e.choix {
                let avant = services.store.area_of(*genre, *chose).ok().flatten();
                if *on && avant != Some(id) {
                    let _ = services.store.set_area_of(*genre, *chose, Some(id));
                } else if !*on && avant == Some(id) {
                    let _ = services.store.set_area_of(*genre, *chose, None);
                }
            }
            drop(e);
            f.set_area_edit_open(false);
            redessiner();
        }
    );
    geste!([services, etat, redessiner, f], on_area_delete, || {
        let domaine = etat.borrow().domaine;
        if let Some(id) = domaine {
            let _ = services.store.delete_area(id);
            f.set_status("Area deleted. What it held stays.".into());
        }
        f.set_area_edit_open(false);
        redessiner();
    });
    geste!([services, etat, redessiner, f], on_cycle_opened, |id| {
        let cycles = services.store.cycles().unwrap_or_default();
        let c = if id < 0 {
            en_cours(&cycles, aujourdhui()).map(|(_, c, _)| c.clone())
        } else {
            cycles.iter().find(|c| c.id == id as i64).cloned()
        };
        if let Some(c) = c {
            ouvrir_cycle(f, services, &mut etat.borrow_mut(), Some(&c));
        }
    });
    geste!([services, etat, redessiner, f], on_cycle_new, || {
        ouvrir_cycle(f, services, &mut etat.borrow_mut(), None);
    });
    geste!(
        [services, etat, redessiner, f],
        on_cycle_goal_toggled,
        |i| {
            let mut e = etat.borrow_mut();
            if let Some(b) = e.buts.get_mut(i as usize) {
                b.1 = !b.1;
            }
            montrer_buts(f, services, &e.buts);
        }
    );
    geste!(
        [services, etat, redessiner, f],
        on_cycle_edit_confirmed,
        || {
            let nom = f.get_cycle_edit_name().trim().to_string();
            if nom.is_empty() {
                f.set_cycle_edit_error("Give the cycle a name: what it is for.".into());
                return;
            }
            let Some(debut) = lire_jour(&f.get_cycle_edit_start()) else {
                f.set_cycle_edit_error("The start should look like 2026-10-12.".into());
                return;
            };
            // A cycle runs from a Monday.
            let debut = iris_growth::habits::monday(debut);
            let Some(semaines) = f
                .get_cycle_edit_weeks()
                .trim()
                .parse::<i32>()
                .ok()
                .filter(|n| (1..=52).contains(n))
            else {
                f.set_cycle_edit_error("Weeks should be a number from 1 to 52.".into());
                return;
            };
            let e = etat.borrow();
            let buts: Vec<i64> = e
                .buts
                .iter()
                .filter(|(_, on)| *on)
                .map(|(id, _)| *id)
                .collect();
            match services.store.save_cycle(
                e.cycle,
                &nom,
                &debut.format("%Y-%m-%d").to_string(),
                semaines,
                &buts,
                now(),
            ) {
                Ok(_) => {
                    drop(e);
                    f.set_cycle_edit_open(false);
                    redessiner();
                }
                Err(err) => f.set_cycle_edit_error(err.to_string().into()),
            }
        }
    );
    geste!([services, etat, redessiner, f], on_cycle_delete, || {
        let cycle = etat.borrow().cycle;
        if let Some(id) = cycle {
            let _ = services.store.delete_cycle(id);
        }
        f.set_cycle_edit_open(false);
        redessiner();
    });
    geste!([services, etat, redessiner, f], on_vision_edited, |texte| {
        let _ = services.store.set_vision(&texte);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_and_short_names_read_as_people_say_them() {
        assert_eq!(heures(45), "45 min");
        assert_eq!(heures(18 * 60), "18 h");
        assert_eq!(heures(90), "1 h 30");
        assert_eq!(court("Studies & career"), "Studies");
        assert_eq!(court("Health"), "Health");
    }

    #[test]
    fn the_month_before_is_the_last_day_of_the_one_before() {
        let d = NaiveDate::from_ymd_opt(2026, 3, 15).unwrap();
        assert_eq!(
            mois_precedent(d),
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
        );
    }

    #[test]
    fn a_task_done_for_a_goal_of_an_area_counts_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("maitre")),
        )
        .unwrap();
        let sante = s.store.areas().unwrap()[1].id;
        let liste = s.store.task_lists().unwrap()[0].id;
        s.store
            .set_area_of(AreaOf::TaskList, liste, Some(sante))
            .unwrap();
        let t = s
            .store
            .insert_task(
                &iris_store::NewTask {
                    list_id: liste,
                    title: "Stretch".into(),
                    estimate: Some(20),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        s.store.set_task_done(t, Some(now())).unwrap();
        let u = s
            .store
            .insert_task(
                &iris_store::NewTask {
                    list_id: liste,
                    title: "Walk".into(),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        s.store.set_task_done(u, Some(now())).unwrap();
        let temps = temps_du_mois(&s, aujourdhui());
        assert_eq!(
            temps.get(&sante),
            Some(&50),
            "20 minutes, then half an hour"
        );
    }
}
