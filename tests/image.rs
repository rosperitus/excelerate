//! Pictures: read from the drawing, written back untouched as bytes, and
//! moved, renamed, replaced, removed and added through the model.
//!
//! `chart1.xlsx` is a dashboard saved by Excel with 156 pictures on 90 media
//! parts: icons stored as SVG with a PNG fallback, some of them inside groups
//! of shapes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::CellRef;
use excelerate::model::Spreadsheet;
use excelerate::model::chart::{Anchor, Marker};
use excelerate::model::image::{Image, ImageFormat};
use excelerate::reader::xlsx::read_xlsx_from;
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::Cursor;

/// A valid 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn open(name: &str) -> Spreadsheet {
    let path = format!("{}/tests/{name}", env!("CARGO_MANIFEST_DIR"));
    read_xlsx_from(Cursor::new(std::fs::read(path).unwrap())).unwrap()
}

fn rewrite(book: &Spreadsheet) -> Spreadsheet {
    let mut bytes = Vec::new();
    write_xlsx_to(book, Cursor::new(&mut bytes)).unwrap();
    read_xlsx_from(Cursor::new(bytes)).unwrap()
}

fn all_images(book: &Spreadsheet) -> Vec<(usize, Image)> {
    book.sheets()
        .iter()
        .enumerate()
        .flat_map(|(i, s)| s.images.iter().map(move |image| (i, image.clone())))
        .collect()
}

/// What a picture says, without where it was read from.
fn said(image: &Image) -> (String, String, Anchor, ImageFormat, Vec<u8>) {
    (
        image.name.clone(),
        image.description.clone(),
        image.anchor,
        image.format,
        image.data.clone(),
    )
}

fn part<'a>(book: &'a Spreadsheet, path: &str) -> &'a [u8] {
    &book.parts.iter().find(|p| p.path == path).unwrap().data
}

fn at(col: u32, row: u32) -> Marker {
    Marker {
        col: excelerate::Col::new(col).unwrap(),
        row: excelerate::Row::new(row).unwrap(),
        ..Marker::default()
    }
}

#[test]
fn every_picture_of_an_excel_dashboard_is_read() {
    let book = open("chart1.xlsx");
    let images = all_images(&book);
    assert_eq!(images.len(), 156);
    assert!(images.iter().all(|(_, i)| i.is_unchanged()));
    let first = &book.sheets()[0].images[0];
    assert_eq!(first.name, "Graphic 45");
    assert_eq!(first.description, "Signal with solid fill");
    assert_eq!(first.format, ImageFormat::Png);
    assert!(first.data.starts_with(b"\x89PNG"));
}

/// `fixtures/pictures.xlsx` is written by excelize, whose drawing markup
/// differs from Excel's; the pictures and their alt text are what it was told
/// to put there.
#[test]
fn pictures_written_by_another_library_are_read() {
    let book = open("fixtures/pictures.xlsx");
    let images = &book.sheets()[0].images;
    assert_eq!(images.len(), 2);
    assert_eq!(images[0].description, "made by excelize");
    assert_eq!(images[1].description, "second");
    assert!(images.iter().all(|i| i.data == PNG));
    let Anchor::TwoCell { from, .. } = images[1].anchor else {
        panic!("{:?}", images[1].anchor)
    };
    assert_eq!((from.col.index(), from.row.index()), (4, 6), "E7");
}

#[test]
fn untouched_pictures_go_back_as_the_bytes_they_came_in() {
    let book = open("chart1.xlsx");
    let back = rewrite(&book);
    let (before, after) = (all_images(&book), all_images(&back));
    assert_eq!(before.len(), after.len());
    for ((_, a), (_, b)) in before.iter().zip(&after) {
        assert_eq!(said(a), said(b));
    }
    for drawing in book
        .parts
        .iter()
        .filter(|p| p.path.starts_with("xl/drawings/"))
    {
        assert_eq!(
            part(&back, &drawing.path),
            drawing.data.as_slice(),
            "{}",
            drawing.path
        );
    }
}

#[test]
fn a_moved_renamed_picture_keeps_what_the_model_does_not_name() {
    let mut book = open("chart1.xlsx");
    let sheet = book.sheet_mut(0).unwrap();
    let image = &mut sheet.images[0];
    image.anchor = Anchor::OneCell {
        from: at(10, 20),
        width: 457_200,
        height: 457_200,
    };
    image.name = "Signal".to_owned();
    image.description = "Signal <strength> & more".to_owned();
    let drawing = image.origin.as_ref().unwrap().drawing().to_owned();

    let back = rewrite(&book);
    let moved = &back.sheets()[0].images[0];
    assert_eq!(moved.anchor, book.sheets()[0].images[0].anchor);
    assert_eq!(moved.name, "Signal");
    assert_eq!(moved.description, "Signal <strength> & more");
    assert_eq!(moved.data, book.sheets()[0].images[0].data);
    // The SVG beside the PNG fallback lives in the element and stays.
    let xml = String::from_utf8(part(&back, &drawing).to_vec()).unwrap();
    let element = &xml[xml.find("name=\"Signal\"").unwrap()..];
    let element = &element[..element.find("</xdr:pic>").unwrap()];
    assert!(element.contains("svgBlip"), "{element}");
    // The other pictures did not move.
    for (a, b) in book.sheets()[0].images[1..]
        .iter()
        .zip(&back.sheets()[0].images[1..])
    {
        assert_eq!(said(a), said(b));
    }
}

