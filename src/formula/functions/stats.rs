//! Statistics.

use super::{Arg, aggregate_numbers, cells, selected};
use crate::error::CellError;
use crate::formula::value::Value;

/// `AVERAGE(number1, ...)`
pub fn average(args: &[Arg]) -> Value {
    match aggregate_numbers(args) {
        // An average of nothing has no answer; Excel says so with `#DIV/0!`.
        Ok(ns) if ns.is_empty() => Value::Error(CellError::Div0),
        #[expect(
            clippy::cast_precision_loss,
            reason = "a count that large cannot be reached"
        )]
        Ok(ns) => Value::Number(ns.iter().sum::<f64>() / ns.len() as f64),
        Err(e) => Value::Error(e),
    }
}

/// `MIN(number1, ...)`
pub fn min(args: &[Arg]) -> Value {
    extreme(args, f64::min)
}

/// `MAX(number1, ...)`
pub fn max(args: &[Arg]) -> Value {
    extreme(args, f64::max)
}

/// Shared body of `MIN` and `MAX`, which both answer 0 when nothing numeric
/// was passed.
fn extreme(args: &[Arg], pick: fn(f64, f64) -> f64) -> Value {
    match aggregate_numbers(args) {
        Ok(ns) => Value::Number(ns.into_iter().reduce(pick).unwrap_or(0.0)),
        Err(e) => Value::Error(e),
    }
}

/// `MEDIAN(number1, ...)`
pub fn median(args: &[Arg]) -> Value {
    match aggregate_numbers(args) {
        Ok(ns) if ns.is_empty() => Value::Error(CellError::Num),
        Ok(mut ns) => {
            ns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mid = ns.len() / 2;
            Value::Number(if ns.len() % 2 == 0 {
                f64::midpoint(ns[mid - 1], ns[mid])
            } else {
                ns[mid]
            })
        }
        Err(e) => Value::Error(e),
    }
}

/// `COUNT(value1, ...)` - how many of them are numbers.
pub fn count(args: &[Arg]) -> Value {
    let mut n = 0usize;
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for v in flat {
            let counts = match v {
                Value::Number(_) => true,
                Value::Blank | Value::Error(_) | Value::Array(_) | Value::Lambda(_) => false,
                // Written into the formula, text and booleans count if they
                // convert; read out of a cell they never do.
                Value::Bool(_) | Value::Text(_) => !arg.reference && v.number().is_ok(),
            };
            n += usize::from(counts);
        }
    }
    count_value(n)
}

/// `COUNTA(value1, ...)` - how many are not empty.
pub fn counta(args: &[Arg]) -> Value {
    let mut n = 0usize;
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        n += flat.iter().filter(|v| !matches!(v, Value::Blank)).count();
    }
    count_value(n)
}

/// `COUNTBLANK(range)` - how many are empty.
///
/// The count only covers the part of the sheet that holds anything: a whole
/// column is clipped to its used rows before it reaches here, so
/// `COUNTBLANK(A:A)` answers about the used range rather than about a million
/// cells.
pub fn countblank(args: &[Arg]) -> Value {
    let mut n = 0usize;
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        n += flat
            .iter()
            .filter(|v| matches!(v, Value::Blank) || matches!(v, Value::Text(t) if t.is_empty()))
            .count();
    }
    count_value(n)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a sheet cannot hold enough cells to lose a digit here"
)]
fn count_value(n: usize) -> Value {
    Value::Number(n as f64)
}

/// `COUNTIF(range, criterion)`
pub fn countif(args: &[Arg]) -> Value {
    let [range, criterion] = args else {
        return Value::Error(CellError::Value);
    };
    conditional(&[range, criterion], None, |kept, _| count_value(kept.len()))
}

/// `COUNTIFS(range1, criterion1, ...)`
pub fn countifs(args: &[Arg]) -> Value {
    if args.is_empty() || !args.len().is_multiple_of(2) {
        return Value::Error(CellError::Value);
    }
    let pairs: Vec<&Arg> = args.iter().collect();
    conditional(&pairs, None, |kept, _| count_value(kept.len()))
}

/// `SUMIF(range, criterion, [sum_range])`
pub fn sumif(args: &[Arg]) -> Value {
    one_condition(args, |ns| {
        Value::Number(ns.iter().fold(0.0, |acc, n| acc + n))
    })
}

/// `SUMIFS(sum_range, range1, criterion1, ...)`
pub fn sumifs(args: &[Arg]) -> Value {
    many_conditions(args, |ns| {
        Value::Number(ns.iter().fold(0.0, |acc, n| acc + n))
    })
}

/// `AVERAGEIF(range, criterion, [average_range])`
pub fn averageif(args: &[Arg]) -> Value {
    one_condition(args, mean)
}

