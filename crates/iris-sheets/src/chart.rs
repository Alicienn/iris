//! Charts drawn from a sheet's table: what each one shows ([`Chart`], kept in the
//! workbook's file), and the numbers it reads from the cells ([`Series`]).
//!
//! A chart reads a table: a row of headers, and every row under it as far as the sheet
//! goes, so rows added later show at once. It groups the rows by one or more columns
//! of categories and adds up a column of numbers, or counts the rows when it has none.

use crate::cell::Addr;
use crate::eval::Value;
use crate::file::{Values, Workbook};
use crate::format::{display, Format, Locale};
use crate::formula::Change;
use std::collections::HashMap;

/// The kinds of chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// Bands from a whole to its parts, step after step.
    #[default]
    Flow,
    /// Horizontal bars, one per category.
    Bars,
    /// Vertical columns, side by side for several series.
    Columns,
    /// A ring cut in shares.
    Donut,
    /// Lines through the categories in order.
    Line,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Flow,
        Kind::Bars,
        Kind::Columns,
        Kind::Donut,
        Kind::Line,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Flow => "Flow",
            Kind::Bars => "Bars",
            Kind::Columns => "Columns",
            Kind::Donut => "Donut",
            Kind::Line => "Line",
        }
    }

    /// As written in the file.
    pub fn key(self) -> &'static str {
        match self {
            Kind::Flow => "flow",
            Kind::Bars => "bars",
            Kind::Columns => "columns",
            Kind::Donut => "donut",
            Kind::Line => "line",
        }
    }

    pub fn from_key(k: &str) -> Kind {
        Kind::ALL
            .into_iter()
            .find(|x| x.key() == k)
            .unwrap_or_default()
    }

    pub fn index(self) -> usize {
        Kind::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// A flow goes through several columns of categories; the others read one.
    pub fn many_groups(self) -> bool {
        self == Kind::Flow
    }

    /// Columns and lines draw several series; the others one.
    pub fn many_values(self) -> bool {
        matches!(self, Kind::Columns | Kind::Line)
    }
}

/// A chart of a workbook.
#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    pub name: String,
    pub kind: Kind,
    /// The sheet it reads, by name.
    pub sheet: String,
    /// The row of the table's headers (from 0); its data are the rows under it.
    pub header: u32,
    /// The columns of categories, in order: a flow goes through them.
    pub groups: Vec<u32>,
    /// The columns of numbers added up; none, and the rows are counted.
    pub values: Vec<u32>,
}

impl Chart {
    /// Keeps what the kind can show: one column of categories but for a flow, one
    /// series but for columns and lines.
    pub fn fit(&mut self) {
        if !self.kind.many_groups() {
            self.groups.truncate(1);
        }
        if !self.kind.many_values() {
            self.values.truncate(1);
        }
    }

    /// Rows or columns inserted or deleted in its sheet: it follows its table.
    pub fn follow(&mut self, change: Change) {
        match change {
            Change::InsertRows { at, n } => {
                if self.header >= at {
                    self.header += n;
                }
            }
            Change::DeleteRows { at, n } => {
                if self.header >= at + n {
                    self.header -= n;
                } else if self.header >= at {
                    self.header = at;
                }
            }
            Change::InsertCols { at, n } => {
                for c in self.groups.iter_mut().chain(self.values.iter_mut()) {
                    if *c >= at {
                        *c += n;
                    }
                }
            }
            Change::DeleteCols { at, n } => {
                let suivre = |v: &mut Vec<u32>| {
                    v.retain(|c| *c < at || *c >= at + n);
                    for c in v.iter_mut() {
                        if *c >= at + n {
                            *c -= n;
                        }
                    }
                };
                suivre(&mut self.groups);
                suivre(&mut self.values);
            }
        }
    }
}

/// A name a chart may take: not empty, not read as a range (`A1:B4` is one), no `]`
/// or `|` (they end an embed).
pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty()
        && !n.contains([']', '|', '[', '#'])
        && crate::cell::Range::parse(n).is_none()
        && Addr::parse(n).is_none()
}

/// A category and what it adds up to in each series.
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub label: String,
    pub values: Vec<f64>,
    /// The colour of its cells' text, `0xrrggbb`, when they have one.
    pub colour: Option<u32>,
}

/// A step of a flow.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub label: String,
    /// Its column: 0 the first.
    pub depth: usize,
    pub value: f64,
    pub colour: Option<u32>,
}

