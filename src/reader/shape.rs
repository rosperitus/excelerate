//! Reading shapes: the `<xdr:sp>` elements of a drawing.
//!
//! See [`crate::model::shape`] for what is modelled.

use super::chart::{children, read_anchor, tag_attr};
use crate::model::chart::Anchor;
use core::ops::Range;

/// One shape element.
#[derive(Debug)]
pub(crate) struct ShapeElement {
    /// The drawing object id.
    pub id: u32,
    pub name: String,
    pub description: String,
    pub geometry: Option<String>,
    pub text: String,
    /// Whether it sits inside a group of shapes.
    pub grouped: bool,
    /// Where the element sits in the drawing part.
    #[cfg_attr(
        not(feature = "write"),
        expect(dead_code, reason = "only a rewrite of the drawing splices by it")
    )]
    pub span: Range<usize>,
}

/// One top-level object of a drawing that holds at least one shape.
#[derive(Debug)]
pub(crate) struct ShapeObject {
    /// Where the object's element sits in the drawing part.
    #[cfg_attr(
        not(feature = "write"),
        expect(dead_code, reason = "only a rewrite of the drawing splices by it")
    )]
    pub span: Range<usize>,
    pub anchor: Option<Anchor>,
    /// The shapes in it: one for a shape on its own, any number for a group.
    pub shapes: Vec<ShapeElement>,
}

