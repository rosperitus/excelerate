# Editing the grid

Setting a cell is one call and touches one cell:

```rust
use excelerate::model::{Spreadsheet, Worksheet};
use excelerate::CellRef;

let mut book = Spreadsheet::empty();
let mut sheet = Worksheet::new("Sheet1")?;
sheet.set(CellRef::parse("B2")?, 42.0);
book.add_sheet(sheet)?;
# Ok::<(), excelerate::Error>(())
```

Inserting a row is not. Everything that named a cell below the insertion point
now names the wrong cell, and those names are spread over the whole workbook:
formulas on other sheets, defined names, merges, hyperlinks, data validations,
conditional formats, protected ranges, the autofilter, table ranges, chart
series, drawing anchors, page breaks. `excelerate::edit` moves them all.

## The four grid edits

```rust
use excelerate::edit::{insert_rows, insert_columns, remove_rows, remove_columns};
use excelerate::{Col, Row};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;

// Three rows before row 5, on sheet 0. Rows are 0-based here, and
// `Row::new` answers `None` past the last row of a sheet.
let row = Row::new(4).unwrap();
insert_rows(&mut book, 0, row, 3)?;
remove_rows(&mut book, 0, row, 3)?;

let column = Col::new(1).unwrap();
insert_columns(&mut book, 0, column, 1)?;
remove_columns(&mut book, 0, column, 1)?;
# Ok::<(), excelerate::Error>(())
```

`Row::new` and `Col::new` count from zero, the way the model does. `Row(4)` is
the row a user calls 5.

## What happens to references

These rules are Excel's, and `tests/edit.rs` holds one test each.

**`$` does not protect anything.** It says a reference will not shift when the
*formula* is copied, which is a different question. A row inserted above the
cell moves the cell, so `$A$5` becomes `$A$6`.

**A deleted cell gives `#REF!`; a range that loses part of itself is
narrowed.** Delete row 5 and `=A5` becomes `=#REF!`, while `=SUM(A1:A10)`
becomes `=SUM(A1:A9)`. Insert inside a range and the range grows.

**The sheet qualifier survives the break.** `Data!A3` whose row is deleted
becomes `Data!#REF!`, not a bare `#REF!`. Excel writes it the same way.

**The cached result is dropped.** It was the answer to the old text. Recalculate
after an edit, or read a stale number later.

## Moving and copying a block

Copying and moving differ in exactly one way, the way Excel's Ctrl+C and
Ctrl+X differ, and it is about the formulas:

```rust
use excelerate::edit::{copy_range, move_range};
use excelerate::{CellRef, Range};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Data")?)?;
let from = Range::parse("A1:B3")?;

// A copy is rewritten as if it had been written where it lands: `=A1` one
// column right reads `=B1`, `$A$1` stays where it is.
copy_range(&mut book, 0, from, 0, CellRef::parse("D1")?)?;

// A move keeps its answers - `=A1` still reads A1 - and the formulas
// elsewhere that read the moved cells follow them instead.
move_range(&mut book, 0, from, 0, CellRef::parse("A10")?)?;
# Ok::<(), excelerate::Error>(())
```

Both carry the values, the styles and the merges lying wholly inside the
block, both may overlap their own source, and both empty a target cell the
source had nothing in - which is what pasting a block does. A block that would
land off the sheet is an error rather than a silent clamp.

Two limits worth knowing. A move to another sheet qualifies what the moved
formulas read without naming a sheet, so `=Z9` becomes `=Data!Z9` - right, but
spelled out per reference, so a range reads `Data!A1:Data!A3`. And a move to
another sheet leaves the references *from elsewhere* alone; within one sheet
they follow the cells.

## Inserting cells, not rows

`insert_cells` moves part of a row, which is what Excel's "Insert Cells" does:
only the columns the block spans move, the rest of the sheet stays put.

```rust
use excelerate::edit::{Axis, insert_cells, remove_cells};
use excelerate::Range;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Data")?)?;
let area = Range::parse("A1:A2")?;

insert_cells(&mut book, 0, area, Axis::Rows)?;     // push the cells below down
remove_cells(&mut book, 0, area, Axis::Rows)?;     // pull them back up
insert_cells(&mut book, 0, area, Axis::Columns)?;  // or sideways
# Ok::<(), excelerate::Error>(())
```

A reference travels only when the whole of it travels, which is why this is a
separate edit from `insert_rows` rather than a special case of it. Data that
the push would shove off the end of the sheet is an error; blank cells there
are no loss, and Excel draws the line in the same place.

## Moving a sheet

```rust
use excelerate::edit::move_sheet;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Data")?)?;
# book.add_sheet(Worksheet::new("Report")?)?;
move_sheet(&mut book, 1, 0)?;   // the report opens first now
# Ok::<(), excelerate::Error>(())
```

Formulas do not change: a sheet is named, not numbered. What does change is
every index kept beside them - the active tab, the sheet a defined name
belongs to - and those are renumbered.

## Renaming and removing sheets

`Worksheet::set_title` changes the tab and nothing else. The sheet's name also
lives in the qualifier of every reference to it, anywhere in the workbook, so
renaming through the model alone leaves those references pointing at a name
that no longer exists.

```rust
use excelerate::edit::{remove_sheet, rename_sheet};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Data")?)?;
# book.add_sheet(Worksheet::new("Report")?)?;

rename_sheet(&mut book, 0, "Sales 2026")?;   // `Data!A1` becomes `'Sales 2026'!A1`
remove_sheet(&mut book, 0)?;                 // and now it is `#REF!A1`
# Ok::<(), excelerate::Error>(())
```

Removing a sheet turns references to it into `#REF!A1` - the bang is part of
the error, not a separator after it - and fixes the sheet indices of defined
names. A 3D range `Sheet1:Sheet3!A1` narrows to what is left when an end sheet
goes and is untouched when a middle one does: both ends are still there, the
span just covers one sheet fewer.

Renaming all 38 sheets of a 19,826-formula workbook takes 360 ms; removing one
takes 10 ms.

## What else moves

Drawing anchors move without going through the model: `edit::anchor` rewrites
the row and column numbers inside the carried part and leaves the rest of the
bytes alone. `editAs` decides what an insertion inside an object does -
`twoCell` (the default) drags each corner with its cell, `oneCell` moves the
object whole, `absolute` does not move it. Offsets inside a cell are not grid
indices and are left as they are.

Chart series (`<c:f>`) are rewritten the same way, by `edit::chart`. The part
is full of numbers, so a wider rewrite would corrupt them.

Comment addresses move in the model. Before that they did not move at all, and
a note stayed on the cell it used to describe.

## Editing in a loop

Each call walks the workbook. For a batch, prefer one call over a range to
several calls over single rows, and recalculate once at the end:

```rust
use excelerate::edit::insert_rows;
use excelerate::formula::eval::{Dependencies, recalculate};
use excelerate::progress::Options;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;
use excelerate::Row;

insert_rows(&mut book, 0, Row::new(0).unwrap(), 10)?;   // not ten calls of one row
recalculate(&mut book, None, &Options::default());
# let _ = Dependencies::of(&book);
# Ok::<(), excelerate::Error>(())
```

If you are making a series of edits and recalculating between them, hold the
dependency index rather than rebuilding it each time - see
[Formulas](formulas.md#recalculating-after-an-edit).
