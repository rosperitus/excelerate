//! Any text as a formula: parsed, then computed in a small workbook with
//! numbers, text and a defined name to read.

#![no_main]

use excelerate::CellRef;
use excelerate::formula::eval::{Engine, Origin};
use excelerate::model::{DefinedName, Spreadsheet};
use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

fn book() -> &'static Spreadsheet {
    static BOOK: OnceLock<Spreadsheet> = OnceLock::new();
    BOOK.get_or_init(|| {
        let mut book = Spreadsheet::new();
        if let Some(sheet) = book.sheet_mut(0) {
            for (i, at) in ["A1", "A2", "A3", "B1", "B2"].iter().enumerate() {
                if let Ok(at) = CellRef::parse(at) {
                    #[allow(clippy::cast_precision_loss)]
                    sheet.set(at, i as f64 - 1.0);
                }
            }
            if let Ok(at) = CellRef::parse("C1") {
                sheet.set(at, "text");
            }
        }
        book.defined_names.push(DefinedName {
            name: "Data".to_owned(),
            sheet: None,
            formula: "Worksheet!$A$1:$B$3".to_owned(),
            hidden: false,
        });
        book
    })
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if excelerate::formula::parser::parse(text).is_err() {
        return;
    }
    let book = book();
    let mut engine = Engine::new(book);
    let at = CellRef::parse("Z99").unwrap_or_default();
    let _ = engine.eval(Origin::new(0, at), text);
});
