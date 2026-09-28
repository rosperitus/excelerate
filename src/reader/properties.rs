//! Reading the document properties parts of an xlsx package.
//!
//! Also used by the writer, which parses what the package holds and compares
//! it with the model: a part the model still states travels as its bytes.

use super::chart::{Node, children};
use crate::model::{CustomProperty, DocumentProperties, PropertyValue};

/// Relationship and content types of the three parts.
pub(crate) const CORE_REL: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
pub(crate) const APP_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
pub(crate) const CUSTOM_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";

/// The root element of a part, found by local name.
fn root<'a>(xml: &'a str, name: &str) -> Option<Node<'a>> {
    children(xml).into_iter().find(|n| n.name == name)
}

/// Fills the Dublin Core fields from `docProps/core.xml`.
pub(crate) fn read_core(xml: &str, out: &mut DocumentProperties) {
    let Some(root) = root(xml, "coreProperties") else {
        return;
    };
    for node in root.children() {
        if let Some(field) = out.core_field_mut(node.name) {
            *field = Some(node.text());
        }
    }
}

/// Fills company and manager from `docProps/app.xml`. The rest of the part
/// is Excel's own bookkeeping and stays in the bytes.
pub(crate) fn read_app(xml: &str, out: &mut DocumentProperties) {
    let Some(root) = root(xml, "Properties") else {
        return;
    };
    out.company = root.child("Company").map(|n| n.text());
    out.manager = root.child("Manager").map(|n| n.text());
}

/// The user's fields from `docProps/custom.xml`.
pub(crate) fn read_custom(xml: &str) -> Vec<CustomProperty> {
    let Some(root) = root(xml, "Properties") else {
        return Vec::new();
    };
    root.children()
        .into_iter()
        .filter(|n| n.name == "property")
        .filter_map(|property| {
            let name = property.attr_text("name")?;
            let value = property.children().into_iter().next()?;
            Some(CustomProperty {
                name,
                value: typed(value.name, value.text()),
            })
        })
        .collect()
}

/// A `vt:` value by its element name. A type Excel never offers (currency,
/// a vector, a blob) reads as its text rather than being dropped.
fn typed(kind: &str, text: String) -> PropertyValue {
    let number = || text.trim().parse::<f64>().ok();
    match kind {
        "i1" | "i2" | "i4" | "i8" | "int" | "ui1" | "ui2" | "ui4" | "ui8" | "uint" => {
            text.trim().parse().map_or_else(
                |_| PropertyValue::Text(text.clone()),
                PropertyValue::Integer,
            )
        }
        "r4" | "r8" | "decimal" => {
            number().map_or_else(|| PropertyValue::Text(text.clone()), PropertyValue::Number)
        }
        "bool" => PropertyValue::Bool(matches!(text.trim(), "true" | "1")),
        "filetime" | "date" => PropertyValue::Date(text),
        _ => PropertyValue::Text(text),
    }
}

/// The properties of an ODS package, from `meta.xml`.
///
/// ODF names the author `meta:initial-creator` and the last editor
/// `dc:creator`, where xlsx says `dc:creator` and `cp:lastModifiedBy`.
/// Keywords are one element each and come back joined with `, `. ODF has no
/// element for category, status, identifier, version, company or manager.
pub(crate) fn read_odf_meta(xml: &str) -> DocumentProperties {
    let mut out = DocumentProperties::default();
    let Some(meta) = root(xml, "document-meta").and_then(|r| r.child("meta")) else {
        return out;
    };
    let mut keywords: Vec<String> = Vec::new();
    for node in meta.children() {
        let text = || Some(node.text());
        match (node.prefix, node.name) {
            ("dc", "title") => out.title = text(),
            ("dc", "subject") => out.subject = text(),
            ("dc", "description") => out.description = text(),
            ("dc", "language") => out.language = text(),
            ("dc", "creator") => out.last_modified_by = text(),
            ("dc", "date") => out.modified = text(),
            ("meta", "initial-creator") => out.creator = text(),
            ("meta", "creation-date") => out.created = text(),
            ("meta", "print-date") => out.last_printed = text(),
            ("meta", "editing-cycles") => out.revision = text(),
            ("meta", "keyword") => keywords.push(node.text()),
            ("meta", "user-defined") => {
                let Some(name) = node.attr_text("name") else {
                    continue;
                };
                let value = match node.attr("value-type").unwrap_or("string") {
                    "float" | "percentage" | "currency" => match node.text().trim() {
                        whole if whole.parse::<i64>().is_ok() => typed("i8", node.text()),
                        _ => typed("r8", node.text()),
                    },
                    "boolean" => typed("bool", node.text()),
                    "date" | "time" => typed("date", node.text()),
                    _ => PropertyValue::Text(node.text()),
                };
                out.custom.push(CustomProperty { name, value });
            }
            _ => {}
        }
    }
    if !keywords.is_empty() {
        out.keywords = Some(keywords.join(", "));
    }
    out
}

