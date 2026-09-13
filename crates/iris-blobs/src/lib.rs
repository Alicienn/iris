//! `iris-blobs` — corps de messages et pièces jointes.
//!
//! Ces objets sortent de la base pour trois raisons : ils sont volumineux, ils se
//! compressent très bien (un corps de mail perd 70 à 80 % de sa taille en zstd), et
//! ils sont **jetables** — on peut toujours les retélécharger. Cette dernière
//! propriété est ce qui autorise une éviction agressive, impensable pour des
//! métadonnées.
//!
//! Le stockage est **adressé par le contenu** : deux messages portant la même pièce
//! jointe ne l'écrivent qu'une fois, et un corps resynchronisé ne crée pas de
//! doublon.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use iris_types::{BlobId, Error, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Statistiques d'exploitation du cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlobStats {
    pub hits: u64,
    pub misses: u64,
    pub writes: u64,
    pub evictions: u64,
    /// Taille occupée sur le disque, après compression.
    pub bytes_on_disk: u64,
    pub count: u64,
}

impl BlobStats {
    /// Proportion de lectures servies localement.
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    size: u64,
    /// Dernier accès, en secondes depuis l'époque. Sert de clé d'éviction.
    accessed: u64,
}

#[derive(Debug, Default)]
struct Index {
    entries: HashMap<BlobId, Entry>,
    bytes: u64,
    evictions: u64,
}

/// Le magasin de contenus.
#[derive(Debug)]
pub struct BlobStore {
    root: PathBuf,
    max_bytes: u64,
    index: Mutex<Index>,
    hits: AtomicU64,
    misses: AtomicU64,
    writes: AtomicU64,
    /// Niveau de compression zstd. 3 est le compromis retenu : au-delà, le gain sur
    /// du texte devient marginal alors que le coût processeur double.
    level: i32,
}

impl BlobStore {
    /// Ouvre le magasin, en reconstruisant son index depuis le disque.
    pub fn open(root: impl AsRef<Path>, max_bytes: u64) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;

        let store = Self {
            root,
            max_bytes,
            index: Mutex::new(Index::default()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            level: 3,
        };
        store.rebuild_index()?;
        Ok(store)
    }

    /// Reconstruit l'index en parcourant l'arborescence.
    ///
    /// Aucun état n'est persisté à côté des fichiers : le disque est la seule source
    /// de vérité. Un index séparé pourrait diverger, et divergerait forcément un jour
    /// — après un arrêt brutal, par exemple.
    fn rebuild_index(&self) -> Result<()> {
        let mut index = self.lock()?;
        index.entries.clear();
        index.bytes = 0;

        for shard in fs::read_dir(&self.root)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for file in fs::read_dir(shard.path())? {
                let file = file?;
                let meta = file.metadata()?;
                if !meta.is_file() {
                    continue;
                }
                let name = file.file_name();
                let Some(stem) = Path::new(&name).file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(id) = BlobId::from_hex(stem) else {
                    // Fichier étranger : on l'ignore plutôt que d'échouer au démarrage.
                    tracing::warn!(?name, "fichier inconnu dans le magasin de contenus");
                    continue;
                };
                let accessed = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                index.bytes += meta.len();
                index.entries.insert(id, Entry { size: meta.len(), accessed });
            }
        }
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Index>> {
        self.index.lock().map_err(|_| Error::store("index de contenus empoisonné"))
    }

    /// Chemin d'un contenu. Les deux premiers caractères servent de répartiteur, pour
    /// qu'aucun répertoire ne contienne cent mille fichiers.
    fn path_of(&self, id: BlobId) -> PathBuf {
        let hex = id.to_hex();
        self.root.join(&hex[..2]).join(format!("{hex}.zst"))
    }

    /// Écrit un contenu et retourne son identifiant.
    ///
    /// Idempotent : réécrire le même contenu ne fait que rafraîchir sa date d'accès.
    pub fn put(&self, data: &[u8]) -> Result<BlobId> {
        let id = Self::id_of(data);
        let path = self.path_of(id);

        if path.exists() {
            self.touch(id)?;
            return Ok(id);
        }

        let compressed = zstd::encode_all(data, self.level)
            .map_err(|e| Error::store(format!("compression : {e}")))?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Écriture par fichier temporaire puis renommage : un arrêt brutal en cours
        // d'écriture ne doit jamais laisser un contenu tronqué qui serait ensuite lu
        // comme valide.
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &compressed)?;
        fs::rename(&tmp, &path)?;

        let size = compressed.len() as u64;
        {
            let mut index = self.lock()?;
            index.bytes += size;
            index.entries.insert(id, Entry { size, accessed: now_secs() });
        }
        self.writes.fetch_add(1, Ordering::Relaxed);

