//! The auto filter of a sheet: which column filters which values.
//!
//!
//! Only the criteria are modelled, not their effect. Applying a filter means
//! hiding rows, and a hidden row is already a row property in the file - Excel
//! stores both, and the two are free to disagree in a file written by
//! something else. Recomputing which rows a filter hides is a separate job
//! from carrying the filter across a rewrite.
//!
//! The four kinds of filter could be flattened into one list with a
//! stringly type tag and a value that is sometimes an array; here each kind is
//! its own variant, because `<filters>` and `<top10>` share neither attributes
//! nor arity.

use crate::coordinate::Range;

/// How a custom filter compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterOperator {
    /// The default, and the one xlsx leaves the attribute off for.
    #[default]
    Equal,
    /// `notEqual`.
    NotEqual,
    /// `greaterThan`.
    GreaterThan,
    /// `greaterThanOrEqual`.
    GreaterThanOrEqual,
    /// `lessThan`.
    LessThan,
    /// `lessThanOrEqual`.
    LessThanOrEqual,
}

impl FilterOperator {
    /// The name xlsx uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Equal => "equal",
            Self::NotEqual => "notEqual",
            Self::GreaterThan => "greaterThan",
            Self::GreaterThanOrEqual => "greaterThanOrEqual",
            Self::LessThan => "lessThan",
            Self::LessThanOrEqual => "lessThanOrEqual",
        }
    }

    /// Parses the xlsx name. An absent or unknown operator is `equal`, which
    /// is what the schema says the default is.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "notEqual" => Self::NotEqual,
            "greaterThan" => Self::GreaterThan,
            "greaterThanOrEqual" => Self::GreaterThanOrEqual,
            "lessThan" => Self::LessThan,
            "lessThanOrEqual" => Self::LessThanOrEqual,
            _ => Self::Equal,
        }
    }
}

/// One comparison of a `<customFilters>` block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomFilter {
    /// How to compare.
    pub operator: FilterOperator,
    /// What to compare against, as the file spells it. `*` and `?` are
    /// wildcards here, so the string is not a number even when it looks like
    /// one.
    pub value: String,
}

/// A `<dateGroupItem>`: a date filter given by calendar parts rather than by a
/// serial number.
///
/// Each part is optional and they nest - a filter on a month has a year too,
/// one on a year has nothing else. `grouping` names the finest part present.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DateGroup {
    /// Year, when the filter reaches that far.
    pub year: Option<u32>,
    /// Month, 1..=12.
    pub month: Option<u32>,
    /// Day of the month.
    pub day: Option<u32>,
    /// Hour of the day.
    pub hour: Option<u32>,
    /// Minute.
    pub minute: Option<u32>,
    /// Second.
    pub second: Option<u32>,
    /// `dateTimeGrouping`: the finest part the filter names - `year`, `month`,
    /// `day`, `hour`, `minute` or `second`.
    pub grouping: String,
}

impl DateGroup {
    /// The parts as (attribute name, value), in the order the schema lists
    /// them, skipping the absent ones.
    #[must_use]
    #[cfg(feature = "write")]
    pub(crate) fn parts(&self) -> Vec<(&'static str, u32)> {
        [
            ("year", self.year),
            ("month", self.month),
            ("day", self.day),
            ("hour", self.hour),
            ("minute", self.minute),
            ("second", self.second),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|v| (name, v)))
        .collect()
    }

    /// The same parts by mutable reference, for the reader to fill.
    pub(crate) fn slots(&mut self) -> [(&'static str, &mut Option<u32>); 6] {
        [
            ("year", &mut self.year),
            ("month", &mut self.month),
            ("day", &mut self.day),
            ("hour", &mut self.hour),
            ("minute", &mut self.minute),
            ("second", &mut self.second),
        ]
    }
}

/// What one column of the auto filter keeps.
///
/// The four variants are the four child elements a `<filterColumn>` may have,
/// and it has exactly one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnFilter {
    /// `<filters>`: keep the rows whose value is one of these. Always an OR.
    Values {
        /// Whether empty cells are kept - the `blank` attribute.
        blank: bool,
        /// The literal values, as the file spells them.
        values: Vec<String>,
        /// Date filters given by calendar parts.
        date_groups: Vec<DateGroup>,
    },
    /// `<customFilters>`: one or two comparisons.
    Custom {
        /// Whether the two comparisons are joined by `and` rather than by
        /// `or`.
        and: bool,
        /// The comparisons; the schema allows at most two.
        rules: Vec<CustomFilter>,
    },
    /// `<dynamicFilter>`: a criterion Excel re-evaluates, such as `today` or
    /// `aboveAverage`. What it keeps depends on when it is opened.
    Dynamic {
        /// The `type` attribute: `today`, `thisMonth`, `Q3`, `aboveAverage`...
        /// It is kept as a string: there are 42 of them, they are inert here,
        /// and an enum would only be a longer way to spell the same word.
        kind: String,
        /// `val`, the boundary Excel had computed when it saved.
        value: Option<String>,
        /// `maxVal`, the upper boundary of a range criterion.
        max_value: Option<String>,
    },
    /// `<top10>`: the extreme rows, by value or by percentage.
    Top10 {
        /// `val`: how many, or what percentage.
        value: Option<String>,
        /// `percent`: whether `value` is a percentage.
        percent: bool,
        /// `top`: the largest rather than the smallest. Defaults to on.
        top: bool,
        /// `filterVal`: the cut-off Excel had computed when it saved.
        filter_value: Option<String>,
    },
}

