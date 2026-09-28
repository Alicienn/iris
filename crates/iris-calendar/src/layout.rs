//! Disposer des occurrences à l'écran.
//!
//! Tout est en **jours locaux** : ceux du fuseau de la personne qui regarde, passé en
//! paramètre (l'application passe celui de la machine, les tests un fuseau fixe).

use crate::Occurrence;
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc, Weekday};

/// Minuit local d'un jour, en millisecondes UTC.
pub fn local_midnight<Z: TimeZone>(date: NaiveDate, tz: &Z) -> i64 {
    crate::time::zoned_millis(date.and_time(NaiveTime::MIN), tz)
}

/// Le jour local d'un instant.
pub fn local_date<Z: TimeZone>(ms: i64, tz: &Z) -> NaiveDate {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_default()
        .with_timezone(tz)
        .date_naive()
}

/// Les minutes écoulées depuis minuit local.
pub fn local_minutes<Z: TimeZone>(ms: i64, tz: &Z) -> i32 {
    let t = Utc
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_default()
        .with_timezone(tz);
    let heure = t.time();
    (chrono::Timelike::hour(&heure) * 60 + chrono::Timelike::minute(&heure)) as i32
}

/// Le lundi de la semaine d'un jour.
pub fn week_start(date: NaiveDate) -> NaiveDate {
    date - Duration::days(date.weekday().num_days_from_monday() as i64)
}

/// Les quarante-deux jours d'une grille de mois, du lundi qui précède le 1er.
///
/// Six semaines toujours, même pour un février de quatre : une grille dont la hauteur
/// change d'un mois à l'autre fait sauter tout ce qui est dessous quand on navigue.
pub fn month_days(year: i32, month: u32) -> Vec<NaiveDate> {
    let premier = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
    let debut = week_start(premier);
    (0..42).map(|i| debut + Duration::days(i)).collect()
}

/// Les jours locaux qu'une occurrence couvre.
///
/// Un jour entier se lit dans ses propres dates — il est posé à minuit UTC et ne doit
/// pas glisser d'un jour pour quelqu'un à l'ouest de Greenwich. Un événement à heure
/// va du jour local de son début à celui de sa dernière milliseconde.
pub fn days_covered<Z: TimeZone>(o: &Occurrence, tz: &Z) -> (NaiveDate, NaiveDate) {
    if o.all_day {
        let debut = local_date(o.start, &Utc);
        let fin = local_date((o.end - 1).max(o.start), &Utc);
        (debut, fin)
    } else {
        let debut = local_date(o.start, tz);
        let fin = local_date((o.end - 1).max(o.start), tz);
        (debut, fin.max(debut))
    }
}

/// Une case de la grille du mois.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayCell {
    pub date: NaiveDate,
    pub in_month: bool,
    /// Les occurrences du jour, jours entiers d'abord, puis par heure de début : des
    /// indices dans la liste fournie.
    pub items: Vec<usize>,
}

/// La grille d'un mois.
pub fn month_grid<Z: TimeZone>(year: i32, month: u32, occ: &[Occurrence], tz: &Z) -> Vec<DayCell> {
    let jours = month_days(year, month);
    let premier = jours[0];
    let mut cases: Vec<DayCell> = jours
        .iter()
        .map(|d| DayCell {
            date: *d,
            in_month: d.month() == month,
            items: Vec::new(),
        })
        .collect();
    for (i, o) in occ.iter().enumerate() {
        let (debut, fin) = days_covered(o, tz);
        let mut jour = debut;
        while jour <= fin {
            let decalage = (jour - premier).num_days();
            if (0..42).contains(&decalage) {
                cases[decalage as usize].items.push(i);
            }
            jour += Duration::days(1);
            if (jour - debut).num_days() > 42 {
                break;
            }
        }
    }
    for c in &mut cases {
        c.items.sort_by_key(|i| (!occ[*i].all_day, occ[*i].start));
    }
    cases
}

