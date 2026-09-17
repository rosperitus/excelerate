//! Pivot tables: what a report says about itself, and that a rewrite keeps it.
//!
//! The fixture is written by excelize, not by us - a file made by the code
//! under test would only prove it agrees with itself.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::CellRef;
use excelerate::model::Spreadsheet;
use excelerate::model::pivot::{
    CacheField, CacheSource, DataField, PivotAxis, PivotCache, PivotField, PivotTable, Subtotal,
};
use excelerate::reader::xlsx::read_xlsx_from;
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::Cursor;

fn at(address: &str) -> CellRef {
    CellRef::parse(address).unwrap()
}

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/pivot.xlsx"
    ))
    .unwrap()
}

#[test]
fn a_report_is_read_with_its_cache() {
    let book = read_xlsx_from(Cursor::new(fixture())).unwrap();

    let cache = &book.pivot_caches[0];
    assert_eq!(cache.id, 2);
    assert_eq!(cache.source.sheet.as_deref(), Some("Sheet1"));
    assert_eq!(cache.source.range.unwrap().to_string(), "A1:D7");
    let names: Vec<&str> = cache.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Регион", "Товар", "Год", "Продажи"]);

    let table = &book.sheet(0).unwrap().pivot_tables[0];
    assert_eq!(table.name, "PivotTable1");
    assert_eq!(table.cache_id, cache.id, "the report points at that cache");
    assert_eq!(table.location.unwrap().to_string(), "F1:J10");
    // Region and product down the side, year across the top, sales in the
    // middle - as indexes into the cache's fields.
    assert_eq!(table.row_fields, [0, 1]);
    assert_eq!(table.column_fields, [2]);
    assert!(table.page_fields.is_empty());
    assert_eq!(table.fields[0].axis, PivotAxis::Row);
    assert_eq!(table.fields[2].axis, PivotAxis::Column);
    assert!(table.fields[3].data_field);

    let data = &table.data_fields[0];
    assert_eq!(data.field, 3);
    assert_eq!(data.subtotal, Subtotal::Sum);
    assert_eq!(data.name.as_deref(), Some("Сумма продаж"));
    assert_eq!(table.style.name.as_deref(), Some("PivotStyleLight16"));
    assert!(table.row_grand_totals && table.column_grand_totals);
}

#[test]
fn a_rewrite_keeps_the_report_whole() {
    let book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    let mut bytes = Vec::new();
    write_xlsx_to(&book, Cursor::new(&mut bytes)).unwrap();
    let back = read_xlsx_from(Cursor::new(&bytes)).unwrap();

    // Nothing changed, so nothing is written from the model: the parts travel whole, and the
    // workbook has to keep naming the cache in `<pivotCaches>` or every report
    // in it is broken.
    assert_eq!(back.pivot_caches, book.pivot_caches);
    assert_eq!(
        back.sheet(0).unwrap().pivot_tables,
        book.sheet(0).unwrap().pivot_tables
    );
}

/// A workbook saved by Excel with three reports over two caches - several
/// value fields with different functions, and the values field itself placed
/// on an axis, which the format spells `-2`.
#[test]
fn a_workbook_excel_wrote_reads_the_same_way() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pivot.xlsx");
    let book = excelerate::reader::read(path).unwrap();

    assert_eq!(book.pivot_caches.len(), 2);
    let first = &book.pivot_caches[0];
    assert_eq!(first.id, 82);
    assert_eq!(first.source.sheet.as_deref(), Some("Исходная таблица1"));
    assert_eq!(first.source.range.unwrap().to_string(), "A9:G60");
    assert_eq!(first.fields.len(), 8);
    assert_eq!(first.fields[7].name, "Цена с НДС");

    let reports: Vec<_> = book
        .sheets()
        .iter()
        .flat_map(|s| s.pivot_tables.iter().map(move |t| (s.title(), t)))
        .collect();
    assert_eq!(reports.len(), 3);

    let (sheet, table) = reports[0];
    assert_eq!(sheet, "Решение1");
    assert_eq!(table.location.unwrap().to_string(), "A8:C33");
    assert_eq!(table.row_fields, [5, 1]);
    // `-2` is not a field of the cache: it is where the value fields go.
    assert_eq!(table.column_fields, [-2]);
    assert_eq!(table.data_fields.len(), 2);
    assert_eq!(table.data_fields[0].subtotal, Subtotal::Sum);
    assert_eq!(table.data_fields[1].subtotal, Subtotal::Average);
    assert_eq!(
        table.data_fields[1].name.as_deref(),
        Some("Среднее по полю Цена")
    );

    // The third report reads the other cache and summarises with a minimum.
    let (_, third) = reports[2];
    assert_eq!(third.cache_id, 41);
    assert_eq!(third.data_fields[0].subtotal, Subtotal::Min);
    assert_eq!(third.column_fields, [8]);
}

