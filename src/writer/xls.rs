//! Writing xls (BIFF8).
//!
//! The records are the ones [`crate::reader::xls`] reads: the globals carry the
//! fonts, the number formats, the cell formats and the shared strings, and each
//! sheet is a substream of its own whose position the `BOUNDSHEET` record
//! names - which is why the sheets are laid out first and their positions
//! patched in afterwards.
//!
//! What is written: values of every type, shared strings (split over
//! `CONTINUE` records where they do not fit), the whole cell format - number
//! format, font, fill, borders, alignment and protection - merges, column
//! widths and row heights, and the workbook's base date.
//!
//! Colours go through the 56-entry palette ([`crate::shared::palette`]): a
//! colour the default palette has keeps its entry, one it lacks takes over an
//! entry no other colour of the workbook uses and a `PALETTE` record is
//! written, and past 56 distinct colours the rest get the nearest entry. Theme
//! colours are resolved through the workbook's theme first.
//!
//! Formulas are compiled into tokens ([`super::xls_formula`]) and written
//! with their result, and defined names go out as `NAME` records. A formula
//! BIFF8 cannot hold - a structured reference, a reference past row 65536 or
//! column IV - is written as its result, so the file still says what the sheet
//! showed.
//!
//! What is not:
//!
//! - Everything the model carries for xlsx and the old format has no room for:
//!   conditional formatting, data validation, rich text inside a cell, opaque
//!   parts.

use super::xls_formula::{self, Compiled, Links, Place};
use crate::error::{Error, Result};
use crate::formula::eval::{Engine, Origin};
use crate::formula::value::Value as FormulaValue;
use crate::model::{CellValue, Spreadsheet, Worksheet};
use crate::shared::date::Epoch;
use crate::shared::palette;
use crate::style::{
    BorderStyle, Color, DiagonalDirection, HorizontalAlign, NumberFormat, Pattern, ProtectionState,
    Script, Style, VerticalAlign,
};
use crate::{CellRef, Col, Row};
use std::collections::HashMap;
use std::io::Write;

/// The largest payload one record may carry; anything longer continues in a
/// `CONTINUE` record.
const MAX_PAYLOAD: usize = 8224;

/// The first index a format of our own may take; below it the format numbers
/// are Excel's own.
const FIRST_CUSTOM_FORMAT: u16 = 164;

/// Writes a workbook as an xls file.
///
/// # Errors
/// [`Error::Xls`] if the workbook has no sheets or the file cannot be written.
pub fn write_xls(book: &Spreadsheet, path: impl AsRef<std::path::Path>) -> Result<()> {
    let file = std::fs::File::create(path).map_err(|e| Error::Xls(e.to_string()))?;
    write_xls_to(book, std::io::BufWriter::new(file))
}

/// The same, to any sink.
///
/// # Errors
/// As [`write_xls`].
pub fn write_xls_to<W: Write>(book: &Spreadsheet, mut sink: W) -> Result<()> {
    let bytes = workbook_stream(book)?;
    let container = super::ole::container("Workbook", &bytes);
    sink.write_all(&container)
        .map_err(|e| Error::Xls(e.to_string()))?;
    sink.flush().map_err(|e| Error::Xls(e.to_string()))
}

