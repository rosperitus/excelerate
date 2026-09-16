//! Dates and times.
//!
//! Every date in Excel is a number: the count of days since the workbook's base
//! date, with the time of day in the fraction. So all of these functions are
//! arithmetic on that number, and all of them need to know which base date the
//! workbook counts from - hence the [`Epoch`] every one of them takes.
//!
//! The calendar they walk is Excel's, not the real one: 1900 is a leap year in
//! a `Windows1900` workbook. An implementation converting to a calendar type uses
//! the true calendar, which is why its `WEEKDAY` disagrees with Excel for the
//! first two months of 1900.

use super::Arg;
use crate::error::CellError;
use crate::formula::value::Value;
use crate::shared::date::{DateTime, Epoch, from_serial, to_serial};

/// `DATE(year, month, day)`
///
/// Excel is generous with the parts: a year below 1900 counts from 1900, a
/// month outside 1..=12 rolls the year over, and a day outside the month rolls
/// the month over. `DATE(2008,14,2)` is 2 February 2009.
pub fn date(epoch: Epoch, args: &[Arg]) -> Value {
    let [y, m, d] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(year), Ok(month), Ok(day)) = (y.number(), m.number(), d.number()) else {
        return first_error(args);
    };
    let (Some(year), Some(month), Some(day)) = (whole(year), whole(month), whole(day)) else {
        return Value::Error(CellError::Num);
    };
    // A one- or two-digit year is an offset from 1900, which is how
    // `DATE(108,1,2)` means 2008.
    let year = if (0..1900).contains(&year) {
        year + 1900
    } else {
        year
    };
    let (year, month) = roll_months(year, month);
    if !(0..10000).contains(&year) {
        return Value::Error(CellError::Num);
    }
    // The day is added rather than validated, so both `DATE(2008,1,35)` and
    // `DATE(2008,1,-15)` land where Excel puts them.
    let Ok(first) = to_serial(DateTime::date(year, month, 1), epoch) else {
        return Value::Error(CellError::Num);
    };
    let serial = first + f64::from(day - 1);
    if serial < 0.0 {
        return Value::Error(CellError::Num);
    }
    Value::Number(serial)
}

/// `TIME(hour, minute, second)` - the fraction of a day, wrapping past 24h.
pub fn time(_: Epoch, args: &[Arg]) -> Value {
    let [h, m, s] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(hour), Ok(minute), Ok(second)) = (h.number(), m.number(), s.number()) else {
        return first_error(args);
    };
    let (Some(hour), Some(minute), Some(second)) = (whole(hour), whole(minute), whole(second))
    else {
        return Value::Error(CellError::Num);
    };
    let total = i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    if total < 0 {
        return Value::Error(CellError::Num);
    }
    #[expect(clippy::cast_precision_loss, reason = "a day is 86400 seconds")]
    let fraction = (total % 86_400) as f64 / 86_400.0;
    Value::Number(fraction)
}

/// `YEAR(serial)`
pub fn year(epoch: Epoch, args: &[Arg]) -> Value {
    part(epoch, args, |dt| f64::from(dt.year))
}

/// `MONTH(serial)`
pub fn month(epoch: Epoch, args: &[Arg]) -> Value {
    part(epoch, args, |dt| f64::from(dt.month))
}

/// `DAY(serial)`
pub fn day(epoch: Epoch, args: &[Arg]) -> Value {
    part(epoch, args, |dt| f64::from(dt.day))
}

/// `HOUR(serial)`
pub fn hour(epoch: Epoch, args: &[Arg]) -> Value {
    part(epoch, args, |dt| f64::from(dt.hour))
}

/// `MINUTE(serial)`
pub fn minute(epoch: Epoch, args: &[Arg]) -> Value {
    part(epoch, args, |dt| f64::from(dt.minute))
}

/// `SECOND(serial)` - whole seconds, rounded as Excel rounds them.
pub fn second(epoch: Epoch, args: &[Arg]) -> Value {
    part(epoch, args, |dt| dt.second.round())
}

