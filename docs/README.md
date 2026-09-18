# Documentation

| Page | What's in it |
|---|---|
| [Getting started](getting-started.md) | install, read a file, write one, the whole loop |
| [Recipes](recipes.md) | short answers: read a value, convert a file, recalculate one cell |
| [Workbook model](model.md) | workbook, sheet, cell, addresses and ranges |
| [Editing the grid](editing.md) | inserting and removing rows, columns and sheets, and what moves with them |
| [Everything on a sheet that is not a cell](sheet-features.md) | merges, comments, tables, protection, filters, validation, print setup |
| [File formats](formats.md) | what each format carries and what it drops |
| [Formulas](formulas.md) | evaluation, incremental recalc, driving the engine yourself |
| [Styles and number formats](styles.md) | fonts, fills, borders, format strings |
| [Progress, custom functions and big files](long-operations.md) | the `*_with` pairs, what each step costs, threads |
| [Links to other workbooks](external-links.md) | `[1]Sheet1!A1`, the value cache, plugging in a live file |
| [WebAssembly](wasm.md) | building for Node and reading a book from JS |

Every Rust example on these pages is compiled by `cargo test`: `Documentation`
in `src/lib.rs` pulls the pages in under `cfg(doctest)`, so an example that
goes stale fails the build rather than the reader.

Runnable programs live in [`examples/`](../examples), and double as API
documentation that has to compile:

| Example | What it shows |
|---|---|
| `build` | a workbook from nothing: styles, widths, merges, formulas, a table, a note, a link, a sheet password |
| `objects` | an inventory of everything on a sheet that is not a cell |
| `edit` | inserting and removing rows and columns, and what that did to the formulas |
| `custom` | functions of your own and a progress callback |
| `convert` | read one format, write another |
| `recalc`, `parse_all`, `why`, `xcheck` | the engine against the cache the file carries, and against another engine |
| `shapes`, `roundtrip`, `probe`, `probe_check` | shapes, the read/write/read invariant, probes for a real Excel |

```text
cargo run --release --example convert   -- input.xlsx output.ods
cargo run --release --example recalc    -- workbook.xlsx
cargo run --release --example parse_all -- workbook.xlsx
cargo run --release --example why       -- workbook.xlsm 'Sheet1!AD14'
```
