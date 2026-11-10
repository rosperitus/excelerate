//! Laying a pivot table's report out in the cells: what Excel's Refresh does.
//!
//! The report is built from its definition ([`PivotTable`]) and the source
//! range its cache names, and written as plain values - labels and numbers,
//! no formulas - in Excel's **tabular** form:
//!
//! - each row field has a label column of its own, its name over it; an
//!   item's label stands on the first row of its group only;
//! - an outer row field with `default_subtotal` gets a subtotal row under
//!   each of its groups, labelled "North Total" in that field's column;
//! - column fields go the same way across: their names in the top row, their
//!   items one header row per field under it;
//! - with two or more value fields the values field (`-2`) goes on the
//!   columns, as Excel puts it, unless the definition already places it;
//! - a grand total row when `column_grand_totals`, a grand total column when
//!   `row_grand_totals`, which is how Excel reads the two flags.
//!
//! Items sort ascending the way Excel's pivot sorts them: numbers, then text
//! without regard to case, then `FALSE`/`TRUE`, then errors, blanks last.
//! Text items that differ only in case are one item, spelled as first seen.
//!
//! A **calculated field** works on any axis: in the values area it is
//! aggregated by [`Subtotal`]'s functions over the values its formula makes
//! of each row, on the other axes it groups by its distinct computed
//! values. The formula - arithmetic over the other fields of the same row,
//! without functions - is [`CacheField::formula`], and
//! [`eval_pivot_formula`] evaluates one for the application.
//!
//! A **grouped** field spreads over the axis: a date grouping ([`Grouping`])
//! contributes one level per part, senior parts first, a number grouping
//! one span level, values outside the range falling into the rest group.
//! The labels of the levels come from [`PivotCaptions`], which carries the
//! words an English Excel writes.
//!
//! ponytail: the compact and outline forms, `show_all`, hidden items,
//! number formats on the values, sorting by value, grouping over seconds,
//! minutes or hours and a day grouping with a step above one day are not
//! laid out; a grouping over a discrete list of items (`discretePr`) reads
//! as no grouping. Excel lays the report out again on open anyway, as the
//! writer marks the cache `refreshOnLoad`.

use crate::coordinate::{CellRef, Col, Range, Row};
use crate::error::{CellError, Error, Result};
use crate::model::pivot::{
    CacheField, DatePart, PivotAxis, PivotField, PivotTable, RangeGroup, Subtotal,
};
use crate::model::{CellValue, Spreadsheet, Worksheet};
use crate::shared::date::{self, Epoch};
use std::cmp::Ordering;

/// The index the format gives the values field on an axis.
const VALUES: i32 = -2;

/// Most source cells a refresh reads: Excel's own limit is a million rows,
/// and this leaves room for a few dozen fields of them.
const MAX_SOURCE_CELLS: usize = 50_000_000;

/// The words a report is laid out with. `{}` in a template stands for the
/// field or item name. [`Default`] is what an English Excel writes; the
/// caller passes another language, which this crate does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PivotCaptions {
    /// The grand total's label: "Grand Total".
    pub grand_total: String,
    /// A subtotal's label: "{} Total", the item in place of `{}`.
    pub total: String,
    /// A grand total of one value field, when the values field shares the
    /// axis: "Total {}", the value field's caption in place of `{}`.
    pub total_of: String,
    /// What an empty source cell is shown as: "(blank)".
    pub blank: String,
    /// The values field's name, over the value fields' captions: "Values".
    pub values: String,
    /// The caption of a value field nobody named, per function, the source
    /// field's name in place of `{}`: "Sum of {}".
    pub sum: String,
    /// "Count of {}".
    pub count: String,
    /// "Average of {}".
    pub average: String,
    /// "Max of {}".
    pub max: String,
    /// "Min of {}".
    pub min: String,
    /// "Product of {}".
    pub product: String,
    /// "Count of {}", as Excel names a count of numbers too.
    pub count_nums: String,
    /// `"StdDev of {}"`.
    pub std_dev: String,
    /// `"StdDevp of {}"`.
    pub std_dev_p: String,
    /// "Var of {}".
    pub var: String,
    /// "Varp of {}".
    pub var_p: String,
    /// The header over a year level's labels: "Years".
    pub years: String,
    /// The header over a quarter level's labels: "Quarters".
    pub quarters: String,
    /// The header over a month level's labels: "Months".
    pub months: String,
    /// The header over a day level's labels: "Days".
    pub days: String,
    /// A quarter item's label, the number in place of `{}`: "Qtr1".
    pub quarter: String,
    /// What a value outside a span grouping's range is shown as: "(other)".
    pub other: String,
    /// A month item's label, January first: "Jan", "Feb", ...
    pub month_names: Vec<String>,
}

/// The English month labels, for a caller that passed fewer than twelve.
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

impl Default for PivotCaptions {
    fn default() -> Self {
        Self {
            grand_total: "Grand Total".into(),
            total: "{} Total".into(),
            total_of: "Total {}".into(),
            blank: "(blank)".into(),
            values: "Values".into(),
            sum: "Sum of {}".into(),
            count: "Count of {}".into(),
            average: "Average of {}".into(),
            max: "Max of {}".into(),
            min: "Min of {}".into(),
            product: "Product of {}".into(),
            count_nums: "Count of {}".into(),
            std_dev: "StdDev of {}".into(),
            std_dev_p: "StdDevp of {}".into(),
            var: "Var of {}".into(),
            var_p: "Varp of {}".into(),
            years: "Years".into(),
            quarters: "Quarters".into(),
            months: "Months".into(),
            days: "Days".into(),
            quarter: "Qtr{}".into(),
            other: "(other)".into(),
            month_names: MONTH_NAMES.iter().map(|m| (*m).into()).collect(),
        }
    }
}

impl PivotCaptions {
    /// A month item's label, January first.
    fn month_name(&self, month: u32) -> &str {
        let at = month.saturating_sub(1).min(11) as usize;
        self.month_names
            .get(at)
            .map_or(MONTH_NAMES[at], String::as_str)
    }

    /// The header over a date part's labels: "Years" for the years.
    fn part_name(&self, part: DatePart) -> &String {
        match part {
            DatePart::Years => &self.years,
            DatePart::Quarters => &self.quarters,
            DatePart::Months => &self.months,
            DatePart::Days => &self.days,
        }
    }

    const fn function(&self, f: Subtotal) -> &String {
        match f {
            Subtotal::Sum => &self.sum,
            Subtotal::Count => &self.count,
            Subtotal::Average => &self.average,
            Subtotal::Max => &self.max,
            Subtotal::Min => &self.min,
            Subtotal::Product => &self.product,
            Subtotal::CountNums => &self.count_nums,
            Subtotal::StdDev => &self.std_dev,
            Subtotal::StdDevP => &self.std_dev_p,
            Subtotal::Var => &self.var,
            Subtotal::VarP => &self.var_p,
        }
    }
}

fn fill(template: &str, name: &str) -> String {
    template.replacen("{}", name, 1)
}

/// Lays out pivot table `table` of sheet `sheet` from its source data, and
/// returns the cells the report now covers.
///
/// The old report's cells are cleared and the new one written from the same
/// top-left corner. The definition is brought up to date the way Excel's
/// Refresh does it: `location`, `first_data_row`, `first_data_col`, a caption
/// for each value field without one, the values field placed or dropped, each
/// field's axis; and its cache takes the source's headings as field names and
/// the items of the fields on an axis as shared items.
///
/// The fields of the report that sit on an axis, and the source columns the
/// report reads: everything the definition names, except the fields with no
/// column of their own - a calculated field computes one, a level of a
/// grouping derives one from its parent's values. A formula's inputs are
/// read even when no axis asks for them, and so are the values a grouping's
/// levels derive from.
fn columns_to_read(
    def: &PivotTable,
    cache_fields: &[CacheField],
    count: usize,
) -> Result<(Vec<usize>, Vec<usize>)> {
    let on_axis: Vec<usize> = def
        .row_fields
        .iter()
        .chain(&def.column_fields)
        .chain(&def.page_fields)
        .filter_map(|&x| usize::try_from(x).ok())
        .collect();
    let mut read: Vec<usize> = on_axis
        .iter()
        .copied()
        .chain(
            def.data_fields
                .iter()
                .filter_map(|d| usize::try_from(d.field).ok()),
        )
        .filter(|&f| !cache_fields.get(f).is_some_and(is_computed))
        .collect();
    // A formula's inputs are read even when no axis asks for them.
    for field in cache_fields {
        let Some(text) = field.formula.as_deref() else {
            continue;
        };
        let terms = Terms::parse(text).ok_or_else(|| bad(formula_error(field)))?;
        for name in terms.names() {
            if let Some(at) = column_of(name, cache_fields)
                && at < count
                && !read.contains(&at)
            {
                read.push(at);
            }
        }
    }
    // The values a grouping's levels are derived from.
    for grouping in cache_fields.iter().filter_map(|f| f.grouping.as_ref()) {
        if let Some(parent) = grouping
            .parent
            .and_then(|p| usize::try_from(p).ok())
            .filter(|&p| p < count && !read.contains(&p))
        {
            read.push(parent);
        }
    }
    Ok((on_axis, read))
}

