//! Small greens on the land roads enclose but no lot can use (the middle
//! of a small ring, the middle of a wide block, the corner rows leave in a
//! triangle): the simulation finds it (`faces::leftovers`), this draws it
//! as a deliberate little park: a lawn with a smooth edge, a path loop
//! where there is room, trees, benches and in the bigger ones a fountain or
//! a pavilion in the middle. Wild trees keep out of it (`trees` reads
//! [`Greens`]).
//!
//! Recomputed per face only when its roads or blocks change (the
//! simulation gives each face a signature).

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::faces::{self, Leftover};

use super::mesh_util::MeshBuilder;
use super::noise::{self, Stream};
use super::palette::{self, lin, mix, shade};
use super::props;
use super::scene::WorldMaterials;
use crate::shared::{GameSet, SimWorld};

/// Ground heights (under lot markers and the zone overlay).
const Y_LAWN: f32 = 0.026;
const Y_PATH: f32 = 0.031;
const Y_INNER: f32 = 0.036;

/// The current greens, for the renderers that keep out of them.
#[derive(Resource, Default)]
pub struct Greens {
    /// Bumped whenever the set changes.
    pub version: u64,
    pub areas: Vec<Leftover>,
}

impl Greens {
    pub fn contains(&self, p: Vec2) -> bool {
        self.areas.iter().any(|a| a.contains(p.into()))
    }
}

/// Runs before the trees.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GreensUpdate;

#[derive(Default)]
struct GreenState {
    key: Option<(u64, u64)>,
    /// (face, signature) -> leftover and its entity.
    by_face: HashMap<((citysim_sim::SegmentId, bool), u64), (Option<Leftover>, Option<Entity>)>,
}

pub struct GreensPlugin;

impl Plugin for GreensPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Greens>()
            .configure_sets(Update, GreensUpdate.in_set(GameSet::Render))
            .add_systems(Update, update_greens.in_set(GreensUpdate));
    }
}

fn update_greens(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mats: Res<WorldMaterials>,
    mut state: Local<GreenState>,
    mut greens: ResMut<Greens>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let world = &sim.world;
    let key = sim.key(world.versions.roads);
    if state.key == Some(key) {
        return;
    }
    state.key = Some(key);
    let faces_now = faces::leftover_faces(world);
    let mut keep: HashMap<_, _> = HashMap::new();
    let mut changed = false;
    for (face, sig) in faces_now {
        let k = (face, sig);
        if let Some(v) = state.by_face.remove(&k) {
            keep.insert(k, v);
            continue;
        }
        changed = true;
        let left = faces::leftover(world, face, sig);
        let entity = left.as_ref().and_then(|l| {
            let (mesh, centre) = park_mesh(l)?;
            Some(
                commands
                    .spawn((
                        Name::new("Green"),
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(mats.lit.clone()),
                        Transform::from_translation(centre),
                        Visibility::default(),
                    ))
                    .id(),
            )
        });
        keep.insert(k, (left, entity));
    }
    // Faces that are gone.
    for (_, (left, e)) in state.by_face.drain() {
        changed |= left.is_some();
        if let Some(e) = e {
            commands.entity(e).despawn();
        }
    }
    state.by_face = keep;
    if changed {
        let mut areas: Vec<Leftover> = state.by_face.values().filter_map(|(l, _)| l.clone()).collect();
        areas.sort_by_key(|l| l.face);
        greens.areas = areas;
        greens.version += 1;
    }
}