/// The property set of `\x05SummaryInformation`.
pub(crate) const SUMMARY_FMTID: [u8; 16] = [
    0xE0, 0x85, 0x9F, 0xF2, 0xF9, 0x4F, 0x68, 0x10, 0xAB, 0x91, 0x08, 0x00, 0x2B, 0x27, 0xB3, 0xD9,
];
/// The first set of `\x05DocumentSummaryInformation`: category, company.
pub(crate) const DOCUMENT_FMTID: [u8; 16] = [
    0x02, 0xD5, 0xCD, 0xD5, 0x9C, 0x2E, 0x1B, 0x10, 0x93, 0x97, 0x08, 0x00, 0x2B, 0x2C, 0xF9, 0xAE,
];
/// Its second set: the user's own fields.
pub(crate) const USER_FMTID: [u8; 16] = [
    0x05, 0xD5, 0xCD, 0xD5, 0x9C, 0x2E, 0x1B, 0x10, 0x93, 0x97, 0x08, 0x00, 0x2B, 0x2C, 0xF9, 0xAE,
];
/// The code page that makes `VT_LPSTR` UTF-16.
pub(crate) const CP_WINUNICODE: u16 = 1200;

/// A value from a property set, before it is given a field.
#[derive(Debug, Clone, PartialEq)]
enum Raw {
    Text(String),
    Integer(i64),
    Number(f64),
    Bool(bool),
    Time(u64),
}

