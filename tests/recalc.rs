//! Recalculation: the whole workbook, one sheet, or only what an edit reached.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::CellRef;
use excelerate::formula::eval::{recalculate, recalculate_cell, recalculate_from};
use excelerate::model::{Attachment, CellValue, OpaquePart, Spreadsheet, Worksheet};
use excelerate::progress::Options;
use excelerate::reader::xlsx::read_xlsx_from;
use excelerate::writer::xlsx::write_xlsx_to;
use std::io::Cursor;

/// `A1=2`, `A2=3`, `A3=A1+A2`, `A4=A3*10`, and `B1` reading nothing of theirs.
fn book() -> Spreadsheet {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("First").unwrap();
    sheet.set(at("A1"), 2.0);
    sheet.set(at("A2"), 3.0);
    sheet.set(at("A3"), formula("A1+A2"));
    sheet.set(at("A4"), formula("A3*10"));
    sheet.set(at("B1"), formula("7*6"));
    book.add_sheet(sheet).unwrap();

    let mut other = Worksheet::new("Second").unwrap();
    other.set(at("A1"), formula("First!A3+1"));
    book.add_sheet(other).unwrap();
    book
}

fn at(address: &str) -> CellRef {
    CellRef::parse(address).unwrap()
}

fn formula(text: &str) -> CellValue {
    CellValue::Formula {
        formula: text.to_owned(),
        cached: None,
    }
}

/// The cached value of a cell, as a number.
fn cached(book: &Spreadsheet, sheet: usize, address: &str) -> Option<f64> {
    match &book.sheet(sheet)?.get(at(address))?.value {
        CellValue::Formula { cached, .. } => match cached.as_deref()? {
            CellValue::Number(n) => Some(*n),
            other => panic!("{address} is {other:?}"),
        },
        CellValue::Number(n) => Some(*n),
        other => panic!("{address} is {other:?}"),
    }
}

#[test]
fn the_whole_workbook_or_one_sheet() {
    let mut book = book();
    assert_eq!(cached(&book, 0, "A3"), None);

    assert_eq!(recalculate(&mut book, Some(0), &Options::default()), 3);
    assert_eq!(cached(&book, 0, "A3"), Some(5.0));
    assert_eq!(cached(&book, 0, "A4"), Some(50.0));
    assert_eq!(cached(&book, 1, "A1"), None, "the other sheet is untouched");

    assert_eq!(recalculate(&mut book, None, &Options::default()), 4);
    assert_eq!(cached(&book, 1, "A1"), Some(6.0));
}

#[test]
fn an_edit_reaches_only_what_reads_it() {
    let mut book = book();
    recalculate(&mut book, None, &Options::default());

    book.sheet_mut(0).unwrap().set(at("A1"), 10.0);
    // A3 reads A1, A4 reads A3, and Second!A1 reads A3 across sheets. B1 reads
    // none of them and is not recomputed.
    assert_eq!(recalculate_from(&mut book, &[(0, at("A1"))]), 3);
    assert_eq!(cached(&book, 0, "A3"), Some(13.0));
    assert_eq!(cached(&book, 0, "A4"), Some(130.0));
    assert_eq!(cached(&book, 1, "A1"), Some(14.0));
    assert_eq!(cached(&book, 0, "B1"), Some(42.0));

    // An edit nothing reads costs nothing.
    book.sheet_mut(0).unwrap().set(at("Z99"), 1.0);
    assert_eq!(recalculate_from(&mut book, &[(0, at("Z99"))]), 0);
}

#[test]
fn a_range_counts_as_reading_every_cell_in_it() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("First").unwrap();
    for row in 1..=5 {
        sheet.set(at(&format!("A{row}")), f64::from(row));
    }
    sheet.set(at("C1"), formula("SUM(A1:A5)"));
    sheet.set(at("C2"), formula("SUM(B1:B5)"));
    book.add_sheet(sheet).unwrap();
    recalculate(&mut book, None, &Options::default());
    assert_eq!(cached(&book, 0, "C1"), Some(15.0));

    book.sheet_mut(0).unwrap().set(at("A3"), 30.0);
    assert_eq!(recalculate_from(&mut book, &[(0, at("A3"))]), 1);
    assert_eq!(cached(&book, 0, "C1"), Some(42.0));
    assert_eq!(cached(&book, 0, "C2"), Some(0.0), "reads another column");
}

