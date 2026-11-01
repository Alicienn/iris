//! What a key does to every block a selection runs across: moved together (Alt and an
//! arrow), nested (Tab), written twice (Ctrl+D). The selection itself — copied, cut,
//! deleted, typed over — is the editor's (`saisie`), a span of the note's text.

use super::{appliquer, retenir, Etat};
use iris_notes::block::{self, Block, BlockKind};
use iris_notes::edit::{self, NoteEdit};
use iris_ui::AppWindow;

/// The blocks of the note, without the empty line the editor adds after a final line
/// break (it is no block of the text).
fn reels(e: &Etat) -> Option<&[Block]> {
    let note = e.note.as_ref()?;
    let n = note.blocks.len();
    let ajoute = note.text.ends_with('\n') && note.blocks.last().is_some_and(|b| b.text.is_empty());
    Some(&note.blocks[..if ajoute { n - 1 } else { n }])
}

/// The note made `texte`, the selection then running over blocks `premier` to
/// `dernier`, whole.
fn changer(f: &AppWindow, e: &mut Etat, texte: String, premier: usize, dernier: usize) {
    retenir(e, true);
    let blocs = super::blocs_de(&texte);
    let dernier = dernier.min(blocs.len().saturating_sub(1));
    let fin_contenu = blocs.get(dernier).map_or(0, |b| b.content().len());
    let ed = NoteEdit {
        text: texte,
        block: dernier,
        cursor: fin_contenu,
    };
    appliquer(f, e, ed, true, None);
    let Some(note) = &e.note else { return };
    let a = block::start_of(&note.blocks, premier);
    let b = block::start_of(&note.blocks, dernier) + fin_contenu;
    super::saisie::placer(f, e, a, b);
}

/// A mark (`bold`…) or a colour (`colour:2`, `colour:none`) on the words a selection
/// runs over, block by block: a mark never crosses a line break, so each line's part
/// gets its own. The line breaks, a list's sign, a heading's `#` are left out, and a
/// part empty there (the line under a line selected to its end) is no part.
pub(super) fn marquer(f: &AppWindow, e: &mut Etat, action: &str) -> bool {
    let (a, b) = (e.ed.ancre.min(e.ed.tete), e.ed.ancre.max(e.ed.tete));
    let Some(note) = &e.note else { return false };
    if a >= b {
        return false;
    }
    let mut texte = String::with_capacity(note.text.len() + 16);
    let mut choix: Option<(usize, usize)> = None;
    let mut dernier = 0;
    let mut fin_locale = 0;
    for (k, bl) in note.blocks.iter().enumerate() {
        let debut = texte.len();
        let s = block::start_of(&note.blocks, k);
        let contenu = bl.content();
        let n = contenu.len();
        let ecrit = !matches!(
            bl.kind,
            BlockKind::Code { .. }
                | BlockKind::Math
                | BlockKind::Table
                | BlockKind::Properties
                | BlockKind::Embed { .. }
                | BlockKind::Rule
        );
        let m = super::render::prefixe(&bl.kind, contenu);
        let de = a.saturating_sub(s).max(m).min(n);
        let jusqu = b.saturating_sub(s).min(n);
        if !ecrit || s + n < a || s > b || de >= jusqu {
            texte.push_str(&bl.text);
            continue;
        }
        let r = match action.strip_prefix("colour:") {
            Some(reste) => {
                let teinte = reste
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| iris_notes::inline::Colour::ALL.get(i).copied());
                edit::set_colour(contenu, de, jusqu, teinte)
            }
            None => edit::toggle_mark(contenu, de, jusqu, action),
        };
        let (ra, rb) = (r.anchor.min(r.cursor), r.anchor.max(r.cursor));
        choix = Some((choix.map_or(debut + ra, |(x, _)| x), debut + rb));
        dernier = k;
        fin_locale = rb;
        texte.push_str(&r.text);
        texte.push_str(bl.line_ending());
    }
    let Some((na, nb)) = choix else { return false };
    retenir(e, true);
    appliquer(
        f,
        e,
        NoteEdit {
            text: texte,
            block: dernier,
            cursor: fin_locale,
        },
        true,
        None,
    );
    super::saisie::placer(f, e, na, nb);
    true
}

/// A key while a selection runs over blocks `premier` to `dernier`. True when used.
pub(super) fn sur_blocs(
    f: &AppWindow,
    e: &mut Etat,
    nom: &str,
    premier: usize,
    dernier: usize,
) -> bool {
    let Some(blocs) = reels(e) else { return false };
    if blocs.is_empty() {
        return false;
    }
    let dernier = dernier.min(blocs.len() - 1);
    let premier = premier.min(dernier);
    match nom {
        // Moved together, one place at a time.
        "alt+up" | "alt+down" => {
            let monte = nom == "alt+up";
            let Some(ed) = edit::move_blocks(blocs, premier, dernier, monte) else {
                return true;
            };
            let p = if monte { premier - 1 } else { premier + 1 };
            changer(f, e, ed.text, p, p + dernier - premier);
            true
        }
        // List items nested one level deeper, or one less.
        "tab" | "shift+tab" => {
            let texte = e.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
            let mut t = String::with_capacity(texte.len() + 16);
            for (k, b) in blocs.iter().enumerate() {
                if (premier..=dernier).contains(&k) {
                    t.push_str(&edit::indent(b.content(), nom == "shift+tab"));
                    t.push_str(b.line_ending());
                } else {
                    t.push_str(&b.text);
                }
            }
            if t != texte {
                changer(f, e, t, premier, dernier);
            }
            true
        }
        // Written twice: the copy is what stays selected.
        "ctrl+d" => {
            let copie = edit::blocks_text(blocs, premier, dernier);
            let le = edit::line_ending_of(&block::join(blocs));
            let debut = block::start_of(blocs, dernier + 1);
            let texte = e.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
            let (avant, apres) = texte.split_at(debut.min(texte.len()));
            // After a last block with no line ending, one goes between.
            let t = if avant.ends_with('\n') || avant.is_empty() {
                format!("{avant}{copie}{le}{apres}")
            } else {
                format!("{avant}{le}{copie}{apres}")
            };
            let p = dernier + 1;
            changer(f, e, t, p, p + dernier - premier);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use iris_notes::block;
    use iris_notes::edit;

    #[test]
    fn a_formula_and_a_theorem_are_copied_with_the_words_around() {
        let texte = "Avant\n$$\nx^2\n$$\n> [!thm] T\n> dit\nAprès\n";
        let blocs = block::parse(texte);
        assert_eq!(
            edit::blocks_text(&blocs, 0, 2),
            "Avant\n$$\nx^2\n$$\n> [!thm] T\n> dit"
        );
    }
}