/// The properties of an xls, from its two property set streams (MS-OLEPS).
///
/// Every offset comes from the file, so every read is bounds-checked and a
/// damaged set yields what could be read rather than an error: the workbook
/// matters more than its title.
pub(crate) fn read_ole_properties(
    summary: Option<&[u8]>,
    document: Option<&[u8]>,
) -> DocumentProperties {
    let mut out = DocumentProperties::default();
    for (fmtid, section) in summary.map(sections).unwrap_or_default() {
        if fmtid != SUMMARY_FMTID {
            continue;
        }
        for (pid, value) in values(section) {
            let text = || match &value {
                Raw::Text(t) => Some(t.clone()),
                _ => None,
            };
            let time = || match value {
                Raw::Time(t) => filetime_to_iso(t),
                _ => None,
            };
            match pid {
                2 => out.title = text(),
                3 => out.subject = text(),
                4 => out.creator = text(),
                5 => out.keywords = text(),
                6 => out.description = text(),
                8 => out.last_modified_by = text(),
                9 => out.revision = text(),
                11 => out.last_printed = time(),
                12 => out.created = time(),
                13 => out.modified = time(),
                _ => {}
            }
        }
    }
    for (fmtid, section) in document.map(sections).unwrap_or_default() {
        if fmtid == DOCUMENT_FMTID {
            for (pid, value) in values(section) {
                let Raw::Text(text) = value else { continue };
                match pid {
                    2 => out.category = Some(text),
                    14 => out.manager = Some(text),
                    15 => out.company = Some(text),
                    0x1B => out.content_status = Some(text),
                    0x1C => out.language = Some(text),
                    0x1D => out.version = Some(text),
                    _ => {}
                }
            }
        } else if fmtid == USER_FMTID {
            let names = dictionary(section);
            for (pid, value) in values(section) {
                let Some(name) = names
                    .iter()
                    .find(|(id, _)| *id == pid)
                    .map(|(_, n)| n.clone())
                else {
                    continue;
                };
                let value = match value {
                    Raw::Text(t) => PropertyValue::Text(t),
                    Raw::Integer(n) => PropertyValue::Integer(n),
                    Raw::Number(n) => PropertyValue::Number(n),
                    Raw::Bool(b) => PropertyValue::Bool(b),
                    Raw::Time(t) => match filetime_to_iso(t) {
                        Some(iso) => PropertyValue::Date(iso),
                        None => continue,
                    },
                };
                out.custom.push(CustomProperty { name, value });
            }
        }
    }
    out
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// The sets of a stream: id and the bytes from the set's start to the end.
fn sections(stream: &[u8]) -> Vec<([u8; 16], &[u8])> {
    if u16_at(stream, 0) != Some(0xFFFE) {
        return Vec::new();
    }
    let count = u32_at(stream, 24).unwrap_or(0).min(8) as usize;
    (0..count)
        .filter_map(|i| {
            let at = 28 + i * 20;
            let fmtid: [u8; 16] = stream.get(at..at + 16)?.try_into().ok()?;
            let offset = u32_at(stream, at + 16)? as usize;
            Some((fmtid, stream.get(offset..)?))
        })
        .collect()
}

/// Each property of a set with where its value starts, codepage and
/// dictionary (ids 1 and 0) left out.
fn entries(section: &[u8]) -> Vec<(u32, usize)> {
    let count = u32_at(section, 4).unwrap_or(0).min(4096) as usize;
    (0..count)
        .filter_map(|i| {
            let pid = u32_at(section, 8 + i * 8)?;
            let offset = u32_at(section, 12 + i * 8)? as usize;
            Some((pid, offset))
        })
        .collect()
}

/// The set's code page, property 1; Windows-1252 when the set names none.
fn codepage(section: &[u8]) -> u16 {
    entries(section)
        .into_iter()
        .find(|&(pid, _)| pid == 1)
        .and_then(|(_, at)| u16_at(section, at + 4))
        .unwrap_or(1252)
}

/// The typed values of a set, other than its code page and dictionary.
fn values(section: &[u8]) -> Vec<(u32, Raw)> {
    let page = codepage(section);
    entries(section)
        .into_iter()
        .filter(|&(pid, _)| pid > 1)
        .filter_map(|(pid, at)| Some((pid, value(section, at, page)?)))
        .collect()
}

/// One typed value: a 16-bit type, two bytes of padding, then the value.
fn value(section: &[u8], at: usize, page: u16) -> Option<Raw> {
    let body = at + 4;
    Some(match u16_at(section, at)? {
        2 => Raw::Integer(i64::from(i16::from_le_bytes(
            section.get(body..body + 2)?.try_into().ok()?,
        ))),
        3 | 22 => Raw::Integer(i64::from(i32::from_le_bytes(
            section.get(body..body + 4)?.try_into().ok()?,
        ))),
        19 | 23 => Raw::Integer(i64::from(u32_at(section, body)?)),
        20 => Raw::Integer(i64::from_le_bytes(
            section.get(body..body + 8)?.try_into().ok()?,
        )),
        4 => Raw::Number(f64::from(f32::from_le_bytes(
            section.get(body..body + 4)?.try_into().ok()?,
        ))),
        5 => Raw::Number(f64::from_le_bytes(
            section.get(body..body + 8)?.try_into().ok()?,
        )),
        11 => Raw::Bool(u16_at(section, body)? != 0),
        30 => {
            let size = u32_at(section, body)? as usize;
            Raw::Text(text(section.get(body + 4..body + 4 + size)?, page))
        }
        31 => {
            let chars = u32_at(section, body)? as usize;
            Raw::Text(text(
                section.get(body + 4..body + 4 + chars.checked_mul(2)?)?,
                CP_WINUNICODE,
            ))
        }
        64 => Raw::Time(u64_at(section, body)?),
        _ => return None,
    })
}

/// A string in the set's code page, without the terminating zeros. Under
/// code page 1200 an `LPSTR` is UTF-16: 1C and WPS write it that way.
fn text(bytes: &[u8], page: u16) -> String {
    let decoded = if page == CP_WINUNICODE {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        crate::shared::codepage::decode(page, bytes)
    };
    decoded.trim_end_matches('\0').to_owned()
}

/// The names of the user's fields: property 0 maps ids to names.
fn dictionary(section: &[u8]) -> Vec<(u32, String)> {
    let page = codepage(section);
    let Some(at) = entries(section)
        .into_iter()
        .find(|&(pid, _)| pid == 0)
        .map(|(_, at)| at)
    else {
        return Vec::new();
    };
    let count = u32_at(section, at).unwrap_or(0).min(4096);
    let mut out = Vec::new();
    let mut cursor = at + 4;
    for _ in 0..count {
        let (Some(pid), Some(chars)) = (u32_at(section, cursor), u32_at(section, cursor + 4))
        else {
            break;
        };
        cursor += 8;
        let chars = chars as usize;
        let width = if page == CP_WINUNICODE { 2 } else { 1 };
        let Some(bytes) = chars
            .checked_mul(width)
            .and_then(|n| section.get(cursor..cursor + n))
        else {
            break;
        };
        out.push((pid, text(bytes, page)));
        cursor += bytes.len();
        // Under UTF-16 each name is padded to four bytes.
        if page == CP_WINUNICODE {
            cursor = cursor.next_multiple_of(4);
        }
    }
    out
}

/// 100-nanosecond ticks between 1601-01-01 and 1970-01-01.
const FILETIME_UNIX: u64 = 116_444_736_000_000_000;

/// A `FILETIME` as ISO 8601 in UTC; zero, the "never" of property sets, is
/// no date.
pub(crate) fn filetime_to_iso(ticks: u64) -> Option<String> {
    if ticks == 0 {
        return None;
    }
    let seconds =
        i64::try_from(ticks / 10_000_000).ok()? - i64::try_from(FILETIME_UNIX / 10_000_000).ok()?;
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    ))
}

