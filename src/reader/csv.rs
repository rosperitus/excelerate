//! Reading CSV.
//!
//! A CSV file says nothing about itself: no encoding, no delimiter, no types.
//! Everything below is a guess made the way Excel makes it —
//! the byte order mark decides the encoding, a leading `sep=;` line or the
//! statistics of the first thousand lines decide the delimiter, and the shape
//! of each field decides whether it is a number, a boolean, an error or text.

use crate::error::{CellError, Error, Result};
use crate::model::{CellValue, Spreadsheet, Worksheet};
use crate::{CellRef, Col, Row};

/// Delimiters the reader will consider, best guess first.
///
const CANDIDATES: [char; 7] = [',', ';', '\t', '|', ':', ' ', '~'];

/// How many lines the delimiter is inferred from.
const INFERENCE_LINES: usize = 1000;

/// The field quote. Some readers let this be configured; no real file uses
/// anything else, so it is a constant until one turns up.
const ENCLOSURE: char = '"';

/// How a field written the way a person writes a number is read.
///
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedNumbers {
    /// What separates the fraction, `,` where `.` is not used.
    pub decimal: char,
    /// What groups the thousands, and is dropped before parsing.
    pub thousands: char,
    /// Whether the cell keeps a number format showing it the way it was
    /// written.
    pub keep_format: bool,
}

/// Everything the CSV reader guesses unless told.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CsvOptions {
    /// The field separator; inferred when `None`. A `sep=` line in the file
    /// still wins over it.
    pub delimiter: Option<char>,
    /// Whether locale-formatted numbers become numbers rather than text.
    pub formatted_numbers: Option<FormattedNumbers>,
    /// Whether rows that produced no cell are skipped rather than left empty,
    /// so the output has no gaps.
    pub contiguous: bool,
    /// Whether an empty field becomes a cell holding the empty string rather
    /// than no cell at all.
    pub preserve_empty_fields: bool,
}

impl CsvOptions {
    /// Options that only pin the delimiter.
    #[must_use]
    pub fn with_delimiter(delimiter: char) -> Self {
        Self {
            delimiter: Some(delimiter),
            ..Self::default()
        }
    }
}

/// Reads a CSV file into a one-sheet workbook, guessing encoding and delimiter.
///
/// # Errors
/// [`Error::Csv`] if the file cannot be read.
pub fn read_csv(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    read_csv_with(path, &CsvOptions::default())
}

/// The same, with the guesses overridden.
///
/// # Errors
/// [`Error::Csv`] if the file cannot be read.
pub fn read_csv_with(
    path: impl AsRef<std::path::Path>,
    options: &CsvOptions,
) -> Result<Spreadsheet> {
    let bytes = std::fs::read(path).map_err(|e| Error::Csv(e.to_string()))?;
    Ok(read_csv_str(&decode(&bytes), options))
}

/// Reads CSV text that has already been decoded.
#[must_use]
pub fn read_csv_str(text: &str, options: &CsvOptions) -> Spreadsheet {
    // A `sep=;` first line is Excel's own way of stating the delimiter, and it
    // wins over both inference and the caller: it is data, not a guess.
    let (text, declared) = strip_separator_line(text);
    let delimiter = declared
        .or(options.delimiter)
        .unwrap_or_else(|| infer(text));

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Worksheet").unwrap_or_default();
    let mut out_row = 0u64;
    for fields in Rows::new(text, delimiter) {
        out_row += 1;
        let Ok(row) = Row::from_one_based(out_row) else {
            // Past the last row Excel has; the rest of the file is dropped, as
            // there is nowhere to put it.
            break;
        };
        let mut wrote = false;
        for (column, field) in fields.iter().enumerate() {
            if field.is_empty() && !options.preserve_empty_fields {
                continue;
            }
            let Ok(col) = Col::from_one_based(column as u64 + 1) else {
                break;
            };
            let at = CellRef::new(col, row);
            let (value, format) = read_field(field, options.formatted_numbers.as_ref());
            sheet.set(at, value);
            if let Some(code) = format {
                let style = crate::style::Style {
                    number_format: crate::style::NumberFormat::Custom(code),
                    ..crate::style::Style::default()
                };
                sheet.entry(at).style = book.styles.intern(style);
            }
            wrote = true;
        }
        // In contiguous mode a row that produced nothing does not take up a
        // row of the sheet, so the next one that does moves up into its place.
        if !wrote && options.contiguous {
            out_row -= 1;
        }
    }

    let _ = book.add_sheet(sheet);
    book
}

