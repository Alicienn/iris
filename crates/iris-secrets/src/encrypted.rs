//! Coffre chiffré, utilisé là où le trousseau du système est absent.
//!
//! Format : un fichier JSON contenant le sel, un nonce et le contenu chiffré. La clé
//! est dérivée du mot de passe maître par Argon2id — délibérément lent, pour qu'une
//! attaque par dictionnaire sur le fichier coûte cher.
//!
//! Le fichier entier est rechiffré à chaque écriture. Sur quelques centaines de
//! secrets, le coût est négligeable, et cela évite d'avoir à raisonner sur des
//! réutilisations de nonce par entrée — la faute qui ruine le plus souvent ce genre
//! de mécanisme.

use crate::{entry_key, Secret, SecretKind, SecretStore};
use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use iris_types::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;

/// Enveloppe écrite sur le disque.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    version: u8,
    salt: String,
    nonce: String,
    ciphertext: String,
}

/// Le coffre chiffré.
#[derive(Debug)]
pub struct EncryptedVault {
    path: PathBuf,
    key: [u8; KEY_LEN],
    entries: RwLock<BTreeMap<String, String>>,
    /// Sérialise les écritures : deux enregistrements simultanés réécriraient le
    /// fichier chacun de leur côté, et le dernier écraserait l'autre.
    write_lock: Mutex<()>,
    salt: [u8; SALT_LEN],
}

impl EncryptedVault {
    /// Ouvre le coffre, ou le crée s'il n'existe pas.
    ///
    /// Un mot de passe maître incorrect produit une erreur explicite plutôt qu'un
    /// coffre vide : sans cela, l'utilisateur croirait avoir perdu ses comptes et
    /// les recréerait par-dessus.
    pub fn open(path: impl AsRef<Path>, master: &Secret) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if !path.exists() {
            let salt = random_bytes::<SALT_LEN>();
            let key = derive_key(master, &salt)?;
            let vault = Self {
                path,
                key,
                salt,
                entries: RwLock::new(BTreeMap::new()),
                write_lock: Mutex::new(()),
            };
            vault.persist()?;
            return Ok(vault);
        }

        let brut = std::fs::read_to_string(&path)?;
        let enveloppe: Envelope = serde_json::from_str(&brut)
            .map_err(|e| Error::Config(format!("coffre illisible : {e}")))?;

        if enveloppe.version != 1 {
            return Err(Error::Config(format!(
                "coffre en version {} : cette version d'Iris n'en connaît que 1",
                enveloppe.version
            )));
        }

        let salt: [u8; SALT_LEN] = from_hex(&enveloppe.salt)?
            .try_into()
            .map_err(|_| Error::Config("sel de taille incorrecte".into()))?;
        let nonce = from_hex(&enveloppe.nonce)?;
        let ciphertext = from_hex(&enveloppe.ciphertext)?;

