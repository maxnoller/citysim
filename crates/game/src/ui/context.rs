//! The cursor readout: a small tag beside the cursor with what the next
//! click does, what it costs and the snap that applied (red with the sim's
//! reason when it is refused); with Inspect, the hovered building, road or
//! car with live numbers. Also the card builders the side panel and the
//! junction tray use (`inspect_card`, `junction_card`).

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
use super::plate::{PlateAnim, PlateStyle};
use super::readout::{self, Readout, Symbol, Tone};
use super::theme::{self as t, UiFonts, icon};
use super::tools::{ToolOptions, ToolState};
use super::widgets::{approach, figure, label, layout, plate_node, zone_icon};
use crate::shared::{
    ActiveTool, CONGESTION_BUSY, CONGESTION_JAM, GameSet, PointerOverUi, RoadChoice, RoadMode, SimWorld, ToolModes,
    ToolPreview, WorldHighlight, WorldTarget,
};

pub struct ContextPlugin;

impl Plugin for ContextPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_tag)
            .add_systems(Update, update_tag.in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct TagRoot;

#[derive(Component)]
struct TagPlate;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum TagPart {
    Title,
    Price,
    /// One small line under the title.
    Line(usize),
    Hint,
}

/// Lines the tag shows under its title.
const TAG_LINES: usize = 4;

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
        CommandError::CannotRestore(p) => match p {
            citysim_sim::RestoreProblem::LandChanged => "the land there has changed since".into(),
            citysim_sim::RestoreProblem::BuiltOn => "another building stands there now".into(),
            citysim_sim::RestoreProblem::Rezoned => "the lot isn't zoned for it any more".into(),
            citysim_sim::RestoreProblem::NoRoad => "no road reaches the lot any more".into(),
        },
        CommandError::Roundabout(p) => ring_problem_text(p),
    }
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
        Tone::Accent => t::ACCENT_TEXT,
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
            // A roundabout size picked: what that ring would be here.
            if let Some(size) = opts.ring_brush {
                let mut card = Card::new(icon::WAYPOINTS, t::ACCENT);
                let ways = world.roads.nodes.get(*node).map_or(0, |n| n.segments.len());
                card.title = format!("{} · {ways} ways", ring_name(size));
                let now = junction::junction(world, *node).and_then(|j| j.ring);
                if let Some(now) = now {
                    card.rows.push(Row::new("Now", ring_name(now), t::TEXT_MUTED));
                }
                match state.ring_preview.as_ref().filter(|p| p.0.0 == *node && p.0.1 == size) {
                    Some((_, Ok(p))) => {
                        card.rows.push(Row::new("Price", format::money(p.cost), t::TEXT));
                        card.rows.push(Row::new(
                            "Upkeep",
                            format!("{} a day", format::money(size.upkeep_per_day())),
                            t::TEXT_MUTED,
                        ));
                        if !p.buildings.is_empty() {
                            card.rows.push(Row::new(
                                "Takes down",
                                format!(
                                    "{} building{}",
                                    p.buildings.len(),
                                    if p.buildings.len() == 1 { "" } else { "s" }
                                ),
                                t::WARNING,
                            ));
                        }
                        card.hint = "Click: build it · Ctrl+Z: undo".into();
                    }
                    Some((_, Err(e))) => {
                        card.title_color = t::NEGATIVE;
                        card.color = t::NEGATIVE;
                        card.rows.push(Row::line(error_text(e), t::NEGATIVE));
                        card.hint = "Pick another size, or another junction".into();
                    }
                    None => {}
                }
                card.rows.truncate(MAX_ROWS);
                return Some(card);
            }
            // The tray shows the phases and queues; the tag stays short.
            let mut card = junction_card(world, *node)?;
            card.rows.truncate(1);
            if junction::junction(world, *node).is_some_and(|j| j.ring.is_some()) {
                let s = opts.junction_brush.map_or("an ordinary junction".to_string(), |b| {
                    format!("{}", setting_name(b).to_lowercase())
                });
                card.hint = format!("Click: back to {s}");
                return Some(card);
            }
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
            readout::bulldoze(plan, world, modes.bulldoze).map(|r| readout_card(&r))
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
    let (title, lines) = super::people::vehicle_lines(world, id)?;
    let truck = world
        .traffic
        .trips
        .get(id)
        .is_some_and(|t| t.purpose == citysim_sim::TripPurpose::Goods);
    let mut card = Card::new(if truck { icon::TRUCK } else { icon::CAR }, t::ACCENT);
    card.title = title;
    for l in lines.iter().take(MAX_ROWS - 1) {
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
        // "30 cars, room for 26 · jammed"; on two lanes "(12 + 18)" after
        // the count, kerb lane first.
        let n = s.load[d].round() as u64;
        let mut count = format!("{} {}", format::group(n), if n == 1 { "car" } else { "cars" });
        let lanes = s.lane_count() as usize;
        if lanes > 1 {
            let id = ids.iter().copied().find(|&i| roads.segments.get(i) == Some(s));
            let per = id
                .and_then(|i| lane_cars.get(&(i, forward)).copied())
                .unwrap_or_default();
            let parts: Vec<String> = (0..lanes.min(4)).map(|l| per[l].to_string()).collect();
            count = format!("{count} ({})", parts.join(" + "));
        }
        let value = format!(
            "{count}, room for {} · {}",
            format::group(s.capacity[d].floor().max(1.0) as u64),
            b.word()
        );
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
        JunctionControl::Roundabout => "Roundabout",
    }
}

