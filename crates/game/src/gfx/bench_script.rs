//! Scripted runs of the windowless benchmark: `CITYSIM_BENCH_SCRIPT=quality|actions`.
//!
//! * `quality`: the camera holds one view over the city while the graphics
//!   quality is switched ten times (High, Low, Medium, High, Medium, Low,
//!   High, Low, Medium, High... every pair of levels both ways), waiting
//!   each time until the switch has been applied (`scene::QualityApplied`)
//!   and one more second. Reported per switch: frames and milliseconds until
//!   it applied, the worst frame from the request until a second after it,
//!   and how many frames went over 33 / 50 ms.
//! * `actions`: during the benchmark's camera path, a fixed list of what a
//!   player does, through real `Command`s and the same resources and keys
//!   the interface uses: build a street out of the city, zone along it, put
//!   a junction on signals, show each overlay the first time, turn day to
//!   night and back, open the city window (keys U, P, Esc: needs the `ui`
//!   feature; `CITYSIM_BENCH_UI=1` draws it), select a car (route ribbon),
//!   the top speed for a while, a fast zoom from street level to the whole
//!   map and back, save and load (keys F5, F9; only when `CITYSIM_SAVE_DIR`
//!   is set, so a player's quick save is never touched), and bulldoze the
//!   street again. Every action owns the frames up to the next one; the
//!   report lists each with its worst frame. What the simulation does by
//!   itself (buildings appearing or levelling up, junctions turning to
//!   signals) is tagged on the frames it happens in.

use bevy::prelude::*;
use citysim_sim::junction::{JunctionControl, JunctionSetting, junction_nodes, junction_view};
use citysim_sim::{Command, Curve, RoadKind, World, Zone};

use crate::shared::{CameraPose, GraphicsQuality, OverlayMode, WorldTarget};

/// Which script runs (`CITYSIM_BENCH_SCRIPT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptKind {
    Quality,
    Actions,
}

impl ScriptKind {
    pub fn from_env() -> Option<ScriptKind> {
        match std::env::var("CITYSIM_BENCH_SCRIPT").as_deref() {
            Ok("quality") => Some(ScriptKind::Quality),
            Ok("actions") => Some(ScriptKind::Actions),
            Ok("" | "0") | Err(_) => None,
            Ok(other) => {
                warn!("unknown CITYSIM_BENCH_SCRIPT={other:?} (quality or actions)");
                None
            }
        }
    }
}

/// The quality levels the `quality` script switches to, in order (from
/// High): every level to every other one, both ways.
pub const QUALITY_ORDER: [GraphicsQuality; 10] = [
    GraphicsQuality::Low,
    GraphicsQuality::Medium,
    GraphicsQuality::High,
    GraphicsQuality::Medium,
    GraphicsQuality::Low,
    GraphicsQuality::High,
    GraphicsQuality::Low,
    GraphicsQuality::Medium,
    GraphicsQuality::High,
    GraphicsQuality::Low,
];

