//! Writing shapes: an untouched one as the bytes it came in, a changed or new
//! one spliced into its drawing.
//!
//! Runs after [`super::image`], over the drawing parts it left, and in the
//! same way: the shape element is kept and only what the model names in it is
//! rewritten. See [`crate::model::shape`].

use super::chart::{
    DRAWING_NS, DRAWING_TYPE, MAIN_NS, REL_NS, XML_DECL, drawings, max_object_id, part_text,
    render_anchor, set_part,
};
use super::image::{free_path, reanchor, rename, set_attribute, splice};
use super::xmlesc::escape;
use crate::model::shape::Shape;
use crate::model::{Attachment, Spreadsheet, Worksheet};
use crate::reader::chart::children;
use crate::reader::shape::{ShapeObject, scan_shapes};
use core::ops::Range;
use std::borrow::Cow;

/// The workbook as the writer should see it, shapes applied.
pub(super) fn prepare(book: Cow<'_, Spreadsheet>) -> Cow<'_, Spreadsheet> {
    let dirty: Vec<usize> = book
        .sheets()
        .iter()
        .enumerate()
        .filter(|(_, sheet)| {
            sheet.shapes.iter().any(|s| !s.is_unchanged())
                || drawings(sheet).any(|d| lost_shapes(&book, sheet, d))
        })
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

/// Whether a drawing holds a shape the sheet no longer has.
fn lost_shapes(book: &Spreadsheet, sheet: &Worksheet, drawing: &str) -> bool {
    let mut from_here = sheet
        .shapes
        .iter()
        .filter_map(|s| s.origin.as_ref())
        .filter(|o| o.drawing == drawing)
        .peekable();
    // What the reader counted answers without parsing, as long as a shape of
    // this drawing is left to carry the count.
    if let Some(read) = from_here.peek().map(|o| o.read_from_drawing) {
        return from_here.count() < read;
    }
    let Some(xml) = part_text(book, drawing) else {
        return false;
    };
    if !xml.contains(":sp ") && !xml.contains(":sp>") && !xml.contains("<sp") {
        return false;
    }
    scan_shapes(xml)
        .iter()
        .filter(|o| o.anchor.is_some())
        .flat_map(|o| &o.shapes)
        .any(|s| claimant(&sheet.shapes, drawing, s.id).is_none())
}

fn claimant(shapes: &[Shape], drawing: &str, id: u32) -> Option<usize> {
    shapes.iter().position(|s| {
        s.origin
            .as_ref()
            .is_some_and(|o| o.drawing == drawing && o.id == id)
    })
}

fn apply_sheet(book: &mut Spreadsheet, index: usize) {
    let Some(sheet) = book.sheet(index) else {
        return;
    };
    let shapes = sheet.shapes.clone();
    let mut paths: Vec<String> = drawings(sheet).map(str::to_owned).collect();
    if paths.is_empty() {
        if shapes.iter().all(|s| s.origin.is_some()) {
            return;
        }
        let path = free_path(book, "xl/drawings/drawing", "xml");
        let empty = format!(
            r#"{XML_DECL}<xdr:wsDr xmlns:xdr="{DRAWING_NS}" xmlns:a="{MAIN_NS}"></xdr:wsDr>"#
        );
        set_part(book, &path, Some(DRAWING_TYPE), empty);
        if let Some(sheet) = book.sheet_mut(index) {
            sheet.attachments.push(Attachment {
                kind: format!("{REL_NS}/drawing"),
                target: path.clone(),
            });
        }
        paths.push(path);
    }
    // The first drawing takes the new shapes.
    for (i, path) in paths.iter().enumerate() {
        rewrite_drawing(book, path, &shapes, i == 0);
    }
}

fn rewrite_drawing(book: &mut Spreadsheet, path: &str, shapes: &[Shape], takes_new: bool) {
    let Some(xml) = part_text(book, path).map(str::to_owned) else {
        return;
    };
    let mut splices: Vec<(Range<usize>, String)> = scan_shapes(&xml)
        .iter()
        // An object without an anchor the reader understood never reached
        // the model, so it is not the model's to change.
        .filter(|o| o.anchor.is_some())
        .filter_map(|o| rewrite_object(&xml, o, path, shapes).map(|t| (o.span.clone(), t)))
        .collect();
    if takes_new {
        let mut next_id = max_object_id(&xml);
        let mut objects = String::new();
        for shape in shapes.iter().filter(|s| s.origin.is_none()) {
            next_id = next_id.saturating_add(1);
            objects.push_str(&render_anchor(shape.anchor, &render_shape(shape, next_id)));
        }
        if !objects.is_empty() {
            let close = xml.rfind("</").unwrap_or(xml.len());
            splices.push((close..close, objects));
        }
    }
    if !splices.is_empty() {
        set_part(book, path, Some(DRAWING_TYPE), splice(&xml, splices));
    }
}

/// The new text of one anchored object, or `None` when it stays as it is. An
/// empty text removes the object.
fn rewrite_object(
    xml: &str,
    object: &ShapeObject,
    drawing: &str,
    shapes: &[Shape],
) -> Option<String> {
    let base = object.span.start;
    let original = &xml[object.span.clone()];
    let alone = matches!(object.shapes.as_slice(), [s] if !s.grouped);
    let mut splices: Vec<(Range<usize>, String)> = Vec::new();
    let mut moved_to = None;

    for element in &object.shapes {
        let local = element.span.start - base..element.span.end - base;
        let Some(index) = claimant(shapes, drawing, element.id) else {
            if alone {
                return Some(String::new());
            }
            splices.push((local, String::new()));
            continue;
        };
        let shape = &shapes[index];
        let Some(origin) = &shape.origin else {
            continue;
        };
        let mut text = original[local.clone()].to_owned();
        if shape.name != origin.name || shape.description != origin.description {
            text = rename(&text, &shape.name, &shape.description);
        }
        if shape.geometry != origin.geometry
            && let Some(geometry) = &shape.geometry
        {
            text = regeometry(&text, geometry);
        }
        if shape.text != origin.text {
            text = retext(&text, &shape.text);
        }
        if text != original[local.clone()] {
            splices.push((local, text));
        }
        // A shape inside a group is placed by the group.
        if alone && shape.anchor != origin.anchor {
            moved_to = Some(shape.anchor);
        }
    }
    if splices.is_empty() && moved_to.is_none() {
        return None;
    }
    let text = splice(original, splices);
    Some(match moved_to {
        Some(anchor) => reanchor(&text, anchor),
        None => text,
    })
}

/// A shape element with another preset outline.
///
/// ponytail: a freeform outline (`custGeom`) is left as it is; turning it into
/// a preset means replacing the element, not an attribute.
fn regeometry(element: &str, geometry: &str) -> String {
    let Some(start) = element.find("prstGeom ") else {
        return element.to_owned();
    };
    let Some(end) = element[start..].find('>').map(|e| start + e) else {
        return element.to_owned();
    };
    let tag = set_attribute(&element[start..end], "prst", geometry);
    format!("{}{tag}{}", &element[..start], &element[end..])
}

/// A shape element with its text replaced. The body's own properties, the
/// first paragraph's properties and the first run's formatting are kept, so
/// the new text looks like the old; a shape that had no text gets a body.
fn retext(element: &str, text: &str) -> String {
    let Some(sp) = children(element).into_iter().next() else {
        return element.to_owned();
    };
    let own = if sp.prefix.is_empty() {
        String::new()
    } else {
        format!("{}:", sp.prefix)
    };
    let Some(body) = sp.child("txBody") else {
        let at = sp.inner_start + sp.inner.len();
        let fresh = format!(
            r#"<{own}txBody><a:bodyPr rtlCol="0" anchor="ctr"/><a:lstStyle/>{}</{own}txBody>"#,
            paragraphs(text, "a:", "", "")
        );
        return format!("{}{fresh}{}", &element[..at], &element[at..]);
    };
    let kids = body.children();
    let keep: String = kids
        .iter()
        .filter(|k| matches!(k.name, "bodyPr" | "lstStyle"))
        .map(|k| k.outer)
        .collect();
    let first = kids.iter().find(|k| k.name == "p");
    let a = kids
        .first()
        .filter(|k| !k.prefix.is_empty())
        .map_or_else(|| "a:".to_owned(), |k| format!("{}:", k.prefix));
    let paragraph_props = first.and_then(|p| p.child("pPr")).map_or("", |n| n.outer);
    let run_props = kids
        .iter()
        .filter(|k| k.name == "p")
        .flat_map(crate::reader::chart::Node::children)
        .find_map(|r| r.child("rPr").filter(|_| r.name == "r"))
        .map_or("", |n| n.outer);
    let start = sp.inner_start + body.inner_start;
    let end = start + body.inner.len();
    format!(
        "{}{keep}{}{}",
        &element[..start],
        paragraphs(text, &a, paragraph_props, run_props),
        &element[end..]
    )
}

/// Paragraphs of `DrawingML` text, a paragraph per line.
fn paragraphs(text: &str, a: &str, paragraph_props: &str, run_props: &str) -> String {
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                format!("<{a}p>{paragraph_props}</{a}p>")
            } else {
                format!(
                    "<{a}p>{paragraph_props}<{a}r>{run_props}<{a}t>{}</{a}t></{a}r></{a}p>",
                    escape(line)
                )
            }
        })
        .collect()
}

