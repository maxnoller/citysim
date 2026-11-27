//! What the tools say at the cursor, as plain data (no colours, no
//! layout): length, angle to the previous piece, tightest bend, price,
//! what the point snapped to, why the simulation refuses, how many zoning
//! cells change. A card, a tooltip or a side panel can show it.

use citysim_sim::{Curve, RoadKind, World, Zone};

use super::format;
use super::fsnap::{Snapped, turn_deg};
use super::shapes::tightest;
use super::tools::{Pending, ToolPlan};
use crate::shared::{BulldozeMode, RoadMode, SnapTarget, ZoneMode};

/// How a line should read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Muted,
    Accent,
    Good,
    Warn,
    Bad,
}

/// One label / value line ("" label: the value is a plain sentence).
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub label: String,
    pub value: String,
    pub tone: Tone,
}

impl Line {
    pub fn new(label: impl Into<String>, value: impl Into<String>, tone: Tone) -> Line {
        Line {
            label: label.into(),
            value: value.into(),
            tone,
        }
    }

    pub fn text(value: impl Into<String>, tone: Tone) -> Line {
        Line::new("", value, tone)
    }
}

/// Which symbol goes with a readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    Road,
    Curve,
    Grid,
    Upgrade,
    Zone(Option<Zone>),
    Bulldoze,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Readout {
    pub symbol: Symbol,
    pub title: String,
    /// The action would be refused.
    pub refused: bool,
    pub lines: Vec<Line>,
    pub hint: String,
}

pub fn kind_name(k: RoadKind) -> &'static str {
    match k {
        RoadKind::Street => "Street",
        RoadKind::Avenue => "Avenue",
        RoadKind::Highway => "Highway",
    }
}

pub fn metres(m: f32) -> String {
    if m >= 1000.0 {
        format!("{:.2} km", m / 1000.0)
    } else {
        format!("{} m", m.round() as i64)
    }
}

/// Price and daily upkeep.
fn price_line(cost: i64, upkeep_per_day: f64) -> Line {
    let up = upkeep_per_day.round() as i64;
    let value = match (cost > 0, up > 0) {
        (true, true) => format!("{} · upkeep {}/day", format::money(cost), format::money(up)),
        (true, false) => format::money(cost),
        (false, true) => format!("free · upkeep {}/day", format::money(up)),
        (false, false) => "free".into(),
    };
    Line::new("Price", value, Tone::Normal)
}

/// A demolition this large in one action gets a stronger warning: more
/// than this many buildings, or more people (residents and jobs).
pub const LARGE_LOSS_BUILDINGS: usize = 10;
pub const LARGE_LOSS_PEOPLE: usize = 100;

pub fn large_loss(buildings: usize, people: usize) -> bool {
    buildings > LARGE_LOSS_BUILDINGS || people > LARGE_LOSS_PEOPLE
}

/// What taking these buildings down means, in the readout's words:
/// "Demolishes 3 homes, 1 shop", "11 residents move out · 4 jobs go", and
/// for a large loss a warning line. `cost`: what the demolition adds to a
/// road's price (shown with the buildings when not zero).
fn loss_lines(world: &World, ids: &[citysim_sim::BuildingId], cost: Option<i64>) -> Vec<Line> {
    let mut lines = Vec::new();
    if ids.is_empty() {
        return lines;
    }
    let mut kinds = [0u32; 3];
    let (mut residents, mut jobs) = (0u32, 0u32);
    for b in ids.iter().filter_map(|b| world.buildings.get(*b)) {
        let i = match b.kind {
            Zone::Residential => 0,
            Zone::Commercial => 1,
            Zone::Industrial => 2,
        };
        kinds[i] += 1;
        if b.kind == Zone::Residential {
            residents += b.occupied;
        } else {
            jobs += b.occupied;
        }
    }
    let words: Vec<String> = [("home", "homes"), ("shop", "shops"), ("works", "works")]
        .iter()
        .zip(kinds)
        .filter(|(_, n)| *n > 0)
        .map(|((one, many), n)| format!("{n} {}", if n == 1 { one } else { many }))
        .collect();
    let mut value = words.join(", ");
    if let Some(c) = cost.filter(|&c| c > 0) {
        value = format!("{value} · {}", format::money(c));
    }
    lines.push(Line::new("Demolishes", value, Tone::Warn));
    let mut who = Vec::new();
    if residents > 0 {
        who.push(format!(
            "{} {} out",
            format::group(residents as u64),
            if residents == 1 {
                "resident moves"
            } else {
                "residents move"
            }
        ));
    }
    if jobs > 0 {
        who.push(format!(
            "{} {}",
            format::group(jobs as u64),
            if jobs == 1 { "job goes" } else { "jobs go" }
        ));
    }
    if !who.is_empty() {
        lines.push(Line::text(who.join(" · "), Tone::Warn));
    }
    let people = (residents + jobs) as usize;
    if large_loss(ids.len(), people) {
        lines.push(Line::text(
            format!(
                "Large demolition: {} buildings, {} people · Ctrl+Z puts them back",
                ids.len(),
                format::group(people as u64)
            ),
            Tone::Bad,
        ));
    }
    lines
}

