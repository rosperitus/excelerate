// Read whatever lands in an upload directory: the bytes decide the format.
import { Book, type CellGrid } from "excelerate";
import { readFileSync } from "node:fs";

/** The first `limit` rows of a sheet, read in one call — no addresses built. */
function head(book: Book, sheet: number, limit = 5): CellGrid {
  const used = book.usedRange(sheet);
  if (used === undefined) {
    return [];
  }
  // "A1:D97" → how many rows and columns it spans.
  const end = used.split(":")[1];
  const rows = Math.min(Number(end.replace(/[A-Z]/g, "")), limit);
  const columns = end.replace(/[0-9]/g, "").length === 1
    ? end.charCodeAt(0) - 64
    : Number.MAX_SAFE_INTEGER;
  return book.getRangeAt(sheet, 1, 1, rows, Math.min(columns, 10));
}

const file = process.argv[2] ?? "files/gen.xlsx";
const bytes = readFileSync(file);

// A 4 GB expansion cap, for packages that legitimately blow past the default.
const book = Book.read(bytes, file, 4 * 1024 ** 3);

console.log(`${file}: ${book.sheetNames().join(", ")}`);
console.log(`${book.cellCount()} non-empty cells`);
console.log("used range:", book.usedRange(0));
console.table(head(book, 0));

book.free();
