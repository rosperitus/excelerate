//! Cell styles: read as `CellStyle` objects, written as partial patches laid
//! over what a cell already has.

use super::convert::{area_at, at_index, object, rows_of};
#[cfg(feature = "write")]
use super::convert::{array, field, offset};
use super::{Book, js};
use crate::coordinate::{CellRef, Range};
use crate::style::{Border, Color, Style, StyleId};
use std::collections::HashMap;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
impl Book {
    /// How a cell is painted: number format, font, fill, borders and text
    /// placement, as the file states them.
    ///
    /// A cell the file says nothing about answers with Excel's defaults, which
    /// is what Excel itself shows for it.
    #[wasm_bindgen(js_name = cellStyle, unchecked_return_type = "CellStyle")]
    pub fn cell_style(&self, sheet: usize, address: &str) -> Result<JsValue, JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        Ok(style_to_js(
            self.style_at(sheet, at)?.unwrap_or(&Style::default()),
        ))
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = cellStyleAt, unchecked_return_type = "CellStyle")]
    pub fn cell_style_at(&self, sheet: usize, row: u32, column: u32) -> Result<JsValue, JsError> {
        let at = at_index(row, column)?;
        Ok(style_to_js(
            self.style_at(sheet, at)?.unwrap_or(&Style::default()),
        ))
    }

    /// The styles of a rectangle in one call: each distinct style once, and a
    /// grid of indexes into them. `cellStyle` per cell costs a crossing and a
    /// whole style object per cell; a formatted table of ten thousand cells
    /// usually has a dozen styles.
    #[wasm_bindgen(js_name = getRangeStyles, unchecked_return_type = "RangeStyles")]
    pub fn get_range_styles(&self, sheet: usize, range: &str) -> Result<JsValue, JsError> {
        self.styles_of(sheet, Range::parse(range).map_err(js)?)
    }

    /// The same by numbers, as `getRangeAt` takes them.
    #[wasm_bindgen(js_name = getRangeStylesAt, unchecked_return_type = "RangeStyles")]
    pub fn get_range_styles_at(
        &self,
        sheet: usize,
        row: u32,
        column: u32,
        rows: u32,
        columns: u32,
    ) -> Result<JsValue, JsError> {
        self.styles_of(sheet, area_at(row, column, rows, columns)?)
    }
}

#[cfg(feature = "write")]
#[wasm_bindgen]
impl Book {
    /// Paints a cell: a number format, a font, a fill, borders, alignment, or
    /// any part of those.
    ///
    /// The patch is laid over the style the cell has, so a field left out
    /// keeps what was there - `{ font: { bold: true } }` does not reset the
    /// number format. Equal styles share one entry in the workbook's table,
    /// so painting a column costs one style, not one per cell.
    ///
    /// ```js
    /// book.setCellStyle(0, "B2", {
    ///   numberFormat: "#,##0.00",
    ///   font: { bold: true, color: "#FF1F4E79" },
    ///   fill: { pattern: "solid", foreground: "#FFFFE699" },
    ///   alignment: { horizontal: "right" },
    /// });
    /// ```
    #[wasm_bindgen(js_name = setCellStyle)]
    pub fn set_cell_style(
        &mut self,
        sheet: usize,
        address: &str,
        #[wasm_bindgen(unchecked_param_type = "CellStylePatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        self.paint(sheet, Range::new(at, at), patch)
    }

    /// The same by 1-based row and column.
    #[wasm_bindgen(js_name = setCellStyleAt)]
    pub fn set_cell_style_at(
        &mut self,
        sheet: usize,
        row: u32,
        column: u32,
        #[wasm_bindgen(unchecked_param_type = "CellStylePatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        let at = at_index(row, column)?;
        self.paint(sheet, Range::new(at, at), patch)
    }

    /// The same over a rectangle: `setRangeStyle(0, "A1:D1", { font: { bold:
    /// true } })`. Cells the range covers but the sheet has no value for are
    /// created empty, which is what carries the style.
    #[wasm_bindgen(js_name = setRangeStyle)]
    pub fn set_range_style(
        &mut self,
        sheet: usize,
        range: &str,
        #[wasm_bindgen(unchecked_param_type = "CellStylePatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        self.paint(sheet, Range::parse(range).map_err(js)?, patch)
    }

    /// Paints a rectangle cell by cell in one call, `at` its top-left corner:
    /// `grid` points into `styles`, each a patch as `setCellStyle` takes it,
    /// and `null` or `-1` leaves its cell alone. The output of
    /// `getRangeStyles` goes back in unchanged, so a block's formatting can be
    /// read, edited in JS and written back - or copied somewhere else.
    #[wasm_bindgen(js_name = setRangeStyles)]
    pub fn set_range_styles(
        &mut self,
        sheet: usize,
        at: &str,
        #[wasm_bindgen(unchecked_param_type = "RangeStylesPatch")] styles: &JsValue,
    ) -> Result<(), JsError> {
        self.paint_grid(sheet, CellRef::parse(at).map_err(js)?, styles)
    }

