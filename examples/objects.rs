//! Lists everything a sheet holds that is not a cell: charts, pictures,
//! shapes, comments, tables, pivot reports, merges, links, validations and
//! conditional formats.
//!
//! A quick answer to "what is actually in this workbook" before writing code
//! against it.
//!
//! `cargo run --release --example objects -- book.xlsx`

// A developer tool: a failed step should stop it with a readable message.
#![allow(clippy::expect_used, clippy::print_stdout)]

use excelerate::CellRef;
use excelerate::model::chart::{Anchor, ChartText};

/// The top left cell an object sits on, as an address.
fn corner(anchor: &Anchor) -> String {
    match anchor {
        Anchor::TwoCell { from, .. } | Anchor::OneCell { from, .. } => {
            CellRef::new(from.col, from.row).to_string()
        }
        Anchor::Absolute { x, y, .. } => format!("{x}x{y} EMU"),
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: objects <book.xlsx>");
    let book = excelerate::reader::read(&path).expect("input reads");

    for sheet in book.sheets() {
        let title = sheet.title();
        println!("== {title} ({} cells, {:?})", sheet.len(), sheet.visibility);

        for chart in &sheet.charts {
            let heading = match chart.title.as_ref().and_then(|t| t.text.as_ref()) {
                Some(ChartText::Text { text, .. }) => text.clone(),
                Some(ChartText::Reference { formula, cache }) => {
                    format!("{formula} -> {}", cache.as_deref().unwrap_or(""))
                }
                None => String::new(),
            };
            println!(
                "  chart    {:<8} {:<24} plots={}",
                corner(&chart.anchor),
                heading,
                chart.plots.len()
            );
        }
        for chart in &sheet.extended_charts {
            println!("  chart16  {:<8} {}", corner(&chart.anchor), chart.name);
        }
        for image in &sheet.images {
            println!(
                "  picture  {:<8} {:<24} {:?}, {} bytes",
                corner(&image.anchor),
                image.name,
                image.format,
                image.data.len()
            );
        }
        for shape in &sheet.shapes {
            println!(
                "  shape    {:<8} {:<24} {}",
                corner(&shape.anchor),
                shape.name,
                shape.geometry.as_deref().unwrap_or("custom")
            );
        }
        for (at, comment) in &sheet.comments {
            println!(
                "  comment  {at:<8} {:<24} {}",
                comment.author,
                comment.plain_text().replace('\n', " | ")
            );
        }
        for table in &sheet.tables {
            println!(
                "  table    {:<8} {:<24} {} columns",
                table.range.to_string(),
                table.display_name,
                table.columns.len()
            );
        }
        for pivot in &sheet.pivot_tables {
            let at = pivot
                .location
                .map_or_else(|| "-".to_owned(), |r| r.to_string());
            println!("  pivot    {at:<8} {}", pivot.name);
        }
        for link in &sheet.hyperlinks {
            println!("  link     {:<8} {:?}", link.range.to_string(), link.target);
        }
        for merge in &sheet.merges {
            println!("  merge    {merge}");
        }
        for validation in &sheet.data_validations {
            println!("  validate {:?}", validation.kind);
        }
        for format in &sheet.conditional_formats {
            println!("  cf       {} rules", format.rules.len());
        }
        if sheet.protection.sheet == Some(true) {
            println!(
                "  locked   password={}",
                sheet.protection.password.is_some()
            );
        }
        if let Some(filter) = &sheet.auto_filter {
            println!("  filter   {}", filter.range);
        }
    }
}
