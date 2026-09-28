//! Shapes drawn on a sheet: rectangles, arrows, callouts, text boxes.
//!
//! A shape is an `<xdr:sp>` element in the drawing of a sheet. The model keeps
//! what a program asks about - the name, the alt text, where it sits and how
//! it is turned, which preset outline it has, its fill and outline, the text
//! inside and the font of its first run - and leaves the rest of the element
//! as written: effects, 3-D, the formatting of later runs.
//!
//! What the element does not state it takes from its style (`<xdr:style>`):
//! a shape drawn in Excel usually says nothing of its own colours and is
//! filled with the theme's first accent. [`Shape::effective_fill`],
//! [`Shape::effective_line`] and [`Shape::effective_font`] answer with what
//! Excel shows, the style applied.
//!
//! Writing follows pictures: a shape the program did not touch goes back byte
//! for byte, a renamed one gets two attributes rewritten, a moved one a new
//! anchor around the same element, a turned one the attributes of `a:xfrm`,
//! a new fill or outline the children of `xdr:spPr` that say them, new text or
//! a new font a new text body that keeps the first run's formatting, and a
//! removed one is cut from the drawing.
//! Connectors (`<xdr:cxnSp>`) are not shapes here and stay in the drawing.

use crate::model::chart::{Anchor, ChartColor, Fill, LineFormat, ShapeFormat};
use crate::style::{Color, DiffFont};

/// A shape on a sheet.
///
/// A new field joins this struct when the model learns more of the element,
/// so build one with [`Shape::new`] rather than a literal.
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
    ///
    /// For a turned shape this is not its frame. Excel anchors a shape turned
    /// by 45 to 135 or 225 to 315 degrees by its frame turned a quarter: the
    /// anchor's width is the shape's height and its height the shape's width,
    /// about the same centre. At other angles the anchor is the frame. The
    /// shape is drawn in its frame, then turned about the centre.
    pub anchor: Anchor,
    /// How far the shape is turned clockwise about its centre, in 60 000ths
    /// of a degree as the file states it (`a:xfrm rot`): `5400000` is a
    /// quarter turn. See [`Shape::anchor`] for how it changes the anchor.
    pub rotation: i32,
    /// Mirrored left to right (`a:xfrm flipH`), before it is turned.
    pub flip_h: bool,
    /// Mirrored top to bottom (`a:xfrm flipV`), before it is turned.
    pub flip_v: bool,
    /// The preset outline, as the format names it: `rect`, `ellipse`,
    /// `rightArrow`, `wedgeRectCallout`. `None` for a freeform outline, which
    /// cannot be changed through here.
    pub geometry: Option<String>,
    /// The fill and outline the element states itself (`xdr:spPr`). `None`
    /// in either means the style's (see [`Shape::effective_fill`]); `source`
    /// is the element as read, whose effects and outline survive a change.
    pub format: ShapeFormat,
    /// The text inside, a line per paragraph; empty for a shape without text.
    pub text: String,
    /// The font of the text as its first run states it (`a:rPr`): family
    /// (`a:latin`), size, bold, italic and colour. What it leaves out comes
    /// from the style and the theme; [`Shape::effective_font`] fills in the
    /// colour. A change is written to every run of a new text body.
    pub font: DiffFont,
    /// Where it was read from; `None` for a shape made in code.
    pub origin: Option<ShapeOrigin>,
}

/// What a shape's style (`<xdr:style>`) gives what the element leaves out.
///
/// Read only: to change a colour, set it in [`Shape::format`], which wins.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ShapeStyle {
    /// `a:fillRef`: the theme's fill style number, `0` for none, and the
    /// colour it is drawn in.
    pub fill: Option<(u32, Option<ChartColor>)>,
    /// `a:lnRef`, the same for the outline.
    pub line: Option<(u32, Option<ChartColor>)>,
    /// `a:fontRef`: the colour of the text.
    pub font: Option<Color>,
}

