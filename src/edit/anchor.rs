//! Where a drawing sits, when the grid under it moves.
//!
//! Pictures, shapes, charts and the boxes behind comments are anchored to a
//! cell: a drawing part says "row 12, column 3", and Excel puts the object
//! there. None of that is modelled here - the parts travel byte for byte - so
//! a row inserted above an object used to move the data and leave the object
//! on the row it named, which is the worst kind of error, the kind only a
//! person looking at the sheet can see.
//!
//! What this does instead of modelling drawings: rewrite the anchor numbers
//! inside the carried bytes and leave every other byte alone. Two dialects
//! carry them, and both are small enough to find by their element names:
//!
//! * `xl/drawings/drawingN.xml`, where `<xdr:from>` and `<xdr:to>` hold
//!   `<xdr:col>` and `<xdr:row>`;
//! * `xl/drawings/vmlDrawingN.vml`, where a comment's box carries `<x:Row>`
//!   and `<x:Column>`.
//!
//! Both are 0-based, as the grid is here.

use super::{Axis, Shift};
use crate::coordinate::{Col, Row};
use crate::model::Spreadsheet;
use crate::model::chart::{Anchor, EditAs, Marker};

/// Moves every drawing anchored to the sheet, so that an object keeps the
/// cells it was put on.
pub(super) fn move_anchors(book: &mut Spreadsheet, sheet: usize, shift: Shift) {
    let Some(target) = book.sheet(sheet) else {
        return;
    };
    let parts: Vec<String> = target
        .attachments
        .iter()
        .filter(|a| matches!(a.role(), "drawing" | "vmlDrawing"))
        .map(|a| a.target.clone())
        .collect();
    for part in &mut book.parts {
        if !parts.iter().any(|p| p == &part.path) {
            continue;
        }
        let Ok(text) = core::str::from_utf8(&part.data) else {
            // A drawing part is XML; anything else is not ours to touch.
            continue;
        };
        part.data = rewrite(text, shift).into_bytes();
    }
}

/// The same move for an anchor held in the model, by the same rules as
/// [`rewrite`] applies to the bytes.
pub(super) fn move_anchor(anchor: &mut Anchor, shift: Shift) {
    let (from, to, resizes) = match anchor {
        Anchor::TwoCell {
            edit_as: Some(EditAs::Absolute),
            ..
        }
        | Anchor::Absolute { .. } => return,
        Anchor::TwoCell { from, to, edit_as } => {
            (from, Some(to), *edit_as != Some(EditAs::OneCell))
        }
        Anchor::OneCell { from, .. } => (from, None, true),
    };
    let index = |m: &Marker| match shift.axis {
        Axis::Rows => m.row.index(),
        Axis::Columns => m.col.index(),
    };
    let set = |m: &mut Marker, i: u32| match shift.axis {
        Axis::Rows => m.row = Row::new(i).unwrap_or(m.row),
        Axis::Columns => m.col = Col::new(i).unwrap_or(m.col),
    };
    let start = index(from);
    let moved = shift.moved_edge(start, true).unwrap_or(start);
    set(from, moved);
    if let Some(to) = to {
        let end = index(to);
        let moved_end = if resizes {
            shift.moved_edge(end, false)
        } else {
            // The trailing corner of an object that keeps its size moves by
            // exactly what the leading corner moved.
            u32::try_from(i64::from(end) + i64::from(moved) - i64::from(start))
                .ok()
                .filter(|&i| i <= shift.ceiling())
        };
        set(to, moved_end.unwrap_or(end));
    }
}

/// The element names carrying an index on this axis, in both dialects.
fn tags(axis: Axis) -> [&'static str; 2] {
    match axis {
        Axis::Rows => ["xdr:row", "x:Row"],
        Axis::Columns => ["xdr:col", "x:Column"],
    }
}

