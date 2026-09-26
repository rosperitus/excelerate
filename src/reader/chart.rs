//! Reading charts: the frames in a drawing, and the chart parts they point at.
//!
//! A chart part is read as a tree of element slices rather than as a stream of
//! events. Most of what it holds is carried as written (see
//! [`crate::model::chart`]), and a slice of the source is exactly that; the
//! model's own fields are the few children whose names it knows. Names are
//! compared without their prefix: Excel writes `<c:ser>`, excelize writes
//! `<ser>` under a default namespace, and both are the same element.

use crate::coordinate::{Col, Row};
use crate::model::chart::{
    Anchor, AxisKind, AxisMarkup, AxisPosition, BarDirection, Chart, ChartAxis, ChartColor,
    ChartEx, ChartLines, ChartText, ColorBase, ColorTransform, DataLabel, DataLabels, DataPoint,
    DataSource, Dimension, DimensionRole, EditAs, ExSeries, Fill, Grouping, LabelPosition, Legend,
    LegendPosition, LineFormat, Marker, MarkerSymbol, Plot, PlotKind, RadarStyle, ScatterStyle,
    Series, SeriesLayout, SeriesMarker, ShapeFormat, Title, UpDownBars,
};
use core::ops::Range;
use quick_xml::Reader;
use quick_xml::events::Event;

use super::zipxml::is_true;

/// One element of a part, as the slices of the source it spans.
#[derive(Debug)]
pub(crate) struct Node<'a> {
    /// The name without its prefix.
    pub name: &'a str,
    /// The prefix, empty when there is none.
    pub prefix: &'a str,
    /// The start tag, from `<` to `>`, where [`Node::attr`] looks.
    ///
    /// Attributes are read on demand: a drawing of two thousand shapes has
    /// tens of thousands of elements, and copying every attribute of every
    /// one of them into strings was most of what reading it cost.
    pub tag: &'a str,
    /// The whole element, tags included.
    pub outer: &'a str,
    /// What is between the tags; empty for `<x/>`.
    pub inner: &'a str,
    /// Where `outer` starts in the text the node was read from.
    pub span: Range<usize>,
    /// Where `inner` starts in that text.
    pub inner_start: usize,
}

impl<'a> Node<'a> {
    /// An attribute by local name.
    ///
    /// The value as written, entities and all: right for ids, numbers and
    /// names from a fixed list. Free text goes through [`Node::attr_text`].
    pub fn attr(&self, name: &str) -> Option<&'a str> {
        tag_attr(self.tag, name)
    }

    /// An attribute holding free text, with its entities resolved.
    pub fn attr_text(&self, name: &str) -> Option<String> {
        self.attr(name)
            .map(|raw| quick_xml::escape::unescape(raw).map_or_else(|_| raw.to_owned(), Into::into))
    }

    /// The `val` attribute, which is how nearly every chart element states
    /// its one value.
    fn val(&self) -> Option<&str> {
        self.attr("val")
    }

    /// A boolean element: `val` defaults to true, so `<c:delete/>` says yes.
    fn flag(&self) -> bool {
        self.val().is_none_or(is_true)
    }

    /// The element's own children.
    pub fn children(&self) -> Vec<Node<'a>> {
        children(self.inner)
    }

    pub fn child(&self, name: &str) -> Option<Node<'a>> {
        self.children().into_iter().find(|n| n.name == name)
    }

    /// The text inside, with the entities resolved.
    pub fn text(&self) -> String {
        quick_xml::escape::unescape(self.inner).map_or_else(|_| self.inner.to_owned(), Into::into)
    }
}

/// The child elements of a stretch of XML, in order.
///
/// Anything malformed ends the list where it breaks: a chart part comes from
/// an untrusted file, and half a chart is a better answer than an error for
/// the whole workbook.
pub(crate) fn children(xml: &str) -> Vec<Node<'_>> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    loop {
        let start = position(&reader);
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let tag_end = position(&reader);
                let name = e.name();
                let Ok(inner) = reader.read_to_end(name) else {
                    break;
                };
                let end = position(&reader);
                let (Ok(from), Ok(to)) = (usize::try_from(inner.start), usize::try_from(inner.end))
                else {
                    break;
                };
                out.push(node(xml, start..tag_end, start..end, from..to));
            }
            Ok(Event::Empty(_)) => {
                let end = position(&reader);
                out.push(node(xml, start..end, start..end, end..end));
            }
            Ok(Event::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
    out
}