/// The source is the cache's sheet and range (the report's own sheet when the
/// cache names none), or its name: a table or a defined name. Its first row
/// names the fields.
///
/// ```
/// use excelerate::edit::{PivotCaptions, refresh_pivot};
/// use excelerate::model::pivot::{CacheField, CacheSource, DataField, PivotCache, PivotTable};
/// use excelerate::model::{Spreadsheet, Worksheet};
/// use excelerate::{CellRef, Range};
///
/// let mut book = Spreadsheet::empty();
/// let mut sheet = Worksheet::new("Data")?;
/// for (row, (region, sales)) in [("Region", 0.0), ("North", 10.0), ("South", 5.0), ("North", 1.0)]
///     .into_iter()
///     .enumerate()
/// {
///     let row = u32::try_from(row).unwrap() + 1;
///     sheet.set_at(row, 1, region)?;
///     if row > 1 {
///         sheet.set_at(row, 2, sales)?;
///     } else {
///         sheet.set_at(row, 2, "Sales")?;
///     }
/// }
/// sheet.pivot_tables.push(PivotTable {
///     name: "Pivot".into(),
///     cache_id: 1,
///     location: Some(Range::parse("D1")?),
///     row_fields: vec![0],
///     data_fields: vec![DataField { field: 1, ..DataField::default() }],
///     row_grand_totals: true,
///     column_grand_totals: true,
///     ..PivotTable::default()
/// });
/// book.add_sheet(sheet)?;
/// book.pivot_caches.push(PivotCache {
///     id: 1,
///     source: CacheSource { range: Some(Range::parse("A1:B4")?), ..CacheSource::default() },
///     ..PivotCache::default()
/// });
///
/// let area = refresh_pivot(&mut book, 0, 0, &PivotCaptions::default())?;
/// assert_eq!(area.to_string(), "D1:E4");
/// let sheet = book.sheet(0).unwrap();
/// let text = |a: &str| sheet.get(CellRef::parse(a).unwrap()).map(|c| c.value.clone());
/// assert_eq!(text("E1"), Some("Sum of Sales".into()));
/// assert_eq!(text("D2"), Some("North".into()));
/// assert_eq!(text("E2"), Some(11.0.into()));
/// assert_eq!(text("D4"), Some("Grand Total".into()));
/// # Ok::<(), excelerate::Error>(())
/// ```
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] for a sheet the book does not have;
/// [`Error::InvalidRange`] for a report that does not exist, has no location
/// or no cache, a source that cannot be found or has an empty heading, a
/// field index past the cache's fields, a formula that does not compute, a
/// grouping this crate does not lay out, or a report that would leave the
/// sheet or overlap its source. Nothing is changed then.
pub fn refresh_pivot(
    book: &mut Spreadsheet,
    sheet: usize,
    table: usize,
    captions: &PivotCaptions,
) -> Result<Range> {
    let epoch = book.epoch;
    let ws = book
        .sheet(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    let mut def = ws
        .pivot_tables
        .get(table)
        .ok_or_else(|| bad(format!("sheet {} has no pivot table {table}", ws.title())))?
        .clone();
    let old = def
        .location
        .ok_or_else(|| bad(format!("pivot table {:?} has no location", def.name)))?;
    let cache_at = book
        .pivot_caches
        .iter()
        .position(|c| c.id == def.cache_id)
        .ok_or_else(|| {
            bad(format!(
                "pivot table {:?} reads cache {}, which the workbook does not have",
                def.name, def.cache_id
            ))
        })?;
    let (source_sheet, source) = source_of(book, cache_at, sheet)?;
    let src = book
        .sheet(source_sheet)
        .ok_or(Error::SheetIndexOutOfRange(source_sheet))?;
    let names = headings(src, source)?;
    let count = names.len();
    let cache_fields = book.pivot_caches[cache_at].fields.clone();
    let width = count.max(cache_fields.len());

    check_fields(&def, count, cache_fields.len())?;
    check_grouping(&cache_fields, count)?;
    normalise(&mut def, &names, &cache_fields, captions);
    let (on_axis, read) = columns_to_read(&def, &cache_fields, count)?;
    let mut data = records(src, source, width, &read)?;
    let height = data.iter().map(Vec::len).max().unwrap_or(0);
    calculated_columns(&mut data, &cache_fields, height);

    // The axes as the report lays them out: a grouped field contributes one
    // level per part, senior first, or one span level; the columns they
    // need grow onto `data`.
    let plans = Plans::build(&def, &cache_fields, &names, captions, epoch, &mut data);

    // The page filters, and the cache's items for every field on an axis.
    let mut shared: Vec<Vec<String>> = vec![Vec::new(); count];
    for &f in &on_axis {
        if f < count {
            shared[f] = items_of(&data[f]);
        }
    }
    let kept = page_filter(&mut def, &plans, &data, height);

    let layout = Layout::new(&def, &plans, &data, &kept);
    let rows = layout.header_rows + layout.rows.len();
    let cols = layout.label_cols + layout.cols.len();
    let start = old.start;
    let end = offset(start, rows - 1, cols - 1)
        .ok_or_else(|| bad(format!("pivot table {:?} would leave the sheet", def.name)))?;
    let area = Range::new(start, end);
    if source_sheet == sheet && area.intersects(&source) {
        return Err(bad(format!(
            "pivot table {:?} would overlap its source {source}",
            def.name
        )));
    }
    let cells = layout.cells(&def, &data, captions, &plans);

    def.location = Some(area);
    def.first_data_row = u32::try_from(layout.header_rows).unwrap_or(u32::MAX);
    def.first_data_col = u32::try_from(layout.label_cols).unwrap_or(u32::MAX);
    let cache = &mut book.pivot_caches[cache_at];
    let old_fields = std::mem::take(&mut cache.fields);
    let mut fields: Vec<CacheField> = names
        .into_iter()
        .zip(shared)
        .enumerate()
        .map(|(i, (name, shared_items))| CacheField {
            name,
            number_format: old_fields.get(i).and_then(|f| f.number_format),
            shared_items,
            formula: old_fields.get(i).and_then(|f| f.formula.clone()),
            grouping: old_fields.get(i).and_then(|f| f.grouping.clone()),
        })
        .collect();
    // The fields past the source's columns - calculated fields, and the
    // level fields a foreign file adds per level of a grouping - keep their
    // places, and so do the indexes that point at them.
    fields.extend(old_fields.into_iter().skip(count));
    cache.fields = fields;

    let ws = book
        .sheet_mut(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    for row in old.start.row.index()..=old.end.row.index() {
        for col in old.start.col.index()..=old.end.col.index() {
            if let (Some(row), Some(col)) = (Row::new(row), Col::new(col)) {
                ws.remove(CellRef::new(col, row));
            }
        }
    }
    for (row, col, value) in cells {
        if let Some(at) = offset(start, row, col) {
            ws.set(at, value);
        }
    }
    ws.pivot_tables[table] = def;
    Ok(area)
}

/// One level a field on an axis contributes: the column its labels come
/// from and the header over them.
struct Slot {
    /// The place of the column in the data.
    column: usize,
    /// The header the labels sit under.
    header: String,
}

/// The axes as the report lays them out. A field with a grouping
/// contributes one entry per level - date parts senior first, or one span
/// level - and the entries carry the columns the levels were derived into.
struct Plans {
    rows: Vec<Slot>,
    /// `-2` where the values field sits, a level's column elsewhere.
    row_list: Vec<i32>,
    /// Whether each entry gets a subtotal when a deeper level follows.
    row_subtotals: Vec<bool>,
    cols: Vec<Slot>,
    col_list: Vec<i32>,
    col_subtotals: Vec<bool>,
    /// Each page field's own level column, in the order the definition
    /// lists the fields.
    pages: Vec<usize>,
}

impl Plans {
    fn build(
        def: &PivotTable,
        cache_fields: &[CacheField],
        names: &[String],
        captions: &PivotCaptions,
        epoch: Epoch,
        data: &mut Vec<Vec<Item>>,
    ) -> Self {
        let (rows, row_list, row_subtotals) = expand_axis(
            &def.row_fields,
            def,
            cache_fields,
            names,
            captions,
            epoch,
            data,
        );
        let (cols, col_list, col_subtotals) = expand_axis(
            &def.column_fields,
            def,
            cache_fields,
            names,
            captions,
            epoch,
            data,
        );
        let (page_slots, _, _) = expand_axis(
            &def.page_fields,
            def,
            cache_fields,
            names,
            captions,
            epoch,
            data,
        );
        let pages = page_slots.iter().map(|slot| slot.column).collect();
        Self {
            rows,
            row_list,
            row_subtotals,
            cols,
            col_list,
            col_subtotals,
            pages,
        }
    }
}

/// Expands one axis's fields into their entries: a field with a grouping
/// becomes one entry per level, any other field stands for its own column,
/// and the values field's `-2` rides along as it is. The derived columns
/// the levels need grow onto `data`.
fn expand_axis(
    list: &[i32],
    def: &PivotTable,
    cache_fields: &[CacheField],
    names: &[String],
    captions: &PivotCaptions,
    epoch: Epoch,
    data: &mut Vec<Vec<Item>>,
) -> (Vec<Slot>, Vec<i32>, Vec<bool>) {
    let mut slots = Vec::new();
    let mut entries = Vec::new();
    let mut subtotals = Vec::new();
    for &x in list {
        let Ok(f) = usize::try_from(x) else {
            // The values field: its header is the word for it, and the
            // axis's entries know `-2` on their own.
            slots.push(Slot {
                column: usize::MAX,
                header: captions.values.clone(),
            });
            entries.push(x);
            subtotals.push(false);
            continue;
        };
        let subtotal = def
            .fields
            .get(f)
            .is_some_and(|field| field.default_subtotal);
        let own_header = || {
            names
                .get(f)
                .cloned()
                .unwrap_or_else(|| captions.values.clone())
        };
        let Some(grouping) = cache_fields
            .get(f)
            .and_then(|field| field.grouping.as_ref())
        else {
            slots.push(Slot {
                column: f,
                header: own_header(),
            });
            entries.push(x);
            subtotals.push(subtotal);
            continue;
        };
        // The values the levels are derived from: another field's, when the
        // field is a level of a foreign file's grouping, else its own.
        let source = grouping
            .parent
            .and_then(|p| usize::try_from(p).ok())
            .unwrap_or(f);
        let values = data.get(source).cloned().unwrap_or_default();
        // The parts of a grouping made in this crate carry every level, and
        // their headers are the words for the parts; a field of a foreign
        // file carries one level, under the field's own name, as Excel
        // spells it.
        let multi = grouping.parent.is_none() && grouping.parts.len() > 1;
        if let Some(range) = &grouping.range {
            let column = data.len();
            data.push(span_column(&values, range, &captions.other));
            slots.push(Slot {
                column,
                header: own_header(),
            });
            entries.push(level_index(column));
            subtotals.push(subtotal);
        }
        let mut parts = grouping.parts.clone();
        parts.sort();
        for part in parts {
            let column = data.len();
            data.push(part_column(&values, part, epoch, captions));
            let header = if multi {
                captions.part_name(part).clone()
            } else {
                own_header()
            };
            slots.push(Slot { column, header });
            entries.push(level_index(column));
            subtotals.push(subtotal);
        }
    }
    (slots, entries, subtotals)
}

/// The place of a derived column on an axis. The columns a report derives
/// are counted in tens, far below what `i32` holds.
fn level_index(column: usize) -> i32 {
    i32::try_from(column).unwrap_or(i32::MAX)
}

/// The records the page filters let through. A filter whose item the source
/// no longer has shows everything again, as in Excel. A grouped field's
/// filter matches its senior-most level.
fn page_filter(
    def: &mut PivotTable,
    plans: &Plans,
    data: &[Vec<Item>],
    height: usize,
) -> Vec<usize> {
    let mut kept: Vec<usize> = (0..height).collect();
    for (i, &p) in def.page_fields.iter().enumerate() {
        let Ok(p) = usize::try_from(p) else {
            continue;
        };
        let Some(&column) = plans.pages.get(i) else {
            continue;
        };
        let field = &mut def.fields[p];
        if let Some(wanted) = &field.page_item {
            let shown = |r: usize| {
                data.get(column)
                    .and_then(|c| c.get(r))
                    .map_or(String::new(), |i| i.text().to_lowercase())
            };
            let wanted = wanted.to_lowercase();
            if (0..height).any(|r| shown(r) == wanted) {
                kept.retain(|&r| shown(r) == wanted);
            } else {
                field.page_item = None;
            }
        }
    }
    kept
}

/// A field's distinct items in order, as text, blanks left out.
fn items_of(column: &[Item]) -> Vec<String> {
    let mut items: Vec<&Item> = column.iter().filter(|i| !i.is_blank()).collect();
    items.sort_by(|a, b| a.cmp(b));
    items.dedup_by(|a, b| a.cmp(b) == Ordering::Equal);
    items.iter().map(|i| i.text()).collect()
}

fn bad(message: String) -> Error {
    Error::InvalidRange(message)
}

fn offset(start: CellRef, rows: usize, cols: usize) -> Option<CellRef> {
    let row = start.row.index().checked_add(u32::try_from(rows).ok()?)?;
    let col = start.col.index().checked_add(u32::try_from(cols).ok()?)?;
    Some(CellRef::new(Col::new(col)?, Row::new(row)?))
}

/// Every field the definition names is a column of the source, or a field
/// of the cache that has no column of its own: a calculated field, or a
/// level of another field's grouping.
fn check_fields(def: &PivotTable, count: usize, cache_len: usize) -> Result<()> {
    let listed = def
        .row_fields
        .iter()
        .chain(&def.column_fields)
        .chain(&def.page_fields)
        .copied()
        .filter(|&x| x != VALUES);
    let data_listed = def.data_fields.iter().map(|d| i32::try_from(d.field));
    let in_source = |x: i32| usize::try_from(x).is_ok_and(|x| x < count);
    let in_cache = |x: i32| usize::try_from(x).is_ok_and(|x| x < cache_len);
    for x in listed.map(Ok).chain(data_listed) {
        if !x.is_ok_and(|x| in_source(x) || in_cache(x)) {
            return Err(bad(format!(
                "pivot table {:?} uses field {}, and the source has {count} columns",
                def.name,
                x.map_or_else(|_| "past i32".to_owned(), |x| x.to_string())
            )));
        }
    }
    Ok(())
}

/// A grouping a refresh can lay out: a calculated field cannot be grouped,
/// a level field's parent must be a plain column of the source, a span
/// grouping needs a positive interval, and a grouping needs something to
/// group over.
fn check_grouping(fields: &[CacheField], count: usize) -> Result<()> {
    for (i, field) in fields.iter().enumerate() {
        if field.formula.is_some() && field.grouping.is_some() {
            return Err(bad(format!(
                "cache field {:?} has both a formula and a grouping",
                field.name
            )));
        }
        let Some(grouping) = &field.grouping else {
            continue;
        };
        if let Some(range) = &grouping.range {
            let spans = range.interval.is_finite()
                && range.interval > 0.0
                && range.start.is_finite()
                && range.end.is_finite();
            if !spans {
                return Err(bad(format!(
                    "cache field {:?} groups numbers over a range that is not a range",
                    field.name
                )));
            }
            if !grouping.parts.is_empty() {
                return Err(bad(format!(
                    "cache field {:?} groups numbers and dates at once",
                    field.name
                )));
            }
        }
        if grouping.parts.is_empty() && grouping.range.is_none() {
            return Err(bad(format!("cache field {:?} groups nothing", field.name)));
        }
        match grouping.parent {
            Some(parent) => {
                // The parent must be a column of the source with values of
                // its own: not a calculated field, and not a level field of
                // yet another grouping.
                let plain = usize::try_from(parent).is_ok_and(|p| {
                    p < count
                        && fields.get(p).is_some_and(|f| {
                            f.formula.is_none()
                                && f.grouping.as_ref().and_then(|g| g.parent).is_none()
                        })
                });
                if !plain {
                    return Err(bad(format!(
                        "cache field {:?} groups field {parent}, which is not a plain column of the source",
                        field.name
                    )));
                }
            }
            // A grouping without a parent derives its levels from the
            // field's own values, which only a column of the source has.
            None if i >= count => {
                return Err(bad(format!(
                    "cache field {:?} has no column in the source to group",
                    field.name
                )));
            }
            None => {}
        }
    }
    Ok(())
}

/// Whether the field has no column of its own: its values are computed from
/// the other fields of the row, or derived from another field's values.
fn is_computed(field: &CacheField) -> bool {
    field.formula.is_some() || field.grouping.as_ref().is_some_and(|g| g.parent.is_some())
}

/// The column of the field a formula names, without regard to case.
fn column_of(name: &str, fields: &[CacheField]) -> Option<usize> {
    let name = name.to_lowercase();
    fields.iter().position(|f| f.name.to_lowercase() == name)
}

fn formula_error(field: &CacheField) -> String {
    format!(
        "cache field {:?} has a formula that does not compute: {:?}",
        field.name,
        field.formula.as_deref().unwrap_or_default()
    )
}

/// The columns of the calculated fields: one per field with a formula,
/// computed row by row from the other fields' values of that row. A formula
/// that names another calculated field waits for that one's column; a
/// formula that cannot compute is an error, which
/// [`refresh_pivot`](crate::edit::refresh_pivot) refuses before anything is
/// laid out.
fn calculated_columns(data: &mut [Vec<Item>], fields: &[CacheField], height: usize) {
    let formulas: Vec<(usize, Terms)> = fields
        .iter()
        .enumerate()
        .filter_map(|(i, field)| {
            field
                .formula
                .as_deref()
                .and_then(Terms::parse)
                .and_then(|t| t.bind(&|name| column_of(name, fields)).map(|t| (i, t)))
        })
        .collect();
    // A column is known when it was read or its field's formula is done;
    // a formula over nothing but known columns can compute.
    let mut done = vec![false; fields.len()];
    for _ in 0..=formulas.len() {
        let mut progressed = false;
        for (i, terms) in &formulas {
            if done[*i] {
                continue;
            }
            let ready = terms.named().iter().all(|&j| {
                data.get(j).is_some_and(|c| !c.is_empty()) || done.get(j).copied().unwrap_or(false)
            });
            if !ready {
                continue;
            }
            let mut column = Vec::with_capacity(height);
            for row in 0..height {
                let value = |at: usize| {
                    data.get(at)
                        .and_then(|c| c.get(row))
                        .and_then(|item| match item {
                            Item::Number(n) => Some(*n),
                            _ => None,
                        })
                };
                column.push(match terms.eval(&value) {
                    Some(n) => Item::Number(n),
                    None => Item::Blank,
                });
            }
            if let Some(slot) = data.get_mut(*i) {
                *slot = column;
            }
            done[*i] = true;
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
}

/// The sheet and range a cache reads.
fn source_of(book: &Spreadsheet, cache: usize, own: usize) -> Result<(usize, Range)> {
    let source = &book.pivot_caches[cache].source;
    if let Some(range) = source.range {
        let sheet = match &source.sheet {
            Some(name) => book
                .sheet_index_by_name(name)
                .ok_or_else(|| bad(format!("pivot source sheet {name:?} does not exist")))?,
            None => own,
        };
        return Ok((sheet, range));
    }
    let Some(name) = source.name.as_deref() else {
        return Err(bad("pivot cache names no source".into()));
    };
    // Excel writes a defined name with its `=` sometimes.
    let name = name.trim_start_matches('=');
    for (index, ws) in book.sheets().iter().enumerate() {
        if let Some(t) = ws.tables.iter().find(|t| {
            t.name.eq_ignore_ascii_case(name) || t.display_name.eq_ignore_ascii_case(name)
        }) {
            let totals = t.totals_row_count.unwrap_or(0);
            let end = t.range.end.row.index().saturating_sub(totals);
            let end = Row::new(end.max(t.range.start.row.index())).unwrap_or(t.range.end.row);
            return Ok((
                index,
                Range::new(t.range.start, CellRef::new(t.range.end.col, end)),
            ));
        }
    }
    let defined = book
        .defined_names
        .iter()
        .filter(|d| d.name.eq_ignore_ascii_case(name))
        .min_by_key(|d| d.sheet.is_some_and(|s| s != own))
        .ok_or_else(|| bad(format!("pivot source {name:?} is not a table or a name")))?;
    let formula = defined.formula.trim_start_matches('=');
    let (sheet, range) = match formula.rsplit_once('!') {
        Some((sheet, range)) => {
            let sheet = sheet
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .map_or_else(|| sheet.to_owned(), |s| s.replace("''", "'"));
            let index = book
                .sheet_index_by_name(&sheet)
                .ok_or_else(|| bad(format!("pivot source sheet {sheet:?} does not exist")))?;
            (index, range)
        }
        None => (defined.sheet.unwrap_or(own), formula),
    };
    Ok((sheet, Range::parse(range)?))
}

/// The field names: the source's first row. Excel refuses an empty one and
/// tells two alike apart with a number, "Sales2".
fn headings(sheet: &Worksheet, source: Range) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for col in source.start.col.index()..=source.end.col.index() {
        let Some(col) = Col::new(col) else { break };
        let at = CellRef::new(col, source.start.row);
        let name = sheet
            .get(at)
            .map(|c| Item::of(&c.value))
            .filter(|i| !i.is_blank())
            .map(|i| i.text())
            .ok_or_else(|| bad(format!("pivot source heading {at} is empty")))?;
        let mut unique = name.clone();
        let mut n = 2;
        while names.iter().any(|x| x.eq_ignore_ascii_case(&unique)) {
            unique = format!("{name}{n}");
            n += 1;
        }
        names.push(unique);
    }
    Ok(names)
}

/// The source's records, a column per field; only the `used` fields are
/// read, the rest left empty. Rows past the sheet's last cell are not read.
fn records(
    sheet: &Worksheet,
    source: Range,
    count: usize,
    used: &[usize],
) -> Result<Vec<Vec<Item>>> {
    let first = source.start.row.index() + 1;
    let last = sheet
        .dimension()
        .map_or(0, |d| d.end.row.index())
        .min(source.end.row.index());
    let height = usize::try_from(last.saturating_add(1).saturating_sub(first)).unwrap_or(0);
    let mut fields: Vec<usize> = used.to_vec();
    fields.sort_unstable();
    fields.dedup();
    if height.saturating_mul(fields.len()) > MAX_SOURCE_CELLS {
        return Err(bad(format!(
            "pivot source {source} is past {MAX_SOURCE_CELLS} cells"
        )));
    }
    let mut out = vec![Vec::new(); count];
    for f in fields {
        let col = u32::try_from(f)
            .ok()
            .and_then(|f| source.start.col.index().checked_add(f))
            .and_then(Col::new);
        let Some(col) = col else { continue };
        out[f] = (first..=last)
            .filter_map(Row::new)
            .map(|row| {
                sheet
                    .get(CellRef::new(col, row))
                    .map_or(Item::Blank, |c| Item::of(&c.value))
            })
            .collect();
    }
    Ok(out)
}

/// Brings the definition in line with its source, as Excel does on refresh.
fn normalise(
    def: &mut PivotTable,
    names: &[String],
    cache_fields: &[CacheField],
    captions: &PivotCaptions,
) {
    let count = names.len();
    def.fields.resize(
        count.max(cache_fields.len()),
        PivotField {
            default_subtotal: true,
            ..PivotField::default()
        },
    );
    // Two value fields or more need the values field somewhere; fewer have
    // no use for it.
    let placed = def.row_fields.contains(&VALUES) || def.column_fields.contains(&VALUES);
    if def.data_fields.len() >= 2 {
        if !placed {
            def.column_fields.push(VALUES);
        }
    } else {
        def.row_fields.retain(|&x| x != VALUES);
        def.column_fields.retain(|&x| x != VALUES);
    }
    for (i, field) in def.fields.iter_mut().enumerate() {
        let index = i32::try_from(i).unwrap_or(i32::MAX);
        field.axis = if def.row_fields.contains(&index) {
            PivotAxis::Row
        } else if def.column_fields.contains(&index) {
            PivotAxis::Column
        } else if def.page_fields.contains(&index) {
            PivotAxis::Page
        } else {
            PivotAxis::Unused
        };
        field.data_field = def
            .data_fields
            .iter()
            .any(|d| usize::try_from(d.field).is_ok_and(|f| f == i));
    }
    for i in 0..def.data_fields.len() {
        if def.data_fields[i].name.is_some() {
            continue;
        }
        let data = &def.data_fields[i];
        // A calculated field sits past the source's columns: its caption
        // names the cache field, not a heading.
        let source = usize::try_from(data.field)
            .ok()
            .and_then(|f| names.get(f))
            .or_else(|| {
                cache_fields
                    .get(usize::try_from(data.field).unwrap_or(usize::MAX))
                    .map(|f| &f.name)
            })
            .map_or("", String::as_str);
        let name = fill(captions.function(data.subtotal), source);
        let mut unique = name.clone();
        let mut n = 2;
        while def
            .data_fields
            .iter()
            .any(|d| d.name.as_deref() == Some(unique.as_str()))
        {
            unique = format!("{name}{n}");
            n += 1;
        }
        def.data_fields[i].name = Some(unique);
    }
}

/// One source value, as an item of a field or a value to summarise.
#[derive(Debug, Clone)]
enum Item {
    Number(f64),
    Text(String),
    /// A grouping level's label, with the place it takes among the column's
    /// labels: calendar order for the parts of a date, span order for a
    /// number grouping, which is why the labels do not sort as text.
    Level {
        label: String,
        order: u32,
    },
    Bool(bool),
    Error(CellError),
    Blank,
}

impl Item {
    fn of(value: &CellValue) -> Self {
        match value {
            CellValue::Number(n) => Self::Number(*n),
            CellValue::Text(t) => Self::Text(t.to_string()),
            CellValue::RichText(_) => Self::Text(value.plain_text().unwrap_or_default()),
            CellValue::Bool(b) => Self::Bool(*b),
            CellValue::Error(e) => Self::Error(*e),
            CellValue::Formula {
                cached: Some(cached),
                ..
            } => Self::of(cached),
            CellValue::Empty | CellValue::Formula { .. } => Self::Blank,
        }
    }

    const fn is_blank(&self) -> bool {
        matches!(self, Self::Blank)
    }

    const fn rank(&self) -> u8 {
        match self {
            Self::Number(_) => 0,
            Self::Text(_) | Self::Level { .. } => 1,
            Self::Bool(_) => 2,
            Self::Error(_) => 3,
            Self::Blank => 4,
        }
    }

    /// Excel's pivot order, which is also its notion of the same item.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (
                Self::Level {
                    order: a,
                    label: la,
                },
                Self::Level {
                    order: b,
                    label: lb,
                },
            ) => a.cmp(b).then_with(|| la.cmp(lb)),
            (Self::Text(a), Self::Text(b)) => {
                if a.is_ascii() && b.is_ascii() {
                    a.bytes()
                        .map(|c| c.to_ascii_uppercase())
                        .cmp(b.bytes().map(|c| c.to_ascii_uppercase()))
                } else {
                    a.chars()
                        .flat_map(char::to_uppercase)
                        .cmp(b.chars().flat_map(char::to_uppercase))
                }
            }
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Error(a), Self::Error(b)) => a.as_str().cmp(b.as_str()),
            _ => self.rank().cmp(&other.rank()),
        }
    }

    /// The item as text: a cache's shared item, a page filter's choice, the
    /// name in a subtotal's label.
    fn text(&self) -> String {
        match self {
            Self::Number(n) => crate::style::format::format(
                crate::style::format::Value::Number(*n),
                crate::style::format::GENERAL,
                crate::shared::date::Epoch::Windows1900,
            ),
            Self::Text(t) => t.clone(),
            Self::Level { label, .. } => label.clone(),
            Self::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
            Self::Error(e) => e.as_str().to_owned(),
            Self::Blank => String::new(),
        }
    }

    /// The item as a label cell shows it.
    fn cell(&self, captions: &PivotCaptions) -> CellValue {
        match self {
            Self::Number(n) => CellValue::Number(*n),
            Self::Text(t) => CellValue::text(t.as_str()),
            Self::Level { label, .. } => CellValue::text(label.as_str()),
            Self::Bool(b) => CellValue::Bool(*b),
            Self::Error(e) => CellValue::Error(*e),
            Self::Blank => CellValue::text(captions.blank.as_str()),
        }
    }

    /// The label "North Total" takes the item from.
    fn name(&self, captions: &PivotCaptions) -> String {
        if self.is_blank() {
            captions.blank.clone()
        } else {
            self.text()
        }
    }
}

