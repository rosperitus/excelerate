//! Writing HTML.
//!
//! A sheet becomes a `<table>`: one `<tr>` per row, one `<td>` per cell, merges
//! as `colspan`/`rowspan`. The values go through the number-format engine, so
//! what the page shows is what Excel shows — a date is a date, not a serial
//! number.
//!
//! Styles are written once as CSS classes (`td.style7`), one per entry of the
//! workbook's style table, exactly as does. The alternative — an
//! inline `style=` on every cell — repeats the same declarations for every one
//! of a quarter-million cells.

use crate::error::{Error, Result};
use crate::formula::eval::{Engine, Origin};
use crate::formula::value::Value as FormulaValue;
use crate::model::{CellValue, Hyperlink, LinkTarget, Spreadsheet, TextRun, Worksheet};
use crate::style::format::{GENERAL, Value as FormatValue, format};
use crate::style::{
    Alignment, Border, BorderStyle, Color, Fill, Font, HorizontalAlign, Pattern, Script, Style,
    Underline, VerticalAlign,
};
use crate::{CellRef, Col, Range, Row};
use std::fmt::Write as _;
use std::io::Write;

/// What the writer puts around the tables.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HtmlOptions {
    /// Which sheet to write; `None` writes every sheet, one table each.
    pub sheet: Option<usize>,
    /// Whether to write only the tables and their stylesheet, for embedding in
    /// a page that already has a `<head>` and a `<body>`.
    pub fragment: bool,
}

/// Writes a workbook as an HTML page.
///
/// # Errors
/// [`Error::Html`] if the workbook has no such sheet, or the file cannot be
/// written.
pub fn write_html(book: &Spreadsheet, path: impl AsRef<std::path::Path>) -> Result<()> {
    let file = std::fs::File::create(path).map_err(|e| Error::Html(e.to_string()))?;
    write_html_to(book, std::io::BufWriter::new(file), &HtmlOptions::default())
}

/// The same, to any sink.
///
/// # Errors
/// [`Error::Html`] if the workbook has no such sheet, or the sink fails.
pub fn write_html_to<W: Write>(
    book: &Spreadsheet,
    mut sink: W,
    options: &HtmlOptions,
) -> Result<()> {
    let indices: Vec<usize> = match options.sheet {
        Some(i) if i < book.sheets().len() => vec![i],
        Some(i) => return Err(Error::Html(format!("workbook has no sheet {i}"))),
        None => (0..book.sheets().len()).collect(),
    };

    let mut out = String::new();
    if !options.fragment {
        out.push_str("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n");
        let title = book
            .sheets()
            .first()
            .map_or("Workbook", Worksheet::title)
            .to_owned();
        let _ = writeln!(out, "<title>{}</title>", escape(&title));
    }
    out.push_str("<style>\n");
    stylesheet(book, &indices, &mut out);
    out.push_str("</style>\n");
    if !options.fragment {
        out.push_str("</head>\n<body>\n");
    }

    let mut engine = Engine::new(book);
    for &index in &indices {
        // Checked above, or produced by the range over the sheets.
        let Some(sheet) = book.sheet(index) else {
            continue;
        };
        table(book, sheet, index, &mut engine, &mut out);
    }

    if !options.fragment {
        out.push_str("</body>\n</html>\n");
    }
    sink.write_all(out.as_bytes())
        .map_err(|e| Error::Html(e.to_string()))?;
    sink.flush().map_err(|e| Error::Html(e.to_string()))
}

