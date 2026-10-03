//! Lists and arrays held in one cell: `FLATTEN`, `HAS`, `HASANY` and
//! `HASALL`, from Microsoft's announcement of 24 September 2026 (Beta
//! Channel), and `FILLDOWN`, which the same specification proposes.
//!
//! A nested array is an array whose elements are arrays: `{{1,2,3};{4,5}}`
//! is two rows of one element each. The engine keeps such values as they
//! are, so these functions need nothing from the core.
//!
//! The announcement leaves some things open, and the choices made here are:
//! `FLATTEN` pads with `#N/A`, as `HSTACK` and `VSTACK` do, and removes every
//! level unless told otherwise; `HAS` and its kin compare as `=` does - text
//! without regard to case, no wildcards.

use super::lookup::index_within;
use super::{Arg, cells};
use crate::error::CellError;
use crate::formula::eval::MAX_RANGE_CELLS;
use crate::formula::value::{Value, compare};
use std::cmp::Ordering;

/// `FLATTEN(array, [pad_value], [levels])` - nested arrays opened out into
/// the row that holds them, short rows padded.
pub fn flatten(args: &[Arg]) -> Value {
    let ([array] | [array, _] | [array, _, _]) = args else {
        return Value::Error(CellError::Value);
    };
    let pad = match args.get(1) {
        Some(arg) if !arg.missing() => arg.value.scalar().clone(),
        _ => Value::Error(CellError::Na),
    };
    let levels = match args.get(2) {
        Some(arg) if !arg.missing() => match arg.number() {
            // Deeper than the parser nests anything, so as good as "all".
            Ok(n) => match index_within(n, 64) {
                Some(at) => at + 1,
                None => return Value::Error(CellError::Value),
            },
            Err(e) => return Value::Error(e),
        },
        _ => usize::MAX,
    };
    let Value::Array(rows) = &array.value else {
        return array.value.clone();
    };
    let mut out: Vec<Vec<Value>> = rows
        .iter()
        .map(|row| {
            let mut line = Vec::new();
            for v in row {
                open(v, levels, &mut line);
            }
            line
        })
        .collect();
    let width = out.iter().map(Vec::len).max().unwrap_or(0);
    if width.saturating_mul(out.len()) > MAX_RANGE_CELLS {
        return Value::Error(CellError::Num);
    }
    for line in &mut out {
        line.resize(width, pad.clone());
    }
    Value::array(out)
}

/// Appends the elements of `value`, `levels` deep, row by row.
fn open(value: &Value, levels: usize, out: &mut Vec<Value>) {
    match value {
        Value::Array(rows) if levels > 0 => {
            for v in rows.iter().flatten() {
                open(v, levels - 1, out);
            }
        }
        other => out.push(other.clone()),
    }
}

/// Every value in an array, at any depth.
fn all(value: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    value.flatten(&mut out);
    out
}

fn same(a: &Value, b: &Value) -> bool {
    compare(a, b) == Ordering::Equal
}

/// `HAS(array, value)` - whether `value` is anywhere in `array`.
pub fn has(args: &[Arg]) -> Value {
    let [array, value] = args else {
        return Value::Error(CellError::Value);
    };
    let value = value.value.scalar();
    Value::Bool(all(&array.value).iter().any(|v| same(v, value)))
}

/// `HASANY(array, values)` - whether any of `values` is in `array`.
pub fn hasany(args: &[Arg]) -> Value {
    has_of(args, false)
}

/// `HASALL(array, values)` - whether every one of `values` is in `array`.
pub fn hasall(args: &[Arg]) -> Value {
    has_of(args, true)
}

fn has_of(args: &[Arg], every: bool) -> Value {
    let [array, values] = args else {
        return Value::Error(CellError::Value);
    };
    let array = all(&array.value);
    let found = |wanted: &&Value| array.iter().any(|v| same(v, wanted));
    let wanted = all(&values.value);
    Value::Bool(if every {
        wanted.iter().all(found)
    } else {
        wanted.iter().any(found)
    })
}

/// `FILLDOWN(array, [direction], [skip_values])` - every empty cell given the
/// last value before it: down (1, the default), up (2), right (3) or left (4).
///
/// Empty is a blank, empty text, or anything among `skip_values`. Empty cells
/// before the first value stay as they are.
pub fn filldown(args: &[Arg]) -> Value {
    let ([array] | [array, _] | [array, _, _]) = args else {
        return Value::Error(CellError::Value);
    };
    let direction = match args.get(1) {
        Some(arg) if !arg.missing() => match arg.number() {
            Ok(n) => n.trunc(),
            Err(e) => return Value::Error(e),
        },
        _ => 1.0,
    };
    let skip = args.get(2).filter(|a| !a.missing()).map(cells);
    let empty = |v: &Value| {
        matches!(v, Value::Blank)
            || matches!(v, Value::Text(t) if t.is_empty())
            || skip.as_ref().is_some_and(|s| s.iter().any(|x| same(x, v)))
    };
    let mut grid = match &array.value {
        Value::Array(rows) => rows.as_ref().clone(),
        other => return other.clone(),
    };
    let (rows, cols) = (grid.len(), grid.first().map_or(0, Vec::len));
    let lines: Vec<Vec<(usize, usize)>> = match direction {
        1.0 => (0..cols)
            .map(|c| (0..rows).map(|r| (r, c)).collect())
            .collect(),
        2.0 => (0..cols)
            .map(|c| (0..rows).rev().map(|r| (r, c)).collect())
            .collect(),
        3.0 => (0..rows)
            .map(|r| (0..cols).map(|c| (r, c)).collect())
            .collect(),
        4.0 => (0..rows)
            .map(|r| (0..cols).rev().map(|c| (r, c)).collect())
            .collect(),
        _ => return Value::Error(CellError::Value),
    };
    for line in lines {
        let mut last: Option<Value> = None;
        for (r, c) in line {
            if empty(&grid[r][c]) {
                if let Some(v) = &last {
                    grid[r][c] = v.clone();
                }
            } else {
                last = Some(grid[r][c].clone());
            }
        }
    }
    Value::array(grid)
}
