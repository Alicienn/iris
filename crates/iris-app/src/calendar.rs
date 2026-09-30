//! L'agenda dans l'application : la vue, l'éditeur, les abonnements, les rappels.
//!
//! Le domaine — lire un fichier iCalendar, dérouler une règle, disposer une semaine —
//! est dans `iris-calendar` et s'y teste. Ce module fait le lien : il lit la base,
//! convertit pour l'interface, et va chercher les abonnements sur le réseau.

use crate::services::{now, Services};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use iris_calendar::{layout, Event, Occurrence};
use iris_store::{NewEvent, StoredCalendar, StoredEvent};
use iris_types::{Error, Result, Timestamp};
use iris_ui::{
    AppWindow, CalendarChipData, CalendarData, EventDetailData, HomeItemData, MonthCellData,
    TimedEventData, WeekDayData,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Combien d'événements une case du mois montre avant « +N ».
const PAR_CASE: usize = 4;

/// La fréquence à laquelle un abonnement est relu.
const RELECTURE: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Au-delà, un fichier d'agenda n'en est plus un.
const MAX_ICS: usize = 20 * 1024 * 1024;

/// Les couleurs proposées aux nouveaux calendriers, à tour de rôle.
const COULEURS: [&str; 8] = [
    "#5b8def", "#e0795b", "#4fb286", "#b67be6", "#e3b341", "#e0608c", "#3fb1c9", "#8a9a5b",
];

/// Les rappels proposés, et leur valeur en minutes.
const RAPPELS: [(&str, Option<i32>); 7] = [
    ("None", None),
    ("At start", Some(0)),
    ("5 minutes before", Some(5)),
    ("15 minutes before", Some(15)),
    ("30 minutes before", Some(30)),
    ("1 hour before", Some(60)),
    ("1 day before", Some(1440)),
];

/// Les répétitions proposées, et leur règle.
const REPETITIONS: [(&str, Option<&str>); 6] = [
    ("Does not repeat", None),
    ("Every day", Some("FREQ=DAILY")),
    ("Every weekday", Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR")),
    ("Every week", Some("FREQ=WEEKLY")),
    ("Every month", Some("FREQ=MONTHLY")),
    ("Every year", Some("FREQ=YEARLY")),
];

/// Ce que l'agenda montre en ce moment. Sur le fil de l'interface.
struct Etat {
    /// 0 mois, 1 semaine, 2 jour.
    mode: i32,
    /// Le jour autour duquel la période est construite.
    jour: NaiveDate,
    /// Le mois du petit calendrier de gauche (son premier jour).
    mini: NaiveDate,
    /// Les événements chargés pour la période, et leurs occurrences.
    evenements: Vec<(StoredEvent, Event)>,
    /// L'événement en cours d'édition ; `None` pour un nouvel événement.
    edite: Option<i64>,
    /// Les calendriers locaux proposés dans l'éditeur, dans l'ordre de la liste.
    locaux: Vec<i64>,
    /// Les rappels déjà donnés, pour ne pas les répéter.
    rappeles: HashSet<String>,
    /// The small month open under a date of the editor: which date (0 start, 1 end)
    /// and the month it shows.
    selecteur: Option<(i32, NaiveDate)>,
    /// Where the notes of the open event go: calendar, identifier, occurrence (0 for
    /// an event that does not repeat, so that moving it keeps them).
    note: Option<(i64, String, i64)>,
    /// The open event, for the tasks added from it.
    ouvert: Option<Ouvert>,
}

/// The event open in the panel, as its tasks need it.
#[derive(Debug, Clone)]
struct Ouvert {
    uid: String,
    /// The occurrence, as the notes key it (0 when the event does not repeat).
    occurrence: i64,
    titre: String,
    /// When this occurrence starts, in milliseconds.
    debut: i64,
    all_day: bool,
}

/// The tasks of the open event, in its panel.
fn remplir_taches(f: &AppWindow, services: &Services, o: &Ouvert) {
    let taches = services
        .store
        .tasks_for_event(&o.uid, o.occurrence)
        .unwrap_or_default();
    f.set_event_tasks(ModelRc::new(VecModel::from(
        taches
            .iter()
            .map(|t| iris_ui::SubtaskData {
                id: t.id as i32,
                title: t.task.title.as_str().into(),
                done: t.is_done(),
            })
            .collect::<Vec<_>>(),
    )));
}

/// A task for an event: in the first list, due when the occurrence starts (the day
/// alone for an all-day event), named after it. It keeps the event's identifier, not
/// its calendar: hiding or deleting the calendar leaves it where it is.
fn tache_d_evenement(o: &Ouvert, liste: i64, titre: &str) -> iris_store::NewTask {
    // An all-day event sits at midnight UTC: its day is read there, not shifted by
    // the local zone into the day before.
    let debut = if o.all_day {
        chrono::DateTime::from_timestamp_millis(o.debut)
            .map(|d| d.naive_utc())
            .unwrap_or_default()
    } else {
        Local
            .timestamp_millis_opt(o.debut)
            .earliest()
            .map(|d| d.naive_local())
            .unwrap_or_default()
    };
    iris_store::NewTask {
        list_id: liste,
        title: titre.trim().to_string(),
        due_day: Some(debut.date().format("%Y-%m-%d").to_string()),
        due_minute: (!o.all_day).then(|| (debut.time() - NaiveTime::MIN).num_minutes() as i32),
        source: format!("Event: {}", o.titre),
        event_uid: Some(o.uid.clone()),
        event_start: Some(o.occurrence),
        ..Default::default()
    }
}

fn aujourd_hui() -> NaiveDate {
    Local::now().date_naive()
}

fn premier_du_mois(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

fn ajouter_mois(d: NaiveDate, n: i32) -> NaiveDate {
    let total = d.year() * 12 + d.month0() as i32 + n;
    let (annee, mois) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    let jour = d.day().min(jours_du_mois(annee, mois));
    NaiveDate::from_ymd_opt(annee, mois, jour).unwrap_or(d)
}

fn jours_du_mois(annee: i32, mois: u32) -> u32 {
    let suivant = if mois == 12 {
        NaiveDate::from_ymd_opt(annee + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(annee, mois + 1, 1)
    };
    suivant
        .and_then(|s| s.pred_opt())
        .map(|d| d.day())
        .unwrap_or(28)
}

/// `#rrggbb` en couleur, gris si illisible.
pub fn couleur(hex: &str) -> slint::Color {
    let h = hex.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0x888888);
    if h.len() == 6 {
        slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
    } else {
        slint::Color::from_rgb_u8(0x88, 0x88, 0x88)
    }
}

fn vers_domaine(e: &NewEvent) -> Event {
    Event {
        uid: e.uid.clone(),
        summary: e.summary.clone(),
        description: e.description.clone(),
        location: e.location.clone(),
        start: e.start_ms,
        end: e.end_ms,
        all_day: e.all_day,
        tzid: e.tzid.clone(),
        rrule: e.rrule.clone(),
        exdates: e.exdates.clone(),
        recurrence_id: e.recurrence_id,
        cancelled: e.cancelled,
        reminder_minutes: e.reminder_minutes,
    }
}

fn depuis_domaine(e: &Event) -> NewEvent {
    NewEvent {
        uid: e.uid.clone(),
        summary: e.summary.clone(),
        description: e.description.clone(),
        location: e.location.clone(),
        start_ms: e.start,
        end_ms: e.end,
        all_day: e.all_day,
        tzid: e.tzid.clone(),
        rrule: e.rrule.clone(),
        exdates: e.exdates.clone(),
        recurrence_id: e.recurrence_id,
        cancelled: e.cancelled,
        reminder_minutes: e.reminder_minutes,
    }
}

// --- Mise en forme -----------------------------------------------------------------

fn heure(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

fn titre(e: &Event) -> String {
    if e.summary.trim().is_empty() {
        "(No title)".into()
    } else {
        e.summary.clone()
    }
}

fn periode(etat: &Etat) -> (NaiveDate, usize) {
    match etat.mode {
        0 => (
            layout::month_days(etat.jour.year(), etat.jour.month())[0],
            42,
        ),
        1 => (layout::week_start(etat.jour), 7),
        _ => (etat.jour, 1),
    }
}

fn titre_periode(etat: &Etat) -> String {
    match etat.mode {
        0 => etat.jour.format("%B %Y").to_string(),
        1 => {
            let debut = layout::week_start(etat.jour);
            let fin = debut + Duration::days(6);
            if debut.month() == fin.month() {
                format!("{} – {}", debut.format("%B %-d"), fin.format("%-d, %Y"))
            } else if debut.year() == fin.year() {
                format!("{} – {}", debut.format("%b %-d"), fin.format("%b %-d, %Y"))
            } else {
                format!(
                    "{} – {}",
                    debut.format("%b %-d, %Y"),
                    fin.format("%b %-d, %Y")
                )
            }
        }
        _ => etat.jour.format("%A, %B %-d, %Y").to_string(),
    }
}

/// Quand un événement a lieu, en toutes lettres.
fn quand(o: &Occurrence) -> String {
    if o.all_day {
        let debut = layout::local_date(o.start, &chrono::Utc);
        let fin = layout::local_date((o.end - 1).max(o.start), &chrono::Utc);
        if debut == fin {
            debut.format("%A, %B %-d, %Y").to_string()
        } else {
            format!(
                "{} – {}",
                debut.format("%A, %B %-d"),
                fin.format("%A, %B %-d, %Y")
            )
        }
    } else {
        let debut = Local.timestamp_millis_opt(o.start).single();
        let fin = Local.timestamp_millis_opt(o.end).single();
        match (debut, fin) {
            (Some(d), Some(f)) if d.date_naive() == f.date_naive() => format!(
                "{} · {} – {}",
                d.format("%A, %B %-d, %Y"),
                d.format("%H:%M"),
                f.format("%H:%M")
            ),
            (Some(d), Some(f)) => format!(
                "{} {} – {} {}",
                d.format("%a %b %-d"),
                d.format("%H:%M"),
                f.format("%a %b %-d, %Y"),
                f.format("%H:%M")
            ),
            _ => String::new(),
        }
    }
}

fn repetition(regle: Option<&str>) -> String {
    let Some(r) = regle else {
        return String::new();
    };
    if let Some((nom, _)) = REPETITIONS
        .iter()
        .find(|(_, x)| x.is_some_and(|x| x.eq_ignore_ascii_case(r)))
    {
        return format!("Repeats {}", nom.to_lowercase());
    }
    let frequence = r
        .split(';')
        .find_map(|p| p.strip_prefix("FREQ="))
        .unwrap_or("")
        .to_ascii_lowercase();
    match frequence.as_str() {
        "daily" => "Repeats daily".into(),
        "weekly" => "Repeats weekly".into(),
        "monthly" => "Repeats monthly".into(),
        "yearly" => "Repeats yearly".into(),
        _ => "Repeats".into(),
    }
}

fn rappel(minutes: Option<i32>) -> String {
    match minutes {
        None => String::new(),
        Some(0) => "At start".into(),
        Some(m) if m % 1440 == 0 => format!("{} day(s) before", m / 1440),
        Some(m) if m % 60 == 0 => format!("{} hour(s) before", m / 60),
        Some(m) => format!("{m} minutes before"),
    }
}

fn statut(c: &StoredCalendar, maintenant: Timestamp) -> (String, bool) {
    if !c.is_subscription() {
        return (String::new(), false);
    }
    if let Some(e) = &c.last_error {
        return (format!("Could not update: {e}"), true);
    }
    match c.last_sync {
        None => ("Not read yet".into(), false),
        Some(t) => (
            format!("Updated {}", crate::vitals::ago(t, maintenant)),
            false,
        ),
    }
}

// --- La vue ------------------------------------------------------------------------

fn charger(services: &Services, etat: &mut Etat) -> Vec<Occurrence> {
    let (debut, jours) = periode(etat);
    let de = layout::local_midnight(debut, &Local);
    let a = layout::local_midnight(debut + Duration::days(jours as i64), &Local);
    // Un jour de marge de chaque côté : un jour entier est posé à minuit UTC, et un
    // fuseau éloigné le ferait tomber juste hors de la période.
    let stockes = services
        .store
        .events_for_range(de - 86_400_000, a + 86_400_000)
        .unwrap_or_default();
    etat.evenements = stockes
        .into_iter()
        .map(|s| {
            let e = vers_domaine(&s.event);
            (s, e)
        })
        .collect();
    let domaine: Vec<Event> = etat.evenements.iter().map(|(_, e)| e.clone()).collect();
    iris_calendar::recur::occurrences(&domaine, de, a)
}

fn cle(etat: &Etat, o: &Occurrence) -> String {
    format!("{}:{}", etat.evenements[o.event].0.id, o.start)
}

fn puce(etat: &Etat, o: &Occurrence, couleurs: &HashMap<i64, String>) -> CalendarChipData {
    let (stocke, e) = &etat.evenements[o.event];
    CalendarChipData {
        key: cle(etat, o).into(),
        title: titre(e).into(),
        time: if o.all_day {
            SharedString::default()
        } else {
            heure(o.start).into()
        },
        color: couleur(
            couleurs
                .get(&stocke.calendar_id)
                .map(String::as_str)
                .unwrap_or(""),
        ),
        all_day: o.all_day,
    }
}

fn cellules(
    etat: &Etat,
    choisi: NaiveDate,
    mois: NaiveDate,
    occ: &[Occurrence],
    couleurs: &HashMap<i64, String>,
    avec_evenements: bool,
) -> Vec<MonthCellData> {
    let grille = layout::month_grid(mois.year(), mois.month(), occ, &Local);
    let today = aujourd_hui();
    grille
        .into_iter()
        .map(|c| {
            let puces: Vec<CalendarChipData> = if avec_evenements {
                c.items
                    .iter()
                    .take(PAR_CASE)
                    .map(|i| puce(etat, &occ[*i], couleurs))
                    .collect()
            } else {
                // Le petit mois ne dit que « il y a quelque chose » : une puce suffit.
                c.items
                    .iter()
                    .take(1)
                    .map(|i| puce(etat, &occ[*i], couleurs))
                    .collect()
            };
            MonthCellData {
                // In the big grid, the first of a month names it: where one month
                // ends and the next begins is read, not guessed.
                day: if avec_evenements && c.date.day() == 1 {
                    c.date.format("%-d %b").to_string().into()
                } else {
                    c.date.day().to_string().into()
                },
                date: c.date.format("%Y-%m-%d").to_string().into(),
                in_month: c.in_month,
                today: c.date == today,
                selected: c.date == choisi,
                in_week: false,
                more: c.items.len().saturating_sub(PAR_CASE) as i32 * avec_evenements as i32,
                events: ModelRc::new(VecModel::from(puces)),
            }
        })
        .collect()
}

/// The small month used to pick a date elsewhere (the tasks): `mois` shown, `choisi`
/// marked, no events.
pub(crate) fn mini_cells(choisi: NaiveDate, mois: NaiveDate) -> Vec<MonthCellData> {
    let vide = Etat {
        mode: 0,
        jour: choisi,
        mini: mois,
        evenements: Vec::new(),
        edite: None,
        locaux: Vec::new(),
        rappeles: HashSet::new(),
        selecteur: None,
        note: None,
        ouvert: None,
    };
    cellules(
        &vide,
        choisi,
        premier_du_mois(mois),
        &[],
        &HashMap::new(),
        false,
    )
}

/// Recalcule tout ce que l'agenda affiche.
fn rafraichir(fenetre: &AppWindow, services: &Services, etat: &mut Etat) {
    let maintenant = now();
    let calendriers = services.store.calendars().unwrap_or_default();
    let couleurs: HashMap<i64, String> = calendriers
        .iter()
        .map(|c| (c.id, c.color.clone()))
        .collect();

    let nb_locaux = calendriers.iter().filter(|c| !c.is_subscription()).count();
    fenetre.set_calendars(ModelRc::new(VecModel::from(
        calendriers
            .iter()
            .map(|c| {
                let (texte, echec) = statut(c, maintenant);
                CalendarData {
                    id: c.id as i32,
                    name: c.name.as_str().into(),
                    color: couleur(&c.color),
                    visible: c.visible,
                    subscribed: c.is_subscription(),
                    status: texte.into(),
                    failed: echec,
                    // The last calendar of one's own stays: a new event needs one.
                    deletable: c.is_subscription() || nb_locaux > 1,
                }
            })
            .collect::<Vec<_>>(),
    )));
    etat.locaux = calendriers
        .iter()
        .filter(|c| !c.is_subscription())
        .map(|c| c.id)
        .collect();

    let occ = charger(services, etat);
    fenetre.set_calendar_title(titre_periode(etat).into());
    fenetre.set_calendar_mode(etat.mode);

    match etat.mode {
        0 => {
            let cases = cellules(etat, etat.jour, etat.jour, &occ, &couleurs, true);
            fenetre.set_calendar_month_cells(ModelRc::new(VecModel::from(cases)));
        }
        _ => {
            let (debut, n) = periode(etat);
            let jours: Vec<NaiveDate> = (0..n as i64).map(|i| debut + Duration::days(i)).collect();
            let (entiers, blocs) = layout::week_layout(&jours, &occ, &Local);
            let today = aujourd_hui();
            fenetre.set_calendar_week_days(ModelRc::new(VecModel::from(
                jours
                    .iter()
                    .enumerate()
                    .map(|(i, d)| WeekDayData {
                        name: d.format("%a").to_string().into(),
                        day: d.day().to_string().into(),
                        date: d.format("%Y-%m-%d").to_string().into(),
                        today: *d == today,
                        all_day: ModelRc::new(VecModel::from(
                            entiers[i]
                                .iter()
                                .map(|k| puce(etat, &occ[*k], &couleurs))
                                .collect::<Vec<_>>(),
                        )),
                    })
                    .collect::<Vec<_>>(),
            )));
            fenetre.set_calendar_week_events(ModelRc::new(VecModel::from(
                blocs
                    .iter()
                    .map(|b| {
                        let o = &occ[b.occurrence];
                        let p = puce(etat, o, &couleurs);
                        let (stocke, domaine) = &etat.evenements[o.event];
                        TimedEventData {
                            movable: etat.locaux.contains(&stocke.calendar_id)
                                && domaine.rrule.is_none()
                                && domaine.recurrence_id.is_none(),
                            key: p.key,
                            title: p.title,
                            time: format!("{} – {}", heure(o.start), heure(o.end)).into(),
                            color: p.color,
                            day: b.day as i32,
                            lane: b.lane as i32,
                            lanes: b.lanes as i32,
                            top: b.start_min as f32 / 1440.0,
                            height: (b.end_min - b.start_min) as f32 / 1440.0,
                            past: o.end <= maintenant.millis(),
                        }
                    })
                    .collect::<Vec<_>>(),
            )));
            match jours.iter().position(|d| *d == today) {
                Some(i) => {
                    fenetre.set_calendar_now_day(i as i32);
                    fenetre.set_calendar_now_fraction(
                        layout::local_minutes(maintenant.millis(), &Local) as f32 / 1440.0,
                    );
                }
                None => {
                    fenetre.set_calendar_now_day(-1);
                    fenetre.set_calendar_now_fraction(-1.0);
                }
            }
        }
    }

    // What is left of today, at the foot of the side column, and the tasks to give a
    // time to.
    fenetre.set_calendar_today_left(ModelRc::new(VecModel::from(reste_du_jour(
        services, maintenant,
    ))));
    fenetre.set_calendar_to_plan(ModelRc::new(VecModel::from(crate::tasks::to_plan(
        services,
    ))));

    // Le petit mois, avec ses propres occurrences quand il ne montre pas le même mois.
    // "September"; the year only when it is not this one.
    fenetre.set_calendar_mini_title(
        etat.mini
            .format(if etat.mini.year() == aujourd_hui().year() {
                "%B"
            } else {
                "%B %Y"
            })
            .to_string()
            .into(),
    );
    // The week (or the day) shown, tinted in the small month.
    let (debut_vu, n_vu) = periode(etat);
    let dans_la_vue = |date: &str| {
        etat.mode != 0
            && NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .is_ok_and(|d| d >= debut_vu && d < debut_vu + Duration::days(n_vu as i64))
    };
    let marquer = |mut cases: Vec<MonthCellData>| {
        for c in &mut cases {
            c.in_week = dans_la_vue(&c.date);
        }
        cases
    };
    let occ_mini = if etat.mode == 0 && premier_du_mois(etat.jour) == etat.mini {
        occ
    } else {
        let mut copie = Etat {
            mode: 0,
            jour: etat.mini,
            mini: etat.mini,
            evenements: Vec::new(),
            edite: None,
            locaux: Vec::new(),
            rappeles: HashSet::new(),
            selecteur: None,
            note: None,
            ouvert: None,
        };
        let o = charger(services, &mut copie);
        let cases = marquer(cellules(&copie, etat.jour, etat.mini, &o, &couleurs, false));
        fenetre.set_calendar_mini_cells(ModelRc::new(VecModel::from(cases)));
        return;
    };
    let cases = marquer(cellules(
        etat, etat.jour, etat.mini, &occ_mini, &couleurs, false,
    ));
    fenetre.set_calendar_mini_cells(ModelRc::new(VecModel::from(cases)));
}

/// The events of today still to come or under way, four at most: "14:30, in 25 min".
fn reste_du_jour(services: &Services, maintenant: Timestamp) -> Vec<HomeItemData> {
    let instant = maintenant.millis();
    upcoming(services, aujourd_hui(), 1)
        .into_iter()
        .filter(|u| !u.all_day && u.end > instant)
        .take(4)
        .map(|u| HomeItemData {
            key: u.key.into(),
            title: u.title.into(),
            meta: if u.start <= instant {
                format!("{}, now", u.time)
            } else {
                format!(
                    "{}, {}",
                    u.time,
                    crate::home::in_how_long(u.start - instant)
                )
            }
            .into(),
            color: couleur(&u.color),
            ..Default::default()
        })
        .collect()
}

/// An event of one's own dragged in the grid: `jours` days and `minutes` later, its
/// length kept; or, with `etirer`, only its end `minutes` later. The shift is made on
/// the local clock, so an event moved across a change of summer time keeps its hour.
/// A repeating event, or one in a subscribed calendar, is left alone: the grid does
/// not offer to drag them.
fn deplacer(services: &Services, cle: &str, jours: i64, minutes: i64, etirer: bool) -> Result<()> {
    let (id, _) = ms_depuis_cle(cle).ok_or_else(|| Error::Config("unknown event".into()))?;
    let stocke = services
        .store
        .event(id)?
        .ok_or_else(|| Error::Config("that event no longer exists".into()))?;
    let calendrier = services
        .store
        .calendar(stocke.calendar_id)?
        .ok_or_else(|| Error::Config("its calendar no longer exists".into()))?;
    let e = &stocke.event;
    if calendrier.is_subscription() || e.rrule.is_some() || e.recurrence_id.is_some() {
        return Ok(());
    }
    let local = |ms: i64| {
        Local
            .timestamp_millis_opt(ms)
            .earliest()
            .map(|d| d.naive_local())
            .unwrap_or_default()
    };
    let (debut, fin) = (local(e.start_ms), local(e.end_ms));
    let (debut, fin) = if etirer {
        let fin = fin + Duration::minutes(minutes);
        (debut, fin.max(debut + Duration::minutes(15)))
    } else {
        let decalage = Duration::days(jours) + Duration::minutes(minutes);
        (debut + decalage, fin + decalage)
    };
    let mut nouveau = e.clone();
    nouveau.start_ms = iris_calendar::time::zoned_millis(debut, &Local);
    nouveau.end_ms = iris_calendar::time::zoned_millis(fin, &Local);
    services
        .store
        .update_event(id, stocke.calendar_id, &nouveau, now())?;
    // A task's slot carries the task's hour with it.
    crate::tasks::slot_moved(services, &nouveau.uid, nouveau.start_ms);
    Ok(())
}

/// An event made in one line from the grid: a title, an hour on a day, an hour long,
/// in the first calendar of one's own.
fn evenement_rapide(
    services: &Services,
    calendrier: i64,
    jour: NaiveDate,
    minute: i32,
    titre: &str,
) -> Result<()> {
    let debut = jour.and_time(NaiveTime::MIN) + Duration::minutes(minute.clamp(0, 1439) as i64);
    let a = iris_calendar::time::zoned_millis(debut, &Local);
    let evenement = NewEvent {
        uid: format!("{}-{}@iris", now().millis(), std::process::id()),
        summary: titre.trim().to_string(),
        description: String::new(),
        location: String::new(),
        start_ms: a,
        end_ms: a + 3_600_000,
        all_day: false,
        tzid: fuseau_local(),
        rrule: None,
        exdates: Vec::new(),
        recurrence_id: None,
        cancelled: false,
        reminder_minutes: None,
    };
    services
        .store
        .insert_event(calendrier, &evenement, now())
        .map(|_| ())
}

// --- L'éditeur ---------------------------------------------------------------------

/// Lit une date : `2026-09-28`, `28/09/2026` ou `28.09.2026`.
fn lire_date(texte: &str) -> Option<NaiveDate> {
    let t = texte.trim();
    NaiveDate::parse_from_str(t, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(t, "%d/%m/%Y"))
        .or_else(|_| NaiveDate::parse_from_str(t, "%d.%m.%Y"))
        .ok()
}

/// Lit une heure : `9:30`, `09:30`, `9h30`, `9h`, `9`.
fn lire_heure(texte: &str) -> Option<NaiveTime> {
    let t = texte.trim().to_ascii_lowercase().replace('h', ":");
    let (h, m) = match t.split_once(':') {
        Some((h, m)) => (h.trim(), if m.trim().is_empty() { "0" } else { m.trim() }),
        None => (t.as_str(), "0"),
    };
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

/// Le fuseau de la machine, par son nom : c'est lui qui garde une réunion récurrente à
/// la même heure de part et d'autre d'un changement d'heure.
fn fuseau_local() -> Option<String> {
    iana_time_zone::get_timezone().ok()
}

/// A date as the editor shows it: "Mon 28 Sep 2026".
fn date_lisible(d: NaiveDate) -> String {
    d.format("%a %-d %b %Y").to_string()
}

/// Sets both dates of the editor, as data and as words.
fn poser_dates(f: &AppWindow, debut: NaiveDate, fin: NaiveDate) {
    f.set_editor_start_date(debut.format("%Y-%m-%d").to_string().into());
    f.set_editor_end_date(fin.format("%Y-%m-%d").to_string().into());
    f.set_editor_start_label(date_lisible(debut).into());
    f.set_editor_end_label(date_lisible(fin).into());
}

/// Fills the small month under a date of the editor.
fn remplir_selecteur(f: &AppWindow, etat: &Etat) {
    let Some((lequel, mois)) = etat.selecteur else {
        f.set_editor_picker(-1);
        return;
    };
    let champ = if lequel == 0 {
        f.get_editor_start_date()
    } else {
        f.get_editor_end_date()
    };
    let choisi = lire_date(&champ).unwrap_or_else(aujourd_hui);
    f.set_editor_picker_title(mois.format("%B %Y").to_string().into());
    f.set_editor_picker_cells(ModelRc::new(VecModel::from(cellules(
        etat,
        choisi,
        mois,
        &[],
        &HashMap::new(),
        false,
    ))));
    f.set_editor_picker(lequel);
}

/// A day picked in the small month. Moving the start moves the end with it, so the
/// event keeps its length; an end picked before the start pulls the start back.
fn dates_apres_choix(
    lequel: i32,
    jour: NaiveDate,
    debut: NaiveDate,
    fin: NaiveDate,
) -> (NaiveDate, NaiveDate) {
    if lequel == 0 {
        (jour, fin + (jour - debut))
    } else {
        (debut.min(jour), jour)
    }
}

/// Ce que l'éditeur a saisi, en événement — ou ce qui ne va pas.
fn lire_editeur(f: &AppWindow) -> std::result::Result<NewEvent, String> {
    let debut_jour = lire_date(&f.get_editor_start_date())
        .ok_or("The start date should look like 2026-09-28.")?;
    let fin_jour = if f.get_editor_end_date().trim().is_empty() {
        debut_jour
    } else {
        lire_date(&f.get_editor_end_date()).ok_or("The end date should look like 2026-09-28.")?
    };
    let tout_le_jour = f.get_editor_all_day();

    let (debut, fin) = if tout_le_jour {
        if fin_jour < debut_jour {
            return Err("The event ends before it starts.".into());
        }
        let minuit = |d: NaiveDate| layout::local_midnight(d, &chrono::Utc);
        // Une fin de jour entier est exclusive : le lendemain du dernier jour.
        (minuit(debut_jour), minuit(fin_jour + Duration::days(1)))
    } else {
        let h1 = lire_heure(&f.get_editor_start_time())
            .ok_or("The start time should look like 09:30.")?;
        let h2 = if f.get_editor_end_time().trim().is_empty() {
            h1 + Duration::hours(1)
        } else {
            lire_heure(&f.get_editor_end_time()).ok_or("The end time should look like 10:30.")?
        };
        let a = iris_calendar::time::zoned_millis(debut_jour.and_time(h1), &Local);
        let b = iris_calendar::time::zoned_millis(fin_jour.and_time(h2), &Local);
        if b < a {
            return Err("The event ends before it starts.".into());
        }
        (a, b)
    };

    let repetition = REPETITIONS
        .get(f.get_editor_repeat_index().max(0) as usize)
        .and_then(|(_, r)| *r)
        .map(str::to_string);

    Ok(NewEvent {
        uid: String::new(),
        summary: f.get_editor_title().trim().to_string(),
        description: f.get_editor_notes().trim().to_string(),
        location: f.get_editor_location().trim().to_string(),
        start_ms: debut,
        end_ms: fin,
        all_day: tout_le_jour,
        tzid: if tout_le_jour { None } else { fuseau_local() },
        rrule: repetition,
        exdates: Vec::new(),
        recurrence_id: None,
        cancelled: false,
        reminder_minutes: RAPPELS
            .get(f.get_editor_reminder_index().max(0) as usize)
            .and_then(|(_, m)| *m),
    })
}

fn ouvrir_editeur(
    f: &AppWindow,
    etat: &mut Etat,
    services: &Services,
    existant: Option<&StoredEvent>,
    jour: NaiveDate,
    minute: i32,
) {
    let calendriers = services.store.calendars().unwrap_or_default();
    let locaux: Vec<&StoredCalendar> = calendriers
        .iter()
        .filter(|c| !c.is_subscription())
        .collect();
    etat.locaux = locaux.iter().map(|c| c.id).collect();
    f.set_editor_calendars(ModelRc::new(VecModel::from(
        locaux
            .iter()
            .map(|c| SharedString::from(c.name.as_str()))
            .collect::<Vec<_>>(),
    )));
    f.set_editor_error(SharedString::default());

    match existant {
        Some(s) => {
            let e = &s.event;
            etat.edite = Some(s.id);
            f.set_editor_is_new(false);
            f.set_editor_title(e.summary.as_str().into());
            f.set_editor_all_day(e.all_day);
            let (d1, d2, h1, h2) = if e.all_day {
                let a = layout::local_date(e.start_ms, &chrono::Utc);
                let b = layout::local_date((e.end_ms - 1).max(e.start_ms), &chrono::Utc);
                (a, b, String::new(), String::new())
            } else {
                let a = Local
                    .timestamp_millis_opt(e.start_ms)
                    .single()
                    .unwrap_or_default();
                let b = Local
                    .timestamp_millis_opt(e.end_ms)
                    .single()
                    .unwrap_or_default();
                (
                    a.date_naive(),
                    b.date_naive(),
                    heure(e.start_ms),
                    heure(e.end_ms),
                )
            };
            poser_dates(f, d1, d2);
            f.set_editor_start_time(h1.into());
            f.set_editor_end_time(h2.into());
            f.set_editor_location(e.location.as_str().into());
            f.set_editor_notes(e.description.as_str().into());
            f.set_editor_calendar_index(
                etat.locaux
                    .iter()
                    .position(|id| *id == s.calendar_id)
                    .unwrap_or(0) as i32,
            );
            f.set_editor_reminder_index(
                RAPPELS
                    .iter()
                    .position(|(_, m)| *m == e.reminder_minutes)
                    .unwrap_or(0) as i32,
            );
            f.set_editor_repeat_index(
                REPETITIONS
                    .iter()
                    .position(|(_, r)| r.map(str::to_string) == e.rrule)
                    .unwrap_or(0) as i32,
            );
        }
        None => {
            etat.edite = None;
            f.set_editor_is_new(true);
            f.set_editor_title(SharedString::default());
            f.set_editor_all_day(false);
            // L'heure proposée : celle cliquée, sinon la prochaine heure pleine
            // aujourd'hui, sinon neuf heures.
            let minute = if minute >= 0 {
                minute
            } else if jour == aujourd_hui() {
                ((layout::local_minutes(now().millis(), &Local) / 60 + 1) * 60).min(23 * 60)
            } else {
                9 * 60
            };
            let debut = NaiveTime::from_hms_opt((minute / 60) as u32, (minute % 60) as u32, 0)
                .unwrap_or_default();
            let fin = debut + Duration::hours(1);
            poser_dates(f, jour, jour);
            f.set_editor_start_time(debut.format("%H:%M").to_string().into());
            f.set_editor_end_time(fin.format("%H:%M").to_string().into());
            f.set_editor_location(SharedString::default());
            f.set_editor_notes(SharedString::default());
            f.set_editor_calendar_index(0);
            f.set_editor_reminder_index(3);
            f.set_editor_repeat_index(0);
        }
    }
    etat.selecteur = None;
    f.set_editor_picker(-1);
    f.set_event_detail_open(false);
    f.set_event_editor_open(true);
}

// --- Les abonnements -----------------------------------------------------------------

/// Ce qu'une relecture a rapporté.
enum Lecture {
    /// Rien n'a changé depuis la dernière fois (le serveur l'a dit).
    Inchange,
    Nouveau {
        texte: String,
        etag: Option<String>,
        modifie: Option<String>,
    },
}

async fn lire_abonnement(url: &str, etag: Option<&str>, modifie: Option<&str>) -> Result<Lecture> {
    let client = reqwest::Client::builder()
        .user_agent(format!("Iris/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| Error::other(format!("client : {e}")))?;
    let mut requete = client.get(url).header("Accept", "text/calendar, */*;q=0.5");
    if let Some(e) = etag {
        requete = requete.header("If-None-Match", e);
    }
    if let Some(m) = modifie {
        requete = requete.header("If-Modified-Since", m);
    }
    let reponse = requete
        .send()
        .await
        .map_err(|e| Error::other(format!("could not reach the calendar ({e})")))?;
    if reponse.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Lecture::Inchange);
    }
    if !reponse.status().is_success() {
        return Err(Error::other(format!(
            "the server answered {}",
            reponse.status()
        )));
    }
    let entete = |nom: &str| {
        reponse
            .headers()
            .get(nom)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let (etag, modifie) = (entete("etag"), entete("last-modified"));
    if reponse
        .content_length()
        .is_some_and(|n| n as usize > MAX_ICS)
    {
        return Err(Error::other("the calendar file is too large"));
    }
    let octets = reponse
        .bytes()
        .await
        .map_err(|e| Error::other(format!("download interrupted ({e})")))?;
    if octets.len() > MAX_ICS {
        return Err(Error::other("the calendar file is too large"));
    }
    Ok(Lecture::Nouveau {
        texte: String::from_utf8_lossy(&octets).into_owned(),
        etag,
        modifie,
    })
}

/// Relit un abonnement et remplace ses événements.
pub async fn refresh_subscription(services: &Services, id: i64) -> Result<usize> {
    let cal = services
        .store
        .calendar(id)?
        .ok_or_else(|| Error::other("calendar not found"))?;
    let Some(url) = cal.source_url.clone() else {
        return Ok(0);
    };
    type Lu = (
        Option<iris_calendar::ics::Parsed>,
        Option<String>,
        Option<String>,
    );
    let resultat: Result<Lu> = async {
        match lire_abonnement(&url, cal.etag.as_deref(), cal.last_modified.as_deref()).await? {
            Lecture::Inchange => Ok((None, None, None)),
            Lecture::Nouveau {
                texte,
                etag,
                modifie,
            } => {
                let lu = iris_calendar::ics::parse(&texte).map_err(Error::other)?;
                Ok((Some(lu), etag, modifie))
            }
        }
    }
    .await;
    match resultat {
        Ok((lu, etag, modifie)) => {
            let n = match lu {
                Some(lu) => {
                    let nouveaux: Vec<NewEvent> = lu.events.iter().map(depuis_domaine).collect();
                    services
                        .store
                        .replace_calendar_events(id, &nouveaux, now())?
                }
                None => services.store.calendar_event_count(id)? as usize,
            };
            services.store.set_calendar_sync(
                id,
                etag.as_deref(),
                modifie.as_deref(),
                now(),
                None,
            )?;
            Ok(n)
        }
        Err(e) => {
            let message = e.to_string();
            let _ = services
                .store
                .set_calendar_sync(id, None, None, now(), Some(&message));
            Err(e)
        }
    }
}

/// S'abonne à un agenda publié : le lit d'abord, et ne crée le calendrier que s'il se
/// lit. Un lien qui ne mène pas à un agenda ne laisse rien derrière lui.
pub async fn subscribe(services: &Services, lien: &str, nom: &str) -> Result<(i64, usize)> {
    let url = iris_calendar::link::normalize(lien).map_err(Error::other)?;
    if services
        .store
        .calendars()?
        .iter()
        .any(|c| c.source_url.as_deref() == Some(url.as_str()))
    {
        return Err(Error::other("You are already subscribed to this calendar."));
    }
    let Lecture::Nouveau {
        texte,
        etag,
        modifie,
    } = lire_abonnement(&url, None, None).await?
    else {
        return Err(Error::other("the server sent nothing"));
    };
    let lu = iris_calendar::ics::parse(&texte)
        .map_err(|_| Error::other("That link does not lead to a calendar (.ics)."))?;

    let nom = if !nom.trim().is_empty() {
        nom.trim().to_string()
    } else {
        lu.name
            .clone()
            .unwrap_or_else(|| iris_calendar::link::default_name(&url))
    };
    let deja = services.store.calendars()?.len();
    let couleur = COULEURS[deja % COULEURS.len()];
    let id = services
        .store
        .create_calendar(&nom, couleur, Some(&url), now())?;
    let nouveaux: Vec<NewEvent> = lu.events.iter().map(depuis_domaine).collect();
    let n = services
        .store
        .replace_calendar_events(id, &nouveaux, now())?;
    services
        .store
        .set_calendar_sync(id, etag.as_deref(), modifie.as_deref(), now(), None)?;
    Ok((id, n))
}

// --- Les invitations ---------------------------------------------------------------

/// Ce qu'un fichier d'agenda importé a changé.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    pub updated: usize,
    pub cancelled: usize,
    /// Le jour du premier événement, `YYYY-MM-DD`, pour y conduire.
    pub first_day: Option<String>,
    pub first_title: Option<String>,
}

impl ImportReport {
    pub fn message(&self) -> String {
        let titre = self.first_title.as_deref().unwrap_or("the event");
        match (self.added, self.updated, self.cancelled) {
            (0, 0, 0) => "The invitation holds no event.".into(),
            (_, _, c) if c > 0 && self.added == 0 && self.updated == 0 => {
                format!("Cancelled in your calendar: {titre}.")
            }
            (1, 0, _) => format!("Added to your calendar: {titre}."),
            (0, 1, _) => format!("Updated in your calendar: {titre}."),
            (a, u, _) => format!("{a} added, {u} updated in your calendar."),
        }
    }
}

/// Importe un fichier iCalendar — une invitation reçue, un événement partagé — dans le
/// premier calendrier local.
///
/// Un événement déjà importé (même `UID`, même occurrence) est **mis à jour** plutôt
/// que dupliqué : l'organisateur qui déplace sa réunion renvoie la même invitation
/// modifiée. Une annulation efface l'événement de l'agenda.
pub fn import_ics(services: &Services, texte: &str) -> Result<ImportReport> {
    let lu = iris_calendar::ics::parse(texte).map_err(Error::other)?;
    let calendrier = services
        .store
        .calendars()?
        .into_iter()
        .find(|c| !c.is_subscription())
        .ok_or_else(|| Error::other("there is no local calendar"))?;

    let mut bilan = ImportReport::default();
    for e in &lu.events {
        let mut nouveau = depuis_domaine(e);
        if nouveau.uid.is_empty() {
            nouveau.uid = format!("{}-{}@iris", now().millis(), bilan.added);
        }
        match services
            .store
            .find_event(calendrier.id, &nouveau.uid, nouveau.recurrence_id)?
        {
            Some(id) if e.cancelled => {
                services.store.set_event_cancelled(id, true)?;
                bilan.cancelled += 1;
            }
            Some(id) => {
                services
                    .store
                    .update_event(id, calendrier.id, &nouveau, now())?;
                services.store.set_event_cancelled(id, false)?;
                bilan.updated += 1;
            }
            None if e.cancelled => bilan.cancelled += 1,
            None => {
                services
                    .store
                    .insert_event(calendrier.id, &nouveau, now())?;
                bilan.added += 1;
            }
        }
        if bilan.first_day.is_none() && !e.cancelled {
            let jour = if e.all_day {
                layout::local_date(e.start, &chrono::Utc)
            } else {
                layout::local_date(e.start, &Local)
            };
            bilan.first_day = Some(jour.format("%Y-%m-%d").to_string());
        }
        if bilan.first_title.is_none() {
            bilan.first_title = Some(titre(e));
        }
    }
    Ok(bilan)
}

// --- Le câblage --------------------------------------------------------------------

/// Local midnight of a day, in milliseconds.
pub fn local_midnight_ms(day: NaiveDate) -> i64 {
    layout::local_midnight(day, &Local)
}

/// An occurrence in the days to come, as Home lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upcoming {
    /// The key the calendar opens it by.
    pub key: String,
    pub title: String,
    pub day: NaiveDate,
    /// "14:30", empty for all day.
    pub time: String,
    /// `#rrggbb`, its calendar's.
    pub color: String,
    /// Over already.
    pub past: bool,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub location: String,
}

/// The occurrences of the visible calendars from `from`, over `days` days, in order.
pub fn upcoming(services: &Services, from: NaiveDate, days: i64) -> Vec<Upcoming> {
    let de = layout::local_midnight(from, &Local);
    let a = layout::local_midnight(from + Duration::days(days), &Local);
    let stockes = services
        .store
        .events_for_range(de - 86_400_000, a + 86_400_000)
        .unwrap_or_default();
    let couleurs: HashMap<i64, String> = services
        .store
        .calendars()
        .unwrap_or_default()
        .into_iter()
        .map(|c| (c.id, c.color))
        .collect();
    let domaine: Vec<Event> = stockes.iter().map(|s| vers_domaine(&s.event)).collect();
    let maintenant = now().millis();
    let mut sortie: Vec<Upcoming> = iris_calendar::recur::occurrences(&domaine, de, a)
        .into_iter()
        .filter_map(|o| {
            let s = &stockes[o.event];
            let jour = if o.all_day {
                layout::local_date(o.start, &chrono::Utc)
            } else {
                layout::local_date(o.start, &Local)
            };
            // An event that began before the period shows from its first day in it.
            let jour = jour.max(from);
            if jour >= from + Duration::days(days) || o.end <= de {
                return None;
            }
            Some(Upcoming {
                key: format!("{}:{}", s.id, o.start),
                title: titre(&domaine[o.event]),
                day: jour,
                time: if o.all_day {
                    String::new()
                } else {
                    heure(o.start)
                },
                color: couleurs.get(&s.calendar_id).cloned().unwrap_or_default(),
                past: o.end <= maintenant,
                start: o.start,
                end: o.end,
                all_day: o.all_day,
                location: s.event.location.clone(),
            })
        })
        .collect();
    sortie.sort_by_key(|u| (u.day, !u.time.is_empty(), u.start));
    sortie
}

fn ms_depuis_cle(cle: &str) -> Option<(i64, i64)> {
    let (id, debut) = cle.split_once(':')?;
    Some((id.parse().ok()?, debut.parse().ok()?))
}

/// Branche l'agenda sur la fenêtre, et lance ses tâches de fond.
pub fn wire_calendar(fenetre: &AppWindow, services: &Services, runtime: tokio::runtime::Handle) {
    let today = aujourd_hui();
    let etat = Rc::new(RefCell::new(Etat {
        // The view chosen last time; the week the first time.
        mode: crate::settings::current().calendar_view.clamp(0, 2),
        jour: today,
        mini: premier_du_mois(today),
        evenements: Vec::new(),
        edite: None,
        locaux: Vec::new(),
        rappeles: HashSet::new(),
        selecteur: None,
        note: None,
        ouvert: None,
    }));

    fenetre.set_calendar_palette(ModelRc::new(VecModel::from(
        COULEURS.iter().map(|c| couleur(c)).collect::<Vec<_>>(),
    )));

    fenetre.set_editor_reminders(ModelRc::new(VecModel::from(
        RAPPELS
            .iter()
            .map(|(n, _)| SharedString::from(*n))
            .collect::<Vec<_>>(),
    )));
    fenetre.set_editor_repeats(ModelRc::new(VecModel::from(
        REPETITIONS
            .iter()
            .map(|(n, _)| SharedString::from(*n))
            .collect::<Vec<_>>(),
    )));

    // Une fermeture qui recalcule la vue, partagée par tous les gestes.
    let redessiner = {
        let (faible, services, etat) = (fenetre.as_weak(), services.clone(), Rc::clone(&etat));
        Rc::new(move || {
            if let Some(f) = faible.upgrade() {
                rafraichir(&f, &services, &mut etat.borrow_mut());
            }
        })
    };

    // An event dragged to another time, or stretched by its lower edge.
    {
        let (faible, services, redessiner) =
            (fenetre.as_weak(), services.clone(), Rc::clone(&redessiner));
        fenetre.on_calendar_event_moved(move |cle, jours, minutes| {
            if let Err(e) = deplacer(&services, &cle, jours as i64, minutes as i64, false) {
                if let Some(f) = faible.upgrade() {
                    f.set_status(format!("Could not move it: {e}").into());
                }
            }
            redessiner();
        });
    }
    {
        let (faible, services, redessiner) =
            (fenetre.as_weak(), services.clone(), Rc::clone(&redessiner));
        fenetre.on_calendar_event_resized(move |cle, minutes| {
            if let Err(e) = deplacer(&services, &cle, 0, minutes as i64, true) {
                if let Some(f) = faible.upgrade() {
                    f.set_status(format!("Could not change its length: {e}").into());
                }
            }
            redessiner();
        });
    }
    // A task dropped on the week: booked there, for as long as it takes.
    {
        let (faible, services, redessiner) =
            (fenetre.as_weak(), services.clone(), Rc::clone(&redessiner));
        fenetre.on_calendar_task_planned(move |id, jour, minute| {
            let Some(jour) = lire_date(&jour) else {
                return;
            };
            let message = match crate::tasks::book(&services, id as i64, jour, minute) {
                Ok(m) => m,
                Err(e) => e,
            };
            if let Some(f) = faible.upgrade() {
                f.set_status(message.into());
            }
            redessiner();
        });
    }

    // A new colour for a calendar, from its menu.
    {
        let (services, redessiner) = (services.clone(), Rc::clone(&redessiner));
        fenetre.on_calendar_color_chosen(move |id, i| {
            let (Ok(Some(c)), Some(hex)) = (
                services.store.calendar(id as i64),
                COULEURS.get(i.max(0) as usize),
            ) else {
                return;
            };
            let _ = services
                .store
                .update_calendar(c.id, &c.name, hex, c.visible);
            redessiner();
        });
    }

    {
        let redessiner = Rc::clone(&redessiner);
        crate::workspace::follow(fenetre, move |w| {
            if w == 1 {
                redessiner();
            }
        });
    }
    {
        let (etat, redessiner) = (Rc::clone(&etat), Rc::clone(&redessiner));
        fenetre.on_calendar_navigate(move |n| {
            {
                let mut e = etat.borrow_mut();
                e.jour = match e.mode {
                    0 => ajouter_mois(e.jour, n),
                    1 => e.jour + Duration::days(7 * n as i64),
                    _ => e.jour + Duration::days(n as i64),
                };
                e.mini = premier_du_mois(e.jour);
            }
            redessiner();
        });
    }
    {
        let (etat, redessiner) = (Rc::clone(&etat), Rc::clone(&redessiner));
        fenetre.on_calendar_today(move || {
            {
                let mut e = etat.borrow_mut();
                e.jour = aujourd_hui();
                e.mini = premier_du_mois(e.jour);
            }
            redessiner();
        });
    }
    {
        let (etat, redessiner) = (Rc::clone(&etat), Rc::clone(&redessiner));
        fenetre.on_calendar_mode_chosen(move |m| {
            let m = m.clamp(0, 2);
            etat.borrow_mut().mode = m;
            // Kept for next time: the calendar reopens the way it was left.
            crate::settings::update(|s| s.calendar_view = m);
            redessiner();
        });
    }
    {
        let (etat, redessiner) = (Rc::clone(&etat), Rc::clone(&redessiner));
        fenetre.on_calendar_day_chosen(move |d| {
            if let Some(jour) = lire_date(&d) {
                let mut e = etat.borrow_mut();
                e.jour = jour;
                e.mini = premier_du_mois(jour);
            }
            redessiner();
        });
    }
    {
        let (etat, redessiner) = (Rc::clone(&etat), Rc::clone(&redessiner));
        fenetre.on_calendar_day_opened(move |d| {
            if let Some(jour) = lire_date(&d) {
                let mut e = etat.borrow_mut();
                e.jour = jour;
                e.mode = 2;
                e.mini = premier_du_mois(jour);
            }
            redessiner();
        });
    }
    {
        let (etat, redessiner) = (Rc::clone(&etat), Rc::clone(&redessiner));
        fenetre.on_calendar_mini_navigate(move |n| {
            {
                let mut e = etat.borrow_mut();
                e.mini = premier_du_mois(ajouter_mois(e.mini, n));
            }
            redessiner();
        });
    }
    {
        let (services, redessiner) = (services.clone(), Rc::clone(&redessiner));
        fenetre.on_calendar_toggled(move |id| {
            if let Ok(Some(c)) = services.store.calendar(id as i64) {
                let _ = services
                    .store
                    .update_calendar(c.id, &c.name, &c.color, !c.visible);
            }
            redessiner();
        });
    }

    // --- Un événement ---
    {
        let (services, etat, faible) = (services.clone(), Rc::clone(&etat), fenetre.as_weak());
        fenetre.on_calendar_event_opened(move |k| {
            let Some(f) = faible.upgrade() else { return };
            let Some((id, debut)) = ms_depuis_cle(&k) else {
                return;
            };
            let Ok(Some(s)) = services.store.event(id) else {
                return;
            };
            let cal = services.store.calendar(s.calendar_id).ok().flatten();
            let duree = s.event.end_ms - s.event.start_ms;
            let o = Occurrence {
                event: 0,
                start: debut,
                end: debut + duree,
                all_day: s.event.all_day,
            };
            let occurrence = if s.event.rrule.is_some() { debut } else { 0 };
            let ouvert = Ouvert {
                uid: s.event.uid.clone(),
                occurrence,
                titre: titre(&vers_domaine(&s.event)),
                debut,
                all_day: s.event.all_day,
            };
            remplir_taches(&f, &services, &ouvert);
            f.set_event_new_task(SharedString::default());
            {
                let mut e = etat.borrow_mut();
                e.edite = Some(id);
                e.note = Some((s.calendar_id, s.event.uid.clone(), occurrence));
                e.ouvert = Some(ouvert);
            }
            f.set_event_notes(
                services
                    .store
                    .event_note(s.calendar_id, &s.event.uid, occurrence)
                    .unwrap_or_default()
                    .into(),
            );
            f.set_event_detail(EventDetailData {
                key: k,
                title: titre(&vers_domaine(&s.event)).into(),
                when: quand(&o).into(),
                calendar: cal
                    .as_ref()
                    .map(|c| c.name.clone())
                    .unwrap_or_default()
                    .into(),
                color: couleur(cal.as_ref().map(|c| c.color.as_str()).unwrap_or("")),
                location: s.event.location.as_str().into(),
                description: s.event.description.as_str().into(),
                reminder: rappel(s.event.reminder_minutes).into(),
                repeats: repetition(s.event.rrule.as_deref()).into(),
                editable: cal.is_some_and(|c| !c.is_subscription()),
            });
            f.set_event_detail_open(true);
        });
    }
    {
        let (services, etat, faible) = (services.clone(), Rc::clone(&etat), fenetre.as_weak());
        fenetre.on_calendar_new_event(move |d, minute| {
            let Some(f) = faible.upgrade() else { return };
            let mut e = etat.borrow_mut();
            let jour = lire_date(&d).unwrap_or(e.jour);
            ouvrir_editeur(&f, &mut e, &services, None, jour, minute);
        });
    }
    // A title typed straight into the grid: the event exists at once.
    {
        let (services, etat, faible, redessiner) = (
            services.clone(),
            Rc::clone(&etat),
            fenetre.as_weak(),
            Rc::clone(&redessiner),
        );
        fenetre.on_calendar_quick_event(move |d, minute, titre| {
            let Some(f) = faible.upgrade() else { return };
            if titre.trim().is_empty() {
                return;
            }
            let (jour, calendrier) = {
                let e = etat.borrow();
                (lire_date(&d).unwrap_or(e.jour), e.locaux.first().copied())
            };
            let Some(calendrier) = calendrier else {
                f.set_toast("There is no calendar to put it in.".into());
                return;
            };
            match evenement_rapide(&services, calendrier, jour, minute, &titre) {
                Ok(()) => redessiner(),
                Err(e) => f.set_toast(format!("Could not add the event: {e}").into()),
            }
        });
    }
    {
        let (services, etat, faible) = (services.clone(), Rc::clone(&etat), fenetre.as_weak());
        fenetre.on_event_edit(move || {
            let Some(f) = faible.upgrade() else { return };
            let mut e = etat.borrow_mut();
            let Some(id) = e.edite else { return };
            if let Ok(Some(s)) = services.store.event(id) {
                let jour = e.jour;
                ouvrir_editeur(&f, &mut e, &services, Some(&s), jour, -1);
            }
        });
    }
    // --- The tasks of an event ---
    {
        let (services, etat, faible) = (services.clone(), Rc::clone(&etat), fenetre.as_weak());
        fenetre.on_event_task_added(move |texte| {
            let Some(f) = faible.upgrade() else { return };
            let Some(o) = etat.borrow().ouvert.clone() else {
                return;
            };
            let Some(liste) = services
                .store
                .task_lists()
                .ok()
                .and_then(|l| l.first().map(|l| l.id))
            else {
                return;
            };
            let t = tache_d_evenement(&o, liste, &texte);
            if t.title.is_empty() {
                return;
            }
            if services.store.insert_task(&t, now()).is_ok() {
                f.set_event_new_task(SharedString::default());
                f.set_status(format!("Added to your tasks: {}.", t.title).into());
            }
            remplir_taches(&f, &services, &o);
        });
    }
    {
        let (services, etat, faible) = (services.clone(), Rc::clone(&etat), fenetre.as_weak());
        fenetre.on_event_task_toggled(move |id| {
            let Some(f) = faible.upgrade() else { return };
            if let Ok(Some(t)) = services.store.task(id as i64) {
                let fait = if t.is_done() { None } else { Some(now()) };
                let _ = services.store.set_task_done(t.id, fait);
            }
            if let Some(o) = etat.borrow().ouvert.as_ref() {
                remplir_taches(&f, &services, o);
            }
        });
    }
    {
        let faible = fenetre.as_weak();
        fenetre.on_event_task_opened(move |id| {
            let Some(f) = faible.upgrade() else { return };
            f.set_event_detail_open(false);
            crate::tasks::show_task(&f, id as i64);
        });
    }

    // What the user writes about an event, saved as it is typed.
    {
        let (services, etat) = (services.clone(), Rc::clone(&etat));
        fenetre.on_event_notes_edited(move |texte| {
            if let Some((cal, uid, occurrence)) = etat.borrow().note.clone() {
                let _ = services
                    .store
                    .set_event_note(cal, &uid, occurrence, &texte, now());
            }
        });
    }
    {
        let (services, etat, faible, redessiner) = (
            services.clone(),
            Rc::clone(&etat),
            fenetre.as_weak(),
            Rc::clone(&redessiner),
        );
        fenetre.on_event_delete(move || {
            let Some(f) = faible.upgrade() else { return };
            let id = etat.borrow().edite;
            if let Some(id) = id {
                // Its notes go with it.
                if let Ok(Some(ev)) = services.store.event(id) {
                    let _ = services
                        .store
                        .move_event_notes(ev.calendar_id, &ev.event.uid, None);
                }
                match services.store.delete_event(id) {
                    Ok(()) => f.set_status("Event deleted.".into()),
                    Err(e) => f.set_status(format!("Could not delete the event: {e}").into()),
                }
            }
            f.set_event_detail_open(false);
            redessiner();
        });
    }
    {
        let (services, etat, faible, redessiner) = (
            services.clone(),
            Rc::clone(&etat),
            fenetre.as_weak(),
            Rc::clone(&redessiner),
        );
        fenetre.on_event_save(move || {
            let Some(f) = faible.upgrade() else { return };
            let mut nouveau = match lire_editeur(&f) {
                Ok(e) => e,
                Err(message) => {
                    f.set_editor_error(message.into());
                    return;
                }
            };
            let (edite, calendrier) = {
                let e = etat.borrow();
                let index = f.get_editor_calendar_index().max(0) as usize;
                (
                    e.edite,
                    e.locaux
                        .get(index)
                        .copied()
                        .or_else(|| e.locaux.first().copied()),
                )
            };
            let Some(calendrier) = calendrier else {
                f.set_editor_error("There is no calendar to put it in.".into());
                return;
            };
            let resultat = match edite {
                Some(id) => {
                    // Moved to another calendar, its notes follow it.
                    if let Ok(Some(ancien)) = services.store.event(id) {
                        if ancien.calendar_id != calendrier {
                            let _ = services.store.move_event_notes(
                                ancien.calendar_id,
                                &ancien.event.uid,
                                Some(calendrier),
                            );
                        }
                    }
                    services.store.update_event(id, calendrier, &nouveau, now())
                }
                None => {
                    nouveau.uid = format!("{}-{}@iris", now().millis(), std::process::id());
                    services
                        .store
                        .insert_event(calendrier, &nouveau, now())
                        .map(|_| ())
                }
            };
            match resultat {
                Ok(()) => {
                    f.set_event_editor_open(false);
                    // La période montrée suit l'événement, pour qu'on le voie.
                    if let Some(jour) = lire_date(&f.get_editor_start_date()) {
                        let mut e = etat.borrow_mut();
                        e.jour = jour;
                        e.mini = premier_du_mois(jour);
                    }
                    redessiner();
                }
                Err(e) => f.set_editor_error(format!("Could not save: {e}").into()),
            }
        });
    }

    // --- Les abonnements ---
    {
        let (services, faible, runtime) = (services.clone(), fenetre.as_weak(), runtime.clone());
        fenetre.on_subscribe_confirmed(move || {
            let Some(f) = faible.upgrade() else { return };
            let (lien, nom) = (
                f.get_subscribe_url().to_string(),
                f.get_subscribe_name().to_string(),
            );
            if let Err(e) = iris_calendar::link::normalize(&lien) {
                f.set_subscribe_error(e.into());
                return;
            }
            f.set_subscribe_error(SharedString::default());
            f.set_subscribe_busy(true);
            let (services, faible) = (services.clone(), faible.clone());
            runtime.spawn(async move {
                let resultat = subscribe(&services, &lien, &nom).await;
                let _ = faible.upgrade_in_event_loop(move |f| {
                    f.set_subscribe_busy(false);
                    match resultat {
                        Ok((_, n)) => {
                            f.set_subscribe_open(false);
                            f.set_toast(
                                format!("Subscribed: {n} event(s) added to your calendar.").into(),
                            );
                            f.invoke_workspace_changed(1);
                        }
                        Err(e) => f.set_subscribe_error(e.to_string().into()),
                    }
                });
            });
        });
    }
    {
        let (services, faible, runtime) = (services.clone(), fenetre.as_weak(), runtime.clone());
        fenetre.on_calendar_refresh(move |id| {
            let Some(f) = faible.upgrade() else { return };
            f.set_status("Updating the calendar…".into());
            let (services, faible) = (services.clone(), faible.clone());
            runtime.spawn(async move {
                let resultat = refresh_subscription(&services, id as i64).await;
                let _ = faible.upgrade_in_event_loop(move |f| {
                    f.set_status(
                        match resultat {
                            Ok(n) => format!("Calendar up to date: {n} event(s)."),
                            Err(e) => format!("Could not update the calendar: {e}"),
                        }
                        .into(),
                    );
                    if f.get_workspace() == 1 {
                        f.invoke_workspace_changed(1);
                    }
                });
            });
        });
    }
    {
        let (services, redessiner, faible) =
            (services.clone(), Rc::clone(&redessiner), fenetre.as_weak());
        fenetre.on_calendar_delete_confirmed(move |id| {
            let calendrier = services.store.calendar(id as i64).ok().flatten();
            // The menu does not offer it; refused here too, whatever asks.
            let locaux = services
                .store
                .calendars()
                .unwrap_or_default()
                .iter()
                .filter(|c| !c.is_subscription())
                .count();
            if calendrier.as_ref().is_some_and(|c| !c.is_subscription()) && locaux <= 1 {
                return;
            }
            let message = match (&calendrier, services.store.delete_calendar(id as i64)) {
                (Some(c), Ok(_)) if c.is_subscription() => format!("Unsubscribed from {}.", c.name),
                (Some(c), Ok(_)) => format!("Calendar {} deleted.", c.name),
                (_, Err(e)) => format!("Could not remove the calendar: {e}"),
                (None, Ok(_)) => return,
            };
            if let Some(f) = faible.upgrade() {
                f.set_status(message.into());
            }
            redessiner();
        });
    }
    {
        let (services, redessiner, faible) =
            (services.clone(), Rc::clone(&redessiner), fenetre.as_weak());
        fenetre.on_calendar_rename_confirmed(move |id, nom| {
            let Some(f) = faible.upgrade() else { return };
            let nom = nom.trim();
            if nom.is_empty() {
                f.set_calendar_rename_error("A calendar needs a name.".into());
                return;
            }
            // A new calendar of one's own, in the first colour nobody has yet.
            if id < 0 {
                let prises: Vec<String> = services
                    .store
                    .calendars()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| c.color)
                    .collect();
                let teinte = COULEURS
                    .iter()
                    .find(|c| !prises.iter().any(|p| p.eq_ignore_ascii_case(c)))
                    .copied()
                    .unwrap_or(COULEURS[prises.len() % COULEURS.len()]);
                match services.store.create_calendar(nom, teinte, None, now()) {
                    Ok(_) => {
                        f.set_calendar_rename_error(SharedString::default());
                        f.set_calendar_rename_open(false);
                        f.set_status(format!("Calendar {nom} created.").into());
                        redessiner();
                    }
                    Err(e) => {
                        f.set_calendar_rename_error(format!("Could not create it: {e}").into())
                    }
                }
                return;
            }
            let Ok(Some(c)) = services.store.calendar(id as i64) else {
                f.set_calendar_rename_open(false);
                return;
            };
            match services
                .store
                .update_calendar(c.id, nom, &c.color, c.visible)
            {
                Ok(_) => {
                    f.set_calendar_rename_error(SharedString::default());
                    f.set_calendar_rename_open(false);
                    redessiner();
                }
                Err(e) => f.set_calendar_rename_error(format!("Could not rename it: {e}").into()),
            }
        });
    }

    // --- The small month under the editor's dates ---
    {
        let (etat, faible) = (Rc::clone(&etat), fenetre.as_weak());
        fenetre.on_editor_picker_requested(move |lequel| {
            let Some(f) = faible.upgrade() else { return };
            let mut e = etat.borrow_mut();
            // A second click on the same date folds it. The window says what is
            // open: Escape folds it there without asking us.
            if f.get_editor_picker() == lequel {
                e.selecteur = None;
            } else {
                let champ = if lequel == 0 {
                    f.get_editor_start_date()
                } else {
                    f.get_editor_end_date()
                };
                let jour = lire_date(&champ).unwrap_or_else(aujourd_hui);
                e.selecteur = Some((lequel, premier_du_mois(jour)));
            }
            remplir_selecteur(&f, &e);
        });
    }
    {
        let (etat, faible) = (Rc::clone(&etat), fenetre.as_weak());
        fenetre.on_editor_picker_navigate(move |n| {
            let Some(f) = faible.upgrade() else { return };
            let mut e = etat.borrow_mut();
            if let Some((lequel, mois)) = e.selecteur {
                e.selecteur = Some((lequel, premier_du_mois(ajouter_mois(mois, n))));
            }
            remplir_selecteur(&f, &e);
        });
    }
    {
        let (etat, faible) = (Rc::clone(&etat), fenetre.as_weak());
        fenetre.on_editor_picker_chosen(move |d| {
            let Some(f) = faible.upgrade() else { return };
            let mut e = etat.borrow_mut();
            let (Some((lequel, _)), Some(jour)) = (e.selecteur, lire_date(&d)) else {
                return;
            };
            let debut = lire_date(&f.get_editor_start_date()).unwrap_or(jour);
            let fin = lire_date(&f.get_editor_end_date()).unwrap_or(debut);
            let (debut, fin) = dates_apres_choix(lequel, jour, debut, fin);
            poser_dates(&f, debut, fin);
            e.selecteur = None;
            remplir_selecteur(&f, &e);
        });
    }

    // Les abonnements se relisent d'eux-mêmes, toutes les demi-heures, et une première
    // fois peu après le démarrage.
    {
        let (services, faible) = (services.clone(), fenetre.as_weak());
        runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            loop {
                let abonnements: Vec<StoredCalendar> = services
                    .store
                    .calendars()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| c.is_subscription())
                    .collect();
                let mut change = false;
                for c in abonnements {
                    let du = c.last_sync.is_none_or(|t| {
                        now().millis() - t.millis() >= RELECTURE.as_millis() as i64 - 60_000
                    });
                    if du {
                        if let Err(e) = refresh_subscription(&services, c.id).await {
                            tracing::info!(calendar = %c.name, error = %e, "calendar subscription");
                        }
                        change = true;
                    }
                }
                if change {
                    let _ = faible.upgrade_in_event_loop(|f| {
                        if f.get_workspace() == 1 {
                            f.invoke_workspace_changed(1);
                        }
                    });
                }
                tokio::time::sleep(RELECTURE).await;
            }
        });
    }

    // Les rappels, et la ligne de l'heure qui avance.
    let minuterie = slint::Timer::default();
    {
        let (services, etat, faible, redessiner) = (
            services.clone(),
            Rc::clone(&etat),
            fenetre.as_weak(),
            Rc::clone(&redessiner),
        );
        minuterie.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_secs(30),
            move || {
                rappels(&services, &mut etat.borrow_mut());
                if let Some(f) = faible.upgrade() {
                    if f.get_workspace() == 1 && f.window().is_visible() && etat.borrow().mode != 0
                    {
                        redessiner();
                    }
                }
            },
        );
    }
    // La minuterie vit aussi longtemps que la fenêtre.
    MINUTERIE.with(|m| *m.borrow_mut() = Some(minuterie));
}

