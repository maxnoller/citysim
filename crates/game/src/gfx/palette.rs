//! The one colour palette every 3D surface takes its colour from, so the
//! whole diorama stays consistent. sRGB hex values; meshes store them as
//! linear vertex colours via [`lin`].

use bevy::prelude::*;

const fn c(hex: u32) -> Color {
    Color::srgb_u32(hex)
}

/// Linear RGBA for a vertex-colour attribute.
pub fn lin(color: Color) -> [f32; 4] {
    color.to_linear().to_f32_array()
}

/// Multiply a colour's brightness (in linear space) by `k`.
pub fn shade(color: Color, k: f32) -> Color {
    let l = color.to_linear();
    Color::linear_rgba(l.red * k, l.green * k, l.blue * k, l.alpha)
}

/// Mix two colours in linear space (`t = 0` -> `a`).
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (a, b) = (a.to_linear(), b.to_linear());
    Color::linear_rgba(
        a.red + (b.red - a.red) * t,
        a.green + (b.green - a.green) * t,
        a.blue + (b.blue - a.blue) * t,
        a.alpha + (b.alpha - a.alpha) * t,
    )
}

// ------------------------------------------------------------ ground
pub const GRASS: Color = c(0x7FA65A);
pub const GRASS_LIGHT: Color = c(0x8DB365);
pub const GRASS_DARK: Color = c(0x6F9550);
pub const EARTH: Color = c(0xB7A77F);
/// Countryside beyond the playable map: a touch drier and duller.
pub const GRASS_OUTSIDE: Color = c(0x86A260);

// ------------------------------------------------------------ roads
pub const ASPHALT: Color = c(0x3F4247);
pub const ASPHALT_HIGHWAY: Color = c(0x36393E);
pub const MARKING: Color = c(0xE8E4D8);
pub const MARKING_YELLOW: Color = c(0xE8C25A);
pub const SIDEWALK: Color = c(0xB9B4A8);
pub const KERB: Color = c(0x9A958A);
pub const GUARDRAIL: Color = c(0xC9CCCF);
pub const GRAVEL: Color = c(0x9C9584);
pub const LAMP: Color = c(0x5E656B);
pub const LAMP_GLASS: Color = c(0xF4EED8);
pub const SIGN_GREEN: Color = c(0x2E7D4F);
pub const MEDIAN_GRASS: Color = c(0x6E9A4E);

// ------------------------------------------------------------ houses
pub const HOUSE_WALLS: [Color; 6] = [
    c(0xF2E6CF), // cream
    c(0xF7F1E5), // warm white
    c(0xEBD9A0), // pale yellow
    c(0xC9D6B8), // sage
    c(0xB8CAD6), // dusty blue
    c(0xE7C4B5), // blush
];
pub const HOUSE_ROOFS: [Color; 6] = [
    c(0xB5553C), // terracotta
    c(0x5C6670), // slate
    c(0x7A5540), // brown
    c(0x8E3B34), // dark red
    c(0x474C54), // charcoal
    c(0x9C6A48), // clay
];
/// Facing bricks: red, orange, brown, buff, grey-brown.
pub const BRICKS: [Color; 5] = [c(0x9E5A48), c(0xAD7259), c(0x8A5E4E), c(0xC9A886), c(0x957D6F)];
/// Front doors.
pub const DOORS: [Color; 7] = [
    c(0x5A4030), // stained wood
    c(0x2F4F6F), // navy
    c(0x7A2E2A), // oxblood
    c(0x2E5E4A), // green
    c(0xEDE7D8), // white
    c(0x2A2D31), // black
    c(0xC9973E), // mustard
];
/// Window shutters.
pub const SHUTTERS: [Color; 5] = [c(0x4F6B4A), c(0x3E5F7A), c(0x7A3530), c(0x5C6670), c(0xE9E1CC)];
/// Stone for sills, lintels, cornices and copings.
pub const STONE: Color = c(0xD9D2C2);
/// Dark metal: gutters, railings, window bars on modern blocks.
pub const METAL_DARK: Color = c(0x3B4046);
/// Profiled cladding on works and warehouses.
pub const CLADDING: [Color; 7] = [
    c(0xC9C4BA),
    c(0x9DB0BC),
    c(0xA9B9A4),
    c(0xD8CBA6),
    c(0xB8C0C6),
    c(0xC2A58A),
    c(0x7F96A8),
];
/// Shipping containers.
pub const CONTAINERS: [Color; 6] = [
    c(0xB8432F),
    c(0x2F6FB5),
    c(0x3F8A5A),
    c(0xD9A441),
    c(0x8A8F94),
    c(0x7A3B5A),
];
pub const WINDOW: Color = c(0x6F8FA6);
pub const WINDOW_FRAME: Color = c(0xF7F1E5);
pub const DOOR: Color = c(0x5A4030);
pub const HEDGE: Color = c(0x4A7F42);
pub const FENCE: Color = c(0xEFE9DC);
pub const PATH: Color = c(0xCFC6B0);

