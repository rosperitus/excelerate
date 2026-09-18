# Everything on a sheet that is not a cell

`Worksheet` keeps its grid in one map and everything else in public fields
beside it. Read them, push to them, drop them; the writer puts back what it
finds. This page covers the ones with rules of their own. Drawings - charts,
pictures, shapes - are in [the model page](model.md#charts).

## Merges

A merge is a rectangle, and the value belongs to its top left cell. Excel
keeps whatever sat in the covered cells and shows none of it.

```rust
use excelerate::{CellRef, Range};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# let mut sheet = Worksheet::new("Sheet1")?;

sheet.merges.push(Range::new(
    CellRef::parse("A1")?,
    CellRef::parse("C1")?,
));
# book.add_sheet(sheet)?;
# Ok::<(), excelerate::Error>(())
```

Nothing stops two merges from overlapping here; Excel refuses to open a file
where they do.

## Comments

A note carries its own author. Excel keeps authors in a table and points each
comment at a row of it; writing rebuilds that table from what the comments say,
so there is no list to keep in step.

```rust
use excelerate::CellRef;
use excelerate::model::{Comment, TextRun};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# let mut sheet = Worksheet::new("Sheet1")?;

sheet.comments.insert(
    CellRef::parse("B2")?,
    Comment {
        author: "Отдел продаж".to_owned(),
        text: vec![TextRun {
            text: "Проверить курс на дату отгрузки".to_owned(),
            font: None,   // `Some(DiffFont { .. })` to change part of the font
        }],
    },
);
# book.add_sheet(sheet)?;
# Ok::<(), excelerate::Error>(())
```

The frame a note is drawn in is a shape in the sheet's VML part, which travels
as bytes. Before writing, `writer::comment::prepare` compares that part with
the model: a deleted note has its shape cut out, a new one gets a shape with
Excel's defaults, and a sheet with no VML at all gets a new part. A note moved
through the model loses its frame size and whether the frame was pinned open; a
note moved by a grid edit keeps both.

## Tables

A table is a named range with a header row, and formulas refer to it by name:
`Sales[Amount]` follows the table when rows are inserted into it. The part is
modelled rather than carried, which is what makes that work - see
[Formulas](formulas.md) for the reference syntax.

```rust
# use excelerate::model::{Spreadsheet, Worksheet};
# let book = Spreadsheet::empty();
# let sheet = Worksheet::new("Sheet1")?;
for table in &sheet.tables {
    println!(
        "{} over {} - {} columns, {} header rows",
        table.display_name,
        table.range,
        table.columns.len(),
        table.header_row_count.unwrap_or(1),
    );
}
# Ok::<(), excelerate::Error>(())
```

`header_row_count` and `totals_row_count` are `Option<u32>` rather than `u32`
for the same reason the protection flags are `Option<bool>`: each attribute has
its own default in the schema, and writing out an attribute the file never had
puts a decision in it that nobody made.

## Protection

`SheetProtection` holds sixteen separate refusals, each an `Option<bool>`.
`Some(false)` is a decision the user made; `None` is the attribute being absent,
and the defaults differ between flags, so collapsing them into `bool` would
write things the file never said.

```rust
use excelerate::model::{PasswordHash, SheetProtection};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# let mut sheet = Worksheet::new("Sheet1")?;

sheet.protection = SheetProtection {
    sheet: Some(true),
    format_cells: Some(false),
    ..SheetProtection::default()
};
// SHA-512, a random 16-byte salt, 100,000 spins - what Excel writes.
sheet.protection.password = PasswordHash::new("проба");
# book.add_sheet(sheet)?;
# Ok::<(), excelerate::Error>(())
```

None of this encrypts anything. It is a hash Excel checks before letting the
user edit, and `PasswordHash::verify(password)` checks it the same way, against
SHA-512, SHA-384, SHA-256 or SHA-1. Excel 2019 accepted a sheet locked this
way and unlocked it with the same password.

`PasswordHash::iso(password, salt, spins)` takes your own salt, and
`PasswordHash::legacy(password)` writes the 16-bit verifier Excel 97 used.

## Autofilter

`AutoFilter` is a range and a list of columns; a column's `col_id` is an offset
from the first column of the range, not a column of the sheet. What the filter
hides is not computed: a hidden row is already a property of the row, Excel
writes both, and in someone else's file the two are allowed to disagree.

```rust
# use excelerate::model::{Spreadsheet, Worksheet};
# let sheet = Worksheet::new("Sheet1")?;
if let Some(filter) = &sheet.auto_filter {
    println!("{} with {} column rules", filter.range, filter.columns.len());
}
# Ok::<(), excelerate::Error>(())
```

A rule is one of four shapes rather than a single tagged list, because
`<filters>` and `<top10>` share no attributes: `Values` (literals, blanks, a
date group), `Custom` (one or two comparisons joined by and/or), `Dynamic`
(`today`, `aboveAverage` - a criterion Excel works out afresh) and `Top10`.

## Data validation

```rust
use excelerate::model::{DataValidation, ValidationType};
use excelerate::{CellRef, Range};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# let mut sheet = Worksheet::new("Sheet1")?;

sheet.data_validations.push(DataValidation {
    sqref: vec![Range::new(CellRef::parse("A2")?, CellRef::parse("A99")?)],
    kind: ValidationType::List,
    formula1: "\"да,нет\"".to_owned(),
    allow_blank: true,
    ..DataValidation::default()
});
# book.add_sheet(sheet)?;
# Ok::<(), excelerate::Error>(())
```

One field lies about itself and is kept as the file spells it: `show_drop_down`
suppresses the in-cell arrow when it is `true`. The attribute is named
`showDropDown`, Excel writes `1` to hide the arrow, and correcting the sense
here would flip every file that has it.

## Conditional formats

A rule carries a `DifferentialStyle`, whose every part is an `Option`. A `dxf`
says *what to replace*, so a whole `Font` in its place would impose a name and a
size the file never had.

## Hyperlinks

A link covers a range, not a cell, and points either outside the workbook or at
a place inside it:

```rust
use excelerate::model::{Hyperlink, LinkTarget};
use excelerate::{CellRef, Range};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# let mut sheet = Worksheet::new("Sheet1")?;

let a1 = CellRef::parse("A1")?;
sheet.hyperlinks.push(Hyperlink {
    range: Range::new(a1, a1),
    target: LinkTarget::Outside("https://example.org/report".to_owned()),
    display: None,
    tooltip: Some("Открыть отчёт".to_owned()),
});
# book.add_sheet(sheet)?;
# Ok::<(), excelerate::Error>(())
```

An outside link goes into the sheet's relationships when written; an inside one
(`LinkTarget::Inside`) is an address or a defined name and stays in the sheet.

## Print setup

`margins`, `page_setup`, `print_options`, `header_footer`, `row_breaks` and
`col_breaks` are read and written whole. The print area and the repeating rows
are not here: they are defined names on the workbook, `_xlnm.Print_Area` and
`_xlnm.Print_Titles`.

## The saved view

`SheetView` holds zoom, the view mode, the top left cell, the selection and the
frozen panes; `Spreadsheet::active_sheet` holds which tab opens. Without these a
rewritten file opens in a different state from the one it was saved in, which is
what they are read for.

## Sheet visibility

```rust
use excelerate::model::SheetVisibility;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut sheet = Worksheet::new("Sheet1")?;
sheet.visibility = SheetVisibility::VeryHidden;   // not in the unhide dialog
# Ok::<(), excelerate::Error>(())
```

`Spreadsheet::sheets()` lists hidden sheets too. The index every other call
takes is the sheet's position in the workbook, and skipping hidden ones would
shift it.