/// A band of a flow, from node to node.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub from: usize,
    pub to: usize,
    pub value: f64,
}

/// What a chart draws, read from its sheet.
#[derive(Debug, Clone, PartialEq)]
pub enum Series {
    Categories {
        /// Each series' name: its column's header ("Count" when the rows are counted).
        names: Vec<String>,
        items: Vec<Category>,
        /// The numbers' format: the first value's cell's.
        format: Format,
    },
    Flow {
        nodes: Vec<Node>,
        links: Vec<Link>,
        format: Format,
    },
    /// Nothing to draw, and why.
    Empty(String),
}

/// At most this many categories show; the others are added up as "Other".
const MAX_PARTS: usize = 12;
/// At most this many steps in a column of a flow.
const MAX_STEPS: usize = 10;
/// At most this many points along a line or columns.
const MAX_POINTS: usize = 400;

fn hex(s: &str) -> Option<u32> {
    let h = s.trim_start_matches('#');
    u32::from_str_radix(h, 16).ok().filter(|_| h.len() == 6)
}

/// A row's way through a flow: each step's name and colour, and what it carries.
type Chemin = (Vec<(String, Option<u32>)>, f64);

/// The rows a chart reads: each one's categories (shown as the table shows them, with
/// their text's colour) and its numbers.
struct Ligne {
    groups: Vec<(String, Option<u32>)>,
    values: Vec<f64>,
}

fn lignes(book: &Workbook, values: &Values, chart: &Chart, l: &Locale) -> Option<Vec<Ligne>> {
    let si = book.sheet_index(&chart.sheet)?;
    let s = &book.sheets[si];
    let (_, rows) = s.extent();
    let mut sortie = Vec::new();
    for row in chart.header + 1..rows {
        let groups: Vec<(String, Option<u32>)> = chart
            .groups
            .iter()
            .map(|&c| {
                let a = Addr::new(c, row);
                let f = s.format(a);
                let texte = display(&values.get(book, si, a), &f, l);
                (texte.trim().to_string(), f.color.as_deref().and_then(hex))
            })
            .collect();
        if groups.first().is_none_or(|(t, _)| t.is_empty()) {
            continue;
        }
        let nombres = chart
            .values
            .iter()
            .map(|&c| match values.get(book, si, Addr::new(c, row)) {
                Value::Number(n) if n.is_finite() => n,
                Value::Bool(b) => f64::from(u8::from(b)),
                _ => 0.0,
            })
            .collect();
        sortie.push(Ligne {
            groups,
            values: nombres,
        });
    }
    Some(sortie)
}

/// A header's text, or its column's name when it has none.
fn entete(book: &Workbook, values: &Values, chart: &Chart, col: u32, l: &Locale) -> String {
    let Some(si) = book.sheet_index(&chart.sheet) else {
        return crate::cell::col_name(col);
    };
    let a = Addr::new(col, chart.header);
    let t = display(&values.get(book, si, a), &book.sheets[si].format(a), l);
    if t.trim().is_empty() {
        crate::cell::col_name(col)
    } else {
        t.trim().to_string()
    }
}

/// The format numbers are shown in: the first value's cell's, under the headers.
fn format_des_valeurs(book: &Workbook, chart: &Chart) -> Format {
    let (Some(si), Some(&c)) = (book.sheet_index(&chart.sheet), chart.values.first()) else {
        return Format::default();
    };
    let mut f = book.sheets[si].format(Addr::new(c, chart.header + 1));
    // What the cell looks like is not the number's: no bold, no colour in a label.
    f.bold = false;
    f.color = None;
    f.fill = None;
    f
}