impl ColumnFilter {
    /// The element name this variant is written as.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Values { .. } => "filters",
            Self::Custom { .. } => "customFilters",
            Self::Dynamic { .. } => "dynamicFilter",
            Self::Top10 { .. } => "top10",
        }
    }

    /// An empty filter of the kind the element name says.
    ///
    /// The reader builds one when the element opens and fills it from the
    /// children that follow.
    #[must_use]
    pub(crate) fn empty(tag: &str) -> Option<Self> {
        Some(match tag {
            "filters" => Self::Values {
                blank: false,
                values: Vec::new(),
                date_groups: Vec::new(),
            },
            "customFilters" => Self::Custom {
                and: false,
                rules: Vec::new(),
            },
            "dynamicFilter" => Self::Dynamic {
                kind: String::new(),
                value: None,
                max_value: None,
            },
            "top10" => Self::Top10 {
                value: None,
                percent: false,
                // The schema's default, and the only one Excel writes without
                // the attribute.
                top: true,
                filter_value: None,
            },
            _ => return None,
        })
    }
}

/// One column of the auto filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterColumn {
    /// `colId`: offset from the first column of the filter's range, not a
    /// sheet column. A filter over `C1:E9` numbers `C` as 0.
    pub col_id: u32,
    /// Whether the drop-down arrow is hidden. Only a table's filter uses this;
    /// a sheet's filter always shows its arrows.
    pub hidden_button: bool,
    /// What the column keeps, when it filters at all. A column with a hidden
    /// button and no criteria has none.
    pub filter: Option<ColumnFilter>,
}

impl FilterColumn {
    /// A column that filters nothing, identified by its offset.
    #[must_use]
    pub const fn new(col_id: u32) -> Self {
        Self {
            col_id,
            hidden_button: false,
            filter: None,
        }
    }
}

/// The auto filter of a sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoFilter {
    /// The cells it covers, header row included.
    pub range: Range,
    /// The columns that filter, in the order the file lists them. A column
    /// that filters nothing is absent.
    pub columns: Vec<FilterColumn>,
}

impl AutoFilter {
    /// A filter over a range with no criteria yet - the arrows, and nothing
    /// filtered.
    #[must_use]
    pub const fn new(range: Range) -> Self {
        Self {
            range,
            columns: Vec::new(),
        }
    }

    /// The column at an offset, added if it is not there yet.
    ///
    pub fn column_at(&mut self, col_id: u32) -> &mut FilterColumn {
        if let Some(i) = self.columns.iter().position(|c| c.col_id == col_id) {
            return &mut self.columns[i];
        }
        self.columns.push(FilterColumn::new(col_id));
        // Just pushed, so the list is not empty.
        self.columns.last_mut().unwrap_or_else(|| unreachable!())
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoFilter, ColumnFilter, DateGroup, FilterOperator};
    use crate::coordinate::Range;

    fn range(s: &str) -> Range {
        Range::parse(s).unwrap_or_else(|e| unreachable!("{e}"))
    }

    #[test]
    fn a_column_is_created_once_and_then_found() {
        let mut filter = AutoFilter::new(range("A1:C9"));
        filter.column_at(2).hidden_button = true;
        filter.column_at(0).hidden_button = true;
        // Asking again must return the same column, not append a second one.
        assert!(filter.column_at(2).hidden_button);
        assert_eq!(filter.columns.len(), 2);
        // The order is the order the offsets first appeared, which is the
        // order the file listed them.
        assert_eq!(
            filter.columns.iter().map(|c| c.col_id).collect::<Vec<_>>(),
            vec![2, 0]
        );
    }

    #[test]
    fn an_unknown_operator_is_equal() {
        assert_eq!(FilterOperator::parse("lessThan"), FilterOperator::LessThan);
        // The attribute is absent for `equal`, so anything unrecognised -
        // including the empty string - means it.
        assert_eq!(FilterOperator::parse(""), FilterOperator::Equal);
        assert_eq!(FilterOperator::parse("nonsense"), FilterOperator::Equal);
    }

    #[test]
    fn each_element_name_builds_its_own_variant() {
        for tag in ["filters", "customFilters", "dynamicFilter", "top10"] {
            let filter = ColumnFilter::empty(tag).unwrap_or_else(|| unreachable!());
            assert_eq!(filter.tag(), tag);
        }
        assert!(ColumnFilter::empty("colorFilter").is_none());
    }

    #[test]
    fn a_top_ten_filter_defaults_to_the_top() {
        let filter = ColumnFilter::empty("top10").unwrap_or_else(|| unreachable!());
        assert!(matches!(
            filter,
            ColumnFilter::Top10 {
                top: true,
                percent: false,
                ..
            }
        ));
    }

    #[cfg(feature = "write")]
    #[test]
    fn a_date_group_lists_only_the_parts_it_has() {
        let group = DateGroup {
            year: Some(2026),
            month: Some(8),
            grouping: "month".into(),
            ..DateGroup::default()
        };
        assert_eq!(group.parts(), vec![("year", 2026), ("month", 8)]);
    }
}
