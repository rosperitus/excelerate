# File formats

Every format is somebody's compromise. Here is what each one carries, what it
drops on the floor, and where the sharp edges are.

## Detection

`reader::read` and `read_bytes` look at the bytes first:

| Signature | Format |
|---|---|
| `PK...` | xlsx, xlsb or ods - the parts inside decide |
| `D0 CF 11 E0` | xls (compound file) |
| `1f 8b` | Gnumeric (gzip) |
| `ID;P` | SYLK |
| `<` | SpreadsheetML 2003 or HTML |

xlsx and xlsb are the same zip with the same part names, so neither gives
itself away in its first bytes; the archive's listing does - `xl/workbook.bin`
means xlsb.

Only when the bytes stay quiet does the extension get a say, and text with an
unknown extension is read as CSV. An extension that lies loses to the
signature - which is what you want when a browser saved `report.xls` that is
really HTML.

```rust
use excelerate::reader::{Format, format_of};

assert_eq!(format_of("tests/corpus/test1.xlsx")?, Format::Xlsx);
# Ok::<(), excelerate::Error>(())
```

## xlsx - the full-fat one

Read *and* written: values, types, formulas (shared ones are expanded), shared
strings, merges, styles, row/column sizing, sheet views and freeze panes, data
validation, the active tab, print setup, headers and footers, page breaks,
hyperlinks, defined names, conditional formatting with `dxf`, the theme, sheet
and workbook protection, autofilters, and the cached values of linked
workbooks.

