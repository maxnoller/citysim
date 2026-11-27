//! The tool state machine. Reads the mouse, keys and the ground point
//! under the cursor; writes `ActiveTool`, `ToolModes`, `RoadChoice`, the
//! previews gfx draws (`CurveToolView`, `ToolPreview`, `SnapFeedback`,
//! `WorldHighlight`) and `SimCommand`s. The maths lives in plain modules:
//! snapping in `fsnap`, shapes and stretches in `shapes`, plans and undo in
//! `plan`, the words on the cursor card in `readout`.
//!
//! * Road (straight, curve, continuous, grid): click points, or press and
//!   drag (the release is the next point). Pieces chain into one road;
//!   Enter or a double-click builds it all as one action (a grid builds on
//!   its third point; a first straight piece dragged out builds at once).
//!   Backspace drops the last point; Esc or right-click steps back (the
//!   piece in progress, then the whole road); Alt holds snapping off.
//! * Road, upgrade: click a road for the stretch between junctions, or
//!   drag along it for a part; it becomes the chosen kind.
//! * Road, junctions: click a junction to switch its control.
//! * Zone: paint (drag over cells of any lattice), frontage (click a road
//!   side for the stretch between junctions; drag along the roads to follow
//!   them through bends, pieces and junctions, keeping to that side; depth
//!   1, 2, 4 rows or the full block), fill (click land enclosed by roads,
//!   even its bare middle), marquee (drag a rectangle turned to the cell it
//!   starts on; it takes the cells of every lattice inside). De-zoning is
//!   the same with no zone.
//! * Bulldoze: road (click a stretch or a building; drag along a road for a
//!   part), area (marquee).
//! * Tab cycles the current tool's modes, Ctrl+Z undoes (`plan::UndoRecord`).
//!
//! Keys: I inspect, R road (again: street / avenue), Z zone (cycles
//! R/C/I/de-zone), B bulldozer on / off (back to the tool before), J
//! junctions.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::command::PieceChange;
use citysim_sim::curve::right_of;
use citysim_sim::{
    BuildingId, CellRef, Command, CommandError, Curve, JunctionSetting, LatticeId, NodeId, RoadKind, World, Zone,
    balance, zoning,
};

use super::UiState;
use super::fsnap::{self, Anchor, OnNet, SnapOptions, Snapped, add, dist, dot, mul, sub};
use super::plan::{self, Evaluated, Op, UndoRecord};
use super::planner::{self, Done, Planner};
use super::shapes::{Draft, GridFrame, Placed, Shape, Stretch};
use super::toasts::{Toast, ToastKind};
use super::widgets::UiAction;
use crate::shared::{
    ActiveTool, BulldozeMode, CurveToolView, GameSet, HoveredCell, PointerOverUi, PreviewKind, RoadChoice, RoadMode,
    RoadStretch, ScriptPointer, SimClock, SimCommand, SimWorld, SnapFeedback, ToolModes, ToolPreview, WorldCamera,
    WorldHighlight, WorldTarget, ZoneMode,
};

// ---------------------------------------------------------------- options

/// Grid tool block sizes, cells between street centre lines (5 leaves two
/// rows of lots on each side).
pub const BLOCK_CELLS: [u32; 6] = [4, 5, 6, 7, 8, 10];
/// Parallel snap distances, metres between centre lines.
pub const PARALLEL_M: [f32; 6] = [24.0, 32.0, 40.0, 48.0, 56.0, 72.0];

/// A snapping rule the options bar can switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapRule {
    Nodes,
    Roads,
    Angles,
    Lengths,
    Lattice,
    Parallel,
    Extensions,
}

/// Tool options (the options bar).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct ToolOptions {
    pub snap: SnapOptions,
    /// Index into [`BLOCK_CELLS`].
    pub block: usize,
    /// Index into [`PARALLEL_M`].
    pub parallel: usize,
    /// Frontage zoning depth in cells (1..=`balance::BLOCK_DEPTH`).
    pub zone_depth: u8,
    /// What a click with the junction tool sets (None: the next setting).
    pub junction_brush: Option<JunctionSetting>,
}

impl Default for ToolOptions {
    fn default() -> Self {
        ToolOptions {
            snap: SnapOptions::default(),
            block: 1,
            parallel: 2,
            zone_depth: balance::BLOCK_DEPTH,
            junction_brush: None,
        }
    }
}

impl ToolOptions {
    pub fn rule(&self, r: SnapRule) -> bool {
        let s = &self.snap;
        match r {
            SnapRule::Nodes => s.nodes,
            SnapRule::Roads => s.roads,
            SnapRule::Angles => s.angles,
            SnapRule::Lengths => s.lengths,
            SnapRule::Lattice => s.lattice,
            SnapRule::Parallel => s.parallel,
            SnapRule::Extensions => s.extensions,
        }
    }

    pub fn toggle(&mut self, r: SnapRule) {
        let s = &mut self.snap;
        let f = match r {
            SnapRule::Nodes => &mut s.nodes,
            SnapRule::Roads => &mut s.roads,
            SnapRule::Angles => &mut s.angles,
            SnapRule::Lengths => &mut s.lengths,
            SnapRule::Lattice => &mut s.lattice,
            SnapRule::Parallel => &mut s.parallel,
            SnapRule::Extensions => &mut s.extensions,
        };
        *f = !*f;
    }

    pub fn block_cells(&self) -> u32 {
        BLOCK_CELLS[self.block.min(BLOCK_CELLS.len() - 1)]
    }

    pub fn block_m(&self) -> f32 {
        self.block_cells() as f32 * citysim_sim::CELL_SIZE
    }

    pub fn parallel_m(&self) -> f32 {
        PARALLEL_M[self.parallel.min(PARALLEL_M.len() - 1)]
    }

    pub fn step_block(&mut self, d: i32) {
        self.block = (self.block as i32 + d).clamp(0, BLOCK_CELLS.len() as i32 - 1) as usize;
    }

    pub fn step_parallel(&mut self, d: i32) {
        self.parallel = (self.parallel as i32 + d).clamp(0, PARALLEL_M.len() as i32 - 1) as usize;
        self.snap.parallel_m = self.parallel_m();
    }

    /// Snapping as the tools use it (Alt held: none).
    pub fn snapping(&self, off: bool) -> SnapOptions {
        if off {
            SnapOptions::none()
        } else {
            SnapOptions {
                parallel_m: self.parallel_m(),
                ..self.snap
            }
        }
    }
}

// ---------------------------------------------------------------- state

/// Max cells the fill tool zones at once.
const FILL_LIMIT: usize = 6000;
/// Double-click window, seconds.
const DOUBLE_CLICK_S: f32 = 0.4;
/// Undo steps kept.
const UNDO_DEPTH: usize = 16;
/// Screen pixels the snap radius spans.
const SNAP_PX: f32 = 18.0;

/// The left button went down on the map here and is still held.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Press {
    /// Ground point of the press (snapped for the road tools).
    pub at: [f32; 2],
    /// Real time of the press.
    pub time: f32,
    /// The pointer has moved away since (a drag).
    pub moved: bool,
    /// Lattice under the press (marquees follow it).
    pub lattice: LatticeId,
}

/// What the current tool would do right now.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Pending {
    #[default]
    None,
    /// New road (draft pieces and the live piece).
    Road {
        ops: Vec<Op>,
        curves: Vec<Curve>,
        kind: RoadKind,
        grid: Option<GridFrame>,
    },
    /// A change of kind along a stretch.
    Upgrade {
        ops: Vec<Op>,
        kind: RoadKind,
        /// The stretch pointed at, as a polyline.
        stretch: Vec<[f32; 2]>,
        hint: Option<&'static str>,
    },
    Zone {
        ops: Vec<Op>,
        zone: Option<Zone>,
        area: Option<[[f32; 2]; 4]>,
        stretch: Vec<[f32; 2]>,
        hint: Option<&'static str>,
    },
    Bulldoze {
        ops: Vec<Op>,
        area: Option<[[f32; 2]; 4]>,
        stretch: Vec<[f32; 2]>,
        building: Option<BuildingId>,
        hint: Option<&'static str>,
    },
}

impl Pending {
    pub fn ops(&self) -> &[Op] {
        match self {
            Pending::None => &[],
            Pending::Road { ops, .. }
            | Pending::Upgrade { ops, .. }
            | Pending::Zone { ops, .. }
            | Pending::Bulldoze { ops, .. } => ops,
        }
    }

    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Pending::Upgrade { hint, .. } | Pending::Zone { hint, .. } | Pending::Bulldoze { hint, .. } => *hint,
            _ => None,
        }
    }
}

/// The evaluated pending action (the last one checked: while a newer one
/// is being checked, `stale`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolPlan {
    pub pending: Pending,
    pub eval: Evaluated,
    /// A newer pending action is being checked; this is the one before.
    pub stale: bool,
    /// The road pieces and junctions of `eval` (`Evaluated::pieces` /
    /// `nodes`, worked out once per plan).
    pub pieces: Vec<(Curve, PieceChange)>,
    pub nodes: Vec<([f32; 2], citysim_sim::command::NodeChange)>,
}

impl ToolPlan {
    fn new(pending: Pending, eval: Evaluated) -> ToolPlan {
        ToolPlan {
            pieces: eval.pieces(),
            nodes: eval.nodes(),
            pending,
            eval,
            stale: false,
        }
    }

    pub fn error(&self) -> Option<&CommandError> {
        self.eval.first_error()
    }
}

type WorldKey = (u64, u64, u64, u64, i64);

fn world_key(sim: &SimWorld) -> WorldKey {
    let w = &sim.world;
    (
        sim.generation,
        w.versions.roads,
        w.versions.grid,
        w.versions.buildings,
        w.economy.money,
    )
}

