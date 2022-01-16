//! Bonds and other securities.
//!
//! These all rest on two things the rest of the finance module does without: a
//! day-count basis, which decides what a "year" is worth (see
//! [`super::date::year_fraction`]), and a coupon calendar, which walks
//! backwards from maturity in whole periods to find the coupon dates around a
//! settlement date. Both live here, and every function below is written in
//! terms of them.

use super::{Arg, first_error};
use crate::error::CellError;
use crate::formula::value::Value;
use crate::shared::date::{DateTime, Epoch, from_serial, to_serial};

/// The arguments almost every function here begins with: when the trade
/// settles, when the security matures, and how the year is counted.
struct Security {
    settlement: f64,
    maturity: f64,
    frequency: u32,
    basis: i32,
}

/// How many days the basis puts in a year. Basis 1 answers for a particular
/// year, because that is the one basis that counts real days.
fn days_per_year(year: i32, basis: i32) -> Option<f64> {
    Some(match basis {
        0 | 2 | 4 => 360.0,
        3 => 365.0,
        1 => {
            if super::date::is_leap(year) {
                366.0
            } else {
                365.0
            }
        }
        _ => return None,
    })
}

/// Whether a date is the last day of its month, which a coupon date keeps: a
/// bond maturing on 31 August pays on 28 February, not on the 31st that month
/// does not have.
fn is_last_day(dt: DateTime) -> bool {
    dt.day == super::date::days_in_month(dt.year, dt.month)
}

/// The coupon date on one side of the settlement, found by stepping back from
/// maturity in whole periods.
///
/// `next` asks for the coupon after the settlement rather than the one before
/// it. The day of month is the maturity's, clamped to what the month holds,
/// except that a maturity on the last day of a month stays on the last day.
fn coupon_date(security: &Security, next: bool, epoch: Epoch) -> Option<f64> {
    let months = 12 / i32::try_from(security.frequency).ok()?;
    let end = from_serial(security.maturity.floor(), epoch).ok()?;
    let (day, last_day) = (end.day, is_last_day(end));
    let (mut year, mut month) = (end.year, i32::try_from(end.month).ok()?);

    // The date built from a year and a month, with the day rule applied.
    let build = |year: i32, month: i32| -> Option<f64> {
        let month = u32::try_from(month).ok()?;
        let days = super::date::days_in_month(year, month);
        let day = if last_day { days } else { day.min(days) };
        to_serial(DateTime::date(year, month, day), epoch).ok()
    };

    let mut current = build(year, month)?;
    // A whole-period walk back from maturity: the loop is bounded by the span
    // between the two dates, which the caller has already checked is sane.
    while security.settlement < current {
        month -= months;
        while month < 1 {
            month += 12;
            year -= 1;
        }
        current = build(year, month)?;
    }
    if next {
        month += months;
        while month > 12 {
            month -= 12;
            year += 1;
        }
        current = build(year, month)?;
    }
    Some(current)
}

/// `COUPPCD(settlement, maturity, frequency, [basis])` — the coupon date
/// before the settlement.
pub fn couppcd(epoch: Epoch, args: &[Arg]) -> Value {
    coupon(epoch, args, |s, epoch| coupon_date(s, false, epoch))
}

/// `COUPNCD(settlement, maturity, frequency, [basis])` — the coupon date after
/// the settlement.
pub fn coupncd(epoch: Epoch, args: &[Arg]) -> Value {
    coupon(epoch, args, |s, epoch| coupon_date(s, true, epoch))
}

/// `COUPNUM(settlement, maturity, frequency, [basis])` — how many coupons are
/// still to be paid.
pub fn coupnum(epoch: Epoch, args: &[Arg]) -> Value {
    coupon(epoch, args, coupons_left)
}

/// How many coupons are left to pay after the settlement.
fn coupons_left(security: &Security, epoch: Epoch) -> Option<f64> {
    let next = coupon_date(security, true, epoch)?;
    let (from, to) = (
        from_serial(next, epoch).ok()?,
        from_serial(security.maturity, epoch).ok()?,
    );
    let months = (to.year - from.year) * 12 + i32::try_from(to.month).ok()?
        - i32::try_from(from.month).ok()?;
    let per_period = 12 / i32::try_from(security.frequency).ok()?;
    Some(f64::from(months / per_period + 1))
}

/// `COUPDAYBS(settlement, maturity, frequency, [basis])` — days from the start
/// of the current coupon period to the settlement.
pub fn coupdaybs(epoch: Epoch, args: &[Arg]) -> Value {
    coupon(epoch, args, days_since_previous)
}

