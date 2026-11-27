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
//!   `SnapFeedback`, `WorldHighlight`, `DisplaySettings`.
//! * Round 4, written by ui, drawn / honoured by gfx: `CurveToolView` (the
//!   free-form road tool), `ToolPreview::area`, `WorldHighlight::doomed_roads`,
//!   `WorldTarget::Vehicle`, `CameraFollow`, `CameraGoTo`.
//! * Round 5, written by ui, drawn by gfx: `RouteView` (the trip of the
//!   selected car or person).
//! * Performance panel: `PerfProbe::detail` written by ui, `PerfProbe::render`
//!   filled by gfx.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::command::{NodeChange, PieceChange};
use citysim_sim::{
    BuildingId, Cell, CellRef, Command, CommandError, Curve, NodeId, Outcome, RoadKind, SegmentId, TICKS_PER_SECOND,
    VehicleId, World, Zone,
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
        let old = std::mem::replace(&mut self.world, world);
        self.generation += 1;
        // Freeing a big city takes milliseconds: not in this frame.
        std::thread::spawn(move || drop(old));
    }
}

/// Steps per fixed update for speed levels 1 to 4 (a day is 16 minutes at
/// 1x, 80 seconds at 12x).
pub const SPEEDS: [u32; 4] = [1, 3, 6, 12];

/// Most fixed updates a second: the steps of a fast speed are spread over
/// several smaller fixed updates (so no single frame carries all of a
/// tick's steps), up to this rate.
pub const MAX_FIXED_HZ: u32 = 120;

/// Pause and speed. The fixed timestep runs at `TICKS_PER_SECOND` times
/// `fixed_per_tick()`; each fixed update performs `steps_per_tick()` sim
/// steps, so a second of real time is always `TICKS_PER_SECOND` times the
/// speed in steps.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SimClock {
    pub paused: bool,
    /// Index into [`SPEEDS`] (0 = 1x).
    pub speed: usize,
    /// ui is sending a plan in parts over a few frames: no steps until it
    /// has all been applied (so nothing changes between its parts; not
    /// shown as a pause).
    pub hold: bool,
}

impl SimClock {
    /// Sim steps per real-time tick (`1/TICKS_PER_SECOND` s) at this speed.
    pub fn speed_steps(&self) -> u32 {
        SPEEDS[self.speed.min(SPEEDS.len() - 1)]
    }

    /// Fixed updates per real-time tick: the largest divisor of the speed
    /// that keeps the fixed rate at most [`MAX_FIXED_HZ`].
    pub fn fixed_per_tick(&self) -> u32 {
        let s = self.speed_steps();
        (1..=s)
            .rev()
            .find(|k| s.is_multiple_of(*k) && k * TICKS_PER_SECOND <= MAX_FIXED_HZ)
            .unwrap_or(1)
    }

    /// Sim steps per fixed update (0 while paused).
    pub fn steps_per_tick(&self) -> u32 {
        if self.paused || self.hold {
            0
        } else {
            self.speed_steps() / self.fixed_per_tick()
        }
    }
}

/// Keep the fixed timestep at the rate the speed asks for.
fn fixed_rate(clock: Res<SimClock>, mut fixed: ResMut<Time<Fixed>>) {
    if clock.is_changed() {
        let hz = (TICKS_PER_SECOND * clock.fixed_per_tick()) as f64;
        if (fixed.timestep().as_secs_f64() - 1.0 / hz).abs() > 1e-9 {
            fixed.set_timestep_hz(hz);
        }
    }
}

/// A command for the sim; applied in `GameSet::Sim`, result in `LastCommand`.
#[derive(Message, Debug, Clone)]
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

/// The ground point under the mouse (sim metres) and the usable zoning
/// cell there (on whatever lattice owns that spot; `None` where there is no
/// zoneable land, e.g. on a road). `None` under the same conditions as [`HoveredCell`]. Computed by
/// shared together with it.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct HoveredPoint {
    pub pos: Option<[f32; 2]>,
    pub zone_cell: Option<CellRef>,
}

/// Real time `apply_commands` took this frame (ms, 0 when nothing was
/// sent): ui sends a plan in parts by it, the benchmark records it.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct CommandTime(pub f32);

/// Dev scripts without a window (`CITYSIM_BENCH`): the pointer's position
/// in the world camera's viewport (logical px). Where there is no primary
/// window, shared's hover and ui's tools take the pointer from here (the
/// script presses the mouse buttons in `ButtonInput<MouseButton>`).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct ScriptPointer(pub Option<Vec2>);