        self.evict_if_needed()?;
        Ok(id)
    }

    /// Lit un contenu. `None` s'il n'est pas — ou n'est plus — en cache.
    pub fn get(&self, id: BlobId) -> Result<Option<Vec<u8>>> {
        let path = self.path_of(id);
        let compressed = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                // Le fichier a pu être supprimé hors de notre dos.
                let mut index = self.lock()?;
                if let Some(entry) = index.entries.remove(&id) {
                    index.bytes = index.bytes.saturating_sub(entry.size);
                }
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };

        let data = zstd::decode_all(compressed.as_slice())
            .map_err(|e| Error::store(format!("décompression de {id} : {e}")))?;

        self.hits.fetch_add(1, Ordering::Relaxed);
        self.touch(id)?;
        Ok(Some(data))
    }

    pub fn contains(&self, id: BlobId) -> Result<bool> {
        Ok(self.lock()?.entries.contains_key(&id))
    }

    /// Supprime un contenu.
    pub fn remove(&self, id: BlobId) -> Result<bool> {
        let path = self.path_of(id);
        let existed = match fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        let mut index = self.lock()?;
        if let Some(entry) = index.entries.remove(&id) {
            index.bytes = index.bytes.saturating_sub(entry.size);
        }
        Ok(existed)
    }

    /// Met à jour la date d'accès, qui pilote l'éviction.
    fn touch(&self, id: BlobId) -> Result<()> {
        let mut index = self.lock()?;
        if let Some(entry) = index.entries.get_mut(&id) {
            entry.accessed = now_secs();
        }
        Ok(())
    }

    /// Évince les contenus les plus anciens jusqu'à repasser sous le plafond.
    ///
    /// L'éviction descend à 90 % du plafond plutôt que d'atteindre exactement la
    /// limite : sans cette marge, chaque écriture ultérieure déclencherait une
    /// éviction, ce qui transformerait le cache en machine à évincer.
    fn evict_if_needed(&self) -> Result<()> {
        let cible = self.max_bytes / 10 * 9;
        let a_evincer = {
            let index = self.lock()?;
            if index.bytes <= self.max_bytes {
                return Ok(());
            }
            let mut par_age: Vec<(BlobId, Entry)> =
                index.entries.iter().map(|(k, v)| (*k, *v)).collect();
            par_age.sort_by_key(|(id, e)| (e.accessed, id.0));

            let mut restant = index.bytes;
            let mut liste = Vec::new();
            for (id, entry) in par_age {
                if restant <= cible {
                    break;
                }
                restant -= entry.size;
                liste.push(id);
            }
            liste
        };

        let mut evinces = 0u64;
        for id in a_evincer {
            if self.remove(id)? {
                evinces += 1;
            }
        }
        if evinces > 0 {
            let mut index = self.lock()?;
            index.evictions += evinces;
            tracing::debug!(evinces, "éviction du cache de contenus");
        }
        Ok(())
    }

    pub fn stats(&self) -> Result<BlobStats> {
        let index = self.lock()?;
        Ok(BlobStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            evictions: index.evictions,
            bytes_on_disk: index.bytes,
            count: index.entries.len() as u64,
        })
    }

    /// Identifiant d'un contenu : BLAKE3 tronqué à 128 bits.
    ///
    /// 128 bits suffisent largement : à un milliard de contenus, la probabilité de
    /// collision reste de l'ordre de 10⁻²¹.
    pub fn id_of(data: &[u8]) -> BlobId {
        let hash = blake3::hash(data);
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        BlobId::from_bytes(bytes)
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(max: u64) -> (BlobStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = BlobStore::open(dir.path(), max).unwrap();
        (s, dir)
    }

    #[test]
    fn ecrire_puis_relire() {
        let (s, _d) = store(1 << 20);
        let contenu = b"Bonjour, je reviens vers vous concernant le devis.";
        let id = s.put(contenu).unwrap();
        assert_eq!(s.get(id).unwrap().as_deref(), Some(&contenu[..]));
        assert!(s.contains(id).unwrap());
    }

    #[test]
    fn le_meme_contenu_donne_le_meme_identifiant() {
        let (s, _d) = store(1 << 20);
        let a = s.put(b"identique").unwrap();
        let b = s.put(b"identique").unwrap();
        assert_eq!(a, b);
        assert_eq!(s.stats().unwrap().count, 1, "aucun doublon sur le disque");
    }

    #[test]
    fn deux_contenus_differents_ne_se_confondent_pas() {
        let (s, _d) = store(1 << 20);
        let a = s.put(b"premier").unwrap();
        let b = s.put(b"second").unwrap();
        assert_ne!(a, b);
        assert_eq!(s.get(a).unwrap().unwrap(), b"premier");
        assert_eq!(s.get(b).unwrap().unwrap(), b"second");
    }

    #[test]
    fn un_contenu_absent_donne_none() {
        let (s, _d) = store(1 << 20);
        let inexistant = BlobStore::id_of(b"jamais ecrit");
        assert!(s.get(inexistant).unwrap().is_none());
        assert_eq!(s.stats().unwrap().misses, 1);
    }

    #[test]
    fn le_texte_se_compresse_reellement() {
        let (s, _d) = store(1 << 24);
        // Un corps de mail réaliste : très redondant.
        let corps = "Bonjour,\nJe vous confirme la réception de votre message.\n".repeat(200);
        let brut = corps.len() as u64;
        s.put(corps.as_bytes()).unwrap();

        let occupe = s.stats().unwrap().bytes_on_disk;
        assert!(
            occupe * 4 < brut,
            "{occupe} octets pour {brut} : la compression doit dépasser un facteur 4 \
             sur du texte redondant"
        );
    }

    #[test]
    fn la_suppression_libere_la_place() {
        let (s, _d) = store(1 << 20);
        let id = s.put(b"a effacer").unwrap();
        assert!(s.stats().unwrap().bytes_on_disk > 0);

        assert!(s.remove(id).unwrap());
        assert_eq!(s.stats().unwrap().bytes_on_disk, 0);
        assert_eq!(s.stats().unwrap().count, 0);
        assert!(!s.remove(id).unwrap(), "seconde suppression sans effet");
    }

    #[test]
    fn le_plafond_declenche_une_eviction() {
        // Plafond minuscule pour forcer le comportement.
        let (s, _d) = store(2_000);
        for i in 0..100u32 {
            // Contenus peu compressibles, pour que la taille sur disque compte.
            let data: Vec<u8> = (0..200).map(|j| (i as u8).wrapping_mul(j as u8 ^ 0x5f)).collect();
            s.put(&data).unwrap();
        }
        let stats = s.stats().unwrap();
        assert!(stats.evictions > 0, "l'éviction doit s'être déclenchée");
        assert!(
            stats.bytes_on_disk <= 2_000,
            "le magasin doit rester sous son plafond, ici {} octets",
            stats.bytes_on_disk
        );
    }

    #[test]
    fn l_eviction_laisse_une_marge_sous_le_plafond() {
        // Sans marge, chaque écriture suivante rappellerait l'éviction.
        let (s, _d) = store(4_000);
        for i in 0..200u32 {
            let data: Vec<u8> = (0..100).map(|j| (i as u8).wrapping_add(j as u8)).collect();
            s.put(&data).unwrap();
        }
        let stats = s.stats().unwrap();
        assert!(stats.bytes_on_disk <= 4_000);
        assert!(stats.count > 0, "le cache ne doit pas se vider entièrement");
    }

    #[test]
    fn l_index_se_reconstruit_a_la_reouverture() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let s = BlobStore::open(dir.path(), 1 << 20).unwrap();
            s.put(b"contenu persistant").unwrap()
        };

        let s = BlobStore::open(dir.path(), 1 << 20).unwrap();
        assert!(s.contains(id).unwrap(), "l'index doit se reconstruire depuis le disque");
        assert_eq!(s.get(id).unwrap().unwrap(), b"contenu persistant");
        assert_eq!(s.stats().unwrap().count, 1);
    }

    #[test]
    fn un_fichier_etranger_ne_bloque_pas_l_ouverture() {
        let dir = tempfile::tempdir().unwrap();
        let intrus = dir.path().join("ab");
        fs::create_dir_all(&intrus).unwrap();
        fs::write(intrus.join("notes.txt"), b"pas un contenu").unwrap();

        let s = BlobStore::open(dir.path(), 1 << 20).unwrap();
        assert_eq!(s.stats().unwrap().count, 0);
    }

    #[test]
    fn le_taux_de_succes_se_calcule() {
        let (s, _d) = store(1 << 20);
        let id = s.put(b"x").unwrap();
        s.get(id).unwrap();
        s.get(id).unwrap();
        s.get(BlobStore::id_of(b"absent")).unwrap();

        let stats = s.stats().unwrap();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_ratio() - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn un_magasin_vide_a_un_taux_nul_sans_diviser_par_zero() {
        let (s, _d) = store(1 << 20);
        assert_eq!(s.stats().unwrap().hit_ratio(), 0.0);
    }

    #[test]
    fn les_contenus_sont_repartis_en_sous_repertoires() {
        // Cent mille fichiers dans un seul répertoire rendent le système de fichiers
        // pénible sur toutes les plateformes.
        let (s, dir) = store(1 << 20);
        let id = s.put(b"reparti").unwrap();
        let attendu = dir.path().join(&id.to_hex()[..2]).join(format!("{}.zst", id.to_hex()));
        assert!(attendu.exists());
    }
}
