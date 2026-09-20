//! `WebAssembly` bindings: a thin JS-facing wrapper over [`Spreadsheet`].
//!
//! Only what a browser or Node caller needs - read a workbook from bytes, look
//! at and set cells, evaluate a formula, write bytes back. Everything else stays
//! in the Rust API; adding a binding is cheaper than maintaining a mirror of it.

// Every fallible binding fails one way: the message of the crate error, or a
// bad address / missing sheet. A `# Errors` section per method would repeat it.
#![allow(clippy::missing_errors_doc)]

use crate::coordinate::{CellRef, Col, Range, Row};
#[cfg(feature = "formulas")]
use crate::error::CellError;
#[cfg(feature = "formulas")]
use crate::formula::CustomFunctions;
#[cfg(feature = "formulas")]
use crate::formula::eval::{Dependencies, Engine, Origin};
#[cfg(feature = "formulas")]
use crate::formula::value::Value;
use crate::model::{CellValue, Spreadsheet, Worksheet};
#[cfg(feature = "write")]
use crate::writer::csv::write_csv_to;
#[cfg(feature = "write")]
use crate::writer::html::{HtmlOptions, write_html_to};
#[cfg(feature = "write")]
use crate::writer::ods::write_ods_to;
#[cfg(feature = "write")]
use crate::writer::xls::write_xls_to;
#[cfg(feature = "write")]
use crate::writer::xlsx::write_xlsx_to;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

/// The TypeScript names for what crosses the boundary as a `JsValue`.
///
/// `wasm-bindgen` would otherwise type these `any`, which is no help to a
/// caller: a cell is one of four things and nothing else.
#[wasm_bindgen(typescript_custom_section)]
const TYPES: &'static str = r#"
/** What a cell can hold on the JS side. An error cell reads as its code, `#DIV/0!`. */
export type CellValue = number | string | boolean | null;
/** A rectangle of cells, row by row, as `getRange` returns it. */
export type CellGrid = CellValue[][];
/**
 * One row with its styling, as `getRowAt` returns it. `formatted` is `null`
 * when the call asked for values alone. `bold` is one byte per cell, 0 or 1.
 */
export interface SheetRow {
  values: CellValue[];
  formatted: string[] | null;
  bold: Uint8Array;
  indent: Uint32Array;
  hidden: boolean;
}
/** Where a drawing object sits, in the 1-based numbers a user sees. */
export interface ObjectAnchor {
  kind: "twoCell" | "oneCell" | "absolute";
  row: number | null;
  column: number | null;
}
/** How a sheet is frozen and shown, as `sheetView` returns it. */
export interface SheetViewInfo {
  frozenRows: number;
  frozenColumns: number;
  zoom: number | null;
  showGridLines: boolean;
  showRowColHeaders: boolean;
  rightToLeft: boolean;
  topLeftCell: string | null;
}
/** A name that stands for a formula, as `definedNames` returns it. */
export interface WorkbookName {
  name: string;
  formula: string;
  sheet: number | null;
  hidden: boolean;
}
/** A rule limiting what a cell accepts, as `dataValidations` returns it. */
export interface SheetValidation {
  sqref: string[];
  type: string;
  operator: string;
  formula1: string;
  formula2: string;
  allowBlank: boolean;
  showDropDown: boolean;
}
/** One conditional formatting block, as `conditionalFormats` returns it. */
export interface SheetConditionalFormat {
  sqref: string[];
  rules: { type: string; priority: number; operator: string | null; formulas: string[]; text: string | null }[];
}
/** The autofilter over a range, as `autoFilter` returns it. */
export interface SheetAutoFilter {
  range: string;
  columns: { colId: number; kind: "values" | "custom" | "dynamic" | "top10" | "none" }[];
}
/** A pivot report on the sheet, as `pivotTables` returns it. */
export interface SheetPivotTable {
  name: string;
  location: string | null;
  cacheId: number;
  rowFields: number;
  columnFields: number;
  valueFields: number;
}
/** How a sheet or the workbook is locked, as `sheetProtection` returns it. */
export interface ProtectionInfo { locked: boolean; hasPassword: boolean }
/** How `toCsv` writes, and how `Book.readCsv` reads. */
export interface CsvOptions {
  delimiter?: string;
  contiguous?: boolean;
  preserveEmptyFields?: boolean;
}
/** A note on a cell, as `comments` returns it. */
export interface SheetComment { address: string; author: string; text: string }
/** A link over a cell or a block of them, as `hyperlinks` returns it. */
export interface SheetHyperlink {
  range: string;
  target: string;
  external: boolean;
  display: string | null;
  tooltip: string | null;
}
/** A table over a block of cells, as `tables` returns it. */
export interface SheetTable {
  name: string;
  displayName: string;
  range: string;
  headerRowCount: number | null;
  totalsRowCount: number | null;
  columns: string[];
}
/** A chart on the sheet, as `charts` returns it. */
export interface SheetChart {
  name: string;
  title: string | null;
  kinds: string[];
  seriesCount: number;
  anchor: ObjectAnchor;
}
/** A picture on the sheet, as `images` returns it. Bytes come from `imageData`. */
export interface SheetImage {
  name: string;
  description: string;
  format: string;
  byteLength: number;
  anchor: ObjectAnchor;
}
/** A drawn shape, as `shapes` returns it. */
export interface SheetShape {
  name: string;
  description: string;
  geometry: string | null;
  text: string;
  anchor: ObjectAnchor;
}
/**
 * A colour as the file states it: `null` when the file leaves it to the
 * reader, `#AARRGGBB` when it names one outright, and `indexed:N` or
 * `theme:N` when it points into the legacy palette or the workbook theme -
 * a theme colour carries its tint as `theme:4@-0.25`.
 */
export type StyleColor = string | null;
/** One side of a cell's border. */
export interface BorderSide { style: string; color: StyleColor }
/** How a cell is painted, as `cellStyle` returns it. */
export interface CellStyle {
  numberFormat: string;
  font: {
    name: string;
    size: number;
    bold: boolean;
    italic: boolean;
    underline: string;
    strike: boolean;
    color: StyleColor;
  };
  fill: { pattern: string; foreground: StyleColor; background: StyleColor };
  borders: {
    left: BorderSide;
    right: BorderSide;
    top: BorderSide;
    bottom: BorderSide;
  };
  alignment: {
    horizontal: string | null;
    vertical: string | null;
    wrapText: boolean;
    shrinkToFit: boolean;
    indent: number;
    textRotation: number;
  };
}
"#;

/// The shapes only the writing build takes, so a read-only package does not
/// declare types for methods it does not have.
#[cfg(feature = "write")]
#[wasm_bindgen(typescript_custom_section)]
const WRITE_TYPES: &'static str = r#"
/**
 * A style to write, as `setCellStyle` takes it. Every field is optional and
 * what is left out keeps the value the cell had, so `{ font: { bold: true } }`
 * makes a cell bold without touching its number format.
 */
export interface CellStylePatch {
  numberFormat?: string;
  font?: Partial<{
    name: string;
    size: number;
    bold: boolean;
    italic: boolean;
    underline: string;
    strike: boolean;
    color: StyleColor;
  }>;
  fill?: Partial<{ pattern: string; foreground: StyleColor; background: StyleColor }>;
  borders?: Partial<{
    left: Partial<BorderSide>;
    right: Partial<BorderSide>;
    top: Partial<BorderSide>;
    bottom: Partial<BorderSide>;
  }>;
  alignment?: Partial<{
    horizontal: string | null;
    vertical: string | null;
    wrapText: boolean;
    shrinkToFit: boolean;
    indent: number;
    textRotation: number;
  }>;
}
"#;

/// A workbook.
#[wasm_bindgen]
pub struct Book {
    book: Spreadsheet,
    /// What every formula reads, built on the first incremental recalculation
    /// and kept across the edits that follow. `set` keeps it in step.
    #[cfg(feature = "formulas")]
    deps: Option<Dependencies>,
    /// Functions registered from JS, called when a formula names one that no
    /// built-in claims.
    #[cfg(feature = "formulas")]
    custom: CustomFunctions,
}

/// Wraps a JS callback so a long operation can report through it.
///
/// The callback is handed `{ stage, done, total, what, fraction }`; anything
/// it throws is dropped, because a progress report failing is not a reason to
/// fail the read or the write it was watching.
fn reporter(callback: &js_sys::Function) -> impl Fn(crate::progress::Progress<'_>) + '_ {
    move |progress| {
        let stage = match progress.stage {
            crate::progress::Stage::Reading => "reading",
            crate::progress::Stage::Writing => "writing",
            crate::progress::Stage::Recalculating => "recalculating",
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "counts of sheets, parts and formulas are far below 2^53"
        )]
        let report = object(&[
            ("stage", JsValue::from_str(stage)),
            ("done", JsValue::from_f64(progress.done as f64)),
            (
                "total",
                progress
                    .total
                    .map_or(JsValue::NULL, |t| JsValue::from_f64(t as f64)),
            ),
            ("what", JsValue::from_str(progress.what)),
            (
                "fraction",
                progress.fraction().map_or(JsValue::NULL, JsValue::from_f64),
            ),
        ]);
        let _ = callback.call1(&JsValue::NULL, &report);
    }
}

