//! Date and number grouping and calculated fields of a pivot cache: the
//! model, the reader/writer and the report they lay out.

#![allow(clippy::unwrap_used, clippy::cast_possible_truncation)]

use std::io::Cursor;

use excelerate::edit::{PivotCaptions, eval_pivot_formula, refresh_pivot};
use excelerate::model::pivot::{
    CacheField, CacheSource, DataField, DatePart, Grouping, PivotAxis, PivotCache, PivotField,
    PivotTable, RangeGroup, Subtotal,
};
use excelerate::model::{CellValue, Spreadsheet, Worksheet};
use excelerate::shared::date::{DateTime, Epoch, to_serial};
use excelerate::writer::xlsx::write_xlsx_to;
use excelerate::{CellRef, Col, Range, Row};

fn serial(year: i32, month: u32, day: u32) -> f64 {
    to_serial(DateTime::date(year, month, day), Epoch::default()).unwrap()
}

/// A book with a source of dated rows and a pivot over it, both empty of
/// report cells: `refresh_pivot` lays the report out.
fn dated_book(grouping: Grouping) -> Spreadsheet {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Data").unwrap_or_default();
    for (col, name) in ["Имя", "Дата", "Сумма"].into_iter().enumerate() {
        sheet.set(
            CellRef::new(Col::new(col as u32).unwrap(), Row::new(0).unwrap()),
            CellValue::Text(name.into()),
        );
    }
    let rows = [
        ("2024", serial(2024, 1, 15), 10.0),
        ("2024", serial(2024, 4, 2), 20.0),
        ("2024", serial(2024, 5, 7), 30.0),
        ("2025", serial(2025, 2, 11), 40.0),
        ("2025", serial(2025, 2, 20), 50.0),
    ];
    for (i, (name, date, value)) in rows.into_iter().enumerate() {
        let row = Row::new(i as u32 + 1).unwrap();
        sheet.set(
            CellRef::new(Col::new(0).unwrap(), row),
            CellValue::Text(name.into()),
        );
        sheet.set(
            CellRef::new(Col::new(1).unwrap(), row),
            CellValue::Number(date),
        );
        sheet.set(
            CellRef::new(Col::new(2).unwrap(), row),
            CellValue::Number(value),
        );
    }
    book.add_sheet(sheet).unwrap_or_default();
    let mut cache = PivotCache {
        id: 1,
        source: CacheSource {
            sheet: Some("Data".into()),
            range: Some(Range::parse("A1:C6").unwrap()),
            ..CacheSource::default()
        },
        ..PivotCache::default()
    };
    cache.fields = vec![
        CacheField {
            name: "Имя".into(),
            ..CacheField::default()
        },
        CacheField {
            name: "Дата".into(),
            grouping: Some(grouping),
            ..CacheField::default()
        },
        CacheField {
            name: "Сумма".into(),
            ..CacheField::default()
        },
    ];
    book.pivot_caches.push(cache);
    let mut sheet = Worksheet::new("Report").unwrap_or_default();
    sheet.pivot_tables.push(PivotTable {
        name: "Pivot".into(),
        cache_id: 1,
        location: Some(Range::parse("B3").unwrap()),
        fields: vec![
            PivotField::default(),
            PivotField {
                axis: PivotAxis::Row,
                default_subtotal: true,
                ..PivotField::default()
            },
            PivotField::default(),
        ],
        row_fields: vec![1],
        data_fields: vec![DataField {
            field: 2,
            ..DataField::default()
        }],
        column_grand_totals: true,
        ..PivotTable::default()
    });
    book.add_sheet(sheet).unwrap_or_default();
    book
}

fn grid(book: &Spreadsheet, sheet: usize, range: Range) -> Vec<Vec<String>> {
    (range.start.row.one_based()..=range.end.row.one_based())
        .map(|r| {
            (range.start.col.one_based()..=range.end.col.one_based())
                .map(|c| book.formatted_at(sheet, r, c))
                .collect()
        })
        .collect()
}

