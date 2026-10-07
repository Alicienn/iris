//! A note as an HTML page: to keep, to send, or to print as PDF from a browser.

use crate::block::BlockKind;
use crate::inline::{self, escape_html, HtmlHooks, Palette};

/// What a page needs from outside besides the inline hooks: display maths and the
/// files a note embeds.
pub trait PageHooks: HtmlHooks {
    /// Display maths (`$$`), drawn.
    fn display_math(&self, latex: &str) -> String {
        format!("<pre class=\"math\">{}</pre>", escape_html(latex))
    }
    /// A file embedded with `![[…]]` (an image, a note).
    fn embed(&self, target: &str) -> String {
        format!("<p class=\"embed\">{}</p>", escape_html(target))
    }
}

/// The colour of a callout kind on the page.
fn teinte(kind: &str) -> &'static str {
    match kind {
        "def" | "info" | "note" | "todo" => "#2f6fde",
        "thm" | "prop" | "lem" | "cor" => "#7c3fb8",
        "example" | "sol" | "tip" | "success" => "#0b7d46",
        "exo" | "warning" | "caution" => "#b8620b",
        "important" | "danger" | "error" | "bug" => "#c4352e",
        "question" | "faq" => "#a07e00",
        "summary" | "abstract" => "#0e8a8a",
        _ => "#67645f",
    }
}

const STYLE: &str = "body{font-family:'Segoe UI Variable Text','Segoe UI',system-ui,sans-serif;\
max-width:760px;margin:40px auto;padding:0 24px;line-height:1.55;color:#1d1d1f}\
h1{font-size:2em}code,pre{font-family:'Cascadia Mono',Consolas,monospace;font-size:.92em}\
pre{background:#f4f4f6;padding:10px 12px;border-radius:6px;overflow:auto}\
blockquote{border-left:3px solid #d0d0d6;margin:0;padding:2px 14px;color:#555}\
.callout{border-left:3px solid;border-radius:6px;padding:8px 14px;margin:10px 0}\
.callout .title{font-weight:700}table{border-collapse:collapse}\
td,th{border:1px solid #d8d8de;padding:5px 9px}th{background:#f4f4f6}\
.math{text-align:center;background:none}.task{list-style:none;margin-left:-1.2em}\
img{max-width:100%}@media print{body{margin:0}}";

