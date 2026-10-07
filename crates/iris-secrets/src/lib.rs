//! `iris-secrets` — mots de passe et jetons.
//!
//! Deux implémentations derrière un même trait :
//!
//! - le **trousseau du système**, qui est le bon endroit — l'utilisateur y a déjà
//!   confiance, il est chiffré par sa session, et aucun secret ne traîne dans nos
//!   fichiers ;
//! - un **coffre chiffré** de repli, pour les environnements dépourvus de trousseau
//!   (serveurs, conteneurs, sessions distantes) et pour les tests.
//!
//! Aucune des deux ne journalise jamais un secret, et le type qui les transporte
//! efface sa mémoire lorsqu'il est détruit.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

mod encrypted;
mod secret;

pub use encrypted::EncryptedVault;
pub use secret::Secret;

use iris_types::{Error, Result};

/// Ce que sait faire un magasin de secrets.
pub trait SecretStore: Send + Sync + std::fmt::Debug {
    /// Enregistre ou remplace un secret.
    fn set(&self, account: &str, kind: SecretKind, value: &Secret) -> Result<()>;

    /// Lit un secret. `None` s'il n'existe pas.
    fn get(&self, account: &str, kind: SecretKind) -> Result<Option<Secret>>;

    /// Supprime un secret. Retourne `true` s'il existait.
    fn delete(&self, account: &str, kind: SecretKind) -> Result<bool>;

    /// Nom du magasin, pour les diagnostics.
    fn backend(&self) -> &'static str;
}

/// Nature d'un secret. Un compte peut en détenir plusieurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecretKind {
    /// Mot de passe IMAP, et SMTP quand l'envoi n'a pas le sien.
    Password,
    /// Jeton d'accès OAuth2, de courte durée.
    AccessToken,
    /// Jeton de rafraîchissement OAuth2, de longue durée. Le plus sensible.
    RefreshToken,
    /// The sending password, when the SMTP server takes another one (a profile's
    /// `OutgoingPassword`).
    SmtpPassword,
}

impl SecretKind {
    /// Every kind, for what must remove or move all of an account's secrets.
    pub const ALL: [Self; 4] = [
        Self::Password,
        Self::AccessToken,
        Self::RefreshToken,
        Self::SmtpPassword,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::AccessToken => "access_token",
            Self::RefreshToken => "refresh_token",
            Self::SmtpPassword => "smtp_password",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "password" => Some(Self::Password),
            "access_token" => Some(Self::AccessToken),
            "refresh_token" => Some(Self::RefreshToken),
            "smtp_password" => Some(Self::SmtpPassword),
            _ => None,
        }
    }
}

/// Clé d'entrée dans le trousseau : `compte/nature`.
fn entry_key(account: &str, kind: SecretKind) -> String {
    format!("{}/{}", account.trim().to_lowercase(), kind.as_str())
}

/// Le trousseau du système.
#[derive(Debug)]
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// `service` identifie l'application auprès du trousseau. Le changer rendrait
    /// invisibles tous les secrets déjà enregistrés.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// Vérifie que le trousseau est utilisable ici.
    ///
    /// À appeler au démarrage : mieux vaut basculer sur le coffre chiffré tout de
    /// suite que découvrir l'absence de trousseau au moment de se connecter.
    pub fn is_available(&self) -> bool {
        let sonde = entry_key("__iris_probe__", SecretKind::Password);
        match keyring::Entry::new(&self.service, &sonde) {
            Ok(entry) => {
                // Une entrée absente est une réponse valable : le trousseau répond.
                !matches!(
                    entry.get_password(),
                    Err(keyring::Error::PlatformFailure(_))
                )
            }
            Err(_) => false,
        }
    }

    fn entry_at(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key).map_err(|e| Error::Config(format!("keyring: {e}")))
    }

    fn write(&self, key: &str, value: &str) -> Result<()> {
        self.entry_at(key)?
            .set_password(value)
            // Le message d'erreur ne doit jamais contenir le secret lui-même.
            .map_err(|e| Error::Config(format!("écriture dans le trousseau : {e}")))
    }

    fn read(&self, key: &str) -> Result<Option<String>> {
        match self.entry_at(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Config(format!("lecture du trousseau : {e}"))),
        }
    }

    fn remove(&self, key: &str) -> Result<bool> {
        match self.entry_at(key)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(Error::Config(format!(
                "suppression dans le trousseau : {e}"
            ))),
        }
    }

    /// Removes the parts a long secret was written in, from the first one missing.
    fn remove_parts(&self, key: &str) -> Result<()> {
        for n in 1.. {
            if !self.remove(&part_key(key, n))? {
                break;
            }
        }
        Ok(())
    }
}

