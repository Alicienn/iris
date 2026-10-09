//! The cells of a spreadsheet embedded in a note (`![[Budget.sheet]]`): selected with
//! a click, moved through with the arrows, typed in, copied, cut, pasted and cleared.
//! Every change is made in the spreadsheet's own file, and the note draws it again.
//!
//! Rows and columns are counted as the note shows them; the embed's range (`#B2:D9`)
//! says where they start in the sheet.

use super::{render, sans_focus, Etat};
use iris_notes::block::BlockKind;
use iris_sheets::cell::{Addr, Range};
use iris_sheets::Workbook;
use iris_ui::{AppWindow, NoteSelect};
use slint::{ComponentHandle, SharedString};
use std::cell::RefCell;
use std::rc::Rc;

/// How many changes to embedded cells Ctrl+Z can take back.
const ANNULATIONS: usize = 50;

/// The cells selected: their block, the corner the selection started at and the active
/// cell (row, column as shown), and whether it is typed in.
#[derive(Debug, Clone, Copy)]
pub(super) struct Choix {
    pub block: usize,
    anchor: (u32, u32),
    cursor: (u32, u32),
    editing: bool,
}

/// What the note keeps for its embedded cells.
#[derive(Debug, Default)]
pub(super) struct Cellules {
    pub choix: Option<Choix>,
    /// What Ctrl+C copied: its text, its file and its cells there (for formulas moved).
    copie: Option<(String, String, Range)>,
    /// A file and its contents before a change, last first.
    annulations: Vec<(String, String)>,
}

/// The spreadsheet a block embeds: its target, when it is one.
fn cible(e: &Etat, i: usize) -> Option<String> {
    match super::contenu(e, i)? {
        (BlockKind::Embed { target }, _) if render::est_tableur(&target) => Some(target),
        _ => None,
    }
}

/// How many rows and columns the note shows of block `i`.
fn taille(e: &Etat, i: usize) -> Option<(u32, u32)> {
    let g = e.ed.vue.get(i)?.grille.as_ref()?;
    (g.lignes > 0 && g.colonnes > 0).then_some((g.lignes, g.colonnes))
}

/// The cells selected: first and last rows, first and last columns, the active cell.
pub(super) fn bornes(ch: &Choix) -> (u32, u32, u32, u32, u32, u32) {
    (
        ch.anchor.0.min(ch.cursor.0),
        ch.anchor.0.max(ch.cursor.0),
        ch.anchor.1.min(ch.cursor.1),
        ch.anchor.1.max(ch.cursor.1),
        ch.cursor.0,
        ch.cursor.1,
    )
}

/// The file of block `i`'s spreadsheet (as the space names it) and the cell its table
/// starts at.
fn fichier(e: &Etat, i: usize) -> Option<(String, Addr)> {
    let espace = e.espace()?;
    let (chemin, origine) = render::origine_inseree(&cible(e, i)?, espace.dir())?;
    Some((espace.rel(&chemin)?, origine))
}

/// A workbook read from the space, and the file's time.
fn lire(e: &Etat, rel: &str) -> Option<(Workbook, i64)> {
    let (texte, quand) = e.espace()?.read(rel).ok()?;
    let book = if texte.trim().is_empty() {
        Workbook::default()
    } else {
        Workbook::from_json(&texte).ok()?
    };
    Some((book, quand))
}

/// The workbook written back, what it was kept for Ctrl+Z, and the note's spreadsheets
/// drawn again.
fn ecrire(f: &AppWindow, e: &mut Etat, rel: &str, avant: &Workbook, book: &Workbook, quand: i64) {
    let Some(espace) = e.espace().cloned() else {
        return;
    };
    match espace.write(rel, &book.to_json(), Some(quand)) {
        Ok(iris_vault::WriteOutcome::Written(_)) => {
            e.cellules
                .annulations
                .push((rel.to_string(), avant.to_json()));
            if e.cellules.annulations.len() > ANNULATIONS {
                e.cellules.annulations.remove(0);
            }
            f.set_note_status("Saved".into());
        }
        Ok(iris_vault::WriteOutcome::Conflict { saved_as, .. }) => {
            f.set_toast(
                format!(
                    "This spreadsheet was changed elsewhere too: that version is kept as “{}”.",
                    super::stem(&saved_as)
                )
                .into(),
            );
        }
        Err(err) => f.set_status(format!("Could not save the spreadsheet: {err}").into()),
    }
    super::rendre_tableurs(f, e);
}

