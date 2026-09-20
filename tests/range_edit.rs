//! Copying and moving a rectangle of cells, and moving a sheet along the tab
//! bar.
//!
//! The rules here are Excel's, and the pair of them is the whole point: a copy
//! rewrites the formulas it carries, a move keeps them pointing where they
//! pointed and drags the references from elsewhere along instead.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::{CellRef, Range};
use excelerate::edit::{Axis, copy_range, insert_cells, move_range, move_sheet, remove_cells};
use excelerate::model::{CellValue, DefinedName, Spreadsheet, Worksheet};

fn at(address: &str) -> CellRef {
    CellRef::parse(address).unwrap()
}

fn area(range: &str) -> Range {
    Range::parse(range).unwrap()
}

/// What a cell says: a formula as its text, a value as its display.
fn text(book: &Spreadsheet, sheet: usize, address: &str) -> String {
    match book
        .sheet(sheet)
        .unwrap()
        .get(at(address))
        .map(|c| &c.value)
    {
        Some(CellValue::Formula { formula, .. }) => format!("={formula}"),
        Some(CellValue::Number(n)) => n.to_string(),
        Some(CellValue::Text(t)) => t.to_string(),
        Some(CellValue::Empty) | None => String::new(),
        other => format!("{other:?}"),
    }
}

/// A sheet with numbers in A1:A3 and a formula over them in B1.
fn book() -> Spreadsheet {
    let mut wb = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Data").unwrap();
    for (cell, value) in [("A1", 1.0), ("A2", 2.0), ("A3", 3.0)] {
        sheet.set(at(cell), CellValue::Number(value));
    }
    sheet.set(
        at("B1"),
        CellValue::Formula {
            formula: "SUM(A1:A3)".to_owned(),
            cached: Some(Box::new(CellValue::Number(6.0))),
        },
    );
    wb.add_sheet(sheet).unwrap();
    wb
}

#[test]
fn a_copied_formula_is_rewritten_as_if_it_had_been_written_there() {
    let mut wb = book();
    copy_range(&mut wb, 0, area("A1:B1"), 0, at("D5")).unwrap();

    assert_eq!(text(&wb, 0, "D5"), "1", "the value came along");
    assert_eq!(
        text(&wb, 0, "E5"),
        "=SUM(D5:D7)",
        "relative references moved with the formula"
    );
    assert_eq!(text(&wb, 0, "B1"), "=SUM(A1:A3)", "the original stayed");
    assert!(
        matches!(
            wb.sheet(0).unwrap().get(at("E5")).map(|c| &c.value),
            Some(CellValue::Formula { cached: None, .. })
        ),
        "the cache answered the formula as it was written"
    );
}

#[test]
fn a_dollar_sign_survives_a_copy_and_a_cell_pushed_off_the_sheet_does_not() {
    let mut wb = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(
        at("B2"),
        CellValue::Formula {
            formula: "$A$1+A1".to_owned(),
            cached: None,
        },
    );
    wb.add_sheet(sheet).unwrap();

    copy_range(&mut wb, 0, area("B2:B2"), 0, at("C3")).unwrap();
    assert_eq!(text(&wb, 0, "C3"), "=$A$1+B2");

    // A1 one row up is off the sheet, which is #REF! in Excel too.
    copy_range(&mut wb, 0, area("B2:B2"), 0, at("B1")).unwrap();
    assert_eq!(text(&wb, 0, "B1"), "=$A$1+#REF!");
}

#[test]
fn an_empty_source_cell_empties_the_target() {
    let mut wb = book();
    wb.sheet_mut(0)
        .unwrap()
        .set(at("D2"), CellValue::Number(99.0));
    // A2 holds 2, A4 holds nothing; pasting the pair over D1:D2 clears D2.
    copy_range(&mut wb, 0, area("A2:A4"), 0, at("D1")).unwrap();
    assert_eq!(text(&wb, 0, "D1"), "2");
    assert_eq!(text(&wb, 0, "D2"), "3");
    assert_eq!(text(&wb, 0, "D3"), "", "the empty cell was pasted too");
}

#[test]
fn a_moved_formula_keeps_pointing_where_it_pointed() {
    let mut wb = book();
    move_range(&mut wb, 0, area("B1:B1"), 0, at("D5")).unwrap();

    assert_eq!(
        text(&wb, 0, "D5"),
        "=SUM(A1:A3)",
        "a cut formula reads the same cells"
    );
    assert_eq!(text(&wb, 0, "B1"), "", "the source is empty");
}

#[test]
fn references_into_a_moved_block_follow_it() {
    let mut wb = book();
    wb.sheet_mut(0).unwrap().set(
        at("C1"),
        CellValue::Formula {
            formula: "A1*2".to_owned(),
            cached: Some(Box::new(CellValue::Number(2.0))),
        },
    );
    move_range(&mut wb, 0, area("A1:A3"), 0, at("F10")).unwrap();

    assert_eq!(text(&wb, 0, "F10"), "1", "the values moved");
    assert_eq!(text(&wb, 0, "A1"), "", "the source is empty");
    assert_eq!(
        text(&wb, 0, "C1"),
        "=F10*2",
        "a formula elsewhere followed the cell it reads"
    );
    assert_eq!(
        text(&wb, 0, "B1"),
        "=SUM(F10:F12)",
        "so did the range, which moved whole"
    );
}

