//! The sheet around the cells: its extent, merges, row and column geometry,
//! how it is shown, and how it and the workbook are locked.

#[cfg(feature = "write")]
use super::convert::at_index;
use super::convert::{col_at, object, opt_str, row_at};
use super::{Book, js};
use crate::coordinate::Col;
#[cfg(feature = "write")]
use crate::coordinate::Range;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
impl Book {
    /// The used range of a sheet as `A1:D9`, or `undefined` if it holds
    /// nothing. This is the rectangle to walk; the sheet itself is sparse.
    #[wasm_bindgen(js_name = usedRange)]
    #[must_use]
    pub fn used_range(&self, sheet: usize) -> Option<String> {
        self.workbook
            .sheet(sheet)?
            .dimension()
            .map(|r| r.to_string())
    }

    /// The used range of a sheet as `"A1:D20"` without walking the rows;
    /// `undefined` for an empty sheet.
    ///
    /// Rows are exact, columns an upper bound: removing a cell never narrows
    /// them. `usedRange` answers the same until a cell in an edge column is
    /// removed, and walks the rows after that to give the exact rectangle.
    #[wasm_bindgen(js_name = usedRangeHint)]
    pub fn used_range_hint(&self, sheet: usize) -> Result<Option<String>, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .dimension_hint()
            .map(|r| r.to_string()))
    }

    /// The merged areas of a sheet, as `"A1:C1"` strings in the order the file
    /// lists them.
    #[wasm_bindgen(js_name = mergedRanges)]
    pub fn merged_ranges(&self, sheet: usize) -> Result<Vec<String>, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .merges
            .iter()
            .map(ToString::to_string)
            .collect())
    }

    /// The merged areas of a sheet as numbers: four per area, `[r1, c1, r2,
    /// c2]`, rows and columns 1-based.
    ///
    /// One typed array instead of a JS string per area, which is what a sheet
    /// carrying three hundred thousand of them needs.
    #[wasm_bindgen(js_name = mergedRangesAt)]
    pub fn merged_ranges_at(&self, sheet: usize) -> Result<js_sys::Uint32Array, JsError> {
        let out: Vec<u32> = self
            .sheet_of(sheet)?
            .merges
            .iter()
            .flat_map(|area| {
                [
                    area.start.row.one_based(),
                    area.start.col.one_based(),
                    area.end.row.one_based(),
                    area.end.col.one_based(),
                ]
            })
            .collect();
        Ok(js_sys::Uint32Array::from(&out[..]))
    }

    /// Whether a sheet has a tab: `"visible"`, `"hidden"` or `"veryHidden"`.
    ///
    /// `sheetNames` lists every sheet, hidden ones included, because that is
    /// what the index of every other call counts.
    #[wasm_bindgen(js_name = sheetVisibility)]
    pub fn sheet_visibility(&self, sheet: usize) -> Result<String, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .visibility
            .as_str()
            .unwrap_or("visible")
            .to_owned())
    }

    /// Whether a row is hidden. `row` is the number a user sees.
    #[wasm_bindgen(js_name = rowHidden)]
    pub fn row_hidden(&self, sheet: usize, row: u32) -> Result<bool, JsError> {
        let at = row_at(row)?;
        Ok(self
            .sheet_of(sheet)?
            .rows
            .get(&at)
            .is_some_and(|props| props.hidden))
    }

    /// How deeply a row is grouped: 0 when it is not, 1 for the outermost
    /// group, and so on. `row` is the number a user sees.
    #[wasm_bindgen(js_name = rowLevel)]
    pub fn row_level(&self, sheet: usize, row: u32) -> Result<u8, JsError> {
        let row = row_at(row)?;
        Ok(self.sheet_of(sheet)?.row_outline_level(row))
    }

    /// The same for a column, named by its letters: `columnLevel(0, "C")`.
    #[wasm_bindgen(js_name = columnLevel)]
    pub fn column_level(&self, sheet: usize, column: &str) -> Result<u8, JsError> {
        let col = Col::from_letters(column).map_err(js)?;
        Ok(self.sheet_of(sheet)?.column_outline_level(col))
    }

    /// The width of a column in characters, or `undefined` when the sheet
    /// leaves it to the default. `column` is 1-based.
    #[wasm_bindgen(js_name = columnWidth)]
    pub fn column_width(&self, sheet: usize, column: u32) -> Result<Option<f64>, JsError> {
        let col = col_at(column)?;
        Ok(self.sheet_of(sheet)?.column_width(col))
    }

    /// The height of a row in points, or `undefined` when the sheet leaves it
    /// to the default. `row` is the number a user sees.
    #[wasm_bindgen(js_name = rowHeight)]
    pub fn row_height(&self, sheet: usize, row: u32) -> Result<Option<f64>, JsError> {
        let at = row_at(row)?;
        Ok(self.sheet_of(sheet)?.row_height(at))
    }

    /// How the sheet is frozen and shown.
    #[wasm_bindgen(js_name = sheetView, unchecked_return_type = "SheetViewInfo")]
    pub fn sheet_view(&self, sheet: usize) -> Result<JsValue, JsError> {
        let view = &self.sheet_of(sheet)?.view;
        let frozen = view
            .pane
            .as_ref()
            .filter(|pane| pane.state != crate::model::PaneState::Split);
        Ok(object(&[
            (
                "frozenRows",
                JsValue::from_f64(frozen.map_or(0.0, |pane| f64::from(pane.y_split))),
            ),
            (
                "frozenColumns",
                JsValue::from_f64(frozen.map_or(0.0, |pane| f64::from(pane.x_split))),
            ),
            (
                "zoom",
                view.zoom_scale
                    .map_or(JsValue::NULL, |z| JsValue::from_f64(f64::from(z))),
            ),
            ("showGridLines", JsValue::from_bool(view.show_grid_lines)),
            (
                "showRowColHeaders",
                JsValue::from_bool(view.show_row_col_headers),
            ),
            ("rightToLeft", JsValue::from_bool(view.right_to_left)),
            (
                "topLeftCell",
                opt_str(view.top_left_cell.map(|at| at.to_string()).as_deref()),
            ),
        ]))
    }

    /// Whether a sheet is locked, and whether the lock has a password.
    #[wasm_bindgen(js_name = sheetProtection, unchecked_return_type = "ProtectionInfo")]
    pub fn sheet_protection(&self, sheet: usize) -> Result<JsValue, JsError> {
        let protection = &self.sheet_of(sheet)?.protection;
        Ok(lock(
            protection.sheet.unwrap_or(false),
            protection.password.is_some(),
        ))
    }

    /// Whether that password unlocks the sheet. `false` for a sheet with no
    /// password on it, and for a hash this crate cannot check.
    #[wasm_bindgen(js_name = verifySheetPassword)]
    pub fn verify_sheet_password(&self, sheet: usize, password: &str) -> Result<bool, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .protection
            .password
            .as_ref()
            .is_some_and(|hash| hash.verify(password)))
    }

    /// Whether the workbook's structure is locked, and whether behind a
    /// password.
    #[wasm_bindgen(js_name = workbookProtection, unchecked_return_type = "ProtectionInfo")]
    #[must_use]
    pub fn workbook_protection(&self) -> JsValue {
        let protection = &self.workbook.protection;
        lock(
            protection.lock_structure.unwrap_or(false),
            protection.workbook_password.is_some(),
        )
    }
}