/// `JsError` from any crate error; the message is the whole report.
fn js(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

#[wasm_bindgen]
impl Book {
    /// A new workbook with one empty sheet.
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self::wrap(Spreadsheet::new())
    }

    /// Reads an xlsx package from its bytes.
    #[wasm_bindgen(js_name = fromXlsx)]
    pub fn from_xlsx(bytes: &[u8]) -> Result<Self, JsError> {
        crate::reader::xlsx::read_xlsx_from(std::io::Cursor::new(bytes))
            .map(Self::wrap)
            .map_err(js)
    }

    /// Reads a workbook of any format this crate supports, working the format
    /// out from the bytes themselves.
    ///
    /// `name` is the file name they came from, if the caller has one: it
    /// settles what the bytes cannot say (a `.csv` against a `.html`) and names
    /// the sheet of a SYLK file.
    #[wasm_bindgen(js_name = read)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an optional string crosses the wasm boundary owned"
    )]
    pub fn read(
        bytes: &[u8],
        name: Option<String>,
        max_expanded: Option<f64>,
        on_progress: Option<js_sys::Function>,
    ) -> Result<Self, JsError> {
        // The cap on how far a zipped package may expand, which stops a zip
        // bomb. A caller who knows where the file came from can raise it; the
        // number crosses as an `f64` because JS has no `u64`.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a byte count from JS is a whole positive number"
        )]
        let limit = max_expanded.map_or(crate::reader::xlsx::MAX_UNCOMPRESSED_SIZE, |n| n as u64);
        // The format is worked out from the bytes, and only xlsx reports its
        // progress: the other readers take a slice and are done.
        match on_progress {
            Some(callback) => {
                let report = reporter(&callback);
                let options = crate::progress::Options::new().reporting(&report);
                crate::reader::read_bytes_limited_with(bytes, name.as_deref(), limit, &options)
                    .map(Self::wrap)
                    .map_err(js)
            }
            None => crate::reader::read_bytes_limited(bytes, name.as_deref(), limit)
                .map(Self::wrap)
                .map_err(js),
        }
    }

    /// Sheet titles, in tab order.
    #[wasm_bindgen(js_name = sheetNames)]
    #[must_use]
    pub fn sheet_names(&self) -> Vec<String> {
        self.book
            .sheets()
            .iter()
            .map(|s| s.title().to_owned())
            .collect()
    }

    /// How many non-empty cells the workbook holds, or one sheet of it.
    #[wasm_bindgen(js_name = cellCount)]
    #[must_use]
    pub fn cell_count(&self, sheet: Option<usize>) -> usize {
        match sheet {
            Some(index) => self.book.sheet(index).map_or(0, Worksheet::len),
            None => self.book.sheets().iter().map(Worksheet::len).sum(),
        }
    }

    /// Appends a sheet and returns its index.
    #[wasm_bindgen(js_name = addSheet)]
    pub fn add_sheet(&mut self, title: &str) -> Result<usize, JsError> {
        let sheet = Worksheet::new(title).map_err(js)?;
        self.book.add_sheet(sheet).map_err(js)
    }

    /// The value at `address` on sheet `sheet`, as a JS number, string, boolean
    /// or `null`. A formula cell gives its cached result, not its text.
    #[wasm_bindgen(unchecked_return_type = "CellValue")]
    pub fn get(&self, sheet: usize, address: &str) -> Result<JsValue, JsError> {
        self.value_at(sheet, CellRef::parse(address).map_err(js)?)
    }

    /// The same by 1-based row and column, the numbers `ROW()` and `COLUMN()`
    /// return: `getAt(0, 4, 2)` is `B4`. Saves formatting an address only to
    /// parse it again.
    #[wasm_bindgen(js_name = getAt, unchecked_return_type = "CellValue")]
    pub fn get_at(&self, sheet: usize, row: u32, column: u32) -> Result<JsValue, JsError> {
        self.value_at(sheet, at_index(row, column)?)
    }

    fn value_at(&self, sheet: usize, at: CellRef) -> Result<JsValue, JsError> {
        let sheet = self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        Ok(match sheet.get(at).map(|c| &c.value) {
            None | Some(CellValue::Empty) => JsValue::NULL,
            Some(CellValue::Number(n)) => JsValue::from_f64(*n),
            Some(CellValue::Bool(b)) => JsValue::from_bool(*b),
            Some(CellValue::Error(e)) => JsValue::from_str(e.as_str()),
            Some(v @ (CellValue::Text(_) | CellValue::RichText(_))) => {
                JsValue::from_str(&v.plain_text().unwrap_or_default())
            }
            Some(CellValue::Formula { cached, .. }) => {
                cached.as_deref().map_or(JsValue::NULL, cell_to_js)
            }
        })
    }

    /// Writes a value: a number, boolean, or string. A string starting with `=`
    /// becomes a formula.
    pub fn set(
        &mut self,
        index: usize,
        address: &str,
        #[wasm_bindgen(unchecked_param_type = "CellValue")] value: &JsValue,
    ) -> Result<(), JsError> {
        self.write(index, CellRef::parse(address).map_err(js)?, value)
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
        self.write(sheet, at_index(row, column)?, value)
    }

    fn write(&mut self, index: usize, at: CellRef, value: &JsValue) -> Result<(), JsError> {
        let value = js_to_cell(value)?;

        let sheet = self
            .book
            .sheet_mut(index)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        #[cfg(feature = "formulas")]
        let formula_edit = matches!(value, CellValue::Formula { .. })
            || matches!(
                sheet.get(at).map(|c| &c.value),
                Some(CellValue::Formula { .. })
            );
        sheet.set(at, value);
        #[cfg(feature = "formulas")]
        // A value edit leaves the index valid; a formula edit does not.
        if let (Some(deps), true) = (self.deps.as_mut(), formula_edit) {
            deps.note(&self.book, index, at);
        }
        Ok(())
    }

    #[cfg(feature = "formulas")]
    /// Registers a function of your own, callable from any formula in the
    /// workbook by that name.
    ///
    /// The function is handed the arguments already computed - a range arrives
    /// as an array of arrays - and whatever it returns becomes the value. A
    /// name a built-in claims stays the built-in's: a workbook where `SUM`
    /// means something else is a workbook nobody else can read.
    ///
    /// ```js
    /// book.registerFunction("MYRATE", (base) => base * 1.5 + 0.5);
    /// book.evaluate(0, "A1", "=MYRATE(10)"); // 15.5
    /// ```
    #[cfg(feature = "formulas")]
    #[wasm_bindgen(js_name = registerFunction)]
    pub fn register_function(&mut self, name: &str, function: &js_sys::Function) {
        let function = function.clone();
        self.custom.register(name, move |args| {
            let js_args = js_sys::Array::new();
            for arg in args {
                js_args.push(&value_to_js_deep(arg));
            }
            // A function that throws is an error value rather than a panic
            // crossing the boundary: a formula may not take the process down.
            match js_sys::Reflect::apply(&function, &JsValue::NULL, &js_args) {
                Ok(value) => js_to_value(&value),
                Err(_) => Value::Error(CellError::Value),
            }
        });
        // The index remembers which names were unknown, so it is rebuilt.
        self.deps = None;
    }

    /// Forgets a function registered earlier; answers whether there was one.
    #[cfg(feature = "formulas")]
    #[wasm_bindgen(js_name = unregisterFunction)]
    pub fn unregister_function(&mut self, name: &str) -> bool {
        self.deps = None;
        self.custom.remove(name)
    }

    /// The names registered, in no particular order.
    #[cfg(feature = "formulas")]
    #[wasm_bindgen(js_name = registeredFunctions)]
    #[must_use]
    pub fn registered_functions(&self) -> Vec<String> {
        self.custom.names().map(str::to_owned).collect()
    }

    /// Evaluates a formula (with or without the leading `=`) as if it sat in
    /// `address` of `sheet`, and returns its result.
    #[cfg(feature = "formulas")]
    #[wasm_bindgen(unchecked_return_type = "CellValue")]
    pub fn evaluate(&self, sheet: usize, address: &str, formula: &str) -> Result<JsValue, JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        if self.book.sheet(sheet).is_none() {
            return Err(JsError::new("no such sheet"));
        }
        let mut engine = Engine::with_functions(&self.book, &self.custom);
        let got = engine.eval(
            Origin::new(sheet, at),
            formula.strip_prefix('=').unwrap_or(formula),
        );
        Ok(value_to_js(&got))
    }

    #[cfg(feature = "formulas")]
    /// Recomputes every formula and stores the results beside them, so that
    /// `get` and `toCsv` show this engine's answers rather than the cache the
    /// file was saved with. Returns how many formulas were computed.
    ///
    /// With no argument the whole workbook is recomputed; with a sheet index,
    /// only that sheet's formulas are - though they still read the whole
    /// workbook, as a cross-sheet reference must.
    pub fn recalculate(
        &mut self,
        sheet: Option<usize>,
        on_progress: Option<js_sys::Function>,
    ) -> usize {
        let options = crate::progress::Options::new().with_functions(&self.custom);
        match on_progress {
            Some(callback) => {
                let report = reporter(&callback);
                let options = options.reporting(&report);
                crate::formula::eval::recalculate(&mut self.book, sheet, &options)
            }
            None => crate::formula::eval::recalculate(&mut self.book, sheet, &options),
        }
    }

    #[cfg(feature = "formulas")]
    /// Recomputes the formula in one cell and stores its result. Returns
    /// whether there was a formula there.
    ///
    /// Not the same as `recalculateFrom`, which recomputes the formulas
    /// *reading* this cell and leaves the cell itself alone.
    #[wasm_bindgen(js_name = recalculateCell)]
    pub fn recalculate_cell(&mut self, sheet: usize, address: &str) -> Result<bool, JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        Ok(self.recalculate_cell_inner(sheet, at))
    }

    #[cfg(feature = "formulas")]
    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = recalculateCellAt)]
    pub fn recalculate_cell_at(
        &mut self,
        sheet: usize,
        row: u32,
        column: u32,
    ) -> Result<bool, JsError> {
        let at = at_index(row, column)?;
        Ok(self.recalculate_cell_inner(sheet, at))
    }

    #[cfg(feature = "formulas")]
    fn recalculate_cell_inner(&mut self, sheet: usize, at: CellRef) -> bool {
        let options = crate::progress::Options::new().with_functions(&self.custom);
        crate::formula::eval::recalculate_cell_with(&mut self.book, sheet, at, &options)
    }

    #[cfg(feature = "formulas")]
    /// Recomputes only what depends on one cell that changed - the formulas
    /// reading it, the formulas reading those, and so on. Returns how many were
    /// computed. This is the pass to run after `set`.
    ///
    /// For a batch of edits use `recalculateFromMany`, which answers them all
    /// in one pass.
    #[wasm_bindgen(js_name = recalculateFrom)]
    pub fn recalculate_from(&mut self, sheet: usize, address: &str) -> Result<usize, JsError> {
        self.recalculate_from_many(sheet, vec![address.to_owned()])
    }

    #[cfg(feature = "formulas")]
    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = recalculateFromAt)]
    pub fn recalculate_from_at(
        &mut self,
        sheet: usize,
        row: u32,
        column: u32,
    ) -> Result<usize, JsError> {
        let at = at_index(row, column)?;
        if self.book.sheet(sheet).is_none() {
            return Err(JsError::new("no such sheet"));
        }
        let deps = self
            .deps
            .get_or_insert_with(|| Dependencies::of(&self.book));
        Ok(deps.recalculate_from(&mut self.book, &[(sheet, at)]))
    }

    #[cfg(feature = "formulas")]
    /// The same for a batch of cells, all on sheet `sheet`, answered in one
    /// pass: editing a hundred cells costs one walk of the workbook, not a
    /// hundred.
    ///
    /// The index of what reads what is built on the first call and kept, so a
    /// run of edits parses the formulas only once. `set` keeps it in step.
    #[wasm_bindgen(js_name = recalculateFromMany)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an array of strings crosses the wasm boundary owned"
    )]
    pub fn recalculate_from_many(
        &mut self,
        sheet: usize,
        addresses: Vec<String>,
    ) -> Result<usize, JsError> {
        if self.book.sheet(sheet).is_none() {
            return Err(JsError::new("no such sheet"));
        }
        let mut changed = Vec::with_capacity(addresses.len());
        for address in &addresses {
            changed.push((sheet, CellRef::parse(address).map_err(js)?));
        }
        let deps = self
            .deps
            .get_or_insert_with(|| Dependencies::of(&self.book));
        Ok(deps.recalculate_from(&mut self.book, &changed))
    }

    /// The tab index of a sheet by name, or `undefined` when there is none.
    /// Names are compared the way Excel does, ignoring case.
    #[wasm_bindgen(js_name = sheetIndex)]
    #[must_use]
    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.book.sheet_index_by_name(name)
    }

    /// Renames a sheet.
    #[wasm_bindgen(js_name = renameSheet)]
    pub fn rename_sheet(&mut self, sheet: usize, title: &str) -> Result<(), JsError> {
        self.book
            .sheet_mut(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?
            .set_title(title)
            .map_err(js)
    }

    /// The tab a reader opens the workbook on.
    #[wasm_bindgen(js_name = activeSheet)]
    #[must_use]
    pub fn active_sheet(&self) -> usize {
        self.book.active_index()
    }

    /// Sets that tab.
    #[wasm_bindgen(js_name = setActiveSheet)]
    pub fn set_active_sheet(&mut self, sheet: usize) -> Result<(), JsError> {
        self.book.set_active(sheet).map_err(js)
    }

    /// The used range of a sheet as `A1:D9`, or `undefined` if it holds
    /// nothing. This is the rectangle to walk; the sheet itself is sparse.
    #[wasm_bindgen(js_name = usedRange)]
    #[must_use]
    pub fn used_range(&self, sheet: usize) -> Option<String> {
        self.book.sheet(sheet)?.dimension().map(|r| r.to_string())
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

    fn formula_of(&self, sheet: usize, at: CellRef) -> Option<String> {
        match &self.book.sheet(sheet)?.get(at)?.value {
            CellValue::Formula { formula, .. } => Some(formula.clone()),
            _ => None,
        }
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

    fn formatted(&self, sheet: usize, at: CellRef) -> Result<String, JsError> {
        if self.book.sheet(sheet).is_none() {
            return Err(JsError::new("no such sheet"));
        }
        Ok(self.book.formatted(sheet, at))
    }

    /// The indent of a cell, in Excel's indent steps (0 when it has none).
    ///
    /// Indent lives on the cell's style, and a spreadsheet only honours it for
    /// left, right and distributed alignment - but the number is returned as
    /// the file states it, whatever the alignment.
    #[wasm_bindgen(js_name = cellIndent)]
    pub fn cell_indent(&self, sheet: usize, address: &str) -> Result<u32, JsError> {
        self.indent_of(sheet, CellRef::parse(address).map_err(js)?)
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = cellIndentAt)]
    pub fn cell_indent_at(&self, sheet: usize, row: u32, column: u32) -> Result<u32, JsError> {
        self.indent_of(sheet, at_index(row, column)?)
    }

    fn indent_of(&self, sheet: usize, at: CellRef) -> Result<u32, JsError> {
        let ws = self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        Ok(ws
            .get(at)
            .and_then(|cell| self.book.styles.get(cell.style))
            .map_or(0, |style| style.alignment.indent))
    }

    /// The merged areas of a sheet, as `"A1:C1"` strings in the order the file
    /// lists them.
    #[wasm_bindgen(js_name = mergedRanges)]
    pub fn merged_ranges(&self, sheet: usize) -> Result<Vec<String>, JsError> {
        Ok(self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?
            .merges
            .iter()
            .map(ToString::to_string)
            .collect())
    }

    /// Whether a cell is bold.
    ///
    /// The one flag, without building the rest of the style: a parser that
    /// looks for a header row asks this of every cell, and `cellStyle` would
    /// make it pay for the font, fill and four borders it never reads.
    #[wasm_bindgen(js_name = cellBold)]
    pub fn cell_bold(&self, sheet: usize, address: &str) -> Result<bool, JsError> {
        self.bold_of(sheet, CellRef::parse(address).map_err(js)?)
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = cellBoldAt)]
    pub fn cell_bold_at(&self, sheet: usize, row: u32, column: u32) -> Result<bool, JsError> {
        self.bold_of(sheet, at_index(row, column)?)
    }

    fn bold_of(&self, sheet: usize, at: CellRef) -> Result<bool, JsError> {
        let ws = self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        Ok(ws
            .get(at)
            .and_then(|cell| self.book.styles.get(cell.style))
            .is_some_and(|style| style.font.bold))
    }

    /// Whether a sheet has a tab: `"visible"`, `"hidden"` or `"veryHidden"`.
    ///
    /// `sheetNames` lists every sheet, hidden ones included, because that is
    /// what the index of every other call counts.
    #[wasm_bindgen(js_name = sheetVisibility)]
    pub fn sheet_visibility(&self, sheet: usize) -> Result<String, JsError> {
        Ok(self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?
            .visibility
            .as_str()
            .unwrap_or("visible")
            .to_owned())
    }

    /// Whether a row is hidden. `row` is the number a user sees.
    #[wasm_bindgen(js_name = rowHidden)]
    pub fn row_hidden(&self, sheet: usize, row: u32) -> Result<bool, JsError> {
        let at = Row::from_one_based(u64::from(row)).map_err(js)?;
        Ok(self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?
            .rows
            .get(&at)
            .is_some_and(|props| props.hidden))
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
        let ws = self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        let at = Row::from_one_based(u64::from(row)).map_err(js)?;
        let width = ws
            .row_cells(at)
            .last()
            .map_or(0, |(col, _)| col.index() + 1);
        let values = js_sys::Array::new();
        let shown = js_sys::Array::new();
        let mut bold = Vec::with_capacity(width as usize);
        let mut indent = Vec::with_capacity(width as usize);
        let wanted = formatted.unwrap_or(true);
        for col in 0..width {
            let Some(col) = Col::new(col) else { continue };
            let cell_at = CellRef::new(col, at);
            let cell = ws.get(cell_at);
            values.push(&cell.map_or(JsValue::NULL, |cell| match &cell.value {
                CellValue::Formula { cached, .. } => {
                    cached.as_deref().map_or(JsValue::NULL, cell_to_js)
                }
                other => cell_to_js(other),
            }));
            if wanted {
                shown.push(&JsValue::from_str(&self.formatted(sheet, cell_at)?));
            }
            let style = cell.and_then(|cell| self.book.styles.get(cell.style));
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

    /// The merged areas of a sheet as numbers: four per area, `[r1, c1, r2,
    /// c2]`, rows and columns 1-based.
    ///
    /// One typed array instead of a JS string per area, which is what a sheet
    /// carrying three hundred thousand of them needs.
    #[wasm_bindgen(js_name = mergedRangesAt)]
    pub fn merged_ranges_at(&self, sheet: usize) -> Result<js_sys::Uint32Array, JsError> {
        let merges = &self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?
            .merges;
        let mut out = Vec::with_capacity(merges.len() * 4);
        for area in merges {
            out.extend_from_slice(&[
                area.start.row.one_based(),
                area.start.col.one_based(),
                area.end.row.one_based(),
                area.end.col.one_based(),
            ]);
        }
        Ok(js_sys::Uint32Array::from(&out[..]))
    }

    /// How deeply a row is grouped: 0 when it is not, 1 for the outermost
    /// group, and so on. `row` is the number a user sees.
    #[wasm_bindgen(js_name = rowLevel)]
    pub fn row_level(&self, sheet: usize, row: u32) -> Result<u8, JsError> {
        let row = Row::from_one_based(u64::from(row)).map_err(js)?;
        Ok(self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?
            .row_outline_level(row))
    }

    /// The same for a column, named by its letters: `columnLevel(0, "C")`.
    #[wasm_bindgen(js_name = columnLevel)]
    pub fn column_level(&self, sheet: usize, column: &str) -> Result<u8, JsError> {
        let col = Col::from_letters(column).map_err(js)?;
        Ok(self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?
            .column_outline_level(col))
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

    fn erase(&mut self, sheet: usize, at: CellRef) -> Result<(), JsError> {
        self.book
            .sheet_mut(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?
            .remove(at);
        #[cfg(feature = "formulas")]
        if let Some(deps) = self.deps.as_mut() {
            deps.note(&self.book, sheet, at);
        }
        Ok(())
    }

    /// How a cell is painted: number format, font, fill, borders and text
    /// placement, as the file states them.
    ///
    /// A cell the file says nothing about answers with Excel's defaults, which
    /// is what Excel itself shows for it.
    #[wasm_bindgen(js_name = cellStyle, unchecked_return_type = "CellStyle")]
    pub fn cell_style(&self, sheet: usize, address: &str) -> Result<JsValue, JsError> {
        self.style_of(sheet, CellRef::parse(address).map_err(js)?)
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = cellStyleAt, unchecked_return_type = "CellStyle")]
    pub fn cell_style_at(&self, sheet: usize, row: u32, column: u32) -> Result<JsValue, JsError> {
        self.style_of(sheet, at_index(row, column)?)
    }

    fn style_of(&self, sheet: usize, at: CellRef) -> Result<JsValue, JsError> {
        let ws = self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        let style = ws
            .get(at)
            .and_then(|cell| self.book.styles.get(cell.style))
            .cloned()
            .unwrap_or_default();
        Ok(style_to_js(&style))
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

    fn grid(&self, sheet: usize, area: Range) -> Result<JsValue, JsError> {
        let ws = self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        let rows = js_sys::Array::new();
        for row in area.start.row.index()..=area.end.row.index() {
            let cells = js_sys::Array::new();
            for col in area.start.col.index()..=area.end.col.index() {
                let value = Row::new(row)
                    .zip(Col::new(col))
                    .and_then(|(row, col)| ws.get(CellRef::new(col, row)))
                    .map_or(JsValue::NULL, |cell| match &cell.value {
                        CellValue::Formula { cached, .. } => {
                            cached.as_deref().map_or(JsValue::NULL, cell_to_js)
                        }
                        other => cell_to_js(other),
                    });
                cells.push(&value);
            }
            rows.push(&cells);
        }
        Ok(rows.into())
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

    fn write_grid(
        &mut self,
        sheet: usize,
        start: CellRef,
        values: &JsValue,
    ) -> Result<(), JsError> {
        if self.book.sheet(sheet).is_none() {
            return Err(JsError::new("no such sheet"));
        }
        let rows: js_sys::Array = values
            .clone()
            .dyn_into()
            .map_err(|_| JsError::new("values must be an array of rows"))?;

        // Parsed first, written second: a bad value halfway through should not
        // leave the sheet half updated.
        let mut writes = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            let row: js_sys::Array = row
                .dyn_into()
                .map_err(|_| JsError::new("every row must be an array"))?;
            for (c, value) in row.iter().enumerate() {
                let col = u32::try_from(c)
                    .ok()
                    .and_then(|c| Col::new(start.col.index() + c))
                    .ok_or_else(|| JsError::new("range runs past the last column"))?;
                let row = u32::try_from(r)
                    .ok()
                    .and_then(|r| Row::new(start.row.index() + r))
                    .ok_or_else(|| JsError::new("range runs past the last row"))?;
                writes.push((CellRef::new(col, row), js_to_cell(&value)?));
            }
        }

        #[cfg(feature = "formulas")]
        let formulas: Vec<CellRef> = writes
            .iter()
            .filter(|(_, v)| matches!(v, CellValue::Formula { .. }))
            .map(|(at, _)| *at)
            .collect();
        let ws = self
            .book
            .sheet_mut(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        for (at, value) in writes {
            ws.set(at, value);
        }
        #[cfg(feature = "formulas")]
        if let Some(deps) = self.deps.as_mut() {
            for at in formulas {
                deps.note(&self.book, sheet, at);
            }
        }
        Ok(())
    }

    #[cfg(feature = "write")]
    /// The workbook as an xlsx package.
    #[wasm_bindgen(js_name = toXlsx)]
    pub fn to_xlsx(&self, on_progress: Option<js_sys::Function>) -> Result<Vec<u8>, JsError> {
        let mut out = std::io::Cursor::new(Vec::new());
        match on_progress {
            Some(callback) => {
                let report = reporter(&callback);
                let options = crate::progress::Options::new().reporting(&report);
                crate::writer::xlsx::write_xlsx_to_with(&self.book, &mut out, &options)
            }
            None => write_xlsx_to(&self.book, &mut out),
        }
        .map_err(js)?;
        Ok(out.into_inner())
    }

    #[cfg(feature = "write")]
    /// The workbook as an `OpenDocument` spreadsheet.
    #[wasm_bindgen(js_name = toOds)]
    pub fn to_ods(&self) -> Result<Vec<u8>, JsError> {
        let mut out = std::io::Cursor::new(Vec::new());
        write_ods_to(&self.book, &mut out).map_err(js)?;
        Ok(out.into_inner())
    }

    #[cfg(feature = "write")]
    /// The workbook as a BIFF8 `.xls` file.
    ///
    /// The format cannot hold a formula as text, so every formula is written
    /// as its last computed value; call `recalculate` first if that matters.
    #[wasm_bindgen(js_name = toXls)]
    pub fn to_xls(&self) -> Result<Vec<u8>, JsError> {
        let mut out = Vec::new();
        write_xls_to(&self.book, &mut out).map_err(js)?;
        Ok(out)
    }

    #[cfg(feature = "write")]
    /// The workbook as an HTML page, or one sheet of it. `fragment` leaves out
    /// the `<html>` wrapper, for embedding in a page that already has one.
    #[wasm_bindgen(js_name = toHtml)]
    pub fn to_html(&self, sheet: Option<usize>, fragment: Option<bool>) -> Result<String, JsError> {
        let mut out = Vec::new();
        let options = HtmlOptions {
            sheet,
            fragment: fragment.unwrap_or(false),
        };
        write_html_to(&self.book, &mut out, &options).map_err(js)?;
        String::from_utf8(out).map_err(js)
    }

    /// The used range of a sheet as `"A1:D20"`, taken from what the file says
    /// rather than from walking the cells; `undefined` when it says nothing.
    ///
    /// A reader that only needs the shape of a sheet pays nothing for it;
    /// `usedRange` walks every cell and answers for a sheet built in memory
    /// too, where the file has nothing to say.
    #[wasm_bindgen(js_name = usedRangeHint)]
    pub fn used_range_hint(&self, sheet: usize) -> Result<Option<String>, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .dimension_hint()
            .map(|r| r.to_string()))
    }

    /// The width of a column in characters, or `undefined` when the sheet
    /// leaves it to the default. `column` is 1-based.
    #[wasm_bindgen(js_name = columnWidth)]
    pub fn column_width(&self, sheet: usize, column: u32) -> Result<Option<f64>, JsError> {
        let col = Col::from_one_based(u64::from(column)).map_err(js)?;
        Ok(self.sheet_of(sheet)?.column_width(col))
    }

    /// The height of a row in points, or `undefined` when the sheet leaves it
    /// to the default. `row` is the number a user sees.
    #[wasm_bindgen(js_name = rowHeight)]
    pub fn row_height(&self, sheet: usize, row: u32) -> Result<Option<f64>, JsError> {
        let at = Row::from_one_based(u64::from(row)).map_err(js)?;
        Ok(self.sheet_of(sheet)?.row_height(at))
    }

    /// The notes on a sheet, in address order.
    #[wasm_bindgen(unchecked_return_type = "SheetComment[]")]
    pub fn comments(&self, sheet: usize) -> Result<JsValue, JsError> {
        let out = js_sys::Array::new();
        for (at, comment) in &self.sheet_of(sheet)?.comments {
            out.push(&object(&[
                ("address", JsValue::from_str(&at.to_string())),
                ("author", JsValue::from_str(&comment.author)),
                ("text", JsValue::from_str(&comment.plain_text())),
            ]));
        }
        Ok(out.into())
    }

    /// The hyperlinks of a sheet. `external` tells a URL from a jump inside
    /// the workbook.
    #[wasm_bindgen(unchecked_return_type = "SheetHyperlink[]")]
    pub fn hyperlinks(&self, sheet: usize) -> Result<JsValue, JsError> {
        use crate::model::LinkTarget;
        let out = js_sys::Array::new();
        for link in &self.sheet_of(sheet)?.hyperlinks {
            let (target, external) = match &link.target {
                LinkTarget::Inside(to) => (to, false),
                LinkTarget::Outside(to) => (to, true),
            };
            out.push(&object(&[
                ("range", JsValue::from_str(&link.range.to_string())),
                ("target", JsValue::from_str(target)),
                ("external", JsValue::from_bool(external)),
                ("display", opt_str(link.display.as_deref())),
                ("tooltip", opt_str(link.tooltip.as_deref())),
            ]));
        }
        Ok(out.into())
    }

    /// The tables of a sheet: what a structured reference like `Sales[Amount]`
    /// names.
    #[wasm_bindgen(unchecked_return_type = "SheetTable[]")]
    pub fn tables(&self, sheet: usize) -> Result<JsValue, JsError> {
        let out = js_sys::Array::new();
        for table in &self.sheet_of(sheet)?.tables {
            let columns = js_sys::Array::new();
            for column in &table.columns {
                columns.push(&JsValue::from_str(&column.name));
            }
            out.push(&object(&[
                ("name", JsValue::from_str(&table.name)),
                ("displayName", JsValue::from_str(&table.display_name)),
                ("range", JsValue::from_str(&table.range.to_string())),
                ("headerRowCount", opt_count(table.header_row_count)),
                ("totalsRowCount", opt_count(table.totals_row_count)),
                ("columns", columns.into()),
            ]));
        }
        Ok(out.into())
    }

    /// The charts on a sheet. `kinds` names the plots drawn in each -
    /// `barChart`, `lineChart` - and a combination chart has more than one.
    #[wasm_bindgen(unchecked_return_type = "SheetChart[]")]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a count of series is far below 2^53"
    )]
    pub fn charts(&self, sheet: usize) -> Result<JsValue, JsError> {
        let out = js_sys::Array::new();
        for chart in &self.sheet_of(sheet)?.charts {
            let kinds = js_sys::Array::new();
            for plot in &chart.plots {
                kinds.push(&JsValue::from_str(plot.kind.element()));
            }
            let title = chart
                .title
                .as_ref()
                .and_then(|t| t.text.as_ref())
                .and_then(crate::model::chart::ChartText::shown);
            out.push(&object(&[
                ("name", JsValue::from_str(&chart.name)),
                ("title", opt_str(title)),
                ("kinds", kinds.into()),
                (
                    "seriesCount",
                    JsValue::from_f64(
                        chart.plots.iter().map(|p| p.series.len()).sum::<usize>() as f64
                    ),
                ),
                ("anchor", anchor_to_js(&chart.anchor)),
            ]));
        }
        Ok(out.into())
    }

    /// The pictures on a sheet, without their bytes: `imageData` fetches those
    /// for the one that is wanted.
    #[wasm_bindgen(unchecked_return_type = "SheetImage[]")]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a picture's byte count is far below 2^53"
    )]
    pub fn images(&self, sheet: usize) -> Result<JsValue, JsError> {
        let out = js_sys::Array::new();
        for image in &self.sheet_of(sheet)?.images {
            out.push(&object(&[
                ("name", JsValue::from_str(&image.name)),
                ("description", JsValue::from_str(&image.description)),
                ("format", JsValue::from_str(image.format.extension())),
                ("byteLength", JsValue::from_f64(image.data.len() as f64)),
                ("anchor", anchor_to_js(&image.anchor)),
            ]));
        }
        Ok(out.into())
    }

    /// The bytes of one picture, by its position in `images`.
    #[wasm_bindgen(js_name = imageData)]
    pub fn image_data(&self, sheet: usize, index: usize) -> Result<Vec<u8>, JsError> {
        self.sheet_of(sheet)?
            .images
            .get(index)
            .map(|image| image.data.clone())
            .ok_or_else(|| JsError::new("no such image"))
    }

    /// The drawn shapes of a sheet, text and all.
    #[wasm_bindgen(unchecked_return_type = "SheetShape[]")]
    pub fn shapes(&self, sheet: usize) -> Result<JsValue, JsError> {
        let out = js_sys::Array::new();
        for shape in &self.sheet_of(sheet)?.shapes {
            out.push(&object(&[
                ("name", JsValue::from_str(&shape.name)),
                ("description", JsValue::from_str(&shape.description)),
                ("geometry", opt_str(shape.geometry.as_deref())),
                ("text", JsValue::from_str(&shape.text)),
                ("anchor", anchor_to_js(&shape.anchor)),
            ]));
        }
        Ok(out.into())
    }

    #[cfg(feature = "write")]
    /// Paints a cell: a number format, a font, a fill, borders, alignment, or
    /// any part of those.
    ///
    /// The patch is laid over the style the cell has, so a field left out
    /// keeps what was there - `{ font: { bold: true } }` does not reset the
    /// number format. Equal styles share one entry in the workbook's table,
    /// so painting a column costs one style, not one per cell.
    ///
    /// ```js
    /// book.setCellStyle(0, "B2", {
    ///   numberFormat: "#,##0.00",
    ///   font: { bold: true, color: "#FF1F4E79" },
    ///   fill: { pattern: "solid", foreground: "#FFFFE699" },
    ///   alignment: { horizontal: "right" },
    /// });
    /// ```
    #[wasm_bindgen(js_name = setCellStyle)]
    pub fn set_cell_style(
        &mut self,
        sheet: usize,
        address: &str,
        #[wasm_bindgen(unchecked_param_type = "CellStylePatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        self.paint(sheet, Range::new(at, at), patch)
    }

    #[cfg(feature = "write")]
    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = setCellStyleAt)]
    pub fn set_cell_style_at(
        &mut self,
        sheet: usize,
        row: u32,
        column: u32,
        #[wasm_bindgen(unchecked_param_type = "CellStylePatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        let at = at_index(row, column)?;
        self.paint(sheet, Range::new(at, at), patch)
    }

    #[cfg(feature = "write")]
    /// The same over a rectangle: `setRangeStyle(0, "A1:D1", { font: { bold:
    /// true } })`. Cells the range covers but the sheet has no value for are
    /// created empty, which is what carries the style.
    #[wasm_bindgen(js_name = setRangeStyle)]
    pub fn set_range_style(
        &mut self,
        sheet: usize,
        range: &str,
        #[wasm_bindgen(unchecked_param_type = "CellStylePatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        self.paint(sheet, Range::parse(range).map_err(js)?, patch)
    }

    #[cfg(feature = "write")]
    /// Lays a patch over every cell of an area, interning each resulting style
    /// once.
    fn paint(&mut self, sheet: usize, area: Range, patch: &JsValue) -> Result<(), JsError> {
        if !patch.is_object() {
            return Err(JsError::new("a style patch is an object"));
        }
        let mut seen: std::collections::HashMap<crate::style::StyleId, crate::style::StyleId> =
            std::collections::HashMap::new();
        for row in area.start.row.index()..=area.end.row.index() {
            for col in area.start.col.index()..=area.end.col.index() {
                let (Some(row), Some(col)) = (Row::new(row), Col::new(col)) else {
                    continue;
                };
                let at = CellRef::new(col, row);
                let ws = self
                    .book
                    .sheet(sheet)
                    .ok_or_else(|| JsError::new("no such sheet"))?;
                let old = ws.get(at).map_or_else(Default::default, |cell| cell.style);
                let id = if let Some(&id) = seen.get(&old) {
                    id
                } else {
                    let mut style = self.book.styles.get(old).cloned().unwrap_or_default();
                    apply_style_patch(&mut style, patch)?;
                    let id = self.book.styles.intern(style);
                    seen.insert(old, id);
                    id
                };
                let ws = self
                    .book
                    .sheet_mut(sheet)
                    .ok_or_else(|| JsError::new("no such sheet"))?;
                ws.entry(at).style = id;
            }
        }
        Ok(())
    }

    #[cfg(feature = "write")]
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
        let col = Col::from_one_based(u64::from(column)).map_err(js)?;
        self.sheet_mut(sheet)?.set_column_width(col, width);
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Sets the height of a row in points, or gives it back to the sheet
    /// default with `undefined`. `row` is the number a user sees.
    #[wasm_bindgen(js_name = setRowHeight)]
    pub fn set_row_height(
        &mut self,
        sheet: usize,
        row: u32,
        height: Option<f64>,
    ) -> Result<(), JsError> {
        let at = Row::from_one_based(u64::from(row)).map_err(js)?;
        self.sheet_mut(sheet)?.set_row_height(at, height);
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Hides a column, or shows it again. `column` is 1-based.
    #[wasm_bindgen(js_name = setColumnHidden)]
    pub fn set_column_hidden(
        &mut self,
        sheet: usize,
        column: u32,
        hidden: bool,
    ) -> Result<(), JsError> {
        let col = Col::from_one_based(u64::from(column)).map_err(js)?;
        self.sheet_mut(sheet)?.set_column_hidden(col, hidden);
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Hides a row, or shows it again.
    #[wasm_bindgen(js_name = setRowHidden)]
    pub fn set_row_hidden(&mut self, sheet: usize, row: u32, hidden: bool) -> Result<(), JsError> {
        let at = Row::from_one_based(u64::from(row)).map_err(js)?;
        self.sheet_mut(sheet)?.set_row_hidden(at, hidden);
        Ok(())
    }

    #[cfg(feature = "write")]
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

    #[cfg(feature = "write")]
    /// Puts a note on a cell, replacing any note already there.
    #[wasm_bindgen(js_name = setComment)]
    pub fn set_comment(
        &mut self,
        sheet: usize,
        address: &str,
        author: &str,
        text: &str,
    ) -> Result<(), JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        let comment = crate::model::Comment {
            author: author.to_owned(),
            text: vec![crate::model::TextRun {
                text: text.to_owned(),
                font: None,
            }],
        };
        self.sheet_mut(sheet)?.comments.insert(at, comment);
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Takes a note off a cell. Answers whether there was one.
    #[wasm_bindgen(js_name = removeComment)]
    pub fn remove_comment(&mut self, sheet: usize, address: &str) -> Result<bool, JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        Ok(self.sheet_mut(sheet)?.comments.remove(&at).is_some())
    }

    #[cfg(feature = "write")]
    /// Puts a link over a cell or a block of them.
    ///
    /// `target` is a URL or a file path unless `inside` is true, and then it
    /// is a place in this workbook: `'Sheet 2'!A1`.
    #[wasm_bindgen(js_name = setHyperlink)]
    pub fn set_hyperlink(
        &mut self,
        sheet: usize,
        range: &str,
        target: &str,
        inside: Option<bool>,
        display: Option<String>,
        tooltip: Option<String>,
    ) -> Result<(), JsError> {
        use crate::model::{Hyperlink, LinkTarget};
        let area = Range::parse(range).map_err(js)?;
        let target = if inside.unwrap_or(false) {
            LinkTarget::Inside(target.to_owned())
        } else {
            LinkTarget::Outside(target.to_owned())
        };
        let ws = self.sheet_mut(sheet)?;
        ws.hyperlinks.retain(|link| link.range != area);
        ws.hyperlinks.push(Hyperlink {
            range: area,
            target,
            display,
            tooltip,
        });
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Takes the link off a range. Answers whether there was one.
    #[wasm_bindgen(js_name = removeHyperlink)]
    pub fn remove_hyperlink(&mut self, sheet: usize, range: &str) -> Result<bool, JsError> {
        let area = Range::parse(range).map_err(js)?;
        let ws = self.sheet_mut(sheet)?;
        let before = ws.hyperlinks.len();
        ws.hyperlinks.retain(|link| link.range != area);
        Ok(ws.hyperlinks.len() != before)
    }

    #[cfg(feature = "write")]
    /// Draws a table over a range, the thing `Sales[Amount]` names.
    ///
    /// The first row of the range is the header unless `headerRow` says
    /// otherwise, and the column names are read from it. The name must be
    /// unique in the workbook, as Excel requires.
    #[wasm_bindgen(js_name = addTable)]
    pub fn add_table(
        &mut self,
        sheet: usize,
        name: &str,
        range: &str,
        header_row: Option<bool>,
    ) -> Result<(), JsError> {
        use crate::model::table::{Table, TableColumn, TableStyle};
        let area = Range::parse(range).map_err(js)?;
        let taken = self
            .book
            .sheets()
            .iter()
            .flat_map(|ws| &ws.tables)
            .any(|t| t.display_name.eq_ignore_ascii_case(name));
        if taken {
            return Err(JsError::new(
                "a table of that name is already in the workbook",
            ));
        }
        let header = header_row.unwrap_or(true);
        let ws = self.sheet_mut(sheet)?;
        let mut columns = Vec::new();
        for (index, col) in (area.start.col.index()..=area.end.col.index()).enumerate() {
            let named = Col::new(col)
                .filter(|_| header)
                .and_then(|col| ws.get(CellRef::new(col, area.start.row)))
                .and_then(|cell| cell.value.plain_text())
                .filter(|text| !text.is_empty());
            columns.push(TableColumn {
                id: u32::try_from(index + 1).unwrap_or(u32::MAX),
                name: named.unwrap_or_else(|| format!("Column{}", index + 1)),
                totals_row_function: None,
                totals_row_label: None,
                calculated_formula: None,
            });
        }
        let id = u32::try_from(ws.tables.len() + 1).unwrap_or(u32::MAX);
        ws.tables.push(Table {
            id,
            name: name.to_owned(),
            display_name: name.to_owned(),
            range: area,
            header_row_count: Some(u32::from(header)),
            totals_row_count: None,
            auto_filter: header.then_some(area),
            columns,
            style: Some(TableStyle::default()),
        });
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Removes a table by name. Answers whether there was one.
    #[wasm_bindgen(js_name = removeTable)]
    pub fn remove_table(&mut self, sheet: usize, name: &str) -> Result<bool, JsError> {
        let ws = self.sheet_mut(sheet)?;
        let before = ws.tables.len();
        ws.tables
            .retain(|table| !table.display_name.eq_ignore_ascii_case(name));
        Ok(ws.tables.len() != before)
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

    #[cfg(feature = "write")]
    /// Freezes the first `rows` rows and `columns` columns, so they stay put
    /// while the rest scrolls. `freezePanes(0, 1, 0)` pins the header row.
    #[wasm_bindgen(js_name = freezePanes)]
    pub fn freeze_panes(&mut self, sheet: usize, rows: u32, columns: u32) -> Result<(), JsError> {
        use crate::model::{Pane, PanePosition, PaneState};
        let ws = self.sheet_mut(sheet)?;
        if rows == 0 && columns == 0 {
            ws.view.pane = None;
            return Ok(());
        }
        let top_left = at_index(rows + 1, columns + 1)?;
        ws.view.pane = Some(Pane {
            x_split: columns,
            y_split: rows,
            top_left_cell: Some(top_left),
            active_pane: match (rows, columns) {
                (0, _) => PanePosition::TopRight,
                (_, 0) => PanePosition::BottomLeft,
                _ => PanePosition::BottomRight,
            },
            state: PaneState::Frozen,
        });
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Unfreezes the panes. The same as `freezePanes(sheet, 0, 0)`.
    #[wasm_bindgen(js_name = unfreezePanes)]
    pub fn unfreeze_panes(&mut self, sheet: usize) -> Result<(), JsError> {
        self.sheet_mut(sheet)?.view.pane = None;
        Ok(())
    }

    #[cfg(feature = "write")]
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

    #[cfg(feature = "write")]
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

    /// The names the workbook defines: what `=Total` and `Print_Area` stand
    /// for. `sheet` is set on a name local to one sheet.
    #[wasm_bindgen(js_name = definedNames, unchecked_return_type = "WorkbookName[]")]
    #[must_use]
    pub fn defined_names(&self) -> JsValue {
        let out = js_sys::Array::new();
        for name in &self.book.defined_names {
            out.push(&object(&[
                ("name", JsValue::from_str(&name.name)),
                ("formula", JsValue::from_str(&name.formula)),
                (
                    "sheet",
                    name.sheet.map_or(JsValue::NULL, |s| {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "a sheet index is far below 2^53"
                        )]
                        JsValue::from_f64(s as f64)
                    }),
                ),
                ("hidden", JsValue::from_bool(name.hidden)),
            ]));
        }
        out.into()
    }

    #[cfg(feature = "write")]
    /// Defines a name, replacing one of the same name. `formula` is what it
    /// stands for - `Sheet1!$A$1:$A$9` or a constant - and `sheet` makes the
    /// name local to that sheet instead of the workbook.
    #[wasm_bindgen(js_name = setDefinedName)]
    pub fn set_defined_name(
        &mut self,
        name: &str,
        formula: &str,
        sheet: Option<usize>,
    ) -> Result<(), JsError> {
        if name.is_empty() {
            return Err(JsError::new("a defined name is not empty"));
        }
        if sheet.is_some_and(|index| self.book.sheet(index).is_none()) {
            return Err(JsError::new("no such sheet"));
        }
        self.book
            .defined_names
            .retain(|existing| !(existing.name == name && existing.sheet == sheet));
        self.book.defined_names.push(crate::model::DefinedName {
            name: name.to_owned(),
            sheet,
            formula: formula.to_owned(),
            hidden: false,
        });
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Removes a defined name. Answers whether there was one.
    #[wasm_bindgen(js_name = removeDefinedName)]
    pub fn remove_defined_name(&mut self, name: &str, sheet: Option<usize>) -> bool {
        let before = self.book.defined_names.len();
        self.book
            .defined_names
            .retain(|existing| !(existing.name == name && existing.sheet == sheet));
        let removed = self.book.defined_names.len() != before;
        if removed {
            self.forget_dependencies();
        }
        removed
    }

    #[cfg(feature = "write")]
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
        use crate::model::protection::PasswordHash;
        let hash = match password.as_deref() {
            Some(word) => Some(
                PasswordHash::new(word)
                    .ok_or_else(|| JsError::new("the password could not be hashed"))?,
            ),
            None => None,
        };
        let protection = &mut self.sheet_mut(sheet)?.protection;
        protection.sheet = Some(true);
        protection.password = hash;
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Unlocks a sheet, password and all.
    #[wasm_bindgen(js_name = unprotectSheet)]
    pub fn unprotect_sheet(&mut self, sheet: usize) -> Result<(), JsError> {
        self.sheet_mut(sheet)?.protection = crate::model::protection::SheetProtection::default();
        Ok(())
    }

    /// Whether a sheet is locked, and whether the lock has a password.
    #[wasm_bindgen(js_name = sheetProtection, unchecked_return_type = "ProtectionInfo")]
    pub fn sheet_protection(&self, sheet: usize) -> Result<JsValue, JsError> {
        let protection = &self.sheet_of(sheet)?.protection;
        Ok(object(&[
            (
                "locked",
                JsValue::from_bool(protection.sheet.unwrap_or(false)),
            ),
            (
                "hasPassword",
                JsValue::from_bool(protection.password.is_some()),
            ),
        ]))
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

    #[cfg(feature = "write")]
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
        use crate::model::protection::PasswordHash;
        let hash = match password.as_deref() {
            Some(word) => Some(
                PasswordHash::new(word)
                    .ok_or_else(|| JsError::new("the password could not be hashed"))?,
            ),
            None => None,
        };
        self.book.protection.lock_structure = Some(true);
        self.book.protection.lock_windows = windows.map(Some).unwrap_or_default();
        self.book.protection.workbook_password = hash;
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Unlocks the workbook.
    #[wasm_bindgen(js_name = unprotectWorkbook)]
    pub fn unprotect_workbook(&mut self) {
        self.book.protection = crate::model::protection::WorkbookProtection::default();
    }

    /// Whether the workbook's structure is locked, and whether behind a
    /// password.
    #[wasm_bindgen(js_name = workbookProtection, unchecked_return_type = "ProtectionInfo")]
    #[must_use]
    pub fn workbook_protection(&self) -> JsValue {
        object(&[
            (
                "locked",
                JsValue::from_bool(self.book.protection.lock_structure.unwrap_or(false)),
            ),
            (
                "hasPassword",
                JsValue::from_bool(self.book.protection.workbook_password.is_some()),
            ),
        ])
    }

    /// The rules limiting what cells accept: the dropdown lists, the ranges,
    /// the dates.
    #[wasm_bindgen(js_name = dataValidations, unchecked_return_type = "SheetValidation[]")]
    pub fn data_validations(&self, sheet: usize) -> Result<JsValue, JsError> {
        let out = js_sys::Array::new();
        for rule in &self.sheet_of(sheet)?.data_validations {
            out.push(&object(&[
                ("sqref", ranges_to_js(&rule.sqref)),
                ("type", JsValue::from_str(rule.kind.as_str())),
                ("operator", JsValue::from_str(rule.operator.as_str())),
                ("formula1", JsValue::from_str(&rule.formula1)),
                ("formula2", JsValue::from_str(&rule.formula2)),
                ("allowBlank", JsValue::from_bool(rule.allow_blank)),
                ("showDropDown", JsValue::from_bool(!rule.hide_drop_down)),
            ]));
        }
        Ok(out.into())
    }

    /// The conditional formatting of a sheet, block by block. The formatting
    /// each rule applies is not modelled here - it lives in the workbook's
    /// differential styles - but what the rule tests is.
    #[wasm_bindgen(
        js_name = conditionalFormats,
        unchecked_return_type = "SheetConditionalFormat[]"
    )]
    pub fn conditional_formats(&self, sheet: usize) -> Result<JsValue, JsError> {
        let out = js_sys::Array::new();
        for block in &self.sheet_of(sheet)?.conditional_formats {
            let rules = js_sys::Array::new();
            for rule in &block.rules {
                let formulas = js_sys::Array::new();
                for formula in &rule.formulas {
                    formulas.push(&JsValue::from_str(formula));
                }
                rules.push(&object(&[
                    ("type", JsValue::from_str(rule.kind.as_str())),
                    ("priority", JsValue::from_f64(f64::from(rule.priority))),
                    (
                        "operator",
                        rule.operator
                            .map_or(JsValue::NULL, |op| JsValue::from_str(op.as_str())),
                    ),
                    ("formulas", formulas.into()),
                    ("text", opt_str(rule.text.as_deref())),
                ]));
            }
            out.push(&object(&[
                ("sqref", ranges_to_js(&block.sqref)),
                ("rules", rules.into()),
            ]));
        }
        Ok(out.into())
    }

    /// The autofilter over a sheet, or `undefined` when it has none. What the
    /// filter hides is a property of each row - see `rowHidden`.
    #[wasm_bindgen(js_name = autoFilter, unchecked_return_type = "SheetAutoFilter | undefined")]
    pub fn auto_filter(&self, sheet: usize) -> Result<JsValue, JsError> {
        use crate::model::autofilter::ColumnFilter;
        let Some(filter) = &self.sheet_of(sheet)?.auto_filter else {
            return Ok(JsValue::UNDEFINED);
        };
        let columns = js_sys::Array::new();
        for column in &filter.columns {
            let kind = match &column.filter {
                Some(ColumnFilter::Values { .. }) => "values",
                Some(ColumnFilter::Custom { .. }) => "custom",
                Some(ColumnFilter::Dynamic { .. }) => "dynamic",
                Some(ColumnFilter::Top10 { .. }) => "top10",
                None => "none",
            };
            columns.push(&object(&[
                ("colId", JsValue::from_f64(f64::from(column.col_id))),
                ("kind", JsValue::from_str(kind)),
            ]));
        }
        Ok(object(&[
            ("range", JsValue::from_str(&filter.range.to_string())),
            ("columns", columns.into()),
        ]))
    }

    /// The pivot reports laid out on a sheet. The numbers they show are cells
    /// like any others; this says where a report sits and how wide its axes
    /// are.
    #[wasm_bindgen(js_name = pivotTables, unchecked_return_type = "SheetPivotTable[]")]
    #[expect(
        clippy::cast_precision_loss,
        reason = "counts of pivot fields are far below 2^53"
    )]
    pub fn pivot_tables(&self, sheet: usize) -> Result<JsValue, JsError> {
        let out = js_sys::Array::new();
        for pivot in &self.sheet_of(sheet)?.pivot_tables {
            out.push(&object(&[
                ("name", JsValue::from_str(&pivot.name)),
                (
                    "location",
                    opt_str(pivot.location.map(|r| r.to_string()).as_deref()),
                ),
                ("cacheId", JsValue::from_f64(f64::from(pivot.cache_id))),
                (
                    "rowFields",
                    JsValue::from_f64(pivot.row_fields.len() as f64),
                ),
                (
                    "columnFields",
                    JsValue::from_f64(pivot.column_fields.len() as f64),
                ),
                (
                    "valueFields",
                    JsValue::from_f64(pivot.data_fields.len() as f64),
                ),
            ]));
        }
        Ok(out.into())
    }

    /// The array formulas of a sheet, as the areas they cover: `["B2:B4"]`.
    /// The formula itself is on the top left cell of each.
    #[wasm_bindgen(js_name = arrayFormulas)]
    pub fn array_formulas(&self, sheet: usize) -> Result<Vec<String>, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .array_formulas
            .iter()
            .map(ToString::to_string)
            .collect())
    }

    /// The other workbooks this one reads, in the order `[1]Sheet1!A1` counts
    /// them. A path is `undefined` where the file does not name one.
    #[wasm_bindgen(js_name = externalBooks)]
    #[must_use]
    pub fn external_books(&self) -> Vec<JsValue> {
        self.book
            .external
            .iter()
            .map(|book| opt_str(book.path.as_deref()))
            .collect()
    }

    #[cfg(feature = "write")]
    /// Merges a block of cells: `merge(0, "A1:C1")`.
    pub fn merge(&mut self, sheet: usize, range: &str) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        let ws = self
            .book
            .sheet_mut(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        if !ws.merges.contains(&area) {
            ws.merges.push(area);
        }
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Takes a merge back out. Answers whether there was one.
    pub fn unmerge(&mut self, sheet: usize, range: &str) -> Result<bool, JsError> {
        let area = Range::parse(range).map_err(js)?;
        let ws = self
            .book
            .sheet_mut(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        let before = ws.merges.len();
        ws.merges.retain(|m| *m != area);
        Ok(ws.merges.len() != before)
    }

    #[cfg(feature = "write")]
    /// Inserts `count` rows above row `at`, moving everything below down.
    ///
    /// Formulas across the whole workbook follow the cells they read, and so
    /// do merges, links, validations, tables and drawings. `at` is the number
    /// a user sees.
    #[wasm_bindgen(js_name = insertRows)]
    pub fn insert_rows(&mut self, sheet: usize, at: u32, count: u32) -> Result<(), JsError> {
        let row = Row::from_one_based(u64::from(at)).map_err(js)?;
        crate::edit::insert_rows(&mut self.book, sheet, row, count).map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Removes `count` rows from row `at` down. A formula that read a removed
    /// cell reads `#REF!` afterwards.
    #[wasm_bindgen(js_name = removeRows)]
    pub fn remove_rows(&mut self, sheet: usize, at: u32, count: u32) -> Result<(), JsError> {
        let row = Row::from_one_based(u64::from(at)).map_err(js)?;
        crate::edit::remove_rows(&mut self.book, sheet, row, count).map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Inserts `count` columns to the left of column `at`, 1-based.
    #[wasm_bindgen(js_name = insertColumns)]
    pub fn insert_columns(&mut self, sheet: usize, at: u32, count: u32) -> Result<(), JsError> {
        let col = Col::from_one_based(u64::from(at)).map_err(js)?;
        crate::edit::insert_columns(&mut self.book, sheet, col, count).map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Removes `count` columns from column `at`, 1-based.
    #[wasm_bindgen(js_name = removeColumns)]
    pub fn remove_columns(&mut self, sheet: usize, at: u32, count: u32) -> Result<(), JsError> {
        let col = Col::from_one_based(u64::from(at)).map_err(js)?;
        crate::edit::remove_columns(&mut self.book, sheet, col, count).map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
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
        let area = Range::parse(range).map_err(js)?;
        let at = CellRef::parse(to).map_err(js)?;
        crate::edit::copy_range(&mut self.book, sheet, area, to_sheet.unwrap_or(sheet), at)
            .map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
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
        let area = Range::parse(range).map_err(js)?;
        let at = CellRef::parse(to).map_err(js)?;
        crate::edit::move_range(&mut self.book, sheet, area, to_sheet.unwrap_or(sheet), at)
            .map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Inserts blank cells over a range, pushing what was there `"down"` or
    /// `"right"` - Excel's "Insert Cells", which moves part of a row rather
    /// than the whole of it.
    #[wasm_bindgen(js_name = insertCells)]
    pub fn insert_cells(&mut self, sheet: usize, range: &str, shift: &str) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        crate::edit::insert_cells(&mut self.book, sheet, area, axis_of(shift)?).map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Removes the cells of a range, pulling the rest back over the hole from
    /// below (`"up"`) or from the right (`"left"`).
    #[wasm_bindgen(js_name = removeCells)]
    pub fn remove_cells(&mut self, sheet: usize, range: &str, shift: &str) -> Result<(), JsError> {
        let area = Range::parse(range).map_err(js)?;
        crate::edit::remove_cells(&mut self.book, sheet, area, axis_of(shift)?).map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
    /// Moves a sheet along the tab bar. Formulas are untouched - a sheet is
    /// named, not numbered - but every index moves, this one included, so read
    /// `sheetIndex` again afterwards.
    #[wasm_bindgen(js_name = moveSheet)]
    pub fn move_sheet(&mut self, from: usize, to: usize) -> Result<(), JsError> {
        crate::edit::move_sheet(&mut self.book, from, to).map_err(js)
    }

    #[cfg(feature = "write")]
    /// Removes a sheet. References to it from anywhere in the workbook become
    /// `#REF!`, the way Excel writes them, and the sheets after it shift down
    /// by one index.
    #[wasm_bindgen(js_name = removeSheet)]
    pub fn remove_sheet(&mut self, sheet: usize) -> Result<(), JsError> {
        crate::edit::remove_sheet(&mut self.book, sheet).map_err(js)?;
        self.forget_dependencies();
        Ok(())
    }

    #[cfg(feature = "write")]
    /// One sheet as CSV, comma-separated.
    #[wasm_bindgen(js_name = toCsv)]
    pub fn to_csv(&self, sheet: usize) -> Result<String, JsError> {
        self.csv(sheet, ',')
    }

    #[cfg(feature = "write")]
    /// The same with the delimiter stated - a semicolon for a locale where the
    /// comma is the decimal mark.
    #[wasm_bindgen(js_name = toCsvWith)]
    pub fn to_csv_with(
        &self,
        sheet: usize,
        #[wasm_bindgen(unchecked_param_type = "CsvOptions")] options: &JsValue,
    ) -> Result<String, JsError> {
        let delimiter = match field(options, "delimiter") {
            Some(value) => one_char(&value)?,
            None => ',',
        };
        self.csv(sheet, delimiter)
    }

    #[cfg(feature = "write")]
    fn csv(&self, sheet: usize, delimiter: char) -> Result<String, JsError> {
        let mut out = Vec::new();
        write_csv_to(&self.book, sheet, &mut out, delimiter).map_err(js)?;
        String::from_utf8(out).map_err(js)
    }

    /// Reads CSV with the guesses overridden: the delimiter, whether a row
    /// that produced no cell is skipped, whether an empty field becomes an
    /// empty string rather than no cell at all.
    ///
    /// `Book.read` guesses all of this from the bytes, which is right for a
    /// file of unknown origin; this is for one whose shape you already know.
    #[wasm_bindgen(js_name = readCsv)]
    pub fn read_csv(
        bytes: &[u8],
        #[wasm_bindgen(unchecked_param_type = "CsvOptions")] options: &JsValue,
    ) -> Result<Book, JsError> {
        let mut parsed = crate::reader::csv::CsvOptions::default();
        if let Some(value) = field(options, "delimiter") {
            parsed.delimiter = Some(one_char(&value)?);
        }
        if let Some(value) = field(options, "contiguous") {
            parsed.contiguous = value.is_truthy();
        }
        if let Some(value) = field(options, "preserveEmptyFields") {
            parsed.preserve_empty_fields = value.is_truthy();
        }
        Ok(Self::wrap(crate::reader::csv::read_csv_bytes(
            bytes, &parsed,
        )))
    }
}

impl Book {
    /// The sheet at that index to write to.
    #[cfg(feature = "write")]
    fn sheet_mut(&mut self, sheet: usize) -> Result<&mut Worksheet, JsError> {
        self.book
            .sheet_mut(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))
    }

    /// The sheet at that index, or the one error every binding reports.
    fn sheet_of(&self, sheet: usize) -> Result<&Worksheet, JsError> {
        self.book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))
    }

    #[cfg(feature = "write")]
    #[cfg_attr(
        not(feature = "formulas"),
        expect(clippy::unused_self, reason = "there is no index without the engine")
    )]
    /// Drops the dependency index after an edit that rewrote formulas: every
    /// address in it may have moved.
    fn forget_dependencies(&mut self) {
        #[cfg(feature = "formulas")]
        {
            self.deps = None;
        }
    }

    /// A workbook with no dependency index built yet.
    fn wrap(book: Spreadsheet) -> Self {
        Self {
            book,
            #[cfg(feature = "formulas")]
            deps: None,
            #[cfg(feature = "formulas")]
            custom: CustomFunctions::new(),
        }
    }
}

impl Default for Book {
    fn default() -> Self {
        Self::new()
    }
}

/// A cell reference from the 1-based numbers a user sees.
fn at_index(row: u32, column: u32) -> Result<CellRef, JsError> {
    let row = Row::from_one_based(u64::from(row)).map_err(js)?;
    let col = Col::from_one_based(u64::from(column)).map_err(js)?;
    Ok(CellRef::new(col, row))
}

/// A rectangle from its top-left cell and its size, both 1-based.
fn area_at(row: u32, column: u32, rows: u32, columns: u32) -> Result<Range, JsError> {
    if rows == 0 || columns == 0 {
        return Err(JsError::new("a range covers at least one row and column"));
    }
    let start = at_index(row, column)?;
    let end = at_index(row + rows - 1, column + columns - 1)?;
    Ok(Range::new(start, end))
}

/// One JS value as a cell. A string starting with `=` is a formula.
fn js_to_cell(value: &JsValue) -> Result<CellValue, JsError> {
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

/// What a registered function answered, as the engine sees it.
///
/// The mirror of [`value_to_js`]: numbers, text and booleans come back as
/// themselves, an array of arrays as an array value, and anything else - a
/// promise, an object, `undefined` - as blank, because a formula has to end
/// with a value and there is nothing else to make of it. An error is spelled
/// the way a cell spells it, so returning `"#N/A"` gives `#N/A` rather than
/// the text.
#[cfg(feature = "formulas")]
fn js_to_value(value: &JsValue) -> Value {
    if let Some(n) = value.as_f64() {
        return Value::Number(n);
    }
    if let Some(b) = value.as_bool() {
        return Value::Bool(b);
    }
    if let Some(text) = value.as_string() {
        return match CellError::parse(&text) {
            Some(e) => Value::Error(e),
            None => Value::Text(text),
        };
    }
    if let Ok(rows) = value.clone().dyn_into::<js_sys::Array>() {
        let grid: Vec<Vec<Value>> = rows
            .iter()
            .map(|row| match row.dyn_into::<js_sys::Array>() {
                Ok(cells) => cells.iter().map(|c| js_to_value(&c)).collect(),
                // A flat array is one row, which is how JS callers write one.
                Err(one) => vec![js_to_value(&one)],
            })
            .collect();
        return Value::array(grid);
    }
    Value::Blank
}

/// A stored value as JS. Formulas are not nested, so their cache is scalar.
fn cell_to_js(value: &CellValue) -> JsValue {
    match value {
        CellValue::Number(n) => JsValue::from_f64(*n),
        CellValue::Bool(b) => JsValue::from_bool(*b),
        CellValue::Error(e) => JsValue::from_str(e.as_str()),
        CellValue::Text(_) | CellValue::RichText(_) => {
            JsValue::from_str(&value.plain_text().unwrap_or_default())
        }
        CellValue::Empty | CellValue::Formula { .. } => JsValue::NULL,
    }
}

/// An argument as a registered function sees it: a range arrives whole, as an
/// array of rows, rather than collapsed the way a cell would show it.
#[cfg(feature = "formulas")]
fn value_to_js_deep(value: &Value) -> JsValue {
    match value {
        Value::Array(rows) => {
            let out = js_sys::Array::new();
            for row in rows.iter() {
                let line = js_sys::Array::new();
                for cell in row {
                    line.push(&value_to_js_deep(cell));
                }
                out.push(&line);
            }
            out.into()
        }
        // A function is not something JS can be handed; it never reaches an
        // argument list anyway, since a lambda is only a value mid-formula.
        Value::Lambda(_) => JsValue::NULL,
        other => value_to_js(other),
    }
}

/// An engine result as JS; an array comes back as its top-left value, the way
/// a single cell shows one.
#[cfg(feature = "formulas")]
fn value_to_js(value: &Value) -> JsValue {
    match value {
        Value::Blank => JsValue::NULL,
        Value::Number(n) => JsValue::from_f64(*n),
        Value::Text(t) => JsValue::from_str(t),
        Value::Bool(b) => JsValue::from_bool(*b),
        Value::Error(e) => JsValue::from_str(e.as_str()),
        Value::Lambda(_) => JsValue::NULL,
        Value::Array(rows) => rows
            .first()
            .and_then(|r| r.first())
            .map_or(JsValue::NULL, value_to_js),
    }
}

/// A colour as JS sees it: the text a reader can act on, or `null` where the
/// file left the choice open.
fn color_to_js(color: &crate::style::Color) -> JsValue {
    use crate::style::Color;
    match color {
        Color::Auto => JsValue::NULL,
        Color::Argb(argb) => JsValue::from_str(&format!("#{argb:08X}")),
        Color::Indexed(i) => JsValue::from_str(&format!("indexed:{i}")),
        Color::Theme { id, tint } => JsValue::from_str(&if *tint == 0 {
            format!("theme:{id}")
        } else {
            format!("theme:{id}@{}", f64::from(*tint) / 1_000_000.0)
        }),
    }
}

/// One field of a JS object, or `None` when it is absent or `undefined`.
fn field(object: &JsValue, key: &str) -> Option<JsValue> {
    let value = js_sys::Reflect::get(object, &JsValue::from_str(key)).ok()?;
    (!value.is_undefined()).then_some(value)
}

/// A colour as the JS side spells it: `#AARRGGBB`, `indexed:N`, `theme:N` with
/// an optional `@tint`, or `null` for the one the file leaves open.
#[cfg(feature = "write")]
fn color_from_js(value: &JsValue) -> Result<crate::style::Color, JsError> {
    use crate::style::Color;
    if value.is_null() {
        return Ok(Color::Auto);
    }
    let text = value
        .as_string()
        .ok_or_else(|| JsError::new("a colour is a string or null"))?;
    if let Some(index) = text.strip_prefix("indexed:") {
        return index
            .parse()
            .map(Color::Indexed)
            .map_err(|_| JsError::new("indexed:N takes a number"));
    }
    if let Some(rest) = text.strip_prefix("theme:") {
        let (id, tint) = match rest.split_once('@') {
            Some((id, tint)) => {
                let tint: f64 = tint
                    .parse()
                    .map_err(|_| JsError::new("a theme tint is a number between -1 and 1"))?;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a tint is between -1 and 1, and the file stores it in millionths"
                )]
                (id, (tint * 1_000_000.0).round() as i32)
            }
            None => (rest, 0),
        };
        return id
            .parse()
            .map(|id| Color::Theme { id, tint })
            .map_err(|_| JsError::new("theme:N takes a number"));
    }
    // Written as `#AARRGGBB`, which is how `cellStyle` reads it back; the
    // file itself stores the eight digits alone, so both forms are taken.
    Color::from_argb_str(text.strip_prefix('#').unwrap_or(&text))
        .ok_or_else(|| JsError::new("a colour is #AARRGGBB, indexed:N, theme:N or null"))
}

