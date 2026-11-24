//! Context card next to the cursor: what the next click does and what it
//! costs (red with the reason when it is not possible); with Inspect, the
//! hovered building, road or zoned lot.

use bevy::prelude::*;
use bevy::ui::ComputedNode;
use bevy::window::PrimaryWindow;
use citysim_sim::{Cell, CommandError, Occupant, RoadKind, World, Zone, balance};

use super::UiState;
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::tools::{Action, ToolState};
use super::widgets::{approach, label, layout, zone_icon, zone_name};
use crate::shared::{ActiveTool, GameSet, HoveredCell, PointerOverUi, PreviewKind, SimWorld, ToolPreview};

pub struct ContextPlugin;

impl Plugin for ContextPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_card)
            .add_systems(Update, update_card.in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct CardRoot;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum CardPart {
    IconBox,
    Icon,
    Title,
    Line,
    Hint,
    Bar,
    BarFill,
}

/// What the card shows this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub glyph: char,
    pub color: Color,
    pub title: String,
    pub title_color: Color,
    pub line: String,
    pub line_color: Color,
    pub hint: String,
    /// Fill fraction and colour of a small meter (occupancy, congestion).
    pub bar: Option<(f32, Color)>,
}

/// Player-facing reason for a rejected command.
pub fn error_text(e: &CommandError) -> String {
    match e {
        CommandError::OutOfBounds => "Outside the map".into(),
        CommandError::ZeroLength => "Pick a second point".into(),
        CommandError::NotStraight => "Roads must be straight or at 45 degrees".into(),
        CommandError::BlockedByBuilding(_) => "A building is in the way".into(),
        CommandError::DiagonalCrossing(_) => "Diagonal roads can't cross here".into(),
        CommandError::InsufficientFunds { cost, money } => {
            format!(
                "Not enough money: costs {}, you have {}",
                format::money(*cost),
                format::money(*money)
            )
        }
        CommandError::InvalidTaxRate(_) => "Tax rate out of range".into(),
    }
}

fn spawn_card(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Context card"),
            CardRoot,
            layout(Node {
                position_type: PositionType::Absolute,
                min_width: px(170),
                max_width: px(300),
                padding: UiRect::new(px(t::S3 + 2.0), px(t::S4 + 2.0), px(t::S3 + 1.0), px(t::S3 + 2.0)),
                column_gap: px(t::S3 + 2.0),
                align_items: AlignItems::Start,
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(t::RADIUS_SM + 1.0)),
                ..default()
            }),
            BackgroundColor(t::PANEL_RAISED),
            BorderColor::all(t::PANEL_BORDER),
            BoxShadow(vec![ShadowStyle {
                color: t::alpha(Color::BLACK, 0.4),
                x_offset: px(0),
                y_offset: px(5),
                spread_radius: px(0),
                blur_radius: px(16),
            }]),
            GlobalZIndex(90),
            UiTransform::default(),
            Visibility::Hidden,
        ))
        .with_children(|card| {
            card.spawn((
                CardPart::IconBox,
                layout(Node {
                    width: px(28),
                    height: px(28),
                    margin: UiRect::top(px(1)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    border_radius: BorderRadius::all(px(t::RADIUS_SM)),
                    flex_shrink: 0.0,
                    ..default()
                }),
                BackgroundColor(t::alpha(t::ACCENT, 0.16)),
            ))
            .with_child((
                super::widgets::icon(fonts, icon::ROAD, t::ICON_SM + 1.0, t::ACCENT),
                CardPart::Icon,
            ));
            card.spawn(layout(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(1),
                flex_grow: 1.0,
                ..default()
            }))
            .with_children(|col| {
                col.spawn((label(fonts, "", t::FONT_BODY, t::W_SEMIBOLD, t::TEXT), CardPart::Title));
                col.spawn((label(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT), CardPart::Line));
                col.spawn((
                    CardPart::Bar,
                    layout(Node {
                        height: px(4),
                        width: percent(100),
                        margin: UiRect::vertical(px(t::S2)),
                        border_radius: BorderRadius::all(px(2)),
                        overflow: Overflow::clip(),
                        ..default()
                    }),
                    BackgroundColor(t::WELL),
                ))
                .with_child((
                    CardPart::BarFill,
                    layout(Node {
                        height: percent(100),
                        width: percent(0),
                        border_radius: BorderRadius::all(px(2)),
                        ..default()
                    }),
                    BackgroundColor(t::ACCENT),
                ));
                col.spawn((
                    label(fonts, "", t::FONT_CAPTION + 0.5, t::W_REGULAR, t::TEXT_MUTED),
                    CardPart::Hint,
                ));
            });
        });
}

