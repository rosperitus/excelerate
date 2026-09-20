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

// Layout: widths in characters, heights in points. Setting one column splits
// the run that covered it, so its neighbours keep the width they had.
book.setColumnWidth(0, 1, 24);
book.setColumnWidth(0, 2, 12);
book.setRowHeight(0, 1, 22);
book.merge(0, "A6:B6");
const notes = book.addSheet("Notes");
book.setSheetVisibility(notes, "hidden");
console.log("A is", book.columnWidth(0, 1), "wide;", book.mergedRanges(0).join(", "), "merged");

// Painting: a patch over the style the cell had, so the number format set
// below does not undo the bold above it.
book.setRangeStyle(0, "A1:B1", {
  font: { bold: true, color: "#FF1F4E79" },
  fill: { pattern: "solid", foreground: "#FFFFE699" },
});
book.setRangeStyle(0, "B2:B4", { numberFormat: "#,##0.00" });
book.freezePanes(0, 1, 0);           // the header stays put while the rest scrolls
book.setComment(0, "B1", "excelerate", "prices include VAT");
book.setHyperlink(0, "A1", "https://example.com/price-list");
book.addTable(0, "Items", "A1:B3");
book.setDefinedName("Total", "Estimate!$B$4");
console.log("B2 shows", book.getFormatted(0, "B2"), "| names:", book.definedNames().map((n) => n.name));

// Editing the grid: a row inserted above the table moves the cells and rewrites
// every formula in the workbook that reads them.
book.insertRows(0, 1, 1);
console.log("after inserting a row the total is at B5:", book.getFormula(0, "B5"));

const dir = path.join(__dirname, "files");
fs.writeFileSync(path.join(dir, "out.xlsx"), book.toXlsx());
fs.writeFileSync(path.join(dir, "out.ods"), book.toOds());
fs.writeFileSync(path.join(dir, "out.xls"), book.toXls());
fs.writeFileSync(path.join(dir, "out.html"), book.toHtml(0));

const reread = Book.read(fs.readFileSync(path.join(dir, "out.xlsx")), "out.xlsx");
console.log("after the round trip A2 =", reread.get(0, "A2"), "| sheet", reread.sheetNames()[0]);
