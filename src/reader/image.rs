//! Reading pictures: the `<xdr:pic>` elements of a drawing.
//!
//! See [`crate::model::image`] for what is modelled.

use super::chart::{Node, children, read_anchor};
use crate::model::chart::Anchor;
use core::ops::Range;

/// One picture element.
#[derive(Debug)]
pub(crate) struct Picture {
    /// The drawing object id.
    pub id: u32,
    pub name: String,
    pub description: String,
    /// The relationship of the embedded image; `None` for a picture that
    /// links to a file outside the package.
    pub rel: Option<String>,
    /// Whether it sits inside a group of shapes.
    pub grouped: bool,
    /// Where the element sits in the drawing part.
    #[cfg_attr(
        not(feature = "write"),
        expect(dead_code, reason = "only a rewrite of the drawing splices by it")
    )]
    pub span: Range<usize>,
}

/// One top-level object of a drawing that holds at least one picture.
#[derive(Debug)]
pub(crate) struct PictureObject {
    /// Where the object's element sits in the drawing part.
    #[cfg_attr(
        not(feature = "write"),
        expect(dead_code, reason = "only a rewrite of the drawing splices by it")
    )]
    pub span: Range<usize>,
    pub anchor: Option<Anchor>,
    /// The pictures in it: one for a picture on its own, any number for a
    /// group.
    pub pictures: Vec<Picture>,
}

/// The objects of a drawing part that hold pictures.
pub(crate) fn scan_pictures(xml: &str) -> Vec<PictureObject> {
    let mut objects = Vec::new();
    let Some(root) = children(xml).into_iter().find(|n| n.name == "wsDr") else {
        return objects;
    };
    for child in root.children() {
        if !child.inner.contains("pic") {
            continue;
        }
        let kids = child.children();
        let mut pictures = Vec::new();
        find_pictures(
            &kids,
            root.inner_start + child.inner_start,
            false,
            &mut pictures,
        );
        if pictures.is_empty() {
            continue;
        }
        objects.push(PictureObject {
            span: root.inner_start + child.span.start..root.inner_start + child.span.end,
            anchor: read_anchor(&child, &kids),
            pictures,
        });
    }
    objects
}

/// Collects the pictures among `kids`, looking into groups and into the
/// preferred branch of `mc:AlternateContent`. `base` is where the text the
/// nodes were read from starts in the drawing part.
fn find_pictures(kids: &[Node<'_>], base: usize, grouped: bool, out: &mut Vec<Picture>) {
    for child in kids {
        let inner = base + child.inner_start;
        match child.name {
            "grpSp" => find_pictures(&child.children(), inner, true, out),
            "AlternateContent" => {
                if let Some(choice) = child.child("Choice") {
                    find_pictures(&choice.children(), inner + choice.inner_start, grouped, out);
                }
            }
            "pic" => {
                let props = child.child("nvPicPr").and_then(|n| n.child("cNvPr"));
                let blip = child.child("blipFill").and_then(|n| n.child("blip"));
                let attr = |name: &str| {
                    props
                        .as_ref()
                        .and_then(|p| p.attr_text(name))
                        .unwrap_or_default()
                };
                out.push(Picture {
                    id: props
                        .as_ref()
                        .and_then(|p| p.attr("id"))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    name: attr("name"),
                    description: attr("descr"),
                    rel: blip.and_then(|b| b.attr("embed").map(str::to_owned)),
                    grouped,
                    span: base + child.span.start..base + child.span.end,
                });
            }
            _ => {}
        }
    }
}