fn congestion_color(c: f32) -> Color {
    if c < 0.5 {
        t::mix(t::POSITIVE, t::WARNING, c / 0.5)
    } else {
        t::mix(t::WARNING, t::NEGATIVE, (c - 0.5) / 0.5)
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", format::group(n as u64), if n == 1 { one } else { many })
}

/// Card for the current tool and preview; `None` hides it.
pub fn tool_card(
    tool: ActiveTool,
    action: Action,
    preview: &ToolPreview,
    world: &World,
    hovered: Option<Cell>,
) -> Option<Card> {
    let mut card = Card {
        glyph: icon::INSPECT,
        color: t::ACCENT,
        title: String::new(),
        title_color: t::TEXT,
        line: String::new(),
        line_color: t::TEXT,
        hint: String::new(),
        bar: None,
    };
    let cost_text = |cost: i64| if cost > 0 { format::money(cost) } else { "free".into() };
    match tool {
        ActiveTool::Inspect => return inspect_card(world, hovered?),
        ActiveTool::Road => {
            card.glyph = icon::ROAD;
            match action {
                Action::Road { held, .. } if preview.kind == PreviewKind::Road => {
                    let n = preview.cells.len();
                    if let Some(e) = &preview.error {
                        card.title = "Can't build here".into();
                        card.line = error_text(e);
                        card.line_color = t::NEGATIVE;
                    } else if preview.cost == 0 {
                        card.title = "Road already built".into();
                        card.line = plural(n, "cell", "cells");
                        card.line_color = t::TEXT_MUTED;
                    } else {
                        card.title = "Build road".into();
                        card.line = format!("{} · {}", plural(n, "cell", "cells"), cost_text(preview.cost));
                    }
                    card.hint = if held {
                        "Release to build · Right-click to cancel".into()
                    } else {
                        "Click to build · Right-click to cancel".into()
                    };
                }
                Action::Road { .. } => {
                    card.title = "Road".into();
                    card.line = "Move to the end point".into();
                    card.hint = "Right-click to cancel".into();
                }
                _ => {
                    hovered?;
                    card.title = "Road".into();
                    card.line = format!("{} per cell", format::money(balance::ROAD_COST_PER_CELL));
                    card.line_color = t::TEXT_MUTED;
                    card.hint = "Click a start point, or drag".into();
                }
            }
        }
        ActiveTool::Zone(zone) => {
            hovered.or(preview.rect.map(|r| r.max))?;
            card.glyph = zone_icon(zone);
            card.color = t::zone_color(zone);
            card.title = match zone {
                Some(_) => format!("Zone {}", zone_name(zone).to_lowercase()),
                None => "Remove zoning".into(),
            };
            let n = preview.cells.len();
            let dragging = matches!(action, Action::Rect { .. });
            if n == 0 {
                card.line = match zone {
                    Some(_) => "Needs a road within 2 cells".into(),
                    None => "Nothing zoned here".into(),
                };
                card.line_color = if zone.is_some() { t::WARNING } else { t::TEXT_MUTED };
            } else {
                card.line = format!("{} · free", plural(n, "cell", "cells"));
            }
            card.hint = if dragging {
                "Release to apply · Right-click to cancel".into()
            } else {
                "Drag a rectangle · Z switches type".into()
            };
        }
        ActiveTool::Bulldoze => {
            hovered.or(preview.rect.map(|r| r.max))?;
            card.glyph = icon::BULLDOZE;
            card.color = t::NEGATIVE;
            card.title = "Bulldoze".into();
            let n = preview.cells.len();
            if let Some(e) = &preview.error {
                card.line = error_text(e);
                card.line_color = t::NEGATIVE;
            } else if n == 0 {
                card.line = "Nothing to remove".into();
                card.line_color = t::TEXT_MUTED;
            } else {
                card.line = format!("{} · {}", plural(n, "cell", "cells"), cost_text(preview.cost));
            }
            card.hint = if matches!(action, Action::Rect { .. }) {
                "Release to clear · Right-click to cancel".into()
            } else {
                "Drag over roads and buildings".into()
            };
        }
    }
    if preview.error.is_some() {
        card.title_color = t::NEGATIVE;
        card.color = t::NEGATIVE;
        card.glyph = icon::ALERT;
    }
    Some(card)
}

fn inspect_card(world: &World, cell: Cell) -> Option<Card> {
    let cd = world.grid.get(cell)?;
    let mut card = Card {
        glyph: icon::INSPECT,
        color: t::ACCENT,
        title: String::new(),
        title_color: t::TEXT,
        line: String::new(),
        line_color: t::TEXT,
        hint: String::new(),
        bar: None,
    };
    match cd.occupant {
        Occupant::Building(id) => {
            let b = world.buildings.get(id)?;
            card.glyph = zone_icon(Some(b.kind));
            card.color = t::zone_color(Some(b.kind));
            let what = match b.kind {
                Zone::Residential => "Homes",
                Zone::Commercial => "Shops & offices",
                Zone::Industrial => "Industry",
            };
            card.title = format!("{what} · level {}", b.level);
            let (noun, hint) = match b.kind {
                Zone::Residential => ("residents", "Residents commute to jobs by car"),
                _ => ("jobs filled", "Workers drive in from homes"),
            };
            card.line = format!(
                "{} / {} {noun}",
                format::group(b.occupied as u64),
                format::group(b.capacity as u64)
            );
            let fill = if b.capacity > 0 {
                b.occupied as f32 / b.capacity as f32
            } else {
                0.0
            };
            card.bar = Some((fill, card.color));
            let (w, h) = b.size();
            card.hint = format!("{hint} · {w}×{h} lot");
        }
        Occupant::Road => {
            let roads = &world.roads;
            let pos = cell.center();
            let segs: Vec<_> = if let Some(s) = roads.segment_through(pos) {
                vec![s]
            } else if let Some(n) = roads.node_at(pos) {
                roads.nodes[n].segments.clone()
            } else {
                Vec::new()
            };
            let segs: Vec<_> = segs.iter().filter_map(|&s| roads.segments.get(s)).collect();
            if segs.is_empty() {
                return None;
            }
            let highway = segs.iter().any(|s| s.kind == RoadKind::Highway);
            card.glyph = icon::ROAD;
            card.title = match (highway, segs.len()) {
                (true, _) => "Highway".into(),
                (false, 1) => "Street".into(),
                (false, n) => format!("Intersection · {n} ways"),
            };
            let load: f32 = segs.iter().map(|s| s.total_load()).sum();
            let cap: f32 = segs.iter().map(|s| s.capacity[0] + s.capacity[1]).sum();
            let worst = segs.iter().map(|s| s.congestion()).fold(0.0f32, f32::max);
            let c = congestion_color(worst);
            card.line = format!(
                "{} cars · {} busy",
                format::group(load.round() as u64),
                format::percent((worst * 100.0).round() / 100.0)
            );
            card.bar = Some((worst, c));
            card.color = c;
            card.hint = if worst > 0.8 {
                "Jammed: add a parallel route".into()
            } else if worst > 0.5 {
                format!("Getting busy · capacity {}", format::group(cap.round() as u64))
            } else {
                format!("Flowing · capacity {} cars", format::group(cap.round() as u64))
            };
        }
        Occupant::Empty => {
            let zone = cd.zone?;
            card.glyph = zone_icon(Some(zone));
            card.color = t::zone_color(Some(zone));
            card.title = format!("{} zone", zone_name(Some(zone)));
            card.line = "Empty lot".into();
            card.line_color = t::TEXT_MUTED;
            card.hint = "Buildings grow when there is demand".into();
        }
    }
    Some(card)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_card(
    time: Res<Time<Real>>,
    tool: Res<ActiveTool>,
    state: Res<ToolState>,
    preview: Res<ToolPreview>,
    hovered: Res<HoveredCell>,
    over_ui: Res<PointerOverUi>,
    ui: Res<UiState>,
    sim: Res<SimWorld>,
    ui_scale: Res<UiScale>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut root: Query<
        (
            &mut Node,
            &mut Visibility,
            &ComputedNode,
            &mut UiTransform,
            &mut BorderColor,
        ),
        (With<CardRoot>, Without<CardPart>),
    >,
    mut parts: Query<
        (
            &CardPart,
            Option<&mut Text>,
            Option<&mut TextColor>,
            Option<&mut BackgroundColor>,
            Option<&mut Node>,
        ),
        Without<CardRoot>,
    >,
    mut slide: Local<f32>,
) {
    let Ok((mut node, mut vis, computed, mut tf, mut border)) = root.single_mut() else {
        return;
    };
    let cursor = window
        .single()
        .ok()
        .and_then(|w| w.cursor_position().map(|c| (c, Vec2::new(w.width(), w.height()))));
    let card = if ui.menu_open || over_ui.0 && !state.busy() {
        None
    } else {
        tool_card(*tool, state.action, &preview, &sim.world, hovered.0)
    };
    let (Some(card), Some((cursor, win))) = (card, cursor) else {
        vis.set_if_neq(Visibility::Hidden);
        *slide = 0.0;
        return;
    };
    for (part, text, color, bg, pnode) in &mut parts {
        match part {
            CardPart::IconBox => {
                if let Some(mut bg) = bg {
                    bg.set_if_neq(BackgroundColor(t::alpha(card.color, 0.16)));
                }
            }
            CardPart::Icon => set_text(text, color, card.glyph.to_string(), card.color),
            CardPart::Title => set_text(text, color, card.title.clone(), card.title_color),
            CardPart::Line => set_text(text, color, card.line.clone(), card.line_color),
            CardPart::Hint => {
                let show = !card.hint.is_empty();
                set_text(text, color, card.hint.clone(), t::TEXT_MUTED);
                set_display(pnode, show);
            }
            CardPart::Bar => set_display(pnode, card.bar.is_some()),
            CardPart::BarFill => {
                if let Some((f, c)) = card.bar {
                    if let Some(mut bg) = bg {
                        bg.set_if_neq(BackgroundColor(c));
                    }
                    if let Some(mut n) = pnode {
                        let w = percent((f.clamp(0.0, 1.0) * 100.0).max(2.0));
                        if n.width != w {
                            n.width = w;
                        }
                    }
                }
            }
        }
    }
    let invalid = card.title_color == t::NEGATIVE;
    border.set_if_neq(BorderColor::all(if invalid {
        t::alpha(t::NEGATIVE, 0.55)
    } else {
        t::PANEL_BORDER
    }));
    // Place beside the cursor, flipping to stay inside the window.
    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let screen = win / scale;
    let size = computed.size * computed.inverse_scale_factor;
    let off = Vec2::new(22.0, 22.0);
    let mut pos = cursor + off;
    if pos.x + size.x > screen.x - t::EDGE {
        pos.x = cursor.x - off.x - size.x;
    }
    if pos.y + size.y > screen.y - t::EDGE - t::TOOL_BUTTON - 2.0 * t::EDGE {
        pos.y = cursor.y - off.y * 0.6 - size.y;
    }
    pos = pos.max(Vec2::splat(t::EDGE));
    node.left = px(pos.x.round());
    node.top = px(pos.y.round());
    *slide += (1.0 - *slide) * approach(time.delta_secs(), 0.05);
    tf.translation = Val2::px(0.0, (1.0 - *slide) * 5.0);
    if size.x > 0.0 {
        vis.set_if_neq(Visibility::Inherited);
    }
}

fn set_text(text: Option<Mut<Text>>, color: Option<Mut<TextColor>>, s: String, c: Color) {
    if let Some(mut text) = text
        && text.0 != s
    {
        text.0 = s;
    }
    if let Some(mut color) = color {
        color.set_if_neq(TextColor(c));
    }
}

fn set_display(node: Option<Mut<Node>>, show: bool) {
    if let Some(mut n) = node {
        let d = if show { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
    }
}
