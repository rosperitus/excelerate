# excelerate

Read, write and recalculate spreadsheets in Node — xlsx, xls, ods, csv, html
and more — with a real formula engine (443 Excel functions). Rust compiled to
WebAssembly, so there is no native module to build, no Python, no headless
Office.

```
npm install excelerate
```

TypeScript declarations ship with the package; nothing extra to install.

## Quick start

```ts
import { Book } from "excelerate";
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

Build one from scratch:

```ts
import { Book } from "excelerate";

const book = new Book();
book.set(0, "A1", "Item");
book.set(0, "B1", "Price");
book.set(0, "A2", "Bolt");
book.set(0, "B2", 7.5);
book.set(0, "A3", "Total");
book.set(0, "B3", "=SUM(B2:B2)");     // a string starting with = is a formula

book.recalculate();
console.log(book.toCsv(0));
```

## API

```ts
type CellValue = number | string | boolean | null;
type CellGrid = CellValue[][];

class Book {
  constructor();
  static fromXlsx(bytes: Uint8Array): Book;
  static read(bytes: Uint8Array, name?: string | null, maxExpanded?: number | null): Book;

  // Sheets
  sheetNames(): string[];
  sheetIndex(name: string): number | undefined;   // case-insensitive, as Excel compares
  addSheet(title: string): number;
  renameSheet(sheet: number, title: string): void;
  activeSheet(): number;
  setActiveSheet(sheet: number): void;
  cellCount(sheet?: number | null): number;
  usedRange(sheet: number): string | undefined;   // "A1:D9"

  // Cells, by address
  get(sheet: number, address: string): CellValue;
  set(sheet: number, address: string, value: CellValue): void;
  clear(sheet: number, address: string): void;
  getFormula(sheet: number, address: string): string | undefined;
  getFormatted(sheet: number, address: string): string;   // through the number format
  getRange(sheet: number, range: string): CellGrid;       // one call, not one per cell
  setRange(sheet: number, at: string, values: CellGrid): void;

  // Cells, by 1-based row and column — no address to build and re-parse
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
  getRowAt(sheet: number, row: number, formatted?: boolean): SheetRow;

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

  free(): void;
}
```

Every method that can fail throws an `Error` with the reason — a bad address, a
missing sheet, a truncated package.

## Notes worth reading once

- **`free()` / `using`.** The workbook lives in wasm memory. Node's GC will get
  to it eventually, but in a loop over many files, release it deliberately:
  `book.free()`, or `using book = Book.read(...)` with explicit resource
  management.
- **Numbers or addresses, your call.** Every cell method has an `…At` twin
  taking 1-based `row` and `column` — the numbers `ROW()` and `COLUMN()`
  return. Looping over a grid, they save building an address string only to
  have it parsed straight back.
- **Move rectangles, not cells.** `getRange`/`setRange` cross the wasm
  boundary once for the whole block; a loop of `get` crosses once per cell.
- **Batch your edits.** `recalculateFromMany(sheet, ["A1", "B7", …])` runs one
  pass for the whole batch. Calling `recalculateFrom` in a loop runs one pass
  per cell — on a 20k-formula workbook that is roughly 20× the work.
- **Big files.** Zip expansion is capped at 512 MB to stop a zip bomb. Real
  workbooks do exceed it (a 100 MB package can expand to 560 MB), so pass a
  larger `maxExpanded` third argument to `Book.read` when you know the source.
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
| SYLK, Gnumeric, SpreadsheetML 2003 | ✅ | — |

SYLK, Gnumeric and SpreadsheetML are read-only in the Rust crate too — there is
nothing to bind.

One catch on `toXls`: BIFF8 cannot store a formula as text, so every formula
goes out as its last computed value. Call `recalculate()` first.

## Development

This folder is both the published package's readme and the workspace that
builds and exercises it.

```
../tools/build-npm.sh          # wasm-pack --release --target nodejs → npm/pkg
npm install && npm test        # tests against the freshly built package
npm start                      # example.js: build a book, calculate, round-trip
npm run ts                     # typescript/basic.ts, no build step
npm run typecheck              # tsc --strict over the TypeScript examples
npm run bench [iterations]     # parse timings, median and best
```

| Path | What it is |
|---|---|
| `pkg/` | build output, published as-is (git-ignored) |
| `example.js`, `test.js`, `bench.js` | JavaScript examples, tests, benchmark |
| `typescript/` | TypeScript examples, run from here: `node typescript/basic.ts` (Node 22.6+). No node_modules of their own — they resolve the package from this folder |
| `files/` | workbooks the benchmark and tests use (git-ignored except `gen.xlsx`) |

## License

MIT
