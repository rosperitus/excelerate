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

#[test]
fn inserted_cells_take_the_style_beside_them() {
    use excelerate::edit::{Axis, CopyOrigin, insert_cells_with};
    use excelerate::style::{Font, Style};

    let mut book = excelerate::model::Spreadsheet::new();
    let bold = book.styles.intern(Style {
        font: Font {
            bold: true,
            ..Default::default()
        },
        ..Default::default()
    });
    let at = |a: &str| excelerate::CellRef::parse(a).unwrap();
    book.sheet_mut(0).unwrap().entry(at("B1")).style = bold;
    let area = excelerate::Range::parse("B2:B3").unwrap();
    insert_cells_with(&mut book, 0, area, Axis::Rows, CopyOrigin::Before).unwrap();
    let sheet = book.sheet(0).unwrap();
    assert_eq!(sheet.get(at("B2")).unwrap().style, bold);
    assert_eq!(sheet.get(at("B3")).unwrap().style, bold);
    assert!(sheet.get(at("C2")).is_none());
}

#[test]
fn a_sort_orders_rows_the_way_excel_does() {
    use excelerate::edit::{SortKey, sort_range};

    let at = |a: &str| excelerate::CellRef::parse(a).unwrap();
    let col = |a: &str| excelerate::Col::from_letters(a).unwrap();
    let mut book = excelerate::model::Spreadsheet::new();
    let sheet = book.sheet_mut(0).unwrap();
    // A: key, B: a formula reading its own row.
    for (row, key) in [
        (1, CellValue::text("b")),
        (2, CellValue::Number(3.0)),
        (4, CellValue::text("A")),
        (5, CellValue::Number(1.0)),
    ] {
        sheet.set(at(&format!("A{row}")), key);
    }
    for row in 1..=5 {
        sheet.set(
            at(&format!("B{row}")),
            CellValue::Formula {
                formula: format!("A{row}"),
                cached: None,
            },
        );
    }
    let area = excelerate::Range::parse("A1:B5").unwrap();
    sort_range(&mut book, 0, area, &[SortKey::column(col("A"))]).unwrap();

    let sheet = book.sheet(0).unwrap();
    let value = |a: &str| sheet.get(at(a)).map(|c| c.value.clone());
    assert_eq!(value("A1"), Some(CellValue::Number(1.0)));
    assert_eq!(value("A2"), Some(CellValue::Number(3.0)));
    assert_eq!(value("A3"), Some(CellValue::text("A")));
    assert_eq!(value("A4"), Some(CellValue::text("b")));
    assert_eq!(value("A5"), None, "the empty row goes last");
    for row in 1..=5 {
        let Some(CellValue::Formula { formula, .. }) = value(&format!("B{row}")) else {
            panic!("B{row} lost its formula");
        };
        assert_eq!(formula, format!("A{row}"), "B{row} still reads its own row");
    }

    let bad = sort_range(
        &mut book,
        0,
        area,
        &[SortKey::column(col("C")).descending()],
    );
    assert!(bad.is_err());
}

#[test]
fn fill_down_and_right_copy_the_first_line() {
    use excelerate::edit::{Axis, fill};

    let at = |a: &str| excelerate::CellRef::parse(a).unwrap();
    let mut book = excelerate::model::Spreadsheet::new();
    let sheet = book.sheet_mut(0).unwrap();
    sheet.set(
        at("B1"),
        CellValue::Formula {
            formula: "A1*2".into(),
            cached: None,
        },
    );
    fill(
        &mut book,
        0,
        excelerate::Range::parse("B1:B3").unwrap(),
        Axis::Rows,
    )
    .unwrap();
    fill(
        &mut book,
        0,
        excelerate::Range::parse("B3:C3").unwrap(),
        Axis::Columns,
    )
    .unwrap();

    let sheet = book.sheet(0).unwrap();
    let formula = |a: &str| match &sheet.get(at(a)).unwrap().value {
        CellValue::Formula { formula, .. } => formula.clone(),
        other => panic!("{a} is {other:?}"),
    };
    assert_eq!(formula("B2"), "A2*2");
    assert_eq!(formula("B3"), "A3*2");
    assert_eq!(formula("C3"), "B3*2");
}