        let key = derive_key(master, &salt)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key)
            .map_err(|e| Error::Config(format!("clé invalide : {e}")))?;

        let clair = cipher
            .decrypt(XNonce::from_slice(&nonce), ciphertext.as_ref())
            .map_err(|_| Error::Config("mot de passe maître incorrect, ou coffre altéré".into()))?;

        let entries: BTreeMap<String, String> = serde_json::from_slice(&clair)
            .map_err(|e| Error::Config(format!("contenu du coffre illisible : {e}")))?;

        Ok(Self {
            path,
            key,
            salt,
            entries: RwLock::new(entries),
            write_lock: Mutex::new(()),
        })
    }

    /// Réécrit le fichier entier.
    fn persist(&self) -> Result<()> {
        let _guard = self.write_lock.lock().map_err(|_| Error::Config("coffre verrouillé".into()))?;

        let clair = {
            let entries = self.entries.read().map_err(|_| Error::Config("coffre".into()))?;
            serde_json::to_vec(&*entries)
                .map_err(|e| Error::Config(format!("sérialisation du coffre : {e}")))?
        };

        // Un nonce neuf à chaque écriture : réutiliser un nonce avec la même clé
        // casse la confidentialité de XChaCha20-Poly1305.
        let nonce = random_bytes::<NONCE_LEN>();
        let cipher = XChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| Error::Config(format!("clé invalide : {e}")))?;
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), clair.as_ref())
            .map_err(|_| Error::Config("chiffrement du coffre".into()))?;

        let enveloppe = Envelope {
            version: 1,
            salt: to_hex(&self.salt),
            nonce: to_hex(&nonce),
            ciphertext: to_hex(&ciphertext),
        };

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Écriture par fichier temporaire puis renommage : une coupure de courant ne
        // doit pas laisser un coffre tronqué, c'est-à-dire tous les comptes perdus.
        let tmp = self.path.with_extension("tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&enveloppe)
                .map_err(|e| Error::Config(format!("écriture du coffre : {e}")))?,
        )?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SecretStore for EncryptedVault {
    fn set(&self, account: &str, kind: SecretKind, value: &Secret) -> Result<()> {
        {
            let mut entries =
                self.entries.write().map_err(|_| Error::Config("coffre".into()))?;
            entries.insert(entry_key(account, kind), value.expose().to_string());
        }
        self.persist()
    }

    fn get(&self, account: &str, kind: SecretKind) -> Result<Option<Secret>> {
        let entries = self.entries.read().map_err(|_| Error::Config("coffre".into()))?;
        Ok(entries.get(&entry_key(account, kind)).map(|v| Secret::new(v.clone())))
    }

    fn delete(&self, account: &str, kind: SecretKind) -> Result<bool> {
        let existait = {
            let mut entries =
                self.entries.write().map_err(|_| Error::Config("coffre".into()))?;
            entries.remove(&entry_key(account, kind)).is_some()
        };
        if existait {
            self.persist()?;
        }
        Ok(existait)
    }

    fn backend(&self) -> &'static str {
        "coffre chiffré"
    }
}

