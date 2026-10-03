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

// Fill, outline, text font and turn.

use excelerate::model::chart::{ChartColor, ColorTransform, Fill, LineFormat, ShapeFormat};
use excelerate::style::{Color, DiffFont};

/// A shape element in a two-cell anchor, as Excel writes one.
fn anchored(sp: &str, row: u32) -> String {
    format!(
        concat!(
            "<xdr:twoCellAnchor><xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff>",
            "<xdr:row>{row}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>3</xdr:col>",
            "<xdr:colOff>0</xdr:colOff><xdr:row>{end}</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to>",
            "{sp}<xdr:clientData/></xdr:twoCellAnchor>"
        ),
        row = row,
        end = row + 3,
        sp = sp
    )
}

/// `<xdr:sp>` with these properties, style and text body.
fn sp(id: u32, properties: &str, style: &str, body: &str) -> String {
    format!(
        concat!(
            r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="{id}" name="S{id}"/><xdr:cNvSpPr/></xdr:nvSpPr>"#,
            "<xdr:spPr>{properties}</xdr:spPr>{style}{body}</xdr:sp>"
        ),
        id = id,
        properties = properties,
        style = style,
        body = body
    )
}

/// The style Excel gives a shape it draws.
const EXCEL_STYLE: &str = concat!(
    r#"<xdr:style><a:lnRef idx="2"><a:schemeClr val="accent1"><a:shade val="15000"/></a:schemeClr></a:lnRef>"#,
    r#"<a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>"#,
    r#"<a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef>"#,
    r#"<a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></xdr:style>"#
);

const FRAME: &str = r#"<a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>"#;
const EFFECTS: &str = r#"<a:effectLst><a:outerShdw blurRad="40000" dist="23000" dir="5400000"><a:srgbClr val="000000"/></a:outerShdw></a:effectLst>"#;

/// The shapes fixture with its drawing replaced by these anchored shapes.
fn with_shapes(objects: &[String]) -> Spreadsheet {
    use std::io::{Read, Write};

    let path = format!("{}/tests/fixtures/shapes.xlsx", env!("CARGO_MANIFEST_DIR"));
    let mut source = zip::ZipArchive::new(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for i in 0..source.len() {
        let mut part = source.by_index(i).unwrap();
        let name = part.name().to_owned();
        let mut data = String::new();
        part.read_to_string(&mut data).unwrap();
        if name == "xl/drawings/drawing1.xml" {
            data = format!(
                concat!(
                    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
                    r#"<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing" "#,
                    r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">{}</xdr:wsDr>"#
                ),
                objects.concat()
            );
        }
        out.start_file(name, options).unwrap();
        out.write_all(data.as_bytes()).unwrap();
    }
    let bytes = out.finish().unwrap().into_inner();
    read_xlsx_from(Cursor::new(bytes)).unwrap()
}

fn drawing_of(book: &Spreadsheet) -> String {
    let part = book
        .parts
        .iter()
        .find(|p| p.path == "xl/drawings/drawing1.xml")
        .unwrap();
    String::from_utf8(part.data.clone()).unwrap()
}

fn written(book: &Spreadsheet) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_xlsx_to(book, Cursor::new(&mut bytes)).unwrap();
    bytes
}

fn tinted(name: &str, transforms: Vec<ColorTransform>) -> ChartColor {
    ChartColor {
        transforms,
        ..ChartColor::scheme(name)
    }
}

/// The four ways a shape states its look, read.
fn styled_book() -> Spreadsheet {
    let body = concat!(
        r#"<xdr:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r>"#,
        r#"<a:rPr lang="ru-RU" sz="1400" b="1" i="1"><a:solidFill><a:schemeClr val="tx1"><a:lumMod val="75000"/><a:lumOff val="25000"/></a:schemeClr></a:solidFill>"#,
        r#"<a:latin typeface="Arial"/></a:rPr><a:t>own</a:t></a:r></a:p></xdr:txBody>"#
    );
    with_shapes(&[
        anchored(
            &sp(
                2,
                &format!(
                    r#"{FRAME}<a:solidFill><a:srgbClr val="FF8000"/></a:solidFill><a:ln w="19050"><a:solidFill><a:schemeClr val="accent2"/></a:solidFill></a:ln>"#
                ),
                EXCEL_STYLE,
                body,
            ),
            1,
        ),
        anchored(
            &sp(
                3,
                &format!(
                    r#"{FRAME}<a:solidFill><a:schemeClr val="accent1"><a:lumMod val="60000"/><a:lumOff val="40000"/></a:schemeClr></a:solidFill>{EFFECTS}"#
                ),
                EXCEL_STYLE,
                r#"<xdr:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>kept</a:t></a:r></a:p></xdr:txBody>"#,
            ),
            5,
        ),
        anchored(
            &sp(
                4,
                &format!("{FRAME}<a:noFill/><a:ln><a:noFill/></a:ln>"),
                EXCEL_STYLE,
                "",
            ),
            9,
        ),
        // From a file Excel saved: an arrow turned by 290.8 degrees, its
        // anchor the frame turned a quarter.
        anchored(
            &sp(
                5,
                r#"<a:xfrm rot="17446704" flipV="1"><a:off x="7437118" y="3177540"/><a:ext cx="1135380" cy="403860"/></a:xfrm><a:prstGeom prst="rightArrow"><a:avLst/></a:prstGeom>"#,
                EXCEL_STYLE,
                "",
            ),
            13,
        ),
    ])
}