/// Un bloc à heure dans une colonne de jour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedBlock {
    pub occurrence: usize,
    /// La colonne : l'indice du jour dans ceux fournis.
    pub day: usize,
    /// De minuit local, dans ce jour : un événement qui commence la veille démarre à 0.
    pub start_min: i32,
    pub end_min: i32,
    /// Sa voie parmi les événements qui se chevauchent, et combien il y en a : deux
    /// réunions simultanées se partagent la largeur au lieu de se recouvrir.
    pub lane: usize,
    pub lanes: usize,
}

/// Les colonnes d'une semaine (ou d'un jour) : les jours entiers de chaque jour, et
/// les blocs à heure.
pub fn week_layout<Z: TimeZone>(
    days: &[NaiveDate],
    occ: &[Occurrence],
    tz: &Z,
) -> (Vec<Vec<usize>>, Vec<TimedBlock>) {
    let mut jours_entiers = vec![Vec::new(); days.len()];
    let mut blocs: Vec<TimedBlock> = Vec::new();

    for (i, o) in occ.iter().enumerate() {
        let (debut, fin) = days_covered(o, tz);
        for (colonne, jour) in days.iter().enumerate() {
            if *jour < debut || *jour > fin {
                continue;
            }
            if o.all_day {
                jours_entiers[colonne].push(i);
                continue;
            }
            let minuit = local_midnight(*jour, tz);
            let lendemain = local_midnight(*jour + Duration::days(1), tz);
            let a = o.start.max(minuit);
            let b = o.end.min(lendemain).max(a);
            let start_min = ((a - minuit) / 60_000) as i32;
            let end_min = (((b - minuit) / 60_000) as i32).max(start_min);
            blocs.push(TimedBlock {
                occurrence: i,
                day: colonne,
                start_min,
                end_min,
                lane: 0,
                lanes: 1,
            });
        }
    }

    // Les voies, jour par jour : une grappe est une suite d'événements qui se
    // chevauchent de proche en proche ; chacun prend la première voie libre, et toute
    // la grappe se partage le nombre de voies qu'elle a ouvertes.
    for colonne in 0..days.len() {
        let mut indices: Vec<usize> = (0..blocs.len())
            .filter(|i| blocs[*i].day == colonne)
            .collect();
        // Une durée affichée d'au moins un quart d'heure, pour le chevauchement comme
        // pour le dessin : deux événements de zéro minute à la même heure se gênent.
        let fin_vue = |b: &TimedBlock| b.end_min.max(b.start_min + 15);
        indices.sort_by_key(|i| (blocs[*i].start_min, -blocs[*i].end_min));
        let mut grappe: Vec<usize> = Vec::new();
        let mut voies: Vec<i32> = Vec::new();
        let mut fin_grappe = i32::MIN;
        let fermer =
            |grappe: &mut Vec<usize>, voies: &mut Vec<i32>, blocs: &mut Vec<TimedBlock>| {
                let n = voies.len().max(1);
                for i in grappe.drain(..) {
                    blocs[i].lanes = n;
                }
                voies.clear();
            };
        for i in indices {
            let (debut, fin) = (blocs[i].start_min, fin_vue(&blocs[i]));
            if debut >= fin_grappe && !grappe.is_empty() {
                fermer(&mut grappe, &mut voies, &mut blocs);
            }
            let voie = match voies.iter().position(|f| *f <= debut) {
                Some(v) => {
                    voies[v] = fin;
                    v
                }
                None => {
                    voies.push(fin);
                    voies.len() - 1
                }
            };
            blocs[i].lane = voie;
            grappe.push(i);
            fin_grappe = if grappe.len() == 1 {
                fin
            } else {
                fin_grappe.max(fin)
            };
        }
        fermer(&mut grappe, &mut voies, &mut blocs);
    }
    (jours_entiers, blocs)
}

