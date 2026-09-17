//! Writing the boxes behind comments.
//!
//! A comment is two parts: its text in `xl/commentsN.xml`, which the writer
//! builds from the model, and the shape Excel draws it in, in a VML part that
//! travels as bytes. A shape without a comment is a stray box; a comment
//! without a shape is not shown at all. So before writing, the VML part is
//! brought in line with the comments: the shape of a comment that is gone is
//! cut out, a comment that has no shape gets one with Excel's defaults, and a
//! sheet that has comments but no VML part gets a new part. Every other byte
//! of the part stays as it was.
//!
//! A comment moved in the model is one of each: its box loses the size and
//! visibility it had. A move made by `edit::insert_rows` and its kin rewrites
//! the anchor in the bytes instead and keeps them.

use super::chart::{part_text, set_part};
use crate::coordinate::CellRef;
use crate::model::{Attachment, Spreadsheet, Worksheet};
use std::borrow::Cow;

const VML_TYPE: &str = "application/vnd.openxmlformats-officedocument.vmlDrawing";
const VML_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing";

/// The shape type every comment box refers to, declared once per part.
const NOTE_SHAPETYPE: &str = concat!(
    r##"<v:shapetype id="_x0000_t202" coordsize="21600,21600" o:spt="202" path="m,l,21600r21600,l21600,xe">"##,
    r#"<v:stroke joinstyle="miter"/><v:path gradientshapeok="t" o:connecttype="rect"/></v:shapetype>"#,
);

/// The workbook as the writer should see it, comment boxes matching comments.
pub(super) fn prepare(book: Cow<'_, Spreadsheet>) -> Cow<'_, Spreadsheet> {
    let dirty: Vec<usize> = book
        .sheets()
        .iter()
        .enumerate()
        .filter(|(_, sheet)| out_of_step(&book, sheet))
        .map(|(i, _)| i)
        .collect();
    if dirty.is_empty() {
        return book;
    }
    let mut book = book.into_owned();
    for index in dirty {
        apply_sheet(&mut book, index);
    }
    Cow::Owned(book)
}

/// The VML part of a sheet, if it has one.
fn vml_of(sheet: &Worksheet) -> Option<&str> {
    sheet
        .attachments
        .iter()
        .find(|a| a.role() == "vmlDrawing")
        .map(|a| a.target.as_str())
}

/// Whether the boxes in the sheet's VML are not the sheet's comments.
fn out_of_step(book: &Spreadsheet, sheet: &Worksheet) -> bool {
    let Some(path) = vml_of(sheet) else {
        return !sheet.comments.is_empty();
    };
    let Some(xml) = part_text(book, path) else {
        return false;
    };
    let shapes = note_shapes(xml);
    shapes.len() != sheet.comments.len()
        || shapes
            .iter()
            .any(|(_, at)| !at.is_some_and(|at| sheet.comments.contains_key(&at)))
}

fn apply_sheet(book: &mut Spreadsheet, index: usize) {
    let Some(sheet) = book.sheet(index) else {
        return;
    };
    let wanted: Vec<CellRef> = sheet.comments.keys().copied().collect();
    let (path, xml) = if let Some(path) = vml_of(sheet) {
        let Some(xml) = part_text(book, path) else {
            return;
        };
        (path.to_owned(), xml.to_owned())
    } else {
        let path = free_name(book);
        // The id block decides which shape ids the part may use; one per
        // sheet keeps two new parts from sharing ids.
        let xml = format!(
            concat!(
                r#"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel">"#,
                r#"<o:shapelayout v:ext="edit"><o:idmap v:ext="edit" data="{}"/></o:shapelayout>{}</xml>"#,
            ),
            index + 1,
            NOTE_SHAPETYPE
        );
        if let Some(sheet) = book.sheet_mut(index) {
            sheet.attachments.push(Attachment {
                kind: VML_REL.to_owned(),
                target: path.clone(),
            });
        }
        (path, xml)
    };

    let shapes = note_shapes(&xml);
    let mut out = String::with_capacity(xml.len());
    let mut last = 0;
    for (span, at) in &shapes {
        if !at.is_some_and(|at| wanted.contains(&at)) {
            out.push_str(&xml[last..span.start]);
            last = span.end;
        }
    }
    out.push_str(&xml[last..]);

    let have: Vec<CellRef> = shapes.iter().filter_map(|(_, at)| *at).collect();
    let first_id = max_shape_id(&out).map_or(1024 * (index + 1) + 1, |id| id + 1);
    let mut added = String::new();
    if !out.contains("_x0000_t202") {
        added.push_str(NOTE_SHAPETYPE);
    }
    for (id, at) in (first_id..).zip(wanted.iter().filter(|at| !have.contains(at))) {
        added.push_str(&note_shape(id, *at));
    }
    if let Some(end) = out.rfind("</xml>") {
        out.insert_str(end, &added);
    }
    set_part(book, &path, Some(VML_TYPE), out);
}

/// A package path for a new VML part that no part has taken.
fn free_name(book: &Spreadsheet) -> String {
    // One more name than there are parts is always enough to find a free one.
    (1..=book.parts.len() + 1)
        .map(|n| format!("xl/drawings/vmlDrawing{n}.vml"))
        .find(|name| !book.parts.iter().any(|p| &p.path == name))
        .unwrap_or_default()
}

/// Every comment box in a VML part: where its element lies, and the cell it
/// belongs to (`None` for one whose cell cannot be read).
///
/// ponytail: looks for the `v:` prefix Excel writes; a part that binds VML
/// to another prefix is left as it is and gets a second set of boxes.
fn note_shapes(xml: &str) -> Vec<(core::ops::Range<usize>, Option<CellRef>)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(found) = xml[from..].find("<v:shape") {
        let start = from + found;
        let after = start + "<v:shape".len();
        // `<v:shapetype` shares the prefix.
        if !xml[after..].starts_with([' ', '>', '\t', '\r', '\n']) {
            from = after;
            continue;
        }
        let Some(close) = xml[after..].find("</v:shape>") else {
            break;
        };
        let end = after + close + "</v:shape>".len();
        let body = &xml[start..end];
        if body.contains("ObjectType=\"Note\"") {
            let number = |tag: &str| -> Option<u32> {
                let open = format!("<x:{tag}>");
                let at = body.find(&open)? + open.len();
                let len = body[at..].find('<')?;
                body[at..at + len].trim().parse().ok()
            };
            let cell = number("Row").zip(number("Column")).and_then(|(r, c)| {
                Some(CellRef::new(
                    crate::coordinate::Col::new(c)?,
                    crate::coordinate::Row::new(r)?,
                ))
            });
            out.push((start..end, cell));
        }
        from = end;
    }
    out
}

