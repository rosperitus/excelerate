//! Writing charts: an untouched chart as the bytes it came in, a changed or
//! new one from the model.
//!
//! The rest of the xlsx writer knows nothing of charts. It writes the carried
//! parts and the attachments a sheet has, so this module's whole job is to
//! hand it a workbook where those already say the right thing: a changed
//! chart's part replaced, a drawing's frames moved, added or dropped, a new
//! drawing attached to a sheet that had none. When no chart changed, that
//! workbook is the one passed in.
//!
//! A drawing is edited by splicing, never rebuilt: pictures, shapes, slicers
//! and 2016 charts share it with the classic charts and are not modelled, so
//! everything but the frames being changed stays byte for byte.

use super::xlsx::relative_target;
use super::xmlesc::escape;
use crate::error::{Error, Result};
use crate::model::chart::{Anchor, ChartText, DataSource, Marker, Plot, PlotKind, Title};
use crate::model::chart::{BarDirection, Chart, ChartAxis};
use crate::model::chart::{
    ChartColor, ColorBase, DataLabels, DataPoint, Fill, LineFormat, SeriesMarker, ShapeFormat,
};
use crate::model::{Attachment, OpaquePart, Spreadsheet, Worksheet};
use crate::reader::chart::{FILLS, Node, children, read_chart, rich_text, scan_drawing};
use crate::reader::chart::{
    read_fill, read_labels, read_line, read_marker, read_point, read_shape_format, tag_attr,
};
use core::ops::Range;
use std::borrow::Cow;
use std::fmt::Write as _;

const CHART_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
pub(super) const DRAWING_NS: &str =
    "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
pub(super) const MAIN_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub(super) const REL_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CHART_TYPE: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
pub(super) const DRAWING_TYPE: &str = "application/vnd.openxmlformats-officedocument.drawing+xml";
pub(super) const XML_DECL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#;

/// The workbook as the writer should see it, charts applied.
///
/// # Errors
/// [`Error::Xlsx`] for a chart the file format cannot say: a plot drawn
/// against axes the chart does not have.
pub(super) fn prepare(book: &Spreadsheet) -> Result<Cow<'_, Spreadsheet>> {
    let dirty: Vec<usize> = book
        .sheets()
        .iter()
        .enumerate()
        .filter(|(_, sheet)| {
            sheet.charts.iter().any(|c| !c.is_unchanged())
                || drawings(sheet).any(|d| lost_frames(book, sheet, d))
        })
        .map(|(i, _)| i)
        .collect();
    if dirty.is_empty() {
        return Ok(Cow::Borrowed(book));
    }
    for sheet in dirty.iter().filter_map(|&i| book.sheet(i)) {
        for chart in &sheet.charts {
            check(chart)?;
        }
    }
    // A clone shares the cells of every sheet (`Worksheet::cells`), so what
    // it copies is the parts and the furniture, not the grid.
    let mut out = book.clone();
    for index in dirty {
        apply_sheet(&mut out, index);
    }
    Ok(Cow::Owned(out))
}

pub(super) fn drawings(sheet: &Worksheet) -> impl Iterator<Item = &str> {
    sheet
        .attachments
        .iter()
        .filter(|a| a.role() == "drawing")
        .map(|a| a.target.as_str())
}

/// Whether a drawing shows a chart the sheet no longer has.
///
/// Asked of the drawing's relationships, which are small, rather than of the
/// drawing, which is not: every frame of a chart has one.
fn lost_frames(book: &Spreadsheet, sheet: &Worksheet, drawing: &str) -> bool {
    relationships(book, drawing)
        .iter()
        .filter(|r| r.kind.ends_with("/chart"))
        .any(|r| !claimed_by(sheet, drawing, &r.target) && is_modelled(book, &r.target))
}

fn claimed_by(sheet: &Worksheet, drawing: &str, part: &str) -> bool {
    sheet.charts.iter().any(|c| {
        c.origin
            .as_ref()
            .is_some_and(|o| o.drawing == drawing && o.part == part)
    })
}

/// Whether a part is a chart the reader turns into the model. A chart part it
/// cannot read never reached the sheet, so its absence there is no removal,
/// and dropping its frame would lose it.
fn is_modelled(book: &Spreadsheet, part: &str) -> bool {
    part_text(book, part).is_some_and(|xml| read_chart(xml).is_some())
}

fn check(chart: &Chart) -> Result<()> {
    for plot in &chart.plots {
        let missing = plot
            .axis_ids
            .iter()
            .any(|id| !chart.axes.iter().any(|a| a.id == *id));
        if plot.kind.has_axes() && (plot.axis_ids.len() < 2 || missing) {
            return Err(Error::Xlsx(format!(
                "chart {:?}: a {:?} plot needs two axes the chart has",
                chart.name, plot.kind
            )));
        }
    }
    Ok(())
}

