//! Reading `OpenDocument` spreadsheets.
//!
//! Same container as xlsx - a zip of XML - and a different vocabulary inside.
//! The differences that matter:
//!
//! * A run of identical cells or rows is written once with a repeat count
//!   (`table:number-columns-repeated`), so a sheet ends with a `<table:table-cell
//!   table:number-columns-repeated="1013"/>` covering the empty tail. Expanding
//!   that literally would make a million cells out of nothing, so a repeat of
//!   an empty cell is skipped rather than stored.
//! * A cell carries both its typed value (`office:value="3.5"`) and the text a
//!   reader showed (`<text:p>3.50</text:p>`). The typed one is the truth.
//! * There is no number-format id on the cell; the format lives in a named
//!   style, and for dates and times the file gives no format at all. A reader
//!   guesses one from the shape of the displayed text, and so does this.
//! * Formulas are in `OpenDocument` notation - see
//!   [`crate::shared::odf_formula`].
//! * An array formula says how far it reaches on the cell that holds it
//!   (`table:number-matrix-columns-spanned`), which is where xlsx puts a
//!   `ref` and BIFF an `ARRAY` record.
//! * A merge is stated on the top-left cell (`table:number-columns-spanned`),
//!   not listed separately as xlsx does.

use super::zipxml::{MAX_UNCOMPRESSED_SIZE, attr, read_part, uncompressed_size};
use crate::error::{Error, Result};
use crate::model::{CellValue, ColumnRun, Hyperlink, LinkTarget, Spreadsheet, Worksheet};
use crate::shared::date::{DateTime, Epoch, to_serial};
use crate::shared::odf_formula;
use crate::style::{
    Border, BorderStyle, Color, HorizontalAlign, NumberFormat, Pattern, Style, StyleId, Underline,
    VerticalAlign,
};
use crate::{CellRef, Col, Range, Row};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
use std::io::{BufReader, Read, Seek};

/// The mimetype an ODS package declares.
const MIMETYPE: &str = "application/vnd.oasis.opendocument.spreadsheet";

/// Reads an `OpenDocument` spreadsheet from a file.
///
/// # Errors
/// [`Error::Ods`] if the file is not a readable ODS package.
pub fn read_ods(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    let file = std::fs::File::open(path).map_err(|e| Error::Ods(e.to_string()))?;
    read_ods_from(BufReader::new(file))
}

/// Reads an `OpenDocument` spreadsheet from any seekable reader.
///
/// # Errors
/// Same as [`read_ods`].
pub fn read_ods_from<R: Read + Seek>(source: R) -> Result<Spreadsheet> {
    let mut zip = zip::ZipArchive::new(source).map_err(|e| Error::Ods(e.to_string()))?;

    let total = uncompressed_size(&mut zip);
    if total > MAX_UNCOMPRESSED_SIZE {
        return Err(Error::Ods(format!(
            "package expands to {total} bytes, over the {MAX_UNCOMPRESSED_SIZE} limit"
        )));
    }

    // The mimetype part is how the format identifies itself; a zip without it
    // may be an xlsx, and reading its content.xml would find nothing.
    match read_part(&mut zip, "mimetype") {
        Ok(kind) if kind.trim() == MIMETYPE => {}
        Ok(kind) => return Err(Error::Ods(format!("not a spreadsheet: mimetype {kind:?}"))),
        Err(e) => return Err(Error::Ods(e)),
    }

    let content = read_part(&mut zip, "content.xml").map_err(Error::Ods)?;
    read_content(&content)
}

/// Reads `content.xml`, which holds the sheets and everything on them.
fn read_content(xml: &str) -> Result<Spreadsheet> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut state = ContentReader::default();

    loop {
        match reader.read_event() {
            Err(e) => return Err(Error::Ods(format!("content.xml: {e}"))),
            Ok(Event::Eof) => break,
            Ok(ev @ (Event::Start(_) | Event::Empty(_))) => {
                // `<table:table-cell/>` is a whole cell in one event: quick-xml
                // reports a self-closing element as `Empty` and never sends the
                // matching `End`, so the closing work has to happen here too.
                let closed = matches!(ev, Event::Empty(_));
                let (Event::Start(e) | Event::Empty(e)) = ev else {
                    continue;
                };
                let name = e.local_name().as_ref().to_owned();
                state.start(&name, &e);
                if closed {
                    state.end(&name);
                }
            }
            Ok(Event::GeneralRef(r)) if state.in_text > 0 => {
                super::zipxml::push_entity(&mut state.cell.text, &r);
            }
            Ok(Event::Text(t)) => {
                if state.in_text > 0 {
                    state.cell.text.push_str(&t.xml10_content());
                }
            }
            Ok(Event::End(e)) => state.end(e.local_name().as_ref()),
            Ok(_) => {}
        }
    }
    state.finish()
}

