//! 3D drawing of the hovered cell and of the current tool's ghost
//! (`ToolPreview`).

use bevy::prelude::*;
use citysim_sim::{CELL_SIZE, Zone};

use super::heights;
use super::mesh_util::FlatMeshBuilder;
use super::roads::STREET_WIDTH;
use super::zones::zone_color;
use crate::shared::{GameSet, HoveredCell, PreviewKind, ToolPreview, cell_center, cell_corners};

#[derive(Component)]
struct Highlight;

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

pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, (move_highlight, draw_ghost).in_set(GameSet::Render));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let mut mat = |c: Color| {
        materials.add(StandardMaterial {
            base_color: c,
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        })
    };
    let highlight = mat(Color::srgba(1.0, 1.0, 1.0, 0.45));
    let ghost = GhostMaterials {
        road_ok: mat(Color::srgba(0.75, 0.85, 1.0, 0.6)),
        invalid: mat(Color::srgba(1.0, 0.1, 0.1, 0.6)),
        bulldoze: mat(Color::srgba(1.0, 0.5, 0.1, 0.6)),
        unzone: mat(Color::srgba(0.9, 0.9, 0.9, 0.5)),
        zones: Zone::ALL.map(|z| mat(zone_color(z).with_alpha(0.7))),
    };
    let mut b = FlatMeshBuilder::new();
    b.quad([
        Vec3::ZERO,
        Vec3::new(CELL_SIZE, 0.0, 0.0),
        Vec3::new(CELL_SIZE, 0.0, CELL_SIZE),
        Vec3::new(0.0, 0.0, CELL_SIZE),
    ]);
    commands.spawn((
        Name::new("Hovered cell"),
        Highlight,
        Mesh3d(meshes.add(b.build())),
        MeshMaterial3d(highlight),
        Transform::default(),
        Visibility::Hidden,
    ));
    commands.spawn((
        Name::new("Tool ghost"),
        GhostMesh,
        MeshMaterial3d(ghost.road_ok.clone()),
        Transform::default(),
    ));
    commands.insert_resource(ghost);
}

fn move_highlight(
    hovered: Res<HoveredCell>,
    mut q: Query<(&mut Transform, &mut Visibility), With<Highlight>>,
    mut gizmos: Gizmos,
) {
    for (mut t, mut vis) in &mut q {
        match hovered.0 {
            Some(c) => {
                t.translation = Vec3::new(c.x as f32 * CELL_SIZE, heights::HIGHLIGHT, c.y as f32 * CELL_SIZE);
                *vis = Visibility::Visible;
                let k = cell_corners(c, 0.0, heights::HIGHLIGHT + 0.05);
                gizmos.linestrip([k[0], k[1], k[2], k[3], k[0]], Color::WHITE);
            }
            None => *vis = Visibility::Hidden,
        }
    }
}

fn draw_ghost(
    mut commands: Commands,
    preview: Res<ToolPreview>,
    mats: Res<GhostMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<Entity, With<GhostMesh>>,
    mut gizmos: Gizmos,
) {
    let Ok(entity) = q.single() else { return };
    if let Some(r) = preview.rect {
        let y = heights::GHOST + 0.05;
        let (x0, z0) = (r.min.x as f32 * CELL_SIZE, r.min.y as f32 * CELL_SIZE);
        let (x1, z1) = ((r.max.x + 1) as f32 * CELL_SIZE, (r.max.y + 1) as f32 * CELL_SIZE);
        let color = if preview.is_valid() {
            Color::WHITE
        } else {
            Color::srgb(1.0, 0.1, 0.1)
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
    if !preview.is_changed() {
        return;
    }
    let y = heights::GHOST;
    let mut b = FlatMeshBuilder::new();
    let valid = preview.is_valid();
    let material = match preview.kind {
        PreviewKind::None => {
            commands.entity(entity).remove::<Mesh3d>();
            return;
        }
        PreviewKind::Road => {
            if let Some((from, to)) = preview.line {
                let (a, c) = (cell_center(from, y), cell_center(to, y));
                let width = if valid { STREET_WIDTH } else { STREET_WIDTH * 0.6 };
                b.strip(a, c, width);
                b.disc(a, width * 0.5, 12);
                b.disc(c, width * 0.5, 12);
            }
            if valid {
                mats.road_ok.clone()
            } else {
                mats.invalid.clone()
            }
        }
        kind => {
            if valid {
                for &c in &preview.cells {
                    b.quad(cell_corners(c, 0.4, y));
                }
            }
            match kind {
                _ if !valid => mats.invalid.clone(),
                PreviewKind::Zone(Some(z)) => mats.zones[Zone::ALL.iter().position(|&x| x == z).unwrap_or(0)].clone(),
                PreviewKind::Zone(None) => mats.unzone.clone(),
                _ => mats.bulldoze.clone(),
            }
        }
    };
    if b.is_empty() {
        commands.entity(entity).remove::<Mesh3d>();
    } else {
        commands
            .entity(entity)
            .insert((Mesh3d(meshes.add(b.build())), MeshMaterial3d(material)));
    }
}