/// A shape element for a shape made in code, drawn the way Excel draws a new
/// one: the theme's first accent colour with a darker outline and light text.
fn render_shape(shape: &Shape, id: u32) -> String {
    let name = if shape.name.is_empty() {
        format!("Shape {id}")
    } else {
        shape.name.clone()
    };
    let geometry = shape.geometry.as_deref().unwrap_or("rect");
    let body = if shape.text.is_empty() {
        String::new()
    } else {
        format!(
            r#"<xdr:txBody><a:bodyPr vertOverflow="clip" horzOverflow="clip" rtlCol="0" anchor="ctr"/><a:lstStyle/>{}</xdr:txBody>"#,
            paragraphs(&shape.text, "a:", r#"<a:pPr algn="ctr"/>"#, "")
        )
    };
    format!(
        concat!(
            r#"<xdr:sp macro="" textlink=""><xdr:nvSpPr><xdr:cNvPr id="{id}" name="{name}" descr="{descr}"/>"#,
            r#"<xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></a:xfrm>"#,
            r#"<a:prstGeom prst="{geometry}"><a:avLst/></a:prstGeom></xdr:spPr>"#,
            r#"<xdr:style><a:lnRef idx="2"><a:schemeClr val="accent1"><a:shade val="50000"/></a:schemeClr></a:lnRef>"#,
            r#"<a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef>"#,
            r#"<a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef>"#,
            r#"<a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></xdr:style>"#,
            r#"{body}</xdr:sp><xdr:clientData/>"#
        ),
        id = id,
        name = escape(&name),
        descr = escape(&shape.description),
        geometry = escape(geometry),
        body = body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: &str = concat!(
        r#"<xdr:sp macro=""><xdr:nvSpPr><xdr:cNvPr id="2" name="Box"/><xdr:cNvSpPr/></xdr:nvSpPr>"#,
        r#"<xdr:spPr><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr>"#,
        r#"<xdr:txBody><a:bodyPr anchor="t"/><a:lstStyle/><a:p><a:pPr algn="l"/>"#,
        r#"<a:r><a:rPr lang="ru-RU" b="1"/><a:t>old</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#,
    );

    #[test]
    fn new_text_keeps_the_formatting_of_the_old() {
        let out = retext(SHAPE, "one & two\n\nthree");
        assert!(out.contains(r#"<a:bodyPr anchor="t"/><a:lstStyle/>"#));
        assert!(out.contains(
            r#"<a:p><a:pPr algn="l"/><a:r><a:rPr lang="ru-RU" b="1"/><a:t>one &amp; two</a:t></a:r></a:p>"#
        ));
        assert!(out.contains(r#"<a:p><a:pPr algn="l"/></a:p>"#));
        assert!(!out.contains("old"));
        let body = children(&out)[0].child("txBody").unwrap();
        assert_eq!(
            crate::reader::shape::body_text(body.outer),
            "one & two\n\nthree"
        );
    }

    #[test]
    fn a_shape_without_text_gets_a_body_and_geometry_is_an_attribute() {
        let bare = SHAPE.replace(
            &SHAPE[SHAPE.find("<xdr:txBody>").unwrap()..SHAPE.find("</xdr:sp>").unwrap()],
            "",
        );
        let out = retext(&bare, "hi");
        assert!(out.ends_with("<a:t>hi</a:t></a:r></a:p></xdr:txBody></xdr:sp>"));
        assert!(regeometry(SHAPE, "ellipse").contains(r#"<a:prstGeom prst="ellipse">"#));
    }
}
