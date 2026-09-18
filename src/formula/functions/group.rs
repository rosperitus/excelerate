//! Summaries of a table in one formula: `GROUPBY` and `PIVOTBY`, with two
//! small relatives, `PERCENTOF` and `TRIMRANGE`.
//!
//! The summarising function is a lambda or a bare function name, as Excel
//! lets it be written: `GROUPBY(A2:A9, C2:C9, SUM)`. A bare name is not a value
//! anywhere else in this engine, so these two look for it themselves.
//!
//! The layout - headers, where the totals go, how groups sort - follows
//! Microsoft's description of the functions. No other engine at hand
//! implements them, so it has not been checked against Excel itself.

use super::{Arg, aggregate_numbers, first_error, is_known};
use crate::error::CellError;
use crate::formula::eval::{Engine, Origin};
use crate::formula::parser::Expr;
use crate::formula::value::{Value, compare};
use std::cmp::Ordering;

/// What a group of values is summarised with.
enum Summary {
    Lambda(Value),
    Builtin(String),
}

/// The name a bare function's argument is bound to while it runs.
const ETA: &str = "_xleta.values";

impl Summary {
    fn read(engine: &mut Engine<'_>, origin: Origin, expr: &Expr) -> Result<Self, Value> {
        let value = engine.eval_expr(origin, expr);
        match (&value, expr) {
            (Value::Lambda(_), _) => Ok(Self::Lambda(value)),
            (Value::Error(CellError::Name), Expr::Name(name)) if is_known(&bare(name)) => {
                Ok(Self::Builtin(bare(name)))
            }
            (Value::Error(e), _) => Err(Value::Error(*e)),
            _ => Err(Value::Error(CellError::Calc)),
        }
    }

    fn apply(&self, engine: &mut Engine<'_>, origin: Origin, values: Vec<Value>) -> Value {
        let column = Value::array(values.into_iter().map(|v| vec![v]).collect());
        let out = match self {
            Self::Lambda(f) => engine.apply(origin, f, vec![column]),
            Self::Builtin(name) => engine.scoped(vec![(ETA.to_owned(), column)], |engine| {
                super::call(engine, origin, name, &[Expr::Name(ETA.to_owned())])
            }),
        };
        out.scalar().clone()
    }
}

/// A function name written as a value, without the prefix a file stores it
/// under: Excel saves `GROUPBY(A2:A9,C2:C9,SUM)` as `..._xleta.SUM)`.
fn bare(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    let mut rest = upper.as_str();
    for prefix in ["_XLFN.", "_XLETA."] {
        rest = rest.strip_prefix(prefix).unwrap_or(rest);
    }
    rest.to_owned()
}

/// A value as rows of cells.
fn grid(value: Value) -> Vec<Vec<Value>> {
    match value {
        Value::Array(rows) => std::sync::Arc::unwrap_or_clone(rows),
        other => vec![vec![other]],
    }
}

/// An optional numeric argument, `default` when left out.
fn option(
    engine: &mut Engine<'_>,
    origin: Origin,
    args: &[Expr],
    at: usize,
    default: f64,
) -> Result<f64, Value> {
    match args.get(at) {
        None | Some(Expr::Missing) => Ok(default),
        Some(expr) => engine
            .eval_expr(origin, expr)
            .scalar()
            .number()
            .map(f64::trunc)
            .map_err(Value::Error),
    }
}

/// Header handling: whether the first row is headers, and whether to show them.
#[derive(Clone, Copy)]
struct Headers {
    present: bool,
    shown: bool,
    generated: bool,
}

impl Headers {
    /// Reads `field_headers`. Left out, the first row counts as headers when
    /// the first value is text and the one under it a number, and they are
    /// not shown.
    fn read(mode: Option<f64>, values: &[Vec<Value>]) -> Result<Self, Value> {
        let mode = mode.unwrap_or_else(|| {
            let first = values.first().and_then(|r| r.first());
            let second = values.get(1).and_then(|r| r.first());
            let looks =
                matches!(first, Some(Value::Text(_))) && matches!(second, Some(Value::Number(_)));
            if looks { 1.0 } else { 0.0 }
        });
        Ok(match mode {
            0.0 => Self {
                present: false,
                shown: false,
                generated: false,
            },
            1.0 => Self {
                present: true,
                shown: false,
                generated: false,
            },
            2.0 => Self {
                present: false,
                shown: true,
                generated: true,
            },
            3.0 => Self {
                present: true,
                shown: true,
                generated: false,
            },
            _ => return Err(Value::Error(CellError::Value)),
        })
    }