/// A number a patch states, rejected if it is not one.
#[cfg(feature = "write")]
fn number(value: &JsValue, what: &str) -> Result<f64, JsError> {
    value
        .as_f64()
        .ok_or_else(|| JsError::new(&format!("{what} is a number")))
}

/// The font half of a `CellStylePatch`.
#[cfg(feature = "write")]
fn patch_font(font: &mut crate::style::Font, patch: &JsValue) -> Result<(), JsError> {
    use crate::style::Underline;
    if let Some(name) = field(patch, "name") {
        font.name = name
            .as_string()
            .ok_or_else(|| JsError::new("a font name is a string"))?;
    }
    if let Some(size) = field(patch, "size") {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a font size in hundredths of a point is a small positive number"
        )]
        {
            font.size = (number(&size, "a font size")? * 100.0).round() as u32;
        }
    }
    if let Some(bold) = field(patch, "bold") {
        font.bold = bold.is_truthy();
    }
    if let Some(italic) = field(patch, "italic") {
        font.italic = italic.is_truthy();
    }
    if let Some(strike) = field(patch, "strike") {
        font.strike = strike.is_truthy();
    }
    if let Some(underline) = field(patch, "underline") {
        font.underline = underline
            .as_string()
            .map(|u| Underline::parse(&u))
            .ok_or_else(|| JsError::new("an underline is a string"))?;
    }
    if let Some(color) = field(patch, "color") {
        font.color = color_from_js(&color)?;
    }
    Ok(())
}

