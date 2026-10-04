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
    assert_eq!(table.page_fields, []);
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
        // A report of Excel's smallest shape: a row of headers, a column of
        // labels.
        first_data_row: 1,
        first_data_col: 1,
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
    assert_eq!(back.sheet(0).unwrap().pivot_tables, []);
    assert!(part(&back, &path).is_none());
    assert_eq!(back.pivot_caches, book.pivot_caches);
}

/// `GETPIVOTDATA` reads the report out of the cells it was laid out in, which
/// is where Excel reads it from too. The workbook is `tests/pivot.xlsx`,
/// saved by Excel: three reports, one with two value fields, one with two row
/// fields and a column field.
#[test]
fn a_value_is_read_out_of_a_laid_out_report() {
    use excelerate::formula::eval::{Engine, Origin};
    use excelerate::formula::value::Value;

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pivot.xlsx");
    let book = excelerate::reader::read(path).unwrap();
    let mut engine = Engine::new(&book);
    let ask = |engine: &mut Engine<'_>, sheet: usize, formula: &str| {
        engine.eval(Origin::new(sheet, at("ZZ999")), formula)
    };
    let sheet = |title: &str| {
        book.sheets()
            .iter()
            .position(|s| s.title() == title)
            .unwrap()
    };

    // Two value fields over one row field: each is asked for by the name of
    // the field it sums, not by the caption over its column.
    let one = sheet("Решение1");
    assert_eq!(
        ask(
            &mut engine,
            one,
            r#"GETPIVOTDATA("Кол-во",'Решение1'!A8,"Товар","Монитор")"#
        ),
        Value::Number(1774.0)
    );
    assert_eq!(
        ask(
            &mut engine,
            one,
            r#"GETPIVOTDATA("Цена",'Решение1'!A8,"Товар","Монитор")"#
        ),
        Value::Number(8136.4)
    );
    // Nothing else asked: the grand total.
    assert_eq!(
        ask(&mut engine, one, r#"GETPIVOTDATA("Кол-во",'Решение1'!A8)"#),
        Value::Number(26422.0)
    );
    // A field this report does not show as a value.
    assert!(matches!(
        ask(&mut engine, one, r#"GETPIVOTDATA("Оклад",'Решение1'!A8)"#),
        Value::Error(_)
    ));
    // A value field whose name is the start of another's caption is not it.
    assert_eq!(
        ask(
            &mut engine,
            sheet("Решение2"),
            r#"GETPIVOTDATA("Цена с НДС",'Решение2'!A8,"Товар","Монитор")"#
        ),
        Value::Number(7068.2)
    );

    // Two row fields and a column field, with subtotals.
    let third = sheet("Исходная таблица2+решение");
    let report = "'Исходная таблица2+решение'!A66";
    assert_eq!(
        ask(
            &mut engine,
            third,
            &format!(
                r#"GETPIVOTDATA("Оклад",{report},"Отдел","Бухгалтерия","Должность","Бухгалтер")"#
            )
        ),
        Value::Number(78950.0)
    );
    // Only the outer field: the subtotal of that department.
    assert_eq!(
        ask(
            &mut engine,
            third,
            &format!(r#"GETPIVOTDATA("Оклад",{report},"Отдел","Администрация")"#)
        ),
        Value::Number(35600.0)
    );
    // An item the report does not show.
    assert!(matches!(
        ask(
            &mut engine,
            third,
            &format!(r#"GETPIVOTDATA("Оклад",{report},"Отдел","Склад")"#)
        ),
        Value::Error(_)
    ));
    // A reference that is not in a report at all.
    assert!(matches!(
        ask(&mut engine, third, r#"GETPIVOTDATA("Оклад",'Решение1'!Z1)"#),
        Value::Error(_)
    ));
}

// Laying a report out: `edit::refresh_pivot`.

use excelerate::Range;
use excelerate::edit::{PivotCaptions, refresh_pivot};
use excelerate::model::CellValue;
use excelerate::model::Worksheet;

/// A book with the rows on sheet "Data" from A1, and an empty sheet
/// "Report" holding one report over them at A1.
fn source(rows: &[&[CellValue]], table: PivotTable) -> Spreadsheet {
    let mut book = Spreadsheet::empty();
    let mut data = Worksheet::new("Data").unwrap();
    for (r, row) in rows.iter().enumerate() {
        for (c, value) in row.iter().enumerate() {
            if *value != CellValue::Empty {
                let (r, c) = (u32::try_from(r).unwrap(), u32::try_from(c).unwrap());
                data.set_at(r + 1, c + 1, value.clone()).unwrap();
            }
        }
    }
    let width = rows[0].len();
    let end = CellRef::from_row_col(
        u32::try_from(rows.len()).unwrap(),
        u32::try_from(width).unwrap(),
    )
    .unwrap();
    book.add_sheet(data).unwrap();
    let mut report = Worksheet::new("Report").unwrap();
    report.pivot_tables.push(PivotTable {
        name: "Pivot".into(),
        cache_id: 1,
        location: Some(Range::parse("A1").unwrap()),
        ..table
    });
    book.add_sheet(report).unwrap();
    book.pivot_caches.push(PivotCache {
        id: 1,
        source: CacheSource {
            sheet: Some("Data".into()),
            range: Some(Range::new(at("A1"), end)),
            name: None,
        },
        ..PivotCache::default()
    });
    book
}

/// A definition with both grand totals, as Excel makes a new one.
fn totals() -> PivotTable {
    PivotTable {
        row_grand_totals: true,
        column_grand_totals: true,
        ..PivotTable::default()
    }
}

fn t(s: &str) -> CellValue {
    CellValue::text(s)
}

fn n(x: f64) -> CellValue {
    CellValue::Number(x)
}

/// The report's cells as text, a row per line and `|` between cells.
fn grid(book: &Spreadsheet, sheet: usize, area: Range) -> Vec<String> {
    let ws = book.sheet(sheet).unwrap();
    (area.start.row.index()..=area.end.row.index())
        .map(|r| {
            (area.start.col.index()..=area.end.col.index())
                .map(|c| {
                    let at = CellRef::new(
                        excelerate::Col::new(c).unwrap(),
                        excelerate::Row::new(r).unwrap(),
                    );
                    match ws.get(at).map(|c| &c.value) {
                        None | Some(CellValue::Empty) => String::new(),
                        Some(CellValue::Number(x)) => format!("{x}"),
                        Some(CellValue::Error(e)) => e.as_str().to_owned(),
                        Some(CellValue::Bool(b)) => b.to_string().to_uppercase(),
                        Some(other) => other.plain_text().unwrap_or_default(),
                    }
                })
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}

fn sales() -> Vec<Vec<CellValue>> {
    vec![
        vec![t("Region"), t("Product"), t("Year"), t("Sales")],
        vec![t("North"), t("Tea"), n(2024.0), n(10.0)],
        vec![t("south"), t("Tea"), n(2024.0), n(5.0)],
        vec![t("North"), t("Coffee"), n(2025.0), n(7.0)],
        vec![t("South"), t("Coffee"), n(2025.0), n(3.0)],
        vec![t("North"), t("Tea"), n(2025.0), n(1.0)],
    ]
}

fn refresh(rows: &[Vec<CellValue>], table: PivotTable) -> (Spreadsheet, Range) {
    let rows: Vec<&[CellValue]> = rows.iter().map(Vec::as_slice).collect();
    let mut book = source(&rows, table);
    let area = refresh_pivot(&mut book, 1, 0, &PivotCaptions::default()).unwrap();
    (book, area)
}

fn sum_of(field: u32) -> DataField {
    DataField {
        field,
        ..DataField::default()
    }
}

#[test]
fn one_row_field_and_a_sum() {
    let (book, area) = refresh(
        &sales(),
        PivotTable {
            row_fields: vec![0],
            data_fields: vec![sum_of(3)],
            ..totals()
        },
    );
    assert_eq!(area.to_string(), "A1:B4");
    // Text items are one item whatever the case, spelled as first seen.
    assert_eq!(
        grid(&book, 1, area),
        [
            "Region|Sum of Sales",
            "North|18",
            "south|8",
            "Grand Total|26"
        ]
    );
    let table = &book.sheet(1).unwrap().pivot_tables[0];
    assert_eq!((table.first_data_row, table.first_data_col), (1, 1));
    assert_eq!(table.data_fields[0].name.as_deref(), Some("Sum of Sales"));
    assert_eq!(table.fields[0].axis, PivotAxis::Row);
    assert!(table.fields[3].data_field);
    let cache = &book.pivot_caches[0];
    let names: Vec<&str> = cache.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Region", "Product", "Year", "Sales"]);
    assert_eq!(cache.fields[0].shared_items, ["North", "south"]);
    assert_eq!(cache.fields[3].shared_items, Vec::<String>::new());
}

#[test]
fn rows_columns_and_two_value_fields() {
    let (book, area) = refresh(
        &sales(),
        PivotTable {
            row_fields: vec![1],
            column_fields: vec![2],
            data_fields: vec![
                sum_of(3),
                DataField {
                    field: 3,
                    subtotal: Subtotal::Count,
                    ..DataField::default()
                },
            ],
            ..totals()
        },
    );
    // The values field goes on the columns, under the year.
    let table = &book.sheet(1).unwrap().pivot_tables[0];
    assert_eq!(table.column_fields, [2, -2]);
    assert_eq!((table.first_data_row, table.first_data_col), (3, 1));
    assert_eq!(
        grid(&book, 1, area),
        [
            "|Year|Values||||",
            "|2024||2025||Total Sum of Sales|Total Count of Sales",
            "Product|Sum of Sales|Count of Sales|Sum of Sales|Count of Sales||",
            "Coffee|||10|2|10|2",
            "Tea|15|2|1|1|16|3",
            "Grand Total|15|2|11|3|26|5",
        ]
    );
}

#[test]
fn outer_row_fields_get_subtotals() {
    let mut table = PivotTable {
        row_fields: vec![0, 1],
        data_fields: vec![sum_of(3)],
        ..totals()
    };
    table.fields = vec![
        PivotField {
            default_subtotal: true,
            ..PivotField::default()
        };
        4
    ];
    let (book, area) = refresh(&sales(), table);
    assert_eq!(
        grid(&book, 1, area),
        [
            "Region|Product|Sum of Sales",
            "North|Coffee|7",
            "|Tea|11",
            "North Total||18",
            "south|Coffee|3",
            "|Tea|5",
            "south Total||8",
            "Grand Total||26",
        ]
    );
    // Without the subtotal, and without the grand total row.
    let mut table = book.sheet(1).unwrap().pivot_tables[0].clone();
    table.fields[0].default_subtotal = false;
    table.column_grand_totals = false;
    let (book, area) = refresh(&sales(), table);
    assert_eq!(
        grid(&book, 1, area),
        [
            "Region|Product|Sum of Sales",
            "North|Coffee|7",
            "|Tea|11",
            "south|Coffee|3",
            "|Tea|5",
        ]
    );
}

#[test]
fn blanks_sort_last_and_errors_carry() {
    let rows = vec![
        vec![t("Key"), t("Value")],
        vec![t("b"), n(1.0)],
        vec![CellValue::Empty, n(2.0)],
        vec![n(10.0), CellValue::Error(excelerate::CellError::Na)],
        vec![t("A"), t("text")],
        vec![n(9.0), n(4.0)],
        vec![CellValue::Bool(true), n(1.0)],
    ];
    let (book, area) = refresh(
        &rows,
        PivotTable {
            row_fields: vec![0],
            data_fields: vec![
                sum_of(1),
                DataField {
                    field: 1,
                    subtotal: Subtotal::Average,
                    ..DataField::default()
                },
            ],
            ..totals()
        },
    );
    assert_eq!(
        grid(&book, 1, area),
        [
            "|Values|",
            "Key|Sum of Value|Average of Value",
            "9|4|4",
            "10|#N/A|#N/A",
            "A|0|#DIV/0!",
            "b|1|1",
            "TRUE|1|1",
            "(blank)|2|2",
            "Grand Total|#N/A|#N/A",
        ]
    );
}

#[test]
fn a_page_filter_keeps_its_item() {
    let mut table = PivotTable {
        row_fields: vec![1],
        page_fields: vec![2],
        data_fields: vec![sum_of(3)],
        ..totals()
    };
    table.fields = vec![PivotField::default(); 4];
    table.fields[2].page_item = Some("2025".into());
    let (book, area) = refresh(&sales(), table.clone());
    assert_eq!(
        grid(&book, 1, area),
        [
            "Product|Sum of Sales",
            "Coffee|10",
            "Tea|1",
            "Grand Total|11"
        ]
    );
    // An item the source does not have shows everything.
    table.fields[2].page_item = Some("1999".into());
    let (book, area) = refresh(&sales(), table);
    assert_eq!(
        grid(&book, 1, area),
        [
            "Product|Sum of Sales",
            "Coffee|10",
            "Tea|16",
            "Grand Total|26"
        ]
    );
    assert_eq!(
        book.sheet(1).unwrap().pivot_tables[0].fields[2].page_item,
        None
    );
}

#[test]
fn a_smaller_report_clears_the_old_one() {
    let (mut book, area) = refresh(
        &sales(),
        PivotTable {
            row_fields: vec![0, 1],
            column_fields: vec![2],
            data_fields: vec![sum_of(3)],
            ..totals()
        },
    );
    // Two header rows, two groups of two and their subtotals, a grand total.
    assert_eq!(area.to_string(), "A1:E9");
    let table = &mut book.sheet_mut(1).unwrap().pivot_tables[0];
    table.row_fields = vec![0];
    table.column_fields.clear();
    let area = refresh_pivot(&mut book, 1, 0, &PivotCaptions::default()).unwrap();
    assert_eq!(area.to_string(), "A1:B4");
    assert_eq!(
        book.sheet(1).unwrap().len(),
        8,
        "only the new report is left"
    );
}

#[test]
fn a_report_that_would_cover_its_source_is_refused() {
    let rows: Vec<Vec<CellValue>> = sales();
    let rows: Vec<&[CellValue]> = rows.iter().map(Vec::as_slice).collect();
    let mut book = source(
        &rows,
        PivotTable {
            row_fields: vec![0],
            data_fields: vec![sum_of(3)],
            ..totals()
        },
    );
    let table = book.sheet_mut(1).unwrap().pivot_tables.remove(0);
    book.sheet_mut(0).unwrap().pivot_tables.push(PivotTable {
        location: Some(Range::parse("B3").unwrap()),
        ..table
    });
    let whole = Range::parse("A1:F9").unwrap();
    let before = grid(&book, 0, whole);
    assert!(refresh_pivot(&mut book, 0, 0, &PivotCaptions::default()).is_err());
    assert_eq!(grid(&book, 0, whole), before, "nothing changed");
}

/// A report laid out in code goes through a file and comes back the same,
/// cells and definition both; and `GETPIVOTDATA` reads it.
#[test]
fn a_refreshed_report_survives_a_cycle() {
    use excelerate::formula::eval::{Engine, Origin};
    use excelerate::formula::value::Value;

    let mut table = PivotTable {
        row_fields: vec![0, 1],
        column_fields: vec![2],
        page_fields: vec![],
        data_fields: vec![sum_of(3)],
        style: excelerate::model::pivot::PivotStyleInfo {
            name: Some("PivotStyleLight16".into()),
            ..Default::default()
        },
        ..totals()
    };
    table.fields = vec![
        PivotField {
            default_subtotal: true,
            ..PivotField::default()
        };
        4
    ];
    let rows = sales();
    let rows: Vec<&[CellValue]> = rows.iter().map(Vec::as_slice).collect();
    let mut book = source(&rows, table);
    // The year moves to a page filter, showing one year.
    {
        let t = &mut book.sheet_mut(1).unwrap().pivot_tables[0];
        t.column_fields.clear();
        t.page_fields = vec![2];
        t.fields[2].page_item = Some("2025".into());
    }
    let area = refresh_pivot(&mut book, 1, 0, &PivotCaptions::default()).unwrap();
    let laid = grid(&book, 1, area);

    let (back, bytes) = cycle(&book);
    let table = &back.sheet(1).unwrap().pivot_tables[0];
    assert_eq!(said(table), book.sheet(1).unwrap().pivot_tables[0]);
    let cache = &back.pivot_caches[0];
    assert_eq!(
        (&cache.source, &cache.fields),
        (&book.pivot_caches[0].source, &book.pivot_caches[0].fields)
    );
    assert_eq!(grid(&back, 1, area), laid);
    let xml = part(&back, &cache.definition_part).unwrap();
    assert!(xml.contains(r#"refreshOnLoad="1""#), "{xml}");
    let _ = bytes;

    let mut engine = Engine::new(&back);
    let ask = |engine: &mut Engine<'_>, f: &str| engine.eval(Origin::new(1, at("ZZ999")), f);
    assert_eq!(
        ask(
            &mut engine,
            r#"GETPIVOTDATA("Sales",Report!A1,"Region","North","Product","Coffee")"#
        ),
        Value::Number(7.0)
    );
    assert_eq!(
        ask(&mut engine, r#"GETPIVOTDATA("Sales",Report!A1)"#),
        Value::Number(11.0)
    );
}

/// The simplest of the reports Excel laid out in a real workbook: a sum by
/// one row field, its source a defined name. Excel used its compact form,
/// which differs from the tabular one only in the heading over the labels.
/// The workbook is in the corpus, which git does not carry.
#[test]
fn a_report_lays_out_as_excel_laid_it_out() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/corpus/Finance_Ledger_100_Unique_Functions.xlsx"
    );
    if !std::path::Path::new(path).exists() {
        return;
    }
    let mut book = excelerate::reader::read(path).unwrap();
    let sheet = book
        .sheets()
        .iter()
        .position(|s| s.title() == "Pivot_Tables")
        .unwrap();
    let index = book
        .sheet(sheet)
        .unwrap()
        .pivot_tables
        .iter()
        .position(|t| t.name == "PivotTable2")
        .unwrap();
    let area = book.sheet(sheet).unwrap().pivot_tables[index]
        .location
        .unwrap();
    let excel = grid(&book, sheet, area);
    let new = refresh_pivot(&mut book, sheet, index, &PivotCaptions::default()).unwrap();
    assert_eq!(new, area);
    let ours = grid(&book, sheet, new);
    assert_eq!(excel[0], "Row Labels|Sum of NetGBP");
    assert_eq!(ours[0], "Department|Sum of NetGBP");
    assert_eq!(ours[1..], excel[1..]);
}

/// A report Excel laid out in tabular form, with subtotals and Russian
/// captions. Its cache reads another workbook; the same data stands on the
/// report's sheet, so the cache is pointed there. Excel's report hides all
/// but two departments and one education; here the education is a page
/// filter instead, and the two departments, first in order, are compared.
#[test]
fn a_tabular_report_lays_out_as_excel_laid_it_out() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pivot.xlsx");
    let mut book = excelerate::reader::read(path).unwrap();
    let title = "Исходная таблица2+решение";
    let sheet = book.sheet_index_by_name(title).unwrap();
    let excel = grid(&book, sheet, Range::parse("A68:C75").unwrap());
    let cache = book.pivot_caches.iter_mut().find(|c| c.id == 41).unwrap();
    cache.source = CacheSource {
        sheet: Some(title.into()),
        range: Some(Range::parse("E12:N62").unwrap()),
        name: None,
    };
    let table = &mut book.sheet_mut(sheet).unwrap().pivot_tables[0];
    assert_eq!(table.column_fields, [8]);
    table.column_fields.clear();
    table.page_fields = vec![8];
    table.fields[8].page_item = Some("высшее".into());
    let captions = PivotCaptions {
        grand_total: "Общий итог".into(),
        total: "{} Итог".into(),
        ..PivotCaptions::default()
    };
    let area = refresh_pivot(&mut book, sheet, 0, &captions).unwrap();
    let ours = grid(&book, sheet, area);
    assert_eq!(ours[0], "Отдел|Должность|Минимум по полю Оклад");
    assert_eq!(ours[1..=excel.len()], excel[..]);
}

/// Excel 2010 and later end a report with `<x14:pivotTableDefinition>` in
/// `<extLst>`: the root's local name again, without its attributes. It must
/// not overwrite the root's name, cache and totals.
#[test]
fn an_extension_does_not_overwrite_the_report() {
    use std::io::{Read, Write};

    let mut source = zip::ZipArchive::new(Cursor::new(fixture())).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for i in 0..source.len() {
        let mut part = source.by_index(i).unwrap();
        let name = part.name().to_owned();
        let mut data = String::new();
        part.read_to_string(&mut data).unwrap();
        if name.starts_with("xl/pivotTables/pivotTable") {
            data = data.replace(
                "</pivotTableDefinition>",
                concat!(
                    r#"<extLst><ext uri="{962EF5D1-5CA2-4c93-8EF4-DBF5C05439D2}" "#,
                    r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
                    r#"<x14:pivotTableDefinition hideValuesRow="1"/></ext></extLst>"#,
                    "</pivotTableDefinition>"
                ),
            );
        }
        out.start_file(name, options).unwrap();
        out.write_all(data.as_bytes()).unwrap();
    }
    let bytes = out.finish().unwrap().into_inner();
    let book = read_xlsx_from(Cursor::new(bytes)).unwrap();
    let table = &book.sheet(0).unwrap().pivot_tables[0];
    assert_eq!(table.name, "PivotTable1");
    assert_eq!(table.cache_id, 2);
    assert!(table.row_grand_totals && table.column_grand_totals);
}
