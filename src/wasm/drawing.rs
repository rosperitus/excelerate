//! Changing the look of shapes and charts: a patch in the shape `shapes` and
//! `charts` read, laid over what the object has. A field left out keeps its
//! value; `null` hands it back to the object's style.

use super::Book;
use super::convert::{array, field};
use super::style::patch::{number, run_font, text};
use crate::model::chart::{
    ChartColor, ColorTransform, Fill, GradientPath, GradientStop, LineFormat, ShapeFormat,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
impl Book {
    /// Changes how a shape looks, by its position in `shapes`:
    ///
    /// ```js
    /// book.setShapeFormat(0, 2, {
    ///   fill: { type: "solid", color: "accent2" },
    ///   line: { width: 2 },
    ///   font: { bold: true, color: "#FFFFFFFF" },
    ///   rotation: 15,
    /// });
    /// ```
    ///
    /// `fill`, `line`, `font`, `rotation` (degrees clockwise), `flipH` and
    /// `flipV`; a field left out stays as it was, `null` for `fill` or `line`
    /// gives it back to the shape's style. A colour is `#RRGGBB`,
    /// `#AARRGGBB` or a theme colour by name (`accent1`, `tx1`, `bg2`), which
    /// follows the theme. The rest of the shape - effects, other runs, the
    /// outline's dashes - stays in the file as it was.
    #[wasm_bindgen(js_name = setShapeFormat)]
    pub fn set_shape_format(
        &mut self,
        sheet: usize,
        index: usize,
        #[wasm_bindgen(unchecked_param_type = "ShapeFormatPatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        let shape = self
            .sheet_mut(sheet)?
            .shapes
            .get_mut(index)
            .ok_or_else(|| JsError::new("no such shape"))?;
        let mut changed = shape.clone();
        lay_format(&mut changed.format, patch)?;
        if let Some(value) = field(patch, "font") {
            let font = run_font(&value)?;
            let own = &mut changed.font;
            // Only what the writer puts in a run: family, size, bold, italic
            // and colour.
            if font.name.is_some() {
                own.name = font.name;
            }
            if font.color.is_some() {
                own.color = font.color;
            }
            own.size = font.size.or(own.size);
            own.bold = font.bold.or(own.bold);
            own.italic = font.italic.or(own.italic);
        }
        if let Some(value) = field(patch, "rotation") {
            let degrees = number(&value, "a rotation")?;
            if !degrees.is_finite() {
                return Err(JsError::new("a rotation is a number of degrees"));
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "taken modulo a full turn, so it fits in 60 000ths of a degree"
            )]
            {
                changed.rotation = (degrees.rem_euclid(360.0) * 60_000.0).round() as i32;
            }
        }
        if let Some(value) = field(patch, "flipH") {
            changed.flip_h = value.is_truthy();
        }
        if let Some(value) = field(patch, "flipV") {
            changed.flip_v = value.is_truthy();
        }
        *shape = changed;
        Ok(())
    }

    /// Changes the fill and outline of a chart's chart area (`format`) and
    /// plot area (`plotFormat`), by its position in `charts`; each takes
    /// `{ fill, line }` as `setShapeFormat` does, or `null` to leave the area
    /// to the chart style.
    #[wasm_bindgen(js_name = setChartFormat)]
    pub fn set_chart_format(
        &mut self,
        sheet: usize,
        index: usize,
        #[wasm_bindgen(unchecked_param_type = "ChartFormatPatch")] patch: &JsValue,
    ) -> Result<(), JsError> {
        let chart = self
            .sheet_mut(sheet)?
            .charts
            .get_mut(index)
            .ok_or_else(|| JsError::new("no such chart"))?;
        let mut format = chart.format.clone();
        let mut plot = chart.plot_format.clone();
        for (key, area) in [("format", &mut format), ("plotFormat", &mut plot)] {
            match field(patch, key) {
                None => {}
                Some(value) if value.is_null() => *area = None,
                Some(value) => lay_format(area.get_or_insert_with(ShapeFormat::default), &value)?,
            }
        }
        (chart.format, chart.plot_format) = (format, plot);
        Ok(())
    }
}

/// Lays `fill` and `line` of a patch over a format.
fn lay_format(format: &mut ShapeFormat, patch: &JsValue) -> Result<(), JsError> {
    if let Some(value) = field(patch, "fill") {
        format.fill = if value.is_null() {
            None
        } else {
            Some(fill_from_js(&value)?)
        };
    }
    match field(patch, "line") {
        None => {}
        Some(value) if value.is_null() => format.line = None,
        Some(value) => {
            let line = format.line.get_or_insert_with(LineFormat::default);
            if let Some(fill) = field(&value, "fill") {
                line.fill = if fill.is_null() {
                    None
                } else {
                    Some(fill_from_js(&fill)?)
                };
            }
            if let Some(width) = field(&value, "width") {
                line.width = if width.is_null() {
                    None
                } else {
                    let points = number(&width, "a line width")?;
                    if !(0.0..=1584.0).contains(&points) {
                        return Err(JsError::new("a line width is between 0 and 1584 points"));
                    }
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "bounded above; 1584 points is the most the format allows"
                    )]
                    Some((points * 12_700.0).round() as u32)
                };
            }
        }
    }
    Ok(())
}

