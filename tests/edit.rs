//! Inserting and removing rows and columns, and what follows the cells.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::{CellRef, Col, Range, Row};
use excelerate::edit::{insert_columns, insert_rows, remove_columns, remove_rows};
use excelerate::model::{CellValue, DefinedName, Hyperlink, LinkTarget, Spreadsheet, Worksheet};

fn at(address: &str) -> CellRef {
    CellRef::parse(address).unwrap()
}

fn range(address: &str) -> Range {
    Range::parse(address).unwrap()
}

fn formula(text: &str) -> CellValue {
    CellValue::Formula {
        formula: text.to_owned(),
        cached: None,
    }
}

fn row(one_based: u32) -> Row {
    Row::from_one_based(u64::from(one_based)).unwrap()
}

fn col(letters: &str) -> Col {
    Col::from_letters(letters).unwrap()
}

/// The formula text of a cell.
fn text(book: &Spreadsheet, sheet: usize, address: &str) -> String {
    match &book.sheet(sheet).unwrap().get(at(address)).unwrap().value {
        CellValue::Formula { formula, .. } => formula.clone(),
        other => panic!("{address} is {other:?}"),
    }
}

/// The number in a cell.
fn number(book: &Spreadsheet, sheet: usize, address: &str) -> Option<f64> {
    match &book.sheet(sheet)?.get(at(address))?.value {
        CellValue::Number(n) => Some(*n),
        other => panic!("{address} is {other:?}"),
    }
}

/// `A1=1`, `A2=2`, `A3=3` with `B1=SUM(A1:A3)` and a second sheet pointing in.
fn book() -> Spreadsheet {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Data").unwrap();
    for (i, address) in ["A1", "A2", "A3"].iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        sheet.set(at(address), (i + 1) as f64);
    }
    sheet.set(at("B1"), formula("SUM(A1:A3)"));
    sheet.set(at("B2"), formula("A3*2"));
    book.add_sheet(sheet).unwrap();

    let mut other = Worksheet::new("Report").unwrap();
    other.set(at("A1"), formula("Data!A3+1"));
    other.set(at("A2"), formula("SUM(Data!A1:A3)"));
    book.add_sheet(other).unwrap();
    book
}

#[test]
fn inserting_rows_moves_cells_and_the_formulas_that_read_them() {
    let mut book = book();
    insert_rows(&mut book, 0, row(2), 2).unwrap();

    // A1 stayed, A2 and A3 moved down by two.
    assert_eq!(number(&book, 0, "A1"), Some(1.0));
    assert_eq!(number(&book, 0, "A2"), None, "the inserted row is empty");
    assert_eq!(number(&book, 0, "A4"), Some(2.0));
    assert_eq!(number(&book, 0, "A5"), Some(3.0));

    // The range grows around the insertion, the lone reference follows. B2
    // held a formula and is itself below the insertion, so it moved to B4.
    assert_eq!(text(&book, 0, "B1"), "SUM(A1:A5)");
    assert_eq!(text(&book, 0, "B4"), "A5*2");
    // A formula on another sheet points at the edited one and moves too.
    assert_eq!(text(&book, 1, "A1"), "Data!A5+1");
    assert_eq!(text(&book, 1, "A2"), "SUM(Data!A1:A5)");
}

#[test]
fn removing_rows_narrows_what_is_left_and_refs_what_is_gone() {
    let mut book = book();
    remove_rows(&mut book, 0, row(3), 1).unwrap();

    assert_eq!(number(&book, 0, "A3"), None, "the third row is gone");
    assert_eq!(text(&book, 0, "B1"), "SUM(A1:A2)", "the range narrows");
    // The cell the formula read is gone outright, so the reference is broken.
    // The sheet qualifier survives it: Excel writes `Data!#REF!`, not `#REF!`.
    assert_eq!(text(&book, 0, "B2"), "#REF!*2");
    assert_eq!(text(&book, 1, "A1"), "Data!#REF!+1");
    assert_eq!(text(&book, 1, "A2"), "SUM(Data!A1:A2)");
}

#[test]
fn a_range_the_removal_swallows_whole_is_broken() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    // Sits below the removal, so the formula itself survives it.
    sheet.set(at("D20"), formula("SUM(A2:A4)"));
    book.add_sheet(sheet).unwrap();

    remove_rows(&mut book, 0, row(1), 9).unwrap();
    assert_eq!(text(&book, 0, "D11"), "SUM(#REF!)");
}

