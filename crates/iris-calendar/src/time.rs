//! Dates, fuseaux et durées du format iCalendar.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use std::str::FromStr;

/// Une date du fichier, avant conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// `VALUE=DATE` : un jour, sans heure.
    Date(NaiveDate),
    /// Suffixée `Z` : un instant absolu.
    Utc(DateTime<Utc>),
    /// Une heure locale dans un fuseau nommé.
    Zoned(NaiveDateTime, Tz),
    /// Une heure sans fuseau : celle de l'endroit où on la lit.
    Floating(NaiveDateTime),
}

impl Moment {
    /// En millisecondes UTC. Un jour est posé à minuit UTC ; une heure flottante est
    /// lue dans le fuseau de la machine.
    pub fn to_millis(self) -> i64 {
        match self {
            Moment::Date(d) => d.and_time(NaiveTime::MIN).and_utc().timestamp_millis(),
            Moment::Utc(t) => t.timestamp_millis(),
            Moment::Zoned(local, tz) => zoned_millis(local, &tz),
            Moment::Floating(local) => zoned_millis(local, &chrono::Local),
        }
    }

    pub fn is_date(self) -> bool {
        matches!(self, Moment::Date(_))
    }
}

/// Une heure locale d'un fuseau, en millisecondes UTC.
///
/// Une heure qui n'existe pas — 2 h 30 la nuit du passage à l'heure d'été — est prise
/// une heure plus tard ; une heure qui existe deux fois, à sa première occurrence.
/// C'est ce que font les agendas, et un rendez-vous déplacé d'une heure vaut mieux
/// qu'un rendez-vous perdu.
pub fn zoned_millis<Z: TimeZone>(local: NaiveDateTime, tz: &Z) -> i64 {
    match tz.from_local_datetime(&local) {
        chrono::LocalResult::Single(t) => t.timestamp_millis(),
        chrono::LocalResult::Ambiguous(a, _) => a.timestamp_millis(),
        chrono::LocalResult::None => tz
            .from_local_datetime(&(local + chrono::Duration::hours(1)))
            .earliest()
            .map(|t| t.timestamp_millis())
            .unwrap_or_else(|| local.and_utc().timestamp_millis()),
    }
}

