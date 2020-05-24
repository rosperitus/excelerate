//! Translating formulas between `OpenDocument` and A1 notation.
//!
//!
//! The two dialects say the same things differently:
//!
//! | A1 | `OpenDocument` |
//! |---|---|
//! | `A1` | `[.A1]` |
//! | `A1:B2` | `[.A1:.B2]` |
//! | `Sheet2!A1` | `['Sheet2'.A1]` |
//! | `SUM(A1,B1)` | `SUM([.A1];[.B1])` |
//! | `{1,2;3,4}` | `{1;2|3;4}` |
//! | `Rate` (a name) | `$$Rate` |
//! | `CEILING(` | `COM.MICROSOFT.CEILING(` |
//!
//! Text in quotes is left alone in both directions: `="A1"` is a string, not a
//! reference, and a semicolon inside one is a semicolon.

/// Turns an `OpenDocument` formula into A1 notation.
///
/// The leading `of:=` an ODS file writes is stripped if present.
#[must_use]
pub fn to_a1(formula: &str) -> String {
    let body = formula
        .strip_prefix("of:")
        .unwrap_or(formula)
        .strip_prefix('=')
        .unwrap_or_else(|| formula.strip_prefix("of:").unwrap_or(formula));
    let mut out = String::with_capacity(body.len());
    // Braces nest, and `;` means a different thing inside a matrix than in an
    // argument list, so the depth has to be tracked rather than guessed.
    let mut matrix_depth = 0usize;
    for (index, part) in split_on_strings(body).enumerate() {
        if index % 2 == 1 {
            out.push_str(part);
            continue;
        }
        let mut chars = part.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            match c {
                '[' => {
                    // A reference runs to the closing bracket; brackets do not
                    // nest inside one.
                    let end = part[at..].find(']').map(|i| at + i);
                    if let Some(end) = end {
                        out.push_str(&reference_to_a1(&part[at + 1..end]));
                        while chars.peek().is_some_and(|&(i, _)| i <= end) {
                            chars.next();
                        }
                    } else {
                        out.push(c);
                    }
                }
                '{' => {
                    matrix_depth += 1;
                    out.push(c);
                }
                '}' => {
                    matrix_depth = matrix_depth.saturating_sub(1);
                    out.push(c);
                }
                ';' if matrix_depth > 0 => out.push(','),
                ';' => out.push(','),
                '|' if matrix_depth > 0 => out.push(';'),
                '$' if part[at..].starts_with("$$") => {
                    // `$$Name` marks a defined name, which A1 writes bare.
                    chars.next();
                }
                _ => out.push(c),
            }
        }
    }
    strip_vendor_prefix(&out)
}

/// Turns an A1 formula into `OpenDocument` notation, with its `of:=` prefix.
///
/// `names` are the workbook's defined names, which ODS marks with `$$` so a
/// reader can tell them from a function call or a sheet.
#[must_use]
pub fn from_a1(formula: &str, names: &[String]) -> String {
    let mut out = String::from("of:=");
    for (index, part) in split_on_strings(formula).enumerate() {
        if index % 2 == 1 {
            out.push_str(part);
            continue;
        }
        out.push_str(&part_from_a1(part, names));
    }
    out
}

/// One run of formula text outside any string literal, in A1 notation.
fn part_from_a1(part: &str, names: &[String]) -> String {
    let mut out = String::with_capacity(part.len());
    let chars: Vec<char> = part.chars().collect();
    let mut i = 0;
    let mut matrix_depth = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '{' => {
                matrix_depth += 1;
                out.push(c);
                i += 1;
            }
            '}' => {
                matrix_depth = matrix_depth.saturating_sub(1);
                out.push(c);
                i += 1;
            }
            ',' if matrix_depth > 0 => {
                out.push(';');
                i += 1;
            }
            ';' if matrix_depth > 0 => {
                out.push('|');
                i += 1;
            }
            ',' => {
                out.push(';');
                i += 1;
            }
            _ => match read_reference(&chars, i) {
                Some((end, reference)) => {
                    out.push_str(&reference);
                    i = end;
                }
                None => {
                    if let Some((end, word)) = read_word(&chars, i) {
                        // A name the workbook defines is marked; a function
                        // call — a word followed by `(` — never is.
                        let called = chars.get(end) == Some(&'(');
                        if !called && names.iter().any(|n| n.eq_ignore_ascii_case(&word)) {
                            out.push_str("$$");
                        }
                        out.push_str(&if called { vendor_prefix(&word) } else { word });
                        i = end;
                    } else {
                        out.push(c);
                        i += 1;
                    }
                }
            },
        }
    }
    out
}

