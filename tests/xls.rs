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
            Some(excelerate::model::CellValue::Text(text)) => text.to_string(),
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

fn formula_of(book: &Spreadsheet, sheet: usize, at: &str) -> Option<String> {
    match &book.sheets()[sheet].get(CellRef::parse(at).unwrap())?.value {
        excelerate::model::CellValue::Formula { formula, .. } => Some(formula.clone()),
        _ => None,
    }
}

#[test]
fn formulas_from_the_xlwt_fixture_survive_our_writer() {
    let original = open("formulas.xls");
    let back = rewrite(&original);
    for row in 4..=31 {
        let at = format!("B{row}");
        assert_eq!(
            formula_of(&back, 0, &at),
            formula_of(&original, 0, &at),
            "{at}"
        );
    }
}

/// What xlwt cannot write: every kind of token our compiler emits, read back
/// by our decompiler to the text it started from.
#[test]
fn every_kind_of_formula_survives_a_round_trip() {
    use excelerate::model::{CellValue, DefinedName, Worksheet};
    let mut book = Spreadsheet::new();
    book.add_sheet(Worksheet::new("Данные 2").unwrap()).unwrap();
    book.add_sheet(Worksheet::new("R1C1").unwrap()).unwrap();
    book.defined_names.push(DefinedName {
        name: "Rate".to_owned(),
        sheet: None,
        formula: "'Данные 2'!$B$2".to_owned(),
        hidden: false,
    });
    book.defined_names.push(DefinedName {
        name: "_xlnm.Print_Area".to_owned(),
        sheet: Some(0),
        formula: "Worksheet!$A$1:$C$10".to_owned(),
        hidden: false,
    });
    let formulas = [
        "(A1+B1)*2",
        "A1-(B1-C1)",
        "A1-B1-C1",
        "-(A1^2)",
        "-A1^2",
        "2^3^2",
        "A1&(B1=\"x\")",
        "$A1+A$1+$A$1",
        "SUM($B4:D$9)",
        "SUM(A:A,$2:$3)",
        "SUM('Данные 2'!A1:B3)",
        "SUM('Worksheet:R1C1'!A1)",
        "'R1C1'!A1",
        "Rate*2",
        "IF(A1>0,\"yes\",\"no\")",
        "IF(A1,1)",
        "CHOOSE(2,\"a\",B1,3)",
        "SUM((A1,B1))",
        "SUM(A1:B2 B1:C3)",
        "INDEX({1,2;3,4},2,1)",
        "{-1.5,\"q\";TRUE,#N/A}",
        "EOMONTH(A1,0)",
        "IFERROR(1/0,\"none\")",
        "VLOOKUP(A1,'Данные 2'!A:B,2,FALSE)",
        "ROUND(PI(),2)",
        "A1%",
        "NOW()",
        "COUNTIF(A1:A9,\">3\")",
    ];
    let sheet = book.sheet_mut(0).unwrap();
    for (i, formula) in formulas.iter().enumerate() {
        let cell = sheet.entry(CellRef::parse(&format!("D{}", i + 1)).unwrap());
        cell.value = CellValue::Formula {
            formula: (*formula).to_owned(),
            cached: Some(Box::new(CellValue::Number(0.0))),
        };
    }
    // A text result goes out in a STRING record and has to come back.
    sheet.entry(CellRef::parse("E1").unwrap()).value = CellValue::Formula {
        formula: "\"a\"&\"b\"".to_owned(),
        cached: Some(Box::new(CellValue::text("ab"))),
    };

    let back = rewrite(&book);
    for (i, formula) in formulas.iter().enumerate() {
        let at = format!("D{}", i + 1);
        assert_eq!(formula_of(&back, 0, &at).as_deref(), Some(*formula), "{at}");
    }
    assert_eq!(
        back.sheets()[0]
            .get(CellRef::parse("E1").unwrap())
            .map(|c| c.value.clone()),
        Some(CellValue::Formula {
            formula: "\"a\"&\"b\"".to_owned(),
            cached: Some(Box::new(CellValue::text("ab"))),
        })
    );
    assert_eq!(back.defined_names, book.defined_names);
}