/// A number the way Excel's General format shows it, which is how a span
/// grouping's labels spell their bounds.
fn general_number(n: f64) -> String {
    crate::style::format::format(
        crate::style::format::Value::Number(n),
        crate::style::format::GENERAL,
        Epoch::Windows1900,
    )
}

/// One calculated field's formula, as a tree of arithmetic: numbers, field
/// names, `+`, `-`, `*`, `/`, parentheses and a unary minus, which is what
/// the file's `formula` attribute allows. Functions Excel has are not part
/// of it; a `Name(` that looks like one does not parse.
#[derive(Debug, Clone)]
enum Terms {
    Number(f64),
    /// A field, named as the cache names it, apostrophes around a name with
    /// a space or a quote of its own.
    Name(String),
    /// A `Name` that found its column, as [`Terms::bind`] leaves it.
    Field(usize),
    Sum(Box<Self>, Box<Self>),
    Difference(Box<Self>, Box<Self>),
    Product(Box<Self>, Box<Self>),
    Quotient(Box<Self>, Box<Self>),
    Negated(Box<Self>),
}

impl Terms {
    /// Parses the text of a formula, without the leading `=`. Everything
    /// after the last term must be spaces.
    fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let mut at = 0;
        let terms = Self::sum(bytes, &mut at)?;
        skip_spaces(bytes, &mut at);
        (at == bytes.len()).then_some(terms)
    }

    fn sum(bytes: &[u8], at: &mut usize) -> Option<Self> {
        let mut left = Self::product(bytes, at)?;
        loop {
            let before = *at;
            skip_spaces(bytes, at);
            let plus = matches!(bytes.get(*at), Some(b'+'));
            let minus = matches!(bytes.get(*at), Some(b'-'));
            if !plus && !minus {
                *at = before;
                return Some(left);
            }
            *at += 1;
            skip_spaces(bytes, at);
            let right = Self::product(bytes, at)?;
            left = if plus {
                Self::Sum(Box::new(left), Box::new(right))
            } else {
                Self::Difference(Box::new(left), Box::new(right))
            };
        }
    }

    fn product(bytes: &[u8], at: &mut usize) -> Option<Self> {
        let mut left = Self::unary(bytes, at)?;
        loop {
            let before = *at;
            skip_spaces(bytes, at);
            let times = matches!(bytes.get(*at), Some(b'*'));
            let by = matches!(bytes.get(*at), Some(b'/'));
            if !times && !by {
                *at = before;
                return Some(left);
            }
            *at += 1;
            skip_spaces(bytes, at);
            let right = Self::unary(bytes, at)?;
            left = if times {
                Self::Product(Box::new(left), Box::new(right))
            } else {
                Self::Quotient(Box::new(left), Box::new(right))
            };
        }
    }

    fn unary(bytes: &[u8], at: &mut usize) -> Option<Self> {
        skip_spaces(bytes, at);
        if bytes.get(*at) == Some(&b'-') {
            *at += 1;
            return Some(Self::Negated(Box::new(Self::unary(bytes, at)?)));
        }
        Self::primary(bytes, at)
    }

    fn primary(bytes: &[u8], at: &mut usize) -> Option<Self> {
        skip_spaces(bytes, at);
        match bytes.get(*at)? {
            b'(' => {
                *at += 1;
                let inner = Self::sum(bytes, at)?;
                skip_spaces(bytes, at);
                if bytes.get(*at) == Some(&b')') {
                    *at += 1;
                } else {
                    return None;
                }
                Some(inner)
            }
            b'\'' => quoted_name(bytes, at).map(Self::Name),
            b'+' => {
                *at += 1;
                Self::unary(bytes, at)
            }
            byte if byte.is_ascii_digit() || *byte == b'.' => Self::number(bytes, at),
            _ => Some(Self::Name(bare_name(bytes, at)?)),
        }
    }

    fn number(bytes: &[u8], at: &mut usize) -> Option<Self> {
        let start = *at;
        while bytes
            .get(*at)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
        {
            *at += 1;
        }
        // The scan stopped on ASCII digits and dots, so the slice is a
        // string.
        let text = std::str::from_utf8(bytes.get(start..*at)?).ok()?;
        text.parse().ok().map(Self::Number)
    }

    /// Every field name the terms mention, in no particular order.
    fn names(&self) -> Vec<&str> {
        match self {
            Self::Name(name) => vec![name.as_str()],
            Self::Number(_) | Self::Field(_) => Vec::new(),
            Self::Sum(a, b)
            | Self::Difference(a, b)
            | Self::Product(a, b)
            | Self::Quotient(a, b) => {
                let mut out = a.names();
                out.extend(b.names());
                out
            }
            Self::Negated(a) => a.names(),
        }
    }

    /// Turns every `Name` into the column of the cache field it names;
    /// `None` when a name names nothing.
    fn bind(&self, columns: &dyn Fn(&str) -> Option<usize>) -> Option<Self> {
        match self {
            Self::Name(name) => Some(Self::Field(columns(name)?)),
            Self::Number(_) | Self::Field(_) => Some(self.clone()),
            Self::Sum(a, b) => Some(Self::Sum(
                Box::new(a.bind(columns)?),
                Box::new(b.bind(columns)?),
            )),
            Self::Difference(a, b) => Some(Self::Difference(
                Box::new(a.bind(columns)?),
                Box::new(b.bind(columns)?),
            )),
            Self::Product(a, b) => Some(Self::Product(
                Box::new(a.bind(columns)?),
                Box::new(b.bind(columns)?),
            )),
            Self::Quotient(a, b) => Some(Self::Quotient(
                Box::new(a.bind(columns)?),
                Box::new(b.bind(columns)?),
            )),
            Self::Negated(a) => Some(Self::Negated(Box::new(a.bind(columns)?))),
        }
    }

    /// The columns the terms read.
    fn named(&self) -> Vec<usize> {
        match self {
            Self::Field(f) => vec![*f],
            Self::Number(_) | Self::Name(_) => Vec::new(),
            Self::Sum(a, b)
            | Self::Difference(a, b)
            | Self::Product(a, b)
            | Self::Quotient(a, b) => {
                let mut out = a.named();
                out.extend(b.named());
                out
            }
            Self::Negated(a) => a.named(),
        }
    }

    /// The value of the terms over a row's fields. A field without a
    /// number, a division by zero and a result that is not a finite number
    /// all give `None`, which is the empty value a calculated field shows.
    fn eval(&self, values: &dyn Fn(usize) -> Option<f64>) -> Option<f64> {
        let value = match self {
            Self::Number(n) => Some(*n),
            Self::Field(f) => values(*f),
            Self::Name(_) => None,
            Self::Negated(a) => Some(-a.eval(values)?),
            Self::Sum(a, b) => Some(a.eval(values)? + b.eval(values)?),
            Self::Difference(a, b) => Some(a.eval(values)? - b.eval(values)?),
            Self::Product(a, b) => Some(a.eval(values)? * b.eval(values)?),
            Self::Quotient(a, b) => {
                let by = b.eval(values)?;
                if by == 0.0 {
                    None
                } else {
                    Some(a.eval(values)? / by)
                }
            }
        };
        value.filter(|v| v.is_finite())
    }
}

