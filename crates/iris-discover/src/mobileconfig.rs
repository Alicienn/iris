//! Configuration profiles (`.mobileconfig`), as hosts and IT departments hand them out
//! for iPhones and Macs.
//!
//! A profile is an XML property list, often signed: then the XML sits whole inside a
//! PKCS #7 envelope, and is read from between its first `<?xml` (or `<plist`) and its
//! `</plist>`. The signature is not checked: the profile only fills in the fields of the
//! add-account screen, which the user reads before saving, and Iris refuses an
//! unencrypted connection whatever the file says.
//!
//! Each mail account is a payload of type `com.apple.mail.managed`. POP accounts are
//! left out: Iris speaks IMAP.

use crate::{Auth, ServerConfig, Transport};
use iris_types::{Error, Result};

/// A mail account read from a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileAccount {
    /// Its name in the profile ("Work mail"), when it has one.
    pub description: Option<String>,
    /// Empty when the profile leaves the address to be typed at install.
    pub config: ServerConfig,
    /// The password, when the profile carries one.
    pub password: Option<String>,
    /// The sending password, when the profile gives one of its own.
    pub smtp_password: Option<String>,
}

/// The mail accounts of a profile, in its order.
pub fn parse(bytes: &[u8]) -> Result<Vec<ProfileAccount>> {
    if bytes.starts_with(b"bplist") {
        return Err(Error::Config(
            "this profile is in binary form: export it as XML and try again".into(),
        ));
    }
    // A signed profile: the property list is the envelope's content, which the
    // signing tool may have cut into pieces with a few bytes of framing between them.
    // Read from the envelope when it can be; from the raw bytes when it cannot.
    let contenu;
    let bytes = if bytes.first() == Some(&0x30) {
        match ber::find_content(bytes, b"<plist") {
            Some(c) => {
                contenu = c;
                &contenu[..]
            }
            None => bytes,
        }
    } else {
        bytes
    };
    let debut = find(bytes, b"<?xml")
        .or_else(|| find(bytes, b"<plist"))
        .ok_or_else(|| Error::Config("this file is not a configuration profile".into()))?;
    let fin = find(&bytes[debut..], b"</plist>")
        .map(|f| debut + f + b"</plist>".len())
        .ok_or_else(|| Error::Config("this profile is cut short".into()))?;
    let xml = String::from_utf8_lossy(&bytes[debut..fin]);

    let racine = Parser::new(&xml)
        .document()
        .ok_or_else(|| Error::Config("this profile could not be read".into()))?;

    let mut comptes = Vec::new();
    let mut pop = false;
    visit(&racine, &mut |dict| {
        if text(dict, "PayloadType") != Some("com.apple.mail.managed") {
            return;
        }
        if text(dict, "EmailAccountType") == Some("EmailTypePOP") {
            pop = true;
            return;
        }
        if let Some(c) = account(dict) {
            comptes.push(c);
        }
    });

    if comptes.is_empty() {
        return Err(Error::Config(if pop {
            "this profile holds a POP account; Iris needs IMAP".into()
        } else {
            "this profile holds no mail account".into()
        }));
    }
    Ok(comptes)
}

