//! Reading a date or a time out of text.
//!
//!
//! What is recognised:
//!
//! | Text | Read as |
//! |---|---|
//! | `2015-05-31`, `2015/5/3` | year first when the first number is four digits |
//! | `12/25/2012`, `2-28-1900` | month, day, year — the American order |
//! | `31-May-2015`, `May 31, 2015` | a month name settles which number is which |
//! | `1st May 2015` | the ordinal suffix is dropped |
//! | `1/1/29`, `1/1/30` | 2029 and 1930: under 30 is this century |
//! | `13:35:55`, `1:00 PM`, `8:55` | a time, as a fraction of the day |
//! | `12/09/2015 08:55` | both, added together |

use super::date::{DateTime, Epoch, to_serial};

/// What a string turned out to hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parsed {
    /// The date part, if the text carried one.
    pub date: Option<DateTime>,
    /// The time of day as a fraction, if the text carried one.
    pub time: Option<f64>,
}

impl Parsed {
    /// The serial number for the whole of it.
    ///
    /// A date with no time is a whole number; a time with no date is the
    /// fraction alone, which is what Excel means by a bare time.
    #[must_use]
    pub fn serial(self, epoch: Epoch) -> Option<f64> {
        let day = match self.date {
            Some(date) => to_serial(date, epoch).ok()?,
            None => 0.0,
        };
        Some(day + self.time.unwrap_or(0.0))
    }
}

/// Reads a date, a time, or both out of text.
///
/// Returns `None` for anything that is not one, including text with no digit
/// in it at all — the cheap check makes first.
#[must_use]
pub fn parse(text: &str) -> Option<Parsed> {
    let text = text.trim().trim_matches('"');
    if !text.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }

    // The time is whichever word holds a colon; the date is what is left.
    let (time_word, rest) = split_time(text);
    let time = match time_word {
        Some(word) => Some(parse_time(&word)?),
        None => None,
    };
    let date = if rest.trim().is_empty() {
        None
    } else {
        Some(parse_date(&rest)?)
    };
    if date.is_none() && time.is_none() {
        return None;
    }
    Some(Parsed { date, time })
}

/// Splits off the part of the text that names a time of day.
///
/// The meridiem stays with it: `1:00 PM` is one word for this purpose, and so
/// is `08:55` on its own.
fn split_time(text: &str) -> (Option<String>, String) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let Some(at) = words.iter().position(|w| w.contains(':')) else {
        return (None, text.to_owned());
    };
    let mut time = words[at].to_owned();
    let mut used = vec![at];
    // A meridiem written apart from the clock belongs to it.
    if let Some(next) = words.get(at + 1)
        && is_meridiem(next)
    {
        time.push(' ');
        time.push_str(next);
        used.push(at + 1);
    }
    let rest: Vec<&str> = words
        .iter()
        .enumerate()
        .filter(|(i, _)| !used.contains(i))
        .map(|(_, w)| *w)
        .collect();
    (Some(time), rest.join(" "))
}

/// Whether a word is `am` or `pm`, however it is written.
fn is_meridiem(word: &str) -> bool {
    matches!(
        word.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "am" | "pm" | "a.m" | "p.m"
    )
}

/// Reads `13:35:55`, `1:00 PM` or `8:55` as a fraction of a day.
///
/// Minutes must be a real minute; hours and seconds are carried, so
/// `13:10:60` is eleven minutes past one and `25:00:00` is one in the morning.
fn parse_time(text: &str) -> Option<f64> {
    let lower = text.to_ascii_lowercase();
    let (body, meridiem) =
        if let Some(body) = lower.strip_suffix("pm").or(lower.strip_suffix("p.m")) {
            (body, Some(true))
        } else if let Some(body) = lower.strip_suffix("am").or(lower.strip_suffix("a.m")) {
            (body, Some(false))
        } else {
            (lower.as_str(), None)
        };

    // Hours and minutes are whole; only the seconds may carry a fraction.
    let mut parts = body.trim().split(':');
    let hour: u32 = parts.next()?.trim().parse().ok()?;
    let minute: u32 = parts.next()?.trim().parse().ok()?;
    let second: f64 = match parts.next() {
        Some(s) => s.trim().parse().ok()?,
        None => 0.0,
    };
    if parts.next().is_some() || minute > 59 || second < 0.0 {
        return None;
    }
    let hour = match meridiem {
        // Twelve is the odd one out: noon is 12, midnight is 0.
        Some(true) if hour < 12 => hour + 12,
        Some(false) if hour == 12 => 0,
        Some(_) | None => hour,
    };
    if meridiem.is_some() && hour > 24 {
        return None;
    }
    let seconds = f64::from(hour) * 3600.0 + f64::from(minute) * 60.0 + second;
    Some((seconds / 86_400.0).rem_euclid(1.0))
}