/// `AVERAGEIFS(average_range, range1, criterion1, ...)`
pub fn averageifs(args: &[Arg]) -> Value {
    many_conditions(args, mean)
}

/// `MEDIANIF(range, criteria, [median_range])` - the median of the values a
/// criterion keeps.
///
/// Undocumented by Microsoft and a stub (`Functions::DUMMY`), so
/// there is no oracle for it; it is spelled here the way `AVERAGEIF` is,
/// because that is the function it mirrors.
pub fn medianif(args: &[Arg]) -> Value {
    one_condition(args, middle)
}

/// The median of the numbers that were kept, or `#NUM!` when none were -
/// which is what `MEDIAN` itself answers for nothing.
fn middle(ns: &[f64]) -> Value {
    if ns.is_empty() {
        return Value::Error(CellError::Num);
    }
    let mut ns = ns.to_vec();
    ns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = ns.len() / 2;
    Value::Number(if ns.len().is_multiple_of(2) {
        f64::midpoint(ns[mid - 1], ns[mid])
    } else {
        ns[mid]
    })
}

/// The average of the numbers that were kept, or `#DIV/0!` when none were.
fn mean(ns: &[f64]) -> Value {
    if ns.is_empty() {
        return Value::Error(CellError::Div0);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a count that large cannot be reached"
    )]
    let count = ns.len() as f64;
    Value::Number(ns.iter().sum::<f64>() / count)
}

/// Shared body of `SUMIF` and `AVERAGEIF`, whose value range is optional and
/// comes last.
fn one_condition(args: &[Arg], body: fn(&[f64]) -> Value) -> Value {
    let (range, criterion, values) = match args {
        [r, c] => (r, c, r),
        [r, c, v] => (r, c, v),
        _ => return Value::Error(CellError::Value),
    };
    conditional(&[range, criterion], Some(values), |kept, values| {
        body(&numbers_at(&kept, values))
    })
}

/// Shared body of `SUMIFS` and `AVERAGEIFS`, whose value range comes first.
fn many_conditions(args: &[Arg], body: fn(&[f64]) -> Value) -> Value {
    let Some((values, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    if rest.is_empty() || !rest.len().is_multiple_of(2) {
        return Value::Error(CellError::Value);
    }
    let pairs: Vec<&Arg> = rest.iter().collect();
    conditional(&pairs, Some(values), |kept, values| {
        body(&numbers_at(&kept, values))
    })
}

/// Applies the criteria and hands the surviving positions to a body.
///
/// The value range, when there is one, is walked in step with the criteria
/// ranges: what matters is the position, not the value itself.
fn conditional(
    pairs: &[&Arg],
    values: Option<&Arg>,
    body: impl Fn(Vec<usize>, &[&Value]) -> Value,
) -> Value {
    let Some(first) = pairs.first() else {
        return Value::Error(CellError::Value);
    };
    let len = cells(first).len();
    let keep = match selected(pairs, len) {
        Ok(keep) => keep,
        Err(e) => return Value::Error(e),
    };
    let positions: Vec<usize> = keep
        .iter()
        .enumerate()
        .filter_map(|(i, on)| on.then_some(i))
        .collect();
    let values = values.map(cells).unwrap_or_default();
    body(positions, &values)
}

/// The numbers at the given positions of a range, skipping anything else.
fn numbers_at(positions: &[usize], values: &[&Value]) -> Vec<f64> {
    positions
        .iter()
        .filter_map(|i| match values.get(*i) {
            Some(Value::Number(n)) => Some(*n),
            _ => None,
        })
        .collect()
}

/// `LARGE(array, k)` - the k-th largest number.
pub fn large(args: &[Arg]) -> Value {
    nth(args, true)
}

/// `SMALL(array, k)` - the k-th smallest.
pub fn small(args: &[Arg]) -> Value {
    nth(args, false)
}

/// Shared body of `LARGE` and `SMALL`.
fn nth(args: &[Arg], from_top: bool) -> Value {
    let [array, k] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(mut ns), Ok(k)) = (aggregate_numbers(std::slice::from_ref(array)), k.number()) else {
        return super::read_error(&[k.number()]);
    };
    if ns.is_empty() || k < 1.0 {
        return Value::Error(CellError::Num);
    }
    ns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked at least 1; a larger k is caught as #NUM! below"
    )]
    let index = k.trunc() as usize - 1;
    let index = if from_top {
        match ns.len().checked_sub(index + 1) {
            Some(i) => i,
            None => return Value::Error(CellError::Num),
        }
    } else {
        index
    };
    ns.get(index)
        .map_or(Value::Error(CellError::Num), |n| Value::Number(*n))
}