/// The selection given to the window (its rectangles drawn with the editor's).
pub(super) fn montrer(f: &AppWindow, e: &mut Etat) {
    donner(f, e);
    super::saisie::montrer(f, e);
}

fn donner(f: &AppWindow, e: &Etat) {
    let g = f.global::<NoteSelect>();
    match e.cellules.choix {
        Some(c) => {
            let (r1, r2) = (c.anchor.0.min(c.cursor.0), c.anchor.0.max(c.cursor.0));
            let (c1, c2) = (c.anchor.1.min(c.cursor.1), c.anchor.1.max(c.cursor.1));
            g.set_r1(r1 as i32);
            g.set_r2(r2 as i32);
            g.set_c1(c1 as i32);
            g.set_c2(c2 as i32);
            g.set_row(c.cursor.0 as i32);
            g.set_col(c.cursor.1 as i32);
            g.set_editing(c.editing);
            g.set_sheet_block(c.block as i32);
        }
        None => {
            g.set_editing(false);
            g.set_sheet_block(-1);
        }
    }
}

/// The keyboard to the cells.
fn clavier(f: &AppWindow) {
    let g = f.global::<NoteSelect>();
    g.set_cells_serial(g.get_cells_serial() + 1);
}

/// The cells let go, what was being typed kept.
pub(super) fn quitter(f: &AppWindow, e: &mut Etat) {
    let Some(c) = e.cellules.choix else {
        return;
    };
    if c.editing {
        let texte = f.global::<NoteSelect>().get_edit_text().to_string();
        poser(f, e, c.block, c.cursor, &texte);
    }
    e.cellules.choix = None;
    montrer(f, e);
}

/// A cell clicked (Shift: the selection grows to it).
pub(super) fn choisir(f: &AppWindow, e: &mut Etat, i: usize, r: u32, c: u32, etendre: bool) {
    if cible(e, i).is_none() {
        return;
    }
    // A cell being typed in is done first.
    if let Some(ch) = e.cellules.choix.filter(|ch| ch.editing) {
        let texte = f.global::<NoteSelect>().get_edit_text().to_string();
        poser(f, e, ch.block, ch.cursor, &texte);
        if let Some(ch) = &mut e.cellules.choix {
            ch.editing = false;
        }
    }
    // The line's cursor and its selection let go.
    super::fermer_popups(f, e);
    sans_focus(f, e);
    let anchor = match e.cellules.choix {
        Some(ch) if etendre && ch.block == i => ch.anchor,
        _ => (r, c),
    };
    e.cellules.choix = Some(Choix {
        block: i,
        anchor,
        cursor: (r, c),
        editing: false,
    });
    montrer(f, e);
    clavier(f);
}

/// Typing in the active cell: from what it holds (a formula as written), or from a
/// character typed over it.
pub(super) fn editer(f: &AppWindow, e: &mut Etat, tape: Option<&str>) {
    let Some(ch) = e.cellules.choix else {
        return;
    };
    let texte = match tape {
        Some(t) => t.to_string(),
        None => fichier(e, ch.block)
            .and_then(|(rel, origine)| {
                let (book, _) = lire(e, &rel)?;
                let a = Addr::new(origine.col + ch.cursor.1, origine.row + ch.cursor.0);
                Some(super::sheet::saisie_dans(
                    book.sheets.first()?,
                    a,
                    &super::sheet::locale(),
                ))
            })
            .unwrap_or_default(),
    };
    let g = f.global::<NoteSelect>();
    g.set_edit_text(texte.as_str().into());
    g.set_edit_cursor(texte.len() as i32);
    if let Some(ch) = &mut e.cellules.choix {
        ch.anchor = ch.cursor;
        ch.editing = true;
    }
    montrer(f, e);
}

