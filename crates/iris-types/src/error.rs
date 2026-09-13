//! Le type d'erreur commun.
//!
//! Une seule énumération traverse toutes les couches, et chaque variante indique
//! explicitement si l'opération vaut la peine d'être retentée. Sans cette
//! information, l'ordonnanceur de synchronisation ne peut pas distinguer une panne
//! passagère d'un mot de passe faux, et finirait par marteler un serveur qui le
//! rejette.

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("configuration invalide : {0}")]
    Config(String),

    #[error("stockage : {0}")]
    Store(String),

    #[error("index de recherche : {0}")]
    Index(String),

    #[error("contenu introuvable : {0}")]
    BlobMissing(String),

    #[error("analyse du message : {0}")]
    Parse(String),

    #[error("réseau : {0}")]
    Network(String),

    #[error("protocole {protocol} : {message}")]
    Protocol { protocol: &'static str, message: String },

    #[error("authentification refusée pour {account}")]
    AuthFailed { account: String },

    #[error("jeton expiré pour {account}")]
    TokenExpired { account: String },

    #[error("serveur occupé, réessayer dans {retry_after_secs} s")]
    Throttled { retry_after_secs: u64 },

    #[error("le serveur a invalidé son état ({reason}) : resynchronisation complète requise")]
    ResyncRequired { reason: String },

    #[error("module « {0} » absent ou non démarré")]
    ModuleMissing(String),

    #[error("capacité « {capability} » refusée à « {requester} »")]
    CapabilityDenied { capability: String, requester: String },

    #[error("plugin « {plugin} » : {message}")]
    Plugin { plugin: String, message: String },

    #[error("entrée-sortie : {0}")]
    Io(#[from] std::io::Error),

    #[error("opération annulée")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// L'opération peut-elle réussir plus tard sans intervention de l'utilisateur ?
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Throttled { .. } | Self::TokenExpired { .. } | Self::Io(_)
        )
    }

    /// L'erreur exige-t-elle une action humaine ? L'interface doit alors la signaler
    /// au lieu de retenter silencieusement.
    pub fn needs_user_action(&self) -> bool {
        matches!(self, Self::AuthFailed { .. } | Self::Config(_))
    }

    /// Délai conseillé avant nouvelle tentative, si l'erreur en impose un.
    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::Throttled { retry_after_secs } => Some(*retry_after_secs),
            _ => None,
        }
    }

    pub fn store(msg: impl Into<String>) -> Self {
        Self::Store(msg.into())
    }

    pub fn parse(msg: impl Into<String>) -> Self {
        Self::Parse(msg.into())
    }

    pub fn network(msg: impl Into<String>) -> Self {
        Self::Network(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_pannes_reseau_sont_retentables() {
        assert!(Error::network("connexion perdue").is_transient());
        assert!(Error::Throttled { retry_after_secs: 30 }.is_transient());
        assert!(Error::TokenExpired { account: "a".into() }.is_transient());
    }

    #[test]
    fn un_mot_de_passe_faux_ne_l_est_pas() {
        let e = Error::AuthFailed { account: "contact@example.com".into() };
        assert!(!e.is_transient());
        assert!(e.needs_user_action());
    }

    #[test]
    fn le_delai_conseille_est_transmis() {
        assert_eq!(Error::Throttled { retry_after_secs: 120 }.retry_after_secs(), Some(120));
        assert_eq!(Error::network("x").retry_after_secs(), None);
    }

    #[test]
    fn une_resynchronisation_requise_n_est_pas_une_panne_passagere() {
        // Retenter à l'identique reproduirait l'erreur : il faut changer de stratégie.
        let e = Error::ResyncRequired { reason: "UIDVALIDITY".into() };
        assert!(!e.is_transient());
        assert!(!e.needs_user_action());
    }

    #[test]
    fn les_erreurs_d_entree_sortie_se_convertissent() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "absent");
        let e: Error = io.into();
        assert!(matches!(e, Error::Io(_)));
        assert!(e.is_transient());
    }
}
