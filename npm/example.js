// An integration example: build a workbook a range at a time, compute it, write
// it out in four formats and read it back.
const fs = require("node:fs");
const path = require("node:path");
const { Book } = require("excelerate");

const book = new Book();
book.renameSheet(0, "Estimate");

// One crossing of the wasm boundary instead of eight.
book.setRange(0, "A1", [
  ["Item", "Price"],
  ["Nut", 12.5],
  ["Bolt", 7.5],
  ["Total", "=SUM(B2:B3)"],
]);
book.recalculate();

console.log("sheets:", book.sheetNames(), "active:", book.activeSheet());
console.log("used range:", book.usedRange(0));
console.log("grid:", book.getRange(0, "A1:B4"));
console.log("B4: formula", book.getFormula(0, "B4"), "->", book.getFormatted(0, "B4"));
console.log(book.toCsv(0));

// An edit and a targeted recalculation: only what depends on B2 is computed.
book.set(0, "B2", 20);
console.log("formulas recalculated:", book.recalculateFrom(0, "B2"), "-> total", book.get(0, "B4"));

const dir = path.join(__dirname, "files");
fs.writeFileSync(path.join(dir, "out.xlsx"), book.toXlsx());
fs.writeFileSync(path.join(dir, "out.ods"), book.toOds());
fs.writeFileSync(path.join(dir, "out.xls"), book.toXls());
fs.writeFileSync(path.join(dir, "out.html"), book.toHtml(0));

const reread = Book.read(fs.readFileSync(path.join(dir, "out.xlsx")), "out.xlsx");
console.log("after the round trip A2 =", reread.get(0, "A2"), "| sheet", reread.sheetNames()[0]);
