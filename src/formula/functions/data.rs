//! `BINS`, `TABLEJOIN` and `FUZZYLOOKUP`: a histogram, a join of two tables
//! and a lookup that forgives typos.
//!
//! None of them is an Excel function. They are proposed in the 2026
//! specification this engine follows for its newest functions, and the names
//! and arguments may change; the behaviour is the specification's.

use super::lookup::index_within;
use super::{Arg, cells};
use crate::error::CellError;
use crate::formula::eval::MAX_RANGE_CELLS;
use crate::formula::value::{Value, compare};
use std::cmp::Ordering;
use std::collections::HashMap;

/// An optional argument, `None` when left out.
fn given(args: &[Arg], at: usize) -> Option<&Arg> {
    args.get(at).filter(|a| !a.missing())
}

/// An optional flag, `false` when left out.
fn flag(args: &[Arg], at: usize) -> Result<bool, CellError> {
    given(args, at).map_or(Ok(false), |a| a.number().map(|n| n != 0.0))
}

/// `BINS(values, edges, [right_closed], [include_outside])` - how many
/// values fall between each pair of edges, as rows of lower edge, upper edge
/// and count.
///
/// `edges` is the edges themselves, or a single number of equal intervals
/// between the smallest value and the largest; in that case both outer edges
/// are inside, so every value lands in some interval. Errors among the values
/// are the answer, as in `FREQUENCY`; text and blanks are skipped.
pub fn bins(args: &[Arg]) -> Value {
    let ([values, edges] | [values, edges, _] | [values, edges, _, _]) = args else {
        return Value::Error(CellError::Value);
    };
    let (right_closed, outside) = match (flag(args, 2), flag(args, 3)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Value::Error(e),
    };
    let mut numbers = Vec::new();
    for v in cells(values) {
        match v {
            Value::Number(n) => numbers.push(*n),
            Value::Error(e) => return Value::Error(*e),
            _ => {}
        }
    }
    let counted = !matches!(edges.value, Value::Array(_));
    let edges: Vec<f64> = if counted {
        let count = match edges.number() {
            Ok(n) => match index_within(n, 1_000_000) {
                Some(at) => at + 1,
                None => return Value::Error(CellError::Value),
            },
            Err(e) => return Value::Error(e),
        };
        let lo = numbers.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if numbers.is_empty() {
            return Value::Error(CellError::Calc);
        }
        if hi <= lo {
            vec![lo, hi + 1.0]
        } else {
            #[expect(
                clippy::cast_precision_loss,
                reason = "at most a million intervals, exact in an f64"
            )]
            let step = (hi - lo) / count as f64;
            #[expect(clippy::cast_precision_loss, reason = "as above")]
            let mut edges: Vec<f64> = (0..count).map(|i| lo + step * i as f64).collect();
            // The last edge is the largest value itself, not a sum that may
            // round below it.
            edges.push(hi);
            edges
        }
    } else {
        let mut out = Vec::new();
        for v in cells(edges) {
            match v {
                Value::Number(n) => out.push(*n),
                Value::Error(e) => return Value::Error(*e),
                _ => return Value::Error(CellError::Value),
            }
        }
        out
    };
    if edges.len() < 2 || edges.windows(2).any(|w| w[0] >= w[1]) {
        return Value::Error(CellError::Value);
    }
    let last = edges.len() - 1;
    // Counts per interval, with the values below and above at either end.
    let mut counts = vec![0_u32; edges.len() + 1];
    for x in numbers {
        let slot = if right_closed {
            // (a, b]: the first edge not below x closes x's interval.
            let at = edges.partition_point(|&e| e < x);
            // Counted edges start at the smallest value: nothing is below.
            if counted && x <= edges[0] { 1 } else { at }
        } else {
            // [a, b): the first edge above x closes it.
            let at = edges.partition_point(|&e| e <= x);
            // ... and end at the largest: nothing is above.
            if counted && x >= edges[last] {
                last
            } else {
                at
            }
        };
        counts[slot] += 1;
    }
    let mut rows = Vec::with_capacity(edges.len() + 1);
    if outside {
        rows.push(vec![
            Value::Blank,
            Value::Number(edges[0]),
            Value::Number(counts[0].into()),
        ]);
    }
    for i in 0..last {
        rows.push(vec![
            Value::Number(edges[i]),
            Value::Number(edges[i + 1]),
            Value::Number(counts[i + 1].into()),
        ]);
    }
    if outside {
        rows.push(vec![
            Value::Number(edges[last]),
            Value::Blank,
            Value::Number(counts[last + 1].into()),
        ]);
    }
    Value::array(rows)
}

/// A value as rows of cells.
fn grid(value: &Value) -> Vec<Vec<Value>> {
    match value {
        Value::Array(rows) => rows.as_ref().clone(),
        other => vec![vec![other.clone()]],
    }
}