thread_local! {
    static MINUTERIE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Donne les rappels arrivés à échéance.
///
/// Relu toutes les trente secondes sur la journée qui vient : un rappel « un jour
/// avant » d'une réunion de demain matin tombe aujourd'hui. Un rappel déjà passé de
/// plus de deux minutes — l'ordinateur était en veille — n'est pas donné en retard.
fn rappels(services: &Services, etat: &mut Etat) {
    let maintenant = now().millis();
    let stockes = services
        .store
        .events_for_range(maintenant, maintenant + 2 * 86_400_000)
        .unwrap_or_default();
    let evenements: Vec<Event> = stockes.iter().map(|s| vers_domaine(&s.event)).collect();
    for o in iris_calendar::recur::occurrences(&evenements, maintenant, maintenant + 2 * 86_400_000)
    {
        let e = &evenements[o.event];
        let Some(minutes) = e.reminder_minutes else {
            continue;
        };
        let echeance = o.start - minutes as i64 * 60_000;
        let cle = format!("{}:{}", stockes[o.event].id, o.start);
        if maintenant >= echeance && maintenant - echeance < 120_000 && etat.rappeles.insert(cle) {
            let corps = if o.all_day {
                "Today".to_string()
            } else if minutes == 0 {
                format!("Now · {}", heure(o.start))
            } else {
                format!(
                    "At {} · {}",
                    heure(o.start),
                    rappel(Some(minutes)).to_lowercase()
                )
            };
            let corps = if e.location.is_empty() {
                corps
            } else {
                format!("{corps} · {}", e.location)
            };
            crate::notify::show_text(&titre(e), &corps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(t: &str) -> NaiveDate {
        lire_date(t).unwrap()
    }

    #[test]
    fn moving_the_start_moves_the_end_with_it() {
        assert_eq!(
            dates_apres_choix(0, d("2026-10-05"), d("2026-09-28"), d("2026-09-30")),
            (d("2026-10-05"), d("2026-10-07"))
        );
    }

    #[test]
    fn an_end_before_the_start_pulls_the_start_back() {
        assert_eq!(
            dates_apres_choix(1, d("2026-09-20"), d("2026-09-28"), d("2026-09-28")),
            (d("2026-09-20"), d("2026-09-20"))
        );
        assert_eq!(
            dates_apres_choix(1, d("2026-10-02"), d("2026-09-28"), d("2026-09-28")),
            (d("2026-09-28"), d("2026-10-02"))
        );
    }

    #[test]
    fn a_date_reads_as_words() {
        assert_eq!(date_lisible(d("2026-09-28")), "Mon 28 Sep 2026");
    }

    #[test]
    fn les_dates_et_heures_se_saisissent_de_plusieurs_facons() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 28).unwrap();
        assert_eq!(lire_date("2026-09-28"), Some(d));
        assert_eq!(lire_date("28/09/2026"), Some(d));
        assert_eq!(lire_date("28.09.2026"), Some(d));
        assert_eq!(lire_date("demain"), None);
        let h = |h, m| NaiveTime::from_hms_opt(h, m, 0);
        assert_eq!(lire_heure("9:30"), h(9, 30));
        assert_eq!(lire_heure("09:30"), h(9, 30));
        assert_eq!(lire_heure("9h30"), h(9, 30));
        assert_eq!(lire_heure("9h"), h(9, 0));
        assert_eq!(lire_heure("14"), h(14, 0));
        assert_eq!(lire_heure("25:00"), None);
    }

    #[test]
    fn un_mois_de_plus_ne_deborde_pas() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        assert_eq!(
            ajouter_mois(d, 1),
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
        );
        assert_eq!(
            ajouter_mois(d, -1),
            NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()
        );
        assert_eq!(
            ajouter_mois(d, 12),
            NaiveDate::from_ymd_opt(2027, 1, 31).unwrap()
        );
    }

    #[test]
    fn une_couleur_se_lit() {
        assert_eq!(
            couleur("#5b8def"),
            slint::Color::from_rgb_u8(0x5b, 0x8d, 0xef)
        );
        assert_eq!(
            couleur("n'importe"),
            slint::Color::from_rgb_u8(0x88, 0x88, 0x88)
        );
    }

    #[test]
    fn la_repetition_se_decrit() {
        assert_eq!(repetition(Some("FREQ=WEEKLY")), "Repeats every week");
        assert_eq!(
            repetition(Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR")),
            "Repeats every weekday"
        );
        assert_eq!(
            repetition(Some("FREQ=MONTHLY;BYMONTHDAY=3")),
            "Repeats monthly"
        );
        assert_eq!(repetition(None), "");
    }

    #[test]
    fn un_rappel_se_decrit() {
        assert_eq!(rappel(Some(15)), "15 minutes before");
        assert_eq!(rappel(Some(60)), "1 hour(s) before");
        assert_eq!(rappel(Some(1440)), "1 day(s) before");
        assert_eq!(rappel(Some(0)), "At start");
    }

    #[test]
    fn un_evenement_passe_du_domaine_a_la_base_sans_perte() {
        let e = Event {
            uid: "u".into(),
            summary: "s".into(),
            start: 1,
            end: 2,
            rrule: Some("FREQ=DAILY".into()),
            exdates: vec![3],
            reminder_minutes: Some(5),
            ..Default::default()
        };
        assert_eq!(vers_domaine(&depuis_domaine(&e)), e);
    }

    #[test]
    fn une_cle_d_occurrence_se_relit() {
        assert_eq!(
            ms_depuis_cle("12:1790586000000"),
            Some((12, 1_790_586_000_000))
        );
        assert_eq!(ms_depuis_cle("n'importe"), None);
    }

    /// Un serveur HTTP d'une ligne, sur la boucle locale : il sert `corps` avec un
    /// ETag, répond 304 quand on le lui rend, et compte les requêtes reçues.
    fn serveur(
        versions: Vec<&'static str>,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let ecoute = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let adresse = format!("http://{}/agenda.ics", ecoute.local_addr().unwrap());
        let compte = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = std::sync::Arc::clone(&compte);
        std::thread::spawn(move || {
            for flux in ecoute.incoming().flatten() {
                let mut flux = flux;
                let mut tampon = [0u8; 4096];
                let n = flux.read(&mut tampon).unwrap_or(0);
                let requete = String::from_utf8_lossy(&tampon[..n]).to_ascii_lowercase();
                let i = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let version = versions[i.min(versions.len() - 1)];
                let etag = format!("\"v{}\"", version.len());
                let reponse = if requete.contains(&format!("if-none-match: {etag}")) {
                    ["HTTP/1.1 304 Not Modified", "Content-Length: 0", "", ""].join("\r\n")
                } else {
                    [
                        "HTTP/1.1 200 OK",
                        "Content-Type: text/calendar",
                        &format!("ETag: {etag}"),
                        &format!("Content-Length: {}", version.len()),
                        "",
                        version,
                    ]
                    .join("\r\n")
                };
                let _ = flux.write_all(reponse.as_bytes());
            }
        });
        (adresse, compte)
    }

    const UN: &str = "BEGIN:VCALENDAR
X-WR-CALNAME:Club
BEGIN:VEVENT
UID:a
DTSTART:20260928T090000Z
DTEND:20260928T100000Z
SUMMARY:Entraînement
END:VEVENT
END:VCALENDAR
";
    const DEUX: &str = "BEGIN:VCALENDAR
X-WR-CALNAME:Club
BEGIN:VEVENT
UID:a
DTSTART:20260928T090000Z
DTEND:20260928T100000Z
SUMMARY:Entraînement
END:VEVENT
BEGIN:VEVENT
UID:b
DTSTART:20261005T090000Z
DTEND:20261005T100000Z
SUMMARY:Match
END:VEVENT
END:VCALENDAR
";

    fn services_de_test() -> (Services, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("maitre")),
        )
        .unwrap();
        (s, dir)
    }

    #[test]
    fn an_event_dragged_moves_or_stretches_and_a_task_slot_takes_its_task_along() {
        let (s, _dir) = services_de_test();
        let perso = s.store.calendars().unwrap()[0].id;
        let lundi = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let a = |h: u32, m: u32| {
            iris_calendar::time::zoned_millis(lundi.and_hms_opt(h, m, 0).unwrap(), &Local)
        };
        let nouveau = |uid: &str, rrule: Option<&str>| NewEvent {
            uid: uid.into(),
            summary: "Atelier".into(),
            start_ms: a(9, 0),
            end_ms: a(10, 0),
            rrule: rrule.map(str::to_string),
            ..Default::default()
        };
        let id = s
            .store
            .insert_event(perso, &nouveau("a@iris", None), now())
            .unwrap();
        let cle = format!("{id}:{}", a(9, 0));

        // A day later and half an hour on: the hour kept, its length too.
        deplacer(&s, &cle, 1, 30, false).unwrap();
        let e = s.store.event(id).unwrap().unwrap().event;
        let mardi = |h: u32, m: u32| {
            iris_calendar::time::zoned_millis(
                (lundi + Duration::days(1)).and_hms_opt(h, m, 0).unwrap(),
                &Local,
            )
        };
        assert_eq!((e.start_ms, e.end_ms), (mardi(9, 30), mardi(10, 30)));

        // Stretched by 45 minutes; then pulled up past its start, it keeps 15.
        deplacer(&s, &cle, 0, 45, true).unwrap();
        assert_eq!(
            s.store.event(id).unwrap().unwrap().event.end_ms,
            mardi(11, 15)
        );
        deplacer(&s, &cle, 0, -600, true).unwrap();
        assert_eq!(
            s.store.event(id).unwrap().unwrap().event.end_ms,
            mardi(9, 45)
        );

        // A repeating one stays where it is.
        let r = s
            .store
            .insert_event(perso, &nouveau("r@iris", Some("FREQ=WEEKLY")), now())
            .unwrap();
        deplacer(&s, &format!("{r}:{}", a(9, 0)), 1, 0, false).unwrap();
        assert_eq!(s.store.event(r).unwrap().unwrap().event.start_ms, a(9, 0));

        // A task's slot moved: the task's day and hour follow.
        let liste = s.store.task_lists().unwrap()[0].id;
        let tache = s
            .store
            .insert_task(
                &iris_store::NewTask {
                    list_id: liste,
                    title: "Relire le devis".into(),
                    due_day: Some("2026-10-05".into()),
                    ..Default::default()
                },
                now(),
            )
            .unwrap();
        crate::tasks::book(&s, tache, lundi, 14 * 60).unwrap();
        let creneau = s
            .store
            .find_event(perso, &format!("task-{tache}@iris"), None)
            .unwrap()
            .unwrap();
        let debut = s.store.event(creneau).unwrap().unwrap().event.start_ms;
        deplacer(&s, &format!("{creneau}:{debut}"), 1, 60, false).unwrap();
        let t = s.store.task(tache).unwrap().unwrap().task;
        assert_eq!(t.due_day.as_deref(), Some("2026-10-06"));
        assert_eq!(t.due_minute, Some(15 * 60));
    }

    #[tokio::test]
    async fn un_abonnement_se_lit_se_nomme_et_se_met_a_jour() {
        let (s, _dir) = services_de_test();
        // 1re lecture : UN ; 2e : même ETag, donc 304 ; 3e : DEUX, nouvel ETag.
        let (url, compte) = serveur(vec![UN, UN, DEUX]);

        let (id, n) = subscribe(&s, &url, "").await.unwrap();
        assert_eq!(n, 1);
        let cal = s.store.calendar(id).unwrap().unwrap();
        assert_eq!(cal.name, "Club", "le nom vient du calendrier");
        assert!(cal.etag.is_some());

        // Rien n'a changé : le serveur répond 304 et les événements restent.
        assert_eq!(refresh_subscription(&s, id).await.unwrap(), 1);
        // Le calendrier a grandi.
        assert_eq!(refresh_subscription(&s, id).await.unwrap(), 2);
        assert_eq!(s.store.calendar_event_count(id).unwrap(), 2);
        assert_eq!(compte.load(std::sync::atomic::Ordering::SeqCst), 3);

        // Deux fois le même lien : refusé.
        assert!(subscribe(&s, &url, "").await.is_err());
    }

    #[test]
    fn une_invitation_s_ajoute_puis_se_met_a_jour_puis_s_annule() {
        let (s, _dir) = services_de_test();
        let invitation = |debut: &str, statut: &str| {
            [
                "BEGIN:VCALENDAR",
                "METHOD:REQUEST",
                "BEGIN:VEVENT",
                "UID:reunion-42@example.com",
                &format!("DTSTART:{debut}"),
                "DTEND:20261006T100000Z",
                "SUMMARY:Revue budgétaire",
                &format!("STATUS:{statut}"),
                "END:VEVENT",
                "END:VCALENDAR",
                "",
            ]
            .join("\r\n")
        };
        let perso = s.store.calendars().unwrap()[0].id;

        let b = import_ics(&s, &invitation("20261006T090000Z", "CONFIRMED")).unwrap();
        assert_eq!((b.added, b.updated), (1, 0));
        assert_eq!(b.message(), "Added to your calendar: Revue budgétaire.");
        assert_eq!(b.first_day.as_deref(), Some("2026-10-06"));

        // L'organisateur la déplace : la même, pas une seconde.
        let b = import_ics(&s, &invitation("20261006T083000Z", "CONFIRMED")).unwrap();
        assert_eq!((b.added, b.updated), (0, 1));
        assert_eq!(s.store.calendar_event_count(perso).unwrap(), 1);

        // Puis l'annule : elle disparaît de l'agenda.
        let b = import_ics(&s, &invitation("20261006T083000Z", "CANCELLED")).unwrap();
        assert_eq!(b.cancelled, 1);
        assert!(s
            .store
            .events_for_range(0, i64::MAX / 2)
            .unwrap()
            .iter()
            .all(|e| e.event.cancelled));
    }

    #[tokio::test]
    async fn une_page_qui_n_est_pas_un_agenda_ne_cree_rien() {
        let (s, _dir) = services_de_test();
        let (url, _) = serveur(vec!["<html>Not a calendar</html>"]);
        assert!(subscribe(&s, &url, "").await.is_err());
        assert_eq!(s.store.calendars().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn un_lien_qui_n_est_pas_un_lien_est_refuse_sans_rien_creer() {
        let dir = tempfile::tempdir().unwrap();
        let s = Services::open(
            crate::paths::Paths::under(dir.path()),
            Some(iris_secrets::Secret::new("maitre")),
        )
        .unwrap();
        assert!(subscribe(&s, "file:///C:/x.ics", "").await.is_err());
        assert_eq!(
            s.store.calendars().unwrap().len(),
            1,
            "seul Personal existe"
        );
    }
}
