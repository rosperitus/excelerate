//! Reading xls (BIFF8 and BIFF5).
//!
//! An xls is a stream of records - a two-byte number, a two-byte length, then
//! that many bytes - kept in the `Workbook` stream of an OLE compound file (see
//! [`super::ole`]). The globals come first and hold the shared strings, the
//! formats and the list of sheets; each sheet is a substream that starts where
//! its `BOUNDSHEET` record says.
//!
//! What is read: sheets and their names, every cell value type, the shared
//! string table (with its continuations), the whole cell format - number
//! format, font, fill, borders, alignment and protection, with colours
//! resolved through the file's palette - merges, column widths and row
//! heights, the workbook's base date, formulas and defined names.
//!
//! A formula is stored as tokens rather than text and comes back as text
//! through [`super::xls_formula`], beside the result the file cached for it.
//! Shared formulas (`SHRFMLA`) are expanded onto each cell, the way the xlsx
//! reader expands them. A formula that does not decompile - a data table, an
//! unknown token - keeps only its cached result.
//!
//! Excel 5 and 95 wrote the same records in a narrower shape - one byte per
//! character in the workbook's code page rather than UTF-16, one byte per
//! column, the relative flags of a reference on its row, and sixteen-byte `XF`
//! records - and named the sheet of a 3D reference inside the token instead of
//! in an `EXTERNSHEET` entry. All of that is read; [`Biff`] is what tells the
//! two apart, from the version in the first `BOF`.
//!
//! What is not read, and why:
//!
//! - **BIFF4 and older, and encrypted files.** Both are refused rather than
//!   half-read.
//! - **A code page this crate holds no table for.** BIFF5 names its page in a
//!   `CODEPAGE` record and [`crate::shared::codepage`] knows the ones such
//!   files carry - Windows Latin and Cyrillic, Mac Roman, DOS 866 - and reads
//!   anything else as 1252. A workbook with no record at all, or one that
//!   lies, is what [`read_xls_in`] is for.

use super::xls_formula::{self, Base, Book, BookKind, Context};
use crate::error::{Error, Result};
use crate::model::DefinedName;
use crate::model::{CellValue, ColumnRun, Spreadsheet, Worksheet};
use crate::shared::codepage;
use crate::shared::date::Epoch;
use crate::shared::palette;
use crate::style::{
    Alignment, Border, BorderStyle, Borders, Color, DiagonalDirection, Fill, Font, HorizontalAlign,
    NumberFormat, Pattern, Protection, ProtectionState, Script, Style, StyleId, StyleTable,
    Underline, VerticalAlign,
};
use crate::{CellRef, Col, Range, Row};
use std::collections::HashMap;

/// Record numbers, in the order the reader meets them.
mod record {
    pub const BOF: u16 = 0x0809;
    pub const EOF: u16 = 0x000A;
    pub const CONTINUE: u16 = 0x003C;
    pub const BOUNDSHEET: u16 = 0x0085;
    pub const SST: u16 = 0x00FC;
    pub const FORMAT: u16 = 0x041E;
    pub const XF: u16 = 0x00E0;
    pub const FONT: u16 = 0x0031;
    pub const PALETTE: u16 = 0x0092;
    pub const DATEMODE: u16 = 0x0022;
    pub const FILEPASS: u16 = 0x002F;
    pub const DIMENSION: u16 = 0x0200;
    pub const ROW: u16 = 0x0208;
    pub const COLINFO: u16 = 0x007D;
    /// Sheet options; the outline says where group summaries sit.
    pub const WSBOOL: u16 = 0x0081;
    pub const MERGEDCELLS: u16 = 0x00E5;
    pub const BLANK: u16 = 0x0201;
    pub const MULBLANK: u16 = 0x00BE;
    pub const NUMBER: u16 = 0x0203;
    pub const RK: u16 = 0x027E;
    pub const MULRK: u16 = 0x00BD;
    pub const LABEL: u16 = 0x0204;
    /// A `LABEL` with formatting runs after it, which BIFF5 writes instead.
    pub const RSTRING: u16 = 0x00D6;
    pub const CODEPAGE: u16 = 0x0042;
    pub const LABELSST: u16 = 0x00FD;
    pub const BOOLERR: u16 = 0x0205;
    pub const FORMULA: u16 = 0x0006;
    pub const STRING: u16 = 0x0207;
    pub const SHRFMLA: u16 = 0x04BC;
    pub const ARRAY: u16 = 0x0221;
    pub const TABLE: u16 = 0x0236;
    pub const NAME: u16 = 0x0018;
    pub const SUPBOOK: u16 = 0x01AE;
    pub const EXTERNNAME: u16 = 0x0023;
    pub const EXTERNSHEET: u16 = 0x0017;
}

/// Reads a workbook from a file.
///
/// # Errors
/// [`Error::Xls`] if the file cannot be read, is not a compound file, holds no
/// workbook stream, or is encrypted.
pub fn read_xls(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    let bytes = std::fs::read(path).map_err(|e| Error::Xls(e.to_string()))?;
    read_xls_from(&bytes)
}

/// The same, reading BIFF5 text in the code page you name.
///
/// A BIFF5 workbook says its page in a `CODEPAGE` record and this is not
/// needed; some of them do not, and then the reader falls back to 1252, which
/// turns Cyrillic into mojibake. Name [`codepage::WINDOWS_1251`] and it comes
/// back as Cyrillic. The page named here outranks the record, for a file whose
/// record lies. BIFF8 text is UTF-16 and is not affected.
///
/// # Errors
/// As [`read_xls`].
pub fn read_xls_in(path: impl AsRef<std::path::Path>, page: u16) -> Result<Spreadsheet> {
    let bytes = std::fs::read(path).map_err(|e| Error::Xls(e.to_string()))?;
    read_xls_from_in(&bytes, page)
}

/// The same, from bytes already in memory.
///
/// # Errors
/// As [`read_xls`].
pub fn read_xls_from(bytes: &[u8]) -> Result<Spreadsheet> {
    let ole = super::ole::Ole::new(bytes).map_err(Error::Xls)?;
    let stream = workbook_stream(&ole)?;
    Reader::new(&stream).read()
}

/// The same as [`read_xls_from`], in the code page you name. See
/// [`read_xls_in`].
///
/// # Errors
/// As [`read_xls`].
pub fn read_xls_from_in(bytes: &[u8], page: u16) -> Result<Spreadsheet> {
    let ole = super::ole::Ole::new(bytes).map_err(Error::Xls)?;
    let stream = workbook_stream(&ole)?;
    let mut reader = Reader::new(&stream);
    reader.codepage = page;
    reader.forced_codepage = Some(page);
    reader.read()
}

/// The stream a workbook lives in, whatever this writer called it.
fn workbook_stream(ole: &super::ole::Ole<'_>) -> Result<Vec<u8>> {
    // Excel 97 and later call it `Workbook`; Excel 5 and 95 call it `Book`.
    ["Workbook", "Book"]
        .into_iter()
        .find_map(|name| ole.stream(name))
        .ok_or_else(|| {
            Error::Xls(format!(
                "no workbook stream; the file holds {}",
                ole.names().collect::<Vec<_>>().join(", ")
            ))
        })
}

/// Which BIFF the workbook is written in.
///
/// BIFF8 (Excel 97 and later) writes strings as UTF-16 with a width byte and
/// gives a column two bytes; BIFF5 (Excel 5 and 95) writes one byte per
/// character in the workbook's code page and gives a column one byte. The
/// records are otherwise the same ones in the same order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Biff {
    V5,
    V8,
}