/// Writes the stylesheet: the fixed rules, then one rule per cell style, then
/// the column widths and row heights of each sheet.
fn stylesheet(book: &Spreadsheet, indices: &[usize], out: &mut String) {
    let default_font = book
        .styles
        .get(crate::style::StyleId::default())
        .map_or_else(Font::default, |s| s.font.clone());
    let _ = writeln!(
        out,
        "html {{ font-family: {}, sans-serif; font-size: {}pt; background-color: white; }}",
        css_font_family(&default_font.name),
        points(default_font.size)
    );
    out.push_str("table { border-collapse: collapse; }\ntd, th { padding: 0 2px; }\n");

    for (index, style) in book.styles.all().iter().enumerate() {
        let declarations = css_of(style);
        if !declarations.is_empty() {
            let _ = writeln!(out, "td.style{index} {{ {declarations} }}");
        }
    }

    for &sheet_index in indices {
        let Some(sheet) = book.sheet(sheet_index) else {
            continue;
        };
        for run in &sheet.columns {
            let mut declarations = String::new();
            if let Some(width) = run.width {
                let _ = write!(
                    declarations,
                    "width: {}pt;",
                    trim(column_points(width, &default_font))
                );
            }
            if run.hidden {
                declarations.push_str(" display: none;");
            }
            if declarations.is_empty() {
                continue;
            }
            for col in run.first.one_based()..=run.last.one_based() {
                let _ = writeln!(
                    out,
                    "table.sheet{sheet_index} col.col{col} {{ {} }}",
                    declarations.trim()
                );
            }
        }
        for (row, properties) in &sheet.rows {
            let mut declarations = String::new();
            if let Some(height) = properties.height {
                let _ = write!(declarations, "height: {}pt;", trim(height));
            }
            if properties.hidden {
                declarations.push_str(" display: none;");
            }
            if !declarations.is_empty() {
                let _ = writeln!(
                    out,
                    "table.sheet{sheet_index} tr.row{} {{ {} }}",
                    row.one_based(),
                    declarations.trim()
                );
            }
        }
    }
}

