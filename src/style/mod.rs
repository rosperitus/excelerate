//! Cell styles.
//!
//!
//! A cell could be handed its own `Style` object, kept in
//! sync through a supervisor object — a workaround for languages with no cheap
//! shared ownership. Here a cell stores a [`StyleId`] and the styles live in a
//! [`StyleTable`]. That mirrors how `cellXfs` works in OOXML itself, which
//! makes reading and writing very nearly an identity mapping.
//!
//! The pseudo-borders some APIs offer (`allBorders`, `outline`,
//! `inside`, `vertical`, `horizontal`) are not carried over: they are an
//! `applyFromArray` convenience, not state a file ever holds.

use std::collections::HashMap;

/// A reference to a style in the workbook's [`StyleTable`].
///
/// [`StyleId::default`] is the default style. It always exists at index 0,
/// just like `cellXfs[0]` in xlsx.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct StyleId(u32);

impl StyleId {
    /// A reference to the style at `index`.
    #[must_use]
    pub const fn from_index(index: u32) -> Self {
        Self(index)
    }

    /// Index into the style table, which is also the index into `cellXfs`.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// A colour.
///
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// No explicit colour; the consumer picks (usually black text on no fill).
    #[default]
    Auto,
    /// Opaque or transparent colour as `AARRGGBB`.
    Argb(u32),
    /// An index into the legacy 56-colour palette, still used by xls.
    Indexed(u32),
    /// A theme colour with a tint in `-1.0..=1.0`, stored in millionths so the
    /// value stays hashable and keeps the precision Excel writes.
    Theme {
        /// Index into the theme's colour scheme.
        id: u32,
        /// Tint applied to the theme colour, in millionths.
        tint: i32,
    },
}

impl Color {
    /// A colour from `AARRGGBB`, as written in xlsx.
    ///
    /// # Errors
    /// Returns `None` if the string is not eight hex digits.
    #[must_use]
    pub fn from_argb_str(s: &str) -> Option<Self> {
        (s.len() == 8)
            .then(|| u32::from_str_radix(s, 16).ok())
            .flatten()
            .map(Self::Argb)
    }

    /// The `AARRGGBB` form, for colours that have one.
    #[must_use]
    pub fn to_argb_str(&self) -> Option<String> {
        match self {
            Self::Argb(v) => Some(format!("{v:08X}")),
            _ => None,
        }
    }
}

/// How text is underlined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Underline {
    /// Not underlined.
    #[default]
    None,
    /// A single line.
    Single,
    /// Two lines.
    Double,
    /// A single line spanning the cell width, used in accounting formats.
    SingleAccounting,
    /// Two lines spanning the cell width.
    DoubleAccounting,
}

impl Underline {
    /// The `val` attribute of `<u>`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Single => "single",
            Self::Double => "double",
            Self::SingleAccounting => "singleAccounting",
            Self::DoubleAccounting => "doubleAccounting",
        }
    }

    /// Parses the `val` attribute of `<u>`; an unknown value reads as a single
    /// underline, since `<u/>` with no value means exactly that.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "none" => Self::None,
            "double" => Self::Double,
            "singleAccounting" => Self::SingleAccounting,
            "doubleAccounting" => Self::DoubleAccounting,
            _ => Self::Single,
        }
    }
}

/// Where text sits vertically relative to the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Script {
    /// On the baseline.
    #[default]
    Baseline,
    /// Raised.
    Superscript,
    /// Lowered.
    Subscript,
}

/// A font.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Font {
    /// Typeface name.
    pub name: String,
    /// Size in hundredths of a point, so the value stays hashable: 1100 is 11pt.
    pub size: u32,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underline style.
    pub underline: Underline,
    /// Struck through.
    pub strike: bool,
    /// Text colour.
    pub color: Color,
    /// Superscript or subscript.
    pub script: Script,
    /// The font family class Excel writes beside the name: 1 roman,
    /// 2 swiss, 3 modern, 4 script, 5 decorative.
    ///
    /// Like the character set, it only matters when the named face is missing
    /// and something has to be substituted for it.
    pub family: Option<u32>,
    /// The character set the font declares, as `<charset val="204"/>`.
    ///
    /// Excel writes it for every font of a non-Latin workbook and picks glyphs
    /// with it; dropping it changes which face a reader falls back to.
    pub charset: Option<u32>,
    /// Which half of the theme's font scheme this is: `major` for headings,
    /// `minor` for body text.
    ///
    /// A font tied to the scheme follows the theme when the theme changes; one
    /// written without the attribute does not, so the two are not the same
    /// font even where every other field matches.
    pub scheme: Option<FontScheme>,
}

