//! The values a formula computes with, and the conversions between them.
//!

use crate::error::CellError;
use crate::formula::parser::Expr;
use std::sync::Arc;

/// A function written in the formula language itself, as `LAMBDA` builds one.
///
/// It carries the names of its parameters, the expression to compute, and the
/// bindings that were in view where it was written: a `LAMBDA` inside a `LET`
/// can use the names that `LET` bound, and it keeps them even where it is
/// finally called.
#[derive(Debug)]
pub struct Lambda {
    /// Parameter names, in order, as they were written.
    pub params: Vec<String>,
    /// The body, computed with the parameters bound to the arguments.
    pub body: Expr,
    /// The bindings in view where the lambda was written.
    pub captured: Vec<(String, Value)>,
}

/// A value a formula can hold.
#[derive(Debug, Clone)]
pub enum Value {
    /// An empty cell, or an argument left out.
    Blank,
    /// A number. Dates are numbers, as everywhere else in Excel.
    Number(f64),
    /// Text.
    Text(String),
    /// A boolean.
    Bool(bool),
    /// An error, which propagates through anything that touches it.
    Error(CellError),
    /// A rectangle of values: a range of cells, or an array constant.
    ///
    /// Shared rather than owned: a range is read once and handed to every
    /// formula that asks for it, and `INDEX(Data,ROW(),1)` down forty thousand
    /// rows must not copy the whole of `Data` forty thousand times. Use
    /// [`Value::array`] to make one and [`Arc::unwrap_or_clone`] to take the
    /// rows out.
    Array(Arc<Vec<Vec<Value>>>),
    /// A function, which `LAMBDA` makes and `MAP` and its kin call.
    ///
    /// A cell cannot hold one: it is a value only while a formula is running,
    /// and a formula that answers with one shows `#CALC!`, as Excel does.
    Lambda(Arc<Lambda>),
}

impl Value {
    /// An array value from its rows.
    #[must_use]
    pub fn array(rows: Vec<Vec<Self>>) -> Self {
        Self::Array(Arc::new(rows))
    }
}

