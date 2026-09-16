//! Cell and range coordinates.
//!
//!
//! **Indices are 0-based inside the crate**, unlike the file formats where
//! everything is 1-based. Conversion happens only at the boundary: when
//! parsing `A1` and when rendering it back. The [`Row`] and [`Col`] newtypes
//! stop the two from being swapped.

use crate::error::{Error, Result};

/// Highest row number of an Excel sheet, 1-based (`AddressRange::MAX_ROW`).
pub const MAX_ROW: u32 = 1_048_576;
/// Highest column number of an Excel sheet, 1-based - column `XFD`.
pub const MAX_COL: u32 = 16_384;

/// A row index, 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Row(u32);

/// A column index, 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Col(u32);

impl Row {
    /// From a 0-based index. `None` if the row lies outside the sheet.
    #[must_use]
    pub const fn new(index0: u32) -> Option<Self> {
        if index0 < MAX_ROW {
            Some(Self(index0))
        } else {
            None
        }
    }

    /// From the number an Excel user sees (1-based).
    ///
    /// # Errors
    /// [`Error::RowOutOfRange`] if the number is zero or above [`MAX_ROW`].
    pub fn from_one_based(n: u64) -> Result<Self> {
        u32::try_from(n)
            .ok()
            .filter(|&n| (1..=MAX_ROW).contains(&n))
            .map(|n| Self(n - 1))
            .ok_or(Error::RowOutOfRange(n))
    }

    /// The 0-based index.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }

    /// The 1-based number, for display and for writing to a file.
    #[must_use]
    pub const fn one_based(self) -> u32 {
        self.0 + 1
    }
}

impl Col {
    /// From a 0-based index. `None` if the column lies outside the sheet.
    #[must_use]
    pub const fn new(index0: u32) -> Option<Self> {
        if index0 < MAX_COL {
            Some(Self(index0))
        } else {
            None
        }
    }

    /// From the number Excel's `COLUMN()` returns (1-based, `A` = 1).
    ///
    /// # Errors
    /// [`Error::ColOutOfRange`] if the number is zero or above [`MAX_COL`].
    pub fn from_one_based(n: u64) -> Result<Self> {
        u32::try_from(n)
            .ok()
            .filter(|&n| (1..=MAX_COL).contains(&n))
            .map(|n| Self(n - 1))
            .ok_or(Error::ColOutOfRange(n))
    }

    /// The 0-based index.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }

    /// The 1-based number, as returned by `COLUMN()`.
    #[must_use]
    pub const fn one_based(self) -> u32 {
        self.0 + 1
    }

    /// Parses a column label: `A` -> 0, `AA` -> 26, `XFD` -> 16383.
    ///
    /// Case-insensitive.
    ///
    /// # Errors
    /// [`Error::ColOutOfRange`] for an empty string, non-letters, more than
    /// three letters, or anything past `XFD`.
    pub fn from_letters(s: &str) -> Result<Self> {
        let bytes = s.as_bytes();
        if bytes.is_empty() || bytes.len() > 3 {
            return Err(Error::ColOutOfRange(0));
        }
        let mut n: u32 = 0;
        for &b in bytes {
            let v = match b.to_ascii_uppercase() {
                b @ b'A'..=b'Z' => u32::from(b - b'A') + 1,
                _ => return Err(Error::ColOutOfRange(0)),
            };
            // At most three letters, so a u32 overflow is unreachable.
            n = n * 26 + v;
        }
        Self::from_one_based(u64::from(n))
    }

    /// Renders the column label: 0 -> `A`, 26 -> `AA`, 16383 -> `XFD`.
    ///
    #[must_use]
    pub fn to_letters(self) -> String {
        let mut n = self.one_based();
        let mut out = [0u8; 3];
        let mut len = 0;
        while n > 0 {
            // Bijective base-26: there is no zero digit, so a remainder of 0
            // means 'Z' and borrows from the next position.
            let rem = match n % 26 {
                0 => 26,
                r => r,
            };
            out[3 - 1 - len] = b'A' + u8::try_from(rem - 1).unwrap_or(0);
            len += 1;
            n = (n - rem) / 26;
        }
        String::from_utf8_lossy(&out[3 - len..]).into_owned()
    }
}