/// Most copies one sheet may be expanded to by repeat counts.
///
/// A repeat count is a number in the file, and `999999999` rows of
/// `999999999` cells is a thousand bytes to write and longer than this machine
/// will run to expand. The grid's own bounds are no defence: a full sheet is
/// still seventeen billion cells. Only copies count against this, so a sheet
/// that really does hold nine million cells is read whole; what it stops is a
/// file that asks for millions of them out of one element.
const MAX_SHEET_COPIES: u64 = 4_000_000;

/// The workbook being built and where in it the reader stands.
struct ContentReader {
    book: Spreadsheet,
    sheet: Option<Worksheet>,
    /// Where the next row and the next cell go. Both are 1-based, as the format
    /// counts them, and both advance by the repeat counts.
    row: u64,
    col: u64,
    cell: CellState,
    /// Copies made by repeat counts on the sheet being read, against
    /// [`MAX_SHEET_COPIES`].
    copies: u64,
    /// Depth of `text:p` and friends, so text is collected only inside a cell.
    in_text: usize,
    names: Vec<(String, String)>,
    /// Automatic styles come before the body, so one pass suffices: by the time
    /// a cell names its style, the style has been read.
    styles: HashMap<String, StyleId>,
    building: Option<(String, Style)>,
    /// Empty cells that carry only a style. The tail of every row is a run of
    /// them reaching the last column of the sheet, and materialising that would
    /// be thousands of cells saying nothing; they are held back until a cell
    /// with a value proves they were in the middle of the row.
    pending: Vec<(u64, u64, StyleId)>,
}

impl Default for ContentReader {
    fn default() -> Self {
        Self {
            book: Spreadsheet::empty(),
            sheet: None,
            row: 1,
            col: 1,
            cell: CellState::default(),
            copies: 0,
            in_text: 0,
            names: Vec::new(),
            styles: HashMap::new(),
            building: None,
            pending: Vec::new(),
        }
    }
}

impl ContentReader {
    fn start(&mut self, name: &str, e: &quick_xml::events::BytesStart<'_>) {
        let _ = self.start_grid(name, e) || self.start_style(name, e) || self.start_text(name, e);
    }

    /// Sheets, columns, rows and cells: the grid itself.
    fn start_grid(&mut self, name: &str, e: &quick_xml::events::BytesStart<'_>) -> bool {
        match name {
            "table" => {
                // A finished sheet is added before the next one starts.
                self.close_sheet();
                let title = attr(e, "name").unwrap_or_else(|| "Sheet".to_owned());
                self.sheet = Some(
                    Worksheet::new(unique_title(&self.book, &title))
                        .unwrap_or_else(|_| Worksheet::default()),
                );
                self.row = 1;
            }
            "table-column" => {
                if let Some(ws) = self.sheet.as_mut() {
                    let repeat = repeat_of(e, "number-columns-repeated");
                    column_run(ws, e, repeat);
                }
            }
            "table-row" => {
                self.col = 1;
                let repeat = repeat_of(e, "number-rows-repeated");
                if let Some(ws) = self.sheet.as_mut() {
                    row_properties(ws, e, self.row, repeat);
                }
                self.cell.row_repeat = repeat;
            }
            "table-cell" | "covered-table-cell" => {
                self.cell = CellState {
                    row_repeat: self.cell.row_repeat,
                    repeat: repeat_of(e, "number-columns-repeated"),
                    value_type: attr(e, "value-type"),
                    value: attr(e, "value"),
                    date: attr(e, "date-value"),
                    time: attr(e, "time-value"),
                    boolean: attr(e, "boolean-value"),
                    string: attr(e, "string-value"),
                    formula: attr(e, "formula"),
                    columns_spanned: repeat_of(e, "number-columns-spanned"),
                    rows_spanned: repeat_of(e, "number-rows-spanned"),
                    matrix: attr(e, "number-matrix-columns-spanned").map(|across| {
                        (
                            across.parse().unwrap_or(1),
                            repeat_of(e, "number-matrix-rows-spanned"),
                        )
                    }),
                    style: attr(e, "style-name").and_then(|n| self.styles.get(&n).copied()),
                    ..CellState::default()
                };
            }
            _ => return false,
        }
        true
    }