#[cfg(feature = "write")]
#[wasm_bindgen]
impl Book {
    /// Merges a block of cells: `merge(0, "A1:C1")`.
    pub fn merge(&mut self, sheet: usize, range: &str) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        let merges = &mut self.sheet_mut(sheet)?.merges;
        if !merges.contains(&area) {
            merges.push(area);
        }
        Ok(())
    }

    /// Takes a merge back out. Answers whether there was one.
    pub fn unmerge(&mut self, sheet: usize, range: &str) -> Result<bool, JsError> {
        let area = Range::parse(range).map_err(js)?;
        let merges = &mut self.sheet_mut(sheet)?.merges;
        let before = merges.len();
        merges.retain(|m| *m != area);
        Ok(merges.len() != before)
    }

    /// Gives a sheet a tab or takes it away: `"visible"`, `"hidden"` or
    /// `"veryHidden"`.
    ///
    /// A very hidden sheet is not in the dialog Excel offers for unhiding one.
    /// Either way the sheet keeps its index, which is what every other call
    /// counts.
    #[wasm_bindgen(js_name = setSheetVisibility)]
    pub fn set_sheet_visibility(&mut self, sheet: usize, state: &str) -> Result<(), JsError> {
        if !matches!(state, "visible" | "hidden" | "veryHidden") {
            return Err(JsError::new(
                r#"visibility is "visible", "hidden" or "veryHidden""#,
            ));
        }
        self.sheet_mut(sheet)?.visibility = crate::model::SheetVisibility::parse(state);
        Ok(())
    }

    /// Sets the width of a column in characters of the default font, or gives
    /// it back to the sheet default with `undefined`. `column` is 1-based.
    ///
    /// A run of columns that covered this one is split around it, so the
    /// neighbours keep the width they had.
    #[wasm_bindgen(js_name = setColumnWidth)]
    pub fn set_column_width(
        &mut self,
        sheet: usize,
        column: u32,
        width: Option<f64>,
    ) -> Result<(), JsError> {
        let col = col_at(column)?;
        self.sheet_mut(sheet)?.set_column_width(col, width);
        Ok(())
    }

    /// Sets the height of a row in points, or gives it back to the sheet
    /// default with `undefined`. `row` is the number a user sees.
    #[wasm_bindgen(js_name = setRowHeight)]
    pub fn set_row_height(
        &mut self,
        sheet: usize,
        row: u32,
        height: Option<f64>,
    ) -> Result<(), JsError> {
        let at = row_at(row)?;
        self.sheet_mut(sheet)?.set_row_height(at, height);
        Ok(())
    }

    /// Hides a column, or shows it again. `column` is 1-based.
    #[wasm_bindgen(js_name = setColumnHidden)]
    pub fn set_column_hidden(
        &mut self,
        sheet: usize,
        column: u32,
        hidden: bool,
    ) -> Result<(), JsError> {
        let col = col_at(column)?;
        self.sheet_mut(sheet)?.set_column_hidden(col, hidden);
        Ok(())
    }

    /// Hides a row, or shows it again.
    #[wasm_bindgen(js_name = setRowHidden)]
    pub fn set_row_hidden(&mut self, sheet: usize, row: u32, hidden: bool) -> Result<(), JsError> {
        let at = row_at(row)?;
        self.sheet_mut(sheet)?.set_row_hidden(at, hidden);
        Ok(())
    }

    /// Freezes the first `rows` rows and `columns` columns, so they stay put
    /// while the rest scrolls. `freezePanes(0, 1, 0)` pins the header row;
    /// `freezePanes(0, 0, 0)` unfreezes.
    #[wasm_bindgen(js_name = freezePanes)]
    pub fn freeze_panes(&mut self, sheet: usize, rows: u32, columns: u32) -> Result<(), JsError> {
        use crate::model::{Pane, PanePosition, PaneState};
        let pane = if rows == 0 && columns == 0 {
            None
        } else {
            Some(Pane {
                x_split: columns,
                y_split: rows,
                top_left_cell: Some(at_index(rows + 1, columns + 1)?),
                active_pane: match (rows, columns) {
                    (0, _) => PanePosition::TopRight,
                    (_, 0) => PanePosition::BottomLeft,
                    _ => PanePosition::BottomRight,
                },
                state: PaneState::Frozen,
            })
        };
        self.sheet_mut(sheet)?.view.pane = pane;
        Ok(())
    }

    /// The zoom a reader opens the sheet at, in percent; `undefined` gives it
    /// back to 100.
    #[wasm_bindgen(js_name = setZoom)]
    pub fn set_zoom(&mut self, sheet: usize, percent: Option<u32>) -> Result<(), JsError> {
        if percent.is_some_and(|z| !(10..=400).contains(&z)) {
            return Err(JsError::new("a zoom is between 10 and 400 percent"));
        }
        self.sheet_mut(sheet)?.view.zoom_scale = percent;
        Ok(())
    }

    /// Shows or hides the grid lines, and the row and column headings with
    /// `headers`.
    #[wasm_bindgen(js_name = setShowGridLines)]
    pub fn set_show_grid_lines(
        &mut self,
        sheet: usize,
        show: bool,
        headers: Option<bool>,
    ) -> Result<(), JsError> {
        let view = &mut self.sheet_mut(sheet)?.view;
        view.show_grid_lines = show;
        if let Some(headers) = headers {
            view.show_row_col_headers = headers;
        }
        Ok(())
    }

    /// Locks a sheet against editing, optionally behind a password.
    ///
    /// This is the lock Excel offers under "Protect Sheet": it stops a user
    /// editing, it does not encrypt anything. The password is stored as Excel
    /// stores it - SHA-512 over a random salt, 100,000 spins - and
    /// `verifySheetPassword` checks it back.
    #[wasm_bindgen(js_name = protectSheet)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an optional string crosses the wasm boundary owned"
    )]
    pub fn protect_sheet(&mut self, sheet: usize, password: Option<String>) -> Result<(), JsError> {
        let hash = hash(password.as_deref())?;
        let protection = &mut self.sheet_mut(sheet)?.protection;
        protection.sheet = Some(true);
        protection.password = hash;
        Ok(())
    }

    /// Unlocks a sheet, password and all.
    #[wasm_bindgen(js_name = unprotectSheet)]
    pub fn unprotect_sheet(&mut self, sheet: usize) -> Result<(), JsError> {
        self.sheet_mut(sheet)?.protection = crate::model::protection::SheetProtection::default();
        Ok(())
    }

    /// Locks the structure of the workbook: which sheets there are, and their
    /// order. `windows` locks the window layout as well.
    #[wasm_bindgen(js_name = protectWorkbook)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an optional string crosses the wasm boundary owned"
    )]
    pub fn protect_workbook(
        &mut self,
        password: Option<String>,
        windows: Option<bool>,
    ) -> Result<(), JsError> {
        let protection = &mut self.workbook.protection;
        protection.workbook_password = hash(password.as_deref())?;
        protection.lock_structure = Some(true);
        protection.lock_windows = windows;
        Ok(())
    }

    /// Unlocks the workbook.
    #[wasm_bindgen(js_name = unprotectWorkbook)]
    pub fn unprotect_workbook(&mut self) {
        self.workbook.protection = crate::model::protection::WorkbookProtection::default();
    }
}

/// `ProtectionInfo`: whether something is locked, and behind a password.
fn lock(locked: bool, has_password: bool) -> JsValue {
    object(&[
        ("locked", JsValue::from_bool(locked)),
        ("hasPassword", JsValue::from_bool(has_password)),
    ])
}

/// A password hashed the way Excel hashes it, if one was given.
#[cfg(feature = "write")]
fn hash(password: Option<&str>) -> Result<Option<crate::model::protection::PasswordHash>, JsError> {
    password
        .map(|word| {
            crate::model::protection::PasswordHash::new(word)
                .ok_or_else(|| JsError::new("the password could not be hashed"))
        })
        .transpose()
}
