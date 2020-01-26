//! Finance.
//!
//! Every function here follows Excel's sign convention: money paid out is
//! negative and money received is positive, which is why `PMT` of a loan comes
//! back negative and why `PV` and `FV` are on opposite sides of the equation.
//!
//! An annuity is one equation solved for whichever term is missing:
//!
//! ```text
//! pv * (1+r)^n + pmt * (1 + r*type) * ((1+r)^n - 1) / r + fv = 0
//! ```
//!
//! `FV`, `PV` and `PMT` rearrange it; `NPER` takes logarithms; `RATE` has no
//! closed form and is found by Newton's method, exactly as does.

use super::{Arg, first_error};
use crate::error::CellError;
use crate::formula::value::Value;
use crate::shared::date::Epoch;

/// How many steps the two iterative solvers take before giving up.
const MAX_ITERATIONS: usize = 128;

/// How close is close enough for them.
const PRECISION: f64 = 1.0e-8;

/// Payments at the end of the period (0) or the beginning (1).
///
/// The flag multiplies the rate by one extra period's worth of interest, which
/// is the whole of the difference between the two.
type PaymentTiming = f64;

/// `FV(rate, nper, pmt, [pv], [type])`
pub fn fv(args: &[Arg]) -> Value {
    let Some([rate, nper, pmt, pv, timing]) = with_defaults(args, 2, [0.0; 5]) else {
        return Value::Error(CellError::Value);
    };
    Value::Number(future_value(rate, nper, pmt, pv, timing_of(timing)))
}

/// `PV(rate, nper, pmt, [fv], [type])`
pub fn pv(args: &[Arg]) -> Value {
    let Some([rate, nper, pmt, fv, timing]) = with_defaults(args, 2, [0.0; 5]) else {
        return Value::Error(CellError::Value);
    };
    let timing = timing_of(timing);
    if nper < 0.0 {
        return Value::Error(CellError::Num);
    }
    if rate == 0.0 {
        return Value::Number(-fv - pmt * nper);
    }
    let growth = (1.0 + rate).powf(nper);
    Value::Number((-pmt * (1.0 + rate * timing) * ((growth - 1.0) / rate) - fv) / growth)
}

/// `PMT(rate, nper, pv, [fv], [type])`
pub fn pmt(args: &[Arg]) -> Value {
    let Some([rate, nper, pv, fv, timing]) = with_defaults(args, 3, [0.0; 5]) else {
        return Value::Error(CellError::Value);
    };
    Value::Number(payment(rate, nper, pv, fv, timing_of(timing)))
}

/// `NPER(rate, pmt, pv, [fv], [type])`
pub fn nper(args: &[Arg]) -> Value {
    let Some([rate, pmt, pv, fv, timing]) = with_defaults(args, 3, [0.0; 5]) else {
        return Value::Error(CellError::Value);
    };
    let timing = timing_of(timing);
    if pmt == 0.0 {
        return Value::Error(CellError::Num);
    }
    if rate == 0.0 {
        return Value::Number((-pv - fv) / pmt);
    }
    if pv == 0.0 {
        return Value::Error(CellError::Num);
    }
    let shift = pmt * (1.0 + rate * timing) / rate;
    let ratio = (shift - fv) / (pv + shift);
    if ratio <= 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(ratio.ln() / (1.0 + rate).ln())
}

/// `RATE(nper, pmt, pv, [fv], [type], [guess])`
///
/// Newton's method from the guess, as has it — taken there from numpy.
pub fn rate(args: &[Arg]) -> Value {
    let Some([nper, pmt, pv, fv, timing, guess]) =
        with_defaults(args, 3, [0.0, 0.0, 0.0, 0.0, 0.0, 0.1])
    else {
        return Value::Error(CellError::Value);
    };
    let timing = timing_of(timing);

    let mut rate = guess;
    for _ in 0..MAX_ITERATIONS {
        let Some(step) = rate_step(rate, nper, pmt, pv, fv, timing) else {
            break;
        };
        let next = rate - step;
        let close = (next - rate).abs() < PRECISION;
        rate = next;
        if close {
            return Value::Number(rate);
        }
    }
    Value::Error(CellError::Num)
}

