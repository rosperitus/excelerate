# Excelerate

Read, write and recalculate spreadsheets in pure Rust.

Opens `.xlsx`, `.xls`, `.ods`, `.csv`, `.html`, `.slk`, `.gnumeric` and
SpreadsheetML 2003, hands you the workbook as a plain Rust struct, evaluates
formulas (443 Excel functions and counting) and writes the whole thing back.
No Excel, no LibreOffice, no COM, no headless anything - just the crate.

```toml
[dependencies]
excelerate = "0.8"
```

```rust
use excelerate::reader;

let book = reader::read("report.xlsx")?;
let sheet = book.active_sheet().unwrap();
println!("{}: {} non-empty cells", sheet.title(), sheet.len());
# Ok::<(), excelerate::Error>(())
```

## Why you might want it

- **The format is sniffed, not guessed from the extension.** `reader::read`
  looks at the bytes first, so a `.txt` that is secretly a zip package still
  opens as xlsx. The extension only gets a vote when the bytes stay quiet.
- **Formulas actually run.** Recalculate the whole book, one sheet, or just
  what an edit touched - the incremental path is roughly 20x cheaper on a book
  with 20k formulas.
- **Round-trips without eating your file.** Read -> write -> read keeps styles,
  merges, conditional formatting, protection, autofilters and print setup.
  Charts, pictures, shapes and comments are modelled and written back;
  anything the crate does not model yet rides through byte for byte along with
  its relationships.
- **Builds for WebAssembly**, so the same reader runs in Node.
- **No `unsafe`**, `clippy::pedantic` clean, and the only deps are `zip`,
  `quick-xml`, `flate2` and `thiserror`.

## Sixty-second tour

```rust
use excelerate::CellRef;
use excelerate::formula::eval::recalculate;
use excelerate::model::{CellValue, Spreadsheet, Worksheet};
use excelerate::writer::write_xlsx;

let mut book = Spreadsheet::empty();
let mut sheet = Worksheet::new("Quote")?;

sheet.set(CellRef::parse("A1")?, "Unit price");
sheet.set(CellRef::parse("B1")?, 120.0);
sheet.set(CellRef::parse("A2")?, "Quantity");
sheet.set(CellRef::parse("B2")?, 3.0);
sheet.set(CellRef::parse("A3")?, "Total");
sheet.set(
    CellRef::parse("B3")?,
    CellValue::Formula { formula: "B1*B2".into(), cached: None },
);

book.add_sheet(sheet)?;
recalculate(&mut book, None);       // B3 is now 360
write_xlsx(&book, "quote.xlsx")?;
# Ok::<(), excelerate::Error>(())
```

Converting between formats is a two-liner - the output format comes from the
extension you ask for:

```rust
use excelerate::{reader, writer};

let book = reader::read("report.xlsx")?;
writer::write_ods(&book, "report.ods")?;
writer::write_csv(&book, 0, "report.csv")?;
# Ok::<(), excelerate::Error>(())
```

## Formats

| Format | Read | Write |
|---|:--:|:--:|
| xlsx (OOXML) | ✅ | ✅ |
| xls (BIFF8) | ✅ | ✅ |
| xlsb (BIFF12) | ✅ | — |
| ods (OpenDocument) | ✅ | ✅ |
| CSV | ✅ | ✅ |
| HTML | ✅ | ✅ |
| SYLK (`.slk`) | ✅ | — |
| Gnumeric | ✅ | — |
| SpreadsheetML 2003 (`.xml`) | ✅ | — |

Every format has its own sharp edges; they are all written down in
[docs/formats.md](docs/formats.md) rather than left for you to find at 3am.

## Docs

| Page | What's in it |
|---|---|
| [Getting started](docs/getting-started.md) | install, read a file, write one, the whole loop |
| [Workbook model](docs/model.md) | workbook, sheet, cell, addresses and ranges |
| [File formats](docs/formats.md) | what each format carries and what it drops |
| [Formulas](docs/formulas.md) | evaluation, incremental recalc, driving the engine yourself |
| [Styles and number formats](docs/styles.md) | fonts, fills, borders, format strings |
| [Links to other workbooks](docs/external-links.md) | `[1]Sheet1!A1`, the value cache, plugging in a live file |
| [WebAssembly](docs/wasm.md) | building for Node and reading a book from JS |

## Building

```
cargo test
cargo clippy --all-targets -- -D warnings
```

## Status

Formats, styles, the formula engine and the xlsx object model (protection,
autofilters, validation, conditional formatting, print setup, tables, charts,
pictures, shapes, comments, pivot tables) are in. xls keeps formulas and styles both ways. See [docs/formats.md](docs/formats.md) for the exact line
between "modelled" and "carried".

## License

MIT - see [LICENSE](LICENSE).
