//! Edits a workbook's grid and shows what moved with it.
//!
//! Inserting a row is not a memmove of the cells: formulas on every sheet,
//! defined names, merges, tables, charts and drawing anchors all point at
//! coordinates, and all of them have to follow.
//!
//! ```text
//! cargo run --release --example edit -- book.xlsx out.xlsx insert-rows Sheet1 5 2
//! cargo run --release --example edit -- book.xlsx out.xlsx remove-columns Sheet1 C 1
//! cargo run --release --example edit -- book.xlsx out.xlsx rename Sheet1 Данные
//! cargo run --release --example edit -- book.xlsx out.xlsx drop Sheet1
//! ```

// A developer tool: a failed step should stop it with a readable message.
#![allow(clippy::expect_used, clippy::panic, clippy::print_stdout)]

use excelerate::model::Spreadsheet;
use excelerate::{CellRef, edit};

/// The index of the sheet with this title.
fn sheet_index(book: &Spreadsheet, title: &str) -> usize {
    book.sheets()
        .iter()
        .position(|s| s.title() == title)
        .expect("no sheet by that name")
}

/// The formulas of each sheet, in order, so that two states can be compared
/// sheet by sheet. A sheet keeps the order of its formulas across an edit
/// even when their addresses move, which is what makes the pairing below
/// honest.
fn formulas(book: &Spreadsheet) -> Vec<Vec<(CellRef, String)>> {
    book.sheets()
        .iter()
        .map(|sheet| {
            sheet
                .iter()
                .filter_map(|(at, cell)| match &cell.value {
                    excelerate::model::CellValue::Formula { formula, .. } => {
                        Some((at, formula.clone()))
                    }
                    _ => None,
                })
                .collect()
        })
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .expect("usage: edit <in> <out> <op> <sheet> [..]");
    let output = args
        .next()
        .expect("usage: edit <in> <out> <op> <sheet> [..]");
    let op = args
        .next()
        .expect("usage: edit <in> <out> <op> <sheet> [..]");
    let title = args
        .next()
        .expect("usage: edit <in> <out> <op> <sheet> [..]");

    let mut book = excelerate::reader::read(&input).expect("input reads");
    let sheet = sheet_index(&book, &title);
    let before = formulas(&book);

    let started = std::time::Instant::now();
    match op.as_str() {
        "insert-rows" | "remove-rows" => {
            let at = args
                .next()
                .expect("a row number")
                .parse::<u64>()
                .expect("a number");
            let at = excelerate::Row::from_one_based(at).expect("a row of the sheet");
            let count = args.next().map_or(1, |n| n.parse().expect("a number"));
            if op == "insert-rows" {
                edit::insert_rows(&mut book, sheet, at, count)
            } else {
                edit::remove_rows(&mut book, sheet, at, count)
            }
            .expect("the edit applies");
        }
        "insert-columns" | "remove-columns" => {
            let letters = args.next().expect("a column letter");
            let at = excelerate::Col::from_letters(&letters).expect("a column of the sheet");
            let count = args.next().map_or(1, |n| n.parse().expect("a number"));
            if op == "insert-columns" {
                edit::insert_columns(&mut book, sheet, at, count)
            } else {
                edit::remove_columns(&mut book, sheet, at, count)
            }
            .expect("the edit applies");
        }
        // Renaming touches every reference to the sheet in the workbook, not
        // just the tab: `Data!A1` lives inside formulas, names and chart
        // series.
        "rename" => {
            let to = args.next().expect("the new name");
            edit::rename_sheet(&mut book, sheet, &to).expect("the name is valid");
        }
        // Removing one turns those references into `#REF!A1`, the way Excel
        // writes it - the bang is part of the error, not a separator after it.
        "drop" => edit::remove_sheet(&mut book, sheet).expect("the sheet goes"),
        other => panic!("unknown operation {other}"),
    }
    let edited = started.elapsed();

    let after = formulas(&book);
    let dropped = op == "drop";
    let mut changed = 0;
    let mut broken = 0;
    for (index, was_sheet) in before.iter().enumerate() {
        // The removed sheet has no counterpart, and every sheet after it moved
        // one place down.
        let now_index = match (dropped, index.cmp(&sheet)) {
            (true, std::cmp::Ordering::Equal) => continue,
            (true, std::cmp::Ordering::Greater) => index - 1,
            _ => index,
        };
        let title = book
            .sheet(now_index)
            .map_or("", excelerate::model::Worksheet::title);
        for ((_, was), (at, now)) in was_sheet.iter().zip(&after[now_index]) {
            if was == now {
                continue;
            }
            changed += 1;
            if changed <= 10 {
                println!("{title}!{at}\t{was}\t->\t{now}");
            }
            if now.contains("#REF!") && !was.contains("#REF!") {
                broken += 1;
            }
        }
    }
    println!("{edited:?}: {changed} formulas rewritten, {broken} now #REF!");

    // The cached results of the rewritten formulas were answers to the old
    // text, so they are gone; this puts the numbers back.
    let computed = excelerate::formula::eval::recalculate(
        &mut book,
        None,
        &excelerate::progress::Options::default(),
    );
    println!("{computed} formulas recomputed");

    excelerate::writer::xlsx::write_xlsx(&book, &output).expect("output writes");
}