/// `WEEKDAY(serial, [type])`
///
/// Type 1 (the default) numbers from Sunday, type 2 from Monday, type 3 from
/// Monday starting at zero; 11 to 17 start the week on Monday through Sunday.
pub fn weekday(epoch: Epoch, args: &[Arg]) -> Value {
    let (serial, kind) = match args {
        [s] => (s.serial(epoch), Ok(1.0)),
        [s, k] => (s.serial(epoch), k.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(serial), Ok(kind)) = (serial, kind) else {
        return first_error(args);
    };
    let (Some(sunday_based), Some(kind)) = (weekday_index(serial, epoch), whole(kind)) else {
        return Value::Error(CellError::Num);
    };
    // How many days after Sunday the week starts on.
    let start = match kind {
        1 | 17 => 0,
        2 | 3 | 11 => 1,
        12 => 2,
        13 => 3,
        14 => 4,
        15 => 5,
        16 => 6,
        _ => return Value::Error(CellError::Num),
    };
    let index = (sunday_based + 7 - start) % 7;
    Value::Number(f64::from(if kind == 3 { index } else { index + 1 }))
}

/// `WEEKNUM(serial, [type])` - the week holding 1 January is week 1.
pub fn weeknum(epoch: Epoch, args: &[Arg]) -> Value {
    let (serial, kind) = match args {
        [s] => (s.serial(epoch), Ok(1.0)),
        [s, k] => (s.serial(epoch), k.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(serial), Ok(kind)) = (serial, kind) else {
        return first_error(args);
    };
    let Some(kind) = whole(kind) else {
        return Value::Error(CellError::Num);
    };
    if kind == 21 {
        return iso_week(epoch, serial);
    }
    let start = match kind {
        1 | 17 => 0,
        2 | 11 => 1,
        12 => 2,
        13 => 3,
        14 => 4,
        15 => 5,
        16 => 6,
        _ => return Value::Error(CellError::Num),
    };
    let Ok(dt) = from_serial(serial, epoch) else {
        return Value::Error(CellError::Num);
    };
    let Ok(jan1) = to_serial(DateTime::date(dt.year, 1, 1), epoch) else {
        return Value::Error(CellError::Num);
    };
    let Some(jan1_dow) = weekday_index(jan1, epoch) else {
        return Value::Error(CellError::Num);
    };
    // Days from the start of the week that holds 1 January.
    let offset = (jan1_dow + 7 - start) % 7;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "both serials are within the sheet's date range"
    )]
    let elapsed = (serial.floor() - jan1) as u32 + offset;
    Value::Number(f64::from(elapsed / 7 + 1))
}

/// `ISOWEEKNUM(serial)` - the ISO 8601 week, whose first week is the one
/// holding the first Thursday of the year.
pub fn isoweeknum(epoch: Epoch, args: &[Arg]) -> Value {
    let [s] = args else {
        return Value::Error(CellError::Value);
    };
    match s.serial(epoch) {
        Ok(serial) => iso_week(epoch, serial),
        Err(e) => Value::Error(e),
    }
}

/// `EDATE(serial, months)` - the same day of the month, that many months away.
pub fn edate(epoch: Epoch, args: &[Arg]) -> Value {
    shift_months(epoch, args, false)
}

/// `EOMONTH(serial, months)` - the last day of that month.
pub fn eomonth(epoch: Epoch, args: &[Arg]) -> Value {
    shift_months(epoch, args, true)
}

/// Shared body of `EDATE` and `EOMONTH`.
fn shift_months(epoch: Epoch, args: &[Arg], to_end: bool) -> Value {
    let [s, m] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(serial), Ok(months)) = (s.serial(epoch), m.number()) else {
        return first_error(args);
    };
    let (Ok(dt), Some(months)) = (from_serial(serial, epoch), whole(months)) else {
        return Value::Error(CellError::Num);
    };
    #[expect(
        clippy::cast_possible_wrap,
        reason = "a month number is between 1 and 12"
    )]
    let (year, month) = roll_months(dt.year, dt.month as i32 + months);
    let last = days_in_month(year, month);
    // A day that the target month does not have is pulled back to its end:
    // one month after 31 January is 28 February.
    let day = if to_end { last } else { dt.day.min(last) };
    match to_serial(DateTime::date(year, month, day), epoch) {
        Ok(serial) => Value::Number(serial),
        Err(_) => Value::Error(CellError::Num),
    }
}

