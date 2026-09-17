//! Information about a value.

use super::Arg;
use crate::error::CellError;
use crate::formula::eval::{Engine, Origin};
use crate::formula::parser::Expr;
use crate::formula::value::Value;

/// `ISBLANK(value)`
pub fn isblank(args: &[Arg]) -> Value {
    test(args, |v| matches!(v, Value::Blank))
}

/// `ISNUMBER(value)`
pub fn isnumber(args: &[Arg]) -> Value {
    test(args, |v| matches!(v, Value::Number(_)))
}

/// `ISTEXT(value)`
pub fn istext(args: &[Arg]) -> Value {
    test(args, |v| matches!(v, Value::Text(_)))
}

/// `ISNONTEXT(value)`
pub fn isnontext(args: &[Arg]) -> Value {
    test(args, |v| !matches!(v, Value::Text(_)))
}

/// `ISLOGICAL(value)`
pub fn islogical(args: &[Arg]) -> Value {
    test(args, |v| matches!(v, Value::Bool(_)))
}

/// `ISERROR(value)` - any error at all.
pub fn iserror(args: &[Arg]) -> Value {
    test(args, |v| v.error().is_some())
}

/// `ISERR(value)` - any error except `#N/A`.
pub fn iserr(args: &[Arg]) -> Value {
    test(args, |v| matches!(v.error(), Some(e) if e != CellError::Na))
}

/// `ISNA(value)`
pub fn isna(args: &[Arg]) -> Value {
    test(args, |v| v.error() == Some(CellError::Na))
}

/// `NA()` - the "no value available" marker itself.
pub fn na(args: &[Arg]) -> Value {
    if args.is_empty() {
        Value::Error(CellError::Na)
    } else {
        Value::Error(CellError::Value)
    }
}

/// `N(value)` - a number as itself, a boolean as 0 or 1, anything else as 0.
pub fn n(args: &[Arg]) -> Value {
    let [a] = args else {
        return Value::Error(CellError::Value);
    };
    match a.value.scalar() {
        Value::Number(x) => Value::Number(*x),
        Value::Bool(b) => Value::Number(f64::from(u8::from(*b))),
        Value::Error(e) => Value::Error(*e),
        _ => Value::Number(0.0),
    }
}

/// `TYPE(value)` - 1 number, 2 text, 4 logical, 16 error, 64 array.
pub fn type_(args: &[Arg]) -> Value {
    let [a] = args else {
        return Value::Error(CellError::Value);
    };
    Value::Number(match &a.value {
        Value::Array(_) => 64.0,
        other => match other.scalar() {
            Value::Text(_) => 2.0,
            Value::Bool(_) => 4.0,
            Value::Error(_) => 16.0,
            _ => 1.0,
        },
    })
}

/// Shared body of the `IS...` family: one argument, a yes or no answer, and no
/// error propagation - asking whether something is an error must not itself
/// fail.
fn test(args: &[Arg], predicate: fn(&Value) -> bool) -> Value {
    match args {
        [a] => Value::Bool(predicate(a.value.scalar())),
        _ => Value::Error(CellError::Value),
    }
}

/// `ISEVEN(number)` - of the whole part, so `ISEVEN(2.9)` is true.
pub fn iseven(args: &[Arg]) -> Value {
    parity(args, false)
}

/// `ISODD(number)`
pub fn isodd(args: &[Arg]) -> Value {
    parity(args, true)
}

/// Shared body of `ISEVEN` and `ISODD`.
///
/// Excel asks about the whole part, so `ISEVEN(2.9)` is true.
fn parity(args: &[Arg], want_odd: bool) -> Value {
    super::one(args, |n| {
        let whole = n.trunc().abs();
        if !whole.is_finite() || whole > 9.0e15 {
            return Value::Error(CellError::Num);
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "bounded on the line above, and non-negative after abs"
        )]
        let whole = whole as u64;
        Value::Bool(whole.is_multiple_of(2) != want_odd)
    })
}

