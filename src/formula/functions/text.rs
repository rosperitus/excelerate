//! Text.
//!
//! Excel counts characters, not bytes, so every position and length here is in
//! characters: `LEN("Ёж")` is 2.

use super::{Arg, first_error};
use crate::error::CellError;
use crate::formula::eval::{Engine, Origin};
use crate::formula::parser::Expr;
use crate::formula::value::Value;
use crate::style::format;

/// `CONCATENATE(text1, ...)`, and `CONCAT`, which also flattens ranges.
pub fn concatenate(args: &[Arg]) -> Value {
    let mut out = String::new();
    for arg in args {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for v in flat {
            match v.text() {
                Ok(t) => out.push_str(&t),
                Err(e) => return Value::Error(e),
            }
        }
    }
    Value::Text(out)
}

/// `LEN(text)`
pub fn len(args: &[Arg]) -> Value {
    one_text(args, |t| count_value(t.chars().count()))
}

/// `UPPER(text)`
pub fn upper(args: &[Arg]) -> Value {
    one_text(args, |t| Value::Text(t.to_uppercase()))
}

/// `LOWER(text)`
pub fn lower(args: &[Arg]) -> Value {
    one_text(args, |t| Value::Text(t.to_lowercase()))
}

/// `TRIM(text)` - drops leading and trailing spaces and squeezes the runs in
/// between down to one.
pub fn trim(args: &[Arg]) -> Value {
    one_text(args, |t| {
        Value::Text(t.split_whitespace().collect::<Vec<_>>().join(" "))
    })
}

/// `LEFT(text, [count])`
pub fn left(args: &[Arg]) -> Value {
    cut(args, |chars, n| chars.iter().take(n).collect())
}

/// `RIGHT(text, [count])`
pub fn right(args: &[Arg]) -> Value {
    cut(args, |chars, n| {
        chars.iter().skip(chars.len().saturating_sub(n)).collect()
    })
}

/// Shared body of `LEFT` and `RIGHT`.
fn cut(args: &[Arg], take: fn(&[char], usize) -> String) -> Value {
    let (text, count) = match args {
        [t] => (t.text(), Ok(1.0)),
        [t, n] => (t.text(), n.number()),
        _ => return Value::Error(CellError::Value),
    };
    match (text, count) {
        (Ok(t), Ok(n)) if n >= 0.0 => {
            let chars: Vec<char> = t.chars().collect();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked non-negative, and a longer count just takes everything"
            )]
            let n = (n as usize).min(chars.len());
            Value::Text(take(&chars, n))
        }
        (Ok(_), Ok(_)) => Value::Error(CellError::Value),
        (Err(e), _) | (_, Err(e)) => Value::Error(e),
    }
}

/// `MID(text, start, count)` - `start` counts from 1.
pub fn mid(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [t, s, n] = args else {
        return Value::Error(CellError::Value);
    };
    match (t.text(), s.number(), n.number()) {
        (Ok(t), Ok(start), Ok(count)) if start >= 1.0 && count >= 0.0 => {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "both bounds are checked just above"
            )]
            let (start, count) = (start as usize - 1, count as usize);
            Value::Text(t.chars().skip(start).take(count).collect())
        }
        (Ok(_), Ok(_), Ok(_)) => Value::Error(CellError::Value),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => Value::Error(e),
    }
}

/// `REPT(text, count)`
pub fn rept(args: &[Arg]) -> Value {
    let [t, n] = args else {
        return Value::Error(CellError::Value);
    };
    match (t.text(), n.number()) {
        (Ok(t), Ok(n)) if n >= 0.0 => {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked non-negative; the length cap below bounds it"
            )]
            let times = n as usize;
            // Excel refuses a result longer than a cell can hold.
            if t.chars().count().saturating_mul(times) > crate::model::MAX_STRING_LENGTH {
                return Value::Error(CellError::Value);
            }
            Value::Text(t.repeat(times))
        }
        (Ok(_), Ok(_)) => Value::Error(CellError::Value),
        (Err(e), _) | (_, Err(e)) => Value::Error(e),
    }
}

/// `EXACT(text1, text2)` - comparison that does mind the case, unlike `=`.
pub fn exact(args: &[Arg]) -> Value {
    let [a, b] = args else {
        return Value::Error(CellError::Value);
    };
    match (a.text(), b.text()) {
        (Ok(x), Ok(y)) => Value::Bool(x == y),
        (Err(e), _) | (_, Err(e)) => Value::Error(e),
    }
}

/// `FIND(needle, haystack, [start])` - case-sensitive.
pub fn find(args: &[Arg]) -> Value {
    locate(args, false)
}

/// `SEARCH(needle, haystack, [start])` - case-insensitive.
///
/// Excel also reads `?` and `*` in the needle as wildcards; that arrives with
/// the criteria functions (`COUNTIF` and its family), which need the same
/// matcher.
pub fn search(args: &[Arg]) -> Value {
    locate(args, true)
}