#[test]
fn an_absolute_reference_moves_like_any_other() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(at("C1"), formula("$A$5+$A5+A$5"));
    book.add_sheet(sheet).unwrap();

    // `$` says the reference does not shift when the *formula* is copied; a
    // row inserted above the cell it names moves the cell itself.
    insert_rows(&mut book, 0, row(1), 1).unwrap();
    assert_eq!(text(&book, 0, "C2"), "$A$6+$A6+A$6");
}

#[test]
fn text_and_names_that_merely_look_like_references_are_left_alone() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(at("D1"), formula("IF(A5>0,\"A5 is big\",LOG10(A5))"));
    book.add_sheet(sheet).unwrap();

    insert_rows(&mut book, 0, row(1), 1).unwrap();
    assert_eq!(
        text(&book, 0, "D2"),
        "IF(A6>0,\"A5 is big\",LOG10(A6))",
        "the string keeps its text and LOG10 stays a function"
    );
}

#[test]
fn columns_move_the_same_way() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(at("A1"), 1.0);
    sheet.set(at("C1"), 3.0);
    sheet.set(at("E1"), formula("A1+C1"));
    book.add_sheet(sheet).unwrap();

    insert_columns(&mut book, 0, col("B"), 1).unwrap();
    assert_eq!(number(&book, 0, "A1"), Some(1.0));
    assert_eq!(number(&book, 0, "D1"), Some(3.0));
    assert_eq!(text(&book, 0, "F1"), "A1+D1");

    remove_columns(&mut book, 0, col("A"), 1).unwrap();
    assert_eq!(number(&book, 0, "C1"), Some(3.0));
    assert_eq!(text(&book, 0, "E1"), "#REF!+C1");
}

#[test]
fn merges_links_and_the_filter_follow_the_grid() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.merges.push(range("A2:B3"));
    sheet.merges.push(range("A8:B9"));
    sheet.hyperlinks.push(Hyperlink {
        range: range("A8:A8"),
        target: LinkTarget::Outside("https://example.com".into()),
        display: None,
        tooltip: None,
    });
    sheet.rows.entry(row(8)).or_default().height = Some(40.0);
    book.add_sheet(sheet).unwrap();

    insert_rows(&mut book, 0, row(1), 2).unwrap();
    let sheet = book.sheet(0).unwrap();
    assert_eq!(sheet.merges, vec![range("A4:B5"), range("A10:B11")]);
    assert_eq!(sheet.hyperlinks[0].range, range("A10:A10"));
    assert_eq!(sheet.row_height(row(10)), Some(40.0));

    // A removal that covers a merge takes it away entirely.
    remove_rows(&mut book, 0, row(4), 2).unwrap();
    let sheet = book.sheet(0).unwrap();
    assert_eq!(sheet.merges, vec![range("A8:B9")]);
}

#[test]
fn defined_names_are_rewritten_too() {
    let mut book = book();
    book.defined_names.push(DefinedName {
        name: "Totals".into(),
        sheet: None,
        formula: "Data!$A$1:$A$3".into(),
        hidden: false,
    });

    insert_rows(&mut book, 0, row(2), 1).unwrap();
    assert_eq!(book.defined_names[0].formula, "Data!$A$1:$A$4");

    remove_rows(&mut book, 0, row(1), 8).unwrap();
    assert_eq!(book.defined_names[0].formula, "Data!#REF!");
}

