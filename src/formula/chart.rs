//! Chart caches read again from the cells.
//!
//! A chart part keeps, beside each reference, the numbers and labels the
//! cells held when the file was saved, and a program drawing the chart reads
//! those rather than the cells. Once the cells change the caches are stale
//! until something reads them again - which is what Excel does on every edit,
//! and what [`refresh_caches`] does here.

use crate::coordinate::{CellRef, Col, Range, Row};
use crate::formula::eval::{same_name, spanned};
use crate::formula::parser::{BinaryOp, Expr, parse};
use crate::model::Spreadsheet;
use crate::model::chart::{Chart, ChartText, DataSource};
use crate::reader::chart::children;
use crate::reader::zipxml::is_true;
use crate::style::format::GENERAL;

/// The rectangles a reference covers, each with its sheet, in order.
type Areas = Vec<(usize, Range)>;

/// How many names a reference may pass through before it is given up: a
/// name that stands for itself would otherwise never end.
const MAX_DEPTH: u8 = 16;

/// Reads the caches of chart series and titles again from the cells their
/// references point at, and returns how many charts changed.
///
/// With `changed` - the cells an edit touched, by sheet index - only the
/// references covering one of them are read; `None` reads them all. What is
/// cached is what Excel caches: the numbers of a numeric source, and for
/// labels a number as its format shows it (through [`crate::style::format`])
/// and text as it is; empty cells are gaps, and the point count is the whole
/// reference. Under `c:plotVisOnly` - a new chart's default - hidden rows and
/// columns are left out and the points numbered among the visible ones. A
/// numeric cache keeps the format code it was read with; one that had none
/// takes its first cell's. A reference may be a range, a union such as
/// `(Sheet1!$A$1:$A$3,Sheet1!$C$1:$C$3)` or a defined name.
///
/// A cache that still says the same is left alone, so a chart whose cells did
/// not change still counts as unchanged and goes back to its file as it was
/// read. References that point outside the book, multi-level categories and
/// the data of 2016 charts are not read.
pub fn refresh_caches(book: &mut Spreadsheet, changed: Option<&[(usize, CellRef)]>) -> usize {
    let mut count = 0;
    for sheet in 0..book.sheets().len() {
        let charts = book.sheet(sheet).map_or(0, |s| s.charts.len());
        for index in 0..charts {
            // Taken out while it is read against the book, and put back.
            let Some(slot) = book.sheet_mut(sheet).and_then(|s| s.charts.get_mut(index)) else {
                continue;
            };
            let mut chart = core::mem::take(slot);
            let touched = refresh_chart(book, sheet, &mut chart, changed);
            if let Some(slot) = book.sheet_mut(sheet).and_then(|s| s.charts.get_mut(index)) {
                *slot = chart;
            }
            count += usize::from(touched);
        }
    }
    count
}

fn refresh_chart(
    book: &Spreadsheet,
    sheet: usize,
    chart: &mut Chart,
    changed: Option<&[(usize, CellRef)]>,
) -> bool {
    let reader = Reader {
        book,
        sheet,
        changed,
        visible_only: visible_only(chart),
    };
    let mut touched = false;
    let titles = chart
        .title
        .iter_mut()
        .chain(chart.axes.iter_mut().filter_map(|a| a.title.as_mut()))
        .filter_map(|t| t.text.as_mut());
    for text in titles {
        touched |= reader.text(text);
    }
    for series in chart.plots.iter_mut().flat_map(|p| &mut p.series) {
        if let Some(name) = series.name.as_mut() {
            touched |= reader.text(name);
        }
        let sources = [
            &mut series.categories,
            &mut series.values,
            &mut series.bubble_sizes,
        ];
        for source in sources.into_iter().flatten() {
            touched |= reader.source(source);
        }
    }
    touched
}

/// Whether the chart plots only the cells that are not hidden
/// (`c:plotVisOnly`), which is what Excel gives a new chart. Its caches then
/// hold only those cells, numbered among themselves.
fn visible_only(chart: &Chart) -> bool {
    let tail = &chart.markup.after_legend;
    if chart.origin.is_none() && tail.is_empty() {
        return true;
    }
    children(tail)
        .iter()
        .find(|n| n.name == "plotVisOnly")
        .is_some_and(|n| n.attr("val").is_none_or(is_true))
}

struct Reader<'a> {
    book: &'a Spreadsheet,
    /// The chart's own sheet, which a reference without one means.
    sheet: usize,
    changed: Option<&'a [(usize, CellRef)]>,
    visible_only: bool,
}