/// The fill half of a `CellStylePatch`.
#[cfg(feature = "write")]
fn patch_fill(fill: &mut crate::style::Fill, patch: &JsValue) -> Result<(), JsError> {
    if let Some(pattern) = field(patch, "pattern") {
        fill.pattern = pattern
            .as_string()
            .map(|p| crate::style::Pattern::parse(&p))
            .ok_or_else(|| JsError::new("a fill pattern is a string"))?;
    }
    if let Some(color) = field(patch, "foreground") {
        fill.foreground = color_from_js(&color)?;
    }
    if let Some(color) = field(patch, "background") {
        fill.background = color_from_js(&color)?;
    }
    Ok(())
}

/// The four sides of a `CellStylePatch`.
#[cfg(feature = "write")]
fn patch_borders(borders: &mut crate::style::Borders, patch: &JsValue) -> Result<(), JsError> {
    for (key, side) in [
        ("left", &mut borders.left),
        ("right", &mut borders.right),
        ("top", &mut borders.top),
        ("bottom", &mut borders.bottom),
    ] {
        let Some(patch) = field(patch, key) else {
            continue;
        };
        if let Some(kind) = field(&patch, "style") {
            side.style = kind
                .as_string()
                .map(|s| crate::style::BorderStyle::parse(&s))
                .ok_or_else(|| JsError::new("a border style is a string"))?;
        }
        if let Some(color) = field(&patch, "color") {
            side.color = color_from_js(&color)?;
        }
    }
    Ok(())
}

