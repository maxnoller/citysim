//! Everything the `gfx` and `ui` halves of the game share — and nothing
//! else. `gfx` and `ui` must never import from each other; they talk only
//! through the resources and types in this file.
//!
//! Ownership (who writes what):
//! * `SimWorld`, `SimClock` stepping, `SimCommand` application, `HoveredCell`:
//!   this module (`SharedPlugin`).
//! * `ActiveTool`, `ToolPreview`, `OverlayMode`, `GraphicsQuality`,
//!   `PointerOverUi`, `CameraInputBlocked`, `SimClock::{paused, speed}`:
//!   written by `ui`.
//! * `WorldCamera` entity (and honouring `CameraOverride`): owned by `gfx`.
//! * `CameraOverride`: written by dev tooling (scripted demo / autoshot).
//! * Round 2, written by ui, drawn by gfx: `RoadChoice`, `ToolModes`,
//!   `SnapFeedback`, `PathPreview`, `WorldHighlight`, `DisplaySettings`.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::{
    BuildingId, CELL_SIZE, Cell, CellRect, Command, CommandError, NodeId, Outcome, SegmentId, TICKS_PER_SECOND, World,
    Zone,
};

// ---------------------------------------------------------------- ordering

/// Frame phases, run in this order every `Update`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameSet {
    /// Read devices: ui sets `PointerOverUi`; dev scripts inject input here.
    Input,
    /// Shared computes `HoveredCell`.
    Hover,
    /// Tools (ui) and camera control (gfx). Tools emit `SimCommand`s.
    Tools,
    /// Shared applies queued `SimCommand`s to the World.
    Sim,
    /// Draw: gfx rebuilds meshes, ui updates the HUD.
    Render,
}

/// `FixedUpdate` set containing `World::step` calls (order after it to
/// observe the stepped world).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SimStep;

// ---------------------------------------------------------------- sim driver

/// The simulation. Read it anywhere; change it via `SimCommand` (or
/// [`SimWorld::replace`] for loading a save / new game).
#[derive(Resource)]
pub struct SimWorld {
    pub world: World,
    /// Bumped whenever `world` is replaced wholesale, so caches keyed on
    /// `world.versions` also notice a reload.
    pub generation: u64,
}

impl SimWorld {
    /// Render-cache key for one of `world.versions`' counters.
    pub fn key(&self, version: u64) -> (u64, u64) {
        (self.generation, version)
    }

    /// Swap in a different world (load / new game).
    pub fn replace(&mut self, world: World) {
        self.world = world;
        self.generation += 1;
    }
}

/// Steps per fixed update for speed levels 1, 2, 3.
pub const SPEEDS: [u32; 3] = [1, 3, 6];

/// Pause and speed. The fixed timestep runs at `TICKS_PER_SECOND`; each
/// fixed update performs `steps_per_tick()` sim steps.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SimClock {
    pub paused: bool,
    /// Index into [`SPEEDS`] (0 = 1x).
    pub speed: usize,
}

impl SimClock {
    pub fn steps_per_tick(&self) -> u32 {
        if self.paused {
            0
        } else {
            SPEEDS[self.speed.min(SPEEDS.len() - 1)]
        }
    }
}

/// A command for the sim; applied in `GameSet::Sim`, result in `LastCommand`.
#[derive(Message, Debug, Clone, Copy)]
pub struct SimCommand(pub Command);

/// Result of the most recently applied `SimCommand`.
#[derive(Resource, Default, Debug)]
pub struct LastCommand {
    pub result: Option<(Command, Result<Outcome, CommandError>)>,
    /// Real time (seconds since start) when it was applied.
    pub at: f32,
}

// ---------------------------------------------------------------- tools & modes

/// The tool the player has selected.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveTool {
    /// Look at things; clicks do not change the world.
    #[default]
    Inspect,
    Road,
    /// `None` removes zoning.
    Zone(Option<Zone>),
    Bulldoze,
}

/// Map cell under the mouse; `None` when off the map, off the window, or
/// over UI (`PointerOverUi`). Computed by shared from the `WorldCamera` ray.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HoveredCell(pub Option<Cell>);

/// True while the mouse is over a UI element (written by ui).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PointerOverUi(pub bool);

/// True while a modal ui (pause menu) owns the keyboard and mouse: gfx must
/// ignore camera keys, wheel and drags (written by ui).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CameraInputBlocked(pub bool);

/// What kind of change a [`ToolPreview`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewKind {
    /// Nothing to preview (no ghost).
    #[default]
    None,
    Road,
    /// Zoning (`None` = removing zoning).
    Zone(Option<Zone>),
    Bulldoze,
}