#[derive(Resource, Default)]
pub struct ToolState {
    /// The road being drawn.
    pub draft: Draft,
    /// The snapped live point of the road tools.
    pub live: Option<Snapped>,
    /// Ground point under the cursor, sim metres (last one on the map).
    pub cursor: Option<[f32; 2]>,
    /// The cursor is over the map this frame.
    pub on_map: bool,
    /// Snap radius in metres (about 18 screen pixels).
    pub radius: f32,
    /// Snapping held off (Alt).
    pub snap_off: bool,
    pub press: Option<Press>,
    /// Paint tool: cells under the stroke so far.
    pub stroke: Vec<CellRef>,
    stroke_last: Option<[f32; 2]>,
    last_click: Option<(f32, [f32; 2])>,
    pub undo: Vec<UndoRecord>,
    /// The plan shown and the inputs it came from.
    pub plan: ToolPlan,
    plan_key: Option<(Pending, WorldKey)>,
    /// Real time the shown plan became stale.
    stale_since: Option<f32>,
    /// The request the frame has already waited for (it waits once).
    waited: Option<(Pending, WorldKey)>,
    /// Copy of the world the plans are checked on (`plan::scratch`), its
    /// id for the planner.
    scratch: Option<(WorldKey, u64, std::sync::Arc<World>)>,
    scratch_made: u64,
    /// Checks plans on a worker thread.
    planner: Planner,
    /// A click (or Enter, a release, Ctrl+Z) waiting for its plan to be
    /// checked before it is sent.
    job: Option<Job>,
    /// Commands of a plan being sent in parts, with the time each took to
    /// check (an estimate of what it costs to apply).
    outbox: std::collections::VecDeque<(Command, f32)>,
    /// A plan was sent this frame (its parts' results are summarised by
    /// the tool, not toasted one by one).
    pub batch_frame: bool,
}

/// An action waiting for its plan.
enum Job {
    /// Send the first of `cands` that is accepted (from `idx`), else
    /// report why the last one is not; then `then`.
    Commit {
        cands: Vec<Pending>,
        idx: usize,
        then: Then,
    },
    /// Take `rec` back with `ops` (buildings that cannot come back are
    /// left out once: `kept_out`).
    Undo {
        rec: UndoRecord,
        ops: Vec<Op>,
        kept_out: Vec<String>,
        filtered: bool,
    },
}

/// What happens to the tool once a plan has been sent (or refused).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Then {
    Nothing,
    /// The road is built: a new road; refused: a grid keeps its corner and
    /// width.
    Road,
}

/// Estimated cost (ms, the time to check them) of the commands sent per
/// frame when a plan is sent in parts.
const SEND_BUDGET_MS: f32 = 3.0;
/// Seconds a plan may be stale before the preview shows it dimmed.
const STALE_DIM_S: f32 = 0.15;

impl ToolState {
    /// Something can be taken back with Ctrl+Z.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn busy(&self) -> bool {
        !self.draft.is_empty() || self.press.is_some()
    }

    /// The scratch copy of the world now, and its id.
    fn scratch_for(&mut self, sim: &SimWorld) -> (u64, std::sync::Arc<World>) {
        let key = world_key(sim);
        if self.scratch.as_ref().is_none_or(|(k, _, _)| *k != key) {
            self.scratch_made += 1;
            self.scratch = Some((key, self.scratch_made, std::sync::Arc::new(plan::scratch(&sim.world))));
        }
        let (_, id, w) = self.scratch.as_ref().expect("just set");
        (*id, w.clone())
    }

    /// The pending action of the job waiting for its plan.
    fn job_pending(&self) -> Option<&Pending> {
        match &self.job {
            Some(Job::Commit { cands, idx, .. }) => cands.get(*idx),
            _ => None,
        }
    }

    /// The plan shown has been stale long enough to be shown so (a plan
    /// checked within a few frames is never dimmed).
    pub fn dimmed(&self, now: f32) -> bool {
        self.plan.stale && self.stale_since.is_some_and(|t| now - t >= STALE_DIM_S)
    }

    pub fn reset(&mut self) {
        let (shape, block_m) = (self.draft.shape, self.draft.block_m);
        self.draft = Draft::new(shape, block_m);
        self.press = None;
        self.stroke.clear();
        self.stroke_last = None;
        self.live = None;
    }
}

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolState>()
            .init_resource::<ToolOptions>()
            .add_systems(
                Update,
                (tool_keys, track_pointer, use_tool, publish)
                    .chain()
                    .in_set(GameSet::Tools),
            );
    }
}

/// Ctrl held, or pressed / released this frame (a fast Ctrl+Z can press
/// and release both keys within one slow frame).
pub fn ctrl(keys: &ButtonInput<KeyCode>) -> bool {
    let k = [KeyCode::ControlLeft, KeyCode::ControlRight];
    keys.any_pressed(k) || keys.any_just_pressed(k) || keys.any_just_released(k)
}

fn alt(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])
}

/// Next mode of the current tool (Tab).
pub fn next_mode(tool: ActiveTool, modes: ToolModes) -> ToolModes {
    let mut m = modes;
    match tool {
        ActiveTool::Road => {
            m.road = match modes.road {
                RoadMode::Straight => RoadMode::Curve,
                RoadMode::Curve => RoadMode::Continuous,
                RoadMode::Continuous => RoadMode::Grid,
                RoadMode::Grid => RoadMode::Upgrade,
                RoadMode::Upgrade => RoadMode::Junction,
                RoadMode::Junction => RoadMode::Straight,
            }
        }
        ActiveTool::Zone(_) => {
            m.zone = match modes.zone {
                ZoneMode::Paint => ZoneMode::Frontage,
                ZoneMode::Frontage => ZoneMode::Fill,
                ZoneMode::Fill => ZoneMode::Marquee,
                ZoneMode::Marquee => ZoneMode::Paint,
            }
        }
        ActiveTool::Bulldoze => {
            m.bulldoze = match modes.bulldoze {
                BulldozeMode::Road => BulldozeMode::Area,
                BulldozeMode::Area => BulldozeMode::Road,
            }
        }
        ActiveTool::Inspect => {}
    }
    m
}

pub fn shape_of(mode: RoadMode) -> Option<Shape> {
    match mode {
        RoadMode::Straight => Some(Shape::Straight),
        RoadMode::Curve => Some(Shape::Curve),
        RoadMode::Continuous => Some(Shape::Continuous),
        RoadMode::Grid => Some(Shape::Grid),
        _ => None,
    }
}

pub fn road_kind(kind: RoadChoice) -> RoadKind {
    match kind {
        RoadChoice::Street => RoadKind::Street,
        RoadChoice::Avenue => RoadKind::Avenue,
    }
}

#[allow(clippy::too_many_arguments)]
fn tool_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ActiveTool>,
    mut modes: ResMut<ToolModes>,
    mut kind: ResMut<RoadChoice>,
    mut state: ResMut<ToolState>,
    mut opts: ResMut<ToolOptions>,
    mut ui: ResMut<UiState>,
) {
    if !ui.menu_open && !ctrl(&keys) {
        let drawing = *tool == ActiveTool::Road && modes.road != RoadMode::Junction;
        let new = if keys.just_pressed(KeyCode::KeyI) {
            Some(ActiveTool::Inspect)
        } else if keys.just_pressed(KeyCode::KeyR) {
            if drawing {
                kind.set_if_neq(match *kind {
                    RoadChoice::Street => RoadChoice::Avenue,
                    RoadChoice::Avenue => RoadChoice::Street,
                });
            } else if *tool == ActiveTool::Road {
                modes.road = RoadMode::Straight;
            }
            Some(ActiveTool::Road)
        } else if keys.just_pressed(KeyCode::KeyJ) {
            modes.road = RoadMode::Junction;
            Some(ActiveTool::Road)
        } else if keys.just_pressed(KeyCode::KeyB) {
            // A switch: on from any tool, off back to it.
            let mut t = *tool;
            super::controls::toggle_demolish(&mut t, &mut ui);
            Some(t)
        } else if keys.just_pressed(KeyCode::KeyZ) {
            Some(ActiveTool::Zone(match *tool {
                ActiveTool::Zone(Some(Zone::Residential)) => Some(Zone::Commercial),
                ActiveTool::Zone(Some(Zone::Commercial)) => Some(Zone::Industrial),
                ActiveTool::Zone(Some(Zone::Industrial)) => None,
                ActiveTool::Zone(None) => Some(Zone::Residential),
                _ => ui.last_zone,
            }))
        } else {
            None
        };
        if let Some(t) = new {
            tool.set_if_neq(t);
        }
        if keys.just_pressed(KeyCode::Tab) {
            let m = next_mode(*tool, *modes);
            modes.set_if_neq(m);
        }
        if *tool == ActiveTool::Road && modes.road == RoadMode::Grid {
            if keys.just_pressed(KeyCode::BracketRight) {
                opts.step_block(1);
            }
            if keys.just_pressed(KeyCode::BracketLeft) {
                opts.step_block(-1);
            }
        }
    }
    // Any tool or mode change (keys, buttons, scripts) drops the action.
    if tool.is_changed() || modes.is_changed() {
        state.reset();
        if let ActiveTool::Zone(z) = *tool {
            ui.last_zone = z;
        }
    }
    let block_m = opts.block_m();
    if let Some(shape) = shape_of(modes.road).filter(|_| *tool == ActiveTool::Road)
        && (state.draft.shape != shape && state.draft.is_empty() || state.draft.block_m != block_m)
    {
        state.draft.shape = shape;
        state.draft.block_m = block_m;
    }
}

/// Ground point (sim metres) under a window position.
fn ground_at(camera: &Camera, cam_tf: &GlobalTransform, pos: Vec2) -> Option<[f32; 2]> {
    let ray = camera.viewport_to_world(cam_tf, pos).ok()?;
    let hit = ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))?;
    Some(crate::shared::world_to_sim(hit))
}

/// The anchor snapping measures from: the draft's last point, and for the
/// first piece started on a road, that road's direction.
fn anchor_of(world: &World, state: &ToolState) -> Option<Anchor> {
    let a = state.draft.anchor()?;
    if a.dir.is_some() {
        return Some(a);
    }
    let towards = state.cursor.map(|c| [c[0] - a.pos[0], c[1] - a.pos[1]]);
    let dir = fsnap::on_net(world, a.pos, 0.5, 0.3).and_then(|on| fsnap::direction_at(world, on, towards));
    Some(Anchor { pos: a.pos, dir })
}

