//! Picking a reader for a file.

use crate::error::{Error, Result};
use crate::model::Spreadsheet;

/// A spreadsheet file format this crate can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// xlsx, the OOXML package.
    Xlsx,
    /// BIFF8, the old binary Excel file.
    Xls,
    /// `OpenDocument` spreadsheet.
    Ods,
    /// Comma-separated text.
    Csv,
    /// An HTML page holding a table.
    Html,
    /// SYLK.
    Slk,
    /// Gnumeric, gzipped XML.
    Gnumeric,
    /// `SpreadsheetML` 2003.
    Xml2003,
}

impl Format {
    /// The format an extension names, if it names one this crate reads.
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        Some(match extension.to_ascii_lowercase().as_str() {
            "xlsx" | "xlsm" | "xltx" | "xltm" => Self::Xlsx,
            "xls" | "xlt" => Self::Xls,
            "ods" | "ots" => Self::Ods,
            "csv" | "txt" | "tsv" => Self::Csv,
            "html" | "htm" => Self::Html,
            "slk" | "sylk" => Self::Slk,
            "gnumeric" => Self::Gnumeric,
            "xml" => Self::Xml2003,
            _ => return None,
        })
    }

    /// The format the first bytes of a file give away, if they give one away.
    ///
    /// This outranks the extension: a `.txt` that is really a zip is not CSV.
    /// Only what a file says about itself is used - a zip is xlsx or ODS by the
    /// `mimetype` part ODS stores first and uncompressed, an XML page is
    /// `SpreadsheetML` by its own processing instruction - and anything else
    /// stays `None` for the extension to answer.
    #[must_use]
    pub fn from_signature(head: &[u8]) -> Option<Self> {
        if head.starts_with(b"PK\x03\x04") {
            // ODS puts an uncompressed `mimetype` part first, so its media type
            // sits in plain sight near the front of the archive.
            let window = &head[..head.len().min(256)];
            return Some(if find(window, b"opendocument.spreadsheet").is_some() {
                Self::Ods
            } else {
                Self::Xlsx
            });
        }
        if head.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") {
            return Some(Self::Xls);
        }
        if head.starts_with(b"\x1F\x8B") {
            return Some(Self::Gnumeric);
        }
        let text = String::from_utf8_lossy(&head[..head.len().min(2048)]);
        let trimmed = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
        if trimmed.starts_with("ID;P") {
            return Some(Self::Slk);
        }
        if trimmed.starts_with('<') {
            let lower = trimmed.to_ascii_lowercase();
            return Some(
                if lower.contains("progid=\"excel.sheet\"") || lower.contains("<workbook") {
                    Self::Xml2003
                } else {
                    Self::Html
                },
            );
        }
        None
    }
}

/// Where `needle` sits in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Reads a spreadsheet, working out its format itself.
///
/// The file's own first bytes decide wherever they can; the extension answers
/// only what they cannot tell apart, and plain text with an unknown extension
/// is read as CSV, the way every spreadsheet reader does.
///
/// # Errors
/// Whatever the chosen reader returns, or [`Error::Io`] if the file cannot be
/// opened.
pub fn read(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    let path = path.as_ref();
    let format = format_of(path)?;
    match format {
        Format::Xlsx => super::xlsx::read_xlsx(path),
        Format::Xls => super::xls::read_xls(path),
        Format::Ods => super::ods::read_ods(path),
        Format::Csv => super::csv::read_csv(path),
        Format::Html => super::html::read_html(path),
        Format::Slk => super::slk::read_slk(path),
        Format::Gnumeric => super::gnumeric::read_gnumeric(path),
        Format::Xml2003 => super::xml2003::read_xml2003(path),
    }
}

/// Reads a spreadsheet from bytes already in memory, working out its format
/// the same way [`read`] does.
///
/// `name` is the file name the bytes came from, if there is one: its extension
/// answers what the signature cannot, and its stem names the sheet of a SYLK
/// file, which carries no name of its own.
///
/// # Errors
/// Whatever the chosen reader returns.
pub fn read_bytes(bytes: &[u8], name: Option<&str>) -> Result<Spreadsheet> {
    read_bytes_limited(bytes, name, super::xlsx::MAX_UNCOMPRESSED_SIZE)
}

/// The same, with the cap on how far a zipped package may expand given
/// explicitly. See [`super::xlsx::read_xlsx_from_limited`].
///
/// # Errors
/// Whatever the chosen reader returns.
pub fn read_bytes_limited(
    bytes: &[u8],
    name: Option<&str>,
    max_expanded: u64,
) -> Result<Spreadsheet> {
    read_bytes_limited_with(
        bytes,
        name,
        max_expanded,
        &crate::progress::Options::default(),
    )
}

/// The same, reporting its progress.
///
/// Only xlsx reports: the other readers take the bytes and are done, with no
/// step worth naming in between.
///
/// # Errors
/// Same as [`read_bytes`].
pub fn read_bytes_limited_with(
    bytes: &[u8],
    name: Option<&str>,
    max_expanded: u64,
    options: &crate::progress::Options<'_>,
) -> Result<Spreadsheet> {
    let path = name.map(std::path::Path::new);
    let format = Format::from_signature(bytes)
        .or_else(|| {
            path.and_then(std::path::Path::extension)
                .and_then(std::ffi::OsStr::to_str)
                .and_then(Format::from_extension)
        })
        .unwrap_or(Format::Csv);

    match format {
        Format::Xlsx => {
            super::xlsx::read_xlsx_from_with(std::io::Cursor::new(bytes), max_expanded, options)
        }
        Format::Xls => super::xls::read_xls_from(bytes),
        Format::Ods => super::ods::read_ods_from(std::io::Cursor::new(bytes)),
        Format::Csv => Ok(super::csv::read_csv_str(
            &super::csv::decode(bytes),
            &super::csv::CsvOptions::default(),
        )),
        Format::Html => Ok(super::html::read_html_str(&super::csv::decode(bytes))),
        Format::Slk => {
            // As `read_slk` does with a path: the sheet takes the file's name.
            let title = path
                .and_then(std::path::Path::file_stem)
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("Worksheet");
            super::slk::read_slk_str(&super::csv::decode(bytes), title)
        }
        Format::Gnumeric => super::gnumeric::read_gnumeric_from(bytes),
        Format::Xml2003 => super::xml2003::read_xml2003_str(&super::csv::decode(bytes)),
    }
}

/// The format [`read`] would use for a file.
///
/// # Errors
/// [`Error::Io`] if the file cannot be opened or its first bytes read.
pub fn format_of(path: impl AsRef<std::path::Path>) -> Result<Format> {
    use std::io::Read;

    let path = path.as_ref();
    let mut head = [0u8; 2048];
    let mut file = std::fs::File::open(path).map_err(|e| Error::Io(e.to_string()))?;
    let read = file.read(&mut head).map_err(|e| Error::Io(e.to_string()))?;

    Ok(Format::from_signature(&head[..read])
        .or_else(|| {
            path.extension()
                .and_then(std::ffi::OsStr::to_str)
                .and_then(Format::from_extension)
        })
        .unwrap_or(Format::Csv))
}
