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
//! A box that no longer shows or measures what its comment says is changed in
//! place: the visibility and size in its `style`, `<x:Visible/>`, and the far
//! corner of its anchor, which is what Excel places it by.
//!
//! A comment moved in the model is one of each: its box starts over at the
//! default place. A move made by `edit::insert_rows` and its kin rewrites the
//! anchor in the bytes instead and keeps it.

use super::chart::{part_text, set_part};
use crate::coordinate::CellRef;
use crate::model::{Attachment, Comment, Spreadsheet, Worksheet};
use crate::reader::vml::{NoteShape, note_shapes, style_of};
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

/// Whether the boxes in the sheet's VML are not the sheet's comments: one
/// too many or too few, or one that shows, or is sized, differently.
fn out_of_step(book: &Spreadsheet, sheet: &Worksheet) -> bool {
    let Some(path) = vml_of(sheet) else {
        return !sheet.comments.is_empty();
    };
    let Some(xml) = part_text(book, path) else {
        return false;
    };
    let shapes = note_shapes(xml);
    shapes.len() != sheet.comments.len()
        || shapes.iter().any(|shape| {
            shape
                .cell
                .and_then(|at| sheet.comments.get(&at))
                .is_none_or(|note| differs(shape, note))
        })
}

/// Whether a box no longer shows or measures what its comment says. A
/// comment with no size keeps whatever size its box has.
fn differs(shape: &NoteShape, note: &Comment) -> bool {
    shape.visible != note.visible || note.size.is_some_and(|size| shape.size != Some(size))
}

fn apply_sheet(book: &mut Spreadsheet, index: usize) {
    let Some(sheet) = book.sheet(index) else {
        return;
    };
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
        (path, xml)
    };

    // A box whose comment is gone is cut; one whose comment shows or measures
    // differently is restyled in place.
    let shapes = note_shapes(&xml);
    let mut out = String::with_capacity(xml.len());
    let mut last = 0;
    for shape in &shapes {
        out.push_str(&xml[last..shape.span.start]);
        last = shape.span.end;
        let body = &xml[shape.span.clone()];
        match shape.cell.and_then(|at| sheet.comments.get(&at)) {
            None => {}
            Some(note) if differs(shape, note) => {
                out.push_str(&restyled(body, shape, note, sheet));
            }
            Some(_) => out.push_str(body),
        }
    }
    out.push_str(&xml[last..]);

    let have: Vec<CellRef> = shapes.iter().filter_map(|s| s.cell).collect();
    let first_id = max_shape_id(&out).map_or(1024 * (index + 1) + 1, |id| id + 1);
    let mut added = String::new();
    if !out.contains("_x0000_t202") {
        added.push_str(NOTE_SHAPETYPE);
    }
    let missing = sheet.comments.iter().filter(|(at, _)| !have.contains(at));
    for (id, (at, note)) in (first_id..).zip(missing) {
        added.push_str(&note_shape(id, *at, note, sheet));
    }
    if let Some(end) = out.rfind("</xml>") {
        out.insert_str(end, &added);
    }
    if vml_of(sheet).is_none()
        && let Some(sheet) = book.sheet_mut(index)
    {
        sheet.attachments.push(Attachment {
            kind: VML_REL.to_owned(),
            target: path.clone(),
        });
    }
    set_part(book, &path, Some(VML_TYPE), out);
}

/// A box made to show and measure what its comment says; every other byte
/// of it stays.
fn restyled(body: &str, shape: &NoteShape, note: &Comment, sheet: &Worksheet) -> String {
    let mut out = body.to_owned();
    if let Some(style) = style_of(body) {
        let mut declarations: Vec<(String, String)> = style
            .split(';')
            .filter_map(|d| d.split_once(':'))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .collect();
        let mut set =
            |name: &str, value: String| match declarations.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = value,
                None => declarations.push((name.to_owned(), value)),
            };
        set("visibility", visibility(note.visible).to_owned());
        if let Some((width, height)) = note.size {
            set("width", format!("{width}pt"));
            set("height", format!("{height}pt"));
        }
        let style_new: Vec<String> = declarations
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect();
        out = out.replacen(style, &style_new.join(";"), 1);
    }
    // `<x:Visible/>` is what Excel reads; the style is for other VML readers.
    match (note.visible, out.contains("<x:Visible/>")) {
        (true, false) => {
            if let Some(at) = out.find("</x:ClientData>") {
                out.insert_str(at, "<x:Visible/>");
            }
        }
        (false, true) => out = out.replacen("<x:Visible/>", "", 1),
        _ => {}
    }
    // The box is placed by its anchor, so a new size moves the far corner.
    if let (Some(size), Some(anchor)) = (note.size, shape.anchor)
        && shape.size != Some(size)
    {
        let new = anchor_text(sheet, [anchor[0], anchor[1], anchor[2], anchor[3]], size);
        if let Some(from) = out.find("<x:Anchor>")
            && let Some(len) = out[from..].find("</x:Anchor>")
        {
            out.replace_range(from..from + len + "</x:Anchor>".len(), &new);
        }
    }
    out
}

const fn visibility(visible: bool) -> &'static str {
    if visible { "visible" } else { "hidden" }
}

