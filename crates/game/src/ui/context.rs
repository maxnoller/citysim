//! Context card next to the cursor: what the next click does and what it
//! costs (red with the sim's reason when it is not possible); with Inspect,
//! the hovered building, road or zoned lot with live numbers from the sim.

use bevy::prelude::*;
use bevy::ui::ComputedNode;
use bevy::window::PrimaryWindow;
use citysim_sim::junction;
use citysim_sim::{
    Aspect, Building, BuildingId, CommandError, JunctionControl, JunctionSetting, RoadKind, RoadSegment, SignalStage,
    World, Zone, balance,
};

use super::UiState;
use super::format;
use super::fsnap;
use super::readout::{self, Readout, Symbol, Tone};
use super::theme::{self as t, UiFonts, icon};
use super::tools::{ToolOptions, ToolState};
use super::widgets::{approach, label, layout, number, zone_icon};
use crate::shared::{
    ActiveTool, CONGESTION_BUSY, CONGESTION_JAM, GameSet, PointerOverUi, RoadChoice, RoadMode, SimWorld, ToolModes,
    ToolPreview, WorldHighlight, WorldTarget,
};

pub struct ContextPlugin;

impl Plugin for ContextPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_card)
            .add_systems(Update, (update_card, update_pinned).chain().in_set(GameSet::Render));
    }
}

/// Root of a card: 0 follows the cursor, 1 is pinned to the inspected
/// object (`WorldHighlight::selected`).
#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct CardRoot(u8);

/// Which card a part belongs to.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct CardOwner(u8);

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum CardPart {
    IconBox,
    Icon,
    Title,
    /// One label / value row.
    Row(usize),
    RowLabel(usize),
    RowValue(usize),
    Hint,
    Bar,
    BarFill,
}

/// Rows a card can show.
pub const MAX_ROWS: usize = 8;

/// One line of a card: a muted label on the left and a value on the right
/// (an empty label lets the value read as a plain line).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub value: String,
    pub color: Color,
}

impl Row {
    pub fn new(label: impl Into<String>, value: impl Into<String>, color: Color) -> Row {
        Row {
            label: label.into(),
            value: value.into(),
            color,
        }
    }

    pub fn line(value: impl Into<String>, color: Color) -> Row {
        Row::new("", value, color)
    }
}

/// What the card shows this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub glyph: char,
    pub color: Color,
    pub title: String,
    pub title_color: Color,
    /// Up to [`MAX_ROWS`]; the meter (if any) sits below the first row.
    pub rows: Vec<Row>,
    pub hint: String,
    /// Fill fraction and colour of a small meter (occupancy, congestion).
    pub bar: Option<(f32, Color)>,
}

impl Card {
    fn new(glyph: char, color: Color) -> Card {
        Card {
            glyph,
            color,
            title: String::new(),
            title_color: t::TEXT,
            rows: Vec::new(),
            hint: String::new(),
            bar: None,
        }
    }
}

/// Player-facing reason for a rejected command (from the sim's own error).
pub fn error_text(e: &CommandError) -> String {
    match e {
        CommandError::OutOfBounds => "That's outside the map".into(),
        CommandError::ZeroLength => "Pick a second point for the road".into(),
        CommandError::NotStraight => "Roads must be straight or at 45 degrees".into(),
        CommandError::BlockedByBuilding(_) => "A building is in the way. Bulldoze it first".into(),
        CommandError::DiagonalCrossing(_) => "Diagonal roads can't cross between cells".into(),
        CommandError::InsufficientFunds { cost, money } => format!(
            "Not enough money: costs {}, you have {}",
            format::money(*cost),
            format::money(*money)
        ),
        CommandError::InvalidRoadKind(_) => "That road type can't be built".into(),
        CommandError::NotAJunction(_) => "Not a junction: point at a crossing of three or more roads".into(),
        CommandError::InvalidTaxRate(_) => format!(
            "Tax rate must be between 0% and {}",
            format::percent(balance::MAX_TAX_RATE)
        ),
        CommandError::TooShort { length, min, .. } => {
            format!("Too short: {length:.0} m here, a road piece needs {min:.0} m")
        }
        CommandError::TooTight { radius, min, .. } => {
            format!("Bend too tight: radius {radius:.0} m, this road needs {min:.0} m")
        }
        CommandError::SharpAngle { degrees, min, .. } => {
            format!("Roads would meet at {degrees:.0}°: at least {min:.0}° needed")
        }
        CommandError::TooClose { .. } => "Too close to another road: keep at least a lot's width".into(),
        CommandError::SelfCrossing { .. } => "The road crosses itself".into(),
        CommandError::BuildingInTheWay { .. } => "A building is in the way. Bulldoze it first".into(),
        CommandError::OffMap { .. } => "That's outside the map".into(),
        CommandError::NoSuchRoad => "There's no road there".into(),
        CommandError::NotANode => "Not a junction: point at a crossing of three or more roads".into(),
        CommandError::TooManyArms { arms, .. } => format!("{arms} roads can't meet at one junction (6 at most)"),
    }
}

fn spawn_card(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    spawn_card_with(&mut commands, fonts, 0);
    spawn_card_with(&mut commands, fonts, 1);
}