/// Decodes CSV bytes to text.
///
#[must_use]
pub fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, rest @ ..] => from_utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => from_utf16(rest, u16::from_be_bytes),
        _ => match core::str::from_utf8(bytes) {
            Ok(text) => text.to_owned(),
            Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
        },
    }
}

/// Decodes UTF-16 of one byte order. A trailing odd byte and an unpaired
/// surrogate both become the replacement character rather than an error: half
/// a readable file beats none.
fn from_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| unit([pair[0], pair[1]]));
    let mut text: String = char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    if !bytes.len().is_multiple_of(2) {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    text
}

/// One CP1252 byte as a character. Only `0x80..=0x9F` differs from Latin-1,
/// where CP1252 puts printable characters and Latin-1 puts control codes.
fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2C6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}',
        '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}', '\u{153}', '\u{9D}', '\u{17E}',
        '\u{178}',
    ];
    match b {
        0x80..=0x9F => HIGH[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}

/// Splits off a leading `sep=x` line, which Excel writes to name the delimiter.
///
fn strip_separator_line(text: &str) -> (&str, Option<char>) {
    let (line, rest) = match text.find('\n') {
        Some(end) => (&text[..end], &text[end + 1..]),
        None => (text, ""),
    };
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.len() >= 4 && line[..4].eq_ignore_ascii_case("sep=") {
        let mut chars = line[4..].chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return (rest, Some(c));
        }
    }
    (text, None)
}

/// Guesses the delimiter from how evenly each candidate is spread over lines.
///
fn infer(text: &str) -> char {
    let mut counts = [(); CANDIDATES.len()].map(|()| Vec::<usize>::new());
    let mut lines = 0usize;
    for line in unquoted_lines(text).take(INFERENCE_LINES) {
        lines += 1;
        for (slot, candidate) in counts.iter_mut().zip(CANDIDATES) {
            slot.push(line.chars().filter(|&c| c == candidate).count());
        }
    }
    if lines == 0 {
        return CANDIDATES[0];
    }

    let middle = (lines - 1) / 2;
    let mut best: Option<(f64, char)> = None;
    for (series, candidate) in counts.iter_mut().zip(CANDIDATES) {
        series.sort_unstable();
        let median = if lines.is_multiple_of(2) {
            let (a, b) = (series[middle], series[middle + 1]);
            f64::from(u32::try_from(a + b).unwrap_or(u32::MAX)) / 2.0
        } else {
            f64::from(u32::try_from(series[middle]).unwrap_or(u32::MAX))
        };
        if median == 0.0 {
            continue;
        }
        let deviation: f64 = series
            .iter()
            .map(|&n| {
                let d = f64::from(u32::try_from(n).unwrap_or(u32::MAX)) - median;
                d * d
            })
            .sum::<f64>()
            / f64::from(u32::try_from(series.len()).unwrap_or(u32::MAX));
        // Strictly smaller, so the earlier candidate keeps a tie.
        if best.is_none_or(|(min, _)| deviation < min) {
            best = Some((deviation, candidate));
        }
    }
    best.map_or(CANDIDATES[0], |(_, c)| c)
}

/// Lines with everything between quotes removed, for counting delimiters.
///
fn unquoted_lines(text: &str) -> impl Iterator<Item = String> {
    let mut chars = text.chars().peekable();
    core::iter::from_fn(move || {
        let mut line = String::new();
        let mut quoted = false;
        let mut any = false;
        for c in chars.by_ref() {
            any = true;
            if c == ENCLOSURE {
                quoted = !quoted;
            } else if c == '\n' && !quoted {
                return Some(line);
            } else if !quoted && c != '\r' {
                line.push(c);
            }
        }
        any.then_some(line)
    })
}

