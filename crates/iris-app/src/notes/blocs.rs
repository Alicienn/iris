//! Whole blocks selected: dragged across with the mouse, Shift and a click, Ctrl+A
//! twice, Shift and an arrow at a block's edge. A formula, a table, a theorem or a
//! picture is selected with the words around it, then copied, cut, deleted, moved,
//! nested or duplicated together, as in Notion.

use super::{appliquer, retenir, Etat};
use iris_notes::block::{self, Block};
use iris_notes::edit;
use iris_ui::{AppWindow, NoteSelect};
use slint::{ComponentHandle, SharedString};
use std::cell::RefCell;
use std::rc::Rc;

/// The blocks of the note, without the empty line the editor adds after a final line
/// break (it is no block of the text).
fn reels(e: &Etat) -> Option<&[Block]> {
    let note = e.note.as_ref()?;
    let n = note.blocks.len();
    let ajoute = note.text.ends_with('\n') && note.blocks.last().is_some_and(|b| b.text.is_empty());
    Some(&note.blocks[..if ajoute { n - 1 } else { n }])
}

/// The blocks selected, first to last.
fn bornes(e: &Etat) -> Option<(usize, usize)> {
    let (a, b) = e.blocs?;
    Some((a.min(b), a.max(b)))
}

/// The selection given to the window.
pub(super) fn montrer(f: &AppWindow, e: &Etat) {
    let g = f.global::<NoteSelect>();
    match bornes(e) {
        Some((premier, dernier)) => {
            g.set_first(premier as i32);
            g.set_last(dernier as i32);
        }
        None => {
            g.set_first(-1);
            g.set_last(-1);
        }
    }
}

/// Nothing selected whole any more.
pub(super) fn effacer(f: &AppWindow, e: &mut Etat) {
    if e.blocs.take().is_some() {
        montrer(f, e);
    }
}

/// Blocks `anchor` to `head` selected (in either order), the keyboard to them.
pub(super) fn choisir(f: &AppWindow, e: &mut Etat, anchor: usize, head: usize) {
    let Some(n) = reels(e).map(<[Block]>::len) else {
        return;
    };
    if n == 0 {
        return;
    }
    let (anchor, head) = (anchor.min(n - 1), head.min(n - 1));
    super::cellules::quitter(f, e);
    super::fermer_popups(f, e);
    super::sans_focus(f, e);
    let avant = e.blocs;
    e.blocs = Some((anchor, head));
    if avant != e.blocs {
        montrer(f, e);
    }
    let g = f.global::<NoteSelect>();
    g.set_blocks_serial(g.get_blocks_serial() + 1);
}

/// Shift and a click on block `i`: the selection grows to it from where it started, or
/// from the line being written.
pub(super) fn etendre(f: &AppWindow, e: &mut Etat, i: usize) {
    let depuis = match e.blocs {
        Some((a, _)) => a,
        None if e.focus >= 0 => e.focus as usize,
        None => i,
    };
    choisir(f, e, depuis, i);
}

/// The note's text changed by an operation on the blocks selected, the selection kept
/// on `premier`..=`dernier` (no line takes the cursor).
fn changer(f: &AppWindow, e: &mut Etat, texte: String, premier: usize, dernier: usize) {
    retenir(e, true);
    let Some(note) = &mut e.note else { return };
    note.text = texte;
    note.blocks = super::blocs_de(&note.text);
    note.dirty = true;
    super::rendre(f, e, false);
    super::planifier_ecriture(f);
    choisir(f, e, premier, dernier);
}

/// The blocks selected replaced by `texte` (nothing: taken out), the cursor after it.
fn remplacer(f: &AppWindow, e: &mut Etat, texte: &str) {
    let (Some((premier, dernier)), Some(blocs)) = (bornes(e), reels(e)) else {
        return;
    };
    let ed = edit::replace_blocks(blocs, premier, dernier, texte);
    e.blocs = None;
    montrer(f, e);
    retenir(e, true);
    appliquer(f, e, ed, true, None);
}

fn copier(e: &Etat) -> Option<String> {
    let (premier, dernier) = bornes(e)?;
    let texte = edit::blocks_text(reels(e)?, premier, dernier);
    if let Ok(mut presse) = arboard::Clipboard::new() {
        let _ = presse.set_text(texte.clone());
    }
    Some(texte)
}

