//! Reading xlsb (BIFF12).
//!
//! The same package as xlsx, with the XML parts replaced by record streams:
//! `xl/workbook.bin`, `xl/worksheets/sheetN.bin`, `xl/sharedStrings.bin` and
//! `xl/styles.bin`. A record is a number, a length and that many bytes, both
//! numbers written seven bits at a time with the high bit saying "one more
//! byte follows".
//!
//! Formulas are tokens, the same reverse Polish stream xls stores, so
//! [`super::xls_formula`] decompiles them; what differs is how wide a row and
//! a column are, which is what its [`Dialect`] selects.
//!
//! Read-only, and only what a reader is asked for most: values, formulas,
//! shared strings, number formats, column widths, merges, sheet visibility and
//! defined names. Everything else a package carries - drawings, tables, pivot
//! tables, conditional formatting, fonts and fills - is left behind.

use super::xls_formula::{self, Base, Book, BookKind, Context, Dialect};
use super::xlsx::{find_workbook_part, read_relationships, rels_path_for, resolve};
use super::zipxml;
use crate::error::{Error, Result};
use crate::model::autofilter::{
    AutoFilter, ColumnFilter, CustomFilter, FilterColumn, FilterOperator,
};
use crate::model::{
    CellValue, ColumnRun, DefinedName, Pane, PanePosition, PaneState, RowProperties, Selection,
    SheetView, SheetVisibility, Spreadsheet, Worksheet,
};
use crate::shared::date::Epoch;
use crate::style::{NumberFormat, Style, StyleId, StyleTable};
use crate::{CellRef, Col, Range, Row};
use std::collections::HashMap;
use std::io::{BufReader, Read, Seek};

/// Record numbers this reader knows. The rest go by.
mod record {
    pub const ROW_HDR: u16 = 0;
    pub const CELL_BLANK: u16 = 1;
    pub const CELL_RK: u16 = 2;
    pub const CELL_ERROR: u16 = 3;
    pub const CELL_BOOL: u16 = 4;
    pub const CELL_REAL: u16 = 5;
    pub const CELL_ST: u16 = 6;
    pub const CELL_ISST: u16 = 7;
    pub const FMLA_STRING: u16 = 8;
    pub const FMLA_NUM: u16 = 9;
    pub const FMLA_BOOL: u16 = 10;
    pub const FMLA_ERROR: u16 = 11;
    pub const SST_ITEM: u16 = 19;
    pub const FMT: u16 = 44;
    pub const FONT: u16 = 43;
    pub const FILL: u16 = 45;
    pub const BORDER: u16 = 46;
    pub const XF: u16 = 47;
    pub const COL_INFO: u16 = 60;
    pub const WS_FMT_INFO: u16 = 485;
    pub const WS_VIEW: u16 = 137;
    pub const PANE: u16 = 151;
    pub const SEL: u16 = 152;
    pub const BEGIN_AFILTER: u16 = 161;
    pub const BEGIN_FILTER_COLUMN: u16 = 163;
    pub const BEGIN_CUSTOM_FILTERS: u16 = 172;
    pub const CUSTOM_FILTER: u16 = 174;
    pub const MERGE_CELL: u16 = 176;
    pub const NAME: u16 = 39;
    pub const BUNDLE_SH: u16 = 156;
    pub const WB_PROP: u16 = 153;
    pub const SUP_SELF: u16 = 357;
    pub const SUP_BOOK_SRC: u16 = 355;
    pub const SUP_SAME: u16 = 356;
    pub const EXTERN_SHEET: u16 = 362;
    pub const ARR_FMLA: u16 = 426;
    pub const SHR_FMLA: u16 = 427;
    pub const BEGIN_CELL_XFS: u16 = 617;
    pub const END_CELL_XFS: u16 = 618;
}

/// Reads an xlsb workbook from a file.
///
/// # Errors
/// [`Error::Xlsb`] if the package cannot be opened or holds no workbook.
pub fn read_xlsb(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    let file = std::fs::File::open(path).map_err(|e| Error::Xlsb(e.to_string()))?;
    read_xlsb_from(BufReader::new(file))
}

/// The same, from any seekable reader.
///
/// # Errors
/// As [`read_xlsb`].
pub fn read_xlsb_from<R: Read + Seek>(source: R) -> Result<Spreadsheet> {
    read_xlsb_from_limited(source, zipxml::MAX_UNCOMPRESSED_SIZE)
}