/// Builds the whole `Workbook` stream: globals first, then one substream per
/// sheet.
fn workbook_stream(book: &Spreadsheet) -> Result<Vec<u8>> {
    if book.sheets().is_empty() {
        return Err(Error::Xls("a workbook needs at least one sheet".to_owned()));
    }
    let mut engine = Engine::new(book);
    let mut plan = Plan::new(book, &mut engine);

    // The sheets are built first: the globals name where each one starts, and
    // that is only known once the globals have their own length.
    let sheets: Vec<Vec<u8>> = (0..book.sheets().len())
        .filter_map(|i| book.sheet(i).map(|sheet| substream(sheet, i, &plan)))
        .collect();

    let mut globals = plan.globals(book);
    // `BOUNDSHEET` holds a position each; the placeholders are patched once the
    // globals are as long as they are going to be.
    let mut at = globals.len() + 4;
    let mut positions = Vec::with_capacity(sheets.len());
    for sheet in &sheets {
        positions.push(at);
        at += sheet.len();
    }
    for (offset, position) in plan.boundsheet_offsets.iter().zip(&positions) {
        let value = u32::try_from(*position).unwrap_or(0);
        globals[*offset..*offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    record(&mut globals, 0x000A, &[]);

    let mut out = globals;
    for sheet in sheets {
        out.extend_from_slice(&sheet);
    }
    Ok(out)
}

/// What the globals hold, worked out before anything is written.
struct Plan {
    /// The palette every colour index refers to.
    palette: Colors,
    /// Every font of the style table, in the order they are written.
    fonts: Vec<crate::style::Font>,
    /// The font index of each style, by style id.
    font_of: Vec<u16>,
    /// The format index of each style, by style id.
    format_of: Vec<u16>,
    /// Format codes to spell out, with the index each takes.
    formats: Vec<(u16, String)>,
    /// Every distinct string, in the order the table holds them.
    strings: Vec<String>,
    /// Where each string sits in the table.
    string_index: HashMap<String, u32>,
    /// How many string cells there are altogether.
    string_uses: u32,
    /// What each formula cell works out to. A formula is written as its
    /// result, so the result has to be known before the string table is, and
    /// working it out twice would mean two passes of the engine.
    resolved: HashMap<(usize, CellRef), CellValue>,
    /// The formulas that compiled, by cell.
    compiled: HashMap<(usize, CellRef), Compiled>,
    /// Each defined name's tokens, in book order; `None` for one that did not
    /// compile, which is still declared so the numbering holds.
    names: Vec<Option<Compiled>>,
    /// The sheets, books and names the formulas refer to.
    links: Links,
    /// Where in the globals each `BOUNDSHEET` keeps its sheet's position.
    boundsheet_offsets: Vec<usize>,
}

impl Plan {
    /// Walks the workbook once, collecting what the globals have to declare.
    fn new(book: &Spreadsheet, engine: &mut Engine<'_>) -> Self {
        let mut plan = Self {
            palette: Colors::plan(book),
            fonts: Vec::new(),
            font_of: Vec::new(),
            format_of: Vec::new(),
            formats: Vec::new(),
            strings: Vec::new(),
            string_index: HashMap::new(),
            string_uses: 0,
            resolved: HashMap::new(),
            compiled: HashMap::new(),
            names: Vec::new(),
            links: Links::new(
                book.sheets().iter().map(|s| s.title().to_owned()).collect(),
                book.defined_names
                    .iter()
                    .map(|n| (n.name.clone(), n.sheet))
                    .collect(),
            ),
            boundsheet_offsets: Vec::new(),
        };
        for style in book.styles.all() {
            plan.font_of.push(plan_font(&mut plan.fonts, &style.font));
            plan.format_of.push(plan_format(&mut plan.formats, style));
        }
        // Excel treats font 4 as absent, so a workbook needs four fonts before
        // the numbering can skip it.
        while plan.fonts.len() < 4 {
            let first = plan.fonts.first().cloned().unwrap_or_default();
            plan.fonts.push(first);
        }
        for (index, sheet) in book.sheets().iter().enumerate() {
            for (at, cell) in sheet.iter() {
                let value = match &cell.value {
                    CellValue::Formula { formula, cached } => {
                        let value = match cached {
                            Some(cached) => (**cached).clone(),
                            None => from_formula(engine.eval(Origin::new(index, at), formula)),
                        };
                        plan.resolved.insert((index, at), value.clone());
                        let compiled =
                            crate::formula::parser::parse(formula)
                                .ok()
                                .and_then(|expr| {
                                    xls_formula::compile(&expr, Place::Cell(index), &mut plan.links)
                                });
                        if let Some(compiled) = compiled {
                            // A formula's text result waits in a `STRING`
                            // record, not in the shared table.
                            plan.compiled.insert((index, at), compiled);
                            continue;
                        }
                        value
                    }
                    other => other.clone(),
                };
                if let Some(text) = string_of(&value) {
                    plan.string_uses += 1;
                    if !plan.string_index.contains_key(&text) {
                        let index = u32::try_from(plan.strings.len()).unwrap_or(0);
                        plan.string_index.insert(text.clone(), index);
                        plan.strings.push(text);
                    }
                }
            }
        }
        for name in &book.defined_names {
            let compiled = crate::formula::parser::parse(&name.formula)
                .ok()
                .and_then(|expr| {
                    xls_formula::compile(&expr, Place::Name(name.sheet), &mut plan.links)
                });
            plan.names.push(compiled);
        }
        plan
    }

    /// The globals substream, up to but not including its `EOF`.
    fn globals(&mut self, book: &Spreadsheet) -> Vec<u8> {
        let mut out = Vec::new();
        record(
            &mut out,
            0x0809,
            &[
                0x00, 0x06, // BIFF8
                0x05, 0x00, // the globals substream
                0xBB, 0x0D, 0xCC, 0x07, // build and year, as Excel writes them
                0xC9, 0x80, 0x00, 0x00, 0x06, 0x06, 0x00, 0x00,
            ],
        );
        // The strings are Unicode, which is what this code page says.
        record(&mut out, 0x0042, &0x04B0u16.to_le_bytes());
        let mode = u16::from(book.epoch == Epoch::Mac1904);
        record(&mut out, 0x0022, &mode.to_le_bytes());

        for font in &self.fonts {
            record(&mut out, 0x0031, &font_record(font, &self.palette));
        }
        for (index, code) in &self.formats {
            let mut data = index.to_le_bytes().to_vec();
            data.extend_from_slice(&unicode_string(code));
            record(&mut out, 0x041E, &data);
        }
        // Excel wants fifteen style formats before the first cell one, and a
        // cell format's parent is the first of them.
        for _ in 0..15 {
            record(
                &mut out,
                0x00E0,
                &xf_record(&Style::default(), 0, 0, true, &self.palette),
            );
        }
        for (id, style) in book.styles.all().iter().enumerate() {
            let font = self.font_of.get(id).copied().unwrap_or(0);
            let format = self.format_of.get(id).copied().unwrap_or(0);
            record(
                &mut out,
                0x00E0,
                &xf_record(style, font, format, false, &self.palette),
            );
        }
        // The `Normal` style, which every cell format hangs off.
        record(&mut out, 0x0293, &[0x00, 0x80, 0x00, 0xFF]);
        if let Some(colors) = self.palette.builder.custom() {
            let mut data = 56u16.to_le_bytes().to_vec();
            for rgb in colors {
                let [_, r, g, b] = rgb.to_be_bytes();
                data.extend_from_slice(&[r, g, b, 0]);
            }
            record(&mut out, 0x0092, &data);
        }

        for sheet in book.sheets() {
            let mut data = vec![0, 0, 0, 0, 0, 0];
            data.extend_from_slice(&short_string(sheet.title()));
            // The position sits four bytes past the record's own header, and
            // is patched once the globals are complete.
            self.boundsheet_offsets.push(out.len() + 4);
            record(&mut out, 0x0085, &data);
        }
        self.links_and_names(book, &mut out);
        self.shared_strings(&mut out);
        out
    }

    /// The books references go through, the table of them, and the defined
    /// names, in the order the format wants them: `SUPBOOK` records with their
    /// `EXTERNNAME`s, then `EXTERNSHEET`, then `NAME`.
    fn links_and_names(&self, book: &Spreadsheet, out: &mut Vec<u8>) {
        let links = &self.links;
        if !links.externs.is_empty() {
            let sheets = u16::try_from(book.sheets().len()).unwrap_or(u16::MAX);
            let mut data = sheets.to_le_bytes().to_vec();
            data.extend_from_slice(&[0x01, 0x04]);
            record(out, 0x01AE, &data);
        }
        if !links.add_ins.is_empty() {
            record(out, 0x01AE, &[0x01, 0x00, 0x01, 0x3A]);
            for name in &links.add_ins {
                let mut data = vec![0, 0, 0, 0, 0, 0];
                data.extend_from_slice(&short_string(name));
                // The name's own formula, which for a function is `#REF!`.
                data.extend_from_slice(&[0x02, 0x00, 0x1C, 0x17]);
                record(out, 0x0023, &data);
            }
        }
        if !links.externs.is_empty() {
            let count = u16::try_from(links.externs.len()).unwrap_or(u16::MAX);
            let mut data = count.to_le_bytes().to_vec();
            for (book, first, last) in &links.externs {
                data.extend_from_slice(&book.to_le_bytes());
                data.extend_from_slice(&first.to_le_bytes());
                data.extend_from_slice(&last.to_le_bytes());
            }
            record(out, 0x0017, &data);
        }
        for (name, compiled) in book.defined_names.iter().zip(&self.names) {
            record(out, 0x0018, &name_record(name, compiled.as_ref()));
        }
    }

    /// The shared string table, split across `CONTINUE` records where a record
    /// cannot hold it all.
    fn shared_strings(&self, out: &mut Vec<u8>) {
        let mut chunks: Vec<Vec<u8>> = vec![Vec::new()];
        // The first chunk carries the two counts before any string.
        let unique = u32::try_from(self.strings.len()).unwrap_or(0);
        if let Some(first) = chunks.first_mut() {
            first.extend_from_slice(&self.string_uses.to_le_bytes());
            first.extend_from_slice(&unique.to_le_bytes());
        }
        for text in &self.strings {
            write_sst_string(&mut chunks, text);
        }
        let mut chunks = chunks.into_iter();
        if let Some(first) = chunks.next() {
            record(out, 0x00FC, &first);
        }
        for chunk in chunks {
            record(out, 0x003C, &chunk);
        }
    }
}

/// Adds a font to the list if it is new, and returns the index a cell format
/// refers to it by. Index 4 is skipped: Excel has never used it.
fn plan_font(fonts: &mut Vec<crate::style::Font>, font: &crate::style::Font) -> u16 {
    let position = fonts.iter().position(|f| f == font).unwrap_or_else(|| {
        fonts.push(font.clone());
        fonts.len() - 1
    });
    let position = u16::try_from(position).unwrap_or(0);
    if position < 4 { position } else { position + 1 }
}

/// The number format index of a style, spelling the code out if Excel has no
/// number of its own for it.
fn plan_format(formats: &mut Vec<(u16, String)>, style: &Style) -> u16 {
    match &style.number_format {
        NumberFormat::General => 0,
        NumberFormat::Builtin(id) => *id,
        NumberFormat::Custom(code) => {
            if let Some((index, _)) = formats.iter().find(|(_, known)| known == code) {
                return *index;
            }
            let index = FIRST_CUSTOM_FORMAT + u16::try_from(formats.len()).unwrap_or(0);
            formats.push((index, code.clone()));
            index
        }
    }
}

/// One formula result as a cell value.
fn from_formula(value: FormulaValue) -> CellValue {
    match value {
        FormulaValue::Number(n) => CellValue::Number(n),
        FormulaValue::Text(t) => CellValue::Text(t.into()),
        FormulaValue::Bool(b) => CellValue::Bool(b),
        FormulaValue::Error(e) => CellValue::Error(e),
        // A cell cannot hold a function; Excel shows `#CALC!`.
        FormulaValue::Lambda(_) => CellValue::Error(crate::error::CellError::Calc),
        // An array shows its top-left value, as everywhere else a cell holds
        // one value and the format has no spill.
        FormulaValue::Array(rows) => rows
            .first()
            .and_then(|line| line.first())
            .cloned()
            .map_or(CellValue::Empty, from_formula),
        FormulaValue::Blank => CellValue::Empty,
    }
}

/// The text a cell is stored as a shared string for. Rich text loses its runs
/// on the way: the old format keeps them in the string table, and the model
/// keeps them on the cell.
fn string_of(value: &CellValue) -> Option<String> {
    match value {
        CellValue::Text(text) => Some(text.to_string()),
        CellValue::RichText(runs) => Some(runs.iter().map(|run| run.text.as_str()).collect()),
        _ => None,
    }
}

/// One sheet substream.
fn substream(sheet: &Worksheet, index: usize, plan: &Plan) -> Vec<u8> {
    let mut out = Vec::new();
    record(
        &mut out,
        0x0809,
        &[
            0x00, 0x06, // BIFF8
            0x10, 0x00, // a worksheet substream
            0xBB, 0x0D, 0xCC, 0x07, 0xC9, 0x80, 0x00, 0x00, 0x06, 0x06, 0x00, 0x00,
        ],
    );

    let used = sheet.dimension();
    let (first_row, last_row, first_col, last_col) = used.map_or((0, 0, 0, 0), |range| {
        (
            range.start.row.index(),
            range.end.row.index() + 1,
            range.start.col.index(),
            range.end.col.index() + 1,
        )
    });
    let mut dimension = Vec::with_capacity(14);
    dimension.extend_from_slice(&first_row.to_le_bytes());
    dimension.extend_from_slice(&last_row.to_le_bytes());
    dimension.extend_from_slice(&u16::try_from(first_col).unwrap_or(0).to_le_bytes());
    dimension.extend_from_slice(&u16::try_from(last_col).unwrap_or(0).to_le_bytes());
    dimension.extend_from_slice(&0u16.to_le_bytes());
    record(&mut out, 0x0200, &dimension);

    for run in &sheet.columns {
        let mut data = Vec::with_capacity(12);
        data.extend_from_slice(&run.first.index_u16().to_le_bytes());
        data.extend_from_slice(&run.last.index_u16().to_le_bytes());
        // The width is in 256ths of a character.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a column width is a small positive number"
        )]
        let width = (run.width.unwrap_or(8.43) * 256.0).round().max(0.0) as u32;
        data.extend_from_slice(&u16::try_from(width).unwrap_or(u16::MAX).to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&u16::from(run.hidden).to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        record(&mut out, 0x007D, &data);
    }

    for (row, properties) in &sheet.rows {
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&row.index_u16().to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a row height in twips is a small positive number"
        )]
        let height = match properties.height {
            Some(points) => ((points * 20.0).round().max(0.0) as u32).min(0x7FFF),
            // Bit 15 says the row keeps the default height.
            None => 0x8000,
        };
        data.extend_from_slice(&u16::try_from(height).unwrap_or(0x8000).to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        let mut flags = 0x0000_0100u32;
        if properties.hidden {
            flags |= 0x20;
        }
        if properties.height.is_some() {
            flags |= 0x40;
        }
        flags |= u32::from(properties.outline_level) & 0x07;
        data.extend_from_slice(&flags.to_le_bytes());
        record(&mut out, 0x0208, &data);
    }

    for (at, cell) in sheet.iter() {
        cell_record(&mut out, index, at, cell, plan);
    }

    // A record holds 1027 merges at most.
    for chunk in sheet.merges.chunks(1027) {
        let mut data = u16::try_from(chunk.len())
            .unwrap_or(0)
            .to_le_bytes()
            .to_vec();
        for range in chunk {
            data.extend_from_slice(&range.start.row.index_u16().to_le_bytes());
            data.extend_from_slice(&range.end.row.index_u16().to_le_bytes());
            data.extend_from_slice(&range.start.col.index_u16().to_le_bytes());
            data.extend_from_slice(&range.end.col.index_u16().to_le_bytes());
        }
        record(&mut out, 0x00E5, &data);
    }

    // Without a window record Excel opens the sheet with no gridlines and no
    // headings, which is not what the model said.
    let mut window = 0x06B6u16;
    if !sheet.view.show_grid_lines {
        window &= !0x0020;
    }
    let mut data = window.to_le_bytes().to_vec();
    data.extend_from_slice(&[0, 0, 0, 0]);
    data.extend_from_slice(&0x0000_0040u32.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    record(&mut out, 0x023E, &data);

    record(&mut out, 0x000A, &[]);
    out
}

/// One cell, as whichever record carries its kind of value.
fn cell_record(
    out: &mut Vec<u8>,
    sheet: usize,
    at: CellRef,
    cell: &crate::model::Cell,
    plan: &Plan,
) {
    let xf = cell_format(plan, cell.style);
    let head = |data: &mut Vec<u8>| {
        data.extend_from_slice(&at.row.index_u16().to_le_bytes());
        data.extend_from_slice(&at.col.index_u16().to_le_bytes());
        data.extend_from_slice(&xf.to_le_bytes());
    };

    if let Some(compiled) = plan.compiled.get(&(sheet, at)) {
        let result = plan.resolved.get(&(sheet, at)).unwrap_or(&CellValue::Empty);
        formula_record(out, at, xf, result, compiled);
        return;
    }

    // A formula that did not compile is written as what it worked out to. The
    // result was worked out while the string table was being planned.
    let value = match &cell.value {
        CellValue::Formula { .. } => plan
            .resolved
            .get(&(sheet, at))
            .cloned()
            .unwrap_or(CellValue::Empty),
        other => other.clone(),
    };

    let mut data = Vec::with_capacity(16);
    match &value {
        CellValue::Number(n) => {
            head(&mut data);
            data.extend_from_slice(&n.to_le_bytes());
            record(out, 0x0203, &data);
        }
        CellValue::Text(text) => {
            head(&mut data);
            let index = plan.string_index.get(text.as_ref()).copied().unwrap_or(0);
            data.extend_from_slice(&index.to_le_bytes());
            record(out, 0x00FD, &data);
        }
        CellValue::Bool(b) => {
            head(&mut data);
            data.push(u8::from(*b));
            data.push(0);
            record(out, 0x0205, &data);
        }
        CellValue::Error(e) => {
            head(&mut data);
            data.push(xls_formula::error_code(*e));
            data.push(1);
            record(out, 0x0205, &data);
        }
        // Rich text keeps only its text; the old format holds the runs in the
        // string table rather than on the cell.
        CellValue::RichText(_) => {
            let text = string_of(&value).unwrap_or_default();
            head(&mut data);
            let index = plan.string_index.get(&text).copied().unwrap_or(0);
            data.extend_from_slice(&index.to_le_bytes());
            record(out, 0x00FD, &data);
        }
        CellValue::Empty | CellValue::Formula { .. } => {
            head(&mut data);
            record(out, 0x0201, &data);
        }
    }
}

/// A `FORMULA` record, and the `STRING` record after it when the result is
/// text.
fn formula_record(
    out: &mut Vec<u8>,
    at: CellRef,
    xf: u16,
    result: &CellValue,
    compiled: &Compiled,
) {
    let mut data = Vec::with_capacity(22 + compiled.tokens.len() + compiled.extra.len());
    data.extend_from_slice(&at.row.index_u16().to_le_bytes());
    data.extend_from_slice(&at.col.index_u16().to_le_bytes());
    data.extend_from_slice(&xf.to_le_bytes());
    // A result that is not a number says what it is in its first byte and
    // marks itself with 0xFFFF in its last two.
    let special = |kind: u8, value: u8| [kind, 0, value, 0, 0, 0, 0xFF, 0xFF];
    let text = match result {
        CellValue::Number(n) => {
            data.extend_from_slice(&n.to_le_bytes());
            None
        }
        CellValue::Text(_) | CellValue::RichText(_) => {
            data.extend_from_slice(&special(0, 0));
            string_of(result)
        }
        CellValue::Bool(b) => {
            data.extend_from_slice(&special(1, u8::from(*b)));
            None
        }
        CellValue::Error(e) => {
            data.extend_from_slice(&special(2, xls_formula::error_code(*e)));
            None
        }
        CellValue::Empty | CellValue::Formula { .. } => {
            data.extend_from_slice(&special(3, 0));
            None
        }
    };
    // Options, then the chain cookie Excel fills in itself.
    data.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let length = u16::try_from(compiled.tokens.len()).unwrap_or(0);
    data.extend_from_slice(&length.to_le_bytes());
    data.extend_from_slice(&compiled.tokens);
    data.extend_from_slice(&compiled.extra);
    record(out, 0x0006, &data);
    if let Some(text) = text {
        record(out, 0x0207, &unicode_string(&text));
    }
}

/// A `NAME` record. A built-in name (`_xlnm.Print_Area`) is stored as a
/// single character code rather than spelled out.
fn name_record(name: &crate::model::DefinedName, compiled: Option<&Compiled>) -> Vec<u8> {
    const BUILTIN: [&str; 14] = [
        "Consolidate_Area",
        "Auto_Open",
        "Auto_Close",
        "Extract",
        "Database",
        "Criteria",
        "Print_Area",
        "Print_Titles",
        "Recorder",
        "Data_Form",
        "Auto_Activate",
        "Auto_Deactivate",
        "Sheet_Title",
        "_FilterDatabase",
    ];
    let builtin = name
        .name
        .strip_prefix("_xlnm.")
        .and_then(|rest| BUILTIN.iter().position(|b| b.eq_ignore_ascii_case(rest)));
    let mut flags = u16::from(name.hidden);
    let units: Vec<u16> = match builtin {
        Some(code) => {
            flags |= 0x20;
            vec![u16::try_from(code).unwrap_or(0)]
        }
        None => name.name.encode_utf16().take(255).collect(),
    };
    let wide = units.iter().any(|&u| u > 0xFF);
    let (tokens, extra): (&[u8], &[u8]) = compiled.map_or((&[], &[]), |c| (&c.tokens, &c.extra));
    let mut data = flags.to_le_bytes().to_vec();
    data.push(0);
    data.push(u8::try_from(units.len()).unwrap_or(0));
    data.extend_from_slice(&u16::try_from(tokens.len()).unwrap_or(0).to_le_bytes());
    data.extend_from_slice(&[0, 0]);
    let sheet = name.sheet.map_or(0, |s| s + 1);
    data.extend_from_slice(&u16::try_from(sheet).unwrap_or(0).to_le_bytes());
    data.extend_from_slice(&[0, 0, 0, 0]);
    data.push(u8::from(wide));
    push_units(&mut data, &units, wide);
    data.extend_from_slice(tokens);
    data.extend_from_slice(extra);
    data
}

/// The `XF` index of a style: the fifteen style formats come first.
fn cell_format(plan: &Plan, style: crate::style::StyleId) -> u16 {
    let index = u16::try_from(style.index()).unwrap_or(0);
    if usize::from(index) < plan.format_of.len() {
        15 + index
    } else {
        15
    }
}

/// A `FONT` record.
fn font_record(font: &crate::style::Font, palette: &Colors) -> Vec<u8> {
    let mut data = Vec::with_capacity(24);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a font size in twips is a small positive number"
    )]
    let height = (font.size_points() * 20.0).round().max(0.0) as u32;
    data.extend_from_slice(&u16::try_from(height).unwrap_or(220).to_le_bytes());
    // Bit 0 says bold beside the weight; older readers look only at it.
    let mut flags = u16::from(font.bold);
    if font.italic {
        flags |= 0x0002;
    }
    if font.strike {
        flags |= 0x0008;
    }
    data.extend_from_slice(&flags.to_le_bytes());
    data.extend_from_slice(&palette.index(&font.color, 0x7FFF).to_le_bytes());
    data.extend_from_slice(&if font.bold { 700u16 } else { 400u16 }.to_le_bytes());
    let escapement: u16 = match font.script {
        Script::Baseline => 0,
        Script::Superscript => 1,
        Script::Subscript => 2,
    };
    data.extend_from_slice(&escapement.to_le_bytes());
    data.push(match font.underline {
        crate::style::Underline::None => 0x00,
        crate::style::Underline::Double => 0x02,
        crate::style::Underline::SingleAccounting => 0x21,
        crate::style::Underline::DoubleAccounting => 0x22,
        crate::style::Underline::Single => 0x01,
    });
    let small = |value: Option<u32>| value.and_then(|v| u8::try_from(v).ok()).unwrap_or(0);
    data.extend_from_slice(&[small(font.family), small(font.charset), 0]);
    data.extend_from_slice(&short_string(&font.name));
    data
}

