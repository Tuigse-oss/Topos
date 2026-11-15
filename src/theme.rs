//! Light and dark colours for everything Topos draws: the map and the
//! window around it.
//!
//! The light palette is the classic look: pastel folders by depth, black
//! captions, a grey toolbar. The dark palette keeps the same hue order for
//! each depth in deep, saturated tones (not browns and olives), so light
//! captions read on every tile. Both are pinned by
//! tests to WCAG AA contrast (4.5:1) for every text colour on every
//! background it is drawn on.

use crate::view::Rgb;

/// The Appearance choice in Setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    /// Light or dark as Windows' app mode is set, following changes live.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    /// The word the settings file stores.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.code() == text)
    }

    /// Whether to draw dark, given whether Windows apps are set to dark.
    #[must_use]
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            Appearance::System => system_dark,
            Appearance::Light => false,
            Appearance::Dark => true,
        }
    }
}

/// Every colour the map and the window paint.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub dark: bool,
    // ---- the map ----
    /// Folder colours by depth below the visible folder, cycling.
    pub depth: [Rgb; 7],
    /// A file is its level's colour moved this far toward white.
    pub file_tint: f32,
    /// File colours by type, in [`crate::view::FileType`] order, when Setup
    /// colours files by type.
    pub types: [Rgb; 8],
    /// Tile outlines.
    pub border: Rgb,
    /// Behind the map, where no tile is.
    pub canvas: Rgb,
    pub free_fill: Rgb,
    pub unreadable_fill: Rgb,
    /// Caption text on tiles.
    pub caption: Rgb,
    pub selected: Rgb,
    pub hovered: Rgb,
    // ---- the window ----
    /// Toolbar, breadcrumb, status row, and dialog background.
    pub face: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub disabled: Rgb,
    pub link: Rgb,
    /// The breadcrumb's note that the map is an earlier scan.
    pub stale: Rgb,
    /// Hairlines between rows.
    pub rule: Rgb,
    /// The drive field: an input-like box in the toolbar.
    pub field: Rgb,
    pub field_edge: Rgb,
    pub tip_fill: Rgb,
    pub tip_edge: Rgb,
    pub hover_fill: Rgb,
    pub hover_edge: Rgb,
    pub down_fill: Rgb,
    pub down_edge: Rgb,
}

pub const LIGHT: Palette = Palette {
    dark: false,
    depth: crate::view::DEPTH_COLORS,
    file_tint: crate::view::FILE_TINT,
    types: [
        Rgb::new(0xF5, 0xA3, 0xA3), // video
        Rgb::new(0xD7, 0xB4, 0xF0), // audio
        Rgb::new(0xA8, 0xE3, 0xA0), // images
        Rgb::new(0xE6, 0xC3, 0x9A), // archives and disk images
        Rgb::new(0xA6, 0xC6, 0xF2), // programs
        Rgb::new(0xF4, 0xE3, 0x8A), // documents
        Rgb::new(0x9C, 0xE0, 0xDA), // code and data
        Rgb::new(0xD6, 0xD6, 0xD6), // other
    ],
    border: crate::view::BORDER,
    canvas: crate::view::CANVAS,
    free_fill: crate::view::FREE_FILL,
    unreadable_fill: crate::view::UNREADABLE_FILL,
    caption: Rgb::gray(0),
    selected: Rgb::new(0xD0, 0x10, 0x10),
    hovered: Rgb::gray(0xFF),
    face: Rgb::gray(0xF0),
    text: Rgb::gray(0),
    muted: Rgb::gray(0x60),
    disabled: Rgb::gray(0x6D),
    link: Rgb::new(0x00, 0x5A, 0xB4),
    stale: Rgb::new(0xB0, 0x50, 0x00),
    rule: Rgb::gray(0xC8),
    field: Rgb::gray(0xFF),
    field_edge: Rgb::gray(0x7A),
    tip_fill: Rgb::new(0xFF, 0xFF, 0xF0),
    tip_edge: Rgb::gray(0x76),
    hover_fill: Rgb::new(0xE5, 0xF3, 0xFF),
    hover_edge: Rgb::new(0xCC, 0xE8, 0xFF),
    down_fill: Rgb::new(0xCC, 0xE8, 0xFF),
    down_edge: Rgb::new(0x99, 0xD1, 0xFF),
};

