//! Formula text to a syntax tree.
//!
//!
//! A shunting-yard parser produces a stack in reverse Polish notation, walked with a
//! second stack at evaluation time. This builds an [`Expr`] tree instead: the
//! evaluator then matches on it, which is what makes lazy arguments (`IF`,
//! `IFERROR`) and reference-taking functions (`ROW`, `OFFSET`) ordinary code
//! rather than special cases threaded through a stack machine.

use crate::coordinate::{CellRef, Range};
use crate::error::{CellError, Error, Result};

/// A parsed formula.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A number literal.
    Number(f64),
    /// A text literal, already unescaped.
    Text(String),
    /// `TRUE` or `FALSE`.
    Bool(bool),
    /// An error literal such as `#N/A`.
    Error(CellError),
    /// A cell or a rectangle of cells.
    Range {
        /// The sheet it points at; `None` means the formula's own sheet.
        sheet: Option<String>,
        /// The cells.
        range: Range,
    },
    /// A defined name.
    Name(String),
    /// An argument left out, as the middle one in `IF(A1,,0)`.
    Missing,
    /// A prefix or postfix operator.
    Unary(UnaryOp, Box<Expr>),
    /// An operator between two operands.
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// A function call.
    Call {
        /// Function name, upper-cased.
        name: String,
        /// Arguments in source order.
        args: Vec<Expr>,
    },
    /// An array constant: rows of literals, as `{1,2;3,4}`.
    Array(Vec<Vec<Expr>>),
    /// A structured reference into a table, as `Sales[Amount]`.
    Structured(Structured),
    /// A call on something other than a name, as `LAMBDA(x,x+1)(5)`: the
    /// callee is computed first and has to answer with a function.
    Apply {
        /// What is being called.
        callee: Box<Expr>,
        /// Arguments in source order.
        args: Vec<Expr>,
    },
}

/// A reference written in terms of a table rather than of cells, as
/// `Sales[Amount]` or `Sales[[#Totals],[Amount]]`.
///
/// It says nothing about where the cells are: the table names them, and the
/// same reference follows the table when rows are inserted into it. Which
/// rectangle it means is worked out at evaluation, where the workbook is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structured {
    /// The table. `None` when the formula sits inside the table itself, which
    /// is how Excel writes a reference from a calculated column.
    pub table: Option<String>,
    /// Which rows of the table are meant.
    pub part: TablePart,
    /// The first column named, and the last when a span was written.
    pub columns: Option<(String, Option<String>)>,
}

/// Which rows of a table a structured reference covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TablePart {
    /// The body, which is what a reference means when it says nothing.
    #[default]
    Data,
    /// Header, body and totals together.
    All,
    /// The header row.
    Headers,
    /// The totals row.
    Totals,
    /// Headers and body, which is `#All` without the totals.
    HeadersData,
    /// Body and totals.
    DataTotals,
    /// The row the formula itself sits on, written `@` or `#This Row`.
    ThisRow,
}

impl TablePart {
    /// Folds a second item specifier into the first, which is how
    /// `[[#Headers],[#Data],[Col]]` says "everything but the totals".
    fn with(self, other: Self) -> Self {
        match (self, other) {
            (Self::Headers, Self::Data) | (Self::Data, Self::Headers) => Self::HeadersData,
            (Self::Totals, Self::Data) | (Self::Data, Self::Totals) => Self::DataTotals,
            (Self::Headers, Self::Totals) | (Self::Totals, Self::Headers) => Self::All,
            _ => other,
        }
    }

    /// The specifier by name, without its `#`.
    fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "all" => Self::All,
            "data" => Self::Data,
            "headers" => Self::Headers,
            "totals" => Self::Totals,
            "this row" => Self::ThisRow,
            _ => return None,
        })
    }
}

/// An operator with one operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Leading `-`.
    Neg,
    /// Leading `+`, which Excel accepts and ignores.
    Plus,
    /// Trailing `%`: divides by a hundred.
    Percent,
}

