//! A workbook, its sheets and cells, and the `.sheet` file they are kept in.
//!
//! The file is JSON, readable and small: what was typed in each cell (formulas as
//! typed), its format when it has one, the widths and heights changed, the rows kept
//! at the top. Values are not kept: they are computed when the file is opened.

use crate::cell::{col_index, col_name, Addr, Range};
use crate::eval::{literal, Value};
use crate::format::Format;
use crate::formula::{self, Change};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// A column's width and a row's height when they were not changed.
pub const COL_WIDTH: f32 = 100.0;
pub const ROW_HEIGHT: f32 = 26.0;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cell {
    /// As typed: `12`, `Paris`, `=SUM(A1:A3)`.
    pub input: String,
    pub format: Format,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub cells: BTreeMap<Addr, Cell>,
    pub col_widths: BTreeMap<u32, f32>,
    pub row_heights: BTreeMap<u32, f32>,
    /// Rows and columns kept in view while the rest scrolls.
    pub frozen_rows: u32,
    pub frozen_cols: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
}

impl Default for Workbook {
    fn default() -> Self {
        Self {
            sheets: vec![Sheet::new("Sheet1")],
        }
    }
}

impl Sheet {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            cells: BTreeMap::new(),
            col_widths: BTreeMap::new(),
            row_heights: BTreeMap::new(),
            frozen_rows: 0,
            frozen_cols: 0,
        }
    }

    pub fn input(&self, a: Addr) -> &str {
        self.cells.get(&a).map_or("", |c| c.input.as_str())
    }

    pub fn format(&self, a: Addr) -> Format {
        self.cells
            .get(&a)
            .map(|c| c.format.clone())
            .unwrap_or_default()
    }

    /// Puts what was typed in a cell, its format kept.
    pub fn set_input(&mut self, a: Addr, input: &str) {
        let c = self.cells.entry(a).or_default();
        c.input = input.to_string();
        if c.input.is_empty() && c.format.is_default() {
            self.cells.remove(&a);
        }
    }

    /// Changes the format of every cell of `r`.
    pub fn set_format(&mut self, r: Range, f: impl Fn(&mut Format)) {
        for a in r.cells() {
            let c = self.cells.entry(a).or_default();
            f(&mut c.format);
            if c.input.is_empty() && c.format.is_default() {
                self.cells.remove(&a);
            }
        }
    }

    /// Empties the cells of `r`, their formats kept.
    pub fn clear(&mut self, r: Range) {
        let dedans: Vec<Addr> = self
            .cells
            .keys()
            .filter(|a| r.contains(**a))
            .copied()
            .collect();
        for a in dedans {
            self.set_input(a, "");
        }
    }

    /// How far the sheet is used: columns and rows.
    pub fn extent(&self) -> (u32, u32) {
        let mut cols = 0;
        let mut rows = 0;
        for a in self.cells.keys() {
            cols = cols.max(a.col + 1);
            rows = rows.max(a.row + 1);
        }
        (cols, rows)
    }

    pub fn col_width(&self, c: u32) -> f32 {
        self.col_widths.get(&c).copied().unwrap_or(COL_WIDTH)
    }

    pub fn row_height(&self, r: u32) -> f32 {
        self.row_heights.get(&r).copied().unwrap_or(ROW_HEIGHT)
    }

    /// A cell's value when it is not a formula.
    pub fn value_of_literal(&self, a: Addr) -> Value {
        match self.cells.get(&a) {
            Some(c) if c.format.number == crate::format::NumberFormat::Text => {
                if c.input.is_empty() {
                    Value::Empty
                } else {
                    Value::Text(c.input.clone())
                }
            }
            Some(c) if !c.input.starts_with('=') => literal(&c.input),
            _ => Value::Empty,
        }
    }

    /// The cells as rows of what was typed, from A1 to the last used.
    pub fn rows(&self) -> Vec<Vec<String>> {
        let (cols, rows) = self.extent();
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| self.input(Addr::new(c, r)).to_string())
                    .collect()
            })
            .collect()
    }

    /// A sheet made of rows of text (from CSV).
    pub fn from_rows(name: &str, rows: &[Vec<String>]) -> Self {
        let mut s = Sheet::new(name);
        for (r, ligne) in rows.iter().enumerate() {
            for (c, v) in ligne.iter().enumerate() {
                if !v.is_empty() {
                    s.set_input(Addr::new(c as u32, r as u32), v);
                }
            }
        }
        s
    }
}

