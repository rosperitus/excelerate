// A progress bar over read, recalculation and write, and a function of your own.
import { Book } from "excelerate";
import { readFileSync, writeFileSync } from "node:fs";

type Progress = {
  stage: string;
  done: number;
  total: number | null;
  what: string;
  fraction: number | null;
};

// Called per sheet on read, per formula on recalculation, per part on write.
// total is null while a read has not reached the workbook part yet.
const report = (p: Progress) => {
  const share = p.fraction === null ? "" : ` ${Math.round(p.fraction * 100)}%`;
  process.stdout.write(`\r${p.stage} ${p.done}/${p.total ?? "?"}${share}   `);
};

const book = Book.read(readFileSync(process.argv[2] ?? "files/gen.xlsx"), null, null, report);

// Arguments arrive evaluated, a range as a grid. A name a built-in already
// has stays the built-in's; a function that throws reads #VALUE!.
book.registerFunction("WITHVAT", (net: unknown) => {
  if (typeof net !== "number") throw new Error("a number, please");
  return Math.round(net * 120) / 100;
});
book.set(0, "Z1", 100);
book.set(0, "Z2", "=WITHVAT(Z1)");

book.recalculate(null, report);
writeFileSync("files/long.xlsx", book.toXlsx(report));
console.log(`\nZ2 = ${book.get(0, "Z2")}; registered: ${book.registeredFunctions().join(", ")}`);
book.free();
