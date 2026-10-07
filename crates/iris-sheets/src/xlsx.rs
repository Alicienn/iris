//! Excel files: read with `calamine`, written with `rust_xlsxwriter`, from and to bytes.
//!
//! What is kept both ways: the sheets and their names, values, formulas (with their
//! last value, for programs that do not compute), dates, column widths, the frozen top
//! rows, and the formats Iris has — bold, italic, underline, strikethrough, colours,
//! alignment, wrap, borders, number formats.

use crate::cell::Addr;
use crate::eval::Value;
use crate::file::{Sheet, Values, Workbook, COL_WIDTH};
use crate::format::{Align, Format, Locale, NumberFormat};

/// Excel's width unit (a character) in pixels, roughly.
const PX_PAR_CARACTERE: f32 = 7.0;

/// A workbook read from an `.xlsx` (or `.xls`, `.ods`) file's bytes.
pub fn read(bytes: &[u8]) -> Result<Workbook, String> {
    use calamine::{open_workbook_auto_from_rs, Data, Reader};
    let mut classeur = open_workbook_auto_from_rs(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|e| e.to_string())?;
    let mut sheets = Vec::new();
    for nom in classeur.sheet_names() {
        let mut s = Sheet::new(&nom);
        if let Ok(plage) = classeur.worksheet_range(&nom) {
            let (r0, c0) = plage.start().unwrap_or((0, 0));
            for (r, c, v) in plage.used_cells() {
                let a = Addr::new(c0 + c as u32, r0 + r as u32);
                let (texte, format) = match v {
                    Data::Int(n) => (n.to_string(), None),
                    Data::Float(f) => (crate::eval::general(*f), None),
                    Data::String(t) => (t.clone(), None),
                    Data::Bool(b) => (if *b { "TRUE" } else { "FALSE" }.to_string(), None),
                    Data::DateTime(d) => {
                        let n = d.as_f64();
                        let f = if d.is_duration() || n < 1.0 {
                            NumberFormat::Time
                        } else if n.fract() == 0.0 {
                            NumberFormat::Date
                        } else {
                            NumberFormat::DateTime
                        };
                        (crate::eval::general(n), Some(f))
                    }
                    Data::DateTimeIso(t) | Data::DurationIso(t) => (t.clone(), None),
                    Data::Error(e) => (format!("{e}"), None),
                    Data::Empty => continue,
                };
                s.set_input(a, &texte);
                if let Some(f) = format {
                    s.set_format(crate::cell::Range::single(a), |x| x.number = f);
                }
            }
        }
        if let Ok(formules) = classeur.worksheet_formula(&nom) {
            let (r0, c0) = formules.start().unwrap_or((0, 0));
            for (r, c, f) in formules.used_cells() {
                if !f.is_empty() {
                    let a = Addr::new(c0 + c as u32, r0 + r as u32);
                    s.set_input(a, &format!("={}", f.trim_start_matches('=')));
                }
            }
        }
        sheets.push(s);
    }
    if sheets.is_empty() {
        return Err("this file has no sheet".into());
    }
    Ok(Workbook { sheets })
}

fn couleur(hex: &str) -> Option<rust_xlsxwriter::Color> {
    let h = hex.trim_start_matches('#');
    (h.len() == 6)
        .then(|| u32::from_str_radix(h, 16).ok())
        .flatten()
        .map(rust_xlsxwriter::Color::RGB)
}