/// One Newton step for `RATE`: the annuity equation over its own derivative.
fn rate_step(
    rate: f64,
    nper: f64,
    pmt: f64,
    pv: f64,
    fv: f64,
    timing: PaymentTiming,
) -> Option<f64> {
    if rate == 0.0 {
        return None;
    }
    let growth = (1.0 + rate).powf(nper);
    let previous = (1.0 + rate).powf(nper - 1.0);
    let due = rate * timing + 1.0;
    let numerator = fv + growth * pv + pmt * (growth - 1.0) * due / rate;
    let denominator = nper * previous * pv - pmt * (growth - 1.0) * due / (rate * rate)
        + nper * pmt * previous * due / rate
        + pmt * (growth - 1.0) * timing / rate;
    (denominator != 0.0).then(|| numerator / denominator)
}

/// `IPMT(rate, per, nper, pv, [fv], [type])` — the interest part of one payment.
pub fn ipmt(args: &[Arg]) -> Value {
    split_payment(args, true)
}

/// `PPMT(rate, per, nper, pv, [fv], [type])` — the principal part of it.
pub fn ppmt(args: &[Arg]) -> Value {
    split_payment(args, false)
}

/// Shared body of `IPMT` and `PPMT`.
///
fn split_payment(args: &[Arg], want_interest: bool) -> Value {
    let Some([rate, period, nper, pv, fv, timing]) = with_defaults(args, 4, [0.0; 6]) else {
        return Value::Error(CellError::Value);
    };
    let timing = timing_of(timing);
    let (period, nper) = (period.trunc(), nper.trunc());
    if period <= 0.0 || period > nper {
        return Value::Error(CellError::Num);
    }
    let (interest, principal) = walk_schedule(rate, whole(period), nper, pv, fv, timing);
    Value::Number(if want_interest { interest } else { principal })
}

/// Walks the payment schedule to a period and returns its interest and
/// principal.
fn walk_schedule(
    rate: f64,
    period: u32,
    nper: f64,
    pv: f64,
    fv: f64,
    timing: PaymentTiming,
) -> (f64, f64) {
    let payment = payment(rate, nper, pv, fv, timing);
    let due_at_start = timing != 0.0;
    let mut capital = pv;
    let (mut interest, mut principal) = (0.0, 0.0);
    for i in 1..=period {
        // With payments at the beginning of the period, the first one is made
        // before any interest has accrued.
        interest = if due_at_start && i == 1 {
            0.0
        } else {
            -capital * rate
        };
        principal = payment - interest;
        capital += principal;
    }
    (interest, principal)
}

/// `CUMIPMT(rate, nper, pv, start, end, type)` — interest paid over a stretch.
pub fn cumipmt(args: &[Arg]) -> Value {
    cumulative(args, true)
}

/// `CUMPRINC(rate, nper, pv, start, end, type)` — principal repaid over one.
pub fn cumprinc(args: &[Arg]) -> Value {
    cumulative(args, false)
}

/// Shared body of `CUMIPMT` and `CUMPRINC`.
fn cumulative(args: &[Arg], want_interest: bool) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [rate, nper, pv, start, end, timing] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(rate), Ok(nper), Ok(pv), Ok(start), Ok(end), Ok(timing)) = (
        rate.number(),
        nper.number(),
        pv.number(),
        start.number(),
        end.number(),
        timing.number(),
    ) else {
        return Value::Error(CellError::Value);
    };
    let timing = timing_of(timing);
    let (start, end) = (start.trunc(), end.trunc());
    if rate <= 0.0 || nper <= 0.0 || pv <= 0.0 || start < 1.0 || end < start || end > nper {
        return Value::Error(CellError::Num);
    }
    let mut total = 0.0;
    for period in whole(start)..=whole(end) {
        let (interest, principal) = walk_schedule(rate, period, nper, pv, 0.0, timing);
        total += if want_interest { interest } else { principal };
    }
    Value::Number(total)
}

