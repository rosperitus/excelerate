//! The web functions.
//!
//! Only the one that needs no network. `WEBSERVICE` fetches a URL and
//! `FILTERXML` runs an `XPath` over what it returned; a spreadsheet library that
//! opened sockets while recalculating would be a surprising thing to hand
//! someone, so neither is here.

use super::Arg;
use crate::error::CellError;
use crate::formula::value::Value;
use std::fmt::Write as _;

/// `ENCODEURL(text)` — percent-encoding, as a URL wants it.
pub fn encodeurl(args: &[Arg]) -> Value {
    let [arg] = args else {
        return Value::Error(CellError::Value);
    };
    let text = match arg.text() {
        Ok(text) => text,
        Err(e) => return Value::Error(e),
    };
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        // Unreserved characters stay; everything else becomes its bytes in
        // hexadecimal, which for text outside ASCII means several of them.
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    Value::Text(out)
}
