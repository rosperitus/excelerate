//! `PARSEJSON(text, [path], [flatten])` - JSON text as a value or a table.
//!
//! Not an Excel function: it is proposed in the 2026 specification this
//! engine follows for its newest functions, and the name and behaviour may
//! change. The reader is a small one of our own, limited as the proposal asks:
//! 32 767 characters (the most a cell holds) and 64 levels of nesting.
//!
//! How JSON becomes a value: `null` is a blank; a list is a column, or rows
//! when every element is a list; an object is rows of key and value; anything
//! nested deeper is a nested array in its cell. With `flatten`, a list of
//! objects is a table with the keys as headers. The path is the subset of
//! `JSONPath` that names one thing: `$`, `.key`, `[n]`, `["key"]`.

use super::Arg;
use crate::error::CellError;
use crate::formula::value::Value;

/// The most characters a cell holds, and so the most `PARSEJSON` reads.
const MAX_LENGTH: usize = 32_767;
/// How deep lists and objects may nest.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Self>),
    Object(Vec<(String, Self)>),
}

/// `PARSEJSON(text, [path], [flatten])`.
pub fn parsejson(args: &[Arg]) -> Value {
    let ([text] | [text, _] | [text, _, _]) = args else {
        return Value::Error(CellError::Value);
    };
    let text = match text.text() {
        Ok(t) => t,
        Err(e) => return Value::Error(e),
    };
    if text.chars().count() > MAX_LENGTH {
        return Value::Error(CellError::Value);
    }
    let Some(json) = parse(&text) else {
        return Value::Error(CellError::Value);
    };
    let path = match args.get(1) {
        Some(arg) if !arg.missing() => match arg.text() {
            Ok(p) => p,
            Err(e) => return Value::Error(e),
        },
        _ => String::new(),
    };
    let table = match args.get(2) {
        Some(arg) if !arg.missing() => match arg.number() {
            Ok(n) => n != 0.0,
            Err(e) => return Value::Error(e),
        },
        _ => false,
    };
    let Some(steps) = steps(&path) else {
        return Value::Error(CellError::Value);
    };
    let mut at = &json;
    for step in steps {
        let next = match (at, step) {
            (Json::Object(fields), Step::Key(key)) => {
                fields.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
            }
            (Json::List(items), Step::Index(i)) => items.get(i),
            _ => None,
        };
        let Some(next) = next else {
            return Value::Error(CellError::Na);
        };
        at = next;
    }
    if table && let Some(rows) = records(at) {
        return Value::array(rows);
    }
    value_of(at)
}

/// A JSON value as a cell value, nested arrays for what nests.
fn value_of(json: &Json) -> Value {
    match json {
        Json::Null => Value::Blank,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Number(*n),
        Json::Text(t) => Value::Text(t.clone()),
        Json::List(items) if items.is_empty() => Value::Error(CellError::Calc),
        Json::List(items) if items.iter().all(|i| matches!(i, Json::List(_))) => {
            let mut rows: Vec<Vec<Value>> = items
                .iter()
                .map(|i| match i {
                    Json::List(row) => row.iter().map(value_of).collect(),
                    _ => Vec::new(),
                })
                .collect();
            let width = rows.iter().map(Vec::len).max().unwrap_or(0);
            if width == 0 {
                return Value::Error(CellError::Calc);
            }
            for row in &mut rows {
                row.resize(width, Value::Blank);
            }
            Value::array(rows)
        }
        Json::List(items) => Value::array(items.iter().map(|i| vec![value_of(i)]).collect()),
        Json::Object(fields) if fields.is_empty() => Value::Error(CellError::Calc),
        Json::Object(fields) => Value::array(
            fields
                .iter()
                .map(|(k, v)| vec![Value::Text(k.clone()), value_of(v)])
                .collect(),
        ),
    }
}

/// A list of objects as a table: the keys in order of first appearance, then
/// a row per object, blank where an object lacks a key.
fn records(json: &Json) -> Option<Vec<Vec<Value>>> {
    let Json::List(items) = json else {
        return None;
    };
    let mut keys: Vec<&str> = Vec::new();
    for item in items {
        let Json::Object(fields) = item else {
            return None;
        };
        for (k, _) in fields {
            if !keys.contains(&k.as_str()) {
                keys.push(k);
            }
        }
    }
    if keys.is_empty() {
        return None;
    }
    let mut rows = vec![keys.iter().map(|k| Value::Text((*k).to_owned())).collect()];
    for item in items {
        let Json::Object(fields) = item else {
            return None;
        };
        rows.push(
            keys.iter()
                .map(|key| {
                    fields
                        .iter()
                        .find(|(k, _)| k == key)
                        .map_or(Value::Blank, |(_, v)| value_of(v))
                })
                .collect(),
        );
    }
    Some(rows)
}

#[derive(Debug, PartialEq)]
enum Step {
    Key(String),
    Index(usize),
}

