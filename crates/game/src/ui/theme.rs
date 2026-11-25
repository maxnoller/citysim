//! Design tokens: every colour, radius, spacing step, font size and icon of
//! the interface lives here, plus the font handles.

use bevy::prelude::*;
use bevy::text::{FontFeatureTag, FontFeatures, FontWeight};
use citysim_sim::Zone;

/// `0xRRGGBB` plus alpha as a const colour.
const fn rgba(hex: u32, a: f32) -> Color {
    Color::srgba(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        a,
    )
}

// ---------------------------------------------------------------- colours

/// ~88% perceived opacity (UI blends in linear space, so a bit higher).
pub const PANEL: Color = rgba(0x14181D, 0.92);
/// Slightly lifted surface (tooltips, modal card).
pub const PANEL_RAISED: Color = rgba(0x1B2027, 0.97);
pub const PANEL_BORDER: Color = rgba(0xFFFFFF, 0.08);
pub const DIVIDER: Color = rgba(0xFFFFFF, 0.08);
/// Recessed areas: bar tracks, key badges, inputs.
pub const WELL: Color = rgba(0xFFFFFF, 0.06);
pub const SHADOW: Color = rgba(0x000000, 0.38);
pub const BACKDROP: Color = rgba(0x07090C, 0.62);

pub const TEXT: Color = rgba(0xEEF2F5, 1.0);
pub const TEXT_MUTED: Color = rgba(0x9AA6B2, 1.0);
pub const TEXT_FAINT: Color = rgba(0x9AA6B2, 0.6);

pub const ACCENT: Color = rgba(0x4DA3F0, 1.0);
pub const POSITIVE: Color = rgba(0x5CCB6B, 1.0);
pub const NEGATIVE: Color = rgba(0xF0625A, 1.0);
pub const WARNING: Color = rgba(0xF0C24D, 1.0);

pub const ZONE_RESIDENTIAL: Color = rgba(0x5CCB6B, 1.0);
pub const ZONE_COMMERCIAL: Color = rgba(0x4DA3F0, 1.0);
pub const ZONE_INDUSTRIAL: Color = rgba(0xF0C24D, 1.0);
/// De-zone has no zone colour; a neutral grey.
pub const ZONE_NONE: Color = rgba(0x9AA6B2, 1.0);

// Interactive surfaces.
pub const BUTTON: Color = rgba(0xFFFFFF, 0.0);
pub const BUTTON_HOVER: Color = rgba(0xFFFFFF, 0.08);
pub const BUTTON_PRESS: Color = rgba(0xFFFFFF, 0.04);
/// Alpha of an accent / zone colour used as an active background tint.
pub const ACTIVE_TINT: f32 = 0.20;
/// Alpha of an accent / zone colour used as an active border.
pub const ACTIVE_BORDER: f32 = 0.55;

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

// ---------------------------------------------------------------- shape & spacing

pub const RADIUS: f32 = 10.0;
pub const RADIUS_SM: f32 = 7.0;
pub const RADIUS_XS: f32 = 4.0;

/// Spacing scale (logical px).
pub const S1: f32 = 2.0;
pub const S2: f32 = 4.0;
pub const S3: f32 = 8.0;
pub const S4: f32 = 12.0;
pub const S5: f32 = 16.0;

/// Distance of floating panels from the window edge.
pub const EDGE: f32 = 12.0;
/// Height of the top bar chips.
pub const TOP_BAR_H: f32 = 48.0;
/// Where content below the top bar may start.
pub const BELOW_TOP_BAR: f32 = EDGE + TOP_BAR_H + S3;
/// Toolbar button size.
pub const TOOL_BUTTON: f32 = 46.0;

// ---------------------------------------------------------------- type

pub const FONT_CAPTION: f32 = 10.5;
pub const FONT_SMALL: f32 = 12.0;
pub const FONT_BODY: f32 = 13.0;
pub const FONT_LABEL: f32 = 14.0;
pub const FONT_VALUE: f32 = 15.0;
pub const FONT_TITLE: f32 = 17.0;
pub const FONT_DISPLAY: f32 = 26.0;

pub const ICON_SM: f32 = 14.0;
pub const ICON_MD: f32 = 18.0;
pub const ICON_LG: f32 = 21.0;

pub const W_REGULAR: u16 = 400;
pub const W_MEDIUM: u16 = 500;
pub const W_SEMIBOLD: u16 = 600;
pub const W_BOLD: u16 = 700;

/// Base resolution the layout is designed for; `UiScale` grows from it.
pub const DESIGN_SIZE: Vec2 = Vec2::new(1280.0, 720.0);

