//! Rendering values through Excel number-format strings.
//!
//!
//! A format string is up to four sections separated by `;`, applied by the
//! value's sign: positive, negative, zero, text. With fewer sections Excel
//! falls back in a specific way - see [`Sections::pick`], which is where most
//! of the subtlety of this module lives.
//!
//! Within a section, the characters that matter are the digit placeholders
//! (`0`, `#`, `?`), the decimal point, the thousands separator, the date and
//! time codes, and the quoting forms (`"..."`, `\x`, `_x`, `*x`). Anything else
//! is a literal.

use crate::shared::date::{Epoch, from_serial};
use crate::style::NumberFormat;
use std::fmt::Write as _;

/// The format code Excel uses when a cell has no explicit format.
pub const GENERAL: &str = "General";

impl NumberFormat {
    /// The format string this format stands for.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::General => GENERAL,
            Self::Builtin(id) => builtin_code(*id),
            Self::Custom(code) => code,
        }
    }
}

/// The format string of a built-in format id, or `General` if unknown.
///
#[must_use]
pub fn builtin_code(id: u16) -> &'static str {
    match id {
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "m/d/yyyy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yyyy h:mm",
        27 | 36 => "[$-404]e/m/d",
        28 | 29 => r#"[$-411]ggge"年"m"月"d"日""#,
        30 => "m/d/yy",
        31 => r#"yyyy"年"m"月"d"日""#,
        32 => r#"h"時"mm"分""#,
        33 => r#"h"時"mm"分"ss"秒""#,
        34 => r#"yyyy"年"m"月""#,
        35 => r#"m"月"d"日""#,
        37 => "#,##0_);(#,##0)",
        38 => "#,##0_);[Red](#,##0)",
        39 => "#,##0.00_);(#,##0.00)",
        40 => "#,##0.00_);[Red](#,##0.00)",
        44 => r#"_("$"* #,##0.00_);_("$"* \(#,##0.00\);_("$"* "-"??_);_(@_)"#,
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => GENERAL,
    }
}

/// A value to render.
#[derive(Debug, Clone, Copy)]
pub enum Value<'a> {
    /// A number, which a date format reads as a serial date.
    Number(f64),
    /// Text, which only the fourth section of a format applies to.
    Text(&'a str),
}

/// Renders a value through a format string.
///
/// The epoch matters only for date formats; pass the workbook's.
#[must_use]
pub fn format(value: Value<'_>, code: &str, epoch: Epoch) -> String {
    let sections = Sections::split(code);
    let (section, negate) = sections.pick(value);

    match value {
        Value::Text(t) => render_text(section, t),
        Value::Number(n) => {
            if section.trim().is_empty() && !sections.only_one {
                // An empty section means "render nothing for this sign".
                return String::new();
            }
            render_number(section, if negate { -n } else { n }, epoch)
        }
    }
}

/// The sections of a format string.
struct Sections<'a> {
    parts: Vec<&'a str>,
    only_one: bool,
}

impl<'a> Sections<'a> {
    /// Splits on `;`, respecting quoting so a separator inside `"..."` or after
    /// a backslash stays part of the literal.
    fn split(code: &'a str) -> Self {
        let mut parts = Vec::new();
        let (mut start, mut in_quotes) = (0, false);
        let mut chars = code.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => in_quotes = !in_quotes,
                '\\' => {
                    chars.next();
                }
                ';' if !in_quotes => {
                    parts.push(&code[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        parts.push(&code[start..]);
        let only_one = parts.len() == 1;
        Self { parts, only_one }
    }

    /// Chooses the section for a value, and whether the number must be negated.
    ///
    /// Excel's rules: with one section it applies to everything and negatives
    /// keep their sign; with two, the second covers negatives and the minus is
    /// dropped because the section is expected to supply its own; with three or
    /// four the third covers zero and the fourth covers text.
    fn pick(&self, value: Value<'_>) -> (&'a str, bool) {
        let n = match value {
            Value::Text(_) => {
                // The fourth section is the text section. With a single section
                // that one section applies to everything, text included. With
                // two or three, text passes through unformatted.
                return match self.parts.len() {
                    1 => (self.parts[0], false),
                    n if n >= 4 => (self.parts[3], false),
                    _ => ("@", false),
                };
            }
            Value::Number(n) => n,
        };
        match self.parts.len() {
            0 | 1 => (self.parts.first().copied().unwrap_or(GENERAL), false),
            2 if n < 0.0 => (self.parts[1], true),
            2 => (self.parts[0], false),
            _ if n > 0.0 => (self.parts[0], false),
            _ if n < 0.0 => (self.parts[1], true),
            _ => (self.parts[2], false),
        }
    }
}

/// Applies a text section: `@` stands for the value, everything else is literal.
fn render_text(section: &str, text: &str) -> String {
    if section.trim().is_empty() || section.trim() == GENERAL {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut chars = section.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '@' => out.push_str(text),
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                    out.push(q);
                }
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            // `_x` reserves the width of x and `*x` pads with it; in plain
            // text both come out as a single space.
            '_' | '*' => {
                chars.next();
                out.push(' ');
            }
            '[' => {
                let inner: String = chars.by_ref().take_while(|b| *b != ']').collect();
                out.push_str(currency(&inner));
            }
            c => out.push(c),
        }
    }
    out
}

/// The symbol a `[$...]` tag shows: `[$₽-419]` is a rouble sign under the
/// Russian locale, `[$-419]` the locale alone. Colours and conditions show
/// nothing.
fn currency(inner: &str) -> &str {
    inner
        .strip_prefix('$')
        .map_or("", |rest| rest.split('-').next().unwrap_or_default())
}

/// The language a section's locale tag names, for month and weekday names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    English,
    Russian,
}