/// The same, with the cap on how far the package may expand given explicitly.
///
/// # Errors
/// As [`read_xlsb`].
pub fn read_xlsb_from_limited<R: Read + Seek>(source: R, max_expanded: u64) -> Result<Spreadsheet> {
    let mut zip = zip::ZipArchive::new(source).map_err(|e| Error::Xlsb(e.to_string()))?;
    let expanded = zipxml::uncompressed_size(&mut zip);
    let compressed = zipxml::compressed_size(&mut zip);
    if !zipxml::expansion_allowed(expanded, compressed, max_expanded) {
        return Err(Error::Xlsb(format!(
            "package expands to {expanded} bytes from {compressed}, over the {max_expanded} limit"
        )));
    }

    let workbook_path = find_workbook_part(&mut zip).map_err(|e| Error::Xlsb(e.to_string()))?;
    let base = workbook_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let rels = read_relationships(&mut zip, &rels_path_for(&workbook_path))
        .map_err(|e| Error::Xlsb(e.to_string()))?;

    let strings = match rels.values().find(|r| r.kind.ends_with("/sharedStrings")) {
        Some(r) => shared_strings(&part(&mut zip, &resolve(base, &r.target))?),
        None => Vec::new(),
    };
    let styles = match rels.values().find(|r| r.kind.ends_with("/styles")) {
        Some(r) => styles(&part(&mut zip, &resolve(base, &r.target))?),
        None => StyleTable::default(),
    };
    let style_count = styles.len();

    let bytes = part(&mut zip, &workbook_path)?;
    let header = workbook(&bytes)?;
    let context = Context {
        dialect: Dialect::Biff12,
        sheets: header.sheets.iter().map(|s| s.name.clone()).collect(),
        externs: header.externs.clone(),
        books: header.books,
        names: header.names.iter().map(|n| n.name.clone()).collect(),
    };

    let mut book = Spreadsheet::empty();
    book.styles = styles;
    book.epoch = header.epoch;
    for entry in &header.sheets {
        let Some(rel) = rels.get(&entry.rel_id) else {
            continue;
        };
        let bytes = part(&mut zip, &resolve(base, &rel.target))?;
        let mut sheet = worksheet(&bytes, &strings, style_count, &context);
        sheet.set_title(&entry.name)?;
        sheet.visibility = entry.visibility;
        sheet.shrink_to_fit();
        book.add_sheet(sheet)?;
    }
    book.defined_names = header
        .names
        .iter()
        .filter_map(|name| name.resolve(&context))
        .collect();
    Ok(book)
}

/// One part as bytes.
fn part<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, path: &str) -> Result<Vec<u8>> {
    zipxml::read_bytes(zip, path).map_err(Error::Xlsb)
}

/// The row height a sheet falls back on, in twips: fifteen points.
const DEFAULT_ROW_HEIGHT: u16 = 300;

/// The records of a stream: each a number and its bytes.
struct Records<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Iterator for Records<'a> {
    type Item = (u16, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        // The number takes one byte, or two when the first has its high bit
        // set; the length takes up to four, seven bits each.
        let mut id = u16::from(*self.data.get(self.at)?);
        self.at += 1;
        if id & 0x80 != 0 {
            let more = u16::from(*self.data.get(self.at)?);
            self.at += 1;
            id = (id & 0x7F) | ((more & 0x7F) << 7);
        }
        let mut size = 0usize;
        for shift in 0..4 {
            let byte = *self.data.get(self.at)?;
            self.at += 1;
            size |= usize::from(byte & 0x7F) << (shift * 7);
            if byte & 0x80 == 0 {
                break;
            }
        }
        let body = self.data.get(self.at..self.at.checked_add(size)?)?;
        self.at += size;
        Some((id, body))
    }
}

const fn records(data: &[u8]) -> Records<'_> {
    Records { data, at: 0 }
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn f64_at(data: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// An `XLWideString`: a character count and that many UTF-16 code units.
/// Returns the text and where it ends.
fn wide_string(data: &[u8], at: usize) -> Option<(String, usize)> {
    let count = usize::try_from(u32_at(data, at)?).ok()?;
    // A count that cannot fit the part is a lie, not a string.
    let end = at.checked_add(4)?.checked_add(count.checked_mul(2)?)?;
    let bytes = data.get(at + 4..end)?;
    let text = char::decode_utf16(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair)),
    )
    .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
    .collect();
    Some((text, end))
}

/// An `RkNumber`: a double with its low bits thrown away, or an integer, and
/// either of them possibly divided by a hundred.
fn rk_number(bits: u32) -> f64 {
    let value = if bits & 0x02 == 0 {
        f64::from_bits(u64::from(bits & 0xFFFF_FFFC) << 32)
    } else {
        #[expect(clippy::cast_possible_wrap, reason = "the field is a signed integer")]
        let integer = (bits as i32) >> 2;
        f64::from(integer)
    };
    if bits & 0x01 == 0 {
        value
    } else {
        value / 100.0
    }
}

/// A cell address from a record's row and column, or `None` when either is out
/// of the sheet.
fn cell_ref(row: u32, col: u32) -> Option<CellRef> {
    Some(CellRef::new(
        Col::from_one_based(u64::from(col) + 1).ok()?,
        Row::from_one_based(u64::from(row) + 1).ok()?,
    ))
}

/// The strings of `sharedStrings.bin`, in the order cells index them.
fn shared_strings(data: &[u8]) -> Vec<CellValue> {
    records(data)
        .filter(|(id, _)| *id == record::SST_ITEM)
        // The flag byte says whether formatting runs and phonetic text follow
        // the string; both are past the text, and neither is read.
        .filter_map(|(_, body)| wide_string(body, 1).map(|(text, _)| CellValue::text(text)))
        .collect()
}