/// Shared body of `FIND` and `SEARCH`.
fn locate(args: &[Arg], fold_case: bool) -> Value {
    let (needle, haystack, start) = match args {
        [n, h] => (n.text(), h.text(), Ok(1.0)),
        [n, h, s] => (n.text(), h.text(), s.number()),
        _ => return Value::Error(CellError::Value),
    };
    // An argument that is already an error is passed along unchanged; only a
    // genuinely unusable one becomes `#VALUE!`.
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (Ok(needle), Ok(haystack), Ok(start)) = (needle, haystack, start) else {
        return Value::Error(CellError::Value);
    };
    if start < 1.0 {
        return Value::Error(CellError::Value);
    }
    let (needle, haystack) = if fold_case {
        (needle.to_uppercase(), haystack.to_uppercase())
    } else {
        (needle, haystack)
    };
    let chars: Vec<char> = haystack.chars().collect();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked at least 1 just above"
    )]
    let from = start as usize - 1;
    if from > chars.len() {
        return Value::Error(CellError::Value);
    }
    // `SEARCH` reads `*` and `?` as wildcards, with `~` before one to mean the
    // character itself; `FIND` takes them literally. That, and the case, is
    // the whole of the difference between the two.
    if fold_case && needle.contains(['*', '?']) {
        return super::wildcard_position(&needle, &haystack, from)
            .map_or(Value::Error(CellError::Value), count_value);
    }
    let tail: String = chars[from..].iter().collect();
    match tail.find(&needle) {
        // `find` answers in bytes; Excel counts characters.
        Some(byte) => count_value(from + tail[..byte].chars().count() + 1),
        None => Value::Error(CellError::Value),
    }
}

/// `SUBSTITUTE(text, old, new, [occurrence])`
pub fn substitute(args: &[Arg]) -> Value {
    let (text, old, new, which) = match args {
        [t, o, n] => (t.text(), o.text(), n.text(), Ok(0.0)),
        [t, o, n, i] => (t.text(), o.text(), n.text(), i.number()),
        _ => return Value::Error(CellError::Value),
    };
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (Ok(text), Ok(old), Ok(new), Ok(which)) = (text, old, new, which) else {
        return Value::Error(CellError::Value);
    };
    if old.is_empty() {
        return Value::Text(text);
    }
    if which == 0.0 {
        return Value::Text(text.replace(&old, &new));
    }
    if which < 1.0 {
        return Value::Error(CellError::Value);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "checked at least 1; a huge index simply matches nothing"
    )]
    let wanted = which as usize;
    // Replace one occurrence only, counted from the left.
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    let mut seen = 0usize;
    while let Some(at) = rest.find(&old) {
        seen += 1;
        out.push_str(&rest[..at]);
        out.push_str(if seen == wanted { &new } else { &old });
        rest = &rest[at + old.len()..];
    }
    out.push_str(rest);
    Value::Text(out)
}

/// `VALUE(text)` - text read back as the number it spells.
///
/// Excel takes "any of the constant number, date, or time formats" it knows,
/// so a grouped number, a currency amount, a percentage and a date all come
/// back as numbers. A date arrives as its serial, which is what a date is.
pub fn value(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    let evaluated = engine.eval_expr(origin, arg);
    let text = match evaluated.scalar() {
        Value::Number(n) => return Value::Number(*n),
        Value::Error(e) => return Value::Error(*e),
        Value::Bool(_) => return Value::Error(CellError::Value),
        other => match other.text() {
            Ok(t) => t,
            Err(e) => return Value::Error(e),
        },
    };
    number_of_text(&text, engine.book().epoch).map_or(Value::Error(CellError::Value), Value::Number)
}

/// The number a string spells, by any of the shapes Excel accepts.
fn number_of_text(text: &str, epoch: crate::shared::date::Epoch) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A leading currency symbol and the separators between groups of digits
    // are decoration around the number, not part of it.
    let stripped: String = trimmed
        .chars()
        .filter(|c| !matches!(c, ',' | '$' | '€' | '£' | '¥' | ' '))
        .collect();
    let (body, scale) = match stripped.strip_suffix('%') {
        Some(body) => (body, 0.01),
        None => (stripped.as_str(), 1.0),
    };
    if let Ok(n) = Value::Text(body.to_owned()).number() {
        return Some(n * scale);
    }
    // Not a number in any shape, so the last thing it may be is a date.
    crate::shared::date_parse::parse(trimmed).and_then(|parsed| parsed.serial(epoch))
}

