//! Reading SYLK (`.slk`).
//!
//! A SYLK file is lines of `TYPE;FIELD;FIELD`, where each field starts with the
//! letter that says what it is. The format is from 1984 and holds one sheet:
//! `C` records carry values, `F` records formatting, `P` records the shared
//! formats and fonts the `F` records point at.
//!
//! Formulas are written in R1C1, where `R[-1]C` means the cell above, so they
//! are translated to A1 on the way in.
//!
//! Not ported: the `\x1b` escapes that spell out accented characters in the
//! seven-bit form of the format (a
//! table of a hundred entries), and comments, which the model has no place for.

use crate::error::{Error, Result};
use crate::model::{CellValue, ColumnRun, Spreadsheet, Worksheet};
use crate::style::{Border, BorderStyle, Color, NumberFormat, Pattern, Style, StyleTable};
use crate::{CellRef, Col, Row};

/// Reads a SYLK file.
///
/// # Errors
/// [`Error::Slk`] if the file cannot be read or does not start with the `ID;P`
/// line every SYLK file opens with.
pub fn read_slk(path: impl AsRef<std::path::Path>) -> Result<Spreadsheet> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| Error::Slk(e.to_string()))?;
    // The sheet takes the name of the file, as does.
    let title = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("Worksheet");
    read_slk_str(&super::csv::decode(&bytes), title)
}

/// Reads SYLK text already in memory, naming the sheet.
///
/// # Errors
/// [`Error::Slk`] if the text does not start with `ID;P`.
pub fn read_slk_str(text: &str, title: &str) -> Result<Spreadsheet> {
    if !text.starts_with("ID;P") {
        return Err(Error::Slk("not a SYLK file: no ID;P line".to_owned()));
    }
    let mut build = Build::new(title)?;
    for line in text.lines() {
        build.line(line);
    }
    Ok(build.finish())
}

/// What the reader carries from line to line.
struct Build {
    sheet: Worksheet,
    styles: StyleTable,
    /// One-based, as the file writes them.
    row: u32,
    col: u32,
    /// Number formats from `P` records, in the order they appear.
    formats: Vec<String>,
    /// Fonts from `P` records, likewise. Only the parts a cell style keeps.
    fonts: Vec<Style>,
}

impl Build {
    fn new(title: &str) -> Result<Self> {
        Ok(Self {
            sheet: Worksheet::new(title.chars().take(31).collect::<String>())?,
            styles: StyleTable::default(),
            row: 1,
            col: 1,
            formats: Vec::new(),
            fonts: Vec::new(),
        })
    }

    fn finish(self) -> Spreadsheet {
        let mut book = Spreadsheet::empty();
        book.styles = self.styles;
        let _ = book.add_sheet(self.sheet);
        book
    }

    /// One line.
    fn line(&mut self, line: &str) {
        let mut fields = split_fields(line.trim_end_matches(['\r', '\n']));
        if fields.is_empty() {
            return;
        }
        let kind = fields.remove(0);
        match kind.as_str() {
            "P" => self.shared_format(&fields),
            "C" => self.cell(&fields),
            "F" => self.format(&fields),
            // `B` states the size of the sheet, and the rest only move the
            // cursor.
            _ => self.cursor(&fields),
        }
    }

    /// `X` and `Y` (or `C` and `R`) move the cursor.
    fn cursor(&mut self, fields: &[String]) {
        for field in fields {
            let (letter, rest) = split_field(field);
            match letter {
                'X' | 'C' => self.col = rest.parse().unwrap_or(self.col),
                'Y' | 'R' => self.row = rest.parse().unwrap_or(self.row),
                _ => {}
            }
        }
    }

    /// The cell the cursor is on.
    fn at(&self) -> Option<CellRef> {
        Some(CellRef::new(
            Col::from_one_based(u64::from(self.col)).ok()?,
            Row::from_one_based(u64::from(self.row)).ok()?,
        ))
    }

    /// A `P` record: a number format, or a font, for the `F` records to point
    /// at by position.
    fn shared_format(&mut self, fields: &[String]) {
        let mut format: Option<String> = None;
        let mut font = Style::default();
        let mut has_font = false;
        for field in fields {
            let (letter, rest) = split_field(field);
            match letter {
                // A backslash escapes a dash or a space inside the code.
                'P' => format = Some(rest.replace("\\-", "-").replace("\\ ", " ")),
                'E' | 'F' => {
                    rest.clone_into(&mut font.font.name);
                    has_font = true;
                }
                'M' => {
                    // The size is in twentieths of a point.
                    if let Ok(twips) = rest.parse::<f64>() {
                        font.font.set_size_points(twips / 20.0);
                        has_font = true;
                    }
                }
                'L' => {
                    if let Ok(index) = rest.parse::<usize>() {
                        font.font.color = palette(index % 8);
                        has_font = true;
                    }
                }
                'S' => {
                    for c in rest.chars() {
                        match c {
                            'B' => font.font.bold = true,
                            'I' => font.font.italic = true,
                            'U' => font.font.underline = crate::style::Underline::Single,
                            'S' => font.font.strike = true,
                            _ => {}
                        }
                    }
                    has_font = true;
                }
                _ => {}
            }
        }
        // The record is filed under whichever it turned out to be: a
        // number format if it named one, a font otherwise.
        if let Some(code) = format {
            self.formats.push(code);
        } else if has_font {
            self.fonts.push(font);
        }
    }