/// "Roundabout, medium".
pub fn ring_name(s: citysim_sim::RoundaboutSize) -> String {
    format!("Roundabout, {}", s.name())
}

/// Why a roundabout does not fit, in the readout's words.
pub fn ring_problem_text(p: &citysim_sim::RingProblem) -> String {
    use citysim_sim::RingProblem as P;
    match p {
        P::TooFewArms => "A roundabout needs three or more roads".into(),
        P::TooManyArms => "Too many roads meet here for a roundabout".into(),
        P::Highway => "Not where the highway meets".into(),
        P::AvenueOnMini => "Too small for an avenue: pick a bigger size".into(),
        P::ArmsTooClose { degrees, .. } => format!("Roads meet at {degrees:.0}°: too sharp for this size"),
        P::NeighbourTooClose { .. } => "Too close to the next junction for this size".into(),
        P::RoadInTheWay { .. } => "Another road passes through the ring".into(),
        P::OffMap => "Too close to the edge of the map".into(),
        P::AlreadyThatSize => "Already this roundabout".into(),
        P::NotARoundabout => "Not a roundabout".into(),
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
        match state.ring {
            Some(r) => ring_name(r),
            None if signal => "Traffic lights".into(),
            None => "Priority junction".into(),
        },
        view.approaches.len()
    );
    card.rows.push(Row::new(
        "Control",
        match (state.ring, view.setting) {
            (Some(r), _) => format!(
                "Give way to the ring · {} m across",
                (r.dims().outer_r * 2.0).round() as i64
            ),
            (None, JunctionSetting::Auto) => format!("{} · automatic", control_name(view.control)),
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
        card.rows.push(Row::new("Cycle", names.join(" › "), t::TEXT_MUTED));
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

fn tag_style(refused: bool, demolish: bool) -> PlateStyle {
    if refused {
        PlateStyle::fill(t::NEGATIVE_FILL_TOP, t::NEGATIVE_FILL_BOTTOM, t::NEGATIVE)
    } else if demolish {
        // The bulldozer's card: a warning-red edge (a click destroys).
        PlateStyle::fill(t::PLATE_BOTTOM, t::PLATE_BOTTOM, t::NEGATIVE)
    } else {
        PlateStyle::fill(t::PLATE_BOTTOM, t::PLATE_BOTTOM, t::ACCENT)
    }
}

fn spawn_tag(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Cursor readout"),
            TagRoot,
            layout(Node {
                position_type: PositionType::Absolute,
                ..default()
            }),
            GlobalZIndex(90),
            UiTransform::default(),
            Visibility::Hidden,
        ))
        .with_children(|r| {
            r.spawn((
                TagPlate,
                plate_node(
                    Node {
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::new(px(12), px(14), px(5), px(7)),
                        row_gap: px(2),
                        max_width: px(420),
                        ..default()
                    },
                    UiRect::left(px(4)),
                    [0.0, 0.0, 9.0, 0.0],
                ),
                PlateAnim::new(tag_style(false, false)),
                Pickable::IGNORE,
            ))
            .with_children(|p| {
                p.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(10),
                    ..default()
                }))
                .with_children(|top| {
                    top.spawn((
                        Text::new(""),
                        fonts.text(13.0, t::W_SEMIBOLD),
                        t::caps_spacing(13.0, 0.04),
                        TextColor(t::TEXT),
                        TextLayout::no_wrap(),
                        Pickable::IGNORE,
                        TagPart::Title,
                    ));
                    top.spawn((figure(fonts, "", 16.0, t::ACCENT_TEXT), TagPart::Price));
                });
                for i in 0..TAG_LINES {
                    p.spawn((
                        label(fonts, "", 12.0, t::W_MEDIUM, t::TEXT_MUTED),
                        TagPart::Line(i),
                        Node {
                            display: Display::None,
                            ..default()
                        },
                    ));
                }
                p.spawn((
                    label(fonts, "", 11.0, t::W_MEDIUM, t::TEXT_FAINT),
                    TagPart::Hint,
                    Node {
                        display: Display::None,
                        margin: UiRect::top(px(1)),
                        ..default()
                    },
                ));
            });
        });
}