impl Language {
    /// Reads `[$-419]`, `[$₽-419]` or `[$-ru-RU]` wherever it stands.
    ///
    /// ponytail: English and Russian only; another language's names come
    /// out in English until a table for it is added.
    fn of(section: &str) -> Self {
        let mut rest = section;
        while let Some(open) = rest.find("[$") {
            let tail = &rest[open + 2..];
            let Some(close) = tail.find(']') else {
                break;
            };
            let tag = tail[..close].split_once('-').map_or("", |(_, t)| t);
            // The low bits of the id are the language: 0x19 is Russian,
            // whatever the country and calendar bits above it say.
            let russian = u32::from_str_radix(tag, 16).map_or_else(
                |_| tag.to_ascii_lowercase().starts_with("ru"),
                |id| id & 0x3FF == 0x19,
            );
            if russian {
                return Self::Russian;
            }
            rest = &tail[close..];
        }
        Self::English
    }
}

/// What a section asks for.
#[derive(Debug, Default)]
struct Spec {
    /// Digits required after the decimal point (`0`), and optional ones (`#`).
    decimals: usize,
    /// Whether the integer part is grouped in thousands.
    thousands: bool,
    /// Minimum digits before the decimal point.
    integer_digits: usize,
    /// Scaling: each trailing comma divides by a thousand.
    scale: f64,
    /// Percent signs multiply by a hundred, once each.
    percent: u32,
    /// Scientific notation, with the digits after `E`.
    exponent: Option<usize>,
    /// Whether the section holds a real digit placeholder (`0`, `#` or `?`).
    ///
    /// Without one there is no number to shape, and a `.` or `,` in the
    /// section is a character like any other.
    placeholders: bool,
}

/// Renders a number through one section.
fn render_number(section: &str, value: f64, epoch: Epoch) -> String {
    if section.trim() == GENERAL || section.trim().is_empty() {
        return general(value);
    }
    if is_date_format(section) {
        return render_datetime(section, value, epoch);
    }

    if let Some(text) = render_fraction(section, value) {
        return text;
    }
    let spec = scan(section);
    let mut n = value;
    for _ in 0..spec.percent {
        n *= 100.0;
    }
    n /= spec.scale;

    let digits = if let Some(exp_digits) = spec.exponent {
        return render_scientific(section, n, &spec, exp_digits);
    } else {
        format_fixed(n.abs(), spec.decimals, spec.integer_digits, spec.thousands)
    };

    let mut out = String::new();
    let negative = n < 0.0 && !digits.chars().all(|c| c == '0' || c == '.' || c == ',');
    let mut placed = false;
    // Whether the previous character belonged to the run of placeholders that
    // the number was rendered from.
    let mut in_run = false;
    let mut chars = section.chars().peekable();
    while let Some(c) = chars.next() {
        let run_char = matches!(c, '0' | '#' | '?' | '.' | ',');
        match c {
            '0' | '#' | '?' | '.' | ',' => {
                if !placed {
                    if negative {
                        out.push('-');
                    }
                    out.push_str(&digits);
                    placed = true;
                    // With no placeholder anywhere the separator shapes
                    // nothing, so it survives beside the number.
                    if !spec.placeholders {
                        out.push(c);
                    }
                } else if !in_run || !spec.placeholders {
                    // A separator standing on its own, after the number is
                    // already written, is just a character: `дд.мм.гггг` typed
                    // in a language Excel does not know keeps all of its dots.
                    out.push(c);
                }
                // Otherwise it is the rest of the run `digits` came from.
            }
            '%' => out.push('%'),
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                    out.push(q);
                }
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '_' | '*' => {
                chars.next();
                out.push(' ');
            }
            '[' => {
                let inner: String = chars.by_ref().take_while(|b| *b != ']').collect();
                out.push_str(currency(&inner));
            }
            // `@` in a section applied to a number renders the number as it
            // would appear under General.
            '@' => {
                if !placed {
                    out.push_str(&general(value));
                    placed = true;
                }
            }
            c => out.push(c),
        }
        in_run = run_char;
    }
    out
}

