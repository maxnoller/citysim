//! The tray: a drawer standing on the bottom bar for the active tool or
//! info slot. A title block, picture slots for what can be built (the
//! selected one framed with corner brackets, a locked one hatched), joined
//! switches for the options, lit switches for the snapping rules, and the
//! live figures as large numerals.
//!
//! Pages: roads, zones, clear (bulldozer), junctions, maps, statistics;
//! the budget (`budget.rs`) and the city's people (`panel.rs`) fill their
//! own pages. One page shows at a time; the drawer rises from behind the
//! bar when it opens and sinks when it closes.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;
use citysim_sim::Zone;

use super::context::{Band, JAM_CRAWL_MPS, junction_card, pct, road_price};
use super::format;
use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::readout::{self, Figure, Tone};
use super::theme::{self as t, UiFonts, icon};
use super::tools::{SnapRule, ToolOptions, ToolState};
use super::widgets::{
    ActiveFill, ButtonFg, ButtonKind, Disabled, Tooltip, UiAction, UiButton, approach, caps, figure, label, layout,
    paragraph, plate_node, plate_shadow, spawn_brackets, spawn_button, spawn_segments, spawn_stepper, spawn_toggle,
    title_tick,
};
use super::{InfoTray, UiState};
use crate::shared::{
    ActiveTool, BulldozeMode, CONGESTION_BUSY, CONGESTION_JAM, GameSet, OverlayMode, RoadChoice, RoadMode, SimWorld,
    ToolModes, WorldHighlight, WorldTarget, ZoneMode,
};

pub struct TrayPlugin;

impl Plugin for TrayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_tray).add_systems(
            Update,
            (
                show_page,
                slide_tray,
                update_figures,
                update_conditions,
                update_junction,
                update_maps,
                update_stats,
            )
                .in_set(GameSet::Render),
        );
    }
}

/// Which page of the tray.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Road,
    Zone,
    Clear,
    Junction,
    Maps,
    Stats,
    Budget,
    People,
}

/// The page the tray should show now.
pub fn current_page(tool: ActiveTool, modes: ToolModes, ui: &UiState) -> Option<Page> {
    if ui.menu_open {
        return None;
    }
    match tool {
        ActiveTool::Road if modes.road == RoadMode::Junction => Some(Page::Junction),
        ActiveTool::Road => Some(Page::Road),
        ActiveTool::Zone(_) => Some(Page::Zone),
        ActiveTool::Bulldoze => Some(Page::Clear),
        ActiveTool::Inspect => ui.tray.map(|t| match t {
            InfoTray::Budget => Page::Budget,
            InfoTray::People => Page::People,
            InfoTray::Maps => Page::Maps,
            InfoTray::Stats => Page::Stats,
        }),
    }
}

#[derive(Component)]
pub struct TrayRoot {
    /// 0 down behind the bar .. 1 up.
    t: f32,
    shown: Option<Page>,
}

#[derive(Component)]
struct TrayPlate;

/// A big figure of a page: `(page, index, is_caption)`.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct FigText(Page, usize, bool);

/// Shown only while a condition holds.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ShowWhen {
    GridMode,
    NotGridMode,
    ParallelOn,
    TrafficMap,
    ZonesMap,
    NoMap,
    JunctionHovered,
    NoJunctionHovered,
}

/// Disabled while a condition does not hold.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum EnableWhen {
    Frontage,
    CanUndo,
    DrawsRoads,
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum TrayLabel {
    Block,
    Parallel,
    RoadPrice(RoadChoice),
    ZoneDemand(Zone),
    ZoneHint,
    ClearHint,
    JunctionTitle,
    /// Label and value of junction row `i`.
    JunctionLabel(usize),
    JunctionValue(usize),
    JunctionHint,
    MapFigure(usize, bool),
    ZoneCount(usize),
    Stat(usize, bool),
}

#[derive(Component, Clone, Copy)]
struct JunctionRow(usize);

const JUNCTION_ROWS: usize = 8;
const STATS: usize = 10;

fn spawn_tray(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
) {
    let fonts = fonts.into_inner();
    // The drawer: a trapezoid, top corners pulled in.
    let mat = cache.get(&mut mats, PlateStyle::PANEL.slanted([0.16, 0.0, 0.16, 0.0]));
    let hatch = cache.get(
        &mut mats,
        PlateStyle::fill(t::alpha(Color::BLACK, 0.4), t::alpha(Color::BLACK, 0.4), Color::NONE).hatched(1.0),
    );
    commands
        .spawn((
            Name::new("Tray"),
            TrayRoot { t: 0.0, shown: None },
            layout(Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                bottom: px(t::BAR_H - t::LW),
                justify_content: JustifyContent::Center,
                ..default()
            }),
            UiTransform::default(),
            GlobalZIndex(28),
            Visibility::Hidden,
        ))
        .with_children(|root| {
            root.spawn((
                TrayPlate,
                plate_node(
                    Node {
                        min_height: px(t::TRAY_H),
                        min_width: px(560),
                        max_width: percent(96),
                        padding: UiRect::new(px(50), px(50), px(10), px(10)),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    UiRect::new(px(t::LW), px(t::LW), px(t::LW), px(0)),
                    [0.0; 4],
                ),
                mat,
                plate_shadow(),
            ))
            .with_children(|tray| {
                road_page(tray, fonts, &hatch);
                zone_page(tray, fonts, &hatch);
                clear_page(tray, fonts);
                junction_page(tray, fonts);
                maps_page(tray, fonts);
                stats_page(tray, fonts);
                page(tray, Page::Budget, |_| {});
                page(tray, Page::People, |_| {});
            });
        });
}

fn page(p: &mut ChildSpawnerCommands, which: Page, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn((
        which,
        Name::new(format!("Tray page {which:?}")),
        layout(Node {
            align_items: AlignItems::Center,
            column_gap: px(t::S6),
            display: Display::None,
            ..default()
        }),
    ))
    .with_children(f);
}

