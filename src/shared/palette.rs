//! The legacy colour palette xls names its colours by, and turning the other
//! kinds of [`Color`] into plain rgb for it.
//!
//! BIFF8 has no rgb colours at all: a font, a border or a fill names an index
//! into a 56-entry palette (indices 8 to 63; 0 to 7 repeat the first eight
//! entries of the old 16-colour one). A file may override the palette with a
//! `PALETTE` record. Reading resolves each index to rgb straight away, since an
//! index means nothing once it leaves the file it was defined in; writing goes
//! the other way and has to pick an index for every rgb the workbook uses.

use crate::style::Color;

/// Excel's default palette, entries 8 to 63, as `RRGGBB`.
pub const DEFAULT: [u32; 56] = [
    0x00_0000, 0xFF_FFFF, 0xFF_0000, 0x00_FF00, 0x00_00FF, 0xFF_FF00, 0xFF_00FF, 0x00_FFFF,
    0x80_0000, 0x00_8000, 0x00_0080, 0x80_8000, 0x80_0080, 0x00_8080, 0xC0_C0C0, 0x80_8080,
    0x99_99FF, 0x99_3366, 0xFF_FFCC, 0xCC_FFFF, 0x66_0066, 0xFF_8080, 0x00_66CC, 0xCC_CCFF,
    0x00_0080, 0xFF_00FF, 0xFF_FF00, 0x00_FFFF, 0x80_0080, 0x80_0000, 0x00_8080, 0x00_00FF,
    0x00_CCFF, 0xCC_FFFF, 0xCC_FFCC, 0xFF_FF99, 0x99_CCFF, 0xFF_99CC, 0xCC_99FF, 0xFF_CC99,
    0x33_66FF, 0x33_CCCC, 0x99_CC00, 0xFF_CC00, 0xFF_9900, 0xFF_6600, 0x66_6699, 0x96_9696,
    0x00_3366, 0x33_9966, 0x00_3300, 0x33_3300, 0x99_3300, 0x99_3366, 0x33_3399, 0x33_3333,
];

/// The first index the 56-entry palette answers to.
pub const FIRST: u16 = 8;

/// Where the palette ends: indices past it are the system colours (window
/// text, window background, "automatic").
pub const END: u16 = FIRST + 56;

/// The colour a palette index stands for, or `Color::Auto` for the system
/// colours past the end of the palette.
#[must_use]
pub fn resolve(palette: &[u32; 56], index: u16) -> Color {
    let slot = match index {
        0..FIRST => usize::from(index),
        FIRST..END => usize::from(index - FIRST),
        _ => return Color::Auto,
    };
    Color::Argb(0xFF00_0000 | palette[slot])
}

/// A colour as plain `RRGGBB`, or `None` when it has no fixed value
/// ([`Color::Auto`]).
///
/// A theme colour is looked up in the workbook's theme when there is one, and
/// in Office's default theme otherwise, then tinted.
#[must_use]
pub fn rgb_of(color: &Color, theme: Option<&str>) -> Option<u32> {
    match color {
        Color::Auto => None,
        Color::Argb(argb) => Some(argb & 0x00FF_FFFF),
        Color::Indexed(index) => match u16::try_from(*index) {
            Ok(index) => match resolve(&DEFAULT, index) {
                Color::Argb(argb) => Some(argb & 0x00FF_FFFF),
                _ => None,
            },
            Err(_) => None,
        },
        Color::Theme { id, tint } => {
            let base = theme
                .and_then(|xml| theme_colors(xml).get(*id as usize).copied())
                .or_else(|| OFFICE_THEME.get(*id as usize).copied())?;
            Some(tinted(base, f64::from(*tint) / 1_000_000.0))
        }
    }
}

