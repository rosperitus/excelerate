// A table from data, filtered, read back: the object layer over Book, where
// every call that changes something returns its object.
import { Workbook, type TableFilter } from "excelerate";
import { writeFileSync } from "node:fs";

const wb = new Workbook();
const sales = wb
  .addSheet("Sales")
  .addTableFromData("Sales", "A1", [
    ["Region", "Manager", "Amount"],
    ["North", "Ivanov", 120],
    ["South", "Petrov", 340],
    ["North", "Sidorov", 75],
    ["West", "Kozlov", 210],
    ["South", "Smirnov", 95],
  ])
  .set(1, "Amount", 125);

sales.sheet.style("A1:C1", { font: { bold: true } }).width("B", 14);

// Excel's own filter: the criterion goes into the file, the rows it rejects
// are hidden there too.
const southAndWest: TableFilter = { values: ["South", "West"] };
sales.addFilter("Region", southAndWest).addFilter("Amount", { custom: [{ op: ">", value: 100 }] });

for (const { Manager, Amount } of sales.records({ visible: true })) {
  console.log(`${Manager}: ${Amount}`);
}

writeFileSync("sales.xlsx", wb.toXlsx());
wb.free();
