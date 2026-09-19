//! Lends the engine a function of the caller's own and watches the work go by.
//!
//! `Options` is what a caller hands a long operation: somewhere to report
//! progress, and functions the workbook may call. A name that is not
//! registered answers `#NAME?`, exactly as Excel does with macros switched
//! off, and a built-in name stays with the built-in function: a workbook where
//! `SUM` means something private is a workbook nobody else can read.
//!
//! `cargo run --release --example custom [-- book.xlsx]`

// A developer tool: a failed step should stop it with a readable message.
#![allow(clippy::expect_used, clippy::print_stdout)]

use excelerate::formula::custom::CustomFunctions;
use excelerate::formula::eval::recalculate;
use excelerate::formula::value::Value;
use excelerate::model::{CellValue, Spreadsheet, Worksheet};
use excelerate::progress::{Options, Progress};
use excelerate::{CellRef, writer};
use std::cell::Cell;

/// `A1` or the program has a typo in it.
fn at(a1: &str) -> CellRef {
    CellRef::parse(a1).expect("literal address")
}

/// A workbook calling two functions that are not Excel's.
fn sample() -> Spreadsheet {
    let mut book = Spreadsheet::empty();
    let mut sheet = Worksheet::new("Смета").expect("valid sheet name");
    for (row, amount) in [1200.0, 340.0, 7800.0].into_iter().enumerate() {
        let row = u32::try_from(row).expect("three rows") + 1;
        sheet.set(at(&format!("A{row}")), amount);
    }
    for (cell, formula) in [
        ("C1", "СНДС(A1)"),
        ("C2", "СНДС(A2)"),
        ("C3", "СНДС(A3)"),
        ("C4", "МОЙИТОГ(A1:A3)"),
        // Nobody registered this one, so it answers the way Excel does.
        ("C5", "НЕИЗВЕСТНАЯ(1)"),
    ] {
        sheet.set(at(cell), CellValue::formula(formula.to_owned()));
    }
    book.add_sheet(sheet).expect("the book takes the sheet");
    book
}

fn main() {
    let mut functions = CustomFunctions::new();

    // Arguments arrive already computed; a range arrives as `Value::Array`.
    functions.register("СНДС", |args| match args.first() {
        Some(Value::Number(n)) => Value::Number(n * 1.2),
        _ => Value::Error(excelerate::error::CellError::Value),
    });
    functions.register("МОЙИТОГ", |args| {
        let mut total = 0.0;
        let mut stack: Vec<&Value> = args.iter().collect();
        while let Some(value) = stack.pop() {
            match value {
                Value::Number(n) => total += n,
                Value::Array(rows) => stack.extend(rows.iter().flatten()),
                _ => {}
            }
        }
        Value::Number(total)
    });

    // The callback is `Fn`, so anything it accumulates lives in a `Cell` it
    // captures - the usual shape for a progress bar anyway.
    let seen = Cell::new(0usize);
    let last = Cell::new(String::new());
    let report = |p: Progress<'_>| {
        seen.set(seen.get() + 1);
        if p.done.is_multiple_of(5000) || p.fraction() == Some(1.0) {
            let percent = p
                .fraction()
                .map_or(String::new(), |f| format!(" {:.0}%", f * 100.0));
            println!("  {:?}{percent}: {} {}", p.stage, p.done, p.what);
        }
        last.set(format!("{:?}", p.stage));
    };

    let options = Options::new().reporting(&report).with_functions(&functions);

    let mut book = match std::env::args().nth(1) {
        Some(path) => excelerate::reader::read_bytes_limited_with(
            &std::fs::read(&path).expect("the file reads"),
            Some(&path),
            4 << 30,
            &options,
        )
        .expect("input reads"),
        None => sample(),
    };

    let computed = recalculate(&mut book, None, &options);
    println!("{computed} formulas, {} progress reports", seen.get());

    for sheet in book.sheets().iter().take(1) {
        for (at, cell) in sheet.iter() {
            if let CellValue::Formula { formula, cached } = &cell.value {
                println!("{at}\t={formula}\t{cached:?}");
            }
        }
    }

    // Writing reports too, one step per part of the package.
    writer::xlsx::write_xlsx_to_with(&book, &mut std::io::Cursor::new(Vec::new()), &options)
        .expect("output writes");
}
