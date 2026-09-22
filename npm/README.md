# excelerate

Read, write and recalculate spreadsheets in Node and the browser - xlsx, xls,
xlsb, ods, csv, html and more - with a formula engine of 518 Excel functions.
Rust compiled to WebAssembly: no native module to build, no Python, no
headless Office.

```text
npm install @rosperitus/excelerate
```

TypeScript declarations ship with the package. Only need to read? The
`@rosperitus/excelerate-reader` package has the reading half of the API in a
wasm file of 1.0 MB instead of 2.3 MB.

## Quick start

```ts
import { Book } from "@rosperitus/excelerate";
import { readFileSync, writeFileSync } from "node:fs";

// The format is detected from the bytes. The file name is optional and only
// settles the ambiguous cases (a .csv against an .html).
const book = Book.read(readFileSync("report.xlsx"), "report.xlsx");

console.log(book.sheetNames());       // ["Sheet1", "Data"]
console.log(book.get(0, "B4"));       // number | string | boolean | null

book.set(0, "B4", 42);
book.recalculateFrom(0, "B4");        // only what depends on B4

writeFileSync("out.xlsx", book.toXlsx());
```

Build a report from scratch - values, a formula, a styled header, a sort and
a total row:

```ts
import { Book } from "@rosperitus/excelerate";
import { writeFileSync } from "node:fs";

const book = new Book();
book.setRange(0, "A1", [
  ["Item", "Qty", "Price", "Sum"],
  ["Bolt", 40, 0.25, "=B2*C2"],
  ["Nut", 100, 0.1, null],
  ["Washer", 250, 0.05, null],
]);
book.fillDown(0, "D2:D4");                           // =B3*C3, =B4*C4
book.setRangeStyle(0, "A1:D1", { font: { bold: true }, fill: { pattern: "solid", foreground: "#FFDDEBF7" } });
book.setRangeStyle(0, "C2:D5", { numberFormat: "#,##0.00" });

book.recalculate();                                  // the sort reads cached values
book.sortRange(0, "A1:D4", ["-Sum"], { header: true });
book.insertRows(0, 5, 1);                            // styled like the row above
book.setRange(0, "A5", [["Total", null, null, "=SUM(D2:D4)"]]);

book.recalculate();
book.freezePanes(0, 1, 0);
writeFileSync("report.xlsx", book.toXlsx());
```

Continue a series the way the fill handle does:

```ts
book.setRange(0, "F1", [["Jan", "Кв1", 1], [null, null, 3]]);
book.fillSeries(0, "F1:H6", "down");   // Feb..Jun, Кв2..Кв6, 5, 7, 9, 11
```

## API