/// "turns 30° left" (sim axes: positive = right).
pub fn turn_words(deg: f32) -> String {
    let a = deg.abs().round() as i64;
    if a == 0 {
        "straight on".into()
    } else if a == 180 {
        "turns back".into()
    } else {
        format!("turns {a}° {}", if deg > 0.0 { "right" } else { "left" })
    }
}

/// What the live point snapped to, in the player's words.
pub fn snap_words(s: &Snapped, suspended: bool) -> Option<String> {
    if suspended {
        return Some("Snapping off (Alt)".into());
    }
    let has = |t: SnapTarget| s.matched.contains(&t);
    let mut parts: Vec<String> = Vec::new();
    match s.target {
        SnapTarget::Node => parts.push("joins the road end / junction".into()),
        SnapTarget::Road => parts.push("makes a junction on the road".into()),
        SnapTarget::Extension => parts.push("continues a road's line".into()),
        SnapTarget::Perpendicular => parts.push("straight on / at right angles".into()),
        SnapTarget::Parallel => parts.push(match s.spacing_m {
            Some(m) => {
                let rows = super::fsnap::rows_between(m);
                let n = rows.round() as i32;
                if (rows - n as f32).abs() < 0.01 && n >= 2 {
                    // Whole rows of lots between the kerbs, back to back.
                    let lots = if n % 2 == 0 {
                        format!("2 × {} rows of lots", n / 2)
                    } else {
                        format!("{n} rows of lots")
                    };
                    format!("parallel, {} from the road · {lots}", metres(m))
                } else {
                    format!("parallel, {} from the road", metres(m))
                }
            }
            None => "parallel to the road".into(),
        }),
        SnapTarget::Angle => parts.push("15° step".into()),
        SnapTarget::Grid => parts.push("lined up with the lots".into()),
        SnapTarget::Length | SnapTarget::Spacing | SnapTarget::None => {}
    }
    if s.target != SnapTarget::Road && has(SnapTarget::Road) {
        parts.push("on the road".into());
    }
    if s.target != SnapTarget::Extension && has(SnapTarget::Extension) {
        parts.push("on a road's line".into());
    }
    if s.target != SnapTarget::Perpendicular && has(SnapTarget::Perpendicular) {
        parts.push("right angle".into());
    }
    if has(SnapTarget::Length) {
        parts.push("whole cells".into());
    }
    if parts.is_empty() {
        return None;
    }
    let mut out = parts.join(" · ");
    if let Some(f) = out.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    Some(out)
}

