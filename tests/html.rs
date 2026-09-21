//! A page we write reads back with its values, formulas and formats.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::CellRef;
use excelerate::model::{CellValue, Spreadsheet};
use excelerate::reader::html::read_html_str;
use excelerate::style::{NumberFormat, Style};
use excelerate::writer::html::{HtmlOptions, write_html_to};

fn at(address: &str) -> CellRef {
    CellRef::parse(address).unwrap()
}

#[test]
fn a_page_we_wrote_reads_back_the_same_values() {
    let mut book = Spreadsheet::new();
    let money = book.styles.intern(Style {
        number_format: NumberFormat::Custom("#,##0.00".to_owned()),
        ..Default::default()
    });
    let sheet = book.sheet_mut(0).unwrap();
    sheet.set(at("A1"), CellValue::Number(1234.5));
    sheet.entry(at("A1")).style = money;
    sheet.set(at("B1"), CellValue::Bool(true));
    sheet.set(at("C1"), CellValue::text("007"));
    sheet.set(at("D1"), CellValue::text("two\nlines"));
    sheet.set(
        at("E1"),
        CellValue::Formula {
            formula: "A1*2".to_owned(),
            cached: Some(Box::new(CellValue::Number(2469.0))),
        },
    );

    let mut page = Vec::new();
    write_html_to(&book, &mut page, &HtmlOptions::default()).unwrap();
    let back = read_html_str(&String::from_utf8(page).unwrap());
    let sheet = back.sheet(0).unwrap();
    let value = |a: &str| sheet.get(at(a)).unwrap().value.clone();

    assert_eq!(value("A1"), CellValue::Number(1234.5));
    assert_eq!(value("B1"), CellValue::Bool(true));
    assert_eq!(value("C1"), CellValue::text("007"));
    assert_eq!(value("D1"), CellValue::text("two\nlines"));
    assert_eq!(
        value("E1"),
        book.sheet(0).unwrap().get(at("E1")).unwrap().value
    );
    let style = back.styles.get(sheet.get(at("A1")).unwrap().style).unwrap();
    assert_eq!(style.number_format.code(), "#,##0.00");
}