/// A drawing is carried, not modelled, so its anchor lives in the bytes of its
/// part. The edit has to reach in there: an object left on the row it named
/// while the data moved is an error only a person looking at the sheet can
/// see.
#[test]
fn a_drawing_moves_with_the_rows_under_it() {
    use excelerate::model::{Attachment, Comment, OpaquePart, TextRun};

    let drawing = concat!(
        r#"<xdr:wsDr><xdr:twoCellAnchor>"#,
        r#"<xdr:from><xdr:col>0</xdr:col><xdr:row>4</xdr:row></xdr:from>"#,
        r#"<xdr:to><xdr:col>2</xdr:col><xdr:row>8</xdr:row></xdr:to>"#,
        r#"</xdr:twoCellAnchor></xdr:wsDr>"#,
    );
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Пикчи").unwrap();
    sheet.set(at("A9"), 1.0);
    sheet.comments.insert(
        at("B6"),
        Comment {
            author: "Кто-то".into(),
            text: vec![TextRun {
                text: "заметка".into(),
                font: None,
            }],
            ..Comment::default()
        },
    );
    sheet.attachments.push(Attachment {
        kind: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing".into(),
        target: "xl/drawings/drawing1.xml".into(),
    });
    book.add_sheet(sheet).unwrap();
    book.parts.push(OpaquePart {
        path: "xl/drawings/drawing1.xml".into(),
        content_type: None,
        data: drawing.as_bytes().to_vec(),
    });

    insert_rows(&mut book, 0, row(1), 3).unwrap();

    let moved = String::from_utf8(book.parts[0].data.clone()).unwrap();
    assert!(moved.contains("<xdr:row>7</xdr:row>"), "{moved}");
    assert!(moved.contains("<xdr:row>11</xdr:row>"), "{moved}");
    // The columns were not the axis edited, so they stayed.
    assert!(moved.contains("<xdr:col>0</xdr:col>"), "{moved}");
    // The note moved with its cell, as the cell itself did.
    assert!(book.sheet(0).unwrap().comments.contains_key(&at("B9")));
    assert!(book.sheet(0).unwrap().get(at("A12")).is_some());
}

#[test]
fn inserted_lines_take_the_formatting_they_are_told_to() {
    use excelerate::edit::{CopyOrigin, insert_columns_with, insert_rows_with};
    use excelerate::model::RowProperties;
    use excelerate::style::Style;

    let mut book = Spreadsheet::new();
    let bold = book.styles.intern(Style {
        font: excelerate::style::Font {
            bold: true,
            ..Default::default()
        },
        ..Default::default()
    });
    let sheet = book.sheet_mut(0).unwrap();
    sheet.entry(at("A1")).style = bold;
    sheet.rows.insert(
        row(1),
        RowProperties {
            height: Some(30.0),
            custom_height: true,
            ..Default::default()
        },
    );
    sheet.set(at("A2"), CellValue::Number(1.0));

    insert_rows_with(&mut book, 0, row(2), 2, CopyOrigin::Before).unwrap();
    let sheet = book.sheet(0).unwrap();
    for r in ["A2", "A3"] {
        assert_eq!(sheet.get(at(r)).unwrap().style, bold, "{r}");
    }
    assert_eq!(sheet.rows[&row(3)].height, Some(30.0));
    assert_eq!(sheet.get(at("A4")).unwrap().value, CellValue::Number(1.0));

    insert_rows_with(&mut book, 0, row(1), 1, CopyOrigin::After).unwrap();
    assert_eq!(book.sheet(0).unwrap().get(at("A1")).unwrap().style, bold);

    insert_columns_with(&mut book, 0, col("A"), 1, CopyOrigin::After).unwrap();
    assert_eq!(book.sheet(0).unwrap().get(at("A1")).unwrap().style, bold);
    insert_columns_with(&mut book, 0, col("A"), 1, CopyOrigin::Blank).unwrap();
    assert!(book.sheet(0).unwrap().get(at("A1")).is_none());
}

#[test]
fn a_recorded_sort_follows_its_rows() {
    use excelerate::model::{AutoFilter, SortCondition, SortState};
    let range = |s: &str| Range::parse(s).unwrap();
    let sort = |r: &str, key: &str| {
        let mut sort = SortState::new(range(r));
        sort.conditions.push(SortCondition::new(range(key)));
        sort
    };
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    let mut filter = AutoFilter::new(range("A1:B5"));
    filter.sort_state = Some(sort("A2:B5", "B2:B5"));
    sheet.auto_filter = Some(filter);
    sheet.sort_state = Some(sort("D2:E5", "D2:D5"));
    book.add_sheet(sheet).unwrap();

    insert_rows(&mut book, 0, row(1), 2).unwrap();
    let sheet = &book.sheets()[0];
    let inner = sheet
        .auto_filter
        .as_ref()
        .unwrap()
        .sort_state
        .as_ref()
        .unwrap();
    assert_eq!(inner.range, range("A4:B7"));
    assert_eq!(inner.conditions[0].range, range("B4:B7"));
    assert_eq!(sheet.sort_state.as_ref().unwrap().range, range("D4:E7"));

    // Removing the sorted rows removes the record of sorting them.
    remove_rows(&mut book, 0, row(4), 4).unwrap();
    assert_eq!(book.sheets()[0].sort_state, None);
}

