//! What a sheet holds besides cells - notes, links, tables, drawings, rules,
//! pivots - and the names the workbook defines.

use super::Book;
use super::convert::{count, list, object, opt_count, opt_str, ranges_to_js};
#[cfg(feature = "write")]
use super::js;
#[cfg(feature = "write")]
use crate::coordinate::{CellRef, Col, Range};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
impl Book {
    /// The notes on a sheet, in address order.
    #[wasm_bindgen(unchecked_return_type = "SheetComment[]")]
    pub fn comments(&self, sheet: usize) -> Result<JsValue, JsError> {
        Ok(list(&self.sheet_of(sheet)?.comments, |(at, comment)| {
            object(&[
                ("address", JsValue::from_str(&at.to_string())),
                ("author", JsValue::from_str(&comment.author)),
                ("text", JsValue::from_str(&comment.plain_text())),
            ])
        }))
    }

    /// The hyperlinks of a sheet. `external` tells a URL from a jump inside
    /// the workbook.
    #[wasm_bindgen(unchecked_return_type = "SheetHyperlink[]")]
    pub fn hyperlinks(&self, sheet: usize) -> Result<JsValue, JsError> {
        use crate::model::LinkTarget;
        Ok(list(&self.sheet_of(sheet)?.hyperlinks, |link| {
            let (target, external) = match &link.target {
                LinkTarget::Inside(to) => (to, false),
                LinkTarget::Outside(to) => (to, true),
            };
            object(&[
                ("range", JsValue::from_str(&link.range.to_string())),
                ("target", JsValue::from_str(target)),
                ("external", JsValue::from_bool(external)),
                ("display", opt_str(link.display.as_deref())),
                ("tooltip", opt_str(link.tooltip.as_deref())),
            ])
        }))
    }

    /// The tables of a sheet: what a structured reference like `Sales[Amount]`
    /// names.
    #[wasm_bindgen(unchecked_return_type = "SheetTable[]")]
    pub fn tables(&self, sheet: usize) -> Result<JsValue, JsError> {
        Ok(list(&self.sheet_of(sheet)?.tables, |table| {
            object(&[
                ("name", JsValue::from_str(&table.name)),
                ("displayName", JsValue::from_str(&table.display_name)),
                ("range", JsValue::from_str(&table.range.to_string())),
                ("headerRowCount", opt_count(table.header_row_count)),
                ("totalsRowCount", opt_count(table.totals_row_count)),
                (
                    "columns",
                    list(&table.columns, |column| JsValue::from_str(&column.name)),
                ),
            ])
        }))
    }

    /// The charts on a sheet. `kinds` names the plots drawn in each -
    /// `barChart`, `lineChart` - and a combination chart has more than one.
    #[wasm_bindgen(unchecked_return_type = "SheetChart[]")]
    pub fn charts(&self, sheet: usize) -> Result<JsValue, JsError> {
        Ok(list(&self.sheet_of(sheet)?.charts, |chart| {
            let title = chart
                .title
                .as_ref()
                .and_then(|t| t.text.as_ref())
                .and_then(crate::model::chart::ChartText::shown);
            object(&[
                ("name", JsValue::from_str(&chart.name)),
                ("title", opt_str(title)),
                (
                    "kinds",
                    list(&chart.plots, |plot| JsValue::from_str(plot.kind.element())),
                ),
                (
                    "seriesCount",
                    count(chart.plots.iter().map(|p| p.series.len()).sum()),
                ),
                ("anchor", anchor_to_js(&chart.anchor)),
            ])
        }))
    }

    /// The pictures on a sheet, without their bytes: `imageData` fetches those
    /// for the one that is wanted.
    #[wasm_bindgen(unchecked_return_type = "SheetImage[]")]
    pub fn images(&self, sheet: usize) -> Result<JsValue, JsError> {
        Ok(list(&self.sheet_of(sheet)?.images, |image| {
            object(&[
                ("name", JsValue::from_str(&image.name)),
                ("description", JsValue::from_str(&image.description)),
                ("format", JsValue::from_str(image.format.extension())),
                ("byteLength", count(image.data.len())),
                ("anchor", anchor_to_js(&image.anchor)),
            ])
        }))
    }

    /// The bytes of one picture, by its position in `images`.
    #[wasm_bindgen(js_name = imageData)]
    pub fn image_data(&self, sheet: usize, index: usize) -> Result<Vec<u8>, JsError> {
        self.sheet_of(sheet)?
            .images
            .get(index)
            .map(|image| image.data.clone())
            .ok_or_else(|| JsError::new("no such image"))
    }

