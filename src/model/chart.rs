//! Charts: what a chart draws, what it reads, and where it sits.
//!
//!
//! A chart is two parts. The drawing of a sheet holds a frame - the anchor,
//! the name - pointing at `xl/charts/chartN.xml`, and that part holds the
//! chart itself: plots, their series, the axes, the title, the legend. Both
//! halves land in one [`Chart`] on [`crate::model::Worksheet::charts`].
//!
//! **What is modelled and what is carried.** A chart part is mostly about
//! looks: fills, line widths, fonts, label positions, effects, each in its own
//! corner of `DrawingML`. The model names what a program asks of a chart -
//! kind, series and the cells they read, axes and their scale, title, legend -
//! and keeps everything else as the markup it was written in, in slots that
//! say where it stood. A series edited through the model keeps its colour.
//!
//! **How it is written.** A chart the program did not touch goes back byte for
//! byte, whatever the file held beyond the model. One that was changed is
//! rendered from the model and its carried markup; one created in code gets
//! the defaults Excel writes for a new chart. Which is which is decided by
//! comparing the chart with what was read, so there is no dirty flag to
//! forget.
//!
//! **`chartEx`** - waterfall, funnel, treemap, sunburst, histogram, box and
//! whisker, region map - is a different schema from Office 2016 and lands in
//! [`ChartEx`] on [`crate::model::Worksheet::extended_charts`]. It is written
//! the same way, by comparison, but edited in place rather than rendered: its
//! part keeps the formatting the model does not name.

use crate::coordinate::{Col, Row};
use crate::model::DefinedName;

/// A chart on a sheet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chart {
    /// The name the frame carries, which is what the selection pane shows.
    pub name: String,
    /// Where the chart sits on the sheet.
    ///
    /// A chart inside a group of shapes reports the group's anchor, and moving
    /// it is not written: the group positions its members, and taking one out
    /// of it is an edit of the group.
    pub anchor: Anchor,
    /// The title above the plot, when the chart has one of its own.
    pub title: Option<Title>,
    /// Whether the automatic title was switched off. A chart of one series
    /// with no title of its own shows that series' name unless this is set.
    pub auto_title_deleted: bool,
    /// The plots, in drawing order. A combination chart - columns with a line
    /// over them - has more than one.
    pub plots: Vec<Plot>,
    /// The axes the plots refer to by id.
    pub axes: Vec<ChartAxis>,
    /// The legend, when there is one.
    pub legend: Option<Legend>,
    /// Everything the model does not name, where it stood.
    pub markup: ChartMarkup,
    /// The part this chart was read from; `None` for a chart made in code.
    pub origin: Option<ChartOrigin>,
}

impl Chart {
    /// Whether the chart still says what it said when it was read, and so
    /// goes back as the bytes that arrived.
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        self.origin
            .as_ref()
            .is_some_and(|o| self.same_placement(&o.read) && self.same_content(&o.read))
    }

    /// Takes the chart as it stands for what was read.
    ///
    /// For edits that change the carried bytes and the model the same way -
    /// a row inserted above the data moves both the series in the part and
    /// the series here - so the chart still counts as untouched.
    pub(crate) fn settle(&mut self) {
        if let Some(mut origin) = self.origin.take() {
            origin.read = Box::new(self.clone());
            self.origin = Some(origin);
        }
    }

    /// Whether the frame is where, and what, it was.
    pub(crate) fn same_placement(&self, other: &Self) -> bool {
        self.name == other.name && self.anchor == other.anchor
    }

    /// Whether the chart part would say the same thing.
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        // Destructured so a field added later cannot be forgotten here.
        let Self {
            name: _,
            anchor: _,
            title,
            auto_title_deleted,
            plots,
            axes,
            legend,
            markup,
            origin: _,
        } = self;
        *title == other.title
            && *auto_title_deleted == other.auto_title_deleted
            && *plots == other.plots
            && *axes == other.axes
            && *legend == other.legend
            && *markup == other.markup
    }

    /// Every formula the chart reads through, for an edit that rewrites them.
    pub(crate) fn formulas_mut(&mut self) -> Vec<&mut String> {
        let mut out = Vec::new();
        let titles = self
            .title
            .iter_mut()
            .chain(self.axes.iter_mut().filter_map(|a| a.title.as_mut()));
        out.extend(
            titles
                .filter_map(|t| t.text.as_mut())
                .filter_map(ChartText::formula_mut),
        );
        for series in self.plots.iter_mut().flat_map(|p| &mut p.series) {
            out.extend(series.name.as_mut().and_then(ChartText::formula_mut));
            for data in [
                &mut series.categories,
                &mut series.values,
                &mut series.bubble_sizes,
            ] {
                out.extend(data.as_mut().and_then(DataSource::formula_mut));
            }
        }
        out
    }

    /// Every stretch of carried markup, for an edit that rewrites the
    /// references inside it: a data label or a trend line can read a cell too.
    pub(crate) fn markups_mut(&mut self) -> Vec<&mut String> {
        let ChartMarkup {
            before_chart,
            before_plot_area,
            plot_area_layout,
            after_axes,
            after_legend,
            after_chart,
        } = &mut self.markup;
        let mut out = vec![
            before_chart,
            before_plot_area,
            plot_area_layout,
            after_axes,
            after_legend,
            after_chart,
        ];
        out.extend(self.title.as_mut().map(|t| &mut t.markup));
        out.extend(self.legend.as_mut().map(|l| &mut l.markup));
        for axis in &mut self.axes {
            let AxisMarkup {
                gridlines,
                ticks,
                tail,
            } = &mut axis.markup;
            out.extend([gridlines, ticks, tail]);
            out.extend(axis.title.as_mut().map(|t| &mut t.markup));
        }
        for plot in &mut self.plots {
            out.push(&mut plot.markup);
            // A label can show a cell (`c:dLbl/c:tx/c:strRef`).
            out.extend(plot.labels.as_mut().and_then(|l| l.source.as_mut()));
            for series in &mut plot.series {
                out.extend(series.labels.as_mut().and_then(|l| l.source.as_mut()));
                let SeriesMarkup {
                    after_format,
                    before_data,
                    after_data,
                } = &mut series.markup;
                out.extend([after_format, before_data, after_data]);
                for data in [
                    &mut series.categories,
                    &mut series.values,
                    &mut series.bubble_sizes,
                ] {
                    if let Some(DataSource::Levels { formula, markup }) = data {
                        out.push(markup);
                        out.extend(formula.as_mut());
                    }
                }
            }
        }
        out
    }
}

