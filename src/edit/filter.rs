//! Filtering a table: the criterion of one column, and the rows it hides.
//!
//! A filter in the file is two things that are free to disagree: the
//! criteria on `<autoFilter>` and the `hidden` flag on each row. Excel writes
//! both, so a filter set here does too - the criterion goes on the table and
//! every data row is shown or hidden by all the criteria the table holds.

use crate::coordinate::{CellRef, Col, Range, Row};
use crate::error::{Error, Result};
use crate::model::autofilter::{AutoFilter, ColumnFilter, CustomFilter, DateGroup, FilterOperator};
use crate::model::{CellValue, Spreadsheet};

/// Sets what one column of a table keeps, then hides the data rows that the
/// table's criteria reject and shows the rest, as applying a filter in Excel
/// does. `column` counts from 0 at the table's first column; `None` takes the
/// column's criterion off. The table is found by name anywhere in the book.
///
/// The table gets the drop-down arrows if it had none. Criteria Excel
/// evaluates against the clock or the formatting - [`ColumnFilter::Dynamic`],
/// [`ColumnFilter::Color`], [`ColumnFilter::Icon`] - are stored but hide
/// nothing here.
///
/// ```
/// use excelerate::edit::filter_table;
/// use excelerate::model::autofilter::ColumnFilter;
/// use excelerate::{Row, reader};
///
/// // Sales over A1:C5: Region, Quarter, Sales; two rows each for North and South.
/// let mut book = reader::read("tests/fixtures/table.xlsx")?;
/// let south = ColumnFilter::Values {
///     blank: false,
///     values: vec!["South".into()],
///     date_groups: Vec::new(),
/// };
/// filter_table(&mut book, "Sales", 0, Some(south))?;
///
/// let hidden = |r: u32| book.sheets()[0].rows.get(&Row::new(r).unwrap()).is_some_and(|p| p.hidden);
/// assert_eq!((1..=4).map(hidden).collect::<Vec<_>>(), [true, true, false, false]);
/// # Ok::<(), excelerate::Error>(())
/// ```
///
/// # Errors
/// [`Error::InvalidRange`] if there is no table of that name or it has no
/// such column.
pub fn filter_table(
    book: &mut Spreadsheet,
    name: &str,
    column: u32,
    filter: Option<ColumnFilter>,
) -> Result<()> {
    let found = book.sheets().iter().enumerate().find_map(|(sheet, ws)| {
        ws.tables
            .iter()
            .position(|t| {
                t.display_name.eq_ignore_ascii_case(name) || t.name.eq_ignore_ascii_case(name)
            })
            .map(|table| (sheet, table))
    });
    let Some((sheet, index)) = found else {
        return Err(Error::InvalidRange(format!("no table called {name}")));
    };
    let ws = book
        .sheet_mut(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    let table = &mut ws.tables[index];
    let width = table.range.end.col.index() - table.range.start.col.index() + 1;
    if column >= width {
        return Err(Error::InvalidRange(format!(
            "table {name} has {width} columns, not {}",
            column + 1
        )));
    }
    let range = table.range;
    let totals = table.totals_row_count.unwrap_or(0);
    let filter_area = Row::new(range.end.row.index().saturating_sub(totals)).map_or(range, |end| {
        Range::new(range.start, CellRef::new(range.end.col, end))
    });
    let body = table.body();
    let auto = table
        .auto_filter
        .get_or_insert_with(|| AutoFilter::new(filter_area));
    auto.column_at(column).filter = filter;
    auto.columns
        .retain(|c| c.filter.is_some() || c.hidden_button);
    let Some(body) = body else {
        return Ok(());
    };
    let mut auto = auto.clone();
    let hidden = rejected(book, sheet, &mut auto, body);
    let ws = book
        .sheet_mut(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    for (row, hide) in hidden {
        ws.set_row_hidden(row, hide);
    }
    // Top 10 records the cut-off it found, as Excel does.
    ws.tables[index].auto_filter = Some(auto);
    Ok(())
}

/// Each row of `body` and whether the criteria hide it.
fn rejected(
    book: &Spreadsheet,
    sheet: usize,
    auto: &mut AutoFilter,
    body: Range,
) -> Vec<(Row, bool)> {
    let first = auto.range.start.col.index();
    let rows: Vec<Row> = (body.start.row.index()..=body.end.row.index())
        .filter_map(Row::new)
        .collect();
    let mut keep = vec![true; rows.len()];
    for column in &mut auto.columns {
        let Some(filter) = column.filter.as_mut() else {
            continue;
        };
        let Some(col) = Col::new(first + column.col_id) else {
            continue;
        };
        let cells: Vec<CellRef> = rows.iter().map(|&row| CellRef::new(col, row)).collect();
        let cutoff = match filter {
            ColumnFilter::Top10 {
                value,
                percent,
                top,
                filter_value,
            } => {
                let cutoff = top_cutoff(book, sheet, &cells, value.as_deref(), *percent, *top);
                *filter_value = cutoff.map(|c| c.to_string());
                cutoff
            }
            _ => None,
        };
        for (kept, &at) in keep.iter_mut().zip(&cells) {
            *kept = *kept && keeps(book, sheet, at, filter, cutoff);
        }
    }
    rows.into_iter()
        .zip(keep)
        .map(|(row, k)| (row, !k))
        .collect()
}

/// Whether one cell passes one criterion.
fn keeps(
    book: &Spreadsheet,
    sheet: usize,
    at: CellRef,
    filter: &ColumnFilter,
    cutoff: Option<f64>,
) -> bool {
    let value = book.sheets()[sheet]
        .get(at)
        .map_or(&CellValue::Empty, |cell| cell.value.result());
    match filter {
        ColumnFilter::Values {
            blank,
            values,
            date_groups,
        } => {
            if matches!(value, CellValue::Empty) {
                return *blank;
            }
            let shown = book.formatted(sheet, at).to_lowercase();
            values.iter().any(|v| v.to_lowercase() == shown)
                || number(value)
                    .is_some_and(|n| date_groups.iter().any(|group| in_group(n, group, book)))
        }
        ColumnFilter::Custom { and, rules } => {
            let shown = book.formatted(sheet, at);
            let mut results = rules.iter().map(|rule| passes(value, &shown, rule));
            if *and {
                results.all(|r| r)
            } else {
                results.any(|r| r)
            }
        }
        ColumnFilter::Top10 { top, .. } => match (number(value), cutoff) {
            (Some(n), Some(cut)) => {
                if *top {
                    n >= cut
                } else {
                    n <= cut
                }
            }
            _ => false,
        },
        // ponytail: dynamic, colour and icon criteria need the clock, the
        // resolved cell colours or the conditional formats; they keep every
        // row until something here can evaluate them.
        _ => true,
    }
}

/// One comparison of a custom filter: numbers against a number, text
/// against text without regard to case, `*` and `?` in an equality.
fn passes(value: &CellValue, shown: &str, rule: &CustomFilter) -> bool {
    use std::cmp::Ordering;
    let order = match (number(value), rule.value.trim().parse::<f64>()) {
        (Some(n), Ok(against)) => n.partial_cmp(&against),
        (None, Err(_)) if !matches!(value, CellValue::Number(_)) => {
            let (text, pattern) = (shown.to_uppercase(), rule.value.to_uppercase());
            if matches!(
                rule.operator,
                FilterOperator::Equal | FilterOperator::NotEqual
            ) {
                let equal = crate::shared::wildcard_match(&pattern, &text);
                return equal == (rule.operator == FilterOperator::Equal);
            }
            Some(text.cmp(&pattern))
        }
        _ => None,
    };
    let Some(order) = order else {
        return rule.operator == FilterOperator::NotEqual;
    };
    match rule.operator {
        FilterOperator::Equal => order == Ordering::Equal,
        FilterOperator::NotEqual => order != Ordering::Equal,
        FilterOperator::GreaterThan => order == Ordering::Greater,
        FilterOperator::GreaterThanOrEqual => order != Ordering::Less,
        FilterOperator::LessThan => order == Ordering::Less,
        FilterOperator::LessThanOrEqual => order != Ordering::Greater,
    }
}

/// The number a cell shows as a number, its formula's result included.
fn number(value: &CellValue) -> Option<f64> {
    match value {
        CellValue::Number(n) => Some(*n),
        _ => None,
    }
}

/// The value the `n`th largest (or smallest) number of the column has, `n`
/// being a count or a percentage of the numbers there.
fn top_cutoff(
    book: &Spreadsheet,
    sheet: usize,
    cells: &[CellRef],
    value: Option<&str>,
    percent: bool,
    top: bool,
) -> Option<f64> {
    let ws = &book.sheets()[sheet];
    let mut numbers: Vec<f64> = cells
        .iter()
        .filter_map(|&at| ws.get(at).and_then(|c| number(c.value.result())))
        .collect();
    let asked: f64 = value?.trim().parse().ok()?;
    if numbers.is_empty() || asked.is_nan() || asked <= 0.0 {
        return None;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a column holds at most a million rows"
    )]
    let wanted = if percent {
        (numbers.len() as f64 * asked / 100.0).ceil()
    } else {
        asked.floor()
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "positive, and clamped to the count below"
    )]
    let n = (wanted.max(1.0) as usize).min(numbers.len());
    numbers.sort_by(f64::total_cmp);
    if top {
        numbers.reverse();
    }
    numbers.get(n - 1).copied()
}

/// Whether a serial date falls in a date group: every part the group names
/// matches.
fn in_group(serial: f64, group: &DateGroup, book: &Spreadsheet) -> bool {
    let Ok(when) = crate::shared::date::from_serial(serial, book.epoch) else {
        return false;
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "seconds of a minute"
    )]
    let parts = [
        (group.year, u32::try_from(when.year).ok()),
        (group.month, Some(when.month)),
        (group.day, Some(when.day)),
        (group.hour, Some(when.hour)),
        (group.minute, Some(when.minute)),
        (group.second, Some(when.second.floor() as u32)),
    ];
    parts
        .iter()
        .all(|(want, have)| want.is_none_or(|w| Some(w) == *have))
}
