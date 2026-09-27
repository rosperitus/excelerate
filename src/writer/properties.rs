//! Writing the document properties parts of an xlsx package.
//!
//! Decided by comparison, like charts: what the package holds is parsed and
//! set against [`Spreadsheet::properties`]. A part the model still states
//! travels as its bytes, namespaces and all. A changed `core.xml` or
//! `custom.xml` is rendered from the model. `app.xml` is Excel's own
//! bookkeeping, the sheet list and the version that saved it, so only its
//! `Company` and `Manager` are rewritten in place; a book without one gets a
//! part holding just those two.

use super::chart::{part_text, set_part};
use super::xmlesc::escape;
use crate::model::{Attachment, CustomProperty, DocumentProperties, PropertyValue, Spreadsheet};
use crate::reader::chart::children;
use crate::reader::properties::{APP_REL, CORE_REL, CUSTOM_REL, read_app, read_core, read_custom};
use std::borrow::Cow;
use std::fmt::Write as _;

const CORE_TYPE: &str = "application/vnd.openxmlformats-package.core-properties+xml";
const APP_TYPE: &str = "application/vnd.openxmlformats-officedocument.extended-properties+xml";
const CUSTOM_TYPE: &str = "application/vnd.openxmlformats-officedocument.custom-properties+xml";
const DECL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;
/// The one property set id custom properties are written under.
const CUSTOM_FMTID: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";

