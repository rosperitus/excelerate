//! The project invariant: read -> write -> read returns the same model.
//!
//! Every later phase leans on this. A format detail that survives reading but
//! is dropped on writing shows up here and nowhere else, so this suite grows
//! with each feature rather than being written once.

// A failing assertion is this suite's output; panicking is the report.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::{CellRef, Range};
use excelerate::model::{CellValue, Spreadsheet, Worksheet};
use excelerate::reader::xlsx::read_xlsx_from;
use excelerate::style::{NumberFormat, Style};
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::Cursor;

fn at(a: &str) -> CellRef {
    CellRef::parse(a).expect("test reference is valid")
}

/// Writes a workbook to memory and reads it straight back.
fn cycle(book: &Spreadsheet) -> Spreadsheet {
    let mut buf = Vec::new();
    write_xlsx_to(book, Cursor::new(&mut buf)).expect("workbook writes");
    read_xlsx_from(Cursor::new(buf)).expect("what we wrote reads back")
}

/// Attachments in a fixed order, so two lists of the same relationships
/// compare equal however they were built.
fn sorted(attachments: &[excelerate::model::Attachment]) -> Vec<&excelerate::model::Attachment> {
    let mut out: Vec<_> = attachments.iter().collect();
    out.sort_by(|a, b| a.target.cmp(&b.target));
    out
}

/// Compares the parts of two workbooks this phase claims to preserve.
#[allow(clippy::too_many_lines)]
fn assert_same(before: &Spreadsheet, after: &Spreadsheet) {
    assert_eq!(before.sheets().len(), after.sheets().len(), "sheet count");
    assert_eq!(
        before.active_index(),
        after.active_index(),
        "the tab the workbook opens on"
    );
    assert_eq!(before.defined_names, after.defined_names, "defined names");
    assert_eq!(before.epoch, after.epoch, "the workbook's base date");
    assert_eq!(
        (&before.workbook_properties, &before.calculation_properties),
        (&after.workbook_properties, &after.calculation_properties),
        "workbook and calculation properties"
    );
    assert_eq!(
        before.attachments, after.attachments,
        "workbook attachments"
    );
    assert_eq!(before.doc_props, after.doc_props, "document properties");
    assert_eq!(
        before.protection, after.protection,
        "what the workbook locks"
    );
    let (mut a, mut b) = (before.parts.clone(), after.parts.clone());
    a.sort_by(|x, y| x.path.cmp(&y.path));
    b.sort_by(|x, y| x.path.cmp(&y.path));
    assert_eq!(a, b, "carried parts");
    // A workbook built in memory has no theme; the writer gives the file the
    // default one, and reading it back finds it. What must not happen is a
    // theme the file did have being replaced.
    if before.theme.is_some() {
        assert_eq!(before.theme, after.theme, "the theme part");
    } else {
        assert!(
            after.theme.is_some(),
            "a written file always carries a theme"
        );
    }
    for (a, b) in before.sheets().iter().zip(after.sheets()) {
        assert_eq!(a.title(), b.title(), "sheet title");
        assert_eq!(a.merges, b.merges, "merges on {}", a.title());
        assert_eq!(a.view, b.view, "view state on {}", a.title());
        assert_eq!(
            a.data_validations,
            b.data_validations,
            "data validations on {}",
            a.title()
        );
        assert_eq!(
            a.properties,
            b.properties,
            "sheet properties on {}",
            a.title()
        );
        assert_eq!(a.margins, b.margins, "margins on {}", a.title());
        assert_eq!(a.page_setup, b.page_setup, "page setup on {}", a.title());
        assert_eq!(
            a.print_options,
            b.print_options,
            "print options on {}",
            a.title()
        );
        assert_eq!(
            a.header_footer,
            b.header_footer,
            "header and footer on {}",
            a.title()
        );
        assert_eq!(
            (&a.row_breaks, &a.col_breaks),
            (&b.row_breaks, &b.col_breaks),
            "page breaks on {}",
            a.title()
        );
        assert_eq!(a.hyperlinks, b.hyperlinks, "hyperlinks on {}", a.title());
        assert_eq!(
            a.conditional_formats,
            b.conditional_formats,
            "conditional formats on {}",
            a.title()
        );
        assert_eq!(
            a.protection,
            b.protection,
            "what editing refuses on {}",
            a.title()
        );
        assert_eq!(
            a.protected_ranges,
            b.protected_ranges,
            "protected ranges on {}",
            a.title()
        );
        assert_eq!(a.auto_filter, b.auto_filter, "auto filter on {}", a.title());
        // A set of relationships has no order of its own - the reader imposes
        // one so that two reads of a file agree.
        assert_eq!(
            sorted(&a.attachments),
            sorted(&b.attachments),
            "attachments on {}",
            a.title()
        );
        assert_eq!(a.columns, b.columns, "column runs on {}", a.title());
        assert_eq!(a.rows, b.rows, "row properties on {}", a.title());
        assert_eq!(
            (a.default_column_width, a.default_row_height),
            (b.default_column_width, b.default_row_height),
            "sheet defaults on {}",
            a.title()
        );

        let cells_a: Vec<_> = a.iter().map(|(r, c)| (r, c.value.clone())).collect();
        let cells_b: Vec<_> = b.iter().map(|(r, c)| (r, c.value.clone())).collect();
        assert_eq!(cells_a, cells_b, "cells on {}", a.title());

        // Styles are compared by value, not by id: the writer is free to
        // renumber them, but the style a cell resolves to must be identical.
        for ((r, ca), (_, cb)) in a.iter().zip(b.iter()) {
            assert_eq!(
                before.styles.get(ca.style),
                after.styles.get(cb.style),
                "style at {}!{r}",
                a.title()
            );
        }
    }
}

