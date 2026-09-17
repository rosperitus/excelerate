//! Lookup and reference.

use super::{Arg, read_error};
use crate::error::CellError;
use crate::formula::eval::{Engine, Origin};
use crate::formula::parser::Expr;
use crate::formula::value::{Value, compare};
use crate::{CellRef, Col, Range, Row};
use std::cmp::Ordering;
use std::rc::Rc;

/// `ROW([reference])` - the row of the reference, or of the cell asking.
///
/// Over a range it is every row of it, as a vertical array.
pub fn row(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    position(engine, origin, args, true)
}

/// `COLUMN([reference])` - likewise, but a horizontal array of columns.
pub fn column(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    position(engine, origin, args, false)
}

/// Shared body of `ROW` and `COLUMN`.
fn position(engine: &mut Engine<'_>, origin: Origin, args: &[Expr], want_rows: bool) -> Value {
    let pick = |r: &crate::coordinate::Range| {
        if want_rows {
            (r.start.row.one_based(), r.end.row.one_based())
        } else {
            (r.start.col.one_based(), r.end.col.one_based())
        }
    };
    match args {
        [] => {
            let (n, _) = pick(&crate::coordinate::Range::new(origin.at, origin.at));
            Value::Number(f64::from(n))
        }
        [Expr::Range { range, .. }] => {
            let (first, last) = pick(range);
            let line: Vec<Value> = (first..=last)
                .map(|n| Value::Number(f64::from(n)))
                .collect();
            match (line.len(), want_rows) {
                (1, _) => line[0].clone(),
                // A column of numbers for `ROW`, a row of them for `COLUMN`.
                (_, true) => Value::array(line.into_iter().map(|v| vec![v]).collect()),
                (_, false) => Value::array(vec![line]),
            }
        }
        [other] => {
            // Anything that is not written as a reference still has to be
            // computed, if only to let its error through.
            let v = engine.eval_expr(origin, other);
            v.error()
                .map_or(Value::Error(CellError::Value), Value::Error)
        }
        _ => Value::Error(CellError::Value),
    }
}

/// `ROWS(array)` - how many rows it covers.
pub fn rows(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    size(engine, origin, args, true)
}

/// `COLUMNS(array)`
pub fn columns(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    size(engine, origin, args, false)
}

/// Shared body of `ROWS` and `COLUMNS`, which count the reference as written
/// rather than the values behind it - `ROWS(A1:A100)` is 100 even when only
/// three of those cells hold anything.
fn size(engine: &mut Engine<'_>, origin: Origin, args: &[Expr], want_rows: bool) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    // The size of the reference as written, chains of `:` included, rather
    // than of the values behind it.
    if let Some((_, range)) = crate::formula::eval::spanned(arg) {
        let n = if want_rows {
            range.height()
        } else {
            range.width()
        };
        return Value::Number(f64::from(n));
    }
    match engine.eval_expr(origin, arg) {
        Value::Array(rows) => {
            let n = if want_rows {
                rows.len()
            } else {
                rows.first().map_or(0, Vec::len)
            };
            count_value(n)
        }
        Value::Error(e) => Value::Error(e),
        _ => Value::Number(1.0),
    }
}

/// `CHOOSE(index, value1, ...)`
pub fn choose(args: &[Arg]) -> Value {
    let Some((index, choices)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    match index.number() {
        Ok(n) if n >= 1.0 => {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked at least 1; a larger index falls through to #VALUE!"
            )]
            let i = n as usize - 1;
            choices
                .get(i)
                .map_or(Value::Error(CellError::Value), |a| a.value.clone())
        }
        Ok(_) => Value::Error(CellError::Value),
        Err(e) => Value::Error(e),
    }
}

/// `INDEX(array, row, [column])` - 1-based, and 0 means the whole row or
/// column.
pub fn index(args: &[Arg]) -> Value {
    // Only the position arguments are checked for errors: an error sitting in
    // some other cell of the array is not this call's problem.
    let (array, row, col, col_given) = match args {
        [a, r] => (&a.value, r.number(), Ok(0.0), false),
        [a, r, c] if c.missing() => (&a.value, r.number(), Ok(0.0), false),
        [a, r, c] => (&a.value, r.number(), c.number(), true),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(row), Ok(mut col)) = (row, col) else {
        return read_error(&[row, col]);
    };
    let grid = as_grid(array);
    // With one row and no column asked for, the single number picks the
    // column rather than the row: `INDEX(B5:F5, 3)` is D5.
    let mut row = row;
    if !col_given && grid.len() == 1 && row >= 1.0 {
        col = row;
        row = 1.0;
    }
    let (Some(r), Some(c)) = (position_of(row), position_of(col)) else {
        return Value::Error(CellError::Value);
    };
    match (r, c) {
        (Position::At(r), Position::At(c)) => grid
            .get(r)
            .and_then(|line| line.get(c))
            .cloned()
            .unwrap_or(Value::Error(CellError::Ref)),
        (Position::At(r), Position::All) => grid.get(r).map_or_else(
            || Value::Error(CellError::Ref),
            |line| Value::array(vec![line.clone()]),
        ),
        (Position::All, Position::At(c)) => {
            if grid.iter().any(|line| c >= line.len()) {
                return Value::Error(CellError::Ref);
            }
            Value::array(grid.iter().map(|line| vec![line[c].clone()]).collect())
        }
        // Both zero: the whole array.
        (Position::All, Position::All) => Value::Array(grid),
    }
}

