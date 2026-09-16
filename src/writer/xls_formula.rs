//! Compiling a formula into BIFF8 tokens: the mirror of
//! `reader::xls_formula`.
//!
//! The parsed tree is walked in post-order, each node leaving its tokens
//! after its operands'. Three things the tree does not say have to be put
//! back on the way:
//!
//! - **Parentheses.** The parser drops them, but Excel shows a formula from
//!   its tokens, and `(A1+B1)*2` without a parenthesis token shows as
//!   `A1+B1*2`. A parenthesis is emitted wherever operator precedence needs
//!   one.
//! - **Operand classes.** Every reference and function token says whether it
//!   is wanted as a reference, a value or an array. `SUM(A1:A3)` needs the
//!   area as a reference, `A1:A3+1` as a value. The function table says what
//!   each argument expects.
//! - **What the globals must declare.** A 3D reference goes through an
//!   `EXTERNSHEET` entry, a function with no number through an add-in name, a
//!   defined name through its `NAME` record; [`Links`] collects them while
//!   formulas compile, and the writer lays them out afterwards.
//!
//! A formula that cannot be stored - a structured reference, a call on a
//! computed function, a reference past row 65536 or column IV, a text longer
//! than 255 characters - gives `None`, and the cell is written as its value.

use crate::coordinate::{MAX_COL, MAX_ROW};
use crate::formula::parser::{Anchors, BinaryOp, Expr, UnaryOp};
use crate::shared::biff_functions;
use crate::{CellError, Range};

/// What the compiled formulas refer to outside themselves.
#[derive(Debug, Default)]
pub struct Links {
    /// Sheet names, in tab order.
    sheets: Vec<String>,
    /// Defined names in `NAME` record order: the name and the sheet it
    /// belongs to.
    names: Vec<(String, Option<usize>)>,
    /// `EXTERNSHEET` entries: book (0 this one, 1 the add-ins), first and
    /// last sheet.
    pub externs: Vec<(u16, u16, u16)>,
    /// Function names stored through the add-in book, in `EXTERNNAME` order.
    pub add_ins: Vec<String>,
}

/// The book index of this workbook in the `SUPBOOK` list.
const OWN_BOOK: u16 = 0;
/// The book index of the add-in `SUPBOOK`, which comes right after.
const ADD_IN_BOOK: u16 = 1;
/// The sheet number an `EXTERNSHEET` entry uses for "the book, no sheet".
const NO_SHEET: u16 = 0xFFFE;

impl Links {
    /// Links for a workbook with these sheets and defined names.
    #[must_use]
    pub fn new(sheets: Vec<String>, names: Vec<(String, Option<usize>)>) -> Self {
        Self {
            sheets,
            names,
            externs: Vec::new(),
            add_ins: Vec::new(),
        }
    }

    fn extern_entry(&mut self, entry: (u16, u16, u16)) -> Option<u16> {
        let index = self
            .externs
            .iter()
            .position(|e| *e == entry)
            .unwrap_or_else(|| {
                self.externs.push(entry);
                self.externs.len() - 1
            });
        u16::try_from(index).ok()
    }

    /// The `EXTERNSHEET` entry for `Sheet!` or `First:Last!`.
    fn sheet_entry(&mut self, qualifier: &str) -> Option<u16> {
        let (first, last) = qualifier.split_once(':').unwrap_or((qualifier, qualifier));
        let first = self.sheet_index(first)?;
        let last = self.sheet_index(last)?;
        self.extern_entry((OWN_BOOK, first, last))
    }

    fn sheet_index(&self, name: &str) -> Option<u16> {
        let name = name.trim_matches('\'').replace("''", "'");
        // Sheet names compare without case, Cyrillic included.
        let name = name.to_lowercase();
        let index = self.sheets.iter().position(|s| s.to_lowercase() == name)?;
        u16::try_from(index).ok()
    }