/// What was typed, put in the cell at `(r, c)` of block `i`'s spreadsheet.
fn poser(f: &AppWindow, e: &mut Etat, i: usize, (r, c): (u32, u32), texte: &str) {
    let Some((rel, origine)) = fichier(e, i) else {
        return;
    };
    let Some((mut book, quand)) = lire(e, &rel) else {
        return;
    };
    let a = Addr::new(origine.col + c, origine.row + r);
    let Some(s) = book.sheets.first() else {
        return;
    };
    if s.input(a) == texte {
        return;
    }
    let avant = book.clone();
    super::sheet::poser_dans(&mut book.sheets[0], a, texte, &super::sheet::locale());
    ecrire(f, e, &rel, &avant, &book, quand);
}

/// The cell typed in is done (`comment`: the key that ended it).
pub(super) fn valider(
    f: &AppWindow,
    e: &mut Etat,
    (i, r, c): (usize, u32, u32),
    texte: &str,
    comment: &str,
) {
    let Some(ch) = e.cellules.choix else {
        return;
    };
    // A field closing late, or another cell's: only the cell typed in counts.
    if !ch.editing || ch.block != i || ch.cursor != (r, c) {
        return;
    }
    if comment != "escape" {
        poser(f, e, i, (r, c), texte);
    }
    if let Some(ch) = &mut e.cellules.choix {
        ch.editing = false;
    }
    match comment {
        "enter" => bouger(f, e, 0, 1, false),
        "shift+enter" => bouger(f, e, 0, -1, false),
        "tab" => bouger(f, e, 1, 0, false),
        "shift+tab" => bouger(f, e, -1, 0, false),
        _ => montrer(f, e),
    }
    // Clicked away: the keyboard is where the click put it.
    if comment != "left" {
        clavier(f);
    }
}

/// The active cell moved within what the note shows; `etendre`: the selection grows.
fn bouger(f: &AppWindow, e: &mut Etat, dc: i64, dr: i64, etendre: bool) {
    let Some(ch) = e.cellules.choix else {
        return;
    };
    let Some((lignes, colonnes)) = taille(e, ch.block) else {
        return;
    };
    let r = (i64::from(ch.cursor.0) + dr).clamp(0, i64::from(lignes) - 1) as u32;
    let c = (i64::from(ch.cursor.1) + dc).clamp(0, i64::from(colonnes) - 1) as u32;
    aller(f, e, (r, c), etendre);
}

fn aller(f: &AppWindow, e: &mut Etat, cellule: (u32, u32), etendre: bool) {
    if let Some(ch) = &mut e.cellules.choix {
        ch.cursor = cellule;
        if !etendre {
            ch.anchor = cellule;
        }
    }
    montrer(f, e);
}

/// The cells selected, in the file's addresses, and the file.
fn plage(e: &Etat) -> Option<(String, Range)> {
    let ch = e.cellules.choix?;
    let (rel, o) = fichier(e, ch.block)?;
    let a = Addr::new(o.col + ch.anchor.1, o.row + ch.anchor.0);
    let b = Addr::new(o.col + ch.cursor.1, o.row + ch.cursor.0);
    Some((rel, Range::new(a, b)))
}

/// Ctrl+C: the values selected on the clipboard as tab-separated text (as a spreadsheet
/// gives them); Ctrl+X takes them out of the sheet too.
fn copier(f: &AppWindow, e: &mut Etat, couper: bool) {
    let Some((rel, sel)) = plage(e) else { return };
    let Some((book, quand)) = lire(e, &rel) else {
        return;
    };
    let valeurs = book.compute();
    let l = super::sheet::locale();
    let Some(feuille) = book.sheets.first() else {
        return;
    };
    let lignes: Vec<Vec<String>> = (sel.start.row..=sel.end.row)
        .map(|r| {
            (sel.start.col..=sel.end.col)
                .map(|c| {
                    let a = Addr::new(c, r);
                    iris_sheets::format::display(&valeurs.get(&book, 0, a), &feuille.format(a), &l)
                })
                .collect()
        })
        .collect();
    let texte = iris_sheets::csv::write(&lignes, '\t');
    if let Ok(mut presse) = arboard::Clipboard::new() {
        let _ = presse.set_text(texte.clone());
    }
    e.cellules.copie = Some((texte, rel.clone(), sel));
    if couper {
        let avant = book.clone();
        let mut book = book;
        book.sheets[0].clear(sel);
        ecrire(f, e, &rel, &avant, &book, quand);
    }
}