/// An `XF` record: which font, which format, how the text sits, and the
/// borders, fill and protection.
fn xf_record(style: &Style, font: u16, format: u16, is_style: bool, palette: &Colors) -> Vec<u8> {
    let mut data = Vec::with_capacity(20);
    data.extend_from_slice(&font.to_le_bytes());
    data.extend_from_slice(&format.to_le_bytes());
    // Bit 0 locks the cell, bit 1 hides its formula, bit 2 marks a style
    // format; a style format has no parent and says so with 0xFFF, a cell
    // format points at the first style format.
    let mut protection: u16 = if is_style { 0xFFF4 } else { 0x0000 };
    if style.protection.locked != ProtectionState::Off {
        protection |= 0x01;
    }
    if style.protection.hidden == ProtectionState::On {
        protection |= 0x02;
    }
    data.extend_from_slice(&protection.to_le_bytes());

    let mut align = match style.alignment.horizontal {
        HorizontalAlign::General => 0u8,
        HorizontalAlign::Left => 1,
        HorizontalAlign::Center => 2,
        HorizontalAlign::Right => 3,
        HorizontalAlign::Fill => 4,
        HorizontalAlign::Justify => 5,
        HorizontalAlign::CenterContinuous => 6,
        HorizontalAlign::Distributed => 7,
    };
    if style.alignment.wrap_text {
        align |= 0x08;
    }
    align |= match style.alignment.vertical {
        VerticalAlign::Top => 0u8,
        VerticalAlign::Center => 1,
        VerticalAlign::Bottom => 2,
        VerticalAlign::Justify => 3,
        VerticalAlign::Distributed => 4,
    } << 4;
    data.push(align);
    // The rotation byte counts the way the model does: 0 to 180, 255 stacked.
    data.push(u8::try_from(style.alignment.text_rotation).unwrap_or(0));
    let mut options = u8::try_from(style.alignment.indent & 0x0F).unwrap_or(0);
    if style.alignment.shrink_to_fit {
        options |= 0x10;
    }
    options |= u8::try_from(style.alignment.reading_order & 0x03).unwrap_or(0) << 6;
    data.push(options);

    let borders = &style.borders;
    let fill = &style.fill;
    let has_borders = [&borders.left, &borders.right, &borders.top, &borders.bottom]
        .iter()
        .any(|side| side.style != BorderStyle::None)
        || borders.diagonal_direction != DiagonalDirection::None;
    let has_alignment = style.alignment != crate::style::Alignment::default();
    // Which groups of attributes this format sets, rather than inheriting.
    let mut used = 0u8;
    if format != 0 {
        used |= 0x04;
    }
    if font != 0 {
        used |= 0x08;
    }
    if has_alignment {
        used |= 0x10;
    }
    if has_borders {
        used |= 0x20;
    }
    if fill.pattern != Pattern::None {
        used |= 0x40;
    }
    if style.protection != crate::style::Protection::default() {
        used |= 0x80;
    }
    data.push(used);

    data.extend_from_slice(&borders_and_fill(style, palette));
    data
}