/// The style table of `styles.bin`.
///
/// The pieces arrive before the entries that point at them - fonts, fills and
/// borders each in their own run of records, then one `BrtXF` per cell style -
/// so they are collected and resolved at the end, exactly as the xlsx reader
/// does with `<fonts>`, `<fills>` and `<borders>`.
fn styles(data: &[u8]) -> StyleTable {
    let mut custom: HashMap<u16, String> = HashMap::new();
    let (mut fonts, mut fills, mut borders, mut xfs) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut in_cell_xfs = false;
    for (id, body) in records(data) {
        match id {
            record::FMT => {
                if let (Some(index), Some((code, _))) = (u16_at(body, 0), wide_string(body, 2)) {
                    custom.insert(index, code);
                }
            }
            record::FONT => fonts.push(font(body).unwrap_or_default()),
            record::FILL => fills.push(fill(body).unwrap_or_default()),
            record::BORDER => borders.push(cell_borders(body).unwrap_or_default()),
            record::BEGIN_CELL_XFS => in_cell_xfs = true,
            record::END_CELL_XFS => in_cell_xfs = false,
            // The same record spells a named style's entry and a cell's; only
            // the cells' table is indexed by a cell.
            record::XF if in_cell_xfs => xfs.push(body.to_vec()),
            _ => {}
        }
    }

    StyleTable::from_styles(
        xfs.iter()
            .map(|xf| {
                let piece = |at: usize, count: usize| {
                    u16_at(xf, at).map(usize::from).filter(|i| *i < count)
                };
                let format = u16_at(xf, 2).unwrap_or(0);
                Style {
                    number_format: match (format, custom.get(&format)) {
                        (_, Some(code)) => NumberFormat::Custom(code.clone()),
                        (0, None) => NumberFormat::General,
                        (id, None) => NumberFormat::Builtin(id),
                    },
                    font: piece(4, fonts.len())
                        .map(|i| fonts[i].clone())
                        .unwrap_or_default(),
                    fill: piece(6, fills.len())
                        .map(|i| fills[i].clone())
                        .unwrap_or_default(),
                    borders: piece(8, borders.len())
                        .map(|i| borders[i].clone())
                        .unwrap_or_default(),
                    alignment: alignment(xf),
                    protection: protection(xf),
                }
            })
            .collect(),
    )
}

/// A `BrtColor`: what kind of colour it is, an index into the palette or the
/// theme, a tint, and the colour itself.
fn color(data: &[u8], at: usize) -> Option<crate::style::Color> {
    use crate::style::Color;
    let kind = (data.get(at)? >> 1) & 0x7F;
    let index = u32::from(*data.get(at + 1)?);
    #[expect(clippy::cast_possible_wrap, reason = "the tint is signed")]
    let tint = i32::from(u16_at(data, at + 2)? as i16);
    let (red, green, blue, alpha) = (
        u32::from(*data.get(at + 4)?),
        u32::from(*data.get(at + 5)?),
        u32::from(*data.get(at + 6)?),
        u32::from(*data.get(at + 7)?),
    );
    Some(match kind {
        // Past the 56-entry palette are the system colours - window text,
        // window background, "automatic" - which this crate calls `Auto`, as
        // it does for xls, and which xlsx says by writing no colour at all.
        1 if index >= u32::from(crate::shared::palette::END) => Color::Auto,
        1 => Color::Indexed(index),
        2 => Color::Argb((alpha << 24) | (red << 16) | (green << 8) | blue),
        // The tint runs from -32767 to 32767 where xlsx writes -1.0 to 1.0,
        // and this crate keeps it in millionths.
        3 => Color::Theme {
            id: index,
            tint: (tint * 1_000_000) / 32767,
        },
        _ => Color::Auto,
    })
}

/// A `BrtFont`.
fn font(data: &[u8]) -> Option<crate::style::Font> {
    use crate::style::{Font, FontScheme, Script, Underline};
    let flags = u16_at(data, 2)?;
    let (name, _) = wide_string(data, 21)?;
    Some(Font {
        name,
        // The height is in twips, a twentieth of a point; the model keeps
        // hundredths of one.
        size: u32::from(u16_at(data, 0)?) * 5,
        bold: u16_at(data, 4)? >= 700,
        italic: flags & 0x02 != 0,
        strike: flags & 0x08 != 0,
        script: match u16_at(data, 6)? {
            1 => Script::Superscript,
            2 => Script::Subscript,
            _ => Script::Baseline,
        },
        underline: match data.get(8)? {
            1 => Underline::Single,
            2 => Underline::Double,
            0x21 => Underline::SingleAccounting,
            0x22 => Underline::DoubleAccounting,
            _ => Underline::None,
        },
        // Zero is "not said" for both of these: xlsx leaves the element out
        // rather than writing the zero.
        family: Some(u32::from(*data.get(9)?)).filter(|v| *v != 0),
        charset: Some(u32::from(*data.get(10)?)).filter(|v| *v != 0),
        color: color(data, 12)?,
        scheme: match data.get(20)? {
            1 => Some(FontScheme::Major),
            2 => Some(FontScheme::Minor),
            _ => None,
        },
    })
}

/// A `BrtFill`. Gradient fills are not modelled and read as no fill, which is
/// what the xlsx reader makes of them too.
fn fill(data: &[u8]) -> Option<crate::style::Fill> {
    use crate::style::{Fill, Pattern};
    /// The patterns in the order the format numbers them.
    const PATTERNS: [&str; 19] = [
        "none",
        "solid",
        "mediumGray",
        "darkGray",
        "lightGray",
        "darkHorizontal",
        "darkVertical",
        "darkDown",
        "darkUp",
        "darkGrid",
        "darkTrellis",
        "lightHorizontal",
        "lightVertical",
        "lightDown",
        "lightUp",
        "lightGrid",
        "lightTrellis",
        "gray125",
        "gray0625",
    ];
    let index = usize::try_from(u32_at(data, 0)?).ok()?;
    Some(Fill {
        pattern: PATTERNS
            .get(index)
            .map_or(Pattern::None, |p| Pattern::parse(p)),
        foreground: color(data, 4)?,
        background: color(data, 12)?,
    })
}

