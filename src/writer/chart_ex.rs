//! Writing 2016 charts: an untouched one as the bytes it came in, a changed
//! one edited inside its part, a new one rendered with Excel's defaults.
//!
//! Runs after [`super::chart`], over the drawings it left. A `chartEx` part is
//! mostly formatting the model does not name - a colour per data point, label
//! styles, axis text - so a changed chart is not rendered but spliced: the
//! title's text, each series' layout, name, visibility and data, and nothing
//! else. See [`crate::model::chart::ChartEx`].

use super::chart::{
    DRAWING_NS, DRAWING_TYPE, MAIN_NS, REL_NS, XML_DECL, drawings, edit_relationships,
    max_object_id, part_text, relationships, render_anchor, set_part,
};
use super::image::{free_path, reanchor, set_attribute, splice};
use super::xmlesc::escape;
use crate::error::{Error, Result};
use crate::model::chart::{ChartEx, ChartText, Dimension, ExSeries, SeriesLayout};
use crate::model::{Attachment, Spreadsheet, Worksheet};
use crate::reader::chart::{Node, children, read_chart_ex, rich_text, scan_drawing};
use core::ops::Range;
use std::borrow::Cow;
use std::fmt::Write as _;

const CHART_EX_NS: &str = "http://schemas.microsoft.com/office/drawing/2014/chartex";
const CHART_EX_TYPE: &str = "application/vnd.ms-office.chartex+xml";
const CHART_EX_REL: &str = "http://schemas.microsoft.com/office/2014/relationships/chartEx";

