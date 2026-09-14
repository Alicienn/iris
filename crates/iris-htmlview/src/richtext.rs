//! Le moteur de texte riche.
//!
//! Transforme le HTML assaini en une suite de blocs simples que l'interface dessine
//! elle-même. Il ne cherche pas à reproduire une mise en page : il cherche à rendre
//! **le texte lisible**, ce qui suffit pour un message écrit par un être humain.
//!
//! Une propriété compte plus que la fidélité : il ne peut pas échouer. Quelle que
//! soit la horreur reçue, il rend quelque chose de lisible, sans exécuter quoi que ce
//! soit et sans appeler le réseau.

use crate::{HtmlRenderer, Rendered};
use iris_types::Result;

/// Un fragment de texte, avec son habillage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub code: bool,
    /// Cible du lien, si le fragment en est un.
    pub link: Option<String>,
}

impl Inline {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
            italic: false,
            underline: false,
            code: false,
            link: None,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// Un bloc de contenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading {
        level: u8,
        spans: Vec<Inline>,
    },
    ListItem {
        depth: u8,
        ordered: bool,
        spans: Vec<Inline>,
    },
    /// Texte cité, avec sa profondeur d'imbrication.
    Quote {
        depth: u8,
        spans: Vec<Inline>,
    },
    Code(String),
    Rule,
    /// Une image, décrite mais non chargée.
    Image {
        alt: String,
        blocked: bool,
    },
    /// Une ligne de tableau, aplatie en cellules.
    TableRow(Vec<Vec<Inline>>),
}

impl Block {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Paragraph(s) | Self::Heading { spans: s, .. } => s.iter().all(Inline::is_blank),
            Self::ListItem { spans, .. } | Self::Quote { spans, .. } => {
                spans.iter().all(Inline::is_blank)
            }
            Self::Code(t) => t.trim().is_empty(),
            Self::TableRow(cells) => cells.is_empty(),
            Self::Rule | Self::Image { .. } => false,
        }
    }

    /// Texte brut du bloc, pour la recherche et l'accessibilité.
    pub fn text(&self) -> String {
        match self {
            Self::Paragraph(s) | Self::Heading { spans: s, .. } => join(s),
            Self::ListItem { spans, .. } | Self::Quote { spans, .. } => join(spans),
            Self::Code(t) => t.clone(),
            Self::Image { alt, .. } => alt.clone(),
            Self::TableRow(cells) => cells.iter().map(|c| join(c)).collect::<Vec<_>>().join("\t"),
            Self::Rule => String::new(),
        }
    }
}

fn join(spans: &[Inline]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect::<String>()
}

/// Un document rendu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RichText {
    pub blocks: Vec<Block>,
    /// Nombre d'images distantes non chargées.
    pub blocked_images: usize,
}

impl RichText {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .map(Block::text)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Profondeur de citation maximale.
    ///
    /// Sert à replier automatiquement les citations : dans un fil de dix réponses,
    /// afficher les neuf précédentes noie la seule ligne utile.
    pub fn max_quote_depth(&self) -> u8 {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Quote { depth, .. } => Some(*depth),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    }
}

/// Le moteur.
#[derive(Debug, Default, Clone, Copy)]
pub struct RichTextRenderer;

impl HtmlRenderer for RichTextRenderer {
    fn render(&self, sanitized_html: &str, _width: f32) -> Result<Rendered> {
        Ok(Rendered::Blocks(parse(sanitized_html)))
    }

    fn name(&self) -> &'static str {
        "texte riche"
    }

    fn is_full_fidelity(&self) -> bool {
        false
    }
}

/// État de l'habillage courant pendant le parcours.
#[derive(Debug, Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    underline: bool,
    code: bool,
}

