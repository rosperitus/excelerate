//! Regular expressions: `REGEXTEST`, `REGEXEXTRACT`, `REGEXREPLACE`.
//!
//! Excel runs these on PCRE2. The engine here is the `regex` crate, which
//! agrees with it on classes, quantifiers, groups, anchors and named groups,
//! and has no lookaround and no backreferences: a pattern using them does not
//! compile, and the call is `#VALUE!` - the answer Excel gives a pattern it
//! cannot compile.
//!
//! A text argument that is an array is matched element by element.

use super::{Arg, first_error};
use crate::error::CellError;
use crate::formula::value::Value;
use ::regex::{Regex, RegexBuilder};

/// Compiles a pattern, case-insensitively when `case_sensitivity` is 1.
fn compile(pattern: &Arg, case_sensitivity: Option<&Arg>) -> Result<Regex, CellError> {
    let pattern = pattern.text()?;
    let insensitive = match case_sensitivity {
        None => false,
        Some(arg) => match arg.number()? {
            0.0 => false,
            1.0 => true,
            _ => return Err(CellError::Value),
        },
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(insensitive)
        // Excel's patterns run into megabytes of cells; a pattern that
        // compiles to more than this is refused rather than allocated.
        .size_limit(1 << 20)
        .build()
        .map_err(|_| CellError::Value)
}

/// Runs `f` over the text argument, element by element when it is an array.
fn each(text: &Value, f: &impl Fn(&str) -> Value) -> Value {
    match text {
        Value::Array(rows) => Value::array(
            rows.iter()
                .map(|row| row.iter().map(|v| one_text(v, f)).collect())
                .collect(),
        ),
        other => one_text(other, f),
    }
}

fn one_text(value: &Value, f: &impl Fn(&str) -> Value) -> Value {
    match value.text() {
        Ok(text) => f(&text),
        Err(e) => Value::Error(e),
    }
}

/// `REGEXTEST(text, pattern, [case_sensitivity])`
pub fn regextest(args: &[Arg]) -> Value {
    let (text, pattern, case) = match args {
        [t, p] => (t, p, None),
        [t, p, c] => (t, p, Some(c)),
        _ => return Value::Error(CellError::Value),
    };
    if let Some(e) = first_error(&args[1..]) {
        return Value::Error(e);
    }
    let re = match compile(pattern, case) {
        Ok(re) => re,
        Err(e) => return Value::Error(e),
    };
    each(&text.value, &|t| Value::Bool(re.is_match(t)))
}

/// `REGEXEXTRACT(text, pattern, [return_mode], [case_sensitivity])`
///
/// Mode 0 is the first match, 1 every match down a column, 2 the groups of the
/// first match across a row; a group that took no part is empty text. No
/// match is `#N/A`.
pub fn regexextract(args: &[Arg]) -> Value {
    let (text, pattern, mode, case) = match args {
        [t, p] => (t, p, None, None),
        [t, p, m] => (t, p, Some(m), None),
        [t, p, m, c] => (t, p, Some(m), Some(c)),
        _ => return Value::Error(CellError::Value),
    };
    if let Some(e) = first_error(&args[1..]) {
        return Value::Error(e);
    }
    let mode: u8 = match mode.map(Arg::number) {
        None | Some(Ok(0.0)) => 0,
        Some(Ok(1.0)) => 1,
        Some(Ok(2.0)) => 2,
        Some(Ok(_)) => return Value::Error(CellError::Value),
        Some(Err(e)) => return Value::Error(e),
    };
    let re = match compile(pattern, case) {
        Ok(re) => re,
        Err(e) => return Value::Error(e),
    };
    let na = Value::Error(CellError::Na);
    let extract = |t: &str| -> Value {
        if mode == 0 {
            return re
                .find(t)
                .map_or_else(|| na.clone(), |m| Value::Text(m.as_str().to_owned()));
        }
        if mode == 1 {
            let found: Vec<Vec<Value>> = re
                .find_iter(t)
                .map(|m| vec![Value::Text(m.as_str().to_owned())])
                .collect();
            return if found.is_empty() {
                na.clone()
            } else {
                Value::array(found)
            };
        }
        let Some(groups) = re.captures(t) else {
            return na.clone();
        };
        let row: Vec<Value> = groups
            .iter()
            .skip(1)
            .map(|g| Value::Text(g.map_or_else(String::new, |m| m.as_str().to_owned())))
            .collect();
        if row.is_empty() {
            na.clone()
        } else {
            Value::array(vec![row])
        }
    };
    // A column of texts gives one answer each only in mode 0; the other modes
    // answer with an array of their own.
    if mode == 0 {
        each(&text.value, &extract)
    } else {
        one_text(text.value.scalar(), &extract)
    }
}

/// `REGEXREPLACE(text, pattern, replacement, [occurrence], [case_sensitivity])`
///
/// Occurrence 0 replaces every match, `n` only the n-th, and `-n` the n-th
/// from the end. The replacement names groups as `$1` or `${name}`.
pub fn regexreplace(args: &[Arg]) -> Value {
    let (text, pattern, replacement, occurrence, case) = match args {
        [text, pattern, with] => (text, pattern, with, None, None),
        [text, pattern, with, nth] => (text, pattern, with, Some(nth), None),
        [text, pattern, with, nth, case] => (text, pattern, with, Some(nth), Some(case)),
        _ => return Value::Error(CellError::Value),
    };
    if let Some(e) = first_error(&args[1..]) {
        return Value::Error(e);
    }
    let replacement = match replacement.text() {
        Ok(r) => r,
        Err(e) => return Value::Error(e),
    };
    let occurrence = match occurrence.map(Arg::number) {
        None => 0,
        #[expect(
            clippy::cast_possible_truncation,
            reason = "truncated toward zero, as Excel reads a count"
        )]
        Some(Ok(n)) => n.trunc().clamp(-1e9, 1e9) as i64,
        Some(Err(e)) => return Value::Error(e),
    };
    let re = match compile(pattern, case) {
        Ok(re) => re,
        Err(e) => return Value::Error(e),
    };
    each(&text.value, &|t| {
        if occurrence == 0 {
            return Value::Text(re.replace_all(t, replacement.as_str()).into_owned());
        }
        let matches: Vec<_> = re.captures_iter(t).collect();
        let index = if occurrence > 0 {
            usize::try_from(occurrence - 1).ok()
        } else {
            usize::try_from(-occurrence)
                .ok()
                .and_then(|back| matches.len().checked_sub(back))
        };
        let Some(caps) = index.and_then(|i| matches.get(i)) else {
            return Value::Text(t.to_owned());
        };
        let Some(whole) = caps.get(0) else {
            return Value::Text(t.to_owned());
        };
        let mut out = String::with_capacity(t.len());
        out.push_str(&t[..whole.start()]);
        caps.expand(&replacement, &mut out);
        out.push_str(&t[whole.end()..]);
        Value::Text(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(value: Value) -> Arg {
        Arg {
            value,
            reference: false,
        }
    }

    fn text(s: &str) -> Arg {
        arg(Value::Text(s.to_owned()))
    }

    #[test]
    fn test_extract_and_replace() {
        assert_eq!(
            regextest(&[text("Привет, мир"), text("МИР"), arg(Value::Number(1.0))]),
            Value::Bool(true)
        );
        assert_eq!(regextest(&[text("abc"), text("B")]), Value::Bool(false));
        assert_eq!(
            regexextract(&[text("tel 555-1234 or 555-9876"), text(r"\d{3}-\d{4}")]),
            Value::Text("555-1234".into())
        );
        assert_eq!(
            regexextract(&[text("no digits"), text(r"\d+")]),
            Value::Error(CellError::Na)
        );
        assert_eq!(
            regexextract(&[text("a1 b22"), text(r"\d+"), arg(Value::Number(1.0))]),
            Value::array(vec![
                vec![Value::Text("1".into())],
                vec![Value::Text("22".into())]
            ])
        );
        assert_eq!(
            regexextract(&[
                text("Sonia Rees"),
                text(r"(\w+) (\w+)"),
                arg(Value::Number(2.0))
            ]),
            Value::array(vec![vec![
                Value::Text("Sonia".into()),
                Value::Text("Rees".into())
            ]])
        );
        assert_eq!(
            regexreplace(&[text("a-b-c"), text("-"), text("+")]),
            Value::Text("a+b+c".into())
        );
        assert_eq!(
            regexreplace(&[
                text("a-b-c"),
                text("-"),
                text("+"),
                arg(Value::Number(-1.0))
            ]),
            Value::Text("a-b+c".into())
        );
        assert_eq!(
            regexreplace(&[text("Rees, Sonia"), text(r"(\w+), (\w+)"), text("$2 $1")]),
            Value::Text("Sonia Rees".into())
        );
        // Lookaround is PCRE2's, not ours.
        assert_eq!(
            regextest(&[text("x"), text("(?=x)")]),
            Value::Error(CellError::Value)
        );
    }
}
