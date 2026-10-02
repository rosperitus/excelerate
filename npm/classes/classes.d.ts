/**
 * Objects over `Book`, whose changing methods return themselves so calls chain:
 *
 * ```js
 * const wb = new Workbook();
 * const sales = wb.addSheet("Sales")
 *   .addTableFromData("Sales", "A1", [["Region", "Amount"], ["North", 120], ["South", 340]])
 *   .set(1, "Amount", 125)
 *   .addFilter("Region", { values: ["South"] });
 * sales.records({ visible: true }); // [{ Region: "South", Amount: 340 }]
 * ```
 *
 * Each object is a handle - a sheet index, a table name, an address - and
 * works through `workbook.book`, which is there for everything not wrapped.
 */
export declare class Workbook {
  /** A new workbook, or one over a `Book` already in hand. */
  constructor(book?: Book);
  readonly book: Book;
  /** Reads any format `Book.read` reads. */
  static read(bytes: Uint8Array, name?: string, maxExpanded?: number, onProgress?: Function): Workbook;
  /** A sheet by position (from 0) or by name. */
  sheet(which: number | string): Sheet;
  sheets(): Sheet[];
  /** Adds a sheet at the end. On a new workbook the first call takes over its empty "Sheet1". */
  addSheet(name?: string): Sheet;
  recalculate(onProgress?: Function): this;
  toXlsx(onProgress?: Function): Uint8Array;
  toOds(): Uint8Array;
  toXls(): Uint8Array;
  free(): void;
}

export declare class Sheet {
  readonly workbook: Workbook;
  /** The tab position the sheet had when it was handed out. */
  readonly index: number;
  readonly book: Book;
  readonly name: string;
  rename(name: string): this;
  /** One cell; a string starting with `=` is a formula. */
  set(address: string, value: CellValue): this;
  get(address: string): CellValue;
  /** What the cell shows, through its number format. */
  text(address: string): string;
  /** A block of values whose top left corner lands on `at`. */
  write(at: string, rows: CellGrid): this;
  range(address: string): SheetRange;
  style(address: string, patch: CellStylePatch): this;
  /** Column width in characters; the column is a letter or a number from 1. */
  width(column: string | number, characters?: number): this;
  /** Freezes the rows above and the columns to the left; zeros unfreeze. */
  freeze(rows: number, columns?: number): this;
  /** A table over a range already filled. */
  addTable(name: string, address: string, options?: { header?: boolean }): Table;
  /** Writes `rows` from `at` - the first is the header - and draws a table over them. */
  addTableFromData(name: string, at: string, rows: CellGrid): Table;
  table(name: string): Table;
  tables(): Table[];
  rowHidden(row: number): boolean;
}

export declare class Table {
  readonly sheet: Sheet;
  readonly name: string;
  readonly book: Book;
  readonly columns: string[];
  info(): SheetTable;
  /** The whole table, header and totals included; with an address, a range of the sheet. */
  range(address?: string): SheetRange;
  /** The data rows, without the header and totals. */
  data(): SheetRange;
  /** The sheet address of a data cell: `row` from 1, `column` a header or a number from 1. */
  address(row: number, column: string | number): string;
  columnIndex(header: string): number;
  set(row: number, column: string | number, value: CellValue): this;
  get(row: number, column: string | number): CellValue;
  /** Filters by a column and hides the rows that fail, as Excel does. */
  addFilter(column: string | number, criteria: TableFilter): this;
  /** Takes the criterion off one column, or off all of them. */
  clearFilter(column?: string | number): this;
  /** `["Region", "-Amount"]`: a minus sorts largest first. */
  sort(keys: SortKeys): this;
  /** The data rows keyed by header; `visible` leaves out hidden rows. */
  records(options?: { visible?: boolean }): Record<string, CellValue>[];
}

export declare class SheetRange {
  readonly sheet: Sheet;
  readonly address: string;
  readonly book: Book;
  values(): CellGrid;
  /** The values of the rows a filter or a user has not hidden. */
  visibleValues(): CellGrid;
  /** Writes a block from the top left corner of the range. */
  set(rows: CellGrid): this;
  style(patch: CellStylePatch): this;
  merge(): this;
}