/// Lit une valeur de date (`20260928`, `20260928T090000Z`, `20260928T090000`).
///
/// `tzid` est le paramètre `TZID` de la propriété, quand il y en a un ; `defaut` le
/// fuseau déclaré par le calendrier (`X-WR-TIMEZONE`), pour les heures flottantes.
pub fn parse_moment(
    value: &str,
    tzid: Option<&str>,
    date_only: bool,
    defaut: Option<Tz>,
) -> Option<Moment> {
    let v = value.trim();
    if date_only || (v.len() == 8 && !v.contains('T')) {
        return NaiveDate::parse_from_str(&v[..v.len().min(8)], "%Y%m%d")
            .ok()
            .map(Moment::Date);
    }
    let (corps, utc) = match v.strip_suffix('Z').or_else(|| v.strip_suffix('z')) {
        Some(c) => (c, true),
        None => (v, false),
    };
    let local = NaiveDateTime::parse_from_str(corps, "%Y%m%dT%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(corps, "%Y%m%dT%H%M"))
        .ok()?;
    if utc {
        return Some(Moment::Utc(local.and_utc()));
    }
    match tzid
        .and_then(resolve_tz)
        .or(if tzid.is_none() { defaut } else { None })
    {
        Some(tz) => Some(Moment::Zoned(local, tz)),
        None => Some(Moment::Floating(local)),
    }
}

/// Reconnaît un fuseau nommé.
///
/// Les noms IANA d'abord (`Europe/Paris`), puis les préfixes qu'ajoutent certains
/// logiciels (`/freeassociation.sourceforge.net/Europe/Paris`), puis les noms
/// Windows qu'écrit Outlook (`Romance Standard Time`). Un nom inconnu rend `None`, et
/// l'heure est lue comme flottante : dans le fuseau de la machine, ce qui est juste
/// dans la grande majorité des cas où c'est le même.
pub fn resolve_tz(nom: &str) -> Option<Tz> {
    let nom = nom.trim().trim_matches('"');
    if let Ok(tz) = Tz::from_str(nom) {
        return Some(tz);
    }
    // Un préfixe devant un nom IANA : on essaie les suffixes à deux et trois segments.
    let segments: Vec<&str> = nom.split('/').filter(|s| !s.is_empty()).collect();
    for n in [3usize, 2] {
        if segments.len() >= n {
            let suffixe = segments[segments.len() - n..].join("/");
            if let Ok(tz) = Tz::from_str(&suffixe) {
                return Some(tz);
            }
        }
    }
    let iana = match nom {
        "Romance Standard Time" => "Europe/Paris",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Central Europe Standard Time" => "Europe/Budapest",
        "Central European Standard Time" => "Europe/Warsaw",
        "GMT Standard Time" => "Europe/London",
        "Greenwich Standard Time" => "Atlantic/Reykjavik",
        "E. Europe Standard Time" => "Europe/Chisinau",
        "FLE Standard Time" => "Europe/Kiev",
        "GTB Standard Time" => "Europe/Bucharest",
        "Russian Standard Time" => "Europe/Moscow",
        "Eastern Standard Time" => "America/New_York",
        "Central Standard Time" => "America/Chicago",
        "Mountain Standard Time" => "America/Denver",
        "Pacific Standard Time" => "America/Los_Angeles",
        "Atlantic Standard Time" => "America/Halifax",
        "E. South America Standard Time" => "America/Sao_Paulo",
        "China Standard Time" => "Asia/Shanghai",
        "Tokyo Standard Time" => "Asia/Tokyo",
        "India Standard Time" => "Asia/Kolkata",
        "Arabian Standard Time" => "Asia/Dubai",
        "Singapore Standard Time" => "Asia/Singapore",
        "AUS Eastern Standard Time" => "Australia/Sydney",
        "Morocco Standard Time" => "Africa/Casablanca",
        "South Africa Standard Time" => "Africa/Johannesburg",
        "UTC" | "Coordinated Universal Time" | "GMT" => "UTC",
        _ => return None,
    };
    Tz::from_str(iana).ok()
}

/// Lit une durée iCalendar (`PT1H30M`, `P1D`, `-PT15M`, `P1W`), en millisecondes.
pub fn parse_duration(value: &str) -> Option<i64> {
    let v = value.trim();
    let (signe, v) = match v.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, v.strip_prefix('+').unwrap_or(v)),
    };
    let v = v.strip_prefix('P')?;
    let mut total: i64 = 0;
    let mut nombre = String::new();
    let mut dans_heure = false;
    for c in v.chars() {
        match c {
            'T' => dans_heure = true,
            '0'..='9' => nombre.push(c),
            _ => {
                let n: i64 = nombre.parse().ok()?;
                nombre.clear();
                total += n * match (c, dans_heure) {
                    ('W', _) => 7 * 86_400_000,
                    ('D', _) => 86_400_000,
                    ('H', true) => 3_600_000,
                    ('M', true) => 60_000,
                    ('S', true) => 1_000,
                    _ => return None,
                };
            }
        }
    }
    if !nombre.is_empty() {
        return None;
    }
    Some(signe * total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_trois_formes_de_date_se_lisent() {
        assert_eq!(
            parse_moment("20260928", None, false, None),
            Some(Moment::Date(NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()))
        );
        let utc = parse_moment("20260928T090000Z", None, false, None).unwrap();
        assert_eq!(utc.to_millis(), 1_790_586_000_000);
        let paris = parse_moment("20260928T110000", Some("Europe/Paris"), false, None).unwrap();
        assert_eq!(
            paris.to_millis(),
            utc.to_millis(),
            "11 h à Paris en été = 9 h UTC"
        );
    }

    #[test]
    fn un_fuseau_windows_ou_prefixe_est_reconnu() {
        assert_eq!(
            resolve_tz("Romance Standard Time"),
            Some(chrono_tz::Europe::Paris)
        );
        assert_eq!(
            resolve_tz("/freeassociation.sourceforge.net/Tzfile/Europe/Paris"),
            Some(chrono_tz::Europe::Paris)
        );
        assert_eq!(
            resolve_tz("America/Argentina/Buenos_Aires"),
            Some(chrono_tz::America::Argentina::Buenos_Aires)
        );
        assert_eq!(resolve_tz("Nowhere Standard Time"), None);
    }

    #[test]
    fn le_fuseau_par_defaut_ne_s_applique_qu_aux_heures_sans_fuseau() {
        let defaut = Some(chrono_tz::Asia::Tokyo);
        let m = parse_moment("20260928T090000", None, false, defaut).unwrap();
        assert!(matches!(m, Moment::Zoned(_, tz) if tz == chrono_tz::Asia::Tokyo));
    }

    #[test]
    fn une_heure_qui_n_existe_pas_est_decalee_plutot_que_perdue() {
        // Nuit du 29 mars 2026 à Paris : 2 h 30 n'existe pas.
        let t = parse_moment("20260329T023000", Some("Europe/Paris"), false, None).unwrap();
        let une_heure_plus_tard =
            parse_moment("20260329T033000", Some("Europe/Paris"), false, None).unwrap();
        assert_eq!(t.to_millis(), une_heure_plus_tard.to_millis());
    }

    #[test]
    fn les_durees_se_lisent() {
        assert_eq!(parse_duration("PT1H30M"), Some(5_400_000));
        assert_eq!(parse_duration("P1D"), Some(86_400_000));
        assert_eq!(parse_duration("P1W"), Some(604_800_000));
        assert_eq!(parse_duration("-PT15M"), Some(-900_000));
        assert_eq!(parse_duration("P1DT2H"), Some(93_600_000));
        assert_eq!(parse_duration("1H"), None);
    }
}
