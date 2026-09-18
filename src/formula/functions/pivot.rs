//! `GETPIVOTDATA`: a value out of a pivot report.
//!
//! The report is not computed here - it is already laid out in the cells,
//! written by whatever made the file - so this reads those cells: the labels
//! down the side, the headers across the top, and the value where the two
//! meet. That is also what makes it answer the same as Excel for a report
//! Excel laid out, and answer `#REF!` for one that was never laid out.

use crate::coordinate::{CellRef, Col, Row};
use crate::error::CellError;
use crate::formula::eval::{Engine, Origin, spanned};
use crate::formula::parser::Expr;
use crate::formula::value::Value;
use crate::model::pivot::PivotTable;
use crate::model::{CellValue, Worksheet};

/// `GETPIVOTDATA(data_field, pivot_table, [field, item]...)`
///
/// `pivot_table` is a reference to any cell of the report. Each pair after it
/// narrows the answer to one row or column; with no pairs the answer is the
/// grand total of that value field.
pub fn getpivotdata(engine: &mut Engine<'_>, origin: Origin, args: &[Expr]) -> Value {
    let [wanted, reference, pairs @ ..] = args else {
        return Value::Error(CellError::Value);
    };
    if pairs.len() % 2 != 0 {
        return Value::Error(CellError::Value);
    }
    let field = match engine.eval_value(origin, wanted).scalar().text() {
        Ok(text) => text,
        Err(e) => return Value::Error(e),
    };
    let Some((sheet_name, at)) = spanned(reference) else {
        return Value::Error(CellError::Ref);
    };
    let index = match sheet_name {
        None => origin.sheet,
        Some(name) => {
            match engine
                .book()
                .sheets()
                .iter()
                .position(|s| s.title().eq_ignore_ascii_case(&name))
            {
                Some(index) => index,
                None => return Value::Error(CellError::Ref),
            }
        }
    };
    let mut asked = Vec::with_capacity(pairs.len() / 2);
    for pair in pairs.chunks(2) {
        let name = match engine.eval_value(origin, &pair[0]).scalar().text() {
            Ok(text) => text,
            Err(e) => return Value::Error(e),
        };
        asked.push((name, engine.eval_value(origin, &pair[1]).scalar().clone()));
    }
    let book = engine.book();
    let Some(sheet) = book.sheet(index) else {
        return Value::Error(CellError::Ref);
    };
    let Some(table) = sheet
        .pivot_tables
        .iter()
        .find(|t| t.location.is_some_and(|area| area.contains(at.start)))
    else {
        return Value::Error(CellError::Ref);
    };
    // The names of the cache's fields, which is what the arguments name: the
    // report shows "Sum of Sales" where the formula says "Sales".
    let cache = book.pivot_caches.iter().find(|c| c.id == table.cache_id);
    let field_name = |index: i64| -> Option<&str> {
        let index = usize::try_from(index).ok()?;
        cache.map(|c| c.fields.get(index).map_or("", |f| f.name.as_str()))
    };
    Report {
        sheet,
        table,
        field_name: &field_name,
    }
    .find(&field, &asked)
}

/// The cells of one report, and what the model says about their layout.
struct Report<'a> {
    sheet: &'a Worksheet,
    table: &'a PivotTable,
    /// The name of a cache field by its index, for the axes' field lists.
    field_name: &'a dyn Fn(i64) -> Option<&'a str>,
}

