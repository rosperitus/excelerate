//! The workbook written out: xlsx, ODS, xls, HTML and CSV.

use super::convert::{field, one_char};
use super::{Book, js, reporter};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
impl Book {
    /// The workbook as an xlsx package.
    #[wasm_bindgen(js_name = toXlsx)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "a callback crosses the wasm boundary owned"
    )]
    pub fn to_xlsx(&self, on_progress: Option<js_sys::Function>) -> Result<Vec<u8>, JsError> {
        let report = on_progress.as_ref().map(reporter);
        let mut options = crate::progress::Options::new();
        if let Some(report) = &report {
            options = options.reporting(report);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        crate::writer::xlsx::write_xlsx_to_with(&self.workbook, &mut out, &options).map_err(js)?;
        Ok(out.into_inner())
    }

    /// The workbook as an `OpenDocument` spreadsheet.
    #[wasm_bindgen(js_name = toOds)]
    pub fn to_ods(&self) -> Result<Vec<u8>, JsError> {
        let mut out = std::io::Cursor::new(Vec::new());
        crate::writer::ods::write_ods_to(&self.workbook, &mut out).map_err(js)?;
        Ok(out.into_inner())
    }

    /// The workbook as a BIFF8 `.xls` file.
    ///
    /// Formulas are written as tokens with their cached result; what BIFF8
    /// cannot hold - a structured reference, a row past 65536 - is written as
    /// the value, so call `recalculate` first if the cache is stale.
    #[wasm_bindgen(js_name = toXls)]
    pub fn to_xls(&self) -> Result<Vec<u8>, JsError> {
        let mut out = Vec::new();
        crate::writer::xls::write_xls_to(&self.workbook, &mut out).map_err(js)?;
        Ok(out)
    }

    /// The workbook as an HTML page, or one sheet of it. `fragment` leaves out
    /// the `<html>` wrapper, for embedding in a page that already has one.
    #[wasm_bindgen(js_name = toHtml)]
    pub fn to_html(&self, sheet: Option<usize>, fragment: Option<bool>) -> Result<String, JsError> {
        let mut out = Vec::new();
        let options = crate::writer::html::HtmlOptions {
            sheet,
            fragment: fragment.unwrap_or(false),
        };
        crate::writer::html::write_html_to(&self.workbook, &mut out, &options).map_err(js)?;
        String::from_utf8(out).map_err(js)
    }

    /// One sheet as CSV, comma-separated unless `options.delimiter` says
    /// otherwise - a semicolon for a locale where the comma is the decimal
    /// mark.
    #[wasm_bindgen(js_name = toCsv)]
    pub fn to_csv(
        &self,
        sheet: usize,
        #[wasm_bindgen(unchecked_optional_param_type = "CsvOptions")] options: &JsValue,
    ) -> Result<String, JsError> {
        let delimiter = match field(options, "delimiter") {
            Some(value) => one_char(&value)?,
            None => ',',
        };
        let mut out = Vec::new();
        crate::writer::csv::write_csv_to(&self.workbook, sheet, &mut out, delimiter).map_err(js)?;
        String::from_utf8(out).map_err(js)
    }
}
