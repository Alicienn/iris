//! Enveloppe d'un message : tout ce qui est nécessaire pour l'afficher dans une
//! liste, le classer et le threader, sans jamais toucher au corps.
//!
//! C'est la structure la plus lue de l'application. Elle ne contient volontairement
//! aucune donnée de taille non bornée : le corps, les parties MIME et les pièces
//! jointes vivent ailleurs et ne sont chargés qu'à l'ouverture.

use crate::{Address, BlobId, FolderId, MessageId, RfcMessageId, ThreadId, Uid};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Instant en millisecondes depuis l'époque Unix, en UTC.
///
/// Un type dédié plutôt qu'un `i64` nu : les dates de mails proviennent d'en-têtes
/// non fiables et se mélangent facilement avec les dates de réception locales.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub const EPOCH: Self = Self(0);

    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    pub const fn millis(self) -> i64 {
        self.0
    }

    pub const fn seconds(self) -> i64 {
        self.0.div_euclid(1000)
    }

    /// Écart en secondes, toujours positif.
    pub const fn distance_secs(self, other: Self) -> i64 {
        (self.0 - other.0).abs().div_euclid(1000)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

/// Drapeaux IMAP standard, plus ceux que nous ajoutons.
///
/// Représentés en champ de bits : une liste de messages en manipule des centaines de
/// milliers, et une allocation par message serait rédhibitoire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Flags(pub u32);

impl Flags {
    pub const NONE: Self = Self(0);
    pub const SEEN: Self = Self(1 << 0);
    pub const ANSWERED: Self = Self(1 << 1);
    pub const FLAGGED: Self = Self(1 << 2);
    pub const DRAFT: Self = Self(1 << 3);
    pub const DELETED: Self = Self(1 << 4);
    pub const RECENT: Self = Self(1 << 5);
    /// Le message porte au moins une pièce jointe réelle (hors parties inline).
    pub const HAS_ATTACHMENT: Self = Self(1 << 6);
    /// Un traqueur a été détecté à l'analyse du corps.
    pub const HAS_TRACKER: Self = Self(1 << 7);
    /// Le message propose un désabonnement exploitable.
    pub const UNSUBSCRIBABLE: Self = Self(1 << 8);

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[inline]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    #[inline]
    pub const fn set(self, other: Self, on: bool) -> Self {
        if on {
            self.with(other)
        } else {
            self.without(other)
        }
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for Flags {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        self.with(rhs)
    }
}

/// Une pièce jointe, décrite sans son contenu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentMeta {
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    /// Présent seulement une fois le contenu téléchargé.
    pub blob: Option<BlobId>,
    /// Partie référencée depuis le corps HTML par `cid:`, à ne pas lister comme
    /// pièce jointe dans l'interface.
    pub inline: bool,
}

/// Moyen de désabonnement extrait des en-têtes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unsubscribe {
    /// RFC 8058 : requête POST à un point d'accès, sans quitter l'application.
    OneClick { url: String },
    /// RFC 2369 : lien web, nécessite d'ouvrir le navigateur.
    Http { url: String },
    /// RFC 2369 : message à envoyer.
    Mailto { addr: String, subject: Option<String> },
}

/// L'enveloppe d'un message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub id: MessageId,
    pub folder: FolderId,
    pub thread: ThreadId,
    pub uid: Uid,

    pub rfc_message_id: Option<RfcMessageId>,
    pub in_reply_to: Option<RfcMessageId>,
    /// Chaîne `References`, du plus ancien au plus récent.
    pub references: Vec<RfcMessageId>,

    pub subject: String,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub reply_to: Vec<Address>,

    /// Date déclarée par l'expéditeur. Peut être absurde.
    pub date: Timestamp,
    /// Date de réception constatée par le serveur. C'est celle qui fait foi pour le tri.
    pub received: Timestamp,

    pub size: u64,
    pub flags: Flags,
    /// Extrait textuel court, calculé à la synchronisation pour éviter tout parsing
    /// pendant le défilement.
    pub preview: String,
    pub attachments: Vec<AttachmentMeta>,
    pub unsubscribe: Option<Unsubscribe>,
    /// Corps stocké localement, absent tant que le message n'a pas été ouvert.
    pub body: Option<BlobId>,
}