/// What an `INDEX` row or column argument asks for.
#[derive(Clone, Copy)]
enum Position {
    /// 0: the whole row or column.
    All,
    /// A 0-based offset into it.
    At(usize),
}

/// Reads an `INDEX` position; a negative number is not a position at all.
fn position_of(n: f64) -> Option<Position> {
    // Excel drops the fraction of a position first, so 0.2 means "all", the
    // same as 0 - and not a position one before the first.
    let n = n.trunc();
    if n < 0.0 {
        return None;
    }
    if n == 0.0 {
        return Some(Position::All);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked positive; a huge index simply misses the array"
    )]
    let i = n as usize - 1;
    Some(Position::At(i))
}

/// `MATCH(value, array, [type])`
///
/// Type 1 (the default) wants the array sorted upwards and finds the largest
/// value that is not greater than the one looked for; -1 wants it sorted
/// downwards; 0 asks for an exact match and needs no order at all.
pub fn match_(args: &[Arg]) -> Value {
    let (needle, array, kind) = match args {
        [n, a] => (&n.value, &a.value, Ok(1.0)),
        [n, a, k] => (&n.value, &a.value, k.number()),
        _ => return Value::Error(CellError::Value),
    };
    if let Some(e) = needle.error() {
        return Value::Error(e);
    }
    let Ok(kind) = kind else {
        return read_error(&[kind]);
    };
    let list = as_list(array);
    // An exact-match lookup takes wildcards, the way `SEARCH` and the criteria
    // functions do; the sorted kinds do not, since ordering a pattern means
    // nothing.
    if kind == 0.0
        && let Value::Text(pattern) = needle.scalar()
        && pattern.contains(['*', '?'])
    {
        let upper = pattern.to_uppercase();
        for (i, v) in list.iter().enumerate() {
            if let Value::Text(text) = v.scalar()
                && super::wildcard_match(&upper, &text.to_uppercase())
            {
                return count_value(i + 1);
            }
        }
        return Value::Error(CellError::Na);
    }
    let found = if kind == 0.0 {
        list.iter()
            .position(|v| compare(v, needle.scalar()) == Ordering::Equal)
    } else {
        sorted_position(&list, needle.scalar(), kind < 0.0)
    };
    found.map_or(Value::Error(CellError::Na), |i| count_value(i + 1))
}

/// Where an approximate lookup lands: the last entry not past the value, found
/// by binary search the way Excel finds it.
///
/// The list is supposed to be sorted, and on one that is this is the last
/// entry not greater (or, `descending`, not less) than the value. On one that
/// is not, the answer is whatever the halving lands on - and a workbook that
/// looks up an unsorted column shows exactly that answer, so a scan for the
/// "right" one would disagree with every cell Excel computed.
fn sorted_position(list: &[&Value], needle: &Value, descending: bool) -> Option<usize> {
    // Excel searches only among values of the needle's kind: text among
    // text, numbers among numbers. A blank or a number in a row of names is
    // not where the halving may land, or `MATCH("si.01", r)` answers a blank.
    let kind = |v: &Value| match v {
        Value::Number(_) => 0,
        Value::Text(_) => 1,
        Value::Bool(_) => 2,
        _ => 3,
    };
    let wanted = kind(needle);
    let candidates: Vec<usize> = (0..list.len())
        .filter(|&i| kind(list[i].scalar()) == wanted)
        .collect();
    let not_past = |v: &Value| {
        let order = compare(v, needle);
        if descending {
            order != Ordering::Less
        } else {
            order != Ordering::Greater
        }
    };
    let (mut low, mut high) = (0usize, candidates.len().checked_sub(1)?);
    let mut found = None;
    while low <= high {
        let middle = low + (high - low) / 2;
        if not_past(list[candidates[middle]]) {
            found = Some(candidates[middle]);
            low = middle + 1;
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    found
}

/// `VLOOKUP(value, table, column, [approximate])`
pub fn vlookup(args: &[Arg]) -> Value {
    table_lookup(args, true)
}

/// `HLOOKUP(value, table, row, [approximate])`
pub fn hlookup(args: &[Arg]) -> Value {
    table_lookup(args, false)
}

/// Shared body of `VLOOKUP` and `HLOOKUP`.
///
/// With `approximate` on - Excel's default - the first column or row must be
/// sorted upwards, and the search stops at the last entry that is not greater
/// than the value looked for.
fn table_lookup(args: &[Arg], vertical: bool) -> Value {
    let (needle, table, offset, approximate) = match args {
        [n, t, o] => (&n.value, &t.value, o.number(), Ok(true)),
        [n, t, o, a] => (&n.value, &t.value, o.number(), a.value.boolean()),
        _ => return Value::Error(CellError::Value),
    };
    if let Some(e) = needle.error() {
        return Value::Error(e);
    }
    let (Ok(offset), Ok(approximate)) = (offset, approximate) else {
        return Value::Error(
            offset
                .err()
                .or_else(|| approximate.err())
                .unwrap_or(CellError::Value),
        );
    };
    if offset < 1.0 {
        return Value::Error(CellError::Value);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked at least 1; a larger offset is caught as #REF! below"
    )]
    let offset = offset as usize - 1;

    let grid = as_grid(table);
    let keys: Vec<&Value> = if vertical {
        grid.iter().filter_map(|line| line.first()).collect()
    } else {
        grid.first()
            .map(|line| line.iter().collect())
            .unwrap_or_default()
    };

    let found = if approximate {
        sorted_position(&keys, needle.scalar(), false)
    } else {
        keys.iter()
            .position(|key| compare(key, needle.scalar()) == Ordering::Equal)
    };
    let Some(i) = found else {
        return Value::Error(CellError::Na);
    };
    let cell = if vertical {
        grid.get(i).and_then(|line| line.get(offset))
    } else {
        grid.get(offset).and_then(|line| line.get(i))
    };
    cell.cloned().unwrap_or(Value::Error(CellError::Ref))
}

/// A value as a rectangle of its own, for a function that reorders or
/// rebuilds the rows.
fn owned_grid(v: &Value) -> Vec<Vec<Value>> {
    Rc::unwrap_or_clone(as_grid(v))
}

/// A value as a rectangle, so a scalar and a range can be walked the same way.
///
/// Shared with the value, not copied: `INDEX` picking one cell out of a
/// table of forty thousand must not copy the forty thousand. A function that
/// reorders the rows takes its own copy with [`Rc::unwrap_or_clone`].
fn as_grid(v: &Value) -> Rc<Vec<Vec<Value>>> {
    match v {
        Value::Array(rows) => Rc::clone(rows),
        other => Rc::new(vec![vec![other.clone()]]),
    }
}

/// A value as a flat list, for the functions that search a single row or
/// column. Borrowed from the value: a search reads the list, and copying a
/// column of text to look for one entry in it cost more than the search.
fn as_list(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(rows) => rows.iter().flatten().collect(),
        other => vec![other],
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a sheet cannot hold enough cells to lose a digit here"
)]
fn count_value(n: usize) -> Value {
    Value::Number(n as f64)
}

