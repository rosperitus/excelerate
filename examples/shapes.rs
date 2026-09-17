//! Lists the shapes of a workbook: sheet, name, outline, anchor and text.
//!
//! `cargo run --release --example shapes -- book.xlsx`

use excelerate::reader::xlsx::read_xlsx;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: shapes <book.xlsx>")?;
    let book = read_xlsx(&path)?;
    let mut total = 0;
    for sheet in book.sheets() {
        for shape in &sheet.shapes {
            total += 1;
            println!(
                "{}\t{}\t{}\t{:?}",
                sheet.title(),
                shape.name,
                shape.geometry.as_deref().unwrap_or("custom"),
                shape.text.replace('\n', " | ")
            );
        }
    }
    eprintln!("{total} shapes");
    Ok(())
}
