//! What conditional formatting does to a cell: which rules match, and the
//! fill, font and borders, colour scale, data bar and icon they give it.
//!
//! The rules are a sheet's [`ConditionalFormat`] blocks, as read from a file
//! or built by hand; this module only applies them.
//!
//! ```
//! use excelerate::at;
//! use excelerate::formula::conditional::ConditionalFormatting;
//! # use excelerate::model::{CfRule, CfRuleType, ConditionalFormat, Spreadsheet, Worksheet};
//! # use excelerate::Range;
//! # let mut book = Spreadsheet::empty();
//! # let mut sheet = Worksheet::new("S")?;
//! # sheet.set(at!("A1"), 5.0);
//! # sheet.conditional_formats.push(ConditionalFormat {
//! #     sqref: vec![Range::parse("A1:A9")?],
//! #     rules: vec![CfRule { kind: CfRuleType::Expression, formulas: vec!["A1>3".into()],
//! #         dxf: Some(0), ..CfRule::default() }],
//! # });
//! # book.add_sheet(sheet)?;
//! let mut cf = ConditionalFormatting::new(&book, 0);
//! let look = cf.look(at!("A1"));
//! assert_eq!(look.dxfs, vec![0]);
//! # Ok::<(), excelerate::Error>(())
//! ```

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::coordinate::{CellRef, Range, shift_references};
use crate::formula::{Engine, Origin, Value};
use crate::model::{
    CellValue, CfOperator, CfRule, CfRuleType, CfScale, CfValue, CfValueType, DataBarAxis,
    Spreadsheet, Worksheet,
};
use crate::shared::date::{Epoch, from_serial};
use crate::style::DifferentialStyle;
use crate::style::format::GENERAL;

/// What conditional formatting gives one cell.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CfLook {
    /// The differential style of every matching rule that has one, the
    /// stronger rule winning each property: what goes over the cell's own.
    pub style: DifferentialStyle,
    /// The fill of a colour scale, as `RRGGBB`.
    pub color_scale: Option<u32>,
    /// A data bar.
    pub data_bar: Option<DataBarLook>,
    /// An icon.
    pub icon: Option<IconLook>,
    /// The differential styles of the rules that matched, by their index in
    /// the workbook's, strongest first. A matched rule with no style (a
    /// colour scale, a bar, an icon set) is not listed.
    pub dxfs: Vec<u32>,
}

impl CfLook {
    /// Whether no rule did anything here.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.style == DifferentialStyle::default()
            && self.color_scale.is_none()
            && self.data_bar.is_none()
            && self.icon.is_none()
    }

    /// Whether a bar or an icon asks for the value itself to be hidden.
    #[must_use]
    pub fn hides_value(&self) -> bool {
        self.data_bar.as_ref().is_some_and(|b| !b.show_value)
            || self.icon.as_ref().is_some_and(|i| !i.show_value)
    }
}

/// A data bar in one cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DataBarLook {
    /// Where the bar starts, as a share of the cell's width from its left
    /// edge: 0 unless the block has negative values and an axis, where a
    /// negative bar ends at the axis and a positive one starts there.
    pub start: f64,
    /// How much of the cell's width the bar takes, `0.0..=1.0`.
    pub length: f64,
    /// Its colour, as `RRGGBB`.
    pub color: u32,
    /// Whether the value is shown beside it.
    pub show_value: bool,
}

/// An icon in one cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconLook {
    /// The set, such as `3TrafficLights1`.
    pub set: String,
    /// Which icon of it, from 0: the one for the lowest values, unless the
    /// set is reversed. In `3TrafficLights1`, 0 is red and 2 green.
    pub index: usize,
    /// Whether the value is shown beside it.
    pub show_value: bool,
}

/// The numbers and texts in the cells one block covers, read once.
#[derive(Debug, Default)]
struct Stats {
    sorted: Vec<f64>,
    mean: f64,
    std_dev: f64,
    /// How often each value occurs, by [`ConditionalFormatting::duplicate_key`].
    counts: HashMap<String, usize>,
}

