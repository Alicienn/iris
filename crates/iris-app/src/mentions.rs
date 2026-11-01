//! Words of a message that speak of an attachment: Send asks before a message that
//! says "attached" leaves with nothing attached.
//!
//! Only what the user wrote counts. The quoted conversation of a reply is not in the
//! writing field (it is drawn under it and added when sending), but a pasted quote is:
//! lines starting with `>` are skipped, and everything from an "On … wrote:" or a
//! forwarded message's header on is the earlier message, not this one.

/// The words, lowercase, that say something is attached. English and French, the
/// languages Iris is used in; each is matched as whole words.
const MOTS: &[&str] = &[
    "attached",
    "attachment",
    "attachments",
    "attaching",
    "enclosed",
    "ci-joint",
    "ci-jointe",
    "ci-joints",
    "ci-jointes",
    "ci joint",
    "ci jointe",
    "pièce jointe",
    "pièces jointes",
    "piece jointe",
    "pieces jointes",
    "en pj",
    "en p.j.",
    "joint à ce",
    "jointe à ce",
    "je joins",
    "je vous joins",
    "je te joins",
];

/// Whether `body`, what the user wrote, speaks of an attachment.
pub fn mentions_attachment(body: &str) -> bool {
    let ecrit = own_words(body).to_lowercase();
    MOTS.iter().any(|mot| contains_word(&ecrit, mot))
}

/// The user's own words: up to the header of a quoted or forwarded message, without
/// the lines quoted with `>`.
fn own_words(body: &str) -> String {
    let mut garde = Vec::new();
    for ligne in body.lines() {
        let l = ligne.trim();
        if is_quote_header(l) {
            break;
        }
        if l.starts_with('>') {
            continue;
        }
        garde.push(ligne);
    }
    garde.join("\n")
}

/// "On Tue, 3 Oct 2026, Marie wrote:", "Le mar. 3 oct. 2026, Marie a écrit :", a
/// forwarded message's or Outlook's header.
fn is_quote_header(ligne: &str) -> bool {
    let l = ligne.to_lowercase();
    (l.starts_with("on ") && l.ends_with("wrote:"))
        || (l.starts_with("le ") && (l.ends_with("a écrit :") || l.ends_with("a écrit:")))
        || l.starts_with("---------- forwarded message")
        || l.starts_with("-------- message transféré")
        || l.starts_with("---------- message transféré")
        || l.starts_with("-----original message-----")
        || l.starts_with("-----message d'origine-----")
}

/// `mot` in `texte` with no letter or digit glued on either side.
fn contains_word(texte: &str, mot: &str) -> bool {
    let colle = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric());
    let mut depuis = 0;
    while let Some(i) = texte[depuis..].find(mot) {
        let debut = depuis + i;
        let fin = debut + mot.len();
        if !colle(texte[..debut].chars().next_back()) && !colle(texte[fin..].chars().next()) {
            return true;
        }
        depuis = debut + mot.len().max(1);
        // Stay on a character boundary.
        while !texte.is_char_boundary(depuis) {
            depuis += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_that_speak_of_an_attachment_are_found() {
        for corps in [
            "Please find attached the quote.",
            "The invoice is in the attachment",
            "Voici le devis ci-joint.",
            "Tu trouveras la facture en pièce jointe.",
            "Je vous joins le contrat.",
            "Attached: the slides",
        ] {
            assert!(mentions_attachment(corps), "{corps}");
        }
    }

    #[test]
    fn other_words_are_not() {
        assert!(!mentions_attachment("Thanks for your help."));
        assert!(!mentions_attachment("The jointed frame is fine."));
        assert!(!mentions_attachment("Rejoins-nous demain."));
        assert!(!mentions_attachment("A detached house."));
        assert!(!mentions_attachment(""));
    }

    #[test]
    fn a_quoted_conversation_does_not_count() {
        let reponse = "Thanks, received.\n\n\
            On Tue, 3 Oct 2026, Marie <marie@example.com> wrote:\n\
            Please find attached the quote.";
        assert!(!mentions_attachment(reponse));
        let francais = "Merci !\n\nLe mar. 3 oct. 2026, Marie a écrit :\nVoici le devis ci-joint.";
        assert!(!mentions_attachment(francais));
        assert!(!mentions_attachment(
            "Got it.\n> the file attached\n> thanks"
        ));
        let transfere = "FYI\n---------- Forwarded message ---------\nSee attached.";
        assert!(!mentions_attachment(transfere));
        // Written above the quote, it counts.
        assert!(mentions_attachment(
            "Here is mine attached.\n\nOn Tue, Marie wrote:\nAnd yours?"
        ));
    }
}