/// A `BrtBorder`: two bits saying which way a diagonal runs, then five sides.
fn cell_borders(data: &[u8]) -> Option<crate::style::Borders> {
    use crate::style::{Border, BorderStyle, Borders, DiagonalDirection};
    /// The line styles in the order the format numbers them.
    const STYLES: [&str; 14] = [
        "none",
        "thin",
        "medium",
        "dashed",
        "dotted",
        "thick",
        "double",
        "hair",
        "mediumDashed",
        "dashDot",
        "mediumDashDot",
        "dashDotDot",
        "mediumDashDotDot",
        "slantDashDot",
    ];
    let flags = *data.first()?;
    // Each side is a line style, a byte of nothing, and a colour.
    let side = |index: usize| -> Option<Border> {
        let at = 1 + index * 10;
        let style = usize::from(*data.get(at)?);
        Some(Border {
            style: STYLES
                .get(style)
                .map_or(BorderStyle::None, |s| BorderStyle::parse(s)),
            color: color(data, at + 2)?,
        })
    };
    let (down, up) = (flags & 0x01 != 0, flags & 0x02 != 0);
    Some(Borders {
        // The sides come top, bottom, left, right, diagonal - not the order
        // xlsx writes them in.
        top: side(0)?,
        bottom: side(1)?,
        left: side(2)?,
        right: side(3)?,
        diagonal: side(4)?,
        diagonal_direction: match (down, up) {
            (true, true) => DiagonalDirection::Both,
            (true, false) => DiagonalDirection::Down,
            (false, true) => DiagonalDirection::Up,
            (false, false) => DiagonalDirection::None,
        },
    })
}

/// The alignment bits of a `BrtXF`.
fn alignment(xf: &[u8]) -> crate::style::Alignment {
    use crate::style::{Alignment, HorizontalAlign, VerticalAlign};
    let flags = u16_at(xf, 12).unwrap_or(0);
    Alignment {
        horizontal: match flags & 0x07 {
            1 => HorizontalAlign::Left,
            2 => HorizontalAlign::Center,
            3 => HorizontalAlign::Right,
            4 => HorizontalAlign::Fill,
            5 => HorizontalAlign::Justify,
            6 => HorizontalAlign::CenterContinuous,
            7 => HorizontalAlign::Distributed,
            _ => HorizontalAlign::General,
        },
        vertical: match (flags >> 3) & 0x07 {
            0 => VerticalAlign::Top,
            1 => VerticalAlign::Center,
            3 => VerticalAlign::Justify,
            4 => VerticalAlign::Distributed,
            _ => VerticalAlign::Bottom,
        },
        wrap_text: flags & 0x0040 != 0,
        shrink_to_fit: flags & 0x0100 != 0,
        indent: xf.get(11).copied().map_or(0, u32::from),
        text_rotation: xf.get(10).copied().map_or(0, u32::from),
        reading_order: u32::from((flags >> 10) & 0x03),
    }
}

/// The protection bits of a `BrtXF`.
///
/// The record always holds both, while xlsx writes `<protection>` only where
/// a cell departs from the default - locked, not hidden. Saying `Inherit` for
/// the default keeps the two readers agreeing on the same workbook.
fn protection(xf: &[u8]) -> crate::style::Protection {
    use crate::style::{Protection, ProtectionState};
    let flags = u16_at(xf, 12).unwrap_or(0);
    Protection {
        locked: if flags & 0x1000 == 0 {
            ProtectionState::Off
        } else {
            ProtectionState::Inherit
        },
        hidden: if flags & 0x2000 == 0 {
            ProtectionState::Inherit
        } else {
            ProtectionState::On
        },
    }
}

/// A sheet of the workbook: its name, the relationship pointing at its part,
/// and whether it shows up as a tab.
struct SheetEntry {
    name: String,
    rel_id: String,
    visibility: SheetVisibility,
}

/// A `BrtName` record, held until every name is known: a name's own formula
/// may mention another name.
struct NameRecord {
    name: String,
    sheet: Option<usize>,
    hidden: bool,
    /// Whether this is a name a formula can stand on. Excel keeps the names of
    /// the add-in functions a workbook calls in the same table, and a formula
    /// reaches those by the same index, so they stay in the list and out of
    /// the workbook.
    defined: bool,
    tokens: Vec<u8>,
    extra: Vec<u8>,
}

impl NameRecord {
    /// The defined name this stands for, or `None` when it is not one: Excel
    /// keeps the names of add-in functions in the same table.
    fn resolve(&self, context: &Context) -> Option<DefinedName> {
        if !self.defined || self.tokens.is_empty() {
            return None;
        }
        let formula = xls_formula::decompile(&self.tokens, &self.extra, Base::default(), context)?;
        Some(DefinedName {
            name: self.name.clone(),
            sheet: self.sheet,
            formula,
            hidden: self.hidden,
        })
    }
}

/// What `workbook.bin` says.
struct WorkbookHeader {
    epoch: Epoch,
    sheets: Vec<SheetEntry>,
    names: Vec<NameRecord>,
    externs: Vec<(u16, u16, u16)>,
    books: Vec<Book>,
}

