//! Les capacités.
//!
//! Un module — interne ou plugin — ne peut atteindre que ce qu'il a déclaré vouloir,
//! et seulement si cela lui a été accordé. C'est ce mécanisme unique qui permet à un
//! plugin WebAssembly d'obéir exactement au même modèle de permissions qu'une crate
//! compilée : la frontière n'est pas la technologie, c'est la capacité.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// Portée d'un accès réseau.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkScope {
    /// N'importe quelle destination. À n'accorder qu'aux modules du cœur.
    Any,
    /// Une liste close de noms d'hôtes.
    Hosts(BTreeSet<String>),
}

impl NetworkScope {
    pub fn hosts<I, S>(hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Hosts(hosts.into_iter().map(Into::into).collect())
    }

    /// Cette portée couvre-t-elle celle demandée ?
    fn covers(&self, requested: &Self) -> bool {
        match (self, requested) {
            (Self::Any, _) => true,
            (Self::Hosts(_), Self::Any) => false,
            (Self::Hosts(granted), Self::Hosts(wanted)) => wanted.is_subset(granted),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Lire les messages, les fils et leur état.
    ReadMail,
    /// Modifier l'état des fils, les drapeaux, déplacer des messages.
    WriteMail,
    /// Lire la liste des comptes et leur configuration, sans les secrets.
    ReadAccounts,
    /// Créer, modifier ou supprimer des comptes.
    WriteAccounts,
    /// Publier sur le bus d'événements.
    PublishEvents,
    /// S'abonner au bus d'événements.
    SubscribeEvents,
    /// Sortir sur le réseau.
    Network(NetworkScope),
    /// Lire ou écrire des identifiants dans le trousseau. Jamais accordée à un plugin.
    Secrets,
    /// Disposer d'un espace de stockage propre et isolé.
    Storage,
    /// Contribuer des panneaux à l'interface.
    UiPanels,
    /// Ajouter des commandes à la palette.
    Commands,
    /// Émettre des notifications système.
    Notifications,
}

impl Capability {
    /// Étiquette stable, utilisée dans les manifestes et l'interface de permissions.
    pub fn label(&self) -> &'static str {
        match self {
            Self::ReadMail => "read_mail",
            Self::WriteMail => "write_mail",
            Self::ReadAccounts => "read_accounts",
            Self::WriteAccounts => "write_accounts",
            Self::PublishEvents => "publish_events",
            Self::SubscribeEvents => "subscribe_events",
            Self::Network(_) => "network",
            Self::Secrets => "secrets",
            Self::Storage => "storage",
            Self::UiPanels => "ui_panels",
            Self::Commands => "commands",
            Self::Notifications => "notifications",
        }
    }

    /// Une capacité sensible doit être présentée explicitement à l'utilisateur avant
    /// d'être accordée à un plugin.
    pub fn is_sensitive(&self) -> bool {
        matches!(
            self,
            Self::WriteMail
                | Self::WriteAccounts
                | Self::Secrets
                | Self::Network(_)
                | Self::Notifications
        )
    }

    /// Cette capacité peut-elle être accordée à du code tiers ?
    ///
    /// Le trousseau ne l'est jamais : un plugin qui peut lire les mots de passe des
    /// comptes n'est plus un plugin, c'est une porte dérobée.
    pub fn grantable_to_plugins(&self) -> bool {
        !matches!(self, Self::Secrets | Self::WriteAccounts)
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(NetworkScope::Any) => f.write_str("network(*)"),
            Self::Network(NetworkScope::Hosts(h)) => {
                write!(f, "network({})", h.iter().cloned().collect::<Vec<_>>().join(","))
            }
            other => f.write_str(other.label()),
        }
    }
}

/// L'ensemble des capacités accordées à un module.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet {
    granted: BTreeSet<Capability>,
}

impl CapabilitySet {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Toutes les capacités, réservé aux modules du cœur.
    pub fn all() -> Self {
        let mut s = Self::default();
        for c in [
            Capability::ReadMail,
            Capability::WriteMail,
            Capability::ReadAccounts,
            Capability::WriteAccounts,
            Capability::PublishEvents,
            Capability::SubscribeEvents,
            Capability::Network(NetworkScope::Any),
            Capability::Secrets,
            Capability::Storage,
            Capability::UiPanels,
            Capability::Commands,
            Capability::Notifications,
        ] {
            s.granted.insert(c);
        }
        s
    }

    pub fn from_iter<I: IntoIterator<Item = Capability>>(caps: I) -> Self {
        Self { granted: caps.into_iter().collect() }
    }

