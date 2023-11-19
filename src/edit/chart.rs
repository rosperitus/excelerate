//! What a chart reads, when the cells under it move.
//!
//! A chart keeps its series as formulas, one per `<c:f>` element:
//! `Sheet1!$B$2:$B$5` and the like. The part is carried, not modelled, so
//! those went stale the moment a row was inserted or a sheet renamed, and a
//! chart reading the wrong cells draws the wrong picture without complaining.
//!
//! Only the text inside `<c:f>` is touched. The rest of the part is left byte
//! for byte, which matters: a chart part is full of numbers that look like
//! nothing in particular, and a rewrite loose enough to find references
//! anywhere in it would corrupt them.
//!
//! Which sheet the chart is drawn on is not asked. A series names its sheet,
//! and a chart on one sheet is free to read another, so the rewrite is by the
//! name in the reference rather than by the part it sits in.

use crate::model::Spreadsheet;

/// Rewrites every series reference in every chart of the workbook.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "a part name inside a package, not a path on a filesystem"
)]
pub(super) fn rewrite_series(book: &mut Spreadsheet, rewrite: impl Fn(&str) -> String) {
    for part in &mut book.parts {
        // The part names inside a package are fixed by the format, so this
        // comparison is over text the writer of the file chose, not a
        // filesystem path.
        if !part.path.starts_with("xl/charts/chart") || !part.path.ends_with(".xml") {
            continue;
        }
        let Ok(text) = core::str::from_utf8(&part.data) else {
            continue;
        };
        let out = map_series(text, &rewrite);
        if out.as_bytes() != part.data {
            part.data = out.into_bytes();
        }
    }
}

/// The same rewrite over the charts in the model: the formulas it names, and
/// the references inside the markup it carries.
///
/// A chart that was untouched stays so. Its part went through
/// [`rewrite_series`] with the same function, so the bytes and the model still
/// say the same thing, and the bytes are still what gets written.
pub(super) fn rewrite_model(book: &mut Spreadsheet, rewrite: impl Fn(&str) -> String) {
    for index in 0..book.sheets().len() {
        let Some(sheet) = book.sheet_mut(index) else {
            continue;
        };
        for chart in &mut sheet.charts {
            let untouched = chart.is_unchanged();
            for formula in chart.formulas_mut() {
                *formula = rewrite(formula);
            }
            for markup in chart.markups_mut() {
                *markup = map_series(markup, &rewrite);
            }
            if untouched {
                chart.settle();
            }
        }
    }
}

/// Maps the text of each `<f>` element, whatever namespace prefix it carries.
///
/// Excel writes `<c:f>`; excelize writes `<f>` with the chart namespace as the
/// default. Both mean the same element, so the prefix is ignored and the local
/// name is what counts.
fn map_series(xml: &str, rewrite: &impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..close];
        out.push_str(&rest[..=close]);
        rest = &rest[close + 1..];

        let name = tag.split([' ', '/', '\t', '\n']).next().unwrap_or(tag);
        let local = name.rsplit(':').next().unwrap_or(name);
        if local != "f" || tag.starts_with('/') || tag.ends_with('/') {
            continue;
        }
        let Some(end) = rest.find('<') else {
            break;
        };
        // A reference holds no XML syntax of its own, so the text between the
        // tags is the whole formula and needs no unescaping.
        out.push_str(&rewrite(&rest[..end]));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHART: &str = concat!(
        r#"<c:chart><c:ser><c:tx><c:strRef><c:f>Sheet1!$B$1</c:f></c:strRef></c:tx>"#,
        r#"<c:val><c:numRef><c:f>Sheet1!$B$2:$B$5</c:f><c:numCache>"#,
        r#"<c:pt idx="0"><c:v>120</c:v></c:pt></c:numCache></c:numRef></c:val>"#,
        r#"</c:ser></c:chart>"#,
    );

    #[test]
    fn only_the_series_text_is_rewritten() {
        let out = map_series(CHART, &|f| format!("[{f}]"));
        assert!(out.contains("<c:f>[Sheet1!$B$1]</c:f>"), "{out}");
        assert!(out.contains("<c:f>[Sheet1!$B$2:$B$5]</c:f>"), "{out}");
        // The cached values sitting beside the reference are not references.
        assert!(out.contains("<c:v>120</c:v>"), "{out}");
    }

    /// The same element without a prefix, which is how excelize writes it.
    #[test]
    fn a_default_namespace_is_the_same_element() {
        let out = map_series("<chart><f>Sheet1!$A$1</f></chart>", &|f| format!("[{f}]"));
        assert_eq!(out, "<chart><f>[Sheet1!$A$1]</f></chart>");
    }

    /// An element named `f` that closes or stands alone carries no text.
    #[test]
    fn an_empty_element_is_left_alone() {
        let out = map_series("<c:f/><c:formatCode>General</c:formatCode>", &|f| {
            format!("[{f}]")
        });
        assert_eq!(out, "<c:f/><c:formatCode>General</c:formatCode>");
    }
}
