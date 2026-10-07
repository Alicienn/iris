//! `iris-caldav` — calendars kept on a server (CalDAV, RFC 4791).
//!
//! What a client needs and nothing else, over HTTPS:
//!
//! - **finding** the user's calendars from an address or a server: the well-known
//!   address (`/.well-known/caldav`, RFC 6764), the principal, its calendar home, and
//!   what each calendar is called, its colour, whether it takes events and whether
//!   it may be written;
//! - **reading**: the tag (`ETag`) of every event, then the objects whose tag changed,
//!   by fifty (`calendar-multiget`);
//! - **writing**: one object put (`If-Match` its tag, or `If-None-Match: *` when new),
//!   or deleted. A tag that no longer matches is a conflict, said as such: the caller
//!   decides (the server's version wins in Iris).
//!
//! It knows nothing of the base nor of iCalendar's content: objects come and go as
//! text. Redirections are followed by hand, keeping the method and the body, which a
//! `PROPFIND` needs and the HTTP client would not do.

#![forbid(unsafe_code)]

pub mod xml;

use iris_types::{Error, Result};
use reqwest::{Method, StatusCode, Url};

/// How to sign in.
#[derive(Clone)]
pub enum Auth {
    /// A user name and a password (an app password, for iCloud and Fastmail).
    Basic { user: String, password: String },
    /// An OAuth access token (Google).
    Bearer(String),
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Auth::Basic { user, .. } => write!(f, "Basic({user}, …)"),
            Auth::Bearer(_) => write!(f, "Bearer(…)"),
        }
    }
}

/// A calendar found on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCalendar {
    /// Its address, whole.
    pub url: String,
    pub name: String,
    /// `#rrggbb`, when the server gives one.
    pub color: Option<String>,
    /// Changes whenever anything in it does (`getctag`), when the server says.
    pub ctag: Option<String>,
    pub read_only: bool,
}

/// An object of a calendar: its address, its tag, and its text when asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    pub url: String,
    pub etag: Option<String>,
    pub ics: Option<String>,
}

/// What became of a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// Done; the new tag, when the server gave it.
    Done(Option<String>),
    /// The object changed on the server since it was read.
    Conflict,
}

/// How many objects one `calendar-multiget` asks for.
const PAR_LOT: usize = 50;
/// How many redirections are followed.
const REDIRECTIONS: usize = 5;

/// Well-known servers, by the domain of an address: where their calendars are.
pub fn server_for_domain(domaine: &str) -> Option<&'static str> {
    Some(match domaine.to_ascii_lowercase().as_str() {
        "icloud.com" | "me.com" | "mac.com" => "https://caldav.icloud.com/",
        "fastmail.com" | "fastmail.fm" | "messagingengine.com" => "https://caldav.fastmail.com/",
        "gmail.com" | "googlemail.com" => "https://apidata.googleusercontent.com/caldav/v2/",
        "mailbox.org" => "https://dav.mailbox.org/",
        "posteo.de" | "posteo.net" => "https://posteo.de:8443/",
        "yahoo.com" | "yahoo.fr" => "https://caldav.calendar.yahoo.com/",
        _ => return None,
    })
}

/// Where to start looking, from what was typed: a full address of a server is taken
/// as it is; an e-mail address or a bare domain goes to its known server, else to the
/// domain's well-known address.
pub fn starting_point(typed: &str) -> Result<String> {
    let t = typed.trim();
    if t.is_empty() {
        return Err(Error::Config("an address or a server is needed".into()));
    }
    if t.starts_with("https://") || t.starts_with("http://") {
        return Ok(t.to_string());
    }
    let domaine = t.rsplit('@').next().unwrap_or(t).trim_end_matches('/');
    if domaine.contains('/') {
        return Ok(format!("https://{t}"));
    }
    Ok(match server_for_domain(domaine) {
        Some(s) => s.to_string(),
        None => format!("https://{domaine}/.well-known/caldav"),
    })
}

/// A client signed in to one server.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    auth: Auth,
}

