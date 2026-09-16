# The workbook model

Three types carry almost everything: `Spreadsheet`, `Worksheet`, `Cell`.

```
Spreadsheet
├── sheets: Vec<Worksheet>
│   ├── cells        (sparse, row-major)
│   ├── merges, columns, rows
│   ├── view, properties, margins, page setup
│   ├── data_validations, conditional_formats
│   ├── protection, protected_ranges, auto_filter
│   └── attachments  (drawings, comments - carried, not modelled)
├── styles: StyleTable          shared by every sheet
├── defined_names               named ranges and Excel's own `_xlnm.*`
├── external: Vec<ExternalBook> cached values of linked workbooks
├── epoch                       1900 or the Mac 1904 base date
└── parts, attachments, theme   everything carried through untouched
```

## Addresses

`CellRef` is a column plus a row, both 1-based and both validated:

```rust
use excelerate::{CellRef, Col, Range, Row};

let a1 = CellRef::parse("A1")?;
let same = CellRef::parse("$A$1")?;      // `$` is accepted and dropped
assert_eq!(a1, same);

// `new` takes 0-based indexes; `from_one_based` takes the numbers a user sees.
let b4 = CellRef::new(Col::from_one_based(2)?, Row::from_one_based(4)?);
assert_eq!(b4.to_string(), "B4");

// Ranges normalise their corners, so a backwards one still means what you
// meant: D9:B4 is B4:D9.
let range = Range::parse("D9:B4")?;
assert_eq!(range.to_string(), "B4:D9");
# Ok::<(), excelerate::Error>(())
```

`A0` is an error, not a shrug - row 0 does not exist, and letting it slide only
moves the bug downstream.

## Cell values

```rust
pub enum CellValue {
    Empty,
    Number(f64),          // dates live here too; the format makes them dates
    Text(String),
    Bool(bool),
    Error(CellError),     // #DIV/0!, #N/A, ...
    RichText(Vec<TextRun>),   // formatting that changes mid-string
    Formula { formula: String, cached: Option<Box<CellValue>> },
}
```

Two things worth internalising:

- **Dates are numbers.** `45000` is a date only because the cell's number
  format says so. Convert with `shared::date`.
- **A formula carries its last result.** That is what lets you read a workbook
  without evaluating a thing - and why [formulas.md](formulas.md) has a section
  on when that cache is worth trusting.

Setting values is `impl Into<CellValue>`, so the common cases are short:

```rust
# use excelerate::CellRef;
# use excelerate::model::{CellValue, Worksheet};
# let mut sheet = Worksheet::new("S")?;
# let at = |a: &str| CellRef::parse(a).unwrap();
sheet.set(at("A1"), 42.0);          // number
sheet.set(at("A2"), "hello");       // text
sheet.set(at("A3"), true);          // bool
sheet.set(at("A4"), CellValue::Formula {
    formula: "SUM(A1:A3)".into(),
    cached: None,
});
# Ok::<(), excelerate::Error>(())
```

`set` keeps the cell's existing style. To touch the style as well, grab the
whole cell with `entry`:

```rust
# use excelerate::CellRef;
# use excelerate::model::Worksheet;
# use excelerate::style::{Style, StyleTable};
# let mut sheet = Worksheet::new("S")?;
# let mut styles = StyleTable::default();
# let mut style = Style::default();
# style.font.bold = true;
let bold = styles.intern(style);
sheet.entry(CellRef::parse("A1")?).style = bold;
# Ok::<(), excelerate::Error>(())
```

## Sheets

```rust
# use excelerate::model::{Spreadsheet, Worksheet};
# let mut book = Spreadsheet::empty();
let index = book.add_sheet(Worksheet::new("Data")?)?;

book.sheet(index);                  // &Worksheet
book.sheet_mut(index);              // &mut Worksheet
book.sheet_by_name("Data");         // case-insensitive, like Excel
book.active_sheet();                // the tab the file was saved on
book.set_active(index)?;
# Ok::<(), excelerate::Error>(())
```

Cells are stored sparsely in a `BTreeMap`, so an empty sheet costs nothing and
`iter()` hands you only what exists, in the row-major order xlsx wants when it
is written back. `dimension()` gives the used range, or `None` for an empty
sheet.

## Column and row properties

Columns are stored as *runs*, the way the file writes them (`<col min="1"
max="16384" width="9.5"/>`), not expanded into 16k entries:

```rust
# use excelerate::{Col, Row};
# use excelerate::model::Worksheet;
# let sheet = Worksheet::new("S")?;
let width = sheet.column_width(Col::from_one_based(3)?);   // None -> sheet default
let height = sheet.row_height(Row::from_one_based(7)?);
# Ok::<(), excelerate::Error>(())
```

