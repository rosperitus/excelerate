# Styles and number formats

## The style table

Styles are interned once per workbook and referenced by id, the way the file
does it - a million cells sharing one look cost one `Style`.

```rust
use excelerate::CellRef;
use excelerate::model::{Spreadsheet, Worksheet};
use excelerate::style::{Color, NumberFormat, Style};

let mut book = Spreadsheet::empty();
let mut sheet = Worksheet::new("Report")?;

let mut style = Style::default();
style.font.bold = true;
style.font.set_size_points(14.0);
style.font.color = Color::Argb(0xFF19_4D33);
style.number_format = NumberFormat::Custom("#,##0.00 ₽".into());

let id = book.styles.intern(style);
sheet.set(CellRef::parse("A1")?, 1234.5);
sheet.entry(CellRef::parse("A1")?).style = id;

book.add_sheet(sheet)?;
# Ok::<(), excelerate::Error>(())
```

`intern` deduplicates: hand it the same `Style` twice and you get the same
`StyleId` back.

## What a style holds

```rust
pub struct Style {
    pub number_format: NumberFormat,
    pub font: Font,           // name, size, bold, italic, underline, colour, script
    pub fill: Fill,           // pattern, foreground, background
    pub borders: Borders,     // left/right/top/bottom/diagonal, each a Border
    pub alignment: Alignment, // horizontal, vertical, wrap, indent, rotation
    pub protection: Protection,
}
```

Colours come in three flavours, and the distinction matters on round-trip:

```rust
use excelerate::style::Color;

let explicit = Color::Argb(0xFFCC_0000);
let themed = Color::Theme { id: 9, tint: 400_000 };  // theme slot 9, tint in millionths
let indexed = Color::Indexed(64);                    // the legacy palette
# let _ = (explicit, themed, indexed);
```

A theme colour is an *index*, not an RGB value - swapping the workbook's theme
part repaints every cell using one. That is why the theme rides through
unparsed rather than being replaced with a canned one.

## Number formats

`NumberFormat::General`, `Builtin(id)` for the built-in ids, or `Custom(code)`
for anything else. To render a value the way a spreadsheet would:

```rust
use excelerate::shared::date::Epoch;
use excelerate::style::format::{Value, format};

assert_eq!(format(Value::Number(1234.5), "#,##0.00", Epoch::Windows1900), "1,234.50");
assert_eq!(format(Value::Number(0.256), "0.0%", Epoch::Windows1900), "25.6%");
assert_eq!(format(Value::Number(45000.0), "yyyy-mm-dd", Epoch::Windows1900), "2023-03-15");
```

The engine covers sections (positive; negative; zero; text), thousands
separators, percentages, scientific notation, dates and times, colour prefixes
and conditions. Fractions (`# ?/?`) and locale prefixes (`[$-409]`) are ignored
when rendering - but they survive a round trip, so nothing is lost from the
file.

The epoch matters only for dates: a workbook saved on a Mac counts from 1904,
and reading it as 1900 shifts every date by 1462 days. It is read from the file
and lives on `Spreadsheet::epoch`.

## Rich text

When formatting changes mid-string, the cell holds `RichText(Vec<TextRun>)` -
each run with its own partial font (`DiffFont`, where every field is optional,
so a run overriding just the colour does not silently impose a font name).

```rust
use excelerate::model::{CellValue, TextRun};
use excelerate::style::DiffFont;

let mut bold = DiffFont::default();
bold.bold = Some(true);

let value = CellValue::RichText(vec![
    TextRun { text: "Total: ".into(), font: None },
    TextRun { text: "480".into(), font: Some(bold) },
]);
// Formulas and CSV read the plain text of it.
assert_eq!(value.plain_text().as_deref(), Some("Total: 480"));
```

## Differential styles

Conditional formatting uses `DifferentialStyle` - a *partial* style where each
part is an `Option`. A `<dxf>` says what to change, not what the result should
be, so filling in a whole `Font` would impose a name and size the original
never asked for.

## Where styles go missing

- **ODS** has no per-cell format string; date and time formats are inferred
  from the value, custom codes do not survive.
- **HTML** and **ODS** lose theme and indexed palette colours - there is no
  equivalent concept.
- **xls** writes values, fonts, number formats and alignment, but not fills or
  borders: their colours are indexes into a 56-colour palette, and choosing the
  nearest entry for arbitrary RGB is a decision worth making on purpose.