/// `ISPMT(rate, per, nper, pv)` — the interest of a straight-line repayment,
/// where every period pays back the same slice of the principal.
pub fn ispmt(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [rate, period, nper, pv] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(rate), Ok(period), Ok(nper), Ok(pv)) =
        (rate.number(), period.number(), nper.number(), pv.number())
    else {
        return Value::Error(CellError::Value);
    };
    let (period, nper) = (period.trunc(), nper.trunc());
    if period <= 0.0 || period > nper {
        return Value::Error(CellError::Num);
    }
    if nper == 0.0 {
        return Value::Error(CellError::Div0);
    }
    // The last period pays no interest at all: nothing is left owing.
    if whole(period) == whole(nper) {
        return Value::Number(0.0);
    }
    Value::Number(-rate * pv * (1.0 - period / nper))
}

/// `NPV(rate, value1, ...)` — the values discounted back, the first of them one
/// period away rather than at once, which is what tells `NPV` from `XNPV`.
pub fn npv(args: &[Arg]) -> Value {
    let Some((rate, rest)) = args.split_first() else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let Ok(rate) = rate.number() else {
        return Value::Error(CellError::Value);
    };
    let mut total = 0.0;
    for (i, v) in flat_numbers(rest).into_iter().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a period index is far inside what f64 counts exactly"
        )]
        let period = (i + 1) as f64;
        total += v / (1.0 + rate).powf(period);
    }
    Value::Number(total)
}

/// `IRR(values, [guess])` — the rate at which `NPV` is zero.
pub fn irr(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (values, guess) = match args {
        [v] => (v, 0.1),
        [v, g] => match g.number() {
            Ok(g) => (v, g),
            Err(_) => return Value::Error(CellError::Value),
        },
        _ => return Value::Error(CellError::Value),
    };
    let values = flat_numbers(core::slice::from_ref(values));
    solve(guess, |rate| npv_at(rate, &values))
}

/// `XIRR(values, dates, [guess])` — the same for cash flows on given days.
pub fn xirr(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (values, dates, guess) = match args {
        [v, d] => (v, d, 0.1),
        [v, d, g] => match g.number() {
            Ok(g) => (v, d, g),
            Err(_) => return Value::Error(CellError::Value),
        },
        _ => return Value::Error(CellError::Value),
    };
    let values = flat_numbers(core::slice::from_ref(values));
    let dates = flat_numbers(core::slice::from_ref(dates));
    if values.len() != dates.len() || values.is_empty() {
        return Value::Error(CellError::Num);
    }
    solve(guess, |rate| xnpv_at(rate, &values, &dates))
}

/// `XNPV(rate, values, dates)`
pub fn xnpv(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [rate, values, dates] = args else {
        return Value::Error(CellError::Value);
    };
    let Ok(rate) = rate.number() else {
        return Value::Error(CellError::Value);
    };
    let values = flat_numbers(core::slice::from_ref(values));
    let dates = flat_numbers(core::slice::from_ref(dates));
    if values.len() != dates.len() || values.is_empty() {
        return Value::Error(CellError::Num);
    }
    if rate <= -1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(xnpv_at(rate, &values, &dates))
}

/// `MIRR(values, finance_rate, reinvest_rate)`
pub fn mirr(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [values, finance, reinvest] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(finance), Ok(reinvest)) = (finance.number(), reinvest.number()) else {
        return Value::Error(CellError::Value);
    };
    let values = flat_numbers(core::slice::from_ref(values));
    #[expect(
        clippy::cast_precision_loss,
        reason = "a count of cash flows is far inside what f64 counts exactly"
    )]
    let n = values.len() as f64;
    let (rr, fr) = (1.0 + reinvest, 1.0 + finance);
    let (mut positive, mut negative) = (0.0, 0.0);
    for (i, v) in values.iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "as above")]
        let period = i as f64;
        if *v >= 0.0 {
            positive += v / rr.powf(period);
        } else {
            negative += v / fr.powf(period);
        }
    }
    if positive == 0.0 || negative == 0.0 || n <= 1.0 {
        return Value::Error(CellError::Div0);
    }
    let result = ((-positive * rr.powf(n)) / (negative * rr)).powf(1.0 / (n - 1.0)) - 1.0;
    if result.is_finite() {
        Value::Number(result)
    } else {
        Value::Error(CellError::Num)
    }
}

/// `FVSCHEDULE(principal, schedule)` — compounded through a series of rates.
pub fn fvschedule(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [principal, schedule] = args else {
        return Value::Error(CellError::Value);
    };
    let Ok(mut total) = principal.number() else {
        return Value::Error(CellError::Value);
    };
    for rate in flat_numbers(core::slice::from_ref(schedule)) {
        total *= 1.0 + rate;
    }
    Value::Number(total)
}