/// Ctrl+V: what was copied here (formulas moved, from the same file), else text from
/// elsewhere, split on tabs and lines, from the active cell.
fn coller(f: &AppWindow, e: &mut Etat) {
    let Some((rel, sel)) = plage(e) else { return };
    let Some((mut book, quand)) = lire(e, &rel) else {
        return;
    };
    let texte = arboard::Clipboard::new()
        .ok()
        .and_then(|mut p| p.get_text().ok())
        .unwrap_or_default();
    let ici = sel.start;
    let avant = book.clone();
    let mut fin = ici;
    match e.cellules.copie.clone() {
        Some((copie, de, plage)) if de == rel && (copie == texte || texte.is_empty()) => {
            book.copy(0, plage, ici);
            fin = Addr::new(ici.col + plage.width() - 1, ici.row + plage.height() - 1);
        }
        _ => {
            if texte.is_empty() {
                return;
            }
            let l = super::sheet::locale();
            for (r, ligne) in iris_sheets::csv::parse(&texte, '\t').iter().enumerate() {
                for (c, v) in ligne.iter().enumerate() {
                    if let Some(a) = ici.offset(c as i64, r as i64) {
                        super::sheet::poser_dans(&mut book.sheets[0], a, v, &l);
                        fin = Addr::new(fin.col.max(a.col), fin.row.max(a.row));
                    }
                }
            }
        }
    }
    ecrire(f, e, &rel, &avant, &book, quand);
    // What was pasted selected, as far as the note shows it.
    let Some(ch) = e.cellules.choix else { return };
    let (Some((_, o)), Some((lignes, colonnes))) = (fichier(e, ch.block), taille(e, ch.block))
    else {
        return;
    };
    let r = fin.row.saturating_sub(o.row).min(lignes - 1);
    let c = fin.col.saturating_sub(o.col).min(colonnes - 1);
    if let Some(x) = &mut e.cellules.choix {
        x.anchor = (x.anchor.0.min(x.cursor.0), x.anchor.1.min(x.cursor.1));
        x.cursor = (r, c);
    }
    montrer(f, e);
}

/// Delete: the cells selected emptied.
fn effacer(f: &AppWindow, e: &mut Etat) {
    let Some((rel, sel)) = plage(e) else { return };
    let Some((mut book, quand)) = lire(e, &rel) else {
        return;
    };
    let avant = book.clone();
    book.sheets[0].clear(sel);
    ecrire(f, e, &rel, &avant, &book, quand);
}

/// Ctrl+Z: the last change made to an embedded spreadsheet taken back.
fn annuler(f: &AppWindow, e: &mut Etat) {
    let Some((rel, avant)) = e.cellules.annulations.pop() else {
        return;
    };
    let Some(espace) = e.espace().cloned() else {
        return;
    };
    let quand = espace.read(&rel).ok().map(|(_, q)| q);
    if let Err(err) = espace.write(&rel, &avant, quand) {
        f.set_status(format!("Could not save the spreadsheet: {err}").into());
    }
    super::rendre_tableurs(f, e);
}

