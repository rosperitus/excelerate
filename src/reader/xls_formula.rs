//! Turning a BIFF8 formula back into text.
//!
//! A formula is stored as tokens in reverse Polish order: `=SUM(A1:A3)*2`
//! is an area, a call to function 4 with one argument, the integer 2 and a
//! multiply. Replaying them onto a stack of text fragments gives the formula
//! back. Parentheses are tokens of their own, so no precedence has to be
//! worked out: the text comes out as it was typed, bar the spaces.
//!
//! Some tokens carry more than they fit: an array constant keeps its values,
//! and a memory area its cached rectangles, after the token stream itself, in
//! the same order the tokens come. Both are consumed in step.
//!
//! A token this module does not know, or a formula whose tokens do not add up
//! to one expression, gives `None`: the caller keeps the cached value rather
//! than a formula that says something else.

use crate::shared::biff_functions;
use crate::{CellError, Col};

/// What a formula can refer to outside itself: sheets, other workbooks and
/// their names, and the workbook's own defined names.
#[derive(Debug, Default)]
pub struct Context {
    /// Sheet names, in tab order.
    pub sheets: Vec<String>,
    /// `EXTERNSHEET` entries: the book, the first sheet and the last sheet.
    pub externs: Vec<(u16, u16, u16)>,
    /// `SUPBOOK` records, in order.
    pub books: Vec<Book>,
    /// Defined names, in `NAME` record order.
    pub names: Vec<String>,
}

/// One `SUPBOOK`: where 3D references and external names point.
#[derive(Debug, Default)]
pub struct Book {
    /// What kind of book it is.
    pub kind: BookKind,
    /// The `EXTERNNAME` records that follow it.
    pub names: Vec<String>,
}

/// The three things a `SUPBOOK` can stand for.
#[derive(Debug, Default)]
pub enum BookKind {
    /// This workbook.
    #[default]
    Internal,
    /// An add-in, whose external names are function names.
    AddIn,
    /// Another file, with its sheet names.
    External {
        /// The path as the file encodes it.
        path: String,
        /// Its sheets.
        sheets: Vec<String>,
    },
}

/// Where a formula sits, which relative tokens (`RefN`, `AreaN`) count from.
#[derive(Debug, Clone, Copy, Default)]
pub struct Base {
    /// Zero-based row.
    pub row: u16,
    /// Zero-based column.
    pub col: u16,
}

/// The formula a token stream spells, without the leading `=`.
pub fn decompile(tokens: &[u8], extra: &[u8], base: Base, context: &Context) -> Option<String> {
    Decompiler {
        tokens,
        extra,
        extra_at: 0,
        base,
        context,
        stack: Vec::new(),
    }
    .run()
}

struct Decompiler<'a> {
    tokens: &'a [u8],
    extra: &'a [u8],
    extra_at: usize,
    base: Base,
    context: &'a Context,
    stack: Vec<String>,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

