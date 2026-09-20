// A finished report, not a grid of values: number formats, a painted header,
// a frozen top row, a table and a note.
//   node typescript/styled-report.ts
import { Book, type CellStylePatch } from "excelerate";
import { writeFileSync } from "node:fs";

const book = new Book();
book.renameSheet(0, "Выручка");

const rows: Array<[string, number, number]> = [
  ["Январь", 1_250_000, 1_100_000],
  ["Февраль", 980_500, 1_050_000],
  ["Март", 1_640_250, 1_500_000],
];

book.setRange(0, "A1", [
  ["Месяц", "Факт", "План"],
  ...rows,
  ["Итого", `=SUM(B2:B${rows.length + 1})`, `=SUM(C2:C${rows.length + 1})`],
]);
book.recalculate();

const header: CellStylePatch = {
  font: { bold: true, color: "#FFFFFFFF" },
  fill: { pattern: "solid", foreground: "#FF1F4E79" },
  alignment: { horizontal: "center" },
};
book.setRangeStyle(0, "A1:C1", header);
// A patch is laid over what the cell has, so the total row keeps the format
// the column already gave it and only gains its border.
book.setRangeStyle(0, `B2:C${rows.length + 2}`, { numberFormat: "#,##0" });
book.setRangeStyle(0, `A${rows.length + 2}:C${rows.length + 2}`, {
  font: { bold: true },
  borders: { top: { style: "double", color: "#FF1F4E79" } },
});

book.setColumnWidth(0, 1, 16);
book.setColumnWidth(0, 2, 14);
book.setColumnWidth(0, 3, 14);
book.freezePanes(0, 1, 1);
book.setShowGridLines(0, false);
book.addTable(0, "Выручка_по_месяцам", `A1:C${rows.length + 1}`);
book.setComment(0, "B5", "excelerate", "сумма по столбцу Факт");
book.setDefinedName("Итого_факт", `Выручка!$B$${rows.length + 2}`);

console.log("итог:", book.getFormatted(0, `B${rows.length + 2}`));
console.log("заморожено строк:", book.sheetView(0).frozenRows);

writeFileSync("files/report.xlsx", book.toXlsx());
book.free();