/// A key while cells are selected. True when it was used here.
pub(super) fn touche(f: &AppWindow, e: &mut Etat, nom: &str) -> bool {
    let Some(ch) = e.cellules.choix else {
        return false;
    };
    let Some((lignes, colonnes)) = taille(e, ch.block) else {
        return false;
    };
    let (dernier_r, dernier_c) = (lignes - 1, colonnes - 1);
    // Ctrl and a letter comes as the letter typed (a capital with Shift).
    let bas = if nom.starts_with("type:") {
        nom.to_string()
    } else {
        nom.to_lowercase()
    };
    let etendre = bas.contains("shift+");
    let base = bas.trim_start_matches("ctrl+").trim_start_matches("shift+");
    let ctrl = bas.starts_with("ctrl+");
    match base {
        "up" | "down" | "left" | "right" if ctrl => {
            let (r, c) = ch.cursor;
            let cellule = match base {
                "up" => (0, c),
                "down" => (dernier_r, c),
                "left" => (r, 0),
                _ => (r, dernier_c),
            };
            aller(f, e, cellule, etendre);
        }
        // Up from the first row, Down from the last: back to the note's lines.
        "up" if !etendre && !ctrl && ch.cursor.0 == 0 && ch.block > 0 => {
            quitter(f, e);
            let fin = super::contenu(e, ch.block - 1).map_or(0, |(_, t)| t.len());
            super::focaliser(f, e, ch.block - 1, fin, fin);
            super::rendre(f, e, false);
        }
        "down"
            if !etendre
                && !ctrl
                && ch.cursor.0 == dernier_r
                && e.note
                    .as_ref()
                    .is_some_and(|n| ch.block + 1 < n.blocks.len()) =>
        {
            quitter(f, e);
            super::focaliser(f, e, ch.block + 1, 0, 0);
            super::rendre(f, e, false);
        }
        "up" => bouger(f, e, 0, -1, etendre),
        "down" => bouger(f, e, 0, 1, etendre),
        "left" => bouger(f, e, -1, 0, etendre),
        "right" => bouger(f, e, 1, 0, etendre),
        "tab" => bouger(f, e, if etendre { -1 } else { 1 }, 0, false),
        "home" => aller(f, e, if ctrl { (0, 0) } else { (ch.cursor.0, 0) }, etendre),
        "end" => aller(
            f,
            e,
            if ctrl {
                (dernier_r, dernier_c)
            } else {
                (ch.cursor.0, dernier_c)
            },
            etendre,
        ),
        // Enter and F2 type in the cell, as in Google Sheets.
        "enter" | "f2" => editer(f, e, None),
        "delete" | "backspace" => effacer(f, e),
        "escape" => quitter(f, e),
        "a" if ctrl => {
            if let Some(x) = &mut e.cellules.choix {
                x.anchor = (0, 0);
                x.cursor = (dernier_r, dernier_c);
            }
            montrer(f, e);
        }
        "c" if ctrl => copier(f, e, false),
        "x" if ctrl => copier(f, e, true),
        "v" if ctrl => coller(f, e),
        "z" if ctrl && !etendre => annuler(f, e),
        _ => {
            // A character typed: writing in the cell starts with it.
            let Some(t) = nom.strip_prefix("type:") else {
                return false;
            };
            let mut car = t.chars();
            match (car.next(), car.next()) {
                (Some(c), None) if !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c) => {
                    editer(f, e, Some(t));
                }
                _ => return false,
            }
        }
    }
    true
}

/// The window's calls for the embedded cells.
pub(super) fn wire(f: &AppWindow, etat: &Rc<RefCell<Etat>>) {
    let g = f.global::<NoteSelect>();
    macro_rules! geste {
        ($installer:ident, |$f:ident, $e:ident $(, $arg:ident)*| $corps:block) => {{
            let (etat, faible) = (Rc::clone(etat), f.as_weak());
            g.$installer(move |$($arg),*| {
                let Some($f) = faible.upgrade() else { return };
                let Ok(mut guard) = etat.try_borrow_mut() else { return };
                let $e: &mut Etat = &mut guard;
                $corps
            });
        }};
    }
    geste!(on_cell_pressed, |f, e, i, r, c, etendre| {
        choisir(
            &f,
            e,
            i.max(0) as usize,
            r.max(0) as u32,
            c.max(0) as u32,
            etendre,
        );
    });
    geste!(on_cell_double_clicked, |f, e, i, r, c| {
        choisir(
            &f,
            e,
            i.max(0) as usize,
            r.max(0) as u32,
            c.max(0) as u32,
            false,
        );
        editer(&f, e, None);
    });
    geste!(on_cell_committed, |f, e, i, r, c, texte, comment| {
        let cellule = (i.max(0) as usize, r.max(0) as u32, c.max(0) as u32);
        valider(&f, e, cellule, &texte, &comment);
    });
    {
        let (etat, faible) = (Rc::clone(etat), f.as_weak());
        g.on_cell_key(move |nom: SharedString| {
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
