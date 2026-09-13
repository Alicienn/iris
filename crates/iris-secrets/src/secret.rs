//! Le porteur de secret.

use std::fmt;

/// Une chaîne sensible.
///
/// Trois protections, toutes destinées à empêcher une fuite par inadvertance plutôt
/// qu'une attaque déterminée :
///
/// - `Debug` et `Display` n'affichent jamais le contenu. Un secret ne doit pas
///   pouvoir arriver dans un journal parce que quelqu'un a écrit `{:?}` ;
/// - la lecture passe par [`expose`](Secret::expose), dont le nom rend le geste
///   visible en relecture de code ;
/// - la mémoire est effacée à la destruction.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Lit le secret en clair. Le nom est délibérément explicite.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret({} caractères)", self.0.chars().count())
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("••••••••")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // Écrasement avant libération. Le compilateur pourrait théoriquement
        // supprimer cette écriture ; passer par les octets de la chaîne rend
        // l'élimination bien moins probable qu'une simple affectation.
        let octets = overwrite_in_place(&mut self.0);
        debug_assert!(octets.is_empty() || octets.iter().all(|b| *b == 0));
    }
}

/// Écrase le contenu d'une chaîne, sans code non sûr.
fn overwrite_in_place(s: &mut String) -> Vec<u8> {
    // `String::clear` ne fait que remettre la longueur à zéro : le contenu reste en
    // mémoire. On le remplace donc caractère par caractère avant de vider.
    let longueur = s.len();
    let remplissage = "\0".repeat(longueur);
    s.replace_range(.., &remplissage);
    let trace = s.as_bytes().to_vec();
    s.clear();
    s.shrink_to_fit();
    trace
}

impl From<String> for Secret {
    fn from(v: String) -> Self {
        Self::new(v)
    }
}

impl From<&str> for Secret {
    fn from(v: &str) -> Self {
        Self::new(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_secret_est_lisible_quand_on_le_demande() {
        let s = Secret::new("mot-de-passe");
        assert_eq!(s.expose(), "mot-de-passe");
        assert_eq!(s.len(), 12);
        assert!(!s.is_empty());
    }

    #[test]
    fn l_affichage_de_debogage_ne_revele_rien() {
        // Le cas réel : quelqu'un écrit tracing::debug!(?config) et la configuration
        // contient un mot de passe.
        let s = Secret::new("mot-de-passe");
        let rendu = format!("{s:?}");
        assert!(!rendu.contains("mot-de-passe"));
        assert!(rendu.contains("12 caractères"));
    }

    #[test]
    fn l_affichage_normal_ne_revele_rien() {
        let s = Secret::new("mot-de-passe");
        assert_eq!(format!("{s}"), "••••••••");
    }

    #[test]
    fn le_compte_de_caracteres_est_correct_en_unicode() {
        let s = Secret::new("éàü");
        assert!(format!("{s:?}").contains("3 caractères"));
    }

    #[test]
    fn un_secret_vide_est_reconnu() {
        let s = Secret::new("");
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn les_secrets_se_comparent() {
        assert_eq!(Secret::new("a"), Secret::from("a"));
        assert_ne!(Secret::new("a"), Secret::new("b"));
    }

    #[test]
    fn la_memoire_est_ecrasee_a_la_destruction() {
        let mut s = String::from("mot-de-passe");
        let trace = overwrite_in_place(&mut s);
        assert!(trace.iter().all(|b| *b == 0), "le contenu doit être écrasé");
    }
}