/// `TRANSPOSE(array)` - rows become columns.
pub fn transpose(args: &[Arg]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    let grid = owned_grid(&arg.value);
    let width = grid.first().map_or(0, Vec::len);
    let flipped = (0..width)
        .map(|c| grid.iter().map(|row| row[c].clone()).collect())
        .collect();
    Value::array(flipped)
}

/// `AREAS(reference)` - how many separate rectangles a reference names.
///
/// Lazy, because the answer is about the reference rather than its values:
/// only the union operator makes more than one area.
pub fn areas(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    // Anything that is not a reference at all is an error, so it is computed
    // only to let that error through.
    if !matches!(arg, Expr::Range { .. } | Expr::Binary(..)) {
        let value = engine.eval_expr(origin, arg);
        return value
            .error()
            .map_or(Value::Error(CellError::Value), Value::Error);
    }
    count_value(count_areas(arg))
}

/// How many rectangles an expression names; only a union makes more than one.
fn count_areas(e: &Expr) -> usize {
    match e {
        Expr::Binary(crate::formula::parser::BinaryOp::Union, a, b) => {
            count_areas(a) + count_areas(b)
        }
        _ => 1,
    }
}

/// `ADDRESS(row, column, [kind], [a1], [sheet])` - a reference as text.
pub fn address(args: &[Arg]) -> Value {
    if let Some(e) = super::first_error(args) {
        return Value::Error(e);
    }
    // Past the column comes the kind of reference, then the `a1` flag - which
    // chooses R1C1 notation and is not honoured here - then a sheet name.
    let (row, column, rest) = match args {
        [row, column, rest @ ..] if rest.len() <= 3 => (row, column, rest),
        _ => return Value::Error(CellError::Value),
    };
    let (row, column) = (row.number(), column.number());
    let kind = rest
        .first()
        .filter(|a| !a.missing())
        .map_or(Ok(1.0), Arg::number);
    let sheet = rest.get(2).map(Arg::text);
    let (Ok(row), Ok(column), Ok(kind)) = (row, column, kind) else {
        return Value::Error(CellError::Value);
    };
    // Both are one-based positions on the sheet, so anything outside what a
    // sheet has is refused rather than clamped.
    let whole = |n: f64| -> Option<u64> {
        (n.is_finite() && (1.0..=1.0e7).contains(&n.trunc())).then(|| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bounded to 1..=1e7 on the line above"
            )]
            let whole = n.trunc() as u64;
            whole
        })
    };
    let (Some(row), Some(column)) = (whole(row), whole(column)) else {
        return Value::Error(CellError::Value);
    };
    let (Ok(row), Ok(column)) = (Row::from_one_based(row), Col::from_one_based(column)) else {
        return Value::Error(CellError::Value);
    };
    // 1 pins both, 2 the row, 3 the column, 4 neither. A code, so an integer.
    let kind = kind.trunc();
    if !(1.0..=4.0).contains(&kind) {
        return Value::Error(CellError::Value);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 1..=4 on the line above"
    )]
    let (row_fixed, column_fixed) = match kind as u8 {
        1 => (true, true),
        2 => (true, false),
        3 => (false, true),
        _ => (false, false),
    };
    let mut out = String::new();
    if let Some(sheet) = sheet {
        let Ok(sheet) = sheet else {
            return Value::Error(CellError::Value);
        };
        if !sheet.is_empty() {
            let quoted = sheet.chars().any(|c| !c.is_alphanumeric() && c != '_');
            if quoted {
                out.push('\'');
                out.push_str(&sheet.replace('\'', "''"));
                out.push('\'');
            } else {
                out.push_str(&sheet);
            }
            out.push('!');
        }
    }
    if column_fixed {
        out.push('$');
    }
    out.push_str(&column.to_letters());
    if row_fixed {
        out.push('$');
    }
    out.push_str(&row.one_based().to_string());
    Value::Text(out)
}