#[test]
fn a_date_group_lays_years_quarters_and_months_out() {
    let mut book = dated_book(Grouping {
        parts: vec![DatePart::Years, DatePart::Quarters, DatePart::Months],
        range: None,
        parent: None,
    });
    let range = refresh_pivot(&mut book, 1, 0, &PivotCaptions::default()).unwrap();
    assert_eq!(
        grid(&book, 1, range),
        [
            ["Years", "Quarters", "Months", "Sum of Сумма"],
            ["2024", "Qtr1", "Jan", "10"],
            ["", "Qtr1 Total", "", "10"],
            ["", "Qtr2", "Apr", "20"],
            ["", "", "May", "30"],
            ["", "Qtr2 Total", "", "50"],
            ["2024 Total", "", "", "60"],
            ["2025", "Qtr1", "Feb", "90"],
            ["", "Qtr1 Total", "", "90"],
            ["2025 Total", "", "", "90"],
            ["Grand Total", "", "", "150"],
        ],
        "years fold quarters, quarters fold months, each level totals in its own column"
    );
}

#[test]
fn a_number_group_folds_spans_and_the_rest() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Data").unwrap_or_default();
    sheet.set(
        CellRef::new(Col::new(0).unwrap(), Row::new(0).unwrap()),
        CellValue::Text("n".into()),
    );
    sheet.set(
        CellRef::new(Col::new(1).unwrap(), Row::new(0).unwrap()),
        CellValue::Text("v".into()),
    );
    for (i, value) in [3.0, 7.0, 12.0, 18.0, 44.0].into_iter().enumerate() {
        let row = Row::new(i as u32 + 1).unwrap();
        sheet.set(
            CellRef::new(Col::new(0).unwrap(), row),
            CellValue::Number(value),
        );
        sheet.set(
            CellRef::new(Col::new(1).unwrap(), row),
            CellValue::Number(value),
        );
    }
    book.add_sheet(sheet).unwrap_or_default();
    let mut cache = PivotCache {
        id: 1,
        source: CacheSource {
            sheet: Some("Data".into()),
            range: Some(Range::parse("A1:B6").unwrap()),
            ..CacheSource::default()
        },
        ..PivotCache::default()
    };
    cache.fields = vec![
        CacheField {
            name: "n".into(),
            grouping: Some(Grouping {
                parts: Vec::new(),
                range: Some(RangeGroup {
                    start: 0.0,
                    end: 20.0,
                    interval: 10.0,
                }),
                parent: None,
            }),
            ..CacheField::default()
        },
        CacheField {
            name: "v".into(),
            ..CacheField::default()
        },
    ];
    book.pivot_caches.push(cache);
    let mut sheet = Worksheet::new("Report").unwrap_or_default();
    sheet.pivot_tables.push(PivotTable {
        name: "Pivot".into(),
        cache_id: 1,
        location: Some(Range::parse("B2").unwrap()),
        fields: vec![PivotField::default(), PivotField::default()],
        row_fields: vec![0],
        data_fields: vec![DataField {
            field: 1,
            subtotal: Subtotal::Sum,
            ..DataField::default()
        }],
        column_grand_totals: true,
        ..PivotTable::default()
    });
    book.add_sheet(sheet).unwrap_or_default();
    let range = refresh_pivot(&mut book, 1, 0, &PivotCaptions::default()).unwrap();
    assert_eq!(
        grid(&book, 1, range),
        [
            ["n", "Sum of v"],
            ["0-10", "10"],
            ["10-20", "30"],
            ["(other)", "44"],
            ["Grand Total", "84"],
        ],
        "spans of ten, the value past the range falls into the rest group"
    );
}