#[test]
fn new_bytes_go_into_a_media_part_of_their_own() {
    let mut book = open("chart1.xlsx");
    let shared_before = book.sheets()[0].images[1].data.clone();
    book.sheet_mut(0).unwrap().images[0].data = PNG.to_vec();

    let back = rewrite(&book);
    let replaced = &back.sheets()[0].images[0];
    assert_eq!(replaced.data, PNG);
    assert_eq!(back.sheets()[0].images[1].data, shared_before);
    // The replaced blip carries no SVG that would be drawn over the new bytes.
    let drawing = replaced.origin.as_ref().unwrap().drawing();
    let xml = String::from_utf8(part(&back, drawing).to_vec()).unwrap();
    let element = &xml[xml.find("name=\"Graphic 45\"").unwrap()..];
    let element = &element[..element.find("</xdr:pic>").unwrap()];
    assert!(!element.contains("svgBlip"), "{element}");
}

#[test]
fn a_removed_picture_is_gone_and_the_rest_stay() {
    let mut book = open("chart1.xlsx");
    let before = all_images(&book).len();
    let is_grouped = |image: &Image| {
        image
            .origin
            .as_ref()
            .is_some_and(excelerate::model::image::ImageOrigin::grouped)
    };
    let (index, grouped, alone) = book
        .sheets()
        .iter()
        .enumerate()
        .find_map(|(index, sheet)| {
            let images = &sheet.images;
            let grouped = images.iter().position(is_grouped)?;
            let alone = images.iter().position(|i| !is_grouped(i))?;
            Some((index, grouped, alone))
        })
        .expect("a sheet with a grouped picture and a picture on its own");
    let sheet = book.sheet_mut(index).unwrap();
    let removed = [
        sheet.images[alone].name.clone(),
        sheet.images[grouped].name.clone(),
    ];
    let group_anchor = sheet.images[grouped].anchor;
    let (first, second) = (alone.max(grouped), alone.min(grouped));
    sheet.images.remove(first);
    sheet.images.remove(second);

    let back = rewrite(&book);
    assert_eq!(all_images(&back).len(), before - 2);
    let names: Vec<&str> = back.sheets()[index]
        .images
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    for name in &removed {
        assert!(!names.contains(&name.as_str()), "{name} is still there");
    }
    // The group itself stays, with its shapes: its anchor still holds an
    // object in the drawing.
    let drawing = back.sheets()[index].images[0]
        .origin
        .as_ref()
        .unwrap()
        .drawing()
        .to_owned();
    let xml = String::from_utf8(part(&back, &drawing).to_vec()).unwrap();
    let Anchor::TwoCell { from, .. } = group_anchor else {
        panic!("{group_anchor:?}")
    };
    let marker = format!(
        "<xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff><xdr:row>{}</xdr:row>",
        from.col.index(),
        from.col_offset,
        from.row.index()
    );
    assert!(xml.contains(&marker), "the group is gone");
    for (a, b) in book.sheets()[index]
        .images
        .iter()
        .zip(&back.sheets()[index].images)
    {
        assert_eq!(said(a), said(b));
    }
}

#[test]
fn a_picture_made_in_code_is_written_on_a_sheet_with_no_drawing() {
    let mut book = Spreadsheet::new();
    let mut image = Image::new(
        PNG.to_vec(),
        Anchor::TwoCell {
            from: at(1, 1),
            to: at(3, 5),
            edit_as: None,
        },
    )
    .unwrap();
    image.description = "a dot".to_owned();
    book.sheet_mut(0).unwrap().images.push(image.clone());
    book.sheet_mut(0)
        .unwrap()
        .set(CellRef::parse("A1").unwrap(), 1.0);

    let back = rewrite(&book);
    let read = &back.sheets()[0].images;
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].data, PNG);
    assert_eq!(read[0].anchor, image.anchor);
    assert_eq!(read[0].description, "a dot");
    assert_eq!(read[0].name, "Picture 1");
    // Writing what was read changes nothing further.
    let again = rewrite(&back);
    assert_eq!(said(&again.sheets()[0].images[0]), said(&read[0]));
}

#[test]
fn a_picture_made_in_code_joins_an_existing_drawing() {
    let mut book = open("chart1.xlsx");
    let before = book.sheets()[0].images.len();
    let image = Image::new(
        PNG.to_vec(),
        Anchor::OneCell {
            from: at(0, 0),
            width: 9525,
            height: 9525,
        },
    )
    .unwrap();
    book.sheet_mut(0).unwrap().images.push(image);
    let back = rewrite(&book);
    assert_eq!(back.sheets()[0].images.len(), before + 1);
    assert!(back.sheets()[0].images.iter().any(|i| i.data == PNG));
    assert_eq!(back.sheets()[0].charts.len(), book.sheets()[0].charts.len());
}

#[test]
fn inserted_rows_move_a_picture_without_touching_it() {
    let mut book = open("chart1.xlsx");
    let row_before = match book.sheets()[0].images[0].anchor {
        Anchor::TwoCell { from, .. } | Anchor::OneCell { from, .. } => from.row.index(),
        Anchor::Absolute { .. } => panic!("absolute"),
    };
    excelerate::edit::insert_rows(&mut book, 0, excelerate::Row::new(0).unwrap(), 3).unwrap();
    let image = &book.sheets()[0].images[0];
    assert!(image.is_unchanged(), "the bytes moved with the model");
    let row_after = match image.anchor {
        Anchor::TwoCell { from, .. } | Anchor::OneCell { from, .. } => from.row.index(),
        Anchor::Absolute { .. } => unreachable!(),
    };
    assert_eq!(row_after, row_before + 3);
    let back = rewrite(&book);
    assert_eq!(back.sheets()[0].images[0].anchor, image.anchor);
}