/// The Excel number format of a cell.
fn format_nombre(f: &Format, l: &Locale) -> Option<String> {
    let d = f.decimals() as usize;
    let decimales = if d == 0 {
        String::new()
    } else {
        format!(".{}", "0".repeat(d))
    };
    let date = if l.day_first {
        "dd/mm/yyyy"
    } else {
        "yyyy-mm-dd"
    };
    Some(match f.number {
        NumberFormat::General => match f.decimals {
            Some(_) => format!("0{decimales}"),
            None => return None,
        },
        NumberFormat::Number => format!("#,##0{decimales}"),
        NumberFormat::Percent => format!("0{decimales}%"),
        NumberFormat::Currency if l.currency_after => {
            format!("#,##0{decimales} \"{}\"", l.currency)
        }
        NumberFormat::Currency => format!("\"{}\"#,##0{decimales}", l.currency),
        NumberFormat::Date => date.to_string(),
        NumberFormat::Time => "hh:mm".to_string(),
        NumberFormat::DateTime => format!("{date} hh:mm"),
        NumberFormat::Text => "@".to_string(),
    })
}

fn format_excel(f: &Format, l: &Locale) -> rust_xlsxwriter::Format {
    use rust_xlsxwriter::{FormatAlign, FormatBorder, FormatUnderline};
    let mut x = rust_xlsxwriter::Format::new();
    if f.bold {
        x = x.set_bold();
    }
    if f.italic {
        x = x.set_italic();
    }
    if f.underline {
        x = x.set_underline(FormatUnderline::Single);
    }
    if f.strike {
        x = x.set_font_strikethrough();
    }
    if f.wrap {
        x = x.set_text_wrap();
    }
    if f.border {
        x = x.set_border(FormatBorder::Thin);
    }
    if let Some(c) = f.color.as_deref().and_then(couleur) {
        x = x.set_font_color(c);
    }
    if let Some(c) = f.fill.as_deref().and_then(couleur) {
        x = x.set_background_color(c);
    }
    x = match f.align {
        Align::Auto => x,
        Align::Left => x.set_align(FormatAlign::Left),
        Align::Center => x.set_align(FormatAlign::Center),
        Align::Right => x.set_align(FormatAlign::Right),
    };
    if let Some(n) = format_nombre(f, l) {
        x = x.set_num_format(n);
    }
    x
}

/// A workbook as an `.xlsx` file's bytes. `values`: what its formulas gave, kept beside
/// them.
pub fn write(book: &Workbook, values: &Values, l: &Locale) -> Result<Vec<u8>, String> {
    let mut sortie = rust_xlsxwriter::Workbook::new();
    let erreur = |e: rust_xlsxwriter::XlsxError| e.to_string();
    for (i, s) in book.sheets.iter().enumerate() {
        let ws = sortie.add_worksheet();
        ws.set_name(s.name.as_str()).map_err(erreur)?;
        for (c, w) in &s.col_widths {
            if *c < 16_384 {
                ws.set_column_width(
                    *c as u16,
                    f64::from(((w - 5.0) / PX_PAR_CARACTERE).max(1.0)),
                )
                .map_err(erreur)?;
            }
        }
        for (r, h) in &s.row_heights {
            // Points, at 96 pixels per inch.
            ws.set_row_height(*r, f64::from(h * 0.75)).map_err(erreur)?;
        }
        if s.frozen_rows > 0 || s.frozen_cols > 0 {
            ws.set_freeze_panes(s.frozen_rows, s.frozen_cols.min(16_383) as u16)
                .map_err(erreur)?;
        }
        for (a, cellule) in &s.cells {
            if a.col >= 16_384 {
                continue;
            }
            let (r, c) = (a.row, a.col as u16);
            let f = format_excel(&cellule.format, l);
            if cellule.input.starts_with('=') {
                let v = values.get(book, i, *a);
                let resultat = match &v {
                    Value::Number(n) => crate::eval::general(*n),
                    Value::Error(e) => e.text().to_string(),
                    v => v.text().unwrap_or_default(),
                };
                // Excel separates arguments with commas.
                let formule = virgules(&cellule.input);
                ws.write_formula_with_format(
                    r,
                    c,
                    rust_xlsxwriter::Formula::new(formule).set_result(resultat),
                    &f,
                )
                .map_err(erreur)?;
                continue;
            }
            match s.value_of_literal(*a) {
                Value::Number(n) => ws.write_number_with_format(r, c, n, &f),
                Value::Bool(b) => ws.write_boolean_with_format(r, c, b, &f),
                Value::Empty => ws.write_blank(r, c, &f),
                _ => ws.write_string_with_format(r, c, cellule.input.as_str(), &f),
            }
            .map_err(erreur)?;
        }
        if s.col_widths.is_empty() {
            let (cols, _) = s.extent();
            for c in 0..cols.min(16_384) {
                ws.set_column_width(c as u16, f64::from((COL_WIDTH - 5.0) / PX_PAR_CARACTERE))
                    .map_err(erreur)?;
            }
        }
    }
    sortie.save_to_buffer().map_err(erreur)
}

