//! The one theme of the interface: gunmetal plates, one muted purple accent,
//! green for gains, amber only for warnings. Every colour, size, font and
//! icon lives here, plus the font handles.
//!
//! The accent is one set of constants ([`Accent`] / [`ACCENT_SET`]): swap
//! the five values there to try another shade.

use bevy::prelude::*;
use bevy::text::{FontFeatureTag, FontFeatures, LetterSpacing};
use citysim_sim::Zone;

/// `0xRRGGBB` plus alpha as a const colour.
pub const fn rgba(hex: u32, a: f32) -> Color {
    Color::srgba(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        a,
    )
}

const fn rgb(hex: u32) -> Color {
    rgba(hex, 1.0)
}

// ---------------------------------------------------------------- accent

/// The accent colour family. Everything selected, focused or "yours" uses
/// these five values and nothing else.
#[derive(Debug, Clone, Copy)]
pub struct Accent {
    /// Lines, markers, meters, the selected outline.
    pub base: Color,
    /// Filled surfaces (active tool, selected segment): top and bottom of
    /// the gradient.
    pub fill_top: Color,
    pub fill_bottom: Color,
    /// Text and icons drawn on an accent fill.
    pub ink: Color,
    /// Accent-coloured text on the dark plates (costs, figures).
    pub text: Color,
}

/// Amethyst (the owner's working choice between five close shades:
/// lavender aea3e0, heather 9990bd, amethyst 8f7bd8, plum a57cc0, iris 7f80d6).
pub const ACCENT_SET: Accent = Accent {
    base: rgb(0x8f7bd8),
    fill_top: rgb(0xa391e6),
    fill_bottom: rgb(0x7c68c8),
    ink: rgb(0xffffff),
    text: rgb(0xb3a4f0),
};

pub const ACCENT: Color = ACCENT_SET.base;
pub const ACCENT_TEXT: Color = ACCENT_SET.text;
pub const ACCENT_INK: Color = ACCENT_SET.ink;

// ---------------------------------------------------------------- gunmetal

/// Text on the plates.
pub const TEXT: Color = rgb(0xeef1f5);
/// Captions, labels.
pub const TEXT_MUTED: Color = rgb(0x8e97a5);
/// Placeholders, disabled.
pub const TEXT_FAINT: Color = rgba(0x8e97a5, 0.55);
/// Text inside slots and segments.
pub const SLOT_INK: Color = rgb(0xcfd5de);

/// The lighter edge line of every plate.
pub const LINE: Color = rgb(0x525b69);
/// Faint rules between rows.
pub const RULE: Color = rgb(0x333942);

/// Bar, plates and slots are top-to-bottom gradients.
pub const BAR_TOP: Color = rgb(0x2c3139);
pub const BAR_BOTTOM: Color = rgb(0x191c21);
pub const PLATE_TOP: Color = rgb(0x2a2f37);
pub const PLATE_BOTTOM: Color = rgb(0x1b1e24);
pub const SLOT_TOP: Color = rgb(0x262a32);
pub const SLOT_BOTTOM: Color = rgb(0x1c1f25);
/// Hovered slot.
pub const SLOT_HOVER_TOP: Color = rgb(0x323843);
pub const SLOT_HOVER_BOTTOM: Color = rgb(0x252931);
/// Recessed areas: meters, portrait frames, the trip box.
pub const WELL: Color = rgb(0x14161a);
/// Header strip of a panel.
pub const HEAD: Color = rgb(0x14161a);

pub const POSITIVE: Color = rgb(0x5bd38c);
pub const WARNING: Color = rgb(0xe3a53c);
/// Ink on the amber warning ribbon.
pub const WARNING_INK: Color = rgb(0x1d1303);
pub const NEGATIVE: Color = rgb(0xe8685a);
/// Red plate for refusals and debt.
pub const NEGATIVE_FILL_TOP: Color = rgb(0x5a2a2a);
pub const NEGATIVE_FILL_BOTTOM: Color = rgb(0x3b1c1d);

/// Dim backdrop behind the pause menu.
pub const BACKDROP: Color = rgba(0x08090b, 0.62);
/// Under every docked plate (keeps it readable over a bright scene).
pub const SHADOW: Color = rgba(0x000000, 0.42);

pub const ZONE_RESIDENTIAL: Color = rgb(0x48c78e);
pub const ZONE_COMMERCIAL: Color = rgb(0x4c9bf0);
pub const ZONE_INDUSTRIAL: Color = rgb(0xf0a73c);
/// De-zone has no zone colour; a neutral grey.
pub const ZONE_NONE: Color = rgb(0x8e97a5);

/// Grass and asphalt of the tray's picture slots.
pub const PIC_GRASS: Color = rgb(0x6fae58);
pub const PIC_ASPHALT: Color = rgb(0x474d59);

