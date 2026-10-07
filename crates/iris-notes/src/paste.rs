//! What the clipboard brings, made Markdown: a web page's or a document's HTML (bold,
//! italic, links, headings, lists, quotes, code, tables), and a web address dropped on
//! words.

/// Whether `text` is one web address and nothing else.
pub fn is_url(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("https://") || t.starts_with("http://"))
        && t.len() > 10
        && !t.chars().any(char::is_whitespace)
}

/// Whether HTML carries formatting worth keeping (else its plain text does as well).
pub fn is_rich(html: &str) -> bool {
    let h = html.to_ascii_lowercase();
    [
        "<b>",
        "<b ",
        "<strong",
        "<i>",
        "<i ",
        "<em",
        "<u>",
        "<h1",
        "<h2",
        "<h3",
        "<h4",
        "<li",
        "<a ",
        "<table",
        "<code",
        "<pre",
        "<blockquote",
        "<s>",
        "<del",
        "<strike",
    ]
    .iter()
    .any(|t| h.contains(t))
}

/// The part of the clipboard's HTML that was copied (between its fragment markers).
fn fragment(html: &str) -> &str {
    let debut = html
        .find("<!--StartFragment-->")
        .map(|i| i + "<!--StartFragment-->".len())
        .unwrap_or(0);
    let fin = html[debut..]
        .find("<!--EndFragment-->")
        .map(|i| debut + i)
        .unwrap_or(html.len());
    &html[debut..fin]
}