/// Splits a formula into runs outside and inside string literals, the literals
/// themselves at the odd positions and keeping their quotes.
///
fn split_on_strings(formula: &str) -> impl Iterator<Item = &str> {
    let mut rest = formula;
    let mut in_string = false;
    core::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        if in_string {
            in_string = false;
            let mut end = 1;
            let bytes = rest.as_bytes();
            while end < bytes.len() {
                if bytes[end] == b'"' {
                    if bytes.get(end + 1) == Some(&b'"') {
                        end += 2;
                        continue;
                    }
                    end += 1;
                    break;
                }
                end += 1;
            }
            let (literal, tail) = rest.split_at(end.min(rest.len()));
            rest = tail;
            return Some(literal);
        }
        match rest.find('"') {
            Some(0) => {
                in_string = true;
                Some("")
            }
            Some(at) => {
                in_string = true;
                let (plain, tail) = rest.split_at(at);
                rest = tail;
                Some(plain)
            }
            None => {
                let all = rest;
                rest = "";
                Some(all)
            }
        }
    })
}

/// The inside of an `OpenDocument` `[...]` reference, as A1.
///
/// `.A1` is a cell, `.A1:.B2` a range, `'Sheet 2'.A1` a cell elsewhere, and
/// `'Sheet 2'.A1:.B2` a range there. A missing sheet on the far end of a range
/// means the same sheet, which is also what A1 means by leaving it off.
fn reference_to_a1(inside: &str) -> String {
    let inside = inside.strip_prefix('$').unwrap_or(inside);
    if let Some((first, second)) = inside.split_once(':') {
        let (sheet, start) = split_sheet(first);
        let (_, end) = split_sheet(second);
        return match sheet {
            Some(sheet) => format!("{}!{start}:{end}", quote_sheet(&sheet)),
            None => format!("{start}:{end}"),
        };
    }
    let (sheet, cell) = split_sheet(inside);
    match sheet {
        Some(sheet) => format!("{}!{cell}", quote_sheet(&sheet)),
        None => cell,
    }
}

/// Splits `'Sheet 2'.A1` into its sheet and its cell.
///
/// The separator is the last `.` outside quotes: a sheet name may hold one, and
/// `''` inside a quoted name is an apostrophe rather than the end of it.
fn split_sheet(part: &str) -> (Option<String>, String) {
    let part = part.strip_prefix('$').unwrap_or(part);
    let mut quoted = false;
    let mut split_at = None;
    let mut chars = part.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '\'' if quoted && chars.peek().is_some_and(|&(_, n)| n == '\'') => {
                chars.next();
            }
            '\'' => quoted = !quoted,
            '.' if !quoted => split_at = Some(i),
            _ => {}
        }
    }
    // The dollars of the address itself are kept: `$B$4` is an absolute
    // reference in both dialects. Only the one a sheet name may carry is
    // dropped, and `split_sheet` already stripped that.
    match split_at {
        Some(0) => (None, part[1..].to_owned()),
        Some(i) => {
            let sheet = part[..i].trim_matches('\'').replace("''", "'");
            (Some(sheet), part[i + 1..].to_owned())
        }
        None => (None, part.to_owned()),
    }
}

/// A sheet name as A1 writes it, quoted when it needs to be.
fn quote_sheet(name: &str) -> String {
    let plain = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    if plain {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// An A1 reference starting at `i`, as an `OpenDocument` one, and where it ended.
///
/// Recognises `Sheet!A1`, `'Sheet 2'!A1:B2`, `$A$1` and the bare forms.
fn read_reference(chars: &[char], i: usize) -> Option<(usize, String)> {
    // A reference may not start in the middle of a word: `SIN1` is a name.
    if i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == '$') {
        return None;
    }
    let (after_sheet, sheet) = read_sheet_prefix(chars, i);
    let (after_first, first) = read_cell(chars, after_sheet)?;
    let mut end = after_first;
    let mut second = None;
    if chars.get(end) == Some(&':')
        && let Some((after, cell)) = read_cell(chars, end + 1)
    {
        second = Some(cell);
        end = after;
    }
    // A word character straight after is a name that merely began like a
    // reference, as `A1B` does.
    if chars
        .get(end)
        .is_some_and(|&c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    let head = match &sheet {
        Some(name) => format!("['{}'", name.replace('\'', "''")),
        None => "[".to_owned(),
    };
    Some(match second {
        Some(second) => (end, format!("{head}.{first}:.{second}]")),
        None => (end, format!("{head}.{first}]")),
    })
}

/// A `Sheet!` or `'Sheet 2'!` prefix at `i`, if there is one.
fn read_sheet_prefix(chars: &[char], i: usize) -> (usize, Option<String>) {
    if chars.get(i) == Some(&'\'') {
        let mut j = i + 1;
        let mut name = String::new();
        while j < chars.len() {
            if chars[j] == '\'' {
                if chars.get(j + 1) == Some(&'\'') {
                    name.push('\'');
                    j += 2;
                    continue;
                }
                break;
            }
            name.push(chars[j]);
            j += 1;
        }
        if chars.get(j) == Some(&'\'') && chars.get(j + 1) == Some(&'!') {
            return (j + 2, Some(name));
        }
        return (i, None);
    }
    let mut j = i;
    let mut name = String::new();
    while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '.') {
        name.push(chars[j]);
        j += 1;
    }
    if !name.is_empty() && chars.get(j) == Some(&'!') {
        (j + 1, Some(name))
    } else {
        (i, None)
    }
}

/// A cell reference such as `$A$1` at `i`, kept with its dollars.
fn read_cell(chars: &[char], i: usize) -> Option<(usize, String)> {
    let mut j = i;
    let mut out = String::new();
    if chars.get(j) == Some(&'$') {
        out.push('$');
        j += 1;
    }
    let letters = j;
    while j < chars.len() && chars[j].is_ascii_alphabetic() && j - letters < 3 {
        out.push(chars[j].to_ascii_uppercase());
        j += 1;
    }
    if j == letters {
        return None;
    }
    if chars.get(j) == Some(&'$') {
        out.push('$');
        j += 1;
    }
    let digits = j;
    while j < chars.len() && chars[j].is_ascii_digit() {
        out.push(chars[j]);
        j += 1;
    }
    if j == digits {
        return None;
    }
    Some((j, out))
}

/// A bare word — a function or a name — starting at `i`.
fn read_word(chars: &[char], i: usize) -> Option<(usize, String)> {
    if !chars[i].is_alphabetic() && chars[i] != '_' {
        return None;
    }
    let mut j = i;
    let mut out = String::new();
    while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '.') {
        out.push(chars[j]);
        j += 1;
    }
    Some((j, out))
}