/// An answer, read whole: its status, its tag, its body, and the address that gave it
/// (after redirections).
struct Reponse {
    status: StatusCode,
    etag: Option<String>,
    body: String,
    url: Url,
}

impl Client {
    pub fn new(auth: Auth) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("Iris/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Network(format!("client: {e}")))?;
        Ok(Self { http, auth })
    }

    async fn requete(
        &self,
        methode: &str,
        url: &str,
        depth: Option<&str>,
        corps: Option<String>,
        entetes: &[(&str, String)],
    ) -> Result<Reponse> {
        let methode = Method::from_bytes(methode.as_bytes())
            .map_err(|e| Error::other(format!("method: {e}")))?;
        let mut adresse = Url::parse(url).map_err(|e| Error::Config(format!("{url}: {e}")))?;
        for _ in 0..=REDIRECTIONS {
            let mut r = self.http.request(methode.clone(), adresse.clone());
            r = match &self.auth {
                Auth::Basic { user, password } => r.basic_auth(user, Some(password)),
                Auth::Bearer(jeton) => r.bearer_auth(jeton),
            };
            if let Some(d) = depth {
                r = r.header("Depth", d);
            }
            for (nom, valeur) in entetes {
                r = r.header(*nom, valeur.as_str());
            }
            if let Some(c) = &corps {
                let type_ = if c.starts_with("BEGIN:VCALENDAR") {
                    "text/calendar; charset=utf-8"
                } else {
                    "application/xml; charset=utf-8"
                };
                r = r.header("Content-Type", type_).body(c.clone());
            }
            let reponse = r.send().await.map_err(|e| {
                Error::Network(format!("{} ({e})", adresse.host_str().unwrap_or("")))
            })?;
            let status = reponse.status();
            if status.is_redirection() {
                let suite = reponse
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| adresse.join(l).ok());
                match suite {
                    Some(s) => {
                        adresse = s;
                        continue;
                    }
                    None => return Err(Error::other(format!("the server answered {status}"))),
                }
            }
            // Refused: a wrong password, or (403 to a lookup) a token without the right
            // to the calendars.
            if status == StatusCode::UNAUTHORIZED
                || (status == StatusCode::FORBIDDEN && depth.is_some())
            {
                return Err(Error::AuthFailed {
                    account: adresse.host_str().unwrap_or_default().to_string(),
                });
            }
            let etag = reponse
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let body = reponse
                .text()
                .await
                .map_err(|e| Error::Network(format!("answer cut short ({e})")))?;
            return Ok(Reponse {
                status,
                etag,
                body,
                url: adresse,
            });
        }
        Err(Error::other("too many redirections"))
    }

    /// A `PROPFIND` or a `REPORT`, whose answer must be a multistatus.
    async fn multistatus(
        &self,
        methode: &str,
        url: &str,
        depth: &str,
        corps: &str,
    ) -> Result<(Vec<xml::Props>, Url)> {
        let r = self
            .requete(methode, url, Some(depth), Some(corps.to_string()), &[])
            .await?;
        if r.status != StatusCode::MULTI_STATUS {
            return Err(Error::Protocol {
                protocol: "CalDAV",
                message: format!("{url} answered {}", r.status),
            });
        }
        let props = xml::multistatus(&r.body).map_err(|e| Error::Protocol {
            protocol: "CalDAV",
            message: e,
        })?;
        Ok((props, r.url))
    }

    /// The calendar home from `start`: itself if it is one, else through the principal.
    /// For a domain's well-known address that does not answer, the paths Nextcloud and
    /// others use are tried.
    pub async fn find_home(&self, start: &str) -> Result<String> {
        let mut essais = vec![start.to_string()];
        if let Ok(u) = Url::parse(start) {
            if u.path().starts_with("/.well-known/caldav") {
                for chemin in ["/remote.php/dav/", "/dav/", "/"] {
                    if let Ok(v) = u.join(chemin) {
                        essais.push(v.to_string());
                    }
                }
            }
        }
        let mut derniere = Error::other("no calendar was found there");
        for essai in essais {
            match self.home_from(&essai).await {
                Ok(h) => return Ok(h),
                Err(e @ Error::AuthFailed { .. }) => return Err(e),
                Err(e) => derniere = e,
            }
        }
        Err(derniere)
    }

    async fn home_from(&self, start: &str) -> Result<String> {
        let (props, ici) = self
            .multistatus("PROPFIND", start, "0", xml::FIND_HOME)
            .await?;
        let p = props.into_iter().next().unwrap_or_default();
        if let Some(h) = p.home_set {
            return join(&ici, &h);
        }
        if p.is_calendar {
            // A calendar's own address: its home is where it lives.
            return Ok(ici
                .join("..")
                .map(|u| u.to_string())
                .unwrap_or_else(|_| ici.to_string()));
        }
        let principal = p
            .principal
            .ok_or_else(|| Error::other("the server does not say where the calendars are"))?;
        let principal = join(&ici, &principal)?;
        let (props, ici) = self
            .multistatus("PROPFIND", &principal, "0", xml::FIND_HOME)
            .await?;
        let h = props
            .into_iter()
            .find_map(|p| p.home_set)
            .ok_or_else(|| Error::other("the server does not say where the calendars are"))?;
        join(&ici, &h)
    }

    /// The calendars of a home that hold events.
    pub async fn calendars(&self, home: &str) -> Result<Vec<RemoteCalendar>> {
        let (props, ici) = self
            .multistatus("PROPFIND", home, "1", xml::LIST_CALENDARS)
            .await?;
        let mut sortie = Vec::new();
        for p in props {
            if !p.is_calendar || !p.takes_events() {
                continue;
            }
            let url = join(&ici, &p.href)?;
            let name = p
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| derniere_partie(&url));
            sortie.push(RemoteCalendar {
                url,
                name,
                color: p.color.as_deref().and_then(couleur),
                ctag: p.ctag.clone(),
                read_only: !p.writable(),
            });
        }
        Ok(sortie)
    }

    /// The tag of every event of a calendar, by address.
    pub async fn etags(&self, calendar: &str) -> Result<Vec<Resource>> {
        let (props, ici) = self
            .multistatus("REPORT", calendar, "1", xml::EVENT_ETAGS)
            .await?;
        let base = Url::parse(calendar).map_err(|e| Error::Config(e.to_string()))?;
        let mut sortie = Vec::new();
        for p in props.into_iter().filter(|p| p.found && !p.href.is_empty()) {
            let url = join(&ici, &p.href)?;
            // The calendar itself, which some servers list too.
            if url.trim_end_matches('/') == base.as_str().trim_end_matches('/') {
                continue;
            }
            sortie.push(Resource {
                url,
                etag: p.etag,
                ics: None,
            });
        }
        Ok(sortie)
    }

    /// The objects at `urls`, with their text and tags; those gone are left out.
    pub async fn fetch(&self, calendar: &str, urls: &[String]) -> Result<Vec<Resource>> {
        let mut sortie = Vec::new();
        for lot in urls.chunks(PAR_LOT) {
            // Asked by path, as a server writes them.
            let chemins: Vec<String> = lot
                .iter()
                .map(|u| {
                    Url::parse(u)
                        .map(|u| u.path().to_string())
                        .unwrap_or_else(|_| u.clone())
                })
                .collect();
            let (props, ici) = self
                .multistatus("REPORT", calendar, "1", &xml::multiget(&chemins))
                .await?;
            for p in props {
                if !p.found || p.calendar_data.is_none() {
                    continue;
                }
                sortie.push(Resource {
                    url: join(&ici, &p.href)?,
                    etag: p.etag,
                    ics: p.calendar_data,
                });
            }
        }
        Ok(sortie)
    }

    /// Writes an object: a new one (`new`, refused if one is there already), or over
    /// the version tagged `etag` (over whatever is there when its tag is not known).
    pub async fn put(
        &self,
        url: &str,
        ics: &str,
        etag: Option<&str>,
        new: bool,
    ) -> Result<Written> {
        let condition: Vec<(&str, String)> = match (new, etag) {
            (true, _) => vec![("If-None-Match", "*".to_string())],
            (false, Some(e)) => vec![("If-Match", e.to_string())],
            (false, None) => Vec::new(),
        };
        let r = self
            .requete("PUT", url, None, Some(ics.to_string()), &condition)
            .await?;
        match r.status {
            s if s.is_success() => Ok(Written::Done(r.etag)),
            StatusCode::PRECONDITION_FAILED => Ok(Written::Conflict),
            s => Err(Error::Protocol {
                protocol: "CalDAV",
                message: format!("the event was refused ({s}): {}", premiere_ligne(&r.body)),
            }),
        }
    }

    /// Deletes an object, the version tagged `etag` when known. One already gone is
    /// deleted.
    pub async fn delete(&self, url: &str, etag: Option<&str>) -> Result<Written> {
        let entetes: Vec<(&str, String)> = etag
            .map(|e| ("If-Match", e.to_string()))
            .into_iter()
            .collect();
        let r = self.requete("DELETE", url, None, None, &entetes).await?;
        match r.status {
            s if s.is_success() => Ok(Written::Done(None)),
            StatusCode::NOT_FOUND | StatusCode::GONE => Ok(Written::Done(None)),
            StatusCode::PRECONDITION_FAILED => Ok(Written::Conflict),
            s => Err(Error::Protocol {
                protocol: "CalDAV",
                message: format!("the event could not be deleted ({s})"),
            }),
        }
    }
}

