//! Traffic overlay (`OverlayMode::Traffic`): a light veil dims the map and
//! each direction of every segment gets a strip coloured green -> yellow ->
//! red by its load / capacity (`congestion_in`). Strips follow the road
//! layout (they stop at junction edges) and refresh a few times a second.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::{MAP_SIZE_M, RoadNetwork};

use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, mix};
use super::road_geom::{self, RoadLayout};
use crate::shared::{GameSet, OverlayMode, SimWorld};

const REFRESH_SECS: f32 = 0.25;
const STRIP_Y: f32 = 0.30;

#[derive(Component)]
struct TrafficStrips;

#[derive(Component)]
struct Veil;

#[derive(Default)]
struct OverlayState {
    layout_key: Option<(u64, u64)>,
    layout: RoadLayout,
    last: f32,
}

pub struct OverlayPlugin;

impl Plugin for OverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, draw_traffic.in_set(GameSet::Render));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let veil_mat = materials.add(StandardMaterial {
        base_color: mix(palette::SKY_HORIZON, Color::WHITE, 0.3).with_alpha(0.5),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    let m = 400.0;
    let mut b = MeshBuilder::new();
    b.quad_up(
        [
            Vec3::new(-m, 0.0, -m),
            Vec3::new(MAP_SIZE_M + m, 0.0, -m),
            Vec3::new(MAP_SIZE_M + m, 0.0, MAP_SIZE_M + m),
            Vec3::new(-m, 0.0, MAP_SIZE_M + m),
        ],
        [1.0; 4],
    );
    commands.spawn((
        Name::new("Traffic veil"),
        Veil,
        Mesh3d(meshes.add(b.build())),
        MeshMaterial3d(veil_mat),
        Transform::from_xyz(0.0, 0.22, 0.0),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Traffic overlay"),
        TrafficStrips,
        // Opaque, so the (transparent) veil can never be drawn over it.
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            depth_bias: 4.0,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
}

/// Overlay colour for a congestion level in `[0, 1]`.
pub fn congestion_color(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        mix(palette::TRAFFIC_FREE, palette::TRAFFIC_BUSY, t * 2.0)
    } else {
        mix(palette::TRAFFIC_BUSY, palette::TRAFFIC_JAM, (t - 0.5) * 2.0)
    }
}

fn strips(roads: &RoadNetwork, layout: &RoadLayout) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    for s in &layout.segs {
        let Some(seg) = s.id.and_then(|id| roads.segments.get(id)) else {
            continue;
        };
        let dir = s.dir();
        let n = Vec2::new(-dir.y, dir.x);
        let (t0, t1) = (s.s_a, s.len() - s.s_b);
        if t1 - t0 < 0.1 {
            continue;
        }
        let h = s.style.h;
        // Forward travel (a -> b) drives on the +n side.
        for (forward, side) in [(true, 1.0f32), (false, -1.0)] {
            let c = lin(congestion_color(seg.congestion_in(forward)));
            let (o0, o1) = (side * 0.2, side * (h - 0.15));
            let q = [
                s.a + dir * t0 + n * o0,
                s.a + dir * t1 + n * o0,
                s.a + dir * t1 + n * o1,
                s.a + dir * t0 + n * o1,
            ];
            m.quad_up(q.map(|p| Vec3::new(p.x, STRIP_Y, p.y)), c);
        }
    }
    m
}

#[allow(clippy::too_many_arguments)]
fn draw_traffic(
    mut commands: Commands,
    mode: Res<OverlayMode>,
    sim: Res<SimWorld>,
    time: Res<Time<Real>>,
    mut state: Local<OverlayState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut strips_q: Query<(Entity, &mut Visibility), (With<TrafficStrips>, Without<Veil>)>,
    mut veil: Query<&mut Visibility, (With<Veil>, Without<TrafficStrips>)>,
) {
    let on = *mode == OverlayMode::Traffic;
    let Ok((entity, mut vis)) = strips_q.single_mut() else {
        return;
    };
    let target = if on { Visibility::Inherited } else { Visibility::Hidden };
    vis.set_if_neq(target);
    for mut v in &mut veil {
        v.set_if_neq(target);
    }
    if !on {
        state.last = -1.0;
        return;
    }
    let key = sim.key(sim.world.versions.roads);
    let roads_changed = state.layout_key != Some(key);
    if roads_changed {
        state.layout = road_geom::layout(&sim.world.roads);
        state.layout_key = Some(key);
    }
    let now = time.elapsed_secs();
    if !roads_changed && !mode.is_changed() && now - state.last < REFRESH_SECS && state.last >= 0.0 {
        return;
    }
    state.last = now;
    let m = strips(&sim.world.roads, &state.layout);
    if m.is_empty() {
        commands.entity(entity).remove::<Mesh3d>();
    } else {
        commands.entity(entity).insert(Mesh3d(meshes.add(m.build())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn congestion_runs_green_to_red() {
        let free = congestion_color(0.0).to_srgba();
        let jam = congestion_color(1.0).to_srgba();
        assert!(free.green > free.red);
        assert!(jam.red > jam.green);
    }
}
