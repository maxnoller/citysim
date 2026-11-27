//! Vehicles: one entity per `world.vehicles` entry with a shared low-poly
//! model that tells the trip (`model_for`):
//! * a household's own car (saloon, hatchback, estate, SUV, van, pickup in
//!   realistic paints) for commuting, shopping and leisure, chosen by the
//!   household's seed, so a family keeps its car all day;
//! * a hired moving van for a household moving in or out;
//! * goods between the highway and the town (imports, exports): a
//!   long-distance lorry (box, curtain-side, tipper, flatbed, tanker);
//!   goods from industry to a shop in town: a delivery truck.
//!
//! The sim spaces goods vehicles by the truck length and everything else
//! by the car length (`drive::stop_back_m`): moving vans are car length,
//! lorries and delivery trucks within the truck length.
//!
//! Placement follows the sim exactly. Its point for a vehicle is the front
//! of the car (the sim stops that point at the stop line and lets the car
//! into the junction box when the point reaches it), so the body is drawn
//! behind it:
//! * on a lane: the segment centre line at `progress`, offset to the sim's
//!   lane (`lane` plus what is left of a lane change, `lane_shift`);
//! * in a junction box: along the movement's curve (`JunctionMove`), the
//!   rear on the approach lane while it is not yet in the box.
//!
//! Samples are taken once per sim tick and interpolated between ticks.
//! Brake lights come on when the sim says the car brakes or it stands.

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::{RoadKind, TripPurpose, Vehicle, VehicleId, World};

use bevy::mesh::MeshTag;
use bevy::tasks::{ComputeTaskPool, TaskPool};
use citysim_sim::Place;

use super::building_gen::Lod;
use super::city_material::TAG_BRAKE;
use super::noise::hash;
use super::palette;
use super::road_geom::{ROAD_Y, lane_offset};
use super::scene::{QualityApplied, WorldMaterials, drawn_quality};
use super::vehicle_gen;
use crate::shared::{GameSet, GraphicsQuality, SimWorld, WorldCamera};

/// If a vehicle moves further than this in one tick it teleported.
const TELEPORT_M: f32 = 30.0;

/// Camera distance beyond which vehicles use the distant model (with a
/// little hysteresis so they do not flicker between models).
const LOD_FAR: f32 = 480.0;
const LOD_NEAR: f32 = 440.0;
/// Hysteresis at the near / middle switch, metres.
const NEAR_HYST: f32 = 8.0;

/// Distance up to which vehicles show their near model, per quality.
fn near_distance(q: GraphicsQuality) -> f32 {
    match q {
        GraphicsQuality::Low => 0.0,
        GraphicsQuality::Medium => 60.0,
        GraphicsQuality::High => 90.0,
    }
}

/// Which model a vehicle shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum VModel {
    /// A private car: body type and paint.
    Car { shape: u8, paint: u8 },
    /// A hired moving van (livery).
    Mover(u8),
    /// A delivery truck from industry to a shop (body, livery).
    Delivery { kind: u8, livery: u8 },
    /// A long-distance lorry to or from the highway (body, livery).
    Lorry { kind: u8, livery: u8 },
}

impl VModel {
    /// Length and width, metres.
    pub fn size(self) -> (f32, f32) {
        match self {
            VModel::Car { shape, .. } => vehicle_gen::car_size(shape as u32),
            VModel::Mover(_) => vehicle_gen::MOVING_VAN_SIZE,
            VModel::Delivery { .. } => (vehicle_gen::TRUCK_LEN, vehicle_gen::TRUCK_WIDTH),
            VModel::Lorry { .. } => (vehicle_gen::LORRY_LEN, vehicle_gen::LORRY_WIDTH),
        }
    }

    fn build(self, lod: Lod) -> Mesh {
        match self {
            VModel::Car { shape, paint } => {
                vehicle_gen::car_model(palette::CAR_BODIES[paint as usize], shape as u32, lod)
            }
            VModel::Mover(l) => vehicle_gen::moving_van(l as usize, lod),
            VModel::Delivery { kind, livery } => {
                let (a, b, c) = palette::TRUCK_LIVERIES[livery as usize];
                vehicle_gen::truck_model(a, b, c, kind as u32, lod)
            }
            VModel::Lorry { kind, livery } => {
                let (a, b, c) = vehicle_gen::LORRY_LIVERIES[livery as usize];
                vehicle_gen::lorry_model(a, b, c, kind as u32, lod)
            }
        }
        .build()
    }
}