#[test]
fn a_sparkline_follows_its_cell_and_its_data() {
    use excelerate::edit::rename_sheet;
    use excelerate::model::sparkline::{Sparkline, SparklineGroup, SparklineKind};
    let mut book = Spreadsheet::empty();
    book.add_sheet(Worksheet::new("Data").unwrap()).unwrap();
    let mut shown = Worksheet::new("Shown").unwrap();
    let mut group = SparklineGroup::new(SparklineKind::Line);
    for (data, cell) in [("Data!B2:B9", "A2"), ("Data!C2:C9", "A5")] {
        group.sparklines.push(Sparkline {
            data: Some(data.into()),
            location: at(cell),
        });
    }
    shown.sparklines.push(group);
    book.add_sheet(shown).unwrap();

    // Rows on the data sheet move what the sparklines read, not where they
    // sit; rows on their own sheet move where they sit.
    insert_rows(&mut book, 0, row(1), 1).unwrap();
    remove_rows(&mut book, 1, row(5), 1).unwrap();
    rename_sheet(&mut book, 0, "Source").unwrap();
    let lines = &book.sheets()[1].sparklines[0].sparklines;
    assert_eq!(lines.len(), 1, "the one drawn in a removed row goes");
    assert_eq!(lines[0].data.as_deref(), Some("Source!B3:B10"));
    assert_eq!(lines[0].location, at("A2"));
}

#[test]
fn a_range_on_another_sheet_ignores_an_edit_to_this_one() {
    let mut book = Spreadsheet::empty();
    book.add_sheet(Worksheet::new("Data").unwrap()).unwrap();
    let mut sheet = Worksheet::new("Shown").unwrap();
    // The second half of the range used to lose the qualifier of the first
    // and move with the rows of the sheet the formula sits on.
    sheet.set(at("Z1"), formula("Data!B3:B10+SUM(B3:B10)"));
    book.add_sheet(sheet).unwrap();
    remove_rows(&mut book, 1, row(5), 1).unwrap();
    assert_eq!(text(&book, 1, "Z1"), "Data!B3:B10+SUM(B3:B9)");
}

#[test]
fn the_newer_rules_in_the_extension_list_move_with_the_old_ones() {
    use excelerate::edit::rename_sheet;
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    // A data bar's x14 half, and a validation reading another sheet.
    sheet.extensions = Some(
        concat!(
            "<extLst><ext uri=\"{78C0D931-6437-407d-A8EE-F0AAD7539E65}\"><x14:conditionalFormattings>",
            "<x14:conditionalFormatting><x14:cfRule/><xm:sqref>B4:E15</xm:sqref>",
            "</x14:conditionalFormatting></x14:conditionalFormattings></ext>",
            "<ext uri=\"{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}\"><x14:dataValidations>",
            "<x14:dataValidation><x14:formula1><xm:f>Data!$A$1:$A$3</xm:f></x14:formula1>",
            "<xm:sqref>C2</xm:sqref></x14:dataValidation></x14:dataValidations></ext></extLst>",
        )
        .to_owned(),
    );
    book.add_sheet(sheet).unwrap();
    book.add_sheet(Worksheet::new("Data").unwrap()).unwrap();

    insert_rows(&mut book, 0, row(1), 2).unwrap();
    insert_rows(&mut book, 1, row(1), 1).unwrap();
    rename_sheet(&mut book, 1, "Lists").unwrap();
    let ext = book.sheets()[0].extensions.clone().unwrap();
    assert!(ext.contains("<xm:sqref>B6:E17</xm:sqref>"), "{ext}");
    assert!(ext.contains("<xm:sqref>C4</xm:sqref>"), "{ext}");
    assert!(ext.contains("<xm:f>Lists!$A$2:$A$4</xm:f>"), "{ext}");
}