fn skip_spaces(bytes: &[u8], at: &mut usize) {
    while bytes.get(*at).is_some_and(u8::is_ascii_whitespace) {
        *at += 1;
    }
}

/// A name in apostrophes, `''` inside standing for the apostrophe itself.
fn quoted_name(bytes: &[u8], at: &mut usize) -> Option<String> {
    *at += 1;
    let start = *at;
    let mut cursor = *at;
    loop {
        match bytes.get(cursor)? {
            b'\'' if bytes.get(cursor + 1) == Some(&b'\'') => cursor += 2,
            b'\'' => break,
            _ => cursor += 1,
        }
    }
    // The scan stopped on ASCII apostrophes, so the slice is a string.
    let raw = std::str::from_utf8(bytes.get(start..cursor)?).ok()?;
    *at = cursor + 1;
    Some(raw.replace("''", "'"))
}

/// A name that stands bare: everything until a space or one of the signs
/// the arithmetic is written with.
fn bare_name(bytes: &[u8], at: &mut usize) -> Option<String> {
    let start = *at;
    while bytes
        .get(*at)
        .is_some_and(|b| !b.is_ascii_whitespace() && !b"+-*/()'".contains(b))
    {
        *at += 1;
    }
    if *at == start {
        return None;
    }
    // The scan stopped on ASCII signs and spaces, so the slice is a string.
    std::str::from_utf8(bytes.get(start..*at)?)
        .ok()
        .map(str::to_owned)
}

