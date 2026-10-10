//! The comment boxes in a sheet's VML part.
//!
//! The part itself travels as bytes; what is read out of it is which cell each
//! box belongs to, whether it shows, and how big it is. The writer uses the
//! same scan to find the box it has to change.

use crate::coordinate::CellRef;
use crate::coordinate::{Col, Row};
use crate::model::chart::{Anchor, Marker};
use crate::model::control::{CheckState, ControlKind, FormControl, ScrollValues};
use crate::reader::html::unescape;

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
    for (span, body) in shapes(xml) {
        if body.contains("ObjectType=\"Note\"") {
            let number = |tag: &str| -> Option<u32> { element(body, tag)?.trim().parse().ok() };
            let cell = number("Row")
                .zip(number("Column"))
                .and_then(|(r, c)| Some(CellRef::new(Col::new(c)?, Row::new(r)?)));
            let style = style_of(body).unwrap_or_default();
            let size = length(style, "width").zip(length(style, "height"));
            out.push(NoteShape {
                span,
                cell,
                visible: body.contains("<x:Visible"),
                size,
                anchor: anchor(body),
            });
        }
    }
    out
}

/// Every `<v:shape>` element in a part: where it lies and its text.
fn shapes(xml: &str) -> Vec<(core::ops::Range<usize>, &str)> {
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
        out.push((start..end, &xml[start..end]));
        from = end;
    }
    out
}

/// `<x:Anchor>`: eight numbers, or nothing.
fn anchor(body: &str) -> Option<[u32; 8]> {
    let numbers: Vec<u32> = element(body, "Anchor")?
        .split(',')
        .filter_map(|n| n.trim().parse().ok())
        .collect();
    numbers.try_into().ok()
}

/// Every form control in a VML part, in part order.
///
/// VML holds all of a control: Excel 2007 wrote nothing else, and later
/// versions repeat every property of the `ctrlProp` part here, so that part
/// and the sheet's `<controls>` are not read.
pub(crate) fn controls(xml: &str) -> Vec<FormControl> {
    let mut out = Vec::new();
    for (_, body) in shapes(xml) {
        let Some(kind) = attribute(body, "ObjectType").and_then(ControlKind::from_vml) else {
            continue;
        };
        let Some([c1, dx1, r1, dy1, c2, dx2, r2, dy2]) = anchor(body) else {
            continue;
        };
        // VML offsets are pixels.
        let marker = |col: u32, dx: u32, row: u32, dy: u32| -> Option<Marker> {
            Some(Marker {
                col: Col::new(col)?,
                col_offset: i64::from(dx) * 9525,
                row: Row::new(row)?,
                row_offset: i64::from(dy) * 9525,
            })
        };
        let (Some(from), Some(to)) = (marker(c1, dx1, r1, dy1), marker(c2, dx2, r2, dy2)) else {
            continue;
        };
        let mut control = FormControl::new(
            kind,
            Anchor::TwoCell {
                from,
                to,
                edit_as: None,
            },
        );
        let text = |tag: &str| element(body, tag).map(|t| unescape(t.trim()));
        control.linked_cell = text("FmlaLink");
        control.input_range = text("FmlaRange");
        control.macro_name = text("FmlaMacro");
        control.text = textbox(body);
        let number = |tag: &str| element(body, tag).and_then(|t| t.trim().parse::<i32>().ok());
        if matches!(kind, ControlKind::CheckBox | ControlKind::OptionButton) {
            let state = number("Checked").and_then(|n| u32::try_from(n).ok());
            control.checked = Some(CheckState::from_number(state.unwrap_or(0)));
        }
        if matches!(kind, ControlKind::Spinner | ControlKind::ScrollBar) {
            // The defaults Excel leaves out.
            control.scroll = Some(ScrollValues {
                value: number("Val").unwrap_or(0),
                min: number("Min").unwrap_or(0),
                max: number("Max").unwrap_or(100),
                step: number("Inc").unwrap_or(1),
                page: number("Page").unwrap_or(10),
            });
        }
        out.push(control);
    }
    out
}

/// The text of `<v:textbox>`, markup dropped; a `<div>` or `<br/>` starts a
/// line.
fn textbox(body: &str) -> Option<String> {
    let at = body.find("<v:textbox")?;
    let inner = &body[at..];
    let inner = &inner[inner.find('>')? + 1..inner.find("</v:textbox>")?];
    let mut out = String::new();
    let mut rest = inner;
    while let Some(open) = rest.find('<') {
        out.push_str(&unescape(&rest[..open]));
        let close = rest[open..].find('>').map_or(rest.len(), |c| open + c + 1);
        let tag = &rest[open..close];
        if (tag.starts_with("<br") || tag.starts_with("<div")) && !out.is_empty() {
            out.push('\n');
        }
        rest = &rest[close..];
    }
    out.push_str(&unescape(rest));
    let out = out.trim().to_owned();
    (!out.is_empty()).then_some(out)
}

/// The value of an attribute somewhere in a shape, double-quoted.
fn attribute<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let open = format!(" {name}=\"");
    let at = body.find(&open)? + open.len();
    let len = body[at..].find('"')?;
    Some(&body[at..at + len])
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

    #[test]
    fn form_controls_read_their_kind_caption_link_and_state() {
        use super::controls;
        use crate::model::control::{CheckState, ControlKind, ScrollValues};
        let xml = concat!(
            r#"<xml><v:shape id="_x0000_s1025" style='position:absolute'>"#,
            r#"<v:textbox><div style='text-align:center'><font face="Calibri">Run &amp; go</font></div></v:textbox>"#,
            r#"<x:ClientData ObjectType="Button"><x:Anchor>1, 2, 3, 4, 5, 6, 7, 8</x:Anchor>"#,
            r#"<x:FmlaMacro>[0]!Macro1</x:FmlaMacro></x:ClientData></v:shape>"#,
            r#"<v:shape id="_x0000_s1026"><v:textbox><div>Tick</div></v:textbox>"#,
            r#"<x:ClientData ObjectType="Checkbox"><x:Anchor>0, 0, 0, 0, 1, 0, 1, 0</x:Anchor>"#,
            r#"<x:Checked>1</x:Checked><x:FmlaLink>$A$1</x:FmlaLink></x:ClientData></v:shape>"#,
            r#"<v:shape id="_x0000_s1027"><x:ClientData ObjectType="Spin"><x:Anchor>0, 0, 0, 0, 1, 0, 1, 0</x:Anchor>"#,
            r#"<x:Val>5</x:Val><x:Max>30</x:Max></x:ClientData></v:shape>"#,
            r#"<v:shape id="_x0000_s1028"><x:ClientData ObjectType="Note"><x:Anchor>0, 0, 0, 0, 1, 0, 1, 0</x:Anchor>"#,
            r#"</x:ClientData></v:shape></xml>"#,
        );
        let found = controls(xml);
        assert_eq!(found.len(), 3, "a note is not a control");
        assert_eq!(found[0].kind, ControlKind::Button);
        assert_eq!(found[0].text.as_deref(), Some("Run & go"));
        assert_eq!(found[0].macro_name.as_deref(), Some("[0]!Macro1"));
        assert_eq!(found[1].checked, Some(CheckState::Checked));
        assert_eq!(found[1].linked_cell.as_deref(), Some("$A$1"));
        assert_eq!(
            found[2].scroll,
            Some(ScrollValues {
                value: 5,
                min: 0,
                max: 30,
                step: 1,
                page: 10
            })
        );
    }
}
