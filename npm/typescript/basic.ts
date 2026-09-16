// Build a workbook, calculate it, write it out, read it back.
// Run from the npm/ folder:  node typescript/basic.ts
// (Node 22.6+ strips the types itself - no build step, no local node_modules)
import { Book, type CellValue } from "excelerate";
import { readFileSync, writeFileSync } from "node:fs";

const book = new Book();

book.renameSheet(0, "Quote");

const rows: Array<[string, number]> = [
  ["Bolt", 7.5],
  ["Nut", 12.5],
  ["Washer", 1.25],
];

// One call across the wasm boundary instead of one per cell.
book.setRange(0, "A1", [
  ["Item", "Price"],
  ...rows,
  ["Total", `=SUM(B2:B${rows.length + 1})`],
]);

book.recalculate();

const total: CellValue = book.get(0, `B${rows.length + 2}`);
console.log("total:", total);                             // 21.25
console.log("used range:", book.usedRange(0));            // A1:B5
console.log("formula:", book.getFormula(0, "B5"));        // SUM(B2:B4)
console.log("as displayed:", book.getFormatted(0, "B5")); // through the cell's format
console.log(book.toCsv(0));

writeFileSync("files/out.xlsx", book.toXlsx());

// Round trip: the file we just wrote reads back the same.
const reread = Book.read(readFileSync("files/out.xlsx"), "out.xlsx");
console.log("sheets:", reread.sheetNames());
console.log("A2 after the round trip:", reread.get(0, "A2"));

book.free();
reread.free();