/// What `chart` draws from the workbook now.
pub fn series(book: &Workbook, values: &Values, chart: &Chart, l: &Locale) -> Series {
    if chart.groups.is_empty() {
        return Series::Empty("Pick a column of categories.".into());
    }
    let Some(lignes) = lignes(book, values, chart, l) else {
        return Series::Empty(format!("The sheet “{}” is gone.", chart.sheet));
    };
    if lignes.is_empty() {
        return Series::Empty("Nothing under the headers yet.".into());
    }
    let format = format_des_valeurs(book, chart);
    if chart.kind == Kind::Flow {
        return flux(book, values, chart, &lignes, format, l);
    }
    let names: Vec<String> = if chart.values.is_empty() {
        vec!["Count".into()]
    } else {
        chart
            .values
            .iter()
            .map(|&c| entete(book, values, chart, c, l))
            .collect()
    };
    let n = names.len();
    let mut items: Vec<Category> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for ligne in &lignes {
        let (label, colour) = ligne.groups[0].clone();
        let k = *index.entry(label.clone()).or_insert_with(|| {
            items.push(Category {
                label,
                values: vec![0.0; n],
                colour: None,
            });
            items.len() - 1
        });
        if items[k].colour.is_none() {
            items[k].colour = colour;
        }
        if chart.values.is_empty() {
            items[k].values[0] += 1.0;
        } else {
            for (s, v) in ligne.values.iter().enumerate() {
                items[k].values[s] += v;
            }
        }
    }
    match chart.kind {
        // Shares and counts read best largest first.
        Kind::Donut | Kind::Bars => {
            if chart.kind == Kind::Donut || chart.values.is_empty() {
                items.sort_by(|a, b| b.values[0].total_cmp(&a.values[0]));
            }
            if items.len() > MAX_PARTS {
                let reste: f64 = items[MAX_PARTS - 1..].iter().map(|c| c.values[0]).sum();
                items.truncate(MAX_PARTS - 1);
                items.push(Category {
                    label: "Other".into(),
                    values: vec![reste],
                    colour: None,
                });
            }
        }
        _ => items.truncate(MAX_POINTS),
    }
    Series::Categories {
        names,
        items,
        format,
    }
}

/// A flow. Its rows go from a whole (every row) through each column of categories in
/// turn; a category written `Sent > Interview` is two steps. When the second column
/// names things of the first (Income → Net, Net → Rent), the rows are bands from one
/// to the other, chained.
fn flux(
    book: &Workbook,
    values: &Values,
    chart: &Chart,
    lignes: &[Ligne],
    format: Format,
    l: &Locale,
) -> Series {
    let valeur = |li: &Ligne| {
        if chart.values.is_empty() {
            1.0
        } else {
            li.values.first().copied().unwrap_or(0.0)
        }
    };
    let chaine = chart.groups.len() == 2 && {
        let premiers: std::collections::HashSet<&str> =
            lignes.iter().map(|li| li.groups[0].0.as_str()).collect();
        lignes
            .iter()
            .any(|li| !li.groups[1].0.is_empty() && premiers.contains(li.groups[1].0.as_str()))
    };
    // Each row as the steps it goes through, with its colours.
    let chemins: Vec<Chemin> = if chaine {
        lignes
            .iter()
            .filter(|li| !li.groups[1].0.is_empty() && li.groups[0].0 != li.groups[1].0)
            .map(|li| (li.groups.clone(), valeur(li)))
            .filter(|(_, v)| *v > 0.0)
            .collect()
    } else {
        let tout = if chart.values.is_empty() {
            "All".to_string()
        } else {
            entete(book, values, chart, chart.values[0], l)
        };
        lignes
            .iter()
            .map(|li| {
                let mut pas = vec![(tout.clone(), None)];
                for (t, c) in &li.groups {
                    for morceau in t.split('>').map(str::trim).filter(|m| !m.is_empty()) {
                        pas.push((morceau.to_string(), *c));
                    }
                }
                (pas, valeur(li))
            })
            .filter(|(p, v)| p.len() > 1 && *v > 0.0)
            .collect()
    };
    if chemins.is_empty() {
        return Series::Empty("Nothing to draw: no row goes anywhere.".into());
    }
    if chaine {
        return chaine_de(&chemins, format);
    }
    // Each column's steps by their size; past the most a column shows, "Other".
    let mut tailles: HashMap<(usize, String), f64> = HashMap::new();
    for (p, v) in &chemins {
        for (d, (t, _)) in p.iter().enumerate() {
            *tailles.entry((d, t.clone())).or_default() += v;
        }
    }
    let mut gardes: HashMap<usize, Vec<(String, f64)>> = HashMap::new();
    for ((d, t), v) in &tailles {
        gardes.entry(*d).or_default().push((t.clone(), *v));
    }
    for liste in gardes.values_mut() {
        liste.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        if liste.len() > MAX_STEPS {
            liste.truncate(MAX_STEPS - 1);
        }
    }
    let garde = |d: usize, t: &str| {
        gardes
            .get(&d)
            .is_some_and(|l| l.iter().any(|(x, _)| x == t))
    };
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<(usize, String), usize> = HashMap::new();
    let mut liens: HashMap<(usize, usize), f64> = HashMap::new();
    for (p, v) in &chemins {
        let mut avant: Option<usize> = None;
        for (d, (t, c)) in p.iter().enumerate() {
            let label = if garde(d, t) {
                t.clone()
            } else {
                "Other".into()
            };
            let k = *index.entry((d, label.clone())).or_insert_with(|| {
                nodes.push(Node {
                    label,
                    depth: d,
                    value: 0.0,
                    colour: *c,
                });
                nodes.len() - 1
            });
            nodes[k].value += v;
            if nodes[k].colour.is_none() {
                nodes[k].colour = *c;
            }
            if let Some(a) = avant {
                *liens.entry((a, k)).or_default() += v;
            }
            avant = Some(k);
        }
    }
    let mut links: Vec<Link> = liens
        .into_iter()
        .map(|((from, to), value)| Link { from, to, value })
        .collect();
    links.sort_by_key(|l| (l.from, l.to));
    Series::Flow {
        nodes,
        links,
        format,
    }
}

