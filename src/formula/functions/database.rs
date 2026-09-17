//! The database functions.
//!
//! Twelve names, one mechanism: a table whose first row is its column headings,
//! a field named by heading or by position, and a second table of criteria. A
//! criteria row holds conditions that must all hold at once; several rows are
//! alternatives. So this pair
//!
//! ```text
//! Tree   Height        ->  apples at least ten feet, or any pear
//! Apple  >10
//! Pear
//! ```
//!
//! selects on `(Tree = Apple AND Height > 10) OR (Tree = Pear)`.
//!
//! The conditions themselves are the ones `COUNTIF` takes, so they are read by
//! the same [`Criterion`](super::Criterion).

use super::{Arg, Criterion, cells};
use crate::error::CellError;
use crate::formula::value::Value;

/// `DSUM(database, field, criteria)`
pub fn dsum(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| Value::Number(ns.iter().sum()))
}

/// `DPRODUCT(database, field, criteria)`
pub fn dproduct(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| {
        Value::Number(if ns.is_empty() {
            0.0
        } else {
            ns.iter().product()
        })
    })
}

/// `DAVERAGE(database, field, criteria)`
pub fn daverage(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| {
        if ns.is_empty() {
            return Value::Error(CellError::Div0);
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a row count this crate could build"
        )]
        let count = ns.len() as f64;
        Value::Number(ns.iter().sum::<f64>() / count)
    })
}

/// `DMAX(database, field, criteria)`
pub fn dmax(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| extreme(ns, f64::max))
}

/// `DMIN(database, field, criteria)`
pub fn dmin(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| extreme(ns, f64::min))
}

/// `DCOUNT(database, [field], criteria)` - how many of the selected rows hold
/// a number in that field.
pub fn dcount(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| count_value(ns.len()))
}

/// `DCOUNTA(database, [field], criteria)` - how many hold anything at all.
pub fn dcounta(args: &[Arg]) -> Value {
    let Some(picked) = select(args) else {
        return Value::Error(CellError::Value);
    };
    count_value(picked.iter().filter(|v| !matches!(v, Value::Blank)).count())
}

/// `DSTDEV(database, field, criteria)`
pub fn dstdev(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| spread(ns, true, true))
}

/// `DSTDEVP(database, field, criteria)`
pub fn dstdevp(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| spread(ns, false, true))
}

/// `DVAR(database, field, criteria)`
pub fn dvar(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| spread(ns, true, false))
}

/// `DVARP(database, field, criteria)`
pub fn dvarp(args: &[Arg]) -> Value {
    selected_numbers(args, |ns| spread(ns, false, false))
}

/// `DGET(database, field, criteria)` - the one value selected, and an error
/// when the criteria pick none or several.
pub fn dget(args: &[Arg]) -> Value {
    let Some(picked) = select(args) else {
        return Value::Error(CellError::Value);
    };
    let mut found = picked.into_iter().filter(|v| !matches!(v, Value::Blank));
    match (found.next(), found.next()) {
        (Some(only), None) => only,
        (Some(_), Some(_)) => Value::Error(CellError::Num),
        (None, _) => Value::Error(CellError::Value),
    }
}

/// Applies a body to the numbers of the selected field.
fn selected_numbers(args: &[Arg], body: impl Fn(&[f64]) -> Value) -> Value {
    let Some(picked) = select(args) else {
        return Value::Error(CellError::Value);
    };
    let numbers: Vec<f64> = picked
        .iter()
        .filter_map(|v| match v {
            Value::Number(n) => Some(*n),
            _ => None,
        })
        .collect();
    body(&numbers)
}