/// Title block: accent tick, title, caps subtitle, optional undo.
fn title_block(p: &mut ChildSpawnerCommands, fonts: &UiFonts, title: &str, sub: &str, undo: bool) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Column,
        min_width: px(98),
        ..default()
    }))
    .with_children(|c| {
        c.spawn(title_tick());
        c.spawn((
            Text::new(title.to_uppercase()),
            fonts.figures(t::FONT_TITLE, t::W_BOLD),
            t::caps_spacing(t::FONT_TITLE, 0.12),
            TextColor(t::TEXT),
            TextLayout::no_wrap(),
            Pickable::IGNORE,
        ));
        c.spawn((
            caps(fonts, sub, t::FONT_CAP, t::TEXT_MUTED),
            Node {
                margin: UiRect::top(px(6)),
                ..default()
            },
        ));
        if undo {
            let id = spawn_button(
                c,
                UiButton::new(UiAction::Undo, ButtonKind::Ghost),
                Node {
                    margin: UiRect::new(px(-6), px(0), px(8), px(0)),
                    padding: UiRect::axes(px(6), px(3)),
                    column_gap: px(6),
                    align_items: AlignItems::Center,
                    align_self: AlignSelf::Start,
                    ..default()
                },
                Some(
                    Tooltip::new(
                        "Undo",
                        "Take back the last road, zoning or demolition. Nothing is refunded; demolished buildings \
                         don't come back.",
                    )
                    .key("Ctrl+Z"),
                ),
                |b, id| {
                    b.spawn((
                        super::widgets::icon(fonts, icon::UNDO, 13.0, t::TEXT_MUTED),
                        ButtonFg {
                            owner: id,
                            base: t::TEXT_MUTED,
                        },
                    ));
                    b.spawn((
                        caps(fonts, "Undo", 10.5, t::TEXT_MUTED),
                        ButtonFg {
                            owner: id,
                            base: t::TEXT_MUTED,
                        },
                    ));
                },
            );
            c.commands().entity(id).insert(EnableWhen::CanUndo);
        }
    });
}

/// The slanted rule between tray groups.
fn tray_rule(p: &mut ChildSpawnerCommands) {
    p.spawn((
        Node {
            width: px(t::LW),
            height: px(86),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(t::LINE),
        UiTransform {
            rotation: Rot2::degrees(9.0),
            ..default()
        },
        Pickable::IGNORE,
    ));
}

/// An option row: caps caption and its controls.
fn opt_row(p: &mut ChildSpawnerCommands, fonts: &UiFonts, caption: &str, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn(layout(Node {
        align_items: AlignItems::Center,
        column_gap: px(8),
        ..default()
    }))
    .with_children(|r| {
        r.spawn((
            caps(fonts, caption, t::FONT_CAP, t::TEXT_MUTED),
            Node {
                width: px(56),
                ..default()
            },
        ));
        f(r);
    });
}

fn options(p: &mut ChildSpawnerCommands, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(8),
        ..default()
    }))
    .with_children(f);
}

/// Three live figures, the last one in the accent colour (cost).
fn figures(p: &mut ChildSpawnerCommands, fonts: &UiFonts, which: Page) {
    p.spawn(layout(Node {
        column_gap: px(18),
        margin: UiRect::left(px(6)),
        ..default()
    }))
    .with_children(|r| {
        for i in 0..3 {
            r.spawn(layout(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::End,
                row_gap: px(5),
                min_width: px(if i == 2 { 96.0 } else { 58.0 }),
                ..default()
            }))
            .with_children(|c| {
                c.spawn((
                    figure(fonts, "", if i == 2 { 30.0 } else { t::FONT_TITLE }, t::TEXT),
                    FigText(which, i, false),
                ));
                c.spawn((caps(fonts, "", t::FONT_CAP, t::TEXT_MUTED), FigText(which, i, true)));
            });
        }
    });
}

/// A picture item: picture, name and price/state in the footer.
#[allow(clippy::too_many_arguments)]
fn item(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    name: &str,
    tip: Tooltip,
    locked: Option<&MaterialNode<PlateMaterial>>,
    price: impl Bundle,
    pic: impl FnOnce(&mut ChildSpawnerCommands),
) -> Entity {
    let id = spawn_button(
        p,
        UiButton::new(action, ButtonKind::Item),
        Node {
            width: px(116),
            height: px(98),
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            ..default()
        },
        Some(tip),
        |b, id| {
            b.spawn(layout(Node {
                height: px(66),
                width: percent(100),
                overflow: Overflow::clip(),
                ..default()
            }))
            .insert(BackgroundColor(t::PIC_GRASS))
            .with_children(|pc| {
                pic(pc);
                if let Some(h) = locked {
                    pc.spawn((
                        layout(Node {
                            position_type: PositionType::Absolute,
                            left: px(0),
                            right: px(0),
                            top: px(0),
                            bottom: px(0),
                            ..default()
                        }),
                        h.clone(),
                    ));
                }
            });
            b.spawn((
                ActiveFill { owner: id },
                layout(Node {
                    height: px(30),
                    padding: UiRect::horizontal(px(9)),
                    border: UiRect::top(px(1)),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    column_gap: px(4),
                    ..default()
                }),
                super::widgets::vgrad(Color::NONE, Color::NONE),
                BorderColor::all(t::LINE),
            ))
            .with_children(|f| {
                let base = if locked.is_some() { t::TEXT_MUTED } else { t::TEXT };
                f.spawn((
                    label(fonts, name, t::FONT_BODY, t::W_SEMIBOLD, base),
                    ButtonFg { owner: id, base },
                ));
                f.spawn((
                    price,
                    ButtonFg {
                        owner: id,
                        base: t::POSITIVE,
                    },
                ));
            });
            spawn_brackets(b, id);
        },
    );
    if locked.is_some() {
        p.commands().entity(id).insert(Disabled);
    }
    id
}

/// A road seen from above: asphalt band with lane marks (and a median).
fn road_pic(p: &mut ChildSpawnerCommands, lanes: usize) {
    let w = match lanes {
        2 => 44.0,
        4 => 66.0,
        _ => 88.0,
    };
    p.spawn(layout(Node {
        position_type: PositionType::Absolute,
        left: px((116.0 - w) / 2.0),
        width: px(w),
        top: px(0),
        bottom: px(0),
        ..default()
    }))
    .insert(BackgroundColor(t::PIC_ASPHALT))
    .with_children(|r| {
        for k in 1..lanes {
            let x = w / lanes as f32 * k as f32;
            if lanes > 2 && k == lanes / 2 {
                r.spawn((
                    layout(Node {
                        position_type: PositionType::Absolute,
                        left: px(x - 3.0),
                        width: px(6),
                        top: px(0),
                        bottom: px(0),
                        ..default()
                    }),
                    BackgroundColor(t::PIC_GRASS),
                ));
                continue;
            }
            for d in 0..6 {
                r.spawn((
                    layout(Node {
                        position_type: PositionType::Absolute,
                        left: px(x - 1.0),
                        width: px(2),
                        top: px(d as f32 * 12.0 - 2.0),
                        height: px(7),
                        ..default()
                    }),
                    BackgroundColor(t::alpha(Color::WHITE, 0.8)),
                ));
            }
        }
    });
}