/// The half of the theme's font pair a font belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontScheme {
    /// Headings.
    Major,
    /// Body text.
    Minor,
}

impl FontScheme {
    /// Reads the `val` attribute of `<scheme>`.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "major" => Some(Self::Major),
            "minor" => Some(Self::Minor),
            _ => None,
        }
    }

    /// The attribute value, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Minor => "minor",
        }
    }
}

impl Default for Font {
    /// Excel's default font.
    fn default() -> Self {
        Self {
            name: "Calibri".into(),
            size: 1100,
            bold: false,
            italic: false,
            underline: Underline::None,
            strike: false,
            color: Color::Auto,
            script: Script::Baseline,
            family: None,
            charset: None,
            scheme: None,
        }
    }
}

impl Font {
    /// The size in points.
    #[must_use]
    pub fn size_points(&self) -> f64 {
        f64::from(self.size) / 100.0
    }

    /// Sets the size from points, rounding to the hundredth Excel stores.
    pub fn set_size_points(&mut self, points: f64) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "font sizes are small positive numbers"
        )]
        {
            self.size = (points.max(0.0) * 100.0).round() as u32;
        }
    }
}

/// A fill pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Pattern {
    /// No fill.
    #[default]
    None,
    /// A flat fill in the foreground colour.
    Solid,
    /// One of the legacy hatch patterns, by its xlsx name.
    Named(&'static str),
}

/// The hatch patterns xlsx names, other than `none` and `solid`.
const NAMED_PATTERNS: [&str; 16] = [
    "darkDown",
    "darkGray",
    "darkGrid",
    "darkHorizontal",
    "darkTrellis",
    "darkUp",
    "darkVertical",
    "gray0625",
    "gray125",
    "lightDown",
    "lightGray",
    "lightGrid",
    "lightHorizontal",
    "lightTrellis",
    "lightUp",
    "lightVertical",
];

impl Pattern {
    /// The `patternType` attribute.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Solid => "solid",
            Self::Named(name) => name,
        }
    }

    /// Parses a `patternType`. An unknown pattern reads as no fill rather than
    /// being invented, so writing the file back cannot name something Excel
    /// does not know.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "solid" => Self::Solid,
            "none" | "" => Self::None,
            other => NAMED_PATTERNS
                .iter()
                .find(|p| **p == other)
                .map_or(Self::None, |p| Self::Named(p)),
        }
    }
}

/// A cell fill.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Fill {
    /// Pattern type.
    pub pattern: Pattern,
    /// Foreground colour, which for [`Pattern::Solid`] is the fill colour.
    pub foreground: Color,
    /// Background colour, used by the hatch patterns.
    pub background: Color,
}

/// A border line style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BorderStyle {
    /// No line.
    #[default]
    None,
    /// One of the line styles xlsx names.
    Named(&'static str),
}

/// Line styles xlsx names, other than `none`.
const BORDER_STYLES: [&str; 12] = [
    "thin",
    "medium",
    "thick",
    "dashed",
    "dotted",
    "double",
    "hair",
    "mediumDashed",
    "dashDot",
    "mediumDashDot",
    "dashDotDot",
    "mediumDashDotDot",
];

impl BorderStyle {
    /// The `style` attribute of a border side.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Named(name) => name,
        }
    }

    /// Parses a border `style`; anything unrecognised reads as no border.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        BORDER_STYLES
            .iter()
            .find(|b| **b == s)
            .map_or(Self::None, |b| Self::Named(b))
    }
}

/// One side of a cell border.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Border {
    /// Line style.
    pub style: BorderStyle,
    /// Line colour.
    pub color: Color,
}

/// The borders of a cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Borders {
    /// Left edge.
    pub left: Border,
    /// Right edge.
    pub right: Border,
    /// Top edge.
    pub top: Border,
    /// Bottom edge.
    pub bottom: Border,
    /// Diagonal line.
    pub diagonal: Border,
    /// Whether the diagonal runs up, down, both, or is absent.
    pub diagonal_direction: DiagonalDirection,
}