/// Days from the coupon before the settlement to the settlement itself.
fn days_since_previous(security: &Security, epoch: Epoch) -> Option<f64> {
    let previous = coupon_date(security, false, epoch)?;
    if security.basis == 1 {
        return Some((security.settlement - previous).abs());
    }
    let year = from_serial(security.settlement, epoch).ok()?.year;
    let fraction =
        super::date::year_fraction(epoch, previous, security.settlement, security.basis)?;
    Some(fraction * days_per_year(year, security.basis)?)
}

/// `COUPDAYS(settlement, maturity, frequency, [basis])` — the length of the
/// coupon period the settlement falls in.
pub fn coupdays(epoch: Epoch, args: &[Arg]) -> Value {
    coupon(epoch, args, period_length)
}

/// The length of the coupon period holding the settlement.
fn period_length(security: &Security, epoch: Epoch) -> Option<f64> {
    let frequency = f64::from(security.frequency);
    match security.basis {
        3 => Some(365.0 / frequency),
        1 => {
            // Actual/actual measures the real period, except that an annual
            // bond has no period shorter than the year itself.
            if security.frequency == 1 {
                let year = from_serial(security.settlement, epoch).ok()?.year;
                return Some(days_per_year(year, security.basis)? / frequency);
            }
            Some(coupon_date(security, true, epoch)? - coupon_date(security, false, epoch)?)
        }
        _ => Some(360.0 / frequency),
    }
}

/// `COUPDAYSNC(settlement, maturity, frequency, [basis])` — days from the
/// settlement to the next coupon.
pub fn coupdaysnc(epoch: Epoch, args: &[Arg]) -> Value {
    coupon(epoch, args, days_to_next)
}

/// Days from the settlement to the coupon after it.
fn days_to_next(security: &Security, epoch: Epoch) -> Option<f64> {
    let next = coupon_date(security, true, epoch)?;
    if security.basis == 1 {
        return Some((next - security.settlement).abs());
    }
    let year = from_serial(security.settlement, epoch).ok()?.year;
    let fraction = super::date::year_fraction(epoch, security.settlement, next, security.basis)?;
    Some(fraction * days_per_year(year, security.basis)?)
}

/// Shared body of the `COUP*` family: read the arguments, check them, and hand
/// them to the part that differs.
fn coupon(epoch: Epoch, args: &[Arg], body: impl Fn(&Security, Epoch) -> Option<f64>) -> Value {
    let security = match security(epoch, args, 2) {
        Ok(s) => s,
        Err(e) => return Value::Error(e),
    };
    match body(&security, epoch) {
        Some(n) => Value::Number(n),
        None => Value::Error(CellError::Num),
    }
}

/// Reads settlement, maturity, an optional frequency and an optional basis.
///
/// `frequency_at` says which argument carries the frequency; a function
/// without one passes `usize::MAX`. A settlement on or after the maturity is
/// `#NUM!`, as is a frequency that is not 1, 2 or 4.
fn security(epoch: Epoch, args: &[Arg], frequency_at: usize) -> Result<Security, CellError> {
    if let Some(e) = first_error(args) {
        return Err(e);
    }
    let settlement = args.first().ok_or(CellError::Value)?.serial(epoch)?.floor();
    let maturity = args.get(1).ok_or(CellError::Value)?.serial(epoch)?.floor();
    if settlement >= maturity {
        return Err(CellError::Num);
    }
    let frequency = match args.get(frequency_at) {
        Some(arg) => {
            let n = arg.number()?;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked against the three values Excel allows"
            )]
            let n = n.trunc() as u32;
            if !matches!(n, 1 | 2 | 4) {
                return Err(CellError::Num);
            }
            n
        }
        None => 1,
    };
    // The basis always comes last and is always optional.
    let basis = match args.get(frequency_at.wrapping_add(1)) {
        Some(arg) if !arg.missing() => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "checked against 0..=4 right below"
            )]
            let basis = arg.number()?.trunc() as i32;
            if !(0..=4).contains(&basis) {
                return Err(CellError::Num);
            }
            basis
        }
        _ => 0,
    };
    Ok(Security {
        settlement,
        maturity,
        frequency,
        basis,
    })
}

/// The `n`-th argument as a whole date serial.
fn date_at(epoch: Epoch, args: &[Arg], n: usize) -> Result<f64, CellError> {
    Ok(args.get(n).ok_or(CellError::Value)?.serial(epoch)?.floor())
}

/// The `n`-th argument as a number.
fn num_at(args: &[Arg], n: usize) -> Result<f64, CellError> {
    args.get(n).ok_or(CellError::Value)?.number()
}

