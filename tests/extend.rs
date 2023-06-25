//! Two things a caller lends the engine: functions of their own, and somewhere
//! to report progress.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::coordinate::CellRef;
use excelerate::formula::eval::{Dependencies, recalculate, recalculate_from_with};
use excelerate::formula::{CustomFunctions, Engine, Origin, Value};
use excelerate::model::{CellValue, Spreadsheet, Worksheet};
use excelerate::progress::{Options, Progress, Stage};
use excelerate::reader::xlsx::{read_xlsx_from, read_xlsx_from_with};
use excelerate::writer::xlsx::write_xlsx_to_with;
use std::cell::RefCell;
use std::io::Cursor;

/// One progress report as these tests keep it: what was worked on, out of how
/// many, and how far along that puts the operation.
type Step = (String, Option<usize>, Option<f64>);

fn at(address: &str) -> CellRef {
    CellRef::parse(address).unwrap()
}

fn formula(text: &str) -> CellValue {
    CellValue::Formula {
        formula: text.to_owned(),
        cached: None,
    }
}

/// `A1=2`, `A2=3`, and three formulas calling a function this crate has never
/// heard of.
fn book() -> Spreadsheet {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Лист").unwrap();
    sheet.set(at("A1"), 2.0);
    sheet.set(at("A2"), 3.0);
    sheet.set(at("B1"), formula("МОЙИТОГ(A1:A2)"));
    sheet.set(at("B2"), formula("MYRATE(A1)*10"));
    sheet.set(at("B3"), formula("A1+A2"));
    book.add_sheet(sheet).unwrap();
    book
}

fn functions() -> CustomFunctions {
    let mut custom = CustomFunctions::new();
    // A range argument arrives as an array, the way it does inside a built-in.
    custom.register("МОЙИТОГ", |args| {
        let mut total = 0.0;
        for arg in args {
            let mut flat = Vec::new();
            arg.flatten(&mut flat);
            for value in flat {
                total += value.number().unwrap_or(0.0);
            }
        }
        Value::Number(total * 2.0)
    });
    custom.register("myrate", |args| match args {
        [one] => Value::Number(one.number().unwrap_or(0.0) + 0.5),
        _ => Value::Error(excelerate::CellError::Value),
    });
    custom
}

#[test]
fn a_registered_function_is_called_like_any_other() {
    let book = book();
    let custom = functions();
    let mut engine = Engine::with_functions(&book, &custom);
    let origin = Origin::new(0, at("Z1"));

    assert_eq!(engine.eval(origin, "МОЙИТОГ(A1:A2)"), Value::Number(10.0));
    // The name is matched ignoring case, as Excel matches function names.
    assert_eq!(engine.eval(origin, "MyRate(4)"), Value::Number(4.5));
    assert_eq!(engine.eval(origin, "MYRATE(A1)*10"), Value::Number(25.0));
    // Nested inside a built-in, and nesting one inside itself.
    assert_eq!(
        engine.eval(origin, "SUM(MYRATE(1),MYRATE(2))"),
        Value::Number(4.0)
    );
    assert_eq!(
        engine.eval(origin, "МОЙИТОГ(МОЙИТОГ(A1))"),
        Value::Number(8.0)
    );
}

#[test]
fn an_unregistered_name_is_still_unknown() {
    let book = book();
    let mut plain = Engine::new(&book);
    // Without the registration the workbook reads as Excel reads it with
    // macros disabled: the name means nothing.
    assert_eq!(
        plain.eval(Origin::new(0, at("Z1")), "МОЙИТОГ(A1:A2)"),
        Value::Error(excelerate::CellError::Name)
    );
}

#[test]
fn a_registered_function_cannot_shadow_a_builtin() {
    let book = book();
    let mut custom = CustomFunctions::new();
    custom.register("SUM", |_| Value::Number(-1.0));
    let mut engine = Engine::with_functions(&book, &custom);
    assert_eq!(
        engine.eval(Origin::new(0, at("Z1")), "SUM(A1:A2)"),
        Value::Number(5.0),
        "the built-in keeps its name"
    );
}