fn spawn_card_with(commands: &mut Commands, fonts: &UiFonts, which: u8) {
    commands
        .spawn((
            Name::new("Context card"),
            CardRoot(which),
            layout(Node {
                position_type: PositionType::Absolute,
                min_width: px(190),
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
            // The cursor card draws over the pinned one.
            GlobalZIndex(if which == 0 { 90 } else { 85 }),
            UiTransform::default(),
            Visibility::Hidden,
        ))
        .with_children(|card| {
            card.spawn((
                CardPart::IconBox,
                CardOwner(which),
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
                CardOwner(which),
            ));
            card.spawn(layout(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(1),
                flex_grow: 1.0,
                ..default()
            }))
            .with_children(|col| {
                col.spawn((
                    label(fonts, "", t::FONT_BODY, t::W_SEMIBOLD, t::TEXT),
                    CardPart::Title,
                    CardOwner(which),
                    Node {
                        margin: UiRect::bottom(px(1)),
                        ..default()
                    },
                ));
                for i in 0..MAX_ROWS {
                    col.spawn((
                        CardPart::Row(i),
                        CardOwner(which),
                        layout(Node {
                            justify_content: JustifyContent::SpaceBetween,
                            align_items: AlignItems::Baseline,
                            column_gap: px(t::S5),
                            display: Display::None,
                            ..default()
                        }),
                    ))
                    .with_children(|r| {
                        r.spawn((
                            label(fonts, "", t::FONT_SMALL, t::W_REGULAR, t::TEXT_MUTED),
                            CardPart::RowLabel(i),
                            CardOwner(which),
                        ));
                        r.spawn((
                            number(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT),
                            CardPart::RowValue(i),
                            CardOwner(which),
                        ));
                    });
                    if i == 0 {
                        col.spawn((
                            CardPart::Bar,
                            CardOwner(which),
                            layout(Node {
                                height: px(4),
                                width: percent(100),
                                margin: UiRect::new(px(0), px(0), px(t::S2), px(t::S2 + 1.0)),
                                border_radius: BorderRadius::all(px(2)),
                                overflow: Overflow::clip(),
                                ..default()
                            }),
                            BackgroundColor(t::WELL),
                        ))
                        .with_child((
                            CardPart::BarFill,
                            CardOwner(which),
                            layout(Node {
                                height: percent(100),
                                width: percent(0),
                                border_radius: BorderRadius::all(px(2)),
                                ..default()
                            }),
                            BackgroundColor(t::ACCENT),
                        ));
                    }
                }
                col.spawn((
                    label(fonts, "", t::FONT_CAPTION + 0.5, t::W_REGULAR, t::TEXT_MUTED),
                    CardPart::Hint,
                    CardOwner(which),
                    Node {
                        margin: UiRect::top(px(t::S1)),
                        ..default()
                    },
                ));
            });
        });
}

/// Traffic band of a direction's `congestion_in`, as the overlay draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    Free,
    Busy,
    Jammed,
}

impl Band {
    pub fn of(congestion: f32) -> Band {
        if congestion >= CONGESTION_JAM {
            Band::Jammed
        } else if congestion >= CONGESTION_BUSY {
            Band::Busy
        } else {
            Band::Free
        }
    }

    pub fn color(self) -> Color {
        match self {
            Band::Free => t::POSITIVE,
            Band::Busy => t::WARNING,
            Band::Jammed => t::NEGATIVE,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Band::Free => "free",
            Band::Busy => "busy",
            Band::Jammed => "jammed",
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", format::group(n as u64), if n == 1 { one } else { many })
}

/// "Eastbound" etc. for travel from `from` to `to` (sim metres; -y is north,
/// the top of the screen in the default view).
pub fn heading_name(from: [f32; 2], to: [f32; 2]) -> &'static str {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    if dx == 0.0 && dy == 0.0 {
        return "Either way";
    }
    // Compass bearing: 0 = north (-y), clockwise.
    let bearing = dx.atan2(-dy).to_degrees().rem_euclid(360.0);
    const NAMES: [&str; 8] = [
        "Northbound",
        "Northeast",
        "Eastbound",
        "Southeast",
        "Southbound",
        "Southwest",
        "Westbound",
        "Northwest",
    ];
    NAMES[((bearing + 22.5) / 45.0) as usize % 8]
}

pub fn pct(f: f32) -> String {
    format!("{}%", (f * 100.0).round() as i64)
}

pub fn kmh(mps: f32) -> String {
    format!("{} km/h", (mps * 3.6).round() as i64)
}

/// "Street" / "Avenue".
pub fn road_kind_name(kind: RoadChoice) -> &'static str {
    match kind {
        RoadChoice::Street => "Street",
        RoadChoice::Avenue => "Avenue",
    }
}

/// Price of 8 m of road of `kind`.
pub fn road_price(kind: RoadChoice) -> i64 {
    super::tools::road_kind(kind).cost_per_cell()
}

fn tone_color(t: Tone) -> Color {
    match t {
        Tone::Normal => t::TEXT,
        Tone::Muted => t::TEXT_MUTED,
        Tone::Accent => t::ACCENT,
        Tone::Good => t::POSITIVE,
        Tone::Warn => t::WARNING,
        Tone::Bad => t::NEGATIVE,
    }
}

