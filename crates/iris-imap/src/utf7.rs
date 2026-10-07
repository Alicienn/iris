//! Folder names as IMAP writes them: modified UTF-7 (RFC 3501, 5.1.3).
//!
//! Printable ASCII stands for itself, `&` is written `&-`, and anything else is
//! UTF-16 in a base64 whose `/` is a `,`, between `&` and `-`. "Envoyés" is
//! `Envoy&AOk-s` on the wire. Names were shown and matched as they came, so a French
//! "Envoyés" folder read `Envoy&AOk-s`, never matched the names that give a folder its
//! role, and a folder created as "Réunions" was refused by the server.
//!
//! Names are decoded as they arrive (`list_folders`) and encoded as they leave (every
//! command that names a folder). The rest of Iris only sees readable names.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+,";

/// A readable name, as the server must be sent it.
pub fn encode(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut attente: Vec<u16> = Vec::new();
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut attente, &mut out);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            let mut tampon = [0u16; 2];
            attente.extend_from_slice(c.encode_utf16(&mut tampon));
        }
    }
    flush(&mut attente, &mut out);
    out
}

/// Writes the waiting UTF-16 units as one `&…-` run.
fn flush(attente: &mut Vec<u16>, out: &mut String) {
    if attente.is_empty() {
        return;
    }
    let octets: Vec<u8> = attente.iter().flat_map(|u| u.to_be_bytes()).collect();
    out.push('&');
    let mut bits: u32 = 0;
    let mut nombre = 0;
    for o in octets {
        bits = (bits << 8) | u32::from(o);
        nombre += 8;
        while nombre >= 6 {
            nombre -= 6;
            out.push(ALPHABET[((bits >> nombre) & 0x3f) as usize] as char);
        }
    }
    if nombre > 0 {
        out.push(ALPHABET[((bits << (6 - nombre)) & 0x3f) as usize] as char);
    }
    out.push('-');
    attente.clear();
}

/// A name as the server sent it, made readable. One that is not valid modified UTF-7
/// (a server sending UTF-8 already) is given back as it came.
pub fn decode(name: &str) -> String {
    try_decode(name).unwrap_or_else(|| name.to_string())
}

fn try_decode(name: &str) -> Option<String> {
    let mut out = String::with_capacity(name.len());
    let mut reste = name;
    while let Some(debut) = reste.find('&') {
        out.push_str(&reste[..debut]);
        let apres = &reste[debut + 1..];
        let fin = apres.find('-')?;
        let code = &apres[..fin];
        if code.is_empty() {
            out.push('&');
        } else {
            out.push_str(&decode_run(code)?);
        }
        reste = &apres[fin + 1..];
    }
    out.push_str(reste);
    Some(out)
}

/// One `&…-` run's inside, as text.
fn decode_run(code: &str) -> Option<String> {
    let mut octets = Vec::new();
    let mut bits: u32 = 0;
    let mut nombre = 0;
    for b in code.bytes() {
        let valeur = ALPHABET.iter().position(|&a| a == b)? as u32;
        bits = (bits << 6) | valeur;
        nombre += 6;
        if nombre >= 8 {
            nombre -= 8;
            octets.push(((bits >> nombre) & 0xff) as u8);
        }
    }
    if octets.len() % 2 != 0 {
        return None;
    }
    let unites: Vec<u16> = octets
        .chunks(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .collect();
    String::from_utf16(&unites).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accented_names_round_trip() {
        assert_eq!(encode("Envoyés"), "Envoy&AOk-s");
        assert_eq!(decode("Envoy&AOk-s"), "Envoyés");
        assert_eq!(encode("Éléments envoyés"), "&AMk-l&AOk-ments envoy&AOk-s");
        for nom in [
            "Réunions",
            "Courrier indésirable",
            "日本語",
            "Ünïcödé/Sous-dossier",
            "😀",
        ] {
            assert_eq!(decode(&encode(nom)), nom, "{nom}");
        }
    }

    #[test]
    fn the_rfc_example_reads() {
        // RFC 3501, 5.1.3.
        assert_eq!(
            decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-"),
            "~peter/mail/台北/日本語"
        );
    }

    #[test]
    fn an_ampersand_is_escaped() {
        assert_eq!(encode("R&D"), "R&-D");
        assert_eq!(decode("R&-D"), "R&D");
    }

    #[test]
    fn plain_ascii_is_left_alone() {
        assert_eq!(encode("INBOX.Devis 2026"), "INBOX.Devis 2026");
        assert_eq!(decode("[Gmail]/Sent Mail"), "[Gmail]/Sent Mail");
    }

    #[test]
    fn what_is_not_modified_utf7_comes_back_as_it_was() {
        assert_eq!(decode("Envoyés"), "Envoyés");
        assert_eq!(decode("A&B"), "A&B");
        assert_eq!(decode("A&***-"), "A&***-");
    }
}