/// One record: its number and its data.
struct Record<'a> {
    id: u16,
    data: &'a [u8],
    /// Where the next record starts.
    next: usize,
}

/// Reads the record at an offset, `None` past the end of the stream.
fn record_at(stream: &[u8], at: usize) -> Option<Record<'_>> {
    let id = u16::from_le_bytes(stream.get(at..at + 2)?.try_into().ok()?);
    let length = usize::from(u16::from_le_bytes(
        stream.get(at + 2..at + 4)?.try_into().ok()?,
    ));
    let start = at + 4;
    Some(Record {
        id,
        data: stream.get(start..start + length)?,
        next: start + length,
    })
}

/// A little-endian number out of a record, or zero past its end. A record
/// shorter than its layout says is a broken file, not a reason to stop reading
/// the rest of the sheet.
fn u16_at(data: &[u8], at: usize) -> u16 {
    data.get(at..at + 2)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u16::from_le_bytes)
}

/// The same for four bytes.
fn u32_at(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

/// The same for a double.
fn f64_at(data: &[u8], at: usize) -> f64 {
    data.get(at..at + 8)
        .and_then(|b| b.try_into().ok())
        .map_or(0.0, f64::from_le_bytes)
}

/// The state the two passes share.
struct Reader<'a> {
    stream: &'a [u8],
    /// Which BIFF the stream is, read from its first `BOF`.
    biff: Biff,
    /// The code page BIFF5 text is written in: what the caller asked for, or
    /// what the `CODEPAGE` record says, or 1252.
    codepage: u16,
    /// A code page the caller named, which outranks the record.
    forced_codepage: Option<u16>,
    book: Spreadsheet,
    styles: StyleTable,
    /// The shared string table, indexed by `LABELSST`.
    strings: Vec<String>,
    /// Format code by its index, for the codes the file spells out.
    formats: HashMap<u16, String>,
    /// What each `XF` record says, in record order.
    cell_formats: Vec<Xf>,
    /// The `FONT` records, in record order; colours are resolved once the
    /// palette is known, which may be after them.
    fonts: Vec<FontRecord>,
    /// The palette colour indices resolve through.
    palette: [u32; 56],
    /// The style each `XF` became, filled as cells ask for them.
    style_ids: HashMap<u16, StyleId>,
    /// Name and stream position of each sheet, from `BOUNDSHEET`.
    sheets: Vec<(String, usize)>,
    /// What formulas refer to: sheets, other books, names.
    context: Context,
    /// `NAME` records, kept whole until every name is known, since one name's
    /// formula may use another.
    name_records: Vec<Vec<u8>>,
    /// The formulas of the sheet being read that other cells point at with an
    /// `Exp` token, by their first cell: tokens, extra data, and whether the
    /// tokens are relative to the cell reading them.
    shared: HashMap<(u16, u16), Group>,
    /// The area of the last `ARRAY` record, until the sheet takes it.
    array: Option<Range>,
}

/// A shared or array formula: its tokens, their extra data, and whether the
/// tokens are relative to the cell that reads them.
type Group = (Vec<u8>, Vec<u8>, bool);

impl<'a> Reader<'a> {
    fn new(stream: &'a [u8]) -> Self {
        Self {
            stream,
            biff: Biff::V8,
            codepage: codepage::WINDOWS_1252,
            forced_codepage: None,
            book: Spreadsheet::empty(),
            styles: StyleTable::default(),
            strings: Vec::new(),
            formats: HashMap::new(),
            cell_formats: Vec::new(),
            fonts: Vec::new(),
            palette: palette::DEFAULT,
            style_ids: HashMap::new(),
            sheets: Vec::new(),
            context: Context::default(),
            name_records: Vec::new(),
            shared: HashMap::new(),
            array: None,
        }
    }

    /// Reads the globals, then each sheet.
    fn read(mut self) -> Result<Spreadsheet> {
        self.globals()?;
        self.context.sheets = self.sheets.iter().map(|(name, _)| name.clone()).collect();
        self.defined_names();
        let sheets = std::mem::take(&mut self.sheets);
        for (name, at) in sheets {
            self.shared.clear();
            let sheet = self.sheet(&name, at)?;
            self.book.add_sheet(sheet)?;
        }
        if self.book.sheets().is_empty() {
            return Err(Error::Xls("the workbook has no sheets".to_owned()));
        }
        self.book.styles = self.styles;
        Ok(self.book)
    }