/// Distance (metres) of every inside sample to the nearest outside one
/// (chamfer); outside samples are 0.
fn distance_field(l: &Leftover) -> Vec<f32> {
    let (w, h) = (l.size[0] as i32, l.size[1] as i32);
    let s = l.step;
    let mut d: Vec<f32> = l.inside.iter().map(|&i| if i { 1e6 } else { 0.0 }).collect();
    let at = |x: i32, y: i32| (y * w + x) as usize;
    let get = |d: &Vec<f32>, x: i32, y: i32| {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            d[at(x, y)]
        }
    };
    let diag = s * std::f32::consts::SQRT_2;
    for y in 0..h {
        for x in 0..w {
            let v = d[at(x, y)];
            if v == 0.0 {
                continue;
            }
            let m = v
                .min(get(&d, x - 1, y) + s)
                .min(get(&d, x, y - 1) + s)
                .min(get(&d, x - 1, y - 1) + diag)
                .min(get(&d, x + 1, y - 1) + diag);
            d[at(x, y)] = m;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let v = d[at(x, y)];
            if v == 0.0 {
                continue;
            }
            let m = v
                .min(get(&d, x + 1, y) + s)
                .min(get(&d, x, y + 1) + s)
                .min(get(&d, x + 1, y + 1) + diag)
                .min(get(&d, x - 1, y + 1) + diag);
            d[at(x, y)] = m;
        }
    }
    d
}

/// The region where the sampled field reaches `t`, as triangles (each grid
/// square clipped by the linearly interpolated field), at height `y`.
fn region(m: &mut MeshBuilder, l: &Leftover, f: &[f32], t: f32, y: f32, color: Color) {
    let (w, h) = (l.size[0] as i32, l.size[1] as i32);
    let s = l.step;
    let c = lin(color);
    let p = |x: i32, z: i32| Vec2::new(l.origin[0] + x as f32 * s, l.origin[1] + z as f32 * s);
    for z in 0..h - 1 {
        for x in 0..w - 1 {
            let corners = [(x, z), (x + 1, z), (x + 1, z + 1), (x, z + 1)];
            let v = corners.map(|(a, b)| f[(b * w + a) as usize]);
            if v.iter().all(|&k| k < t) {
                continue;
            }
            let mut poly: Vec<Vec2> = Vec::with_capacity(6);
            for k in 0..4 {
                let (a, b) = (k, (k + 1) % 4);
                let (pa, pb) = (p(corners[a].0, corners[a].1), p(corners[b].0, corners[b].1));
                if v[a] >= t {
                    poly.push(pa);
                }
                if (v[a] >= t) != (v[b] >= t) {
                    let u = (t - v[a]) / (v[b] - v[a]);
                    poly.push(pa + (pb - pa) * u);
                }
            }
            for k in 1..poly.len().saturating_sub(1) {
                m.tri_up(
                    Vec3::new(poly[0].x, y, poly[0].y),
                    Vec3::new(poly[k + 1].x, y, poly[k + 1].y),
                    Vec3::new(poly[k].x, y, poly[k].y),
                    c,
                );
            }
        }
    }
}

