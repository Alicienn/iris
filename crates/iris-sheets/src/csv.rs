//! CSV and the tab-separated text spreadsheets exchange through the clipboard.

/// Rows of fields, `"` quoting as RFC 4180 says (a field may hold the separator, a
/// line break, or `""` for a quote).
pub fn parse(text: &str, sep: char) -> Vec<Vec<String>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lignes = Vec::new();
    let mut ligne = Vec::new();
    let mut champ = String::new();
    let mut cite = false;
    let mut car = text.chars().peekable();
    while let Some(c) = car.next() {
        if cite {
            if c == '"' {
                if car.peek() == Some(&'"') {
                    champ.push('"');
                    car.next();
                } else {
                    cite = false;
                }
            } else {
                champ.push(c);
            }
        } else if c == '"' && champ.is_empty() {
            cite = true;
        } else if c == sep {
            ligne.push(std::mem::take(&mut champ));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && car.peek() == Some(&'\n') {
                car.next();
            }
            ligne.push(std::mem::take(&mut champ));
            lignes.push(std::mem::take(&mut ligne));
        } else {
            champ.push(c);
        }
    }
    if !champ.is_empty() || !ligne.is_empty() {
        ligne.push(champ);
        lignes.push(ligne);
    }
    lignes
}

/// Rows written with `sep`, fields quoted when they need it.
pub fn write(rows: &[Vec<String>], sep: char) -> String {
    let mut sortie = String::new();
    for ligne in rows {
        let champs: Vec<String> = ligne
            .iter()
            .map(|f| {
                if f.contains([sep, '"', '\n', '\r']) {
                    format!("\"{}\"", f.replace('"', "\"\""))
                } else {
                    f.clone()
                }
            })
            .collect();
        sortie.push_str(&champs.join(&sep.to_string()));
        sortie.push_str("\r\n");
    }
    sortie
}

/// The separator a CSV file uses: the one of `;`, `,` and tab found most on its first
/// line, outside quotes.
pub fn guess_separator(text: &str) -> char {
    let premiere = text.lines().next().unwrap_or("");
    let mut cite = false;
    let mut comptes = [(';', 0), (',', 0), ('\t', 0)];
    for c in premiere.chars() {
        if c == '"' {
            cite = !cite;
        } else if !cite {
            for (s, n) in &mut comptes {
                if c == *s {
                    *n += 1;
                }
            }
        }
    }
    comptes
        .iter()
        .max_by_key(|(_, n)| *n)
        .filter(|(_, n)| *n > 0)
        .map_or(',', |(s, _)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_separators_and_line_breaks() {
        let t = "nom;note\r\n\"Dupont; Jean\";12,5\n\"Il dit \"\"oui\"\"\";\"deux\nlignes\"\n";
        let r = parse(t, ';');
        assert_eq!(
            r,
            vec![
                vec!["nom".to_string(), "note".into()],
                vec!["Dupont; Jean".into(), "12,5".into()],
                vec!["Il dit \"oui\"".into(), "deux\nlignes".into()],
            ]
        );
        assert_eq!(parse(&write(&r, ';'), ';'), r);
        assert_eq!(guess_separator(t), ';');
        assert_eq!(guess_separator("a,b\tc,d"), ',');
        assert_eq!(guess_separator("a"), ',');
    }

    #[test]
    fn a_last_line_without_its_break_and_empty_fields() {
        assert_eq!(
            parse("a\t\tc\n1\t2", '\t'),
            vec![
                vec!["a".to_string(), String::new(), "c".into()],
                vec!["1".into(), "2".into()]
            ]
        );
        assert_eq!(parse("\u{feff}x", ','), vec![vec!["x".to_string()]]);
        assert!(parse("", ',').is_empty());
    }
}