/// The road tool: what the road being drawn would be.
pub fn road(
    world: &World,
    mode: RoadMode,
    plan: &ToolPlan,
    live: Option<&Snapped>,
    start_road_dir: Option<[f32; 2]>,
    suspended: bool,
) -> Option<Readout> {
    let Pending::Road { curves, kind, grid, .. } = &plan.pending else {
        return None;
    };
    let mut lines = Vec::new();
    let symbol = match mode {
        RoadMode::Curve | RoadMode::Continuous => Symbol::Curve,
        RoadMode::Grid => Symbol::Grid,
        _ => Symbol::Road,
    };
    let what = match mode {
        RoadMode::Straight => "straight",
        RoadMode::Curve => "curve",
        RoadMode::Continuous => "continuous",
        _ => "",
    };
    let title = match grid {
        Some(g) => format!("{} grid · {} x {} blocks", kind_name(*kind), g.nu, g.nv.max(1)),
        None if curves.len() > 1 => format!("{} · {what} · {} pieces", kind_name(*kind), curves.len()),
        None => format!("{} · {what}", kind_name(*kind)),
    };
    let total: f32 = curves.iter().map(Curve::length).sum();
    if let Some(g) = grid {
        lines.push(Line::new(
            "Size",
            format!("{} x {}", metres(g.width_m()), metres(g.depth_m())),
            Tone::Normal,
        ));
        lines.push(Line::new("Streets", format!("{} of road", metres(total)), Tone::Normal));
    } else {
        let mut v = metres(total);
        if curves.len() > 1
            && let Some(last) = curves.last()
        {
            v = format!("{v} · this piece {}", metres(last.length()));
        }
        lines.push(Line::new("Length", v, Tone::Normal));
        // Angle to the previous piece, or to the road it starts from.
        let angle = match curves.as_slice() {
            [.., a, b] => Some(turn_words(turn_deg(a.end_dir(), b.start_dir()))),
            [only] => start_road_dir.map(|d| {
                let deg = turn_deg(d, only.start_dir()).abs();
                let deg = if deg > 90.0 { 180.0 - deg } else { deg };
                format!("{}° to the road it leaves", deg.round() as i64)
            }),
            _ => None,
        };
        if let Some(a) = angle {
            lines.push(Line::new("Angle", a, Tone::Normal));
        }
        if let Some((_, _, r, _)) = tightest(curves) {
            let min = kind.min_radius_m();
            let tone = if r < min {
                Tone::Bad
            } else if r < min * 1.5 {
                Tone::Warn
            } else {
                Tone::Normal
            };
            lines.push(Line::new(
                "Tightest bend",
                format!("radius {} · needs {}", metres(r), metres(min)),
                tone,
            ));
        }
    }
    let error = plan.error();
    if error.is_some() {
        // Nothing is built: what it would cost as drawn.
        let cost = (total as f64 * kind.cost_per_m()).round() as i64;
        lines.push(Line::new(
            "Price",
            format!("about {} if it could be built", format::money(cost)),
            Tone::Muted,
        ));
    } else {
        let upkeep = plan.eval.length_m as f64 * kind.upkeep_per_m();
        lines.push(price_line(plan.eval.cost, upkeep));
    }
    // The grown buildings in the road's way go with it.
    lines.extend(loss_lines(world, &plan.eval.buildings(), Some(demolition_cost(plan))));
    if let Some(s) = live.and_then(|s| snap_words(s, suspended)) {
        lines.push(Line::text(s, Tone::Accent));
    }
    if let Some(e) = error {
        lines.push(Line::text(super::context::error_text(e), Tone::Bad));
    }
    let hint = match mode {
        RoadMode::Grid => "Click a corner, the width, then the length · [ ] block size · Esc".to_string(),
        RoadMode::Curve => "Click start, corner, end · Enter or double-click builds · Backspace · Esc".to_string(),
        _ => "Click to add points · Enter or double-click builds · Backspace · Esc".to_string(),
    };
    Some(Readout {
        symbol,
        title,
        refused: error.is_some(),
        lines,
        hint,
    })
}

/// What the plan's demolitions add to its price.
fn demolition_cost(plan: &ToolPlan) -> i64 {
    plan.eval
        .steps
        .iter()
        .filter_map(|s| s.result.as_ref().ok())
        .map(|p| p.demolition_cost)
        .sum()
}

