# Changelog

## Unreleased

### Breaking changes

- `CellValue::Text` holds `Arc<str>` instead of `String`. Cells read from the
  same shared string share one allocation. `CellValue::text` still builds one
  from any string; `CellValue::shared_text` takes an `Arc<str>` as it is.
- A zip package larger than the expansion cap is read when it expands no more
  than 100 times its compressed size. The cap alone refused a 70 MB export
  that expands to 634 MB and that Excel opens; the ratio is what tells such a
  workbook from a zip bomb.

### Added

- `PasswordHash::new`, `iso`, `legacy` and `verify`: set a sheet or workbook
  password from its text the way Excel does (SHA-512, random 16-byte salt,
  100,000 spins), and check a password against a hash a file carries.
  Verified against hashes `excelize` wrote.

### Fixed

- Array formulas keep their flag through an xlsx round trip:
  `<f t="array" ref="…">` is read into `Worksheet::array_formulas` and
  written back. Before, `{=…}` came back as a plain formula.
- Implicit intersection: a formula stored in a cell that answers with a range
  shows the cell of that range in its own row or column (`=A1:A9` in B5 is
  A5; `INDEX(A1:D6,2,0)` in column C is C2), and `#VALUE!` when its row
  misses the range. The same holds for a range met by an operator or passed
  to a value parameter (`=A1:A9*2`, `ABS(A1:A9)`, a defined name standing for
  a range); array parameters (`SUMPRODUCT`) and array formulas keep the range
  whole. A reference returned at run time by `IF`, `CHOOSE` or `OFFSET` is not
  narrowed.
- The `ARRAY` record of an xls file marks its formula as an array formula.

### Performance

- Reading a 70 MB xlsx of nine million cells takes 5.4 s and 770 MB (Excel
  2016: 7 s and 900 MB). Before, it was refused, and with the cap raised took
  7.3 s and 2.7 GB. Shared strings are no longer copied into each cell; the
  sheet and the shared string table are parsed as they inflate instead of
  being held as text; attributes are looked up in place instead of
  collected; and a sheet stores its cells as a sorted vector per row instead
  of one tree keyed by cell, each new row reserving the width of the one
  before. Walking every cell and computing the used range take a quarter of
  the time.

### Fixed

- The styles reader took the differential formats of slicer styles inside
  `<extLst>` for the book's own, and each save added them again: `chart1.xlsx`
  went from 50 to 98 to 146.

## 0.8.0

### Breaking changes

- `Value::Array` holds `Rc<Vec<Vec<Value>>>` instead of `Vec<Vec<Value>>`, so
  a range read once is shared by every formula that reads it. Build one with
  `Value::array(rows)`; take the rows out with `Rc::unwrap_or_clone`.
- `Expr::Range` has a new field, `anchors`, recording which parts of the
  reference were written with `$`. Patterns that list the fields need `..`.
- A reference to a cell whose formula computed an array reads the cell's
  top-left value, as Excel shows it. `Engine::spilled` (and `A1#` in a
  formula) returns the whole array.
- An error in a function argument is the function's answer, whichever error
  it is. `RANDBETWEEN(#REF!, 1)` is `#REF!`, where it was `#VALUE!`.
- `AVERAGEA`, `MAXA`, `MINA`, `STDEVA`, `STDEVPA`, `VARA` and `VARPA` return
  `#VALUE!` for text written directly as an argument, as Excel 2016 does.
  Text read from cells still counts as zero.
- Approximate `MATCH`, `VLOOKUP`, `HLOOKUP` and `LOOKUP` search by halving,
  among values of the looked-up value's type. On an unsorted list the answer
  is the one Excel gives, which can differ from the first match a linear scan
  found.
- Financial functions that walk a schedule period by period (`DB`, `DDB`,
  `VDB`, `IPMT`, `PPMT`, `CUMIPMT`, `CUMPRINC`, `AMORDEGRC`) return `#NUM!`
  past one million periods.
- Arrays a formula builds are capped at four million cells, the cap a
  reference already had. `EXPAND`, `RANDARRAY`, element-wise operators and
  lifted functions return `#NUM!` beyond it.
- Dates past 31 December 9999 or before the start of the calendar are
  `#NUM!` in every function that reads a date.

### Added

- xls formulas are read as text, beside their cached results: relative and
  absolute references, 3D references, add-in and newer functions, array
  constants, shared formulas expanded per cell, array formulas on their first
  cell, and defined names.
