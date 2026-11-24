//! Vehicles: one entity per `world.vehicles` entry with a shared low-poly
//! car model (shape x colour from the vehicle seed).
//!
//! Placement uses only the road graph and each vehicle's `segment`,
//! `forward` and `progress` (never the grid):
//! * cars drive in the right-hand lane, offset along the segment's exact
//!   direction (so diagonals and highways are consistent);
//! * the sim's lane `0..length` (node centre to node centre) is mapped onto
//!   the drawn stretch between junction surfaces, so a car waiting at the
//!   end of its lane stands at the stop line, not in the junction; the
//!   mapping never compresses below 0.82, so cars queued nose-to-tail at
//!   the sim's minimum spacing never overlap;
//! * when a car moves onto its next segment it follows a curve from where
//!   it was through the junction into the new lane;
//! * positions are sampled each sim tick and interpolated between ticks.

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::{RoadKind, SegmentId, TICKS_PER_SECOND, Vehicle, VehicleId, World};

use super::noise::hash;
use super::palette;
use super::road_geom::{self, ROAD_Y, lane_offset};
use super::scene::WorldMaterials;
use super::vehicle_gen::{self, SHAPES};
use crate::shared::{GameSet, SimWorld, WorldCamera};

/// If a vehicle moves further than this in one tick it teleported (new trip).
const TELEPORT_M: f32 = 30.0;
/// Smallest lane compression (keeps queued cars from overlapping: the sim
/// queues at 5.5 m and the longest car is 4.4 m).
const MIN_COMPRESSION: f32 = 0.82;
/// Where cars stop before a junction with crosswalks (metres from its edge).
const STOP_BACK: f32 = 3.5;

/// Camera distance beyond which cars use the simplified model (with a
/// little hysteresis so they do not flicker between models).
const LOD_FAR: f32 = 480.0;
const LOD_NEAR: f32 = 440.0;

#[derive(Resource)]
pub struct VehicleModels {
    /// `[shape * colours + colour]` -> (detailed, distant).
    meshes: Vec<(Handle<Mesh>, Handle<Mesh>)>,
}

impl VehicleModels {
    pub fn count(&self) -> usize {
        self.meshes.len()
    }
}

/// Drawn geometry of one segment's lanes.
#[derive(Debug, Clone, Copy)]
pub struct LaneGeom {
    pub a: Vec2,
    pub b: Vec2,
    pub kind: RoadKind,
    /// Where the drawn lane starts / ends, metres from `a` along `a -> b`,
    /// for travel `a -> b` (start, end) and `b -> a` (start, end) measured
    /// from the travel origin.
    pub fwd: (f32, f32),
    pub back: (f32, f32),
}

impl LaneGeom {
    pub fn len(&self) -> f32 {
        (self.b - self.a).length()
    }

    /// Lane point and travel direction for `progress` metres along the
    /// sim lane in the given direction, `offset` metres to the right.
    pub fn point(&self, forward: bool, progress: f32, offset: f32) -> (Vec2, Vec2) {
        let len = self.len().max(1e-3);
        let (from, to, (e0, e1)) = if forward {
            (self.a, self.b, self.fwd)
        } else {
            (self.b, self.a, self.back)
        };
        let dir = (to - from) / len;
        let k = ((e1 - e0) / len).max(MIN_COMPRESSION);
        let t = e0 + progress.clamp(0.0, len) * k;
        // Right of travel in sim space is (-dir.y, dir.x).
        (from + dir * t + Vec2::new(-dir.y, dir.x) * offset, dir)
    }
}

/// Lane geometry for every segment, from the road layout.
pub fn lane_geoms(world: &World) -> HashMap<SegmentId, LaneGeom> {
    let layout = road_geom::layout(&world.roads);
    let end = |s: f32, deg: usize| if deg >= 3 { s + STOP_BACK } else { s };
    layout
        .segs
        .iter()
        .filter_map(|s| {
            let id = s.id?;
            let len = s.len();
            Some((
                id,
                LaneGeom {
                    a: s.a,
                    b: s.b,
                    kind: s.kind,
                    fwd: (s.s_a, len - end(s.s_b, s.deg_b)),
                    back: (s.s_b, len - end(s.s_a, s.deg_a)),
                },
            ))
        })
        .collect()
}

