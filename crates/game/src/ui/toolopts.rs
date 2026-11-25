//! Tool options bar above the toolbar: the current tool's modes, the road
//! type picker, the block size, undo, and the modifier keys.
//!
//! Road: Straight / Curve / Continuous / Grid / Upgrade / Junctions · Street /
//! Avenue · block size (grid) · snapping switches and the parallel distance.
//! Zone: types · Paint / Along road / Fill / Rectangle · depth (along road).
//! Bulldoze: Road / Area.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;

use super::context::{road_kind_name, road_price};
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::tools::{SnapRule, ToolOptions};
use super::widgets::{ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, label, layout, panel, spawn_button};
use citysim_sim::Zone;

use crate::shared::{ActiveTool, BulldozeMode, GameSet, RoadChoice, RoadMode, ToolModes, ZoneMode};

pub struct ToolOptsPlugin;

impl Plugin for ToolOptsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_opts.in_set(GameSet::Render));
    }
}

/// Which tool a section of the bar belongs to.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Section {
    Road,
    /// Street / avenue picker (drawing and upgrade modes).
    RoadKind,
    /// Block size stepper (grid mode).
    Block,
    /// Snapping switches (drawing modes).
    Snap,
    Zone,
    /// Frontage depth (frontage zoning).
    ZoneDepth,
    Bulldoze,
    /// Undo (road, zone and bulldozer tools).
    Undo,
}

#[derive(Component)]
struct OptsRoot;

#[derive(Component)]
struct BlockLabel;

#[derive(Component)]
struct ParallelLabel;

#[derive(Component)]
struct PriceLabel(RoadChoice);

fn tip(title: &'static str, desc: &'static str, key: Option<&'static str>) -> Tooltip {
    Tooltip {
        title,
        desc,
        key,
        price: None,
    }
}

