//! The tool state machine. Reads the mouse, keys and the ground point
//! under the cursor; writes `ActiveTool`, `ToolModes`, `RoadChoice`, the
//! previews gfx draws (`ToolPreview`, `PathPreview`, `SnapFeedback`,
//! `WorldHighlight`) and `SimCommand`s.
//!
//! * Road, path mode: each click adds a corner (snapped, see `snap.rs`);
//!   double-click, Enter, right-click or clicking the first corner again
//!   builds the whole path as one action; Backspace drops the last corner,
//!   Escape cancels. A drag from a fresh start builds that one straight road
//!   at once; a drag inside a path adds a corner where it is released.
//! * Road, block mode: drag a rectangle; release builds its outline and,
//!   with a block size, the streets inside it ([ and ] change the size).
//! * Zone: rectangle (drag), frontage (click a road side for the whole
//!   stretch between junctions, or drag along a road), fill (click inside a
//!   block enclosed by roads).
//! * Bulldoze: rectangle, or road (click a stretch, drag along a road).
//! * Tab cycles the current tool's modes, Alt holds snapping off, Ctrl+Z
//!   undoes the last road or zoning action (`plan::UndoRecord`).
//!
//! A plan is sent only when every command in it is accepted on a scratch
//! copy of the world, so a path is never built in part.
//!
//! Keys: I inspect, R road (again: street / avenue), Z zone (cycles
//! R/C/I/de-zone), B bulldoze, J junctions.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::{BuildingId, Cell, CellRect, Command, CommandError, Occupant, World, Zone};

use super::UiState;
use super::plan::{self, Evaluated, RoadCells, UndoRecord};
use super::snap::{self, RoadIndex, Snap};
use super::toasts::{Toast, ToastKind};
use super::widgets::UiAction;
use crate::shared::{
    ActiveTool, BulldozeMode, GameSet, HoveredCell, PathPreview, PointerOverUi, PreviewKind, RoadChoice, RoadMode,
    SimCommand, SimWorld, SnapFeedback, SnapTarget, ToolModes, ToolPreview, WorldCamera, WorldHighlight, WorldTarget,
    ZoneMode,
};

/// What the current tool is in the middle of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    Idle,
    /// A road path is being drawn (corners in `ToolState::path`). `held`:
    /// the button is still down from the press that placed the last corner;
    /// `dragged`: the pointer moved to another point while it was.
    Path { held: bool, dragged: bool },
    /// Rectangle drag from `start` (button held): zoning, bulldozer, block.
    Rect { start: Cell },
    /// Drag along a road from road cell `start` (frontage zoning, road
    /// bulldozing); the press point is `ToolState::press_point`.
    Along { start: Cell },
}

/// Block sizes the block tool offers (cells between street centre lines;
/// `None` = outline only). 5 leaves two rows of lots on each side.
pub const BLOCK_SIZES: [Option<i32>; 6] = [None, Some(4), Some(5), Some(6), Some(7), Some(9)];

/// Max cells of an enclosed block the fill tool zones.
const FILL_LIMIT: usize = 6000;
/// Double-click window, seconds.
const DOUBLE_CLICK_S: f32 = 0.4;
/// Undo steps kept.
const UNDO_DEPTH: usize = 12;

/// What the tool would do right now.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Pending {
    #[default]
    None,
    /// Roads: a path (`points`, the first `placed` of them fixed) or a block
    /// grid (`block`).
    Roads {
        points: Vec<Cell>,
        placed: usize,
        legs: Vec<(Cell, Cell)>,
        block: Option<CellRect>,
    },
    Zone {
        cmds: Vec<Command>,
        rect: Option<CellRect>,
        zone: Option<Zone>,
        /// Road run the frontage follows (for the highlight).
        run: Vec<Cell>,
        /// Why there is nothing to do (no road under the cursor, open land).
        empty: Option<&'static str>,
    },
    Bulldoze {
        cmds: Vec<Command>,
        rect: Option<CellRect>,
        run: Vec<Cell>,
        empty: Option<&'static str>,
    },
}

/// The evaluated pending action.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolPlan {
    pub pending: Pending,
    pub eval: Evaluated,
    pub legs: Vec<crate::shared::PathLeg>,
    pub length_m: f32,
    pub doomed_buildings: Vec<BuildingId>,
    pub doomed_road: Vec<Cell>,
}

impl ToolPlan {
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
    pub action: Action,
    /// Last on-map hovered cell (used while the pointer is over UI / off map).
    pub last_cell: Option<Cell>,
    /// Ground point under the cursor, sim metres (last one on the map).
    pub cursor: Option<[f32; 2]>,
    /// Snap radius in metres (about 18 screen pixels).
    pub radius: f32,
    /// Placed corners of the road path being drawn.
    pub path: Vec<Cell>,
    /// Current snap of the road tools (and the road modes).
    pub snap: Option<Snap>,
    /// Snapping held off (Alt).
    pub snap_off: bool,
    /// Where the press that started an `Along` drag was (sim metres).
    pub press_point: Option<[f32; 2]>,
    /// Index into `BLOCK_SIZES`.
    pub block_size: usize,
    last_press: Option<(f32, Cell)>,
    pub undo: Vec<UndoRecord>,
    /// The current plan and the inputs it came from.
    pub plan: ToolPlan,
    plan_key: Option<(Pending, RoadChoice, WorldKey)>,
    /// Road graph on the lattice, for snapping and stretches.
    pub index: RoadIndex,
    pub road_cells: RoadCells,
    index_key: Option<(u64, u64)>,
    scratch: Option<(WorldKey, World)>,
    /// Command the last click sent, so the result can be matched.
    pub sent: Option<Command>,
    /// A multi-command plan was sent this frame (its parts' results are
    /// summarised by the tool, not toasted one by one).
    pub batch_frame: bool,
}

