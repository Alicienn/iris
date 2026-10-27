//! Iris on the other devices of the same network: pairing and syncing with them.
//!
//! A computer opens a pairing window: a fresh invite, shown as a QR code, good for one
//! connection within five minutes. The phone that scans it connects with the invite's
//! key; the computer then hands it a long-term secret, which both keep in their vault
//! (`device/<id>`), and the first exchange runs. Later exchanges open with that secret.
//! What an exchange carries and how it merges is `iris-peer`'s and `iris-store`'s; this
//! module listens, connects, and is the app's side of the exchange.

use iris_peer::{
    exchange, pack, unpack, Invite, ManifestEntry, Msg, NoteCopy, PeerError, Session, Side,
    Summary, Wants,
};
use iris_secrets::{Secret, SecretKind, SecretStore};
use iris_store::{ChangeSet, Store};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

/// How long a pairing code stays good.
pub const PAIRING_TIME: Duration = Duration::from_secs(5 * 60);

/// What one exchange ended with, for the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub peer_id: String,
    pub peer_name: String,
    pub result: Result<Summary, String>,
}

/// What the window hears of the devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A device came in with the pairing code: the six digits both show.
    Connected {
        digits: String,
    },
    Done(Outcome),
}

struct Pairing {
    invite: Invite,
    made: Instant,
    used: bool,
}

struct Inner {
    store: Arc<Store>,
    secrets: Arc<dyn SecretStore>,
    name: String,
    addr: SocketAddr,
    pairing: Mutex<Option<Pairing>>,
    events: tokio::sync::mpsc::UnboundedSender<Event>,
    notes_root: Mutex<Option<std::path::PathBuf>>,
}

/// Listening for the paired devices, and reaching them.
#[derive(Clone)]
pub struct Peers {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Peers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Peers")
            .field("addr", &self.inner.addr)
            .finish()
    }
}

fn secret_key(device_id: &str) -> String {
    format!("device/{device_id}")
}

impl Peers {
    /// Listens on `bind` (port 0: any) and reports each exchange's outcome.
    pub async fn start(
        store: Arc<Store>,
        secrets: Arc<dyn SecretStore>,
        name: String,
        bind: SocketAddr,
    ) -> std::io::Result<(Peers, tokio::sync::mpsc::UnboundedReceiver<Event>)> {
        let ecoute = TcpListener::bind(bind).await?;
        let addr = ecoute.local_addr()?;
        let (envoi, recu) = tokio::sync::mpsc::unbounded_channel();
        let peers = Peers {
            inner: Arc::new(Inner {
                store,
                secrets,
                name,
                addr,
                pairing: Mutex::new(None),
                events: envoi,
                notes_root: Mutex::new(None),
            }),
        };
        let p = peers.clone();
        tokio::spawn(async move {
            while let Ok((flux, _)) = ecoute.accept().await {
                let p = p.clone();
                tokio::spawn(async move { p.accepted(flux).await });
            }
        });
        Ok((peers, recu))
    }

    /// Where this device's note spaces are: they travel with each exchange.
    pub fn set_notes_root(&self, root: Option<std::path::PathBuf>) {
        *self
            .inner
            .notes_root
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = root;
    }

    /// The port this device listens on, for the invite and the announce.
    pub fn port(&self) -> u16 {
        self.inner.addr.port()
    }

