//! Least squares.
//!
//! Four functions, one method. `LINEST` fits `y = m1*x1 + ... + b` and returns
//! the coefficients; `TREND` returns what that line predicts; `LOGEST` and
//! `GROWTH` are the same two over `ln(y)`, so an exponential curve becomes a
//! straight line and comes back exponentiated.
//!
//! The fit is by the normal equations: `(X'X) c = X'y`, solved by
//! Gauss-Jordan with partial pivoting. That is the textbook method rather than
//! a QR decomposition, and it is what the reference implementations use — a
//! spreadsheet regression has a handful of columns, where the difference in
//! conditioning does not show.
//!
//! `LINEST` returns its coefficients backwards — the last predictor first and
//! the intercept last — because that is the order Excel lays them out in.

use super::{Arg, cells, first_error};
use crate::error::CellError;
use crate::formula::value::Value;

/// `LINEST(y, [x], [const], [stats])`
pub fn linest(args: &[Arg]) -> Value {
    fitted(args, false)
}

/// `LOGEST(y, [x], [const], [stats])` — the same over `ln(y)`.
pub fn logest(args: &[Arg]) -> Value {
    fitted(args, true)
}

/// Shared body of `LINEST` and `LOGEST`.
fn fitted(args: &[Arg], exponential: bool) -> Value {
    let Some(request) = Request::read(args, false) else {
        return Value::Error(CellError::Value);
    };
    let Some(fit) = request.fit(exponential) else {
        return Value::Error(CellError::Num);
    };

    // Backwards, and exponentiated for the curve, which is how Excel shows it.
    let show = |c: f64| if exponential { c.exp() } else { c };
    let mut row: Vec<Value> = fit
        .coefficients
        .iter()
        .skip(1)
        .rev()
        .map(|c| Value::Number(show(*c)))
        .collect();
    row.push(Value::Number(show(fit.coefficients[0])));

    if !request.stats {
        return Value::Array(vec![row]);
    }
    let width = row.len();
    let blank = |mut line: Vec<Value>| {
        // Excel pads the statistics rows with #N/A out to the width of the
        // first one, since only the first two hold a value per coefficient.
        while line.len() < width {
            line.push(Value::Error(CellError::Na));
        }
        line
    };
    let mut errors: Vec<Value> = fit
        .errors
        .iter()
        .skip(1)
        .rev()
        .map(|e| Value::Number(*e))
        .collect();
    errors.push(Value::Number(fit.errors[0]));
    Value::Array(vec![
        row,
        errors,
        blank(vec![
            Value::Number(fit.r_squared),
            Value::Number(fit.standard_error),
        ]),
        blank(vec![Value::Number(fit.f), Value::Number(fit.degrees)]),
        blank(vec![
            Value::Number(fit.regression_sum),
            Value::Number(fit.residual_sum),
        ]),
    ])
}

/// `TREND(y, [x], [new_x], [const])`
pub fn trend(args: &[Arg]) -> Value {
    predicted(args, false)
}

/// `GROWTH(y, [x], [new_x], [const])` — the same over `ln(y)`.
pub fn growth(args: &[Arg]) -> Value {
    predicted(args, true)
}

/// Shared body of `TREND` and `GROWTH`.
fn predicted(args: &[Arg], exponential: bool) -> Value {
    let Some(request) = Request::read(args, true) else {
        return Value::Error(CellError::Value);
    };
    let Some(fit) = request.fit(exponential) else {
        return Value::Error(CellError::Num);
    };
    // Without new values, the prediction is made at the original ones.
    let wanted = request.new_x.as_ref().unwrap_or(&request.x);
    let predictions: Vec<Value> = wanted
        .iter()
        .map(|row| {
            let mut total = fit.coefficients[0];
            for (c, value) in fit.coefficients[1..].iter().zip(row) {
                total += c * value;
            }
            Value::Number(if exponential { total.exp() } else { total })
        })
        .collect();
    // The answer takes the shape of the values it was asked about: a row of
    // new values gives a row back, a column gives a column.
    if request.new_x_is_row {
        Value::Array(vec![predictions])
    } else {
        Value::Array(predictions.into_iter().map(|v| vec![v]).collect())
    }
}

/// The arguments of the four, once read into shape.
struct Request {
    y: Vec<f64>,
    /// One row of predictor values per observation.
    x: Vec<Vec<f64>>,
    new_x: Option<Vec<Vec<f64>>>,
    /// Whether those new values were written across rather than down.
    new_x_is_row: bool,
    /// Whether the intercept is fitted or pinned at zero.
    intercept: bool,
    stats: bool,
}

