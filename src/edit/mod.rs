//! Inserting and removing rows and columns, and what that does to the rest of
//! the workbook.
//!
//!
//! A reference the edit deletes outright becomes `#REF!`, as it does in Excel;
//! one that merely loses part of a range is narrowed instead. `$` does not
//! protect a reference here: an absolute address names a cell, and inserting a
//! row above that cell moves the cell.

mod anchor;
// mod chart;

use crate::coordinate::{
    CellRef, Col, MAX_COL, MAX_ROW, Range, Row, parse_ref_at, scan_formula, scan_references,
};
use crate::error::{Error, Result};
use crate::model::{CellValue, Spreadsheet, Worksheet};

/// Which way the grid is being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Rows, numbered as the user sees them.
    Rows,
    /// Columns.
    Columns,
}

/// Inserts `count` rows before row `at`, 0-based. Everything from there down
/// moves, and every reference to it follows.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet.
pub fn insert_rows(book: &mut Spreadsheet, sheet: usize, at: Row, count: u32) -> Result<()> {
    apply(book, sheet, Shift::insert(Axis::Rows, at.index(), count))
}

/// Removes `count` rows starting at row `at`, 0-based.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet.
pub fn remove_rows(book: &mut Spreadsheet, sheet: usize, at: Row, count: u32) -> Result<()> {
    apply(book, sheet, Shift::remove(Axis::Rows, at.index(), count))
}

/// Inserts `count` columns before column `at`, 0-based.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet.
pub fn insert_columns(book: &mut Spreadsheet, sheet: usize, at: Col, count: u32) -> Result<()> {
    apply(book, sheet, Shift::insert(Axis::Columns, at.index(), count))
}

/// Removes `count` columns starting at column `at`, 0-based.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet.
pub fn remove_columns(book: &mut Spreadsheet, sheet: usize, at: Col, count: u32) -> Result<()> {
    apply(book, sheet, Shift::remove(Axis::Columns, at.index(), count))
}

/// One grid edit: which axis it moves, from where, and by how much.
#[derive(Debug, Clone, Copy)]
struct Shift {
    axis: Axis,
    /// First index the edit touches, 0-based.
    at: u32,
    /// How far indexes from `at` on move; negative for a removal.
    delta: i64,
}

impl Shift {
    const fn insert(axis: Axis, at: u32, count: u32) -> Self {
        Self {
            axis,
            at,
            delta: count as i64,
        }
    }

    const fn remove(axis: Axis, at: u32, count: u32) -> Self {
        Self {
            axis,
            at,
            delta: -(count as i64),
        }
    }

    /// The last index a removal takes with it.
    fn last_removed(self) -> u32 {
        let count = u32::try_from(self.delta.unsigned_abs()).unwrap_or(u32::MAX);
        self.at.saturating_add(count).saturating_sub(1)
    }

    /// Whether a removal takes `i` with it.
    fn removes(self, i: u32) -> bool {
        self.delta < 0 && i >= self.at && i <= self.last_removed()
    }

    /// The largest index this axis has.
    const fn ceiling(self) -> u32 {
        match self.axis {
            Axis::Rows => MAX_ROW - 1,
            Axis::Columns => MAX_COL - 1,
        }
    }

    /// Where index `i` ends up. `None` when the edit removes it, or when an
    /// insertion would push it off the sheet.
    fn moved(self, i: u32) -> Option<u32> {
        if self.removes(i) {
            return None;
        }
        if i < self.at {
            return Some(i);
        }
        let moved = i64::from(i) + self.delta;
        u32::try_from(moved).ok().filter(|&i| i <= self.ceiling())
    }

    /// Where index `i` ends up when it is the edge of a range rather than a
    /// cell: a removal that swallows it pulls it onto the edit point instead
    /// of deleting it, which is how a range shrinks around a removal.
    ///
    /// `start` says which edge it is: the leading one collapses onto the edit
    /// point, the trailing one onto the row or column just before it.
    fn moved_edge(self, i: u32, start: bool) -> Option<u32> {
        if self.removes(i) {
            return if start {
                Some(self.at)
            } else {
                self.at.checked_sub(1)
            };
        }
        self.moved(i)
    }

