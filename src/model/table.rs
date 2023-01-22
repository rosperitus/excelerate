//! Tables: a named range with a header row that Excel treats as one object.
//!
//! Until now the part travelled whole and the sheet kept only the
//! `<tableParts>` element pointing at it. That was enough to reopen a file
//! unchanged and not enough for anything else: a row inserted inside a table
//! moved the cells and left the table's own `ref` where it was, so the range
//! Excel highlighted no longer covered the data.

use crate::coordinate::{CellRef, Range, Row};

/// A table on a sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The id the part carries, unique within the workbook.
    pub id: u32,
    /// The name Excel shows in the name box.
    pub name: String,
    /// The name formulas use, which Excel keeps free of spaces.
    pub display_name: String,
    /// Everything the table covers, header and totals rows included.
    pub range: Range,
    /// How many rows of the range are the header, when the file says.
    ///
    /// Absent means one, and one is by far the common case; writing the
    /// attribute out anyway would put a decision in the file that nobody made.
    pub header_row_count: Option<u32>,
    /// How many rows are the totals, when the file says. Absent means none.
    pub totals_row_count: Option<u32>,
    /// The filter dropdowns, which sit on the header row and follow it.
    pub auto_filter: Option<Range>,
    /// The columns, in the order the header lists them.
    pub columns: Vec<TableColumn>,
    /// Banding and which edges are emphasised.
    pub style: Option<TableStyle>,
}

/// One column of a table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableColumn {
    /// The id the part carries, unique within the table.
    pub id: u32,
    /// The header text, which is also how a formula names the column.
    pub name: String,
    /// The aggregate the totals row shows, as the format spells it (`sum`,
    /// `count`, `custom` and the rest).
    pub totals_row_function: Option<String>,
    /// The text the totals row shows instead of an aggregate.
    pub totals_row_label: Option<String>,
    /// The formula every cell of the column repeats.
    pub calculated_formula: Option<String>,
}

/// How a table is banded and which of its edges are emphasised.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per attribute of <tableStyleInfo>; a bitfield would hide the format"
)]
pub struct TableStyle {
    /// The name of a built-in or custom table style.
    pub name: Option<String>,
    /// Whether the first column is emphasised.
    pub show_first_column: bool,
    /// Whether the last column is.
    pub show_last_column: bool,
    /// Whether rows alternate shading.
    pub show_row_stripes: bool,
    /// Whether columns do.
    pub show_column_stripes: bool,
}

impl Table {
    /// The rows holding data, which is the range without its header and
    /// totals rows.
    ///
    /// Returns nothing when the two eat the range whole, which a file is free
    /// to say and Excel draws as a table with no body.
    #[must_use]
    pub fn body(&self) -> Option<Range> {
        let top = Row::new(
            self.range
                .start
                .row
                .index()
                .checked_add(self.header_row_count.unwrap_or(1))?,
        )?;
        let bottom = Row::new(
            self.range
                .end
                .row
                .index()
                .checked_sub(self.totals_row_count.unwrap_or(0))?,
        )?;
        if top > bottom {
            return None;
        }
        Some(Range {
            start: CellRef::new(self.range.start.col, top),
            end: CellRef::new(self.range.end.col, bottom),
        })
    }
}