/// Office's default theme, in the order `theme="N"` counts: light 1, dark 1,
/// light 2, dark 2, six accents, hyperlink, followed hyperlink.
const OFFICE_THEME: [u32; 12] = [
    0xFF_FFFF, 0x00_0000, 0xE7_E6E6, 0x44_546A, 0x44_72C4, 0xED_7D31, 0xA5_A5A5, 0xFF_C000,
    0x5B_9BD5, 0x70_AD47, 0x05_63C1, 0x95_4F72,
];

/// The colour scheme of a theme part, in the order `theme="N"` counts.
///
/// The part lists dark 1 before light 1 and dark 2 before light 2, and a
/// theme index names them the other way round - a quirk every reader of the
/// format has to know. System colours (`<a:sysClr>`) are taken at the value
/// the saving machine recorded in `lastClr`.
fn theme_colors(xml: &str) -> Vec<u32> {
    let Some(scheme) = xml
        .find("clrScheme")
        .and_then(|start| xml.get(start..))
        .map(|rest| rest.find("/a:clrScheme>").map_or(rest, |end| &rest[..end]))
    else {
        return Vec::new();
    };
    let mut colors = Vec::new();
    let mut rest = scheme;
    while let Some(at) = rest.find(" val=\"").or_else(|| rest.find(" lastClr=\"")) {
        // Of the two attributes, the one that carries the colour: `lastClr` on
        // a system colour, `val` on an rgb one.
        let tag_start = rest[..at].rfind('<').unwrap_or(0);
        let tag = &rest[tag_start..];
        let value = if tag.starts_with("<a:sysClr") {
            attribute(tag, "lastClr")
        } else {
            attribute(tag, "val")
        };
        if let Some(rgb) = value.and_then(|v| u32::from_str_radix(v, 16).ok()) {
            colors.push(rgb);
        }
        rest = tag.find('>').map_or("", |end| &tag[end..]);
    }
    if colors.len() >= 4 {
        colors.swap(0, 1);
        colors.swap(2, 3);
    }
    colors
}

/// One attribute of the tag a slice starts with.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// A colour with Excel's tint applied: the lightness in HSL moves toward white
/// for a positive tint and toward black for a negative one.
fn tinted(rgb: u32, tint: f64) -> u32 {
    if tint == 0.0 {
        return rgb;
    }
    let channel = |shift: u32| f64::from((rgb >> shift) & 0xFF) / 255.0;
    let (red, green, blue) = (channel(16), channel(8), channel(0));
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let lightness = f64::midpoint(max, min);
    let delta = max - min;
    let (hue, saturation) = if delta == 0.0 {
        (0.0, 0.0)
    } else {
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if (max - red).abs() < f64::EPSILON {
            ((green - blue) / delta).rem_euclid(6.0)
        } else if (max - green).abs() < f64::EPSILON {
            (blue - red) / delta + 2.0
        } else {
            (red - green) / delta + 4.0
        };
        (hue * 60.0, saturation)
    };
    let lightness = if tint < 0.0 {
        lightness * (1.0 + tint)
    } else {
        lightness * (1.0 - tint) + tint
    };

    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let second = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let offset = lightness - chroma / 2.0;
    let (red, green, blue) = match hue {
        h if h < 60.0 => (chroma, second, 0.0),
        h if h < 120.0 => (second, chroma, 0.0),
        h if h < 180.0 => (0.0, chroma, second),
        h if h < 240.0 => (0.0, second, chroma),
        h if h < 300.0 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255 first"
    )]
    let byte = |v: f64| ((v + offset) * 255.0).round().clamp(0.0, 255.0) as u32;
    (byte(red) << 16) | (byte(green) << 8) | byte(blue)
}

/// The palette a writer lays out: Excel's own, with entries nobody uses
/// replaced by colours the workbook asks for and the default does not have.
#[derive(Debug, Clone)]
pub struct Builder {
    colors: [u32; 56],
    /// Which entries a colour of the workbook already claims.
    taken: [bool; 56],
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            colors: DEFAULT,
            taken: [false; 56],
        }
    }
}