/// Moves the keys of a map by a change of rows or columns; those deleted are dropped.
fn deplacer_cles<V>(m: &mut BTreeMap<u32, V>, at: u32, n: u32, insert: bool) {
    let anciennes = std::mem::take(m);
    for (k, v) in anciennes {
        if k < at {
            m.insert(k, v);
        } else if insert {
            m.insert(k + n, v);
        } else if k >= at + n {
            m.insert(k - n, v);
        }
    }
}

impl Workbook {
    /// The values of every cell of the workbook, computed now.
    pub fn compute(&self) -> Values {
        Values(crate::eval::recalc(
            self,
            chrono::Local::now().naive_local(),
        ))
    }

    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.sheets
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// A name for a new sheet: `Sheet2`, `Sheet3`…
    pub fn new_sheet_name(&self) -> String {
        (1..)
            .map(|k| format!("Sheet{k}"))
            .find(|n| self.sheet_index(n).is_none())
            .unwrap_or_default()
    }

    /// Renames a sheet and the formulas that read it. Refused when the name is taken or empty.
    pub fn rename_sheet(&mut self, i: usize, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty()
            || name.contains(['\'', '!', '[', ']', '*', '?', '/', '\\', ':'])
            || self.sheet_index(name).is_some_and(|j| j != i)
            || i >= self.sheets.len()
        {
            return false;
        }
        let ancien = self.sheets[i].name.clone();
        for s in &mut self.sheets {
            for c in s.cells.values_mut() {
                if let Some(f) = c.input.strip_prefix('=') {
                    c.input = format!("={}", formula::rename_sheet(f, &ancien, name));
                }
            }
        }
        self.sheets[i].name = name.to_string();
        true
    }

    /// Rows or columns inserted or deleted in sheet `i`: its cells move, and every
    /// formula reading them follows.
    pub fn change(&mut self, i: usize, change: Change) {
        let Some(nom) = self.sheets.get(i).map(|s| s.name.clone()) else {
            return;
        };
        for s in &mut self.sheets {
            let home = s.name.clone();
            for c in s.cells.values_mut() {
                if let Some(f) = c.input.strip_prefix('=') {
                    c.input = format!("={}", formula::adjust(f, &home, &nom, change));
                }
            }
        }
        let s = &mut self.sheets[i];
        let (at, n, insert, rows) = match change {
            Change::InsertRows { at, n } => (at, n, true, true),
            Change::DeleteRows { at, n } => (at, n, false, true),
            Change::InsertCols { at, n } => (at, n, true, false),
            Change::DeleteCols { at, n } => (at, n, false, false),
        };
        let anciennes = std::mem::take(&mut s.cells);
        for (a, c) in anciennes {
            let x = if rows { a.row } else { a.col };
            let nx = if x < at {
                x
            } else if insert {
                x + n
            } else if x >= at + n {
                x - n
            } else {
                continue;
            };
            let na = if rows {
                Addr::new(a.col, nx)
            } else {
                Addr::new(nx, a.row)
            };
            if na.row < crate::cell::MAX_ROWS && na.col < crate::cell::MAX_COLS {
                s.cells.insert(na, c);
            }
        }
        if rows {
            deplacer_cles(&mut s.row_heights, at, n, insert);
        } else {
            deplacer_cles(&mut s.col_widths, at, n, insert);
        }
    }