impl Request {
    /// Reads the arguments.
    ///
    /// The two families lay them out differently past the second: `LINEST` has
    /// `const` third and `stats` fourth, while `TREND` has the new values third
    /// and `const` fourth.
    fn read(args: &[Arg], has_new_values: bool) -> Option<Self> {
        if first_error(args).is_some() || args.is_empty() || args.len() > 4 {
            return None;
        }
        let const_at = if has_new_values { 3 } else { 2 };
        let y = numbers(args.first()?);
        if y.is_empty() {
            return None;
        }
        // Left out, the predictors are 1, 2, 3, ... — the position in the list.
        let x = match args.get(1).filter(|a| !a.missing()) {
            Some(arg) => grid(arg, y.len())?,
            None => (1..=y.len())
                .map(|i| {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "a row count this crate could build"
                    )]
                    let i = i as f64;
                    vec![i]
                })
                .collect(),
        };
        if x.len() != y.len() {
            return None;
        }
        let predictors = x.first().map_or(0, Vec::len);
        // TREND and GROWTH take the new values where LINEST takes its flag.
        let new_x = if has_new_values {
            match args.get(2).filter(|a| !a.missing()) {
                Some(arg) => Some(grid_of_width(arg, predictors)?),
                None => None,
            }
        } else {
            None
        };
        // The answer takes the shape of what it was asked about: the new
        // values if there are any, and otherwise the observations themselves.
        let laid_out_in_a_row = |arg: Option<&Arg>| {
            arg.filter(|a| !a.missing())
                .is_some_and(|arg| matches!(&arg.value, Value::Array(rows) if rows.len() == 1))
        };
        let new_x_is_row = if args.get(2).is_some_and(|a| !a.missing()) {
            predictors == 1 && laid_out_in_a_row(args.get(2))
        } else {
            laid_out_in_a_row(args.first())
        };
        let flag = |at: usize| {
            args.get(at)
                .filter(|a| !a.missing())
                .map_or(Some(true), |a| a.value.boolean().ok())
        };
        Some(Self {
            y,
            x,
            new_x,
            new_x_is_row: has_new_values && new_x_is_row,
            intercept: flag(const_at)?,
            stats: !has_new_values && args.get(3).is_some_and(|a| a.value.boolean() == Ok(true)),
        })
    }

    /// Fits the model, taking logarithms first for the exponential forms.
    fn fit(&self, exponential: bool) -> Option<Fit> {
        let mut y = self.y.clone();
        if exponential {
            for value in &mut y {
                // An exponential curve cannot pass through zero or below it.
                if *value <= 0.0 {
                    return None;
                }
                *value = value.ln();
            }
        }
        least_squares(&y, &self.x, self.intercept)
    }
}

/// What a fit produced, including the statistics `LINEST` can report.
struct Fit {
    /// The intercept first, then one per predictor.
    coefficients: Vec<f64>,
    errors: Vec<f64>,
    r_squared: f64,
    standard_error: f64,
    f: f64,
    degrees: f64,
    regression_sum: f64,
    residual_sum: f64,
}

/// Fits `y` against the columns of `x` by the normal equations.
fn least_squares(y: &[f64], x: &[Vec<f64>], intercept: bool) -> Option<Fit> {
    let predictors = x.first()?.len();
    if predictors == 0 || x.iter().any(|row| row.len() != predictors) {
        return None;
    }
    // A column of ones makes the intercept just another coefficient. Pinned
    // through the origin, that column is simply absent — zeroing it instead
    // would leave a singular matrix rather than a smaller one.
    let width = predictors + usize::from(intercept);
    let design: Vec<Vec<f64>> = x
        .iter()
        .map(|row| {
            let mut line = Vec::with_capacity(width);
            if intercept {
                line.push(1.0);
            }
            line.extend(row.iter().copied());
            line
        })
        .collect();

    // The normal equations: X'X on the left, X'y on the right.
    let mut normal = vec![vec![0.0; width]; width];
    let mut right = vec![0.0; width];
    for (row, value) in design.iter().zip(y) {
        for i in 0..width {
            right[i] += row[i] * value;
            for j in 0..width {
                normal[i][j] += row[i] * row[j];
            }
        }
    }
    let inverse = invert_matrix(&normal)?;
    let solved: Vec<f64> = (0..width)
        .map(|i| (0..width).map(|j| inverse[i][j] * right[j]).sum())
        .collect();
    // The rest of the code wants the intercept in the first slot whether it
    // was fitted or pinned at zero.
    let mut coefficients = Vec::with_capacity(predictors + 1);
    if !intercept {
        coefficients.push(0.0);
    }
    coefficients.extend(solved.iter().copied());

    let count = count_of(y);
    let fitted = if intercept {
        &coefficients[..]
    } else {
        &coefficients[1..]
    };
    let predicted =
        |row: &[f64]| -> f64 { row.iter().zip(fitted).map(|(value, c)| value * c).sum() };
    let mean = y.iter().sum::<f64>() / count;
    let residual_sum: f64 = design
        .iter()
        .zip(y)
        .map(|(row, value)| {
            let error = value - predicted(row);
            error * error
        })
        .sum();
    let total: f64 = if intercept {
        y.iter().map(|value| (value - mean) * (value - mean)).sum()
    } else {
        y.iter().map(|value| value * value).sum()
    };
    let regression_sum = total - residual_sum;

    #[expect(
        clippy::cast_precision_loss,
        reason = "a predictor count this crate could build"
    )]
    let k = predictors as f64;
    let degrees = count - k - if intercept { 1.0 } else { 0.0 };
    if degrees < 0.0 {
        return None;
    }
    // With no degrees of freedom left there is nothing to estimate the spread
    // from, but the coefficients themselves are still the answer.
    let variance = if degrees > 0.0 {
        residual_sum / degrees
    } else {
        0.0
    };
    let standard_error = variance.sqrt();
    let mut errors = Vec::with_capacity(predictors + 1);
    if !intercept {
        errors.push(0.0);
    }
    errors.extend((0..width).map(|i| (variance * inverse[i][i]).max(0.0).sqrt()));

    Some(Fit {
        coefficients,
        errors,
        r_squared: if total == 0.0 {
            1.0
        } else {
            regression_sum / total
        },
        standard_error,
        f: if residual_sum == 0.0 {
            f64::INFINITY
        } else {
            (regression_sum / k) / variance
        },
        degrees,
        regression_sum,
        residual_sum,
    })
}