/// The same file through a rewrite: three reports, two caches, all intact.
#[test]
fn a_rewrite_keeps_every_report_of_a_real_workbook() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pivot.xlsx");
    let book = excelerate::reader::read(path).unwrap();
    let mut bytes = Vec::new();
    write_xlsx_to(&book, Cursor::new(&mut bytes)).unwrap();
    let back = read_xlsx_from(Cursor::new(&bytes)).unwrap();

    assert_eq!(back.pivot_caches, book.pivot_caches);
    for (before, after) in book.sheets().iter().zip(back.sheets()) {
        assert_eq!(
            after.pivot_tables,
            before.pivot_tables,
            "{}",
            before.title()
        );
    }
    // The sheet's VBA name survives too: macros address sheets by it, and a
    // rename in Excel leaves it alone.
    assert_eq!(
        back.sheet(0).unwrap().properties.code_name,
        book.sheet(0).unwrap().properties.code_name
    );
    assert!(back.sheet(0).unwrap().properties.code_name.is_some());
}

/// A real BIFF8 workbook with pivot tables in it, saved by Excel in 2006.
///
/// The pivots are not read: in BIFF8 they are `SX*` records, an entirely
/// different mechanism from the XML ones above, and the xls writer rebuilds a
/// file from values rather than carrying what it did not parse - so they are
/// lost on the way out. What this pins down is that the data underneath them reads correctly.
#[test]
fn the_data_under_a_biff8_pivot_still_reads() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pivot.xls");
    let book = excelerate::reader::read(path).unwrap();

    assert_eq!(book.sheets().len(), 2);
    let sheet = book.sheet(0).unwrap();
    assert_eq!(
        sheet.get(at("A1")).unwrap().value,
        excelerate::model::CellValue::Text("Наименование".into())
    );
    let headers: Vec<String> = ["A1", "B1", "C1", "D1", "E1"]
        .iter()
        .filter_map(|a| sheet.get(at(a)).and_then(|c| c.value.plain_text()))
        .collect();
    assert_eq!(
        headers,
        ["Наименование", "Месяц", "День", "Склад", "Продано"]
    );
    // Two and a half thousand cells of data behind the reports.
    assert!(sheet.len() > 2_000, "cells: {}", sheet.len());
    assert!(
        book.sheets().iter().all(|s| s.pivot_tables.is_empty()),
        "BIFF8 pivots are not read"
    );
}

fn cycle(book: &Spreadsheet) -> (Spreadsheet, Vec<u8>) {
    let mut bytes = Vec::new();
    write_xlsx_to(book, Cursor::new(&mut bytes)).unwrap();
    (read_xlsx_from(Cursor::new(bytes.clone())).unwrap(), bytes)
}

fn part<'a>(book: &'a Spreadsheet, path: &str) -> Option<&'a str> {
    book.parts
        .iter()
        .find(|p| p.path == path)
        .map(|p| std::str::from_utf8(&p.data).unwrap())
}

/// What a report says, without where it was read from.
fn said(table: &PivotTable) -> PivotTable {
    PivotTable {
        origin: None,
        ..table.clone()
    }
}