/// The columns a key names: header names, one-based numbers, or a list of
/// either for a key of several columns.
fn key_columns(headers: &[Value], key: &Arg) -> Result<Vec<usize>, CellError> {
    cells(key)
        .into_iter()
        .map(|part| match part {
            Value::Number(n) => index_within(*n, headers.len()).ok_or(CellError::Value),
            Value::Text(_) => headers
                .iter()
                .position(|h| compare(h, part) == Ordering::Equal)
                .ok_or(CellError::Na),
            Value::Error(e) => Err(*e),
            _ => Err(CellError::Value),
        })
        .collect()
}

/// What a key cell is matched on: text without regard to case, as Excel's
/// lookups match it. A blank key matches nothing, as a null does in SQL.
fn key_of(row: &[Value], columns: &[usize]) -> Option<Vec<String>> {
    columns
        .iter()
        .map(|&c| match row.get(c)? {
            Value::Blank | Value::Array(_) | Value::Lambda(_) => None,
            Value::Text(t) if t.is_empty() => None,
            // `+ 0.0` makes a negative zero the zero it equals.
            Value::Number(n) => Some(format!("n{}", n + 0.0)),
            Value::Text(t) => Some(format!("t{}", t.to_lowercase())),
            Value::Bool(b) => Some(format!("b{b}")),
            Value::Error(e) => Some(format!("e{}", e.as_str())),
        })
        .collect()
}

/// `TABLEJOIN(left, right, left_key, right_key, [join_type], [suffix])` -
/// two tables, headers in their first rows, joined on a key.
///
/// The answer has every column of `left`, then the columns of `right` but its
/// key; a right header that repeats a left one gets `suffix` (`_2`). Rows keep
/// the order of `left`, a left row matching several right rows repeats once
/// per match, and right rows nothing matched (`right`, `full`) come last, with
/// the key in the left key columns.
pub fn tablejoin(args: &[Arg]) -> Value {
    let ([left, right, left_key, right_key]
    | [left, right, left_key, right_key, _]
    | [left, right, left_key, right_key, _, _]) = args
    else {
        return Value::Error(CellError::Value);
    };
    let (keep_left, keep_right) = match given(args, 4).map(Arg::text) {
        None => (false, false),
        Some(Ok(kind)) => match kind.to_ascii_lowercase().as_str() {
            "inner" => (false, false),
            "left" => (true, false),
            "right" => (false, true),
            "full" => (true, true),
            _ => return Value::Error(CellError::Value),
        },
        Some(Err(e)) => return Value::Error(e),
    };
    let suffix = match given(args, 5).map(Arg::text) {
        None => "_2".to_owned(),
        Some(Ok(s)) => s,
        Some(Err(e)) => return Value::Error(e),
    };
    let (left, right) = (grid(&left.value), grid(&right.value));
    let (Some((left_head, left_rows)), Some((right_head, right_rows))) =
        (left.split_first(), right.split_first())
    else {
        return Value::Error(CellError::Value);
    };
    let (lk, rk) = match (
        key_columns(left_head, left_key),
        key_columns(right_head, right_key),
    ) {
        (Ok(l), Ok(r)) if l.len() == r.len() => (l, r),
        (Ok(_), Ok(_)) => return Value::Error(CellError::Value),
        (Err(e), _) | (_, Err(e)) => return Value::Error(e),
    };
    let carried: Vec<usize> = (0..right_head.len()).filter(|c| !rk.contains(c)).collect();

    let header = joined_header(left_head, right_head, &carried, &suffix);

    let mut index: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
    for (i, row) in right_rows.iter().enumerate() {
        if let Some(key) = key_of(row, &rk) {
            index.entry(key).or_default().push(i);
        }
    }
    let width = header.len();
    let mut out = vec![header];
    let mut matched = vec![false; right_rows.len()];
    let right_part = |row: &[Value]| -> Vec<Value> {
        carried
            .iter()
            .map(|&c| row.get(c).cloned().unwrap_or(Value::Blank))
            .collect()
    };
    for row in left_rows {
        let hits = key_of(row, &lk).and_then(|k| index.get(&k));
        match hits {
            Some(hits) => {
                for &i in hits {
                    // Duplicate keys on both sides multiply: a join is where
                    // a small table turns into a huge one.
                    if out.len().saturating_mul(width) >= MAX_RANGE_CELLS {
                        return Value::Error(CellError::Num);
                    }
                    matched[i] = true;
                    let mut line = row.clone();
                    line.extend(right_part(&right_rows[i]));
                    out.push(line);
                }
            }
            None if keep_left => {
                let mut line = row.clone();
                line.resize(width, Value::Blank);
                out.push(line);
            }
            None => {}
        }
    }
    if keep_right {
        for (i, row) in right_rows.iter().enumerate() {
            if matched[i] {
                continue;
            }
            let mut line = vec![Value::Blank; left_head.len()];
            for (&l, &r) in lk.iter().zip(&rk) {
                line[l] = row.get(r).cloned().unwrap_or(Value::Blank);
            }
            line.extend(right_part(row));
            out.push(line);
        }
    }
    Value::array(out)
}