/// Zoned lots seen from above: two rows of lots in the zone's colour along
/// a road.
fn zone_pic(p: &mut ChildSpawnerCommands, zone: Option<Zone>) {
    p.spawn((
        layout(Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(0),
            bottom: px(0),
            height: px(12),
            ..default()
        }),
        BackgroundColor(t::PIC_ASPHALT),
    ));
    let c = match zone {
        Some(z) => t::zone_color(Some(z)),
        None => t::alpha(Color::WHITE, 0.0),
    };
    for row in 0..2 {
        for col in 0..4 {
            let mut n = Node {
                position_type: PositionType::Absolute,
                left: px(8.0 + col as f32 * 26.0),
                top: px(7.0 + row as f32 * 22.0),
                width: px(22),
                height: px(18),
                border: UiRect::all(px(1)),
                ..default()
            };
            if zone.is_none() {
                n.border = UiRect::all(px(1));
            }
            p.spawn((
                layout(n),
                BackgroundColor(if zone.is_some() { t::alpha(c, 0.85) } else { Color::NONE }),
                BorderColor::all(if zone.is_some() {
                    t::mix(c, Color::BLACK, 0.3)
                } else {
                    t::alpha(Color::WHITE, 0.55)
                }),
            ));
        }
    }
}

fn road_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts, hatch: &MaterialNode<PlateMaterial>) {
    page(p, Page::Road, |pg| {
        title_block(pg, fonts, "Roads", "3 types", true);
        pg.spawn(layout(Node {
            column_gap: px(8),
            ..default()
        }))
        .with_children(|items| {
            for (kind, lanes, desc) in [
                (
                    RoadChoice::Street,
                    2,
                    "One lane each way. Cheap; fine for quiet neighbourhoods.",
                ),
                (
                    RoadChoice::Avenue,
                    4,
                    "Two lanes each way with a median, faster. Needs gentler bends (radius 24 m).",
                ),
            ] {
                let name = super::context::road_kind_name(kind);
                item(
                    items,
                    fonts,
                    UiAction::RoadKind(kind),
                    name,
                    Tooltip::new(if kind == RoadChoice::Street { "Street" } else { "Avenue" }, desc)
                        .key("R")
                        .price(format!("{} per 8 m", format::money(road_price(kind)))),
                    None,
                    (figure(fonts, "", 15.0, t::POSITIVE), TrayLabel::RoadPrice(kind)),
                    |pc| road_pic(pc, lanes),
                );
            }
            item(
                items,
                fonts,
                UiAction::None,
                "Highway",
                Tooltip::new(
                    "Highway",
                    "Three lanes each way. Only the highway into town exists; you can't build more yet.",
                ),
                Some(hatch),
                (
                    layout(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(3),
                        ..default()
                    }),
                    Text::new(""),
                ),
                |pc| road_pic(pc, 6),
            );
        });
        tray_rule(pg);
        options(pg, |o| {
            opt_row(o, fonts, "Shape", |r| {
                spawn_segments(
                    r,
                    fonts,
                    &[
                        (
                            UiAction::RoadMode(RoadMode::Straight),
                            Some(icon::SLASH),
                            "Straight",
                            Some(
                                Tooltip::new(
                                    "Straight",
                                    "Click (or drag) from point to point; pieces chain into one road. Enter or \
                                     double-click builds.",
                                )
                                .key("Tab"),
                            ),
                        ),
                        (
                            UiAction::RoadMode(RoadMode::Curve),
                            Some(icon::SPLINE),
                            "Curve",
                            Some(
                                Tooltip::new(
                                    "Curve",
                                    "Three clicks: the start, the corner where the two directions meet, the end.",
                                )
                                .key("Tab"),
                            ),
                        ),
                        (
                            UiAction::RoadMode(RoadMode::Continuous),
                            Some(icon::ROUTE),
                            "Chain",
                            Some(
                                Tooltip::new(
                                    "Continuous",
                                    "Click along the way: every piece leaves the last one smoothly.",
                                )
                                .key("Tab"),
                            ),
                        ),
                        (
                            UiAction::RoadMode(RoadMode::Grid),
                            Some(icon::GRID),
                            "Grid",
                            Some(
                                Tooltip::new(
                                    "Street grid",
                                    "Three clicks: a corner, the width, the length. Any angle, blocks of the \
                                     chosen size.",
                                )
                                .key("Tab"),
                            ),
                        ),
                        (
                            UiAction::RoadMode(RoadMode::Upgrade),
                            Some(icon::UPGRADE),
                            "Upgrade",
                            Some(
                                Tooltip::new(
                                    "Change road type",
                                    "Click a road for the stretch between junctions, or drag along it, to make it \
                                     the chosen type.",
                                )
                                .key("Tab"),
                            ),
                        ),
                    ],
                );
            });
            opt_row(o, fonts, "Snap", |r| {
                r.spawn(layout(Node::default())).with_children(|g| {
                    for (i, (rule, text, title, desc)) in [
                        (
                            SnapRule::Nodes,
                            "Ends",
                            "Road ends",
                            "Ends and junctions of existing roads.",
                        ),
                        (
                            SnapRule::Roads,
                            "Roads",
                            "Onto roads",
                            "A point on a road makes a T junction.",
                        ),
                        (
                            SnapRule::Angles,
                            "15°",
                            "Angles",
                            "Straight on and at right angles to the road you start from, and every 15°.",
                        ),
                        (
                            SnapRule::Lengths,
                            "8 m",
                            "Whole cells",
                            "Lengths in whole cells of 8 m.",
                        ),
                        (
                            SnapRule::Lattice,
                            "Lots",
                            "Line up with lots",
                            "The lattice of nearby zoning blocks, so lots line up across the street.",
                        ),
                        (
                            SnapRule::Parallel,
                            "Parallel",
                            "Parallel roads",
                            "Parallel to a nearby road at the set distance.",
                        ),
                        (
                            SnapRule::Extensions,
                            "Lines",
                            "Road extensions",
                            "The lines of dead ends, continued.",
                        ),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        spawn_toggle(
                            g,
                            fonts,
                            UiAction::Snap(rule),
                            text,
                            Tooltip::new(title, desc).key("Alt: all off"),
                            i == 0,
                        );
                    }
                });
            });
            opt_row(o, fonts, "Spacing", |r| {
                r.spawn((
                    layout(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(4),
                        margin: UiRect::right(px(14)),
                        ..default()
                    }),
                    ShowWhen::GridMode,
                ))
                .with_children(|s| {
                    spawn_stepper(
                        s,
                        fonts,
                        UiAction::BlockSize(-1),
                        icon::MINUS,
                        Tooltip::new("Smaller blocks", "").key("["),
                    );
                    s.spawn((
                        figure(fonts, "", 17.0, t::TEXT),
                        TrayLabel::Block,
                        Node {
                            min_width: px(44),
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                        super::widgets::Justified(Justify::Center),
                    ));
                    spawn_stepper(
                        s,
                        fonts,
                        UiAction::BlockSize(1),
                        icon::PLUS,
                        Tooltip::new("Bigger blocks", "").key("]"),
                    );
                });
                r.spawn((caps(fonts, "Blocks", 10.5, t::TEXT_FAINT), ShowWhen::GridMode));
                r.spawn((
                    layout(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(4),
                        margin: UiRect::left(px(6)),
                        ..default()
                    }),
                    ShowWhen::ParallelOn,
                ))
                .with_children(|s| {
                    spawn_stepper(
                        s,
                        fonts,
                        UiAction::ParallelStep(-1),
                        icon::MINUS,
                        Tooltip::new("Closer parallel roads", ""),
                    );
                    s.spawn((
                        figure(fonts, "", 17.0, t::TEXT),
                        TrayLabel::Parallel,
                        Node {
                            min_width: px(40),
                            ..default()
                        },
                        super::widgets::Justified(Justify::Center),
                    ));
                    spawn_stepper(
                        s,
                        fonts,
                        UiAction::ParallelStep(1),
                        icon::PLUS,
                        Tooltip::new("Wider parallel roads", ""),
                    );
                });
                r.spawn((caps(fonts, "Parallel", 10.5, t::TEXT_FAINT), ShowWhen::ParallelOn));
            });
        });
        tray_rule(pg);
        figures(pg, fonts, Page::Road);
    });
}