    /// The same with the corner given as 1-based row and column.
    #[wasm_bindgen(js_name = setRangeStylesAt)]
    pub fn set_range_styles_at(
        &mut self,
        sheet: usize,
        row: u32,
        column: u32,
        #[wasm_bindgen(unchecked_param_type = "RangeStylesPatch")] styles: &JsValue,
    ) -> Result<(), JsError> {
        self.paint_grid(sheet, at_index(row, column)?, styles)
    }
}

impl Book {
    fn styles_of(&self, sheet: usize, area: Range) -> Result<JsValue, JsError> {
        let ws = self.sheet_of(sheet)?;
        let default = Style::default();
        let mut index: HashMap<StyleId, u32> = HashMap::new();
        let styles = js_sys::Array::new();
        let grid = js_sys::Array::new();
        for line in rows_of(area) {
            let row: js_sys::Array = line
                .map(|at| {
                    let id = ws.get(at).map(|cell| cell.style).unwrap_or_default();
                    let pick = *index.entry(id).or_insert_with(|| {
                        styles.push(&style_to_js(
                            self.workbook.styles.get(id).unwrap_or(&default),
                        )) - 1
                    });
                    JsValue::from(pick)
                })
                .collect();
            grid.push(&row);
        }
        Ok(object(&[("styles", styles.into()), ("grid", grid.into())]))
    }

    /// Lays one patch over every cell of an area.
    #[cfg(feature = "write")]
    fn paint(&mut self, sheet: usize, area: Range, patch: &JsValue) -> Result<(), JsError> {
        let cells = rows_of(area).flatten().map(|at| (at, 0)).collect();
        self.restyle(sheet, cells, std::slice::from_ref(patch))
    }

    /// Reads a `RangeStylesPatch` into the cells it paints.
    #[cfg(feature = "write")]
    fn paint_grid(&mut self, sheet: usize, start: CellRef, input: &JsValue) -> Result<(), JsError> {
        let patches: Vec<JsValue> = array(&field(input, "styles").unwrap_or_default(), "`styles`")?
            .iter()
            .collect();
        let grid = array(&field(input, "grid").unwrap_or_default(), "`grid`")?;
        let mut cells = Vec::new();
        for (r, line) in grid.iter().enumerate() {
            for (c, pick) in array(&line, "every row of `grid`")?.iter().enumerate() {
                // `null` and `-1` leave the cell alone.
                let Some(pick) = pick.as_f64().filter(|p| *p >= 0.0) else {
                    continue;
                };
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "an index into `styles`, checked against its length below"
                )]
                cells.push((offset(start, r, c)?, pick as usize));
            }
        }
        self.restyle(sheet, cells, &patches)
    }

    /// Gives each cell the style it has with `patches[pick]` laid over it.
    ///
    /// Every style is worked out before any is written, so a bad patch
    /// halfway through leaves the sheet as it was, and each distinct (old
    /// style, patch) pair is interned once.
    #[cfg(feature = "write")]
    fn restyle(
        &mut self,
        sheet: usize,
        cells: Vec<(CellRef, usize)>,
        patches: &[JsValue],
    ) -> Result<(), JsError> {
        let ws = self.sheet_of(sheet)?;
        let old: Vec<StyleId> = cells
            .iter()
            .map(|(at, _)| ws.get(*at).map(|cell| cell.style).unwrap_or_default())
            .collect();
        let mut seen: HashMap<(StyleId, usize), StyleId> = HashMap::new();
        let mut ids = Vec::with_capacity(cells.len());
        for (&(_, pick), old) in cells.iter().zip(old) {
            let id = if let Some(&id) = seen.get(&(old, pick)) {
                id
            } else {
                let patch = patches
                    .get(pick)
                    .ok_or_else(|| JsError::new(&format!("no style {pick} in `styles`")))?;
                if !patch.is_object() {
                    return Err(JsError::new("a style patch is an object"));
                }
                let mut style = self.workbook.styles.get(old).cloned().unwrap_or_default();
                patch::apply(&mut style, patch)?;
                let id = self.workbook.styles.intern(style);
                seen.insert((old, pick), id);
                id
            };
            ids.push(id);
        }
        let ws = self.sheet_mut(sheet)?;
        for ((at, _), id) in cells.into_iter().zip(ids) {
            ws.entry(at).style = id;
        }
        Ok(())
    }
}

