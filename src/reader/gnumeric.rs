//! Reading Gnumeric (`.gnumeric`).
//!
//! One gzipped XML document: `<Workbook><Sheets><Sheet>`, each sheet holding
//! `<Cells><Cell Row="0" Col="0" ValueType="60">text</Cell></Cells>`. Rows and
//! columns are zero-based, and the value's type is a number rather than a name
//! - `60` is a string, `40` a float.
//!
//! A formula is written in the cell's own text, and a formula shared by a run
//! of cells is written once with an `ExprID`; every other cell of the run
//! carries the id alone and offsets the master itself.
//!
//! Not ported: styles (they come as ranges of a style vocabulary of their own),
//! comments, print settings and autofilters.

use super::zipxml::attr;
use crate::error::{Error, Result};
use crate::model::{CellValue, ColumnRun, Spreadsheet, Worksheet};
use crate::style::NumberFormat;
use crate::{CellRef, Col, Range, Row};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;

/// Largest document the reader will inflate, the cap the zip readers share.
const MAX_SIZE: u64 = super::zipxml::MAX_PART_SIZE;

/// Reads a Gnumeric workbook from a file.
///
/// # Errors
/// [`Error::Gnumeric`] if the file cannot be read, cannot be inflated, or is
/// not a Gnumeric document.
pub fn read_gnumeric(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    let bytes = std::fs::read(path).map_err(|e| Error::Gnumeric(e.to_string()))?;
    read_gnumeric_from(&bytes)
}

/// The same, from bytes already in memory. The document may be gzipped, as
/// Gnumeric writes it, or plain XML, as a file saved uncompressed is.
///
/// # Errors
/// As [`read_gnumeric`].
pub fn read_gnumeric_from(bytes: &[u8]) -> Result<Spreadsheet> {
    let xml = if bytes.starts_with(&[0x1F, 0x8B]) {
        use std::io::Read as _;
        let mut out = String::new();
        flate2::read::GzDecoder::new(bytes)
            .take(MAX_SIZE)
            .read_to_string(&mut out)
            .map_err(|e| Error::Gnumeric(format!("gzip: {e}")))?;
        out
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    read_gnumeric_str(&xml)
}

/// Reads the XML of a Gnumeric workbook.
///
/// # Errors
/// [`Error::Gnumeric`] if the document is malformed or holds no workbook.
pub fn read_gnumeric_str(xml: &str) -> Result<Spreadsheet> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut state = GnumericReader::default();

    loop {
        match reader.read_event() {
            Err(e) => return Err(Error::Gnumeric(e.to_string())),
            Ok(Event::Eof) => break,
            Ok(ev @ (Event::Start(_) | Event::Empty(_))) => {
                let empty = matches!(ev, Event::Empty(_));
                let (Event::Start(e) | Event::Empty(e)) = ev else {
                    continue;
                };
                state.start(e.local_name().as_ref(), &e, empty);
            }
            Ok(Event::Text(t)) if state.collecting.is_some() => {
                state.text.push_str(&t.xml10_content());
            }
            Ok(Event::GeneralRef(r)) if state.collecting.is_some() => {
                super::zipxml::push_entity(&mut state.text, &r);
            }
            Ok(Event::End(e)) => state.end(e.local_name().as_ref()),
            Ok(_) => {}
        }
    }
    state.finish()
}

/// The workbook being built and where in it the reader stands.
struct GnumericReader {
    book: Spreadsheet,
    sheet: Option<Worksheet>,
    cell: Option<Pending>,
    text: String,
    /// Where text is being collected: the name of a sheet, or the body of a
    /// cell or of a merge.
    collecting: Option<&'static str>,
    /// The master of each shared formula, by its id.
    expressions: HashMap<String, (u32, u32, String)>,
    /// A number format is on the cell, while the style table it has to go into
    /// lives on the workbook; the pairs are collected and applied at the end.
    formats: Vec<(usize, CellRef, String)>,
    seen_workbook: bool,
}