#[test]
fn a_changed_report_is_written_from_the_model() {
    let mut book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    let table = &mut book.sheet_mut(0).unwrap().pivot_tables[0];
    table.name = "Итоги".into();
    table.column_fields.clear();
    table.page_fields = vec![2];
    table.fields[2].axis = PivotAxis::Page;
    table.data_fields[0].subtotal = Subtotal::Average;
    table.data_fields[0].name = Some("Средние продажи".into());
    table.column_grand_totals = false;
    let wanted = said(table);
    let path = table.origin.as_ref().unwrap().part().to_owned();

    let (back, _) = cycle(&book);
    let after = &back.sheet(0).unwrap().pivot_tables[0];
    assert_eq!(said(after), wanted);
    assert_eq!(after.origin.as_ref().unwrap().part(), path);
    assert_eq!(back.pivot_caches, book.pivot_caches, "the cache is kept");
    let cache = part(&back, &book.pivot_caches[0].definition_part).unwrap();
    assert!(cache.contains(r#"refreshOnLoad="1""#), "{cache}");
}

#[test]
fn a_report_made_in_code_gets_a_cache_of_its_own() {
    let mut book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    let cache = PivotCache {
        id: 9,
        source: CacheSource {
            sheet: Some("Sheet1".into()),
            range: Some(excelerate::Range::parse("A1:D7").unwrap()),
            name: None,
        },
        fields: ["Регион", "Товар", "Год", "Продажи"]
            .iter()
            .map(|name| CacheField {
                name: (*name).into(),
                ..CacheField::default()
            })
            .collect(),
        ..PivotCache::default()
    };
    let default = PivotField {
        default_subtotal: true,
        ..PivotField::default()
    };
    let table = PivotTable {
        name: "Новая".into(),
        cache_id: 9,
        location: Some(excelerate::Range::parse("L1:P12").unwrap()),
        fields: vec![
            PivotField {
                axis: PivotAxis::Row,
                ..default.clone()
            },
            default.clone(),
            PivotField {
                axis: PivotAxis::Column,
                ..default.clone()
            },
            PivotField {
                data_field: true,
                ..default
            },
        ],
        row_fields: vec![0],
        column_fields: vec![2],
        data_fields: vec![DataField {
            name: Some("Сумма".into()),
            field: 3,
            subtotal: Subtotal::Sum,
            number_format: None,
        }],
        row_grand_totals: true,
        column_grand_totals: true,
        ..PivotTable::default()
    };
    book.pivot_caches.push(cache.clone());
    book.sheet_mut(0).unwrap().pivot_tables.push(table.clone());

    let (back, _) = cycle(&book);
    let tables = &back.sheet(0).unwrap().pivot_tables;
    assert_eq!(tables.len(), 2);
    let made = tables.iter().find(|t| t.name == "Новая").unwrap();
    assert_eq!(said(made), table);
    let read = back.pivot_caches.iter().find(|c| c.id == 9).unwrap();
    assert_eq!((&read.source, &read.fields), (&cache.source, &cache.fields));
    // The report that was there is untouched.
    let old = book.sheet(0).unwrap().pivot_tables[0]
        .origin
        .as_ref()
        .unwrap()
        .part()
        .to_owned();
    assert_eq!(part(&back, &old), part(&book, &old));
    // And it is a fixed point from here.
    let (again, _) = cycle(&back);
    assert_eq!(
        again.sheet(0).unwrap().pivot_tables,
        back.sheet(0).unwrap().pivot_tables
    );
    assert_eq!(again.pivot_caches, back.pivot_caches);
}

#[test]
fn a_removed_report_takes_its_part() {
    let mut book = read_xlsx_from(Cursor::new(fixture())).unwrap();
    let path = book.sheet(0).unwrap().pivot_tables[0]
        .origin
        .as_ref()
        .unwrap()
        .part()
        .to_owned();
    book.sheet_mut(0).unwrap().pivot_tables.clear();
    let (back, _) = cycle(&book);
    assert!(back.sheet(0).unwrap().pivot_tables.is_empty());
    assert!(part(&back, &path).is_none());
    assert_eq!(back.pivot_caches, book.pivot_caches);
}