/// `DAYS(end, start)` - plain difference, which is subtraction in disguise.
pub fn days(epoch: Epoch, args: &[Arg]) -> Value {
    let [end, start] = args else {
        return Value::Error(CellError::Value);
    };
    match (end.serial(epoch), start.serial(epoch)) {
        (Ok(e), Ok(s)) => Value::Number(e.floor() - s.floor()),
        (Err(e), _) | (_, Err(e)) => Value::Error(e),
    }
}

/// `DAYS360(start, end, [european])` - every month counted as thirty days.
pub fn days360(epoch: Epoch, args: &[Arg]) -> Value {
    let (start, end, european) = match args {
        [s, e] => (s.serial(epoch), e.serial(epoch), Ok(false)),
        [s, e, m] => (s.serial(epoch), e.serial(epoch), m.value.boolean()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(start), Ok(end), Ok(european)) = (start, end, european) else {
        return first_error(args);
    };
    let (Ok(a), Ok(b)) = (from_serial(start, epoch), from_serial(end, epoch)) else {
        return Value::Error(CellError::Num);
    };
    let (mut d1, mut d2) = (a.day, b.day);
    if european {
        d1 = d1.min(30);
        d2 = d2.min(30);
    } else {
        // The American rule moves the end of a month to the 30th, but only
        // moves the later date when the earlier one has already been moved.
        if d1 == days_in_month(a.year, a.month) {
            d1 = 30;
        }
        if d2 == days_in_month(b.year, b.month) && d1 == 30 {
            d2 = 30;
        }
    }
    let months = (b.year - a.year) * 12 + i32::try_from(b.month).unwrap_or(0)
        - i32::try_from(a.month).unwrap_or(0);
    let days = i32::try_from(d2).unwrap_or(0) - i32::try_from(d1).unwrap_or(0);
    Value::Number(f64::from(months * 30 + days))
}

/// `DATEDIF(start, end, unit)`
///
/// Units are `Y`, `M` and `D` for whole years, months and days, and `MD`, `YM`
/// and `YD` for the remainder after taking the larger unit out.
pub fn datedif(epoch: Epoch, args: &[Arg]) -> Value {
    let [first, last, unit] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(start), Ok(end), Ok(unit)) = (first.serial(epoch), last.serial(epoch), unit.text())
    else {
        return first_error(args);
    };
    if end < start {
        return Value::Error(CellError::Num);
    }
    let (Ok(a), Ok(b)) = (from_serial(start, epoch), from_serial(end, epoch)) else {
        return Value::Error(CellError::Num);
    };
    // Whole months between the two dates, the day of the month deciding
    // whether the last one counts.
    let mut months = (b.year - a.year) * 12 + i32::try_from(b.month).unwrap_or(0)
        - i32::try_from(a.month).unwrap_or(0);
    if b.day < a.day {
        months -= 1;
    }
    let value = match unit.to_uppercase().as_str() {
        "Y" => f64::from(months / 12),
        "M" => f64::from(months),
        "D" => end.floor() - start.floor(),
        "MD" => {
            // Days since the same day of the previous month.
            let (year, month) = roll_months(b.year, i32::try_from(b.month).unwrap_or(1) - 1);
            let day = a.day.min(days_in_month(year, month));
            match to_serial(DateTime::date(year, month, day), epoch) {
                Ok(anchor) if anchor <= end => end.floor() - anchor,
                _ => return Value::Error(CellError::Num),
            }
        }
        "YM" => f64::from(months % 12),
        "YD" => {
            // Excel measures the remainder inside the *start* year, so the end
            // date is moved back to it: a leap February there counts even when
            // the end year has none.
            let year = if (b.month, b.day) < (a.month, a.day) {
                a.year + 1
            } else {
                a.year
            };
            let day = b.day.min(days_in_month(year, b.month));
            let Ok(anchor) = to_serial(DateTime::date(year, b.month, day), epoch) else {
                return Value::Error(CellError::Num);
            };
            // And a quirk of its own: when the year it lands in is a leap year
            // that neither of the two dates was in, the extra day is taken
            // back off again.
            let borrowed_leap = is_leap(year)
                && !is_leap(a.year)
                && !is_leap(b.year)
                && (b.month, b.day) >= (2, 29);
            anchor - start.floor() - f64::from(u8::from(borrowed_leap))
        }
        _ => return Value::Error(CellError::Num),
    };
    Value::Number(value)
}