impl PartialEq for Value {
    /// Two lambdas are the same only if they are the same lambda: comparing
    /// what they would compute is not a question a spreadsheet can answer.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Blank, Self::Blank) => true,
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::Text(a), Self::Text(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Error(a), Self::Error(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            (Self::Lambda(a), Self::Lambda(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Value {
    /// The error this value carries, if it is one.
    #[must_use]
    pub const fn error(&self) -> Option<CellError> {
        match self {
            Self::Error(e) => Some(*e),
            _ => None,
        }
    }

    /// The single value this stands for in a scalar position: the top-left
    /// cell of an array, and the value itself otherwise.
    #[must_use]
    pub fn scalar(&self) -> &Self {
        match self {
            Self::Array(rows) => rows
                .first()
                .and_then(|r| r.first())
                .map_or(&Self::Blank, |v| v.scalar()),
            other => other,
        }
    }

    /// The value as a number, following Excel's coercion rules.
    ///
    /// # Errors
    /// [`CellError::Value`] for text that is not a number, and whatever error
    /// the value already carried.
    pub fn number(&self) -> Result<f64, CellError> {
        match self.scalar() {
            Self::Blank => Ok(0.0),
            Self::Number(n) => Ok(*n),
            Self::Bool(b) => Ok(f64::from(u8::from(*b))),
            Self::Error(e) => Err(*e),
            Self::Text(t) => parse_number(t).ok_or(CellError::Value),
            // `scalar` never returns an array; a lambda is not a number and
            // never becomes one.
            Self::Array(_) => Err(CellError::Value),
            Self::Lambda(_) => Err(CellError::Calc),
        }
    }

    /// The value as text, following Excel's coercion rules.
    ///
    /// # Errors
    /// Whatever error the value already carried.
    pub fn text(&self) -> Result<String, CellError> {
        Ok(match self.scalar() {
            Self::Number(n) => number_to_text(*n),
            Self::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
            Self::Text(t) => t.clone(),
            Self::Error(e) => return Err(*e),
            // A blank is empty text; `scalar` never hands back an array.
            Self::Blank | Self::Array(_) => String::new(),
            Self::Lambda(_) => return Err(CellError::Calc),
        })
    }

    /// The value as a condition: numbers are true when non-zero, text is not
    /// accepted at all.
    ///
    /// # Errors
    /// [`CellError::Value`] for text, and whatever error the value carried.
    pub fn boolean(&self) -> Result<bool, CellError> {
        match self.scalar() {
            Self::Blank => Ok(false),
            Self::Bool(b) => Ok(*b),
            Self::Number(n) => Ok(*n != 0.0),
            Self::Error(e) => Err(*e),
            Self::Text(t) => match t.to_uppercase().as_str() {
                "TRUE" => Ok(true),
                "FALSE" => Ok(false),
                _ => Err(CellError::Value),
            },
            Self::Array(_) => Err(CellError::Value),
            Self::Lambda(_) => Err(CellError::Calc),
        }
    }

    /// Walks every element, so a function can treat a mix of scalars, ranges
    /// and array constants as one flat list of values.
    pub fn flatten<'a>(&'a self, out: &mut Vec<&'a Self>) {
        match self {
            Self::Array(rows) => {
                for row in rows.iter() {
                    for v in row {
                        v.flatten(out);
                    }
                }
            }
            other => out.push(other),
        }
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Self::Number(v)
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::Text(v)
    }
}

impl From<CellError> for Value {
    fn from(v: CellError) -> Self {
        Self::Error(v)
    }
}

impl From<Result<f64, CellError>> for Value {
    fn from(v: Result<f64, CellError>) -> Self {
        match v {
            Ok(n) => Self::Number(n),
            Err(e) => Self::Error(e),
        }
    }
}

/// Renders a number the way `&` does.
///
/// This is not the `General` cell format: a formula keeps fifteen significant
/// digits, so `=1/3&""` is `0.333333333333333`, while the same value displayed
/// in a cell is cut to eleven.
fn number_to_text(n: f64) -> String {
    if n == 0.0 {
        return "0".to_owned();
    }
    if !n.is_finite() {
        return n.to_string();
    }
    // Round to fifteen significant digits first, then let the shortest
    // round-trip form of *that* number do the printing.
    let rounded: f64 = format!("{n:.14e}").parse().unwrap_or(n);
    let magnitude = rounded.abs();
    if (1e-5..1e21).contains(&magnitude) {
        return format!("{rounded}");
    }
    // Outside that band Excel switches to its own scientific notation, with
    // the exponent signed and at least two digits.
    let text = format!("{rounded:E}");
    let (mantissa, exponent) = text.split_once('E').unwrap_or((text.as_str(), "0"));
    let (sign, digits) = match exponent.strip_prefix('-') {
        Some(rest) => ('-', rest),
        None => ('+', exponent.trim_start_matches('+')),
    };
    format!("{mantissa}E{sign}{digits:0>2}")
}

/// Reads a number the way Excel reads one typed into a cell: leading and
/// trailing spaces are ignored, a trailing `%` divides by a hundred.
///
fn parse_number(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(head) = t.strip_suffix('%') {
        return head.trim_end().parse::<f64>().ok().map(|n| n / 100.0);
    }
    // Rust accepts `inf` and `nan`, Excel does not.
    let n: f64 = t.parse().ok()?;
    n.is_finite().then_some(n)
}

/// Excel's ordering across types: every number comes before every piece of
/// text, which comes before `FALSE`, which comes before `TRUE`.
///
/// An implementation that compares the operands as strings instead answers
/// `TRUE` for `"z">TRUE`, where Excel answers `FALSE`.
const fn type_rank(v: &Value) -> u8 {
    match v {
        Value::Blank | Value::Number(_) => 0,
        Value::Text(_) => 1,
        Value::Bool(false) => 2,
        Value::Bool(true) => 3,
        Value::Error(_) | Value::Array(_) | Value::Lambda(_) => 4,
    }
}

/// Difference below which two numbers count as equal, as the
/// `BinaryComparison::DELTA`.
const DELTA: f64 = 1e-13;

/// Compares two values by Excel's rules.
///
/// A blank cell takes the type of what it is compared against: blank equals
/// both `0` and `""`.
#[must_use]
pub fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b) = (a.scalar(), b.scalar());
    // A blank compared with text behaves as empty text, not as zero.
    let (a, b) = match (a, b) {
        (Value::Blank, Value::Text(_)) => (&Value::Text(String::new()), b),
        (Value::Text(_), Value::Blank) => (a, &Value::Text(String::new())),
        _ => (a, b),
    };
    match (type_rank(a), type_rank(b)) {
        (x, y) if x != y => x.cmp(&y),
        (0, _) => {
            let (x, y) = (a.number().unwrap_or(0.0), b.number().unwrap_or(0.0));
            if (x - y).abs() < DELTA {
                Ordering::Equal
            } else {
                x.partial_cmp(&y).unwrap_or(Ordering::Equal)
            }
        }
        (1, _) => match (a, b) {
            // Text comparison ignores case, as in Excel. Compared a character
            // at a time rather than by upper-casing copies: a `MATCH` down a
            // column of names does this for every row.
            (Value::Text(x), Value::Text(y)) => {
                if x == y {
                    return Ordering::Equal;
                }
                if x.is_ascii() && y.is_ascii() {
                    return x
                        .bytes()
                        .map(|b| b.to_ascii_uppercase())
                        .cmp(y.bytes().map(|b| b.to_ascii_uppercase()));
                }
                x.chars()
                    .flat_map(char::to_uppercase)
                    .cmp(y.chars().flat_map(char::to_uppercase))
            }
            _ => Ordering::Equal,
        },
        _ => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, compare};
    use crate::error::CellError;
    use std::cmp::Ordering;

    #[test]
    fn coercion_follows_excel() {
        assert_eq!(Value::Text("  1.5 ".into()).number(), Ok(1.5));
        assert_eq!(Value::Text("50%".into()).number(), Ok(0.5));
        assert_eq!(Value::Text("abc".into()).number(), Err(CellError::Value));
        assert_eq!(Value::Text("inf".into()).number(), Err(CellError::Value));
        assert_eq!(Value::Blank.number(), Ok(0.0));
        assert_eq!(Value::Bool(true).number(), Ok(1.0));
        assert_eq!(
            Value::Error(CellError::Div0).number(),
            Err(CellError::Div0),
            "an error passes straight through"
        );

        assert_eq!(Value::Number(1234.5678).text(), Ok("1234.5678".into()));
        assert_eq!(
            Value::Number(1.0 / 3.0).text(),
            Ok("0.333333333333333".into()),
            "fifteen significant digits, not the eleven a cell displays"
        );
        assert_eq!(Value::Number(1e21).text(), Ok("1E+21".into()));
        assert_eq!(Value::Number(-1.5e-7).text(), Ok("-1.5E-07".into()));
        assert_eq!(Value::Bool(false).text(), Ok("FALSE".into()));
        assert_eq!(Value::Blank.text(), Ok(String::new()));

        assert_eq!(Value::Text("x".into()).boolean(), Err(CellError::Value));
        assert_eq!(Value::Number(-2.0).boolean(), Ok(true));
    }

    #[test]
    fn comparison_orders_types_as_excel_does() {
        let num = Value::Number(9e9);
        let text = Value::Text("a".into());
        assert_eq!(
            compare(&num, &text),
            Ordering::Less,
            "any number < any text"
        );
        assert_eq!(
            compare(&Value::Text("z".into()), &Value::Bool(true)),
            Ordering::Less,
            "any text < TRUE, where says the opposite"
        );
        assert_eq!(
            compare(&Value::Bool(false), &Value::Bool(true)),
            Ordering::Less
        );
        assert_eq!(
            compare(&Value::Text("ABC".into()), &Value::Text("abc".into())),
            Ordering::Equal,
            "text compares without case"
        );
        assert_eq!(compare(&Value::Blank, &Value::Number(0.0)), Ordering::Equal);
        assert_eq!(
            compare(&Value::Blank, &Value::Text(String::new())),
            Ordering::Equal,
            "a blank takes the type it is compared with"
        );
    }
}