impl ShapeStyle {
    /// What Excel gives a shape drawn with it: the first accent, outlined in
    /// a darker shade of it, with light text.
    pub(crate) fn office() -> Self {
        let accent = || ChartColor::scheme("accent1");
        let mut dark = accent();
        dark.transforms
            .push(crate::model::chart::ColorTransform::Shade(50_000));
        Self {
            fill: Some((1, Some(accent()))),
            line: Some((2, Some(dark))),
            font: Some(Color::Theme { id: 0, tint: 0 }),
        }
    }
}

/// A style reference as the fill it stands for.
///
/// ponytail: any style but `0` is taken for a solid fill in the reference's
/// colour; the theme's second and third fill styles may be gradients. Read
/// `fmtScheme` of the theme if a program has to draw those.
fn style_fill(reference: Option<&(u32, Option<ChartColor>)>) -> Fill {
    match reference {
        Some((index, color)) if *index > 0 => color.clone().map_or(Fill::Other, Fill::Solid),
        _ => Fill::None,
    }
}

impl Shape {
    /// A new shape with a preset outline, `rect` for a plain box.
    #[must_use]
    pub fn new(geometry: impl Into<String>, anchor: Anchor) -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            anchor,
            rotation: 0,
            flip_h: false,
            flip_v: false,
            geometry: Some(geometry.into()),
            format: ShapeFormat::default(),
            text: String::new(),
            font: DiffFont::default(),
            origin: None,
        }
    }

    /// The style behind what the element leaves out: the one read, or for a
    /// shape made in code the one it is written with.
    fn style(&self) -> std::borrow::Cow<'_, ShapeStyle> {
        self.origin.as_ref().map_or_else(
            || std::borrow::Cow::Owned(ShapeStyle::office()),
            |o| std::borrow::Cow::Borrowed(&o.style),
        )
    }

    /// The fill Excel shows: the element's own, else the style's.
    /// [`Fill::None`] when the shape is not filled. A theme colour is left as
    /// a name; [`ChartColor::resolve`] turns it into RGB.
    #[must_use]
    pub fn effective_fill(&self) -> Fill {
        self.format
            .fill
            .clone()
            .unwrap_or_else(|| style_fill(self.style().fill.as_ref()))
    }

    /// The outline Excel shows: the element's own colour and width, else the
    /// style's. `fill` is always set, [`Fill::None`] for no outline. A `width`
    /// of `None` is the theme's for the style, 12 700 EMU (one point) in
    /// Office's themes since 2013.
    #[must_use]
    pub fn effective_line(&self) -> LineFormat {
        let own = self.format.line.clone().unwrap_or_default();
        LineFormat {
            fill: Some(
                own.fill
                    .unwrap_or_else(|| style_fill(self.style().line.as_ref())),
            ),
            width: own.width,
        }
    }

    /// The font of the text with the style's colour where the run states
    /// none. What is still `None` is the theme's: its minor font, 11 points,
    /// regular.
    #[must_use]
    pub fn effective_font(&self) -> DiffFont {
        let mut font = self.font.clone();
        if font.color.is_none() {
            font.color.clone_from(&self.style().font);
        }
        font
    }
    /// Whether the shape still says what it said when it was read.
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        self.origin.as_ref().is_some_and(|o| {
            o.name == self.name
                && o.description == self.description
                && o.anchor == self.anchor
                && o.rotation == self.rotation
                && o.flip_h == self.flip_h
                && o.flip_v == self.flip_v
                && o.geometry == self.geometry
                && o.format == self.format
                && o.text == self.text
                && o.font == self.font
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
    pub(crate) rotation: i32,
    pub(crate) flip_h: bool,
    pub(crate) flip_v: bool,
    pub(crate) geometry: Option<String>,
    pub(crate) format: ShapeFormat,
    pub(crate) text: String,
    pub(crate) font: DiffFont,
    /// The style the element names, which is not the program's to change.
    pub(crate) style: ShapeStyle,
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
