# Links to other workbooks

A formula written `=[1]Prices!A1+1` reads a cell out of a *different* file.
Spreadsheet apps do not need that file to be open: they keep the last values
they saw and recalculate from those until the link is refreshed. This crate
does the same.

## How `[1]` resolves

```
formula "[1]Prices!A1"
  └─ 1 → the first <externalReference> in workbook.xml   (position, not a name)
        └─ its relationship → xl/externalLinks/externalLink1.xml
              ├─ <sheetNames>   the linked book's sheets
              └─ <sheetDataSet> the cached values, cell by cell
        └─ that part's own .rels → file:///C:/work/prices.xlsx  (where it came from)
```

The number in brackets is a *position*, not a filename. The filename lives one
level deeper and is only there to tell you — and a future refresh — where the
numbers came from.

## Reading the cache

```rust
# use excelerate::reader;
# let book = reader::read("tests/test1.xlsx")?;
for (i, linked) in book.external.iter().enumerate() {
    println!("[{}] {:?}", i + 1, linked.path);
    for sheet in &linked.sheets {
        println!("  {} — {} cached cells", sheet.name, sheet.cells.len());
    }
}
# Ok::<(), excelerate::Error>(())
```

`Spreadsheet::external` is a `Vec<ExternalBook>` in the order the workbook
lists its references, so `[N]` is index `N - 1`.

## Recalculating against it

Nothing extra to switch on — the engine reads links out of that cache:

```rust
use excelerate::formula::eval::recalculate;
use excelerate::progress::Options;
# use excelerate::reader;

# let mut book = reader::read("tests/test1.xlsx")?;
recalculate(&mut book, None, &Options::default());   // formulas using [1]Sheet!A1 get real numbers
# Ok::<(), excelerate::Error>(())
```

Rules of the road:

| Situation | Result |
|---|---|
| cell is in the cache | its value |
| cell is not cached | blank — same as the app shows before a refresh |
| no such sheet, or no such link | `#REF!` |
| link written by name (`[prices.xlsx]Sheet1!A1`) | `#REF!` — see below |

The cached parts also ride through a write untouched, so saving a book does not
break its links.

## Plugging in a live file

When you *do* have the other file, fill the same slot from it and recalculate.
The engine does not care where the values came from:

```rust
use excelerate::model::{CellValue, ExternalBook, ExternalSheet, Spreadsheet};

/// Refresh link `[index + 1]` of `book` from an open workbook.
fn link(book: &mut Spreadsheet, index: usize, source: &Spreadsheet) {
    let sheets = source
        .sheets()
        .iter()
        .map(|s| ExternalSheet {
            name: s.title().to_owned(),
            cells: s
                .iter()
                .map(|(at, cell)| {
                    // A link carries values, never formulas.
                    let value = match &cell.value {
                        CellValue::Formula { cached, .. } => {
                            cached.as_deref().cloned().unwrap_or(CellValue::Empty)
                        }
                        other => other.clone(),
                    };
                    ((at.row, at.col), value)
                })
                .collect(),
        })
        .collect();

    match book.external.get_mut(index) {
        Some(slot) => slot.sheets = sheets,
        None => book.external.push(ExternalBook { path: None, sheets }),
    }
}
```

```rust
# use excelerate::{reader, formula::eval::recalculate, progress::Options};
# fn link(_: &mut excelerate::model::Spreadsheet, _: usize, _: &excelerate::model::Spreadsheet) {}
let mut prices = reader::read("prices.xlsx")?;
recalculate(&mut prices, None, &Options::default());          // source first

let mut report = reader::read("report.xlsx")?;
link(&mut report, 0, &prices);           // [1] is now the real file
recalculate(&mut report, None, &Options::default());
# Ok::<(), excelerate::Error>(())
```

Three things to keep in mind:

- **Ordering is yours.** There is no cross-workbook dependency graph, on
  purpose — a cycle `A → B → A` is resolved by iterating, not by topology, in
  every spreadsheet app too.
- **It is a copy.** Later edits to `prices` are invisible to `report` until you
  call `link` again.
- **Memory.** The snippet above copies the whole source. On a 200k-cell book
  that shows up; copy only the sheets the links actually mention if it bites.

## Known limits

- Only the `[N]` form resolves. A link spelled by filename —
  `[prices.xlsx]Sheet1!A1` or `'C:\work\[prices.xlsx]Sheet1'!A1` — parses but
  yields `#REF!`. Files written by Excel always use the number; the named form
  shows up in hand-written formulas and files from other generators.
- **xls does not do this at all.** In BIFF8 a formula is a token tree, and the
  reader keeps only its cached result — there is no formula text to hold a
  `[1]`. Linked-book support there waits on a formula decompiler.
- Refreshing the cache back into the file on save is not implemented: the parts
  are written exactly as they arrived.