/// The objects of a drawing part that hold shapes.
///
/// One pass of events over the part. Parsing it into nodes level by level
/// read every shape once per level it is nested in, and on a drawing of two
/// thousand shapes that was most of what reading the workbook cost.
pub(crate) fn scan_shapes(xml: &str) -> Vec<ShapeObject> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut objects = Vec::new();
    let mut reader = Reader::from_str(xml);
    let position = |reader: &Reader<&[u8]>| usize::try_from(reader.buffer_position()).unwrap_or(0);
    // Depth 1 is the root, 2 an anchored object.
    let mut depth = 0usize;
    let mut object_start = 0;
    let mut object_name = String::new();
    // Where the object's content starts, after the markers that place it.
    let mut head_end: Option<usize> = None;
    let mut shapes: Vec<ShapeElement> = Vec::new();
    let mut groups = 0usize;
    // Depth of the `mc:Fallback` being skipped, if any.
    let mut fallback: Option<usize> = None;
    loop {
        let before = position(&reader);
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                depth += 1;
                let name = local(e.name().as_ref()).to_owned();
                if depth == 2 {
                    object_start = before;
                    e.name().as_ref().clone_into(&mut object_name);
                    head_end = None;
                    shapes.clear();
                    groups = 0;
                }
                if depth == 3 && head_end.is_none() && !is_marker(&name) {
                    head_end = Some(before);
                }
                match name.as_str() {
                    "Fallback" if fallback.is_none() => fallback = Some(depth),
                    "grpSp" => groups += 1,
                    "sp" if depth > 2 && fallback.is_none() => {
                        let qname = e.name();
                        if reader.read_to_end(qname).is_err() {
                            break;
                        }
                        depth -= 1;
                        let end = position(&reader);
                        shapes.push(shape_element(xml, before..end, groups > 0));
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                if depth == 2 && head_end.is_none() && !is_marker(local(e.name().as_ref())) {
                    head_end = Some(before);
                }
            }
            Ok(Event::End(e)) => {
                let name = local(e.name().as_ref()).to_owned();
                if fallback == Some(depth) {
                    fallback = None;
                } else if name == "grpSp" {
                    groups = groups.saturating_sub(1);
                }
                if depth == 2 && !shapes.is_empty() {
                    let span = object_start..position(&reader);
                    // Only the markers are parsed: the object's start tag and
                    // what precedes its content, closed by hand.
                    let head = format!(
                        "{}</{object_name}>",
                        &xml[object_start..head_end.unwrap_or(span.end)]
                    );
                    let anchor = children(&head)
                        .into_iter()
                        .next()
                        .and_then(|node| read_anchor(&node, &node.children()));
                    objects.push(ShapeObject {
                        span,
                        anchor,
                        shapes: core::mem::take(&mut shapes),
                    });
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    objects
}

/// A name without its prefix.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or_default()
}

/// The elements that place an anchored object, before its content.
fn is_marker(name: &str) -> bool {
    matches!(name, "from" | "to" | "ext" | "pos")
}

/// What the model takes from one `<sp>` element, found by name in its text:
/// only the text body is parsed into nodes.
fn shape_element(xml: &str, span: Range<usize>, grouped: bool) -> ShapeElement {
    let outer = &xml[span.clone()];
    let props = start_tag(outer, "cNvPr");
    let attr = |name: &str| {
        props
            .and_then(|t| tag_attr(t, name))
            .map(|raw| quick_xml::escape::unescape(raw).map_or_else(|_| raw.to_owned(), Into::into))
            .unwrap_or_default()
    };
    ShapeElement {
        id: props
            .and_then(|t| tag_attr(t, "id"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        name: attr("name"),
        description: attr("descr"),
        geometry: start_tag(outer, "prstGeom")
            .and_then(|t| tag_attr(t, "prst"))
            .map(str::to_owned),
        text: text_body(outer).map(body_text).unwrap_or_default(),
        grouped,
        span,
    }
}

/// The first start tag with this local name in `xml`, from `<` to `>`.
fn start_tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(found) = xml[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        let before = &xml[..at];
        // `<name` or `<prefix:name`, followed by the end of the name.
        let opens = before.ends_with('<')
            || before.strip_suffix(':').is_some_and(|head| {
                head.rfind('<').is_some_and(|lt| {
                    let prefix = &head[lt + 1..];
                    !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_alphanumeric())
                })
            });
        let ends = xml[from..].starts_with([' ', '>', '/', '\t', '\r', '\n']);
        if opens && ends {
            let lt = before.rfind('<')?;
            let gt = from + xml[from..].find('>')?;
            return Some(&xml[lt..=gt]);
        }
    }
    None
}

/// The text body element of a shape, from its start tag to its end tag.
fn text_body(shape: &str) -> Option<&str> {
    let open = start_tag(shape, "txBody")?;
    let start = open.as_ptr() as usize - shape.as_ptr() as usize;
    if open.ends_with("/>") {
        return Some(open);
    }
    let close = shape.rfind("txBody>")?;
    let end = close + "txBody>".len();
    (end > start).then(|| &shape[start..end])
}

/// The text of a text body element: runs and fields joined, a line per
/// paragraph.
pub(crate) fn body_text(body: &str) -> String {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut out = String::new();
    let mut reader = Reader::from_str(body);
    let mut paragraphs = 0usize;
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                "p" => {
                    if paragraphs > 0 {
                        out.push('\n');
                    }
                    paragraphs += 1;
                }
                "t" => in_text = true,
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                "p" => {
                    if paragraphs > 0 {
                        out.push('\n');
                    }
                    paragraphs += 1;
                }
                "br" => out.push('\n'),
                _ => {}
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == "t" => in_text = false,
            Ok(Event::Text(t)) if in_text => out.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if in_text => super::zipxml::push_entity(&mut out, &r),
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_attributes_are_found_by_name_only() {
        let sp = concat!(
            r#"<xdr:sp><xdr:nvSpPr><xdr:cNvPr id="7" name="A &amp; B" descr='d'></xdr:cNvPr>"#,
            r#"</xdr:nvSpPr><xdr:spPr><a:prstGeomX/><a:prstGeom prst="ellipse"/></xdr:spPr>"#,
            r#"<xdr:txBody><a:bodyPr/><a:p><a:r><a:t>x</a:t></a:r></a:p></xdr:txBody></xdr:sp>"#,
        );
        let props = start_tag(sp, "cNvPr").unwrap();
        assert_eq!(tag_attr(props, "name"), Some("A &amp; B"));
        assert_eq!(tag_attr(props, "descr"), Some("d"));
        assert_eq!(tag_attr(props, "id"), Some("7"));
        let geometry = start_tag(sp, "prstGeom").unwrap();
        assert_eq!(tag_attr(geometry, "prst"), Some("ellipse"));
        assert_eq!(body_text(text_body(sp).unwrap()), "x");
    }
}
