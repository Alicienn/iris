//! A sealed conversation between two devices that hold the same secret.
//!
//! The device that connects says who it is and sends 32 random bytes; the one that
//! listens finds the secret it shares with it (the invite's key when pairing, the
//! long-term secret after) and answers with 32 random bytes of its own. Both derive the
//! session key from the secret and the two nonces, and the first frame each sends
//! proves it: a frame that does not open ends the session. Every frame is
//! XChaCha20Poly1305 under that key, its nonce the direction and a counter, never sent.

use crate::protocol::Msg;
use crate::PeerError;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A frame larger than this is refused before it is read: 64 MiB.
const FRAME_MAX: u32 = 64 * 1024 * 1024;
const CONTEXTE: &[u8] = b"iris-sync 1";
/// How long a connecting device has to prove itself.
const HANDSHAKE_TIME: std::time::Duration = std::time::Duration::from_secs(15);

/// The six digits both devices show, to be compared by eye: "482 913".
pub fn check_digits(session_key: &[u8; 32]) -> String {
    let d = blake3::derive_key("iris-sync digits 1", session_key);
    let n = u32::from_le_bytes([d[0], d[1], d[2], d[3]]) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

pub struct Session {
    stream: TcpStream,
    chiffre: XChaCha20Poly1305,
    key: [u8; 32],
    /// 0 for frames this side sends when it connected, 1 when it listened.
    sens: u8,
    envoyes: u64,
    recus: u64,
}

impl Session {
    /// Connects to a device and proves this one holds `secret`.
    pub async fn connect(
        addr: SocketAddr,
        device_id: &str,
        secret: &[u8; 32],
    ) -> Result<Session, PeerError> {
        // A device that left the Wi-Fi does not refuse, it is silent: five seconds, not
        // the system's minute.
        let mut stream =
            tokio::time::timeout(std::time::Duration::from_secs(5), TcpStream::connect(addr))
                .await
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
        let na = aleatoire()?;
        ecrire_bloc(&mut stream, device_id.as_bytes()).await?;
        stream.write_all(&na).await?;
        let mut nb = [0u8; 32];
        stream.read_exact(&mut nb).await?;
        let mut s = Session::ouverte(stream, secret, &na, &nb, 0);
        s.prouver().await?;
        Ok(s)
    }

    /// Accepts a device that connected, if `secret_for` knows the device it says it
    /// is. Returns the session and that device's id.
    pub async fn accept(
        stream: TcpStream,
        secret_for: &(dyn Fn(&str) -> Option<[u8; 32]> + Sync),
    ) -> Result<(Session, String), PeerError> {
        // A device that connects and says nothing does not hold the listener's task.
        tokio::time::timeout(HANDSHAKE_TIME, Self::accept_now(stream, secret_for))
            .await
            .map_err(|_| PeerError::Refused)?
    }

    async fn accept_now(
        mut stream: TcpStream,
        secret_for: &(dyn Fn(&str) -> Option<[u8; 32]> + Sync),
    ) -> Result<(Session, String), PeerError> {
        let id = String::from_utf8(lire_bloc(&mut stream, 256).await?)
            .map_err(|_| PeerError::Refused)?;
        let mut na = [0u8; 32];
        stream.read_exact(&mut na).await?;
        let secret = secret_for(&id).ok_or(PeerError::Refused)?;
        let nb = aleatoire()?;
        stream.write_all(&nb).await?;
        let mut s = Session::ouverte(stream, &secret, &na, &nb, 1);
        s.prouver().await?;
        Ok((s, id))
    }

    fn ouverte(stream: TcpStream, secret: &[u8; 32], na: &[u8], nb: &[u8], sens: u8) -> Session {
        let mut materiau = Vec::with_capacity(CONTEXTE.len() + 64);
        materiau.extend_from_slice(CONTEXTE);
        materiau.extend_from_slice(na);
        materiau.extend_from_slice(nb);
        let key = *blake3::keyed_hash(secret, &materiau).as_bytes();
        Session {
            stream,
            chiffre: XChaCha20Poly1305::new_from_slice(&key).expect("a key of 32 bytes"),
            key,
            sens,
            envoyes: 0,
            recus: 0,
        }
    }

    /// Each side's first frame: it opens only under the same key.
    async fn prouver(&mut self) -> Result<(), PeerError> {
        self.send(&Msg::Proof).await?;
        match self.recv().await {
            Ok(Msg::Proof) => Ok(()),
            _ => Err(PeerError::Refused),
        }
    }

    /// The session key, for the six digits.
    pub fn key(&self) -> &[u8; 32] {
        &self.key
    }

    fn nonce(sens: u8, n: u64) -> XNonce {
        let mut octets = [0u8; 24];
        octets[0] = sens;
        octets[1..9].copy_from_slice(&n.to_le_bytes());
        *XNonce::from_slice(&octets)
    }

    pub async fn send(&mut self, msg: &Msg) -> Result<(), PeerError> {
        let clair = serde_json::to_vec(msg).map_err(|e| PeerError::Message(e.to_string()))?;
        let nonce = Session::nonce(self.sens, self.envoyes);
        let scelle = self
            .chiffre
            .encrypt(&nonce, clair.as_slice())
            .map_err(|_| PeerError::Message("sealing".into()))?;
        self.envoyes += 1;
        ecrire_bloc(&mut self.stream, &scelle).await
    }

    pub async fn recv(&mut self) -> Result<Msg, PeerError> {
        let scelle = lire_bloc(&mut self.stream, FRAME_MAX).await?;
        let nonce = Session::nonce(1 - self.sens, self.recus);
        let clair = self
            .chiffre
            .decrypt(&nonce, scelle.as_slice())
            .map_err(|_| PeerError::Refused)?;
        self.recus += 1;
        serde_json::from_slice(&clair).map_err(|e| PeerError::Message(e.to_string()))
    }
}

fn aleatoire() -> Result<[u8; 32], PeerError> {
    let mut octets = [0u8; 32];
    getrandom::fill(&mut octets).map_err(|e| PeerError::Message(e.to_string()))?;
    Ok(octets)
}

async fn ecrire_bloc(stream: &mut TcpStream, octets: &[u8]) -> Result<(), PeerError> {
    let n = u32::try_from(octets.len()).map_err(|_| PeerError::Message("too large".into()))?;
    stream.write_all(&n.to_be_bytes()).await?;
    stream.write_all(octets).await?;
    Ok(())
}

async fn lire_bloc(stream: &mut TcpStream, max: u32) -> Result<Vec<u8>, PeerError> {
    let mut entete = [0u8; 4];
    stream.read_exact(&mut entete).await?;
    let n = u32::from_be_bytes(entete);
    if n > max {
        return Err(PeerError::Message(format!("a frame of {n} bytes")));
    }
    let mut octets = vec![0u8; n as usize];
    stream.read_exact(&mut octets).await?;
    Ok(octets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    const SECRET: [u8; 32] = [7u8; 32];

    async fn paire(
        secret_ecoute: [u8; 32],
    ) -> (
        Result<Session, PeerError>,
        Result<(Session, String), PeerError>,
    ) {
        let ecoute = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let adresse = ecoute.local_addr().unwrap();
        let serveur = tokio::spawn(async move {
            let (flux, _) = ecoute.accept().await.unwrap();
            Session::accept(flux, &move |id: &str| {
                (id == "phone").then_some(secret_ecoute)
            })
            .await
        });
        let client = Session::connect(adresse, "phone", &SECRET).await;
        (client, serveur.await.unwrap())
    }

    #[tokio::test]
    async fn two_devices_with_the_same_secret_talk() {
        let (client, serveur) = paire(SECRET).await;
        let mut client = client.unwrap();
        let (mut serveur, id) = serveur.unwrap();
        assert_eq!(id, "phone");
        assert_eq!(check_digits(client.key()), check_digits(serveur.key()));

        client
            .send(&Msg::Refused {
                reason: "hello".into(),
            })
            .await
            .unwrap();
        match serveur.recv().await.unwrap() {
            Msg::Refused { reason } => assert_eq!(reason, "hello"),
            autre => panic!("{autre:?}"),
        }
        serveur.send(&Msg::Proof).await.unwrap();
        assert!(matches!(client.recv().await.unwrap(), Msg::Proof));
    }

    #[tokio::test]
    async fn another_secret_is_refused() {
        let (client, serveur) = paire([9u8; 32]).await;
        assert!(client.is_err() || serveur.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_that_says_nothing_is_let_go() {
        let ecoute = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let adresse = ecoute.local_addr().unwrap();
        let _muet = tokio::net::TcpStream::connect(adresse).await.unwrap();
        let (flux, _) = ecoute.accept().await.unwrap();
        let r = Session::accept(flux, &|_: &str| Some(SECRET)).await;
        assert!(matches!(r, Err(PeerError::Refused)));
    }

    #[test]
    fn the_digits_are_six_and_spaced() {
        let d = check_digits(&[1u8; 32]);
        assert_eq!(d.len(), 7);
        assert_eq!(d.as_bytes()[3], b' ');
        assert_ne!(d, check_digits(&[2u8; 32]));
    }
}
