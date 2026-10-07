//! WebDAV's answers (`207 Multi-Status`), read into what CalDAV needs, and the
//! requests that ask for them.

/// The namespaces met in CalDAV.
pub const DAV: &str = "DAV:";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const CS: &str = "http://calendarserver.org/ns/";
pub const APPLE: &str = "http://apple.com/ns/ical/";

/// What one `<response>` says about one resource: only what was found (`200`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Props {
    /// The resource, as the server wrote it (often a path).
    pub href: String,
    /// False when the response itself says the resource is not there (a `404` in a
    /// multiget).
    pub found: bool,
    pub is_calendar: bool,
    pub name: Option<String>,
    pub color: Option<String>,
    pub ctag: Option<String>,
    pub etag: Option<String>,
    pub calendar_data: Option<String>,
    pub principal: Option<String>,
    pub home_set: Option<String>,
    /// The components a calendar takes (`VEVENT`, `VTODO`); empty when not said.
    pub components: Vec<String>,
    /// What the user may do there, when the server says (`read`, `write`, `all`…).
    pub privileges: Option<Vec<String>>,
}

impl Props {
    /// Whether events can be written there: unless the server says otherwise.
    pub fn writable(&self) -> bool {
        self.privileges.as_ref().is_none_or(|p| {
            p.iter()
                .any(|x| matches!(x.as_str(), "write" | "write-content" | "all" | "bind"))
        })
    }

    /// Whether it holds events: unless it says it takes other things only.
    pub fn takes_events(&self) -> bool {
        self.components.is_empty() || self.components.iter().any(|c| c == "VEVENT")
    }
}

/// Reads a `207 Multi-Status` body.
pub fn multistatus(xml: &str) -> Result<Vec<Props>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("unreadable answer ({e})"))?;
    let mut sortie = Vec::new();
    for reponse in doc
        .descendants()
        .filter(|n| n.has_tag_name((DAV, "response")))
    {
        let mut p = Props {
            found: true,
            ..Default::default()
        };
        for enfant in reponse.children().filter(|n| n.is_element()) {
            if enfant.has_tag_name((DAV, "href")) {
                p.href = texte(enfant).trim().to_string();
            } else if enfant.has_tag_name((DAV, "status")) {
                p.found = status_ok(&texte(enfant));
            } else if enfant.has_tag_name((DAV, "propstat")) {
                let ok = enfant
                    .children()
                    .find(|n| n.has_tag_name((DAV, "status")))
                    .is_none_or(|s| status_ok(&texte(s)));
                if !ok {
                    continue;
                }
                for prop in enfant
                    .children()
                    .filter(|n| n.has_tag_name((DAV, "prop")))
                    .flat_map(|n| n.children().filter(|c| c.is_element()))
                {
                    lire(prop, &mut p);
                }
            }
        }
        sortie.push(p);
    }
    Ok(sortie)
}

fn lire(prop: roxmltree::Node<'_, '_>, p: &mut Props) {
    let nom = (
        prop.tag_name().namespace().unwrap_or(""),
        prop.tag_name().name(),
    );
    let valeur = || Some(texte(prop).trim().to_string()).filter(|v| !v.is_empty());
    let href = || {
        prop.children()
            .find(|n| n.has_tag_name((DAV, "href")))
            .map(|h| texte(h).trim().to_string())
            .filter(|v| !v.is_empty())
    };
    match nom {
        (DAV, "resourcetype") => {
            p.is_calendar = prop
                .children()
                .any(|n| n.has_tag_name((CALDAV, "calendar")));
        }
        (DAV, "displayname") => p.name = valeur(),
        (APPLE, "calendar-color") => p.color = valeur(),
        (CS, "getctag") => p.ctag = valeur(),
        (DAV, "getetag") => p.etag = valeur(),
        (CALDAV, "calendar-data") => p.calendar_data = Some(texte(prop)),
        (DAV, "current-user-principal") => p.principal = href(),
        (CALDAV, "calendar-home-set") => p.home_set = href(),
        (CALDAV, "supported-calendar-component-set") => {
            p.components = prop
                .children()
                .filter(|n| n.has_tag_name((CALDAV, "comp")))
                .filter_map(|n| n.attribute("name"))
                .map(|n| n.to_ascii_uppercase())
                .collect();
        }
        (DAV, "current-user-privilege-set") => {
            p.privileges = Some(
                prop.descendants()
                    .filter(|n| n.has_tag_name((DAV, "privilege")))
                    .flat_map(|n| n.children().filter(|c| c.is_element()))
                    .map(|n| n.tag_name().name().to_string())
                    .collect(),
            );
        }
        _ => {}
    }
}

