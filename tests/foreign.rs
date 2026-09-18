//! Expectations taken from another reader's test suite.
//!
//! They carry values snapped from workbooks that Excel and `LibreOffice`
//! wrote, which makes them the only third party this crate has for xlsb and
//! ODS: excelize reads neither.
//!
//! The workbooks themselves are not committed - they are somebody else's test
//! data. Put them in `tests/corpus/foreign/` and these tests check against
//! them; without that directory each one passes having checked nothing, the
//! way `tests/corpus.rs` does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::model::{CellValue, Spreadsheet, Worksheet};
use excelerate::{CellRef, reader};

/// A workbook from the corpus, or `None` when the corpus is not on this
/// machine.
fn book(name: &str) -> Option<Spreadsheet> {
    let path = format!("tests/corpus/foreign/{name}");
    if !std::path::Path::new(&path).exists() {
        return None;
    }
    Some(reader::read(&path).unwrap_or_else(|e| panic!("{name} reads: {e}")))
}

fn sheet<'a>(book: &'a Spreadsheet, title: &str) -> &'a Worksheet {
    book.sheets()
        .iter()
        .find(|s| s.title() == title)
        .unwrap_or_else(|| panic!("no sheet {title:?}"))
}

/// The value of one cell, with a formula standing for its cached result: that
/// is what the other reader's ranges hold.
fn value(sheet: &Worksheet, address: &str) -> CellValue {
    let at = CellRef::parse(address).unwrap();
    match sheet.get(at).map(|c| &c.value) {
        Some(CellValue::Formula { cached, .. }) => {
            cached.as_deref().cloned().unwrap_or(CellValue::Empty)
        }
        Some(other) => other.clone(),
        None => CellValue::Empty,
    }
}

fn text(sheet: &Worksheet, address: &str) -> String {
    match value(sheet, address) {
        CellValue::Text(s) => s.to_string(),
        other => panic!("{address} is {other:?}, not text"),
    }
}

fn number(sheet: &Worksheet, address: &str) -> f64 {
    match value(sheet, address) {
        CellValue::Number(n) => n,
        other => panic!("{address} is {other:?}, not a number"),
    }
}

#[test]
fn xlsb_values_match_the_other_reader() {
    let Some(book) = book("issues.xlsb") else {
        return;
    };
    let issue2 = sheet(&book, "issue2");
    for (address, n) in [("A1", 1.0), ("A2", 2.0), ("A3", 3.0)] {
        assert!(
            (number(issue2, address) - n).abs() < f64::EPSILON,
            "{address}"
        );
    }
    for (address, s) in [("B1", "a"), ("B2", "b"), ("B3", "c")] {
        assert_eq!(text(issue2, address), s);
    }

    // Characters that XML escapes, and characters outside the basic plane.
    let spc = sheet(&book, "spc_chrs");
    for (address, s) in [
        ("A1", "&"),
        ("A2", "<"),
        ("A3", ">"),
        ("A4", "aaa ' aaa"),
        ("A5", "\""),
        ("A6", "☺"),
        ("A7", "֍"),
        ("A8", "àâéêèçöïî«»"),
    ] {
        assert_eq!(text(spc, address), s, "{address}");
    }

    let mut names: Vec<_> = book
        .defined_names
        .iter()
        .map(|n| (n.name.clone(), n.formula.clone()))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            ("MyBrokenRange".to_owned(), "Sheet1!#REF!".to_owned()),
            ("MyDataTypes".to_owned(), "datatypes!$A$1:$A$6".to_owned()),
            ("OneRange".to_owned(), "Sheet1!$A$1".to_owned()),
        ]
    );

    let formula = sheet(&book, "Sheet1");
    let at = CellRef::parse("A2").unwrap();
    let CellValue::Formula { formula, .. } = &formula.get(at).unwrap().value else {
        panic!("A2 holds a formula");
    };
    assert_eq!(formula, "B1+OneRange");
}

#[test]
fn xlsb_cached_results_keep_their_type() {
    let Some(book) = book("issue_182.xlsb") else {
        return;
    };
    let sheet = sheet(&book, "formula_vals");
    assert_eq!(value(sheet, "A1"), CellValue::Number(3.0));
    assert_eq!(text(sheet, "A2"), "Ab");
    assert_eq!(value(sheet, "A3"), CellValue::Bool(false));
}

#[test]
fn xlsb_floats_survive_their_encoding() {
    let Some(book) = book("issue_186.xlsb") else {
        return;
    };
    let sheet = sheet(&book, "Sheet1");
    for (address, n) in [
        ("A1", 1.23),
        ("A2", 12.34),
        ("A3", 123.45),
        ("A4", 1234.56),
        ("A5", 12345.67),
    ] {
        assert!((number(sheet, address) - n).abs() < 1e-9, "{address}");
    }
}

