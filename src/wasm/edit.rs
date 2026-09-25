//! Editing the grid: rows, columns and cells inserted and removed, blocks
//! copied, moved, sorted and filled, sheets moved and removed. Every one of
//! these rewrites formulas across the workbook, so each drops the dependency
//! index when it succeeds.

use super::convert::array;
use super::{Book, js};
use crate::coordinate::{CellRef, Col, Range, Row};
use crate::edit::{Axis, CopyOrigin, SortBy, SortKey, SortOptions};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
impl Book {
    /// Inserts `count` rows above row `at`, moving everything below down.
    /// `copyOrigin` - `"before"` (the row above), `"after"` (the row below)
    /// or `"none"` - says whose formatting the new rows take. Left out, it is
    /// `"before"`, as in Excel.
    ///
    /// Formulas across the whole workbook follow the cells they read, and so
    /// do merges, links, validations, tables and drawings. `at` is the number
    /// a user sees.
    #[wasm_bindgen(js_name = insertRows)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an optional string crosses the wasm boundary owned"
    )]
    pub fn insert_rows(
        &mut self,
        sheet: usize,
        at: u32,
        count: u32,
        #[wasm_bindgen(
            js_name = "copyOrigin",
            unchecked_optional_param_type = "\"before\" | \"after\" | \"none\""
        )]
        copy_origin: Option<String>,
    ) -> Result<(), JsError> {
        let row = Row::from_one_based(u64::from(at)).map_err(js)?;
        let origin = copy_origin_of(copy_origin.as_deref())?;
        let done = crate::edit::insert_rows_with(&mut self.workbook, sheet, row, count, origin);
        self.edited(done)
    }

    /// Removes `count` rows from row `at` down. A formula that read a removed
    /// cell reads `#REF!` afterwards.
    #[wasm_bindgen(js_name = removeRows)]
    pub fn remove_rows(&mut self, sheet: usize, at: u32, count: u32) -> Result<(), JsError> {
        let row = Row::from_one_based(u64::from(at)).map_err(js)?;
        let done = crate::edit::remove_rows(&mut self.workbook, sheet, row, count);
        self.edited(done)
    }

    /// Inserts `count` columns to the left of column `at`, 1-based.
    /// `copyOrigin` works as in `insertRows`, with left and right.
    #[wasm_bindgen(js_name = insertColumns)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an optional string crosses the wasm boundary owned"
    )]
    pub fn insert_columns(
        &mut self,
        sheet: usize,
        at: u32,
        count: u32,
        #[wasm_bindgen(
            js_name = "copyOrigin",
            unchecked_optional_param_type = "\"before\" | \"after\" | \"none\""
        )]
        copy_origin: Option<String>,
    ) -> Result<(), JsError> {
        let col = Col::from_one_based(u64::from(at)).map_err(js)?;
        let origin = copy_origin_of(copy_origin.as_deref())?;
        let done = crate::edit::insert_columns_with(&mut self.workbook, sheet, col, count, origin);
        self.edited(done)
    }

    /// Removes `count` columns from column `at`, 1-based.
    #[wasm_bindgen(js_name = removeColumns)]
    pub fn remove_columns(&mut self, sheet: usize, at: u32, count: u32) -> Result<(), JsError> {
        let col = Col::from_one_based(u64::from(at)).map_err(js)?;
        let done = crate::edit::remove_columns(&mut self.workbook, sheet, col, count);
        self.edited(done)
    }

    /// Inserts blank cells over a range, pushing what was there `"down"` or
    /// `"right"` - Excel's "Insert Cells", which moves part of a row rather
    /// than the whole of it. `copyOrigin` works as in `insertRows`.
    #[wasm_bindgen(js_name = insertCells)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an optional string crosses the wasm boundary owned"
    )]
    pub fn insert_cells(
        &mut self,
        sheet: usize,
        range: &str,
        #[wasm_bindgen(unchecked_param_type = "\"down\" | \"right\"")] shift: &str,
        #[wasm_bindgen(
            js_name = "copyOrigin",
            unchecked_optional_param_type = "\"before\" | \"after\" | \"none\""
        )]
        copy_origin: Option<String>,
    ) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        let origin = copy_origin_of(copy_origin.as_deref())?;
        let done = crate::edit::insert_cells_with(
            &mut self.workbook,
            sheet,
            area,
            axis_of(shift)?,
            origin,
        );
        self.edited(done)
    }

    /// Removes the cells of a range, pulling the rest back over the hole from
    /// below (`"up"`) or from the right (`"left"`).
    #[wasm_bindgen(js_name = removeCells)]
    pub fn remove_cells(
        &mut self,
        sheet: usize,
        range: &str,
        #[wasm_bindgen(unchecked_param_type = "\"up\" | \"left\"")] shift: &str,
    ) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        let done = crate::edit::remove_cells(&mut self.workbook, sheet, area, axis_of(shift)?);
        self.edited(done)
    }

    /// Copies a rectangle of cells so its top left corner lands on `to`,
    /// rewriting the formulas as Excel rewrites a copied formula: `=A1` one
    /// row down reads `=A2`, `$A$1` stays put.
    ///
    /// `toSheet` defaults to the sheet copied from. A cell the source leaves
    /// empty empties the target, the way pasting a block does.
    #[wasm_bindgen(js_name = copyRange)]
    pub fn copy_range(
        &mut self,
        sheet: usize,
        range: &str,
        to: &str,
        to_sheet: Option<usize>,
    ) -> Result<(), JsError> {
        let (area, at) = (
            Range::parse(range).map_err(js)?,
            CellRef::parse(to).map_err(js)?,
        );
        let done = crate::edit::copy_range(
            &mut self.workbook,
            sheet,
            area,
            to_sheet.unwrap_or(sheet),
            at,
        );
        self.edited(done)
    }

    /// Moves a rectangle of cells, leaving the source empty.
    ///
    /// A moved formula keeps reading the cells it read; the formulas elsewhere
    /// that read *these* cells follow them instead, as they do when Excel cuts
    /// and pastes.
    #[wasm_bindgen(js_name = moveRange)]
    pub fn move_range(
        &mut self,
        sheet: usize,
        range: &str,
        to: &str,
        to_sheet: Option<usize>,
    ) -> Result<(), JsError> {
        let (area, at) = (
            Range::parse(range).map_err(js)?,
            CellRef::parse(to).map_err(js)?,
        );
        let done = crate::edit::move_range(
            &mut self.workbook,
            sheet,
            area,
            to_sheet.unwrap_or(sheet),
            at,
        );
        self.edited(done)
    }

    /// Sorts the rows of a range, or its columns. A key is a column number of
    /// the sheet, 1-based (a row number when sorting columns), or the text of
    /// a header when `options.header` says the first line is one; a minus
    /// sorts it largest first: `[2, -4]`, `["Region", "-Amount"]`. The first
    /// key decides first.
    ///
    /// ```js
    /// book.sortRange(0, "A1:D100", ["Region", "-Amount"], { header: true });
    /// ```
    #[wasm_bindgen(js_name = sortRange)]
    pub fn sort_range(
        &mut self,
        sheet: usize,
        range: &str,
        #[wasm_bindgen(unchecked_param_type = "SortKeys")] keys: &JsValue,
        #[wasm_bindgen(unchecked_optional_param_type = "SortRangeOptions")] options: &JsValue,
    ) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        let flag = |name: &str| super::convert::field(options, name).is_some_and(|v| v.is_truthy());
        let options = SortOptions {
            header: flag("header"),
            orientation: if flag("byColumns") {
                Axis::Columns
            } else {
                Axis::Rows
            },
        };
        let keys = sort_keys_of(keys)?;
        let done = crate::edit::sort_range_with(&mut self.workbook, sheet, area, &keys, options);
        self.edited(done)
    }

    /// Sorts the data rows of a table, found by name anywhere in the book;
    /// its header and totals rows stay put. Keys name its columns, as in
    /// `sortRange`: `book.sortTable("Sales", ["-Amount"])`.
    #[wasm_bindgen(js_name = sortTable)]
    pub fn sort_table(
        &mut self,
        name: &str,
        #[wasm_bindgen(unchecked_param_type = "SortKeys")] keys: &JsValue,
    ) -> Result<(), JsError> {
        let keys = sort_keys_of(keys)?;
        let done = crate::edit::sort_table(&mut self.workbook, name, &keys);
        self.edited(done)
    }

    /// Continues what the first cells of a range start, as dragging the fill
    /// handle does: `1, 2` goes on `3, 4`, `Кв1` to `Кв2`, `Jan` to `Feb`, a
    /// date by a day; anything else repeats. `"down"` fills each column,
    /// `"right"` each row; `"up"` and `"left"` run the series back from the
    /// last cells.
    #[wasm_bindgen(js_name = fillSeries)]
    pub fn fill_series(
        &mut self,
        sheet: usize,
        range: &str,
        #[wasm_bindgen(unchecked_param_type = "\"down\" | \"right\" | \"up\" | \"left\"")]
        direction: &str,
    ) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        let fill = match direction {
            "down" | "right" => crate::edit::fill_series,
            "up" | "left" => crate::edit::fill_series_back,
            _ => {
                return Err(JsError::new(
                    r#"a fill goes "down", "right", "up" or "left""#,
                ));
            }
        };
        let done = fill(&mut self.workbook, sheet, area, axis_of(direction)?);
        self.edited(done)
    }

    /// Copies the first row of a range into the rows below it, as Ctrl+D.
    #[wasm_bindgen(js_name = fillDown)]
    pub fn fill_down(&mut self, sheet: usize, range: &str) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        let done = crate::edit::fill(&mut self.workbook, sheet, area, Axis::Rows);
        self.edited(done)
    }

    /// Copies the first column of a range into the columns to its right, as
    /// Ctrl+R.
    #[wasm_bindgen(js_name = fillRight)]
    pub fn fill_right(&mut self, sheet: usize, range: &str) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        let done = crate::edit::fill(&mut self.workbook, sheet, area, Axis::Columns);
        self.edited(done)
    }

    /// Moves a sheet along the tab bar. Formulas are untouched - a sheet is
    /// named, not numbered - but every index moves, this one included, so read
    /// `sheetIndex` again afterwards.
    #[wasm_bindgen(js_name = moveSheet)]
    pub fn move_sheet(&mut self, from: usize, to: usize) -> Result<(), JsError> {
        crate::edit::move_sheet(&mut self.workbook, from, to).map_err(js)
    }

    /// Removes a sheet. References to it from anywhere in the workbook become
    /// `#REF!`, the way Excel writes them, and the sheets after it shift down
    /// by one index.
    #[wasm_bindgen(js_name = removeSheet)]
    pub fn remove_sheet(&mut self, sheet: usize) -> Result<(), JsError> {
        let done = crate::edit::remove_sheet(&mut self.workbook, sheet);
        self.edited(done)
    }
}

