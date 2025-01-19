// Editing loop: change many cells, recalculate once.
import { Book } from "excelerate";
import { readFileSync, writeFileSync } from "node:fs";

const book = Book.read(readFileSync(process.argv[2] ?? "files/gen.xlsx"));

const edits: Array<[address: string, value: number]> = [
  ["B2", 100],
  ["B3", 200],
  ["B4", 300],
];

for (const [address, value] of edits) {
  book.set(0, address, value);
}

// One pass for the whole batch — not one per cell.
const recomputed = book.recalculateFromMany(0, edits.map(([address]) => address));
console.log(`${recomputed} formulas recomputed`);

// Evaluating something without storing it: handy for validation or previews.
console.log("preview:", book.evaluate(0, "Z1", "=SUM(B2:B4)"));

// Same workbook, four formats — pick what the caller asked for.
writeFileSync("files/edited.xlsx", book.toXlsx());
writeFileSync("files/edited.ods", book.toOds());
writeFileSync("files/edited.html", book.toHtml(0, /* fragment */ false));
book.free();
