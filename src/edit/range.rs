//! Copying and moving a rectangle of cells, and moving a sheet in the tab bar.
//!
//! The difference between a copy and a move is what happens to the formulas,
//! and it is the difference Excel draws between Ctrl+C and Ctrl+X: a copied
//! formula is rewritten as if it had been written where it lands, so `=A1` one
//! row down reads `=A2`; a moved formula keeps pointing at the very cells it
//! pointed at, and only references *into the block being moved* travel with
//! it. References from elsewhere in the workbook follow a move, because the
//! cells they name went somewhere, and ignore a copy, because the originals
//! stayed put.

use super::Axis;
use super::quote_sheet_name;
use crate::coordinate::{
    CellRef, Col, MAX_COL, MAX_ROW, Range, Row, parse_ref_at, scan_formula, shift_references,
};
use crate::error::{Error, Result};
use crate::model::{Cell, CellValue, Spreadsheet};

/// Copies a rectangle of cells, formatting and all, so that its top left
/// corner lands on `to`.
///
/// Formulas are rewritten the way Excel rewrites a copied formula: a relative
/// reference moves with the cell, `$A$1` stays where it is, and one pushed off
/// the sheet becomes `#REF!`. A cached result is dropped - it answered the
/// formula as it was written. Cells the source leaves empty empty the target,
/// which is what pasting a block does.
///
/// The source and the target may overlap, and may be on the same sheet or on
/// two different ones.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if either sheet is missing, and
/// [`Error::Xlsx`] if the block would land off the sheet.
pub fn copy_range(
    book: &mut Spreadsheet,
    from_sheet: usize,
    from: Range,
    to_sheet: usize,
    to: CellRef,
) -> Result<()> {
    let (d_col, d_row) = landing(book, from_sheet, from, to_sheet, to)?;
    let taken = read_block(book, from_sheet, from, |formula| {
        shift_references(formula, d_col, d_row)
    });
    write_block(book, to_sheet, to, from, &taken)?;
    let merges = moved_merges(book, from_sheet, from, d_col, d_row);
    place_merges(book, to_sheet, target_of(from, d_col, d_row), merges);
    Ok(())
}

/// Moves a rectangle of cells so that its top left corner lands on `to`,
/// leaving the source empty.
///
/// A moved formula keeps its answers: `=Z9` still reads `Z9` afterwards, and
/// only a reference into the block being moved travels with it. References
/// from anywhere else in the workbook follow the cells, as they do when Excel
/// cuts and pastes - the cells they name are simply somewhere else now.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if either sheet is missing, and
/// [`Error::Xlsx`] if the block would land off the sheet.
pub fn move_range(
    book: &mut Spreadsheet,
    from_sheet: usize,
    from: Range,
    to_sheet: usize,
    to: CellRef,
) -> Result<()> {
    let (d_col, d_row) = landing(book, from_sheet, from, to_sheet, to)?;
    if d_col == 0 && d_row == 0 && from_sheet == to_sheet {
        return Ok(());
    }
    // The name the moved formulas need for what they used to read without
    // naming it: on a move to another sheet, `=Z9` means Z9 of the sheet it
    // came from.
    let from_name = book
        .sheet(from_sheet)
        .map_or_else(String::new, |ws| ws.title().to_owned());
    let source_name = (from_sheet != to_sheet).then(|| quote_sheet_name(&from_name));
    let taken = read_block(book, from_sheet, from, |formula| {
        retarget(
            formula,
            None,
            &from_name,
            from,
            d_col,
            d_row,
            source_name.as_deref(),
        )
    });
    // The source goes before the target is written: the two may overlap.
    for at in from.cells() {
        if let Some(ws) = book.sheet_mut(from_sheet) {
            ws.remove(at);
        }
    }
    let merges = moved_merges(book, from_sheet, from, d_col, d_row);
    if let Some(ws) = book.sheet_mut(from_sheet) {
        ws.merges.retain(|m| !from.contains_range(m));
    }
    write_block(book, to_sheet, to, from, &taken)?;
    place_merges(book, to_sheet, target_of(from, d_col, d_row), merges);
    follow_the_cells(book, from_sheet, to_sheet, from, d_col, d_row);
    Ok(())
}

