//! Answering an invitation: Accept, Maybe or Decline, sent back to the organiser.
//!
//! The answer is an iTIP `REPLY` (RFC 5546): the event's UID, its sequence and
//! occurrence, the organiser, and one attendee — the one answering — with their
//! `PARTSTAT`. It travels as a `text/calendar; method=REPLY` part of a short message,
//! which is what Outlook, Gmail and Apple Calendar read to mark the answer.
//!
//! Built from the invitation's own lines rather than from the parsed event: the
//! organiser's calendar matches its copy by what it wrote, parameters included.

use crate::services::{now, Services};
use iris_types::Result;

/// How an invitation is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Accept,
    Maybe,
    Decline,
}

impl Answer {
    pub fn from_index(i: i32) -> Option<Self> {
        Some(match i {
            0 => Answer::Accept,
            1 => Answer::Maybe,
            2 => Answer::Decline,
            _ => return None,
        })
    }

    pub fn partstat(self) -> &'static str {
        match self {
            Answer::Accept => "ACCEPTED",
            Answer::Maybe => "TENTATIVE",
            Answer::Decline => "DECLINED",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Answer::Accept => "Accepted",
            Answer::Maybe => "Tentative",
            Answer::Decline => "Declined",
        }
    }
}

/// An iCalendar text's lines, unfolded (a line starting with a space or a tab goes on
/// the one before).
fn lines(texte: &str) -> Vec<String> {
    let mut sortie: Vec<String> = Vec::new();
    for brute in texte.split('\n') {
        let l = brute.trim_end_matches('\r');
        if let Some(suite) = l.strip_prefix([' ', '\t']) {
            if let Some(derniere) = sortie.last_mut() {
                derniere.push_str(suite);
                continue;
            }
        }
        sortie.push(l.to_string());
    }
    sortie
}

/// A property line's name (without its parameters) and value: the value starts at the
/// first colon outside quotes.
fn split(ligne: &str) -> (String, &str, &str) {
    let mut guillemets = false;
    for (i, c) in ligne.char_indices() {
        match c {
            '"' => guillemets = !guillemets,
            ':' if !guillemets => {
                let tete = &ligne[..i];
                let nom = tete.split(';').next().unwrap_or("").to_ascii_uppercase();
                return (nom, tete, &ligne[i + 1..]);
            }
            _ => {}
        }
    }
    (String::new(), ligne, "")
}

/// The address in `mailto:x@y`, lower case.
fn adresse(valeur: &str) -> String {
    let v = valeur.trim();
    v.get(..7)
        .filter(|p| p.eq_ignore_ascii_case("mailto:"))
        .map(|_| &v[7..])
        .unwrap_or(v)
        .trim()
        .to_ascii_lowercase()
}

/// What an invitation needs to be answered: who organises it, and whether it asks
/// (`METHOD:REQUEST`, or no method at all as some senders write it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replyable {
    pub organizer: String,
    pub uid: String,
    pub summary: String,
}

/// Can `moi` answer this invitation? Not a cancellation, not one they organise, and
/// with an organiser to answer to.
pub fn replyable(texte: &str, moi: &str) -> Option<Replyable> {
    let lignes = lines(texte);
    let methode = lignes
        .iter()
        .map(|l| split(l))
        .find(|(n, _, _)| n == "METHOD")
        .map(|(_, _, v)| v.trim().to_ascii_uppercase());
    if methode.as_deref().is_some_and(|m| m != "REQUEST") {
        return None;
    }
    let (mut organizer, mut uid, mut summary) = (None, None, String::new());
    let mut dedans = false;
    for l in &lignes {
        let (nom, _, valeur) = split(l);
        match nom.as_str() {
            "BEGIN" if valeur.eq_ignore_ascii_case("VEVENT") => dedans = true,
            "END" if valeur.eq_ignore_ascii_case("VEVENT") => break,
            "ORGANIZER" if dedans => organizer = Some(adresse(valeur)),
            "UID" if dedans => uid = Some(valeur.trim().to_string()),
            "SUMMARY" if dedans => summary = valeur.replace("\\,", ",").replace("\\;", ";"),
            _ => {}
        }
    }
    let organizer = organizer.filter(|o| !o.is_empty() && !o.eq_ignore_ascii_case(moi))?;
    Some(Replyable {
        organizer,
        uid: uid?,
        summary,
    })
}

