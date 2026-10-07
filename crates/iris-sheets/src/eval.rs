//! Computing a workbook: every formula once, after the cells it reads.
//!
//! The formulas are put in the order they depend on each other (a formula reading a
//! range depends on the formulas inside it), then computed in that order, each reading
//! only values already known. Formulas that depend on themselves, directly or not, are
//! `#CYCLE!`, and so is everything that reads them. Nothing recurses: a column of ten
//! thousand formulas each adding one to the one above is computed like any other.

use crate::cell::{Addr, Range};
use crate::file::Workbook;
use crate::formula::{BinOp, Expr};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use std::collections::{BTreeMap, HashMap, VecDeque};

/// What a formula can fail with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CellError {
    Div0,
    Ref,
    Name,
    Value,
    Num,
    NA,
    Cycle,
    /// The formula could not be read.
    Parse,
}

impl CellError {
    pub const ALL: [CellError; 8] = [
        CellError::Div0,
        CellError::Ref,
        CellError::Name,
        CellError::Value,
        CellError::Num,
        CellError::NA,
        CellError::Cycle,
        CellError::Parse,
    ];

    pub fn text(&self) -> &'static str {
        match self {
            CellError::Div0 => "#DIV/0!",
            CellError::Ref => "#REF!",
            CellError::Name => "#NAME?",
            CellError::Value => "#VALUE!",
            CellError::Num => "#NUM!",
            CellError::NA => "#N/A",
            CellError::Cycle => "#CYCLE!",
            CellError::Parse => "#ERROR!",
        }
    }
}

/// A cell's value.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(CellError),
}

impl Value {
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }

    /// As a number: `TRUE` is 1, nothing is 0, a text must read as one.
    pub fn number(&self) -> Result<f64, CellError> {
        match self {
            Value::Empty => Ok(0.0),
            Value::Number(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Value::Text(t) => {
                let t = t.trim();
                if t.is_empty() {
                    return Ok(0.0);
                }
                if let Some(p) = t.strip_suffix('%') {
                    return p
                        .trim()
                        .parse::<f64>()
                        .map(|n| n / 100.0)
                        .map_err(|_| CellError::Value);
                }
                t.parse().map_err(|_| CellError::Value)
            }
            Value::Error(e) => Err(*e),
        }
    }

    /// As a text: numbers as the general format writes them.
    pub fn text(&self) -> Result<String, CellError> {
        match self {
            Value::Empty => Ok(String::new()),
            Value::Number(n) => Ok(general(*n)),
            Value::Bool(b) => Ok(if *b { "TRUE" } else { "FALSE" }.into()),
            Value::Text(t) => Ok(t.clone()),
            Value::Error(e) => Err(*e),
        }
    }

    pub fn truth(&self) -> Result<bool, CellError> {
        match self {
            Value::Bool(b) => Ok(*b),
            Value::Text(t) if t.eq_ignore_ascii_case("true") => Ok(true),
            Value::Text(t) if t.eq_ignore_ascii_case("false") => Ok(false),
            Value::Text(_) => Err(CellError::Value),
            v => v.number().map(|n| n != 0.0),
        }
    }
}

/// A number as the general format writes it: up to ten significant digits, no
/// trailing zeros, `.` before decimals.
pub fn general(n: f64) -> String {
    if !n.is_finite() {
        return "#NUM!".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    let a = n.abs();
    if (1e-9..1e15).contains(&a) {
        if n.fract() == 0.0 {
            return format!("{n:.0}");
        }
        let entiers = a.log10().floor() as i32 + 1;
        let decimales = (10 - entiers).clamp(0, 15) as usize;
        let s = format!("{n:.decimales$}");
        let s = if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        };
        if s == "-0" {
            "0".into()
        } else {
            s
        }
    } else {
        let s = format!("{n:.5E}");
        // 1.23450E7 → 1.2345E+7
        match s.split_once('E') {
            Some((m, e)) => {
                let m = m.trim_end_matches('0').trim_end_matches('.');
                let e: i32 = e.parse().unwrap_or(0);
                format!("{m}E{}{}", if e < 0 { "-" } else { "+" }, e.abs())
            }
            None => s,
        }
    }
}

// --- Dates as numbers --------------------------------------------------------------------

fn epoque() -> NaiveDate {
    NaiveDate::from_ymd_opt(1899, 12, 30).expect("a date")
}

/// A date as spreadsheets count it: days since 30 December 1899.
pub fn date_serial(d: NaiveDate) -> f64 {
    (d - epoque()).num_days() as f64
}

pub fn datetime_serial(d: NaiveDateTime) -> f64 {
    date_serial(d.date()) + f64::from(d.time().num_seconds_from_midnight()) / 86_400.0
}

/// The date of a serial number.
pub fn serial_date(n: f64) -> Option<NaiveDate> {
    if !(0.0..3_000_000.0).contains(&n) {
        return None;
    }
    epoque().checked_add_signed(chrono::Duration::days(n.floor() as i64))
}

pub fn serial_datetime(n: f64) -> Option<NaiveDateTime> {
    let d = serial_date(n)?;
    let secondes = ((n - n.floor()) * 86_400.0).round() as u32;
    d.and_hms_opt(0, 0, 0)
        .map(|t| t + chrono::Duration::seconds(i64::from(secondes.min(86_399))))
}

// --- Computing ---------------------------------------------------------------------------