#[allow(clippy::too_many_arguments)]
fn track_pointer(
    hovered: Res<HoveredCell>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Query<&Window, With<PrimaryWindow>>,
    script: Res<ScriptPointer>,
    cameras: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    opts: Res<ToolOptions>,
    sim: Res<SimWorld>,
    mut state: ResMut<ToolState>,
) {
    state.on_map = hovered.0.is_some();
    if hovered.0.is_some() {
        if let Some(pos) = script.position(window.single().ok())
            && let Ok((cam, tf)) = cameras.single()
            && let Some(p) = ground_at(cam, tf, pos)
        {
            state.cursor = Some(p);
            if let Some(q) = ground_at(cam, tf, pos + Vec2::new(SNAP_PX, 0.0)) {
                state.radius = dist(p, q).clamp(3.0, 60.0);
            }
        } else if let Some(c) = hovered.0 {
            // No camera (ui-only build): the cell centre.
            state.cursor = Some(c.center());
            state.radius = 10.0;
        }
    }
    state.snap_off = alt(&keys);
    let snapping = *tool == ActiveTool::Road && modes.road.draws();
    let (Some(cursor), true) = (state.cursor, snapping && (state.on_map || state.busy())) else {
        state.live = None;
        return;
    };
    let w = &sim.world;
    let o = opts.snapping(state.snap_off);
    let grid_depth = modes.road == RoadMode::Grid && state.draft.points.len() == 2;
    let live = if grid_depth {
        // The grid's depth: across its first side, in whole blocks.
        Snapped::free(cursor)
    } else {
        fsnap::snap(w, anchor_of(w, &state), cursor, state.radius, &o)
    };
    state.live = Some(live);
}

/// The lattice rectangle from `p0` to `p1` (whole cells of lattice `l`,
/// both corner cells included), as a quad in sim metres.
pub fn lattice_quad(world: &World, l: LatticeId, p0: [f32; 2], p1: [f32; 2]) -> [[f32; 2]; 4] {
    let lat = match world.zoning.lattice(l) {
        // A strip bends: the rectangle follows the cell the drag started on.
        Some(st) if st.is_strip() => {
            let (i, j) = st.cell_of(p0);
            let (u, _) = st.axes_at(i as f32 + 0.5);
            citysim_sim::Lattice::new(st.world_at(i as f32, j as f32), u)
        }
        Some(x) => x.clone(),
        None => citysim_sim::Lattice::world(),
    };
    let (a, b) = (lat.cell_of(p0), lat.cell_of(p1));
    let (i0, i1) = (a.0.min(b.0), a.0.max(b.0) + 1);
    let (j0, j1) = (a.1.min(b.1), a.1.max(b.1) + 1);
    [
        lat.world_at(i0 as f32, j0 as f32),
        lat.world_at(i1 as f32, j0 as f32),
        lat.world_at(i1 as f32, j1 as f32),
        lat.world_at(i0 as f32, j1 as f32),
    ]
}

/// A walk along the roads as one polyline (for the tool's preview).
fn walk_polyline(world: &World, walk: &[zoning::RoadPiece]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::new();
    for p in walk {
        let Some(seg) = world.roads.segments.get(p.segment) else {
            continue;
        };
        let (a, b) = (p.from_m.min(p.to_m), p.from_m.max(p.to_m));
        if b - a < 0.05 {
            continue;
        }
        let mut pts = seg.curve.sub(a, b).flatten(0.2);
        if !p.forward {
            pts.reverse();
        }
        if out.last().zip(pts.first()).is_some_and(|(x, y)| dist(*x, *y) < 0.01) {
            pts.remove(0);
        }
        out.extend(pts);
    }
    out
}

/// A quad shrunk towards its centre by `m` metres along each diagonal.
fn shrink(q: [[f32; 2]; 4], m: f32) -> [[f32; 2]; 4] {
    let c = [
        (q[0][0] + q[1][0] + q[2][0] + q[3][0]) * 0.25,
        (q[0][1] + q[1][1] + q[2][1] + q[3][1]) * 0.25,
    ];
    q.map(|p| {
        let d = dist(p, c).max(1e-3);
        let k = (m / d).min(0.9);
        [p[0] + (c[0] - p[0]) * k, p[1] + (c[1] - p[1]) * k]
    })
}

/// The stretch of road under `p` (within `reach`), and the position of
/// `p` along it.
fn stretch_at(world: &World, p: [f32; 2], reach: f32) -> Option<(Stretch, f32)> {
    let (s, _, _) = world.roads.nearest_segment(p, reach)?;
    let st = Stretch::of(world, s);
    let at = st.locate(world, p)?;
    Some((st, at))
}

/// The part of a stretch a click or drag means: the whole stretch for a
/// click, from the press to the cursor for a drag along it.
fn stretch_part(world: &World, state: &ToolState, cursor: [f32; 2]) -> Option<(Stretch, f32, f32)> {
    let reach = state.radius.max(6.0);
    match state.press {
        Some(p) if p.moved => {
            let (st, s0) = stretch_at(world, p.at, reach)?;
            let s1 = st.locate(world, cursor)?;
            if (s1 - s0).abs() < balance::MIN_SEGMENT_M {
                let len = st.length();
                return Some((st, 0.0, len));
            }
            Some((st, s0, s1))
        }
        Some(p) => {
            let (st, _) = stretch_at(world, p.at, reach)?;
            let len = st.length();
            Some((st, 0.0, len))
        }
        None => {
            let (st, _) = stretch_at(world, cursor, reach)?;
            let len = st.length();
            Some((st, 0.0, len))
        }
    }
}

/// Paint brush along the pointer's path from `a` to `b`: every cell the
/// path crosses and every cell whose centre is within half a cell's
/// diagonal of it, so a stroke is one band of cells sharing their sides
/// (never a chain of cells touching at the corners, even drawn at 45
/// degrees across the grid), however far the pointer moved since the last
/// frame.
fn brush_cells(world: &World, a: [f32; 2], b: [f32; 2]) -> Vec<CellRef> {
    const R: f32 = citysim_sim::CELL_SIZE * 0.5 * std::f32::consts::SQRT_2 + 0.15;
    // Candidates: centres within reach of the path.
    let reach = R + 0.5;
    let d = sub(b, a);
    let len = dist(a, b);
    let u = if len > 1e-3 { mul(d, 1.0 / len) } else { [1.0, 0.0] };
    let n = [-u[1], u[0]];
    let at = |s: f32, t: f32| add(add(a, mul(u, s)), mul(n, t));
    let q = [
        at(-reach, -reach),
        at(len + reach, -reach),
        at(len + reach, reach),
        at(-reach, reach),
    ];
    let mut out: Vec<CellRef> = zoning::cells_in_quad(world, &q)
        .into_iter()
        .filter(|&c| {
            let Some(ctr) = world.zoning.cell_center(c) else {
                return false;
            };
            let t = dot(sub(ctr, a), u).clamp(0.0, len);
            if dist(ctr, add(a, mul(u, t))) <= R {
                return true;
            }
            // The path crosses the cell (or ends in it).
            let Some(k) = world.zoning.cell_corners(c) else {
                return false;
            };
            zoning::in_quad(&k, a)
                || zoning::in_quad(&k, b)
                || (0..4).any(|e| citysim_sim::curve::segments_meet(a, b, k[e], k[(e + 1) % 4]).is_some())
        })
        .collect();
    out.extend(world.zoning.cell_at(b));
    out
}

fn no_zone(zone: Option<Zone>, hint: &'static str) -> Pending {
    Pending::Zone {
        ops: Vec::new(),
        zone,
        area: None,
        stretch: Vec::new(),
        hint: Some(hint),
    }
}

fn no_bulldoze(hint: &'static str) -> Pending {
    Pending::Bulldoze {
        ops: Vec::new(),
        area: None,
        stretch: Vec::new(),
        building: None,
        hint: Some(hint),
    }
}