impl Reader<'_> {
    /// The areas of `formula`, when the edit touched one of them.
    fn areas(&self, formula: &str) -> Option<Areas> {
        let expr = parse(formula).ok()?;
        let mut out = Vec::new();
        if !collect(self.book, self.sheet, &expr, 0, &mut out) {
            return None;
        }
        let touched = self.changed.is_none_or(|cells| {
            cells.iter().any(|(sheet, at)| {
                out.iter()
                    .any(|(s, range)| s == sheet && range.contains(*at))
            })
        });
        touched.then_some(out)
    }

    /// What a cell shows in a label: a number through its format, text as
    /// it is - Excel caches `Name`, not the padded text an accounting format
    /// would draw.
    fn shown(&self, sheet: usize, at: CellRef) -> String {
        let value = self
            .book
            .sheet(sheet)
            .and_then(|s| s.get(at))
            .map(|c| &c.value);
        match value.and_then(|v| v.result().plain_text()) {
            Some(text) => text,
            None => self.book.formatted(sheet, at),
        }
    }

    fn text(&self, text: &mut ChartText) -> bool {
        let ChartText::Reference { formula, cache } = text else {
            return false;
        };
        let Some(areas) = self.areas(formula) else {
            return false;
        };
        // A name over two cells shows both.
        let mut words = Vec::new();
        for_cells(self.book, &areas, false, |sheet, at, _| {
            let shown = self.shown(sheet, at);
            if !shown.is_empty() {
                words.push(shown);
            }
        });
        let fresh = (!words.is_empty()).then(|| words.join(" "));
        replace(cache, fresh)
    }

    fn source(&self, source: &mut DataSource) -> bool {
        let Some(areas) = source.formula().and_then(|f| self.areas(f)) else {
            return false;
        };
        match source {
            DataSource::Numbers {
                format_code,
                count,
                points,
                ..
            } => {
                let mut fresh = Vec::new();
                let size = for_cells(self.book, &areas, self.visible_only, |sheet, at, index| {
                    let cell = self.book.sheet(sheet).and_then(|s| s.get(at));
                    if let Some(n) = cell.and_then(|c| c.value.as_number()) {
                        fresh.push((index, n));
                    }
                });
                let changed = replace(count, Some(size)) | replace(points, fresh);
                // The format a file gave is kept: Excel does not always write
                // the first cell's, and a cache that did not change must not
                // change the chart.
                if changed && format_code.is_none() {
                    *format_code = Some(self.first_format(&areas).to_owned());
                }
                changed
            }
            DataSource::Strings { count, points, .. } => {
                let mut fresh = Vec::new();
                let size = for_cells(self.book, &areas, self.visible_only, |sheet, at, index| {
                    let shown = self.shown(sheet, at);
                    if !shown.is_empty() {
                        fresh.push((index, shown));
                    }
                });
                replace(count, Some(size)) | replace(points, fresh)
            }
            DataSource::Levels { .. } => false,
        }
    }

    /// The number format of the first cell a reference reads.
    fn first_format(&self, areas: &Areas) -> &str {
        areas.first().map_or(GENERAL, |(sheet, range)| {
            self.book
                .sheet(*sheet)
                .and_then(|s| s.get(range.start))
                .and_then(|c| self.book.styles.get(c.style))
                .map_or(GENERAL, |s| s.number_format.code())
        })
    }
}

/// Sets `slot` to `value` if they differ, and says whether they did.
fn replace<T: PartialEq>(slot: &mut T, value: T) -> bool {
    let differs = *slot != value;
    if differs {
        *slot = value;
    }
    differs
}

/// Gathers the areas `expr` covers; false for anything that is not a
/// reference into this book.
fn collect(book: &Spreadsheet, own: usize, expr: &Expr, depth: u8, out: &mut Areas) -> bool {
    match expr {
        Expr::Binary(BinaryOp::Union, a, b) => {
            collect(book, own, a, depth, out) && collect(book, own, b, depth, out)
        }
        Expr::Range { .. } | Expr::Binary(BinaryOp::Span, ..) => {
            let Some((sheet, range)) = spanned(expr) else {
                return false;
            };
            let Some(index) = sheet_index(book, own, sheet.as_deref()) else {
                return false;
            };
            out.push((index, range));
            true
        }
        Expr::Name(name) if depth < MAX_DEPTH => {
            // `Sheet1!Name` looks in that sheet's names; Excel writes a
            // workbook's own name as `[0]!Name`, which no sheet matches.
            let (scope, bare) = match name.rsplit_once('!') {
                Some((qualifier, bare)) => (sheet_index(book, own, Some(qualifier)), bare),
                None => (Some(own), name.as_str()),
            };
            let names = &book.defined_names;
            let found = names
                .iter()
                .find(|n| n.sheet.is_some() && n.sheet == scope && same_name(&n.name, bare))
                .or_else(|| {
                    names
                        .iter()
                        .find(|n| n.sheet.is_none() && same_name(&n.name, bare))
                });
            found
                .and_then(|n| parse(&n.formula).ok())
                .is_some_and(|e| collect(book, own, &e, depth + 1, out))
        }
        _ => false,
    }
}