/// Where the chart came from, so an untouched one can go back as it was.
///
/// Opaque on purpose: nothing in it is a property of the chart.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartOrigin {
    /// The drawing part holding the frame.
    pub(crate) drawing: String,
    /// The chart part.
    pub(crate) part: String,
    /// The chart as it was read.
    pub(crate) read: Box<Chart>,
    /// The attributes of the root element, namespace declarations included:
    /// the carried markup uses whatever prefixes they bind.
    pub(crate) root_attributes: String,
    /// The prefix the part gave the chart namespace, empty for a default
    /// namespace (which is how excelize writes it).
    pub(crate) prefix: String,
    /// Whether the frame sits inside a group of shapes.
    pub(crate) grouped: bool,
}

impl ChartOrigin {
    /// The chart part inside the package.
    #[must_use]
    pub fn part(&self) -> &str {
        &self.part
    }

    /// The drawing part the frame lives in.
    #[must_use]
    pub fn drawing(&self) -> &str {
        &self.drawing
    }
}

/// Where a drawing object sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// Between two cells: each corner is tied to its own.
    TwoCell {
        /// The top left corner.
        from: Marker,
        /// The bottom right corner.
        to: Marker,
        /// What moving or resizing the cells underneath does to the object.
        /// `None` is the format's default, which behaves as
        /// [`EditAs::TwoCell`].
        edit_as: Option<EditAs>,
    },
    /// At a cell, with a size of its own in EMU.
    OneCell {
        /// The top left corner.
        from: Marker,
        /// Width, in EMU (914 400 to the inch).
        width: i64,
        /// Height, in EMU.
        height: i64,
    },
    /// At a point on the sheet, in EMU, whatever the cells do.
    Absolute {
        /// Distance from the left edge of the sheet.
        x: i64,
        /// Distance from the top edge.
        y: i64,
        /// Width.
        width: i64,
        /// Height.
        height: i64,
    },
}

impl Default for Anchor {
    /// Eight columns by fifteen rows from `A1`, the size Excel gives a new
    /// chart.
    fn default() -> Self {
        Self::TwoCell {
            from: Marker::default(),
            to: Marker {
                col: Col::new(8).unwrap_or_default(),
                row: Row::new(15).unwrap_or_default(),
                ..Marker::default()
            },
            edit_as: None,
        }
    }
}

/// A corner of an object: a cell, and how far into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Marker {
    /// The column.
    pub col: Col,
    /// How far right of the column's left edge, in EMU.
    pub col_offset: i64,
    /// The row.
    pub row: Row,
    /// How far below the row's top edge, in EMU.
    pub row_offset: i64,
}

/// What a two-cell anchor does when the cells under it move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAs {
    /// Moves and resizes with the cells.
    TwoCell,
    /// Moves with its top left cell, keeping its size.
    OneCell,
    /// Stays where it is.
    Absolute,
}

impl EditAs {
    /// Reads the `editAs` attribute.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "twoCell" => Some(Self::TwoCell),
            "oneCell" => Some(Self::OneCell),
            "absolute" => Some(Self::Absolute),
            _ => None,
        }
    }

    /// The attribute value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TwoCell => "twoCell",
            Self::OneCell => "oneCell",
            Self::Absolute => "absolute",
        }
    }
}

/// Text a chart shows: typed in, or read from a cell.
#[derive(Debug, Clone, PartialEq)]
pub enum ChartText {
    /// Read from a cell, and the value it had when the file was saved.
    Reference {
        /// The cell, as a formula: `Sheet1!$B$1`.
        formula: String,
        /// What the cell held.
        cache: Option<String>,
    },
    /// Typed into the chart.
    Text {
        /// The words, paragraphs separated by `\n`.
        text: String,
        /// The formatted text it was read from, carried whole: it is used
        /// again as long as it still says `text`, and dropped for plain text
        /// once `text` is changed.
        rich: Option<String>,
    },
}

impl ChartText {
    /// Plain text, with no formatting.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            rich: None,
        }
    }

    /// The words shown, whichever way they got there.
    #[must_use]
    pub fn shown(&self) -> Option<&str> {
        match self {
            Self::Reference { cache, .. } => cache.as_deref(),
            Self::Text { text, .. } => Some(text),
        }
    }

    fn formula_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Reference { formula, .. } => Some(formula),
            Self::Text { .. } => None,
        }
    }
}

