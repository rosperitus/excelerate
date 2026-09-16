const test = require("node:test");
const assert = require("node:assert");
const { Book } = require("excelerate");

test("values and formulas", () => {
  const book = new Book();
  book.set(0, "A1", 2);
  book.set(0, "A2", 3);
  assert.strictEqual(book.get(0, "A1"), 2);
  assert.strictEqual(book.evaluate(0, "A3", "=A1*A2+SUM(A1:A2)"), 11);
  assert.strictEqual(book.evaluate(0, "A3", "=UPPER(\"ой\")"), "ОЙ");
  assert.strictEqual(book.evaluate(0, "A3", "=1/0"), "#DIV/0!");
});

test("a round trip through xlsx", () => {
  const book = new Book();
  book.set(0, "A1", "Гайка");
  book.set(0, "B1", 12.5);
  book.set(0, "C1", true);
  book.addSheet("Второй");

  const reread = Book.fromXlsx(book.toXlsx());
  assert.deepStrictEqual(reread.sheetNames(), book.sheetNames());
  assert.strictEqual(reread.get(0, "A1"), "Гайка");
  assert.strictEqual(reread.get(0, "B1"), 12.5);
  assert.strictEqual(reread.get(0, "C1"), true);
  assert.strictEqual(reread.get(0, "D1"), null);
});

test("csv", () => {
  const book = new Book();
  book.set(0, "A1", "a,b");
  book.set(0, "B1", 1);
  assert.strictEqual(book.toCsv(0).trim(), '"a,b",1');
});

test("the errors are readable", () => {
  const book = new Book();
  assert.throws(() => book.get(9, "A1"), /no such sheet/);
  assert.throws(() => book.set(0, "нея", 1));
});

test("a round trip with a real file: read -> edit -> write", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  const path = require("node:path");

  // files/gen.xlsx: on sheet "01_Математика" column C holds formulas, D holds
  // the expectations, and the data they read sits in A101 and below.
  const book = Book.fromXlsx(fs.readFileSync(path.join(__dirname, "files", "gen.xlsx")));
  assert.strictEqual(book.sheetNames()[0], "01_Математика");
  assert.strictEqual(book.get(0, "A101"), 10);
  assert.strictEqual(book.get(0, "D3"), 100);
  assert.strictEqual(book.evaluate(0, "C4", "=SUM(A100:A109)"), 450);

  book.set(0, "A101", 110);
  assert.strictEqual(book.get(0, "A101"), 110);
  assert.strictEqual(book.evaluate(0, "C4", "=SUM(A100:A109)"), 550);

  const out = path.join(os.tmpdir(), `excelerate-${process.pid}.xlsx`);
  fs.writeFileSync(out, book.toXlsx());
  try {
    const reread = Book.fromXlsx(fs.readFileSync(out));
    assert.deepStrictEqual(reread.sheetNames(), book.sheetNames());
    assert.strictEqual(reread.get(0, "A101"), 110);
    assert.strictEqual(reread.get(0, "D3"), 100);
    assert.strictEqual(reread.evaluate(0, "C4", "=SUM(A100:A109)"), 550);
  } finally {
    fs.unlinkSync(out);
  }
});

test("Book.read works out the format on its own", () => {
  const fs = require("node:fs");
  const path = require("node:path");
  const bytes = fs.readFileSync(path.join(__dirname, "files", "gen.xlsx"));

  // With no file name, the signature alone answers.
  assert.strictEqual(Book.read(bytes).get(0, "A101"), 10);
  // The name changes nothing, even when it lies.
  assert.strictEqual(Book.read(bytes, "gen.csv").get(0, "A101"), 10);

  // CSV announces itself by extension only; without one it is still CSV, by default.
  const csv = Buffer.from("a,b\n1,2\n");
  assert.strictEqual(Book.read(csv, "data.csv").get(0, "B2"), 2);
  assert.strictEqual(Book.read(csv).get(0, "A1"), "a");

  // HTML is recognized by its own tag.
  const html = Buffer.from("<table><tr><td>7</td></tr></table>");
  assert.strictEqual(Book.read(html).get(0, "A1"), 7);

  assert.throws(() => Book.read(Buffer.from("PK\x03\x04не архив")));
});

