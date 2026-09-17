//! `FORECAST.ETS` and its three companions: exponential smoothing over a
//! timeline.
//!
//! The model is the one Microsoft names in the documentation, AAA: additive
//! error, additive trend, additive seasonality, better known as additive
//! Holt-Winters. Three smoothing weights drive it, and Excel picks them itself
//! rather than asking; so do we, by minimising the sum of squared one-step
//! errors.
//!
//! What Excel does not publish is which optimiser it runs, so our weights are
//! ours and the last digits of a forecast will differ from its. The shape does
//! not: on a series that is exactly a line the forecast continues that line,
//! and on one that is exactly periodic it repeats the period. Both are pinned
//! by tests, and they are the strongest check available without an oracle.

#![expect(
    clippy::cast_precision_loss,
    reason = "these counts are cells of a sheet, far below 2^53"
)]

use super::{Arg, cells, first_error};
use crate::error::CellError;
use crate::formula::functions::distributions::standard_inverse;
use crate::formula::value::Value;

/// The longest grid we will build out of a timeline. A file is free to claim a
/// step of one second across a decade; we are not obliged to allocate for it.
const MAX_POINTS: usize = 1 << 20;

/// The longest period Excel accepts, and the longest we look for.
const MAX_SEASON: usize = 8760;

/// A grid index out of a float, refusing anything that is not one. Both the
/// timeline and the target date come from a file, so neither is trusted to
/// land inside `usize` on its own.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the bounds are checked on the line above"
)]
fn index(v: f64) -> Result<usize, CellError> {
    let v = v.round();
    if !v.is_finite() || v < 0.0 || v > MAX_POINTS as f64 {
        return Err(CellError::Num);
    }
    Ok(v as usize)
}

/// A timeline reduced to what the smoother needs: values on an even grid.
struct Series {
    /// The distance between two neighbouring points.
    step: f64,
    /// The first point of the timeline.
    first: f64,
    /// One value per grid point, gaps already filled.
    y: Vec<f64>,
}

/// How duplicate timeline points are collapsed, the `aggregation` argument.
fn aggregate(code: u8, mut xs: Vec<f64>) -> Option<f64> {
    if xs.is_empty() {
        // Only COUNT and COUNTA have an answer for nothing at all, and both
        // answer zero; the rest leave the point missing.
        return (code == 2 || code == 3).then_some(0.0);
    }
    let n = xs.len() as f64;
    Some(match code {
        2 | 3 => n,
        4 => xs.iter().copied().fold(f64::MIN, f64::max),
        5 => {
            xs.sort_by(f64::total_cmp);
            let half = xs.len() / 2;
            if xs.len().is_multiple_of(2) {
                f64::midpoint(xs[half - 1], xs[half])
            } else {
                xs[half]
            }
        }
        6 => xs.iter().copied().fold(f64::MAX, f64::min),
        7 => xs.iter().sum(),
        _ => xs.iter().sum::<f64>() / n,
    })
}

impl Series {
    /// Reads the `values` and `timeline` pair into an even grid.
    ///
    /// The timeline carries the two rules Excel states: its points sit at a
    /// constant step, and no more than 30% of the grid may be missing.
    fn read(
        values: &Arg,
        timeline: &Arg,
        completion: u8,
        aggregation: u8,
    ) -> Result<Self, CellError> {
        if let Some(e) = first_error(&[values.clone(), timeline.clone()]) {
            return Err(e);
        }
        let (vs, ts) = (cells(values), cells(timeline));
        if vs.len() != ts.len() || vs.is_empty() {
            return Err(CellError::Na);
        }

        let mut points: Vec<(f64, Option<f64>)> = Vec::with_capacity(ts.len());
        for (t, v) in ts.iter().zip(&vs) {
            let &Value::Number(t) = t.scalar() else {
                return Err(CellError::Value);
            };
            let v = match v.scalar() {
                Value::Number(v) => Some(*v),
                Value::Blank => None,
                _ => return Err(CellError::Value),
            };
            points.push((t, v));
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));