/// A card showing a readout.
pub fn readout_card(r: &Readout) -> Card {
    let (glyph, color) = match r.symbol {
        Symbol::Road => (icon::ROAD, t::ACCENT),
        Symbol::Curve => (icon::SPLINE, t::ACCENT),
        Symbol::Grid => (icon::GRID, t::ACCENT),
        Symbol::Upgrade => (icon::ROUTE, t::ACCENT),
        Symbol::Zone(z) => (zone_icon(z), t::zone_color(z)),
        Symbol::Bulldoze => (icon::BULLDOZE, t::NEGATIVE),
    };
    let mut card = Card::new(glyph, color);
    card.title = r.title.clone();
    if r.refused {
        card.title_color = t::NEGATIVE;
        card.color = t::NEGATIVE;
    }
    card.rows = r
        .lines
        .iter()
        .map(|l| Row::new(l.label.clone(), l.value.clone(), tone_color(l.tone)))
        .collect();
    card.rows.truncate(MAX_ROWS);
    card.hint = r.hint.clone();
    card
}

/// Card for the current tool and what it points at; `None` hides it.
pub fn tool_card(
    tool: ActiveTool,
    modes: ToolModes,
    kind: RoadChoice,
    state: &ToolState,
    opts: &ToolOptions,
    world: &World,
    hovered: Option<&WorldTarget>,
) -> Option<Card> {
    let plan = &state.plan;
    let dragging = state.press.is_some_and(|p| p.moved);
    match tool {
        ActiveTool::Inspect => inspect_card(world, hovered?),
        ActiveTool::Road if modes.road == RoadMode::Junction => {
            let Some(WorldTarget::Junction { node, .. }) = hovered else {
                state.cursor.filter(|_| state.on_map)?;
                let mut card = Card::new(icon::WAYPOINTS, t::ACCENT);
                card.title = "Junctions".into();
                card.rows
                    .push(Row::line("Point at a junction of three or more roads", t::TEXT_MUTED));
                card.hint = "Lights or priority signs are chosen automatically".into();
                return Some(card);
            };
            let mut card = junction_card(world, *node)?;
            let setting = junction::junction(world, *node).map(|j| j.setting).unwrap_or_default();
            card.hint = format!(
                "Click: switch to {}",
                setting_name(next_setting(setting)).to_lowercase()
            );
            Some(card)
        }
        ActiveTool::Road if modes.road == RoadMode::Upgrade => {
            state.cursor.filter(|_| state.on_map || state.busy())?;
            readout::upgrade(plan, dragging).map(|r| readout_card(&r))
        }
        ActiveTool::Road => {
            let live = state.live.as_ref();
            if state.draft.is_empty() {
                state.cursor.filter(|_| state.on_map)?;
                let k = super::tools::road_kind(kind);
                let mut card = readout_card(&readout::road_idle(modes.road, k, live, opts.block_m()));
                if world.economy.money < k.cost_per_cell() {
                    card.rows.push(Row::line(
                        format!("You have {}", format::money(world.economy.money)),
                        t::NEGATIVE,
                    ));
                }
                return Some(card);
            }
            let start = state
                .draft
                .pieces
                .first()
                .map(|c| c.start())
                .or(state.draft.points.first().copied());
            let start_dir = start
                .and_then(|p| fsnap::on_net(world, p, 0.5, 0.3))
                .and_then(|on| fsnap::direction_at(world, on, None));
            readout::road(modes.road, plan, live, start_dir, state.snap_off).map(|r| readout_card(&r))
        }
        ActiveTool::Zone(_) => {
            state.cursor.filter(|_| state.on_map || state.busy())?;
            readout::zone(plan, modes.zone, dragging).map(|r| readout_card(&r))
        }
        ActiveTool::Bulldoze => {
            state.cursor.filter(|_| state.on_map || state.busy())?;
            readout::bulldoze(plan).map(|r| readout_card(&r))
        }
    }
}

fn zone_slot(z: Zone) -> usize {
    match z {
        Zone::Residential => 0,
        Zone::Commercial => 1,
        Zone::Industrial => 2,
    }
}

/// Inspect card for a thing in the world.
pub fn inspect_card(world: &World, target: &WorldTarget) -> Option<Card> {
    match target {
        WorldTarget::Building(id) => building_card(world, *id, world.buildings.get(*id)?),
        WorldTarget::Road(stretch) => road_card(world, &stretch.segments),
        WorldTarget::Junction { node, .. } => junction_card(world, *node),
        WorldTarget::Vehicle(v) => vehicle_card(world, *v),
    }
}

/// A car or truck: who is in it and where it goes.
fn vehicle_card(world: &World, id: citysim_sim::VehicleId) -> Option<Card> {
    let v = world.vehicles.get(id)?;
    let view = super::people::vehicle_sheet(world, id)?;
    let mut card = Card::new(icon::CAR, t::ACCENT);
    card.title = view.title.clone();
    for l in view.lines.iter().take(MAX_ROWS - 1) {
        card.rows
            .push(Row::new(l.label.clone(), l.value.clone(), tone_color(l.tone)));
    }
    card.rows.push(Row::new("Speed", kmh(v.speed), t::TEXT_MUTED));
    card.hint = "Click to see who is in it".into();
    Some(card)
}