/// An attribute of a start tag by local name: `embed` finds `r:embed`.
pub(crate) fn tag_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let mut from = 0;
    while let Some(found) = tag[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        // The name starts after whitespace or a prefix's colon.
        let start_ok = match at.checked_sub(1).map(|i| bytes[i]) {
            Some(b' ' | b'\t' | b'\r' | b'\n') => true,
            Some(b':') => tag[..at - 1]
                .rfind([' ', '\t', '\r', '\n'])
                .is_some_and(|ws| {
                    tag[ws + 1..at - 1]
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric())
                }),
            _ => false,
        };
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        if !start_ok {
            continue;
        }
        let rest = rest.trim_start();
        let quote = rest.chars().next().filter(|q| matches!(q, '"' | '\''))?;
        let value = &rest[1..];
        return Some(&value[..value.find(quote)?]);
    }
    None
}

fn position(reader: &Reader<&[u8]>) -> usize {
    usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX)
}

fn node(xml: &str, tag: Range<usize>, span: Range<usize>, inner: Range<usize>) -> Node<'_> {
    let qname = &xml[span.start..span.end];
    // The tag name runs from after `<` to the first space, `/` or `>`.
    let full = qname[1..]
        .split([' ', '/', '>', '\t', '\n', '\r'])
        .next()
        .unwrap_or_default();
    let (prefix, name) = full.split_once(':').unwrap_or(("", full));
    Node {
        name,
        prefix,
        tag: xml.get(tag).unwrap_or_default(),
        outer: xml.get(span.clone()).unwrap_or_default(),
        inner: xml.get(inner.clone()).unwrap_or_default(),
        inner_start: inner.start,
        span,
    }
}

/// A chart frame found in a drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    /// The relationship id of the chart part.
    pub rel: String,
    /// The frame's name.
    pub name: String,
    /// The frame's drawing object id, unique within the drawing.
    pub id: u32,
    /// A 2016 chart rather than a classic one.
    pub extended: bool,
    /// Inside a group of shapes.
    pub grouped: bool,
    /// Where the frame's own element sits in the drawing part, which inside a
    /// group is not where the object does.
    pub span: Range<usize>,
}

/// One top-level object of a drawing that holds at least one chart.
#[derive(Debug)]
pub(crate) struct DrawingObject {
    /// Where the object's element sits in the drawing part.
    #[cfg_attr(
        not(feature = "write"),
        expect(dead_code, reason = "only a rewrite of the drawing splices by it")
    )]
    pub span: Range<usize>,
    /// Where it is anchored.
    pub anchor: Option<Anchor>,
    /// The chart frames in it: one for a chart on its own, any number for a
    /// group.
    pub frames: Vec<Frame>,
}

/// The objects of a drawing part that hold charts.
///
/// Pictures and shapes are most of a drawing by volume and nothing here needs
/// them, so an object with no frame in its text is not parsed at all.
pub(crate) fn scan_drawing(xml: &str) -> Vec<DrawingObject> {
    let mut objects = Vec::new();
    let Some(root) = children(xml).into_iter().find(|n| n.name == "wsDr") else {
        return objects;
    };
    for child in root.children() {
        if !child.inner.contains("graphicFrame") {
            continue;
        }
        let kids = child.children();
        let mut frames = Vec::new();
        find_frames(
            &kids,
            root.inner_start + child.inner_start,
            false,
            &mut frames,
        );
        if frames.is_empty() {
            continue;
        }
        objects.push(DrawingObject {
            span: root.inner_start + child.span.start..root.inner_start + child.span.end,
            anchor: read_anchor(&child, &kids),
            frames,
        });
    }
    objects
}

pub(crate) fn read_anchor(node: &Node<'_>, kids: &[Node<'_>]) -> Option<Anchor> {
    let find = |name: &str| kids.iter().find(|n| n.name == name);
    let ext = |n: Option<&Node<'_>>| {
        let size = |k| n.and_then(|n| n.attr(k)).and_then(|v| v.parse().ok());
        (size("cx").unwrap_or(0), size("cy").unwrap_or(0))
    };
    match node.name {
        "twoCellAnchor" => Some(Anchor::TwoCell {
            from: marker(find("from")?),
            to: marker(find("to")?),
            edit_as: node.attr("editAs").and_then(EditAs::parse),
        }),
        "oneCellAnchor" => {
            let (width, height) = ext(find("ext"));
            Some(Anchor::OneCell {
                from: marker(find("from")?),
                width,
                height,
            })
        }
        "absoluteAnchor" => {
            let pos = find("pos");
            let at = |k| pos.and_then(|n| n.attr(k)).and_then(|v| v.parse().ok());
            let (width, height) = ext(find("ext"));
            Some(Anchor::Absolute {
                x: at("x").unwrap_or(0),
                y: at("y").unwrap_or(0),
                width,
                height,
            })
        }
        _ => None,
    }
}

