//! What two devices say to each other once the session is open.
//!
//! The exchange is symmetric: each side says who it is and how far it has merged the
//! other, sends what changed since, applies what it received, then the secrets and the
//! notes if they were asked for, and both end with a summary. A side applies a batch
//! only once it holds the whole of it: a connection cut in the middle leaves it as it
//! was, and the next exchange starts again from the same point.

use crate::{PeerError, Session};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Msg {
    /// The first frame each side sends: it opens only under the right key.
    Proof,
    Hello {
        device_id: String,
        name: String,
        wants: Wants,
    },
    /// The last clock of the other side this one has merged: send what came after.
    Since {
        your_last_clock: String,
    },
    /// The long-term secret two devices keep after pairing (64 hex characters).
    Pair {
        long_term_secret_hex: String,
    },
    /// An `iris_store` change set, as JSON compressed with zstd.
    Changes {
        zstd_json: Vec<u8>,
    },
    /// Passwords and tokens: account address, kind, value.
    Secrets {
        entries: Vec<(String, String, String)>,
    },
    NotesManifest {
        space_id: String,
        entries: Vec<ManifestEntry>,
    },
    FileWanted {
        space_id: String,
        path: String,
    },
    File {
        space_id: String,
        path: String,
        mtime_ms: i64,
        zstd_bytes: Vec<u8>,
    },
    Done {
        summary: Summary,
    },
    Refused {
        reason: String,
    },
}

/// What a side wants shared, chosen at pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wants {
    pub accounts: bool,
    pub tasks: bool,
    pub calendars: bool,
    pub settings: bool,
    pub notes: bool,
}

impl Default for Wants {
    fn default() -> Self {
        Self {
            accounts: true,
            tasks: true,
            calendars: true,
            settings: true,
            notes: true,
        }
    }
}

/// A file of a space, as the other side needs to know it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub hash_hex: String,
}

/// What one side did with what it received.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub inserted: usize,
    pub updated: usize,
    pub deleted: usize,
    /// Changed on both sides, the newer kept: what it was (a task's title…).
    pub conflicts: Vec<String>,
    /// Accounts the other side removed: asked about here, never removed by a merge.
    pub accounts_removed_there: Vec<String>,
}

/// What to do with one note file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteCopy {
    Fetch(String),
    Send(String),
    DeleteHere(String),
    /// Changed on both sides: the newer wins, the other is kept as a revision.
    KeepBoth {
        path: String,
        older_is_here: bool,
    },
}

/// One side of an exchange: what it has, and what it does with what it receives. The
/// work is blocking (SQLite, files); the app's implementation runs it off the network
/// task.
pub trait Side: Send {
    fn device_id(&self) -> String;
    fn name(&self) -> String;
    /// The other side said who it is.
    fn met(&mut self, _peer: &str, _name: &str) {}
    fn last_clock_of(&self, peer: &str) -> String;
    /// zstd JSON of what changed here since `clock`.
    fn changes_since(&self, clock: &str) -> Result<Vec<u8>, PeerError>;
    /// Applies what the other side sent; records how far it got.
    fn apply_changes(&mut self, peer: &str, zstd_json: &[u8]) -> Result<Summary, PeerError>;
    fn secrets_out(&self) -> Vec<(String, String, String)>;
    fn secrets_in(&mut self, entries: Vec<(String, String, String)>);
    fn manifests(&self) -> Vec<(String, Vec<ManifestEntry>)>;
    fn plan_notes(&self, peer: &str, space_id: &str, there: &[ManifestEntry]) -> Vec<NoteCopy>;
    fn read_file(&self, space_id: &str, path: &str) -> std::io::Result<(i64, Vec<u8>)>;
    fn write_file(
        &mut self,
        space_id: &str,
        path: &str,
        mtime_ms: i64,
        bytes: &[u8],
    ) -> std::io::Result<()>;
    /// A file deleted on the other side, and not changed here since.
    fn delete_file(&mut self, _space_id: &str, _path: &str) -> std::io::Result<()> {
        Ok(())
    }
    /// A file changed on both sides is about to be replaced by the other's newer copy:
    /// keep this one as a revision.
    fn keep_revision(&mut self, _space_id: &str, _path: &str) {}
    /// The manifest of a space both sides have after this sync: `last` next time.
    fn notes_synced(&mut self, _peer: &str, _space_id: &str, _entries: Vec<ManifestEntry>) {}
}

/// A value as JSON compressed with zstd: how change sets travel.
pub fn pack<T: Serialize>(value: &T) -> Result<Vec<u8>, PeerError> {
    let json = serde_json::to_vec(value).map_err(|e| PeerError::Message(e.to_string()))?;
    Ok(zstd::encode_all(json.as_slice(), 3)?)
}

pub fn unpack<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, PeerError> {
    let json = zstd::decode_all(bytes)?;
    serde_json::from_slice(&json).map_err(|e| PeerError::Message(e.to_string()))
}

fn inattendu(m: &Msg) -> PeerError {
    PeerError::Exchange(format!("unexpected {m:?}"))
}

