//! Player tools: road (click start, click end; chains), zone and bulldoze
//! (drag a rectangle). Keys: R road, Z zone (cycles R/C/I/unzone), B
//! bulldoze, Escape / right click cancels.
//!
//! Tools read only [`Pointer`], preview through `World::preview` (the
//! ghost) and act only by sending `SimCommand`s.

use bevy::prelude::*;
use citysim_sim::{Cell, Command, CommandError, Preview, Zone};

use crate::cursor::Pointer;
use crate::mesh_util::FlatMeshBuilder;
use crate::road_render::STREET_WIDTH;
use crate::sim_driver::{SimCommand, SimWorld};
use crate::zone_render::zone_color;
use crate::{GameSet, coords};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Road,
    /// `None` removes zoning.
    Zone(Option<Zone>),
    Bulldoze,
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Road => "Road",
            Tool::Zone(Some(Zone::Residential)) => "Zone: Residential",
            Tool::Zone(Some(Zone::Commercial)) => "Zone: Commercial",
            Tool::Zone(Some(Zone::Industrial)) => "Zone: Industrial",
            Tool::Zone(None) => "Zone: Remove",
            Tool::Bulldoze => "Bulldoze",
        }
    }
}

#[derive(Resource, Debug)]
pub struct ToolState {
    pub tool: Tool,
    /// Road start cell, or rectangle drag start.
    pub anchor: Option<Cell>,
    /// Last on-map pointer cell (used when the pointer leaves the map).
    pub last_cell: Option<Cell>,
}

impl Default for ToolState {
    fn default() -> Self {
        ToolState {
            tool: Tool::Road,
            anchor: None,
            last_cell: None,
        }
    }
}

/// What the current tool would do right now.
#[derive(Resource, Default, Debug)]
pub struct Ghost {
    pub cmd: Option<Command>,
    pub result: Option<Result<Preview, CommandError>>,
    /// Inputs `result` was computed from (command + world state).
    key: Option<(Command, u64, u64, u64, i64)>,
    rendered: bool,
}

#[derive(Component)]
struct GhostMesh;

#[derive(Resource)]
struct GhostMaterials {
    road_ok: Handle<StandardMaterial>,
    invalid: Handle<StandardMaterial>,
    bulldoze: Handle<StandardMaterial>,
    unzone: Handle<StandardMaterial>,
    zones: [Handle<StandardMaterial>; 3],
}

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolState>()
            .init_resource::<Ghost>()
            .add_systems(Startup, setup_ghost)
            .add_systems(
                Update,
                (tool_keys, use_tool, update_ghost).chain().in_set(GameSet::Tools),
            )
            .add_systems(Update, draw_ghost.in_set(GameSet::Render));
    }
}

fn setup_ghost(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    let mut mat = |c: Color| {
        materials.add(StandardMaterial {
            base_color: c,
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        })
    };
    let ghost = GhostMaterials {
        road_ok: mat(Color::srgba(0.75, 0.85, 1.0, 0.6)),
        invalid: mat(Color::srgba(1.0, 0.1, 0.1, 0.6)),
        bulldoze: mat(Color::srgba(1.0, 0.5, 0.1, 0.6)),
        unzone: mat(Color::srgba(0.9, 0.9, 0.9, 0.5)),
        zones: Zone::ALL.map(|z| mat(zone_color(z).with_alpha(0.7))),
    };
    commands.spawn((
        Name::new("Tool ghost"),
        GhostMesh,
        MeshMaterial3d(ghost.road_ok.clone()),
        Transform::default(),
    ));
    commands.insert_resource(ghost);
}

fn tool_keys(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<ToolState>) {
    let new = if keys.just_pressed(KeyCode::KeyR) {
        Some(Tool::Road)
    } else if keys.just_pressed(KeyCode::KeyB) {
        Some(Tool::Bulldoze)
    } else if keys.just_pressed(KeyCode::KeyZ) {
        Some(Tool::Zone(match state.tool {
            Tool::Zone(Some(Zone::Residential)) => Some(Zone::Commercial),
            Tool::Zone(Some(Zone::Commercial)) => Some(Zone::Industrial),
            Tool::Zone(Some(Zone::Industrial)) => None,
            _ => Some(Zone::Residential),
        }))
    } else {
        None
    };
    if let Some(tool) = new {
        state.tool = tool;
        state.anchor = None;
    }
}

fn rect_command(tool: Tool, a: Cell, b: Cell) -> Option<Command> {
    match tool {
        Tool::Zone(zone) => Some(Command::SetZone { min: a, max: b, zone }),
        Tool::Bulldoze => Some(Command::Bulldoze { min: a, max: b }),
        Tool::Road => None,
    }
}