/// The `n`-th argument as a day-count basis, defaulting to 0 when it is left
/// out or written empty.
fn basis_at(args: &[Arg], n: usize) -> Result<i32, CellError> {
    let Some(arg) = args.get(n) else { return Ok(0) };
    if arg.missing() {
        return Ok(0);
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "checked against 0..=4 right below"
    )]
    let basis = arg.number()?.trunc() as i32;
    if (0..=4).contains(&basis) {
        Ok(basis)
    } else {
        Err(CellError::Num)
    }
}

/// The `n`-th argument as a coupon frequency: 1, 2 or 4 and nothing else.
fn frequency_at(args: &[Arg], n: usize) -> Result<u32, CellError> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked against the three values Excel allows"
    )]
    let n = num_at(args, n)?.trunc() as u32;
    if matches!(n, 1 | 2 | 4) {
        Ok(n)
    } else {
        Err(CellError::Num)
    }
}

/// Turns a computed result into a value, and any refusal into its error.
fn answer(args: &[Arg], body: impl FnOnce() -> Result<f64, CellError>) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    match body() {
        Ok(n) => Value::Number(n),
        Err(e) => Value::Error(e),
    }
}

/// The span from settlement to maturity as a fraction of a year, refusing a
/// settlement that is not before the maturity.
fn span(epoch: Epoch, from: f64, to: f64, basis: i32) -> Result<f64, CellError> {
    if from >= to {
        return Err(CellError::Num);
    }
    super::date::year_fraction(epoch, from, to, basis).ok_or(CellError::Num)
}

/// `PRICE(settlement, maturity, rate, yield, redemption, frequency, [basis])`
/// — what 100 of face value is worth.
///
/// The redemption is discounted over the whole remaining term and every coupon
/// over its own, and the interest the seller has already earned in the current
/// period comes off: the answer is the clean price.
pub fn price(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let security = Security {
            settlement: date_at(epoch, args, 0)?,
            maturity: date_at(epoch, args, 1)?,
            frequency: frequency_at(args, 5)?,
            basis: basis_at(args, 6)?,
        };
        let (rate, yield_, redemption) = (num_at(args, 2)?, num_at(args, 3)?, num_at(args, 4)?);
        if security.settlement >= security.maturity
            || rate < 0.0
            || yield_ < 0.0
            || redemption <= 0.0
        {
            return Err(CellError::Num);
        }
        clean_price(epoch, &security, rate, yield_, redemption)
    })
}

/// The body of `PRICE`, also used by `YIELD` to search for the yield that
/// reproduces a given price.
fn clean_price(
    epoch: Epoch,
    security: &Security,
    rate: f64,
    yield_: f64,
    redemption: f64,
) -> Result<f64, CellError> {
    let frequency = f64::from(security.frequency);
    let period = coupon_measure(epoch, security)?;
    let base = 1.0 + yield_ / frequency;
    let coupon = 100.0 * rate / frequency;
    let (dsc, e, n, a) = period;
    let de = dsc / e;
    let last = n - 1.0;

    // With one coupon period or less to run, Excel documents a formula of its
    // own: the single remaining period is discounted at simple interest, not
    // compounded: a formula that compounds the last stub period answers
    // 99.6665866 where Excel and excelize both answer 99.6568627.
    if n <= 1.0 {
        return Ok((redemption + coupon) / (1.0 + de * yield_ / frequency) - coupon * (a / e));
    }

    let mut out = redemption / base.powf(last + de);
    let mut k = 0.0;
    while k <= last {
        out += coupon / base.powf(k + de);
        k += 1.0;
    }
    Ok(out - coupon * (a / e))
}

/// The four coupon measurements every price formula is written in terms of:
/// days to the next coupon, the length of the period, how many coupons are
/// left, and days since the last one.
fn coupon_measure(epoch: Epoch, security: &Security) -> Result<(f64, f64, f64, f64), CellError> {
    let measure = |value: Option<f64>| value.ok_or(CellError::Num);
    Ok((
        measure(days_to_next(security, epoch))?,
        measure(period_length(security, epoch))?,
        measure(coupons_left(security, epoch))?,
        measure(days_since_previous(security, epoch))?,
    ))
}

/// `PRICEDISC(settlement, maturity, discount, redemption, [basis])` — the
/// price of a security sold at a discount and paying no coupon.
pub fn pricedisc(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (settlement, maturity) = (date_at(epoch, args, 0)?, date_at(epoch, args, 1)?);
        let (discount, redemption) = (num_at(args, 2)?, num_at(args, 3)?);
        if discount <= 0.0 || redemption <= 0.0 {
            return Err(CellError::Num);
        }
        let fraction = span(epoch, settlement, maturity, basis_at(args, 4)?)?;
        Ok(redemption * (1.0 - discount * fraction))
    })
}