/// A car moving through a junction onto its next segment.
#[derive(Debug, Clone, Copy)]
struct Crossing {
    p0: Vec2,
    p1: Vec2,
    start_tick: u64,
    ticks: f32,
}

#[derive(Component)]
struct VehicleVisual {
    prev: (Vec2, f32),
    cur: (Vec2, f32),
    seg: Option<(SegmentId, bool)>,
    crossing: Option<Crossing>,
    model: usize,
    lod: bool,
}

#[derive(Default)]
struct SyncState {
    entities: HashMap<VehicleId, Entity>,
    lanes: HashMap<SegmentId, LaneGeom>,
    roads_key: Option<(u64, u64)>,
    last_fixed: f64,
    last_tick: u64,
    generation: u64,
    /// The sim advanced on the most recent fixed update.
    moving: bool,
}

pub struct VehiclesPlugin;

impl Plugin for VehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, sync_vehicles.in_set(GameSet::Render));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mut handles = Vec::new();
    for shape in 0..SHAPES {
        for &c in &palette::CAR_BODIES {
            handles.push((
                meshes.add(vehicle_gen::car_model(c, shape, false).build()),
                meshes.add(vehicle_gen::car_model(c, shape, true).build()),
            ));
        }
    }
    commands.insert_resource(VehicleModels { meshes: handles });
}

fn model_index(seed: u32) -> usize {
    let h = hash(seed ^ 0x51ED_270B);
    // Mostly sedans and hatchbacks, some vans and pickups.
    let shape = match h % 10 {
        0..=4 => 0,
        5..=7 => 1,
        8 => 2,
        _ => 3,
    };
    let colours = palette::CAR_BODIES.len();
    shape * colours + (h / 16) as usize % colours
}

/// Where the sim says the car is (lane position and travel direction).
fn lane_sample(lanes: &HashMap<SegmentId, LaneGeom>, v: &Vehicle) -> (Vec2, Vec2) {
    if let Some(g) = v.segment.and_then(|s| lanes.get(&s)) {
        return g.point(v.forward, v.progress, lane_offset(g.kind, v.seed));
    }
    let dir = Vec2::from_angle(v.heading);
    let off = lane_offset(RoadKind::Street, v.seed);
    (Vec2::from(v.pos) + Vec2::new(-dir.y, dir.x) * off, dir)
}

fn bezier(p0: Vec2, p1: Vec2, p2: Vec2, t: f32) -> (Vec2, Vec2) {
    let u = 1.0 - t;
    let p = p0 * u * u + p1 * 2.0 * u * t + p2 * t * t;
    let d = (p1 - p0) * 2.0 * u + (p2 - p1) * 2.0 * t;
    (p, d)
}

/// Control point for a turn from `p0` heading `d0` into `p2` heading `d2`:
/// where the two lanes meet, or the midpoint when they are (nearly) parallel.
fn turn_control(p0: Vec2, d0: Vec2, p2: Vec2, d2: Vec2) -> Vec2 {
    let denom = d0.perp_dot(d2);
    let mid = (p0 + p2) * 0.5;
    if denom.abs() < 0.05 {
        return mid;
    }
    let t = (p2 - p0).perp_dot(d2) / denom;
    let p1 = p0 + d0 * t;
    if !(0.0..=25.0).contains(&t) || (p1 - p2).dot(d2) > 0.0 {
        mid
    } else {
        p1
    }
}