    /// The header row taken off `rows`, or names made up for its columns.
    fn take(self, rows: &mut Vec<Vec<Value>>, width: usize, stem: &str) -> Vec<Value> {
        if self.present && !rows.is_empty() {
            return rows.remove(0);
        }
        (1..=width)
            .map(|i| {
                Value::Text(if self.generated {
                    format!("{stem} {i}")
                } else {
                    String::new()
                })
            })
            .collect()
    }
}

/// The rows the filter keeps, all of them when there is none.
fn kept(
    engine: &mut Engine<'_>,
    origin: Origin,
    filter: Option<&Expr>,
    height: usize,
    headers: bool,
) -> Result<Vec<bool>, Value> {
    let Some(expr) = filter.filter(|e| !matches!(e, Expr::Missing)) else {
        return Ok(vec![true; height]);
    };
    let mut flat: Vec<Value> = grid(engine.eval_expr(origin, expr))
        .into_iter()
        .flatten()
        .collect();
    // A filter may cover the header row or not.
    if headers && flat.len() == height + 1 {
        flat.remove(0);
    }
    if flat.len() != height {
        return Err(Value::Error(CellError::Value));
    }
    flat.iter()
        .map(|v| match v {
            Value::Error(e) => Err(Value::Error(*e)),
            other => Ok(other.boolean().unwrap_or(false)),
        })
        .collect()
}

/// Keys equal the way Excel groups them: by value, text without case.
fn same_key(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| compare(x, y) == Ordering::Equal)
}

/// The distinct keys, in order of first appearance.
fn distinct(keys: &[Vec<Value>], rows: &[usize]) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = Vec::new();
    for &r in rows {
        if !out.iter().any(|k| same_key(k, &keys[r])) {
            out.push(keys[r].clone());
        }
    }
    out
}

fn total_label(width: usize) -> Vec<Value> {
    let mut row = vec![Value::Blank; width];
    if let Some(first) = row.first_mut() {
        *first = Value::Text("Total".into());
    }
    row
}

/// `GROUPBY(row_fields, values, function, [field_headers], [total_depth],
/// [sort_order], [filter_array], [field_relationship])`
pub fn groupby(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    match groupby_inner(engine, origin, args) {
        Ok(v) | Err(v) => v,
    }
}

