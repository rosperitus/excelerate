//! `WebAssembly` bindings: a thin JS-facing wrapper over [`Spreadsheet`].
//!
//! Only what a browser or Node caller needs - read a workbook from bytes, look
//! at and set cells, evaluate a formula, write bytes back. Everything else stays
//! in the Rust API; adding a binding is cheaper than maintaining a mirror of it.
//!
//! One `Book` class, its methods spread over the files below by what they
//! touch. A method that needs the writers or the formula engine sits in an
//! `impl` block gated on that feature, so the reader-only package is the same
//! source built with less.

// Every fallible binding fails one way: the message of the crate error, or a
// bad address / missing sheet. A `# Errors` section per method would repeat it.
#![allow(clippy::missing_errors_doc)]

mod cells;
mod convert;
#[cfg(feature = "write")]
mod edit;
#[cfg(feature = "formulas")]
mod formulas;
mod objects;
#[cfg(feature = "write")]
mod output;
mod sheet;
mod style;
mod types;

use crate::coordinate::CellRef;
#[cfg(feature = "formulas")]
use crate::formula::CustomFunctions;
#[cfg(feature = "formulas")]
use crate::formula::eval::Dependencies;
use crate::model::{Spreadsheet, Worksheet};
use convert::{field, object, one_char};
use wasm_bindgen::prelude::*;

/// A workbook.
#[wasm_bindgen]
pub struct Book {
    workbook: Spreadsheet,
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

    /// Reads a workbook of any format this crate supports, working the format
    /// out from the bytes themselves.
    ///
    /// `name` is the file name they came from, if the caller has one: it
    /// settles what the bytes cannot say (a `.csv` against a `.html`) and names
    /// the sheet of a SYLK file.
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
        // Only xlsx reports its progress: the other readers take a slice and
        // are done.
        let report = on_progress.as_ref().map(reporter);
        let mut options = crate::progress::Options::new();
        if let Some(report) = &report {
            options = options.reporting(report);
        }
        crate::reader::read_bytes_limited_with(bytes, name.as_deref(), limit, &options)
            .map(Self::wrap)
            .map_err(js)
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

    /// Sheet titles, in tab order.
    #[wasm_bindgen(js_name = sheetNames)]
    #[must_use]
    pub fn sheet_names(&self) -> Vec<String> {
        self.workbook
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
            Some(index) => self.workbook.sheet(index).map_or(0, Worksheet::len),
            None => self.workbook.sheets().iter().map(Worksheet::len).sum(),
        }
    }

    /// Appends a sheet and returns its index.
    #[wasm_bindgen(js_name = addSheet)]
    pub fn add_sheet(&mut self, title: &str) -> Result<usize, JsError> {
        let sheet = Worksheet::new(title).map_err(js)?;
        self.workbook.add_sheet(sheet).map_err(js)
    }

    /// The tab index of a sheet by name, or `undefined` when there is none.
    /// Names are compared the way Excel does, ignoring case.
    #[wasm_bindgen(js_name = sheetIndex)]
    #[must_use]
    pub fn sheet_index(&self, name: &str) -> Option<usize> {
        self.workbook.sheet_index_by_name(name)
    }

    /// Renames a sheet.
    #[wasm_bindgen(js_name = renameSheet)]
    pub fn rename_sheet(&mut self, sheet: usize, title: &str) -> Result<(), JsError> {
        self.sheet_mut(sheet)?.set_title(title).map_err(js)
    }

    /// The tab a reader opens the workbook on.
    #[wasm_bindgen(js_name = activeSheet)]
    #[must_use]
    pub fn active_sheet(&self) -> usize {
        self.workbook.active_index()
    }

    /// Sets that tab.
    #[wasm_bindgen(js_name = setActiveSheet)]
    pub fn set_active_sheet(&mut self, sheet: usize) -> Result<(), JsError> {
        self.workbook.set_active(sheet).map_err(js)
    }
}

impl Book {
    /// A workbook with no dependency index built yet.
    fn wrap(book: Spreadsheet) -> Self {
        Self {
            workbook: book,
            #[cfg(feature = "formulas")]
            deps: None,
            #[cfg(feature = "formulas")]
            custom: CustomFunctions::new(),
        }
    }

    /// The sheet at that index, or the one error every binding reports.
    fn sheet_of(&self, sheet: usize) -> Result<&Worksheet, JsError> {
        self.workbook
            .sheet(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))
    }

    /// The same, to write to.
    fn sheet_mut(&mut self, sheet: usize) -> Result<&mut Worksheet, JsError> {
        self.workbook
            .sheet_mut(sheet)
            .ok_or_else(|| JsError::new("no such sheet"))
    }

    /// Tells the dependency index that these cells gained or lost a formula,
    /// so it stays valid without being rebuilt.
    #[cfg_attr(
        not(feature = "formulas"),
        expect(clippy::unused_self, reason = "there is no index without the engine")
    )]
    fn note(&mut self, sheet: usize, cells: &[CellRef]) {
        #[cfg(feature = "formulas")]
        if let Some(deps) = self.deps.as_mut() {
            for &at in cells {
                deps.note(&self.workbook, sheet, at);
            }
        }
        #[cfg(not(feature = "formulas"))]
        let _ = (sheet, cells);
    }

    /// Drops the dependency index after an edit that rewrote formulas: every
    /// address in it may have moved.
    #[cfg(feature = "write")]
    #[cfg_attr(
        not(feature = "formulas"),
        expect(clippy::unused_self, reason = "there is no index without the engine")
    )]
    fn forget_dependencies(&mut self) {
        #[cfg(feature = "formulas")]
        {
            self.deps = None;
        }
    }
}

impl Default for Book {
    fn default() -> Self {
        Self::new()
    }
}
