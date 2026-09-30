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
}

/// The mail accounts of a profile, in its order.
pub fn parse(bytes: &[u8]) -> Result<Vec<ProfileAccount>> {
    if bytes.starts_with(b"bplist") {
        return Err(Error::Config(
            "this profile is in binary form: export it as XML and try again".into(),
        ));
    }
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
    // Never in the clear. The profile's "use SSL" is read from the port: on 993 and
    // 465 the connection is encrypted from the start, on any other it is upgraded
    // with STARTTLS — and one that cannot be upgraded fails rather than sends a
    // password openly.
    let transport = |port: u16| {
        if port == 993 || port == 465 {
            Transport::Tls
        } else {
            Transport::StartTls
        }
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

    Some(ProfileAccount {
        description: description.clone(),
        config: ServerConfig {
            provider: description,
            email,
            imap_host,
            imap_port,
            imap_transport: transport(imap_port),
            smtp_host,
            smtp_port,
            smtp_transport: transport(smtp_port),
            auth: Auth::Password,
            note: None,
        },
        password,
    })
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
    /// Booleans, data, dates: nothing Iris needs.
    Other,
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
            "true" | "false" => Value::Other,
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
    }

    #[test]
    fn a_signed_profile_is_read_from_inside_its_envelope() {
        let mut signe = vec![0x30, 0x80, 0x06, 0x09, 0x2a, 0x86, 0x48, 0xff, 0x00];
        signe.extend_from_slice(PROFIL.as_bytes());
        signe.extend_from_slice(&[0x00, 0x00, 0xa0, 0x82, 0xfe]);
        let comptes = parse(&signe).unwrap();
        assert_eq!(comptes[0].config.imap_host, "imap.example.com");
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