/// Spawn the bar as a child of the toolbar column (above the zone flyout).
pub fn spawn_opts(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    p.spawn((
        OptsRoot,
        panel(Node {
            padding: UiRect::all(px(t::S2 + 1.0)),
            column_gap: px(t::S1 + 1.0),
            row_gap: px(t::S1 + 1.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_wrap: FlexWrap::Wrap,
            max_width: px(660),
            display: Display::None,
            ..default()
        }),
    ))
    .with_children(|bar| {
        // Road modes.
        section(bar, Section::Road, |s| {
            for (mode, glyph, title, desc) in [
                (
                    RoadMode::Straight,
                    icon::ROAD,
                    "Straight road",
                    "Click (or drag) from point to point; pieces chain into one road. Enter or double-click builds.",
                ),
                (
                    RoadMode::Curve,
                    icon::SPLINE,
                    "Curved road",
                    "Three clicks: the start, the corner where the two directions meet, the end.",
                ),
                (
                    RoadMode::Continuous,
                    icon::LASSO,
                    "Continuous road",
                    "Click along the way: every piece leaves the last one smoothly.",
                ),
                (
                    RoadMode::Grid,
                    icon::GRID,
                    "Street grid",
                    "Three clicks: a corner, the width, the length. Any angle, blocks of the chosen size.",
                ),
                (
                    RoadMode::Upgrade,
                    icon::UPGRADE,
                    "Change road type",
                    "Click a road for the stretch between junctions, or drag along it, to make it the chosen type.",
                ),
                (
                    RoadMode::Junction,
                    icon::WAYPOINTS,
                    "Junctions",
                    "Hover a junction to see how it is controlled; click to switch it.",
                ),
            ] {
                let key = if mode == RoadMode::Junction { "J" } else { "Tab" };
                mode_button(s, fonts, UiAction::RoadMode(mode), glyph, "", tip(title, desc, Some(key)));
            }
        });
        section(bar, Section::RoadKind, |s| {
            s.spawn(super::widgets::vdivider(26.0));
            for (kind, desc) in [
                (RoadChoice::Street, "One lane each way. Cheap; fine for quiet neighbourhoods."),
                (
                    RoadChoice::Avenue,
                    "Two lanes each way, faster. Needs gentler bends (radius 24 m).",
                ),
            ] {
                kind_button(s, fonts, kind, desc);
            }
        });
        section(bar, Section::Block, |s| {
            s.spawn(super::widgets::vdivider(26.0));
            stepper(s, fonts, UiAction::BlockSize(-1), icon::MINUS, "Smaller blocks", "[");
            s.spawn((
                label(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT),
                BlockLabel,
                Node {
                    min_width: px(74),
                    margin: UiRect::horizontal(px(t::S2)),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
            ));
            stepper(s, fonts, UiAction::BlockSize(1), icon::PLUS, "Bigger blocks", "]");
        });
        section(bar, Section::Snap, |s| {
            s.spawn(super::widgets::vdivider(26.0));
            s.spawn(super::widgets::icon(fonts, icon::MAGNET, t::ICON_SM, t::TEXT_FAINT));
            for (rule, glyph, title, desc) in [
                (SnapRule::Nodes, icon::CIRCLE, "Snap to road ends", "Ends and junctions of existing roads."),
                (SnapRule::Roads, '\u{e28d}', "Snap onto roads", "A point on a road makes a T junction."),
                (
                    SnapRule::Angles,
                    '\u{e0a2}',
                    "Snap to angles",
                    "Straight on and at right angles to the road you start from, and every 15°.",
                ),
                (SnapRule::Lengths, icon::RULER, "Whole cells", "Lengths in whole cells of 8 m."),
                (
                    SnapRule::Lattice,
                    icon::SCAN,
                    "Line up with lots",
                    "The lattice of nearby zoning blocks, so lots line up across the street.",
                ),
                (
                    SnapRule::Parallel,
                    icon::PAUSE,
                    "Parallel roads",
                    "Parallel to a nearby road at the set distance.",
                ),
                (
                    SnapRule::Extensions,
                    icon::ARROW_RIGHT,
                    "Road extensions",
                    "The lines of dead ends, continued.",
                ),
            ] {
                toggle_button(s, fonts, UiAction::Snap(rule), glyph, tip(title, desc, Some("Alt: all off")));
            }
            stepper(s, fonts, UiAction::ParallelStep(-1), icon::MINUS, "Closer parallel roads", "");
            s.spawn((
                label(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT),
                ParallelLabel,
                Node {
                    min_width: px(40),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
            ));
            stepper(s, fonts, UiAction::ParallelStep(1), icon::PLUS, "Wider parallel roads", "");
        });
        // Zone types and modes.
        section(bar, Section::Zone, |s| {
            for (zone, desc) in [
                (
                    Some(Zone::Residential),
                    "Homes. Residents need jobs and a road to drive on.",
                ),
                (
                    Some(Zone::Commercial),
                    "Shops and offices. Provide jobs, need customers.",
                ),
                (
                    Some(Zone::Industrial),
                    "Factories. Many jobs; keep them away from homes.",
                ),
                (None, "Remove zoning from empty cells."),
            ] {
                super::toolbar::spawn_zone_item(s, fonts, zone, desc);
            }
            s.spawn(super::widgets::vdivider(26.0));
            for (mode, glyph, title, desc) in [
                (ZoneMode::Paint, icon::BRUSH, "Paint", "Drag over the lots to zone them cell by cell."),
                (
                    ZoneMode::Frontage,
                    icon::ROUTE,
                    "Along a road",
                    "Click one side of a road to zone it between two junctions, or drag along a road.",
                ),
                (
                    ZoneMode::Fill,
                    icon::PAINT_BUCKET,
                    "Fill",
                    "Click to zone all the connected land around the cursor.",
                ),
                (
                    ZoneMode::Marquee,
                    icon::SCAN,
                    "Rectangle",
                    "Drag a rectangle; it follows the lots of the block you start on.",
                ),
            ] {
                mode_button(
                    s,
                    fonts,
                    UiAction::ZoneMode(mode),
                    glyph,
                    "",
                    Tooltip {
                        title,
                        desc,
                        key: Some("Tab"),
                        price: Some("Free".into()),
                    },
                );
            }
        });
        section(bar, Section::ZoneDepth, |s| {
            s.spawn(super::widgets::vdivider(26.0));
            for d in 1..=citysim_sim::balance::BLOCK_DEPTH {
                let (text, desc) = if d == 1 {
                    ("1 row", "Only the row of lots along the road.")
                } else {
                    ("2 rows", "The whole depth of the block.")
                };
                mode_button(s, fonts, UiAction::ZoneDepth(d), icon::RULER, text, tip("Depth", desc, None));
            }
        });
        section(bar, Section::Bulldoze, |s| {
            mode_button(
                s,
                fonts,
                UiAction::BulldozeMode(BulldozeMode::Road),
                icon::ROUTE,
                "Road",
                tip(
                    "Remove a road or building",
                    "Click a road to remove it between two junctions, drag along it to remove part; click a building.",
                    Some("Tab"),
                ),
            );
            mode_button(
                s,
                fonts,
                UiAction::BulldozeMode(BulldozeMode::Area),
                icon::SCAN,
                "Area",
                tip(
                    "Bulldoze an area",
                    "Drag a rectangle over roads and buildings to remove them.",
                    Some("Tab"),
                ),
            );
        });
        section(bar, Section::Undo, |s| {
            s.spawn(super::widgets::vdivider(26.0));
            spawn_button(
                s,
                UiButton {
                    action: UiAction::Undo,
                    kind: ButtonKind::Ghost,
                    accent: t::ACCENT,
                },
                Node {
                    height: px(34),
                    padding: UiRect::horizontal(px(t::S3)),
                    column_gap: px(t::S2 + 1.0),
                    align_items: AlignItems::Center,
                    ..default()
                },
                Some(Tooltip {
                    title: "Undo",
                    desc: "Take back the last road, zoning or demolition. Nothing is refunded; demolished buildings don't come back.",
                    key: Some("Ctrl+Z"),
                    price: None,
                }),
                |b, id| {
                    b.spawn((
                        super::widgets::icon(fonts, icon::UNDO, t::ICON_SM + 1.0, t::TEXT_MUTED),
                        ButtonFg {
                            owner: id,
                            base: t::TEXT_MUTED,
                        },
                    ));
                },
            );
        });
    });
}

/// A small square on / off switch with an icon.
fn toggle_button(p: &mut ChildSpawnerCommands, fonts: &UiFonts, action: UiAction, glyph: char, tip: Tooltip) {
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Segment,
            accent: t::ACCENT,
        },
        Node {
            width: px(28),
            height: px(28),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Some(tip),
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, t::ICON_SM, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
            ));
        },
    );
}