/// Whether the bands lead from `a` to `b`.
fn mene(links: &[Link], a: usize, b: usize) -> bool {
    let mut pile = vec![a];
    let mut vus = std::collections::HashSet::new();
    while let Some(k) = pile.pop() {
        if k == b {
            return true;
        }
        if vus.insert(k) {
            pile.extend(links.iter().filter(|l| l.from == k).map(|l| l.to));
        }
    }
    false
}

/// Bands from one category to another: `Income → Net`, `Net → Rent`. Each name is one
/// step, placed after every step that flows into it; a band going back is left out.
fn chaine_de(chemins: &[Chemin], format: Format) -> Series {
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    // The bands in the order of the rows; one that would lead back to where it starts
    // is left out.
    let mut links: Vec<Link> = Vec::new();
    let mut noeud = |t: &str, c: Option<u32>, nodes: &mut Vec<Node>| -> usize {
        *index.entry(t.to_string()).or_insert_with(|| {
            nodes.push(Node {
                label: t.to_string(),
                depth: 0,
                value: 0.0,
                colour: c,
            });
            nodes.len() - 1
        })
    };
    for (p, v) in chemins {
        let a = noeud(&p[0].0, p[0].1, &mut nodes);
        let b = noeud(&p[1].0, p[1].1, &mut nodes);
        if let Some(l) = links.iter_mut().find(|l| l.from == a && l.to == b) {
            l.value += v;
        } else if !mene(&links, b, a) {
            links.push(Link {
                from: a,
                to: b,
                value: *v,
            });
        }
    }
    // Each step after every step flowing into it.
    let n = nodes.len();
    let mut prof = vec![0usize; n];
    for _ in 0..n {
        let mut bouge = false;
        for l in &links {
            if prof[l.to] < prof[l.from] + 1 {
                prof[l.to] = prof[l.from] + 1;
                bouge = true;
            }
        }
        if !bouge {
            break;
        }
    }
    links.sort_by_key(|l| (l.from, l.to));
    let mut entree = vec![0.0; n];
    let mut sortie = vec![0.0; n];
    for l in &links {
        sortie[l.from] += l.value;
        entree[l.to] += l.value;
    }
    for (k, nd) in nodes.iter_mut().enumerate() {
        nd.depth = prof[k];
        nd.value = f64::max(entree[k], sortie[k]);
    }
    Series::Flow {
        nodes,
        links,
        format,
    }
}