impl ScriptPointer {
    /// The pointer: the window's cursor, else (no window) the script's.
    pub fn position(&self, window: Option<&Window>) -> Option<Vec2> {
        match window {
            Some(w) => w.cursor_position(),
            None => self.0,
        }
    }
}

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
/// Written by ui (only when it changes), drawn by gfx as the ghost. The
/// road tools' ghost is [`CurveToolView`]; zoning and the bulldozer fill
/// `area` (marquee modes) and `zone_cells`.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct ToolPreview {
    pub kind: PreviewKind,
    /// Marquee tools (zoning, bulldozer area): the dragged area, a
    /// rectangle along the lattice it started on, sim metres (corners in
    /// order). gfx draws its outline.
    pub area: Option<[[f32; 2]; 4]>,
    /// Zoning tools: the zoning cells that would change, on any lattice
    /// (`Preview::zone_cells`), drawn rotated along their roads.
    pub zone_cells: Vec<CellRef>,
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

/// What the road tool draws (sub-mode of `ActiveTool::Road`). Every
/// drawing mode shows its road in [`CurveToolView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RoadMode {
    /// Straight pieces: click (or drag) from point to point.
    #[default]
    Straight,
    /// Three clicks per piece: start, the corner where the two directions
    /// meet, end.
    Curve,
    /// Each piece leaves the last one smoothly: click along the way.
    Continuous,
    /// Three clicks: a corner, the width, the length: a street grid at any
    /// angle.
    Grid,
    /// Draw along a road to change it to the chosen kind (upgrade or
    /// downgrade); click for the whole stretch between junctions.
    Upgrade,
    /// Hover a junction to see its control, click to switch it (no ghost;
    /// `WorldHighlight::hovered` is the junction).
    Junction,
}

impl RoadMode {
    /// Modes that draw new road.
    pub fn draws(self) -> bool {
        matches!(
            self,
            RoadMode::Straight | RoadMode::Curve | RoadMode::Continuous | RoadMode::Grid
        )
    }
}

/// What a zoning drag does (sub-mode of `ActiveTool::Zone(_)`). Every mode
/// previews exactly the cells it changes in `ToolPreview::zone_cells`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ZoneMode {
    /// Paint cells under the cursor while the button is held.
    #[default]
    Paint,
    /// Click a road side: that whole frontage between two junctions, to the
    /// chosen depth. Drag along the roads: the frontage passed over, through
    /// bends, pieces and junctions.
    Frontage,
    /// Click: the connected zoneable area under the cursor.
    Fill,
    /// Drag a rectangle turned to the cell it starts on; it takes the cells
    /// of every lattice inside (`ToolPreview::area`).
    Marquee,
}

/// What a bulldozer drag does (sub-mode of `ActiveTool::Bulldoze`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BulldozeMode {
    /// Click a road: the stretch between two junctions. Drag along a road:
    /// just the part passed over. Click a building: that building.
    #[default]
    Road,
    /// Drag a rectangle along the lattice it starts on: roads and buildings
    /// inside (`ToolPreview::area`).
    Area,
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
    /// A length in whole cells (8 m) from the previous point.
    Length,
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

/// What the free-form road tool shows right now (sim metres). Written by ui
/// while a road is being drawn (also the upgrade mode: pieces marked
/// `PieceChange::Upgrade` / `Downgrade`), drawn by gfx. The snap marker and
/// guide lines of the live point are in [`SnapFeedback`]; `snap` / `guides`
/// here are only filled by gfx's own sample states.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct CurveToolView {
    pub active: bool,
    pub kind: Option<RoadKind>,
    /// The road as it would be, split at its nodes, with the kind of change.
    pub pieces: Vec<(Curve, PieceChange)>,
    /// Curves the sim refuses (the whole piece in error).
    pub refused: Vec<Curve>,
    /// Where the sim says the problem is.
    pub error_at: Option<[f32; 2]>,
    /// Control points: start, corners, live end.
    pub points: Vec<[f32; 2]>,
    /// Tangent handles: (anchor, handle tip).
    pub handles: Vec<([f32; 2], [f32; 2])>,
    /// Tightest bend: (point, its centre of curvature, radius, too tight).
    pub tightest: Option<([f32; 2], [f32; 2], f32, bool)>,
    /// The snapped point and what it snapped to.
    pub snap: Option<([f32; 2], SnapTarget)>,
    pub guides: Vec<Guide>,
    /// Junctions it would make or use.
    pub nodes: Vec<([f32; 2], NodeChange)>,
    /// What is shown (`pieces`, `refused`, `error_at`, `nodes`) is the last
    /// plan checked; a newer one has been in the works for a moment (gfx
    /// draws it dimmed). The points, handles and snap are always current.
    pub stale: bool,
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
    /// A car or truck (follows it as it drives).
    Vehicle(VehicleId),
}