    /// The `EXTERNSHEET` entry and the one-based `EXTERNNAME` number of a
    /// function stored by name.
    fn add_in(&mut self, name: &str) -> Option<(u16, u16)> {
        let position = self
            .add_ins
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| {
                self.add_ins.push(name.to_owned());
                self.add_ins.len() - 1
            });
        let entry = self.extern_entry((ADD_IN_BOOK, NO_SHEET, NO_SHEET))?;
        Some((entry, u16::try_from(position + 1).ok()?))
    }

    /// The one-based `NAME` number a name has from a sheet: the sheet's own
    /// name wins over the workbook's.
    fn name(&self, name: &str, sheet: Option<usize>) -> Option<u16> {
        let matches = |(n, _): &&(String, Option<usize>)| n.eq_ignore_ascii_case(name);
        let local = self
            .names
            .iter()
            .position(|entry| matches(&entry) && entry.1.is_some() && entry.1 == sheet);
        let global = || {
            self.names
                .iter()
                .position(|entry| matches(&entry) && entry.1.is_none())
        };
        u16::try_from(local.or_else(global)? + 1).ok()
    }
}

/// Where a formula lives: in a cell of a sheet, or in a defined name, which
/// has no cell and so stores every reference with its sheet.
#[derive(Debug, Clone, Copy)]
pub enum Place {
    /// A cell on the sheet with this index.
    Cell(usize),
    /// A defined name, belonging to a sheet or to the whole workbook.
    Name(Option<usize>),
}

/// The tokens of a formula and the extra data its array constants need.
#[derive(Debug, Default)]
pub struct Compiled {
    /// The token stream, `rgce`.
    pub tokens: Vec<u8>,
    /// What follows it, `rgbExtra`.
    pub extra: Vec<u8>,
}

/// Compiles a parsed formula, or gives `None` when BIFF8 cannot hold it.
pub fn compile(expr: &Expr, place: Place, links: &mut Links) -> Option<Compiled> {
    let mut compiler = Compiler {
        out: Compiled::default(),
        place,
        links,
    };
    compiler.expr(expr, Class::Value)?;
    u16::try_from(compiler.out.tokens.len())
        .is_ok()
        .then_some(compiler.out)
}

/// What an operand is wanted as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Reference,
    Value,
    Array,
}

impl Class {
    /// The amount an operand token's number grows by in this class.
    const fn delta(self) -> u8 {
        match self {
            Self::Reference => 0x00,
            Self::Value => 0x20,
            Self::Array => 0x40,
        }
    }

    fn of(letter: u8) -> Self {
        match letter {
            b'R' => Self::Reference,
            b'A' => Self::Array,
            _ => Self::Value,
        }
    }
}

struct Compiler<'a> {
    out: Compiled,
    place: Place,
    links: &'a mut Links,
}

/// How tightly an operator binds, as the parser has it.
const fn precedence(expr: &Expr) -> u8 {
    match expr {
        Expr::Binary(op, ..) => match op {
            BinaryOp::Span => 90,
            BinaryOp::Intersect => 80,
            BinaryOp::Union => 70,
            BinaryOp::Pow => 40,
            BinaryOp::Mul | BinaryOp::Div => 30,
            BinaryOp::Add | BinaryOp::Sub => 20,
            BinaryOp::Concat => 10,
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => 5,
        },
        Expr::Unary(UnaryOp::Percent, _) => 50,
        Expr::Unary(..) => 60,
        _ => 100,
    }
}