/// A path as its steps; `None` when it is not one.
fn steps(path: &str) -> Option<Vec<Step>> {
    let path = path.trim();
    let mut rest = match path.strip_prefix('$') {
        Some(rest) => rest,
        None if path.is_empty() || path.starts_with('[') => path,
        // A path may start with a bare key: `a.b` is `$.a.b`.
        None => return steps(&format!("$.{path}")),
    };
    let mut out = Vec::new();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            if end == 0 {
                return None;
            }
            out.push(Step::Key(after[..end].to_owned()));
            rest = &after[end..];
        } else {
            let after = rest.strip_prefix('[')?;
            let end = after.find(']')?;
            let inside = after[..end].trim();
            let quoted = inside
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| inside.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));
            out.push(match quoted {
                Some(key) => Step::Key(key.to_owned()),
                None => Step::Index(inside.parse().ok()?),
            });
            rest = &after[end + 1..];
        }
    }
    Some(out)
}

/// Reads a whole JSON document; `None` when it is not one.
fn parse(text: &str) -> Option<Json> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        at: 0,
    };
    let json = reader.value(0)?;
    reader.space();
    (reader.at == reader.bytes.len()).then_some(json)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.space();
        let hit = self.bytes.get(self.at) == Some(&byte);
        if hit {
            self.at += 1;
        }
        hit
    }

    fn word(&mut self, word: &str) -> bool {
        let hit = self.bytes[self.at..].starts_with(word.as_bytes());
        if hit {
            self.at += word.len();
        }
        hit
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.space();
        match *self.bytes.get(self.at)? {
            b'n' => self.word("null").then_some(Json::Null),
            b't' => self.word("true").then_some(Json::Bool(true)),
            b'f' => self.word("false").then_some(Json::Bool(false)),
            b'"' => self.string().map(Json::Text),
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b']') {
                    return Some(Json::List(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Some(Json::List(items));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                if self.eat(b'}') {
                    return Some(Json::Object(fields));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    if !self.eat(b':') {
                        return None;
                    }
                    fields.push((key, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Some(Json::Object(fields));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).ok()?;
        // JSON has no `+1`, `.5` or `1.`; Rust's reader would take them.
        let digits = text.strip_prefix('-').unwrap_or(text);
        let (whole, tail) = digits.split_at(
            digits
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(digits.len()),
        );
        let well_formed = (whole == "0" || !whole.is_empty() && !whole.starts_with('0'))
            && tail
                .strip_prefix('.')
                .is_none_or(|f| f.starts_with(|c: char| c.is_ascii_digit()));
        let n: f64 = text.parse().ok()?;
        (well_formed && n.is_finite()).then_some(Json::Number(n))
    }

    /// A string, the opening quote not yet consumed.
    fn string(&mut self) -> Option<String> {
        if self.bytes.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        let mut out = String::new();
        loop {
            let start = self.at;
            while self
                .bytes
                .get(self.at)
                .is_some_and(|b| !matches!(b, b'"' | b'\\') && *b >= 0x20)
            {
                self.at += 1;
            }
            // The input is a `&str` and the run stops only at ASCII, so this
            // is always whole characters.
            out.push_str(std::str::from_utf8(&self.bytes[start..self.at]).ok()?);
            match *self.bytes.get(self.at)? {
                b'"' => {
                    self.at += 1;
                    return Some(out);
                }
                b'\\' => {
                    let escape = *self.bytes.get(self.at + 1)?;
                    self.at += 2;
                    out.push(match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode()?,
                        _ => return None,
                    });
                }
                // A control character must be escaped.
                _ => return None,
            }
        }
    }

    /// The character after `\u`, joining a surrogate pair.
    fn unicode(&mut self) -> Option<char> {
        let high = self.hex()?;
        if (0xD800..0xDC00).contains(&high) {
            if !self.word("\\u") {
                return None;
            }
            let low = self.hex()?;
            if !(0xDC00..0xE000).contains(&low) {
                return None;
            }
            return char::from_u32(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00));
        }
        char::from_u32(high)
    }

    fn hex(&mut self) -> Option<u32> {
        let digits = self.bytes.get(self.at..self.at + 4)?;
        self.at += 4;
        u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_json_allows_and_nothing_else() {
        assert_eq!(
            parse(r#" {"a": [1, -2.5e1, true, null, "xé😀"]} "#),
            Some(Json::Object(vec![(
                "a".into(),
                Json::List(vec![
                    Json::Number(1.0),
                    Json::Number(-25.0),
                    Json::Bool(true),
                    Json::Null,
                    Json::Text("xé😀".into()),
                ])
            )]))
        );
        for bad in [
            r#"{"a":"#, "[1,]", "01", "+1", ".5", "1.", "[1] 2", "\"\t\"",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert_eq!(parse(&deep), None);
    }

    #[test]
    fn a_path_names_keys_and_positions() {
        assert_eq!(
            steps("$.orders[0][\"total sum\"]"),
            Some(vec![
                Step::Key("orders".into()),
                Step::Index(0),
                Step::Key("total sum".into())
            ])
        );
        assert_eq!(steps("a.b"), steps("$.a.b"));
        assert_eq!(steps(""), Some(vec![]));
        assert_eq!(steps("$..a"), None);
        assert_eq!(steps("$[x]"), None);
    }
}
