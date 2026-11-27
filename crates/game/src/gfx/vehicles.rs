//! Vehicles: one entity per `world.vehicles` entry with a shared low-poly
//! model (saloon, hatchback, estate, van, pickup in realistic paints, or a
//! delivery truck for goods trips).
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
//! Samples are taken after every fixed update that moved the sim (at fast
//! speeds a tick's steps are spread over several, so each is sampled) and
//! the body is drawn between the last two, at a drawn moment
//! ([`DrawClock`]) that runs one fixed update behind the newest sample with
//! real time: the body moves at the same pace whatever the frame times.
//! Paused, the drawn moment glides on to the newest sample and stops there
//! (no jump); resumed, it runs on from where it stopped.
//!
//! Where the highlighted, followed and routed vehicles are drawn goes to
//! `shared::DrawnPoses` each frame ([`DrawnSet`]) for the ring, the
//! diamond, the camera and the interface's marks.
//!
//! Brake lights come on when the sim says the car brakes or it stands.

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::{RoadKind, TripPurpose, Vehicle, VehicleId, World};

use super::city_material::TAG_BRAKE;
use super::noise::hash;
use super::palette;
use super::road_geom::{ROAD_Y, lane_offset};
use super::scene::WorldMaterials;
use super::vehicle_gen::{self, SHAPES};
use crate::shared::{
    CameraFollow, DrawnPoses, DrawnVehicle, FollowOn, GameSet, RouteView, SimStep, SimWorld, WorldCamera,
    WorldHighlight, WorldTarget,
};
use bevy::mesh::MeshTag;

/// If a vehicle moves further than this in one tick it teleported.
const TELEPORT_M: f32 = 30.0;

/// Camera distance beyond which vehicles use the simplified model (with a
/// little hysteresis so they do not flicker between models).
const LOD_FAR: f32 = 480.0;
const LOD_NEAR: f32 = 440.0;

/// Number of car models (shape x colour); truck models follow them.
const CAR_MODELS: usize = SHAPES as usize * palette::CAR_BODIES.len();

#[derive(Resource)]
pub struct VehicleModels {
    /// Cars (`[shape * colours + colour]`), then trucks (one per livery):
    /// (detailed, distant).
    meshes: Vec<(Handle<Mesh>, Handle<Mesh>)>,
}

impl VehicleModels {
    pub fn count(&self) -> usize {
        self.meshes.len()
    }