/// A chart made from the cells selected: what they hold decides its kind. One column of
/// words: a flow of them; words and numbers: bars (columns for several numbers, a line
/// when the words are dates or numbers); the first row of the selection is the headers.
pub fn guess(book: &Workbook, values: &Values, sheet: usize, sel: crate::cell::Range) -> Chart {
    let s = &book.sheets[sheet];
    let (_, rows) = s.extent();
    let header = sel.start.row;
    let dernier = if sel.height() > 1 {
        sel.end.row
    } else {
        rows.max(header + 1) - 1
    };
    // What each column holds under its header: numbers, or words.
    let mut mots = Vec::new();
    let mut nombres = Vec::new();
    let mut dates = false;
    for c in sel.start.col..=sel.end.col {
        let mut n = 0;
        let mut t = 0;
        for r in header + 1..=dernier.max(header + 1) {
            match values.get(book, sheet, Addr::new(c, r)) {
                Value::Number(_) => n += 1,
                Value::Text(x) if !x.trim().is_empty() => t += 1,
                _ => {}
            }
        }
        if n > 0 && n >= t {
            // A first column of numbers formatted as dates names the points of a line.
            let f = s.format(Addr::new(c, header + 1));
            let date = matches!(
                f.number,
                crate::format::NumberFormat::Date | crate::format::NumberFormat::DateTime
            );
            if mots.is_empty() && (date || (sel.width() > 1 && c == sel.start.col)) {
                dates = true;
                mots.push(c);
            } else {
                nombres.push(c);
            }
        } else if t > 0 {
            mots.push(c);
        }
    }
    let (kind, groups, valeurs) = match (mots.len(), nombres.len()) {
        (0, 0) => (Kind::Flow, vec![sel.start.col], Vec::new()),
        (_, 0) => (Kind::Flow, mots.into_iter().take(3).collect(), Vec::new()),
        (0, _) => (Kind::Line, vec![nombres[0]], nombres[1..].to_vec()),
        (_, 1) if !dates => (Kind::Bars, vec![mots[0]], nombres),
        _ if dates => (Kind::Line, vec![mots[0]], nombres),
        _ => (Kind::Columns, vec![mots[0]], nombres),
    };
    let base = entete(
        book,
        values,
        &Chart {
            name: String::new(),
            kind,
            sheet: s.name.clone(),
            header,
            groups: Vec::new(),
            values: Vec::new(),
        },
        groups[0],
        &Locale::english(),
    );
    let mut name = if valid_name(&base) {
        base
    } else {
        "Chart".into()
    };
    if book
        .charts
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(&name))
    {
        name = (2..)
            .map(|k| format!("{name} {k}"))
            .find(|n| !book.charts.iter().any(|c| c.name.eq_ignore_ascii_case(n)))
            .unwrap_or_default();
    }
    let mut chart = Chart {
        name,
        kind,
        sheet: s.name.clone(),
        header,
        groups,
        values: valeurs,
    };
    chart.fit();
    chart
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Range;

    fn a(s: &str) -> Addr {
        Addr::parse(s).unwrap()
    }

    /// A table of requests: a status, a city, an amount.
    fn classeur() -> Workbook {
        let mut b = Workbook::default();
        let s = &mut b.sheets[0];
        for (k, (st, ville, m)) in [
            ("Status", "City", "Amount"),
            ("Sent", "Paris", "10"),
            ("Sent", "Lyon", "5"),
            ("Refused", "Paris", "2"),
            ("Sent > Interview", "Paris", "1"),
            ("", "Nantes", "7"),
        ]
        .into_iter()
        .enumerate()
        {
            let r = k + 1;
            if !st.is_empty() {
                s.set_input(a(&format!("A{r}")), st);
            }
            s.set_input(a(&format!("B{r}")), ville);
            s.set_input(a(&format!("C{r}")), m);
        }
        s.set_format(Range::parse("A3").unwrap(), |f| {
            f.color = Some("#d93025".into())
        });
        b
    }

    fn graphique(kind: Kind, groups: Vec<u32>, values: Vec<u32>) -> Chart {
        Chart {
            name: "Status".into(),
            kind,
            sheet: "Sheet1".into(),
            header: 0,
            groups,
            values,
        }
    }

    #[test]
    fn rows_are_counted_by_category_largest_first() {
        let b = classeur();
        let v = b.compute();
        let Series::Categories { names, items, .. } = series(
            &b,
            &v,
            &graphique(Kind::Bars, vec![0], vec![]),
            &Locale::english(),
        ) else {
            panic!("categories");
        };
        assert_eq!(names, vec!["Count"]);
        let vus: Vec<(&str, f64)> = items
            .iter()
            .map(|c| (c.label.as_str(), c.values[0]))
            .collect();
        // The row without a status is left out.
        assert_eq!(
            vus,
            vec![("Sent", 2.0), ("Refused", 1.0), ("Sent > Interview", 1.0)]
        );
        // A category takes its cells' colour.
        assert_eq!(items[0].colour, Some(0xd93025));
    }

    #[test]
    fn a_column_of_numbers_is_added_up_in_the_order_of_the_sheet() {
        let b = classeur();
        let v = b.compute();
        let Series::Categories { names, items, .. } = series(
            &b,
            &v,
            &graphique(Kind::Columns, vec![1], vec![2]),
            &Locale::english(),
        ) else {
            panic!("categories");
        };
        assert_eq!(names, vec!["Amount"]);
        let vus: Vec<(&str, f64)> = items
            .iter()
            .map(|c| (c.label.as_str(), c.values[0]))
            .collect();
        // Grouped by city: the row without a status counts too.
        assert_eq!(vus, vec![("Paris", 13.0), ("Lyon", 5.0), ("Nantes", 7.0)]);
    }

    #[test]
    fn a_flow_goes_from_the_whole_through_each_step() {
        let b = classeur();
        let v = b.compute();
        let Series::Flow { nodes, links, .. } = series(
            &b,
            &v,
            &graphique(Kind::Flow, vec![0], vec![]),
            &Locale::english(),
        ) else {
            panic!("flow");
        };
        let nom = |k: usize| (nodes[k].label.as_str(), nodes[k].depth);
        assert_eq!(nom(0), ("All", 0));
        assert_eq!(nodes[0].value, 4.0);
        // `Sent > Interview` is two steps: Sent holds 3 rows, one of which goes on.
        let sent = nodes.iter().position(|n| n.label == "Sent").unwrap();
        assert_eq!(nodes[sent].value, 3.0);
        let interview = nodes.iter().position(|n| n.label == "Interview").unwrap();
        assert_eq!(nodes[interview].depth, 2);
        assert!(links
            .iter()
            .any(|l| l.from == sent && l.to == interview && l.value == 1.0));
        assert!(links
            .iter()
            .any(|l| l.from == 0 && l.to == sent && l.value == 3.0));
    }

    #[test]
    fn a_second_column_naming_the_first_chains_the_bands() {
        let mut b = Workbook::default();
        let s = &mut b.sheets[0];
        for (k, (de, vers, m)) in [
            ("From", "To", "Amount"),
            ("Income", "Net", "70"),
            ("Income", "Taxes", "30"),
            ("Net", "Rent", "40"),
            ("Net", "Savings", "30"),
            ("Rent", "Income", "5"),
        ]
        .into_iter()
        .enumerate()
        {
            s.set_input(a(&format!("A{}", k + 1)), de);
            s.set_input(a(&format!("B{}", k + 1)), vers);
            s.set_input(a(&format!("C{}", k + 1)), m);
        }
        let v = b.compute();
        let Series::Flow { nodes, links, .. } = series(
            &b,
            &v,
            &graphique(Kind::Flow, vec![0, 1], vec![2]),
            &Locale::english(),
        ) else {
            panic!("flow");
        };
        let p = |t: &str| nodes.iter().find(|n| n.label == t).unwrap();
        assert_eq!(
            (p("Income").depth, p("Net").depth, p("Rent").depth),
            (0, 1, 2)
        );
        assert_eq!(p("Net").value, 70.0);
        // The band going back (Rent → Income) is left out.
        assert_eq!(links.len(), 4);
    }

    #[test]
    fn a_chart_follows_its_table_when_columns_move() {
        let mut c = graphique(Kind::Columns, vec![1], vec![2, 4]);
        c.follow(Change::InsertCols { at: 0, n: 1 });
        assert_eq!((c.groups.clone(), c.values.clone()), (vec![2], vec![3, 5]));
        c.follow(Change::DeleteCols { at: 3, n: 1 });
        assert_eq!(c.values, vec![4]);
        c.follow(Change::InsertRows { at: 0, n: 2 });
        assert_eq!(c.header, 2);
    }

    #[test]
    fn the_cells_selected_suggest_the_chart() {
        let b = classeur();
        let v = b.compute();
        let c = guess(&b, &v, 0, Range::parse("A1").unwrap());
        assert_eq!(
            (c.kind, c.groups.clone(), c.name.as_str()),
            (Kind::Flow, vec![0], "Status")
        );
        let c = guess(&b, &v, 0, Range::parse("B1:C6").unwrap());
        assert_eq!(
            (c.kind, c.groups.clone(), c.values.clone()),
            (Kind::Bars, vec![1], vec![2])
        );
        assert!(!valid_name("A1:B2"));
        assert!(!valid_name("B7"));
        assert!(valid_name("Status"));
    }
}
