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
//! ponytail: the compact and outline forms, `show_all`, hidden items, number
//! formats on the values, sorting by value and calculated fields are not
//! laid out; Excel lays the report out again on open anyway, as the writer
//! marks the cache `refreshOnLoad`.

use crate::coordinate::{CellRef, Col, Range, Row};
use crate::error::{CellError, Error, Result};
use crate::model::pivot::{CacheField, PivotAxis, PivotField, PivotTable, Subtotal};
use crate::model::{CellValue, Spreadsheet, Worksheet};
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
}

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
        }
    }
}

impl PivotCaptions {
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
/// field index past the source's columns, or a report that would leave the
/// sheet or overlap its source. Nothing is changed then.
pub fn refresh_pivot(
    book: &mut Spreadsheet,
    sheet: usize,
    table: usize,
    captions: &PivotCaptions,
) -> Result<Range> {
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

    check_fields(&def, count)?;
    normalise(&mut def, &names, captions);
    let on_axis: Vec<usize> = def
        .row_fields
        .iter()
        .chain(&def.column_fields)
        .chain(&def.page_fields)
        .filter_map(|&x| usize::try_from(x).ok())
        .collect();
    let used: Vec<usize> = on_axis
        .iter()
        .copied()
        .chain(
            def.data_fields
                .iter()
                .filter_map(|d| usize::try_from(d.field).ok()),
        )
        .collect();
    let data = records(src, source, count, &used)?;
    let height = data.iter().map(Vec::len).max().unwrap_or(0);

    // The page filters, and the cache's items for every field on an axis.
    let mut shared: Vec<Vec<String>> = vec![Vec::new(); count];
    for &f in &on_axis {
        shared[f] = items_of(&data[f]);
    }
    let kept = page_filter(&mut def, &data, &shared, height);

    let layout = Layout::new(&def, &data, &kept);
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
    let cells = layout.cells(&def, &data, &names, captions);

    def.location = Some(area);
    def.first_data_row = u32::try_from(layout.header_rows).unwrap_or(u32::MAX);
    def.first_data_col = u32::try_from(layout.label_cols).unwrap_or(u32::MAX);
    let cache = &mut book.pivot_caches[cache_at];
    cache.fields = names
        .into_iter()
        .zip(shared)
        .enumerate()
        .map(|(i, (name, shared_items))| CacheField {
            name,
            number_format: cache.fields.get(i).and_then(|f| f.number_format),
            shared_items,
        })
        .collect();

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

/// The records the page filters let through. A filter whose item the source
/// no longer has shows everything again, as in Excel.
fn page_filter(
    def: &mut PivotTable,
    data: &[Vec<Item>],
    shared: &[Vec<String>],
    height: usize,
) -> Vec<usize> {
    let mut kept: Vec<usize> = (0..height).collect();
    for &p in &def.page_fields {
        let Ok(p) = usize::try_from(p) else { continue };
        let field = &mut def.fields[p];
        if let Some(wanted) = &field.page_item {
            let wanted = wanted.to_lowercase();
            if shared[p].iter().any(|s| s.to_lowercase() == wanted) {
                kept.retain(|&r| data[p][r].text().to_lowercase() == wanted);
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

/// Every field the definition names is a column of the source.
fn check_fields(def: &PivotTable, count: usize) -> Result<()> {
    let in_source = |x: i32| usize::try_from(x).is_ok_and(|x| x < count);
    let listed = def
        .row_fields
        .iter()
        .chain(&def.column_fields)
        .chain(&def.page_fields)
        .copied()
        .filter(|&x| x != VALUES);
    let data_listed = def.data_fields.iter().map(|d| i32::try_from(d.field));
    match listed
        .map(Ok)
        .chain(data_listed)
        .find(|x| !x.is_ok_and(in_source))
    {
        Some(x) => Err(bad(format!(
            "pivot table {:?} uses field {}, and its source has {count}",
            def.name,
            x.map_or_else(|_| "past i32".to_owned(), |x| x.to_string())
        ))),
        None => Ok(()),
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
fn normalise(def: &mut PivotTable, names: &[String], captions: &PivotCaptions) {
    let count = names.len();
    def.fields.resize(
        count,
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
        let source = usize::try_from(data.field)
            .ok()
            .and_then(|f| names.get(f))
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
            Self::Text(_) => 1,
            Self::Bool(_) => 2,
            Self::Error(_) => 3,
            Self::Blank => 4,
        }
    }

    /// Excel's pivot order, which is also its notion of the same item.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
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
            Item::Text(_) | Item::Bool(_) => filled += 1,
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
        def: &PivotTable,
        data: &[Vec<Item>],
        records: &[usize],
        grand: bool,
    ) -> Vec<Entry> {
        let real = |x: &i32| *x != VALUES;
        let subtotals = fields
            .iter()
            .enumerate()
            .map(|(level, &f)| {
                usize::try_from(f).is_ok_and(|f| def.fields[f].default_subtotal)
                    && fields[level + 1..].iter().any(real)
            })
            .collect();
        let mut axis = Axis {
            fields,
            data,
            subtotals,
            values: def.data_fields.len(),
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
    fn new(def: &PivotTable, data: &[Vec<Item>], kept: &[usize]) -> Self {
        let height = data.iter().map(Vec::len).max().unwrap_or(0);
        let rows = Axis::entries(&def.row_fields, def, data, kept, def.column_grand_totals);
        let mut cols = Axis::entries(&def.column_fields, def, data, kept, def.row_grand_totals);
        if def.data_fields.is_empty() && def.column_fields.is_empty() {
            cols.clear();
        }
        let leaf_of = leaves(&def.column_fields, data, kept, height);
        let row_leaf_of = leaves(&def.row_fields, data, kept, height);
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
        let header_rows = 1 + def.column_fields.len();
        let label_cols = def.row_fields.len().max(1);
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
        names: &[String],
        captions: &PivotCaptions,
    ) -> Vec<(usize, usize, CellValue)> {
        let caption = |i: usize| {
            def.data_fields
                .get(i)
                .and_then(|d| d.name.clone())
                .unwrap_or_default()
        };
        let field_name = |x: i32| {
            usize::try_from(x)
                .ok()
                .and_then(|f| names.get(f))
                .map_or_else(|| captions.values.clone(), Clone::clone)
        };
        let single = def.data_fields.len() == 1;
        let mut out = Vec::new();
        let last_header = self.header_rows - 1;

        // The headers.
        if !def.column_fields.is_empty() && single && !def.row_fields.is_empty() {
            out.push((0, 0, CellValue::text(caption(0))));
        }
        for (j, &x) in def.column_fields.iter().enumerate() {
            out.push((0, self.label_cols + j, CellValue::text(field_name(x))));
        }
        for (i, &x) in def.row_fields.iter().enumerate() {
            out.push((last_header, i, CellValue::text(field_name(x))));
        }
        if def.column_fields.is_empty() && single {
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
            if def.row_fields.is_empty() {
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
