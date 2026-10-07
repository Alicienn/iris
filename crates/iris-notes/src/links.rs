//! Links between notes: found in a note, resolved to a file, rewritten when the file
//! is renamed or moved, and turned around into "mentioned in".
//!
//! A link names a note as Obsidian does: by its name alone when that is enough
//! (`[[Limites]]`), by its path when two notes share a name (`[[Analyse/Limites]]`),
//! with a heading (`[[Limites#Cauchy]]`) and a label (`[[Limites|ici]]`) if one likes.

use crate::block::{self, BlockKind};
use crate::inline::{self, Mark, Node};
use std::ops::Range;

/// A link in a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// What it names, as written: `Analyse/Limites`, `Limites`.
    pub target: String,
    pub heading: Option<String>,
    pub label: Option<String>,
    pub embed: bool,
    /// Where `[[…]]` is in the note's text.
    pub range: Range<usize>,
    /// The line it is on, for "mentioned in".
    pub line: String,
}

/// Every link of a note, code and maths left out.
pub fn links(text: &str) -> Vec<Link> {
    let mut sortie = Vec::new();
    let blocs = block::parse(text);
    let mut debut = 0;
    for b in &blocs {
        let ignorer = matches!(
            b.kind,
            BlockKind::Code { .. } | BlockKind::Math | BlockKind::Properties
        );
        if !ignorer {
            let contenu = b.content();
            let noeuds = inline::parse_inline(contenu);
            recueillir(&noeuds, contenu, debut, &mut sortie);
        }
        debut += b.text.len();
    }
    sortie
}

fn recueillir(noeuds: &[Node], contenu: &str, debut: usize, sortie: &mut Vec<Link>) {
    for n in noeuds {
        if let Node::Mark {
            mark,
            children,
            range,
        } = n
        {
            if let Mark::NoteLink { target, embed } = mark {
                // The source between the brackets, for its label.
                let ouvre = if *embed { 3 } else { 2 };
                let dedans = &contenu[range.start + ouvre..range.end - 2];
                let label = dedans.split_once('|').map(|(_, l)| l.to_string());
                let (cible, titre) = match target.split_once('#') {
                    Some((c, h)) => (c.to_string(), Some(h.to_string())),
                    None => (target.clone(), None),
                };
                let ligne_debut = contenu[..range.start].rfind('\n').map_or(0, |k| k + 1);
                let ligne_fin = contenu[range.end..]
                    .find('\n')
                    .map_or(contenu.len(), |k| range.end + k);
                sortie.push(Link {
                    target: cible,
                    heading: titre,
                    label,
                    embed: *embed,
                    range: debut + range.start..debut + range.end,
                    line: contenu[ligne_debut..ligne_fin].trim().to_string(),
                });
            } else {
                recueillir(children, contenu, debut, sortie);
            }
        }
    }
}

/// The note a link names, among the notes of the space (relative paths, `.md`
/// included): the exact path first, then the shortest path ending with it, then a
/// note of that name. `None`: no such note (yet).
pub fn resolve(target: &str, notes: &[String]) -> Option<String> {
    let t = target.trim().trim_start_matches('/');
    if t.is_empty() {
        return None;
    }
    let avec = if t.to_ascii_lowercase().ends_with(".md") {
        t.to_string()
    } else {
        format!("{t}.md")
    };
    let egal = |a: &str, b: &str| a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase();
    if let Some(n) = notes.iter().find(|n| egal(n, &avec)) {
        return Some(n.clone());
    }
    let suffixe = format!("/{}", avec.to_lowercase());
    let mut candidats: Vec<&String> = notes
        .iter()
        .filter(|n| n.to_lowercase().ends_with(&suffixe))
        .collect();
    candidats.sort_by_key(|n| n.len());
    candidats.first().map(|n| (*n).clone())
}

/// The name of a note, without its folder and `.md`.
pub fn stem(rel: &str) -> &str {
    let nom = rel.rsplit('/').next().unwrap_or(rel);
    nom.strip_suffix(".md").unwrap_or(nom)
}

/// How a note is best named in a link: its name alone when no other note has it,
/// else its path (without `.md`).
pub fn link_name(rel: &str, notes: &[String]) -> String {
    let nom = stem(rel);
    let homonymes = notes
        .iter()
        .filter(|n| stem(n).eq_ignore_ascii_case(nom))
        .count();
    if homonymes <= 1 {
        nom.to_string()
    } else {
        rel.strip_suffix(".md").unwrap_or(rel).to_string()
    }
}