impl Book {
    /// The end of every edit: its error for JS, or the dependency index
    /// dropped, since every address in it may have moved.
    fn edited(&mut self, done: crate::error::Result<()>) -> Result<(), JsError> {
        done.map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }
}

/// Which way a shift or a fill runs.
fn axis_of(direction: &str) -> Result<Axis, JsError> {
    match direction {
        "down" | "up" => Ok(Axis::Rows),
        "right" | "left" => Ok(Axis::Columns),
        _ => Err(JsError::new(
            r#"a shift is "down", "up", "right" or "left""#,
        )),
    }
}

/// Reads the `copyOrigin` argument of the inserts.
fn copy_origin_of(name: Option<&str>) -> Result<CopyOrigin, JsError> {
    match name {
        Some("none") => Ok(CopyOrigin::Blank),
        None | Some("before") => Ok(CopyOrigin::Before),
        Some("after") => Ok(CopyOrigin::After),
        Some(other) => Err(JsError::new(&format!(
            "copyOrigin must be \"before\", \"after\" or \"none\", not {other:?}"
        ))),
    }
}

/// Reads the keys of `sortRange`: a number is a 1-based line of the sheet, a
/// string a header; a minus in front sorts largest first.
fn sort_keys_of(keys: &JsValue) -> Result<Vec<SortKey>, JsError> {
    array(keys, "sort keys")?
        .iter()
        .map(|key| {
            if let Some(n) = key.as_f64() {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a line number of the sheet, saturated if it is not one"
                )]
                let line = (n.abs() as u32)
                    .checked_sub(1)
                    .ok_or_else(|| JsError::new("a sort key counts from 1"))?;
                return Ok(SortKey {
                    by: SortBy::Line(line),
                    descending: n < 0.0,
                });
            }
            let text = key
                .as_string()
                .ok_or_else(|| JsError::new("a sort key is a number or a header"))?;
            Ok(match text.strip_prefix('-') {
                Some(name) => SortKey::header(name).descending(),
                None => SortKey::header(text),
            })
        })
        .collect()
}