/// Which days of the week a `.INTL` function is to treat as the weekend.
///
/// Excel spells it two ways: a code from its own table, or a seven-character
/// mask starting on Monday where `1` marks a day off. Both end up here as one
/// bit per day, indexed the way [`weekday_index`] numbers them (0 = Sunday).
#[derive(Debug, Clone, Copy)]
struct Weekend([bool; 7]);

impl Weekend {
    /// The default of `NETWORKDAYS` and `WORKDAY`: Saturday and Sunday.
    const SATURDAY_SUNDAY: Self = Self([true, false, false, false, false, false, true]);

    /// Reads the `weekend` argument of a `.INTL` function.
    ///
    /// The codes are Excel's own: 1..=7 are the seven two-day weekends
    /// starting with Saturday/Sunday, and 11..=17 the seven one-day ones
    /// starting with Sunday. A mask is seven characters of `0` and `1`
    /// beginning on Monday; `"1111111"` - every day off - is `#VALUE!`,
    /// because nothing would ever be a working day.
    fn parse(value: &Value) -> Result<Self, CellError> {
        if let Value::Text(mask) = value {
            let chars: Vec<char> = mask.chars().collect();
            if chars.len() != 7 || chars.iter().any(|c| !matches!(c, '0' | '1')) {
                return Err(CellError::Value);
            }
            if chars.iter().all(|&c| c == '1') {
                return Err(CellError::Value);
            }
            let mut days = [false; 7];
            for (i, c) in chars.iter().enumerate() {
                // The mask starts on Monday, `weekday_index` on Sunday.
                days[(i + 1) % 7] = *c == '1';
            }
            return Ok(Self(days));
        }
        let arg = Arg {
            value: value.clone(),
            reference: false,
        };
        let code = arg.number()?;
        let Some(code) = whole(code) else {
            return Err(CellError::Num);
        };
        let mut days = [false; 7];
        let code = usize::try_from(code).map_err(|_| CellError::Num)?;
        match code {
            // 1 is Saturday/Sunday, 2 Sunday/Monday, and so on round the week.
            1..=7 => {
                let first = (code + 5) % 7;
                days[first] = true;
                days[(first + 1) % 7] = true;
            }
            // 11 is Sunday alone, 12 Monday, and so on.
            11..=17 => days[(code - 11) % 7] = true,
            _ => return Err(CellError::Num),
        }
        Ok(Self(days))
    }

    /// Whether a serial falls on a day this weekend covers.
    fn covers(self, serial: f64, epoch: Epoch) -> bool {
        weekday_index(serial, epoch).is_some_and(|day| self.0[day as usize])
    }
}

/// `NETWORKDAYS(start, end, [holidays])` - working days in the span, both ends
/// included.
pub fn networkdays(epoch: Epoch, args: &[Arg]) -> Value {
    let (start, end, rest) = match args.split_at_checked(2) {
        Some(([s, e], rest)) => (s.serial(epoch), e.serial(epoch), rest),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(start), Ok(end)) = (start, end) else {
        return first_error(args);
    };
    net(epoch, start, end, Weekend::SATURDAY_SUNDAY, rest)
}

/// `NETWORKDAYS.INTL(start, end, [weekend], [holidays])` - the same, with the
/// weekend named rather than assumed.
pub fn networkdays_intl(epoch: Epoch, args: &[Arg]) -> Value {
    let (start, end, weekend, rest) = match args.split_at_checked(2) {
        Some(([s, e], rest)) => {
            let (weekend, rest) = split_weekend(rest);
            (s.serial(epoch), e.serial(epoch), weekend, rest)
        }
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(start), Ok(end)) = (start, end) else {
        return first_error(args);
    };
    let weekend = match weekend {
        Ok(w) => w,
        Err(e) => return Value::Error(e),
    };
    net(epoch, start, end, weekend, rest)
}