/// What the current tool would do right now (from `World::preview`).
/// Written by ui (only when it changes), drawn by gfx as the ghost.
///
/// Round 2: for the road tool `line`/`cells` are the leg under the cursor
/// only; the whole multi-point path (or block grid) is in [`PathPreview`].
/// Zoning in every mode (rectangle, frontage, fill) and the bulldozer put
/// exactly the cells they change in `cells`; `rect` is set only for the
/// rectangle modes.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct ToolPreview {
    pub kind: PreviewKind,
    /// Road tools: start and end cell.
    pub line: Option<(Cell, Cell)>,
    /// Rectangle tools: the dragged rectangle.
    pub rect: Option<CellRect>,
    /// Affected cells (valid: from the preview; invalid road: the straight
    /// line's cells if it is straight, else empty).
    pub cells: Vec<Cell>,
    /// `None` when the command would succeed.
    pub error: Option<CommandError>,
    /// Money it would cost.
    pub cost: i64,
}

impl ToolPreview {
    pub fn is_valid(&self) -> bool {
        self.error.is_none()
    }
}

/// Data overlay shown on the map (written by ui, drawn by gfx).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayMode {
    #[default]
    None,
    Zones,
    Traffic,
}

/// Traffic overlay bands for a segment direction's `congestion_in` (load /
/// capacity). The sim counts a direction as congested above 0.8 (cars crawl
/// below ~12 km/h, `stats.congestion_share`); 0.5 is where cars start to
/// slow noticeably. Below `CONGESTION_BUSY` = free flow (green), up to
/// `CONGESTION_JAM` = busy (amber), from it on = jammed (red).
pub const CONGESTION_BUSY: f32 = 0.5;
pub const CONGESTION_JAM: f32 = 0.8;

/// Rendering quality preset (written by ui, applied by gfx).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphicsQuality {
    Low,
    Medium,
    #[default]
    High,
}

// ---------------------------------------------------------------- construction tools (round 2)
//
// Everything below is written by ui every frame it changes (`set_if_neq`)
// and only read by gfx. Positions are in sim metres (`[x, y]`, see
// `sim_to_world`); cells are map cells. Nothing here is game state: it is
// what the interface is showing right now.

/// Road type the road tool builds (written by ui). Maps 1:1 onto the sim's
/// `RoadKind::{Street, Avenue}` (the highway is never player-built).
#[allow(dead_code)] // variants constructed by ui tools as they land
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RoadChoice {
    /// One lane each way, cheap; the default.
    #[default]
    Street,
    /// Two lanes each way, faster and with more capacity; drawn wider (about
    /// 13 m) but still one cell line in the grid.
    Avenue,
}

/// What a road-tool drag does (sub-mode of `ActiveTool::Road`).
#[allow(dead_code)] // variants constructed by ui tools as they land
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RoadMode {
    /// Click/drag corners of a multi-segment path (`PathPreview`).
    #[default]
    Path,
    /// Drag a rectangle: a street loop, optionally subdivided into a grid
    /// (`PathPreview::legs` holds every street of it).
    Block,
    /// Hover a junction to see its control, click to switch it (no ghost;
    /// `WorldHighlight::hovered` is the junction).
    Junction,
}

/// What a zoning drag does (sub-mode of `ActiveTool::Zone(_)`).
#[allow(dead_code)] // variants constructed by ui tools as they land
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ZoneMode {
    /// Drag a rectangle (round-1 behaviour).
    #[default]
    Rect,
    /// Click a road side: that whole frontage between two junctions, to the
    /// lot depth. Drag along a road: paint the frontage passed over.
    Frontage,
    /// Click: every eligible cell of the enclosed block under the cursor.
    Fill,
}

/// What a bulldozer drag does (sub-mode of `ActiveTool::Bulldoze`).
#[allow(dead_code)] // variants constructed by ui tools as they land
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BulldozeMode {
    /// Drag a rectangle (round-1 behaviour).
    #[default]
    Rect,
    /// Click a road: the stretch between two junctions. Drag along a road:
    /// just the part passed over.
    Road,
}

/// Sub-mode of every tool (written by ui; the active one is the field for
/// the current `ActiveTool`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToolModes {
    pub road: RoadMode,
    pub zone: ZoneMode,
    pub bulldoze: BulldozeMode,
}

/// What a snapped point attached to, strongest first.
#[allow(dead_code)] // variants constructed by ui tools as they land
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SnapTarget {
    /// Snapping is off (Alt held) or nothing applied: the plain cell.
    #[default]
    None,
    /// Plain grid cell (roads are built on cell centres).
    Grid,
    /// An existing road end or junction.
    Node,
    /// Onto the middle of an existing road (forms a T-junction).
    Road,
    /// The nearest straight / 45-degree direction from the previous corner.
    Angle,
    /// Perpendicular to a nearby road.
    Perpendicular,
    /// Parallel to (alongside) a nearby road.
    Parallel,
    /// On the extension of an existing road end.
    Extension,
    /// Same spacing as the neighbouring block.
    Spacing,
}