/// An operator with two operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `^`
    Pow,
    /// `&`, text concatenation.
    Concat,
    /// `=`
    Eq,
    /// `<>`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// A space between two references: the cells they have in common.
    Intersect,
    /// A comma between two references: both areas together.
    Union,
    /// A colon between two references: the smallest rectangle holding both.
    ///
    /// `A1:A2:B1` is `A1:B2`. The lexer joins a plain `A1:B2` into one range
    /// on its own, so this is only reached where an operand on either side is
    /// already a range, or is one a function returned.
    Span,
}

/// Parses a formula, with or without its leading `=`.
///
/// # Errors
/// [`Error::InvalidFormula`] describing where the text stopped making sense.
pub fn parse(formula: &str) -> Result<Expr> {
    let text = formula.strip_prefix('=').unwrap_or(formula);
    let tokens = lex(text)?;
    let mut p = Parser {
        tokens,
        at: 0,
        comma_separates: false,
        depth: 0,
    };
    let expr = p.expr(0)?;
    match p.peek() {
        Tok::Eof => Ok(expr),
        other => Err(Error::InvalidFormula(format!("unexpected {other:?}"))),
    }
}

/// A punctuation token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Amp,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Percent,
    Colon,
    Comma,
    Semi,
    LParen,
    RParen,
    LBrace,
    RBrace,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Err(CellError),
    /// A name, a function name or a cell reference, with any sheet prefix
    /// already split off.
    Ident {
        sheet: Option<String>,
        text: String,
    },
    /// A structured reference, brackets already read.
    Structured(Structured),
    Op(Op),
    Eof,
}

/// A token and whether whitespace came before it. The space matters: between
/// two references it is the intersection operator.
#[derive(Debug, Clone, PartialEq)]
struct Lexed {
    tok: Tok,
    spaced: bool,
}

/// Splits formula text into tokens.
fn lex(s: &str) -> Result<Vec<Lexed>> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut spaced = false;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            spaced = true;
            i += 1;
            continue;
        }
        let tok = match ch {
            '"' => Tok::Str(lex_quoted(&c, &mut i, '"', "unterminated string")?),
            '#' => lex_error(&c, &mut i)?,
            '0'..='9' | '.' => lex_number(&c, &mut i)?,
            '\'' => lex_quoted_sheet(&c, &mut i)?,
            // `[1]Sheet1!A1` names another workbook by its index; anything
            // else in brackets at the front of a reference is a column of the
            // table the formula sits in.
            '[' if brackets_hold_an_index(&c, i) => lex_workbook_ref(&c, &mut i)?,
            '[' => lex_structured(&c, &mut i, None)?,
            c0 if is_name_char(c0) => {
                let text = take_name(&c, &mut i);
                // A name followed straight by `[` is a table.
                if c.get(i) == Some(&'[') {
                    lex_structured(&c, &mut i, Some(text))?
                }
                // A bare sheet name is only recognised as one by the `!`.
                else if c.get(i) == Some(&'!') {
                    i += 1;
                    Tok::Ident {
                        sheet: Some(text),
                        text: take_name(&c, &mut i),
                    }
                } else {
                    Tok::Ident { sheet: None, text }
                }
            }
            _ => Tok::Op(lex_op(&c, &mut i)?),
        };
        out.push(Lexed { tok, spaced });
        spaced = false;
    }
    out.push(Lexed {
        tok: Tok::Eof,
        spaced,
    });
    Ok(out)
}

/// Consumes a run delimited by `quote`, where a doubled quote is one literal
/// quote. The opening quote is at `*i`.
fn lex_quoted(c: &[char], i: &mut usize, quote: char, unterminated: &str) -> Result<String> {
    let mut text = String::new();
    // Skip the opening quote.
    *i += 1;
    loop {
        let Some(&ch) = c.get(*i) else {
            return Err(Error::InvalidFormula(unterminated.into()));
        };
        *i += 1;
        if ch == quote {
            if c.get(*i) == Some(&quote) {
                text.push(quote);
                *i += 1;
                continue;
            }
            return Ok(text);
        }
        text.push(ch);
    }
}

/// An error literal, `#` first.
fn lex_error(c: &[char], i: &mut usize) -> Result<Tok> {
    // FIXME: this builds a `String` for every `#`, where a prefix comparison
    // would do. Error literals are rare enough that it has not been worth it.
    let rest: String = c[*i..].iter().collect();
    let found = ERROR_LITERALS
        .iter()
        .find(|lit| rest.starts_with(**lit))
        .ok_or_else(|| Error::InvalidFormula(format!("unknown error literal {rest}")))?;
    *i += found.chars().count();
    CellError::parse(found)
        .map(Tok::Err)
        .ok_or_else(|| Error::InvalidFormula((*found).to_owned()))
}