/// New visual sample for a car (position, heading), updating its state.
fn advance(vis: &mut VehicleVisual, v: &Vehicle, lanes: &HashMap<SegmentId, LaneGeom>, tick: u64) -> (Vec2, f32) {
    let (p2, d2) = lane_sample(lanes, v);
    let seg = v.segment.map(|s| (s, v.forward));
    if seg != vis.seg {
        let p0 = vis.cur.0;
        let d0 = Vec2::from_angle(vis.cur.1);
        vis.crossing = None;
        let dist = p0.distance(p2);
        if vis.seg.is_some() && seg.is_some() && dist > 0.3 && dist < TELEPORT_M {
            let ticks = (dist / v.speed.max(4.0) * TICKS_PER_SECOND as f32).clamp(3.0, 40.0);
            vis.crossing = Some(Crossing {
                p0,
                p1: turn_control(p0, d0, p2, d2),
                start_tick: tick,
                ticks,
            });
        }
        vis.seg = seg;
    }
    if let Some(c) = vis.crossing {
        let t = (tick.saturating_sub(c.start_tick) as f32 / c.ticks).min(1.0);
        if t < 1.0 {
            let (p, d) = bezier(c.p0, c.p1, p2, t);
            let d = if d.length_squared() > 1e-6 { d } else { d2 };
            return (p, d.y.atan2(d.x));
        }
        vis.crossing = None;
    }
    (p2, d2.y.atan2(d2.x))
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let d = (b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    a + d * t
}

#[allow(clippy::too_many_arguments)]
fn sync_vehicles(
    mut commands: Commands,
    sim: Res<SimWorld>,
    fixed: Res<Time<Fixed>>,
    models: Res<VehicleModels>,
    mats: Res<WorldMaterials>,
    mut state: Local<SyncState>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<(&mut VehicleVisual, &mut Transform, &mut Mesh3d)>,
) {
    let cam = cams.single().map_or(Vec3::ZERO, |c| c.translation());
    let world = &sim.world;
    if state.generation != sim.generation {
        for (_, e) in state.entities.drain() {
            commands.entity(e).despawn();
        }
        state.generation = sim.generation;
    }
    let roads_key = sim.key(world.versions.roads);
    if state.roads_key != Some(roads_key) {
        state.lanes = lane_geoms(world);
        state.roads_key = Some(roads_key);
    }
    let vehicles = &world.vehicles;
    state.entities.retain(|id, e| {
        let keep = vehicles.contains_key(*id);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    // Did a fixed update run since last frame, and did the sim advance?
    let fixed_now = fixed.elapsed_secs_f64();
    let stepped = fixed_now != state.last_fixed;
    let tick = world.time.tick;
    let advanced = tick != state.last_tick;
    state.last_fixed = fixed_now;
    state.last_tick = tick;
    if stepped {
        state.moving = advanced;
    }
    let alpha = if state.moving {
        fixed.overstep_fraction().clamp(0.0, 1.0)
    } else {
        1.0
    };
    let state = &mut *state;
    for (id, v) in vehicles.iter() {
        match state.entities.get(&id) {
            Some(&e) => {
                let Ok((mut vis, mut t, mut mesh)) = q.get_mut(e) else {
                    continue;
                };
                if stepped && advanced {
                    let now = advance(&mut vis, v, &state.lanes, tick);
                    vis.prev = vis.cur;
                    vis.cur = now;
                    if vis.prev.0.distance(vis.cur.0) > TELEPORT_M {
                        vis.prev = vis.cur;
                    }
                } else if stepped {
                    vis.prev = vis.cur;
                }
                let p = vis.prev.0.lerp(vis.cur.0, alpha);
                let h = lerp_angle(vis.prev.1, vis.cur.1, alpha);
                let new = Transform::from_xyz(p.x, ROAD_Y, p.y).with_rotation(Quat::from_rotation_y(-h));
                if *t != new {
                    *t = new;
                }
                let d = cam.distance(new.translation);
                let lod = if vis.lod { d > LOD_NEAR } else { d > LOD_FAR };
                if lod != vis.lod {
                    vis.lod = lod;
                    let (near, far) = &models.meshes[vis.model];
                    mesh.0 = if lod { far.clone() } else { near.clone() };
                }
            }
            None => {
                let (p, d) = lane_sample(&state.lanes, v);
                let h = d.y.atan2(d.x);
                let model = model_index(v.seed);
                let pos = Vec3::new(p.x, ROAD_Y, p.y);
                let lod = cam.distance(pos) > LOD_FAR;
                let (near, far) = &models.meshes[model];
                let e = commands
                    .spawn((
                        VehicleVisual {
                            prev: (p, h),
                            cur: (p, h),
                            seg: v.segment.map(|s| (s, v.forward)),
                            crossing: None,
                            model,
                            lod,
                        },
                        Mesh3d(if lod { far.clone() } else { near.clone() }),
                        MeshMaterial3d(mats.lit.clone()),
                        Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(-h)),
                    ))
                    .id();
                state.entities.insert(id, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citysim_sim::{Cell, Command};

    #[test]
    fn lane_is_to_the_right_of_travel() {
        // Heading +x in sim (east). Right-hand traffic: the lane is on the
        // driver's right, i.e. (heading x up) in 3D -> +z in Bevy.
        let heading = 0.0f32;
        let (s, c) = heading.sin_cos();
        let right = Vec2::new(-s, c);
        let forward3 = Vec3::new(c, 0.0, s);
        let right3 = forward3.cross(Vec3::Y);
        assert!(right3.distance(Vec3::new(right.x, 0.0, right.y)) < 1e-6);
    }

    #[test]
    fn angle_lerp_takes_the_short_way() {
        let a = lerp_angle(3.0, -3.0, 0.5);
        assert!((a.abs() - std::f32::consts::PI).abs() < 0.3, "{a}");
    }

    fn town() -> World {
        let mut w = World::new(1);
        w.economy.money = 1_000_000;
        for (a, b) in [((11, 128), (40, 128)), ((30, 110), (30, 150)), ((40, 128), (55, 113))] {
            w.apply(Command::BuildRoad {
                from: Cell::new(a.0, a.1),
                to: Cell::new(b.0, b.1),
            })
            .expect("road");
        }
        w
    }

    #[test]
    fn queued_cars_never_overlap_and_stop_before_the_junction() {
        let w = town();
        let lanes = lane_geoms(&w);
        let junction = Vec2::from(Cell::new(30, 128).center());
        for (sid, g) in &lanes {
            let seg = &w.roads.segments[*sid];
            for forward in [true, false] {
                let len = g.len();
                // Cars at the sim's minimum queue spacing from the lane end.
                let mut p = len - 0.6;
                let mut last: Option<Vec2> = None;
                while p > 0.0 {
                    let (pos, _) = g.point(forward, p, 1.35);
                    if let Some(l) = last {
                        assert!(l.distance(pos) >= 4.4, "overlap on {sid:?}: {}", l.distance(pos));
                    }
                    last = Some(pos);
                    p -= 5.5;
                }
                // The head of the queue does not stand inside a crossing.
                let end = if forward { seg.b } else { seg.a };
                if w.roads.nodes[end].segments.len() >= 3 && Vec2::from(w.roads.nodes[end].pos) == junction {
                    let (head, _) = g.point(forward, len - 0.6, 1.35);
                    assert!(head.distance(junction) > 6.0, "head car in the junction");
                }
            }
        }
    }

    #[test]
    fn diagonal_lanes_are_offset_perpendicular_to_travel() {
        let w = town();
        let lanes = lane_geoms(&w);
        for g in lanes.values() {
            for forward in [true, false] {
                let (on, dir) = g.point(forward, 3.0, 0.0);
                let (off, _) = g.point(forward, 3.0, 1.35);
                let side = off - on;
                assert!((side.length() - 1.35).abs() < 1e-3);
                assert!(side.dot(dir).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn turn_curve_starts_and_ends_on_the_lanes() {
        let p0 = Vec2::new(0.0, 0.0);
        let d0 = Vec2::X;
        let p2 = Vec2::new(8.0, 8.0);
        let d2 = Vec2::Y;
        let p1 = turn_control(p0, d0, p2, d2);
        assert!(p1.distance(Vec2::new(8.0, 0.0)) < 1e-4);
        let (a, _) = bezier(p0, p1, p2, 0.0);
        let (b, db) = bezier(p0, p1, p2, 1.0);
        assert!(a.distance(p0) < 1e-5 && b.distance(p2) < 1e-5);
        assert!(db.normalize().distance(d2) < 1e-4);
    }
}