        // Points that share a timestamp become one, by the aggregation asked
        // for. The whole group is gathered before it is collapsed: an average
        // taken one value at a time is not an average.
        let mut known: Vec<(f64, Option<f64>)> = Vec::new();
        let mut group: Vec<f64> = Vec::new();
        let mut at = f64::NAN;
        for (t, v) in points {
            if t.total_cmp(&at) != core::cmp::Ordering::Equal {
                if at.is_finite() {
                    known.push((at, aggregate(aggregation, core::mem::take(&mut group))));
                }
                at = t;
                group.clear();
            }
            group.extend(v);
        }
        if at.is_finite() {
            known.push((at, aggregate(aggregation, group)));
        }
        if known.len() < 2 {
            return Err(CellError::Num);
        }

        let first = known[0].0;
        let step = known
            .windows(2)
            .map(|w| w[1].0 - w[0].0)
            .filter(|d| *d > 0.0)
            .fold(f64::MAX, f64::min);
        if !step.is_finite() || step <= 0.0 {
            return Err(CellError::Num);
        }
        let span = (known[known.len() - 1].0 - first) / step;
        if !span.is_finite() || span + 1.0 > MAX_POINTS as f64 {
            return Err(CellError::Num);
        }

        let mut grid = vec![None; index(span)? + 1];
        for (t, v) in &known {
            let offset = (t - first) / step;
            // A timeline whose points are not whole steps apart is the one
            // shape Excel refuses outright.
            if (offset - offset.round()).abs() > 1e-6 {
                return Err(CellError::Num);
            }
            let Some(slot) = grid.get_mut(index(offset)?) else {
                return Err(CellError::Num);
            };
            *slot = *v;
        }

        let missing = grid.iter().filter(|v| v.is_none()).count();
        if missing * 10 > grid.len() * 3 {
            return Err(CellError::Num);
        }
        Ok(Self {
            step,
            first,
            y: fill(&grid, completion),
        })
    }

    /// The grid position a date falls on, counted from the first point.
    fn position(&self, at: f64) -> f64 {
        (at - self.first) / self.step
    }
}

/// Fills the gaps of a grid: linear interpolation, or zeros when asked.
fn fill(grid: &[Option<f64>], completion: u8) -> Vec<f64> {
    if completion == 0 {
        return grid.iter().map(|v| v.unwrap_or(0.0)).collect();
    }
    let mut out: Vec<f64> = grid.iter().map(|v| v.unwrap_or(f64::NAN)).collect();
    let mut i = 0;
    while i < out.len() {
        if !out[i].is_nan() {
            i += 1;
            continue;
        }
        // The first and last points of the grid are always known, so a gap
        // always has a neighbour on each side.
        let start = i;
        let mut end = i;
        while end < out.len() && out[end].is_nan() {
            end += 1;
        }
        let (left, right) = (
            out[start - 1],
            out.get(end).copied().unwrap_or(out[start - 1]),
        );
        let span = (end - start + 1) as f64;
        for (k, slot) in out[start..end].iter_mut().enumerate() {
            *slot = left + (right - left) * (k + 1) as f64 / span;
        }
        i = end;
    }
    out
}

/// The fitted model: its weights, its end state and what it got wrong on the
/// way there.
struct Fit {
    alpha: f64,
    beta: f64,
    gamma: f64,
    period: usize,
    level: f64,
    trend: f64,
    season: Vec<f64>,
    /// One-step errors, and the grid position each belongs to.
    residuals: Vec<(usize, f64)>,
}

impl Fit {
    /// Runs the recursion once with the weights given.
    fn run(y: &[f64], period: usize, alpha: f64, beta: f64, gamma: f64) -> Self {
        let (mut level, mut trend, mut season, start) = initial(y, period);
        let mut residuals = Vec::with_capacity(y.len().saturating_sub(start));
        for (t, &value) in y.iter().enumerate().skip(start) {
            let phase = if period == 0 { 0 } else { t % period };
            let seasonal = season.get(phase).copied().unwrap_or(0.0);
            let forecast = level + trend + seasonal;
            residuals.push((t, value - forecast));

            let previous = level;
            level = alpha * (value - seasonal) + (1.0 - alpha) * (level + trend);
            trend = beta * (level - previous) + (1.0 - beta) * trend;
            if period > 0 {
                season[phase] = gamma * (value - previous - trend) + (1.0 - gamma) * seasonal;
            }
        }
        Self {
            alpha,
            beta,
            gamma,
            period,
            level,
            trend,
            season,
            residuals,
        }
    }