/// The values of the chosen field in every row the criteria select.
fn select(args: &[Arg]) -> Option<Vec<Value>> {
    let [database, field, criteria] = args else {
        return None;
    };
    let table = grid_of(database)?;
    let (headings, rows) = table.split_first()?;
    let conditions = grid_of(criteria)?;
    let (criteria_headings, criteria_rows) = conditions.split_first()?;

    // The field may be named or numbered; an empty one means every field,
    // which is what `DCOUNT` uses to count rows rather than values.
    let column = match field.value.scalar() {
        Value::Blank => None,
        Value::Text(name) if name.is_empty() => None,
        Value::Text(name) => Some(position_of(headings, name)?),
        other => {
            // A field given as a number is a one-based column position.
            let n = other.number().ok()?.trunc();
            if !(1.0..=1.0e4).contains(&n) {
                return None;
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bounded to 1..=1e4 on the line above"
            )]
            let at = n as usize;
            (at <= headings.len()).then_some(at - 1)?.into()
        }
    };

    let mut out = Vec::new();
    for row in rows {
        if !passes(row, headings, criteria_headings, criteria_rows) {
            continue;
        }
        match column {
            Some(at) => out.push(row.get(at).cloned().unwrap_or(Value::Blank)),
            // No field named: the row itself counts, so one value stands in.
            None => out.push(row.first().cloned().unwrap_or(Value::Blank)),
        }
    }
    Some(out)
}

/// Whether a row satisfies any one of the criteria rows.
fn passes(
    row: &[Value],
    headings: &[Value],
    criteria_headings: &[Value],
    criteria_rows: &[Vec<Value>],
) -> bool {
    criteria_rows.iter().any(|conditions| {
        // Every condition in the row has to hold, and a blank is not one.
        let mut any = false;
        let all = conditions.iter().enumerate().all(|(i, condition)| {
            if matches!(condition, Value::Blank) {
                return true;
            }
            let Some(heading) = criteria_headings.get(i) else {
                return true;
            };
            let Value::Text(name) = heading.scalar() else {
                return true;
            };
            let Some(at) = position_of(headings, name) else {
                return true;
            };
            any = true;
            let value = row.get(at).unwrap_or(&Value::Blank);
            Criterion::parse(condition).matches(value)
        });
        // A criteria row with nothing in it selects everything.
        all && (any || conditions.iter().all(|c| matches!(c, Value::Blank)))
    })
}

/// Which column a heading names, compared without case as Excel compares them.
fn position_of(headings: &[Value], name: &str) -> Option<usize> {
    headings.iter().position(
        |heading| matches!(heading.scalar(), Value::Text(text) if text.eq_ignore_ascii_case(name)),
    )
}

/// An argument as a rectangle of values.
fn grid_of(arg: &Arg) -> Option<Vec<Vec<Value>>> {
    if let Value::Array(rows) = &arg.value {
        return Some(rows.as_ref().clone());
    }
    // A single cell is not a table.
    let flat: Vec<Value> = cells(arg).into_iter().cloned().collect();
    (flat.len() > 1).then(|| flat.into_iter().map(|v| vec![v]).collect())
}

/// The largest or smallest of the selected numbers.
fn extreme(ns: &[f64], pick: fn(f64, f64) -> f64) -> Value {
    if ns.is_empty() {
        return Value::Number(0.0);
    }
    Value::Number(ns.iter().copied().fold(ns[0], pick))
}

/// The variance or standard deviation of them.
fn spread(ns: &[f64], sample: bool, root: bool) -> Value {
    if ns.len() < usize::from(sample) + 1 {
        return Value::Error(CellError::Div0);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a row count this crate could build"
    )]
    let count = ns.len() as f64;
    let mean = ns.iter().sum::<f64>() / count;
    let squares: f64 = ns.iter().map(|n| (n - mean) * (n - mean)).sum();
    let variance = squares / if sample { count - 1.0 } else { count };
    Value::Number(if root { variance.sqrt() } else { variance })
}

/// A count as a value.
#[expect(
    clippy::cast_precision_loss,
    reason = "a count that large cannot be reached"
)]
fn count_value(n: usize) -> Value {
    Value::Number(n as f64)
}