/// The last ten bytes of an `XF` record: line styles and colours of the four
/// sides and the diagonal, then the fill pattern and its two colours.
fn borders_and_fill(style: &Style, palette: &Colors) -> Vec<u8> {
    let borders = &style.borders;
    let fill = &style.fill;
    let mut data = Vec::with_capacity(10);
    // A side with no line keeps colour 0, which is what Excel writes.
    let side = |border: &crate::style::Border| {
        let line = border_code(border.style);
        let color = if line == 0 {
            0
        } else {
            palette.index(&border.color, 0x40)
        };
        (u32::from(line), u32::from(color))
    };
    let (left, left_color) = side(&borders.left);
    let (right, right_color) = side(&borders.right);
    let (top, top_color) = side(&borders.top);
    let (bottom, bottom_color) = side(&borders.bottom);
    let (diagonal, diagonal_color) = if borders.diagonal_direction == DiagonalDirection::None {
        (0, 0)
    } else {
        side(&borders.diagonal)
    };
    let mut sides = left | (right << 4) | (top << 8) | (bottom << 12);
    sides |= (left_color << 16) | (right_color << 23);
    if matches!(
        borders.diagonal_direction,
        DiagonalDirection::Down | DiagonalDirection::Both
    ) {
        sides |= 1 << 30;
    }
    if matches!(
        borders.diagonal_direction,
        DiagonalDirection::Up | DiagonalDirection::Both
    ) {
        sides |= 1 << 31;
    }
    data.extend_from_slice(&sides.to_le_bytes());

    let pattern = pattern_code(fill.pattern);
    let more = top_color
        | (bottom_color << 7)
        | (diagonal_color << 14)
        | (diagonal << 21)
        | (u32::from(pattern) << 26);
    data.extend_from_slice(&more.to_le_bytes());
    // An unfilled cell keeps the system foreground and background.
    let (foreground, background) = if pattern == 0 {
        (0x40, 0x41)
    } else {
        (
            palette.index(&fill.foreground, 0x40),
            palette.index(&fill.background, 0x41),
        )
    };
    data.extend_from_slice(&((foreground & 0x7F) | ((background & 0x7F) << 7)).to_le_bytes());
    data
}

