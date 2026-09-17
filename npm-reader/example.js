// Reads a workbook and prints what its first sheet holds, with the styling.
const { readFileSync } = require("node:fs");
const { Book } = require("excelerate-reader");

const path = process.argv[2] ?? "../tests/corpus/test1.xlsx";
const book = Book.read(readFileSync(path), path);

console.log("sheets:", book.sheetNames().join(", "));
console.log("used range:", book.usedRange(0));

const grid = book.getRange(0, "A1:D5");
for (const row of grid) {
  console.log(row.map((v) => (v === null ? "" : String(v))).join("\t"));
}

const style = book.cellStyle(0, "A1");
console.log("A1:", book.getFormatted(0, "A1"), JSON.stringify(style.font));
