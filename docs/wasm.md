# WebAssembly

The crate builds for `wasm32-unknown-unknown` with a thin JS wrapper, so the
same reader that runs on your server runs in Node or a browser tab.

## Install

```text
npm install @rosperitus/excelerate          # everything
npm install @rosperitus/excelerate-reader   # reading only, 1.0 MB of wasm instead of 2.3
```

TypeScript declarations ship with the package - `wasm-bindgen` derives them
from the Rust signatures, so they never drift from the API.

## Build it yourself

```text
./build-npm.sh            # nodejs target, release, into npm/pkg
./build-npm.sh web        # or bundler
```

The result is a publishable npm package: wasm, the JS glue, `.d.ts`, and
`npm/README.md` as its readme.

## Use it

```ts
import { Book } from "@rosperitus/excelerate";
import fs from "node:fs";

// Any supported format - the bytes decide. The name is optional and only
// settles ambiguous cases.
const book = Book.read(fs.readFileSync("report.xlsx"), "report.xlsx");

console.log(book.sheetNames());        // ["Sheet1", "Data"]
console.log(book.cellCount(0));        // non-empty cells on sheet 0
console.log(book.get(0, "B4"));        // number | string | boolean | null

book.set(0, "B4", 42);
book.recalculateFrom(0, "B4");         // only what depends on B4

fs.writeFileSync("out.xlsx", book.toXlsx());
book.free();     // wasm memory is not the JS heap; let it go when you are done
```

Runnable TypeScript examples live in
[`npm/typescript/`](../npm/typescript): building a workbook, reading any
format, batching edits, painting a report, taking an inventory of a sheet's
charts, pictures, shapes, tables and notes, and a progress bar with a function
of your own. Run them from `npm/` - `node typescript/basic.ts`
on Node 22.6+, no build step and no separate install.

## Objects and chains

Over the flat `Book` sit `Workbook`, `Sheet`, `Table` and `SheetRange`, whose
changing calls return themselves:

```ts
import { Workbook } from "@rosperitus/excelerate";

const wb = new Workbook();
const sales = wb.addSheet("Sales")
  .addTableFromData("Sales", "A1", [["Region", "Amount"], ["North", 120], ["South", 340]])
  .set(1, "Amount", 125)
  .addFilter("Region", { values: ["South"] });
sales.records({ visible: true });   // [{ Region: "South", Amount: 340 }]
```