/// What the tool would do now, from the tool state and the world.
pub fn pending(
    tool: ActiveTool,
    modes: ToolModes,
    kind: RoadChoice,
    opts: &ToolOptions,
    state: &ToolState,
    world: &World,
) -> Pending {
    let cursor = state.cursor.filter(|_| state.on_map || state.busy());
    match tool {
        ActiveTool::Inspect => Pending::None,
        ActiveTool::Road => match modes.road {
            RoadMode::Straight | RoadMode::Curve | RoadMode::Continuous | RoadMode::Grid => {
                if state.draft.is_empty() {
                    return Pending::None;
                }
                let live = state.live.as_ref().map(|s| s.point).filter(|_| cursor.is_some());
                let k = road_kind(kind);
                let (curves, grid) = draft_roads(&state.draft, live, world);
                let ops = curves
                    .iter()
                    .map(|&curve| Op::Cmd(Command::BuildCurve { curve, kind: k }))
                    .collect();
                Pending::Road {
                    ops,
                    curves,
                    kind: k,
                    grid,
                }
            }
            RoadMode::Upgrade => {
                let k = road_kind(kind);
                let Some(c) = cursor else { return Pending::None };
                let Some((st, s0, s1)) = stretch_part(world, state, c) else {
                    return Pending::Upgrade {
                        ops: Vec::new(),
                        kind: k,
                        stretch: Vec::new(),
                        hint: Some("Point at a road to change its type"),
                    };
                };
                let mut ops = Vec::new();
                let mut highway = false;
                for (s, a, b) in st.ranges(s0, s1) {
                    let seg = &world.roads.segments[s];
                    if seg.kind == RoadKind::Highway {
                        highway = true;
                    } else if seg.kind != k {
                        ops.push(Op::Kind(seg.curve.sub(a, b), k));
                    }
                }
                let hint = ops.is_empty().then_some(if highway {
                    "The highway can't be changed"
                } else if k == RoadKind::Avenue {
                    "Already an avenue"
                } else {
                    "Already a street"
                });
                Pending::Upgrade {
                    ops,
                    kind: k,
                    stretch: st.polyline(world),
                    hint,
                }
            }
            RoadMode::Junction => Pending::None,
        },
        ActiveTool::Zone(zone) => {
            let Some(c) = cursor else { return Pending::None };
            let cells_op = |cells: Vec<CellRef>| Op::Cmd(Command::ZoneCells { cells, zone });
            match modes.zone {
                ZoneMode::Paint => {
                    let cells = if state.press.is_some() {
                        state.stroke.clone()
                    } else {
                        world.zoning.cell_at(c).into_iter().collect()
                    };
                    if cells.is_empty() {
                        return no_zone(zone, "No zoning land here: zone beside a road");
                    }
                    Pending::Zone {
                        ops: vec![cells_op(cells)],
                        zone,
                        area: None,
                        stretch: Vec::new(),
                        hint: None,
                    }
                }
                ZoneMode::Frontage => {
                    let depth = opts.zone_depth.clamp(1, balance::BLOCK_DEPTH);
                    let anchor = state.press.map_or(c, |p| p.at);
                    // Pointing anywhere on the land beside a road picks it.
                    let reach = 4.0 + depth.min(4) as f32 * citysim_sim::CELL_SIZE + 2.0;
                    let Some((s, d, _)) = world.roads.nearest_segment(anchor, reach) else {
                        return no_zone(zone, "Point at the land beside a road");
                    };
                    let seg = &world.roads.segments[s];
                    let pose = seg.pose(d, 0.0);
                    let r = right_of(pose.dir);
                    let rel = [anchor[0] - pose.pos[0], anchor[1] - pose.pos[1]];
                    let side = if rel[0] * r[0] + rel[1] * r[1] >= 0.0 {
                        zoning::Side::Right
                    } else {
                        zoning::Side::Left
                    };
                    // A click: the road between its junctions. A drag: along
                    // the roads from the press to the pointer, through bends,
                    // pieces and junctions, keeping to the same side.
                    let dragged = state
                        .press
                        .filter(|p| p.moved && dist(p.at, c) >= citysim_sim::CELL_SIZE)
                        .and_then(|_| world.roads.nearest_segment(c, reach.max(24.0)))
                        .and_then(|(s1, d1, _)| zoning::road_walk(world, (s, d), (s1, d1), 4000.0));
                    let walk = dragged.unwrap_or_else(|| zoning::stretch_walk(world, s));
                    let side = match walk.iter().find(|p| p.segment == s) {
                        Some(p) if !p.forward => match side {
                            zoning::Side::Left => zoning::Side::Right,
                            zoning::Side::Right => zoning::Side::Left,
                        },
                        _ => side,
                    };
                    let cells = zoning::frontage_walk(world, &walk, side, depth);
                    let stretch = walk_polyline(world, &walk);
                    if cells.is_empty() {
                        return Pending::Zone {
                            ops: Vec::new(),
                            zone,
                            area: None,
                            stretch,
                            hint: Some(if zone.is_some() {
                                "No free land along this side of the road"
                            } else {
                                "Nothing zoned along this side of the road"
                            }),
                        };
                    }
                    Pending::Zone {
                        ops: vec![cells_op(cells)],
                        zone,
                        area: None,
                        stretch,
                        hint: None,
                    }
                }
                ZoneMode::Fill => {
                    let cells = zoning::fill_area(world, c, FILL_LIMIT);
                    if cells.is_empty() {
                        return no_zone(zone, "No zoning land here: point between roads");
                    }
                    Pending::Zone {
                        ops: vec![cells_op(cells)],
                        zone,
                        area: None,
                        stretch: Vec::new(),
                        hint: None,
                    }
                }
                ZoneMode::Marquee => {
                    let usable = |p: [f32; 2]| {
                        world
                            .zoning
                            .cell_at(p)
                            .filter(|&r| world.zoning.cell(r).is_some_and(|x| x.usable()))
                    };
                    let (l, p0) = match state.press {
                        Some(p) => (p.lattice, p.at),
                        None => match usable(c) {
                            Some(r) => (r.lattice, c),
                            // Not over zoning land: nothing to outline yet.
                            None => return no_zone(zone, "Drag over the land beside the roads"),
                        },
                    };
                    let quad = lattice_quad(world, l, p0, c);
                    Pending::Zone {
                        ops: vec![Op::Cmd(Command::ZoneArea {
                            quad: shrink(quad, 0.5),
                            zone,
                        })],
                        zone,
                        area: Some(quad),
                        stretch: Vec::new(),
                        hint: None,
                    }
                }
            }
        }
        ActiveTool::Bulldoze => {
            let Some(c) = cursor else { return Pending::None };
            match modes.bulldoze {
                BulldozeMode::Road => {
                    let reach = state.radius.max(6.0);
                    let on_road = world.roads.nearest_segment(c, reach * 0.8).is_some()
                        || state
                            .press
                            .is_some_and(|p| world.roads.nearest_segment(p.at, reach).is_some());
                    if !on_road && let Some(b) = zoning::building_at(world, c) {
                        let quad = shrink(world.buildings[b].corners(), 0.5);
                        return Pending::Bulldoze {
                            ops: vec![Op::Cmd(Command::BulldozeArea { quad })],
                            area: None,
                            stretch: Vec::new(),
                            building: Some(b),
                            hint: None,
                        };
                    }
                    let Some((st, s0, s1)) = stretch_part(world, state, c) else {
                        return no_bulldoze("Point at a road or a building to remove it");
                    };
                    let mut ops = Vec::new();
                    for (s, a, b) in st.ranges(s0, s1) {
                        let seg = &world.roads.segments[s];
                        if seg.kind != RoadKind::Highway {
                            ops.push(Op::Remove(seg.curve.sub(a, b)));
                        }
                    }
                    let hint = ops.is_empty().then_some("The highway can't be removed");
                    Pending::Bulldoze {
                        ops,
                        area: None,
                        stretch: st.polyline(world),
                        building: None,
                        hint,
                    }
                }
                BulldozeMode::Area => {
                    let Some(p) = state.press else {
                        return no_bulldoze("Drag over roads and buildings to remove them");
                    };
                    let quad = lattice_quad(world, p.lattice, p.at, c);
                    Pending::Bulldoze {
                        ops: vec![Op::Cmd(Command::BulldozeArea {
                            quad: shrink(quad, 0.3),
                        })],
                        area: Some(quad),
                        stretch: Vec::new(),
                        building: None,
                        hint: None,
                    }
                }
            }
        }
    }
}

fn refresh_plan(state: &mut ToolState, sim: &SimWorld, pending: Pending, now: f32) {
    let key = (pending, world_key(sim));
    if state.plan_key.as_ref() == Some(&key) {
        state.plan.stale = false;
        state.stale_since = None;
        return;
    }
    if key.0.ops().is_empty() {
        state.plan = ToolPlan::new(key.0.clone(), Evaluated::default());
        state.plan_key = Some(key);
        state.stale_since = None;
        return;
    }
    let (id, base) = state.scratch_for(sim);
    // Wait (a little) only the first frame a plan is asked for.
    let wait = if state.waited.as_ref() == Some(&key) {
        std::time::Duration::ZERO
    } else {
        planner::WAIT
    };
    state.waited = Some(key.clone());
    if let Some(done) = state.planner.request(id, &base, key.0.ops(), wait) {
        if done.ms > 8.0 {
            debug!("tool plan took {:.1} ms ({} steps)", done.ms, done.eval.steps.len());
        }
        state.plan = ToolPlan::new(key.0.clone(), done.eval.clone());
        state.plan_key = Some(key);
        state.stale_since = None;
        return;
    }
    // Still being checked: keep showing the last plan of the same tool.
    if std::mem::discriminant(&state.plan.pending) != std::mem::discriminant(&key.0) {
        state.plan = ToolPlan::new(key.0.clone(), Evaluated::default());
        state.plan_key = None;
    }
    state.plan.stale = true;
    state.stale_since.get_or_insert(now);
}

/// Carry on with the action waiting for its plan (once the plan sent
/// before it has all gone out): send it when it is checked.
fn drive_job(state: &mut ToolState, sim: &SimWorld, toasts: &mut MessageWriter<Toast>) {
    if !state.outbox.is_empty() {
        return;
    }
    // A refused candidate moves on to the next one within the frame.
    for _ in 0..4 {
        let Some(job) = state.job.take() else { return };
        let ops: Vec<Op> = match &job {
            Job::Commit { cands, idx, .. } => cands[*idx].ops().to_vec(),
            Job::Undo { ops, .. } => ops.clone(),
        };
        let (id, base) = state.scratch_for(sim);
        if state.planner.request(id, &base, &ops, planner::WAIT).is_none() {
            state.job = Some(job);
            return;
        }
        let done = state.planner.take(id, &ops).expect("just checked");
        match job {
            Job::Commit { cands, idx, then } => {
                if done.eval.first_error().is_some() && idx + 1 < cands.len() {
                    state.job = Some(Job::Commit {
                        cands,
                        idx: idx + 1,
                        then,
                    });
                    continue;
                }
                let pending = cands[idx].clone();
                let (toast, sent) = commit(state, sim, &pending, done);
                if let Some(t) = toast {
                    toasts.write(t);
                }
                if then == Then::Road {
                    if sent {
                        state.reset();
                    } else if state.draft.shape == Shape::Grid {
                        // Refused: keep the corner and width so the player
                        // can fix it.
                        state.draft.points.truncate(2);
                    }
                }
            }
            Job::Undo {
                rec,
                ops,
                mut kept_out,
                filtered,
            } => {
                // Buildings whose lot has changed stay demolished; the rest
                // of the undo (roads, other buildings) still goes ahead.
                let refused: Vec<usize> = done
                    .eval
                    .steps
                    .iter()
                    .filter(|s| matches!(s.result, Err(CommandError::CannotRestore(_))))
                    .map(|s| s.op)
                    .collect();
                if !filtered && !refused.is_empty() && refused.len() < ops.len() {
                    for s in &done.eval.steps {
                        if let Err(e @ CommandError::CannotRestore(_)) = &s.result {
                            kept_out.push(super::context::error_text(e));
                        }
                    }
                    let ops = ops
                        .into_iter()
                        .enumerate()
                        .filter(|(i, _)| !refused.contains(i))
                        .map(|(_, o)| o)
                        .collect();
                    state.job = Some(Job::Undo {
                        rec,
                        ops,
                        kept_out,
                        filtered: true,
                    });
                    continue;
                }
                toasts.write(finish_undo(state, sim, &rec, &ops, &kept_out, done));
            }
        }
        return;
    }
}