/// `TEXT(value, format)` - a number rendered through a cell format string.
///
/// Lazy because it needs the workbook: which epoch the dates in it count from
/// decides what `dd.mm.yyyy` renders.
pub fn text_format(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [v, f] = args else {
        return Value::Error(CellError::Value);
    };
    let value = engine.eval_expr(origin, v);
    let code = engine.eval_expr(origin, f);
    if let Some(e) = value.error().or_else(|| code.error()) {
        return Value::Error(e);
    }
    let Ok(code) = code.text() else {
        return Value::Error(CellError::Value);
    };
    let epoch = engine.book().epoch;
    let rendered = match value.scalar() {
        Value::Text(t) => format::format(format::Value::Text(t), &code, epoch),
        other => match other.number() {
            Ok(n) => format::format(format::Value::Number(n), &code, epoch),
            Err(e) => return Value::Error(e),
        },
    };
    Value::Text(rendered)
}

/// Applies a body to exactly one text argument.
fn one_text(args: &[Arg], f: impl Fn(&str) -> Value) -> Value {
    match args {
        [a] => match a.text() {
            Ok(t) => f(&t),
            Err(e) => Value::Error(e),
        },
        _ => Value::Error(CellError::Value),
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a cell holds at most 32767 characters"
)]
fn count_value(n: usize) -> Value {
    Value::Number(n as f64)
}

/// `PROPER(text)` - the first letter of every word capitalised, the rest not.
///
/// A word starts after anything that is not a letter, so `o'neil` becomes
/// `O'Neil` and `2nd` stays `2Nd`, which is what Excel does.
pub fn proper(args: &[Arg]) -> Value {
    one_text(args, |t| {
        let mut out = String::with_capacity(t.len());
        let mut start_of_word = true;
        for c in t.chars() {
            if start_of_word {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start_of_word = !c.is_alphabetic();
        }
        Value::Text(out)
    })
}

/// `CLEAN(text)` - drops the control characters a terminal would not print.
pub fn clean(args: &[Arg]) -> Value {
    one_text(args, |t| {
        Value::Text(t.chars().filter(|c| !c.is_control()).collect())
    })
}

/// `T(value)` - the value if it is text, and the empty string if it is not.
pub fn t(args: &[Arg]) -> Value {
    let [a] = args else {
        return Value::Error(CellError::Value);
    };
    match a.value.scalar() {
        Value::Text(text) => Value::Text(text.clone()),
        Value::Error(e) => Value::Error(*e),
        _ => Value::Text(String::new()),
    }
}

/// `CHAR(number)` - the character of a code point, 1 to 255.
pub fn char_(args: &[Arg]) -> Value {
    code_point(args, 1.0, 255.0)
}

/// `UNICHAR(number)` - the same over the whole of Unicode.
pub fn unichar(args: &[Arg]) -> Value {
    code_point(args, 1.0, f64::from(u32::from(char::MAX)))
}

/// Shared body of `CHAR` and `UNICHAR`.
fn code_point(args: &[Arg], low: f64, high: f64) -> Value {
    super::one(args, |n| {
        let n = n.trunc();
        if n < low || n > high {
            return Value::Error(CellError::Value);
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "bounded by the range checked just above"
        )]
        let code = n as u32;
        // A surrogate half is a code point with no character of its own.
        char::from_u32(code).map_or(Value::Error(CellError::Value), |c| {
            Value::Text(c.to_string())
        })
    })
}

/// `CODE(text)` - the code of the first character, capped at 255 the way
/// Excel's own byte-oriented `CODE` is.
pub fn code(args: &[Arg]) -> Value {
    first_code(args, true)
}

/// `UNICODE(text)` - the same without the cap.
pub fn unicode(args: &[Arg]) -> Value {
    first_code(args, false)
}

/// Shared body of `CODE` and `UNICODE`.
fn first_code(args: &[Arg], legacy: bool) -> Value {
    one_text(args, |t| {
        let Some(c) = t.chars().next() else {
            return Value::Error(CellError::Value);
        };
        let code = u32::from(c);
        // An implementation may answer 63 - a question mark - for a character the legacy
        // single-byte `CODE` cannot name, which is what Excel shows.
        let code = if legacy && code > 255 { 63 } else { code };
        Value::Number(f64::from(code))
    })
}

/// `REPLACE(text, start, count, new)` - by position, where `SUBSTITUTE` works
/// by content.
pub fn replace(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let [text, start, count, new] = args else {
        return Value::Error(CellError::Value);
    };
    let (Ok(text), Ok(start), Ok(count), Ok(new)) =
        (text.text(), start.number(), count.number(), new.text())
    else {
        return Value::Error(CellError::Value);
    };
    if start < 1.0 || count < 0.0 {
        return Value::Error(CellError::Value);
    }
    let chars: Vec<char> = text.chars().collect();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "both checked non-negative; past the end simply appends"
    )]
    let (from, count) = ((start as usize - 1).min(chars.len()), count as usize);
    let mut out: String = chars[..from].iter().collect();
    out.push_str(&new);
    out.extend(chars.iter().skip(from.saturating_add(count)));
    Value::Text(out)
}