fn building_card(world: &World, id: BuildingId, b: &Building) -> Option<Card> {
    let zone = Some(b.kind);
    let mut card = Card::new(zone_icon(zone), t::zone_color(zone));
    let what = match b.kind {
        Zone::Residential => "Homes",
        Zone::Commercial => "Shops & offices",
        Zone::Industrial => "Industry",
    };
    let max_level = balance::MAX_LEVEL[zone_slot(b.kind)];
    card.title = format!("{what} · level {} of {max_level}", b.level);
    let of = |a: u32, b: u32| format!("{} / {}", format::group(a as u64), format::group(b as u64));
    let fill = if b.capacity > 0 {
        b.occupied as f32 / b.capacity as f32
    } else {
        0.0
    };
    card.bar = Some((fill, card.color));
    match b.kind {
        Zone::Residential => {
            card.rows
                .push(Row::new("Residents", of(b.occupied, b.capacity), t::TEXT));
            let (mut homes, mut jobless) = (0usize, 0usize);
            for h in world.households.values().filter(|h| h.home == id) {
                homes += 1;
                jobless += h.job.is_none() as usize;
            }
            if homes > 0 {
                // One worker per household (the sim's workforce).
                card.rows.push(Row::new(
                    "Households",
                    match jobless {
                        0 => format!("{} · all employed", format::group(homes as u64)),
                        n if n == homes => format!("{} · none employed", format::group(homes as u64)),
                        n => format!(
                            "{} · {} employed",
                            format::group(homes as u64),
                            format::group((homes - n) as u64)
                        ),
                    },
                    if jobless > 0 { t::WARNING } else { t::TEXT },
                ));
            }
            card.hint = "Residents drive to work and to the shops".into();
        }
        _ => {
            card.rows.push(Row::new("Workers", of(b.occupied, b.capacity), t::TEXT));
            let open = b.capacity.saturating_sub(b.occupied);
            card.rows.push(Row::new(
                "Open jobs",
                format::group(open as u64),
                if open > 0 { t::TEXT } else { t::TEXT_MUTED },
            ));
            card.hint = if b.kind == Zone::Industrial {
                "Workers drive in; trucks deliver goods to shops".into()
            } else {
                "Workers drive in; residents come to shop".into()
            };
        }
    }
    // Deep lots: the sim folds the second row into the lot of the building
    // that fronts the road.
    let (w, h) = b.size();
    if w * h > 1 {
        let side = match b.facing {
            citysim_sim::Facing::PlusX => "east",
            citysim_sim::Facing::PlusY => "south",
            citysim_sim::Facing::MinusX => "west",
            citysim_sim::Facing::MinusY => "north",
        };
        let deep = if w == h { "large lot" } else { "deep lot" };
        card.rows.push(Row::new(
            "Lot",
            format!("{w} x {h} cells · {deep}, faces {side}"),
            t::TEXT_MUTED,
        ));
    }
    // At-risk state, from the sim's own bookkeeping (`world.growth`).
    let meta = world.growth.meta.get(id).copied().unwrap_or_default();
    let now = world.time.tick;
    if let Some(since) = meta.unreachable_since {
        card.rows.push(Row::line("Cut off from the road network", t::NEGATIVE));
        card.rows.push(Row::line(
            format!(
                "Abandoned in ~{} unless reconnected",
                hours_text(abandon_in_ticks(since, now))
            ),
            t::NEGATIVE,
        ));
    }
    if b.occupied == 0 {
        let who = if b.kind == Zone::Residential {
            "residents"
        } else {
            "workers"
        };
        let young = now.saturating_sub(b.created_tick) < balance::TICKS_PER_DAY as u64;
        if young {
            card.rows
                .push(Row::line(format!("New · waiting for {who}"), t::TEXT_MUTED));
        } else {
            // Counted at midnight; abandoned at the midnight that reaches
            // `ABANDON_EMPTY_DAYS`.
            let left = balance::ABANDON_EMPTY_DAYS.saturating_sub(meta.empty_days).max(1);
            card.rows.push(Row::line(
                if left == 1 {
                    format!("No {who} · abandoned at midnight if still empty")
                } else {
                    format!("No {who} · abandoned after {left} more empty midnights")
                },
                t::WARNING,
            ));
        }
    }
    if card.rows.iter().any(|r| r.color == t::NEGATIVE) {
        card.color = t::NEGATIVE;
        card.glyph = icon::ALERT;
    }
    card.rows.truncate(MAX_ROWS);
    Some(card)
}