    /// An automatic cell style and the property elements inside it.
    fn start_style(&mut self, name: &str, e: &quick_xml::events::BytesStart<'_>) -> bool {
        match name {
            "style" => {
                if attr(e, "family").as_deref() == Some("table-cell")
                    && let Some(named) = attr(e, "name")
                {
                    self.building = Some((named, Style::default()));
                }
            }
            "table-cell-properties" => {
                if let Some((_, style)) = self.building.as_mut() {
                    cell_properties(style, e);
                }
            }
            "paragraph-properties" => {
                if let Some((_, style)) = self.building.as_mut() {
                    style.alignment.horizontal = horizontal_of(e);
                }
            }
            "text-properties" => {
                if let Some((_, style)) = self.building.as_mut() {
                    text_properties(style, e);
                }
            }
            _ => return false,
        }
        true
    }

    /// Text inside a cell, the whitespace elements it uses instead of literal
    /// characters, and the named expressions that sit beside the sheets.
    fn start_text(&mut self, name: &str, e: &quick_xml::events::BytesStart<'_>) -> bool {
        match name {
            "p" | "span" => self.in_text += 1,
            "a" => {
                if let Some(href) = attr(e, "href") {
                    self.cell.link = Some(href);
                }
                self.in_text += 1;
            }
            // <text:s/> is a space, <text:tab/> a tab, <text:line-break/> a newline.
            "s" if self.in_text > 0 => self.cell.text.push(' '),
            "tab" if self.in_text > 0 => self.cell.text.push('\t'),
            "line-break" if self.in_text > 0 => self.cell.text.push('\n'),
            "named-range" | "named-expression" => {
                let target = attr(e, "cell-range-address")
                    .or_else(|| attr(e, "expression"))
                    .unwrap_or_default();
                if let Some(named) = attr(e, "name") {
                    self.names.push((named, target));
                }
            }
            _ => return false,
        }
        true
    }

    fn end(&mut self, name: &str) {
        match name {
            "table" => self.close_sheet(),
            "table-row" => {
                // Whatever was held back turned out to be the tail of the row.
                self.pending.clear();
                self.row += self.cell.row_repeat.max(1);
                self.cell.row_repeat = 1;
            }
            "table-cell" | "covered-table-cell" => {
                if let Some(ws) = self.sheet.as_mut() {
                    place(
                        ws,
                        &mut self.book.styles,
                        &self.cell,
                        self.row,
                        self.col,
                        &mut self.pending,
                        &mut self.copies,
                    );
                }
                self.col += self.cell.repeat.max(1);
                self.cell = CellState {
                    row_repeat: self.cell.row_repeat,
                    ..CellState::default()
                };
            }
            "style" => {
                if let Some((named, style)) = self.building.take() {
                    let id = self.book.styles.intern(style);
                    self.styles.insert(named, id);
                }
            }
            "p" => {
                // Several paragraphs in one cell are separate lines.
                self.in_text = self.in_text.saturating_sub(1);
                if self.in_text == 0 {
                    self.cell.text.push('\n');
                }
            }
            "span" | "a" => self.in_text = self.in_text.saturating_sub(1),
            _ => {}
        }
    }

    /// Adds the sheet being read to the book, if there is one.
    // `add_sheet` can refuse a duplicate name, but `unique_title` above already
    // settled that, so the result is ignored here.
    fn close_sheet(&mut self) {
        if let Some(done) = self.sheet.take() {
            let _ = self.book.add_sheet(done);
        }
        self.copies = 0;
    }

    fn finish(mut self) -> Result<Spreadsheet> {
        self.close_sheet();
        if self.book.sheets().is_empty() {
            return Err(Error::Ods("file declares no sheets".into()));
        }
        for (name, target) in self.names {
            self.book.defined_names.push(crate::model::DefinedName {
                name,
                sheet: None,
                formula: odf_formula::to_a1(&target),
                hidden: false,
            });
        }
        Ok(self.book)
    }
}

/// Everything read off one `table:table-cell` before it is placed.
#[derive(Default)]
struct CellState {
    repeat: u64,
    row_repeat: u64,
    value_type: Option<String>,
    value: Option<String>,
    date: Option<String>,
    time: Option<String>,
    boolean: Option<String>,
    string: Option<String>,
    formula: Option<String>,
    columns_spanned: u64,
    rows_spanned: u64,
    /// How far a formula entered as a matrix - an array formula - reaches;
    /// `None` for an ordinary formula.
    matrix: Option<(u64, u64)>,
    style: Option<StyleId>,
    text: String,
    link: Option<String>,
}

