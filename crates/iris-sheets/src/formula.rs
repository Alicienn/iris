//! Formulas: read into a tree, and rewritten when cells move.
//!
//! What follows the `=`: numbers, `"text"`, `TRUE`/`FALSE`, references (`A1`, `$A$1`,
//! `Sheet2!A1`, `'My sheet'!A1:B3`), operators by Excel's precedence — comparison,
//! then `&`, `+ -`, `* /`, `^`, the sign, `%` — and function calls (`SUM(A1:A3; 2)`,
//! with `,` or `;` between arguments).

use crate::cell::{parse_ref, Addr, Range};
use crate::eval::CellError;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(f64),
    Text(String),
    Bool(bool),
    Error(CellError),
    Ref {
        sheet: Option<String>,
        addr: Addr,
    },
    Range {
        sheet: Option<String>,
        range: Range,
    },
    Neg(Box<Expr>),
    Percent(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// A function, its name in capitals.
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Word(String),
    /// `'a quoted sheet name'`.
    Quoted(String),
    Err(CellError),
    Op(&'static str),
    LParen,
    RParen,
    Sep,
    Colon,
    Bang,
    Percent,
}

#[derive(Debug, Clone)]
struct Lexed {
    tok: Tok,
    start: usize,
    end: usize,
}

fn lex(src: &str) -> Result<Vec<Lexed>, String> {
    let b = src.as_bytes();
    let mut sortie = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let debut = i;
        let tok = match c {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
                continue;
            }
            b'0'..=b'9' | b'.' => {
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                    let mut j = i + 1;
                    if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                        j += 1;
                    }
                    if j < b.len() && b[j].is_ascii_digit() {
                        while j < b.len() && b[j].is_ascii_digit() {
                            j += 1;
                        }
                        i = j;
                    }
                }
                let n: f64 = src[debut..i]
                    .parse()
                    .map_err(|_| format!("“{}” is not a number", &src[debut..i]))?;
                Tok::Num(n)
            }
            b'"' => {
                let mut texte = String::new();
                i += 1;
                loop {
                    let Some(rel) = src[i..].find('"') else {
                        return Err("a text is not closed with \"".into());
                    };
                    texte.push_str(&src[i..i + rel]);
                    i += rel + 1;
                    if b.get(i) == Some(&b'"') {
                        texte.push('"');
                        i += 1;
                    } else {
                        break;
                    }
                }
                Tok::Str(texte)
            }
            b'\'' => {
                let Some(rel) = src[i + 1..].find('\'') else {
                    return Err("a sheet name is not closed with '".into());
                };
                let nom = src[i + 1..i + 1 + rel].to_string();
                i += rel + 2;
                Tok::Quoted(nom)
            }
            b'#' => {
                let reste = &src[i..];
                let Some((e, n)) = CellError::ALL
                    .iter()
                    .map(|e| (*e, e.text()))
                    .find(|(_, t)| reste.to_ascii_uppercase().starts_with(t))
                    .map(|(e, t)| (e, t.len()))
                else {
                    return Err("an unknown error name".into());
                };
                i += n;
                Tok::Err(e)
            }
            b'(' => {
                i += 1;
                Tok::LParen
            }
            b')' => {
                i += 1;
                Tok::RParen
            }
            b',' | b';' => {
                i += 1;
                Tok::Sep
            }
            b':' => {
                i += 1;
                Tok::Colon
            }
            b'!' => {
                i += 1;
                Tok::Bang
            }
            b'%' => {
                i += 1;
                Tok::Percent
            }
            b'+' | b'-' | b'*' | b'/' | b'^' | b'&' | b'=' => {
                i += 1;
                Tok::Op(match c {
                    b'+' => "+",
                    b'-' => "-",
                    b'*' => "*",
                    b'/' => "/",
                    b'^' => "^",
                    b'&' => "&",
                    _ => "=",
                })
            }
            b'<' => {
                i += 1;
                match b.get(i) {
                    Some(b'=') => {
                        i += 1;
                        Tok::Op("<=")
                    }
                    Some(b'>') => {
                        i += 1;
                        Tok::Op("<>")
                    }
                    _ => Tok::Op("<"),
                }
            }
            b'>' => {
                i += 1;
                if b.get(i) == Some(&b'=') {
                    i += 1;
                    Tok::Op(">=")
                } else {
                    Tok::Op(">")
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 => {
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric()
                        || b[i] == b'_'
                        || b[i] == b'.'
                        || b[i] == b'$'
                        || b[i] >= 0x80)
                {
                    i += 1;
                }
                Tok::Word(src[debut..i].to_string())
            }
            _ => {
                let car = src[i..].chars().next().unwrap_or('?');
                return Err(format!("“{car}” is not understood here"));
            }
        };
        sortie.push(Lexed {
            tok,
            start: debut,
            end: i,
        });
    }
    Ok(sortie)
}