/// Inverts a square matrix by Gauss-Jordan with partial pivoting.
pub(crate) fn invert_matrix(matrix: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = matrix.len();
    // The matrix beside an identity; row-reducing the left half turns the
    // right half into the inverse.
    let mut work: Vec<Vec<f64>> = matrix
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut line = row.clone();
            line.extend((0..n).map(|j| f64::from(u8::from(i == j))));
            line
        })
        .collect();

    for column in 0..n {
        // The largest remaining entry makes the steadiest pivot.
        let pivot =
            (column..n).max_by(|a, b| work[*a][column].abs().total_cmp(&work[*b][column].abs()))?;
        if work[pivot][column].abs() < 1.0e-300 {
            // Singular: the predictors are not independent.
            return None;
        }
        work.swap(column, pivot);
        let scale = work[column][column];
        for value in &mut work[column] {
            *value /= scale;
        }
        for row in 0..n {
            if row == column {
                continue;
            }
            let factor = work[row][column];
            if factor == 0.0 {
                continue;
            }
            // The pivot row is read while another is written, so the two have
            // to be borrowed apart.
            let (before, after) = work.split_at_mut(row.max(column));
            let (target, source) = if row > column {
                (&mut after[0], &before[column])
            } else {
                (&mut before[row], &after[0])
            };
            for (value, pivot) in target.iter_mut().zip(source.iter()) {
                *value -= factor * pivot;
            }
        }
    }
    Some(work.into_iter().map(|row| row[n..].to_vec()).collect())
}

/// The numbers of an argument, in order.
fn numbers(arg: &Arg) -> Vec<f64> {
    cells(arg)
        .into_iter()
        .filter_map(|v| match v.scalar() {
            Value::Number(n) => Some(*n),
            _ => None,
        })
        .collect()
}

/// An argument as one row of predictors per observation.
///
/// A single row or column is one predictor; a rectangle is one per column,
/// unless it is wider than it is tall, which is how Excel lays several
/// predictors out along a row.
fn grid(arg: &Arg, observations: usize) -> Option<Vec<Vec<f64>>> {
    let Value::Array(rows) = &arg.value else {
        return Some(numbers(arg).into_iter().map(|n| vec![n]).collect());
    };
    let (height, width) = (rows.len(), rows.first().map_or(0, Vec::len));
    if height == observations && width >= 1 {
        return Some(
            rows.iter()
                .map(|row| row.iter().filter_map(as_number).collect())
                .collect(),
        );
    }
    if width == observations {
        // Laid out along the rows, so each column is one observation.
        return Some(
            (0..width)
                .map(|c| rows.iter().filter_map(|row| as_number(&row[c])).collect())
                .collect(),
        );
    }
    None
}

/// The new values of `TREND` and `GROWTH`, as rows of the same width.
fn grid_of_width(arg: &Arg, predictors: usize) -> Option<Vec<Vec<f64>>> {
    let flat = numbers(arg);
    if predictors == 0 || !flat.len().is_multiple_of(predictors) {
        return None;
    }
    Some(flat.chunks(predictors).map(<[f64]>::to_vec).collect())
}

/// A value as a number, for the grid readers.
fn as_number(value: &Value) -> Option<f64> {
    match value.scalar() {
        Value::Number(n) => Some(*n),
        _ => None,
    }
}

/// A count as a float.
#[expect(
    clippy::cast_precision_loss,
    reason = "a count that large cannot be reached"
)]
fn count_of<T>(items: &[T]) -> f64 {
    items.len() as f64
}
