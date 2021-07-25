//! Reads a workbook and writes it back out, with no verification pass.
//!
//! The input's format is worked out by [`excelerate::reader::read`]; the
//! output's is taken from its extension, so this also converts between them:
//! `.csv` means CSV, `.ods` `OpenDocument`, `.html` HTML, `.xls` BIFF8,
//! anything else xlsx.
//!
//! `cargo run --release --example convert -- in.xlsx out.ods`

// A developer tool: a failed step should stop it with a readable message.
#![allow(clippy::expect_used, clippy::print_stdout)]

use excelerate::reader::read;
use excelerate::writer::{write_csv, write_html, write_ods, write_xls, xlsx::write_xlsx};

/// Whether a path carries one of these extensions.
fn has_extension(path: &str, wanted: &[&str]) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|e| wanted.iter().any(|w| e.eq_ignore_ascii_case(w)))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: convert <in.xlsx> <out.xlsx>");
    let output = args.next().expect("usage: convert <in.xlsx> <out.xlsx>");

    let started = std::time::Instant::now();
    let book = read(&input).expect("input reads");
    let read_time = started.elapsed();

    let started = std::time::Instant::now();
    if has_extension(&output, &["csv"]) {
        write_csv(&book, book.active_index(), &output)
    } else if has_extension(&output, &["ods"]) {
        write_ods(&book, &output)
    } else if has_extension(&output, &["html", "htm"]) {
        write_html(&book, &output)
    } else if has_extension(&output, &["xls"]) {
        write_xls(&book, &output)
    } else {
        write_xlsx(&book, &output)
    }
    .expect("output writes");

    let cells: usize = book
        .sheets()
        .iter()
        .map(excelerate::model::Worksheet::len)
        .sum();
    println!(
        "read in {read_time:?}, written in {:?}: {} sheets, {cells} cells, {} bytes",
        started.elapsed(),
        book.sheets().len(),
        std::fs::metadata(&output).map_or(0, |m| m.len())
    );
}