/// `PRICEMAT(settlement, maturity, issue, rate, yield, [basis])` — the price
/// of a security that pays its interest in one go at maturity.
pub fn pricemat(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (settlement, maturity, issue) = (
            date_at(epoch, args, 0)?,
            date_at(epoch, args, 1)?,
            date_at(epoch, args, 2)?,
        );
        let (rate, yield_) = (num_at(args, 3)?, num_at(args, 4)?);
        let basis = basis_at(args, 5)?;
        if rate < 0.0 || yield_ < 0.0 {
            return Err(CellError::Num);
        }
        let to_settlement = span(epoch, issue, settlement, basis)?;
        let to_maturity = span(epoch, issue, maturity, basis)?;
        let remaining = span(epoch, settlement, maturity, basis)?;
        Ok(
            (100.0 + to_maturity * rate * 100.0) / (1.0 + remaining * yield_)
                - to_settlement * rate * 100.0,
        )
    })
}

/// `DISC(settlement, maturity, price, redemption, [basis])` — the discount
/// rate a price implies.
pub fn disc(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (settlement, maturity) = (date_at(epoch, args, 0)?, date_at(epoch, args, 1)?);
        let (price, redemption) = (num_at(args, 2)?, num_at(args, 3)?);
        if price <= 0.0 || redemption <= 0.0 {
            return Err(CellError::Num);
        }
        let fraction = span(epoch, settlement, maturity, basis_at(args, 4)?)?;
        Ok((1.0 - price / redemption) / fraction)
    })
}

/// `INTRATE(settlement, maturity, investment, redemption, [basis])` — the rate
/// a fully invested security returns.
pub fn intrate(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (settlement, maturity) = (date_at(epoch, args, 0)?, date_at(epoch, args, 1)?);
        let (investment, redemption) = (num_at(args, 2)?, num_at(args, 3)?);
        if investment <= 0.0 || redemption <= 0.0 {
            return Err(CellError::Num);
        }
        let fraction = span(epoch, settlement, maturity, basis_at(args, 4)?)?;
        Ok((redemption / investment - 1.0) / fraction)
    })
}

/// `RECEIVED(settlement, maturity, investment, discount, [basis])` — what a
/// fully invested security pays at maturity.
pub fn received(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (settlement, maturity) = (date_at(epoch, args, 0)?, date_at(epoch, args, 1)?);
        let (investment, discount) = (num_at(args, 2)?, num_at(args, 3)?);
        if investment <= 0.0 || discount <= 0.0 {
            return Err(CellError::Num);
        }
        let fraction = span(epoch, settlement, maturity, basis_at(args, 4)?)?;
        Ok(investment / (1.0 - discount * fraction))
    })
}

/// `ACCRINTM(issue, settlement, rate, [par], [basis])` — interest earned by a
/// security that pays at maturity.
pub fn accrintm(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (issue, settlement) = (date_at(epoch, args, 0)?, date_at(epoch, args, 1)?);
        let rate = num_at(args, 2)?;
        let par = match args.get(3) {
            Some(arg) if !arg.missing() => arg.number()?,
            _ => 1000.0,
        };
        if rate <= 0.0 || par <= 0.0 {
            return Err(CellError::Num);
        }
        let fraction = span(epoch, issue, settlement, basis_at(args, 4)?)?;
        Ok(par * rate * fraction)
    })
}

/// `ACCRINT(issue, first_interest, settlement, rate, [par], [frequency],
/// [basis], [method])` — interest earned by a security paying periodically.
///
/// The interest is measured from the issue to the settlement, which is what
/// does; `first_interest` and `method` are read and checked but do
/// not change the answer, as they do not there either.
pub fn accrint(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let issue = date_at(epoch, args, 0)?;
        let _first_interest = date_at(epoch, args, 1)?;
        let settlement = date_at(epoch, args, 2)?;
        let rate = num_at(args, 3)?;
        let par = match args.get(4) {
            Some(arg) if !arg.missing() => arg.number()?,
            _ => 1000.0,
        };
        if args.len() > 5 && !args[5].missing() {
            frequency_at(args, 5)?;
        }
        if rate <= 0.0 || par <= 0.0 {
            return Err(CellError::Num);
        }
        let fraction = span(epoch, issue, settlement, basis_at(args, 6)?)?;
        Ok(par * rate * fraction)
    })
}