/// A book whose cache carries a calculated field over two source columns.
fn calculated_book(formula: &str) -> Spreadsheet {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Data").unwrap_or_default();
    let rows: [(f64, f64); 3] = [(10.0, 2.0), (20.0, 3.0), (5.0, 4.0)];
    sheet.set(
        CellRef::new(Col::new(0).unwrap(), Row::new(0).unwrap()),
        CellValue::Text("A".into()),
    );
    sheet.set(
        CellRef::new(Col::new(1).unwrap(), Row::new(0).unwrap()),
        CellValue::Text("B".into()),
    );
    for (i, (a, b)) in rows.into_iter().enumerate() {
        let row = Row::new(i as u32 + 1).unwrap();
        sheet.set(
            CellRef::new(Col::new(0).unwrap(), row),
            CellValue::Number(a),
        );
        sheet.set(
            CellRef::new(Col::new(1).unwrap(), row),
            CellValue::Number(b),
        );
    }
    book.add_sheet(sheet).unwrap_or_default();
    let mut cache = PivotCache {
        id: 1,
        source: CacheSource {
            sheet: Some("Data".into()),
            range: Some(Range::parse("A1:B4").unwrap()),
            ..CacheSource::default()
        },
        ..PivotCache::default()
    };
    cache.fields = vec![
        CacheField {
            name: "A".into(),
            ..CacheField::default()
        },
        CacheField {
            name: "B".into(),
            ..CacheField::default()
        },
        CacheField {
            name: "C".into(),
            formula: Some(formula.to_owned()),
            ..CacheField::default()
        },
    ];
    book.pivot_caches.push(cache);
    let mut sheet = Worksheet::new("Report").unwrap_or_default();
    sheet.pivot_tables.push(PivotTable {
        name: "Pivot".into(),
        cache_id: 1,
        location: Some(Range::parse("B2").unwrap()),
        fields: vec![
            PivotField::default(),
            PivotField::default(),
            PivotField {
                data_field: true,
                ..PivotField::default()
            },
        ],
        row_fields: vec![0],
        data_fields: vec![DataField {
            field: 2,
            ..DataField::default()
        }],
        column_grand_totals: true,
        ..PivotTable::default()
    });
    book.add_sheet(sheet).unwrap_or_default();
    book
}

#[test]
fn a_calculated_field_aggregates_row_formulas() {
    let mut book = calculated_book("'A' * 'B' + 1");
    let range = refresh_pivot(&mut book, 1, 0, &PivotCaptions::default()).unwrap();
    // 10*2+1, 20*3+1, 5*4+1 sum to 92.
    assert_eq!(
        grid(&book, 1, range),
        [
            ["A", "Sum of C"],
            ["5", "21"],
            ["10", "21"],
            ["20", "61"],
            ["Grand Total", "103"],
        ],
        "each row computes A*B+1, the report sums them"
    );
    assert_eq!(
        eval_pivot_formula("'A' * 'B' + 1", &[("A".into(), 6.0), ("B".into(), 7.0)]),
        Some(43.0)
    );
    assert_eq!(
        eval_pivot_formula("SUM('A')", &[("A".into(), 6.0)]),
        None,
        "no functions in a calculated field, as Excel"
    );
}

#[test]
fn a_formula_that_does_not_parse_refuses_the_refresh() {
    let mut book = calculated_book("SUM(A B");
    let error = refresh_pivot(&mut book, 1, 0, &PivotCaptions::default()).unwrap_err();
    assert!(error.to_string().contains("does not compute"), "{error}");
}

#[test]
fn grouping_and_formulas_round_trip_xlsx() {
    // One date part: the base field carries it whole, so the file needs no
    // extra level fields and the report reads back unchanged.
    let mut book = dated_book(Grouping {
        parts: vec![DatePart::Years],
        range: None,
        parent: None,
    });
    // The calculated field rides in the same cache: the third field computes.
    book.pivot_caches[0].fields[2].formula = Some("'Имя'*0".into());
    let mut bytes = Vec::new();
    write_xlsx_to(&book, Cursor::new(&mut bytes)).unwrap();
    let mut back = excelerate::reader::xlsx::read_xlsx_from(Cursor::new(bytes)).unwrap();
    let fields = &back.pivot_caches[0].fields;
    assert_eq!(
        fields[1].grouping.as_ref().map(|g| g.parts.as_slice()),
        Some(&[DatePart::Years][..]),
        "the reader spells a fieldGroup back into Grouping"
    );
    assert_eq!(fields[2].formula.as_deref(), Some("'Имя'*0"));
    let back_range = refresh_pivot(&mut back, 1, 0, &PivotCaptions::default()).unwrap();
    let before_range = refresh_pivot(&mut book, 1, 0, &PivotCaptions::default()).unwrap();
    assert_eq!(
        grid(&back, 1, back_range),
        grid(&book, 1, before_range),
        "the report reads the same from the file as from the model"
    );
}
