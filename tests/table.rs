//! Tables: what a table part says about itself, that a rewrite keeps it, and
//! that an edit to the grid moves it.
//!
//! The fixture is written by excelize, not by us — a file made by the code
//! under test would only prove it agrees with itself.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::Row;
use excelerate::edit::{insert_rows, remove_rows};
use excelerate::model::Spreadsheet;
use excelerate::reader::xlsx::read_xlsx_from;
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::Cursor;

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/table.xlsx"
    ))
    .unwrap()
}

/// The row a user would call by that number.
fn row(one_based: u64) -> Row {
    Row::from_one_based(one_based).unwrap()
}

fn cycle(book: &Spreadsheet) -> Spreadsheet {
    let mut buf = Vec::new();
    write_xlsx_to(book, Cursor::new(&mut buf)).expect("workbook writes");
    read_xlsx_from(Cursor::new(buf)).expect("what we wrote reads back")
}

#[test]
fn a_table_is_read_with_its_columns_and_style() {
    let book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    let table = &book.sheets()[0].tables[0];

    assert_eq!(table.name, "Sales");
    assert_eq!(table.display_name, "Sales");
    assert_eq!(table.range.to_string(), "A1:C5");
    assert_eq!(
        table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Region", "Quarter", "Sales"]
    );
    // The file names no header or totals count, so neither is invented here.
    assert_eq!(table.header_row_count, None);
    assert_eq!(table.totals_row_count, None);
    assert_eq!(
        table.auto_filter.map(|f| f.to_string()).as_deref(),
        Some("A1:C5")
    );

    let style = table.style.as_ref().unwrap();
    assert_eq!(style.name.as_deref(), Some("TableStyleMedium2"));
    assert!(style.show_first_column);
    assert!(style.show_row_stripes);
    assert!(!style.show_column_stripes);

    // The header row is not the body: this is what an aggregate over the table
    // would run on.
    assert_eq!(table.body().unwrap().to_string(), "A2:C5");
}

#[test]
fn the_table_survives_a_cycle() {
    let before = read_xlsx_from(Cursor::new(fixture())).unwrap();
    let after = cycle(&before);
    assert_eq!(after.sheets()[0].tables, before.sheets()[0].tables);
}

/// The reason for parsing tables at all. A row inserted inside the table used
/// to move the cells and leave the table's own range where it was, so what
/// Excel drew as the table no longer covered the data.
#[test]
fn an_inserted_row_takes_the_table_with_it() {
    let mut book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    insert_rows(&mut book, 0, row(3), 2).unwrap();
    let table = &book.sheets()[0].tables[0];
    assert_eq!(table.range.to_string(), "A1:C7");
    assert_eq!(
        table.auto_filter.map(|f| f.to_string()).as_deref(),
        Some("A1:C7")
    );

    // And it is that widened range that reaches the file, not the one read.
    let after = cycle(&book);
    assert_eq!(after.sheets()[0].tables[0].range.to_string(), "A1:C7");
}

/// A table that loses part of itself narrows; one whose rows all go is gone.
#[test]
fn a_removed_row_narrows_the_table() {
    let mut book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    remove_rows(&mut book, 0, row(3), 1).unwrap();
    assert_eq!(book.sheets()[0].tables[0].range.to_string(), "A1:C4");

    let mut all = read_xlsx_from(Cursor::new(fixture())).unwrap();
    remove_rows(&mut all, 0, row(1), 5).unwrap();
    assert!(all.sheets()[0].tables.is_empty());
}