/// Inserts blank cells over `area`, pushing what was there down or right.
///
/// This is Excel's "Insert Cells", the edit that moves part of a row rather
/// than the whole of it: `Axis::Rows` pushes the cells below `area` down by
/// its height, `Axis::Columns` pushes the cells to its right along by its
/// width. Only the columns (or rows) the area spans move; the rest of the
/// sheet stays where it is, which is exactly why the reference rules differ
/// from [`insert_rows`](super::insert_rows) - a reference is carried only when
/// the whole of it travels.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet, and
/// [`Error::Xlsx`] if the cells pushed along would leave the sheet.
pub fn insert_cells(book: &mut Spreadsheet, sheet: usize, area: Range, axis: Axis) -> Result<()> {
    insert_cells_with(book, sheet, area, axis, super::CopyOrigin::Blank)
}

/// [`insert_cells`], with the new cells styled like the line beside them:
/// above or to the left for `Before`, below or to the right for `After`.
///
/// # Errors
/// As [`insert_cells`].
pub fn insert_cells_with(
    book: &mut Spreadsheet,
    sheet: usize,
    area: Range,
    axis: Axis,
    origin: super::CopyOrigin,
) -> Result<()> {
    let (d_col, d_row) = step(area, axis);
    let pushed = below(area, axis);
    slide(book, sheet, pushed, d_col, d_row)?;
    clear(book, sheet, area);
    let Some(ws) = book.sheet_mut(sheet) else {
        return Ok(());
    };
    for at in area.cells() {
        let source = match (origin, axis) {
            (super::CopyOrigin::Blank, _) => None,
            (super::CopyOrigin::Before, Axis::Rows) => area
                .start
                .row
                .index()
                .checked_sub(1)
                .and_then(Row::new)
                .map(|row| CellRef::new(at.col, row)),
            (super::CopyOrigin::Before, Axis::Columns) => area
                .start
                .col
                .index()
                .checked_sub(1)
                .and_then(Col::new)
                .map(|col| CellRef::new(col, at.row)),
            (super::CopyOrigin::After, Axis::Rows) => area
                .end
                .row
                .index()
                .checked_add(1)
                .and_then(Row::new)
                .map(|row| CellRef::new(at.col, row)),
            (super::CopyOrigin::After, Axis::Columns) => area
                .end
                .col
                .index()
                .checked_add(1)
                .and_then(Col::new)
                .map(|col| CellRef::new(col, at.row)),
        };
        if let Some(style) = source.and_then(|from| ws.get(from)).map(|cell| cell.style)
            && style != crate::style::StyleId::default()
        {
            ws.entry(at).style = style;
        }
    }
    Ok(())
}

/// Removes the cells of `area`, pulling what was below or to the right of them
/// back over the hole.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet.
pub fn remove_cells(book: &mut Spreadsheet, sheet: usize, area: Range, axis: Axis) -> Result<()> {
    let (d_col, d_row) = step(area, axis);
    let pulled = beyond(area, axis)?;
    clear(book, sheet, area);
    slide(book, sheet, pulled, -d_col, -d_row)?;
    Ok(())
}

/// How far an insert or a remove over `area` moves the cells beside it.
const fn step(area: Range, axis: Axis) -> (i64, i64) {
    match axis {
        Axis::Rows => (0, area.height() as i64),
        Axis::Columns => (area.width() as i64, 0),
    }
}

/// The block an insert pushes along: everything from `area` to the end of the
/// sheet, in the columns or rows it spans.
fn below(area: Range, axis: Axis) -> Range {
    let end = match axis {
        Axis::Rows => CellRef::new(area.end.col, last_row()),
        Axis::Columns => CellRef::new(last_col(), area.end.row),
    };
    Range::new(area.start, end)
}