    /// The globals substream: strings, formats, the sheet list.
    fn globals(&mut self) -> Result<()> {
        let mut at = 0;
        let Some(bof) = record_at(self.stream, at) else {
            return Err(Error::Xls("the workbook stream is empty".to_owned()));
        };
        if bof.id != record::BOF {
            return Err(Error::Xls(
                "the workbook stream does not start with a BOF".to_owned(),
            ));
        }
        let version = u16_at(bof.data, 0);
        self.biff = match version {
            0x0600 => Biff::V8,
            0x0500 => Biff::V5,
            _ => {
                return Err(Error::Xls(format!(
                    "BIFF version {version:#06x} is not supported; only BIFF5 (Excel 5 and 95) \
                     and BIFF8 (Excel 97 and later) are"
                )));
            }
        };
        if self.biff == Biff::V5 {
            self.context.dialect = xls_formula::Dialect::Biff5;
            // BIFF5 has no `SUPBOOK`: every `EXTERNSHEET` names a sheet
            // directly, so they all belong to one implied internal book.
            self.context.books.push(Book {
                kind: BookKind::Internal,
                names: Vec::new(),
            });
        }
        at = bof.next;

        while let Some(record) = record_at(self.stream, at) {
            at = record.next;
            match record.id {
                record::EOF => break,
                record::FILEPASS => {
                    return Err(Error::Xls("the workbook is encrypted".to_owned()));
                }
                // BIFF8 says 1200 here and means UTF-16, which the reader
                // works out from the strings themselves; only BIFF5 needs it.
                record::CODEPAGE => {
                    if self.forced_codepage.is_none() && self.biff == Biff::V5 {
                        let page = u16_at(record.data, 0);
                        if page != 0 && page != codepage::UTF16 {
                            self.codepage = page;
                        }
                    }
                }
                record::DATEMODE => {
                    if u16_at(record.data, 0) == 1 {
                        self.book.epoch = Epoch::Mac1904;
                    }
                }
                record::BOUNDSHEET => {
                    let position = u32_at(record.data, 0) as usize;
                    let name = short_string(record.data, 6, self.biff, self.codepage);
                    self.sheets.push((name, position));
                }
                record::FORMAT => {
                    let index = u16_at(record.data, 0);
                    let code = match self.biff {
                        Biff::V5 => short_string(record.data, 2, Biff::V5, self.codepage),
                        Biff::V8 => unicode_string(record.data, 2, Biff::V8, self.codepage).0,
                    };
                    self.formats.insert(index, code);
                }
                record::XF => self.cell_formats.push(Xf::parse(record.data, self.biff)),
                record::FONT => {
                    let font = FontRecord::parse(record.data, self.biff, self.codepage);
                    self.fonts.push(font);
                }
                record::PALETTE => {
                    let count = usize::from(u16_at(record.data, 0)).min(56);
                    for slot in 0..count {
                        // Each entry is red, green, blue and an unused byte.
                        let bytes = record.data.get(2 + slot * 4..5 + slot * 4);
                        if let Some(&[r, g, b]) = bytes {
                            self.palette[slot] =
                                (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
                        }
                    }
                }
                record::NAME => self.name_records.push(record.data.to_vec()),
                record::SUPBOOK => self.context.books.push(supbook(record.data)),
                record::EXTERNNAME => {
                    if let Some(book) = self.context.books.last_mut() {
                        book.names
                            .push(short_string(record.data, 6, self.biff, self.codepage));
                    }
                }
                record::EXTERNSHEET => self.extern_sheet(record.data),
                record::SST => {
                    let (strings, next) = self.shared_strings(&record, at);
                    self.strings = strings;
                    at = next;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// One `EXTERNSHEET` record.
    ///
    /// BIFF8 writes a single record of triples pointing into the `SUPBOOK`
    /// records; BIFF5 writes one record per reference, holding the name it
    /// stands for - a byte count, a byte saying what kind of reference it is
    /// (`0x03` is a sheet of this workbook), then the name.
    fn extern_sheet(&mut self, data: &[u8]) {
        if self.biff == Biff::V5 {
            let count = usize::from(data.first().copied().unwrap_or(0));
            let name = bytes_string(data, 2, count, self.codepage);
            let sheet = self
                .sheets
                .iter()
                .position(|(known, _)| known == &name)
                .and_then(|i| u16::try_from(i).ok())
                .unwrap_or(u16::MAX);
            self.context.externs.push((0, sheet, sheet));
            return;
        }
        let count = usize::from(u16_at(data, 0));
        for i in 0..count {
            let at = 2 + i * 6;
            if data.len() < at + 6 {
                break;
            }
            self.context.externs.push((
                u16_at(data, at),
                u16_at(data, at + 2),
                u16_at(data, at + 4),
            ));
        }
    }

    /// The shared string table, which is one record plus however many
    /// `CONTINUE` records it needs. Returns where the reader should carry on.
    fn shared_strings(&self, sst: &Record<'_>, mut at: usize) -> (Vec<String>, usize) {
        let unique = u32_at(sst.data, 4) as usize;
        let mut data = sst.data[8.min(sst.data.len())..].to_vec();
        // Where each continuation begins: a string may be cut in half there,
        // and the first byte of the rest says how it goes on.
        let mut breaks = Vec::new();
        while let Some(record) = record_at(self.stream, at) {
            if record.id != record::CONTINUE {
                break;
            }
            breaks.push(data.len());
            data.extend_from_slice(record.data);
            at = record.next;
        }

        let mut strings = Vec::with_capacity(unique.min(1 << 20));
        let mut pos = 0;
        for _ in 0..unique {
            if pos >= data.len() {
                break;
            }
            let Some((text, next)) = sst_string(&data, pos, &breaks) else {
                break;
            };
            strings.push(text);
            pos = next;
        }
        (strings, at)
    }

    /// One sheet substream.
    fn sheet(&mut self, name: &str, start: usize) -> Result<Worksheet> {
        let mut sheet = Worksheet::new(name)?;
        let mut at = start;
        // The substream opens with its own BOF.
        if let Some(bof) = record_at(self.stream, at)
            && bof.id == record::BOF
        {
            at = bof.next;
        }
        while let Some(record) = record_at(self.stream, at) {
            at = record.next;
            match record.id {
                record::EOF => break,
                record::DIMENSION | record::BOF => {}
                record::WSBOOL => {
                    let flags = u16_at(record.data, 0);
                    sheet.properties.summary_below = flags & 0x40 != 0;
                    sheet.properties.summary_right = flags & 0x80 != 0;
                }
                record::MERGEDCELLS => {
                    let count = usize::from(u16_at(record.data, 0));
                    for i in 0..count {
                        let at = 2 + i * 8;
                        if let (Ok(first), Ok(last)) = (
                            cell_ref(u16_at(record.data, at + 4), u16_at(record.data, at)),
                            cell_ref(u16_at(record.data, at + 6), u16_at(record.data, at + 2)),
                        ) {
                            sheet.merges.push(Range::new(first, last));
                        }
                    }
                }
                record::COLINFO => {
                    let (first, last) = (u16_at(record.data, 0), u16_at(record.data, 2));
                    let width = f64::from(u16_at(record.data, 4)) / 256.0;
                    // Bit 0 hides, bits 8-10 are the outline level, bit 12
                    // collapses the group.
                    let options = u16_at(record.data, 8);
                    if let (Ok(first), Ok(last)) = (column(first), column(last)) {
                        let mut run = ColumnRun::new(first, last);
                        run.width = Some(width);
                        run.custom_width = true;
                        run.hidden = options & 1 != 0;
                        run.outline_level = u8::try_from((options >> 8) & 0x07).unwrap_or_default();
                        run.collapsed = options & 0x1000 != 0;
                        sheet.columns.push(run);
                    }
                }
                record::ROW => {
                    let flags = u32_at(record.data, 12);
                    let height = u16_at(record.data, 6);
                    if let Ok(row) = row(u16_at(record.data, 0)) {
                        let properties = sheet.rows.entry(row).or_default();
                        // Bit 15 of the height says the row uses the default.
                        if height & 0x8000 == 0 {
                            properties.height = Some(f64::from(height) / 20.0);
                            properties.custom_height = true;
                        }
                        properties.hidden = flags & 0x20 != 0;
                        properties.outline_level = u8::try_from(flags & 0x07).unwrap_or_default();
                        properties.collapsed = flags & 0x10 != 0;
                    }
                }
                _ => self.cell_record(&mut sheet, &record, &mut at),
            }
        }
        Ok(sheet)
    }

    /// The records that carry a value.
    fn cell_record(&mut self, sheet: &mut Worksheet, record: &Record<'_>, at: &mut usize) {
        let data = record.data;
        let (r, c) = (u16_at(data, 0), u16_at(data, 2));
        match record.id {
            record::BLANK => self.put(sheet, r, c, u16_at(data, 4), CellValue::Empty),
            record::MULBLANK => {
                // The record holds one `XF` per column, then the last column.
                let count = data.len().saturating_sub(6) / 2;
                for i in 0..count {
                    let xf = u16_at(data, 4 + i * 2);
                    // A run that starts near the last column would wrap round
                    // to the first; the file says nothing past the edge.
                    let Some(col) = u16::try_from(i).ok().and_then(|i| c.checked_add(i)) else {
                        break;
                    };
                    self.put(sheet, r, col, xf, CellValue::Empty);
                }
            }
            record::NUMBER => self.put(
                sheet,
                r,
                c,
                u16_at(data, 4),
                CellValue::Number(f64_at(data, 6)),
            ),
            record::RK => self.put(
                sheet,
                r,
                c,
                u16_at(data, 4),
                CellValue::Number(rk(u32_at(data, 6))),
            ),
            record::MULRK => {
                let count = data.len().saturating_sub(6) / 6;
                for i in 0..count {
                    let at = 4 + i * 6;
                    let Some(col) = u16::try_from(i).ok().and_then(|i| c.checked_add(i)) else {
                        break;
                    };
                    self.put(
                        sheet,
                        r,
                        col,
                        u16_at(data, at),
                        CellValue::Number(rk(u32_at(data, at + 2))),
                    );
                }
            }
            // `RSTRING` is a `LABEL` with formatting runs after the text.
            // The runs are not modelled here, and the text is the same.
            record::LABEL | record::RSTRING => {
                let (text, _) = unicode_string(data, 6, self.biff, self.codepage);
                self.put(sheet, r, c, u16_at(data, 4), CellValue::text(text));
            }
            record::LABELSST => {
                let index = u32_at(data, 6) as usize;
                let text = self.strings.get(index).cloned().unwrap_or_default();
                self.put(sheet, r, c, u16_at(data, 4), CellValue::text(text));
            }
            record::BOOLERR => {
                let value = data.get(6).copied().unwrap_or(0);
                let is_error = data.get(7).copied().unwrap_or(0) == 1;
                let value = if is_error {
                    CellValue::Error(xls_formula::error(value))
                } else {
                    CellValue::Bool(value != 0)
                };
                self.put(sheet, r, c, u16_at(data, 4), value);
            }
            record::FORMULA => {
                let formula = self.formula(data, r, c, at);
                if let Some(area) = self.array.take() {
                    sheet.array_formulas.push(area);
                }
                let cached = self.formula_result(data, at);
                let value = match formula {
                    Some(formula) => CellValue::Formula {
                        formula,
                        cached: Some(Box::new(cached)),
                    },
                    None => cached,
                };
                self.put(sheet, r, c, u16_at(data, 4), value);
            }
            _ => {}
        }
    }

    /// The text of a formula cell, if its tokens decompile.
    ///
    /// A cell that belongs to a shared or an array formula holds a single
    /// `Exp` token naming the group's first cell. The group's own record
    /// follows the first cell's `FORMULA`, so it is picked up here as the
    /// reader passes it.
    fn formula(&mut self, data: &[u8], row: u16, col: u16, at: &mut usize) -> Option<String> {
        if let Some(next) = record_at(self.stream, *at) {
            let group = match next.id {
                record::SHRFMLA => Some((10, true)),
                record::ARRAY => Some((14, false)),
                record::TABLE => Some((0, false)),
                _ => None,
            };
            if let Some((start, relative)) = group {
                *at = next.next;
                let first = (u16_at(next.data, 0), u16::from(next.data.get(4).copied()?));
                if next.id == record::ARRAY {
                    let corner = |r: u16, c: u8| {
                        Some(CellRef::new(column(c.into()).ok()?, self::row(r).ok()?))
                    };
                    self.array = corner(first.0, next.data.get(4).copied()?)
                        .zip(corner(u16_at(next.data, 2), next.data.get(5).copied()?))
                        .map(|(a, b)| Range::new(a, b));
                }
                if next.id != record::TABLE {
                    let length = usize::from(u16_at(next.data, start - 2));
                    let tokens = next.data.get(start..start + length)?.to_vec();
                    let extra = next.data.get(start + length..).unwrap_or(&[]).to_vec();
                    self.shared.insert(first, (tokens, extra, relative));
                }
            }
        }

        let length = usize::from(u16_at(data, 20));
        let tokens = data.get(22..22 + length)?;
        let extra = data.get(22 + length..).unwrap_or(&[]);
        let base = Base {
            row: row.into(),
            col: col.into(),
        };
        if tokens.first() == Some(&0x01) {
            let first = (u16_at(tokens, 1), u16_at(tokens, 3));
            let (tokens, extra, relative) = self.shared.get(&first)?;
            // An array formula lives on its first cell; the other cells of
            // its range show their part of the result and nothing more.
            if !relative && (row, col) != first {
                return None;
            }
            return xls_formula::decompile(tokens, extra, base, &self.context);
        }
        xls_formula::decompile(tokens, extra, base, &self.context)
    }

    /// Turns the `NAME` records into defined names, now that every name is
    /// known and one may refer to another.
    fn defined_names(&mut self) {
        // The names first, since formulas refer to them by position.
        let (biff, page) = (self.biff, self.codepage);
        self.context.names = self
            .name_records
            .iter()
            .map(|data| name_text(data, biff, page))
            .collect();
        for (data, name) in self.name_records.iter().zip(&self.context.names) {
            let flags = u16_at(data, 0);
            let length = usize::from(u16_at(data, 4));
            // A name that is a function (a macro, or a function newer than
            // the format) is not a defined name.
            if flags & 0x02 != 0 || name.starts_with("_xlfn.") || length == 0 {
                continue;
            }
            let count = usize::from(data.get(3).copied().unwrap_or(0));
            // BIFF5 writes the name as bytes straight after the header; BIFF8
            // puts a width byte first and may write it in two-byte characters.
            let start = match biff {
                Biff::V5 => 14 + count,
                Biff::V8 => {
                    let wide = data.get(14).copied().unwrap_or(0) & 1 != 0;
                    15 + count * if wide { 2 } else { 1 }
                }
            };
            let Some(tokens) = data.get(start..start + length) else {
                continue;
            };
            let extra = data.get(start + length..).unwrap_or(&[]);
            let Some(formula) =
                xls_formula::decompile(tokens, extra, Base::default(), &self.context)
            else {
                continue;
            };
            let sheet = u16_at(data, 8);
            self.book.defined_names.push(DefinedName {
                name: name.clone(),
                sheet: sheet.checked_sub(1).map(usize::from),
                formula,
                hidden: flags & 0x01 != 0,
            });
        }
    }

    /// The result a formula cell carries. A result of `#FFFF` in its top two
    /// bytes means the value is not a number but a flag, and a string result
    /// waits in the `STRING` record that follows.
    fn formula_result(&self, data: &[u8], at: &mut usize) -> CellValue {
        if u16_at(data, 12) != 0xFFFF {
            return CellValue::Number(f64_at(data, 6));
        }
        match data.get(6).copied().unwrap_or(3) {
            0 => {
                let Some(next) = record_at(self.stream, *at) else {
                    return CellValue::text("");
                };
                if next.id != record::STRING {
                    return CellValue::text("");
                }
                *at = next.next;
                let (text, _) = unicode_string(next.data, 0, self.biff, self.codepage);
                CellValue::text(text)
            }
            1 => CellValue::Bool(data.get(8).copied().unwrap_or(0) != 0),
            2 => CellValue::Error(xls_formula::error(data.get(8).copied().unwrap_or(0))),
            _ => CellValue::Empty,
        }
    }

    /// Puts a value on the sheet with the style its `XF` asks for.
    fn put(&mut self, sheet: &mut Worksheet, row: u16, col: u16, xf: u16, value: CellValue) {
        let (Ok(row), Ok(col)) = (self::row(row), column(col)) else {
            return;
        };
        let style = self.style_of(xf);
        let cell = sheet.entry(CellRef::new(col, row));
        cell.value = value;
        cell.style = style;
    }

    /// The style one `XF` record stands for, interned once per record.
    fn style_of(&mut self, xf: u16) -> StyleId {
        if let Some(&id) = self.style_ids.get(&xf) {
            return id;
        }
        let Some(record) = self.cell_formats.get(usize::from(xf)).copied() else {
            return StyleId::default();
        };
        let style = self.style(&record);
        let id = self.styles.intern(style);
        self.style_ids.insert(xf, id);
        id
    }

    /// Builds the style an `XF` record describes.
    fn style(&self, xf: &Xf) -> Style {
        let number_format = match self.formats.get(&xf.format) {
            Some(code) if code == "General" => NumberFormat::General,
            Some(code) => NumberFormat::Custom(code.clone()),
            None if xf.format == 0 => NumberFormat::General,
            None => NumberFormat::Builtin(xf.format),
        };
        // Font 4 does not exist: the numbering skips it, so every index past
        // it is one ahead of its record.
        let font_record = match xf.font {
            0..4 => self.fonts.get(usize::from(xf.font)),
            4 => None,
            _ => self.fonts.get(usize::from(xf.font) - 1),
        };
        let font = font_record.map_or_else(Font::default, |f| f.font(&self.palette));
        let color = |index| palette::resolve(&self.palette, index);
        let border = |style, index| Border {
            style: border_style(style),
            color: if style == 0 {
                Color::Auto
            } else {
                color(index)
            },
        };
        let diagonal_direction = match (xf.diagonal_down, xf.diagonal_up) {
            (false, false) => DiagonalDirection::None,
            (true, false) => DiagonalDirection::Down,
            (false, true) => DiagonalDirection::Up,
            (true, true) => DiagonalDirection::Both,
        };
        let pattern = fill_pattern(xf.pattern);
        Style {
            number_format,
            font,
            fill: Fill {
                pattern,
                foreground: if pattern == Pattern::None {
                    Color::Auto
                } else {
                    color(xf.pattern_foreground)
                },
                background: if pattern == Pattern::None {
                    Color::Auto
                } else {
                    color(xf.pattern_background)
                },
            },
            borders: Borders {
                left: border(xf.left, xf.left_color),
                right: border(xf.right, xf.right_color),
                top: border(xf.top, xf.top_color),
                bottom: border(xf.bottom, xf.bottom_color),
                diagonal: if diagonal_direction == DiagonalDirection::None {
                    Border::default()
                } else {
                    border(xf.diagonal, xf.diagonal_color)
                },
                diagonal_direction,
            },
            alignment: Alignment {
                horizontal: match xf.horizontal {
                    1 => HorizontalAlign::Left,
                    2 => HorizontalAlign::Center,
                    3 => HorizontalAlign::Right,
                    4 => HorizontalAlign::Fill,
                    5 => HorizontalAlign::Justify,
                    6 => HorizontalAlign::CenterContinuous,
                    7 => HorizontalAlign::Distributed,
                    _ => HorizontalAlign::General,
                },
                vertical: match xf.vertical {
                    0 => VerticalAlign::Top,
                    1 => VerticalAlign::Center,
                    3 => VerticalAlign::Justify,
                    4 => VerticalAlign::Distributed,
                    _ => VerticalAlign::Bottom,
                },
                wrap_text: xf.wrap,
                shrink_to_fit: xf.shrink,
                indent: xf.indent,
                // BIFF8 counts rotation exactly the way xlsx does: 0 to 90
                // up, 91 to 180 down, 255 stacked.
                text_rotation: u32::from(xf.rotation),
                reading_order: xf.reading_order,
            },
            protection: Protection {
                // Locked is the default, so only its absence is worth saying.
                locked: if xf.locked {
                    ProtectionState::Inherit
                } else {
                    ProtectionState::Off
                },
                hidden: if xf.hidden {
                    ProtectionState::On
                } else {
                    ProtectionState::Inherit
                },
            },
        }
    }
}

/// An `XF` record, unpacked.
///
/// The record is twenty bytes: the font and format indices, then protection,
/// alignment and a flags byte, then two double words and a word of bitfields
/// for the borders and the fill.
#[derive(Debug, Clone, Copy, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one bit of the record, named after it"
)]
struct Xf {
    /// Index into the fonts, with 4 skipped.
    font: u16,
    /// Index into the number formats, spelled out by `FORMAT` or built in.
    format: u16,
    locked: bool,
    hidden: bool,
    horizontal: u8,
    wrap: bool,
    vertical: u8,
    rotation: u8,
    /// Indent in character widths, the low nibble of the alignment byte.
    indent: u32,
    shrink: bool,
    reading_order: u32,
    left: u8,
    right: u8,
    top: u8,
    bottom: u8,
    diagonal: u8,
    left_color: u16,
    right_color: u16,
    top_color: u16,
    bottom_color: u16,
    diagonal_color: u16,
    diagonal_down: bool,
    diagonal_up: bool,
    pattern: u8,
    pattern_foreground: u16,
    pattern_background: u16,
}

impl Xf {
    /// Unpacks the record's data.
    ///
    /// BIFF5 packs the same record into sixteen bytes instead of twenty, and
    /// packs it differently: the font, the format, the protection flags and
    /// the alignment sit where BIFF8 puts them, and everything past that -
    /// the fill and the four borders - moves.
    fn parse(data: &[u8], biff: Biff) -> Self {
        if biff == Biff::V5 {
            return Self::parse_biff5(data);
        }
        let byte = |at: usize| data.get(at).copied().unwrap_or(0);
        let protection = u16_at(data, 4);
        let align = byte(6);
        let options = byte(8);
        let sides = u32_at(data, 10);
        let more = u32_at(data, 14);
        let colors = u16_at(data, 18);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "every field is masked to its width first"
        )]
        let bits = |value: u32, shift: u32, mask: u32| ((value >> shift) & mask) as u16;
        let nibble = |value: u32, shift: u32| ((value >> shift) & 0x0F) as u8;
        Self {
            font: u16_at(data, 0),
            format: u16_at(data, 2),
            locked: protection & 0x01 != 0,
            hidden: protection & 0x02 != 0,
            horizontal: align & 0x07,
            wrap: align & 0x08 != 0,
            vertical: (align >> 4) & 0x07,
            rotation: byte(7),
            // Byte 8 packs the indent into its low four bits, then
            // shrink-to-fit, then the reading order in the top two.
            indent: u32::from(options & 0x0F),
            shrink: options & 0x10 != 0,
            reading_order: u32::from(options >> 6),
            left: nibble(sides, 0),
            right: nibble(sides, 4),
            top: nibble(sides, 8),
            bottom: nibble(sides, 12),
            left_color: bits(sides, 16, 0x7F),
            right_color: bits(sides, 23, 0x7F),
            diagonal_down: sides & (1 << 30) != 0,
            diagonal_up: sides & (1 << 31) != 0,
            top_color: bits(more, 0, 0x7F),
            bottom_color: bits(more, 7, 0x7F),
            diagonal_color: bits(more, 14, 0x7F),
            diagonal: nibble(more, 21),
            pattern: ((more >> 26) & 0x3F) as u8,
            pattern_foreground: colors & 0x7F,
            pattern_background: (colors >> 7) & 0x7F,
        }
    }

    /// The sixteen-byte record Excel 5 and 95 write.
    ///
    /// Two words hold what BIFF8 spreads over three: the fill and the bottom
    /// border in the first, the other three borders in the second. A diagonal
    /// border has no place in it - BIFF5 has none.
    fn parse_biff5(data: &[u8]) -> Self {
        let byte = |at: usize| data.get(at).copied().unwrap_or(0);
        let protection = u16_at(data, 4);
        let align = byte(6);
        let fill = u32_at(data, 8);
        let sides = u32_at(data, 12);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "every field is masked to its width first"
        )]
        let bits = |value: u32, shift: u32, mask: u32| ((value >> shift) & mask) as u16;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "every field is masked to its width first"
        )]
        let small = |value: u32, shift: u32, mask: u32| ((value >> shift) & mask) as u8;
        Self {
            font: u16_at(data, 0),
            format: u16_at(data, 2),
            locked: protection & 0x01 != 0,
            hidden: protection & 0x02 != 0,
            horizontal: align & 0x07,
            wrap: align & 0x08 != 0,
            vertical: (align >> 4) & 0x07,
            rotation: match byte(7) & 0x03 {
                // BIFF5 has four orientations rather than an angle.
                1 => 255,
                2 => 90,
                3 => 180,
                _ => 0,
            },
            indent: 0,
            shrink: false,
            reading_order: 0,
            left: small(sides, 3, 0x07),
            right: small(sides, 6, 0x07),
            top: small(sides, 0, 0x07),
            bottom: small(fill, 22, 0x07),
            diagonal: 0,
            left_color: bits(sides, 16, 0x7F),
            right_color: bits(sides, 23, 0x7F),
            top_color: bits(sides, 9, 0x7F),
            bottom_color: bits(fill, 25, 0x7F),
            diagonal_color: 0,
            diagonal_down: false,
            diagonal_up: false,
            pattern: small(fill, 16, 0x3F),
            pattern_foreground: bits(fill, 0, 0x7F),
            pattern_background: bits(fill, 7, 0x7F),
        }
    }
}