#[test]
fn recalculation_uses_the_registered_functions() {
    let mut book = book();
    let custom = functions();
    let options = Options::new().with_functions(&custom);

    assert_eq!(recalculate(&mut book, None, &options), 3);
    let cached = |book: &Spreadsheet, address: &str| match &book
        .sheet(0)
        .unwrap()
        .get(at(address))
        .unwrap()
        .value
    {
        CellValue::Formula { cached, .. } => cached.as_deref().cloned(),
        other => panic!("{address} is {other:?}"),
    };
    assert_eq!(cached(&book, "B1"), Some(CellValue::Number(10.0)));
    assert_eq!(cached(&book, "B2"), Some(CellValue::Number(25.0)));

    // And so does the incremental pass after an edit.
    book.sheet_mut(0).unwrap().set(at("A1"), 4.0);
    let computed = recalculate_from_with(&mut book, &[(0, at("A1"))], &options);
    assert!(computed >= 2, "computed {computed}");
    assert_eq!(cached(&book, "B1"), Some(CellValue::Number(14.0)));
    assert_eq!(cached(&book, "B2"), Some(CellValue::Number(45.0)));

    // A kept index answers the same way.
    let deps = Dependencies::of(&book);
    book.sheet_mut(0).unwrap().set(at("A2"), 5.0);
    deps.recalculate_from_with(&mut book, &[(0, at("A2"))], &options);
    assert_eq!(cached(&book, "B1"), Some(CellValue::Number(18.0)));
}

#[test]
fn recalculation_reports_its_progress() {
    let mut book = book();
    let seen: RefCell<Vec<(Stage, usize, Option<usize>)>> = RefCell::new(Vec::new());
    let report = |p: Progress<'_>| seen.borrow_mut().push((p.stage, p.done, p.total));
    let options = Options::new().reporting(&report);

    recalculate(&mut book, None, &options);
    let seen = seen.into_inner();
    assert_eq!(seen.len(), 3, "one report per formula: {seen:?}");
    assert!(
        seen.iter()
            .all(|(stage, ..)| *stage == Stage::Recalculating)
    );
    assert_eq!(seen[0], (Stage::Recalculating, 0, Some(3)));
    assert_eq!(seen[2], (Stage::Recalculating, 2, Some(3)));
}

#[test]
fn reading_and_writing_report_theirs() {
    let mut book = Spreadsheet::empty();
    let mut first = Worksheet::new("Первый").unwrap();
    first.set(at("A1"), 1.0);
    book.add_sheet(first).unwrap();
    for name in ["Второй", "Третий"] {
        book.add_sheet(Worksheet::new(name).unwrap()).unwrap();
    }

    let written: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let mut bytes = Vec::new();
    {
        let report = |p: Progress<'_>| {
            assert_eq!(p.stage, Stage::Writing);
            written.borrow_mut().push(p.what.to_owned());
        };
        write_xlsx_to_with(
            &book,
            Cursor::new(&mut bytes),
            &Options::new().reporting(&report),
        )
        .unwrap();
    }
    let written = written.into_inner();
    assert!(
        written.contains(&"xl/workbook.xml".to_owned()),
        "{written:?}"
    );
    assert!(
        written.iter().any(|p| p.ends_with("sheet3.xml")),
        "{written:?}"
    );

    let read: RefCell<Vec<Step>> = RefCell::new(Vec::new());
    {
        let report = |p: Progress<'_>| {
            read.borrow_mut()
                .push((p.what.to_owned(), p.total, p.fraction()));
        };
        let options = Options::new().reporting(&report);
        let back = read_xlsx_from_with(Cursor::new(&bytes), 512 << 20, &options).unwrap();
        assert_eq!(back.sheets().len(), 3);
    }
    let read = read.into_inner();
    // Sheets are the unit, and each report names the one about to be read.
    assert_eq!(read.len(), 3);
    assert_eq!(read[0].0, "Первый");
    assert_eq!(read[1].0, "Второй");
    assert_eq!(read[2], ("Третий".to_owned(), Some(3), Some(2.0 / 3.0)));
}

#[test]
fn the_plain_functions_still_work_untold() {
    // Nothing above changes what the existing entry points do.
    let mut book = book();
    let mut bytes = Vec::new();
    write_xlsx_to_with(&book, Cursor::new(&mut bytes), &Options::new()).unwrap();
    let back = read_xlsx_from(Cursor::new(&bytes)).unwrap();
    assert_eq!(back.sheets().len(), 1);
    assert_eq!(
        excelerate::formula::eval::recalculate(&mut book, None, &Options::default()),
        3
    );
}