    /// The drawn shapes of a sheet, text and all.
    #[wasm_bindgen(unchecked_return_type = "SheetShape[]")]
    pub fn shapes(&self, sheet: usize) -> Result<JsValue, JsError> {
        Ok(list(&self.sheet_of(sheet)?.shapes, |shape| {
            object(&[
                ("name", JsValue::from_str(&shape.name)),
                ("description", JsValue::from_str(&shape.description)),
                ("geometry", opt_str(shape.geometry.as_deref())),
                ("text", JsValue::from_str(&shape.text)),
                ("anchor", anchor_to_js(&shape.anchor)),
            ])
        }))
    }

    /// The names the workbook defines: what `=Total` and `Print_Area` stand
    /// for. `sheet` is set on a name local to one sheet.
    #[wasm_bindgen(js_name = definedNames, unchecked_return_type = "WorkbookName[]")]
    #[must_use]
    pub fn defined_names(&self) -> JsValue {
        list(&self.workbook.defined_names, |name| {
            object(&[
                ("name", JsValue::from_str(&name.name)),
                ("formula", JsValue::from_str(&name.formula)),
                ("sheet", name.sheet.map_or(JsValue::NULL, count)),
                ("hidden", JsValue::from_bool(name.hidden)),
            ])
        })
    }

    /// The rules limiting what cells accept: the dropdown lists, the ranges,
    /// the dates.
    #[wasm_bindgen(js_name = dataValidations, unchecked_return_type = "SheetValidation[]")]
    pub fn data_validations(&self, sheet: usize) -> Result<JsValue, JsError> {
        Ok(list(&self.sheet_of(sheet)?.data_validations, |rule| {
            object(&[
                ("sqref", ranges_to_js(&rule.sqref)),
                ("type", JsValue::from_str(rule.kind.as_str())),
                ("operator", JsValue::from_str(rule.operator.as_str())),
                ("formula1", JsValue::from_str(&rule.formula1)),
                ("formula2", JsValue::from_str(&rule.formula2)),
                ("allowBlank", JsValue::from_bool(rule.allow_blank)),
                ("showDropDown", JsValue::from_bool(!rule.hide_drop_down)),
            ])
        }))
    }

    /// The conditional formatting of a sheet, block by block. The formatting
    /// each rule applies is not modelled here - it lives in the workbook's
    /// differential styles - but what the rule tests is.
    #[wasm_bindgen(
        js_name = conditionalFormats,
        unchecked_return_type = "SheetConditionalFormat[]"
    )]
    pub fn conditional_formats(&self, sheet: usize) -> Result<JsValue, JsError> {
        Ok(list(&self.sheet_of(sheet)?.conditional_formats, |block| {
            let rules = list(&block.rules, |rule| {
                object(&[
                    ("type", JsValue::from_str(rule.kind.as_str())),
                    ("priority", JsValue::from_f64(f64::from(rule.priority))),
                    (
                        "operator",
                        opt_str(rule.operator.map(crate::model::CfOperator::as_str)),
                    ),
                    ("formulas", list(&rule.formulas, JsValue::from)),
                    ("text", opt_str(rule.text.as_deref())),
                ])
            });
            object(&[("sqref", ranges_to_js(&block.sqref)), ("rules", rules)])
        }))
    }

    /// The autofilter over a sheet, or `undefined` when it has none. What the
    /// filter hides is a property of each row - see `rowHidden`.
    #[wasm_bindgen(js_name = autoFilter, unchecked_return_type = "SheetAutoFilter | undefined")]
    pub fn auto_filter(&self, sheet: usize) -> Result<JsValue, JsError> {
        use crate::model::autofilter::ColumnFilter;
        let Some(filter) = &self.sheet_of(sheet)?.auto_filter else {
            return Ok(JsValue::UNDEFINED);
        };
        let columns = list(&filter.columns, |column| {
            let kind = match &column.filter {
                Some(ColumnFilter::Values { .. }) => "values",
                Some(ColumnFilter::Custom { .. }) => "custom",
                Some(ColumnFilter::Dynamic { .. }) => "dynamic",
                Some(ColumnFilter::Top10 { .. }) => "top10",
                None => "none",
            };
            object(&[
                ("colId", JsValue::from_f64(f64::from(column.col_id))),
                ("kind", JsValue::from_str(kind)),
            ])
        });
        Ok(object(&[
            ("range", JsValue::from_str(&filter.range.to_string())),
            ("columns", columns),
        ]))
    }

    /// The pivot reports laid out on a sheet. The numbers they show are cells
    /// like any others; this says where a report sits and how wide its axes
    /// are.
    #[wasm_bindgen(js_name = pivotTables, unchecked_return_type = "SheetPivotTable[]")]
    pub fn pivot_tables(&self, sheet: usize) -> Result<JsValue, JsError> {
        Ok(list(&self.sheet_of(sheet)?.pivot_tables, |pivot| {
            object(&[
                ("name", JsValue::from_str(&pivot.name)),
                (
                    "location",
                    opt_str(pivot.location.map(|r| r.to_string()).as_deref()),
                ),
                ("cacheId", JsValue::from_f64(f64::from(pivot.cache_id))),
                ("rowFields", count(pivot.row_fields.len())),
                ("columnFields", count(pivot.column_fields.len())),
                ("valueFields", count(pivot.data_fields.len())),
            ])
        }))
    }

    /// The array formulas of a sheet, as the areas they cover: `["B2:B4"]`.
    /// The formula itself is on the top left cell of each.
    #[wasm_bindgen(js_name = arrayFormulas)]
    pub fn array_formulas(&self, sheet: usize) -> Result<Vec<String>, JsError> {
        Ok(self
            .sheet_of(sheet)?
            .array_formulas
            .iter()
            .map(ToString::to_string)
            .collect())
    }

    /// The other workbooks this one reads, in the order `[1]Sheet1!A1` counts
    /// them. A path is `undefined` where the file does not name one.
    #[wasm_bindgen(js_name = externalBooks)]
    #[must_use]
    pub fn external_books(&self) -> Vec<JsValue> {
        self.workbook
            .external
            .iter()
            .map(|book| opt_str(book.path.as_deref()))
            .collect()
    }
}