/// Which way a diagonal border runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DiagonalDirection {
    /// No diagonal.
    #[default]
    None,
    /// Bottom-left to top-right.
    Up,
    /// Top-left to bottom-right.
    Down,
    /// Both diagonals.
    Both,
}

/// Horizontal text placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HorizontalAlign {
    /// Excel's default: numbers right, text left.
    #[default]
    General,
    /// Against the left edge.
    Left,
    /// Centred.
    Center,
    /// Against the right edge.
    Right,
    /// Stretched to both edges.
    Justify,
    /// Repeated to fill the width.
    Fill,
    /// Centred across the selected columns.
    CenterContinuous,
    /// Spread evenly, used for East Asian text.
    Distributed,
}

impl HorizontalAlign {
    /// The `horizontal` attribute, or `None` for the default.
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        Some(match self {
            Self::General => return None,
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
            Self::Justify => "justify",
            Self::Fill => "fill",
            Self::CenterContinuous => "centerContinuous",
            Self::Distributed => "distributed",
        })
    }

    /// Parses the `horizontal` attribute.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "left" => Self::Left,
            "center" => Self::Center,
            "right" => Self::Right,
            "justify" => Self::Justify,
            "fill" => Self::Fill,
            "centerContinuous" => Self::CenterContinuous,
            "distributed" => Self::Distributed,
            _ => Self::General,
        }
    }
}

/// Vertical text placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VerticalAlign {
    /// Against the bottom edge, Excel's default.
    #[default]
    Bottom,
    /// Against the top edge.
    Top,
    /// Centred.
    Center,
    /// Stretched to both edges.
    Justify,
    /// Spread evenly.
    Distributed,
}

impl VerticalAlign {
    /// The `vertical` attribute, or `None` for the default.
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        Some(match self {
            Self::Bottom => return None,
            Self::Top => "top",
            Self::Center => "center",
            Self::Justify => "justify",
            Self::Distributed => "distributed",
        })
    }

    /// Parses the `vertical` attribute.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "top" => Self::Top,
            "center" => Self::Center,
            "justify" => Self::Justify,
            "distributed" => Self::Distributed,
            _ => Self::Bottom,
        }
    }
}

/// Text placement inside a cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Alignment {
    /// Horizontal placement.
    pub horizontal: HorizontalAlign,
    /// Vertical placement.
    pub vertical: VerticalAlign,
    /// Wrap long text onto more lines.
    pub wrap_text: bool,
    /// Shrink the text until it fits.
    pub shrink_to_fit: bool,
    /// Indent, in character widths.
    ///
    /// Excel applies it only with [`HorizontalAlign::General`], `Left`, `Right`
    /// or `Distributed`, and ignores it otherwise. The value is stored as given
    /// rather than being silently zeroed the way `Alignment::setIndent` does in
    /// What the file says is what we keep.
    pub indent: u32,
    /// Rotation in degrees, `0..=180`; 255 means stacked vertically, which is
    /// how the format encodes it.
    pub text_rotation: u32,
    /// Which way the text runs: 0 leaves it to the content, 1 is
    /// left-to-right, 2 right-to-left.
    pub reading_order: u32,
}

/// Whether a cell is protected once the sheet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ProtectionState {
    /// Follow the sheet default, which is locked.
    #[default]
    Inherit,
    /// Explicitly on.
    On,
    /// Explicitly off.
    Off,
}

impl ProtectionState {
    /// The attribute value, or `None` when nothing should be written.
    #[must_use]
    pub const fn as_bool(self) -> Option<bool> {
        match self {
            Self::Inherit => None,
            Self::On => Some(true),
            Self::Off => Some(false),
        }
    }

    /// From an xlsx boolean attribute.
    #[must_use]
    pub fn from_attr(s: &str) -> Self {
        if s == "1" || s == "true" {
            Self::On
        } else {
            Self::Off
        }
    }
}

/// Cell protection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Protection {
    /// Whether the cell is locked.
    pub locked: ProtectionState,
    /// Whether its formula is hidden.
    pub hidden: ProtectionState,
}