/// An address the server wrote (a path, usually), made whole against where it said it.
fn join(base: &Url, href: &str) -> Result<String> {
    base.join(href)
        .map(|u| u.to_string())
        .map_err(|e| Error::Protocol {
            protocol: "CalDAV",
            message: format!("{href}: {e}"),
        })
}

/// The last part of an address, for a calendar that has no name.
fn derniere_partie(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("Calendar")
        .to_string()
}

/// `#3A87ADFF` → `#3a87ad`.
fn couleur(v: &str) -> Option<String> {
    let v = v.trim();
    let hex = v.strip_prefix('#')?;
    (hex.len() >= 6 && hex[..6].chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex[..6].to_ascii_lowercase()))
}

fn premiere_ligne(texte: &str) -> String {
    texte
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(160)
        .collect()
}

/// Where a new event of `calendar` goes: its UID made safe for a path, and `.ics`.
pub fn object_url(calendar: &str, uid: &str) -> String {
    let propre: String = uid
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = if calendar.ends_with('/') {
        calendar.to_string()
    } else {
        format!("{calendar}/")
    };
    format!("{base}{propre}.ics")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_leads_somewhere() {
        assert_eq!(
            starting_point("a@icloud.com").unwrap(),
            "https://caldav.icloud.com/"
        );
        assert_eq!(
            starting_point("someone@example.com").unwrap(),
            "https://example.com/.well-known/caldav"
        );
        assert_eq!(
            starting_point("cloud.example.com/remote.php/dav").unwrap(),
            "https://cloud.example.com/remote.php/dav"
        );
        assert_eq!(
            starting_point("https://dav.example.com/cal/").unwrap(),
            "https://dav.example.com/cal/"
        );
        assert!(starting_point("  ").is_err());
    }

    #[test]
    fn colours_and_names_are_tidied() {
        assert_eq!(couleur("#3A87ADFF").as_deref(), Some("#3a87ad"));
        assert_eq!(couleur("blue"), None);
        assert_eq!(derniere_partie("https://example.com/cal/work/"), "work");
    }

    #[test]
    fn a_new_event_has_an_address_of_its_own() {
        assert_eq!(
            object_url("https://example.com/cal/work", "1759830000-42@iris"),
            "https://example.com/cal/work/1759830000-42-iris.ics"
        );
    }
}