    /// Where a range ends up, narrowed if the edit took part of it. `None`
    /// when nothing of it is left.
    fn range(self, r: Range) -> Option<Range> {
        let (start, end) = match self.axis {
            Axis::Rows => (r.start.row.index(), r.end.row.index()),
            Axis::Columns => (r.start.col.index(), r.end.col.index()),
        };
        // A removal that covers the whole range leaves nothing to narrow.
        if self.delta < 0 && start >= self.at && end <= self.last_removed() {
            return None;
        }
        let (start, end) = (self.moved_edge(start, true)?, self.moved_edge(end, false)?);
        if start > end {
            return None;
        }
        Some(match self.axis {
            Axis::Rows => Range::new(
                CellRef::new(r.start.col, Row::new(start)?),
                CellRef::new(r.end.col, Row::new(end)?),
            ),
            Axis::Columns => Range::new(
                CellRef::new(Col::new(start)?, r.start.row),
                CellRef::new(Col::new(end)?, r.end.row),
            ),
        })
    }

    /// The index a cell reference carries on this axis.
    const fn of(self, at: CellRef) -> u32 {
        match self.axis {
            Axis::Rows => at.row.index(),
            Axis::Columns => at.col.index(),
        }
    }

    /// `at` with its index on this axis replaced.
    fn with(self, at: CellRef, i: u32) -> Option<CellRef> {
        Some(match self.axis {
            Axis::Rows => CellRef::new(at.col, Row::new(i)?),
            Axis::Columns => CellRef::new(Col::new(i)?, at.row),
        })
    }
}

