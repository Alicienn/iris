//! A spreadsheet open in Notes: the grid drawn from what shows, typing in cells, the
//! keys and the mouse, formats, rows and columns, the clipboard, undo, and the file
//! written half a second after the last change.
//!
//! The workbook is held here whole, as `iris_sheets` reads it; every change recomputes
//! its values and draws again the part of the grid in view.

use super::{stem, Etat};
use iris_sheets::cell::{Addr, Range, MAX_COLS, MAX_ROWS};
use iris_sheets::file::{COL_WIDTH, ROW_HEIGHT};
use iris_sheets::format::{display, Align, Locale, NumberFormat};
use iris_sheets::formula::Change;
use iris_sheets::{Format, Value, Values, Workbook};
use iris_ui::{AppWindow, SheetCellData, SheetGrid, SheetHeaderData};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// How many steps of undo a spreadsheet keeps.
const ANNULATIONS: usize = 100;
/// Drawn beyond what shows, so a small scroll draws nothing new.
const MARGE_COLS: u32 = 3;
const MARGE_LIGNES: u32 = 20;

/// The spreadsheet open.
pub(super) struct Feuille {
    pub rel: String,
    pub book: Workbook,
    values: Values,
    sheet: usize,
    anchor: Addr,
    cursor: Addr,
    /// Being typed in the active cell.
    editing: Option<String>,
    /// What shows: the offset in the content and the size.
    vue: (f32, f32, f32, f32),
    /// What was drawn: columns and rows, first to last.
    dessine: Option<(u32, u32, u32, u32)>,
    /// How far the grid goes: columns and rows.
    etendue: (u32, u32),
    undo: Vec<(Workbook, usize)>,
    redo: Vec<(Workbook, usize)>,
    modified: Option<i64>,
    dirty: bool,
    /// What Ctrl+C copied: its text on the clipboard, its sheet, its cells.
    copie: Option<(String, usize, Range)>,
    locale: Locale,
}

/// The locale of the user's region.
pub(super) fn locale() -> Locale {
    let (decimal, thousands, day_first) = crate::platform::number_style();
    let mut l = if decimal == ',' {
        Locale::french()
    } else {
        Locale::english()
    };
    l.decimal = decimal;
    l.thousands = if thousands == ' ' {
        '\u{202f}'
    } else {
        thousands
    };
    l.day_first = day_first;
    l
}

// --- Geometry --------------------------------------------------------------------------

impl Feuille {
    fn feuille(&self) -> &iris_sheets::Sheet {
        &self.book.sheets[self.sheet]
    }

    /// Where column `c` starts, in pixels.
    fn x(&self, c: u32) -> f32 {
        let changees: f32 = self
            .feuille()
            .col_widths
            .range(..c)
            .map(|(_, w)| w - COL_WIDTH)
            .sum();
        c as f32 * COL_WIDTH + changees
    }

    fn y(&self, r: u32) -> f32 {
        let changees: f32 = self
            .feuille()
            .row_heights
            .range(..r)
            .map(|(_, h)| h - ROW_HEIGHT)
            .sum();
        r as f32 * ROW_HEIGHT + changees
    }