struct Parser {
    toks: Vec<Lexed>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|l| &l.tok)
    }

    fn peek_at(&self, k: usize) -> Option<&Tok> {
        self.toks.get(self.pos + k).map(|l| &l.tok)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|l| l.tok.clone());
        self.pos += 1;
        t
    }

    fn op(&self) -> Option<&'static str> {
        match self.peek() {
            Some(Tok::Op(o)) => Some(o),
            _ => None,
        }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut gauche = self.concat()?;
        while let Some(o) = self.op() {
            let op = match o {
                "=" => BinOp::Eq,
                "<>" => BinOp::Ne,
                "<" => BinOp::Lt,
                "<=" => BinOp::Le,
                ">" => BinOp::Gt,
                ">=" => BinOp::Ge,
                _ => break,
            };
            self.pos += 1;
            let droite = self.concat()?;
            gauche = Expr::Bin(op, Box::new(gauche), Box::new(droite));
        }
        Ok(gauche)
    }

    fn concat(&mut self) -> Result<Expr, String> {
        let mut gauche = self.add()?;
        while self.op() == Some("&") {
            self.pos += 1;
            let droite = self.add()?;
            gauche = Expr::Bin(BinOp::Concat, Box::new(gauche), Box::new(droite));
        }
        Ok(gauche)
    }

    fn add(&mut self) -> Result<Expr, String> {
        let mut gauche = self.mul()?;
        while let Some(o @ ("+" | "-")) = self.op() {
            self.pos += 1;
            let droite = self.mul()?;
            let op = if o == "+" { BinOp::Add } else { BinOp::Sub };
            gauche = Expr::Bin(op, Box::new(gauche), Box::new(droite));
        }
        Ok(gauche)
    }

    fn mul(&mut self) -> Result<Expr, String> {
        let mut gauche = self.pow()?;
        while let Some(o @ ("*" | "/")) = self.op() {
            self.pos += 1;
            let droite = self.pow()?;
            let op = if o == "*" { BinOp::Mul } else { BinOp::Div };
            gauche = Expr::Bin(op, Box::new(gauche), Box::new(droite));
        }
        Ok(gauche)
    }

    fn pow(&mut self) -> Result<Expr, String> {
        let mut gauche = self.unary()?;
        while self.op() == Some("^") {
            self.pos += 1;
            let droite = self.unary()?;
            gauche = Expr::Bin(BinOp::Pow, Box::new(gauche), Box::new(droite));
        }
        Ok(gauche)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.op() {
            Some("-") => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Some("+") => {
                self.pos += 1;
                self.unary()
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        while self.peek() == Some(&Tok::Percent) {
            self.pos += 1;
            e = Expr::Percent(Box::new(e));
        }
        Ok(e)
    }

    /// A reference after its sheet (if any): `A1` or `A1:B3`.
    fn reference(&mut self, sheet: Option<String>) -> Result<Expr, String> {
        let Some(Tok::Word(w)) = self.next() else {
            return Err("a cell was expected".into());
        };
        let Some((a, _, _)) = parse_ref(&w) else {
            return Err(format!("“{w}” is not a cell"));
        };
        if self.peek() == Some(&Tok::Colon) {
            self.pos += 1;
            // `Sheet!A1:Sheet!B2` is read too.
            if matches!(self.peek(), Some(Tok::Word(_) | Tok::Quoted(_)))
                && self.peek_at(1) == Some(&Tok::Bang)
            {
                self.pos += 2;
            }
            let Some(Tok::Word(w2)) = self.next() else {
                return Err("a cell was expected after “:”".into());
            };
            let Some((b, _, _)) = parse_ref(&w2) else {
                return Err(format!("“{w2}” is not a cell"));
            };
            return Ok(Expr::Range {
                sheet,
                range: Range::new(a, b),
            });
        }
        Ok(Expr::Ref { sheet, addr: a })
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(Expr::Number(n))
            }
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Ok(Expr::Text(s))
            }
            Some(Tok::Err(e)) => {
                self.pos += 1;
                Ok(Expr::Error(e))
            }
            Some(Tok::LParen) => {
                self.pos += 1;
                let e = self.expr()?;
                if self.next() != Some(Tok::RParen) {
                    return Err("a “)” is missing".into());
                }
                Ok(e)
            }
            Some(Tok::Quoted(nom)) => {
                self.pos += 1;
                if self.next() != Some(Tok::Bang) {
                    return Err("a “!” was expected after the sheet's name".into());
                }
                self.reference(Some(nom))
            }
            Some(Tok::Word(w)) => {
                if self.peek_at(1) == Some(&Tok::Bang) {
                    self.pos += 2;
                    return self.reference(Some(w));
                }
                if self.peek_at(1) == Some(&Tok::LParen) {
                    self.pos += 2;
                    let nom = w.to_ascii_uppercase();
                    let mut args = Vec::new();
                    if self.peek() == Some(&Tok::RParen) {
                        self.pos += 1;
                        return Ok(Expr::Call(nom, args));
                    }
                    loop {
                        // An empty argument (`IF(A1;;2)`) is nothing.
                        if matches!(self.peek(), Some(Tok::Sep | Tok::RParen)) {
                            args.push(Expr::Text(String::new()));
                        } else {
                            args.push(self.expr()?);
                        }
                        match self.next() {
                            Some(Tok::Sep) => continue,
                            Some(Tok::RParen) => break,
                            _ => return Err(format!("a “)” is missing after {nom}(")),
                        }
                    }
                    return Ok(Expr::Call(nom, args));
                }
                match w.to_ascii_uppercase().as_str() {
                    "TRUE" => {
                        self.pos += 1;
                        Ok(Expr::Bool(true))
                    }
                    "FALSE" => {
                        self.pos += 1;
                        Ok(Expr::Bool(false))
                    }
                    _ if parse_ref(&w).is_some() => self.reference(None),
                    _ => Err(format!("“{w}” is unknown")),
                }
            }
            Some(t) => Err(format!("{} is not expected here", montrer(&t))),
            None => Err("the formula ends too soon".into()),
        }
    }
}