/// A stretch of road (its segments) with live traffic numbers.
fn road_card(world: &World, ids: &[citysim_sim::SegmentId]) -> Option<Card> {
    let roads = &world.roads;
    let segs: Vec<&RoadSegment> = ids.iter().filter_map(|&s| roads.segments.get(s)).collect();
    if segs.is_empty() {
        return None;
    }
    let highway = segs.iter().any(|s| s.kind == RoadKind::Highway);
    let worst = segs.iter().map(|s| s.congestion()).fold(0.0f32, f32::max);
    let band = Band::of(worst);
    let mut card = Card::new(icon::ROAD, band.color());
    let avenue = segs.iter().any(|s| s.kind == RoadKind::Avenue);
    let length: f32 = segs.iter().map(|s| s.length).sum();
    card.title = match (highway, avenue) {
        (true, _) => "Highway".into(),
        (false, true) => format!("Avenue · 2 lanes each way · {}", readout::metres(length)),
        _ => format!("Street · 1 lane each way · {}", readout::metres(length)),
    };
    let verdict = match band {
        Band::Free => "Free flow",
        Band::Busy => "Busy",
        Band::Jammed => "Jammed",
    };
    card.rows.push(Row::new("Traffic", verdict, band.color()));
    card.bar = Some((worst, band.color()));
    // Cars per lane right now, per segment and direction (kerb lane first).
    let mut lane_cars: std::collections::HashMap<(citysim_sim::SegmentId, bool), [u32; 4]> = Default::default();
    for v in world.vehicles.values() {
        if let Some(sid) = v.segment.filter(|s| ids.contains(s)) {
            lane_cars.entry((sid, v.forward)).or_default()[(v.lane as usize).min(3)] += 1;
        }
    }
    let dir_row = |name: &str, s: &RoadSegment, forward: bool| {
        let d = RoadSegment::dir(forward);
        let c = s.congestion_in(forward);
        let b = Band::of(c);
        let mut value = format!(
            "{} / {} · {}",
            format::group(s.load[d].round() as u64),
            plural(s.capacity[d].floor().max(1.0) as usize, "car", "cars"),
            b.word()
        );
        let lanes = s.lane_count() as usize;
        if lanes > 1 {
            let id = ids.iter().copied().find(|&i| roads.segments.get(i) == Some(s));
            let per = id
                .and_then(|i| lane_cars.get(&(i, forward)).copied())
                .unwrap_or_default();
            let parts: Vec<String> = (0..lanes.min(4)).map(|l| per[l].to_string()).collect();
            value = format!("lanes {} · {value}", parts.join("|"));
        }
        Row::new(name, value, b.color())
    };
    // Both directions of the busiest piece of the stretch.
    if let Some(s) = segs
        .iter()
        .copied()
        .max_by(|a, b| a.congestion().total_cmp(&b.congestion()))
    {
        let (pa, pb) = (roads.node_pos(s.a), roads.node_pos(s.b));
        let busiest = if segs.len() > 1 { " (busiest part)" } else { "" };
        card.rows
            .push(dir_row(&format!("{}{busiest}", heading_name(pa, pb)), s, true));
        card.rows.push(dir_row(heading_name(pb, pa), s, false));
    }
    // Cars on these segments right now (the sim's per-direction loads are
    // vehicle counts) and how fast they actually go.
    let (mut n, mut speed) = (0u32, 0.0f32);
    for v in world.vehicles.values() {
        if v.segment.is_some_and(|s| ids.contains(&s)) {
            n += 1;
            speed += v.speed;
        }
    }
    card.rows.push(Row::new(
        "Cars on it now",
        if n > 0 {
            format!("{} · avg {}", format::group(n as u64), kmh(speed / n as f32))
        } else {
            "none".into()
        },
        t::TEXT,
    ));
    let limit = segs.iter().map(|s| s.speed_limit).fold(0.0f32, f32::max);
    card.hint = match band {
        Band::Free if avenue => format!(
            "Speed limit {} · upkeep {}/cell a day",
            kmh(limit),
            format::money(RoadKind::Avenue.upkeep_per_cell())
        ),
        Band::Free => format!("Speed limit {}", kmh(limit)),
        Band::Busy => format!("Cars slow down here · limit {}", kmh(limit)),
        Band::Jammed => format!(
            "Over {} full: cars crawl (~{}) · add a route",
            pct(CONGESTION_JAM),
            kmh(JAM_CRAWL_MPS)
        ),
    };
    card.rows.truncate(MAX_ROWS);
    Some(card)
}

/// Compass name of a direction in sim space (-y is north).
pub fn compass(d: [f32; 2]) -> &'static str {
    let bearing = d[0].atan2(-d[1]).to_degrees().rem_euclid(360.0);
    const NAMES: [&str; 8] = [
        "north",
        "north-east",
        "east",
        "south-east",
        "south",
        "south-west",
        "west",
        "north-west",
    ];
    NAMES[((bearing + 22.5) / 45.0) as usize % 8]
}

fn compass_letter(d: [f32; 2]) -> &'static str {
    match compass(d) {
        "north" => "N",
        "north-east" => "NE",
        "east" => "E",
        "south-east" => "SE",
        "south" => "S",
        "south-west" => "SW",
        "west" => "W",
        _ => "NW",
    }
}

pub fn aspect_color(a: Aspect) -> Color {
    match a {
        Aspect::Green => t::POSITIVE,
        Aspect::Yellow => t::WARNING,
        Aspect::Red => t::NEGATIVE,
    }
}

fn aspect_word(a: Aspect) -> &'static str {
    match a {
        Aspect::Green => "Green",
        Aspect::Yellow => "Yellow",
        Aspect::Red => "Red",
    }
}

/// "Traffic lights" / "Priority".
pub fn control_name(c: JunctionControl) -> &'static str {
    match c {
        JunctionControl::Signal => "Traffic lights",
        JunctionControl::Priority => "Priority (give way)",
    }
}

/// The setting a click on the junction tool switches to.
pub fn next_setting(s: JunctionSetting) -> JunctionSetting {
    match s {
        JunctionSetting::Auto => JunctionSetting::Signal,
        JunctionSetting::Signal => JunctionSetting::Priority,
        JunctionSetting::Priority => JunctionSetting::Auto,
    }
}