    /// The cells of `r` copied to `to` (its top left): formulas moved as copies move.
    pub fn copy(&mut self, i: usize, r: Range, to: Addr) {
        let s = &mut self.sheets[i];
        let source: Vec<(Addr, Option<Cell>)> =
            r.cells().map(|a| (a, s.cells.get(&a).cloned())).collect();
        for (a, c) in source {
            let dc = i64::from(to.col) - i64::from(r.start.col);
            let dr = i64::from(to.row) - i64::from(r.start.row);
            let Some(cible) = a.offset(dc, dr) else {
                continue;
            };
            match c {
                Some(mut c) => {
                    if let Some(f) = c.input.strip_prefix('=') {
                        c.input = format!("={}", formula::shift(f, dc, dr));
                    }
                    s.cells.insert(cible, c);
                }
                None => {
                    s.cells.remove(&cible);
                }
            }
        }
    }

    /// The first row of `r` copied to the rows under it (Ctrl+D).
    pub fn fill_down(&mut self, i: usize, r: Range) {
        let premiere = Range::new(r.start, Addr::new(r.end.col, r.start.row));
        for row in r.start.row + 1..=r.end.row {
            self.copy(i, premiere, Addr::new(r.start.col, row));
        }
    }

    /// The first column of `r` copied to the columns on its right.
    pub fn fill_right(&mut self, i: usize, r: Range) {
        let premiere = Range::new(r.start, Addr::new(r.start.col, r.end.row));
        for col in r.start.col + 1..=r.end.col {
            self.copy(i, premiere, Addr::new(col, r.start.row));
        }
    }

    /// The rows of `r` ordered by their value in column `by`.
    pub fn sort(&mut self, i: usize, r: Range, by: u32, ascending: bool, values: &Values) {
        let mut ordre: Vec<(u32, Value)> = (r.start.row..=r.end.row)
            .map(|row| (row, values.get(self, i, Addr::new(by, row))))
            .collect();
        ordre.sort_by(|(_, a), (_, b)| {
            // Empty cells last, whichever the order.
            match (a.is_empty(), b.is_empty()) {
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                _ => {
                    let o = crate::eval::compare(a, b);
                    if ascending {
                        o
                    } else {
                        o.reverse()
                    }
                }
            }
        });
        let s = &self.sheets[i];
        let lignes: Vec<Vec<Option<Cell>>> = ordre
            .iter()
            .map(|(row, _)| {
                (r.start.col..=r.end.col)
                    .map(|c| s.cells.get(&Addr::new(c, *row)).cloned())
                    .collect()
            })
            .collect();
        let s = &mut self.sheets[i];
        for (k, (ancienne, ligne)) in ordre.iter().map(|(r, _)| *r).zip(lignes).enumerate() {
            let row = r.start.row + k as u32;
            for (j, c) in ligne.into_iter().enumerate() {
                let a = Addr::new(r.start.col + j as u32, row);
                match c {
                    Some(mut c) => {
                        if let Some(f) = c.input.strip_prefix('=') {
                            c.input = format!(
                                "={}",
                                formula::shift(f, 0, i64::from(row) - i64::from(ancienne))
                            );
                        }
                        s.cells.insert(a, c);
                    }
                    None => {
                        s.cells.remove(&a);
                    }
                }
            }
        }
    }

    // --- The file ---

    pub fn from_json(text: &str) -> Result<Workbook, String> {
        let brut: Brut = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut sheets = Vec::new();
        for f in brut.sheets {
            let mut s = Sheet::new(&f.name);
            for (k, c) in f.cells {
                if let Some(a) = Addr::parse(&k) {
                    s.cells.insert(
                        a,
                        Cell {
                            input: c.v,
                            format: c.f,
                        },
                    );
                }
            }
            s.col_widths = f
                .cols
                .into_iter()
                .filter_map(|(k, w)| Some((col_index(&k)?, w.clamp(20.0, 2000.0))))
                .collect();
            s.row_heights = f
                .rows
                .into_iter()
                .filter_map(|(k, h)| {
                    Some((
                        k.parse::<u32>().ok()?.checked_sub(1)?,
                        h.clamp(14.0, 1000.0),
                    ))
                })
                .collect();
            s.frozen_rows = f.frozen.rows;
            s.frozen_cols = f.frozen.cols;
            sheets.push(s);
        }
        if sheets.is_empty() {
            sheets.push(Sheet::new("Sheet1"));
        }
        Ok(Workbook { sheets })
    }

