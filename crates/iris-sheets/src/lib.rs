//! `iris-sheets` — spreadsheets, without a window.
//!
//! A `.sheet` file is a workbook of sheets of cells. Each cell keeps what was typed in
//! it (`12`, `Paris`, `=SUM(A1:A3)`) and its format; values are computed when the file
//! is read, every formula after those it reads ([`eval::recalc`]). The same workbook
//! is read from and written to CSV and Excel files.

#![forbid(unsafe_code)]

pub mod cell;
pub mod csv;
pub mod eval;
pub mod file;
pub mod format;
pub mod formula;
pub mod xlsx;

pub use cell::{col_name, Addr, Range};
pub use eval::{CellError, Value};
pub use file::{Cell, Sheet, Values, Workbook};
pub use format::{Align, Format, Locale, NumberFormat};