/// Brings the three parts in line with the model; an untouched book passes
/// through without a copy.
pub(super) fn prepare(book: Cow<'_, Spreadsheet>) -> Cow<'_, Spreadsheet> {
    let stored = stored(&book);
    let wanted = &book.properties;
    let core = stored.core_fields() != wanted.core_fields();
    let app = (&stored.company, &stored.manager) != (&wanted.company, &wanted.manager);
    let custom = stored.custom != wanted.custom;
    if !(core || app || custom) {
        return book;
    }
    let mut book = book.into_owned();
    let props = book.properties.clone();
    if core {
        let body = props.has_core().then(|| core_xml(&props));
        put(&mut book, CORE_REL, "docProps/core.xml", CORE_TYPE, body);
    }
    if app {
        let path = target(&book, APP_REL);
        let body = match path.as_deref().and_then(|p| part_text(&book, p)) {
            Some(xml) => Some(patch_app(xml, &props)),
            None => (props.company.is_some() || props.manager.is_some()).then(|| app_xml(&props)),
        };
        put(&mut book, APP_REL, "docProps/app.xml", APP_TYPE, body);
    }
    if custom {
        let body = (!props.custom.is_empty()).then(|| custom_xml(&props.custom));
        put(
            &mut book,
            CUSTOM_REL,
            "docProps/custom.xml",
            CUSTOM_TYPE,
            body,
        );
    }
    Cow::Owned(book)
}

/// What the package's parts say now, read the way the reader reads them.
fn stored(book: &Spreadsheet) -> DocumentProperties {
    let mut out = DocumentProperties::default();
    let text = |kind| target(book, kind).and_then(|p| part_text(book, &p).map(str::to_owned));
    if let Some(xml) = text(CORE_REL) {
        read_core(&xml, &mut out);
    }
    if let Some(xml) = text(APP_REL) {
        read_app(&xml, &mut out);
    }
    if let Some(xml) = text(CUSTOM_REL) {
        out.custom = read_custom(&xml);
    }
    out
}

/// Where the package's relationship of this kind points, if it has one.
fn target(book: &Spreadsheet, kind: &str) -> Option<String> {
    book.doc_props
        .iter()
        .find(|a| a.kind == kind)
        .map(|a| a.target.clone())
}

/// Replaces a part with `body`, adding it and its relationship when the
/// package has none; `None` removes both.
fn put(
    book: &mut Spreadsheet,
    kind: &str,
    default: &str,
    content_type: &str,
    body: Option<String>,
) {
    let path = target(book, kind).unwrap_or_else(|| default.to_owned());
    let Some(body) = body else {
        book.parts.retain(|p| p.path != path);
        book.doc_props.retain(|a| a.kind != kind);
        return;
    };
    set_part(book, &path, Some(content_type), body);
    if !book.doc_props.iter().any(|a| a.kind == kind) {
        book.doc_props.push(Attachment {
            kind: kind.to_owned(),
            target: path,
        });
    }
}

fn core_xml(props: &DocumentProperties) -> String {
    let mut s = format!(
        concat!(
            "{}",
            r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties""#,
            r#" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/""#,
            r#" xmlns:dcmitype="http://purl.org/dc/dcmitype/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">"#,
        ),
        DECL
    );
    for (element, value) in props.core_fields() {
        let Some(value) = value else { continue };
        // The two dates carry their type; Excel refuses a bare one.
        let typed = if element.starts_with("dcterms:") {
            r#" xsi:type="dcterms:W3CDTF""#
        } else {
            ""
        };
        let _ = write!(s, "<{element}{typed}>{}</{element}>", escape(value));
    }
    s.push_str("</cp:coreProperties>");
    s
}

fn app_xml(props: &DocumentProperties) -> String {
    let mut s = format!(
        concat!(
            "{}",
            r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties""#,
            r#" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">"#,
        ),
        DECL
    );
    for (element, value) in [("Manager", &props.manager), ("Company", &props.company)] {
        if let Some(value) = value {
            let _ = write!(s, "<{element}>{}</{element}>", escape(value));
        }
    }
    s.push_str("</Properties>");
    s
}

/// `app.xml` with `Manager` and `Company` set to the model and every other
/// byte as it was. A value that is gone takes its element with it; a new one
/// goes before the closing tag.
fn patch_app(xml: &str, props: &DocumentProperties) -> String {
    let Some(root) = children(xml).into_iter().find(|n| n.name == "Properties") else {
        return app_xml(props);
    };
    // Edits from the end, so earlier offsets stay valid.
    let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut appended = String::new();
    for (element, value) in [("Manager", &props.manager), ("Company", &props.company)] {
        let rendered = value
            .as_ref()
            .map(|v| format!("<{element}>{}</{element}>", escape(v)))
            .unwrap_or_default();
        match root.children().into_iter().find(|n| n.name == element) {
            Some(node) => {
                let start = root.inner_start + node.span.start;
                edits.push((start..root.inner_start + node.span.end, rendered));
            }
            None => appended.push_str(&rendered),
        }
    }
    let close = root.inner_start + root.inner.len();
    edits.push((close..close, appended));
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut out = xml.to_owned();
    for (range, text) in edits {
        out.replace_range(range, &text);
    }
    out
}

fn custom_xml(custom: &[CustomProperty]) -> String {
    let mut s = format!(
        concat!(
            "{}",
            r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/custom-properties""#,
            r#" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">"#,
        ),
        DECL
    );
    // Property ids start at 2: 0 and 1 are reserved by the property set format.
    for (pid, property) in (2..).zip(custom) {
        let (kind, text) = match &property.value {
            PropertyValue::Text(t) => ("lpwstr", escape(t)),
            PropertyValue::Integer(n) => ("i4", n.to_string()),
            PropertyValue::Number(n) => ("r8", n.to_string()),
            PropertyValue::Bool(b) => ("bool", b.to_string()),
            PropertyValue::Date(d) => ("filetime", escape(d)),
        };
        // `i4` holds 32 bits; a larger whole number goes as `i8`.
        let kind = match &property.value {
            PropertyValue::Integer(n) if i32::try_from(*n).is_err() => "i8",
            _ => kind,
        };
        let _ = write!(
            s,
            r#"<property fmtid="{CUSTOM_FMTID}" pid="{pid}" name="{}"><vt:{kind}>{text}</vt:{kind}></property>"#,
            escape(&property.name)
        );
    }
    s.push_str("</Properties>");
    s
}

/// The two property set streams of an xls, as `(name, bytes)`, leaving out a
/// stream that would say nothing (MS-OLEPS).
///
/// Strings go as `VT_LPSTR` under code page 1200, which makes them UTF-16:
/// 1C and WPS write their exports this way, and a single ANSI page would lose
/// every character outside it. A date that is not ISO 8601 is left out, and a
/// whole number past 32 bits goes as `VT_R8`, since `VT_I8` is not allowed in
/// the version 0 sets Excel writes.
pub(crate) fn ole_streams(props: &DocumentProperties) -> Vec<(&'static str, Vec<u8>)> {
    use crate::reader::properties::{DOCUMENT_FMTID, SUMMARY_FMTID, USER_FMTID, iso_to_filetime};
    let text = |value: &Option<String>| value.as_ref().map(|t| Value::Text(t.clone()));
    let time = |value: &Option<String>| value.as_deref().and_then(iso_to_filetime).map(Value::Time);
    let summary: Vec<(u32, Value)> = [
        (2, text(&props.title)),
        (3, text(&props.subject)),
        (4, text(&props.creator)),
        (5, text(&props.keywords)),
        (6, text(&props.description)),
        (8, text(&props.last_modified_by)),
        (9, text(&props.revision)),
        (11, time(&props.last_printed)),
        (12, time(&props.created)),
        (13, time(&props.modified)),
    ]
    .into_iter()
    .filter_map(|(pid, value)| Some((pid, value?)))
    .collect();
    let document: Vec<(u32, Value)> = [
        (2, text(&props.category)),
        (14, text(&props.manager)),
        (15, text(&props.company)),
        (0x1B, text(&props.content_status)),
        (0x1C, text(&props.language)),
        (0x1D, text(&props.version)),
    ]
    .into_iter()
    .filter_map(|(pid, value)| Some((pid, value?)))
    .collect();
    let user: Vec<(String, Value)> = props
        .custom
        .iter()
        .filter_map(|p| {
            let value = match &p.value {
                PropertyValue::Text(t) => Value::Text(t.clone()),
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "past 32 bits only VT_R8 is left, exact to 2^53"
                )]
                PropertyValue::Integer(n) => {
                    i32::try_from(*n).map_or_else(|_| Value::Real(*n as f64), Value::Int)
                }
                PropertyValue::Number(n) => Value::Real(*n),
                PropertyValue::Bool(b) => Value::Bool(*b),
                PropertyValue::Date(d) => Value::Time(iso_to_filetime(d)?),
            };
            Some((p.name.clone(), value))
        })
        .collect();

    let mut out = Vec::new();
    if !summary.is_empty() {
        out.push((
            "\u{5}SummaryInformation",
            property_stream(&[(SUMMARY_FMTID, section(&summary, &[]))]),
        ));
    }
    if !document.is_empty() || !user.is_empty() {
        let mut sets = vec![(DOCUMENT_FMTID, section(&document, &[]))];
        if !user.is_empty() {
            let names: Vec<(u32, &str)> = (2..).zip(user.iter().map(|(n, _)| n.as_str())).collect();
            let values: Vec<(u32, Value)> =
                (2..).zip(user.iter().map(|(_, v)| v.clone())).collect();
            sets.push((USER_FMTID, section(&values, &names)));
        }
        out.push(("\u{5}DocumentSummaryInformation", property_stream(&sets)));
    }
    out
}