/// The road tool before the first point: what a click would start.
pub fn road_idle(mode: RoadMode, kind: RoadKind, live: Option<&Snapped>, block_m: f32) -> Readout {
    let (symbol, title, hint) = match mode {
        RoadMode::Curve => (
            Symbol::Curve,
            format!("{} · curve", kind_name(kind)),
            "Click the start, the corner where the directions meet, then the end",
        ),
        RoadMode::Continuous => (
            Symbol::Curve,
            format!("{} · continuous", kind_name(kind)),
            "Click along the way: each piece leaves the last one smoothly",
        ),
        RoadMode::Grid => (
            Symbol::Grid,
            format!("{} grid · blocks of {}", kind_name(kind), metres(block_m)),
            "Click a corner, then the width, then the length",
        ),
        _ => (
            Symbol::Road,
            format!("{} · straight", kind_name(kind)),
            "Click (or drag) from point to point; Enter builds",
        ),
    };
    let mut lines = vec![Line::new(
        "Price",
        format!(
            "{} per 8 m · upkeep {}/day",
            format::money(kind.cost_per_cell()),
            format::money(kind.upkeep_per_cell())
        ),
        Tone::Muted,
    )];
    if let Some(s) = live.and_then(|s| snap_words(s, false)) {
        lines.push(Line::text(s, Tone::Accent));
    }
    Readout {
        symbol,
        title,
        refused: false,
        lines,
        hint: hint.into(),
    }
}

/// The upgrade mode.
pub fn upgrade(world: &World, plan: &ToolPlan, dragging: bool) -> Option<Readout> {
    let Pending::Upgrade { kind, hint, ops, .. } = &plan.pending else {
        return None;
    };
    let mut lines = Vec::new();
    let error = plan.error();
    if let Some(h) = hint {
        lines.push(Line::text(*h, Tone::Muted));
    } else {
        let len: f32 = ops
            .iter()
            .filter_map(|o| match o {
                super::plan::Op::Kind(c, _) => Some(c.length()),
                _ => None,
            })
            .sum();
        lines.push(Line::new("Length", metres(len), Tone::Normal));
        lines.push(price_line(plan.eval.cost, len as f64 * kind.upkeep_per_m()));
        // What the change does to the daily bill (the other kind is what
        // is there now).
        let other = if *kind == RoadKind::Avenue {
            RoadKind::Street
        } else {
            RoadKind::Avenue
        };
        let delta = (len as f64 * (kind.upkeep_per_m() - other.upkeep_per_m())).round() as i64;
        if delta != 0 && len > 0.0 {
            lines.push(Line::text(
                format!("{}/day upkeep against now", format::signed_money(delta)),
                Tone::Muted,
            ));
        }
        lines.extend(loss_lines(world, &plan.eval.buildings(), Some(demolition_cost(plan))));
        if let Some(e) = error {
            lines.push(Line::text(super::context::error_text(e), Tone::Bad));
        }
    }
    let title = if *kind == RoadKind::Avenue {
        "Upgrade to avenue"
    } else {
        "Change to street"
    };
    Some(Readout {
        symbol: Symbol::Upgrade,
        title: title.into(),
        refused: error.is_some(),
        lines,
        hint: if dragging {
            "Release to change this part".into()
        } else {
            "Click for the stretch between junctions · drag along it for a part".into()
        },
    })
}

/// Zoning modes: how many cells change.
pub fn zone(plan: &ToolPlan, mode: ZoneMode, dragging: bool) -> Option<Readout> {
    let Pending::Zone { zone, hint, .. } = &plan.pending else {
        return None;
    };
    let name = super::widgets::zone_name(*zone);
    let title = match zone {
        Some(_) => format!("Zone {}", name.to_lowercase()),
        None => "Remove zoning".into(),
    };
    let mut lines = Vec::new();
    let n = plan.eval.zone_cells().len();
    if let Some(h) = hint {
        lines.push(Line::text(*h, Tone::Muted));
    } else if n == 0 {
        lines.push(Line::text(
            if zone.is_some() {
                "Already zoned, built on or not beside a road"
            } else {
                "Nothing zoned here"
            },
            Tone::Muted,
        ));
    } else {
        let what = if zone.is_some() { "zoned" } else { "unzoned" };
        lines.push(Line::new(
            "Cells",
            format!("{} {what} · free", format::group(n as u64)),
            Tone::Normal,
        ));
    }
    let hint = match mode {
        ZoneMode::Paint if dragging => "Release to zone what you painted",
        ZoneMode::Paint => "Drag over the lots to paint them",
        ZoneMode::Frontage => "Click a side of a road · drag along it for a part",
        ZoneMode::Fill => "Click to fill the land around the cursor",
        ZoneMode::Marquee => "Drag a rectangle along the lots",
    };
    Some(Readout {
        symbol: Symbol::Zone(*zone),
        title,
        refused: false,
        lines,
        hint: hint.into(),
    })
}