/// A package path for a new VML part that no part has taken.
fn free_name(book: &Spreadsheet) -> String {
    // One more name than there are parts is always enough to find a free one.
    (1..=book.parts.len() + 1)
        .map(|n| format!("xl/drawings/vmlDrawing{n}.vml"))
        .find(|name| !book.parts.iter().any(|p| &p.path == name))
        .unwrap_or_default()
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

/// A comment box with Excel's defaults, beside the cell it belongs to.
fn note_shape(id: usize, at: CellRef, note: &Comment, sheet: &Worksheet) -> String {
    let (row, col) = (at.row.index(), at.col.index());
    let start = [col + 1, 15, row.saturating_sub(1), 2];
    let (anchor, (width, height)) = match note.size {
        Some(size) => (anchor_text(sheet, start, size), size),
        // Excel's own corner for its own size, whatever the columns measure.
        None => (
            format!(
                "<x:Anchor>{}, 15, {}, 2, {}, 15, {}, 16</x:Anchor>",
                col + 1,
                row.saturating_sub(1),
                col + 3,
                row + 3
            ),
            (108.0, 59.25),
        ),
    };
    format!(
        concat!(
            r##"<v:shape id="_x0000_s{id}" type="#_x0000_t202" style="position:absolute;margin-left:59.25pt;margin-top:1.5pt;width:{width}pt;height:{height}pt;z-index:1;visibility:{visibility}" fillcolor="#ffffe1" o:insetmode="auto">"##,
            r##"<v:fill color2="#ffffe1"/><v:shadow on="t" color="black" obscured="t"/><v:path o:connecttype="none"/>"##,
            r#"<v:textbox style="mso-direction-alt:auto"><div style="text-align:left"></div></v:textbox>"#,
            r#"<x:ClientData ObjectType="Note"><x:MoveWithCells/><x:SizeWithCells/>"#,
            r#"{anchor}<x:AutoFill>False</x:AutoFill>"#,
            r#"<x:Row>{row}</x:Row><x:Column>{col}</x:Column>{shown}</x:ClientData></v:shape>"#,
        ),
        id = id,
        width = width,
        height = height,
        visibility = visibility(note.visible),
        anchor = anchor,
        row = row,
        col = col,
        shown = if note.visible { "<x:Visible/>" } else { "" },
    )
}

/// `<x:Anchor>` for a box whose top-left corner is `start` (column, offset,
/// row, offset, in pixels) and whose size is given in points.
fn anchor_text(sheet: &Worksheet, start: [u32; 4], (width, height): (f64, f64)) -> String {
    let [c1, dx1, r1, dy1] = start;
    let (c2, dx2) = walk(
        c1,
        dx1,
        points_to_pixels(width),
        crate::coordinate::MAX_COL,
        |c| column_pixels(sheet, c),
    );
    let (r2, dy2) = walk(
        r1,
        dy1,
        points_to_pixels(height),
        crate::coordinate::MAX_ROW,
        |r| row_pixels(sheet, r),
    );
    format!("<x:Anchor>{c1}, {dx1}, {r1}, {dy1}, {c2}, {dx2}, {r2}, {dy2}</x:Anchor>")
}

/// Where a run of `pixels` starting at `offset` into unit `index` ends, as a
/// unit and an offset into it. Hidden units measure nothing and are crossed.
fn walk(
    mut index: u32,
    offset: u32,
    pixels: u32,
    limit: u32,
    size: impl Fn(u32) -> u32,
) -> (u32, u32) {
    let mut left = pixels + offset;
    while index + 1 < limit {
        let here = size(index);
        if left < here {
            break;
        }
        left -= here;
        index += 1;
    }
    (index, left)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to a u32 range first"
)]
fn points_to_pixels(points: f64) -> u32 {
    (points * 96.0 / 72.0)
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32
}

/// A column in pixels, at the maximum digit width of Calibri 11 - seven
/// pixels - that Excel's widths assume.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to a u32 range first"
)]
fn column_pixels(sheet: &Worksheet, index: u32) -> u32 {
    let Some(col) = crate::coordinate::Col::new(index) else {
        return 0;
    };
    if sheet.column_run(col).is_some_and(|run| run.hidden) {
        return 0;
    }
    // 8.43 characters and five pixels of padding: 64 pixels.
    let width = sheet.column_width(col).unwrap_or(9.140_625);
    (width * 7.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// A row in pixels at 96 dpi.
fn row_pixels(sheet: &Worksheet, index: u32) -> u32 {
    let Some(row) = crate::coordinate::Row::new(index) else {
        return 0;
    };
    if sheet.rows.get(&row).is_some_and(|r| r.hidden) {
        return 0;
    }
    points_to_pixels(sheet.row_height(row).unwrap_or(15.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_are_found_by_their_cell_and_shapetypes_are_not_shapes() {
        let at = CellRef::parse("C5").unwrap_or_else(|_| unreachable!());
        let sheet = Worksheet::default();
        let note = Comment::default();
        let xml = format!(
            "<xml>{NOTE_SHAPETYPE}{}</xml>",
            note_shape(1025, at, &note, &sheet)
        );
        let shapes = note_shapes(&xml);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].cell, Some(at));
        assert!(!differs(&shapes[0], &note));
        assert_eq!(max_shape_id(&xml), Some(1025));
    }

    #[test]
    fn a_resized_box_ends_where_its_size_says() {
        let mut sheet = Worksheet::default();
        // Column C is hidden: the box crosses it at no width.
        let c = crate::coordinate::Col::new(2).unwrap_or_else(|| unreachable!());
        sheet.column_entry(c).hidden = true;
        // From B, 15 px in: 144 px is 49 left of B, D's 64, then 31 into E.
        assert_eq!(
            anchor_text(&sheet, [1, 15, 0, 2], (108.0, 30.0)),
            "<x:Anchor>1, 15, 0, 2, 4, 31, 2, 2</x:Anchor>"
        );
    }
}
