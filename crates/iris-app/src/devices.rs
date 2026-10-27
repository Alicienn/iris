//! Settings › Devices: the devices paired with this one, pairing another, syncing.
//!
//! The listener and the exchange are `peers`; the announce and the watch on the
//! network, `iris_peer::Discovery`. This module joins them to the window: the list of
//! devices (with "On this Wi-Fi" for those seen), the pairing panel and its QR code,
//! what each sync did, and the question when another device removed an account.

use crate::peers::{Event, Peers};
use iris_secrets::SecretStore;
use iris_store::Store;
use iris_ui::{AppWindow, DeviceRowData};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A paired device seen on the network is synced with when its last sync is older.
const AUTO_SYNC: Duration = Duration::from_secs(10 * 60);

type Vus = Arc<Mutex<HashMap<String, (SocketAddr, Instant)>>>;

/// This computer's address on the local network: the one its default route leaves
/// from. Nothing is sent: a UDP socket only picks its address when "connected".
pub fn local_address() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// The name this device gives itself to the others.
pub fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "Iris".into())
}

/// A QR code's pixels, one a module, four modules of white around it (the panel scales
/// it up, pixelated). Pixels, not an image: they are made off the window's thread.
pub fn qr_pixels(text: &str) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let n = code.width();
    let marge = 4;
    let cote = (n + 2 * marge) as u32;
    let mut tampon = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(cote, cote);
    let couleurs = code.to_colors();
    for (i, px) in tampon.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = (i % cote as usize, i / cote as usize);
        let sombre = x >= marge
            && y >= marge
            && x < n + marge
            && y < n + marge
            && couleurs[(y - marge) * n + (x - marge)] == qrcode::Color::Dark;
        let v = if sombre { 0 } else { 255 };
        *px = slint::Rgba8Pixel {
            r: v,
            g: v,
            b: v,
            a: 255,
        };
    }
    Some(tampon)
}

/// "Synced with PC-Marie: 12 added, 3 changed. Changed on both, the newer kept: Work."
pub fn summary_line(peer: &str, s: &iris_peer::Summary) -> String {
    let mut parts = Vec::new();
    if s.inserted > 0 {
        parts.push(format!("{} added", s.inserted));
    }
    if s.updated > 0 {
        parts.push(format!("{} changed", s.updated));
    }
    if s.deleted > 0 {
        parts.push(format!("{} removed", s.deleted));
    }
    let mut ligne = if parts.is_empty() {
        format!("Synced with {peer}: nothing new.")
    } else {
        format!("Synced with {peer}: {}.", parts.join(", "))
    };
    if !s.conflicts.is_empty() {
        ligne.push_str(&format!(
            " Changed on both, the newer kept: {}.",
            s.conflicts.join(", ")
        ));
    }
    ligne
}

/// How a paired device reads in the list.
fn status_line(here: bool, last_seen_secs: i64, now_secs: i64) -> String {
    let ecart = (now_secs - last_seen_secs).max(0);
    let quand = match ecart {
        _ if last_seen_secs == 0 => "never synced".to_string(),
        0..=59 => "synced just now".to_string(),
        60..=3599 => format!("synced {} min ago", ecart / 60),
        3600..=86_399 => format!("synced {} h ago", ecart / 3600),
        _ => format!("synced {} days ago", ecart / 86_400),
    };
    if here {
        format!("On this Wi-Fi · {quand}")
    } else {
        let mut q = quand;
        if let Some(c) = q.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        q
    }
}

fn rows(services: &Base, vus: &Vus) -> Vec<DeviceRowData> {
    let maintenant = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let vus = vus.lock().unwrap_or_else(|e| e.into_inner());
    services
        .store
        .peers()
        .unwrap_or_default()
        .into_iter()
        .map(|(id, nom, vu)| {
            let here = vus.contains_key(&id);
            DeviceRowData {
                status: status_line(here, vu, maintenant).into(),
                name: if nom.is_empty() { id.clone() } else { nom }.into(),
                id: id.into(),
                here,
            }
        })
        .collect()
}

