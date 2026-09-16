# excelerate-reader

Read spreadsheets in Node or the browser - xlsx, xls, ods, csv, html, SYLK,
Gnumeric and SpreadsheetML 2003 - and read how their cells are painted. Rust
compiled to WebAssembly, no native module to build and nothing to install
alongside it.

This is the reading half of [excelerate](https://www.npmjs.com/package/excelerate).
The full package also writes files back out and carries a 443-function formula
engine; if you need either, use that one. Here they are not compiled in at all,
which is the point: a reader that cannot write cannot be made to write, and the
binary is the smaller for it.

```js
const { readFileSync } = require("node:fs");
const { Book } = require("excelerate-reader");

// The format is worked out from the bytes; the name only settles what they
// cannot say themselves.
const book = Book.read(readFileSync("report.xlsx"), "report.xlsx");

for (const name of book.sheetNames()) {
  const sheet = book.sheetIndex(name);
  console.log(name, book.usedRange(sheet));
}

// A cell's value, the text Excel shows for it, and the formula behind it.
book.get(0, "B2");           // 1234.5
book.getFormatted(0, "B2");  // "1 234,50 ₽"
book.getFormula(0, "B2");    // "SUM(B3:B9)" - the text, not its answer

// A whole rectangle in one crossing of the wasm boundary.
book.getRange(0, "A1:D20");

// Or a whole row with its styling, which is one crossing instead of a dozen.
book.getRowAt(0, 3);          // { values, formatted, bold, indent, hidden }
book.getRowAt(0, 3, false);   // the same without the displayed text, which is
                              // a string per cell nobody asked for
```

## Layout

Reading a sheet usually means asking the same few things of every row, so each
has an answer that does not build a whole style to get at one flag:

```js
book.cellBoldAt(0, 3, 1);   // true - the font flag on its own
book.rowHidden(0, 3);       // false
book.mergedRanges(0);       // ["A1:H1", "A2:H2", ...] - what the header spans
book.mergedRangesAt(0);     // the same as numbers: [r1, c1, r2, c2] per area,
                            // one Uint32Array where a big sheet has 300k areas
book.sheetVisibility(0);    // "visible" | "hidden" | "veryHidden"
```

`sheetNames` lists every sheet, hidden ones included: the index every other
call takes is the sheet's position in the workbook, and skipping the hidden
ones would shift it.

## Styles

`cellStyle` answers with the number format, font, fill, borders and text
placement, as the file states them. A cell the file says nothing about answers
with Excel's own defaults, which is what Excel shows for it.

```js
const style = book.cellStyle(0, "A1");
style.font.bold;          // true
style.font.size;          // 14 - points, not the hundredths the file stores
style.fill.foreground;    // "#FFFFE699"
style.borders.bottom;     // { style: "thin", color: "#FF000000" }
style.alignment.wrapText; // true
style.numberFormat;       // "#,##0.00"
```

Colours come as text: `#AARRGGBB` when the file names one outright, `null`
where it leaves the choice to the reader, and `indexed:N` or `theme:N` when it
points into the legacy palette or the workbook theme instead. A theme colour
carries its tint as `theme:4@-0.25`. The two indirect forms are not resolved
for you - an indexed colour needs the palette of the file it came from, and a
theme colour needs the workbook theme, which the reader keeps but does not
interpret.

## Formulas

A formula's **text** is here (`getFormula`), and so is the value the file was
saved with - `get` on a formula cell answers with the cached result. What is
not here is the engine that would recompute it: `evaluate`, `recalculate` and
`recalculateFrom` exist only in the full package.

That cached value is worth exactly what the application that wrote the file is
worth. A package with no `xl/calcChain.xml` was not last saved by Excel, and
its cached values may be stale.

## Reading a package that expands a long way

`Book.read(bytes, name, maxExpanded)` takes a ceiling on how far a zipped
package may unpack, which is what stops a zip bomb. It defaults to 512 MB; a
real workbook can outgrow that - a 100 MB package unpacking to 560 MB is not
unusual - so raise it when you know where the file came from.

## What is not here

Writing, in any format. Recalculation. Everything else the full package can do
with a workbook once it is open - the reading side of it - is.

MIT.