/// A `FONT` record, kept with its colour as a palette index.
#[derive(Debug, Clone, Default)]
struct FontRecord {
    height: u16,
    italic: bool,
    strike: bool,
    color: u16,
    weight: u16,
    escapement: u16,
    underline: u8,
    family: u8,
    charset: u8,
    name: String,
}

impl FontRecord {
    fn parse(data: &[u8], biff: Biff, page: u16) -> Self {
        let flags = u16_at(data, 2);
        Self {
            height: u16_at(data, 0),
            italic: flags & 0x02 != 0,
            strike: flags & 0x08 != 0,
            color: u16_at(data, 4),
            weight: u16_at(data, 6),
            escapement: u16_at(data, 8),
            underline: data.get(10).copied().unwrap_or(0),
            family: data.get(11).copied().unwrap_or(0),
            charset: data.get(12).copied().unwrap_or(0),
            name: short_string(data, 14, biff, page),
        }
    }

    /// The font, with its colour looked up in the palette.
    fn font(&self, palette: &[u32; 56]) -> Font {
        Font {
            name: self.name.clone(),
            // Twips are twentieths of a point; the model keeps hundredths.
            size: u32::from(self.height) * 5,
            // Excel writes 700 for bold and 400 for regular; anything from
            // 600 up reads as bold the way it draws it.
            bold: self.weight >= 600,
            italic: self.italic,
            underline: match self.underline {
                0x01 => Underline::Single,
                0x02 => Underline::Double,
                0x21 => Underline::SingleAccounting,
                0x22 => Underline::DoubleAccounting,
                _ => Underline::None,
            },
            strike: self.strike,
            color: palette::resolve(palette, self.color),
            script: match self.escapement {
                1 => Script::Superscript,
                2 => Script::Subscript,
                _ => Script::Baseline,
            },
            family: (self.family != 0).then_some(u32::from(self.family)),
            charset: (self.charset != 0).then_some(u32::from(self.charset)),
            scheme: None,
        }
    }
}

