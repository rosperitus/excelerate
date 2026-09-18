# Progress, custom functions and big files

Reading a 70 MB package takes 5.4 seconds and writing it back takes 7.3. A
recalculation of 651,614 formulas takes 6.9 seconds across eight threads.
Anything on that scale wants a progress bar, and some workbooks call functions
this crate does not define. Both arrive through `progress::Options`.

## The `*_with` pairs

Every long operation has a twin that takes `Options`:

| Plain | With options |
|---|---|
| `reader::read_xlsx_from` | `reader::read_xlsx_from_with` |
| `reader::read_bytes_limited` | `reader::read_bytes_limited_with` |
| `writer::xlsx::write_xlsx_to` | `writer::xlsx::write_xlsx_to_with` |
| `formula::eval::recalculate_from` | `recalculate_from_with` |
| `formula::eval::recalculate_cell` | `recalculate_cell_with` |

`recalculate` is the exception: it takes `Options` as its third argument and has
no twin. A full recalculation is the one operation where progress is the normal
case rather than the rare one, so the caller who does not want it writes
`&Options::default()` instead of everybody else having to remember a second
name.

## Drawing a bar

```rust
use excelerate::progress::{Options, Progress, Stage};
use std::cell::Cell;

let done = Cell::new(0usize);
let report = |p: Progress<'_>| {
    done.set(p.done);
    match p.fraction() {
        Some(f) => println!("{:?} {:.0}%: {}", p.stage, f * 100.0, p.what),
        None => println!("{:?} {}: {}", p.stage, p.done, p.what),
    }
};
let options = Options::new().reporting(&report);
# let _ = (options, Stage::Reading, done.get());
```

The callback is `Fn`, not `FnMut`: the engine holds the same `Options` while it
borrows it. Anything you accumulate goes in a `Cell` the closure captures, which
is the usual shape for a progress bar anyway.

The unit differs by stage. Reading counts sheets, writing counts parts of the
package, recalculating counts formulas. `total` is `Option` because reading does
not know it until the workbook part has been read, so `fraction()` returns
`None` early on.

## Lending the engine a function

```rust
use excelerate::formula::custom::CustomFunctions;
use excelerate::formula::value::Value;
use excelerate::progress::Options;

let mut functions = CustomFunctions::new();
functions.register("СНДС", |args| match args.first() {
    Some(Value::Number(n)) => Value::Number(n * 1.2),
    _ => Value::Error(excelerate::error::CellError::Value),
});
let options = Options::new().with_functions(&functions);
# let _ = options;
```

Arguments arrive already computed, and a range arrives as `Value::Array`. Names
are matched without regard to case.

**A built-in name stays with the built-in function.** A workbook where `SUM`
means something private is a workbook nobody else can read, so registering `SUM`
does not take the name. An unregistered name answers `#NAME?`, which is exactly
what Excel shows with macros switched off.

## Reading a package that expands past the cap

`MAX_UNCOMPRESSED_SIZE` is 512 MB, and it is there to stop a zip bomb. Real
workbooks outgrow it: a 100 MB package expands to 560 MB. So it is a parameter
rather than a constant in the way:

```rust,no_run
use excelerate::reader::read_bytes_limited;

let bytes = std::fs::read("huge.xlsx")?;
let book = read_bytes_limited(&bytes, Some("huge.xlsx"), 4 << 30)?;
# let _ = book;
# Ok::<(), Box<dyn std::error::Error>>(())
```

A package is also allowed through when it expands no more than a hundred times
its compressed size, whatever the cap says. The ratio is what tells a real
workbook from a bomb: the 70 MB export that expands to 634 MB compresses nine
times, and a bomb compresses thousands.

## What costs what

Measured on a 67 MB workbook of 9,000,602 cells on one sheet, release build:

| Step | Time | Peak RSS |
|---|---|---|
| read | 5.4 s | 754 MB |
| write xlsx | 7.3 s | 1.11 GB |

Around 124 bytes per cell after reading. Deflate is 32% of the whole run, all
of it in the write; the zip level is the one knob that moves that number.

For formulas, the shape of the call matters more than the size of the book:

| Call | On a 19,811-formula workbook |
|---|---|
| `recalculate` | 70 ms |
| `Dependencies::of` | 16 ms |
| 50 edits in one batch | 13 ms for the 184 formulas they reached |
| the same 50 edits one at a time | 263 ms |

Batch the edits, keep the index. See
[Formulas](formulas.md#recalculating-after-an-edit).

## Threads

A full recalculation runs in waves across `min(cores, 8)` threads, from 20,000
formulas up. It stays single-threaded where the caller registered custom
functions - they are not required to be `Sync` - and under wasm. On a 651,614
formula workbook that is 6.9 seconds against 13.5 single-threaded. Twelve
threads gave 6.6 s and sixteen gave 7.7, so the ceiling stayed at eight.
