//! Calendars kept with a server, both ways (CalDAV).
//!
//! An account is a server and who signs in there: a user name and a password kept in
//! the vault, or, for Google, the sign-in of one of one's mailboxes. Its calendars are
//! found once (`iris_caldav::Client::find_home`) and listed again at each sync, which
//! adds the new ones and drops those gone.
//!
//! A sync first **sends** what was changed here — objects deleted, then objects
//! changed or made, each written over the version last read (`If-Match`), the object
//! as the server had it patched rather than rewritten, so attendees and whatever else
//! Iris does not keep go back unchanged — then **reads** what changed there: the tag
//! of every event, and the objects whose tag moved. When both sides changed one object,
//! the server's version wins. Nothing is read when the calendar's own tag (`getctag`)
//! did not move and nothing was sent.
//!
//! Syncs run one at a time: every quarter of an hour, within a minute of a change made
//! here, and when asked.

use crate::services::{now, Services};
use iris_caldav::{Auth, Client, Resource, Written};
use iris_secrets::{Secret, SecretKind};
use iris_store::{CalendarAccount, StoredCalendar};
use iris_types::{AccountId, Error, Result};
use iris_ui::AppWindow;
use slint::ComponentHandle;
use std::collections::HashSet;

/// How often every account is read again.
const PERIODE: std::time::Duration = std::time::Duration::from_secs(15 * 60);
/// How often changes made here are looked for, to send them.
const ENVOI: std::time::Duration = std::time::Duration::from_secs(60);

/// What a sync did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub sent: usize,
    pub received: usize,
    pub removed: usize,
    /// Objects changed on both sides: the server's version was kept.
    pub conflicts: usize,
}

impl Report {
    pub fn changed_anything(&self) -> bool {
        self.sent + self.received + self.removed + self.conflicts > 0
    }
}

/// Where an account's password is kept in the vault.
fn cle(id: i64) -> String {
    format!("caldav:{id}")
}

/// One sync at a time, whoever asks.
fn verrou() -> &'static tokio::sync::Mutex<()> {
    static VERROU: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    VERROU.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// How an account signs in now.
async fn auth(services: &Services, compte: &CalendarAccount) -> Result<Auth> {
    if let Some(id) = compte.mail_account {
        return google(services, AccountId(id)).await;
    }
    let motdepasse = services
        .secrets
        .get(&cle(compte.id), SecretKind::Password)?
        .ok_or_else(|| Error::AuthFailed {
            account: compte.name.clone(),
        })?;
    Ok(Auth::Basic {
        user: compte.username.clone(),
        password: motdepasse.expose().to_string(),
    })
}

/// A mailbox's Google sign-in, renewed if need be, as the mail's is.
async fn google(services: &Services, id: AccountId) -> Result<Auth> {
    let boite = services
        .store
        .account(id)?
        .ok_or_else(|| Error::other("the Gmail mailbox it used has been removed"))?;
    match services.credentials.credentials(id, &boite.email).await? {
        iris_imap::Credentials::OAuth2 { token, .. } => Ok(Auth::Bearer(token)),
        iris_imap::Credentials::Password { .. } => Err(Error::other(
            "this mailbox does not sign in with Google: Google's calendars need its sign-in",
        )),
    }
}

/// The mailbox signed in with Google under `adresse`, if there is one.
fn boite_google(services: &Services, adresse: &str) -> Option<i64> {
    services
        .store
        .accounts()
        .ok()?
        .into_iter()
        .find(|a| {
            a.email.eq_ignore_ascii_case(adresse.trim())
                && a.auth == iris_store::AuthKind::OAuthGoogle
        })
        .map(|a| a.id.0)
}

/// What a refusal means for whoever typed.
fn dire(e: &Error, google: bool) -> String {
    match e {
        Error::AuthFailed { .. } if google => "Google refused Iris the calendars of this \
            mailbox. Disconnect them and connect them again to sign in, and allow the calendars."
            .into(),
        Error::AuthFailed { .. } => "The server refused this user name and password. iCloud \
            and Fastmail need an app password, made in their account settings."
            .into(),
        autre => autre.to_string(),
    }
}