/// The alignment half of a `CellStylePatch`.
#[cfg(feature = "write")]
fn patch_alignment(
    alignment: &mut crate::style::Alignment,
    patch: &JsValue,
) -> Result<(), JsError> {
    use crate::style::{HorizontalAlign, VerticalAlign};
    if let Some(horizontal) = field(patch, "horizontal") {
        alignment.horizontal = horizontal
            .as_string()
            .map_or(HorizontalAlign::General, |h| HorizontalAlign::parse(&h));
    }
    if let Some(vertical) = field(patch, "vertical") {
        alignment.vertical = vertical
            .as_string()
            .map_or(VerticalAlign::Bottom, |v| VerticalAlign::parse(&v));
    }
    if let Some(wrap) = field(patch, "wrapText") {
        alignment.wrap_text = wrap.is_truthy();
    }
    if let Some(shrink) = field(patch, "shrinkToFit") {
        alignment.shrink_to_fit = shrink.is_truthy();
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "indent steps and a rotation in degrees are small positive numbers"
    )]
    {
        if let Some(indent) = field(patch, "indent") {
            alignment.indent = number(&indent, "an indent")? as u32;
        }
        if let Some(rotation) = field(patch, "textRotation") {
            alignment.text_rotation = number(&rotation, "a text rotation")? as u32;
        }
    }
    Ok(())
}