fn sheet_index(book: &Spreadsheet, own: usize, sheet: Option<&str>) -> Option<usize> {
    let Some(name) = sheet else {
        return Some(own);
    };
    let name = name
        .strip_prefix('\'')
        .and_then(|n| n.strip_suffix('\''))
        .map_or_else(|| name.to_owned(), |n| n.replace("''", "'"));
    book.sheets()
        .iter()
        .position(|s| same_name(s.title(), &name))
}

/// Calls `f` with each stored cell of the areas and its point index - its
/// place among the areas' cells row by row, hidden ones left out when
/// `visible_only` - and returns how many points there are. The empty
/// stretches of a whole column or row are counted, not walked.
fn for_cells(
    book: &Spreadsheet,
    areas: &Areas,
    visible_only: bool,
    mut f: impl FnMut(usize, CellRef, u32),
) -> u32 {
    let mut base: u64 = 0;
    for (sheet, range) in areas {
        let Some(ws) = book.sheet(*sheet) else {
            continue;
        };
        let (r0, r1) = (range.start.row, range.end.row);
        let hidden_rows = |from: Row, to: Row| -> u64 {
            if !visible_only || from > to {
                return 0;
            }
            let hidden = ws.rows.range(from..=to).filter(|(_, p)| p.hidden).count();
            u64::try_from(hidden).unwrap_or(u64::MAX)
        };
        // At most the sheet's 16 384 columns, whatever the file says.
        let cols: Vec<Col> = (range.start.col.index()..=range.end.col.index())
            .filter_map(Col::new)
            .filter(|&c| !(visible_only && ws.column_run(c).is_some_and(|run| run.hidden)))
            .collect();
        let width = u64::try_from(cols.len()).unwrap_or(u64::MAX);
        let rows = u64::from(r1.index().saturating_sub(r0.index())) + 1;
        let height = rows.saturating_sub(hidden_rows(r0, r1));
        if let Some(used) = ws.dimension_hint() {
            let first = r0.max(used.start.row);
            let last = r1.min(used.end.row);
            let skipped = first.index().saturating_sub(r0.index());
            let before = first.index().checked_sub(1).and_then(Row::new);
            let mut row_index = u64::from(skipped)
                - before
                    .map_or(0, |b| hidden_rows(r0, b))
                    .min(u64::from(skipped));
            let from = cols.partition_point(|&c| c < used.start.col);
            let to = cols.partition_point(|&c| c <= used.end.col);
            for r in first.index()..=last.index() {
                let Some(row) = Row::new(r) else {
                    break;
                };
                if visible_only && ws.rows.get(&row).is_some_and(|p| p.hidden) {
                    continue;
                }
                for (k, &col) in (from..).zip(&cols[from..to.max(from)]) {
                    let at = CellRef::new(col, row);
                    if ws.get(at).is_none() {
                        continue;
                    }
                    let k = u64::try_from(k).unwrap_or(u64::MAX);
                    let Ok(index) = u32::try_from(base + row_index * width + k) else {
                        return u32::MAX;
                    };
                    f(*sheet, at, index);
                }
                row_index += 1;
            }
        }
        base = base.saturating_add(height.saturating_mul(width));
    }
    u32::try_from(base).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DefinedName, Worksheet};

    fn book() -> Spreadsheet {
        let mut book = Spreadsheet::empty();
        let mut sheet = Worksheet::new("My Data").unwrap();
        for (i, v) in [1.0, 2.0, 3.0].into_iter().enumerate() {
            let row = u32::try_from(i).unwrap();
            sheet.set(
                CellRef::new(Col::new(0).unwrap(), Row::new(row).unwrap()),
                v,
            );
            sheet.set(
                CellRef::new(Col::new(2).unwrap(), Row::new(row).unwrap()),
                v * 10.0,
            );
        }
        book.add_sheet(sheet).unwrap();
        book.add_sheet(Worksheet::new("Other").unwrap()).unwrap();
        book.defined_names.push(DefinedName {
            name: "Picked".into(),
            sheet: None,
            formula: "('My Data'!$A$1:$A$2,'My Data'!$C$3)".into(),
            hidden: false,
        });
        book
    }

    fn areas_of(book: &Spreadsheet, formula: &str) -> Option<Vec<(usize, String)>> {
        let expr = parse(formula).ok()?;
        let mut out = Vec::new();
        collect(book, 1, &expr, 0, &mut out).then(|| {
            out.iter()
                .map(|(s, r)| (*s, format!("{}:{}", r.start, r.end)))
                .collect()
        })
    }

    #[test]
    fn a_reference_is_a_range_a_union_or_a_name() {
        let book = book();
        let a = |s: &str| (0, s.to_owned());
        assert_eq!(
            areas_of(&book, "'My Data'!$A$1:$A$3"),
            Some(vec![a("A1:A3")])
        );
        assert_eq!(
            areas_of(&book, "('My Data'!$A$1:$A$3,'My Data'!$C$1:$C$3)"),
            Some(vec![a("A1:A3"), a("C1:C3")])
        );
        for name in ["Picked", "[0]!Picked"] {
            assert_eq!(
                areas_of(&book, name),
                Some(vec![a("A1:A2"), a("C3:C3")]),
                "{name}"
            );
        }
        // Without a sheet the chart's own is meant.
        assert_eq!(areas_of(&book, "$B$2"), Some(vec![(1, "B2:B2".to_owned())]));
        assert_eq!(areas_of(&book, "Missing"), None);
        assert_eq!(areas_of(&book, "[1]Sheet1!$A$1"), None);
    }

    #[test]
    fn a_union_numbers_its_points_across_its_areas() {
        let book = book();
        let mut out = Vec::new();
        let areas: Areas = vec![
            (0, Range::parse("A1:A3").unwrap()),
            (0, Range::parse("C1:C1048576").unwrap()),
        ];
        let count = for_cells(&book, &areas, false, |_, at, i| {
            out.push((i, at.to_string()));
        });
        assert_eq!(
            out,
            [
                (0, "A1".into()),
                (1, "A2".into()),
                (2, "A3".into()),
                (3, "C1".into()),
                (4, "C2".into()),
                (5, "C3".into())
            ]
        );
        assert_eq!(count, 3 + 1_048_576);
    }

    #[test]
    fn labels_show_numbers_formatted_and_text_as_is_and_hidden_rows_drop_out() {
        use crate::model::chart::{Plot, PlotKind, Series};
        use crate::style::{NumberFormat, Style};
        let mut book = book();
        let at = |s: &str| CellRef::parse(s).unwrap();
        let percent = book.styles.intern(Style {
            number_format: NumberFormat::Custom("0.0%".into()),
            ..Style::default()
        });
        let padded = book.styles.intern(Style {
            number_format: NumberFormat::Custom("_(@_)".into()),
            ..Style::default()
        });
        let sheet = book.sheet_mut(0).unwrap();
        sheet.set_styled(at("B1"), 0.25, percent);
        sheet.set_styled(at("B3"), "North", padded);
        sheet.set_row_hidden(Row::new(1).unwrap(), true);

        let mut plot = Plot::new(PlotKind::Line {
            grouping: crate::model::chart::Grouping::Standard,
            three_d: false,
        });
        plot.series.push(Series {
            categories: Some(DataSource::strings("'My Data'!$B$1:$B$3")),
            values: Some(DataSource::numbers("Picked")),
            name: Some(ChartText::Reference {
                formula: "'My Data'!$B$3".into(),
                cache: None,
            }),
            ..Series::default()
        });
        let chart = Chart {
            plots: vec![plot],
            ..Chart::default()
        };
        book.sheet_mut(1).unwrap().charts.push(chart);

        assert_eq!(refresh_caches(&mut book, None), 1);
        let series = &book.sheet(1).unwrap().charts[0].plots[0].series[0];
        // A new chart plots visible cells only: row 2 is not a point.
        assert_eq!(
            series.categories,
            Some(DataSource::Strings {
                formula: Some("'My Data'!$B$1:$B$3".into()),
                count: Some(2),
                points: vec![(0, "25.0%".into()), (1, "North".into())],
            })
        );
        assert_eq!(
            series.values,
            Some(DataSource::Numbers {
                formula: Some("Picked".into()),
                format_code: Some(GENERAL.into()),
                count: Some(2),
                points: vec![(0, 1.0), (1, 30.0)],
            })
        );
        assert_eq!(
            series.name.as_ref().and_then(ChartText::shown),
            Some("North")
        );
        // Read again, nothing is new; an edit elsewhere reads nothing.
        assert_eq!(refresh_caches(&mut book, None), 0);
        book.sheet_mut(0).unwrap().set(at("A1"), 5.0);
        assert_eq!(refresh_caches(&mut book, Some(&[(0, at("B9"))])), 0);
        assert_eq!(refresh_caches(&mut book, Some(&[(0, at("A1"))])), 1);
    }
}