/// What BIFF8 cannot hold is written as the value it shows, not dropped.
#[test]
fn a_formula_the_format_cannot_hold_is_written_as_its_value() {
    use excelerate::model::CellValue;
    let mut book = Spreadsheet::new();
    let sheet = book.sheet_mut(0).unwrap();
    sheet.entry(CellRef::parse("A1").unwrap()).value = CellValue::Formula {
        formula: "SUM(A70000:A70001)".to_owned(),
        cached: Some(Box::new(CellValue::Number(5.0))),
    };
    let back = rewrite(&book);
    assert_eq!(
        back.sheets()[0]
            .get(CellRef::parse("A1").unwrap())
            .map(|c| c.value.clone()),
        Some(CellValue::Number(5.0))
    );
}

#[test]
fn the_normal_font_survives_a_round_trip() {
    // Column widths are counted in digits of this font, so losing it to the
    // Calibri 11 default made every column of an Arial 8 book wider.
    let mut normal = Style::default();
    normal.font.name = "Arial".to_owned();
    normal.font.set_size_points(8.0);
    let mut book = Spreadsheet::empty();
    book.styles = excelerate::style::StyleTable::from_styles(vec![normal.clone()]);
    let mut sheet = excelerate::model::Worksheet::new("S").unwrap();
    sheet.set(CellRef::parse("A1").unwrap(), 1.0);
    book.add_sheet(sheet).unwrap();
    let back = rewrite(&book);
    let font = &back
        .styles
        .get(excelerate::style::StyleId::default())
        .unwrap()
        .font;
    assert_eq!((font.name.as_str(), font.size_points()), ("Arial", 8.0));
    assert_eq!(style(&back, "A1").font, normal.font);
}

#[test]
fn outline_levels_and_summary_placement_survive_a_round_trip() {
    use excelerate::{Col, Row};
    let mut book = Spreadsheet::new();
    let sheet = book.sheet_mut(0).unwrap();
    sheet.properties.summary_below = false;
    sheet.properties.summary_right = false;
    for r in 1..=3 {
        let row = sheet.rows.entry(Row::new(r).unwrap()).or_default();
        row.outline_level = 2;
        row.hidden = true;
    }
    sheet
        .rows
        .entry(Row::new(0).unwrap())
        .or_default()
        .collapsed = true;
    let run = sheet.column_entry(Col::new(2).unwrap());
    run.outline_level = 1;
    run.collapsed = true;

    let back = rewrite(&book);
    let sheet = &back.sheets()[0];
    assert!(!sheet.properties.summary_below);
    assert!(!sheet.properties.summary_right);
    assert_eq!(sheet.row_outline_level(Row::new(2).unwrap()), 2);
    assert!(sheet.rows[&Row::new(0).unwrap()].collapsed);
    let run = sheet.column_run(Col::new(2).unwrap()).unwrap();
    assert_eq!((run.outline_level, run.collapsed), (1, true));
}

#[test]
fn frozen_panes_selections_and_window_switches_survive_a_round_trip() {
    use excelerate::Range;
    use excelerate::model::{Pane, PanePosition, PaneState, Selection};
    let mut book = Spreadsheet::new();
    let view = &mut book.sheet_mut(0).unwrap().view;
    view.pane = Some(Pane {
        x_split: 4,
        y_split: 11,
        top_left_cell: Some(CellRef::parse("E1459").unwrap()),
        active_pane: PanePosition::BottomRight,
        state: PaneState::Frozen,
    });
    // Gridlines are bit 1 of `WINDOW2`; the writer used to clear bit 5, the
    // default gridline colour, and leave the grid on.
    view.show_grid_lines = false;
    view.show_zeros = false;
    view.top_left_cell = Some(CellRef::parse("B3").unwrap());
    // A selection per pane; ONLYOFFICE reads the one in the active pane off
    // the source workbook as E12:G12 with the cursor on E12.
    view.selections = vec![
        Selection {
            pane: Some(PanePosition::BottomLeft),
            active_cell: Some(CellRef::parse("A12").unwrap()),
            sqref: vec![Range::parse("A12").unwrap()],
        },
        Selection {
            pane: Some(PanePosition::BottomRight),
            active_cell: Some(CellRef::parse("F12").unwrap()),
            sqref: vec![
                Range::parse("B20").unwrap(),
                Range::parse("E12:G12").unwrap(),
            ],
        },
    ];

    let expected = book.sheets()[0].view.clone();
    assert_eq!(rewrite(&book).sheets()[0].view, expected);
}