/// A BIFF line style as the model names it.
fn border_style(value: u8) -> BorderStyle {
    BorderStyle::parse(match value {
        1 => "thin",
        2 => "medium",
        3 => "dashed",
        4 => "dotted",
        5 => "thick",
        6 => "double",
        7 => "hair",
        8 => "mediumDashed",
        9 => "dashDot",
        // `slantDashDot` has no xlsx name of its own in the model; the medium
        // dash-dot is the line that looks most like it.
        10 | 13 => "mediumDashDot",
        11 => "dashDotDot",
        12 => "mediumDashDotDot",
        _ => "none",
    })
}

/// A BIFF fill pattern as the model names it.
fn fill_pattern(value: u8) -> Pattern {
    Pattern::parse(match value {
        1 => "solid",
        2 => "mediumGray",
        3 => "darkGray",
        4 => "lightGray",
        5 => "darkHorizontal",
        6 => "darkVertical",
        7 => "darkDown",
        8 => "darkUp",
        9 => "darkGrid",
        10 => "darkTrellis",
        11 => "lightHorizontal",
        12 => "lightVertical",
        13 => "lightDown",
        14 => "lightUp",
        15 => "lightGrid",
        16 => "lightTrellis",
        17 => "gray125",
        18 => "gray0625",
        _ => "none",
    })
}