/// `RANK(number, range, [order])` - 0 or omitted ranks downwards.
pub fn rank(args: &[Arg]) -> Value {
    let (number, range, ascending) = match args {
        [n, r] => (n.number(), r, Ok(0.0)),
        [n, r, o] => (n.number(), r, o.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(number), Ok(order)) = (number, ascending) else {
        return super::read_error(&[number, ascending]);
    };
    let Ok(ns) = aggregate_numbers(std::slice::from_ref(range)) else {
        return Value::Error(CellError::Value);
    };
    if !ns.iter().any(|n| (n - number).abs() < f64::EPSILON) {
        return Value::Error(CellError::Na);
    }
    let ahead = ns
        .iter()
        .filter(|n| {
            if order == 0.0 {
                **n > number
            } else {
                **n < number
            }
        })
        .count();
    count_value(ahead + 1)
}

/// `STDEV(number1, ...)` - over a sample.
pub fn stdev(args: &[Arg]) -> Value {
    spread(args, true, true)
}

/// `STDEVP(number1, ...)` - over the whole population.
pub fn stdevp(args: &[Arg]) -> Value {
    spread(args, false, true)
}

/// `VAR(number1, ...)` - over a sample.
pub fn var(args: &[Arg]) -> Value {
    spread(args, true, false)
}

/// `VARP(number1, ...)` - over the whole population.
pub fn varp(args: &[Arg]) -> Value {
    spread(args, false, false)
}

/// Shared body of the spread functions: a sample divides by one less than the
/// count, a population by the count itself.
fn spread(args: &[Arg], sample: bool, root: bool) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    let wanted = if sample { 2 } else { 1 };
    if ns.len() < wanted {
        return Value::Error(CellError::Div0);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a count that large cannot be reached"
    )]
    let count = ns.len() as f64;
    let mean = ns.iter().sum::<f64>() / count;
    let sum_squares: f64 = ns.iter().map(|n| (n - mean) * (n - mean)).sum();
    let divisor = if sample { count - 1.0 } else { count };
    let variance = sum_squares / divisor;
    Value::Number(if root { variance.sqrt() } else { variance })
}

/// `AVEDEV(number1, ...)` - the mean distance from the mean.
pub fn avedev(args: &[Arg]) -> Value {
    around_mean(args, |ns, mean, count| {
        ns.iter().map(|n| (n - mean).abs()).sum::<f64>() / count
    })
}

/// `DEVSQ(number1, ...)` - the sum of the squared distances from the mean.
pub fn devsq(args: &[Arg]) -> Value {
    around_mean(args, |ns, mean, _| {
        ns.iter().map(|n| (n - mean) * (n - mean)).sum()
    })
}

/// Shared body of the two that measure spread around the mean directly.
fn around_mean(args: &[Arg], body: impl Fn(&[f64], f64, f64) -> f64) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    if ns.is_empty() {
        return Value::Error(CellError::Num);
    }
    let count = count_of(&ns);
    let mean = ns.iter().sum::<f64>() / count;
    Value::Number(body(&ns, mean, count))
}

/// `GEOMEAN(number1, ...)` - the n-th root of the product, which needs every
/// number to be positive.
pub fn geomean(args: &[Arg]) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    if ns.is_empty() || ns.iter().any(|n| *n <= 0.0) {
        return Value::Error(CellError::Num);
    }
    // Summed as logarithms: the product of a long column overflows long before
    // its geometric mean does.
    let logs: f64 = ns.iter().map(|n| n.ln()).sum();
    Value::Number((logs / count_of(&ns)).exp())
}

/// `HARMEAN(number1, ...)` - the reciprocal of the mean of the reciprocals.
pub fn harmean(args: &[Arg]) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    if ns.is_empty() || ns.iter().any(|n| *n <= 0.0) {
        return Value::Error(CellError::Num);
    }
    let sum: f64 = ns.iter().map(|n| 1.0 / n).sum();
    Value::Number(count_of(&ns) / sum)
}

/// `SKEW(number1, ...)` - how lopsided the distribution is, from a sample.
pub fn skew(args: &[Arg]) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    let count = count_of(&ns);
    if ns.len() < 3 {
        return Value::Error(CellError::Div0);
    }
    let mean = ns.iter().sum::<f64>() / count;
    let deviation = sample_deviation(&ns, mean, count);
    if deviation == 0.0 {
        return Value::Error(CellError::Div0);
    }
    let sum: f64 = ns.iter().map(|n| ((n - mean) / deviation).powi(3)).sum();
    Value::Number(sum * count / ((count - 1.0) * (count - 2.0)))
}

/// `SKEW.P(number1, ...)` - the same for a whole population.
pub fn skewp(args: &[Arg]) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    if ns.is_empty() {
        return Value::Error(CellError::Div0);
    }
    let count = count_of(&ns);
    let mean = ns.iter().sum::<f64>() / count;
    let variance = ns.iter().map(|n| (n - mean) * (n - mean)).sum::<f64>() / count;
    if variance == 0.0 {
        return Value::Error(CellError::Div0);
    }
    let deviation = variance.sqrt();
    let sum: f64 = ns.iter().map(|n| ((n - mean) / deviation).powi(3)).sum();
    Value::Number(sum / count)
}