fn workbook(data: &[u8]) -> Result<WorkbookHeader> {
    let mut header = WorkbookHeader {
        epoch: Epoch::Windows1900,
        sheets: Vec::new(),
        names: Vec::new(),
        externs: Vec::new(),
        books: Vec::new(),
    };
    for (id, body) in records(data) {
        match id {
            // The first bit of the workbook's flags is the Mac epoch, which
            // moves every date in the book by 1462 days.
            record::WB_PROP => {
                if u32_at(body, 0).is_some_and(|flags| flags & 1 != 0) {
                    header.epoch = Epoch::Mac1904;
                }
            }
            record::BUNDLE_SH => {
                let Some((rel_id, at)) = wide_string(body, 8) else {
                    continue;
                };
                let Some((name, _)) = wide_string(body, at) else {
                    continue;
                };
                header.sheets.push(SheetEntry {
                    name,
                    rel_id,
                    visibility: match u32_at(body, 0) {
                        Some(1) => SheetVisibility::Hidden,
                        Some(2) => SheetVisibility::VeryHidden,
                        _ => SheetVisibility::Visible,
                    },
                });
            }
            record::NAME => {
                if let Some(name) = name_record(body) {
                    header.names.push(name);
                }
                // A record that cannot be read still takes its place: the
                // names are numbered by position.
                else {
                    header.names.push(NameRecord {
                        name: String::new(),
                        sheet: None,
                        hidden: false,
                        defined: false,
                        tokens: Vec::new(),
                        extra: Vec::new(),
                    });
                }
            }
            // Only this workbook's own sheets are resolved. An outside book
            // would need its cached parts read; until then its references
            // decompile to nothing and the cached value stands.
            record::SUP_SELF => header.books.push(Book::default()),
            record::SUP_BOOK_SRC | record::SUP_SAME => header.books.push(Book {
                kind: BookKind::AddIn,
                names: Vec::new(),
            }),
            record::EXTERN_SHEET => {
                let count = u32_at(body, 0).unwrap_or(0);
                for i in 0..count as usize {
                    let at = 4 + i * 12;
                    let (Some(book), Some(first), Some(last)) =
                        (u32_at(body, at), u32_at(body, at + 4), u32_at(body, at + 8))
                    else {
                        break;
                    };
                    // The sheet numbers are signed, and -1 marks a sheet that
                    // was deleted, which is what 0xFFFF means to BIFF8.
                    let truncate = |value: u32| u16::try_from(value).unwrap_or(0xFFFF);
                    header
                        .externs
                        .push((truncate(book), truncate(first), truncate(last)));
                }
            }
            _ => {}
        }
    }
    if header.sheets.is_empty() {
        return Err(Error::Xlsb("the workbook has no sheets".to_owned()));
    }
    Ok(header)
}

/// One `BrtName`.
fn name_record(body: &[u8]) -> Option<NameRecord> {
    let flags = u32_at(body, 0)?;
    let tab = u32_at(body, 5)?;
    let (name, at) = wide_string(body, 9)?;
    let (tokens, extra) = parsed_formula(body, at)?;
    Some(NameRecord {
        // A built-in name is stored bare and shown with its prefix.
        name: if flags & 0x20 == 0 {
            name
        } else {
            format!("_xlnm.{name}")
        },
        // The tab is all ones when the name covers the whole workbook.
        sheet: (tab != u32::MAX).then(|| usize::try_from(tab).unwrap_or(0)),
        hidden: flags & 0x01 != 0,
        // fFunc and fProc: this entry names a function, not a range.
        defined: flags & 0x0A == 0,
        tokens: tokens.to_vec(),
        extra: extra.to_vec(),
    })
}

/// A `CellParsedFormula`: the token stream and the data some tokens keep past
/// it. Returns them and where the formula ends.
fn parsed_formula(data: &[u8], at: usize) -> Option<(&[u8], &[u8])> {
    let length = usize::try_from(u32_at(data, at)?).ok()?;
    let tokens = data.get(at + 4..at + 4 + length)?;
    let extra_at = at + 4 + length;
    let extra_length = usize::try_from(u32_at(data, extra_at)?).ok()?;
    let extra = data.get(extra_at + 4..extra_at + 4 + extra_length)?;
    Some((tokens, extra))
}

/// A formula that several cells share, or one that fills an area.
struct GroupFormula<'a> {
    tokens: &'a [u8],
    extra: &'a [u8],
    /// Whether it is an array formula, which only its first cell carries.
    array: bool,
    last: (u32, u32),
}