/// A private car body for a seed (the household's): mostly saloons,
/// hatchbacks, estates and SUVs, some vans and pickups.
pub(super) fn car_for(seed: u32) -> VModel {
    let h = hash(seed ^ 0x51ED_270B);
    let shape = match h % 12 {
        0..=2 => 0,
        3..=5 => 1,
        6 | 7 => 4,
        8 | 9 => 5,
        10 => 2,
        _ => 3,
    };
    VModel::Car {
        shape,
        paint: vehicle_gen::paint(h / 16) as u8,
    }
}

/// The model that tells a vehicle's trip (see the module docs).
pub(super) fn model_for(world: &World, id: VehicleId, v: &Vehicle) -> VModel {
    let trip = world.traffic.trips.get(id);
    let h = hash(v.seed ^ 0x2C1B_3C6D);
    match trip {
        Some(t) if t.purpose == TripPurpose::Goods => {
            let export = t.to == Place::Entry;
            if export || t.from == Place::Entry {
                // Exports leave in anything; imports come boxed or
                // curtained (or on a flatbed).
                let kind = if export {
                    h % vehicle_gen::LORRY_KINDS
                } else {
                    [0, 3, 3, 0, 2][h as usize % 5]
                };
                VModel::Lorry {
                    kind: kind as u8,
                    livery: ((h / 8) as usize % vehicle_gen::LORRY_LIVERIES.len()) as u8,
                }
            } else {
                VModel::Delivery {
                    kind: if h.is_multiple_of(5) { 2 } else { 0 },
                    livery: ((h / 8) as usize % palette::TRUCK_LIVERIES.len()) as u8,
                }
            }
        }
        Some(t) if matches!(t.purpose, TripPurpose::MoveIn | TripPurpose::MoveOut) => {
            let seed = household_seed(world, t).unwrap_or(v.seed);
            VModel::Mover((hash(seed) % vehicle_gen::MOVING_LIVERIES.len() as u32) as u8)
        }
        Some(t) => car_for(household_seed(world, t).unwrap_or(v.seed)),
        None => car_for(v.seed),
    }
}

/// Seed of the household a trip belongs to (its driver's or owner's).
fn household_seed(world: &World, trip: &citysim_sim::traffic::Trip) -> Option<u32> {
    let c = trip.citizen.or(trip.owner)?;
    let hh = world.citizens.get(c)?.household;
    world.households.get(hh).map(|h| h.seed)
}

#[derive(Resource, Default)]
pub struct VehicleModels {
    /// Every model shown so far: (near, mid, far).
    meshes: HashMap<VModel, [Handle<Mesh>; 3]>,
}

impl VehicleModels {
    pub fn count(&self) -> usize {
        self.meshes.len()
    }

    /// Build the models of `wanted` not built yet (side by side on the
    /// compute threads).
    fn prepare(&mut self, wanted: impl IntoIterator<Item = VModel>, meshes: &mut Assets<Mesh>) {
        let mut todo: Vec<VModel> = wanted.into_iter().filter(|m| !self.meshes.contains_key(m)).collect();
        todo.sort();
        todo.dedup();
        if todo.is_empty() {
            return;
        }
        let pool = ComputeTaskPool::get_or_init(TaskPool::default);
        let built: Vec<(VModel, [Mesh; 3])> = pool.scope(|s| {
            for m in todo {
                s.spawn(async move { (m, Lod::ALL.map(|l| m.build(l))) });
            }
        });
        for (k, [a, b, c]) in built {
            self.meshes.insert(k, [meshes.add(a), meshes.add(b), meshes.add(c)]);
        }
    }

    fn mesh(&self, model: VModel, level: usize) -> Handle<Mesh> {
        self.meshes[&model][level.min(2)].clone()
    }
}

#[derive(Component)]
pub(super) struct VehicleVisual {
    /// (centre, heading) at the previous and the latest sim tick.
    prev: (Vec2, f32),
    cur: (Vec2, f32),
    seed: u32,
    model: VModel,
    /// The model and level of detail the entity's mesh shows.
    mesh_model: Option<(VModel, usize)>,
    /// Level of detail: 0 near, 1 middle, 2 far.
    lod: usize,
    /// Speed at the last sample, and whether the brake lights are on.
    speed: f32,
    brake: bool,
    /// Brake state the entity's material shows.
    shown_brake: Option<bool>,
}

/// Brake lights: the sim's flag, or standing.
fn braking(v: &Vehicle) -> bool {
    v.braking || v.speed < 0.8
}

impl VehicleVisual {
    fn new(world: &World, id: VehicleId, v: &Vehicle) -> Self {
        let model = model_for(world, id, v);
        let now = place(world, v, model.size().0 * 0.5);
        VehicleVisual {
            prev: now,
            cur: now,
            seed: v.seed,
            model,
            mesh_model: None,
            lod: 1,
            speed: v.speed,
            brake: braking(v),
            shown_brake: None,
        }
    }