/// Two sheets reading each other across a grid, with merges, rules,
/// validation, names, row heights and column widths: everything a batch has
/// to move the way the single edits do.
fn busy_book() -> Spreadsheet {
    use excelerate::model::{ConditionalFormat, DataValidation};

    let mut book = Spreadsheet::empty();
    let mut data = Worksheet::new("Data").unwrap();
    for r in 1..=30 {
        for (c, letters) in (0..).zip(["A", "B", "C", "D", "E", "F", "G", "H"]) {
            data.set(at(&format!("{letters}{r}")), f64::from(r * 10 + c));
        }
        data.set(at(&format!("J{r}")), formula(&format!("A{r}+H{r}")));
        data.rows.entry(row(r)).or_default().height = Some(f64::from(r));
    }
    data.set(at("K1"), formula("SUM(A1:A10)+SUM(A3:A12)+A9+SUM(A20:A24)"));
    data.set(
        at("K2"),
        formula("SUM(A1:H1)+SUM(C2:E2)+D3+SUM(B4:C4)+$G$5"),
    );
    data.set(at("K3"), formula("Report!A1+SUM(A28:A30)"));
    data.merges
        .extend([range("A5:B8"), range("C20:D24"), range("E2:F2")]);
    data.conditional_formats.push(ConditionalFormat {
        sqref: vec![range("A1:A30"), range("B9:C10"), range("D20:E24")],
        ..Default::default()
    });
    data.data_validations.push(DataValidation {
        sqref: vec![range("G3:H12"), range("B4:B4")],
        ..Default::default()
    });
    let bold = book.styles.intern(excelerate::style::Style {
        font: excelerate::style::Font {
            bold: true,
            ..Default::default()
        },
        ..Default::default()
    });
    for address in ["A5", "D5", "C1", "C12", "C29"] {
        data.entry(at(address)).style = bold;
    }
    data.column_entry(col("C")).width = Some(20.0);
    data.column_entry(col("F")).width = Some(30.0);
    book.add_sheet(data).unwrap();

    let mut report = Worksheet::new("Report").unwrap();
    report.set(
        at("A1"),
        formula("Data!A5+SUM(Data!A4:A9)+SUM(Data!A1:H30)"),
    );
    report.set(at("A2"), formula("Data!B9+Data!D2+SUM(Data!C1:E1)"));
    report.set(at("A3"), formula("SUM(Data!A20:H24)+A1"));
    book.add_sheet(report).unwrap();

    book.defined_names.push(DefinedName {
        name: "All".into(),
        sheet: None,
        formula: "Data!$A$1:$H$30".into(),
        hidden: false,
    });
    book.defined_names.push(DefinedName {
        name: "Gone".into(),
        sheet: Some(0),
        formula: "Data!$A$9".into(),
        hidden: false,
    });
    book
}

/// The batch against the same edits made one at a time from the far end.
fn same_as_one_by_one(
    batch: impl Fn(&mut Spreadsheet),
    one: impl Fn(&mut Spreadsheet, u32, u32),
    points: &[(u32, u32)],
) {
    let start = busy_book();
    let mut together = start.clone();
    batch(&mut together);
    let mut apart = start;
    let mut sorted = points.to_vec();
    sorted.sort_unstable();
    for &(at, count) in sorted.iter().rev() {
        one(&mut apart, at, count);
    }
    same_book(&together, &apart);
}

/// Compares two books field by field, naming the first line that differs.
///
/// Column runs are compared column by column: formatting inserted columns
/// splits the runs around them, and the batch splits them in another order.
fn same_book(together: &Spreadsheet, apart: &Spreadsheet) {
    let (mut together, mut apart) = (together.clone(), apart.clone());
    for index in 0..together.sheets().len() {
        let (l, r) = (
            together.sheet_mut(index).unwrap(),
            apart.sheet_mut(index).unwrap(),
        );
        for c in (0..40).filter_map(Col::new) {
            let each = |run: Option<&excelerate::model::ColumnRun>| {
                run.map(|run| (run.width, run.style, run.hidden))
            };
            assert_eq!(each(l.column_run(c)), each(r.column_run(c)), "column {c:?}");
        }
        l.columns.clear();
        r.columns.clear();
    }
    let (left, right) = (format!("{together:#?}"), format!("{apart:#?}"));
    for (n, (l, r)) in left.lines().zip(right.lines()).enumerate() {
        assert_eq!(
            l,
            r,
            "line {n} of the batch differs:\n{}",
            context(&left, n)
        );
    }
    assert_eq!(left.lines().count(), right.lines().count());
}

fn context(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[n.saturating_sub(15)..(n + 3).min(lines.len())].join("\n")
}

