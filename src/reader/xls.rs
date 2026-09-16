//! Reading xls (BIFF8).
//!
//! An xls is a stream of records - a two-byte number, a two-byte length, then
//! that many bytes - kept in the `Workbook` stream of an OLE compound file (see
//! [`super::ole`]). The globals come first and hold the shared strings, the
//! formats and the list of sheets; each sheet is a substream that starts where
//! its `BOUNDSHEET` record says.
//!
//! What is read: sheets and their names, every cell value type, the shared
//! string table (with its continuations), number formats, merges, column widths
//! and row heights, and the workbook's base date.
//!
//! What is not, and why:
//!
//! - **Formulas.** A formula is stored as a token stream, not as text, and
//!   turning those back into `=SUM(A1:A3)` is a decompiler of its own
//!   (some 900 lines of work). Until it exists a formula
//!   cell reads as the result the file cached for it, which is what the value
//!   was anyway.
//! - **Fonts, fills and borders.** They are bitfields inside `XF`, and worth a
//!   pass of their own once the values are trusted.
//! - **BIFF5 and older, and encrypted files.** Both are refused rather than
//!   half-read.

use crate::error::{CellError, Error, Result};
use crate::model::{CellValue, ColumnRun, Spreadsheet, Worksheet};
use crate::shared::date::Epoch;
use crate::style::{NumberFormat, Style, StyleTable};
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
    pub const DATEMODE: u16 = 0x0022;
    pub const FILEPASS: u16 = 0x002F;
    pub const DIMENSION: u16 = 0x0200;
    pub const ROW: u16 = 0x0208;
    pub const COLINFO: u16 = 0x007D;
    pub const MERGEDCELLS: u16 = 0x00E5;
    pub const BLANK: u16 = 0x0201;
    pub const MULBLANK: u16 = 0x00BE;
    pub const NUMBER: u16 = 0x0203;
    pub const RK: u16 = 0x027E;
    pub const MULRK: u16 = 0x00BD;
    pub const LABEL: u16 = 0x0204;
    pub const LABELSST: u16 = 0x00FD;
    pub const BOOLERR: u16 = 0x0205;
    pub const FORMULA: u16 = 0x0006;
    pub const STRING: u16 = 0x0207;
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

/// The same, from bytes already in memory.
///
/// # Errors
/// As [`read_xls`].
pub fn read_xls_from(bytes: &[u8]) -> Result<Spreadsheet> {
    let ole = super::ole::Ole::new(bytes).map_err(Error::Xls)?;
    // Excel 97 and later call it `Workbook`; Excel 5 and 95 call it `Book`.
    let stream = ["Workbook", "Book"]
        .into_iter()
        .find_map(|name| ole.stream(name))
        .ok_or_else(|| {
            Error::Xls(format!(
                "no workbook stream; the file holds {}",
                ole.names().collect::<Vec<_>>().join(", ")
            ))
        })?;
    Reader::new(&stream).read()
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
    book: Spreadsheet,
    styles: StyleTable,
    /// The shared string table, indexed by `LABELSST`.
    strings: Vec<String>,
    /// Format code by its index, for the codes the file spells out.
    formats: HashMap<u16, String>,
    /// What each `XF` record says, in record order.
    cell_formats: Vec<Xf>,
    /// Name and stream position of each sheet, from `BOUNDSHEET`.
    sheets: Vec<(String, usize)>,
}

impl<'a> Reader<'a> {
    fn new(stream: &'a [u8]) -> Self {
        Self {
            stream,
            book: Spreadsheet::empty(),
            styles: StyleTable::default(),
            strings: Vec::new(),
            formats: HashMap::new(),
            cell_formats: Vec::new(),
            sheets: Vec::new(),
        }
    }