/// Lays a `CellStylePatch` over a style. A field the patch leaves out keeps
/// what the cell had.
#[cfg(feature = "write")]
fn apply_style_patch(style: &mut crate::style::Style, patch: &JsValue) -> Result<(), JsError> {
    if let Some(code) = field(patch, "numberFormat") {
        let code = code
            .as_string()
            .ok_or_else(|| JsError::new("a number format is a string"))?;
        style.number_format = if code == crate::style::format::GENERAL {
            crate::style::NumberFormat::General
        } else {
            crate::style::NumberFormat::Custom(code)
        };
    }
    if let Some(font) = field(patch, "font") {
        patch_font(&mut style.font, &font)?;
    }
    if let Some(fill) = field(patch, "fill") {
        patch_fill(&mut style.fill, &fill)?;
    }
    if let Some(borders) = field(patch, "borders") {
        patch_borders(&mut style.borders, &borders)?;
    }
    if let Some(alignment) = field(patch, "alignment") {
        patch_alignment(&mut style.alignment, &alignment)?;
    }
    Ok(())
}

/// Which way an insert or a remove of cells moves the neighbours.
#[cfg(feature = "write")]
fn axis_of(shift: &str) -> Result<crate::edit::Axis, JsError> {
    match shift {
        "down" | "up" => Ok(crate::edit::Axis::Rows),
        "right" | "left" => Ok(crate::edit::Axis::Columns),
        _ => Err(JsError::new(
            r#"a shift is "down", "up", "right" or "left""#,
        )),
    }
}

