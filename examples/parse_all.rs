//! Parses every formula in a workbook and reports what the parser choked on.
//!
//! `cargo run --release --example parse_all -- book.xlsx`

// A developer tool: a failed step should stop it with a readable message.
#![allow(clippy::expect_used, clippy::print_stdout)]

use excelerate::model::CellValue;
use excelerate::reader::xlsx::read_xlsx;
use std::collections::BTreeMap;

fn main() {
    let input = std::env::args()
        .nth(1)
        .expect("usage: parse_all <book.xlsx>");
    let book = read_xlsx(&input).expect("input reads");

    let (mut total, mut failed) = (0usize, 0usize);
    let mut reasons: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for sheet in book.sheets() {
        for (_, cell) in sheet.iter() {
            let CellValue::Formula { formula, .. } = &cell.value else {
                continue;
            };
            total += 1;
            if let Err(e) = excelerate::formula::parse(formula) {
                failed += 1;
                let entry = reasons
                    .entry(e.to_string())
                    .or_insert_with(|| (0, formula.clone()));
                entry.0 += 1;
            }
        }
    }
    println!("{total} formulas, {failed} did not parse");
    for (reason, (count, example)) in reasons {
        println!("  {count:6}  {reason}   for example: {example}");
    }
}