pub(super) fn part_text<'a>(book: &'a Spreadsheet, path: &str) -> Option<&'a str> {
    let part = book.parts.iter().find(|p| p.path == path)?;
    core::str::from_utf8(&part.data).ok()
}

/// One entry of a relationship part.
pub(super) struct Relationship {
    pub id: String,
    pub kind: String,
    /// The package path it resolves to.
    pub target: String,
    /// Where the element sits in the relationship part.
    pub span: Range<usize>,
}

pub(super) fn relationships(book: &Spreadsheet, part: &str) -> Vec<Relationship> {
    part_text(book, &rels_path(part)).map_or_else(Vec::new, |xml| parse_relationships(xml, part))
}

pub(super) fn parse_relationships(xml: &str, part: &str) -> Vec<Relationship> {
    let base = part.rsplit_once('/').map_or("", |(dir, _)| dir);
    let Some(root) = children(xml)
        .into_iter()
        .find(|n| n.name == "Relationships")
    else {
        return Vec::new();
    };
    root.children()
        .iter()
        .filter(|n| n.name == "Relationship")
        .filter_map(|n| {
            Some(Relationship {
                id: n.attr("Id")?.to_owned(),
                kind: n.attr("Type")?.to_owned(),
                target: resolve(base, &n.attr_text("Target")?),
                span: root.inner_start + n.span.start..root.inner_start + n.span.end,
            })
        })
        .collect()
}