/// The `weekend` argument and the holidays after it, as a `.INTL` function
/// receives them: both are optional.
fn split_weekend(rest: &[Arg]) -> (Result<Weekend, CellError>, &[Arg]) {
    match rest.split_first() {
        // An omitted argument (`NETWORKDAYS.INTL(a,b,,h)`) means the default.
        Some((first, tail)) => match first.value.scalar() {
            Value::Blank => (Ok(Weekend::SATURDAY_SUNDAY), tail),
            value => (Weekend::parse(value), tail),
        },
        None => (Ok(Weekend::SATURDAY_SUNDAY), rest),
    }
}

/// Shared body of `NETWORKDAYS` and `NETWORKDAYS.INTL`.
fn net(epoch: Epoch, start: f64, end: f64, weekend: Weekend, rest: &[Arg]) -> Value {
    let holidays = match holidays(epoch, rest) {
        Ok(h) => h,
        Err(e) => return Value::Error(e),
    };
    let (from, to) = (
        start.floor().min(end.floor()),
        start.floor().max(end.floor()),
    );
    let sign = if end < start { -1.0 } else { 1.0 };
    let mut count = 0.0;
    let mut serial = from;
    while serial <= to {
        if !weekend.covers(serial, epoch) && !holidays.contains(&serial) {
            count += 1.0;
        }
        serial += 1.0;
    }
    Value::Number(count * sign)
}

/// `WORKDAY(start, days, [holidays])` - the date that many working days away.
pub fn workday(epoch: Epoch, args: &[Arg]) -> Value {
    let (start, count, rest) = match args.split_at_checked(2) {
        Some(([s, d], rest)) => (s.serial(epoch), d.number(), rest),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(start), Ok(count)) = (start, count) else {
        return first_error(args);
    };
    work(epoch, start, count, Weekend::SATURDAY_SUNDAY, rest)
}

/// `WORKDAY.INTL(start, days, [weekend], [holidays])` - the same, with the
/// weekend named rather than assumed.
pub fn workday_intl(epoch: Epoch, args: &[Arg]) -> Value {
    let (start, count, weekend, rest) = match args.split_at_checked(2) {
        Some(([s, d], rest)) => {
            let (weekend, rest) = split_weekend(rest);
            (s.serial(epoch), d.number(), weekend, rest)
        }
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(start), Ok(count)) = (start, count) else {
        return first_error(args);
    };
    let weekend = match weekend {
        Ok(w) => w,
        Err(e) => return Value::Error(e),
    };
    work(epoch, start, count, weekend, rest)
}

/// Shared body of `WORKDAY` and `WORKDAY.INTL`.
fn work(epoch: Epoch, start: f64, count: f64, weekend: Weekend, rest: &[Arg]) -> Value {
    let holidays = match holidays(epoch, rest) {
        Ok(h) => h,
        Err(e) => return Value::Error(e),
    };
    let step = if count < 0.0 { -1.0 } else { 1.0 };
    let mut left = count.abs().trunc();
    let mut serial = start.floor();
    while left > 0.0 {
        serial += step;
        if serial < 0.0 {
            return Value::Error(CellError::Num);
        }
        if !weekend.covers(serial, epoch) && !holidays.contains(&serial) {
            left -= 1.0;
        }
    }
    Value::Number(serial)
}