fn montrer(t: &Tok) -> String {
    match t {
        Tok::Op(o) => format!("“{o}”"),
        Tok::RParen => "“)”".into(),
        Tok::Sep => "“;”".into(),
        Tok::Colon => "“:”".into(),
        Tok::Bang => "“!”".into(),
        Tok::Percent => "“%”".into(),
        _ => "this".into(),
    }
}

/// Reads a formula, without its `=`.
pub fn parse(src: &str) -> Result<Expr, String> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err("the formula is empty".into());
    }
    let mut p = Parser { toks, pos: 0 };
    let e = p.expr()?;
    match p.peek() {
        None => Ok(e),
        Some(t) => Err(format!("{} is not expected here", montrer(t))),
    }
}

// --- Rewriting ------------------------------------------------------------------------

/// A reference as written: its place in the text, its sheet, its one or two corners.
struct Written {
    start: usize,
    end: usize,
    /// The sheet as written, `!` included (`'My sheet'!`), or "".
    prefix: String,
    sheet: Option<String>,
    first: (Addr, bool, bool),
    second: Option<(Addr, bool, bool)>,
}

fn references(src: &str) -> Vec<Written> {
    let Ok(toks) = lex(src) else {
        return Vec::new();
    };
    let mut sortie = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let mut j = i;
        let mut sheet = None;
        if matches!(toks[j].tok, Tok::Word(_) | Tok::Quoted(_))
            && toks.get(j + 1).map(|l| &l.tok) == Some(&Tok::Bang)
        {
            sheet = Some(match &toks[j].tok {
                Tok::Word(w) | Tok::Quoted(w) => w.clone(),
                _ => unreachable!(),
            });
            j += 2;
        }
        let premiere = match toks.get(j).map(|l| &l.tok) {
            Some(Tok::Word(w)) if toks.get(j + 1).map(|l| &l.tok) != Some(&Tok::LParen) => {
                parse_ref(w)
            }
            _ => None,
        };
        let Some(first) = premiere else {
            i += 1;
            continue;
        };
        let mut fin = toks[j].end;
        let mut second = None;
        if toks.get(j + 1).map(|l| &l.tok) == Some(&Tok::Colon) {
            if let Some(Lexed {
                tok: Tok::Word(w2),
                end,
                ..
            }) = toks.get(j + 2)
            {
                if let Some(r) = parse_ref(w2) {
                    second = Some(r);
                    fin = *end;
                    j += 2;
                }
            }
        }
        sortie.push(Written {
            start: toks[i].start,
            end: fin,
            prefix: if sheet.is_some() {
                src[toks[i].start..toks[i + 1].end].to_string()
            } else {
                String::new()
            },
            sheet,
            first,
            second,
        });
        i = j + 1;
    }
    sortie
}