/// A title over the chart or beside an axis.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Title {
    /// What it says; `None` when Excel makes the text up - a series name over
    /// the chart, nothing beside an axis.
    pub text: Option<ChartText>,
    /// Position, overlay and formatting, carried as written.
    pub markup: String,
}

/// One plot: a chart type and the series drawn that way.
#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    /// The chart type and what it needs said about itself.
    pub kind: PlotKind,
    /// Whether each point gets its own colour. Excel sets it for pies.
    pub vary_colors: Option<bool>,
    /// The series, in the order the file lists them.
    pub series: Vec<Series>,
    /// Ids of the axes this plot is drawn against, from [`Chart::axes`].
    /// Two for a flat chart, three for a 3-D one; none for a pie.
    pub axis_ids: Vec<u32>,
    /// Data labels for every series of the plot that has none of its own.
    pub labels: Option<DataLabels>,
    /// What follows the labels - gap width, overlap, hole size, drop lines -
    /// carried as written.
    pub markup: String,
}

impl Plot {
    /// A plot of that kind and nothing else yet.
    #[must_use]
    pub const fn new(kind: PlotKind) -> Self {
        Self {
            kind,
            vary_colors: None,
            series: Vec::new(),
            axis_ids: Vec::new(),
            labels: None,
            markup: String::new(),
        }
    }
}

/// The chart types of the 2006 schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlotKind {
    /// Columns or bars.
    Bar {
        /// Upright columns or sideways bars.
        direction: BarDirection,
        /// Side by side or stacked.
        grouping: Grouping,
        /// Drawn in 3-D.
        three_d: bool,
    },
    /// Lines.
    Line {
        /// On their own or stacked.
        grouping: Grouping,
        /// Drawn in 3-D.
        three_d: bool,
    },
    /// Filled areas.
    Area {
        /// On their own or stacked.
        grouping: Grouping,
        /// Drawn in 3-D.
        three_d: bool,
    },
    /// A pie.
    Pie {
        /// Drawn in 3-D.
        three_d: bool,
    },
    /// A pie with a hole.
    Doughnut,
    /// A pie with some slices pulled out into a second pie or a bar.
    OfPie {
        /// A bar rather than a second pie.
        bar: bool,
    },
    /// Points at x and y.
    Scatter(ScatterStyle),
    /// A spider web.
    Radar(RadarStyle),
    /// Points at x and y with a size.
    Bubble,
    /// High, low, close and optionally open.
    Stock,
    /// A surface.
    Surface {
        /// Drawn in 3-D.
        three_d: bool,
        /// Only the wires, no fill.
        wireframe: bool,
    },
}

impl PlotKind {
    /// The chart element that draws this kind of plot: `barChart`,
    /// `pie3DChart` and the rest, as the file names them.
    #[must_use]
    pub const fn element(self) -> &'static str {
        match self {
            Self::Bar { three_d: false, .. } => "barChart",
            Self::Bar { three_d: true, .. } => "bar3DChart",
            Self::Line { three_d: false, .. } => "lineChart",
            Self::Line { three_d: true, .. } => "line3DChart",
            Self::Area { three_d: false, .. } => "areaChart",
            Self::Area { three_d: true, .. } => "area3DChart",
            Self::Pie { three_d: false } => "pieChart",
            Self::Pie { three_d: true } => "pie3DChart",
            Self::Doughnut => "doughnutChart",
            Self::OfPie { .. } => "ofPieChart",
            Self::Scatter(_) => "scatterChart",
            Self::Radar(_) => "radarChart",
            Self::Bubble => "bubbleChart",
            Self::Stock => "stockChart",
            Self::Surface { three_d: false, .. } => "surfaceChart",
            Self::Surface { three_d: true, .. } => "surface3DChart",
        }
    }

    /// Whether the plot is drawn against axes at all.
    #[must_use]
    pub const fn has_axes(self) -> bool {
        !matches!(self, Self::Pie { .. } | Self::Doughnut | Self::OfPie { .. })
    }

    /// Whether the series place their points by x and y values rather than by
    /// category.
    #[must_use]
    pub const fn plots_xy(self) -> bool {
        matches!(self, Self::Scatter(_) | Self::Bubble)
    }
}

/// Which way the bars go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BarDirection {
    /// Upright: columns.
    #[default]
    Column,
    /// Sideways: bars.
    Bar,
}

/// How the series of a plot are laid out against each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grouping {
    /// Each series on its own, one behind another.
    #[default]
    Standard,
    /// Side by side.
    Clustered,
    /// On top of each other.
    Stacked,
    /// On top of each other, scaled to 100 %.
    PercentStacked,
}

impl Grouping {
    /// Reads the `grouping` value.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "clustered" => Self::Clustered,
            "stacked" => Self::Stacked,
            "percentStacked" => Self::PercentStacked,
            _ => Self::Standard,
        }
    }

    /// The value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Clustered => "clustered",
            Self::Stacked => "stacked",
            Self::PercentStacked => "percentStacked",
        }
    }
}

