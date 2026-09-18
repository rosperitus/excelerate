//! What the two zip-and-XML readers share.
//!
//! xlsx and ods are different vocabularies over the same container: a zip of
//! XML parts. The size caps and the attribute helpers below are the same for
//! both, so they live here rather than being written twice with a chance of
//! drifting apart.

use std::io::{Read, Seek};

/// Largest total size of decompressed parts a reader will accept.
///
/// A zip file can claim a tiny compressed size and expand to gigabytes; the cap
/// is what stops a malicious file from exhausting memory.
pub const MAX_UNCOMPRESSED_SIZE: u64 = 512 * 1024 * 1024;

/// How many times its compressed size a package may expand to past
/// [`MAX_UNCOMPRESSED_SIZE`].
///
/// The absolute cap alone cannot tell a zip bomb from a big workbook: a 70 MB
/// export of nine million cells expands to 634 MB, and Excel opens it. What
/// tells them apart is the ratio. Sheet XML deflates five to twenty times; a
/// bomb is built to deflate hundreds or thousands of times, the format's own
/// limit being about a thousand.
pub const MAX_COMPRESSION_RATIO: u64 = 100;

/// Largest single part accepted: what a `String` of it can sensibly be.
///
/// A part is read up to the size its entry declares, and those sizes are what
/// the package check added up, so a part is bounded by the package; this is a
/// ceiling for a header that claims four gigabytes of XML in one piece.
pub const MAX_PART_SIZE: u64 = 4 << 30;

/// The name a part is stored under, matched without regard to case and to the
/// slash a Windows writer used.
///
/// Part names in a package are case-sensitive by the letter of the format, and
/// the separator is a forward slash. Excel opens files that break both - parts
/// spelled `xl/SharedStrings.xml`, and packages whose names carry a backslash
/// (`xl\_rels\workbook.xml.rels`) - and such files exist in the wild. So:
/// exact match first, then a scan.
pub fn resolve<R: Read + Seek>(zip: &zip::ZipArchive<R>, path: &str) -> Option<String> {
    if zip.index_for_name(path).is_some() {
        return Some(path.to_owned());
    }
    zip.file_names()
        .find(|name| same_part_name(name, path))
        .map(ToOwned::to_owned)
}

/// Whether two part names address the same part.
fn same_part_name(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes().zip(b.bytes()).all(|(x, y)| {
            let (x, y) = (
                if x == b'\\' { b'/' } else { x },
                if y == b'\\' { b'/' } else { y },
            );
            x.eq_ignore_ascii_case(&y)
        })
}

/// Whether a package may be read: under the absolute cap, or over it but
/// compressed no harder than a real workbook is.
#[must_use]
pub fn expansion_allowed(expanded: u64, compressed: u64, max_expanded: u64) -> bool {
    expanded <= max_expanded || expanded <= compressed.saturating_mul(MAX_COMPRESSION_RATIO)
}

/// The total size the archive's entries take compressed.
pub fn compressed_size<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> u64 {
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        if let Ok(entry) = zip.by_index_raw(i) {
            total = total.saturating_add(entry.compressed_size());
        }
    }
    total
}

/// The total size the archive expands to, saturating rather than overflowing.
pub fn uncompressed_size<R: Read + Seek>(zip: &mut zip::ZipArchive<R>) -> u64 {
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        if let Ok(entry) = zip.by_index_raw(i) {
            total = total.saturating_add(entry.size());
        }
    }
    total
}

/// Reads a part as bytes, refusing anything oversized.
///
/// # Errors
/// A message naming the part; the caller wraps it in the error of its format.
pub fn read_bytes<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<Vec<u8>, String> {
    let name = resolve(zip, path).ok_or_else(|| format!("package has no part {path:?}"))?;
    let mut file = zip
        .by_name(&name)
        .map_err(|_| format!("package has no part {path:?}"))?;
    if file.size() > MAX_PART_SIZE {
        return Err(format!("part {path:?} is too large"));
    }
    let mut out = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
    // No further than the entry says: that size is what the package check
    // counted, and a stream that runs past it is lying about itself.
    let declared = file.size();
    file.by_ref()
        .take(declared)
        .read_to_end(&mut out)
        .map_err(|e| format!("part {path:?}: {e}"))?;
    Ok(out)
}

/// Opens a part for reading as a stream, refusing anything oversized.
///
/// For the parts too large to hold as text while they are parsed: a sheet of
/// nine million cells is 377 MB of XML, all of it in memory beside the cells
/// built from it when the part is read whole.
///
/// # Errors
/// A message naming the part; the caller wraps it in the error of its format.
pub fn open_part<'a, R: Read + Seek>(
    zip: &'a mut zip::ZipArchive<R>,
    path: &str,
) -> Result<impl Read + 'a, String> {
    let name = resolve(zip, path).ok_or_else(|| format!("package has no part {path:?}"))?;
    let file = zip
        .by_name(&name)
        .map_err(|_| format!("package has no part {path:?}"))?;
    if file.size() > MAX_PART_SIZE {
        return Err(format!("part {path:?} is too large"));
    }
    let declared = file.size();
    Ok(file.take(declared))
}

/// Reads a part fully as text, refusing anything oversized.
///
/// # Errors
/// A message naming the part; the caller wraps it in the error of its format.
pub fn read_part<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<String, String> {
    let name = resolve(zip, path).ok_or_else(|| format!("package has no part {path:?}"))?;
    let mut file = zip
        .by_name(&name)
        .map_err(|_| format!("package has no part {path:?}"))?;
    if file.size() > MAX_PART_SIZE {
        return Err(format!("part {path:?} is too large"));
    }
    // Reserve by the declared size only after checking it, and cap the read so
    // a lying header cannot make us grow past the limit either.
    let mut text = String::with_capacity(usize::try_from(file.size()).unwrap_or(0));
    let declared = file.size();
    file.by_ref()
        .take(declared)
        .read_to_string(&mut text)
        .map_err(|e| format!("part {path:?}: {e}"))?;
    Ok(text)
}

/// Whether an XML boolean attribute says yes.
pub fn is_true(value: &str) -> bool {
    value == "1" || value == "true"
}

/// One attribute by local name, ignoring its namespace prefix.
pub fn attr(e: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    // Looked up in place: collecting every attribute into owned strings to
    // find one cost a fifth of reading a sheet of nine million cells, where
    // each `<c>` asks for three.
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .map(|a| {
            a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .unwrap_or_default()
                .into_owned()
        })
}

/// Every attribute as (local name, value).
pub fn attrs(e: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .map(|a| {
            (
                a.key.local_name().as_ref().to_owned(),
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .unwrap_or_default()
                    .into_owned(),
            )
        })
        .collect()
}

/// Appends what an entity reference stands for.
///
/// quick-xml reports `&amp;` and friends as their own event rather than as
/// text, so a reader that ignores them silently drops every escaped character.
pub fn push_entity(out: &mut String, r: &quick_xml::events::BytesRef<'_>) {
    let name = r.xml10_content();
    if let Some(text) = quick_xml::escape::resolve_predefined_entity(&name) {
        out.push_str(text);
        return;
    }
    // Numeric character references: &#65; and &#x41;.
    let code = match name.strip_prefix('#') {
        Some(hex) if hex.starts_with(['x', 'X']) => u32::from_str_radix(&hex[1..], 16).ok(),
        Some(dec) => dec.parse().ok(),
        None => None,
    };
    if let Some(c) = code.and_then(char::from_u32) {
        out.push(c);
    }
    // An unresolvable entity is dropped: the file references something it never
    // declared, and inventing text for it would be worse than losing it.
}