fn ecrire_ref((a, abs_c, abs_r): (Addr, bool, bool)) -> String {
    format!(
        "{}{}{}{}",
        if abs_c { "$" } else { "" },
        crate::cell::col_name(a.col),
        if abs_r { "$" } else { "" },
        a.row + 1
    )
}

/// The formula with each reference replaced by what `f` makes of it (`None`: `#REF!`).
fn rewrite(
    src: &str,
    mut f: impl FnMut(&Written) -> Option<((Addr, bool, bool), Option<(Addr, bool, bool)>)>,
) -> String {
    let mut sortie = String::with_capacity(src.len());
    let mut dernier = 0;
    for w in references(src) {
        sortie.push_str(&src[dernier..w.start]);
        match f(&w) {
            Some((a, b)) => {
                sortie.push_str(&w.prefix);
                sortie.push_str(&ecrire_ref(a));
                if let Some(b) = b {
                    sortie.push(':');
                    sortie.push_str(&ecrire_ref(b));
                }
            }
            None => sortie.push_str("#REF!"),
        }
        dernier = w.end;
    }
    sortie.push_str(&src[dernier..]);
    sortie
}

/// A formula copied `dc` columns and `dr` rows away: its references not fixed with `$`
/// move with it.
pub fn shift(src: &str, dc: i64, dr: i64) -> String {
    let bouger = |(a, abs_c, abs_r): (Addr, bool, bool)| {
        let n = a.offset(if abs_c { 0 } else { dc }, if abs_r { 0 } else { dr })?;
        Some((n, abs_c, abs_r))
    };
    rewrite(src, |w| {
        let a = bouger(w.first)?;
        let b = match w.second {
            Some(s) => Some(bouger(s)?),
            None => None,
        };
        Some((a, b))
    })
}

/// Rows or columns inserted or deleted in a sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    InsertRows { at: u32, n: u32 },
    DeleteRows { at: u32, n: u32 },
    InsertCols { at: u32, n: u32 },
    DeleteCols { at: u32, n: u32 },
}

/// One coordinate after the change: inserted before it pushes it on, deleted under it
/// is `None`.
fn deplacer(x: u32, at: u32, n: u32, insert: bool) -> Option<u32> {
    if insert {
        Some(if x >= at { x + n } else { x })
    } else if x < at {
        Some(x)
    } else if x >= at + n {
        Some(x - n)
    } else {
        None
    }
}