/// `ERROR.TYPE(value)` - the number Excel gives each of its errors.
pub fn error_type(args: &[Arg]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    // The order is the one `CellError` already keeps, which is the BIFF order
    // and the order Excel numbers them in.
    let code = match arg.value.scalar().error() {
        Some(CellError::Null) => 1.0,
        Some(CellError::Div0) => 2.0,
        Some(CellError::Value) => 3.0,
        Some(CellError::Ref) => 4.0,
        Some(CellError::Name) => 5.0,
        Some(CellError::Num) => 6.0,
        Some(CellError::Na) => 7.0,
        Some(CellError::Calc) => 14.0,
        None => return Value::Error(CellError::Na),
    };
    Value::Number(code)
}

/// `ISREF(value)` - whether the argument is a reference at all.
///
/// Lazy, because that is a question about the expression rather than about
/// what it evaluates to.
pub fn isref(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    if crate::formula::eval::spanned(arg).is_some() {
        return Value::Bool(true);
    }
    // A function that gives back a reference counts too, which is only knowable
    // by asking whether it answers `#REF!` when it fails.
    let _ = engine.eval_expr(origin, arg);
    Value::Bool(false)
}

/// `ISFORMULA(reference)` - whether the cell it names holds one.
pub fn isformula(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    match formula_of(engine, origin, args) {
        Ok(text) => Value::Bool(text.is_some()),
        Err(e) => Value::Error(e),
    }
}

/// `FORMULATEXT(reference)` - the formula a cell holds, with its leading `=`.
pub fn formulatext(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    match formula_of(engine, origin, args) {
        Ok(Some(text)) => Value::Text(format!("={text}")),
        // A cell with no formula has no text to give.
        Ok(None) => Value::Error(CellError::Na),
        Err(e) => Value::Error(e),
    }
}

/// The formula in the cell an argument names, if it names one and it has one.
fn formula_of(
    engine: &mut Engine<'_>,
    origin: Origin,
    args: &[Expr],
) -> Result<Option<String>, CellError> {
    let [arg] = args else {
        return Err(CellError::Value);
    };
    let Some((sheet, range)) = crate::formula::eval::spanned(arg) else {
        return Err(CellError::Na);
    };
    let index = match sheet {
        None => origin.sheet,
        Some(name) => engine
            .book()
            .sheets()
            .iter()
            .position(|s| s.title().eq_ignore_ascii_case(&name))
            .ok_or(CellError::Ref)?,
    };
    let cell = engine
        .book()
        .sheet(index)
        .and_then(|sheet| sheet.get(range.start));
    Ok(match cell.map(|c| &c.value) {
        Some(crate::model::CellValue::Formula { formula, .. }) => Some(formula.clone()),
        _ => None,
    })
}

/// `SHEET([value])` - the tab number of a sheet, counting from one.
pub fn sheet(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let index = match args {
        [] => Some(origin.sheet),
        [arg] => match crate::formula::eval::spanned(arg) {
            // A reference names its own sheet, or the one the formula is on.
            Some((None, _)) => Some(origin.sheet),
            Some((Some(name), _)) => named_sheet(engine, &name),
            None => {
                let value = engine.eval_expr(origin, arg);
                match value.text() {
                    Ok(name) => named_sheet(engine, &name),
                    Err(e) => return Value::Error(e),
                }
            }
        },
        _ => return Value::Error(CellError::Value),
    };
    match index {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a workbook holds far fewer sheets than f64 counts"
        )]
        Some(index) => Value::Number((index + 1) as f64),
        None => Value::Error(CellError::Na),
    }
}

/// `SHEETS([reference])` - how many sheets the workbook has.
pub fn sheets(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let _ = origin;
    if args.len() > 1 {
        return Value::Error(CellError::Value);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a workbook holds far fewer sheets than f64 counts"
    )]
    let count = engine.book().sheets().len() as f64;
    Value::Number(count)
}