/// The cells a computation sees.
struct Grid<'a> {
    book: &'a Workbook,
    /// Formula values known so far, by sheet.
    values: &'a [HashMap<Addr, Value>],
    /// The sheet of the formula being computed.
    home: usize,
    now: NaiveDateTime,
}

/// What an argument gives: one value, or a rectangle of them.
enum Arg {
    One(Value),
    Many { width: u32, values: Vec<Value> },
}

impl Arg {
    fn single(self) -> Value {
        match self {
            Arg::One(v) => v,
            // A range where one value is expected: its only cell, else #VALUE!.
            Arg::Many { values, .. } if values.len() == 1 => {
                values.into_iter().next().unwrap_or_default()
            }
            Arg::Many { .. } => Value::Error(CellError::Value),
        }
    }
}

/// The most cells a range may have.
const MAX_RANGE: u64 = 2_000_000;

impl Grid<'_> {
    fn sheet_index(&self, name: &Option<String>) -> Option<usize> {
        match name {
            None => Some(self.home),
            Some(n) => self
                .book
                .sheets
                .iter()
                .position(|s| s.name.eq_ignore_ascii_case(n)),
        }
    }

    fn cell(&self, sheet: usize, a: Addr) -> Value {
        if let Some(v) = self.values[sheet].get(&a) {
            return v.clone();
        }
        self.book.sheets[sheet].value_of_literal(a)
    }

    fn range(&self, sheet: usize, r: Range) -> Arg {
        if r.len() > MAX_RANGE {
            return Arg::One(Value::Error(CellError::Ref));
        }
        Arg::Many {
            width: r.width(),
            values: r.cells().map(|a| self.cell(sheet, a)).collect(),
        }
    }

    fn arg(&self, e: &Expr) -> Arg {
        match e {
            Expr::Range { sheet, range } => match self.sheet_index(sheet) {
                Some(s) => self.range(s, *range),
                None => Arg::One(Value::Error(CellError::Ref)),
            },
            e => Arg::One(self.eval(e)),
        }
    }

    fn eval(&self, e: &Expr) -> Value {
        match e {
            Expr::Number(n) => Value::Number(*n),
            Expr::Text(t) => Value::Text(t.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Error(e) => Value::Error(*e),
            Expr::Ref { sheet, addr } => match self.sheet_index(sheet) {
                Some(s) => self.cell(s, *addr),
                None => Value::Error(CellError::Ref),
            },
            Expr::Range { .. } => self.arg(e).single(),
            Expr::Neg(x) => num(self.eval(x).number().map(|n| -n)),
            Expr::Percent(x) => num(self.eval(x).number().map(|n| n / 100.0)),
            Expr::Bin(op, a, b) => {
                let (a, b) = (self.eval(a), self.eval(b));
                binaire(*op, a, b)
            }
            Expr::Call(nom, args) => self.call(nom, args),
        }
    }

    /// Every value of the arguments, ranges opened.
    fn flat(&self, args: &[Expr]) -> Vec<(Value, bool)> {
        let mut sortie = Vec::new();
        for a in args {
            match self.arg(a) {
                Arg::One(v) => sortie.push((v, true)),
                Arg::Many { values, .. } => sortie.extend(values.into_iter().map(|v| (v, false))),
            }
        }
        sortie
    }

    /// The numbers of the arguments: typed ones read as numbers, in ranges only numbers.
    fn numbers(&self, args: &[Expr]) -> Result<Vec<f64>, CellError> {
        let mut sortie = Vec::new();
        for (v, direct) in self.flat(args) {
            match v {
                Value::Error(e) => return Err(e),
                Value::Number(n) => sortie.push(n),
                v if direct && !v.is_empty() => sortie.push(v.number()?),
                _ => {}
            }
        }
        Ok(sortie)
    }

    fn call(&self, nom: &str, args: &[Expr]) -> Value {
        let n = args.len();
        let un = |i: usize| self.eval(&args[i]);
        let nombre = |i: usize| un(i).number();
        let compte = |min: usize, max: usize| (min..=max).contains(&n);
        macro_rules! essayer {
            ($e:expr) => {
                match $e {
                    Ok(v) => v,
                    Err(e) => return Value::Error(e),
                }
            };
        }
        match nom {
            "SUM" => num(self.numbers(args).map(|v| v.iter().sum())),
            "PRODUCT" => num(self.numbers(args).map(|v| v.iter().product())),
            "AVERAGE" => {
                let v = essayer!(self.numbers(args));
                if v.is_empty() {
                    Value::Error(CellError::Div0)
                } else {
                    Value::Number(v.iter().sum::<f64>() / v.len() as f64)
                }
            }
            "MIN" | "MAX" => {
                let v = essayer!(self.numbers(args));
                let r = if nom == "MIN" {
                    v.iter().copied().fold(f64::INFINITY, f64::min)
                } else {
                    v.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                };
                Value::Number(if v.is_empty() { 0.0 } else { r })
            }
            "MEDIAN" => {
                let mut v = essayer!(self.numbers(args));
                if v.is_empty() {
                    return Value::Error(CellError::Num);
                }
                v.sort_by(f64::total_cmp);
                let m = v.len() / 2;
                Value::Number(if v.len() % 2 == 0 {
                    (v[m - 1] + v[m]) / 2.0
                } else {
                    v[m]
                })
            }
            "COUNT" => Value::Number(
                self.flat(args)
                    .iter()
                    .filter(|(v, direct)| {
                        matches!(v, Value::Number(_))
                            || (*direct && v.number().is_ok() && !v.is_empty())
                    })
                    .count() as f64,
            ),
            "COUNTA" => Value::Number(
                self.flat(args)
                    .iter()
                    .filter(|(v, _)| !v.is_empty())
                    .count() as f64,
            ),
            "COUNTBLANK" => Value::Number(
                self.flat(args)
                    .iter()
                    .filter(|(v, _)| v.is_empty() || *v == Value::Text(String::new()))
                    .count() as f64,
            ),
            "IF" if compte(2, 3) => {
                if essayer!(un(0).truth()) {
                    un(1)
                } else if n == 3 {
                    un(2)
                } else {
                    Value::Bool(false)
                }
            }
            "IFERROR" if n == 2 => match un(0) {
                Value::Error(_) => un(1),
                v => v,
            },
            "AND" | "OR" if n >= 1 => {
                let mut r = nom == "AND";
                for (v, direct) in self.flat(args) {
                    if v.is_empty() || (!direct && matches!(v, Value::Text(_))) {
                        continue;
                    }
                    let b = essayer!(v.truth());
                    if nom == "AND" {
                        r &= b;
                    } else {
                        r |= b;
                    }
                }
                Value::Bool(r)
            }
            "NOT" if n == 1 => num_bool(un(0).truth().map(|b| !b)),
            "ROUND" | "ROUNDUP" | "ROUNDDOWN" if compte(1, 2) => {
                let x = essayer!(nombre(0));
                let d = if n == 2 {
                    essayer!(nombre(1)) as i32
                } else {
                    0
                };
                let f = 10f64.powi(d);
                let y = x * f;
                // Rounded as written, not as stored: 2.675 → 2.68.
                let y = (y * 1e9).round() / 1e9;
                Value::Number(
                    match nom {
                        "ROUND" => y.abs().round().copysign(y),
                        "ROUNDUP" => y.abs().ceil().copysign(y),
                        _ => y.abs().floor().copysign(y),
                    } / f,
                )
            }
            "INT" if n == 1 => num(nombre(0).map(f64::floor)),
            "ABS" if n == 1 => num(nombre(0).map(f64::abs)),
            "SQRT" if n == 1 => {
                let x = essayer!(nombre(0));
                if x < 0.0 {
                    Value::Error(CellError::Num)
                } else {
                    Value::Number(x.sqrt())
                }
            }
            "POWER" if n == 2 => num(nombre(0).and_then(|a| Ok(a.powf(nombre(1)?)))),
            "EXP" if n == 1 => num(nombre(0).map(f64::exp)),
            "LN" | "LOG10" if n == 1 => {
                let x = essayer!(nombre(0));
                if x <= 0.0 {
                    Value::Error(CellError::Num)
                } else {
                    Value::Number(if nom == "LN" { x.ln() } else { x.log10() })
                }
            }
            "PI" if n == 0 => Value::Number(std::f64::consts::PI),
            "MOD" if n == 2 => {
                let (a, b) = (essayer!(nombre(0)), essayer!(nombre(1)));
                if b == 0.0 {
                    Value::Error(CellError::Div0)
                } else {
                    // The sign of the divisor, as in every spreadsheet.
                    Value::Number(a - b * (a / b).floor())
                }
            }
            "CONCAT" | "CONCATENATE" => {
                let mut s = String::new();
                for (v, _) in self.flat(args) {
                    s.push_str(&essayer!(v.text()));
                }
                Value::Text(s)
            }
            "LEN" if n == 1 => num(un(0).text().map(|t| t.chars().count() as f64)),
            "UPPER" if n == 1 => texte(un(0).text().map(|t| t.to_uppercase())),
            "LOWER" if n == 1 => texte(un(0).text().map(|t| t.to_lowercase())),
            "TRIM" if n == 1 => texte(
                un(0)
                    .text()
                    .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")),
            ),
            "LEFT" | "RIGHT" if compte(1, 2) => {
                let t = essayer!(un(0).text());
                let k = if n == 2 { essayer!(nombre(1)) } else { 1.0 };
                if k < 0.0 {
                    return Value::Error(CellError::Value);
                }
                let k = k as usize;
                let car: Vec<char> = t.chars().collect();
                let k = k.min(car.len());
                Value::Text(if nom == "LEFT" {
                    car[..k].iter().collect()
                } else {
                    car[car.len() - k..].iter().collect()
                })
            }
            "MID" if n == 3 => {
                let t = essayer!(un(0).text());
                let (debut, k) = (essayer!(nombre(1)), essayer!(nombre(2)));
                if debut < 1.0 || k < 0.0 {
                    return Value::Error(CellError::Value);
                }
                Value::Text(
                    t.chars()
                        .skip(debut as usize - 1)
                        .take(k as usize)
                        .collect(),
                )
            }
            "TODAY" if n == 0 => Value::Number(date_serial(self.now.date())),
            "NOW" if n == 0 => Value::Number(datetime_serial(self.now)),
            "DATE" if n == 3 => {
                let (a, m, j) = (
                    essayer!(nombre(0)),
                    essayer!(nombre(1)),
                    essayer!(nombre(2)),
                );
                // Months and days past their end roll over, as in every spreadsheet.
                let mois = (a as i32) * 12 + (m as i32 - 1);
                let Some(premier) =
                    NaiveDate::from_ymd_opt(mois.div_euclid(12), mois.rem_euclid(12) as u32 + 1, 1)
                else {
                    return Value::Error(CellError::Num);
                };
                match premier.checked_add_signed(chrono::Duration::days(j as i64 - 1)) {
                    Some(d) => Value::Number(date_serial(d)),
                    None => Value::Error(CellError::Num),
                }
            }
            "YEAR" | "MONTH" | "DAY" if n == 1 => {
                let Some(d) = serial_date(essayer!(nombre(0))) else {
                    return Value::Error(CellError::Num);
                };
                Value::Number(f64::from(match nom {
                    "YEAR" => d.year() as u32,
                    "MONTH" => d.month(),
                    _ => d.day(),
                }))
            }
            "SUMIF" | "COUNTIF" | "AVERAGEIF" if compte(2, 3) => self.si(nom, args),
            "VLOOKUP" if compte(3, 4) => self.vlookup(args),
            "MATCH" if compte(2, 3) => self.match_(args),
            "INDEX" if compte(2, 3) => self.index(args),
            // A function known, given the wrong number of arguments.
            _ if FUNCTIONS.contains(&nom) => Value::Error(CellError::Value),
            _ => Value::Error(CellError::Name),
        }
    }

    fn si(&self, nom: &str, args: &[Expr]) -> Value {
        let (Arg::Many { values: plage, .. }, critere) = (self.arg(&args[0]), self.eval(&args[1]))
        else {
            return Value::Error(CellError::Value);
        };
        let sommes = if args.len() == 3 {
            match self.arg(&args[2]) {
                Arg::Many { values, .. } => values,
                Arg::One(v) => vec![v],
            }
        } else {
            plage.clone()
        };
        let mut total = 0.0;
        let mut nb = 0usize;
        for (i, v) in plage.iter().enumerate() {
            if !critere_ok(v, &critere) {
                continue;
            }
            nb += 1;
            if let Some(Value::Number(x)) = sommes.get(i) {
                total += x;
            }
        }
        match nom {
            "COUNTIF" => Value::Number(nb as f64),
            "SUMIF" => Value::Number(total),
            _ if nb == 0 => Value::Error(CellError::Div0),
            _ => Value::Number(total / nb as f64),
        }
    }

    fn vlookup(&self, args: &[Expr]) -> Value {
        let cherche = self.eval(&args[0]);
        if let Value::Error(e) = cherche {
            return Value::Error(e);
        }
        let Arg::Many { width, values } = self.arg(&args[1]) else {
            return Value::Error(CellError::Value);
        };
        let col = match self.eval(&args[2]).number() {
            Ok(c) if c >= 1.0 && (c as u32) <= width => c as usize - 1,
            Ok(_) => return Value::Error(CellError::Ref),
            Err(e) => return Value::Error(e),
        };
        let approche = args.len() == 3 || self.eval(&args[3]).truth().unwrap_or(true);
        let w = width as usize;
        let lignes = values.len() / w.max(1);
        if approche {
            // Sorted ascending: the last row not greater than what is looked for.
            let mut trouve = None;
            for r in 0..lignes {
                match comparer(&values[r * w], &cherche) {
                    std::cmp::Ordering::Greater => break,
                    _ => trouve = Some(r),
                }
            }
            match trouve {
                Some(r) => values[r * w + col].clone(),
                None => Value::Error(CellError::NA),
            }
        } else {
            (0..lignes)
                .find(|r| egal(&values[r * w], &cherche))
                .map(|r| values[r * w + col].clone())
                .unwrap_or(Value::Error(CellError::NA))
        }
    }

    fn match_(&self, args: &[Expr]) -> Value {
        let cherche = self.eval(&args[0]);
        let Arg::Many { values, .. } = self.arg(&args[1]) else {
            return Value::Error(CellError::NA);
        };
        let genre = if args.len() == 3 {
            match self.eval(&args[2]).number() {
                Ok(g) => g as i32,
                Err(e) => return Value::Error(e),
            }
        } else {
            1
        };
        let position = match genre {
            0 => values.iter().position(|v| egal(v, &cherche)),
            g => {
                let mut trouve = None;
                for (i, v) in values.iter().enumerate() {
                    let o = comparer(v, &cherche);
                    let ok = if g > 0 {
                        o != std::cmp::Ordering::Greater
                    } else {
                        o != std::cmp::Ordering::Less
                    };
                    if ok {
                        trouve = Some(i);
                    } else {
                        break;
                    }
                }
                trouve
            }
        };
        match position {
            Some(i) => Value::Number(i as f64 + 1.0),
            None => Value::Error(CellError::NA),
        }
    }

    fn index(&self, args: &[Expr]) -> Value {
        let Arg::Many { width, values } = self.arg(&args[0]) else {
            return Value::Error(CellError::Ref);
        };
        let w = width as usize;
        let h = values.len() / w.max(1);
        let a = match self.eval(&args[1]).number() {
            Ok(x) => x as usize,
            Err(e) => return Value::Error(e),
        };
        let b = if args.len() == 3 {
            match self.eval(&args[2]).number() {
                Ok(x) => Some(x as usize),
                Err(e) => return Value::Error(e),
            }
        } else {
            None
        };
        // One row or one column: the one number is a place in it.
        let (r, c) = match b {
            Some(c) => (a, c),
            None if h == 1 => (1, a),
            None => (a, 1),
        };
        if r == 0 || c == 0 || r > h || c > w {
            return Value::Error(CellError::Ref);
        }
        values[(r - 1) * w + c - 1].clone()
    }
}

