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
    /// Only the tool acts of `actions` ([`TOOL_ACTIONS`]).
    Tools,
}

impl ScriptKind {
    pub fn from_env() -> Option<ScriptKind> {
        match std::env::var("CITYSIM_BENCH_SCRIPT").as_deref() {
            Ok("quality") => Some(ScriptKind::Quality),
            Ok("actions") => Some(ScriptKind::Actions),
            Ok("tools") => Some(ScriptKind::Tools),
            Ok("" | "0") | Err(_) => None,
            Ok(other) => {
                warn!("unknown CITYSIM_BENCH_SCRIPT={other:?} (quality, actions or tools)");
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
    /// A step of the tool acts.
    Tool(ToolAct),
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

/// The acts a script plays, in order: (frame, name, action). `actions`:
/// [`ACTIONS`] up to its end, then the tool acts from there.
pub fn acts(kind: ScriptKind) -> Vec<(u32, &'static str, Act)> {
    let tools = TOOL_ACTIONS.iter().copied();
    match kind {
        ScriptKind::Quality => Vec::new(),
        ScriptKind::Tools => tools.collect(),
        ScriptKind::Actions => {
            let base = ACTIONS[ACTIONS.len() - 1].0;
            ACTIONS[..ACTIONS.len() - 1]
                .iter()
                .copied()
                .chain(tools.map(|(f, n, a)| (base + f, n, a)))
                .collect()
        }
    }
}

/// `CITYSIM_BENCH_SCRIPT_SCALE=f` (default 1): every frame count of the
/// `actions` script times `f` (below 1 to try it on a slow software renderer).
pub fn scaled(frames: u32) -> u32 {
    let f: f32 = std::env::var("CITYSIM_BENCH_SCRIPT_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    ((frames as f32 * f).round() as u32).max(1)
}

/// Frames the `actions` / `tools` script measures.
pub fn actions_frames(kind: ScriptKind) -> u32 {
    scaled(acts(kind).last().map_or(0, |a| a.0))
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

// ------------------------------------------------------------ tool acts

/// The tool acts (`CITYSIM_BENCH_SCRIPT=tools`, and the end of `actions`):
/// large pending actions of the real tools, driven like a player through
/// the shared tool resources, the pointer (`ScriptPointer`) and the mouse
/// buttons and keys: a street grid (`CITYSIM_BENCH_GRID=8x13` blocks of
/// 40 m by default) drawn in the free land beside the city, pending while
/// the camera pans and zooms (the pointer still, as when the player moves
/// the camera with the keys) and while the pointer moves, then built and
/// undone; a chain of a dozen curves, the same; the zone tool's fill and
/// marquee over the city, applied and undone; the area bulldozer dragged
/// over a district, applied and undone. Frames relative to the start of the
/// tool acts.
pub const TOOL_ACTIONS: [(u32, &str, Act); 20] = [
    (0, "tools: camera over the free land", Act::Tool(ToolAct::Setup)),
    (30, "grid: corner and width (clicks)", Act::Tool(ToolAct::GridClicks)),
    (70, "grid pending: camera pans and zooms", Act::Tool(ToolAct::Pan)),
    (370, "grid pending: pointer moves", Act::Tool(ToolAct::Wiggle)),
    (490, "build the grid (click)", Act::Tool(ToolAct::Click)),
    (640, "undo the grid (Ctrl+Z)", Act::Tool(ToolAct::Undo)),
    (790, "chain of curves: placing points", Act::Tool(ToolAct::ChainClicks)),
    (950, "chain pending: camera pans and zooms", Act::Tool(ToolAct::Pan)),
    (1250, "build the chain (Enter)", Act::Tool(ToolAct::Enter)),
    (1400, "undo the chain (Ctrl+Z)", Act::Tool(ToolAct::Undo)),
    (1520, "zone fill pending: pointer moves", Act::Tool(ToolAct::Fill)),
    (1640, "zone fill (click)", Act::Tool(ToolAct::Click)),
    (1760, "undo the zoning (Ctrl+Z)", Act::Tool(ToolAct::Undo)),
    (1880, "zone marquee: dragging", Act::Tool(ToolAct::Marquee)),
    (2000, "zone marquee (release)", Act::Tool(ToolAct::Release)),
    (2120, "undo the zoning (Ctrl+Z)", Act::Tool(ToolAct::Undo)),
    (
        2240,
        "area bulldozer: dragging over a district",
        Act::Tool(ToolAct::Bulldoze),
    ),
    (2360, "bulldoze the district (release)", Act::Tool(ToolAct::Release)),
    (2480, "undo the demolition (Ctrl+Z)", Act::Tool(ToolAct::Undo)),
    (2600, "end", Act::End),
];

/// One step of the tool acts (see [`TOOL_ACTIONS`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolAct {
    /// Camera over the site, the road tool in grid mode.
    Setup,
    /// Click the grid's corner, then its width; the pointer to its depth.
    GridClicks,
    /// The camera pans in a loop and zooms in and out, the pointer still.
    Pan,
    /// The camera still, the pointer circles round where it was.
    Wiggle,
    /// A click where the pointer is.
    Click,
    Undo,
    /// The road tool in chain mode, a click on each point of a long chain;
    /// the pointer on to the next.
    ChainClicks,
    Enter,
    /// The zone tool's fill mode over the city, the pointer moving.
    Fill,
    /// The zone tool's marquee: press, drag over the city.
    Marquee,
    /// The area bulldozer: press, drag over a district.
    Bulldoze,
    /// Let go of the button.
    Release,
}

/// `CITYSIM_BENCH_GRID=NUxNV`: the grid's blocks (default 8 x 13, the
/// owner's grid).
pub fn grid_blocks() -> (u32, u32) {
    std::env::var("CITYSIM_BENCH_GRID")
        .ok()
        .and_then(|s| {
            let (a, b) = s.split_once('x')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((8, 13))
}

/// Money the city is given before the tool acts (if it has less).
pub const TOOL_MONEY: i64 = 5_000_000;

/// Grid block, metres (the road tool's default block size).
pub const GRID_BLOCK_M: f32 = 40.0;

/// Where the tool acts work: the grid in free land beside the city (its
/// corner, unit sides, blocks along each), the chain's points, and points
/// in the city for zoning and the bulldozer.
#[derive(Debug, Clone)]
pub struct ToolSite {
    pub a: Vec2,
    pub u: Vec2,
    pub v: Vec2,
    pub nu: u32,
    pub nv: u32,
    pub chain: Vec<Vec2>,
    /// A zoned spot near the centre (fill, marquee start) and the
    /// marquee's other corner.
    pub zone_at: Vec2,
    pub zone_to: Vec2,
    /// The district the bulldozer is dragged over (corners).
    pub district: (Vec2, Vec2),
}

impl ToolSite {
    pub fn at(&self, i: f32, j: f32) -> Vec2 {
        self.a + self.u * i * GRID_BLOCK_M + self.v * j * GRID_BLOCK_M
    }

    pub fn centre(&self) -> Vec2 {
        self.at(self.nu as f32 * 0.5, self.nv as f32 * 0.5)
    }

    /// A camera pose over the whole grid.
    pub fn pose(&self) -> CameraPose {
        let c = self.centre();
        let extent = (self.nu.max(self.nv) as f32 * GRID_BLOCK_M).max(200.0);
        CameraPose {
            focus: Vec3::new(c.x, 0.0, c.y),
            yaw: 0.35,
            pitch: 0.95,
            distance: extent * 2.1,
        }
    }

    fn streets(&self) -> Vec<Curve> {
        let mut out = Vec::new();
        for j in 0..=self.nv {
            let s = self.at(0.0, j as f32);
            out.push(Curve::straight(s.into(), self.at(self.nu as f32, j as f32).into()));
        }
        for i in 0..=self.nu {
            let s = self.at(i as f32, 0.0);
            out.push(Curve::straight(s.into(), self.at(i as f32, self.nv as f32).into()));
        }
        out
    }
}

/// A copy of the parts of `w` road commands use (as the road tool's
/// scratch copy), to try commands on.
fn scratch(w: &World) -> World {
    let mut s = World::new(w.seed);
    s.roads = w.roads.clone();
    s.zoning = w.zoning.clone();
    s.buildings = w.buildings.clone();
    s.growth = w.growth.clone();
    s.economy = w.economy.clone();
    s.time = w.time.clone();
    s.versions = w.versions.clone();
    s.versions.roads += 1;
    s.versions.grid += 1;
    s.versions.buildings += 1;
    s.economy.money = s.economy.money.max(100_000_000);
    s
}

/// Find the tool acts' site: a grid of `nu` x `nv` blocks every street of
/// which can be built, in the free land on one side of the city (tried
/// east, north, south, west, a few cells further out each time).
pub fn find_site(w: &World, centre: Vec2, nu: u32, nv: u32) -> Option<ToolSite> {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for n in w.roads.nodes.values() {
        lo = lo.min(Vec2::from(n.pos));
        hi = hi.max(Vec2::from(n.pos));
    }
    let map = citysim_sim::MAP_SIZE_M;
    let (wu, wv) = (nu as f32 * GRID_BLOCK_M, nv as f32 * GRID_BLOCK_M);
    // (corner, u, v): the grid's first street runs along the city's edge.
    let mid = centre;
    let sides = [
        (Vec2::new(hi.x, mid.y - wu * 0.5), Vec2::Y, Vec2::X),
        (Vec2::new(mid.x - wu * 0.5, lo.y), Vec2::X, Vec2::NEG_Y),
        (Vec2::new(mid.x - wu * 0.5, hi.y), Vec2::X, Vec2::Y),
        (Vec2::new(lo.x, mid.y - wu * 0.5), Vec2::Y, Vec2::NEG_X),
    ];
    let snap = |p: Vec2| (p / citysim_sim::CELL_SIZE).round() * citysim_sim::CELL_SIZE;
    for (a, u, v) in sides {
        for step in [1.0f32, 2.0, 4.0, 8.0] {
            let a = snap(a + v * step * citysim_sim::CELL_SIZE);
            let far = a + u * wu + v * wv;
            let inside = |p: Vec2| p.x > 16.0 && p.y > 16.0 && p.x < map - 16.0 && p.y < map - 16.0;
            if !inside(a) || !inside(far) || !inside(a + u * wu) || !inside(a + v * wv) {
                continue;
            }
            let mut site = ToolSite {
                a,
                u,
                v,
                nu,
                nv,
                chain: Vec::new(),
                zone_at: centre,
                zone_to: centre,
                district: (centre, centre),
            };
            let mut s = scratch(w);
            let ok = site.streets().into_iter().all(|curve| {
                s.apply(Command::BuildCurve {
                    curve,
                    kind: RoadKind::Street,
                })
                .is_ok()
            });
            if !ok {
                continue;
            }
            // The chain: a dozen pieces of 40 m winding gently across the
            // same land (along its long side, turning up to 20 degrees).
            let mut p = site.at(nu as f32 * 0.5, 0.5);
            site.chain = vec![p];
            for k in 0..12 {
                let heading = 0.35 * (k as f32 * 0.8).sin();
                let d = (site.v * heading.cos() + site.u * heading.sin()).normalize();
                p += d * (nv as f32 - 1.0) * GRID_BLOCK_M / 12.0;
                site.chain.push(p);
            }
            // Zoning and the bulldozer: in the city near its centre.
            let usable = |p: [f32; 2]| {
                w.zoning
                    .cell_at(p)
                    .filter(|&c| w.zoning.cell(c).is_some_and(|x| x.usable() && x.zone.is_none()))
            };
            let mut best: Option<(f32, Vec2)> = None;
            for r in 0..40 {
                for k in 0..16 {
                    let p = centre + Vec2::from_angle(k as f32 * 0.39) * r as f32 * 8.0;
                    if usable(p.into()).is_some() && best.is_none_or(|b| b.0 > p.distance(centre)) {
                        best = Some((p.distance(centre), p));
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            let z = best.map_or(centre, |b| b.1);
            site.zone_at = z;
            site.zone_to = z + Vec2::new(90.0, 60.0);
            site.district = (centre - Vec2::new(110.0, 80.0), centre + Vec2::new(110.0, 80.0));
            return Some(site);
        }
    }
    None
}