test("recalculating formulas when a workbook is opened", () => {
  const fs = require("node:fs");
  const path = require("node:path");
  const book = Book.fromXlsx(fs.readFileSync(path.join(__dirname, "files", "gen.xlsx")));

  // In files/gen.xlsx the formulas carry no cache: column C is the formula, D the expectation.
  assert.strictEqual(book.get(0, "C3"), null);
  assert.strictEqual(book.get(1, "C3"), null);

  // One sheet.
  const onSheet = book.recalculate(0);
  assert.ok(onSheet > 0, `sheet 0 should hold formulas, not ${onSheet}`);
  assert.strictEqual(book.get(0, "C3"), book.get(0, "D3"));
  assert.strictEqual(book.get(1, "C3"), null, "the neighbouring sheet is untouched");

  // The whole workbook: it holds more formulas than any one sheet.
  const whole = book.recalculate();
  assert.ok(whole > onSheet, `${whole} > ${onSheet}`);
  assert.strictEqual(book.get(1, "C3"), book.get(1, "D3"));

  // Recalculation is idempotent: the same formulas, the same values.
  assert.strictEqual(book.recalculate(), whole);
  assert.strictEqual(book.get(0, "C3"), book.get(0, "D3"));

  // And the result survives writing: the cache goes into the file.
  const reread = Book.read(book.toXlsx());
  assert.strictEqual(reread.get(0, "C3"), book.get(0, "D3"));
  assert.strictEqual(reread.get(1, "C3"), book.get(1, "D3"));
});

test("recalculation sees the edit", () => {
  const book = new Book();
  book.set(0, "A1", 2);
  book.set(0, "A2", 3);
  book.set(0, "A3", "=A1+A2");
  assert.strictEqual(book.get(0, "A3"), null);

  assert.strictEqual(book.recalculate(), 1);
  assert.strictEqual(book.get(0, "A3"), 5);

  book.set(0, "A1", 10);
  book.recalculate(0);
  assert.strictEqual(book.get(0, "A3"), 13);
  assert.strictEqual(book.toCsv(0).trim(), "10\r\n3\r\n13");
});

test("an edit recalculates only the cells that depend on it", () => {
  const book = new Book();
  book.set(0, "A1", 2);
  book.set(0, "A2", 3);
  book.set(0, "A3", "=A1+A2");
  book.set(0, "A4", "=A3*10");
  book.set(0, "B1", "=7*6");
  assert.strictEqual(book.recalculate(), 3);

  book.set(0, "A1", 10);
  // A3 reads A1, A4 reads A3; B1 reads none of them.
  assert.strictEqual(book.recalculateFrom(0, "A1"), 2);
  assert.strictEqual(book.get(0, "A3"), 13);
  assert.strictEqual(book.get(0, "A4"), 130);
  assert.strictEqual(book.get(0, "B1"), 42);

  // An edit nobody reads costs nothing.
  book.set(0, "Z99", 1);
  assert.strictEqual(book.recalculateFrom(0, "Z99"), 0);

  assert.throws(() => book.recalculateFrom(9, "A1"), /no such sheet/);
});

test("on a large workbook an edit is cheaper than a full recalculation", () => {
  const fs = require("node:fs");
  const path = require("node:path");
  const book = Book.read(fs.readFileSync(path.join(__dirname, "files", "gen.xlsx")));

  const all = book.recalculate();
  assert.ok(all > 100, `${all} formulas`);
  const touched = book.recalculateFrom(0, "A101");
  assert.ok(touched > 0 && touched < all / 10, `${touched} of ${all}`);
});

test("a batch of edits is one pass", () => {
  const book = new Book();
  for (let row = 1; row <= 200; row++) {
    book.set(0, `A${row}`, row);
    book.set(0, `B${row}`, `=A${row}*2`);
  }
  book.set(0, "D1", "=SUM(B1:B200)");
  assert.strictEqual(book.recalculate(), 201);
  assert.strictEqual(book.get(0, "D1"), 40200);

  // Ten edits, one pass, and it touches only their formulas plus the sum.
  const edited = [];
  for (let row = 1; row <= 10; row++) {
    book.set(0, `A${row}`, 1000 + row);
    edited.push(`A${row}`);
  }
  assert.strictEqual(book.recalculateFromMany(0, edited), 11);
  assert.strictEqual(book.get(0, "B1"), 2002);
  assert.strictEqual(book.get(0, "D1"), 60200);

  // The index survives an edited value and an edited formula.
  book.set(0, "B1", "=A1*3");
  assert.strictEqual(book.recalculateFrom(0, "A1"), 2);
  assert.strictEqual(book.get(0, "B1"), 3003);
  assert.strictEqual(book.get(0, "D1"), 61201);
});