/// A key while blocks are selected. True when it was used here.
pub(super) fn touche(f: &AppWindow, e: &mut Etat, nom: &str) -> bool {
    let (Some((premier, dernier)), Some(n)) = (bornes(e), reels(e).map(<[Block]>::len)) else {
        return false;
    };
    let nom = if nom.starts_with("type:") {
        nom.to_string()
    } else {
        nom.to_lowercase()
    };
    let (anchor, head) = e.blocs.unwrap_or((premier, dernier));
    match nom.as_str() {
        "escape" => effacer(f, e),
        "ctrl+c" => {
            copier(e);
        }
        "ctrl+x" => {
            if copier(e).is_some() {
                remplacer(f, e, "");
            }
        }
        "delete" | "backspace" => remplacer(f, e, ""),
        "ctrl+v" => {
            let texte = arboard::Clipboard::new()
                .ok()
                .and_then(|mut p| p.get_text().ok())
                .unwrap_or_default();
            remplacer(f, e, texte.trim_end_matches(['\n', '\r']));
        }
        "ctrl+a" => choisir(f, e, 0, n.saturating_sub(1)),
        "shift+up" => choisir(f, e, anchor, head.saturating_sub(1)),
        "shift+down" => choisir(f, e, anchor, (head + 1).min(n - 1)),
        // Up, Down, Enter: back to writing, before, after or in the selection.
        "up" | "left" => {
            effacer(f, e);
            let i = premier.saturating_sub(1);
            let fin = super::contenu(e, i).map_or(0, |(_, t)| t.len());
            super::focaliser(f, e, i, fin, fin);
            super::rendre(f, e, false);
        }
        "down" | "right" => {
            effacer(f, e);
            let total = e.note.as_ref().map_or(0, |n| n.blocks.len());
            let i = (dernier + 1).min(total.saturating_sub(1));
            super::focaliser(f, e, i, 0, 0);
            super::rendre(f, e, false);
        }
        "enter" => {
            effacer(f, e);
            let fin = super::contenu(e, dernier).map_or(0, |(_, t)| t.len());
            super::focaliser(f, e, dernier, fin, fin);
            super::rendre(f, e, false);
        }
        // Moved together, one place at a time.
        "alt+up" | "alt+down" => {
            let monte = nom == "alt+up";
            let Some(ed) = reels(e).and_then(|b| edit::move_blocks(b, premier, dernier, monte))
            else {
                return true;
            };
            let p = if monte { premier - 1 } else { premier + 1 };
            changer(f, e, ed.text, p, p + dernier - premier);
        }
        // List items nested one level deeper, or one less.
        "tab" | "shift+tab" => {
            let Some(blocs) = reels(e) else { return true };
            let note_texte = e.note.as_ref().map(|n| n.text.clone()).unwrap_or_default();
            let mut t = String::with_capacity(note_texte.len() + 16);
            for (k, b) in blocs.iter().enumerate() {
                if (premier..=dernier).contains(&k) {
                    t.push_str(&edit::indent(b.content(), nom == "shift+tab"));
                    t.push_str(b.line_ending());
                } else {
                    t.push_str(&b.text);
                }
            }
            if t != note_texte {
                changer(f, e, t, premier, dernier);
            }
        }
        // Written twice: the copy is what stays selected.
        "ctrl+d" => {
            let Some(blocs) = reels(e) else { return true };
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
        }
        "ctrl+z" => {
            effacer(f, e);
            super::annuler(f, e, false);
        }
        _ => return false,
    }
    true
}

/// A Shift and an arrow at the edge of the line being written: whole blocks selected
/// once the line's words are all taken. True when it happened.
pub(super) fn depuis_la_ligne(
    f: &AppWindow,
    e: &mut Etat,
    i: usize,
    texte: &str,
    cursor: usize,
    marque: usize,
    vers_le_haut: bool,
) -> bool {
    if vers_le_haut {
        if cursor > marque {
            return false;
        }
        choisir(f, e, i, i.saturating_sub(1));
    } else {
        if cursor < texte.len() {
            return false;
        }
        choisir(f, e, i, i + 1);
    }
    true
}

/// The window's calls for the blocks selected.
pub(super) fn wire(f: &AppWindow, etat: &Rc<RefCell<Etat>>) {
    let g = f.global::<NoteSelect>();
    {
        let (etat, faible) = (Rc::clone(etat), f.as_weak());
        g.on_drag(move |de, vers| {
            let Some(f) = faible.upgrade() else { return };
            let Ok(mut e) = etat.try_borrow_mut() else {
                return;
            };
            if de >= 0 && vers >= 0 {
                choisir(&f, &mut e, de as usize, vers as usize);
            }
        });
    }
    {
        let (etat, faible) = (Rc::clone(etat), f.as_weak());
        g.on_shift_clicked(move |i| {
            let Some(f) = faible.upgrade() else { return };
            let Ok(mut e) = etat.try_borrow_mut() else {
                return;
            };
            etendre(&f, &mut e, i.max(0) as usize);
        });
    }
    {
        let (etat, faible) = (Rc::clone(etat), f.as_weak());
        g.on_blocks_key(move |nom: SharedString| {
            let Some(f) = faible.upgrade() else {
                return false;
            };
            let Ok(mut e) = etat.try_borrow_mut() else {
                return false;
            };
            touche(&f, &mut e, &nom)
        });
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
