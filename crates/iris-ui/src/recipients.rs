//! The address fields of the composer: each address typed becomes a chip.
//!
//! The fields keep the comma-separated text the send path reads ("Marie
//! <marie@example.com>, paul@example.com"). What is here cuts that text into entries
//! for the chips, and says when what is being typed is a whole address. Pure
//! functions, behind the `ComposeRecipients` global that [`install`] wires; a window
//! without it keeps plain text fields.
//!
//! A field is held as two parts: the entries already made into chips (`head`, in the
//! same comma-separated form), and what is being typed after them (`pending`).

use crate::{AppWindow, ComposeRecipients, RecipientChipData, RecipientEditData};
use iris_types::Address;
use slint::{ComponentHandle, ModelRc, VecModel};

/// Cuts a field at its commas and semicolons, but not inside a quoted name or angle
/// brackets (`"Dupont, Marie" <m@example.com>` is one recipient): the entries before
/// the last cut, and what follows it, being typed.
fn cut(field: &str) -> (Vec<String>, String) {
    let mut entrees = Vec::new();
    let (mut debut, mut guillemets, mut chevrons) = (0, false, false);
    for (i, c) in field.char_indices() {
        match c {
            '"' => guillemets = !guillemets,
            '<' if !guillemets => chevrons = true,
            '>' if !guillemets => chevrons = false,
            ',' | ';' if !guillemets && !chevrons => {
                let entree = field[debut..i].trim();
                if !entree.is_empty() {
                    entrees.push(entree.to_string());
                }
                debut = i + c.len_utf8();
            }
            _ => {}
        }
    }
    (entrees, field[debut..].trim_start().to_string())
}

/// Every entry of a field, what follows its last comma included.
pub fn entries(field: &str) -> Vec<String> {
    let (mut entrees, reste) = cut(field);
    let reste = reste.trim();
    if !reste.is_empty() {
        entrees.push(reste.to_string());
    }
    entrees
}

/// The field's text for these entries.
pub fn join(entries: &[String]) -> String {
    entries.join(", ")
}

/// An entry read as an address: `Marie <marie@example.com>`, or a bare address.
pub fn address(entry: &str) -> Address {
    let e = entry.trim();
    match (e.rfind('<'), e.rfind('>')) {
        (Some(ouvre), Some(ferme)) if ferme > ouvre + 1 => {
            let nom = e[..ouvre].trim().trim_matches('"').trim();
            let adresse = e[ouvre + 1..ferme].trim();
            if nom.is_empty() {
                Address::new(adresse)
            } else {
                Address::named(nom, adresse)
            }
        }
        _ => Address::new(e),
    }
}

/// Whether two entries are the same mailbox, whatever names they carry.
pub fn same(a: &str, b: &str) -> bool {
    address(a).key() == address(b).key()
}

/// An address as a field takes it: its name quoted when it holds a comma, which
/// would otherwise cut it in two.
pub fn entry_of(a: &Address) -> String {
    match &a.name {
        Some(nom) if !nom.trim().is_empty() => {
            let nom = nom.trim();
            if nom.contains([',', ';', '"']) {
                format!("\"{}\" <{}>", nom.replace('"', ""), a.addr)
            } else {
                format!("{nom} <{}>", a.addr)
            }
        }
        _ => a.addr.clone(),
    }
}

/// What is being typed is a whole address: a space after it ends it. A name being
/// typed ("Marie ") is not, until its address closes it.
fn complete(pending: &str) -> bool {
    let t = pending.trim();
    !t.is_empty()
        && address(t).looks_valid()
        && (t.ends_with('>') || !t.contains(char::is_whitespace))
}

/// The field's text: its chips, then what is being typed.
pub fn joined(head: &str, pending: &str) -> String {
    let mut texte = join(&entries(head));
    let reste = pending.trim_start();
    if !reste.trim().is_empty() {
        if !texte.is_empty() {
            texte.push_str(", ");
        }
        texte.push_str(reste);
    }
    texte
}