/// Send queued commands of a plan sent in parts: about
/// [`SEND_BUDGET_MS`] of them a frame; the simulation holds still until
/// the last one is applied.
fn pump_outbox(state: &mut ToolState, out: &mut MessageWriter<SimCommand>, clock: &mut SimClock) {
    let sending = !state.outbox.is_empty();
    let mut budget = SEND_BUDGET_MS;
    while let Some((cmd, ms)) = state.outbox.pop_front() {
        debug!("tool sends {cmd:?}");
        out.write(SimCommand(cmd));
        budget -= ms;
        if budget <= 0.0 {
            break;
        }
    }
    if sending {
        state.batch_frame = true;
    }
    // Hold from the frame the first part goes out until the frame after
    // the last one (applied in this frame's `GameSet::Sim`).
    if clock.hold != sending {
        clock.hold = sending;
    }
}

/// Send `pending`, checked as `done` (only when every step is accepted):
/// its commands are queued and go out over the next frames. Returns the
/// toast to show, and whether it was sent.
fn commit(state: &mut ToolState, sim: &SimWorld, pending: &Pending, done: Done) -> (Option<Toast>, bool) {
    let Done {
        eval,
        base,
        after,
        step_ms,
        ..
    } = done;
    let base: &World = &base;
    if eval.steps.is_empty() {
        return (pending.hint().map(|h| Toast::new(ToastKind::Info, h)), false);
    }
    if let Some(e) = eval.first_error() {
        return (Some(Toast::new(ToastKind::Error, super::context::error_text(e))), false);
    }
    let mut toast = None;
    // Buildings the plan takes down (bulldozed, or on land a new road now
    // needs), recorded from the real world so undo can put them back.
    let lost = || plan::demolished(&sim.world, base, &after);
    match pending {
        Pending::Road { curves, grid, .. } => {
            let label = if grid.is_some() {
                "street grid".to_string()
            } else if curves.len() > 1 {
                format!("{}-part road", curves.len())
            } else {
                "road".to_string()
            };
            push_undo(state, plan::undo_for_roads(base, &after, &eval, label, lost()));
        }
        Pending::Upgrade { kind, .. } => {
            let label = if *kind == RoadKind::Avenue {
                "upgrade to avenue"
            } else {
                "change to street"
            };
            push_undo(state, plan::undo_for_roads(base, &after, &eval, label.into(), lost()));
        }
        Pending::Bulldoze { .. } => {
            let removed = eval.removed().len();
            let buildings = lost();
            let n = buildings.len();
            if removed > 0 || n > 0 {
                let label = if removed > 0 {
                    "demolition"
                } else if n == 1 {
                    "building demolition"
                } else {
                    "demolition of buildings"
                };
                push_undo(
                    state,
                    plan::undo_for_roads(base, &after, &eval, label.into(), buildings),
                );
            }
            if removed == 0 && n > 0 {
                let what = if n == 1 {
                    "Building demolished".to_string()
                } else {
                    format!("{n} buildings demolished")
                };
                toast = Some(Toast::new(ToastKind::Info, format!("{what} · Ctrl+Z puts it back")));
            }
        }
        Pending::Zone { zone, .. } => {
            let cells = eval.zone_cells();
            if cells.is_empty() {
                toast = Some(Toast::new(
                    ToastKind::Info,
                    if zone.is_some() {
                        "Nothing to zone here: already zoned, built on or not beside a road"
                    } else {
                        "Nothing to unzone here"
                    },
                ));
            } else {
                let what = super::widgets::zone_name(*zone).to_lowercase();
                let label = if zone.is_some() {
                    format!("{what} zoning")
                } else {
                    "de-zoning".into()
                };
                push_undo(state, plan::undo_for_zones(base, &cells, label));
            }
        }
        Pending::None => {}
    }
    state.batch_frame = eval.steps.len() > 1;
    for (st, ms) in eval.steps.iter().zip(step_ms) {
        if let Some(cmd) = &st.cmd {
            state.outbox.push_back((cmd.clone(), ms));
        }
    }
    (toast, true)
}

fn push_undo(state: &mut ToolState, rec: UndoRecord) {
    state.undo.push(rec);
    if state.undo.len() > UNDO_DEPTH {
        state.undo.remove(0);
    }
}

/// Ctrl+Z: take the last action back once its plan is checked
/// ([`drive_job`]). A toast now only when there is nothing to do.
fn undo(state: &mut ToolState, sim: &SimWorld) -> Option<Toast> {
    let Some(rec) = state.undo.pop() else {
        return Some(Toast::new(ToastKind::Info, "Nothing to undo"));
    };
    let label = rec.label().to_string();
    let ops = plan::undo_ops(&sim.world, &rec);
    if ops.is_empty() {
        return Some(Toast::new(ToastKind::Info, format!("The {label} is already gone")));
    }
    state.job = Some(Job::Undo {
        rec,
        ops,
        kept_out: Vec::new(),
        filtered: false,
    });
    None
}

/// Send the undo of `rec` checked as `done` (operations `ops`), or say why
/// it cannot be done.
fn finish_undo(
    state: &mut ToolState,
    sim: &SimWorld,
    rec: &UndoRecord,
    ops: &[Op],
    kept_out: &[String],
    done: Done,
) -> Toast {
    let label = rec.label().to_string();
    let restores = match rec {
        UndoRecord::Roads { buildings, .. } => buildings.len(),
        UndoRecord::Zones { .. } => 0,
    };
    let eval = done.eval;
    if let Some(e) = eval.first_error() {
        if let CommandError::BuildingInTheWay { at, building } = e {
            let b = sim.world.buildings.get(*building);
            debug!(
                "undo refused: building {building:?} in the way at {at:?}: {:?} ops {:?}",
                b.map(|b| (b.center(), b.footprint, b.created_tick)),
                ops.iter()
                    .filter(|o| !matches!(o, Op::Cmd(Command::RestoreBuilding(_))))
                    .collect::<Vec<_>>()
            );
        }
        return Toast::new(
            ToastKind::Warning,
            format!("Can't undo the {label}: {}", super::context::error_text(e)),
        );
    }
    state.batch_frame = true;
    for (st, ms) in eval.steps.iter().zip(done.step_ms) {
        if let Some(cmd) = &st.cmd {
            state.outbox.push_back((cmd.clone(), ms));
        }
    }
    let mut extra = String::new();
    let back: Vec<&citysim_sim::BuildingRecord> = ops
        .iter()
        .filter_map(|o| match o {
            Op::Cmd(Command::RestoreBuilding(r)) => Some(&**r),
            _ => None,
        })
        .collect();
    let road_cost: i64 = eval.cost + back.iter().map(|r| r.building.footprint.area() as i64).sum::<i64>();
    if road_cost > 0 && back.len() < ops.len() {
        extra = format!(" (the road costs {})", super::format::money(road_cost));
    }
    if !back.is_empty() {
        let people: usize = back.iter().map(|r| r.residents()).sum();
        let staff: usize = back.iter().map(|r| r.workers.len()).sum();
        let what = if back.len() == 1 {
            "building back".to_string()
        } else {
            format!("{} buildings back", back.len())
        };
        extra.push_str(&format!(" · {what}"));
        if people > 0 {
            extra.push_str(&format!(
                ", {people} {}",
                if people == 1 { "resident" } else { "residents" }
            ));
        } else if staff > 0 {
            extra.push_str(&format!(", {staff} on the staff"));
        }
    }
    if !kept_out.is_empty() {
        let n = kept_out.len();
        let first = kept_out[0].trim_end_matches('.').to_string();
        let msg = if n == restores {
            format!("Undid the {label}{extra}, but the buildings can't come back: {first}")
        } else {
            format!(
                "Undid the {label}{extra}; {n} {} can't come back: {first}",
                if n == 1 { "building" } else { "buildings" }
            )
        };
        return Toast::new(ToastKind::Warning, msg);
    }
    Toast::new(ToastKind::Success, format!("Undid the {label}{extra}"))
}

/// What the road tools do with a placed point.
fn place_point(state: &mut ToolState, sim: &SimWorld, p: Snapped) -> Placed {
    let w = &sim.world;
    let first = state.draft.is_empty();
    let placed = state.draft.place(p.point);
    if first && placed == Placed::Point && state.draft.shape == Shape::Continuous {
        // A continuous road started at a dead end carries on out of it.
        state.draft.start_dir = p.on.and_then(|on| match on {
            OnNet::Node(_) => fsnap::direction_at(w, on, None),
            OnNet::Road { .. } => None,
        });
    }
    if placed == Placed::Grid {
        finish(state, sim, false);
    }
    placed
}

/// The roads a draft would build (and its grid), the grid moved onto an
/// existing node it nearly touches.
fn draft_roads(draft: &Draft, live: Option<[f32; 2]>, world: &World) -> (Vec<Curve>, Option<GridFrame>) {
    if draft.shape != Shape::Grid {
        return (draft.curves(live), None);
    }
    let grid = draft.grid(live).map(|g| align_grid(world, g));
    let mut curves = draft.pieces.clone();
    curves.extend(grid.iter().flat_map(|g| g.streets()));
    (curves, grid)
}

/// A grid whose crossings or ends come within a few centimetres of an
/// existing junction or road end (a corner snapped onto a road a hair off
/// the cell centre) is moved onto it, so every street meets that node
/// exactly; a near miss would be a crossing too close to a road's end.
pub fn align_grid(world: &World, g: GridFrame) -> GridFrame {
    const REACH: f32 = 1.0;
    let mut best: Option<(f32, [f32; 2])> = None;
    for i in 0..=g.nu {
        for j in 0..=g.nv {
            let p = [
                g.a[0] + g.u[0] * i as f32 * g.block_m + g.v[0] * j as f32 * g.block_m,
                g.a[1] + g.u[1] * i as f32 * g.block_m + g.v[1] * j as f32 * g.block_m,
            ];
            for (_, n) in world.roads.nodes.iter() {
                let d = dist(n.pos, p);
                if d < REACH && best.is_none_or(|b| d < b.0) {
                    best = Some((d, [n.pos[0] - p[0], n.pos[1] - p[1]]));
                }
            }
        }
    }
    match best {
        Some((d, shift)) if d > 1e-4 => GridFrame {
            a: [g.a[0] + shift[0], g.a[1] + shift[1]],
            ..g
        },
        _ => g,
    }
}