    /// The sum of squared one-step errors, which is what the weights are
    /// chosen to make small.
    fn error(&self) -> f64 {
        self.residuals.iter().map(|(_, r)| r * r).sum()
    }

    /// The forecast `steps` grid points past the last one, where the last
    /// point itself is step zero.
    fn ahead(&self, last: usize, steps: f64) -> f64 {
        let seasonal = if self.period == 0 {
            0.0
        } else {
            let phase = index(last as f64 + steps).unwrap_or(0) % self.period;
            self.season.get(phase).copied().unwrap_or(0.0)
        };
        self.level + steps * self.trend + seasonal
    }
}

/// Level, trend and seasonal figures to start the recursion from, plus the
/// grid position it may start at: the first period goes into the estimate, so
/// scoring it again would be marking the model on its own homework.
fn initial(y: &[f64], period: usize) -> (f64, f64, Vec<f64>, usize) {
    if period < 2 || y.len() < 2 * period {
        let trend = if y.len() > 1 { y[1] - y[0] } else { 0.0 };
        return (y[0], trend, Vec::new(), 1);
    }
    let periods = y.len() / period;
    let mean = |k: usize| y[k * period..(k + 1) * period].iter().sum::<f64>() / period as f64;
    let level = mean(0);
    let trend = (mean(1) - level) / period as f64;
    let season = (0..period)
        .map(|phase| {
            let sum: f64 = (0..periods).map(|k| y[k * period + phase] - mean(k)).sum();
            sum / periods as f64
        })
        .collect();
    (level, trend, season, period)
}

/// The period the series repeats on, or zero when it does not repeat.
///
/// The test is the autocorrelation of the series with its own trend taken out.
///
// ponytail: one autocorrelation peak, not a model comparison. Excel weighs
// candidate periods by how well each fits; upgrade to fitting every candidate
// and keeping the smallest AICc if a real workbook disagrees with us here.
fn detect_period(y: &[f64]) -> usize {
    let n = y.len();
    if n < 6 {
        return 0;
    }
    // Take out the straight line, so that a rising series does not correlate
    // with itself at every lag.
    let count = n as f64;
    let mean_t = (count - 1.0) / 2.0;
    let mean_y = y.iter().sum::<f64>() / count;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (t, value) in y.iter().enumerate() {
        let dt = t as f64 - mean_t;
        sxy += dt * (value - mean_y);
        sxx += dt * dt;
    }
    let gradient = if sxx == 0.0 { 0.0 } else { sxy / sxx };
    let flat: Vec<f64> = y
        .iter()
        .enumerate()
        .map(|(t, value)| value - (mean_y + gradient * (t as f64 - mean_t)))
        .collect();

    let variance: f64 = flat.iter().map(|v| v * v).sum();
    if variance <= 0.0 {
        return 0;
    }
    let mut best = (0usize, 0.0f64);
    for lag in 2..=(n / 2).min(MAX_SEASON) {
        let covariance: f64 = flat[lag..]
            .iter()
            .zip(&flat)
            .map(|(a, b)| a * b)
            .sum::<f64>()
            / variance;
        if covariance > best.1 {
            best = (lag, covariance);
        }
    }
    // Below this the peak is noise; Excel reports no seasonality rather than
    // fitting one that is not there.
    if best.1 < 0.3 { 0 } else { best.0 }
}

/// Fits the model, choosing the three weights by a coarse sweep and then a
/// finer one around the winner.
fn fit(y: &[f64], period: usize) -> Fit {
    let seasonal = period >= 2 && y.len() >= 2 * period;
    let mut best = Fit::run(y, if seasonal { period } else { 0 }, 0.5, 0.1, 0.1);
    let mut score = best.error();
    let mut centre = (0.5, 0.1, 0.1);
    for width in [0.4, 0.1, 0.025] {
        let axis = |c: f64| {
            [c - width, c, c + width]
                .into_iter()
                .filter(|v| (0.01..=0.99).contains(v))
                .collect::<Vec<_>>()
        };
        for &alpha in &axis(centre.0) {
            for &beta in &axis(centre.1) {
                // Without a season there is nothing for gamma to weigh, so
                // it stays put instead of being swept over.
                let gammas = if seasonal {
                    axis(centre.2)
                } else {
                    vec![centre.2]
                };
                for gamma in gammas {
                    let candidate =
                        Fit::run(y, if seasonal { period } else { 0 }, alpha, beta, gamma);
                    let error = candidate.error();
                    if error < score {
                        (score, centre, best) = (error, (alpha, beta, gamma), candidate);
                    }
                }
            }
        }
    }
    best
}

