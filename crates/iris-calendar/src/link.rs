//! Les liens d'abonnement.

/// Ce qu'on accepte d'appeler un lien d'agenda.
///
/// `webcal://` et `webcals://` sont des `https://` qui se présentent comme des
/// agendas : c'est la forme que publient Google, Outlook et iCloud. Un lien `http://`
/// est accepté tel quel — certains serveurs d'école ou d'association n'ont rien
/// d'autre — et tout le reste est refusé : un agenda ne se lit pas sur le disque.
pub fn normalize(url: &str) -> Result<String, String> {
    let u = url.trim();
    if u.is_empty() {
        return Err("Paste the calendar's link.".into());
    }
    let bas = u.to_ascii_lowercase();
    let normal = if let Some(reste) = bas
        .strip_prefix("webcals://")
        .or_else(|| bas.strip_prefix("webcal://"))
    {
        let longueur = u.len() - reste.len();
        format!("https://{}", &u[longueur..])
    } else if bas.starts_with("https://") || bas.starts_with("http://") {
        u.to_string()
    } else if !bas.contains("://") && bas.contains('.') {
        format!("https://{u}")
    } else {
        return Err("That is not a web link. It should start with https:// or webcal://.".into());
    };
    if normal.contains(char::is_whitespace) {
        return Err("A link has no spaces in it.".into());
    }
    Ok(normal)
}

/// Un nom par défaut pour un abonnement qui n'en déclare pas : le site.
pub fn default_name(url: &str) -> String {
    let sans_schema = url.split("://").nth(1).unwrap_or(url);
    let hote = sans_schema
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(sans_schema);
    let hote = hote.strip_prefix("www.").unwrap_or(hote);
    if hote.is_empty() {
        "Calendar".into()
    } else {
        hote.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcal_devient_https() {
        assert_eq!(
            normalize("webcal://p01-caldav.icloud.com/published/2/ABC").unwrap(),
            "https://p01-caldav.icloud.com/published/2/ABC"
        );
        assert_eq!(
            normalize("WEBCALS://x.example.com/a.ics").unwrap(),
            "https://x.example.com/a.ics"
        );
    }

    #[test]
    fn les_liens_web_passent_et_le_reste_est_refuse() {
        assert!(normalize("https://calendar.google.com/calendar/ical/x/basic.ics").is_ok());
        assert!(normalize("http://ecole.example.fr/agenda.ics").is_ok());
        assert_eq!(
            normalize("example.com/a.ics").unwrap(),
            "https://example.com/a.ics"
        );
        assert!(normalize("file:///C:/a.ics").is_err());
        assert!(normalize("").is_err());
        assert!(normalize("https://a b.ics").is_err());
    }

    #[test]
    fn le_nom_par_defaut_est_le_site() {
        assert_eq!(
            default_name("https://www.example.com/cal.ics"),
            "example.com"
        );
    }
}