/// Connects the calendars found from what was typed. Gives the account and how many
/// calendars it has; the first sync is done before returning.
pub async fn connect(
    services: &Services,
    typed: &str,
    user: &str,
    password: &str,
) -> std::result::Result<(i64, usize), String> {
    let depart = iris_caldav::starting_point(typed).map_err(|e| e.to_string())?;
    let est_google = depart.contains("googleusercontent.com");
    let utilisateur = if user.trim().is_empty() {
        typed.trim().to_string()
    } else {
        user.trim().to_string()
    };
    let deja = services
        .store
        .calendar_accounts()
        .map_err(|e| e.to_string())?
        .into_iter()
        .any(|c| c.server == depart && c.username.eq_ignore_ascii_case(&utilisateur));
    if deja {
        return Err("These calendars are already connected.".into());
    }
    let boite = est_google.then(|| boite_google(services, typed)).flatten();
    let auth = match boite {
        Some(id) => google(services, AccountId(id))
            .await
            .map_err(|e| dire(&e, true))?,
        None if est_google => {
            return Err(
                "Google's calendars take no password: add this Gmail mailbox to Iris \
                 with Google's sign-in first, then type its address here."
                    .into(),
            )
        }
        None if password.is_empty() => {
            return Err("A password is needed (an app password for iCloud and Fastmail).".into())
        }
        None => Auth::Basic {
            user: utilisateur.clone(),
            password: password.to_string(),
        },
    };
    let mut client = Client::new(auth).map_err(|e| e.to_string())?;
    let home = match (client.find_home(&depart).await, boite) {
        (Ok(h), _) => h,
        // Signed in for the mail only, before Iris asked for the calendars: Google asks
        // again, in the browser, and the search starts over with the new sign-in.
        (Err(Error::AuthFailed { .. }), Some(id)) => {
            let reglages = services
                .oauth
                .read()
                .map_err(|_| "the sign-in settings could not be read".to_string())?
                .clone();
            crate::oauth::authorize_with(
                std::sync::Arc::clone(&services.secrets),
                &reglages,
                iris_oauth::Provider::Google,
                typed.trim(),
                now(),
                &[iris_oauth::GOOGLE_CALENDAR_SCOPE],
            )
            .await
            .map_err(|e| e.to_string())?;
            let auth = google(services, AccountId(id))
                .await
                .map_err(|e| dire(&e, true))?;
            client = Client::new(auth).map_err(|e| e.to_string())?;
            client
                .find_home(&depart)
                .await
                .map_err(|e| dire(&e, true))?
        }
        (Err(e), _) => return Err(dire(&e, est_google)),
    };
    let calendriers = client
        .calendars(&home)
        .await
        .map_err(|e| dire(&e, est_google))?;
    if calendriers.is_empty() {
        return Err("No calendar was found there.".into());
    }

    let nom = if typed.contains('@') {
        typed.trim().to_string()
    } else {
        reqwest::Url::parse(&home)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| typed.trim().to_string())
    };
    let id = services
        .store
        .create_calendar_account(&nom, &depart, &utilisateur, boite, now())
        .map_err(|e| e.to_string())?;
    if boite.is_none() {
        services
            .secrets
            .set(&cle(id), SecretKind::Password, &Secret::new(password))
            .map_err(|e| e.to_string())?;
    }
    services
        .store
        .set_calendar_account_home(id, &home)
        .map_err(|e| e.to_string())?;
    if let Err(e) = sync_account(services, id).await {
        tracing::warn!(error = %e, "first calendar sync");
    }
    Ok((id, calendriers.len()))
}

/// Removes an account and its calendars from this computer; the server keeps them.
pub fn disconnect(services: &Services, id: i64) -> Result<()> {
    services.store.delete_calendar_account(id)?;
    let _ = services.secrets.delete(&cle(id), SecretKind::Password);
    Ok(())
}

/// Syncs one account, and keeps how it went.
pub async fn sync_account(services: &Services, id: i64) -> Result<Report> {
    let _un_a_la_fois = verrou().lock().await;
    let compte = services
        .store
        .calendar_account(id)?
        .ok_or_else(|| Error::other("this calendar account is gone"))?;
    let resultat = synchroniser(services, &compte).await;
    let erreur = resultat
        .as_ref()
        .err()
        .map(|e| dire(e, compte.mail_account.is_some()));
    services
        .store
        .set_calendar_account_sync(id, now(), erreur.as_deref())?;
    resultat
}

async fn synchroniser(services: &Services, compte: &CalendarAccount) -> Result<Report> {
    let client = Client::new(auth(services, compte).await?)?;
    let home = match &compte.home {
        Some(h) => h.clone(),
        None => {
            let h = client.find_home(&compte.server).await?;
            services.store.set_calendar_account_home(compte.id, &h)?;
            h
        }
    };
    let distants = client.calendars(&home).await?;
    let adresses: Vec<String> = distants.iter().map(|c| c.url.clone()).collect();
    services
        .store
        .remove_remote_calendars_except(compte.id, &adresses)?;

    let mut bilan = Report::default();
    for d in &distants {
        let teinte = d
            .color
            .clone()
            .unwrap_or_else(|| crate::calendar::free_color(services).to_string());
        let id = services.store.upsert_remote_calendar(
            compte.id,
            &d.url,
            &d.name,
            &teinte,
            d.read_only,
            now(),
        )?;
        let Some(cal) = services.store.calendar(id)? else {
            continue;
        };
        let envoye = if d.read_only {
            false
        } else {
            envoyer(services, &client, &cal, &mut bilan).await?
        };
        if envoye || d.ctag.is_none() || cal.etag != d.ctag {
            recevoir(services, &client, &cal, &mut bilan).await?;
            services.store.set_remote_ctag(id, d.ctag.as_deref())?;
        }
    }
    Ok(bilan)
}