/// One character from JS, for a delimiter.
fn one_char(value: &JsValue) -> Result<char, JsError> {
    value
        .as_string()
        .and_then(|text| {
            let mut chars = text.chars();
            chars.next().filter(|_| chars.next().is_none())
        })
        .ok_or_else(|| JsError::new("a delimiter is one character"))
}

/// A list of areas as their `"A1:C9"` strings.
fn ranges_to_js(ranges: &[Range]) -> JsValue {
    let out = js_sys::Array::new();
    for area in ranges {
        out.push(&JsValue::from_str(&area.to_string()));
    }
    out.into()
}

/// A string that may not be there.
fn opt_str(text: Option<&str>) -> JsValue {
    text.map_or(JsValue::NULL, JsValue::from_str)
}

/// A count that may not be there.
fn opt_count(count: Option<u32>) -> JsValue {
    count.map_or(JsValue::NULL, |n| JsValue::from_f64(f64::from(n)))
}

/// Where a drawing object sits, as `ObjectAnchor`. An absolute anchor is at a
/// point on the sheet rather than at a cell, so it names no row or column.
fn anchor_to_js(anchor: &crate::model::chart::Anchor) -> JsValue {
    use crate::model::chart::Anchor;
    let (kind, at) = match anchor {
        Anchor::TwoCell { from, .. } => ("twoCell", Some(from)),
        Anchor::OneCell { from, .. } => ("oneCell", Some(from)),
        Anchor::Absolute { .. } => ("absolute", None),
    };
    object(&[
        ("kind", JsValue::from_str(kind)),
        (
            "row",
            at.map_or(JsValue::NULL, |m| {
                JsValue::from_f64(f64::from(m.row.one_based()))
            }),
        ),
        (
            "column",
            at.map_or(JsValue::NULL, |m| {
                JsValue::from_f64(f64::from(m.col.one_based()))
            }),
        ),
    ])
}

