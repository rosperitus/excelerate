//! Shapes drawn on a sheet: rectangles, arrows, callouts, text boxes.
//!
//! A shape is an `<xdr:sp>` element in the drawing of a sheet. The model keeps
//! what a program asks about - the name, the alt text, where it sits, which
//! preset outline it has and the text inside - and leaves the rest of the
//! element as written: fill, line, effects, the formatting of the text.
//!
//! Writing follows pictures: a shape the program did not touch goes back byte
//! for byte, a renamed one gets two attributes rewritten, a moved one a new
//! anchor around the same element, new text a new text body that keeps the
//! first run's formatting, and a removed one is cut from the drawing.
//! Connectors (`<xdr:cxnSp>`) are not shapes here and stay in the drawing.

use crate::model::chart::Anchor;

/// A shape on a sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    /// The name the drawing gives it, which the selection pane shows.
    pub name: String,
    /// The alternative text a screen reader says for it.
    pub description: String,
    /// Where it sits.
    ///
    /// A shape inside a group reports the group's anchor, and moving it is
    /// not written: the group positions its members.
    pub anchor: Anchor,
    /// The preset outline, as the format names it: `rect`, `ellipse`,
    /// `rightArrow`, `wedgeRectCallout`. `None` for a freeform outline, which
    /// cannot be changed through here.
    pub geometry: Option<String>,
    /// The text inside, a line per paragraph; empty for a shape without text.
    pub text: String,
    /// Where it was read from; `None` for a shape made in code.
    pub origin: Option<ShapeOrigin>,
}

impl Shape {
    /// A new shape with a preset outline, `rect` for a plain box.
    #[must_use]
    pub fn new(geometry: impl Into<String>, anchor: Anchor) -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            anchor,
            geometry: Some(geometry.into()),
            text: String::new(),
            origin: None,
        }
    }

    /// Whether the shape still says what it said when it was read.
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        self.origin.as_ref().is_some_and(|o| {
            o.name == self.name
                && o.description == self.description
                && o.anchor == self.anchor
                && o.geometry == self.geometry
                && o.text == self.text
        })
    }

    /// Takes the shape as it stands for what was read, for an edit that moved
    /// the bytes of the drawing and the model the same way.
    pub(crate) fn settle(&mut self) {
        if let Some(origin) = &mut self.origin {
            origin.anchor = self.anchor;
        }
    }
}

/// Where a shape came from, so an untouched one goes back as it was.
///
/// Opaque on purpose: nothing in it is a property of the shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeOrigin {
    /// The drawing part holding the element.
    pub(crate) drawing: String,
    /// The drawing object id, unique within the drawing.
    pub(crate) id: u32,
    /// Whether the element sits inside a group of shapes.
    pub(crate) grouped: bool,
    /// What was read, to compare with.
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) anchor: Anchor,
    pub(crate) geometry: Option<String>,
    pub(crate) text: String,
    /// How many shapes were read from the same drawing. A writer that finds
    /// fewer in the model knows one was removed without parsing the drawing
    /// again, which on a book of two thousand shapes doubled the cost of
    /// writing it.
    pub(crate) read_from_drawing: usize,
}

impl ShapeOrigin {
    /// The drawing part the shape lives in.
    #[must_use]
    pub fn drawing(&self) -> &str {
        &self.drawing
    }

    /// Whether the shape sits inside a group, which places it: moving such a
    /// shape through [`Shape::anchor`] is not written.
    #[must_use]
    pub const fn grouped(&self) -> bool {
        self.grouped
    }
}