/// `SORT(array, [index], [order], [by_column])`
pub fn sort(args: &[Arg]) -> Value {
    let Some((array, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    let number = |at: usize, default: f64| {
        rest.get(at)
            .filter(|a| !a.missing())
            .map_or(Ok(default), Arg::number)
    };
    let (Ok(index), Ok(order)) = (number(0, 1.0), number(1, 1.0)) else {
        return Value::Error(CellError::Value);
    };
    let mut grid = owned_grid(&array.value);
    let width = grid.first().map_or(0, Vec::len);
    let Some(column) = index_within(index, width) else {
        return Value::Error(CellError::Value);
    };
    let descending = order < 0.0;
    grid.sort_by(|a, b| {
        let ordering = compare(&a[column], &b[column]);
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    Value::array(grid)
}

/// A one-based index into an axis of `len` cells, as a position from zero.
fn index_within(n: f64, len: usize) -> Option<usize> {
    let n = n.trunc();
    if !(1.0..=1.0e7).contains(&n) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 1..=1e7 on the line above"
    )]
    let at = n as usize;
    (at <= len).then(|| at - 1)
}

/// `SORTBY(array, by1, [order1], ...)` - sorted by another array of the same
/// height rather than by one of its own columns.
pub fn sortby(args: &[Arg]) -> Value {
    let Some((array, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    if rest.is_empty() {
        return Value::Error(CellError::Value);
    }
    let grid = owned_grid(&array.value);
    let keys = as_list(&rest[0].value);
    if keys.len() != grid.len() {
        return Value::Error(CellError::Value);
    }
    let descending = rest
        .get(1)
        .and_then(|a| a.number().ok())
        .is_some_and(|order| order < 0.0);
    let mut rows: Vec<(usize, Vec<Value>)> = grid.into_iter().enumerate().collect();
    rows.sort_by(|(i, _), (j, _)| {
        let ordering = compare(keys[*i], keys[*j]);
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    Value::array(rows.into_iter().map(|(_, row)| row).collect())
}

/// `UNIQUE(array, [by_column], [exactly_once])`
pub fn unique(args: &[Arg]) -> Value {
    let Some((array, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    let once = rest
        .get(1)
        .and_then(|a| a.value.boolean().ok())
        .unwrap_or(false);
    let grid = owned_grid(&array.value);
    let same = |a: &[Value], b: &[Value]| {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| compare(x, y) == core::cmp::Ordering::Equal)
    };
    let mut out: Vec<Vec<Value>> = Vec::new();
    for row in &grid {
        let seen = grid.iter().filter(|other| same(other, row)).count();
        // `exactly_once` keeps only the rows that appear a single time.
        if once && seen > 1 {
            continue;
        }
        if !out.iter().any(|kept| same(kept, row)) {
            out.push(row.clone());
        }
    }
    if out.is_empty() {
        return Value::Error(CellError::Calc);
    }
    Value::array(out)
}

/// `FILTER(array, include, [if_empty])` - the rows where the test holds.
pub fn filter(args: &[Arg]) -> Value {
    let (array, include, fallback) = match args {
        [a, i] => (a, i, None),
        [a, i, f] => (a, i, Some(f)),
        _ => return Value::Error(CellError::Value),
    };
    let grid = owned_grid(&array.value);
    let tests = as_list(&include.value);
    if tests.len() != grid.len() {
        return Value::Error(CellError::Value);
    }
    let kept: Vec<Vec<Value>> = grid
        .into_iter()
        .zip(&tests)
        .filter(|(_, test)| test.boolean().unwrap_or(false))
        .map(|(row, _)| row)
        .collect();
    if kept.is_empty() {
        return match fallback {
            Some(value) => value.value.clone(),
            None => Value::Error(CellError::Calc),
        };
    }
    Value::array(kept)
}

/// `TAKE(array, rows, [columns])` - the first or last few of each.
pub fn take(args: &[Arg]) -> Value {
    slice_of(args, true)
}

/// `DROP(array, rows, [columns])` - everything but them.
pub fn drop(args: &[Arg]) -> Value {
    slice_of(args, false)
}

/// Shared body of `TAKE` and `DROP`.
///
/// A negative count works from the far end, which is the whole of what the two
/// have in common with each other.
fn slice_of(args: &[Arg], keep: bool) -> Value {
    let (array, rows, columns) = match args {
        [a, r] => (a, r.number(), None),
        [a, r, c] => (a, r.number(), Some(c.number())),
        _ => return Value::Error(CellError::Value),
    };
    let grid = owned_grid(&array.value);
    let width = grid.first().map_or(0, Vec::len);
    let span = |count: Option<f64>, len: usize| -> Option<(usize, usize)> {
        let Some(count) = count else {
            return Some((0, len));
        };
        let count = count.trunc();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to the length of the axis just below"
        )]
        let taken = (count.abs() as usize).min(len);
        Some(if keep {
            if count < 0.0 {
                (len - taken, len)
            } else {
                (0, taken)
            }
        } else if count < 0.0 {
            (0, len - taken)
        } else {
            (taken, len)
        })
    };
    let (Ok(rows), columns) = (rows, columns.transpose()) else {
        return Value::Error(CellError::Value);
    };
    let Ok(columns) = columns else {
        return Value::Error(CellError::Value);
    };
    let (Some((top, bottom)), Some((left, right))) =
        (span(Some(rows), grid.len()), span(columns, width))
    else {
        return Value::Error(CellError::Value);
    };
    if top >= bottom || left >= right {
        return Value::Error(CellError::Calc);
    }
    Value::array(
        grid[top..bottom]
            .iter()
            .map(|row| row[left..right].to_vec())
            .collect(),
    )
}

/// `CHOOSEROWS(array, row1, ...)` - the rows named, in the order named.
pub fn chooserows(args: &[Arg]) -> Value {
    chosen(args, true)
}

/// `CHOOSECOLS(array, column1, ...)`
pub fn choosecols(args: &[Arg]) -> Value {
    chosen(args, false)
}

/// Shared body of `CHOOSEROWS` and `CHOOSECOLS`.
fn chosen(args: &[Arg], by_row: bool) -> Value {
    let Some((array, wanted)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    if wanted.is_empty() {
        return Value::Error(CellError::Value);
    }
    let grid = owned_grid(&array.value);
    let width = grid.first().map_or(0, Vec::len);
    let len = if by_row { grid.len() } else { width };
    let mut picked = Vec::new();
    for arg in wanted {
        for value in super::cells(arg) {
            let Ok(n) = value.number() else {
                return Value::Error(CellError::Value);
            };
            let n = n.trunc();
            // A negative index counts from the far end, as one.
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "range-checked against the axis on the line below"
            )]
            let at = if n < 0.0 {
                let back = n.abs() as usize;
                if back > len {
                    return Value::Error(CellError::Value);
                }
                len - back
            } else {
                if n < 1.0 || n as usize > len {
                    return Value::Error(CellError::Value);
                }
                n as usize - 1
            };
            picked.push(at);
        }
    }
    Value::array(if by_row {
        picked.into_iter().map(|i| grid[i].clone()).collect()
    } else {
        grid.iter()
            .map(|row| picked.iter().map(|i| row[*i].clone()).collect())
            .collect()
    })
}

/// `VSTACK(array1, ...)` - the arrays one under another.
pub fn vstack(args: &[Arg]) -> Value {
    let grids: Vec<Vec<Vec<Value>>> = args.iter().map(|a| owned_grid(&a.value)).collect();
    let width = grids
        .iter()
        .map(|g| g.first().map_or(0, Vec::len))
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    for grid in grids {
        for mut row in grid {
            // Short rows are padded, which is what Excel shows for a ragged
            // stack rather than refusing it.
            while row.len() < width {
                row.push(Value::Error(CellError::Na));
            }
            out.push(row);
        }
    }
    if out.is_empty() {
        return Value::Error(CellError::Value);
    }
    Value::array(out)
}

/// `HSTACK(array1, ...)` - the arrays side by side.
pub fn hstack(args: &[Arg]) -> Value {
    let grids: Vec<Vec<Vec<Value>>> = args.iter().map(|a| owned_grid(&a.value)).collect();
    let height = grids.iter().map(Vec::len).max().unwrap_or(0);
    if height == 0 {
        return Value::Error(CellError::Value);
    }
    let mut out = vec![Vec::new(); height];
    for grid in &grids {
        let width = grid.first().map_or(0, Vec::len);
        for (r, row) in out.iter_mut().enumerate() {
            match grid.get(r) {
                Some(line) => row.extend(line.iter().cloned()),
                None => row.extend((0..width).map(|_| Value::Error(CellError::Na))),
            }
        }
    }
    Value::array(out)
}

/// `TOROW(array)` - everything in one row, read across.
pub fn torow(args: &[Arg]) -> Value {
    let flat = flatten_grid(args);
    flat.map_or(Value::Error(CellError::Value), |values| {
        Value::array(vec![values])
    })
}

/// `TOCOL(array)` - everything in one column.
pub fn tocol(args: &[Arg]) -> Value {
    let flat = flatten_grid(args);
    flat.map_or(Value::Error(CellError::Value), |values| {
        Value::array(values.into_iter().map(|v| vec![v]).collect())
    })
}

/// Every value of the first argument, read across the rows.
fn flatten_grid(args: &[Arg]) -> Option<Vec<Value>> {
    let array = args.first()?;
    let grid = owned_grid(&array.value);
    let values: Vec<Value> = grid.into_iter().flatten().collect();
    (!values.is_empty()).then_some(values)
}

/// `HYPERLINK(target, [label])` - the label, since a formula has no link to
/// follow; the target is what the writer stores beside the cell.
pub fn hyperlink(args: &[Arg]) -> Value {
    match args {
        [target] => target.value.scalar().clone(),
        [_, label] => label.value.scalar().clone(),
        _ => Value::Error(CellError::Value),
    }
}

/// `XLOOKUP(value, lookup, return, [if_missing], [mode], [search])`
///
/// The one that fixed what `VLOOKUP` got wrong: the arrays are given
/// separately rather than by a column offset, a miss has an answer of its own,
/// and an exact match is what it does unless told otherwise.
pub fn xlookup(args: &[Arg]) -> Value {
    let (needle, haystack, results, rest) = match args {
        [needle, haystack, results, rest @ ..] if rest.len() <= 3 => {
            (needle, haystack, results, rest)
        }
        _ => return Value::Error(CellError::Value),
    };
    let missing = rest.first();
    let number_at = |at: usize, default: f64| {
        rest.get(at)
            .filter(|a| !a.missing())
            .map_or(Ok(default), Arg::number)
    };
    let (mode, direction) = (number_at(1, 0.0), number_at(2, 1.0));
    let (Ok(mode), Ok(direction)) = (mode, direction) else {
        return Value::Error(CellError::Value);
    };
    let list = as_list(&haystack.value);
    let Some(at) = find_in(&list, needle.value.scalar(), mode, direction) else {
        return match missing.filter(|a| !a.missing()) {
            Some(fallback) => fallback.value.clone(),
            None => Value::Error(CellError::Na),
        };
    };
    // The result array may be a rectangle, in which case a whole row of it
    // comes back rather than a single value.
    let grid = as_grid(&results.value);
    if grid.len() == list.len() {
        return match grid[at].len() {
            1 => grid[at][0].clone(),
            _ => Value::array(vec![grid[at].clone()]),
        };
    }
    as_list(&results.value)
        .get(at)
        .map_or(Value::Error(CellError::Ref), |v| (*v).clone())
}

/// `XMATCH(value, lookup, [mode], [search])` - the position rather than the
/// value, with the same rules.
pub fn xmatch(args: &[Arg]) -> Value {
    let (needle, haystack, rest) = match args {
        [needle, haystack, rest @ ..] if rest.len() <= 2 => (needle, haystack, rest),
        _ => return Value::Error(CellError::Value),
    };
    let number_at = |at: usize, default: f64| {
        rest.get(at)
            .filter(|a| !a.missing())
            .map_or(Ok(default), Arg::number)
    };
    let (mode, direction) = (number_at(0, 0.0), number_at(1, 1.0));
    let (Ok(mode), Ok(direction)) = (mode, direction) else {
        return Value::Error(CellError::Value);
    };
    let list = as_list(&haystack.value);
    find_in(&list, needle.value.scalar(), mode, direction)
        .map_or(Value::Error(CellError::Na), |at| count_value(at + 1))
}

/// Where a value sits in a list, by the mode `XLOOKUP` and `XMATCH` share.
///
/// Mode 0 is exact, -1 takes the next smaller and 1 the next larger, and 2 is
/// a wildcard match. A negative direction searches from the end.
fn find_in(list: &[&Value], needle: &Value, mode: f64, direction: f64) -> Option<usize> {
    /// Exact, the nearest below, the nearest above, or a wildcard match.
    #[derive(PartialEq, Eq)]
    enum Mode {
        Exact,
        Smaller,
        Larger,
        Wildcard,
    }

    let order: Vec<usize> = if direction < 0.0 {
        (0..list.len()).rev().collect()
    } else {
        (0..list.len()).collect()
    };
    let mode = match mode.trunc() {
        m if m <= -1.0 => Mode::Smaller,
        m if m >= 2.0 => Mode::Wildcard,
        m if m >= 1.0 => Mode::Larger,
        _ => Mode::Exact,
    };
    if mode == Mode::Wildcard {
        let Value::Text(pattern) = needle else {
            return None;
        };
        let upper = pattern.to_uppercase();
        return order.into_iter().find(|i| {
            matches!(list[*i].scalar(), Value::Text(text)
                if super::wildcard_match(&upper, &text.to_uppercase()))
        });
    }
    // An exact hit wins whatever the mode, and is the only answer for mode 0.
    if let Some(exact) = order
        .iter()
        .copied()
        .find(|i| compare(list[*i], needle) == Ordering::Equal)
    {
        return Some(exact);
    }
    if mode == Mode::Exact {
        return None;
    }
    // Otherwise the closest on the side the mode asks for.
    let wanted = if mode == Mode::Smaller {
        Ordering::Less
    } else {
        Ordering::Greater
    };
    let mut best: Option<(usize, &Value)> = None;
    for i in order {
        let value = &list[i];
        if compare(value, needle) != wanted {
            continue;
        }
        // Closest means the largest of the smaller, or the smallest of the
        // larger, so the comparison flips with the side.
        let better = best.is_none_or(|(_, current)| compare(value, current) == wanted.reverse());
        if better {
            best = Some((i, value));
        }
    }
    best.map(|(i, _)| i)
}

/// `LOOKUP(value, vector, [result])`, and its array form.
///
/// The old one: it assumes the list is sorted and takes the last value that is
/// not greater than the one looked for.
pub fn lookup_vector(args: &[Arg]) -> Value {
    let (needle, haystack, results) = match args {
        [n, h] => (n, h, None),
        [n, h, r] => (n, h, Some(r)),
        _ => return Value::Error(CellError::Value),
    };
    let grid = as_grid(&haystack.value);
    let width = grid.first().map_or(0, Vec::len);
    // The array form looks in the first row or column and answers from the
    // last, whichever way round the rectangle is.
    let (list, answers): (Vec<Value>, Vec<Value>) = match results {
        Some(_) => (
            as_list(&haystack.value).into_iter().cloned().collect(),
            Vec::new(),
        ),
        None if width > grid.len() => (
            grid.first().cloned().unwrap_or_default(),
            grid.last().cloned().unwrap_or_default(),
        ),
        None => (
            grid.iter().filter_map(|row| row.first().cloned()).collect(),
            grid.iter().filter_map(|row| row.last().cloned()).collect(),
        ),
    };
    let needle = needle.value.scalar();
    let refs: Vec<&Value> = list.iter().collect();
    let Some(at) = sorted_position(&refs, needle, false) else {
        return Value::Error(CellError::Na);
    };
    match results {
        Some(results) => as_list(&results.value)
            .get(at)
            .map_or(Value::Error(CellError::Na), |v| (*v).clone()),
        None => answers
            .get(at)
            .cloned()
            .unwrap_or(Value::Error(CellError::Na)),
    }
}

/// `INDIRECT(text, [a1])` - the reference a string spells, read now.
///
/// The text is parsed as a formula and then checked to be a reference: that
/// is what keeps `INDIRECT("1+1")` from quietly computing two, which Excel
/// answers `#REF!` for.
pub fn indirect(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let (text, style) = match args {
        [t] => (t, None),
        [t, s] => (t, Some(s)),
        _ => return Value::Error(CellError::Value),
    };
    // R1C1 notation is a different language; this reads A1 only.
    if let Some(style) = style
        && engine.eval_expr(origin, style).boolean() == Ok(false)
    {
        return Value::Error(CellError::Ref);
    }
    let value = engine.eval_expr(origin, text);
    if let Some(e) = value.error() {
        return Value::Error(e);
    }
    let Ok(text) = value.text() else {
        return Value::Error(CellError::Ref);
    };
    let Some(parsed) = engine.parsed(&text) else {
        return Value::Error(CellError::Ref);
    };
    // A defined name is a reference too: `INDIRECT("Sales")` reads the cells
    // the name points at. A name that is no reference is not one to follow.
    let parsed = match &*parsed {
        Expr::Name(name) => {
            let Some(found) = engine.defined_name(name, Some(origin.sheet)) else {
                return Value::Error(CellError::Ref);
            };
            let formula = engine.book().defined_names[found].formula.clone();
            match engine.parsed(&formula) {
                Some(definition) => definition,
                None => return Value::Error(CellError::Ref),
            }
        }
        _ => parsed,
    };
    let Some((sheet, range)) = crate::formula::eval::spanned(&parsed) else {
        return Value::Error(CellError::Ref);
    };
    engine.range(origin, sheet.as_deref(), range)
}

/// `OFFSET(reference, rows, columns, [height], [width])`
///
/// Lazy, because it works on the reference rather than on what it holds: the
/// rectangle is moved and then optionally resized.
pub fn offset(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let (reference, rest) = match args {
        [reference, rest @ ..] if (2..=4).contains(&rest.len()) => (reference, rest),
        _ => return Value::Error(CellError::Value),
    };
    let Some((sheet, base)) = crate::formula::eval::spanned(reference) else {
        return Value::Error(CellError::Ref);
    };
    let mut numbers = Vec::with_capacity(rest.len());
    for arg in rest {
        let value = engine.eval_expr(origin, arg);
        if let Some(e) = value.error() {
            return Value::Error(e);
        }
        // A left-out size keeps the one the reference already has.
        numbers.push(if matches!(value, Value::Blank) {
            None
        } else {
            match value.number() {
                Ok(n) => Some(n.trunc()),
                Err(e) => return Value::Error(e),
            }
        });
    }

    let down = numbers[0].unwrap_or(0.0);
    let across = numbers[1].unwrap_or(0.0);
    let height = numbers.get(2).copied().flatten();
    let width = numbers.get(3).copied().flatten();

    // Anything past a sheet's million rows fails the bounds check below, so
    // the cast only has to be sane for the values that can succeed.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "out-of-range offsets are caught by the corner check below"
    )]
    let step = |by: f64| by as i64;
    let (top, rows) = (
        i64::from(base.start.row.one_based()) + step(down),
        i64::from(base.height()),
    );
    let (left, columns) = (
        i64::from(base.start.col.one_based()) + step(across),
        i64::from(base.width()),
    );
    // A negative size grows the rectangle the other way from its corner.
    let sized = |start: i64, span: i64, wanted: Option<f64>| -> Option<(i64, i64)> {
        let span = match wanted {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "bounded by the sheet in the check that follows"
            )]
            Some(n) => n as i64,
            None => span,
        };
        if span == 0 {
            return None;
        }
        Some(if span < 0 {
            (start + span + 1, -span)
        } else {
            (start, span)
        })
    };
    let (Some((top, rows)), Some((left, columns))) =
        (sized(top, rows, height), sized(left, columns, width))
    else {
        return Value::Error(CellError::Ref);
    };
    let corner = |row: i64, column: i64| -> Option<CellRef> {
        let row = Row::from_one_based(u64::try_from(row).ok()?).ok()?;
        let column = Col::from_one_based(u64::try_from(column).ok()?).ok()?;
        Some(CellRef::new(column, row))
    };
    let (Some(start), Some(end)) = (
        corner(top, left),
        corner(top + rows - 1, left + columns - 1),
    ) else {
        // Moved off the sheet, which is what `#REF!` is for.
        return Value::Error(CellError::Ref);
    };
    engine.range(origin, sheet.as_deref(), Range { start, end })
}