fn section(p: &mut ChildSpawnerCommands, s: Section, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn((
        s,
        layout(Node {
            column_gap: px(t::S1 + 1.0),
            align_items: AlignItems::Center,
            display: Display::None,
            ..default()
        }),
    ))
    .with_children(f);
}

/// A mode button: icon and label, or icon only when `text` is empty (the
/// tooltip names it).
fn mode_button(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    glyph: char,
    text: &'static str,
    tip: Tooltip,
) {
    let node = if text.is_empty() {
        Node {
            width: px(36),
            height: px(34),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        }
    } else {
        Node {
            height: px(34),
            padding: UiRect::new(px(t::S3 + 1.0), px(t::S4 - 1.0), px(0), px(0)),
            column_gap: px(t::S2 + 2.0),
            align_items: AlignItems::Center,
            ..default()
        }
    };
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Segment,
            accent: t::ACCENT,
        },
        node,
        Some(tip),
        |b, id| {
            let size = if text.is_empty() {
                t::ICON_MD - 1.0
            } else {
                t::ICON_SM + 1.0
            };
            b.spawn((
                super::widgets::icon(fonts, glyph, size, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
            ));
            if !text.is_empty() {
                b.spawn((
                    label(fonts, text, t::FONT_BODY, t::W_MEDIUM, t::TEXT),
                    ButtonFg {
                        owner: id,
                        base: t::TEXT,
                    },
                ));
            }
        },
    );
}

