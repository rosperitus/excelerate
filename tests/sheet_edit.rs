//! Renaming and removing a sheet, and what that does to the formulas naming it.
//!
//! Both used to be silent corruption: `set_title` changed the tab and left
//! every reference pointing at a name no longer in the book, and there was no
//! way to remove a sheet at all without leaving the references dangling.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::CellRef;
use excelerate::edit::{remove_sheet, rename_sheet};
use excelerate::model::{CellValue, DefinedName, Spreadsheet, Worksheet};

fn at(address: &str) -> CellRef {
    CellRef::parse(address).unwrap()
}

/// A book of four sheets where the first one points at all the others.
fn book(formulas: &[(&str, &str)]) -> Spreadsheet {
    let mut wb = Spreadsheet::empty();
    let mut first = Worksheet::new("Main").unwrap();
    for (cell, formula) in formulas {
        first.set(
            at(cell),
            CellValue::Formula {
                formula: (*formula).to_owned(),
                cached: None,
            },
        );
    }
    wb.add_sheet(first).unwrap();
    for name in ["Data", "Отчёт 2024", "Tail"] {
        wb.add_sheet(Worksheet::new(name).unwrap()).unwrap();
    }
    wb
}

fn formula(book: &Spreadsheet, sheet: usize, cell: &str) -> String {
    match &book.sheet(sheet).unwrap().get(at(cell)).unwrap().value {
        CellValue::Formula { formula, .. } => formula.clone(),
        other => panic!("expected a formula, found {other:?}"),
    }
}

#[test]
fn renaming_follows_every_reference_to_the_sheet() {
    let mut wb = book(&[
        ("A1", "Data!A1+1"),
        ("A2", "SUM(Data!A1:B9)"),
        ("A3", "'Отчёт 2024'!C3"),
        // A name that merely looks like the sheet must be left alone: the
        // function, the text and the defined name all spell it.
        ("A4", "DATA(1)+\"Data!A1\"+Data_2"),
        // A 3-D span renames whichever of its ends is the sheet.
        ("A5", "SUM(Data:Tail!A1)"),
    ]);
    rename_sheet(&mut wb, 1, "Продажи").unwrap();

    assert_eq!(wb.sheet(1).unwrap().title(), "Продажи");
    // Cyrillic letters need no quotes, and Excel writes the name bare too.
    assert_eq!(formula(&wb, 0, "A1"), "Продажи!A1+1");
    assert_eq!(formula(&wb, 0, "A2"), "SUM(Продажи!A1:B9)");
    assert_eq!(formula(&wb, 0, "A3"), "'Отчёт 2024'!C3");
    assert_eq!(formula(&wb, 0, "A4"), "DATA(1)+\"Data!A1\"+Data_2");
    assert_eq!(formula(&wb, 0, "A5"), "SUM(Продажи:Tail!A1)");
}

/// A name that no longer needs its quotes loses them, which is how Excel
/// writes it back.
#[test]
fn a_quoted_name_is_requoted_only_when_it_has_to_be() {
    let mut wb = book(&[("A1", "'Отчёт 2024'!A1")]);
    rename_sheet(&mut wb, 2, "Report").unwrap();
    assert_eq!(formula(&wb, 0, "A1"), "Report!A1");

    rename_sheet(&mut wb, 2, "O'Neil").unwrap();
    assert_eq!(formula(&wb, 0, "A1"), "'O''Neil'!A1");
}

#[test]
fn renaming_to_a_taken_name_changes_nothing() {
    let mut wb = book(&[("A1", "Data!A1")]);
    assert!(rename_sheet(&mut wb, 1, "Tail").is_err());
    assert_eq!(wb.sheet(1).unwrap().title(), "Data");
    assert_eq!(formula(&wb, 0, "A1"), "Data!A1");
}

#[test]
fn a_removed_sheet_leaves_ref_errors_behind() {
    let mut wb = book(&[
        ("A1", "Data!A1+1"),
        ("A2", "SUM(Data!A1:B9)+Tail!A1"),
        ("A3", "\"Data!A1\""),
    ]);
    wb.defined_names.push(DefinedName {
        name: "Rate".into(),
        sheet: None,
        formula: "Data!A1".into(),
        hidden: false,
    });
    // A name belonging to the sheet after the one removed keeps its sheet.
    wb.defined_names.push(DefinedName {
        name: "Local".into(),
        sheet: Some(3),
        formula: "A1".into(),
        hidden: false,
    });
    remove_sheet(&mut wb, 1).unwrap();

    assert_eq!(wb.sheets().len(), 3);
    assert_eq!(formula(&wb, 0, "A1"), "#REF!A1+1");
    assert_eq!(formula(&wb, 0, "A2"), "SUM(#REF!A1:B9)+Tail!A1");
    // Text is text, whatever it spells.
    assert_eq!(formula(&wb, 0, "A3"), "\"Data!A1\"");
    assert_eq!(wb.defined_names[0].formula, "#REF!A1");
    assert_eq!(wb.defined_names[1].sheet, Some(2));
}

/// A span narrows to what is left of it, and only collapses when nothing is.
#[test]
fn a_three_dimensional_span_narrows() {
    let mut wb = book(&[("A1", "SUM(Data:Tail!A1)")]);
    remove_sheet(&mut wb, 1).unwrap();
    assert_eq!(formula(&wb, 0, "A1"), "SUM('Отчёт 2024':Tail!A1)");

    let mut gone = book(&[("A1", "SUM(Data:Data!A1)")]);
    remove_sheet(&mut gone, 1).unwrap();
    assert_eq!(formula(&gone, 0, "A1"), "SUM(#REF!A1)");
}

/// A sheet the removed one sat inside keeps its span: the ends are still
/// there, and the span simply covers one sheet fewer.
#[test]
fn removing_from_inside_a_span_leaves_the_ends_alone() {
    let mut wb = book(&[("A1", "SUM(Main:Tail!A1)")]);
    remove_sheet(&mut wb, 1).unwrap();
    assert_eq!(formula(&wb, 0, "A1"), "SUM(Main:Tail!A1)");
}

#[test]
fn the_last_sheet_cannot_be_removed() {
    let mut wb = Spreadsheet::empty();
    wb.add_sheet(Worksheet::new("Only").unwrap()).unwrap();
    assert!(remove_sheet(&mut wb, 0).is_err());
    assert_eq!(wb.sheets().len(), 1);
}

/// The tab that was open must stay inside the list, or the written file names
/// a sheet that is not there.
#[test]
fn the_active_tab_stays_in_range() {
    let mut wb = book(&[]);
    wb.set_active(3).unwrap();
    remove_sheet(&mut wb, 3).unwrap();
    assert_eq!(wb.active_sheet().unwrap().title(), "Отчёт 2024");
}
