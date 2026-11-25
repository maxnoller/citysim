//! Buildings: one entity per `world.buildings` entry, synced when
//! `versions.buildings` changes. Models come from `building_gen`, cached by
//! `(kind, lot size, level, variant)` and shared, all with one material, so
//! thousands of buildings batch into few draws. New buildings grow in with
//! an ease-out; removed ones shrink out before despawning.

use std::collections::HashMap;

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use citysim_sim::{Building, BuildingId, CELL_SIZE, Facing, RoadNetwork};

use super::building_gen::{self, ModelKey, VARIANTS};
use super::road_geom;
use super::scene::WorldMaterials;
use crate::shared::{GameSet, SimWorld};

const GROW_SECS: f32 = 0.75;
const SHRINK_SECS: f32 = 0.35;
/// Buildings younger than this (sim ticks) animate in; older ones (e.g. from
/// a loaded save) just appear.
const ANIMATE_IF_YOUNGER: u64 = 3 * citysim_sim::TICKS_PER_SECOND as u64;

/// Camera distance (m) where buildings cross-fade to their simplified model.
pub const LOD_DISTANCE: std::ops::Range<f32> = 650.0..700.0;

/// Cache of generated building models: `(detailed, distant)` per key.
#[derive(Resource, Default)]
pub struct BuildingModels {
    meshes: HashMap<ModelKey, (Handle<Mesh>, Handle<Mesh>)>,
}

impl BuildingModels {
    /// Number of distinct building models (each has two levels of detail).
    pub fn count(&self) -> usize {
        self.meshes.len()
    }

    fn get(&mut self, key: ModelKey, meshes: &mut Assets<Mesh>) -> (Handle<Mesh>, Handle<Mesh>) {
        self.meshes
            .entry(key)
            .or_insert_with(|| {
                (
                    meshes.add(building_gen::generate(key, false).build()),
                    meshes.add(building_gen::generate(key, true).build()),
                )
            })
            .clone()
    }
}

#[derive(Component)]
pub(super) struct BuildingVisual;

#[derive(Component)]
struct Growing {
    start: f32,
}

#[derive(Component)]
pub(super) struct Shrinking {
    start: f32,
}

#[derive(Default)]
struct Spawned {
    key: Option<((u64, u64), (u64, u64))>,
    entities: HashMap<BuildingId, (Entity, ModelKey, Facing, Inset)>,
}

/// How a building model is shrunk away from a diagonal road: uniform
/// scale about the lot corner farthest from the road, and the resulting
/// shift of the lot centre (world xz).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inset {
    pub scale: f32,
    pub shift: Vec2,
    /// Extra squeeze along world x / z (lots stepping back from a wide
    /// road beside them), 1 = none.
    pub squeeze: Vec2,
}

impl Inset {
    const NONE: Inset = Inset {
        scale: 1.0,
        shift: Vec2::ZERO,
        squeeze: Vec2::ONE,
    };

    /// Scale for the model child, in the model's local frame (its x runs
    /// along world z when the building faces +-x).
    fn local_scale(&self, f: Facing) -> Vec3 {
        let (sx, sz) = match f {
            Facing::PlusY | Facing::MinusY => (self.squeeze.x, self.squeeze.y),
            Facing::PlusX | Facing::MinusX => (self.squeeze.y, self.squeeze.x),
        };
        Vec3::new(sx, 1.0, sz) * self.scale
    }
}

/// Axis-aligned roads drawn wider than their cell line (avenues), as
/// (a, b, half-width incl. sidewalks).
pub fn wide_roads(roads: &RoadNetwork) -> Vec<(Vec2, Vec2, f32)> {
    roads
        .segments
        .iter()
        .filter_map(|(_, seg)| {
            let a = Vec2::from(roads.nodes.get(seg.a)?.pos);
            let b = Vec2::from(roads.nodes.get(seg.b)?.pos);
            let d = (b - a).normalize_or_zero();
            let outer = road_geom::style(seg.kind).outer();
            (outer > CELL_SIZE * 0.5 + 0.01 && (d.x.abs() < 0.02 || d.y.abs() < 0.02)).then_some((a, b, outer))
        })
        .collect()
}