/// All the text under a node, CDATA included.
fn texte(n: roxmltree::Node<'_, '_>) -> String {
    n.descendants()
        .filter(|d| d.is_text())
        .filter_map(|d| d.text())
        .collect()
}

/// `HTTP/1.1 200 OK` → true.
fn status_ok(ligne: &str) -> bool {
    ligne
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .is_some_and(|c| (200..300).contains(&c))
}

/// Where the user's calendars are, and who they are.
pub const FIND_HOME: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:current-user-principal/>
    <c:calendar-home-set/>
    <d:resourcetype/>
  </d:prop>
</d:propfind>"#;

/// What each calendar of a home is.
pub const LIST_CALENDARS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/">
  <d:prop>
    <d:resourcetype/>
    <d:displayname/>
    <a:calendar-color/>
    <cs:getctag/>
    <c:supported-calendar-component-set/>
    <d:current-user-privilege-set/>
  </d:prop>
</d:propfind>"#;

/// The tag of every event of a calendar.
pub const EVENT_ETAGS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT"/>
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#;

/// The objects at `hrefs`, with their tags.
pub fn multiget(hrefs: &[String]) -> String {
    let mut corps = String::from(
        r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag/>
    <c:calendar-data/>
  </d:prop>
"#,
    );
    for h in hrefs {
        corps.push_str("  <d:href>");
        corps.push_str(&echapper(h));
        corps.push_str("</d:href>\n");
    }
    corps.push_str("</c:calendar-multiget>");
    corps
}

fn echapper(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_with_its_calendars_is_read() {
        let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:x1="http://apple.com/ns/ical/">
  <d:response>
    <d:href>/dav/calendars/user/a@example.com/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/calendars/user/a@example.com/work/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/><cal:calendar/></d:resourcetype>
        <d:displayname>Work &amp; more</d:displayname>
        <x1:calendar-color>#3A87ADFF</x1:calendar-color>
        <cs:getctag>"42"</cs:getctag>
        <cal:supported-calendar-component-set><cal:comp name="VEVENT"/></cal:supported-calendar-component-set>
        <d:current-user-privilege-set><d:privilege><d:read/></d:privilege><d:privilege><d:write/></d:privilege></d:current-user-privilege-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
    <d:propstat><d:prop><d:sync-token/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/calendars/user/a@example.com/holidays/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/><cal:calendar/></d:resourcetype>
        <d:displayname>Holidays</d:displayname>
        <d:current-user-privilege-set><d:privilege><d:read/></d:privilege></d:current-user-privilege-set>
      </d:prop>
      <d:status>HTTP/1.1 200 OK</d:status>
    </d:propstat>
  </d:response>
</d:multistatus>"##;
        let r = multistatus(xml).unwrap();
        assert_eq!(r.len(), 3);
        assert!(!r[0].is_calendar);
        let travail = &r[1];
        assert!(travail.is_calendar && travail.takes_events() && travail.writable());
        assert_eq!(travail.name.as_deref(), Some("Work & more"));
        assert_eq!(travail.color.as_deref(), Some("#3A87ADFF"));
        assert_eq!(travail.ctag.as_deref(), Some("\"42\""));
        assert!(!r[2].writable(), "read only");
    }

    #[test]
    fn a_multiget_gives_each_object_and_says_which_are_gone() {
        let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/cal/a.ics</d:href>
    <d:propstat><d:prop><d:getetag>"e1"</d:getetag><c:calendar-data><![CDATA[BEGIN:VCALENDAR
END:VCALENDAR
]]></c:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
  </d:response>
  <d:response><d:href>/cal/b.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
</d:multistatus>"#;
        let r = multistatus(xml).unwrap();
        assert_eq!(r[0].etag.as_deref(), Some("\"e1\""));
        assert!(r[0]
            .calendar_data
            .as_deref()
            .unwrap()
            .starts_with("BEGIN:VCALENDAR"));
        assert!(!r[1].found);
    }

    #[test]
    fn where_the_calendars_are_is_read() {
        let xml = r#"<multistatus xmlns="DAV:"><response><href>/</href><propstat><prop>
<current-user-principal><href>/principals/a/</href></current-user-principal>
<C:calendar-home-set xmlns:C="urn:ietf:params:xml:ns:caldav"><href>https://p01-caldav.example.com/123/calendars/</href></C:calendar-home-set>
</prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>"#;
        let r = multistatus(xml).unwrap();
        assert_eq!(r[0].principal.as_deref(), Some("/principals/a/"));
        assert_eq!(
            r[0].home_set.as_deref(),
            Some("https://p01-caldav.example.com/123/calendars/")
        );
    }

    #[test]
    fn hrefs_are_escaped_in_a_multiget() {
        assert!(multiget(&["/c/a&b.ics".into()]).contains("<d:href>/c/a&amp;b.ics</d:href>"));
    }
}
