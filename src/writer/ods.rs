//! Writing `OpenDocument` spreadsheets.
//!
//! Five parts make a package a reader will open:
//!
//! | Part | What it says |
//! |---|---|
//! | `mimetype` | the format, **stored uncompressed and written first** |
//! | `META-INF/manifest.xml` | what else is in the package |
//! | `content.xml` | the sheets, their cells, and the automatic styles |
//! | `styles.xml` | the document styles, which here is only the required frame |
//! | `meta.xml` | who wrote it |
//!
//! The `mimetype` rule is not decoration: a reader identifies an ODF package by
//! finding that entry stored, not deflated, at a fixed offset in the archive.

use super::xmlesc::escape;
use crate::error::{Error, Result};
use crate::model::{CellValue, Hyperlink, Spreadsheet, Worksheet};
use crate::shared::odf_formula;
use crate::style::{
    Border, BorderStyle, Color, HorizontalAlign, Pattern, Style, StyleId, Underline, VerticalAlign,
};
use crate::{CellRef, Col, Range, Row};
use std::fmt::Write as _;
use std::io::{Seek, Write};

/// The mimetype an ODS package declares.
const MIMETYPE: &str = "application/vnd.oasis.opendocument.spreadsheet";

/// The namespace declarations every part of a package repeats.
const NAMESPACES: &str = concat!(
    r#" xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0""#,
    r#" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0""#,
    r#" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0""#,
    r#" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0""#,
    r#" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0""#,
    r#" xmlns:xlink="http://www.w3.org/1999/xlink""#,
    r#" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0""#,
    r#" office:version="1.3""#,
);

/// Writes a workbook as an `OpenDocument` spreadsheet.
///
/// # Errors
/// [`Error::Ods`] if the workbook holds no sheets, or the file cannot be
/// written.
pub fn write_ods(book: &Spreadsheet, path: impl AsRef<std::path::Path>) -> Result<()> {
    let file = std::fs::File::create(path).map_err(|e| Error::Ods(e.to_string()))?;
    write_ods_to(book, std::io::BufWriter::new(file))
}

/// Writes a workbook to any seekable sink.
///
/// # Errors
/// [`Error::Ods`] if the workbook holds no sheets, or the sink fails.
pub fn write_ods_to<W: Write + Seek>(book: &Spreadsheet, sink: W) -> Result<()> {
    if book.sheets().is_empty() {
        return Err(Error::Ods("cannot write a workbook with no sheets".into()));
    }
    let mut zip = zip::ZipWriter::new(sink);

    // Stored, not deflated, and first in the archive: that is how a reader
    // recognises the package before it has unzipped anything.
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file::<_, ()>("mimetype", stored)
        .map_err(|e| Error::Ods(e.to_string()))?;
    zip.write_all(MIMETYPE.as_bytes())
        .map_err(|e| Error::Ods(e.to_string()))?;

    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut part = |name: &str, body: &str| -> Result<()> {
        zip.start_file::<_, ()>(name, opts)
            .map_err(|e| Error::Ods(e.to_string()))?;
        zip.write_all(body.as_bytes())
            .map_err(|e| Error::Ods(e.to_string()))
    };

    part("META-INF/manifest.xml", MANIFEST)?;
    part("content.xml", &content(book))?;
    part("styles.xml", STYLES)?;
    part("meta.xml", META)?;

    zip.finish().map_err(|e| Error::Ods(e.to_string()))?;
    Ok(())
}

/// What the package holds.
const MANIFEST: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8"?>"#,
    r#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3">"#,
    r#"<manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>"#,
    r#"<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>"#,
    r#"<manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/>"#,
    r#"<manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>"#,
    r"</manifest:manifest>",
);

/// The document styles.
const STYLES: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8"?>"#,
    r#"<office:document-styles"#,
    r#" xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0""#,
    r#" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0""#,
    r#" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0""#,
    r#" office:version="1.3">"#,
    r"<office:styles/><office:automatic-styles/><office:master-styles/>",
    r"</office:document-styles>",
);

/// Who wrote the file.
const META: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8"?>"#,
    r#"<office:document-meta"#,
    r#" xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0""#,
    r#" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0""#,
    r#" office:version="1.3">"#,
    r"<office:meta><meta:generator>excelerate</meta:generator></office:meta>",
    r"</office:document-meta>",
);