/// What gfx should highlight in the world (issue 6): the hovered and the
/// inspected (clicked, pinned) thing, and what the bulldozer would remove.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct WorldHighlight {
    /// Under the cursor (inspect, junction tool, road-mode zoning/bulldozer).
    pub hovered: Option<WorldTarget>,
    /// Pinned by a click with Inspect (its card stays open).
    pub selected: Option<WorldTarget>,
    /// Buildings that would be removed: the bulldozer's, or those in the
    /// way of the road being drawn (tint red).
    pub doomed_buildings: Vec<BuildingId>,
    /// Bulldozer preview: the parts of roads that would be removed (centre
    /// lines and kinds; tint red over the road's width).
    pub doomed_roads: Vec<(Curve, RoadKind)>,
}

/// How a [`RouteView`] is under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RouteState {
    /// Nothing selected that travels (gfx draws nothing).
    #[default]
    None,
    /// A car on the road: `points` start at the car and follow the lanes it
    /// has planned (the part already driven is not in it).
    Driving,
    /// A trip that has not started yet (a person at home, at work...): the
    /// way they would drive now, from the building they are in.
    Planned,
    /// Under way where the map draws nothing (on foot, a lift, a car beyond
    /// the cars shown): the way a car would take, start to end.
    Unseen,
}

/// One end of a [`RouteView`]: a ground point (sim metres) and the
/// building there, if any (the highway entry has none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteEnd {
    pub pos: [f32; 2],
    pub building: Option<BuildingId>,
}

/// The trip of the selected car or person, drawn on the map. Written by ui
/// (only when it changes meaningfully: a new route, the car a few metres on,
/// another selection), drawn by gfx. Empty (`state: None`) when nothing
/// travelling is selected.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct RouteView {
    pub state: RouteState,
    /// The way still ahead along the lanes, sim metres (lane offsets
    /// included), in driving order: from where they are now (`Driving`) or
    /// from the start (`Planned`, `Unseen`) to where the car will stop.
    pub points: Vec<[f32; 2]>,
    /// Where the trip started (quiet marker) and where it goes (strong
    /// marker; gfx also highlights that building).
    pub origin: Option<RouteEnd>,
    pub destination: Option<RouteEnd>,
    /// A person under way off the map (`Unseen`: on foot, a lift, a car
    /// beyond those drawn): where they should be by now, estimated from the
    /// share of the trip's time gone. `points` then start there and gfx
    /// draws the line as for a car, with a marker here (`gfx/route.rs`).
    pub traveller: Option<[f32; 2]>,
}

/// Player display settings that gfx honours (written by ui from the
/// settings panel). Both off by default: in a window the cursor crosses
/// the border whenever it leaves the game.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct DisplaySettings {
    /// Move the camera when the cursor touches a window edge.
    pub edge_scroll: bool,
    /// Mark zoned-but-empty lots always; when false (the default) gfx marks
    /// them only while a zoning tool is active (the zones overlay shows them
    /// anyway).
    pub lot_markers: bool,
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

/// While `focus` is `Some`, gfx's camera keeps its focus on that ground
/// point (sim metres), easing after it as it moves; zoom and rotation stay
/// with the player, panning keys are ignored (ui stops following on them).
/// Written by ui every frame while following a person or car.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct CameraFollow {
    pub focus: Option<[f32; 2]>,
}

/// Move the camera's focus to a ground point (sim metres), and zoom to
/// `distance` metres when given (sent by ui: links between views).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct CameraGoTo {
    pub focus: [f32; 2],
    pub distance: Option<f32>,
}

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

// ---------------------------------------------------------------- performance panel

/// How much the performance panel shows (written by ui). gfx measures its
/// figures only while it is `Full`; `Off` and `Small` cost it nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PerfDetail {
    #[default]
    Off,
    /// Frames per second and frame time.
    Small,
    /// Everything, including what only the renderer knows.
    Full,
}