/// Why a guide line is shown.
#[allow(dead_code)] // variants constructed by ui tools as they land
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuideKind {
    /// The straight / 45-degree ray from the previous corner.
    Angle,
    /// An existing road end continued straight on.
    Extension,
    /// Perpendicular through / to a nearby road.
    Perpendicular,
    /// Parallel to a nearby road at the snapped offset.
    Parallel,
    /// The road this spacing is measured from (to the snapped point).
    Spacing,
}

/// A dashed guide on the ground, in sim metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub kind: GuideKind,
}

/// Snapping feedback for the active tool (road tool; also zoning/bulldozer
/// road modes, which snap to road stretches). Empty (`point: None`) when the
/// tool does not snap.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct SnapFeedback {
    /// The snapped point (a cell centre), sim metres.
    pub point: Option<[f32; 2]>,
    /// Its cell.
    pub cell: Option<Cell>,
    /// The strongest rule that placed the point.
    pub target: SnapTarget,
    /// Every rule that agreed on it (includes `target`), e.g. Node + Angle.
    pub matched: Vec<SnapTarget>,
    /// Guide lines to draw.
    pub guides: Vec<Guide>,
    /// With `SnapTarget::Spacing` (or a parallel snap): the spacing to the
    /// neighbouring road in cells, shown as a label by ui.
    pub spacing_cells: Option<i32>,
    /// True while the player holds snapping off (Alt).
    pub suspended: bool,
}

/// One straight leg of a road being drawn (one `Command::BuildRoad`).
#[derive(Debug, Clone, PartialEq)]
pub struct PathLeg {
    pub from: Cell,
    pub to: Cell,
    /// `None` when the sim would accept this leg (from `World::preview`,
    /// checked after the legs before it).
    pub error: Option<CommandError>,
    /// Cells of the leg that already carry a road of a different kind (an
    /// avenue over a street previews as an upgrade).
    pub upgrade_cells: Vec<Cell>,
}

/// The whole road being drawn: the multi-point path of the road tool, or
/// every street of the block tool. Empty when not drawing.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct PathPreview {
    pub kind: RoadChoice,
    /// Path corners in order (first = start, last = live end under the
    /// cursor). Empty for the block tool.
    pub points: Vec<Cell>,
    /// Corners already placed (the rest of `points` follows the cursor).
    pub placed: usize,
    /// Every leg that would be built, in build order.
    pub legs: Vec<PathLeg>,
    /// Total length in metres and total price (new road + upgrades).
    pub length_m: f32,
    pub cost: i64,
    /// First error of any leg (the whole path builds only when `None`).
    pub error: Option<CommandError>,
}

/// A stretch of road between two junctions (or dead ends).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoadStretch {
    /// The graph segments of the stretch, end to end.
    pub segments: Vec<SegmentId>,
    /// Its centre line as a polyline, sim metres (node positions).
    pub points: Vec<[f32; 2]>,
    /// Grid cells it covers.
    pub cells: Vec<Cell>,
}

/// Something in the world the interface points at.
#[allow(dead_code)] // variants constructed by ui tools as they land
#[derive(Debug, Clone, PartialEq)]
pub enum WorldTarget {
    Building(BuildingId),
    Road(RoadStretch),
    /// A road node with three or more arms.
    Junction {
        node: NodeId,
        pos: [f32; 2],
    },
}

/// What gfx should highlight in the world (issue 6): the hovered and the
/// inspected (clicked, pinned) thing, and what the bulldozer would remove.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct WorldHighlight {
    /// Under the cursor (inspect, junction tool, road-mode zoning/bulldozer).
    pub hovered: Option<WorldTarget>,
    /// Pinned by a click with Inspect (its card stays open).
    pub selected: Option<WorldTarget>,
    /// Bulldozer preview: buildings that would be removed (tint red).
    pub doomed_buildings: Vec<BuildingId>,
    /// Bulldozer preview: road cells that would be removed (tint red).
    pub doomed_road_cells: Vec<Cell>,
}

/// Player display settings that gfx honours (written by ui from the
/// settings panel).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct DisplaySettings {
    /// Move the camera when the cursor touches a window edge.
    pub edge_scroll: bool,
    /// Always mark zoned-but-empty lots, even without the zoning tool or
    /// zones overlay (issue 9).
    pub lot_markers: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        DisplaySettings {
            edge_scroll: true,
            lot_markers: true,
        }
    }
}

// ---------------------------------------------------------------- camera contract

/// Marker for the 3D camera that looks at the map (spawned by gfx). Used for
/// cursor picking and by dev tooling.
#[derive(Component, Debug, Default)]
pub struct WorldCamera;

