//! Writing CSV.
//!
//! One sheet, no styling, no formulas - a CSV holds the values a reader would
//! see on screen and nothing else. A formula is written as its result: its
//! cached one when the file it came from had it, otherwise what the engine
//! computes now.

use crate::error::{Error, Result};
use crate::formula::eval::{Engine, Origin};
use crate::formula::value::Value;
use crate::model::{CellValue, Spreadsheet};
use crate::{CellRef, Col, Row};
use std::io::Write;

/// The field quote, and what a quote inside a field is doubled into.
const ENCLOSURE: char = '"';

/// Writes one sheet of a workbook as a comma-separated file.
///
/// # Errors
/// [`Error::Csv`] if the workbook has no such sheet, or the file cannot be
/// written.
pub fn write_csv(
    book: &Spreadsheet,
    sheet: usize,
    path: impl AsRef<std::path::Path>,
) -> Result<()> {
    let file = std::fs::File::create(path).map_err(|e| Error::Csv(e.to_string()))?;
    write_csv_to(book, sheet, std::io::BufWriter::new(file), ',')
}

/// The same, to any sink and with the delimiter given.
///
/// Lines end with `\r\n`, which is what RFC 4180 asks for and what Excel
/// writes, rather than the platform's own ending.
///
/// # Errors
/// [`Error::Csv`] if the workbook has no such sheet, or the sink fails.
pub fn write_csv_to<W: Write>(
    book: &Spreadsheet,
    sheet: usize,
    mut sink: W,
    delimiter: char,
) -> Result<()> {
    let Some(ws) = book.sheet(sheet) else {
        return Err(Error::Csv(format!("workbook has no sheet {sheet}")));
    };
    let Some(used) = ws.dimension() else {
        return Ok(());
    };
    let mut engine = Engine::new(book);

    let mut line = String::new();
    for row in used.start.row.one_based()..=used.end.row.one_based() {
        let Ok(row) = Row::from_one_based(u64::from(row)) else {
            break;
        };
        line.clear();
        for col in used.start.col.one_based()..=used.end.col.one_based() {
            let Ok(col) = Col::from_one_based(u64::from(col)) else {
                break;
            };
            if col != used.start.col {
                line.push(delimiter);
            }
            let at = CellRef::new(col, row);
            let field = ws.get(at).map_or_else(String::new, |cell| {
                render(&mut engine, sheet, at, &cell.value)
            });
            quote_into(&mut line, &field, delimiter);
        }
        line.push_str("\r\n");
        sink.write_all(line.as_bytes())
            .map_err(|e| Error::Csv(e.to_string()))?;
    }
    sink.flush().map_err(|e| Error::Csv(e.to_string()))
}

/// A cell as the text a CSV should carry.
fn render(engine: &mut Engine<'_>, sheet: usize, at: CellRef, value: &CellValue) -> String {
    match value {
        CellValue::Empty => String::new(),
        CellValue::Number(n) => number(*n),
        CellValue::Text(t) => t.clone(),
        CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        CellValue::Error(e) => e.as_str().to_owned(),
        CellValue::RichText(runs) => runs.iter().map(|r| r.text.as_str()).collect(),
        CellValue::Formula { formula, cached } => match cached {
            Some(cached) => render(engine, sheet, at, cached),
            None => match engine.eval(Origin::new(sheet, at), formula) {
                Value::Blank => String::new(),
                Value::Number(n) => number(n),
                Value::Text(t) => t,
                Value::Bool(b) => if b { "TRUE" } else { "FALSE" }.to_owned(),
                // A cell cannot hold a function, so it shows the error Excel
                // shows in its place.
                Value::Lambda(_) => crate::error::CellError::Calc.as_str().to_owned(),
                Value::Error(e) => e.as_str().to_owned(),
                // A formula that spilled: a CSV cell holds one value, so it
                // gets the top-left one, the way Excel shows it in the cell.
                Value::Array(rows) => rows
                    .first()
                    .and_then(|line| line.first())
                    .map_or_else(String::new, |v| format!("{v:?}")),
            },
        },
    }
}

