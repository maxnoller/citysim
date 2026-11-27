//! Traffic overlay (`OverlayMode::Traffic`): a light veil dims the map and
//! each direction of every segment gets a strip coloured by its load /
//! capacity (`congestion_in`) in the bands shared with the ui legend:
//! green below `CONGESTION_BUSY`, amber up to `CONGESTION_JAM`, red from it
//! on. Strips follow the road layout (they stop at junction edges) and
//! refresh a few times a second.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::{MAP_SIZE_M, RoadNetwork};

use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, mix};
use super::road_geom::{self, RoadLayout};
use crate::shared::{CONGESTION_BUSY, CONGESTION_JAM, GameSet, OverlayMode, SimWorld};

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

/// Overlay colour for a congestion level (`congestion_in`, 0..=1): one hue
/// per band, with a gentle ramp inside each band (greener when quieter,
/// deeper red when fully stopped) so the band edges stay unmistakable.
pub fn congestion_color(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    if t < CONGESTION_BUSY {
        let k = t / CONGESTION_BUSY;
        mix(
            palette::TRAFFIC_FREE,
            mix(palette::TRAFFIC_FREE, palette::TRAFFIC_BUSY, 0.2),
            k,
        )
    } else if t < CONGESTION_JAM {
        let k = (t - CONGESTION_BUSY) / (CONGESTION_JAM - CONGESTION_BUSY);
        mix(
            palette::TRAFFIC_BUSY,
            mix(palette::TRAFFIC_BUSY, palette::TRAFFIC_JAM, 0.3),
            k,
        )
    } else {
        let k = (t - CONGESTION_JAM) / (1.0 - CONGESTION_JAM).max(1e-3);
        mix(palette::TRAFFIC_JAM, palette::shade(palette::TRAFFIC_JAM, 0.7), k)
    }
}

fn strips(roads: &RoadNetwork, layout: &RoadLayout) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    for s in &layout.segs {
        let Some(seg) = s.id.and_then(|id| roads.segments.get(id)) else {
            continue;
        };
        let (t0, t1) = s.span();
        if t1 - t0 < 0.1 {
            continue;
        }
        let h = s.style.h;
        // Forward travel (a -> b) drives on the right (+offset) side.
        for (forward, side) in [(true, 1.0f32), (false, -1.0)] {
            let c = lin(congestion_color(seg.congestion_in(forward)));
            let (o0, o1) = (side * 0.2, side * (h - 0.15));
            road_geom::strip(&mut m, &s.frame, t0, t1, o0.min(o1), o0.max(o1), STRIP_Y, c);
        }
    }
    // Junction surfaces take the colour of their worst approach, so an
    // overloaded junction reads as one red patch.
    for node in &layout.nodes {
        if node.kind != road_geom::NodeKind::Junction {
            continue;
        }
        let mut worst = 0.0f32;
        let mut ring = Vec::with_capacity(node.arms.len() * 2 + 1);
        for arm in &node.arms {
            if let Some(seg) = roads.segments.get(arm.seg) {
                // Travel towards this node is `a -> b` when it is `b`.
                worst = worst.max(seg.congestion_in(seg.b == node.id));
            }
            let h = arm.style.h - 0.15;
            ring.push(arm.at(0.0, -h));
            ring.push(arm.at(0.0, h));
        }
        let ring: Vec<Vec3> = ring.iter().map(|p| Vec3::new(p.x, STRIP_Y, p.y)).collect();
        m.fan_up(
            Vec3::new(node.pos.x, STRIP_Y, node.pos.y),
            &ring,
            lin(congestion_color(worst)),
        );
    }
    m
}

type StripsQuery = (Entity, &'static mut Visibility);

#[allow(clippy::too_many_arguments)]
fn draw_traffic(
    mut commands: Commands,
    mode: Res<OverlayMode>,
    sim: Res<SimWorld>,
    time: Res<Time<Real>>,
    mut state: Local<OverlayState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut strips_q: Query<StripsQuery, (With<TrafficStrips>, Without<Veil>)>,
    mut veil: Query<&mut Visibility, (With<Veil>, Without<TrafficStrips>)>,
    shared: Res<super::roads::SharedLayout>,
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
        state.layout = super::roads::layout_of(&shared, &sim);
        state.layout_key = Some(key);
    }
    let now = time.elapsed_secs();
    if !roads_changed && !mode.is_changed() && now - state.last < REFRESH_SECS && state.last >= 0.0 {
        return;
    }
    state.last = now;
    let m = strips(&sim.world.roads, &state.layout);
    if m.is_empty() {
        commands.entity(entity).insert(Visibility::Hidden); // never remove Mesh3d: see cursor.rs
    } else {
        commands
            .entity(entity)
            .insert((Mesh3d(meshes.add(m.build())), Visibility::Inherited));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hue(t: f32) -> f32 {
        Hsla::from(congestion_color(t)).hue
    }

    #[test]
    fn congestion_bands_are_green_amber_red() {
        // Green (hue ~ 90-150) below BUSY.
        for t in [0.0, 0.25, CONGESTION_BUSY - 0.01] {
            assert!((85.0..160.0).contains(&hue(t)), "{t}: {}", hue(t));
        }
        // Amber (hue ~ 25-55) between BUSY and JAM.
        for t in [CONGESTION_BUSY, 0.65, CONGESTION_JAM - 0.01] {
            assert!((25.0..55.0).contains(&hue(t)), "{t}: {}", hue(t));
        }
        // Red (hue < 15 or > 345) from JAM on.
        for t in [CONGESTION_JAM, 0.9, 1.0, 1.5] {
            let h = hue(t);
            assert!(!(15.0..345.0).contains(&h), "{t}: {h}");
        }
    }
}