/// Evaluates one calculated field's formula - the text without the leading
/// `=` - over a row's values, as `("Sales", 100.0)` pairs named the way the
/// cache names its fields. Names match without regard to case; a missing
/// name, a division by zero and a result that is not a finite number give
/// `None`, which is the empty value [`refresh_pivot`] writes for them.
///
/// ```
/// use excelerate::edit::eval_pivot_formula;
///
/// let row = [("Sales".to_owned(), 100.0), ("Rate".to_owned(), 0.2)];
/// assert_eq!(eval_pivot_formula("Sales * Rate", &row), Some(20.0));
/// assert_eq!(eval_pivot_formula("'Sales' / 0", &row), None);
/// assert_eq!(eval_pivot_formula("-Sales + 1", &row), Some(-99.0));
/// assert_eq!(eval_pivot_formula("SUM(Sales)", &row), None);
/// ```
///
/// An empty value is what the app can show before a refresh recomputes the
/// field from the source.
#[must_use]
pub fn eval_pivot_formula(formula: &str, fields: &[(String, f64)]) -> Option<f64> {
    let terms = Terms::parse(formula)?;
    let columns = |name: &str| {
        fields
            .iter()
            .position(|(field, _)| field.to_lowercase() == name.to_lowercase())
    };
    let bound = terms.bind(&columns)?;
    bound.eval(&|at| fields.get(at).map(|(_, value)| *value))
}