fn worksheet(
    data: &[u8],
    strings: &[CellValue],
    style_count: usize,
    context: &Context,
) -> Worksheet {
    let groups = group_formulas(data);

    let mut sheet = Worksheet::default();
    let mut row = 0u32;
    // Every row record carries a height, where xlsx writes one only for a row
    // that departs from the sheet default; the default is what tells the two
    // apart.
    let mut default_height = DEFAULT_ROW_HEIGHT;
    for (id, body) in records(data) {
        match id {
            record::WS_FMT_INFO => {
                default_height = u16_at(body, 6).unwrap_or(DEFAULT_ROW_HEIGHT);
            }
            record::ROW_HDR => {
                row = u32_at(body, 0).unwrap_or(0);
                if let Ok(at) = Row::from_one_based(u64::from(row) + 1) {
                    let properties = row_properties(body, default_height);
                    if properties.is_meaningful() {
                        sheet.rows.insert(at, properties);
                    }
                }
            }
            record::WS_VIEW => sheet_view(&mut sheet.view, body),
            record::PANE => sheet.view.pane = pane(body),
            record::SEL => {
                // Excel writes a selection for every pane, the untouched ones
                // included; xlsx leaves out the one that sits on A1 with
                // nothing selected, because the default says as much.
                if let Some(selection) = selection(body, sheet.view.pane.is_some())
                    && !is_corner(&selection)
                {
                    sheet.view.selections.push(selection);
                }
            }
            record::BEGIN_AFILTER => {
                if let (Some(first), Some(last)) = (
                    cell_ref(u32_at(body, 0).unwrap_or(0), u32_at(body, 8).unwrap_or(0)),
                    cell_ref(u32_at(body, 4).unwrap_or(0), u32_at(body, 12).unwrap_or(0)),
                ) {
                    sheet.auto_filter = Some(AutoFilter::new(Range::new(first, last)));
                }
            }
            record::BEGIN_FILTER_COLUMN => {
                if let (Some(filter), Some(col_id)) = (sheet.auto_filter.as_mut(), u32_at(body, 0))
                {
                    filter.columns.push(FilterColumn::new(col_id));
                }
            }
            record::BEGIN_CUSTOM_FILTERS => {
                if let Some(column) = last_filter_column(&mut sheet) {
                    column.filter = Some(ColumnFilter::Custom {
                        and: u32_at(body, 0) == Some(1),
                        rules: Vec::new(),
                    });
                }
            }
            record::CUSTOM_FILTER => {
                if let Some(rule) = custom_filter(body)
                    && let Some(ColumnFilter::Custom { rules, .. }) =
                        last_filter_column(&mut sheet).and_then(|c| c.filter.as_mut())
                {
                    rules.push(rule);
                }
            }
            record::COL_INFO => column_info(&mut sheet, body),
            record::MERGE_CELL => {
                if let (Some(first), Some(last)) = (
                    cell_ref(u32_at(body, 0).unwrap_or(0), u32_at(body, 8).unwrap_or(0)),
                    cell_ref(u32_at(body, 4).unwrap_or(0), u32_at(body, 12).unwrap_or(0)),
                ) {
                    sheet.merges.push(Range::new(first, last));
                }
            }
            record::ARR_FMLA => {
                if let (Some(first), Some(last)) = (
                    cell_ref(u32_at(body, 0).unwrap_or(0), u32_at(body, 8).unwrap_or(0)),
                    cell_ref(u32_at(body, 4).unwrap_or(0), u32_at(body, 12).unwrap_or(0)),
                ) {
                    sheet.array_formulas.push(Range::new(first, last));
                }
            }
            record::CELL_BLANK..=record::FMLA_ERROR => {
                let Some(col) = u32_at(body, 0) else { continue };
                let Some(at) = cell_ref(row, col) else {
                    continue;
                };
                let Some(value) = cell_value(id, body, row, col, strings, &groups, context) else {
                    continue;
                };
                let style = u32_at(body, 4).unwrap_or(0) & 0x00FF_FFFF;
                let cell = sheet.entry(at);
                cell.value = value;
                if (style as usize) < style_count {
                    cell.style = StyleId::from_index(style);
                }
            }
            _ => {}
        }
    }
    sheet
}

/// The formulas several cells share and the ones that fill an area, by the
/// cell they hang on.
///
/// Both are records of their own, written after the first cell that uses
/// them, so the sheet is read twice: once for these, once for the cells that
/// point at them.
fn group_formulas(data: &[u8]) -> HashMap<(u32, u32), GroupFormula<'_>> {
    let mut groups = HashMap::new();
    for (id, body) in records(data) {
        if id != record::SHR_FMLA && id != record::ARR_FMLA {
            continue;
        }
        let (Some(first_row), Some(last_row), Some(first_col), Some(last_col)) = (
            u32_at(body, 0),
            u32_at(body, 4),
            u32_at(body, 8),
            u32_at(body, 12),
        ) else {
            continue;
        };
        // An array formula has a flag byte between its area and its tokens.
        let at = if id == record::ARR_FMLA { 17 } else { 16 };
        if let Some((tokens, extra)) = parsed_formula(body, at) {
            groups.insert(
                (first_row, first_col),
                GroupFormula {
                    tokens,
                    extra,
                    array: id == record::ARR_FMLA,
                    last: (last_row, last_col),
                },
            );
        }
    }
    groups
}

/// The column the auto filter is filling in, which is the last one it added.
fn last_filter_column(sheet: &mut Worksheet) -> Option<&mut FilterColumn> {
    sheet.auto_filter.as_mut()?.columns.last_mut()
}

/// A `BrtRowHdr`, past the row number: its height and what is done to it.
fn row_properties(body: &[u8], default_height: u16) -> RowProperties {
    let flags = body.get(11).copied().unwrap_or(0);
    RowProperties {
        // The height is in twips, a twentieth of a point, and is kept only
        // where it says something the sheet default does not.
        height: u16_at(body, 8)
            .filter(|twips| *twips != default_height)
            .map(|twips| f64::from(twips) / 20.0),
        custom_height: flags & 0x20 != 0,
        hidden: flags & 0x10 != 0,
        // The row's own style is not read: which bit says the row has one
        // rather than inheriting it is not settled, and a wrong one would
        // paint every row with the style of whatever `ixfe` happened to hold.
        style: None,
        outline_level: flags & 0x07,
        collapsed: flags & 0x08 != 0,
    }
}

