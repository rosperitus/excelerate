# Documentation

| Page | What's in it |
|---|---|
| [Getting started](getting-started.md) | install, read a file, write one, the whole loop |
| [Workbook model](model.md) | workbook, sheet, cell, addresses and ranges |
| [File formats](formats.md) | what each format carries and what it drops |
| [Formulas](formulas.md) | evaluation, incremental recalc, driving the engine yourself |
| [Styles and number formats](styles.md) | fonts, fills, borders, format strings |
| [Links to other workbooks](external-links.md) | `[1]Sheet1!A1`, the value cache, plugging in a live file |
| [WebAssembly](wasm.md) | building for Node and reading a book from JS |

Runnable examples live in [`examples/`](../examples):

```text
cargo run --release --example convert   -- input.xlsx output.ods
cargo run --release --example recalc    -- workbook.xlsx
cargo run --release --example parse_all -- workbook.xlsx
```

`PLAN.md` in this folder is the internal porting plan - roadmap and design
notes, not user documentation.