/// How many characters one keyring entry is given. Windows keeps 2,560 bytes of
/// UTF-16, 1,280 characters: a Microsoft access token (1,300 to 2,500) was refused,
/// and signing in failed after the consent. A longer secret is written in parts.
const PART: usize = 1_000;

/// What the first entry of a secret written in parts holds: the number of parts.
const PARTS_MARK: &str = "iris-parts:";

fn part_key(key: &str, n: usize) -> String {
    format!("{key}#{n}")
}

/// A secret cut into parts of at most `PART` characters.
fn split_parts(value: &str) -> Vec<String> {
    let chars: Vec<char> = value.chars().collect();
    chars.chunks(PART).map(|c| c.iter().collect()).collect()
}

impl SecretStore for KeyringStore {
    fn set(&self, account: &str, kind: SecretKind, value: &Secret) -> Result<()> {
        let key = entry_key(account, kind);
        let secret = value.expose();
        if secret.chars().count() <= PART {
            self.write(&key, secret)?;
            return self.remove_parts(&key);
        }
        let parts = split_parts(secret);
        for (n, part) in parts.iter().enumerate() {
            self.write(&part_key(&key, n + 1), part)?;
        }
        // Parts of a longer secret written before, past these.
        let mut n = parts.len() + 1;
        while self.remove(&part_key(&key, n))? {
            n += 1;
        }
        self.write(&key, &format!("{PARTS_MARK}{}", parts.len()))
    }

    fn get(&self, account: &str, kind: SecretKind) -> Result<Option<Secret>> {
        let key = entry_key(account, kind);
        let Some(tete) = self.read(&key)? else {
            return Ok(None);
        };
        let Some(nombre) = tete
            .strip_prefix(PARTS_MARK)
            .and_then(|n| n.parse::<usize>().ok())
        else {
            return Ok(Some(Secret::new(tete)));
        };
        let mut secret = String::new();
        for n in 1..=nombre {
            match self.read(&part_key(&key, n))? {
                Some(part) => secret.push_str(&part),
                // A part gone: the secret cannot be told whole.
                None => return Ok(None),
            }
        }
        Ok(Some(Secret::new(secret)))
    }

    fn delete(&self, account: &str, kind: SecretKind) -> Result<bool> {
        let key = entry_key(account, kind);
        self.remove_parts(&key)?;
        self.remove(&key)
    }

    fn backend(&self) -> &'static str {
        "system keyring"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_clef_d_entree_normalise_le_compte() {
        assert_eq!(
            entry_key("  Marie@Example.COM ", SecretKind::Password),
            "marie@example.com/password"
        );
    }

    #[test]
    fn a_long_secret_is_cut_in_parts_the_keyring_takes() {
        let jeton = "é".repeat(10) + &"x".repeat(2_490);
        let parts = split_parts(&jeton);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.chars().count() <= PART));
        assert_eq!(parts.concat(), jeton);
    }

    #[test]
    fn les_natures_de_secret_font_un_aller_retour() {
        for k in [
            SecretKind::Password,
            SecretKind::AccessToken,
            SecretKind::RefreshToken,
        ] {
            assert_eq!(SecretKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(SecretKind::parse("inconnu"), None);
    }

    #[test]
    fn deux_natures_ne_partagent_pas_la_meme_clef() {
        // Sinon enregistrer un jeton écraserait le mot de passe.
        assert_ne!(
            entry_key("a@x.fr", SecretKind::AccessToken),
            entry_key("a@x.fr", SecretKind::RefreshToken)
        );
    }
}
