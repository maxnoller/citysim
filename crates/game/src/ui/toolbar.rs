//! Bottom-centre toolbar (Inspect, Road, Zone, Bulldoze, Junctions | Traffic,
//! Zones overlays); the tool options bar (`toolopts.rs`) sits above it.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;
use citysim_sim::{Zone, balance};

use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ActiveMark, ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, label, layout, panel, spawn_button,
};
use crate::shared::{ActiveTool, OverlayMode, RoadMode};

pub struct ToolbarPlugin;

impl Plugin for ToolbarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_toolbar);
    }
}

fn spawn_toolbar(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Toolbar"),
            layout(Node {
                position_type: PositionType::Absolute,
                bottom: px(t::EDGE),
                left: px(0),
                right: px(0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(t::S3),
                ..default()
            }),
        ))
        .with_children(|root| {
            super::toolopts::spawn_opts(root, fonts);
            // Main bar.
            root.spawn(panel(Node {
                padding: UiRect::all(px(t::S2 + 1.0)),
                column_gap: px(t::S1 + 1.0),
                align_items: AlignItems::Center,
                ..default()
            }))
            .with_children(|bar| {
                tool_button(
                    bar,
                    fonts,
                    UiAction::Tool(ActiveTool::Inspect),
                    icon::INSPECT,
                    Tooltip {
                        title: "Inspect",
                        desc: "Hover buildings, roads and cars; click a home, workplace or car to meet its people.",
                        key: Some("I"),
                        price: None,
                    },
                );
                tool_button(
                    bar,
                    fonts,
                    UiAction::Tool(ActiveTool::Road),
                    icon::ROAD,
                    Tooltip {
                        title: "Road",
                        desc: "Straight, curved, continuous roads and street grids at any angle. Snaps to roads, angles and lots; Alt for free placement. R again: street / avenue.",
                        key: Some("R"),
                        price: Some(format!("${} per 8 m", balance::ROAD_COST_PER_CELL)),
                    },
                );
                tool_button(
                    bar,
                    fonts,
                    UiAction::ZoneTool,
                    icon::ZONE,
                    Tooltip {
                        title: "Zone",
                        desc: "Paint lots, zone a road side, fill an area or drag a rectangle along the lots. Buildings grow by demand. Tab switches mode.",
                        key: Some("Z"),
                        price: Some("Free".into()),
                    },
                );
                tool_button(
                    bar,
                    fonts,
                    UiAction::Tool(ActiveTool::Bulldoze),
                    icon::BULLDOZE,
                    Tooltip {
                        title: "Bulldoze",
                        desc: "Click a road (between junctions) or a building; drag along a road for a part, or drag an area. Tab switches mode.",
                        key: Some("B"),
                        price: None,
                    },
                );
                tool_button(
                    bar,
                    fonts,
                    UiAction::RoadMode(RoadMode::Junction),
                    icon::WAYPOINTS,
                    Tooltip {
                        title: "Junctions",
                        desc: "Hover a junction to see its traffic lights or priority rules; click to switch them.",
                        key: Some("J"),
                        price: None,
                    },
                );
                bar.spawn(super::widgets::vdivider(26.0));
                tool_button(
                    bar,
                    fonts,
                    UiAction::Overlay(OverlayMode::Traffic),
                    icon::TRAFFIC,
                    Tooltip {
                        title: "Traffic overlay",
                        desc: "Colour roads by congestion: green flows, red is jammed.",
                        key: Some("T"),
                        price: None,
                    },
                );
                tool_button(
                    bar,
                    fonts,
                    UiAction::Overlay(OverlayMode::Zones),
                    icon::LAYERS,
                    Tooltip {
                        title: "Zones overlay",
                        desc: "Show zoning in strong colours. O cycles overlays.",
                        key: Some("O"),
                        price: None,
                    },
                );
                bar.spawn(super::widgets::vdivider(26.0));
                tool_button(
                    bar,
                    fonts,
                    UiAction::CityPeople,
                    icon::USERS,
                    Tooltip {
                        title: "People",
                        desc: "Who lives in the city: ages, households, work, what everyone is doing, why people are unhappy.",
                        key: Some("P"),
                        price: None,
                    },
                );
            });
        });
}

fn tool_button(p: &mut ChildSpawnerCommands, fonts: &UiFonts, action: UiAction, glyph: char, tip: Tooltip) {
    let key = tip.key;
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Tool,
            accent: t::ACCENT,
        },
        Node {
            width: px(t::TOOL_BUTTON),
            height: px(t::TOOL_BUTTON),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Some(tip),
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, t::ICON_LG, t::TEXT),
                ButtonFg {
                    owner: id,
                    base: t::TEXT,
                },
            ));
            if let Some(key) = key {
                b.spawn((
                    label(fonts, key, 9.5, t::W_SEMIBOLD, t::TEXT_FAINT),
                    Node {
                        position_type: PositionType::Absolute,
                        top: px(2),
                        right: px(5),
                        ..default()
                    },
                ));
            }
            // Active indicator: a short accent bar at the bottom.
            b.spawn((
                ActiveMark { owner: id },
                layout(Node {
                    position_type: PositionType::Absolute,
                    bottom: px(3),
                    width: px(14),
                    height: px(3),
                    border_radius: BorderRadius::all(px(2)),
                    ..default()
                }),
                BackgroundColor(t::ACCENT),
                Visibility::Hidden,
            ));
        },
    );
}

/// One zone type button (used by the tool options bar).
pub fn spawn_zone_item(p: &mut ChildSpawnerCommands, fonts: &UiFonts, zone: Option<Zone>, desc: &'static str) {
    let color = t::zone_color(zone);
    spawn_button(
        p,
        UiButton {
            action: UiAction::Tool(ActiveTool::Zone(zone)),
            kind: ButtonKind::Tool,
            accent: color,
        },
        Node {
            height: px(36),
            padding: UiRect::new(px(t::S3 + 2.0), px(t::S4), px(0), px(0)),
            align_items: AlignItems::Center,
            column_gap: px(t::S3),
            ..default()
        },
        Some(Tooltip {
            title: super::widgets::zone_name(zone),
            desc,
            key: Some("Z"),
            price: Some("Free".into()),
        }),
        |b, id| {
            // Colour swatch with the zone icon.
            b.spawn(layout(Node {
                width: px(22),
                height: px(22),
                border_radius: BorderRadius::all(px(t::RADIUS_XS + 1.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            }))
            .insert(BackgroundColor(t::alpha(
                color,
                if zone.is_some() { 0.9 } else { 0.25 },
            )))
            .with_child(super::widgets::icon(
                fonts,
                super::widgets::zone_icon(zone),
                t::ICON_SM - 1.0,
                if zone.is_some() {
                    t::alpha(Color::BLACK, 0.75)
                } else {
                    t::TEXT
                },
            ));
            b.spawn((
                label(
                    fonts,
                    super::widgets::zone_name(zone),
                    t::FONT_BODY,
                    t::W_MEDIUM,
                    t::TEXT,
                ),
                ButtonFg {
                    owner: id,
                    base: t::TEXT,
                },
            ));
        },
    );
}