/// Step a lot (`min..max`) back from wide roads running beside it: the
/// lot shrinks on that side (never below `MIN_INSET_SCALE` of its size).
pub fn wide_inset(min: Vec2, max: Vec2, roads: &[(Vec2, Vec2, f32)]) -> Inset {
    let (mut lo, mut hi) = (min, max);
    for &(a, b, outer) in roads {
        let need = outer + ROAD_CLEARANCE;
        if (a.y - b.y).abs() < 0.01 {
            // Along x at z = a.y: must overlap the lot's x extent.
            if a.x.max(b.x) <= min.x + 0.5 || a.x.min(b.x) >= max.x - 0.5 {
                continue;
            }
            let z = a.y;
            if z <= min.y {
                lo.y = lo.y.max(z + need);
            } else if z >= max.y {
                hi.y = hi.y.min(z - need);
            }
        } else {
            if a.y.max(b.y) <= min.y + 0.5 || a.y.min(b.y) >= max.y - 0.5 {
                continue;
            }
            let x = a.x;
            if x <= min.x {
                lo.x = lo.x.max(x + need);
            } else if x >= max.x {
                hi.x = hi.x.min(x - need);
            }
        }
    }
    if lo == min && hi == max {
        return Inset::NONE;
    }
    let size = max - min;
    let floor = size * MIN_INSET_SCALE;
    // Keep at least the floor, anchored at the side away from the road.
    if hi.x - lo.x < floor.x {
        if lo.x > min.x {
            lo.x = hi.x - floor.x;
        } else {
            hi.x = lo.x + floor.x;
        }
    }
    if hi.y - lo.y < floor.y {
        if lo.y > min.y {
            lo.y = hi.y - floor.y;
        } else {
            hi.y = lo.y + floor.y;
        }
    }
    Inset {
        scale: 1.0,
        shift: (lo + hi) * 0.5 - (min + max) * 0.5,
        squeeze: (hi - lo) / size,
    }
}

/// Clearance kept between a building lot and a diagonal road's edge, m.
const ROAD_CLEARANCE: f32 = 0.3;
/// A building is never shrunk below this.
const MIN_INSET_SCALE: f32 = 0.45;

/// Diagonal road segments as (a, b, half-width incl. sidewalks). Roads
/// along the grid axes fill their cells exactly and need no inset.
pub fn diagonal_roads(roads: &RoadNetwork) -> Vec<(Vec2, Vec2, f32)> {
    roads
        .segments
        .iter()
        .filter_map(|(_, seg)| {
            let a = Vec2::from(roads.nodes.get(seg.a)?.pos);
            let b = Vec2::from(roads.nodes.get(seg.b)?.pos);
            let d = (b - a).normalize_or_zero();
            (d.x.abs() > 0.02 && d.y.abs() > 0.02).then_some((a, b, road_geom::style(seg.kind).outer()))
        })
        .collect()
}

