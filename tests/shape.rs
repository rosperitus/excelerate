//! Shapes: read from a drawing `excelize` wrote (`tests/fixtures/shapes.xlsx`)
//! and from `tests/chart1.xlsx`, edited, and written back.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::model::Spreadsheet;
use excelerate::model::chart::{Anchor, Marker};
use excelerate::model::shape::Shape;
use excelerate::reader::xlsx::read_xlsx_from;
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::Cursor;

fn open(name: &str) -> Spreadsheet {
    let path = format!("{}/tests/{name}", env!("CARGO_MANIFEST_DIR"));
    read_xlsx_from(Cursor::new(std::fs::read(path).unwrap())).unwrap()
}

fn rewrite(book: &Spreadsheet) -> Spreadsheet {
    let mut bytes = Vec::new();
    write_xlsx_to(book, Cursor::new(&mut bytes)).unwrap();
    read_xlsx_from(Cursor::new(bytes)).unwrap()
}

fn at(col: u32, row: u32) -> Marker {
    Marker {
        col: excelerate::Col::new(col).unwrap(),
        row: excelerate::Row::new(row).unwrap(),
        ..Marker::default()
    }
}

/// What a shape says, without where it was read from.
fn said(shape: &Shape) -> (String, Option<String>, String, Anchor) {
    (
        shape.name.clone(),
        shape.geometry.clone(),
        shape.text.clone(),
        shape.anchor,
    )
}

#[test]
fn shapes_another_program_drew_are_read() {
    let book = open("fixtures/shapes.xlsx");
    let shapes = &book.sheet(0).unwrap().shapes;
    let got: Vec<_> = shapes
        .iter()
        .map(|s| (s.name.as_str(), s.geometry.as_deref(), s.text.trim()))
        .collect();
    assert_eq!(
        got,
        [
            ("Shape 2", Some("rect"), "Итого за квартал"),
            ("Shape 3", Some("ellipse"), ""),
            ("Shape 4", Some("rightArrow"), "next & more"),
        ]
    );
    assert!(matches!(shapes[0].anchor, Anchor::TwoCell { from, .. } if from == at(1, 1)));
    assert!(shapes.iter().all(Shape::is_unchanged));
}

#[test]
fn every_kind_of_edit_is_written() {
    let mut book = open("fixtures/shapes.xlsx");
    let sheet = book.sheet_mut(0).unwrap();
    sheet.shapes[0].name = "Total".into();
    sheet.shapes[0].text = "Итого\nза год".into();
    sheet.shapes[1].geometry = Some("triangle".into());
    sheet.shapes[1].text = "new text".into();
    sheet.shapes[1].anchor = Anchor::TwoCell {
        from: at(6, 20),
        to: at(8, 24),
        edit_as: None,
    };
    sheet.shapes.remove(2);
    let mut added = Shape::new(
        "wedgeRectCallout",
        Anchor::TwoCell {
            from: at(10, 2),
            to: at(13, 6),
            edit_as: None,
        },
    );
    added.text = "added".into();
    sheet.shapes.push(added);
    let expected: Vec<_> = sheet.shapes.iter().map(said).collect();

    let back = rewrite(&book);
    let shapes = &back.sheet(0).unwrap().shapes;
    let mut got: Vec<_> = shapes.iter().map(said).collect();
    // The new shape takes the next id and gets a name from it.
    assert_eq!(got[2].0, "Shape 5");
    got[2].0 = String::new();
    assert_eq!(got, expected);

    // The first run's formatting went with the new text.
    let drawing = String::from_utf8(
        back.parts
            .iter()
            .find(|p| p.path == "xl/drawings/drawing1.xml")
            .unwrap()
            .data
            .clone(),
    )
    .unwrap();
    assert_eq!(drawing.matches("<a:t>за год</a:t>").count(), 1);
    assert!(!drawing.contains("next &amp; more"));

    // Written again untouched, the drawing is a fixed point.
    let again = rewrite(&back);
    assert_eq!(
        again
            .sheet(0)
            .unwrap()
            .shapes
            .iter()
            .map(said)
            .collect::<Vec<_>>(),
        shapes.iter().map(said).collect::<Vec<_>>()
    );
}

#[test]
fn a_shape_on_a_sheet_without_a_drawing_gets_one() {
    let mut book = Spreadsheet::empty();
    let mut sheet = excelerate::model::Worksheet::new("S").unwrap();
    let mut shape = Shape::new(
        "ellipse",
        Anchor::TwoCell {
            from: at(1, 1),
            to: at(3, 5),
            edit_as: None,
        },
    );
    shape.name = "Circle".into();
    shape.text = "hello".into();
    sheet.shapes.push(shape);
    book.add_sheet(sheet).unwrap();
    let back = rewrite(&book);
    let shapes = &back.sheet(0).unwrap().shapes;
    assert_eq!(shapes.len(), 1);
    assert_eq!(said(&shapes[0]), said(&book.sheet(0).unwrap().shapes[0]));
}

#[test]
fn shapes_move_with_inserted_rows_and_stay_untouched() {
    let mut book = open("fixtures/shapes.xlsx");
    excelerate::edit::insert_rows(&mut book, 0, excelerate::Row::new(0).unwrap(), 2).unwrap();
    let shape = &book.sheet(0).unwrap().shapes[0];
    assert!(matches!(shape.anchor, Anchor::TwoCell { from, .. } if from == at(1, 3)));
    assert!(shape.is_unchanged(), "the bytes moved with the model");
    let back = rewrite(&book);
    assert_eq!(said(&back.sheet(0).unwrap().shapes[0]), said(shape));
}

#[test]
fn every_shape_of_a_large_workbook_survives() {
    let book = open("chart1.xlsx");
    let count: usize = book.sheets().iter().map(|s| s.shapes.len()).sum();
    // Counted independently by walking the drawings, `mc:Fallback` copies
    // left out.
    assert_eq!(count, 2030);
    let mut book = book;
    // Removing one shape rewrites one drawing and leaves the rest.
    let sheet = book
        .sheets()
        .iter()
        .position(|s| {
            s.shapes
                .iter()
                .any(|x| !x.origin.as_ref().unwrap().grouped())
        })
        .unwrap();
    let victim = book
        .sheet(sheet)
        .unwrap()
        .shapes
        .iter()
        .position(|x| !x.origin.as_ref().unwrap().grouped())
        .unwrap();
    book.sheet_mut(sheet).unwrap().shapes.remove(victim);
    let back = rewrite(&book);
    let after: usize = back.sheets().iter().map(|s| s.shapes.len()).sum();
    assert_eq!(after, 2029);
    assert_eq!(
        back.sheets().iter().map(|s| s.charts.len()).sum::<usize>(),
        130
    );
    assert_eq!(
        back.sheets().iter().map(|s| s.images.len()).sum::<usize>(),
        156
    );
}
