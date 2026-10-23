//! Plain-text bodies, laid out for the renderers.
//!
//! A message written as plain text marks what it quotes with `>` at the start of the
//! line, one per level (`>>`, or `> >`, quotes a quotation). Shown as they are, those
//! markers are noise down the left of every reply. Here they become quotations — the
//! markers taken off, each level a `<blockquote>` — which the rich-text renderer draws
//! indented, with a bar per level. Only what is shown changes: the message stored, the
//! text quoted in a reply and what is forwarded keep their markers.

/// How deep a quotation is drawn. A thread quoted twenty times over would otherwise
/// push its oldest words off the right of the column.
const MAX_DEPTH: usize = 8;

/// How deeply a line of a plain-text message is quoted, and its words without the
/// markers: `"> > Hello"` is `(2, "Hello")`.
///
/// Markers start the line; spaces may separate them (`> >`). The one space after the
/// last marker belongs to it. A `>` further into the line is just a character.
pub fn quote_level(line: &str) -> (usize, &str) {
    let mut reste = line;
    let mut niveau = 0;
    loop {
        // Spaces only between markers: a line that starts with spaces is not quoted.
        let essai = if niveau > 0 {
            reste.trim_start_matches([' ', '\t'])
        } else {
            reste
        };
        match essai.strip_prefix('>') {
            Some(apres) => {
                niveau += 1;
                reste = apres;
            }
            None => break,
        }
    }
    if niveau > 0 {
        reste = reste.strip_prefix(' ').unwrap_or(reste);
    }
    (niveau, reste)
}

/// Lays out a plain-text body as HTML for the renderers, to be shown, never sent.
///
/// Plain text is not HTML: fed in raw, every line break would collapse and anything
/// between angle brackets — in mail, usually an address — would vanish. Each line is
/// kept, paragraphs split at blank lines, and the lines quoted with `>` become nested
/// `<blockquote>`s, their markers removed.
pub fn plain_text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    // Spaces kept as written, for the full engine; the rich-text renderer reads the
    // paragraphs and line breaks.
    out.push_str("<div style=\"white-space:pre-wrap\">");

    let mut ouverts = 0usize;
    let mut lignes: Vec<&str> = Vec::new();
    for ligne in text.lines() {
        let (niveau, mots) = quote_level(ligne);
        let niveau = niveau.min(MAX_DEPTH);
        if niveau != ouverts {
            paragraphe(&mut out, &mut lignes);
            while ouverts > niveau {
                out.push_str("</blockquote>");
                ouverts -= 1;
            }
            while ouverts < niveau {
                out.push_str("<blockquote type=\"cite\">");
                ouverts += 1;
            }
        }
        if mots.trim().is_empty() {
            paragraphe(&mut out, &mut lignes);
        } else {
            lignes.push(mots);
        }
    }
    paragraphe(&mut out, &mut lignes);
    for _ in 0..ouverts {
        out.push_str("</blockquote>");
    }

    out.push_str("</div>");
    out
}