#[test]
fn fill_outline_and_font_are_read_with_the_style_behind_them() {
    let book = styled_book();
    let shapes = &book.sheet(0).unwrap().shapes;
    assert_eq!(shapes.len(), 4);

    // Its own fill, outline and run font.
    let own = &shapes[0];
    assert_eq!(
        own.format.fill,
        Some(Fill::Solid(ChartColor::rgb(0xFF_8000)))
    );
    let line = LineFormat {
        fill: Some(Fill::Solid(ChartColor::scheme("accent2"))),
        width: Some(19_050),
    };
    assert_eq!(own.format.line.as_ref(), Some(&line));
    assert_eq!(
        own.effective_fill(),
        Fill::Solid(ChartColor::rgb(0xFF_8000))
    );
    assert_eq!(own.effective_line(), line);
    assert_eq!(
        own.font,
        DiffFont {
            name: Some("Arial".into()),
            size: Some(1400),
            bold: Some(true),
            italic: Some(true),
            // "Text 1, lighter 25%" in Excel's palette.
            color: Some(Color::Theme {
                id: 1,
                tint: 250_000
            }),
            ..DiffFont::default()
        }
    );

    // A theme colour lightened, and the outline and text colour the style's.
    let themed = &shapes[1];
    let light = tinted(
        "accent1",
        vec![
            ColorTransform::LumMod(60_000),
            ColorTransform::LumOff(40_000),
        ],
    );
    assert_eq!(themed.effective_fill(), Fill::Solid(light));
    let dark = tinted("accent1", vec![ColorTransform::Shade(15_000)]);
    assert_eq!(themed.format.line, None);
    assert_eq!(
        themed.effective_line().fill,
        Some(Fill::Solid(dark.clone()))
    );
    assert_eq!(themed.font.color, None);
    assert_eq!(
        themed.effective_font().color,
        Some(Color::Theme { id: 0, tint: 0 })
    );

    // Not filled, not outlined, whatever the style says.
    let bare = &shapes[2];
    assert_eq!(bare.effective_fill(), Fill::None);
    assert_eq!(bare.effective_line().fill, Some(Fill::None));

    // Only the style: the first accent, outlined darker.
    let styled = &shapes[3];
    assert_eq!(styled.format.fill, None);
    assert_eq!(
        styled.effective_fill(),
        Fill::Solid(ChartColor::scheme("accent1"))
    );
    assert_eq!(styled.effective_line().fill, Some(Fill::Solid(dark)));
    assert_eq!(styled.effective_line().width, None);
    assert_eq!(
        (styled.rotation, styled.flip_h, styled.flip_v),
        (17_446_704, false, true)
    );
    assert!(shapes.iter().all(Shape::is_unchanged));

    // Untouched, the drawing goes back byte for byte.
    assert_eq!(drawing_of(&rewrite(&book)), drawing_of(&book));
}