fn account(dict: &[(String, Value)]) -> Option<ProfileAccount> {
    let imap_host = text(dict, "IncomingMailServerHostName")?.trim().to_string();
    let smtp_host = text(dict, "OutgoingMailServerHostName")?.trim().to_string();
    if imap_host.is_empty() || smtp_host.is_empty() {
        return None;
    }
    let imap_port = number(dict, "IncomingMailServerPortNumber").unwrap_or(993);
    let smtp_port = number(dict, "OutgoingMailServerPortNumber").unwrap_or(587);
    // Never in the clear. On 993 and 465 the connection is encrypted from the start;
    // on the ports made for STARTTLS (143, 587, 25) it is upgraded, which is what
    // Apple's "use SSL" means there. On any other port the profile's "use SSL" says
    // TLS from the start: the port alone was read before, and SSL on a port of the
    // host's own (10993, 2465) was taken for STARTTLS and never connected. A
    // connection that cannot be upgraded fails rather than sends a password openly.
    let transport = |cle: &str, port: u16| match port {
        993 | 465 => Transport::Tls,
        143 | 587 | 25 => Transport::StartTls,
        _ if boolean(dict, cle) == Some(true) => Transport::Tls,
        _ => Transport::StartTls,
    };
    // The logins, when they are not the address: a corporate profile signs in as
    // `jdoe` or `DOMAIN\jdoe`, and the address was always used instead.
    let login = |cle: &str| {
        text(dict, cle)
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty())
    };
    let email = text(dict, "EmailAddress")
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let description = text(dict, "EmailAccountDescription")
        .or_else(|| text(dict, "EmailAccountName"))
        .or_else(|| text(dict, "PayloadDisplayName"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let password = text(dict, "IncomingPassword")
        .map(str::to_string)
        .filter(|p| !p.is_empty());
    // A password of its own for sending, unless the profile says it is the same.
    let smtp_password = if boolean(dict, "OutgoingPasswordSameAsIncomingPassword") == Some(true) {
        None
    } else {
        text(dict, "OutgoingPassword")
            .map(str::to_string)
            .filter(|p| !p.is_empty())
    };
    let imap_user = login("IncomingMailServerUsername").filter(|u| !u.eq_ignore_ascii_case(&email));
    let smtp_user = login("OutgoingMailServerUsername")
        .filter(|u| !u.eq_ignore_ascii_case(&email) && imap_user.as_deref() != Some(u.as_str()));

    Some(ProfileAccount {
        description: description.clone(),
        config: ServerConfig {
            provider: description,
            email,
            imap_host,
            imap_port,
            imap_transport: transport("IncomingMailServerUseSSL", imap_port),
            smtp_host,
            smtp_port,
            smtp_transport: transport("OutgoingMailServerUseSSL", smtp_port),
            auth: Auth::Password,
            note: None,
            imap_user,
            smtp_user,
        },
        password,
        smtp_password,
    })
}

/// Just enough of BER, the encoding of a signed profile's envelope (PKCS #7), to take
/// its content out: lengths definite or not, and an octet string sent in pieces put
/// back together. Nothing is verified.
mod ber {
    /// One element: its tag byte, whether it holds elements, and where its content
    /// is (`start..end`); `next` is where the element after it begins.
    struct Tlv {
        tag: u8,
        constructed: bool,
        start: usize,
        end: usize,
        next: usize,
    }

    /// The element at `pos`, or `None` when the bytes do not make one. Deep nesting is
    /// refused rather than followed: a profile is a few levels deep.
    fn tlv(d: &[u8], pos: usize, depth: u32) -> Option<Tlv> {
        if depth > 32 {
            return None;
        }
        let tag = *d.get(pos)?;
        let mut i = pos + 1;
        if tag & 0x1f == 0x1f {
            // A tag number over 30, in the bytes that follow.
            while *d.get(i)? & 0x80 != 0 {
                i += 1;
            }
            i += 1;
        }
        let constructed = tag & 0x20 != 0;
        let premier = *d.get(i)?;
        i += 1;
        if premier == 0x80 {
            // No length: the elements run to two zero bytes.
            if !constructed {
                return None;
            }
            let start = i;
            let mut p = i;
            loop {
                if d.get(p..p + 2)? == [0, 0] {
                    return Some(Tlv {
                        tag,
                        constructed,
                        start,
                        end: p,
                        next: p + 2,
                    });
                }
                p = tlv(d, p, depth + 1)?.next;
            }
        }
        let longueur = if premier < 0x80 {
            premier as usize
        } else {
            let n = (premier & 0x7f) as usize;
            if n == 0 || n > 4 {
                return None;
            }
            let l = d
                .get(i..i + n)?
                .iter()
                .fold(0usize, |a, b| (a << 8) | *b as usize);
            i += n;
            l
        };
        let end = i.checked_add(longueur)?;
        if end > d.len() {
            return None;
        }
        Some(Tlv {
            tag,
            constructed,
            start: i,
            end,
            next: end,
        })
    }

    /// The bytes of an octet string, its pieces put together.
    fn octets(d: &[u8], e: &Tlv, depth: u32, out: &mut Vec<u8>) -> Option<()> {
        if !e.constructed {
            out.extend_from_slice(&d[e.start..e.end]);
            return Some(());
        }
        let mut p = e.start;
        while p < e.end {
            let enfant = tlv(d, p, depth + 1)?;
            octets(d, &enfant, depth + 1, out)?;
            p = enfant.next;
        }
        Some(())
    }

    /// The first octet string, depth first, whose bytes contain `needle`.
    pub fn find_content(d: &[u8], needle: &[u8]) -> Option<Vec<u8>> {
        fn chercher(
            d: &[u8],
            start: usize,
            end: usize,
            needle: &[u8],
            depth: u32,
        ) -> Option<Vec<u8>> {
            let mut p = start;
            while p < end {
                let e = tlv(d, p, depth)?;
                if e.tag & 0x1f == 0x04 && e.tag & 0xc0 == 0 {
                    let mut out = Vec::new();
                    if octets(d, &e, depth, &mut out).is_some()
                        && out.windows(needle.len()).any(|w| w == needle)
                    {
                        return Some(out);
                    }
                } else if e.constructed {
                    if let Some(c) = chercher(d, e.start, e.end, needle, depth + 1) {
                        return Some(c);
                    }
                }
                p = e.next;
            }
            None
        }
        let racine = tlv(d, 0, 0)?;
        chercher(d, 0, racine.next, needle, 0)
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

// --- A property list, just enough of it ---

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Dict(Vec<(String, Value)>),
    Array(Vec<Value>),
    Text(String),
    Number(i64),
    /// `<true/>` and `<false/>`.
    Bool(bool),
    /// Data, dates: nothing Iris needs.
    Other,
}

fn boolean(dict: &[(String, Value)], key: &str) -> Option<bool> {
    dict.iter().find_map(|(k, v)| match v {
        Value::Bool(b) if k == key => Some(*b),
        _ => None,
    })
}

fn text<'a>(dict: &'a [(String, Value)], key: &str) -> Option<&'a str> {
    dict.iter().find_map(|(k, v)| match v {
        Value::Text(t) if k == key => Some(t.as_str()),
        _ => None,
    })
}

