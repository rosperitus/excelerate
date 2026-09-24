//! Excel's auto fill: dragging the fill handle over a range continues what its
//! first cells start instead of copying them.

use super::Axis;
use crate::coordinate::{CellRef, Col, Range, Row, shift_references};
use crate::error::{Error, Result};
use crate::model::{Cell, CellValue, Spreadsheet};
use crate::style::format::is_date_format;

/// The lists Excel continues by name: "Jan" goes on to "Feb", "Пн" to "Вт".
const LISTS: [&[&str]; 8] = [
    &[
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
    &[
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
    &[
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ],
    &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    &[
        "Январь",
        "Февраль",
        "Март",
        "Апрель",
        "Май",
        "Июнь",
        "Июль",
        "Август",
        "Сентябрь",
        "Октябрь",
        "Ноябрь",
        "Декабрь",
    ],
    &[
        "янв", "фев", "мар", "апр", "май", "июн", "июл", "авг", "сен", "окт", "ноя", "дек",
    ],
    &[
        "Понедельник",
        "Вторник",
        "Среда",
        "Четверг",
        "Пятница",
        "Суббота",
        "Воскресенье",
    ],
    &["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"],
];

/// Fills `area` the way dragging the fill handle does: down each column for
/// `Axis::Rows`, right along each row for `Axis::Columns`.
///
/// The seed of each line is its run of filled cells from the start of the
/// area, and the rest of the line continues it:
///
/// - numbers follow their linear trend (`1, 3` → `5, 7`); a single number is
///   copied, a single date steps a day;
/// - text ending in a number counts on (`Кв1` → `Кв2`, `Item 007` →
///   `Item 008`), by the step between the last two seeds;
/// - month and weekday names go round their list, in English or Russian,
///   keeping the case of the seed (`Jan` → `Feb`, `ПН` → `ВТ`);
/// - anything else - formulas, plain text, a mix - is repeated in turn, a
///   formula's references moving as they do in a copy.
///
/// Every filled cell takes the style of the seed cell it continues. A line
/// with no seed, or with nothing after it, is left alone.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet.
pub fn fill_series(book: &mut Spreadsheet, sheet: usize, area: Range, axis: Axis) -> Result<()> {
    auto_fill(book, sheet, area, axis, false)
}

/// [`fill_series`] the other way: the fill handle dragged up (`Axis::Rows`)
/// or left (`Axis::Columns`). The seed is the run of filled cells at the end
/// of the area, and the series runs back from it: `1, 2` above goes `0, -1`,
/// a single `Кв3` goes `Кв2`, `Кв1`, a single date steps back a day.
///
/// # Errors
/// [`Error::SheetIndexOutOfRange`] if there is no such sheet.
pub fn fill_series_back(
    book: &mut Spreadsheet,
    sheet: usize,
    area: Range,
    axis: Axis,
) -> Result<()> {
    auto_fill(book, sheet, area, axis, true)
}

/// Both directions of the fill handle: `back` walks each line from its end.
fn auto_fill(
    book: &mut Spreadsheet,
    sheet: usize,
    area: Range,
    axis: Axis,
    back: bool,
) -> Result<()> {
    let dates: Vec<bool> = book
        .styles
        .all()
        .iter()
        .map(|style| is_date_format(style.number_format.code()))
        .collect();
    let ws = book
        .sheet_mut(sheet)
        .ok_or(Error::SheetIndexOutOfRange(sheet))?;
    let down = axis == Axis::Rows;
    let (along, across) = if down {
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
    let at = |along: u32, across: u32| -> Option<CellRef> {
        if down {
            Some(CellRef::new(Col::new(across)?, Row::new(along)?))
        } else {
            Some(CellRef::new(Col::new(along)?, Row::new(across)?))
        }
    };
    let mut along: Vec<u32> = along.collect();
    if back {
        along.reverse();
    }

    for cross in across {
        let seed: Vec<Cell> = along
            .iter()
            .map_while(|&i| {
                at(i, cross)
                    .and_then(|a| ws.get(a))
                    .filter(|c| !c.value.is_empty())
            })
            .cloned()
            .collect();
        if seed.is_empty() || seed.len() == along.len() {
            continue;
        }
        let date = seed.len() == 1
            && matches!(seed[0].value, CellValue::Number(_))
            && dates
                .get(seed[0].style.index() as usize)
                .copied()
                .unwrap_or(false);
        let next = continuation(&seed, date, back);
        for (step, &i) in along.iter().enumerate().skip(seed.len()) {
            let Some(target) = at(i, cross) else { continue };
            let source = &seed[step % seed.len()];
            let value = match &next {
                Some(next) => next(step),
                None => match &source.value {
                    CellValue::Formula { formula, .. } => {
                        let moved = i64::try_from(step - step % seed.len()).unwrap_or(0);
                        let moved = if back { -moved } else { moved };
                        let (d_col, d_row) = if down { (0, moved) } else { (moved, 0) };
                        CellValue::Formula {
                            formula: shift_references(formula, d_col, d_row),
                            cached: None,
                        }
                    }
                    other => other.clone(),
                },
            };
            let cell = ws.entry(target);
            cell.value = value;
            cell.style = source.style;
        }
    }
    Ok(())
}

/// The value at position `step` of a line, when its seed is a series; `None`
/// when the seed is only repeated.
type Next = Box<dyn Fn(usize) -> CellValue>;

/// `back` only matters for a single seed, which has no step of its own: it
/// then counts down instead of up.
fn continuation(seed: &[Cell], date: bool, back: bool) -> Option<Next> {
    let numbers: Option<Vec<f64>> = seed
        .iter()
        .map(|c| match c.value {
            CellValue::Number(n) => Some(n),
            _ => None,
        })
        .collect();
    if let Some(numbers) = numbers {
        return trend(&numbers, date, back);
    }
    let texts: Option<Vec<String>> = seed
        .iter()
        .map(|c| match &c.value {
            CellValue::Text(_) | CellValue::RichText(_) => c.value.plain_text(),
            _ => None,
        })
        .collect();
    let texts = texts?;
    listed(&texts, back).or_else(|| counted(&texts, back))
}

/// Numbers: the least-squares line through the seed, which for two values is
/// their step. One number is copied, unless it is a date.
#[expect(clippy::cast_precision_loss)]
fn trend(numbers: &[f64], date: bool, back: bool) -> Option<Next> {
    if numbers.len() == 1 {
        let start = numbers[0];
        let day = if back { -1.0 } else { 1.0 };
        return date.then(|| {
            Box::new(move |step: usize| CellValue::Number(start + day * step as f64)) as Next
        });
    }
    let n = numbers.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = numbers.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, y) in numbers.iter().enumerate() {
        let dx = i as f64 - mean_x;
        sxy += dx * (y - mean_y);
        sxx += dx * dx;
    }
    let slope = sxy / sxx;
    let intercept = mean_y - slope * mean_x;
    Some(Box::new(move |step| {
        CellValue::Number(intercept + slope * step as f64)
    }))
}

/// Month and day names: every seed from one list, stepping by the distance
/// between the last two.
fn listed(texts: &[String], back: bool) -> Option<Next> {
    let list = LISTS.iter().find(|list| {
        texts.iter().all(|t| {
            list.iter()
                .any(|name| name.to_lowercase() == t.trim().to_lowercase())
        })
    })?;
    let position = |t: &String| {
        list.iter()
            .position(|name| name.to_lowercase() == t.trim().to_lowercase())
            .unwrap_or(0)
    };
    let positions: Vec<usize> = texts.iter().map(position).collect();
    let len = list.len();
    let latest = positions[positions.len() - 1];
    let step = match positions.len() {
        1 if back => len - 1,
        1 => 1,
        k => (latest + len - positions[k - 2]) % len,
    };
    let first = &texts[0];
    let upper = first.chars().any(char::is_alphabetic) && first.to_uppercase() == *first;
    let lower = first.to_lowercase() == *first;
    let from = texts.len() - 1;
    let names: &'static [&'static str] = list;
    Some(Box::new(move |at| {
        let name = names[(latest + step * (at - from)) % len];
        CellValue::text(if upper {
            name.to_uppercase()
        } else if lower {
            name.to_lowercase()
        } else {
            name.to_owned()
        })
    }))
}

/// Text around a number: the last run of digits counts on, the text around it
/// stays, and leading zeros keep its width.
fn counted(texts: &[String], back: bool) -> Option<Next> {
    let parts: Vec<(String, u64, usize, String)> = texts
        .iter()
        .map(|t| {
            let end = t.rfind(|c: char| c.is_ascii_digit())? + 1;
            let start = t[..end]
                .char_indices()
                .rev()
                .find(|(_, c)| !c.is_ascii_digit())
                .map_or(0, |(i, c)| i + c.len_utf8());
            let number = t[start..end].parse().ok()?;
            Some((
                t[..start].to_owned(),
                number,
                end - start,
                t[end..].to_owned(),
            ))
        })
        .collect::<Option<_>>()?;
    let (prefix, _, width, suffix) = parts[0].clone();
    if parts
        .iter()
        .any(|(p, _, _, s)| *p != prefix || *s != suffix)
    {
        return None;
    }
    let last = i128::from(parts[parts.len() - 1].1);
    let step = match parts.len() {
        1 if back => -1,
        1 => 1,
        k => last - i128::from(parts[k - 2].1),
    };
    let from = texts.len() - 1;
    Some(Box::new(move |at| {
        let ahead = i128::try_from(at - from).unwrap_or(0);
        let number = (last + step * ahead).max(0);
        CellValue::text(format!("{prefix}{number:0width$}{suffix}"))
    }))
}