#[test]
fn a_wide_sheet_recomputes_only_the_column_that_moved() {
    // A thousand independent formulas; an edit reaches the one that reads it.
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("First").unwrap();
    for row in 1..=1000 {
        sheet.set(at(&format!("A{row}")), f64::from(row));
        sheet.set(at(&format!("B{row}")), formula(&format!("A{row}*2")));
    }
    book.add_sheet(sheet).unwrap();

    assert_eq!(recalculate(&mut book, None, &Options::default()), 1000);
    assert_eq!(cached(&book, 0, "B7"), Some(14.0));

    book.sheet_mut(0).unwrap().set(at("A7"), 100.0);
    assert_eq!(recalculate_from(&mut book, &[(0, at("A7"))]), 1);
    assert_eq!(cached(&book, 0, "B7"), Some(200.0));
    assert_eq!(cached(&book, 0, "B8"), Some(16.0));
}

#[test]
fn one_index_serves_a_run_of_edits() {
    use excelerate::formula::eval::Dependencies;

    let mut book = book();
    let mut deps = Dependencies::of(&book);
    assert_eq!(deps.len(), 4);
    recalculate(&mut book, None, &Options::default());

    // A batch of edits is one pass, not one per cell.
    book.sheet_mut(0).unwrap().set(at("A1"), 10.0);
    book.sheet_mut(0).unwrap().set(at("A2"), 20.0);
    let touched = deps.recalculate_from(&mut book, &[(0, at("A1")), (0, at("A2"))]);
    assert_eq!(touched, 3);
    assert_eq!(cached(&book, 0, "A3"), Some(30.0));
    assert_eq!(cached(&book, 0, "A4"), Some(300.0));

    // A value edit leaves the index valid, so it is reused as it is.
    book.sheet_mut(0).unwrap().set(at("A1"), 1.0);
    assert_eq!(deps.recalculate_from(&mut book, &[(0, at("A1"))]), 3);
    assert_eq!(cached(&book, 0, "A3"), Some(21.0));

    // A formula edit does not: the index has to be told.
    book.sheet_mut(0).unwrap().set(at("B1"), formula("A1*100"));
    deps.note(&book, 0, at("B1"));
    assert_eq!(deps.len(), 4);
    assert_eq!(deps.recalculate_from(&mut book, &[(0, at("A1"))]), 4);
    assert_eq!(cached(&book, 0, "B1"), Some(100.0));

    // And a formula deleted leaves the index one shorter.
    book.sheet_mut(0).unwrap().set(at("B1"), 0.0);
    deps.note(&book, 0, at("B1"));
    assert_eq!(deps.len(), 3);
}

/// A workbook that links to a closed one recalculates from the cached values
/// its `externalLink` part holds, and those survive a round trip through the
/// package.
#[test]
fn a_link_to_another_workbook_recalculates_from_its_cache() {
    let link = br#"<?xml version="1.0"?>
<externalLink xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
 <externalBook r:id="rId1">
  <sheetNames><sheetName val="Prices"/></sheetNames>
  <sheetDataSet><sheetData sheetId="0">
   <row r="1"><cell r="A1"><v>41</v></cell><cell r="B1" t="str"><v>tea</v></cell></row>
   <row r="2"><cell r="A2"><v>1</v></cell></row>
  </sheetData></sheetDataSet>
 </externalBook>
</externalLink>"#;
    let rels = br#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Id="rId1" Target="file:///C:/work/prices.xlsx" TargetMode="External"
   Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLinkPath"/>
</Relationships>"#;

    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Report").unwrap();
    sheet.set(at("A1"), formula("[1]Prices!A1+1"));
    sheet.set(at("A2"), formula("SUM([1]Prices!A1:A2)"));
    sheet.set(at("A3"), formula("[1]Prices!B1"));
    sheet.set(at("A4"), formula("[1]Prices!Z9"));
    sheet.set(at("A5"), formula("[2]Prices!A1"));
    book.add_sheet(sheet).unwrap();
    book.attachments.push(Attachment {
        kind: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/externalLink"
            .into(),
        target: "xl/externalLinks/externalLink1.xml".into(),
    });
    for (path, data) in [
        ("xl/externalLinks/externalLink1.xml", &link[..]),
        ("xl/externalLinks/_rels/externalLink1.xml.rels", &rels[..]),
    ] {
        book.parts.push(OpaquePart {
            path: path.into(),
            content_type: None,
            data: data.to_vec(),
        });
    }

    let mut bytes = Vec::new();
    write_xlsx_to(&book, Cursor::new(&mut bytes)).unwrap();
    let mut book = read_xlsx_from(Cursor::new(&bytes)).unwrap();

    assert_eq!(book.external.len(), 1);
    assert_eq!(
        book.external[0].path.as_deref(),
        Some("file:///C:/work/prices.xlsx")
    );

    recalculate(&mut book, None, &Options::default());
    assert_eq!(cached(&book, 0, "A1"), Some(42.0));
    assert_eq!(cached(&book, 0, "A2"), Some(42.0));
    let text = |address: &str| match &book.sheet(0).unwrap().get(at(address)).unwrap().value {
        CellValue::Formula { cached, .. } => format!("{:?}", cached.as_deref()),
        other => panic!("{address} is {other:?}"),
    };
    assert!(text("A3").contains("tea"));
    // Nothing cached for that cell reads as empty, as Excel shows it.
    assert_eq!(cached(&book, 0, "A4"), Some(0.0));
    // A link that is not there at all is a broken reference, not a blank.
    assert!(text("A5").contains("Ref"));
}