/// A number as Excel would put it in a CSV.
///
fn number(n: f64) -> String {
    if !n.is_finite() {
        return String::new();
    }
    let plain = format!("{n}");
    if !plain.contains('.') {
        return plain;
    }
    let whole = plain.trim_start_matches(['+', '-']).split('.').next();
    let whole_len = whole.map_or(1, str::len);
    let Some(fraction) = 15usize.checked_sub(whole_len) else {
        return plain;
    };
    let clamped = format!("{n:.fraction$}");
    if clamped.contains('.') {
        let trimmed = clamped.trim_end_matches('0').trim_end_matches('.');
        return trimmed.to_owned();
    }
    clamped
}

/// Appends a field, quoted only where it has to be.
///
fn quote_into(line: &mut String, field: &str, delimiter: char) {
    if !field.contains([delimiter, ENCLOSURE, '\n', '\r']) {
        line.push_str(field);
        return;
    }
    line.push(ENCLOSURE);
    for c in field.chars() {
        if c == ENCLOSURE {
            line.push(ENCLOSURE);
        }
        line.push(c);
    }
    line.push(ENCLOSURE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Worksheet;

    fn text_of(book: &Spreadsheet) -> String {
        let mut out = Vec::new();
        write_csv_to(book, 0, &mut out, ',').expect("writes");
        String::from_utf8(out).expect("utf-8")
    }

    fn at(a: &str) -> CellRef {
        CellRef::parse(a).expect("test reference is valid")
    }

    #[test]
    fn a_sheet_becomes_lines_of_fields() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), "a");
        ws.set(at("B1"), 1.0);
        ws.set(at("A2"), true);
        // A gap in the middle is an empty field, not a missing one.
        ws.set(at("C2"), "x");
        let mut book = Spreadsheet::empty();
        book.add_sheet(ws).expect("added");
        assert_eq!(text_of(&book), "a,1,\r\nTRUE,,x\r\n");
    }

    #[test]
    fn only_fields_that_need_quoting_get_it() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), "plain");
        ws.set(at("B1"), "has,comma");
        ws.set(at("C1"), "say \"hi\"");
        ws.set(at("D1"), "two\nlines");
        let mut book = Spreadsheet::empty();
        book.add_sheet(ws).expect("added");
        assert_eq!(
            text_of(&book),
            "plain,\"has,comma\",\"say \"\"hi\"\"\",\"two\nlines\"\r\n"
        );
    }

    #[test]
    fn a_formula_is_written_as_its_result() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), 2.0);
        ws.set(at("A2"), 3.0);
        ws.set(
            at("A3"),
            CellValue::Formula {
                formula: "SUM(A1:A2)".to_owned(),
                cached: None,
            },
        );
        // A cached result is believed rather than recomputed.
        ws.set(
            at("A4"),
            CellValue::Formula {
                formula: "SUM(A1:A2)".to_owned(),
                cached: Some(Box::new(CellValue::Number(99.0))),
            },
        );
        let mut book = Spreadsheet::empty();
        book.add_sheet(ws).expect("added");
        assert_eq!(text_of(&book), "2\r\n3\r\n5\r\n99\r\n");
    }

    #[test]
    fn a_number_keeps_fifteen_significant_digits() {
        // What 0.1 + 0.2 really is; Excel shows and writes the short form.
        assert_eq!(number(0.1 + 0.2), "0.3");
        assert_eq!(number(1234.5678), "1234.5678");
        assert_eq!(number(1.0), "1");
        assert_eq!(number(-2.5), "-2.5");
    }

    #[test]
    fn an_empty_sheet_is_an_empty_file() {
        let mut book = Spreadsheet::empty();
        book.add_sheet(Worksheet::new("S").expect("valid name"))
            .expect("added");
        assert_eq!(text_of(&book), "");
    }
}