/// The `REPLY` for `moi` answering `answer`, built from the invitation's first event.
pub fn reply_ics(texte: &str, moi: &str, answer: Answer, stamp: &str) -> Option<String> {
    let lignes = lines(texte);
    let mut garde: Vec<String> = Vec::new();
    let mut participant: Option<String> = None;
    let mut dedans = false;
    for l in &lignes {
        let (nom, tete, valeur) = split(l);
        match nom.as_str() {
            "BEGIN" if valeur.eq_ignore_ascii_case("VEVENT") => dedans = true,
            "END" if valeur.eq_ignore_ascii_case("VEVENT") => break,
            // What identifies the event and the occurrence, as the organiser wrote it.
            "UID" | "SEQUENCE" | "RECURRENCE-ID" | "DTSTART" | "DTEND" | "DURATION" | "SUMMARY"
            | "ORGANIZER"
                if dedans =>
            {
                garde.push(l.clone())
            }
            // Oneself among the attendees, to keep their name; the answer replaces
            // their status.
            "ATTENDEE" if dedans && adresse(valeur) == moi.to_ascii_lowercase() => {
                let params: Vec<&str> = tete
                    .split(';')
                    .skip(1)
                    .filter(|p| {
                        let cle = p.split('=').next().unwrap_or("").to_ascii_uppercase();
                        !matches!(cle.as_str(), "PARTSTAT" | "RSVP")
                    })
                    .collect();
                let mut ligne = String::from("ATTENDEE");
                for p in params {
                    ligne.push(';');
                    ligne.push_str(p);
                }
                ligne.push_str(&format!(
                    ";PARTSTAT={}:{}",
                    answer.partstat(),
                    valeur.trim()
                ));
                participant = Some(ligne);
            }
            _ => {}
        }
    }
    if !garde.iter().any(|l| split(l).0 == "UID") {
        return None;
    }
    let participant = participant
        .unwrap_or_else(|| format!("ATTENDEE;PARTSTAT={}:mailto:{moi}", answer.partstat()));
    let mut sortie = vec![
        "BEGIN:VCALENDAR".to_string(),
        "PRODID:-//Iris//Iris Mail//EN".to_string(),
        "VERSION:2.0".to_string(),
        "METHOD:REPLY".to_string(),
        "BEGIN:VEVENT".to_string(),
        format!("DTSTAMP:{stamp}"),
    ];
    sortie.extend(garde);
    sortie.push(participant);
    sortie.push("END:VEVENT".to_string());
    sortie.push("END:VCALENDAR".to_string());
    // Folded at 75 octets, as the format wants long lines.
    let plie: Vec<String> = sortie.iter().map(|l| fold(l)).collect();
    Some(plie.join("\r\n") + "\r\n")
}

fn fold(ligne: &str) -> String {
    let mut sortie = String::new();
    let mut compte = 0;
    for c in ligne.chars() {
        let n = c.len_utf8();
        if compte + n > 75 {
            sortie.push_str("\r\n ");
            compte = 1;
        }
        sortie.push(c);
        compte += n;
    }
    sortie
}