/// Distance from point `p` to segment `a..b`.
fn point_seg(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Distance between segment `a..b` and the rectangle `min..max` (0 when
/// they intersect).
fn seg_rect(a: Vec2, b: Vec2, min: Vec2, max: Vec2) -> f32 {
    // Liang-Barsky clip: does the segment enter the rectangle?
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let mut inside = true;
    for (p, q) in [
        (-d.x, a.x - min.x),
        (d.x, max.x - a.x),
        (-d.y, a.y - min.y),
        (d.y, max.y - a.y),
    ] {
        if p.abs() < 1e-9 {
            if q < 0.0 {
                inside = false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    if inside && t0 <= t1 {
        return 0.0;
    }
    let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
    let to_rect = |p: Vec2| p.clamp(min, max).distance(p);
    corners
        .iter()
        .map(|&c| point_seg(c, a, b))
        .chain([to_rect(a), to_rect(b)])
        .fold(f32::MAX, f32::min)
}

/// Shrink a lot (`min..max`, world xz) away from the diagonal roads that
/// cut into it (on a 45 degree road the road surface covers the corners of
/// the zoned cells beside it).
pub fn diagonal_inset(min: Vec2, max: Vec2, roads: &[(Vec2, Vec2, f32)]) -> Inset {
    let mut best = Inset::NONE;
    let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
    let centre = (min + max) * 0.5;
    for &(a, b, w) in roads {
        let need = w + ROAD_CLEARANCE;
        // Cheap reject on bounding boxes.
        if a.x.min(b.x) - need > max.x
            || a.x.max(b.x) + need < min.x
            || a.y.min(b.y) - need > max.y
            || a.y.max(b.y) + need < min.y
            || seg_rect(a, b, min, max) >= need
        {
            continue;
        }
        // Scale about the corner farthest from the road.
        let anchor = *corners
            .iter()
            .max_by(|p, q| point_seg(**p, a, b).total_cmp(&point_seg(**q, a, b)))
            .expect("corners");
        if point_seg(anchor, a, b) < need {
            continue;
        }
        let fits = |s: f32| {
            let (lo, hi) = (anchor + (min - anchor) * s, anchor + (max - anchor) * s);
            seg_rect(a, b, lo.min(hi), lo.max(hi)) >= need
        };
        let (mut lo, mut hi) = (MIN_INSET_SCALE, 1.0f32);
        if !fits(lo) {
            continue;
        }
        for _ in 0..14 {
            let mid = (lo + hi) * 0.5;
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if lo < best.scale {
            best = Inset {
                scale: lo,
                shift: (anchor + (centre - anchor) * lo) - centre,
                squeeze: Vec2::ONE,
            };
        }
    }
    best
}

pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingModels>().add_systems(
            Update,
            (sync_buildings, animate_buildings).chain().in_set(GameSet::Render),
        );
    }
}

/// Model key and placement for a building.
pub fn model_key(b: &Building) -> ModelKey {
    let (w, d) = b.size();
    let (w, d) = match b.facing {
        Facing::PlusY | Facing::MinusY => (w, d),
        Facing::PlusX | Facing::MinusX => (d, w),
    };
    ModelKey {
        kind: b.kind,
        cells: (w.clamp(1, 255) as u8, d.clamp(1, 255) as u8),
        level: b.level.max(1),
        variant: (b.seed % VARIANTS) as u8,
    }
}

/// Rotation turning the model's front (+z) towards the building's facing.
pub fn facing_rotation(f: Facing) -> Quat {
    use std::f32::consts::{FRAC_PI_2, PI};
    Quat::from_rotation_y(match f {
        Facing::PlusY => 0.0,
        Facing::PlusX => FRAC_PI_2,
        Facing::MinusY => PI,
        Facing::MinusX => -FRAC_PI_2,
    })
}

fn footprint_center(b: &Building) -> Vec3 {
    let (w, d) = b.size();
    let min = b.footprint.min;
    Vec3::new(
        (min.x as f32 + w as f32 * 0.5) * CELL_SIZE,
        0.0,
        (min.y as f32 + d as f32 * 0.5) * CELL_SIZE,
    )
}

#[allow(clippy::too_many_arguments)]
fn sync_buildings(
    mut commands: Commands,
    sim: Res<SimWorld>,
    time: Res<Time<Real>>,
    mats: Res<WorldMaterials>,
    mut models: ResMut<BuildingModels>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut spawned: Local<Spawned>,
) {
    let key = (sim.key(sim.world.versions.buildings), sim.key(sim.world.versions.roads));
    if spawned.key == Some(key) {
        return;
    }
    let reloaded = spawned.key.is_some_and(|((g, _), _)| g != sim.generation);
    spawned.key = Some(key);
    let now = time.elapsed_secs();
    let world = &sim.world;
    let buildings = &world.buildings;
    let diagonals = diagonal_roads(&world.roads);
    let wides = wide_roads(&world.roads);
    let inset_of = |b: &Building| {
        if diagonals.is_empty() && wides.is_empty() {
            return Inset::NONE;
        }
        let (w, d) = b.size();
        let min = Vec2::new(b.footprint.min.x as f32, b.footprint.min.y as f32) * CELL_SIZE;
        let max = min + Vec2::new(w as f32, d as f32) * CELL_SIZE;
        let wide = wide_inset(min, max, &wides);
        // Diagonal clearance is measured on the stepped-back lot.
        let c = (min + max) * 0.5 + wide.shift;
        let half = (max - min) * 0.5 * wide.squeeze;
        let diag = diagonal_inset(c - half, c + half, &diagonals);
        Inset {
            scale: diag.scale,
            shift: wide.shift + diag.shift,
            squeeze: wide.squeeze,
        }
    };
    spawned.entities.retain(|id, (e, k, f, i)| {
        let keep = buildings
            .get(*id)
            .is_some_and(|b| model_key(b) == *k && b.facing == *f && inset_of(b) == *i && !reloaded);
        if !keep {
            commands.entity(*e).remove::<Growing>().insert(Shrinking { start: now });
        }
        keep
    });
    for (id, b) in buildings.iter() {
        if spawned.entities.contains_key(&id) {
            continue;
        }
        let mk = model_key(b);
        let young = world.time.tick.saturating_sub(b.created_tick) < ANIMATE_IF_YOUNGER && !reloaded;
        let rot = facing_rotation(b.facing);
        let tf = Transform::from_translation(footprint_center(b)).with_rotation(rot);
        let inset = inset_of(b);
        // The inset lives on the model children, so the grow animation can
        // scale the parent freely.
        let model_tf = Transform::from_translation(rot.inverse() * Vec3::new(inset.shift.x, 0.0, inset.shift.y))
            .with_scale(inset.local_scale(b.facing));
        let (detail, far) = models.get(mk, &mut meshes);
        let mut ec = commands.spawn((BuildingVisual, Visibility::default()));
        ec.with_children(|p| {
            p.spawn((
                model_tf,
                Mesh3d(detail),
                MeshMaterial3d(mats.lit.clone()),
                VisibilityRange {
                    start_margin: 0.0..0.0,
                    end_margin: LOD_DISTANCE,
                    use_aabb: false,
                },
            ));
            p.spawn((
                model_tf,
                Mesh3d(far),
                MeshMaterial3d(mats.lit.clone()),
                VisibilityRange {
                    start_margin: LOD_DISTANCE,
                    end_margin: 1e6..1e6,
                    use_aabb: false,
                },
            ));
        });
        if young {
            ec.insert((tf.with_scale(Vec3::new(0.9, 0.01, 0.9)), Growing { start: now }));
        } else {
            ec.insert(tf);
        }
        spawned.entities.insert(id, (ec.id(), mk, b.facing, inset));
    }
}

/// Ease-out with a slight overshoot.
fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.4;
    let c3 = c1 + 1.0;
    let u = t - 1.0;
    1.0 + c3 * u * u * u + c1 * u * u
}

fn animate_buildings(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut growing: Query<(Entity, &Growing, &mut Transform), Without<Shrinking>>,
    mut shrinking: Query<(Entity, &Shrinking, &mut Transform), Without<Growing>>,
) {
    let now = time.elapsed_secs();
    for (e, g, mut t) in &mut growing {
        let k = ((now - g.start) / GROW_SECS).clamp(0.0, 1.0);
        if k >= 1.0 {
            t.scale = Vec3::ONE;
            commands.entity(e).remove::<Growing>();
        } else {
            let y = ease_out_back(k).max(0.01);
            let xz = 0.9 + 0.1 * (k * 2.0).min(1.0);
            t.scale = Vec3::new(xz, y, xz);
        }
    }
    for (e, s, mut t) in &mut shrinking {
        let k = ((now - s.start) / SHRINK_SECS).clamp(0.0, 1.0);
        if k >= 1.0 {
            commands.entity(e).despawn();
        } else {
            let y = (1.0 - k * k).max(0.01);
            t.scale = Vec3::new(1.0 - 0.1 * k, y, 1.0 - 0.1 * k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_rotation_turns_front_towards_facing() {
        for f in Facing::ALL {
            let front = facing_rotation(f) * Vec3::Z;
            let (dx, dy) = f.offset();
            assert!(front.distance(Vec3::new(dx as f32, 0.0, dy as f32)) < 1e-5, "{f:?}");
        }
    }

    #[test]
    fn grow_easing_ends_at_one() {
        assert!(ease_out_back(0.0).abs() < 1e-6);
        assert!((ease_out_back(1.0) - 1.0).abs() < 1e-6);
    }

    /// A town with a 45 degree street and housing zoned on both sides.
    fn diagonal_town() -> citysim_sim::World {
        use citysim_sim::{Cell, Command, World, Zone};
        let mut w = World::new(3);
        w.economy.money = 1_000_000;
        for (a, b) in [((11, 128), (40, 128)), ((40, 128), (60, 108)), ((25, 120), (25, 136))] {
            w.apply(Command::BuildRoad {
                from: Cell::new(a.0, a.1),
                to: Cell::new(b.0, b.1),
            })
            .expect("road");
        }
        w.apply(Command::SetZone {
            min: Cell::new(38, 104),
            max: Cell::new(64, 130),
            zone: Some(Zone::Residential),
        })
        .expect("zone");
        w.apply(Command::SetZone {
            min: Cell::new(12, 120),
            max: Cell::new(37, 127),
            zone: Some(Zone::Commercial),
        })
        .expect("zone");
        for _ in 0..citysim_sim::balance::TICKS_PER_DAY * 2 {
            w.step();
        }
        w
    }

    #[test]
    fn buildings_beside_a_diagonal_road_are_inset_clear_of_it() {
        let w = diagonal_town();
        let diagonals = diagonal_roads(&w.roads);
        assert!(!diagonals.is_empty());
        let mut inset_count = 0;
        let mut beside = 0;
        for (_, b) in w.buildings.iter() {
            let (sw, sd) = b.size();
            let min = Vec2::new(b.footprint.min.x as f32, b.footprint.min.y as f32) * CELL_SIZE;
            let max = min + Vec2::new(sw as f32, sd as f32) * CELL_SIZE;
            let raw = diagonals
                .iter()
                .map(|&(a, c, _)| seg_rect(a, c, min, max))
                .fold(f32::MAX, f32::min);
            if raw < 4.0 {
                beside += 1;
            }
            let i = diagonal_inset(min, max, &diagonals);
            if i.scale < 1.0 {
                inset_count += 1;
            }
            // The drawn lot after the inset.
            let c = (min + max) * 0.5 + i.shift;
            let half = (max - min) * 0.5 * i.scale;
            for &(a, c2, hw) in &diagonals {
                let d = seg_rect(a, c2, c - half, c + half);
                assert!(
                    d >= hw + ROAD_CLEARANCE - 0.05,
                    "building at {min} clips a diagonal road: {d}"
                );
            }
        }
        println!(
            "{} buildings, {beside} beside the diagonal, {inset_count} inset",
            w.buildings.len()
        );
        assert!(
            beside > 0 && inset_count > 0,
            "the test town grew no buildings beside the diagonal"
        );
    }

    #[test]
    fn inset_keeps_the_far_corner_and_shrinks_toward_it() {
        // A cell whose corner touches a 45 degree centre line.
        let road = [(Vec2::new(0.0, 16.0), Vec2::new(16.0, 0.0), 4.0)];
        let (min, max) = (Vec2::new(8.0, 8.0), Vec2::new(16.0, 16.0));
        let i = diagonal_inset(min, max, &road);
        assert!(i.scale < 0.8 && i.scale > MIN_INSET_SCALE, "{i:?}");
        // Anchored at the far corner (16, 16): the centre moves towards it.
        assert!(i.shift.x > 0.0 && i.shift.y > 0.0);
        // A lot far away is untouched.
        assert_eq!(diagonal_inset(min + 40.0, max + 40.0, &road), Inset::NONE);
    }
}