/// A formula with `;` between arguments rewritten with `,`, outside texts.
fn virgules(formule: &str) -> String {
    let mut sortie = String::with_capacity(formule.len());
    let mut texte = false;
    for c in formule.chars() {
        match c {
            '"' => {
                texte = !texte;
                sortie.push(c);
            }
            ';' if !texte => sortie.push(','),
            c => sortie.push(c),
        }
    }
    sortie
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Range;

    #[test]
    fn a_workbook_survives_excel() {
        let mut b = Workbook::default();
        b.sheets[0].name = "Notes".into();
        let s = &mut b.sheets[0];
        s.set_input(Addr::new(0, 0), "Élève");
        s.set_input(Addr::new(1, 0), "Note");
        s.set_input(Addr::new(0, 1), "Léa");
        s.set_input(Addr::new(1, 1), "15,5");
        s.set_input(Addr::new(0, 2), "Tom");
        s.set_input(Addr::new(1, 2), "12");
        s.set_input(Addr::new(1, 3), "=AVERAGE(B2:B3; 20)");
        s.set_input(Addr::new(2, 1), "46302");
        s.set_format(Range::single(Addr::new(2, 1)), |f| {
            f.number = NumberFormat::Date
        });
        s.set_format(Range::parse("A1:B1").unwrap(), |f| f.bold = true);
        s.col_widths.insert(0, 180.0);
        s.frozen_rows = 1;
        b.sheets.push(Sheet::new("Vide"));
        let v = b.compute();
        let octets = write(&b, &v, &Locale::french()).unwrap();
        assert!(octets.starts_with(b"PK"), "a zip");

        let lu = read(&octets).unwrap();
        assert_eq!(lu.sheets.len(), 2);
        assert_eq!(lu.sheets[0].name, "Notes");
        let s = &lu.sheets[0];
        assert_eq!(s.input(Addr::new(0, 1)), "Léa");
        assert_eq!(s.input(Addr::new(1, 1)), "15.5");
        assert_eq!(s.input(Addr::new(1, 3)), "=AVERAGE(B2:B3, 20)");
        let v = lu.compute();
        assert_eq!(
            v.get(&lu, 0, Addr::new(1, 3)),
            Value::Number(15.833333333333334)
        );
        assert!(read(b"not a spreadsheet").is_err());
    }

    #[test]
    fn semicolons_become_commas_outside_texts() {
        assert_eq!(virgules("=IF(A1;\"a;b\";2)"), "=IF(A1,\"a;b\",2)");
    }

    #[test]
    fn number_formats_for_excel() {
        let fr = Locale::french();
        let f = |number, decimals| Format {
            number,
            decimals,
            ..Format::default()
        };
        assert_eq!(format_nombre(&Format::default(), &fr), None);
        assert_eq!(
            format_nombre(&f(NumberFormat::Number, None), &fr).unwrap(),
            "#,##0.00"
        );
        assert_eq!(
            format_nombre(&f(NumberFormat::Percent, Some(1)), &fr).unwrap(),
            "0.0%"
        );
        assert_eq!(
            format_nombre(&f(NumberFormat::Currency, None), &fr).unwrap(),
            "#,##0.00 \"€\""
        );
        assert_eq!(
            format_nombre(&f(NumberFormat::Date, None), &fr).unwrap(),
            "dd/mm/yyyy"
        );
    }
}