#[test]
fn every_value_type_survives() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Values").unwrap();
    sheet.set(at("A1"), "plain text");
    sheet.set(at("A2"), "punctuation & markup <tag> \"quotes\" 'apos'");
    sheet.set(at("A3"), "многострочный\nтекст\tс табом");
    sheet.set(at("A4"), "   leading and trailing   ");
    sheet.set(at("B1"), 42.0);
    sheet.set(at("B2"), -3.5);
    sheet.set(at("B3"), 0.0);
    sheet.set(at("B4"), 1e-7);
    sheet.set(at("B5"), 1.797_693_134_862_315_7e308);
    sheet.set(at("C1"), true);
    sheet.set(at("C2"), false);
    sheet.entry(at("D1")).value = CellValue::Error(excelerate::CellError::Div0);
    book.add_sheet(sheet).unwrap();

    assert_same(&book, &cycle(&book));
}

#[test]
fn formulas_keep_their_cached_result() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Formulas").unwrap();
    for (cell, formula, cached) in [
        ("A1", "1+1", CellValue::Number(2.0)),
        ("A2", "B1*2", CellValue::Number(84.0)),
        ("A3", r#"CONCAT("a","b")"#, CellValue::Text("ab".into())),
        ("A4", "1>0", CellValue::Bool(true)),
        ("A5", "1/0", CellValue::Error(excelerate::CellError::Div0)),
        ("A6", "SUM(B1:B9)", CellValue::Number(0.0)),
    ] {
        sheet.entry(at(cell)).value = CellValue::Formula {
            formula: formula.into(),
            cached: Some(Box::new(cached)),
        };
    }
    // A formula that has never been calculated has no cached value.
    sheet.entry(at("A7")).value = CellValue::Formula {
        formula: "TODAY()".into(),
        cached: None,
    };
    book.add_sheet(sheet).unwrap();

    assert_same(&book, &cycle(&book));
}

#[test]
fn sheets_keep_their_order_names_and_merges() {
    let mut book = Spreadsheet::empty();
    for name in ["First", "Второй лист", "Third (last)"] {
        let mut sheet = Worksheet::new(name).unwrap();
        sheet.set(at("A1"), name);
        sheet.merges.push(Range::parse("B2:C3").unwrap());
        book.add_sheet(sheet).unwrap();
    }
    let after = cycle(&book);
    assert_same(&book, &after);
    let names: Vec<&str> = after.sheets().iter().map(Worksheet::title).collect();
    assert_eq!(names, ["First", "Второй лист", "Third (last)"], "tab order");
}

#[test]
fn number_formats_survive() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Formats").unwrap();
    let formats = [
        NumberFormat::General,
        NumberFormat::Builtin(9),
        NumberFormat::Custom("yyyy-mm-dd".into()),
        NumberFormat::Custom(r#"0.00" \u{20bd}""#.into()),
    ];
    for (i, format) in formats.iter().enumerate() {
        let id = book.styles.intern(Style {
            number_format: format.clone(),
            ..Style::default()
        });
        let cell = sheet.entry(at(&format!("A{}", i + 1)));
        cell.value = CellValue::Number(45_658.0);
        cell.style = id;
    }
    book.add_sheet(sheet).unwrap();

    let after = cycle(&book);
    assert_same(&book, &after);
    let seen: Vec<&NumberFormat> = after.sheets()[0]
        .iter()
        .map(|(_, c)| &after.styles.get(c.style).unwrap().number_format)
        .collect();
    assert_eq!(
        seen,
        formats.iter().collect::<Vec<_>>(),
        "formats in cell order"
    );
}

#[test]
fn a_styled_empty_cell_is_not_dropped() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Styled").unwrap();
    let id = book.styles.intern(Style {
        number_format: NumberFormat::Custom("yyyy-mm-dd".into()),
        ..Style::default()
    });
    sheet.entry(at("C3")).style = id;
    book.add_sheet(sheet).unwrap();

    let after = cycle(&book);
    let cell = after.sheets()[0]
        .get(at("C3"))
        .expect("a cell carrying only a style still has to be written");
    assert!(cell.value.is_empty());
    assert_eq!(
        after.styles.get(cell.style).map(|s| &s.number_format),
        Some(&NumberFormat::Custom("yyyy-mm-dd".into()))
    );
}

#[test]
fn the_reference_file_survives_a_cycle() {
    // The sample written elsewhere is the real test: it was produced by another
    // implementation, so it exercises shapes we would not think to write.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.xlsx");
    let bytes = std::fs::read(path).expect("sample.xlsx exists");
    let original = read_xlsx_from(Cursor::new(bytes)).expect("sample.xlsx reads");
    let after = cycle(&original);
    assert_same(&original, &after);
    // And again, to catch anything that degrades on each pass rather than once.
    assert_same(&after, &cycle(&after));
}

#[test]
fn refuses_to_write_a_workbook_with_no_sheets() {
    let mut buf = Vec::new();
    let err = write_xlsx_to(&Spreadsheet::empty(), Cursor::new(&mut buf))
        .expect_err("a sheetless workbook is not a valid xlsx");
    assert!(err.to_string().contains("no sheets"), "{err}");
}