/// A reference to a single cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CellRef {
    /// The column.
    pub col: Col,
    /// The row.
    pub row: Row,
}

impl CellRef {
    /// A cell reference from a column and a row.
    #[must_use]
    pub const fn new(col: Col, row: Row) -> Self {
        Self { col, row }
    }

    /// Parses `A1`, `$A$1` or `a1`.
    ///
    /// `$` signs are accepted and dropped: absoluteness only matters to
    /// formulas, and it will arrive together with them.
    ///
    /// `A0` is rejected: Excel has no row zero, and a reader that accepts one
    /// only postpones the confusion to whoever reads the value back.
    ///
    /// # Errors
    /// [`Error::InvalidCellRef`] if the string is not a cell reference or falls
    /// outside the sheet.
    pub fn parse(s: &str) -> Result<Self> {
        let err = || Error::InvalidCellRef(s.to_owned());
        let letters = s.trim_start_matches('$');
        let split = letters.find(|c: char| !c.is_ascii_alphabetic());
        let (col, rest) = letters.split_at(split.ok_or_else(err)?);
        let digits = rest.strip_prefix('$').unwrap_or(rest);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        let row: u64 = digits.parse().map_err(|_| err())?;
        Ok(Self {
            col: Col::from_letters(col).map_err(|_| err())?,
            row: Row::from_one_based(row).map_err(|_| err())?,
        })
    }
}

impl core::fmt::Display for CellRef {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}{}", self.col.to_letters(), self.row.one_based())
    }
}

/// A rectangular range of cells, both corners inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Range {
    /// Top-left corner.
    pub start: CellRef,
    /// Bottom-right corner.
    pub end: CellRef,
}

impl Range {
    /// A range from two corners; they are normalised, so their order is free.
    #[must_use]
    pub fn new(a: CellRef, b: CellRef) -> Self {
        Self {
            start: CellRef::new(a.col.min(b.col), a.row.min(b.row)),
            end: CellRef::new(a.col.max(b.col), a.row.max(b.row)),
        }
    }

    /// Parses `A1:B2`, a lone cell `B2`, a column range `B:C`, or a row
    /// range `2:3`.
    ///
    /// Unions and intersections (`A1:B2,C3:D4`, or a space) are not handled
    /// here - that is formula syntax and belongs to its own phase.
    ///
    /// # Errors
    /// [`Error::InvalidRange`] if the string does not parse as a range.
    pub fn parse(s: &str) -> Result<Self> {
        let err = || Error::InvalidRange(s.to_owned());
        let (a, b) = s.split_once(':').unwrap_or((s, s));
        let (a, b) = (a.replace('$', ""), b.replace('$', ""));

        let all_digits = |t: &str| !t.is_empty() && t.bytes().all(|c| c.is_ascii_digit());
        let all_alpha = |t: &str| !t.is_empty() && t.bytes().all(|c| c.is_ascii_alphabetic());

        if all_digits(&a) && all_digits(&b) {
            let (r1, r2) = (
                Row::from_one_based(a.parse().map_err(|_| err())?).map_err(|_| err())?,
                Row::from_one_based(b.parse().map_err(|_| err())?).map_err(|_| err())?,
            );
            let last = Col::new(MAX_COL - 1).ok_or_else(err)?;
            return Ok(Self::new(
                CellRef::new(Col::default(), r1),
                CellRef::new(last, r2),
            ));
        }
        if all_alpha(&a) && all_alpha(&b) {
            let (c1, c2) = (
                Col::from_letters(&a).map_err(|_| err())?,
                Col::from_letters(&b).map_err(|_| err())?,
            );
            let last = Row::new(MAX_ROW - 1).ok_or_else(err)?;
            return Ok(Self::new(
                CellRef::new(c1, Row::default()),
                CellRef::new(c2, last),
            ));
        }
        Ok(Self::new(
            CellRef::parse(&a).map_err(|_| err())?,
            CellRef::parse(&b).map_err(|_| err())?,
        ))
    }