```ts
type CellValue = number | string | boolean | null;
type CellGrid = CellValue[][];
type CopyOrigin = "before" | "after" | "none";   // whose formatting new rows take
type SortKeys = (number | string)[];             // 2 = column B, "-Amount" = header, largest first

class Book {
  constructor();
  static fromXlsx(bytes: Uint8Array): Book;
  static read(bytes: Uint8Array, name?: string | null, maxExpanded?: number | null): Book;
  static readCsv(bytes: Uint8Array, options: CsvOptions): Book;   // shape stated, not guessed

  // Sheets
  sheetNames(): string[];
  sheetIndex(name: string): number | undefined;   // case-insensitive, as Excel compares
  addSheet(title: string): number;
  renameSheet(sheet: number, title: string): void;
  activeSheet(): number;
  setActiveSheet(sheet: number): void;
  removeSheet(sheet: number): void;               // references to it become #REF!
  cellCount(sheet?: number | null): number;
  usedRange(sheet: number): string | undefined;   // "A1:D9", by walking the cells
  usedRangeHint(sheet: number): string | undefined; // the same, as the file states it

  // Cells, by address
  get(sheet: number, address: string): CellValue;
  set(sheet: number, address: string, value: CellValue): void;
  clear(sheet: number, address: string): void;
  getFormula(sheet: number, address: string): string | undefined;
  getFormatted(sheet: number, address: string): string;   // through the number format
  getRange(sheet: number, range: string): CellGrid;       // one call, not one per cell
  setRange(sheet: number, at: string, values: CellGrid): void;
  getRangeStyles(sheet: number, range: string): RangeStyles;  // { styles, grid } - each style once
  setRangeStyles(sheet: number, at: string, styles: RangeStylesPatch): void;

  // Cells, by 1-based row and column - no address to build and re-parse
  getAt(sheet: number, row: number, column: number): CellValue;
  setAt(sheet: number, row: number, column: number, value: CellValue): void;
  clearAt(sheet: number, row: number, column: number): void;
  getFormulaAt(sheet: number, row: number, column: number): string | undefined;
  getFormattedAt(sheet: number, row: number, column: number): string;
  getRangeAt(sheet: number, row: number, column: number, rows: number, columns: number): CellGrid;
  setRangeAt(sheet: number, row: number, column: number, values: CellGrid): void;
  recalculateCellAt(sheet: number, row: number, column: number): boolean;
  recalculateFromAt(sheet: number, row: number, column: number): number;

  // Layout
  cellIndent(sheet: number, address: string): number;     // indent steps, 0 when none
  cellIndentAt(sheet: number, row: number, column: number): number;
  cellBold(sheet: number, address: string): boolean;      // the one flag, without the whole style
  cellBoldAt(sheet: number, row: number, column: number): boolean;
  rowLevel(sheet: number, row: number): number;           // outline depth, 0 when ungrouped
  columnLevel(sheet: number, column: string): number;     // by letters: "C"
  rowHidden(sheet: number, row: number): boolean;
  sheetVisibility(sheet: number): "visible" | "hidden" | "veryHidden";
  mergedRanges(sheet: number): string[];                  // ["A1:C1", ...]
  mergedRangesAt(sheet: number): Uint32Array;             // [r1, c1, r2, c2] per area
  merge(sheet: number, range: string): void;
  unmerge(sheet: number, range: string): boolean;
  columnWidth(sheet: number, column: number): number | undefined; // characters
  rowHeight(sheet: number, row: number): number | undefined;      // points
  setColumnWidth(sheet: number, column: number, width?: number): void; // undefined: back to the default
  setRowHeight(sheet: number, row: number, height?: number): void;
  setColumnHidden(sheet: number, column: number, hidden: boolean): void;
  setRowHidden(sheet: number, row: number, hidden: boolean): void;
  setSheetVisibility(sheet: number, state: "visible" | "hidden" | "veryHidden"): void;
  getRowAt(sheet: number, row: number, formatted?: boolean): SheetRow;

  // The grid itself: formulas everywhere in the workbook follow the cells they
  // read, and so do merges, links, validations, tables and drawings
  insertRows(sheet: number, at: number, count: number, copyOrigin?: CopyOrigin): void;
  removeRows(sheet: number, at: number, count: number): void;
  insertColumns(sheet: number, at: number, count: number, copyOrigin?: CopyOrigin): void;  // 1-based
  removeColumns(sheet: number, at: number, count: number): void;
  copyRange(sheet: number, range: string, to: string, toSheet?: number): void;   // formulas rewritten
  moveRange(sheet: number, range: string, to: string, toSheet?: number): void;   // formulas kept, references follow
  insertCells(sheet: number, range: string, shift: "down" | "right", copyOrigin?: CopyOrigin): void;
  removeCells(sheet: number, range: string, shift: "up" | "left"): void;
  moveSheet(from: number, to: number): void;

  // Sorting and filling, as Excel's Data - Sort, Ctrl+D and the fill handle
  sortRange(sheet: number, range: string, keys: SortKeys, options?: SortRangeOptions): void;
  sortTable(name: string, keys: SortKeys): void;
  fillDown(sheet: number, range: string): void;
  fillRight(sheet: number, range: string): void;
  fillSeries(sheet: number, range: string, direction: "down" | "right" | "up" | "left"): void;

  // What is on a sheet besides cells
  comments(sheet: number): SheetComment[];
  hyperlinks(sheet: number): SheetHyperlink[];
  tables(sheet: number): SheetTable[];
  charts(sheet: number): SheetChart[];
  images(sheet: number): SheetImage[];          // without the bytes
  imageData(sheet: number, index: number): Uint8Array;
  shapes(sheet: number): SheetShape[];

  // Style, written as a patch over what the cell has
  setCellStyle(sheet: number, address: string, patch: CellStylePatch): void;
  setCellStyleAt(sheet: number, row: number, column: number, patch: CellStylePatch): void;
  setRangeStyle(sheet: number, range: string, patch: CellStylePatch): void;

  // Notes, links, tables
  setComment(sheet: number, address: string, author: string, text: string): void;
  removeComment(sheet: number, address: string): boolean;
  setHyperlink(sheet: number, range: string, target: string, inside?: boolean, display?: string, tooltip?: string): void;
  removeHyperlink(sheet: number, range: string): boolean;
  addTable(sheet: number, name: string, range: string, headerRow?: boolean): void;
  removeTable(sheet: number, name: string): boolean;

  // The saved view
  sheetView(sheet: number): SheetViewInfo;
  freezePanes(sheet: number, rows: number, columns: number): void;   // (0, 1, 0) pins the header
  unfreezePanes(sheet: number): void;
  setZoom(sheet: number, percent?: number): void;
  setShowGridLines(sheet: number, show: boolean, headers?: boolean): void;

  // Names
  definedNames(): WorkbookName[];
  setDefinedName(name: string, formula: string, sheet?: number): void;
  removeDefinedName(name: string, sheet?: number): boolean;

  // Rules a file states
  dataValidations(sheet: number): SheetValidation[];
  conditionalFormats(sheet: number): SheetConditionalFormat[];
  autoFilter(sheet: number): SheetAutoFilter | undefined;
  pivotTables(sheet: number): SheetPivotTable[];
  arrayFormulas(sheet: number): string[];       // ["B2:B4"]
  externalBooks(): (string | undefined)[];

  // Locks - they stop editing, they do not encrypt
  protectSheet(sheet: number, password?: string): void;
  unprotectSheet(sheet: number): void;
  sheetProtection(sheet: number): ProtectionInfo;
  verifySheetPassword(sheet: number, password: string): boolean;
  protectWorkbook(password?: string, windows?: boolean): void;
  unprotectWorkbook(): void;
  workbookProtection(): ProtectionInfo;

  // Calculation
  evaluate(sheet: number, address: string, formula: string): CellValue;
  recalculate(sheet?: number | null): number;
  recalculateCell(sheet: number, address: string): boolean;
  recalculateFrom(sheet: number, address: string): number;
  recalculateFromMany(sheet: number, addresses: string[]): number;

  // Output
  toXlsx(): Uint8Array;
  toOds(): Uint8Array;
  toXls(): Uint8Array;
  toHtml(sheet?: number | null, fragment?: boolean | null): string;
  toCsv(sheet: number): string;
  toCsvWith(sheet: number, options: CsvOptions): string;   // a delimiter of your own

  free(): void;
}
```