/// A colour as JS sees it: the text a reader can act on, or `null` where the
/// file left the choice open.
fn color_to_js(color: &Color) -> JsValue {
    match color {
        Color::Auto => JsValue::NULL,
        Color::Argb(argb) => JsValue::from_str(&format!("#{argb:08X}")),
        Color::Indexed(i) => JsValue::from_str(&format!("indexed:{i}")),
        Color::Theme { id, tint } => JsValue::from_str(&if *tint == 0 {
            format!("theme:{id}")
        } else {
            format!("theme:{id}@{}", f64::from(*tint) / 1_000_000.0)
        }),
    }
}

/// One border side as `{ style, color }`.
fn border_to_js(border: &Border) -> JsValue {
    object(&[
        ("style", JsValue::from_str(border.style.as_str())),
        ("color", color_to_js(&border.color)),
    ])
}

/// The whole style of a cell, shaped as the `CellStyle` declaration says.
fn style_to_js(style: &Style) -> JsValue {
    let (font, fill, borders, alignment) =
        (&style.font, &style.fill, &style.borders, &style.alignment);
    let optional = |text: Option<&str>| text.map_or(JsValue::NULL, JsValue::from_str);
    object(&[
        (
            "numberFormat",
            JsValue::from_str(style.number_format.code()),
        ),
        (
            "font",
            object(&[
                ("name", JsValue::from_str(&font.name)),
                ("size", JsValue::from_f64(f64::from(font.size) / 100.0)),
                ("bold", JsValue::from_bool(font.bold)),
                ("italic", JsValue::from_bool(font.italic)),
                ("underline", JsValue::from_str(font.underline.as_str())),
                ("strike", JsValue::from_bool(font.strike)),
                ("color", color_to_js(&font.color)),
            ]),
        ),
        (
            "fill",
            object(&[
                ("pattern", JsValue::from_str(fill.pattern.as_str())),
                ("foreground", color_to_js(&fill.foreground)),
                ("background", color_to_js(&fill.background)),
            ]),
        ),
        (
            "borders",
            object(&[
                ("left", border_to_js(&borders.left)),
                ("right", border_to_js(&borders.right)),
                ("top", border_to_js(&borders.top)),
                ("bottom", border_to_js(&borders.bottom)),
            ]),
        ),
        (
            "alignment",
            object(&[
                ("horizontal", optional(alignment.horizontal.as_str())),
                ("vertical", optional(alignment.vertical.as_str())),
                ("wrapText", JsValue::from_bool(alignment.wrap_text)),
                ("shrinkToFit", JsValue::from_bool(alignment.shrink_to_fit)),
                ("indent", JsValue::from_f64(f64::from(alignment.indent))),
                (
                    "textRotation",
                    JsValue::from_f64(f64::from(alignment.text_rotation)),
                ),
            ]),
        ),
    ])
}

/// Reading a `CellStylePatch`: every field is optional, and one left out
/// keeps what the cell had.
#[cfg(feature = "write")]
mod patch {
    use super::super::convert::field;
    use crate::style::{
        Alignment, BorderStyle, Borders, Color, Fill, Font, HorizontalAlign, NumberFormat, Pattern,
        Style, Underline, VerticalAlign,
    };
    use wasm_bindgen::prelude::*;

    /// Lays a patch over a style.
    pub(super) fn apply(style: &mut Style, patch: &JsValue) -> Result<(), JsError> {
        if let Some(code) = field(patch, "numberFormat") {
            let code = text(&code, "a number format")?;
            style.number_format = if code == crate::style::format::GENERAL {
                NumberFormat::General
            } else {
                NumberFormat::Custom(code)
            };
        }
        if let Some(patch) = field(patch, "font") {
            font(&mut style.font, &patch)?;
        }
        if let Some(patch) = field(patch, "fill") {
            fill(&mut style.fill, &patch)?;
        }
        if let Some(patch) = field(patch, "borders") {
            borders(&mut style.borders, &patch)?;
        }
        if let Some(patch) = field(patch, "alignment") {
            alignment(&mut style.alignment, &patch)?;
        }
        Ok(())
    }