/// A line style as BIFF numbers it.
fn border_code(style: BorderStyle) -> u8 {
    match style.as_str() {
        "thin" => 1,
        "medium" => 2,
        "dashed" => 3,
        "dotted" => 4,
        "thick" => 5,
        "double" => 6,
        "hair" => 7,
        "mediumDashed" => 8,
        "dashDot" => 9,
        "mediumDashDot" => 10,
        "dashDotDot" => 11,
        "mediumDashDotDot" => 12,
        _ => 0,
    }
}

/// A fill pattern as BIFF numbers it.
fn pattern_code(pattern: Pattern) -> u8 {
    match pattern.as_str() {
        "solid" => 1,
        "mediumGray" => 2,
        "darkGray" => 3,
        "lightGray" => 4,
        "darkHorizontal" => 5,
        "darkVertical" => 6,
        "darkDown" => 7,
        "darkUp" => 8,
        "darkGrid" => 9,
        "darkTrellis" => 10,
        "lightHorizontal" => 11,
        "lightVertical" => 12,
        "lightDown" => 13,
        "lightUp" => 14,
        "lightGrid" => 15,
        "lightTrellis" => 16,
        "gray125" => 17,
        "gray0625" => 18,
        _ => 0,
    }
}

/// The workbook's colours laid out in a palette, with the theme that resolves
/// its theme colours.
struct Colors {
    builder: palette::Builder,
    theme: Option<String>,
}