Every method that can fail throws an `Error` with the reason - a bad address, a
missing sheet, a truncated package.

## Notes worth reading once

- **`free()` / `using`.** The workbook lives in wasm memory. Node's GC will get
  to it eventually, but in a loop over many files, release it deliberately:
  `book.free()`, or `using book = Book.read(...)` with explicit resource
  management.
- **Numbers or addresses, your call.** Every cell method has an `...At` twin
  taking 1-based `row` and `column` - the numbers `ROW()` and `COLUMN()`
  return. Looping over a grid, they save building an address string only to
  have it parsed straight back.
- **Move rectangles, not cells.** `getRange`/`setRange` cross the wasm
  boundary once for the whole block; a loop of `get` crosses once per cell.
- **Batch your edits.** `recalculateFromMany(sheet, ["A1", "B7", ...])` runs one
  pass for the whole batch. Calling `recalculateFrom` in a loop runs one pass
  per cell - on a 20k-formula workbook that is roughly 20x the work.
- **Big files.** Zip expansion is capped at 512 MB to stop a zip bomb. Real
  workbooks do exceed it (a 100 MB package can expand to 560 MB), so pass a
  larger `maxExpanded` third argument to `Book.read` when you know the source.
- **Editing the grid.** `insertRows` and its three siblings move the whole
  workbook, not one sheet: `$A$5` becomes `$A$6`, a range that lost cells
  narrows, a deleted cell reads `#REF!`, and drawings and tables follow. The
  cached result of a rewritten formula is dropped, so recalculate after.