/// `TEXTJOIN(delimiter, ignore_empty, text1, ...)`
pub fn textjoin(args: &[Arg]) -> Value {
    let [delimiter, ignore_empty, rest @ ..] = args else {
        return Value::Error(CellError::Value);
    };
    if let Some(e) = first_error(&args[..2]) {
        return Value::Error(e);
    }
    let (Ok(delimiter), Ok(ignore_empty)) = (delimiter.text(), ignore_empty.value.boolean()) else {
        return Value::Error(CellError::Value);
    };
    let mut parts = Vec::new();
    for arg in rest {
        let mut flat = Vec::new();
        arg.value.flatten(&mut flat);
        for v in flat {
            match v {
                Value::Error(e) => return Value::Error(*e),
                Value::Blank if ignore_empty => {}
                other => {
                    let Ok(text) = other.text() else {
                        return Value::Error(CellError::Value);
                    };
                    if !(ignore_empty && text.is_empty()) {
                        parts.push(text);
                    }
                }
            }
        }
    }
    Value::Text(parts.join(&delimiter))
}

/// `TEXTBEFORE(text, delimiter, [instance])`
pub fn textbefore(args: &[Arg]) -> Value {
    split_at_delimiter(args, true)
}

/// `TEXTAFTER(text, delimiter, [instance])`
pub fn textafter(args: &[Arg]) -> Value {
    split_at_delimiter(args, false)
}

/// Shared body of `TEXTBEFORE` and `TEXTAFTER`.
///
/// A negative instance counts from the right, and an instance the text does not
/// have is `#N/A` - the two ways these differ from `FIND`.
fn split_at_delimiter(args: &[Arg], before: bool) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (text, delimiter, instance) = match args {
        [t, d] => (t.text(), d.text(), Ok(1.0)),
        [t, d, i] => (t.text(), d.text(), i.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(text), Ok(delimiter), Ok(instance)) = (text, delimiter, instance) else {
        return Value::Error(CellError::Value);
    };
    if delimiter.is_empty() || instance == 0.0 {
        return Value::Error(CellError::Value);
    }
    let found: Vec<usize> = text.match_indices(&delimiter).map(|(at, _)| at).collect();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an instance number past the matches falls through to #N/A"
    )]
    let wanted = instance.trunc() as isize;
    let at = if wanted > 0 {
        found.get(wanted.cast_unsigned() - 1).copied()
    } else {
        let from_end = wanted.unsigned_abs();
        found
            .len()
            .checked_sub(from_end)
            .and_then(|i| found.get(i))
            .copied()
    };
    let Some(at) = at else {
        return Value::Error(CellError::Na);
    };
    Value::Text(if before {
        text[..at].to_owned()
    } else {
        text[at + delimiter.len()..].to_owned()
    })
}

/// `NUMBERVALUE(text, [decimal], [group])` - text read as a number with the
/// separators said out loud, rather than the workbook's own.
pub fn numbervalue(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (text, decimal, group) = match args {
        [t] => (t.text(), Ok(".".to_owned()), Ok(",".to_owned())),
        [t, d] => (t.text(), d.text(), Ok(",".to_owned())),
        [t, d, g] => (t.text(), d.text(), g.text()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(text), Ok(decimal), Ok(group)) = (text, decimal, group) else {
        return Value::Error(CellError::Value);
    };
    // Only the first character of each separator counts, as Excel documents.
    let decimal = decimal.chars().next().unwrap_or('.');
    let group = group.chars().next().unwrap_or(',');
    let decimals = text.chars().filter(|c| *c == decimal).count();
    if decimals > 1 {
        return Value::Error(CellError::Value);
    }
    if let Some(at) = text.find(decimal)
        && text[at..].contains(group)
    {
        // From the separator itself, not past it: that is also how the two
        // being the same character is caught.
        return Value::Error(CellError::Value);
    }
    let plain: String = text
        .chars()
        .filter(|c| *c != group && !c.is_whitespace())
        .map(|c| if c == decimal { '.' } else { c })
        .collect();
    if plain.is_empty() {
        return Value::Number(0.0);
    }
    // A trailing run of percent signs divides by a hundred once each.
    let percents = plain.chars().rev().take_while(|c| *c == '%').count();
    let body = &plain[..plain.len() - percents];
    match body.parse::<f64>() {
        Ok(n) => Value::Number(n / 100f64.powi(i32::try_from(percents).unwrap_or(0))),
        Err(_) => Value::Error(CellError::Value),
    }
}

/// `FIXED(number, [decimals], [no_commas])` - a number as text, rounded and
/// grouped.
pub fn fixed(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (number, decimals, plain) = match args {
        [n] => (n.number(), Ok(2.0), false),
        [n, d] => (n.number(), d.number(), false),
        [n, d, c] => (n.number(), d.number(), c.value.boolean().unwrap_or(false)),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(number), Ok(decimals)) = (number, decimals) else {
        return Value::Error(CellError::Value);
    };
    let decimals = decimals.trunc().clamp(-127.0, 127.0);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to -127..=127 on the line above"
    )]
    let places = decimals as i32;
    // Negative places round the whole part: FIXED(1234.5,-2) is 1,200.
    let rounded = if places < 0 {
        let step = 10f64.powi(-places);
        (number / step).round() * step
    } else {
        number
    };
    // Rust's formatting rounds halves to even; Excel rounds them away from
    // zero, so `FIXED(0.5,0)` is 1 and `FIXED(-1234.5,0)` is -1,235.
    let places_shown = places.max(0).unsigned_abs() as usize;
    let step = 10f64.powi(places.max(0));
    let rounded = (rounded * step).abs().round().copysign(rounded) / step;
    let shown = format!("{rounded:.places_shown$}");
    Value::Text(if plain {
        shown
    } else {
        group_thousands(&shown)
    })
}