/// A number, with an exponent only where a sign or digit really follows it:
/// `1E` alone is not a number.
fn lex_number(c: &[char], i: &mut usize) -> Result<Tok> {
    let start = *i;
    while *i < c.len() && (c[*i].is_ascii_digit() || c[*i] == '.') {
        *i += 1;
    }
    // The exponent.
    if let Some(&e) = c.get(*i)
        && (e == 'e' || e == 'E')
    {
        let mut j = *i + 1;
        if matches!(c.get(j), Some('+' | '-')) {
            j += 1;
        }
        if c.get(j).is_some_and(char::is_ascii_digit) {
            *i = j;
            while *i < c.len() && c[*i].is_ascii_digit() {
                *i += 1;
            }
        }
    }
    let text: String = c[start..*i].iter().collect();
    text.parse()
        .map(Tok::Num)
        .map_err(|_| Error::InvalidFormula(format!("bad number {text}")))
}

/// A sheet name in single quotes, which must be followed by `!`.
fn lex_quoted_sheet(c: &[char], i: &mut usize) -> Result<Tok> {
    let name = lex_quoted(c, i, '\'', "unterminated sheet name")?;
    expect_bang(c, i, "sheet name", &name)?;
    Ok(Tok::Ident {
        sheet: Some(name),
        text: take_name(c, i),
    })
}

/// Whether the brackets at `i` hold a workbook index rather than a column.
///
/// Excel numbers external workbooks, so `[1]` is a link and `[Amount]` is a
/// column of the table the formula sits in.
fn brackets_hold_an_index(c: &[char], i: usize) -> bool {
    let Some(close) = c[i..].iter().position(|&ch| ch == ']') else {
        return false;
    };
    close > 1 && c[i + 1..i + close].iter().all(char::is_ascii_digit)
}

/// A structured reference: the brackets after a table name, or standing on
/// their own inside the table they belong to.
///
/// The forms are `Sales[Amount]`, `Sales[[#Totals],[Amount]]`,
/// `Sales[[Q1]:[Q4]]` and `Sales[@Amount]`, plus the bare `[Amount]`. Inside a
/// bracketed name an apostrophe escapes the character after it, which is how a
/// column called `Total [%]` is written.
fn lex_structured(c: &[char], i: &mut usize, table: Option<String>) -> Result<Tok> {
    let body = take_brackets(c, i)?;
    let mut out = Structured {
        table,
        part: TablePart::Data,
        columns: None,
    };
    let mut part: Option<TablePart> = None;
    let mut columns: Vec<String> = Vec::new();
    let mut spans = false;
    let mut this_row = false;
    let mut at = 0;
    while at < body.len() {
        match body[at] {
            ',' => at += 1,
            ':' => {
                spans = true;
                at += 1;
            }
            '@' => {
                this_row = true;
                at += 1;
            }
            '[' => {
                let mut inner = at;
                let text = take_brackets(&body, &mut inner)?;
                at = inner;
                read_item(&text.iter().collect::<String>(), &mut part, &mut columns);
            }
            _ => {
                let start = at;
                while at < body.len() && !matches!(body[at], ',' | ':' | ']') {
                    at += 1;
                }
                read_item(
                    &body[start..at].iter().collect::<String>(),
                    &mut part,
                    &mut columns,
                );
            }
        }
    }
    // `@` and `#This Row` say the same thing, and `@` wins because it is what
    // Excel writes.
    out.part = if this_row {
        TablePart::ThisRow
    } else {
        part.unwrap_or(TablePart::Data)
    };
    let mut names = columns.into_iter();
    if let Some(first) = names.next() {
        out.columns = Some((first, spans.then(|| names.next()).flatten()));
    }
    Ok(Tok::Structured(out))
}