fn zone_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts, hatch: &MaterialNode<PlateMaterial>) {
    let _ = hatch;
    page(p, Page::Zone, |pg| {
        title_block(pg, fonts, "Zones", "Free", true);
        pg.spawn(layout(Node {
            column_gap: px(8),
            ..default()
        }))
        .with_children(|items| {
            for (zone, name, desc) in [
                (
                    Some(Zone::Residential),
                    "Homes",
                    "Residential: homes. Residents need jobs and a road to drive on.",
                ),
                (
                    Some(Zone::Commercial),
                    "Shops",
                    "Commercial: shops and offices. Jobs; they need customers.",
                ),
                (
                    Some(Zone::Industrial),
                    "Industry",
                    "Industrial: factories and depots. Many jobs; keep them away from homes.",
                ),
                (None, "De-zone", "Remove zoning from empty lots."),
            ] {
                let price = (
                    caps(fonts, "", 10.5, t::TEXT_MUTED),
                    zone.map_or(TrayLabel::ZoneHint, TrayLabel::ZoneDemand),
                );
                item(
                    items,
                    fonts,
                    UiAction::Tool(ActiveTool::Zone(zone)),
                    name,
                    Tooltip::new(super::widgets::zone_name(zone), desc)
                        .key("Z")
                        .price("Free"),
                    None,
                    price,
                    |pc| zone_pic(pc, zone),
                );
            }
        });
        tray_rule(pg);
        options(pg, |o| {
            opt_row(o, fonts, "Mode", |r| {
                spawn_segments(
                    r,
                    fonts,
                    &[
                        (
                            UiAction::ZoneMode(ZoneMode::Paint),
                            Some(icon::BRUSH),
                            "Paint",
                            Some(Tooltip::new("Paint", "Drag over the lots to zone them cell by cell.").key("Tab")),
                        ),
                        (
                            UiAction::ZoneMode(ZoneMode::Frontage),
                            Some(icon::ROUTE),
                            "Along road",
                            Some(
                                Tooltip::new(
                                    "Along a road",
                                    "Click one side of a road to zone it between two junctions, or drag along a \
                                     road.",
                                )
                                .key("Tab"),
                            ),
                        ),
                        (
                            UiAction::ZoneMode(ZoneMode::Fill),
                            Some(icon::PAINT_BUCKET),
                            "Fill",
                            Some(
                                Tooltip::new("Fill", "Click to zone all the connected land around the cursor.")
                                    .key("Tab"),
                            ),
                        ),
                        (
                            UiAction::ZoneMode(ZoneMode::Marquee),
                            Some(icon::SCAN),
                            "Rectangle",
                            Some(
                                Tooltip::new(
                                    "Rectangle",
                                    "Drag a rectangle; it follows the lots of the block you start on.",
                                )
                                .key("Tab"),
                            ),
                        ),
                    ],
                );
            });
            opt_row(o, fonts, "Depth", |r| {
                r.spawn(layout(Node::default())).with_children(|g| {
                    for d in 1..=citysim_sim::balance::BLOCK_DEPTH {
                        let (text, desc) = if d == 1 {
                            ("1 row", "Only the row of lots along the road (along-road mode).")
                        } else {
                            ("2 rows", "The whole depth of the block (along-road mode).")
                        };
                        let id = spawn_button(
                            g,
                            UiButton::new(UiAction::ZoneDepth(d), ButtonKind::Segment),
                            Node {
                                height: px(32),
                                padding: UiRect::horizontal(px(12)),
                                margin: UiRect::left(px(if d == 1 { 0.0 } else { -1.0 })),
                                align_items: AlignItems::Center,
                                ..default()
                            },
                            Some(Tooltip::new("Depth", desc)),
                            |b, id| super::widgets::button_face(b, fonts, id, None, text),
                        );
                        g.commands().entity(id).insert(EnableWhen::Frontage);
                    }
                });
            });
        });
        tray_rule(pg);
        figures(pg, fonts, Page::Zone);
    });
}