/// A value on its way into a property set.
#[derive(Clone)]
enum Value {
    Text(String),
    Int(i32),
    Real(f64),
    Bool(bool),
    Time(u64),
}

/// The stream header and its sets, each at the offset the header names.
fn property_stream(sets: &[([u8; 16], Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0xFFFEu16.to_le_bytes()); // byte order
    out.extend_from_slice(&0u16.to_le_bytes()); // version 0
    out.extend_from_slice(&0x0002_0006u32.to_le_bytes()); // Windows, as Excel writes
    out.extend_from_slice(&[0; 16]); // no class id
    out.extend_from_slice(&u32::try_from(sets.len()).unwrap_or(0).to_le_bytes());
    let mut offset = 28 + sets.len() * 20;
    for (fmtid, body) in sets {
        out.extend_from_slice(fmtid);
        out.extend_from_slice(&u32::try_from(offset).unwrap_or(0).to_le_bytes());
        offset += body.len();
    }
    for (_, body) in sets {
        out.extend_from_slice(body);
    }
    out
}

/// One set: its size, the id and offset of each property, then the values,
/// each padded to four bytes. Code page 1200 is property 1; a set with names
/// carries them as the dictionary, property 0.
fn section(values: &[(u32, Value)], names: &[(u32, &str)]) -> Vec<u8> {
    let utf16 = |text: &str| -> Vec<u8> {
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    };
    let mut bodies: Vec<(u32, Vec<u8>)> = Vec::new();
    if !names.is_empty() {
        let mut dictionary = u32::try_from(names.len())
            .unwrap_or(0)
            .to_le_bytes()
            .to_vec();
        for (pid, name) in names {
            let bytes = utf16(name);
            dictionary.extend_from_slice(&pid.to_le_bytes());
            dictionary
                .extend_from_slice(&u32::try_from(bytes.len() / 2).unwrap_or(0).to_le_bytes());
            dictionary.extend_from_slice(&bytes);
            dictionary.resize(dictionary.len().next_multiple_of(4), 0);
        }
        bodies.push((0, dictionary));
    }
    // VT_I2 1200, padded to four bytes.
    bodies.push((1, [2, 0, 0, 0, 0xB0, 0x04, 0, 0].to_vec()));
    for (pid, value) in values {
        let mut body = Vec::new();
        match value {
            Value::Text(text) => {
                let bytes = utf16(text);
                body.extend_from_slice(&30u32.to_le_bytes());
                body.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(0).to_le_bytes());
                body.extend_from_slice(&bytes);
            }
            Value::Int(n) => {
                body.extend_from_slice(&3u32.to_le_bytes());
                body.extend_from_slice(&n.to_le_bytes());
            }
            Value::Real(n) => {
                body.extend_from_slice(&5u32.to_le_bytes());
                body.extend_from_slice(&n.to_le_bytes());
            }
            Value::Bool(b) => {
                body.extend_from_slice(&11u32.to_le_bytes());
                body.extend_from_slice(&(if *b { 0xFFFFu32 } else { 0 }).to_le_bytes());
            }
            Value::Time(ticks) => {
                body.extend_from_slice(&64u32.to_le_bytes());
                body.extend_from_slice(&ticks.to_le_bytes());
            }
        }
        body.resize(body.len().next_multiple_of(4), 0);
        bodies.push((*pid, body));
    }
    let header = 8 + bodies.len() * 8;
    let size = header + bodies.iter().map(|(_, b)| b.len()).sum::<usize>();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&u32::try_from(size).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&u32::try_from(bodies.len()).unwrap_or(0).to_le_bytes());
    let mut offset = header;
    for (pid, body) in &bodies {
        out.extend_from_slice(&pid.to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset).unwrap_or(0).to_le_bytes());
        offset += body.len();
    }
    for (_, body) in bodies {
        out.extend_from_slice(&body);
    }
    out
}