/// The bulldozer: what a click removes, what it costs, what it saves, and
/// who it affects (the cursor card is the tool's only readout).
pub fn bulldoze(plan: &ToolPlan, world: &World, mode: BulldozeMode) -> Option<Readout> {
    let Pending::Bulldoze { hint, building, .. } = &plan.pending else {
        return None;
    };
    let mut lines = Vec::new();
    let removed = plan.eval.removed();
    let road: f32 = removed.iter().map(|(c, _)| c.length()).sum();
    let upkeep: f64 = removed.iter().map(|(c, k)| c.length() as f64 * k.upkeep_per_m()).sum();
    let houses = plan.eval.buildings();
    if let Some(h) = hint {
        lines.push(Line::text(*h, Tone::Muted));
    }
    if road > 0.0 {
        // "Length" joins the card's title ("Demolish road · 120 m").
        lines.push(Line::new("Length", metres(road), Tone::Normal));
    }
    lines.extend(loss_lines(world, &houses, None));
    if hint.is_none() && road == 0.0 && houses.is_empty() {
        lines.push(Line::text("Nothing to remove here", Tone::Muted));
    }
    if road > 0.0 || !houses.is_empty() {
        lines.push(Line::new(
            "Price",
            if plan.eval.cost > 0 {
                format!("{} · no refund", format::money(plan.eval.cost))
            } else {
                "Free · no refund".into()
            },
            Tone::Accent,
        ));
    }
    if upkeep >= 0.5 {
        lines.push(Line::new(
            "Upkeep saved",
            format!("{}/day", format::money(upkeep.round() as i64)),
            Tone::Good,
        ));
    }
    if let Some(e) = plan.error() {
        lines.push(Line::text(super::context::error_text(e), Tone::Bad));
    }
    let title = match (building.is_some(), mode) {
        (true, _) => "Demolish building",
        (false, BulldozeMode::Road) => "Demolish road",
        (false, BulldozeMode::Area) => "Demolish area",
    };
    Some(Readout {
        symbol: Symbol::Bulldoze,
        title: title.into(),
        refused: plan.error().is_some(),
        lines,
        hint: match mode {
            BulldozeMode::Road => "Click: a stretch or a building · drag along a road: a part · Tab: area",
            BulldozeMode::Area => "Drag a rectangle over roads and buildings · Tab: road or building",
        }
        .into(),
    })
}

/// One big figure of a build tray: caption, value, tone.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    pub caption: &'static str,
    pub value: String,
    pub tone: Tone,
}

impl Figure {
    fn new(caption: &'static str, value: impl Into<String>, tone: Tone) -> Figure {
        Figure {
            caption,
            value: value.into(),
            tone,
        }
    }
}

const DASH: &str = "\u{2014}";