/// A volatile function is not described by what it reads: `RAND` has no
/// inputs at all, and `INDIRECT` builds its reference while evaluating. Both
/// are recomputed by an edit that reaches neither.
#[test]
fn volatile_formulas_recompute_on_any_edit() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("First").unwrap();
    sheet.set(at("A1"), 2.0);
    sheet.set(at("B1"), 5.0);
    sheet.set(at("C1"), formula("INDIRECT(\"B1\")"));
    sheet.set(at("D1"), formula("C1*2"));
    sheet.set(at("E1"), formula("1+1"));
    book.add_sheet(sheet).unwrap();

    recalculate(&mut book, None, &Options::default());
    assert_eq!(cached(&book, 0, "C1"), Some(5.0));
    assert_eq!(cached(&book, 0, "D1"), Some(10.0));

    // The edit touches neither C1 nor B1, yet the volatile call and what
    // reads it are brought up to date all the same.
    book.sheet_mut(0).unwrap().set(at("B1"), 7.0);
    let computed = recalculate_from(&mut book, &[(0, at("A1"))]);
    assert_eq!(cached(&book, 0, "C1"), Some(7.0));
    assert_eq!(cached(&book, 0, "D1"), Some(14.0));
    // `E1` reads nothing and calls nothing volatile: it stays out of the pass.
    assert_eq!(computed, 2);
}

/// A chain of formulas is computed by recursion, so an untrusted workbook must
/// not be able to drive it off the stack: past the ceiling the cell is
/// `#VALUE!` and the pass finishes.
#[test]
fn a_long_chain_of_formulas_stops_at_the_ceiling() {
    use excelerate::coordinate::{Col, Row};

    let deep: u32 = 5_000;
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    let cell = |row: u32| CellRef::new(Col::new(0).unwrap(), Row::new(row - 1).unwrap());
    sheet.set(cell(deep + 1), CellValue::Number(1.0));
    for row in 1..=deep {
        sheet.set(cell(row), formula(&format!("A{}+1", row + 1)));
    }
    book.add_sheet(sheet).unwrap();

    // Computed in dependency order, so the chain costs no recursion at all
    // and every link of it adds up.
    assert_eq!(
        recalculate(&mut book, None, &Options::default()),
        deep as usize
    );
    assert_eq!(cached(&book, 0, "A1"), Some(f64::from(deep) + 1.0));
}

/// The same for one formula nested into itself: the parser refuses it rather
/// than recursing to the bottom of the stack.
#[test]
fn a_formula_nested_too_deeply_is_refused() {
    let deep = 100_000;
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    sheet.set(
        at("A1"),
        formula(&format!("{}1{}", "(".repeat(deep), ")".repeat(deep))),
    );
    book.add_sheet(sheet).unwrap();

    recalculate(&mut book, None, &Options::default());
    let CellValue::Formula { cached, .. } = &book.sheet(0).unwrap().get(at("A1")).unwrap().value
    else {
        panic!("not a formula")
    };
    // A formula that does not parse is `#NAME?`, as any other unparseable one.
    assert!(format!("{:?}", cached.as_deref()).contains("Name"));
}

/// One cell, and only that one: `recalculate_cell` computes the formula it is
/// pointed at, not the formulas reading it.
#[test]
fn one_cell_recalculates_by_itself() {
    let mut book = book();

    assert!(recalculate_cell(&mut book, 0, at("A3")));
    assert_eq!(cached(&book, 0, "A3"), Some(5.0));
    // `A4` reads `A3` and was left alone, which is what tells this apart from
    // `recalculate_from`.
    assert_eq!(cached(&book, 0, "A4"), None);
    // What the formula reads is computed on the way, but only in the engine's
    // own cache: `A1` is a number, not a formula, and nothing else was stored.
    assert_eq!(cached(&book, 1, "A1"), None);

    // A cell holding no formula is nothing to recompute, and neither is a
    // sheet that is not there.
    assert!(!recalculate_cell(&mut book, 0, at("A1")));
    assert!(!recalculate_cell(&mut book, 0, at("Z99")));
    assert!(!recalculate_cell(&mut book, 9, at("A3")));
}