/// The same block for a remove, starting past the hole.
fn beyond(area: Range, axis: Axis) -> Result<Range> {
    let start = match axis {
        Axis::Rows => CellRef::new(
            area.start.col,
            Row::new(area.end.row.index() + 1).ok_or_else(off_the_sheet)?,
        ),
        Axis::Columns => CellRef::new(
            Col::new(area.end.col.index() + 1).ok_or_else(off_the_sheet)?,
            area.start.row,
        ),
    };
    Ok(below(Range::new(start, area.end), axis))
}

/// The last row and column of a sheet.
fn last_row() -> Row {
    Row::new(MAX_ROW - 1).unwrap_or_default()
}

fn last_col() -> Col {
    Col::new(MAX_COL - 1).unwrap_or_default()
}

fn off_the_sheet() -> Error {
    Error::Xlsx("the cells would be pushed off the sheet".into())
}

/// Moves a block by a delta, rewriting the references that travel whole with
/// it. The cells it leaves behind are emptied by the caller.
fn slide(book: &mut Spreadsheet, sheet: usize, block: Range, d_col: i64, d_row: i64) -> Result<()> {
    // A push only fails when it would shove something off the sheet: the cells
    // at the far end of the block have nowhere to go. Blank ones there are no
    // loss, which is how Excel decides it too.
    if d_col > 0 || d_row > 0 {
        let edge = Range::new(
            CellRef::new(
                Col::new(shifted(block.end.col.index(), 1 - d_col)).unwrap_or(block.start.col),
                Row::new(shifted(block.end.row.index(), 1 - d_row)).unwrap_or(block.start.row),
            ),
            block.end,
        );
        let occupied = book
            .sheet(sheet)
            .is_some_and(|ws| ws.iter().any(|(at, _)| edge.contains(at)));
        if occupied {
            return Err(off_the_sheet());
        }
    }
    let name = book
        .sheet(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?
        .title()
        .to_owned();
    let cells: Vec<(CellRef, Cell)> = book
        .sheet(sheet)
        .map(|ws| {
            ws.iter()
                .filter(|(at, _)| block.contains(*at))
                .map(|(at, cell)| (at, cell.clone()))
                .collect()
        })
        .unwrap_or_default();
    let ws = book
        .sheet_mut(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    for (at, _) in &cells {
        ws.remove(*at);
    }
    for (at, mut cell) in cells {
        let (Some(col), Some(row)) = (
            Col::new(shifted(at.col.index(), d_col)),
            Row::new(shifted(at.row.index(), d_row)),
        ) else {
            continue;
        };
        if let CellValue::Formula { formula, .. } = &cell.value {
            cell.value = CellValue::Formula {
                formula: retarget(formula, None, &name, block, d_col, d_row, None),
                cached: None,
            };
        }
        *ws.entry(CellRef::new(col, row)) = cell;
    }
    follow_the_cells(book, sheet, sheet, block, d_col, d_row);
    Ok(())
}

/// Empties every cell of an area.
fn clear(book: &mut Spreadsheet, sheet: usize, area: Range) {
    if let Some(ws) = book.sheet_mut(sheet) {
        for at in area.cells() {
            ws.remove(at);
        }
    }
}

/// Moves the sheet at `from` to position `to` in the tab bar.
///
/// Nothing in a formula changes: a sheet is named, not numbered. What does
/// change is every index kept beside the formulas - the active tab and the
/// sheet a defined name belongs to - and those are renumbered here.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if either position is outside the book.
pub fn move_sheet(book: &mut Spreadsheet, from: usize, to: usize) -> Result<()> {
    let count = book.sheets().len();
    if from >= count {
        return Err(Error::SheetIndexOutOfRange(from));
    }
    if to >= count {
        return Err(Error::SheetIndexOutOfRange(to));
    }
    if from == to {
        return Ok(());
    }
    book.reorder_sheet(from, to);
    for name in &mut book.defined_names {
        name.sheet = name.sheet.map(|index| renumber(index, from, to));
    }
    let active = renumber(book.active_index(), from, to);
    book.set_active(active)
}

/// Where an index lands once the sheet at `from` has moved to `to`.
const fn renumber(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        return to;
    }
    if from < to && index > from && index <= to {
        index - 1
    } else if to <= index && index < from {
        index + 1
    } else {
        index
    }
}

/// How far the block travels, checking that both sheets exist and that it
/// lands on the sheet.
fn landing(
    book: &Spreadsheet,
    from_sheet: usize,
    from: Range,
    to_sheet: usize,
    to: CellRef,
) -> Result<(i64, i64)> {
    for sheet in [from_sheet, to_sheet] {
        if book.sheet(sheet).is_none() {
            return Err(Error::SheetIndexOutOfRange(sheet));
        }
    }
    let d_col = i64::from(to.col.index()) - i64::from(from.start.col.index());
    let d_row = i64::from(to.row.index()) - i64::from(from.start.row.index());
    let last_col = i64::from(from.end.col.index()) + d_col;
    let last_row = i64::from(from.end.row.index()) + d_row;
    // MAX_COL and MAX_ROW count from one; these are indexes.
    if last_col >= i64::from(MAX_COL) || last_row >= i64::from(MAX_ROW) {
        return Err(Error::Xlsx("the block would land off the sheet".into()));
    }
    Ok((d_col, d_row))
}

/// The rectangle the block lands on.
fn target_of(from: Range, d_col: i64, d_row: i64) -> Range {
    let shift = |at: CellRef| {
        let col = Col::new(shifted(at.col.index(), d_col)).unwrap_or(at.col);
        let row = Row::new(shifted(at.row.index(), d_row)).unwrap_or(at.row);
        CellRef::new(col, row)
    };
    Range::new(shift(from.start), shift(from.end))
}

/// An index moved by a delta, clamped at zero.
fn shifted(index: u32, delta: i64) -> u32 {
    u32::try_from(i64::from(index) + delta).unwrap_or(0)
}

/// The cells of a block, keyed by their offset inside it, with each formula
/// put through `rewrite`.
fn read_block(
    book: &Spreadsheet,
    sheet: usize,
    area: Range,
    rewrite: impl Fn(&str) -> String,
) -> Vec<((u32, u32), Cell)> {
    let Some(ws) = book.sheet(sheet) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for at in area.cells() {
        let Some(cell) = ws.get(at) else { continue };
        let mut cell = cell.clone();
        if let CellValue::Formula { formula, .. } = &cell.value {
            cell.value = CellValue::Formula {
                formula: rewrite(formula),
                // The cached value answered the formula as it was written.
                cached: None,
            };
        }
        out.push((
            (
                at.col.index() - area.start.col.index(),
                at.row.index() - area.start.row.index(),
            ),
            cell,
        ));
    }
    out
}

/// Writes a block read by [`read_block`], emptying the cells the source had
/// nothing in.
fn write_block(
    book: &mut Spreadsheet,
    sheet: usize,
    to: CellRef,
    from: Range,
    cells: &[((u32, u32), Cell)],
) -> Result<()> {
    let ws = book
        .sheet_mut(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    let width = from.end.col.index() - from.start.col.index();
    let height = from.end.row.index() - from.start.row.index();
    for d_row in 0..=height {
        for d_col in 0..=width {
            let (Some(col), Some(row)) = (
                Col::new(to.col.index() + d_col),
                Row::new(to.row.index() + d_row),
            ) else {
                continue;
            };
            let at = CellRef::new(col, row);
            match cells.iter().find(|((c, r), _)| *c == d_col && *r == d_row) {
                Some((_, cell)) => *ws.entry(at) = cell.clone(),
                None => {
                    ws.remove(at);
                }
            }
        }
    }
    Ok(())
}

/// The merges lying wholly inside the block, moved along with it.
fn moved_merges(
    book: &Spreadsheet,
    sheet: usize,
    area: Range,
    d_col: i64,
    d_row: i64,
) -> Vec<Range> {
    book.sheet(sheet).map_or_else(Vec::new, |ws| {
        ws.merges
            .iter()
            .filter(|m| area.contains_range(m))
            .map(|m| target_of(*m, d_col, d_row))
            .collect()
    })
}

/// Puts merges on the target sheet, clearing whatever crossed the area they
/// land on: two overlapping merges are a file Excel repairs.
fn place_merges(book: &mut Spreadsheet, sheet: usize, area: Range, merges: Vec<Range>) {
    let Some(ws) = book.sheet_mut(sheet) else {
        return;
    };
    ws.merges.retain(|m| !m.intersects(&area));
    ws.merges.extend(merges);
}

/// Rewrites the references of one formula for a move: what points into the
/// block travels with it, and what does not stays where it was.
///
/// `on` is the sheet the formula itself sits on, `None` when the formula is
/// one of the cells being moved - either way an unqualified reference means
/// that sheet. `qualify` names the sheet an unqualified reference used to
/// mean, for a move that crosses sheets.
fn retarget(
    formula: &str,
    on: Option<&str>,
    source_sheet: &str,
    source: Range,
    d_col: i64,
    d_row: i64,
    qualify: Option<&str>,
) -> String {
    let names_source = |qualifier: Option<&str>| match qualifier {
        Some(name) => name.eq_ignore_ascii_case(source_sheet),
        None => on.is_none_or(|sheet| sheet.eq_ignore_ascii_case(source_sheet)),
    };
    scan_formula(
        formula,
        |qualifier, s| {
            let (len, r) = parse_ref_at(s)?;
            if names_source(qualifier) && source.contains(CellRef::new(r.col, r.row)) {
                let col = Col::new(shifted(r.col.index(), d_col))?;
                let row = Row::new(shifted(r.row.index(), d_row))?;
                return Some((len, r.render(col, row)));
            }
            // It stayed where it was - but a move to another sheet takes the
            // old sheet's name along for what named no sheet at all.
            match (qualifier, qualify) {
                (None, Some(name)) => Some((len, format!("{name}!{}", r.render(r.col, r.row)))),
                _ => None,
            }
        },
        |_| None,
    )
}

/// Points every formula in the workbook at where the moved cells went.
fn follow_the_cells(
    book: &mut Spreadsheet,
    from_sheet: usize,
    to_sheet: usize,
    source: Range,
    d_col: i64,
    d_row: i64,
) {
    // ponytail: a move to another sheet leaves the references from elsewhere
    // alone. Following them would mean rewriting the sheet qualifier in front
    // of each, which the scanner hands out before it knows which reference
    // follows; lift it when someone needs it.
    if from_sheet != to_sheet {
        return;
    }
    let Some(source_name) = book.sheet(from_sheet).map(|ws| ws.title().to_owned()) else {
        return;
    };
    let target = target_of(source, d_col, d_row);
    for index in 0..book.sheets().len() {
        let Some(ws) = book.sheet_mut(index) else {
            continue;
        };
        let on = ws.title().to_owned();
        let own = index == from_sheet;
        let rewritten: Vec<(CellRef, String)> = ws
            .iter()
            // The cells just written are the moved formulas themselves, which
            // were rewritten as they travelled.
            .filter(|(at, _)| !(own && target.contains(*at)))
            .filter_map(|(at, cell)| match &cell.value {
                CellValue::Formula { formula, .. } => {
                    let text =
                        retarget(formula, Some(&on), &source_name, source, d_col, d_row, None);
                    (&text != formula).then_some((at, text))
                }
                _ => None,
            })
            .collect();
        for (at, formula) in rewritten {
            ws.entry(at).value = CellValue::Formula {
                formula,
                cached: None,
            };
        }
    }
}

/// What a [`SortKey`] compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortBy {
    /// A column of the sheet when sorting rows, a row when sorting columns;
    /// 0-based, and counted on the sheet, not inside the range.
    Line(u32),
    /// The line whose header - the first cell of it in the range, or the
    /// column name of a table - says this, without regard to case.
    Header(String),
}

/// One key of [`sort_range`]: what to compare and in which direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    /// The line compared.
    pub by: SortBy,
    /// Largest first.
    pub descending: bool,
}