impl Builder {
    /// Plans a palette for a set of colours, in the order they were met.
    ///
    /// Exact matches claim their default entry first, so a workbook that only
    /// uses default colours keeps the default palette unchanged. What is left
    /// takes over entries nobody claimed, from the last one back, and once the
    /// palette is full the rest are drawn with the nearest entry.
    #[must_use]
    pub fn plan(colors: &[u32]) -> Self {
        let mut builder = Self::default();
        let mut missing = Vec::new();
        for &rgb in colors {
            match builder.colors.iter().position(|&c| c == rgb) {
                Some(slot) => builder.taken[slot] = true,
                None if !missing.contains(&rgb) => missing.push(rgb),
                None => {}
            }
        }
        let free: Vec<usize> = (0..56).rev().filter(|&slot| !builder.taken[slot]).collect();
        for (slot, rgb) in free.into_iter().zip(missing) {
            builder.colors[slot] = rgb;
            builder.taken[slot] = true;
        }
        builder
    }

    /// The index to write for a colour: its own entry, or the nearest.
    #[must_use]
    pub fn index_of(&self, rgb: u32) -> u16 {
        let distance = |c: u32| {
            let d = |shift: u32| i64::from((c >> shift) & 0xFF) - i64::from((rgb >> shift) & 0xFF);
            d(16).pow(2) + d(8).pow(2) + d(0).pow(2)
        };
        let slot = self
            .colors
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| distance(**c))
            .map_or(0, |(slot, _)| slot);
        FIRST + u16::try_from(slot).unwrap_or(0)
    }

    /// The palette itself, when it differs from Excel's default and so has to
    /// be written out.
    #[must_use]
    pub fn custom(&self) -> Option<&[u32; 56]> {
        (self.colors != DEFAULT).then_some(&self.colors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_resolve_through_the_palette() {
        assert_eq!(resolve(&DEFAULT, 10), Color::Argb(0xFFFF_0000));
        assert_eq!(resolve(&DEFAULT, 2), Color::Argb(0xFFFF_0000));
        assert_eq!(resolve(&DEFAULT, 0x40), Color::Auto);
        assert_eq!(resolve(&DEFAULT, 0x7FFF), Color::Auto);
    }

    #[test]
    fn a_default_colour_keeps_the_default_palette() {
        let palette = Builder::plan(&[0xFF_0000, 0x00_0080]);
        assert_eq!(palette.custom(), None);
        assert_eq!(palette.index_of(0xFF_0000), 10);
    }

    #[test]
    fn a_new_colour_takes_an_unclaimed_entry() {
        // The last entry, 0x333333, is claimed; the new colour takes the one
        // before it.
        let palette = Builder::plan(&[0x33_3333, 0x12_3456]);
        assert_eq!(palette.index_of(0x33_3333), 63);
        assert_eq!(palette.index_of(0x12_3456), 62);
        assert!(palette.custom().is_some());
    }

    #[test]
    fn a_theme_colour_resolves_through_the_theme_and_its_tint() {
        let theme = r#"<a:clrScheme name="x"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="1F497D"/></a:dk2><a:lt2><a:srgbClr val="EEECE1"/></a:lt2><a:accent1><a:srgbClr val="4F81BD"/></a:accent1></a:clrScheme>"#;
        let theme_color = |id, tint| rgb_of(&Color::Theme { id, tint }, Some(theme));
        assert_eq!(theme_color(0, 0), Some(0xFF_FFFF), "light 1 comes first");
        assert_eq!(theme_color(1, 0), Some(0x00_0000));
        assert_eq!(theme_color(3, 0), Some(0x1F_497D));
        assert_eq!(theme_color(4, 0), Some(0x4F_81BD));
        // Excel shows `theme="1" tint="0.5"` as 50% grey.
        assert_eq!(theme_color(1, 500_000), Some(0x80_8080));
        assert_eq!(theme_color(0, -500_000), Some(0x80_8080));
    }
}