impl ToolState {
    pub fn busy(&self) -> bool {
        self.action != Action::Idle
    }

    pub fn block_spacing(&self) -> Option<i32> {
        BLOCK_SIZES[self.block_size.min(BLOCK_SIZES.len() - 1)]
    }

    fn scratch_for(&mut self, sim: &SimWorld) -> &World {
        let key = world_key(sim);
        if self.scratch.as_ref().is_none_or(|(k, _)| *k != key) {
            self.scratch = Some((key, plan::scratch(&sim.world)));
        }
        &self.scratch.as_ref().expect("just set").1
    }

    fn reset(&mut self) {
        self.action = Action::Idle;
        self.path.clear();
        self.press_point = None;
    }
}

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolState>().add_systems(
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
                RoadMode::Path => RoadMode::Block,
                RoadMode::Block => RoadMode::Junction,
                RoadMode::Junction => RoadMode::Path,
            }
        }
        ActiveTool::Zone(_) => {
            m.zone = match modes.zone {
                ZoneMode::Rect => ZoneMode::Frontage,
                ZoneMode::Frontage => ZoneMode::Fill,
                ZoneMode::Fill => ZoneMode::Rect,
            }
        }
        ActiveTool::Bulldoze => {
            m.bulldoze = match modes.bulldoze {
                BulldozeMode::Rect => BulldozeMode::Road,
                BulldozeMode::Road => BulldozeMode::Rect,
            }
        }
        ActiveTool::Inspect => {}
    }
    m
}