/// What a key typed in a field leaves: the chips, with what a comma, a semicolon or
/// a space after a whole address has just completed, and what is still being typed.
pub fn typing(head: &str, typed: &str) -> (String, String) {
    let (nouvelles, mut reste) = cut(typed);
    let mut entrees = entries(head);
    entrees.extend(nouvelles);
    if reste.ends_with(char::is_whitespace) && complete(&reste) {
        entrees.push(reste.trim().to_string());
        reste.clear();
    }
    (join(&entrees), reste)
}

/// The chips without the one at `index`.
pub fn remove(head: &str, index: usize) -> String {
    let mut entrees = entries(head);
    if index < entrees.len() {
        entrees.remove(index);
    }
    join(&entrees)
}

/// One chip: the name when there is one, else the address; an entry that is not an
/// address is said.
fn chip(entry: &str) -> RecipientChipData {
    let a = address(entry);
    let label = match &a.name {
        Some(nom) if !nom.trim().is_empty() => nom.trim().to_string(),
        _ => a.addr.trim().to_string(),
    };
    RecipientChipData {
        label: label.into(),
        address: a.addr.trim().into(),
        valid: a.looks_valid(),
    }
}

/// Wires the composer's address fields to these functions. Until then they are
/// plain text fields.
pub fn install(window: &AppWindow) {
    let g = window.global::<ComposeRecipients>();
    g.on_chips(|head| {
        ModelRc::new(VecModel::from(
            entries(&head)
                .iter()
                .map(|e| chip(e.as_str()))
                .collect::<Vec<_>>(),
        ))
    });
    g.on_committed(|value| join(&entries(&value)).into());
    g.on_joined(|head, pending| joined(&head, &pending).into());
    g.on_typing(|head, typed| {
        let (tete, reste) = typing(&head, &typed);
        RecipientEditData {
            head: tete.into(),
            pending: reste.into(),
        }
    });
    g.on_remove(|head, index| remove(&head, index.max(0) as usize).into());
    g.on_listed(|value| join(&entries(&value)).into());
    g.set_ready(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comma_makes_a_chip_of_what_was_typed() {
        assert_eq!(
            typing("", "marie@example.com,"),
            ("marie@example.com".to_string(), String::new())
        );
        assert_eq!(
            typing("marie@example.com", "pa"),
            ("marie@example.com".to_string(), "pa".to_string())
        );
        // A semicolon too, and a paste of several.
        assert_eq!(
            typing("", "a@example.com; b@example.com, c@exa"),
            (
                "a@example.com, b@example.com".to_string(),
                "c@exa".to_string()
            )
        );
    }

    #[test]
    fn a_space_ends_an_address_but_not_a_name() {
        assert_eq!(
            typing("", "paul@example.com "),
            ("paul@example.com".to_string(), String::new())
        );
        assert_eq!(typing("", "Marie "), (String::new(), "Marie ".to_string()));
        assert_eq!(
            typing("", "Marie Curie <marie@example.com> "),
            ("Marie Curie <marie@example.com>".to_string(), String::new())
        );
    }

    #[test]
    fn a_name_with_a_comma_stays_one_chip() {
        let (tete, reste) = typing("", "\"Dupont, Marie\" <marie@example.com>,");
        assert_eq!(entries(&tete), ["\"Dupont, Marie\" <marie@example.com>"]);
        assert!(reste.is_empty());
        let c = chip(&entries(&tete)[0]);
        assert_eq!(c.label.as_str(), "Dupont, Marie");
        assert_eq!(c.address.as_str(), "marie@example.com");
        assert!(c.valid);
    }

    #[test]
    fn the_field_s_text_is_its_chips_then_what_is_typed() {
        assert_eq!(
            joined("a@example.com, b@example.com", ""),
            "a@example.com, b@example.com"
        );
        assert_eq!(joined("a@example.com", "pa"), "a@example.com, pa");
        assert_eq!(joined("", "pa"), "pa");
        assert_eq!(remove("a@example.com, b@example.com", 0), "b@example.com");
        assert!(!chip("not an address").valid);
    }

    #[test]
    fn a_name_with_a_comma_is_quoted_for_the_field() {
        let a = Address::named("Dupont, Marie", "marie@example.com");
        assert_eq!(entry_of(&a), "\"Dupont, Marie\" <marie@example.com>");
        assert!(same(&entry_of(&a), "MARIE@example.com"));
    }
}