    /// A `C` record: the value of one cell, and possibly a formula.
    ///
    /// Only `X` and `Y` move the cursor here: in a `C` record `C` and `R` name
    /// the master of a shared formula instead.
    fn cell(&mut self, fields: &[String]) {
        for field in fields {
            let (letter, rest) = split_field(field);
            match letter {
                'X' => self.col = rest.parse().unwrap_or(self.col),
                'Y' => self.row = rest.parse().unwrap_or(self.row),
                _ => {}
            }
        }
        let mut value: Option<CellValue> = None;
        let mut formula: Option<String> = None;
        let (mut shared_col, mut shared_row) = (None, None);
        let mut shared = false;
        for field in fields {
            let (letter, rest) = split_field(field);
            match letter {
                'K' => value = Some(literal(rest)),
                'E' => formula = Some(r1c1_to_a1(rest, self.row, self.col)),
                'C' => shared_col = rest.parse::<u32>().ok(),
                'R' => shared_row = rest.parse::<u32>().ok(),
                'S' => shared = true,
                _ => {}
            }
        }
        let Some(at) = self.at() else { return };

        // A shared formula names the cell that holds the master; every other
        // cell of the run offsets it.
        if shared && let (Some(col), Some(row)) = (shared_col, shared_row) {
            let master = CellRef::new(
                match Col::from_one_based(u64::from(col)) {
                    Ok(col) => col,
                    Err(_) => return,
                },
                match Row::from_one_based(u64::from(row)) {
                    Ok(row) => row,
                    Err(_) => return,
                },
            );
            if let Some(CellValue::Formula { formula, .. }) =
                self.sheet.get(master).map(|c| c.value.clone())
            {
                let shifted = crate::coordinate::shift_references(
                    &formula,
                    i64::from(self.col) - i64::from(col),
                    i64::from(self.row) - i64::from(row),
                );
                self.sheet.entry(at).value = CellValue::Formula {
                    formula: shifted,
                    cached: value.map(Box::new),
                };
            }
            return;
        }

        self.sheet.entry(at).value = match formula {
            Some(formula) => CellValue::Formula {
                formula,
                cached: value.map(Box::new),
            },
            None => value.unwrap_or(CellValue::Empty),
        };
    }

    /// An `F` record: the formatting of a cell, or the width of a run of
    /// columns.
    fn format(&mut self, fields: &[String]) {
        self.cursor(fields);
        let mut style: Option<Style> = None;
        for field in fields {
            let (letter, rest) = split_field(field);
            match letter {
                'P' => {
                    if let Some(code) = rest.parse::<usize>().ok().and_then(|i| self.formats.get(i))
                    {
                        style
                            .get_or_insert_with(|| self.current_style())
                            .number_format = NumberFormat::Custom(code.clone());
                    }
                }
                'W' => self.widths(rest),
                'S' => {
                    let style = style.get_or_insert_with(|| self.current_style());
                    apply_settings(style, rest, &self.fonts);
                }
                _ => {}
            }
        }
        if let (Some(style), Some(at)) = (style, self.at()) {
            let id = self.styles.intern(style);
            self.sheet.entry(at).style = id;
        }
    }

    /// The style the cursor's cell has now, which the record adds to.
    fn current_style(&self) -> Style {
        self.at()
            .and_then(|at| self.sheet.get(at))
            .map(|cell| cell.style)
            .and_then(|id| self.styles.get(id).cloned())
            .unwrap_or_default()
    }

    /// A `W` field: `first last width`.
    fn widths(&mut self, rest: &str) {
        let mut parts = rest.split_whitespace();
        let (Some(first), Some(last), Some(width)) = (parts.next(), parts.next(), parts.next())
        else {
            return;
        };
        let (Ok(first), Ok(last), Ok(width)) = (
            first.parse::<u64>(),
            last.parse::<u64>(),
            width.parse::<f64>(),
        ) else {
            return;
        };
        let (Ok(first), Ok(last)) = (
            Col::from_one_based(first),
            Col::from_one_based(last.max(first)),
        ) else {
            return;
        };
        let mut run = ColumnRun::new(first, last);
        run.width = Some(width);
        run.custom_width = true;
        self.sheet.columns.push(run);
    }
}