/// An orbit camera pose: looking at `focus` on the ground from `distance`
/// metres, `yaw` radians around +Y, `pitch` radians above the horizon.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraPose {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl CameraPose {
    pub fn transform(&self) -> Transform {
        let offset = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        ) * self.distance;
        Transform::from_translation(self.focus + offset).looking_at(self.focus, Vec3::Y)
    }
}

/// When `Some`, the gfx camera must show exactly this pose and ignore player
/// camera input (used by scripted demos and screenshots).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct CameraOverride(pub Option<CameraPose>);

// ---------------------------------------------------------------- coordinates

/// Sim `(x, y)` metres -> Bevy `(x, height, y)`.
pub fn sim_to_world(p: [f32; 2], height: f32) -> Vec3 {
    Vec3::new(p[0], height, p[1])
}

/// Bevy position -> sim `(x, y)` metres (drops height).
pub fn world_to_sim(v: Vec3) -> [f32; 2] {
    [v.x, v.z]
}

/// Centre of a cell in Bevy space at `height`.
pub fn cell_center(c: Cell, height: f32) -> Vec3 {
    sim_to_world(c.center(), height)
}

/// The cell containing a Bevy-space point (may be out of bounds).
pub fn world_to_cell(v: Vec3) -> Cell {
    Cell::from_pos(world_to_sim(v))
}

/// Corners of a cell (inset by `inset` metres) at `height`, in order around
/// the edge.
pub fn cell_corners(c: Cell, inset: f32, height: f32) -> [Vec3; 4] {
    let (x0, z0) = (c.x as f32 * CELL_SIZE + inset, c.y as f32 * CELL_SIZE + inset);
    let (x1, z1) = (
        (c.x + 1) as f32 * CELL_SIZE - inset,
        (c.y + 1) as f32 * CELL_SIZE - inset,
    );
    [
        Vec3::new(x0, height, z0),
        Vec3::new(x1, height, z0),
        Vec3::new(x1, height, z1),
        Vec3::new(x0, height, z1),
    ]
}

// ---------------------------------------------------------------- plugin

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        let seed = std::env::var("CITYSIM_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        app.insert_resource(SimWorld {
            world: World::new(seed),
            generation: 0,
        })
        .insert_resource(Time::<Fixed>::from_hz(TICKS_PER_SECOND as f64))
        .init_resource::<SimClock>()
        .init_resource::<LastCommand>()
        .init_resource::<ActiveTool>()
        .init_resource::<HoveredCell>()
        .init_resource::<PointerOverUi>()
        .init_resource::<CameraInputBlocked>()
        .init_resource::<ToolPreview>()
        .init_resource::<OverlayMode>()
        .init_resource::<GraphicsQuality>()
        .init_resource::<CameraOverride>()
        .init_resource::<RoadChoice>()
        .init_resource::<ToolModes>()
        .init_resource::<SnapFeedback>()
        .init_resource::<PathPreview>()
        .init_resource::<WorldHighlight>()
        .init_resource::<DisplaySettings>()
        .add_message::<SimCommand>()
        .configure_sets(
            Update,
            (
                GameSet::Input,
                GameSet::Hover,
                GameSet::Tools,
                GameSet::Sim,
                GameSet::Render,
            )
                .chain(),
        )
        .add_systems(FixedUpdate, step_sim.in_set(SimStep))
        .add_systems(Update, compute_hover.in_set(GameSet::Hover))
        .add_systems(Update, apply_commands.in_set(GameSet::Sim));
    }
}

fn step_sim(mut sim: ResMut<SimWorld>, clock: Res<SimClock>) {
    for _ in 0..clock.steps_per_tick() {
        sim.world.step();
    }
}

fn apply_commands(
    mut sim: ResMut<SimWorld>,
    mut cmds: MessageReader<SimCommand>,
    mut last: ResMut<LastCommand>,
    time: Res<Time<Real>>,
) {
    for SimCommand(cmd) in cmds.read() {
        let res = sim.world.apply(*cmd);
        if let Err(e) = &res {
            debug!("command {cmd:?} rejected: {e}");
        }
        last.result = Some((*cmd, res));
        last.at = time.elapsed_secs();
    }
}

fn compute_hover(
    window: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    over_ui: Res<PointerOverUi>,
    mut hovered: ResMut<HoveredCell>,
) {
    let mut cell = None;
    if !over_ui.0
        && let Ok(window) = window.single()
        && let Some(pos) = window.cursor_position()
        && let Ok((camera, cam_tf)) = cameras.single()
        && let Ok(ray) = camera.viewport_to_world(cam_tf, pos)
        && let Some(hit) = ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
    {
        let c = world_to_cell(hit);
        cell = c.in_bounds().then_some(c);
    }
    hovered.set_if_neq(HoveredCell(cell));
}