/// The optional tail every one of these functions ends with, in the order
/// `seasonality`, `data_completion`, `aggregation`. A caller passes the
/// arguments that remain after its own.
struct Options {
    seasonality: Option<usize>,
    completion: u8,
    aggregation: u8,
}

impl Options {
    fn read(tail: &[Arg], with_seasonality: bool) -> Result<Self, CellError> {
        let mut it = tail.iter();
        let seasonality = if with_seasonality {
            match it.next().map(Arg::number).transpose()? {
                None | Some(1.0) => None,
                Some(0.0) => Some(0),
                Some(n) if (2.0..=MAX_SEASON as f64).contains(&n) => Some(index(n.trunc())?),
                Some(_) => return Err(CellError::Num),
            }
        } else {
            None
        };
        let code = |v: Option<f64>, range: core::ops::RangeInclusive<f64>, fallback: u8| match v {
            None => Ok(fallback),
            Some(n) if range.contains(&n.trunc()) => {
                u8::try_from(index(n.trunc())?).map_err(|_| CellError::Num)
            }
            Some(_) => Err(CellError::Num),
        };
        let completion = code(it.next().map(Arg::number).transpose()?, 0.0..=1.0, 1)?;
        let aggregation = code(it.next().map(Arg::number).transpose()?, 1.0..=7.0, 1)?;
        if it.next().is_some() {
            return Err(CellError::Value);
        }
        Ok(Self {
            seasonality,
            completion,
            aggregation,
        })
    }
}

/// Reads the series and fits it, which every one of the four names starts with.
fn prepare(values: &Arg, timeline: &Arg, options: &Options) -> Result<(Series, Fit), CellError> {
    let series = Series::read(values, timeline, options.completion, options.aggregation)?;
    let period = options
        .seasonality
        .unwrap_or_else(|| detect_period(&series.y));
    let fitted = fit(&series.y, period);
    Ok((series, fitted))
}

/// Turns the error a helper returned into the value a formula returns.
fn done(result: Result<f64, CellError>) -> Value {
    result.map_or_else(Value::Error, Value::Number)
}

/// `FORECAST.ETS(target, values, timeline, [seasonality], [completion], [aggregation])`.
pub fn forecast_ets(args: &[Arg]) -> Value {
    let [target, values, timeline, tail @ ..] = args else {
        return Value::Error(CellError::Value);
    };
    done((|| {
        let options = Options::read(tail, true)?;
        let target = target.number()?;
        let (series, fitted) = prepare(values, timeline, &options)?;
        let position = series.position(target);
        if position < 0.0 {
            return Err(CellError::Num);
        }
        let last = series.y.len() - 1;
        Ok(fitted.ahead(last, position - last as f64))
    })())
}

/// `FORECAST.ETS.CONFINT(target, values, timeline, [confidence], [seasonality],
/// [completion], [aggregation])`: half the width of the prediction interval.
pub fn forecast_ets_confint(args: &[Arg]) -> Value {
    let [target, values, timeline, tail @ ..] = args else {
        return Value::Error(CellError::Value);
    };
    done((|| {
        let (confidence, rest) = match tail {
            [] => (0.95, tail),
            [level, rest @ ..] => (level.number()?, rest),
        };
        if !(0.0..1.0).contains(&confidence) || confidence == 0.0 {
            return Err(CellError::Num);
        }
        let options = Options::read(rest, true)?;
        let target = target.number()?;
        let (series, fitted) = prepare(values, timeline, &options)?;
        let position = series.position(target);
        if position < 0.0 {
            return Err(CellError::Num);
        }
        let last = series.y.len() - 1;
        let steps = index((position - last as f64).max(0.0))?;

        // The variance of a forecast made h steps out, for this model: each
        // step past the first adds the error it inherits from the ones before.
        let count = fitted.residuals.len();
        if count < 2 {
            return Err(CellError::Num);
        }
        let sigma2 = fitted.error() / count as f64;
        let inflation: f64 = (1..steps)
            .map(|j| {
                let seasonal = f64::from(fitted.period > 0 && j % fitted.period == 0);
                let c = fitted.alpha * (1.0 + j as f64 * fitted.beta) + fitted.gamma * seasonal;
                c * c
            })
            .sum();
        Ok(standard_inverse(f64::midpoint(1.0, confidence)) * (sigma2 * (1.0 + inflation)).sqrt())
    })())
}

