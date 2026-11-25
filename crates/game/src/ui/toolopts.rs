//! Tool options bar above the toolbar: the current tool's modes, the road
//! type picker, the block size, undo, and the modifier keys.
//!
//! Road: Path / Block / Junctions · Street / Avenue · block size.
//! Zone: Rectangle / Along road / Fill block. Bulldoze: Rectangle / Road.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;

use super::context::{road_kind_name, road_price};
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::tools::ToolState;
use super::widgets::{ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, label, layout, panel, spawn_button};
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
    /// Street / avenue picker (path and block modes).
    RoadKind,
    /// Block size stepper (block mode).
    Block,
    Zone,
    Bulldoze,
    /// Undo (road and zone tools).
    Undo,
}

#[derive(Component)]
struct OptsRoot;

#[derive(Component)]
struct BlockLabel;

#[derive(Component)]
struct KeysLabel;

#[derive(Component)]
struct PriceLabel(RoadChoice);

/// Spawn the bar as a child of the toolbar column (above the zone flyout).
pub fn spawn_opts(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    p.spawn((
        OptsRoot,
        panel(Node {
            padding: UiRect::all(px(t::S2 + 1.0)),
            column_gap: px(t::S1 + 1.0),
            align_items: AlignItems::Center,
            display: Display::None,
            ..default()
        }),
    ))
    .with_children(|bar| {
        // Road modes.
        section(bar, Section::Road, |s| {
            mode_button(
                s,
                fonts,
                UiAction::RoadMode(RoadMode::Path),
                icon::SPLINE,
                "Path",
                Tooltip {
                    title: "Draw a road",
                    desc: "Click corners; double-click, Enter or right-click builds the whole path. Drag for one straight road.",
                    key: Some("R"),
                    price: None,
                },
            );
            mode_button(
                s,
                fonts,
                UiAction::RoadMode(RoadMode::Block),
                icon::GRID,
                "Block",
                Tooltip {
                    title: "Block of streets",
                    desc: "Drag a rectangle for a street loop, split into blocks of the chosen size.",
                    key: Some("Tab"),
                    price: None,
                },
            );
            mode_button(
                s,
                fonts,
                UiAction::RoadMode(RoadMode::Junction),
                icon::WAYPOINTS,
                "Junctions",
                Tooltip {
                    title: "Junctions",
                    desc: "Hover a junction to see how it is controlled; click to switch it.",
                    key: Some("J"),
                    price: None,
                },
            );
        });
        section(bar, Section::RoadKind, |s| {
            s.spawn(super::widgets::vdivider(26.0));
            for (kind, desc) in [
                (RoadChoice::Street, "One lane each way. Cheap; fine for quiet neighbourhoods."),
                (
                    RoadChoice::Avenue,
                    "Two lanes each way, faster. Draw it over a street to upgrade it.",
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
                    ..default()
                },
                TextLayout::justify(Justify::Center),
            ));
            stepper(s, fonts, UiAction::BlockSize(1), icon::PLUS, "Bigger blocks", "]");
        });
        // Zone modes.
        section(bar, Section::Zone, |s| {
            mode_button(
                s,
                fonts,
                UiAction::ZoneMode(ZoneMode::Rect),
                icon::SCAN,
                "Rectangle",
                Tooltip {
                    title: "Zone a rectangle",
                    desc: "Drag a rectangle; only land near a road is zoned.",
                    key: Some("Tab"),
                    price: Some("Free".into()),
                },
            );
            mode_button(
                s,
                fonts,
                UiAction::ZoneMode(ZoneMode::Frontage),
                icon::ROUTE,
                "Along road",
                Tooltip {
                    title: "Zone along a road",
                    desc: "Click one side of a road to zone it between two junctions, or drag along a road.",
                    key: Some("Tab"),
                    price: Some("Free".into()),
                },
            );
            mode_button(
                s,
                fonts,
                UiAction::ZoneMode(ZoneMode::Fill),
                icon::PAINT_BUCKET,
                "Fill block",
                Tooltip {
                    title: "Fill a block",
                    desc: "Click inside a block enclosed by roads to zone all of it.",
                    key: Some("Tab"),
                    price: Some("Free".into()),
                },
            );
        });
        section(bar, Section::Bulldoze, |s| {
            mode_button(
                s,
                fonts,
                UiAction::BulldozeMode(BulldozeMode::Rect),
                icon::SCAN,
                "Rectangle",
                Tooltip {
                    title: "Bulldoze a rectangle",
                    desc: "Drag over roads and buildings to remove them.",
                    key: Some("Tab"),
                    price: None,
                },
            );
            mode_button(
                s,
                fonts,
                UiAction::BulldozeMode(BulldozeMode::Road),
                icon::ROUTE,
                "Road",
                Tooltip {
                    title: "Remove a road",
                    desc: "Click a road to remove it between two junctions, or drag along it to remove part.",
                    key: Some("Tab"),
                    price: None,
                },
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
                    desc: "Take back the last road or zoning. Removing a road costs the demolition fee; nothing is refunded.",
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
        bar.spawn((
            label(fonts, "", t::FONT_CAPTION + 0.5, t::W_REGULAR, t::TEXT_FAINT),
            KeysLabel,
            Node {
                margin: UiRect::new(px(t::S3), px(t::S3), px(0), px(0)),
                ..default()
            },
        ));
    });
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

fn mode_button(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    glyph: char,
    text: &'static str,
    tip: Tooltip,
) {
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Segment,
            accent: t::ACCENT,
        },
        Node {
            height: px(34),
            padding: UiRect::new(px(t::S3 + 1.0), px(t::S4 - 1.0), px(0), px(0)),
            column_gap: px(t::S2 + 2.0),
            align_items: AlignItems::Center,
            ..default()
        },
        Some(tip),
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, t::ICON_SM + 1.0, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
            ));
            b.spawn((
                label(fonts, text, t::FONT_BODY, t::W_MEDIUM, t::TEXT),
                ButtonFg {
                    owner: id,
                    base: t::TEXT,
                },
            ));
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
            desc: "Cells between the streets of the block tool.",
            key: Some(key),
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

/// Modifier keys line for the current tool and mode.
pub fn keys_text(tool: ActiveTool, modes: ToolModes) -> &'static str {
    match tool {
        ActiveTool::Road => match modes.road {
            RoadMode::Path => "Alt: no snap · Backspace: last corner · Ctrl+Z: undo",
            RoadMode::Block => "Alt: no snap · [ ]: block size · Ctrl+Z: undo",
            RoadMode::Junction => "Click a junction to switch its control",
        },
        ActiveTool::Zone(_) => "Z: zone type · Tab: mode · Ctrl+Z: undo",
        ActiveTool::Bulldoze => "Tab: mode · Right-click: cancel",
        ActiveTool::Inspect => "",
    }
}

