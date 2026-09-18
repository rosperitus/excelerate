//! Functions that make and use functions: `LAMBDA` and what iterates with it.
//!
//! These are not ordinary builtins. `LAMBDA` never computes its arguments - it
//! keeps the last one as an expression and the rest as parameter names - and
//! `LET` binds names for the body that follows. Both need the engine's scope,
//! so all of them are lazy, taking the unevaluated arguments.
//!
//! They are written from what Microsoft documents.

use crate::error::CellError;
use crate::formula::eval::{Engine, Origin};
use crate::formula::parser::Expr;
use crate::formula::value::{Lambda, Value};
use std::sync::Arc;

/// `LAMBDA([param, ...], body)` - a function written in the formula language.
///
/// The parameters are names, not values, so they are read from the expressions
/// themselves rather than computed. Called directly, as
/// `LAMBDA(x,x+1)(5)`, the parser turns the second pair of brackets into an
/// application; passed to `MAP` or `REDUCE`, the lambda travels as a value.
pub fn lambda(engine: &mut Engine<'_>, _origin: Origin, args: &[Expr]) -> Value {
    let Some((body, params)) = args.split_last() else {
        return Value::Error(CellError::Value);
    };
    let mut names = Vec::with_capacity(params.len());
    for param in params {
        // A parameter is written as a bare name; anything else - a number, a
        // reference, a call - is not something that can be bound.
        match param {
            Expr::Name(name) => names.push(name.clone()),
            _ => return Value::Error(CellError::Value),
        }
    }
    Value::Lambda(Arc::new(Lambda {
        params: names,
        body: body.clone(),
        captured: engine.captured(),
    }))
}

/// `LET(name, value, [name, value, ...], body)` - names bound for one formula.
///
/// The bindings take effect in order, so a later value may use an earlier
/// name; the body sees them all. A name bound here shadows a defined name of
/// the workbook.
pub fn let_(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    // Pairs of name and value, then the body: an even count means the body is
    // missing, and one argument means there are no bindings at all.
    if args.len() < 3 || args.len().is_multiple_of(2) {
        return Value::Error(CellError::Value);
    }
    let Some((body, pairs)) = args.split_last() else {
        return Value::Error(CellError::Value);
    };
    let mut bound = Vec::with_capacity(pairs.len() / 2);
    for pair in pairs.chunks(2) {
        let [name, value] = pair else {
            return Value::Error(CellError::Value);
        };
        let Expr::Name(name) = name else {
            return Value::Error(CellError::Value);
        };
        // Each value is computed with the names bound so far in view.
        let computed = engine.scoped(bound.clone(), |engine| engine.eval_expr(origin, value));
        if let Some(e) = computed.error() {
            return Value::Error(e);
        }
        bound.push((name.clone(), computed));
    }
    engine.scoped(bound, |engine| engine.eval_expr(origin, body))
}

/// `MAP(array, [array, ...], lambda)` - the lambda applied to each element.
///
/// With several arrays the lambda is called with one element from each, so
/// they have to be the same shape.
pub fn map(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let Some((last, rest)) = args.split_last() else {
        return Value::Error(CellError::Value);
    };
    if rest.is_empty() {
        return Value::Error(CellError::Value);
    }
    let function = engine.eval_expr(origin, last);
    if let Some(refusal) = callable(&function) {
        return refusal;
    }
    let grids: Vec<Vec<Vec<Value>>> = rest
        .iter()
        .map(|a| grid_of(&engine.eval_expr(origin, a)))
        .collect();
    let Some((rows, cols)) = same_shape(&grids) else {
        return Value::Error(CellError::Value);
    };
    let mut out = Vec::with_capacity(rows);
    for r in 0..rows {
        let mut line = Vec::with_capacity(cols);
        for c in 0..cols {
            let arguments = grids.iter().map(|g| g[r][c].clone()).collect();
            line.push(engine.apply(origin, &function, arguments));
        }
        out.push(line);
    }
    Value::array(out)
}

/// `REDUCE(initial, array, lambda)` - the array folded into one value.
pub fn reduce(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    fold(engine, origin, args, false)
}

/// `SCAN(initial, array, lambda)` - the same, keeping every step.
pub fn scan(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    fold(engine, origin, args, true)
}

