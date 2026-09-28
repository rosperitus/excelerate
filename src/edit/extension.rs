//! What the sheet's carried `<extLst>` points at, when cells move.
//!
//! The extensions newer than the 2006 schema - the `x14` conditional formats
//! and data validations, sparklines - travel as the bytes they came in, and
//! they name cells in two elements: `<xm:sqref>`, the cells the rule covers on
//! the sheet that holds it, and `<xm:f>`, a formula. Left alone, a row
//! inserted above a data bar moved the rule of the bar and not the bar itself.
//!
//! Only the text of those two elements changes. A rule whose every cell the
//! edit removed goes with its enclosing element, the way an ordinary
//! conditional format does.

use crate::coordinate::Range;

/// The elements that go when their `<xm:sqref>` is left with nothing.
const CONTAINERS: [&str; 3] = [
    "x14:conditionalFormatting",
    "x14:dataValidation",
    "x14:sparkline",
];

/// Rewrites every `<xm:f>` through `formula`, and every `<xm:sqref>` through
/// `range` when there is one - only the sheet the edit is on has its own
/// cells moved.
pub(super) fn rewrite(
    ext: &str,
    formula: impl Fn(&str) -> String,
    range: Option<&dyn Fn(Range) -> Option<Range>>,
) -> String {
    let with_formulas = map_text(ext, "xm:f", |text| Some(formula(text)));
    let Some(range) = range else {
        return with_formulas;
    };
    map_text(&with_formulas, "xm:sqref", |text| {
        let kept: Vec<String> = text
            .split_whitespace()
            .filter_map(|part| Range::parse(part).ok())
            .filter_map(range)
            .map(|r| {
                if r.start == r.end {
                    r.start.to_string()
                } else {
                    r.to_string()
                }
            })
            .collect();
        (!kept.is_empty()).then(|| kept.join(" "))
    })
}

/// Maps the text of every `<tag>` element. `None` removes the innermost
/// [`CONTAINERS`] element around it; an element outside any is left as it
/// was.
fn map_text(ext: &str, tag: &str, f: impl Fn(&str) -> Option<String>) -> String {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = String::with_capacity(ext.len());
    let mut rest = ext;
    while let Some(start) = rest.find(&open) {
        let body = start + open.len();
        let Some(len) = rest[body..].find(&close) else {
            break;
        };
        let raw = &rest[body..body + len];
        let text = quick_xml::escape::unescape(raw)
            .map_or_else(|_| raw.to_owned(), std::borrow::Cow::into_owned);
        let after = body + len + close.len();
        if let Some(new) = f(&text) {
            out.push_str(&rest[..body]);
            out.push_str(&escape(&new));
            out.push_str(&close);
            rest = &rest[after..];
            continue;
        }
        // The nearest container opened before the element is the one around
        // it: `<x14:sparklines` shares a prefix but always opens earlier.
        let before = &rest[..start];
        let around = CONTAINERS
            .iter()
            .filter_map(|name| Some((before.rfind(&format!("<{name}"))?, *name)))
            .max_by_key(|(at, _)| *at)
            .and_then(|(from, name)| {
                let closing = format!("</{name}>");
                Some((from, after + rest[after..].find(&closing)? + closing.len()))
            });
        if let Some((from, to)) = around {
            out.push_str(&rest[..from]);
            rest = &rest[to..];
        } else {
            out.push_str(&rest[..after]);
            rest = &rest[after..];
        }
    }
    out.push_str(rest);
    out
}

/// Text back into element content.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::rewrite;
    use crate::coordinate::Range;

    const EXT: &str = concat!(
        r#"<extLst><ext uri="{78C0D931-6437-407d-A8EE-F0AAD7539E65}"><x14:conditionalFormattings>"#,
        r#"<x14:conditionalFormatting><x14:cfRule id="{1}"/><xm:sqref>B4:E15</xm:sqref>"#,
        r#"</x14:conditionalFormatting><x14:conditionalFormatting><x14:cfRule id="{2}"/>"#,
        r#"<xm:sqref>A1</xm:sqref></x14:conditionalFormatting></x14:conditionalFormattings></ext>"#,
        r#"<ext uri="{CCE6A557-97BC-4b89-ADB6-D9C93CAAB3DF}"><x14:dataValidations>"#,
        r#"<x14:dataValidation><x14:formula1><xm:f>'A&amp;B'!$A$1:$A$3</xm:f></x14:formula1>"#,
        r#"<xm:sqref>C2 C9</xm:sqref></x14:dataValidation></x14:dataValidations></ext></extLst>"#,
    );

    #[test]
    fn rules_follow_their_cells_and_one_left_with_none_goes() {
        // Row 1 removed: everything moves up one, and the rule on A1 is gone.
        let shift = |r: Range| {
            let up = |c: crate::CellRef| {
                Some(crate::CellRef::new(
                    c.col,
                    crate::Row::new(c.row.index().checked_sub(1)?)?,
                ))
            };
            match (up(r.start), up(r.end)) {
                (Some(start), Some(end)) => Some(Range::new(start, end)),
                (None, Some(end)) if r.start != r.end => Some(Range::new(
                    crate::CellRef::new(r.start.col, crate::Row::new(0)?),
                    end,
                )),
                _ => None,
            }
        };
        let out = rewrite(EXT, |f| f.replace("A&B", "Data"), Some(&shift));
        assert!(out.contains("<xm:sqref>B3:E14</xm:sqref>"), "{out}");
        assert!(!out.contains(r#"id="{2}""#), "{out}");
        assert!(out.contains("<xm:sqref>C1 C8</xm:sqref>"), "{out}");
        assert!(out.contains("<xm:f>'Data'!$A$1:$A$3</xm:f>"), "{out}");
        // The list around the removed rule stays well-formed.
        assert!(out.contains("</x14:conditionalFormatting></x14:conditionalFormattings>"));
    }
}