/// How a scatter plot joins its points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScatterStyle {
    /// As the series' own formatting says.
    None,
    /// Straight lines.
    Line,
    /// Straight lines with markers.
    LineMarker,
    /// Markers only.
    #[default]
    Marker,
    /// Smooth lines.
    Smooth,
    /// Smooth lines with markers.
    SmoothMarker,
}

impl ScatterStyle {
    /// Reads the `scatterStyle` value.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "none" => Self::None,
            "line" => Self::Line,
            "lineMarker" => Self::LineMarker,
            "smooth" => Self::Smooth,
            "smoothMarker" => Self::SmoothMarker,
            _ => Self::Marker,
        }
    }

    /// The value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Line => "line",
            Self::LineMarker => "lineMarker",
            Self::Marker => "marker",
            Self::Smooth => "smooth",
            Self::SmoothMarker => "smoothMarker",
        }
    }
}

/// How a radar plot draws its series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RadarStyle {
    /// Lines.
    #[default]
    Standard,
    /// Lines with markers.
    Marker,
    /// Filled.
    Filled,
}

impl RadarStyle {
    /// Reads the `radarStyle` value.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "marker" => Self::Marker,
            "filled" => Self::Filled,
            _ => Self::Standard,
        }
    }

    /// The value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Marker => "marker",
            Self::Filled => "filled",
        }
    }
}

/// One series of a plot.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Series {
    /// Its index, which picks its default colour.
    pub index: u32,
    /// Its place in the drawing order.
    pub order: u32,
    /// The name the legend shows.
    pub name: Option<ChartText>,
    /// The categories along the axis; for a scatter or bubble plot, the x
    /// values.
    pub categories: Option<DataSource>,
    /// The values plotted; for a scatter or bubble plot, the y values.
    pub values: Option<DataSource>,
    /// The size of each bubble, for a bubble plot.
    pub bubble_sizes: Option<DataSource>,
    /// Fill and outline of the series (`c:spPr`); `None` leaves them to the
    /// chart style.
    pub format: Option<ShapeFormat>,
    /// The markers of a line, scatter or radar series (`c:marker`).
    pub marker: Option<SeriesMarker>,
    /// Points formatted apart from the rest (`c:dPt`): a pie gives each slice
    /// its own colour this way.
    pub data_points: Vec<DataPoint>,
    /// The series' data labels (`c:dLbls`), which win over the plot's.
    pub labels: Option<DataLabels>,
    /// Formatting around the data, carried as written.
    pub markup: SeriesMarkup,
}

/// What a series says beyond the model, split where the data sits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SeriesMarkup {
    /// Between the fill and the marker: `invertIfNegative`,
    /// `pictureOptions`, `explosion`.
    pub after_format: String,
    /// Trend lines and error bars - what sits between the labels and the
    /// data.
    pub before_data: String,
    /// Smoothing, bar shape, extensions - everything after the data.
    pub after_data: String,
}

/// Fill and outline of a series, a point or a label (`c:spPr`).
///
/// The fields are what a program asks; `source` is the element as read, so
/// gradients, effects, dashes and colour transforms the model does not name
/// survive. It is written back as is while the fields still say what it says;
/// a changed field is written from the model into it, the rest kept.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShapeFormat {
    /// The fill; `None` when the element does not state one.
    pub fill: Option<Fill>,
    /// The outline (`a:ln`); `None` when there is none.
    pub line: Option<LineFormat>,
    /// The element as read; `None` for one made in code.
    pub source: Option<String>,
}

impl ShapeFormat {
    /// A solid fill of one colour.
    #[must_use]
    pub fn solid(color: ChartColor) -> Self {
        Self {
            fill: Some(Fill::Solid(color)),
            ..Self::default()
        }
    }
}

/// An outline.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LineFormat {
    /// The colour of the stroke; `Some(Fill::None)` hides the line.
    pub fill: Option<Fill>,
    /// The width in EMU (12 700 to the point); `None` for the default.
    pub width: Option<u32>,
}

/// How an area is filled.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    /// Not filled (`a:noFill`).
    None,
    /// One colour (`a:solidFill`).
    Solid(ChartColor),
    /// A gradient, pattern or picture, or a colour of a kind the model does not
    /// read: kept in [`ShapeFormat::source`], not described here.
    Other,
}

/// A `DrawingML` colour: a value or a theme colour, and what is done to it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartColor {
    /// The colour before the transforms.
    pub base: ColorBase,
    /// Transforms, applied in order.
    pub transforms: Vec<ColorTransform>,
}

impl ChartColor {
    /// A plain colour, `RRGGBB`.
    #[must_use]
    pub const fn rgb(rgb: u32) -> Self {
        Self {
            base: ColorBase::Rgb(rgb),
            transforms: Vec::new(),
        }
    }

    /// A colour of the theme, by the name the file uses: `accent1`, `tx1`.
    #[must_use]
    pub fn scheme(name: impl Into<String>) -> Self {
        Self {
            base: ColorBase::Scheme(name.into()),
            transforms: Vec::new(),
        }
    }