impl Colors {
    /// Collects every colour the style table uses, in style order.
    fn plan(book: &Spreadsheet) -> Self {
        let theme = book.theme.clone();
        let mut used = Vec::new();
        for style in book.styles.all() {
            let borders = &style.borders;
            let mut colors = vec![&style.font.color];
            if style.fill.pattern != Pattern::None {
                colors.extend([&style.fill.foreground, &style.fill.background]);
            }
            for side in [
                &borders.left,
                &borders.right,
                &borders.top,
                &borders.bottom,
                &borders.diagonal,
            ] {
                if side.style != BorderStyle::None {
                    colors.push(&side.color);
                }
            }
            used.extend(
                colors
                    .into_iter()
                    .filter_map(|c| palette::rgb_of(c, theme.as_deref())),
            );
        }
        Self {
            builder: palette::Builder::plan(&used),
            theme,
        }
    }

    /// The palette index for a colour, or `automatic` for one with no value.
    fn index(&self, color: &Color, automatic: u16) -> u16 {
        palette::rgb_of(color, self.theme.as_deref())
            .map_or(automatic, |rgb| self.builder.index_of(rgb))
    }
}

/// A string with a one-byte character count, as `BOUNDSHEET` and `FONT` write
/// it.
fn short_string(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().take(255).collect();
    let wide = units.iter().any(|&u| u > 0xFF);
    let mut out = Vec::with_capacity(units.len() * 2 + 2);
    out.push(u8::try_from(units.len()).unwrap_or(0));
    out.push(u8::from(wide));
    push_units(&mut out, &units, wide);
    out
}

