//! Compares the answers Excel stored in a probe workbook with this engine's.
//!
//! `cargo run --release --example probe_check -- probe-formulas-excel.xlsx`
//!
//! The file is one that [`probe`](../probe.rs) built and Excel then opened and
//! saved: every question is a formula, and Excel left its answer beside it.
//! Each line of the output is one question, what Excel said, what this engine
//! says, and whether the two agree.

use excelerate::coordinate::{CellRef, Col};
use excelerate::model::{CellValue, Worksheet};

type Fallible = Result<(), Box<dyn std::error::Error>>;

fn main() -> Fallible {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: probe_check <probe-formulas-excel.xlsx>")?;
    let book = excelerate::reader::read(&path)?;
    let mut agreed = 0;
    let mut differed = 0;
    for (index, sheet) in book.sheets().iter().enumerate() {
        for (at, cell) in sheet.iter() {
            let CellValue::Formula { formula, cached } = &cell.value else {
                continue;
            };
            let theirs = cached.as_deref().map_or_else(String::new, shown);
            let value = excelerate::formula::eval::Engine::new(&book)
                .eval(excelerate::formula::eval::Origin::new(index, at), formula);
            let ours = ours(&value);
            let same = theirs.trim() == ours.trim();
            if same {
                agreed += 1;
            } else {
                differed += 1;
            }
            println!(
                "{}\t{at}\t{}\tExcel: {theirs:?}\tнаш: {ours:?}\t{}",
                sheet.title(),
                question(sheet, at).unwrap_or_else(|| formula.clone()),
                if same {
                    "совпало"
                } else {
                    "РАЗОШЛОСЬ"
                }
            );
        }
    }
    eprintln!("совпало {agreed}, разошлось {differed}");
    Ok(())
}

/// What this engine answers, rendered the way the cells show it; an array
/// row by row, as the probe writes our answer beside the question.
fn ours(value: &excelerate::formula::value::Value) -> String {
    use excelerate::formula::value::Value;
    match value {
        Value::Array(rows) => rows
            .iter()
            .map(|row| row.iter().map(ours).collect::<Vec<_>>().join(" | "))
            .collect::<Vec<_>>()
            .join(" ; "),
        Value::Blank => String::new(),
        Value::Number(n) => format!("{n}"),
        Value::Text(t) => t.clone(),
        Value::Bool(b) => if *b { "ИСТИНА" } else { "ЛОЖЬ" }.to_owned(),
        Value::Error(e) => e.as_str().to_owned(),
        Value::Lambda(_) => "(функция)".to_owned(),
    }
}

/// The text of the question, which the probe puts to the left of the formula.
fn question(sheet: &Worksheet, at: CellRef) -> Option<String> {
    let left = CellRef::new(Col::new(at.col.index().checked_sub(2)?)?, at.row);
    sheet.get(left)?.value.plain_text()
}

fn shown(value: &CellValue) -> String {
    match value {
        CellValue::Text(t) => t.to_string(),
        CellValue::Number(n) => format!("{n}"),
        CellValue::Bool(b) => if *b { "ИСТИНА" } else { "ЛОЖЬ" }.to_owned(),
        CellValue::Error(e) => e.as_str().to_owned(),
        rich @ CellValue::RichText(_) => rich.plain_text().unwrap_or_default(),
        CellValue::Empty | CellValue::Formula { .. } => String::new(),
    }
}
