//! Excel serial dates.
//!
//!
//! The standard library has no date type, and `chrono` would be a dependency
//! for arithmetic Excel does its own way regardless (see [`Epoch`]), so the
//! conversion is computed directly through the Julian day number.

use crate::error::{Error, Result};

/// The workbook's base date.
///
/// `Windows1900` reproduces the Lotus 1-2-3 bug Excel inherited: 1900 is
/// treated as a leap year, so a nonexistent 29 February 1900 occupies serial
/// number 60, and every later date sits one day off the real calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Epoch {
    /// 1 January 1900 = 1. The default.
    #[default]
    Windows1900,
    /// 2 January 1904 = 1. Workbooks created on a Mac.
    Mac1904,
}

impl Epoch {
    /// The Julian day that serial number 0 maps to.
    const fn julian_base(self) -> i64 {
        match self {
            // JD 2415020 = 31 December 1899
            Self::Windows1900 => 2_415_020,
            Self::Mac1904 => 2_416_481,
        }
    }
}

/// A calendar date and time, without a time zone.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DateTime {
    /// Full year.
    pub year: i32,
    /// Month, 1..=12.
    pub month: u32,
    /// Day of month, 1..=31.
    pub day: u32,
    /// Hour, 0..=23.
    pub hour: u32,
    /// Minute, 0..=59.
    pub minute: u32,
    /// Second, fractional part included.
    pub second: f64,
}

impl DateTime {
    /// A date with no time of day.
    #[must_use]
    pub const fn date(year: i32, month: u32, day: u32) -> Self {
        Self {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0.0,
        }
    }
}

/// Julian day from a calendar date (Fliegel-Van Flandern).
const fn to_julian(year: i32, month: u32, day: u32) -> i64 {
    let (y, m) = if month > 2 {
        (year as i64, month as i64 - 3)
    } else {
        (year as i64 - 1, month as i64 + 9)
    };
    let century = y.div_euclid(100);
    let decade = y.rem_euclid(100);
    146_097 * century / 4 + 1461 * decade / 4 + (153 * m + 2) / 5 + day as i64 + 1_721_119
}

/// Calendar date from a Julian day - the inverse of [`to_julian`].
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers bound the serial to 0..=2_958_465, so the date fits"
)]
const fn from_julian(jd: i64) -> (i32, u32, u32) {
    let mut l = jd - 1_721_119;
    let century = (4 * l - 1).div_euclid(146_097);
    l = 4 * l - 1 - 146_097 * century;
    let mut d = l.div_euclid(4);
    let decade = (4 * d + 3).div_euclid(1461);
    d = 4 * d + 3 - 1461 * decade;
    d = (d + 4).div_euclid(4);
    let m = (5 * d - 3).div_euclid(153);
    d = 5 * d - 3 - 153 * m;
    let day = (d + 5).div_euclid(5);
    let mut year = 100 * century + decade;
    let month = if m < 10 {
        m + 3
    } else {
        year += 1;
        m - 9
    };
    (year as i32, month as u32, day as u32)
}

/// Whether such a calendar date actually exists.
const fn is_valid_date(year: i32, month: u32, day: u32) -> bool {
    if month == 0 || month > 12 || day == 0 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    day <= last
}

/// Calendar date to Excel serial number.
///
///
/// # Errors
/// [`Error::DateOutOfRange`] if the date does not exist or precedes the start
/// of the chosen epoch.
pub fn to_serial(dt: DateTime, epoch: Epoch) -> Result<f64> {
    if !is_valid_date(dt.year, dt.month, dt.day) {
        // The one exception is the nonexistent 29 February 1900, which Excel
        // numbers anyway (serial 60).
        if !(epoch == Epoch::Windows1900 && dt.year == 1900 && dt.month == 2 && dt.day == 29) {
            return Err(Error::DateOutOfRange);
        }
    }
    if dt.hour > 23 || dt.minute > 59 || !(0.0..60.0).contains(&dt.second) {
        return Err(Error::DateOutOfRange);
    }

    let mut days = to_julian(dt.year, dt.month, dt.day) - epoch.julian_base();
    // Shift for the phantom 29 February 1900: from 1 March 1900 on, Excel's
    // serial is one greater than the true day count. Test cases
    // `!(year == 1900 && month <= 2)`, which also catches pre-1900 dates - it
    // does not support those anyway.
    if epoch == Epoch::Windows1900 && (dt.year > 1900 || (dt.year == 1900 && dt.month > 2)) {
        days += 1;
    }
    if days < 0 {
        return Err(Error::DateOutOfRange);
    }

    #[expect(clippy::cast_precision_loss, reason = "Excel serials are below 2^31")]
    let day_part = days as f64;
    let time = (f64::from(dt.hour) * 3600.0 + f64::from(dt.minute) * 60.0 + dt.second) / 86_400.0;
    Ok(day_part + time)
}