    fn mesh(&self, model: usize, lod: bool) -> Handle<Mesh> {
        let (near, far) = &self.meshes[model];
        if lod { far.clone() } else { near.clone() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Car,
    Truck,
}

/// A pose: body centre (sim metres) and heading (radians).
type Pose = (Vec2, f32);

#[derive(Component)]
pub(super) struct VehicleVisual {
    /// The body's pose at the last three samples ([`DrawClock`]), newest
    /// last.
    poses: [Pose; 3],
    seed: u32,
    kind: Kind,
    model: usize,
    /// The model the entity's mesh shows.
    mesh_model: Option<usize>,
    lod: bool,
    /// Whether the brake lights are on.
    brake: bool,
    /// Brake state the entity's material shows.
    shown_brake: Option<bool>,
    /// The pose drawn this frame.
    shown: Pose,
}

/// Brake lights: the sim's flag, or standing.
fn braking(v: &Vehicle) -> bool {
    v.braking || v.speed < 0.8
}

impl VehicleVisual {
    fn new(world: &World, v: &Vehicle, kind: Kind) -> Self {
        let model = model_index(v.seed, kind);
        let now = place(world, v, model_size(model).0 * 0.5);
        VehicleVisual {
            poses: [now; 3],
            seed: v.seed,
            kind,
            model,
            mesh_model: None,
            lod: false,
            brake: braking(v),
            shown_brake: None,
            shown: now,
        }
    }

    /// Take the sim's newest state of the vehicle (after a fixed update
    /// that moved the sim).
    fn sample(&mut self, world: &World, v: &Vehicle, kind: Kind) {
        if self.seed != v.seed {
            *self = VehicleVisual {
                lod: self.lod,
                mesh_model: self.mesh_model,
                shown_brake: self.shown_brake,
                ..VehicleVisual::new(world, v, kind)
            };
            return;
        }
        if kind != self.kind {
            self.kind = kind;
            self.model = model_index(v.seed, kind);
        }
        let now = place(world, v, model_size(self.model).0 * 0.5);
        if self.poses[2].0.distance(now.0) > TELEPORT_M {
            self.poses = [now; 3];
        } else {
            self.poses = [self.poses[1], self.poses[2], now];
        }
    }

    /// The pose at `k` samples after the oldest kept (0..=2).
    fn pose_at(&self, k: f32) -> Pose {
        let (a, b, t) = if k <= 1.0 {
            (self.poses[0], self.poses[1], k.max(0.0))
        } else {
            (self.poses[1], self.poses[2], (k - 1.0).min(1.0))
        };
        (a.0.lerp(b.0, t), lerp_angle(a.1, b.1, t))
    }
}

/// The moment cars are drawn at, in samples (fixed updates that moved the
/// sim): one behind the newest sample plus the fixed clock's overstep
/// while the sim runs. It only ever moves forward, at the pace of real time
/// (a little faster to catch up after a stall).
#[derive(Resource, Debug)]
pub(super) struct DrawClock {
    /// Samples taken (the newest is number `newest`).
    newest: u64,
    /// The sim tick at the last three samples, newest last.
    ticks: [u64; 3],
    /// The drawn moment, in samples.
    s: f64,
    /// The last fixed update moved the sim.
    moving: bool,
    /// The sim tick at the last fixed update, and the world's generation.
    last_tick: u64,
    generation: u64,
}

impl Default for DrawClock {
    fn default() -> Self {
        DrawClock {
            newest: 2,
            ticks: [0; 3],
            s: 2.0,
            moving: false,
            last_tick: 0,
            // Not a world's: the first frame starts over.
            generation: u64::MAX,
        }
    }
}

impl DrawClock {
    /// The drawn moment as samples after the oldest kept (0..=2).
    fn k(&self) -> f32 {
        (self.s - (self.newest as f64 - 2.0)).clamp(0.0, 2.0) as f32
    }

    /// The drawn moment as a sim tick (fractional).
    pub fn tick(&self) -> f64 {
        let k = self.k() as f64;
        let [a, b, c] = self.ticks.map(|t| t as f64);
        if k <= 1.0 {
            a + (b - a) * k
        } else {
            b + (c - b) * (k - 1.0)
        }
    }

    /// Advance the drawn moment by `dt` seconds of a fixed clock with
    /// period `h` s and overstep fraction `over`.
    fn advance(&mut self, dt: f64, h: f64, over: f64) {
        let newest = self.newest as f64;
        let target = if self.moving {
            newest - 1.0 + over.clamp(0.0, 1.0)
        } else {
            newest
        };
        let step = dt / h.max(1e-6);
        // Behind (after a stall or a pause): catch up a little faster.
        let behind = target - self.s > step + 1e-6;
        let s = self.s + if behind { step * 1.5 } else { step };
        self.s = s.min(target).max(self.s);
        if target - self.s > 2.0 {
            self.s = target;
        }
        self.s = self.s.max(newest - 2.0);
    }
}

/// Placing what moves this frame, in `GameSet::Render`: the cars (and
/// their [`DrawnPoses`]), then the off-map traveller's marker
/// (`route.rs`). Order after `Traveller` to read where things are drawn
/// this frame (the ring, the diamond, the camera).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum DrawnSet {
    Vehicles,
    Traveller,
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

/// The entity drawing each vehicle.
#[derive(Resource, Default)]
pub(super) struct VehicleEntities(pub HashMap<VehicleId, Entity>);

pub struct VehiclesPlugin;

impl Plugin for VehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VehicleEntities>()
            .init_resource::<DrawClock>()
            .add_systems(Startup, setup)
            .add_systems(FixedUpdate, sample_vehicles.after(SimStep))
            .configure_sets(
                Update,
                (DrawnSet::Vehicles, DrawnSet::Traveller)
                    .chain()
                    .in_set(GameSet::Render),
            )
            .add_systems(
                Update,
                (sync_vehicles, publish_drawn).chain().in_set(DrawnSet::Vehicles),
            );
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
    for &(cab, cargo, stripe) in &palette::TRUCK_LIVERIES {
        handles.push((
            meshes.add(vehicle_gen::truck_model(cab, cargo, stripe, false).build()),
            meshes.add(vehicle_gen::truck_model(cab, cargo, stripe, true).build()),
        ));
    }
    commands.insert_resource(VehicleModels { meshes: handles });
}

fn kind_of(world: &World, id: VehicleId) -> Kind {
    match world.traffic.trips.get(id).map(|t| t.purpose) {
        Some(TripPurpose::Goods) => Kind::Truck,
        _ => Kind::Car,
    }
}

fn model_index(seed: u32, kind: Kind) -> usize {
    let h = hash(seed ^ 0x51ED_270B);
    if kind == Kind::Truck {
        return CAR_MODELS + (h / 16) as usize % palette::TRUCK_LIVERIES.len();
    }
    // Mostly saloons, hatchbacks and estates, some vans and pickups.
    let shape = match h % 12 {
        0..=3 => 0,
        4..=6 => 1,
        7 | 8 => 4,
        9 => 2,
        _ => 3,
    };
    let colours = palette::CAR_BODIES.len();
    shape * colours + (h / 16) as usize % colours
}

/// Length and width of a vehicle model, metres.
fn model_size(model: usize) -> (f32, f32) {
    if model >= CAR_MODELS {
        (vehicle_gen::TRUCK_LEN, vehicle_gen::TRUCK_WIDTH)
    } else {
        vehicle_gen::car_size((model / palette::CAR_BODIES.len()) as u32)
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

/// After every fixed update: a new sample of every drawn vehicle when the
/// sim moved.
fn sample_vehicles(
    sim: Res<SimWorld>,
    entities: Res<VehicleEntities>,
    mut clock: ResMut<DrawClock>,
    mut q: Query<&mut VehicleVisual>,
) {
    let world = &sim.world;
    let tick = world.time.tick;
    if clock.generation != sim.generation {
        // A new world: `sync_vehicles` starts over.
        return;
    }
    clock.moving = tick != clock.last_tick;
    clock.last_tick = tick;
    if !clock.moving {
        return;
    }
    clock.newest += 1;
    clock.ticks = [clock.ticks[1], clock.ticks[2], tick];
    for (&id, &e) in &entities.0 {
        if let Ok(mut vis) = q.get_mut(e)
            && let Some(v) = world.vehicles.get(id)
        {
            vis.sample(world, v, kind_of(world, id));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_vehicles(
    mut commands: Commands,
    sim: Res<SimWorld>,
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    models: Res<VehicleModels>,
    mats: Res<WorldMaterials>,
    mut clock: ResMut<DrawClock>,
    mut entities: ResMut<VehicleEntities>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<(&mut VehicleVisual, &mut Transform, &mut Mesh3d, &mut MeshTag)>,
) {
    let cam = cams.single().map_or(Vec3::ZERO, |c| c.translation());
    let world = &sim.world;
    if clock.generation != sim.generation {
        for (_, e) in entities.0.drain() {
            commands.entity(e).despawn();
        }
        let tick = world.time.tick;
        *clock = DrawClock {
            newest: 2,
            ticks: [tick; 3],
            s: 2.0,
            moving: false,
            last_tick: tick,
            generation: sim.generation,
        };
    }
    let vehicles = &world.vehicles;
    entities.0.retain(|id, e| {
        let keep = vehicles.contains_key(*id);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    clock.advance(
        time.delta_secs_f64(),
        fixed.timestep().as_secs_f64(),
        fixed.overstep_fraction_f64(),
    );
    let k = clock.k();
    for (id, v) in vehicles.iter() {
        match entities.0.get(&id) {
            Some(&e) => {
                if let Ok((mut vis, _, _, _)) = q.get_mut(e) {
                    vis.brake = braking(v);
                }
            }
            None => {
                let mut vis = VehicleVisual::new(world, v, kind_of(world, id));
                let (p, h) = vis.poses[2];
                let pos = Vec3::new(p.x, ROAD_Y, p.y);
                vis.lod = cam.distance(pos) > LOD_FAR;
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
                entities.0.insert(id, e);
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
        let (p, h) = vis.pose_at(k);
        vis.shown = (p, h);
        let new = Transform::from_xyz(p.x, ROAD_Y, p.y).with_rotation(Quat::from_rotation_y(-h));
        if *t != new {
            *t = new;
        }
        let d = cam.distance(new.translation);
        let lod = if vis.lod { d > LOD_NEAR } else { d > LOD_FAR };
        if lod != vis.lod || vis.mesh_model != Some(vis.model) {
            vis.lod = lod;
            vis.mesh_model = Some(vis.model);
            mesh.0 = models.mesh(vis.model, lod);
        }
    }
}

/// Where the vehicles the interface points at are drawn this frame. New
/// vehicles spawned this frame are not drawn yet (their entity comes with
/// the next command flush): they are left out.
fn publish_drawn(
    hl: Res<WorldHighlight>,
    follow: Res<CameraFollow>,
    route: Res<RouteView>,
    entities: Res<VehicleEntities>,
    q: Query<&VehicleVisual>,
    mut drawn: ResMut<DrawnPoses>,
) {
    let mut ids: Vec<VehicleId> = Vec::with_capacity(4);
    for t in [&hl.selected, &hl.hovered] {
        if let Some(WorldTarget::Vehicle(v)) = t {
            ids.push(*v);
        }
    }
    if let FollowOn::Vehicle(v) = follow.on {
        ids.push(v);
    }
    ids.extend(route.vehicle);
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if out.iter().any(|(v, _)| *v == id) {
            continue;
        }
        let Some(vis) = entities.0.get(&id).and_then(|e| q.get(*e).ok()) else {
            continue;
        };
        out.push((
            id,
            DrawnVehicle {
                pos: vis.shown.0.to_array(),
                heading: vis.shown.1,
                half_length: model_size(vis.model).0 * 0.5,
            },
        ));
    }
    if drawn.vehicles != out {
        drawn.vehicles = out;
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
                    let model = model_index(v.seed, kind_of(world, id));
                    let (l, w) = model_size(model);
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
