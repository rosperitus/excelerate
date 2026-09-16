//! Pictures placed on a sheet.
//!
//! A picture is two parts, like a chart: the drawing of a sheet holds a
//! `<xdr:pic>` element - where it sits, its name and alt text, how it is
//! cropped and drawn - and that element points at a media part holding the
//! image file. The model keeps the parts a program asks about: the bytes, the
//! format, the anchor, the name and the description.
//!
//! Everything else about the picture - cropping, borders, effects, the SVG a
//! modern Excel draws in place of its PNG fallback - stays in the drawing as
//! written. A picture the program did not touch goes back byte for byte; one
//! that moved keeps its element and gets a new anchor around it; one whose
//! bytes changed gets a new media part. Which is which is decided by comparing
//! the picture with what was read, as for charts.

use crate::model::chart::Anchor;
use std::hash::{DefaultHasher, Hash, Hasher};

/// A picture on a sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// The name the drawing gives it, which the selection pane shows.
    pub name: String,
    /// The alternative text a screen reader says for it.
    pub description: String,
    /// Where it sits.
    ///
    /// A picture inside a group of shapes reports the group's anchor, and
    /// moving it is not written: the group positions its members.
    pub anchor: Anchor,
    /// What kind of file [`Image::data`] is.
    pub format: ImageFormat,
    /// The image file itself.
    pub data: Vec<u8>,
    /// Where it was read from; `None` for a picture made in code.
    pub origin: Option<ImageOrigin>,
}

impl Image {
    /// A new picture from the bytes of an image file, its format told by the
    /// bytes themselves.
    ///
    /// Returns `None` for bytes that are not one of the formats Excel shows.
    #[must_use]
    pub fn new(data: Vec<u8>, anchor: Anchor) -> Option<Self> {
        let format = ImageFormat::sniff(&data)?;
        Some(Self {
            name: String::new(),
            description: String::new(),
            anchor,
            format,
            data,
            origin: None,
        })
    }

    /// Whether the picture still says what it said when it was read.
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        self.origin.as_ref().is_some_and(|o| {
            o.name == self.name
                && o.description == self.description
                && o.anchor == self.anchor
                && !o.data_changed(&self.data)
        })
    }

    /// Takes the picture as it stands for what was read, for an edit that
    /// moved the bytes of the drawing and the model the same way.
    pub(crate) fn settle(&mut self) {
        if let Some(origin) = &mut self.origin {
            origin.anchor = self.anchor;
        }
    }
}

/// An image file format Excel can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
    /// GIF.
    Gif,
    /// Windows bitmap.
    Bmp,
    /// TIFF.
    Tiff,
    /// Windows enhanced metafile.
    Emf,
    /// Windows metafile.
    Wmf,
    /// SVG, which Excel 2016 and later draw.
    Svg,
}

impl ImageFormat {
    /// The format of an image file, told by its first bytes.
    #[must_use]
    pub fn sniff(data: &[u8]) -> Option<Self> {
        let starts = |magic: &[u8]| data.starts_with(magic);
        Some(if starts(b"\x89PNG\r\n\x1a\n") {
            Self::Png
        } else if starts(&[0xFF, 0xD8, 0xFF]) {
            Self::Jpeg
        } else if starts(b"GIF87a") || starts(b"GIF89a") {
            Self::Gif
        } else if starts(b"BM") {
            Self::Bmp
        } else if starts(b"II*\0") || starts(b"MM\0*") {
            Self::Tiff
        } else if data.get(40..44) == Some(b" EMF") {
            Self::Emf
        } else if starts(&[0xD7, 0xCD, 0xC6, 0x9A]) || starts(&[0x01, 0x00, 0x09, 0x00]) {
            Self::Wmf
        } else if is_svg(data) {
            Self::Svg
        } else {
            return None;
        })
    }