    /// Width in columns.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.end.col.index() - self.start.col.index() + 1
    }

    /// Height in rows.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.end.row.index() - self.start.row.index() + 1
    }

    /// Whether a cell lies inside the range.
    #[must_use]
    pub fn contains(&self, cell: CellRef) -> bool {
        (self.start.col..=self.end.col).contains(&cell.col)
            && (self.start.row..=self.end.row).contains(&cell.row)
    }

    /// Walks every cell of the range, row by row, left to right.
    pub fn cells(&self) -> impl Iterator<Item = CellRef> + use<> {
        let (c0, c1) = (self.start.col.index(), self.end.col.index());
        let (r0, r1) = (self.start.row.index(), self.end.row.index());
        (r0..=r1).flat_map(move |r| {
            (c0..=c1).filter_map(move |c| Some(CellRef::new(Col::new(c)?, Row::new(r)?)))
        })
    }
}

impl core::fmt::Display for Range {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}:{}", self.start, self.end)
    }
}

/// Shifts every relative reference in a formula by a number of columns and
/// rows.
///
///
/// Absolute parts (`$A$1`) stay put, quoted text and quoted sheet names are
/// left alone, and a reference pushed off the sheet becomes `#REF!`.
#[must_use]
pub fn shift_references(formula: &str, d_col: i64, d_row: i64) -> String {
    if d_col == 0 && d_row == 0 {
        return formula.to_owned();
    }
    scan_references(formula, |_, s| shift_one(s, d_col, d_row))
}

/// Walks a formula and lets `rewrite` replace every A1-style reference in it.
///
/// The scanning half of a reference rewrite: quoted text
/// and quoted sheet names pass through untouched, and a reference is only one
/// where a reference can begin - never inside a longer name. `rewrite` is told
/// which sheet the reference names, when it is qualified with one, and answers
/// with how many characters of `s` it consumed and what replaces them, or
/// `None` to leave the text alone.
pub(crate) fn scan_references(
    formula: &str,
    rewrite: impl FnMut(Option<&str>, &[char]) -> Option<(usize, String)>,
) -> String {
    scan_formula(formula, rewrite, |_| None)
}