/// Builds a JS object from `(key, value)` pairs.
fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let out = js_sys::Object::new();
    for (key, value) in fields {
        // The only way this fails is a frozen object, and this one is ours.
        let _ = js_sys::Reflect::set(&out, &JsValue::from_str(key), value);
    }
    out.into()
}

/// One border side as `{ style, color }`.
fn border_to_js(border: &crate::style::Border) -> JsValue {
    object(&[
        ("style", JsValue::from_str(border.style.as_str())),
        ("color", color_to_js(&border.color)),
    ])
}

/// The whole style of a cell, shaped as the `CellStyle` declaration says.
fn style_to_js(style: &crate::style::Style) -> JsValue {
    let font = object(&[
        ("name", JsValue::from_str(&style.font.name)),
        (
            "size",
            JsValue::from_f64(f64::from(style.font.size) / 100.0),
        ),
        ("bold", JsValue::from_bool(style.font.bold)),
        ("italic", JsValue::from_bool(style.font.italic)),
        (
            "underline",
            JsValue::from_str(style.font.underline.as_str()),
        ),
        ("strike", JsValue::from_bool(style.font.strike)),
        ("color", color_to_js(&style.font.color)),
    ]);
    let fill = object(&[
        ("pattern", JsValue::from_str(style.fill.pattern.as_str())),
        ("foreground", color_to_js(&style.fill.foreground)),
        ("background", color_to_js(&style.fill.background)),
    ]);
    let borders = object(&[
        ("left", border_to_js(&style.borders.left)),
        ("right", border_to_js(&style.borders.right)),
        ("top", border_to_js(&style.borders.top)),
        ("bottom", border_to_js(&style.borders.bottom)),
    ]);
    let alignment = object(&[
        (
            "horizontal",
            style
                .alignment
                .horizontal
                .as_str()
                .map_or(JsValue::NULL, JsValue::from_str),
        ),
        (
            "vertical",
            style
                .alignment
                .vertical
                .as_str()
                .map_or(JsValue::NULL, JsValue::from_str),
        ),
        ("wrapText", JsValue::from_bool(style.alignment.wrap_text)),
        (
            "shrinkToFit",
            JsValue::from_bool(style.alignment.shrink_to_fit),
        ),
        (
            "indent",
            JsValue::from_f64(f64::from(style.alignment.indent)),
        ),
        (
            "textRotation",
            JsValue::from_f64(f64::from(style.alignment.text_rotation)),
        ),
    ]);
    object(&[
        (
            "numberFormat",
            JsValue::from_str(style.number_format.code()),
        ),
        ("font", font),
        ("fill", fill),
        ("borders", borders),
        ("alignment", alignment),
    ])
}