fn groupby_inner(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Result<Value, Value> {
    if args.len() < 3 || args.len() > 8 {
        return Err(Value::Error(CellError::Value));
    }
    let mut fields = grid(engine.eval_expr(origin, &args[0]));
    let mut values = grid(engine.eval_expr(origin, &args[1]));
    let summary = Summary::read(engine, origin, &args[2])?;
    if let Some(e) = error_in(&fields).or_else(|| error_in(&values)) {
        return Err(Value::Error(e));
    }
    if fields.len() != values.len() || fields.is_empty() {
        return Err(Value::Error(CellError::Value));
    }
    let mode = match args.get(3) {
        None | Some(Expr::Missing) => None,
        Some(_) => Some(option(engine, origin, args, 3, 0.0)?),
    };
    let headers = Headers::read(mode, &values)?;
    let depth = option(engine, origin, args, 4, 1.0)?;
    let sort = option(engine, origin, args, 5, 1.0)?;
    let width = fields[0].len();
    let value_width = values[0].len();
    let field_names = headers.take(&mut fields, width, "Row Field");
    let value_names = headers.take(&mut values, value_width, "Value");
    let height = fields.len();
    let keep = kept(engine, origin, args.get(6), height, headers.present)?;
    let rows: Vec<usize> = (0..height).filter(|&r| keep[r]).collect();

    let mut ctx = Grouping {
        engine,
        origin,
        summary: &summary,
        fields: &fields,
        values: &values,
        depth,
        sort,
    };
    let mut out = Vec::new();
    if headers.shown {
        out.push(field_names.into_iter().chain(value_names).collect());
    }
    let grand = |ctx: &mut Grouping<'_, '_>| {
        let mut row = total_label(width);
        row.extend(ctx.summarise(&rows));
        row
    };
    if depth < 0.0 {
        out.push(grand(&mut ctx));
    }
    ctx.level(0, &rows, &mut out);
    if depth > 0.0 {
        out.push(grand(&mut ctx));
    }
    Ok(Value::array(out))
}

fn error_in(rows: &[Vec<Value>]) -> Option<CellError> {
    rows.iter().flatten().find_map(|v| match v {
        Value::Error(e) => Some(*e),
        _ => None,
    })
}

/// What one `GROUPBY` works with while it lays the groups out.
struct Grouping<'e, 'a> {
    engine: &'e mut Engine<'a>,
    origin: Origin,
    summary: &'e Summary,
    fields: &'e [Vec<Value>],
    values: &'e [Vec<Value>],
    depth: f64,
    sort: f64,
}

impl Grouping<'_, '_> {
    /// The summary of each value column over `rows`.
    fn summarise(&mut self, rows: &[usize]) -> Vec<Value> {
        let width = self.values.first().map_or(0, Vec::len);
        (0..width)
            .map(|c| {
                let column = rows.iter().map(|&r| self.values[r][c].clone()).collect();
                self.summary.apply(self.engine, self.origin, column)
            })
            .collect()
    }

    /// Lays out the groups of field `level` among `rows`.
    fn level(&mut self, level: usize, rows: &[usize], out: &mut Vec<Vec<Value>>) {
        let width = self.fields.first().map_or(0, Vec::len);
        let keys: Vec<Vec<Value>> = self.fields.iter().map(|r| vec![r[level].clone()]).collect();
        let mut groups: Vec<(Vec<Value>, Vec<usize>, Vec<Value>)> = distinct(&keys, rows)
            .into_iter()
            .map(|key| {
                let members: Vec<usize> = rows
                    .iter()
                    .copied()
                    .filter(|&r| same_key(&keys[r], &key))
                    .collect();
                let summary = self.summarise(&members);
                (key, members, summary)
            })
            .collect();
        // The sort column counts the fields and then the values, from one.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a small column index"
        )]
        let column = self.sort.abs() as usize;
        let descending = self.sort < 0.0;
        groups.sort_by(|a, b| {
            let ordering = if column > width {
                let c = column - width - 1;
                match (a.2.get(c), b.2.get(c)) {
                    (Some(x), Some(y)) => compare(x, y),
                    _ => Ordering::Equal,
                }
            } else {
                compare(&a.0[0], &b.0[0])
            };
            if descending && (column == level + 1 || column > width) {
                ordering.reverse()
            } else {
                ordering
            }
        });
        let last = level + 1 == width;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a small depth"
        )]
        let depth = self.depth.abs() as usize;
        let subtotal = !last && level + 1 < depth;
        for (_, members, summary) in groups {
            let prefix = |fields: &[Vec<Value>]| -> Vec<Value> {
                let first = members[0];
                (0..width)
                    .map(|c| {
                        if c <= level {
                            fields[first][c].clone()
                        } else {
                            Value::Blank
                        }
                    })
                    .collect()
            };
            let total_row = || {
                let mut row = prefix(self.fields);
                row.extend(summary.iter().cloned());
                row
            };
            if subtotal && self.depth < 0.0 {
                out.push(total_row());
            }
            if last {
                out.push(total_row());
            } else {
                self.level(level + 1, &members, out);
            }
            if subtotal && self.depth > 0.0 {
                let mut row = prefix(self.fields);
                row.extend(summary.iter().cloned());
                out.push(row);
            }
        }
    }
}

/// `PIVOTBY(row_fields, col_fields, values, function, [field_headers],
/// [row_total_depth], [row_sort_order], [col_total_depth], [col_sort_order],
/// [filter_array], [relative_to])`
///
/// ponytail: totals are grand totals only; a depth of two or more, which asks
/// for subtotals, is read as one.
pub fn pivotby(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    match pivotby_inner(engine, origin, args) {
        Ok(v) | Err(v) => v,
    }
}