/// The three live figures of the road tray (length, tightest radius, cost;
/// a grid's size and blocks; an upgrade's upkeep).
pub fn road_figures(mode: RoadMode, plan: &ToolPlan) -> [Figure; 3] {
    let refused = plan.error().is_some();
    let cost_tone = if refused { Tone::Bad } else { Tone::Accent };
    match &plan.pending {
        Pending::Road { curves, kind, grid, .. } => {
            let total: f32 = curves.iter().map(Curve::length).sum();
            let cost = if refused {
                (total as f64 * kind.cost_per_m()).round() as i64
            } else {
                plan.eval.cost
            };
            if let Some(g) = grid {
                return [
                    Figure::new(
                        "Size",
                        format!("{}x{}", g.width_m().round() as i64, g.depth_m().round() as i64),
                        Tone::Normal,
                    ),
                    Figure::new("Blocks", format!("{}x{}", g.nu, g.nv.max(1)), Tone::Normal),
                    Figure::new("Cost", format::money(cost), cost_tone),
                ];
            }
            let radius = match tightest(curves) {
                Some((_, _, r, _)) => {
                    let tone = if r < kind.min_radius_m() {
                        Tone::Bad
                    } else {
                        Tone::Normal
                    };
                    Figure::new("Radius", metres(r), tone)
                }
                None if total > 0.0 => Figure::new("Radius", "straight", Tone::Muted),
                None => Figure::new("Radius", DASH, Tone::Muted),
            };
            [
                Figure::new(
                    "Length",
                    if total > 0.0 { metres(total) } else { DASH.into() },
                    Tone::Normal,
                ),
                radius,
                Figure::new(
                    "Cost",
                    if total > 0.0 { format::money(cost) } else { DASH.into() },
                    cost_tone,
                ),
            ]
        }
        Pending::Upgrade { ops, kind, .. } => {
            let len: f32 = ops
                .iter()
                .filter_map(|o| match o {
                    super::plan::Op::Kind(c, _) => Some(c.length()),
                    _ => None,
                })
                .sum();
            let up = (len as f64 * kind.upkeep_per_m()).round() as i64;
            [
                Figure::new(
                    "Length",
                    if len > 0.0 { metres(len) } else { DASH.into() },
                    Tone::Normal,
                ),
                Figure::new(
                    "Upkeep",
                    if len > 0.0 {
                        format!("{}/d", format::money(up))
                    } else {
                        DASH.into()
                    },
                    Tone::Normal,
                ),
                Figure::new(
                    "Cost",
                    if len > 0.0 {
                        format::money(plan.eval.cost)
                    } else {
                        DASH.into()
                    },
                    cost_tone,
                ),
            ]
        }
        _ => {
            let _ = mode;
            [
                Figure::new("Length", DASH, Tone::Muted),
                Figure::new("Radius", DASH, Tone::Muted),
                Figure::new("Cost", DASH, Tone::Muted),
            ]
        }
    }
}

/// The zone tray's figures: cells, area, price.
pub fn zone_figures(plan: &ToolPlan) -> [Figure; 3] {
    let n = match &plan.pending {
        Pending::Zone { .. } => plan.eval.zone_cells().len(),
        _ => 0,
    };
    let cell = citysim_sim::CELL_SIZE * citysim_sim::CELL_SIZE;
    [
        Figure::new(
            "Cells",
            if n > 0 { format::group(n as u64) } else { DASH.into() },
            Tone::Normal,
        ),
        Figure::new(
            "Area",
            if n > 0 {
                format!("{} m²", format::group((n as f32 * cell) as u64))
            } else {
                DASH.into()
            },
            Tone::Normal,
        ),
        Figure::new("Cost", "Free", Tone::Good),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_read_naturally() {
        assert_eq!(turn_words(0.2), "straight on");
        assert_eq!(turn_words(30.4), "turns 30° right");
        assert_eq!(turn_words(-90.0), "turns 90° left");
        assert_eq!(metres(1234.0), "1.23 km");
        assert_eq!(metres(64.4), "64 m");
    }

    #[test]
    fn snap_words_name_every_rule_that_agrees() {
        let s = Snapped {
            point: [0.0, 0.0],
            target: SnapTarget::Perpendicular,
            matched: vec![SnapTarget::Perpendicular, SnapTarget::Extension, SnapTarget::Length],
            guides: Vec::new(),
            on: None,
            spacing_m: None,
        };
        assert_eq!(
            snap_words(&s, false).unwrap(),
            "Straight on / at right angles · on a road's line · whole cells"
        );
        assert_eq!(snap_words(&s, true).unwrap(), "Snapping off (Alt)");
    }
}
