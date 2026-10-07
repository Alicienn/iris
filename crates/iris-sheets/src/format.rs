//! How a cell looks: its type, colours, alignment, and how its number is written.

use crate::eval::{general, serial_date, serial_datetime, Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    /// Numbers on the right, text on the left.
    #[default]
    Auto,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NumberFormat {
    #[default]
    General,
    Number,
    Percent,
    Currency,
    Date,
    Time,
    DateTime,
    /// Shown as typed, never read as a number.
    Text,
}

fn faux(b: &bool) -> bool {
    !*b
}

fn auto(a: &Align) -> bool {
    *a == Align::Auto
}

fn general_fmt(n: &NumberFormat) -> bool {
    *n == NumberFormat::General
}

/// A cell's format. What is left as it starts is not written in the file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Format {
    #[serde(skip_serializing_if = "faux")]
    pub bold: bool,
    #[serde(skip_serializing_if = "faux")]
    pub italic: bool,
    #[serde(skip_serializing_if = "faux")]
    pub underline: bool,
    #[serde(skip_serializing_if = "faux")]
    pub strike: bool,
    #[serde(skip_serializing_if = "faux")]
    pub wrap: bool,
    /// A line around the cell.
    #[serde(skip_serializing_if = "faux")]
    pub border: bool,
    /// The text's colour, `#rrggbb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The cell's colour, `#rrggbb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(skip_serializing_if = "auto")]
    pub align: Align,
    #[serde(skip_serializing_if = "general_fmt")]
    pub number: NumberFormat,
    /// Decimals shown; each format has its own when none is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
}

impl Format {
    pub fn is_default(&self) -> bool {
        *self == Format::default()
    }

    /// The decimals shown: the cell's, else its format's.
    pub fn decimals(&self) -> u8 {
        self.decimals.unwrap_or(match self.number {
            NumberFormat::Number | NumberFormat::Currency => 2,
            _ => 0,
        })
    }
}

/// How numbers and dates are written where the user is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locale {
    pub decimal: char,
    /// Between thousands.
    pub thousands: char,
    /// 07/10/2026 rather than 2026-10-07.
    pub day_first: bool,
    pub currency: String,
    /// `12,50 €` rather than `€12.50`.
    pub currency_after: bool,
}

impl Locale {
    /// `1 234,50 €`, `07/10/2026`.
    pub fn french() -> Self {
        Self {
            decimal: ',',
            thousands: '\u{202f}',
            day_first: true,
            currency: "€".into(),
            currency_after: true,
        }
    }

    /// `€1,234.50`, `2026-10-07`.
    pub fn english() -> Self {
        Self {
            decimal: '.',
            thousands: ',',
            day_first: false,
            currency: "€".into(),
            currency_after: false,
        }
    }
}

impl Default for Locale {
    fn default() -> Self {
        Locale::english()
    }
}

/// `n` with `decimales` decimals and the thousands separated.
fn fixe(n: f64, decimales: u8, l: &Locale) -> String {
    let s = format!("{:.*}", decimales as usize, n.abs());
    // No minus before a number that rounds to nothing.
    let negatif = n < 0.0 && s.chars().any(|c| c.is_ascii_digit() && c != '0');
    let (entiers, fraction) = match s.split_once('.') {
        Some((e, f)) => (e.to_string(), Some(f.to_string())),
        None => (s, None),
    };
    let mut groupe = String::new();
    for (i, c) in entiers.chars().enumerate() {
        if i > 0 && (entiers.len() - i) % 3 == 0 {
            groupe.push(l.thousands);
        }
        groupe.push(c);
    }
    let mut sortie = String::new();
    if negatif {
        sortie.push('-');
    }
    sortie.push_str(&groupe);
    if let Some(f) = fraction {
        sortie.push(l.decimal);
        sortie.push_str(&f);
    }
    sortie
}

