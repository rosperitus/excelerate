//! Readers for spreadsheet file formats.

pub(crate) mod chart;
pub mod csv;
pub mod detect;
pub mod gnumeric;
pub mod html;
pub(crate) mod image;
pub mod ods;
pub(crate) mod ole;
pub(crate) mod properties;
pub(crate) mod shape;
pub mod slk;
pub(crate) mod vml;
pub mod xls;
mod xls_formula;
pub mod xlsb;
pub mod xlsx;
pub mod xml2003;
pub(crate) mod zipxml;

pub use csv::{CsvOptions, FormattedNumbers, read_csv, read_csv_str, read_csv_with};
pub use detect::{
    Format, format_of, read, read_bytes, read_bytes_limited, read_bytes_limited_with,
};
pub use gnumeric::{read_gnumeric, read_gnumeric_from, read_gnumeric_str};
pub use html::{read_html, read_html_str};
pub use ods::{read_ods, read_ods_from};
pub use slk::{read_slk, read_slk_str};
pub use xls::{read_xls, read_xls_from};
pub use xlsb::{read_xlsb, read_xlsb_from};
pub use xlsx::read_xlsx;
pub use xml2003::{read_xml2003, read_xml2003_str};

/// Rich text out of the binary formats: the text, and where each run starts
/// (a byte offset into it) with its font; `None` is the cell's own font.
/// Text before the first run is in the cell's font too. With no runs it is
/// plain text. A run that starts before the previous one or off a character
/// boundary is not honoured: the offsets are numbers in the file.
pub(crate) fn rich_text(
    text: String,
    runs: Vec<(usize, Option<crate::style::DiffFont>)>,
) -> crate::model::CellValue {
    use crate::model::{CellValue, TextRun};
    if runs.is_empty() {
        return CellValue::text(text);
    }
    let mut out = Vec::with_capacity(runs.len() + 1);
    let (mut start, mut font) = (0, None);
    for (at, next) in runs {
        if at < start || at > text.len() || !text.is_char_boundary(at) {
            continue;
        }
        out.push(TextRun {
            text: text[start..at].to_owned(),
            font,
        });
        (start, font) = (at, next);
    }
    out.push(TextRun {
        text: text[start..].to_owned(),
        font,
    });
    out.retain(|r| !r.text.is_empty());
    CellValue::RichText(out)
}

/// The byte offset in `text` of every UTF-16 unit, and one past the end: the
/// binary formats count run starts in UTF-16 units.
pub(crate) fn utf16_offsets(text: &str) -> Vec<usize> {
    let mut out = Vec::with_capacity(text.len() + 1);
    for (at, c) in text.char_indices() {
        out.extend(std::iter::repeat_n(at, c.len_utf16()));
    }
    out.push(text.len());
    out
}
