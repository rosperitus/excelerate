//! Real workbooks, too large or too private to commit: whatever lies in
//! `tests/corpus/`, which git ignores.
//!
//! ```text
//! cargo test --release --test corpus -- --ignored --nocapture
//! ```
//!
//! Ignored by default: a full recalculation of a book with half a million
//! formulas takes seconds in a release build and minutes in a debug one. With
//! no corpus on the machine the test passes having checked nothing, so a
//! clone without the files is not broken.
//!
//! For every book: it reads, every formula parses, and it survives being
//! written in its own format and read back. For a book listed in
//! [`AGREEMENT`], at least that many formulas must compute to the result the
//! file cached - a floor, so a speed-up that changes answers shows up here
//! rather than in someone's spreadsheet.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use excelerate::formula::eval::{Engine, Origin};
use excelerate::formula::parser::parse;
use excelerate::formula::value::Value;
use excelerate::model::{CellValue, Spreadsheet};
use std::io::Cursor;
use std::path::Path;

/// Formulas that must agree with the cache Excel saved, by file name.
///
/// `test1.xlsx` was last saved by something other than Excel (no
/// `calcChain.xml`), so its cache is not Excel's and the floor is what the
/// engine agreed on when it was recorded, not a target. `COIN` was saved by
/// Excel in manual calculation mode.
const AGREEMENT: &[(&str, usize)] = &[
    ("test1.xlsx", 13_270),
    ("COIN_Tool_v1_LITE_exampledata.xlsm", 564_694),
];

/// Files the reader refuses on purpose, with the reason it must give.
const REFUSED: &[&str] = &["BIFF version 0x0500 is not supported"];

#[test]
#[ignore = "needs tests/corpus/, and a release build to finish in seconds"]
fn every_workbook_of_the_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!("no tests/corpus/, nothing to check");
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    let mut failures = Vec::new();
    for path in paths.iter().filter(|p| p.is_file()) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if let Err(problem) = check(path, &name) {
            println!("FAIL {name}: {problem}");
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "failed: {failures:?}");
}

fn check(path: &Path, name: &str) -> Result<(), String> {
    let started = std::time::Instant::now();
    let book = match excelerate::reader::read(path) {
        Ok(book) => book,
        Err(e) if REFUSED.iter().any(|r| e.to_string().contains(r)) => {
            println!("skip {name}: {e}");
            return Ok(());
        }
        Err(e) => return Err(format!("does not read: {e}")),
    };

    let formulas: Vec<(usize, excelerate::CellRef, String)> = book
        .sheets()
        .iter()
        .enumerate()
        .flat_map(|(i, s)| {
            s.iter().filter_map(move |(at, c)| match &c.value {
                CellValue::Formula { formula, .. } => Some((i, at, formula.clone())),
                _ => None,
            })
        })
        .collect();
    if let Some((i, at, f)) = formulas.iter().find(|(_, _, f)| parse(f).is_err()) {
        return Err(format!("sheet {i} {at}: {f:?} does not parse"));
    }

    round_trip(&book, name)?;

    let (checked, agree) = agreement(&book);
    println!(
        "ok   {name}: {} formulas, {agree} of {checked} agree with the cache, {:.1?}",
        formulas.len(),
        started.elapsed()
    );
    if let Some((_, floor)) = AGREEMENT.iter().find(|(n, _)| *n == name)
        && agree < *floor
    {
        return Err(format!("{agree} formulas agree, the floor is {floor}"));
    }
    Ok(())
}

/// Writes the book in its own format, reads it back, and compares cell by cell.
///
/// xls stores formulas as tokens, and parentheses that change nothing do not
/// survive that; formulas there are compared by what they parse to.
fn round_trip(book: &Spreadsheet, name: &str) -> Result<(), String> {
    let xls = name.to_ascii_lowercase().ends_with(".xls");
    let mut bytes = Vec::new();
    let back = if xls {
        excelerate::writer::xls::write_xls_to(book, &mut bytes).map_err(|e| e.to_string())?;
        excelerate::reader::xls::read_xls_from(&bytes).map_err(|e| e.to_string())?
    } else {
        excelerate::writer::xlsx::write_xlsx_to(book, Cursor::new(&mut bytes))
            .map_err(|e| e.to_string())?;
        excelerate::reader::xlsx::read_xlsx_from(Cursor::new(bytes)).map_err(|e| e.to_string())?
    };
    if back.sheets().len() != book.sheets().len() {
        return Err("a sheet was lost on the way back".to_owned());
    }
    for (sheet, (a, b)) in book.sheets().iter().zip(back.sheets()).enumerate() {
        for (at, cell) in a.iter() {
            let after = b.get(at).map(|c| &c.value);
            let same = match (&cell.value, after) {
                (
                    CellValue::Formula {
                        formula: x,
                        cached: cx,
                    },
                    Some(CellValue::Formula {
                        formula: y,
                        cached: cy,
                    }),
                ) => {
                    let text = x == y || (xls && parse(x).ok() == parse(y).ok());
                    // A formula with no cache gets one computed on the way.
                    text && (cx.is_none() || cx == cy)
                }
                (before, Some(after)) => before == after,
                (CellValue::Empty, None) => true,
                (_, None) => false,
            };
            if !same {
                return Err(format!(
                    "sheet {sheet} {at}: {:?} came back as {after:?}",
                    cell.value
                ));
            }
        }
    }
    Ok(())
}

/// How many formulas with a cached result compute to it: checked, agreeing.
fn agreement(book: &Spreadsheet) -> (usize, usize) {
    let mut engine = Engine::new(book);
    let (mut checked, mut agree) = (0, 0);
    for (index, sheet) in book.sheets().iter().enumerate() {
        for (at, cell) in sheet.iter() {
            let CellValue::Formula {
                formula,
                cached: Some(cached),
            } = &cell.value
            else {
                continue;
            };
            checked += 1;
            let got = engine.eval(Origin::new(index, at), formula);
            if same(&got, cached) {
                agree += 1;
            }
        }
    }
    (checked, agree)
}

/// Whether a computed value is the one a file cached. The same rule as
/// `examples/recalc.rs`, which the floors in [`AGREEMENT`] were taken with.
fn same(got: &Value, cached: &CellValue) -> bool {
    match (got, cached) {
        (Value::Number(a), CellValue::Number(b)) => (a - b).abs() <= b.abs() * 1e-9 + 1e-9,
        (Value::Text(a), CellValue::Text(b)) => a == b,
        (Value::Bool(a), CellValue::Bool(b)) => a == b,
        (Value::Error(a), CellValue::Error(b)) => a == b,
        // Excel writes an empty result as an empty string.
        (Value::Number(a), CellValue::Text(b)) => *a == 0.0 && b.is_empty(),
        _ => false,
    }
}