Later runs win over earlier ones covering the same column - that is how Excel
reads them, and reproducing it matters more than tidiness.

## Everything else on the sheet

`merges`, `hyperlinks`, `data_validations`, `conditional_formats`,
`protection`, `protected_ranges`, `auto_filter`, `view`, `margins`,
`page_setup`, `header_footer`, `row_breaks`, `col_breaks` are all plain public fields. Read
them, mutate them, write the book back - no builders, no setters that only
assign.

## Charts

`Worksheet::charts` holds one `Chart` per chart frame in the sheet's drawing:
its name, anchor, title, plots with their series, axes and legend. A series
reads its cells through a formula (`DataSource::formula`) and keeps the values
Excel cached next to it. Fills, fonts and label positions are not modelled;
they stay in the `markup` fields as the XML they were written in, so a series
edited through the model keeps its colour.

The writer compares each chart with the copy taken when it was read. An
unchanged chart goes back as its original bytes. A changed one is rendered from
the model and its markup, and a chart built in code gets the elements Excel
writes for a new chart.

```rust
# use excelerate::model::Spreadsheet;
# use excelerate::model::chart::{
#     BarDirection, Chart, ChartAxis, ChartText, DataSource, Grouping, Plot, PlotKind, Series,
# };
let mut book = Spreadsheet::new();
let mut plot = Plot::new(PlotKind::Bar {
    direction: BarDirection::Column,
    grouping: Grouping::Clustered,
    three_d: false,
});
plot.series.push(Series {
    name: Some(ChartText::text("Revenue")),
    categories: Some(DataSource::strings("Worksheet!$A$2:$A$5")),
    values: Some(DataSource::numbers("Worksheet!$B$2:$B$5")),
    ..Series::default()
});
plot.axis_ids = vec![1, 2];
if let Some(sheet) = book.sheet_mut(0) {
    sheet.charts.push(Chart {
        name: "Revenue".into(),
        plots: vec![plot],
        axes: vec![ChartAxis::category(1, 2), ChartAxis::value(2, 1)],
        ..Chart::default()
    });
}
```

Three limits. A chart inside a group of shapes is positioned by the group, so
changing its anchor is not written; removing it is. The 2016 chart types
(waterfall, funnel, treemap and the rest) are read into
`Worksheet::extended_charts` and carried as bytes, not written from the model.
A plot that needs axes and names fewer than two existing ones makes the write
fail instead of producing a file Excel repairs.

## Pictures

`Worksheet::images` holds one `Image` per picture in the sheet's drawing: its
bytes, format, anchor, name and alt text. Cropping, borders, effects and the SVG
a modern Excel keeps beside a PNG fallback stay in the drawing as written.

The writer compares each picture with what was read. An untouched one goes back
byte for byte. A moved one keeps its element and gets a new anchor, a renamed
one gets two attributes rewritten, and new bytes go into a media part of their
own (the SVG beside the old bytes is dropped, or it would be drawn instead). A
picture removed from the list is removed from the drawing; one built in code is
added to it, and a sheet with no drawing gets one.

```rust
# use excelerate::model::Spreadsheet;
# use excelerate::model::chart::{Anchor, Marker};
# use excelerate::model::image::Image;
# let png: Vec<u8> = b"\x89PNG\r\n\x1a\n".to_vec();
let mut book = Spreadsheet::new();
let from = Marker {
    col: excelerate::Col::new(1).unwrap_or_default(),
    row: excelerate::Row::new(2).unwrap_or_default(),
    ..Marker::default()
};
// 914 400 EMU to the inch: a picture one inch square at B3.
let anchor = Anchor::OneCell { from, width: 914_400, height: 914_400 };
if let (Some(mut logo), Some(sheet)) = (Image::new(png, anchor), book.sheet_mut(0)) {
    logo.description = "Company logo".into();
    sheet.images.push(logo);
}
```

`Image::new` tells the format from the bytes and returns `None` for a file Excel
would not show. A picture inside a group of shapes is placed by the group
(`ImageOrigin::grouped`), so moving it is not written; removing it is. A picture
linked to a file outside the package has no bytes and is not in the list; it
stays in the drawing as written.

## Carried parts

`OpaquePart` and `Attachment` are the escape hatch. Anything the crate does not
model, such as shapes, comments and their VML, and document properties, is
carried as raw bytes plus the relationship pointing at it, recursively. Chart
parts, drawings and media are carried as well, which is what lets an untouched
chart or picture go back byte for byte.

One part is deliberately *not* carried: `calcChain.xml`. It records the order
formulas were computed in, and after a rewrite it would be a lie.