/// `KURT(number1, ...)` - how heavy the tails are, against a normal curve.
pub fn kurt(args: &[Arg]) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    if ns.len() < 4 {
        return Value::Error(CellError::Div0);
    }
    let count = count_of(&ns);
    let mean = ns.iter().sum::<f64>() / count;
    let deviation = sample_deviation(&ns, mean, count);
    if deviation == 0.0 {
        return Value::Error(CellError::Div0);
    }
    let sum: f64 = ns.iter().map(|n| ((n - mean) / deviation).powi(4)).sum();
    let scale = count * (count + 1.0) / ((count - 1.0) * (count - 2.0) * (count - 3.0));
    let correction = 3.0 * (count - 1.0).powi(2) / ((count - 2.0) * (count - 3.0));
    Value::Number(sum * scale - correction)
}

/// The sample standard deviation, which the shape statistics divide by.
fn sample_deviation(ns: &[f64], mean: f64, count: f64) -> f64 {
    (ns.iter().map(|n| (n - mean) * (n - mean)).sum::<f64>() / (count - 1.0)).sqrt()
}

/// `MODE(number1, ...)`, and `MODE.SNGL`, which is the same function.
///
/// The most common number; where several tie, the one that appears first.
pub fn mode(args: &[Arg]) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    let mut best: Option<(f64, usize)> = None;
    for candidate in &ns {
        // The same number, exactly: `MODE` counts repeats, not near misses.
        let seen = ns
            .iter()
            .filter(|n| n.total_cmp(candidate) == core::cmp::Ordering::Equal)
            .count();
        // Strictly greater, so a tie is settled by whichever came first.
        if seen > 1 && best.is_none_or(|(_, most)| seen > most) {
            best = Some((*candidate, seen));
        }
    }
    // Excel has no mode for a list where nothing repeats.
    best.map_or(Value::Error(CellError::Na), |(n, _)| Value::Number(n))
}

/// `TRIMMEAN(array, percent)` - the mean after the extremes are dropped.
pub fn trimmean(args: &[Arg]) -> Value {
    let Some((percent, rest)) = args.split_last() else {
        return Value::Error(CellError::Value);
    };
    let Ok(percent) = percent.number() else {
        return Value::Error(CellError::Value);
    };
    if !(0.0..=1.0).contains(&percent) {
        return Value::Error(CellError::Num);
    }
    let mut ns = match aggregate_numbers(rest) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    if ns.is_empty() {
        return Value::Error(CellError::Num);
    }
    ns.sort_by(f64::total_cmp);
    // The count to drop is rounded down and then split between the two ends,
    // so the same number goes from each.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a fraction of a count, floored, is a small non-negative number"
    )]
    let discard = (count_of(&ns) * percent / 2.0).floor() as usize;
    if discard * 2 >= ns.len() {
        return Value::Error(CellError::Num);
    }
    mean(&ns[discard..ns.len() - discard])
}

/// `STANDARDIZE(value, mean, deviation)` - how many deviations from the mean.
pub fn standardize(args: &[Arg]) -> Value {
    let [x, mean, deviation] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(x), Ok(mean), Ok(deviation)) = (x.number(), mean.number(), deviation.number()) else {
        return Value::Error(CellError::Value);
    };
    if deviation <= 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number((x - mean) / deviation)
}

/// `PERMUTATIONA(n, k)` - arrangements of k from n with repeats allowed.
pub fn permutationa(args: &[Arg]) -> Value {
    let [n, k] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(n), Ok(k)) = (n.number(), k.number()) else {
        return Value::Error(CellError::Value);
    };
    let (n, k) = (n.trunc(), k.trunc());
    if n < 0.0 || k < 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(n.powf(k))
}

/// `PERCENTILE(array, k)`, and `PERCENTILE.INC`, which is the same function.
///
/// `k` runs from 0 to 1 over the sorted values, and a `k` landing between two
/// of them is interpolated.
pub fn percentile(args: &[Arg]) -> Value {
    let Some((k, rest)) = args.split_last() else {
        return Value::Error(CellError::Value);
    };
    let Ok(k) = k.number() else {
        return Value::Error(CellError::Value);
    };
    if !(0.0..=1.0).contains(&k) {
        return Value::Error(CellError::Num);
    }
    let Some(sorted) = sorted_numbers(rest) else {
        return Value::Error(CellError::Num);
    };
    Value::Number(at_position(&sorted, k * (count_of(&sorted) - 1.0)))
}