/// What a report cell says, as text; empty for a cell with nothing in it.
fn text_of(sheet: &Worksheet, at: CellRef) -> String {
    sheet
        .get(at)
        .map(|c| &c.value)
        .and_then(|v| match v {
            CellValue::Text(t) => Some(t.to_string()),
            CellValue::Number(n) => Some(crate::style::format::format(
                crate::style::format::Value::Number(*n),
                crate::style::format::GENERAL,
                crate::shared::date::Epoch::Windows1900,
            )),
            CellValue::Bool(b) => Some(if *b { "TRUE" } else { "FALSE" }.to_owned()),
            CellValue::Formula { cached, .. } => match cached.as_deref() {
                Some(CellValue::Text(t)) => Some(t.to_string()),
                Some(CellValue::Number(n)) => Some(format!("{n}")),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_default()
}

/// Whether a label stands for the item asked about. A subtotal carries the
/// item's name and a word after it - "North Total", "Администрация Итог" -
/// and Excel answers those with the subtotal, so they count as the item.
fn says(label: &str, item: &str, allow_total: bool) -> bool {
    let (label, item) = (label.trim(), item.trim());
    label.eq_ignore_ascii_case(item)
        || allow_total
            && label.get(..item.len()).is_some_and(|head| {
                head.eq_ignore_ascii_case(item) && label[item.len()..].starts_with(' ')
            })
}

impl Report<'_> {
    /// The value for `field` where every pair holds, or `#REF!`.
    fn find(&self, field: &str, asked: &[(String, Value)]) -> Value {
        let Some(area) = self.table.location else {
            return Value::Error(CellError::Ref);
        };
        let headers = self.table.first_data_row.max(1);
        let labels = self.table.first_data_col.max(1);
        let Some(first_row) = Row::new(area.start.row.index() + headers) else {
            return Value::Error(CellError::Ref);
        };
        let Some(first_col) = Col::new(area.start.col.index() + labels) else {
            return Value::Error(CellError::Ref);
        };
        let mut rows: Vec<Row> = (first_row.index()..=area.end.row.index())
            .filter_map(Row::new)
            .collect();
        let mut cols: Vec<Col> = (first_col.index()..=area.end.col.index())
            .filter_map(Col::new)
            .collect();
        let label_cols: Vec<Col> = (area.start.col.index()..first_col.index())
            .filter_map(Col::new)
            .collect();
        let header_rows: Vec<Row> = (area.start.row.index()..first_row.index())
            .filter_map(Row::new)
            .collect();

        // A field the report does not show as a value is not something it can
        // answer, whatever the cells happen to say.
        let Some(caption) = self.caption(field) else {
            return Value::Error(CellError::Ref);
        };
        // With several value fields the report gives each a column (or a row)
        // of its own, under its caption.
        let mut narrowed_cols = false;
        if self.table.data_fields.len() > 1 {
            // The caption is what the report writes over the column, so it is
            // matched as it stands - no allowance for a name that merely
            // starts the same way, or "Sales" would take "Sales with VAT".
            let names = [caption.clone(), field.to_owned()];
            narrowed_cols = names
                .iter()
                .any(|name| keep_cols(self.sheet, &mut cols, &header_rows, name, first_col, false));
            if !narrowed_cols
                && !names.iter().any(|name| {
                    keep_rows(self.sheet, &mut rows, &label_cols, name, first_row, false)
                })
            {
                return Value::Error(CellError::Ref);
            }
        }
        let mut narrowed_rows = false;
        for (name, item) in asked {
            let item = match item.text() {
                Ok(text) => text,
                Err(e) => return Value::Error(e),
            };
            let column = self.axis_column(name, &label_cols);
            let row = self.axis_row(name, &header_rows);
            let found = match (column, row) {
                (Some(col), _) => Found::of(
                    keep_rows(self.sheet, &mut rows, &[col], &item, first_row, true),
                    Found::Rows,
                ),
                (_, Some(row)) => Found::of(
                    keep_cols(self.sheet, &mut cols, &[row], &item, first_col, true),
                    Found::Cols,
                ),
                // A report that puts every row field in one column - Excel's
                // compact layout - does not say which field a label belongs
                // to, so the label is looked for wherever the labels are.
                (None, None) => {
                    if keep_rows(self.sheet, &mut rows, &label_cols, &item, first_row, true) {
                        Found::Rows
                    } else {
                        Found::of(
                            keep_cols(self.sheet, &mut cols, &header_rows, &item, first_col, true),
                            Found::Cols,
                        )
                    }
                }
            };
            match found {
                Found::Rows => narrowed_rows = true,
                Found::Cols => narrowed_cols = true,
                Found::Nothing => return Value::Error(CellError::Ref),
            }
        }
        // Nothing asked about the rows, or the columns: the answer is then the
        // grand total, which the report puts last.
        if !narrowed_rows && self.table.row_grand_totals && rows.len() > 1 {
            rows = rows.split_off(rows.len() - 1);
        }
        if !narrowed_cols && self.table.column_grand_totals && cols.len() > 1 {
            cols = cols.split_off(cols.len() - 1);
        }
        self.total_rows(&mut rows, asked, &label_cols);
        let (Some(&row), Some(&col)) = (rows.first(), cols.first()) else {
            return Value::Error(CellError::Ref);
        };
        self.value_at(CellRef::new(col, row))
    }

    /// A field nobody asked about leaves its total: among the rows still in
    /// play, the ones where that field has no label of its own.
    fn total_rows(&self, rows: &mut Vec<Row>, asked: &[(String, Value)], label_cols: &[Col]) {
        let asked_columns: Vec<Col> = asked
            .iter()
            .filter_map(|(name, _)| self.axis_column(name, label_cols))
            .collect();
        let untouched: Vec<Col> = label_cols
            .iter()
            .copied()
            .filter(|c| !asked_columns.contains(c))
            .collect();
        if rows.len() > 1 && !untouched.is_empty() {
            let totals: Vec<Row> = rows
                .iter()
                .copied()
                .filter(|&row| {
                    untouched
                        .iter()
                        .all(|&col| text_of(self.sheet, CellRef::new(col, row)).is_empty())
                })
                .collect();
            if !totals.is_empty() {
                *rows = totals;
            }
        }
    }

    /// What one cell of the report says, as a formula sees it.
    fn value_at(&self, at: CellRef) -> Value {
        match self.sheet.get(at).map(|c| &c.value) {
            Some(CellValue::Number(n)) => Value::Number(*n),
            Some(CellValue::Text(t)) => Value::Text(t.to_string()),
            Some(CellValue::Bool(b)) => Value::Bool(*b),
            Some(CellValue::Error(e)) => Value::Error(*e),
            Some(CellValue::Formula { cached, .. }) => match cached.as_deref() {
                Some(value) => crate::formula::eval::stored_value(value),
                None => Value::Error(CellError::Ref),
            },
            _ => Value::Error(CellError::Ref),
        }
    }

    /// The caption a value field is shown under; `None` when the report has
    /// no such value field at all.
    fn caption(&self, field: &str) -> Option<String> {
        let found = self.table.data_fields.iter().find(|d| {
            (self.field_name)(i64::from(d.field))
                .is_some_and(|name| name.eq_ignore_ascii_case(field))
                || d.name
                    .as_deref()
                    .is_some_and(|caption| caption.eq_ignore_ascii_case(field))
        })?;
        Some(found.name.clone().unwrap_or_default())
    }

    /// The label column a row field of this name occupies, when the report
    /// gives each field a column of its own.
    fn axis_column(&self, name: &str, label_cols: &[Col]) -> Option<Col> {
        let at = self.position(&self.table.row_fields, name)?;
        (self.table.row_fields.len() <= label_cols.len())
            .then(|| label_cols.get(at).copied())
            .flatten()
    }

    /// The header row a column field of this name occupies.
    fn axis_row(&self, name: &str, header_rows: &[Row]) -> Option<Row> {
        let at = self.position(&self.table.column_fields, name)?;
        (self.table.column_fields.len() <= header_rows.len())
            .then(|| header_rows.get(at).copied())
            .flatten()
    }

    fn position(&self, axis: &[i32], name: &str) -> Option<usize> {
        axis.iter().position(|&index| {
            (self.field_name)(i64::from(index))
                .is_some_and(|field| field.eq_ignore_ascii_case(name))
        })
    }
}

/// Which way a pair narrowed the report, if it did.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Found {
    Rows,
    Cols,
    Nothing,
}

impl Found {
    const fn of(found: bool, way: Self) -> Self {
        if found { way } else { Self::Nothing }
    }
}

/// Keeps the rows whose label in one of `columns` says `item`. A label stands
/// for every row under it until the next one, which is how a report writes a
/// group: the name once, on its first row.
fn keep_rows(
    sheet: &Worksheet,
    rows: &mut Vec<Row>,
    columns: &[Col],
    item: &str,
    first: Row,
    allow_total: bool,
) -> bool {
    let kept: Vec<Row> = rows
        .iter()
        .copied()
        .filter(|&row| {
            columns.iter().any(|&col| {
                let label = filled_down(sheet, col, row, first);
                says(&label, item, allow_total)
            })
        })
        .collect();
    if kept.is_empty() {
        return false;
    }
    *rows = kept;
    true
}

/// The same across: the headers of the columns.
fn keep_cols(
    sheet: &Worksheet,
    cols: &mut Vec<Col>,
    rows: &[Row],
    item: &str,
    first: Col,
    allow_total: bool,
) -> bool {
    let kept: Vec<Col> = cols
        .iter()
        .copied()
        .filter(|&col| {
            rows.iter().any(|&row| {
                let header = filled_right(sheet, row, col, first);
                says(&header, item, allow_total)
            })
        })
        .collect();
    if kept.is_empty() {
        return false;
    }
    *cols = kept;
    true
}

/// The label standing over `row` in `col`: its own, or the last one above it.
fn filled_down(sheet: &Worksheet, col: Col, row: Row, first: Row) -> String {
    for index in (first.index()..=row.index()).rev() {
        let Some(row) = Row::new(index) else { continue };
        let text = text_of(sheet, CellRef::new(col, row));
        if !text.is_empty() {
            return text;
        }
    }
    String::new()
}

/// The header standing over `col` in `row`: its own, or the last one to its
/// left.
fn filled_right(sheet: &Worksheet, row: Row, col: Col, first: Col) -> String {
    for index in (first.index()..=col.index()).rev() {
        let Some(col) = Col::new(index) else { continue };
        let text = text_of(sheet, CellRef::new(col, row));
        if !text.is_empty() {
            return text;
        }
    }
    String::new()
}