    /// The colour as `RRGGBB`, the theme colours looked up in `theme` (the
    /// workbook's [`crate::model::Spreadsheet::theme`]) or in Office's default
    /// one. `None` for a name no theme defines, such as the placeholder
    /// `phClr`. Alpha is not applied.
    #[must_use]
    pub fn resolve(&self, theme: Option<&str>) -> Option<u32> {
        let mut rgb = match &self.base {
            ColorBase::Rgb(rgb) => *rgb & 0x00FF_FFFF,
            ColorBase::Scheme(name) => {
                let id = match name.as_str() {
                    "lt1" | "bg1" => 0,
                    "dk1" | "tx1" => 1,
                    "lt2" | "bg2" => 2,
                    "dk2" | "tx2" => 3,
                    "accent1" => 4,
                    "accent2" => 5,
                    "accent3" => 6,
                    "accent4" => 7,
                    "accent5" => 8,
                    "accent6" => 9,
                    "hlink" => 10,
                    "folHlink" => 11,
                    _ => return None,
                };
                crate::shared::palette::rgb_of(&crate::style::Color::Theme { id, tint: 0 }, theme)?
            }
        };
        for t in &self.transforms {
            rgb = t.apply(rgb);
        }
        Some(rgb)
    }
}

/// Where a [`ChartColor`] starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorBase {
    /// `a:srgbClr`, or the value a system colour had when saved.
    Rgb(u32),
    /// `a:schemeClr`: a theme colour by name.
    Scheme(String),
}

/// A transform of a [`ChartColor`], in thousandths of a percent as the file
/// writes them: `100000` is 100 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTransform {
    /// Luminance multiplied by this much.
    LumMod(i32),
    /// Luminance raised by this much.
    LumOff(i32),
    /// Mixed with white: this much of the colour is kept.
    Tint(i32),
    /// Mixed with black: this much of the colour is kept.
    Shade(i32),
    /// Opacity.
    Alpha(i32),
}

impl ColorTransform {
    /// Reads a transform element by its local name.
    #[must_use]
    pub fn parse(name: &str, value: i32) -> Option<Self> {
        Some(match name {
            "lumMod" => Self::LumMod(value),
            "lumOff" => Self::LumOff(value),
            "tint" => Self::Tint(value),
            "shade" => Self::Shade(value),
            "alpha" => Self::Alpha(value),
            _ => return None,
        })
    }

    /// The element name and value.
    #[must_use]
    pub const fn parts(self) -> (&'static str, i32) {
        match self {
            Self::LumMod(v) => ("lumMod", v),
            Self::LumOff(v) => ("lumOff", v),
            Self::Tint(v) => ("tint", v),
            Self::Shade(v) => ("shade", v),
            Self::Alpha(v) => ("alpha", v),
        }
    }

    // ponytail: tint and shade mix in sRGB; Office mixes in linear RGB, so a
    // strong tint comes out a little darker here. Gamma-correct if it shows.
    fn apply(self, rgb: u32) -> u32 {
        let (_, v) = self.parts();
        let f = f64::from(v) / 100_000.0;
        match self {
            Self::LumMod(_) => crate::shared::palette::map_lightness(rgb, |l| l * f),
            Self::LumOff(_) => crate::shared::palette::map_lightness(rgb, |l| l + f),
            Self::Tint(_) => map_channels(rgb, |c| c * f + (1.0 - f)),
            Self::Shade(_) => map_channels(rgb, |c| c * f),
            Self::Alpha(_) => rgb,
        }
    }
}

fn map_channels(rgb: u32, f: impl Fn(f64) -> f64) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 first"
    )]
    let channel = |shift: u32| {
        let c = f64::from((rgb >> shift) & 0xFF) / 255.0;
        (f(c) * 255.0).round().clamp(0.0, 255.0) as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// The markers of a series (`c:marker`).
///
/// The marker's own fill and outline are not modelled; they stay in
/// `source`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SeriesMarker {
    /// The shape; `None` for the chart's default.
    pub symbol: Option<MarkerSymbol>,
    /// The size in points, 2 to 72.
    pub size: Option<u8>,
    /// The element as read; `None` for one made in code.
    pub source: Option<String>,
}

/// The shape of a marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerSymbol {
    /// Whatever the series' index picks.
    Auto,
    /// No marker.
    None,
    /// A circle.
    Circle,
    /// A short horizontal bar.
    Dash,
    /// A diamond.
    Diamond,
    /// A small dot.
    Dot,
    /// A picture.
    Picture,
    /// A plus sign.
    Plus,
    /// A square.
    Square,
    /// A star.
    Star,
    /// A triangle.
    Triangle,
    /// An x.
    X,
}

impl MarkerSymbol {
    /// Reads the `symbol` value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "auto" => Self::Auto,
            "none" => Self::None,
            "circle" => Self::Circle,
            "dash" => Self::Dash,
            "diamond" => Self::Diamond,
            "dot" => Self::Dot,
            "picture" => Self::Picture,
            "plus" => Self::Plus,
            "square" => Self::Square,
            "star" => Self::Star,
            "triangle" => Self::Triangle,
            "x" => Self::X,
            _ => return None,
        })
    }

    /// The value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::Circle => "circle",
            Self::Dash => "dash",
            Self::Diamond => "diamond",
            Self::Dot => "dot",
            Self::Picture => "picture",
            Self::Plus => "plus",
            Self::Square => "square",
            Self::Star => "star",
            Self::Triangle => "triangle",
            Self::X => "x",
        }
    }
}

/// One point of a series formatted apart from the rest (`c:dPt`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DataPoint {
    /// The point's index in the series.
    pub index: u32,
    /// Its fill and outline.
    pub format: Option<ShapeFormat>,
    /// The element as read, which also holds what is not modelled: a pulled-out
    /// slice, a marker, 3-D bubble; `None` for one made in code.
    pub source: Option<String>,
}