/// Shared body of `REDUCE` and `SCAN`: the second answers with the running
/// total at each element, the first only with the last one.
fn fold(engine: &mut Engine<'_>, origin: Origin, args: &[Expr], keep_steps: bool) -> Value {
    let [initial, array, function] = args else {
        return Value::Error(CellError::Value);
    };
    let mut total = engine.eval_expr(origin, initial);
    let function = engine.eval_expr(origin, function);
    if let Some(refusal) = callable(&function) {
        return refusal;
    }
    let grid = grid_of(&engine.eval_expr(origin, array));
    let mut steps = Vec::with_capacity(grid.len());
    for row in &grid {
        let mut line = Vec::with_capacity(row.len());
        for value in row {
            total = engine.apply(origin, &function, vec![total.clone(), value.clone()]);
            if let Some(e) = total.error() {
                return Value::Error(e);
            }
            line.push(total.clone());
        }
        steps.push(line);
    }
    if keep_steps {
        Value::array(steps)
    } else {
        total
    }
}

/// `BYROW(array, lambda)` - the lambda applied to each row, which answers with
/// one value per row.
pub fn byrow(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    by_line(engine, origin, args, true)
}

/// `BYCOL(array, lambda)` - the same by column.
pub fn bycol(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    by_line(engine, origin, args, false)
}

/// Shared body of `BYROW` and `BYCOL`.
fn by_line(engine: &mut Engine<'_>, origin: Origin, args: &[Expr], rows: bool) -> Value {
    let [array, function] = args else {
        return Value::Error(CellError::Value);
    };
    let grid = grid_of(&engine.eval_expr(origin, array));
    let function = engine.eval_expr(origin, function);
    if let Some(refusal) = callable(&function) {
        return refusal;
    }
    let (height, width) = (grid.len(), grid.first().map_or(0, Vec::len));
    let lines: Vec<Vec<Value>> = if rows {
        grid.clone()
    } else {
        (0..width)
            .map(|c| (0..height).map(|r| grid[r][c].clone()).collect())
            .collect()
    };
    // A row answers with one value, and the answers stack the way the lines
    // ran: `BYROW` gives a column, `BYCOL` a row.
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let argument = if rows {
            Value::array(vec![line])
        } else {
            Value::array(line.into_iter().map(|v| vec![v]).collect())
        };
        out.push(engine.apply(origin, &function, vec![argument]));
    }
    if rows {
        Value::array(out.into_iter().map(|v| vec![v]).collect())
    } else {
        Value::array(vec![out])
    }
}

/// `MAKEARRAY(rows, columns, lambda)` - an array built by calling the lambda
/// with each row and column number, counted from one.
pub fn makearray(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [rows, columns, function] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(rows), Ok(columns)) = (
        engine.eval_expr(origin, rows).number(),
        engine.eval_expr(origin, columns).number(),
    ) else {
        return Value::Error(CellError::Value);
    };
    let (rows, columns) = (rows.trunc(), columns.trunc());
    if rows < 1.0 || columns < 1.0 {
        return Value::Error(CellError::Value);
    }
    // A workbook is untrusted input, so the size a formula asks for is capped
    // at what a sheet could hold rather than believed.
    if rows * columns > 4_000_000.0 {
        return Value::Error(CellError::Num);
    }
    let function = engine.eval_expr(origin, function);
    if let Some(refusal) = callable(&function) {
        return refusal;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "both are whole, positive and capped just above"
    )]
    let (rows, columns) = (rows as usize, columns as usize);
    let mut out = Vec::with_capacity(rows);
    for r in 1..=rows {
        let mut line = Vec::with_capacity(columns);
        for c in 1..=columns {
            #[expect(
                clippy::cast_precision_loss,
                reason = "an index below the four million cap is exact as a double"
            )]
            let arguments = vec![Value::Number(r as f64), Value::Number(c as f64)];
            line.push(engine.apply(origin, &function, arguments));
        }
        out.push(line);
    }
    Value::array(out)
}

/// Checks that what was passed where a function belongs really is one, so
/// that `MAP(A1:A9, 5)` answers `#CALC!` once rather than filling a whole
/// array with it.
fn callable(value: &Value) -> Option<Value> {
    match value {
        Value::Lambda(_) => None,
        Value::Error(e) => Some(Value::Error(*e)),
        _ => Some(Value::Error(CellError::Calc)),
    }
}

/// Any value as a rectangle, so one element and a whole range are handled the
/// same way.
fn grid_of(value: &Value) -> Vec<Vec<Value>> {
    match value {
        Value::Array(rows) if !rows.is_empty() => rows.as_ref().clone(),
        other => vec![vec![other.clone()]],
    }
}

/// The shape the grids share, or `None` when they differ.
fn same_shape(grids: &[Vec<Vec<Value>>]) -> Option<(usize, usize)> {
    let first = grids.first()?;
    let shape = (first.len(), first.first().map_or(0, Vec::len));
    for grid in grids {
        if grid.len() != shape.0 || grid.iter().any(|row| row.len() != shape.1) {
            return None;
        }
    }
    Some(shape)
}