/// A string with a two-byte character count, as `FORMAT` writes it.
fn unicode_string(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().take(0xFFFF).collect();
    let wide = units.iter().any(|&u| u > 0xFF);
    let mut out = Vec::with_capacity(units.len() * 2 + 3);
    out.extend_from_slice(&u16::try_from(units.len()).unwrap_or(0).to_le_bytes());
    out.push(u8::from(wide));
    push_units(&mut out, &units, wide);
    out
}

/// The characters themselves, one byte each or two.
fn push_units(out: &mut Vec<u8>, units: &[u16], wide: bool) {
    for &unit in units {
        if wide {
            out.extend_from_slice(&unit.to_le_bytes());
        } else {
            out.push(u8::try_from(unit).unwrap_or(b'?'));
        }
    }
}

/// One string of the shared table, continuing into another record when the
/// current one is full.
///
/// A record that runs out mid-string is continued by a `CONTINUE` whose first
/// byte says the width of what follows - the same rule the reader takes apart.
fn write_sst_string(chunks: &mut Vec<Vec<u8>>, text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(0xFFFF).collect();
    let wide = units.iter().any(|&u| u > 0xFF);
    let size = if wide { 2 } else { 1 };

    // The header is never split: a chunk with no room for it starts a new one.
    if chunks
        .last()
        .is_none_or(|chunk| chunk.len() + 3 + size > MAX_PAYLOAD)
    {
        chunks.push(Vec::new());
    }
    if let Some(chunk) = chunks.last_mut() {
        chunk.extend_from_slice(&u16::try_from(units.len()).unwrap_or(0).to_le_bytes());
        chunk.push(u8::from(wide));
    }

    let mut written = 0;
    while written < units.len() {
        let room = match chunks.last() {
            Some(chunk) => MAX_PAYLOAD.saturating_sub(chunk.len()) / size,
            None => 0,
        };
        if room == 0 {
            chunks.push(vec![u8::from(wide)]);
            continue;
        }
        let take = room.min(units.len() - written);
        if let Some(chunk) = chunks.last_mut() {
            push_units(chunk, &units[written..written + take], wide);
        }
        written += take;
    }
}

/// Appends one record: its number, its length, and its payload.
fn record(out: &mut Vec<u8>, id: u16, data: &[u8]) {
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&u16::try_from(data.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(data);
}

/// The zero-based index a record writes, as two bytes.
trait IndexU16 {
    fn index_u16(self) -> u16;
}

impl IndexU16 for Row {
    fn index_u16(self) -> u16 {
        u16::try_from(self.index()).unwrap_or(u16::MAX)
    }
}

impl IndexU16 for Col {
    fn index_u16(self) -> u16 {
        u16::try_from(self.index()).unwrap_or(u16::MAX)
    }
}
