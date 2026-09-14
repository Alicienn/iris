//! Identifiants typés.
//!
//! Toutes les entités persistées sont adressées par un identifiant distinct, pour
//! qu'aucun appel ne puisse confondre un identifiant de fil avec un identifiant de
//! message. Le coût est nul à l'exécution : ce sont des enveloppes transparentes.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! numeric_id {
    ($(#[$meta:meta])* $name:ident, $inner:ty, $prefix:literal) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub $inner);

        impl $name {
            /// Valeur réservée signifiant « aucune ligne », jamais attribuée par le store.
            pub const NONE: Self = Self(0);

            #[inline]
            pub const fn get(self) -> $inner {
                self.0
            }

            #[inline]
            pub const fn is_none(self) -> bool {
                self.0 == 0
            }
        }

        impl From<$inner> for $name {
            #[inline]
            fn from(v: $inner) -> Self {
                Self(v)
            }
        }

        impl From<$name> for $inner {
            #[inline]
            fn from(v: $name) -> Self {
                v.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }
    };
}

numeric_id!(
    /// Une boîte configurée dans l'application.
    AccountId, i64, "acc:"
);
numeric_id!(
    /// Un dossier distant (IMAP) rattaché à un compte.
    FolderId, i64, "fld:"
);
numeric_id!(
    /// Un message tel que stocké localement. Distinct du `Message-ID` du protocole.
    MessageId, i64, "msg:"
);
numeric_id!(
    /// Un fil de conversation. Porte l'état de workflow.
    ThreadId, i64, "thr:"
);
numeric_id!(
    /// Une entrée du journal d'opérations en attente de réconciliation.
    OpId, i64, "op:"
);

/// Identifiant de contenu d'un corps ou d'une pièce jointe.
///
/// Adressé par le contenu (BLAKE3 tronqué à 128 bits) : deux messages portant la même
/// pièce jointe ne la stockent qu'une fois, et un même corps resynchronisé ne crée pas
/// de doublon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlobId(pub [u8; 16]);

impl BlobId {
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Représentation hexadécimale, utilisée comme nom de fichier.
    pub fn to_hex(self) -> String {
        let mut s = String::with_capacity(32);
        for b in self.0 {
            use fmt::Write as _;
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 32 {
            return None;
        }
        let mut out = [0u8; 16];
        for (i, chunk) in s.as_bytes().chunks_exact(2).enumerate() {
            let hi = (chunk[0] as char).to_digit(16)?;
            let lo = (chunk[1] as char).to_digit(16)?;
            out[i] = (hi * 16 + lo) as u8;
        }
        Some(Self(out))
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Le `Message-ID` défini par le RFC 5322, sans les chevrons.
///
/// Conservé tel quel pour le threading : c'est une chaîne fournie par l'expéditeur,
/// pas un identifiant que nous contrôlons.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RfcMessageId(pub String);

impl RfcMessageId {
    /// Normalise une valeur brute d'en-tête : retire les chevrons et les espaces.
    ///
    /// Retourne `None` pour une valeur vide, qui ne doit jamais servir de clé de
    /// threading — sinon tous les messages sans `Message-ID` fusionneraient.
    pub fn parse(raw: &str) -> Option<Self> {
        let t = raw
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim();
        if t.is_empty() {
            None
        } else {
            Some(Self(t.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RfcMessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{}>", self.0)
    }
}

/// UID IMAP, unique au sein d'un couple (dossier, `UIDVALIDITY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Uid(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_identifiants_numeriques_sont_transparents() {
        let a = AccountId::from(42);
        assert_eq!(a.get(), 42);
        assert_eq!(i64::from(a), 42);
        assert_eq!(a.to_string(), "acc:42");
        assert!(AccountId::NONE.is_none());
        assert!(!a.is_none());
    }

    #[test]
    fn les_identifiants_de_types_differents_ne_se_melangent_pas() {
        // Ce test documente une propriété du compilateur : la ligne suivante ne
        // compilerait pas, et c'est tout l'intérêt des enveloppes.
        //     let _: ThreadId = AccountId::from(1);
        let t = ThreadId::from(1);
        let a = AccountId::from(1);
        assert_eq!(t.get(), a.get());
    }

    #[test]
    fn blob_id_fait_un_aller_retour_hexadecimal() {
        let id = BlobId::from_bytes([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ]);
        let hex = id.to_hex();
        assert_eq!(hex, "00112233445566778899aabbccddeeff");
        assert_eq!(BlobId::from_hex(&hex), Some(id));
    }

    #[test]
    fn blob_id_rejette_les_chaines_invalides() {
        assert_eq!(BlobId::from_hex("trop court"), None);
        assert_eq!(BlobId::from_hex(&"z".repeat(32)), None);
    }

    #[test]
    fn message_id_retire_les_chevrons() {
        let id = RfcMessageId::parse("  <abc@example.com> ").unwrap();
        assert_eq!(id.as_str(), "abc@example.com");
        assert_eq!(id.to_string(), "<abc@example.com>");
    }

    #[test]
    fn message_id_vide_est_refuse() {
        // Sans cette garde, tous les messages dépourvus de Message-ID seraient
        // recollés dans un unique fil géant.
        assert_eq!(RfcMessageId::parse("<>"), None);
        assert_eq!(RfcMessageId::parse("   "), None);
    }
}