/// `SLN(cost, salvage, life)` — the same amount written off every period.
pub fn sln(args: &[Arg]) -> Value {
    let Some([cost, salvage, life]) = numbers(args) else {
        return Value::Error(CellError::Value);
    };
    if life == 0.0 {
        return Value::Error(CellError::Div0);
    }
    Value::Number((cost - salvage) / life)
}

/// `SYD(cost, salvage, life, period)` — the sum-of-years'-digits method.
pub fn syd(args: &[Arg]) -> Value {
    let Some([cost, salvage, life, period]) = numbers(args) else {
        return Value::Error(CellError::Value);
    };
    if life <= 0.0 {
        return Value::Error(CellError::Num);
    }
    if period > life {
        return Value::Error(CellError::Num);
    }
    Value::Number(((cost - salvage) * (life - period + 1.0) * 2.0) / (life * (life + 1.0)))
}

/// `DB(cost, salvage, life, period, [month])` — fixed-declining balance.
pub fn db(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let Some([cost, salvage, life, period, month]) =
        with_defaults(args, 4, [0.0, 0.0, 0.0, 0.0, 12.0])
    else {
        return Value::Error(CellError::Value);
    };
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period <= 0.0 {
        return Value::Error(CellError::Num);
    }
    if cost == 0.0 {
        return Value::Number(0.0);
    }
    // Excel rounds the rate to three places before using it, and the rounding
    // is visible in the answer, so it is not an implementation detail.
    let raw = 1.0 - (salvage / cost).powf(1.0 / life);
    let depreciation_rate = (raw * 1000.0).round() / 1000.0;

    let (mut previous, mut depreciation) = (0.0, 0.0);
    let last_year = whole(life) + 1;
    for per in 1..=whole(period) {
        depreciation = if per == 1 {
            cost * depreciation_rate * month / 12.0
        } else if per == last_year {
            // The stub year: what is left of the twelve months the first one
            // did not use.
            (cost - previous) * depreciation_rate * (12.0 - month) / 12.0
        } else {
            (cost - previous) * depreciation_rate
        };
        previous += depreciation;
    }
    Value::Number(depreciation)
}

/// `DDB(cost, salvage, life, period, [factor])` — double-declining balance.
pub fn ddb(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let Some([cost, salvage, life, period, factor]) =
        with_defaults(args, 4, [0.0, 0.0, 0.0, 0.0, 2.0])
    else {
        return Value::Error(CellError::Value);
    };
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || period <= 0.0 || factor <= 0.0 {
        return Value::Error(CellError::Num);
    }
    if period > life {
        return Value::Error(CellError::Num);
    }
    let (mut previous, mut depreciation) = (0.0, 0.0);
    for _ in 1..=whole(period) {
        // The write-off stops at the salvage value, however much the factor
        // would otherwise take off.
        depreciation = ((cost - previous) * (factor / life)).min(cost - salvage - previous);
        previous += depreciation;
    }
    Value::Number(depreciation)
}

/// `VDB(cost, salvage, life, start, end, [factor], [no_switch])` — declining
/// balance over any span of periods, switching to straight line when that
/// writes off more.
///
/// A partial period is depreciated pro rata, which is what makes
/// `VDB(…, 0.5, 1.5, …)` meaningful. `no_switch` keeps the declining balance
/// even where straight line would be faster; Excel's default is to switch.
pub fn vdb(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let Some([cost, salvage, life, start, end, factor, no_switch]) =
        with_defaults(args, 5, [0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0])
    else {
        return Value::Error(CellError::Value);
    };
    if cost < 0.0 || salvage < 0.0 || life <= 0.0 || start < 0.0 || end < start || end > life {
        return Value::Error(CellError::Num);
    }
    if factor <= 0.0 {
        return Value::Error(CellError::Num);
    }
    let switching = no_switch == 0.0;

    // Whole periods are summed one by one; a fractional end or start is taken
    // as that share of the period it falls in, which is how Excel prorates.
    let mut written_off = 0.0;
    let mut total = 0.0;
    let mut period = 0.0;
    while period < end.ceil() {
        let next = period + 1.0;
        let remaining = cost - written_off;
        let declining = (remaining * factor / life)
            .min(remaining - salvage)
            .max(0.0);
        // Straight line over what is left of the asset's life; once it writes
        // off more than the declining balance would, Excel stays with it.
        let periods_left = (life - period).max(1.0);
        let straight = ((remaining - salvage) / periods_left).max(0.0);
        let this_period = if switching && straight > declining {
            straight
        } else {
            declining
        };
        // The share of this period that falls inside `start..end`.
        let from = start.max(period);
        let to = end.min(next);
        if to > from {
            total += this_period * (to - from);
        }
        written_off += this_period;
        period = next;
    }
    Value::Number(total)
}