fn kind_button(p: &mut ChildSpawnerCommands, fonts: &UiFonts, kind: RoadChoice, desc: &'static str) {
    spawn_button(
        p,
        UiButton {
            action: UiAction::RoadKind(kind),
            kind: ButtonKind::Segment,
            accent: t::ACCENT,
        },
        Node {
            height: px(34),
            padding: UiRect::new(px(t::S3 + 1.0), px(t::S3 + 2.0), px(0), px(0)),
            column_gap: px(t::S2 + 2.0),
            align_items: AlignItems::Center,
            ..default()
        },
        Some(Tooltip {
            title: road_kind_name(kind),
            desc,
            key: Some("R"),
            price: Some(format!("{} per cell", format::money(road_price(kind)))),
        }),
        |b, id| {
            // Lane glyph: one or two bars.
            b.spawn(layout(Node {
                column_gap: px(2),
                align_items: AlignItems::Center,
                ..default()
            }))
            .with_children(|g| {
                let n = if kind == RoadChoice::Avenue { 2 } else { 1 };
                for _ in 0..n {
                    g.spawn((
                        layout(Node {
                            width: px(4),
                            height: px(14),
                            border_radius: BorderRadius::all(px(1.5)),
                            ..default()
                        }),
                        BackgroundColor(t::TEXT_MUTED),
                    ));
                }
            });
            b.spawn((
                label(fonts, road_kind_name(kind), t::FONT_BODY, t::W_MEDIUM, t::TEXT),
                ButtonFg {
                    owner: id,
                    base: t::TEXT,
                },
            ));
            b.spawn((
                super::widgets::number(fonts, "", t::FONT_CAPTION + 0.5, t::W_MEDIUM, t::TEXT_FAINT),
                PriceLabel(kind),
            ));
        },
    );
}

fn stepper(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    glyph: char,
    title: &'static str,
    key: &'static str,
) {
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Filled,
            accent: t::ACCENT,
        },
        Node {
            width: px(26),
            height: px(26),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Some(Tooltip {
            title,
            desc: "",
            key: (!key.is_empty()).then_some(key),
            price: None,
        }),
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, t::ICON_SM, t::TEXT),
                ButtonFg {
                    owner: id,
                    base: t::TEXT,
                },
            ));
        },
    );
}

#[allow(clippy::type_complexity)]
fn update_opts(
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    opts: Res<ToolOptions>,
    mut root: Query<&mut Node, (With<OptsRoot>, Without<Section>)>,
    mut sections: Query<(&Section, &mut Node), Without<OptsRoot>>,
    mut texts: ParamSet<(
        Query<&mut Text, With<BlockLabel>>,
        Query<(&mut Text, &PriceLabel)>,
        Query<&mut Text, With<ParallelLabel>>,
    )>,
    mut first: Local<bool>,
) {
    if !tool.is_changed() && !modes.is_changed() && !opts.is_changed() && *first {
        return;
    }
    *first = true;
    let road = *tool == ActiveTool::Road;
    let zone = matches!(*tool, ActiveTool::Zone(_));
    let show = |s: Section| match s {
        Section::Road => road,
        Section::RoadKind => road && modes.road != RoadMode::Junction,
        Section::Block => road && modes.road == RoadMode::Grid,
        Section::Snap => road && modes.road.draws(),
        Section::Zone => zone,
        Section::ZoneDepth => zone && modes.zone == ZoneMode::Frontage,
        Section::Bulldoze => *tool == ActiveTool::Bulldoze,
        Section::Undo => *tool != ActiveTool::Inspect,
    };
    for (s, mut node) in &mut sections {
        let d = if show(*s) { Display::Flex } else { Display::None };
        if node.display != d {
            node.display = d;
        }
    }
    let d = if *tool == ActiveTool::Inspect {
        Display::None
    } else {
        Display::Flex
    };
    for mut n in &mut root {
        if n.display != d {
            n.display = d;
        }
    }
    let block = format!("Blocks of {}", opts.block_cells());
    for mut tx in &mut texts.p0() {
        if tx.0 != block {
            tx.0 = block.clone();
        }
    }
    for (mut tx, p) in &mut texts.p1() {
        let s = format::money(road_price(p.0));
        if tx.0 != s {
            tx.0 = s;
        }
    }
    let par = format!("{} m", opts.parallel_m() as i64);
    for mut tx in &mut texts.p2() {
        if tx.0 != par {
            tx.0 = par.clone();
        }
    }
}