test("ranges are read and written in one call", () => {
  const book = new Book();
  book.setRange(0, "A1", [
    ["Товар", "Цена"],
    ["Болт", 7.5],
    ["Гайка", 12.5],
    ["Итого", "=SUM(B2:B3)"],
  ]);
  book.recalculate();

  assert.strictEqual(book.usedRange(0), "A1:B4");
  assert.deepStrictEqual(book.getRange(0, "A1:B4"), [
    ["Товар", "Цена"],
    ["Болт", 7.5],
    ["Гайка", 12.5],
    ["Итого", 20],
  ]);
  // Beyond what is filled in it answers null, not an error.
  assert.deepStrictEqual(book.getRange(0, "D9:D10"), [[null], [null]]);
  assert.throws(() => book.setRange(0, "A1", "not an array"));
});

test("a formula, formatting and clearing a cell", () => {
  const book = new Book();
  book.set(0, "A1", 0.256);
  book.set(0, "A2", "=A1*2");
  book.recalculate();

  assert.strictEqual(book.getFormula(0, "A2"), "A1*2");
  assert.strictEqual(book.getFormula(0, "A1"), undefined);
  // With no format on the cell it is General.
  assert.strictEqual(book.getFormatted(0, "A1"), "0.256");

  book.clear(0, "A1");
  assert.strictEqual(book.get(0, "A1"), null);
  assert.strictEqual(book.usedRange(0), "A2:A2");
});

test("sheets: name, index, active tab", () => {
  const book = new Book();
  book.addSheet("Данные");

  assert.strictEqual(book.sheetIndex("данные"), 1, "a sheet name is matched case-insensitively");
  assert.strictEqual(book.sheetIndex("no such sheet"), undefined);

  book.renameSheet(0, "Отчёт");
  assert.deepStrictEqual(book.sheetNames(), ["Отчёт", "Данные"]);

  assert.strictEqual(book.activeSheet(), 0);
  book.setActiveSheet(1);
  assert.strictEqual(book.activeSheet(), 1);
  assert.throws(() => book.setActiveSheet(9));
});

test("output in ods, xls and html", () => {
  const book = new Book();
  book.set(0, "A1", "Гайка");
  book.set(0, "B1", 12.5);

  assert.ok(book.toOds().length > 0);
  assert.ok(book.toXls().length > 0);

  const page = book.toHtml(0, true);
  assert.ok(page.includes("Гайка"), "the value lands in the table");
  assert.ok(!page.includes("<html"), "a fragment comes without the wrapper");
  assert.ok(book.toHtml().includes("<html"));
});

test("cell indent and outline levels", () => {
  // files/grouped.xlsx is committed: A2 indented by 3, row 2 at level 2,
  // columns C:D at level 1.
  const fs = require("node:fs");
  const path = require("node:path");
  const file = path.join(__dirname, "files", "grouped.xlsx");

  const book = Book.read(fs.readFileSync(file), "grouped.xlsx");
  assert.strictEqual(book.cellIndent(0, "A1"), 0, "with no style there is no indent");
  assert.strictEqual(book.cellIndent(0, "A2"), 3);

  assert.strictEqual(book.rowLevel(0, 1), 0, "a row outside any group");
  assert.strictEqual(book.rowLevel(0, 2), 2);

  assert.strictEqual(book.columnLevel(0, "A"), 0);
  assert.strictEqual(book.columnLevel(0, "C"), 1, "a column inside the C:D run");
  assert.strictEqual(book.columnLevel(0, "E"), 0);

  assert.throws(() => book.rowLevel(0, 0), "there is no row zero");
  assert.throws(() => book.columnLevel(0, "not letters"));
});