/// Puts a comma between every three digits of the whole part.
fn group_thousands(shown: &str) -> String {
    let (sign, rest) = match shown.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", shown),
    };
    let (whole, fraction) = match rest.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (rest, None),
    };
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    match fraction {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// `DOLLAR(number, [decimals])` - a number as currency text.
///
/// The dollar sign, because the locale a workbook was written in is not
/// recorded in the file; `FIXED` is the same thing without one.
pub fn dollar(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (number, decimals) = match args {
        [n] => (n.number(), Ok(2.0)),
        [n, d] => (n.number(), d.number()),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(number), Ok(decimals)) = (number, decimals) else {
        return Value::Error(CellError::Value);
    };
    let Value::Text(shown) = fixed(&[
        arg_of(Value::Number(number)),
        arg_of(Value::Number(decimals)),
    ]) else {
        return Value::Error(CellError::Value);
    };
    // A negative amount is written in brackets, the way an accountant does.
    Value::Text(match shown.strip_prefix('-') {
        Some(rest) => format!("(${rest})"),
        None => format!("${shown}"),
    })
}

/// `TEXTSPLIT(text, column_delimiter, [row_delimiter], [ignore_empty])`
pub fn textsplit(args: &[Arg]) -> Value {
    if let Some(e) = first_error(args) {
        return Value::Error(e);
    }
    let (text, across, down, ignore_empty) = match args {
        [t, c] => (t.text(), c.text(), None, false),
        [t, c, r] => (t.text(), c.text(), Some(r.text()), false),
        [t, c, r, i] => (
            t.text(),
            c.text(),
            Some(r.text()),
            i.value.boolean().unwrap_or(false),
        ),
        _ => return Value::Error(CellError::Value),
    };
    let (Ok(text), Ok(across)) = (text, across) else {
        return Value::Error(CellError::Value);
    };
    let down = match down {
        Some(Ok(d)) if !d.is_empty() => Some(d),
        Some(Err(e)) => return Value::Error(e),
        _ => None,
    };
    let split = |text: &str, on: &str| -> Vec<String> {
        if on.is_empty() {
            return vec![text.to_owned()];
        }
        text.split(on)
            .filter(|part| !(ignore_empty && part.is_empty()))
            .map(str::to_owned)
            .collect()
    };
    let lines = match &down {
        Some(down) => split(&text, down),
        None => vec![text],
    };
    let rows: Vec<Vec<Value>> = lines
        .iter()
        .map(|line| split(line, &across).into_iter().map(Value::Text).collect())
        .collect();
    if rows.iter().all(Vec::is_empty) {
        return Value::Error(CellError::Calc);
    }
    // A ragged split is padded, the way the stacking functions pad.
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    Value::array(
        rows.into_iter()
            .map(|mut row| {
                while row.len() < width {
                    row.push(Value::Error(CellError::Na));
                }
                row
            })
            .collect(),
    )
}

/// `ARRAYTOTEXT(array, [format])` - an array written out as one string.
pub fn arraytotext(args: &[Arg]) -> Value {
    let (array, strict) = match args {
        [a] => (a, false),
        [a, f] => (a, f.number().unwrap_or(0.0) != 0.0),
        _ => return Value::Error(CellError::Value),
    };
    let show = |value: &Value| match value {
        Value::Text(text) if strict => format!("{text:?}"),
        Value::Text(text) => text.clone(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        Value::Error(e) => e.as_str().to_owned(),
        Value::Blank => String::new(),
        other => other.text().unwrap_or_default(),
    };
    let Value::Array(rows) = &array.value else {
        return Value::Text(show(array.value.scalar()));
    };
    // The strict form writes what a formula would: braces, commas across a
    // row and semicolons between them. The plain form is a flat list.
    if strict {
        let lines: Vec<String> = rows
            .iter()
            .map(|row| row.iter().map(show).collect::<Vec<_>>().join(","))
            .collect();
        return Value::Text(format!("{{{}}}", lines.join(";")));
    }
    Value::Text(
        rows.iter()
            .flatten()
            .map(show)
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// `VALUETOTEXT(value, [format])` - one value written the same way.
pub fn valuetotext(args: &[Arg]) -> Value {
    let [value, rest @ ..] = args else {
        return Value::Error(CellError::Value);
    };
    let mut one = vec![arg_of(value.value.scalar().clone())];
    one.extend(rest.iter().cloned());
    arraytotext(&one)
}

/// `T(value)` and the rest take their argument as it is; this wraps a value
/// back into one for the functions built out of others.
fn arg_of(value: Value) -> Arg {
    Arg {
        value,
        reference: false,
    }
}

/// Half-width katakana and the full-width character each one stands for.
///
/// The two alphabets do not line up by arithmetic the way the ASCII range
/// does, so the mapping is a table. A voiced sound is written with two
/// half-width characters - `ｶ` plus `ﾞ` - and one full-width character, `ガ`,
/// which is why [`dbcs`] has to look ahead by one.
const KATAKANA: [(char, char); 63] = [
    ('｡', '。'),
    ('｢', '「'),
    ('｣', '」'),
    ('､', '、'),
    ('･', '・'),
    ('ｦ', 'ヲ'),
    ('ｧ', 'ァ'),
    ('ｨ', 'ィ'),
    ('ｩ', 'ゥ'),
    ('ｪ', 'ェ'),
    ('ｫ', 'ォ'),
    ('ｬ', 'ャ'),
    ('ｭ', 'ュ'),
    ('ｮ', 'ョ'),
    ('ｯ', 'ッ'),
    ('ｰ', 'ー'),
    ('ｱ', 'ア'),
    ('ｲ', 'イ'),
    ('ｳ', 'ウ'),
    ('ｴ', 'エ'),
    ('ｵ', 'オ'),
    ('ｶ', 'カ'),
    ('ｷ', 'キ'),
    ('ｸ', 'ク'),
    ('ｹ', 'ケ'),
    ('ｺ', 'コ'),
    ('ｻ', 'サ'),
    ('ｼ', 'シ'),
    ('ｽ', 'ス'),
    ('ｾ', 'セ'),
    ('ｿ', 'ソ'),
    ('ﾀ', 'タ'),
    ('ﾁ', 'チ'),
    ('ﾂ', 'ツ'),
    ('ﾃ', 'テ'),
    ('ﾄ', 'ト'),
    ('ﾅ', 'ナ'),
    ('ﾆ', 'ニ'),
    ('ﾇ', 'ヌ'),
    ('ﾈ', 'ネ'),
    ('ﾉ', 'ノ'),
    ('ﾊ', 'ハ'),
    ('ﾋ', 'ヒ'),
    ('ﾌ', 'フ'),
    ('ﾍ', 'ヘ'),
    ('ﾎ', 'ホ'),
    ('ﾏ', 'マ'),
    ('ﾐ', 'ミ'),
    ('ﾑ', 'ム'),
    ('ﾒ', 'メ'),
    ('ﾓ', 'モ'),
    ('ﾔ', 'ヤ'),
    ('ﾕ', 'ユ'),
    ('ﾖ', 'ヨ'),
    ('ﾗ', 'ラ'),
    ('ﾘ', 'リ'),
    ('ﾙ', 'ル'),
    ('ﾚ', 'レ'),
    ('ﾛ', 'ロ'),
    ('ﾜ', 'ワ'),
    ('ﾝ', 'ン'),
    ('ﾞ', '゛'),
    ('ﾟ', '゜'),
];

/// `ASC(text)` - full-width characters narrowed to half-width.
///
/// The ASCII range is a fixed offset apart (`Ａ` is `A` plus 0xFEE0) and the
/// ideographic space is its own case; katakana comes from the table, read
/// backwards, and a voiced character becomes the two half-width ones it is
/// written with.
pub fn asc(args: &[Arg]) -> Value {
    one_text(args, |t| {
        let mut out = String::with_capacity(t.len());
        for c in t.chars() {
            match c {
                '\u{FF01}'..='\u{FF5E}' => out.push(narrow(c)),
                '\u{3000}' => out.push(' '),
                _ => match KATAKANA.iter().find(|(_, wide)| *wide == c) {
                    Some((narrow, _)) => out.push(*narrow),
                    None => match voiced(c) {
                        Some((base, mark)) => {
                            out.push(base);
                            out.push(mark);
                        }
                        None => out.push(c),
                    },
                },
            }
        }
        Value::Text(out)
    })
}

/// `DBCS(text)`, also spelled `JIS` - half-width characters widened.
///
/// A half-width katakana followed by a voiced or semi-voiced mark is one
/// full-width character, so the two are folded together when the combination
/// exists; `ｱﾞ` has none, and stays two characters.
pub fn dbcs(args: &[Arg]) -> Value {
    one_text(args, |t| {
        let chars: Vec<char> = t.chars().collect();
        let mut out = String::with_capacity(t.len());
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let wide = match c {
                '\u{0021}'..='\u{007E}' => Some(widen(c)),
                ' ' => Some('\u{3000}'),
                _ => KATAKANA
                    .iter()
                    .find(|(narrow, _)| *narrow == c)
                    .map(|(_, wide)| *wide),
            };
            let wide = wide.unwrap_or(c);
            // `ｶ` + `ﾞ` is one character, `ガ`, when that character exists.
            if let Some(&mark) = chars.get(i + 1)
                && matches!(mark, 'ﾞ' | 'ﾟ')
                && let Some(joined) = join_voiced(wide, mark)
            {
                out.push(joined);
                i += 2;
                continue;
            }
            out.push(wide);
            i += 1;
        }
        Value::Text(out)
    })
}

/// The half-width twin of a full-width ASCII character.
fn narrow(c: char) -> char {
    char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
}

/// The full-width twin of a half-width ASCII character.
fn widen(c: char) -> char {
    char::from_u32(c as u32 + 0xFEE0).unwrap_or(c)
}

/// A voiced katakana split into the plain character and its mark, which is how
/// half-width writes it. Voicing moves a character one or two code points
/// along its row, and only where the row has a voiced form.
fn voiced(c: char) -> Option<(char, char)> {
    let (base, mark) = match c {
        'ガ'..='ヂ' | 'ヅ'..='ド' if is_voiced_pair(c) => (step_back(c, 1), 'ﾞ'),
        'バ' | 'ビ' | 'ブ' | 'ベ' | 'ボ' => (step_back(c, 1), 'ﾞ'),
        'パ' | 'ピ' | 'プ' | 'ペ' | 'ポ' => (step_back(c, 2), 'ﾟ'),
        'ヴ' => ('ｳ', 'ﾞ'),
        _ => return None,
    };
    let narrow = KATAKANA.iter().find(|(_, wide)| *wide == base)?;
    Some((narrow.0, mark))
}

/// Whether a character in the `ガ`..`ド` span is the voiced member of its pair
/// rather than the plain one: they alternate.
fn is_voiced_pair(c: char) -> bool {
    matches!(
        c,
        'ガ' | 'ギ'
            | 'グ'
            | 'ゲ'
            | 'ゴ'
            | 'ザ'
            | 'ジ'
            | 'ズ'
            | 'ゼ'
            | 'ゾ'
            | 'ダ'
            | 'ヂ'
            | 'ヅ'
            | 'デ'
            | 'ド'
    )
}

/// The character `n` code points earlier.
fn step_back(c: char, n: u32) -> char {
    char::from_u32(c as u32 - n).unwrap_or(c)
}

/// A plain full-width katakana joined with its voicing mark, where such a
/// character exists.
fn join_voiced(base: char, mark: char) -> Option<char> {
    let step = match (base, mark) {
        ('ウ', 'ﾞ') => return Some('ヴ'),
        ('カ'..='ト', 'ﾞ') if is_plain_pair(base) => 1,
        ('ハ' | 'ヒ' | 'フ' | 'ヘ' | 'ホ', 'ﾞ') => 1,
        ('ハ' | 'ヒ' | 'フ' | 'ヘ' | 'ホ', 'ﾟ') => 2,
        _ => return None,
    };
    char::from_u32(base as u32 + step)
}

/// Whether a character in the `カ`..`ト` span can take a voicing mark: the
/// plain members of the pairs can, the voiced ones cannot.
fn is_plain_pair(c: char) -> bool {
    matches!(
        c,
        'カ' | 'キ'
            | 'ク'
            | 'ケ'
            | 'コ'
            | 'サ'
            | 'シ'
            | 'ス'
            | 'セ'
            | 'ソ'
            | 'タ'
            | 'チ'
            | 'ツ'
            | 'テ'
            | 'ト'
    )
}

/// The Thai numerals, which `THAIDIGIT` writes and `ISTHAIDIGIT` recognises.
const THAI_DIGITS: [char; 10] = ['๐', '๑', '๒', '๓', '๔', '๕', '๖', '๗', '๘', '๙'];

/// The Thai words for the digits, as `BAHTTEXT` spells them.
const THAI_WORDS: [&str; 10] = [
    "ศูนย์",
    "หนึ่ง",
    "สอง",
    "สาม",
    "สี่",
    "ห้า",
    "หก",
    "เจ็ด",
    "แปด",
    "เก้า",
];

/// The Thai words for the powers of ten, from ten to a million.
const THAI_UNITS: [&str; 6] = ["สิบ", "ร้อย", "พัน", "หมื่น", "แสน", "ล้าน"];

/// `THAIDIGIT(text)` - Arabic numerals rewritten as Thai ones.
pub fn thaidigit(args: &[Arg]) -> Value {
    one_text(args, |t| {
        Value::Text(
            t.chars()
                .map(|c| {
                    c.to_digit(10)
                        .and_then(|d| THAI_DIGITS.get(d as usize).copied())
                        .unwrap_or(c)
                })
                .collect(),
        )
    })
}

/// `ISTHAIDIGIT(text)` - whether the text is written in Thai numerals.
///
/// Empty text is not: there are no Thai digits in it.
pub fn isthaidigit(args: &[Arg]) -> Value {
    one_text(args, |t| {
        Value::Bool(!t.is_empty() && t.chars().all(|c| THAI_DIGITS.contains(&c)))
    })
}

/// `ROUNDBAHTUP(number)` - the amount rounded up to a whole baht.
///
/// Microsoft documents neither of this pair, so the one thing pinning them is
/// the name: a baht is the unit, so the rounding is to no decimal places. The
/// sign follows `ROUNDUP`, away from zero, because that is the only convention
/// Excel states for a rounding pair of its own.
pub fn roundbahtup(args: &[Arg]) -> Value {
    super::one(args, |n| Value::Number(n.abs().ceil().copysign(n)))
}

/// `ROUNDBAHTDOWN(number)` - the same towards zero.
pub fn roundbahtdown(args: &[Arg]) -> Value {
    super::one(args, |n| Value::Number(n.abs().floor().copysign(n)))
}

/// `BAHTTEXT(number)` - an amount of money written out in Thai.
///
pub fn bahttext(args: &[Arg]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    let number = match arg.number() {
        Ok(n) => n,
        Err(e) => return Value::Error(e),
    };
    let negative = number < 0.0;
    // Two decimal places, rounded, as the satang are a hundredth of a baht.
    let hundredths = (number.abs() * 100.0).round();
    if !hundredths.is_finite() {
        return Value::Error(CellError::Value);
    }
    let baht = format!("{}", (hundredths / 100.0).trunc());
    let satang = format!("{:02}", (hundredths % 100.0).trunc());

    let has_baht = baht != "0";
    let has_satang = satang != "00";
    if !has_baht && !has_satang {
        return Value::Text(format!("{}บาทถ้วน", THAI_WORDS[0]));
    }
    let mut out = String::new();
    if negative {
        out.push_str("ลบ");
    }
    if has_baht {
        out.push_str(&thai_large(&baht));
        out.push_str("บาท");
    }
    if has_satang {
        out.push_str(&thai_block(&satang));
        out.push_str("สตางค์");
    } else {
        out.push_str("ถ้วน");
    }
    Value::Text(out)
}

/// A whole number in Thai: blocks of six digits, joined by the word for a
/// million.
fn thai_large(digits: &str) -> String {
    let head = match digits.len() % 6 {
        0 => 6,
        n => n,
    };
    let (first, rest) = digits.split_at(head.min(digits.len()));
    let mut blocks = vec![thai_block(first)];
    let rest: Vec<char> = rest.chars().collect();
    for block in rest.chunks(6) {
        blocks.push(thai_block(&block.iter().collect::<String>()));
    }
    blocks.join(THAI_UNITS[5])
}

/// One block of up to six digits.
fn thai_block(block: &str) -> String {
    let digits: Vec<u32> = block.chars().filter_map(|c| c.to_digit(10)).collect();
    let mut out = String::new();
    let length = digits.len();
    let mut i = 0;
    // Hundreds and above are regular: the digit, then its power of ten.
    for power in (2..length).rev() {
        let digit = digits[i];
        i += 1;
        if digit != 0 {
            out.push_str(THAI_WORDS[digit as usize]);
            out.push_str(THAI_UNITS[power - 1]);
        }
    }
    let ten = if length > 1 {
        let ten = digits[i];
        i += 1;
        ten
    } else {
        0
    };
    if ten != 0 {
        // Ten is not "one ten", and twenty has a word of its own.
        match ten {
            1 => {}
            2 => out.push_str("ยี่"),
            other => out.push_str(THAI_WORDS[other as usize]),
        }
        out.push_str(THAI_UNITS[0]);
    }
    let one = digits.get(i).copied().unwrap_or(0);
    if one != 0 {
        if ten != 0 && one == 1 {
            out.push_str("เอ็ด");
        } else {
            out.push_str(THAI_WORDS[one as usize]);
        }
    }
    out
}