/// `FORECAST.ETS.SEASONALITY(values, timeline, [completion], [aggregation])`.
pub fn forecast_ets_seasonality(args: &[Arg]) -> Value {
    let [values, timeline, tail @ ..] = args else {
        return Value::Error(CellError::Value);
    };
    done((|| {
        let options = Options::read(tail, false)?;
        let series = Series::read(values, timeline, options.completion, options.aggregation)?;
        Ok(detect_period(&series.y) as f64)
    })())
}

/// `FORECAST.ETS.STAT(values, timeline, statistic, [seasonality], [completion],
/// [aggregation])`: one of the eight numbers the fit produced.
pub fn forecast_ets_stat(args: &[Arg]) -> Value {
    let [values, timeline, statistic, tail @ ..] = args else {
        return Value::Error(CellError::Value);
    };
    done((|| {
        let which = statistic.number()?.trunc();
        if !(1.0..=8.0).contains(&which) {
            return Err(CellError::Num);
        }
        let options = Options::read(tail, true)?;
        let (series, fitted) = prepare(values, timeline, &options)?;
        let count = fitted.residuals.len() as f64;
        if count == 0.0 {
            return Err(CellError::Num);
        }
        let absolute: f64 = fitted.residuals.iter().map(|(_, r)| r.abs()).sum();
        Ok(match index(which)? {
            1 => fitted.alpha,
            2 => fitted.beta,
            3 => fitted.gamma,
            4 => mase(&series.y, absolute / count),
            5 => smape(&series.y, &fitted.residuals),
            6 => absolute / count,
            7 => (fitted.error() / count).sqrt(),
            _ => series.step,
        })
    })())
}

/// The mean absolute error against the error of predicting no change at all,
/// which is what makes it comparable between series.
fn mase(y: &[f64], mae: f64) -> f64 {
    let naive: f64 = y.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
    let count = y.len().saturating_sub(1) as f64;
    if naive == 0.0 || count == 0.0 {
        return 0.0;
    }
    mae / (naive / count)
}