/// What the tag shows: title, price, lines (text, colour), hint, refused.
#[derive(Debug, Clone, PartialEq)]
struct Tag {
    title: String,
    price: String,
    lines: Vec<(String, Color)>,
    hint: String,
    refused: bool,
    /// The bulldozer's card (red edge).
    demolish: bool,
}

/// A card as a tag: the price row moves next to the title, short rows
/// read as "label value", the rest as sentences.
fn tag_of(card: &Card) -> Tag {
    let mut price = String::new();
    let mut lines = Vec::new();
    let mut title = card.title.clone();
    let mut measured = false;
    for r in &card.rows {
        // The road's length and tightest bend join the title, as in
        // "Avenue · curve · 212 m · radius 64 m".
        if r.label == "Length" {
            title = format!("{title} · {}", r.value.split(" · ").next().unwrap_or_default());
            measured = true;
            continue;
        }
        if r.label == "Tightest bend" {
            let rad = r.value.split(" · ").next().unwrap_or_default();
            title = format!("{title} · {rad}");
            if r.color != t::TEXT {
                lines.push((r.value.clone(), r.color));
            }
            continue;
        }
        if r.label == "Price" && price.is_empty() && r.color != t::TEXT_MUTED {
            price = r.value.split(" · ").next().unwrap_or_default().to_string();
            if let Some(rest) = r.value.split_once(" · ") {
                lines.push((rest.1.to_string(), t::TEXT_MUTED));
            }
            continue;
        }
        let text = if r.label.is_empty() {
            r.value.clone()
        } else {
            format!("{} · {}", r.label, r.value)
        };
        let color = if r.color == t::TEXT { t::SLOT_INK } else { r.color };
        lines.push((text, color));
    }
    // Refusals and snaps first: they matter most at the cursor.
    lines.sort_by_key(|(_, c)| {
        if *c == t::NEGATIVE {
            0
        } else if *c == t::ACCENT_TEXT {
            1
        } else {
            2
        }
    });
    lines.truncate(TAG_LINES);
    Tag {
        title,
        price,
        lines,
        // How to use the tool, until there is something to measure.
        hint: if measured { String::new() } else { card.hint.clone() },
        refused: card.title_color == t::NEGATIVE,
        demolish: false,
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_tag(
    time: Res<Time<Real>>,
    tool: Res<ActiveTool>,
    (modes, kind): (Res<ToolModes>, Res<RoadChoice>),
    state: Res<ToolState>,
    (preview, opts, over_ui, ui): (Res<ToolPreview>, Res<ToolOptions>, Res<PointerOverUi>, Res<UiState>),
    (sim, ui_scale): (Res<SimWorld>, Res<UiScale>),
    window: Query<&Window, With<PrimaryWindow>>,
    mut root: Query<(&mut Node, &mut Visibility, &mut UiTransform), With<TagRoot>>,
    plate: Query<&ComputedNode, With<TagPlate>>,
    mut plate_anim: Query<&mut PlateAnim, With<TagPlate>>,
    mut parts: Query<(&TagPart, &mut Text, &mut TextColor, &mut Node), Without<TagRoot>>,
    (highlight, panel): (Res<WorldHighlight>, Res<super::panel::PanelArea>),
    (extent, window_area): (Res<super::tray::TrayExtent>, Res<super::city::WindowArea>),
    mut slide: Local<f32>,
    mut cache: Local<TagCache>,
) {
    let Ok((mut node, mut vis, mut tf)) = root.single_mut() else {
        return;
    };
    let cursor = window
        .single()
        .ok()
        .and_then(|w| w.cursor_position().map(|c| (c, Vec2::new(w.width(), w.height()))));
    // The side panel already shows what is selected.
    let on_selected = *tool == ActiveTool::Inspect
        && highlight.selected.is_some()
        && highlight.hovered.is_some()
        && highlight.selected == highlight.hovered;
    let hidden = ui.menu_open || over_ui.0 || on_selected;
    let key = (*tool, *modes, *kind, highlight.hovered.clone(), hidden, sim.generation);
    cache.age += time.delta_secs();
    if cache.key.as_ref() != Some(&key) || preview.is_changed() || state.is_changed() || cache.age >= CARD_REFRESH_S {
        cache.key = Some(key);
        cache.age = 0.0;
        cache.tag = if hidden {
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
            .map(|c| Tag {
                demolish: *tool == ActiveTool::Bulldoze,
                ..tag_of(&c)
            })
        };
    }
    let (Some(tag), Some((cursor, win))) = (cache.tag.clone(), cursor) else {
        vis.set_if_neq(Visibility::Hidden);
        cache.shown = None;
        *slide = 0.0;
        return;
    };
    if cache.shown.as_ref() != Some(&tag) {
        if let Ok(mut a) = plate_anim.single_mut() {
            let st = tag_style(tag.refused, tag.demolish);
            if a.style != st {
                a.style = st;
            }
        }
        for (part, mut text, mut color, mut n) in &mut parts {
            let (v, c, show) = match *part {
                TagPart::Title => (tag.title.clone(), t::TEXT, true),
                TagPart::Price => (
                    tag.price.clone(),
                    if tag.refused { t::NEGATIVE } else { t::ACCENT_TEXT },
                    !tag.price.is_empty(),
                ),
                TagPart::Line(i) => match tag.lines.get(i) {
                    Some((l, c)) => (
                        l.clone(),
                        if tag.refused && *c == t::NEGATIVE { t::TEXT } else { *c },
                        true,
                    ),
                    None => (String::new(), t::TEXT_MUTED, false),
                },
                TagPart::Hint => (tag.hint.clone(), t::TEXT_FAINT, !tag.hint.is_empty()),
            };
            if text.0 != v {
                text.0 = v;
            }
            color.set_if_neq(TextColor(c));
            let d = if show { Display::Flex } else { Display::None };
            if n.display != d {
                n.display = d;
            }
        }
        cache.shown = Some(tag.clone());
    }
    // Beside the cursor, flipping to stay inside the window and clear of
    // the bar and its tray.
    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let screen = win / scale;
    let size = plate
        .single()
        .map(|c| c.size * c.inverse_scale_factor)
        .unwrap_or(Vec2::ZERO);
    let off = Vec2::new(22.0, 20.0);
    let mut pos = cursor + off;
    if pos.x + size.x > screen.x - 8.0 {
        pos.x = cursor.x - off.x - size.x;
    }
    // Above the drawer standing on the bar (whatever drawer is up).
    let floor = screen.y - t::BAR_H - extent.0 - 8.0;
    if pos.y + size.y > floor {
        pos.y = cursor.y - off.y * 0.6 - size.y;
    }
    // Clear of the side panel.
    if let Some(r) = panel.0
        && pos.x + size.x > r.min.x - 8.0
        && pos.y < r.max.y
        && pos.y + size.y > r.min.y
    {
        pos.x = (cursor.x - off.x - size.x).min(r.min.x - 8.0 - size.x);
    }
    pos = pos.max(Vec2::new(8.0, 64.0));
    // Never over the city window or a drawer: flip to the other side of the
    // cursor, and if it still would not fit, don't show it.
    let tag = |p: Vec2| Rect::from_corners(p, p + size);
    let blocked = |p: Vec2| {
        window_area.logical.is_some_and(|w| !w.intersect(tag(p)).is_empty())
            || (extent.0 > 0.0 && p.y + size.y > floor + 1.0)
    };
    if blocked(pos) {
        let flips = [
            Vec2::new(cursor.x - off.x - size.x, pos.y),
            Vec2::new(pos.x, cursor.y - off.y * 0.6 - size.y),
            Vec2::new(cursor.x - off.x - size.x, cursor.y - off.y * 0.6 - size.y),
        ];
        match flips
            .into_iter()
            .map(|p| p.max(Vec2::new(8.0, 64.0)))
            .find(|p| !blocked(*p))
        {
            Some(p) => pos = p,
            None => {
                vis.set_if_neq(Visibility::Hidden);
                return;
            }
        }
    }
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

/// Last tag built by `update_tag` and what it was built from.
#[derive(Default)]
struct TagCache {
    key: Option<CardKey>,
    age: f32,
    tag: Option<Tag>,
    /// Tag currently written into the entities.
    shown: Option<Tag>,
}

/// Seconds between refreshes of the live numbers on the tag.
const CARD_REFRESH_S: f32 = 0.25;

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