    /// A fresh invite at `host`, good for one device within five minutes. Opening
    /// another closes the one before.
    pub fn open_pairing(&self, host: std::net::IpAddr) -> Result<Invite, PeerError> {
        let invite = Invite::new(
            host,
            self.port(),
            &self.inner.store.device_id(),
            &self.inner.name,
        )?;
        *self.inner.pairing.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pairing {
            invite: invite.clone(),
            made: Instant::now(),
            used: false,
        });
        Ok(invite)
    }

    /// Closes the pairing window: its code no longer opens anything.
    pub fn close_pairing(&self) {
        *self.inner.pairing.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The invite's key, while it is still good and was not used.
    fn invite_key(&self) -> Option<[u8; 32]> {
        let garde = self.inner.pairing.lock().unwrap_or_else(|e| e.into_inner());
        let p = garde.as_ref()?;
        (!p.used && p.made.elapsed() < PAIRING_TIME).then_some(p.invite.key)
    }

    fn long_term(&self, device_id: &str) -> Option<[u8; 32]> {
        let s = self
            .inner
            .secrets
            .get(&secret_key(device_id), SecretKind::Password)
            .ok()??;
        let octets = hex::decode(s.expose()).ok()?;
        <[u8; 32]>::try_from(octets.as_slice()).ok()
    }

    async fn accepted(&self, flux: tokio::net::TcpStream) {
        let invite = self.invite_key();
        let moi = self.clone();
        let secret_for = move |id: &str| moi.long_term(id).or(invite);
        let (mut session, id) = match Session::accept(flux, &secret_for).await {
            Ok(s) => s,
            Err(e) => {
                tracing::info!(error = %e, "a device was refused");
                return;
            }
        };
        // A device without a secret of its own came in with the invite: pair it.
        if self.long_term(&id).is_none() {
            {
                let mut garde = self.inner.pairing.lock().unwrap_or_else(|e| e.into_inner());
                match garde.as_mut() {
                    Some(p) if !p.used => p.used = true,
                    _ => return,
                }
            }
            let secret = match iris_peer::random_key() {
                Ok(s) => s,
                Err(e) => return self.report(&id, Err(e.to_string())),
            };
            let _ = self.inner.events.send(Event::Connected {
                digits: iris_peer::check_digits(session.key()),
            });
            let hexa = hex::encode(secret);
            if let Err(e) = self.keep_secret(&id, &hexa) {
                return self.report(&id, Err(e));
            }
            if let Err(e) = session
                .send(&Msg::Pair {
                    long_term_secret_hex: hexa,
                })
                .await
            {
                return self.report(&id, Err(e.to_string()));
            }
        }
        let r = self.run(&mut session, Wants::default()).await;
        self.report(&id, r.map_err(|e| e.to_string()));
    }

    fn keep_secret(&self, device_id: &str, hexa: &str) -> Result<(), String> {
        self.inner
            .secrets
            .set(
                &secret_key(device_id),
                SecretKind::Password,
                &Secret::new(hexa),
            )
            .map_err(|e| e.to_string())
    }

    /// The phone's side of pairing: connect with the scanned code, keep the secret
    /// the computer hands over, and sync.
    pub async fn pair_with(&self, uri: &str, wants: Wants) -> Result<Summary, PeerError> {
        let invite = Invite::parse(uri)?;
        let addr = SocketAddr::new(invite.host, invite.port);
        let mut session =
            Session::connect(addr, &self.inner.store.device_id(), &invite.key).await?;
        match session.recv().await? {
            Msg::Pair {
                long_term_secret_hex,
            } => self
                .keep_secret(&invite.device_id, &long_term_secret_hex)
                .map_err(PeerError::Exchange)?,
            m => return Err(PeerError::Exchange(format!("unexpected {m:?}"))),
        }
        let r = self.run(&mut session, wants).await;
        self.report(
            &invite.device_id,
            r.as_ref().map(Clone::clone).map_err(|e| e.to_string()),
        );
        r
    }

    /// Syncs now with a paired device seen at `addr`.
    pub async fn sync_with(&self, device_id: &str, addr: SocketAddr) -> Result<Summary, PeerError> {
        let secret = self.long_term(device_id).ok_or(PeerError::Refused)?;
        let mut session = Session::connect(addr, &self.inner.store.device_id(), &secret).await?;
        let r = self.run(&mut session, Wants::default()).await;
        self.report(
            device_id,
            r.as_ref().map(Clone::clone).map_err(|e| e.to_string()),
        );
        r
    }

    async fn run(&self, session: &mut Session, wants: Wants) -> Result<Summary, PeerError> {
        let mut cote = AppSide {
            store: Arc::clone(&self.inner.store),
            secrets: Arc::clone(&self.inner.secrets),
            name: self.inner.name.clone(),
            settings: Vec::new(),
            notes_root: self
                .inner
                .notes_root
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            mine: Mutex::new(Vec::new()),
        };
        let bilan = exchange(session, &mut cote, wants).await?;
        if !cote.settings.is_empty() {
            crate::settings::apply_remote(&cote.settings);
        }
        Ok(bilan)
    }

    fn report(&self, peer_id: &str, result: Result<Summary, String>) {
        let peer_name = self
            .inner
            .store
            .peers()
            .ok()
            .and_then(|v| v.into_iter().find(|(id, _, _)| id == peer_id))
            .map(|(_, n, _)| n)
            .unwrap_or_default();
        let _ = self.inner.events.send(Event::Done(Outcome {
            peer_id: peer_id.to_string(),
            peer_name,
            result,
        }));
    }
}