fn clear_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    page(p, Page::Clear, |pg| {
        title_block(pg, fonts, "Clear", "Roads & buildings", true);
        tray_rule(pg);
        options(pg, |o| {
            opt_row(o, fonts, "Mode", |r| {
                spawn_segments(
                    r,
                    fonts,
                    &[
                        (
                            UiAction::BulldozeMode(BulldozeMode::Road),
                            Some(icon::ROUTE),
                            "Road or building",
                            Some(
                                Tooltip::new(
                                    "Road or building",
                                    "Click a road to remove it between two junctions, drag along it to remove \
                                     part; click a building.",
                                )
                                .key("Tab"),
                            ),
                        ),
                        (
                            UiAction::BulldozeMode(BulldozeMode::Area),
                            Some(icon::SCAN),
                            "Area",
                            Some(
                                Tooltip::new("Area", "Drag a rectangle over roads and buildings to remove them.")
                                    .key("Tab"),
                            ),
                        ),
                    ],
                );
            });
            o.spawn((
                paragraph(fonts, "", t::FONT_BODY, t::W_MEDIUM, t::TEXT_MUTED),
                TrayLabel::ClearHint,
                Node {
                    width: px(330),
                    ..default()
                },
            ));
        });
        tray_rule(pg);
        figures(pg, fonts, Page::Clear);
    });
}

fn junction_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    page(p, Page::Junction, |pg| {
        title_block(pg, fonts, "Junctions", "Lights & signs", false);
        tray_rule(pg);
        options(pg, |o| {
            opt_row(o, fonts, "Click", |r| {
                use citysim_sim::JunctionSetting as S;
                spawn_segments(
                    r,
                    fonts,
                    &[
                        (
                            UiAction::JunctionBrush(None),
                            Some(icon::ROTATE),
                            "Next",
                            Some(Tooltip::new(
                                "Switch in turn",
                                "Each click moves a junction on: automatic, traffic lights, priority.",
                            )),
                        ),
                        (
                            UiAction::JunctionBrush(Some(S::Auto)),
                            None,
                            "Auto",
                            Some(Tooltip::new(
                                "Automatic",
                                "The city decides: lights where busy roads meet, priority signs elsewhere.",
                            )),
                        ),
                        (
                            UiAction::JunctionBrush(Some(S::Signal)),
                            None,
                            "Lights",
                            Some(Tooltip::new("Traffic lights", "Signal phases in turn.")),
                        ),
                        (
                            UiAction::JunctionBrush(Some(S::Priority)),
                            None,
                            "Priority",
                            Some(Tooltip::new(
                                "Priority",
                                "The main road goes first; side roads give way.",
                            )),
                        ),
                    ],
                );
            });
            o.spawn((
                paragraph(
                    fonts,
                    "Point at a junction of three or more roads to see its phases and queues; click to apply.",
                    t::FONT_BODY,
                    t::W_MEDIUM,
                    t::TEXT_MUTED,
                ),
                Node {
                    width: px(360),
                    ..default()
                },
            ));
        });
        tray_rule(pg);
        // The hovered junction.
        pg.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(6),
            width: px(470),
            ..default()
        }))
        .with_children(|c| {
            c.spawn((
                label(fonts, "", t::FONT_LEAD, t::W_SEMIBOLD, t::TEXT),
                TrayLabel::JunctionTitle,
            ));
            c.spawn((
                caps(fonts, "No junction under the cursor", t::FONT_CAP, t::TEXT_FAINT),
                ShowWhen::NoJunctionHovered,
            ));
            c.spawn((
                layout(Node {
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(18),
                    row_gap: px(2),
                    ..default()
                }),
                ShowWhen::JunctionHovered,
            ))
            .with_children(|g| {
                for i in 0..JUNCTION_ROWS {
                    g.spawn((
                        JunctionRow(i),
                        layout(Node {
                            width: px(226),
                            justify_content: JustifyContent::SpaceBetween,
                            column_gap: px(8),
                            height: px(20),
                            align_items: AlignItems::Center,
                            display: Display::None,
                            ..default()
                        }),
                    ))
                    .with_children(|r| {
                        r.spawn((caps(fonts, "", 10.5, t::TEXT_MUTED), TrayLabel::JunctionLabel(i)));
                        r.spawn((
                            label(fonts, "", t::FONT_SMALL, t::W_SEMIBOLD, t::TEXT),
                            TrayLabel::JunctionValue(i),
                            Node {
                                max_width: px(150),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                        ));
                    });
                }
            });
            c.spawn((
                caps(fonts, "", 10.5, t::TEXT_FAINT),
                TrayLabel::JunctionHint,
                ShowWhen::JunctionHovered,
            ));
        });
    });
}

