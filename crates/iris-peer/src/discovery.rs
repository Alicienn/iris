//! Finding the other copies of Iris on the same network.
//!
//! Each device announces itself over mDNS as `<device id>._iris-sync._tcp.local.` with
//! the port it listens on, and nothing else: no name, no account. A device seen is
//! reported with its address; whether it is paired, and whether to sync, is the app's.

use crate::PeerError;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::{IpAddr, SocketAddr};

const SERVICE: &str = "_iris-sync._tcp.local.";

/// A device of the network that announced itself, or left it (`addr` is `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub device_id: String,
    pub addr: Option<SocketAddr>,
}

/// Announcing this device and watching for the others, until dropped.
pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discovery")
            .field("fullname", &self.fullname)
            .finish()
    }
}

impl Discovery {
    /// Announces `device_id` on `port` at the addresses of this machine, and reports
    /// every other device that does the same.
    pub fn start(
        device_id: &str,
        port: u16,
    ) -> Result<(Discovery, tokio::sync::mpsc::Receiver<Seen>), PeerError> {
        let erreur = |e: mdns_sd::Error| PeerError::Io(std::io::Error::other(e.to_string()));
        let daemon = ServiceDaemon::new().map_err(erreur)?;
        let hote = format!("{device_id}.local.");
        let info = ServiceInfo::new(
            SERVICE,
            device_id,
            &hote,
            "",
            port,
            None::<std::collections::HashMap<String, String>>,
        )
        .map_err(erreur)?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        daemon.register(info).map_err(erreur)?;
        let evenements = daemon.browse(SERVICE).map_err(erreur)?;

        let (envoi, recu) = tokio::sync::mpsc::channel(16);
        let moi = device_id.to_string();
        std::thread::Builder::new()
            .name("iris-peer-discovery".into())
            .spawn(move || {
                while let Ok(e) = evenements.recv() {
                    // Gone from the network (it said goodbye, or its announce ran out):
                    // reported with no address.
                    if let ServiceEvent::ServiceRemoved(_, nom) = &e {
                        if let Some(id) = device_of(nom).filter(|id| *id != moi) {
                            let parti = Seen {
                                device_id: id,
                                addr: None,
                            };
                            if envoi.blocking_send(parti).is_err() {
                                break;
                            }
                        }
                        continue;
                    }
                    let ServiceEvent::ServiceResolved(info) = e else {
                        continue;
                    };
                    let Some(id) = device_of(info.get_fullname()) else {
                        continue;
                    };
                    if id == moi {
                        continue;
                    }
                    // IPv4 first: the phone and the computer share a Wi-Fi, rarely
                    // an IPv6 route.
                    let mut adresses: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                    adresses.sort_by_key(|a| a.is_ipv6());
                    let Some(ip) = adresses.first() else {
                        continue;
                    };
                    let vu = Seen {
                        device_id: id,
                        addr: Some(SocketAddr::new(*ip, info.get_port())),
                    };
                    if envoi.blocking_send(vu).is_err() {
                        break;
                    }
                }
            })?;
        Ok((Discovery { daemon, fullname }, recu))
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// The device id in `<id>._iris-sync._tcp.local.`.
fn device_of(fullname: &str) -> Option<String> {
    let id = fullname.strip_suffix(&format!(".{SERVICE}"))?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit())).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_is_named_by_its_service() {
        assert_eq!(
            device_of("a1b2c3d4e5f60718._iris-sync._tcp.local.").as_deref(),
            Some("a1b2c3d4e5f60718")
        );
        assert_eq!(device_of("printer._ipp._tcp.local."), None);
        assert_eq!(device_of("not hex._iris-sync._tcp.local."), None);
    }
}