/// The French depreciation coefficient, which depends on how long the asset
/// lives: under 3 years none, then 1.5, 2, and 2.5 beyond six.
fn amortization_coefficient(rate: f64) -> f64 {
    let life = 1.0 / rate;
    if life < 3.0 {
        1.0
    } else if life < 4.0 {
        1.5
    } else if life <= 6.0 {
        2.0
    } else {
        2.5
    }
}

/// `AMORDEGRC(cost, purchased, first_period, salvage, period, rate, [basis])`
/// — French declining depreciation, coefficient and all.
///
/// Each period's write-off is rounded to whole currency, which is the French
/// accounting rule and not a shortcut: the rounding is part of the answer.
pub fn amordegrc(epoch: Epoch, args: &[Arg]) -> Value {
    let Some((cost, purchased, first, salvage, period, rate, basis)) =
        amortization_args(epoch, args)
    else {
        return match first_error(args) {
            Some(e) => Value::Error(e),
            None => Value::Error(CellError::Value),
        };
    };
    if rate <= 0.0 {
        return Value::Error(CellError::Num);
    }
    let Some(fraction) = super::date::year_fraction(epoch, purchased, first, basis) else {
        return Value::Error(CellError::Num);
    };
    let rate = rate * amortization_coefficient(rate);
    let mut cost = cost;
    let mut write_off = (fraction * rate * cost).round();
    cost -= write_off;
    let mut rest = cost - salvage;
    for n in 0..whole(period) {
        write_off = (rate * cost).round();
        rest -= write_off;
        if rest < 0.0 {
            // The last period takes half of what is left; anything past the
            // asset's life takes nothing.
            return Value::Number(if whole(period) - n == 1 {
                (cost * 0.5).round()
            } else {
                0.0
            });
        }
        cost -= write_off;
    }
    Value::Number(write_off)
}

/// `AMORLINC(cost, purchased, first_period, salvage, period, rate, [basis])` —
/// French straight-line depreciation with the first period prorated.
pub fn amorlinc(epoch: Epoch, args: &[Arg]) -> Value {
    let Some((cost, purchased, first, salvage, period, rate, basis)) =
        amortization_args(epoch, args)
    else {
        return match first_error(args) {
            Some(e) => Value::Error(e),
            None => Value::Error(CellError::Value),
        };
    };
    if rate <= 0.0 {
        return Value::Error(CellError::Num);
    }
    let Some(mut fraction) = super::date::year_fraction(epoch, purchased, first, basis) else {
        return Value::Error(CellError::Num);
    };
    // A quirk of this function alone: on basis 1, a first period shorter than
    // a year is measured against 365 days even in a leap year.
    if basis == 1
        && fraction < 1.0
        && let Ok(bought) = crate::shared::date::from_serial(purchased.floor(), epoch)
        && super::date::is_leap(bought.year)
    {
        fraction *= 365.0 / 366.0;
    }
    let full_period = cost * rate;
    let first_period = fraction * rate * cost;
    if full_period == 0.0 {
        return Value::Error(CellError::Num);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a count of accounting periods, bounded by the asset's life"
    )]
    let full_periods = ((cost - salvage - first_period) / full_period).trunc() as u32;
    let period = whole(period);
    Value::Number(if period == 0 {
        first_period
    } else if period <= full_periods {
        full_period
    } else if period == full_periods + 1 {
        (cost - salvage) - full_period * f64::from(full_periods) - first_period
    } else {
        0.0
    })
}

/// The arguments `AMORDEGRC` and `AMORLINC` share, in the order they take them.
type Amortization = (f64, f64, f64, f64, f64, f64, i32);