/// The cell a row and a column number name.
fn cell_ref(col: u16, row: u16) -> Result<CellRef> {
    Ok(CellRef::new(column(col)?, self::row(row)?))
}

/// A BIFF row number, which is zero-based.
fn row(value: u16) -> Result<Row> {
    Row::from_one_based(u64::from(value) + 1)
}

/// A BIFF column number, which is zero-based.
fn column(value: u16) -> Result<Col> {
    Col::from_one_based(u64::from(value) + 1)
}

/// The name a `NAME` record defines. A built-in name is stored as a single
/// character code and spelled with the `_xlnm.` prefix xlsx gives it.
fn name_text(data: &[u8], biff: Biff, page: u16) -> String {
    let count = usize::from(data.get(3).copied().unwrap_or(0));
    let text = match biff {
        Biff::V5 => bytes_string(data, 14, count, page),
        Biff::V8 => {
            let wide = data.get(14).copied().unwrap_or(0) & 1 != 0;
            read_chars(data, 15, count, wide).0
        }
    };
    if u16_at(data, 0) & 0x20 == 0 {
        return text;
    }
    let builtin = match text.chars().next().map_or(0xFF, u32::from) {
        0x00 => "Consolidate_Area",
        0x01 => "Auto_Open",
        0x02 => "Auto_Close",
        0x03 => "Extract",
        0x04 => "Database",
        0x05 => "Criteria",
        0x06 => "Print_Area",
        0x07 => "Print_Titles",
        0x08 => "Recorder",
        0x09 => "Data_Form",
        0x0A => "Auto_Activate",
        0x0B => "Auto_Deactivate",
        0x0C => "Sheet_Title",
        0x0D => "_FilterDatabase",
        _ => return text,
    };
    format!("_xlnm.{builtin}")
}

/// A `SUPBOOK` record: this workbook, an add-in, or another file with the
/// names of its sheets.
fn supbook(data: &[u8]) -> Book {
    let sheets = usize::from(u16_at(data, 0));
    let kind = match u16_at(data, 2) {
        0x0401 => BookKind::Internal,
        0x3A01 => BookKind::AddIn,
        _ => {
            let (path, mut at) = unicode_string(data, 2, Biff::V8, codepage::UTF16);
            let mut names = Vec::with_capacity(sheets.min(1024));
            for _ in 0..sheets {
                let (name, next) = unicode_string(data, at, Biff::V8, codepage::UTF16);
                names.push(name);
                at = next;
            }
            BookKind::External {
                path: decode_path(&path),
                sheets: names,
            }
        }
    };
    Book {
        kind,
        names: Vec::new(),
    }
}

/// The file name inside an encoded external path. The path starts with a
/// control character saying how it is rooted and separates folders with
/// more control characters; a formula shows only the file.
fn decode_path(path: &str) -> String {
    path.rsplit(|c: char| c.is_control() || c == '\\' || c == '/')
        .next()
        .unwrap_or(path)
        .to_owned()
}

/// An `RK` number: a float squeezed into four bytes, either as the top half of
/// a double or as a 30-bit integer, in either case optionally divided by 100.
fn rk(value: u32) -> f64 {
    let number = if value & 0x02 != 0 {
        // A signed 30-bit integer, sign included in the shift.
        #[expect(clippy::cast_possible_wrap)]
        f64::from((value as i32) >> 2)
    } else {
        f64::from_bits(u64::from(value & 0xFFFF_FFFC) << 32)
    };
    if value & 0x01 != 0 {
        number / 100.0
    } else {
        number
    }
}

/// A string with a one-byte character count, as `BOUNDSHEET` writes it.
///
/// BIFF8 puts a width byte between the count and the text; BIFF5 has none,
/// every character being one byte of the workbook's code page.
fn short_string(data: &[u8], at: usize, biff: Biff, page: u16) -> String {
    let count = usize::from(data.get(at).copied().unwrap_or(0));
    match biff {
        Biff::V5 => bytes_string(data, at + 1, count, page),
        Biff::V8 => {
            let wide = data.get(at + 1).copied().unwrap_or(0) & 1 != 0;
            read_chars(data, at + 2, count, wide).0
        }
    }
}

