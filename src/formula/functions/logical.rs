//! Logic.

use super::Arg;
use crate::error::CellError;
use crate::formula::eval::{Engine, Origin};
use crate::formula::parser::Expr;
use crate::formula::value::Value;

/// `IF(condition, then, [else])`
///
/// Lazy on purpose: only the branch that is taken is computed, which is what
/// lets `IF(A1=0,0,1/A1)` avoid dividing by zero.
pub fn if_(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    // Both branches are optional: with neither, `IF` answers the condition
    // itself, so `IF(1=1)` is TRUE.
    let (condition, then, otherwise) = match args {
        [c] => (c, None, None),
        [c, t] => (c, Some(t), None),
        [c, t, e] => (c, Some(t), Some(e)),
        _ => return Value::Error(CellError::Value),
    };
    let taken = match engine.eval_expr(origin, condition).boolean() {
        Ok(true) => then,
        Ok(false) => match otherwise {
            Some(e) => Some(e),
            // A condition that is false with no third argument is FALSE.
            None => return Value::Bool(false),
        },
        Err(e) => return Value::Error(e),
    };
    match taken {
        Some(branch) => engine.eval_expr(origin, branch),
        None => Value::Bool(true),
    }
}

/// `IFERROR(value, fallback)` - the fallback is computed only if needed.
pub fn iferror(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    fallback_when(engine, origin, args, |v| v.error().is_some())
}

/// `IFNA(value, fallback)`
pub fn ifna(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    fallback_when(engine, origin, args, |v| v.error() == Some(CellError::Na))
}

/// Shared body of `IFERROR` and `IFNA`.
fn fallback_when(
    engine: &mut Engine<'_>,
    origin: Origin,
    args: &[Expr],
    caught: fn(&Value) -> bool,
) -> Value {
    let [value, fallback] = args else {
        return Value::Error(CellError::Value);
    };
    let v = engine.eval_expr(origin, value);
    if caught(v.scalar()) {
        return engine.eval_expr(origin, fallback);
    }
    v
}

/// `AND(logical1, ...)` - every argument must be true.
pub fn and(args: &[Arg]) -> Value {
    fold(args, true, |a, b| a && b)
}

/// `OR(logical1, ...)` - any argument true is enough.
pub fn or(args: &[Arg]) -> Value {
    fold(args, false, |a, b| a || b)
}

/// `XOR(logical1, ...)` - true when an odd number of arguments are true.
pub fn xor(args: &[Arg]) -> Value {
    fold(args, false, |a, b| a != b)
}

/// Shared body of the multi-argument connectives.
///
/// Text is skipped rather than rejected, as in Excel: `AND(TRUE,"x")` is
/// `TRUE`, because the text is not a logical value at all.
fn fold(args: &[Arg], seed: bool, join: fn(bool, bool) -> bool) -> Value {
    let mut acc = seed;
    let mut any_logical = false;
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for v in flat {
            match v {
                Value::Error(e) => return Value::Error(*e),
                Value::Blank | Value::Text(_) => {}
                other => match other.boolean() {
                    Ok(b) => {
                        acc = join(acc, b);
                        any_logical = true;
                    }
                    Err(e) => return Value::Error(e),
                },
            }
        }
    }
    if any_logical {
        Value::Bool(acc)
    } else {
        // Nothing logical among the arguments at all.
        Value::Error(CellError::Value)
    }
}

/// `NOT(logical)`
pub fn not(args: &[Arg]) -> Value {
    let [a] = args else {
        return Value::Error(CellError::Value);
    };
    match a.value.boolean() {
        Ok(b) => Value::Bool(!b),
        Err(e) => Value::Error(e),
    }
}

/// `TRUE()`
pub fn true_(args: &[Arg]) -> Value {
    if args.is_empty() {
        Value::Bool(true)
    } else {
        Value::Error(CellError::Value)
    }
}

/// `FALSE()`
pub fn false_(args: &[Arg]) -> Value {
    if args.is_empty() {
        Value::Bool(false)
    } else {
        Value::Error(CellError::Value)
    }
}

/// `IFS(test1, value1, ...)` - the value beside the first test that passes.
///
/// Lazy: only the branch that is taken is computed, and the tests stop at the
/// first one that holds.
pub fn ifs(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    if args.is_empty() || !args.len().is_multiple_of(2) {
        return Value::Error(CellError::Value);
    }
    for pair in args.chunks(2) {
        match engine.eval_expr(origin, &pair[0]).boolean() {
            Ok(true) => return engine.eval_expr(origin, &pair[1]),
            Ok(false) => {}
            Err(e) => return Value::Error(e),
        }
    }
    // Nothing matched, which Excel calls not available rather than false.
    Value::Error(CellError::Na)
}

/// `SWITCH(value, case1, result1, ..., [default])`
///
/// The cases are compared for equality rather than tested, and an odd argument
/// left over at the end is what to answer when none of them match.
pub fn switch(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let Some((subject, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    if rest.is_empty() {
        return Value::Error(CellError::Value);
    }
    let subject = engine.eval_expr(origin, subject);
    if let Some(e) = subject.error() {
        return Value::Error(e);
    }
    let mut pairs = rest.chunks(2);
    let mut fallback = None;
    for pair in pairs.by_ref() {
        let [case, result] = pair else {
            // A lone argument at the end is the default.
            fallback = pair.first();
            break;
        };
        let candidate = engine.eval_expr(origin, case);
        if let Some(e) = candidate.error() {
            return Value::Error(e);
        }
        if crate::formula::value::compare(&candidate, subject.scalar())
            == core::cmp::Ordering::Equal
        {
            return engine.eval_expr(origin, result);
        }
    }
    match fallback {
        Some(default) => engine.eval_expr(origin, default),
        None => Value::Error(CellError::Na),
    }
}