/// Build the draft (with the live piece when `with_live`).
/// It is sent once its plan is checked ([`drive_job`]).
fn finish(state: &mut ToolState, sim: &SimWorld, with_live: bool) {
    let mut live = if with_live && state.on_map {
        state.live.as_ref().map(|s| s.point)
    } else {
        None
    };
    // Enter right where the last piece ended: the cursor (snapped a cell
    // on, or a few metres off) must not add a stub to the road.
    let placed = !state.draft.pieces.is_empty();
    if placed
        && let (Some(l), Some(end)) = (live, state.draft.last())
        && dist(l, end) < citysim_sim::CELL_SIZE
    {
        live = None;
    }
    let kind = match &state.plan.pending {
        Pending::Road { kind, .. } => *kind,
        _ => RoadKind::Street,
    };
    let draft = state.draft.clone();
    let draft_plan = |live: Option<[f32; 2]>| -> Option<Pending> {
        let (curves, grid) = draft_roads(&draft, live, &sim.world);
        if curves.is_empty() {
            return None;
        }
        let ops: Vec<Op> = curves
            .iter()
            .map(|&curve| Op::Cmd(Command::BuildCurve { curve, kind }))
            .collect();
        Some(Pending::Road {
            ops,
            curves,
            kind,
            grid,
        })
    };
    let Some(p) = draft_plan(live) else {
        state.reset();
        return;
    };
    // The piece under the cursor is refused but the pieces already placed
    // are fine: build those (the cursor piece was never clicked).
    let mut cands = vec![p];
    if placed
        && live.is_some()
        && state.draft.shape != Shape::Grid
        && let Some(q) = draft_plan(None)
    {
        cands.push(q);
    }
    state.job = Some(Job::Commit {
        cands,
        idx: 0,
        then: Then::Road,
    });
}

/// Esc / right-click: drop the piece in progress, then the whole road.
fn step_back(state: &mut ToolState) {
    if state.draft.points.len() > 1 {
        state.draft.points.truncate(1);
        state.press = None;
    } else {
        state.reset();
    }
}

/// Paint mode, button held: add the cells along the pointer's path since
/// the last frame to the stroke.
fn extend_stroke(state: &mut ToolState, tool: ActiveTool, mode: ZoneMode, world: &World) {
    if !(matches!(tool, ActiveTool::Zone(_)) && mode == ZoneMode::Paint && state.press.is_some()) {
        return;
    }
    let Some(c) = state.cursor else { return };
    let from = state.stroke_last.unwrap_or(c);
    for cell in brush_cells(world, from, c) {
        if !state.stroke.contains(&cell) {
            state.stroke.push(cell);
        }
    }
    state.stroke_last = Some(c);
}

#[allow(clippy::too_many_arguments)]
fn use_tool(
    time: Res<Time<Real>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    over_ui: Res<PointerOverUi>,
    (tool, modes, kind): (Res<ActiveTool>, Res<ToolModes>, Res<RoadChoice>),
    opts: Res<ToolOptions>,
    ui: Res<UiState>,
    sim: Res<SimWorld>,
    mut actions: MessageReader<UiAction>,
    mut state: ResMut<ToolState>,
    mut out: MessageWriter<SimCommand>,
    mut toasts: MessageWriter<Toast>,
    mut clock: ResMut<SimClock>,
) {
    state.batch_frame = false;
    let state = state.as_mut();
    let now = time.elapsed_secs();
    // An action waiting for its plan, then whatever is queued to send.
    drive_job(state, &sim, &mut toasts);
    pump_outbox(state, &mut out, &mut clock);
    let undo_asked = actions.read().any(|a| *a == UiAction::Undo);
    if state.job.is_some() {
        // Input waits until the action is on its way (a few frames at
        // most).
        return;
    }
    if ui.menu_open {
        state.reset();
        return;
    }
    if undo_asked || ctrl(&keys) && keys.just_pressed(KeyCode::KeyZ) {
        state.reset();
        if let Some(t) = undo(state, &sim) {
            toasts.write(t);
        }
        // Checked and sent from the next frame.
        return;
    }
    let pressed = mouse.just_pressed(MouseButton::Left) && state.on_map && !over_ui.0;
    // A release outside the window may never arrive: the button simply is
    // no longer held.
    let released = mouse.just_released(MouseButton::Left) || state.press.is_some() && !mouse.pressed(MouseButton::Left);
    let right = mouse.just_pressed(MouseButton::Right) && !over_ui.0;
    // The stroke so far, before the preview of this frame.
    extend_stroke(state, *tool, modes.zone, &sim.world);
    let p = pending(*tool, *modes, *kind, &opts, state, &sim.world);
    refresh_plan(state, &sim, p, now);
    if (keys.just_pressed(KeyCode::Escape) && !ui.tray_focus || right) && state.busy() {
        step_back(state);
        return;
    }
    // A drag: the pointer left the press point.
    let slop = state.radius.max(3.0) * 0.6;
    if let (Some(p), Some(c)) = (state.press.as_mut(), state.cursor)
        && dist(p.at, c) > slop
    {
        p.moved = true;
    }
    match *tool {
        ActiveTool::Inspect => {}
        ActiveTool::Road if modes.road.draws() => {
            if keys.just_pressed(KeyCode::Backspace) {
                state.draft.pop();
            }
            let enter =
                (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter)) && !ui.tray_focus;
            if pressed && let Some(live) = state.live.clone() {
                let double = state
                    .last_click
                    .is_some_and(|(t, at)| now - t < DOUBLE_CLICK_S && dist(at, live.point) < state.radius.max(2.0));
                state.last_click = Some((now, live.point));
                if double && !state.draft.is_empty() {
                    finish(state, &sim, false);
                    state.last_click = None;
                } else {
                    let lattice = sim
                        .world
                        .zoning
                        .cell_at(live.point)
                        .map_or(LatticeId::WORLD, |c| c.lattice);
                    // Drags are measured from where the pointer went down,
                    // not from the snapped point (a snap of a few metres is
                    // not a drag, and must not add a piece on release).
                    let at = state.cursor.unwrap_or(live.point);
                    place_point(state, &sim, live);
                    state.press = Some(Press {
                        at,
                        time: now,
                        moved: false,
                        lattice,
                    });
                }
            }
            if released
                && let Some(p) = state.press.take()
                && p.moved
                && state.on_map
                && let Some(live) = state.live.clone()
            {
                // The release of a drag is the next point.
                let fresh = state.draft.pieces.is_empty() && state.draft.points.len() == 1;
                let placed = place_point(state, &sim, live);
                if fresh && placed == Placed::Piece && state.draft.shape != Shape::Curve {
                    finish(state, &sim, false);
                }
                state.last_click = None;
            }
            if enter && !state.draft.is_empty() {
                finish(state, &sim, true);
            }
        }
        ActiveTool::Road if modes.road == RoadMode::Junction => {
            if pressed
                && let Some(c) = state.cursor
                && let Some((node, _)) = junction_near(&sim.world, c, state.radius.max(8.0))
            {
                let setting = citysim_sim::junction::junction(&sim.world, node)
                    .map(|j| j.setting)
                    .unwrap_or_default();
                let next = opts
                    .junction_brush
                    .unwrap_or_else(|| super::context::next_setting(setting));
                let cmd = Command::SetNodeControl { node, control: next };
                match sim.world.preview(&cmd) {
                    Ok(p) => {
                        out.write(SimCommand(cmd));
                        let now_control = p.junction.map(|j| j.control_after);
                        let text = match (next, now_control) {
                            (JunctionSetting::Auto, Some(ctl)) => format!(
                                "Junction back to automatic: {}",
                                super::context::control_name(ctl).to_lowercase()
                            ),
                            (_, Some(ctl)) => {
                                format!("Junction set to {}", super::context::control_name(ctl).to_lowercase())
                            }
                            _ => "Junction control changed".into(),
                        };
                        toasts.write(Toast::new(ToastKind::Info, text));
                    }
                    Err(e) => {
                        toasts.write(Toast::new(ToastKind::Error, super::context::error_text(&e)));
                    }
                }
            }
        }
        _ => {
            // Press-and-release tools: upgrade, zoning, bulldozer.
            if pressed && let Some(c) = state.cursor {
                let lattice = sim.world.zoning.cell_at(c).map_or(LatticeId::WORLD, |r| r.lattice);
                state.press = Some(Press {
                    at: c,
                    time: now,
                    moved: false,
                    lattice,
                });
                state.stroke.clear();
                state.stroke_last = None;
            }
            // Paint: add the cells under the pointer's path.
            extend_stroke(state, *tool, modes.zone, &sim.world);
            if released && state.press.is_some() {
                // The plan for the release position, sent once checked.
                let p = pending(*tool, *modes, *kind, &opts, state, &sim.world);
                state.job = Some(Job::Commit {
                    cands: vec![p],
                    idx: 0,
                    then: Then::Nothing,
                });
                state.reset();
            }
        }
    }
}

/// A junction (three or more arms) within `reach` of `p`.
pub fn junction_near(world: &World, p: [f32; 2], reach: f32) -> Option<(NodeId, [f32; 2])> {
    let mut best: Option<(f32, NodeId, [f32; 2])> = None;
    for (n, node) in world.roads.nodes.iter() {
        if node.segments.len() < 3 {
            continue;
        }
        let d = dist(node.pos, p);
        if d <= reach && best.is_none_or(|b| d < b.0) {
            best = Some((d, n, node.pos));
        }
    }
    best.map(|b| (b.1, b.2))
}