/// A cell style's `S` field: one letter per setting.
fn apply_settings(style: &mut Style, settings: &str, fonts: &[Style]) {
    let thin = Border {
        style: BorderStyle::parse("thin"),
        color: Color::default(),
    };
    let mut chars = settings.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            'D' => style.font.bold = true,
            'I' => style.font.italic = true,
            'B' => style.borders.bottom = thin.clone(),
            'L' => style.borders.left = thin.clone(),
            'R' => style.borders.right = thin.clone(),
            'T' => style.borders.top = thin.clone(),
            'S' => style.fill.pattern = Pattern::parse("gray125"),
            'M' => {
                // `M3` picks the third font of the `P` records.
                let mut digits = String::new();
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    digits.extend(chars.next());
                }
                if let Some(font) = digits
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| fonts.get(i.saturating_sub(1)))
                {
                    style.font = font.font.clone();
                }
            }
            _ => {}
        }
    }
}

/// The eight colours a `L` field can name, in the order SYLK numbers them.
fn palette(index: usize) -> Color {
    const COLOURS: [u32; 8] = [
        0xFF00_0000,
        0xFF00_00FF,
        0xFF00_FF00,
        0xFF00_FFFF,
        0xFFFF_0000,
        0xFFFF_00FF,
        0xFFFF_FF00,
        0xFFFF_FFFF,
    ];
    Color::Argb(COLOURS[index % 8])
}

/// Splits a line into fields on `;`, where `;;` is a literal semicolon.
fn split_fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut rest = line.chars().peekable();
    while let Some(c) = rest.next() {
        if c != ';' {
            // The last field always exists: the vector starts with one.
            if let Some(field) = out.last_mut() {
                field.push(c);
            }
            continue;
        }
        if rest.peek() == Some(&';') {
            rest.next();
            if let Some(field) = out.last_mut() {
                field.push(';');
            }
            continue;
        }
        out.push(String::new());
    }
    out.retain(|field| !field.is_empty());
    out
}

/// A field as its leading letter and the rest.
fn split_field(field: &str) -> (char, &str) {
    let mut chars = field.chars();
    let letter = chars.next().unwrap_or(' ');
    (letter, chars.as_str())
}

/// A `K` field: a quoted string, a number, or a logical value.
fn literal(text: &str) -> CellValue {
    if let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        return CellValue::text(inner);
    }
    // `TRUE` is not a logical value here: the format has no type for one, and
    // keeps it as the text it is written as.
    if let Some(error) = crate::CellError::parse(text) {
        return CellValue::Error(error);
    }
    text.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .map_or_else(|| CellValue::text(text), CellValue::Number)
}

/// Rewrites the R1C1 references of a formula as A1 ones.
///
pub(crate) fn r1c1_to_a1(formula: &str, row: u32, col: u32) -> String {
    let chars: Vec<char> = formula.chars().collect();
    let mut out = String::with_capacity(formula.len());
    let mut i = 0;
    let mut in_text = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            in_text = !in_text;
            out.push(c);
            i += 1;
            continue;
        }
        if in_text || c != 'R' {
            out.push(c);
            i += 1;
            continue;
        }
        // `R` opens a reference only when a `C` part follows it.
        let after_row = index_part(&chars, i + 1, row);
        let Some((row_number, at)) = after_row else {
            out.push(c);
            i += 1;
            continue;
        };
        if chars.get(at) != Some(&'C') {
            out.push(c);
            i += 1;
            continue;
        }
        let Some((col_number, end)) = index_part(&chars, at + 1, col) else {
            out.push(c);
            i += 1;
            continue;
        };
        match (
            Col::from_one_based(col_number.max(1)),
            Row::from_one_based(row_number.max(1)),
        ) {
            (Ok(col), Ok(row)) => out.push_str(&CellRef::new(col, row).to_string()),
            _ => out.push_str("#REF!"),
        }
        i = end;
    }
    out
}

/// One half of an R1C1 reference: nothing, a number, or a bracketed offset.
/// Returns the one-based index it names and where it ends.
fn index_part(chars: &[char], at: usize, current: u32) -> Option<(u64, usize)> {
    let mut i = at;
    if chars.get(i) == Some(&'[') {
        let close = chars[i..].iter().position(|&c| c == ']')? + i;
        let offset: i64 = chars[i + 1..close]
            .iter()
            .collect::<String>()
            .parse()
            .ok()?;
        let value = i64::from(current) + offset;
        return Some((u64::try_from(value).ok()?, close + 1));
    }
    let start = i;
    while chars.get(i).is_some_and(char::is_ascii_digit) {
        i += 1;
    }
    if i == start {
        // A bare `R` or `C` is this row or this column.
        return Some((u64::from(current), i));
    }
    let value: u64 = chars[start..i].iter().collect::<String>().parse().ok()?;
    Some((value, i))
}
