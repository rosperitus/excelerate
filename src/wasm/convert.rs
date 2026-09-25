//! Values across the boundary: addresses from numbers, cells to and from JS,
//! and the small builders every binding shares.

use super::js;
use crate::coordinate::{CellRef, Col, Range, Row};
use crate::model::CellValue;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// A cell reference from the 1-based numbers a user sees.
pub(super) fn at_index(row: u32, column: u32) -> Result<CellRef, JsError> {
    Ok(CellRef::new(col_at(column)?, row_at(row)?))
}

/// A row from the number a user sees.
pub(super) fn row_at(row: u32) -> Result<Row, JsError> {
    Row::from_one_based(u64::from(row)).map_err(js)
}

/// A column from its 1-based number.
pub(super) fn col_at(column: u32) -> Result<Col, JsError> {
    Col::from_one_based(u64::from(column)).map_err(js)
}

/// A rectangle from its top-left cell and its size, both 1-based.
pub(super) fn area_at(row: u32, column: u32, rows: u32, columns: u32) -> Result<Range, JsError> {
    if rows == 0 || columns == 0 {
        return Err(JsError::new("a range covers at least one row and column"));
    }
    let start = at_index(row, column)?;
    let end = at_index(row + rows - 1, column + columns - 1)?;
    Ok(Range::new(start, end))
}

/// The cell `rows` down and `columns` right of `start`, for a JS grid laid
/// out from that corner.
pub(super) fn offset(start: CellRef, rows: usize, columns: usize) -> Result<CellRef, JsError> {
    let row = u32::try_from(rows)
        .ok()
        .and_then(|r| Row::new(start.row.index().checked_add(r)?));
    let col = u32::try_from(columns)
        .ok()
        .and_then(|c| Col::new(start.col.index().checked_add(c)?));
    row.zip(col)
        .map(|(row, col)| CellRef::new(col, row))
        .ok_or_else(|| JsError::new("the grid runs off the sheet"))
}

/// Every cell of an area, row by row, as rows of cells.
pub(super) fn rows_of(area: Range) -> impl Iterator<Item = impl Iterator<Item = CellRef>> {
    (area.start.row.index()..=area.end.row.index())
        .filter_map(Row::new)
        .map(move |row| {
            (area.start.col.index()..=area.end.col.index())
                .filter_map(Col::new)
                .map(move |col| CellRef::new(col, row))
        })
}

/// A JS array, or the error naming what it should have been.
pub(super) fn array(value: &JsValue, what: &str) -> Result<js_sys::Array, JsError> {
    value
        .clone()
        .dyn_into()
        .map_err(|_| JsError::new(&format!("{what} must be an array")))
}

/// One JS value as a cell. A string starting with `=` is a formula.
pub(super) fn js_to_cell(value: &JsValue) -> Result<CellValue, JsError> {
    if let Some(n) = value.as_f64() {
        Ok(CellValue::Number(n))
    } else if let Some(b) = value.as_bool() {
        Ok(CellValue::Bool(b))
    } else if let Some(s) = value.as_string() {
        Ok(match s.strip_prefix('=') {
            Some(formula) => CellValue::Formula {
                formula: formula.to_owned(),
                cached: None,
            },
            None => CellValue::text(s),
        })
    } else if value.is_null() || value.is_undefined() {
        Ok(CellValue::Empty)
    } else {
        Err(JsError::new(
            "value must be a number, boolean, string or null",
        ))
    }
}

/// A stored value as JS: a formula shows its cached result, the way a cell
/// displays it.
pub(super) fn cell_to_js(value: &CellValue) -> JsValue {
    match value {
        CellValue::Number(n) => JsValue::from_f64(*n),
        CellValue::Bool(b) => JsValue::from_bool(*b),
        CellValue::Error(e) => JsValue::from_str(e.as_str()),
        CellValue::Text(_) | CellValue::RichText(_) => {
            JsValue::from_str(&value.plain_text().unwrap_or_default())
        }
        CellValue::Formula { cached, .. } => cached.as_deref().map_or(JsValue::NULL, cell_to_js),
        CellValue::Empty => JsValue::NULL,
    }
}

/// One field of a JS object, or `None` when it is absent or `undefined`.
pub(super) fn field(object: &JsValue, key: &str) -> Option<JsValue> {
    let value = js_sys::Reflect::get(object, &JsValue::from_str(key)).ok()?;
    (!value.is_undefined()).then_some(value)
}

/// One character from JS, for a delimiter.
pub(super) fn one_char(value: &JsValue) -> Result<char, JsError> {
    value
        .as_string()
        .and_then(|text| {
            let mut chars = text.chars();
            chars.next().filter(|_| chars.next().is_none())
        })
        .ok_or_else(|| JsError::new("a delimiter is one character"))
}

/// Builds a JS object from `(key, value)` pairs.
pub(super) fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let out = js_sys::Object::new();
    for (key, value) in fields {
        // The only way this fails is a frozen object, and this one is ours.
        let _ = js_sys::Reflect::set(&out, &JsValue::from_str(key), value);
    }
    out.into()
}

/// A JS array of what `items` turn into.
pub(super) fn list<T>(items: impl IntoIterator<Item = T>, each: impl Fn(T) -> JsValue) -> JsValue {
    items
        .into_iter()
        .map(each)
        .collect::<js_sys::Array>()
        .into()
}

/// A list of areas as their `"A1:C9"` strings.
pub(super) fn ranges_to_js(ranges: &[Range]) -> JsValue {
    list(ranges, |area| JsValue::from_str(&area.to_string()))
}

/// A string that may not be there.
pub(super) fn opt_str(text: Option<&str>) -> JsValue {
    text.map_or(JsValue::NULL, JsValue::from_str)
}

/// A count that may not be there.
pub(super) fn opt_count(count: Option<u32>) -> JsValue {
    count.map_or(JsValue::NULL, |n| JsValue::from_f64(f64::from(n)))
}

/// A count as a JS number.
#[expect(
    clippy::cast_precision_loss,
    reason = "counts of cells, series, fields and bytes are far below 2^53"
)]
pub(super) fn count(n: usize) -> JsValue {
    JsValue::from_f64(n as f64)
}