#[test]
fn xlsb_dates_follow_the_workbook_epoch() {
    use excelerate::shared::date::Epoch;

    for (name, epoch, first) in [
        ("date.xlsb", Epoch::Windows1900, 44197.0),
        ("date_1904.xlsb", Epoch::Mac1904, 42735.0),
    ] {
        let Some(book) = book(name) else {
            return;
        };
        assert_eq!(book.epoch, epoch, "{name}");
        let sheet = &book.sheets()[0];
        assert!((number(sheet, "A1") - first).abs() < 1e-9, "{name}");
        // A duration rather than a date: 255:10:10 as a fraction of a day.
        assert!(
            (number(sheet, "A3") - 10.632_060_185_185_2).abs() < 1e-9,
            "{name}"
        );
    }
}

#[test]
fn xlsb_sheets_keep_their_visibility() {
    use excelerate::model::SheetVisibility::{Hidden, VeryHidden, Visible};

    let Some(book) = book("any_sheets.xlsb") else {
        return;
    };
    let seen: Vec<_> = book
        .sheets()
        .iter()
        .map(|s| (s.title().to_owned(), s.visibility))
        .collect();
    assert_eq!(
        seen,
        [
            ("Visible".to_owned(), Visible),
            ("Hidden".to_owned(), Hidden),
            ("VeryHidden".to_owned(), VeryHidden),
            ("Chart".to_owned(), Visible),
        ]
    );
}

/// Both of these hold a workbook record this reader does not know: one cost
/// another reader every sheet and made the other panic. The record's declared
/// length is what carries a reader past it.
#[test]
fn xlsb_survives_a_record_it_does_not_know() {
    for name in [
        "issue_666_lost_sheets.xlsb",
        "issue_666_panic.xlsb",
        "issue_419.xlsb",
    ] {
        let Some(book) = book(name) else {
            return;
        };
        assert_eq!(book.sheets().len(), 1, "{name}");
        assert_eq!(book.sheets()[0].title(), "Sheet1", "{name}");
    }
    if let Some(book) = book("issue_419.xlsb") {
        assert_eq!(text(&book.sheets()[0], "A1"), "Hello");
    }
}

#[test]
fn ods_values_match_the_other_reader() {
    let Some(book) = book("issues.ods") else {
        return;
    };
    let types = sheet(&book, "datatypes");
    assert_eq!(value(types, "A1"), CellValue::Number(1.0));
    assert_eq!(value(types, "A2"), CellValue::Number(1.5));
    assert_eq!(text(types, "A3"), "ab"); // =CONCATENATE("a","b")
    assert_eq!(value(types, "A4"), CellValue::Bool(false));
    assert_eq!(text(types, "A5"), "test");
    assert_eq!(value(types, "A6"), CellValue::Number(42663.0)); // 2016-10-20

    let issue2 = sheet(&book, "issue2");
    assert!((number(issue2, "A1") - 1.0).abs() < f64::EPSILON);
    assert_eq!(text(issue2, "B3"), "c");
    assert!((number(sheet(&book, "issue5"), "A1") - 0.5).abs() < f64::EPSILON);
}

/// A cell inside a merge may hold a value of its own: the merge hides it, it
/// does not erase it, and a reader that drops it loses what the file says.
#[test]
fn ods_keeps_the_value_of_a_covered_cell() {
    let Some(book) = book("covered.ods") else {
        return;
    };
    let sheet = sheet(&book, "sheet1");
    assert_eq!(text(sheet, "A1"), "a1");
    assert_eq!(text(sheet, "A2"), "a2");
    assert_eq!(text(sheet, "A3"), "a3");

    // And it survives a round trip through this crate's own writer.
    let mut bytes = Vec::new();
    excelerate::writer::write_ods_to(&book, std::io::Cursor::new(&mut bytes)).unwrap();
    let reread = reader::read_bytes(&bytes, Some("covered.ods")).unwrap();
    assert_eq!(text(&reread.sheets()[0], "A3"), "a3");
}

#[test]
fn ods_repeated_rows_land_where_they_belong() {
    let Some(book) = book("number_rows_repeated.ods") else {
        return;
    };
    let sheet = &book.sheets()[0];
    assert_eq!(text(sheet, "A1"), "A");
    assert_eq!(text(sheet, "B1"), "B");
    for row in ["2", "3", "6", "8"] {
        assert_eq!(text(sheet, &format!("A{row}")), "C", "row {row}");
        assert_eq!(text(sheet, &format!("B{row}")), "D", "row {row}");
    }
    for row in ["4", "5", "7"] {
        assert_eq!(
            value(sheet, &format!("A{row}")),
            CellValue::Empty,
            "row {row}"
        );
    }
}

/// Text split into several formatted runs is one string, not the first run.
#[test]
fn ods_reads_the_whole_of_a_formatted_string() {
    let Some(book) = book("richtext_issue.ods") else {
        return;
    };
    assert_eq!(text(sheet(&book, "datatypes"), "A1"), "abc");
}

/// A repeat count is a number in the file: this one asks for a billion rows of
/// a billion cells out of thirteen hundred bytes. It must be read in a moment,
/// not in a lifetime.
#[test]
fn ods_refuses_to_expand_a_repeat_count_forever() {
    if book("issue_594_dos.ods").is_none() {
        return;
    }
    let started = std::time::Instant::now();
    let book = book("issue_594_dos.ods").unwrap();
    assert_eq!(book.sheets().len(), 1);
    assert!(started.elapsed().as_secs() < 30, "{:?}", started.elapsed());
}
