//! Cells: values in and out, one at a time, a row at a time, or a rectangle
//! at a time.
//!
//! Every call that names a cell comes twice: by address (`get(0, "B4")`) and
//! by the 1-based numbers `ROW()` and `COLUMN()` return (`getAt(0, 4, 2)`),
//! which saves formatting an address only to parse it again. Both go through
//! one private body.

use super::convert::{
    area_at, array, at_index, cell_to_js, js_to_cell, object, offset, row_at, rows_of,
};
use super::{Book, js};
use crate::coordinate::{CellRef, Col, Range};
use crate::model::CellValue;
use crate::style::Style;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
impl Book {
    /// The value at `address` on sheet `sheet`, as a JS number, string, boolean
    /// or `null`. A formula cell gives its cached result, not its text.
    #[wasm_bindgen(unchecked_return_type = "CellValue")]
    pub fn get(&self, sheet: usize, address: &str) -> Result<JsValue, JsError> {
        self.value_at(sheet, CellRef::parse(address).map_err(js)?)
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = getAt, unchecked_return_type = "CellValue")]
    pub fn get_at(&self, sheet: usize, row: u32, column: u32) -> Result<JsValue, JsError> {
        self.value_at(sheet, at_index(row, column)?)
    }

    /// Writes a value: a number, boolean, or string. A string starting with `=`
    /// becomes a formula.
    pub fn set(
        &mut self,
        sheet: usize,
        address: &str,
        #[wasm_bindgen(unchecked_param_type = "CellValue")] value: &JsValue,
    ) -> Result<(), JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        self.store(sheet, vec![(at, js_to_cell(value)?)])
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = setAt)]
    pub fn set_at(
        &mut self,
        sheet: usize,
        row: u32,
        column: u32,
        #[wasm_bindgen(unchecked_param_type = "CellValue")] value: &JsValue,
    ) -> Result<(), JsError> {
        let at = at_index(row, column)?;
        self.store(sheet, vec![(at, js_to_cell(value)?)])
    }

    /// Empties a cell, style and all.
    pub fn clear(&mut self, sheet: usize, address: &str) -> Result<(), JsError> {
        self.erase(sheet, CellRef::parse(address).map_err(js)?)
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = clearAt)]
    pub fn clear_at(&mut self, sheet: usize, row: u32, column: u32) -> Result<(), JsError> {
        self.erase(sheet, at_index(row, column)?)
    }

    /// The formula text of a cell without its leading `=`, or `undefined` when
    /// the cell holds a plain value.
    #[wasm_bindgen(js_name = getFormula)]
    #[must_use]
    pub fn get_formula(&self, sheet: usize, address: &str) -> Option<String> {
        self.formula_of(sheet, CellRef::parse(address).ok()?)
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = getFormulaAt)]
    #[must_use]
    pub fn get_formula_at(&self, sheet: usize, row: u32, column: u32) -> Option<String> {
        self.formula_of(sheet, at_index(row, column).ok()?)
    }

    /// The cell as a spreadsheet would display it: the value run through its
    /// number format, so `0.256` under `0.0%` reads `25.6%`.
    #[wasm_bindgen(js_name = getFormatted)]
    pub fn get_formatted(&self, sheet: usize, address: &str) -> Result<String, JsError> {
        self.formatted(sheet, CellRef::parse(address).map_err(js)?)
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = getFormattedAt)]
    pub fn get_formatted_at(&self, sheet: usize, row: u32, column: u32) -> Result<String, JsError> {
        self.formatted(sheet, at_index(row, column)?)
    }

    /// Whether a cell is bold.
    ///
    /// The one flag, without building the rest of the style: a parser that
    /// looks for a header row asks this of every cell, and `cellStyle` would
    /// make it pay for the font, fill and four borders it never reads.
    #[wasm_bindgen(js_name = cellBold)]
    pub fn cell_bold(&self, sheet: usize, address: &str) -> Result<bool, JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        Ok(self.style_at(sheet, at)?.is_some_and(|s| s.font.bold))
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = cellBoldAt)]
    pub fn cell_bold_at(&self, sheet: usize, row: u32, column: u32) -> Result<bool, JsError> {
        let at = at_index(row, column)?;
        Ok(self.style_at(sheet, at)?.is_some_and(|s| s.font.bold))
    }

    /// The indent of a cell, in Excel's indent steps (0 when it has none).
    ///
    /// Indent lives on the cell's style, and a spreadsheet only honours it for
    /// left, right and distributed alignment - but the number is returned as
    /// the file states it, whatever the alignment.
    #[wasm_bindgen(js_name = cellIndent)]
    pub fn cell_indent(&self, sheet: usize, address: &str) -> Result<u32, JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        Ok(self.style_at(sheet, at)?.map_or(0, |s| s.alignment.indent))
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = cellIndentAt)]
    pub fn cell_indent_at(&self, sheet: usize, row: u32, column: u32) -> Result<u32, JsError> {
        let at = at_index(row, column)?;
        Ok(self.style_at(sheet, at)?.map_or(0, |s| s.alignment.indent))
    }

    /// A whole row in one crossing of the boundary: values, displayed text,
    /// bold and indent, plus whether the row is hidden.
    ///
    /// The arrays run from column 1 to the last column this row has a cell in,
    /// so a row of ten cells costs one call rather than forty.
    ///
    /// `formatted` is what the number format makes of each value, and building
    /// it allocates a string per cell - which a caller reading values does not
    /// want to pay half a sheet over. Pass `false` and the field comes back
    /// `null`; `getFormattedAt` still answers for the cells that need it.
    #[wasm_bindgen(js_name = getRowAt, unchecked_return_type = "SheetRow")]
    pub fn get_row_at(
        &self,
        sheet: usize,
        row: u32,
        formatted: Option<bool>,
    ) -> Result<JsValue, JsError> {
        let ws = self.sheet_of(sheet)?;
        let at = row_at(row)?;
        let width = ws
            .row_cells(at)
            .last()
            .map_or(0, |(col, _)| col.index() + 1);
        let values = js_sys::Array::new();
        let shown = js_sys::Array::new();
        let mut bold = Vec::with_capacity(width as usize);
        let mut indent = Vec::with_capacity(width as usize);
        let wanted = formatted.unwrap_or(true);
        for cell_at in (0..width)
            .filter_map(Col::new)
            .map(|col| CellRef::new(col, at))
        {
            let cell = ws.get(cell_at);
            values.push(&cell.map_or(JsValue::NULL, |cell| cell_to_js(&cell.value)));
            if wanted {
                shown.push(&JsValue::from_str(&self.workbook.formatted(sheet, cell_at)));
            }
            let style = cell.and_then(|cell| self.workbook.styles.get(cell.style));
            bold.push(u8::from(style.is_some_and(|s| s.font.bold)));
            indent.push(style.map_or(0, |s| s.alignment.indent));
        }
        Ok(object(&[
            ("values", values.into()),
            (
                "formatted",
                if wanted { shown.into() } else { JsValue::NULL },
            ),
            ("bold", js_sys::Uint8Array::from(&bold[..]).into()),
            ("indent", js_sys::Uint32Array::from(&indent[..]).into()),
            (
                "hidden",
                JsValue::from_bool(ws.rows.get(&at).is_some_and(|props| props.hidden)),
            ),
        ]))
    }

    /// A rectangle of cells, row by row. One call across the boundary instead
    /// of one per cell, which is what makes reading a sheet worth doing.
    #[wasm_bindgen(js_name = getRange, unchecked_return_type = "CellGrid")]
    pub fn get_range(&self, sheet: usize, range: &str) -> Result<JsValue, JsError> {
        self.grid(sheet, Range::parse(range).map_err(js)?)
    }

    /// The same by numbers: the top-left cell, then how many rows and columns
    /// to take. `getRangeAt(0, 1, 1, 3, 2)` is `A1:B3`.
    #[wasm_bindgen(js_name = getRangeAt, unchecked_return_type = "CellGrid")]
    pub fn get_range_at(
        &self,
        sheet: usize,
        row: u32,
        column: u32,
        rows: u32,
        columns: u32,
    ) -> Result<JsValue, JsError> {
        self.grid(sheet, area_at(row, column, rows, columns)?)
    }

    /// Writes a rectangle of values with `at` as its top-left corner. Rows may
    /// differ in length; a `null` leaves its cell empty.
    #[wasm_bindgen(js_name = setRange)]
    pub fn set_range(
        &mut self,
        sheet: usize,
        at: &str,
        #[wasm_bindgen(unchecked_param_type = "CellGrid")] values: &JsValue,
    ) -> Result<(), JsError> {
        self.write_grid(sheet, CellRef::parse(at).map_err(js)?, values)
    }

    /// The same with the corner given as 1-based row and column.
    #[wasm_bindgen(js_name = setRangeAt)]
    pub fn set_range_at(
        &mut self,
        sheet: usize,
        row: u32,
        column: u32,
        #[wasm_bindgen(unchecked_param_type = "CellGrid")] values: &JsValue,
    ) -> Result<(), JsError> {
        self.write_grid(sheet, at_index(row, column)?, values)
    }
}

