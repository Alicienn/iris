//! Cell addresses: `A1`, `$B$2`, ranges `A1:C3`.

/// A cell: column and row, from 0. Ordered row by row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Addr {
    pub row: u32,
    pub col: u32,
}

/// The most columns and rows a sheet has (Excel's).
pub const MAX_COLS: u32 = 16_384;
pub const MAX_ROWS: u32 = 1_048_576;

impl Addr {
    pub fn new(col: u32, row: u32) -> Self {
        Self { row, col }
    }

    /// Reads `A1`, `$A$1`, `aa10`; `None` when it is not an address.
    pub fn parse(s: &str) -> Option<Addr> {
        parse_ref(s).map(|r| r.0)
    }

    /// `A1`.
    pub fn name(&self) -> String {
        format!("{}{}", col_name(self.col), self.row + 1)
    }

    /// Moved by `dc` columns and `dr` rows; `None` off the sheet.
    pub fn offset(&self, dc: i64, dr: i64) -> Option<Addr> {
        let c = i64::from(self.col) + dc;
        let r = i64::from(self.row) + dr;
        (c >= 0 && r >= 0 && c < i64::from(MAX_COLS) && r < i64::from(MAX_ROWS))
            .then(|| Addr::new(c as u32, r as u32))
    }
}

/// An address and whether its column and row are fixed (`$`).
pub(crate) fn parse_ref(s: &str) -> Option<(Addr, bool, bool)> {
    let b = s.as_bytes();
    let mut i = 0;
    let abs_col = b.first() == Some(&b'$');
    if abs_col {
        i += 1;
    }
    let debut_lettres = i;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    let lettres = &s[debut_lettres..i];
    if lettres.is_empty() || lettres.len() > 3 {
        return None;
    }
    let abs_row = b.get(i) == Some(&b'$');
    if abs_row {
        i += 1;
    }
    let chiffres = &s[i..];
    if chiffres.is_empty()
        || !chiffres.bytes().all(|c| c.is_ascii_digit())
        || chiffres.starts_with('0')
    {
        return None;
    }
    let col = col_index(lettres)?;
    let row: u32 = chiffres.parse().ok()?;
    if row == 0 || row > MAX_ROWS || col >= MAX_COLS {
        return None;
    }
    Some((Addr::new(col, row - 1), abs_col, abs_row))
}

/// `A` for 0, `Z` for 25, `AA` for 26.
pub fn col_name(col: u32) -> String {
    let mut n = col + 1;
    let mut lettres = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        lettres.push(b'A' + r as u8);
        n = (n - 1) / 26;
    }
    lettres.reverse();
    String::from_utf8(lettres).unwrap_or_default()
}

/// The column of `A`, `aa`…; `None` when it is not letters.
pub fn col_index(lettres: &str) -> Option<u32> {
    if lettres.is_empty() || lettres.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for c in lettres.bytes() {
        if !c.is_ascii_alphabetic() {
            return None;
        }
        n = n * 26 + u32::from(c.to_ascii_uppercase() - b'A') + 1;
    }
    Some(n - 1)
}

/// A rectangle of cells, its corners in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: Addr,
    pub end: Addr,
}

impl Range {
    /// The rectangle between two cells, whichever corners they are.
    pub fn new(a: Addr, b: Addr) -> Self {
        Self {
            start: Addr::new(a.col.min(b.col), a.row.min(b.row)),
            end: Addr::new(a.col.max(b.col), a.row.max(b.row)),
        }
    }

    pub fn single(a: Addr) -> Self {
        Self { start: a, end: a }
    }

    /// Reads `A1:C3` or `A1`.
    pub fn parse(s: &str) -> Option<Range> {
        match s.split_once(':') {
            Some((a, b)) => Some(Range::new(Addr::parse(a.trim())?, Addr::parse(b.trim())?)),
            None => Addr::parse(s.trim()).map(Range::single),
        }
    }

    pub fn contains(&self, a: Addr) -> bool {
        a.col >= self.start.col
            && a.col <= self.end.col
            && a.row >= self.start.row
            && a.row <= self.end.row
    }

    pub fn width(&self) -> u32 {
        self.end.col - self.start.col + 1
    }

    pub fn height(&self) -> u32 {
        self.end.row - self.start.row + 1
    }

    pub fn len(&self) -> u64 {
        u64::from(self.width()) * u64::from(self.height())
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Its cells, row by row.
    pub fn cells(&self) -> impl Iterator<Item = Addr> + '_ {
        (self.start.row..=self.end.row)
            .flat_map(move |r| (self.start.col..=self.end.col).map(move |c| Addr::new(c, r)))
    }

    /// `A1:C3`, or `A1` for one cell.
    pub fn name(&self) -> String {
        if self.start == self.end {
            self.start.name()
        } else {
            format!("{}:{}", self.start.name(), self.end.name())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_named_as_in_every_spreadsheet() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(701), "ZZ");
        assert_eq!(col_name(702), "AAA");
        for c in [0, 1, 25, 26, 27, 700, 701, 702, 16_383] {
            assert_eq!(col_index(&col_name(c)), Some(c));
        }
    }

    #[test]
    fn addresses_read_and_written() {
        assert_eq!(Addr::parse("A1"), Some(Addr::new(0, 0)));
        assert_eq!(Addr::parse("$c$12"), Some(Addr::new(2, 11)));
        assert_eq!(parse_ref("B$3"), Some((Addr::new(1, 2), false, true)));
        assert_eq!(Addr::parse("A0"), None);
        assert_eq!(Addr::parse("A01"), None);
        assert_eq!(Addr::parse("ABCD1"), None);
        assert_eq!(Addr::parse("LOG"), None);
        assert_eq!(Addr::new(27, 9).name(), "AB10");
    }

    #[test]
    fn ranges_put_their_corners_in_order() {
        let r = Range::parse("C3:A1").unwrap();
        assert_eq!(r.name(), "A1:C3");
        assert_eq!((r.width(), r.height(), r.len()), (3, 3, 9));
        assert!(r.contains(Addr::new(1, 1)));
        assert!(!r.contains(Addr::new(3, 1)));
        let toutes: Vec<String> = Range::parse("A1:B2")
            .unwrap()
            .cells()
            .map(|a| a.name())
            .collect();
        assert_eq!(toutes, ["A1", "B1", "A2", "B2"]);
    }
}