impl Decompiler<'_> {
    fn run(mut self) -> Option<String> {
        let mut at = 0;
        while at < self.tokens.len() {
            at = self.token(at)?;
        }
        match self.stack.as_slice() {
            [one] => Some(one.clone()),
            _ => None,
        }
    }

    fn pop(&mut self) -> Option<String> {
        self.stack.pop()
    }

    /// Pops the last `count` fragments, in the order they were pushed.
    fn pop_many(&mut self, count: usize) -> Option<Vec<String>> {
        let start = self.stack.len().checked_sub(count)?;
        Some(self.stack.split_off(start))
    }

    fn byte(&self, at: usize) -> Option<u8> {
        self.tokens.get(at).copied()
    }

    fn word(&self, at: usize) -> Option<u16> {
        u16_at(self.tokens, at)
    }

    /// Replays the token at `at` and returns where the next one starts.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per token kind, read as a table"
    )]
    fn token(&mut self, at: usize) -> Option<usize> {
        let ptg = self.byte(at)?;
        let data = at + 1;
        // Operand tokens come in three classes (reference, value, array) that
        // differ only in the top bits; the text is the same for all three.
        let base = if ptg >= 0x20 {
            (ptg & 0x1F) | 0x20
        } else {
            ptg
        };
        let next = match base {
            0x03..=0x11 => {
                let right = self.pop()?;
                let left = self.pop()?;
                let op = [
                    "+", "-", "*", "/", "^", "&", "<", "<=", "=", ">=", ">", "<>", " ", ",", ":",
                ][usize::from(base - 0x03)];
                self.stack.push(format!("{left}{op}{right}"));
                data
            }
            0x12 | 0x13 => {
                let operand = self.pop()?;
                let op = if base == 0x12 { '+' } else { '-' };
                self.stack.push(format!("{op}{operand}"));
                data
            }
            0x14 => {
                let operand = self.pop()?;
                self.stack.push(format!("{operand}%"));
                data
            }
            0x15 => {
                let operand = self.pop()?;
                self.stack.push(format!("({operand})"));
                data
            }
            0x16 => {
                self.stack.push(String::new());
                data
            }
            0x17 => {
                let count = usize::from(self.byte(data)?);
                let wide = self.byte(data + 1)? & 1 != 0;
                let (text, end) = chars(self.tokens, data + 2, count, wide)?;
                self.stack
                    .push(format!("\"{}\"", text.replace('"', "\"\"")));
                end
            }
            0x19 => self.attribute(data)?,
            0x1C => {
                self.stack.push(error(self.byte(data)?).as_str().to_owned());
                data + 1
            }
            0x1D => {
                let text = if self.byte(data)? == 0 {
                    "FALSE"
                } else {
                    "TRUE"
                };
                self.stack.push(text.to_owned());
                data + 1
            }
            0x1E => {
                self.stack.push(self.word(data)?.to_string());
                data + 2
            }
            0x1F => {
                let bytes = self.tokens.get(data..data + 8)?;
                let value = f64::from_le_bytes(bytes.try_into().ok()?);
                self.stack.push(number(value));
                data + 8
            }
            0x20 => {
                let array = self.array()?;
                self.stack.push(array);
                data + 7
            }
            0x21 => {
                let function = biff_functions::by_index(self.word(data)?)?;
                self.call(function.name, usize::from(function.max))?;
                data + 2
            }
            0x22 => {
                let count = usize::from(self.byte(data)? & 0x7F);
                let index = self.word(data + 1)? & 0x7FFF;
                if index == 255 {
                    // A function with no number: the first argument is its
                    // name, pushed by a name token.
                    let mut args = self.pop_many(count)?;
                    if args.is_empty() {
                        return None;
                    }
                    let name = args.remove(0);
                    let name = name
                        .strip_prefix("_xlfn.")
                        .unwrap_or(&name)
                        .trim_start_matches("_xlws.")
                        .to_owned();
                    self.stack.push(format!("{name}({})", args.join(",")));
                } else {
                    let function = biff_functions::by_index(index)?;
                    self.call(function.name, count)?;
                }
                data + 3
            }
            0x23 => {
                let index = usize::from(self.word(data)?);
                let name = self.context.names.get(index.checked_sub(1)?)?.clone();
                self.stack.push(name);
                data + 4
            }
            0x24 | 0x2C => {
                let cell = self.cell(data, base == 0x2C)?;
                self.stack.push(cell);
                data + 4
            }
            0x25 | 0x2D => {
                let area = self.area(data, base == 0x2D)?;
                self.stack.push(area);
                data + 8
            }
            // A memory area's rectangles wait in the extra data; the tokens
            // that compute it follow and are replayed as usual.
            0x26 => {
                let count = usize::from(u16_at(self.extra, self.extra_at)?);
                self.extra_at += 2 + count * 8;
                data + 6
            }
            0x27 | 0x28 => data + 6,
            0x29 => data + 2,
            0x2A => {
                self.stack.push("#REF!".to_owned());
                data + 4
            }
            0x2B => {
                self.stack.push("#REF!".to_owned());
                data + 8
            }
            0x39 => {
                let name = self.external_name(self.word(data)?, self.word(data + 2)?)?;
                self.stack.push(name);
                data + 6
            }
            0x3A => {
                let sheet = self.sheet(self.word(data)?)?;
                let cell = self.cell(data + 2, false)?;
                self.stack.push(format!("{sheet}{cell}"));
                data + 6
            }
            0x3B => {
                let sheet = self.sheet(self.word(data)?)?;
                let area = self.area(data + 2, false)?;
                self.stack.push(format!("{sheet}{area}"));
                data + 10
            }
            0x3C => {
                let sheet = self.sheet(self.word(data)?)?;
                self.stack.push(format!("{sheet}#REF!"));
                data + 6
            }
            0x3D => {
                let sheet = self.sheet(self.word(data)?)?;
                self.stack.push(format!("{sheet}#REF!"));
                data + 10
            }
            // `Exp` and `Tbl` point at a shared, array or table formula; the
            // caller resolves those before coming here. Everything else is a
            // token this module does not read.
            _ => return None,
        };
        Some(next)
    }

    /// Pops a call's arguments and pushes the call.
    fn call(&mut self, name: &str, count: usize) -> Option<()> {
        let args = self.pop_many(count)?;
        self.stack.push(format!("{name}({})", args.join(",")));
        Some(())
    }

    /// The `Attr` token. Most of its kinds are hints for the calculator
    /// (volatile, jump offsets for `IF` and `CHOOSE`, spaces); one of them is
    /// a whole function: `SUM` with a single argument.
    fn attribute(&mut self, data: usize) -> Option<usize> {
        let kind = self.byte(data)?;
        let value = usize::from(self.word(data + 1)?);
        if kind & 0x10 != 0 {
            let operand = self.pop()?;
            self.stack.push(format!("SUM({operand})"));
        }
        // `CHOOSE` carries a jump table: one offset per choice, plus one.
        let table = if kind & 0x04 != 0 { (value + 1) * 2 } else { 0 };
        Some(data + 3 + table)
    }

    /// An array constant, read from the extra data.
    fn array(&mut self) -> Option<String> {
        let extra = self.extra;
        let at = self.extra_at;
        let cols = usize::from(*extra.get(at)?) + 1;
        let rows = usize::from(u16_at(extra, at + 1)?) + 1;
        let mut pos = at + 3;
        let mut lines = Vec::with_capacity(rows.min(1024));
        for _ in 0..rows {
            let mut line = Vec::with_capacity(cols.min(1024));
            for _ in 0..cols {
                let kind = *extra.get(pos)?;
                pos += 1;
                let value = match kind {
                    0x00 => {
                        pos += 8;
                        String::new()
                    }
                    0x01 => {
                        let value = f64::from_le_bytes(extra.get(pos..pos + 8)?.try_into().ok()?);
                        pos += 8;
                        number(value)
                    }
                    0x02 => {
                        let count = usize::from(u16_at(extra, pos)?);
                        let wide = *extra.get(pos + 2)? & 1 != 0;
                        let (text, end) = chars(extra, pos + 3, count, wide)?;
                        pos = end;
                        format!("\"{}\"", text.replace('"', "\"\""))
                    }
                    0x04 => {
                        let value = *extra.get(pos)?;
                        pos += 8;
                        if value == 0 { "FALSE" } else { "TRUE" }.to_owned()
                    }
                    0x10 => {
                        let code = *extra.get(pos)?;
                        pos += 8;
                        error(code).as_str().to_owned()
                    }
                    _ => return None,
                };
                line.push(value);
            }
            lines.push(line.join(","));
        }
        self.extra_at = pos;
        Some(format!("{{{}}}", lines.join(";")))
    }

    /// A row and a column with their relative flags, as `$A$1`.
    ///
    /// In a relative token (`RefN`, `AreaN`) a relative row or column is an
    /// offset from the base cell rather than a position.
    fn position(&self, row: u16, col_field: u16, relative: bool) -> Option<(String, String)> {
        let col_relative = col_field & 0x4000 != 0;
        let row_relative = col_field & 0x8000 != 0;
        let col = col_field & 0x00FF;
        let (row, col) = if relative {
            let row = if row_relative {
                #[expect(clippy::cast_possible_wrap, reason = "the field is a signed offset")]
                self.base.row.wrapping_add_signed(row as i16)
            } else {
                row
            };
            let col = if col_relative {
                #[expect(clippy::cast_possible_wrap, reason = "the low byte is a signed offset")]
                let offset = i16::from(col as u8 as i8);
                self.base.col.wrapping_add_signed(offset) & 0x00FF
            } else {
                col
            };
            (row, col)
        } else {
            (row, col)
        };
        let letters = Col::new(u32::from(col))?.to_letters();
        let dollar = |relative: bool| if relative { "" } else { "$" };
        Some((
            format!("{}{letters}", dollar(col_relative)),
            format!("{}{}", dollar(row_relative), u32::from(row) + 1),
        ))
    }

    fn cell(&self, at: usize, relative: bool) -> Option<String> {
        let (col, row) = self.position(self.word(at)?, self.word(at + 2)?, relative)?;
        Some(format!("{col}{row}"))
    }

    /// An area, written as whole columns or whole rows where it spans the
    /// sheet, the way Excel shows `A:A` and `1:1`.
    fn area(&self, at: usize, relative: bool) -> Option<String> {
        let (first_row, last_row) = (self.word(at)?, self.word(at + 2)?);
        let (first_col, last_col) = (self.word(at + 4)?, self.word(at + 6)?);
        let (col1, row1) = self.position(first_row, first_col, relative)?;
        let (col2, row2) = self.position(last_row, last_col, relative)?;
        let whole_columns = first_row == 0 && last_row == 0xFFFF;
        let whole_rows = first_col.trailing_zeros() >= 8 && last_col & 0xFF == 0xFF;
        Some(if whole_columns && !relative {
            format!("{col1}:{col2}")
        } else if whole_rows && !relative {
            format!("{row1}:{row2}")
        } else {
            format!("{col1}{row1}:{col2}{row2}")
        })
    }

    /// The `Sheet!` prefix an `EXTERNSHEET` entry stands for.
    fn sheet(&self, index: u16) -> Option<String> {
        let &(book, first, last) = self.context.externs.get(usize::from(index))?;
        let book = self.context.books.get(usize::from(book))?;
        let names: &[String] = match &book.kind {
            BookKind::Internal => &self.context.sheets,
            BookKind::External { sheets, .. } => sheets,
            BookKind::AddIn => return None,
        };
        // 0xFFFF marks a sheet that was deleted, 0xFFFE the workbook itself.
        if first == 0xFFFF {
            return Some("#REF!".to_owned());
        }
        let first_name = names.get(usize::from(first))?;
        let mut name = first_name.clone();
        if last != first {
            name = format!("{name}:{}", names.get(usize::from(last))?);
        }
        let prefix = match &book.kind {
            BookKind::External { path, .. } => format!("[{path}]{name}"),
            _ => name,
        };
        Some(format!("{}!", quote(&prefix)))
    }

    /// A name from another book or an add-in.
    fn external_name(&self, index: u16, name: u16) -> Option<String> {
        let &(book, _, _) = self.context.externs.get(usize::from(index))?;
        let book = self.context.books.get(usize::from(book))?;
        let slot = usize::from(name).checked_sub(1)?;
        match &book.kind {
            BookKind::Internal => self.context.names.get(slot).cloned(),
            BookKind::AddIn => book.names.get(slot).cloned(),
            BookKind::External { path, .. } => {
                let name = book.names.get(slot)?;
                Some(format!("{}!{name}", quote(&format!("[{path}]"))))
            }
        }
    }
}