/// The largest `_x0000_sN` shape id in the part.
fn max_shape_id(xml: &str) -> Option<usize> {
    xml.match_indices("_x0000_s")
        .filter_map(|(i, m)| {
            let digits = &xml[i + m.len()..];
            let len = digits.find(|c: char| !c.is_ascii_digit())?;
            digits[..len].parse().ok()
        })
        .max()
}

/// A hidden comment box with Excel's defaults, beside the cell it belongs to.
fn note_shape(id: usize, at: CellRef) -> String {
    let (row, col) = (at.row.index(), at.col.index());
    format!(
        concat!(
            r##"<v:shape id="_x0000_s{id}" type="#_x0000_t202" style="position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:108pt;height:59.25pt;z-index:1;visibility:hidden" fillcolor="#ffffe1" o:insetmode="auto">"##,
            r##"<v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/><v:path o:connecttype="none"/>"##,
            r#"<v:textbox style="mso-direction-alt:auto"><div style="text-align:left"></div></v:textbox>"#,
            r#"<x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/>"#,
            r#"<x:Anchor>{c1}, 15, {r1}, 2, {c2}, 15, {r2}, 16</x:Anchor><x:AutoFill>False</x:AutoFill>"#,
            r#"<x:Row>{row}</x:Row><x:Column>{col}</x:Column></x:ClientData></v:shape>"#,
        ),
        id = id,
        c1 = col + 1,
        r1 = row.saturating_sub(1),
        c2 = col + 3,
        r2 = row + 3,
        row = row,
        col = col,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_are_found_by_their_cell_and_shapetypes_are_not_shapes() {
        let at = CellRef::parse("C5").unwrap_or_else(|_| unreachable!());
        let xml = format!("<xml>{NOTE_SHAPETYPE}{}</xml>", note_shape(1025, at));
        let shapes = note_shapes(&xml);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].1, Some(at));
        assert_eq!(max_shape_id(&xml), Some(1025));
    }
}