pub fn zone_color(zone: Option<Zone>) -> Color {
    match zone {
        Some(Zone::Residential) => ZONE_RESIDENTIAL,
        Some(Zone::Commercial) => ZONE_COMMERCIAL,
        Some(Zone::Industrial) => ZONE_INDUSTRIAL,
        None => ZONE_NONE,
    }
}

pub fn alpha(c: Color, a: f32) -> Color {
    c.with_alpha(a)
}

/// Linear blend in sRGB space (fine for short UI transitions).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (a.to_srgba(), b.to_srgba());
    let t = t.clamp(0.0, 1.0);
    Color::srgba(
        a.red + (b.red - a.red) * t,
        a.green + (b.green - a.green) * t,
        a.blue + (b.blue - a.blue) * t,
        a.alpha + (b.alpha - a.alpha) * t,
    )
}

// ---------------------------------------------------------------- sizes (logical px at 1920x1080)

/// Width of every plate's edge line.
pub const LW: f32 = 2.0;
/// Height of the bottom bar.
pub const BAR_H: f32 = 92.0;
/// Tool slot size (the active one is `SLOT_ACTIVE_H` tall).
pub const SLOT_W: f32 = 76.0;
pub const SLOT_H: f32 = 74.0;
pub const SLOT_ACTIVE_H: f32 = 84.0;
/// Height of the build tray standing on the bar.
pub const TRAY_H: f32 = 128.0;
/// Width of the right-edge selection panel.
pub const PANEL_W: f32 = 376.0;
/// Chamfer of plate corners.
pub const CUT: f32 = 18.0;
/// Slope of slanted rules and the speed switch (tan 14°).
pub const SLANT: f32 = 0.249;

/// The layout the interface is designed for; `UiScale` grows and shrinks
/// from it (see `ui_scale_for`).
pub const DESIGN_SIZE: Vec2 = Vec2::new(1920.0, 1080.0);
/// The smallest logical window the layout must fit (compact bar).
pub const MIN_LAYOUT: Vec2 = Vec2::new(1250.0, 700.0);
/// Below this logical width the bar compacts (day strip and labels go).
pub const COMPACT_W: f32 = 1660.0;

// Spacing steps.
pub const S2: f32 = 4.0;
pub const S3: f32 = 8.0;
pub const S5: f32 = 16.0;
pub const S6: f32 = 22.0;

// ---------------------------------------------------------------- type

/// Caps captions under figures (letter-spaced).
pub const FONT_CAP: f32 = 11.5;
pub const FONT_KEY: f32 = 10.0;
pub const FONT_SMALL: f32 = 12.5;
pub const FONT_BODY: f32 = 13.0;
pub const FONT_ROW: f32 = 14.0;
pub const FONT_LEAD: f32 = 16.0;
/// Panel header, tray title.
pub const FONT_HEAD: f32 = 19.0;
pub const FONT_TITLE: f32 = 22.0;
/// Bar figures.
pub const FONT_MID: f32 = 24.0;
pub const FONT_BIG: f32 = 32.0;
pub const FONT_CLOCK: f32 = 34.0;

/// Letter spacing of caps labels (em).
pub const CAPS_EM: f32 = 0.09;

pub const ICON_SM: f32 = 15.0;
pub const ICON_MD: f32 = 18.0;
pub const ICON_LG: f32 = 26.0;

pub const W_REGULAR: u16 = 400;
pub const W_MEDIUM: u16 = 500;
pub const W_SEMIBOLD: u16 = 600;
pub const W_BOLD: u16 = 700;

// ---------------------------------------------------------------- icons (Lucide font codepoints)