/// The years, quarters, months or days of a column of dates, as level
/// items. A date that is not a number leaves a blank, and so does a serial
/// that names no date.
fn part_column(
    source: &[Item],
    part: DatePart,
    epoch: Epoch,
    captions: &PivotCaptions,
) -> Vec<Item> {
    source
        .iter()
        .map(|item| {
            let Item::Number(serial) = item else {
                return Item::Blank;
            };
            let Ok(date) = date::from_serial(*serial, epoch) else {
                return Item::Blank;
            };
            let year = u32::try_from(date.year.max(0)).unwrap_or(u32::MAX);
            let (label, order) = match part {
                DatePart::Years => (year.to_string(), year),
                DatePart::Quarters => {
                    let quarter = date.month.saturating_sub(1) / 3 + 1;
                    (fill(&captions.quarter, &quarter.to_string()), quarter)
                }
                DatePart::Months => (captions.month_name(date.month).to_owned(), date.month),
                DatePart::Days => (
                    format!("{:02}-{}", date.day, captions.month_name(date.month)),
                    date.day,
                ),
            };
            Item::Level { label, order }
        })
        .collect()
}

/// A column of numbers folded into spans of `interval` between `start` and
/// `end`, labelled "10-20" after their bounds; values outside fall into the
/// rest group, which sorts last.
fn span_column(source: &[Item], range: &RangeGroup, other: &str) -> Vec<Item> {
    source
        .iter()
        .map(|item| {
            let Item::Number(value) = item else {
                return Item::Blank;
            };
            if !value.is_finite() || *value < range.start || *value > range.end {
                return Item::Level {
                    label: other.to_owned(),
                    order: u32::MAX,
                };
            }
            let at = ((*value - range.start) / range.interval).floor();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the floor is taken above and a negative span index cannot happen"
            )]
            let order = u32::try_from(at as u64).unwrap_or(u32::MAX - 1);
            let lo = range.start + at * range.interval;
            let label = format!(
                "{}-{}",
                general_number(lo),
                general_number(lo + range.interval)
            );
            Item::Level { label, order }
        })
        .collect()
}