#[allow(clippy::type_complexity)]
fn update_opts(
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    state: Res<ToolState>,
    mut root: Query<&mut Node, (With<OptsRoot>, Without<Section>)>,
    mut sections: Query<(&Section, &mut Node), Without<OptsRoot>>,
    mut texts: ParamSet<(
        Query<&mut Text, With<BlockLabel>>,
        Query<&mut Text, With<KeysLabel>>,
        Query<(&mut Text, &PriceLabel)>,
    )>,
    mut first: Local<bool>,
) {
    if !tool.is_changed() && !modes.is_changed() && !state.is_changed() && *first {
        return;
    }
    *first = true;
    let road = *tool == ActiveTool::Road;
    let show = |s: Section| match s {
        Section::Road => road,
        Section::RoadKind => road && modes.road != RoadMode::Junction,
        Section::Block => road && modes.road == RoadMode::Block,
        Section::Zone => matches!(*tool, ActiveTool::Zone(_)),
        Section::Bulldoze => *tool == ActiveTool::Bulldoze,
        Section::Undo => road || matches!(*tool, ActiveTool::Zone(_)),
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
    let block = match state.block_spacing() {
        Some(s) => format!("Blocks of {s}"),
        None => "Outline".into(),
    };
    for mut tx in &mut texts.p0() {
        if tx.0 != block {
            tx.0 = block.clone();
        }
    }
    let keys = keys_text(*tool, *modes);
    for mut tx in &mut texts.p1() {
        if tx.0 != keys {
            tx.0 = keys.into();
        }
    }
    for (mut tx, p) in &mut texts.p2() {
        let s = format::money(road_price(p.0));
        if tx.0 != s {
            tx.0 = s;
        }
    }
}