/// `YEARFRAC(start, end, [basis])`
///
/// Basis 0 is the American 30/360 convention, 1 counts actual days over the
/// actual year, 2 over 360, 3 over 365, and 4 is the European 30/360.
pub fn yearfrac(epoch: Epoch, args: &[Arg]) -> Value {
    let (start, end, basis) = match args {
        [s, e] => (s.serial(epoch), e.serial(epoch), Ok(0.0)),
        [s, e, b] => (s.serial(epoch), e.serial(epoch), b.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(start), Ok(end), Ok(basis)) = (start, end, basis) else {
        return first_error(args);
    };
    let Some(basis) = whole(basis) else {
        return Value::Error(CellError::Num);
    };
    match year_fraction(epoch, start, end, basis) {
        Some(fraction) => Value::Number(fraction),
        None => Value::Error(CellError::Num),
    }
}

/// The day-count fraction of a span, which is `YEARFRAC` and also the measure
/// every French depreciation and every bond function is written in terms of.
///
/// Basis 0 is the American 30/360 convention, 1 counts actual days over the
/// actual year, 2 over 360, 3 over 365, and 4 is the European 30/360.
pub(crate) fn year_fraction(epoch: Epoch, start: f64, end: f64, basis: i32) -> Option<f64> {
    let (from, to) = (
        start.floor().min(end.floor()),
        start.floor().max(end.floor()),
    );
    let (Ok(a), Ok(b)) = (from_serial(from, epoch), from_serial(to, epoch)) else {
        return None;
    };
    let days360 = |european: bool| {
        let (mut d1, mut d2) = (a.day, b.day);
        if european {
            d1 = d1.min(30);
            d2 = d2.min(30);
        } else {
            if d1 == days_in_month(a.year, a.month) {
                d1 = 30;
            }
            if d2 == days_in_month(b.year, b.month) && d1 == 30 {
                d2 = 30;
            }
        }
        let months = (b.year - a.year) * 12 + i32::try_from(b.month).unwrap_or(0)
            - i32::try_from(a.month).unwrap_or(0);
        f64::from(months * 30 + i32::try_from(d2).unwrap_or(0) - i32::try_from(d1).unwrap_or(0))
    };
    Some(match basis {
        0 => days360(false) / 360.0,
        1 => {
            // The denominator is the average length of the years the span
            // touches, which is what makes basis 1 "actual/actual".
            let years = b.year - a.year + 1;
            let leaps = (a.year..=b.year).filter(|y| is_leap(*y)).count();
            #[expect(
                clippy::cast_precision_loss,
                reason = "a span covers far fewer years than a double can count"
            )]
            let average = 365.0 + leaps as f64 / f64::from(years);
            (to - from) / average
        }
        2 => (to - from) / 360.0,
        3 => (to - from) / 365.0,
        4 => days360(true) / 360.0,
        _ => return None,
    })
}

/// `TODAY()` - today's date, with no time of day.
///
/// The clock is read in UTC: time zones are not modelled anywhere in the crate
/// yet, and neither is the "recalculate on open" that makes this function
/// volatile in Excel.
pub fn today(epoch: Epoch, args: &[Arg]) -> Value {
    match clock(epoch, args) {
        Value::Number(serial) => Value::Number(serial.floor()),
        other => other,
    }
}

/// `NOW()` - the date and time, in UTC. See [`today`].
pub fn now(epoch: Epoch, args: &[Arg]) -> Value {
    clock(epoch, args)
}

/// Shared body of `TODAY` and `NOW`.
fn clock(epoch: Epoch, args: &[Arg]) -> Value {
    if !args.is_empty() {
        return Value::Error(CellError::Value);
    }
    let Ok(unix) = to_serial(DateTime::date(1970, 1, 1), epoch) else {
        return Value::Error(CellError::Num);
    };
    Value::Number(unix + crate::shared::unix_seconds() / 86_400.0)
}

/// `DATEVALUE(text)` - the date a string spells, without its time.
///
/// Text that holds only a time is not a date, so it is `#VALUE!` here even
/// though `TIMEVALUE` reads the same string happily.
pub fn datevalue(epoch: Epoch, args: &[Arg]) -> Value {
    from_text(epoch, args, |parsed, epoch| {
        to_serial(parsed.date?, epoch).ok()
    })
}

/// `TIMEVALUE(text)` - the time of day a string spells, as a fraction.
///
/// A string carrying both gives up its time alone, which is what makes
/// `TIMEVALUE("2015-05-31 13:00")` half past noon rather than a date.
pub fn timevalue(epoch: Epoch, args: &[Arg]) -> Value {
    from_text(epoch, args, |parsed, _| parsed.time)
}

/// Shared body of `DATEVALUE` and `TIMEVALUE`.
fn from_text(
    epoch: Epoch,
    args: &[Arg],
    pick: fn(crate::shared::date_parse::Parsed, Epoch) -> Option<f64>,
) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    // A number is already what these return; only text is read.
    let text = match arg.value.scalar() {
        Value::Text(text) => text.clone(),
        Value::Error(e) => return Value::Error(*e),
        other => {
            return other
                .number()
                .map_or(Value::Error(CellError::Value), Value::Number);
        }
    };
    crate::shared::date_parse::parse(&text)
        .and_then(|parsed| pick(parsed, epoch))
        .map_or(Value::Error(CellError::Value), Value::Number)
}