#[test]
fn style_components_survive() {
    use excelerate::style::{
        Alignment, Border, BorderStyle, Borders, Color, DiagonalDirection, Fill, Font,
        HorizontalAlign, Pattern, Protection, ProtectionState, Script, Underline, VerticalAlign,
    };

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Rich").unwrap();

    let mut font = Font {
        name: "Arial".into(),
        bold: true,
        italic: true,
        underline: Underline::DoubleAccounting,
        strike: true,
        color: Color::Argb(0xFFFF_0000),
        script: Script::Superscript,
        ..Font::default()
    };
    font.set_size_points(14.5);

    let style = Style {
        number_format: NumberFormat::Custom("yyyy-mm-dd".into()),
        font,
        fill: Fill {
            pattern: Pattern::Named("darkGrid"),
            foreground: Color::Argb(0xFF00_FF00),
            background: Color::Indexed(9),
        },
        borders: Borders {
            left: Border {
                style: BorderStyle::Named("thin"),
                color: Color::Argb(0xFF00_00FF),
            },
            right: Border {
                style: BorderStyle::Named("mediumDashDot"),
                color: Color::Auto,
            },
            top: Border::default(),
            bottom: Border {
                style: BorderStyle::Named("double"),
                color: Color::Theme { id: 4, tint: -2499 },
            },
            diagonal: Border {
                style: BorderStyle::Named("hair"),
                color: Color::Auto,
            },
            diagonal_direction: DiagonalDirection::Both,
        },
        alignment: Alignment {
            horizontal: HorizontalAlign::CenterContinuous,
            vertical: VerticalAlign::Distributed,
            wrap_text: true,
            shrink_to_fit: true,
            indent: 3,
            text_rotation: 45,
            reading_order: 2,
        },
        protection: Protection {
            locked: ProtectionState::Off,
            hidden: ProtectionState::On,
        },
    };
    let id = book.styles.intern(style.clone());
    let cell = sheet.entry(at("A1"));
    cell.value = CellValue::Number(45_658.0);
    cell.style = id;
    book.add_sheet(sheet).unwrap();

    let after = cycle(&book);
    assert_same(&book, &after);

    let got = after
        .styles
        .get(after.sheets()[0].get(at("A1")).unwrap().style)
        .unwrap();
    assert_eq!(*got, style, "every component survives unchanged");
}

#[test]
fn the_styles_reference_file_survives_a_cycle() {
    // Written by so it carries shapes we would not think to produce.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/styles.xlsx");
    let bytes = std::fs::read(path).expect("styles.xlsx exists");
    let original = read_xlsx_from(Cursor::new(bytes)).expect("styles.xlsx reads");
    let after = cycle(&original);
    assert_same(&original, &after);
    assert_same(&after, &cycle(&after));
}

#[test]
fn column_widths_and_row_heights_survive() {
    use excelerate::coordinate::Col;
    use excelerate::model::{ColumnRun, RowProperties};

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Sized").unwrap();
    sheet.default_column_width = Some(9.140_625);
    sheet.default_row_height = Some(13.8);

    let styled = book.styles.intern(Style {
        number_format: NumberFormat::Custom("0.00%".into()),
        ..Style::default()
    });
    sheet.columns = vec![
        ColumnRun {
            width: Some(4.425),
            custom_width: true,
            style: Some(styled),
            ..ColumnRun::new(
                Col::from_letters("A").unwrap(),
                Col::from_letters("A").unwrap(),
            )
        },
        ColumnRun {
            width: Some(92.141_666_666_666_67),
            custom_width: true,
            ..ColumnRun::new(
                Col::from_letters("B").unwrap(),
                Col::from_letters("D").unwrap(),
            )
        },
        ColumnRun {
            hidden: true,
            outline_level: 2,
            collapsed: true,
            best_fit: true,
            ..ColumnRun::new(
                Col::from_letters("E").unwrap(),
                Col::from_letters("E").unwrap(),
            )
        },
    ];
    sheet.rows.insert(
        at("A1").row,
        RowProperties {
            height: Some(36.0),
            custom_height: true,
            ..RowProperties::default()
        },
    );
    // A row with a height but no cells at all must still be written.
    sheet.rows.insert(
        at("A9").row,
        RowProperties {
            height: Some(54.75),
            hidden: true,
            outline_level: 1,
            ..RowProperties::default()
        },
    );
    sheet.set(at("A1"), 1.0);
    book.add_sheet(sheet).unwrap();

    let after = cycle(&book);
    assert_same(&book, &after);

    let s = &after.sheets()[0];
    assert_eq!(s.column_width(Col::from_letters("A").unwrap()), Some(4.425));
    assert_eq!(
        s.column_width(Col::from_letters("C").unwrap()),
        Some(92.141_666_666_666_67)
    );
    // A column outside every run falls back to the sheet default.
    assert_eq!(
        s.column_width(Col::from_letters("Z").unwrap()),
        Some(9.140_625)
    );
    assert_eq!(s.row_height(at("A1").row), Some(36.0));
    assert_eq!(
        s.row_height(at("A9").row),
        Some(54.75),
        "an empty row keeps its height"
    );
    assert_eq!(
        s.row_height(at("A5").row),
        Some(13.8),
        "other rows use the default"
    );
}

#[test]
fn view_state_and_dropdowns_survive() {
    use excelerate::model::{
        DataValidation, Pane, PanePosition, PaneState, Selection, SheetViewType, ValidationType,
    };

    let mut book = Spreadsheet::empty();
    book.add_sheet(Worksheet::new("Списки").unwrap()).unwrap();
    let mut sheet = Worksheet::new("Резюме").unwrap();
    sheet.set(at("J17"), "выбор");

    sheet.view.view = SheetViewType::PageBreakPreview;
    sheet.view.tab_selected = true;
    sheet.view.zoom_scale = Some(80);
    sheet.view.zoom_scale_normal = Some(90);
    sheet.view.show_grid_lines = false;
    sheet.view.top_left_cell = Some(at("A5"));
    sheet.view.pane = Some(Pane {
        x_split: 2,
        y_split: 4,
        top_left_cell: Some(at("C5")),
        active_pane: PanePosition::BottomRight,
        state: PaneState::Frozen,
    });
    sheet.view.selections.push(Selection {
        pane: Some(PanePosition::BottomRight),
        active_cell: Some(at("J17")),
        sqref: vec![Range::parse("J17").unwrap(), Range::parse("B2:C4").unwrap()],
    });

    sheet.data_validations.push(DataValidation {
        sqref: vec![Range::parse("J17").unwrap()],
        kind: ValidationType::List,
        allow_blank: true,
        show_input_message: true,
        show_error_message: true,
        // A cross-sheet source with a quoted name and an ampersand: both the
        // apostrophes and the escaping have to survive.
        formula1: "'Выпадающие списки'!$B$90:$BS$90 & \"\"".into(),
        prompt: "Выберите <значение>".into(),
        ..DataValidation::default()
    });
    book.add_sheet(sheet).unwrap();
    book.set_active(1).unwrap();

    let back = cycle(&book);
    assert_same(&book, &back);
    let sheet = back.sheet(1).unwrap();
    assert_eq!(sheet.view.zoom_scale, Some(80));
    assert_eq!(
        sheet.data_validations[0].formula1,
        book.sheet(1).unwrap().data_validations[0].formula1
    );
    assert_eq!(back.active_sheet().unwrap().title(), "Резюме");
}

