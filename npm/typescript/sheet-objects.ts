// An inventory of a sheet: everything on it that is not a cell.
import { Book, type SheetChart, type SheetImage } from "excelerate";
import { readFileSync } from "node:fs";

const file = process.argv[2] ?? "../tests/fixtures/chart.xlsx";
const book = Book.read(readFileSync(file), file);

for (let sheet = 0; sheet < book.sheetNames().length; sheet += 1) {
  const name = book.sheetNames()[sheet];
  // The used range as the file states it - no walk over the cells.
  console.log(`${name} (${book.sheetVisibility(sheet)}) ${book.usedRangeHint(sheet) ?? "empty"}`);

  for (const table of book.tables(sheet)) {
    console.log(`  table ${table.displayName} over ${table.range}: ${table.columns.join(", ")}`);
  }
  for (const note of book.comments(sheet)) {
    console.log(`  note at ${note.address} by ${note.author}: ${note.text}`);
  }
  for (const link of book.hyperlinks(sheet)) {
    console.log(`  link ${link.range} -> ${link.target}${link.external ? "" : " (inside)"}`);
  }
  for (const chart of book.charts(sheet) as SheetChart[]) {
    const where = `${chart.anchor.row ?? "?"}:${chart.anchor.column ?? "?"}`;
    console.log(`  chart ${chart.title ?? chart.name} at ${where}: ${chart.kinds.join("+")}, ${chart.seriesCount} series`);
  }
  book.images(sheet).forEach((image: SheetImage, index: number) => {
    // The listing carries no bytes; ask for them only when they are wanted.
    const bytes = book.imageData(sheet, index);
    console.log(`  picture ${image.name} (${image.format}, ${bytes.length} bytes)`);
  });
  for (const shape of book.shapes(sheet)) {
    console.log(`  shape ${shape.name} (${shape.geometry ?? "freeform"}) "${shape.text}"`);
  }
}

book.free();