/// Answers the invitation carried by `texte` for the mailbox `moi`: the reply goes to
/// the organiser from that mailbox, the calendar follows (added for Accept and Maybe,
/// taken out for Decline), and the answer is kept for the banner.
pub fn respond(
    services: &Services,
    send: &iris_sync::SendService,
    account: iris_types::AccountId,
    moi: &str,
    texte: &str,
    answer: Answer,
) -> Result<String> {
    let cible = replyable(texte, moi)
        .ok_or_else(|| iris_types::Error::other("this invitation cannot be answered"))?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let ics = reply_ics(texte, moi, answer, &stamp)
        .ok_or_else(|| iris_types::Error::other("the invitation could not be read"))?;

    let titre = if cible.summary.trim().is_empty() {
        "your invitation".to_string()
    } else {
        cible.summary.trim().to_string()
    };
    let brouillon = iris_sync::Draft {
        account,
        to: cible.organizer.clone(),
        cc: String::new(),
        bcc: String::new(),
        subject: format!("{}: {titre}", answer.verb()),
        body: format!("{} {moi}.", answer.verb()),
        attachments: vec![iris_smtp::Attachment {
            filename: "reply.ics".into(),
            mime_type: "text/calendar; method=REPLY; charset=UTF-8".into(),
            content: ics.into_bytes(),
        }],
    };
    let message = send.compose_full(&brouillon)?;
    send.set_delay(std::time::Duration::ZERO);
    send.queue(message)?;

    // The calendar follows the answer.
    match answer {
        Answer::Accept | Answer::Maybe => {
            crate::calendar::import_ics(services, texte)?;
        }
        Answer::Decline => crate::calendar::remove_invited(services, &cible.uid),
    }
    services
        .store
        .set_invite_reply(&cible.uid, answer.partstat(), now())?;
    Ok(format!("{} sent to {}.", answer.verb(), cible.organizer))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVITATION: &str = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\n\
UID:revue-7@example.com\r\nSEQUENCE:2\r\nDTSTART;TZID=Europe/Paris:20261006T090000\r\n\
DTEND;TZID=Europe/Paris:20261006T100000\r\nSUMMARY:Revue budg\u{e9}taire\r\n\
ORGANIZER;CN=Paul:mailto:paul@example.com\r\n\
ATTENDEE;CN=\"Marie, Atelier\";RSVP=TRUE;PARTSTAT=NEEDS-ACTION:mailto:marie@exam\r\n ple.com\r\n\
ATTENDEE;CN=Luc:mailto:luc@example.com\r\nDESCRIPTION:Salle 3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn an_invitation_is_answered_with_what_identifies_it() {
        let r = replyable(INVITATION, "marie@example.com").unwrap();
        assert_eq!(r.organizer, "paul@example.com");
        assert_eq!(r.uid, "revue-7@example.com");

        let ics = reply_ics(
            INVITATION,
            "marie@example.com",
            Answer::Accept,
            "20260930T120000Z",
        )
        .unwrap();
        assert!(ics.contains("METHOD:REPLY"));
        assert!(ics.contains("UID:revue-7@example.com"));
        assert!(ics.contains("SEQUENCE:2"));
        assert!(ics.contains("DTSTART;TZID=Europe/Paris:20261006T090000"));
        assert!(ics.contains("ORGANIZER;CN=Paul:mailto:paul@example.com"));
        // Oneself, with one's name (its comma kept inside the quotes), and the answer.
        let unfolded = ics.replace("\r\n ", "");
        assert!(unfolded
            .contains("ATTENDEE;CN=\"Marie, Atelier\";PARTSTAT=ACCEPTED:mailto:marie@example.com"));
        // Not the others, not what the reply does not need.
        assert!(!ics.contains("luc@example.com"));
        assert!(!ics.contains("DESCRIPTION"));
        assert!(ics.lines().all(|l| l.trim_end_matches('\r').len() <= 75));
    }

    #[test]
    fn one_answers_neither_a_cancellation_nor_ones_own_invitation() {
        let annulee = INVITATION.replace("METHOD:REQUEST", "METHOD:CANCEL");
        assert!(replyable(&annulee, "marie@example.com").is_none());
        assert!(replyable(INVITATION, "paul@example.com").is_none());
        let sans = INVITATION.replace("ORGANIZER;CN=Paul:mailto:paul@example.com\r\n", "");
        assert!(replyable(&sans, "marie@example.com").is_none());
    }

    #[test]
    fn an_attendee_not_listed_answers_all_the_same() {
        let ics = reply_ics(
            INVITATION,
            "zoe@example.com",
            Answer::Decline,
            "20260930T120000Z",
        )
        .unwrap();
        assert!(ics.contains("ATTENDEE;PARTSTAT=DECLINED:mailto:zoe@example.com"));
    }
}