/// Renders a fraction format: `# ?/?`, `# ??/100`, `?/???`.
///
/// `None` when the section has no fraction in it. With an integer part the
/// whole number goes there and the rest into the fraction; without one the
/// fraction holds it all. A `?` pads with a space, `0` with a zero and `#`
/// with nothing: the numerator is aligned right, the denominator left, so
/// fractions in a column line up on the slash. A fraction that comes out
/// zero is blanked to spaces as wide as it would have been.
fn render_fraction(section: &str, value: f64) -> Option<String> {
    let chars: Vec<char> = section.chars().collect();
    let code = code_mask(&chars);
    let is = |i: usize, set: &str| code[i] && set.contains(chars[i]);
    let slash = (0..chars.len()).find(|&i| is(i, "/"))?;
    let mut num_start = slash;
    while num_start > 0 && is(num_start - 1, "0#?") {
        num_start -= 1;
    }
    let mut den_end = slash + 1;
    while den_end < chars.len() && is(den_end, "0123456789#?") {
        den_end += 1;
    }
    if num_start == slash || den_end == slash + 1 {
        return None;
    }
    let numerator = &chars[num_start..slash];
    let denominator = &chars[slash + 1..den_end];
    // An integer part is the nearest run of placeholders before, past the
    // literal text between them.
    let mut int_end = num_start;
    while int_end > 0 && !is(int_end - 1, "0#?") {
        int_end -= 1;
    }
    let mut int_start = int_end;
    while int_start > 0 && is(int_start - 1, "0#?,") {
        int_start -= 1;
    }
    let integer = (int_start < int_end).then(|| &chars[int_start..int_end]);

    let abs = value.abs();
    let (mut whole, frac) = if integer.is_some() {
        (abs.floor(), abs - abs.floor())
    } else {
        (0.0, abs)
    };
    let fixed: Option<f64> = denominator
        .iter()
        .collect::<String>()
        .parse::<u32>()
        .ok()
        .filter(|d| *d > 0)
        .map(f64::from);
    let (mut n, d) = fixed.map_or_else(
        || closest_fraction(frac, denominator.len()),
        |d| ((frac * d).round(), d),
    );
    if integer.is_some() && n >= d {
        whole += 1.0;
        n = 0.0;
    }
    let blank = integer.is_some() && n == 0.0;
    let whole_text = if whole == 0.0 && !blank {
        String::new()
    } else {
        format!("{whole:.0}")
    };

    let mut out = String::new();
    let negative = value < 0.0 && (whole > 0.0 || n > 0.0);
    let mut signed = false;
    let mut i = 0;
    while i < chars.len() {
        if integer.is_some() && i == int_start {
            if negative {
                out.push('-');
                signed = true;
            }
            let run: Vec<char> = chars[int_start..int_end]
                .iter()
                .copied()
                .filter(|c| *c != ',')
                .collect();
            out.push_str(&fill(&whole_text, &run, true));
            i = int_end;
        } else if i == num_start {
            if negative && !signed {
                out.push('-');
            }
            if blank {
                out.push_str(&" ".repeat(den_end - num_start));
                i = den_end;
            } else {
                out.push_str(&fill(&format!("{n:.0}"), numerator, true));
                out.push('/');
                let den = format!("{d:.0}");
                if fixed.is_some() {
                    out.push_str(&den);
                } else {
                    out.push_str(&fill(&den, denominator, false));
                }
                i = den_end;
            }
        } else {
            i = push_literal(&mut out, &chars, i);
        }
    }
    Some(out)
}

/// The fraction closest to `frac` whose denominator has at most `digits`
/// digits: numerator and denominator.
fn closest_fraction(frac: f64, digits: usize) -> (f64, f64) {
    let digits = u32::try_from(digits.min(5)).unwrap_or(5);
    let most = 10_u32.pow(digits) - 1;
    let mut best = (frac.round(), 1.0, (frac - frac.round()).abs());
    for d in 2..=most {
        let d = f64::from(d);
        let n = (frac * d).round();
        let error = (frac - n / d).abs();
        if error < best.2 {
            best = (n, d, error);
        }
    }
    (best.0, best.1)
}

/// Which characters of a section are format codes rather than quoted,
/// escaped or inside a `[...]` tag.
fn code_mask(chars: &[char]) -> Vec<bool> {
    let mut code = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
            }
            '\\' | '_' | '*' => i += 1,
            '[' => {
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
            }
            _ => code[i] = true,
        }
        i += 1;
    }
    code
}

/// Writes the literal at `i` - quoted text, an escaped character, padding or
/// a tag - and answers where the next character is.
fn push_literal(out: &mut String, chars: &[char], mut i: usize) -> usize {
    match chars[i] {
        '"' => {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                out.push(chars[i]);
                i += 1;
            }
        }
        '\\' => {
            if let Some(c) = chars.get(i + 1) {
                out.push(*c);
            }
            i += 1;
        }
        '_' | '*' => {
            out.push(' ');
            i += 1;
        }
        '[' => {
            let start = i + 1;
            while i < chars.len() && chars[i] != ']' {
                i += 1;
            }
            let inner: String = chars[start..i.min(chars.len())].iter().collect();
            out.push_str(currency(&inner));
        }
        c => out.push(c),
    }
    i + 1
}

/// Digits placed into a run of placeholders: the ones the digits do not
/// cover pad with a space (`?`), a zero (`0`) or nothing (`#`), on the left
/// when the digits align right and on the right otherwise.
fn fill(digits: &str, run: &[char], right: bool) -> String {
    let missing = run.len().saturating_sub(digits.len());
    let pads: &[char] = if right {
        &run[..missing]
    } else {
        &run[run.len() - missing..]
    };
    let pad: String = pads
        .iter()
        .filter_map(|c| match c {
            '?' => Some(' '),
            '0' => Some('0'),
            _ => None,
        })
        .collect();
    if right {
        pad + digits
    } else {
        format!("{digits}{pad}")
    }
}