impl Envelope {
    /// Expéditeur principal, celui qu'affiche la liste.
    pub fn primary_from(&self) -> Option<&Address> {
        self.from.first()
    }

    /// Date de tri : la réception constatée, avec repli sur la date déclarée.
    ///
    /// Trier sur la date déclarée exposerait la liste aux expéditeurs qui datent
    /// leurs messages en l'an 2099 pour rester en tête.
    pub fn sort_date(&self) -> Timestamp {
        if self.received == Timestamp::EPOCH {
            self.date
        } else {
            self.received
        }
    }

    /// Pièces jointes réellement présentables à l'utilisateur.
    pub fn visible_attachments(&self) -> impl Iterator<Item = &AttachmentMeta> {
        self.attachments.iter().filter(|a| !a.inline)
    }

    pub fn is_seen(&self) -> bool {
        self.flags.contains(Flags::SEEN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Envelope {
        Envelope {
            id: MessageId(1),
            folder: FolderId(1),
            thread: ThreadId(1),
            uid: Uid(10),
            rfc_message_id: RfcMessageId::parse("a@x"),
            in_reply_to: None,
            references: vec![],
            subject: "Devis".into(),
            from: vec![Address::named("Marie", "marie@example.com")],
            to: vec![],
            cc: vec![],
            reply_to: vec![],
            date: Timestamp::from_millis(1_000),
            received: Timestamp::from_millis(2_000),
            size: 42,
            flags: Flags::NONE,
            preview: String::new(),
            attachments: vec![],
            unsubscribe: None,
            body: None,
        }
    }

    #[test]
    fn les_drapeaux_se_composent() {
        let f = Flags::SEEN | Flags::FLAGGED;
        assert!(f.contains(Flags::SEEN));
        assert!(f.contains(Flags::FLAGGED));
        assert!(!f.contains(Flags::DRAFT));
        assert!(f.contains(Flags::SEEN | Flags::FLAGGED));
    }

    #[test]
    fn retirer_un_drapeau_absent_ne_change_rien() {
        let f = Flags::SEEN;
        assert_eq!(f.without(Flags::DRAFT), Flags::SEEN);
        assert_eq!(f.without(Flags::SEEN), Flags::NONE);
        assert!(Flags::NONE.is_empty());
    }

    #[test]
    fn set_pose_ou_retire_selon_le_booleen() {
        assert!(Flags::NONE.set(Flags::SEEN, true).contains(Flags::SEEN));
        assert!(!Flags::SEEN.set(Flags::SEEN, false).contains(Flags::SEEN));
    }

    #[test]
    fn le_tri_utilise_la_date_de_reception() {
        let e = env();
        assert_eq!(e.sort_date(), Timestamp::from_millis(2_000));
    }

    #[test]
    fn sans_date_de_reception_on_retombe_sur_la_date_declaree() {
        let mut e = env();
        e.received = Timestamp::EPOCH;
        assert_eq!(e.sort_date(), Timestamp::from_millis(1_000));
    }

    #[test]
    fn les_parties_inline_ne_comptent_pas_comme_pieces_jointes() {
        let mut e = env();
        e.attachments = vec![
            AttachmentMeta {
                filename: "logo.png".into(),
                mime_type: "image/png".into(),
                size: 10,
                blob: None,
                inline: true,
            },
            AttachmentMeta {
                filename: "devis.pdf".into(),
                mime_type: "application/pdf".into(),
                size: 20,
                blob: None,
                inline: false,
            },
        ];
        let visibles: Vec<_> = e.visible_attachments().map(|a| a.filename.as_str()).collect();
        assert_eq!(visibles, ["devis.pdf"]);
    }

    #[test]
    fn ecart_entre_horodatages_est_absolu() {
        let a = Timestamp::from_millis(5_000);
        let b = Timestamp::from_millis(2_000);
        assert_eq!(a.distance_secs(b), 3);
        assert_eq!(b.distance_secs(a), 3);
    }
}