/// What was deleted and changed here, sent. True when anything was.
async fn envoyer(
    services: &Services,
    client: &Client,
    cal: &StoredCalendar,
    bilan: &mut Report,
) -> Result<bool> {
    let Some(adresse) = cal.remote_url.as_deref() else {
        return Ok(false);
    };
    let mut fait = false;
    for t in services.store.tombstones(cal.id)? {
        // Changed there since: the server's version comes back at the reading.
        if client.delete(&t.href, t.etag.as_deref()).await? == Written::Conflict {
            bilan.conflicts += 1;
        }
        services.store.clear_tombstone(t.id)?;
        fait = true;
    }
    for o in services.store.outgoing_objects(cal.id)? {
        let evenements: Vec<iris_calendar::Event> = o
            .events
            .iter()
            .map(|e| crate::calendar::to_domain(&e.event))
            .collect();
        let refs: Vec<&iris_calendar::Event> = evenements.iter().collect();
        let instant = now().millis();
        let ics = match &o.remote_ics {
            Some(original) => iris_calendar::write::patch_object(original, &refs, instant),
            None => iris_calendar::write::object(&refs, instant),
        };
        let nouveau = o.href.is_none();
        let href = o
            .href
            .clone()
            .unwrap_or_else(|| iris_caldav::object_url(adresse, &o.uid));
        match client.put(&href, &ics, o.etag.as_deref(), nouveau).await? {
            Written::Done(etag) => {
                services
                    .store
                    .mark_object_sent(cal.id, &o.uid, &href, etag.as_deref(), &ics)?;
                bilan.sent += 1;
            }
            Written::Conflict => {
                // Changed there meanwhile: the server's version wins.
                match client
                    .fetch(adresse, std::slice::from_ref(&href))
                    .await?
                    .first()
                {
                    Some(r) => garder(services, cal.id, r)?,
                    None => services.store.remove_remote_object(cal.id, &href)?,
                }
                bilan.conflicts += 1;
            }
        }
        fait = true;
    }
    Ok(fait)
}

/// What changed there, read: the objects whose tag moved, and those gone.
async fn recevoir(
    services: &Services,
    client: &Client,
    cal: &StoredCalendar,
    bilan: &mut Report,
) -> Result<()> {
    let Some(adresse) = cal.remote_url.as_deref() else {
        return Ok(());
    };
    let distants = client.etags(adresse).await?;
    let locaux = services.store.remote_objects(cal.id)?;
    let a_lire: Vec<String> = distants
        .iter()
        .filter(|r| match locaux.get(&r.url) {
            // Changed here: sent first, at the next sync if it failed now.
            Some((_, true)) => false,
            Some((etag, false)) => etag.is_none() || *etag != r.etag,
            None => true,
        })
        .map(|r| r.url.clone())
        .collect();
    for r in client.fetch(adresse, &a_lire).await? {
        garder(services, cal.id, &r)?;
        bilan.received += 1;
    }
    let presents: HashSet<&str> = distants.iter().map(|r| r.url.as_str()).collect();
    for (href, (_, change_ici)) in &locaux {
        if !change_ici && !presents.contains(href.as_str()) {
            services.store.remove_remote_object(cal.id, href)?;
            bilan.removed += 1;
        }
    }
    Ok(())
}

/// The server's version of an object, kept.
fn garder(services: &Services, calendrier: i64, r: &Resource) -> Result<()> {
    let ics = r.ics.as_deref().unwrap_or_default();
    let lu = iris_calendar::ics::parse(ics).map_err(Error::other)?;
    let evenements: Vec<iris_store::NewEvent> =
        lu.events.iter().map(crate::calendar::from_domain).collect();
    if evenements.is_empty() {
        return services.store.remove_remote_object(calendrier, &r.url);
    }
    services.store.store_remote_object(
        calendrier,
        &r.url,
        r.etag.as_deref(),
        ics,
        &evenements,
        now(),
    )
}