/// `PERCENTILE.EXC(array, k)` - the exclusive form, where `k` may not reach
/// either end: with n values it runs strictly between `1/(n+1)` and `n/(n+1)`.
pub fn percentile_exc(args: &[Arg]) -> Value {
    let Some((k, rest)) = args.split_last() else {
        return Value::Error(CellError::Value);
    };
    let Ok(k) = k.number() else {
        return Value::Error(CellError::Value);
    };
    let Some(sorted) = sorted_numbers(rest) else {
        return Value::Error(CellError::Num);
    };
    let count = count_of(&sorted);
    let position = k * (count + 1.0) - 1.0;
    if k <= 0.0 || k >= 1.0 || position < 0.0 || position > count - 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(at_position(&sorted, position))
}

/// `QUARTILE(array, quarter)`, and `QUARTILE.INC`: the percentile at a
/// quarter of the way along.
pub fn quartile(args: &[Arg]) -> Value {
    quarter_of(args, percentile)
}

/// `QUARTILE.EXC(array, quarter)`
pub fn quartile_exc(args: &[Arg]) -> Value {
    quarter_of(args, percentile_exc)
}

/// Shared body of the two quartile functions: a quarter is a percentile.
fn quarter_of(args: &[Arg], body: fn(&[Arg]) -> Value) -> Value {
    let Some((quarter, rest)) = args.split_last() else {
        return Value::Error(CellError::Value);
    };
    let Ok(quarter) = quarter.number() else {
        return Value::Error(CellError::Value);
    };
    let k = quarter.floor() / 4.0;
    if !(0.0..=1.0).contains(&k) {
        return Value::Error(CellError::Num);
    }
    let mut with_k: Vec<Arg> = rest.to_vec();
    with_k.push(Arg {
        value: Value::Number(k),
        reference: false,
    });
    body(&with_k)
}

/// `PERCENTRANK(array, value, [digits])`, and `PERCENTRANK.INC`.
pub fn percentrank(args: &[Arg]) -> Value {
    rank_within(args, false)
}

/// `PERCENTRANK.EXC(array, value, [digits])`
pub fn percentrank_exc(args: &[Arg]) -> Value {
    rank_within(args, true)
}

/// Shared body of the two percent-rank functions.
///
/// The inclusive form divides by `n - 1`, so the smallest value ranks 0 and
/// the largest 1; the exclusive form divides by `n + 1`, leaving room at both
/// ends. Both truncate rather than round, to three digits unless told.
fn rank_within(args: &[Arg], exclusive: bool) -> Value {
    let (array, value, digits) = match args {
        [a, v] => (a, v, Ok(3.0)),
        [a, v, d] => (a, v, d.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(value), Ok(digits)) = (value.number(), digits) else {
        return Value::Error(CellError::Value);
    };
    if digits < 1.0 {
        return Value::Error(CellError::Num);
    }
    let Some(sorted) = sorted_numbers(core::slice::from_ref(array)) else {
        return Value::Error(CellError::Num);
    };
    let count = count_of(&sorted);
    if value < sorted[0] || value > sorted[sorted.len() - 1] {
        return Value::Error(CellError::Na);
    }
    // Where the value sits among the sorted ones, interpolated when it falls
    // between two of them.
    let exact = sorted
        .iter()
        .position(|n| n.total_cmp(&value) == core::cmp::Ordering::Equal);
    let position = if let Some(at) = exact {
        count_of(&sorted[..at])
    } else {
        let above = sorted.iter().position(|n| *n > value).unwrap_or(0);
        let below = above - 1;
        count_of(&sorted[..below]) + (value - sorted[below]) / (sorted[above] - sorted[below])
    };
    let rank = if exclusive {
        (position + 1.0) / (count + 1.0)
    } else {
        position / (count - 1.0)
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a digit count Excel caps well below what i32 holds"
    )]
    let scale = 10f64.powi(digits.trunc() as i32);
    Value::Number((rank * scale).trunc() / scale)
}

/// `CORREL(y, x)`, and `PEARSON`, which is the same coefficient.
pub fn correl(args: &[Arg]) -> Value {
    paired_stat(args, |xs, ys| {
        let (sxx, syy, sxy) = moments(xs, ys);
        let denominator = (sxx * syy).sqrt();
        (denominator != 0.0).then(|| sxy / denominator)
    })
}

/// `RSQ(y, x)` - the square of the correlation.
pub fn rsq(args: &[Arg]) -> Value {
    paired_stat(args, |xs, ys| {
        let (sxx, syy, sxy) = moments(xs, ys);
        let denominator = sxx * syy;
        (denominator != 0.0).then(|| sxy * sxy / denominator)
    })
}

/// `COVAR(y, x)`, and `COVARIANCE.P`: the covariance of a whole population.
pub fn covar(args: &[Arg]) -> Value {
    paired_stat(args, |xs, ys| {
        let (_, _, sxy) = moments(xs, ys);
        Some(sxy / count_of(xs))
    })
}