/// The functions known, for the completion list and `#VALUE!` against `#NAME?`.
pub const FUNCTIONS: &[&str] = &[
    "ABS",
    "AND",
    "AVERAGE",
    "AVERAGEIF",
    "CONCAT",
    "CONCATENATE",
    "COUNT",
    "COUNTA",
    "COUNTBLANK",
    "COUNTIF",
    "DATE",
    "DAY",
    "EXP",
    "IF",
    "IFERROR",
    "INDEX",
    "INT",
    "LEFT",
    "LEN",
    "LN",
    "LOG10",
    "LOWER",
    "MATCH",
    "MAX",
    "MEDIAN",
    "MID",
    "MIN",
    "MOD",
    "MONTH",
    "NOT",
    "NOW",
    "OR",
    "PI",
    "POWER",
    "PRODUCT",
    "RIGHT",
    "ROUND",
    "ROUNDDOWN",
    "ROUNDUP",
    "SQRT",
    "SUM",
    "SUMIF",
    "TODAY",
    "TRIM",
    "UPPER",
    "VLOOKUP",
    "YEAR",
];

fn num(r: Result<f64, CellError>) -> Value {
    match r {
        Ok(n) if n.is_finite() => Value::Number(n),
        Ok(_) => Value::Error(CellError::Num),
        Err(e) => Value::Error(e),
    }
}