/// Writes the lines gathered as one paragraph, a line break between them.
fn paragraphe(out: &mut String, lignes: &mut Vec<&str>) {
    if lignes.is_empty() {
        return;
    }
    out.push_str("<p>");
    for (i, ligne) in lignes.iter().enumerate() {
        if i > 0 {
            out.push_str("<br>");
        }
        for c in ligne.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                c => out.push(c),
            }
        }
    }
    out.push_str("</p>");
    lignes.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::richtext::{parse, Block};

    /// The blocks the rich-text renderer makes of a plain-text body: (quote depth,
    /// text), 0 for a paragraph.
    fn blocs(texte: &str) -> Vec<(u8, String)> {
        parse(&plain_text_to_html(texte))
            .blocks
            .iter()
            .map(|b| match b {
                Block::Quote { depth, .. } => (*depth, b.text()),
                Block::Paragraph(_) => (0, b.text()),
                autre => panic!("unexpected block {autre:?}"),
            })
            .collect()
    }

    #[test]
    fn markers_are_counted_and_taken_off() {
        assert_eq!(quote_level("> Hello"), (1, "Hello"));
        assert_eq!(quote_level(">Hello"), (1, "Hello"));
        assert_eq!(quote_level(">> Hello"), (2, "Hello"));
        assert_eq!(quote_level("> > Hello"), (2, "Hello"));
        assert_eq!(quote_level(">>> > Hello"), (4, "Hello"));
        assert_eq!(quote_level(">"), (1, ""));
        assert_eq!(quote_level("> "), (1, ""));
        // Only the one space after the last marker is the marker's.
        assert_eq!(quote_level(">   indented"), (1, "  indented"));
    }

    #[test]
    fn a_marker_elsewhere_than_the_start_is_a_character() {
        assert_eq!(quote_level("Hello"), (0, "Hello"));
        assert_eq!(quote_level("a > b"), (0, "a > b"));
        assert_eq!(quote_level(" > not a quote"), (0, " > not a quote"));
        assert_eq!(quote_level("-> arrow"), (0, "-> arrow"));
        assert_eq!(quote_level(""), (0, ""));
    }

    #[test]
    fn quoted_lines_become_quotations_by_depth() {
        let texte = "Thanks, see below.\n\
                     \n\
                     On Monday, Marie wrote:\n\
                     > Can we meet?\n\
                     > On Friday?\n\
                     >> Paul wrote:\n\
                     > > Next week works.\n\
                     >\n\
                     > Let me know.\n\
                     \n\
                     Paul";
        assert_eq!(
            blocs(texte),
            [
                (0, "Thanks, see below.".to_string()),
                (0, "On Monday, Marie wrote:".to_string()),
                (1, "Can we meet?\nOn Friday?".to_string()),
                (2, "Paul wrote:\nNext week works.".to_string()),
                (1, "Let me know.".to_string()),
                (0, "Paul".to_string()),
            ]
        );
    }

    #[test]
    fn no_marker_is_left_in_the_text() {
        let texte = "> one\n>> two\n> > three\n>>>four";
        for (_, t) in blocs(texte) {
            assert!(!t.contains('>'), "{t:?}");
        }
    }

    #[test]
    fn lines_and_paragraphs_are_kept() {
        assert_eq!(
            blocs("First line\nsecond line\n\n\nNext paragraph"),
            [
                (0, "First line\nsecond line".to_string()),
                (0, "Next paragraph".to_string()),
            ]
        );
    }

    #[test]
    fn windows_line_ends_are_lines_too() {
        assert_eq!(
            blocs("Hello\r\n> quoted\r\nBye\r\n"),
            [
                (0, "Hello".to_string()),
                (1, "quoted".to_string()),
                (0, "Bye".to_string()),
            ]
        );
    }

    #[test]
    fn angle_brackets_and_ampersands_survive() {
        assert_eq!(
            blocs("Write to <marie@example.com> & co\n> a < b > c"),
            [
                (0, "Write to <marie@example.com> & co".to_string()),
                (1, "a < b > c".to_string()),
            ]
        );
    }

    #[test]
    fn every_quotation_opened_is_closed() {
        let html = plain_text_to_html("a\n>>> deep\n> less");
        assert_eq!(
            html.matches("<blockquote").count(),
            html.matches("</blockquote>").count()
        );
        let html = plain_text_to_html(">> ends quoted");
        assert_eq!(html.matches("<blockquote").count(), 2);
        assert_eq!(html.matches("</blockquote>").count(), 2);
    }

    #[test]
    fn a_very_deep_quotation_is_drawn_at_the_deepest_level_shown() {
        let texte = format!("{} old", ">".repeat(30));
        assert_eq!(blocs(&texte), [(MAX_DEPTH as u8, "old".to_string())]);
    }

    #[test]
    fn an_empty_body_gives_nothing_to_show() {
        assert!(blocs("").is_empty());
        assert!(blocs("\n\n>\n").is_empty());
    }

    #[test]
    fn a_plain_text_body_stays_with_the_rich_text_renderer() {
        // Quotations add no inline style: a long thread written as text is not taken
        // for a laid-out newsletter.
        let texte = "> a\n>> b\n> c\n".repeat(50);
        let mesure = crate::measure_complexity(&plain_text_to_html(&texte));
        assert_eq!(mesure.inline_styles, 1);
        assert_eq!(mesure.tables, 0);
    }
}
