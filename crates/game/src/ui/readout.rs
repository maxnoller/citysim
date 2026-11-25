//! What the tools say at the cursor, as plain data (no colours, no
//! layout): length, angle to the previous piece, tightest bend, price,
//! what the point snapped to, why the simulation refuses, how many zoning
//! cells change. A card, a tooltip or a side panel can show it.

use citysim_sim::{Curve, RoadKind, Zone};

use super::format;
use super::fsnap::{Snapped, turn_deg};
use super::shapes::tightest;
use super::tools::{Pending, ToolPlan};
use crate::shared::{RoadMode, SnapTarget, ZoneMode};

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
            Some(m) => format!("parallel, {} from the road", metres(m)),
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
pub fn upgrade(plan: &ToolPlan, dragging: bool) -> Option<Readout> {
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

/// The bulldozer.
pub fn bulldoze(plan: &ToolPlan) -> Option<Readout> {
    let Pending::Bulldoze { hint, building, .. } = &plan.pending else {
        return None;
    };
    let mut lines = Vec::new();
    let removed: f32 = plan.eval.removed().iter().map(|(c, _)| c.length()).sum();
    let houses = plan.eval.buildings().len();
    if let Some(h) = hint {
        lines.push(Line::text(*h, Tone::Muted));
    }
    if removed > 0.0 {
        lines.push(Line::new("Road", metres(removed), Tone::Normal));
    }
    if houses > 0 {
        lines.push(Line::new(
            "Buildings",
            format!("{} demolished", format::group(houses as u64)),
            Tone::Warn,
        ));
    }
    if hint.is_none() && removed == 0.0 && houses == 0 {
        lines.push(Line::text("Nothing to remove here", Tone::Muted));
    }
    if plan.eval.cost > 0 {
        lines.push(Line::new("Demolition", format::money(plan.eval.cost), Tone::Normal));
    }
    if let Some(e) = plan.error() {
        lines.push(Line::text(super::context::error_text(e), Tone::Bad));
    }
    let title = if building.is_some() {
        "Demolish building"
    } else {
        "Bulldoze"
    };
    Some(Readout {
        symbol: Symbol::Bulldoze,
        title: title.into(),
        refused: plan.error().is_some(),
        lines,
        hint: "Click a road or building · drag along a road for a part · Ctrl+Z undoes".into(),
    })
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
