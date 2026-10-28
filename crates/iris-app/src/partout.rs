//! Searching everything at once: the mail, the tasks, the events and the notes.
//!
//! On a phone the magnifier of Home opens a page of its own (the Android mockups' S1,
//! S2): before a key, what can be made; then, at each key, what was found, grouped by
//! place, the word found set apart in each title. A task is ticked from there; anything
//! else opens where it lives, through the same callbacks Home's widgets use.

use crate::services::{now, Services};
use chrono::{Local, TimeZone};
use iris_ui::{AppWindow, EverywhereHitData};
use slint::{ComponentHandle, ModelRc, VecModel};

/// How many of each place: a page a thumb runs down, not an archive.
const PAR_LIEU: usize = 5;

/// One thing found, before it is cut for the interface.
#[derive(Debug, Clone, PartialEq)]
pub struct Trouve {
    pub place: &'static str,
    pub kind: &'static str,
    pub id: String,
    pub title: String,
    pub detail: String,
    pub done: bool,
}

/// The title cut around the first word of the query found in it, ignoring case:
/// (before, the word as written, after). Not found: the whole title before.
pub fn decouper(titre: &str, requete: &str) -> (String, String, String) {
    let mot = requete.split_whitespace().next().unwrap_or("");
    if mot.is_empty() {
        return (titre.to_string(), String::new(), String::new());
    }
    let bas = titre.to_lowercase();
    let cherche = mot.to_lowercase();
    // Lowercasing can change lengths (a few scripts): cut only where both agree.
    if bas.len() == titre.len() {
        if let Some(i) = bas.find(&cherche) {
            let fin = i + cherche.len();
            if titre.is_char_boundary(i) && titre.is_char_boundary(fin) {
                return (
                    titre[..i].to_string(),
                    titre[i..fin].to_string(),
                    titre[fin..].to_string(),
                );
            }
        }
    }
    (titre.to_string(), String::new(), String::new())
}

/// Every word of the query is in the text, case and accents aside: "reunion" finds
/// "Réunion", as the notes' search does.
fn contient(texte: &str, requete: &str) -> bool {
    let plie = iris_notes::fuzzy::folded(texte);
    requete
        .split_whitespace()
        .all(|mot| plie.contains(&iris_notes::fuzzy::folded(mot)))
}

/// What the query finds, place by place, in the order the page shows them.
pub fn chercher(services: &Services, requete: &str) -> Vec<Trouve> {
    let requete = requete.trim();
    if requete.is_empty() {
        return Vec::new();
    }
    let mut trouves = Vec::new();

    // The mail: the index, by thread.
    if let Ok(hits) = services.index.search(requete, PAR_LIEU) {
        for hit in hits.into_iter().take(PAR_LIEU) {
            if let Ok(Some(fil)) = services.store.thread_row(hit.thread) {
                trouves.push(Trouve {
                    place: "Mail",
                    kind: "mail",
                    id: fil.id.get().to_string(),
                    title: if fil.subject.is_empty() {
                        "(no subject)".to_string()
                    } else {
                        fil.subject.clone()
                    },
                    detail: fil.from_display.clone(),
                    done: false,
                });
            }
        }
    }

    // The tasks: what is still to do first.
    if let Ok(mut taches) = services.store.all_tasks() {
        taches.retain(|t| t.task.parent_id.is_none() && contient(&t.task.title, requete));
        taches.sort_by_key(|t| t.is_done());
        for t in taches.into_iter().take(PAR_LIEU) {
            trouves.push(Trouve {
                place: "Tasks",
                kind: "task",
                id: t.id.to_string(),
                title: t.task.title.clone(),
                detail: t.task.due_day.clone().unwrap_or_default(),
                done: t.is_done(),
            });
        }
    }

    // The events: half a year back, a year ahead, the nearest first.
    let maintenant = now().millis();
    let jour = 86_400_000_i64;
    if let Ok(mut evenements) = services
        .store
        .events_for_range(maintenant - 182 * jour, maintenant + 365 * jour)
    {
        evenements.retain(|e| {
            contient(&e.event.summary, requete) || contient(&e.event.location, requete)
        });
        evenements.sort_by_key(|e| (e.event.start_ms - maintenant).abs());
        for e in evenements.into_iter().take(PAR_LIEU) {
            let quand = Local
                .timestamp_millis_opt(e.event.start_ms)
                .single()
                .map(|d| {
                    if e.event.all_day {
                        d.format("%a %-d %b").to_string()
                    } else {
                        d.format("%a %-d %b · %H:%M").to_string()
                    }
                })
                .unwrap_or_default();
            trouves.push(Trouve {
                place: "Events",
                kind: "event",
                id: format!("{}:{}", e.id, e.event.start_ms),
                title: e.event.summary.clone(),
                detail: quand,
                done: false,
            });
        }
    }

    // The notes of the space shown, with the line found.
    for (chemin, ligne) in crate::notes::search_words(requete, PAR_LIEU) {
        let nom = chemin
            .rsplit('/')
            .next()
            .unwrap_or(&chemin)
            .trim_end_matches(".md")
            .to_string();
        trouves.push(Trouve {
            place: "Notes",
            kind: "note",
            id: chemin,
            title: nom,
            detail: ligne,
            done: false,
        });
    }
    trouves
}

