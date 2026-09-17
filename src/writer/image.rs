//! Writing pictures: an untouched one as the bytes it came in, a changed or
//! new one spliced into its drawing.
//!
//! Like [`super::chart`], this hands the xlsx writer a workbook whose carried
//! parts already say the right thing. The picture element is never rebuilt
//! when it can be kept: what the model does not name - cropping, effects, the
//! SVG beside a PNG fallback - lives in it. A move rewrites the anchor around
//! the element, a rename rewrites two attributes, and new bytes go into a new
//! media part the element is pointed at.

use super::chart::{
    DRAWING_NS, DRAWING_TYPE, MAIN_NS, REL_NS, XML_DECL, drawings, edit_relationships,
    max_object_id, part_text, relationships, render_anchor, set_part,
};
use super::xmlesc::escape;
use crate::model::image::{Image, ImageFormat};
use crate::model::{Attachment, Spreadsheet, Worksheet};
use crate::reader::chart::children;
use crate::reader::image::{Picture, PictureObject, scan_pictures};
use core::ops::Range;
use std::borrow::Cow;

/// The workbook as the writer should see it, pictures applied.
pub(super) fn prepare(book: Cow<'_, Spreadsheet>) -> Cow<'_, Spreadsheet> {
    let dirty: Vec<usize> = book
        .sheets()
        .iter()
        .enumerate()
        .filter(|(_, sheet)| {
            sheet.images.iter().any(|i| !i.is_unchanged())
                || drawings(sheet).any(|d| lost_pictures(&book, sheet, d))
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

/// Whether a drawing holds a picture the sheet no longer has.
fn lost_pictures(book: &Spreadsheet, sheet: &Worksheet, drawing: &str) -> bool {
    let Some(xml) = part_text(book, drawing) else {
        return false;
    };
    if !xml.contains("pic") {
        return false;
    }
    let rels = relationships(book, drawing);
    scan_pictures(xml)
        .iter()
        .flat_map(|o| &o.pictures)
        .any(|p| is_modelled(book, &rels, p) && claimant(&sheet.images, drawing, p.id).is_none())
}

/// Whether the reader turned a picture into the model. One it could not read,
/// linked to an outside file or of a format it does not know, never reached
/// the sheet, so its absence there is no removal.
fn is_modelled(book: &Spreadsheet, rels: &[super::chart::Relationship], picture: &Picture) -> bool {
    let Some(rel) = picture
        .rel
        .as_ref()
        .and_then(|id| rels.iter().find(|r| r.id == *id))
    else {
        return false;
    };
    book.parts
        .iter()
        .find(|p| p.path == rel.target)
        .is_some_and(|p| {
            ImageFormat::sniff(&p.data)
                .or_else(|| ImageFormat::from_extension(&p.path))
                .is_some()
        })
}

fn claimant(images: &[Image], drawing: &str, id: u32) -> Option<usize> {
    images.iter().position(|i| {
        i.origin
            .as_ref()
            .is_some_and(|o| o.drawing == drawing && o.id == id)
    })
}

fn apply_sheet(book: &mut Spreadsheet, index: usize) {
    let Some(sheet) = book.sheet(index) else {
        return;
    };
    let images = sheet.images.clone();
    let mut paths: Vec<String> = drawings(sheet).map(str::to_owned).collect();
    if paths.is_empty() {
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
    // The first drawing takes the new pictures.
    for (i, path) in paths.iter().enumerate() {
        rewrite_drawing(book, path, &images, i == 0);
    }
}

/// A part path `{stem}{n}.{extension}` no part uses yet.
pub(super) fn free_path(book: &Spreadsheet, stem: &str, extension: &str) -> String {
    (1..=u32::MAX)
        .map(|n| format!("{stem}{n}.{extension}"))
        .find(|p| !book.parts.iter().any(|part| &part.path == p))
        .unwrap_or_default()
}

/// What a rewrite of one drawing collects before splicing.
struct Edits<'a> {
    book: &'a mut Spreadsheet,
    /// Relationship ids already taken, the drawing's own and new ones.
    taken: Vec<String>,
    /// New relationships: id and media part.
    added: Vec<(String, String)>,
}

impl Edits<'_> {
    /// Adds the image's bytes as a new media part and returns the
    /// relationship id pointing at it.
    fn media(&mut self, image: &Image) -> String {
        let part = free_path(self.book, "xl/media/image", image.format.extension());
        self.book.parts.push(crate::model::OpaquePart {
            path: part.clone(),
            content_type: Some(image.format.content_type().to_owned()),
            data: image.data.clone(),
        });
        let id = (1..=u32::MAX)
            .map(|n| format!("rId{n}"))
            .find(|id| !self.taken.contains(id))
            .unwrap_or_default();
        self.taken.push(id.clone());
        self.added.push((id.clone(), part));
        id
    }
}

fn rewrite_drawing(book: &mut Spreadsheet, path: &str, images: &[Image], takes_new: bool) {
    let Some(xml) = part_text(book, path).map(str::to_owned) else {
        return;
    };
    let rels = relationships(book, path);
    let modelled: Vec<bool> = scan_pictures(&xml)
        .iter()
        .flat_map(|o| &o.pictures)
        .map(|p| is_modelled(book, &rels, p))
        .collect();
    let mut edits = Edits {
        book,
        taken: rels.iter().map(|r| r.id.clone()).collect(),
        added: Vec::new(),
    };
    let mut splices: Vec<(Range<usize>, String)> = Vec::new();
    let mut modelled = modelled.into_iter();

    for object in scan_pictures(&xml) {
        let flags: Vec<bool> = modelled.by_ref().take(object.pictures.len()).collect();
        if let Some(text) = rewrite_object(&xml, &object, &flags, path, images, &mut edits) {
            splices.push((object.span.clone(), text));
        }
    }

    if takes_new {
        let mut next_id = max_object_id(&xml);
        let mut objects = String::new();
        for image in images.iter().filter(|i| i.origin.is_none()) {
            next_id = next_id.saturating_add(1);
            let rel = edits.media(image);
            objects.push_str(&render_anchor(
                image.anchor,
                &render_picture(image, next_id, &rel),
            ));
        }
        if !objects.is_empty() {
            let close = xml.rfind("</").unwrap_or(xml.len());
            splices.push((close..close, objects));
        }
    }
    let Edits { book, added, .. } = edits;
    if splices.is_empty() && added.is_empty() {
        return;
    }
    set_part(book, path, Some(DRAWING_TYPE), splice(&xml, splices));
    edit_relationships(book, path, &[], &added, "image");
}

/// The new text of one anchored object, or `None` when it stays as it is. An
/// empty text removes the object.
fn rewrite_object(
    xml: &str,
    object: &PictureObject,
    modelled: &[bool],
    drawing: &str,
    images: &[Image],
    edits: &mut Edits<'_>,
) -> Option<String> {
    let base = object.span.start;
    let original = &xml[object.span.clone()];
    let alone = matches!(object.pictures.as_slice(), [p] if !p.grouped);
    let mut splices: Vec<(Range<usize>, String)> = Vec::new();
    let mut moved_to = None;

    for (picture, &modelled) in object.pictures.iter().zip(modelled) {
        let local = picture.span.start - base..picture.span.end - base;
        let Some(index) = claimant(images, drawing, picture.id) else {
            if modelled {
                if alone {
                    return Some(String::new());
                }
                splices.push((local, String::new()));
            }
            continue;
        };
        let image = &images[index];
        let Some(origin) = &image.origin else {
            continue;
        };
        let mut text = original[local.clone()].to_owned();
        if image.name != origin.name || image.description != origin.description {
            text = rename(&text, &image.name, &image.description);
        }
        if origin.data_changed(&image.data) {
            let rel = edits.media(image);
            text = repoint(&text, &rel);
        }
        if text != original[local.clone()] {
            splices.push((local, text));
        }
        // A picture inside a group is placed by the group.
        if alone && image.anchor != origin.anchor {
            moved_to = Some(image.anchor);
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

/// The object's children other than its anchor markers, placed by a new
/// anchor.
pub(super) fn reanchor(object: &str, anchor: crate::model::chart::Anchor) -> String {
    let Some(node) = children(object).into_iter().next() else {
        return object.to_owned();
    };
    let body: String = node
        .children()
        .iter()
        .filter(|kid| !matches!(kid.name, "from" | "to" | "ext" | "pos"))
        .map(|kid| kid.outer)
        .collect();
    render_anchor(anchor, &body)
}

/// A drawing element with its `cNvPr` name and description replaced.
pub(super) fn rename(element: &str, name: &str, description: &str) -> String {
    let Some(start) = element.find("cNvPr ") else {
        return element.to_owned();
    };
    let Some(end) = element[start..].find('>').map(|e| start + e) else {
        return element.to_owned();
    };
    let tag = &element[start..end];
    let tag = set_attribute(tag, "name", name);
    let tag = set_attribute(&tag, "descr", description);
    format!("{}{tag}{}", &element[..start], &element[end..])
}

/// A tag with one attribute set, added before the tag closes if absent.
pub(super) fn set_attribute(tag: &str, name: &str, value: &str) -> String {
    let key = format!(" {name}=\"");
    let value = escape(value);
    if let Some(at) = tag.find(&key) {
        let start = at + key.len();
        let end = tag[start..].find('"').map_or(tag.len(), |e| start + e);
        return format!("{}{value}{}", &tag[..start], &tag[end..]);
    }
    let close = if tag.ends_with('/') {
        tag.len() - 1
    } else {
        tag.len()
    };
    format!("{} {name}=\"{value}\"{}", &tag[..close], &tag[close..])
}

/// A picture element pointed at another media part. The blip is replaced
/// whole: an SVG beside the old bytes would show in their place.
fn repoint(picture: &str, rel: &str) -> String {
    let Some(pic) = children(picture).into_iter().next() else {
        return picture.to_owned();
    };
    let Some(fill) = pic.child("blipFill") else {
        return picture.to_owned();
    };
    let Some(blip) = fill.child("blip") else {
        return picture.to_owned();
    };
    let start = pic.inner_start + fill.inner_start + blip.span.start;
    let end = pic.inner_start + fill.inner_start + blip.span.end;
    let prefix = if blip.prefix.is_empty() {
        String::new()
    } else {
        format!("{}:", blip.prefix)
    };
    format!(
        r#"{}<{prefix}blip xmlns:r="{REL_NS}" r:embed="{}"/>{}"#,
        &picture[..start],
        escape(rel),
        &picture[end..]
    )
}

/// A picture element for a picture made in code.
fn render_picture(image: &Image, id: u32, rel: &str) -> String {
    let name = if image.name.is_empty() {
        format!("Picture {id}")
    } else {
        image.name.clone()
    };
    format!(
        concat!(
            r#"<xdr:pic><xdr:nvPicPr><xdr:cNvPr id="{id}" name="{name}" descr="{descr}"/>"#,
            r#"<xdr:cNvPicPr><a:picLocks noChangeAspect="1"/></xdr:cNvPicPr></xdr:nvPicPr>"#,
            r#"<xdr:blipFill><a:blip r:embed="{rel}"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill>"#,
            r#"<xdr:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></a:xfrm>"#,
            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic><xdr:clientData/>"#
        ),
        id = id,
        name = escape(&name),
        descr = escape(&image.description),
        rel = escape(rel),
    )
}

/// Text with stretches replaced, in order; a stretch overlapping one already
/// replaced is skipped.
pub(super) fn splice(text: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (span, replacement) in edits {
        if span.start < at {
            continue;
        }
        out.push_str(&text[at..span.start]);
        out.push_str(&replacement);
        at = span.end;
    }
    out.push_str(&text[at..]);
    out
}