/// Applies the conditional formatting of one sheet, cell by cell. Built once
/// per sheet: what a rule needs from its whole block (the minimum, the
/// average, the top ten) is worked out on first use and kept.
pub struct ConditionalFormatting<'a> {
    book: &'a Spreadsheet,
    sheet: usize,
    engine: Engine<'a>,
    theme: Vec<u32>,
    /// `(block, Stats)` by block index.
    stats: HashMap<usize, Stats>,
    today: Option<f64>,
}

impl<'a> ConditionalFormatting<'a> {
    /// The conditional formatting of `sheet` in `book`.
    #[must_use]
    pub fn new(book: &'a Spreadsheet, sheet: usize) -> Self {
        Self {
            book,
            sheet,
            engine: Engine::new(book),
            theme: book
                .theme
                .as_deref()
                .map(crate::shared::palette::theme_colors)
                .unwrap_or_default(),
            stats: HashMap::new(),
            today: None,
        }
    }

    fn worksheet(&self) -> Option<&'a Worksheet> {
        self.book.sheet(self.sheet)
    }

    /// What the conditional formatting gives `at`.
    pub fn look(&mut self, at: CellRef) -> CfLook {
        let Some(ws) = self.worksheet() else {
            return CfLook::default();
        };
        // Every rule over the cell, strongest first.
        let mut rules: Vec<(usize, &'a CfRule)> = ws
            .conditional_formats
            .iter()
            .enumerate()
            .filter(|(_, block)| block.sqref.iter().any(|r| r.contains(at)))
            .flat_map(|(i, block)| block.rules.iter().map(move |rule| (i, rule)))
            .collect();
        rules.sort_by_key(|(_, rule)| rule.priority);

        let value = ws
            .get(at)
            .map(|c| c.value.result().clone())
            .unwrap_or_default();
        let mut look = CfLook::default();
        for (block, rule) in rules {
            let matched = match (&rule.kind, &rule.scale) {
                (CfRuleType::ColorScale, Some(scale)) => {
                    if look.color_scale.is_none() {
                        look.color_scale = self.color_scale(block, at, &value, scale);
                    }
                    look.color_scale.is_some()
                }
                (CfRuleType::DataBar, Some(scale)) => {
                    if look.data_bar.is_none() {
                        // The `x14:id` of a 2010 bar: its own lengths and axis.
                        let modern = rule.extensions.is_some();
                        look.data_bar = self.data_bar(block, modern, &value, scale);
                    }
                    look.data_bar.is_some()
                }
                (CfRuleType::IconSet, Some(scale)) => {
                    if look.icon.is_none() {
                        look.icon = self.icon(block, at, &value, scale);
                    }
                    look.icon.is_some()
                }
                _ => self.matches(block, rule, at, &value),
            };
            if !matched {
                continue;
            }
            if let Some(dxf) = rule.dxf {
                look.dxfs.push(dxf);
                if let Some(style) = self.book.styles.differential.get(dxf as usize) {
                    merge_under(&mut look.style, style);
                }
            }
            if rule.stop_if_true {
                break;
            }
        }
        look
    }

    /// The first cell of a block, which its formulas are written for.
    fn anchor(&self, block: usize) -> CellRef {
        self.worksheet()
            .and_then(|ws| ws.conditional_formats.get(block))
            .and_then(|b| b.sqref.first())
            .map(|r| r.start)
            .unwrap_or_default()
    }

    /// A rule's formula, moved from the block's first cell to `at`, and its
    /// answer there.
    fn eval_at(&mut self, block: usize, formula: &str, at: CellRef) -> Value {
        let anchor = self.anchor(block);
        let moved = shift_references(
            formula.trim().trim_start_matches('='),
            i64::from(at.col.index()) - i64::from(anchor.col.index()),
            i64::from(at.row.index()) - i64::from(anchor.row.index()),
        );
        self.engine.eval(Origin::new(self.sheet, at), &moved)
    }

    fn stats(&mut self, block: usize) -> &Stats {
        if !self.stats.contains_key(&block) {
            let stats = self.read_stats(block);
            self.stats.insert(block, stats);
        }
        &self.stats[&block]
    }

    fn read_stats(&self, block: usize) -> Stats {
        let Some(ws) = self.worksheet() else {
            return Stats::default();
        };
        let Some(ranges) = ws.conditional_formats.get(block).map(|b| &b.sqref) else {
            return Stats::default();
        };
        let mut sorted = Vec::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for (at, cell) in ws.iter() {
            if !ranges.iter().any(|r: &Range| r.contains(at)) {
                continue;
            }
            let value = cell.value.result();
            if let CellValue::Number(n) = value {
                sorted.push(*n);
            }
            if !value.is_empty() {
                *counts.entry(self.duplicate_key(value)).or_default() += 1;
            }
        }
        sorted.sort_by(f64::total_cmp);
        #[allow(
            clippy::cast_precision_loss,
            reason = "a block holds far fewer than 2^53 cells"
        )]
        let n = sorted.len() as f64;
        let mean = if sorted.is_empty() {
            0.0
        } else {
            sorted.iter().sum::<f64>() / n
        };
        // Excel's "above average by N standard deviations" uses the population's.
        let std_dev = if sorted.is_empty() {
            0.0
        } else {
            (sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt()
        };
        Stats {
            sorted,
            mean,
            std_dev,
            counts,
        }
    }

    fn shown(&self, value: &CellValue) -> String {
        value.display(GENERAL, self.book.epoch).to_lowercase()
    }

    /// What Duplicate Values compares: the value, text without regard to
    /// case, and the type with it, so the number 1 and the text "1" differ.
    fn duplicate_key(&self, value: &CellValue) -> String {
        let kind = match value {
            CellValue::Number(_) => 'n',
            CellValue::Bool(_) => 'b',
            CellValue::Error(_) => 'e',
            _ => 't',
        };
        format!("{kind}{}", self.shown(value))
    }

    fn matches(&mut self, block: usize, rule: &CfRule, at: CellRef, value: &CellValue) -> bool {
        let text = || self.shown(value);
        let needle = || rule.text.as_deref().unwrap_or_default().to_lowercase();
        match rule.kind {
            CfRuleType::CellIs => {
                let Some(op) = rule.operator else {
                    return false;
                };
                let cell = crate::formula::eval::stored_value(value);
                let mut bound = |i: usize| {
                    rule.formulas
                        .get(i)
                        .map(|f| self.eval_at(block, f, at).scalar().clone())
                };
                let Some(a) = bound(0) else { return false };
                if matches!(a, Value::Error(_)) || matches!(cell, Value::Error(_)) {
                    return false;
                }
                let order = compare(&cell, &a);
                match op {
                    CfOperator::Equal => order == Ordering::Equal,
                    CfOperator::NotEqual => order != Ordering::Equal,
                    CfOperator::GreaterThan => order == Ordering::Greater,
                    CfOperator::LessThan => order == Ordering::Less,
                    CfOperator::GreaterThanOrEqual => order != Ordering::Less,
                    CfOperator::LessThanOrEqual => order != Ordering::Greater,
                    CfOperator::Between | CfOperator::NotBetween => {
                        let Some(b) = bound(1) else { return false };
                        let (lo, hi) = if compare(&a, &b) == Ordering::Greater {
                            (b, a)
                        } else {
                            (a, b)
                        };
                        let inside = compare(&cell, &lo) != Ordering::Less
                            && compare(&cell, &hi) != Ordering::Greater;
                        inside == (op == CfOperator::Between)
                    }
                    #[allow(unreachable_patterns, reason = "the operators may grow")]
                    _ => false,
                }
            }
            CfRuleType::Expression => rule
                .formulas
                .first()
                .is_some_and(|f| truthy(&self.eval_at(block, f, at))),
            CfRuleType::ContainsText => !value.is_empty() && text().contains(&needle()),
            CfRuleType::NotContainsText => !text().contains(&needle()),
            CfRuleType::BeginsWith => !value.is_empty() && text().starts_with(&needle()),
            CfRuleType::EndsWith => !value.is_empty() && text().ends_with(&needle()),
            CfRuleType::ContainsBlanks => text().trim().is_empty(),
            CfRuleType::NotContainsBlanks => !text().trim().is_empty(),
            CfRuleType::ContainsErrors => matches!(value, CellValue::Error(_)),
            CfRuleType::NotContainsErrors => !matches!(value, CellValue::Error(_)),
            CfRuleType::DuplicateValues | CfRuleType::UniqueValues => {
                if value.is_empty() {
                    return false;
                }
                let key = self.duplicate_key(value);
                let count = self.stats(block).counts.get(&key).copied().unwrap_or(0);
                (count > 1) == (rule.kind == CfRuleType::DuplicateValues)
            }
            CfRuleType::Top10 | CfRuleType::AboveAverage => {
                let CellValue::Number(n) = *value else {
                    return false;
                };
                self.ranked(block, rule, n)
            }
            CfRuleType::TimePeriod => {
                let CellValue::Number(n) = *value else {
                    return false;
                };
                let today = self.today(at);
                rule.time_period
                    .as_deref()
                    .is_some_and(|p| in_period(p, n.floor(), today, self.book.epoch))
            }
            CfRuleType::ColorScale | CfRuleType::DataBar | CfRuleType::IconSet => false,
        }
    }

    /// The rules that place a number among its block's: top and bottom N,
    /// above and below the average.
    fn ranked(&mut self, block: usize, rule: &CfRule, n: f64) -> bool {
        match rule.kind {
            CfRuleType::Top10 => {
                let stats = self.stats(block);
                let len = stats.sorted.len();
                let rank = rule.rank.unwrap_or(10) as usize;
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss,
                    reason = "a share of a block's cell count"
                )]
                let take = if rule.percent {
                    ((len as f64 * rank as f64 / 100.0).floor() as usize).max(1)
                } else {
                    rank
                }
                .min(len);
                if take == 0 {
                    return false;
                }
                if rule.bottom {
                    n <= stats.sorted[take - 1]
                } else {
                    n >= stats.sorted[len - take]
                }
            }
            CfRuleType::AboveAverage => {
                let stats = self.stats(block);
                let reach = f64::from(rule.std_dev.unwrap_or(0)) * stats.std_dev;
                if rule.above_average {
                    let line = stats.mean + reach;
                    n > line || (rule.equal_average && n >= line)
                } else {
                    let line = stats.mean - reach;
                    n < line || (rule.equal_average && n <= line)
                }
            }
            _ => false,
        }
    }

    fn today(&mut self, at: CellRef) -> f64 {
        if let Some(t) = self.today {
            return t;
        }
        let t = self
            .engine
            .eval(Origin::new(self.sheet, at), "TODAY()")
            .number()
            .unwrap_or(0.0)
            .floor();
        self.today = Some(t);
        t
    }

    /// A threshold of a scale, a bar or an icon set.
    fn threshold(&mut self, block: usize, point: &CfValue, low: bool) -> Option<f64> {
        let anchor = self.anchor(block);
        // A number, a percent or a percentile may be written as a formula too.
        let p = match point.kind {
            CfValueType::Number | CfValueType::Percent | CfValueType::Percentile => point
                .value
                .trim()
                .parse::<f64>()
                .ok()
                .or_else(|| self.eval_at(block, &point.value, anchor).number().ok()),
            _ => None,
        };
        let stats = self.stats(block);
        let (min, max) = (stats.sorted.first().copied(), stats.sorted.last().copied());
        match point.kind {
            CfValueType::Min => min,
            CfValueType::Max => max,
            CfValueType::Number => p,
            CfValueType::Percent => {
                let (p, min, max) = (p?, min?, max?);
                Some((max - min).mul_add(p / 100.0, min))
            }
            CfValueType::Percentile => percentile(&self.stats(block).sorted, p?),
            CfValueType::Formula => self.eval_at(block, &point.value, anchor).number().ok(),
            #[allow(unreachable_patterns, reason = "the kinds may grow")]
            _ => {
                if low {
                    min
                } else {
                    max
                }
            }
        }
    }

    fn color_scale(
        &mut self,
        block: usize,
        _at: CellRef,
        value: &CellValue,
        scale: &CfScale,
    ) -> Option<u32> {
        let CfScale::Color { values, colors } = scale else {
            return None;
        };
        let CellValue::Number(n) = *value else {
            return None;
        };
        if values.len() < 2 || values.len() != colors.len() {
            return None;
        }
        let last = values.len() - 1;
        let points: Vec<f64> = values
            .iter()
            .enumerate()
            .map(|(i, v)| self.threshold(block, v, i == 0))
            .collect::<Option<_>>()?;
        let rgb: Vec<u32> = colors
            .iter()
            .map(|c| crate::shared::palette::rgb_in(c, &self.theme))
            .collect::<Option<_>>()?;
        if n <= points[0] {
            return Some(rgb[0]);
        }
        if n >= points[last] {
            return Some(rgb[last]);
        }
        let i = (1..=last).find(|&i| n <= points[i]).unwrap_or(last);
        let span = points[i] - points[i - 1];
        let t = if span > 0.0 {
            (n - points[i - 1]) / span
        } else {
            1.0
        };
        Some(lerp(rgb[i - 1], rgb[i], t))
    }

    /// The bar for `value`. A 2010 bar (`modern`) runs 0 to 100 % of the cell
    /// by default, and with negative values in its block draws them from an
    /// axis leftwards in their own colour; a 2007 bar runs 10 to 90 % from the
    /// left edge whatever the sign.
    fn data_bar(
        &mut self,
        block: usize,
        modern: bool,
        value: &CellValue,
        scale: &CfScale,
    ) -> Option<DataBarLook> {
        let CfScale::DataBar {
            values,
            color,
            min_length,
            max_length,
            show_value,
            negative_fill_color,
            axis_position,
            ..
        } = scale
        else {
            return None;
        };
        let CellValue::Number(n) = *value else {
            return None;
        };
        let low = values
            .first()
            .and_then(|v| self.threshold(block, v, true))?;
        let high = values
            .get(1)
            .and_then(|v| self.threshold(block, v, false))?;
        let paint = |c: &crate::style::Color| crate::shared::palette::rgb_in(c, &self.theme);
        let positive = paint(color).unwrap_or(0x63_8E_C6);
        // ponytail: min_length and max_length are not applied to bars drawn
        // from an axis; Excel's own use of them there is not documented.
        if modern && low < 0.0 && *axis_position != DataBarAxis::None {
            let axis = match axis_position {
                DataBarAxis::Middle => 0.5,
                _ if high > 0.0 => -low / (high - low),
                _ => 1.0,
            };
            // How far each side of the axis reaches in value.
            let (left, right) = match axis_position {
                DataBarAxis::Middle => {
                    let reach = low.abs().max(high.abs());
                    (reach, reach)
                }
                _ => (-low, high.max(0.0)),
            };
            let share = |v: f64, reach: f64| {
                if reach > 0.0 {
                    (v / reach).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            };
            return Some(if n < 0.0 {
                let length = share(-n, left) * axis;
                DataBarLook {
                    start: axis - length,
                    length,
                    color: negative_fill_color
                        .as_ref()
                        .and_then(paint)
                        .unwrap_or(positive),
                    show_value: *show_value,
                }
            } else {
                DataBarLook {
                    start: axis,
                    length: share(n, right) * (1.0 - axis),
                    color: positive,
                    show_value: *show_value,
                }
            });
        }
        let t = if high > low {
            ((n - low) / (high - low)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let (short, long) = if modern { (0, 100) } else { (10, 90) };
        let shortest = f64::from(min_length.unwrap_or(short)) / 100.0;
        let longest = f64::from(max_length.unwrap_or(long)) / 100.0;
        Some(DataBarLook {
            start: 0.0,
            length: (longest - shortest).mul_add(t, shortest),
            color: positive,
            show_value: *show_value,
        })
    }

    fn icon(
        &mut self,
        block: usize,
        _at: CellRef,
        value: &CellValue,
        scale: &CfScale,
    ) -> Option<IconLook> {
        let CfScale::IconSet {
            values,
            set,
            show_value,
            reverse,
            ..
        } = scale
        else {
            return None;
        };
        let CellValue::Number(n) = *value else {
            return None;
        };
        let mut index = 0;
        for (i, point) in values.iter().enumerate().skip(1) {
            let Some(line) = self.threshold(block, point, false) else {
                continue;
            };
            let past = if point.greater_or_equal {
                n >= line
            } else {
                n > line
            };
            if past {
                index = i;
            }
        }
        if *reverse {
            index = values.len().saturating_sub(1) - index;
        }
        Some(IconLook {
            set: set.clone().unwrap_or_else(|| "3TrafficLights1".into()),
            index,
            show_value: *show_value,
        })
    }
}

/// Lays `weaker` under `look`: each property `look` does not set yet.
fn merge_under(look: &mut DifferentialStyle, weaker: &DifferentialStyle) {
    match (&mut look.font, &weaker.font) {
        (None, Some(f)) => look.font = Some(f.clone()),
        (Some(a), Some(b)) => {
            a.name = a.name.take().or_else(|| b.name.clone());
            a.size = a.size.or(b.size);
            a.bold = a.bold.or(b.bold);
            a.italic = a.italic.or(b.italic);
            a.underline = a.underline.or(b.underline);
            a.strike = a.strike.or(b.strike);
            a.color = a.color.take().or_else(|| b.color.clone());
            a.script = a.script.or(b.script);
        }
        _ => {}
    }
    match (&mut look.fill, &weaker.fill) {
        (None, Some(f)) => look.fill = Some(f.clone()),
        (Some(a), Some(b)) => {
            a.pattern = a.pattern.or(b.pattern);
            a.foreground = a.foreground.take().or_else(|| b.foreground.clone());
            a.background = a.background.take().or_else(|| b.background.clone());
        }
        _ => {}
    }
    match (&mut look.borders, &weaker.borders) {
        (None, Some(b)) => look.borders = Some(b.clone()),
        (Some(a), Some(b)) => {
            a.left = a.left.take().or_else(|| b.left.clone());
            a.right = a.right.take().or_else(|| b.right.clone());
            a.top = a.top.take().or_else(|| b.top.clone());
            a.bottom = a.bottom.take().or_else(|| b.bottom.clone());
        }
        _ => {}
    }
    if look.number_format.is_none() {
        look.number_format.clone_from(&weaker.number_format);
    }
    if look.alignment.is_none() {
        look.alignment.clone_from(&weaker.alignment);
    }
}

/// Excel's order across types: numbers, then text (without regard to case),
/// then booleans; an empty cell counts as 0 against a number, as "" against
/// text and as `FALSE` against a boolean.
fn compare(a: &Value, b: &Value) -> Ordering {
    let rank = |v: &Value| match v {
        Value::Number(_) => 0,
        Value::Text(_) => 1,
        Value::Bool(_) => 2,
        _ => 3,
    };
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.total_cmp(y),
        (Value::Text(x), Value::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Blank, Value::Number(_)) => compare(&Value::Number(0.0), b),
        (Value::Number(_), Value::Blank) => compare(a, &Value::Number(0.0)),
        (Value::Blank, Value::Text(_)) => compare(&Value::Text(String::new()), b),
        (Value::Text(_), Value::Blank) => compare(a, &Value::Text(String::new())),
        (Value::Blank, Value::Bool(_)) => compare(&Value::Bool(false), b),
        (Value::Bool(_), Value::Blank) => compare(a, &Value::Bool(false)),
        (Value::Blank, Value::Blank) => Ordering::Equal,
        _ => rank(a).cmp(&rank(b)),
    }
}

/// What a formula rule's answer counts as.
fn truthy(value: &Value) -> bool {
    match value.scalar() {
        Value::Bool(b) => *b,
        Value::Number(n) => *n != 0.0,
        Value::Text(t) => t.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// `PERCENTILE.INC` of sorted numbers.
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "a block holds far fewer than 2^53 cells"
    )]
    let rank = (p / 100.0).clamp(0.0, 1.0) * last as f64;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the slice just above"
    )]
    let (lo, hi) = (rank.floor() as usize, rank.ceil() as usize);
    #[allow(clippy::cast_precision_loss, reason = "as above")]
    let t = rank - lo as f64;
    Some((sorted[hi] - sorted[lo]).mul_add(t, sorted[lo]))
}