/// Applies a body to the date a single serial argument stands for.
fn part(epoch: Epoch, args: &[Arg], body: fn(DateTime) -> f64) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    let serial = match arg.serial(epoch) {
        Ok(serial) => serial,
        Err(e) => return Value::Error(e),
    };
    let Ok(mut dt) = from_serial(serial, epoch) else {
        return Value::Error(CellError::Num);
    };
    // Excel calls serial 0 "0 January 1900" - a day that is not on any
    // calendar, but one it still answers about: `YEAR(0)` is 1900 and `DAY(0)`
    // is 0. The arithmetic elsewhere keeps the honest 31 December 1899.
    if epoch == Epoch::Windows1900 && serial.floor() == 0.0 {
        dt.year = 1900;
        dt.month = 1;
        dt.day = 0;
    }
    Value::Number(body(dt))
}

/// The first error among the arguments, for the functions that read several
/// numbers at once.
fn first_error(args: &[Arg]) -> Value {
    super::first_error(args).map_or(Value::Error(CellError::Value), Value::Error)
}

/// A number as a whole one, or `None` when it is not.
fn whole(n: f64) -> Option<i32> {
    if !n.is_finite() || n.abs() > f64::from(i32::MAX) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded just above; Excel drops the fraction of a date part"
    )]
    let truncated = n.trunc() as i32;
    Some(truncated)
}

/// Normalises a month outside 1..=12 into the year, as `DATE` and `EDATE` do.
fn roll_months(year: i32, month: i32) -> (i32, u32) {
    let zero_based = month - 1;
    let year = year + zero_based.div_euclid(12);
    // `rem_euclid` of 12 is between 0 and 11, so the conversion cannot fail.
    let month = u32::try_from(zero_based.rem_euclid(12)).unwrap_or(0) + 1;
    (year, month)
}

/// Whether a year has a 29 February.
pub(crate) const fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// How many days a month has.
pub(crate) const fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

/// Day of the week as an offset from Sunday, on Excel's calendar rather than
/// the real one.
fn weekday_index(serial: f64, epoch: Epoch) -> Option<u32> {
    if !(0.0..=2_958_466.0).contains(&serial) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bounded just above"
    )]
    let days = serial.floor() as u64;
    // Serial 1 is 1 January 1900, a Sunday; in the 1904 epoch it is
    // 2 January 1904, a Saturday.
    let index = match epoch {
        Epoch::Windows1900 => (days + 6) % 7,
        Epoch::Mac1904 => (days + 5) % 7,
    };
    u32::try_from(index).ok()
}

/// The holiday list of `NETWORKDAYS` and `WORKDAY`, flattened to whole days.
///
/// A holiday may be written as text, the same as any other date argument.
fn holidays(epoch: Epoch, args: &[Arg]) -> Result<Vec<f64>, CellError> {
    let mut out = Vec::new();
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for v in flat {
            match v {
                Value::Blank => {}
                Value::Error(e) => return Err(*e),
                other => {
                    let arg = Arg {
                        value: other.clone(),
                        reference: true,
                    };
                    out.push(arg.serial(epoch)?.floor());
                }
            }
        }
    }
    Ok(out)
}

/// The ISO 8601 week number: week 1 is the one holding the first Thursday.
fn iso_week(epoch: Epoch, serial: f64) -> Value {
    let Some(dow) = weekday_index(serial, epoch) else {
        return Value::Error(CellError::Num);
    };
    // Monday of this week, then the Thursday in it, whose year names the week.
    let monday = serial.floor() - f64::from((dow + 6) % 7);
    let thursday = monday + 3.0;
    let Ok(dt) = from_serial(thursday, epoch) else {
        return Value::Error(CellError::Num);
    };
    let Ok(jan1) = to_serial(DateTime::date(dt.year, 1, 1), epoch) else {
        return Value::Error(CellError::Num);
    };
    Value::Number(((thursday - jan1) / 7.0).floor() + 1.0)
}