    /// Reads the globals, then each sheet.
    fn read(mut self) -> Result<Spreadsheet> {
        self.globals()?;
        let sheets = std::mem::take(&mut self.sheets);
        for (name, at) in sheets {
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
        if version != 0x0600 {
            return Err(Error::Xls(format!(
                "BIFF version {version:#06x} is not supported; only BIFF8 (Excel 97 and later) is"
            )));
        }
        at = bof.next;

        while let Some(record) = record_at(self.stream, at) {
            at = record.next;
            match record.id {
                record::EOF => break,
                record::FILEPASS => {
                    return Err(Error::Xls("the workbook is encrypted".to_owned()));
                }
                record::DATEMODE => {
                    if u16_at(record.data, 0) == 1 {
                        self.book.epoch = Epoch::Mac1904;
                    }
                }
                record::BOUNDSHEET => {
                    let position = u32_at(record.data, 0) as usize;
                    let name = short_string(record.data, 6);
                    self.sheets.push((name, position));
                }
                record::FORMAT => {
                    let index = u16_at(record.data, 0);
                    let (code, _) = unicode_string(record.data, 2);
                    self.formats.insert(index, code);
                }
                record::XF => self.cell_formats.push(Xf::parse(record.data)),
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
                    let hidden = u16_at(record.data, 8) & 1 != 0;
                    if let (Ok(first), Ok(last)) = (column(first), column(last)) {
                        let mut run = ColumnRun::new(first, last);
                        run.width = Some(width);
                        run.custom_width = true;
                        run.hidden = hidden;
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
                    #[expect(clippy::cast_possible_truncation)]
                    self.put(sheet, r, c + i as u16, xf, CellValue::Empty);
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
                    #[expect(clippy::cast_possible_truncation)]
                    self.put(
                        sheet,
                        r,
                        c + i as u16,
                        u16_at(data, at),
                        CellValue::Number(rk(u32_at(data, at + 2))),
                    );
                }
            }
            record::LABEL => {
                let (text, _) = unicode_string(data, 6);
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
                    CellValue::Error(error_code(value))
                } else {
                    CellValue::Bool(value != 0)
                };
                self.put(sheet, r, c, u16_at(data, 4), value);
            }
            record::FORMULA => {
                // Only the cached result is kept; see the module note.
                let value = self.formula_result(data, at);
                self.put(sheet, r, c, u16_at(data, 4), value);
            }
            _ => {}
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
                let (text, _) = unicode_string(next.data, 0);
                CellValue::text(text)
            }
            1 => CellValue::Bool(data.get(8).copied().unwrap_or(0) != 0),
            2 => CellValue::Error(error_code(data.get(8).copied().unwrap_or(0))),
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

    /// The style one `XF` record stands for. Only its number format and indent
    /// are read; the rest of it is bitfields worth a pass of their own.
    fn style_of(&mut self, xf: u16) -> crate::style::StyleId {
        let Some(&Xf {
            format: index,
            indent,
        }) = self.cell_formats.get(xf as usize)
        else {
            return crate::style::StyleId::default();
        };
        let format = match self.formats.get(&index) {
            Some(code) if code == "General" => NumberFormat::General,
            Some(code) => NumberFormat::Custom(code.clone()),
            None if index == 0 => NumberFormat::General,
            None => NumberFormat::Builtin(index),
        };
        let mut style = Style {
            number_format: format,
            ..Style::default()
        };
        style.alignment.indent = indent;
        self.styles.intern(style)
    }
}

/// The part of an `XF` record this reader uses.
///
/// The record is twenty bytes of bitfields; fonts, fills and borders live in it
/// too and are not read yet.
#[derive(Debug, Clone, Copy, Default)]
struct Xf {
    /// Index into the number formats, spelled out by `FORMAT` or built in.
    format: u16,
    /// Indent in character widths, the low nibble of the alignment byte.
    indent: u32,
}

impl Xf {
    /// Reads what is used from the record's data.
    fn parse(data: &[u8]) -> Self {
        Self {
            format: u16_at(data, 2),
            // Byte 8 packs the indent into its low four bits, and shrink-to-fit
            // and the reading order into the rest.
            indent: u32::from(data.get(8).copied().unwrap_or(0) & 0x0F),
        }
    }
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

/// An error code as BIFF numbers them.
fn error_code(value: u8) -> CellError {
    match value {
        0x00 => CellError::Null,
        0x07 => CellError::Div0,
        0x0F => CellError::Value,
        0x17 => CellError::Ref,
        0x1D => CellError::Name,
        0x24 => CellError::Num,
        _ => CellError::Na,
    }
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
fn short_string(data: &[u8], at: usize) -> String {
    let count = usize::from(data.get(at).copied().unwrap_or(0));
    let wide = data.get(at + 1).copied().unwrap_or(0) & 1 != 0;
    read_chars(data, at + 2, count, wide).0
}

/// A string with a two-byte character count, as `FORMAT` and `STRING` write it.
/// Returns the text and where it ends.
fn unicode_string(data: &[u8], at: usize) -> (String, usize) {
    let count = usize::from(u16_at(data, at));
    let wide = data.get(at + 2).copied().unwrap_or(0) & 1 != 0;
    read_chars(data, at + 3, count, wide)
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
    if breaks.contains(pos) {
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

    #[test]
    fn error_codes_are_the_ones_biff_numbers() {
        assert_eq!(error_code(0x07), CellError::Div0);
        assert_eq!(error_code(0x17), CellError::Ref);
        assert_eq!(error_code(0x2A), CellError::Na);
    }
}