/// Splits CSV text into rows of fields.
///
/// RFC 4180 as Excel writes it: a quote inside a quoted field
/// is written twice. A line break inside quotes belongs to the field.
struct Rows<'a> {
    rest: &'a str,
    delimiter: char,
}

impl<'a> Rows<'a> {
    const fn new(text: &'a str, delimiter: char) -> Self {
        Self {
            rest: text,
            delimiter,
        }
    }
}

impl Iterator for Rows<'_> {
    type Item = Vec<String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let mut fields = vec![String::new()];
        let mut quoted = false;
        let mut chars = self.rest.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                ENCLOSURE if quoted => {
                    // Two in a row are one literal quote; a single one closes.
                    if self.rest[i + c.len_utf8()..].starts_with(ENCLOSURE) {
                        fields.last_mut()?.push(ENCLOSURE);
                        chars.next();
                    } else {
                        quoted = false;
                    }
                }
                // A quote only opens a field where the field is still empty;
                // elsewhere it is an ordinary character, as Excel reads it.
                ENCLOSURE if fields.last().is_some_and(String::is_empty) => quoted = true,
                _ if quoted => fields.last_mut()?.push(c),
                c if c == self.delimiter => fields.push(String::new()),
                '\r' => {}
                '\n' => {
                    self.rest = &self.rest[i + 1..];
                    return Some(fields);
                }
                _ => fields.last_mut()?.push(c),
            }
        }
        self.rest = "";
        Some(fields)
    }
}

/// A field as a value, plus the number format it asks for, if any.
///
fn read_field(field: &str, formatted: Option<&FormattedNumbers>) -> (CellValue, Option<String>) {
    let Some(rules) = formatted else {
        return (value_of(field), None);
    };
    let plain: String = field
        .chars()
        .filter(|&c| c != rules.thousands)
        .map(|c| if c == rules.decimal { '.' } else { c })
        .collect();
    let Some(n) = numeric(&plain) else {
        return (value_of(field), None);
    };
    if !rules.keep_format {
        return (CellValue::Number(n), None);
    }
    let mut code = if field.contains(rules.thousands) {
        "#,##0".to_owned()
    } else {
        "0".to_owned()
    };
    if let Some((_, fraction)) = field.split_once(rules.decimal) {
        let places = fraction.chars().count().min(6);
        if places > 0 {
            code.push('.');
            for _ in 0..places {
                code.push('0');
            }
        }
    }
    (CellValue::Number(n), Some(code))
}

/// What a field means.
///
pub(crate) fn value_of(field: &str) -> CellValue {
    if field.eq_ignore_ascii_case("true") {
        return CellValue::Bool(true);
    }
    if field.eq_ignore_ascii_case("false") {
        return CellValue::Bool(false);
    }
    if let Some(e) = CellError::parse(field) {
        return CellValue::Error(e);
    }
    // Without the formula engine there is nothing to check the text against,
    // so a leading `=` is taken at its word: keeping it as a formula loses
    // less than turning a real formula into text.
    #[cfg(feature = "formulas")]
    let parses = |rest: &str| crate::formula::parse(rest).is_ok();
    // Without the engine, the cheap test of what Excel would accept: a formula
    // starts with a value, a name, a sign or a bracket — never with another
    // operator, which is what makes `==` text.
    #[cfg(not(feature = "formulas"))]
    let parses = |rest: &str| {
        rest.starts_with(|c: char| {
            c.is_alphanumeric() || matches!(c, '(' | '+' | '-' | '"' | '\'' | '$' | '.')
        })
    };
    if let Some(rest) = field.strip_prefix('=')
        && !rest.is_empty()
        && parses(rest)
    {
        return CellValue::Formula {
            formula: rest.to_owned(),
            cached: None,
        };
    }
    if let Some(n) = numeric(field) {
        return CellValue::Number(n);
    }
    CellValue::text(field)
}