fn num_bool(r: Result<bool, CellError>) -> Value {
    match r {
        Ok(b) => Value::Bool(b),
        Err(e) => Value::Error(e),
    }
}

fn texte(r: Result<String, CellError>) -> Value {
    match r {
        Ok(t) => Value::Text(t),
        Err(e) => Value::Error(e),
    }
}

/// How two values order: numbers, then texts (without case), then booleans; nothing is
/// 0 against a number and "" against a text.
fn comparer(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn rang(v: &Value) -> u8 {
        match v {
            Value::Number(_) | Value::Empty => 0,
            Value::Text(_) => 1,
            Value::Bool(_) => 2,
            Value::Error(_) => 3,
        }
    }
    match (a, b) {
        (Value::Empty, Value::Text(t)) => "".cmp(t.to_lowercase().as_str()),
        (Value::Text(t), Value::Empty) => t.to_lowercase().as_str().cmp(""),
        (Value::Text(x), Value::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        _ if rang(a) == 0 && rang(b) == 0 => {
            let (x, y) = (a.number().unwrap_or(0.0), b.number().unwrap_or(0.0));
            x.partial_cmp(&y).unwrap_or(Ordering::Equal)
        }
        _ => rang(a).cmp(&rang(b)),
    }
}

/// How two values order when sorted.
pub fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    comparer(a, b)
}

