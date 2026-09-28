# Recipes

Short answers to the questions that come up first. Each one compiles; the ones
marked `no_run` need a file on disk.

## Name a cell

There are three ways, depending on where the address comes from:

```rust
use excelerate::{CellRef, at};
# use excelerate::model::Worksheet;
# let mut sheet = Worksheet::new("Sheet1")?;

// Written in your code: checked by the compiler, no `?` needed.
sheet.set(at!("A1"), 123);

// Counted in a loop: row and column from one, as ROW() and COLUMN() count.
for row in 2..=4 {
    sheet.set_at(row, 1, row * 10)?;
}
assert_eq!(sheet.get_at(3, 1).and_then(|c| c.value.as_number()), Some(30.0));

// Arriving at run time: parsed, and a bad one is an `Err`.
let from_user = "B7";
sheet.set(CellRef::parse(from_user)?, "typed in");
assert!(CellRef::parse("B0").is_err());
# Ok::<(), excelerate::Error>(())
```

`at!("XFE1")` does not compile: the column is past XFD, and the compiler says
`not a cell address: XFE1`.

## Colour part of a cell's text

A cell whose formatting changes mid-string holds `RichText`, a list of runs.
Each run names only what it changes about the cell's font; everything else
comes from the cell.

```rust
use excelerate::at;
use excelerate::model::{CellValue, TextRun, Worksheet};
use excelerate::style::{Color, DiffFont};
# let mut sheet = Worksheet::new("Sheet1")?;

let colored = |text: &str, argb: u32| TextRun {
    text: text.to_owned(),
    font: Some(DiffFont {
        color: Some(Color::Argb(argb)),
        ..DiffFont::default()
    }),
};
sheet.set(
    at!("A1"),
    CellValue::RichText(vec![
        colored("Red ", 0xFFFF_0000),
        colored("green ", 0xFF00_B050),
        TextRun { text: "plain".into(), font: None },
    ]),
);
# Ok::<(), excelerate::Error>(())
```

The runs survive xlsx, xls, ODS and HTML. xlsb keeps them on reading only,
since there is no xlsb writer. A theme colour (`Color::Theme { id, tint }`)
goes to ODS and HTML as the RGB it shows.

## Read a value, whatever kind it is

`Worksheet::get` gives you the cell and `CellValue` says what is in it. A
formula cell holds its text and the result the file cached.

```rust
use excelerate::at;
use excelerate::model::CellValue;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# let mut sheet = Worksheet::new("Sheet1")?;
# sheet.set(at!("A1"), 1.0);
# book.add_sheet(sheet)?;
# let sheet = &book.sheets()[0];

match sheet.get(at!("A1")).map(|c| &c.value) {
    Some(CellValue::Number(n)) => println!("number {n}"),
    Some(CellValue::Text(t)) => println!("text {t}"),
    Some(CellValue::Bool(b)) => println!("boolean {b}"),
    Some(CellValue::Error(e)) => println!("error {}", e.as_str()),
    Some(CellValue::Formula { formula, cached }) => {
        println!("={formula}, last computed as {cached:?}");
    }
    Some(CellValue::RichText(runs)) => println!("{} runs", runs.len()),
    Some(CellValue::Empty) | None => println!("nothing"),
}
# Ok::<(), excelerate::Error>(())
```

## Read a number without caring whether it is a formula

`result()` looks through a formula to what it computed, and the `as_*` family
looks through it to one type. A formula nobody has computed yet reads as empty
rather than as an error.

```rust
use excelerate::at;
use excelerate::model::CellValue;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut sheet = Worksheet::new("Sheet1")?;
# sheet.set(at!("A1"), 1.5);

let total: f64 = sheet
    .iter()
    .filter_map(|(_, cell)| cell.value.as_number())
    .sum();
# let _ = total;

// And writing one back, with no result until something computes it:
sheet.set(at!("B1"), CellValue::formula("A1*2"));
# Ok::<(), excelerate::Error>(())
```

## Show a number the way Excel shows it

The value is 45292, the cell shows `01.01.2024`. The difference is the number
format, and `style::format::format` applies one:

For a cell of a workbook you have, `Spreadsheet::formatted` looks up the
cell's own format and the workbook's epoch for you:

```rust
use excelerate::at;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;
println!("{}", book.formatted(0, at!("A1")));
println!("{}", book.formatted_at(0, 1, 1));   // the same cell, by numbers
# Ok::<(), excelerate::Error>(())
```

With a format string of your own, `style::format::format` applies one to a
bare value:

```rust
use excelerate::shared::date::Epoch;
use excelerate::style::format::{Value, format};

assert_eq!(format(Value::Number(45292.0), "DD.MM.YYYY", Epoch::Windows1900), "01.01.2024");
assert_eq!(format(Value::Number(0.5), "0.00%", Epoch::Windows1900), "50.00%");
assert_eq!(format(Value::Number(1234.5), "#,##0.00", Epoch::Windows1900), "1,234.50");
```

Pass the workbook's epoch: a Mac workbook counts from 1904, and reading its
dates as 1900 shifts everything by 1,462 days.

Russian number formats, where the space groups digits and the comma is the
decimal point (`# ##0,00`), are a known gap: the space reads as a literal.
Russian date codes (`ДД.ММ.ГГГГ`) and month names do work.

## Walk a sheet without walking empty cells

`iter` visits what the file holds, in address order, and skips the rest.

```rust
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;
# let sheet = &book.sheets()[0];
for (at, cell) in sheet.iter() {
    let _ = (at, &cell.value);
}
# Ok::<(), excelerate::Error>(())
```

For one row, `row_cells` takes a slice of the map rather than filtering the
sheet. On a 666,000-row sheet the filter cost 70 ms per row, which is thirteen
hours for the sheet; the range costs 9 microseconds.

```rust
use excelerate::Row;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;
# let sheet = &book.sheets()[0];
for (col, cell) in sheet.row_cells(Row::new(0).unwrap()) {
    let _ = (col, &cell.value);
}
# Ok::<(), excelerate::Error>(())
```

## Fill a sheet from records

A row at a time rather than a cell at a time, and the value with the style it
is shown in:

```rust
use excelerate::{CellRef, at};
use excelerate::model::{CellValue, Spreadsheet, Worksheet};
use excelerate::style::Style;

let mut book = Spreadsheet::empty();
let mut sheet = Worksheet::new("Продажи")?;
let mut bold = Style::default();
bold.font.bold = true;
let bold = book.styles.intern(bold);

sheet.set_styled(at!("A1"), "Товар", bold);
sheet.set_styled(at!("B1"), "Сумма", bold);

for (row, (name, sum)) in (2..).zip([("Чай", 180.0), ("Кофе", 350.0)]) {
    let first = CellRef::from_row_col(row, 1)?;
    sheet.set_row(first, [CellValue::text(name), CellValue::Number(sum)]);
}
book.add_sheet(sheet)?;
# Ok::<(), excelerate::Error>(())
```

`set_row` stops at the last column of the sheet rather than wrapping round to
column A.

## Add a row that looks like the one above

```rust
use excelerate::edit::{CopyOrigin, insert_rows_with};
use excelerate::Row;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;

// A new row 10 with row 9's cell styles and height, as Excel's Insert gives.
insert_rows_with(&mut book, 0, Row::new(9).unwrap(), 1, CopyOrigin::Before)?;
# Ok::<(), excelerate::Error>(())
```

## Sort by a column with a header

```rust
use excelerate::edit::{SortKey, SortOptions, sort_range_with};
use excelerate::Range;
# use excelerate::model::{Spreadsheet, Worksheet};
use excelerate::at;
# let mut book = Spreadsheet::empty();
# let mut sheet = Worksheet::new("Sheet1")?;
# sheet.set(at!("B1"), "Amount");
# book.add_sheet(sheet)?;

let header = SortOptions { header: true, ..SortOptions::default() };
let keys = [SortKey::header("Amount").descending()];
sort_range_with(&mut book, 0, Range::parse("A1:D200")?, &keys, header)?;
# Ok::<(), excelerate::Error>(())
```

For a table, `edit::sort_table(&mut book, "Sales", &keys)` finds the range and
the header by itself.

## Number rows, or write out the months

```rust
use excelerate::edit::{Axis, fill_series};
use excelerate::{Range, at};
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;

let sheet = book.sheet_mut(0).unwrap();
sheet.set(at!("A2"), 1.0);
sheet.set(at!("A3"), 2.0);
sheet.set(at!("B1"), "Jan");
fill_series(&mut book, 0, Range::parse("A2:A100")?, Axis::Rows)?;     // 1..99
fill_series(&mut book, 0, Range::parse("B1:M1")?, Axis::Columns)?;    // Jan..Dec
# Ok::<(), excelerate::Error>(())
```

## Find where the data is

```rust
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;
# let sheet = &book.sheets()[0];
match sheet.dimension() {
    Some(used) => println!("{} to {}", used.start, used.end),
    None => println!("the sheet is empty"),
}
# Ok::<(), excelerate::Error>(())
```

## Convert a file

The output format comes from the extension you ask for:

```rust,no_run
use excelerate::{reader, writer};

let book = reader::read("report.xlsb")?;
writer::write_xlsx(&book, "report.xlsx")?;
writer::write_ods(&book, "report.ods")?;
writer::write_html(&book, "report.html")?;
writer::write_csv(&book, 0, "sheet1.csv")?;   // one sheet at a time
# Ok::<(), excelerate::Error>(())
```

xlsb, SYLK, Gnumeric and SpreadsheetML 2003 read but do not write. What each
format drops on the way is in [File formats](formats.md).

## Read a CSV that is not comma-separated

```rust,no_run
use excelerate::reader::{CsvOptions, read_csv_with};

let book = read_csv_with(
    "export.csv",
    &CsvOptions {
        delimiter: Some(';'),
        contiguous: true,            // skip rows that produced no cell
        ..CsvOptions::default()
    },
)?;
# let _ = book;
# Ok::<(), excelerate::Error>(())
```

Left alone, the reader works the delimiter out from how evenly each candidate
splits the lines, honours a leading `sep=;` line, and takes the encoding from
the BOM, falling back to CP1252.

## Write an HTML fragment rather than a page

```rust,no_run
use excelerate::writer::{HtmlOptions, write_html_to};
# use excelerate::model::Spreadsheet;
# let book = Spreadsheet::empty();

let mut out = Vec::new();
write_html_to(
    &book,
    &mut out,
    &HtmlOptions { sheet: Some(0), fragment: true },
)?;
# Ok::<(), excelerate::Error>(())
```

`fragment` leaves out `<html>` and the head, so the table can be dropped into a
page you already have.

## Recalculate one cell, or everything a change reached

Three calls that sound alike and are not:

```rust
use excelerate::formula::eval::{recalculate, recalculate_cell, recalculate_from};
use excelerate::progress::Options;
use excelerate::at;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;

let b4 = at!("B4");
recalculate(&mut book, None, &Options::default());   // the whole workbook
recalculate_cell(&mut book, 0, b4);                  // that cell, and cache it
recalculate_from(&mut book, &[(0, b4)]);             // what reads B4, not B4
# Ok::<(), excelerate::Error>(())
```

`recalculate_cell` answers "compute this one". `recalculate_from` answers "I
have changed this one" and leaves it alone, computing the formulas that read it.

## Read a BIFF5 export that comes out as mojibake

BIFF5 text is bytes, and the page is in the file. Some files have no `CODEPAGE`
record, and some lie:

```rust,no_run
use excelerate::reader::xls::read_xls_in;
use excelerate::shared::codepage::WINDOWS_1251;

let book = read_xls_in("выгрузка.xls", WINDOWS_1251)?;
# let _ = book;
# Ok::<(), excelerate::Error>(())
```

## Open a file whose extension lies

`reader::read` looks at the bytes first, so a `.txt` that is really a zip
package opens as xlsx. The extension only votes when the bytes say nothing.
`read_bytes(bytes, name)` does the same in memory, and the name is optional.

```rust,no_run
use excelerate::reader::{format_of, read};

println!("{:?}", format_of("mystery.txt")?);   // Format::Xlsx, maybe
let book = read("mystery.txt")?;
# let _ = book;
# Ok::<(), excelerate::Error>(())
```

## Check a sheet password without unlocking anything

```rust
use excelerate::model::PasswordHash;

let hash = PasswordHash::new("проба").unwrap();
assert!(hash.verify("проба"));
assert!(!hash.verify("другой"));
```

## Count the formulas that failed to parse

The cached result is still there when a formula does not decompile, so a file
that reads clean is not the same as a file that parsed clean:

```rust
use excelerate::formula::parser::parse;
use excelerate::model::CellValue;
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
# book.add_sheet(Worksheet::new("Sheet1")?)?;

let mut unparsed = 0;
for sheet in book.sheets() {
    for (_, cell) in sheet.iter() {
        if let CellValue::Formula { formula, .. } = &cell.value
            && parse(formula).is_err()
        {
            unparsed += 1;
        }
    }
}
println!("{unparsed} formulas did not parse");
# Ok::<(), excelerate::Error>(())
```

`examples/parse_all.rs` is this with the offending text printed, and
`examples/recalc.rs` compares every result against the cache the file carries.

## Ask why a result disagrees with the cache

```text
cargo run --release --example why -- book.xlsm 'Minmax!AD14'
```

It walks from the cell through the disagreeing cells it reads, down to the
roots whose inputs agree. That is usually where the real difference is.

## Keep unmodelled parts intact

Nothing to do: drawings, comments' VML, document properties, links to other
workbooks and everything they point at travel byte for byte, with their
relationships, recursively. `calcChain` is the one part deliberately dropped -
it describes the order formulas were computed in, and after a rewrite that
would be a lie.