/// The same walk, with the sheet qualifiers rewritable as well.
///
/// `rename` is handed the sheet names of each qualifier, one for `Sheet1!` and
/// two for the 3-D form `Sheet1:Sheet3!`, and answers with the text replacing
/// the whole qualifier *including its `!`*, or `None` to leave it alone. The
/// `!` belongs to the answer because a deleted sheet turns `Sheet2!A1` into
/// `#REF!A1`, where the bang is part of the error rather than a separator
/// after it.
pub(crate) fn scan_formula(
    formula: &str,
    mut rewrite: impl FnMut(Option<&str>, &[char]) -> Option<(usize, String)>,
    mut rename: impl FnMut(&[String]) -> Option<String>,
) -> String {
    let chars: Vec<char> = formula.chars().collect();
    let mut out = String::with_capacity(formula.len());
    let mut i = 0;
    // The sheet named just before the reference being read, if any.
    let mut qualifier: Option<String> = None;
    while i < chars.len() {
        let c = chars[i];
        // A reference cannot start in the middle of a longer name, and neither
        // can a sheet qualifier.
        let joined = i > 0 && (chars[i - 1].is_alphanumeric() || matches!(chars[i - 1], '_' | '.'));
        if !joined
            && c != '"'
            && let Some((len, names)) = parse_qualifier(&chars[i..])
        {
            if let Some(text) = rename(&names) {
                out.push_str(&text);
                qualifier = None;
            } else {
                out.extend(&chars[i..i + len]);
                qualifier = names.last().cloned();
            }
            i += len;
            continue;
        }
        // A string literal passes through untouched, and so does a quoted name
        // that turned out not to be a qualifier.
        if c == '"' || c == '\'' {
            out.push(c);
            i += 1;
            while i < chars.len() {
                out.push(chars[i]);
                i += 1;
                if chars[i - 1] == c {
                    break;
                }
            }
            qualifier = None;
            continue;
        }
        if !joined && let Some((len, text)) = rewrite(qualifier.as_deref(), &chars[i..]) {
            out.push_str(&text);
            i += len;
            qualifier = None;
            continue;
        }
        // A bare name: a function or a defined name.
        if !joined && (c.is_alphabetic() || c == '_') {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.')) {
                i += 1;
            }
            out.extend(&chars[start..i]);
            qualifier = None;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Reads a sheet qualifier at the front of `s`: `Sheet1!`, `'Sheet 1'!` or the
/// 3-D form with two names joined by `:`. Answers with how many characters it
/// spans, the `!` included, and the names it holds.
fn parse_qualifier(s: &[char]) -> Option<(usize, Vec<String>)> {
    let (mut i, first) = parse_sheet_name(s, 0)?;
    let mut names = vec![first];
    if s.get(i) == Some(&':')
        && let Some((after, second)) = parse_sheet_name(s, i + 1)
    {
        names.push(second);
        i = after;
    }
    (s.get(i) == Some(&'!')).then(|| (i + 1, names))
}

/// One sheet name at `from`, quoted or bare, and where it ends.
fn parse_sheet_name(s: &[char], from: usize) -> Option<(usize, String)> {
    if s.get(from) == Some(&'\'') {
        let mut i = from + 1;
        let mut name = String::new();
        while i < s.len() {
            if s[i] == '\'' {
                // A doubled apostrophe is one apostrophe of the name itself.
                if s.get(i + 1) == Some(&'\'') {
                    name.push('\'');
                    i += 2;
                    continue;
                }
                return Some((i + 1, name));
            }
            name.push(s[i]);
            i += 1;
        }
        return None;
    }
    let mut i = from;
    while i < s.len() && (s[i].is_alphanumeric() || matches!(s[i], '_' | '.')) {
        i += 1;
    }
    // A bare name cannot start with a digit, which is what tells `A1!` from a
    // sheet called `A1`; Excel quotes the latter for exactly this reason.
    let first = *s.get(from)?;
    if i == from || !(first.is_alphabetic() || first == '_') {
        return None;
    }
    Some((i, s[from..i].iter().collect()))
}

/// Shifts the reference at the front of `s`, returning how many characters it
/// spanned. `None` when what is there is not a reference at all.
fn shift_one(s: &[char], d_col: i64, d_row: i64) -> Option<(usize, String)> {
    let (len, r) = parse_ref_at(s)?;
    let new_col = if r.col_abs {
        Some(r.col)
    } else {
        u32::try_from(i64::from(r.col.index()) + d_col)
            .ok()
            .and_then(Col::new)
    };
    let new_row = if r.row_abs {
        Some(r.row)
    } else {
        u32::try_from(i64::from(r.row.index()) + d_row)
            .ok()
            .and_then(Row::new)
    };
    let text = match (new_col, new_row) {
        (Some(c), Some(row)) => r.render(c, row),
        // Excel does the same when a reference is moved off the sheet.
        _ => "#REF!".to_owned(),
    };
    Some((len, text))
}

/// One A1-style reference as it was written: where it points and which halves
/// of it carry a `$`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RefParts {
    pub(crate) col: Col,
    pub(crate) row: Row,
    pub(crate) col_abs: bool,
    pub(crate) row_abs: bool,
}

impl RefParts {
    /// The reference written out again, pointing at `col`/`row` and keeping
    /// the `$` signs it was written with.
    pub(crate) fn render(self, col: Col, row: Row) -> String {
        format!(
            "{}{}{}{}",
            if self.col_abs { "$" } else { "" },
            col.to_letters(),
            if self.row_abs { "$" } else { "" },
            row.one_based()
        )
    }
}

/// Reads the reference at the front of `s`, returning how many characters it
/// spanned. `None` when what is there is not a reference at all.
pub(crate) fn parse_ref_at(s: &[char]) -> Option<(usize, RefParts)> {
    let mut i = 0;
    let col_abs = s.first() == Some(&'$');
    if col_abs {
        i += 1;
    }
    let letters_at = i;
    // A column is at most three letters; a fourth means this is a name.
    while i < s.len() && s[i].is_ascii_alphabetic() && i - letters_at < 3 {
        i += 1;
    }
    let letters: String = s[letters_at..i].iter().collect();
    if letters.is_empty() {
        return None;
    }
    let row_abs = s.get(i) == Some(&'$');
    if row_abs {
        i += 1;
    }
    let digits_at = i;
    while i < s.len() && s[i].is_ascii_digit() && i - digits_at < 7 {
        i += 1;
    }
    let digits: String = s[digits_at..i].iter().collect();
    if digits.is_empty() {
        return None;
    }
    // What follows decides whether this was a reference or the start of
    // something longer: `LOG10(` is a function, `Table1[` a table, `Sheet1!` a
    // sheet, and a fourth letter or eighth digit means neither.
    if let Some(&next) = s.get(i)
        && (next.is_alphanumeric() || matches!(next, '_' | '.' | '(' | '[' | '!'))
    {
        return None;
    }

    let col = Col::from_letters(&letters).ok()?;
    let row = Row::from_one_based(digits.parse().ok()?).ok()?;
    Some((
        i,
        RefParts {
            col,
            row,
            col_abs,
            row_abs,
        },
    ))
}

/// Whether an address looks like a range rather than a single cell.
///
#[must_use]
pub fn is_range(address: &str) -> bool {
    address.contains(':') || address.contains(',')
}

#[cfg(test)]
mod tests {
    use super::{CellRef, Col, MAX_COL, MAX_ROW, Range, Row, is_range};
    use crate::error::Error;

    #[test]
    fn column_letters_roundtrip() {
        // Cases taken from Excel's own column labelling.
        for (letters, index1) in [
            ("A", 1u32),
            ("Z", 26),
            ("AA", 27),
            ("AZ", 52),
            ("BA", 53),
            ("ZZ", 702),
            ("AAA", 703),
            ("XFD", MAX_COL),
        ] {
            let col = Col::from_letters(letters).expect("valid column");
            assert_eq!(col.one_based(), index1, "parsing {letters}");
            assert_eq!(col.to_letters(), letters, "rendering {letters}");
        }
        assert_eq!(
            Col::from_letters("a").unwrap(),
            Col::from_letters("A").unwrap()
        );
    }

    #[test]
    fn column_bounds() {
        assert!(Col::from_letters("XFE").is_err(), "past XFD");
        assert!(Col::from_letters("AAAA").is_err(), "four letters");
        assert!(Col::from_letters("").is_err());
        assert!(Col::from_letters("A1").is_err());
        assert!(matches!(
            Col::from_one_based(0),
            Err(Error::ColOutOfRange(0))
        ));
    }

    #[test]
    fn cell_refs() {
        let parse = CellRef::parse;
        assert_eq!(parse("A1").unwrap().to_string(), "A1");
        assert_eq!(parse("$B$12").unwrap().to_string(), "B12");
        assert_eq!(parse("z10").unwrap().to_string(), "Z10");
        assert_eq!(parse("XFD1048576").unwrap().to_string(), "XFD1048576");

        assert!(parse("A0").is_err(), "row 0 is not an Excel coordinate");
        assert!(parse("1A").is_err());
        assert!(parse("A").is_err());
        assert!(parse("").is_err());
        assert!(parse("A1048577").is_err(), "past the last row");
        assert!(parse("A1:B2").is_err(), "a range is not a cell");
    }

    #[test]
    fn ranges() {
        let r = Range::parse("B4:D9").unwrap();
        assert_eq!((r.width(), r.height()), (3, 6));
        assert_eq!(r.to_string(), "B4:D9");

        let single = Range::parse("B2").unwrap();
        assert_eq!((single.width(), single.height()), (1, 1));

        let cols = Range::parse("B:C").unwrap();
        assert_eq!((cols.width(), cols.height()), (2, MAX_ROW));
        let rows = Range::parse("2:3").unwrap();
        assert_eq!((rows.width(), rows.height()), (MAX_COL, 2));

        assert_eq!(Range::parse("D9:B4").unwrap(), r, "corners are normalised");
        assert_eq!(Range::parse("$B$4:$D$9").unwrap(), r);
        assert!(Range::parse("B4:").is_err());
    }

    #[test]
    fn range_contains_and_iterates() {
        let r = Range::parse("B2:C3").unwrap();
        let cells: Vec<String> = r.cells().map(|c| c.to_string()).collect();
        assert_eq!(cells, ["B2", "C2", "B3", "C3"]);
        assert!(cells.iter().all(|c| r.contains(CellRef::parse(c).unwrap())));
        assert!(!r.contains(CellRef::parse("A1").unwrap()));
        assert!(!r.contains(CellRef::parse("D2").unwrap()));
    }

    #[test]
    fn row_bounds() {
        assert_eq!(Row::from_one_based(1).unwrap().index(), 0);
        assert_eq!(
            Row::from_one_based(u64::from(MAX_ROW)).unwrap().index(),
            MAX_ROW - 1
        );
        assert!(Row::from_one_based(0).is_err());
        assert!(Row::from_one_based(u64::from(MAX_ROW) + 1).is_err());
    }

    #[test]
    fn range_detection() {
        assert!(is_range("A1:A2"));
        assert!(is_range("A1:A2,C1:C2"));
        assert!(!is_range("A1"));
    }

    #[test]
    fn shared_formulas_shift_only_what_may_move() {
        use super::shift_references;
        let shift = |f: &str| shift_references(f, 1, 2);

        assert_eq!(shift("A1+B2"), "B3+C4");
        assert_eq!(shift("$A$1+A$1+$A1"), "$A$1+B$1+$A3", "absolute parts stay");
        assert_eq!(shift("SUM(A1:C3)"), "SUM(B3:D5)");
        assert_eq!(
            shift("LOG10(A1)+B2"),
            "LOG10(B3)+C4",
            "a function name is not a reference"
        );
        assert_eq!(
            shift("'Лист 1'!A1+Sheet2!B2"),
            "'Лист 1'!B3+Sheet2!C4",
            "quoted and bare sheet names survive, their cells still move"
        );
        assert_eq!(
            shift(r#"IF(A1="B2",A1,"")"#),
            r#"IF(B3="B2",B3,"")"#,
            "text in quotes is left alone"
        );
        assert_eq!(shift("Table1[Amount]"), "Table1[Amount]");
        assert_eq!(shift("1E5+A1"), "1E5+B3", "not a reference, an exponent");
        assert_eq!(
            shift("XFD1"),
            "#REF!",
            "pushed off the right edge of the sheet"
        );
        assert_eq!(shift_references("A1", 0, 0), "A1", "no shift, no work");
        assert_eq!(
            shift_references("B2", -5, 0),
            "#REF!",
            "pushed off the left edge"
        );
    }
}