#[test]
fn a_batch_of_rows_is_the_same_as_one_row_at_a_time() {
    use excelerate::edit::{CopyOrigin, insert_rows_many, insert_rows_with, remove_rows_many};

    let r = |i: u32| Row::new(i).unwrap();
    // 0-based, out of order, two at the same row.
    let inserts = [(11, 2), (1, 1), (4, 3), (28, 1), (4, 1), (0, 2)];
    for origin in [CopyOrigin::Blank, CopyOrigin::Before, CopyOrigin::After] {
        same_as_one_by_one(
            |b| {
                let at: Vec<_> = inserts.iter().map(|&(i, n)| (r(i), n)).collect();
                insert_rows_many(b, 0, &at, origin).unwrap();
            },
            |b, i, n| insert_rows_with(b, 0, r(i), n, origin).unwrap(),
            &inserts,
        );
    }
    // Rows 3 and 5 out of A1:A10, A9 alone, A20:A24 whole, two blocks that
    // touch and the last rows of the sheet.
    let removals = [(8, 1), (2, 1), (4, 1), (19, 5), (13, 2), (15, 1), (27, 3)];
    same_as_one_by_one(
        |b| {
            let spans: Vec<_> = removals.iter().map(|&(i, n)| (r(i), n)).collect();
            remove_rows_many(b, 0, &spans).unwrap();
        },
        |b, i, n| remove_rows(b, 0, r(i), n).unwrap(),
        &removals,
    );

    let mut book = busy_book();
    let spans: Vec<_> = removals.iter().map(|&(i, n)| (r(i), n)).collect();
    remove_rows_many(&mut book, 0, &spans).unwrap();
    assert_eq!(
        text(&book, 0, "K1"),
        "SUM(A1:A7)+SUM(A3:A9)+#REF!+SUM(#REF!)",
        "a range cut in the middle narrows, one removed whole breaks"
    );
}

#[test]
fn a_batch_of_columns_is_the_same_as_one_column_at_a_time() {
    use excelerate::edit::{
        CopyOrigin, insert_columns_many, insert_columns_with, remove_columns_many,
    };

    let c = |i: u32| Col::new(i).unwrap();
    let inserts = [(5, 1), (1, 2), (2, 1), (5, 2), (0, 1)];
    for origin in [CopyOrigin::Blank, CopyOrigin::Before, CopyOrigin::After] {
        same_as_one_by_one(
            |b| {
                let at: Vec<_> = inserts.iter().map(|&(i, n)| (c(i), n)).collect();
                insert_columns_many(b, 0, &at, origin).unwrap();
            },
            |b, i, n| insert_columns_with(b, 0, c(i), n, origin).unwrap(),
            &inserts,
        );
    }
    let removals = [(3, 1), (1, 1), (5, 2), (2, 1)];
    same_as_one_by_one(
        |b| {
            let spans: Vec<_> = removals.iter().map(|&(i, n)| (c(i), n)).collect();
            remove_columns_many(b, 0, &spans).unwrap();
        },
        |b, i, n| remove_columns(b, 0, c(i), n).unwrap(),
        &removals,
    );
}

#[test]
fn overlapping_spans_remove_each_row_once() {
    use excelerate::edit::remove_rows_many;

    let r = |i: u32| Row::new(i).unwrap();
    // One book cloned: the style index is a hash map, printed in its order.
    let mut together = busy_book();
    let mut apart = together.clone();
    remove_rows_many(&mut together, 0, &[(r(6), 3), (r(4), 4), (r(5), 1)]).unwrap();
    remove_rows(&mut apart, 0, r(4), 5).unwrap();
    same_book(&together, &apart);
}

/// One pass however many places: 200 000 rows and 20 000 insertions took a
/// pass each before, hours in all. Timed, so release only:
/// `cargo test --release --test edit -- --ignored`.
#[test]
#[ignore = "timing; run in release"]
fn a_batch_costs_one_pass() {
    use excelerate::edit::{CopyOrigin, insert_rows_many, remove_rows_many};

    let rows = 200_000;
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Data").unwrap();
    for r in 0..rows {
        let at = |c| CellRef::new(Col::new(c).unwrap(), Row::new(r).unwrap());
        sheet.set(at(0), f64::from(r));
        sheet.set(at(1), formula(&format!("A{}*2", r + 1)));
    }
    book.add_sheet(sheet).unwrap();
    let points: Vec<(Row, u32)> = (1..=20_000)
        .map(|g| (Row::new(g * 10).unwrap(), 1))
        .collect();

    let started = std::time::Instant::now();
    insert_rows_many(&mut book, 0, &points, CopyOrigin::Blank).unwrap();
    // Each inserted row lands after the ten rows of its group.
    assert_eq!(text(&book, 0, "B12"), "A12*2");
    let spans: Vec<(Row, u32)> = (0..20_000)
        .map(|g| (Row::new(g * 11 + 10).unwrap(), 1))
        .collect();
    remove_rows_many(&mut book, 0, &spans).unwrap();
    let elapsed = started.elapsed();

    assert_eq!(text(&book, 0, &format!("B{rows}")), format!("A{rows}*2"));
    assert_eq!(book.sheet(0).unwrap().len(), 400_000);
    assert!(elapsed.as_secs() < 5, "{elapsed:?}");
}