/// Builds `content.xml`: the automatic styles, then the sheets.
fn content(book: &Spreadsheet) -> String {
    let mut out = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = write!(out, "<office:document-content{NAMESPACES}>");

    out.push_str("<office:automatic-styles>");
    for (index, style) in book.styles.all().iter().enumerate() {
        cell_style(index, style, &mut out);
    }
    // Column and row styles carry the sizes; ODS has nowhere else to put them.
    for (index, sheet) in book.sheets().iter().enumerate() {
        for run in &sheet.columns {
            if let Some(width) = run.width {
                let _ = write!(
                    out,
                    r#"<style:style style:name="co{index}_{}" style:family="table-column">"#,
                    run.first.one_based()
                );
                let _ = write!(
                    out,
                    r#"<style:table-column-properties style:column-width="{:.4}in"/></style:style>"#,
                    // A character of the default font is a seventh of an inch
                    // across at 96 dpi, which is 64 px for 9.14 characters.
                    width * 64.0 / 9.140_625 / 96.0
                );
            }
        }
        for (row, properties) in &sheet.rows {
            if let Some(height) = properties.height {
                let _ = write!(
                    out,
                    r#"<style:style style:name="ro{index}_{}" style:family="table-row">"#,
                    row.one_based()
                );
                let _ = write!(
                    out,
                    r#"<style:table-row-properties style:row-height="{:.4}in"/></style:style>"#,
                    height / 72.0
                );
            }
        }
    }
    out.push_str("</office:automatic-styles>");

    out.push_str("<office:body><office:spreadsheet>");
    for (index, sheet) in book.sheets().iter().enumerate() {
        table(book, sheet, index, &mut out);
    }
    if !book.defined_names.is_empty() {
        out.push_str("<table:named-expressions>");
        let names: Vec<String> = book.defined_names.iter().map(|n| n.name.clone()).collect();
        for name in &book.defined_names {
            let _ = write!(
                out,
                r#"<table:named-expression table:name="{}" table:expression="{}"/>"#,
                escape(&name.name),
                escape(&odf_formula::from_a1(&name.formula, &names))
            );
        }
        out.push_str("</table:named-expressions>");
    }
    out.push_str("</office:spreadsheet></office:body></office:document-content>");
    out
}