/// Applies one grid edit to the whole workbook.
fn apply(book: &mut Spreadsheet, sheet: usize, shift: Shift) -> Result<()> {
    let title = book
        .sheet(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?
        .title()
        .to_owned();

    // Formulas first, while the cells still sit where the formulas expect
    // them: every sheet may point at the edited one.
    let names: Vec<String> = book.sheets().iter().map(|s| s.title().to_owned()).collect();
    for (index, own) in names.iter().enumerate() {
        let Some(target) = book.sheet_mut(index) else {
            continue;
        };
        let rewritten: Vec<(CellRef, String)> = target
            .iter()
            .filter_map(|(at, cell)| match &cell.value {
                CellValue::Formula { formula, .. } => {
                    let text = adjust(formula, shift, &title, own == &title);
                    (&text != formula).then_some((at, text))
                }
                _ => None,
            })
            .collect();
        for (at, formula) in rewritten {
            // The cached value belonged to the formula as it was written.
            target.entry(at).value = CellValue::Formula {
                formula,
                cached: None,
            };
        }
    }
    for name in &mut book.defined_names {
        let own = name.sheet.and_then(|i| names.get(i)) == Some(&title);
        name.formula = adjust(&name.formula, shift, &title, own);
    }

    // The drawings are carried, not modelled, so their anchors are rewritten
    // inside the bytes; this reads the sheet, so it runs before the borrow.
    anchor::move_anchors(book, sheet, shift);
    // A chart keeps its series as formulas, and they name their sheet.
    chart::rewrite_series(book, |series| adjust(series, shift, &title, false));
    chart::rewrite_model(book, |series| adjust(series, shift, &title, false));

    let Some(target) = book.sheet_mut(sheet) else {
        return Ok(());
    };
    move_cells(target, shift);
    move_furniture(target, shift);
    Ok(())
}

/// Moves the cells themselves, dropping what the edit removed.
fn move_cells(sheet: &mut Worksheet, shift: Shift) {
    let moved: Vec<(CellRef, Option<CellRef>)> = sheet
        .iter()
        .map(|(at, _)| {
            (
                at,
                shift.moved(shift.of(at)).and_then(|i| shift.with(at, i)),
            )
        })
        .collect();
    // Taken out first, so a cell never lands on one not yet moved.
    let mut carried = Vec::with_capacity(moved.len());
    for (from, to) in moved {
        if let Some(cell) = sheet.remove(from)
            && let Some(to) = to
        {
            carried.push((to, cell));
        }
    }
    for (at, cell) in carried {
        *sheet.entry(at) = cell;
    }
}

/// Moves everything on the sheet that names a row, a column or a range.
fn move_furniture(sheet: &mut Worksheet, shift: Shift) {
    sheet.merges = sheet
        .merges
        .iter()
        .filter_map(|&r| shift.range(r))
        .collect();
    sheet
        .hyperlinks
        .retain_mut(|link| match shift.range(link.range) {
            Some(range) => {
                link.range = range;
                true
            }
            None => false,
        });
    sheet.data_validations.retain_mut(|v| {
        v.sqref = v.sqref.iter().filter_map(|&r| shift.range(r)).collect();
        !v.sqref.is_empty()
    });
    sheet.conditional_formats.retain_mut(|f| {
        f.sqref = f.sqref.iter().filter_map(|&r| shift.range(r)).collect();
        !f.sqref.is_empty()
    });
    sheet.protected_ranges.retain_mut(|p| {
        p.sqref = p.sqref.iter().filter_map(|&r| shift.range(r)).collect();
        !p.sqref.is_empty()
    });
    if let Some(filter) = &mut sheet.auto_filter {
        match shift.range(filter.range) {
            Some(range) => filter.range = range,
            None => sheet.auto_filter = None,
        }
    }
    // A note is attached to a cell, so it goes where the cell goes; one whose
    // cell the edit removed goes with it. Where its box is drawn is the VML
    // part, which `anchor` moves in the bytes.
    sheet.comments = std::mem::take(&mut sheet.comments)
        .into_iter()
        .filter_map(|(at, note)| {
            let moved = shift.moved(shift.of(at))?;
            Some((shift.with(at, moved)?, note))
        })
        .collect();
    // A table whose range is gone entirely goes with it, the way a filter
    // does; one that lost part of itself narrows. Its own filter sits on the
    // header row and follows the same rule.
    sheet.tables.retain_mut(|t| {
        let Some(range) = shift.range(t.range) else {
            return false;
        };
        t.range = range;
        t.auto_filter = t.auto_filter.and_then(|f| shift.range(f));
        true
    });
    // A chart's frame is anchored to cells like any drawing. The bytes of the
    // drawing were moved by `anchor`, so an untouched chart stays untouched.
    for chart in &mut sheet.charts {
        let untouched = chart.is_unchanged();
        anchor::move_anchor(&mut chart.anchor, shift);
        if untouched {
            chart.settle();
        }
    }
    for chart in &mut sheet.extended_charts {
        anchor::move_anchor(&mut chart.anchor, shift);
    }

    // Row and column properties are indexed by the axis they sit on, so only
    // the matching ones move; the others name the axis that did not change.
    match shift.axis {
        Axis::Rows => {
            sheet.rows = std::mem::take(&mut sheet.rows)
                .into_iter()
                .filter_map(|(row, props)| Some((Row::new(shift.moved(row.index())?)?, props)))
                .collect();
            sheet.row_breaks.retain_mut(|b| match shift.moved(b.at) {
                Some(at) => {
                    b.at = at;
                    true
                }
                None => false,
            });
        }
        Axis::Columns => {
            sheet.columns.retain_mut(|run| {
                match (
                    shift.moved_edge(run.first.index(), true),
                    shift.moved_edge(run.last.index(), false),
                ) {
                    (Some(first), Some(last)) if first <= last => {
                        let (Some(first), Some(last)) = (Col::new(first), Col::new(last)) else {
                            return false;
                        };
                        run.first = first;
                        run.last = last;
                        true
                    }
                    _ => false,
                }
            });
            sheet.col_breaks.retain_mut(|b| match shift.moved(b.at) {
                Some(at) => {
                    b.at = at;
                    true
                }
                None => false,
            });
        }
    }
}

/// Rewrites the references of one formula for a grid edit on `target`.
///
/// `own` says whether the formula lives on the edited sheet, which is what an
/// unqualified reference points at.
fn adjust(formula: &str, shift: Shift, target: &str, own: bool) -> String {
    scan_references(formula, |qualifier, s| {
        let aims_at_target = qualifier.map_or(own, |q| q.eq_ignore_ascii_case(target));
        if !aims_at_target {
            return None;
        }
        let (len, first) = parse_ref_at(s)?;
        // `A1:B2` is one reference: an edit inside it narrows it, where the
        // same edit over a lone `A1` deletes it outright.
        if s.get(len) == Some(&':')
            && let Some((second_len, second)) = parse_ref_at(&s[len + 1..])
        {
            let range = Range::new(
                CellRef::new(first.col, first.row),
                CellRef::new(second.col, second.row),
            );
            let text = shift.range(range).map_or_else(
                || "#REF!".to_owned(),
                |moved| {
                    format!(
                        "{}:{}",
                        first.render(moved.start.col, moved.start.row),
                        second.render(moved.end.col, moved.end.row)
                    )
                },
            );
            return Some((len + 1 + second_len, text));
        }
        let at = CellRef::new(first.col, first.row);
        let text = shift
            .moved(shift.of(at))
            .and_then(|i| shift.with(at, i))
            .map_or_else(
                || "#REF!".to_owned(),
                |moved| first.render(moved.col, moved.row),
            );
        Some((len, text))
    })
}

/// Renames the sheet at `sheet`, rewriting every formula that names it.
///
/// A sheet name lives in two places: the tab, and the qualifier of every
/// reference pointing at the sheet from anywhere in the workbook. Changing
/// only the first leaves those references naming a sheet that no longer
/// exists, which is why this is not `Worksheet::set_title`.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet,
/// [`Error::DuplicateSheetName`] if another sheet already has the name, and
/// whatever [`crate::model::Worksheet::set_title`] rejects the name with.
pub fn rename_sheet(book: &mut Spreadsheet, sheet: usize, to: &str) -> Result<()> {
    let from = book
        .sheet(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?
        .title()
        .to_owned();
    if from == to {
        return Ok(());
    }
    if let Some(other) = book.sheet_by_name(to)
        && other.title() != from
    {
        return Err(Error::DuplicateSheetName(to.to_owned()));
    }
    // The title first: it validates the name, and a rejected name must leave
    // the formulas alone.
    book.sheet_mut(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?
        .set_title(to)?;
    rewrite_qualifiers(book, |names| {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&from)) {
            return None;
        }
        Some(render_qualifier(names, |n| {
            if n.eq_ignore_ascii_case(&from) {
                to.to_owned()
            } else {
                n.clone()
            }
        }))
    });
    Ok(())
}

/// Removes the sheet at `sheet`, turning every reference into it into `#REF!`.
///
/// A 3-D span keeps working where it can: `Sheet1:Sheet3!A1` with `Sheet3`
/// gone becomes `Sheet1:Sheet2!A1` when a sheet remains between the ends, and
/// only collapses to `#REF!` when nothing of the span is left.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet, and
/// [`Error::Xlsx`] when it is the only one: a workbook with no sheets cannot
/// be written, and Excel refuses the same edit.
pub fn remove_sheet(book: &mut Spreadsheet, sheet: usize) -> Result<()> {
    let order: Vec<String> = book.sheets().iter().map(|s| s.title().to_owned()).collect();
    let gone = order
        .get(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?
        .clone();
    if order.len() == 1 {
        return Err(Error::Xlsx(
            "a workbook must keep at least one sheet".into(),
        ));
    }
    let survivors: Vec<String> = order
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != sheet)
        .map(|(_, n)| n.clone())
        .collect();

    rewrite_qualifiers(book, |names| match names {
        [one] => one.eq_ignore_ascii_case(&gone).then(|| "#REF!".to_owned()),
        [first, last] => {
            let ends = [
                nearest_inside(&order, &survivors, first, last),
                nearest_inside(&order, &survivors, last, first),
            ];
            match ends {
                [Some(a), Some(b)] => {
                    (a != *first || b != *last).then(|| render_qualifier(&[a, b], Clone::clone))
                }
                // Nothing of the span is left to point at.
                _ => Some("#REF!".to_owned()),
            }
        }
        _ => None,
    });

    // A name that belonged to the sheet goes with it; the rest keep pointing
    // at their sheet, whose tab index has moved down by one.
    book.defined_names.retain(|n| n.sheet != Some(sheet));
    for name in &mut book.defined_names {
        if let Some(index) = name.sheet
            && index > sheet
        {
            name.sheet = Some(index - 1);
        }
    }
    book.take_sheet(sheet);
    Ok(())
}

/// The endpoint a 3-D span should use once a sheet is gone: itself when it
/// survived, otherwise the nearest surviving sheet inside the span, walking
/// from that end towards the other.
///
/// `None` when the whole span went, which is the only case that becomes
/// `#REF!`.
fn nearest_inside(
    order: &[String],
    survivors: &[String],
    end: &str,
    other: &str,
) -> Option<String> {
    let alive = |name: &str| survivors.iter().any(|s| s.eq_ignore_ascii_case(name));
    let position = |name: &str| order.iter().position(|s| s.eq_ignore_ascii_case(name));
    let (from, to) = (position(end)?, position(other)?);
    let mut inwards: Box<dyn Iterator<Item = &String>> = if from <= to {
        Box::new(order[from..=to].iter())
    } else {
        Box::new(order[to..=from].iter().rev())
    };
    inwards.find(|n| alive(n)).cloned()
}

/// Writes a qualifier back out, mapping each name through `f`, quoting what
/// has to be quoted and ending with the `!` the scanner expects.
fn render_qualifier(names: &[String], f: impl Fn(&String) -> String) -> String {
    let parts: Vec<String> = names.iter().map(|n| quote_sheet_name(&f(n))).collect();
    format!("{}!", parts.join(":"))
}

/// A sheet name as a formula spells it: bare when it can be, quoted when the
/// name holds anything else, with inner apostrophes doubled.
fn quote_sheet_name(name: &str) -> String {
    let bare = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.'));
    if bare {
        name.to_owned()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// Runs `rename` over every formula in the workbook: the cells of every sheet
/// and the defined names, which are formulas too.
fn rewrite_qualifiers(book: &mut Spreadsheet, rename: impl Fn(&[String]) -> Option<String>) {
    for index in 0..book.sheets().len() {
        let Some(target) = book.sheet_mut(index) else {
            continue;
        };
        let rewritten: Vec<(CellRef, String)> = target
            .iter()
            .filter_map(|(at, cell)| match &cell.value {
                CellValue::Formula { formula, .. } => {
                    let text = scan_formula(formula, |_, _| None, &rename);
                    (&text != formula).then_some((at, text))
                }
                _ => None,
            })
            .collect();
        for (at, formula) in rewritten {
            // The cached value answered the formula as it was written.
            target.entry(at).value = CellValue::Formula {
                formula,
                cached: None,
            };
        }
    }
    for name in &mut book.defined_names {
        name.formula = scan_formula(&name.formula, |_, _| None, &rename);
    }
    // A chart series is a formula too, and it always names its sheet.
    chart::rewrite_series(book, |series| scan_formula(series, |_, _| None, &rename));
    chart::rewrite_model(book, |series| scan_formula(series, |_, _| None, &rename));
}
