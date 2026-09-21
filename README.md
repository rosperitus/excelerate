# Excelerate

Read, write and recalculate spreadsheets in pure Rust.

Opens `.xlsx`, `.xlsb`, `.xls`, `.ods`, `.csv`, `.html`, `.slk`, `.gnumeric`
and SpreadsheetML 2003, hands you the workbook as a plain Rust struct, evaluates
formulas (518 Excel functions and counting) and writes the whole thing back.
No Excel, no LibreOffice, no COM, no headless anything - just the crate.

```toml
[dependencies]
excelerate = "0.12"
```

```rust,no_run
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
- **Edits the grid like Excel does.** Insert or remove rows, columns and
  cells, copy or move a block, reorder the sheets - and the formulas, merges,
  links, tables and drawings across the whole workbook follow. A copy rewrites
  its formulas, a move keeps them and drags the references to it along.
  Inserted rows can take the formatting of their neighbour, ranges and tables
  sort in Excel's order, and the fill handle's series (`1, 3, 5`, `Jan, Feb`,
  `Q1, Q2`) are one call.
- **Builds for WebAssembly**, and ships as two npm packages:
  [`@rosperitus/excelerate`](https://www.npmjs.com/package/@rosperitus/excelerate)
  and a read-only `@rosperitus/excelerate-reader` at a third of the size.
- **No `unsafe`**, `clippy::pedantic` clean, and the deps are `zip`, `quick-xml`,
  `flate2`, `thiserror`, `regex` for the `REGEX*` functions, and `sha1`/`sha2`/
  `getrandom` for password hashes - all but `regex` already come with `zip`.

## Sixty-second tour

```rust,no_run
use excelerate::CellRef;
use excelerate::formula::eval::recalculate;
use excelerate::progress::Options;
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
recalculate(&mut book, None, &Options::default());   // B3 is now 360
write_xlsx(&book, "quote.xlsx")?;
# Ok::<(), excelerate::Error>(())
```

Converting between formats is a two-liner:

```rust,no_run
use excelerate::{reader, writer};

let book = reader::read("report.xlsx")?;
writer::write_ods(&book, "report.ods")?;
writer::write_csv(&book, 0, "report.csv")?;
# Ok::<(), excelerate::Error>(())
```

## Editing like Excel

```rust,no_run
use excelerate::edit::{self, Axis, CopyOrigin, SortKey, SortOptions};
use excelerate::{Range, Row, reader, writer};

let mut book = reader::read("sales.xlsx")?;

// Two new rows above row 8, formatted like row 7. Every formula in the book
// that pointed at row 8 or below now points two rows lower.
edit::insert_rows_with(&mut book, 0, Row::new(7).unwrap(), 2, CopyOrigin::Before)?;

// Copy the formula in E2 down the column (Ctrl+D), then sort by the header
// "Amount", largest first. The header row stays on top.
edit::fill(&mut book, 0, Range::parse("E2:E500")?, Axis::Rows)?;
let options = SortOptions { header: true, ..SortOptions::default() };
edit::sort_range_with(
    &mut book, 0, Range::parse("A1:E500")?,
    &[SortKey::header("Amount").descending()], options,
)?;

writer::write_xlsx(&book, "sales-sorted.xlsx")?;
# Ok::<(), excelerate::Error>(())
```

[docs/editing.md](docs/editing.md) has the rules each edit follows and where
they come from.

## From JavaScript

The same crate compiled to WebAssembly, for Node and the browser:

```ts
import { Book } from "@rosperitus/excelerate";
import { readFileSync, writeFileSync } from "node:fs";

const book = Book.read(readFileSync("sales.xlsx"));
book.insertRows(0, 8, 2);                              // styled like row 7
book.sortRange(0, "A1:E500", ["-Amount"], { header: true });
book.setRangeStyle(0, "A1:E1", { font: { bold: true } });
book.recalculate();
writeFileSync("sales-sorted.xlsx", book.toXlsx());
```

The whole JS API is in [docs/wasm.md](docs/wasm.md) and the package's
[README](npm/README.md).

## Formats

| Format | Read | Write |
|---|:--:|:--:|
| xlsx (OOXML) | ✅ | ✅ |
| xls (BIFF8) | ✅ | ✅ |
| xls (BIFF5, Excel 5 and 95) | ✅ | — |
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
| [Recipes](docs/recipes.md) | short answers: read a value, convert a file, recalculate one cell |
| [Workbook model](docs/model.md) | workbook, sheet, cell, addresses and ranges |
| [Editing the grid](docs/editing.md) | inserting and removing rows, columns and cells, formatting what you insert, copying, moving, sorting and filling, and what follows each edit |
| [Sheet features](docs/sheet-features.md) | merges, comments, tables, protection, filters, validation, print setup |
| [File formats](docs/formats.md) | what each format carries and what it drops |
| [Formulas](docs/formulas.md) | evaluation, incremental recalc, driving the engine yourself |
| [Styles and number formats](docs/styles.md) | fonts, fills, borders, format strings |
| [Long operations](docs/long-operations.md) | progress, custom functions, what each step costs |
| [Links to other workbooks](docs/external-links.md) | `[1]Sheet1!A1`, the value cache, plugging in a live file |
| [WebAssembly](docs/wasm.md) | building for Node and the browser, and the whole JS API |

## Building

```text
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
tools/build-npm.sh && (cd npm && npm install && npm test && npm run typecheck)
```

## Status

Formats, styles, the formula engine and the xlsx object model (protection,
autofilters, validation, conditional formatting, print setup, tables, charts,
pictures, shapes, comments, pivot tables) are in. xls keeps formulas and styles both ways. See [docs/formats.md](docs/formats.md) for the exact line
between "modelled" and "carried".

## License

MIT - see [LICENSE](LICENSE).
