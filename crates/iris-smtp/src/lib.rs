//! `iris-smtp` — composition et envoi.
//!
//! La composition ([`compose`]) est pure et intégralement testée : c'est là que se
//! joue la qualité perçue d'un client mail, parce qu'une chaîne `References` mal
//! construite casse la conversation chez le destinataire, sans que personne ne le
//! voie de son côté.
//!
//! L'envoi passe par un trait, ce qui permet de tester tout le flux — y compris
//! l'annulation de dix secondes — sans serveur.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod compose;
mod outbox;
mod transport;

pub use compose::{
    build_references, forward, generate_message_id, reply, Attachment, Outgoing, ReplyScope,
    ReplyTarget,
};
pub use outbox::{Outbox, OutboxEvent, SendHandle};
pub use transport::{FakeMailer, LettreMailer, Mailer, SendOutcome};

#[cfg(test)]
mod tests {
    use super::*;
    use iris_types::Address;

    #[test]
    fn la_crate_expose_le_necessaire_pour_repondre() {
        // Test de surface : la composition doit être utilisable sans connaître les
        // modules internes.
        let m = Outgoing::new(
            Address::new("moi@example.com"),
            vec![Address::new("marie@example.com")],
            "Devis",
        )
        .body("Bonjour");
        assert!(m.validate().is_ok());
    }
}