/// Excel serial number to calendar date.
///
///
/// # Errors
/// [`Error::DateOutOfRange`] for a negative, non-finite or too large value.
pub fn from_serial(serial: f64, epoch: Epoch) -> Result<DateTime> {
    if !serial.is_finite() || !(0.0..=2_958_466.0).contains(&serial) {
        return Err(Error::DateOutOfRange);
    }
    let whole = serial.floor();
    #[expect(clippy::cast_possible_truncation, reason = "upper bound checked above")]
    let mut days = whole as i64;

    // Serial 60 is that phantom 29 February 1900.
    if epoch == Epoch::Windows1900 {
        if days == 60 {
            let frac = serial - whole;
            let (hour, minute, second) = split_time(frac);
            return Ok(DateTime {
                year: 1900,
                month: 2,
                day: 29,
                hour,
                minute,
                second,
            });
        }
        if days > 60 {
            days -= 1;
        }
    }

    let (year, month, day) = from_julian(days + epoch.julian_base());
    let (hour, minute, second) = split_time(serial - whole);
    Ok(DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}

/// Fraction of a day to hours, minutes and seconds.
fn split_time(frac: f64) -> (u32, u32, f64) {
    let total = frac * 86_400.0;
    // Excel keeps time to the millisecond; rounding here removes the binary
    // representation jitter that would turn 12:00 into 11:59:59.
    let total = (total * 1000.0).round() / 1000.0;
    #[expect(clippy::cast_possible_truncation, reason = "total < 86400")]
    #[expect(clippy::cast_sign_loss, reason = "frac is non-negative")]
    let secs = total as u32;
    (
        secs / 3600,
        (secs % 3600) / 60,
        total - f64::from(secs / 60 * 60),
    )
}

#[cfg(test)]
mod tests {
    use super::{DateTime, Epoch, from_serial, to_serial};

    /// Reference pairs taken from Excel's documentation and Excel's own examples.
    const CASES_1900: &[(f64, (i32, u32, u32))] = &[
        (1.0, (1900, 1, 1)),
        (59.0, (1900, 2, 28)),
        (60.0, (1900, 2, 29)), // a day that never existed: the bug, reproduced on purpose
        (61.0, (1900, 3, 1)),
        (367.0, (1901, 1, 1)),
        (25_569.0, (1970, 1, 1)), // the Unix epoch
        (36_526.0, (2000, 1, 1)),
        (45_658.0, (2025, 1, 1)),
        (2_958_465.0, (9999, 12, 31)),
    ];

    #[test]
    fn serial_roundtrip_1900() {
        for &(serial, (y, m, d)) in CASES_1900 {
            let dt = from_serial(serial, Epoch::Windows1900).expect("valid serial");
            assert_eq!((dt.year, dt.month, dt.day), (y, m, d), "serial {serial}");
            assert!(
                (to_serial(DateTime::date(y, m, d), Epoch::Windows1900).unwrap() - serial).abs()
                    < f64::EPSILON,
                "back from {y}-{m}-{d}"
            );
        }
    }

    #[test]
    fn leap_year_1900_bug() {
        // 1900 was not a leap year, but Excel disagrees: an extra day fits
        // between 28 February and 1 March.
        let feb28 = to_serial(DateTime::date(1900, 2, 28), Epoch::Windows1900).unwrap();
        let mar01 = to_serial(DateTime::date(1900, 3, 1), Epoch::Windows1900).unwrap();
        assert!(
            (mar01 - feb28 - 2.0).abs() < f64::EPSILON,
            "Excel inserts a phantom 29 February 1900"
        );
    }

    #[test]
    fn mac_epoch() {
        let dt = from_serial(1.0, Epoch::Mac1904).unwrap();
        assert_eq!((dt.year, dt.month, dt.day), (1904, 1, 2));
        // The same date differs by exactly 1462 days between the two epochs.
        let d = DateTime::date(2000, 1, 1);
        let diff =
            to_serial(d, Epoch::Windows1900).unwrap() - to_serial(d, Epoch::Mac1904).unwrap();
        assert!((diff - 1462.0).abs() < f64::EPSILON);
    }

    #[test]
    fn time_of_day() {
        let noon = to_serial(
            DateTime {
                year: 2025,
                month: 1,
                day: 1,
                hour: 12,
                minute: 0,
                second: 0.0,
            },
            Epoch::Windows1900,
        )
        .unwrap();
        assert!((noon - 45_658.5).abs() < f64::EPSILON);

        let dt = from_serial(45_658.5, Epoch::Windows1900).unwrap();
        assert_eq!((dt.hour, dt.minute), (12, 0));
        assert!(dt.second.abs() < 1e-6);

        let dt = from_serial(45_658.999_988_425_9, Epoch::Windows1900).unwrap();
        assert_eq!((dt.hour, dt.minute), (23, 59), "no drift down by a minute");
    }

    #[test]
    fn rejects_impossible_dates() {
        assert!(to_serial(DateTime::date(2025, 2, 30), Epoch::Windows1900).is_err());
        assert!(to_serial(DateTime::date(2025, 13, 1), Epoch::Windows1900).is_err());
        assert!(to_serial(DateTime::date(1899, 12, 30), Epoch::Windows1900).is_err());
        assert!(from_serial(-1.0, Epoch::Windows1900).is_err());
        assert!(from_serial(f64::NAN, Epoch::Windows1900).is_err());
        // 29 February 2024 exists; 29 February 2025 does not.
        assert!(to_serial(DateTime::date(2024, 2, 29), Epoch::Windows1900).is_ok());
        assert!(to_serial(DateTime::date(2025, 2, 29), Epoch::Windows1900).is_err());
    }
}