fn refresh(fenetre: &slint::Weak<AppWindow>, services: &Arc<Base>, vus: &Vus) {
    let lignes = rows(services, vus);
    let _ = fenetre.upgrade_in_event_loop(move |f| {
        f.set_devices(ModelRc::new(VecModel::from(lignes)));
    });
}

/// What the devices need of the services.
struct Base {
    store: Arc<Store>,
    secrets: Arc<dyn SecretStore>,
}

pub fn wire_devices(
    fenetre: &AppWindow,
    services: &crate::services::Services,
    runtime: tokio::runtime::Handle,
) {
    let racine_notes = crate::notes::racine(services);
    let services = Arc::new(Base {
        store: Arc::clone(&services.store),
        secrets: Arc::clone(&services.secrets),
    });
    let pairs: Arc<OnceLock<Peers>> = Arc::new(OnceLock::new());
    let vus: Vus = Arc::new(Mutex::new(HashMap::new()));
    let a_retirer: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let faible = fenetre.as_weak();
    fenetre.set_devices(ModelRc::new(VecModel::from(rows(&services, &vus))));

    // Listen, announce, watch: once a device is paired, or once the person opens the
    // pairing panel, never before (Windows asks to let Iris on the local network the
    // first time it listens, and that question belongs to the moment one pairs).
    let demarre = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let demarrer: Arc<dyn Fn() + Send + Sync> = {
        let (pairs, vus, services, faible, a_retirer) = (
            Arc::clone(&pairs),
            Arc::clone(&vus),
            Arc::clone(&services),
            faible.clone(),
            Arc::clone(&a_retirer),
        );
        let rt = runtime.clone();
        Arc::new(move || {
            if demarre.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let (pairs, vus, services, faible, a_retirer) = (
                Arc::clone(&pairs),
                Arc::clone(&vus),
                Arc::clone(&services),
                faible.clone(),
                Arc::clone(&a_retirer),
            );
            let rt2 = rt.clone();
            let racine_notes = racine_notes.clone();
            rt.spawn(async move {
                let rt = rt2;
                let demarre = Peers::start(
                    Arc::clone(&services.store),
                    Arc::clone(&services.secrets),
                    device_name(),
                    "0.0.0.0:0".parse().expect("an address"),
                )
                .await;
                let (p, mut evenements) = match demarre {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(error = %e, "listening for other devices");
                        return;
                    }
                };
                let port = p.port();
                p.set_notes_root(Some(racine_notes));
                let _ = pairs.set(p);
                crate::platform::current().multicast(true);

                match iris_peer::Discovery::start(&services.store.device_id(), port) {
                    Ok((annonce, mut vu)) => {
                        let (vus, services, faible, pairs) = (
                            Arc::clone(&vus),
                            Arc::clone(&services),
                            faible.clone(),
                            Arc::clone(&pairs),
                        );
                        rt.spawn(async move {
                            // Kept alive as long as the window watches.
                            let _annonce = annonce;
                            while let Some(s) = vu.recv().await {
                                vus.lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .insert(s.device_id.clone(), (s.addr, Instant::now()));
                                refresh(&faible, &services, &vus);
                                auto_sync(&pairs, &services, &s.device_id, s.addr).await;
                            }
                        });
                    }
                    Err(e) => tracing::warn!(error = %e, "announcing on the network"),
                }

                while let Some(ev) = evenements.recv().await {
                    match ev {
                        Event::Connected { digits } => {
                            let _ = faible.upgrade_in_event_loop(move |f| {
                                f.set_pair_digits(digits.into());
                            });
                        }
                        Event::Done(o) => {
                            refresh(&faible, &services, &vus);
                            let nom = if o.peer_name.is_empty() {
                                "the other device".to_string()
                            } else {
                                o.peer_name.clone()
                            };
                            let (ligne, erreur, question) = match &o.result {
                                Ok(s) => {
                                    let question = s.accounts_removed_there.first().map(|a| {
                                        a_retirer
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .push(a.clone());
                                        format!("{nom} removed {a}. Remove it here too?")
                                    });
                                    (summary_line(&nom, s), String::new(), question)
                                }
                                Err(e) => (
                                    String::new(),
                                    format!("The sync with {nom} failed: {e}"),
                                    None,
                                ),
                            };
                            let _ = faible.upgrade_in_event_loop(move |f| {
                                f.set_pair_peer(nom.into());
                                f.set_pair_summary(ligne.clone().into());
                                f.set_pair_error(erreur.clone().into());
                                f.set_device_summary(
                                    if ligne.is_empty() { erreur } else { ligne }.into(),
                                );
                                if let Some(q) = question {
                                    f.set_device_question(q.into());
                                }
                            });
                        }
                    }
                }
            });
        })
    };
    if !services.store.peers().unwrap_or_default().is_empty() {
        demarrer();
    }

    // The code as text, for a device without a camera.
    let dernier_code: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    {
        let dernier_code = Arc::clone(&dernier_code);
        fenetre.on_pair_copy_code(move || {
            let code = dernier_code
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if code.is_empty() {
                return;
            }
            match crate::presse::Clipboard::new().and_then(|mut c| c.set_text(code)) {
                Ok(()) => {}
                Err(e) => tracing::warn!(error = %e, "copying the pairing code"),
            }
        });
    }
    // Joining a device by its code pasted here: this one is the "phone".
    {
        let (pairs, faible, demarrer) = (Arc::clone(&pairs), faible.clone(), Arc::clone(&demarrer));
        let rt = runtime.clone();
        fenetre.on_pair_join(move |code| {
            let code = code.trim().to_string();
            if iris_peer::Invite::parse(&code).is_err() {
                if let Some(f) = faible.upgrade() {
                    f.set_device_summary("This is not an Iris pairing code.".into());
                }
                return;
            }
            demarrer();
            let pairs = Arc::clone(&pairs);
            let faible = faible.clone();
            rt.spawn(async move {
                for _ in 0..40 {
                    if pairs.get().is_some() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let Some(p) = pairs.get().cloned() else {
                    return;
                };
                // The outcome reaches the window through the events, like any sync.
                if p.pair_with(&code, iris_peer::Wants::default())
                    .await
                    .is_ok()
                {
                    let _ = faible.upgrade_in_event_loop(|f| f.set_join_code("".into()));
                }
            });
        });
    }

    // Pairing: a fresh code each time the panel opens.
    {
        let pairs = Arc::clone(&pairs);
        let faible = faible.clone();
        let demarrer = Arc::clone(&demarrer);
        let dernier_code = Arc::clone(&dernier_code);
        let rt = runtime.clone();
        fenetre.on_pair_requested(move || {
            let Some(f) = faible.upgrade() else { return };
            f.set_pair_digits("".into());
            f.set_pair_peer("".into());
            f.set_pair_summary("".into());
            f.set_pair_error("".into());
            f.set_pair_code(slint::Image::default());
            demarrer();
            let (pairs, faible, dernier_code) = (
                Arc::clone(&pairs),
                faible.clone(),
                Arc::clone(&dernier_code),
            );
            rt.spawn(async move {
                // The listener starts in its own task: wait for it a moment.
                for _ in 0..40 {
                    if pairs.get().is_some() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let issue = match (pairs.get(), local_address()) {
                    (Some(p), Some(ip)) => p
                        .open_pairing(ip)
                        .map(|invite| {
                            let uri = invite.to_uri();
                            *dernier_code.lock().unwrap_or_else(|e| e.into_inner()) = uri.clone();
                            (qr_pixels(&uri), ip.to_string())
                        })
                        .map_err(|e| e.to_string()),
                    (None, _) => Err("Iris could not listen on the network.".to_string()),
                    (_, None) => {
                        Err("Iris is not on a network: connect to the Wi-Fi first.".to_string())
                    }
                };
                let _ = faible.upgrade_in_event_loop(move |f| match issue {
                    Ok((pixels, ou)) => {
                        if let Some(p) = pixels {
                            f.set_pair_code(slint::Image::from_rgba8(p));
                        }
                        f.set_pair_where(ou.into());
                    }
                    Err(e) => f.set_pair_error(e.into()),
                });
            });
        });
    }
    {
        let pairs = Arc::clone(&pairs);
        fenetre.on_pair_closed(move || {
            if let Some(p) = pairs.get() {
                p.close_pairing();
            }
        });
    }

    // Sync now, with a device seen on the network.
    {
        let (pairs, vus) = (Arc::clone(&pairs), Arc::clone(&vus));
        let rt = runtime.clone();
        fenetre.on_sync_device(move |id| {
            let id = id.to_string();
            let addr = vus
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&id)
                .map(|(a, _)| *a);
            let (Some(p), Some(addr)) = (pairs.get().cloned(), addr) else {
                return;
            };
            rt.spawn(async move {
                if let Err(e) = p.sync_with(&id, addr).await {
                    tracing::info!(error = %e, "sync with another device");
                }
            });
        });
    }

    // An account removed on the other device: removed here only if the person says so.
    {
        let (services, a_retirer, faible) = (
            Arc::clone(&services),
            Arc::clone(&a_retirer),
            faible.clone(),
        );
        fenetre.on_device_answered(move |oui| {
            let adresse = {
                let mut v = a_retirer.lock().unwrap_or_else(|e| e.into_inner());
                (!v.is_empty()).then(|| v.remove(0))
            };
            if let (true, Some(adresse)) = (oui, &adresse) {
                let compte = services
                    .store
                    .accounts()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|c| c.email.eq_ignore_ascii_case(adresse));
                if let Some(c) = compte {
                    if let Err(e) = crate::accounts::remove_account(
                        &services.store,
                        services.secrets.as_ref(),
                        c.id,
                    ) {
                        tracing::warn!(error = %e, "removing an account removed elsewhere");
                    }
                }
            }
            if let Some(f) = faible.upgrade() {
                f.set_device_question("".into());
            }
        });
    }
}