/// How many switches the `quality` script makes (`CITYSIM_BENCH_SWITCHES`,
/// at most and by default all ten).
pub fn switches() -> usize {
    std::env::var("CITYSIM_BENCH_SWITCHES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(QUALITY_ORDER.len())
        .min(QUALITY_ORDER.len())
}

/// Frames to keep measuring after a quality switch has applied.
pub const AFTER_SWITCH: u32 = 60;

/// One quality switch of the `quality` script.
#[derive(Debug, Clone)]
pub struct Switch {
    pub from: GraphicsQuality,
    pub to: GraphicsQuality,
    /// Index into the frame records of the request.
    pub start: usize,
    /// Frames and milliseconds from the request until it applied.
    pub applied: Option<(usize, f64)>,
    /// Index into the frame records where its window ends.
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Act {
    BuildRoad,
    ZoneRoad,
    Signals,
    Overlay(OverlayMode),
    /// Animate the drawn hour by this many hours over `HOUR_FRAMES`.
    Hours(f32),
    Unpin,
    Key(KeyCode),
    SelectCar,
    Deselect,
    Speed(usize),
    Zoom,
    Save,
    Load,
    RemoveRoad,
    End,
}

/// Frames a day-night transition takes.
pub const HOUR_FRAMES: u32 = 150;
/// Frames of the fast zoom (out, then back in).
pub const ZOOM_FRAMES: u32 = 120;

/// The `actions` script: (frame, name, action).
pub const ACTIONS: [(u32, &str, Act); 22] = [
    (0, "idle (camera path only)", Act::Speed(0)),
    (120, "build a street", Act::BuildRoad),
    (240, "paint zones", Act::ZoneRoad),
    (360, "junction to signals", Act::Signals),
    (480, "traffic overlay (first time)", Act::Overlay(OverlayMode::Traffic)),
    (580, "zones overlay (first time)", Act::Overlay(OverlayMode::Zones)),
    (680, "overlay off", Act::Overlay(OverlayMode::None)),
    (760, "day to night", Act::Hours(12.0)),
    (940, "night to day", Act::Hours(12.0)),
    (1120, "back to the sim's hour", Act::Unpin),
    (1160, "open the budget (U)", Act::Key(KeyCode::KeyU)),
    (1240, "city people page (P)", Act::Key(KeyCode::KeyP)),
    (1320, "close the city window (Esc)", Act::Key(KeyCode::Escape)),
    (1380, "select a car (route)", Act::SelectCar),
    (1500, "deselect", Act::Deselect),
    (1560, "top speed (12x)", Act::Speed(3)),
    (1800, "back to 1x", Act::Speed(0)),
    (1860, "zoom street to map and back", Act::Zoom),
    (2040, "save (F5)", Act::Save),
    (2120, "load (F9)", Act::Load),
    (2240, "bulldoze the street", Act::RemoveRoad),
    (2360, "end", Act::End),
];

/// `CITYSIM_BENCH_SCRIPT_SCALE=f` (default 1): every frame count of the
/// `actions` script times `f` (below 1 to try it on a slow software renderer).
pub fn scaled(frames: u32) -> u32 {
    let f: f32 = std::env::var("CITYSIM_BENCH_SCRIPT_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    ((frames as f32 * f).round() as u32).max(1)
}

/// Frames the `actions` script measures.
pub fn actions_frames() -> u32 {
    scaled(ACTIONS[ACTIONS.len() - 1].0)
}

/// The street the script builds: start and end (sim metres).
#[derive(Debug, Clone, Copy)]
pub struct Street {
    pub a: [f32; 2],
    pub b: [f32; 2],
}

impl Street {
    fn dir(&self) -> Vec2 {
        (Vec2::from(self.b) - Vec2::from(self.a)).normalize_or_zero()
    }

    pub fn build(&self) -> Command {
        Command::BuildCurve {
            curve: Curve::straight(self.a, self.b),
            kind: RoadKind::Street,
        }
    }

    /// A quad along the street from `from` m past its start to its end,
    /// `half` m to each side.
    fn quad(&self, from: f32, beyond: f32, half: f32) -> [[f32; 2]; 4] {
        let d = self.dir();
        let n = d.perp() * half;
        let a = Vec2::from(self.a) + d * from;
        let b = Vec2::from(self.b) + d * beyond;
        [(a - n).into(), (b - n).into(), (b + n).into(), (a + n).into()]
    }

    pub fn zone(&self) -> Command {
        Command::ZoneArea {
            quad: self.quad(12.0, 0.0, 44.0),
            zone: Some(Zone::Residential),
        }
    }

    pub fn remove(&self) -> Command {
        Command::BulldozeArea {
            quad: self.quad(6.0, 2.0, 2.5),
        }
    }
}

/// A buildable street of 160 m leading out of the city from one of its
/// nodes (dead ends first, then the outermost nodes).
pub fn find_street(w: &World, centre: Vec2) -> Option<Street> {
    let mut nodes: Vec<(f32, [f32; 2], usize)> = w
        .roads
        .nodes
        .iter()
        .filter(|(id, _)| Some(*id) != w.roads.entry)
        .map(|(_, n)| (Vec2::from(n.pos).distance(centre), n.pos, n.segments.len()))
        .collect();
    nodes.sort_by(|a, b| (a.2 != 1).cmp(&(b.2 != 1)).then(b.0.total_cmp(&a.0)));
    for (_, pos, _) in nodes.iter().take(400) {
        let p = Vec2::from(*pos);
        let out = (p - centre).normalize_or(Vec2::X);
        for turn in [0.0f32, 0.5, -0.5, 1.0, -1.0, 1.57, -1.57] {
            let d = Vec2::from_angle(turn).rotate(out);
            let street = Street {
                a: *pos,
                b: (p + d * 160.0).into(),
            };
            if w.preview(&street.build()).is_ok() {
                return Some(street);
            }
        }
    }
    None
}

/// A junction to put on signals: the one nearest the centre that has none.
pub fn find_junction(w: &World, centre: Vec2) -> Option<citysim_sim::NodeId> {
    let mut best: Option<(f32, citysim_sim::NodeId)> = None;
    for node in junction_nodes(w) {
        let Some(view) = junction_view(w, node) else { continue };
        if view.control == JunctionControl::Signal || view.setting != JunctionSetting::Auto {
            continue;
        }
        let Some(n) = w.roads.nodes.get(node) else { continue };
        let d = Vec2::from(n.pos).distance(centre);
        if best.is_none_or(|(b, _)| d < b) {
            best = Some((d, node));
        }
    }
    best.map(|(_, n)| n)
}

/// A car on the road with a trip ahead, nearest the centre.
pub fn find_car(w: &World, centre: Vec2) -> Option<WorldTarget> {
    w.vehicles
        .iter()
        .filter(|(id, _)| w.traffic.trips.get(*id).is_some())
        .min_by(|a, b| {
            Vec2::from(a.1.pos)
                .distance(centre)
                .total_cmp(&Vec2::from(b.1.pos).distance(centre))
        })
        .map(|(id, _)| WorldTarget::Vehicle(id))
}

/// The fast zoom at `t` in 0..1: from street level over `focus` out to the
/// whole map and back.
pub fn zoom_pose(t: f32, focus: Vec2, start: &CameraPose) -> CameraPose {
    let k = 1.0 - (2.0 * t - 1.0).abs();
    let k = k * k * (3.0 - 2.0 * k);
    let near = 60.0f32;
    let far = citysim_sim::MAP_SIZE_M * 1.1;
    CameraPose {
        focus: Vec3::new(focus.x, 0.0, focus.y),
        yaw: start.yaw,
        pitch: 0.45 + 0.6 * k,
        distance: (near.ln() + (far.ln() - near.ln()) * k).exp(),
    }
}
