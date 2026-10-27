//! The pairing code: where the computer listens, who it is, and a key for once.

use crate::PeerError;
use std::net::IpAddr;

/// What a QR code carries, as the URI
/// `iris-pair:1?h=192.168.1.20&p=51834&d=<device id>&n=PC-Marie&k=<64 hex>`.
///
/// The key is random, good for one connection within five minutes: the listener that
/// made it enforces both. The invite itself carries no time, so that a code shown is
/// the same code however long it stays on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub host: IpAddr,
    pub port: u16,
    pub device_id: String,
    pub name: String,
    pub key: [u8; 32],
}

const PREFIXE: &str = "iris-pair:1?";

/// 32 random bytes: an invite's key, or the secret two paired devices keep.
pub fn random_key() -> Result<[u8; 32], PeerError> {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| PeerError::Invite(e.to_string()))?;
    Ok(key)
}

impl Invite {
    /// A fresh invite with a random key.
    pub fn new(host: IpAddr, port: u16, device_id: &str, name: &str) -> Result<Self, PeerError> {
        Ok(Self {
            host,
            port,
            device_id: device_id.to_string(),
            name: name.to_string(),
            key: random_key()?,
        })
    }

    pub fn to_uri(&self) -> String {
        format!(
            "{PREFIXE}h={}&p={}&d={}&n={}&k={}",
            self.host,
            self.port,
            encode(&self.device_id),
            encode(&self.name),
            hex::encode(self.key)
        )
    }

    pub fn parse(uri: &str) -> Result<Self, PeerError> {
        let mauvais = |quoi: &str| PeerError::Invite(quoi.to_string());
        let reste = uri
            .trim()
            .strip_prefix(PREFIXE)
            .ok_or_else(|| mauvais("prefix"))?;
        let (mut host, mut port, mut device, mut name, mut key) = (None, None, None, None, None);
        for paire in reste.split('&') {
            let (cle, valeur) = paire.split_once('=').ok_or_else(|| mauvais(paire))?;
            match cle {
                "h" => host = valeur.parse::<IpAddr>().ok(),
                "p" => port = valeur.parse::<u16>().ok(),
                "d" => device = decode(valeur),
                "n" => name = decode(valeur),
                "k" => {
                    let octets = hex::decode(valeur).map_err(|_| mauvais("key"))?;
                    key = <[u8; 32]>::try_from(octets.as_slice()).ok();
                }
                _ => {}
            }
        }
        Ok(Self {
            host: host.ok_or_else(|| mauvais("host"))?,
            port: port.filter(|p| *p != 0).ok_or_else(|| mauvais("port"))?,
            device_id: device
                .filter(|d| !d.is_empty())
                .ok_or_else(|| mauvais("device"))?,
            name: name.unwrap_or_default(),
            key: key.ok_or_else(|| mauvais("key"))?,
        })
    }
}

/// Percent-encodes what is not a letter, a digit or `-._~`.
fn encode(texte: &str) -> String {
    let mut sortie = String::new();
    for octet in texte.bytes() {
        if octet.is_ascii_alphanumeric() || b"-._~".contains(&octet) {
            sortie.push(octet as char);
        } else {
            sortie.push_str(&format!("%{octet:02X}"));
        }
    }
    sortie
}

fn decode(texte: &str) -> Option<String> {
    let octets = texte.as_bytes();
    let mut sortie = Vec::with_capacity(octets.len());
    let mut i = 0;
    while i < octets.len() {
        if octets[i] == b'%' {
            let h = std::str::from_utf8(octets.get(i + 1..i + 3)?).ok()?;
            sortie.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            sortie.push(octets[i]);
            i += 1;
        }
    }
    String::from_utf8(sortie).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invite_goes_through_its_uri_and_back() {
        let i = Invite::new(
            "192.168.1.20".parse().unwrap(),
            51834,
            "a1b2c3d4e5f60718",
            "PC de Marie — bureau",
        )
        .unwrap();
        let uri = i.to_uri();
        assert!(uri.starts_with("iris-pair:1?h=192.168.1.20&p=51834&"));
        assert!(!uri.contains(' '), "{uri}");
        assert_eq!(Invite::parse(&uri).unwrap(), i);
    }

    #[test]
    fn what_is_not_an_invite_is_refused() {
        assert!(Invite::parse("https://example.com").is_err());
        assert!(
            Invite::parse("iris-pair:1?h=10.0.0.1&p=1&d=x&k=00").is_err(),
            "short key"
        );
        assert!(Invite::parse("iris-pair:1?h=nope&p=1&d=x&k=").is_err());
        let sans_appareil = format!("iris-pair:1?h=10.0.0.1&p=9&k={}", "ab".repeat(32));
        assert!(Invite::parse(&sans_appareil).is_err());
    }

    #[test]
    fn two_invites_never_share_a_key() {
        let h: IpAddr = "10.0.0.2".parse().unwrap();
        assert_ne!(
            Invite::new(h, 1, "d", "n").unwrap().key,
            Invite::new(h, 1, "d", "n").unwrap().key
        );
    }
}