/// The workbook as the writer should see it, 2016 charts applied.
///
/// # Errors
/// [`Error::Xlsx`] for a series whose layout this crate cannot name.
pub(super) fn prepare(book: Cow<'_, Spreadsheet>) -> Result<Cow<'_, Spreadsheet>> {
    let dirty: Vec<usize> = book
        .sheets()
        .iter()
        .enumerate()
        .filter(|(_, sheet)| {
            sheet.extended_charts.iter().any(|c| !c.is_unchanged())
                || drawings(sheet).any(|d| lost_frames(&book, sheet, d))
        })
        .map(|(i, _)| i)
        .collect();
    if dirty.is_empty() {
        return Ok(book);
    }
    for sheet in dirty.iter().filter_map(|&i| book.sheet(i)) {
        for chart in &sheet.extended_charts {
            let changed = chart
                .origin
                .as_ref()
                .is_none_or(|o| o.read.series != chart.series);
            if let Some(series) = chart.series.iter().find(|s| s.layout.as_str().is_none())
                && changed
            {
                return Err(Error::Xlsx(format!(
                    "chart {:?}: a series layout of {:?} cannot be written",
                    chart.name, series.layout
                )));
            }
        }
    }
    let mut book = book.into_owned();
    for index in dirty {
        apply_sheet(&mut book, index);
    }
    Ok(Cow::Owned(book))
}

/// Whether a drawing shows a 2016 chart the sheet no longer has.
fn lost_frames(book: &Spreadsheet, sheet: &Worksheet, drawing: &str) -> bool {
    relationships(book, drawing)
        .iter()
        .filter(|r| r.kind == CHART_EX_REL)
        .any(|r| {
            claimant(&sheet.extended_charts, drawing, &r.target).is_none()
                && is_modelled(book, &r.target)
        })
}

fn claimant(charts: &[ChartEx], drawing: &str, part: &str) -> Option<usize> {
    charts
        .iter()
        .position(|c| c.part == part && c.origin.as_ref().is_some_and(|o| o.drawing == drawing))
}

/// Whether the reader turned the part into the model; one it could not read
/// never reached the sheet, so its absence there is no removal.
fn is_modelled(book: &Spreadsheet, part: &str) -> bool {
    part_text(book, part).is_some_and(|xml| read_chart_ex(xml).is_some())
}

fn apply_sheet(book: &mut Spreadsheet, index: usize) {
    let Some(sheet) = book.sheet(index) else {
        return;
    };
    let charts = sheet.extended_charts.clone();
    let mut paths: Vec<String> = drawings(sheet).map(str::to_owned).collect();
    if paths.is_empty() {
        if charts.iter().all(|c| c.origin.is_some()) {
            return;
        }
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
    // The first drawing takes the new charts.
    for (i, path) in paths.iter().enumerate() {
        rewrite_drawing(book, path, &charts, i == 0);
    }
}

fn rewrite_drawing(book: &mut Spreadsheet, path: &str, charts: &[ChartEx], takes_new: bool) {
    let Some(xml) = part_text(book, path).map(str::to_owned) else {
        return;
    };
    let rels = relationships(book, path);
    let mut splices: Vec<(Range<usize>, String)> = Vec::new();
    let mut removed: Vec<String> = Vec::new();

    for object in scan_drawing(&xml) {
        let alone = matches!(object.frames.as_slice(), [f] if !f.grouped);
        let original = &xml[object.span.clone()];
        let mut local: Vec<(Range<usize>, String)> = Vec::new();
        let mut moved_to = None;
        for frame in object.frames.iter().filter(|f| f.extended) {
            let Some(target) = rels
                .iter()
                .find(|r| r.id == frame.rel)
                .map(|r| r.target.clone())
            else {
                continue;
            };
            let Some(i) = claimant(charts, path, &target) else {
                // ponytail: a removed chart inside a group keeps its frame;
                // cutting it means finding the `mc:AlternateContent` around it.
                if alone && is_modelled(book, &target) {
                    splices.push((object.span.clone(), String::new()));
                    removed.push(frame.rel.clone());
                }
                continue;
            };
            let chart = &charts[i];
            let Some(origin) = &chart.origin else {
                continue;
            };
            if !chart.same_content(&origin.read)
                && let Some(part) = part_text(book, &target)
            {
                let text = rewrite_part(part, chart, &origin.read);
                set_part(book, &target, Some(CHART_EX_TYPE), text);
            }
            if chart.name != origin.read.name {
                let span = frame.span.start - object.span.start..frame.span.end - object.span.start;
                local.push((span.clone(), rename_frame(&original[span], &chart.name)));
            }
            // A frame inside a group is placed by the group.
            if alone && chart.anchor != origin.read.anchor {
                moved_to = Some(chart.anchor);
            }
        }
        if local.is_empty() && moved_to.is_none() {
            continue;
        }
        let text = splice(original, local);
        let text = match moved_to {
            Some(anchor) => reanchor(&text, anchor),
            None => text,
        };
        splices.push((object.span.clone(), text));
    }

    let mut added: Vec<(String, String)> = Vec::new();
    if takes_new {
        let mut next_id = max_object_id(&xml);
        let mut objects = String::new();
        for chart in charts.iter().filter(|c| c.origin.is_none()) {
            next_id = next_id.saturating_add(1);
            let part = free_path(book, "xl/charts/chartEx", "xml");
            set_part(book, &part, Some(CHART_EX_TYPE), render_part(chart));
            let rel = (1..=u32::MAX)
                .map(|n| format!("rId{n}"))
                .find(|id| !rels.iter().any(|r| r.id == *id) && !added.iter().any(|a| a.0 == *id))
                .unwrap_or_default();
            objects.push_str(&render_anchor(
                chart.anchor,
                &render_frame(chart, next_id, &rel),
            ));
            added.push((rel, part));
        }
        if !objects.is_empty() {
            let close = xml.rfind("</").unwrap_or(xml.len());
            splices.push((close..close, objects));
        }
    }
    if splices.is_empty() {
        return;
    }
    set_part(book, path, Some(DRAWING_TYPE), splice(&xml, splices));
    edit_relationships(book, path, &removed, &added, CHART_EX_REL);
}

/// A frame element with its `cNvPr` name replaced.
fn rename_frame(frame: &str, name: &str) -> String {
    let Some(start) = frame.find("cNvPr ") else {
        return frame.to_owned();
    };
    let Some(end) = frame[start..].find('>').map(|e| start + e) else {
        return frame.to_owned();
    };
    let tag = set_attribute(&frame[start..end], "name", name);
    format!("{}{tag}{}", &frame[..start], &frame[end..])
}

/// The frame of a new chart, behind the `mc:AlternateContent` switch a 2016
/// chart needs so older readers skip it.
fn render_frame(chart: &ChartEx, id: u32, rel: &str) -> String {
    let layout = chart.series.first().map(|s| s.layout);
    // Funnels and maps came after the first 2016 charts, and Excel asks for
    // the namespace that introduced them.
    let (prefix, requires) = match layout {
        Some(SeriesLayout::Funnel) => (
            "cx2",
            "http://schemas.microsoft.com/office/drawing/2015/10/21/chartex",
        ),
        Some(SeriesLayout::RegionMap) => (
            "cx4",
            "http://schemas.microsoft.com/office/drawing/2016/5/10/chartex",
        ),
        _ => (
            "cx1",
            "http://schemas.microsoft.com/office/drawing/2015/9/8/chartex",
        ),
    };
    let name = if chart.name.is_empty() {
        format!("Chart {id}")
    } else {
        chart.name.clone()
    };
    format!(
        concat!(
            r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">"#,
            r#"<mc:Choice xmlns:{prefix}="{requires}" Requires="{prefix}">"#,
            r#"<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{id}" name="{name}"/>"#,
            r#"<xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>"#,
            r#"<xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm>"#,
            r#"<a:graphic><a:graphicData uri="{ns}">"#,
            r#"<cx:chart xmlns:cx="{ns}" r:id="{rel}"/></a:graphicData></a:graphic>"#,
            r#"</xdr:graphicFrame></mc:Choice></mc:AlternateContent><xdr:clientData/>"#
        ),
        prefix = prefix,
        requires = requires,
        id = id,
        name = escape(&name),
        ns = CHART_EX_NS,
        rel = escape(rel),
    )
}

/// A chart part for a chart made in code.
fn render_part(chart: &ChartEx) -> String {
    let p = "cx:";
    let mut s = format!(
        r#"{XML_DECL}<cx:chartSpace xmlns:a="{MAIN_NS}" xmlns:r="{REL_NS}" xmlns:cx="{CHART_EX_NS}"><cx:chartData>"#
    );
    for (i, series) in chart.series.iter().enumerate() {
        s.push_str(&render_data(i, &series.dimensions, p));
    }
    s.push_str("</cx:chartData><cx:chart>");
    if let Some(title) = &chart.title {
        s.push_str(&render_title(title, p));
    }
    s.push_str("<cx:plotArea><cx:plotAreaRegion>");
    for (i, series) in chart.series.iter().enumerate() {
        s.push_str(&render_series(i, series, p));
    }
    s.push_str("</cx:plotAreaRegion>");
    match chart.series.first().map(|s| s.layout) {
        Some(
            layout @ (SeriesLayout::Waterfall
            | SeriesLayout::ClusteredColumn
            | SeriesLayout::BoxWhisker
            | SeriesLayout::ParetoLine),
        ) => {
            let gap = if layout == SeriesLayout::ClusteredColumn {
                "0"
            } else {
                "0.5"
            };
            let _ = write!(
                s,
                concat!(
                    r#"<cx:axis id="0"><cx:catScaling gapWidth="{gap}"/><cx:tickLabels/></cx:axis>"#,
                    r#"<cx:axis id="1"><cx:valScaling/><cx:majorGridlines/><cx:tickLabels/></cx:axis>"#
                ),
                gap = gap
            );
        }
        Some(SeriesLayout::Funnel) => {
            s.push_str(
                r#"<cx:axis id="0"><cx:catScaling gapWidth="0.06"/><cx:tickLabels/></cx:axis>"#,
            );
        }
        _ => {}
    }
    s.push_str("</cx:plotArea></cx:chart></cx:chartSpace>");
    s
}

/// The chart part with what the model changed spliced in.
fn rewrite_part(xml: &str, chart: &ChartEx, read: &ChartEx) -> String {
    let Some(root) = children(xml).into_iter().find(|n| n.name == "chartSpace") else {
        return xml.to_owned();
    };
    let p = prefix_of(&root);
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let base = root.inner_start;
    let kids = root.children();

    // The data sets are rewritten only when some series reads other data; the
    // series then point at them by position.
    let same_data = chart.series.len() == read.series.len()
        && chart
            .series
            .iter()
            .zip(&read.series)
            .all(|(a, b)| a.dimensions == b.dimensions);
    if !same_data && let Some(data) = kids.iter().find(|n| n.name == "chartData") {
        let inner = base + data.inner_start;
        let sets: Vec<Node<'_>> = data
            .children()
            .into_iter()
            .filter(|n| n.name == "data")
            .collect();
        let at = sets
            .first()
            .map_or(inner + data.inner.len(), |n| inner + n.span.start);
        let text: String = chart
            .series
            .iter()
            .enumerate()
            .map(|(i, s)| render_data(i, &s.dimensions, &p))
            .collect();
        // Before the cuts: a splice at the start of a cut stretch goes first.
        edits.push((at..at, text));
        for set in &sets {
            edits.push((inner + set.span.start..inner + set.span.end, String::new()));
        }
    }

    if let Some(body) = kids.iter().find(|n| n.name == "chart") {
        let inner = base + body.inner_start;
        let parts = body.children();
        if chart.title != read.title {
            let title = parts.iter().find(|n| n.name == "title");
            match (&chart.title, title) {
                (None, Some(node)) => edits.push((
                    inner + node.span.start..inner + node.span.end,
                    String::new(),
                )),
                (Some(text), Some(node)) => {
                    let at = inner + node.inner_start;
                    edits.extend(replace_tx(node, at, text, &p));
                }
                (Some(text), None) => edits.push((inner..inner, render_title(text, &p))),
                (None, None) => {}
            }
        }
        if let Some(area) = parts.iter().find(|n| n.name == "plotArea") {
            let area_inner = inner + area.inner_start;
            if let Some(region) = area
                .children()
                .into_iter()
                .find(|n| n.name == "plotAreaRegion")
            {
                let region_inner = area_inner + region.inner_start;
                let series: Vec<Node<'_>> = region
                    .children()
                    .into_iter()
                    .filter(|n| n.name == "series")
                    .collect();
                for (i, node) in series.iter().enumerate() {
                    let span = region_inner + node.span.start..region_inner + node.span.end;
                    match (chart.series.get(i), read.series.get(i)) {
                        (Some(now), Some(was)) if now != was || !same_data => {
                            edits.push((span, edit_series(node, i, now, was, !same_data, &p)));
                        }
                        (None, _) => edits.push((span, String::new())),
                        _ => {}
                    }
                }
                let at = series
                    .last()
                    .map_or(region_inner, |n| region_inner + n.span.end);
                let extra: String = chart
                    .series
                    .iter()
                    .enumerate()
                    .skip(series.len())
                    .map(|(i, s)| render_series(i, s, &p))
                    .collect();
                if !extra.is_empty() {
                    edits.push((at..at, extra));
                }
            }
        }
    }
    splice(xml, edits)
}

/// The prefix elements take in a part, `cx:` or empty.
fn prefix_of(node: &Node<'_>) -> String {
    if node.prefix.is_empty() {
        String::new()
    } else {
        format!("{}:", node.prefix)
    }
}

/// One series element with what the model changed in it.
fn edit_series(
    node: &Node<'_>,
    index: usize,
    now: &ExSeries,
    was: &ExSeries,
    repoint: bool,
    p: &str,
) -> String {
    let outer = node.outer;
    let head_len = node.tag.len();
    let mut tag = node.tag.trim_end_matches('>').to_owned();
    let empty = tag.ends_with('/');
    if empty {
        tag.pop();
    }
    if now.layout != was.layout
        && let Some(layout) = now.layout.as_str()
    {
        tag = set_attribute(&tag, "layoutId", layout);
    }
    if now.hidden != was.hidden {
        tag = set_attribute(&tag, "hidden", if now.hidden { "1" } else { "0" });
    }
    if empty {
        // A series with nothing in it: rendered whole.
        return render_series(index, now, p);
    }
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    // Spans below are within `outer`.
    let inner = node.inner_start - node.span.start;
    let kids = node.children();
    if now.name != was.name {
        let tx = kids.iter().find(|n| n.name == "tx");
        let span = tx.map_or(inner..inner, |tx| {
            inner + tx.span.start..inner + tx.span.end
        });
        let text = now.name.as_ref().map_or(String::new(), |t| render_tx(t, p));
        edits.push((span, text));
    }
    if repoint {
        let id = format!(r#"<{p}dataId val="{index}"/>"#);
        // `dataId` comes before the layout properties and the axis ids.
        let span = kids.iter().find(|n| n.name == "dataId").map_or_else(
            || {
                let at = kids
                    .iter()
                    .find(|n| matches!(n.name, "layoutPr" | "axisId" | "extLst"))
                    .map_or(inner + node.inner.len(), |n| inner + n.span.start);
                at..at
            },
            |d| inner + d.span.start..inner + d.span.end,
        );
        edits.push((span, id));
    }
    let body = splice(outer, edits);
    format!("{tag}>{}", &body[head_len..])
}

/// Edits replacing the `tx` child of a title, or adding one first.
fn replace_tx(
    node: &Node<'_>,
    inner: usize,
    text: &ChartText,
    p: &str,
) -> Vec<(Range<usize>, String)> {
    match node.children().into_iter().find(|n| n.name == "tx") {
        Some(tx) => vec![(
            inner + tx.span.start..inner + tx.span.end,
            render_tx(text, p),
        )],
        None => vec![(inner..inner, render_tx(text, p))],
    }
}

fn render_title(text: &ChartText, p: &str) -> String {
    format!(
        r#"<{p}title pos="t" align="ctr" overlay="0">{}</{p}title>"#,
        render_tx(text, p)
    )
}

/// `<cx:tx>`: a cell reference with its cached value, formatted text that
/// still says the same words, or plain text.
fn render_tx(text: &ChartText, p: &str) -> String {
    let body = match text {
        // `txData` is the form for text a cell holds: the formula, and what it
        // said. Typed text goes as `rich`, the way it does in a classic chart.
        ChartText::Reference { formula, cache } => {
            let value = cache
                .as_ref()
                .map_or(String::new(), |v| format!("<{p}v>{}</{p}v>", escape(v)));
            format!(
                "<{p}txData><{p}f>{}</{p}f>{value}</{p}txData>",
                escape(formula)
            )
        }
        ChartText::Text {
            text,
            rich: Some(rich),
        } if rich_text(rich) == *text => rich.clone(),
        ChartText::Text { text, .. } => render_rich(text, p),
    };
    format!("<{p}tx>{body}</{p}tx>")
}

/// Typed text as `DrawingML`, a paragraph per line.
fn render_rich(text: &str, p: &str) -> String {
    let mut s = format!("<{p}rich><a:bodyPr/><a:lstStyle/>");
    for line in text.split('\n') {
        let _ = write!(s, "<a:p><a:r><a:t>{}</a:t></a:r></a:p>", escape(line));
    }
    let _ = write!(s, "</{p}rich>");
    s
}

fn render_series(index: usize, series: &ExSeries, p: &str) -> String {
    let mut s = format!(
        r#"<{p}series layoutId="{}""#,
        series.layout.as_str().unwrap_or("clusteredColumn")
    );
    if series.hidden {
        s.push_str(r#" hidden="1""#);
    }
    s.push('>');
    if let Some(name) = &series.name {
        s.push_str(&render_tx(name, p));
    }
    let _ = write!(s, r#"<{p}dataId val="{index}"/>"#);
    // What Excel writes for a new chart of each kind; the rest need nothing.
    match series.layout {
        SeriesLayout::ClusteredColumn => {
            let _ = write!(
                s,
                r#"<{p}layoutPr><{p}binning intervalClosed="r"/></{p}layoutPr>"#
            );
        }
        SeriesLayout::BoxWhisker => {
            let _ = write!(
                s,
                r#"<{p}layoutPr><{p}visibility meanLine="0" meanMarker="1" nonoutliers="0" outliers="1"/><{p}statistics quartileMethod="exclusive"/></{p}layoutPr>"#
            );
        }
        SeriesLayout::Treemap => {
            let _ = write!(
                s,
                r#"<{p}layoutPr><{p}parentLabelLayout val="overlapping"/></{p}layoutPr>"#
            );
        }
        _ => {}
    }
    let _ = write!(s, "</{p}series>");
    s
}

fn render_data(id: usize, dimensions: &[Dimension], p: &str) -> String {
    let mut s = format!(r#"<{p}data id="{id}">"#);
    for dim in dimensions {
        let kind = if dim.numeric { "numDim" } else { "strDim" };
        let _ = write!(s, r#"<{p}{kind} type="{}">"#, dim.role.as_str());
        if let Some(formula) = &dim.formula {
            let _ = write!(s, "<{p}f>{}</{p}f>", escape(formula));
        }
        for level in &dim.levels {
            let count = level.iter().map(|(i, _)| i + 1).max().unwrap_or(0);
            let _ = write!(s, r#"<{p}lvl ptCount="{count}">"#);
            for (idx, value) in level {
                let _ = write!(s, r#"<{p}pt idx="{idx}">{}</{p}pt>"#, escape(value));
            }
            let _ = write!(s, "</{p}lvl>");
        }
        let _ = write!(s, "</{p}{kind}>");
    }
    let _ = write!(s, "</{p}data>");
    s
}