/// "Synced 5 min ago", or what went wrong, for a calendar of an account.
pub fn status(
    services: &Services,
    compte: i64,
    maintenant: iris_types::Timestamp,
) -> (String, bool) {
    match services.store.calendar_account(compte).ok().flatten() {
        Some(CalendarAccount {
            last_error: Some(e),
            ..
        }) => (format!("Could not sync: {e}"), true),
        Some(CalendarAccount {
            last_sync: Some(t), ..
        }) => (
            format!("Synced {}", crate::vitals::ago(t, maintenant)),
            false,
        ),
        _ => ("Not synced yet".into(), false),
    }
}

/// The account's name, to say what disconnecting removes.
pub fn account_name(services: &Services, compte: i64) -> String {
    services
        .store
        .calendar_account(compte)
        .ok()
        .flatten()
        .map(|c| c.name)
        .unwrap_or_default()
}

/// The panel that connects an account, and the syncs that keep it.
pub fn wire_caldav(fenetre: &AppWindow, services: &Services, runtime: tokio::runtime::Handle) {
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        let runtime = runtime.clone();
        fenetre.on_connect_confirmed(move || {
            let Some(f) = faible.upgrade() else {
                return;
            };
            if f.get_connect_busy() {
                return;
            }
            let (serveur, utilisateur, motdepasse) = (
                f.get_connect_server().to_string(),
                f.get_connect_user().to_string(),
                f.get_connect_password().to_string(),
            );
            f.set_connect_error("".into());
            f.set_connect_busy(true);
            let services = services.clone();
            let faible = f.as_weak();
            runtime.spawn(async move {
                let issue = connect(&services, &serveur, &utilisateur, &motdepasse).await;
                let _ = faible.upgrade_in_event_loop(move |f| {
                    f.set_connect_busy(false);
                    match issue {
                        Ok((_, n)) => {
                            f.set_connect_open(false);
                            f.set_connect_password("".into());
                            f.set_toast(
                                match n {
                                    1 => "1 calendar connected.".to_string(),
                                    n => format!("{n} calendars connected."),
                                }
                                .into(),
                            );
                            if f.get_workspace() == 1 {
                                f.invoke_workspace_changed(1);
                            }
                        }
                        Err(e) => f.set_connect_error(e.into()),
                    }
                });
            });
        });
    }

    // Every quarter of an hour, all of them; every minute, those with changes made here.
    {
        let services = services.clone();
        let faible = fenetre.as_weak();
        runtime.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(25)).await;
            let mut dernier_tour: Option<std::time::Instant> = None;
            loop {
                let tous = dernier_tour.is_none_or(|t| t.elapsed() >= PERIODE);
                let comptes: Vec<i64> = if tous {
                    dernier_tour = Some(std::time::Instant::now());
                    services
                        .store
                        .calendar_accounts()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|c| c.id)
                        .collect()
                } else {
                    services
                        .store
                        .calendar_accounts_with_changes()
                        .unwrap_or_default()
                };
                let mut change = false;
                for id in comptes {
                    match sync_account(&services, id).await {
                        Ok(b) => change |= b.changed_anything(),
                        Err(e) => {
                            tracing::info!(account = id, error = %e, "calendar sync");
                            change = true;
                        }
                    }
                }
                if change {
                    redessiner(&faible);
                }
                tokio::time::sleep(ENVOI).await;
            }
        });
    }
}

/// Syncs the account of a calendar now, and draws the calendar again after.
pub fn sync_now(
    fenetre: &AppWindow,
    services: &Services,
    runtime: &tokio::runtime::Handle,
    compte: i64,
) {
    let services = services.clone();
    let faible = fenetre.as_weak();
    runtime.spawn(async move {
        let issue = sync_account(&services, compte).await;
        let _ = faible.upgrade_in_event_loop(move |f| {
            if let Err(e) = issue {
                f.set_status(format!("Could not sync: {e}").into());
            }
            if f.get_workspace() == 1 {
                f.invoke_workspace_changed(1);
            }
        });
    });
}

fn redessiner(faible: &slint::Weak<AppWindow>) {
    let _ = faible.upgrade_in_event_loop(|f| {
        if f.get_workspace() == 1 {
            f.invoke_workspace_changed(1);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_says_whether_anything_changed() {
        assert!(!Report::default().changed_anything());
        assert!(Report {
            conflicts: 1,
            ..Default::default()
        }
        .changed_anything());
    }

    #[test]
    fn refusals_say_what_to_do() {
        let refus = Error::AuthFailed {
            account: "x".into(),
        };
        assert!(dire(&refus, false).contains("app password"));
        assert!(dire(&refus, true).contains("allow the calendars"));
    }
}