/// Rewrites one drawing part.
///
/// `editAs` decides what an insertion inside an object does to it: the default
/// `twoCell` moves and resizes it, so each corner follows its own cell, while
/// `oneCell` keeps the size and moves the whole object by whatever its top
/// left corner moved. `absolute` pins the object to the page and is left
/// alone.
fn rewrite(xml: &str, shift: Shift) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    // How far the anchor being read has moved its first corner, for the two
    // modes that size the object with the grid.
    let mut lead: Option<i64> = None;
    let mut absolute = false;
    let mut resizes = true;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..close];
        let name = tag.split([' ', '/', '\t', '\n']).next().unwrap_or(tag);
        match name {
            "xdr:twoCellAnchor" | "xdr:oneCellAnchor" | "xdr:absoluteAnchor" => {
                lead = None;
                absolute = name == "xdr:absoluteAnchor" || tag.contains(r#"editAs="absolute""#);
                resizes = !tag.contains(r#"editAs="oneCell""#);
            }
            "xdr:from" | "x:ClientData" => lead = None,
            _ => {}
        }
        out.push_str(&rest[..=close]);
        rest = &rest[close + 1..];

        // A tag that holds an index is followed by that index and its closing
        // tag, and by nothing else.
        if absolute || !tags(shift.axis).contains(&name) {
            continue;
        }
        let Some(end) = rest.find('<') else {
            break;
        };
        let Ok(index) = rest[..end].trim().parse::<u32>() else {
            continue;
        };
        // The trailing corner of an object that keeps its size moves by
        // exactly what the leading corner moved, however many rows were
        // pushed in between them.
        let moved = match (lead, resizes) {
            (Some(delta), false) => u32::try_from(i64::from(index) + delta)
                .ok()
                .filter(|&i| i <= shift.ceiling()),
            _ => shift.moved_edge(index, lead.is_none()),
        };
        let moved = moved.unwrap_or(index);
        if lead.is_none() {
            lead = Some(i64::from(moved) - i64::from(index));
        }
        out.push_str(&moved.to_string());
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{Axis, Shift};

    const DRAWING: &str = concat!(
        r#"<xdr:wsDr><xdr:twoCellAnchor>"#,
        r#"<xdr:from><xdr:col>1</xdr:col><xdr:colOff>7</xdr:colOff>"#,
        r#"<xdr:row>4</xdr:row><xdr:rowOff>9</xdr:rowOff></xdr:from>"#,
        r#"<xdr:to><xdr:col>3</xdr:col><xdr:colOff>7</xdr:colOff>"#,
        r#"<xdr:row>8</xdr:row><xdr:rowOff>9</xdr:rowOff></xdr:to>"#,
        r#"</xdr:twoCellAnchor></xdr:wsDr>"#,
    );

    fn rows(xml: &str) -> Vec<u32> {
        xml.split("<xdr:row>")
            .skip(1)
            .filter_map(|s| s.split('<').next()?.parse().ok())
            .collect()
    }

    #[test]
    fn an_insert_above_moves_the_whole_object() {
        let out = rewrite(DRAWING, Shift::insert(Axis::Rows, 0, 3));
        assert_eq!(rows(&out), [7, 11]);
        // The offsets inside the cell are not indexes and must not move.
        assert!(out.contains("<xdr:rowOff>9</xdr:rowOff>"), "{out}");
    }

    /// An insert between the corners stretches an object that sizes with the
    /// grid, which is what `twoCellAnchor` means by default.
    #[test]
    fn an_insert_inside_stretches_the_default_anchor() {
        let out = rewrite(DRAWING, Shift::insert(Axis::Rows, 6, 2));
        assert_eq!(rows(&out), [4, 10]);
    }

    /// With `editAs="oneCell"` the object keeps its size, so the far corner
    /// moves by whatever the near one did - here, nothing.
    #[test]
    fn an_object_that_keeps_its_size_does_not_stretch() {
        let pinned = DRAWING.replace(
            "<xdr:twoCellAnchor>",
            r#"<xdr:twoCellAnchor editAs="oneCell">"#,
        );
        let out = rewrite(&pinned, Shift::insert(Axis::Rows, 6, 2));
        assert_eq!(rows(&out), [4, 8]);

        let out = rewrite(&pinned, Shift::insert(Axis::Rows, 0, 3));
        assert_eq!(rows(&out), [7, 11]);
    }

    /// An object pinned to the page ignores the grid entirely.
    #[test]
    fn an_absolute_anchor_stays_put() {
        let pinned = DRAWING.replace(
            "<xdr:twoCellAnchor>",
            r#"<xdr:twoCellAnchor editAs="absolute">"#,
        );
        let out = rewrite(&pinned, Shift::insert(Axis::Rows, 0, 3));
        assert_eq!(rows(&out), [4, 8]);
    }

    /// A removal that swallows a corner pulls it onto the edit point rather
    /// than leaving it pointing into the gap.
    #[test]
    fn a_removal_pulls_the_corner_in() {
        let out = rewrite(DRAWING, Shift::remove(Axis::Rows, 3, 3));
        assert_eq!(rows(&out), [3, 5]);
    }

    /// The other dialect: a comment's box says the same thing in VML.
    #[test]
    fn a_comment_box_moves_too() {
        let vml = r#"<x:ClientData ObjectType="Note"><x:Anchor>10,12,12,88</x:Anchor><x:Row>13</x:Row><x:Column>3</x:Column></x:ClientData>"#;
        let out = rewrite(vml, Shift::insert(Axis::Rows, 2, 5));
        assert!(out.contains("<x:Row>18</x:Row>"), "{out}");
        // The pixel offsets in `<x:Anchor>` are not grid indexes.
        assert!(out.contains("<x:Anchor>10,12,12,88</x:Anchor>"), "{out}");

        let out = rewrite(vml, Shift::insert(Axis::Columns, 0, 1));
        assert!(out.contains("<x:Column>4</x:Column>"), "{out}");
        assert!(out.contains("<x:Row>13</x:Row>"), "{out}");
    }
}