/// The park on one leftover, local to its centre.
fn park_mesh(l: &Leftover) -> Option<(Mesh, Vec3)> {
    let f = distance_field(l);
    let s = l.step;
    let w = l.size[0] as usize;
    let max = f.iter().copied().fold(0.0f32, f32::max);
    if max < s {
        return None;
    }
    let seed = noise::hash((l.signature ^ (l.signature >> 32)) as u32 ^ 0x9a7e);
    let mut rng = Stream::new(seed);
    let mut m = MeshBuilder::new();
    let lawn = mix(palette::GRASS_LIGHT, Color::srgb_u32(0x9CCB6A), 0.5);
    region(&mut m, l, &f, s * 0.5, Y_LAWN, lawn);
    // A path loop a little in from the edge where there is room, the inner
    // lawn mown a shade lighter.
    let path = max >= 7.0;
    let (p0, p1) = (2.6, 4.2);
    if path {
        region(&mut m, l, &f, p0, Y_PATH, palette::PATH);
        region(&mut m, l, &f, p1, Y_INNER, shade(lawn, 1.04));
    }
    let pos = |i: usize| {
        let (x, y) = ((i % w) as f32, (i / w) as f32);
        Vec2::new(l.origin[0] + x * s, l.origin[1] + y * s)
    };
    // The middle: a fountain or a pavilion.
    let (centre_i, _) = f
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |b, (i, &v)| if v > b.1 { (i, v) } else { b });
    let middle = pos(centre_i);
    let feature = max >= 9.0;
    if feature {
        let c = Vec3::new(middle.x, 0.0, middle.y);
        if rng.chance(0.5) {
            let stone = palette::PAVING;
            m.cylinder(c, 3.0, 0.5, 14, shade(stone, 0.9), stone);
            m.cylinder(
                c + Vec3::Y * 0.5,
                2.6,
                0.02,
                14,
                Color::srgb_u32(0x5FB6D6),
                Color::srgb_u32(0x6FC3E0),
            );
            m.cylinder(c, 0.35, 1.6, 6, stone, stone);
            m.cylinder(c + Vec3::Y * 1.6, 0.9, 0.12, 8, stone, Color::srgb_u32(0x6FC3E0));
        } else {
            let wood = Color::srgb_u32(0xEDE6D6);
            for k in 0..6 {
                let a = k as f32 / 6.0 * std::f32::consts::TAU;
                m.cylinder(
                    c + Vec3::new(a.cos() * 2.4, 0.0, a.sin() * 2.4),
                    0.12,
                    2.6,
                    5,
                    wood,
                    wood,
                );
            }
            m.cylinder(c, 2.9, 0.25, 6, shade(wood, 0.92), palette::PAVING);
            m.cone(c + Vec3::Y * 2.6, 3.3, 1.6, 6, Color::srgb_u32(0x6E7F8C), 0.0);
        }
    }
    // Trees on the lawn, spaced, clear of the path and the middle.
    let mut trees: Vec<Vec2> = Vec::new();
    for (i, &v) in f.iter().enumerate() {
        let ok_band = if path {
            v >= p1 + 1.8 || (1.6..=p0 - 0.8).contains(&v)
        } else {
            v >= 1.8
        };
        if !ok_band {
            continue;
        }
        let p = pos(i);
        let jitter = Vec2::new(rng.range(-0.8, 0.8), rng.range(-0.8, 0.8));
        let q = p + jitter;
        if feature && q.distance(middle) < 6.0 {
            continue;
        }
        if trees.iter().any(|t| t.distance(q) < 6.5) {
            continue;
        }
        if !rng.chance(0.35) {
            continue;
        }
        trees.push(q);
        let canopy = rng.pick(&palette::CANOPIES);
        let yaw = rng.range(0.0, 6.28);
        let sc = rng.range(0.6, 0.95);
        if rng.chance(0.8) {
            props::round_tree(&mut m, Vec3::new(q.x, 0.0, q.y), sc, canopy, yaw);
        } else {
            props::pine_tree(&mut m, Vec3::new(q.x, 0.0, q.y), sc, canopy, yaw);
        }
    }
    // Benches along the inside of the path, facing the middle.
    if path {
        let mut benches: Vec<Vec2> = Vec::new();
        for (i, &v) in f.iter().enumerate() {
            if !(p1 + 0.2..=p1 + 1.4).contains(&v) {
                continue;
            }
            let p = pos(i);
            if benches.iter().any(|b| b.distance(p) < 14.0) || trees.iter().any(|t| t.distance(p) < 2.5) {
                continue;
            }
            benches.push(p);
            let to = (middle - p).normalize_or(Vec2::X);
            let yaw = -to.y.atan2(to.x);
            let wood = Color::srgb_u32(0x7A5A3E);
            m.box_yaw(
                Vec3::new(p.x, 0.0, p.y),
                Vec3::new(0.5, 0.45, 1.7),
                yaw,
                wood,
                shade(wood, 1.1),
            );
        }
    }
    // Low bushes around the edge.
    for (i, &v) in f.iter().enumerate() {
        if (s * 0.9..=s * 1.1).contains(&v) && (i % w + i / w) % 4 == 0 && rng.chance(0.4) {
            let p = pos(i);
            props::bush(
                &mut m,
                Vec3::new(p.x, 0.0, p.y),
                rng.range(0.8, 1.2),
                palette::HEDGE,
                0.0,
            );
        }
    }
    if m.is_empty() {
        return None;
    }
    let centre = Vec3::new(
        l.origin[0] + l.size[0] as f32 * s * 0.5,
        0.0,
        l.origin[1] + l.size[1] as f32 * s * 0.5,
    );
    m.translate(-centre);
    Some((m.build(), centre))
}