/// Data labels of a series or a plot (`c:dLbls`).
///
/// Labels of single points (`c:dLbl`), number format, font and fill are not
/// modelled and stay in `source`.
#[derive(Debug, Clone, PartialEq, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches, one element each in the file"
)]
pub struct DataLabels {
    /// Hidden altogether (`c:delete`).
    pub deleted: bool,
    /// Where each label sits relative to its point; `None` for the default.
    pub position: Option<LabelPosition>,
    /// Shows the legend key beside the label.
    pub show_legend_key: bool,
    /// Shows the value.
    pub show_value: bool,
    /// Shows the category.
    pub show_category_name: bool,
    /// Shows the series name.
    pub show_series_name: bool,
    /// Shows the share of the whole, on a pie.
    pub show_percent: bool,
    /// The element as read; `None` for one made in code.
    pub source: Option<String>,
}

/// Where a data label sits (`c:dLblPos`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelPosition {
    /// Wherever it fits, on a pie.
    BestFit,
    /// Below the point.
    Bottom,
    /// Centred on it.
    Center,
    /// Inside the bar, at its base.
    InsideBase,
    /// Inside the bar, at its end.
    InsideEnd,
    /// Left of the point.
    Left,
    /// Past the end of the bar or slice.
    OutsideEnd,
    /// Right of the point.
    Right,
    /// Above the point.
    Top,
}

impl LabelPosition {
    /// Reads the `dLblPos` value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "bestFit" => Self::BestFit,
            "b" => Self::Bottom,
            "ctr" => Self::Center,
            "inBase" => Self::InsideBase,
            "inEnd" => Self::InsideEnd,
            "l" => Self::Left,
            "outEnd" => Self::OutsideEnd,
            "r" => Self::Right,
            "t" => Self::Top,
            _ => return None,
        })
    }

    /// The value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BestFit => "bestFit",
            Self::Bottom => "b",
            Self::Center => "ctr",
            Self::InsideBase => "inBase",
            Self::InsideEnd => "inEnd",
            Self::Left => "l",
            Self::OutsideEnd => "outEnd",
            Self::Right => "r",
            Self::Top => "t",
        }
    }
}

/// Where a series gets its numbers or labels.
#[derive(Debug, Clone, PartialEq)]
pub enum DataSource {
    /// Numbers.
    Numbers {
        /// The cells, as a formula; `None` for numbers typed into the chart.
        formula: Option<String>,
        /// The number format the values were shown in.
        format_code: Option<String>,
        /// How many points there are, gaps included.
        count: Option<u32>,
        /// The values when the file was saved, by point index. A missing
        /// index is a gap.
        points: Vec<(u32, f64)>,
    },
    /// Text.
    Strings {
        /// The cells, as a formula; `None` for labels typed into the chart.
        formula: Option<String>,
        /// How many points there are, gaps included.
        count: Option<u32>,
        /// The labels when the file was saved, by point index.
        points: Vec<(u32, String)>,
    },
    /// Categories in several levels - region, then city - carried whole
    /// besides the formula.
    Levels {
        /// The cells, as a formula.
        formula: Option<String>,
        /// The element as written, formula included.
        markup: String,
    },
}

impl DataSource {
    /// Numbers read from cells, with nothing cached: Excel fills the cache in
    /// when it opens the file.
    #[must_use]
    pub fn numbers(formula: impl Into<String>) -> Self {
        Self::Numbers {
            formula: Some(formula.into()),
            format_code: None,
            count: None,
            points: Vec::new(),
        }
    }

    /// Labels read from cells, with nothing cached.
    #[must_use]
    pub fn strings(formula: impl Into<String>) -> Self {
        Self::Strings {
            formula: Some(formula.into()),
            count: None,
            points: Vec::new(),
        }
    }

    /// The cells the data is read from.
    #[must_use]
    pub fn formula(&self) -> Option<&str> {
        match self {
            Self::Numbers { formula, .. }
            | Self::Strings { formula, .. }
            | Self::Levels { formula, .. } => formula.as_deref(),
        }
    }

    /// The same, to rewrite. A multi-level source keeps its formula inside
    /// the carried markup as well, so it is left to that markup's own rewrite.
    fn formula_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Numbers { formula, .. } | Self::Strings { formula, .. } => formula.as_mut(),
            Self::Levels { .. } => None,
        }
    }
}

/// An axis.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartAxis {
    /// What the axis measures.
    pub kind: AxisKind,
    /// The id plots refer to it by.
    pub id: u32,
    /// The id of the axis this one crosses.
    pub cross_axis: u32,
    /// Which edge of the plot it runs along.
    pub position: AxisPosition,
    /// Hidden: the axis exists for the plot to be drawn against, but is not
    /// shown.
    pub deleted: bool,
    /// Runs from maximum to minimum.
    pub reversed: bool,
    /// Lowest value shown; automatic when `None`.
    pub min: Option<f64>,
    /// Highest value shown; automatic when `None`.
    pub max: Option<f64>,
    /// Base of a logarithmic scale.
    pub log_base: Option<f64>,
    /// The title beside the axis.
    pub title: Option<Title>,
    /// The number format of the labels, and whether it follows the source
    /// cells.
    pub number_format: Option<(String, bool)>,
    /// Formatting, carried as written.
    pub markup: AxisMarkup,
}