/// The performance panel's link to the renderer: ui sets `detail`, gfx
/// fills `render` a few times a second while `detail` is `Full` (and
/// leaves it as it was otherwise).
#[derive(Resource, Debug, Clone, Default)]
pub struct PerfProbe {
    pub detail: PerfDetail,
    pub render: Option<RenderStats>,
}

/// What only the renderer knows, averaged over the frames since the last
/// refresh. `None` / empty where this backend or build cannot tell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderStats {
    /// Real time (s since start) of this refresh.
    pub at: f32,
    /// GPU time per frame (ms): the sum of the top-level render passes
    /// (timestamp queries; Vulkan and DX12 only).
    pub gpu_ms: Option<f32>,
    /// GPU ms per frame of each top-level pass, slowest first.
    pub passes: Vec<(String, f32)>,
    /// CPU time of the render thread per frame (ms): from the start of its
    /// frame to the GPU submit, not counting the wait for a swapchain image.
    pub render_cpu_ms: Option<f32>,
    /// Draw calls per frame: the main camera's passes, all shadow views.
    pub draw_calls: (u32, u32),
    /// Triangles per frame in the main passes (pipeline statistics).
    pub triangles: Option<u64>,
    /// Mesh entities, and how many of them are on screen.
    pub meshes: (u32, u32),
    /// Cars and trucks drawn, and how many of them are on screen.
    pub vehicles: (u32, u32),
    /// GPU memory the game has allocated and reserved (bytes; Vulkan and
    /// DX12 only).
    pub gpu_memory: Option<(u64, u64)>,
    /// Graphics card, graphics API in use, driver.
    pub adapter: String,
    pub backend: String,
    pub driver: String,
}

// ---------------------------------------------------------------- plugin

/// True once the 3D scene's shaders are built and it really draws (written
/// by gfx; ui shows a loading card until then). Starts false.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SceneReady(pub bool);

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
        .init_resource::<HoveredPoint>()
        .init_resource::<PointerOverUi>()
        .init_resource::<ScriptPointer>()
        .init_resource::<CommandTime>()
        .init_resource::<CameraInputBlocked>()
        .init_resource::<ToolPreview>()
        .init_resource::<OverlayMode>()
        .init_resource::<GraphicsQuality>()
        .init_resource::<CameraOverride>()
        .init_resource::<RoadChoice>()
        .init_resource::<ToolModes>()
        .init_resource::<SnapFeedback>()
        .init_resource::<WorldHighlight>()
        .init_resource::<DisplaySettings>()
        .init_resource::<CurveToolView>()
        .init_resource::<CameraFollow>()
        .init_resource::<RouteView>()
        .init_resource::<PerfProbe>()
        .add_message::<CameraGoTo>()
        // Without gfx there is nothing to wait for.
        .insert_resource(SceneReady(!cfg!(feature = "gfx")))
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
        .add_systems(First, fixed_rate)
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
    mut took: ResMut<CommandTime>,
) {
    let started = std::time::Instant::now();
    let mut any = false;
    for SimCommand(cmd) in cmds.read() {
        any = true;
        let res = sim.world.apply(cmd.clone());
        if let Err(e) = &res {
            debug!("command {cmd:?} rejected: {e}");
        }
        last.result = Some((cmd.clone(), res));
        last.at = time.elapsed_secs();
    }
    let ms = if any {
        started.elapsed().as_secs_f32() * 1e3
    } else {
        0.0
    };
    took.set_if_neq(CommandTime(ms));
}

fn compute_hover(
    window: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    over_ui: Res<PointerOverUi>,
    script: Res<ScriptPointer>,
    sim: Res<SimWorld>,
    mut hovered: ResMut<HoveredCell>,
    mut point: ResMut<HoveredPoint>,
) {
    let mut cell = None;
    let mut ground = None;
    if !over_ui.0
        && let Some(pos) = script.position(window.single().ok())
        && let Ok((camera, cam_tf)) = cameras.single()
        && let Ok(ray) = camera.viewport_to_world(cam_tf, pos)
        && let Some(hit) = ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
    {
        let c = world_to_cell(hit);
        cell = c.in_bounds().then_some(c);
        ground = cell.map(|_| world_to_sim(hit));
    }
    hovered.set_if_neq(HoveredCell(cell));
    point.set_if_neq(HoveredPoint {
        pos: ground,
        zone_cell: ground.and_then(|p| {
            let z = &sim.world.zoning;
            z.cell_at(p).filter(|&c| z.cell(c).is_some_and(|x| x.usable()))
        }),
    });
}
