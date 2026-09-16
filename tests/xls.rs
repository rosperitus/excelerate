//! xls beyond values: cell formats, formulas and the round trip through our
//! own writer.
//!
//! `fixtures/styles.xls` is written by the Python `xlwt` package, not by us;
//! every expectation beside it is what `xlrd` (a reader written independently
//! of both) printed for the same file.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::CellRef;
use excelerate::model::Spreadsheet;
use excelerate::reader::xls::read_xls_from;
use excelerate::style::{
    BorderStyle, Color, DiagonalDirection, HorizontalAlign, Pattern, ProtectionState, Script,
    Style, Underline, VerticalAlign,
};
use excelerate::writer::xls::write_xls_to;

fn open(name: &str) -> Spreadsheet {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    read_xls_from(&std::fs::read(path).unwrap()).unwrap()
}

fn rewrite(book: &Spreadsheet) -> Spreadsheet {
    let mut bytes = Vec::new();
    write_xls_to(book, &mut bytes).unwrap();
    read_xls_from(&bytes).unwrap()
}

fn style<'a>(book: &'a Spreadsheet, at: &str) -> &'a Style {
    let cell = book.sheets()[0].get(CellRef::parse(at).unwrap()).unwrap();
    book.styles.get(cell.style).unwrap()
}

fn rgb(value: u32) -> Color {
    Color::Argb(0xFF00_0000 | value)
}

/// Checks every style of the fixture, so a writer can be held to the same.
fn assert_fixture_styles(book: &Spreadsheet) {
    let bold = style(book, "A1");
    assert_eq!(bold.font.name, "Arial");
    assert!(bold.font.bold);
    assert_eq!(bold.font.size, 1400, "280 twips is 14pt");
    assert_eq!(bold.font.color, rgb(0xFF_0000));

    let italic = style(book, "A2");
    assert_eq!(italic.font.name, "Times New Roman");
    assert!(italic.font.italic && italic.font.strike && !italic.font.bold);
    assert_eq!(italic.font.underline, Underline::Double);
    assert_eq!(italic.font.color, Color::Auto, "the system text colour");

    assert_eq!(style(book, "A3").font.script, Script::Superscript);

    let fill = &style(book, "A4").fill;
    assert_eq!(fill.pattern, Pattern::Solid);
    assert_eq!(fill.foreground, rgb(0xFF_FF00));

    let hatch = &style(book, "A5").fill;
    assert_eq!(hatch.pattern, Pattern::parse("lightHorizontal"));
    assert_eq!(hatch.foreground, rgb(0x00_00FF));
    assert_eq!(hatch.background, rgb(0xFF_FFFF));

    let borders = &style(book, "A6").borders;
    assert_eq!(borders.left.style, BorderStyle::parse("thin"));
    assert_eq!(borders.right.style, BorderStyle::parse("medium"));
    assert_eq!(borders.top.style, BorderStyle::parse("dashed"));
    assert_eq!(borders.bottom.style, BorderStyle::parse("double"));
    assert_eq!(borders.left.color, rgb(0xFF_0000));
    assert_eq!(borders.bottom.color, rgb(0x00_00FF));

    let align = &style(book, "A7").alignment;
    assert_eq!(align.horizontal, HorizontalAlign::Center);
    assert_eq!(align.vertical, VerticalAlign::Top);
    assert!(align.wrap_text);
    assert_eq!(align.text_rotation, 45);

    // The file redefines palette entry 0x21; the colour is the one it wrote.
    assert_eq!(style(book, "A8").fill.foreground, rgb(0x12_3456));

    let protection = style(book, "A9").protection;
    assert_eq!(protection.locked, ProtectionState::Off);
    assert_eq!(protection.hidden, ProtectionState::On);
    assert_eq!(
        style(book, "A1").protection.locked,
        ProtectionState::Inherit
    );

    let diagonal = &style(book, "A10").borders;
    assert_eq!(diagonal.diagonal_direction, DiagonalDirection::Down);
    assert_eq!(diagonal.diagonal.style, BorderStyle::parse("thin"));
    assert_eq!(diagonal.diagonal.color, rgb(0x00_8000));
}

#[test]
fn cell_formats_read_as_another_reader_sees_them() {
    assert_fixture_styles(&open("styles.xls"));
}

#[test]
fn cell_formats_survive_our_writer() {
    assert_fixture_styles(&rewrite(&open("styles.xls")));
}

/// A workbook from xlsx names colours the default palette lacks, and more of
/// them than one palette holds. The first ones get exact entries; the rest
/// get the nearest.
#[test]
fn colours_outside_the_palette_take_free_entries() {
    let mut book = Spreadsheet::new();
    let mut ids = Vec::new();
    for i in 0..60u32 {
        let mut grey = Style::default();
        grey.fill.pattern = Pattern::Solid;
        // Sixty greys a step apart, none of them in the default palette
        // except by accident.
        grey.fill.foreground = rgb(0x01_0101 * (i * 4 + 1));
        ids.push(book.styles.intern(grey));
    }
    let sheet = book.sheet_mut(0).unwrap();
    for (row, id) in ids.iter().enumerate() {
        let cell = sheet.entry(CellRef::parse(&format!("A{}", row + 1)).unwrap());
        cell.value = excelerate::model::CellValue::Number(1.0);
        cell.style = *id;
    }
    let back = rewrite(&book);
    let exact = (1..=60)
        .filter(|row| {
            let wanted = rgb(0x01_0101 * ((row - 1) * 4 + 1));
            style(&back, &format!("A{row}")).fill.foreground == wanted
        })
        .count();
    assert!(exact >= 56, "only {exact} of 60 colours came back exactly");
    let last = &style(&back, "A60").fill.foreground;
    let Color::Argb(argb) = last else {
        panic!("{last:?}")
    };
    let grey = argb & 0xFF;
    assert!(grey.abs_diff(237) <= 16, "the nearest grey, got {grey}");
}

#[test]
fn a_theme_colour_is_written_as_the_colour_it_shows() {
    let mut book = Spreadsheet::new();
    let mut accent = Style::default();
    // Accent 1 of the default Office theme.
    accent.font.color = Color::Theme { id: 4, tint: 0 };
    let id = book.styles.intern(accent);
    let cell = book
        .sheet_mut(0)
        .unwrap()
        .entry(CellRef::parse("A1").unwrap());
    cell.value = excelerate::model::CellValue::text("x");
    cell.style = id;
    assert_eq!(style(&rewrite(&book), "A1").font.color, rgb(0x44_72C4));
}

/// `fixtures/formulas.xls` is written by `xlwt`, whose formula compiler is
/// independent of ours: column A holds the text it was given and column B
/// the tokens it compiled from that text. Every one has to come back as the
/// text it started from.
#[test]
fn formulas_decompile_to_the_text_they_were_compiled_from() {
    let book = open("formulas.xls");
    let sheet = &book.sheets()[0];
    let mut checked = 0;
    for row in 4..=31 {
        let at = |col: &str| CellRef::parse(&format!("{col}{row}")).unwrap();
        let source = match sheet.get(at("A")).map(|c| &c.value) {
            Some(excelerate::model::CellValue::Text(text)) => text.clone(),
            other => panic!("A{row} holds {other:?}"),
        };
        match sheet.get(at("B")).map(|c| &c.value) {
            Some(excelerate::model::CellValue::Formula { formula, .. }) => {
                assert_eq!(formula, &source, "B{row}");
            }
            other => panic!("B{row} holds {other:?}"),
        }
        checked += 1;
    }
    assert_eq!(checked, 28);
}