impl SortKey {
    /// A key on a column of the sheet, smallest first.
    #[must_use]
    pub const fn column(column: Col) -> Self {
        Self {
            by: SortBy::Line(column.index()),
            descending: false,
        }
    }

    /// A key on a row of the sheet, for sorting columns left to right.
    #[must_use]
    pub const fn row(row: Row) -> Self {
        Self {
            by: SortBy::Line(row.index()),
            descending: false,
        }
    }

    /// A key on the line headed `name`: needs [`SortOptions::header`], or a
    /// table.
    #[must_use]
    pub fn header(name: impl Into<String>) -> Self {
        Self {
            by: SortBy::Header(name.into()),
            descending: false,
        }
    }

    /// The same key, largest first.
    #[must_use]
    pub fn descending(mut self) -> Self {
        self.descending = true;
        self
    }
}

/// How [`sort_range_with`] reads its range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortOptions {
    /// The first line of the range is a header: it stays where it is and
    /// names the lines for [`SortBy::Header`].
    pub header: bool,
    /// `Axis::Rows` reorders rows (the usual sort), `Axis::Columns` reorders
    /// columns left to right, keyed by rows.
    pub orientation: Axis,
}

impl Default for SortOptions {
    fn default() -> Self {
        Self {
            header: false,
            orientation: Axis::Rows,
        }
    }
}