fn maps_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    page(p, Page::Maps, |pg| {
        title_block(pg, fonts, "Maps", "Over the city", false);
        pg.spawn(layout(Node {
            column_gap: px(8),
            ..default()
        }))
        .with_children(|items| {
            for (mode, name, desc) in [
                (OverlayMode::None, "Off", "The city as it is."),
                (
                    OverlayMode::Traffic,
                    "Traffic",
                    "Roads coloured by how full they are: green flows, red is jammed.",
                ),
                (OverlayMode::Zones, "Zones", "Zoning in strong colours over every lot."),
            ] {
                item(
                    items,
                    fonts,
                    UiAction::Overlay(mode),
                    name,
                    Tooltip::new(name, desc).key(if mode == OverlayMode::Traffic { "T" } else { "O" }),
                    None,
                    Text::new(""),
                    |pc| map_pic(pc, mode),
                );
            }
        });
        tray_rule(pg);
        // Legend.
        pg.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(7),
            min_width: px(260),
            ..default()
        }))
        .with_children(|l| {
            l.spawn((caps(fonts, "Legend", t::FONT_CAP, t::TEXT_MUTED), ShowWhen::TrafficMap));
            for (band, name, what) in traffic_bands() {
                l.spawn((
                    layout(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(9),
                        ..default()
                    }),
                    ShowWhen::TrafficMap,
                ))
                .with_children(|r| {
                    r.spawn((
                        layout(Node {
                            width: px(26),
                            height: px(7),
                            ..default()
                        }),
                        BackgroundColor(band.color()),
                    ));
                    r.spawn((
                        label(fonts, name, t::FONT_BODY, t::W_SEMIBOLD, t::TEXT),
                        Node {
                            width: px(62),
                            ..default()
                        },
                    ));
                    r.spawn(label(fonts, what, t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED));
                });
            }
            l.spawn((
                caps(fonts, "Built on zoned land", t::FONT_CAP, t::TEXT_MUTED),
                ShowWhen::ZonesMap,
            ));
            for (i, zone) in [Zone::Residential, Zone::Commercial, Zone::Industrial]
                .into_iter()
                .enumerate()
            {
                l.spawn((
                    layout(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(9),
                        ..default()
                    }),
                    ShowWhen::ZonesMap,
                ))
                .with_children(|r| {
                    r.spawn((
                        layout(Node {
                            width: px(14),
                            height: px(14),
                            ..default()
                        }),
                        BackgroundColor(t::zone_color(Some(zone))),
                    ));
                    r.spawn((
                        label(
                            fonts,
                            super::widgets::zone_name(Some(zone)),
                            t::FONT_BODY,
                            t::W_SEMIBOLD,
                            t::TEXT,
                        ),
                        Node {
                            width: px(92),
                            ..default()
                        },
                    ));
                    r.spawn((
                        label(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                        TrayLabel::ZoneCount(i),
                    ));
                });
            }
            l.spawn((
                paragraph(
                    fonts,
                    "Pick a map to see traffic or zoning over the city. O cycles them, T toggles traffic.",
                    t::FONT_BODY,
                    t::W_MEDIUM,
                    t::TEXT_MUTED,
                ),
                Node {
                    width: px(260),
                    ..default()
                },
                ShowWhen::NoMap,
            ));
        });
        tray_rule(pg);
        pg.spawn(layout(Node {
            column_gap: px(18),
            ..default()
        }))
        .with_children(|r| {
            for i in 0..2 {
                r.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::End,
                    row_gap: px(5),
                    min_width: px(70),
                    ..default()
                }))
                .with_children(|c| {
                    c.spawn((
                        figure(fonts, "", t::FONT_TITLE, t::TEXT),
                        TrayLabel::MapFigure(i, false),
                    ));
                    c.spawn((
                        caps(fonts, "", t::FONT_CAP, t::TEXT_MUTED),
                        TrayLabel::MapFigure(i, true),
                    ));
                });
            }
        });
    });
}

/// A little map tile for the maps tray.
fn map_pic(p: &mut ChildSpawnerCommands, mode: OverlayMode) {
    // A crossroads: a horizontal and a vertical road.
    let road = |p: &mut ChildSpawnerCommands, n: Node, c: Color| {
        p.spawn((layout(n), BackgroundColor(c)));
    };
    let base = |c| c;
    match mode {
        OverlayMode::Zones => {
            for (i, z) in [Zone::Residential, Zone::Commercial, Zone::Industrial, Zone::Residential]
                .into_iter()
                .enumerate()
            {
                let (x, y) = ((i % 2) as f32, (i / 2) as f32);
                road(
                    p,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(4.0 + x * 62.0),
                        top: px(3.0 + y * 36.0),
                        width: px(46),
                        height: px(24),
                        ..default()
                    },
                    t::alpha(t::zone_color(Some(z)), 0.9),
                );
            }
        }
        _ => {}
    }
    let (h, v) = match mode {
        OverlayMode::Traffic => (t::POSITIVE, t::NEGATIVE),
        _ => (base(t::PIC_ASPHALT), t::PIC_ASPHALT),
    };
    road(
        p,
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(0),
            top: px(28),
            height: px(10),
            ..default()
        },
        h,
    );
    road(
        p,
        Node {
            position_type: PositionType::Absolute,
            left: px(53),
            width: px(10),
            top: px(0),
            bottom: px(0),
            ..default()
        },
        v,
    );
    if mode == OverlayMode::Traffic {
        road(
            p,
            Node {
                position_type: PositionType::Absolute,
                left: px(53),
                width: px(10),
                top: px(38),
                bottom: px(0),
                ..default()
            },
            t::WARNING,
        );
    }
}

/// Legend rows of the traffic map: band, name, what it means.
pub fn traffic_bands() -> [(Band, &'static str, String); 3] {
    [
        (Band::Free, "Free", format!("under {} full", pct(CONGESTION_BUSY))),
        (
            Band::Busy,
            "Busy",
            format!(
                "{}\u{2013}{} full",
                (CONGESTION_BUSY * 100.0).round() as i64,
                pct(CONGESTION_JAM)
            ),
        ),
        (
            Band::Jammed,
            "Jammed",
            format!(
                "over {} · under {} km/h",
                pct(CONGESTION_JAM),
                (JAM_CRAWL_MPS * 3.6).round() as i64
            ),
        ),
    ]
}

fn stats_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    page(p, Page::Stats, |pg| {
        title_block(pg, fonts, "Statistics", "Live", false);
        tray_rule(pg);
        pg.spawn(layout(Node {
            flex_wrap: FlexWrap::Wrap,
            column_gap: px(26),
            row_gap: px(12),
            width: px(760),
            ..default()
        }))
        .with_children(|g| {
            for i in 0..STATS {
                g.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(4),
                    width: px(126),
                    ..default()
                }))
                .with_children(|c| {
                    c.spawn((figure(fonts, "", t::FONT_TITLE, t::TEXT), TrayLabel::Stat(i, false)));
                    c.spawn((caps(fonts, "", 10.5, t::TEXT_MUTED), TrayLabel::Stat(i, true)));
                });
            }
        });
    });
}

// ---------------------------------------------------------------- systems

