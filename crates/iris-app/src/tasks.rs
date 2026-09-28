//! L'onglet des tâches : ce qu'il y a à faire, venu du courrier ou d'ailleurs.
//!
//! Quatre vues et les listes de l'utilisateur. « Today » rassemble ce qui compte
//! aujourd'hui, d'où qu'il vienne : les tâches dues ou en retard, les événements de
//! l'agenda, et les conversations qui attendent une action. Une conversation devient
//! une tâche d'une touche (`T`), et la tâche la rouvre d'un clic.

use crate::controller::{Controller, Request};
use crate::services::{now, Services};
use chrono::{Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use iris_store::{NewTask, StoredTask, TaskList};
use iris_tasks::{due_label, is_overdue, remind_at, section, Section, REMINDERS};
use iris_types::{ThreadId, WorkflowState};
use iris_ui::{AppWindow, SubtaskData, TaskDetailData, TaskPlaceData, TaskRowData};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Les couleurs données aux nouvelles listes, tour à tour.
const COULEURS: [&str; 8] = [
    "#5b8def", "#e0795b", "#4fb286", "#b67be6", "#e3b341", "#e0608c", "#3fb1c9", "#8a9a5b",
];

/// Combien de conversations « à faire » la vue Today montre.
const COURRIER_DU_JOUR: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vue {
    Today,
    Upcoming,
    Anytime,
    Mail,
    List(i64),
}

impl Vue {
    fn cle(self) -> String {
        match self {
            Vue::Today => "today".into(),
            Vue::Upcoming => "upcoming".into(),
            Vue::Anytime => "anytime".into(),
            Vue::Mail => "mail".into(),
            Vue::List(id) => format!("list:{id}"),
        }
    }

    fn depuis(cle: &str) -> Option<Vue> {
        Some(match cle {
            "today" => Vue::Today,
            "upcoming" => Vue::Upcoming,
            "anytime" => Vue::Anytime,
            "mail" => Vue::Mail,
            autre => Vue::List(autre.strip_prefix("list:")?.parse().ok()?),
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
        Vue::Anytime => "Anytime".into(),
        Vue::Mail => "From mail".into(),
        Vue::List(id) => listes
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.name.clone())
            .unwrap_or_default(),
    }
}

fn indication(vue: Vue) -> &'static str {
    match vue {
        Vue::Today => "Add a task for today — e.g. “Call Marie at 3pm !!”",
        _ => "Add a task — e.g. “tomorrow 9am Send the quote #Work”",
    }
}

fn section_ligne(titre: &str, compte: usize, rouge: bool) -> TaskRowData {
    TaskRowData {
        kind: 1,
        title: titre.to_uppercase().into(),
        meta: if compte > 0 {
            compte.to_string().into()
        } else {
            SharedString::default()
        },
        overdue: rouge,
        ..Default::default()
    }
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
            format!("{faites}/{total}").into()
        } else {
            SharedString::default()
        },
        selected: etat.choisie == Some(t.id),
    }
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

    let ouvertes = services.store.open_tasks().unwrap_or_default();
    let dessus: Vec<&StoredTask> = ouvertes
        .iter()
        .filter(|t| t.task.parent_id.is_none())
        .collect();
    let du_jour = |t: &StoredTask| jour(&t.task).is_some_and(|j| j <= today);
    let a_venir =
        |t: &StoredTask| jour(&t.task).is_some_and(|j| j > today && j <= today + Duration::days(7));

    // --- La colonne de gauche.
    let mut lieux = vec![
        (
            Vue::Today,
            "Today",
            iris_ui_icone::SOLEIL,
            dessus.iter().filter(|t| du_jour(t)).count(),
        ),
        (
            Vue::Upcoming,
            "Upcoming",
            iris_ui_icone::AGENDA,
            dessus.iter().filter(|t| a_venir(t)).count(),
        ),
        (
            Vue::Anytime,
            "Anytime",
            iris_ui_icone::LISTE,
            dessus.iter().filter(|t| t.task.due_day.is_none()).count(),
        ),
        (
            Vue::Mail,
            "From mail",
            iris_ui_icone::COURRIER,
            dessus.iter().filter(|t| t.task.thread_id.is_some()).count(),
        ),
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

    match etat.vue {
        Vue::Today => {
            let taches: Vec<&StoredTask> = dessus.iter().copied().filter(|t| du_jour(t)).collect();
            par_sections(taches, true, &mut lignes);

            let evenements = crate::calendar::events_on(services, today);
            if !evenements.is_empty() {
                lignes.push(section_ligne("Calendar", evenements.len(), false));
                for (debut, entier, titre, couleur) in evenements {
                    let heure = if entier {
                        "All day".to_string()
                    } else {
                        Local
                            .timestamp_millis_opt(debut)
                            .single()
                            .map(|t| t.format("%H:%M").to_string())
                            .unwrap_or_default()
                    };
                    lignes.push(TaskRowData {
                        kind: 2,
                        key: today.format("%Y-%m-%d").to_string().into(),
                        title: titre.into(),
                        meta: heure.into(),
                        color: couleur,
                        ..Default::default()
                    });
                }
            }

            let courrier = services
                .store
                .list_threads(&iris_store::ListQuery {
                    state: WorkflowState::Todo,
                    accounts: Vec::new(),
                    hide_snoozed_until: Some(now()),
                    limit: COURRIER_DU_JOUR,
                    after: None,
                    scope: iris_store::Scope::Queue,
                    filters: iris_store::Filters::default(),
                })
                .unwrap_or_default();
            if !courrier.is_empty() {
                lignes.push(section_ligne("Mail to do", courrier.len(), false));
                for c in courrier {
                    lignes.push(TaskRowData {
                        kind: 3,
                        id: c.id.0 as i32,
                        title: if c.subject.trim().is_empty() {
                            "(No subject)".into()
                        } else {
                            c.subject.as_str().into()
                        },
                        meta: c.from_display.as_str().into(),
                        ..Default::default()
                    });
                }
            }
        }
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
        Vue::Anytime => {
            for t in dessus.iter().filter(|t| t.task.due_day.is_none()) {
                lignes.push(ligne_tache(services, t, etat, true, maintenant));
            }
        }
        Vue::Mail => {
            let taches: Vec<&StoredTask> = dessus
                .iter()
                .copied()
                .filter(|t| t.task.thread_id.is_some())
                .collect();
            par_sections(taches, true, &mut lignes);
        }
        Vue::List(id) => {
            let taches: Vec<&StoredTask> = dessus
                .iter()
                .copied()
                .filter(|t| t.task.list_id == id)
                .collect();
            par_sections(taches, false, &mut lignes);
            let faites = services.store.done_tasks(Some(id), 20).unwrap_or_default();
            if !faites.is_empty() {
                lignes.push(section_ligne("Completed", faites.len(), false));
                for t in &faites {
                    lignes.push(ligne_tache(services, t, etat, false, maintenant));
                }
            }
        }
    }

    etat.ordre = lignes
        .iter()
        .filter(|l| l.kind == 0)
        .map(|l| l.id as i64)
        .collect();
    f.set_tasks_title(titre_vue(etat.vue, &etat.listes).into());
    f.set_task_add_hint(indication(etat.vue).into());
    f.set_task_rows(ModelRc::new(VecModel::from(lignes)));
    remplir_detail(f, services, etat, maintenant);
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
    let (libelle, retard) = match jour(&t.task) {
        Some(j) => (
            due_label(j, minute(&t.task), maintenant.date()),
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
        source: t.task.source.as_str().into(),
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
        ..Default::default()
    };
    recalculer_rappel(&mut t);
    let id = services.store.insert_task(&t, now()).ok()?;

    let ou = match echeance {
        Some((j, m)) => format!("Added — due {}.", due_label(j, m, today)),
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
    if let Ok(Some(t)) = services.store.task(id) {
        let _ = services
            .store
            .set_task_done(id, if t.is_done() { None } else { Some(now()) });
    }
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
        source: format!("{} — {}", fil.from_display, sujet),
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
    }));
    f.set_task_reminders(ModelRc::new(VecModel::from(
        REMINDERS
            .iter()
            .map(|(n, _)| SharedString::from(*n))
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

    geste!(
        on_task_add,
        [services, etat, redessiner, f, controller],
        |texte| {
            let resultat = ajouter(services, &mut etat.borrow_mut(), &texte);
            if let Some((_, message)) = resultat {
                f.set_task_add_text(SharedString::default());
                f.set_status(message.into());
            }
            redessiner();
        }
    );

    geste!(
        on_task_row_selected,
        [services, etat, redessiner, f, controller],
        |ligne| {
            match ligne.kind {
                0 => {
                    let mut e = etat.borrow_mut();
                    if e.choisie != Some(ligne.id as i64) {
                        e.choisie = Some(ligne.id as i64);
                        e.mois = None;
                    }
                    drop(e);
                    redessiner();
                }
                // Un événement : l'agenda, sur ce jour.
                2 => {
                    f.set_workspace(1);
                    f.invoke_workspace_changed(1);
                    f.invoke_calendar_day_opened(ligne.key.clone());
                }
                // Une conversation : le courrier, sur elle.
                3 => ouvrir_fil(f, services, controller, ligne.id as i64),
                _ => {}
            }
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
            let _ = services.store.delete_task(id as i64);
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
                ouvrir_fil(f, services, controller, fil);
            }
        }
    );

    geste!(
        on_task_delete,
        [services, etat, redessiner, f, controller],
        || {
            let mut e = etat.borrow_mut();
            let Some(id) = e.choisie else { return };
            let titre = services
                .store
                .task(id)
                .ok()
                .flatten()
                .map(|t| t.task.title)
                .unwrap_or_default();
            // La sélection passe à la suivante, pour enchaîner au clavier.
            let i = e.ordre.iter().position(|x| *x == id);
            e.choisie = i
                .and_then(|i| {
                    e.ordre
                        .get(i + 1)
                        .or_else(|| i.checked_sub(1).and_then(|j| e.ordre.get(j)))
                })
                .copied();
            e.mois = None;
            drop(e);
            let _ = services.store.delete_task(id);
            f.set_status(format!("Task deleted: {titre}").into());
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
fn ouvrir_fil(f: &AppWindow, services: &Services, controller: &Controller, fil: i64) {
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
        ] {
            assert_eq!(Vue::depuis(&v.cle()), Some(v));
        }
        assert_eq!(Vue::depuis("list:x"), None);
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
