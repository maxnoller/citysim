//! Cursor picking: which cell is under the mouse, plus its highlight.
//!
//! Real mouse input is turned into a [`Pointer`] resource each frame; the
//! tools read only `Pointer`. Scripted runs set `Pointer` themselves (and
//! insert [`PointerScripted`] so the mouse is ignored).

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::Cell;

use crate::{GameSet, coords};

/// The pointer as the tools see it.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct Pointer {
    /// Map cell under the cursor (None when off the map / off the window).
    pub cell: Option<Cell>,
    /// Primary button went down / up this frame, or is held.
    pub pressed: bool,
    pub released: bool,
    pub held: bool,
    /// Cancel requested this frame (Escape or right click).
    pub cancel: bool,
}

/// When present, the mouse does not drive [`Pointer`].
#[derive(Resource)]
pub struct PointerScripted;

#[derive(Component)]
struct Highlight;

pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pointer>()
            .add_systems(Startup, spawn_highlight)
            .add_systems(Update, read_mouse.in_set(GameSet::Input))
            .add_systems(Update, move_highlight.in_set(GameSet::Render));
    }
}

fn read_mouse(
    scripted: Option<Res<PointerScripted>>,
    window: Single<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut pointer: ResMut<Pointer>,
) {
    if scripted.is_some() {
        return;
    }
    pointer.cell = None;
    if let (Some(pos), Ok((camera, cam_tf))) = (window.cursor_position(), cameras.single())
        && let Ok(ray) = camera.viewport_to_world(cam_tf, pos)
        && let Some(hit) = ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
    {
        let cell = Cell::from_pos(coords::world_to_sim(hit));
        pointer.cell = cell.in_bounds().then_some(cell);
    }
    pointer.pressed = mouse.just_pressed(MouseButton::Left);
    pointer.released = mouse.just_released(MouseButton::Left);
    pointer.held = mouse.pressed(MouseButton::Left);
    pointer.cancel = keys.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right);
}

fn spawn_highlight(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut b = crate::mesh_util::FlatMeshBuilder::new();
    let s = citysim_sim::CELL_SIZE;
    b.quad([
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(s, 0.0, 0.0),
        Vec3::new(s, 0.0, s),
        Vec3::new(0.0, 0.0, s),
    ]);
    commands.spawn((
        Name::new("Cursor highlight"),
        Highlight,
        Mesh3d(meshes.add(b.build())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 1.0, 1.0, 0.45),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
    ));
}

fn move_highlight(
    pointer: Res<Pointer>,
    mut q: Query<(&mut Transform, &mut Visibility), With<Highlight>>,
    mut gizmos: Gizmos,
) {
    for (mut t, mut vis) in &mut q {
        match pointer.cell {
            Some(c) => {
                let s = citysim_sim::CELL_SIZE;
                t.translation = Vec3::new(c.x as f32 * s, coords::HIGHLIGHT_Y, c.y as f32 * s);
                *vis = Visibility::Visible;
                let corners = coords::cell_corners(c, 0.0, coords::HIGHLIGHT_Y + 0.05);
                gizmos.linestrip(
                    [corners[0], corners[1], corners[2], corners[3], corners[0]],
                    Color::WHITE,
                );
            }
            None => *vis = Visibility::Hidden,
        }
    }
}