/// `COVARIANCE.S(y, x)` - the same from a sample, so divided by one less.
pub fn covariance_s(args: &[Arg]) -> Value {
    paired_stat(args, |xs, ys| {
        let (_, _, sxy) = moments(xs, ys);
        let count = count_of(xs);
        (count > 1.0).then(|| sxy / (count - 1.0))
    })
}

/// `SLOPE(y, x)` - the gradient of the line of best fit.
pub fn slope(args: &[Arg]) -> Value {
    paired_stat(args, |xs, ys| {
        let (sxx, _, sxy) = moments(xs, ys);
        (sxx != 0.0).then(|| sxy / sxx)
    })
}

/// `INTERCEPT(y, x)` - where that line crosses the axis.
pub fn intercept(args: &[Arg]) -> Value {
    paired_stat(args, |xs, ys| {
        let (sxx, _, sxy) = moments(xs, ys);
        if sxx == 0.0 {
            return None;
        }
        let count = count_of(xs);
        let (mx, my) = (
            xs.iter().sum::<f64>() / count,
            ys.iter().sum::<f64>() / count,
        );
        Some(my - (sxy / sxx) * mx)
    })
}

/// `STEYX(y, x)` - the standard error of the predicted y.
pub fn steyx(args: &[Arg]) -> Value {
    paired_stat(args, |xs, ys| {
        let count = count_of(xs);
        if count < 3.0 {
            return None;
        }
        let (sxx, syy, sxy) = moments(xs, ys);
        if sxx == 0.0 {
            return None;
        }
        Some(((syy - sxy * sxy / sxx) / (count - 2.0)).sqrt())
    })
}

/// `FORECAST(x, y_values, x_values)`, and `FORECAST.LINEAR`: the line of best
/// fit read off at a given x.
pub fn forecast(args: &[Arg]) -> Value {
    let [x, ys, xs] = args else {
        return Value::Error(CellError::Value);
    };
    let Ok(x) = x.number() else {
        return Value::Error(CellError::Value);
    };
    paired_stat(&[ys.clone(), xs.clone()], |xs, ys| {
        let (sxx, _, sxy) = moments(xs, ys);
        if sxx == 0.0 {
            return None;
        }
        let count = count_of(xs);
        let (mx, my) = (
            xs.iter().sum::<f64>() / count,
            ys.iter().sum::<f64>() / count,
        );
        let gradient = sxy / sxx;
        Some(my - gradient * mx + gradient * x)
    })
}

/// `FISHER(x)` - the transformation that makes a correlation normal.
pub fn fisher(args: &[Arg]) -> Value {
    super::one(args, |x| {
        if x <= -1.0 || x >= 1.0 {
            return Value::Error(CellError::Num);
        }
        Value::Number(0.5 * ((1.0 + x) / (1.0 - x)).ln())
    })
}

/// `FISHERINV(y)` - its inverse.
pub fn fisherinv(args: &[Arg]) -> Value {
    super::one(args, |y| {
        let e = (2.0 * y).exp();
        Value::Number((e - 1.0) / (e + 1.0))
    })
}

/// `PHI(x)` - the height of the standard normal curve at x.
pub fn phi(args: &[Arg]) -> Value {
    super::one(args, |x| {
        Value::Number((-x * x / 2.0).exp() / (2.0 * core::f64::consts::PI).sqrt())
    })
}

/// The three sums the regression statistics are all built from.
fn moments(xs: &[f64], ys: &[f64]) -> (f64, f64, f64) {
    let count = count_of(xs);
    let (mx, my) = (
        xs.iter().sum::<f64>() / count,
        ys.iter().sum::<f64>() / count,
    );
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for (x, y) in xs.iter().zip(ys) {
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
        sxy += (x - mx) * (y - my);
    }
    (sxx, syy, sxy)
}

/// Applies a body to two ranges walked in step.
///
/// A pair where either side is not a number is dropped, which is what keeps a
/// header row or a gap from skewing the line.
fn paired_stat(args: &[Arg], body: impl Fn(&[f64], &[f64]) -> Option<f64>) -> Value {
    let [ys, xs] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = super::first_error(args) {
        return Value::Error(e);
    }
    let (ys, xs) = (cells(ys), cells(xs));
    if ys.len() != xs.len() {
        return Value::Error(CellError::Na);
    }
    let (mut x_kept, mut y_kept) = (Vec::new(), Vec::new());
    for (y, x) in ys.iter().zip(&xs) {
        if let (Value::Number(y), Value::Number(x)) = (y.scalar(), x.scalar()) {
            y_kept.push(*y);
            x_kept.push(*x);
        }
    }
    if x_kept.is_empty() {
        return Value::Error(CellError::Div0);
    }
    body(&x_kept, &y_kept).map_or(Value::Error(CellError::Div0), Value::Number)
}