/// Writes one cell, and the run of copies its repeat count asks for.
fn place(
    sheet: &mut Worksheet,
    styles: &mut crate::style::StyleTable,
    cell: &CellState,
    row: u64,
    col: u64,
    pending: &mut Vec<(u64, u64, StyleId)>,
    copies: &mut u64,
) {
    // A covered cell is the inside of a merge. It usually exists only to keep
    // the columns lined up, but it is allowed to carry a value of its own -
    // the merge hides it rather than deletes it, and dropping it here would
    // lose data the file holds.
    let (value, format) = value_of(cell);
    let has_value = !matches!(value, CellValue::Empty);
    if !has_value && cell.link.is_none() {
        // Nothing but a style: held back until something after it in the row
        // shows it was not the tail.
        if let Some(style) = cell.style.filter(|s| *s != StyleId::default()) {
            pending.push((col, cell.repeat.max(1), style));
        }
        return;
    }
    // Everything held back is now known to be inside the row, not after it.
    let row_repeat = cell.row_repeat.max(1);
    for (at_col, repeat, style) in pending.drain(..) {
        for r in 0..row_repeat {
            let Ok(row) = Row::from_one_based(row + r) else {
                break;
            };
            if *copies >= MAX_SHEET_COPIES {
                break;
            }
            for c in 0..repeat {
                let Ok(column) = Col::from_one_based(at_col + c) else {
                    break;
                };
                if *copies >= MAX_SHEET_COPIES {
                    break;
                }
                *copies += u64::from(c > 0 || r > 0);
                sheet.entry(CellRef::new(column, row)).style = style;
            }
        }
    }

    // A repeat count on a cell that holds something is a real run of copies.
    let repeat = cell.repeat.max(1);
    // A format the file only implied - a date, a percentage - becomes a style
    // of its own; a style the cell named already carries whatever it carried.
    let style = match (format, cell.style) {
        (Some(code), _) => Some(styles.intern(Style {
            number_format: NumberFormat::Custom(code),
            ..Style::default()
        })),
        (None, named) => named,
    };

    for r in 0..row_repeat {
        let Ok(row) = Row::from_one_based(row + r) else {
            return;
        };
        // A repeat count comes from the file, and the grid's bounds alone
        // leave seventeen billion cells to fill.
        if *copies >= MAX_SHEET_COPIES {
            return;
        }
        for c in 0..repeat {
            let Ok(col) = Col::from_one_based(col + c) else {
                break;
            };
            if *copies >= MAX_SHEET_COPIES {
                break;
            }
            *copies += u64::from(c > 0 || r > 0);
            let at = CellRef::new(col, row);
            if has_value {
                sheet.set(at, value.clone());
            }
            if let Some(style) = style {
                sheet.entry(at).style = style;
            }
            if let Some(href) = &cell.link {
                sheet.hyperlinks.push(link_of(at, href));
            }
            // A merge is stated once, on the cell that spans.
            if c == 0 && r == 0 && (cell.columns_spanned > 1 || cell.rows_spanned > 1) {
                let end = CellRef::new(
                    Col::from_one_based(
                        u64::from(col.one_based()) + cell.columns_spanned.max(1) - 1,
                    )
                    .unwrap_or(col),
                    Row::from_one_based(u64::from(row.one_based()) + cell.rows_spanned.max(1) - 1)
                        .unwrap_or(row),
                );
                sheet.merges.push(Range::new(at, end));
            }
            // So is the reach of an array formula.
            if let Some((across, down)) = cell.matrix.filter(|_| c == 0 && r == 0) {
                let end = CellRef::new(
                    Col::from_one_based(u64::from(col.one_based()) + across.max(1) - 1)
                        .unwrap_or(col),
                    Row::from_one_based(u64::from(row.one_based()) + down.max(1) - 1)
                        .unwrap_or(row),
                );
                sheet.array_formulas.push(Range::new(at, end));
            }
        }
    }
}