fn entite(nom: &str) -> Option<char> {
    Some(match nom {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "rsquo" => '’',
        "lsquo" => '‘',
        "rdquo" => '”',
        "ldquo" => '“',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "eacute" => 'é',
        "egrave" => 'è',
        "agrave" => 'à',
        "ccedil" => 'ç',
        _ => {
            let n = nom.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

/// Text with its entities read and its runs of spaces made one.
fn texte(s: &str) -> String {
    let mut sortie = String::new();
    let mut reste = s;
    while let Some(i) = reste.find('&') {
        sortie.push_str(&reste[..i]);
        let apres = &reste[i + 1..];
        match apres.find(';').filter(|&f| f <= 10) {
            Some(f) => match entite(&apres[..f]) {
                Some(c) => {
                    sortie.push(c);
                    reste = &apres[f + 1..];
                }
                None => {
                    sortie.push('&');
                    reste = apres;
                }
            },
            None => {
                sortie.push('&');
                reste = apres;
            }
        }
    }
    sortie.push_str(reste);
    let mut compact = String::with_capacity(sortie.len());
    let mut blanc = false;
    for c in sortie.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            if !blanc {
                compact.push(' ');
            }
            blanc = true;
        } else {
            compact.push(c);
            blanc = false;
        }
    }
    compact
}

/// A tag: its name in lower case, whether it closes, and its `href`.
struct Balise {
    nom: String,
    ferme: bool,
    href: Option<String>,
}

fn lire_balise(b: &str) -> Balise {
    let b = b.trim();
    let ferme = b.starts_with('/');
    let corps = b.trim_start_matches('/');
    let nom: String = corps
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let href = corps.to_ascii_lowercase().find("href=").and_then(|i| {
        let v = &corps[i + 5..];
        let (q, v) = match v.chars().next()? {
            '"' => ('"', &v[1..]),
            '\'' => ('\'', &v[1..]),
            _ => (' ', v),
        };
        let fin = v.find([q, '>']).unwrap_or(v.len());
        Some(texte(&v[..fin]))
    });
    Balise { nom, ferme, href }
}

/// HTML made Markdown, Iris's marks for underline and strikethrough.
pub fn html_to_markdown(html: &str) -> String {
    let html = fragment(html);
    let mut lignes: Vec<String> = Vec::new();
    let mut ligne = String::new();
    // Lists open, ordered or not, and how far each has counted.
    let mut listes: Vec<Option<u32>> = Vec::new();
    let mut citation = 0usize;
    let mut pre = false;
    let mut liens: Vec<Option<String>> = Vec::new();
    let mut ignore = 0usize;
    // A table: its rows of cells as they come.
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cellule: Option<String> = None;

    let finir_ligne = |ligne: &mut String, lignes: &mut Vec<String>, citation: usize| {
        let t = ligne.trim_end().to_string();
        if !t.trim().is_empty() {
            lignes.push(format!("{}{t}", "> ".repeat(citation)));
        }
        ligne.clear();
    };
    let blanc = |lignes: &mut Vec<String>| {
        if lignes.last().is_some_and(|l| !l.is_empty()) {
            lignes.push(String::new());
        }
    };

    let mut reste = html;
    while !reste.is_empty() {
        let Some(i) = reste.find('<') else {
            let t = if pre { reste.to_string() } else { texte(reste) };
            match &mut cellule {
                Some(c) => c.push_str(&t),
                None if ignore == 0 => ligne.push_str(&t),
                None => {}
            }
            break;
        };
        if i > 0 && ignore == 0 {
            let brut = &reste[..i];
            let t = if pre {
                brut.replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&")
            } else {
                texte(brut)
            };
            match &mut cellule {
                Some(c) => c.push_str(&t),
                None => {
                    if pre {
                        for (k, morceau) in t.split('\n').enumerate() {
                            if k > 0 {
                                lignes.push(std::mem::take(&mut ligne));
                            }
                            ligne.push_str(morceau);
                        }
                    } else if !(ligne.is_empty() && t.trim().is_empty()) {
                        ligne.push_str(if ligne.is_empty() { t.trim_start() } else { &t });
                    }
                }
            }
        }
        let apres = &reste[i + 1..];
        // A comment.
        if let Some(c) = apres.strip_prefix("!--") {
            reste = c.find("-->").map_or("", |f| &c[f + 3..]);
            continue;
        }
        let Some(f) = apres.find('>') else { break };
        let b = lire_balise(&apres[..f]);
        reste = &apres[f + 1..];
        if matches!(b.nom.as_str(), "style" | "script" | "head" | "title") {
            if b.ferme {
                ignore = ignore.saturating_sub(1);
            } else {
                ignore += 1;
            }
            continue;
        }
        if ignore > 0 {
            continue;
        }
        let marque = |nom: &str| -> Option<&'static str> {
            Some(match nom {
                "b" | "strong" => "**",
                "i" | "em" => "*",
                "u" => "__",
                "s" | "del" | "strike" => "--",
                "code" if !pre => "`",
                _ => return None,
            })
        };
        let dans_cellule = cellule.is_some();
        let cible: &mut String = match &mut cellule {
            Some(c) => c,
            None => &mut ligne,
        };
        if let Some(m) = marque(&b.nom) {
            cible.push_str(m);
            continue;
        }
        match (b.nom.as_str(), b.ferme) {
            ("a", false) => {
                liens.push(b.href.filter(|h| h.starts_with("http")));
                if liens.last().is_some_and(Option::is_some) {
                    cible.push('[');
                }
            }
            ("a", true) => {
                if let Some(Some(h)) = liens.pop() {
                    cible.push_str(&format!("]({h})"));
                }
            }
            ("br", _) => {
                if dans_cellule {
                    cible.push(' ');
                } else {
                    finir_ligne(&mut ligne, &mut lignes, citation);
                }
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                blanc(&mut lignes);
                let n = b.nom[1..].parse::<usize>().unwrap_or(1);
                ligne.push_str(&format!("{} ", "#".repeat(n)));
            }
            ("p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6", _) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                if b.ferme && listes.is_empty() {
                    blanc(&mut lignes);
                }
            }
            ("ul", false) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                listes.push(None);
            }
            ("ol", false) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                listes.push(Some(0));
            }
            ("ul" | "ol", true) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                listes.pop();
                if listes.is_empty() {
                    blanc(&mut lignes);
                }
            }
            ("li", false) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                let retrait = "  ".repeat(listes.len().saturating_sub(1));
                let puce = match listes.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{n}. ")
                    }
                    _ => "- ".to_string(),
                };
                ligne.push_str(&format!("{retrait}{puce}"));
            }
            ("li", true) => finir_ligne(&mut ligne, &mut lignes, citation),
            ("blockquote", false) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                citation += 1;
            }
            ("blockquote", true) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                citation = citation.saturating_sub(1);
                blanc(&mut lignes);
            }
            ("pre", false) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                lignes.push("```".into());
                pre = true;
            }
            ("pre", true) => {
                if !ligne.is_empty() {
                    lignes.push(std::mem::take(&mut ligne));
                }
                lignes.push("```".into());
                pre = false;
                blanc(&mut lignes);
            }
            ("hr", _) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                lignes.push("---".into());
            }
            ("table", false) => {
                finir_ligne(&mut ligne, &mut lignes, citation);
                table = Some(Vec::new());
            }
            ("tr", false) => {
                if let Some(t) = &mut table {
                    t.push(Vec::new());
                }
            }
            ("td" | "th", false) => cellule = Some(String::new()),
            ("td" | "th", true) => {
                if let (Some(c), Some(t)) = (cellule.take(), &mut table) {
                    if let Some(rang) = t.last_mut() {
                        rang.push(c.trim().replace('|', "\\|"));
                    }
                }
            }
            ("table", true) => {
                if let Some(t) = table.take() {
                    let t: Vec<Vec<String>> = t.into_iter().filter(|r| !r.is_empty()).collect();
                    let n = t.iter().map(Vec::len).max().unwrap_or(0);
                    if n > 0 {
                        blanc(&mut lignes);
                        for (k, mut rang) in t.into_iter().enumerate() {
                            rang.resize(n, String::new());
                            lignes.push(format!("| {} |", rang.join(" | ")));
                            if k == 0 {
                                lignes.push(format!("|{}", " --- |".repeat(n)));
                            }
                        }
                        blanc(&mut lignes);
                    }
                }
            }
            _ => {}
        }
    }
    finir_ligne(&mut ligne, &mut lignes, citation);
    while lignes.last().is_some_and(String::is_empty) {
        lignes.pop();
    }
    while lignes.first().is_some_and(String::is_empty) {
        lignes.remove(0);
    }
    lignes.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_addresses_alone() {
        assert!(is_url("https://example.com/a?b=1"));
        assert!(is_url("  http://example.com  "));
        assert!(!is_url("see https://example.com"));
        assert!(!is_url("example.com"));
    }

    #[test]
    fn marks_links_and_headings() {
        let h = "<html><head><style>p{}</style></head><body><!--StartFragment-->\
                 <h2>Limites</h2><p>Une <b>suite</b> <i>converge</i> si <u>elle</u> \
                 <s>diverge</s> pas &amp; <code>u_n</code>. Voir \
                 <a href=\"https://example.com/x\">le cours</a>.</p><!--EndFragment--></body></html>";
        assert!(is_rich(h));
        assert_eq!(
            html_to_markdown(h),
            "## Limites\n\nUne **suite** *converge* si __elle__ --diverge-- pas & `u_n`. Voir [le cours](https://example.com/x)."
        );
    }

    #[test]
    fn lists_quotes_code_and_tables() {
        let h = "<ul><li>un</li><li>deux<ol><li>a</li><li>b</li></ol></li></ul>\
                 <blockquote><p>cité</p></blockquote>\
                 <pre>let x = 1;\nlet y = x &lt; 2;</pre>\
                 <table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2 | 3</td></tr></table>";
        assert_eq!(
            html_to_markdown(h),
            "- un\n- deux\n  1. a\n  2. b\n\n> cité\n\n```\nlet x = 1;\nlet y = x < 2;\n```\n\n| A | B |\n| --- | --- |\n| 1 | 2 \\| 3 |"
        );
    }

    #[test]
    fn plain_html_is_not_rich_and_entities_are_read() {
        assert!(!is_rich("<span>texte</span>"));
        assert_eq!(
            html_to_markdown("<p>a&nbsp;&lt;b&gt; &#233;t&#xE9;</p>"),
            "a <b> été"
        );
    }
}
