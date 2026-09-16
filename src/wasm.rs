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
use crate::style::format::{Value as FormatValue, format};
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
const TYPES: &'static str = r"
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
";

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
        let ws = self
            .book
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))?;
        let Some(cell) = ws.get(at) else {
            return Ok(String::new());
        };
        let code = self
            .book
            .styles
            .get(cell.style)
            .map_or(crate::style::format::GENERAL, |s| s.number_format.code());
        let shown = match &cell.value {
            CellValue::Formula { cached, .. } => cached.as_deref().cloned().unwrap_or_default(),
            other => other.clone(),
        };
        Ok(match &shown {
            CellValue::Number(n) => format(FormatValue::Number(*n), code, self.book.epoch),
            CellValue::Bool(b) => (if *b { "TRUE" } else { "FALSE" }).to_owned(),
            CellValue::Error(e) => e.as_str().to_owned(),
            CellValue::Empty => String::new(),
            other => format(
                FormatValue::Text(&other.plain_text().unwrap_or_default()),
                code,
                self.book.epoch,
            ),
        })
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

    #[cfg(feature = "write")]
    /// One sheet as CSV.
    #[wasm_bindgen(js_name = toCsv)]
    pub fn to_csv(&self, sheet: usize) -> Result<String, JsError> {
        let mut out = Vec::new();
        write_csv_to(&self.book, sheet, &mut out, ',').map_err(js)?;
        String::from_utf8(out).map_err(js)
    }
}

impl Book {
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
        return Value::Array(grid);
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
            for row in rows {
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