pub mod icon {
    pub const INSPECT: char = '\u{e1c3}'; // mouse-pointer-2
    pub const ROAD: char = '\u{e6d9}'; // road
    pub const ZONE: char = '\u{e528}'; // land-plot
    pub const GRID_ZONE: char = '\u{e0ff}'; // layout-grid
    pub const BULLDOZE: char = '\u{e28f}'; // eraser
    pub const LAYERS: char = '\u{e529}'; // layers
    pub const HOUSE: char = '\u{e0f5}';
    pub const STORE: char = '\u{e3e4}';
    pub const FACTORY: char = '\u{e29f}';
    pub const ERASER: char = '\u{e28f}';
    pub const PAUSE: char = '\u{e12e}';
    pub const PLAY: char = '\u{e13c}';
    pub const WALLET: char = '\u{e204}';
    pub const USERS: char = '\u{e1a4}';
    pub const USER: char = '\u{e19f}';
    pub const BRIEFCASE: char = '\u{e062}';
    pub const SUN: char = '\u{e178}';
    pub const MOON: char = '\u{e11e}';
    pub const CLOSE: char = '\u{e1b2}';
    pub const CHECK: char = '\u{e06c}';
    pub const CHECK_CIRCLE: char = '\u{e226}';
    pub const ALERT: char = '\u{e193}';
    pub const ERROR: char = '\u{e084}';
    pub const INFO: char = '\u{e0f9}';
    pub const SAVE: char = '\u{e14d}';
    pub const LOAD: char = '\u{e247}';
    pub const MONITOR: char = '\u{e11d}';
    pub const QUIT: char = '\u{e10e}';
    pub const MINUS: char = '\u{e11c}';
    pub const PLUS: char = '\u{e13d}';
    pub const CHEVRON_LEFT: char = '\u{e06e}';
    pub const CHEVRON_RIGHT: char = '\u{e06f}';
    pub const SPLINE: char = '\u{e38b}';
    pub const GRID: char = '\u{e4ff}'; // grid-2x2
    pub const WAYPOINTS: char = '\u{e542}';
    pub const SCAN: char = '\u{e257}'; // rectangle selection
    pub const ROUTE: char = '\u{e53e}';
    pub const PAINT_BUCKET: char = '\u{e2e6}';
    pub const UNDO: char = '\u{e2a1}'; // undo-2
    pub const SETTINGS: char = '\u{e154}';
    pub const SCALING: char = '\u{e2ec}';
    pub const MOVE: char = '\u{e121}';
    pub const BRUSH: char = '\u{e1d3}';
    pub const UPGRADE: char = '\u{e420}'; // arrow-big-up-dash
    pub const CAR: char = '\u{e1d5}';
    pub const TRUCK: char = '\u{e194}';
    pub const EYE: char = '\u{e0ba}';
    pub const CHART: char = '\u{e2a3}'; // bar-chart-3
    pub const KEYBOARD: char = '\u{e284}';
    pub const SLASH: char = '\u{e51d}';
    pub const ROTATE: char = '\u{e148}'; // rotate-ccw
}

// ---------------------------------------------------------------- fonts

/// The interface's fonts: Barlow Semi Condensed for text, Barlow Condensed
/// for figures and titles (both static faces, one handle per weight), and
/// the Lucide icon font. Never use Bevy's default font.
#[derive(Resource, Clone)]
pub struct UiFonts {
    text: [Handle<Font>; 4],
    figures: [Handle<Font>; 3],
    pub icons: Handle<Font>,
}

impl FromWorld for UiFonts {
    fn from_world(world: &mut World) -> Self {
        let assets = world.resource::<AssetServer>();
        UiFonts {
            text: [
                assets.load("ui/fonts/BarlowSemiCondensed-Regular.ttf"),
                assets.load("ui/fonts/BarlowSemiCondensed-Medium.ttf"),
                assets.load("ui/fonts/BarlowSemiCondensed-SemiBold.ttf"),
                assets.load("ui/fonts/BarlowSemiCondensed-Bold.ttf"),
            ],
            figures: [
                assets.load("ui/fonts/BarlowCondensed-Medium.ttf"),
                assets.load("ui/fonts/BarlowCondensed-SemiBold.ttf"),
                assets.load("ui/fonts/BarlowCondensed-Bold.ttf"),
            ],
            icons: assets.load("ui/fonts/lucide.ttf"),
        }
    }
}

impl UiFonts {
    fn face(&self, weight: u16) -> Handle<Font> {
        let i = match weight {
            0..=449 => 0,
            450..=549 => 1,
            550..=649 => 2,
            _ => 3,
        };
        self.text[i].clone()
    }

    /// Body text: Barlow Semi Condensed at `size` and `weight`.
    pub fn text(&self, size: f32, weight: u16) -> TextFont {
        TextFont {
            font: self.face(weight).into(),
            font_size: FontSize::Px(size),
            ..default()
        }
    }

    /// Figures and titles: Barlow Condensed with tabular numerals.
    pub fn figures(&self, size: f32, weight: u16) -> TextFont {
        let i = match weight {
            0..=549 => 0,
            550..=649 => 1,
            _ => 2,
        };
        TextFont {
            font: self.figures[i].clone().into(),
            font_size: FontSize::Px(size),
            font_features: FontFeatures::from([FontFeatureTag::TABULAR_FIGURES]),
            ..default()
        }
    }

    pub fn icon(&self, size: f32) -> TextFont {
        TextFont {
            font: self.icons.clone().into(),
            font_size: FontSize::Px(size),
            ..default()
        }
    }
}

/// Letter spacing for caps labels of `size`.
pub fn caps_spacing(size: f32, em: f32) -> LetterSpacing {
    LetterSpacing::Px(size * em)
}