    /// The column at `x` pixels.
    fn col_at(&self, x: f32) -> u32 {
        let (mut lo, mut hi) = (0u32, MAX_COLS - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.x(mid) <= x {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    fn row_at(&self, y: f32) -> u32 {
        let (mut lo, mut hi) = (0u32, MAX_ROWS - 1);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.y(mid) <= y {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    fn selection(&self) -> Range {
        Range::new(self.anchor, self.cursor)
    }

    fn recompute(&mut self) {
        self.values = self.book.compute();
    }

    /// The grid goes a little past what is used and what shows.
    fn agrandir(&mut self) {
        let (cols, rows) = self.feuille().extent();
        let (vx, vy, vw, vh) = self.vue;
        let voit_c = self.col_at(vx + vw) + 1;
        let voit_r = self.row_at(vy + vh) + 1;
        self.etendue = (
            (cols + 8)
                .max(26)
                .max(voit_c + 4)
                .max(self.cursor.col + 4)
                .min(MAX_COLS),
            (rows + 30)
                .max(100)
                .max(voit_r + 30)
                .max(self.cursor.row + 30)
                .min(MAX_ROWS),
        );
    }

    /// Before a change: what to undo to.
    fn retenir(&mut self) {
        self.undo.push((self.book.clone(), self.sheet));
        if self.undo.len() > ANNULATIONS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
}

fn couleur(hex: &str) -> Option<slint::Color> {
    let h = hex.trim_start_matches('#');
    let n = u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6)?;
    Some(slint::Color::from_rgb_u8(
        (n >> 16) as u8,
        (n >> 8) as u8,
        n as u8,
    ))
}

const TEXTES: [&str; 7] = [
    "#d93025", "#e8710a", "#b88400", "#188038", "#1a73e8", "#8e24aa", "#5f6368",
];
const FONDS: [&str; 7] = [
    "#fce8e6", "#feefe3", "#fef7e0", "#e6f4ea", "#e8f0fe", "#f3e8fd", "#eceef0",
];

// --- Drawing ---------------------------------------------------------------------------

fn cellule(
    fe: &Feuille,
    a: Addr,
    c: &iris_sheets::Cell,
    encre: slint::Color,
) -> Option<SheetCellData> {
    let v = fe.values.get(&fe.book, fe.sheet, a);
    let texte = display(&v, &c.format, &fe.locale);
    if texte.is_empty() && c.format.fill.is_none() && !c.format.border {
        return None;
    }
    let fond = c.format.fill.as_deref().and_then(couleur);
    let ecrit = match (c.format.color.as_deref().and_then(couleur), fond) {
        (Some(x), _) => x,
        // On a light fill, dark ink whatever the theme.
        (None, Some(_)) => slint::Color::from_rgb_u8(0x1d, 0x1d, 0x1f),
        (None, None) if matches!(v, Value::Error(_)) => slint::Color::from_rgb_u8(0xd9, 0x30, 0x25),
        (None, None) => encre,
    };
    let (x, y) = (fe.x(a.col), fe.y(a.row));
    Some(SheetCellData {
        x,
        y,
        width: fe.x(a.col + 1) - x,
        height: fe.y(a.row + 1) - y,
        text: texte.into(),
        color: ecrit,
        fill: fond.unwrap_or_default(),
        has_fill: fond.is_some(),
        bold: c.format.bold,
        italic: c.format.italic,
        underline: c.format.underline,
        strike: c.format.strike,
        wrap: c.format.wrap,
        border: c.format.border,
        align: match iris_sheets::format::align_of(&v, &c.format) {
            Align::Center => 1,
            Align::Right => 2,
            _ => 0,
        },
    })
}

/// The grid drawn again around what shows (`tout`: even if it was drawn there).
pub(super) fn dessiner(f: &AppWindow, fe: &mut Feuille, tout: bool) {
    let g = f.global::<SheetGrid>();
    fe.agrandir();
    let (vx, vy, vw, vh) = fe.vue;
    let c0 = fe.col_at(vx).saturating_sub(MARGE_COLS);
    let c1 = (fe.col_at(vx + vw.max(100.0)) + MARGE_COLS).min(fe.etendue.0 - 1);
    let r0 = fe.row_at(vy).saturating_sub(MARGE_LIGNES);
    let r1 = (fe.row_at(vy + vh.max(100.0)) + MARGE_LIGNES).min(fe.etendue.1 - 1);
    let deja = fe
        .dessine
        .is_some_and(|(a, b, c, d)| a <= c0 && b >= c1 && c <= r0 && d >= r1);
    let sel = fe.selection();
    let gelees = fe.feuille().frozen_rows;
    if tout || !deja {
        let (c0, c1) = (
            c0.saturating_sub(MARGE_COLS),
            (c1 + MARGE_COLS).min(fe.etendue.0 - 1),
        );
        let (r0, r1) = (
            r0.saturating_sub(MARGE_LIGNES),
            (r1 + MARGE_LIGNES).min(fe.etendue.1 - 1),
        );
        fe.dessine = Some((c0, c1, r0, r1));
        let encre = f.global::<iris_ui::Tokens>().get_text();
        let s = fe.feuille();
        let mut cellules = Vec::new();
        for r in r0..=r1 {
            for (a, c) in s.cells.range(Addr::new(c0, r)..=Addr::new(c1, r)) {
                if let Some(d) = cellule(fe, *a, c, encre) {
                    cellules.push(d);
                }
            }
        }
        let mut figees = Vec::new();
        for r in 0..gelees {
            for (a, c) in s.cells.range(Addr::new(c0, r)..=Addr::new(c1, r)) {
                if let Some(d) = cellule(fe, *a, c, encre) {
                    figees.push(d);
                }
            }
        }
        g.set_cells(ModelRc::new(VecModel::from(cellules)));
        g.set_pinned(ModelRc::new(VecModel::from(figees)));
    }
    // The headers: always, the selection shows in them.
    let (c0, c1, r0, r1) = fe.dessine.unwrap_or((c0, c1, r0, r1));
    let colonnes: Vec<SheetHeaderData> = (c0..=c1)
        .map(|c| SheetHeaderData {
            index: c as i32,
            pos: fe.x(c),
            size: fe.x(c + 1) - fe.x(c),
            label: iris_sheets::col_name(c).into(),
            selected: (sel.start.col..=sel.end.col).contains(&c),
        })
        .collect();
    let mut lignes: Vec<SheetHeaderData> = (0..gelees.min(r0))
        .chain(r0..=r1)
        .map(|r| SheetHeaderData {
            index: r as i32,
            pos: fe.y(r),
            size: fe.y(r + 1) - fe.y(r),
            label: (r + 1).to_string().into(),
            selected: (sel.start.row..=sel.end.row).contains(&r),
        })
        .collect();
    lignes.sort_by_key(|l| l.index);
    g.set_columns(ModelRc::new(VecModel::from(colonnes)));
    g.set_rows(ModelRc::new(VecModel::from(lignes)));
    g.set_frozen_rows(gelees as i32);
    g.set_frozen_height(if gelees > 0 { fe.y(gelees) } else { 0.0 });
    g.set_frozen(gelees > 0);
    g.set_content_width(fe.x(fe.etendue.0));
    g.set_content_height(fe.y(fe.etendue.1));
}

/// The selection, the active cell, the formula bar, the toolbar and the status line.
pub(super) fn montrer_selection(f: &AppWindow, fe: &mut Feuille) {
    let g = f.global::<SheetGrid>();
    let sel = fe.selection();
    let (sx, sy) = (fe.x(sel.start.col), fe.y(sel.start.row));
    g.set_sel_x(sx);
    g.set_sel_y(sy);
    g.set_sel_width(fe.x(sel.end.col + 1) - sx);
    g.set_sel_height(fe.y(sel.end.row + 1) - sy);
    let (cx, cy) = (fe.x(fe.cursor.col), fe.y(fe.cursor.row));
    g.set_cur_x(cx);
    g.set_cur_y(cy);
    g.set_cur_width(fe.x(fe.cursor.col + 1) - cx);
    g.set_cur_height(fe.y(fe.cursor.row + 1) - cy);
    g.set_address(
        if sel.start == sel.end {
            fe.cursor.name()
        } else {
            format!("{}  ({}×{})", sel.name(), sel.height(), sel.width())
        }
        .into(),
    );
    if fe.editing.is_none() {
        g.set_input(saisie_de(fe, fe.cursor).into());
        g.set_input_serial(g.get_input_serial() + 1);
    }
    let fmt = fe.feuille().format(fe.cursor);
    g.set_bold(fmt.bold);
    g.set_italic(fmt.italic);
    g.set_underline(fmt.underline);
    g.set_strike(fmt.strike);
    g.set_wrap(fmt.wrap);
    g.set_border(fmt.border);
    g.set_align(match fmt.align {
        Align::Auto => 0,
        Align::Left => 1,
        Align::Center => 2,
        Align::Right => 3,
    });
    g.set_number(
        match fmt.number {
            NumberFormat::General => "123",
            NumberFormat::Number => "1.00",
            NumberFormat::Percent => "%",
            NumberFormat::Currency => "€",
            NumberFormat::Date => "Date",
            NumberFormat::Time => "Time",
            NumberFormat::DateTime => "Date & time",
            NumberFormat::Text => "Text",
        }
        .into(),
    );
    // What the selection adds up to.
    let mut nombres = Vec::new();
    let mut remplies = 0usize;
    if (2..200_000).contains(&sel.len()) {
        let s = fe.feuille();
        for (a, _) in s
            .cells
            .range(sel.start..=sel.end)
            .filter(|(a, _)| sel.contains(**a))
        {
            match fe.values.get(&fe.book, fe.sheet, *a) {
                Value::Number(n) => {
                    nombres.push(n);
                    remplies += 1;
                }
                Value::Empty => {}
                _ => remplies += 1,
            }
        }
    }
    let ecrire = |n: f64| display(&Value::Number(n), &Format::default(), &fe.locale);
    g.set_status(
        if nombres.len() >= 2 {
            let somme: f64 = nombres.iter().sum();
            format!(
                "Sum {}   Average {}   Count {}",
                ecrire(somme),
                ecrire(somme / nombres.len() as f64),
                remplies
            )
        } else if remplies >= 2 {
            format!("Count {remplies}")
        } else {
            String::new()
        }
        .into(),
    );
    dessiner(f, fe, false);
}

/// What a cell shows in the formula bar and when typed in: its formula, or its value
/// written as typed (a date as a date).
fn saisie_de(fe: &Feuille, a: Addr) -> String {
    saisie_dans(fe.feuille(), a, &fe.locale)
}

/// [`saisie_de`] for any sheet: also what a spreadsheet embedded in a note types in.
pub(super) fn saisie_dans(s: &iris_sheets::Sheet, a: Addr, locale: &Locale) -> String {
    let brut = s.input(a);
    if brut.starts_with('=') {
        return brut.to_string();
    }
    let fmt = s.format(a);
    match (fmt.number, s.value_of_literal(a)) {
        (
            NumberFormat::Date | NumberFormat::Time | NumberFormat::DateTime,
            v @ Value::Number(_),
        ) => display(&v, &fmt, locale),
        (NumberFormat::Percent, Value::Number(n)) => {
            format!("{}%", iris_sheets::eval::general(n * 100.0))
                .replace('.', &locale.decimal.to_string())
        }
        (_, Value::Number(n)) => {
            iris_sheets::eval::general(n).replace('.', &locale.decimal.to_string())
        }
        _ => brut.to_string(),
    }
}

/// The active cell kept in view.
fn suivre_curseur(f: &AppWindow, fe: &mut Feuille) {
    let (vx, vy, vw, vh) = fe.vue;
    let gele = fe.y(fe.feuille().frozen_rows);
    let (x0, x1) = (fe.x(fe.cursor.col), fe.x(fe.cursor.col + 1));
    let (y0, y1) = (fe.y(fe.cursor.row), fe.y(fe.cursor.row + 1));
    let mut nx = vx;
    let mut ny = vy;
    if x0 < vx {
        nx = x0;
    } else if x1 > vx + vw {
        nx = (x1 - vw).max(0.0);
    }
    if fe.cursor.row >= fe.feuille().frozen_rows {
        if y0 < vy + gele {
            ny = (y0 - gele).max(0.0);
        } else if y1 > vy + vh {
            ny = (y1 - vh).max(0.0);
        }
    }
    if (nx, ny) != (vx, vy) {
        fe.vue.0 = nx;
        fe.vue.1 = ny;
        fe.agrandir();
        let g = f.global::<SheetGrid>();
        g.set_content_width(fe.x(fe.etendue.0));
        g.set_content_height(fe.y(fe.etendue.1));
        g.set_scroll_x(-nx);
        g.set_scroll_y(-ny);
        g.set_scroll_serial(g.get_scroll_serial() + 1);
    }
}

// --- Changes ---------------------------------------------------------------------------

/// After a change of the workbook: values, drawing, the file soon.
fn change(f: &AppWindow, fe: &mut Feuille) {
    fe.recompute();
    fe.dirty = true;
    montrer_feuilles(f, fe);
    dessiner(f, fe, true);
    montrer_selection(f, fe);
    super::planifier_ecriture(f);
}

fn montrer_feuilles(f: &AppWindow, fe: &Feuille) {
    let g = f.global::<SheetGrid>();
    let noms: Vec<SharedString> = fe
        .book
        .sheets
        .iter()
        .map(|s| s.name.as_str().into())
        .collect();
    g.set_sheets(ModelRc::new(VecModel::from(noms)));
    g.set_sheet(fe.sheet as i32);
}

/// What was typed, put in cell `a`: a formula as typed, a date or a number read as
/// the user writes them (and its format suggested), else the text.
fn poser(fe: &mut Feuille, a: Addr, texte: &str) {
    poser_dans(&mut fe.book.sheets[fe.sheet], a, texte, &fe.locale);
}

/// [`poser`] for any sheet: also what is typed in a spreadsheet embedded in a note.
pub(super) fn poser_dans(s: &mut iris_sheets::Sheet, a: Addr, texte: &str, locale: &Locale) {
    let fmt = s.format(a);
    if texte.starts_with('=') || fmt.number == NumberFormat::Text || texte.trim().is_empty() {
        s.set_input(a, texte);
        return;
    }
    if let Some((n, genre)) = iris_sheets::format::read_date(texte, locale) {
        s.set_input(a, &iris_sheets::eval::general(n));
        if !matches!(
            fmt.number,
            NumberFormat::Date | NumberFormat::Time | NumberFormat::DateTime
        ) {
            s.set_format(Range::single(a), |f| f.number = genre);
        }
        return;
    }
    // A number typed with the user's decimal sign.
    let lu = if locale.decimal == ',' {
        iris_sheets::format::read_number(texte)
    } else {
        iris_sheets::format::read_number(&texte.replace(',', ""))
    };
    match lu {
        Some((n, genre)) => {
            s.set_input(a, &iris_sheets::eval::general(n));
            if let Some(g) = genre {
                if fmt.number == NumberFormat::General {
                    s.set_format(Range::single(a), |f| f.number = g);
                }
            }
        }
        None => s.set_input(a, texte),
    }
}

fn commencer(f: &AppWindow, fe: &mut Feuille, texte: &str) {
    let g = f.global::<SheetGrid>();
    fe.editing = Some(texte.to_string());
    g.set_edit_text(texte.into());
    g.set_edit_cursor(texte.len() as i32);
    g.set_editing(true);
    g.set_edit_serial(g.get_edit_serial() + 1);
    g.set_input(texte.into());
    g.set_input_serial(g.get_input_serial() + 1);
}

fn finir(f: &AppWindow, fe: &mut Feuille) {
    fe.editing = None;
    f.global::<SheetGrid>().set_editing(false);
}

/// The active cell moved by `dc` columns and `dr` rows; `etendre`: the selection grows.
fn bouger(f: &AppWindow, fe: &mut Feuille, dc: i64, dr: i64, etendre: bool) {
    if let Some(a) = fe.cursor.offset(dc, dr) {
        fe.cursor = a;
    }
    if !etendre {
        fe.anchor = fe.cursor;
    }
    suivre_curseur(f, fe);
    montrer_selection(f, fe);
}

fn aller(f: &AppWindow, fe: &mut Feuille, a: Addr, etendre: bool) {
    fe.cursor = a;
    if !etendre {
        fe.anchor = a;
    }
    suivre_curseur(f, fe);
    montrer_selection(f, fe);
}

/// Where Ctrl and an arrow go: to the edge of the data, as in every spreadsheet.
fn bord(fe: &Feuille, dc: i64, dr: i64) -> Addr {
    let s = fe.feuille();
    let plein = |a: Addr| !s.input(a).is_empty();
    let mut a = fe.cursor;
    let Some(premier) = a.offset(dc, dr) else {
        return a;
    };
    let (cols, rows) = s.extent();
    let limite = |b: Addr| b.col <= cols.max(1) && b.row <= rows.max(1);
    if plein(a) && plein(premier) {
        // Along the filled cells, to the last of them.
        while let Some(b) = a.offset(dc, dr) {
            if !plein(b) {
                break;
            }
            a = b;
        }
    } else {
        // Across the empty cells, to the next filled one (or the end of the data).
        a = premier;
        while !plein(a) {
            match a.offset(dc, dr) {
                Some(b) if limite(b) => a = b,
                _ => break,
            }
        }
    }
    a
}

fn appliquer_format(f: &AppWindow, fe: &mut Feuille, fx: impl Fn(&mut Format)) {
    fe.retenir();
    let sel = fe.selection();
    fe.book.sheets[fe.sheet].set_format(sel, fx);
    change(f, fe);
}

/// Ctrl+C: the selection's values on the clipboard as tab-separated text, and its
/// cells kept for a paste here.
fn copier(fe: &mut Feuille, couper: bool) {
    let sel = fe.selection();
    let lignes: Vec<Vec<String>> = (sel.start.row..=sel.end.row)
        .map(|r| {
            (sel.start.col..=sel.end.col)
                .map(|c| {
                    let a = Addr::new(c, r);
                    display(
                        &fe.values.get(&fe.book, fe.sheet, a),
                        &fe.feuille().format(a),
                        &fe.locale,
                    )
                })
                .collect()
        })
        .collect();
    let texte = iris_sheets::csv::write(&lignes, '\t');
    if let Ok(mut presse) = arboard::Clipboard::new() {
        let _ = presse.set_text(texte.clone());
    }
    fe.copie = Some((texte, fe.sheet, sel));
    if couper {
        fe.retenir();
        fe.book.sheets[fe.sheet].clear(sel);
    }
}

/// Ctrl+V: what this spreadsheet copied (formulas moved), else text from elsewhere,
/// split on tabs and lines.
fn coller(f: &AppWindow, fe: &mut Feuille) {
    let texte = arboard::Clipboard::new()
        .ok()
        .and_then(|mut p| p.get_text().ok())
        .unwrap_or_default();
    let ici = fe.selection().start;
    if let Some((copie, feuille, plage)) = fe.copie.clone() {
        if copie == texte || texte.is_empty() {
            fe.retenir();
            if feuille == fe.sheet {
                fe.book.copy(fe.sheet, plage, ici);
            } else {
                // From another sheet: the cells as they are.
                let cellules: Vec<(Addr, Option<iris_sheets::Cell>)> = plage
                    .cells()
                    .map(|a| (a, fe.book.sheets[feuille].cells.get(&a).cloned()))
                    .collect();
                for (a, c) in cellules {
                    let dc = i64::from(ici.col) - i64::from(plage.start.col);
                    let dr = i64::from(ici.row) - i64::from(plage.start.row);
                    if let Some(b) = a.offset(dc, dr) {
                        match c {
                            Some(c) => {
                                fe.book.sheets[fe.sheet].cells.insert(b, c);
                            }
                            None => {
                                fe.book.sheets[fe.sheet].cells.remove(&b);
                            }
                        }
                    }
                }
            }
            fe.anchor = ici;
            fe.cursor = Addr::new(ici.col + plage.width() - 1, ici.row + plage.height() - 1);
            change(f, fe);
            return;
        }
    }
    if texte.is_empty() {
        return;
    }
    let lignes = iris_sheets::csv::parse(&texte, '\t');
    fe.retenir();
    let mut fin = ici;
    for (r, ligne) in lignes.iter().enumerate() {
        for (c, v) in ligne.iter().enumerate() {
            if let Some(a) = ici.offset(c as i64, r as i64) {
                poser(fe, a, v);
                fin = Addr::new(fin.col.max(a.col), fin.row.max(a.row));
            }
        }
    }
    fe.anchor = ici;
    fe.cursor = fin;
    change(f, fe);
}

fn annuler(f: &AppWindow, fe: &mut Feuille, refaire: bool) {
    let (depuis, vers) = if refaire {
        (&mut fe.redo, &mut fe.undo)
    } else {
        (&mut fe.undo, &mut fe.redo)
    };
    let Some((book, sheet)) = depuis.pop() else {
        return;
    };
    vers.push((std::mem::replace(&mut fe.book, book), fe.sheet));
    fe.sheet = sheet.min(fe.book.sheets.len() - 1);
    change(f, fe);
}

/// A key while the grid has the keyboard; `true` when it did something.
fn touche(f: &AppWindow, fe: &mut Feuille, texte: &str, shift: bool, ctrl: bool) -> bool {
    use slint::platform::Key;
    let k = |key: Key| -> SharedString { key.into() };
    let t = SharedString::from(texte);
    let page = ((fe.vue.3 / ROW_HEIGHT) as i64 - 1).max(1);
    if t == k(Key::UpArrow)
        || t == k(Key::DownArrow)
        || t == k(Key::LeftArrow)
        || t == k(Key::RightArrow)
    {
        let (dc, dr) = if t == k(Key::UpArrow) {
            (0, -1)
        } else if t == k(Key::DownArrow) {
            (0, 1)
        } else if t == k(Key::LeftArrow) {
            (-1, 0)
        } else {
            (1, 0)
        };
        if ctrl {
            let a = bord(fe, dc, dr);
            aller(f, fe, a, shift);
        } else {
            bouger(f, fe, dc, dr, shift);
        }
        return true;
    }
    if t == k(Key::Return) {
        bouger(f, fe, 0, if shift { -1 } else { 1 }, false);
        return true;
    }
    if t == k(Key::Tab) || t == k(Key::Backtab) {
        bouger(
            f,
            fe,
            if shift || t == k(Key::Backtab) { -1 } else { 1 },
            0,
            false,
        );
        return true;
    }
    if t == k(Key::PageDown) || t == k(Key::PageUp) {
        bouger(
            f,
            fe,
            0,
            if t == k(Key::PageDown) { page } else { -page },
            shift,
        );
        return true;
    }
    if t == k(Key::Home) {
        let a = if ctrl {
            Addr::new(0, 0)
        } else {
            Addr::new(0, fe.cursor.row)
        };
        aller(f, fe, a, shift);
        return true;
    }
    if t == k(Key::End) {
        let (cols, rows) = fe.feuille().extent();
        let a = if ctrl {
            Addr::new(cols.saturating_sub(1), rows.saturating_sub(1))
        } else {
            Addr::new(cols.saturating_sub(1), fe.cursor.row)
        };
        aller(f, fe, a, shift);
        return true;
    }
    if t == k(Key::F2) {
        let s = saisie_de(fe, fe.cursor);
        commencer(f, fe, &s);
        return true;
    }
    if t == k(Key::Delete) {
        fe.retenir();
        let sel = fe.selection();
        fe.book.sheets[fe.sheet].clear(sel);
        change(f, fe);
        return true;
    }
    if t == k(Key::Backspace) {
        fe.retenir();
        let a = fe.cursor;
        fe.book.sheets[fe.sheet].set_input(a, "");
        change(f, fe);
        commencer(f, fe, "");
        return true;
    }
    if t == k(Key::Escape) {
        if fe.anchor != fe.cursor {
            fe.anchor = fe.cursor;
            montrer_selection(f, fe);
            return true;
        }
        return false;
    }
    if ctrl {
        match texte.to_lowercase().as_str() {
            "a" => {
                let (cols, rows) = fe.feuille().extent();
                fe.anchor = Addr::new(0, 0);
                fe.cursor = Addr::new(cols.max(1) - 1, rows.max(1) - 1);
                montrer_selection(f, fe);
            }
            "c" => copier(fe, false),
            "x" => {
                copier(fe, true);
                change(f, fe);
            }
            "v" => coller(f, fe),
            "z" if shift => annuler(f, fe, true),
            "z" => annuler(f, fe, false),
            "y" => annuler(f, fe, true),
            "d" => action(f, fe, "fill-down"),
            "r" => action(f, fe, "fill-right"),
            "b" => action(f, fe, "bold"),
            "i" => action(f, fe, "italic"),
            "u" => action(f, fe, "underline"),
            "5" | "(" => action(f, fe, "strike"),
            "s" => {
                fe.dirty = true;
                super::planifier_ecriture(f);
            }
            _ => return false,
        }
        return true;
    }
    // A character typed: writing in the cell starts with it.
    let mut car = texte.chars();
    if let (Some(c), None) = (car.next(), car.next()) {
        if !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(&c) {
            commencer(f, fe, texte);
            return true;
        }
    }
    false
}

/// The formula being typed takes the address of the cell clicked, when it waits for
/// one (after `=`, an operator, `(` or a separator).
fn pointer(f: &AppWindow, fe: &mut Feuille, a: Addr, etendre: bool) -> bool {
    let Some(texte) = fe.editing.clone() else {
        return false;
    };
    if !texte.starts_with('=') {
        return false;
    }
    let fin = texte.trim_end();
    let attend = fin.ends_with([
        '=', '+', '-', '*', '/', '^', '&', '(', ',', ';', '<', '>', ':',
    ]);
    let nouveau = if attend {
        format!("{fin}{}", a.name())
    } else if etendre {
        // Shift and a click: the address typed last becomes a range.
        match fin.rfind(|c: char| !c.is_ascii_alphanumeric() && c != '$') {
            Some(k) if Addr::parse(&fin[k + 1..]).is_some() => {
                let premier = fin[k + 1..].to_string();
                format!("{}{premier}:{}", &fin[..=k], a.name())
            }
            _ => return false,
        }
    } else {
        // A click again replaces the address typed last.
        match fin.rfind(|c: char| !c.is_ascii_alphanumeric() && c != '$') {
            Some(k) if Addr::parse(&fin[k + 1..]).is_some() => {
                format!("{}{}", &fin[..=k], a.name())
            }
            _ => return false,
        }
    };
    commencer(f, fe, &nouveau);
    true
}

/// A button of the toolbar, an entry of a menu, or a shortcut.
fn action(f: &AppWindow, fe: &mut Feuille, quoi: &str) {
    let actif = fe.feuille().format(fe.cursor);
    let sel = fe.selection();
    match quoi {
        "undo" => annuler(f, fe, false),
        "redo" => annuler(f, fe, true),
        "bold" => appliquer_format(f, fe, |x| x.bold = !actif.bold),
        "italic" => appliquer_format(f, fe, |x| x.italic = !actif.italic),
        "underline" => appliquer_format(f, fe, |x| x.underline = !actif.underline),
        "strike" => appliquer_format(f, fe, |x| x.strike = !actif.strike),
        "wrap" => appliquer_format(f, fe, |x| x.wrap = !actif.wrap),
        "border" => appliquer_format(f, fe, |x| x.border = !actif.border),
        "align-left" | "align-center" | "align-right" => {
            let voulu = match quoi {
                "align-left" => Align::Left,
                "align-center" => Align::Center,
                _ => Align::Right,
            };
            let a = if actif.align == voulu {
                Align::Auto
            } else {
                voulu
            };
            appliquer_format(f, fe, |x| x.align = a);
        }
        "decimals+" | "decimals-" => {
            let d = actif.decimals();
            let n = if quoi == "decimals+" {
                (d + 1).min(10)
            } else {
                d.saturating_sub(1)
            };
            appliquer_format(f, fe, |x| {
                x.decimals = Some(n);
                if x.number == NumberFormat::General {
                    x.number = NumberFormat::Number;
                }
            });
        }
        "select-all" => {
            let (cols, rows) = fe.feuille().extent();
            fe.anchor = Addr::new(0, 0);
            fe.cursor = Addr::new(cols.max(1) - 1, rows.max(1) - 1);
            montrer_selection(f, fe);
        }
        "sort-asc" | "sort-desc" => {
            let plage = if sel.start == sel.end {
                let (cols, rows) = fe.feuille().extent();
                let haut = fe.feuille().frozen_rows;
                if rows <= haut + 1 {
                    return;
                }
                Range::new(Addr::new(0, haut), Addr::new(cols.max(1) - 1, rows - 1))
            } else {
                sel
            };
            fe.retenir();
            let valeurs = fe.values.clone();
            fe.book
                .sort(fe.sheet, plage, fe.cursor.col, quoi == "sort-asc", &valeurs);
            change(f, fe);
        }
        "freeze" => {
            fe.retenir();
            let s = &mut fe.book.sheets[fe.sheet];
            s.frozen_rows = if s.frozen_rows > 0 {
                0
            } else {
                fe.cursor.row.max(1)
            };
            change(f, fe);
        }
        "row-above" | "row-below" | "row-delete" | "col-left" | "col-right" | "col-delete" => {
            let c = match quoi {
                "row-above" => Change::InsertRows {
                    at: sel.start.row,
                    n: sel.height(),
                },
                "row-below" => Change::InsertRows {
                    at: sel.end.row + 1,
                    n: sel.height(),
                },
                "row-delete" => Change::DeleteRows {
                    at: sel.start.row,
                    n: sel.height(),
                },
                "col-left" => Change::InsertCols {
                    at: sel.start.col,
                    n: sel.width(),
                },
                "col-right" => Change::InsertCols {
                    at: sel.end.col + 1,
                    n: sel.width(),
                },
                _ => Change::DeleteCols {
                    at: sel.start.col,
                    n: sel.width(),
                },
            };
            fe.retenir();
            fe.book.change(fe.sheet, c);
            if quoi == "row-below" {
                fe.anchor = Addr::new(sel.start.col, sel.end.row + 1);
                fe.cursor = fe.anchor;
            } else if quoi == "col-right" {
                fe.anchor = Addr::new(sel.end.col + 1, sel.start.row);
                fe.cursor = fe.anchor;
            } else {
                fe.cursor = fe.anchor;
            }
            change(f, fe);
        }
        "fill-down" if sel.height() > 1 => {
            fe.retenir();
            fe.book.fill_down(fe.sheet, sel);
            change(f, fe);
        }
        "fill-right" if sel.width() > 1 => {
            fe.retenir();
            fe.book.fill_right(fe.sheet, sel);
            change(f, fe);
        }
        "sheet-add" => {
            fe.retenir();
            let nom = fe.book.new_sheet_name();
            fe.book.sheets.push(iris_sheets::Sheet::new(&nom));
            fe.sheet = fe.book.sheets.len() - 1;
            fe.anchor = Addr::default();
            fe.cursor = Addr::default();
            fe.dessine = None;
            change(f, fe);
        }
        "sheet-duplicate" => {
            fe.retenir();
            let mut copie = fe.feuille().clone();
            copie.name = (2..)
                .map(|k| format!("{} {k}", copie.name))
                .find(|n| fe.book.sheet_index(n).is_none())
                .unwrap_or_default();
            fe.book.sheets.insert(fe.sheet + 1, copie);
            fe.sheet += 1;
            fe.dessine = None;
            change(f, fe);
        }
        "sheet-delete" => {
            if fe.book.sheets.len() < 2 {
                f.set_toast("A spreadsheet keeps at least one sheet.".into());
                return;
            }
            fe.retenir();
            fe.book.sheets.remove(fe.sheet);
            fe.sheet = fe.sheet.min(fe.book.sheets.len() - 1);
            fe.anchor = Addr::default();
            fe.cursor = Addr::default();
            fe.dessine = None;
            change(f, fe);
        }
        "export-xlsx" => exporter(f, fe, true),
        "export-csv" => exporter(f, fe, false),
        _ => {
            if let Some(nom) = quoi.strip_prefix("sheet-rename:") {
                let avant = fe.book.clone();
                if fe.book.rename_sheet(fe.sheet, nom) {
                    fe.undo.push((avant, fe.sheet));
                    fe.redo.clear();
                    change(f, fe);
                } else {
                    f.set_toast("That name cannot be used for a sheet.".into());
                }
            } else if let Some(genre) = quoi.strip_prefix("number:") {
                let n = match genre {
                    "number" => NumberFormat::Number,
                    "percent" => NumberFormat::Percent,
                    "currency" => NumberFormat::Currency,
                    "date" => NumberFormat::Date,
                    "time" => NumberFormat::Time,
                    "datetime" => NumberFormat::DateTime,
                    "text" => NumberFormat::Text,
                    _ => NumberFormat::General,
                };
                appliquer_format(f, fe, |x| {
                    x.number = n;
                    x.decimals = None;
                });
            } else if let Some(c) = quoi.strip_prefix("color:") {
                let v = c
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| TEXTES.get(i))
                    .map(|s| s.to_string());
                appliquer_format(f, fe, |x| x.color = v.clone());
            } else if let Some(c) = quoi.strip_prefix("fill:") {
                let v = c
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| FONDS.get(i))
                    .map(|s| s.to_string());
                appliquer_format(f, fe, |x| x.fill = v.clone());
            }
        }
    }
}

fn exporter(f: &AppWindow, fe: &mut Feuille, excel: bool) {
    let nom = stem(&fe.rel);
    let (ext, filtre) = if excel {
        ("xlsx", "Excel workbook")
    } else {
        ("csv", "CSV")
    };
    let Some(chemin) = rfd::FileDialog::new()
        .set_title(if excel {
            "Export as an Excel workbook"
        } else {
            "Export the sheet as CSV"
        })
        .set_file_name(format!("{nom}.{ext}"))
        .add_filter(filtre, &[ext])
        .save_file()
    else {
        return;
    };
    let octets = if excel {
        match iris_sheets::xlsx::write(&fe.book, &fe.values, &fe.locale) {
            Ok(o) => o,
            Err(err) => {
                f.set_status(format!("Could not write the workbook: {err}").into());
                return;
            }
        }
    } else {
        // The values, numbers with the region's decimal sign; `;` between them where
        // that sign is a comma.
        let sep = if fe.locale.decimal == ',' { ';' } else { ',' };
        let s = fe.feuille();
        let (cols, rows) = s.extent();
        let lignes: Vec<Vec<String>> = (0..rows)
            .map(|r| {
                (0..cols)
                    .map(
                        |c| match fe.values.get(&fe.book, fe.sheet, Addr::new(c, r)) {
                            Value::Number(n) => iris_sheets::eval::general(n)
                                .replace('.', &fe.locale.decimal.to_string()),
                            v => v.text().unwrap_or_else(|e| e.text().to_string()),
                        },
                    )
                    .collect()
            })
            .collect();
        format!("\u{feff}{}", iris_sheets::csv::write(&lignes, sep)).into_bytes()
    };
    match std::fs::write(&chemin, octets) {
        Ok(()) => f.set_toast(format!("“{nom}” is exported.").into()),
        Err(err) => f.set_status(format!("Could not write the file: {err}").into()),
    }
}

// --- Opening and writing ---------------------------------------------------------------

/// Opens the spreadsheet `rel` of the space.
pub(super) fn ouvrir(f: &AppWindow, e: &mut Etat, rel: &str) {
    let Some(espace) = e.espace() else { return };
    let (texte, modified) = match espace.read(rel) {
        Ok(x) => x,
        Err(err) => {
            f.set_status(format!("Could not open the spreadsheet: {err}").into());
            return;
        }
    };
    let book = if texte.trim().is_empty() {
        Workbook::default()
    } else {
        match Workbook::from_json(&texte) {
            Ok(b) => b,
            Err(err) => {
                f.set_status(format!("This spreadsheet could not be read: {err}").into());
                return;
            }
        }
    };
    let values = book.compute();
    e.sheet = Some(Feuille {
        rel: rel.to_string(),
        book,
        values,
        sheet: 0,
        anchor: Addr::default(),
        cursor: Addr::default(),
        editing: None,
        vue: (0.0, 0.0, 800.0, 600.0),
        dessine: None,
        etendue: (26, 100),
        undo: Vec::new(),
        redo: Vec::new(),
        modified: Some(modified),
        dirty: false,
        copie: None,
        locale: locale(),
    });
    f.set_note_sheet_open(true);
    let g = f.global::<SheetGrid>();
    g.set_scroll_x(0.0);
    g.set_scroll_y(0.0);
    g.set_scroll_serial(g.get_scroll_serial() + 1);
    g.set_editing(false);
    g.set_menu(SharedString::default());
    if let Some(fe) = &mut e.sheet {
        montrer_feuilles(f, fe);
        dessiner(f, fe, true);
        montrer_selection(f, fe);
    }
    g.set_focus_serial(g.get_focus_serial() + 1);
}

/// Writes the spreadsheet if it changed.
pub(super) fn ecrire(f: &AppWindow, e: &mut Etat) {
    let Some(espace) = e.spaces.get(e.space).cloned() else {
        return;
    };
    let Some(fe) = &mut e.sheet else { return };
    if !fe.dirty {
        return;
    }
    match espace.write(&fe.rel, &fe.book.to_json(), fe.modified) {
        Ok(iris_vault::WriteOutcome::Written(m)) => {
            fe.modified = Some(m);
            fe.dirty = false;
            f.set_note_status("Saved".into());
        }
        Ok(iris_vault::WriteOutcome::Conflict { saved_as, modified }) => {
            fe.modified = Some(modified);
            fe.dirty = false;
            f.set_toast(
                format!(
                    "This spreadsheet was changed elsewhere too: that version is kept as “{}”.",
                    stem(&saved_as)
                )
                .into(),
            );
        }
        Err(err) => f.set_status(format!("Could not save the spreadsheet: {err}").into()),
    }
}

/// Closes the spreadsheet, written first.
pub(super) fn fermer(f: &AppWindow, e: &mut Etat) {
    ecrire(f, e);
    e.sheet = None;
    f.set_note_sheet_open(false);
    f.global::<SheetGrid>().set_menu(SharedString::default());
}

/// A new spreadsheet in `folder`, opened.
pub(super) fn nouvelle(f: &AppWindow, e: &mut Etat, folder: &str) -> Option<String> {
    let espace = e.espace()?;
    match espace.create_file(
        folder,
        "Untitled",
        "sheet",
        Workbook::default().to_json().as_bytes(),
    ) {
        Ok(rel) => Some(rel),
        Err(err) => {
            f.set_status(format!("Could not create the spreadsheet: {err}").into());
            None
        }
    }
}

/// A CSV or Excel file of the space made into a spreadsheet beside it.
pub(super) fn importer(f: &AppWindow, e: &mut Etat, rel: &str) -> Option<String> {
    let espace = e.espace()?;
    let chemin = espace.path(rel).ok()?;
    let octets = match std::fs::read(&chemin) {
        Ok(o) => o,
        Err(err) => {
            f.set_status(format!("Could not read the file: {err}").into());
            return None;
        }
    };
    let ext = rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let book = if ext == "csv" || ext == "tsv" || ext == "txt" {
        let texte = String::from_utf8_lossy(&octets).into_owned();
        let sep = if ext == "tsv" {
            '\t'
        } else {
            iris_sheets::csv::guess_separator(&texte)
        };
        let lignes = iris_sheets::csv::parse(&texte, sep);
        let l = locale();
        let mut fe_tmp = Feuille {
            rel: String::new(),
            book: Workbook::default(),
            values: Values::default(),
            sheet: 0,
            anchor: Addr::default(),
            cursor: Addr::default(),
            editing: None,
            vue: (0.0, 0.0, 0.0, 0.0),
            dessine: None,
            etendue: (0, 0),
            undo: Vec::new(),
            redo: Vec::new(),
            modified: None,
            dirty: false,
            copie: None,
            locale: l,
        };
        for (r, ligne) in lignes.iter().enumerate() {
            for (c, v) in ligne.iter().enumerate() {
                poser(&mut fe_tmp, Addr::new(c as u32, r as u32), v);
            }
        }
        fe_tmp.book
    } else {
        match iris_sheets::xlsx::read(&octets) {
            Ok(b) => b,
            Err(err) => {
                f.set_status(format!("This workbook could not be read: {err}").into());
                return None;
            }
        }
    };
    let dossier = super::parent_of(rel);
    match espace.create_file(&dossier, &stem(rel), "sheet", book.to_json().as_bytes()) {
        Ok(nouveau) => Some(nouveau),
        Err(err) => {
            f.set_status(format!("Could not create the spreadsheet: {err}").into());
            None
        }
    }
}

// --- The window's callbacks ------------------------------------------------------------

pub(super) fn wire(f: &AppWindow, etat: &Rc<RefCell<Etat>>) {
    let g = f.global::<SheetGrid>();
    // The palettes offered, from the lists the cells' colours are written from.
    let palette = |liste: &[&str]| -> ModelRc<slint::Color> {
        ModelRc::new(VecModel::from(
            liste
                .iter()
                .copied()
                .filter_map(couleur)
                .collect::<Vec<_>>(),
        ))
    };
    g.set_text_colours(palette(&TEXTES));
    g.set_fill_colours(palette(&FONDS));
    macro_rules! geste {
        ($installer:ident, |$f:ident, $fe:ident $(, $arg:ident)*| $corps:block) => {{
            let (etat, faible) = (Rc::clone(etat), f.as_weak());
            g.$installer(move |$($arg),*| {
                let Some($f) = faible.upgrade() else { return };
                let Ok(mut guard) = etat.try_borrow_mut() else { return };
                let Some($fe) = guard.sheet.as_mut() else { return };
                $corps
            });
        }};
    }
    geste!(on_viewport, |f, fe, x, y, w, h| {
        fe.vue = (x, y, w, h);
        dessiner(&f, fe, false);
    });
    geste!(on_pressed, |f, fe, x, y, shift| {
        let a = Addr::new(fe.col_at(x), fe.row_at(y));
        if pointer(&f, fe, a, shift) {
            return;
        }
        if let Some(t) = fe.editing.take() {
            fe.retenir();
            let c = fe.cursor;
            poser(fe, c, &t);
            finir(&f, fe);
            fe.cursor = a;
            if !shift {
                fe.anchor = a;
            }
            change(&f, fe);
            return;
        }
        aller(&f, fe, a, shift);
    });
    geste!(on_dragged, |f, fe, x, y| {
        if fe.editing.is_some() {
            return;
        }
        let a = Addr::new(fe.col_at(x.max(0.0)), fe.row_at(y.max(0.0)));
        if a != fe.cursor {
            fe.cursor = a;
            montrer_selection(&f, fe);
        }
    });
    geste!(on_double_clicked, |f, fe, x, y| {
        let a = Addr::new(fe.col_at(x), fe.row_at(y));
        fe.anchor = a;
        fe.cursor = a;
        montrer_selection(&f, fe);
        let s = saisie_de(fe, a);
        commencer(&f, fe, &s);
    });
    geste!(on_header_pressed, |f, fe, genre, i, shift| {
        let i = i.max(0) as u32;
        let (debut, fin) = if genre == 0 {
            (Addr::new(i, 0), Addr::new(i, fe.etendue.1.max(1) - 1))
        } else {
            (Addr::new(0, i), Addr::new(fe.etendue.0.max(1) - 1, i))
        };
        if shift {
            fe.cursor = if genre == 0 {
                Addr::new(i, fin.row)
            } else {
                Addr::new(fin.col, i)
            };
            if genre == 0 {
                fe.anchor.row = 0;
            } else {
                fe.anchor.col = 0;
            }
        } else {
            fe.anchor = debut;
            fe.cursor = fin;
        }
        montrer_selection(&f, fe);
    });
    geste!(on_header_resized, |f, fe, genre, i, taille| {
        fe.retenir();
        let i = i.max(0) as u32;
        let s = &mut fe.book.sheets[fe.sheet];
        if genre == 0 {
            s.col_widths.insert(i, taille.clamp(24.0, 2000.0));
        } else {
            s.row_heights.insert(i, taille.clamp(16.0, 1000.0));
        }
        change(&f, fe);
    });
    {
        let (etat, faible) = (Rc::clone(etat), f.as_weak());
        g.on_key(move |texte, shift, ctrl| {
            let Some(f) = faible.upgrade() else {
                return false;
            };
            let Ok(mut guard) = etat.try_borrow_mut() else {
                return false;
            };
            let Some(fe) = guard.sheet.as_mut() else {
                return false;
            };
            touche(&f, fe, &texte, shift, ctrl)
        });
    }
    geste!(on_edit_done, |f, fe, texte, sens| {
        if fe.editing.take().is_none() {
            finir(&f, fe);
            return;
        }
        finir(&f, fe);
        if sens >= 0 {
            fe.retenir();
            let c = fe.cursor;
            poser(fe, c, &texte);
            let (dc, dr) = match sens {
                1 => (0, 1),
                2 => (1, 0),
                3 => (0, -1),
                4 => (-1, 0),
                _ => (0, 0),
            };
            if let Some(a) = fe.cursor.offset(dc, dr) {
                fe.cursor = a;
            }
            fe.anchor = fe.cursor;
            suivre_curseur(&f, fe);
            change(&f, fe);
        } else {
            montrer_selection(&f, fe);
        }
    });
    geste!(on_edit_changed, |f, fe, texte| {
        fe.editing = Some(texte.to_string());
        let g = f.global::<SheetGrid>();
        g.set_input(texte);
        g.set_input_serial(g.get_input_serial() + 1);
    });
    geste!(on_input_done, |f, fe, texte| {
        fe.editing = None;
        finir(&f, fe);
        fe.retenir();
        let c = fe.cursor;
        poser(fe, c, &texte);
        change(&f, fe);
    });
    geste!(on_action, |f, fe, quoi| {
        action(&f, fe, &quoi);
    });
    geste!(on_sheet_chosen, |f, fe, i| {
        let i = i.max(0) as usize;
        if i < fe.book.sheets.len() && i != fe.sheet {
            if let Some(t) = fe.editing.take() {
                fe.retenir();
                let c = fe.cursor;
                poser(fe, c, &t);
                finir(&f, fe);
                fe.recompute();
            }
            fe.sheet = i;
            fe.anchor = Addr::default();
            fe.cursor = Addr::default();
            fe.dessine = None;
            let g = f.global::<SheetGrid>();
            fe.vue.0 = 0.0;
            fe.vue.1 = 0.0;
            g.set_scroll_x(0.0);
            g.set_scroll_y(0.0);
            g.set_scroll_serial(g.get_scroll_serial() + 1);
            montrer_feuilles(&f, fe);
            dessiner(&f, fe, true);
            montrer_selection(&f, fe);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feuille() -> Feuille {
        Feuille {
            rel: "Budget.sheet".into(),
            book: Workbook::default(),
            values: Values::default(),
            sheet: 0,
            anchor: Addr::default(),
            cursor: Addr::default(),
            editing: None,
            vue: (0.0, 0.0, 800.0, 600.0),
            dessine: None,
            etendue: (26, 100),
            undo: Vec::new(),
            redo: Vec::new(),
            modified: None,
            dirty: false,
            copie: None,
            locale: Locale::french(),
        }
    }

    #[test]
    fn columns_and_rows_found_under_the_pointer() {
        let mut fe = feuille();
        fe.book.sheets[0].col_widths.insert(1, 200.0);
        assert_eq!(fe.x(2), 300.0);
        assert_eq!(fe.col_at(0.0), 0);
        assert_eq!(fe.col_at(99.0), 0);
        assert_eq!(fe.col_at(100.0), 1);
        assert_eq!(fe.col_at(299.0), 1);
        assert_eq!(fe.col_at(300.0), 2);
        assert_eq!(fe.row_at(ROW_HEIGHT * 3.5), 3);
    }

    #[test]
    fn typed_values_are_read_as_the_user_writes_them() {
        let mut fe = feuille();
        poser(&mut fe, Addr::new(0, 0), "12,5");
        poser(&mut fe, Addr::new(0, 1), "07/10/2026");
        poser(&mut fe, Addr::new(0, 2), "15 %");
        poser(&mut fe, Addr::new(0, 3), "=A1*2");
        poser(&mut fe, Addr::new(0, 4), "Chapitre 2");
        let s = &fe.book.sheets[0];
        assert_eq!(s.input(Addr::new(0, 0)), "12.5");
        assert_eq!(s.input(Addr::new(0, 1)), "46302");
        assert_eq!(s.format(Addr::new(0, 1)).number, NumberFormat::Date);
        assert_eq!(s.input(Addr::new(0, 2)), "0.15");
        assert_eq!(s.format(Addr::new(0, 2)).number, NumberFormat::Percent);
        assert_eq!(s.input(Addr::new(0, 3)), "=A1*2");
        assert_eq!(s.input(Addr::new(0, 4)), "Chapitre 2");
        fe.recompute();
        assert_eq!(saisie_de(&fe, Addr::new(0, 0)), "12,5");
        assert_eq!(saisie_de(&fe, Addr::new(0, 1)), "07/10/2026");
        assert_eq!(saisie_de(&fe, Addr::new(0, 2)), "15%");
    }

    #[test]
    fn ctrl_and_an_arrow_go_to_the_edge_of_the_data() {
        let mut fe = feuille();
        for r in 0..5 {
            poser(&mut fe, Addr::new(0, r), "1");
        }
        poser(&mut fe, Addr::new(0, 9), "1");
        assert_eq!(bord(&fe, 0, 1), Addr::new(0, 4));
        fe.cursor = Addr::new(0, 4);
        assert_eq!(bord(&fe, 0, 1), Addr::new(0, 9));
        fe.cursor = Addr::new(0, 0);
        assert_eq!(bord(&fe, 0, -1), Addr::new(0, 0));
    }
}