/// A formula of sheet `home` after `change` in sheet `changed`: its references to that
/// sheet follow the cells; a reference to a deleted cell becomes `#REF!`, a range loses
/// what was deleted of it.
pub fn adjust(src: &str, home: &str, changed: &str, change: Change) -> String {
    let (at, n, insert, rows) = match change {
        Change::InsertRows { at, n } => (at, n, true, true),
        Change::DeleteRows { at, n } => (at, n, false, true),
        Change::InsertCols { at, n } => (at, n, true, false),
        Change::DeleteCols { at, n } => (at, n, false, false),
    };
    let coord = |a: Addr| if rows { a.row } else { a.col };
    let poser = |a: Addr, x: u32| {
        if rows {
            Addr::new(a.col, x)
        } else {
            Addr::new(x, a.row)
        }
    };
    rewrite(src, |w| {
        let sheet = w.sheet.as_deref().unwrap_or(home);
        if !sheet.eq_ignore_ascii_case(changed) {
            return Some((w.first, w.second));
        }
        match w.second {
            None => {
                let x = deplacer(coord(w.first.0), at, n, insert)?;
                Some(((poser(w.first.0, x), w.first.1, w.first.2), None))
            }
            Some(second) => {
                let (lo, hi) = (coord(w.first.0), coord(second.0));
                let (nlo, nhi) = if insert {
                    (
                        deplacer(lo, at, n, true)?,
                        // Inserted inside the range (or just after its first line): it grows.
                        if hi >= at { hi + n } else { hi },
                    )
                } else {
                    let fin = at + n;
                    if lo >= at && hi < fin {
                        return None;
                    }
                    let nlo = if lo < at {
                        lo
                    } else if lo < fin {
                        at
                    } else {
                        lo - n
                    };
                    let nhi = if hi < at {
                        hi
                    } else if hi < fin {
                        at.saturating_sub(1)
                    } else {
                        hi - n
                    };
                    (nlo, nhi)
                };
                Some((
                    (poser(w.first.0, nlo), w.first.1, w.first.2),
                    Some((poser(second.0, nhi), second.1, second.2)),
                ))
            }
        }
    })
}

/// A formula after its sheet `old` was renamed `new`.
pub fn rename_sheet(src: &str, old: &str, new: &str) -> String {
    let prefixe = sheet_prefix(new);
    let mut sortie = String::with_capacity(src.len());
    let mut dernier = 0;
    for w in references(src) {
        if w.sheet
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(old))
        {
            sortie.push_str(&src[dernier..w.start]);
            sortie.push_str(&prefixe);
            sortie.push_str(&src[w.start + w.prefix.len()..w.end]);
            dernier = w.end;
        }
    }
    sortie.push_str(&src[dernier..]);
    sortie
}