/// The tab a name belongs to.
fn named_sheet(engine: &Engine<'_>, name: &str) -> Option<usize> {
    engine
        .book()
        .sheets()
        .iter()
        .position(|s| s.title().eq_ignore_ascii_case(name))
}

/// `CELL(kind, [reference])` - one fact about a cell.
///
/// Only the kinds that do not need the screen are answered: where the cell is,
/// what it holds, and how wide its column was set. `"format"` and `"color"`
/// describe a rendering this crate does not do, so they are left out.
pub fn cell_info(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let (kind, reference) = match args {
        [k] => (k, None),
        [k, r] => (k, Some(r)),
        _ => return Value::Error(CellError::Value),
    };
    let kind = match engine.eval_expr(origin, kind).text() {
        Ok(text) => text.to_lowercase(),
        Err(e) => return Value::Error(e),
    };
    let (sheet_name, range) = match reference {
        Some(arg) => match crate::formula::eval::spanned(arg) {
            Some(found) => found,
            None => return Value::Error(CellError::Value),
        },
        None => (None, crate::Range::new(origin.at, origin.at)),
    };
    let index = match &sheet_name {
        None => origin.sheet,
        Some(name) => match named_sheet(engine, name) {
            Some(index) => index,
            None => return Value::Error(CellError::Ref),
        },
    };
    let at = range.start;
    match kind.as_str() {
        "address" => {
            let cell = format!("${}${}", at.col.to_letters(), at.row.one_based());
            Value::Text(match sheet_name {
                Some(name) => format!("'{name}'!{cell}"),
                None => cell,
            })
        }
        "col" => Value::Number(f64::from(at.col.one_based())),
        "row" => Value::Number(f64::from(at.row.one_based())),
        "contents" => engine.range(origin, sheet_name.as_deref(), range),
        "width" => {
            let width = engine
                .book()
                .sheet(index)
                .and_then(|sheet| sheet.column_width(at.col))
                .unwrap_or(8.43);
            Value::Number(width.round())
        }
        "type" => {
            let value = engine.range(origin, sheet_name.as_deref(), range);
            Value::Text(
                match value.scalar() {
                    Value::Blank => "b",
                    Value::Text(_) => "l",
                    _ => "v",
                }
                .to_owned(),
            )
        }
        "filename" => Value::Text(String::new()),
        // Anything about how the cell is drawn is not something this answers.
        _ => Value::Error(CellError::Value),
    }
}

/// `ISOMITTED(value)` - whether an argument was left out.
///
/// Lazy, since a missing argument is a hole in the call rather than a value
/// that could be passed along.
pub fn isomitted(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    if matches!(arg, Expr::Missing) {
        return Value::Bool(true);
    }
    let value = engine.eval_expr(origin, arg);
    Value::Bool(matches!(value, Value::Blank))
}

/// `INFO(type_text)`: about the environment the workbook is open in.
///
/// There is no Excel around this engine, so the answers are the ones a
/// current Excel on Windows gives, the sheet count is the workbook's own, and
/// the directory, which a workbook read from bytes does not have, is `#N/A`.
pub fn info(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [kind] = args else {
        return Value::Error(CellError::Value);
    };
    let kind = match engine.eval_expr(origin, kind).scalar().text() {
        Ok(text) => text.to_lowercase(),
        Err(e) => return Value::Error(e),
    };
    match kind.as_str() {
        "numfile" => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a workbook holds far fewer sheets than f64 counts"
            )]
            let count = engine.book().sheets().len() as f64;
            Value::Number(count)
        }
        "origin" => Value::Text("$A:$A$1".into()),
        "osversion" => Value::Text("Windows (32-bit) NT 10.00".into()),
        "recalc" => Value::Text("Automatic".into()),
        "release" => Value::Text("16.0".into()),
        "system" => Value::Text("pcdos".into()),
        "directory" => Value::Error(CellError::Na),
        _ => Value::Error(CellError::Value),
    }
}