/// Reads what a numeric section asks for.
fn scan(section: &str) -> Spec {
    let mut spec = Spec {
        scale: 1.0,
        ..Spec::default()
    };
    let mut seen_point = false;
    let mut in_digits = false;
    let mut chars = section.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            '[' => {
                for b in chars.by_ref() {
                    if b == ']' {
                        break;
                    }
                }
            }
            '0' | '#' | '?' => {
                in_digits = true;
                spec.placeholders = true;
                if seen_point {
                    spec.decimals += 1;
                } else if c == '0' {
                    spec.integer_digits += 1;
                }
            }
            '.' => {
                seen_point = true;
                in_digits = true;
            }
            ',' if in_digits => {
                // A comma between placeholders groups thousands; one that
                // trails the number scales it down instead.
                if chars.peek().is_some_and(|n| "0#?.".contains(*n)) {
                    spec.thousands = true;
                } else {
                    spec.scale *= 1000.0;
                }
            }
            '%' => spec.percent += 1,
            'E' | 'e' if chars.peek().is_some_and(|n| *n == '+' || *n == '-') => {
                chars.next();
                let mut count = 0;
                while chars.peek().is_some_and(|n| *n == '0' || *n == '#') {
                    chars.next();
                    count += 1;
                }
                spec.exponent = Some(count.max(1));
            }
            _ => {}
        }
    }
    spec
}

/// Rounds half away from zero, the way Excel does.
///
/// Rust's formatting rounds half to even, so `{:.0}` renders 0.5 as "0" and
/// 0.125 at two places as "0.12". Excel gives "1" and "0.13"; a spreadsheet
/// that disagrees with itself about 0.5 is not much use.
fn round_half_up(value: f64, decimals: usize) -> f64 {
    let factor = 10f64.powi(i32::try_from(decimals).unwrap_or(0));
    let scaled = value * factor;
    // Nudge by one ulp before rounding: 1.005 * 100 lands just below 100.5 in
    // binary, and Excel still rounds it up.
    let nudged = if scaled >= 0.0 {
        scaled + f64::EPSILON * scaled.abs().max(1.0)
    } else {
        scaled - f64::EPSILON * scaled.abs().max(1.0)
    };
    if nudged >= 0.0 {
        (nudged + 0.5).floor() / factor
    } else {
        (nudged - 0.5).ceil() / factor
    }
}

/// Renders a number with a fixed number of decimals, optionally grouped.
fn format_fixed(value: f64, decimals: usize, min_integer: usize, thousands: bool) -> String {
    let rendered = format!("{:.decimals$}", round_half_up(value, decimals));
    let (int_part, frac) = rendered.split_once('.').unwrap_or((rendered.as_str(), ""));

    let mut int_part = int_part.to_owned();
    while int_part.len() < min_integer {
        int_part.insert(0, '0');
    }
    // `#` with no `0` before the point drops a lone leading zero: `#.##` renders
    // 0.5 as ".5".
    if min_integer == 0 && int_part == "0" {
        int_part.clear();
    }

    let mut out = if thousands {
        group(&int_part)
    } else {
        int_part
    };
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// Inserts thousands separators.
fn group(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Renders scientific notation, as `0.00E+00` asks for.
fn render_scientific(section: &str, value: f64, spec: &Spec, exp_digits: usize) -> String {
    if value == 0.0 {
        let mantissa = format_fixed(0.0, spec.decimals, spec.integer_digits.max(1), false);
        return format!("{mantissa}E+{}", "0".repeat(exp_digits));
    }
    // `##0.0E+0` is engineering notation: three or more integer placeholders in
    // the mantissa mean the exponent moves in steps of three. Only the digits
    // before the decimal point count.
    let step = section
        .split(['E', 'e'])
        .next()
        .and_then(|m| m.split('.').next())
        .map_or(1, |m| {
            m.chars().filter(|c| "0#?".contains(*c)).count().max(1)
        });
    let step = if step >= 3 { 3 } else { 1 };

    #[expect(
        clippy::cast_possible_truncation,
        reason = "log10 of a finite f64 fits in i32"
    )]
    let mut exp = value.abs().log10().floor() as i32;
    exp -= exp.rem_euclid(step);
    let mantissa = value / 10f64.powi(exp);

    let sign = if exp < 0 { '-' } else { '+' };
    let minus = if value < 0.0 { "-" } else { "" };
    format!(
        "{minus}{}E{sign}{:0width$}",
        format_fixed(mantissa.abs(), spec.decimals, 1, false),
        exp.abs(),
        width = exp_digits
    )
}

/// Excel's `General` format.
///
/// Excel shows at most eleven significant digits and falls back to scientific
/// notation when a number does not fit. It does *not* print the full binary
/// expansion of a double: 1234.5678 shows as `1234.5678`, not as
/// `1234.56780000000003`.
const GENERAL_SIGNIFICANT_DIGITS: usize = 11;