Charts are modelled and written back: an untouched chart as its original bytes,
a changed or new one from the model (see [model.md](model.md#charts)). The
2016 chart types are written the same way, a change spliced into their part.
Pivot tables and their caches are written by comparison as well (see
[model.md](model.md#pivot-tables)).

Pictures and shapes are modelled the same way: an untouched one goes back as
written, an edit is spliced into its drawing (see [model.md](model.md#pictures)
and [model.md](model.md#shapes)). A comment's box lives in a VML part that is
carried as bytes and brought in line with the comments on write: a new comment
gets a box, a removed one loses it.

Not modelled, but carried through byte for byte with their relationships:
connectors and other drawing objects, document properties, links to other
workbooks.

Two deliberate calls:

- `calcChain.xml` is dropped. It states the order formulas were evaluated in;
  after a rewrite that would be fiction.
- `<fileVersion>` is not written. It names the application that last saved the
  file, and that is not us.

## xls - BIFF8

Reads sheets, every value type, the shared string table (including
continuation records), the whole cell format - number format, font, fill,
borders, alignment and protection - merges, column widths, row heights and the
workbook epoch. Writes the same back.

Colours in BIFF8 are indexes into a 56-entry palette. Reading resolves them to
RGB through the file's own palette, so a colour means the same thing once it
leaves the file. Writing goes the other way: a colour the default palette has
keeps its entry, one it lacks takes over an entry nothing else uses (and the
file gets a `PALETTE` record), and beyond 56 distinct colours the rest are
drawn with the nearest entry. Theme colours are resolved through the
workbook's theme, tint included.

Formulas come back as text beside their cached result. BIFF8 stores a formula
as tokens, and the reader turns them back into `SUM(A1:A3)`: references
relative and absolute, 3D references across sheets, other workbooks and
add-in functions, array constants, shared formulas (expanded onto every cell,
as xlsx reading does) and defined names. An array formula lives on its first
cell; the rest of its range keeps the values. A formula the reader cannot turn
back - a data table, a token it does not know - keeps only its cached value
rather than a text that says something else.

The writer compiles formulas back into tokens and writes them with their
result, and defined names as `NAME` records. Parentheses Excel needs are put
back from operator precedence, so `(A1+B1)*2` survives, but redundant ones
(`(A1&B1)="x"`) do not: the parsed formula no longer holds them. Functions
newer than the format go out by name with the `_xlfn.` prefix, the way Excel
stores them. What BIFF8 cannot hold is written as its value instead: a
structured reference, a reference past row 65536 or column IV, a string
literal over 255 characters, a function called with more arguments than the
format allows.

BIFF5 - the xls Excel 5 and 95 wrote - is read too, and by the same code: the
records are the same ones in the same order, packed more narrowly. One byte per
character in the workbook's code page rather than UTF-16, one byte per column,
the relative flags of a reference on its row rather than its column, and an
`XF` record of sixteen bytes rather than twenty. A 3D reference names its sheet
inside the token, where BIFF8 points at an `EXTERNSHEET` entry. Two things it
leaves behind: a code page other than 1252, which is read as 1252, and the
formatting runs of an `RSTRING` cell, whose text is kept and whose runs are
not.

Writing is BIFF8 only. BIFF4 and older, and encrypted workbooks, are rejected
rather than read halfway.

## ods - OpenDocument

Both directions: sheets, value types (float, percentage, currency, boolean,
date, time, string), repeated rows and columns, merges, formulas, hyperlinks,
cell styles, named expressions.

Three things behave differently by nature of the format:

- A row's tail is one cell repeated to the end of the sheet, so an empty cell
  that only carries a style is recoverable only if a non-empty cell follows it
  in the same row.
- ODS has no per-cell number format string. Date and time formats are inferred
  from the value on read and re-encoded on write; an arbitrary custom format
  does not survive the loop.
- Theme and indexed palette colours have no equivalent and are lost.

Formula syntax is translated both ways (`A1` <-> `[.A1]`, argument separators,
`COM.MICROSOFT.` prefixes), so a formula written here still parses there.

## CSV

Reading guesses the encoding from the BOM (UTF-8/16, else CP1252), honours a
`sep=` line, and infers the delimiter by scoring candidates across rows. RFC
4180 quoting, with a doubled quote inside a quoted field.

```rust,no_run
use excelerate::reader::{CsvOptions, read_csv_with};

let opts = CsvOptions {
    delimiter: Some(';'),
    contiguous: true,             // skip rows that produced no cell
    preserve_empty_fields: true,  // "" becomes an empty string, not a hole
    ..CsvOptions::default()
};
let book = read_csv_with("export.csv", &opts)?;
# Ok::<(), excelerate::Error>(())
```

Writing takes one sheet, resolves formulas (cache first, otherwise it
evaluates), quotes only where needed, and prints 15 significant digits - the
same precision Excel shows.

## HTML

Writing gives you one table per sheet, workbook styles as `td.styleN` classes,
values rendered through the number-format engine, `colspan`/`rowspan`, widths,
heights, hyperlinks and rich text:

```rust
use excelerate::writer::{HtmlOptions, write_html_to};
# use excelerate::reader;

# let book = reader::read("tests/corpus/test1.xlsx")?;
let mut out = Vec::new();
write_html_to(&book, &mut out, &HtmlOptions {
    sheet: Some(0),      // None writes every sheet
    fragment: true,      // no <html>/<head>, for embedding
})?;
# Ok::<(), excelerate::Error>(())
```

Reading uses a hand-rolled tag scanner rather than a DOM: attributes, entities,
implied end tags (`<tr>` closes `<td>`). A page becomes one sheet - tables give
the grid, `colspan`/`rowspan` become merges, text outside a table (`<p>`,
`<h1>`) gets a cell per block. Inline `style` and the old `bgcolor`/`align`/
`width`/`height` attributes are honoured, as are the `data-*` hints our own
writer emits.

The catch nobody escapes: **a page has no cell addresses.** The grid always
starts at `A1`, so a sheet whose used range began at row 5 comes back shifted
up. Images, comments and document properties are not carried; relative CSS
units (`em`, `%`) and `direction` are ignored.

## xlsb - BIFF12

Read-only. The xlsx package with its XML parts replaced by record streams: a
record is a number, a length and that many bytes, both numbers written seven
bits at a time.

What comes across: values of every type, shared strings, formulas (decompiled
from their tokens, shared and array formulas expanded onto every cell that
uses them), defined names, the full cell style - number format, font, fill,
borders, alignment, protection - row heights and hidden rows, column widths,
merges, the saved view (frozen panes, scroll position, selection), the auto
filter, and sheet visibility. What does not: hyperlinks and data validation
(no workbook to check them against yet), and everything built on parts this
reader does not open - tables, pivot tables, drawings, conditional
formatting.

Four sharp edges:

- **Functions numbered past the BIFF8 table.** xlsb numbers the analysis
  add-in functions and the ones Excel 2007 added (`IFERROR`, `COUNTIFS`,
  `AVERAGEIF`). Those numbers are known; a number outside both tables leaves
  the formula unread and its cached value standing, which is true as far as
  it goes.
- **Structured references** (`Sales[Amount]`) are a token kind of their own.
  One whose table was deleted reads as `#REF!`, the way Excel writes it into
  xlsx; a live one needs the table's name and columns, which means reading the
  table parts, and that is not done yet - such a formula keeps its cached
  value rather than a guess.
- A function Excel itself does not know is written `_xludf.MAXIFS` in xlsx and
  bare in xlsb. Bare is what you get here - this crate's own engine knows
  `MAXIFS` under that name.
- The binary always says what the XML leaves to the default: every row carries
  a height, every fill an automatic foreground, every pane its selection. Those
  are read as the default, so the same workbook in either format gives the
  same model rather than one full of noise.

## SYLK, Gnumeric, SpreadsheetML 2003

Read-only, all three.

- **SYLK** - `C`/`F`/`P` records, R1C1 formulas converted to A1, shared
  formulas, number formats, fonts, borders, column widths.
- **Gnumeric** - gzipped XML: sheets, value types, shared formulas by
  `ExprID`, per-cell formats, merges, sizing.
- **SpreadsheetML 2003** - named styles, `ss:Index` instead of addresses,
  R1C1 formulas, `MergeAcross`/`MergeDown`, widths and heights.

## Round-trip guarantee

`read -> write -> read` is covered by tests for xlsx and the other writable
formats. What that means in practice: if a feature is listed as modelled above,
it comes back identical; if it is listed as carried, the bytes come back
identical; anything else is a bug worth reporting.