/// The colour `share` of the way from `from` to `to`, channel by channel,
/// both as `RRGGBB`.
pub(crate) fn lerp(from: u32, to: u32, share: f64) -> u32 {
    let channel = |shift: u32| {
        let start = f64::from((from >> shift) & 0xFF);
        let end = f64::from((to >> shift) & 0xFF);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "between two bytes"
        )]
        let mixed = (end - start)
            .mul_add(share, start)
            .round()
            .clamp(0.0, 255.0) as u32;
        mixed << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// The day of the week of a whole serial, 0 for Sunday: day 1 of the 1900
/// calendar is a Sunday, day 0 of the 1904 one a Friday.
fn weekday(serial: f64, epoch: Epoch) -> f64 {
    let shift = match epoch {
        Epoch::Windows1900 => -1.0,
        Epoch::Mac1904 => 5.0,
    };
    (serial + shift).rem_euclid(7.0)
}

/// Whether day `n` falls in a `timePeriod` rule's span around `today`.
#[allow(clippy::float_cmp, reason = "both are whole days, floored")]
fn in_period(period: &str, n: f64, today: f64, epoch: Epoch) -> bool {
    let month = |s: f64| {
        from_serial(s, epoch)
            .ok()
            .map(|d| i64::from(d.year) * 12 + i64::from(d.month))
    };
    let week_start = |s: f64| s - weekday(s, epoch);
    match period {
        "today" => n == today,
        "yesterday" => n == today - 1.0,
        "tomorrow" => n == today + 1.0,
        "last7Days" => n <= today && n > today - 7.0,
        "thisWeek" | "lastWeek" | "nextWeek" => {
            let shift = match period {
                "lastWeek" => -7.0,
                "nextWeek" => 7.0,
                _ => 0.0,
            };
            let start = week_start(today) + shift;
            n >= start && n < start + 7.0
        }
        "thisMonth" | "lastMonth" | "nextMonth" => {
            let shift = match period {
                "lastMonth" => -1,
                "nextMonth" => 1,
                _ => 0,
            };
            matches!((month(n), month(today)), (Some(a), Some(b)) if a == b + shift)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_blend_by_channel() {
        assert_eq!(lerp(0xF8_69_6B, 0x63_BE_7B, 0.0), 0xF8_69_6B);
        assert_eq!(lerp(0x00_00_00, 0xFF_FF_FF, 0.5), 0x80_80_80);
    }

    #[test]
    fn percentiles_are_inclusive() {
        let sorted = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&sorted, 50.0), Some(3.0));
        assert_eq!(percentile(&sorted, 25.0), Some(2.0));
        assert_eq!(percentile(&sorted, 10.0), Some(1.4));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn weeks_start_on_sunday() {
        // 45658 is 2025-01-01, a Wednesday.
        assert_eq!(weekday(45_658.0, Epoch::Windows1900), 3.0);
        // 2025-01-01 is serial 44196 in the 1904 calendar.
        assert_eq!(weekday(44_196.0, Epoch::Mac1904), 3.0);
        assert!(in_period(
            "thisWeek",
            45_655.0,
            45_658.0,
            Epoch::Windows1900
        ));
        assert!(!in_period(
            "thisWeek",
            45_654.0,
            45_658.0,
            Epoch::Windows1900
        ));
        assert!(in_period(
            "lastMonth",
            45_657.0,
            45_658.0,
            Epoch::Windows1900
        ));
    }
}
