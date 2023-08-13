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