/// Sorts the rows of `area` by `keys`, the first key deciding first: Excel's
/// Data - Sort on a range with no header row. [`sort_range_with`] takes a
/// header row and sorts columns.
///
/// # Errors
/// As [`sort_range_with`].
pub fn sort_range(
    book: &mut Spreadsheet,
    sheet: usize,
    area: Range,
    keys: &[SortKey],
) -> Result<()> {
    sort_range_with(book, sheet, area, keys, SortOptions::default())
}

/// Sorts the rows (or columns) of `area` by `keys`, the first key deciding
/// first.
///
/// Cells travel with their formatting. The order is Excel's: numbers, then
/// text without regard to case, then `FALSE`, `TRUE` and errors; an empty
/// cell goes last whichever way the key runs. Lines that compare equal keep
/// their order. A formula is rewritten as if copied to its new place, so a
/// relative reference to its own row keeps pointing at its own row, and its
/// cached result is dropped; formulas elsewhere are not retargeted, as in
/// Excel. A formula is sorted by its cached value.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet, and
/// [`Error::InvalidRange`] if a key names a line outside `area` or a header
/// nothing is called, or a merge crosses `area`.
pub fn sort_range_with(
    book: &mut Spreadsheet,
    sheet: usize,
    area: Range,
    keys: &[SortKey],
    options: SortOptions,
) -> Result<()> {
    let ws = book
        .sheet(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    let rows = options.orientation == Axis::Rows;
    // The cross axis is where the keys live: columns when sorting rows.
    let (first, last) = if rows {
        (area.start.col.index(), area.end.col.index())
    } else {
        (area.start.row.index(), area.end.row.index())
    };
    let header: Vec<(u32, String)> = if options.header {
        (first..=last)
            .filter_map(|line| {
                let at = if rows {
                    CellRef::new(Col::new(line)?, area.start.row)
                } else {
                    CellRef::new(area.start.col, Row::new(line)?)
                };
                Some((line, ws.get(at)?.value.plain_text()?))
            })
            .collect()
    } else {
        Vec::new()
    };
    let lines = resolve_keys(keys, &header, first, last)?;
    let data = if !options.header {
        area
    } else if rows {
        match Row::new(area.start.row.index() + 1).filter(|r| *r <= area.end.row) {
            Some(start) => Range::new(CellRef::new(area.start.col, start), area.end),
            None => return Ok(()),
        }
    } else {
        match Col::new(area.start.col.index() + 1).filter(|c| *c <= area.end.col) {
            Some(start) => Range::new(CellRef::new(start, area.start.row), area.end),
            None => return Ok(()),
        }
    };
    sort_lines(book, sheet, data, &lines, rows)
}

/// Sorts the data rows of the table called `name` - its header and totals
/// rows stay put - with keys naming its columns by [`SortBy::Header`] or by
/// sheet column.
///
/// # Errors
/// [`Error::InvalidRange`] if there is no such table or a key names no
/// column of it, and the errors of [`sort_range_with`].
pub fn sort_table(book: &mut Spreadsheet, name: &str, keys: &[SortKey]) -> Result<()> {
    let found = book.sheets().iter().enumerate().find_map(|(index, ws)| {
        ws.tables
            .iter()
            .find(|t| {
                t.display_name.eq_ignore_ascii_case(name) || t.name.eq_ignore_ascii_case(name)
            })
            .map(|t| (index, t.clone()))
    });
    let Some((sheet, table)) = found else {
        return Err(Error::InvalidRange(format!("no table called {name}")));
    };
    let header: Vec<(u32, String)> = table
        .columns
        .iter()
        .zip(table.range.start.col.index()..)
        .map(|(column, line)| (line, column.name.clone()))
        .collect();
    let lines = resolve_keys(
        keys,
        &header,
        table.range.start.col.index(),
        table.range.end.col.index(),
    )?;
    let Some(data) = table.body() else {
        return Ok(());
    };
    sort_lines(book, sheet, data, &lines, true)
}

/// Turns keys into sheet lines, checking each falls inside `first..=last`.
fn resolve_keys(
    keys: &[SortKey],
    header: &[(u32, String)],
    first: u32,
    last: u32,
) -> Result<Vec<(u32, bool)>> {
    keys.iter()
        .map(|key| {
            let line = match &key.by {
                SortBy::Line(line) => Some(*line).filter(|l| (first..=last).contains(l)),
                SortBy::Header(name) => header
                    .iter()
                    .find(|(_, text)| text.trim().to_lowercase() == name.trim().to_lowercase())
                    .map(|(line, _)| *line),
            };
            line.map(|line| (line, key.descending)).ok_or_else(|| {
                Error::InvalidRange(match &key.by {
                    SortBy::Line(line) => format!("sort key {} is outside the range", line + 1),
                    SortBy::Header(name) => format!("no header called {name:?}"),
                })
            })
        })
        .collect()
}

/// The sort itself, over lines already resolved: `(line, descending)`.
fn sort_lines(
    book: &mut Spreadsheet,
    sheet: usize,
    area: Range,
    keys: &[(u32, bool)],
    rows: bool,
) -> Result<()> {
    let ws = book
        .sheet_mut(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    if ws.merges.iter().any(|m| m.intersects(&area)) {
        return Err(Error::InvalidRange(format!(
            "a merged range crosses {area}; unmerge it to sort"
        )));
    }
    // A line is a row when sorting rows; `at(line, cross)` names a cell by the
    // index along the sort and the index across it.
    let (lines, cross) = if rows {
        (
            area.start.row.index()..=area.end.row.index(),
            area.start.col.index()..=area.end.col.index(),
        )
    } else {
        (
            area.start.col.index()..=area.end.col.index(),
            area.start.row.index()..=area.end.row.index(),
        )
    };
    let at = |line: u32, cross: u32| -> Option<CellRef> {
        if rows {
            Some(CellRef::new(Col::new(cross)?, Row::new(line)?))
        } else {
            Some(CellRef::new(Col::new(line)?, Row::new(cross)?))
        }
    };
    let lines: Vec<u32> = lines.collect();
    let key_of = |line: u32, key: u32| -> Option<CellValue> {
        let value = &ws.get(at(line, key)?)?.value;
        match value {
            CellValue::Formula { cached, .. } => cached.as_deref().cloned(),
            other => Some(other.clone()),
        }
        .filter(|v| !v.is_empty())
    };
    let mut order: Vec<(usize, Vec<Option<CellValue>>)> = lines
        .iter()
        .enumerate()
        .map(|(i, &line)| (i, keys.iter().map(|&(key, _)| key_of(line, key)).collect()))
        .collect();
    order.sort_by(|(_, a), (_, b)| {
        keys.iter()
            .zip(a.iter().zip(b))
            .map(|(&(_, descending), (a, b))| match (a, b) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(a), Some(b)) if descending => sort_order(b, a),
                (Some(a), Some(b)) => sort_order(a, b),
            })
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Every line is taken out first, so a line never lands on one not yet read.
    let taken: Vec<Vec<(u32, Cell)>> = lines
        .iter()
        .map(|&line| {
            cross
                .clone()
                .filter_map(|c| Some((c, ws.remove(at(line, c)?)?)))
                .collect()
        })
        .collect();
    for (to, (from, _)) in order.iter().enumerate() {
        let delta = i64::try_from(to).unwrap_or(0) - i64::try_from(*from).unwrap_or(0);
        for (c, cell) in &taken[*from] {
            let Some(target) = at(lines[to], *c) else {
                continue;
            };
            let mut cell = cell.clone();
            if delta != 0
                && let CellValue::Formula { formula, .. } = &cell.value
            {
                let (d_col, d_row) = if rows { (0, delta) } else { (delta, 0) };
                cell.value = CellValue::Formula {
                    formula: shift_references(formula, d_col, d_row),
                    cached: None,
                };
            }
            *ws.entry(target) = cell;
        }
    }
    Ok(())
}

/// Excel's sort order between two non-empty values.
fn sort_order(a: &CellValue, b: &CellValue) -> std::cmp::Ordering {
    let rank = |v: &CellValue| match v {
        CellValue::Number(_) => 0,
        CellValue::Bool(_) => 2,
        CellValue::Error(_) => 3,
        _ => 1,
    };
    match (a, b) {
        (CellValue::Number(x), CellValue::Number(y)) => x.total_cmp(y),
        (CellValue::Bool(x), CellValue::Bool(y)) => x.cmp(y),
        _ if rank(a) != rank(b) => rank(a).cmp(&rank(b)),
        (CellValue::Error(_), _) => std::cmp::Ordering::Equal,
        _ => {
            let text = |v: &CellValue| v.plain_text().unwrap_or_default().to_lowercase();
            text(a).cmp(&text(b))
        }
    }
}

/// Copies the first row of `area` into every row below it (`Axis::Rows`,
/// Excel's Ctrl+D), or its first column into every column to the right
/// (`Axis::Columns`, Ctrl+R). Values, formatting and formulas go as
/// [`copy_range`] takes them: a relative reference moves with each copy.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet.
pub fn fill(book: &mut Spreadsheet, sheet: usize, area: Range, axis: Axis) -> Result<()> {
    let (source, rest) = match axis {
        Axis::Rows => (
            Range::new(area.start, CellRef::new(area.end.col, area.start.row)),
            area.start.row.index() + 1..=area.end.row.index(),
        ),
        Axis::Columns => (
            Range::new(area.start, CellRef::new(area.start.col, area.end.row)),
            area.start.col.index() + 1..=area.end.col.index(),
        ),
    };
    for index in rest {
        let to = match axis {
            Axis::Rows => Row::new(index).map(|row| CellRef::new(area.start.col, row)),
            Axis::Columns => Col::new(index).map(|col| CellRef::new(col, area.start.row)),
        };
        if let Some(to) = to {
            copy_range(book, sheet, source, sheet, to)?;
        }
    }
    Ok(())
}