#[cfg(feature = "write")]
#[wasm_bindgen]
impl Book {
    /// Puts a note on a cell, replacing any note already there.
    #[wasm_bindgen(js_name = setComment)]
    pub fn set_comment(
        &mut self,
        sheet: usize,
        address: &str,
        author: &str,
        text: &str,
    ) -> Result<(), JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        let comment = crate::model::Comment {
            author: author.to_owned(),
            text: vec![crate::model::TextRun {
                text: text.to_owned(),
                font: None,
            }],
        };
        self.sheet_mut(sheet)?.comments.insert(at, comment);
        Ok(())
    }

    /// Takes a note off a cell. Answers whether there was one.
    #[wasm_bindgen(js_name = removeComment)]
    pub fn remove_comment(&mut self, sheet: usize, address: &str) -> Result<bool, JsError> {
        let at = CellRef::parse(address).map_err(js)?;
        Ok(self.sheet_mut(sheet)?.comments.remove(&at).is_some())
    }

    /// Puts a link over a cell or a block of them.
    ///
    /// `target` is a URL or a file path unless `inside` is true, and then it
    /// is a place in this workbook: `'Sheet 2'!A1`.
    #[wasm_bindgen(js_name = setHyperlink)]
    pub fn set_hyperlink(
        &mut self,
        sheet: usize,
        range: &str,
        target: &str,
        inside: Option<bool>,
        display: Option<String>,
        tooltip: Option<String>,
    ) -> Result<(), JsError> {
        use crate::model::{Hyperlink, LinkTarget};
        let area = Range::parse(range).map_err(js)?;
        let target = if inside.unwrap_or(false) {
            LinkTarget::Inside(target.to_owned())
        } else {
            LinkTarget::Outside(target.to_owned())
        };
        let links = &mut self.sheet_mut(sheet)?.hyperlinks;
        links.retain(|link| link.range != area);
        links.push(Hyperlink {
            range: area,
            target,
            display,
            tooltip,
        });
        Ok(())
    }

    /// Takes the link off a range. Answers whether there was one.
    #[wasm_bindgen(js_name = removeHyperlink)]
    pub fn remove_hyperlink(&mut self, sheet: usize, range: &str) -> Result<bool, JsError> {
        let area = Range::parse(range).map_err(js)?;
        let links = &mut self.sheet_mut(sheet)?.hyperlinks;
        let before = links.len();
        links.retain(|link| link.range != area);
        Ok(links.len() != before)
    }

    /// Draws a table over a range, the thing `Sales[Amount]` names.
    ///
    /// The first row of the range is the header unless `headerRow` says
    /// otherwise, and the column names are read from it. The name must be
    /// unique in the workbook, as Excel requires.
    #[wasm_bindgen(js_name = addTable)]
    pub fn add_table(
        &mut self,
        sheet: usize,
        name: &str,
        range: &str,
        header_row: Option<bool>,
    ) -> Result<(), JsError> {
        use crate::model::table::{Table, TableColumn, TableStyle};
        let area = Range::parse(range).map_err(js)?;
        let taken = self
            .workbook
            .sheets()
            .iter()
            .flat_map(|ws| &ws.tables)
            .any(|t| t.display_name.eq_ignore_ascii_case(name));
        if taken {
            return Err(JsError::new(
                "a table of that name is already in the workbook",
            ));
        }
        let header = header_row.unwrap_or(true);
        let ws = self.sheet_mut(sheet)?;
        let columns = (area.start.col.index()..=area.end.col.index())
            .zip(1..)
            .map(|(col, id)| {
                let named = Col::new(col)
                    .filter(|_| header)
                    .and_then(|col| ws.get(CellRef::new(col, area.start.row)))
                    .and_then(|cell| cell.value.plain_text())
                    .filter(|text| !text.is_empty());
                TableColumn {
                    id,
                    name: named.unwrap_or_else(|| format!("Column{id}")),
                    totals_row_function: None,
                    totals_row_label: None,
                    calculated_formula: None,
                }
            })
            .collect();
        let id = u32::try_from(ws.tables.len() + 1).unwrap_or(u32::MAX);
        ws.tables.push(Table {
            id,
            name: name.to_owned(),
            display_name: name.to_owned(),
            range: area,
            header_row_count: Some(u32::from(header)),
            totals_row_count: None,
            auto_filter: header.then_some(area),
            columns,
            style: Some(TableStyle::default()),
        });
        Ok(())
    }

    /// Removes a table by name. Answers whether there was one.
    #[wasm_bindgen(js_name = removeTable)]
    pub fn remove_table(&mut self, sheet: usize, name: &str) -> Result<bool, JsError> {
        let tables = &mut self.sheet_mut(sheet)?.tables;
        let before = tables.len();
        tables.retain(|table| !table.display_name.eq_ignore_ascii_case(name));
        Ok(tables.len() != before)
    }

    /// Defines a name, replacing one of the same name. `formula` is what it
    /// stands for - `Sheet1!$A$1:$A$9` or a constant - and `sheet` makes the
    /// name local to that sheet instead of the workbook.
    #[wasm_bindgen(js_name = setDefinedName)]
    pub fn set_defined_name(
        &mut self,
        name: &str,
        formula: &str,
        sheet: Option<usize>,
    ) -> Result<(), JsError> {
        if name.is_empty() {
            return Err(JsError::new("a defined name is not empty"));
        }
        if let Some(index) = sheet {
            self.sheet_of(index)?;
        }
        self.remove_defined_name(name, sheet);
        self.workbook.defined_names.push(crate::model::DefinedName {
            name: name.to_owned(),
            sheet,
            formula: formula.to_owned(),
            hidden: false,
        });
        self.forget_dependencies();
        Ok(())
    }

    /// Removes a defined name. Answers whether there was one.
    #[wasm_bindgen(js_name = removeDefinedName)]
    pub fn remove_defined_name(&mut self, name: &str, sheet: Option<usize>) -> bool {
        let names = &mut self.workbook.defined_names;
        let before = names.len();
        names.retain(|existing| !(existing.name == name && existing.sheet == sheet));
        let removed = names.len() != before;
        if removed {
            self.forget_dependencies();
        }
        removed
    }
}

/// Where a drawing object sits, as `ObjectAnchor`. An absolute anchor is at a
/// point on the sheet rather than at a cell, so it names no row or column.
fn anchor_to_js(anchor: &crate::model::chart::Anchor) -> JsValue {
    use crate::model::chart::Anchor;
    let at = match anchor {
        Anchor::TwoCell { from, .. } | Anchor::OneCell { from, .. } => Some(from),
        Anchor::Absolute { .. } => None,
    };
    let kind = match anchor {
        Anchor::TwoCell { .. } => "twoCell",
        Anchor::OneCell { .. } => "oneCell",
        Anchor::Absolute { .. } => "absolute",
    };
    object(&[
        ("kind", JsValue::from_str(kind)),
        ("row", opt_count(at.map(|m| m.row.one_based()))),
        ("column", opt_count(at.map(|m| m.col.one_based()))),
    ])
}