- xls formulas are written as BIFF8 tokens with their results, and defined
  names as `NAME` records. What BIFF8 cannot hold (a structured reference, a
  reference past row 65536 or column IV) is written as its value.
- The whole xls cell format is read and written: fonts, fills, borders,
  alignment and protection. Palette colours resolve to RGB on reading; the
  writer lays out its own palette, and resolves theme colours through the
  workbook's theme.
- Pictures: `Worksheet::images` models each embedded picture of a sheet's
  drawing (bytes, format, anchor, name, alt text). An untouched picture is
  written back byte for byte; a moved, renamed, replaced, removed or new one
  is spliced into the drawing. Inserting rows moves pictures with the grid.
- Functions with a value parameter are lifted over arrays, the way array
  formulas need: `ISNUMBER(A1:A3)` returns three answers. `IF`, `IFERROR` and
  `IFNA` work element by element on arrays.
- `INDIRECT` follows a defined name that stands for a reference.
- `examples/why.rs` follows a formula that disagrees with its cached result
  to the cells it reads, down to the ones whose inputs agree.
- `tests/corpus.rs`, an ignored test over the workbooks in `tests/corpus/`
  (kept out of git), with floors for how many formulas agree with the cache.
- `fuzz/`, three `cargo-fuzz` targets: any bytes through format detection,
  an xls stream, and formula text.

### Fixed

- Functions stored under the `_xlfn.` prefix, which Excel writes for every
  function added after 2007, answered `#NAME?`.
- An `.xlsm` was written with the content type of a plain workbook, which
  Excel refuses to open.
- Conditional formats and validations inside a sheet's `<extLst>` were read
  as the sheet's own, and written back as a rule with `sqref=""`.
- xls files over 7 MB were written without DIFAT sectors, losing everything
  past the first 7 MB.
- A reference to a cell holding an array formula counted the whole array
  again: `COUNTIF` over such a column returned 110 for 49 cells.
- A leading `+` converted text to a number: `=+Sheet!A1` on text was
  `#VALUE!`.
- Aggregates skip text inside computed arrays, as inside references:
  `SUM({1,"2",TRUE})` is 1.
- `NPER` refused a present value of zero.
- `DATEDIF` with `"MD"` counted from the previous month when the day had
  already come.
- `XNPV` and `XIRR` dropped a non-numeric date and paired the rest with the
  wrong cash flows; they return `#VALUE!`.
- `TBILLEQ` returned a negative yield for a discount that makes the bill
  worthless; it returns `#NUM!`.
- `IRR` and `XIRR` stopped at a tolerance of 1e-8; they now agree with
  Excel to the sixteenth digit.
- `COUNTIF` and its family counted nothing, instead of failing, when the
  range itself was an error.
- xls writing took `GETPIVOTDATA` and `RTD` to have at most 2 and 5
  arguments, and wrote a formula calling them with more as its value.
- Crashes and hangs found by fuzzing: a CSV whose first line has a
  multi-byte character in its first four bytes; an xls run of cells starting
  near the last column; a stray `]` in a structured reference, which looped
  forever; `INDEX` with a fractional position below one; `OFFSET` and
  `EDATE` with counts near the integer limits.
- A CSV cell holding an uncached array formula was written as the debug form
  of its value, `Number(1.0)`.

### Performance

- Full recalculation of a workbook with 650,000 formulas takes 16 s and
  1.4 GB. Before, it did not finish, and used 12 GB. The dependency order goes
  through one node per distinct range; ranges read are cached; names are
  indexed and their formulas parsed once; recalculation reuses the trees
  parsed for the index; text compares without allocating.
- Reading a 30 MB xls took 8.7 s and takes 0.4 s: a record break inside a
  shared string is found by binary search.
- CSV writing visits only the cells each row has, instead of looking up every
  field of the used range.
- `CUMIPMT` and `CUMPRINC` walk the schedule once instead of once per period.
- `BINOM.INV` and `CRITBINOM` search for the count, and
  `BINOM.DIST.RANGE` subtracts two distribution values, instead of summing
  millions of terms.

## 0.1.0

First version: reading and writing xlsx, xls, ods, csv and html; reading
SYLK, Gnumeric and SpreadsheetML 2003; the formula engine; styles, charts,
tables, protection, autofilters and comments; pivot tables on read; the
WebAssembly packages.