#[test]
fn a_move_to_another_sheet_names_the_sheet_the_formula_came_from() {
    let mut wb = book();
    wb.add_sheet(Worksheet::new("Other").unwrap()).unwrap();
    move_range(&mut wb, 0, area("B1:B1"), 1, at("A1")).unwrap();

    assert_eq!(
        text(&wb, 1, "A1"),
        "=SUM(Data!A1:Data!A3)",
        "what it read without naming a sheet is now named"
    );
}

#[test]
fn a_moved_block_takes_its_merges_with_it() {
    let mut wb = book();
    wb.sheet_mut(0).unwrap().merges.push(area("A1:A2"));
    move_range(&mut wb, 0, area("A1:A3"), 0, at("C1")).unwrap();

    let merges = &wb.sheet(0).unwrap().merges;
    assert_eq!(merges.len(), 1);
    assert_eq!(merges[0].to_string(), "C1:C2");
}

#[test]
fn moving_a_sheet_renumbers_what_counts_tabs_and_leaves_the_formulas_alone() {
    let mut wb = Spreadsheet::empty();
    for name in ["One", "Two", "Three"] {
        wb.add_sheet(Worksheet::new(name).unwrap()).unwrap();
    }
    wb.sheet_mut(2).unwrap().set(
        at("A1"),
        CellValue::Formula {
            formula: "One!A1".to_owned(),
            cached: None,
        },
    );
    wb.defined_names.push(DefinedName {
        name: "Local".to_owned(),
        sheet: Some(2),
        formula: "Three!$A$1".to_owned(),
        hidden: false,
    });
    wb.set_active(2).unwrap();

    move_sheet(&mut wb, 2, 0).unwrap();

    assert_eq!(
        wb.sheets().iter().map(Worksheet::title).collect::<Vec<_>>(),
        ["Three", "One", "Two"]
    );
    assert_eq!(
        text(&wb, 0, "A1"),
        "=One!A1",
        "a sheet is named, not numbered"
    );
    assert_eq!(wb.defined_names[0].sheet, Some(0), "the name moved with it");
    assert_eq!(wb.active_index(), 0, "and so did the active tab");
    assert!(move_sheet(&mut wb, 0, 9).is_err());
}

#[test]
fn a_block_that_would_land_off_the_sheet_is_refused() {
    let mut wb = book();
    let end = at("XFD1048576");
    assert!(copy_range(&mut wb, 0, area("A1:B2"), 0, end).is_err());
    assert!(move_range(&mut wb, 0, area("A1:B2"), 0, end).is_err());
    assert!(copy_range(&mut wb, 0, area("A1:A1"), 9, at("A1")).is_err());
}

#[test]
fn inserting_cells_pushes_only_the_columns_the_area_spans() {
    let mut wb = book();
    wb.sheet_mut(0)
        .unwrap()
        .set(at("B3"), CellValue::Text("beside".into()));

    // A1:A1 down: A1 goes to A2, and B keeps its own cells where they were.
    insert_cells(&mut wb, 0, area("A1:A1"), Axis::Rows).unwrap();

    assert_eq!(text(&wb, 0, "A1"), "", "the hole is blank");
    assert_eq!(text(&wb, 0, "A2"), "1");
    assert_eq!(text(&wb, 0, "A4"), "3");
    assert_eq!(text(&wb, 0, "B3"), "beside", "column B did not move");
    assert_eq!(
        text(&wb, 0, "B1"),
        "=SUM(A2:A4)",
        "the range travelled whole, so the reference followed"
    );
}

#[test]
fn removing_cells_pulls_the_rest_back_over_the_hole() {
    let mut wb = book();
    remove_cells(&mut wb, 0, area("A1:A1"), Axis::Rows).unwrap();

    assert_eq!(text(&wb, 0, "A1"), "2", "what was below moved up");
    assert_eq!(text(&wb, 0, "A2"), "3");
    assert_eq!(text(&wb, 0, "A3"), "", "and left the last row empty");
}

#[test]
fn cells_can_be_pushed_sideways_too() {
    let mut wb = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(at("A1"), CellValue::Number(1.0));
    sheet.set(at("B1"), CellValue::Number(2.0));
    sheet.set(at("A2"), CellValue::Number(3.0));
    wb.add_sheet(sheet).unwrap();

    insert_cells(&mut wb, 0, area("A1:A1"), Axis::Columns).unwrap();

    assert_eq!(text(&wb, 0, "A1"), "");
    assert_eq!(text(&wb, 0, "B1"), "1");
    assert_eq!(text(&wb, 0, "C1"), "2");
    assert_eq!(text(&wb, 0, "A2"), "3", "row 2 stayed put");
}

#[test]
fn a_push_that_would_shove_data_off_the_sheet_is_refused() {
    let mut wb = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(at("A1048576"), CellValue::Number(1.0));
    wb.add_sheet(sheet).unwrap();

    assert!(insert_cells(&mut wb, 0, area("A1:A1"), Axis::Rows).is_err());
    assert_eq!(text(&wb, 0, "A1048576"), "1", "and nothing was touched");
}
