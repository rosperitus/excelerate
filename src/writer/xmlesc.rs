//! Escaping text for XML, shared by the writers that produce it.

/// Escapes text for XML content and attribute values.
///
/// Excel refuses control characters outright, so they are dropped rather than
/// escaped — a file carrying them would not open at all.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Tab, newline and carriage return are the only control characters
            // XML 1.0 permits. A carriage return has to be written as a
            // reference: left literal, XML line-ending normalization folds it
            // away in whatever reads the file back.
            '\r' => out.push_str("&#13;"),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}
