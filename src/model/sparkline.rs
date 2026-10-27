//! Sparklines: the small charts Excel draws inside a cell.
//!
//! They are newer than the 2006 schema, so the file keeps them in the sheet's
//! `<extLst>`, under the `x14` extension with the uri below. That element
//! travels whole in [`crate::model::Worksheet::extensions`]; the groups here
//! are read out of it, and the writer puts them back only when they changed,
//! leaving the other extensions beside them byte for byte.
//!
//! What a program asks of a sparkline is modelled: its kind, the cells it
//! reads, the cell it sits in, and which points it marks. The colours and the
//! axis settings stay as the file wrote them, and are read out on request
//! ([`SparklineGroup::color`], [`SparklineGroup::custom_min`]).

use crate::coordinate::CellRef;
use crate::reader::xlsx::read_color;
use crate::reader::zipxml::{attr, push_entity};
use crate::style::Color;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// The `uri` of the `<ext>` that holds a sheet's sparkline groups.
pub(crate) const EXTENSION_URI: &str = "{05C60535-1F16-4fd2-B633-F4F36F0B64E0}";

/// How a sparkline draws its numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SparklineKind {
    /// A line through the points. The default, left off in the file.
    #[default]
    Line,
    /// A bar per point.
    Column,
    /// A bar per point, up or down by sign only: `stacked` in the file.
    WinLoss,
}

impl SparklineKind {
    /// The `type` attribute, `None` for the default.
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Line => None,
            Self::Column => Some("column"),
            Self::WinLoss => Some("stacked"),
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "column" => Self::Column,
            "stacked" => Self::WinLoss,
            _ => Self::Line,
        }
    }
}

/// One sparkline: the cells it reads and the cell it is drawn in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sparkline {
    /// What it reads, as a formula: `Sheet1!B2:M2`. Excel always names the
    /// sheet, since the data may sit on another one.
    pub data: Option<String>,
    /// The cell it is drawn in, on the sheet that holds the group.
    pub location: CellRef,
}

/// Sparklines that share their kind and look, as Excel groups them: a group
/// is what the Sparkline tab edits at once.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the independent point markers of one <x14:sparklineGroup>"
)]
pub struct SparklineGroup {
    /// Line, column or win/loss.
    pub kind: SparklineKind,
    /// The sparklines, in file order.
    pub sparklines: Vec<Sparkline>,
    /// The cells holding dates that space the points, as a formula; `None`
    /// spaces them evenly.
    pub date_axis: Option<String>,
    /// Marks every point of a line.
    pub markers: bool,
    /// Marks the highest point.
    pub high: bool,
    /// Marks the lowest point.
    pub low: bool,
    /// Marks the first point.
    pub first: bool,
    /// Marks the last point.
    pub last: bool,
    /// Marks the negative points.
    pub negative: bool,
    /// The other attributes of the group, as written: axis bounds, line
    /// weight, how empty cells show, `xr2:uid`.
    pub attributes: Vec<(String, String)>,
    /// The colour elements (`x14:colorSeries` to `x14:colorLow`), as
    /// written.
    pub colors: String,
}

impl SparklineGroup {
    /// An empty group of a kind, with the look Excel gives a new one.
    #[must_use]
    pub fn new(kind: SparklineKind) -> Self {
        let colors = concat!(
            r#"<x14:colorSeries rgb="FF376092"/><x14:colorNegative rgb="FFD00000"/>"#,
            r#"<x14:colorAxis rgb="FF000000"/><x14:colorMarkers rgb="FFD00000"/>"#,
            r#"<x14:colorFirst rgb="FFD00000"/><x14:colorLast rgb="FFD00000"/>"#,
            r#"<x14:colorHigh rgb="FFD00000"/><x14:colorLow rgb="FFD00000"/>"#,
        )
        .to_owned();
        Self {
            kind,
            sparklines: Vec::new(),
            date_axis: None,
            markers: false,
            high: false,
            low: false,
            first: false,
            last: false,
            negative: false,
            attributes: vec![("displayEmptyCellsAs".into(), "gap".into())],
            colors,
        }
    }