/// A `BrtBeginWsView`: what the sheet shows and where it is scrolled to.
///
/// The zoom is not read: every sheet of the workbook this was checked against
/// sits at 100 %, so which of the record's numbers is the zoom could not be
/// told apart from the ones that are not.
fn sheet_view(view: &mut SheetView, body: &[u8]) {
    let Some(flags) = u16_at(body, 0) else {
        return;
    };
    view.show_grid_lines = flags & 0x0004 != 0;
    view.show_row_col_headers = flags & 0x0008 != 0;
    view.show_zeros = flags & 0x0010 != 0;
    view.right_to_left = flags & 0x0020 != 0;
    view.tab_selected = flags & 0x0040 != 0;
    if let (Some(row), Some(col)) = (u32_at(body, 6), u32_at(body, 10))
        && (row, col) != (0, 0)
    {
        view.top_left_cell = cell_ref(row, col);
    }
}

/// A `BrtPane`: where the sheet is split and whether the split is frozen.
fn pane(body: &[u8]) -> Option<Pane> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a split is a small count of rows or columns, written as a double"
    )]
    let split = |at: usize| f64_at(body, at).unwrap_or(0.0).max(0.0) as u32;
    let flags = *body.get(28)?;
    Some(Pane {
        x_split: split(0),
        y_split: split(8),
        top_left_cell: cell_ref(u32_at(body, 16)?, u32_at(body, 20)?),
        active_pane: pane_position(u32_at(body, 24)?),
        // The two bits are "frozen" and "frozen without a split to drag".
        state: if flags & 0x01 == 0 {
            PaneState::Split
        } else if flags & 0x02 == 0 {
            PaneState::FrozenSplit
        } else {
            PaneState::Frozen
        },
    })
}

/// A `BrtSel`: which pane, where the cursor is, and what is selected.
fn selection(body: &[u8], split: bool) -> Option<Selection> {
    let position = pane_position(u32_at(body, 0)?);
    let count = usize::try_from(u32_at(body, 16)?).ok()?;
    let mut sqref = Vec::with_capacity(count.min(1024));
    for i in 0..count {
        let at = 20 + i * 16;
        let (Some(first), Some(last)) = (
            cell_ref(u32_at(body, at)?, u32_at(body, at + 8)?),
            cell_ref(u32_at(body, at + 4)?, u32_at(body, at + 12)?),
        ) else {
            break;
        };
        sqref.push(Range::new(first, last));
    }
    Some(Selection {
        // An unsplit sheet has one selection, and which pane it belongs to is
        // then not worth saying - xlsx writes no `pane` attribute for it.
        pane: split.then_some(position),
        active_cell: cell_ref(u32_at(body, 4)?, u32_at(body, 8)?),
        sqref,
    })
}

/// Whether a selection says no more than the default does: the cursor on A1
/// and that one cell selected.
fn is_corner(selection: &Selection) -> bool {
    let Some(corner) = cell_ref(0, 0) else {
        return false;
    };
    selection.active_cell == Some(corner)
        && selection
            .sqref
            .iter()
            .all(|range| range.start == corner && range.end == corner)
}

/// Which pane a number names.
fn pane_position(value: u32) -> PanePosition {
    match value {
        1 => PanePosition::TopRight,
        2 => PanePosition::BottomLeft,
        3 => PanePosition::TopLeft,
        _ => PanePosition::BottomRight,
    }
}

/// A `BrtCustomFilter`: how to compare, and against what.
fn custom_filter(body: &[u8]) -> Option<CustomFilter> {
    let operator = match body.get(1)? {
        1 => FilterOperator::LessThan,
        3 => FilterOperator::LessThanOrEqual,
        4 => FilterOperator::GreaterThan,
        5 => FilterOperator::NotEqual,
        6 => FilterOperator::GreaterThanOrEqual,
        _ => FilterOperator::Equal,
    };
    // The value is a number or a string, by the byte before the operator.
    let value = match body.first()? {
        // As the shortest text that reads back as the same number, which is
        // how xlsx spells the attribute.
        0x04 => format!("{}", f64_at(body, 2)?),
        0x06 => wide_string(body, 2)?.0,
        _ => return None,
    };
    Some(CustomFilter { operator, value })
}

/// What one cell record holds.
fn cell_value(
    id: u16,
    body: &[u8],
    row: u32,
    col: u32,
    strings: &[CellValue],
    groups: &HashMap<(u32, u32), GroupFormula<'_>>,
    context: &Context,
) -> Option<CellValue> {
    // Every cell record opens with its column and style; the value follows.
    let at = 8;
    Some(match id {
        record::CELL_BLANK => CellValue::Empty,
        record::CELL_RK => CellValue::Number(rk_number(u32_at(body, at)?)),
        record::CELL_ERROR => CellValue::Error(xls_formula::error(*body.get(at)?)),
        record::CELL_BOOL => CellValue::Bool(*body.get(at)? != 0),
        record::CELL_REAL => CellValue::Number(f64_at(body, at)?),
        record::CELL_ST => CellValue::text(wide_string(body, at)?.0),
        record::CELL_ISST => strings
            .get(usize::try_from(u32_at(body, at)?).ok()?)?
            .clone(),
        record::FMLA_STRING | record::FMLA_NUM | record::FMLA_BOOL | record::FMLA_ERROR => {
            let (cached, after) = match id {
                record::FMLA_NUM => (CellValue::Number(f64_at(body, at)?), at + 8),
                record::FMLA_BOOL => (CellValue::Bool(*body.get(at)? != 0), at + 1),
                record::FMLA_ERROR => {
                    (CellValue::Error(xls_formula::error(*body.get(at)?)), at + 1)
                }
                _ => {
                    let (text, end) = wide_string(body, at)?;
                    (CellValue::text(text), end)
                }
            };
            // Two flag bytes sit between the cached result and the formula.
            let (tokens, extra) = parsed_formula(body, after + 2)?;
            let formula = formula_text(tokens, extra, row, col, groups, context);
            match formula {
                Some(formula) => CellValue::Formula {
                    formula,
                    cached: Some(Box::new(cached)),
                },
                // A formula whose tokens this crate cannot read leaves the
                // value it last came out as, which is true as far as it goes.
                None => cached,
            }
        }
        _ => return None,
    })
}