/// The numbers of the arguments, sorted, or nothing when there are none.
fn sorted_numbers(args: &[Arg]) -> Option<Vec<f64>> {
    let mut ns = aggregate_numbers(args).ok()?;
    if ns.is_empty() {
        return None;
    }
    ns.sort_by(f64::total_cmp);
    Some(ns)
}

/// The value at a fractional position along a sorted list.
fn at_position(sorted: &[f64], position: f64) -> f64 {
    let floor = position.floor();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a position inside a list this crate could build"
    )]
    let base = floor as usize;
    // Landing exactly on a value needs no special case: the interpolation
    // below multiplies the step by zero, and the last value has no next one
    // to step towards.
    let next = sorted.get(base + 1).copied().unwrap_or(sorted[base]);
    sorted[base] + (next - sorted[base]) * (position - floor)
}

/// A count as a float, for the arithmetic every one of these does.
#[expect(
    clippy::cast_precision_loss,
    reason = "a count that large cannot be reached"
)]
fn count_of<T>(items: &[T]) -> f64 {
    items.len() as f64
}

/// `AVERAGEA(value1, ...)` - the mean counting text as zero.
pub fn averagea(args: &[Arg]) -> Value {
    with_text_as_zero(args, mean)
}

/// `MAXA(value1, ...)`
pub fn maxa(args: &[Arg]) -> Value {
    with_text_as_zero(args, |ns| extreme_of(ns, f64::max))
}

/// `MINA(value1, ...)`
pub fn mina(args: &[Arg]) -> Value {
    with_text_as_zero(args, |ns| extreme_of(ns, f64::min))
}

/// `STDEVA(value1, ...)`
pub fn stdeva(args: &[Arg]) -> Value {
    with_text_as_zero(args, |ns| spread_of(ns, true, true))
}

/// `STDEVPA(value1, ...)`
pub fn stdevpa(args: &[Arg]) -> Value {
    with_text_as_zero(args, |ns| spread_of(ns, false, true))
}

/// `VARA(value1, ...)`
pub fn vara(args: &[Arg]) -> Value {
    with_text_as_zero(args, |ns| spread_of(ns, true, false))
}

/// `VARPA(value1, ...)`
pub fn varpa(args: &[Arg]) -> Value {
    with_text_as_zero(args, |ns| spread_of(ns, false, false))
}

/// The shared body of the `A` forms.
///
/// Where `AVERAGE` skips the text and the booleans it finds in cells, these
/// count them: text is zero, `TRUE` is one and `FALSE` is zero. An empty cell
/// is still skipped by both.
fn with_text_as_zero(args: &[Arg], body: impl Fn(&[f64]) -> Value) -> Value {
    let mut ns = Vec::new();
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for v in flat {
            match v {
                Value::Error(e) => return Value::Error(*e),
                Value::Number(n) => ns.push(*n),
                Value::Bool(b) => ns.push(f64::from(u8::from(*b))),
                Value::Text(_) => ns.push(0.0),
                Value::Blank | Value::Array(_) | Value::Lambda(_) => {}
            }
        }
    }
    body(&ns)
}

/// The largest or smallest of a list already reduced to numbers.
fn extreme_of(ns: &[f64], pick: fn(f64, f64) -> f64) -> Value {
    if ns.is_empty() {
        return Value::Number(0.0);
    }
    Value::Number(ns.iter().copied().fold(ns[0], pick))
}

/// The variance or standard deviation of a list already reduced to numbers.
fn spread_of(ns: &[f64], sample: bool, root: bool) -> Value {
    if ns.len() < usize::from(sample) + 1 {
        return Value::Error(CellError::Div0);
    }
    let count = count_of(ns);
    let mean = ns.iter().sum::<f64>() / count;
    let squares: f64 = ns.iter().map(|n| (n - mean) * (n - mean)).sum();
    let variance = squares / if sample { count - 1.0 } else { count };
    Value::Number(if root { variance.sqrt() } else { variance })
}

/// `MAXIFS(values, range, criterion, ...)`
pub fn maxifs(args: &[Arg]) -> Value {
    extreme_ifs(args, f64::max)
}

/// `MINIFS(values, range, criterion, ...)`
pub fn minifs(args: &[Arg]) -> Value {
    extreme_ifs(args, f64::min)
}