pub fn setting_name(s: JunctionSetting) -> &'static str {
    match s {
        JunctionSetting::Auto => "Automatic",
        JunctionSetting::Signal => "Traffic lights",
        JunctionSetting::Priority => "Priority",
    }
}

/// Card for the junction at node `n`: control, signal phases, and the
/// aspect and queue of every approach.
pub fn junction_card(world: &World, n: citysim_sim::NodeId) -> Option<Card> {
    let view = junction::junction_view(world, n)?;
    let state = junction::junction(world, n)?;
    let signal = view.control == JunctionControl::Signal;
    let mut card = Card::new(icon::WAYPOINTS, if signal { t::WARNING } else { t::ACCENT });
    card.title = format!(
        "{} · {} ways",
        if signal { "Traffic lights" } else { "Priority junction" },
        view.approaches.len()
    );
    card.rows.push(Row::new(
        "Control",
        match view.setting {
            JunctionSetting::Auto => format!("{} · automatic", control_name(view.control)),
            _ => format!("{} · set by you", control_name(view.control)),
        },
        t::TEXT,
    ));
    if let (Some(plan), Some((cur, stage))) = (&state.plan, view.phase) {
        let names: Vec<String> = plan
            .phases
            .iter()
            .map(|p| {
                let arms: Vec<&str> = p
                    .arms
                    .iter()
                    .filter_map(|&a| state.arms.get(a as usize).map(|arm| compass_letter(arm.dir)))
                    .collect();
                format!("{}{}", arms.join("+"), if p.left_only { " left" } else { "" })
            })
            .collect();
        let now = names.get(cur as usize).cloned().unwrap_or_default();
        let stage = match stage {
            SignalStage::Green => "green",
            SignalStage::Yellow => "yellow",
            SignalStage::AllRed => "all red",
        };
        card.rows.push(Row::new(
            format!("Phase {} of {}", cur as usize + 1, names.len()),
            format!("{now} {stage}"),
            t::TEXT,
        ));
        card.rows.push(Row::new("Cycle", names.join(" → "), t::TEXT_MUTED));
    }
    for (i, a) in view.approaches.iter().enumerate() {
        let from = state.arms.get(i).map(|arm| compass(arm.dir)).unwrap_or("?");
        let queue = if a.queue > 0 {
            format!("{} waiting", a.queue)
        } else {
            "none waiting".into()
        };
        let (value, color) = match a.aspect {
            Some(asp) => {
                let mut v = aspect_word(asp).to_string();
                if let Some(l) = a.left_aspect
                    && l != asp
                {
                    v += &format!(", left {}", aspect_word(l).to_lowercase());
                } else if a.left_protected {
                    v += ", left protected";
                }
                if let Some(s) = a.time_to_change_s.filter(|&s| asp != Aspect::Yellow && s >= 0.5) {
                    v += &format!(" {}s", s.ceil() as i64);
                }
                (format!("{v} · {queue}"), aspect_color(asp))
            }
            None if a.major => (format!("Main road · {queue}"), t::TEXT),
            None => (format!("Gives way · {queue}"), t::TEXT_MUTED),
        };
        card.rows.push(Row::new(format!("From {from}"), value, color));
    }
    card.hint = if view.peak_per_minute > 0 {
        format!("Busiest minute yesterday: {} cars", view.peak_per_minute)
    } else {
        "J: junction tool to change control".into()
    };
    card.rows.truncate(MAX_ROWS);
    Some(card)
}

/// Ticks until a building cut off since tick `since` is abandoned: the
/// grace period runs out, then the next hourly reachability check removes it.
pub fn abandon_in_ticks(since: u64, now: u64) -> u64 {
    let due = since + balance::ABANDON_UNREACHABLE_GRACE_TICKS;
    let hour = balance::REACHABILITY_CHECK_INTERVAL_TICKS;
    (due.div_ceil(hour) * hour).saturating_sub(now)
}

/// In-game hours, rounded up ("1 h", "17 h").
pub fn hours_text(ticks: u64) -> String {
    let h = ticks.div_ceil(balance::TICKS_PER_HOUR as u64).max(1);
    format!("{h} h")
}