- **Style is a patch.** `setCellStyle(0, "A1", { font: { bold: true } })` keeps
  the number format the cell had. Equal styles share one entry in the
  workbook's table, so painting a column costs one style, not one per cell -
  and `setRangeStyle` is one call for the whole rectangle.
- **Copy rewrites, move does not.** `copyRange` rewrites the formulas it
  carries, as if they had been written where they land; `moveRange` keeps them
  pointing at the same cells and rewrites the formulas elsewhere that read the
  moved ones. That is Ctrl+C against Ctrl+X, and it is the whole difference.
- **Inserted rows look like their neighbours.** `insertRows` copies the row
  above's cell styles and height, as Excel does. Pass `"after"` for the row
  below, `"none"` for a blank row.
- **Sort keys are numbers or names.** `sortRange(0, "A1:D99", [3, -2])` sorts
  by column C, then by B largest first; with `{ header: true }` the same reads
  `["Price", "-Qty"]`. Formulas sort by their cached value, so recalculate
  first if you have edited the inputs. `sortTable("Sales", ["-Amount"])`
  needs no range at all.
- **Read and write formatting by the block.** `getRangeStyles` returns each
  distinct style once and a grid of indexes into them; `setRangeStyles` takes
  the same shape back, so copying a block's look is two calls.
- **Charts, pictures and shapes are read-only here.** `charts`, `images` and
  `shapes` describe what a sheet carries; the parts themselves travel through
  a write byte for byte.
- **Cached results.** A formula cell read from a file carries whatever the app
  that saved it computed. Call `recalculate()` before relying on those numbers.

## Formats

| Format | Read | Write |
|---|:--:|:--:|
| xlsx | ✅ | ✅ (`toXlsx`) |
| xls (BIFF8) | ✅ | ✅ (`toXls`) |
| ods | ✅ | ✅ (`toOds`) |
| CSV | ✅ | ✅ (`toCsv`) |
| HTML | ✅ | ✅ (`toHtml`) |
| SYLK, Gnumeric, SpreadsheetML 2003 | ✅ | - |

SYLK, Gnumeric and SpreadsheetML are read-only in the Rust crate too - there is
nothing to bind.

`toXls` compiles formulas to BIFF8 tokens with their cached results; what
BIFF8 cannot hold (a structured reference, a row past 65,536) goes out as its
value. Call `recalculate()` first so those values are current.

## Development

This folder is both the published package's readme and the workspace that
builds and exercises it.

```
../tools/build-npm.sh          # wasm-pack --release --target nodejs -> npm/pkg
npm install && npm test        # tests against the freshly built package
npm start                      # example.js: build a book, calculate, round-trip
npm run ts                     # typescript/basic.ts, no build step
npm run typecheck              # tsc --strict over the TypeScript examples
npm run bench [iterations]     # parse timings, median and best
../tools/pack-npm.sh           # pkg -> pkg-publish, then: cd pkg-publish && npm publish --otp=...
```

| Path | What it is |
|---|---|
| `pkg/` | build output for Node (git-ignored); `pkg-web/` and `pkg-bundler/` for the other targets |
| `pkg-publish/` | `tools/pack-npm.sh`: a copy of `pkg/` renamed to `@rosperitus/excelerate`, the folder `npm publish` runs in (git-ignored) |
| `example.js`, `test.js`, `bench.js` | JavaScript examples, tests, benchmark |
| `typescript/` | TypeScript examples, run from here: `node typescript/basic.ts` (Node 22.6+). No node_modules of their own - they resolve the package from this folder |
| `browser/` | the same API in a page: `index.html` over `pkg-web/` |
| `files/` | workbooks the tests read (`gen.xlsx`, `grouped.xlsx`, committed) and whatever the benchmark and examples read and write (git-ignored) |

## License

MIT