impl ChartAxis {
    /// An axis of categories along the bottom, crossing `cross_axis`.
    #[must_use]
    pub fn category(id: u32, cross_axis: u32) -> Self {
        Self::new(AxisKind::Category, id, cross_axis, AxisPosition::Bottom)
    }

    /// An axis of values up the left side, crossing `cross_axis`.
    #[must_use]
    pub fn value(id: u32, cross_axis: u32) -> Self {
        Self::new(AxisKind::Value, id, cross_axis, AxisPosition::Left)
    }

    /// An axis with nothing set beyond what it is and where.
    #[must_use]
    pub fn new(kind: AxisKind, id: u32, cross_axis: u32, position: AxisPosition) -> Self {
        Self {
            kind,
            id,
            cross_axis,
            position,
            deleted: false,
            reversed: false,
            min: None,
            max: None,
            log_base: None,
            title: None,
            number_format: None,
            markup: AxisMarkup::default(),
        }
    }
}

/// What an axis says beyond the model, split where the model's elements sit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AxisMarkup {
    /// The major and minor gridlines.
    pub gridlines: String,
    /// Tick marks, label position, line and label formatting.
    pub ticks: String,
    /// Where it crosses, label spacing, units, extensions.
    pub tail: String,
}

/// What an axis measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    /// Categories, spaced evenly.
    Category,
    /// Numbers.
    Value,
    /// Dates, spaced by time.
    Date,
    /// The series, front to back on a 3-D chart.
    Series,
}

impl AxisKind {
    /// The element name.
    #[must_use]
    pub const fn element(self) -> &'static str {
        match self {
            Self::Category => "catAx",
            Self::Value => "valAx",
            Self::Date => "dateAx",
            Self::Series => "serAx",
        }
    }
}

/// An edge of the plot area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AxisPosition {
    /// Along the bottom.
    #[default]
    Bottom,
    /// Up the left.
    Left,
    /// Up the right.
    Right,
    /// Along the top.
    Top,
}

impl AxisPosition {
    /// Reads the `axPos` value.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "l" => Self::Left,
            "r" => Self::Right,
            "t" => Self::Top,
            _ => Self::Bottom,
        }
    }

    /// The value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bottom => "b",
            Self::Left => "l",
            Self::Right => "r",
            Self::Top => "t",
        }
    }
}

/// A legend.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Legend {
    /// Where it sits.
    pub position: LegendPosition,
    /// Hidden entries, layout, overlay and formatting, carried as written.
    pub markup: String,
}

/// Where the legend sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LegendPosition {
    /// Below the plot.
    Bottom,
    /// Left of it.
    Left,
    /// Right of it.
    #[default]
    Right,
    /// Above it.
    Top,
    /// In the top right corner.
    TopRight,
}

impl LegendPosition {
    /// Reads the `legendPos` value.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "b" => Self::Bottom,
            "l" => Self::Left,
            "t" => Self::Top,
            "tr" => Self::TopRight,
            _ => Self::Right,
        }
    }

    /// The value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bottom => "b",
            Self::Left => "l",
            Self::Right => "r",
            Self::Top => "t",
            Self::TopRight => "tr",
        }
    }
}

/// What a chart says beyond the model, split where the model's elements sit.
///
/// Each slot is the markup between two elements the model does name, so a
/// chart rendered from the model puts everything back in the order the schema
/// demands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChartMarkup {
    /// Before the chart: the 1904 flag, language, rounded corners, style.
    pub before_chart: String,
    /// Between the title and the plot area: 3-D view, floor and walls.
    pub before_plot_area: String,
    /// The layout of the plot area.
    pub plot_area_layout: String,
    /// After the axes, inside the plot area: data table, fill, extensions.
    pub after_axes: String,
    /// After the legend: visible cells only, blanks, extensions.
    pub after_legend: String,
    /// After the chart: chart area formatting, text, print settings.
    pub after_chart: String,
}

/// A chart of the 2016 schema: waterfall, funnel, treemap and the rest.
///
/// An untouched one goes back as the bytes it came in. A changed one has what
/// the model names rewritten inside its part - the frame's name and anchor, the
/// title, each series' layout, name, visibility and data - and keeps the rest.
/// One made in code gets a part of its own with Excel's defaults.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartEx {
    /// The name the frame carries.
    pub name: String,
    /// Where the chart sits on the sheet.
    pub anchor: Anchor,
    /// The chart part inside the package; empty for a chart made in code.
    pub part: String,
    /// The title, when it has one of its own.
    pub title: Option<ChartText>,
    /// The series, each with the data it reads.
    pub series: Vec<ExSeries>,
    /// Where it was read from; `None` for a chart made in code.
    pub origin: Option<ChartExOrigin>,
}

impl ChartEx {
    /// A new chart with one series of `layout` reading `dimensions`.
    #[must_use]
    pub fn new(layout: SeriesLayout, dimensions: Vec<Dimension>, anchor: Anchor) -> Self {
        Self {
            anchor,
            series: vec![ExSeries {
                layout,
                name: None,
                hidden: false,
                dimensions,
            }],
            ..Self::default()
        }
    }

    /// Whether the chart still says what it said when it was read.
    #[must_use]
    pub fn is_unchanged(&self) -> bool {
        self.origin
            .as_ref()
            .is_some_and(|o| self.same_placement(&o.read) && self.same_content(&o.read))
    }

