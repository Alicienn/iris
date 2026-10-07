//! Mail written now to leave later.
//!
//! The draft is kept in the base (`scheduled_mail`), not a composed message: it is
//! composed when its time comes, with that date, and one taken back opens in the
//! composer as it was written. Sending is the ordinary path, the outbox, with no undo
//! delay: the waiting was the delay.

use crate::services::{now, Services};
use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Weekday};
use iris_types::{AccountId, Result, Timestamp};
use serde::{Deserialize, Serialize};

/// What is kept of a draft: everything the composer shows.
#[derive(Debug, Serialize, Deserialize)]
struct Enveloppe {
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    body: String,
    pieces: Vec<Piece>,
    /// The alias it goes out as, when not the mailbox's own address.
    #[serde(default)]
    alias: Option<iris_types::Address>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Piece {
    filename: String,
    mime_type: String,
    /// Base64: as a list of numbers, a 10 MB attachment took about 40 MB of JSON.
    /// Drafts kept before read either way.
    #[serde(with = "contenu")]
    content: Vec<u8>,
}

/// An attachment's bytes in the kept draft: written as base64, read as base64 or as
/// the list of numbers older versions wrote.
mod contenu {
    use serde::{Deserialize, Deserializer, Serializer};

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(octets: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let mut sortie = String::with_capacity(octets.len().div_ceil(3) * 4);
        for bloc in octets.chunks(3) {
            let n = (u32::from(bloc[0]) << 16)
                | (u32::from(*bloc.get(1).unwrap_or(&0)) << 8)
                | u32::from(*bloc.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= bloc.len() {
                    sortie.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    sortie.push('=');
                }
            }
        }
        s.serialize_str(&sortie)
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Forme {
        Texte(String),
        Nombres(Vec<u8>),
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        match Forme::deserialize(d)? {
            Forme::Nombres(n) => Ok(n),
            Forme::Texte(t) => decode(&t).ok_or_else(|| serde::de::Error::custom("bad base64")),
        }
    }

    fn decode(texte: &str) -> Option<Vec<u8>> {
        let mut sortie = Vec::with_capacity(texte.len() / 4 * 3);
        let (mut bits, mut nombre) = (0u32, 0);
        for b in texte.bytes().filter(|b| *b != b'=') {
            let v = ALPHABET.iter().position(|a| *a == b)? as u32;
            bits = (bits << 6) | v;
            nombre += 6;
            if nombre >= 8 {
                nombre -= 8;
                sortie.push(((bits >> nombre) & 0xff) as u8);
            }
        }
        Some(sortie)
    }
}

fn pack(d: &iris_sync::Draft, alias: Option<&iris_types::Address>) -> String {
    serde_json::to_string(&Enveloppe {
        alias: alias.cloned(),
        to: d.to.clone(),
        cc: d.cc.clone(),
        bcc: d.bcc.clone(),
        subject: d.subject.clone(),
        body: d.body.clone(),
        pieces: d
            .attachments
            .iter()
            .map(|a| Piece {
                filename: a.filename.clone(),
                mime_type: a.mime_type.clone(),
                content: a.content.clone(),
            })
            .collect(),
    })
    .unwrap_or_default()
}

fn unpack(
    account: AccountId,
    payload: &str,
) -> Option<(iris_sync::Draft, Option<iris_types::Address>)> {
    let e: Enveloppe = serde_json::from_str(payload).ok()?;
    let alias = e.alias;
    let d = iris_sync::Draft {
        account,
        to: e.to,
        cc: e.cc,
        bcc: e.bcc,
        subject: e.subject,
        body: e.body,
        attachments: e
            .pieces
            .into_iter()
            .map(|p| iris_smtp::Attachment {
                filename: p.filename,
                mime_type: p.mime_type,
                content: p.content,
            })
            .collect(),
    };
    Some((d, alias))
}

/// The times offered, from `maintenant`: this evening (before five), tomorrow morning
/// and afternoon, and Monday morning when tomorrow is not Monday already.
pub fn options(maintenant: NaiveDateTime) -> Vec<(String, NaiveDateTime)> {
    let aujourd_hui = maintenant.date();
    let a = |jour: NaiveDate, h: u32| jour.and_time(NaiveTime::from_hms_opt(h, 0, 0).unwrap());
    let demain = aujourd_hui + Duration::days(1);
    let mut sortie = Vec::new();
    if maintenant.time() < NaiveTime::from_hms_opt(17, 0, 0).unwrap() {
        sortie.push(("This evening, 18:00".to_string(), a(aujourd_hui, 18)));
    }
    sortie.push(("Tomorrow morning, 08:00".to_string(), a(demain, 8)));
    sortie.push(("Tomorrow afternoon, 13:00".to_string(), a(demain, 13)));
    if demain.weekday() != Weekday::Mon {
        let lundi = (2..=8)
            .map(|k| aujourd_hui + Duration::days(k))
            .find(|d| d.weekday() == Weekday::Mon)
            .unwrap_or(demain);
        sortie.push(("Monday morning, 08:00".to_string(), a(lundi, 8)));
    }
    sortie
}

/// "Today, 18:00", "Tomorrow, 08:00", "Mon 5 Oct, 08:00".
pub fn when_label(t: Timestamp, maintenant: NaiveDateTime) -> String {
    let Some(local) = Local
        .timestamp_millis_opt(t.millis())
        .earliest()
        .map(|d| d.naive_local())
    else {
        return String::new();
    };
    let jour = match (local.date() - maintenant.date()).num_days() {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        _ => local.format("%a %-d %b").to_string(),
    };
    format!("{jour}, {}", local.format("%H:%M"))
}

/// Keeps `draft` to send at `quand` (local time); what the notice says.
pub fn schedule(
    services: &Services,
    draft: &iris_sync::Draft,
    alias: Option<&iris_types::Address>,
    quand: NaiveDateTime,
) -> Result<String> {
    let a = Timestamp::from_millis(iris_calendar::time::zoned_millis(quand, &Local));
    services.store.schedule_mail(
        draft.account,
        a,
        draft.to.trim(),
        draft.subject.trim(),
        &pack(draft, alias),
        now(),
    )?;
    Ok(format!(
        "Will be sent {}.",
        when_label(a, Local::now().naive_local()).to_lowercase()
    ))
}

/// The waiting messages, for their window.
pub fn rows(services: &Services) -> Vec<iris_ui::ScheduledMailData> {
    let maintenant = Local::now().naive_local();
    services
        .store
        .scheduled_mail_list()
        .unwrap_or_default()
        .into_iter()
        .map(|m| iris_ui::ScheduledMailData {
            id: m.id as i32,
            to: m.to_line.as_str().into(),
            subject: m.subject.as_str().into(),
            when: when_label(m.send_at, maintenant).into(),
        })
        .collect()
}

/// Takes one out of the waiting ones, and gives its draft back, with the address of
/// the alias it was to go out as.
pub fn take(services: &Services, id: i64) -> Option<(iris_sync::Draft, Option<String>)> {
    let m = services.store.scheduled_mail_by_id(id).ok().flatten()?;
    let (d, alias) = unpack(m.account, &m.payload)?;
    services.store.unschedule_mail(id).ok()?;
    Some((d, alias.map(|a| a.addr)))
}

/// Hands every message whose time has come (or `seulement` that one, now) to the
/// outbox, without an undo delay. Each stays on the list until it has actually left:
/// `avis` takes it off then, or keeps it for another try if it did not. Messages
/// already on their way are not sent twice. Gives how many went to the outbox, and
/// what could not even be composed.
pub fn send_due(
    services: &Services,
    send: &iris_sync::SendService,
    avis: &crate::shell::AvisEnvoi,
    seulement: Option<i64>,
) -> (usize, Vec<String>) {
    let prets: Vec<_> = match seulement {
        Some(id) => services
            .store
            .scheduled_mail_by_id(id)
            .ok()
            .flatten()
            .into_iter()
            .collect(),
        None => services.store.due_scheduled_mail(now()).unwrap_or_default(),
    };
    let en_route = avis.programmes_en_vol();
    let mut partis = 0;
    let mut echecs = Vec::new();
    for m in prets.into_iter().filter(|m| !en_route.contains(&m.id)) {
        let Some((brouillon, alias)) = unpack(m.account, &m.payload) else {
            echecs.push(format!("“{}” could not be read back", m.subject));
            let _ = services.store.unschedule_mail(m.id);
            continue;
        };
        let envoye = send.compose_full(&brouillon).and_then(|mut message| {
            if let Some(a) = alias {
                message.from = a;
            }
            avis.programmer(message, m.id, m.subject.clone(), services.clone())
        });
        match envoye {
            Ok(()) => partis += 1,
            Err(e) => echecs.push(format!("“{}”: {e}", m.subject)),
        }
    }
    (partis, echecs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn the_times_offered_follow_the_clock_and_the_week() {
        // Wednesday morning: this evening, tomorrow twice, Monday.
        let o = options(a("2026-09-30 10:00"));
        assert_eq!(o.len(), 4);
        assert_eq!(o[0].1, a("2026-09-30 18:00"));
        assert_eq!(o[1].1, a("2026-10-01 08:00"));
        assert_eq!(o[3].1, a("2026-10-05 08:00"));
        // Wednesday evening: no "this evening".
        assert!(options(a("2026-09-30 19:00"))
            .iter()
            .all(|(l, _)| !l.starts_with("This evening")));
        // Sunday: tomorrow is Monday, offered once.
        assert!(options(a("2026-10-04 10:00"))
            .iter()
            .all(|(l, _)| !l.starts_with("Monday")));
    }

    #[test]
    fn attachments_are_kept_as_base64_and_old_drafts_still_read() {
        let d = iris_sync::Draft {
            account: AccountId(1),
            to: "b@example.com".into(),
            cc: String::new(),
            bcc: String::new(),
            subject: "Devis".into(),
            body: String::new(),
            attachments: vec![iris_smtp::Attachment {
                filename: "a.bin".into(),
                mime_type: "application/octet-stream".into(),
                content: (0..=255u8).collect(),
            }],
        };
        let paquet = pack(&d, None);
        assert!(
            !paquet.contains("[0,1,2"),
            "not a list of numbers: {paquet}"
        );
        assert_eq!(unpack(AccountId(1), &paquet).unwrap().0, d);

        let ancien = r#"{"to":"b@example.com","cc":"","bcc":"","subject":"S","body":"",
            "pieces":[{"filename":"x","mime_type":"text/plain","content":[104,105]}]}"#;
        assert_eq!(
            unpack(AccountId(1), ancien).unwrap().0.attachments[0].content,
            b"hi"
        );
    }

    #[test]
    fn a_draft_is_kept_whole_and_given_back() {
        let d = iris_sync::Draft {
            account: AccountId(3),
            to: "b@example.com".into(),
            cc: "c@example.com".into(),
            bcc: String::new(),
            subject: "Devis".into(),
            body: "Bonjour,\nci-joint.".into(),
            attachments: vec![iris_smtp::Attachment {
                filename: "devis.pdf".into(),
                mime_type: "application/pdf".into(),
                content: vec![0, 1, 2, 255],
            }],
        };
        let alias = iris_types::Address::named("Support", "support@example.com");
        assert_eq!(
            unpack(AccountId(3), &pack(&d, Some(&alias))),
            Some((d, Some(alias)))
        );
    }
}