/// Show the page that belongs to the tool / info slot.
#[allow(clippy::type_complexity)]
fn show_page(
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    ui: Res<UiState>,
    mut root: Query<&mut TrayRoot>,
    mut pages: Query<(&Page, &mut Node)>,
) {
    let Ok(mut r) = root.single_mut() else { return };
    let want = current_page(*tool, *modes, &ui);
    // Switch pages only while the drawer is down (it sinks, then rises).
    if r.t <= 0.001 && r.shown != want {
        r.shown = want;
        for (p, mut n) in &mut pages {
            let d = if Some(*p) == want { Display::Flex } else { Display::None };
            if n.display != d {
                n.display = d;
            }
        }
    }
}

fn slide_tray(
    time: Res<Time<Real>>,
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    ui: Res<UiState>,
    mut root: Query<(&mut TrayRoot, &mut UiTransform, &mut Visibility, &ComputedNode)>,
) {
    let Ok((mut r, mut tf, mut vis, computed)) = root.single_mut() else {
        return;
    };
    let want = current_page(*tool, *modes, &ui);
    let target = if want.is_some() && want == r.shown { 1.0 } else { 0.0 };
    // Rise calmly, sink quickly (a switch between pages stays short).
    let tau = if target > r.t { 0.07 } else { 0.035 };
    r.t += (target - r.t) * approach(time.delta_secs(), tau);
    if (target - r.t).abs() < 0.003 {
        r.t = target;
    }
    let h = (computed.size.y * computed.inverse_scale_factor).max(t::TRAY_H);
    let e = 1.0 - (1.0 - r.t).powi(2);
    let y = Val2::px(0.0, (1.0 - e) * (h + 4.0));
    if tf.translation != y {
        tf.translation = y;
    }
    vis.set_if_neq(if r.t > 0.0 && r.shown.is_some() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    });
}

fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Normal => t::TEXT,
        Tone::Muted => t::TEXT_MUTED,
        Tone::Accent => t::ACCENT_TEXT,
        Tone::Good => t::POSITIVE,
        Tone::Warn => t::WARNING,
        Tone::Bad => t::NEGATIVE,
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_figures(
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    state: Res<ToolState>,
    opts: Res<ToolOptions>,
    sim: Res<SimWorld>,
    kind: Res<RoadChoice>,
    mut figs: Query<(&FigText, &mut Text, &mut TextColor), Without<TrayLabel>>,
    mut labels: Query<(&TrayLabel, &mut Text, &mut TextColor), Without<FigText>>,
) {
    let plan = &state.plan;
    let road = match *tool {
        ActiveTool::Road if modes.road.draws() || modes.road == RoadMode::Upgrade => {
            let mut f = readout::road_figures(modes.road, plan);
            if state.draft.is_empty() && !matches!(plan.pending, super::tools::Pending::Road { .. }) {
                // Nothing drawn yet: what a cell of this road costs.
                let k = super::tools::road_kind(*kind);
                f[2] = Figure {
                    caption: "Per 8 m",
                    value: format::money(k.cost_per_cell()),
                    tone: Tone::Accent,
                };
            }
            Some(f)
        }
        _ => None,
    };
    let zone = matches!(*tool, ActiveTool::Zone(_)).then(|| readout::zone_figures(plan));
    let clear = (*tool == ActiveTool::Bulldoze).then(|| readout::bulldoze_figures(plan));
    for (f, mut text, mut color) in &mut figs {
        let set = match f.0 {
            Page::Road => road.as_ref(),
            Page::Zone => zone.as_ref(),
            Page::Clear => clear.as_ref(),
            _ => None,
        };
        let Some(set) = set else { continue };
        let fig = &set[f.1];
        let (v, c) = if f.2 {
            (fig.caption.to_uppercase(), t::TEXT_MUTED)
        } else {
            (fig.value.clone(), tone_color(fig.tone))
        };
        if text.0 != v {
            text.0 = v;
        }
        color.set_if_neq(TextColor(c));
    }
    let w = &sim.world;
    for (l, mut text, mut color) in &mut labels {
        let (v, c) = match l {
            TrayLabel::Block => (format!("{} m", opts.block_m() as i64), t::TEXT),
            TrayLabel::Parallel => (format!("{} m", opts.parallel_m() as i64), t::TEXT),
            TrayLabel::RoadPrice(k) => (format::money(road_price(*k)), t::POSITIVE),
            TrayLabel::ZoneDemand(z) => {
                let d = match z {
                    Zone::Residential => w.demand.residential,
                    Zone::Commercial => w.demand.commercial,
                    Zone::Industrial => w.demand.industrial,
                };
                let (word, c) = demand_word(d);
                (word.to_uppercase(), c)
            }
            TrayLabel::ZoneHint => (String::new(), t::TEXT_MUTED),
            TrayLabel::ClearHint => (
                match modes.bulldoze {
                    BulldozeMode::Road => {
                        "Click a road for the stretch between junctions, drag along it for a part, or click a \
                         building."
                    }
                    BulldozeMode::Area => "Drag a rectangle over the roads and buildings to remove.",
                }
                .to_string(),
                t::TEXT_MUTED,
            ),
            _ => continue,
        };
        if text.0 != v {
            text.0 = v;
        }
        color.set_if_neq(TextColor(c));
    }
}