/// The page for a note's text, titled `title`.
pub fn note_to_html(text: &str, title: &str, palette: &Palette, hooks: &dyn PageHooks) -> String {
    let mut corps = String::new();
    let mut liste_ouverte: Option<&'static str> = None;
    let fermer_liste = |corps: &mut String, liste: &mut Option<&'static str>| {
        if let Some(t) = liste.take() {
            corps.push_str(&format!("</{t}>\n"));
        }
    };
    let ligne = |s: &str| inline::to_html(&inline::parse_inline(s), palette, hooks);
    let compteurs = crate::block::parse(text);
    let mut numeros: std::collections::HashMap<&'static str, u32> = Default::default();
    for b in &compteurs {
        let c = b.content();
        let t = c.trim_start();
        let (genre_liste, item): (Option<&'static str>, Option<String>) = match &b.kind {
            BlockKind::Bullet { indent } => (
                Some("ul"),
                Some(format!(
                    "<li style=\"margin-left:{}em\">{}</li>",
                    *indent as f32 * 1.4,
                    ligne(t.get(2..).unwrap_or(""))
                )),
            ),
            BlockKind::Numbered { indent, .. } => {
                let k = t.find(['.', ')']).map_or(0, |k| k + 1);
                (
                    Some("ol"),
                    Some(format!(
                        "<li style=\"margin-left:{}em\">{}</li>",
                        *indent as f32 * 1.4,
                        ligne(t[k..].trim_start())
                    )),
                )
            }
            BlockKind::Task { indent, done } => (
                Some("ul"),
                Some(format!(
                    "<li class=\"task\" style=\"margin-left:{}em\">{} {}</li>",
                    *indent as f32 * 1.4,
                    if *done { "☑" } else { "☐" },
                    ligne(t.get(6..).unwrap_or(""))
                )),
            ),
            _ => (None, None),
        };
        if let (Some(g), Some(item)) = (genre_liste, item) {
            if liste_ouverte != Some(g) {
                fermer_liste(&mut corps, &mut liste_ouverte);
                corps.push_str(&format!("<{g}>\n"));
                liste_ouverte = Some(g);
            }
            corps.push_str(&item);
            corps.push('\n');
            continue;
        }
        fermer_liste(&mut corps, &mut liste_ouverte);
        match &b.kind {
            BlockKind::Blank => {}
            BlockKind::Paragraph => corps.push_str(&format!("<p>{}</p>\n", ligne(c))),
            BlockKind::Heading(n) => corps.push_str(&format!(
                "<h{n}>{}</h{n}>\n",
                ligne(t[*n as usize..].trim_start())
            )),
            BlockKind::Quote => {
                let interieur: Vec<String> = c
                    .lines()
                    .map(|l| ligne(l.trim_start().trim_start_matches('>').trim_start()))
                    .collect();
                corps.push_str(&format!(
                    "<blockquote>{}</blockquote>\n",
                    interieur.join("<br>")
                ));
            }
            BlockKind::Callout { kind, title, .. } => {
                let couleur = teinte(kind);
                let etiquette = match kind.as_str() {
                    "def" => "Definition",
                    "thm" => "Theorem",
                    "prop" => "Proposition",
                    "lem" => "Lemma",
                    "cor" => "Corollary",
                    "proof" => "Proof",
                    "example" => "Example",
                    "exo" => "Exercise",
                    "sol" => "Solution",
                    "question" => "Question",
                    "important" => "Important",
                    "summary" => "Summary",
                    "tip" => "Tip",
                    "warning" => "Warning",
                    _ => "Note",
                };
                let numero = if matches!(
                    kind.as_str(),
                    "def" | "thm" | "prop" | "lem" | "cor" | "example" | "exo"
                ) {
                    let n = numeros.entry(etiquette).or_insert(0);
                    *n += 1;
                    format!(" {n}")
                } else {
                    String::new()
                };
                let interieur: Vec<String> = c
                    .lines()
                    .skip(1)
                    .map(|l| ligne(l.trim_start().trim_start_matches('>').trim_start()))
                    .collect();
                corps.push_str(&format!(
                    "<div class=\"callout\" style=\"border-color:{couleur};background:{couleur}14\">\
                     <div class=\"title\" style=\"color:{couleur}\">{etiquette}{numero}{}{}</div>{}</div>\n",
                    if title.is_empty() { "" } else { " — " },
                    ligne(title),
                    interieur.join("<br>")
                ));
            }
            BlockKind::Math => {
                let latex = c
                    .trim()
                    .trim_start_matches("$$")
                    .trim_end_matches("$$")
                    .trim();
                corps.push_str(&hooks.display_math(latex));
                corps.push('\n');
            }
            BlockKind::Code { lang } => {
                let lignes: Vec<&str> = c.lines().collect();
                let fin = lignes.len().saturating_sub(1).max(1);
                let code = lignes.get(1..fin).map(|l| l.join("\n")).unwrap_or_default();
                corps.push_str(&format!(
                    "<pre><code class=\"{}\">{}</code></pre>\n",
                    escape_html(lang),
                    escape_html(&code)
                ));
            }
            BlockKind::Rule => corps.push_str("<hr>\n"),
            BlockKind::Table => {
                corps.push_str("<table>\n");
                for (i, l) in c.lines().enumerate() {
                    if i == 1 {
                        continue;
                    }
                    let l = l.trim().trim_start_matches('|').trim_end_matches('|');
                    let balise = if i == 0 { "th" } else { "td" };
                    corps.push_str("<tr>");
                    for cellule in l.split('|') {
                        corps.push_str(&format!("<{balise}>{}</{balise}>", ligne(cellule.trim())));
                    }
                    corps.push_str("</tr>\n");
                }
                corps.push_str("</table>\n");
            }
            BlockKind::Embed { target } => {
                corps.push_str(&hooks.embed(target));
                corps.push('\n');
            }
            BlockKind::Container { title, .. } => {
                let interieur: Vec<String> = c
                    .lines()
                    .skip(1)
                    .filter(|l| !l.trim().starts_with(":::"))
                    .map(ligne)
                    .collect();
                corps.push_str(&format!(
                    "<details open><summary>{}</summary>{}</details>\n",
                    escape_html(title),
                    interieur.join("<br>")
                ));
            }
            BlockKind::Properties => {}
            BlockKind::Flashcard => {
                let (q, r) = c.split_once(" :: ").unwrap_or((c, ""));
                corps.push_str(&format!("<p><b>{}</b> — {}</p>\n", ligne(q), ligne(r)));
            }
            BlockKind::Bullet { .. } | BlockKind::Numbered { .. } | BlockKind::Task { .. } => {}
        }
    }
    fermer_liste(&mut corps, &mut liste_ouverte);
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{}</title><style>{STYLE}</style></head>\n<body>\n<h1>{}</h1>\n{corps}</body></html>\n",
        escape_html(title),
        escape_html(title)
    )
}

/// The default page hooks.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainPage;
impl HtmlHooks for PlainPage {}
impl PageHooks for PlainPage {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_becomes_a_page() {
        let t = "# Limites\n- un **point**\n- deux\n1. premier\n> [!thm] Unicité\n> La limite est unique.\n\n$$\nx^2\n$$\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let html = note_to_html(t, "Cours <1>", &Palette::default(), &PlainPage);
        assert!(html.contains("<title>Cours &lt;1&gt;</title>"));
        assert!(html.contains("<h1>Limites</h1>"));
        assert!(html.contains("<ul>\n<li style=\"margin-left:0em\">un <strong>point</strong></li>"));
        assert!(html.contains("</ul>\n<ol>"));
        assert!(html.contains("Theorem 1 — Unicité"));
        assert!(html.contains("<pre class=\"math\">x^2</pre>"));
        assert!(html.contains("<th>a</th><th>b</th>"));
        assert!(html.contains("<td>1</td><td>2</td>"));
    }
}