fn pivotby_inner(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Result<Value, Value> {
    if args.len() < 4 || args.len() > 11 {
        return Err(Value::Error(CellError::Value));
    }
    let mut row_fields = grid(engine.eval_expr(origin, &args[0]));
    let mut col_fields = grid(engine.eval_expr(origin, &args[1]));
    let mut values = grid(engine.eval_expr(origin, &args[2]));
    let summary = Summary::read(engine, origin, &args[3])?;
    for g in [&row_fields, &col_fields, &values] {
        if let Some(e) = error_in(g) {
            return Err(Value::Error(e));
        }
    }
    if row_fields.len() != values.len() || col_fields.len() != values.len() || values.is_empty() {
        return Err(Value::Error(CellError::Value));
    }
    let mode = match args.get(4) {
        None | Some(Expr::Missing) => None,
        Some(_) => Some(option(engine, origin, args, 4, 0.0)?),
    };
    let headers = Headers::read(mode, &values)?;
    let row_depth = option(engine, origin, args, 5, 1.0)?;
    let row_sort = option(engine, origin, args, 6, 1.0)?;
    let col_depth = option(engine, origin, args, 7, 1.0)?;
    let col_sort = option(engine, origin, args, 8, 1.0)?;
    let (row_width, col_width, value_width) =
        (row_fields[0].len(), col_fields[0].len(), values[0].len());
    let row_names = headers.take(&mut row_fields, row_width, "Row Field");
    let _ = headers.take(&mut col_fields, col_width, "Col Field");
    let value_names = headers.take(&mut values, value_width, "Value");
    let height = values.len();
    let keep = kept(engine, origin, args.get(9), height, headers.present)?;
    let rows: Vec<usize> = (0..height).filter(|&r| keep[r]).collect();

    let row_keys = sorted_keys(&row_fields, &rows, row_sort);
    let col_keys = sorted_keys(&col_fields, &rows, col_sort);

    // Each column of the report: a column key (or the total) and a value.
    let mut columns: Vec<(Option<&Vec<Value>>, usize)> = Vec::new();
    let totals_column: Vec<(Option<&Vec<Value>>, usize)> = if col_depth == 0.0 {
        Vec::new()
    } else {
        (0..value_width).map(|v| (None, v)).collect()
    };
    if col_depth < 0.0 {
        columns.extend(totals_column.iter().copied());
    }
    for key in &col_keys {
        columns.extend((0..value_width).map(|v| (Some(key), v)));
    }
    if col_depth > 0.0 {
        columns.extend(totals_column.iter().copied());
    }

    let names = headers.shown.then_some((row_names, value_names.as_slice()));
    let mut out = column_headers(&columns, row_width, col_width, value_width, names);

    let mut cell = |row_key: Option<&Vec<Value>>, col_key: Option<&Vec<Value>>, value: usize| {
        let members: Vec<Value> = rows
            .iter()
            .copied()
            .filter(|&r| row_key.is_none_or(|k| same_key(k, &row_fields[r])))
            .filter(|&r| col_key.is_none_or(|k| same_key(k, &col_fields[r])))
            .map(|r| values[r][value].clone())
            .collect();
        if members.is_empty() {
            Value::Blank
        } else {
            summary.apply(engine, origin, members)
        }
    };
    let mut body: Vec<Vec<Value>> = Vec::new();
    for key in &row_keys {
        let mut row = key.clone();
        for (col, value) in &columns {
            row.push(cell(Some(key), *col, *value));
        }
        body.push(row);
    }
    let mut total = total_label(row_width);
    for (col, value) in &columns {
        total.push(cell(None, *col, *value));
    }
    if row_depth < 0.0 {
        out.push(total);
        out.extend(body);
    } else {
        out.extend(body);
        if row_depth > 0.0 {
            out.push(total);
        }
    }
    Ok(Value::array(out))
}

/// The distinct keys among `rows`, sorted by the one-based key column
/// `order` names, descending when it is negative.
fn sorted_keys(keys: &[Vec<Value>], rows: &[usize], order: f64) -> Vec<Vec<Value>> {
    let mut distinct = distinct(keys, rows);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small column index"
    )]
    let column = (order.abs() as usize).saturating_sub(1);
    distinct.sort_by(|a, b| {
        let o = match (a.get(column), b.get(column)) {
            (Some(x), Some(y)) => compare(x, y),
            _ => Ordering::Equal,
        };
        if order < 0.0 { o.reverse() } else { o }
    });
    distinct
}