pub(super) fn rels_path(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

fn resolve(base: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_owned();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    let mut rest = target;
    while let Some(up) = rest.strip_prefix("../") {
        parts.pop();
        rest = up;
    }
    parts.push(rest);
    parts.join("/")
}

/// A part path of the form `{stem}{n}.xml` that no part uses yet.
fn free_path(book: &Spreadsheet, stem: &str) -> String {
    (1..=u32::MAX)
        .map(|n| format!("{stem}{n}.xml"))
        .find(|p| !book.parts.iter().any(|part| &part.path == p))
        .unwrap_or_default()
}

pub(super) fn set_part(
    book: &mut Spreadsheet,
    path: &str,
    content_type: Option<&str>,
    data: String,
) {
    if let Some(part) = book.parts.iter_mut().find(|p| p.path == path) {
        part.data = data.into_bytes();
    } else {
        book.parts.push(OpaquePart {
            path: path.to_owned(),
            content_type: content_type.map(str::to_owned),
            data: data.into_bytes(),
        });
    }
}

/// The highest drawing object id in use, so a new frame's is unique. Every
/// object carries one on its `cNvPr`, whatever kind of object it is.
pub(super) fn max_object_id(xml: &str) -> u32 {
    xml.match_indices("cNvPr ")
        .filter_map(|(i, _)| {
            let tag = &xml[i..];
            let tag = &tag[..tag.find('>')?];
            let value = tag.split(" id=\"").nth(1)?;
            value.split('"').next()?.parse().ok()
        })
        .max()
        .unwrap_or(0)
}

/// Where a frame is placed in its drawing.
struct Placed {
    /// The relationship id the frame uses.
    rel: String,
    /// The drawing object id.
    id: u32,
}

fn apply_sheet(book: &mut Spreadsheet, index: usize) {
    let Some(sheet) = book.sheet(index) else {
        return;
    };
    let charts = sheet.charts.clone();
    let mut paths: Vec<String> = drawings(sheet).map(str::to_owned).collect();
    // A sheet with a new chart and no drawing gets one.
    if paths.is_empty() {
        let path = free_path(book, "xl/drawings/drawing");
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
    // Every drawing drops the frames of charts that are gone and moves the
    // ones that moved. The first also takes the new charts, and goes last, so
    // the charts of the others are claimed before anything counts as new.
    let first = paths.first().cloned().unwrap_or_default();
    paths.rotate_left(1);
    let mut claimed = vec![false; charts.len()];
    for path in &paths {
        rewrite_drawing(book, path, &charts, &mut claimed, *path == first);
    }
}

/// Rewrites one drawing part and its relationships.
fn rewrite_drawing(
    book: &mut Spreadsheet,
    path: &str,
    charts: &[Chart],
    claimed: &mut [bool],
    takes_new: bool,
) {
    let Some(xml) = part_text(book, path).map(str::to_owned) else {
        return;
    };
    let rels = relationships(book, path);
    // What replaces which stretch of the drawing; an empty text drops it.
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut removed: Vec<String> = Vec::new();

    for object in scan_drawing(&xml) {
        let alone = matches!(object.frames.as_slice(), [f] if !f.grouped);
        for frame in object.frames.iter().filter(|f| !f.extended) {
            let Some(target) = rels.iter().find(|r| r.id == frame.rel).map(|r| &r.target) else {
                continue;
            };
            let found = charts.iter().zip(claimed.iter()).position(|(c, taken)| {
                !taken
                    && c.origin
                        .as_ref()
                        .is_some_and(|o| o.drawing == path && o.part == *target)
            });
            let Some(i) = found else {
                if is_modelled(book, target) {
                    // The frame goes, and so does the relationship pointing at
                    // it. Inside a group only the frame goes.
                    let span = if alone { &object.span } else { &frame.span };
                    edits.push((span.clone(), String::new()));
                    removed.push(frame.rel.clone());
                }
                continue;
            };
            claimed[i] = true;
            let chart = &charts[i];
            let Some(origin) = &chart.origin else {
                continue;
            };
            if !chart.same_content(&origin.read) {
                set_part(book, target, Some(CHART_TYPE), render_chart(chart));
            }
            // A frame inside a group is placed by the group.
            if alone && !chart.same_placement(&origin.read) {
                let placed = Placed {
                    rel: frame.rel.clone(),
                    id: frame.id,
                };
                edits.push((object.span.clone(), render_frame(chart, &placed)));
            }
        }
    }

    let mut added: Vec<(String, String)> = Vec::new();
    if takes_new {
        let mut frames = String::new();
        let mut next_id = max_object_id(&xml);
        for (chart, taken) in charts.iter().zip(claimed.iter_mut()) {
            if *taken {
                continue;
            }
            *taken = true;
            next_id = next_id.saturating_add(1);
            let part = free_path(book, "xl/charts/chart");
            set_part(book, &part, Some(CHART_TYPE), render_chart(chart));
            let rel = (1..=u32::MAX)
                .map(|n| format!("rId{n}"))
                .find(|id| !rels.iter().any(|r| r.id == *id) && !added.iter().any(|a| a.0 == *id))
                .unwrap_or_default();
            let placed = Placed {
                rel: rel.clone(),
                id: next_id,
            };
            frames.push_str(&render_frame(chart, &placed));
            added.push((rel, part));
        }
        if !frames.is_empty() {
            // New frames go last, just before the root closes.
            let close = xml.rfind("</").unwrap_or(xml.len());
            edits.push((close..close, frames));
        }
    }
    if edits.is_empty() {
        return;
    }

    edits.sort_by_key(|(span, _)| span.start);
    let mut out = String::with_capacity(xml.len());
    let mut at = 0;
    for (span, text) in edits {
        // Nothing nests inside a stretch already replaced; this only guards
        // against a file whose spans do.
        if span.start < at {
            continue;
        }
        out.push_str(&xml[at..span.start]);
        out.push_str(&text);
        at = span.end;
    }
    out.push_str(&xml[at..]);
    set_part(book, path, Some(DRAWING_TYPE), out);
    edit_relationships(book, path, &removed, &added, "chart");
}

/// Drops and adds relationships of a drawing, creating its relationship part
/// when it had none. The added ones are all of one kind: the last segment of
/// an Office relationship type (`chart`, `image`), or a whole type.
pub(super) fn edit_relationships(
    book: &mut Spreadsheet,
    drawing: &str,
    removed: &[String],
    added: &[(String, String)],
    kind: &str,
) {
    if removed.is_empty() && added.is_empty() {
        return;
    }
    let rels_part = rels_path(drawing);
    let base = drawing.rsplit_once('/').map_or("", |(dir, _)| dir);
    let text = part_text(book, &rels_part).map_or_else(
        || {
            format!(
                r#"{XML_DECL}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"></Relationships>"#
            )
        },
        str::to_owned,
    );
    let kind = if kind.contains("://") {
        kind.to_owned()
    } else {
        format!("{REL_NS}/{kind}")
    };
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for rel in parse_relationships(&text, drawing) {
        if removed.contains(&rel.id) {
            out.push_str(&text[at..rel.span.start]);
            at = rel.span.end;
        }
    }
    let close = text.rfind("</").filter(|&c| c >= at).unwrap_or(text.len());
    out.push_str(&text[at..close]);
    for (id, target) in added {
        let _ = write!(
            out,
            r#"<Relationship Id="{id}" Type="{kind}" Target="{}"/>"#,
            escape(&relative_target(base, target))
        );
    }
    out.push_str(&text[close..]);
    set_part(book, &rels_part, None, out);
}

/// A frame for a chart, declaring its own namespaces so it can go into a
/// drawing whose root binds them to other prefixes, or not at all.
fn render_frame(chart: &Chart, placed: &Placed) -> String {
    let frame = format!(
        concat!(
            r#"<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{id}" name="{name}"/>"#,
            r#"<xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr>"#,
            r#"<xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm>"#,
            r#"<a:graphic><a:graphicData uri="{chart_ns}">"#,
            r#"<c:chart xmlns:c="{chart_ns}" r:id="{rel}"/></a:graphicData></a:graphic>"#,
            r#"</xdr:graphicFrame><xdr:clientData/>"#
        ),
        id = placed.id,
        name = escape(&chart.name),
        chart_ns = CHART_NS,
        rel = escape(&placed.rel),
    );
    render_anchor(chart.anchor, &frame)
}

/// An anchor element around the object it places, declaring the namespaces
/// its own markup uses so it can go into any drawing.
pub(super) fn render_anchor(anchor: Anchor, object: &str) -> String {
    let ns = format!(r#" xmlns:xdr="{DRAWING_NS}" xmlns:a="{MAIN_NS}" xmlns:r="{REL_NS}""#);
    match anchor {
        Anchor::TwoCell { from, to, edit_as } => {
            let edit = edit_as.map_or(String::new(), |e| format!(r#" editAs="{}""#, e.as_str()));
            format!(
                "<xdr:twoCellAnchor{ns}{edit}>{}{}{object}</xdr:twoCellAnchor>",
                marker("from", from),
                marker("to", to)
            )
        }
        Anchor::OneCell {
            from,
            width,
            height,
        } => format!(
            r#"<xdr:oneCellAnchor{ns}>{}<xdr:ext cx="{width}" cy="{height}"/>{object}</xdr:oneCellAnchor>"#,
            marker("from", from)
        ),
        Anchor::Absolute {
            x,
            y,
            width,
            height,
        } => format!(
            r#"<xdr:absoluteAnchor{ns}><xdr:pos x="{x}" y="{y}"/><xdr:ext cx="{width}" cy="{height}"/>{object}</xdr:absoluteAnchor>"#
        ),
    }
}

fn marker(tag: &str, m: Marker) -> String {
    format!(
        concat!(
            "<xdr:{tag}><xdr:col>{}</xdr:col><xdr:colOff>{}</xdr:colOff>",
            "<xdr:row>{}</xdr:row><xdr:rowOff>{}</xdr:rowOff></xdr:{tag}>"
        ),
        m.col.index(),
        m.col_offset,
        m.row.index(),
        m.row_offset,
        tag = tag
    )
}

/// Renders a chart part from the model.
///
/// A chart that was read keeps its root's namespace declarations and the
/// prefix it gave the chart namespace, because the markup it carries was
/// written against them. A chart made in code gets what Excel writes for a
/// new one wherever the model leaves a slot empty.
pub(crate) fn render_chart(chart: &Chart) -> String {
    let fresh = chart.origin.is_none();
    let (prefix, root) = chart.origin.as_ref().map_or_else(
        || {
            (
                "c:".to_owned(),
                format!(r#"xmlns:c="{CHART_NS}" xmlns:a="{MAIN_NS}" xmlns:r="{REL_NS}""#),
            )
        },
        |o| {
            let p = if o.prefix.is_empty() {
                String::new()
            } else {
                format!("{}:", o.prefix)
            };
            (p, o.root_attributes.clone())
        },
    );
    let mut w = Out {
        s: String::new(),
        p: prefix,
    };
    let _ = write!(w.s, "{XML_DECL}<{}chartSpace {root}>", w.p);
    let m = &chart.markup;
    if fresh && m.before_chart.is_empty() {
        w.empty("roundedCorners", Some("0"));
    }
    w.s.push_str(&m.before_chart);
    w.open("chart");
    if let Some(title) = &chart.title {
        w.title(title, fresh);
    }
    w.empty(
        "autoTitleDeleted",
        Some(if chart.auto_title_deleted { "1" } else { "0" }),
    );
    w.s.push_str(&m.before_plot_area);
    w.open("plotArea");
    if m.plot_area_layout.is_empty() {
        w.empty("layout", None);
    } else {
        w.s.push_str(&m.plot_area_layout);
    }
    for plot in &chart.plots {
        w.plot(plot);
    }
    for axis in &chart.axes {
        w.axis(axis, fresh);
    }
    w.s.push_str(&m.after_axes);
    w.close("plotArea");
    if let Some(legend) = &chart.legend {
        w.open("legend");
        w.empty("legendPos", Some(legend.position.as_str()));
        if fresh && legend.markup.is_empty() {
            w.empty("overlay", Some("0"));
        }
        w.s.push_str(&legend.markup);
        w.close("legend");
    }
    if fresh && m.after_legend.is_empty() {
        w.empty("plotVisOnly", Some("1"));
    }
    w.s.push_str(&m.after_legend);
    w.close("chart");
    w.s.push_str(&m.after_chart);
    w.close("chartSpace");
    w.s
}

/// The text being built, and the prefix chart elements take in it.
struct Out {
    s: String,
    p: String,
}

impl Out {
    fn open(&mut self, name: &str) {
        let _ = write!(self.s, "<{}{name}>", self.p);
    }

    fn close(&mut self, name: &str) {
        let _ = write!(self.s, "</{}{name}>", self.p);
    }

    fn empty(&mut self, name: &str, val: Option<&str>) {
        match val {
            Some(v) => {
                let _ = write!(self.s, r#"<{}{name} val="{}"/>"#, self.p, escape(v));
            }
            None => {
                let _ = write!(self.s, "<{}{name}/>", self.p);
            }
        }
    }

    fn text_element(&mut self, name: &str, text: &str) {
        let _ = write!(
            self.s,
            "<{p}{name}>{}</{p}{name}>",
            escape(text),
            p = self.p
        );
    }

    fn title(&mut self, title: &Title, fresh: bool) {
        self.open("title");
        if let Some(text) = &title.text {
            self.open("tx");
            match text {
                ChartText::Reference { .. } => self.reference(text),
                ChartText::Text { text, rich } => match rich {
                    Some(r) if rich_text(r) == *text => self.s.push_str(r),
                    _ => self.rich(text),
                },
            }
            self.close("tx");
        }
        if fresh && title.markup.is_empty() {
            self.empty("overlay", Some("0"));
        }
        self.s.push_str(&title.markup);
        self.close("title");
    }

    /// `<c:strRef>` for text read from a cell.
    fn reference(&mut self, text: &ChartText) {
        let ChartText::Reference { formula, cache } = text else {
            return;
        };
        self.open("strRef");
        self.text_element("f", formula);
        if let Some(cache) = cache {
            self.open("strCache");
            self.empty("ptCount", Some("1"));
            let _ = write!(self.s, r#"<{}pt idx="0">"#, self.p);
            self.text_element("v", cache);
            self.close("pt");
            self.close("strCache");
        }
        self.close("strRef");
    }

    /// Plain text as a formatted text element, a paragraph per line.
    fn rich(&mut self, text: &str) {
        let _ = write!(
            self.s,
            r#"<{}rich><a:bodyPr xmlns:a="{MAIN_NS}"/><a:lstStyle xmlns:a="{MAIN_NS}"/>"#,
            self.p
        );
        for line in text.split('\n') {
            let _ = write!(
                self.s,
                r#"<a:p xmlns:a="{MAIN_NS}"><a:r><a:t>{}</a:t></a:r></a:p>"#,
                escape(line)
            );
        }
        self.close("rich");
    }

    fn plot(&mut self, plot: &Plot) {
        let element = plot.kind.element();
        self.open(element);
        match plot.kind {
            PlotKind::Bar {
                direction,
                grouping,
                ..
            } => {
                let dir = match direction {
                    BarDirection::Column => "col",
                    BarDirection::Bar => "bar",
                };
                self.empty("barDir", Some(dir));
                self.empty("grouping", Some(grouping.as_str()));
            }
            PlotKind::Line { grouping, .. } | PlotKind::Area { grouping, .. } => {
                self.empty("grouping", Some(grouping.as_str()));
            }
            PlotKind::OfPie { bar } => {
                self.empty("ofPieType", Some(if bar { "bar" } else { "pie" }));
            }
            PlotKind::Scatter(style) => self.empty("scatterStyle", Some(style.as_str())),
            PlotKind::Radar(style) => self.empty("radarStyle", Some(style.as_str())),
            PlotKind::Surface { wireframe, .. } => {
                self.empty("wireframe", Some(if wireframe { "1" } else { "0" }));
            }
            _ => {}
        }
        // A surface has no such element; its colours come from bands.
        if let Some(vary) = plot.vary_colors
            && !matches!(plot.kind, PlotKind::Surface { .. } | PlotKind::Stock)
        {
            self.empty("varyColors", Some(if vary { "1" } else { "0" }));
        }
        let xy = plot.kind.plots_xy();
        for series in &plot.series {
            self.open("ser");
            self.empty("idx", Some(&series.index.to_string()));
            self.empty("order", Some(&series.order.to_string()));
            match &series.name {
                Some(text @ ChartText::Reference { .. }) => {
                    self.open("tx");
                    self.reference(text);
                    self.close("tx");
                }
                Some(ChartText::Text { text, .. }) => {
                    self.open("tx");
                    self.text_element("v", text);
                    self.close("tx");
                }
                None => {}
            }
            if let Some(format) = &series.format {
                self.s.push_str(&shape_format(&self.p, format));
            }
            self.s.push_str(&series.markup.after_format);
            if let Some(marker) = &series.marker {
                self.s.push_str(&series_marker(&self.p, marker));
            }
            for point in &series.data_points {
                self.s.push_str(&data_point(&self.p, point));
            }
            if let Some(labels) = &series.labels {
                self.s.push_str(&data_labels(&self.p, labels));
            }
            self.s.push_str(&series.markup.before_data);
            let (cat, val) = if xy { ("xVal", "yVal") } else { ("cat", "val") };
            if let Some(data) = &series.categories {
                self.data(cat, data);
            }
            if let Some(data) = &series.values {
                self.data(val, data);
            }
            if let Some(data) = &series.bubble_sizes {
                self.data("bubbleSize", data);
            }
            self.s.push_str(&series.markup.after_data);
            self.close("ser");
        }
        if let Some(labels) = &plot.labels {
            self.s.push_str(&data_labels(&self.p, labels));
        }
        self.s.push_str(&plot.markup);
        for id in &plot.axis_ids {
            self.empty("axId", Some(&id.to_string()));
        }
        self.close(element);
    }

    fn data(&mut self, name: &str, data: &DataSource) {
        self.open(name);
        match data {
            DataSource::Numbers {
                formula,
                format_code,
                count,
                points,
            } => {
                let (outer, cache) = if formula.is_some() {
                    ("numRef", Some("numCache"))
                } else {
                    ("numLit", None)
                };
                self.open(outer);
                if let Some(f) = formula {
                    self.text_element("f", f);
                }
                let has_cache = format_code.is_some() || count.is_some() || !points.is_empty();
                if has_cache || cache.is_none() {
                    if let Some(c) = cache {
                        self.open(c);
                    }
                    if let Some(code) = format_code {
                        self.text_element("formatCode", code);
                    }
                    let n =
                        count.unwrap_or_else(|| points.iter().map(|p| p.0 + 1).max().unwrap_or(0));
                    self.empty("ptCount", Some(&n.to_string()));
                    for (idx, value) in points {
                        let _ = write!(self.s, r#"<{}pt idx="{idx}">"#, self.p);
                        self.text_element("v", &value.to_string());
                        self.close("pt");
                    }
                    if let Some(c) = cache {
                        self.close(c);
                    }
                }
                self.close(outer);
            }
            DataSource::Strings {
                formula,
                count,
                points,
            } => {
                let (outer, cache) = if formula.is_some() {
                    ("strRef", Some("strCache"))
                } else {
                    ("strLit", None)
                };
                self.open(outer);
                if let Some(f) = formula {
                    self.text_element("f", f);
                }
                if count.is_some() || !points.is_empty() || cache.is_none() {
                    if let Some(c) = cache {
                        self.open(c);
                    }
                    let n =
                        count.unwrap_or_else(|| points.iter().map(|p| p.0 + 1).max().unwrap_or(0));
                    self.empty("ptCount", Some(&n.to_string()));
                    for (idx, value) in points {
                        let _ = write!(self.s, r#"<{}pt idx="{idx}">"#, self.p);
                        self.text_element("v", value);
                        self.close("pt");
                    }
                    if let Some(c) = cache {
                        self.close(c);
                    }
                }
                self.close(outer);
            }
            DataSource::Levels { markup, .. } => self.s.push_str(markup),
        }
        self.close(name);
    }

    fn axis(&mut self, axis: &ChartAxis, fresh: bool) {
        let element = axis.kind.element();
        self.open(element);
        self.empty("axId", Some(&axis.id.to_string()));
        self.open("scaling");
        if let Some(base) = axis.log_base {
            self.empty("logBase", Some(&base.to_string()));
        }
        self.empty(
            "orientation",
            Some(if axis.reversed { "maxMin" } else { "minMax" }),
        );
        if let Some(max) = axis.max {
            self.empty("max", Some(&max.to_string()));
        }
        if let Some(min) = axis.min {
            self.empty("min", Some(&min.to_string()));
        }
        self.close("scaling");
        self.empty("delete", Some(if axis.deleted { "1" } else { "0" }));
        self.empty("axPos", Some(axis.position.as_str()));
        self.s.push_str(&axis.markup.gridlines);
        if let Some(title) = &axis.title {
            self.title(title, fresh);
        }
        if let Some((code, linked)) = &axis.number_format {
            let _ = write!(
                self.s,
                r#"<{}numFmt formatCode="{}" sourceLinked="{}"/>"#,
                self.p,
                escape(code),
                u8::from(*linked)
            );
        }
        if fresh && axis.markup.ticks.is_empty() {
            self.empty("majorTickMark", Some("out"));
            self.empty("minorTickMark", Some("none"));
            self.empty("tickLblPos", Some("nextTo"));
        }
        self.s.push_str(&axis.markup.ticks);
        self.empty("crossAx", Some(&axis.cross_axis.to_string()));
        if fresh && axis.markup.tail.is_empty() {
            self.empty("crosses", Some("autoZero"));
            match axis.kind {
                crate::model::chart::AxisKind::Value => {
                    self.empty("crossBetween", Some("between"));
                }
                crate::model::chart::AxisKind::Category => {
                    self.empty("auto", Some("1"));
                    self.empty("lblAlgn", Some("ctr"));
                    self.empty("lblOffset", Some("100"));
                }
                _ => {}
            }
        }
        self.s.push_str(&axis.markup.tail);
        self.close(element);
    }
}

// Formatting the model names inside elements it does not fully model: each is
// written back as read while the model still says what the element says, and
// otherwise rebuilt from what was read with the model's children put in.

/// Children of `c:spPr`, in schema order; names that share a slot are
/// alternatives.
const SHAPE: &[&[&str]] = &[
    &["xfrm"],
    &["custGeom", "prstGeom"],
    &FILLS,
    &["ln"],
    &["effectLst", "effectDag"],
    &["scene3d"],
    &["sp3d"],
    &["extLst"],
];
const LINE: &[&[&str]] = &[
    &FILLS,
    &["prstDash", "custDash"],
    &["round", "bevel", "miter"],
    &["headEnd"],
    &["tailEnd"],
    &["extLst"],
];
const MARKER: &[&[&str]] = &[&["symbol"], &["size"], &["spPr"], &["extLst"]];
const POINT: &[&[&str]] = &[
    &["idx"],
    &["invertIfNegative"],
    &["marker"],
    &["bubble3D"],
    &["explosion"],
    &["spPr"],
    &["pictureOptions"],
    &["extLst"],
];
const LABELS: &[&[&str]] = &[
    &["dLbl"],
    &["delete"],
    &["numFmt"],
    &["spPr"],
    &["txPr"],
    &["dLblPos"],
    &["showLegendKey"],
    &["showVal"],
    &["showCatName"],
    &["showSerName"],
    &["showPercent"],
    &["showBubbleSize"],
    &["separator"],
    &["showLeaderLines"],
    &["leaderLines"],
    &["extLst"],
];

/// The element a stretch of carried markup is.
fn element(xml: &str) -> Option<Node<'_>> {
    children(xml).into_iter().next()
}

/// An element that was read, rewritten: the children in the `owned` slots of
/// `order` are replaced by the text given (dropped when it is empty), those in
/// slots `drop` accepts go, and everything else stays as it was. `tag`
/// replaces the start tag.
fn rebuild(
    node: &Node<'_>,
    order: &[&[&str]],
    owned: &[(usize, String)],
    drop: impl Fn(usize) -> bool,
    tag: Option<String>,
) -> String {
    let mut items: Vec<(usize, &str)> = Vec::new();
    let mut last = 0;
    for kid in node.children() {
        // A child the schema does not list stays beside the one before it.
        let slot = order
            .iter()
            .position(|names| names.contains(&kid.name))
            .unwrap_or(last);
        last = slot;
        if !drop(slot) && !owned.iter().any(|(s, _)| *s == slot) {
            items.push((slot, kid.outer));
        }
    }
    for (slot, text) in owned.iter().filter(|(_, t)| !t.is_empty()) {
        let at = items
            .iter()
            .position(|(s, _)| s > slot)
            .unwrap_or(items.len());
        items.insert(at, (*slot, text));
    }
    let tag = tag.unwrap_or_else(|| node.tag.to_owned());
    let open = match tag.strip_suffix("/>") {
        Some(start) => format!("{}>", start.trim_end()),
        None => tag,
    };
    let name = if node.prefix.is_empty() {
        node.name.to_owned()
    } else {
        format!("{}:{}", node.prefix, node.name)
    };
    let mut out = open;
    for (_, text) in items {
        out.push_str(text);
    }
    let _ = write!(out, "</{name}>");
    out
}

/// A start tag with one attribute set to `value`, or removed.
fn set_attr(tag: &str, name: &str, value: Option<String>) -> String {
    let body = tag.trim_end_matches('>').trim_end_matches('/').trim_end();
    let body = match tag_attr(body, name) {
        Some(old) => {
            let at = old.as_ptr().addr() - body.as_ptr().addr();
            let start = body[..at].rfind(name).unwrap_or(at);
            let end = (at + old.len() + 1).min(body.len());
            format!("{}{}", body[..start].trim_end(), &body[end..])
        }
        None => body.to_owned(),
    };
    match value {
        Some(v) => format!(r#"{body} {name}="{}">"#, escape(&v)),
        None => format!("{body}>"),
    }
}

fn color_xml(color: &ChartColor) -> String {
    let (name, val) = match &color.base {
        ColorBase::Rgb(rgb) => ("srgbClr", format!("{:06X}", rgb & 0x00FF_FFFF)),
        ColorBase::Scheme(scheme) => ("schemeClr", escape(scheme)),
    };
    if color.transforms.is_empty() {
        return format!(r#"<a:{name} val="{val}"/>"#);
    }
    let mut out = format!(r#"<a:{name} val="{val}">"#);
    for t in &color.transforms {
        let (t, v) = t.parts();
        let _ = write!(out, r#"<a:{t} val="{v}"/>"#);
    }
    let _ = write!(out, "</a:{name}>");
    out
}

/// A fill made from the model, declaring the namespace it is written in.
fn fill_xml(fill: &Fill) -> String {
    match fill {
        Fill::None => format!(r#"<a:noFill xmlns:a="{MAIN_NS}"/>"#),
        Fill::Solid(color) => format!(
            r#"<a:solidFill xmlns:a="{MAIN_NS}">{}</a:solidFill>"#,
            color_xml(color)
        ),
        // Nothing to say it with: a gradient is only ever kept as read.
        Fill::Other => String::new(),
    }
}

/// The fill among `kids` if the model still says it, else one from the model.
fn fill_or_kept(model: Option<&Fill>, kids: &[Node<'_>]) -> String {
    let read = kids.iter().find(|n| FILLS.contains(&n.name));
    if read.map(read_fill).as_ref() == model {
        read.map_or_else(String::new, |n| n.outer.to_owned())
    } else {
        model.map_or_else(String::new, fill_xml)
    }
}

fn line_xml(line: &LineFormat, read: Option<&Node<'_>>) -> String {
    let width = line.width.map(|w| w.to_string());
    match read {
        Some(node) if read_line(node) == *line => node.outer.to_owned(),
        Some(node) => rebuild(
            node,
            LINE,
            &[(0, fill_or_kept(line.fill.as_ref(), &node.children()))],
            |_| false,
            Some(set_attr(node.tag, "w", width)),
        ),
        None => format!(
            r#"<a:ln xmlns:a="{MAIN_NS}"{}>{}</a:ln>"#,
            width.map_or_else(String::new, |w| format!(r#" w="{w}""#)),
            line.fill.as_ref().map_or_else(String::new, fill_xml)
        ),
    }
}

/// `c:spPr` for a series or a point; `p` is the chart namespace prefix.
fn shape_format(p: &str, format: &ShapeFormat) -> String {
    let read = format.source.as_deref().and_then(element);
    let Some(node) = read else {
        return format!(
            "<{p}spPr>{}{}</{p}spPr>",
            format.fill.as_ref().map_or_else(String::new, fill_xml),
            format
                .line
                .as_ref()
                .map_or_else(String::new, |l| line_xml(l, None))
        );
    };
    if read_shape_format(&node) == *format {
        return node.outer.to_owned();
    }
    let kids = node.children();
    let line = format.line.as_ref().map_or_else(String::new, |l| {
        line_xml(l, kids.iter().find(|n| n.name == "ln"))
    });
    rebuild(
        &node,
        SHAPE,
        &[(2, fill_or_kept(format.fill.as_ref(), &kids)), (3, line)],
        |_| false,
        None,
    )
}

fn series_marker(p: &str, marker: &SeriesMarker) -> String {
    let symbol = marker.symbol.map_or_else(String::new, |s| {
        format!(r#"<{p}symbol val="{}"/>"#, s.as_str())
    });
    let size = marker
        .size
        .map_or_else(String::new, |s| format!(r#"<{p}size val="{s}"/>"#));
    match marker.source.as_deref().and_then(element) {
        Some(node) if read_marker(&node) == *marker => node.outer.to_owned(),
        Some(node) => rebuild(&node, MARKER, &[(0, symbol), (1, size)], |_| false, None),
        None => format!("<{p}marker>{symbol}{size}</{p}marker>"),
    }
}

fn data_point(p: &str, point: &DataPoint) -> String {
    let idx = format!(r#"<{p}idx val="{}"/>"#, point.index);
    let format = point
        .format
        .as_ref()
        .map_or_else(String::new, |f| shape_format(p, f));
    match point.source.as_deref().and_then(element) {
        Some(node) if read_point(&node) == *point => node.outer.to_owned(),
        Some(node) => rebuild(&node, POINT, &[(0, idx), (5, format)], |_| false, None),
        None => format!("<{p}dPt>{idx}{format}</{p}dPt>"),
    }
}

fn data_labels(p: &str, labels: &DataLabels) -> String {
    let read = labels.source.as_deref().and_then(element);
    if let Some(node) = &read
        && read_labels(node) == *labels
    {
        return node.outer.to_owned();
    }
    let flag = |name: &str, on: bool| format!(r#"<{p}{name} val="{}"/>"#, u8::from(on));
    // Hidden labels have nothing but the `delete`: the schema makes it a
    // choice between the two.
    let owned: Vec<(usize, String)> = if labels.deleted {
        vec![(1, flag("delete", true))]
    } else {
        vec![
            (1, String::new()),
            (
                5,
                labels.position.map_or_else(String::new, |pos| {
                    format!(r#"<{p}dLblPos val="{}"/>"#, pos.as_str())
                }),
            ),
            (6, flag("showLegendKey", labels.show_legend_key)),
            (7, flag("showVal", labels.show_value)),
            (8, flag("showCatName", labels.show_category_name)),
            (9, flag("showSerName", labels.show_series_name)),
            (10, flag("showPercent", labels.show_percent)),
        ]
    };
    let deleted = labels.deleted;
    if let Some(node) = read {
        return rebuild(
            &node,
            LABELS,
            &owned,
            |slot| deleted && (2..=14).contains(&slot),
            None,
        );
    }
    let mut out = format!("<{p}dLbls>");
    for (_, text) in owned {
        out.push_str(&text);
    }
    let _ = write!(out, "</{p}dLbls>");
    out
}
