//! Reading xlsb, checked against the same workbook saved as xlsx.
//!
//! The pair lives in `tests/corpus/`, which git ignores, so with no corpus on
//! the machine this test passes having checked nothing - the same bargain
//! `tests/corpus.rs` makes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::model::{CellValue, Spreadsheet};
use excelerate::reader::Format;
use excelerate::style::NumberFormat;
use std::path::Path;

/// The workbook in both formats, or `None` when the corpus is not there.
fn pair() -> Option<(Spreadsheet, Spreadsheet)> {
    let stem = "tests/corpus/Finance_Ledger_100_Unique_Functions";
    let (binary, xml) = (format!("{stem}.xlsb"), format!("{stem}.xlsx"));
    if !Path::new(&binary).exists() || !Path::new(&xml).exists() {
        return None;
    }
    Some((
        excelerate::reader::read(&binary).expect("the xlsb reads"),
        excelerate::reader::read(&xml).expect("the xlsx reads"),
    ))
}

/// Whether a formula's result is one no two saves agree on.
fn volatile(formula: &str) -> bool {
    ["RAND(", "RANDBETWEEN(", "NOW(", "TODAY("]
        .iter()
        .any(|name| formula.contains(name))
}

/// Excel marks a name it does not know as a function of its own; the same
/// formula in the binary carries the name bare, and bare is what this crate
/// hands back - our engine knows `MAXIFS` by that name.
fn strip_marker(formula: &str) -> String {
    formula.replace("_xludf.", "")
}

#[test]
fn a_binary_workbook_reads_as_the_same_workbook_saved_as_xml() {
    let Some((binary, xml)) = pair() else {
        return;
    };
    assert_eq!(
        binary
            .sheets()
            .iter()
            .map(|s| s.title().to_owned())
            .collect::<Vec<_>>(),
        xml.sheets()
            .iter()
            .map(|s| s.title().to_owned())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        binary
            .defined_names
            .iter()
            .map(|n| (n.name.as_str(), n.formula.as_str(), n.sheet, n.hidden))
            .collect::<Vec<_>>(),
        [
            ("_xlnm._FilterDatabase", "Ledger!$A$1:$M$241", Some(0), true),
            ("Transactions", "Ledger!$A$1:$M$241", None, false),
        ]
    );

    let (mut cells, mut unread) = (0, 0);
    // The last sheet holds what three pivot tables laid out, and the two
    // files were saved from separate refreshes: theirs has five departments
    // where ours has seven. That is a difference between the workbooks, not
    // between the readers, so the comparison stops at the sheets people typed.
    for (mine, theirs) in binary.sheets().iter().zip(xml.sheets()).take(3) {
        for (at, cell) in theirs.iter() {
            cells += 1;
            let ours = mine.get(at).map(|c| c.value.clone()).unwrap_or_default();
            let expected = &cell.value;
            if let CellValue::Formula { formula, cached } = expected {
                let formula = strip_marker(formula);
                match &ours {
                    CellValue::Formula {
                        formula: ours,
                        cached: theirs,
                    } => {
                        assert_eq!(*ours, formula, "{} {at}", theirs_title(mine));
                        if !volatile(&formula) {
                            assert_eq!(theirs, cached, "{} {at}", theirs_title(mine));
                        }
                    }
                    // A formula whose tokens this crate cannot read leaves the
                    // cached value standing. Two cells here do: both hold a
                    // structured reference to a table that was deleted, which
                    // is a token kind the decompiler does not know.
                    other => {
                        assert_eq!(
                            Some(other),
                            cached.as_deref(),
                            "{} {at}",
                            theirs_title(mine)
                        );
                        unread += 1;
                    }
                }
                continue;
            }
            assert_eq!(&ours, expected, "{} {at}", theirs_title(mine));
        }
    }
    assert!(cells > 3000, "the workbook has cells: {cells}");
    assert_eq!(unread, 2, "only the two structured references go unread");

    // The style a cell points at carries its number format, which is what
    // makes one of these numbers a date.
    let ledger = binary.sheet(0).unwrap();
    let dated = ledger
        .get(excelerate::CellRef::parse("B3").unwrap())
        .unwrap();
    assert_eq!(
        binary.styles.get(dated.style).unwrap().number_format,
        NumberFormat::Custom("yyyy\\-mm\\-dd".to_owned())
    );
}

/// The sheet's name, for the message of a failed assertion.
fn theirs_title(sheet: &excelerate::model::Worksheet) -> &str {
    sheet.title()
}

#[test]
fn the_package_says_which_of_the_two_it_is_itself() {
    let stem = "tests/corpus/Finance_Ledger_100_Unique_Functions";
    if !Path::new(&format!("{stem}.xlsb")).exists() {
        return;
    }
    // Both are zips whose first bytes say nothing beyond that; the parts
    // inside are what tell them apart, so a renamed file still reads.
    assert_eq!(
        excelerate::reader::format_of(format!("{stem}.xlsb")).unwrap(),
        Format::Xlsb
    );
    assert_eq!(
        excelerate::reader::format_of(format!("{stem}.xlsx")).unwrap(),
        Format::Xlsx
    );
    let bytes = std::fs::read(format!("{stem}.xlsb")).unwrap();
    let book = excelerate::reader::read_bytes(&bytes, None).unwrap();
    assert_eq!(book.sheets().len(), 4, "read from bytes, with no name");
}