/// The joined header: `left`'s, then the carried columns of `right`, a name
/// that repeats one of `left`'s taking `suffix`.
fn joined_header(left: &[Value], right: &[Value], carried: &[usize], suffix: &str) -> Vec<Value> {
    let mut header = left.to_vec();
    for &c in carried {
        let name = &right[c];
        let clash = left.iter().any(|h| compare(h, name) == Ordering::Equal);
        header.push(match (clash, name.text()) {
            (true, Ok(text)) => Value::Text(text + suffix),
            _ => name.clone(),
        });
    }
    header
}

/// The optimal string alignment distance: Levenshtein's edits plus swapping
/// two neighbours, which is how most typos are made.
fn distance(a: &[char], b: &[char]) -> usize {
    let mut before = vec![0; b.len() + 1];
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        current[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(before[j - 2] + 1);
            }
            current[j] = best;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// How alike two texts are, from 0 to 1: one minus the edits over the longer
/// length.
#[expect(
    clippy::cast_precision_loss,
    reason = "lengths of cell text, at most 32 767"
)]
fn similarity(a: &[char], b: &[char]) -> f64 {
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    1.0 - distance(a, b) as f64 / longest as f64
}

fn letters(text: &str) -> Vec<char> {
    text.to_lowercase().chars().collect()
}

/// `FUZZYLOOKUP(lookup_value, lookup_array, [return_array], [threshold],
/// [if_not_found])` - the entry closest in spelling, without regard to case.
///
/// The closest entry wins if it is at least `threshold` alike (0.8 unless
/// given); on a tie the first in the list. An array of lookup values answers
/// one per value.
pub fn fuzzylookup(args: &[Arg]) -> Value {
    if !(2..=5).contains(&args.len()) {
        return Value::Error(CellError::Value);
    }
    let list = cells(&args[1]);
    let returns = given(args, 2).map(cells);
    if returns.as_ref().is_some_and(|r| r.len() != list.len()) {
        return Value::Error(CellError::Value);
    }
    let threshold = match given(args, 3).map(Arg::number) {
        None => 0.8,
        Some(Ok(t)) if (0.0..=1.0).contains(&t) => t,
        Some(Ok(_)) => return Value::Error(CellError::Value),
        Some(Err(e)) => return Value::Error(e),
    };
    let not_found = given(args, 4).map_or(Value::Error(CellError::Na), |a| a.value.clone());
    let entries: Vec<Option<Vec<char>>> = list
        .iter()
        .map(|v| match v {
            Value::Blank | Value::Error(_) => None,
            other => other.text().ok().map(|t| letters(&t)),
        })
        .collect();
    let find = |needle: &Value| -> Value {
        let needle = match needle.text() {
            Ok(t) => letters(&t),
            Err(e) => return Value::Error(e),
        };
        let mut best: Option<(f64, usize)> = None;
        for (i, entry) in entries.iter().enumerate() {
            let Some(entry) = entry else { continue };
            // No text this much longer or shorter can reach the threshold.
            let (short, long) = (needle.len().min(entry.len()), needle.len().max(entry.len()));
            #[expect(clippy::cast_precision_loss, reason = "lengths of cell text")]
            let reachable = long == 0 || short as f64 / long as f64 >= threshold;
            if !reachable {
                continue;
            }
            let score = similarity(&needle, entry);
            if score >= threshold && best.is_none_or(|(b, _)| score > b) {
                best = Some((score, i));
            }
        }
        match best {
            Some((_, i)) => returns.as_ref().map_or(list[i], |r| r[i]).clone(),
            None => not_found.clone(),
        }
    };
    match &args[0].value {
        Value::Array(rows) => Value::array(
            rows.iter()
                .map(|row| row.iter().map(|v| find(v).scalar().clone()).collect())
                .collect(),
        ),
        other => find(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swap_of_two_letters_is_one_edit() {
        let d = |a: &str, b: &str| distance(&letters(a), &letters(b));
        assert_eq!(d("Microsfot", "Microsoft"), 1);
        assert_eq!(d("kitten", "sitting"), 3);
        assert_eq!(d("", "abc"), 3);
        assert_eq!(d("ca", "abc"), 3);
        assert_eq!(d("Москва", "мсоква"), 1);
    }
}