fn number(dict: &[(String, Value)], key: &str) -> Option<u16> {
    dict.iter().find_map(|(k, v)| match v {
        Value::Number(n) if k == key => u16::try_from(*n).ok().filter(|p| *p > 0),
        // Some tools write the port as a string.
        Value::Text(t) if k == key => t.trim().parse().ok().filter(|p| *p > 0),
        _ => None,
    })
}

/// Every dictionary of the tree, the outer ones first.
fn visit(v: &Value, f: &mut impl FnMut(&[(String, Value)])) {
    match v {
        Value::Dict(d) => {
            f(d);
            for (_, v) in d {
                visit(v, f);
            }
        }
        Value::Array(a) => {
            for v in a {
                visit(v, f);
            }
        }
        _ => {}
    }
}

/// A reader for the tags a property list uses. Tolerant: an element it does not know
/// is skipped whole, not an error.
struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Parser { s, i: 0 }
    }

    /// The value inside `<plist>`.
    fn document(&mut self) -> Option<Value> {
        let debut = self.s.find("<plist")?;
        self.i = debut + self.s[debut..].find('>')? + 1;
        self.value()
    }

    /// The next tag: its name, whether it closes (`</x>`), whether it is empty (`<x/>`).
    fn tag(&mut self) -> Option<(String, bool, bool)> {
        loop {
            let rel = self.s[self.i..].find('<')?;
            self.i += rel;
            let reste = &self.s[self.i..];
            if reste.starts_with("<!--") {
                self.i += reste.find("-->")? + 3;
                continue;
            }
            if reste.starts_with("<?") || reste.starts_with("<!") {
                self.i += reste.find('>')? + 1;
                continue;
            }
            let fin = reste.find('>')?;
            let dedans = &reste[1..fin];
            self.i += fin + 1;
            let fermant = dedans.starts_with('/');
            let vide = dedans.ends_with('/');
            let nom = dedans
                .trim_start_matches('/')
                .trim_end_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            return Some((nom, fermant, vide));
        }
    }

    /// The text up to the closing tag of `name`, entities decoded.
    fn content(&mut self, name: &str) -> Option<String> {
        let fermant = format!("</{name}>");
        let rel = self.s[self.i..].find(&fermant)?;
        let brut = &self.s[self.i..self.i + rel];
        self.i += rel + fermant.len();
        Some(decode(brut))
    }

    fn value(&mut self) -> Option<Value> {
        let (nom, fermant, vide) = self.tag()?;
        if fermant {
            return None;
        }
        self.value_of(&nom, vide)
    }

    fn value_of(&mut self, nom: &str, vide: bool) -> Option<Value> {
        Some(match nom {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ if vide => match nom {
                "dict" => Value::Dict(Vec::new()),
                "array" => Value::Array(Vec::new()),
                "string" => Value::Text(String::new()),
                _ => Value::Other,
            },
            "string" => Value::Text(self.content("string")?),
            "integer" => {
                let t = self.content("integer")?;
                t.trim().parse().map(Value::Number).unwrap_or(Value::Other)
            }
            "dict" => {
                let mut d = Vec::new();
                loop {
                    let (n, fermant, vide) = self.tag()?;
                    if fermant {
                        break;
                    }
                    if n != "key" || vide {
                        // Out of place: take the value and move on.
                        self.value_of(&n, vide)?;
                        continue;
                    }
                    let cle = self.content("key")?;
                    let v = self.value()?;
                    d.push((cle, v));
                }
                Value::Dict(d)
            }
            "array" => {
                let mut a = Vec::new();
                loop {
                    let (n, fermant, vide) = self.tag()?;
                    if fermant {
                        break;
                    }
                    a.push(self.value_of(&n, vide)?);
                }
                Value::Array(a)
            }
            // data, date, real and the rest: their text is of no use here.
            autre => {
                self.content(autre)?;
                Value::Other
            }
        })
    }
}