/// `WRAPROWS(vector, count, [pad])` - a line folded into rows of `count`.
pub fn wraprows(args: &[Arg]) -> Value {
    wrapped(args, true)
}

/// `WRAPCOLS(vector, count, [pad])` - the same folded into columns.
pub fn wrapcols(args: &[Arg]) -> Value {
    wrapped(args, false)
}

/// Shared body of the two wrapping functions.
fn wrapped(args: &[Arg], by_row: bool) -> Value {
    let (vector, count, pad) = match args {
        [v, c] => (v, c.number(), None),
        [v, c, p] => (v, c.number(), Some(p.value.scalar().clone())),
        _ => return Value::Error(CellError::Value),
    };
    let Ok(count) = count else {
        return Value::Error(CellError::Value);
    };
    let count = count.trunc();
    if !(1.0..=1.0e6).contains(&count) {
        return Value::Error(CellError::Value);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded to 1..=1e6 on the line above"
    )]
    let count = count as usize;
    let values = as_list(&vector.value);
    if values.is_empty() {
        return Value::Error(CellError::Value);
    }
    // The last line is filled out with the pad, or with #N/A when none was
    // given, which is what Excel puts there.
    let filler = pad.unwrap_or(Value::Error(CellError::Na));
    let lines: Vec<Vec<Value>> = values
        .chunks(count)
        .map(|chunk| {
            let mut line: Vec<Value> = chunk.iter().map(|v| (*v).clone()).collect();
            while line.len() < count {
                line.push(filler.clone());
            }
            line
        })
        .collect();
    Value::array(if by_row {
        lines
    } else {
        // Folded into columns, each chunk is a column of the answer.
        (0..count)
            .map(|i| lines.iter().map(|line| line[i].clone()).collect())
            .collect()
    })
}