/// Plain reading of a demand value (`-1..=1`).
pub fn demand_word(v: f32) -> (&'static str, Color) {
    if v >= 0.5 {
        ("High", t::POSITIVE)
    } else if v >= 0.15 {
        ("Some", t::TEXT)
    } else if v > -0.15 {
        ("None", t::TEXT_MUTED)
    } else {
        ("Surplus", t::WARNING)
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_conditions(
    mut commands: Commands,
    modes: Res<ToolModes>,
    opts: Res<ToolOptions>,
    overlay: Res<OverlayMode>,
    highlight: Res<WorldHighlight>,
    state: Res<ToolState>,
    tool: Res<ActiveTool>,
    mut shows: Query<(&ShowWhen, &mut Node)>,
    enables: Query<(Entity, &EnableWhen, Has<Disabled>)>,
) {
    let hovered_junction = matches!(highlight.hovered, Some(WorldTarget::Junction { .. }));
    for (w, mut n) in &mut shows {
        let show = match w {
            ShowWhen::GridMode => modes.road == RoadMode::Grid,
            ShowWhen::NotGridMode => modes.road != RoadMode::Grid,
            ShowWhen::ParallelOn => opts.rule(SnapRule::Parallel),
            ShowWhen::TrafficMap => *overlay == OverlayMode::Traffic,
            ShowWhen::ZonesMap => *overlay == OverlayMode::Zones,
            ShowWhen::NoMap => *overlay == OverlayMode::None,
            ShowWhen::JunctionHovered => hovered_junction,
            ShowWhen::NoJunctionHovered => !hovered_junction,
        };
        let d = if show { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
    }
    for (e, w, disabled) in &enables {
        let on = match w {
            EnableWhen::Frontage => modes.zone == ZoneMode::Frontage,
            EnableWhen::CanUndo => state.can_undo(),
            EnableWhen::DrawsRoads => *tool == ActiveTool::Road && modes.road.draws(),
        };
        if on && disabled {
            commands.entity(e).remove::<Disabled>();
        } else if !on && !disabled {
            commands.entity(e).insert(Disabled);
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_junction(
    time: Res<Time<Real>>,
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    sim: Res<SimWorld>,
    highlight: Res<WorldHighlight>,
    mut rows: Query<(&JunctionRow, &mut Node)>,
    mut labels: Query<(&TrayLabel, &mut Text, &mut TextColor)>,
    mut age: Local<f32>,
) {
    if !(*tool == ActiveTool::Road && modes.road == RoadMode::Junction) {
        return;
    }
    *age += time.delta_secs();
    if *age < 0.2 && !highlight.is_changed() {
        return;
    }
    *age = 0.0;
    let card = match &highlight.hovered {
        Some(WorldTarget::Junction { node, .. }) => junction_card(&sim.world, *node),
        _ => None,
    };
    let n = card.as_ref().map_or(0, |c| c.rows.len().min(JUNCTION_ROWS));
    for (r, mut node) in &mut rows {
        let d = if r.0 < n { Display::Flex } else { Display::None };
        if node.display != d {
            node.display = d;
        }
    }
    let Some(card) = card else {
        for (l, mut text, _) in &mut labels {
            if matches!(l, TrayLabel::JunctionTitle) && !text.0.is_empty() {
                text.0.clear();
            }
        }
        return;
    };
    for (l, mut text, mut color) in &mut labels {
        let (v, c) = match l {
            TrayLabel::JunctionTitle => (card.title.clone(), t::TEXT),
            TrayLabel::JunctionHint => (card.hint.to_uppercase(), t::TEXT_FAINT),
            TrayLabel::JunctionLabel(i) => match card.rows.get(*i) {
                Some(r) => (r.label.to_uppercase(), t::TEXT_MUTED),
                None => continue,
            },
            TrayLabel::JunctionValue(i) => match card.rows.get(*i) {
                Some(r) => (r.value.clone(), r.color),
                None => continue,
            },
            _ => continue,
        };
        if text.0 != v {
            text.0 = v;
        }
        color.set_if_neq(TextColor(c));
    }
}

fn update_maps(
    ui: Res<UiState>,
    overlay: Res<OverlayMode>,
    sim: Res<SimWorld>,
    mut labels: Query<(&TrayLabel, &mut Text)>,
    mut last: Local<Option<(OverlayMode, u32, u32, [u32; 3])>>,
) {
    if ui.tray != Some(InfoTray::Maps) {
        return;
    }
    let s = &sim.world.stats;
    let key = (
        *overlay,
        (s.avg_speed * 10.0) as u32,
        (s.congestion_share * 1000.0) as u32,
        s.buildings_by_kind,
    );
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let figs: [(&str, String); 2] = match *overlay {
        OverlayMode::Traffic | OverlayMode::None => [
            (
                "Avg speed",
                if s.avg_speed > 0.0 {
                    super::context::kmh(s.avg_speed)
                } else {
                    "\u{2014}".into()
                },
            ),
            ("Busy roads", pct(s.congestion_share)),
        ],
        OverlayMode::Zones => [
            ("Buildings", format::group(s.buildings as u64)),
            ("Jobs", format::group(s.jobs as u64)),
        ],
    };
    for (l, mut text) in &mut labels {
        let v = match l {
            TrayLabel::MapFigure(i, cap) => {
                let (c, v) = &figs[*i];
                if *cap { c.to_uppercase() } else { v.clone() }
            }
            TrayLabel::ZoneCount(i) => format!("{} buildings", format::group(s.buildings_by_kind[*i] as u64)),
            _ => continue,
        };
        if text.0 != v {
            text.0 = v;
        }
    }
}

fn update_stats(
    time: Res<Time<Real>>,
    ui: Res<UiState>,
    sim: Res<SimWorld>,
    mut labels: Query<(&TrayLabel, &mut Text, &mut TextColor)>,
    mut age: Local<f32>,
) {
    if ui.tray != Some(InfoTray::Stats) {
        return;
    }
    *age += time.delta_secs();
    if *age < 0.3 && !ui.is_changed() {
        return;
    }
    *age = 0.0;
    let s = &sim.world.stats;
    let g = |v: u32| format::group(v as u64);
    let stats: [(&str, String, Color); STATS] = [
        ("Residents", g(s.population), t::TEXT),
        ("Households", g(s.households), t::TEXT),
        ("Workforce", g(s.workforce), t::TEXT),
        ("Employed", g(s.employed), t::TEXT),
        (
            "Jobless",
            g(s.unemployed),
            if s.unemployment > 0.15 { t::NEGATIVE } else { t::TEXT },
        ),
        ("Jobs", g(s.jobs), t::TEXT),
        ("Vacant jobs", g(s.vacant_jobs), t::TEXT),
        (
            "Avg commute",
            super::bar::commute_text(s.avg_commute_s),
            super::bar::commute_color(s.avg_commute_s),
        ),
        ("Cars on roads", g(s.vehicles_on_road), t::TEXT),
        (
            "Homes · shops · industry",
            format!(
                "{} · {} · {}",
                s.buildings_by_kind[0], s.buildings_by_kind[1], s.buildings_by_kind[2]
            ),
            t::TEXT,
        ),
    ];
    for (l, mut text, mut color) in &mut labels {
        let TrayLabel::Stat(i, cap) = l else { continue };
        let (c, v, col) = &stats[*i];
        let (v, col) = if *cap {
            (c.to_uppercase(), t::TEXT_MUTED)
        } else {
            (v.clone(), *col)
        };
        if text.0 != v {
            text.0 = v;
        }
        color.set_if_neq(TextColor(col));
    }
}