    /// The format a media part's name says, by extension.
    #[must_use]
    pub fn from_extension(path: &str) -> Option<Self> {
        let extension = path.rsplit_once('.')?.1.to_ascii_lowercase();
        Some(match extension.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" | "jpe" => Self::Jpeg,
            "gif" => Self::Gif,
            "bmp" | "dib" => Self::Bmp,
            "tif" | "tiff" => Self::Tiff,
            "emf" => Self::Emf,
            "wmf" => Self::Wmf,
            "svg" => Self::Svg,
            _ => return None,
        })
    }

    /// The extension a media part of this format gets.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Emf => "emf",
            Self::Wmf => "wmf",
            Self::Svg => "svg",
        }
    }

    /// The content type the package declares for it.
    #[must_use]
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Tiff => "image/tiff",
            Self::Emf => "image/x-emf",
            Self::Wmf => "image/x-wmf",
            Self::Svg => "image/svg+xml",
        }
    }
}

/// Whether bytes are an SVG document: XML whose first element is `<svg`.
fn is_svg(data: &[u8]) -> bool {
    let head = &data[..data.len().min(1024)];
    let Ok(text) = core::str::from_utf8(head) else {
        return false;
    };
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    // Skip the declaration, comments and a doctype before the root.
    while let Some(after) = rest.strip_prefix("<?").or_else(|| rest.strip_prefix("<!")) {
        let Some(end) = after.find('>') else {
            return false;
        };
        rest = after[end + 1..].trim_start();
    }
    rest.starts_with("<svg")
}

/// Where a picture came from, so an untouched one goes back as it was.
///
/// Opaque on purpose: nothing in it is a property of the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageOrigin {
    /// The drawing part holding the element.
    pub(crate) drawing: String,
    /// The drawing object id, unique within the drawing. Several pictures
    /// may share one media part, so the part does not tell them apart.
    pub(crate) id: u32,
    /// Whether the element sits inside a group of shapes.
    pub(crate) grouped: bool,
    /// What was read, to compare with.
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) anchor: Anchor,
    /// The bytes as read, by length and hash: keeping a second copy of every
    /// image to notice an edit would double what a workbook costs in memory.
    pub(crate) data_len: usize,
    pub(crate) data_hash: u64,
}

impl ImageOrigin {
    /// The drawing part the picture lives in.
    #[must_use]
    pub fn drawing(&self) -> &str {
        &self.drawing
    }

    /// Whether the picture sits inside a group of shapes, which places it:
    /// moving such a picture through [`Image::anchor`] is not written.
    #[must_use]
    pub const fn grouped(&self) -> bool {
        self.grouped
    }

    pub(crate) fn data_changed(&self, data: &[u8]) -> bool {
        data.len() != self.data_len || hash(data) != self.data_hash
    }
}

/// A hash of an image's bytes, to notice that they changed.
pub(crate) fn hash(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_told_by_their_bytes() {
        assert_eq!(
            ImageFormat::sniff(b"\x89PNG\r\n\x1a\n...."),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            ImageFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(ImageFormat::sniff(b"GIF89a"), Some(ImageFormat::Gif));
        assert_eq!(
            ImageFormat::sniff(b"<?xml version=\"1.0\"?>\n<!-- x --><svg xmlns=\"\"/>"),
            Some(ImageFormat::Svg)
        );
        assert_eq!(ImageFormat::sniff(b"<html>"), None);
        assert_eq!(ImageFormat::sniff(b""), None);
    }

    #[test]
    fn an_image_read_and_left_alone_is_unchanged() {
        let mut image = Image::new(b"GIF89a".to_vec(), Anchor::default()).unwrap();
        assert!(!image.is_unchanged(), "made in code");
        image.origin = Some(ImageOrigin {
            drawing: "xl/drawings/drawing1.xml".into(),
            id: 2,
            grouped: false,
            name: String::new(),
            description: String::new(),
            anchor: Anchor::default(),
            data_len: 6,
            data_hash: hash(b"GIF89a"),
        });
        assert!(image.is_unchanged());
        image.data = b"GIF87a".to_vec();
        assert!(!image.is_unchanged(), "same length, other bytes");
    }
}