/// An array formula entered over a column keeps the whole array in its first
/// cell and the rest of it as stored values below. Read through a range, each
/// cell is one value: the first cell must not bring the whole array along.
#[test]
fn an_array_formula_is_one_value_when_its_cell_is_read() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("First").unwrap();
    sheet.set(at("A1"), formula("TRANSPOSE({1,2,3})"));
    sheet.set(at("A2"), 2.0);
    sheet.set(at("A3"), 3.0);
    sheet.set(at("B1"), formula("COUNTA(A1:A3)"));
    sheet.set(at("B2"), formula("SUM(A1:A3)"));
    sheet.set(at("B3"), formula("A1*10"));
    book.add_sheet(sheet).unwrap();
    recalculate(&mut book, None, &Options::default());
    assert_eq!(cached(&book, 0, "B1"), Some(3.0));
    assert_eq!(cached(&book, 0, "B2"), Some(6.0));
    assert_eq!(cached(&book, 0, "B3"), Some(10.0));
}

/// A formula answering with a range shows the cell of that range in its own
/// row or column; an array formula keeps the range and shows its top left.
#[test]
fn implicit_intersection_picks_the_formula_row() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    for (i, n) in [10.0, 20.0, 30.0].into_iter().enumerate() {
        sheet.set(at(&format!("A{}", i + 1)), n);
        sheet.set(at(&format!("B{}", i + 1)), n + 1.0);
    }
    sheet.set(at("C2"), formula("A1:A3"));
    sheet.set(at("C9"), formula("A1:A3"));
    // Excel's own example: the second row of a 2D range, taken in column B.
    sheet.set(at("B7"), formula("INDEX(A1:B3,2,0)"));
    sheet.set(at("D3"), formula("A1:A3"));
    sheet
        .array_formulas
        .push(excelerate::coordinate::Range::parse("D3:D5").unwrap());
    book.add_sheet(sheet).unwrap();
    recalculate(&mut book, None, &Options::default());

    // Round trip keeps the array flag.
    let mut bytes = Cursor::new(Vec::new());
    write_xlsx_to(&book, &mut bytes).unwrap();
    let mut book = read_xlsx_from(Cursor::new(bytes.into_inner())).unwrap();
    recalculate(&mut book, None, &Options::default());

    assert_eq!(cached(&book, 0, "C2"), Some(20.0));
    assert!(matches!(
        book.sheet(0).unwrap().get(at("C9")).map(|c| &c.value),
        Some(CellValue::Formula { cached: Some(v), .. })
            if **v == CellValue::Error(excelerate::error::CellError::Value)
    ));
    assert_eq!(cached(&book, 0, "B7"), Some(21.0));
    assert_eq!(cached(&book, 0, "D3"), Some(10.0));
}

/// Inside a formula that is not an array formula a range met by an operator or
/// a value parameter is one cell too; an array parameter and an array formula
/// keep it whole.
#[test]
fn implicit_intersection_reaches_operators_and_parameters() {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("S").unwrap();
    for (i, n) in [-1.0, -2.0, -3.0].into_iter().enumerate() {
        sheet.set(at(&format!("A{}", i + 1)), n);
    }
    sheet.set(at("B2"), formula("A1:A3*10"));
    sheet.set(at("C2"), formula("ABS(A1:A3)"));
    sheet.set(at("D2"), formula("SUM(A1:A3*2)"));
    sheet.set(at("E2"), formula("SUMPRODUCT(A1:A3*2)"));
    sheet.set(at("F2"), formula("IF(A1:A3<-1,7,0)"));
    sheet.set(at("G2"), formula("Vals+1"));
    sheet.set(at("H9"), formula("A1:A3*10"));
    sheet.set(at("I1"), formula("SUM(A1:A3*2)"));
    sheet
        .array_formulas
        .push(excelerate::coordinate::Range::parse("I1").unwrap());
    book.add_sheet(sheet).unwrap();
    book.defined_names.push(excelerate::model::DefinedName {
        name: "Vals".to_owned(),
        sheet: None,
        formula: "S!$A$1:$A$3".to_owned(),
        hidden: false,
    });
    recalculate(&mut book, None, &Options::default());

    assert_eq!(cached(&book, 0, "B2"), Some(-20.0));
    assert_eq!(cached(&book, 0, "C2"), Some(2.0));
    assert_eq!(cached(&book, 0, "D2"), Some(-4.0));
    assert_eq!(cached(&book, 0, "E2"), Some(-12.0));
    assert_eq!(cached(&book, 0, "F2"), Some(7.0));
    assert_eq!(cached(&book, 0, "G2"), Some(-1.0));
    assert!(matches!(
        book.sheet(0).unwrap().get(at("H9")).map(|c| &c.value),
        Some(CellValue::Formula { cached: Some(v), .. })
            if **v == CellValue::Error(excelerate::error::CellError::Value)
    ));
    assert_eq!(cached(&book, 0, "I1"), Some(-12.0));
}