    /// Take the sim's newest state of the vehicle (once per sim tick).
    fn step(&mut self, world: &World, id: VehicleId, v: &Vehicle) {
        if self.seed != v.seed {
            *self = VehicleVisual {
                lod: self.lod,
                mesh_model: self.mesh_model,
                shown_brake: self.shown_brake,
                ..VehicleVisual::new(world, id, v)
            };
            return;
        }
        self.model = model_for(world, id, v);
        let now = place(world, v, self.model.size().0 * 0.5);
        self.brake = braking(v);
        self.speed = v.speed;
        self.prev = self.cur;
        self.cur = now;
        if self.prev.0.distance(self.cur.0) > TELEPORT_M {
            self.prev = self.cur;
        }
    }
}

/// Centre and heading of a vehicle's body whose front is at the sim's point.
fn place(world: &World, v: &Vehicle, half: f32) -> (Vec2, f32) {
    if let Some(jm) = &v.junction
        && let Some(m) = world
            .traffic
            .junction_states
            .get(jm.node)
            .and_then(|j| j.movements.get(jm.movement as usize))
    {
        let d = jm.distance.min(m.length) - half;
        let (p, dir) = if d >= 0.0 {
            let (p, dir) = m.pose_at(d);
            (Vec2::from(p), Vec2::from(dir))
        } else {
            // The rear is still on the approach lane.
            let (p0, d0) = m.pose_at(0.0);
            let d0 = Vec2::from(d0);
            (Vec2::from(p0) + d0 * d, d0)
        };
        return (p, dir.y.atan2(dir.x));
    }
    if let Some(s) = v.segment
        && let Some(seg) = world.roads.segments.get(s)
    {
        // Along the segment's curve, in the sim's lane; the rear of a car
        // that has only just left a junction continues straight back.
        let off = vehicle_offset(seg.kind, v);
        let d = v.progress - half;
        let pose = seg.pose_dir(v.forward, d.clamp(0.0, seg.length), off);
        let dir = Vec2::from(pose.dir);
        let p = Vec2::from(pose.pos) + dir * (d - d.clamp(0.0, seg.length));
        return (p, dir.y.atan2(dir.x));
    }
    let dir = Vec2::from_angle(v.heading);
    (Vec2::from(v.pos) - dir * half, v.heading)
}

#[derive(Default)]
struct SyncState {
    entities: HashMap<VehicleId, Entity>,
    last_fixed: f64,
    last_tick: u64,
    generation: u64,
    /// The sim advanced on the most recent fixed update.
    moving: bool,
    /// Fixed updates between the kept poses (see `sync_vehicles`).
    span: u32,
}

pub struct VehiclesPlugin;

impl Plugin for VehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VehicleModels>()
            .add_systems(Update, sync_vehicles.in_set(GameSet::Render));
    }
}

/// Lateral offset of a vehicle from the centre line (to the right of
/// travel): its sim lane, plus what is left of a lane change.
pub fn vehicle_offset(kind: RoadKind, v: &Vehicle) -> f32 {
    let base = lane_offset(kind, v.lane);
    if v.lane_shift.abs() < 1e-3 {
        return base;
    }
    // Positive shift: it came from the lane to its left (higher index).
    let other = if v.lane_shift > 0.0 {
        lane_offset(kind, v.lane.saturating_add(1))
    } else {
        lane_offset(kind, v.lane.saturating_sub(1))
    };
    base + (other - base) * v.lane_shift.abs().min(1.0)
}

/// Oriented footprints `(centre, heading, half length, half width)`
/// overlap (separating axis test), with `slack` metres of tolerance.
#[cfg(test)]
fn boxes_overlap(a: (Vec2, f32, f32, f32), b: (Vec2, f32, f32, f32), slack: f32) -> bool {
    let axes = |h: f32| [Vec2::from_angle(h), Vec2::from_angle(h).perp()];
    let extent = |bx: (Vec2, f32, f32, f32), ax: Vec2| {
        let (f, r) = (Vec2::from_angle(bx.1), Vec2::from_angle(bx.1).perp());
        bx.2 * f.dot(ax).abs() + bx.3 * r.dot(ax).abs()
    };
    for ax in axes(a.1).into_iter().chain(axes(b.1)) {
        let d = (a.0 - b.0).dot(ax).abs();
        if d + slack >= extent(a, ax) + extent(b, ax) {
            return false;
        }
    }
    true
}