fn marker(node: &Node<'_>) -> Marker {
    let mut out = Marker::default();
    for child in node.children() {
        let text = child.text();
        let text = text.trim();
        match child.name {
            "col" => {
                out.col = text.parse().ok().and_then(Col::new).unwrap_or_default();
            }
            "row" => {
                out.row = text.parse().ok().and_then(Row::new).unwrap_or_default();
            }
            "colOff" => out.col_offset = text.parse().unwrap_or(0),
            "rowOff" => out.row_offset = text.parse().unwrap_or(0),
            _ => {}
        }
    }
    out
}

/// Collects the chart frames among `kids`, looking into groups and into the
/// preferred branch of `mc:AlternateContent`, which is where a 2016 chart
/// hides from readers that do not know it.
///
/// `base` is where the text the nodes were read from starts in the drawing
/// part, so the spans recorded are the part's own.
fn find_frames(kids: &[Node<'_>], base: usize, grouped: bool, out: &mut Vec<Frame>) {
    for child in kids {
        let inner = base + child.inner_start;
        match child.name {
            "grpSp" => find_frames(&child.children(), inner, true, out),
            "AlternateContent" => {
                if let Some(choice) = child.child("Choice") {
                    find_frames(&choice.children(), inner + choice.inner_start, grouped, out);
                }
            }
            "graphicFrame" => {
                let props = child
                    .child("nvGraphicFramePr")
                    .and_then(|n| n.child("cNvPr"));
                let Some(data) = child.child("graphic").and_then(|g| g.child("graphicData")) else {
                    continue;
                };
                let Some(rel) = data
                    .child("chart")
                    .and_then(|c| c.attr("id").map(str::to_owned))
                else {
                    continue;
                };
                out.push(Frame {
                    rel,
                    name: props
                        .as_ref()
                        .and_then(|p| p.attr_text("name"))
                        .unwrap_or_default(),
                    id: props
                        .as_ref()
                        .and_then(|p| p.attr("id"))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    extended: data.attr("uri").is_some_and(|u| u.contains("chartex")),
                    grouped,
                    span: base + child.span.start..base + child.span.end,
                });
            }
            _ => {}
        }
    }
}

/// Reads a chart part into a chart with no frame yet.
///
/// Returns the chart, the attributes of its root element and the prefix the
/// chart namespace goes by, which a rewrite needs to keep the carried markup
/// meaning what it meant.
pub(crate) fn read_chart(xml: &str) -> Option<(Chart, String, String)> {
    let root = children(xml).into_iter().find(|n| n.name == "chartSpace")?;
    let attributes = root_attributes(&root);
    let mut out = Chart::default();
    let mut seen_chart = false;
    for child in root.children() {
        if child.name == "chart" {
            seen_chart = true;
            read_chart_body(&child, &mut out);
        } else if seen_chart {
            out.markup.after_chart.push_str(child.outer);
        } else {
            out.markup.before_chart.push_str(child.outer);
        }
    }
    seen_chart.then(|| (out, attributes, root.prefix.to_owned()))
}

/// The attribute text of an element's start tag, as written.
fn root_attributes(node: &Node<'_>) -> String {
    let open = node.outer.find('>').unwrap_or(node.outer.len());
    let tag = node.outer[1..open].trim_end_matches('/');
    tag.split_once([' ', '\t', '\n', '\r'])
        .map(|(_, rest)| rest.trim().to_owned())
        .unwrap_or_default()
}

fn read_chart_body(chart: &Node<'_>, out: &mut Chart) {
    let mut past_plot = false;
    for child in chart.children() {
        match child.name {
            "title" => out.title = Some(read_title(&child)),
            "autoTitleDeleted" => out.auto_title_deleted = child.flag(),
            "plotArea" => {
                past_plot = true;
                read_plot_area(&child, out);
            }
            "legend" => out.legend = Some(read_legend(&child)),
            _ if past_plot => out.markup.after_legend.push_str(child.outer),
            _ => out.markup.before_plot_area.push_str(child.outer),
        }
    }
}

fn read_plot_area(area: &Node<'_>, out: &mut Chart) {
    for child in area.children() {
        if child.name == "layout" {
            child.outer.clone_into(&mut out.markup.plot_area_layout);
        } else if let Some(plot) = read_plot(&child) {
            out.plots.push(plot);
        } else if let Some(axis) = read_axis(&child) {
            out.axes.push(axis);
        } else {
            out.markup.after_axes.push_str(child.outer);
        }
    }
}

fn read_title(node: &Node<'_>) -> Title {
    let mut out = Title::default();
    for child in node.children() {
        if child.name == "tx" {
            out.text = read_text(&child);
        } else {
            out.markup.push_str(child.outer);
        }
    }
    out
}

/// Reads `<c:tx>`: a cell reference, formatted text, or a plain value.
fn read_text(tx: &Node<'_>) -> Option<ChartText> {
    let child = tx.children().into_iter().next()?;
    Some(match child.name {
        "strRef" => ChartText::Reference {
            formula: child.child("f").map(|f| f.text()).unwrap_or_default(),
            cache: child
                .child("strCache")
                .and_then(|c| strings(&c).1.into_iter().next())
                .map(|(_, v)| v),
        },
        "rich" => ChartText::Text {
            text: rich_text(child.outer),
            rich: Some(child.outer.to_owned()),
        },
        "v" => ChartText::text(child.text()),
        _ => return None,
    })
}

/// The words of a formatted text element, paragraphs separated by `\n`.
pub(crate) fn rich_text(rich: &str) -> String {
    let Some(root) = children(rich).into_iter().next() else {
        return String::new();
    };
    root.children()
        .iter()
        .filter(|p| p.name == "p")
        .map(|p| {
            p.children()
                .iter()
                .filter(|r| matches!(r.name, "r" | "fld"))
                .filter_map(|r| r.child("t"))
                .map(|t| t.text())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_legend(node: &Node<'_>) -> Legend {
    let mut out = Legend::default();
    for child in node.children() {
        if child.name == "legendPos" {
            out.position = LegendPosition::parse(child.val().unwrap_or_default());
        } else {
            out.markup.push_str(child.outer);
        }
    }
    out
}

fn read_plot(node: &Node<'_>) -> Option<Plot> {
    let kids = node.children();
    let val = |name: &str| {
        kids.iter()
            .find(|n| n.name == name)
            .and_then(|n| n.val())
            .unwrap_or_default()
    };
    let flag = |name: &str| kids.iter().find(|n| n.name == name).is_some_and(Node::flag);
    let grouping = |default| {
        let v = val("grouping");
        if v.is_empty() {
            default
        } else {
            Grouping::parse(v)
        }
    };
    let bar = |three_d| PlotKind::Bar {
        direction: if val("barDir") == "bar" {
            BarDirection::Bar
        } else {
            BarDirection::Column
        },
        grouping: grouping(Grouping::Clustered),
        three_d,
    };
    let kind = match node.name {
        "barChart" => bar(false),
        "bar3DChart" => bar(true),
        "lineChart" | "line3DChart" => PlotKind::Line {
            grouping: grouping(Grouping::Standard),
            three_d: node.name == "line3DChart",
        },
        "areaChart" | "area3DChart" => PlotKind::Area {
            grouping: grouping(Grouping::Standard),
            three_d: node.name == "area3DChart",
        },
        "pieChart" => PlotKind::Pie { three_d: false },
        "pie3DChart" => PlotKind::Pie { three_d: true },
        "doughnutChart" => PlotKind::Doughnut,
        "ofPieChart" => PlotKind::OfPie {
            bar: val("ofPieType") == "bar",
        },
        "scatterChart" => PlotKind::Scatter(ScatterStyle::parse(val("scatterStyle"))),
        "radarChart" => PlotKind::Radar(RadarStyle::parse(val("radarStyle"))),
        "bubbleChart" => PlotKind::Bubble,
        "stockChart" => PlotKind::Stock,
        "surfaceChart" | "surface3DChart" => PlotKind::Surface {
            three_d: node.name == "surface3DChart",
            wireframe: flag("wireframe"),
        },
        _ => return None,
    };
    let mut out = Plot::new(kind);
    for child in kids {
        match child.name {
            "barDir" | "grouping" | "ofPieType" | "scatterStyle" | "radarStyle" | "wireframe" => {}
            "varyColors" => out.vary_colors = Some(child.flag()),
            "ser" => out.series.push(read_series(&child)),
            "dLbls" => out.labels = Some(read_labels(&child)),
            "dropLines" => out.drop_lines = Some(read_lines(&child)),
            "hiLowLines" => out.high_low_lines = Some(read_lines(&child)),
            "upDownBars" => out.up_down_bars = Some(read_up_down_bars(&child)),
            "axId" => {
                if let Some(id) = child.val().and_then(|v| v.parse().ok()) {
                    out.axis_ids.push(id);
                }
            }
            _ => out.markup.push_str(child.outer),
        }
    }
    Some(out)
}

fn read_series(node: &Node<'_>) -> Series {
    let mut out = Series::default();
    let mut past_data = false;
    for child in node.children() {
        let number = || child.val().and_then(|v| v.parse().ok()).unwrap_or(0);
        match child.name {
            "idx" => out.index = number(),
            "order" => out.order = number(),
            "tx" => out.name = read_text(&child),
            "cat" | "xVal" => {
                past_data = true;
                out.categories = read_data(&child);
            }
            "val" | "yVal" => {
                past_data = true;
                out.values = read_data(&child);
            }
            "bubbleSize" => {
                past_data = true;
                out.bubble_sizes = read_data(&child);
            }
            _ if past_data => out.markup.after_data.push_str(child.outer),
            "spPr" => out.format = Some(read_shape_format(&child)),
            "marker" => out.marker = Some(read_marker(&child)),
            "dPt" => out.data_points.push(read_point(&child)),
            "dLbls" => out.labels = Some(read_labels(&child)),
            // Named rather than placed: the schema puts these between the fill
            // and the marker, and so does the writer, even when a file (excelize)
            // wrote them elsewhere.
            "invertIfNegative" | "pictureOptions" | "explosion" => {
                out.markup.after_format.push_str(child.outer);
            }
            _ => out.markup.before_data.push_str(child.outer),
        }
    }
    out
}

/// The fill elements of `DrawingML`, one of which a shape or line may hold.
pub(crate) const FILLS: [&str; 6] = [
    "noFill",
    "solidFill",
    "gradFill",
    "blipFill",
    "pattFill",
    "grpFill",
];

/// Reads `<c:spPr>`, keeping the element as its source.
pub(crate) fn read_shape_format(node: &Node<'_>) -> ShapeFormat {
    let kids = node.children();
    ShapeFormat {
        fill: kids.iter().find(|n| FILLS.contains(&n.name)).map(read_fill),
        line: kids.iter().find(|n| n.name == "ln").map(read_line),
        source: Some(node.outer.to_owned()),
    }
}

/// Reads one fill element.
pub(crate) fn read_fill(node: &Node<'_>) -> Fill {
    match node.name {
        "noFill" => Fill::None,
        "solidFill" => node
            .children()
            .first()
            .and_then(read_color)
            .map_or(Fill::Other, Fill::Solid),
        _ => Fill::Other,
    }
}

fn read_color(node: &Node<'_>) -> Option<ChartColor> {
    let hex = |v: &str| u32::from_str_radix(v, 16).ok().filter(|_| v.len() == 6);
    let base = match node.name {
        "srgbClr" => ColorBase::Rgb(hex(node.val()?)?),
        "sysClr" => ColorBase::Rgb(hex(node.attr("lastClr")?)?),
        "schemeClr" => ColorBase::Scheme(node.val()?.to_owned()),
        _ => return None,
    };
    let transforms = node
        .children()
        .iter()
        .filter_map(|t| ColorTransform::parse(t.name, t.val()?.parse().ok()?))
        .collect();
    Some(ChartColor { base, transforms })
}

/// Reads `<a:ln>`.
pub(crate) fn read_line(node: &Node<'_>) -> LineFormat {
    LineFormat {
        fill: node
            .children()
            .iter()
            .find(|n| FILLS.contains(&n.name))
            .map(read_fill),
        width: node.attr("w").and_then(|w| w.parse().ok()),
    }
}

/// Reads `<c:marker>`.
pub(crate) fn read_marker(node: &Node<'_>) -> SeriesMarker {
    let kids = node.children();
    let val = |name: &str| kids.iter().find(|n| n.name == name).and_then(Node::val);
    SeriesMarker {
        symbol: val("symbol").and_then(MarkerSymbol::parse),
        size: val("size").and_then(|v| v.parse().ok()),
        format: kids
            .iter()
            .find(|n| n.name == "spPr")
            .map(read_shape_format),
        source: Some(node.outer.to_owned()),
    }
}

/// Reads `<c:dPt>`.
pub(crate) fn read_point(node: &Node<'_>) -> DataPoint {
    let kids = node.children();
    DataPoint {
        index: kids
            .iter()
            .find(|n| n.name == "idx")
            .and_then(Node::val)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        format: kids
            .iter()
            .find(|n| n.name == "spPr")
            .map(read_shape_format),
        source: Some(node.outer.to_owned()),
    }
}

/// Reads `<c:dropLines>` or `<c:hiLowLines>`.
fn read_lines(node: &Node<'_>) -> ChartLines {
    ChartLines {
        format: node.child("spPr").as_ref().map(read_shape_format),
    }
}

/// Reads `<c:upDownBars>`.
pub(crate) fn read_up_down_bars(node: &Node<'_>) -> UpDownBars {
    let kids = node.children();
    let find = |name: &str| kids.iter().find(|n| n.name == name);
    let bars = |name: &str| {
        find(name)
            .and_then(|n| n.child("spPr"))
            .as_ref()
            .map(read_shape_format)
    };
    UpDownBars {
        gap_width: find("gapWidth")
            .and_then(Node::val)
            .and_then(|v| v.parse().ok()),
        up: bars("upBars"),
        down: bars("downBars"),
        source: Some(node.outer.to_owned()),
    }
}

/// Reads `<c:dLbls>`. A flag that is not there is off, as Excel reads it.
pub(crate) fn read_labels(node: &Node<'_>) -> DataLabels {
    let kids = node.children();
    let find = |name: &str| kids.iter().find(|n| n.name == name);
    let flag = |name: &str| find(name).is_some_and(Node::flag);
    DataLabels {
        points: kids
            .iter()
            .filter(|n| n.name == "dLbl")
            .map(read_label)
            .collect(),
        deleted: flag("delete"),
        position: find("dLblPos")
            .and_then(Node::val)
            .and_then(LabelPosition::parse),
        show_legend_key: flag("showLegendKey"),
        show_value: flag("showVal"),
        show_category_name: flag("showCatName"),
        show_series_name: flag("showSerName"),
        show_percent: flag("showPercent"),
        source: Some(node.outer.to_owned()),
    }
}

/// Reads `<c:dLbl>`: the same switches as `<c:dLbls>`, for one point.
pub(crate) fn read_label(node: &Node<'_>) -> DataLabel {
    let DataLabels {
        points: _,
        deleted,
        position,
        show_legend_key,
        show_value,
        show_category_name,
        show_series_name,
        show_percent,
        source,
    } = read_labels(node);
    DataLabel {
        index: node
            .child("idx")
            .and_then(|n| n.val().and_then(|v| v.parse().ok()))
            .unwrap_or(0),
        deleted,
        position,
        show_legend_key,
        show_value,
        show_category_name,
        show_series_name,
        show_percent,
        source,
    }
}

fn read_data(node: &Node<'_>) -> Option<DataSource> {
    let source = node.children().into_iter().next()?;
    let formula = source.child("f").map(|f| f.text());
    Some(match source.name {
        "numRef" | "numLit" => {
            let cache = if source.name == "numRef" {
                source.child("numCache")
            } else {
                Some(Node { ..source })
            };
            let (format_code, count, points) = cache.map(|c| numbers(&c)).unwrap_or_default();
            DataSource::Numbers {
                formula,
                format_code,
                count,
                points,
            }
        }
        "strRef" | "strLit" => {
            let cache = if source.name == "strRef" {
                source.child("strCache")
            } else {
                Some(Node { ..source })
            };
            let (count, points) = cache.map(|c| strings(&c)).unwrap_or_default();
            DataSource::Strings {
                formula,
                count,
                points,
            }
        }
        "multiLvlStrRef" => DataSource::Levels {
            formula,
            markup: source.outer.to_owned(),
        },
        _ => return None,
    })
}

type NumberCache = (Option<String>, Option<u32>, Vec<(u32, f64)>);

fn numbers(cache: &Node<'_>) -> NumberCache {
    let mut format = None;
    let mut count = None;
    let mut points = Vec::new();
    for child in cache.children() {
        match child.name {
            "formatCode" => format = Some(child.text()),
            "ptCount" => count = child.val().and_then(|v| v.parse().ok()),
            "pt" => {
                let idx = child.attr("idx").and_then(|v| v.parse().ok());
                let value = child.child("v").and_then(|v| v.text().trim().parse().ok());
                if let (Some(idx), Some(value)) = (idx, value) {
                    points.push((idx, value));
                }
            }
            _ => {}
        }
    }
    (format, count, points)
}

fn strings(cache: &Node<'_>) -> (Option<u32>, Vec<(u32, String)>) {
    let mut count = None;
    let mut points = Vec::new();
    for child in cache.children() {
        match child.name {
            "ptCount" => count = child.val().and_then(|v| v.parse().ok()),
            "pt" => {
                if let Some(idx) = child.attr("idx").and_then(|v| v.parse().ok()) {
                    points.push((idx, child.child("v").map(|v| v.text()).unwrap_or_default()));
                }
            }
            _ => {}
        }
    }
    (count, points)
}

fn read_axis(node: &Node<'_>) -> Option<ChartAxis> {
    let kind = match node.name {
        "catAx" => AxisKind::Category,
        "valAx" => AxisKind::Value,
        "dateAx" => AxisKind::Date,
        "serAx" => AxisKind::Series,
        _ => return None,
    };
    let mut out = ChartAxis::new(kind, 0, 0, AxisPosition::Bottom);
    let mut markup = AxisMarkup::default();
    let mut past_cross = false;
    for child in node.children() {
        let number = || child.val().and_then(|v| v.parse().ok());
        match child.name {
            "axId" => out.id = number().unwrap_or(0),
            "scaling" => {
                for s in child.children() {
                    let value = || s.val().and_then(|v| v.parse().ok());
                    match s.name {
                        "orientation" => out.reversed = s.val() == Some("maxMin"),
                        "min" => out.min = value(),
                        "max" => out.max = value(),
                        "logBase" => out.log_base = value(),
                        _ => {}
                    }
                }
            }
            "delete" => out.deleted = child.flag(),
            "axPos" => out.position = AxisPosition::parse(child.val().unwrap_or_default()),
            "majorGridlines" | "minorGridlines" => markup.gridlines.push_str(child.outer),
            "title" => out.title = Some(read_title(&child)),
            "numFmt" => {
                out.number_format = Some((
                    child.attr_text("formatCode").unwrap_or_default(),
                    child.attr("sourceLinked").is_some_and(is_true),
                ));
            }
            "crossAx" => {
                past_cross = true;
                out.cross_axis = number().unwrap_or(0);
            }
            _ if past_cross => markup.tail.push_str(child.outer),
            _ => markup.ticks.push_str(child.outer),
        }
    }
    out.markup = markup;
    Some(out)
}

/// Reads a 2016 chart part into a chart with no frame yet.
pub(crate) fn read_chart_ex(xml: &str) -> Option<ChartEx> {
    let root = children(xml).into_iter().find(|n| n.name == "chartSpace")?;
    let mut data: Vec<(String, Vec<Dimension>)> = Vec::new();
    let mut out = ChartEx::default();
    for child in root.children() {
        match child.name {
            "chartData" => {
                for set in child.children().iter().filter(|n| n.name == "data") {
                    let id = set.attr("id").unwrap_or_default().to_owned();
                    let dims = set.children().iter().filter_map(read_dimension).collect();
                    data.push((id, dims));
                }
            }
            "chart" => {
                for part in child.children() {
                    match part.name {
                        "title" => out.title = part.child("tx").and_then(|t| read_ex_text(&t)),
                        "plotArea" => {
                            let regions = part.children();
                            let series = regions
                                .iter()
                                .filter(|n| n.name == "plotAreaRegion")
                                .flat_map(Node::children)
                                .filter(|n| n.name == "series");
                            for s in series {
                                let id = s.child("dataId").and_then(|d| d.val().map(str::to_owned));
                                out.series.push(ExSeries {
                                    layout: SeriesLayout::parse(
                                        s.attr("layoutId").unwrap_or_default(),
                                    ),
                                    name: s.child("tx").and_then(|t| read_ex_text(&t)),
                                    hidden: s.attr("hidden").is_some_and(is_true),
                                    dimensions: data
                                        .iter()
                                        .find(|(d, _)| Some(d) == id.as_ref())
                                        .map(|(_, dims)| dims.clone())
                                        .unwrap_or_default(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// Reads `<cx:tx>`: a formula with the value it had, or formatted text.
fn read_ex_text(tx: &Node<'_>) -> Option<ChartText> {
    let child = tx.children().into_iter().next()?;
    match child.name {
        "txData" => {
            let value = child.child("v").map(|v| v.text());
            Some(match child.child("f") {
                Some(f) => ChartText::Reference {
                    formula: f.text(),
                    cache: value,
                },
                None => ChartText::text(value.unwrap_or_default()),
            })
        }
        "rich" => Some(ChartText::Text {
            text: rich_text(child.outer),
            rich: Some(child.outer.to_owned()),
        }),
        _ => None,
    }
}

fn read_dimension(node: &Node<'_>) -> Option<Dimension> {
    let numeric = match node.name {
        "numDim" => true,
        "strDim" => false,
        _ => return None,
    };
    let role = DimensionRole::parse(node.attr("type")?)?;
    let mut out = Dimension {
        role,
        numeric,
        formula: None,
        levels: Vec::new(),
    };
    for child in node.children() {
        match child.name {
            "f" => out.formula = Some(child.text()),
            "lvl" => out.levels.push(
                child
                    .children()
                    .iter()
                    .filter(|p| p.name == "pt")
                    .filter_map(|p| Some((p.attr("idx")?.parse().ok()?, p.text())))
                    .collect(),
            ),
            _ => {}
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_are_sliced_from_the_source() {
        let xml = r#"<c:a x="1"><c:b/><c:c>t&amp;u</c:c></c:a>"#;
        let root = &children(xml)[0];
        assert_eq!((root.prefix, root.name), ("c", "a"));
        assert_eq!(root.attr("x"), Some("1"));
        let kids = root.children();
        assert_eq!(kids[0].outer, "<c:b/>");
        assert_eq!(kids[1].text(), "t&u");
        assert_eq!(&xml[root.inner_start..][..6], "<c:b/>");
    }

    #[test]
    fn a_rich_title_reads_as_its_words() {
        let rich = concat!(
            r#"<c:rich><a:bodyPr/><a:p><a:r><a:rPr b="1"/><a:t>Sales </a:t></a:r>"#,
            r#"<a:r><a:t>2024</a:t></a:r></a:p><a:p><a:r><a:t>by region</a:t></a:r></a:p></c:rich>"#,
        );
        assert_eq!(rich_text(rich), "Sales 2024\nby region");
    }

    #[test]
    fn a_frame_inside_a_group_is_marked() {
        let xml = concat!(
            r#"<xdr:wsDr><xdr:twoCellAnchor editAs="oneCell"><xdr:from><xdr:col>1</xdr:col>"#,
            r#"<xdr:colOff>5</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>"#,
            r#"<xdr:to><xdr:col>4</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>9</xdr:row>"#,
            r#"<xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:grpSp><xdr:graphicFrame>"#,
            r#"<xdr:nvGraphicFramePr><xdr:cNvPr id="7" name="Chart 6"/></xdr:nvGraphicFramePr>"#,
            r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">"#,
            r#"<c:chart r:id="rId3"/></a:graphicData></a:graphic></xdr:graphicFrame></xdr:grpSp>"#,
            r#"</xdr:twoCellAnchor></xdr:wsDr>"#,
        );
        let objects = scan_drawing(xml);
        let object = &objects[0];
        assert!(xml[object.span.clone()].starts_with("<xdr:twoCellAnchor"));
        assert!(xml[object.span.clone()].ends_with("</xdr:twoCellAnchor>"));
        let frame = &object.frames[0];
        assert_eq!(
            (frame.rel.as_str(), frame.name.as_str(), frame.id),
            ("rId3", "Chart 6", 7)
        );
        assert!(frame.grouped && !frame.extended);
        // Inside a group the frame has a span of its own.
        assert!(xml[frame.span.clone()].starts_with("<xdr:graphicFrame>"));
        assert!(xml[frame.span.clone()].ends_with("</xdr:graphicFrame>"));
        let Some(Anchor::TwoCell { from, edit_as, .. }) = object.anchor else {
            panic!("a two-cell anchor");
        };
        assert_eq!(
            (from.col.index(), from.row.index(), from.col_offset),
            (1, 2, 5)
        );
        assert_eq!(edit_as, Some(EditAs::OneCell));
    }
}