    fn font(font: &mut Font, patch: &JsValue) -> Result<(), JsError> {
        if let Some(name) = field(patch, "name") {
            font.name = text(&name, "a font name")?;
        }
        if let Some(size) = field(patch, "size") {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a font size in hundredths of a point is a small positive number"
            )]
            {
                font.size = (number(&size, "a font size")? * 100.0).round() as u32;
            }
        }
        flag(patch, "bold", &mut font.bold);
        flag(patch, "italic", &mut font.italic);
        flag(patch, "strike", &mut font.strike);
        if let Some(underline) = field(patch, "underline") {
            font.underline = Underline::parse(&text(&underline, "an underline")?);
        }
        if let Some(color) = field(patch, "color") {
            font.color = color_from_js(&color)?;
        }
        Ok(())
    }

    fn fill(fill: &mut Fill, patch: &JsValue) -> Result<(), JsError> {
        if let Some(pattern) = field(patch, "pattern") {
            fill.pattern = Pattern::parse(&text(&pattern, "a fill pattern")?);
        }
        if let Some(color) = field(patch, "foreground") {
            fill.foreground = color_from_js(&color)?;
        }
        if let Some(color) = field(patch, "background") {
            fill.background = color_from_js(&color)?;
        }
        Ok(())
    }

    fn borders(borders: &mut Borders, patch: &JsValue) -> Result<(), JsError> {
        for (key, side) in [
            ("left", &mut borders.left),
            ("right", &mut borders.right),
            ("top", &mut borders.top),
            ("bottom", &mut borders.bottom),
        ] {
            let Some(patch) = field(patch, key) else {
                continue;
            };
            if let Some(kind) = field(&patch, "style") {
                side.style = BorderStyle::parse(&text(&kind, "a border style")?);
            }
            if let Some(color) = field(&patch, "color") {
                side.color = color_from_js(&color)?;
            }
        }
        Ok(())
    }

    fn alignment(alignment: &mut Alignment, patch: &JsValue) -> Result<(), JsError> {
        // `null` gives the placement back to the default.
        if let Some(horizontal) = field(patch, "horizontal") {
            alignment.horizontal = horizontal
                .as_string()
                .map_or(HorizontalAlign::General, |h| HorizontalAlign::parse(&h));
        }
        if let Some(vertical) = field(patch, "vertical") {
            alignment.vertical = vertical
                .as_string()
                .map_or(VerticalAlign::Bottom, |v| VerticalAlign::parse(&v));
        }
        flag(patch, "wrapText", &mut alignment.wrap_text);
        flag(patch, "shrinkToFit", &mut alignment.shrink_to_fit);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "indent steps and a rotation in degrees are small positive numbers"
        )]
        {
            if let Some(indent) = field(patch, "indent") {
                alignment.indent = number(&indent, "an indent")? as u32;
            }
            if let Some(rotation) = field(patch, "textRotation") {
                alignment.text_rotation = number(&rotation, "a text rotation")? as u32;
            }
        }
        Ok(())
    }

    /// A colour as the JS side spells it: `#AARRGGBB` (or `#RRGGBB`),
    /// `indexed:N`, `theme:N` with an optional `@tint`, or `null` for the one
    /// the file leaves open.
    fn color_from_js(value: &JsValue) -> Result<Color, JsError> {
        if value.is_null() {
            return Ok(Color::Auto);
        }
        let text = value
            .as_string()
            .ok_or_else(|| JsError::new("a colour is a string or null"))?;
        if let Some(index) = text.strip_prefix("indexed:") {
            return index
                .parse()
                .map(Color::Indexed)
                .map_err(|_| JsError::new("indexed:N takes a number"));
        }
        if let Some(rest) = text.strip_prefix("theme:") {
            let (id, tint) = match rest.split_once('@') {
                Some((id, tint)) => {
                    let tint: f64 = tint
                        .parse()
                        .map_err(|_| JsError::new("a theme tint is a number between -1 and 1"))?;
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "a tint is between -1 and 1, and the file stores it in millionths"
                    )]
                    (id, (tint * 1_000_000.0).round() as i32)
                }
                None => (rest, 0),
            };
            return id
                .parse()
                .map(|id| Color::Theme { id, tint })
                .map_err(|_| JsError::new("theme:N takes a number"));
        }
        // Written as `#AARRGGBB`, which is how `cellStyle` reads it back; the
        // file itself stores the eight digits alone, so both forms are taken.
        Color::from_argb_str(text.strip_prefix('#').unwrap_or(&text))
            .ok_or_else(|| JsError::new("a colour is #AARRGGBB, indexed:N, theme:N or null"))
    }

    /// Sets a flag the patch names, by JS truthiness.
    fn flag(patch: &JsValue, key: &str, to: &mut bool) {
        if let Some(value) = field(patch, key) {
            *to = value.is_truthy();
        }
    }

    /// A string a patch states, rejected if it is not one.
    fn text(value: &JsValue, what: &str) -> Result<String, JsError> {
        value
            .as_string()
            .ok_or_else(|| JsError::new(&format!("{what} is a string")))
    }

    /// A number a patch states, rejected if it is not one.
    fn number(value: &JsValue, what: &str) -> Result<f64, JsError> {
        value
            .as_f64()
            .ok_or_else(|| JsError::new(&format!("{what} is a number")))
    }
}