    pub fn to_json(&self) -> String {
        let brut = Brut {
            version: 1,
            sheets: self
                .sheets
                .iter()
                .map(|s| BrutFeuille {
                    name: s.name.clone(),
                    cols: s
                        .col_widths
                        .iter()
                        .map(|(c, w)| (col_name(*c), *w))
                        .collect(),
                    rows: s
                        .row_heights
                        .iter()
                        .map(|(r, h)| ((r + 1).to_string(), *h))
                        .collect(),
                    cells: s
                        .cells
                        .iter()
                        .map(|(a, c)| {
                            (
                                a.name(),
                                BrutCellule {
                                    v: c.input.clone(),
                                    f: c.format.clone(),
                                },
                            )
                        })
                        .collect(),
                    frozen: Fige {
                        rows: s.frozen_rows,
                        cols: s.frozen_cols,
                    },
                })
                .collect(),
        };
        serde_json::to_string_pretty(&brut).unwrap_or_default()
    }
}

/// The values of a workbook's formulas, by sheet.
#[derive(Debug, Clone, Default)]
pub struct Values(pub Vec<HashMap<Addr, Value>>);

impl Values {
    /// A cell's value: its formula's, or what was typed.
    pub fn get(&self, book: &Workbook, sheet: usize, a: Addr) -> Value {
        if let Some(v) = self.0.get(sheet).and_then(|m| m.get(&a)) {
            return v.clone();
        }
        book.sheets
            .get(sheet)
            .map(|s| s.value_of_literal(a))
            .unwrap_or_default()
    }
}

#[derive(Serialize, Deserialize)]
struct Brut {
    version: u32,
    #[serde(default)]
    sheets: Vec<BrutFeuille>,
}

#[derive(Serialize, Deserialize)]
struct BrutFeuille {
    name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    cols: BTreeMap<String, f32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    rows: BTreeMap<String, f32>,
    #[serde(default)]
    cells: BTreeMap<String, BrutCellule>,
    #[serde(default)]
    frozen: Fige,
}

#[derive(Serialize, Deserialize)]
struct BrutCellule {
    #[serde(default)]
    v: String,
    #[serde(default, skip_serializing_if = "Format::is_default")]
    f: Format,
}

#[derive(Serialize, Deserialize, Default)]
struct Fige {
    #[serde(default)]
    rows: u32,
    #[serde(default)]
    cols: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::NumberFormat;

    fn a(s: &str) -> Addr {
        Addr::parse(s).unwrap()
    }

    fn classeur() -> Workbook {
        let mut b = Workbook::default();
        let s = &mut b.sheets[0];
        s.set_input(a("A1"), "Fruit");
        s.set_input(a("B1"), "Prix");
        s.set_input(a("A2"), "pomme");
        s.set_input(a("B2"), "2");
        s.set_input(a("A3"), "kiwi");
        s.set_input(a("B3"), "1,5");
        s.set_input(a("B4"), "=SUM(B2:B3)");
        s.set_format(Range::parse("B2:B4").unwrap(), |f| {
            f.number = NumberFormat::Currency
        });
        s.col_widths.insert(0, 160.0);
        s.frozen_rows = 1;
        b
    }