/// Writes one sheet as a table.
fn table(
    book: &Spreadsheet,
    sheet: &Worksheet,
    index: usize,
    engine: &mut Engine<'_>,
    out: &mut String,
) {
    let _ = writeln!(
        out,
        "<table class=\"sheet{index}\" id=\"sheet{index}\">\n<caption>{}</caption>",
        escape(sheet.title())
    );
    let Some(used) = sheet.dimension() else {
        out.push_str("</table>\n");
        return;
    };

    let (first_col, last_col) = (used.start.col.one_based(), used.end.col.one_based());
    out.push_str("<colgroup>");
    for col in first_col..=last_col {
        let _ = write!(out, "<col class=\"col{col}\">");
    }
    out.push_str("</colgroup>\n");

    for row in used.start.row.one_based()..=used.end.row.one_based() {
        let Ok(row) = Row::from_one_based(u64::from(row)) else {
            break;
        };
        let _ = write!(out, "<tr class=\"row{}\">", row.one_based());
        for col in first_col..=last_col {
            let Ok(col) = Col::from_one_based(u64::from(col)) else {
                break;
            };
            let at = CellRef::new(col, row);
            let merge = covering_merge(sheet, at);
            // A cell swallowed by a merge that starts elsewhere writes no
            // `<td>` at all: the spanning one already covers this column.
            if merge.is_some_and(|m| m.start != at) {
                continue;
            }
            cell(book, sheet, index, at, merge, engine, out);
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

/// Writes one `<td>`.
fn cell(
    book: &Spreadsheet,
    sheet: &Worksheet,
    index: usize,
    at: CellRef,
    merge: Option<Range>,
    engine: &mut Engine<'_>,
    out: &mut String,
) {
    let cell = sheet.get(at);
    let style_id = cell.map(|c| c.style).unwrap_or_default();
    let _ = write!(
        out,
        "<td class=\"column{} style{}\"",
        at.col.one_based(),
        style_id.index()
    );
    if let Some(merge) = merge {
        if merge.width() > 1 {
            let _ = write!(out, " colspan=\"{}\"", merge.width());
        }
        if merge.height() > 1 {
            let _ = write!(out, " rowspan=\"{}\"", merge.height());
        }
    }
    out.push('>');

    let style = book.styles.get(style_id);
    let code = style.map_or(GENERAL, |s| s.number_format.code());
    let body = match cell.map(|c| &c.value) {
        None | Some(CellValue::Empty) => String::new(),
        Some(CellValue::RichText(runs)) => rich_text(runs),
        Some(value) => {
            let shown = displayed(engine, index, at, value);
            rendered(&shown, code, book.epoch)
        }
    };
    match link_at(sheet, at) {
        Some(link) => {
            let href = match &link.target {
                LinkTarget::Outside(url) => escape(url),
                LinkTarget::Inside(place) => format!("#{}", escape(place)),
            };
            let _ = write!(out, "<a href=\"{href}\"");
            if let Some(tip) = &link.tooltip {
                let _ = write!(out, " title=\"{}\"", escape(tip));
            }
            let text = match &link.display {
                Some(text) if body.is_empty() => escape(text),
                _ => body,
            };
            let _ = write!(out, ">{text}</a>");
        }
        None => out.push_str(&body),
    }
    out.push_str("</td>");
}

/// The value a viewer would see: a formula stands for its result.
fn displayed(engine: &mut Engine<'_>, sheet: usize, at: CellRef, value: &CellValue) -> CellValue {
    match value {
        CellValue::Formula { formula, cached } => match cached {
            Some(cached) => (**cached).clone(),
            None => match engine.eval(Origin::new(sheet, at), formula) {
                FormulaValue::Number(n) => CellValue::Number(n),
                FormulaValue::Text(t) => CellValue::Text(t),
                FormulaValue::Bool(b) => CellValue::Bool(b),
                FormulaValue::Error(e) => CellValue::Error(e),
                FormulaValue::Blank => CellValue::Empty,
                // A cell cannot hold a function; Excel shows `#CALC!`.
                FormulaValue::Lambda(_) => CellValue::Error(crate::error::CellError::Calc),
                // A formula that produced a whole array shows its top-left
                // value in the cell, which is what a reader without spilling
                // sees; dropping it would show an empty cell instead.
                FormulaValue::Array(rows) => {
                    rows.first()
                        .and_then(|line| line.first())
                        .map_or(CellValue::Empty, |value| match value {
                            FormulaValue::Number(n) => CellValue::Number(*n),
                            FormulaValue::Text(t) => CellValue::Text(t.clone()),
                            FormulaValue::Bool(b) => CellValue::Bool(*b),
                            FormulaValue::Error(e) => CellValue::Error(*e),
                            FormulaValue::Lambda(_) => {
                                CellValue::Error(crate::error::CellError::Calc)
                            }
                            FormulaValue::Blank | FormulaValue::Array(_) => CellValue::Empty,
                        })
                }
            },
        },
        other => other.clone(),
    }
}

/// A value through its number format, escaped and ready for the page.
///
fn rendered(value: &CellValue, code: &str, epoch: crate::shared::date::Epoch) -> String {
    let text = match value {
        CellValue::Number(n) => format(FormatValue::Number(*n), code, epoch),
        CellValue::Text(t) => format(FormatValue::Text(t), code, epoch),
        CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        CellValue::Error(e) => e.as_str().to_owned(),
        CellValue::Empty | CellValue::RichText(_) | CellValue::Formula { .. } => String::new(),
    };
    let escaped = escape(&text);
    match colour_prefix(code) {
        Some(colour) => format!("<span style=\"color:{colour}\">{escaped}</span>"),
        None => escaped,
    }
}

/// The `[Red]` colour a format opens with, lower-cased as a CSS colour name.
fn colour_prefix(code: &str) -> Option<String> {
    let rest = code.strip_prefix('[')?;
    let (name, _) = rest.split_once(']')?;
    name.chars()
        .all(char::is_alphabetic)
        .then(|| name.to_lowercase())
}

/// Formatted runs inside one cell, each as a `<span>`.
fn rich_text(runs: &[TextRun]) -> String {
    let mut out = String::new();
    for run in runs {
        let mut declarations = String::new();
        if let Some(font) = &run.font {
            if font.bold == Some(true) {
                declarations.push_str("font-weight:bold;");
            }
            if font.italic == Some(true) {
                declarations.push_str("font-style:italic;");
            }
            if let Some(color) = font.color.as_ref().and_then(css_color) {
                let _ = write!(declarations, "color:{color};");
            }
            if let Some(size) = font.size {
                let _ = write!(declarations, "font-size:{}pt;", points(size));
            }
        }
        if declarations.is_empty() {
            out.push_str(&escape(&run.text));
        } else {
            let _ = write!(
                out,
                "<span style=\"{declarations}\">{}</span>",
                escape(&run.text)
            );
        }
    }
    out
}

/// The merge covering a cell, if any.
fn covering_merge(sheet: &Worksheet, at: CellRef) -> Option<Range> {
    sheet
        .merges
        .iter()
        .find(|m| {
            (m.start.col..=m.end.col).contains(&at.col)
                && (m.start.row..=m.end.row).contains(&at.row)
        })
        .copied()
}

/// The hyperlink whose range covers a cell, if any.
fn link_at(sheet: &Worksheet, at: CellRef) -> Option<&Hyperlink> {
    sheet.hyperlinks.iter().find(|h| {
        (h.range.start.col..=h.range.end.col).contains(&at.col)
            && (h.range.start.row..=h.range.end.row).contains(&at.row)
    })
}

/// One cell style as CSS declarations.
///
fn css_of(style: &Style) -> String {
    let mut out = String::new();
    alignment_css(&style.alignment, &mut out);
    for (side, border) in [
        ("top", &style.borders.top),
        ("right", &style.borders.right),
        ("bottom", &style.borders.bottom),
        ("left", &style.borders.left),
    ] {
        if let Some(rule) = border_css(border) {
            let _ = write!(out, "border-{side}: {rule}; ");
        }
    }
    font_css(&style.font, &mut out);
    fill_css(&style.fill, &mut out);
    out.trim_end().to_owned()
}

/// Placement.
fn alignment_css(alignment: &Alignment, out: &mut String) {
    let horizontal = match alignment.horizontal {
        HorizontalAlign::General => None,
        HorizontalAlign::Left | HorizontalAlign::Fill => Some("left"),
        HorizontalAlign::Center | HorizontalAlign::CenterContinuous => Some("center"),
        HorizontalAlign::Right => Some("right"),
        HorizontalAlign::Justify | HorizontalAlign::Distributed => Some("justify"),
    };
    if let Some(value) = horizontal {
        let _ = write!(out, "text-align: {value}; ");
        // The indent becomes padding on the side the text sits on.
        if alignment.indent > 0 && (value == "left" || value == "right") {
            let _ = write!(out, "padding-{value}: {}px; ", alignment.indent * 9);
        }
    } else if alignment.indent > 0 {
        let _ = write!(out, "text-indent: {}px; ", alignment.indent * 9);
    }
    let vertical = match alignment.vertical {
        VerticalAlign::Bottom => "bottom",
        VerticalAlign::Top => "top",
        VerticalAlign::Center => "middle",
        VerticalAlign::Justify | VerticalAlign::Distributed => "baseline",
    };
    let _ = write!(out, "vertical-align: {vertical}; ");
    if alignment.wrap_text {
        out.push_str("white-space: pre-wrap; ");
    }
    // 255 is how the format says "stacked", which is not a rotation.
    if alignment.text_rotation > 0 && alignment.text_rotation != 255 {
        let degrees = i64::from(alignment.text_rotation);
        // Excel counts 91..=180 as 1..=90 degrees the other way.
        let degrees = if degrees > 90 { 90 - degrees } else { degrees };
        let _ = write!(out, "transform: rotate({degrees}deg); ");
    }
}

/// One border side.
fn border_css(border: &Border) -> Option<String> {
    let line = match border.style {
        BorderStyle::None => return None,
        BorderStyle::Named(name) => match name {
            "dashed" | "dashDot" => "1px dashed",
            "dotted" | "dashDotDot" => "1px dotted",
            "double" => "3px double",
            "medium" => "2px solid",
            "mediumDashed" | "mediumDashDot" => "2px dashed",
            "mediumDashDotDot" => "2px dotted",
            "thick" => "3px solid",
            // `thin`, `hair` and anything unknown.
            _ => "1px solid",
        },
    };
    let colour = css_color(&border.color).unwrap_or_else(|| "#000000".to_owned());
    Some(format!("{line} {colour}"))
}

/// Typeface.
fn font_css(font: &Font, out: &mut String) {
    if font.bold {
        out.push_str("font-weight: bold; ");
    }
    let decoration = match (font.underline != Underline::None, font.strike) {
        (true, true) => Some("underline line-through"),
        (true, false) => Some("underline"),
        (false, true) => Some("line-through"),
        (false, false) => None,
    };
    if let Some(value) = decoration {
        let _ = write!(out, "text-decoration: {value}; ");
    }
    if font.italic {
        out.push_str("font-style: italic; ");
    }
    if let Some(colour) = css_color(&font.color) {
        let _ = write!(out, "color: {colour}; ");
    }
    let _ = write!(
        out,
        "font-family: {}; font-size: {}pt; ",
        css_font_family(&font.name),
        points(font.size)
    );
    match font.script {
        Script::Baseline => {}
        Script::Superscript => out.push_str("vertical-align: super; font-size: smaller; "),
        Script::Subscript => out.push_str("vertical-align: sub; font-size: smaller; "),
    }
}

/// Background.
fn fill_css(fill: &Fill, out: &mut String) {
    if fill.pattern == Pattern::None {
        return;
    }
    let colour = css_color(&fill.foreground).or_else(|| css_color(&fill.background));
    if let Some(colour) = colour {
        let _ = write!(out, "background-color: {colour}; ");
    }
}

/// A colour as CSS, for the colours that state their own value.
///
/// Theme and indexed colours name an entry of a palette this writer does not
/// resolve, so they are left to the page's default rather than guessed at.
fn css_color(color: &Color) -> Option<String> {
    match color {
        Color::Argb(v) => {
            let (r, g, b) = ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF);
            let alpha = (v >> 24) & 0xFF;
            if alpha == 0xFF || alpha == 0 {
                // Excel writes fully opaque colours with alpha 00 as often as
                // FF, and a transparent cell colour is not a thing it means.
                Some(format!("#{r:02X}{g:02X}{b:02X}"))
            } else {
                Some(format!(
                    "rgba({r}, {g}, {b}, {:.3})",
                    f64::from(alpha) / 255.0
                ))
            }
        }
        Color::Auto | Color::Indexed(_) | Color::Theme { .. } => None,
    }
}

/// A font name as a CSS family, quoted so a name with spaces survives.
fn css_font_family(name: &str) -> String {
    format!("'{}'", name.replace(['\'', '\\'], ""))
}

/// Hundredths of a point as points, without a trailing `.0`.
fn points(hundredths: u32) -> String {
    trim(f64::from(hundredths) / 100.0)
}

/// A number without a trailing `.0`, so `11pt` does not read `11.0pt`.
fn trim(n: f64) -> String {
    let text = format!("{n:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A column width in characters as points.
///
/// A measured pixel width for fifteen fonts at every size would be exact;
/// only the ratio for the default font matters here, so this uses the one for
/// Calibri 11 — 9.140625 characters to 64 pixels — and extrapolates by size,
/// which is what itself does for every font it has no table for.
fn column_points(width: f64, font: &Font) -> f64 {
    const CALIBRI_11_PIXELS_PER_CHARACTER: f64 = 64.0 / 9.140_625;
    let size = f64::from(font.size) / 100.0;
    let pixels = if (size - 11.0).abs() < f64::EPSILON {
        width * CALIBRI_11_PIXELS_PER_CHARACTER
    } else {
        width * size * CALIBRI_11_PIXELS_PER_CHARACTER / 11.0
    };
    pixels.round() * 0.75
}

/// Escapes text for a page, and turns the line breaks Excel stores into ones a
/// browser shows.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{NumberFormat, StyleId};

    fn at(a: &str) -> CellRef {
        CellRef::parse(a).expect("test reference is valid")
    }

    fn page(book: &Spreadsheet) -> String {
        let mut out = Vec::new();
        write_html_to(book, &mut out, &HtmlOptions::default()).expect("writes");
        String::from_utf8(out).expect("utf-8")
    }

    fn one_sheet(ws: Worksheet) -> Spreadsheet {
        let mut book = Spreadsheet::empty();
        book.add_sheet(ws).expect("added");
        book
    }

    #[test]
    fn a_sheet_becomes_a_table() {
        let mut ws = Worksheet::new("Данные").expect("valid name");
        ws.set(at("A1"), "a");
        ws.set(at("B1"), 1.0);
        ws.set(at("A2"), true);
        let html = page(&one_sheet(ws));
        assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
        assert!(html.contains("<caption>Данные</caption>"), "{html}");
        assert!(html.contains(">a</td>"), "{html}");
        assert!(html.contains(">1</td>"), "{html}");
        assert!(html.contains(">TRUE</td>"), "{html}");
        // Two rows, two columns, and an empty cell where B2 would be.
        assert_eq!(html.matches("<tr").count(), 2, "{html}");
        assert_eq!(html.matches("<td").count(), 4, "{html}");
    }

    #[test]
    fn a_value_is_shown_the_way_its_format_shows_it() {
        let mut book = Spreadsheet::empty();
        let dated = book.styles.intern(Style {
            number_format: NumberFormat::Custom("yyyy-mm-dd".into()),
            ..Style::default()
        });
        let red = book.styles.intern(Style {
            number_format: NumberFormat::Custom("[Red]0.00".into()),
            ..Style::default()
        });
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), 45_000.0);
        ws.entry(at("A1")).style = dated;
        ws.set(at("B1"), -1.5);
        ws.entry(at("B1")).style = red;
        book.add_sheet(ws).expect("added");

        let html = page(&book);
        assert!(html.contains(">2023-03-15</td>"), "{html}");
        assert!(
            html.contains("<span style=\"color:red\">-1.50</span>"),
            "{html}"
        );
    }

    #[test]
    fn merges_become_spans_and_swallow_the_cells_under_them() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), "wide");
        ws.set(at("C3"), "corner");
        ws.merges.push(Range::parse("A1:B2").expect("valid range"));
        let html = page(&one_sheet(ws));
        assert!(html.contains("colspan=\"2\" rowspan=\"2\""), "{html}");
        // Three rows of three columns, less the three cells the merge covers.
        assert_eq!(html.matches("<td").count(), 9 - 3, "{html}");
    }

    #[test]
    fn a_style_becomes_a_class_written_once() {
        let mut book = Spreadsheet::empty();
        let mut style = Style::default();
        style.font.bold = true;
        style.font.color = Color::Argb(0xFF00_7700);
        style.fill.pattern = Pattern::Solid;
        style.fill.foreground = Color::Argb(0xFFFF_FF00);
        style.alignment.horizontal = HorizontalAlign::Right;
        style.borders.top.style = BorderStyle::Named("thick");
        let id = book.styles.intern(style);

        let mut ws = Worksheet::new("S").expect("valid name");
        for row in 1..=3 {
            let cell = at(&format!("A{row}"));
            ws.set(cell, f64::from(row));
            ws.entry(cell).style = id;
        }
        book.add_sheet(ws).expect("added");

        let html = page(&book);
        let rule = format!("td.style{}", id.index());
        assert_eq!(html.matches(&rule).count(), 1, "one rule, not one per cell");
        assert!(html.contains("font-weight: bold;"), "{html}");
        assert!(html.contains("color: #007700;"), "{html}");
        assert!(html.contains("background-color: #FFFF00;"), "{html}");
        assert!(html.contains("text-align: right;"), "{html}");
        assert!(html.contains("border-top: 3px solid #000000;"), "{html}");
        assert_eq!(html.matches(&format!("style{}\"", id.index())).count(), 3);
    }

    #[test]
    fn text_that_looks_like_markup_is_escaped() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), "<b>&\"x\"</b>");
        ws.set(at("A2"), "two\nlines");
        let html = page(&one_sheet(ws));
        assert!(
            html.contains("&lt;b&gt;&amp;&quot;x&quot;&lt;/b&gt;"),
            "{html}"
        );
        assert!(html.contains("two<br>lines"), "{html}");
    }

    #[test]
    fn a_formula_that_spills_shows_its_first_value() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), 1.0);
        ws.set(at("A2"), 2.0);
        ws.set(
            at("B1"),
            CellValue::Formula {
                formula: "A1:A2*2".to_owned(),
                cached: None,
            },
        );
        let html = page(&one_sheet(ws));
        assert!(html.contains(">2</td>"), "{html}");
    }

    #[test]
    fn a_link_wraps_the_value() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), "click");
        ws.hyperlinks.push(Hyperlink {
            range: Range::parse("A1").expect("valid range"),
            target: LinkTarget::Outside("https://example.test/?a=1&b=2".into()),
            display: None,
            tooltip: Some("go".into()),
        });
        let html = page(&one_sheet(ws));
        assert!(
            html.contains("<a href=\"https://example.test/?a=1&amp;b=2\" title=\"go\">click</a>"),
            "{html}"
        );
    }

    #[test]
    fn a_fragment_carries_no_document_around_it() {
        let mut ws = Worksheet::new("S").expect("valid name");
        ws.set(at("A1"), 1.0);
        let book = one_sheet(ws);
        let mut out = Vec::new();
        write_html_to(
            &book,
            &mut out,
            &HtmlOptions {
                fragment: true,
                ..HtmlOptions::default()
            },
        )
        .expect("writes");
        let html = String::from_utf8(out).expect("utf-8");
        assert!(!html.contains("<html>"), "{html}");
        assert!(!html.contains("<body>"), "{html}");
        assert!(html.contains("<table"), "{html}");
        assert!(html.contains("<style>"), "{html}");
    }

    #[test]
    fn only_the_sheet_asked_for_is_written() {
        let mut book = Spreadsheet::empty();
        for name in ["One", "Two"] {
            let mut ws = Worksheet::new(name).expect("valid name");
            ws.set(at("A1"), name);
            book.add_sheet(ws).expect("added");
        }
        let mut out = Vec::new();
        write_html_to(
            &book,
            &mut out,
            &HtmlOptions {
                sheet: Some(1),
                ..HtmlOptions::default()
            },
        )
        .expect("writes");
        let html = String::from_utf8(out).expect("utf-8");
        assert!(html.contains("<caption>Two</caption>"), "{html}");
        assert!(!html.contains("<caption>One</caption>"), "{html}");

        let mut out = Vec::new();
        assert!(
            write_html_to(
                &book,
                &mut out,
                &HtmlOptions {
                    sheet: Some(9),
                    ..HtmlOptions::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn a_column_width_in_characters_becomes_points() {
        let font = Font::default();
        // Excel's default column: 8.43 characters, which is 64 pixels for the
        // default font, and 48 points.
        assert!((column_points(9.140_625, &font) - 48.0).abs() < 0.01);
        assert_eq!(trim(column_points(9.140_625, &font)), "48");
        // The style table's first entry is the default style, so an unstyled
        // cell points at style 0.
        assert_eq!(StyleId::default().index(), 0);
    }
}