/// A cell's value, and the number format its shape asks for.
///
fn value_of(cell: &CellState) -> (CellValue, Option<String>) {
    let shown = cell.text.trim_end_matches('\n');
    let kind = cell.value_type.as_deref().unwrap_or("");
    let (value, format) = match kind {
        // A currency is a number whose format lives in a style this reader
        // does not resolve, so it arrives plain, as a float does.
        "float" | "currency" => (number(cell, shown), None),
        "percentage" => (
            number(cell, shown),
            Some(percentage_format(shown).to_owned()),
        ),
        "boolean" => (
            CellValue::Bool(cell.boolean.as_deref() == Some("true")),
            None,
        ),
        "date" => match cell.date.as_deref().and_then(iso_serial) {
            Some((serial, has_time)) => (
                CellValue::Number(serial),
                Some(if has_time {
                    "yyyy-mm-dd hh:mm:ss".to_owned()
                } else {
                    "yyyy-mm-dd".to_owned()
                }),
            ),
            None => (CellValue::text(shown), None),
        },
        "time" => match cell.time.as_deref().and_then(duration_serial) {
            Some(serial) => (
                CellValue::Number(serial),
                Some(if (0.0..1.0).contains(&serial) {
                    "hh:mm:ss".to_owned()
                } else {
                    // Past a day, or before it: `[hh]` is the only form that
                    // shows more than 24 hours instead of wrapping round.
                    "[hh]:mm:ss".to_owned()
                }),
            ),
            None => (CellValue::text(shown), None),
        },
        "string" => (
            CellValue::text(cell.string.as_deref().unwrap_or(shown)),
            None,
        ),
        // No type and no text is an empty cell; text without a type is text,
        // which is what a formula's string result looks like.
        _ if shown.is_empty() => (CellValue::Empty, None),
        _ => (CellValue::text(shown), None),
    };

    match &cell.formula {
        Some(formula) => (
            CellValue::Formula {
                formula: odf_formula::to_a1(formula),
                cached: (!matches!(value, CellValue::Empty)).then(|| Box::new(value)),
            },
            format,
        ),
        None => (value, format),
    }
}

/// The typed number of a cell, falling back to the text if the attribute is
/// missing - some writers leave it off for a formula whose result is a number.
fn number(cell: &CellState, shown: &str) -> CellValue {
    cell.value
        .as_deref()
        .and_then(|v| v.parse::<f64>().ok())
        .or_else(|| shown.parse::<f64>().ok())
        .map_or(CellValue::Empty, CellValue::Number)
}

/// The percentage format matching how many places the text showed.
fn percentage_format(shown: &str) -> &'static str {
    match shown.split_once('.') {
        None => "0%",
        Some((_, fraction)) => {
            // The `%` sits at the end of the text, so it counts as a place.
            let places = fraction.trim_end_matches('%').chars().count();
            if places <= 1 { "0.0%" } else { "0.00%" }
        }
    }
}

/// An ISO date or date-time as a serial number, and whether it carried a time.
fn iso_serial(iso: &str) -> Option<(f64, bool)> {
    let (date, time) = match iso.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (iso, None),
    };
    let mut parts = date.split('-');
    // A leading `-` would be a negative year, which no spreadsheet has.
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    let mut dt = DateTime::date(year, month, day);
    if let Some(time) = time {
        let mut clock = time.trim_end_matches('Z').split(':');
        dt.hour = clock.next()?.parse().ok()?;
        dt.minute = clock.next().and_then(|m| m.parse().ok()).unwrap_or(0);
        dt.second = clock.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    }
    let serial = to_serial(dt, Epoch::Windows1900).ok()?;
    Some((serial, time.is_some()))
}

/// An ISO 8601 duration such as `PT1H30M0S` as a fraction of a day.
///
fn duration_serial(duration: &str) -> Option<f64> {
    let (sign, body) = match duration.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, duration),
    };
    let body = body.strip_prefix("PT")?;
    let (mut hours, mut minutes, mut seconds) = (0.0f64, 0.0f64, 0.0f64);
    let mut digits = String::new();
    for c in body.chars() {
        match c {
            'H' => {
                hours = digits.parse().ok()?;
                digits.clear();
            }
            'M' => {
                minutes = digits.parse().ok()?;
                digits.clear();
            }
            'S' => {
                seconds = digits.parse().ok()?;
                digits.clear();
            }
            _ => digits.push(c),
        }
    }
    Some(sign * (hours / 24.0 + minutes / 1440.0 + seconds / 86400.0))
}

/// A hyperlink over one cell.
///
/// ODS writes an inside-the-document link as `#Sheet1.A1`, which A1 spells
/// `Sheet1!A1`.
fn link_of(at: CellRef, href: &str) -> Hyperlink {
    let target = match href.strip_prefix('#') {
        Some(place) => LinkTarget::Inside(odf_formula::to_a1(place)),
        None => LinkTarget::Outside(href.to_owned()),
    };
    Hyperlink {
        range: Range::new(at, at),
        target,
        display: None,
        tooltip: None,
    }
}