impl Compiler<'_> {
    fn push(&mut self, bytes: &[u8]) {
        self.out.tokens.extend_from_slice(bytes);
    }

    /// An operand, in parentheses when it binds looser than it has to.
    fn operand(&mut self, expr: &Expr, class: Class, needs: bool) -> Option<()> {
        self.expr(expr, class)?;
        if needs {
            self.push(&[0x15]);
        }
        Some(())
    }

    fn expr(&mut self, expr: &Expr, class: Class) -> Option<()> {
        match expr {
            Expr::Number(n) => self.number(*n),
            Expr::Text(text) => {
                let text = short_text(text)?;
                self.push(&[0x17]);
                self.push(&text);
            }
            Expr::Bool(b) => self.push(&[0x1D, u8::from(*b)]),
            Expr::Error(e) => self.push(&[0x1C, error_code(*e)]),
            Expr::Missing => self.push(&[0x16]),
            Expr::Range {
                sheet,
                range,
                anchors,
            } => self.range(sheet.as_deref(), *range, *anchors, class)?,
            Expr::Name(name) => {
                let sheet = match self.place {
                    Place::Cell(sheet) => Some(sheet),
                    Place::Name(sheet) => sheet,
                };
                let index = self.links.name(name, sheet)?;
                self.push(&[0x23 + class.delta()]);
                self.push(&index.to_le_bytes());
                self.push(&[0, 0]);
            }
            Expr::Unary(op, inner) => {
                let needs = precedence(inner) < precedence(expr);
                self.operand(inner, Class::Value, needs)?;
                self.push(&[match op {
                    UnaryOp::Plus => 0x12,
                    UnaryOp::Neg => 0x13,
                    UnaryOp::Percent => 0x14,
                }]);
            }
            Expr::Binary(op, left, right) => {
                let own = precedence(expr);
                let operands = match op {
                    BinaryOp::Span | BinaryOp::Intersect | BinaryOp::Union => Class::Reference,
                    _ => Class::Value,
                };
                // Operators group to the left, so an equal operator on the
                // right needs parentheses and one on the left does not.
                self.operand(left, operands, precedence(left) < own)?;
                self.operand(right, operands, precedence(right) <= own)?;
                self.push(&[match op {
                    BinaryOp::Add => 0x03,
                    BinaryOp::Sub => 0x04,
                    BinaryOp::Mul => 0x05,
                    BinaryOp::Div => 0x06,
                    BinaryOp::Pow => 0x07,
                    BinaryOp::Concat => 0x08,
                    BinaryOp::Lt => 0x09,
                    BinaryOp::Le => 0x0A,
                    BinaryOp::Eq => 0x0B,
                    BinaryOp::Ge => 0x0C,
                    BinaryOp::Gt => 0x0D,
                    BinaryOp::Ne => 0x0E,
                    BinaryOp::Intersect => 0x0F,
                    BinaryOp::Union => 0x10,
                    BinaryOp::Span => 0x11,
                }]);
            }
            Expr::Call { name, args } => self.call(name, args, class)?,
            Expr::Array(rows) => self.array(rows)?,
            Expr::Structured(_) | Expr::Apply { .. } => return None,
        }
        Some(())
    }

    fn number(&mut self, n: f64) {
        if n.fract() == 0.0 && (0.0..=65535.0).contains(&n) {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked to be a whole number in range"
            )]
            let int = n as u16;
            self.push(&[0x1E]);
            self.push(&int.to_le_bytes());
        } else {
            self.push(&[0x1F]);
            self.push(&n.to_le_bytes());
        }
    }

    /// A cell or an area, on the formula's own sheet or through `EXTERNSHEET`.
    fn range(
        &mut self,
        sheet: Option<&str>,
        range: Range,
        anchors: Anchors,
        class: Class,
    ) -> Option<()> {
        let sheet_entry = match (sheet, self.place) {
            (Some(qualifier), _) => Some(self.links.sheet_entry(qualifier)?),
            (None, Place::Cell(_)) => None,
            // A name has no sheet of its own to fall back on, unless it
            // belongs to one.
            (None, Place::Name(Some(index))) => {
                let first = u16::try_from(index).ok()?;
                Some(self.links.extern_entry((OWN_BOOK, first, first))?)
            }
            (None, Place::Name(None)) => return None,
        };
        let (row1, row2) = (range.start.row.index(), range.end.row.index());
        let (col1, col2) = (range.start.col.index(), range.end.col.index());
        // A whole column in the model runs to row 1048576; in BIFF8 to 65536.
        let row2 = if row1 == 0 && row2 == MAX_ROW - 1 {
            0xFFFF
        } else {
            row2
        };
        let col2 = if col1 == 0 && col2 == MAX_COL - 1 {
            0xFF
        } else {
            col2
        };
        let row = |value: u32| u16::try_from(value).ok();
        let col = |value: u32, absolute_col: bool, absolute_row: bool| {
            let mut field = u16::try_from(value).ok().filter(|&c| c <= 0xFF)?;
            if !absolute_col {
                field |= 0x4000;
            }
            if !absolute_row {
                field |= 0x8000;
            }
            Some(field)
        };
        let single = range.start == range.end;
        let base = match (single, sheet_entry.is_some()) {
            (true, false) => 0x24,
            (false, false) => 0x25,
            (true, true) => 0x3A,
            (false, true) => 0x3B,
        };
        self.push(&[base + class.delta()]);
        if let Some(entry) = sheet_entry {
            self.push(&entry.to_le_bytes());
        }
        if single {
            self.push(&row(row1)?.to_le_bytes());
            self.push(&col(col1, anchors.start_col, anchors.start_row)?.to_le_bytes());
        } else {
            self.push(&row(row1)?.to_le_bytes());
            self.push(&row(row2)?.to_le_bytes());
            self.push(&col(col1, anchors.start_col, anchors.start_row)?.to_le_bytes());
            self.push(&col(col2, anchors.end_col, anchors.end_row)?.to_le_bytes());
        }
        Some(())
    }

    fn call(&mut self, name: &str, args: &[Expr], class: Class) -> Option<()> {
        let count = u8::try_from(args.len()).ok()?;
        let Some(function) = biff_functions::by_name(name) else {
            return self.call_by_name(name, args, count, class);
        };
        if count < function.min || count > function.max {
            return None;
        }
        match function.name {
            "IF" if count == 3 => return self.if_call(function, args, class),
            "CHOOSE" => return self.choose_call(function, args, class),
            _ => {}
        }
        for (position, arg) in args.iter().enumerate() {
            self.argument(arg, Class::of(function.arg_class(position)))?;
        }
        let delta = result_class(function.returns, class).delta();
        if function.is_variadic() {
            self.push(&[0x22 + delta, count]);
        } else {
            self.push(&[0x21 + delta]);
        }
        self.push(&function.index.to_le_bytes());
        Some(())
    }

    /// An argument. A union needs parentheses there, or its comma would read
    /// as the next argument.
    fn argument(&mut self, arg: &Expr, class: Class) -> Option<()> {
        let union = matches!(arg, Expr::Binary(BinaryOp::Union, ..));
        self.operand(arg, class, union)
    }

    /// A function with no number: an add-in function, or one newer than the
    /// format, stored as a call to function 255 whose first operand names it.
    fn call_by_name(&mut self, name: &str, args: &[Expr], count: u8, class: Class) -> Option<()> {
        let stored = if biff_functions::ADD_IN.contains(&name) {
            name.to_owned()
        } else {
            format!("_xlfn.{name}")
        };
        let (entry, index) = self.links.add_in(&stored)?;
        self.push(&[0x39]);
        self.push(&entry.to_le_bytes());
        self.push(&index.to_le_bytes());
        self.push(&[0, 0]);
        for arg in args {
            self.argument(arg, Class::Reference)?;
        }
        let delta = result_class(b'V', class).delta();
        self.push(&[0x22 + delta, count.checked_add(1)?, 0xFF, 0x00]);
        Some(())
    }

    /// `IF` with both branches, with the jump hints Excel writes: after the
    /// condition, where the false branch starts; after each branch, where the
    /// call is.
    fn if_call(
        &mut self,
        function: &biff_functions::Function,
        args: &[Expr],
        class: Class,
    ) -> Option<()> {
        let [condition, yes, no] = args else {
            return None;
        };
        let branch = Class::of(function.arg_class(1));
        self.argument(condition, Class::Value)?;
        self.push(&[0x19, 0x02, 0, 0]);
        let if_at = self.out.tokens.len() - 2;
        self.argument(yes, branch)?;
        self.push(&[0x19, 0x08, 0, 0]);
        let skip_at = self.out.tokens.len() - 2;
        let offset = u16::try_from(skip_at - if_at).ok()?;
        self.out.tokens[if_at..if_at + 2].copy_from_slice(&offset.to_le_bytes());
        self.argument(no, branch)?;
        self.push(&[0x19, 0x08, 3, 0]);
        let delta = result_class(function.returns, class).delta();
        self.push(&[0x22 + delta, 3]);
        self.push(&function.index.to_le_bytes());
        let end = self.out.tokens.len();
        let offset = u16::try_from(end - (skip_at + 2) - 1).ok()?;
        self.out.tokens[skip_at..skip_at + 2].copy_from_slice(&offset.to_le_bytes());
        Some(())
    }

    /// `CHOOSE` with its jump table: one offset per choice to where it
    /// starts, and a skip after each to the call. The arithmetic is the one
    /// `xlwt` writes and Excel reads.
    fn choose_call(
        &mut self,
        function: &biff_functions::Function,
        args: &[Expr],
        class: Class,
    ) -> Option<()> {
        let (index, choices) = args.split_first()?;
        self.argument(index, Class::Value)?;
        let mut chunks = Vec::with_capacity(choices.len());
        for choice in choices {
            let start = self.out.tokens.len();
            self.argument(choice, Class::of(function.arg_class(1)))?;
            chunks.push(self.out.tokens.split_off(start));
        }
        let count = chunks.len();
        let mut skips = vec![0usize; count];
        if let Some(last) = skips.last_mut() {
            *last = 3;
        }
        for i in (1..count).rev() {
            skips[i - 1] = skips[i] + chunks[i].len() + 4;
        }
        let mut jumps = vec![2 * count + 2];
        for chunk in &chunks {
            let last = *jumps.last()?;
            jumps.push(last + chunk.len() + 4);
        }
        self.push(&[0x19, 0x04]);
        self.push(&u16::try_from(count).ok()?.to_le_bytes());
        for jump in jumps {
            self.push(&u16::try_from(jump).ok()?.to_le_bytes());
        }
        for (chunk, skip) in chunks.iter().zip(skips) {
            self.push(chunk);
            self.push(&[0x19, 0x08]);
            self.push(&u16::try_from(skip).ok()?.to_le_bytes());
        }
        let delta = result_class(function.returns, class).delta();
        self.push(&[0x22 + delta, u8::try_from(count + 1).ok()?]);
        self.push(&function.index.to_le_bytes());
        Some(())
    }

    /// An array constant: a token, and the values in the extra data.
    fn array(&mut self, rows: &[Vec<Expr>]) -> Option<()> {
        let cols = rows.first()?.len();
        if cols == 0 || rows.iter().any(|row| row.len() != cols) {
            return None;
        }
        self.push(&[0x60, 0, 0, 0, 0, 0, 0, 0]);
        let extra = &mut self.out.extra;
        extra.push(u8::try_from(cols - 1).ok()?);
        extra.extend_from_slice(&u16::try_from(rows.len() - 1).ok()?.to_le_bytes());
        for value in rows.iter().flatten() {
            match value {
                Expr::Number(n) => {
                    extra.push(0x01);
                    extra.extend_from_slice(&n.to_le_bytes());
                }
                Expr::Unary(UnaryOp::Neg, inner) => match **inner {
                    Expr::Number(n) => {
                        extra.push(0x01);
                        extra.extend_from_slice(&(-n).to_le_bytes());
                    }
                    _ => return None,
                },
                Expr::Text(text) => {
                    let units: Vec<u16> = text.encode_utf16().collect();
                    extra.push(0x02);
                    extra.extend_from_slice(&u16::try_from(units.len()).ok()?.to_le_bytes());
                    extra.push(0x01);
                    for unit in units {
                        extra.extend_from_slice(&unit.to_le_bytes());
                    }
                }
                Expr::Bool(b) => {
                    extra.extend_from_slice(&[0x04, u8::from(*b), 0, 0, 0, 0, 0, 0, 0]);
                }
                Expr::Error(e) => {
                    extra.extend_from_slice(&[0x10, error_code(*e), 0, 0, 0, 0, 0, 0, 0]);
                }
                _ => return None,
            }
        }
        Some(())
    }
}

/// The class a function token takes where its result is wanted as `wanted`.
const fn result_class(returns: u8, wanted: Class) -> Class {
    match (returns, wanted) {
        (b'R', Class::Reference) => Class::Reference,
        (b'A', _) | (_, Class::Array) => Class::Array,
        _ => Class::Value,
    }
}

/// A string token's body: a one-byte count, a flags byte, the characters.
fn short_text(text: &str) -> Option<Vec<u8>> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let count = u8::try_from(units.len()).ok()?;
    let wide = units.iter().any(|&u| u > 0xFF);
    let mut out = vec![count, u8::from(wide)];
    for unit in units {
        if wide {
            out.extend_from_slice(&unit.to_le_bytes());
        } else {
            out.push(u8::try_from(unit).ok()?);
        }
    }
    Some(out)
}

/// An error code, as BIFF numbers them.
pub const fn error_code(error: CellError) -> u8 {
    match error {
        CellError::Null => 0x00,
        CellError::Div0 => 0x07,
        CellError::Value => 0x0F,
        CellError::Ref => 0x17,
        CellError::Name => 0x1D,
        CellError::Num => 0x24,
        CellError::Na | CellError::Calc => 0x2A,
    }
}