/// What `function` makes of `values`. An error among them is the answer, as
/// in Excel; a function with nothing to work on gives what Excel shows.
///
/// `values` come in the report's cells, each tagged with its cell: a sum adds
/// up each cell's numbers and then the cells' sums.
fn summarise<'a, K: PartialEq>(
    function: Subtotal,
    values: impl Iterator<Item = (K, &'a Item)>,
) -> CellValue {
    let mut numbers = Vec::new();
    let mut filled = 0_u32;
    let mut sum = 0.0;
    let mut partial = 0.0;
    let mut cell = None;
    for (key, value) in values {
        if cell.as_ref() != Some(&key) {
            sum += partial;
            partial = 0.0;
            cell = Some(key);
        }
        match value {
            Item::Error(e) => return CellValue::Error(*e),
            Item::Number(n) => {
                numbers.push(*n);
                partial += n;
                filled += 1;
            }
            Item::Blank => {}
            Item::Text(_) | Item::Bool(_) | Item::Level { .. } => filled += 1,
        }
    }
    let n = f64::from(u32::try_from(numbers.len()).unwrap_or(u32::MAX));
    let sum = sum + partial;
    let deviations = || {
        let mean = sum / n;
        numbers.iter().map(|x| (x - mean).powi(2)).sum::<f64>()
    };
    let spread = |minimum: f64, by: f64, root: bool| {
        if n < minimum {
            return CellValue::Error(CellError::Div0);
        }
        let v = deviations() / by;
        CellValue::Number(if root { v.sqrt() } else { v })
    };
    match function {
        Subtotal::Sum => CellValue::Number(sum),
        Subtotal::Count => CellValue::Number(f64::from(filled)),
        Subtotal::CountNums => CellValue::Number(n),
        Subtotal::Average if numbers.is_empty() => CellValue::Error(CellError::Div0),
        Subtotal::Average => CellValue::Number(sum / n),
        Subtotal::Max => CellValue::Number(numbers.iter().copied().reduce(f64::max).unwrap_or(0.0)),
        Subtotal::Min => CellValue::Number(numbers.iter().copied().reduce(f64::min).unwrap_or(0.0)),
        Subtotal::Product if numbers.is_empty() => CellValue::Number(0.0),
        Subtotal::Product => CellValue::Number(numbers.iter().product()),
        Subtotal::StdDev => spread(2.0, n - 1.0, true),
        Subtotal::StdDevP => spread(1.0, n, true),
        Subtotal::Var => spread(2.0, n - 1.0, false),
        Subtotal::VarP => spread(1.0, n, false),
    }
}

/// One level of an entry's path: an item of a field, or one value field.
#[derive(Debug, Clone)]
enum Step {
    Item(Item),
    Data(usize),
}

impl Step {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Item(a), Self::Item(b)) => a.cmp(b) == Ordering::Equal,
            (Self::Data(a), Self::Data(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Leaf,
    /// The subtotal of the field at this level.
    Subtotal(usize),
    Grand,
}

/// A row or a column of the report.
#[derive(Debug, Clone)]
struct Entry {
    kind: Kind,
    path: Vec<Step>,
    /// The value field a total stands for, when the values field shares its
    /// axis and so each value field has a total of its own.
    data: Option<usize>,
    /// The records it summarises, in source order.
    records: Vec<usize>,
}

impl Entry {
    fn data(&self) -> Option<usize> {
        self.data.or_else(|| {
            self.path.iter().find_map(|s| match s {
                Step::Data(i) => Some(*i),
                Step::Item(_) => None,
            })
        })
    }

    /// Whether the label at `level` is the one the entry before already
    /// showed: a group's label stands on its first line only.
    fn continues(&self, before: Option<&Self>, level: usize) -> bool {
        before.is_some_and(|b| {
            b.path.len() > level
                && self.path.len() > level
                && (0..=level).all(|l| b.path[l].same(&self.path[l]))
        })
    }
}

/// The rows or the columns of the report.
struct Axis<'a> {
    fields: &'a [i32],
    data: &'a [Vec<Item>],
    subtotals: Vec<bool>,
    values: usize,
    out: Vec<Entry>,
}

impl Axis<'_> {
    fn entries(
        fields: &[i32],
        subtotals: &[bool],
        values: usize,
        data: &[Vec<Item>],
        records: &[usize],
        grand: bool,
    ) -> Vec<Entry> {
        let real = |x: &i32| *x != VALUES;
        let subtotals = fields
            .iter()
            .enumerate()
            .map(|(level, _)| {
                subtotals.get(level).copied().unwrap_or(false)
                    && fields[level + 1..].iter().any(real)
            })
            .collect();
        let mut axis = Axis {
            fields,
            data,
            subtotals,
            values,
            out: Vec::new(),
        };
        axis.level(0, records.to_vec(), Vec::new());
        if grand && fields.iter().any(real) {
            let entry = |data| Entry {
                kind: Kind::Grand,
                path: Vec::new(),
                data,
                records: records.to_vec(),
            };
            if fields.contains(&VALUES) {
                axis.out.extend((0..axis.values).map(|i| entry(Some(i))));
            } else {
                axis.out.push(entry(None));
            }
        }
        axis.out
    }

    fn level(&mut self, level: usize, records: Vec<usize>, path: Vec<Step>) {
        let Some(&field) = self.fields.get(level) else {
            self.out.push(Entry {
                kind: Kind::Leaf,
                path,
                data: None,
                records,
            });
            return;
        };
        let Ok(field) = usize::try_from(field) else {
            for i in 0..self.values {
                let mut path = path.clone();
                path.push(Step::Data(i));
                self.level(level + 1, records.clone(), path);
            }
            return;
        };
        let column = &self.data[field];
        let mut sorted = records;
        // Stable, so each group's first record is its first in the source.
        sorted.sort_by(|&a, &b| column[a].cmp(&column[b]));
        let groups: Vec<Vec<usize>> = sorted
            .chunk_by(|&a, &b| column[a].cmp(&column[b]) == Ordering::Equal)
            .map(<[usize]>::to_vec)
            .collect();
        let values_below = self.fields[level + 1..].contains(&VALUES);
        for group in groups {
            let mut path = path.clone();
            path.push(Step::Item(column[group[0]].clone()));
            self.level(level + 1, group.clone(), path.clone());
            if self.subtotals[level] {
                let entry = |data| Entry {
                    kind: Kind::Subtotal(level),
                    path: path.clone(),
                    data,
                    records: group.clone(),
                };
                if values_below {
                    let totals: Vec<Entry> = (0..self.values).map(|i| entry(Some(i))).collect();
                    self.out.extend(totals);
                } else {
                    self.out.push(entry(None));
                }
            }
        }
    }
}

/// Where everything in the report goes.
struct Layout {
    header_rows: usize,
    label_cols: usize,
    rows: Vec<Entry>,
    cols: Vec<Entry>,
    /// Each record's place among the column leaves, which is what a value
    /// cell collects its records by.
    leaf_of: Vec<usize>,
    /// The same for the row leaves.
    row_leaf_of: Vec<usize>,
    /// The leaves each column covers, as a range of `leaf_of` values.
    spans: Vec<(usize, usize)>,
}

impl Layout {
    fn new(def: &PivotTable, plans: &Plans, data: &[Vec<Item>], kept: &[usize]) -> Self {
        let height = data.iter().map(Vec::len).max().unwrap_or(0);
        let values = def.data_fields.len();
        let rows = Axis::entries(
            &plans.row_list,
            &plans.row_subtotals,
            values,
            data,
            kept,
            def.column_grand_totals,
        );
        let mut cols = Axis::entries(
            &plans.col_list,
            &plans.col_subtotals,
            values,
            data,
            kept,
            def.row_grand_totals,
        );
        if values == 0 && plans.col_list.is_empty() {
            cols.clear();
        }
        let leaf_of = leaves(&plans.col_list, data, kept, height);
        let row_leaf_of = leaves(&plans.row_list, data, kept, height);
        let spans = cols
            .iter()
            .map(|c| {
                let lo = c.records.iter().map(|&r| leaf_of[r]).min().unwrap_or(0);
                let hi = c
                    .records
                    .iter()
                    .map(|&r| leaf_of[r])
                    .max()
                    .map_or(0, |h| h + 1);
                (lo, hi)
            })
            .collect();
        let header_rows = 1 + plans.col_list.len();
        let label_cols = plans.rows.len().max(1);
        Self {
            header_rows,
            label_cols,
            rows,
            cols,
            leaf_of,
            row_leaf_of,
            spans,
        }
    }