/// The symmetric mean absolute percentage error: each miss against the size of
/// what was seen and what was said, so a large series does not dwarf it.
fn smape(y: &[f64], residuals: &[(usize, f64)]) -> f64 {
    if residuals.is_empty() {
        return 0.0;
    }
    let total: f64 = residuals
        .iter()
        .map(|&(t, r)| {
            let actual = y[t];
            let predicted = actual - r;
            let scale = f64::midpoint(actual.abs(), predicted.abs());
            if scale == 0.0 { 0.0 } else { r.abs() / scale }
        })
        .sum();
    total / residuals.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formula::value::Value;

    fn arg(values: &[f64]) -> Arg {
        Arg {
            value: Value::array(vec![values.iter().copied().map(Value::Number).collect()]),
            reference: true,
        }
    }

    fn number_arg(value: Value) -> Arg {
        Arg {
            value,
            reference: false,
        }
    }

    fn number(v: Value) -> f64 {
        match v {
            Value::Number(n) => n,
            other => panic!("expected a number, got {other:?}"),
        }
    }

    /// A series that is exactly a line has to be continued as that line: no
    /// smoothing weight can beat a model that is already right.
    #[test]
    fn a_straight_line_is_continued() {
        let times: Vec<f64> = (1..=20).map(f64::from).collect();
        let values: Vec<f64> = times.iter().map(|t| 3.0 * t + 7.0).collect();
        let got = forecast_ets(&[
            number_arg(Value::Number(25.0)),
            arg(&values),
            arg(&times),
            number_arg(Value::Number(0.0)),
        ]);
        assert!((number(got) - 82.0).abs() < 1e-6);
    }

    /// And one that is exactly periodic has to repeat, period found on its own.
    #[test]
    fn a_repeating_series_repeats() {
        let times: Vec<f64> = (1..=24).map(f64::from).collect();
        let values: Vec<f64> = (0..24).map(|i| [10.0, 20.0, 30.0, 40.0][i % 4]).collect();
        let season = forecast_ets_seasonality(&[arg(&values), arg(&times)]);
        assert!((number(season) - 4.0).abs() < 1e-9);

        let got = forecast_ets(&[number_arg(Value::Number(25.0)), arg(&values), arg(&times)]);
        assert!((number(got) - 10.0).abs() < 0.5);
    }

    /// The step of the timeline is statistic 8, and it is read off the data
    /// rather than assumed to be one.
    #[test]
    fn the_step_is_measured_not_assumed() {
        let times: Vec<f64> = (0..12).map(|i| 100.0 + f64::from(i) * 7.0).collect();
        let values: Vec<f64> = (0..12).map(|i| 5.0 + f64::from(i)).collect();
        let got = forecast_ets_stat(&[arg(&values), arg(&times), number_arg(Value::Number(8.0))]);
        assert!((number(got) - 7.0).abs() < 1e-9);
    }

    /// A gap in the timeline is filled by interpolation, and the value that
    /// lands in it is the one the two neighbours imply.
    #[test]
    fn a_gap_is_interpolated() {
        let times: Vec<f64> = vec![1.0, 2.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let values: Vec<f64> = times.iter().map(|t| 2.0 * t).collect();
        let got = forecast_ets(&[
            number_arg(Value::Number(12.0)),
            arg(&values),
            arg(&times),
            number_arg(Value::Number(0.0)),
        ]);
        assert!((number(got) - 24.0).abs() < 1e-6);
    }

    /// An interval that is not a whole number of steps is the one timeline
    /// Excel refuses.
    #[test]
    fn an_uneven_timeline_is_refused() {
        let times = [1.0, 2.0, 2.5, 4.0];
        let values = [1.0, 2.0, 3.0, 4.0];
        let got = forecast_ets(&[number_arg(Value::Number(5.0)), arg(&values), arg(&times)]);
        assert_eq!(got, Value::Error(CellError::Num));
    }

    /// A date before the timeline starts has nothing to be forecast from.
    #[test]
    fn a_target_before_the_start_is_an_error() {
        let times: Vec<f64> = (10..=20).map(f64::from).collect();
        let values: Vec<f64> = (10..=20).map(f64::from).collect();
        let got = forecast_ets(&[number_arg(Value::Number(3.0)), arg(&values), arg(&times)]);
        assert_eq!(got, Value::Error(CellError::Num));
    }

    /// The interval widens the further out the forecast goes, and on a series
    /// the model fits exactly it stays at nothing.
    #[test]
    fn the_interval_widens_with_distance() {
        let times: Vec<f64> = (1..=20).map(f64::from).collect();
        let values: Vec<f64> = times.iter().map(|t| 3.0 * t + 7.0).collect();
        let near = number(forecast_ets_confint(&[
            number_arg(Value::Number(21.0)),
            arg(&values),
            arg(&times),
        ]));
        let far = number(forecast_ets_confint(&[
            number_arg(Value::Number(40.0)),
            arg(&values),
            arg(&times),
        ]));
        assert!(near < 1e-6, "an exact fit has no spread, got {near}");
        assert!(far >= near);
    }

    /// Errors of the fit: none of them can be negative, and on an exact fit
    /// all four are zero.
    #[test]
    fn an_exact_fit_has_no_error() {
        let times: Vec<f64> = (1..=20).map(f64::from).collect();
        let values: Vec<f64> = times.iter().map(|t| 3.0 * t + 7.0).collect();
        for statistic in 4..=7 {
            let got = number(forecast_ets_stat(&[
                arg(&values),
                arg(&times),
                number_arg(Value::Number(f64::from(statistic))),
                number_arg(Value::Number(0.0)),
            ]));
            assert!(got.abs() < 1e-6, "statistic {statistic} came out {got}");
        }
    }

    /// Noise with no period in it is reported as having none.
    #[test]
    fn no_period_is_reported_as_zero() {
        let times: Vec<f64> = (1..=20).map(f64::from).collect();
        let values: Vec<f64> = times.iter().map(|t| 3.0 * t + 7.0).collect();
        let got = forecast_ets_seasonality(&[arg(&values), arg(&times)]);
        assert!((number(got) - 0.0).abs() < 1e-9);
    }
}