/// The app's side of an exchange: the store and the vault.
struct AppSide {
    store: Arc<Store>,
    secrets: Arc<dyn SecretStore>,
    name: String,
    /// Shared settings that came, applied once the exchange is over.
    settings: Vec<(String, String)>,
    /// The notes' folder; none: notes do not travel.
    notes_root: Option<std::path::PathBuf>,
    /// This side's manifests, as sent.
    mine: Mutex<Vec<(String, Vec<ManifestEntry>)>>,
}

/// The secrets that travel with an account.
const KINDS: [SecretKind; 3] = [
    SecretKind::Password,
    SecretKind::SmtpPassword,
    SecretKind::RefreshToken,
];

impl Side for AppSide {
    fn device_id(&self) -> String {
        self.store.device_id()
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn met(&mut self, peer: &str, name: &str) {
        if let Err(e) = self.store.remember_peer(peer, name) {
            tracing::warn!(error = %e, "remembering a paired device");
        }
    }

    fn last_clock_of(&self, peer: &str) -> String {
        self.store.peer_last_clock(peer).unwrap_or_default()
    }

    fn changes_since(&self, clock: &str) -> Result<Vec<u8>, PeerError> {
        let set = self
            .store
            .changes_since(clock)
            .map_err(|e| PeerError::Exchange(e.to_string()))?;
        pack(&set)
    }

    fn apply_changes(&mut self, peer: &str, zstd_json: &[u8]) -> Result<Summary, PeerError> {
        let set: ChangeSet = unpack(zstd_json)?;
        let depuis = self.last_clock_of(peer);
        let r = self
            .store
            .apply_changes(&set, &depuis)
            .map_err(|e| PeerError::Exchange(e.to_string()))?;
        self.settings = r.settings.clone();
        Ok(Summary {
            inserted: r.inserted,
            updated: r.updated,
            deleted: r.deleted,
            conflicts: r.conflicts.into_iter().map(|c| c.label).collect(),
            accounts_removed_there: r.accounts_removed_there,
        })
    }

    fn secrets_out(&self) -> Vec<(String, String, String)> {
        let comptes = self.store.accounts().unwrap_or_default();
        let mut v = Vec::new();
        for c in comptes {
            for kind in KINDS {
                if let Ok(Some(s)) = self.secrets.get(&c.email, kind) {
                    v.push((
                        c.email.clone(),
                        kind.as_str().to_string(),
                        s.expose().to_string(),
                    ));
                }
            }
        }
        v
    }

    fn secrets_in(&mut self, entries: Vec<(String, String, String)>) {
        for (compte, genre, valeur) in entries {
            let Some(kind) = SecretKind::parse(&genre).filter(|k| KINDS.contains(k)) else {
                continue;
            };
            if let Err(e) = self.secrets.set(&compte, kind, &Secret::new(valeur)) {
                tracing::warn!(error = %e, "a secret from another device");
            }
        }
    }

    fn manifests(&self) -> Vec<(String, Vec<ManifestEntry>)> {
        let Some(racine) = &self.notes_root else {
            return Vec::new();
        };
        let v = spaces_of(racine)
            .into_iter()
            .map(|(nom, dir)| {
                let mut entrees = Vec::new();
                walk(&dir, "", &mut entrees);
                (nom, entrees)
            })
            .collect::<Vec<_>>();
        *self.mine.lock().unwrap_or_else(|e| e.into_inner()) = v.clone();
        v
    }

    fn plan_notes(&self, peer: &str, space: &str, there: &[ManifestEntry]) -> Vec<NoteCopy> {
        if self.notes_root.is_none() || !safe_space(space) {
            return Vec::new();
        }
        let ici = self
            .mine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(s, _)| s == space)
            .map(|(_, e)| e.clone())
            .unwrap_or_default();
        let avant = self.last_notes(peer).remove(space).unwrap_or_default();
        let la: Vec<ManifestEntry> = there
            .iter()
            .filter(|e| iris_peer::notes::safe_path(&e.path))
            .cloned()
            .collect();
        iris_peer::notes::plan_copies(&ici, &la, &avant)
    }

    fn read_file(&self, space: &str, path: &str) -> std::io::Result<(i64, Vec<u8>)> {
        let chemin = self.file(space, path)?;
        let octets = std::fs::read(&chemin)?;
        Ok((mtime_ms(&chemin), octets))
    }

    fn write_file(
        &mut self,
        space: &str,
        path: &str,
        _m: i64,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        let chemin = self.file(space, path)?;
        // A space made on the other device is made here first, as Iris makes one.
        if let Some(racine) = &self.notes_root {
            if !racine
                .join(space)
                .join(".iris")
                .join("space.json")
                .is_file()
            {
                iris_vault::Vault::open(racine.clone())
                    .and_then(|v| v.create_space(space))
                    .map_err(|e| std::io::Error::other(e.to_string()))?;
            }
        }
        if let Some(d) = chemin.parent() {
            std::fs::create_dir_all(d)?;
        }
        // Next to it, then over it: a cut never leaves half a note.
        let provisoire = chemin.with_extension("iris-tmp");
        std::fs::write(&provisoire, bytes)?;
        std::fs::rename(&provisoire, &chemin)
    }