#[test]
fn a_sort_can_keep_a_header_name_its_keys_and_run_across() {
    use excelerate::edit::{Axis, SortKey, SortOptions, sort_range_with};

    let at = |a: &str| CellRef::parse(a).unwrap();
    let mut book = Spreadsheet::new();
    let sheet = book.sheet_mut(0).unwrap();
    for (address, value) in [
        ("A1", CellValue::text("Name")),
        ("B1", CellValue::text("Score")),
        ("A2", CellValue::text("ann")),
        ("B2", CellValue::Number(5.0)),
        ("A3", CellValue::text("bob")),
        ("B3", CellValue::Number(9.0)),
        ("A4", CellValue::text("cy")),
        ("B4", CellValue::Number(5.0)),
    ] {
        sheet.set(at(address), value);
    }
    let options = SortOptions {
        header: true,
        ..SortOptions::default()
    };
    let keys = [
        SortKey::header("score").descending(),
        SortKey::header("Name").descending(),
    ];
    sort_range_with(&mut book, 0, Range::parse("A1:B4").unwrap(), &keys, options).unwrap();
    let text =
        |book: &Spreadsheet, a: &str| book.sheet(0).unwrap().get(at(a)).unwrap().value.clone();
    assert_eq!(
        text(&book, "A1"),
        CellValue::text("Name"),
        "the header stays"
    );
    assert_eq!(text(&book, "A2"), CellValue::text("bob"));
    assert_eq!(text(&book, "A3"), CellValue::text("cy"));
    assert_eq!(text(&book, "A4"), CellValue::text("ann"));

    // Across: the columns of row 1 reorder by it.
    let mut book = Spreadsheet::new();
    let sheet = book.sheet_mut(0).unwrap();
    for (address, n) in [
        ("A1", 3.0),
        ("B1", 1.0),
        ("C1", 2.0),
        ("A2", 30.0),
        ("B2", 10.0),
    ] {
        sheet.set(at(address), CellValue::Number(n));
    }
    let across = SortOptions {
        orientation: Axis::Columns,
        ..SortOptions::default()
    };
    let key = [SortKey::row(excelerate::Row::from_one_based(1).unwrap())];
    sort_range_with(&mut book, 0, Range::parse("A1:C2").unwrap(), &key, across).unwrap();
    assert_eq!(text(&book, "A1"), CellValue::Number(1.0));
    assert_eq!(text(&book, "A2"), CellValue::Number(10.0));
    assert_eq!(text(&book, "C2"), CellValue::Number(30.0));
    assert!(book.sheet(0).unwrap().get(at("B2")).is_none());
}

#[test]
fn fill_series_continues_what_the_seed_starts() {
    use excelerate::edit::{Axis, fill_series};

    let at = |a: &str| CellRef::parse(a).unwrap();
    let mut book = Spreadsheet::new();
    let sheet = book.sheet_mut(0).unwrap();
    for (address, value) in [
        ("A1", CellValue::Number(1.0)),
        ("A2", CellValue::Number(3.0)),
        ("B1", CellValue::text("Кв1")),
        ("C1", CellValue::text("Jan")),
        ("D1", CellValue::text("ПН")),
        ("E1", CellValue::text("Item 007")),
        ("F1", CellValue::Number(5.0)),
        ("G1", CellValue::text("x")),
        ("G2", CellValue::text("y")),
    ] {
        sheet.set(at(address), value);
    }
    fill_series(&mut book, 0, Range::parse("A1:G4").unwrap(), Axis::Rows).unwrap();
    let sheet = book.sheet(0).unwrap();
    let value = |a: &str| sheet.get(at(a)).unwrap().value.clone();
    assert_eq!(value("A4"), CellValue::Number(7.0));
    assert_eq!(value("B4"), CellValue::text("Кв4"));
    assert_eq!(value("C4"), CellValue::text("Apr"));
    assert_eq!(value("D2"), CellValue::text("ВТ"));
    assert_eq!(value("E3"), CellValue::text("Item 009"));
    assert_eq!(value("F4"), CellValue::Number(5.0), "one number is copied");
    assert_eq!(value("G3"), CellValue::text("x"));
    assert_eq!(value("G4"), CellValue::text("y"));
}

#[test]
fn a_table_sorts_by_its_column_names() {
    use excelerate::edit::{SortKey, sort_table};
    use excelerate::model::table::{Table, TableColumn};

    let at = |a: &str| CellRef::parse(a).unwrap();
    let mut book = Spreadsheet::new();
    let sheet = book.sheet_mut(0).unwrap();
    for (address, value) in [
        ("A1", CellValue::text("Item")),
        ("B1", CellValue::text("Qty")),
        ("A2", CellValue::text("pen")),
        ("B2", CellValue::Number(2.0)),
        ("A3", CellValue::text("cup")),
        ("B3", CellValue::Number(7.0)),
    ] {
        sheet.set(at(address), value);
    }
    sheet.tables.push(Table {
        id: 1,
        name: "Stock".into(),
        display_name: "Stock".into(),
        range: Range::parse("A1:B3").unwrap(),
        columns: ["Item", "Qty"]
            .iter()
            .zip(1..)
            .map(|(name, id)| TableColumn {
                id,
                name: (*name).into(),
                totals_row_function: None,
                totals_row_label: None,
                calculated_formula: None,
            })
            .collect(),
        header_row_count: None,
        totals_row_count: None,
        auto_filter: None,
        style: None,
    });
    sort_table(&mut book, "stock", &[SortKey::header("Qty").descending()]).unwrap();
    let sheet = book.sheet(0).unwrap();
    assert_eq!(sheet.get(at("A2")).unwrap().value, CellValue::text("cup"));
    assert_eq!(sheet.get(at("A1")).unwrap().value, CellValue::text("Item"));
    assert!(sort_table(&mut book, "nope", &[]).is_err());
}