/// A number format.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum NumberFormat {
    /// `General`, built-in format 0.
    #[default]
    General,
    /// A built-in format by id.
    Builtin(u16),
    /// A custom format string, such as `0.00%` or `yyyy-mm-dd`.
    Custom(String),
}

/// A cell style: everything one `cellXfs` entry points at.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Style {
    /// Number format.
    pub number_format: NumberFormat,
    /// Font.
    pub font: Font,
    /// Fill.
    pub fill: Fill,
    /// Borders.
    pub borders: Borders,
    /// Text placement.
    pub alignment: Alignment,
    /// Protection.
    pub protection: Protection,
}

/// A font as a differential format states it: only the parts that override.
///
/// A `<dxf>` in xlsx says what to change, not what the result is, so every
/// field here is optional. Writing a whole [`Font`] instead would silently
/// impose a name and a size the original never asked for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct DiffFont {
    /// Override the family name.
    pub name: Option<String>,
    /// Override the size, in hundredths of a point.
    pub size: Option<u32>,
    /// Override boldness.
    pub bold: Option<bool>,
    /// Override slant.
    pub italic: Option<bool>,
    /// Override the underline.
    pub underline: Option<Underline>,
    /// Override the strike-through.
    pub strike: Option<bool>,
    /// Override the colour.
    pub color: Option<Color>,
    /// Override the superscript or subscript.
    pub script: Option<Script>,
    /// Override the character set.
    pub charset: Option<u32>,
    /// Override which half of the theme's font pair this run follows.
    pub scheme: Option<FontScheme>,
}

/// A fill as a differential format states it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct DiffFill {
    /// Override the pattern.
    pub pattern: Option<Pattern>,
    /// Override the foreground colour.
    pub foreground: Option<Color>,
    /// Override the background colour, which is the one a solid conditional
    /// fill actually uses.
    pub background: Option<Color>,
}

/// Borders as a differential format states them: each edge on its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct DiffBorders {
    /// Left edge.
    pub left: Option<Border>,
    /// Right edge.
    pub right: Option<Border>,
    /// Top edge.
    pub top: Option<Border>,
    /// Bottom edge.
    pub bottom: Option<Border>,
    /// Diagonal line.
    pub diagonal: Option<Border>,
}

/// A partial style, the kind conditional formatting lays over a cell's own.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct DifferentialStyle {
    /// Font overrides.
    pub font: Option<DiffFont>,
    /// Fill overrides.
    pub fill: Option<DiffFill>,
    /// Border overrides.
    pub borders: Option<DiffBorders>,
    /// Number format override.
    pub number_format: Option<NumberFormat>,
    /// Alignment override.
    pub alignment: Option<Alignment>,
    /// Protection override.
    pub protection: Option<Protection>,
}

/// The workbook's style table: it interns styles, so equal styles share one
/// [`StyleId`].
#[derive(Debug, Clone)]
pub struct StyleTable {
    styles: Vec<Style>,
    index: HashMap<Style, StyleId>,
    /// Partial styles, referred to by index from conditional formatting rules.
    pub differential: Vec<DifferentialStyle>,
}

impl Default for StyleTable {
    /// A table holding just the default style at index 0.
    fn default() -> Self {
        let default = Style::default();
        Self {
            styles: vec![default.clone()],
            index: HashMap::from([(default, StyleId::default())]),
            differential: Vec::new(),
        }
    }
}

impl StyleTable {
    /// Builds a table from styles, one entry per position.
    ///
    /// Positions are preserved even when two entries are equal, because cells
    /// refer to styles by index: collapsing duplicates the way [`Self::intern`]
    /// does would silently point them at the wrong slot. Reading a file is the
    /// case that needs this.
    #[must_use]
    pub fn from_styles(styles: Vec<Style>) -> Self {
        if styles.is_empty() {
            return Self::default();
        }
        let mut index = HashMap::new();
        for (i, style) in styles.iter().enumerate() {
            // First position wins, so interning a duplicate later reuses the
            // earliest id rather than appending another copy.
            index
                .entry(style.clone())
                .or_insert_with(|| StyleId::from_index(u32::try_from(i).unwrap_or(u32::MAX)));
        }
        Self {
            styles,
            index,
            differential: Vec::new(),
        }
    }