#[test]
fn printing_and_links_survive() {
    use excelerate::model::{
        DefinedName, Hyperlink, LinkTarget, Orientation, PageBreak, PageMargins, PageSetup,
        PrintOptions,
    };
    use excelerate::style::Color;

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Печать").unwrap();
    sheet.set(at("A1"), "к содержанию");

    sheet.properties.tab_color = Some(Color::Argb(0xFF00_8000));
    sheet.properties.summary_right = false;
    sheet.properties.fit_to_page = true;
    sheet.margins = PageMargins {
        left: 0.236_220_472_440_945,
        right: 0.039_370_078_740_157_5,
        ..PageMargins::default()
    };
    sheet.page_setup = PageSetup {
        paper_size: Some(9),
        orientation: Orientation::Landscape,
        scale: Some(21),
        fit_to_height: Some(0),
        black_and_white: true,
        ..PageSetup::default()
    };
    sheet.print_options = PrintOptions {
        horizontal_centered: true,
        grid_lines: true,
        ..PrintOptions::default()
    };
    sheet.header_footer.different_first = true;
    sheet.header_footer.odd_header = "&LОтчёт&CСтраница &P".into();
    sheet.header_footer.first_footer = "&Rдата: &D".into();
    sheet.row_breaks.push(PageBreak {
        at: 42,
        max: Some(16_383),
        manual: true,
    });
    sheet.col_breaks.push(PageBreak {
        at: 5,
        max: None,
        manual: false,
    });
    sheet.hyperlinks.push(Hyperlink {
        range: Range::parse("A1:K1").unwrap(),
        target: LinkTarget::Inside("'Другой лист'!A1".into()),
        display: Some("1".into()),
        tooltip: None,
    });
    sheet.hyperlinks.push(Hyperlink {
        range: Range::parse("B2").unwrap(),
        // An external address lives in the sheet's relationships, not in the
        // sheet part, so it takes a different road through the writer.
        target: LinkTarget::Outside("https://example.org/отчёт?a=1&b=2".into()),
        display: None,
        tooltip: Some("подсказка".into()),
    });
    book.add_sheet(sheet).unwrap();
    book.add_sheet(Worksheet::new("Другой лист").unwrap())
        .unwrap();
    book.defined_names.push(DefinedName {
        name: "_xlnm.Print_Area".into(),
        sheet: Some(0),
        formula: "Печать!$A$1:$K$50".into(),
        hidden: false,
    });
    book.defined_names.push(DefinedName {
        name: "Ставка".into(),
        sheet: None,
        formula: "0.15".into(),
        hidden: false,
    });

    let back = cycle(&book);
    assert_same(&book, &back);
    let sheet = back.sheet(0).unwrap();
    assert_eq!(sheet.page_setup.scale, Some(21));
    assert_eq!(
        sheet.hyperlinks[1].target,
        LinkTarget::Outside("https://example.org/отчёт?a=1&b=2".into()),
        "the ampersand survives both the escaping and the relationship"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn protection_and_filters_survive() {
    use excelerate::model::{
        AutoFilter, ColumnFilter, CustomFilter, DateGroup, FilterOperator, PasswordHash,
        ProtectedRange, SheetProtection, WorkbookProtection,
    };

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Защита").unwrap();
    sheet.set(at("A1"), "Отдел");
    sheet.set(at("B1"), "Сумма");
    sheet.set(at("C1"), "Дата");

    // The ISO password form, and a flag of each of the three states: on, off,
    // and absent - the absent one must not come back as `Some(false)`.
    sheet.protection = SheetProtection {
        sheet: Some(true),
        objects: Some(false),
        select_locked_cells: Some(true),
        format_cells: None,
        password: Some(PasswordHash::Iso {
            algorithm: "SHA-512".into(),
            hash: "Kd7L2vQ=".into(),
            salt: "8sJ1sw==".into(),
            spin_count: 100_000,
        }),
        ..SheetProtection::default()
    };
    sheet.protected_ranges.push(ProtectedRange {
        name: "Правка сумм".into(),
        sqref: vec![Range::parse("B2:B99").unwrap(), Range::parse("D1").unwrap()],
        password: Some(PasswordHash::Legacy("CC1A".into())),
        security_descriptor: "O:WDG:WDD:(A;;CC;;;S-1-5-21-1)".into(),
    });

    let mut filter = AutoFilter::new(Range::parse("A1:C99").unwrap());
    // A values filter: literals, blanks, and a date given by calendar parts.
    filter.column_at(0).filter = Some(ColumnFilter::Values {
        blank: true,
        values: vec!["Сбыт".into(), "R&D".into(), "<прочее>".into()],
        date_groups: Vec::new(),
    });
    // Two comparisons joined by `and`, one of them with the default operator,
    // which xlsx leaves off entirely.
    filter.column_at(1).filter = Some(ColumnFilter::Custom {
        and: true,
        rules: vec![
            CustomFilter {
                operator: FilterOperator::GreaterThanOrEqual,
                value: "100".into(),
            },
            CustomFilter {
                operator: FilterOperator::Equal,
                value: "*итог*".into(),
            },
        ],
    });
    filter.column_at(2).filter = Some(ColumnFilter::Values {
        blank: false,
        values: Vec::new(),
        date_groups: vec![DateGroup {
            year: Some(2026),
            month: Some(8),
            grouping: "month".into(),
            ..DateGroup::default()
        }],
    });
    sheet.auto_filter = Some(filter);
    book.add_sheet(sheet).unwrap();

    // A second sheet carrying the two filter kinds the first one does not, so
    // every variant of the enum goes through the writer.
    let mut other = Worksheet::new("Прочее").unwrap();
    other.set(at("A1"), "Ставка");
    let mut filter = AutoFilter::new(Range::parse("A1:B50").unwrap());
    filter.column_at(0).filter = Some(ColumnFilter::Top10 {
        value: Some("25".into()),
        percent: true,
        top: false,
        filter_value: Some("3.5".into()),
    });
    filter.column_at(1).filter = Some(ColumnFilter::Dynamic {
        kind: "aboveAverage".into(),
        value: Some("42".into()),
        max_value: None,
    });
    // A column with no criteria at all: the arrow is hidden and nothing is
    // filtered.
    filter.column_at(4).hidden_button = true;
    other.auto_filter = Some(filter);
    book.add_sheet(other).unwrap();

    book.protection = WorkbookProtection {
        lock_structure: Some(true),
        lock_windows: Some(false),
        lock_revision: None,
        workbook_password: Some(PasswordHash::Legacy("83AF".into())),
        revisions_password: Some(PasswordHash::Iso {
            algorithm: "SHA-1".into(),
            hash: "cmV2".into(),
            salt: "c2FsdA==".into(),
            spin_count: 10_000,
        }),
    };

    let back = cycle(&book);
    assert_same(&book, &back);

    let sheet = back.sheet(0).unwrap();
    assert_eq!(
        sheet.protection.format_cells, None,
        "a flag the file never carried must not come back as a decision"
    );
    assert_eq!(
        sheet.auto_filter.as_ref().unwrap().columns.len(),
        3,
        "every filtering column comes back"
    );
    // The literal with an ampersand and the one with angle brackets both pass
    // through the escaping unchanged.
    let ColumnFilter::Values { values, blank, .. } = sheet.auto_filter.as_ref().unwrap().columns[0]
        .filter
        .as_ref()
        .unwrap()
    else {
        panic!("the first column filters by value");
    };
    assert!(blank);
    assert_eq!(values, &["Сбыт", "R&D", "<прочее>"]);
    assert_eq!(
        back.protection.lock_revision, None,
        "an absent lock stays absent"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one long fixture: every rule shape the writer has to handle"
)]
fn conditional_formatting_survives() {
    use excelerate::model::{
        CfOperator, CfRule, CfRuleType, CfScale, CfValue, CfValueType, ConditionalFormat,
    };
    use excelerate::style::{Color, DiffFill, DiffFont, DifferentialStyle, Pattern};

    let mut book = Spreadsheet::empty();
    // Excel's own "light red fill with dark red text": a partial style that
    // must not grow a font name or a size it never had.
    book.styles.differential.push(DifferentialStyle {
        font: Some(DiffFont {
            color: Some(Color::Argb(0xFF9C_0006)),
            bold: Some(true),
            ..DiffFont::default()
        }),
        fill: Some(DiffFill {
            pattern: Some(Pattern::Solid),
            background: Some(Color::Argb(0xFFFF_C7CE)),
            ..DiffFill::default()
        }),
        ..DifferentialStyle::default()
    });

    let mut sheet = Worksheet::new("Правила").unwrap();
    sheet.set(at("A1"), 5.0);
    sheet.conditional_formats.push(ConditionalFormat {
        sqref: vec![
            Range::parse("A1:A10").unwrap(),
            Range::parse("C1:C10").unwrap(),
        ],
        rules: vec![
            CfRule {
                kind: CfRuleType::CellIs,
                priority: 1,
                dxf: Some(0),
                operator: Some(CfOperator::Between),
                formulas: vec!["1".into(), "10".into()],
                ..CfRule::default()
            },
            CfRule {
                kind: CfRuleType::ContainsText,
                priority: 2,
                text: Some("да".into()),
                operator: Some(CfOperator::ContainsText),
                formulas: vec!["NOT(ISERROR(SEARCH(\"да\",A1)))".into()],
                stop_if_true: true,
                ..CfRule::default()
            },
            CfRule {
                kind: CfRuleType::NotContainsBlanks,
                priority: 3,
                formulas: vec!["LEN(TRIM(A1))>0".into()],
                ..CfRule::default()
            },
        ],
    });
    sheet.conditional_formats.push(ConditionalFormat {
        sqref: vec![Range::parse("B1:B10").unwrap()],
        rules: vec![
            CfRule {
                kind: CfRuleType::ColorScale,
                priority: 4,
                scale: Some(CfScale::Color {
                    values: vec![
                        CfValue {
                            kind: CfValueType::Min,
                            ..CfValue::default()
                        },
                        CfValue {
                            kind: CfValueType::Percentile,
                            value: "50".into(),
                            greater_or_equal: true,
                        },
                        CfValue {
                            kind: CfValueType::Max,
                            ..CfValue::default()
                        },
                    ],
                    colors: vec![
                        Color::Argb(0xFFF8_696B),
                        Color::Argb(0xFFFF_EB84),
                        Color::Argb(0xFF63_BE7B),
                    ],
                }),
                ..CfRule::default()
            },
            CfRule {
                kind: CfRuleType::DataBar,
                priority: 5,
                scale: Some(CfScale::DataBar {
                    values: vec![
                        CfValue {
                            kind: CfValueType::Min,
                            ..CfValue::default()
                        },
                        CfValue {
                            kind: CfValueType::Max,
                            ..CfValue::default()
                        },
                    ],
                    color: Color::Argb(0xFF63_8EC6),
                    min_length: Some(10),
                    max_length: Some(90),
                    show_value: false,
                }),
                ..CfRule::default()
            },
        ],
    });
    book.add_sheet(sheet).unwrap();

    let back = cycle(&book);
    assert_same(&book, &back);
    assert_eq!(
        back.styles.differential[0].font.as_ref().unwrap().name,
        None,
        "a differential font must not gain a name it never had"
    );
}

#[test]
fn parts_the_model_does_not_know_are_carried_through() {
    use excelerate::model::{Attachment, OpaquePart};

    let drawing = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing";
    let core =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("С рисунком").unwrap();
    sheet.set(at("A1"), 1.0);
    sheet.attachments.push(Attachment {
        kind: drawing.into(),
        target: "xl/drawings/drawing1.xml".into(),
    });
    book.add_sheet(sheet).unwrap();
    book.doc_props.push(Attachment {
        kind: core.into(),
        target: "docProps/core.xml".into(),
    });
    for (path, kind, data) in [
        (
            "xl/drawings/drawing1.xml",
            "application/vnd.openxmlformats-officedocument.drawing+xml",
            b"<wsDr/>".to_vec(),
        ),
        (
            "docProps/core.xml",
            "application/vnd.openxmlformats-package.core-properties+xml",
            b"<coreProperties/>".to_vec(),
        ),
    ] {
        book.parts.push(OpaquePart {
            path: path.into(),
            content_type: Some(kind.into()),
            data,
        });
    }

    let back = cycle(&book);
    assert_same(&book, &back);
    let drawing = back
        .parts
        .iter()
        .find(|p| p.path == "xl/drawings/drawing1.xml")
        .expect("the drawing part comes back");
    assert_eq!(drawing.data, b"<wsDr/>", "carried bytes are not touched");
}

#[test]
fn the_mac_base_date_is_not_quietly_turned_into_the_windows_one() {
    use excelerate::shared::date::Epoch;

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Даты").unwrap();
    // 1 January 2025 counted from 1904; under the 1900 epoch the same serial
    // is 1462 days earlier.
    sheet.set(at("A1"), 44_196.0);
    book.add_sheet(sheet).unwrap();
    book.epoch = Epoch::Mac1904;
    book.workbook_properties
        .push(("codeName".into(), "ЭтаКнига".into()));
    book.calculation_properties
        .push(("calcMode".into(), "manual".into()));
    book.calculation_properties
        .push(("calcId".into(), "144525".into()));

    let back = cycle(&book);
    assert_same(&book, &back);
    assert_eq!(back.epoch, Epoch::Mac1904);
}

#[test]
fn formatting_inside_a_cell_survives() {
    use excelerate::model::{CellValue, TextRun};
    use excelerate::style::{Color, DiffFont};

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Текст").unwrap();
    let rich = CellValue::RichText(vec![
        TextRun {
            text: "Важно".into(),
            font: Some(DiffFont {
                bold: Some(true),
                color: Some(Color::Argb(0xFFFF_0000)),
                ..DiffFont::default()
            }),
        },
        TextRun {
            text: ": остальное обычным".into(),
            font: None,
        },
    ]);
    sheet.entry(at("A1")).value = rich.clone();
    // The same runs in another cell must land on one entry of the pool.
    sheet.entry(at("A2")).value = rich.clone();
    sheet.set(at("A3"), "просто текст");
    book.add_sheet(sheet).unwrap();

    let back = cycle(&book);
    assert_same(&book, &back);
    let sheet = back.sheet(0).unwrap();
    assert_eq!(sheet.get(at("A1")).unwrap().value, rich);
    assert_eq!(
        sheet.get(at("A1")).unwrap().value.plain_text().unwrap(),
        "Важно: остальное обычным"
    );
    assert!(
        matches!(sheet.get(at("A3")).unwrap().value, CellValue::Text(_)),
        "text that does not change part way through stays plain"
    );
}

/// The same invariant for CSV, as far as the format can carry it: a CSV holds
/// values and nothing else, so only the values have to come back.
#[test]
fn values_survive_a_csv_cycle() {
    let mut ws = Worksheet::new("Data").unwrap();
    ws.set(at("A1"), "plain");
    ws.set(at("B1"), "has,comma");
    ws.set(at("C1"), "say \"hi\"");
    ws.set(at("D1"), "two\nlines");
    ws.set(at("A2"), 1234.5);
    ws.set(at("B2"), -0.25);
    ws.set(at("C2"), true);
    ws.entry(at("D2")).value = CellValue::Error(excelerate::error::CellError::Div0);
    // A part number is not a number, and has to come back as text.
    ws.set(at("A3"), "007");
    ws.set(at("B3"), "привет");
    let mut before = Spreadsheet::empty();
    before.add_sheet(ws).unwrap();

    let mut buf = Vec::new();
    excelerate::writer::write_csv_to(&before, 0, &mut buf, ',').expect("csv writes");
    let text = excelerate::reader::csv::decode(&buf);
    let after = excelerate::reader::read_csv_str(&text, &excelerate::reader::CsvOptions::default());

    let (a, b) = (&before.sheets()[0], &after.sheets()[0]);
    for (cell, before) in a.iter() {
        assert_eq!(
            b.get(cell).map(|c| &c.value),
            Some(&before.value),
            "cell {cell}"
        );
    }
    assert_eq!(a.len(), b.len(), "no cells invented");
}

/// The same for HTML, which carries as much as a page can: the values as they
/// are shown, the merges, and the link on a cell. The number formats are what
/// produced the text in the first place, so they do not come back - a page
/// shows `12.50`, not the rule that made it.
#[test]
fn values_and_merges_survive_an_html_cycle() {
    let mut ws = Worksheet::new("Data").unwrap();
    ws.set(at("A1"), "plain");
    ws.set(at("B1"), "a & b < c");
    ws.set(at("C1"), "two\nlines");
    ws.set(at("A2"), 1234.5);
    ws.set(at("B2"), -0.25);
    ws.set(at("C2"), true);
    ws.entry(at("D2")).value = CellValue::Error(excelerate::error::CellError::Div0);
    ws.set(at("A3"), "007");
    ws.set(at("B3"), "привет");
    ws.merges.push(Range::new(at("A4"), at("C5")));
    ws.set(at("A4"), "spans");
    let mut before = Spreadsheet::empty();
    before.add_sheet(ws).unwrap();

    let mut buf = Vec::new();
    excelerate::writer::write_html_to(
        &before,
        &mut buf,
        &excelerate::writer::HtmlOptions::default(),
    )
    .expect("html writes");
    let after = excelerate::reader::read_html_str(&String::from_utf8(buf).unwrap());

    let (a, b) = (&before.sheets()[0], &after.sheets()[0]);
    for (cell, before) in a.iter() {
        assert_eq!(
            b.get(cell).map(|c| &c.value),
            Some(&before.value),
            "cell {cell}"
        );
    }
    assert_eq!(a.merges, b.merges, "merges");
}

/// The same invariant for xls, as far as BIFF8 and the two halves of the port
/// carry it: values, merges and the sizes of rows and columns come back, and a
/// formula comes back as its result, which is all the writer puts there.
#[test]
fn values_and_sizes_survive_an_xls_cycle() {
    use excelerate::style::NumberFormat;

    let mut ws = Worksheet::new("Values").unwrap();
    ws.set(at("A1"), "plain");
    ws.set(at("B1"), "привет");
    ws.set(at("C1"), "x".repeat(9000)); // longer than one record holds
    ws.set(at("A2"), 1234.5);
    ws.set(at("B2"), -0.25);
    ws.set(at("C2"), true);
    ws.entry(at("D2")).value = CellValue::Error(excelerate::error::CellError::Div0);
    ws.entry(at("A3")).value = CellValue::Formula {
        formula: "A2*2".to_owned(),
        cached: Some(Box::new(CellValue::Number(2469.0))),
    };
    ws.merges.push(Range::new(at("A5"), at("C6")));
    let mut second = Worksheet::new("Second").unwrap();
    second.set(at("A1"), 7.0);

    let mut before = Spreadsheet::empty();
    before.add_sheet(ws).unwrap();
    before.add_sheet(second).unwrap();
    let id = before.styles.intern(Style {
        number_format: NumberFormat::Custom("0.00".to_owned()),
        ..Style::default()
    });
    before.sheet_mut(0).unwrap().entry(at("A2")).style = id;
    {
        let sheet = before.sheet_mut(0).unwrap();
        let mut run = excelerate::model::ColumnRun::new(
            excelerate::Col::from_one_based(1).unwrap(),
            excelerate::Col::from_one_based(1).unwrap(),
        );
        run.width = Some(20.0);
        run.custom_width = true;
        sheet.columns.push(run);
        let row = sheet
            .rows
            .entry(excelerate::Row::from_one_based(1).unwrap())
            .or_default();
        row.height = Some(30.0);
        row.custom_height = true;
    }

    let mut buf = Vec::new();
    excelerate::writer::write_xls_to(&before, &mut buf).expect("xls writes");
    let after = excelerate::reader::read_xls_from(&buf).expect("what we wrote reads back");

    assert_eq!(after.sheets().len(), 2);
    assert_eq!(after.sheets()[0].title(), "Values");
    assert_eq!(after.sheets()[1].title(), "Second");
    let (a, b) = (&before.sheets()[0], &after.sheets()[0]);
    for (cell, before) in a.iter() {
        // A formula comes back as itself, cached result included.
        assert_eq!(
            b.get(cell).map(|c| &c.value),
            Some(&before.value),
            "cell {cell}"
        );
    }
    assert_eq!(a.merges, b.merges, "merges");
    assert_eq!(
        b.column_width(excelerate::Col::from_one_based(1).unwrap()),
        Some(20.0)
    );
    assert_eq!(
        b.row_height(excelerate::Row::from_one_based(1).unwrap()),
        Some(30.0)
    );
    let style = b
        .get(at("A2"))
        .map(|c| c.style)
        .and_then(|id| after.styles.get(id))
        .expect("A2 keeps a style");
    assert_eq!(style.number_format, NumberFormat::Custom("0.00".to_owned()));
}

/// Notes are modelled now, so they come back as notes rather than as bytes.
#[test]
fn notes_on_cells_survive_the_cycle() {
    use excelerate::model::{Comment, TextRun};
    use excelerate::style::DiffFont;

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("С примечаниями").unwrap();
    sheet.set(at("B2"), 1.0);
    sheet.comments.insert(
        at("B2"),
        Comment {
            author: "Юрий".into(),
            text: vec![
                TextRun {
                    text: "Юрий:".into(),
                    font: Some(DiffFont {
                        bold: Some(true),
                        ..DiffFont::default()
                    }),
                },
                TextRun {
                    text: "\nпроверить итог".into(),
                    font: None,
                },
            ],
        },
    );
    // A note on a cell that holds nothing is still a note.
    sheet.comments.insert(
        at("D4"),
        Comment {
            author: String::new(),
            text: vec![TextRun {
                text: "без автора".into(),
                font: None,
            }],
        },
    );
    book.add_sheet(sheet).unwrap();

    let back = cycle(&book);
    assert_same(&book, &back);
    let sheet = back.sheet(0).unwrap();
    assert_eq!(sheet.comments.len(), 2);
    let note = &sheet.comments[&at("B2")];
    assert_eq!(note.author, "Юрий");
    assert_eq!(note.plain_text(), "Юрий:\nпроверить итог");
    assert_eq!(note.text[0].font.as_ref().unwrap().bold, Some(true));
    assert_eq!(sheet.comments[&at("D4")].author, "");
}

/// The same for the workbook and the stylesheet, where the slicer caches and
/// the slicer styles live.
#[test]
fn the_workbook_and_style_extension_lists_travel_too() {
    let styles = concat!(
        r#"<extLst><ext uri="{EB79DEF2-80B8-43e5-95BD-54CBDDF9020C}" "#,
        r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
        r#"<x14:slicerStyles defaultSlicerStyle="SlicerStyleLight1"/></ext></extLst>"#,
    );
    let workbook = r#"<extLst><ext uri="{B58B0392-4F1F-4190-BB64-5DF3571DCE5F}"/></extLst>"#;

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Лист1").unwrap();
    sheet.set(at("A1"), 1.0);
    book.add_sheet(sheet).unwrap();
    book.style_extensions = Some(styles.to_owned());
    book.workbook_extensions = Some(workbook.to_owned());

    let back = cycle(&book);
    assert_same(&book, &back);
    assert_eq!(back.style_extensions.as_deref(), Some(styles));
    assert_eq!(back.workbook_extensions.as_deref(), Some(workbook));
}

/// Everything newer than the 2006 schema hangs off `<extLst>`: sparklines, the
/// conditional formats that needed more than the original format could say.
/// None of it is modelled, so it has to travel whole or be lost.
#[test]
fn the_sheet_extension_list_travels_whole() {
    let sparklines = concat!(
        r#"<extLst><ext uri="{05C60535-1F16-4fd2-B633-F4F36F0B64E0}" "#,
        r#"xmlns:x14="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main">"#,
        r#"<x14:sparklineGroups><x14:sparklineGroup type="column">"#,
        r#"<x14:sparklines><x14:sparkline><xm:f>Лист1!B1:E1</xm:f><xm:sqref>F1</xm:sqref>"#,
        r#"</x14:sparkline></x14:sparklines></x14:sparklineGroup></x14:sparklineGroups>"#,
        "</ext></extLst>",
    );

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Лист1").unwrap();
    sheet.set(at("B1"), 1.0);
    sheet.extensions = Some(sparklines.to_owned());
    book.add_sheet(sheet).unwrap();

    let back = cycle(&book);
    assert_same(&book, &back);
    assert_eq!(
        back.sheet(0).unwrap().extensions.as_deref(),
        Some(sparklines),
        "the extension list comes back byte for byte"
    );
}

/// A font is more than a name and a size: the character set decides which
/// glyphs a reader falls back to, and the scheme ties the font to the theme,
/// so a font written with `scheme="minor"` follows a theme change and one
/// without it does not.
#[test]
fn a_font_keeps_its_charset_and_its_scheme() {
    use excelerate::model::TextRun;
    use excelerate::style::{DiffFont, Font, FontScheme, Script, Style};

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Шрифты").unwrap();
    let style = book.styles.intern(Style {
        font: Font {
            name: "Calibri".into(),
            charset: Some(204),
            scheme: Some(FontScheme::Minor),
            ..Font::default()
        },
        ..Style::default()
    });
    sheet.set(at("A1"), "кириллица");
    sheet.entry(at("A1")).style = style;
    // The same on one run of a rich string, where `<rPr>` carries it.
    sheet.set(
        at("A2"),
        CellValue::RichText(vec![
            TextRun {
                text: "H".into(),
                font: None,
            },
            TextRun {
                text: "2".into(),
                font: Some(DiffFont {
                    script: Some(Script::Subscript),
                    charset: Some(204),
                    scheme: Some(FontScheme::Minor),
                    ..DiffFont::default()
                }),
            },
            TextRun {
                text: "O".into(),
                font: None,
            },
        ]),
    );
    book.add_sheet(sheet).unwrap();

    let back = cycle(&book);
    assert_same(&book, &back);
    let font = &back
        .styles
        .get(back.sheet(0).unwrap().get(at("A1")).unwrap().style)
        .unwrap()
        .font;
    assert_eq!(font.charset, Some(204));
    assert_eq!(font.scheme, Some(FontScheme::Minor));
    let CellValue::RichText(runs) = &back.sheet(0).unwrap().get(at("A2")).unwrap().value else {
        panic!("A2 is not rich text")
    };
    let run = runs[1].font.as_ref().unwrap();
    assert_eq!(run.script, Some(Script::Subscript));
    assert_eq!(run.charset, Some(204));
    assert_eq!(run.scheme, Some(FontScheme::Minor));
}

/// A workbook with a VBA project is macro-enabled, and its main part has to
/// say so: Excel refuses an `.xlsm` whose workbook claims to be plain.
#[test]
fn a_workbook_with_macros_is_written_as_macro_enabled() {
    use excelerate::model::{Attachment, OpaquePart};
    let content_types = |book: &Spreadsheet| {
        let mut bytes = Vec::new();
        write_xlsx_to(book, Cursor::new(&mut bytes)).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("[Content_Types].xml").unwrap(), &mut text)
            .unwrap();
        text
    };
    let mut book = Spreadsheet::new();
    assert!(content_types(&book).contains("spreadsheetml.sheet.main+xml"));
    book.parts.push(OpaquePart {
        path: "xl/vbaProject.bin".to_owned(),
        content_type: Some("application/vnd.ms-office.vbaProject".to_owned()),
        data: vec![0xD0, 0xCF, 0x11, 0xE0],
    });
    book.attachments.push(Attachment {
        kind: "http://schemas.microsoft.com/office/2006/relationships/vbaProject".to_owned(),
        target: "xl/vbaProject.bin".to_owned(),
    });
    let text = content_types(&book);
    assert!(
        text.contains(r#"PartName="/xl/workbook.xml" ContentType="application/vnd.ms-excel.sheet.macroEnabled.main+xml""#),
        "{text}"
    );
}