/// The vehicle nearest to `p` within `reach` metres (at its drawn pose).
pub fn vehicle_near(world: &World, p: [f32; 2], reach: f32) -> Option<citysim_sim::VehicleId> {
    let mut best: Option<(f32, citysim_sim::VehicleId)> = None;
    for (id, v) in world.vehicles.iter() {
        if (v.pos[0] - p[0]).abs() > reach + 10.0 || (v.pos[1] - p[1]).abs() > reach + 10.0 {
            continue;
        }
        let (pos, _) = citysim_sim::drive::vehicle_pose(world, v);
        let d = dist(pos, p);
        if d <= reach && best.is_none_or(|b| d < b.0) {
            best = Some((d, id));
        }
    }
    best.map(|b| b.1)
}

/// What the inspect tool points at: a car, a building, a junction, a road
/// stretch (in that order).
pub fn target_at(world: &World, p: [f32; 2], radius: f32) -> Option<WorldTarget> {
    // Cars are small from far away: the reach grows with the zoom (the
    // cursor radius is a fixed number of pixels in metres).
    if let Some(v) = vehicle_near(world, p, 3.2f32.max(radius * 0.45)) {
        return Some(WorldTarget::Vehicle(v));
    }
    if let Some(b) = zoning::building_at(world, p) {
        return Some(WorldTarget::Building(b));
    }
    if let Some((node, pos)) = junction_near(world, p, radius.clamp(4.0, 10.0)) {
        return Some(WorldTarget::Junction { node, pos });
    }
    let (s, _, d) = world.roads.nearest_segment(p, 9.0)?;
    let half = world.roads.segments[s].kind.half_width_m() + 1.0;
    if d > half.max(radius * 0.5) {
        return None;
    }
    Some(WorldTarget::Road(stretch_target(world, &Stretch::of(world, s))))
}

pub fn stretch_target(world: &World, st: &Stretch) -> RoadStretch {
    RoadStretch {
        segments: st.segments(),
        points: st.polyline(world),
        cells: Vec::new(),
    }
}

/// The curve tool view of the current plan.
fn curve_view(state: &ToolState, now: f32) -> CurveToolView {
    let plan = &state.plan;
    let live = state
        .live
        .as_ref()
        .map(|s| s.point)
        .filter(|_| state.on_map || state.busy());
    match &plan.pending {
        Pending::Road { curves, kind, .. } => {
            let mut refused = Vec::new();
            let mut error_at = None;
            for s in &plan.eval.steps {
                if let Err(e) = &s.result {
                    let curve = match plan.pending.ops().get(s.op) {
                        Some(Op::Cmd(Command::BuildCurve { curve, .. })) => Some(*curve),
                        _ => None,
                    };
                    refused.extend(curve);
                    if error_at.is_none() {
                        error_at = e.at().or(curve.map(|c| c.end()));
                    }
                }
            }
            let tightest = super::shapes::tightest(curves).map(|(p, c, r, _)| (p, c, r, r < kind.min_radius_m()));
            CurveToolView {
                active: true,
                kind: Some(*kind),
                pieces: plan.pieces.clone(),
                refused,
                error_at,
                points: state.draft.control_points(live),
                handles: state.draft.handles(live),
                tightest,
                snap: None,
                guides: Vec::new(),
                nodes: plan.nodes.clone(),
                stale: state.dimmed(now),
            }
        }
        Pending::Upgrade { kind, ops, .. } => {
            let mut pieces = plan.pieces.clone();
            if pieces.is_empty() {
                pieces = ops
                    .iter()
                    .filter_map(|op| match op {
                        Op::Kind(c, _) => Some((*c, PieceChange::Upgrade)),
                        _ => None,
                    })
                    .collect();
            }
            let refused = plan
                .eval
                .steps
                .iter()
                .filter(|s| s.result.is_err())
                .filter_map(|s| match plan.pending.ops().get(s.op) {
                    Some(Op::Kind(c, _)) => Some(*c),
                    _ => None,
                })
                .collect();
            CurveToolView {
                active: !pieces.is_empty(),
                kind: Some(*kind),
                pieces,
                refused,
                error_at: plan.error().and_then(|e| e.at()),
                stale: state.dimmed(now),
                ..default()
            }
        }
        _ => CurveToolView::default(),
    }
}

type HoverKey = (Option<(i32, i32)>, (u64, u64, u64), u64);

/// Inspect selection (pinned by a click) lives in `WorldHighlight::selected`.
#[allow(clippy::too_many_arguments)]
fn publish(
    time: Res<Time<Real>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    over_ui: Res<PointerOverUi>,
    (tool, modes, kind): (Res<ActiveTool>, Res<ToolModes>, Res<RoadChoice>),
    opts: Res<ToolOptions>,
    sim: Res<SimWorld>,
    ui: Res<UiState>,
    mut state: ResMut<ToolState>,
    (mut preview, mut curve, mut snapfb, mut highlight): (
        ResMut<ToolPreview>,
        ResMut<CurveToolView>,
        ResMut<SnapFeedback>,
        ResMut<WorldHighlight>,
    ),
    mut last_view: Local<CurveToolView>,
    mut hover: Local<(Option<HoverKey>, Option<WorldTarget>)>,
) {
    let state = state.as_mut();
    let w = &sim.world;
    if ui.menu_open {
        preview.set_if_neq(ToolPreview::default());
        snapfb.set_if_neq(SnapFeedback::default());
        if *last_view != CurveToolView::default() {
            *last_view = CurveToolView::default();
            curve.set_if_neq(CurveToolView::default());
        }
        return;
    }
    // The action waiting to be sent shows its own plan.
    let p = match state.job_pending() {
        Some(p) => p.clone(),
        None => pending(*tool, *modes, *kind, &opts, state, w),
    };
    let now = time.elapsed_secs();
    refresh_plan(state, &sim, p, now);
    // The free-form road (written only when it changes, so gfx's own
    // sample states survive while no road is drawn).
    let view = curve_view(state, now);
    if *last_view != view {
        *last_view = view.clone();
        curve.set_if_neq(view);
    }
    let plan = &state.plan;
    let mut tp = ToolPreview::default();
    let error = plan.error().cloned();
    let mut doomed_roads = Vec::new();
    let mut doomed_buildings = Vec::new();
    let mut stretch_hl: Option<Vec<[f32; 2]>> = None;
    match &plan.pending {
        Pending::None => {}
        Pending::Road { .. } | Pending::Upgrade { .. } => {
            tp.kind = PreviewKind::Road;
            tp.error = error;
            tp.cost = plan.eval.cost;
            if let Pending::Upgrade { stretch, .. } = &plan.pending {
                stretch_hl = Some(stretch.clone());
            }
        }
        Pending::Zone {
            zone, area, stretch, ..
        } => {
            tp.kind = PreviewKind::Zone(*zone);
            tp.zone_cells = plan.eval.zone_cells();
            tp.area = *area;
            if !stretch.is_empty() {
                stretch_hl = Some(stretch.clone());
            }
        }
        Pending::Bulldoze { area, stretch, .. } => {
            tp.kind = PreviewKind::Bulldoze;
            tp.area = *area;
            tp.cost = plan.eval.cost;
            tp.error = error;
            doomed_roads = plan.eval.removed();
            doomed_buildings = plan.eval.buildings();
            if !stretch.is_empty() && doomed_roads.is_empty() {
                stretch_hl = Some(stretch.clone());
            }
        }
    }
    preview.set_if_neq(tp);
    let fb = match &state.live {
        Some(s) if state.on_map || state.busy() => SnapFeedback {
            point: Some(s.point),
            cell: None,
            target: s.target,
            matched: s.matched.clone(),
            guides: s.guides.clone(),
            spacing_cells: s.spacing_m.map(|m| (m / citysim_sim::CELL_SIZE).round() as i32),
            suspended: state.snap_off,
        },
        _ => SnapFeedback::default(),
    };
    snapfb.set_if_neq(fb);

    // Hover / selection highlight.
    let wants_hover = match *tool {
        ActiveTool::Inspect => true,
        ActiveTool::Road => modes.road == RoadMode::Junction,
        _ => false,
    };
    let cursor = state.cursor.filter(|_| wants_hover && state.on_map && !over_ui.0);
    // Cars move: with Inspect, pick again every tick.
    let key = (
        cursor.map(|c| ((c[0] * 2.0) as i32, (c[1] * 2.0) as i32)),
        (sim.generation, w.versions.roads, w.versions.buildings),
        if *tool == ActiveTool::Inspect { w.time.tick } else { 0 },
    );
    if hover.0 != Some(key) {
        hover.0 = Some(key);
        hover.1 = cursor.and_then(|c| match *tool {
            ActiveTool::Inspect => target_at(w, c, state.radius),
            _ => junction_near(w, c, state.radius.max(8.0)).map(|(node, pos)| WorldTarget::Junction { node, pos }),
        });
    }
    let mut hl = WorldHighlight {
        hovered: hover.1.clone(),
        selected: highlight.selected.clone(),
        doomed_buildings,
        doomed_roads,
    };
    if let Some(points) = stretch_hl.filter(|p| p.len() >= 2) {
        hl.hovered = Some(WorldTarget::Road(RoadStretch {
            segments: Vec::new(),
            points,
            cells: Vec::new(),
        }));
    }
    if let Pending::Bulldoze { building: Some(b), .. } = &plan.pending {
        hl.hovered = Some(WorldTarget::Building(*b));
    }
    // Inspect: a click pins what is under the cursor (or clears it).
    if *tool == ActiveTool::Inspect {
        if mouse.just_pressed(MouseButton::Left) && !over_ui.0 && state.on_map {
            hl.selected = hover.1.clone();
        }
        if keys.just_pressed(KeyCode::Escape) {
            hl.selected = None;
        }
    }
    // A pinned thing that no longer exists is dropped.
    hl.selected = hl.selected.filter(|t| target_exists(w, t));
    highlight.set_if_neq(hl);
}