    /// Puts a style into the table and returns a reference to it. Equal styles
    /// share one [`StyleId`] and are stored once.
    pub fn intern(&mut self, style: Style) -> StyleId {
        if let Some(&id) = self.index.get(&style) {
            return id;
        }
        // The table cannot outgrow u32: neither the file format nor memory
        // would hold that many styles.
        let id = StyleId::from_index(u32::try_from(self.styles.len()).unwrap_or(u32::MAX));
        self.styles.push(style.clone());
        self.index.insert(style, id);
        id
    }

    /// The style behind a reference. `None` if the id came from another workbook.
    #[must_use]
    pub fn get(&self, id: StyleId) -> Option<&Style> {
        self.styles.get(id.index() as usize)
    }

    /// Every style in index order — the order `cellXfs` is written in.
    #[must_use]
    pub fn all(&self) -> &[Style] {
        &self.styles
    }

    /// How many styles the table holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    /// The table is never empty: the default style is always present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BorderStyle, Color, Font, NumberFormat, Pattern, Style, StyleId, StyleTable, Underline,
    };

    #[test]
    fn default_style_is_index_zero() {
        let t = StyleTable::default();
        assert_eq!(t.len(), 1);
        assert_eq!(t.get(StyleId::default()), Some(&Style::default()));
    }

    #[test]
    fn equal_styles_share_one_id() {
        let mut t = StyleTable::default();
        let percent = Style {
            number_format: NumberFormat::Custom("0.00%".into()),
            ..Style::default()
        };
        let a = t.intern(percent.clone());
        let b = t.intern(percent);
        assert_eq!(a, b, "equal styles are not duplicated");
        assert_eq!(t.len(), 2, "the default style plus one new one");

        let other = t.intern(Style {
            number_format: NumberFormat::Builtin(9),
            ..Style::default()
        });
        assert_ne!(a, other);
        assert_eq!(
            t.intern(Style::default()),
            StyleId::default(),
            "the default is already there"
        );
    }

    #[test]
    fn positional_table_keeps_duplicate_slots() {
        // Two equal entries must keep their own indices, or every cell pointing
        // at the second one would silently move to the first.
        let table = StyleTable::from_styles(vec![Style::default(), Style::default()]);
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(StyleId::from_index(1)), Some(&Style::default()));
    }

    #[test]
    fn unknown_id_is_none() {
        assert!(StyleTable::default().get(StyleId::from_index(99)).is_none());
    }

    #[test]
    fn colors_round_trip_through_argb() {
        let c = Color::from_argb_str("FFFF0000").expect("valid argb");
        assert_eq!(c, Color::Argb(0xFFFF_0000));
        assert_eq!(c.to_argb_str().as_deref(), Some("FFFF0000"));
        assert!(
            Color::from_argb_str("FF0000").is_none(),
            "six digits is not argb"
        );
        assert!(Color::from_argb_str("zzzzzzzz").is_none());
        assert!(Color::Auto.to_argb_str().is_none());
    }

    #[test]
    fn enum_attributes_round_trip() {
        for u in [
            Underline::None,
            Underline::Single,
            Underline::Double,
            Underline::SingleAccounting,
            Underline::DoubleAccounting,
        ] {
            assert_eq!(Underline::parse(u.as_str()), u);
        }
        // `<u/>` with no value means a single underline.
        assert_eq!(Underline::parse(""), Underline::Single);

        for p in [Pattern::None, Pattern::Solid, Pattern::Named("darkGrid")] {
            assert_eq!(Pattern::parse(p.as_str()), p);
        }
        assert_eq!(
            Pattern::parse("nonsense"),
            Pattern::None,
            "unknown patterns are dropped"
        );

        for b in [BorderStyle::None, BorderStyle::Named("mediumDashDot")] {
            assert_eq!(BorderStyle::parse(b.as_str()), b);
        }
    }

    #[test]
    fn font_size_is_stored_in_hundredths() {
        let mut f = Font::default();
        assert!((f.size_points() - 11.0).abs() < f64::EPSILON);
        f.set_size_points(11.5);
        assert_eq!(f.size, 1150, "stored in hundredths of a point");
        assert!((f.size_points() - 11.5).abs() < f64::EPSILON);
    }
}
pub mod format;