/// Reads the date part, whatever order it is written in.
fn parse_date(text: &str) -> Option<DateTime> {
    let mut month_name: Option<u32> = None;
    let mut numbers: Vec<(u32, usize)> = Vec::new();

    for token in text
        .split(|c: char| c == '/' || c == '-' || c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
    {
        let token = strip_ordinal(token);
        if token.is_empty() {
            continue;
        }
        if let Ok(n) = token.parse::<u32>() {
            numbers.push((n, token.len()));
            continue;
        }
        match month_of(token) {
            // Two month names is not a date.
            Some(_) if month_name.is_some() => return None,
            Some(m) => month_name = Some(m),
            None => return None,
        }
    }

    match (month_name, numbers.len()) {
        (Some(month), 1) => {
            let (n, digits) = numbers[0];
            // One number beside a month name is either its day or its year.
            if digits >= 3 || n > 31 {
                build(year_of(n, digits), month, 1)
            } else {
                build(this_year(), month, n)
            }
        }
        (Some(month), 2) => {
            let ((a, a_digits), (b, b_digits)) = (numbers[0], numbers[1]);
            // Whichever of the two looks like a year is one; when neither
            // does, the written order settles it — `25-Jan-20` is the
            // twenty-fifth of January 2020, day before year.
            if a_digits >= 3 || a > 31 {
                build(year_of(a, a_digits), month, b)
            } else {
                build(year_of(b, b_digits), month, a)
            }
        }
        (None, 2) => {
            // `5/31` is this year, the way Excel fills in the missing part.
            let ((month, _), (day, _)) = (numbers[0], numbers[1]);
            build(this_year(), month, day)
        }
        (None, 3) => {
            let ((a, a_digits), (b, _), (c, c_digits)) = (numbers[0], numbers[1], numbers[2]);
            if a_digits >= 3 {
                // Four digits first is a year, and the rest is month then day.
                build(year_of(a, a_digits), b, c)
            } else {
                // Otherwise month, day, year -- the American order Excel reads.
                build(year_of(c, c_digits), a, b)
            }
        }
        _ => None,
    }
}

/// A date if the parts make one, and nothing if they do not.
fn build(year: i32, month: u32, day: u32) -> Option<DateTime> {
    let dt = DateTime::date(year, month, day);
    // `to_serial` is the calendar this crate keeps, including Excel's phantom
    // 29 February 1900, so it is also the check for whether a date exists.
    to_serial(dt, Epoch::Windows1900).ok().map(|_| dt)
}

/// A written year as the year it means.
///
/// Two digits are this century up to 29 and the last one from 30, which is the
/// window Excel uses and every spreadsheet has copied since.
fn year_of(n: u32, digits: usize) -> i32 {
    let n = i32::try_from(n).unwrap_or(0);
    if digits >= 3 || n > 99 {
        return n;
    }
    if n < 30 { 2000 + n } else { 1900 + n }
}

/// The current year, for a date written without one.
fn this_year() -> i32 {
    // Days from the Unix epoch to 1970-01-01 as an Excel serial is 25569; the
    // clock only has to be good enough to name the year.
    let serial = 25_569.0 + super::unix_seconds() / 86_400.0;
    super::date::from_serial(serial, Epoch::Windows1900).map_or(1900, |dt| dt.year)
}

/// Drops the `st`, `nd`, `rd` or `th` after a number, which Excel allows.
fn strip_ordinal(token: &str) -> &str {
    let lower = token.to_ascii_lowercase();
    for suffix in ["st", "nd", "rd", "th"] {
        if let Some(head) = lower.strip_suffix(suffix)
            && !head.is_empty()
            && head.chars().all(|c| c.is_ascii_digit())
        {
            return &token[..head.len()];
        }
    }
    token
}

/// The month a name stands for, full or shortened, in any case.
fn month_of(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let name = name.to_ascii_lowercase();
    if name.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| *m == name || m.starts_with(&name) && name.len() == 3)
        .map(|i| u32::try_from(i + 1).unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial(text: &str) -> Option<f64> {
        parse(text)?.serial(Epoch::Windows1900)
    }

    #[test]
    fn the_american_order_is_the_one_excel_reads() {
        // 30805 is 3 May 1984. An implementation reading the string by the
        // machine's locale answers 30746 -- 5 March -- instead.
        assert_eq!(serial("05/03/1984"), Some(30805.0));
        assert_eq!(serial("12/25/2012"), Some(41268.0));
        assert_eq!(serial("2-28-1900"), Some(59.0));
        // A month past twelve is not a month, whatever the day might be.
        assert_eq!(serial("31/12/2015"), None);
        assert_eq!(serial("0/1/2015"), None);
    }

    #[test]
    fn four_digits_first_means_the_year_leads() {
        assert_eq!(serial("2015-05-31"), Some(42155.0));
        assert_eq!(serial("2015/05/31"), Some(42155.0));
        assert_eq!(serial("2015-5-3"), Some(42127.0));
    }

    #[test]
    fn a_month_name_settles_which_number_is_which() {
        assert_eq!(serial("31-May-2015"), Some(42155.0));
        assert_eq!(serial("May 31, 2015"), Some(42155.0));
        assert_eq!(serial("31 May 2015"), Some(42155.0));
        assert_eq!(serial("January 25, 2020"), Some(43855.0));
        assert_eq!(serial("Jan 25 2020"), Some(43855.0));
        // Case does not matter, and neither does an ordinal suffix.
        assert_eq!(serial("31-MAY-2015"), Some(42155.0));
        assert_eq!(serial("1st May 2015"), Some(42125.0));
        assert_eq!(serial("3rd March 2015"), Some(42066.0));
        // A month and a year alone is the first of that month.
        assert_eq!(serial("May-2015"), Some(42125.0));
    }

    #[test]
    fn a_two_digit_year_turns_at_thirty() {
        assert_eq!(serial("1/1/29"), Some(47119.0));
        assert_eq!(serial("1/1/30"), Some(10959.0));
        assert_eq!(serial("1/1/99"), Some(36161.0));
        assert_eq!(serial("25-Jan-20"), Some(43855.0));
    }

    #[test]
    fn a_time_is_a_fraction_of_the_day() {
        let close = |text: &str, want: f64| {
            let got = serial(text).unwrap_or(f64::NAN);
            assert!((got - want).abs() < 1e-9, "{text}: {got} != {want}");
        };
        close("13:35:55", 0.566_608_796_296_296);
        close("1:00 PM", 0.541_666_666_666_667);
        close("01:00 AM", 0.041_666_666_666_667);
        close("12:00 AM", 0.0);
        close("12:00 PM", 0.5);
        close("8:55", 0.371_527_777_777_778);
        // Seconds and hours carry; minutes have to be minutes.
        close("13:10:60", 0.549_305_555_555_556);
        close("25:00:00", 0.041_666_666_666_667);
        close("13:35:55.5", 0.566_614_583_333_333);
        assert_eq!(serial("1:70"), None);
    }

    #[test]
    fn a_date_and_a_time_add_up() {
        let got = serial("12/09/2015 08:55").expect("reads");
        assert!((got - 42_347.371_527_777_8).abs() < 1e-6, "{got}");
        assert_eq!(parse("13:35:55").expect("reads").date, None);
        assert_eq!(parse("2015-05-31").expect("reads").time, None);
    }

    #[test]
    fn what_is_not_a_date_is_not_read_as_one() {
        assert_eq!(parse("nonsense"), None);
        assert_eq!(parse(""), None);
        assert_eq!(serial("2/29/2019"), None);
        assert_eq!(serial("12/32/2015"), None);
        // The one date that does not exist but Excel numbers anyway.
        assert_eq!(serial("2/29/1900"), Some(60.0));
    }
}