/// A fill in the form `shapes` gives it, `other` excepted: a picture cannot
/// be made from here.
fn fill_from_js(value: &JsValue) -> Result<Fill, JsError> {
    let kind = field(value, "type").ok_or_else(|| JsError::new("a fill has a type"))?;
    let color = |key: &str| field(value, key).map(|c| color_from_js(&c)).transpose();
    Ok(match text(&kind, "a fill type")?.as_str() {
        "none" => Fill::None,
        "solid" => {
            Fill::Solid(color("color")?.ok_or_else(|| JsError::new("a solid fill has a color"))?)
        }
        "gradient" => {
            let stops =
                field(value, "stops").ok_or_else(|| JsError::new("a gradient has stops"))?;
            let stops = array(&stops, "a gradient's stops")?
                .iter()
                .map(|stop| {
                    let at = field(&stop, "position")
                        .ok_or_else(|| JsError::new("a stop has a position"))?;
                    let percent = number(&at, "a stop's position")?;
                    if !(0.0..=100.0).contains(&percent) {
                        return Err(JsError::new("a stop's position is 0 to 100 percent"));
                    }
                    let color =
                        field(&stop, "color").ok_or_else(|| JsError::new("a stop has a color"))?;
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "a percentage checked just above"
                    )]
                    Ok(GradientStop {
                        position: (percent * 1000.0).round() as u32,
                        color: color_from_js(&color)?,
                    })
                })
                .collect::<Result<Vec<_>, JsError>>()?;
            if stops.len() < 2 {
                return Err(JsError::new("a gradient has two stops at least"));
            }
            let angle = match field(value, "angle") {
                Some(a) if !a.is_null() => {
                    let degrees = number(&a, "a gradient's angle")?;
                    if !degrees.is_finite() {
                        return Err(JsError::new("a gradient's angle is a number of degrees"));
                    }
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "taken modulo a full turn, so positive and in range"
                    )]
                    Some((degrees.rem_euclid(360.0) * 60_000.0).round() as u32)
                }
                _ => None,
            };
            let path = match field(value, "path") {
                Some(p) if !p.is_null() => Some(
                    GradientPath::parse(&text(&p, "a gradient's path")?).ok_or_else(|| {
                        JsError::new(r#"a gradient's path is "shape", "circle" or "rect""#)
                    })?,
                ),
                _ => None,
            };
            Fill::Gradient { stops, angle, path }
        }
        "pattern" => Fill::Pattern {
            preset: field(value, "preset")
                .filter(|p| !p.is_null())
                .map(|p| text(&p, "a pattern"))
                .transpose()?,
            foreground: color("foreground")?,
            background: color("background")?,
        },
        other => {
            return Err(JsError::new(&format!(
                r#"a fill is "none", "solid", "gradient" or "pattern", not {other:?}"#
            )));
        }
    })
}

/// A drawing colour: `#RRGGBB`, `#AARRGGBB` (the alpha becomes an opacity),
/// or the name of a theme colour.
fn color_from_js(value: &JsValue) -> Result<ChartColor, JsError> {
    let spelled = text(value, "a colour")?;
    let Some(hex) = spelled.strip_prefix('#') else {
        let color = ChartColor::scheme(&spelled);
        // Any theme will do: the question is only whether the name is one.
        return match color.resolve(None) {
            Some(_) => Ok(color),
            None => Err(JsError::new(&format!(
                "{spelled:?} is not #RRGGBB, #AARRGGBB or a theme colour such as accent1"
            ))),
        };
    };
    let bad = || JsError::new("a colour is #RRGGBB or #AARRGGBB");
    let argb = u32::from_str_radix(hex, 16).map_err(|_| bad())?;
    let (alpha, rgb) = match hex.len() {
        6 => (0xFF, argb),
        8 => (argb >> 24, argb & 0x00FF_FFFF),
        _ => return Err(bad()),
    };
    let mut color = ChartColor::rgb(rgb);
    if alpha != 0xFF {
        #[expect(clippy::cast_possible_wrap, reason = "at most 100 000")]
        color.transforms.push(ColorTransform::Alpha(
            ((alpha * 100_000 + 127) / 255) as i32,
        ));
    }
    Ok(color)
}