/// Dérive la clé de chiffrement à partir du mot de passe maître.
fn derive_key(master: &Secret, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    let mut key = [0u8; KEY_LEN];
    Argon2::default()
        .hash_password_into(master.expose().as_bytes(), salt, &mut key)
        .map_err(|e| Error::Config(format!("dérivation de clé : {e}")))?;
    Ok(key)
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("source d'aléa du système indisponible");
    buf
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn from_hex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        return Err(Error::Config("données hexadécimales de longueur impaire".into()));
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| {
            let hi = (p[0] as char).to_digit(16);
            let lo = (p[1] as char).to_digit(16);
            match (hi, lo) {
                (Some(h), Some(l)) => Ok((h * 16 + l) as u8),
                _ => Err(Error::Config("données hexadécimales invalides".into())),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master() -> Secret {
        Secret::new("phrase de passe maîtresse")
    }

    #[test]
    fn ecrire_puis_relire_un_secret() {
        let dir = tempfile::tempdir().unwrap();
        let v = EncryptedVault::open(dir.path().join("coffre.json"), &master()).unwrap();

        v.set("a@x.fr", SecretKind::Password, &Secret::new("hunter2")).unwrap();
        let lu = v.get("a@x.fr", SecretKind::Password).unwrap().unwrap();
        assert_eq!(lu.expose(), "hunter2");
    }

    #[test]
    fn un_secret_absent_donne_none() {
        let dir = tempfile::tempdir().unwrap();
        let v = EncryptedVault::open(dir.path().join("coffre.json"), &master()).unwrap();
        assert!(v.get("inconnu@x.fr", SecretKind::Password).unwrap().is_none());
        assert!(v.is_empty());
    }

    #[test]
    fn le_coffre_survit_a_une_reouverture() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("coffre.json");
        {
            let v = EncryptedVault::open(&chemin, &master()).unwrap();
            v.set("a@x.fr", SecretKind::RefreshToken, &Secret::new("jeton-long")).unwrap();
        }
        let v = EncryptedVault::open(&chemin, &master()).unwrap();
        assert_eq!(
            v.get("a@x.fr", SecretKind::RefreshToken).unwrap().unwrap().expose(),
            "jeton-long"
        );
    }

    #[test]
    fn un_mauvais_mot_de_passe_est_signale_et_non_silencieux() {
        // Retourner un coffre vide ferait croire à l'utilisateur qu'il a perdu ses
        // comptes, et il les recréerait par-dessus.
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("coffre.json");
        {
            let v = EncryptedVault::open(&chemin, &master()).unwrap();
            v.set("a@x.fr", SecretKind::Password, &Secret::new("hunter2")).unwrap();
        }
        let e = EncryptedVault::open(&chemin, &Secret::new("mauvaise phrase")).unwrap_err();
        assert!(e.to_string().contains("incorrect"));
    }

    #[test]
    fn un_fichier_altere_est_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("coffre.json");
        {
            let v = EncryptedVault::open(&chemin, &master()).unwrap();
            v.set("a@x.fr", SecretKind::Password, &Secret::new("hunter2")).unwrap();
        }

        // On retouche un octet du texte chiffré : l'authentification doit le voir.
        let mut env: Envelope =
            serde_json::from_str(&std::fs::read_to_string(&chemin).unwrap()).unwrap();
        let mut octets = from_hex(&env.ciphertext).unwrap();
        octets[0] ^= 0xff;
        env.ciphertext = to_hex(&octets);
        std::fs::write(&chemin, serde_json::to_vec(&env).unwrap()).unwrap();

        assert!(EncryptedVault::open(&chemin, &master()).is_err());
    }

    #[test]
    fn aucun_secret_n_apparait_en_clair_dans_le_fichier() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("coffre.json");
        let v = EncryptedVault::open(&chemin, &master()).unwrap();
        v.set("a@x.fr", SecretKind::Password, &Secret::new("motdepasse-tres-reconnaissable"))
            .unwrap();

        let contenu = std::fs::read_to_string(&chemin).unwrap();
        assert!(!contenu.contains("motdepasse-tres-reconnaissable"));
        assert!(!contenu.contains("a@x.fr"), "même les identifiants sont chiffrés");
    }

    #[test]
    fn deux_ecritures_produisent_des_nonces_differents() {
        // Réutiliser un nonce avec la même clé casse la confidentialité.
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("coffre.json");
        let v = EncryptedVault::open(&chemin, &master()).unwrap();

        v.set("a@x.fr", SecretKind::Password, &Secret::new("un")).unwrap();
        let premier: Envelope =
            serde_json::from_str(&std::fs::read_to_string(&chemin).unwrap()).unwrap();

        v.set("b@x.fr", SecretKind::Password, &Secret::new("deux")).unwrap();
        let second: Envelope =
            serde_json::from_str(&std::fs::read_to_string(&chemin).unwrap()).unwrap();

        assert_ne!(premier.nonce, second.nonce);
        assert_eq!(premier.salt, second.salt, "le sel, lui, ne change pas");
    }

    #[test]
    fn supprimer_retire_le_secret() {
        let dir = tempfile::tempdir().unwrap();
        let v = EncryptedVault::open(dir.path().join("coffre.json"), &master()).unwrap();
        v.set("a@x.fr", SecretKind::Password, &Secret::new("x")).unwrap();

        assert!(v.delete("a@x.fr", SecretKind::Password).unwrap());
        assert!(v.get("a@x.fr", SecretKind::Password).unwrap().is_none());
        assert!(!v.delete("a@x.fr", SecretKind::Password).unwrap());
    }

    #[test]
    fn les_natures_de_secret_coexistent() {
        let dir = tempfile::tempdir().unwrap();
        let v = EncryptedVault::open(dir.path().join("coffre.json"), &master()).unwrap();
        v.set("a@x.fr", SecretKind::AccessToken, &Secret::new("court")).unwrap();
        v.set("a@x.fr", SecretKind::RefreshToken, &Secret::new("long")).unwrap();

        assert_eq!(v.get("a@x.fr", SecretKind::AccessToken).unwrap().unwrap().expose(), "court");
        assert_eq!(v.get("a@x.fr", SecretKind::RefreshToken).unwrap().unwrap().expose(), "long");
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn une_version_inconnue_est_refusee() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("coffre.json");
        std::fs::write(
            &chemin,
            r#"{"version":99,"salt":"00","nonce":"00","ciphertext":"00"}"#,
        )
        .unwrap();
        let e = EncryptedVault::open(&chemin, &master()).unwrap_err();
        assert!(e.to_string().contains("version 99"));
    }

    #[test]
    fn conversion_hexadecimale() {
        assert_eq!(to_hex(&[0x00, 0xff, 0x10]), "00ff10");
        assert_eq!(from_hex("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
    }
}
