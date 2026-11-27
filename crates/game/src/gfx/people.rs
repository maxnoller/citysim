//! People on foot: one entity per walker the sim moves (`world.walkers`),
//! drawn with the shared figure models of `person_gen`.

use bevy::mesh::MeshTag;
use bevy::prelude::*;

use super::person_gen::{self, BUILDS, POSES};
use super::road_geom::WALK_Y;
use super::scene::WorldMaterials;

/// Figure meshes: `[build][pose]` -> (detailed, distant).
#[derive(Resource)]
pub struct PeopleModels {
    meshes: Vec<(Handle<Mesh>, Handle<Mesh>)>,
}

impl PeopleModels {
    pub fn mesh(&self, build: usize, pose: usize, far: bool) -> Handle<Mesh> {
        let (near, distant) = &self.meshes[(build % BUILDS) * POSES + pose % POSES];
        if far { distant.clone() } else { near.clone() }
    }

    pub fn count(&self) -> usize {
        self.meshes.len()
    }
}

pub struct PeoplePlugin;

impl Plugin for PeoplePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup);
        if let Some(at) = std::env::var("CITYSIM_GFX_PEOPLE_SHOWCASE").ok().and_then(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() >= 2).then(|| Vec2::new(v[0], v[1]))
        }) {
            app.add_systems(
                PostStartup,
                move |c: Commands, m: Res<PeopleModels>, w: Res<WorldMaterials>| showcase(c, m, w, at),
            );
        }
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mut handles = Vec::with_capacity(BUILDS * POSES);
    for build in 0..BUILDS {
        for pose in 0..POSES {
            handles.push((
                meshes.add(person_gen::person_model(build, pose, false).build()),
                meshes.add(person_gen::person_model(build, pose, true).build()),
            ));
        }
    }
    commands.insert_resource(PeopleModels { meshes: handles });
}

/// Dev aid (`CITYSIM_GFX_PEOPLE_SHOWCASE=x,z`): every body type in rows at
/// sim point `(x, z)`, standing and at several phases of the walk, in
/// varied colours and sizes (children and the old at their heights).
fn showcase(mut commands: Commands, models: Res<PeopleModels>, mats: Res<WorldMaterials>, at: Vec2) {
    let poses = [person_gen::STAND, 0, 1, 2, 3, 5, 6];
    for build in 0..BUILDS {
        for (k, &pose) in poses.iter().enumerate() {
            let n = (build * poses.len() + k) as u32;
            let h = super::noise::hash(n.wrapping_mul(0x9E37_79B9) ^ 0x5EED);
            let scale = match build {
                person_gen::CHILD_BUILD => 0.62 + 0.08 * (k % 3) as f32,
                person_gen::SENIOR_BUILD => 0.95,
                _ => 0.94 + 0.12 * ((h >> 20) % 100) as f32 / 100.0,
            };
            let hair = if build == person_gen::SENIOR_BUILD {
                6 + (h >> 13) % 2
            } else {
                (h >> 13) % 6
            };
            let tag = person_gen::person_tag(h % 16, (h >> 4) % 8, (h >> 8) % 6, hair);
            let p = at + Vec2::new(k as f32 * 1.5, build as f32 * 2.0);
            commands.spawn((
                Mesh3d(models.mesh(build, pose, false)),
                MeshMaterial3d(mats.lit.clone()),
                MeshTag::new(tag),
                Transform::from_xyz(p.x, WALK_Y, p.y)
                    .with_rotation(Quat::from_rotation_y(0.0))
                    .with_scale(Vec3::splat(scale)),
            ));
        }
    }
}