/// A note's text with every link to `old` made a link to `new` (paths in the space,
/// `.md` included). `notes_before` and `notes_after` are the space's notes before and
/// after the move, to resolve the links as they were and name the note as it now is.
/// `None` when nothing changed.
pub fn rewrite_links(
    text: &str,
    old: &str,
    new: &str,
    notes_before: &[String],
    notes_after: &[String],
) -> Option<String> {
    let mut a_changer: Vec<(Range<usize>, String)> = Vec::new();
    for l in links(text) {
        if resolve(&l.target, notes_before).as_deref() != Some(old) {
            continue;
        }
        let nom = link_name(new, notes_after);
        let mut nouveau = String::new();
        nouveau.push_str(if l.embed { "![[" } else { "[[" });
        nouveau.push_str(&nom);
        if let Some(h) = &l.heading {
            nouveau.push('#');
            nouveau.push_str(h);
        }
        if let Some(lab) = &l.label {
            nouveau.push('|');
            nouveau.push_str(lab);
        }
        nouveau.push_str("]]");
        a_changer.push((l.range, nouveau));
    }
    if a_changer.is_empty() {
        return None;
    }
    let mut sortie = text.to_string();
    for (plage, remplacement) in a_changer.into_iter().rev() {
        sortie.replace_range(plage, &remplacement);
    }
    (sortie != text).then_some(sortie)
}

/// The headings of a note, with their level and their block's index.
pub fn outline(text: &str) -> Vec<(u8, String, usize)> {
    block::parse(text)
        .iter()
        .enumerate()
        .filter_map(|(i, b)| match b.kind {
            BlockKind::Heading(n) => {
                let t = b.content().trim_start()[n as usize..].trim();
                Some((n, inline::plain_text(&inline::parse_inline(t)), i))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn links_are_found_with_their_parts() {
        let t = "Voir [[Limites#Cauchy|ici]] et ![[schema.png]].\n```\n[[pas un lien]]\n```\n";
        let l = links(t);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].target, "Limites");
        assert_eq!(l[0].heading.as_deref(), Some("Cauchy"));
        assert_eq!(l[0].label.as_deref(), Some("ici"));
        assert_eq!(&t[l[0].range.clone()], "[[Limites#Cauchy|ici]]");
        assert!(l[1].embed);
        assert_eq!(&t[l[1].range.clone()], "![[schema.png]]");
        assert!(l[0].line.starts_with("Voir"));
    }

    #[test]
    fn names_resolve_to_files() {
        let n = notes(&[
            "Analyse/Limites.md",
            "Algèbre/Limites.md",
            "Accueil.md",
            "A/B/Suites.md",
        ]);
        assert_eq!(resolve("Accueil", &n).as_deref(), Some("Accueil.md"));
        assert_eq!(resolve("accueil", &n).as_deref(), Some("Accueil.md"));
        assert_eq!(
            resolve("Analyse/Limites", &n).as_deref(),
            Some("Analyse/Limites.md")
        );
        assert_eq!(resolve("Suites", &n).as_deref(), Some("A/B/Suites.md"));
        assert_eq!(resolve("B/Suites", &n).as_deref(), Some("A/B/Suites.md"));
        assert_eq!(resolve("Nouvelle", &n), None);
        assert_eq!(link_name("Accueil.md", &n), "Accueil");
        assert_eq!(link_name("Analyse/Limites.md", &n), "Analyse/Limites");
    }

    #[test]
    fn a_rename_rewrites_only_the_links_to_that_note() {
        let avant = notes(&["Analyse/Limites.md", "Algèbre/Limites.md", "Cours.md"]);
        let apres = notes(&[
            "Analyse/Limites et suites.md",
            "Algèbre/Limites.md",
            "Cours.md",
        ]);
        let texte = "[[Analyse/Limites#Def|déf]] puis [[Algèbre/Limites]]; le mot Limites reste.";
        let r = rewrite_links(
            texte,
            "Analyse/Limites.md",
            "Analyse/Limites et suites.md",
            &avant,
            &apres,
        )
        .unwrap();
        assert_eq!(
            r,
            "[[Limites et suites#Def|déf]] puis [[Algèbre/Limites]]; le mot Limites reste."
        );
        assert!(rewrite_links("rien", "Cours.md", "X.md", &avant, &apres).is_none());
    }

    #[test]
    fn the_outline_lists_headings() {
        let o = outline("# Un\ntexte\n## Deux **gras**\n");
        assert_eq!(o, vec![(1, "Un".into(), 0), (2, "Deux gras".into(), 2)]);
    }
}