/// A field as a number, or `None` if Excel would keep it as text.
///
/// A leading zero (`0123`) and more than fifteen digits (`12345678901234567`)
/// both stay text: they are part numbers and card numbers, and turning them
/// into floats would lose exactly what they are for.
fn numeric(field: &str) -> Option<f64> {
    let digits = field.trim_start_matches(['+', '-']);
    if digits.len() > 1 && digits.starts_with('0') && !digits[1..].starts_with('.') {
        return None;
    }
    // Rust parses `inf`, `NaN` and `1_0`; Excel does not.
    if !digits
        .chars()
        .all(|c| c.is_ascii_digit() || ".eE+-".contains(c))
    {
        return None;
    }
    let n: f64 = field.parse().ok()?;
    if !n.is_finite() {
        return None;
    }
    if !field.contains(['.', 'e', 'E']) && n.abs() > 999_999_999_999_999.0 {
        return None;
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(text: &str, delimiter: char) -> Vec<Vec<String>> {
        Rows::new(text, delimiter).collect()
    }

    #[test]
    fn quotes_hold_delimiters_and_line_breaks() {
        assert_eq!(
            fields("a,\"b,c\",\"d\ne\"\r\nf,g\n", ','),
            vec![
                vec!["a".to_owned(), "b,c".to_owned(), "d\ne".to_owned()],
                vec!["f".to_owned(), "g".to_owned()],
            ]
        );
    }

    #[test]
    fn a_doubled_quote_is_one_quote() {
        assert_eq!(
            fields("\"say \"\"hi\"\"\",x", ','),
            vec![vec!["say \"hi\"".to_owned(), "x".to_owned()]]
        );
    }

    #[test]
    fn a_last_line_without_a_break_is_still_a_row() {
        assert_eq!(
            fields("a,b", ','),
            vec![vec!["a".to_owned(), "b".to_owned()]]
        );
        assert_eq!(fields("", ','), Vec::<Vec<String>>::new());
    }

    #[test]
    fn the_delimiter_is_the_one_spread_most_evenly() {
        assert_eq!(infer("a;b;c\nd;e;f\n"), ';');
        assert_eq!(infer("a\tb\tc\nd\te\tf\n"), '\t');
        // A comma inside quotes does not vote, so the semicolon wins.
        assert_eq!(infer("\"a,b\";c\n\"d,e,f\";g\n"), ';');
        // Nothing to go on: the default.
        assert_eq!(infer("abc\ndef\n"), ',');
    }

    #[test]
    fn a_sep_line_states_the_delimiter() {
        assert_eq!(strip_separator_line("sep=;\na;b\n"), ("a;b\n", Some(';')));
        assert_eq!(strip_separator_line("sep=;;\na\n").1, None);
        assert_eq!(strip_separator_line("a,b\n").1, None);
    }

    #[test]
    fn fields_take_the_type_excel_would_give_them() {
        assert_eq!(value_of("42"), CellValue::Number(42.0));
        assert_eq!(value_of("-3.5e2"), CellValue::Number(-350.0));
        assert_eq!(value_of("TRUE"), CellValue::Bool(true));
        assert_eq!(value_of("#DIV/0!"), CellValue::Error(CellError::Div0));
        // Leading zeros and long digit strings are identifiers, not numbers.
        assert_eq!(value_of("007"), CellValue::text("007"));
        assert_eq!(
            value_of("12345678901234567"),
            CellValue::text("12345678901234567")
        );
        assert_eq!(value_of("0.5"), CellValue::Number(0.5));
        assert_eq!(value_of("1 000"), CellValue::text("1 000"));
        assert_eq!(value_of("NaN"), CellValue::text("NaN"));
        assert_eq!(
            value_of("=SUM(A1:A2)"),
            CellValue::Formula {
                formula: "SUM(A1:A2)".to_owned(),
                cached: None
            }
        );
        // Not a formula, just text that starts with a sign.
        assert_eq!(value_of("=="), CellValue::text("=="));
    }

    #[test]
    fn encodings_are_told_apart_by_their_marks() {
        assert_eq!(decode(b"\xEF\xBB\xBFa,b"), "a,b");
        assert_eq!(decode(&[0xFF, 0xFE, b'a', 0, b'b', 0]), "ab");
        assert_eq!(decode(&[0xFE, 0xFF, 0, b'a', 0, b'b']), "ab");
        assert_eq!(decode("привет".as_bytes()), "привет");
        // Not UTF-8: CP1252, where 0x93/0x94 are curly quotes.
        assert_eq!(decode(&[0x93, b'x', 0x94]), "\u{201C}x\u{201D}");
    }

    #[test]
    fn locale_written_numbers_become_numbers_when_asked() {
        let rules = FormattedNumbers {
            decimal: ',',
            thousands: ' ',
            keep_format: false,
        };
        // Off by default: the same field is text.
        assert_eq!(read_field("1 234,56", None).0, CellValue::text("1 234,56"));
        assert_eq!(
            read_field("1 234,56", Some(&rules)),
            (CellValue::Number(1234.56), None)
        );
        // Still not a number when nothing but the separators would make it one.
        assert_eq!(read_field("x,y", Some(&rules)).0, CellValue::text("x,y"));

        // With the format kept, the cell shows the number the way it was
        // written: grouped when the field was grouped, as many places as shown.
        let kept = FormattedNumbers {
            keep_format: true,
            ..rules
        };
        assert_eq!(
            read_field("1 234,56", Some(&kept)).1.as_deref(),
            Some("#,##0.00")
        );
        assert_eq!(read_field("7", Some(&kept)).1.as_deref(), Some("0"));
        // The places are clamped at six.
        assert_eq!(
            read_field("0,12345678", Some(&kept)).1.as_deref(),
            Some("0.000000")
        );
    }

    #[test]
    fn contiguous_mode_leaves_no_empty_rows() {
        let text = "a\n\n\nb\n";
        let spread = read_csv_str(text, &CsvOptions::default());
        assert_eq!(
            spread.sheets()[0]
                .dimension()
                .map(|r| r.end.row.one_based()),
            Some(4)
        );

        let packed = read_csv_str(
            text,
            &CsvOptions {
                contiguous: true,
                ..CsvOptions::default()
            },
        );
        let sheet = &packed.sheets()[0];
        assert_eq!(sheet.len(), 2);
        assert_eq!(
            sheet.get(CellRef::parse("A2").unwrap()).map(|c| &c.value),
            Some(&CellValue::text("b"))
        );
    }

    #[test]
    fn an_empty_field_can_be_kept_as_an_empty_string() {
        let options = CsvOptions {
            preserve_empty_fields: true,
            delimiter: Some(','),
            ..CsvOptions::default()
        };
        let book = read_csv_str("a,,b\n", &options);
        assert_eq!(book.sheets()[0].len(), 3);
        assert_eq!(
            book.sheets()[0]
                .get(CellRef::parse("B1").unwrap())
                .map(|c| &c.value),
            Some(&CellValue::text(""))
        );
    }

    #[test]
    fn a_file_becomes_a_sheet() {
        let book = read_csv_str("a;1\nb;2\n", &CsvOptions::default());
        let sheet = book.sheet(0).expect("one sheet");
        assert_eq!(
            sheet.get(CellRef::parse("A1").unwrap()).map(|c| &c.value),
            Some(&CellValue::text("a"))
        );
        assert_eq!(
            sheet.get(CellRef::parse("B2").unwrap()).map(|c| &c.value),
            Some(&CellValue::Number(2.0))
        );
        // An empty field leaves no cell behind.
        assert_eq!(
            read_csv_str("a,,b\n", &CsvOptions::default())
                .sheet(0)
                .unwrap()
                .len(),
            2
        );
    }
}
