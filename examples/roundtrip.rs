//! Runs a workbook through read -> write -> read and reports what changed.
//!
//! `cargo run --release --example roundtrip -- in.xlsx out.xlsx`

// A developer tool: a failed step should stop it with a readable message.
#![allow(clippy::expect_used, clippy::print_stdout)]

use excelerate::model::{CellValue, Spreadsheet};
use excelerate::reader::xlsx::{read_xlsx, read_xlsx_from};
use excelerate::writer::xlsx::write_xlsx;
use std::io::Cursor;

fn main() {
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("usage: roundtrip <in.xlsx> <out.xlsx>");
    let output = args.next().expect("usage: roundtrip <in.xlsx> <out.xlsx>");

    let started = std::time::Instant::now();
    let before = read_xlsx(&input).expect("input reads");
    println!("read in {:?}: {}", started.elapsed(), summary(&before));

    let started = std::time::Instant::now();
    write_xlsx(&before, &output).expect("output writes");
    let size = std::fs::metadata(&output).map_or(0, |m| m.len());
    println!("written in {:?}: {size} bytes", started.elapsed());

    let bytes = std::fs::read(&output).expect("output re-reads");
    let after = read_xlsx_from(Cursor::new(bytes)).expect("what we wrote parses");
    println!("read back:  {}", summary(&after));

    compare(&before, &after);
}

fn summary(book: &Spreadsheet) -> String {
    let cells: usize = book
        .sheets()
        .iter()
        .map(excelerate::model::Worksheet::len)
        .sum();
    let formulas: usize = book
        .sheets()
        .iter()
        .flat_map(excelerate::model::Worksheet::iter)
        .filter(|(_, c)| matches!(c.value, CellValue::Formula { .. }))
        .count();
    let merges: usize = book.sheets().iter().map(|s| s.merges.len()).sum();
    let cols: usize = book.sheets().iter().map(|s| s.columns.len()).sum();
    let rows: usize = book.sheets().iter().map(|s| s.rows.len()).sum();
    let dv: usize = book.sheets().iter().map(|s| s.data_validations.len()).sum();
    let links: usize = book.sheets().iter().map(|s| s.hyperlinks.len()).sum();
    let cf: usize = book
        .sheets()
        .iter()
        .map(|s| {
            s.conditional_formats
                .iter()
                .map(|c| c.rules.len())
                .sum::<usize>()
        })
        .sum();
    let carried: usize = book.parts.len();
    let breaks: usize = book
        .sheets()
        .iter()
        .map(|s| s.row_breaks.len() + s.col_breaks.len())
        .sum();
    format!(
        "{} sheets, {cells} cells ({formulas} formulas), {merges} merges, \
         {cols} column ranges, {rows} sized rows, {dv} data validations, \
         {links} hyperlinks, {breaks} page breaks, {cf} formatting rules, \
         {} defined names, {carried} carried parts, {} styles, \
         active sheet {}",
        book.sheets().len(),
        book.defined_names.len(),
        book.styles.len(),
        book.active_index()
    )
}

/// Reports the first difference in each category rather than only a verdict.
#[expect(
    clippy::too_many_lines,
    reason = "one report line per category; splitting it would only scatter it"
)]
fn compare(before: &Spreadsheet, after: &Spreadsheet) {
    let mut problems = 0;
    if before.parts.len() != after.parts.len()
        || before.attachments != after.attachments
        || before.doc_props != after.doc_props
    {
        println!(
            "MISMATCH: carried parts - {} against {}",
            before.parts.len(),
            after.parts.len()
        );
        problems += 1;
    }
    if before.styles.differential != after.styles.differential {
        println!("MISMATCH: differential styles (dxf)");
        problems += 1;
    }
    if before.defined_names != after.defined_names {
        println!("MISMATCH: defined names");
        problems += 1;
    }
    if before.active_index() != after.active_index() {
        println!("MISMATCH: active sheet");
        problems += 1;
    }
    if before.sheets().len() != after.sheets().len() {
        println!("MISMATCH: sheet count");
        problems += 1;
    }
    for (a, b) in before.sheets().iter().zip(after.sheets()) {
        if a.title() != b.title() {
            println!("MISMATCH: sheet name {:?} -> {:?}", a.title(), b.title());
            problems += 1;
        }
        if a.merges != b.merges {
            println!("MISMATCH: merges on {:?}", a.title());
            problems += 1;
        }
        if a.columns != b.columns {
            println!("MISMATCH: column widths on {:?}", a.title());
            problems += 1;
        }
        if a.rows != b.rows {
            println!("MISMATCH: row heights on {:?}", a.title());
            problems += 1;
        }
        if a.view != b.view {
            println!(
                "MISMATCH: the view (zoom, selection, panes) on {:?}",
                a.title()
            );
            problems += 1;
        }
        if a.data_validations != b.data_validations {
            println!("MISMATCH: data validations on {:?}", a.title());
            problems += 1;
        }
        if (
            &a.margins,
            &a.page_setup,
            &a.print_options,
            &a.header_footer,
        ) != (
            &b.margins,
            &b.page_setup,
            &b.print_options,
            &b.header_footer,
        ) || a.properties != b.properties
        {
            println!("MISMATCH: print setup on {:?}", a.title());
            problems += 1;
        }
        if (&a.row_breaks, &a.col_breaks) != (&b.row_breaks, &b.col_breaks) {
            println!("MISMATCH: page breaks on {:?}", a.title());
            problems += 1;
        }
        if a.hyperlinks != b.hyperlinks {
            println!("MISMATCH: hyperlinks on {:?}", a.title());
            problems += 1;
        }
        if a.attachments != b.attachments {
            println!("MISMATCH: sheet attachments on {:?}", a.title());
            problems += 1;
        }
        if a.conditional_formats != b.conditional_formats {
            println!("MISMATCH: conditional formatting on {:?}", a.title());
            problems += 1;
        }
        if a.default_column_width != b.default_column_width
            || a.default_row_height != b.default_row_height
        {
            println!("MISMATCH: default sizes on {:?}", a.title());
            problems += 1;
        }
        if a.len() != b.len() {
            println!(
                "MISMATCH: {:?} - {} cells against {}",
                a.title(),
                a.len(),
                b.len()
            );
            problems += 1;
        }
        for ((at, ca), (_, cb)) in a.iter().zip(b.iter()) {
            if ca.value != cb.value {
                println!(
                    "MISMATCH: {}!{at} value {:?} -> {:?}",
                    a.title(),
                    ca.value,
                    cb.value
                );
                problems += 1;
            } else if before.styles.get(ca.style) != after.styles.get(cb.style) {
                println!("MISMATCH: {}!{at} style", a.title());
                problems += 1;
            }
            if problems > 10 {
                println!("... further mismatches not shown");
                return;
            }
        }
    }
    if problems == 0 {
        println!("\nread -> write -> read: no mismatches");
    }
}