/// Transforme un fragment HTML en blocs.
pub fn parse(html: &str) -> RichText {
    let mut doc = RichText::default();
    let mut spans: Vec<Inline> = Vec::new();
    let mut style = Style::default();
    let mut lien: Option<String> = None;
    let mut quote_depth = 0u8;
    let mut list_depth = 0u8;
    let mut ordered = false;
    let mut dans_code = false;
    let mut cellules: Vec<Vec<Inline>> = Vec::new();
    let mut dans_tableau = 0usize;

    let mut position = 0usize;
    let octets = html.as_bytes();

    // Vide les fragments accumulés dans un bloc du type courant.
    macro_rules! chasser {
        ($doc:expr, $spans:expr) => {
            if !$spans.iter().all(Inline::is_blank) {
                let bloc = if quote_depth > 0 {
                    Block::Quote {
                        depth: quote_depth,
                        spans: std::mem::take(&mut $spans),
                    }
                } else if list_depth > 0 {
                    Block::ListItem {
                        depth: list_depth,
                        ordered,
                        spans: std::mem::take(&mut $spans),
                    }
                } else if dans_code {
                    Block::Code(join(&std::mem::take(&mut $spans)))
                } else {
                    Block::Paragraph(std::mem::take(&mut $spans))
                };
                $doc.blocks.push(bloc);
            } else {
                $spans.clear();
            }
        };
    }

    while position < octets.len() {
        if octets[position] == b'<' {
            let Some(fin) = html[position..].find('>') else {
                break;
            };
            let balise = &html[position..position + fin + 1];
            let nom = tag_name(balise);
            let fermante = balise.starts_with("</");

            match nom.as_str() {
                "b" | "strong" => style.bold = !fermante,
                "i" | "em" => style.italic = !fermante,
                "u" => style.underline = !fermante,
                "code" | "tt" | "kbd" => style.code = !fermante,
                "a" => {
                    if fermante {
                        lien = None;
                    } else {
                        lien = attribute(balise, "href");
                    }
                }
                "br" => spans.push(Inline::plain("\n")),
                "p" | "div" => chasser!(doc, spans),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    chasser!(doc, spans);
                    if fermante {
                        // Le contenu a déjà été chassé comme paragraphe : on le
                        // requalifie en titre.
                        if let Some(Block::Paragraph(s)) = doc.blocks.pop() {
                            let level = nom.as_bytes()[1] - b'0';
                            doc.blocks.push(Block::Heading { level, spans: s });
                        }
                    }
                }
                "blockquote" => {
                    chasser!(doc, spans);
                    quote_depth = if fermante {
                        quote_depth.saturating_sub(1)
                    } else {
                        quote_depth.saturating_add(1)
                    };
                }
                "ul" | "ol" => {
                    chasser!(doc, spans);
                    if fermante {
                        list_depth = list_depth.saturating_sub(1);
                    } else {
                        list_depth = list_depth.saturating_add(1);
                        ordered = nom == "ol";
                    }
                }
                "li" => chasser!(doc, spans),
                "pre" => {
                    chasser!(doc, spans);
                    dans_code = !fermante;
                }
                "hr" => {
                    chasser!(doc, spans);
                    doc.blocks.push(Block::Rule);
                }
                "img" => {
                    let source = attribute(balise, "src").unwrap_or_default();
                    // L'assainissement a remplacé la source des images distantes par
                    // un marqueur inerte : c'est lui qui nous dit qu'elle est bloquée.
                    let bloquee = source.starts_with("iris:blocked");
                    if bloquee {
                        doc.blocked_images += 1;
                    }
                    let alt = attribute(balise, "alt").unwrap_or_else(|| "image".into());
                    doc.blocks.push(Block::Image {
                        alt,
                        blocked: bloquee,
                    });
                }
                "table" => {
                    chasser!(doc, spans);
                    if fermante {
                        dans_tableau = dans_tableau.saturating_sub(1);
                    } else {
                        dans_tableau += 1;
                    }
                }
                "tr" if fermante && !cellules.is_empty() => {
                    doc.blocks
                        .push(Block::TableRow(std::mem::take(&mut cellules)));
                }
                "td" | "th" if fermante => {
                    cellules.push(std::mem::take(&mut spans));
                }
                _ => {}
            }

            position += fin + 1;
            continue;
        }

        // Texte jusqu'à la prochaine balise.
        let fin = html[position..]
            .find('<')
            .map(|i| position + i)
            .unwrap_or(html.len());
        let texte = decode_entities(&html[position..fin]);
        if !texte.is_empty() {
            let normalise = if dans_code {
                texte
            } else {
                collapse_spaces(&texte)
            };
            if !normalise.is_empty() {
                spans.push(Inline {
                    text: normalise,
                    bold: style.bold,
                    italic: style.italic,
                    underline: style.underline,
                    code: style.code || dans_code,
                    link: lien.clone(),
                });
            }
        }
        position = fin;
    }

    chasser!(doc, spans);
    if !cellules.is_empty() {
        doc.blocks.push(Block::TableRow(cellules));
    }

    doc.blocks.retain(|b| !b.is_empty());
    doc
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('<')
        .trim_start_matches('/')
        .trim_end_matches('>')
        .split([' ', '\t', '\n', '/', '>'])
        .next()
        .unwrap_or("")
        .to_lowercase()
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let minuscules = tag.to_lowercase();
    let motif = format!("{name}=");
    let debut = minuscules.find(&motif)? + motif.len();
    let reste = tag.get(debut..)?;

    let valeur = if let Some(r) = reste.strip_prefix('"') {
        r.split('"').next()?
    } else if let Some(r) = reste.strip_prefix('\'') {
        r.split('\'').next()?
    } else {
        reste.split_whitespace().next()?.trim_end_matches('>')
    };
    Some(valeur.to_string())
}