/// Function names `OpenDocument` keeps under a vendor prefix because its own
/// versions of them round differently.
const VENDOR_PREFIXED: [&str; 2] = ["CEILING", "FLOOR"];

/// The `OpenDocument` spelling of a function name.
fn vendor_prefix(name: &str) -> String {
    let upper = name.to_uppercase();
    let base = upper.split('.').next().unwrap_or(&upper);
    if VENDOR_PREFIXED.contains(&base) && !upper.starts_with("COM.MICROSOFT.") {
        format!("COM.MICROSOFT.{upper}")
    } else {
        name.to_owned()
    }
}

/// Drops the vendor prefix a reader sees, so `COM.MICROSOFT.CEILING` is the
/// `CEILING` the rest of the crate knows.
fn strip_vendor_prefix(formula: &str) -> String {
    let mut out = formula.to_owned();
    while let Some(at) = out.to_uppercase().find("COM.MICROSOFT.") {
        out.replace_range(at..at + "COM.MICROSOFT.".len(), "");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(a1: &str, ods: &str) {
        assert_eq!(from_a1(a1, &[]), format!("of:={ods}"), "A1 -> ODS: {a1}");
        assert_eq!(to_a1(&format!("of:={ods}")), a1, "ODS -> A1: {ods}");
    }

    #[test]
    fn references_get_their_brackets() {
        round("A1", "[.A1]");
        round("$B$4", "[.$B$4]");
        round("A1:B2", "[.A1:.B2]");
        round("SUM(A1:A3)", "SUM([.A1:.A3])");
    }

    #[test]
    fn a_sheet_name_moves_inside_the_brackets() {
        round("Sheet2!A1", "['Sheet2'.A1]");
        round("Sheet2!A1:B2", "['Sheet2'.A1:.B2]");
        round("'Sheet 2'!A1", "['Sheet 2'.A1]");
        // An apostrophe in a name is doubled on both sides.
        round("'it''s'!A1", "['it''s'.A1]");
    }

    #[test]
    fn argument_and_matrix_separators_swap() {
        round("SUM(A1,B1)", "SUM([.A1];[.B1])");
        round("{1,2;3,4}", "{1;2|3;4}");
        round("SUM({1,2},A1)", "SUM({1;2};[.A1])");
    }

    #[test]
    fn text_in_quotes_is_left_alone() {
        round("\"A1;B1\"", "\"A1;B1\"");
        round("CONCATENATE(\"a,b\",A1)", "CONCATENATE(\"a,b\";[.A1])");
        // A doubled quote inside a literal does not end it.
        round("\"say \"\"A1\"\"\"", "\"say \"\"A1\"\"\"");
    }

    #[test]
    fn a_defined_name_is_marked() {
        assert_eq!(from_a1("Rate*2", &["Rate".to_owned()]), "of:=$$Rate*2");
        assert_eq!(to_a1("of:=$$Rate*2"), "Rate*2");
        // Only names the workbook has; a bare word otherwise stays bare.
        assert_eq!(from_a1("Rate*2", &[]), "of:=Rate*2");
    }

    #[test]
    fn the_two_functions_that_differ_keep_their_vendor_prefix() {
        assert_eq!(
            from_a1("CEILING(A1,1)", &[]),
            "of:=COM.MICROSOFT.CEILING([.A1];1)"
        );
        assert_eq!(to_a1("of:=COM.MICROSOFT.CEILING([.A1];1)"), "CEILING(A1,1)");
        assert_eq!(from_a1("SUM(A1)", &[]), "of:=SUM([.A1])");
    }

    #[test]
    fn a_word_that_merely_looks_like_a_reference_is_not_one() {
        assert_eq!(from_a1("A1B", &[]), "of:=A1B");
        assert_eq!(from_a1("SIN(A1)", &[]), "of:=SIN([.A1])");
        assert_eq!(from_a1("TRUE", &[]), "of:=TRUE");
    }
}