/// A string with a two-byte character count, as `LABEL` and `STRING` write it.
/// Returns the text and where it ends.
fn unicode_string(data: &[u8], at: usize, biff: Biff, page: u16) -> (String, usize) {
    let count = usize::from(u16_at(data, at));
    match biff {
        Biff::V5 => (bytes_string(data, at + 2, count, page), at + 2 + count),
        Biff::V8 => {
            let wide = data.get(at + 2).copied().unwrap_or(0) & 1 != 0;
            read_chars(data, at + 3, count, wide)
        }
    }
}

/// Text one byte per character, the way BIFF5 writes it, in `page`.
fn bytes_string(data: &[u8], at: usize, count: usize, page: u16) -> String {
    let end = at.saturating_add(count).min(data.len());
    codepage::decode(page, data.get(at..end).unwrap_or(&[]))
}

/// Characters, either one byte each in the Latin-1 half of CP1252 or two bytes
/// each in UTF-16. Returns the text and where it ends.
fn read_chars(data: &[u8], at: usize, count: usize, wide: bool) -> (String, usize) {
    let mut units = Vec::with_capacity(count.min(1 << 16));
    let mut pos = at;
    for _ in 0..count {
        if wide {
            units.push(u16_at(data, pos));
            pos += 2;
        } else {
            match data.get(pos) {
                Some(&b) => units.push(u16::from(b)),
                None => break,
            }
            pos += 1;
        }
        if pos > data.len() {
            break;
        }
    }
    let text = char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    (text, pos)
}

/// One string of the shared table, which may be cut in half by a `CONTINUE`
/// record and go on in the other width. Returns the text and where it ends.
///
/// The cut lands anywhere, the middle of a character included: the byte after
/// the break says how the rest is written, and a two-byte character whose
/// first half sat in the previous record takes its second half after that
/// byte. Reading a byte at a time rather than a character at a time is what
/// keeps that case straight.
fn sst_string(data: &[u8], at: usize, breaks: &[usize]) -> Option<(String, usize)> {
    let count = usize::from(u16_at(data, at));
    let flags = data.get(at + 2).copied()?;
    let mut pos = at + 3;
    let rich_runs = if flags & 0x08 != 0 {
        let runs = usize::from(u16_at(data, pos));
        pos += 2;
        runs
    } else {
        0
    };
    let extended = if flags & 0x04 != 0 {
        let length = u32_at(data, pos) as usize;
        pos += 4;
        length
    } else {
        0
    };

    let mut wide = flags & 0x01 != 0;
    let mut units = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        // A break right where a character starts sets the width of that
        // character; one inside it sets the width of the next.
        skip_break(data, &mut pos, breaks, &mut wide);
        let here = wide;
        let low = take_byte(data, &mut pos, breaks, &mut wide)?;
        let high = if here {
            take_byte(data, &mut pos, breaks, &mut wide)?
        } else {
            0
        };
        units.push(u16::from_le_bytes([low, high]));
    }
    // The formatting runs and the far-eastern extras are skipped: the model
    // keeps rich text on a cell, not on a shared string.
    pos = pos.saturating_add(rich_runs * 4).saturating_add(extended);
    let text = char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    Some((text, pos))
}

/// Steps over the flag byte a `CONTINUE` record opens with, taking the width
/// from it.
fn skip_break(data: &[u8], pos: &mut usize, breaks: &[usize], wide: &mut bool) {
    // The breaks are in stream order, and this is asked for every byte of
    // every string: a linear scan made a 30 MB workbook take seconds.
    if breaks.binary_search(pos).is_ok() {
        *wide = data.get(*pos).copied().unwrap_or(0) & 1 != 0;
        *pos += 1;
    }
}