/// `YIELDDISC(settlement, maturity, price, redemption, [basis])` — the yield
/// of a security sold at a discount.
pub fn yielddisc(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (settlement, maturity) = (date_at(epoch, args, 0)?, date_at(epoch, args, 1)?);
        let (price, redemption) = (num_at(args, 2)?, num_at(args, 3)?);
        if price <= 0.0 || redemption <= 0.0 {
            return Err(CellError::Num);
        }
        let basis = basis_at(args, 4)?;
        let year = from_serial(settlement, epoch)
            .map_err(|_| CellError::Num)?
            .year;
        let per_year = days_per_year(year, basis).ok_or(CellError::Num)?;
        let days = span(epoch, settlement, maturity, basis)? * per_year;
        Ok(((redemption - price) / price) * (per_year / days))
    })
}

/// `YIELDMAT(settlement, maturity, issue, rate, price, [basis])` — the yield
/// of a security that pays its interest at maturity.
pub fn yieldmat(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (settlement, maturity, issue) = (
            date_at(epoch, args, 0)?,
            date_at(epoch, args, 1)?,
            date_at(epoch, args, 2)?,
        );
        let (rate, price) = (num_at(args, 3)?, num_at(args, 4)?);
        let basis = basis_at(args, 5)?;
        if rate < 0.0 || price <= 0.0 {
            return Err(CellError::Num);
        }
        let year = from_serial(settlement, epoch)
            .map_err(|_| CellError::Num)?
            .year;
        let per_year = days_per_year(year, basis).ok_or(CellError::Num)?;
        let issue_to_settlement = span(epoch, issue, settlement, basis)?;
        let issue_to_maturity = span(epoch, issue, maturity, basis)?;
        let remaining = span(epoch, settlement, maturity, basis)?;
        let carried = price / 100.0 + issue_to_settlement * rate;
        Ok(((1.0 + issue_to_maturity * rate - carried) / carried)
            * (per_year / (remaining * per_year)))
    })
}

/// The three treasury-bill functions share one rule: the span may not exceed a
/// year, which is what makes them bills rather than bonds.
fn bill_days(epoch: Epoch, args: &[Arg]) -> Result<f64, CellError> {
    let (settlement, maturity) = (date_at(epoch, args, 0)?, date_at(epoch, args, 1)?);
    let days = maturity - settlement;
    let year = from_serial(settlement, epoch)
        .map_err(|_| CellError::Num)?
        .year;
    if days < 0.0 || days > days_per_year(year, 1).ok_or(CellError::Num)? {
        return Err(CellError::Num);
    }
    Ok(days)
}

/// `TBILLEQ(settlement, maturity, discount)` — the bond-equivalent yield of a
/// treasury bill.
pub fn tbilleq(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let days = bill_days(epoch, args)?;
        let discount = num_at(args, 2)?;
        if discount <= 0.0 {
            return Err(CellError::Num);
        }
        Ok((365.0 * discount) / (360.0 - discount * days))
    })
}

/// `TBILLPRICE(settlement, maturity, discount)` — the price per 100 of face
/// value.
pub fn tbillprice(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let days = bill_days(epoch, args)?;
        let discount = num_at(args, 2)?;
        if discount <= 0.0 {
            return Err(CellError::Num);
        }
        let price = 100.0 * (1.0 - discount * days / 360.0);
        if price < 0.0 {
            return Err(CellError::Num);
        }
        Ok(price)
    })
}

/// `TBILLYIELD(settlement, maturity, price)` — the yield a price implies.
pub fn tbillyield(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let days = bill_days(epoch, args)?;
        let price = num_at(args, 2)?;
        if price <= 0.0 {
            return Err(CellError::Num);
        }
        Ok(((100.0 - price) / price) * (360.0 / days))
    })
}

/// `YIELD(settlement, maturity, rate, price, redemption, frequency, [basis])`
/// — the yield a price implies.
///
/// [`price`] has no closed-form inverse, so the yield is searched for: the
/// price falls as the yield rises, which makes a bisection safe where Newton's
/// method can walk off. The bracket is widened until it holds the answer.
pub fn yield_(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let security = Security {
            settlement: date_at(epoch, args, 0)?,
            maturity: date_at(epoch, args, 1)?,
            frequency: frequency_at(args, 5)?,
            basis: basis_at(args, 6)?,
        };
        let (rate, target, redemption) = (num_at(args, 2)?, num_at(args, 3)?, num_at(args, 4)?);
        if rate < 0.0 || target <= 0.0 || redemption <= 0.0 {
            return Err(CellError::Num);
        }
        let at = |y: f64| clean_price(epoch, &security, rate, y, redemption);

        // Price falls as yield rises, so the answer sits where the curve
        // crosses the target; find a yield priced below it to bracket that.
        let (mut low, mut high) = (0.0, 1.0);
        let mut guard = 0;
        while at(high)? > target {
            low = high;
            high *= 2.0;
            guard += 1;
            if guard > 40 {
                return Err(CellError::Num);
            }
        }
        for _ in 0..200 {
            let middle = f64::midpoint(low, high);
            if at(middle)? > target {
                low = middle;
            } else {
                high = middle;
            }
        }
        Ok(f64::midpoint(low, high))
    })
}