async fn auto_sync(pairs: &Arc<OnceLock<Peers>>, services: &Arc<Base>, id: &str, addr: SocketAddr) {
    let Some(p) = pairs.get().cloned() else {
        return;
    };
    let dernier = services
        .store
        .peers()
        .unwrap_or_default()
        .into_iter()
        .find(|(i, _, _)| i == id)
        .map(|(_, _, vu)| vu);
    let Some(vu) = dernier else {
        // Seen, but not paired.
        return;
    };
    let maintenant = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if maintenant - vu < AUTO_SYNC.as_secs() as i64 {
        return;
    }
    if let Err(e) = p.sync_with(id, addr).await {
        tracing::info!(error = %e, "sync with another device");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_names_what_changed_on_both() {
        let s = iris_peer::Summary {
            inserted: 12,
            updated: 3,
            conflicts: vec!["Work".into()],
            ..Default::default()
        };
        assert_eq!(
            summary_line("PC-Marie", &s),
            "Synced with PC-Marie: 12 added, 3 changed. Changed on both, the newer kept: Work."
        );
        assert_eq!(
            summary_line("PC-Marie", &iris_peer::Summary::default()),
            "Synced with PC-Marie: nothing new."
        );
    }

    #[test]
    fn a_device_seen_is_on_this_wifi() {
        assert_eq!(
            status_line(true, 1000, 1000 + 7200),
            "On this Wi-Fi · synced 2 h ago"
        );
        assert_eq!(
            status_line(false, 1000, 1000 + 3 * 86_400),
            "Synced 3 days ago"
        );
        assert_eq!(status_line(false, 0, 5), "Never synced");
    }

    #[test]
    fn the_code_is_an_image_with_its_margin() {
        let p = qr_pixels("iris-pair:1?h=192.168.1.20&p=51834&d=ab&n=PC&k=00").unwrap();
        assert_eq!(p.width(), p.height());
        assert!(
            p.width() >= 29 + 8,
            "a version 3 code and its margin: {}",
            p.width()
        );
    }
}