/// One cell style as an automatic style.
fn cell_style(index: usize, style: &Style, out: &mut String) {
    let _ = write!(
        out,
        r#"<style:style style:name="ce{index}" style:family="table-cell" style:parent-style-name="Default">"#
    );

    let mut cell_properties = String::new();
    if style.fill.pattern != Pattern::None
        && let Some(colour) = rgb(&style.fill.foreground)
    {
        let _ = write!(cell_properties, r#" fo:background-color="{colour}""#);
    }
    for (side, border) in [
        ("fo:border-top", &style.borders.top),
        ("fo:border-right", &style.borders.right),
        ("fo:border-bottom", &style.borders.bottom),
        ("fo:border-left", &style.borders.left),
    ] {
        if let Some(rule) = border_rule(border) {
            let _ = write!(cell_properties, r#" {side}="{rule}""#);
        }
    }
    let vertical = match style.alignment.vertical {
        VerticalAlign::Bottom => "bottom",
        VerticalAlign::Top => "top",
        VerticalAlign::Center => "middle",
        VerticalAlign::Justify | VerticalAlign::Distributed => "automatic",
    };
    let _ = write!(cell_properties, r#" style:vertical-align="{vertical}""#);
    if style.alignment.wrap_text {
        cell_properties.push_str(r#" fo:wrap-option="wrap""#);
    }
    let _ = write!(out, "<style:table-cell-properties{cell_properties}/>");

    if let Some(horizontal) = match style.alignment.horizontal {
        HorizontalAlign::General => None,
        HorizontalAlign::Left | HorizontalAlign::Fill => Some("start"),
        HorizontalAlign::Center | HorizontalAlign::CenterContinuous => Some("center"),
        HorizontalAlign::Right => Some("end"),
        HorizontalAlign::Justify | HorizontalAlign::Distributed => Some("justify"),
    } {
        let _ = write!(
            out,
            r#"<style:paragraph-properties fo:text-align="{horizontal}"/>"#
        );
    }

    let mut text = String::new();
    let _ = write!(
        text,
        r#" style:font-name="{}" fo:font-size="{}pt""#,
        escape(&style.font.name),
        f64::from(style.font.size) / 100.0
    );
    if style.font.bold {
        text.push_str(r#" fo:font-weight="bold""#);
    }
    if style.font.italic {
        text.push_str(r#" fo:font-style="italic""#);
    }
    if style.font.underline != Underline::None {
        text.push_str(r#" style:text-underline-style="solid" style:text-underline-width="auto""#);
    }
    if style.font.strike {
        text.push_str(r#" style:text-line-through-style="solid""#);
    }
    if let Some(colour) = rgb(&style.font.color) {
        let _ = write!(text, r#" fo:color="{colour}""#);
    }
    let _ = write!(out, "<style:text-properties{text}/>");

    out.push_str("</style:style>");
}

/// One border side as an `fo:border` value, or nothing for no line.
fn border_rule(border: &Border) -> Option<String> {
    let width = match border.style {
        BorderStyle::None => return None,
        BorderStyle::Named(name) => match name {
            "thick" | "double" => "0.1in",
            "medium" | "mediumDashed" | "mediumDashDot" | "mediumDashDotDot" => "0.05in",
            _ => "0.0181in",
        },
    };
    let kind = match border.style {
        BorderStyle::Named("dashed" | "mediumDashed" | "dashDot" | "mediumDashDot") => "dashed",
        BorderStyle::Named("dotted" | "dashDotDot" | "mediumDashDotDot") => "dotted",
        BorderStyle::Named("double") => "double",
        _ => "solid",
    };
    let colour = rgb(&border.color).unwrap_or_else(|| "#000000".to_owned());
    Some(format!("{width} {kind} {colour}"))
}

/// A colour as `#rrggbb`, for the colours that state their own value.
fn rgb(color: &Color) -> Option<String> {
    match color {
        Color::Argb(v) => Some(format!("#{:06X}", v & 0x00FF_FFFF)),
        Color::Auto | Color::Indexed(_) | Color::Theme { .. } => None,
    }
}

/// One sheet as a `table:table`.
fn table(book: &Spreadsheet, sheet: &Worksheet, index: usize, out: &mut String) {
    let _ = write!(
        out,
        r#"<table:table table:name="{}">"#,
        escape(sheet.title())
    );

    let used = sheet.dimension();
    let (last_col, last_row) =
        used.map_or((1, 1), |r| (r.end.col.one_based(), r.end.row.one_based()));

    for run in &sheet.columns {
        let repeat = u64::from(run.last.one_based()) - u64::from(run.first.one_based()) + 1;
        let _ = write!(out, "<table:table-column");
        if repeat > 1 {
            let _ = write!(out, r#" table:number-columns-repeated="{repeat}""#);
        }
        if run.width.is_some() {
            let _ = write!(
                out,
                r#" table:style-name="co{index}_{}""#,
                run.first.one_based()
            );
        }
        if run.hidden {
            out.push_str(r#" table:visibility="collapse""#);
        }
        out.push_str("/>");
    }

    let names: Vec<String> = book.defined_names.iter().map(|n| n.name.clone()).collect();
    for row in 1..=last_row {
        let Ok(row) = Row::from_one_based(u64::from(row)) else {
            break;
        };
        let properties = sheet.rows.get(&row);
        out.push_str("<table:table-row");
        if properties.is_some_and(|p| p.height.is_some()) {
            let _ = write!(out, r#" table:style-name="ro{index}_{}""#, row.one_based());
        }
        if properties.is_some_and(|p| p.hidden) {
            out.push_str(r#" table:visibility="collapse""#);
        }
        out.push('>');

        // Identical neighbours collapse into one element with a repeat count,
        // the way the format expects: a row otherwise spells out every column
        // of the sheet, and most of them say the same nothing.
        let mut col = 1u32;
        let mut run: Option<(String, u32)> = None;
        while col <= last_col {
            let Ok(column) = Col::from_one_based(u64::from(col)) else {
                break;
            };
            let at = CellRef::new(column, row);
            let mut element = String::new();
            if covered_by(sheet, at).is_some_and(|merge| merge.start != at) {
                element.push_str("<table:covered-table-cell/>");
            } else {
                cell(book, sheet, at, &names, &mut element);
            }
            match &mut run {
                Some((previous, count)) if *previous == element => *count += 1,
                Some((previous, count)) => {
                    push_run(out, previous, *count);
                    run = Some((element, 1));
                }
                None => run = Some((element, 1)),
            }
            col += 1;
        }
        if let Some((previous, count)) = &run {
            push_run(out, previous, *count);
        }
        out.push_str("</table:table-row>");
    }
    out.push_str("</table:table>");
}

/// Appends a cell element, repeated `count` times.
fn push_run(out: &mut String, element: &str, count: u32) {
    if count == 1 {
        out.push_str(element);
        return;
    }
    // The count goes on the element's own tag, before its other attributes.
    let at = element.find([' ', '>', '/']).unwrap_or(0);
    let _ = write!(
        out,
        "{} table:number-columns-repeated=\"{count}\"{}",
        &element[..at],
        &element[at..]
    );
}

/// One cell as a `table:table-cell`.
fn cell(book: &Spreadsheet, sheet: &Worksheet, at: CellRef, names: &[String], out: &mut String) {
    let cell = sheet.get(at);
    let value = cell.map_or(&CellValue::Empty, |c| &c.value);
    let style = cell.map_or_else(StyleId::default, |c| c.style);

    out.push_str("<table:table-cell");
    if style != StyleId::default() {
        let _ = write!(out, r#" table:style-name="ce{}""#, style.index());
    }
    if let Some(merge) = covered_by(sheet, at).filter(|m| m.start == at) {
        let _ = write!(
            out,
            r#" table:number-columns-spanned="{}" table:number-rows-spanned="{}""#,
            merge.width(),
            merge.height()
        );
    }

    // A formula carries both its text and its last known result, the way it
    // does in xlsx; a reader with no engine still has something to show.
    let (shown, formula) = match value {
        CellValue::Formula { formula, cached } => (
            cached.as_deref().unwrap_or(&CellValue::Empty).clone(),
            Some(odf_formula::from_a1(formula, names)),
        ),
        other => (other.clone(), None),
    };
    if let Some(formula) = &formula {
        let _ = write!(out, r#" table:formula="{}""#, escape(formula));
    }

    let code = book
        .styles
        .get(style)
        .map_or(crate::style::format::GENERAL, |s| s.number_format.code());
    let text = typed_value(&shown, code, book.epoch, out);
    out.push('>');
    if let Some(text) = text {
        let link = sheet.hyperlinks.iter().find(|h| covers(h, at));
        match link {
            Some(link) => {
                let href = match &link.target {
                    crate::model::LinkTarget::Outside(url) => url.clone(),
                    crate::model::LinkTarget::Inside(place) => {
                        format!("#{}", inside_target(place))
                    }
                };
                let _ = write!(
                    out,
                    r#"<text:p><text:a xlink:href="{}">{}</text:a></text:p>"#,
                    escape(&href),
                    escape(&text)
                );
            }
            // Several lines in one cell are several paragraphs.
            None => {
                for line in text.split('\n') {
                    let _ = write!(out, "<text:p>{}</text:p>", escape(line));
                }
            }
        }
    }
    out.push_str("</table:table-cell>");
}

/// Writes the typed-value attributes of a cell and returns the text to show.
///
/// `None` means the cell holds nothing and needs no `text:p` at all.
///
fn typed_value(
    value: &CellValue,
    code: &str,
    epoch: crate::shared::date::Epoch,
    out: &mut String,
) -> Option<String> {
    match value {
        // A formula's own cached value was unwrapped before this was called;
        // reaching here means it had none, which is as empty as an empty cell.
        CellValue::Empty | CellValue::Formula { .. } => None,
        CellValue::Number(n) => Some(number_value(*n, code, epoch, out)),
        CellValue::Bool(b) => {
            let _ = write!(
                out,
                r#" office:value-type="boolean" office:boolean-value="{b}""#
            );
            Some(if *b { "TRUE" } else { "FALSE" }.to_owned())
        }
        CellValue::Text(t) => {
            out.push_str(r#" office:value-type="string""#);
            Some(t.clone())
        }
        CellValue::RichText(runs) => {
            out.push_str(r#" office:value-type="string""#);
            Some(runs.iter().map(|r| r.text.as_str()).collect())
        }
        // ODS has no error type: Excel writes the code as text beside a
        // formula that produces it, and so does this.
        CellValue::Error(e) => {
            out.push_str(r#" office:value-type="string""#);
            Some(e.as_str().to_owned())
        }
    }
}

/// A number as one of the three shapes ODS gives numbers, by its format.
fn number_value(n: f64, code: &str, epoch: crate::shared::date::Epoch, out: &mut String) -> String {
    use crate::style::format::{Value as FormatValue, format, is_date_format};

    let shown = format(FormatValue::Number(n), code, epoch);
    if is_date_format(code) {
        // A format with hours but no days is a duration, which ODS states as
        // an ISO 8601 one rather than as a point in time.
        let clock = code.contains(['h', 'H', 's', 'S']);
        let calendar = code.contains(['y', 'Y', 'd', 'D']);
        if clock && !calendar {
            let _ = write!(
                out,
                r#" office:value-type="time" office:time-value="{}""#,
                duration(n)
            );
            return shown;
        }
        if let Ok(dt) = crate::shared::date::from_serial(n, epoch) {
            let _ = write!(
                out,
                r#" office:value-type="date" office:date-value="{:04}-{:02}-{:02}T{:02}:{:02}:{:02}""#,
                dt.year,
                dt.month,
                dt.day,
                dt.hour,
                dt.minute,
                seconds_of(dt.second),
            );
            return shown;
        }
    }
    let kind = if code.trim_end().ends_with('%') {
        "percentage"
    } else {
        "float"
    };
    let _ = write!(
        out,
        r#" office:value-type="{kind}" office:value="{}""#,
        shortest(n)
    );
    shown
}

/// Whole seconds of a time, clamped to the range a clock has.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=59 on the line above the cast"
)]
fn seconds_of(second: f64) -> u32 {
    second.trunc().clamp(0.0, 59.0) as u32
}

/// A fraction of a day as the ISO 8601 duration ODS wants.
fn duration(days: f64) -> String {
    let sign = if days < 0.0 { "-" } else { "" };
    let total = (days.abs() * 86_400.0).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "seconds in a spreadsheet's date range fit an i64 many times over"
    )]
    let seconds = total as u64;
    format!(
        "{sign}PT{:02}H{:02}M{:02}S",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

/// A number the way ODS wants it: a plain decimal, no exponent, no locale.
fn shortest(n: f64) -> String {
    if n.is_finite() {
        let plain = format!("{n}");
        // `1e21` and friends are valid Rust but not valid `office:value`.
        if plain.contains(['e', 'E']) {
            return format!("{n:.10}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_owned();
        }
        plain
    } else {
        "0".to_owned()
    }
}

/// The merge covering a cell, if any.
fn covered_by(sheet: &Worksheet, at: CellRef) -> Option<Range> {
    sheet
        .merges
        .iter()
        .find(|m| {
            (m.start.col..=m.end.col).contains(&at.col)
                && (m.start.row..=m.end.row).contains(&at.row)
        })
        .copied()
}

/// Whether a hyperlink covers a cell.
fn covers(link: &Hyperlink, at: CellRef) -> bool {
    (link.range.start.col..=link.range.end.col).contains(&at.col)
        && (link.range.start.row..=link.range.end.row).contains(&at.row)
}

/// An inside-the-document link target as ODS spells it: `Sheet1.A1`.
fn inside_target(place: &str) -> String {
    match place.split_once('!') {
        Some((sheet, cell)) => format!("{}.{cell}", sheet.trim_matches('\'')),
        None => place.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::ods::read_ods_from;
    use std::io::Cursor;

    fn at(a: &str) -> CellRef {
        CellRef::parse(a).expect("test reference is valid")
    }

    /// Writes a workbook to memory and reads it straight back.
    fn cycle(book: &Spreadsheet) -> Spreadsheet {
        let mut buf = Vec::new();
        write_ods_to(book, Cursor::new(&mut buf)).expect("workbook writes");
        read_ods_from(Cursor::new(buf)).expect("what we wrote reads back")
    }

    fn one_sheet(ws: Worksheet) -> Spreadsheet {
        let mut book = Spreadsheet::empty();
        book.add_sheet(ws).expect("added");
        book
    }

    #[test]
    fn the_mimetype_is_stored_first_and_uncompressed() {
        let ws = {
            let mut ws = Worksheet::new("S").expect("valid name");
            ws.set(at("A1"), 1.0);
            ws
        };
        let mut buf = Vec::new();
        write_ods_to(&one_sheet(ws), Cursor::new(&mut buf)).expect("writes");
        // A reader looks for the literal bytes near the start of the archive,
        // which only works if the entry is stored rather than deflated.
        let head = String::from_utf8_lossy(&buf[..120]).into_owned();
        assert!(head.contains("mimetype"), "{head}");
        assert!(head.contains(MIMETYPE), "{head}");

        let mut zip = zip::ZipArchive::new(Cursor::new(&buf)).expect("valid zip");
        for name in [
            "mimetype",
            "META-INF/manifest.xml",
            "content.xml",
            "styles.xml",
            "meta.xml",
        ] {
            assert!(zip.by_name(name).is_ok(), "package is missing {name}");
        }
    }

    #[test]
    fn values_survive_a_cycle() {
        let mut ws = Worksheet::new("Данные").expect("valid name");
        ws.set(at("A1"), "текст");
        ws.set(at("B1"), 1234.5);
        ws.set(at("C1"), true);
        ws.set(at("A2"), "two\nlines");
        ws.set(at("B2"), -0.25);
        let before = one_sheet(ws);
        let after = cycle(&before);

        assert_eq!(after.sheets()[0].title(), "Данные");
        for (cell, original) in before.sheets()[0].iter() {
            assert_eq!(
                after.sheets()[0].get(cell).map(|c| &c.value),
                Some(&original.value),
                "cell {cell}"
            );
        }
        assert_eq!(after.sheets()[0].len(), before.sheets()[0].len());
    }

    #[test]
    fn formulas_survive_translation_both_ways() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), 2.0);
        ws.set(at("A2"), 3.0);
        for (cell, formula) in [
            ("A3", "SUM(A1:A2)"),
            ("A4", "IF(A1>1,\"да,нет\",A2)"),
            ("A5", "CEILING(A1,1)"),
        ] {
            ws.set(
                at(cell),
                CellValue::Formula {
                    formula: formula.to_owned(),
                    cached: Some(Box::new(CellValue::Number(5.0))),
                },
            );
        }
        let after = cycle(&one_sheet(ws));
        let formula_at = |a: &str| match &after.sheets()[0].get(at(a)).expect("cell").value {
            CellValue::Formula { formula, .. } => formula.clone(),
            other => panic!("{a} is not a formula: {other:?}"),
        };
        assert_eq!(formula_at("A3"), "SUM(A1:A2)");
        assert_eq!(formula_at("A4"), "IF(A1>1,\"да,нет\",A2)");
        assert_eq!(formula_at("A5"), "CEILING(A1,1)");
    }

    #[test]
    fn merges_and_links_survive_a_cycle() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), "wide");
        ws.set(at("C1"), "click");
        ws.merges.push(Range::parse("A1:B2").expect("valid range"));
        ws.hyperlinks.push(Hyperlink {
            range: Range::parse("C1").expect("valid range"),
            target: crate::model::LinkTarget::Outside("https://example.test/".into()),
            display: None,
            tooltip: None,
        });
        let after = cycle(&one_sheet(ws));
        let sheet = &after.sheets()[0];
        assert_eq!(sheet.merges, vec![Range::parse("A1:B2").expect("range")]);
        assert_eq!(
            sheet.hyperlinks[0].target,
            crate::model::LinkTarget::Outside("https://example.test/".to_owned())
        );
        // The cells the merge covers stay empty on the way back.
        assert_eq!(sheet.get(at("B1")), None);
    }

    #[test]
    fn several_sheets_keep_their_order_and_names() {
        let mut book = Spreadsheet::empty();
        for name in ["Первый", "Второй", "Третий"] {
            let mut ws = Worksheet::new(name).expect("valid name");
            ws.set(at("A1"), name);
            book.add_sheet(ws).expect("added");
        }
        let after = cycle(&book);
        let titles: Vec<&str> = after.sheets().iter().map(Worksheet::title).collect();
        assert_eq!(titles, ["Первый", "Второй", "Третий"]);
    }

    #[test]
    fn style_components_survive_a_cycle() {
        let mut book = Spreadsheet::empty();
        let mut style = Style::default();
        style.font.name = "Georgia".into();
        style.font.size = 1400;
        style.font.bold = true;
        style.font.italic = true;
        style.font.underline = Underline::Single;
        style.font.strike = true;
        style.font.color = Color::Argb(0xFF00_7700);
        style.fill.pattern = Pattern::Solid;
        style.fill.foreground = Color::Argb(0xFFFF_FF00);
        style.alignment.horizontal = HorizontalAlign::Right;
        style.alignment.vertical = VerticalAlign::Center;
        style.alignment.wrap_text = true;
        style.borders.top = Border {
            style: BorderStyle::Named("thick"),
            color: Color::Argb(0xFFFF_0000),
        };
        let id = book.styles.intern(style.clone());

        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), "x");
        ws.entry(at("A1")).style = id;
        // An empty cell with a style, with something after it in the row.
        ws.entry(at("B1")).style = id;
        ws.set(at("C1"), "y");
        book.add_sheet(ws).expect("added");

        let after = cycle(&book);
        let sheet = &after.sheets()[0];
        let got = after
            .styles
            .get(sheet.get(at("A1")).expect("cell").style)
            .expect("style");
        assert_eq!(got.font.name, "Georgia");
        assert_eq!(got.font.size, 1400);
        assert!(got.font.bold && got.font.italic && got.font.strike);
        assert_eq!(got.font.underline, Underline::Single);
        assert_eq!(got.font.color, Color::Argb(0xFF00_7700));
        assert_eq!(got.fill.foreground, Color::Argb(0xFFFF_FF00));
        assert_eq!(got.alignment.horizontal, HorizontalAlign::Right);
        assert_eq!(got.alignment.vertical, VerticalAlign::Center);
        assert!(got.alignment.wrap_text);
        assert_eq!(got.borders.top.style, BorderStyle::Named("thick"));
        assert_eq!(got.borders.top.color, Color::Argb(0xFFFF_0000));
        // The styled empty cell in the middle of the row is kept.
        assert_eq!(
            sheet.get(at("B1")).map(|c| c.style),
            Some(got_id(&after, id))
        );
    }

    /// The style the cycle gave a cell that started with `id`.
    fn got_id(after: &Spreadsheet, _id: StyleId) -> StyleId {
        after.sheets()[0].get(at("A1")).expect("cell").style
    }

    #[test]
    fn a_date_keeps_being_a_date() {
        let mut book = Spreadsheet::empty();
        let dated = book.styles.intern(Style {
            number_format: crate::style::NumberFormat::Custom("yyyy-mm-dd".into()),
            ..Style::default()
        });
        let timed = book.styles.intern(Style {
            number_format: crate::style::NumberFormat::Custom("hh:mm:ss".into()),
            ..Style::default()
        });
        let percent = book.styles.intern(Style {
            number_format: crate::style::NumberFormat::Custom("0.00%".into()),
            ..Style::default()
        });
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), 45_296.0);
        ws.entry(at("A1")).style = dated;
        ws.set(at("B1"), 0.0625);
        ws.entry(at("B1")).style = timed;
        ws.set(at("C1"), 0.125);
        ws.entry(at("C1")).style = percent;
        book.add_sheet(ws).expect("added");

        let after = cycle(&book);
        let value = |a: &str| after.sheets()[0].get(at(a)).map(|c| c.value.clone());
        assert_eq!(value("A1"), Some(CellValue::Number(45_296.0)));
        assert_eq!(value("B1"), Some(CellValue::Number(0.0625)));
        assert_eq!(value("C1"), Some(CellValue::Number(0.125)));
    }

    #[test]
    fn refuses_to_write_a_workbook_with_no_sheets() {
        let mut buf = Vec::new();
        assert!(write_ods_to(&Spreadsheet::empty(), Cursor::new(&mut buf)).is_err());
    }

    #[test]
    fn a_number_is_written_without_an_exponent() {
        assert_eq!(shortest(1.5), "1.5");
        assert_eq!(shortest(1e21), "1000000000000000000000");
        assert_eq!(shortest(0.000_001), "0.000001");
    }
}