/// `DURATION(settlement, maturity, coupon, yield, frequency, [basis])` — the
/// Macaulay duration: the average time to a payment, each weighted by what it
/// is worth today.
pub fn duration(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || macaulay(epoch, args).map(|(duration, _)| duration))
}

/// `MDURATION(settlement, maturity, coupon, yield, frequency, [basis])` — the
/// modified duration, which is the Macaulay duration discounted by one period.
pub fn mduration(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let (duration, base) = macaulay(epoch, args)?;
        Ok(duration / base)
    })
}

/// The Macaulay duration and the per-period discount factor, which is all
/// `MDURATION` needs on top of it.
fn macaulay(epoch: Epoch, args: &[Arg]) -> Result<(f64, f64), CellError> {
    let security = Security {
        settlement: date_at(epoch, args, 0)?,
        maturity: date_at(epoch, args, 1)?,
        frequency: frequency_at(args, 4)?,
        basis: basis_at(args, 5)?,
    };
    let (coupon_rate, yield_) = (num_at(args, 2)?, num_at(args, 3)?);
    if coupon_rate < 0.0 || yield_ < 0.0 {
        return Err(CellError::Num);
    }
    let frequency = f64::from(security.frequency);
    let (dsc, e, n, _) = coupon_measure(epoch, &security)?;
    let base = 1.0 + yield_ / frequency;
    let coupon = 100.0 * coupon_rate / frequency;
    let de = dsc / e;

    // Each payment discounted to today, and the same weighted by when it
    // arrives; the duration is one divided by the other, in years.
    let (mut present, mut weighted) = (0.0, 0.0);
    // The coupons are counted off one period at a time; the last one arrives
    // together with the redemption, which is why the loop counts down.
    let mut left = n;
    let mut k = 0.0;
    while left > 0.0 {
        let periods = k + de;
        let flow = if left <= 1.0 { coupon + 100.0 } else { coupon };
        let value = flow / base.powf(periods);
        present += value;
        weighted += value * periods;
        k += 1.0;
        left -= 1.0;
    }
    if present == 0.0 {
        return Err(CellError::Num);
    }
    Ok((weighted / present / frequency, base))
}

/// The days between two dates as the basis counts them: the 30/360 bases
/// count months of thirty days, the rest count the days there actually are.
///
/// A share of a quasi-period has to be measured this way rather than through
/// [`super::date::year_fraction`]: basis 1 divides by the average length of
/// the years a span touches, and two spans inside one period would then be
/// measured against slightly different years.
fn days_between(epoch: Epoch, from: f64, to: f64, basis: i32) -> Result<f64, CellError> {
    match basis {
        0 | 4 => {
            Ok(super::date::year_fraction(epoch, from, to, basis).ok_or(CellError::Num)? * 360.0)
        }
        _ => Ok(to.floor() - from.floor()),
    }
}

/// The quasi-coupon dates of an odd last period: the schedule the bond would
/// have had, counted forward from the last real coupon.
///
/// An odd period is measured against these imaginary periods rather than
/// against itself, which is what makes the odd formulas agree with the regular
/// ones when the period turns out not to be odd after all.
fn quasi_forward(from: f64, until: f64, frequency: u32, epoch: Epoch) -> Option<Vec<f64>> {
    let months = 12 / i32::try_from(frequency).ok()?;
    let start = from_serial(from.floor(), epoch).ok()?;
    let (day, last_day) = (start.day, is_last_day(start));
    let mut out = vec![from.floor()];
    let (mut year, mut month) = (start.year, i32::try_from(start.month).ok()?);
    // Bounded by the term of a bond; a hundred years of quarterly coupons is
    // four hundred steps.
    for _ in 0..1_000 {
        month += months;
        while month > 12 {
            month -= 12;
            year += 1;
        }
        let m = u32::try_from(month).ok()?;
        let days = super::date::days_in_month(year, m);
        let day = if last_day { days } else { day.min(days) };
        let next = to_serial(DateTime::date(year, m, day), epoch).ok()?;
        out.push(next);
        if next >= until.floor() {
            return Some(out);
        }
    }
    None
}

