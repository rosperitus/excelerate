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

/// Largest single XML part accepted, for the same reason.
///
/// A part is not more dangerous than the package holding it, and the package
/// is checked before any part is read, so this is the same number: it caps how
/// much one part may hold in memory at once, not how far a file may expand.
/// A real workbook does reach it - a hundred-megabyte package can carry a
/// third of a gigabyte of sheet XML.
pub const MAX_PART_SIZE: u64 = MAX_UNCOMPRESSED_SIZE;

/// The name a part is stored under, matched without regard to case.
///
/// Part names in a package are case-sensitive by the letter of the format, but
/// Excel opens files whose parts are spelled `xl/SharedStrings.xml`, and such
/// files exist in the wild. So: exact match first, then a scan.
pub fn resolve<R: Read + Seek>(zip: &zip::ZipArchive<R>, path: &str) -> Option<String> {
    if zip.index_for_name(path).is_some() {
        return Some(path.to_owned());
    }
    zip.file_names()
        .find(|name| name.eq_ignore_ascii_case(path))
        .map(ToOwned::to_owned)
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
    file.by_ref()
        .take(MAX_PART_SIZE)
        .read_to_end(&mut out)
        .map_err(|e| format!("part {path:?}: {e}"))?;
    Ok(out)
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
    file.by_ref()
        .take(MAX_PART_SIZE)
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
    attrs(e)
        .into_iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
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