/// The text a cell shows.
pub fn display(v: &Value, f: &Format, l: &Locale) -> String {
    let n = match v {
        Value::Empty => return String::new(),
        Value::Text(t) => return t.clone(),
        Value::Bool(b) => return if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Error(e) => return e.text().into(),
        Value::Number(n) => *n,
    };
    let d = f.decimals();
    match f.number {
        NumberFormat::General | NumberFormat::Text => match f.decimals {
            Some(d) => fixe(n, d, l),
            None => general(n).replace('.', &l.decimal.to_string()),
        },
        NumberFormat::Number => fixe(n, d, l),
        NumberFormat::Percent => format!("{}%", fixe(n * 100.0, d, l)),
        NumberFormat::Currency => {
            let corps = fixe(n.abs(), d, l);
            let signe = if n < 0.0 && corps.chars().any(|c| c.is_ascii_digit() && c != '0') {
                "-"
            } else {
                ""
            };
            if l.currency_after {
                format!("{signe}{corps}\u{a0}{}", l.currency)
            } else {
                format!("{signe}{}{corps}", l.currency)
            }
        }
        NumberFormat::Date => match serial_date(n) {
            Some(d) if l.day_first => d.format("%d/%m/%Y").to_string(),
            Some(d) => d.format("%Y-%m-%d").to_string(),
            None => "#####".into(),
        },
        NumberFormat::Time => match serial_datetime(n.rem_euclid(1.0)) {
            Some(t) => t.format("%H:%M").to_string(),
            None => "#####".into(),
        },
        NumberFormat::DateTime => match serial_datetime(n) {
            Some(t) if l.day_first => t.format("%d/%m/%Y %H:%M").to_string(),
            Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
            None => "#####".into(),
        },
    }
}

/// Where a value sits in its cell when the format does not say.
pub fn align_of(v: &Value, f: &Format) -> Align {
    match (f.align, v) {
        (Align::Auto, Value::Number(_)) => Align::Right,
        (Align::Auto, Value::Bool(_) | Value::Error(_)) => Align::Center,
        (Align::Auto, _) => Align::Left,
        (a, _) => a,
    }
}

/// Reads a number as people type it — `3.5`, `3,5`, `1 234,5`, `1,234.5`, `12%`,
/// `12 €`, `-4e3` — and the format it suggests.
pub fn read_number(t: &str) -> Option<(f64, Option<NumberFormat>)> {
    let mut s: String = t
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\u{2009}' | '\''))
        .collect();
    let mut format = None;
    if let Some(r) = s.strip_suffix('%') {
        s = r.to_string();
        format = Some(NumberFormat::Percent);
    } else if let Some(r) = s
        .strip_suffix('€')
        .or_else(|| s.strip_prefix('€'))
        .or_else(|| s.strip_prefix('$'))
        .or_else(|| s.strip_suffix('$'))
    {
        s = r.to_string();
        format = Some(NumberFormat::Currency);
    }
    let (negatif, corps) = match s.strip_prefix('-') {
        Some(r) => (true, r.to_string()),
        None => (false, s.strip_prefix('+').unwrap_or(&s).to_string()),
    };
    if corps.is_empty() || !corps.starts_with(|c: char| c.is_ascii_digit() || c == '.' || c == ',')
    {
        return None;
    }
    let virgules = corps.matches(',').count();
    let points = corps.matches('.').count();
    let normal = match (virgules, points) {
        (0, _) => corps.clone(),
        // `1,234.5`: commas between thousands.
        (_, 1) if corps.rfind(',') < corps.rfind('.') => corps.replace(',', ""),
        // `1.234,5`: points between thousands.
        (1, _) if corps.rfind('.') < corps.rfind(',') => corps.replace('.', "").replace(',', "."),
        // `3,5`: a decimal comma.
        (1, 0) => corps.replace(',', "."),
        // `1,234,567`.
        (_, 0) if corps.split(',').skip(1).all(|g| g.len() == 3) => corps.replace(',', ""),
        _ => return None,
    };
    if normal.contains(['e', 'E']) && format.is_some() {
        return None;
    }
    let n: f64 = normal.parse().ok()?;
    if !n.is_finite() {
        return None;
    }
    let n = if negatif { -n } else { n };
    Some((
        if format == Some(NumberFormat::Percent) {
            n / 100.0
        } else {
            n
        },
        format,
    ))
}

