//! Reading `SpreadsheetML` 2003 (`.xml`).
//!
//! Excel 2003's XML format, and the one thing it shares with xlsx is the word
//! XML: one document, `<Workbook><Worksheet><Table><Row><Cell><Data>`, in the
//! `urn:schemas-microsoft-com:office:spreadsheet` namespace, where the
//! interesting attributes carry the `ss:` prefix.
//!
//! Two of its habits shape the reader. A cell states its column only when it
//! is not the next one (`ss:Index`), so the cursor has to be kept; and a
//! formula is written in R1C1, the same as SYLK, so it goes through the same
//! translation.
//!
//! Not ported: comments, print settings, the workbook's window state, and the
//! `ss:ArrayRange` of an array formula, which the model has no spill for.

use super::zipxml::attr;
use crate::error::{Error, Result};
use crate::model::{CellValue, ColumnRun, Spreadsheet, Worksheet};
use crate::shared::date::Epoch;
use crate::style::{
    Color, HorizontalAlign, NumberFormat, Pattern, Style, StyleTable, Underline, VerticalAlign,
};
use crate::{CellRef, Col, Range, Row};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;

/// Reads a `SpreadsheetML` workbook from a file.
///
/// # Errors
/// [`Error::Xml2003`] if the file cannot be read or is not a workbook.
pub fn read_xml2003(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    let bytes = std::fs::read(path).map_err(|e| Error::Xml2003(e.to_string()))?;
    read_xml2003_str(&super::csv::decode(&bytes))
}

/// The same, from text already in memory.
///
/// # Errors
/// As [`read_xml2003`].
pub fn read_xml2003_str(xml: &str) -> Result<Spreadsheet> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut state = Xml2003Reader::default();

    loop {
        match reader.read_event() {
            Err(e) => return Err(Error::Xml2003(e.to_string())),
            Ok(Event::Eof) => break,
            Ok(ev @ (Event::Start(_) | Event::Empty(_))) => {
                let empty = matches!(ev, Event::Empty(_));
                let (Event::Start(e) | Event::Empty(e)) = ev else {
                    continue;
                };
                state.start(e.local_name().as_ref(), &e, empty);
            }
            Ok(Event::Text(t)) if state.in_data => state.text.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if state.in_data => {
                super::zipxml::push_entity(&mut state.text, &r);
            }
            Ok(Event::End(e)) => state.end(e.local_name().as_ref()),
            Ok(_) => {}
        }
    }
    state.finish()
}

/// The workbook being built and where in it the reader stands.
struct Xml2003Reader {
    book: Spreadsheet,
    styles: StyleTable,
    sheet: Option<Worksheet>,
    seen_workbook: bool,
    /// Named styles, by the id a cell points at.
    named: HashMap<String, Style>,
    building: Option<(String, Style)>,
    /// One-based, and advanced by the file rather than stated on every cell.
    row: u64,
    col: u64,
    cell: Option<PendingCell>,
    text: String,
    in_data: bool,
}

impl Default for Xml2003Reader {
    fn default() -> Self {
        Self {
            book: Spreadsheet::empty(),
            styles: StyleTable::default(),
            sheet: None,
            seen_workbook: false,
            named: HashMap::new(),
            building: None,
            row: 1,
            col: 1,
            cell: None,
            text: String::new(),
            in_data: false,
        }
    }
}

impl Xml2003Reader {
    fn start(&mut self, name: &str, e: &quick_xml::events::BytesStart<'_>, empty: bool) {
        // `Workbook` is on its own: it writes nothing, it only marks the file as ours.
        if name == "Workbook" {
            self.seen_workbook = true;
        } else if !self.start_style(name, e) {
            self.start_grid(name, e, empty);
        }
    }

    /// A named style and the elements that make it up. Cells point at these by
    /// id rather than carrying formatting of their own.
    fn start_style(&mut self, name: &str, e: &quick_xml::events::BytesStart<'_>) -> bool {
        match name {
            "Style" => {
                let id = attr(e, "ID").unwrap_or_default();
                // `Default` is the style every other one starts from.
                // FIXME: a `Default` declared after other styles is invisible to
                // them. Excel always writes it first, so this holds in practice.
                let base = self.named.get("Default").cloned().unwrap_or_default();
                self.building = Some((id, base));
                return true;
            }
            "NumberFormat" | "Font" | "Interior" | "Alignment" => {}
            _ => return false,
        }
        let Some((_, style)) = self.building.as_mut() else {
            return true;
        };
        match name {
            "NumberFormat" => {
                if let Some(code) = attr(e, "Format") {
                    style.number_format = number_format(&code);
                }
            }
            "Font" => font(style, e),
            "Interior" => {
                if let Some(colour) = attr(e, "Color").and_then(|c| colour(&c)) {
                    style.fill.pattern = Pattern::Solid;
                    style.fill.foreground = colour;
                }
            }
            _ => {
                if let Some(value) = attr(e, "Horizontal") {
                    style.alignment.horizontal = horizontal(&value);
                }
                if let Some(value) = attr(e, "Vertical") {
                    style.alignment.vertical = vertical(&value);
                }
                style.alignment.wrap_text |= attr(e, "WrapText").as_deref() == Some("1");
            }
        }
        true
    }