// ------------------------------------------------------------ shops / offices
pub const SHOP_WALLS: [Color; 2] = [c(0xE9EEF2), c(0xF3EFE6)];
pub const GLASS: Color = c(0x7FB2D0);
pub const OFFICE_WALLS: [Color; 6] = [
    c(0xE9EEF2),
    c(0xF3EFE6),
    c(0xD9C7A7), // warm stone
    c(0xC98B6B), // brick
    c(0xA9ADB0), // concrete
    c(0xE3CFA6), // sandstone
];
pub const ACCENTS: [Color; 4] = [c(0xE0564A), c(0x2F8FCB), c(0xF2B33D), c(0x3FA66B)];
pub const ROOF_FLAT: Color = c(0xA9A9A2);
pub const ROOF_DETAIL: Color = c(0x8F959A);
pub const PAVING: Color = c(0xC4BEB0);

// ------------------------------------------------------------ industry
pub const INDUSTRY_ROOF: Color = c(0x7D868C);
pub const INDUSTRY_STRIPE: Color = c(0xD98F3E);
pub const BRICK: Color = c(0x8E3B34);
pub const TANK: Color = c(0xAEB6BB);
pub const CRATE: Color = c(0xA47B4F);
pub const YARD_CONCRETE: Color = c(0xA8A397);

// ------------------------------------------------------------ trees
pub const CANOPIES: [Color; 3] = [c(0x4E8B4A), c(0x5E9C52), c(0x3F7A46)];
pub const TRUNK: Color = c(0x6B4E36);

// ------------------------------------------------------------ sky
/// Horizon haze (also the fog colour): pale and slightly warm.
pub const SKY_HORIZON: Color = c(0xDCE5E6);
/// Clear blue a little above the horizon.
pub const SKY_MID: Color = c(0x6FA9EA);
/// Deeper blue overhead.
pub const SKY_ZENITH: Color = c(0x3B7BD8);
/// Warm glow towards the sun near the horizon.
pub const SKY_GLOW: Color = c(0xF6E7C8);
pub const SUN: Color = c(0xFFF1DC);

// ------------------------------------------------------------ vehicles
/// Car paints: mostly whites, silvers, greys and black like a real car park,
/// with a few colours.
pub const CAR_BODIES: [Color; 12] = [
    c(0xF2F1EC), // white
    c(0xBFC4C8), // silver
    c(0x5B6269), // grey
    c(0x2A2D31), // black
    c(0xD0483D), // red
    c(0x2F6FB5), // blue
    c(0x2F5D49), // dark green
    c(0xD8C9A6), // beige
    c(0xE6A83E), // amber
    c(0x8E3B34), // maroon
    c(0x23365A), // dark blue
    c(0x7A5C44), // bronze
];
pub const CAR_GLASS: Color = c(0x3E5566);
/// Delivery trucks: (cab, cargo box, livery stripe).
pub const TRUCK_LIVERIES: [(Color, Color, Color); 6] = [
    (c(0xF7F1E5), c(0xF7F1E5), c(0xE0564A)),
    (c(0x2F8FCB), c(0xE9EEF2), c(0x2F8FCB)),
    (c(0xE0564A), c(0xF3EFE6), c(0xE0564A)),
    (c(0x3FA66B), c(0xF7F1E5), c(0xF2B33D)),
    (c(0xF2B33D), c(0xEBD9A0), c(0x3F4A55)),
    (c(0x3F4A55), c(0xE9EEF2), c(0x3FA66B)),
];
pub const TYRE: Color = c(0x26282B);

// ------------------------------------------------------------ overlays & tools
pub const ZONE_RES: Color = c(0x5CCB6B);
pub const ZONE_COM: Color = c(0x4DA3F0);
pub const ZONE_IND: Color = c(0xF0C24D);
pub const TOOL_OK: Color = c(0xBFE3FF);
pub const TOOL_BAD: Color = c(0xF04A3C);
pub const TOOL_BULLDOZE: Color = c(0xF08A3C);
pub const TRAFFIC_FREE: Color = c(0x4CC66A);
pub const TRAFFIC_BUSY: Color = c(0xF2C23D);
pub const TRAFFIC_JAM: Color = c(0xE0453A);
