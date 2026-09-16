//! Evaluates formulas from stdin against the table excelize's own tests use.
//!
//! Its purpose is to run the expectations in `excelize-master/calc_test.go`
//! through this engine - see `tools/excelize-oracle/README.md`. The sheet is
//! the one that file sets up above its formula table, so the expectations line
//! up cell for cell.
//!
//! `cargo run --release --example xcheck < formulas.txt`
#![allow(clippy::expect_used, clippy::print_stdout, clippy::unwrap_used)]

use excelerate::CellRef;
use excelerate::formula::eval::{Engine, Origin};
use excelerate::formula::value::Value;
use excelerate::model::{Spreadsheet, Worksheet};
use std::io::BufRead;

fn main() {
    let mut wb = Spreadsheet::empty();
    let mut ws = Worksheet::new("Sheet1").unwrap();
    let at = |a: &str| CellRef::parse(a).unwrap();
    for (row, n) in [(1, 1.0), (2, 2.0), (3, 3.0), (4, 0.0)] {
        ws.set(at(&format!("A{row}")), n);
    }
    ws.set(at("B1"), 4.0);
    ws.set(at("B2"), 5.0);
    for (row, text) in [
        (1, "Month"),
        (2, "Jan"),
        (3, "Jan"),
        (4, "Jan"),
        (5, "Jan"),
        (6, "Feb"),
        (7, "Feb"),
        (8, "Feb"),
        (9, "Feb"),
    ] {
        ws.set(at(&format!("D{row}")), text);
    }
    for (row, text) in [
        (1, "Team"),
        (2, "North 1"),
        (3, "North 2"),
        (4, "South 1"),
        (5, "South 2"),
        (6, "North 1"),
        (7, "North 2"),
        (8, "South 1"),
        (9, "South 2"),
    ] {
        ws.set(at(&format!("E{row}")), text);
    }
    ws.set(at("F1"), "Sales");
    for (row, n) in [
        (2, 36693.0),
        (3, 22100.0),
        (4, 53321.0),
        (5, 34440.0),
        (6, 29889.0),
        (7, 50090.0),
        (8, 32080.0),
        (9, 45500.0),
    ] {
        ws.set(at(&format!("F{row}")), n);
    }
    ws.set(at("G2"), 4.0);
    ws.set(at("G3"), 2.0);
    wb.add_sheet(ws).unwrap();

    let mut engine = Engine::new(&wb);
    let origin = Origin::new(0, at("Z100"));
    for line in std::io::stdin().lock().lines() {
        let formula = line.unwrap();
        if formula.trim().is_empty() {
            continue;
        }
        let shown = match engine.eval(origin, &formula) {
            Value::Number(n) => format!("{n}"),
            Value::Text(t) => t,
            Value::Bool(b) => if b { "TRUE" } else { "FALSE" }.to_owned(),
            Value::Error(e) => e.as_str().to_owned(),
            Value::Blank => String::new(),
            Value::Array(_) => "#ARRAY".to_owned(),
            Value::Lambda(_) => "#LAMBDA".to_owned(),
        };
        println!("{formula}\t{shown}");
    }
}