#[allow(clippy::too_many_arguments)]
fn tool_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ActiveTool>,
    mut modes: ResMut<ToolModes>,
    mut kind: ResMut<RoadChoice>,
    mut state: ResMut<ToolState>,
    mut ui: ResMut<UiState>,
) {
    if !ui.menu_open && !ctrl(&keys) {
        let road_drawing = *tool == ActiveTool::Road && modes.road != RoadMode::Junction;
        let new = if keys.just_pressed(KeyCode::KeyI) {
            Some(ActiveTool::Inspect)
        } else if keys.just_pressed(KeyCode::KeyR) {
            if road_drawing {
                kind.set_if_neq(match *kind {
                    RoadChoice::Street => RoadChoice::Avenue,
                    RoadChoice::Avenue => RoadChoice::Street,
                });
            } else if *tool == ActiveTool::Road {
                modes.road = RoadMode::Path;
            }
            Some(ActiveTool::Road)
        } else if keys.just_pressed(KeyCode::KeyJ) {
            modes.road = RoadMode::Junction;
            Some(ActiveTool::Road)
        } else if keys.just_pressed(KeyCode::KeyB) {
            Some(ActiveTool::Bulldoze)
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
        if *tool == ActiveTool::Road && modes.road == RoadMode::Block {
            let n = BLOCK_SIZES.len();
            if keys.just_pressed(KeyCode::BracketRight) {
                state.block_size = (state.block_size + 1).min(n - 1);
            }
            if keys.just_pressed(KeyCode::BracketLeft) {
                state.block_size = state.block_size.saturating_sub(1);
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
}

/// Ground point (sim metres) under a window position.
fn ground_at(camera: &Camera, cam_tf: &GlobalTransform, pos: Vec2) -> Option<[f32; 2]> {
    let ray = camera.viewport_to_world(cam_tf, pos).ok()?;
    let hit = ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))?;
    Some(crate::shared::world_to_sim(hit))
}

/// Screen pixels the snap radius spans.
const SNAP_PX: f32 = 18.0;

#[allow(clippy::too_many_arguments)]
fn track_pointer(
    hovered: Res<HoveredCell>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    sim: Res<SimWorld>,
    mut state: ResMut<ToolState>,
) {
    let key = (sim.generation, sim.world.versions.roads);
    if state.index_key != Some(key) {
        state.index_key = Some(key);
        state.index = RoadIndex::build(&sim.world);
        state.road_cells = RoadCells::build(&state.index);
    }
    if hovered.0.is_some() {
        state.last_cell = hovered.0;
        if let Ok(w) = window.single()
            && let Some(pos) = w.cursor_position()
            && let Ok((cam, tf)) = cameras.single()
            && let Some(p) = ground_at(cam, tf, pos)
        {
            state.cursor = Some(p);
            if let Some(q) = ground_at(cam, tf, pos + Vec2::new(SNAP_PX, 0.0)) {
                state.radius = citysim_sim::geom::dist(p, q).clamp(5.0, 48.0);
            }
        } else if let Some(c) = hovered.0 {
            // No camera (ui-only build): the cell centre.
            state.cursor = Some(c.center());
            state.radius = 10.0;
        }
    }
    state.snap_off = alt(&keys);
    let enabled = !state.snap_off;
    let (Some(cursor), true) = (state.cursor, hovered.0.is_some() || state.busy()) else {
        state.snap = None;
        return;
    };
    let radius = state.radius;
    let snap = match *tool {
        ActiveTool::Road => match modes.road {
            RoadMode::Path => Some(match state.path.last() {
                Some(&from) => snap::snap_leg(&state.index, from, cursor, radius, enabled),
                None => snap::snap_point(&state.index, cursor, radius, enabled),
            }),
            RoadMode::Block => Some(match state.action {
                Action::Rect { start } => {
                    let raw = Cell::from_pos(cursor);
                    let spacing = if enabled { state.block_spacing() } else { None };
                    let c = plan::block_corner(start, raw, spacing);
                    let mut s = snap::snap_point(&state.index, c.center(), 0.0, false);
                    s.cell = c;
                    s.target = if spacing.is_some() {
                        SnapTarget::Spacing
                    } else {
                        SnapTarget::Grid
                    };
                    s.spacing = spacing;
                    s
                }
                _ => snap::snap_point(&state.index, cursor, radius, enabled),
            }),
            RoadMode::Junction => None,
        },
        ActiveTool::Zone(_) if modes.zone == ZoneMode::Frontage => road_snap(&state, cursor, radius),
        ActiveTool::Bulldoze if modes.bulldoze == BulldozeMode::Road => road_snap(&state, cursor, radius),
        _ => None,
    };
    state.snap = snap;
}

/// Snap to the nearest road cell within reach (road modes), else none.
fn road_snap(state: &ToolState, cursor: [f32; 2], radius: f32) -> Option<Snap> {
    let s = snap::snap_point(&state.index, cursor, radius.max(10.0), true);
    let c = Cell::from_pos(cursor);
    if matches!(s.target, SnapTarget::Node | SnapTarget::Road) {
        Some(s)
    } else if state.road_cells.is_road(c) {
        let mut s = s;
        s.cell = c;
        s.target = SnapTarget::Road;
        s.guides.clear();
        Some(s)
    } else {
        None
    }
}

/// What the tool would do now, from the tool state and the world.
pub fn pending(tool: ActiveTool, modes: ToolModes, state: &ToolState, hovered: Option<Cell>, world: &World) -> Pending {
    let cell = hovered.or(state.last_cell.filter(|_| state.busy()));
    let snapped = state.snap.as_ref().map(|s| s.cell);
    match tool {
        ActiveTool::Inspect => Pending::None,
        ActiveTool::Road => match modes.road {
            RoadMode::Path => {
                if state.path.is_empty() {
                    return Pending::None;
                }
                let mut points = state.path.clone();
                let placed = points.len();
                if let Some(c) = snapped.filter(|_| hovered.is_some() || state.busy()) {
                    points.push(c);
                }
                if points.is_empty() {
                    return Pending::None;
                }
                let legs = plan::legs_of(&points);
                Pending::Roads {
                    points,
                    placed,
                    legs,
                    block: None,
                }
            }
            RoadMode::Block => match (state.action, snapped) {
                (Action::Rect { start }, Some(end)) => Pending::Roads {
                    points: Vec::new(),
                    placed: 0,
                    legs: plan::block_legs(start, end, state.block_spacing()),
                    block: Some(CellRect::from_corners(start, end)),
                },
                _ => Pending::None,
            },
            RoadMode::Junction => Pending::None,
        },
        ActiveTool::Zone(zone) => {
            let Some(c) = cell else { return Pending::None };
            match modes.zone {
                ZoneMode::Rect => {
                    let start = match state.action {
                        Action::Rect { start } => start,
                        _ => c,
                    };
                    Pending::Zone {
                        cmds: vec![Command::SetZone {
                            min: start,
                            max: c,
                            zone,
                        }],
                        rect: Some(CellRect::from_corners(start, c)),
                        zone,
                        run: Vec::new(),
                        empty: None,
                    }
                }
                ZoneMode::Frontage => {
                    let Some((run, side)) = road_run(state, snapped) else {
                        return Pending::Zone {
                            cmds: Vec::new(),
                            rect: None,
                            zone,
                            run: Vec::new(),
                            empty: Some("Point at a road to zone along it"),
                        };
                    };
                    let cells = plan::frontage(&run, side, plan::LOT_DEPTH, &state.road_cells);
                    Pending::Zone {
                        cmds: plan::zone_commands(&cells, zone),
                        rect: None,
                        zone,
                        run,
                        empty: None,
                    }
                }
                ZoneMode::Fill => {
                    let block = plan::enclosed_block(c, &state.road_cells, FILL_LIMIT);
                    let empty = match block {
                        None if state.road_cells.is_road(c) => Some("Point inside a block, not at a road"),
                        None => Some("This land isn't enclosed by roads"),
                        Some(_) => None,
                    };
                    let cells = block.unwrap_or_default();
                    Pending::Zone {
                        cmds: plan::zone_commands(&cells, zone),
                        rect: None,
                        zone,
                        run: Vec::new(),
                        empty,
                    }
                }
            }
        }
        ActiveTool::Bulldoze => {
            let Some(c) = cell else { return Pending::None };
            match modes.bulldoze {
                BulldozeMode::Rect => {
                    let start = match state.action {
                        Action::Rect { start } => start,
                        _ => c,
                    };
                    Pending::Bulldoze {
                        cmds: vec![Command::Bulldoze { min: start, max: c }],
                        rect: Some(CellRect::from_corners(start, c)),
                        run: Vec::new(),
                        empty: None,
                    }
                }
                BulldozeMode::Road => {
                    let Some((run, _)) = road_run(state, snapped) else {
                        let _ = world;
                        return Pending::Bulldoze {
                            cmds: Vec::new(),
                            rect: None,
                            run: Vec::new(),
                            empty: Some("Point at a road to remove it"),
                        };
                    };
                    let partial = matches!(state.action, Action::Along { start } if Some(start) != snapped);
                    let cells = plan::bulldoze_run_cells(&run, &state.road_cells, partial);
                    let empty = cells.is_empty().then_some(if partial {
                        "Drag further: nothing between these points"
                    } else {
                        "Too short to remove alone; use the rectangle"
                    });
                    Pending::Bulldoze {
                        cmds: plan::bulldoze_commands(&cells),
                        rect: None,
                        run,
                        empty,
                    }
                }
            }
        }
    }
}

/// The road run a road-mode tool works on and the side of it the press (or
/// cursor) is on: the stretch under the cursor, or while dragging along a
/// road, the cells from the press to the cursor.
fn road_run(state: &ToolState, snapped: Option<Cell>) -> Option<(Vec<Cell>, i32)> {
    let rc = &state.road_cells;
    let point = state.press_point.or(state.cursor)?;
    let run = match (state.action, snapped) {
        (Action::Along { start }, Some(end)) if end != start => rc.route(start, end, 4000)?,
        (Action::Along { start }, _) => rc.stretch_cells(start)?,
        (_, Some(c)) => rc.stretch_cells(c).or_else(|| {
            // On a junction: the arm the cursor is over.
            let cur = Cell::from_pos(state.cursor?);
            rc.stretch_cells(cur)
        })?,
        _ => return None,
    };
    if run.len() < 2 {
        return None;
    }
    let side = plan::side_of(&run, point);
    Some((run, side))
}

/// Evaluate a pending action on the scratch world.
pub fn evaluate(base: &World, pending: &Pending, kind: RoadChoice) -> ToolPlan {
    let mut out = ToolPlan {
        pending: pending.clone(),
        ..default()
    };
    match pending {
        Pending::None => {}
        Pending::Roads { legs, .. } => {
            let p = plan::plan_roads(base, legs, kind);
            out.eval = p.eval;
            out.legs = p.legs;
            out.length_m = p.length_m;
        }
        Pending::Zone { cmds, .. } => {
            out.eval = plan::evaluate(base, cmds.clone()).0;
        }
        Pending::Bulldoze { cmds, run, .. } => {
            let (eval, after) = plan::evaluate(base, cmds.clone());
            let mut cells = eval.cells.clone();
            cells.extend(run.iter().copied());
            out.doomed_road = plan::cleared_road_cells(base, &after, &cells);
            out.doomed_buildings = base
                .buildings
                .keys()
                .filter(|&id| !after.buildings.contains_key(id))
                .collect();
            out.eval = eval;
        }
    }
    out
}

fn refresh_plan(state: &mut ToolState, sim: &SimWorld, pending: Pending, kind: RoadChoice) {
    let key = (pending, kind, world_key(sim));
    if state.plan_key.as_ref() == Some(&key) {
        return;
    }
    let base = state.scratch_for(sim).clone();
    state.plan = evaluate(&base, &key.0, kind);
    state.plan_key = Some(key);
}

/// Send a plan (only when every command is accepted). Returns the toast to
/// show when it is refused or does nothing.
fn commit(
    state: &mut ToolState,
    sim: &SimWorld,
    kind: RoadChoice,
    out: &mut MessageWriter<SimCommand>,
) -> Option<Toast> {
    let pending = state.plan.pending.clone();
    let base = state.scratch_for(sim).clone();
    let plan = evaluate(&base, &pending, kind);
    if plan.eval.cmds.is_empty() {
        return match &pending {
            Pending::Zone { empty: Some(e), .. } | Pending::Bulldoze { empty: Some(e), .. } => {
                Some(Toast::new(ToastKind::Info, *e))
            }
            _ => None,
        };
    }
    if let Some(e) = plan.error() {
        return Some(Toast::new(ToastKind::Error, super::context::error_text(e)));
    }
    // Single rectangle commands keep round-1 behaviour (and its toasts).
    let single = plan.eval.cmds.len() == 1 && matches!(pending, Pending::Zone { rect: Some(_), .. });
    let mut toast = None;
    match &pending {
        Pending::Roads { legs, block, .. } => {
            let (_, after) = plan::evaluate(&base, plan.eval.cmds.clone());
            let label = if block.is_some() {
                "block of streets".to_string()
            } else if legs.len() > 1 {
                format!("{}-part road", legs.len())
            } else {
                "road".to_string()
            };
            match plan::undo_for_roads(&base, &after, label) {
                Ok(rec) => push_undo(state, rec),
                Err(why) => debug!("road not undoable: {why}"),
            }
        }
        Pending::Zone { zone, .. } => {
            if plan.eval.cells.is_empty() {
                if !single {
                    toast = Some(Toast::new(
                        ToastKind::Info,
                        if zone.is_some() {
                            "Nothing to zone here: already zoned, built on or too far from a road"
                        } else {
                            "Nothing to unzone here"
                        },
                    ));
                }
            } else {
                let what = super::widgets::zone_name(*zone).to_lowercase();
                let label = if zone.is_some() {
                    format!("{what} zoning")
                } else {
                    "de-zoning".into()
                };
                push_undo(state, plan::undo_for_zones(&base, &plan.eval.cells, label));
            }
        }
        _ => {}
    }
    state.batch_frame = !single;
    for &cmd in &plan.eval.cmds {
        state.sent = Some(cmd);
        info!("tool sends {cmd:?}");
        out.write(SimCommand(cmd));
    }
    toast
}

fn push_undo(state: &mut ToolState, rec: UndoRecord) {
    state.undo.push(rec);
    if state.undo.len() > UNDO_DEPTH {
        state.undo.remove(0);
    }
}

fn undo(state: &mut ToolState, sim: &SimWorld, out: &mut MessageWriter<SimCommand>) -> Toast {
    let Some(rec) = state.undo.pop() else {
        return Toast::new(ToastKind::Info, "Nothing to undo");
    };
    let label = rec.label().to_string();
    let cmds = match plan::undo_commands(&sim.world, &rec) {
        Ok(c) => c,
        Err(why) => return Toast::new(ToastKind::Warning, format!("Can't undo the {label}: {why}")),
    };
    let base = state.scratch_for(sim).clone();
    let (eval, _) = plan::evaluate(&base, cmds.clone());
    if let Some(e) = eval.first_error() {
        return Toast::new(
            ToastKind::Warning,
            format!("Can't undo the {label}: {}", super::context::error_text(e)),
        );
    }
    state.batch_frame = true;
    for cmd in cmds {
        out.write(SimCommand(cmd));
    }
    let fee = if eval.cost > 0 {
        format!(" (demolition {}, no refund)", super::format::money(eval.cost))
    } else {
        String::new()
    };
    Toast::new(ToastKind::Success, format!("Undid the {label}{fee}"))
}

#[allow(clippy::too_many_arguments)]
fn use_tool(
    time: Res<Time<Real>>,
    hovered: Res<HoveredCell>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    kind: Res<RoadChoice>,
    ui: Res<UiState>,
    sim: Res<SimWorld>,
    mut actions: MessageReader<UiAction>,
    mut state: ResMut<ToolState>,
    mut out: MessageWriter<SimCommand>,
    mut toasts: MessageWriter<Toast>,
) {
    state.batch_frame = false;
    let state = state.as_mut();
    let undo_asked = actions.read().any(|a| *a == UiAction::Undo);
    if mouse.just_pressed(MouseButton::Left) || mouse.just_released(MouseButton::Left) {
        info!(
            "tool mouse pressed={} released={} hovered={:?} snap={:?} action={:?} path={:?}",
            mouse.just_pressed(MouseButton::Left),
            mouse.just_released(MouseButton::Left),
            hovered.0,
            state.snap.as_ref().map(|s| (s.cell, s.target)),
            state.action,
            state.path
        );
    }
    if ui.menu_open {
        state.reset();
        return;
    }
    if undo_asked || ctrl(&keys) && keys.just_pressed(KeyCode::KeyZ) {
        state.reset();
        toasts.write(undo(state, &sim, &mut out));
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        state.reset();
        return;
    }
    let now = time.elapsed_secs();
    let pressed = mouse.just_pressed(MouseButton::Left) && hovered.0.is_some();
    let down = mouse.pressed(MouseButton::Left);
    let right = mouse.just_pressed(MouseButton::Right);
    let snapped = state.snap.as_ref().map(|s| s.cell);
    // Fresh plan for what the pointer points at now.
    let p = pending(*tool, *modes, state, hovered.0, &sim.world);
    refresh_plan(state, &sim, p, *kind);
    let mut fire = false;
    match (*tool, modes.road) {
        (ActiveTool::Inspect, _) => {}
        (ActiveTool::Road, RoadMode::Path) => {
            let finish = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) || right;
            if keys.just_pressed(KeyCode::Backspace) {
                state.path.pop();
                if state.path.is_empty() {
                    state.reset();
                }
            } else if finish && !state.path.is_empty() {
                if state.path.len() >= 2 {
                    finish_path(state, &sim, *kind, &mut out, &mut toasts);
                } else {
                    state.reset();
                }
            } else {
                match state.action {
                    Action::Idle => {
                        if pressed && let Some(c) = snapped {
                            state.path = vec![c];
                            state.action = Action::Path {
                                held: down,
                                dragged: false,
                            };
                            state.last_press = Some((now, c));
                        }
                    }
                    Action::Path { held: true, dragged } => {
                        let last = *state.path.last().expect("path has a start");
                        let moved = snapped.is_some_and(|c| c != last);
                        if down {
                            if moved && !dragged {
                                state.action = Action::Path {
                                    held: true,
                                    dragged: true,
                                };
                            }
                        } else if dragged && moved {
                            // Drag released on a new point.
                            state.path.push(snapped.expect("moved"));
                            if state.path.len() == 2 {
                                finish_path(state, &sim, *kind, &mut out, &mut toasts);
                            } else {
                                state.action = Action::Path {
                                    held: false,
                                    dragged: false,
                                };
                            }
                        } else {
                            state.action = Action::Path {
                                held: false,
                                dragged: false,
                            };
                        }
                    }
                    Action::Path { held: false, .. } => {
                        if pressed && let Some(c) = snapped {
                            let last = *state.path.last().expect("path has a start");
                            let double = state
                                .last_press
                                .is_some_and(|(t, pc)| now - t < DOUBLE_CLICK_S && pc == c);
                            state.last_press = Some((now, c));
                            if c == last || double {
                                if state.path.len() >= 2 {
                                    finish_path(state, &sim, *kind, &mut out, &mut toasts);
                                } else {
                                    state.reset();
                                }
                            } else if c == state.path[0] && state.path.len() >= 2 {
                                // Closing a loop finishes it.
                                state.path.push(c);
                                finish_path(state, &sim, *kind, &mut out, &mut toasts);
                            } else {
                                state.path.push(c);
                                state.action = Action::Path {
                                    held: down,
                                    dragged: false,
                                };
                            }
                        }
                    }
                    _ => state.reset(),
                }
            }
        }
        (ActiveTool::Road, RoadMode::Junction) => {}
        // Rectangle modes: zoning, bulldozer, block.
        (t, _)
            if matches!(t, ActiveTool::Road)
                || matches!(t, ActiveTool::Zone(_)) && modes.zone == ZoneMode::Rect
                || t == ActiveTool::Bulldoze && modes.bulldoze == BulldozeMode::Rect =>
        {
            if right {
                state.reset();
            } else {
                match state.action {
                    Action::Idle => {
                        if pressed && let Some(c) = hovered.0 {
                            let start = if t == ActiveTool::Road { snapped.unwrap_or(c) } else { c };
                            if down {
                                state.action = Action::Rect { start };
                            } else if t != ActiveTool::Road {
                                // Pressed and released within one frame: a click.
                                fire = true;
                            }
                        }
                    }
                    Action::Rect { .. } => {
                        if !down {
                            fire = true;
                        }
                    }
                    _ => state.reset(),
                }
            }
        }
        // Road modes (frontage zoning, road bulldozing) and fill.
        (t, _) => {
            let along = matches!(t, ActiveTool::Zone(_)) && modes.zone == ZoneMode::Frontage
                || t == ActiveTool::Bulldoze && modes.bulldoze == BulldozeMode::Road;
            if right {
                state.reset();
            } else if along {
                match state.action {
                    Action::Idle => {
                        if pressed && let Some(c) = snapped {
                            state.press_point = state.cursor;
                            if down {
                                state.action = Action::Along { start: c };
                            } else {
                                fire = true;
                            }
                        }
                    }
                    Action::Along { .. } => {
                        if !down {
                            fire = true;
                        }
                    }
                    _ => state.reset(),
                }
            } else if pressed {
                fire = true;
            }
        }
    }
    if fire {
        // The plan for the release position.
        let p = pending(*tool, *modes, state, hovered.0, &sim.world);
        refresh_plan(state, &sim, p, *kind);
        if let Some(t) = commit(state, &sim, *kind, &mut out) {
            toasts.write(t);
        }
        state.reset();
    }
}

fn finish_path(
    state: &mut ToolState,
    sim: &SimWorld,
    kind: RoadChoice,
    out: &mut MessageWriter<SimCommand>,
    toasts: &mut MessageWriter<Toast>,
) {
    let points = state.path.clone();
    let p = Pending::Roads {
        legs: plan::legs_of(&points),
        placed: points.len(),
        points,
        block: None,
    };
    refresh_plan(state, sim, p, kind);
    match commit(state, sim, kind, out) {
        Some(t) => {
            // Refused: keep the path so the player can fix it.
            toasts.write(t);
            state.action = Action::Path {
                held: false,
                dragged: false,
            };
        }
        None => state.reset(),
    }
}

/// What the inspect / junction tools point at on `cell`.
pub fn target_at(world: &World, state: &ToolState, cell: Cell) -> Option<WorldTarget> {
    let cd = world.grid.get(cell)?;
    match cd.occupant {
        Occupant::Building(id) => Some(WorldTarget::Building(id)),
        Occupant::Road => {
            if state.road_cells.is_junction(cell)
                && let Some(n) = world.roads.node_at(cell.center())
            {
                return Some(WorldTarget::Junction {
                    node: n,
                    pos: cell.center(),
                });
            }
            let run = state.road_cells.stretch_cells(cell)?;
            Some(WorldTarget::Road(plan::stretch_info(world, &run)))
        }
        Occupant::Empty => None,
    }
}

type HoverKey = (Option<Cell>, (u64, u64, u64));

/// Inspect selection (pinned by a click) lives in `WorldHighlight::selected`.
#[allow(clippy::too_many_arguments)]
fn publish(
    hovered: Res<HoveredCell>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    over_ui: Res<PointerOverUi>,
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    kind: Res<RoadChoice>,
    sim: Res<SimWorld>,
    ui: Res<UiState>,
    mut state: ResMut<ToolState>,
    mut preview: ResMut<ToolPreview>,
    mut path: ResMut<PathPreview>,
    mut snapfb: ResMut<SnapFeedback>,
    mut highlight: ResMut<WorldHighlight>,
    mut hover_key: Local<Option<HoverKey>>,
    mut hover_target: Local<Option<WorldTarget>>,
) {
    let state = state.as_mut();
    if ui.menu_open {
        preview.set_if_neq(ToolPreview::default());
        path.set_if_neq(PathPreview::default());
        snapfb.set_if_neq(SnapFeedback::default());
        return;
    }
    let p = pending(*tool, *modes, state, hovered.0, &sim.world);
    refresh_plan(state, &sim, p, *kind);
    let plan = &state.plan;
    // Compatible single-ghost preview.
    let mut tp = ToolPreview::default();
    let mut pp = PathPreview {
        kind: *kind,
        ..default()
    };
    let error = plan.error().cloned();
    match &plan.pending {
        Pending::None => {}
        Pending::Roads {
            points, placed, legs, ..
        } => {
            pp.points = points.clone();
            pp.placed = *placed;
            pp.legs = plan.legs.clone();
            pp.length_m = plan.length_m;
            pp.cost = plan.eval.cost;
            pp.error = error.clone();
            if let Some(&(a, b)) = legs.last() {
                tp.kind = PreviewKind::Road;
                tp.line = Some((a, b));
                tp.cells = a.straight_line_to(b).unwrap_or_default();
                tp.error = plan.legs.last().and_then(|l| l.error.clone()).or(error.clone());
                tp.cost = plan.eval.cost;
            }
        }
        Pending::Zone { rect, zone, .. } => {
            tp.kind = PreviewKind::Zone(*zone);
            tp.rect = *rect;
            tp.cells = plan.eval.cells.clone();
        }
        Pending::Bulldoze { rect, .. } => {
            tp.kind = PreviewKind::Bulldoze;
            tp.rect = *rect;
            tp.cells = plan.eval.cells.clone();
            tp.cost = plan.eval.cost;
            tp.error = error;
        }
    }
    preview.set_if_neq(tp);
    path.set_if_neq(pp);
    let fb = match &state.snap {
        Some(s) => SnapFeedback {
            point: Some(s.cell.center()),
            cell: Some(s.cell),
            target: s.target,
            matched: s.matched.clone(),
            guides: s.guides.clone(),
            spacing_cells: s.spacing,
            suspended: state.snap_off,
        },
        None => SnapFeedback::default(),
    };
    snapfb.set_if_neq(fb);

    // Hover / selection highlight.
    let w = &sim.world;
    let vkey = (sim.generation, w.versions.roads, w.versions.buildings);
    let wants_hover = match *tool {
        ActiveTool::Inspect => true,
        ActiveTool::Road => modes.road == RoadMode::Junction,
        _ => false,
    };
    let cell = hovered.0.filter(|_| wants_hover && !over_ui.0);
    if *hover_key != Some((cell, vkey)) {
        *hover_key = Some((cell, vkey));
        *hover_target = cell.and_then(|c| target_at(w, state, c));
        if *tool != ActiveTool::Inspect {
            // Junction tool: only junctions.
            if !matches!(*hover_target, Some(WorldTarget::Junction { .. })) {
                *hover_target = None;
            }
        }
    }
    let mut hl = WorldHighlight {
        hovered: hover_target.clone(),
        selected: highlight.selected.clone(),
        doomed_buildings: plan.doomed_buildings.clone(),
        doomed_road_cells: plan.doomed_road.clone(),
    };
    if let Pending::Zone { run, .. } | Pending::Bulldoze { run, .. } = &plan.pending
        && run.len() >= 2
    {
        hl.hovered = Some(WorldTarget::Road(plan::stretch_info(w, run)));
    }
    // Inspect: a click pins what is under the cursor (or clears it).
    if *tool == ActiveTool::Inspect {
        if mouse.just_pressed(MouseButton::Left) && !over_ui.0 {
            hl.selected = hover_target.clone();
        }
        if keys.just_pressed(KeyCode::Escape) {
            hl.selected = None;
        }
    }
    // A pinned thing that no longer exists is dropped.
    hl.selected = hl.selected.filter(|t| target_exists(w, t));
    highlight.set_if_neq(hl);
}

fn target_exists(world: &World, t: &WorldTarget) -> bool {
    match t {
        WorldTarget::Building(id) => world.buildings.contains_key(*id),
        WorldTarget::Road(s) => s.segments.iter().all(|&id| world.roads.segments.contains_key(id)),
        WorldTarget::Junction { node, .. } => world.roads.nodes.get(*node).is_some_and(|n| n.segments.len() >= 3),
    }
}

/// Preview of a single command (tests).
#[cfg(test)]
pub fn compute_preview(w: &World, cmd: Command) -> ToolPreview {
    let result = w.preview(&cmd);
    let mut p = ToolPreview::default();
    match cmd {
        Command::BuildRoad { from, to } => {
            p.kind = PreviewKind::Road;
            p.line = Some((from, to));
        }
        Command::SetZone { min, max, zone } => {
            p.kind = PreviewKind::Zone(zone);
            p.rect = Some(CellRect::from_corners(min, max));
        }
        Command::Bulldoze { min, max } => {
            p.kind = PreviewKind::Bulldoze;
            p.rect = Some(CellRect::from_corners(min, max));
        }
        Command::SetTaxRate(_) => {}
    }
    match result {
        Ok(r) => {
            p.cells = r.cells;
            p.cost = r.cost;
        }
        Err(e) => {
            if let Some((from, to)) = p.line {
                p.cells = from.straight_line_to(to).unwrap_or_default();
            }
            p.error = Some(e);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_cycles_every_mode_and_returns() {
        let mut m = ToolModes::default();
        for _ in 0..3 {
            m = next_mode(ActiveTool::Road, m);
        }
        assert_eq!(m, ToolModes::default());
        m = next_mode(ActiveTool::Zone(None), m);
        assert_eq!(m.zone, ZoneMode::Frontage);
        m = next_mode(ActiveTool::Bulldoze, m);
        assert_eq!(m.bulldoze, BulldozeMode::Road);
    }

    fn state_for(w: &World) -> ToolState {
        let index = RoadIndex::build(w);
        let road_cells = RoadCells::build(&index);
        ToolState {
            index,
            road_cells,
            radius: 12.0,
            ..default()
        }
    }

    #[test]
    fn path_pending_follows_the_snapped_end() {
        let w = World::new(1);
        let mut s = state_for(&w);
        s.path = vec![Cell::new(11, 128), Cell::new(20, 128)];
        s.action = Action::Path {
            held: false,
            dragged: false,
        };
        s.cursor = Some(Cell::new(20, 120).center());
        s.snap = Some(snap::snap_leg(
            &s.index,
            Cell::new(20, 128),
            s.cursor.unwrap(),
            12.0,
            true,
        ));
        let p = pending(ActiveTool::Road, ToolModes::default(), &s, Some(Cell::new(20, 120)), &w);
        let Pending::Roads { legs, placed, .. } = &p else {
            panic!("{p:?}")
        };
        assert_eq!(*placed, 2);
        assert_eq!(legs.len(), 2);
        let plan = evaluate(&plan::scratch(&w), &p, RoadChoice::Street);
        assert!(plan.eval.ok(), "{plan:?}");
        assert_eq!(plan.legs.len(), 2);
    }

    #[test]
    fn road_bulldoze_marks_the_stretch_red() {
        let mut w = World::new(1);
        w.economy.money = 1_000_000;
        w.apply(Command::BuildRoad {
            from: Cell::new(11, 128),
            to: Cell::new(30, 128),
        })
        .unwrap();
        w.apply(Command::BuildRoad {
            from: Cell::new(20, 128),
            to: Cell::new(20, 120),
        })
        .unwrap();
        let mut s = state_for(&w);
        s.cursor = Some(Cell::new(25, 128).center());
        s.snap = road_snap(&s, s.cursor.unwrap(), 12.0);
        let modes = ToolModes {
            bulldoze: BulldozeMode::Road,
            ..default()
        };
        let p = pending(ActiveTool::Bulldoze, modes, &s, Some(Cell::new(25, 128)), &w);
        let plan = evaluate(&plan::scratch(&w), &p, RoadChoice::Street);
        assert!(plan.eval.ok());
        // (20, 128) is a junction and stays; the dead end at (30, 128) goes.
        assert!(plan.doomed_road.contains(&Cell::new(30, 128)));
        assert!(!plan.doomed_road.contains(&Cell::new(20, 128)));
        assert_eq!(plan.doomed_road.len(), 10);
    }

    #[test]
    fn frontage_zoning_previews_both_rows_of_one_side() {
        let mut w = World::new(1);
        w.economy.money = 1_000_000;
        w.apply(Command::BuildRoad {
            from: Cell::new(11, 128),
            to: Cell::new(30, 128),
        })
        .unwrap();
        let mut s = state_for(&w);
        // Just north of the road (smaller y).
        let c = Cell::new(20, 128).center();
        s.cursor = Some([c[0], c[1] - 3.0]);
        s.snap = road_snap(&s, s.cursor.unwrap(), 12.0);
        let modes = ToolModes {
            zone: ZoneMode::Frontage,
            ..default()
        };
        let p = pending(
            ActiveTool::Zone(Some(Zone::Residential)),
            modes,
            &s,
            Some(Cell::new(20, 128)),
            &w,
        );
        let plan = evaluate(&plan::scratch(&w), &p, RoadChoice::Street);
        assert!(!plan.eval.cells.is_empty());
        assert!(
            plan.eval.cells.iter().all(|c| c.y == 126 || c.y == 127),
            "{:?}",
            plan.eval.cells
        );
    }
}