impl Book {
    fn value_at(&self, sheet: usize, at: CellRef) -> Result<JsValue, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .get(at)
            .map_or(JsValue::NULL, |cell| cell_to_js(&cell.value)))
    }

    fn formula_of(&self, sheet: usize, at: CellRef) -> Option<String> {
        match &self.workbook.sheet(sheet)?.get(at)?.value {
            CellValue::Formula { formula, .. } => Some(formula.clone()),
            _ => None,
        }
    }

    fn formatted(&self, sheet: usize, at: CellRef) -> Result<String, JsError> {
        self.sheet_of(sheet)?;
        Ok(self.workbook.formatted(sheet, at))
    }

    /// The style a cell carries, or `None` for a cell the file says nothing
    /// about.
    pub(super) fn style_at(&self, sheet: usize, at: CellRef) -> Result<Option<&Style>, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .get(at)
            .and_then(|cell| self.workbook.styles.get(cell.style)))
    }

    fn grid(&self, sheet: usize, area: Range) -> Result<JsValue, JsError> {
        let ws = self.sheet_of(sheet)?;
        let rows = js_sys::Array::new();
        for line in rows_of(area) {
            let cells: js_sys::Array = line
                .map(|at| {
                    ws.get(at)
                        .map_or(JsValue::NULL, |cell| cell_to_js(&cell.value))
                })
                .collect();
            rows.push(&cells);
        }
        Ok(rows.into())
    }

    fn write_grid(
        &mut self,
        sheet: usize,
        start: CellRef,
        values: &JsValue,
    ) -> Result<(), JsError> {
        self.sheet_of(sheet)?;
        // Parsed first, written second: a bad value halfway through should not
        // leave the sheet half updated.
        let mut writes = Vec::new();
        for (r, row) in array(values, "values")?.iter().enumerate() {
            for (c, value) in array(&row, "every row")?.iter().enumerate() {
                writes.push((offset(start, r, c)?, js_to_cell(&value)?));
            }
        }
        self.store(sheet, writes)
    }

    /// Writes parsed values, telling the dependency index about every cell
    /// that gained or lost a formula - a value edit leaves it valid.
    fn store(&mut self, sheet: usize, writes: Vec<(CellRef, CellValue)>) -> Result<(), JsError> {
        let ws = self.sheet_mut(sheet)?;
        let is_formula =
            |value: Option<&CellValue>| matches!(value, Some(CellValue::Formula { .. }));
        let mut formulas = Vec::new();
        for (at, value) in writes {
            if is_formula(Some(&value)) || is_formula(ws.get(at).map(|c| &c.value)) {
                formulas.push(at);
            }
            ws.set(at, value);
        }
        self.note(sheet, &formulas);
        Ok(())
    }

    fn erase(&mut self, sheet: usize, at: CellRef) -> Result<(), JsError> {
        self.sheet_mut(sheet)?.remove(at);
        self.note(sheet, &[at]);
        Ok(())
    }
}