fn decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFIL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>EmailAccountDescription</key>
      <string>Marie &amp; Co</string>
      <key>EmailAccountType</key>
      <string>EmailTypeIMAP</string>
      <key>EmailAddress</key>
      <string>Marie@Example.com</string>
      <key>IncomingMailServerHostName</key>
      <string>imap.example.com</string>
      <key>IncomingMailServerPortNumber</key>
      <integer>993</integer>
      <key>IncomingMailServerUseSSL</key>
      <true/>
      <key>IncomingPassword</key>
      <string>secret</string>
      <key>OutgoingMailServerHostName</key>
      <string>smtp.example.com</string>
      <key>OutgoingMailServerPortNumber</key>
      <integer>587</integer>
      <key>OutgoingMailServerUseSSL</key>
      <true/>
      <key>PayloadIdentifier</key>
      <string>com.example.mail</string>
      <key>PayloadType</key>
      <string>com.apple.mail.managed</string>
      <key>PayloadVersion</key>
      <integer>1</integer>
    </dict>
  </array>
  <key>PayloadDisplayName</key>
  <string>Example mail</string>
  <key>PayloadType</key>
  <string>Configuration</string>
</dict>
</plist>"#;

    #[test]
    fn a_profile_fills_in_the_account() {
        let comptes = parse(PROFIL.as_bytes()).unwrap();
        assert_eq!(comptes.len(), 1);
        let c = &comptes[0];
        assert_eq!(c.description.as_deref(), Some("Marie & Co"));
        assert_eq!(c.config.email, "marie@example.com");
        assert_eq!(c.config.imap_host, "imap.example.com");
        assert_eq!(c.config.imap_port, 993);
        assert_eq!(c.config.imap_transport, Transport::Tls);
        assert_eq!(c.config.smtp_host, "smtp.example.com");
        assert_eq!(c.config.smtp_port, 587);
        assert_eq!(c.config.smtp_transport, Transport::StartTls);
        assert_eq!(c.password.as_deref(), Some("secret"));
        assert_eq!(c.config.imap_user, None, "no login given: the address");
        assert_eq!(c.smtp_password, None);
    }

    #[test]
    fn a_corporate_profile_keeps_its_logins_and_ports() {
        // `DOMAIN\jdoe` was replaced by the address, and SSL on the host's own port
        // was taken for STARTTLS: such accounts never connected.
        let xml = PROFIL
            .replace(
                "<key>IncomingPassword</key>",
                "<key>IncomingMailServerUsername</key><string>CORP\\jdoe</string>\
                 <key>OutgoingMailServerUsername</key><string>jdoe-smtp</string>\
                 <key>OutgoingPasswordSameAsIncomingPassword</key><false/>\
                 <key>OutgoingPassword</key><string>envoi</string>\
                 <key>IncomingPassword</key>",
            )
            .replace("<integer>993</integer>", "<integer>10993</integer>");
        let c = &parse(xml.as_bytes()).unwrap()[0];
        assert_eq!(c.config.imap_user.as_deref(), Some("CORP\\jdoe"));
        assert_eq!(c.config.smtp_user.as_deref(), Some("jdoe-smtp"));
        assert_eq!(c.smtp_password.as_deref(), Some("envoi"));
        assert_eq!(
            (c.config.imap_port, c.config.imap_transport),
            (10993, Transport::Tls)
        );
    }

    #[test]
    fn the_same_password_for_sending_is_not_kept_twice() {
        let xml = PROFIL.replace(
            "<key>IncomingPassword</key>",
            "<key>OutgoingPasswordSameAsIncomingPassword</key><true/>\
             <key>OutgoingPassword</key><string>ignored</string>\
             <key>IncomingPassword</key>",
        );
        assert_eq!(parse(xml.as_bytes()).unwrap()[0].smtp_password, None);
    }

    #[test]
    fn a_signed_profile_is_read_from_inside_its_envelope() {
        let mut signe = vec![0x30, 0x80, 0x06, 0x09, 0x2a, 0x86, 0x48, 0xff, 0x00];
        signe.extend_from_slice(PROFIL.as_bytes());
        signe.extend_from_slice(&[0x00, 0x00, 0xa0, 0x82, 0xfe]);
        let comptes = parse(&signe).unwrap();
        assert_eq!(comptes[0].config.imap_host, "imap.example.com");
    }

    /// A signed profile as signing tools write it: indefinite lengths, and the
    /// property list sent as an octet string in pieces, framing between them.
    fn enveloppe(xml: &[u8], morceau: usize) -> Vec<u8> {
        let oid_signed = [
            0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02,
        ];
        let oid_data = [
            0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01,
        ];
        let mut d = vec![0x30, 0x80];
        d.extend_from_slice(&oid_signed);
        d.extend_from_slice(&[0xa0, 0x80, 0x30, 0x80, 0x02, 0x01, 0x01, 0x31, 0x00]);
        d.extend_from_slice(&[0x30, 0x80]);
        d.extend_from_slice(&oid_data);
        d.extend_from_slice(&[0xa0, 0x80, 0x24, 0x80]);
        for piece in xml.chunks(morceau) {
            d.push(0x04);
            d.push(0x82);
            d.push((piece.len() >> 8) as u8);
            d.push(piece.len() as u8);
            d.extend_from_slice(piece);
        }
        // The octet string, [0], the content info; then the certificates would come,
        // then the signed data and the outer [0] and sequence end.
        d.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        d.extend_from_slice(&[0x31, 0x03, 0x02, 0x01, 0x00]);
        d.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        d
    }

    #[test]
    fn a_signed_profile_cut_in_pieces_is_put_back_together() {
        let signe = enveloppe(PROFIL.as_bytes(), 300);
        // The pieces' framing lands inside the XML: read raw, it would not parse.
        let comptes = parse(&signe).unwrap();
        assert_eq!(comptes[0].config.imap_host, "imap.example.com");
        assert_eq!(comptes[0].config.smtp_port, 587);
        assert_eq!(comptes[0].password.as_deref(), Some("secret"));
    }

    /// A profile of one's own, read where it lies and never copied here:
    /// `IRIS_PROFILE=<path> cargo test -p iris-discover -- --ignored a_real_profile`.
    #[test]
    #[ignore]
    fn a_real_profile_reads() {
        let chemin = std::env::var("IRIS_PROFILE").expect("IRIS_PROFILE");
        let comptes = parse(&std::fs::read(chemin).unwrap()).unwrap();
        assert!(!comptes.is_empty());
        assert!(!comptes[0].config.imap_host.is_empty());
    }

    #[test]
    fn an_empty_password_is_no_password() {
        let xml = PROFIL.replace("<string>secret</string>", "<string/>");
        assert_eq!(parse(xml.as_bytes()).unwrap()[0].password, None);
    }

    #[test]
    fn a_profile_without_an_address_leaves_it_to_be_typed() {
        let xml = PROFIL.replace(
            "<key>EmailAddress</key>\n      <string>Marie@Example.com</string>",
            "",
        );
        let c = &parse(xml.as_bytes()).unwrap()[0];
        assert_eq!(c.config.email, "");
    }

    #[test]
    fn missing_ports_take_the_usual_ones() {
        let xml = PROFIL
            .replace("<integer>993</integer>", "")
            .replace("<key>IncomingMailServerPortNumber</key>", "")
            .replace("<key>OutgoingMailServerPortNumber</key>", "")
            .replace("<integer>587</integer>", "");
        let c = &parse(xml.as_bytes()).unwrap()[0];
        assert_eq!((c.config.imap_port, c.config.smtp_port), (993, 587));
    }

    #[test]
    fn a_pop_account_is_refused_by_name() {
        let xml = PROFIL.replace("EmailTypeIMAP", "EmailTypePOP");
        let e = parse(xml.as_bytes()).unwrap_err();
        assert!(e.to_string().contains("POP"));
    }

    #[test]
    fn a_profile_without_mail_says_so() {
        let xml = PROFIL.replace("com.apple.mail.managed", "com.apple.wifi.managed");
        let e = parse(xml.as_bytes()).unwrap_err();
        assert!(e.to_string().contains("no mail account"));
    }

    #[test]
    fn other_files_are_refused() {
        assert!(parse(b"hello").is_err());
        assert!(parse(b"bplist00\x00\x01")
            .unwrap_err()
            .to_string()
            .contains("binary"));
    }
}