/// One byte of a shared string, stepping over a flag byte first.
fn take_byte(data: &[u8], pos: &mut usize, breaks: &[usize], wide: &mut bool) -> Option<u8> {
    skip_break(data, pos, breaks, wide);
    let byte = data.get(*pos).copied()?;
    *pos += 1;
    Some(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found by fuzzing: a run of numbers starting at the last column
    /// overflowed the column counter.
    #[test]
    fn a_run_of_cells_past_the_last_column_stops_at_the_edge() {
        let mut globals = Vec::new();
        push(&mut globals, record::BOF, &[0x00, 0x06, 0x05, 0x00]);
        let boundsheet_at = globals.len() + 4;
        push(
            &mut globals,
            record::BOUNDSHEET,
            &[0, 0, 0, 0, 0, 0, 1, 0, b'S'],
        );
        push(&mut globals, record::EOF, &[]);
        let start = u32::try_from(globals.len()).unwrap();
        globals[boundsheet_at..boundsheet_at + 4].copy_from_slice(&start.to_le_bytes());
        push(&mut globals, record::BOF, &[0x00, 0x06, 0x10, 0x00]);
        // MULRK on row 0 from column 0xFFFE, three values, then the last column.
        let mut mulrk = vec![0, 0, 0xFE, 0xFF];
        for _ in 0..3 {
            mulrk.extend_from_slice(&[0, 0, 0x02, 0x01, 0, 0]);
        }
        mulrk.extend_from_slice(&[0x00, 0x01]);
        push(&mut globals, record::MULRK, &mulrk);
        let mut mulblank = vec![1, 0, 0xFE, 0xFF, 0, 0, 0, 0, 0, 0];
        mulblank.extend_from_slice(&[0x00, 0x01]);
        push(&mut globals, record::MULBLANK, &mulblank);
        push(&mut globals, record::EOF, &[]);
        assert!(Reader::new(&globals).read().is_ok());
    }

    #[test]
    fn rk_numbers_decode_all_four_ways() {
        assert!(
            (rk(0x3FF0_0000) - 1.0).abs() < f64::EPSILON,
            "a double's top half"
        );
        assert!(
            (rk(0x3FF0_0001) - 0.01).abs() < f64::EPSILON,
            "the same, hundredths"
        );
        assert!((rk(0x0000_0102) - 64.0).abs() < f64::EPSILON, "an integer");
        assert!(
            (rk(0x0000_0103) - 0.64).abs() < f64::EPSILON,
            "an integer in hundredths"
        );
        assert!(
            (rk(0xFFFF_FFFE) - -1.0).abs() < f64::EPSILON,
            "a negative integer"
        );
    }

    /// The case that a page of documentation does not mention and every real
    /// file contains: a `CONTINUE` record cuts a string in half, and the cut
    /// falls between the two bytes of one character.
    #[test]
    fn a_shared_string_survives_a_break_inside_a_character() {
        // cch 3, flags 1 (wide), then "аб" and the low byte of "в".
        let mut data = vec![3, 0, 0x01, 0x30, 0x04, 0x31, 0x04, 0x32];
        let boundary = data.len();
        // The continuation opens with its own flags byte, then the high byte.
        data.extend_from_slice(&[0x01, 0x04]);
        let (text, end) = sst_string(&data, 0, &[boundary]).expect("the string parses");
        assert_eq!(text, "абв");
        assert_eq!(end, data.len());
    }

    #[test]
    fn a_shared_string_can_change_width_at_a_break() {
        // Three wide characters, then a continuation that goes on in one byte.
        let mut data = vec![4, 0, 0x01, 0x30, 0x04, 0x31, 0x04, 0x32, 0x04];
        let boundary = data.len();
        data.extend_from_slice(&[0x00, b'x']);
        let (text, _) = sst_string(&data, 0, &[boundary]).expect("the string parses");
        assert_eq!(text, "абвx");
    }

    fn push(out: &mut Vec<u8>, id: u16, data: &[u8]) {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&u16::try_from(data.len()).unwrap().to_le_bytes());
        out.extend_from_slice(data);
    }

    fn formula(row: u8, col: u8, result: f64, tokens: &[u8]) -> Vec<u8> {
        let mut data = vec![row, 0, col, 0, 0, 0];
        data.extend_from_slice(&result.to_le_bytes());
        data.extend_from_slice(&[0x08, 0, 0, 0, 0, 0]);
        data.extend_from_slice(&u16::try_from(tokens.len()).unwrap().to_le_bytes());
        data.extend_from_slice(tokens);
        data
    }

    /// A shared formula, an array formula and a defined name, the three
    /// things a formula cell or a name reaches outside its own record for.
    #[test]
    fn shared_and_array_formulas_and_names_read_back_as_text() {
        let mut globals = Vec::new();
        push(&mut globals, record::BOF, &[0x00, 0x06, 0x05, 0x00]);
        push(&mut globals, record::SUPBOOK, &[1, 0, 0x01, 0x04]);
        push(&mut globals, record::EXTERNSHEET, &[1, 0, 0, 0, 0, 0, 0, 0]);
        // NAME "Rate" = Main!$A$1, through a Ref3d.
        let mut name = vec![0, 0, 0, 4, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        name.extend_from_slice(b"Rate");
        name.extend_from_slice(&[0x3A, 0, 0, 0, 0, 0, 0]);
        push(&mut globals, record::NAME, &name);
        let boundsheet_at = globals.len() + 4;
        push(
            &mut globals,
            record::BOUNDSHEET,
            &[0, 0, 0, 0, 0, 0, 4, 0, b'M', b'a', b'i', b'n'],
        );
        push(&mut globals, record::EOF, &[]);
        let start = u32::try_from(globals.len()).unwrap();
        globals[boundsheet_at..boundsheet_at + 4].copy_from_slice(&start.to_le_bytes());

        let mut sheet = Vec::new();
        push(&mut sheet, record::BOF, &[0x00, 0x06, 0x10, 0x00]);
        let exp = [0x01, 1, 0, 0, 0];
        push(&mut sheet, record::FORMULA, &formula(1, 0, 2.0, &exp));
        // A2:A3 share RefN(row -1, same column) * Int 2.
        let mut shared = vec![1, 0, 2, 0, 0, 0, 0, 2, 9, 0];
        shared.extend_from_slice(&[0x4C, 0xFF, 0xFF, 0x00, 0xC0, 0x1E, 2, 0, 0x05]);
        push(&mut sheet, record::SHRFMLA, &shared);
        push(&mut sheet, record::FORMULA, &formula(2, 0, 4.0, &exp));
        // B1:B2 hold one array formula, `Rate`.
        let array_exp = [0x01, 0, 0, 1, 0];
        push(&mut sheet, record::FORMULA, &formula(0, 1, 1.0, &array_exp));
        let mut array = vec![0, 0, 1, 0, 1, 1, 0, 0, 0, 0, 0, 0, 5, 0];
        array.extend_from_slice(&[0x43, 1, 0, 0, 0]);
        push(&mut sheet, record::ARRAY, &array);
        push(&mut sheet, record::FORMULA, &formula(1, 1, 1.0, &array_exp));
        push(&mut sheet, record::EOF, &[]);
        globals.extend_from_slice(&sheet);

        let book = Reader::new(&globals).read().unwrap();
        let text = |at: &str| match &book.sheets()[0].get(CellRef::parse(at).unwrap())?.value {
            CellValue::Formula { formula, .. } => Some(formula.clone()),
            _ => None,
        };
        assert_eq!(text("A2").as_deref(), Some("A1*2"));
        assert_eq!(text("A3").as_deref(), Some("A2*2"));
        assert_eq!(text("B1").as_deref(), Some("Rate"));
        assert_eq!(text("B2"), None, "the rest of an array keeps its value");
        assert_eq!(
            book.sheets()[0].array_formulas,
            vec![Range::parse("B1:B2").unwrap()]
        );
        assert_eq!(
            book.sheets()[0]
                .get(CellRef::parse("B2").unwrap())
                .map(|c| c.value.clone()),
            Some(CellValue::Number(1.0))
        );
        let rate = &book.defined_names[0];
        assert_eq!(
            (rate.name.as_str(), rate.formula.as_str()),
            ("Rate", "Main!$A$1")
        );
    }

    #[test]
    fn error_codes_are_the_ones_biff_numbers() {
        use crate::CellError;
        assert_eq!(xls_formula::error(0x07), CellError::Div0);
        assert_eq!(xls_formula::error(0x17), CellError::Ref);
        assert_eq!(xls_formula::error(0x2A), CellError::Na);
    }

    /// A BIFF5 workbook holding one byte of text, so that the code page is
    /// the whole of what the answer depends on. Built with the OLE writer,
    /// which the cut-down build does not have.
    #[cfg(feature = "write")]
    fn biff5_with(page: Option<u16>, byte: u8) -> Vec<u8> {
        let mut globals = Vec::new();
        push(&mut globals, record::BOF, &[0x00, 0x05, 0x05, 0x00]);
        if let Some(page) = page {
            push(&mut globals, record::CODEPAGE, &page.to_le_bytes());
        }
        let boundsheet_at = globals.len() + 4;
        // Position, visibility and type, then the name as bytes: no width byte.
        push(
            &mut globals,
            record::BOUNDSHEET,
            &[0, 0, 0, 0, 0, 0, 1, b'S'],
        );
        push(&mut globals, record::EOF, &[]);
        let start = u32::try_from(globals.len()).unwrap();
        globals[boundsheet_at..boundsheet_at + 4].copy_from_slice(&start.to_le_bytes());
        push(&mut globals, record::BOF, &[0x00, 0x05, 0x10, 0x00]);
        // LABEL at A1: row, column, XF, a two-byte count, then the text.
        push(&mut globals, record::LABEL, &[0, 0, 0, 0, 0, 0, 1, 0, byte]);
        push(&mut globals, record::EOF, &[]);
        crate::writer::ole::container("Workbook", &globals)
    }

    /// The same byte is a different letter in each page, and a caller who
    /// names one outranks the record - a file whose record lies, or has none.
    #[cfg(feature = "write")]
    #[test]
    fn biff5_text_is_read_in_the_workbook_code_page() {
        let at = CellRef::parse("A1").unwrap();
        let text = |bytes: &[u8], page: Option<u16>| {
            let book = match page {
                Some(page) => read_xls_from_in(bytes, page),
                None => read_xls_from(bytes),
            }
            .unwrap();
            match &book.sheets()[0].get(at).unwrap().value {
                CellValue::Text(text) => text.to_string(),
                other => panic!("A1 is {other:?}"),
            }
        };

        // 0xC0 is `À` in 1252 and `А` in 1251.
        assert_eq!(
            text(&biff5_with(Some(codepage::WINDOWS_1252), 0xC0), None),
            "À"
        );
        assert_eq!(
            text(&biff5_with(Some(codepage::WINDOWS_1251), 0xC0), None),
            "А"
        );
        // No record at all: 1252, until the caller says otherwise.
        assert_eq!(text(&biff5_with(None, 0xC0), None), "À");
        assert_eq!(
            text(&biff5_with(None, 0xC0), Some(codepage::WINDOWS_1251)),
            "А"
        );
        // And the caller wins over a record that says something else.
        assert_eq!(
            text(
                &biff5_with(Some(codepage::WINDOWS_1252), 0xC0),
                Some(codepage::WINDOWS_1251)
            ),
            "А"
        );
    }
}