pub const DARK: Palette = Palette {
    dark: true,
    depth: [
        Rgb::new(0x9B, 0x2D, 0x3F), // raspberry
        Rgb::new(0x8E, 0x3C, 0x0E), // burnt orange
        Rgb::new(0x78, 0x4C, 0x14), // dark gold
        Rgb::new(0x24, 0x60, 0x2C), // green
        Rgb::new(0x10, 0x5E, 0x5E), // teal
        Rgb::new(0x35, 0x46, 0xA0), // blue
        Rgb::new(0x7A, 0x2E, 0x86), // violet
    ],
    file_tint: 0.1,
    types: [
        Rgb::new(0x8E, 0x30, 0x36), // video
        Rgb::new(0x5E, 0x3A, 0x86), // audio
        Rgb::new(0x2F, 0x6B, 0x30), // images
        Rgb::new(0x7A, 0x54, 0x20), // archives and disk images
        Rgb::new(0x2F, 0x4F, 0x8F), // programs
        Rgb::new(0x74, 0x60, 0x00), // documents
        Rgb::new(0x1B, 0x66, 0x60), // code and data
        Rgb::new(0x4A, 0x4A, 0x4A), // other
    ],
    border: Rgb::gray(0x12),
    canvas: Rgb::gray(0x2A),
    free_fill: Rgb::gray(0x3A),
    unreadable_fill: Rgb::gray(0x4A),
    caption: Rgb::gray(0xE8),
    selected: Rgb::new(0xFF, 0x5A, 0x5A),
    hovered: Rgb::gray(0xFF),
    face: Rgb::gray(0x20),
    text: Rgb::gray(0xE8),
    muted: Rgb::gray(0xA8),
    disabled: Rgb::gray(0x70),
    link: Rgb::new(0x6C, 0xB4, 0xFF),
    stale: Rgb::new(0xF0, 0xA0, 0x60),
    rule: Rgb::gray(0x3C),
    field: Rgb::gray(0x2B),
    field_edge: Rgb::gray(0x5A),
    tip_fill: Rgb::gray(0x2B),
    tip_edge: Rgb::gray(0x70),
    hover_fill: Rgb::new(0x2A, 0x3B, 0x4D),
    hover_edge: Rgb::new(0x3D, 0x5A, 0x78),
    down_fill: Rgb::new(0x24, 0x40, 0x5C),
    down_edge: Rgb::new(0x4A, 0x7B, 0xA8),
};

impl Palette {
    /// The palette for a resolved darkness.
    #[must_use]
    pub fn of(dark: bool) -> &'static Palette {
        if dark { &DARK } else { &LIGHT }
    }
}

/// WCAG relative luminance.
#[must_use]
pub fn luminance(color: Rgb) -> f64 {
    let linear = |channel: u8| {
        let c = f64::from(channel) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// WCAG contrast ratio, 1 to 21.
#[must_use]
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AA: f64 = 4.5;

    fn every_palette() -> [&'static Palette; 2] {
        [&LIGHT, &DARK]
    }

    #[test]
    fn appearance_codes_round_trip_and_resolve() {
        for mode in Appearance::ALL {
            assert_eq!(Appearance::parse(mode.code()), Some(mode));
        }
        assert_eq!(Appearance::parse("Dark"), None, "codes are exact");
        assert_eq!(Appearance::default(), Appearance::System);
        assert!(Appearance::System.is_dark(true));
        assert!(!Appearance::System.is_dark(false));
        assert!(Appearance::Dark.is_dark(false));
        assert!(!Appearance::Light.is_dark(true));
        assert!(Palette::of(true).dark && !Palette::of(false).dark);
    }

    #[test]
    fn captions_read_on_every_tile() {
        for palette in every_palette() {
            let mut fills = vec![palette.free_fill, palette.unreadable_fill];
            for color in palette.depth {
                fills.push(color);
                fills.push(color.lighten(palette.file_tint));
            }
            fills.extend(palette.types);
            for fill in fills {
                let ratio = contrast(palette.caption, fill);
                assert!(
                    ratio >= AA,
                    "dark={} caption on {fill:?}: {ratio:.2}",
                    palette.dark
                );
            }
        }
    }

    #[test]
    fn window_text_reads_on_its_background() {
        for p in every_palette() {
            for (name, fg, bg) in [
                ("text", p.text, p.face),
                ("muted", p.muted, p.face),
                ("link", p.link, p.face),
                ("stale", p.stale, p.face),
                ("text on hover", p.text, p.hover_fill),
                ("text pressed", p.text, p.down_fill),
                ("tip", p.text, p.tip_fill),
                ("drive field", p.text, p.field),
            ] {
                let ratio = contrast(fg, bg);
                assert!(ratio >= AA, "dark={} {name}: {ratio:.2}", p.dark);
            }
        }
    }

    #[test]
    fn neighbouring_depths_stay_apart() {
        for p in every_palette() {
            for (index, &color) in p.depth.iter().enumerate() {
                let next = p.depth[(index + 1) % p.depth.len()];
                let distance = i32::from(color.r.abs_diff(next.r))
                    + i32::from(color.g.abs_diff(next.g))
                    + i32::from(color.b.abs_diff(next.b));
                assert!(distance >= 40, "dark={} depth {index}", p.dark);
                // Outlines show against every fill.
                assert!(
                    contrast(p.border, color) >= 1.8,
                    "dark={} depth {index}",
                    p.dark
                );
            }
        }
    }

    #[test]
    fn file_types_are_told_apart() {
        for p in every_palette() {
            for (index, &a) in p.types.iter().enumerate() {
                for &b in &p.types[index + 1..] {
                    let distance = i32::from(a.r.abs_diff(b.r))
                        + i32::from(a.g.abs_diff(b.g))
                        + i32::from(a.b.abs_diff(b.b));
                    assert!(distance >= 40, "dark={} {a:?} vs {b:?}", p.dark);
                }
                assert!(contrast(p.border, a) >= 1.8, "dark={} type {index}", p.dark);
            }
        }
    }

    #[test]
    fn the_light_palette_is_the_classic_map() {
        assert_eq!(LIGHT.depth, crate::view::DEPTH_COLORS);
        assert_eq!(LIGHT.border, crate::view::BORDER);
        assert_eq!(LIGHT.canvas, crate::view::CANVAS);
    }
}
