//! Writing pivot tables and their caches: untouched ones as the bytes they
//! came in, changed or new ones from the model.
//!
//! A report written from the model is what excelize writes for a new one: the
//! fields on their axes, the values area, a placeholder for the laid-out
//! rows, and a cache without records marked `refreshOnLoad`, so the
//! application that opens the file builds the report from the source range.
//! See [`crate::model::pivot`].

use super::chart::{REL_NS, XML_DECL, part_text, relationships, rels_path, set_part};
use super::image::{free_path, set_attribute};
use super::xlsx::relative_target;
use super::xmlesc::escape;
use crate::error::{Error, Result};
use crate::model::pivot::{PivotCache, PivotTable};
use crate::model::{Attachment, Spreadsheet};
use std::borrow::Cow;
use std::fmt::Write as _;

const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const TABLE_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotTable+xml";
const CACHE_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.pivotCacheDefinition+xml";

/// The workbook as the writer should see it, pivots applied.
///
/// # Errors
/// [`Error::Xlsx`] for a report that cannot be written: one pointing at a
/// cache the workbook does not have, or without a location.
pub(super) fn prepare(book: Cow<'_, Spreadsheet>) -> Result<Cow<'_, Spreadsheet>> {
    let dirty = book.pivot_caches.iter().any(|c| !c.is_unchanged())
        || book.sheets().iter().any(|sheet| {
            sheet.pivot_tables.iter().any(|t| !t.is_unchanged())
                || sheet
                    .attachments
                    .iter()
                    .filter(|a| a.role() == "pivotTable")
                    .any(|a| !claimed(&sheet.pivot_tables, &a.target))
        });
    if !dirty {
        return Ok(book);
    }
    for sheet in book.sheets() {
        for table in sheet.pivot_tables.iter().filter(|t| !t.is_unchanged()) {
            if !book.pivot_caches.iter().any(|c| c.id == table.cache_id) {
                return Err(Error::Xlsx(format!(
                    "pivot table {:?} reads cache {}, which the workbook does not have",
                    table.name, table.cache_id
                )));
            }
            if table.location.is_none() {
                return Err(Error::Xlsx(format!(
                    "pivot table {:?} has no location",
                    table.name
                )));
            }
        }
    }
    let mut book = book.into_owned();
    write_caches(&mut book);
    for index in 0..book.sheets().len() {
        write_tables(&mut book, index);
    }
    Ok(Cow::Owned(book))
}

fn claimed(tables: &[PivotTable], part: &str) -> bool {
    tables
        .iter()
        .any(|t| t.origin.as_ref().is_some_and(|o| o.part == part))
}

fn write_caches(book: &mut Spreadsheet) {
    for index in 0..book.pivot_caches.len() {
        let cache = &book.pivot_caches[index];
        if cache.is_unchanged() {
            continue;
        }
        let mut path = cache.definition_part.clone();
        if path.is_empty() {
            path = free_path(book, "xl/pivotCache/pivotCacheDefinition", "xml");
            book.attachments.push(Attachment {
                kind: format!("{REL_NS}/pivotCacheDefinition"),
                target: path.clone(),
            });
            book.pivot_caches[index].definition_part.clone_from(&path);
        } else {
            // The records went with the old definition.
            remove_with_relationships(book, &path);
        }
        let text = render_cache(&book.pivot_caches[index]);
        set_part(book, &path, Some(CACHE_TYPE), text);
    }
}

/// Drops a part's relationship part and every part it points at, keeping the
/// part itself.
fn remove_with_relationships(book: &mut Spreadsheet, part: &str) {
    let targets: Vec<String> = relationships(book, part)
        .into_iter()
        .map(|r| r.target)
        .collect();
    let rels = rels_path(part);
    book.parts
        .retain(|p| p.path != rels && !targets.contains(&p.path));
}

fn write_tables(book: &mut Spreadsheet, index: usize) {
    let Some(sheet) = book.sheet(index) else {
        return;
    };
    let tables = sheet.pivot_tables.clone();
    let lost: Vec<String> = sheet
        .attachments
        .iter()
        .filter(|a| a.role() == "pivotTable" && !claimed(&tables, &a.target))
        .map(|a| a.target.clone())
        .collect();
    for part in &lost {
        let rels = rels_path(part);
        book.parts.retain(|p| p.path != *part && p.path != rels);
    }
    if let Some(sheet) = book.sheet_mut(index) {
        sheet.attachments.retain(|a| !lost.contains(&a.target));
    }

    for table in tables.iter().filter(|t| !t.is_unchanged()) {
        let Some(cache) = book
            .pivot_caches
            .iter()
            .find(|c| c.id == table.cache_id)
            .cloned()
        else {
            continue;
        };
        let path = if let Some(origin) = &table.origin {
            origin.part.clone()
        } else {
            let path = free_path(book, "xl/pivotTables/pivotTable", "xml");
            if let Some(sheet) = book.sheet_mut(index) {
                sheet.attachments.push(Attachment {
                    kind: format!("{REL_NS}/pivotTable"),
                    target: path.clone(),
                });
            }
            path
        };
        set_part(book, &path, Some(TABLE_TYPE), render_table(table, &cache));
        let base = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        let rels = format!(
            concat!(
                r#"{}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
                r#"<Relationship Id="rId1" Type="{}/pivotCacheDefinition" Target="{}"/></Relationships>"#
            ),
            XML_DECL,
            REL_NS,
            escape(&relative_target(base, &cache.definition_part))
        );
        set_part(book, &rels_path(&path), None, rels);
        refresh_on_load(book, &cache.definition_part);
    }
}

/// Marks a cache to be rebuilt when the file is opened: a report written from
/// the model has no laid-out rows, and this is what makes them appear.
fn refresh_on_load(book: &mut Spreadsheet, path: &str) {
    let Some(xml) = part_text(book, path) else {
        return;
    };
    let Some(start) = xml.find("pivotCacheDefinition") else {
        return;
    };
    let Some(open) = xml[..start].rfind('<') else {
        return;
    };
    let Some(end) = xml[start..].find('>').map(|e| start + e) else {
        return;
    };
    let tag = &xml[open..end];
    if tag.contains(r#"refreshOnLoad="1""#) {
        return;
    }
    let text = format!(
        "{}{}{}",
        &xml[..open],
        set_attribute(tag, "refreshOnLoad", "1"),
        &xml[end..]
    );
    set_part(book, path, Some(CACHE_TYPE), text);
}

fn render_cache(cache: &PivotCache) -> String {
    let mut s = format!(
        concat!(
            r#"{}<pivotCacheDefinition xmlns="{}" xmlns:r="{}" saveData="0" refreshOnLoad="1" "#,
            r#"createdVersion="6" refreshedVersion="6" minRefreshableVersion="3">"#,
            r#"<cacheSource type="worksheet"><worksheetSource"#
        ),
        XML_DECL, MAIN_NS, REL_NS
    );
    if let Some(range) = cache.source.range {
        let _ = write!(s, r#" ref="{range}""#);
    }
    if let Some(sheet) = &cache.source.sheet {
        let _ = write!(s, r#" sheet="{}""#, escape(sheet));
    }
    if let Some(name) = &cache.source.name {
        let _ = write!(s, r#" name="{}""#, escape(name));
    }
    let _ = write!(
        s,
        r#"/></cacheSource><cacheFields count="{}">"#,
        cache.fields.len()
    );
    for field in &cache.fields {
        let _ = write!(s, r#"<cacheField name="{}""#, escape(&field.name));
        if let Some(format) = field.number_format {
            let _ = write!(s, r#" numFmtId="{format}""#);
        }
        s.push('>');
        if field.shared_items.is_empty() {
            // What excelize writes: one blank item, no values.
            s.push_str(r#"<sharedItems containsBlank="1"><m/></sharedItems>"#);
        } else {
            let _ = write!(s, r#"<sharedItems count="{}">"#, field.shared_items.len());
            for item in &field.shared_items {
                let _ = write!(s, r#"<s v="{}"/>"#, escape(item));
            }
            s.push_str("</sharedItems>");
        }
        s.push_str("</cacheField>");
    }
    s.push_str("</cacheFields></pivotCacheDefinition>");
    s
}

fn render_table(table: &PivotTable, cache: &PivotCache) -> String {
    let mut s = format!(
        r#"{}<pivotTableDefinition xmlns="{}" name="{}" cacheId="{}" dataCaption="Values" updatedVersion="6" minRefreshableVersion="3" createdVersion="6""#,
        XML_DECL,
        MAIN_NS,
        escape(&table.name),
        table.cache_id
    );
    if !table.row_grand_totals {
        s.push_str(r#" rowGrandTotals="0""#);
    }
    if !table.column_grand_totals {
        s.push_str(r#" colGrandTotals="0""#);
    }
    let location = table.location.map_or(String::new(), |r| r.to_string());
    let _ = write!(
        s,
        r#"><location ref="{location}" firstHeaderRow="1" firstDataRow="1" firstDataCol="1"/>"#
    );

    render_fields(&mut s, table, cache);

    for (list, tag, items) in [
        (&table.row_fields, "rowFields", "rowItems"),
        (&table.column_fields, "colFields", "colItems"),
    ] {
        if list.is_empty() {
            continue;
        }
        let _ = write!(s, r#"<{tag} count="{}">"#, list.len());
        for x in list {
            let _ = write!(s, r#"<field x="{x}"/>"#);
        }
        // A placeholder for the laid-out rows, rebuilt on open.
        let _ = write!(s, r#"</{tag}><{items} count="1"><i/></{items}>"#);
    }
    if !table.page_fields.is_empty() {
        let _ = write!(s, r#"<pageFields count="{}">"#, table.page_fields.len());
        for fld in &table.page_fields {
            let _ = write!(s, r#"<pageField fld="{fld}" hier="-1"/>"#);
        }
        s.push_str("</pageFields>");
    }
    if !table.data_fields.is_empty() {
        let _ = write!(s, r#"<dataFields count="{}">"#, table.data_fields.len());
        for data in &table.data_fields {
            s.push_str("<dataField");
            if let Some(name) = &data.name {
                let _ = write!(s, r#" name="{}""#, escape(name));
            }
            let _ = write!(
                s,
                r#" fld="{}" subtotal="{}" baseField="0" baseItem="0""#,
                data.field,
                data.subtotal.as_str()
            );
            if let Some(format) = data.number_format {
                let _ = write!(s, r#" numFmtId="{format}""#);
            }
            s.push_str("/>");
        }
        s.push_str("</dataFields>");
    }
    let style = &table.style;
    s.push_str("<pivotTableStyleInfo");
    if let Some(name) = &style.name {
        let _ = write!(s, r#" name="{}""#, escape(name));
    }
    let flag = |on: bool| if on { "1" } else { "0" };
    let _ = write!(
        s,
        r#" showRowHeaders="{}" showColHeaders="{}" showRowStripes="{}" showColStripes="{}" showLastColumn="1"/>"#,
        flag(style.show_row_headers),
        flag(style.show_column_headers),
        flag(style.show_row_stripes),
        flag(style.show_column_stripes)
    );
    s.push_str("</pivotTableDefinition>");
    s
}

/// `<pivotFields>`: one per cache field, placed on the axis the report's
/// lists put it on.
fn render_fields(s: &mut String, table: &PivotTable, cache: &PivotCache) {
    let count = cache.fields.len().max(table.fields.len());
    let _ = write!(s, r#"<pivotFields count="{count}">"#);
    for i in 0..count {
        let index = i32::try_from(i).unwrap_or(i32::MAX);
        let field = table.fields.get(i).cloned().unwrap_or_default();
        let axis = if table.row_fields.contains(&index) {
            Some("axisRow")
        } else if table.column_fields.contains(&index) {
            Some("axisCol")
        } else if table.page_fields.contains(&index) {
            Some("axisPage")
        } else {
            None
        };
        s.push_str("<pivotField");
        if let Some(axis) = axis {
            let _ = write!(s, r#" axis="{axis}""#);
        }
        let data = field.data_field
            || table
                .data_fields
                .iter()
                .any(|d| usize::try_from(d.field).is_ok_and(|f| f == i));
        if data {
            s.push_str(r#" dataField="1""#);
        }
        s.push_str(if field.show_all {
            r#" showAll="1""#
        } else {
            r#" showAll="0""#
        });
        if !field.default_subtotal {
            s.push_str(r#" defaultSubtotal="0""#);
        }
        let shared = cache.fields.get(i).map_or(0, |f| f.shared_items.len());
        let items = shared + usize::from(field.default_subtotal);
        if axis.is_some() && items > 0 {
            let _ = write!(s, r#"><items count="{items}">"#);
            for x in 0..shared {
                let _ = write!(s, r#"<item x="{x}"/>"#);
            }
            if field.default_subtotal {
                s.push_str(r#"<item t="default"/>"#);
            }
            s.push_str("</items></pivotField>");
        } else {
            s.push_str("/>");
        }
    }
    s.push_str("</pivotFields>");
}