/// One whole exchange over an open session. Both sides run the same function.
pub async fn exchange(
    session: &mut Session,
    here: &mut dyn Side,
    wants: Wants,
) -> Result<Summary, PeerError> {
    // Who each is, and how far each has merged the other.
    let mon_id = here.device_id();
    session
        .send(&Msg::Hello {
            device_id: mon_id.clone(),
            name: here.name(),
            wants,
        })
        .await?;
    let (pair, nom_pair, ses_envies) = match session.recv().await? {
        Msg::Hello {
            device_id,
            name,
            wants,
        } => (device_id, name, wants),
        Msg::Refused { reason } => return Err(PeerError::Exchange(reason)),
        m => return Err(inattendu(&m)),
    };
    here.met(&pair, &nom_pair);
    let partage = Wants {
        accounts: wants.accounts && ses_envies.accounts,
        tasks: wants.tasks && ses_envies.tasks,
        calendars: wants.calendars && ses_envies.calendars,
        settings: wants.settings && ses_envies.settings,
        notes: wants.notes && ses_envies.notes,
    };

    // Records: each says how far it merged the other, and sends what came after
    // what the other merged.
    session
        .send(&Msg::Since {
            your_last_clock: here.last_clock_of(&pair),
        })
        .await?;
    let depuis = match session.recv().await? {
        Msg::Since { your_last_clock } => your_last_clock,
        m => return Err(inattendu(&m)),
    };
    session
        .send(&Msg::Changes {
            zstd_json: here.changes_since(&depuis)?,
        })
        .await?;
    let mut bilan = match session.recv().await? {
        Msg::Changes { zstd_json } => here.apply_changes(&pair, &zstd_json)?,
        m => return Err(inattendu(&m)),
    };

    // Secrets, when the accounts are shared.
    if partage.accounts {
        session
            .send(&Msg::Secrets {
                entries: here.secrets_out(),
            })
            .await?;
        match session.recv().await? {
            Msg::Secrets { entries } => here.secrets_in(entries),
            m => return Err(inattendu(&m)),
        }
    }

    // Notes, file by file.
    if partage.notes {
        let miens = here.manifests();
        for (espace, entrees) in &miens {
            session
                .send(&Msg::NotesManifest {
                    space_id: espace.clone(),
                    entries: entrees.clone(),
                })
                .await?;
        }
        session
            .send(&Msg::Done {
                summary: Summary::default(),
            })
            .await?;
        let mut leurs = Vec::new();
        loop {
            match session.recv().await? {
                Msg::NotesManifest { space_id, entries } => leurs.push((space_id, entries)),
                Msg::Done { .. } => break,
                m => return Err(inattendu(&m)),
            }
        }
        // What this side wants, then serve what the other wants, then receive.
        let mut voulus = 0usize;
        for (espace, entrees) in &leurs {
            let copies = here.plan_notes(&pair, espace, entrees);
            let les_miens = miens
                .iter()
                .find(|(s, _)| s == espace)
                .map(|(_, e)| e.as_slice())
                .unwrap_or_default();
            here.notes_synced(
                &pair,
                espace,
                crate::notes::after_sync(les_miens, entrees, &copies),
            );
            for copie in copies {
                let chemin = match copie {
                    NoteCopy::Fetch(p) => Some(p),
                    NoteCopy::KeepBoth {
                        path,
                        older_is_here: true,
                    } => {
                        here.keep_revision(espace, &path);
                        Some(path)
                    }
                    NoteCopy::DeleteHere(p) => {
                        here.delete_file(espace, &p)?;
                        bilan.deleted += 1;
                        None
                    }
                    _ => None,
                };
                if let Some(path) = chemin {
                    session
                        .send(&Msg::FileWanted {
                            space_id: espace.clone(),
                            path,
                        })
                        .await?;
                    voulus += 1;
                }
            }
        }
        // A space only this side has: the other fetches all of it, so both will have
        // this side's files.
        for (espace, entrees) in &miens {
            if !leurs.iter().any(|(s, _)| s == espace) {
                here.notes_synced(&pair, espace, entrees.clone());
            }
        }
        session
            .send(&Msg::Done {
                summary: Summary::default(),
            })
            .await?;
        // Answer the other's requests until its Done, receive the files asked for.
        let mut fini_demandes = false;
        let mut recus = 0usize;
        while !fini_demandes || recus < voulus {
            match session.recv().await? {
                Msg::FileWanted { space_id, path } => {
                    let (mtime_ms, octets) = here.read_file(&space_id, &path)?;
                    let zstd_bytes = zstd::encode_all(octets.as_slice(), 3)?;
                    session
                        .send(&Msg::File {
                            space_id,
                            path,
                            mtime_ms,
                            zstd_bytes,
                        })
                        .await?;
                }
                Msg::File {
                    space_id,
                    path,
                    mtime_ms,
                    zstd_bytes,
                } => {
                    let octets = zstd::decode_all(zstd_bytes.as_slice())?;
                    here.write_file(&space_id, &path, mtime_ms, &octets)?;
                    recus += 1;
                    bilan.updated += 1;
                }
                Msg::Done { .. } => fini_demandes = true,
                m => return Err(inattendu(&m)),
            }
        }
    }

    session
        .send(&Msg::Done {
            summary: bilan.clone(),
        })
        .await?;
    match session.recv().await? {
        Msg::Done { .. } => Ok(bilan),
        m => Err(inattendu(&m)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tokio::net::TcpListener;

    /// A side whose "store" is a map of records, its changes a JSON object.
    struct Memoire {
        id: String,
        lignes: BTreeMap<String, String>,
        secrets: Vec<(String, String, String)>,
        fichiers: BTreeMap<String, Vec<u8>>,
    }

    impl Memoire {
        fn new(id: &str) -> Self {
            Self {
                id: id.into(),
                lignes: BTreeMap::new(),
                secrets: Vec::new(),
                fichiers: BTreeMap::new(),
            }
        }
    }

    impl Side for Memoire {
        fn device_id(&self) -> String {
            self.id.clone()
        }
        fn name(&self) -> String {
            self.id.clone()
        }
        fn last_clock_of(&self, _peer: &str) -> String {
            String::new()
        }
        fn changes_since(&self, _clock: &str) -> Result<Vec<u8>, PeerError> {
            let json = serde_json::to_vec(&self.lignes).unwrap();
            Ok(zstd::encode_all(json.as_slice(), 3)?)
        }
        fn apply_changes(&mut self, _peer: &str, zstd_json: &[u8]) -> Result<Summary, PeerError> {
            let json = zstd::decode_all(zstd_json)?;
            let leurs: BTreeMap<String, String> = serde_json::from_slice(&json).unwrap();
            let mut s = Summary::default();
            for (k, v) in leurs {
                if self.lignes.insert(k, v).is_none() {
                    s.inserted += 1;
                }
            }
            Ok(s)
        }
        fn secrets_out(&self) -> Vec<(String, String, String)> {
            self.secrets.clone()
        }
        fn secrets_in(&mut self, entries: Vec<(String, String, String)>) {
            self.secrets.extend(entries);
        }
        fn manifests(&self) -> Vec<(String, Vec<ManifestEntry>)> {
            let entrees = self
                .fichiers
                .iter()
                .map(|(p, b)| ManifestEntry {
                    path: p.clone(),
                    size: b.len() as u64,
                    mtime_ms: 0,
                    hash_hex: blake3::hash(b).to_hex().to_string(),
                })
                .collect();
            vec![("space".into(), entrees)]
        }
        fn plan_notes(&self, _peer: &str, _space: &str, there: &[ManifestEntry]) -> Vec<NoteCopy> {
            there
                .iter()
                .filter(|e| !self.fichiers.contains_key(&e.path))
                .map(|e| NoteCopy::Fetch(e.path.clone()))
                .collect()
        }
        fn read_file(&self, _space: &str, path: &str) -> std::io::Result<(i64, Vec<u8>)> {
            Ok((0, self.fichiers[path].clone()))
        }
        fn write_file(&mut self, _s: &str, path: &str, _m: i64, b: &[u8]) -> std::io::Result<()> {
            self.fichiers.insert(path.into(), b.to_vec());
            Ok(())
        }
    }

    #[tokio::test]
    async fn two_sides_end_with_each_others_records_secrets_and_notes() {
        let ecoute = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let adresse = ecoute.local_addr().unwrap();
        let secret = [3u8; 32];

        let pc = tokio::spawn(async move {
            let (flux, _) = ecoute.accept().await.unwrap();
            let (mut s, _) = Session::accept(flux, &move |_: &str| Some(secret))
                .await
                .unwrap();
            let mut pc = Memoire::new("pc");
            pc.lignes.insert("task-1".into(), "Call Marie".into());
            pc.secrets
                .push(("a@example.com".into(), "password".into(), "x".into()));
            pc.fichiers
                .insert("Cours/Chapitre 2.md".into(), b"# Espaces".to_vec());
            let bilan = exchange(&mut s, &mut pc, Wants::default()).await.unwrap();
            (pc, bilan)
        });

        let mut s = Session::connect(adresse, "phone", &secret).await.unwrap();
        let mut tel = Memoire::new("phone");
        tel.lignes.insert("task-2".into(), "Buy bread".into());
        let bilan_tel = exchange(&mut s, &mut tel, Wants::default()).await.unwrap();
        let (pc, bilan_pc) = pc.await.unwrap();

        assert_eq!(tel.lignes.len(), 2);
        assert_eq!(pc.lignes.len(), 2);
        assert_eq!(bilan_tel.inserted, 1);
        assert_eq!(bilan_pc.inserted, 1);
        assert_eq!(tel.secrets.len(), 1, "the PC's password reached the phone");
        assert_eq!(tel.fichiers["Cours/Chapitre 2.md"], b"# Espaces");
    }
}