/// `ODDLPRICE(settlement, maturity, last_interest, rate, yield, redemption,
/// frequency, [basis])` — the price of a bond whose last period is odd.
pub fn oddlprice(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let odd = OddLast::read(epoch, args)?;
        odd.price(epoch, num_at(args, 4)?)
    })
}

/// `ODDLYIELD(settlement, maturity, last_interest, rate, price, redemption,
/// frequency, [basis])` — the yield of a bond whose last period is odd.
///
/// The inverse of [`oddlprice`], and found the same way [`yield_`] is: the
/// price falls as the yield rises, so a bisection cannot miss it.
pub fn oddlyield(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let odd = OddLast::read(epoch, args)?;
        let target = num_at(args, 4)?;
        if target <= 0.0 {
            return Err(CellError::Num);
        }
        let (mut low, mut high) = (0.0, 1.0);
        let mut guard = 0;
        while odd.price(epoch, high)? > target {
            low = high;
            high *= 2.0;
            guard += 1;
            if guard > 40 {
                return Err(CellError::Num);
            }
        }
        for _ in 0..200 {
            let middle = f64::midpoint(low, high);
            if odd.price(epoch, middle)? > target {
                low = middle;
            } else {
                high = middle;
            }
        }
        Ok(f64::midpoint(low, high))
    })
}

/// A bond whose final period does not line up with its coupon schedule.
struct OddLast {
    settlement: f64,
    maturity: f64,
    last_interest: f64,
    rate: f64,
    redemption: f64,
    frequency: u32,
    basis: i32,
}

impl OddLast {
    /// Reads the arguments the two odd-last functions share; they differ only
    /// in whether the fifth is a yield or a price.
    fn read(epoch: Epoch, args: &[Arg]) -> Result<Self, CellError> {
        let odd = Self {
            settlement: date_at(epoch, args, 0)?,
            maturity: date_at(epoch, args, 1)?,
            last_interest: date_at(epoch, args, 2)?,
            rate: num_at(args, 3)?,
            redemption: num_at(args, 5)?,
            frequency: frequency_at(args, 6)?,
            basis: basis_at(args, 7)?,
        };
        if odd.settlement >= odd.maturity
            || odd.last_interest >= odd.settlement
            || odd.rate < 0.0
            || odd.redemption <= 0.0
        {
            return Err(CellError::Num);
        }
        Ok(odd)
    }

    /// The price at a given yield.
    ///
    /// The odd period is broken into the quasi-periods it spans; each one
    /// contributes the share of a coupon its days are worth, and the whole is
    /// discounted over the time still to run.
    fn price(&self, epoch: Epoch, yield_: f64) -> Result<f64, CellError> {
        let frequency = f64::from(self.frequency);
        let dates = quasi_forward(self.last_interest, self.maturity, self.frequency, epoch)
            .ok_or(CellError::Num)?;
        let coupon = 100.0 * self.rate / frequency;

        // Days of the odd period that have already run, and days that are
        // still to run, both counted in whole quasi-periods.
        let (mut accrued, mut remaining) = (0.0, 0.0);
        for pair in dates.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let length = days_between(epoch, from, to, self.basis)?;
            if length == 0.0 {
                return Err(CellError::Num);
            }
            let share = |a: f64, b: f64| -> Result<f64, CellError> {
                let (a, b) = (a.max(from), b.min(to));
                if b <= a {
                    return Ok(0.0);
                }
                Ok(days_between(epoch, a, b, self.basis)? / length)
            };
            accrued += share(self.last_interest, self.settlement)?;
            remaining += share(self.settlement, self.maturity)?;
        }

        let interest = coupon * (accrued + remaining);
        Ok(
            (self.redemption + interest) / (1.0 + remaining * yield_ / frequency)
                - coupon * accrued,
        )
    }
}

/// The quasi-coupon dates of an odd first period, counted backwards from the
/// first real coupon to before the issue.
fn quasi_backward(from: f64, until: f64, frequency: u32, epoch: Epoch) -> Option<Vec<f64>> {
    let months = 12 / i32::try_from(frequency).ok()?;
    let start = from_serial(from.floor(), epoch).ok()?;
    let (day, last_day) = (start.day, is_last_day(start));
    let mut out = vec![from.floor()];
    let (mut year, mut month) = (start.year, i32::try_from(start.month).ok()?);
    for _ in 0..1_000 {
        month -= months;
        while month < 1 {
            month += 12;
            year -= 1;
        }
        let m = u32::try_from(month).ok()?;
        let days = super::date::days_in_month(year, m);
        let day = if last_day { days } else { day.min(days) };
        let previous = to_serial(DateTime::date(year, m, day), epoch).ok()?;
        out.push(previous);
        if previous <= until.floor() {
            out.reverse();
            return Some(out);
        }
    }
    None
}

