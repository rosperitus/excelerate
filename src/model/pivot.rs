//! Pivot tables: what a sheet's pivot says about itself, and where its data
//! came from.
//!
//!
//! **Read only.** The parts still travel byte for byte, and nothing here is
//! written back: a pivot is bound to its cache, its records and the versions
//! that wrote them, and rebuilding one from a summary would be a worse file
//! than the one that arrived. What this buys is the ability to *see* the
//! report — its source, its fields and how they are laid out — without
//! parsing XML by hand.

use crate::coordinate::Range;

/// Where a field sits in the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PivotAxis {
    /// Down the left-hand side.
    Row,
    /// Across the top.
    Column,
    /// A filter above the report.
    Page,
    /// The values area, which holds the numbers rather than the labels.
    Values,
    /// In the field list but not in the report.
    #[default]
    Unused,
}

impl PivotAxis {
    /// Reads the `axis` attribute.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "axisRow" => Self::Row,
            "axisCol" => Self::Column,
            "axisPage" => Self::Page,
            "axisValues" => Self::Values,
            _ => Self::Unused,
        }
    }
}

/// How a data field summarises what it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Subtotal {
    /// Added up.
    #[default]
    Sum,
    /// Counted, blanks and text included.
    Count,
    /// Averaged.
    Average,
    /// The largest.
    Max,
    /// The smallest.
    Min,
    /// Multiplied together.
    Product,
    /// Counted, numbers only.
    CountNums,
    /// Sample standard deviation, and the population form after it.
    StdDev,
    /// Population standard deviation.
    StdDevP,
    /// Sample variance.
    Var,
    /// Population variance.
    VarP,
}

impl Subtotal {
    /// Reads the `subtotal` attribute of a data field.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "count" => Self::Count,
            "average" => Self::Average,
            "max" => Self::Max,
            "min" => Self::Min,
            "product" => Self::Product,
            "countNums" => Self::CountNums,
            "stdDev" => Self::StdDev,
            "stdDevp" => Self::StdDevP,
            "var" => Self::Var,
            "varp" => Self::VarP,
            _ => Self::Sum,
        }
    }

    /// The attribute value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sum => "sum",
            Self::Count => "count",
            Self::Average => "average",
            Self::Max => "max",
            Self::Min => "min",
            Self::Product => "product",
            Self::CountNums => "countNums",
            Self::StdDev => "stdDev",
            Self::StdDevP => "stdDevp",
            Self::Var => "var",
            Self::VarP => "varp",
        }
    }
}

/// One field of the report, in the order the cache lists them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotField {
    /// Where it sits, if it is in the report at all.
    pub axis: PivotAxis,
    /// Whether this is the field the values come from.
    pub data_field: bool,
    /// Whether Excel adds a subtotal row or column for it.
    pub default_subtotal: bool,
    /// Whether items with no data are shown.
    pub show_all: bool,
}

/// A field in the values area: which cache field it reads and how it
/// summarises it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DataField {
    /// The caption shown in the report; Excel writes "Sum of Sales" here when
    /// the user has not renamed it.
    pub name: Option<String>,
    /// Index into the cache's fields.
    pub field: u32,
    /// How the numbers are summarised.
    pub subtotal: Subtotal,
    /// Number format applied to the result, as an index into the stylesheet.
    pub number_format: Option<u32>,
}

/// The table style the report is painted with.
///
/// The four flags say which parts of the style are switched on; the format
/// spells them as four attributes, so they are four fields here.
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per attribute of `<pivotTableStyleInfo>`"
)]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotStyleInfo {
    /// Name of the built-in style, as `PivotStyleLight16`.
    pub name: Option<String>,
    /// Whether the row headers are emphasised.
    pub show_row_headers: bool,
    /// Whether the column headers are.
    pub show_column_headers: bool,
    /// Whether the rows are banded.
    pub show_row_stripes: bool,
    /// Whether the columns are.
    pub show_column_stripes: bool,
}

/// A pivot table on a sheet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotTable {
    /// The name Excel shows in the field list.
    pub name: String,
    /// Which cache it reads, by the id the workbook gives that cache.
    pub cache_id: u32,
    /// The cells the report occupies.
    pub location: Option<Range>,
    /// Every field of the cache, in the cache's order.
    pub fields: Vec<PivotField>,
    /// Cache field indexes down the side, in order. `-2` is the values field,
    /// which is how the format says "the data fields go here".
    pub row_fields: Vec<i32>,
    /// The same across the top.
    pub column_fields: Vec<i32>,
    /// Cache field indexes used as filters above the report.
    pub page_fields: Vec<i32>,
    /// The values area.
    pub data_fields: Vec<DataField>,
    /// Whether the report ends with a grand total row.
    pub row_grand_totals: bool,
    /// Whether it ends with a grand total column.
    pub column_grand_totals: bool,
    /// How it is painted.
    pub style: PivotStyleInfo,
}

/// Where a cache took its data from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CacheSource {
    /// The sheet the range is on, when the source is a range of cells.
    pub sheet: Option<String>,
    /// The range itself.
    pub range: Option<Range>,
    /// A defined name or table name, when the source is named rather than
    /// addressed.
    pub name: Option<String>,
}

/// One column of the source data, as the cache describes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CacheField {
    /// The heading it took its name from.
    pub name: String,
    /// Number format of its values, as an index into the stylesheet.
    pub number_format: Option<u32>,
    /// The distinct values the cache remembers for it, when it lists them.
    /// A cache written with `saveData="false"` lists none.
    pub shared_items: Vec<String>,
}

/// The data behind one or more pivot tables.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotCache {
    /// The id the workbook gives it, which is what a report points at.
    pub id: u32,
    /// Where the data came from.
    pub source: CacheSource,
    /// The columns of that data.
    pub fields: Vec<CacheField>,
    /// Package path of the definition part this was read from.
    ///
    /// The workbook has to name its caches in `<pivotCaches>`, and each entry
    /// points at a part; the part itself travels unparsed, so the path is what
    /// ties the two together when the workbook is written again.
    pub definition_part: String,
}
