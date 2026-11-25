//! Chimney smoke: soft puffs rising and drifting from the stacks of the
//! bigger works (`MeshBuilder::emitters`). A fixed pool of puff entities is
//! shared by the stacks nearest the camera, so the cost never grows with the
//! city; Low quality has none.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use super::buildings::{BuildingEntities, BuildingModels};
use super::mesh_util::MeshBuilder;
use super::props;
use crate::shared::{GameSet, GraphicsQuality, WorldCamera};

const MAX_STACKS: usize = 32;
const PUFFS: usize = 9;
/// Stacks further than this from the camera do not smoke.
const RANGE: f32 = 650.0;

#[derive(Component)]
struct Puff(usize);

#[derive(Default)]
struct Stacks {
    points: Vec<Vec3>,
    since: f32,
}

pub struct SmokePlugin;

impl Plugin for SmokePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, animate.in_set(GameSet::Render));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let mut m = MeshBuilder::new();
    props::blob(&mut m, Vec3::ZERO, Vec3::ONE, Color::WHITE, 0.0);
    let mesh = meshes.add(m.build());
    let mat = materials.add(StandardMaterial {
        base_color: Color::srgba(0.8, 0.8, 0.78, 0.3),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 1.0,
        ..default()
    });
    for i in 0..MAX_STACKS * PUFFS {
        commands.spawn((
            Name::new("Smoke"),
            Puff(i),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::default(),
            Visibility::Hidden,
            NotShadowCaster,
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn animate(
    time: Res<Time<Real>>,
    quality: Res<GraphicsQuality>,
    index: Res<BuildingEntities>,
    models: Res<BuildingModels>,
    parents: Query<&GlobalTransform>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    mut puffs: Query<(&Puff, &mut Transform, &mut Visibility)>,
    mut stacks: Local<Stacks>,
) {
    let Ok(cam) = cams.single() else { return };
    let cam = cam.translation();
    stacks.since += time.delta_secs();
    if stacks.since > 0.5 {
        stacks.since = 0.0;
        let mut pts: Vec<(f32, Vec3)> = Vec::new();
        if *quality != GraphicsQuality::Low {
            for &(parent, key, model_tf) in index.0.values() {
                let local = models.emitters(key);
                if local.is_empty() {
                    continue;
                }
                let Ok(g) = parents.get(parent) else { continue };
                for &p in local {
                    let w = g.transform_point(model_tf.transform_point(p));
                    let d = w.distance(cam);
                    if d < RANGE {
                        pts.push((d, w));
                    }
                }
            }
        }
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        stacks.points = pts.into_iter().take(MAX_STACKS).map(|(_, p)| p).collect();
    }
    let t = time.elapsed_secs();
    let wind = Vec3::new(2.2, 0.0, 0.9);
    for (puff, mut tf, mut vis) in &mut puffs {
        let stack = puff.0 / PUFFS;
        let Some(&p) = stacks.points.get(stack) else {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        };
        let k = puff.0 % PUFFS;
        let seed = (stack as f32 * 0.37).fract();
        let phase = (t / 9.0 + k as f32 / PUFFS as f32 + seed).fract();
        let rise = phase * 13.0;
        let pos =
            p + Vec3::Y * (rise + 0.6) + wind * phase * 6.0 + Vec3::new((t * 0.7 + k as f32).sin(), 0.0, 0.0) * 0.4;
        let grow = 0.8 + phase * 2.2;
        // Fades out sooner (the tail of a long plume read as a pale band
        // across the town from afar) and with distance from the camera.
        let fade = 1.0 - ((phase - 0.55) / 0.35).clamp(0.0, 1.0);
        let fade = fade * (1.0 - ((p.distance(cam) - 350.0) / 300.0).clamp(0.0, 1.0));
        let s = grow * fade.max(0.05) * (0.85 + 0.3 * ((k as f32 * 1.7).sin() * 0.5 + 0.5));
        *tf = Transform::from_translation(pos).with_scale(Vec3::new(s, s * 0.8, s));
        vis.set_if_neq(Visibility::Inherited);
    }
}