/// Les noms courts des jours, du lundi au dimanche.
pub const WEEKDAYS: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Paris;

    fn d(y: i32, m: u32, j: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, j).unwrap()
    }

    fn a_heure(jour: NaiveDate, h: u32, m: u32, minutes: i64) -> Occurrence {
        let debut = crate::time::zoned_millis(jour.and_hms_opt(h, m, 0).unwrap(), &Paris);
        Occurrence {
            event: 0,
            start: debut,
            end: debut + minutes * 60_000,
            all_day: false,
        }
    }

    #[test]
    fn la_grille_du_mois_commence_un_lundi_et_compte_six_semaines() {
        let jours = month_days(2026, 9);
        assert_eq!(jours.len(), 42);
        assert_eq!(
            jours[0],
            d(2026, 8, 31),
            "le 1er septembre 2026 est un mardi"
        );
        assert_eq!(jours[0].weekday(), Weekday::Mon);
    }

    #[test]
    fn un_evenement_de_plusieurs_jours_apparait_chaque_jour() {
        let debut = local_midnight(d(2026, 9, 10), &Utc);
        let o = vec![Occurrence {
            event: 0,
            start: debut,
            end: debut + 3 * 86_400_000,
            all_day: true,
        }];
        let grille = month_grid(2026, 9, &o, &Paris);
        let jours: Vec<u32> = grille
            .iter()
            .filter(|c| !c.items.is_empty())
            .map(|c| c.date.day())
            .collect();
        assert_eq!(jours, vec![10, 11, 12], "fin exclusive : pas le 13");
    }

    #[test]
    fn un_jour_entier_ne_glisse_pas_a_l_ouest_de_greenwich() {
        let debut = local_midnight(d(2026, 9, 10), &Utc);
        let o = Occurrence {
            event: 0,
            start: debut,
            end: debut + 86_400_000,
            all_day: true,
        };
        assert_eq!(
            days_covered(&o, &chrono_tz::America::Los_Angeles),
            (d(2026, 9, 10), d(2026, 9, 10))
        );
    }

    #[test]
    fn une_reunion_de_nuit_est_coupee_a_minuit() {
        let jours = [d(2026, 9, 10), d(2026, 9, 11)];
        let o = vec![a_heure(d(2026, 9, 10), 23, 0, 120)];
        let (_, blocs) = week_layout(&jours, &o, &Paris);
        assert_eq!(blocs.len(), 2);
        assert_eq!(
            (blocs[0].day, blocs[0].start_min, blocs[0].end_min),
            (0, 23 * 60, 24 * 60)
        );
        assert_eq!(
            (blocs[1].day, blocs[1].start_min, blocs[1].end_min),
            (1, 0, 60)
        );
    }

    #[test]
    fn deux_reunions_simultanees_se_partagent_la_largeur() {
        let jour = d(2026, 9, 10);
        let o = vec![
            a_heure(jour, 9, 0, 60),
            a_heure(jour, 9, 30, 60),
            a_heure(jour, 14, 0, 30),
        ];
        let (_, blocs) = week_layout(&[jour], &o, &Paris);
        let par = |i: usize| blocs.iter().find(|b| b.occurrence == i).unwrap();
        assert_eq!((par(0).lane, par(0).lanes), (0, 2));
        assert_eq!((par(1).lane, par(1).lanes), (1, 2));
        assert_eq!(
            (par(2).lane, par(2).lanes),
            (0, 1),
            "l'après-midi a toute la largeur"
        );
    }

    #[test]
    fn les_jours_entiers_sont_ranges_a_part() {
        let jour = d(2026, 9, 10);
        let debut = local_midnight(jour, &Utc);
        let o = vec![
            a_heure(jour, 9, 0, 60),
            Occurrence {
                event: 1,
                start: debut,
                end: debut + 86_400_000,
                all_day: true,
            },
        ];
        let (entiers, blocs) = week_layout(&[jour], &o, &Paris);
        assert_eq!(entiers[0], vec![1]);
        assert_eq!(blocs.len(), 1);
    }

    #[test]
    fn dans_une_case_les_jours_entiers_passent_devant() {
        let jour = d(2026, 9, 10);
        let debut = local_midnight(jour, &Utc);
        let o = vec![
            a_heure(jour, 9, 0, 60),
            Occurrence {
                event: 1,
                start: debut,
                end: debut + 86_400_000,
                all_day: true,
            },
        ];
        let grille = month_grid(2026, 9, &o, &Paris);
        let case = grille.iter().find(|c| c.date == jour).unwrap();
        assert_eq!(case.items, vec![1, 0]);
    }
}