| Object | Has |
|---|---|
| `Workbook` | every `Book` method that takes no sheet (`definedNames`, `setDefinedName`, `moveSheet`, `registerFunction`, `protectWorkbook`...), plus `new Workbook()`, `Workbook.read(bytes)`, `Workbook.readCsv`, `sheet(index or name)`, `sheets()`, `addSheet(name)` (a new workbook's first call takes over its empty `Sheet1`), `recalculate()`, `toXlsx()`, `book` |
| `Sheet` | every `Book` method that takes the sheet first, without it (`setColumnWidth(2, 14)`, `getRange("A1:C3")`, `insertRows(8, 2)`...), plus `text`, `write(at, rows)`, `range(address)`, `style(address, patch)`, `width(column, chars)`, `freeze(rows, cols)`, `addTable`, `addTableFromData(name, at, rows)`, `table(name)`, `tables()`, `rename`, `activate`, `recalculate`, `remove` |
| `Table` | `set(row, column, value)` and `get` by data row and header, `addFilter(column, criteria)`, `clearFilter(column?)`, `sort(keys)`, `range()`, `data()`, `records({ visible? })`, `columns` |
| `SheetRange` | `values()`, `visibleValues()`, `set(rows)`, `style(patch)`, `merge()` |

A `Book` method that answers with nothing returns the object, so it chains;
the rest return their answer. The types are derived from `Book`'s own, so a
method `Book` gains is typed on `Sheet` too, and a test fails until it is
listed.

They are handles - a sheet index, a table name, an address - over
`workbook.book`; the only package without them is `excelerate-reader`, since
most of what they do writes.

## The API

| Method | Does |
|---|---|
| `new Book()` | empty workbook with one sheet |
| `Book.read(bytes, name?, maxExpanded?, onProgress?)` | read any supported format |
| `sheetNames()` / `sheetIndex(name)` | sheet titles; the index of one, case-insensitively |
| `addSheet(title)` / `renameSheet(sheet, title)` | append a sheet; rename one |
| `activeSheet()` / `setActiveSheet(sheet)` | the tab a reader opens on |
| `cellCount(sheet?)` / `usedRange(sheet)` | how many cells; the rectangle they sit in |
| `get(sheet, address)` | a cell's value as `CellValue` (`number \| string \| boolean \| null`) |
| `set(sheet, address, value)` | write a number, string, boolean, or `"=FORMULA"` |
| `clear(sheet, address)` | empty a cell |
| `getFormula(sheet, address)` | its formula text, or `undefined` for a plain value |
| `getFormatted(sheet, address)` | the value through its number format, as displayed |
| `getRange(sheet, "A1:C9")` / `setRange(sheet, "A1", grid)` | a rectangle in one crossing |
| `getRangeStyles(sheet, range)` / `setRangeStyles(sheet, at, styles)` | a rectangle's formatting in one crossing: `{ styles, grid }`, each distinct style once and a grid of indexes into it; what `get` returns, `set` takes back |
| `getAt` / `setAt` / `clearAt` / `getFormulaAt` / `getFormattedAt` / `cellIndentAt` | the same cell operations by 1-based row and column |
| `getRangeAt(sheet, row, col, rows, cols)` / `setRangeAt(sheet, row, col, grid)` | a rectangle by numbers |
| `getRangeStylesAt` / `setRangeStylesAt` | its formatting by numbers |
| `recalculateCellAt` / `recalculateFromAt(sheet, row, column)` | recalculate a cell, or from a cell, named by numbers |
| `cellIndent(sheet, address)` | the cell's indent steps, 0 when it has none |
| `rowLevel(sheet, row)` / `columnLevel(sheet, "C")` | outline depth of a row or column, 0 when ungrouped |
| `cellBold(sheet, address)` / `cellBoldAt(sheet, row, col)` | whether the cell is bold, without building the rest of its style |
| `getRowAt(sheet, row, formatted?)` | a whole row in one crossing: `{ values, formatted, bold, indent, hidden }`; `formatted: false` skips the displayed text and its string per cell |
| `rowHidden(sheet, row)` | whether the row is hidden |
| `sheetVisibility(sheet)` | `"visible"`, `"hidden"` or `"veryHidden"` |
| `mergedRanges(sheet)` | the sheet's merged areas as `"A1:C1"` strings |
| `mergedRangesAt(sheet)` | the same areas as one `Uint32Array`, four numbers each: `[r1, c1, r2, c2]` |
| `usedRangeHint(sheet)` | the used range without ever walking the rows; columns may be wider after a removal |
| `columnWidth(sheet, column)` / `rowHeight(sheet, row)` | the size the sheet gives them, `undefined` when it leaves it to the default |
| `setColumnWidth` / `setRowHeight` / `setColumnHidden` / `setRowHidden` | write those; `undefined` for a size gives it back to the default |
| `setSheetVisibility(sheet, state)` | `"visible"`, `"hidden"` or `"veryHidden"` |
| `merge(sheet, "A1:C1")` / `unmerge(sheet, range)` | merge a block of cells, or take the merge back out |
| `insertRows` / `removeRows` / `insertColumns` / `removeColumns` | edit the grid; formulas across the workbook follow. An insert takes `copyOrigin` last: `"before"` (the default, as in Excel), `"after"` or `"none"` |
| `insertRowsMany(sheet, [[at, count], ...], copyOrigin?)` / `removeRowsMany` / `insertColumnsMany` / `removeColumnsMany` | the same at many places in one walk of the workbook; places are numbered as before the call, in any order |
| `copyRange(sheet, range, to, toSheet?)` / `moveRange(...)` | a block of cells: a copy rewrites its formulas, a move keeps them and drags the references to it along |
| `insertCells(sheet, range, "down" \| "right", copyOrigin?)` / `removeCells(sheet, range, "up" \| "left")` | Excel's "Insert Cells": part of a row moves, the rest of the sheet stays |
| `sortRange(sheet, range, keys, { header?, byColumns? })` | Data - Sort. A key is a column number (`2`) or a header (`"Amount"`); a minus sorts largest first |
| `sortTable(name, keys)` | sort a table's data rows by its column names; header and totals stay put |
| `fillDown(sheet, range)` / `fillRight(sheet, range)` | Ctrl+D / Ctrl+R: the first row or column copied over the rest |
| `fillSeries(sheet, range, "down" \| "right")` | the fill handle: `1, 3` → `5, 7`, `Кв1` → `Кв2`, `Jan` → `Feb`, a date by a day |
| `moveSheet(from, to)` | reorder the tabs; every sheet index moves with them |
| `removeSheet(sheet)` | drop a sheet - references to it become `#REF!` |
| `comments(sheet)` / `hyperlinks(sheet)` / `tables(sheet)` | what the sheet carries besides cells |
| `charts(sheet)` / `shapes(sheet)` / `images(sheet)` | the drawing objects, each with its anchor; `imageData(sheet, i)` for a picture's bytes. A shape carries `rotation` (degrees), `flipH`, `flipV` and the `fill`, `line` and `font` Excel shows, its style filling in what it leaves out; a chart carries `format` and `plotFormat`, the fill and outline of its chart and plot area. A drawing colour comes resolved through the workbook theme as `#AARRGGBB` |
| `setShapeFormat(sheet, i, { fill?, line?, font?, rotation?, flipH?, flipV? })` / `setChartFormat(sheet, i, { format?, plotFormat? })` | change that look; a field left out stays, `null` gives a fill or line back to the style. A colour to write is `#RRGGBB`, `#AARRGGBB` or a theme name such as `accent1`, which follows the theme |
| `cellStyle(sheet, address)` / `cellStyleAt` | the whole style: `numberFormat`, `font`, `fill`, `borders`, `alignment`; a colour is `#AARRGGBB`, `indexed:N`, `theme:N` or `null` |
| `getRichText(sheet, address)` / `getRichTextAt` | formatted text as runs, `{ text, font }` each, `font` naming only what the run changes; `null` for a cell without it |
| `setRichText(sheet, address, runs)` / `setRichTextAt` | write formatted text in the same shape |
| `setCellStyle(sheet, address, patch)` / `setCellStyleAt` / `setRangeStyle` | paint a cell or a rectangle: number format, font, fill, borders, alignment. A patch is laid over what the cell had |
| `setComment` / `removeComment` | put a note on a cell, take it off |
| `setHyperlink` / `removeHyperlink` | link a cell or a block of them |
| `addTable(sheet, name, range, headerRow?)` / `removeTable` | draw a table, the thing `Sales[Amount]` names |
| `setTableFilter(name, column, criteria \| null)` | filter a table by a column (its header or number from 1): `{ values }`, `{ custom: [{ op, value }] }`, `{ top }` or `{ bottom }`. The criteria go into the file and the rows they reject are hidden there, as Excel does |
| `sheetView(sheet)` | how the sheet is frozen and shown |
| `freezePanes(sheet, rows, columns)` | pin the header row and the first columns; `(sheet, 0, 0)` unfreezes |
| `setZoom(sheet, percent?)` / `setShowGridLines(sheet, show, headers?)` | how a reader opens it |
| `definedNames()` / `setDefinedName(name, formula, sheet?)` / `removeDefinedName` | the names a formula can use |
| `documentProperties()` / `setDocumentProperties(patch)` | title, author, dates, company and the user's own fields; the patch sets what it names, `null` clears, `custom` replaces the list |
| `dataValidations(sheet)` / `conditionalFormats(sheet)` / `autoFilter(sheet)` | the rules over a sheet |
| `pivotTables(sheet)` / `arrayFormulas(sheet)` / `externalBooks()` | pivot reports, array areas, the workbooks this one reads |
| `protectSheet(sheet, password?)` / `unprotectSheet` / `sheetProtection(sheet)` / `verifySheetPassword` | the lock Excel offers under "Protect Sheet" - it stops editing, it does not encrypt |
| `protectWorkbook(password?, windows?)` / `unprotectWorkbook` / `workbookProtection()` | the same for the workbook's structure |
| `Book.readCsv(bytes, options)` / `toCsv(sheet, options)` | CSV with the delimiter and the rest stated rather than guessed |
| `evaluate(sheet, address, formula)` | evaluate without storing |
| `recalculate(sheet?, onProgress?)` | recompute everything, or one sheet |
| `recalculateCell(sheet, address)` | recompute one formula and store its result |
| `recalculateFrom(sheet, address)` | recompute what one edit reached |
| `recalculateFromMany(sheet, addresses)` | same, for a batch of edits |
| `registerFunction(name, fn)` / `unregisterFunction` / `registeredFunctions()` | functions of your own for names no built-in claims |
| `toXlsx(onProgress?)` / `toOds()` / `toXls()` | the workbook as bytes |
| `toHtml(sheet?, fragment?)` / `toCsv(sheet, options?)` | the workbook as text |
| `free()` | release the wasm memory it holds |

Everything that writes - the setters above, the grid edits, the output methods
- is in the full package only. `excelerate-reader` has the reading half of this
table and nothing else; a grid edit without a way to save it is no use.

`recalculateFrom` keeps its dependency index across calls, and `set` keeps that
index in step - so a loop of edit-then-recalc does not rebuild it each time.

## Gotchas

- **Big packages.** The zip expansion cap defaults to 512 MB. A 100 MB workbook
  can expand past that, so pass a bigger `maxExpanded` as the third argument to
  `Book.read` when you know what you are feeding it.
- **The clock.** `wasm32-unknown-unknown` has no clock of its own, so time comes
  from `Date.now()`. That is what keeps `TODAY`, `NOW`, `RAND` and
  year-less dates working instead of panicking.
- **Free your books.** The workbook lives in wasm memory. `book.free()`, or
  `using book = Book.read(...)`, keeps a loop over many files from ballooning.
- **Addresses are optional.** Every cell method has an `...At` twin taking
  1-based `row` and `column`; in a loop that is one less string to build and
  parse per cell.
- **Cross the boundary once.** `getRange`/`setRange` move a whole rectangle per
  call; a loop of `get` pays the crossing per cell. The same goes for
  formatting: `getRangeStyles` instead of `cellStyle` per cell.
- **Progress and functions of your own.** `Book.read`, `toXlsx` and
  `recalculate` take a callback last, called with
  `{ stage, done, total, what, fraction }`. `registerFunction` gives a formula
  a name no built-in claims; a function that throws reads `#VALUE!`. Both are
  the JS face of [long operations](long-operations.md).
- **Batch your edits.** `recalculateFromMany` runs one pass for the whole batch;
  calling `recalculateFrom` in a loop runs one per cell, and on a big book the
  difference is roughly 20x.