/// Replie les blancs, comme le fait un moteur HTML.
fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut precedent_blanc = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !precedent_blanc {
                out.push(' ');
            }
            precedent_blanc = true;
        } else {
            out.push(c);
            precedent_blanc = false;
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&nbsp;", "\u{a0}")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&eacute;", "é")
        .replace("&egrave;", "è")
        .replace("&agrave;", "à")
        .replace("&ccedil;", "ç")
        .replace("&hellip;", "…")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&laquo;", "«")
        .replace("&raquo;", "»")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_paragraphe_simple() {
        let doc = parse("<p>Bonjour Marie</p>");
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.text(), "Bonjour Marie");
    }

    #[test]
    fn les_paragraphes_sont_separes() {
        let doc = parse("<p>Premier</p><p>Second</p>");
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.text(), "Premier\nSecond");
    }

    #[test]
    fn l_habillage_est_conserve() {
        let doc = parse("<p>Texte <b>gras</b> et <i>italique</i></p>");
        let Block::Paragraph(spans) = &doc.blocks[0] else {
            panic!("attendu un paragraphe")
        };

        assert!(spans.iter().any(|s| s.bold && s.text.contains("gras")));
        assert!(spans
            .iter()
            .any(|s| s.italic && s.text.contains("italique")));
        assert!(spans.iter().any(|s| !s.bold && !s.italic));
    }

    #[test]
    fn les_liens_portent_leur_cible() {
        let doc = parse(r#"<p>Voir <a href="https://exemple.fr">le devis</a></p>"#);
        let Block::Paragraph(spans) = &doc.blocks[0] else {
            panic!()
        };
        let lien = spans.iter().find(|s| s.link.is_some()).expect("un lien");
        assert_eq!(lien.link.as_deref(), Some("https://exemple.fr"));
        assert_eq!(lien.text, "le devis");
    }

    #[test]
    fn les_titres_sont_reconnus_avec_leur_niveau() {
        let doc = parse("<h2>Titre</h2><p>Corps</p>");
        match &doc.blocks[0] {
            Block::Heading { level, spans } => {
                assert_eq!(*level, 2);
                assert_eq!(join(spans), "Titre");
            }
            autre => panic!("attendu un titre, obtenu {autre:?}"),
        }
    }

    #[test]
    fn les_listes_portent_leur_profondeur() {
        let doc = parse("<ul><li>Un</li><li>Deux</li></ul>");
        let items: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ListItem {
                    depth,
                    ordered,
                    spans,
                } => Some((*depth, *ordered, join(spans))),
                _ => None,
            })
            .collect();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0], (1, false, "Un".to_string()));
    }

    #[test]
    fn une_liste_numerotee_est_distinguee() {
        let doc = parse("<ol><li>Premier</li></ol>");
        assert!(matches!(
            doc.blocks[0],
            Block::ListItem { ordered: true, .. }
        ));
    }

    #[test]
    fn les_citations_portent_leur_profondeur() {
        // Indispensable pour replier automatiquement : dans un fil de dix réponses,
        // afficher les neuf précédentes noie la seule ligne utile.
        let doc = parse(
            "<p>Ma réponse</p><blockquote><p>Sa question</p>\
             <blockquote><p>Ma question initiale</p></blockquote></blockquote>",
        );
        assert_eq!(doc.max_quote_depth(), 2);

        let profondeurs: Vec<u8> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Quote { depth, .. } => Some(*depth),
                _ => None,
            })
            .collect();
        assert_eq!(profondeurs, [1, 2]);
    }

    #[test]
    fn un_message_sans_citation_a_une_profondeur_nulle() {
        assert_eq!(parse("<p>Bonjour</p>").max_quote_depth(), 0);
    }

    #[test]
    fn une_image_bloquee_est_signalee() {
        let doc = parse(r#"<img src="iris:blocked" alt="Bannière">"#);
        match &doc.blocks[0] {
            Block::Image { alt, blocked } => {
                assert_eq!(alt, "Bannière");
                assert!(blocked);
            }
            autre => panic!("attendu une image, obtenu {autre:?}"),
        }
        assert_eq!(doc.blocked_images, 1);
    }

    #[test]
    fn une_image_embarquee_n_est_pas_bloquee() {
        let doc = parse(r#"<img src="cid:logo" alt="Logo">"#);
        assert!(matches!(doc.blocks[0], Block::Image { blocked: false, .. }));
        assert_eq!(doc.blocked_images, 0);
    }

    #[test]
    fn une_image_sans_texte_alternatif_reste_annoncee() {
        let doc = parse(r#"<img src="cid:x">"#);
        assert_eq!(doc.blocks[0].text(), "image");
    }

    #[test]
    fn les_tableaux_sont_aplatis_en_lignes() {
        let doc = parse("<table><tr><td>Janvier</td><td>1200 €</td></tr></table>");
        match &doc.blocks[0] {
            Block::TableRow(cellules) => {
                assert_eq!(cellules.len(), 2);
                assert_eq!(join(&cellules[0]), "Janvier");
                assert_eq!(join(&cellules[1]), "1200 €");
            }
            autre => panic!("attendu une ligne de tableau, obtenu {autre:?}"),
        }
    }

    #[test]
    fn le_texte_prefome_conserve_ses_espaces() {
        let doc = parse("<pre>fn main() {\n    println!();\n}</pre>");
        match &doc.blocks[0] {
            Block::Code(t) => assert!(t.contains("    println")),
            autre => panic!("attendu du code, obtenu {autre:?}"),
        }
    }

    #[test]
    fn les_blancs_sont_replies_comme_par_un_moteur_html() {
        let doc = parse("<p>Trop     d'espaces\n\n\net de lignes</p>");
        assert_eq!(doc.text(), "Trop d'espaces et de lignes");
    }

    #[test]
    fn les_entites_sont_decodees() {
        let doc = parse("<p>Marie &amp; Cie &laquo; devis &raquo; &hellip;</p>");
        assert_eq!(doc.text(), "Marie & Cie « devis » …");
    }

    #[test]
    fn les_blocs_vides_sont_ecartes() {
        let doc = parse("<p></p><div>   </div><p>Contenu</p><p></p>");
        assert_eq!(doc.blocks.len(), 1);
    }

    #[test]
    fn un_corps_vide_produit_un_document_vide() {
        assert!(parse("").is_empty());
        assert!(parse("   ").is_empty());
        assert!(parse("<p></p>").is_empty());
    }

    #[test]
    fn le_moteur_ne_peut_pas_echouer() {
        // Quelle que soit l'horreur reçue, il rend quelque chose.
        let horreurs = [
            "<<<>>>",
            "<p>non fermé",
            "<b><i><u>imbrication sans fin",
            "</p></div></b>",
            "<img src=",
            "&&&;;;",
            "<table><tr><td>",
        ];
        for h in horreurs {
            let r = RichTextRenderer.render(h, 800.0);
            assert!(r.is_ok(), "échec sur « {h} »");
        }
    }

    #[test]
    fn un_document_tres_imbrique_ne_fait_pas_deborder_la_pile() {
        let profond = "<div>".repeat(5_000) + "texte" + &"</div>".repeat(5_000);
        let doc = parse(&profond);
        assert!(doc.text().contains("texte"));
    }

    #[test]
    fn le_texte_brut_est_extractible() {
        let doc = parse("<h1>Titre</h1><p>Un <b>paragraphe</b></p><ul><li>Point</li></ul>");
        let texte = doc.text();
        assert!(texte.contains("Titre"));
        assert!(texte.contains("Un paragraphe"));
        assert!(texte.contains("Point"));
    }

    #[test]
    fn le_moteur_se_declare_approximatif() {
        // L'interface s'en sert pour proposer un repli quand un message s'affiche mal.
        assert!(!RichTextRenderer.is_full_fidelity());
        assert_eq!(RichTextRenderer.name(), "texte riche");
    }

    #[test]
    fn lecture_des_attributs() {
        assert_eq!(attribute(r#"<a href="x">"#, "href").as_deref(), Some("x"));
        assert_eq!(attribute("<a href='x'>", "href").as_deref(), Some("x"));
        assert_eq!(attribute("<a href=x>", "href").as_deref(), Some("x"));
        assert_eq!(attribute("<a>", "href"), None);
    }

    #[test]
    fn lecture_du_nom_de_balise() {
        assert_eq!(tag_name("<p>"), "p");
        assert_eq!(tag_name("</p>"), "p");
        assert_eq!(tag_name(r#"<a href="x">"#), "a");
        assert_eq!(tag_name("<BR/>"), "br");
    }
}