fn use_tool(
    pointer: Res<Pointer>,
    mut state: ResMut<ToolState>,
    ghost: Res<Ghost>,
    mut out: MessageWriter<SimCommand>,
) {
    if pointer.cell.is_some() {
        state.last_cell = pointer.cell;
    }
    if pointer.cancel {
        state.anchor = None;
        return;
    }
    match state.tool {
        Tool::Road => {
            let Some(cell) = pointer.cell else { return };
            if !pointer.pressed {
                return;
            }
            match state.anchor {
                None => state.anchor = Some(cell),
                Some(start) if start == cell => state.anchor = None,
                Some(start) => {
                    out.write(SimCommand(Command::BuildRoad { from: start, to: cell }));
                    // Chain from the end if the ghost said it would work.
                    let ok = ghost.cmd == Some(Command::BuildRoad { from: start, to: cell })
                        && matches!(ghost.result, Some(Ok(_)));
                    if ok {
                        state.anchor = Some(cell);
                    }
                }
            }
        }
        tool => {
            if pointer.pressed && pointer.cell.is_some() {
                state.anchor = pointer.cell;
            }
            if pointer.released
                && let (Some(a), Some(b)) = (state.anchor, state.last_cell)
            {
                if let Some(cmd) = rect_command(tool, a, b) {
                    out.write(SimCommand(cmd));
                }
                state.anchor = None;
            }
        }
    }
}

fn update_ghost(pointer: Res<Pointer>, state: Res<ToolState>, sim: Res<SimWorld>, mut ghost: ResMut<Ghost>) {
    let cell = pointer.cell.or(state.last_cell.filter(|_| state.anchor.is_some()));
    let cmd = match (state.tool, state.anchor, cell) {
        (Tool::Road, Some(a), Some(c)) if a != c => Some(Command::BuildRoad { from: a, to: c }),
        (Tool::Road, _, _) => None,
        (tool, anchor, Some(c)) => rect_command(tool, anchor.unwrap_or(c), c),
        (_, _, None) => None,
    };
    let Some(cmd) = cmd else {
        if ghost.cmd.is_some() {
            *ghost = Ghost::default();
        }
        return;
    };
    let w = &sim.world;
    let key = (cmd, sim.generation, w.versions.roads, w.versions.grid, w.economy.money);
    if ghost.key == Some(key) {
        return;
    }
    ghost.key = Some(key);
    ghost.cmd = Some(cmd);
    ghost.result = Some(w.preview(&cmd));
    ghost.rendered = false;
}

fn draw_ghost(
    mut commands: Commands,
    mut ghost: ResMut<Ghost>,
    mats: Res<GhostMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<Entity, With<GhostMesh>>,
    mut gizmos: Gizmos,
) {
    let Ok(entity) = q.single() else { return };
    // Rectangle outline every frame while a rect command is shown.
    if let Some(Command::SetZone { min, max, .. } | Command::Bulldoze { min, max }) = ghost.cmd {
        let r = citysim_sim::CellRect::from_corners(min, max);
        let s = citysim_sim::CELL_SIZE;
        let y = coords::GHOST_Y + 0.05;
        let (x0, z0, x1, z1) = (
            r.min.x as f32 * s,
            r.min.y as f32 * s,
            (r.max.x + 1) as f32 * s,
            (r.max.y + 1) as f32 * s,
        );
        let color = match ghost.result {
            Some(Ok(_)) => Color::WHITE,
            _ => Color::srgb(1.0, 0.1, 0.1),
        };
        gizmos.linestrip(
            [
                Vec3::new(x0, y, z0),
                Vec3::new(x1, y, z0),
                Vec3::new(x1, y, z1),
                Vec3::new(x0, y, z1),
                Vec3::new(x0, y, z0),
            ],
            color,
        );
    }
    if ghost.rendered {
        return;
    }
    ghost.rendered = true;
    let (Some(cmd), Some(result)) = (ghost.cmd, ghost.result.as_ref()) else {
        commands.entity(entity).remove::<Mesh3d>();
        return;
    };
    let mut b = FlatMeshBuilder::new();
    let y = coords::GHOST_Y;
    let material = match (cmd, result) {
        (Command::BuildRoad { from, to }, res) => {
            let (a, c) = (coords::cell_center(from, y), coords::cell_center(to, y));
            let width = if res.is_ok() { STREET_WIDTH } else { STREET_WIDTH * 0.6 };
            b.strip(a, c, width);
            b.disc(a, width * 0.5, 12);
            b.disc(c, width * 0.5, 12);
            if res.is_ok() {
                mats.road_ok.clone()
            } else {
                mats.invalid.clone()
            }
        }
        (Command::SetZone { zone, .. }, Ok(p)) => {
            for &c in &p.cells {
                b.quad(coords::cell_corners(c, 0.4, y));
            }
            match zone {
                Some(z) => mats.zones[Zone::ALL.iter().position(|&x| x == z).unwrap_or(0)].clone(),
                None => mats.unzone.clone(),
            }
        }
        (Command::Bulldoze { .. }, Ok(p)) => {
            for &c in &p.cells {
                b.quad(coords::cell_corners(c, 0.4, y));
            }
            mats.bulldoze.clone()
        }
        _ => mats.invalid.clone(),
    };
    if b.is_empty() {
        commands.entity(entity).remove::<Mesh3d>();
    } else {
        commands
            .entity(entity)
            .insert((Mesh3d(meshes.add(b.build())), MeshMaterial3d(material)));
    }
}