/// A chart keeps its series as formulas naming cells of a sheet. The part is
/// carried, so those went stale on every edit: a chart reading the wrong
/// cells draws the wrong picture and says nothing about it.
#[test]
fn a_chart_follows_the_cells_it_reads() {
    use excelerate::coordinate::Row;
    use excelerate::edit::{insert_rows, remove_sheet, rename_sheet};

    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/chart.xlsx"
    ))
    .unwrap();
    let series = |book: &Spreadsheet| -> Vec<String> {
        let part = book
            .parts
            .iter()
            .find(|p| p.path == "xl/charts/chart1.xml")
            .expect("the chart part is carried");
        let text = String::from_utf8(part.data.clone()).unwrap();
        text.split("<f>")
            .skip(1)
            .filter_map(|s| Some(s.split("</f>").next()?.to_owned()))
            .collect()
    };

    let before = read_xlsx_from(Cursor::new(bytes.clone())).unwrap();
    assert_eq!(
        series(&before),
        ["Sheet1!$B$1", "Sheet1!$A$2:$A$5", "Sheet1!$B$2:$B$5"]
    );

    // Two rows pushed in above the data take the series with them.
    let mut moved = read_xlsx_from(Cursor::new(bytes.clone())).unwrap();
    insert_rows(&mut moved, 0, Row::from_one_based(1).unwrap(), 2).unwrap();
    assert_eq!(
        series(&moved),
        ["Sheet1!$B$3", "Sheet1!$A$4:$A$7", "Sheet1!$B$4:$B$7"]
    );
    // And that is what reaches the file, not what was read.
    assert_eq!(series(&cycle(&moved)), series(&moved));

    // A rename follows the qualifier, a removal turns it into an error.
    let mut renamed = read_xlsx_from(Cursor::new(bytes.clone())).unwrap();
    rename_sheet(&mut renamed, 0, "Продажи").unwrap();
    assert_eq!(series(&renamed)[0], "Продажи!$B$1");

    let mut gone = read_xlsx_from(Cursor::new(bytes)).unwrap();
    gone.add_sheet(excelerate::model::Worksheet::new("Другой").unwrap())
        .unwrap();
    remove_sheet(&mut gone, 0).unwrap();
    assert_eq!(series(&gone)[0], "#REF!$B$1");
}

/// Structured references: a formula naming a table rather than its cells.
///
/// The fixture's table is `Sales`, `A1:C5`, columns Region, Quarter and
/// Sales, with the header on row 1 and no totals row.
#[test]
fn a_formula_can_name_the_table_instead_of_its_cells() {
    use excelerate::coordinate::CellRef;
    use excelerate::formula::eval::{Engine, Origin};
    use excelerate::formula::value::Value;

    let book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    let mut engine = Engine::new(&book);
    // A cell outside the table, so that `[@…]` has to fail there.
    let outside = Origin::new(0, CellRef::parse("F20").unwrap());

    let number =
        |engine: &mut Engine<'_>, at: Origin, formula: &str| -> Value { engine.eval(at, formula) };

    // The column on its own is the body of that column: rows 2 to 5.
    assert_eq!(
        number(&mut engine, outside, "SUM(Sales[Sales])"),
        Value::Number(465.0)
    );
    assert_eq!(
        number(&mut engine, outside, "COUNT(Sales[Sales])"),
        Value::Number(4.0)
    );
    // `#All` takes the header in, and a header is text, so COUNT drops it
    // while COUNTA keeps it.
    assert_eq!(
        number(&mut engine, outside, "COUNTA(Sales[[#All],[Sales]])"),
        Value::Number(5.0)
    );
    assert_eq!(
        number(&mut engine, outside, "Sales[[#Headers],[Sales]]"),
        Value::Text("Sales".into())
    );
    // A span of columns is a rectangle two columns wide.
    assert_eq!(
        number(&mut engine, outside, "COUNTA(Sales[[Region]:[Quarter]])"),
        Value::Number(8.0)
    );
    // No column at all is the whole width of the body.
    assert_eq!(
        number(&mut engine, outside, "COUNTA(Sales[#Data])"),
        Value::Number(12.0)
    );

    // `@` is the row the formula sits on, so it only works inside the table.
    let inside = Origin::new(0, CellRef::parse("E3").unwrap());
    assert_eq!(
        number(&mut engine, inside, "Sales[@Sales]"),
        Value::Number(140.0)
    );
    assert_eq!(
        number(&mut engine, outside, "Sales[@Sales]"),
        Value::Error(excelerate::error::CellError::Value)
    );

    // The table has no totals row, so asking for one is a broken reference.
    assert_eq!(
        number(&mut engine, outside, "Sales[[#Totals],[Sales]]"),
        Value::Error(excelerate::error::CellError::Ref)
    );
    // A column that is not there, and a table that is not there.
    assert_eq!(
        number(&mut engine, outside, "Sales[Profit]"),
        Value::Error(excelerate::error::CellError::Name)
    );
    assert_eq!(
        number(&mut engine, outside, "Costs[Sales]"),
        Value::Error(excelerate::error::CellError::Name)
    );

    // Written without a table name, the reference means the table the formula
    // is written inside, so it needs an origin actually in the table. `@`
    // above did not: it asks only for the row, and Excel resolves it from any
    // column on that row.
    let within = Origin::new(0, CellRef::parse("B3").unwrap());
    assert_eq!(
        number(&mut engine, within, "SUM([Sales])"),
        Value::Number(465.0)
    );
    assert_eq!(
        number(&mut engine, outside, "SUM([Sales])"),
        Value::Error(excelerate::error::CellError::Name)
    );
}