/// A bond whose first period does not line up with its coupon schedule.
struct OddFirst {
    settlement: f64,
    maturity: f64,
    issue: f64,
    first_coupon: f64,
    rate: f64,
    redemption: f64,
    frequency: u32,
    basis: i32,
}

impl OddFirst {
    /// Reads the arguments the two odd-first functions share.
    fn read(epoch: Epoch, args: &[Arg]) -> Result<Self, CellError> {
        let odd = Self {
            settlement: date_at(epoch, args, 0)?,
            maturity: date_at(epoch, args, 1)?,
            issue: date_at(epoch, args, 2)?,
            first_coupon: date_at(epoch, args, 3)?,
            rate: num_at(args, 4)?,
            redemption: num_at(args, 6)?,
            frequency: frequency_at(args, 7)?,
            basis: basis_at(args, 8)?,
        };
        if odd.settlement >= odd.maturity
            || odd.issue >= odd.settlement
            || odd.first_coupon <= odd.settlement
            || odd.first_coupon >= odd.maturity
            || odd.rate < 0.0
            || odd.redemption <= 0.0
        {
            return Err(CellError::Num);
        }
        Ok(odd)
    }

    /// The price at a given yield.
    ///
    /// The odd first coupon is worth the share of a full one that its days are
    /// worth, measured against the quasi-periods it spans; after it the bond
    /// runs to its schedule, so the rest is the regular sum.
    fn price(&self, epoch: Epoch, yield_: f64) -> Result<f64, CellError> {
        let frequency = f64::from(self.frequency);
        let coupon = 100.0 * self.rate / frequency;
        let base = 1.0 + yield_ / frequency;

        // The quasi-periods of the odd first period, and where the issue and
        // the settlement fall inside them.
        let dates = quasi_backward(self.first_coupon, self.issue, self.frequency, epoch)
            .ok_or(CellError::Num)?;
        let (mut odd_coupon, mut accrued, mut to_first) = (0.0, 0.0, 0.0);
        for pair in dates.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let length = days_between(epoch, from, to, self.basis)?;
            if length == 0.0 {
                return Err(CellError::Num);
            }
            let share = |a: f64, b: f64| -> Result<f64, CellError> {
                let (a, b) = (a.max(from), b.min(to));
                if b <= a {
                    return Ok(0.0);
                }
                Ok(days_between(epoch, a, b, self.basis)? / length)
            };
            odd_coupon += share(self.issue, self.first_coupon)?;
            accrued += share(self.issue, self.settlement)?;
            to_first += share(self.settlement, self.first_coupon)?;
        }

        // Coupons after the first one follow the ordinary schedule.
        let after_first = Security {
            settlement: self.first_coupon,
            maturity: self.maturity,
            frequency: self.frequency,
            basis: self.basis,
        };
        let later = coupons_left(&after_first, epoch).ok_or(CellError::Num)?;

        let mut out = coupon * odd_coupon / base.powf(to_first);
        // `later` is a whole number of coupons, counted off one at a time so
        // the exponent never has to leave the floating point domain.
        let mut k = 1.0;
        while k <= later {
            out += coupon / base.powf(to_first + k);
            k += 1.0;
        }
        out += self.redemption / base.powf(to_first + later);
        Ok(out - coupon * accrued)
    }
}

/// `ODDFPRICE(settlement, maturity, issue, first_coupon, rate, yield,
/// redemption, frequency, [basis])` — the price of a bond whose first period
/// is odd.
pub fn oddfprice(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let odd = OddFirst::read(epoch, args)?;
        odd.price(epoch, num_at(args, 5)?)
    })
}

/// `ODDFYIELD(settlement, maturity, issue, first_coupon, rate, price,
/// redemption, frequency, [basis])` — the inverse of [`oddfprice`], bisected
/// the same way the other yields are.
pub fn oddfyield(epoch: Epoch, args: &[Arg]) -> Value {
    answer(args, || {
        let odd = OddFirst::read(epoch, args)?;
        let target = num_at(args, 5)?;
        if target <= 0.0 {
            return Err(CellError::Num);
        }
        let (mut low, mut high) = (0.0, 1.0);
        let mut guard = 0;
        while odd.price(epoch, high)? > target {
            low = high;
            high *= 2.0;
            guard += 1;
            if guard > 40 {
                return Err(CellError::Num);
            }
        }
        for _ in 0..200 {
            let middle = f64::midpoint(low, high);
            if odd.price(epoch, middle)? > target {
                low = middle;
            } else {
                high = middle;
            }
        }
        Ok(f64::midpoint(low, high))
    })
}
