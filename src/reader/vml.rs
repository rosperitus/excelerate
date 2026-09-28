//! The comment boxes in a sheet's VML part.
//!
//! The part itself travels as bytes; what is read out of it is which cell each
//! box belongs to, whether it shows, and how big it is. The writer uses the
//! same scan to find the box it has to change.

use crate::coordinate::CellRef;
use crate::coordinate::{Col, Row};

/// One comment box: a `<v:shape>` whose client data says `Note`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NoteShape {
    /// Where the element lies in the part.
    pub span: core::ops::Range<usize>,
    /// The cell it belongs to, when `<x:Row>` and `<x:Column>` read.
    pub cell: Option<CellRef>,
    /// Shown all the time rather than on hover: `<x:Visible/>`.
    pub visible: bool,
    /// Width and height in points, from the shape's `style`.
    pub size: Option<(f64, f64)>,
    /// `<x:Anchor>`: left column, its offset in pixels, top row, its offset,
    /// then the same four for the bottom-right corner.
    pub anchor: Option<[u32; 8]>,
}

/// Every comment box in a VML part, in part order.
///
/// ponytail: looks for the `v:` and `x:` prefixes Excel writes; a part that
/// binds VML to other prefixes reads as having no boxes.
pub(crate) fn note_shapes(xml: &str) -> Vec<NoteShape> {
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
            let number = |tag: &str| -> Option<u32> { element(body, tag)?.trim().parse().ok() };
            let cell = number("Row")
                .zip(number("Column"))
                .and_then(|(r, c)| Some(CellRef::new(Col::new(c)?, Row::new(r)?)));
            let style = style_of(body).unwrap_or_default();
            let size = length(style, "width").zip(length(style, "height"));
            let anchor = element(body, "Anchor").and_then(|text| {
                let numbers: Vec<u32> = text
                    .split(',')
                    .filter_map(|n| n.trim().parse().ok())
                    .collect();
                numbers.try_into().ok()
            });
            out.push(NoteShape {
                span: start..end,
                cell,
                visible: body.contains("<x:Visible"),
                size,
                anchor,
            });
        }
        from = end;
    }
    out
}

/// The text of `<x:tag>` in a shape.
fn element<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<x:{tag}>");
    let at = body.find(&open)? + open.len();
    let len = body[at..].find('<')?;
    Some(&body[at..at + len])
}

/// The `style` attribute of the `<v:shape>` element itself. Excel 2007 and
/// later quote it with `"`, older versions with `'`, across several lines.
pub(crate) fn style_of(body: &str) -> Option<&str> {
    let head = &body[..body.find('>')?];
    let at = head.find(" style=")? + " style=".len();
    let quote = head[at..]
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '\''))?;
    let at = at + 1;
    let len = head[at..].find(quote)?;
    Some(&head[at..at + len])
}

/// One length of a VML style, in points. Excel writes points; pixels are
/// taken at 96 dpi.
fn length(style: &str, name: &str) -> Option<f64> {
    let value = style
        .split(';')
        .filter_map(|d| d.split_once(':'))
        .find(|(k, _)| k.trim() == name)?
        .1
        .trim();
    if let Some(points) = value.strip_suffix("pt") {
        points.parse().ok()
    } else if let Some(pixels) = value.strip_suffix("px") {
        pixels.parse::<f64>().ok().map(|px| px * 0.75)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::note_shapes;

    #[test]
    fn a_box_reads_its_cell_visibility_size_and_anchor() {
        let xml = concat!(
            r##"<xml><v:shapetype id="_x0000_t202"/><v:shape id="_x0000_s1025" type="#_x0000_t202" "##,
            "style='position:absolute;\n  margin-left:59.25pt;width:144px;height:59.25pt;visibility:visible'>",
            "<x:ClientData ObjectType=\"Note\"><x:Anchor>\n    2, 15, 0, 2, 4, 15, 4, 16</x:Anchor>",
            r#"<x:Row>1</x:Row><x:Column>1</x:Column><x:Visible/></x:ClientData></v:shape></xml>"#,
        );
        let shapes = note_shapes(xml);
        assert_eq!(shapes.len(), 1);
        let shape = &shapes[0];
        assert_eq!(shape.cell.map(|c| c.to_string()).as_deref(), Some("B2"));
        assert!(shape.visible);
        assert_eq!(shape.size, Some((108.0, 59.25)));
        assert_eq!(shape.anchor, Some([2, 15, 0, 2, 4, 15, 4, 16]));
    }
}