    fn delete_file(&mut self, space: &str, path: &str) -> std::io::Result<()> {
        let chemin = self.file(space, path)?;
        if !chemin.exists() {
            return Ok(());
        }
        // To the Recycle Bin, where it can be brought back.
        iris_vault::RecycleBin::throw(&crate::notes::bin::SystemBin, &chemin)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    fn keep_revision(&mut self, space: &str, path: &str) {
        let Ok(chemin) = self.file(space, path) else {
            return;
        };
        let Ok(texte) = std::fs::read_to_string(&chemin) else {
            return;
        };
        let Some(dir) = self.notes_root.as_ref().map(|r| r.join(space)) else {
            return;
        };
        if let Ok(s) = iris_vault::Space::open(dir) {
            let _ = s.save_version(path, &texte, mtime_ms(&chemin));
        }
    }

    fn notes_synced(&mut self, peer: &str, space: &str, entries: Vec<ManifestEntry>) {
        let mut tout = self.last_notes(peer);
        tout.insert(space.to_string(), entries);
        if let Ok(json) = serde_json::to_string(&tout) {
            let _ = self.store.set_peer_notes_manifest(peer, &json);
        }
    }
}

impl AppSide {
    fn last_notes(&self, peer: &str) -> std::collections::BTreeMap<String, Vec<ManifestEntry>> {
        self.store
            .peer_notes_manifest(peer)
            .ok()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    /// A file of a space, refused when the names would leave the notes' folder.
    fn file(&self, space: &str, path: &str) -> std::io::Result<std::path::PathBuf> {
        let racine = self
            .notes_root
            .as_ref()
            .ok_or_else(|| std::io::Error::other("no notes here"))?;
        if !safe_space(space) || !iris_peer::notes::safe_path(path) {
            return Err(std::io::Error::other("a path outside the notes"));
        }
        let mut p = racine.join(space);
        for part in path.split('/') {
            p.push(part);
        }
        Ok(p)
    }
}

/// A space's name from the other device: one plain folder name.
fn safe_space(space: &str) -> bool {
    iris_peer::notes::safe_path(space) && !space.contains('/')
}

/// The spaces of the notes' folder that travel: those Iris made there, by name.
fn spaces_of(racine: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let Ok(lecture) = std::fs::read_dir(racine) else {
        return Vec::new();
    };
    let mut v: Vec<_> = lecture
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join(".iris").join("space.json").is_file())
        .filter_map(|p| Some((p.file_name()?.to_str()?.to_string(), p)))
        .filter(|(n, _)| safe_space(n))
        .collect();
    v.sort();
    v
}

/// Every file of a space but its hidden folders (`.iris`, `.trash`) and temporaries.
fn walk(dir: &std::path::Path, prefixe: &str, sortie: &mut Vec<ManifestEntry>) {
    let Ok(lecture) = std::fs::read_dir(dir) else {
        return;
    };
    for e in lecture.filter_map(|e| e.ok()) {
        let p = e.path();
        let Some(nom) = p.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if nom.starts_with('.') || nom.ends_with(".iris-tmp") {
            continue;
        }
        let rel = if prefixe.is_empty() {
            nom
        } else {
            format!("{prefixe}/{nom}")
        };
        if p.is_dir() {
            walk(&p, &rel, sortie);
        } else if let Ok(octets) = std::fs::read(&p) {
            sortie.push(iris_peer::notes::entry_for(&rel, &octets, mtime_ms(&p)));
        }
    }
}

fn mtime_ms(p: &std::path::Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Default)]
    struct Coffre(Mutex<HashMap<(String, SecretKind), String>>);

    impl SecretStore for Coffre {
        fn set(&self, account: &str, kind: SecretKind, value: &Secret) -> iris_types::Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert((account.into(), kind), value.expose().into());
            Ok(())
        }
        fn get(&self, account: &str, kind: SecretKind) -> iris_types::Result<Option<Secret>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&(account.to_string(), kind))
                .map(Secret::new))
        }
        fn delete(&self, account: &str, kind: SecretKind) -> iris_types::Result<bool> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .remove(&(account.to_string(), kind))
                .is_some())
        }
        fn backend(&self) -> &'static str {
            "test"
        }
    }

    async fn appareil(nom: &str) -> (Peers, Arc<Store>, Arc<Coffre>) {
        let store = Arc::new(Store::in_memory().unwrap());
        let coffre = Arc::new(Coffre::default());
        let (p, _) = Peers::start(
            Arc::clone(&store),
            coffre.clone(),
            nom.into(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .await
        .unwrap();
        (p, store, coffre)
    }

    #[tokio::test]
    async fn a_phone_pairs_by_the_code_and_gets_the_computers_tasks_and_passwords() {
        let (pc, store_pc, coffre_pc) = appareil("PC").await;
        let (tel, store_tel, coffre_tel) = appareil("Phone").await;
        store_pc
            .create_task_list("Courses", "#000", iris_types::Timestamp::from_millis(0))
            .unwrap();
        store_pc
            .create_account(
                &iris_store::NewAccount::new(
                    "marie@example.com",
                    "imap.example.com",
                    "smtp.example.com",
                ),
                iris_types::Timestamp::from_millis(0),
            )
            .unwrap();
        coffre_pc
            .set("marie@example.com", SecretKind::Password, &Secret::new("x"))
            .unwrap();

        let code = pc.open_pairing("127.0.0.1".parse().unwrap()).unwrap();
        let bilan = tel
            .pair_with(&code.to_uri(), Wants::default())
            .await
            .unwrap();
        assert!(bilan.inserted >= 2, "{bilan:?}");
        assert!(store_tel
            .task_lists()
            .unwrap()
            .iter()
            .any(|l| l.name == "Courses"));
        assert_eq!(
            coffre_tel
                .get("marie@example.com", SecretKind::Password)
                .unwrap()
                .map(|s| s.expose().to_string()),
            Some("x".into())
        );
        assert_eq!(store_tel.peers().unwrap()[0].1, "PC");

        // The code was used: it opens nothing again.
        assert!(tel
            .pair_with(&code.to_uri(), Wants::default())
            .await
            .is_err());
        // The secret they now share opens the next exchange.
        let addr: SocketAddr = format!("127.0.0.1:{}", pc.port()).parse().unwrap();
        tel.sync_with(&store_pc.device_id(), addr).await.unwrap();
    }

    #[tokio::test]
    async fn a_note_made_on_one_device_reaches_the_other_and_a_deletion_follows() {
        let (pc, store_pc, _) = appareil("PC").await;
        let (tel, _, _) = appareil("Phone").await;
        let (d_pc, d_tel) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let espace = iris_vault::Vault::open(d_pc.path().to_path_buf())
            .unwrap()
            .create_space("Cours")
            .unwrap();
        espace.create_note("", "Chapitre 2", "# Espaces").unwrap();
        pc.set_notes_root(Some(d_pc.path().to_path_buf()));
        tel.set_notes_root(Some(d_tel.path().to_path_buf()));

        let code = pc.open_pairing("127.0.0.1".parse().unwrap()).unwrap();
        tel.pair_with(&code.to_uri(), Wants::default())
            .await
            .unwrap();
        let recu = d_tel.path().join("Cours").join("Chapitre 2.md");
        assert_eq!(std::fs::read_to_string(&recu).unwrap(), "# Espaces");
        assert!(
            d_tel.path().join("Cours/.iris/space.json").is_file(),
            "made a space here, as Iris makes one"
        );

        // Deleted on the phone: gone from the computer at the next sync.
        std::fs::remove_file(&recu).unwrap();
        let addr: SocketAddr = format!("127.0.0.1:{}", pc.port()).parse().unwrap();
        tel.sync_with(&store_pc.device_id(), addr).await.unwrap();
        // The deletion is carried out by the PC's side, after the exchange returns
        // here: give its task a moment.
        for _ in 0..50 {
            if !d_pc.path().join("Cours/Chapitre 2.md").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!d_pc.path().join("Cours/Chapitre 2.md").exists());
    }

    #[tokio::test]
    async fn a_code_too_old_opens_nothing() {
        let (pc, _, _) = appareil("PC").await;
        let (tel, _, _) = appareil("Phone").await;
        let code = pc.open_pairing("127.0.0.1".parse().unwrap()).unwrap();
        {
            let mut garde = pc.inner.pairing.lock().unwrap();
            let p = garde.as_mut().unwrap();
            p.made = p.made.checked_sub(PAIRING_TIME).unwrap_or(p.made);
        }
        assert!(tel
            .pair_with(&code.to_uri(), Wants::default())
            .await
            .is_err());
    }
}