/// The text of a cell's formula, following `PtgExp` to the group formula it
/// points at.
fn formula_text(
    tokens: &[u8],
    extra: &[u8],
    row: u32,
    col: u32,
    groups: &HashMap<(u32, u32), GroupFormula<'_>>,
    context: &Context,
) -> Option<String> {
    let base = Base { row, col };
    if tokens.first() == Some(&0x01) {
        // `PtgExp` names the first cell of a shared or array formula: the row
        // in the token, the column in the data that follows the stream.
        let anchor = (u32_at(tokens, 1)?, u32_at(extra, 0)?);
        let group = groups.get(&anchor)?;
        // An array formula belongs to its first cell; the others show their
        // part of its result and say nothing themselves.
        if group.array && (row, col) != anchor {
            return None;
        }
        if (row, col) > group.last {
            return None;
        }
        return xls_formula::decompile(group.tokens, group.extra, base, context);
    }
    xls_formula::decompile(tokens, extra, base, context)
}

/// A `BrtColInfo`: the width and flags of a run of columns.
fn column_info(sheet: &mut Worksheet, body: &[u8]) {
    let (Some(first), Some(last), Some(width), Some(flags)) = (
        u32_at(body, 0),
        u32_at(body, 4),
        u32_at(body, 8),
        u16_at(body, 16),
    ) else {
        return;
    };
    let (Ok(first), Ok(last)) = (
        Col::from_one_based(u64::from(first) + 1),
        Col::from_one_based(u64::from(last) + 1),
    ) else {
        return;
    };
    let mut run = ColumnRun::new(first, last);
    // The width is in two hundred fifty-sixths of a character.
    run.width = Some(f64::from(width) / 256.0);
    run.custom_width = flags & 0x0002 != 0;
    run.hidden = flags & 0x0001 != 0;
    run.best_fit = flags & 0x0004 != 0;
    run.outline_level = u8::try_from((flags >> 8) & 0x07).unwrap_or(0);
    run.collapsed = flags & 0x1000 != 0;
    if run.is_meaningful() {
        sheet.columns.push(run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a record: its number and length, both seven bits at a time.
    fn record(id: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if id < 0x80 {
            out.push(u8::try_from(id).unwrap_or(0));
        } else {
            out.push(u8::try_from(id & 0x7F).unwrap_or(0) | 0x80);
            out.push(u8::try_from(id >> 7).unwrap_or(0));
        }
        let mut size = body.len();
        loop {
            let byte = u8::try_from(size & 0x7F).unwrap_or(0);
            size >>= 7;
            out.push(if size == 0 { byte } else { byte | 0x80 });
            if size == 0 {
                break;
            }
        }
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn records_carry_their_number_and_length_in_seven_bit_pieces() {
        let mut stream = record(7, &[1, 2, 3]);
        stream.extend(record(427, &[0; 200]));
        let read: Vec<_> = records(&stream)
            .map(|(id, body)| (id, body.len()))
            .collect();
        assert_eq!(read, [(7, 3), (427, 200)]);
    }

    #[test]
    fn a_stream_cut_short_stops_rather_than_inventing_a_record() {
        let mut stream = record(7, &[1, 2, 3]);
        stream.truncate(stream.len() - 1);
        assert_eq!(records(&stream).count(), 0, "the body is not all there");
        assert_eq!(records(&[0x80]).count(), 0, "the number is not all there");
    }

    #[test]
    fn rk_numbers_come_in_four_shapes() {
        // The top thirty bits of a double, the same divided by a hundred, an
        // integer, and an integer divided by a hundred.
        assert!((rk_number(0x4059_0000) - 100.0).abs() < 1e-9);
        assert!((rk_number(0x4059_0001) - 1.0).abs() < 1e-9);
        assert!((rk_number((100 << 2) | 0x02) - 100.0).abs() < 1e-9);
        assert!((rk_number((100 << 2) | 0x03) - 1.0).abs() < 1e-9);
        // Negative integers keep their sign.
        #[expect(clippy::cast_sign_loss, reason = "the field holds the bits")]
        let negative = ((-25i32 << 2) as u32) | 0x02;
        assert!((rk_number(negative) + 25.0).abs() < 1e-9);
    }

    #[test]
    fn a_string_that_claims_more_than_the_part_holds_is_refused() {
        let mut body = vec![0xFF, 0xFF, 0xFF, 0xFF];
        body.extend_from_slice(b"ab");
        assert_eq!(wide_string(&body, 0), None);
        let mut good = 2u32.to_le_bytes().to_vec();
        good.extend_from_slice(&[b'h', 0, b'i', 0]);
        assert_eq!(wide_string(&good, 0), Some(("hi".to_owned(), 8)));
    }
}