/// Reads those arguments: two of them are dates, and the basis is optional.
fn amortization_args(epoch: Epoch, args: &[Arg]) -> Option<Amortization> {
    if first_error(args).is_some() || args.len() < 6 || args.len() > 7 {
        return None;
    }
    let cost = args[0].number().ok()?;
    let purchased = args[1].serial(epoch).ok()?;
    let first = args[2].serial(epoch).ok()?;
    let salvage = args[3].number().ok()?;
    let period = args[4].number().ok()?;
    let rate = args[5].number().ok()?;
    let basis = match args.get(6) {
        Some(arg) if !arg.missing() => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a basis is 0..=4; anything else is rejected downstream"
            )]
            let basis = arg.number().ok()?.trunc() as i32;
            basis
        }
        _ => 0,
    };
    Some((cost, purchased, first, salvage, period, rate, basis))
}

/// `EFFECT(nominal_rate, npery)` — the yearly rate compounding actually earns.
pub fn effect(args: &[Arg]) -> Value {
    let Some([nominal, periods]) = numbers(args) else {
        return Value::Error(CellError::Value);
    };
    let periods = periods.trunc();
    if nominal <= 0.0 || periods < 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number((1.0 + nominal / periods).powf(periods) - 1.0)
}

/// `NOMINAL(effect_rate, npery)` — the inverse of `EFFECT`.
pub fn nominal(args: &[Arg]) -> Value {
    let Some([effective, periods]) = numbers(args) else {
        return Value::Error(CellError::Value);
    };
    let periods = periods.trunc();
    if effective <= 0.0 || periods < 1.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(periods * ((effective + 1.0).powf(1.0 / periods) - 1.0))
}

/// `RRI(nper, pv, fv)` — the rate that grows `pv` into `fv` over `nper`.
pub fn rri(args: &[Arg]) -> Value {
    let Some([periods, present, future]) = numbers(args) else {
        return Value::Error(CellError::Value);
    };
    if periods <= 0.0 || present <= 0.0 || future < 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number((future / present).powf(1.0 / periods) - 1.0)
}

/// `PDURATION(rate, pv, fv)` — how many periods that growth takes.
pub fn pduration(args: &[Arg]) -> Value {
    let Some([rate, present, future]) = numbers(args) else {
        return Value::Error(CellError::Value);
    };
    if rate <= 0.0 || present <= 0.0 || future <= 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number((future.ln() - present.ln()) / (1.0 + rate).ln())
}

/// `DOLLARDE(fractional_dollar, fraction)` — `1.02` at a fraction of 16 is
/// one dollar and two sixteenths, which in decimal is 1.125.
pub fn dollarde(args: &[Arg]) -> Value {
    dollar_convert(args, true)
}

/// `DOLLARFR(decimal_dollar, fraction)` — the inverse.
pub fn dollarfr(args: &[Arg]) -> Value {
    dollar_convert(args, false)
}

/// Shared body of `DOLLARDE` and `DOLLARFR`.
fn dollar_convert(args: &[Arg], to_decimal: bool) -> Value {
    let Some([dollar, fraction]) = numbers(args) else {
        return Value::Error(CellError::Value);
    };
    let fraction = fraction.trunc();
    if fraction < 0.0 {
        return Value::Error(CellError::Num);
    }
    if fraction == 0.0 {
        return Value::Error(CellError::Div0);
    }
    let dollars = if dollar < 0.0 {
        dollar.ceil()
    } else {
        dollar.floor()
    };
    let mut cents = dollar % 1.0;
    let digits = fraction.log10().ceil();
    if to_decimal {
        cents /= fraction;
        cents *= 10f64.powf(digits);
    } else {
        cents *= fraction;
        cents *= 10f64.powf(-digits);
    }
    Value::Number(dollars + cents)
}

/// The arguments of a function whose trailing ones may be left out.
///
/// `defaults` gives both the arity and what a missing argument stands for; an
/// argument skipped in the middle — `PMT(r,n,pv,,1)` — takes its default too.
fn with_defaults<const N: usize>(
    args: &[Arg],
    required: usize,
    defaults: [f64; N],
) -> Option<[f64; N]> {
    if first_error(args).is_some() || args.len() < required || args.len() > N {
        return None;
    }
    let mut out = defaults;
    for (slot, arg) in out.iter_mut().zip(args) {
        if !arg.missing() {
            *slot = arg.number().ok()?;
        }
    }
    Some(out)
}

