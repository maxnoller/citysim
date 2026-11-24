//! Static scene: ground plane the size of the map, sun, ambient light.

use bevy::light::GlobalAmbientLight;
use bevy::prelude::*;
use citysim_sim::MAP_SIZE_M;

pub const SKY_COLOR: Color = Color::srgb(0.53, 0.72, 0.90);
const GROUND_COLOR: Color = Color::srgb(0.40, 0.55, 0.32);

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GlobalAmbientLight {
            color: Color::WHITE,
            brightness: 400.0,
            ..default()
        })
        .add_systems(Startup, setup_scene)
        .add_systems(Update, draw_map_border);
    }
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let half = MAP_SIZE_M * 0.5;
    commands.spawn((
        Name::new("Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(MAP_SIZE_M, MAP_SIZE_M))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: GROUND_COLOR,
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(half, 0.0, half),
    ));
    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            illuminance: 9_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(half, 1000.0, half).looking_at(Vec3::new(half + 400.0, 0.0, half + 250.0), Vec3::Y),
    ));
}

fn draw_map_border(mut gizmos: Gizmos) {
    let s = MAP_SIZE_M;
    let y = 0.2;
    let c = [
        Vec3::new(0.0, y, 0.0),
        Vec3::new(s, y, 0.0),
        Vec3::new(s, y, s),
        Vec3::new(0.0, y, s),
    ];
    gizmos.linestrip([c[0], c[1], c[2], c[3], c[0]], Color::srgb(0.2, 0.3, 0.2));
}