    /// Whether the frame is where, and what, it was.
    pub(crate) fn same_placement(&self, other: &Self) -> bool {
        self.name == other.name && self.anchor == other.anchor
    }

    /// Whether the chart part would say the same thing.
    pub(crate) fn same_content(&self, other: &Self) -> bool {
        self.title == other.title && self.series == other.series
    }

    /// Takes the chart as it stands for what was read, for an edit that moved
    /// the bytes and the model the same way.
    pub(crate) fn settle(&mut self) {
        if let Some(mut origin) = self.origin.take() {
            origin.read = Box::new(self.clone());
            self.origin = Some(origin);
        }
    }
}

/// Where a 2016 chart came from, so an untouched one goes back as it was.
///
/// Opaque on purpose: nothing in it is a property of the chart.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartExOrigin {
    /// The drawing part holding the frame.
    pub(crate) drawing: String,
    /// The frame's drawing object id.
    pub(crate) id: u32,
    /// Whether the frame sits inside a group of shapes.
    pub(crate) grouped: bool,
    /// The chart as it was read.
    pub(crate) read: Box<ChartEx>,
}

impl ChartExOrigin {
    /// The drawing part the frame lives in.
    #[must_use]
    pub fn drawing(&self) -> &str {
        &self.drawing
    }
}

/// One series of a [`ChartEx`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExSeries {
    /// How the series is drawn, which is what makes the chart a waterfall or a
    /// funnel.
    pub layout: SeriesLayout,
    /// The name the legend shows.
    pub name: Option<ChartText>,
    /// Hidden from the chart.
    pub hidden: bool,
    /// The data it reads, one dimension per role.
    pub dimensions: Vec<Dimension>,
}

/// What a [`ChartEx`] series draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeriesLayout {
    /// Box and whisker.
    BoxWhisker,
    /// Histogram columns.
    ClusteredColumn,
    /// Funnel.
    Funnel,
    /// The cumulative line of a Pareto chart.
    ParetoLine,
    /// Filled map.
    RegionMap,
    /// Sunburst.
    Sunburst,
    /// Treemap.
    Treemap,
    /// Waterfall.
    Waterfall,
    /// A layout newer than this crate.
    Unknown,
}

impl SeriesLayout {
    /// Reads the `layoutId` attribute.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "boxWhisker" => Self::BoxWhisker,
            "clusteredColumn" => Self::ClusteredColumn,
            "funnel" => Self::Funnel,
            "paretoLine" => Self::ParetoLine,
            "regionMap" => Self::RegionMap,
            "sunburst" => Self::Sunburst,
            "treemap" => Self::Treemap,
            "waterfall" => Self::Waterfall,
            _ => Self::Unknown,
        }
    }

    /// The attribute value; `None` for a layout this crate does not know.
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        Some(match self {
            Self::BoxWhisker => "boxWhisker",
            Self::ClusteredColumn => "clusteredColumn",
            Self::Funnel => "funnel",
            Self::ParetoLine => "paretoLine",
            Self::RegionMap => "regionMap",
            Self::Sunburst => "sunburst",
            Self::Treemap => "treemap",
            Self::Waterfall => "waterfall",
            Self::Unknown => return None,
        })
    }
}

/// One dimension of the data a [`ChartEx`] series reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Dimension {
    /// What the dimension is for.
    pub role: DimensionRole,
    /// Numbers rather than text.
    pub numeric: bool,
    /// The cells, as a formula. Excel writes a hidden defined name here -
    /// `_xlchart.v1.0` - rather than the range: see [`Dimension::reference`].
    pub formula: Option<String>,
    /// The values when the file was saved: one list per level, each by point
    /// index. A hierarchy - a treemap's region and city - has several.
    pub levels: Vec<Vec<(u32, String)>>,
}

impl Dimension {
    /// The range the dimension reads, looking through the hidden name Excel
    /// puts in its place.
    #[must_use]
    pub fn reference<'a>(&'a self, names: &'a [DefinedName]) -> Option<&'a str> {
        let formula = self.formula.as_deref()?;
        Some(
            names
                .iter()
                .find(|n| n.sheet.is_none() && n.name == formula)
                .map_or(formula, |n| n.formula.as_str()),
        )
    }
}

/// What a dimension of chart data is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimensionRole {
    /// Category labels.
    Categories,
    /// Values.
    Values,
    /// Sizes, as in a treemap.
    Sizes,
    /// X values.
    X,
    /// Y values.
    Y,
    /// Numbers a map colours by.
    ColorValues,
    /// Text a map colours by.
    ColorStrings,
    /// Map region ids.
    EntityIds,
}

impl DimensionRole {
    /// Reads the `type` attribute.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "cat" => Self::Categories,
            "val" => Self::Values,
            "size" => Self::Sizes,
            "x" => Self::X,
            "y" => Self::Y,
            "colorVal" => Self::ColorValues,
            "colorStr" => Self::ColorStrings,
            "entityId" => Self::EntityIds,
            _ => return None,
        })
    }

    /// The attribute value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Categories => "cat",
            Self::Values => "val",
            Self::Sizes => "size",
            Self::X => "x",
            Self::Y => "y",
            Self::ColorValues => "colorVal",
            Self::ColorStrings => "colorStr",
            Self::EntityIds => "entityId",
        }
    }
}