fn egal(a: &Value, b: &Value) -> bool {
    comparer(a, b) == std::cmp::Ordering::Equal
}

/// Whether a value meets a criterion of SUMIF and COUNTIF: `">5"`, `"<>x"`, `"=3"`, a
/// number, a text (`*` and `?` as wildcards).
fn critere_ok(v: &Value, critere: &Value) -> bool {
    use std::cmp::Ordering::*;
    let Value::Text(c) = critere else {
        return egal(v, critere) && !v.is_empty();
    };
    let (op, reste) = [">=", "<=", "<>", ">", "<", "="]
        .iter()
        .find_map(|o| c.strip_prefix(o).map(|r| (*o, r)))
        .unwrap_or(("=", c.as_str()));
    let cible = match reste.trim().parse::<f64>() {
        Ok(n) => Value::Number(n),
        Err(_) => Value::Text(reste.to_string()),
    };
    // A number criterion only meets numbers.
    if matches!(cible, Value::Number(_)) && !matches!(v, Value::Number(_)) && op != "<>" {
        return false;
    }
    let o = comparer(v, &cible);
    match op {
        ">=" => o != Less,
        "<=" => o != Greater,
        ">" => o == Greater,
        "<" => o == Less,
        "<>" => !correspond(v, &cible),
        _ => correspond(v, &cible),
    }
}

/// Equal, with `*` and `?` in a text standing for anything.
fn correspond(v: &Value, cible: &Value) -> bool {
    match (v, cible) {
        (Value::Text(t), Value::Text(motif)) if motif.contains(['*', '?']) => {
            joker(&t.to_lowercase(), &motif.to_lowercase())
        }
        (Value::Empty, Value::Text(m)) if m.is_empty() => true,
        _ => !v.is_empty() && egal(v, cible),
    }
}

fn joker(t: &str, motif: &str) -> bool {
    let (t, m): (Vec<char>, Vec<char>) = (t.chars().collect(), motif.chars().collect());
    let (mut i, mut j) = (0, 0);
    let (mut etoile, mut reprise) = (None, 0);
    while i < t.len() {
        if j < m.len() && (m[j] == '?' || m[j] == t[i]) {
            i += 1;
            j += 1;
        } else if j < m.len() && m[j] == '*' {
            etoile = Some(j);
            reprise = i;
            j += 1;
        } else if let Some(e) = etoile {
            j = e + 1;
            reprise += 1;
            i = reprise;
        } else {
            return false;
        }
    }
    m[j..].iter().all(|c| *c == '*')
}