/// Records a run of columns, with its width if the file states one.
fn column_run(sheet: &mut Worksheet, e: &quick_xml::events::BytesStart<'_>, repeat: u64) {
    let first = Col::from_one_based(next_column(sheet)).unwrap_or_default();
    let last = Col::from_one_based(next_column(sheet) + repeat.max(1) - 1).unwrap_or(first);
    let mut run = ColumnRun::new(first, last);
    run.hidden = attr(e, "visibility").is_some_and(|v| v != "visible");
    if run.is_meaningful() {
        sheet.columns.push(run);
    } else {
        // An unremarkable run still takes up its columns, so a later one lands
        // in the right place; a placeholder keeps the count without saying
        // anything the writer would have to reproduce.
        sheet.columns.push(ColumnRun::new(first, last));
    }
}

/// Where the next `table:table-column` starts, 1-based.
fn next_column(sheet: &Worksheet) -> u64 {
    sheet
        .columns
        .last()
        .map_or(1, |run| u64::from(run.last.one_based()) + 1)
}

/// Records a row's visibility, if it has anything worth recording.
fn row_properties(
    sheet: &mut Worksheet,
    e: &quick_xml::events::BytesStart<'_>,
    row: u64,
    repeat: u64,
) {
    let hidden = attr(e, "visibility").is_some_and(|v| v != "visible");
    if !hidden {
        return;
    }
    for r in 0..repeat.max(1) {
        let Ok(row) = Row::from_one_based(row + r) else {
            return;
        };
        sheet.rows.entry(row).or_default().hidden = true;
    }
}

/// A repeat or span attribute, as a count.
fn repeat_of(e: &quick_xml::events::BytesStart<'_>, name: &str) -> u64 {
    attr(e, name).and_then(|v| v.parse().ok()).unwrap_or(1)
}

/// A sheet name not already taken, since a workbook may not hold two.
fn unique_title(book: &Spreadsheet, wanted: &str) -> String {
    if book.sheet_by_name(wanted).is_none() {
        return wanted.to_owned();
    }
    for n in 2..1000 {
        let candidate = format!("{wanted} ({n})");
        if book.sheet_by_name(&candidate).is_none() {
            return candidate;
        }
    }
    wanted.to_owned()
}

/// Reads `style:table-cell-properties`: the fill, the borders, the placement.
fn cell_properties(style: &mut Style, e: &quick_xml::events::BytesStart<'_>) {
    if let Some(colour) = attr(e, "background-color").as_deref().and_then(colour_of) {
        style.fill.pattern = Pattern::Solid;
        style.fill.foreground = colour;
    }
    // `fo:border` sets all four sides at once; a side of its own overrides it.
    if let Some(all) = attr(e, "border") {
        let border = border_of(&all);
        style.borders.top = border.clone();
        style.borders.right = border.clone();
        style.borders.bottom = border.clone();
        style.borders.left = border;
    }
    for (name, side) in [
        ("border-top", &mut style.borders.top),
        ("border-right", &mut style.borders.right),
        ("border-bottom", &mut style.borders.bottom),
        ("border-left", &mut style.borders.left),
    ] {
        if let Some(value) = attr(e, name) {
            *side = border_of(&value);
        }
    }
    if let Some(vertical) = attr(e, "vertical-align") {
        style.alignment.vertical = match vertical.as_str() {
            "top" => VerticalAlign::Top,
            "middle" => VerticalAlign::Center,
            _ => VerticalAlign::Bottom,
        };
    }
    if attr(e, "wrap-option").as_deref() == Some("wrap") {
        style.alignment.wrap_text = true;
    }
}

/// Reads `style:text-properties`: the font.
fn text_properties(style: &mut Style, e: &quick_xml::events::BytesStart<'_>) {
    if let Some(name) = attr(e, "font-name").or_else(|| attr(e, "font-family")) {
        style.font.name = name;
    }
    if let Some(size) = attr(e, "font-size")
        .and_then(|s| s.trim_end_matches("pt").parse::<f64>().ok())
        .filter(|s| *s > 0.0)
    {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a font size is a small positive number of points"
        )]
        let hundredths = (size * 100.0).round() as u32;
        style.font.size = hundredths;
    }
    if attr(e, "font-weight").as_deref() == Some("bold") {
        style.font.bold = true;
    }
    if attr(e, "font-style").as_deref() == Some("italic") {
        style.font.italic = true;
    }
    if attr(e, "text-underline-style").is_some_and(|v| v != "none") {
        style.font.underline = Underline::Single;
    }
    if attr(e, "text-line-through-style").is_some_and(|v| v != "none") {
        style.font.strike = true;
    }
    if let Some(colour) = attr(e, "color").as_deref().and_then(colour_of) {
        style.font.color = colour;
    }
}