/// One item of a bracket body: a `#specifier`, or a column name.
fn read_item(text: &str, part: &mut Option<TablePart>, columns: &mut Vec<String>) {
    let text = text.trim();
    if let Some(name) = text.strip_prefix('#') {
        if let Some(found) = TablePart::parse(name) {
            *part = Some(part.map_or(found, |had| had.with(found)));
        }
        return;
    }
    // `@Amount` writes the row specifier and the column in one item.
    let name = text.strip_prefix('@').unwrap_or(text);
    if !name.is_empty() {
        columns.push(name.to_owned());
    }
}

/// The characters between the bracket at `*i` and the one closing it, with
/// nesting counted and apostrophe escapes resolved.
fn take_brackets(c: &[char], i: &mut usize) -> Result<Vec<char>> {
    let mut depth = 0usize;
    let mut out = Vec::new();
    while *i < c.len() {
        let ch = c[*i];
        *i += 1;
        match ch {
            '\'' => {
                // The escape belongs to the name, not to the syntax.
                if let Some(&next) = c.get(*i) {
                    out.push(next);
                    *i += 1;
                }
            }
            '[' => {
                depth += 1;
                if depth > 1 {
                    out.push(ch);
                }
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(out);
                }
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    Err(Error::InvalidFormula(
        "unterminated structured reference".into(),
    ))
}

/// `[1]Sheet1!A1` points at another workbook: the number is the position of the
/// link in the book's `<externalReferences>`, and it stays part of the sheet
/// name for the evaluator to split off.
fn lex_workbook_ref(c: &[char], i: &mut usize) -> Result<Tok> {
    let Some(close) = c[*i..].iter().position(|&ch| ch == ']') else {
        return Err(Error::InvalidFormula("unterminated workbook index".into()));
    };
    let mut name: String = c[*i..=*i + close].iter().collect();
    *i += close + 1;
    name.push_str(&take_name(c, i));
    expect_bang(c, i, "workbook reference", &name)?;
    Ok(Tok::Ident {
        sheet: Some(name),
        text: take_name(c, i),
    })
}

/// Consumes the `!` that must close a sheet qualifier.
// Swallowing a missing `!` silently used to parse 'Sheet1'A1 into nonsense.
fn expect_bang(c: &[char], i: &mut usize, what: &str, name: &str) -> Result<()> {
    if c.get(*i) != Some(&'!') {
        return Err(Error::InvalidFormula(format!(
            "{what} {name:?} is not followed by '!'"
        )));
    }
    *i += 1;
    Ok(())
}

/// An operator or a piece of punctuation, two characters first.
fn lex_op(c: &[char], i: &mut usize) -> Result<Op> {
    let two: String = c[*i..(*i + 2).min(c.len())].iter().collect();
    if let Some(op) = match two.as_str() {
        "<=" => Some(Op::Le),
        ">=" => Some(Op::Ge),
        "<>" => Some(Op::Ne),
        _ => None,
    } {
        *i += 2;
        return Ok(op);
    }
    let ch = c[*i];
    let op = match ch {
        '+' => Op::Plus,
        '-' => Op::Minus,
        '*' => Op::Star,
        '/' => Op::Slash,
        '^' => Op::Caret,
        '&' => Op::Amp,
        '=' => Op::Eq,
        '<' => Op::Lt,
        '>' => Op::Gt,
        '%' => Op::Percent,
        ':' => Op::Colon,
        ',' => Op::Comma,
        ';' => Op::Semi,
        '(' => Op::LParen,
        ')' => Op::RParen,
        '{' => Op::LBrace,
        '}' => Op::RBrace,
        _ => {
            return Err(Error::InvalidFormula(format!(
                "unexpected character {ch:?}"
            )));
        }
    };
    *i += 1;
    Ok(op)
}

/// Every error literal Excel writes, longest first so `#N/A` cannot shadow a
/// longer one that starts the same way.
const ERROR_LITERALS: [&str; 8] = [
    "#DIV/0!", "#VALUE!", "#NAME?", "#NULL!", "#CALC!", "#REF!", "#NUM!", "#N/A",
];

/// Characters a name, a function name or a reference may be made of.
fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\\')
}

/// Consumes a run of name characters.
fn take_name(c: &[char], i: &mut usize) -> String {
    let start = *i;
    while *i < c.len() && is_name_char(c[*i]) {
        *i += 1;
    }
    c[start..*i].iter().collect()
}

struct Parser {
    tokens: Vec<Lexed>,
    at: usize,
    /// Whether a comma currently separates arguments rather than joining two
    /// areas. Excel decides this by context, not by precedence: the comma in
    /// `SUM(A1,B2)` is a separator, the one in `SUM((A1,B2))` is the union
    /// operator.
    comma_separates: bool,
    /// How many `expr` calls are on the stack right now.
    depth: u32,
}

/// How tightly an operator binds.
const fn lbp(op: Op) -> u8 {
    match op {
        Op::Colon => 90,
        Op::Comma => 70,
        Op::Percent => 50,
        Op::Caret => 40,
        Op::Star | Op::Slash => 30,
        Op::Plus | Op::Minus => 20,
        Op::Amp => 10,
        Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => 5,
        _ => 0,
    }
}

/// Binding power of the intersection operator, which is written as a space.
const INTERSECT_BP: u8 = 80;

/// How deeply expressions may nest before the parser gives up.
///
/// The parser is a recursive descent, so nesting costs stack: a workbook is
/// untrusted input and `((((...1...))))` a hundred thousand deep would overflow it.
/// Excel itself refuses more than 64 levels, so this is not a limit a formula
/// written by anyone can reach.
const MAX_DEPTH: u32 = 256;

/// Binding power of a leading `-` or `+`. Above `^`, which is why `-2^2` is 4
/// in Excel and not -4.
const UNARY_BP: u8 = 60;

impl Parser {
    fn peek(&self) -> &Tok {
        self.tokens.get(self.at).map_or(&Tok::Eof, |l| &l.tok)
    }

    fn peek_at(&self, offset: usize) -> &Tok {
        self.tokens
            .get(self.at + offset)
            .map_or(&Tok::Eof, |l| &l.tok)
    }

    fn spaced(&self) -> bool {
        self.tokens.get(self.at).is_some_and(|l| l.spaced)
    }

    fn next(&mut self) -> Tok {
        let tok = self.peek().clone();
        self.at += 1;
        tok
    }

    fn eat(&mut self, op: Op) -> bool {
        if *self.peek() == Tok::Op(op) {
            self.at += 1;
            return true;
        }
        false
    }

    fn expect(&mut self, op: Op) -> Result<()> {
        if self.eat(op) {
            return Ok(());
        }
        Err(Error::InvalidFormula(format!(
            "expected {op:?}, found {:?}",
            self.peek()
        )))
    }

    /// Runs `body` with the comma given the meaning `separates` asks for,
    /// restoring the outer meaning afterwards.
    fn grouped<T>(
        &mut self,
        separates: bool,
        body: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        let outer = std::mem::replace(&mut self.comma_separates, separates);
        let out = body(self);
        self.comma_separates = outer;
        out
    }

    /// Parses an expression, stopping at the first operator that binds no
    /// tighter than `min_bp`.
    fn expr(&mut self, min_bp: u8) -> Result<Expr> {
        // Not decremented on the error path: an error here ends the parse.
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::InvalidFormula("nested too deeply".into()));
        }
        let mut lhs = self.operand()?;
        // `LAMBDA(x,x+1)(5)` calls what the operand answered with. Only a
        // `(` glued to it can do that: a space would make it an intersection.
        while *self.peek() == Tok::Op(Op::LParen) && !self.spaced() {
            self.at += 1;
            let args = self.arguments()?;
            lhs = Expr::Apply {
                callee: Box::new(lhs),
                args,
            };
        }
        loop {
            // Two operands in a row, separated by a space, intersect.
            if INTERSECT_BP > min_bp && self.spaced() && self.starts_operand() {
                let rhs = self.expr(INTERSECT_BP)?;
                lhs = Expr::Binary(BinaryOp::Intersect, Box::new(lhs), Box::new(rhs));
                continue;
            }
            let Tok::Op(op) = *self.peek() else { break };
            if op == Op::Comma && self.comma_separates {
                break;
            }
            let bp = lbp(op);
            if bp == 0 || bp <= min_bp {
                break;
            }
            self.at += 1;
            if op == Op::Percent {
                lhs = Expr::Unary(UnaryOp::Percent, Box::new(lhs));
                continue;
            }
            let binary = match op {
                Op::Plus => BinaryOp::Add,
                Op::Minus => BinaryOp::Sub,
                Op::Star => BinaryOp::Mul,
                Op::Slash => BinaryOp::Div,
                Op::Caret => BinaryOp::Pow,
                Op::Amp => BinaryOp::Concat,
                Op::Eq => BinaryOp::Eq,
                Op::Ne => BinaryOp::Ne,
                Op::Lt => BinaryOp::Lt,
                Op::Le => BinaryOp::Le,
                Op::Gt => BinaryOp::Gt,
                Op::Ge => BinaryOp::Ge,
                Op::Comma => BinaryOp::Union,
                Op::Colon => BinaryOp::Span,
                _ => break,
            };
            let rhs = self.expr(bp)?;
            lhs = Expr::Binary(binary, Box::new(lhs), Box::new(rhs));
        }
        self.depth -= 1;
        Ok(lhs)
    }

    /// Whether the current token could begin an operand.
    fn starts_operand(&self) -> bool {
        matches!(
            self.peek(),
            Tok::Num(_)
                | Tok::Str(_)
                | Tok::Err(_)
                | Tok::Ident { .. }
                | Tok::Op(Op::LParen | Op::LBrace)
        )
    }

    /// Parses one operand, including any prefix operator.
    fn operand(&mut self) -> Result<Expr> {
        match self.next() {
            Tok::Num(n) => {
                // `2:3` is a range of whole rows, and both halves lex as
                // numbers rather than as names.
                if *self.peek() == Tok::Op(Op::Colon)
                    && let Tok::Num(end) = *self.peek_at(1)
                    && let Ok(range) = Range::parse(&format!("{n}:{end}"))
                {
                    self.at += 2;
                    return Ok(Expr::Range { sheet: None, range });
                }
                Ok(Expr::Number(n))
            }
            Tok::Str(s) => Ok(Expr::Text(s)),
            Tok::Err(e) => Ok(Expr::Error(e)),
            Tok::Op(Op::Minus) => Ok(Expr::Unary(UnaryOp::Neg, Box::new(self.expr(UNARY_BP)?))),
            Tok::Op(Op::Plus) => Ok(Expr::Unary(UnaryOp::Plus, Box::new(self.expr(UNARY_BP)?))),
            Tok::Op(Op::LParen) => {
                let inner = self.grouped(false, |p| p.expr(0))?;
                self.expect(Op::RParen)?;
                Ok(inner)
            }
            Tok::Op(Op::LBrace) => self.array(),
            Tok::Ident { sheet, text } => self.ident(sheet, text),
            Tok::Structured(reference) => Ok(Expr::Structured(reference)),
            other => Err(Error::InvalidFormula(format!(
                "expected a value, found {other:?}"
            ))),
        }
    }

    /// Parses an array constant, the opening brace already consumed.
    fn array(&mut self) -> Result<Expr> {
        let mut rows = vec![Vec::new()];
        loop {
            let last = rows.len() - 1;
            rows[last].push(self.grouped(true, |p| p.expr(0))?);
            if self.eat(Op::Comma) {
                continue;
            }
            if self.eat(Op::Semi) {
                rows.push(Vec::new());
                continue;
            }
            self.expect(Op::RBrace)?;
            break;
        }
        if rows.iter().any(|r| r.len() != rows[0].len()) {
            return Err(Error::InvalidFormula("array rows differ in length".into()));
        }
        Ok(Expr::Array(rows))
    }

    /// Reads the arguments of a call, the opening `(` already consumed.
    ///
    /// An argument written empty (`SUM(1,,2)`) is [`Expr::Missing`], which is
    /// what lets a function tell "left out" from "nothing".
    fn arguments(&mut self) -> Result<Vec<Expr>> {
        let mut args = Vec::new();
        if self.eat(Op::RParen) {
            return Ok(args);
        }
        loop {
            args.push(if matches!(self.peek(), Tok::Op(Op::Comma | Op::RParen)) {
                Expr::Missing
            } else {
                self.grouped(true, |p| p.expr(0))?
            });
            if self.eat(Op::Comma) {
                continue;
            }
            self.expect(Op::RParen)?;
            break;
        }
        Ok(args)
    }

    /// Resolves a name token: a function call, a boolean, a reference, or a
    /// defined name.
    fn ident(&mut self, sheet: Option<String>, text: String) -> Result<Expr> {
        // A name followed directly by `(` is a call, whatever else it looks
        // like - that is what keeps `LOG10(2)` from being read as a cell.
        if *self.peek() == Tok::Op(Op::LParen) && !self.spaced() {
            self.at += 1;
            let name = function_name(&text);
            let args = self.arguments()?;
            return Ok(Expr::Call { name, args });
        }
        if sheet.is_none() {
            match text.to_uppercase().as_str() {
                "TRUE" => return Ok(Expr::Bool(true)),
                "FALSE" => return Ok(Expr::Bool(false)),
                _ => {}
            }
        }
        // `A1:B2`, `A:A` and `1:1` are all one reference; the halves are only
        // meaningful together, so they are joined here rather than left to the
        // `:` operator.
        if *self.peek() == Tok::Op(Op::Colon) {
            // The far half of `Sheet1!1:1` lexes as a number, not a name, so
            // both shapes have to be accepted here.
            let right = match self.peek_at(1).clone() {
                Tok::Ident { text, .. } => Some(text),
                Tok::Num(n) if n.fract() == 0.0 && n >= 0.0 => Some(format!("{n}")),
                _ => None,
            };
            if let Some(right) = right
                && let Ok(range) = Range::parse(&format!("{text}:{right}"))
            {
                self.at += 2;
                return Ok(Expr::Range { sheet, range });
            }
        }
        if let Ok(cell) = CellRef::parse(&text) {
            return Ok(Expr::Range {
                sheet,
                range: Range::new(cell, cell),
            });
        }
        Ok(match sheet {
            // A name qualified by a sheet stays qualified; the evaluator looks
            // it up in that sheet's scope.
            Some(s) => Expr::Name(format!("{s}!{text}")),
            None => Expr::Name(text),
        })
    }
}