#[test]
fn a_changed_fill_is_written_into_the_properties_and_the_rest_stays() {
    let mut book = styled_book();
    let before = drawing_of(&book);
    let shapes = &mut book.sheet_mut(0).unwrap().shapes;
    let green = Fill::Solid(ChartColor::rgb(0x00_B050));
    shapes[1].format.fill = Some(green.clone());
    shapes[1].format.line = Some(LineFormat {
        fill: Some(Fill::None),
        width: None,
    });
    shapes[3].rotation = 5_400_000;
    shapes[3].flip_v = false;
    let expected: Vec<Shape> = shapes.clone();

    let back = rewrite(&book);
    let shapes = &back.sheet(0).unwrap().shapes;
    assert_eq!(shapes[1].effective_fill(), green);
    assert_eq!(shapes[1].effective_line().fill, Some(Fill::None));
    assert_eq!(
        (shapes[3].rotation, shapes[3].flip_h, shapes[3].flip_v),
        (5_400_000, false, false)
    );
    for (got, want) in shapes.iter().zip(&expected) {
        assert_eq!(said(got), said(want));
        assert_eq!(got.font, want.font);
    }
    let drawing = drawing_of(&back);
    // The effects, the outline and the text of the changed shape stay, and
    // the shapes nobody touched are as they were.
    assert!(drawing.contains(EFFECTS));
    assert!(drawing.contains(r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill"#));
    assert!(drawing.contains("<a:t>kept</a:t>"));
    assert!(!drawing.contains("lumMod val=\"60000\""));
    let untouched = |xml: &str, id: u32| {
        let start = xml.find(&format!(r#"id="{id}""#)).unwrap();
        let end = start + xml[start..].find("</xdr:sp>").unwrap();
        xml[start..end].to_owned()
    };
    assert_eq!(untouched(&drawing, 2), untouched(&before, 2));
    assert_eq!(untouched(&drawing, 4), untouched(&before, 4));
    assert!(untouched(&drawing, 5).contains(r#"<a:xfrm rot="5400000" flipV="0""#));

    // Written again untouched, it is a fixed point.
    assert_eq!(written(&rewrite(&back)), written(&rewrite(&rewrite(&back))));
}

#[test]
fn a_changed_font_is_written_to_the_runs_of_the_new_text() {
    let mut book = styled_book();
    let shape = &mut book.sheet_mut(0).unwrap().shapes[1];
    shape.font.color = Some(Color::Theme {
        id: 5,
        tint: -250_000,
    });
    shape.font.bold = Some(true);
    shape.font.size = Some(1800);
    let font = shape.font.clone();
    let back = rewrite(&book);
    let shape = &back.sheet(0).unwrap().shapes[1];
    assert_eq!(shape.font, font);
    assert_eq!(shape.text, "kept");

    assert!(drawing_of(&back).contains(r#"<a:rPr lang="en-US" sz="1800" b="1">"#));
}

#[test]
fn a_new_shape_with_its_own_look_goes_round() {
    let mut book = Spreadsheet::empty();
    let mut sheet = excelerate::model::Worksheet::new("S").unwrap();
    let mut shape = Shape::new(
        "rightArrow",
        Anchor::TwoCell {
            from: at(1, 1),
            to: at(3, 5),
            edit_as: None,
        },
    );
    shape.format = ShapeFormat {
        fill: Some(Fill::Solid(tinted(
            "accent6",
            vec![ColorTransform::LumMod(75_000)],
        ))),
        line: Some(LineFormat {
            fill: Some(Fill::Solid(ChartColor::rgb(0xC0_0000))),
            width: Some(28_575),
        }),
        source: None,
    };
    shape.rotation = 2_700_000;
    shape.flip_h = true;
    shape.text = "go".into();
    shape.font = DiffFont {
        size: Some(1200),
        italic: Some(true),
        color: Some(Color::Argb(0xFF12_3456)),
        ..DiffFont::default()
    };
    // Nothing of its own: the style Excel gives a new shape.
    let plain = Shape::new(
        "rect",
        Anchor::TwoCell {
            from: at(5, 1),
            to: at(7, 5),
            edit_as: None,
        },
    );
    assert_eq!(
        plain.effective_fill(),
        Fill::Solid(ChartColor::scheme("accent1"))
    );
    sheet.shapes.push(shape.clone());
    sheet.shapes.push(plain);
    book.add_sheet(sheet).unwrap();

    let back = rewrite(&book);
    let shapes = &back.sheet(0).unwrap().shapes;
    let got = &shapes[0];
    assert_eq!(got.format.fill, shape.format.fill);
    assert_eq!(got.format.line, shape.format.line);
    assert_eq!(
        (got.rotation, got.flip_h, got.flip_v),
        (2_700_000, true, false)
    );
    assert_eq!(got.font, shape.font);
    assert_eq!(got.text, "go");
    assert_eq!(shapes[1].format.fill, None);
    assert_eq!(
        shapes[1].effective_fill(),
        Fill::Solid(ChartColor::scheme("accent1"))
    );
    assert_eq!(
        shapes[1].effective_font().color,
        Some(Color::Theme { id: 0, tint: 0 })
    );
}