/// Shared body of `MAXIFS` and `MINIFS`, which take their value range first
/// the way `SUMIFS` does.
fn extreme_ifs(args: &[Arg], pick: fn(f64, f64) -> f64) -> Value {
    let Some((values, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    if rest.is_empty() || !rest.len().is_multiple_of(2) {
        return Value::Error(CellError::Value);
    }
    let pairs: Vec<&Arg> = rest.iter().collect();
    conditional(&pairs, Some(values), |kept, values| {
        let ns = numbers_at(&kept, values);
        // Excel answers zero when nothing matched, rather than an error.
        extreme_of(&ns, pick)
    })
}

/// `RANK.AVG(number, ref, [order])` - like `RANK`, but ties share the average
/// of the places they take up rather than all taking the first of them.
pub fn rank_avg(args: &[Arg]) -> Value {
    let (needle, list, ascending) = match args {
        [n, r] => (n.number(), r, Ok(0.0)),
        [n, r, o] => (n.number(), r, o.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(needle), Ok(ascending)) = (needle, ascending) else {
        return Value::Error(CellError::Value);
    };
    let ns: Vec<f64> = cells(list)
        .into_iter()
        .filter_map(|v| match v.scalar() {
            Value::Number(n) => Some(*n),
            _ => None,
        })
        .collect();
    let ahead = ns
        .iter()
        .filter(|n| {
            if ascending == 0.0 {
                **n > needle
            } else {
                **n < needle
            }
        })
        .count();
    let tied = ns
        .iter()
        .filter(|n| n.total_cmp(&needle) == core::cmp::Ordering::Equal)
        .count();
    if tied == 0 {
        return Value::Error(CellError::Na);
    }
    // The places the tie takes up run from `ahead + 1` upwards; their average
    // is the first plus half of what is left.
    let first = count_of(&ns[..ahead]) + 1.0;
    Value::Number(first + (count_of(&ns[..tied]) - 1.0) / 2.0)
}

/// `MODE.MULT(number1, ...)` - every value tied for most common, as a column.
pub fn mode_mult(args: &[Arg]) -> Value {
    let ns = match aggregate_numbers(args) {
        Ok(ns) => ns,
        Err(e) => return Value::Error(e),
    };
    let count_same = |candidate: &f64| {
        ns.iter()
            .filter(|n| n.total_cmp(candidate) == core::cmp::Ordering::Equal)
            .count()
    };
    let most = ns.iter().map(count_same).max().unwrap_or(0);
    if most < 2 {
        return Value::Error(CellError::Na);
    }
    let mut out: Vec<Vec<Value>> = Vec::new();
    for (i, n) in ns.iter().enumerate() {
        // Each tied value once, at the place it first appeared.
        let first = ns[..i]
            .iter()
            .all(|earlier| earlier.total_cmp(n) != core::cmp::Ordering::Equal);
        if first && count_same(n) == most {
            out.push(vec![Value::Number(*n)]);
        }
    }
    Value::array(out)
}

/// `FREQUENCY(data, bins)` - how many of the data fall in each bin, as a
/// column one longer than the bins: the last row is everything above them.
pub fn frequency(args: &[Arg]) -> Value {
    let [data, bins] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = super::first_error(args) {
        return Value::Error(e);
    }
    let numbers = |arg: &Arg| -> Vec<f64> {
        cells(arg)
            .into_iter()
            .filter_map(|v| match v.scalar() {
                Value::Number(n) => Some(*n),
                _ => None,
            })
            .collect()
    };
    let data = numbers(data);
    let mut edges = numbers(bins);
    edges.sort_by(f64::total_cmp);
    let mut counts = vec![0.0; edges.len() + 1];
    for value in data {
        // The first bin whose edge the value does not exceed takes it.
        let bin = edges.iter().position(|edge| value <= *edge);
        counts[bin.unwrap_or(edges.len())] += 1.0;
    }
    Value::array(counts.into_iter().map(|n| vec![Value::Number(n)]).collect())
}

/// `PROB(values, probabilities, low, [high])` - the chance of landing in a
/// range, given the chance of each value.
pub fn prob(args: &[Arg]) -> Value {
    if let Some(e) = super::first_error(args) {
        return Value::Error(e);
    }
    let (values, chances, low, high) = match args {
        [v, c, l] => (v, c, l.number(), None),
        [v, c, l, h] => (v, c, l.number(), Some(h.number())),
        _ => return Value::Error(CellError::Value),
    };
    let Ok(low) = low else {
        return Value::Error(CellError::Value);
    };
    let high = match high {
        Some(Ok(h)) => h,
        Some(Err(_)) => return Value::Error(CellError::Value),
        // With one bound the range is that single value.
        None => low,
    };
    let (values, chances) = (cells(values), cells(chances));
    if values.len() != chances.len() {
        return Value::Error(CellError::Na);
    }
    let mut total = 0.0;
    let mut all = 0.0;
    for (value, chance) in values.iter().zip(&chances) {
        let (Value::Number(value), Value::Number(chance)) = (value.scalar(), chance.scalar())
        else {
            continue;
        };
        if *chance < 0.0 || *chance > 1.0 {
            return Value::Error(CellError::Num);
        }
        all += chance;
        if (low..=high).contains(value) {
            total += chance;
        }
    }
    // Excel insists the chances add up to one before it answers.
    if (all - 1.0).abs() > 1.0e-9 {
        return Value::Error(CellError::Num);
    }
    Value::Number(total)
}
