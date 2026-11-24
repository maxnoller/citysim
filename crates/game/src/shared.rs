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

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::{CELL_SIZE, Cell, CellRect, Command, CommandError, Outcome, TICKS_PER_SECOND, World, Zone};

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