/// `EXPAND(array, rows, [columns], [pad])` - grown to a given size.
pub fn expand(args: &[Arg]) -> Value {
    if let Some(e) = super::first_error(args) {
        return Value::Error(e);
    }
    let (array, rest) = match args {
        [array, rest @ ..] if (1..=3).contains(&rest.len()) => (array, rest),
        _ => return Value::Error(CellError::Value),
    };
    let grid = owned_grid(&array.value);
    let width = grid.first().map_or(0, Vec::len);
    let size = |at: usize, current: usize| -> Option<usize> {
        let Some(arg) = rest.get(at).filter(|a| !a.missing()) else {
            return Some(current);
        };
        let n = arg.number().ok()?.trunc();
        (1.0..=1.0e6).contains(&n).then(|| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bounded to 1..=1e6 on the line above"
            )]
            let size = n as usize;
            size
        })
    };
    let (Some(rows), Some(columns)) = (size(0, grid.len()), size(1, width)) else {
        return Value::Error(CellError::Value);
    };
    // Shrinking is not what this does; Excel calls that an error.
    if rows < grid.len() || columns < width {
        return Value::Error(CellError::Value);
    }
    if rows.saturating_mul(columns) > crate::formula::eval::MAX_RANGE_CELLS {
        return Value::Error(CellError::Num);
    }
    let filler = rest
        .get(2)
        .filter(|a| !a.missing())
        .map_or(Value::Error(CellError::Na), |a| a.value.scalar().clone());
    Value::array(
        (0..rows)
            .map(|r| {
                (0..columns)
                    .map(|c| {
                        grid.get(r)
                            .and_then(|row| row.get(c))
                            .cloned()
                            .unwrap_or_else(|| filler.clone())
                    })
                    .collect()
            })
            .collect(),
    )
}