    /// The marker switches by attribute name.
    #[must_use]
    pub fn flags(&self) -> [(&'static str, bool); 6] {
        [
            ("markers", self.markers),
            ("high", self.high),
            ("low", self.low),
            ("first", self.first),
            ("last", self.last),
            ("negative", self.negative),
        ]
    }

    /// The same switches to set, for the reader.
    pub(crate) fn flags_mut(&mut self) -> [(&'static str, &mut bool); 6] {
        [
            ("markers", &mut self.markers),
            ("high", &mut self.high),
            ("low", &mut self.low),
            ("first", &mut self.first),
            ("last", &mut self.last),
            ("negative", &mut self.negative),
        ]
    }

    /// A colour of the group by its element's local name: `colorSeries`,
    /// `colorNegative`, `colorAxis`, `colorMarkers`, `colorFirst`,
    /// `colorLast`, `colorHigh` or `colorLow`. `None` when the file left it
    /// out.
    #[must_use]
    pub fn color(&self, name: &str) -> Option<Color> {
        let mut reader = Reader::from_str(&self.colors);
        loop {
            match reader.read_event() {
                Ok(Event::Start(ref e) | Event::Empty(ref e))
                    if e.local_name().as_ref() == name =>
                {
                    return Some(read_color(e));
                }
                Ok(Event::Eof) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    /// An attribute of the group as written: `displayEmptyCellsAs` (`gap`,
    /// `zero` or `span`), `minAxisType`/`maxAxisType` (`individual`, `group`
    /// or `custom`), `manualMin`, `manualMax`, `lineWeight`, `displayXAxis`,
    /// `rightToLeft`, `displayHidden`.
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// The fixed bottom of the value axis: `manualMin` when `minAxisType` is
    /// `custom`, `None` when each sparkline or the group sets its own.
    #[must_use]
    pub fn custom_min(&self) -> Option<f64> {
        self.custom_bound("minAxisType", "manualMin")
    }

    /// The fixed top of the value axis, as [`Self::custom_min`].
    #[must_use]
    pub fn custom_max(&self) -> Option<f64> {
        self.custom_bound("maxAxisType", "manualMax")
    }

    fn custom_bound(&self, kind: &str, value: &str) -> Option<f64> {
        (self.attribute(kind)? == "custom")
            .then(|| self.attribute(value)?.parse().ok())
            .flatten()
            .filter(|v: &f64| v.is_finite())
    }

    /// The formulas the group reads through, to rewrite when cells move.
    pub(crate) fn formulas_mut(&mut self) -> impl Iterator<Item = &mut String> {
        self.sparklines
            .iter_mut()
            .filter_map(|s| s.data.as_mut())
            .chain(self.date_axis.as_mut())
    }
}

/// The sparkline groups in a sheet's `<extLst>`, in file order.
#[must_use]
pub(crate) fn read(extensions: &str) -> Vec<SparklineGroup> {
    let mut reader = Reader::from_str(extensions);
    let mut out: Vec<SparklineGroup> = Vec::new();
    let mut inside = false;
    let mut in_list = false;
    // The sparkline being read: its formula and its cell, when seen.
    let mut sparkline: Option<(Option<String>, Option<CellRef>)> = None;
    // The text of the open `<xm:f>` or `<xm:sqref>`.
    let mut text: Option<String> = None;
    loop {
        let before = reader.buffer_position();
        match reader.read_event() {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) if inside || is_ours(e) => {
                match e.local_name().as_ref() {
                    "ext" => inside = true,
                    "sparklineGroup" => out.push(group(e)),
                    "sparklines" => in_list = true,
                    "sparkline" => sparkline = Some((None, None)),
                    "f" | "sqref" => text = Some(String::new()),
                    color if color.starts_with("color") && !in_list => {
                        if let Some(group) = out.last_mut() {
                            let end = reader.buffer_position();
                            group.colors.push_str(slice(extensions, before, end));
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(text) = &mut text {
                    text.push_str(&t.xml10_content());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if let Some(text) = &mut text {
                    push_entity(text, &r);
                }
            }
            Ok(Event::End(ref e)) if inside => match e.local_name().as_ref() {
                "ext" => inside = false,
                "sparklines" => in_list = false,
                "f" => {
                    let formula = text.take();
                    match (&mut sparkline, out.last_mut()) {
                        (Some((data, _)), _) => *data = formula,
                        (None, Some(group)) => group.date_axis = formula,
                        (None, None) => {}
                    }
                }
                "sqref" => {
                    let at = text.take().and_then(|t| CellRef::parse(t.trim()).ok());
                    if let Some((_, location)) = &mut sparkline {
                        *location = at;
                    }
                }
                "sparkline" => {
                    // One with no cell to sit in draws nowhere.
                    if let (Some((data, Some(location))), Some(group)) =
                        (sparkline.take(), out.last_mut())
                    {
                        group.sparklines.push(Sparkline { data, location });
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    out
}

/// Whether an element opens the sparkline extension.
fn is_ours(e: &BytesStart<'_>) -> bool {
    e.local_name().as_ref() == "ext" && attr(e, "uri").as_deref() == Some(EXTENSION_URI)
}

/// A group from its opening tag; the children fill the rest.
fn group(e: &BytesStart<'_>) -> SparklineGroup {
    let mut group = SparklineGroup::new(SparklineKind::Line);
    group.attributes.clear();
    group.colors.clear();
    for a in e.attributes().flatten() {
        let key = a.key.as_ref().to_owned();
        let value = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .unwrap_or_default()
            .into_owned();
        if key == "type" {
            group.kind = SparklineKind::parse(&value);
        } else if let Some((_, flag)) = group.flags_mut().into_iter().find(|(n, _)| *n == key) {
            *flag = value == "1" || value == "true";
        } else if !key.starts_with("xmlns") {
            group.attributes.push((key, value));
        }
    }
    group
}

/// The text between two reader positions.
fn slice(text: &str, from: u64, to: u64) -> &str {
    let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
        return "";
    };
    text.get(from..to).unwrap_or_default().trim_start()
}

#[cfg(test)]
mod tests {
    use super::{SparklineKind, read};
    use crate::coordinate::CellRef;
    use crate::style::Color;

    /// As Excel 365 wrote it, next to an extension that is not ours.
    const EXCEL: &str = concat!(
        r#"<extLst xmlns:xr2="http://schemas.microsoft.com/office/spreadsheetml/2015/revision2">"#,
        r#"<ext uri="{78C0D931-6437-407d-A8EE-F0AAD7539E65}"><x14:conditionalFormattings>"#,
        r#"<x14:conditionalFormatting><xm:f>A1</xm:f><xm:sqref>B4:E15</xm:sqref>"#,
        r#"</x14:conditionalFormatting></x14:conditionalFormattings></ext>"#,
        r#"<ext uri="{05C60535-1F16-4fd2-B633-F4F36F0B64E0}" xmlns:x14="x14">"#,
        r#"<x14:sparklineGroups xmlns:xm="xm">"#,
        r#"<x14:sparklineGroup type="column" displayEmptyCellsAs="gap" high="1" xr2:uid="{6322}">"#,
        r#"<x14:colorSeries rgb="FF376092"/><x14:colorNegative theme="5" tint="-0.5"/>"#,
        r#"<xm:f>'A&amp;B'!A1:A10</xm:f>"#,
        r#"<x14:sparklines><x14:sparkline><xm:f>'A&amp;B'!C19:C28</xm:f><xm:sqref>B32</xm:sqref>"#,
        r#"</x14:sparkline><x14:sparkline><xm:sqref>B33</xm:sqref></x14:sparkline></x14:sparklines>"#,
        r#"</x14:sparklineGroup></x14:sparklineGroups></ext></extLst>"#,
    );

    #[test]
    fn a_group_is_read_from_its_own_extension_only() {
        let groups = read(EXCEL);
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert_eq!(group.kind, SparklineKind::Column);
        assert!(group.high && !group.low);
        assert_eq!(
            group.attributes,
            [
                ("displayEmptyCellsAs".to_owned(), "gap".to_owned()),
                ("xr2:uid".to_owned(), "{6322}".to_owned()),
            ]
        );
        assert_eq!(
            group.colors,
            r#"<x14:colorSeries rgb="FF376092"/><x14:colorNegative theme="5" tint="-0.5"/>"#
        );
        // The group's own formula is the date axis; the sheet-level
        // `<xm:f>` of the other extension is not.
        assert_eq!(group.date_axis.as_deref(), Some("'A&B'!A1:A10"));
        let lines = &group.sparklines;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].data.as_deref(), Some("'A&B'!C19:C28"));
        assert_eq!(lines[0].location, CellRef::parse("B32").unwrap_or_default());
        assert_eq!(lines[1].data, None);
    }

    #[test]
    fn colours_and_axis_bounds_are_read_on_request() {
        let mut group = read(EXCEL).remove(0);
        assert_eq!(group.color("colorSeries"), Some(Color::Argb(0xFF37_6092)));
        assert_eq!(
            group.color("colorNegative"),
            Some(Color::Theme {
                id: 5,
                tint: -500_000
            })
        );
        assert_eq!(group.color("colorHigh"), None);
        assert_eq!(group.attribute("displayEmptyCellsAs"), Some("gap"));
        assert_eq!(group.custom_min(), None);
        group.attributes.extend([
            ("minAxisType".into(), "custom".into()),
            ("manualMin".into(), "-2.5".into()),
            ("maxAxisType".into(), "group".into()),
            ("manualMax".into(), "9".into()),
        ]);
        assert_eq!((group.custom_min(), group.custom_max()), (Some(-2.5), None));
    }
}