/// A function name as the engine knows it: upper-cased, without the prefixes
/// a file puts on functions newer than the format.
///
/// Excel writes every function added after 2007 as `_xlfn.IFERROR`, and
/// the ones that return arrays as `_xlfn._xlws.FILTER`, so that older versions
/// show `#NAME?` rather than misread them. The prefix is storage, not a
/// different function.
fn function_name(text: &str) -> String {
    let mut name = text.to_uppercase();
    for prefix in ["_XLFN.", "_XLWS."] {
        if let Some(rest) = name.strip_prefix(prefix) {
            name = rest.to_owned();
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::{BinaryOp, Expr, UnaryOp, parse};

    /// Renders a tree as a compact prefix form, so a test can state the shape
    /// it expects on one line.
    fn show(e: &Expr) -> String {
        match e {
            Expr::Number(n) => format!("{n}"),
            Expr::Text(t) => format!("{t:?}"),
            Expr::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
            Expr::Error(err) => err.as_str().to_owned(),
            Expr::Structured(r) => format!(
                "{}[{:?}{:?}]",
                r.table.as_deref().unwrap_or(""),
                r.part,
                r.columns
            ),
            Expr::Range { sheet, range } => match sheet {
                Some(s) => format!("{s}!{range}"),
                None => range.to_string(),
            },
            Expr::Name(n) => n.clone(),
            Expr::Missing => "_".to_owned(),
            Expr::Unary(op, x) => {
                let op = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Plus => "+",
                    UnaryOp::Percent => "%",
                };
                format!("({op} {})", show(x))
            }
            Expr::Binary(op, a, b) => {
                let op = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Pow => "^",
                    BinaryOp::Concat => "&",
                    BinaryOp::Eq => "=",
                    BinaryOp::Ne => "<>",
                    BinaryOp::Lt => "<",
                    BinaryOp::Le => "<=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Ge => ">=",
                    BinaryOp::Intersect => "isect",
                    BinaryOp::Union => "union",
                    BinaryOp::Span => "span",
                };
                format!("({op} {} {})", show(a), show(b))
            }
            Expr::Apply { callee, args } => {
                let args: Vec<String> = args.iter().map(show).collect();
                format!("(apply {} {})", show(callee), args.join(" "))
            }
            Expr::Call { name, args } => {
                let args: Vec<String> = args.iter().map(show).collect();
                if args.is_empty() {
                    format!("({name})")
                } else {
                    format!("({name} {})", args.join(" "))
                }
            }
            Expr::Array(rows) => {
                let rows: Vec<String> = rows
                    .iter()
                    .map(|r| r.iter().map(show).collect::<Vec<_>>().join(" "))
                    .collect();
                format!("{{{}}}", rows.join("; "))
            }
        }
    }

    #[track_caller]
    fn shows(formula: &str, expected: &str) {
        let tree = parse(formula).unwrap_or_else(|e| panic!("{formula:?} must parse: {e}"));
        assert_eq!(show(&tree), expected, "{formula}");
    }

    #[test]
    fn literals() {
        shows("=1", "1");
        shows("1.5", "1.5");
        shows(".5", "0.5");
        shows("1E3", "1000");
        shows("1.5e-2", "0.015");
        shows(r#""a""b""#, r#""a\"b""#);
        shows(r#""""#, r#""""#);
        shows("TRUE", "TRUE");
        shows("false", "FALSE");
        shows("#N/A", "#N/A");
        shows("#DIV/0!", "#DIV/0!");
    }

    #[test]
    fn references() {
        shows("A1", "A1:A1");
        shows("$A$1", "A1:A1");
        shows("A1:B2", "A1:B2");
        shows("$B$4:$D$9", "B4:D9");
        shows("A:A", "A1:A1048576");
        shows("2:3", "A2:XFD3");
        shows("Sheet1!A1", "Sheet1!A1:A1");
        shows("'Лист 1'!A1:B2", "Лист 1!A1:B2");
        shows("Sheet1!A1:B2", "Sheet1!A1:B2");
        shows("myName", "myName");
        shows("Sheet1!myName", "Sheet1!myName");
    }

    #[test]
    fn precedence_follows_excel() {
        shows("1+2*3", "(+ 1 (* 2 3))");
        shows("(1+2)*3", "(* (+ 1 2) 3)");
        // A leading minus binds tighter than a power, so this is 4, not -4.
        shows("-2^2", "(^ (- 2) 2)");
        shows("2^3^2", "(^ (^ 2 3) 2)");
        shows("1&2=3", "(= (& 1 2) 3)");
        shows("1<=2", "(<= 1 2)");
        shows("5%", "(% 5)");
        shows("1+2%", "(+ 1 (% 2))");
        shows("-A1", "(- A1:A1)");
        shows("1--2", "(- 1 (- 2))");
    }

    #[test]
    fn calls() {
        shows("SUM(A1:A3)", "(SUM A1:A3)");
        shows("sum(1,2)", "(SUM 1 2)");
        shows("NOW()", "(NOW)");
        shows("IF(A1,,0)", "(IF A1:A1 _ 0)");
        shows("IF(A1>0,\"y\",\"n\")", "(IF (> A1:A1 0) \"y\" \"n\")");
        shows("SUM(SUM(1),2)", "(SUM (SUM 1) 2)");
        // A function name that reads like a cell reference stays a call.
        shows("LOG10(100)", "(LOG10 100)");
        // The prefixes Excel stores newer functions under are not part of
        // the name.
        shows("_xlfn.IFERROR(1,2)", "(IFERROR 1 2)");
        shows("_xlfn._xlws.FILTER(A1:A2,B1:B2)", "(FILTER A1:A2 B1:B2)");
    }

    #[test]
    fn arrays_and_reference_operators() {
        shows("{1,2;3,4}", "{1 2; 3 4}");
        shows("{1;2}", "{1; 2}");
        shows("A1:B5 A3:D3", "(isect A1:B5 A3:D3)");
        shows("SUM((A1,B2))", "(SUM (union A1:A1 B2:B2))");
        // Inside a call a comma separates arguments and never unions.
        shows("SUM(A1,B2)", "(SUM A1:A1 B2:B2)");
    }

    #[test]
    fn rejects_broken_formulas() {
        for bad in [
            r#""unterminated"#,
            "1+",
            "(1",
            "SUM(1",
            "1 @ 2",
            "#WHAT!",
            "{1,2;3}",
            "'sheet'A1",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must not parse");
        }
    }
}
