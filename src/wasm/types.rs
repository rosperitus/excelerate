//! The TypeScript names for what crosses the boundary as a `JsValue`.
//!
//! `wasm-bindgen` would otherwise type these `any`, which is no help to a
//! caller: a cell is one of four things and nothing else.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TYPES: &'static str = r#"
/** What a cell can hold on the JS side. An error cell reads as its code, `#DIV/0!`. */
export type CellValue = number | string | boolean | null;
/** A rectangle of cells, row by row, as `getRange` returns it. */
export type CellGrid = CellValue[][];
/**
 * One row with its styling, as `getRowAt` returns it. `formatted` is `null`
 * when the call asked for values alone. `bold` is one byte per cell, 0 or 1.
 */
export interface SheetRow {
  values: CellValue[];
  formatted: string[] | null;
  bold: Uint8Array;
  indent: Uint32Array;
  hidden: boolean;
}
/** Where a drawing object sits, in the 1-based numbers a user sees. */
export interface ObjectAnchor {
  kind: "twoCell" | "oneCell" | "absolute";
  row: number | null;
  column: number | null;
}
/** How a sheet is frozen and shown, as `sheetView` returns it. */
export interface SheetViewInfo {
  frozenRows: number;
  frozenColumns: number;
  zoom: number | null;
  showGridLines: boolean;
  showRowColHeaders: boolean;
  rightToLeft: boolean;
  topLeftCell: string | null;
}
/** A name that stands for a formula, as `definedNames` returns it. */
export interface WorkbookName {
  name: string;
  formula: string;
  sheet: number | null;
  hidden: boolean;
}
/** A rule limiting what a cell accepts, as `dataValidations` returns it. */
export interface SheetValidation {
  sqref: string[];
  type: string;
  operator: string;
  formula1: string;
  formula2: string;
  allowBlank: boolean;
  showDropDown: boolean;
}
/** One conditional formatting block, as `conditionalFormats` returns it. */
export interface SheetConditionalFormat {
  sqref: string[];
  rules: { type: string; priority: number; operator: string | null; formulas: string[]; text: string | null }[];
}
/** The autofilter over a range, as `autoFilter` returns it. */
export interface SheetAutoFilter {
  range: string;
  columns: { colId: number; kind: "values" | "custom" | "dynamic" | "top10" | "none" }[];
}
/** A pivot report on the sheet, as `pivotTables` returns it. */
export interface SheetPivotTable {
  name: string;
  location: string | null;
  cacheId: number;
  rowFields: number;
  columnFields: number;
  valueFields: number;
}
/** How a sheet or the workbook is locked, as `sheetProtection` returns it. */
export interface ProtectionInfo { locked: boolean; hasPassword: boolean }
/** How `toCsv` writes, and how `Book.readCsv` reads. */
export interface CsvOptions {
  delimiter?: string;
  contiguous?: boolean;
  preserveEmptyFields?: boolean;
}
/** A note on a cell, as `comments` returns it. */
export interface SheetComment { address: string; author: string; text: string }
/** A link over a cell or a block of them, as `hyperlinks` returns it. */
export interface SheetHyperlink {
  range: string;
  target: string;
  external: boolean;
  display: string | null;
  tooltip: string | null;
}
/** A table over a block of cells, as `tables` returns it. */
export interface SheetTable {
  name: string;
  displayName: string;
  range: string;
  headerRowCount: number | null;
  totalsRowCount: number | null;
  columns: string[];
}
/** A chart on the sheet, as `charts` returns it. */
export interface SheetChart {
  name: string;
  title: string | null;
  kinds: string[];
  seriesCount: number;
  anchor: ObjectAnchor;
}
/** A picture on the sheet, as `images` returns it. Bytes come from `imageData`. */
export interface SheetImage {
  name: string;
  description: string;
  format: string;
  byteLength: number;
  anchor: ObjectAnchor;
}
/** A drawn shape, as `shapes` returns it. */
export interface SheetShape {
  name: string;
  description: string;
  geometry: string | null;
  text: string;
  anchor: ObjectAnchor;
}
/**
 * A colour as the file states it: `null` when the file leaves it to the
 * reader, `#AARRGGBB` when it names one outright, and `indexed:N` or
 * `theme:N` when it points into the legacy palette or the workbook theme -
 * a theme colour carries its tint as `theme:4@-0.25`.
 */
export type StyleColor = string | null;
/** One side of a cell's border. */
export interface BorderSide { style: string; color: StyleColor }
/** How a cell is painted, as `cellStyle` returns it. */
export interface CellStyle {
  numberFormat: string;
  font: {
    name: string;
    size: number;
    bold: boolean;
    italic: boolean;
    underline: string;
    strike: boolean;
    color: StyleColor;
  };
  fill: { pattern: string; foreground: StyleColor; background: StyleColor };
  borders: {
    left: BorderSide;
    right: BorderSide;
    top: BorderSide;
    bottom: BorderSide;
  };
  alignment: {
    horizontal: string | null;
    vertical: string | null;
    wrapText: boolean;
    shrinkToFit: boolean;
    indent: number;
    textRotation: number;
  };
}
/**
 * The styles of a rectangle, as `getRangeStyles` returns them: each distinct
 * style once in `styles`, and `grid` - row by row - pointing into it.
 */
export interface RangeStyles {
  styles: CellStyle[];
  grid: number[][];
}
"#;

/// The shapes only the writing build takes, so a read-only package does not
/// declare types for methods it does not have.
#[cfg(feature = "write")]
#[wasm_bindgen(typescript_custom_section)]
const WRITE_TYPES: &'static str = r#"
/**
 * A style to write, as `setCellStyle` takes it. Every field is optional and
 * what is left out keeps the value the cell had, so `{ font: { bold: true } }`
 * makes a cell bold without touching its number format.
 */
export interface CellStylePatch {
  numberFormat?: string;
  font?: Partial<{
    name: string;
    size: number;
    bold: boolean;
    italic: boolean;
    underline: string;
    strike: boolean;
    color: StyleColor;
  }>;
  fill?: Partial<{ pattern: string; foreground: StyleColor; background: StyleColor }>;
  borders?: Partial<{
    left: Partial<BorderSide>;
    right: Partial<BorderSide>;
    top: Partial<BorderSide>;
    bottom: Partial<BorderSide>;
  }>;
  alignment?: Partial<{
    horizontal: string | null;
    vertical: string | null;
    wrapText: boolean;
    shrinkToFit: boolean;
    indent: number;
    textRotation: number;
  }>;
}
/**
 * Sort keys: a 1-based column number of the sheet (a row number when sorting
 * columns) or a header's text; a minus in front sorts largest first.
 */
export type SortKeys = (number | string)[];
/** How `sortRange` reads its range. */
export interface SortRangeOptions {
  /** The first row (or column) is a header: it stays put and names keys. */
  header?: boolean;
  /** Reorder columns left to right instead of rows. */
  byColumns?: boolean;
}
/**
 * Styles to write over a rectangle, as `setRangeStyles` takes them: `grid`
 * points into `styles`, and `null` or `-1` leaves its cell as it is. What
 * `getRangeStyles` returns fits here unchanged.
 */
export interface RangeStylesPatch {
  styles: CellStylePatch[];
  grid: (number | null)[][];
}
"#;