    /// Every cell of the report, as (row, column, value) from its top-left.
    fn cells(
        &self,
        def: &PivotTable,
        data: &[Vec<Item>],
        captions: &PivotCaptions,
        plans: &Plans,
    ) -> Vec<(usize, usize, CellValue)> {
        let caption = |i: usize| {
            def.data_fields
                .get(i)
                .and_then(|d| d.name.clone())
                .unwrap_or_default()
        };
        let single = def.data_fields.len() == 1;
        let mut out = Vec::new();
        let last_header = self.header_rows - 1;

        // The headers.
        if !plans.cols.is_empty() && single && !plans.rows.is_empty() {
            out.push((0, 0, CellValue::text(caption(0))));
        }
        for (j, slot) in plans.cols.iter().enumerate() {
            out.push((
                0,
                self.label_cols + j,
                CellValue::text(slot.header.as_str()),
            ));
        }
        for (i, slot) in plans.rows.iter().enumerate() {
            out.push((last_header, i, CellValue::text(slot.header.as_str())));
        }
        if plans.cols.is_empty() && single {
            out.push((last_header, self.label_cols, CellValue::text(caption(0))));
        }
        for (k, col) in self.cols.iter().enumerate() {
            let before = k.checked_sub(1).and_then(|b| self.cols.get(b));
            for (level, label) in labels(col, before, &caption, captions) {
                out.push((1 + level, self.label_cols + k, label));
            }
        }
        let leaves = self.spans.iter().map(|s| s.1).max().unwrap_or(0);
        for (k, row) in self.rows.iter().enumerate() {
            let before = k.checked_sub(1).and_then(|b| self.rows.get(b));
            let r = self.header_rows + k;
            if plans.rows.is_empty() {
                if single {
                    out.push((r, 0, CellValue::text(caption(0))));
                }
            } else {
                for (level, label) in labels(row, before, &caption, captions) {
                    out.push((r, level, label));
                }
            }
            // The values: the row's records, bucketed by column leaf.
            if self.cols.is_empty() {
                continue;
            }
            let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); leaves];
            for &rec in &row.records {
                if let Some(b) = buckets.get_mut(self.leaf_of[rec]) {
                    b.push(rec);
                }
            }
            for (c, col) in self.cols.iter().enumerate() {
                let field = row.data().or_else(|| col.data()).unwrap_or(0);
                let Some(data_field) = def.data_fields.get(field) else {
                    continue;
                };
                let (lo, hi) = self.spans[c];
                let members = buckets.get(lo..hi).unwrap_or_default();
                if members.iter().all(Vec::is_empty) {
                    continue;
                }
                let Some(column) = usize::try_from(data_field.field)
                    .ok()
                    .and_then(|f| data.get(f))
                else {
                    continue;
                };
                // A total adds up the sums of the cells it covers, in the
                // order they are shown, as Excel does; that order only shows
                // in the last bits of a sum.
                let mut members: Vec<usize> = members.iter().flatten().copied().collect();
                members.sort_by_key(|&r| (self.row_leaf_of[r], self.leaf_of[r], r));
                let values = members.iter().filter_map(|&r| {
                    Some(((self.row_leaf_of[r], self.leaf_of[r]), column.get(r)?))
                });
                out.push((
                    r,
                    self.label_cols + c,
                    summarise(data_field.subtotal, values),
                ));
            }
        }
        out
    }
}

/// Each record's place among an axis's leaves without the values field:
/// records grouped by the items of the real fields, in the order the report
/// shows them. `usize::MAX` for a record no leaf holds.
fn leaves(fields: &[i32], data: &[Vec<Item>], kept: &[usize], height: usize) -> Vec<usize> {
    let real: Vec<i32> = fields.iter().copied().filter(|&x| x != VALUES).collect();
    let mut axis = Axis {
        fields: &real,
        data,
        subtotals: vec![false; real.len()],
        values: 0,
        out: Vec::new(),
    };
    axis.level(0, kept.to_vec(), Vec::new());
    let mut out = vec![usize::MAX; height];
    for (i, leaf) in axis.out.iter().enumerate() {
        for &r in &leaf.records {
            out[r] = i;
        }
    }
    out
}

/// The labels an entry shows, by level: a leaf's items where its group
/// starts, a total's word at its own level.
fn labels(
    entry: &Entry,
    before: Option<&Entry>,
    caption: &dyn Fn(usize) -> String,
    captions: &PivotCaptions,
) -> Vec<(usize, CellValue)> {
    match entry.kind {
        Kind::Leaf => entry
            .path
            .iter()
            .enumerate()
            .filter(|(level, _)| !entry.continues(before, *level))
            .map(|(level, step)| {
                let label = match step {
                    Step::Item(item) => item.cell(captions),
                    Step::Data(i) => CellValue::text(caption(*i)),
                };
                (level, label)
            })
            .collect(),
        Kind::Subtotal(level) => {
            let name = match entry.path.get(level) {
                Some(Step::Item(item)) => item.name(captions),
                Some(Step::Data(i)) => caption(*i),
                None => String::new(),
            };
            let label = match entry.data {
                Some(i) => format!("{name} {}", caption(i)),
                None => fill(&captions.total, &name),
            };
            vec![(level, CellValue::text(label))]
        }
        Kind::Grand => {
            let label = match entry.data {
                Some(i) => fill(&captions.total_of, &caption(i)),
                None => captions.grand_total.clone(),
            };
            vec![(0, CellValue::text(label))]
        }
    }
}

/// Refreshes every pivot table of the workbook, as Excel's Refresh All does,
/// and returns how many were laid out.
///
/// # Errors
/// The first error of [`refresh_pivot`]; the reports before it stay
/// refreshed.
pub fn refresh_all_pivots(book: &mut Spreadsheet, captions: &PivotCaptions) -> Result<usize> {
    let mut done = 0;
    for sheet in 0..book.sheets().len() {
        let count = book.sheet(sheet).map_or(0, |ws| ws.pivot_tables.len());
        for table in 0..count {
            refresh_pivot(book, sheet, table, captions)?;
            done += 1;
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(function: Subtotal, values: &[Item]) -> CellValue {
        summarise(function, values.iter().map(|v| ((), v)))
    }

    #[test]
    fn each_function_summarises_as_excel_does() {
        let values = [
            Item::Number(2.0),
            Item::Number(4.0),
            Item::Text("x".into()),
            Item::Blank,
            Item::Number(6.0),
        ];
        let number = |f| match run(f, &values) {
            CellValue::Number(n) => n,
            other => panic!("{f:?}: {other:?}"),
        };
        assert_eq!(number(Subtotal::Sum), 12.0);
        assert_eq!(
            number(Subtotal::Count),
            4.0,
            "text counts, a blank does not"
        );
        assert_eq!(number(Subtotal::CountNums), 3.0);
        assert_eq!(number(Subtotal::Average), 4.0);
        assert_eq!(number(Subtotal::Max), 6.0);
        assert_eq!(number(Subtotal::Min), 2.0);
        assert_eq!(number(Subtotal::Product), 48.0);
        assert_eq!(number(Subtotal::Var), 4.0);
        assert_eq!(number(Subtotal::StdDev), 2.0);
        assert!((number(Subtotal::VarP) - 8.0 / 3.0).abs() < 1e-12);
        assert!((number(Subtotal::StdDevP) - (8.0_f64 / 3.0).sqrt()).abs() < 1e-12);

        let one = [Item::Number(5.0)];
        assert_eq!(
            run(Subtotal::StdDev, &one),
            CellValue::Error(CellError::Div0)
        );
        assert_eq!(run(Subtotal::VarP, &one), CellValue::Number(0.0));
        let text = [Item::Text("x".into())];
        assert_eq!(
            run(Subtotal::Average, &text),
            CellValue::Error(CellError::Div0)
        );
        assert_eq!(run(Subtotal::Max, &text), CellValue::Number(0.0));
        let error = [Item::Number(1.0), Item::Error(CellError::Ref)];
        assert_eq!(
            run(Subtotal::Count, &error),
            CellValue::Error(CellError::Ref)
        );
    }

    #[test]
    fn items_sort_numbers_text_logicals_errors_blanks() {
        let mut items = [
            Item::Blank,
            Item::Error(CellError::Na),
            Item::Bool(true),
            Item::Text("b".into()),
            Item::Bool(false),
            Item::Text("A".into()),
            Item::Number(10.0),
            Item::Number(-1.0),
        ];
        items.sort_by(Item::cmp);
        let shown: Vec<String> = items.iter().map(Item::text).collect();
        assert_eq!(shown, ["-1", "10", "A", "b", "FALSE", "TRUE", "#N/A", ""]);
        assert_eq!(
            Item::Text("Ёж".into()).cmp(&Item::Text("ёж".into())),
            Ordering::Equal
        );
    }
}