/// Reads `fo:text-align` off `style:paragraph-properties`.
fn horizontal_of(e: &quick_xml::events::BytesStart<'_>) -> HorizontalAlign {
    match attr(e, "text-align").as_deref() {
        // `start` and `end` follow the reading direction; for the left-to-right
        // scripts a spreadsheet lays out they are left and right.
        Some("start" | "left") => HorizontalAlign::Left,
        Some("center") => HorizontalAlign::Center,
        Some("end" | "right") => HorizontalAlign::Right,
        Some("justify") => HorizontalAlign::Justify,
        _ => HorizontalAlign::General,
    }
}

/// A `#rrggbb` colour as the model holds it, opaque.
fn colour_of(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    (hex.len() == 6)
        .then(|| u32::from_str_radix(hex, 16).ok())
        .flatten()
        .map(|rgb| Color::Argb(0xFF00_0000 | rgb))
}

/// An `fo:border` value - `0.05in solid #000000` - as a border.
fn border_of(value: &str) -> Border {
    let mut parts = value.split_whitespace();
    let width = parts.next().unwrap_or("");
    let kind = parts.next().unwrap_or("solid");
    let colour = parts.next().and_then(colour_of).unwrap_or(Color::Auto);
    if kind == "none" || width == "none" {
        return Border::default();
    }
    // ODF states a width in a length rather than by name, so the name that
    // comes back is the nearest of Excel's three.
    let inches: f64 = width.trim_end_matches("in").parse().unwrap_or(0.0);
    let style = match kind {
        "double" => BorderStyle::Named("double"),
        "dashed" if inches >= 0.04 => BorderStyle::Named("mediumDashed"),
        "dashed" => BorderStyle::Named("dashed"),
        "dotted" => BorderStyle::Named("dotted"),
        _ if inches >= 0.09 => BorderStyle::Named("thick"),
        _ if inches >= 0.04 => BorderStyle::Named("medium"),
        _ => BorderStyle::Named("thin"),
    };
    Border {
        style,
        color: colour,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal `content.xml` around the given rows.
    fn content(rows: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?>\
<office:document-content \
xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" \
xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" \
xmlns:xlink=\"http://www.w3.org/1999/xlink\">\
<office:body><office:spreadsheet>\
<table:table table:name=\"Лист\">{rows}</table:table>\
</office:spreadsheet></office:body></office:document-content>"
        )
    }

    fn at(a: &str) -> CellRef {
        CellRef::parse(a).expect("test reference is valid")
    }

    fn value(book: &Spreadsheet, a: &str) -> Option<CellValue> {
        book.sheets()[0].get(at(a)).map(|c| c.value.clone())
    }

    #[test]
    fn a_table_becomes_a_sheet_of_typed_values() {
        let book = read_content(&content(
            "<table:table-row>\
<table:table-cell office:value-type=\"string\"><text:p>привет</text:p></table:table-cell>\
<table:table-cell office:value-type=\"float\" office:value=\"3.5\"><text:p>3,50</text:p></table:table-cell>\
<table:table-cell office:value-type=\"boolean\" office:boolean-value=\"true\"><text:p>ИСТИНА</text:p></table:table-cell>\
</table:table-row>",
        ))
        .expect("reads");
        assert_eq!(book.sheets()[0].title(), "Лист");
        assert_eq!(value(&book, "A1"), Some(CellValue::text("привет")));
        // The typed value wins over the text a reader showed.
        assert_eq!(value(&book, "B1"), Some(CellValue::Number(3.5)));
        assert_eq!(value(&book, "C1"), Some(CellValue::Bool(true)));
    }

    #[test]
    fn a_repeat_count_is_a_run_of_copies_but_not_of_nothing() {
        let book = read_content(&content(
            "<table:table-row>\
<table:table-cell office:value-type=\"float\" office:value=\"1\" table:number-columns-repeated=\"3\"><text:p>1</text:p></table:table-cell>\
<table:table-cell table:number-columns-repeated=\"1013\"/>\
</table:table-row>\
<table:table-row table:number-rows-repeated=\"2\">\
<table:table-cell office:value-type=\"string\"><text:p>x</text:p></table:table-cell>\
</table:table-row>",
        ))
        .expect("reads");
        let sheet = &book.sheets()[0];
        // Three ones, then the empty tail leaves nothing behind.
        assert_eq!(value(&book, "C1"), Some(CellValue::Number(1.0)));
        assert_eq!(value(&book, "D1"), None);
        // A repeated row is a run of copies too.
        assert_eq!(value(&book, "A2"), Some(CellValue::text("x")));
        assert_eq!(value(&book, "A3"), Some(CellValue::text("x")));
        assert_eq!(sheet.len(), 5);
    }

    #[test]
    fn a_formula_arrives_in_a1_notation_with_its_result() {
        let book = read_content(&content(
            "<table:table-row>\
<table:table-cell table:formula=\"of:=SUM([.A2:.A3])\" office:value-type=\"float\" office:value=\"7\"><text:p>7</text:p></table:table-cell>\
</table:table-row>",
        ))
        .expect("reads");
        assert_eq!(
            value(&book, "A1"),
            Some(CellValue::Formula {
                formula: "SUM(A2:A3)".to_owned(),
                cached: Some(Box::new(CellValue::Number(7.0))),
            })
        );
    }

    #[test]
    fn a_span_becomes_a_merge_and_the_cells_under_it_stay_empty() {
        let book = read_content(&content(
            "<table:table-row>\
<table:table-cell table:number-columns-spanned=\"2\" table:number-rows-spanned=\"2\" office:value-type=\"string\"><text:p>wide</text:p></table:table-cell>\
<table:covered-table-cell/>\
</table:table-row>",
        ))
        .expect("reads");
        let sheet = &book.sheets()[0];
        assert_eq!(sheet.merges, vec![Range::parse("A1:B2").expect("range")]);
        assert_eq!(value(&book, "B1"), None);
    }

    #[test]
    fn dates_and_times_arrive_as_serials_with_a_format() {
        let book = read_content(&content(
            "<table:table-row>\
<table:table-cell office:value-type=\"date\" office:date-value=\"2024-01-05\"><text:p>2024-01-05</text:p></table:table-cell>\
<table:table-cell office:value-type=\"date\" office:date-value=\"2024-01-05T12:00:00\"><text:p>2024-01-05 12:00</text:p></table:table-cell>\
<table:table-cell office:value-type=\"time\" office:time-value=\"PT01H30M00S\"><text:p>01:30</text:p></table:table-cell>\
<table:table-cell office:value-type=\"percentage\" office:value=\"0.125\"><text:p>12.50%</text:p></table:table-cell>\
</table:table-row>",
        ))
        .expect("reads");
        assert_eq!(value(&book, "A1"), Some(CellValue::Number(45_296.0)));
        assert_eq!(value(&book, "B1"), Some(CellValue::Number(45_296.5)));
        assert_eq!(value(&book, "C1"), Some(CellValue::Number(0.0625)));
        assert_eq!(value(&book, "D1"), Some(CellValue::Number(0.125)));

        let sheet = &book.sheets()[0];
        let code = |a: &str| {
            book.styles
                .get(sheet.get(at(a)).expect("cell").style)
                .expect("style")
                .number_format
                .code()
                .to_owned()
        };
        assert_eq!(code("A1"), "yyyy-mm-dd");
        assert_eq!(code("B1"), "yyyy-mm-dd hh:mm:ss");
        assert_eq!(code("C1"), "hh:mm:ss");
        assert_eq!(code("D1"), "0.00%");
    }

    #[test]
    fn several_paragraphs_in_a_cell_are_several_lines() {
        let book = read_content(&content(
            "<table:table-row><table:table-cell office:value-type=\"string\">\
<text:p>one</text:p><text:p>two</text:p></table:table-cell></table:table-row>",
        ))
        .expect("reads");
        assert_eq!(value(&book, "A1"), Some(CellValue::text("one\ntwo")));
    }

    #[test]
    fn a_link_is_kept_with_the_cell_it_covers() {
        let book = read_content(&content(
            "<table:table-row><table:table-cell office:value-type=\"string\">\
<text:p><text:a xlink:href=\"https://example.test/\">click</text:a></text:p>\
</table:table-cell></table:table-row>",
        ))
        .expect("reads");
        let sheet = &book.sheets()[0];
        assert_eq!(value(&book, "A1"), Some(CellValue::text("click")));
        assert_eq!(
            sheet.hyperlinks[0].target,
            LinkTarget::Outside("https://example.test/".to_owned())
        );
    }

    #[test]
    fn a_package_that_is_not_a_spreadsheet_is_refused() {
        assert!(read_content("<office:body/>").is_err());
        assert!(duration_serial("PT25H00M00S").is_some_and(|d| d > 1.0));
        assert_eq!(iso_serial("nonsense"), None);
    }
}