    #[test]
    fn the_file_round_trips() {
        let b = classeur();
        let json = b.to_json();
        assert!(json.contains("\"B4\""), "{json}");
        assert!(json.contains("=SUM(B2:B3)"));
        assert!(json.contains("\"A\": 160.0"), "{json}");
        assert_eq!(Workbook::from_json(&json).unwrap(), b);
        assert!(Workbook::from_json("{").is_err());
        assert_eq!(
            Workbook::from_json(r#"{"version":1}"#)
                .unwrap()
                .sheets
                .len(),
            1
        );
    }

    #[test]
    fn values_of_formulas_and_literals() {
        let b = classeur();
        let v = b.compute();
        assert_eq!(v.get(&b, 0, a("B4")), Value::Number(3.5));
        assert_eq!(v.get(&b, 0, a("A2")), Value::Text("pomme".into()));
        assert_eq!(v.get(&b, 0, a("Z9")), Value::Empty);
    }

    #[test]
    fn rows_inserted_move_cells_and_formulas() {
        let mut b = classeur();
        b.change(0, Change::InsertRows { at: 2, n: 1 });
        let s = &b.sheets[0];
        assert_eq!(s.input(a("A4")), "kiwi");
        assert_eq!(s.input(a("A3")), "");
        assert_eq!(s.input(a("B5")), "=SUM(B2:B4)");
        b.change(0, Change::DeleteRows { at: 1, n: 1 });
        assert_eq!(b.sheets[0].input(a("B4")), "=SUM(B2:B3)");
        b.change(0, Change::DeleteCols { at: 1, n: 1 });
        assert_eq!(b.sheets[0].input(a("A4")), "");
        assert_eq!(b.sheets[0].col_widths.get(&0), Some(&160.0));
    }

    #[test]
    fn fill_down_moves_formulas() {
        let mut b = Workbook::default();
        b.sheets[0].set_input(a("A1"), "1");
        b.sheets[0].set_input(a("B1"), "=A1*2");
        b.fill_down(0, Range::parse("B1:B3").unwrap());
        assert_eq!(b.sheets[0].input(a("B3")), "=A3*2");
        b.fill_right(0, Range::parse("A1:C1").unwrap());
        assert_eq!(b.sheets[0].input(a("C1")), "1");
    }

    #[test]
    fn sorting_rows_by_a_column() {
        let mut b = classeur();
        let v = b.compute();
        b.sort(0, Range::parse("A2:B3").unwrap(), 1, true, &v);
        assert_eq!(b.sheets[0].input(a("A2")), "kiwi");
        assert_eq!(b.sheets[0].input(a("A3")), "pomme");
        let v = b.compute();
        b.sort(0, Range::parse("A2:B3").unwrap(), 0, false, &v);
        assert_eq!(b.sheets[0].input(a("A2")), "pomme");
    }

    #[test]
    fn a_renamed_sheet_keeps_its_readers() {
        let mut b = classeur();
        b.sheets.push(Sheet::new("Summary"));
        b.sheets[1].set_input(a("A1"), "=Sheet1!B4*2");
        assert!(b.rename_sheet(0, "Fruits du marché"));
        assert_eq!(b.sheets[1].input(a("A1")), "='Fruits du marché'!B4*2");
        assert_eq!(b.compute().get(&b, 1, a("A1")), Value::Number(7.0));
        assert!(!b.rename_sheet(0, "summary"));
        assert!(!b.rename_sheet(0, "a/b"));
        assert_eq!(b.new_sheet_name(), "Sheet1");
    }

    #[test]
    fn empty_cells_without_format_are_not_kept() {
        let mut s = Sheet::new("S");
        s.set_input(a("A1"), "x");
        s.set_input(a("A1"), "");
        assert!(s.cells.is_empty());
        s.set_format(Range::single(a("A1")), |f| f.bold = true);
        s.clear(Range::single(a("A1")));
        assert_eq!(s.cells.len(), 1);
        s.set_format(Range::single(a("A1")), |f| f.bold = false);
        assert!(s.cells.is_empty());
    }
}
