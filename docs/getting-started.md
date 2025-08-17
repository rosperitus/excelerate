# Getting started

## Install

```toml
[dependencies]
excelerate = "0.1"
```

Rust 2024 edition. No system libraries, no build scripts, nothing to install
next to it.

## Open a file

`reader::read` figures out the format from the file's own bytes and falls back
to the extension only when the bytes say nothing:

```rust
use excelerate::reader;

let book = reader::read("budget.xlsx")?;

for sheet in book.sheets() {
    println!("{} — {} cells", sheet.title(), sheet.len());
}
# Ok::<(), excelerate::Error>(())
```

Got bytes instead of a path — an upload, an S3 object, a blob from JS?

```rust
use excelerate::reader::read_bytes;

# let bytes: Vec<u8> = std::fs::read("tests/test1.xlsx").unwrap();
let book = read_bytes(&bytes, Some("upload.xlsx"))?;
# Ok::<(), excelerate::Error>(())
```

The name is optional — it is only consulted when the signature is ambiguous
(and it gives a SYLK file its sheet name).

If you know exactly what you have, call the format directly and skip the
sniffing: `read_xlsx`, `read_xls`, `read_ods`, `read_csv`, `read_html`,
`read_slk`, `read_gnumeric`, `read_xml2003`.

## Read some values

```rust
use excelerate::CellRef;
use excelerate::model::CellValue;
# use excelerate::reader;

# let book = reader::read("tests/test1.xlsx")?;
let sheet = book.sheet(0).unwrap();

match &sheet.get(CellRef::parse("B4")?).map(|c| &c.value) {
    Some(CellValue::Number(n)) => println!("number: {n}"),
    Some(CellValue::Text(t)) => println!("text: {t}"),
    // A formula keeps the result the file was saved with, so you can read a
    // workbook without recalculating anything.
    Some(CellValue::Formula { formula, cached }) => {
        println!("={formula} → {cached:?}");
    }
    _ => println!("empty"),
}
# Ok::<(), excelerate::Error>(())
```

Walking everything that is actually there beats scanning a rectangle of mostly
nothing:

```rust
# use excelerate::reader;
# let book = reader::read("tests/test1.xlsx")?;
# let sheet = book.sheet(0).unwrap();
for (at, cell) in sheet.iter() {
    // Row by row, left to right.
    let _ = (at, &cell.value);
}
# Ok::<(), excelerate::Error>(())
```

## Build a workbook from scratch

```rust
use excelerate::CellRef;
use excelerate::model::{Spreadsheet, Worksheet};
use excelerate::writer::write_xlsx;

// `empty()` gives you a book with no sheets; `Spreadsheet::new()` starts with
// one, the way a fresh file in Excel does.
let mut book = Spreadsheet::empty();
let mut sheet = Worksheet::new("Sales")?;

for (row, (name, amount)) in [("Tea", 42.0), ("Coffee", 128.5)].iter().enumerate() {
    let row = u32::try_from(row).unwrap() + 1;
    sheet.set(CellRef::parse(&format!("A{row}"))?, *name);
    sheet.set(CellRef::parse(&format!("B{row}"))?, *amount);
}

book.add_sheet(sheet)?;
write_xlsx(&book, "sales.xlsx")?;
# Ok::<(), excelerate::Error>(())
```

Sheet names are validated where you make them: empty, longer than 31 chars, or
containing `* : / \ ? [ ]` gets you an `Error::InvalidSheetName` instead of a
file Excel refuses to open.

## Convert between formats

```rust
use excelerate::{reader, writer};

let book = reader::read("legacy.xls")?;
writer::write_xlsx(&book, "modern.xlsx")?;
writer::write_html(&book, "preview.html")?;
# Ok::<(), excelerate::Error>(())
```

There is a ready-made example in the repo, too:

```
cargo run --release --example convert -- input.xlsx output.ods
```

## Errors

Everything fallible returns `excelerate::Result<T>`, and the error is a
plain `thiserror` enum — one variant per format plus the shared ones
(`InvalidCellRef`, `InvalidSheetName`, …). No panics on malformed input: a
truncated zip, a bogus address or a BIFF5 file all come back as `Err`.

Heads up on one deliberate limit: zip expansion is capped at 512 MB to keep a
zip bomb from eating the process. Real workbooks do blow past it — a 100 MB
package can expand to 560 MB — so raise it explicitly when you need to:

```rust
use excelerate::reader::read_bytes_limited;

# let bytes: Vec<u8> = std::fs::read("tests/test1.xlsx").unwrap();
let book = read_bytes_limited(&bytes, Some("big.xlsx"), 4 << 30)?;
# Ok::<(), excelerate::Error>(())
```

## Where to next

- [Workbook model](model.md) — how sheets, cells and addresses fit together
- [Formulas](formulas.md) — making the numbers update
- [File formats](formats.md) — what survives which format