    /// Sheets, columns, rows and cells. A self-closing element gets its closing
    /// work here, since no `End` follows it.
    fn start_grid(&mut self, name: &str, e: &quick_xml::events::BytesStart<'_>, empty: bool) {
        match name {
            "Worksheet" => {
                self.close_sheet();
                let title = attr(e, "Name").unwrap_or_else(|| "Sheet".to_owned());
                self.sheet = Some(Worksheet::new(title).unwrap_or_default());
                self.row = 1;
            }
            "Column" => {
                if let Some(sheet) = self.sheet.as_mut() {
                    column(sheet, e);
                }
            }
            "Row" => {
                if let Some(index) = attr(e, "Index").and_then(|v| v.parse().ok()) {
                    self.row = index;
                }
                if let (Some(sheet), Some(points)) = (
                    self.sheet.as_mut(),
                    attr(e, "Height").and_then(|v| v.parse::<f64>().ok()),
                ) && let Ok(at) = Row::from_one_based(self.row)
                {
                    let properties = sheet.rows.entry(at).or_default();
                    properties.height = Some(points);
                    properties.custom_height = true;
                }
                self.col = 1;
                if empty {
                    self.row += 1;
                }
            }
            "Cell" => {
                if let Some(index) = attr(e, "Index").and_then(|v| v.parse().ok()) {
                    self.col = index;
                }
                let pending = PendingCell {
                    style: attr(e, "StyleID"),
                    formula: attr(e, "Formula"),
                    across: attr(e, "MergeAcross").and_then(|v| v.parse().ok()),
                    down: attr(e, "MergeDown").and_then(|v| v.parse().ok()),
                    kind: None,
                };
                if empty {
                    self.place(&pending, "");
                } else {
                    self.cell = Some(pending);
                }
            }
            "Data" => {
                if let Some(pending) = self.cell.as_mut() {
                    pending.kind = attr(e, "Type");
                    self.text.clear();
                    self.in_data = true;
                }
                if empty {
                    self.in_data = false;
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        match name {
            "Style" => {
                if let Some((id, style)) = self.building.take() {
                    self.named.insert(id, style);
                }
            }
            "Worksheet" => self.close_sheet(),
            "Row" => self.row += 1,
            "Data" => self.in_data = false,
            "Cell" => {
                if let Some(pending) = self.cell.take() {
                    let text = std::mem::take(&mut self.text);
                    self.place(&pending, &text);
                }
                self.text.clear();
            }
            _ => {}
        }
    }

    /// Puts a cell on the sheet and steps past whatever it merges over.
    fn place(&mut self, pending: &PendingCell, text: &str) {
        pending.put(
            self.sheet.as_mut(),
            &mut self.styles,
            &self.named,
            self.row,
            self.col,
            text,
        );
        self.col += 1 + u64::from(pending.across.unwrap_or(0));
    }

    /// Adds the sheet being read to the book, if there is one.
    fn close_sheet(&mut self) {
        if let Some(done) = self.sheet.take() {
            let _ = self.book.add_sheet(done);
        }
    }

    fn finish(mut self) -> Result<Spreadsheet> {
        self.close_sheet();
        if !self.seen_workbook || self.book.sheets().is_empty() {
            return Err(Error::Xml2003(
                "no Workbook element with a worksheet in it".to_owned(),
            ));
        }
        self.book.styles = self.styles;
        Ok(self.book)
    }
}

/// A cell whose attributes are known and whose `<Data>` is still coming.
struct PendingCell {
    style: Option<String>,
    formula: Option<String>,
    across: Option<u32>,
    down: Option<u32>,
    kind: Option<String>,
}

impl PendingCell {
    /// Puts the finished cell on the sheet.
    fn put(
        &self,
        sheet: Option<&mut Worksheet>,
        styles: &mut StyleTable,
        named: &HashMap<String, Style>,
        row: u64,
        col: u64,
        text: &str,
    ) {
        let (Some(sheet), Ok(col), Ok(row)) =
            (sheet, Col::from_one_based(col), Row::from_one_based(row))
        else {
            return;
        };
        let at = CellRef::new(col, row);
        let value = self.value(text, at);
        let style = self
            .style
            .as_ref()
            .and_then(|id| named.get(id))
            .map(|style| styles.intern(style.clone()));

        let cell = sheet.entry(at);
        cell.value = value;
        if let Some(id) = style {
            cell.style = id;
        }
        if let Some(range) = self.merge(at) {
            sheet.merges.push(range);
        }
    }

    /// The range the cell's merge attributes cover.
    fn merge(&self, at: CellRef) -> Option<Range> {
        let (across, down) = (self.across.unwrap_or(0), self.down.unwrap_or(0));
        if across == 0 && down == 0 {
            return None;
        }
        let end = CellRef::new(
            Col::from_one_based(u64::from(at.col.one_based()) + u64::from(across)).ok()?,
            Row::from_one_based(u64::from(at.row.one_based()) + u64::from(down)).ok()?,
        );
        Some(Range::new(at, end))
    }

    /// What the cell holds.
    fn value(&self, text: &str, at: CellRef) -> CellValue {
        let literal = match self.kind.as_deref() {
            Some("Number") => text
                .trim()
                .parse()
                .map_or_else(|_| CellValue::text(text), CellValue::Number),
            Some("Boolean") => CellValue::Bool(matches!(text.trim(), "1" | "TRUE" | "True")),
            Some("Error") => crate::CellError::parse(text.trim())
                .map_or_else(|| CellValue::text(text), CellValue::Error),
            Some("DateTime") => crate::shared::date_parse::parse(&text.replace('T', " "))
                .and_then(|parsed| parsed.serial(Epoch::Windows1900))
                .map_or_else(|| CellValue::text(text), CellValue::Number),
            // `String` and anything unknown are text.
            _ => CellValue::text(text),
        };
        match &self.formula {
            Some(formula) => CellValue::Formula {
                formula: super::slk::r1c1_to_a1(
                    formula.trim_start_matches('='),
                    at.row.one_based(),
                    at.col.one_based(),
                ),
                cached: Some(Box::new(literal)),
            },
            None => literal,
        }
    }
}

/// A `<Column>` element: the width of one column, or of a run of them.
fn column(sheet: &mut Worksheet, e: &quick_xml::events::BytesStart<'_>) {
    // The reader keeps its own cursor for columns as well: `ss:Index` names one
    // only when it is not the next.
    let index: u64 = attr(e, "Index")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            sheet
                .columns
                .last()
                .map_or(1, |run| u64::from(run.last.one_based()) + 1)
        });
    let span: u64 = attr(e, "Span").and_then(|v| v.parse().ok()).unwrap_or(0);
    let Some(points) = attr(e, "Width").and_then(|v| v.parse::<f64>().ok()) else {
        return;
    };
    let (Ok(first), Ok(last)) = (
        Col::from_one_based(index),
        Col::from_one_based(index + span),
    ) else {
        return;
    };
    let mut run = ColumnRun::new(first, last);
    // The width is in points, as in Gnumeric.
    run.width = Some(points / 5.4);
    run.custom_width = true;
    run.hidden = attr(e, "Hidden").as_deref() == Some("1");
    sheet.columns.push(run);
}

/// A `<Font>` element inside a style.
fn font(style: &mut Style, e: &quick_xml::events::BytesStart<'_>) {
    if let Some(name) = attr(e, "FontName") {
        style.font.name = name;
    }
    if let Some(size) = attr(e, "Size").and_then(|v| v.parse::<f64>().ok()) {
        style.font.set_size_points(size);
    }
    if let Some(colour) = attr(e, "Color").and_then(|c| colour(&c)) {
        style.font.color = colour;
    }
    style.font.bold |= attr(e, "Bold").as_deref() == Some("1");
    style.font.italic |= attr(e, "Italic").as_deref() == Some("1");
    style.font.strike |= attr(e, "StrikeThrough").as_deref() == Some("1");
    if attr(e, "Underline").is_some() {
        style.font.underline = Underline::Single;
    }
}

/// A colour, written as `#RRGGBB`.
fn colour(value: &str) -> Option<Color> {
    let hex = value.trim().strip_prefix('#')?;
    (hex.len() == 6)
        .then(|| Color::from_argb_str(&format!("FF{hex}")))
        .flatten()
}

/// The format codes the format spells out in words.
fn number_format(code: &str) -> NumberFormat {
    match code {
        "General" => NumberFormat::General,
        "Short Date" => NumberFormat::Builtin(14),
        "Long Time" => NumberFormat::Builtin(21),
        "Percent" => NumberFormat::Builtin(9),
        "Fixed" => NumberFormat::Builtin(2),
        "Standard" => NumberFormat::Builtin(4),
        "Scientific" => NumberFormat::Builtin(11),
        other => NumberFormat::Custom(other.replace("\\-", "-").replace("\\ ", " ")),
    }
}

/// Horizontal placement, which the format spells the way Excel does.
fn horizontal(value: &str) -> HorizontalAlign {
    match value {
        "Left" => HorizontalAlign::Left,
        "Center" => HorizontalAlign::Center,
        "Right" => HorizontalAlign::Right,
        "Justify" => HorizontalAlign::Justify,
        "Fill" => HorizontalAlign::Fill,
        "CenterAcrossSelection" => HorizontalAlign::CenterContinuous,
        "Distributed" => HorizontalAlign::Distributed,
        _ => HorizontalAlign::General,
    }
}

/// Vertical placement, likewise.
fn vertical(value: &str) -> VerticalAlign {
    match value {
        "Top" => VerticalAlign::Top,
        "Center" => VerticalAlign::Center,
        "Justify" => VerticalAlign::Justify,
        "Distributed" => VerticalAlign::Distributed,
        _ => VerticalAlign::Bottom,
    }
}