/// What the page shows: each hit cut around the word, the first of each place marked
/// so the page can title its group.
pub fn lignes(trouves: &[Trouve], requete: &str) -> Vec<EverywhereHitData> {
    trouves
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (avant, mot, apres) = decouper(&t.title, requete);
            EverywhereHitData {
                place: t.place.into(),
                kind: t.kind.into(),
                id: t.id.as_str().into(),
                before: avant.into(),
                word: mot.into(),
                after: apres.into(),
                detail: t.detail.as_str().into(),
                done: t.done,
                first: i == 0 || trouves[i - 1].place != t.place,
            }
        })
        .collect()
}

fn montrer(f: &AppWindow, services: &Services) {
    let q = f.get_everywhere_text();
    let trouves = chercher(services, &q);
    f.set_everywhere_hits(ModelRc::new(VecModel::from(lignes(&trouves, &q))));
}

/// Wires the page: a key searches again; a task's circle ticks it.
pub fn wire(f: &AppWindow, services: &Services) {
    {
        let (faible, services) = (f.as_weak(), services.clone());
        f.on_everywhere_query(move |_| {
            if let Some(f) = faible.upgrade() {
                montrer(&f, &services);
            }
        });
    }
    {
        let (faible, services) = (f.as_weak(), services.clone());
        f.on_everywhere_task_toggled(move |id| {
            let Some(f) = faible.upgrade() else { return };
            if let Ok(id) = id.parse::<i64>() {
                crate::tasks::toggle_done(&services, id);
            }
            montrer(&f, &services);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_mot_trouve_est_mis_a_part() {
        assert_eq!(
            decouper("Marie Martin: Quote", "marie"),
            ("".into(), "Marie".into(), " Martin: Quote".into())
        );
        assert_eq!(
            decouper("Call Marie", "MARIE plumber"),
            ("Call ".into(), "Marie".into(), "".into())
        );
        assert_eq!(
            decouper("Weekly plan", "devis"),
            ("Weekly plan".into(), "".into(), "".into())
        );
    }

    #[test]
    fn tous_les_mots_doivent_y_etre() {
        assert!(contient("Send the quote to Marie", "quote marie"));
        assert!(!contient("Send the quote to Marie", "quote paul"));
    }

    #[test]
    fn ni_la_casse_ni_les_accents_ne_comptent() {
        for requete in ["réu", "reunion", "RÉUNION", "Réunion de"] {
            assert!(contient("Réunion de jeudi", requete), "« {requete} »");
        }
    }
}