/// Speed cars crawl below on a jammed road (shared.rs: ~12 km/h at
/// `CONGESTION_JAM`).
pub const JAM_CRAWL_MPS: f32 = 12.0 / 3.6;

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_card(
    time: Res<Time<Real>>,
    tool: Res<ActiveTool>,
    (modes, kind): (Res<ToolModes>, Res<RoadChoice>),
    state: Res<ToolState>,
    preview: Res<ToolPreview>,
    opts: Res<ToolOptions>,
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
            &CardRoot,
        ),
        Without<CardPart>,
    >,
    mut parts: CardParts,
    highlight: Res<WorldHighlight>,
    mut slide: Local<f32>,
    mut cache: Local<CardCache>,
) {
    let Some((mut node, mut vis, computed, mut tf, mut border, _)) = root.iter_mut().find(|r| *r.5 == CardRoot(0))
    else {
        return;
    };
    let cursor = window
        .single()
        .ok()
        .and_then(|w| w.cursor_position().map(|c| (c, Vec2::new(w.width(), w.height()))));
    // Rebuild the card only when what it describes changed, and a few
    // times a second for live numbers; texts are touched only on change.
    // The pinned card already shows the inspected thing under the cursor.
    let on_pinned = *tool == ActiveTool::Inspect
        && highlight.selected.is_some()
        && highlight.hovered.is_some()
        && highlight.selected == highlight.hovered;
    // Over the interface the buttons' own tooltips speak (a path in
    // progress keeps its corners; the card comes back over the map).
    let hidden = ui.menu_open || over_ui.0 || on_pinned;
    let key = (*tool, *modes, *kind, highlight.hovered.clone(), hidden, sim.generation);
    cache.age += time.delta_secs();
    if cache.key.as_ref() != Some(&key) || preview.is_changed() || state.is_changed() || cache.age >= CARD_REFRESH_S {
        cache.key = Some(key);
        cache.age = 0.0;
        cache.card = if hidden {
            None
        } else {
            tool_card(
                *tool,
                *modes,
                *kind,
                &state,
                &opts,
                &sim.world,
                highlight.hovered.as_ref(),
            )
        };
    }
    let (Some(card), Some((cursor, win))) = (cache.card.clone(), cursor) else {
        vis.set_if_neq(Visibility::Hidden);
        cache.shown = None;
        *slide = 0.0;
        return;
    };
    if cache.shown.as_ref() != Some(&card) {
        apply_card(&card, &mut parts, 0);
        cache.shown = Some(card.clone());
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
    // Keep clear of the toolbar and, for building tools, its options bar.
    let options_bar = if *tool == ActiveTool::Inspect { 0.0 } else { 52.0 };
    if pos.y + size.y > screen.y - t::EDGE - t::TOOL_BUTTON - 2.0 * t::EDGE - options_bar {
        pos.y = cursor.y - off.y * 0.6 - size.y;
    }
    pos = pos.max(Vec2::splat(t::EDGE));
    let (left, top) = (px(pos.x.round()), px(pos.y.round()));
    if node.left != left || node.top != top {
        node.left = left;
        node.top = top;
    }
    if *slide < 1.0 {
        *slide += (1.0 - *slide) * approach(time.delta_secs(), 0.05);
        if *slide > 0.995 {
            *slide = 1.0;
        }
        tf.translation = Val2::px(0.0, (1.0 - *slide) * 5.0);
    }
    if size.x > 0.0 {
        vis.set_if_neq(Visibility::Inherited);
    }
}

type CardKey = (ActiveTool, ToolModes, RoadChoice, Option<WorldTarget>, bool, u64);

/// Card for a pinned target and the world point it hangs from (sim m).
fn pinned_card(world: &World, target: &WorldTarget) -> Option<(Card, [f32; 2])> {
    match target {
        WorldTarget::Building(id) => {
            let b = world.buildings.get(*id)?;
            Some((building_card(world, *id, b)?, b.center()))
        }
        WorldTarget::Junction { node, pos } => Some((junction_card(world, *node)?, *pos)),
        WorldTarget::Road(stretch) => {
            let mid = *stretch.points.get(stretch.points.len() / 2)?;
            Some((road_card(world, &stretch.segments)?, mid))
        }
        WorldTarget::Vehicle(v) => {
            let veh = world.vehicles.get(*v)?;
            let (pos, _) = citysim_sim::drive::vehicle_pose(world, veh);
            Some((vehicle_card(world, *v)?, pos))
        }
    }
}

/// The inspected (clicked) building, road or junction keeps a card next to
/// it, following it on screen, until Escape or a click elsewhere.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_pinned(
    time: Res<Time<Real>>,
    highlight: Res<WorldHighlight>,
    tool: Res<ActiveTool>,
    ui: Res<UiState>,
    sim: Res<SimWorld>,
    ui_scale: Res<UiScale>,
    window: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<crate::shared::WorldCamera>>,
    mut root: Query<(&mut Node, &mut Visibility, &ComputedNode, &CardRoot), Without<CardPart>>,
    mut parts: CardParts,
    mut cache: Local<(f32, Option<Card>, Option<WorldTarget>)>,
) {
    let Some((mut node, mut vis, computed, _)) = root.iter_mut().find(|r| *r.3 == CardRoot(1)) else {
        return;
    };
    let target = highlight
        .selected
        .clone()
        .filter(|_| !ui.menu_open && *tool == ActiveTool::Inspect);
    let Some(target) = target else {
        vis.set_if_neq(Visibility::Hidden);
        cache.1 = None;
        return;
    };
    cache.0 += time.delta_secs();
    let Some((mut card, at)) = pinned_card(&sim.world, &target) else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    card.hint = "Pinned · Esc or click elsewhere to let go".into();
    if cache.2.as_ref() != Some(&target) || cache.0 >= CARD_REFRESH_S || cache.1.is_none() {
        cache.0 = 0.0;
        cache.2 = Some(target);
        if cache.1.as_ref() != Some(&card) {
            apply_card(&card, &mut parts, 1);
            cache.1 = Some(card);
        }
    }
    let (Ok(win), Ok((cam, cam_tf))) = (window.single(), cameras.single()) else {
        return;
    };
    let Ok(screen_pt) = cam.world_to_viewport(cam_tf, crate::shared::sim_to_world(at, 3.0)) else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    let scale = ui_scale.0.max(0.01);
    let p = screen_pt / scale;
    let screen = Vec2::new(win.width(), win.height()) / scale;
    let size = computed.size * computed.inverse_scale_factor;
    let mut pos = Vec2::new(p.x + 18.0, p.y - size.y - 18.0);
    if pos.x + size.x > screen.x - t::EDGE {
        pos.x = p.x - 18.0 - size.x;
    }
    let top = t::BELOW_TOP_BAR;
    let bottom = screen.y - t::EDGE - t::TOOL_BUTTON - 3.0 * t::EDGE;
    pos.y = pos.y.clamp(top, (bottom - size.y).max(top));
    pos.x = pos.x.clamp(t::EDGE, (screen.x - t::EDGE - size.x).max(t::EDGE));
    let (left, topv) = (px(pos.x.round()), px(pos.y.round()));
    if node.left != left || node.top != topv {
        node.left = left;
        node.top = topv;
    }
    if size.x > 0.0 {
        vis.set_if_neq(Visibility::Inherited);
    }
}