pub fn target_exists(world: &World, t: &WorldTarget) -> bool {
    match t {
        WorldTarget::Building(id) => world.buildings.contains_key(*id),
        WorldTarget::Road(s) => s.segments.iter().all(|&id| world.roads.segments.contains_key(id)),
        WorldTarget::Junction { node, .. } => world.roads.nodes.get(*node).is_some_and(|n| n.segments.len() >= 3),
        WorldTarget::Vehicle(v) => world.vehicles.contains_key(*v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn town() -> World {
        let mut w = World::new(1);
        w.economy.money = 10_000_000;
        w.apply(Command::BuildCurve {
            curve: Curve::straight([92.0, 1028.0], [300.0, 1028.0]),
            kind: RoadKind::Street,
        })
        .unwrap();
        w
    }

    fn state_at(c: [f32; 2]) -> ToolState {
        ToolState {
            cursor: Some(c),
            on_map: true,
            radius: 8.0,
            ..default()
        }
    }

    #[test]
    fn tab_cycles_every_mode_and_returns() {
        let mut m = ToolModes::default();
        for _ in 0..6 {
            m = next_mode(ActiveTool::Road, m);
        }
        assert_eq!(m, ToolModes::default());
        m = next_mode(ActiveTool::Zone(None), m);
        assert_eq!(m.zone, ZoneMode::Frontage);
        m = next_mode(ActiveTool::Bulldoze, m);
        assert_eq!(m.bulldoze, BulldozeMode::Area);
    }

    #[test]
    fn a_drawn_road_previews_with_its_live_piece() {
        let w = town();
        let mut s = state_at([200.0, 1100.0]);
        s.draft = Draft::new(Shape::Straight, 40.0);
        s.draft.place([200.0, 1028.0]);
        s.live = Some(Snapped::free([200.0, 1100.0]));
        let p = pending(
            ActiveTool::Road,
            ToolModes::default(),
            RoadChoice::Street,
            &ToolOptions::default(),
            &s,
            &w,
        );
        let Pending::Road { ops, .. } = &p else { panic!("{p:?}") };
        assert_eq!(ops.len(), 1);
        let (e, _) = plan::evaluate(&plan::scratch(&w), p.ops());
        assert!(e.ok(), "{:?}", e.first_error());
        // A T onto the street.
        assert!(e.nodes().iter().any(|n| n.1 == citysim_sim::command::NodeChange::Split));
    }

    #[test]
    fn frontage_zones_one_side_to_the_chosen_depth() {
        let w = town();
        // Just north of the street (smaller y).
        let s = state_at([200.0, 1022.0]);
        let modes = ToolModes {
            zone: ZoneMode::Frontage,
            ..default()
        };
        let opts = ToolOptions::default();
        let zone = ActiveTool::Zone(Some(Zone::Residential));
        let p = pending(zone, modes, RoadChoice::Street, &opts, &s, &w);
        let (e, _) = plan::evaluate(&plan::scratch(&w), p.ops());
        let cells = e.zone_cells();
        assert!(cells.len() > 20, "{}", cells.len());
        assert!(
            cells
                .iter()
                .all(|&c| w.zoning.cell_center(c).is_some_and(|q| q[1] < 1028.0)),
            "all on the north side"
        );
        // One row is half of two rows; the full block is deeper still.
        let count = |d: u8| {
            let o = ToolOptions { zone_depth: d, ..opts };
            let p = pending(zone, modes, RoadChoice::Street, &o, &s, &w);
            plan::evaluate(&plan::scratch(&w), p.ops()).0.zone_cells().len()
        };
        assert_eq!(count(1) * 2, count(2));
        assert!(cells.len() > count(2), "{} full vs {} two rows", cells.len(), count(2));
    }

    #[test]
    fn marquee_follows_the_lattice_and_counts_its_cells() {
        let w = town();
        let mut s = state_at([150.0, 1043.0]);
        s.press = Some(Press {
            at: [122.0, 1034.0],
            time: 0.0,
            moved: true,
            lattice: LatticeId::WORLD,
        });
        let modes = ToolModes {
            zone: ZoneMode::Marquee,
            ..default()
        };
        let p = pending(
            ActiveTool::Zone(Some(Zone::Commercial)),
            modes,
            RoadChoice::Street,
            &ToolOptions::default(),
            &s,
            &w,
        );
        let Pending::Zone { area: Some(q), .. } = &p else {
            panic!("{p:?}")
        };
        // Whole cells: corners on multiples of 8 m.
        assert!(q.iter().flatten().all(|v| (v / 8.0).fract().abs() < 1e-3), "{q:?}");
        let (e, _) = plan::evaluate(&plan::scratch(&w), p.ops());
        // x 120..152 (4 cells), rows 129 and 130 south of the road.
        assert_eq!(e.zone_cells().len(), 8, "{:?}", e.zone_cells());
    }

    #[test]
    fn upgrade_a_part_by_dragging_along_it() {
        let w = town();
        let mut s = state_at([250.0, 1028.0]);
        s.press = Some(Press {
            at: [150.0, 1029.0],
            time: 0.0,
            moved: true,
            lattice: LatticeId::WORLD,
        });
        let modes = ToolModes {
            road: RoadMode::Upgrade,
            ..default()
        };
        let p = pending(
            ActiveTool::Road,
            modes,
            RoadChoice::Avenue,
            &ToolOptions::default(),
            &s,
            &w,
        );
        let (e, after) = plan::evaluate(&plan::scratch(&w), p.ops());
        assert!(e.ok(), "{:?}", e.first_error());
        let avenue: f32 = after
            .roads
            .segments
            .values()
            .filter(|s| s.kind == RoadKind::Avenue)
            .map(|s| s.length)
            .sum();
        assert!((avenue - 100.0).abs() < 1.0, "{avenue}");
    }

    #[test]
    fn bulldoze_a_part_of_a_road() {
        let w = town();
        let mut s = state_at([280.0, 1028.0]);
        s.press = Some(Press {
            at: [200.0, 1028.0],
            time: 0.0,
            moved: true,
            lattice: LatticeId::WORLD,
        });
        let p = pending(
            ActiveTool::Bulldoze,
            ToolModes::default(),
            RoadChoice::Street,
            &ToolOptions::default(),
            &s,
            &w,
        );
        let (e, _) = plan::evaluate(&plan::scratch(&w), p.ops());
        assert!(e.ok(), "{:?}", e.first_error());
        let removed: f32 = e.removed().iter().map(|(c, _)| c.length()).sum();
        assert!((removed - 80.0).abs() < 1.0, "{removed}");
    }

    /// A grown city (hierarchy roads, about 5,000 residents) and an 8 x 13
    /// street grid of 40 m blocks next to it that can all be built.
    fn grown_city_and_grid() -> (SimWorld, GridFrame) {
        let mut w = World::new(3);
        let mut player = citysim_sim::scenarios::GridPlayer::hierarchy();
        while w.stats.population < 5_000 && w.time.day < 40 {
            player.act(&mut w);
            w.step();
        }
        w.economy.money = 50_000_000;
        let hi = w
            .roads
            .nodes
            .values()
            .fold([f32::MIN; 2], |m, n| [m[0].max(n.pos[0]), m[1].max(n.pos[1])]);
        let lo = w
            .roads
            .nodes
            .values()
            .fold([f32::MAX; 2], |m, n| [m[0].min(n.pos[0]), m[1].min(n.pos[1])]);
        let mid = (lo[1] + hi[1]) * 0.5;
        for step in [1.0f32, 2.0, 4.0, 8.0] {
            let g = GridFrame {
                a: [
                    (hi[0] / 8.0).round() * 8.0 + step * 8.0,
                    (mid / 8.0).round() * 8.0 - 160.0,
                ],
                u: [0.0, 1.0],
                v: [1.0, 0.0],
                nu: 8,
                nv: 13,
                block_m: 40.0,
            };
            let (e, _) = plan::evaluate(&plan::scratch(&w), &grid_ops(&g));
            if e.first_error().is_none() {
                return (
                    SimWorld {
                        world: w,
                        generation: 0,
                    },
                    g,
                );
            }
        }
        panic!("no room for the grid");
    }

    fn grid_ops(g: &GridFrame) -> Vec<Op> {
        g.streets()
            .into_iter()
            .map(|curve| {
                Op::Cmd(Command::BuildCurve {
                    curve,
                    kind: RoadKind::Street,
                })
            })
            .collect()
    }

    fn grid_pending(g: GridFrame) -> Pending {
        Pending::Road {
            ops: grid_ops(&g),
            curves: g.streets(),
            kind: RoadKind::Street,
            grid: Some(g),
        }
    }

    #[test]
    fn a_large_grid_pending_never_holds_up_a_frame() {
        // The owner's case: a grid of a hundred blocks hanging on the
        // cursor while the camera moves, so its depth changes from frame to
        // frame. Checking it takes many frames; the frame must not wait for
        // it (only the first frame of a plan waits, at most
        // `planner::WAIT`).
        let (sim, g) = grown_city_and_grid();
        let started = std::time::Instant::now();
        let (want, _) = plan::evaluate(&plan::scratch(&sim.world), &grid_ops(&g));
        let plain_ms = started.elapsed().as_secs_f32() * 1e3;
        assert!(want.first_error().is_none());
        // Generous for CI's two-core runner; about 30 ms on a desktop.
        assert!(plain_ms < 1_500.0, "checking the grid took {plain_ms:.0} ms");
        let mut state = ToolState::default();
        let mut worst = 0.0f32;
        for frame in 0..40 {
            let nv = 11 + (frame / 3) % 4;
            let t = std::time::Instant::now();
            refresh_plan(
                &mut state,
                &sim,
                grid_pending(GridFrame { nv, ..g }),
                frame as f32 / 60.0,
            );
            worst = worst.max(t.elapsed().as_secs_f32() * 1e3);
        }
        assert!(worst < 25.0, "a frame spent {worst:.1} ms on the plan");
        // Held still, the plan arrives and is exactly the plain one.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            refresh_plan(&mut state, &sim, grid_pending(g), 1.0);
            if !state.plan.stale && state.plan.pending == grid_pending(g) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the plan never arrived");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(state.plan.eval, want);
        assert_eq!(state.plan.pieces, want.pieces());
    }
}