/// The header rows of a `PIVOTBY`: a row per column field with its keys over
/// the columns, and a row of value names when there are several values or the
/// headers are shown (`names`: the row field names and the value names).
fn column_headers(
    columns: &[(Option<&Vec<Value>>, usize)],
    row_width: usize,
    col_width: usize,
    value_width: usize,
    names: Option<(Vec<Value>, &[Value])>,
) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = Vec::new();
    for level in 0..col_width {
        let mut row = vec![Value::Blank; row_width];
        for (key, value) in columns {
            row.push(match key {
                Some(k) if *value == 0 => k[level].clone(),
                None if *value == 0 && level == 0 => Value::Text("Total".into()),
                _ => Value::Blank,
            });
        }
        out.push(row);
    }
    if value_width > 1 || names.is_some() {
        let (mut row, value_names) = match names {
            Some((rows, values)) => (rows, Some(values)),
            None => (vec![Value::Blank; row_width], None),
        };
        row.extend(
            columns
                .iter()
                .map(|(_, v)| value_names.map_or(Value::Blank, |n| n[*v].clone())),
        );
        out.push(row);
    }
    out
}

/// `PERCENTOF(data_subset, data_all)`
pub fn percentof(args: &[Arg]) -> Value {
    let [subset, all] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (Ok(part), Ok(whole)) = (
        aggregate_numbers(std::slice::from_ref(subset)),
        aggregate_numbers(std::slice::from_ref(all)),
    ) else {
        return Value::Error(CellError::Value);
    };
    let whole: f64 = whole.iter().sum();
    if whole == 0.0 {
        return Value::Error(CellError::Div0);
    }
    Value::Number(part.iter().sum::<f64>() / whole)
}

/// `TRIMRANGE(range, [trim_rows], [trim_cols])`: the range without its empty
/// outer rows and columns. 0 trims nothing, 1 the leading ones, 2 the
/// trailing ones, 3 (the default) both.
pub fn trimrange(args: &[Arg]) -> Value {
    let Some((range, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    if rest.len() > 2 {
        return Value::Error(CellError::Value);
    }
    let how = |i: usize| -> Result<u8, CellError> {
        match rest.get(i) {
            None => Ok(3),
            Some(a) if a.missing() => Ok(3),
            Some(a) => match a.number()?.trunc() {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "checked to be 0..=3"
                )]
                n if (0.0..=3.0).contains(&n) => Ok(n as u8),
                _ => Err(CellError::Value),
            },
        }
    };
    let (rows_how, cols_how) = match (how(0), how(1)) {
        (Ok(r), Ok(c)) => (r, c),
        (Err(e), _) | (_, Err(e)) => return Value::Error(e),
    };
    let grid = grid(range.value.clone());
    let blank = |v: &Value| matches!(v, Value::Blank);
    let height = grid.len();
    let width = grid.first().map_or(0, Vec::len);
    let row_empty = |r: usize| grid[r].iter().all(blank);
    let col_empty = |c: usize| grid.iter().all(|row| row.get(c).is_none_or(blank));
    let span = |len: usize, how: u8, empty: &dyn Fn(usize) -> bool| {
        let mut start = 0;
        let mut end = len;
        if how & 1 == 1 {
            while start < end && empty(start) {
                start += 1;
            }
        }
        if how & 2 == 2 {
            while end > start && empty(end - 1) {
                end -= 1;
            }
        }
        start..end
    };
    let rows = span(height, rows_how, &row_empty);
    let cols = span(width, cols_how, &col_empty);
    if rows.is_empty() || cols.is_empty() {
        return Value::Error(CellError::Calc);
    }
    Value::array(
        grid[rows]
            .iter()
            .map(|row| row[cols.clone()].to_vec())
            .collect(),
    )
}