/// Last card built by `update_card` and what it was built from.
#[derive(Default)]
struct CardCache {
    key: Option<CardKey>,
    age: f32,
    card: Option<Card>,
    /// Card currently written into the entities.
    shown: Option<Card>,
}

/// Seconds between refreshes of the live numbers on a card.
const CARD_REFRESH_S: f32 = 0.25;

type CardParts<'w, 's> = Query<
    'w,
    's,
    (
        &'static CardPart,
        &'static CardOwner,
        Option<&'static mut Text>,
        Option<&'static mut TextColor>,
        Option<&'static mut BackgroundColor>,
        Option<&'static mut Node>,
    ),
    Without<CardRoot>,
>;

fn apply_card(card: &Card, parts: &mut CardParts, owner: u8) {
    for (part, of, text, color, bg, pnode) in parts.iter_mut() {
        if of.0 != owner {
            continue;
        }
        match *part {
            CardPart::IconBox => {
                if let Some(mut bg) = bg {
                    bg.set_if_neq(BackgroundColor(t::alpha(card.color, 0.16)));
                }
            }
            CardPart::Icon => set_text(text, color, card.glyph.to_string(), card.color),
            CardPart::Title => set_text(text, color, card.title.clone(), card.title_color),
            CardPart::Row(i) => set_display(pnode, i < card.rows.len()),
            CardPart::RowLabel(i) => {
                if let Some(r) = card.rows.get(i) {
                    set_display(pnode, !r.label.is_empty());
                    set_text(text, color, r.label.clone(), t::TEXT_MUTED);
                }
            }
            CardPart::RowValue(i) => {
                if let Some(r) = card.rows.get(i) {
                    set_text(text, color, r.value.clone(), r.color);
                }
            }
            CardPart::Hint => {
                let show = !card.hint.is_empty();
                set_text(text, color, card.hint.clone(), t::TEXT_FAINT);
                set_display(pnode, show);
            }
            CardPart::Bar => set_display(pnode, card.bar.is_some() && !card.rows.is_empty()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use citysim_sim::{Cell, Command};

    #[test]
    fn bands_follow_the_shared_thresholds() {
        assert_eq!(Band::of(0.0), Band::Free);
        assert_eq!(Band::of(CONGESTION_BUSY - 0.01), Band::Free);
        assert_eq!(Band::of(CONGESTION_BUSY), Band::Busy);
        assert_eq!(Band::of(CONGESTION_JAM), Band::Jammed);
        assert_eq!(Band::of(1.0), Band::Jammed);
    }

    #[test]
    fn headings() {
        assert_eq!(heading_name([0.0, 0.0], [8.0, 0.0]), "Eastbound");
        assert_eq!(heading_name([0.0, 0.0], [0.0, -8.0]), "Northbound");
        assert_eq!(heading_name([0.0, 0.0], [-8.0, 8.0]), "Southwest");
    }

    #[test]
    fn insufficient_funds_names_both_amounts() {
        let e = CommandError::InsufficientFunds {
            cost: 1_250,
            money: -40,
        };
        assert_eq!(error_text(&e), "Not enough money: costs $1,250, you have \u{2212}$40");
    }

    #[test]
    fn road_card_reads_the_sim() {
        let mut w = World::new(1);
        w.apply(Command::BuildRoad {
            from: Cell::new(11, 128),
            to: Cell::new(30, 128),
            kind: citysim_sim::RoadKind::Street,
        })
        .unwrap();
        let t = super::super::tools::target_at(&w, Cell::new(20, 128).center(), 4.0).expect("road");
        let card = inspect_card(&w, &t).expect("road card");
        assert_eq!(card.title, "Street · 1 lane each way · 152 m");
        assert_eq!(card.rows[0].value, "Free flow");
        assert!(card.rows.iter().any(|r| r.label.starts_with("Eastbound")));
        assert!(card.rows.iter().any(|r| r.label == "Westbound"));
    }

    #[test]
    fn cut_off_countdown_follows_the_hourly_check() {
        let grace = balance::ABANDON_UNREACHABLE_GRACE_TICKS;
        let hour = balance::REACHABILITY_CHECK_INTERVAL_TICKS;
        // Found cut off at an hourly check: exactly one day to go.
        assert_eq!(abandon_in_ticks(10 * hour, 10 * hour), grace);
        assert_eq!(hours_text(abandon_in_ticks(10 * hour, 10 * hour)), "24 h");
        assert_eq!(hours_text(abandon_in_ticks(10 * hour, 10 * hour + grace - 5)), "1 h");
    }
}