/// Whether two drawn vehicles' footprints overlap.
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let d = (b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    a + d * t
}

#[allow(clippy::too_many_arguments)]
fn sync_vehicles(
    mut commands: Commands,
    sim: Res<SimWorld>,
    fixed: Res<Time<Fixed>>,
    mut models: ResMut<VehicleModels>,
    mut meshes: ResMut<Assets<Mesh>>,
    quality: (Option<Res<QualityApplied>>, Res<GraphicsQuality>),
    mats: Res<WorldMaterials>,
    mut state: Local<SyncState>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<(&mut VehicleVisual, &mut Transform, &mut Mesh3d, &mut MeshTag)>,
) {
    let cam = cams.single().map_or(Vec3::ZERO, |c| c.translation());
    let world = &sim.world;
    if state.generation != sim.generation {
        for (_, e) in state.entities.drain() {
            commands.entity(e).despawn();
        }
        state.generation = sim.generation;
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
    if stepped {
        // Fast speeds run several fixed updates a frame: the poses kept
        // (`prev`, `cur`) are this many fixed updates apart.
        let period = fixed.timestep().as_secs_f64().max(1e-6);
        state.span = (((fixed_now - state.last_fixed) / period).round() as u32).clamp(1, 16);
        state.moving = advanced;
    }
    state.last_fixed = fixed_now;
    state.last_tick = tick;
    // Draw one fixed update behind the newest pose: the same lag at every
    // speed, so cars move evenly whatever the frame rate.
    let alpha = if state.moving {
        let span = state.span.max(1) as f32;
        ((span - 1.0 + fixed.overstep_fraction().clamp(0.0, 1.0)) / span).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let tick_now = stepped && advanced;
    let near_m = near_distance(drawn_quality(quality.0.as_deref(), *quality.1));
    let level_at = |d: f32, was: usize| {
        let far = if was == 2 { d > LOD_NEAR } else { d > LOD_FAR };
        let near = if was == 0 {
            d < near_m + NEAR_HYST
        } else {
            d < near_m - NEAR_HYST
        };
        if far {
            2
        } else if near && near_m > 0.0 {
            0
        } else {
            1
        }
    };
    // Models of new vehicles (and of trips that changed), built side by
    // side before they are needed.
    if tick_now || state.entities.len() != vehicles.len() {
        models.prepare(vehicles.iter().map(|(id, v)| model_for(world, id, v)), &mut meshes);
    }
    for (id, v) in vehicles.iter() {
        match state.entities.get(&id) {
            Some(&e) => {
                let Ok((mut vis, _, _, _)) = q.get_mut(e) else {
                    continue;
                };
                if tick_now {
                    vis.step(world, id, v);
                } else if stepped {
                    vis.prev = vis.cur;
                }
            }
            None => {
                let mut vis = VehicleVisual::new(world, id, v);
                let (p, h) = vis.cur;
                let pos = Vec3::new(p.x, ROAD_Y, p.y);
                vis.lod = level_at(cam.distance(pos), 1);
                vis.mesh_model = Some((vis.model, vis.lod));
                let mesh = models.mesh(vis.model, vis.lod);
                let e = commands
                    .spawn((
                        vis,
                        Mesh3d(mesh),
                        MeshMaterial3d(mats.lit.clone()),
                        MeshTag::new(0),
                        Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(-h)),
                    ))
                    .id();
                state.entities.insert(id, e);
            }
        }
    }
    for (mut vis, mut t, mut mesh, mut tag) in &mut q {
        if vis.shown_brake != Some(vis.brake) {
            vis.shown_brake = Some(vis.brake);
            // A tag, not a second material: swapping materials would make
            // the renderer re-specialise the car.
            *tag = MeshTag::new(if vis.brake { TAG_BRAKE } else { 0 });
        }
        let p = vis.prev.0.lerp(vis.cur.0, alpha);
        let h = lerp_angle(vis.prev.1, vis.cur.1, alpha);
        let new = Transform::from_xyz(p.x, ROAD_Y, p.y).with_rotation(Quat::from_rotation_y(-h));
        if *t != new {
            *t = new;
        }
        let d = cam.distance(new.translation);
        let lod = level_at(d, vis.lod);
        if vis.mesh_model != Some((vis.model, lod)) {
            vis.lod = lod;
            vis.mesh_model = Some((vis.model, lod));
            mesh.0 = models.mesh(vis.model, lod);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citysim_sim::{Cell, Command};

    #[test]
    fn angle_lerp_takes_the_short_way() {
        let a = lerp_angle(3.0, -3.0, 0.5);
        assert!((a.abs() - std::f32::consts::PI).abs() < 0.2, "{a}");
    }

    #[test]
    fn box_overlap_basics() {
        let a = (Vec2::ZERO, 0.0, 2.0, 0.9);
        assert!(boxes_overlap(a, (Vec2::new(3.9, 0.0), 0.0, 2.0, 0.9), 0.0));
        assert!(!boxes_overlap(a, (Vec2::new(4.1, 0.0), 0.0, 2.0, 0.9), 0.0));
        assert!(!boxes_overlap(a, (Vec2::new(0.0, 2.0), 0.0, 2.0, 0.9), 0.0));
        assert!(boxes_overlap(a, (Vec2::new(2.5, 1.5), 1.0, 2.0, 0.9), 0.0));
    }

    fn town() -> World {
        let mut w = World::new(1);
        w.economy.money = 1_000_000;
        for (a, b, kind) in [
            ((11, 128), (40, 128), RoadKind::Avenue),
            ((30, 110), (30, 150), RoadKind::Street),
            ((40, 128), (55, 113), RoadKind::Street),
        ] {
            w.apply(Command::BuildRoad {
                from: Cell::new(a.0, a.1),
                to: Cell::new(b.0, b.1),
                kind,
            })
            .expect("road");
        }
        w
    }

    #[test]
    fn lanes_are_offset_to_the_right_of_travel() {
        let w = town();
        let mut v = Vehicle::default();
        let (s, _) = w.roads.segments.iter().next().expect("segment");
        v.segment = Some(s);
        v.forward = true;
        v.progress = 10.0;
        let (pa, pb) = w.roads.segment_ends(s);
        let dir = (Vec2::from(pb) - Vec2::from(pa)).normalize();
        let (p, h) = place(&w, &v, 2.0);
        let on_line = Vec2::from(pa) + dir * 8.0;
        let side = p - on_line;
        assert!(side.dot(Vec2::new(-dir.y, dir.x)) > 1.0, "{side}");
        assert!((Vec2::from_angle(h) - dir).length() < 1e-4);
    }

    /// Vehicles of a busy scripted town, drawn exactly as `sync_vehicles`
    /// does: (overlapping pairs, samples).
    fn drawn_overlaps(world: &mut World, warmup: u32, ticks: u32) -> (usize, usize, Vec<String>) {
        let mut examples = Vec::new();
        let (mut events, mut samples) = (0, 0);
        for tick in 0..warmup + ticks {
            world.step();
            if tick < warmup {
                continue;
            }
            let boxes: Vec<_> = world
                .vehicles
                .iter()
                .map(|(id, v)| {
                    let (l, w) = model_for(world, id, v).size();
                    let (p, h) = place(world, v, l * 0.5);
                    (id, (p, h, l * 0.5, w * 0.5))
                })
                .collect();
            samples += boxes.len();
            for i in 0..boxes.len() {
                for j in i + 1..boxes.len() {
                    let (a, b) = (boxes[i].1, boxes[j].1);
                    if a.0.distance(b.0) < 6.0 && boxes_overlap(a, b, 0.15) {
                        events += 1;
                        if examples.len() < 10 {
                            let d = |id: VehicleId| {
                                let v = &world.vehicles[id];
                                format!(
                                    "seg {:?} fwd {} p {:.1} lane {} jm {:?}",
                                    v.segment,
                                    v.forward,
                                    v.progress,
                                    v.lane,
                                    v.junction.as_ref().map(|j| (j.movement, j.distance))
                                )
                            };
                            examples.push(format!("{} vs {}", d(boxes[i].0), d(boxes[j].0)));
                        }
                    }
                }
            }
        }
        (events, samples, examples)
    }

    #[test]
    fn drawn_vehicles_do_not_overlap_in_a_busy_town() {
        let mut w = World::new(1);
        let mut player = citysim_sim::scenarios::GridPlayer::new(&[
            citysim_sim::Zone::Residential,
            citysim_sim::Zone::Commercial,
            citysim_sim::Zone::Industrial,
        ]);
        while w.stats.population < 1_200 && w.time.day < 40 {
            player.act(&mut w);
            w.step();
        }
        let (events, samples, ex) = drawn_overlaps(&mut w, 0, 2_000);
        println!("{events} overlaps in {samples} samples");
        for e in &ex {
            println!("{e}");
        }
        assert!(samples > 10_000, "too few vehicles: {samples}");
        // Pairs touching inside busy boxes (movements the sim lets run
        // together whose bodies brush) are tolerated up to 1 %.
        assert!((events as f64) < samples as f64 * 0.01, "{events} / {samples}");
    }
}