fn general(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    if !value.is_finite() {
        return value.to_string();
    }
    let abs = value.abs();
    if !(1e-4..1e11).contains(&abs) {
        // `{:E}` gives one digit before the point, which is what Excel shows.
        let s = format!("{value:E}");
        let (mantissa, exp) = s.split_once('E').unwrap_or((s.as_str(), "0"));
        let mantissa = trim_trailing_zeros(mantissa);
        let exp: i32 = exp.parse().unwrap_or(0);
        return format!(
            "{mantissa}E{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        );
    }
    // Round to eleven significant digits, then drop the zeros that adds.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "magnitude is within 1e-4..1e11"
    )]
    let magnitude = abs.log10().floor() as i32;
    let significant = i32::try_from(GENERAL_SIGNIFICANT_DIGITS).unwrap_or(11);
    let decimals = usize::try_from((significant - 1 - magnitude).max(0)).unwrap_or(0);
    let rendered = format!("{value:.decimals$}");
    trim_trailing_zeros(&rendered)
}

/// Drops trailing zeros of a decimal fraction, and a bare trailing point.
fn trim_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_owned();
    }
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Whether a section is a date or time format rather than a numeric one.
#[must_use]
pub fn is_date_format(section: &str) -> bool {
    // `General` is not a format of codes at all, and its `e` is not the era
    // code: without this every plain number counted as a date, and the
    // `OpenDocument` writer stated `10` as the tenth of January 1900.
    if section.trim().eq_ignore_ascii_case(GENERAL) || section.trim().is_empty() {
        return false;
    }
    let mut chars = section.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            '[' => {
                // `[h]`, `[m]` and `[s]` are elapsed-time codes, which do make a
                // format a time format; `[Red]` and `[$-409]` do not.
                let inner: String = chars.by_ref().take_while(|b| *b != ']').collect();
                if matches!(
                    inner.to_ascii_lowercase().as_str(),
                    "h" | "hh" | "m" | "mm" | "s" | "ss"
                ) {
                    return true;
                }
            }
            // `E+` / `E-` is scientific notation; a bare `e` is the Japanese
            // era code, which does make this a date format.
            'e' | 'E' => {
                if !chars.peek().is_some_and(|n| *n == '+' || *n == '-') {
                    return true;
                }
                chars.next();
            }
            'y' | 'd' | 'h' | 's' | 'Y' | 'D' | 'H' | 'S' | 'm' | 'M' => return true,
            _ => {}
        }
    }
    false
}