    pub fn grant(&mut self, cap: Capability) {
        self.granted.insert(cap);
    }

    pub fn revoke(&mut self, cap: &Capability) {
        self.granted.remove(cap);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.granted.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.granted.is_empty()
    }

    /// La capacité demandée est-elle couverte ?
    ///
    /// Les accès réseau se comparent par inclusion : un module autorisé sur
    /// `example.com` et `example.org` peut demander `example.com` seul.
    pub fn allows(&self, requested: &Capability) -> bool {
        if self.granted.contains(requested) {
            return true;
        }
        if let Capability::Network(wanted) = requested {
            return self.granted.iter().any(|g| match g {
                Capability::Network(granted) => granted.covers(wanted),
                _ => false,
            });
        }
        false
    }

    /// Vérifie une liste d'exigences et retourne la première refusée.
    pub fn check_all<'a, I>(&self, required: I) -> Result<(), Capability>
    where
        I: IntoIterator<Item = &'a Capability>,
    {
        for r in required {
            if !self.allows(r) {
                return Err(r.clone());
            }
        }
        Ok(())
    }

    /// Restreint l'ensemble à ce qui peut être accordé à du code tiers.
    pub fn sandboxed(&self) -> Self {
        Self {
            granted: self
                .granted
                .iter()
                .filter(|c| c.grantable_to_plugins())
                .cloned()
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_capacite_non_accordee_est_refusee() {
        let set = CapabilitySet::from_iter([Capability::ReadMail]);
        assert!(set.allows(&Capability::ReadMail));
        assert!(!set.allows(&Capability::WriteMail));
    }

    #[test]
    fn le_reseau_se_compare_par_inclusion() {
        let set = CapabilitySet::from_iter([Capability::Network(NetworkScope::hosts([
            "example.com",
            "example.org",
        ]))]);
        assert!(set.allows(&Capability::Network(NetworkScope::hosts(["example.com"]))));
        assert!(set.allows(&Capability::Network(NetworkScope::hosts([
            "example.com",
            "example.org"
        ]))));
        assert!(!set.allows(&Capability::Network(NetworkScope::hosts(["ailleurs.net"]))));
    }

    #[test]
    fn un_acces_restreint_ne_donne_pas_un_acces_total() {
        let set = CapabilitySet::from_iter([Capability::Network(NetworkScope::hosts([
            "example.com",
        ]))]);
        assert!(!set.allows(&Capability::Network(NetworkScope::Any)));
    }

    #[test]
    fn un_acces_total_couvre_tout() {
        let set = CapabilitySet::from_iter([Capability::Network(NetworkScope::Any)]);
        assert!(set.allows(&Capability::Network(NetworkScope::hosts(["n-importe-quoi.fr"]))));
    }

    #[test]
    fn check_all_designe_la_capacite_manquante() {
        let set = CapabilitySet::from_iter([Capability::ReadMail]);
        let besoin = [Capability::ReadMail, Capability::WriteMail];
        assert_eq!(set.check_all(besoin.iter()), Err(Capability::WriteMail));
    }

    #[test]
    fn le_bac_a_sable_retire_le_trousseau_et_les_comptes() {
        let complet = CapabilitySet::all();
        assert!(complet.allows(&Capability::Secrets));

        let sandbox = complet.sandboxed();
        assert!(!sandbox.allows(&Capability::Secrets));
        assert!(!sandbox.allows(&Capability::WriteAccounts));
        // Le reste survit.
        assert!(sandbox.allows(&Capability::ReadMail));
        assert!(sandbox.allows(&Capability::UiPanels));
    }

    #[test]
    fn les_capacites_sensibles_sont_identifiees() {
        assert!(Capability::Secrets.is_sensitive());
        assert!(Capability::WriteMail.is_sensitive());
        assert!(!Capability::ReadMail.is_sensitive());
        assert!(!Capability::Commands.is_sensitive());
    }

    #[test]
    fn revoquer_retire_bien_la_capacite() {
        let mut set = CapabilitySet::all();
        set.revoke(&Capability::Secrets);
        assert!(!set.allows(&Capability::Secrets));
    }

    #[test]
    fn l_affichage_montre_la_portee_reseau() {
        assert_eq!(Capability::Network(NetworkScope::Any).to_string(), "network(*)");
        assert_eq!(
            Capability::Network(NetworkScope::hosts(["a.fr"])).to_string(),
            "network(a.fr)"
        );
        assert_eq!(Capability::ReadMail.to_string(), "read_mail");
    }
}
