const test = require("node:test");
const assert = require("node:assert");
const { readFileSync } = require("node:fs");
const { Book } = require("excelerate-reader");

const bytes = readFileSync(`${__dirname}/../tests/corpus/test1.xlsx`);

test("reads a workbook and its sheets", () => {
  const book = Book.read(bytes, "test1.xlsx");
  assert.ok(book.sheetNames().length > 0);
  assert.ok(book.cellCount() > 0);
  assert.ok(book.usedRange(0).includes(":"));
});

test("the value, the displayed text and the formula text", () => {
  const book = Book.read(bytes, "test1.xlsx");
  const range = book.getRange(0, "A1:E5");
  assert.strictEqual(range.length, 5);
  // Formula cells answer with the value the file was saved with.
  const grid = book.getRangeAt(0, 1, 1, 20, 10);
  assert.strictEqual(grid.length, 20);
  assert.strictEqual(typeof book.getFormatted(0, "A1"), "string");
});

test("the style of a cell", () => {
  const book = Book.read(bytes, "test1.xlsx");
  const style = book.cellStyle(0, "A1");
  assert.strictEqual(typeof style.numberFormat, "string");
  assert.strictEqual(typeof style.font.name, "string");
  assert.strictEqual(typeof style.font.size, "number");
  assert.strictEqual(typeof style.font.bold, "boolean");
  assert.ok("foreground" in style.fill);
  for (const side of ["left", "right", "top", "bottom"]) {
    assert.strictEqual(typeof style.borders[side].style, "string");
  }
  assert.strictEqual(typeof style.alignment.wrapText, "boolean");
});

test("neither writing nor recalculation is in this build", () => {
  const book = Book.read(bytes, "test1.xlsx");
  for (const gone of [
    "toXlsx",
    "toOds",
    "toXls",
    "toHtml",
    "toCsv",
    "evaluate",
    "recalculate",
    "recalculateFrom",
  ]) {
    assert.strictEqual(book[gone], undefined, `${gone} should be absent`);
  }
});

test("a whole row, boldness and merges in one call", () => {
  const book = Book.read(bytes, "test1.xlsx");
  const row = book.getRowAt(0, 1);
  assert.strictEqual(row.values.length, row.formatted.length);
  assert.strictEqual(row.values.length, row.bold.length);
  assert.strictEqual(row.values.length, row.indent.length);
  assert.strictEqual(typeof row.hidden, "boolean");
  // The batch answer and the single accessor say the same thing.
  for (let i = 0; i < row.bold.length; i += 1) {
    assert.strictEqual(row.bold[i] === 1, book.cellBoldAt(0, 1, i + 1));
    assert.strictEqual(row.indent[i], book.cellIndentAt(0, 1, i + 1));
  }
  // Asked for values alone, the row costs no strings at all.
  const bare = book.getRowAt(0, 1, false);
  assert.strictEqual(bare.formatted, null);
  assert.deepStrictEqual(Array.from(bare.values), Array.from(row.values));
  assert.deepStrictEqual(bare.bold, row.bold);
  // The numeric merges are the string ones, four numbers to an area.
  const flat = book.mergedRangesAt(0);
  assert.strictEqual(flat.length, book.mergedRanges(0).length * 4);
  assert.strictEqual(book.sheetVisibility(0), "visible");
  assert.ok(Array.isArray(book.mergedRanges(0)));
  assert.strictEqual(typeof book.rowHidden(0, 1), "boolean");
});