// ---------------------------------------------------------------- icons (Lucide font codepoints)

pub mod icon {
    pub const INSPECT: char = '\u{e1c3}'; // mouse-pointer-2
    pub const ROAD: char = '\u{e6d9}'; // road
    pub const ZONE: char = '\u{e528}'; // land-plot
    pub const BULLDOZE: char = '\u{e15d}'; // shovel
    pub const TRAFFIC: char = '\u{e1d5}'; // car
    pub const LAYERS: char = '\u{e529}'; // layers
    pub const HOUSE: char = '\u{e0f5}';
    pub const STORE: char = '\u{e3e4}';
    pub const FACTORY: char = '\u{e29f}';
    pub const ERASER: char = '\u{e28f}';
    pub const PAUSE: char = '\u{e12e}';
    pub const PLAY: char = '\u{e13c}';
    pub const WALLET: char = '\u{e204}';
    pub const USERS: char = '\u{e1a4}';
    pub const BRIEFCASE: char = '\u{e062}';
    pub const SUN: char = '\u{e178}';
    pub const MOON: char = '\u{e11e}';
    pub const SUNRISE: char = '\u{e179}';
    pub const SUNSET: char = '\u{e17a}';
    pub const TRENDING_UP: char = '\u{e191}';
    pub const TRENDING_DOWN: char = '\u{e190}';
    pub const CLOSE: char = '\u{e1b2}';
    pub const CHECK_CIRCLE: char = '\u{e226}';
    pub const ALERT: char = '\u{e193}';
    pub const ERROR: char = '\u{e084}';
    pub const INFO: char = '\u{e0f9}';
    pub const SAVE: char = '\u{e14d}';
    pub const LOAD: char = '\u{e247}';
    pub const MONITOR: char = '\u{e11d}';
    pub const QUIT: char = '\u{e10e}';
    pub const LIGHTBULB: char = '\u{e1c2}';
    pub const PERCENT: char = '\u{e132}';
    pub const MINUS: char = '\u{e11c}';
    pub const PLUS: char = '\u{e13d}';
    pub const LANDMARK: char = '\u{e23a}';
    pub const CHEVRON_DOWN: char = '\u{e06d}';
    pub const CHEVRON_LEFT: char = '\u{e06e}';
    pub const CIRCLE: char = '\u{e076}';
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
    pub const MAGNET: char = '\u{e2b5}';
    pub const UPGRADE: char = '\u{e420}'; // arrow-big-up-dash
    pub const CAR: char = '\u{e1d5}';
    pub const TRUCK: char = '\u{e194}';
    pub const USER: char = '\u{e19f}';
    pub const FOLLOW: char = '\u{e1db}'; // locate-fixed
    pub const MAP_PIN: char = '\u{e111}';
    pub const HEART: char = '\u{e0f2}';
    pub const BED: char = '\u{e2c1}';
    pub const SHOPPING: char = '\u{e15b}';
    pub const COFFEE: char = '\u{e096}';
    pub const CHART: char = '\u{e609}'; // chart-bar-stacked
    pub const BOOK: char = '\u{e05f}';
    pub const CLOCK: char = '\u{e087}';
    pub const ARROW_RIGHT: char = '\u{e049}';
    pub const BABY: char = '\u{e2ce}';
    pub const FROWN: char = '\u{e0db}';
    pub const RULER: char = '\u{e14b}';
    pub const LASSO: char = '\u{e1ce}';
    pub const EYE: char = '\u{e0ba}';
}

// ---------------------------------------------------------------- fonts

/// The interface's two fonts. Never use Bevy's default font.
#[derive(Resource, Clone)]
pub struct UiFonts {
    pub text: Handle<Font>,
    pub icons: Handle<Font>,
}

impl FromWorld for UiFonts {
    fn from_world(world: &mut World) -> Self {
        let assets = world.resource::<AssetServer>();
        UiFonts {
            text: assets.load("ui/fonts/InterVariable.ttf"),
            icons: assets.load("ui/fonts/lucide.ttf"),
        }
    }
}

impl UiFonts {
    /// Body text: Inter at `size` and `weight`.
    pub fn text(&self, size: f32, weight: u16) -> TextFont {
        TextFont {
            font: self.text.clone().into(),
            font_size: FontSize::Px(size),
            weight: FontWeight(weight),
            ..default()
        }
    }

    /// Numbers that change live: tabular figures so they do not jitter.
    pub fn numbers(&self, size: f32, weight: u16) -> TextFont {
        TextFont {
            font_features: FontFeatures::from([FontFeatureTag::TABULAR_FIGURES]),
            ..self.text(size, weight)
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