/// Reads a date (`07/10/2026`, `7/10/26`, `2026-10-07`) or a time (`14:30`) as typed:
/// its serial number and its format.
pub fn read_date(t: &str, l: &Locale) -> Option<(f64, NumberFormat)> {
    use chrono::{NaiveDate, NaiveTime};
    let t = t.trim();
    if let Ok(h) = NaiveTime::parse_from_str(t, "%H:%M") {
        return Some((
            f64::from(chrono::Timelike::num_seconds_from_midnight(&h)) / 86_400.0,
            NumberFormat::Time,
        ));
    }
    if let Ok(d) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        return Some((crate::eval::date_serial(d), NumberFormat::Date));
    }
    let parts: Vec<&str> = t.split(['/', '.']).collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let nombres: Vec<i32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    let (j, m) = if l.day_first {
        (nombres[0], nombres[1])
    } else {
        (nombres[1], nombres[0])
    };
    let a = if parts[2].len() <= 2 {
        2000 + nombres[2]
    } else {
        nombres[2]
    };
    let d = NaiveDate::from_ymd_opt(a, m as u32, j as u32)?;
    Some((crate::eval::date_serial(d), NumberFormat::Date))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avec(number: NumberFormat, decimals: Option<u8>) -> Format {
        Format {
            number,
            decimals,
            ..Format::default()
        }
    }

    #[test]
    fn numbers_written_both_ways() {
        let (fr, en) = (Locale::french(), Locale::english());
        let n = Value::Number(-1234.5);
        assert_eq!(display(&n, &Format::default(), &fr), "-1234,5");
        assert_eq!(
            display(&n, &avec(NumberFormat::Number, None), &en),
            "-1,234.50"
        );
        assert_eq!(
            display(&n, &avec(NumberFormat::Number, None), &fr),
            "-1\u{202f}234,50"
        );
        assert_eq!(
            display(
                &Value::Number(12.5),
                &avec(NumberFormat::Currency, None),
                &fr
            ),
            "12,50\u{a0}€"
        );
        assert_eq!(
            display(
                &Value::Number(-12.6),
                &avec(NumberFormat::Currency, Some(0)),
                &en
            ),
            "-€13"
        );
        assert_eq!(
            display(
                &Value::Number(0.125),
                &avec(NumberFormat::Percent, Some(1)),
                &en
            ),
            "12.5%"
        );
        assert_eq!(
            display(
                &Value::Number(-0.001),
                &avec(NumberFormat::Number, Some(2)),
                &en
            ),
            "0.00"
        );
        assert_eq!(
            display(
                &Value::Number(46302.75),
                &avec(NumberFormat::DateTime, None),
                &fr
            ),
            "07/10/2026 18:00"
        );
        assert_eq!(
            display(
                &Value::Number(46302.0),
                &avec(NumberFormat::Date, None),
                &en
            ),
            "2026-10-07"
        );
        assert_eq!(display(&Value::Bool(true), &Format::default(), &en), "TRUE");
    }

    #[test]
    fn numbers_read_as_typed() {
        assert_eq!(read_number("3,5"), Some((3.5, None)));
        assert_eq!(read_number("1 234,5"), Some((1234.5, None)));
        assert_eq!(read_number("1.234,5"), Some((1234.5, None)));
        assert_eq!(read_number("1,234.5"), Some((1234.5, None)));
        assert_eq!(read_number("1,234,567"), Some((1_234_567.0, None)));
        assert_eq!(
            read_number("-12 %"),
            Some((-0.12, Some(NumberFormat::Percent)))
        );
        assert_eq!(
            read_number("12,50 €"),
            Some((12.5, Some(NumberFormat::Currency)))
        );
        assert_eq!(read_number("-4e3"), Some((-4000.0, None)));
        assert_eq!(read_number("abc"), None);
        assert_eq!(read_number("1,2,3"), None);
        assert_eq!(read_number("e5"), None);
        assert_eq!(read_number("inf"), None);
    }

    #[test]
    fn dates_read_as_typed() {
        let fr = Locale::french();
        assert_eq!(
            read_date("07/10/2026", &fr),
            Some((46302.0, NumberFormat::Date))
        );
        assert_eq!(
            read_date("7/10/26", &fr),
            Some((46302.0, NumberFormat::Date))
        );
        assert_eq!(
            read_date("10/07/2026", &Locale::english()),
            Some((46302.0, NumberFormat::Date))
        );
        assert_eq!(
            read_date("2026-10-07", &fr),
            Some((46302.0, NumberFormat::Date))
        );
        assert_eq!(read_date("12:00", &fr), Some((0.5, NumberFormat::Time)));
        assert_eq!(read_date("31/02/2026", &fr), None);
        assert_eq!(read_date("3.5", &fr), None);
    }

    #[test]
    fn a_default_format_is_not_written() {
        assert_eq!(serde_json::to_string(&Format::default()).unwrap(), "{}");
        let f = Format {
            bold: true,
            number: NumberFormat::Percent,
            ..Format::default()
        };
        let s = serde_json::to_string(&f).unwrap();
        assert_eq!(s, r#"{"bold":true,"number":"percent"}"#);
        assert_eq!(serde_json::from_str::<Format>(&s).unwrap(), f);
    }
}
