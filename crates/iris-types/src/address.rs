//! Adresses électroniques et leur comparaison.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Une adresse, avec son nom d'affichage éventuel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address {
    /// Nom affiché tel que fourni par l'expéditeur. Jamais utilisé pour comparer.
    pub name: Option<String>,
    /// Partie adresse, conservée telle qu'écrite.
    pub addr: String,
}

impl Address {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            name: None,
            addr: addr.into(),
        }
    }

    pub fn named(name: impl Into<String>, addr: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            addr: addr.into(),
        }
    }

    /// Partie locale, avant l'arobase.
    pub fn local_part(&self) -> &str {
        match self.addr.rfind('@') {
            Some(i) => &self.addr[..i],
            None => &self.addr,
        }
    }

    /// Domaine, après la dernière arobase, en minuscules à la comparaison près.
    pub fn domain(&self) -> &str {
        match self.addr.rfind('@') {
            Some(i) => &self.addr[i + 1..],
            None => "",
        }
    }

    /// Forme canonique servant de clé d'identité.
    ///
    /// Le domaine est insensible à la casse par le RFC 5321 ; la partie locale ne
    /// l'est pas formellement, mais aucun fournisseur réel ne distingue deux boîtes
    /// par la casse, et les traiter comme distinctes casserait le regroupement par
    /// correspondant. On replie donc l'ensemble en minuscules.
    pub fn key(&self) -> String {
        self.addr.trim().to_lowercase()
    }

    /// Nom à afficher : le nom fourni s'il existe, sinon la partie locale.
    pub fn display(&self) -> &str {
        match &self.name {
            Some(n) if !n.trim().is_empty() => n,
            _ => self.local_part(),
        }
    }

    /// Vérification de forme minimale : une arobase, du texte des deux côtés, un point
    /// dans le domaine. Volontairement permissif — refuser une adresse exotique mais
    /// valide est plus grave qu'en accepter une douteuse.
    pub fn looks_valid(&self) -> bool {
        let a = self.addr.trim();
        let Some(at) = a.rfind('@') else { return false };
        let (local, domain) = (&a[..at], &a[at + 1..]);
        !local.is_empty()
            && !domain.is_empty()
            && domain.contains('.')
            && !domain.starts_with('.')
            && !domain.ends_with('.')
            && !a.contains(char::is_whitespace)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(n) if !n.is_empty() => write!(f, "{n} <{}>", self.addr),
            _ => f.write_str(&self.addr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoupe_partie_locale_et_domaine() {
        let a = Address::new("marie.vasseur@example.com");
        assert_eq!(a.local_part(), "marie.vasseur");
        assert_eq!(a.domain(), "example.com");
    }

    #[test]
    fn la_cle_est_insensible_a_la_casse_et_aux_espaces() {
        let a = Address::new("  Marie@Example.COM ");
        let b = Address::named("Marie V.", "marie@example.com");
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn le_nom_affiche_retombe_sur_la_partie_locale() {
        assert_eq!(Address::new("contact@example.com").display(), "contact");
        assert_eq!(
            Address::named("Marie", "contact@example.com").display(),
            "Marie"
        );
        // Un nom vide ne doit pas produire une ligne vide dans la liste.
        assert_eq!(
            Address::named("   ", "contact@example.com").display(),
            "contact"
        );
    }

    #[test]
    fn validation_de_forme() {
        assert!(Address::new("a@b.co").looks_valid());
        assert!(!Address::new("sans-arobase").looks_valid());
        assert!(!Address::new("@example.com").looks_valid());
        assert!(!Address::new("a@").looks_valid());
        assert!(!Address::new("a@localhost").looks_valid());
        assert!(!Address::new("a b@example.com").looks_valid());
    }

    #[test]
    fn une_adresse_sans_arobase_a_un_domaine_vide() {
        let a = Address::new("postmaster");
        assert_eq!(a.domain(), "");
        assert_eq!(a.local_part(), "postmaster");
    }
}
