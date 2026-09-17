//! Recomputes every formula of a workbook and compares with the result Excel
//! itself left in the file.
//!
//! The cached value beside each formula is what the application that last saved
//! the file computed, which makes a real workbook an oracle for the engine with
//! no other spreadsheet program on the machine.
//!
//! It is only as good as that application, though. A workbook whose package
//! holds no `xl/calcChain.xml` was not last written by Excel - Excel always
//! writes one for a workbook with formulas - and its cached values may be
//! whatever some other tool left behind. Check a disagreement against a third
//! engine before treating it as a bug here.
//!
//! `cargo run --release --example recalc -- book.xlsx [examples-per-bucket]`

// A developer tool: a failed step should stop it with a readable message.
#![allow(clippy::expect_used, clippy::print_stdout)]

use excelerate::formula::eval::{Engine, Origin};
use excelerate::formula::value::Value;
use excelerate::model::CellValue;
use std::collections::BTreeMap;

fn main() {
    let input = std::env::args().nth(1).expect("usage: recalc <book.xlsx>");
    // How many disagreements of each kind to print; five is enough to see the
    // shape of them, more is for digging into one.
    let per_bucket: usize = std::env::args()
        .nth(2)
        .map_or(5, |n| n.parse().expect("example count is a number"));
    let book = excelerate::reader::read(&input).expect("input reads");

    let mut engine = Engine::new(&book);
    let (mut checked, mut agree) = (0usize, 0usize);
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut buckets: BTreeMap<&str, usize> = BTreeMap::new();
    let mut wrong: Vec<(&str, String)> = Vec::new();

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
            } else if let Value::Error(excelerate::error::CellError::Name) = got {
                // An unimplemented function, not a wrong answer.
                for name in unknown_names(formula) {
                    *missing.entry(name).or_default() += 1;
                }
            } else {
                // Where the two sides disagree matters more than that they do:
                // a stale error in the file is not the same as a wrong number.
                let bucket = match (&got, cached.as_ref()) {
                    (Value::Error(_), CellValue::Error(_)) => "an error, but a different one",
                    (_, CellValue::Error(_)) => "an error in the file, a value here",
                    (Value::Error(_), _) => "a value in the file, an error here",
                    _ => "different values",
                };
                *buckets.entry(bucket).or_default() += 1;
                let shown = wrong.iter().filter(|(b, _)| *b == bucket).count();
                if shown < per_bucket {
                    wrong.push((
                        bucket,
                        format!(
                            "{}!{at}  {formula}\n      Excel {cached:?}\n      us    {got:?}",
                            sheet.title()
                        ),
                    ));
                }
            }
        }
    }

    println!("{checked} formulas with a cached result, {agree} agree");
    for (bucket, count) in &buckets {
        println!("  {count:6}  {bucket}");
    }
    if !missing.is_empty() {
        let mut top: Vec<_> = missing.iter().collect();
        top.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        println!("\nno such function (top 20 by cell count):");
        for (name, count) in top.iter().take(20) {
            println!("  {count:6}  {name}");
        }
        println!("  {} distinct functions in all", missing.len());
    }
    for bucket in buckets.keys() {
        println!("\n{bucket}:");
        for (_, text) in wrong.iter().filter(|(b, _)| b == bucket) {
            println!("    {text}");
        }
    }
}

/// Whether a computed value matches the one Excel stored.
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

/// Function names in a formula that the engine does not know.
fn unknown_names(formula: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = formula.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '.') {
            i += 1;
        }
        if chars.get(i) == Some(&'(') {
            let name: String = chars[start..i].iter().collect::<String>().to_uppercase();
            if !excelerate::formula::functions::is_known(&name) {
                out.push(name);
            }
        }
    }
    out
}