#[cfg(any(feature = "write", test))]
/// ISO 8601 as a `FILETIME`: `YYYY-MM-DD`, optionally `THH:MM:SS` with a
/// fraction, then `Z` or an offset such as `+03:00`. No zone means UTC.
pub(crate) fn iso_to_filetime(iso: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| iso.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = if iso.len() >= 19 {
        (number(11..13)?, number(14..16)?, number(17..19)?)
    } else {
        (0, 0, 0)
    };
    // The zone, if any, after the seconds and their fraction.
    let zone = iso
        .get(19..)
        .unwrap_or("")
        .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone.as_bytes().first() {
        Some(sign @ (b'+' | b'-')) => {
            let minutes =
                zone.get(1..3)?.parse::<i64>().ok()? * 60 + zone.get(4..6)?.parse::<i64>().ok()?;
            if *sign == b'+' {
                minutes * 60
            } else {
                -minutes * 60
            }
        }
        _ => 0,
    };
    let days = days_from_civil(year, u32::try_from(month).ok()?, u32::try_from(day).ok()?);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    let ticks = i128::from(seconds) * 10_000_000 + i128::from(FILETIME_UNIX);
    u64::try_from(ticks).ok()
}

#[cfg(any(feature = "write", test))]
/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let month = i64::from(month);
    let of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_cycle - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let of_era = z - era * 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * of_year + 2) / 153;
    let day = of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_and_iso_convert_both_ways() {
        // 2026-09-27T10:00:00Z, as Windows counts it.
        let ticks = iso_to_filetime("2026-09-27T10:00:00Z").unwrap();
        assert_eq!(
            filetime_to_iso(ticks).as_deref(),
            Some("2026-09-27T10:00:00Z")
        );
        assert_eq!(iso_to_filetime("2026-09-27T13:00:00+03:00"), Some(ticks));
        assert_eq!(iso_to_filetime("2026-09-27T10:00:00.250Z"), Some(ticks));
        assert_eq!(
            filetime_to_iso(FILETIME_UNIX).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert_eq!(filetime_to_iso(0), None);
        assert_eq!(
            iso_to_filetime("2000-02-29").map(filetime_to_iso),
            Some(Some("2000-02-29T00:00:00Z".into()))
        );
        assert_eq!(iso_to_filetime("junk"), None);
    }

    #[test]
    fn the_three_parts_read_into_one_model() {
        let mut props = DocumentProperties::default();
        read_core(
            r#"<?xml version="1.0"?><cp:coreProperties xmlns:cp="c" xmlns:dc="d" xmlns:dcterms="t"><dc:title>Отчёт &amp; план</dc:title><dc:creator>Ann</dc:creator><dcterms:created xsi:type="dcterms:W3CDTF">2026-01-02T03:04:05Z</dcterms:created></cp:coreProperties>"#,
            &mut props,
        );
        read_app(
            r"<Properties><Application>Microsoft Excel</Application><Company>ACME</Company></Properties>",
            &mut props,
        );
        props.custom = read_custom(
            r#"<Properties xmlns:vt="v"><property fmtid="{D5CDD505-2E9C-101B-9397-08002B2CF9AE}" pid="2" name="Отдел"><vt:lpwstr>Продажи</vt:lpwstr></property><property pid="3" name="N"><vt:i4>7</vt:i4></property><property pid="4" name="Ok"><vt:bool>true</vt:bool></property><property pid="5" name="R"><vt:r8>1.5</vt:r8></property></Properties>"#,
        );
        assert_eq!(props.title.as_deref(), Some("Отчёт & план"));
        assert_eq!(props.creator.as_deref(), Some("Ann"));
        assert_eq!(props.created.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert_eq!(props.company.as_deref(), Some("ACME"));
        assert_eq!(props.manager, None);
        assert_eq!(
            props.custom("отдел"),
            Some(&PropertyValue::Text("Продажи".into()))
        );
        assert_eq!(props.custom("N"), Some(&PropertyValue::Integer(7)));
        assert_eq!(props.custom("Ok"), Some(&PropertyValue::Bool(true)));
        assert_eq!(props.custom("R"), Some(&PropertyValue::Number(1.5)));
    }
}