/// `SINGLE(reference)`, written `@reference` - the one value a reference
/// stands for where a single value is wanted.
///
/// This is Excel's implicit intersection: a reference spanning one column
/// answers with the cell on the caller's own row, one spanning a row with the
/// cell in the caller's column, and anything else is `#VALUE!`. It needs no
/// spilling, which is why it can be here while `ANCHORARRAY` is limited.
pub fn single(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [reference] = args else {
        return Value::Error(CellError::Value);
    };
    let Some((sheet, range)) = crate::formula::eval::spanned(reference) else {
        // Not a reference at all: a single value already stands for itself.
        return engine.eval_expr(origin, reference);
    };
    let one_column = range.start.col == range.end.col;
    let one_row = range.start.row == range.end.row;
    let at = if one_column && one_row {
        range.start
    } else if one_column {
        if origin.at.row < range.start.row || origin.at.row > range.end.row {
            return Value::Error(CellError::Value);
        }
        crate::coordinate::CellRef::new(range.start.col, origin.at.row)
    } else if one_row {
        if origin.at.col < range.start.col || origin.at.col > range.end.col {
            return Value::Error(CellError::Value);
        }
        crate::coordinate::CellRef::new(origin.at.col, range.start.row)
    } else {
        // A rectangle has no single cell to intersect with.
        return Value::Error(CellError::Value);
    };
    // The cell may itself hold an array; `@` takes one value from it, which
    // is the top-left one.
    let value = engine.range(
        origin,
        sheet.as_deref(),
        crate::coordinate::Range::new(at, at),
    );
    value.scalar().clone()
}

/// `ANCHORARRAY(reference)`, written `reference#` - the whole array a formula
/// produced, rather than the one value its cell shows.
///
/// Excel means by this the range a result spilled into. Nothing spills here:
/// a formula answering with an array occupies one cell and shows its top-left
/// value. A plain reference to that cell reads the value it shows; this reads
/// the array behind it.
pub fn anchorarray(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [reference] = args else {
        return Value::Error(CellError::Value);
    };
    let Some((sheet, range)) = crate::formula::eval::spanned(reference) else {
        return Value::Error(CellError::Ref);
    };
    if range.start != range.end {
        // The anchor is the cell the result came from, not a range.
        return Value::Error(CellError::Ref);
    }
    let Some(index) = engine.sheet_index(origin, sheet.as_deref()) else {
        return Value::Error(CellError::Ref);
    };
    engine.spilled(index, range.start)
}