test("addressing by numbers instead of strings", () => {
  const book = new Book();
  book.setRangeAt(0, 1, 1, [
    ["Товар", "Цена"],
    ["Болт", 7.5],
    ["Гайка", 12.5],
    ["Итого", "=SUM(B2:B3)"],
  ]);
  book.recalculate();

  // The numbers are the ones the user sees: row 4, column 2 is B4.
  assert.strictEqual(book.getAt(0, 4, 2), book.get(0, "B4"));
  assert.strictEqual(book.getFormulaAt(0, 4, 2), "SUM(B2:B3)");
  assert.strictEqual(book.getFormattedAt(0, 4, 2), "20");
  assert.strictEqual(book.cellIndentAt(0, 1, 1), 0);

  assert.deepStrictEqual(book.getRangeAt(0, 1, 1, 2, 2), [
    ["Товар", "Цена"],
    ["Болт", 7.5],
  ]);

  book.setAt(0, 2, 2, 20);
  assert.strictEqual(book.recalculateFromAt(0, 2, 2), 1);
  assert.strictEqual(book.getAt(0, 4, 2), 32.5);

  book.clearAt(0, 1, 1);
  assert.strictEqual(book.getAt(0, 1, 1), null);

  assert.throws(() => book.getAt(0, 0, 1), "numbering starts at one, there is no row 0");
  assert.throws(() => book.getRangeAt(0, 1, 1, 0, 2), "an empty range");
});

test("a custom function in a formula", () => {
  const book = new Book();
  book.set(0, "A1", 2);
  book.set(0, "A2", 3);
  book.registerFunction("МОЙИТОГ", (range) => {
    let total = 0;
    for (const row of range) for (const v of row) total += Number(v) || 0;
    return total * 2;
  });
  book.registerFunction("myrate", (x) => x * 1.5 + 0.5);

  assert.strictEqual(book.evaluate(0, "B1", "=МОЙИТОГ(A1:A2)"), 10);
  // The name is matched case-insensitively, as a builtin is.
  assert.strictEqual(book.evaluate(0, "B1", "=MYRATE(10)"), 15.5);
  assert.strictEqual(book.evaluate(0, "B1", "=SUM(MYRATE(1),MYRATE(2))"), 5.5);
  // An array from JS becomes an array of the engine.
  book.registerFunction("МАССИВ", () => [[1, 2], [3, 4]]);
  assert.strictEqual(book.evaluate(0, "B1", "=SUM(МАССИВ())"), 10);
  // A string holding an error code is an error, not text.
  book.registerFunction("СЛОМАН", () => "#N/A");
  assert.strictEqual(book.evaluate(0, "B1", "=СЛОМАН()"), "#N/A");
  // An exception inside the function does not bring the process down.
  book.registerFunction("ПЛОХАЯ", () => { throw new Error("no"); });
  assert.strictEqual(book.evaluate(0, "B1", "=ПЛОХАЯ()"), "#VALUE!");
  // A builtin name does not become the caller's.
  book.registerFunction("SUM", () => -1);
  assert.strictEqual(book.evaluate(0, "B1", "=SUM(A1:A2)"), 5);

  assert.ok(book.registeredFunctions().includes("МОЙИТОГ"));
  assert.strictEqual(book.unregisterFunction("МОЙИТОГ"), true);
  assert.strictEqual(book.evaluate(0, "B1", "=МОЙИТОГ(A1:A2)"), "#NAME?");
});

test("recalculation sees the custom functions", () => {
  const book = new Book();
  book.set(0, "A1", 4);
  book.registerFunction("УДВОЙ", (x) => x * 2);
  book.set(0, "B1", "=УДВОЙ(A1)");
  assert.strictEqual(book.recalculate(), 1);
  assert.strictEqual(book.get(0, "B1"), 8);
});

test("progress while reading, writing and recalculating", () => {
  const book = new Book();
  book.set(0, "A1", 1);
  book.set(0, "B1", "=A1+1");
  book.addSheet("Второй");

  const written = [];
  const bytes = book.toXlsx((p) => {
    assert.strictEqual(p.stage, "writing");
    written.push(p.what);
  });
  assert.ok(written.includes("xl/workbook.xml"), written.join(","));

  const read = [];
  const back = Book.read(bytes, "b.xlsx", undefined, (p) => read.push(p));
  assert.strictEqual(back.sheetNames().length, 2);
  assert.strictEqual(read.length, 2, "one report per sheet");
  assert.strictEqual(read[0].stage, "reading");
  assert.strictEqual(read[0].done, 0);
  assert.strictEqual(read[0].total, 2);
  assert.strictEqual(read[1].fraction, 0.5);

  const steps = [];
  back.recalculate(undefined, (p) => steps.push(p.stage));
  assert.deepStrictEqual(steps, ["recalculating"]);
});