/// A period count as the whole number it is, saturating rather than wrapping.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "every caller checks the period is positive and no larger than \
              the term first; the clamp is belt and braces"
)]
fn whole(n: f64) -> u32 {
    n.trunc().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The payment timing flag: anything but zero means the beginning of the period.
fn timing_of(raw: f64) -> PaymentTiming {
    if raw == 0.0 { 0.0 } else { 1.0 }
}

/// Exactly `N` numeric arguments, or nothing.
fn numbers<const N: usize>(args: &[Arg]) -> Option<[f64; N]> {
    if first_error(args).is_some() || args.len() != N {
        return None;
    }
    let mut out = [0.0f64; N];
    for (slot, arg) in out.iter_mut().zip(args) {
        *slot = arg.number().ok()?;
    }
    Some(out)
}

/// Every number in the arguments, ranges flattened, non-numbers dropped.
///
/// A cash flow series is read out of cells, where Excel skips text and blanks
/// rather than counting them as zero.
fn flat_numbers(args: &[Arg]) -> Vec<f64> {
    let mut out = Vec::new();
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for v in flat {
            if let Value::Number(n) = v {
                out.push(*n);
            }
        }
    }
    out
}

/// The annuity equation solved for the future value.
fn future_value(rate: f64, nper: f64, pmt: f64, pv: f64, timing: PaymentTiming) -> f64 {
    if rate == 0.0 {
        return -pv - pmt * nper;
    }
    let growth = (1.0 + rate).powf(nper);
    -pv * growth - pmt * (1.0 + rate * timing) * (growth - 1.0) / rate
}

/// The annuity equation solved for the payment.
fn payment(rate: f64, nper: f64, pv: f64, fv: f64, timing: PaymentTiming) -> f64 {
    if rate == 0.0 {
        return (-pv - fv) / nper;
    }
    let growth = (1.0 + rate).powf(nper);
    (-fv - pv * growth) / (1.0 + rate * timing) / ((growth - 1.0) / rate)
}

/// `NPV` of a series whose first value sits at period zero, which is the
/// convention `IRR` solves against.
fn npv_at(rate: f64, values: &[f64]) -> f64 {
    let mut total = 0.0;
    for (i, v) in values.iter().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a period index is far inside what f64 counts exactly"
        )]
        let period = (i + 1) as f64;
        total += v / (1.0 + rate).powf(period);
    }
    total
}

/// `XNPV` of dated cash flows, discounted by actual days over 365.
fn xnpv_at(rate: f64, values: &[f64], dates: &[f64]) -> f64 {
    let first = dates.first().copied().unwrap_or(0.0);
    let mut total = 0.0;
    for (v, d) in values.iter().zip(dates) {
        let days = (d - first).trunc();
        total += v / (1.0 + rate).powf(days / 365.0);
    }
    total
}

/// Finds the rate at which `f` is zero.
///
fn solve(guess: f64, f: impl Fn(f64) -> f64) -> Value {
    let (mut x1, mut x2) = (0.0, guess);
    let (mut f1, mut f2) = (f(x1), f(x2));
    for _ in 0..MAX_ITERATIONS {
        if f1 * f2 < 0.0 {
            break;
        }
        if f1.abs() < f2.abs() {
            x1 += 1.6 * (x1 - x2);
            f1 = f(x1);
        } else {
            x2 += 1.6 * (x2 - x1);
            f2 = f(x2);
        }
    }
    if f1 * f2 > 0.0 {
        return Value::Error(CellError::Value);
    }

    let (mut bound, mut step) = if f(x1) < 0.0 {
        (x1, x2 - x1)
    } else {
        (x2, x1 - x2)
    };
    for _ in 0..MAX_ITERATIONS {
        step *= 0.5;
        let middle = bound + step;
        let value = f(middle);
        if value <= 0.0 {
            bound = middle;
        }
        if value.abs() < PRECISION || step.abs() < PRECISION {
            return Value::Number(middle);
        }
    }
    Value::Error(CellError::Value)
}
