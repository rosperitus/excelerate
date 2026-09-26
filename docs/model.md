# The workbook model

Three types carry almost everything: `Spreadsheet`, `Worksheet`, `Cell`.

```text
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

```text
pub enum CellValue {
    Empty,
    Number(f64),          // dates live here too; the format makes them dates
    Text(Arc<str>),      // shared: one allocation per distinct string
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
sheet.set(at("A4"), CellValue::formula("SUM(A1:A3)"));   // no result yet
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
sheet, without a walk: the sheet keeps its column span as cells arrive. Only
after a cell in the leftmost or rightmost column is removed does it walk the
rows to narrow the span. `dimension_hint()` never walks: same rows, columns as
an upper bound.

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

Writing one is a setter, not a push:

```rust
# use excelerate::{Col, Row};
# use excelerate::model::Worksheet;
# let mut sheet = Worksheet::new("S")?;
sheet.set_column_width(Col::from_one_based(3)?, Some(32.0));   // None: back to the default
sheet.set_column_hidden(Col::from_one_based(4)?, true);
sheet.set_row_height(Row::from_one_based(1)?, Some(28.0));
sheet.set_row_hidden(Row::from_one_based(2)?, true);
# Ok::<(), excelerate::Error>(())
```

`set_column_width` and `set_column_hidden` go through `column_entry`, which
splits a run covering several columns around the one being written, so its
neighbours keep the width they had and the file gets no overlapping `<col>`
elements.

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
Excel cached next to it. Series fill and line (`ShapeFormat`), markers, data
points, data labels, and a stock chart's high-low lines and up/down bars are
fields of the model; each keeps the element it was read from, so gradients,
effects and extensions the model does not name survive an edit. Fonts,
trend lines and the rest stay in the `markup` fields as the XML they were
written in.

The cached values are what a program drawing the chart reads, and they go
stale when the cells change. `formula::chart::refresh_caches` reads them again
the way Excel does - numbers with gaps left out, labels as the cells show
them, hidden cells skipped when the chart plots visible cells only - either
for every chart or only for the references that cover the cells an edit
touched. A cache that comes out the same is left alone, so the chart still
goes back byte for byte.

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
// Fill in the caches from the cells: one chart changed.
assert_eq!(excelerate::formula::chart::refresh_caches(&mut book, None), 1);
```

Two limits. A chart inside a group of shapes is positioned by the group, so
changing its anchor is not written; removing it is. A plot that needs axes and
names fewer than two existing ones makes the write fail instead of producing a
file Excel repairs.

### 2016 charts

Waterfall, funnel, treemap, sunburst, histogram, box and whisker and region map
are a different schema, read into `Worksheet::extended_charts` as `ChartEx`:
name, anchor, title and series, each series with its layout, name, visibility
and the data it reads (`Dimension`, one per role). Excel writes a hidden
defined name such as `_xlchart.v1.0` in place of the range;
`Dimension::reference` looks through it.

These are written by comparison too, but a changed one is edited inside its
part rather than rendered, because the part is mostly formatting the model does
not name: a colour per data point, label styles, subtotal bars. The title, each
series' layout, name, visibility and data are rewritten; the rest stays. A
removed chart is cut from the drawing, and one built in code gets Excel's
defaults for its kind.

```rust
# use excelerate::model::Spreadsheet;
# use excelerate::model::chart::{Anchor, ChartEx, ChartText, Dimension, DimensionRole, Marker, SeriesLayout};
let mut book = Spreadsheet::new();
let dimension = |role, numeric, formula: &str| Dimension {
    role,
    numeric,
    formula: Some(formula.into()),
    levels: Vec::new(),
};
let data = vec![
    dimension(DimensionRole::Categories, false, "Worksheet!$A$2:$A$5"),
    dimension(DimensionRole::Values, true, "Worksheet!$B$2:$B$5"),
];
let anchor = Anchor::OneCell { from: Marker::default(), width: 4_572_000, height: 2_743_200 };
let mut funnel = ChartEx::new(SeriesLayout::Funnel, data, anchor);
funnel.title = Some(ChartText::text("Pipeline"));
if let Some(sheet) = book.sheet_mut(0) {
    sheet.extended_charts.push(funnel);
}
```

A removed 2016 chart inside a group keeps its frame. What Excel draws for one
built in code has not been checked against Excel itself: no other program on
hand reads these charts.

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

## Shapes

`Worksheet::shapes` holds one `Shape` per `<xdr:sp>` in the sheet's drawing -
boxes, arrows, callouts, text boxes: its name, alt text, anchor, preset outline
(`geometry`, such as `rect` or `rightArrow`; `None` for a freeform one) and the
text inside, a line per paragraph. Fill, line, effects and the formatting of the
text stay in the drawing as written.

Writing works as it does for pictures. An untouched shape goes back byte for
byte; a moved one gets a new anchor around the same element; a new outline is
one attribute; new text replaces the paragraphs and keeps the first run's
formatting, so it looks like the old. A shape removed from the list is removed
from the drawing, and one built in code is added with Excel's default look.

```rust
# use excelerate::model::Spreadsheet;
# use excelerate::model::chart::{Anchor, Marker};
# use excelerate::model::shape::Shape;
let mut book = Spreadsheet::new();
let marker = |col, row| Marker {
    col: excelerate::Col::new(col).unwrap_or_default(),
    row: excelerate::Row::new(row).unwrap_or_default(),
    ..Marker::default()
};
let anchor = Anchor::TwoCell { from: marker(1, 1), to: marker(4, 4), edit_as: None };
let mut note = Shape::new("wedgeRectCallout", anchor);
note.text = "Check the totals\nbefore sending".into();
if let Some(sheet) = book.sheet_mut(0) {
    sheet.shapes.push(note);
}
```

Connectors (`<xdr:cxnSp>`) are not shapes here and stay in the drawing. A shape
inside a group is placed by the group (`ShapeOrigin::grouped`), so moving it is
not written; renaming, retexting or removing it is.

## Pivot tables

`Worksheet::pivot_tables` holds the reports on a sheet (`PivotTable`: name,
location, the fields on each axis, the values area with its functions, style,
grand totals), and `Spreadsheet::pivot_caches` the data they read
(`PivotCache`: source sheet and range, or a name, and the columns).

An untouched report or cache goes back byte for byte. One changed or built in
code is written the way excelize writes a new pivot: fields on their axes, the
values area and a placeholder for the laid-out rows, with the cache marked
`refreshOnLoad` so the application that opens the file lays the report out
from the source range. A changed cache is written without its records. A
report removed from the sheet takes its part with it.

```rust
# use excelerate::Range;
# use excelerate::model::Spreadsheet;
# use excelerate::model::pivot::{CacheField, CacheSource, DataField, PivotCache, PivotTable, Subtotal};
# fn main() -> Result<(), excelerate::Error> {
let mut book = Spreadsheet::new();
book.pivot_caches.push(PivotCache {
    id: 1,
    source: CacheSource {
        sheet: Some("Worksheet".into()),
        range: Some(Range::parse("A1:C100")?),
        name: None,
    },
    fields: ["Region", "Product", "Sales"]
        .iter()
        .map(|name| CacheField { name: (*name).into(), ..CacheField::default() })
        .collect(),
    ..PivotCache::default()
});
if let Some(sheet) = book.sheet_mut(0) {
    sheet.pivot_tables.push(PivotTable {
        name: "Sales by region".into(),
        cache_id: 1,
        location: Some(Range::parse("E1:G20")?),
        row_fields: vec![0],
        data_fields: vec![DataField { field: 2, subtotal: Subtotal::Sum, ..DataField::default() }],
        row_grand_totals: true,
        column_grand_totals: true,
        ..PivotTable::default()
    });
}
# Ok(())
# }
```

A report pointing at a cache the workbook does not have, or without a
location, makes the write fail. The fields' items are written from the
cache's shared items, so a hidden or reordered item of a changed report is
shown again in cache order.

## Carried parts

`OpaquePart` and `Attachment` are the escape hatch. Anything the crate does not
model, such as shapes, comments and their VML, and document properties, is
carried as raw bytes plus the relationship pointing at it, recursively. Chart
parts, drawings and media are carried as well, which is what lets an untouched
chart or picture go back byte for byte.

One part is deliberately *not* carried: `calcChain.xml`. It records the order
formulas were computed in, and after a rewrite it would be a lie.