/// The last row and column of a sheet, 0-based.
fn last_row() -> Row {
    Row::new(excelerate::coordinate::MAX_ROW - 1).unwrap()
}

fn last_col() -> Col {
    Col::new(excelerate::coordinate::MAX_COL - 1).unwrap()
}

/// Asserts that `edit` refuses and leaves the book as it was.
fn refused(book: &Spreadsheet, edit: impl FnOnce(&mut Spreadsheet) -> excelerate::Result<()>) {
    let mut after = book.clone();
    assert_eq!(edit(&mut after), Err(excelerate::Error::WouldPushOffSheet));
    assert_eq!(
        format!("{after:?}"),
        format!("{book:?}"),
        "the book changed"
    );
}

#[test]
fn an_insertion_that_would_push_content_off_the_sheet_is_refused() {
    use excelerate::edit::{CopyOrigin, insert_columns_many, insert_rows_many};

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(at("A1"), formula("A3+1"));
    sheet.set(CellRef::new(col("A"), last_row()), 1.0);
    sheet.set(CellRef::new(last_col(), row(2)), 2.0);
    book.add_sheet(sheet).unwrap();

    refused(&book, |b| insert_rows(b, 0, row(2), 1));
    refused(&book, |b| {
        insert_rows_many(b, 0, &[(row(5), 1), (last_row(), 1)], CopyOrigin::Blank)
    });
    refused(&book, |b| insert_columns(b, 0, col("A"), 1));
    refused(&book, |b| {
        insert_columns_many(b, 0, &[(col("B"), 2)], CopyOrigin::Before)
    });

    // A merged area is content too, even with no cell under it.
    let mut merged = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    let bottom = Row::new(last_row().index() - 1).unwrap();
    sheet.merges.push(Range::new(
        CellRef::new(col("A"), bottom),
        CellRef::new(col("B"), bottom),
    ));
    merged.add_sheet(sheet).unwrap();
    refused(&merged, |b| insert_rows(b, 0, row(1), 2));
    insert_rows(&mut merged, 0, row(1), 1).unwrap();
}

#[test]
fn an_insertion_that_pushes_off_only_blank_rows_goes_through() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    let near = Row::new(last_row().index() - 3).unwrap();
    sheet.set(CellRef::new(col("A"), near), 1.0);
    // Formatting at the edge is cut off without a word, as Excel does.
    sheet.rows.entry(last_row()).or_default().height = Some(40.0);
    sheet.column_entry(last_col()).width = Some(30.0);
    book.add_sheet(sheet).unwrap();

    insert_rows(&mut book, 0, row(1), 3).unwrap();
    let sheet = book.sheet(0).unwrap();
    assert_eq!(
        sheet.get(CellRef::new(col("A"), last_row())).unwrap().value,
        CellValue::Number(1.0)
    );
    assert!(!sheet.rows.contains_key(&last_row()));
    insert_columns(&mut book, 0, col("A"), 2).unwrap();
}

#[test]
fn inserted_cells_that_would_push_content_off_the_sheet_are_refused() {
    use excelerate::edit::{Axis, insert_cells};

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(CellRef::new(col("B"), last_row()), 1.0);
    let bottom = Row::new(last_row().index() - 1).unwrap();
    sheet.merges.push(Range::new(
        CellRef::new(col("D"), bottom),
        CellRef::new(col("D"), bottom),
    ));
    book.add_sheet(sheet).unwrap();

    refused(&book, |b| insert_cells(b, 0, range("B2:B3"), Axis::Rows));
    refused(&book, |b| insert_cells(b, 0, range("D2:D3"), Axis::Rows));
    // Column C has nothing at the edge.
    insert_cells(&mut book, 0, range("C2:C3"), Axis::Rows).unwrap();
}