/// Renders a serial number through a date or time format.
fn render_datetime(section: &str, serial: f64, epoch: Epoch) -> String {
    let Ok(dt) = from_serial(serial, epoch) else {
        return general(serial);
    };
    // Seconds are rounded, not truncated, unless the format shows a fraction:
    // 13:37:37.92 displays as 13:37:38.
    let (dt, whole_seconds) = rounded_seconds(dt, section.contains(".0"));

    // Whether an `h` came before decides if `m` means month or minute.
    let mut out = String::new();
    let lower = section.to_ascii_lowercase();
    let bytes: Vec<char> = section.chars().collect();
    let lower: Vec<char> = lower.chars().collect();
    let has_ampm =
        lower.windows(2).any(|w| w == ['a', 'm']) && lower.windows(2).any(|w| w == ['p', 'm']);

    let language = Language::of(section);
    let mut i = 0;
    let mut after_hour = false;
    while i < bytes.len() {
        let c = lower[i];
        match c {
            '"' | '\\' | '_' | '*' | '[' => {
                i += write_non_code(&mut out, &bytes, &lower, i, serial);
            }
            'a' if lower[i..].starts_with(&['a', 'm', '/', 'p', 'm']) => {
                out.push_str(if dt.hour < 12 { "AM" } else { "PM" });
                i += 5;
            }
            'y' => {
                let n = run_length(&lower, i, 'y');
                if n <= 2 {
                    let _ = write!(out, "{:02}", dt.year % 100);
                } else {
                    let _ = write!(out, "{:04}", dt.year);
                }
                i += n;
            }
            'm' => {
                let n = run_length(&lower, i, 'm');
                // `m` right after an hour code, or right before seconds, is
                // minutes; otherwise it is a month.
                let is_minute = after_hour || next_code(&lower, i + n) == Some('s');
                if is_minute {
                    let _ = write!(out, "{:0width$}", dt.minute, width = n.min(2));
                } else {
                    out.push_str(&month(dt.month, n, language));
                }
                i += n;
                after_hour = false;
            }
            'd' => {
                let n = run_length(&lower, i, 'd');
                out.push_str(&day(dt, n, epoch, language));
                i += n;
            }
            'h' => {
                let n = run_length(&lower, i, 'h');
                let hour = if has_ampm {
                    match dt.hour % 12 {
                        0 => 12,
                        h => h,
                    }
                } else {
                    dt.hour
                };
                let _ = write!(out, "{hour:0width$}", width = n.min(2));
                i += n;
                after_hour = true;
            }
            's' => {
                let n = run_length(&lower, i, 's');
                let _ = write!(out, "{whole_seconds:0width$}", width = n.min(2));
                i += n;
                i += write_second_fraction(
                    &mut out,
                    &lower,
                    i,
                    dt.second - f64::from(whole_seconds),
                );
                after_hour = false;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    out
}

/// Everything in a date section that is not a date code: quoted text, an
/// escaped character, a padding or repeat code, and a `[...]` tag. Answers how
/// many characters of the format it consumed.
fn write_non_code(
    out: &mut String,
    bytes: &[char],
    lower: &[char],
    start: usize,
    serial: f64,
) -> usize {
    match bytes[start] {
        '"' => {
            let mut i = start + 1;
            while i < bytes.len() && bytes[i] != '"' {
                out.push(bytes[i]);
                i += 1;
            }
            (i + 1 - start).min(bytes.len() - start)
        }
        '\\' => {
            if let Some(&c) = bytes.get(start + 1) {
                out.push(c);
                2
            } else {
                1
            }
        }
        // `_x` reserves the width of `x`, `*x` repeats it to fill; neither has
        // a width to work with here, so both just swallow their argument.
        '_' | '*' => 2,
        _ => {
            let inner_start = start + 1;
            let mut end = inner_start;
            // Find the closing bracket.
            while end < bytes.len() && bytes[end] != ']' {
                end += 1;
            }
            let inner: String = lower[inner_start..end.min(lower.len())].iter().collect();
            let written: String = bytes[inner_start..end.min(bytes.len())].iter().collect();
            out.push_str(currency(&written));
            write_elapsed(out, &inner, serial);
            end + 1 - start
        }
    }
}

/// A fraction of a second, written as `.0` after the seconds code. Answers how
/// many characters of the format it consumed, zero where no fraction follows.
fn write_second_fraction(out: &mut String, lower: &[char], i: usize, frac: f64) -> usize {
    if lower.get(i) != Some(&'.') || lower.get(i + 1) != Some(&'0') {
        return 0; // No fraction here; move on.
    }
    let n = run_length(lower, i + 1, '0');
    let r = format!("{frac:.n$}");
    // `trim_start_matches`, not `[1..]`: the format may ask for zero digits.
    out.push_str(r.trim_start_matches('0'));
    1 + n
}

/// Rounds the seconds of a time to whole ones, unless the format shows a
/// fraction of a second. Rounding 59.6 up carries into the minute, and possibly
/// into the hour.
fn rounded_seconds(
    dt: crate::shared::date::DateTime,
    shows_fraction: bool,
) -> (crate::shared::date::DateTime, u32) {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "second is in 0.0..60.0"
    )]
    let s = if shows_fraction {
        dt.second as u32
    } else {
        dt.second.round() as u32
    };
    if s < 60 {
        return (dt, s);
    }
    let mut d = dt;
    d.second = 0.0;
    d.minute += 1;
    if d.minute >= 60 {
        d.minute = 0;
        d.hour = (d.hour + 1) % 24;
    }
    // The day is not carried: it has been worked out already, and a format
    // without `d` would not show it anyway.
    (d, 0)
}

/// The contents of a `[...]` code. `[h]`, `[m]` and `[s]` are elapsed time,
/// which runs past the end of a day rather than wrapping; anything else in
/// brackets is a colour or a locale tag and renders nothing.
fn write_elapsed(out: &mut String, inner: &str, serial: f64) {
    // Hours and minutes truncate, seconds round.
    let v = match inner {
        "h" | "hh" => (serial * 24.0).floor(),
        "m" | "mm" => (serial * 1440.0).floor(),
        "s" | "ss" => (serial * 86_400.0).round(),
        _ => return,
    };
    let _ = write_padded(out, v, inner.len());
}

/// Writes a whole number padded to at least `width` digits.
fn write_padded(out: &mut String, value: f64, width: usize) -> std::fmt::Result {
    use std::fmt::Write as _;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "elapsed values come from a bounded serial"
    )]
    let v = value.max(0.0) as u64;
    write!(out, "{v:0width$}")
}

/// How many times a character repeats from `start`.
fn run_length(chars: &[char], start: usize, c: char) -> usize {
    chars[start..].iter().take_while(|x| **x == c).count()
}

/// The next date code after `from`, skipping separators.
fn next_code(chars: &[char], from: usize) -> Option<char> {
    chars[from..]
        .iter()
        .find(|c| "ymdhs".contains(**c))
        .copied()
}

/// Month names for the `m` codes.
fn month(month: u32, width: usize, language: Language) -> String {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    // The names excelize gives the Russian locale; no Excel was at hand to
    // check them against.
    const RUSSIAN: [&str; 12] = [
        "январь",
        "февраль",
        "март",
        "апрель",
        "май",
        "июнь",
        "июль",
        "август",
        "сентябрь",
        "октябрь",
        "ноябрь",
        "декабрь",
    ];
    const RUSSIAN_SHORT: [&str; 12] = [
        "янв.", "фев.", "март", "апр.", "май", "июнь", "июль", "авг.", "сен.", "окт.", "ноя.",
        "дек.",
    ];
    let index = (month.max(1) - 1) as usize % 12;
    let (name, short) = match language {
        Language::English => (NAMES[index], &NAMES[index][..3]),
        Language::Russian => (RUSSIAN[index], RUSSIAN_SHORT[index]),
    };
    match width {
        1 => month.to_string(),
        2 => format!("{month:02}"),
        3 => short.to_owned(),
        4 => name.to_owned(),
        // `mmmmm` is the single-letter form.
        _ => name.chars().take(1).collect(),
    }
}