fn binaire(op: BinOp, a: Value, b: Value) -> Value {
    use std::cmp::Ordering::*;
    if let Value::Error(e) = a {
        return Value::Error(e);
    }
    if let Value::Error(e) = b {
        return Value::Error(e);
    }
    let nombres = || -> Result<(f64, f64), CellError> { Ok((a.number()?, b.number()?)) };
    match op {
        BinOp::Add => num(nombres().map(|(x, y)| x + y)),
        BinOp::Sub => num(nombres().map(|(x, y)| x - y)),
        BinOp::Mul => num(nombres().map(|(x, y)| x * y)),
        BinOp::Div => match nombres() {
            Ok((_, 0.0)) => Value::Error(CellError::Div0),
            r => num(r.map(|(x, y)| x / y)),
        },
        BinOp::Pow => num(nombres().map(|(x, y)| x.powf(y))),
        BinOp::Concat => match (a.text(), b.text()) {
            (Ok(x), Ok(y)) => Value::Text(x + &y),
            (Err(e), _) | (_, Err(e)) => Value::Error(e),
        },
        BinOp::Eq => Value::Bool(comparer(&a, &b) == Equal),
        BinOp::Ne => Value::Bool(comparer(&a, &b) != Equal),
        BinOp::Lt => Value::Bool(comparer(&a, &b) == Less),
        BinOp::Le => Value::Bool(comparer(&a, &b) != Greater),
        BinOp::Gt => Value::Bool(comparer(&a, &b) == Greater),
        BinOp::Ge => Value::Bool(comparer(&a, &b) != Less),
    }
}

/// What a cell holds when it is not a formula: a number (`3.5`, `3,5`, `1 234`, `12%`,
/// `-4e3`), `TRUE`/`FALSE`, else its text.
pub fn literal(input: &str) -> Value {
    let t = input.trim();
    if t.is_empty() {
        return Value::Empty;
    }
    if t.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if t.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    match crate::format::read_number(t) {
        Some((n, _)) => Value::Number(n),
        None => Value::Text(input.to_string()),
    }
}

/// Every formula of the workbook computed: by sheet, the value of each formula cell.
pub fn recalc(book: &Workbook, now: NaiveDateTime) -> Vec<HashMap<Addr, Value>> {
    let mut valeurs: Vec<HashMap<Addr, Value>> = vec![HashMap::new(); book.sheets.len()];
    // The formulas, read.
    let mut noeuds: Vec<(usize, Addr, Result<Expr, String>)> = Vec::new();
    let mut index: HashMap<(usize, Addr), usize> = HashMap::new();
    // The formula cells of each sheet, by row then column, to find those in a range.
    let mut par_feuille: Vec<BTreeMap<Addr, usize>> = vec![BTreeMap::new(); book.sheets.len()];
    for (s, feuille) in book.sheets.iter().enumerate() {
        for (a, c) in &feuille.cells {
            if let Some(f) = c.input.strip_prefix('=') {
                let i = noeuds.len();
                noeuds.push((s, *a, crate::formula::parse(f)));
                index.insert((s, *a), i);
                par_feuille[s].insert(*a, i);
            }
        }
    }
    let nom_feuille = |home: usize, sheet: &Option<String>| -> Option<usize> {
        match sheet {
            None => Some(home),
            Some(n) => book
                .sheets
                .iter()
                .position(|s| s.name.eq_ignore_ascii_case(n)),
        }
    };
    // What each formula reads, among the formulas.
    let mut lecteurs: Vec<Vec<usize>> = vec![Vec::new(); noeuds.len()];
    let mut attend: Vec<usize> = vec![0; noeuds.len()];
    for (i, (home, _, e)) in noeuds.iter().enumerate() {
        let Ok(e) = e else { continue };
        let mut lus: Vec<usize> = Vec::new();
        visiter(e, &mut |x| match x {
            Expr::Ref { sheet, addr } => {
                if let Some(s) = nom_feuille(*home, sheet) {
                    if let Some(j) = index.get(&(s, *addr)) {
                        lus.push(*j);
                    }
                }
            }
            Expr::Range { sheet, range } => {
                if let Some(s) = nom_feuille(*home, sheet) {
                    for (a, j) in par_feuille[s].range(range.start..=range.end) {
                        if range.contains(*a) {
                            lus.push(*j);
                        }
                    }
                }
            }
            _ => {}
        });
        lus.sort_unstable();
        lus.dedup();
        attend[i] = lus.len();
        for j in lus {
            lecteurs[j].push(i);
        }
    }
    // In order: first the formulas that read no formula.
    let mut prets: VecDeque<usize> = (0..noeuds.len()).filter(|i| attend[*i] == 0).collect();
    let mut fait = vec![false; noeuds.len()];
    while let Some(i) = prets.pop_front() {
        fait[i] = true;
        let (s, a, e) = &noeuds[i];
        let v = match e {
            Ok(e) => {
                let grille = Grid {
                    book,
                    values: &valeurs,
                    home: *s,
                    now,
                };
                match grille.eval(e) {
                    // A formula giving nothing shows 0, as everywhere.
                    Value::Empty => Value::Number(0.0),
                    v => v,
                }
            }
            Err(_) => Value::Error(CellError::Parse),
        };
        valeurs[*s].insert(*a, v);
        for &k in &lecteurs[i] {
            attend[k] -= 1;
            if attend[k] == 0 {
                prets.push_back(k);
            }
        }
    }
    // What is left reads itself, or reads what does.
    for (i, (s, a, _)) in noeuds.iter().enumerate() {
        if !fait[i] {
            valeurs[*s].insert(*a, Value::Error(CellError::Cycle));
        }
    }
    valeurs
}