/// A sheet prefix, quoted unless it is a plain word that cannot be misread as
/// a reference.
pub fn quote(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        && crate::CellRef::parse(name).is_err()
        && !looks_like_r1c1(name);
    if plain {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// Whether a name reads as an R1C1 reference (`R1C1`, `R`, `C5`), which Excel
/// quotes as well.
fn looks_like_r1c1(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let rest = upper.strip_prefix('R').unwrap_or(&upper);
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = rest.strip_prefix('C').unwrap_or(rest);
    rest.trim_start_matches(|c: char| c.is_ascii_digit())
        .is_empty()
}

/// A number as a formula writes it: the shortest text that reads back as the
/// same double.
fn number(value: f64) -> String {
    format!("{value}")
}

/// Characters of a string token, one byte each or two. Returns the text and
/// where it ends.
fn chars(data: &[u8], at: usize, count: usize, wide: bool) -> Option<(String, usize)> {
    let size = if wide { 2 } else { 1 };
    let end = at.checked_add(count.checked_mul(size)?)?;
    let bytes = data.get(at..end)?;
    let text = if wide {
        char::decode_utf16(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair)),
        )
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
    } else {
        bytes.iter().map(|&b| char::from(b)).collect()
    };
    Some((text, end))
}

/// An error code as BIFF numbers them.
pub fn error(code: u8) -> CellError {
    match code {
        0x00 => CellError::Null,
        0x07 => CellError::Div0,
        0x0F => CellError::Value,
        0x17 => CellError::Ref,
        0x1D => CellError::Name,
        0x24 => CellError::Num,
        _ => CellError::Na,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        Context {
            sheets: vec!["Main".into(), "Data 2".into(), "R1C1".into()],
            externs: vec![(0, 1, 1), (0, 0, 2), (1, 0, 0), (0, 0xFFFF, 0xFFFF)],
            books: vec![
                Book::default(),
                Book {
                    kind: BookKind::AddIn,
                    names: vec!["EOMONTH".into()],
                },
            ],
            names: vec!["Rate".into()],
        }
    }

    fn text(tokens: &[u8], extra: &[u8], base: Base) -> Option<String> {
        decompile(tokens, extra, base, &context())
    }

    #[test]
    fn relative_tokens_count_from_the_cell_reading_them() {
        // RefN: row offset -1, column offset +1, both relative, read from C5.
        let base = Base { row: 4, col: 2 };
        let tokens = [0x4C, 0xFF, 0xFF, 0x01, 0xC0];
        assert_eq!(text(&tokens, &[], base).as_deref(), Some("D4"));
        // AreaN: absolute row 0 to relative +0, columns relative -2 to 0.
        let tokens = [0x2D, 0x00, 0x00, 0x00, 0x00, 0xFE, 0x40, 0x00, 0xC0];
        assert_eq!(text(&tokens, &[], base).as_deref(), Some("A$1:C5"));
    }

    #[test]
    fn whole_columns_and_rows_read_as_excel_shows_them() {
        let column = [0x25, 0x00, 0x00, 0xFF, 0xFF, 0x00, 0xC0, 0x01, 0xC0];
        assert_eq!(text(&column, &[], Base::default()).as_deref(), Some("A:B"));
        let row = [0x25, 0x02, 0x00, 0x02, 0x00, 0x00, 0x00, 0xFF, 0x00];
        assert_eq!(text(&row, &[], Base::default()).as_deref(), Some("$3:$3"));
    }

    #[test]
    fn three_d_references_quote_what_needs_quoting() {
        // Ref3d through entry 0 (sheet "Data 2"), then Area3d over all three.
        let tokens = [0x3A, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0];
        assert_eq!(
            text(&tokens, &[], Base::default()).as_deref(),
            Some("'Data 2'!A1")
        );
        let tokens = [
            0x3B, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(
            text(&tokens, &[], Base::default()).as_deref(),
            Some("'Main:R1C1'!$A$1:$A$2")
        );
        // A sheet that was deleted, written the way Excel writes it.
        let tokens = [0x3A, 0x03, 0x00, 0x00, 0x00, 0x00, 0xC0];
        assert_eq!(
            text(&tokens, &[], Base::default()).as_deref(),
            Some("#REF!A1")
        );
        assert_eq!(quote("R1C1"), "'R1C1'", "reads as a reference");
        assert_eq!(quote("B2"), "'B2'");
        assert_eq!(quote("Sales_2024"), "Sales_2024");
    }

    #[test]
    fn an_array_constant_comes_from_the_extra_data() {
        let tokens = [0x60, 0, 0, 0, 0, 0, 0, 0];
        let mut extra = vec![1, 1, 0];
        extra.push(0x01);
        extra.extend_from_slice(&1.5f64.to_le_bytes());
        extra.extend_from_slice(&[0x02, 2, 0, 0, b'a', b'"']);
        extra.extend_from_slice(&[0x04, 1, 0, 0, 0, 0, 0, 0, 0]);
        extra.extend_from_slice(&[0x10, 0x07, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            text(&tokens, &extra, Base::default()).as_deref(),
            Some("{1.5,\"a\"\"\";TRUE,#DIV/0!}")
        );
    }

    #[test]
    fn attributes_that_are_functions_and_ones_that_are_hints() {
        // Int 2, Str "a", Str "b", then CHOOSE's jump table and the call.
        let tokens = [
            0x1E, 0x02, 0x00, 0x19, 0x04, 0x01, 0x00, 0x04, 0x00, 0x0A, 0x00, 0x17, 0x01, 0x00,
            b'a', 0x19, 0x08, 0x05, 0x00, 0x17, 0x01, 0x00, b'b', 0x19, 0x08, 0x00, 0x00, 0x22,
            0x03, 0x64, 0x00,
        ];
        assert_eq!(
            text(&tokens, &[], Base::default()).as_deref(),
            Some("CHOOSE(2,\"a\",\"b\")")
        );
        // A1:A3 followed by the one-argument SUM attribute.
        let tokens = [
            0x25, 0x00, 0x00, 0x02, 0x00, 0x00, 0xC0, 0x00, 0xC0, 0x19, 0x10, 0x00, 0x00,
        ];
        assert_eq!(
            text(&tokens, &[], Base::default()).as_deref(),
            Some("SUM(A1:A3)")
        );
    }

    #[test]
    fn functions_without_a_number_take_their_name_from_the_first_argument() {
        // NameX through entry 2 (the add-in), name 1; then Int 0 and the call.
        let tokens = [
            0x39, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x44, 0x00, 0x00, 0x00, 0xC0, 0x1E, 0x00,
            0x00, 0x42, 0x03, 0xFF, 0x00,
        ];
        assert_eq!(
            text(&tokens, &[], Base::default()).as_deref(),
            Some("EOMONTH(A1,0)")
        );
        // A defined name and a missing argument.
        let tokens = [0x43, 0x01, 0x00, 0x00, 0x00, 0x16, 0x42, 0x02, 0x01, 0x00];
        assert_eq!(
            text(&tokens, &[], Base::default()).as_deref(),
            Some("IF(Rate,)")
        );
    }

    #[test]
    fn broken_streams_give_nothing_rather_than_a_wrong_formula() {
        assert_eq!(
            text(&[0x03], &[], Base::default()),
            None,
            "an operator alone"
        );
        assert_eq!(text(&[0x1E, 0x01], &[], Base::default()), None, "cut short");
        assert_eq!(
            text(&[0x02, 0, 0, 0, 0], &[], Base::default()),
            None,
            "a table"
        );
        assert_eq!(
            text(&[0x1E, 1, 0, 0x1E, 2, 0], &[], Base::default()),
            None,
            "two values"
        );
    }
}