impl Default for GnumericReader {
    fn default() -> Self {
        Self {
            book: Spreadsheet::empty(),
            sheet: None,
            cell: None,
            text: String::new(),
            collecting: None,
            expressions: HashMap::new(),
            formats: Vec::new(),
            seen_workbook: false,
        }
    }
}

impl GnumericReader {
    fn start(&mut self, name: &str, e: &quick_xml::events::BytesStart<'_>, empty: bool) {
        match name {
            "Workbook" => self.seen_workbook = true,
            "Sheet" => {
                self.close_sheet();
                self.sheet = Some(Worksheet::default());
            }
            "Name" => self.collect("Name"),
            "Merge" => self.collect("Merge"),
            "Cell" => {
                self.collect("Cell");
                self.cell = Some(Pending {
                    row: attr(e, "Row").and_then(|v| v.parse().ok()).unwrap_or(0),
                    col: attr(e, "Col").and_then(|v| v.parse().ok()).unwrap_or(0),
                    value_type: attr(e, "ValueType"),
                    expression: attr(e, "ExprID"),
                    format: attr(e, "ValueFormat"),
                });
                // `<Cell .../>` holds no text of its own.
                if empty {
                    self.place_cell();
                }
            }
            "ColInfo" | "RowInfo" => {
                if let Some(sheet) = self.sheet.as_mut() {
                    size(sheet, e, name == "ColInfo");
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        match name {
            "Sheet" => self.close_sheet(),
            "Name" if self.collecting == Some("Name") => {
                if let Some(sheet) = self.sheet.as_mut() {
                    let _ = sheet.set_title(self.text.trim());
                }
                self.collecting = None;
            }
            "Merge" if self.collecting == Some("Merge") => {
                if let (Some(sheet), Ok(range)) =
                    (self.sheet.as_mut(), Range::parse(self.text.trim()))
                {
                    sheet.merges.push(range);
                }
                self.collecting = None;
            }
            "Cell" if self.collecting == Some("Cell") => self.place_cell(),
            _ => {}
        }
    }

    /// Starts gathering the text of `what`.
    // `&'static str` rather than an enum: there are three of them and no more coming.
    fn collect(&mut self, what: &'static str) {
        self.text.clear();
        self.collecting = Some(what);
    }

    /// Puts the cell just read on the sheet, with whatever text it gathered.
    fn place_cell(&mut self) {
        if let (Some(sheet), Some(pending)) = (self.sheet.as_mut(), self.cell.take()) {
            let index = self.book.sheets().len();
            pending.put(
                sheet,
                &self.text,
                &mut self.expressions,
                index,
                &mut self.formats,
            );
        }
        self.collecting = None;
    }

    /// Adds the sheet being read to the book, if there is one.
    fn close_sheet(&mut self) {
        if let Some(done) = self.sheet.take() {
            let _ = self.book.add_sheet(done);
        }
    }

    /// Interns the number formats gathered along the way. They could not be
    /// interned as they arrived: the style table lives on the workbook, which
    /// was borrowed by the sheet being filled in.
    fn finish(mut self) -> Result<Spreadsheet> {
        self.close_sheet();
        if !self.seen_workbook {
            return Err(Error::Gnumeric("no Workbook element".to_owned()));
        }
        if self.book.sheets().is_empty() {
            return Err(Error::Gnumeric("the workbook has no sheets".to_owned()));
        }
        // Nothing borrows `book` any more, so styles can be interned now.
        for (index, at, code) in self.formats {
            let id = self.book.styles.intern(crate::style::Style {
                number_format: value_format(&code),
                ..crate::style::Style::default()
            });
            if let Some(sheet) = self.book.sheet_mut(index) {
                sheet.entry(at).style = id;
            }
        }
        Ok(self.book)
    }
}

/// A cell whose attributes are known and whose text is still being collected.
struct Pending {
    row: u32,
    col: u32,
    value_type: Option<String>,
    expression: Option<String>,
    format: Option<String>,
}

impl Pending {
    /// Puts the finished cell on the sheet.
    fn put(
        self,
        sheet: &mut Worksheet,
        text: &str,
        expressions: &mut HashMap<String, (u32, u32, String)>,
        index: usize,
        formats: &mut Vec<(usize, CellRef, String)>,
    ) {
        let (Ok(col), Ok(row)) = (
            Col::from_one_based(u64::from(self.col) + 1),
            Row::from_one_based(u64::from(self.row) + 1),
        ) else {
            return;
        };
        let at = CellRef::new(col, row);
        let value = self.value(text, expressions);
        sheet.entry(at).value = value;
        if let Some(code) = self.format {
            formats.push((index, at, code));
        }
    }

    /// What the cell's text means.
    fn value(
        &self,
        text: &str,
        expressions: &mut HashMap<String, (u32, u32, String)>,
    ) -> CellValue {
        // A shared formula: the master carries the text, the rest the id alone.
        if let Some(id) = &self.expression {
            if text.trim().is_empty() {
                let Some((col, row, formula)) = expressions.get(id).cloned() else {
                    return CellValue::Empty;
                };
                return CellValue::Formula {
                    formula: crate::coordinate::shift_references(
                        &formula,
                        i64::from(self.col) - i64::from(col),
                        i64::from(self.row) - i64::from(row),
                    ),
                    cached: None,
                };
            }
            let formula = text.trim().trim_start_matches('=').to_owned();
            expressions.insert(id.clone(), (self.col, self.row, formula.clone()));
            return CellValue::Formula {
                formula,
                cached: None,
            };
        }
        match self.value_type.as_deref() {
            // 10 empty, 20 boolean, 30 integer, 40 float, 50 error, 60 string.
            Some("10") => CellValue::Empty,
            Some("20") => CellValue::Bool(text.trim() == "TRUE"),
            Some("30" | "40") => text
                .trim()
                .parse()
                .map_or_else(|_| CellValue::text(text), CellValue::Number),
            Some("50") => crate::CellError::parse(text.trim())
                .map_or_else(|| CellValue::text(text), CellValue::Error),
            Some("60") => CellValue::text(text),
            // No type at all means a formula, which is written as its text.
            _ => CellValue::Formula {
                formula: text.trim().trim_start_matches('=').to_owned(),
                cached: None,
            },
        }
    }
}

/// A `ColInfo` or `RowInfo` element: the size of one line, or of a run of them.
fn size(sheet: &mut Worksheet, e: &quick_xml::events::BytesStart<'_>, is_column: bool) {
    let first: u64 = attr(e, "No").and_then(|v| v.parse().ok()).unwrap_or(0);
    let count: u64 = attr(e, "Count").and_then(|v| v.parse().ok()).unwrap_or(1);
    let points: f64 = match attr(e, "Unit").and_then(|v| v.parse().ok()) {
        Some(points) => points,
        None => return,
    };
    let hidden = attr(e, "Hidden").as_deref() == Some("1");
    let last = first.saturating_add(count.max(1) - 1);
    if is_column {
        let (Ok(first), Ok(last)) = (
            Col::from_one_based(first + 1),
            Col::from_one_based(last + 1),
        ) else {
            return;
        };
        let mut run = ColumnRun::new(first, last);
        // Gnumeric measures a column in points; xlsx counts characters of the
        // default font, and converts between them by this one number.
        run.width = Some(points / 5.4);
        run.custom_width = true;
        run.hidden = hidden;
        sheet.columns.push(run);
    } else {
        for i in first..=last {
            let Ok(row) = Row::from_one_based(i + 1) else {
                return;
            };
            let properties = sheet.rows.entry(row).or_default();
            properties.height = Some(points);
            properties.custom_height = true;
            properties.hidden = hidden;
        }
    }
}

/// The number format a cell asks for, if the file spelled one out.
fn value_format(code: &str) -> NumberFormat {
    if code == "General" {
        NumberFormat::General
    } else {
        NumberFormat::Custom(code.to_owned())
    }
}