/// Every part of a formula, the formula first.
pub fn visiter(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    f(e);
    match e {
        Expr::Neg(x) | Expr::Percent(x) => visiter(x, f),
        Expr::Bin(_, a, b) => {
            visiter(a, f);
            visiter(b, f);
        }
        Expr::Call(_, args) => {
            for a in args {
                visiter(a, f);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::Sheet;

    fn classeur(cellules: &[(&str, &str)]) -> Workbook {
        let mut s = Sheet::new("Sheet1");
        for (a, v) in cellules {
            s.set_input(Addr::parse(a).unwrap(), v);
        }
        Workbook { sheets: vec![s] }
    }

    fn maintenant() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 7)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn valeur(cellules: &[(&str, &str)], ou: &str) -> Value {
        let b = classeur(cellules);
        let v = recalc(&b, maintenant());
        v[0].get(&Addr::parse(ou).unwrap())
            .cloned()
            .unwrap_or_else(|| b.sheets[0].value_of_literal(Addr::parse(ou).unwrap()))
    }

    fn f(formule: &str) -> Value {
        valeur(
            &[
                ("A1", "1"),
                ("A2", "2"),
                ("A3", "3"),
                ("B1", "pomme"),
                ("B2", "Poire"),
                ("B3", ""),
                ("C1", "10"),
                ("C2", "20"),
                ("C3", "30"),
                ("D1", formule),
            ],
            "D1",
        )
    }

    fn n(x: f64) -> Value {
        Value::Number(x)
    }

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn arithmetic_and_text() {
        assert_eq!(f("=1+2*3"), n(7.0));
        assert_eq!(f("=-2^2"), n(4.0));
        assert_eq!(f("=A1/0"), Value::Error(CellError::Div0));
        assert_eq!(f("=50%*C1"), n(5.0));
        assert_eq!(f("=B1&\" \"&A2"), t("pomme 2"));
        assert_eq!(f("=B1+1"), Value::Error(CellError::Value));
        assert_eq!(f("=A1<A2"), Value::Bool(true));
        assert_eq!(f("=B1=\"POMME\""), Value::Bool(true));
        assert_eq!(f("=Z99"), n(0.0));
        assert_eq!(f("=nope(1)"), Value::Error(CellError::Name));
        assert_eq!(f("=1+"), Value::Error(CellError::Parse));
    }

    #[test]
    fn every_function() {
        let cas: &[(&str, Value)] = &[
            ("=SUM(A1:A3)", n(6.0)),
            ("=SUM(A1:B3; 4)", n(10.0)),
            ("=PRODUCT(A1:A3)", n(6.0)),
            ("=AVERAGE(A1:A3)", n(2.0)),
            ("=AVERAGE(B1:B3)", Value::Error(CellError::Div0)),
            ("=MIN(C1:C3)", n(10.0)),
            ("=MAX(C1:C3)", n(30.0)),
            ("=MEDIAN(A1:A3;10)", n(2.5)),
            ("=COUNT(A1:C3)", n(6.0)),
            ("=COUNTA(A1:C3)", n(8.0)),
            ("=COUNTBLANK(B1:B3)", n(1.0)),
            ("=IF(A1>0;\"yes\";\"no\")", t("yes")),
            ("=IF(A1>5;\"yes\")", Value::Bool(false)),
            ("=IFERROR(1/0;\"none\")", t("none")),
            ("=AND(A1>0;A2>1)", Value::Bool(true)),
            ("=OR(A1>5;FALSE)", Value::Bool(false)),
            ("=NOT(A1)", Value::Bool(false)),
            ("=ROUND(2.675;2)", n(2.68)),
            ("=ROUND(-1.5)", n(-2.0)),
            ("=ROUNDUP(1.21;1)", n(1.3)),
            ("=ROUNDDOWN(-1.29;1)", n(-1.2)),
            ("=INT(-1.5)", n(-2.0)),
            ("=ABS(-3)", n(3.0)),
            ("=SQRT(16)", n(4.0)),
            ("=SQRT(-1)", Value::Error(CellError::Num)),
            ("=POWER(2;10)", n(1024.0)),
            ("=MOD(-3;2)", n(1.0)),
            ("=MOD(1;0)", Value::Error(CellError::Div0)),
            ("=CONCAT(B1:B2;\"!\")", t("pommePoire!")),
            ("=LEN(B2)", n(5.0)),
            ("=LEFT(B1;3)", t("pom")),
            ("=RIGHT(B1;2)", t("me")),
            ("=MID(B1;2;3)", t("omm")),
            ("=UPPER(B2)", t("POIRE")),
            ("=LOWER(B2)", t("poire")),
            ("=TRIM(\"  a   b \")", t("a b")),
            ("=TODAY()", n(46302.0)),
            ("=DATE(2026;10;7)", n(46302.0)),
            ("=DATE(2026;13;1)", n(46388.0)),
            ("=YEAR(46302)", n(2026.0)),
            ("=MONTH(46302)", n(10.0)),
            ("=DAY(46302)", n(7.0)),
            ("=SUMIF(A1:A3;\">1\")", n(5.0)),
            ("=SUMIF(B1:B3;\"p*\";C1:C3)", n(30.0)),
            ("=COUNTIF(B1:B3;\"poire\")", n(1.0)),
            ("=AVERAGEIF(A1:A3;\"<>2\";C1:C3)", n(20.0)),
            ("=VLOOKUP(2;A1:C3;3;FALSE)", n(20.0)),
            ("=VLOOKUP(2.5;A1:C3;2)", t("Poire")),
            ("=VLOOKUP(9;A1:C3;4;FALSE)", Value::Error(CellError::Ref)),
            ("=VLOOKUP(9;A1:C3;2;FALSE)", Value::Error(CellError::NA)),
            ("=MATCH(\"poire\";B1:B3;0)", n(2.0)),
            ("=MATCH(25;C1:C3)", n(2.0)),
            ("=INDEX(A1:C3;2;3)", n(20.0)),
            ("=INDEX(C1:C3;3)", n(30.0)),
            ("=INDEX(A1:C3;4;1)", Value::Error(CellError::Ref)),
            ("=PI()", n(std::f64::consts::PI)),
            ("=SUM()", n(0.0)),
            ("=IF(1)", Value::Error(CellError::Value)),
        ];
        for (formule, attendu) in cas {
            let v = f(formule);
            match (&v, attendu) {
                (Value::Number(x), Value::Number(y)) => {
                    assert!((x - y).abs() < 1e-9, "{formule}: {v:?} ≠ {attendu:?}")
                }
                _ => assert_eq!(&v, attendu, "{formule}"),
            }
        }
    }

    #[test]
    fn formulas_are_computed_after_what_they_read() {
        // Written in the wrong order on purpose.
        let cellules = [
            ("A3", "=A2*2"),
            ("A2", "=A1+1"),
            ("A1", "=5"),
            ("B1", "=SUM(A1:A3)"),
        ];
        assert_eq!(valeur(&cellules, "A3"), n(12.0));
        assert_eq!(valeur(&cellules, "B1"), n(23.0));
    }

    #[test]
    fn a_long_chain_does_not_recurse() {
        let mut s = Sheet::new("Sheet1");
        s.set_input(Addr::new(0, 0), "1");
        for r in 1..20_000 {
            s.set_input(Addr::new(0, r), &format!("=A{r}+1"));
        }
        let v = recalc(&Workbook { sheets: vec![s] }, maintenant());
        assert_eq!(v[0][&Addr::new(0, 19_999)], n(20_000.0));
    }

    #[test]
    fn cycles_are_told_and_spread() {
        let cellules = [
            ("A1", "=B1"),
            ("B1", "=A1+1"),
            ("C1", "=A1*2"),
            ("D1", "=SUM(D1:D2)"),
            ("E1", "=1"),
        ];
        assert_eq!(valeur(&cellules, "A1"), Value::Error(CellError::Cycle));
        assert_eq!(valeur(&cellules, "C1"), Value::Error(CellError::Cycle));
        assert_eq!(valeur(&cellules, "D1"), Value::Error(CellError::Cycle));
        assert_eq!(valeur(&cellules, "E1"), n(1.0));
    }

    #[test]
    fn errors_spread_through_formulas() {
        let cellules = [("A1", "=1/0"), ("A2", "=A1+1"), ("A3", "=IFERROR(A2;0)")];
        assert_eq!(valeur(&cellules, "A2"), Value::Error(CellError::Div0));
        assert_eq!(valeur(&cellules, "A3"), n(0.0));
    }

    #[test]
    fn other_sheets_are_read() {
        let mut a = Sheet::new("Data");
        a.set_input(Addr::new(0, 0), "4");
        a.set_input(Addr::new(0, 1), "=A1*2");
        let mut b = Sheet::new("Summary");
        b.set_input(Addr::new(0, 0), "=SUM(Data!A1:A2)+data!A1");
        b.set_input(Addr::new(0, 1), "=Nowhere!A1");
        let v = recalc(&Workbook { sheets: vec![a, b] }, maintenant());
        assert_eq!(v[1][&Addr::new(0, 0)], n(16.0));
        assert_eq!(v[1][&Addr::new(0, 1)], Value::Error(CellError::Ref));
    }

    #[test]
    fn literals_and_the_general_format() {
        assert_eq!(literal("3,5"), n(3.5));
        assert_eq!(literal(" 1 234,5 "), n(1234.5));
        assert_eq!(literal("12%"), n(0.12));
        assert_eq!(literal("true"), Value::Bool(true));
        assert_eq!(literal("Chapitre 2"), t("Chapitre 2"));
        assert_eq!(literal(""), Value::Empty);
        assert_eq!(general(0.1 + 0.2), "0.3");
        assert_eq!(general(1.0 / 3.0), "0.3333333333");
        assert_eq!(general(1234567.0), "1234567");
        assert_eq!(general(-2.5), "-2.5");
        assert_eq!(general(1e20), "1E+20");
        assert_eq!(general(1.5e-12), "1.5E-12");
    }

    #[test]
    fn criteria_with_wildcards() {
        assert!(joker("chapitre 12", "chap*"));
        assert!(joker("abc", "a?c"));
        assert!(!joker("abc", "a?d"));
        assert!(joker("", "*"));
    }
}
