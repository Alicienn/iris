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
    /// Mot de passe IMAP et SMTP.
    Password,
    /// Jeton d'accès OAuth2, de courte durée.
    AccessToken,
    /// Jeton de rafraîchissement OAuth2, de longue durée. Le plus sensible.
    RefreshToken,
}

impl SecretKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::AccessToken => "access_token",
            Self::RefreshToken => "refresh_token",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "password" => Some(Self::Password),
            "access_token" => Some(Self::AccessToken),
            "refresh_token" => Some(Self::RefreshToken),
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
        Self { service: service.into() }
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
                !matches!(entry.get_password(), Err(keyring::Error::PlatformFailure(_)))
            }
            Err(_) => false,
        }
    }

    fn entry(&self, account: &str, kind: SecretKind) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &entry_key(account, kind))
            .map_err(|e| Error::Config(format!("trousseau : {e}")))
    }
}

impl SecretStore for KeyringStore {
    fn set(&self, account: &str, kind: SecretKind, value: &Secret) -> Result<()> {
        self.entry(account, kind)?
            .set_password(value.expose())
            // Le message d'erreur ne doit jamais contenir le secret lui-même.
            .map_err(|e| Error::Config(format!("écriture dans le trousseau : {e}")))
    }

    fn get(&self, account: &str, kind: SecretKind) -> Result<Option<Secret>> {
        match self.entry(account, kind)?.get_password() {
            Ok(v) => Ok(Some(Secret::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(Error::Config(format!("lecture du trousseau : {e}"))),
        }
    }

    fn delete(&self, account: &str, kind: SecretKind) -> Result<bool> {
        match self.entry(account, kind)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(Error::Config(format!("suppression dans le trousseau : {e}"))),
        }
    }

    fn backend(&self) -> &'static str {
        "trousseau du système"
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
    fn les_natures_de_secret_font_un_aller_retour() {
        for k in [SecretKind::Password, SecretKind::AccessToken, SecretKind::RefreshToken] {
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