/// Day of month, or weekday name for `ddd` and `dddd`.
fn day(
    dt: crate::shared::date::DateTime,
    width: usize,
    epoch: Epoch,
    language: Language,
) -> String {
    const NAMES: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    const RUSSIAN: [&str; 7] = [
        "воскресенье",
        "понедельник",
        "вторник",
        "среда",
        "четверг",
        "пятница",
        "суббота",
    ];
    const RUSSIAN_SHORT: [&str; 7] = ["Вс", "Пн", "Вт", "Ср", "Чт", "Пт", "Сб"];
    match width {
        1 => dt.day.to_string(),
        2 => format!("{:02}", dt.day),
        _ => {
            // Serial 1 is 1 January 1900, a Sunday, so the weekday index is
            // (serial - 1) mod 7. In the 1904 epoch serial 1 is 2 January 1904,
            // a Saturday.
            let serial = crate::shared::date::to_serial(dt, epoch).unwrap_or(0.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "serial is bounded and non-negative"
            )]
            let index = match epoch {
                Epoch::Windows1900 => (serial as u64 + 6) % 7,
                Epoch::Mac1904 => (serial as u64 + 5) % 7,
            };
            let index = usize::try_from(index).unwrap_or(0);
            match (language, width) {
                (Language::English, 3) => NAMES[index][..3].to_owned(),
                (Language::English, _) => NAMES[index].to_owned(),
                (Language::Russian, 3) => RUSSIAN_SHORT[index].to_owned(),
                (Language::Russian, _) => RUSSIAN[index].to_owned(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, builtin_code, format, general, round_half_up};
    use crate::shared::date::Epoch;

    fn render(value: f64, code: &str) -> String {
        format(Value::Number(value), code, Epoch::Windows1900)
    }

    /// Compares two floats within a tolerance finer than any value under test.
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn rounds_half_away_from_zero() {
        // Rust rounds half to even, Excel away from zero.
        assert!(close(round_half_up(0.5, 0), 1.0));
        assert!(close(round_half_up(1.5, 0), 2.0));
        assert!(close(round_half_up(2.5, 0), 3.0));
        assert!(close(round_half_up(-0.5, 0), -1.0));
        assert!(close(round_half_up(0.125, 2), 0.13));
        assert_eq!(render(0.5, "0"), "1");
        assert_eq!(render(2.5, "0"), "3");
        assert_eq!(render(-2.5, "0"), "-3");
    }

    #[test]
    fn sections_are_chosen_by_sign() {
        // Two sections: the second covers negatives and supplies its own sign.
        assert_eq!(render(5.0, "0;(0)"), "5");
        assert_eq!(render(-5.0, "0;(0)"), "(5)");
        // Three: the third covers zero.
        assert_eq!(render(0.0, r#"0;-0;"zero""#), "zero");
        assert_eq!(render(-5.0, r#"0;-0;"zero""#), "-5");
        // A single section covers everything, negatives included.
        assert_eq!(render(-5.0, "0"), "-5");
        // An empty section renders nothing for that sign.
        assert_eq!(render(-5.0, "0;"), "");
    }

    #[test]
    fn separators_inside_literals_do_not_split_sections() {
        assert_eq!(render(1.0, r#""a;b"0"#), "a;b1");
        assert_eq!(render(1.0, r"\;0"), ";1");
    }

    #[test]
    fn text_uses_the_fourth_section_or_a_lone_one() {
        let text = |t: &str, code: &str| format(Value::Text(t), code, Epoch::Windows1900);
        assert_eq!(text("hi", "@"), "hi");
        assert_eq!(text("hi", r#""pre"@"post""#), "prehipost");
        // With two or three sections text passes through untouched.
        assert_eq!(text("hi", "0;-0"), "hi");
        assert_eq!(text("hi", r#"0;-0;"zero""#), "hi");
        // The fourth section is the text one.
        assert_eq!(text("hi", r#"0;-0;"zero";"[" @ "]""#), "[ hi ]");
    }

    #[test]
    fn thousands_and_scaling() {
        assert_eq!(render(1_234_567.0, "#,##0"), "1,234,567");
        assert_eq!(render(1234.5, "#,##0.00"), "1,234.50");
        // A trailing comma scales the number down by a thousand.
        assert_eq!(render(1_234_567.0, "0,"), "1235");
        assert_eq!(render(1_234_567.0, "0,,"), "1");
    }

    #[test]
    fn percent_multiplies() {
        assert_eq!(render(0.125, "0%"), "13%");
        assert_eq!(render(0.125, "0.0%"), "12.5%");
    }

    #[test]
    fn general_matches_excel_not_the_double() {
        assert_eq!(general(0.0), "0");
        assert_eq!(general(1234.5678), "1234.5678");
        assert_eq!(general(-1234.5678), "-1234.5678");
        assert_eq!(general(1e-7), "1E-07");
        assert_eq!(general(1e12), "1E+12");
        assert_eq!(general(0.1), "0.1");
    }

    #[test]
    fn general_is_not_a_date_format() {
        use super::is_date_format;
        assert!(!is_date_format("General"));
        assert!(!is_date_format("general"));
        assert!(!is_date_format(""));
        assert!(!is_date_format("0.00"));
        // The era code still counts, and so do the ordinary date codes.
        assert!(is_date_format("ge"));
        assert!(is_date_format("yyyy-mm-dd"));
        assert!(!is_date_format("0.00E+00"));
    }

    #[test]
    fn dates_and_times() {
        assert_eq!(render(45_658.0, "yyyy-mm-dd"), "2025-01-01");
        assert_eq!(render(45_658.0, "d-mmm-yy"), "1-Jan-25");
        assert_eq!(render(45_658.0, "dddd"), "Wednesday");
        assert_eq!(render(45_658.5, "h:mm"), "12:00");
        assert_eq!(render(45_658.5, "h:mm AM/PM"), "12:00 PM");
        assert_eq!(render(45_658.25, "h:mm AM/PM"), "6:00 AM");
        // Midnight is 12 AM, not 0 AM.
        assert_eq!(render(45_658.0, "h:mm AM/PM"), "12:00 AM");
        // `m` means minutes next to an hour and months otherwise.
        assert_eq!(
            render(45_658.5, "yyyy-mm-dd h:mm:ss"),
            "2025-01-01 12:00:00"
        );
        // Elapsed time keeps counting past 24 hours.
        assert_eq!(render(1.5, "[h]:mm:ss"), "36:00:00");
    }

    #[test]
    fn seconds_are_rounded_and_carry() {
        // 13:37:37.92 shows as ...38.
        assert_eq!(
            render(1234.5678, "yyyy-mm-dd h:mm:ss"),
            "1903-05-18 13:37:38"
        );
        // Rounding 59.6 up carries into the next minute.
        let almost = 45_658.0 + (11.0 * 3600.0 + 59.0 * 60.0 + 59.6) / 86_400.0;
        assert_eq!(render(almost, "h:mm:ss"), "12:00:00");
    }

    #[test]
    fn builtin_codes_cover_the_known_ids() {
        assert_eq!(builtin_code(0), "General");
        assert_eq!(builtin_code(4), "#,##0.00");
        assert_eq!(builtin_code(14), "m/d/yyyy");
        assert_eq!(builtin_code(49), "@");
        // Locale-dependent ids are left as General, as does.
        assert_eq!(builtin_code(5), "General");
        assert_eq!(builtin_code(999), "General");
    }

    #[test]
    fn fractions_line_up_on_the_slash() {
        assert_eq!(render(1.25, "# ?/?"), "1 1/4");
        assert_eq!(render(0.5, "# ?/?"), " 1/2");
        assert_eq!(render(-1.75, "# ?/?"), "-1 3/4");
        // A whole number blanks the fraction, and so does one that rounds up.
        assert_eq!(render(2.0, "# ?/?"), "2    ");
        assert_eq!(render(0.99, "# ?/?"), "1    ");
        assert_eq!(render(0.0, "# ?/?"), "0    ");
        assert_eq!(render(5.25, "# ???/???"), "5   1/4  ");
        // The closest fraction with that many digits, not the first near one.
        assert_eq!(render(10.0 / 7.0, "?/???"), "10/7  ");
        assert_eq!(render(5.2381, "# ?/???"), "5 5/21 ");
        assert_eq!(render(1.3, "# ?/8"), "1 2/8");
        assert_eq!(render(0.37, "0 ??/100"), "0 37/100");
    }

    #[test]
    fn locale_tags_show_their_symbol_and_language() {
        assert_eq!(render(1234.5, "#,##0.00 [$₽-419]"), "1,234.50 ₽");
        assert_eq!(render(5.0, "[$€-407] 0.00"), "€ 5.00");
        assert_eq!(render(45_658.0, "[$-409]mmmm"), "January");
        // Checked against excelize: 1 January 2025 was a Wednesday.
        assert_eq!(render(45_658.0, "[$-419]d mmmm yyyy"), "1 январь 2025");
        assert_eq!(render(45_658.0, "[$-419]mmm dddd ddd"), "янв. среда Ср");
        assert_eq!(render(45_658.0, "[$-ru-RU]mmmmm"), "я");
    }

    #[test]
    fn a_separator_outside_the_number_stays_a_character() {
        // A format typed in a language Excel does not know is mostly literal.
        // The first run of placeholders - here a lone `.` - is where the
        // number goes; the dots after it are just dots.
        assert_eq!(
            format(Value::Number(693_597.0), "ДД.ММ.ГГГГ", Epoch::Windows1900),
            "ДД693597.ММ.ГГГГ"
        );
        // Inside one run they still belong to the number.
        assert_eq!(
            format(Value::Number(1234.5), "#,##0.00", Epoch::Windows1900),
            "1,234.50"
        );
        assert_eq!(
            format(Value::Number(2.0), "0 шт.", Epoch::Windows1900),
            "2 шт."
        );
    }
}