/// `Sheet2!` or `'My sheet'!`.
pub fn sheet_prefix(name: &str) -> String {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && !name.starts_with(|c: char| c.is_ascii_digit())
    {
        format!("{name}!")
    } else {
        format!("'{}'!", name.replace('\'', ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Expr {
        Expr::Ref {
            sheet: None,
            addr: Addr::parse(s).unwrap(),
        }
    }

    #[test]
    fn precedence_is_excels() {
        // 1 + 2 * 3 ^ 2
        assert_eq!(
            parse("1+2*3^2").unwrap(),
            Expr::Bin(
                BinOp::Add,
                Box::new(Expr::Number(1.0)),
                Box::new(Expr::Bin(
                    BinOp::Mul,
                    Box::new(Expr::Number(2.0)),
                    Box::new(Expr::Bin(
                        BinOp::Pow,
                        Box::new(Expr::Number(3.0)),
                        Box::new(Expr::Number(2.0))
                    ))
                ))
            )
        );
        // The sign binds tighter than ^: -2^2 is 4.
        assert_eq!(
            parse("-2^2").unwrap(),
            Expr::Bin(
                BinOp::Pow,
                Box::new(Expr::Neg(Box::new(Expr::Number(2.0)))),
                Box::new(Expr::Number(2.0))
            )
        );
        assert!(matches!(
            parse("A1&\"x\"=B1").unwrap(),
            Expr::Bin(BinOp::Eq, _, _)
        ));
        assert_eq!(
            parse("50%").unwrap(),
            Expr::Percent(Box::new(Expr::Number(50.0)))
        );
    }

    #[test]
    fn references_ranges_sheets_and_calls() {
        assert_eq!(parse("$B$2").unwrap(), r("B2"));
        assert_eq!(
            parse("SUM(A1:B3; 'My sheet'!C1, Data!A2:A1)").unwrap(),
            Expr::Call(
                "SUM".into(),
                vec![
                    Expr::Range {
                        sheet: None,
                        range: Range::parse("A1:B3").unwrap()
                    },
                    Expr::Ref {
                        sheet: Some("My sheet".into()),
                        addr: Addr::new(2, 0)
                    },
                    Expr::Range {
                        sheet: Some("Data".into()),
                        range: Range::parse("A1:A2").unwrap()
                    },
                ]
            )
        );
        assert_eq!(
            parse("today()").unwrap(),
            Expr::Call("TODAY".into(), vec![])
        );
        // LOG10 is a function when called, though it reads as a cell.
        assert_eq!(
            parse("LOG10(100)").unwrap(),
            Expr::Call("LOG10".into(), vec![Expr::Number(100.0)])
        );
        assert_eq!(parse("true").unwrap(), Expr::Bool(true));
        assert_eq!(
            parse("\"a \"\"b\"\"\"").unwrap(),
            Expr::Text("a \"b\"".into())
        );
        assert_eq!(
            parse("#REF!+1").unwrap(),
            Expr::Bin(
                BinOp::Add,
                Box::new(Expr::Error(CellError::Ref)),
                Box::new(Expr::Number(1.0))
            )
        );
    }

    #[test]
    fn mistakes_are_told() {
        for faux in ["", "1+", "(1", "SUM(1", "foo", "1 2", "\"a", "A1:", "@"] {
            assert!(parse(faux).is_err(), "{faux}");
        }
    }

    #[test]
    fn copied_formulas_move_their_references() {
        assert_eq!(shift("A1+$B$1+C$1+$D1", 1, 2), "B3+$B$1+D$1+$D3");
        assert_eq!(shift("SUM(A1:A3)*Data!B2", 0, 1), "SUM(A2:A4)*Data!B3");
        assert_eq!(shift("A1", -1, 0), "#REF!");
        // Functions and texts are left alone.
        assert_eq!(shift("LOG10(A1)&\"B2\"", 0, 1), "LOG10(A2)&\"B2\"");
    }

    #[test]
    fn rows_inserted_and_deleted_move_references() {
        let ins = Change::InsertRows { at: 1, n: 2 };
        assert_eq!(
            adjust("A1+A2+SUM(A1:A3)", "S", "S", ins),
            "A1+A4+SUM(A1:A5)"
        );
        // Another sheet's change does not touch it…
        assert_eq!(adjust("A2", "S", "T", ins), "A2");
        // …but a reference to that sheet does move.
        assert_eq!(adjust("T!A2", "S", "T", ins), "T!A4");
        let del = Change::DeleteRows { at: 1, n: 1 };
        assert_eq!(adjust("A2", "S", "S", del), "#REF!");
        assert_eq!(adjust("A3+SUM(A1:A4)", "S", "S", del), "A2+SUM(A1:A3)");
        assert_eq!(adjust("SUM(A2:A2)", "S", "S", del), "SUM(#REF!)");
        let cols = Change::DeleteCols { at: 0, n: 1 };
        assert_eq!(adjust("B1+SUM(A1:C1)", "S", "S", cols), "A1+SUM(A1:B1)");
    }

    #[test]
    fn a_renamed_sheet_is_renamed_in_formulas() {
        assert_eq!(
            rename_sheet("Data!A1+'data'!B2+A3", "Data", "My data"),
            "'My data'!A1+'My data'!B2+A3"
        );
        assert_eq!(sheet_prefix("Sheet2"), "Sheet2!");
    }
}
