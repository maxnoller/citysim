//! Yards: the land of a lot behind its building (lots are up to six rows
//! deep, the building stands on the front two). Drawn so the land reads as
//! belonging to the building and a block looks built up from above:
//!
//! * homes: a back garden: a lawn of its own shade, a hedge or a fence on
//!   the lot lines, garden trees, and by the household's luck a shed, a
//!   patio, vegetable beds, a washing line, a pool; flats (level 3) share a
//!   garden with paths and benches;
//! * shops and offices: a car park with marked bays and parked cars,
//!   fuller at higher levels, kerbed and with a tree or two;
//! * industry: a concrete or gravel storage yard behind a wire fence, with
//!   containers, pallets, crates, tanks and at higher levels a lorry.
//!
//! The furnished part reaches only so far back: a garden one and a half
//! times the house's depth (12-16 m, at most two rows), a car park or storage
//! yard at most three rows; the rest of a deep lot is open common green
//! (one shade for every lot, so neighbours' greens join into a block's
//! green middle; grass and a few trees, no fences or hedges across it).
//!
//! Varied by the building's seed and level. Merged into one mesh per 256 m
//! chunk (all yards share the city material), with a near level of detail
//! (everything) and a far one (ground, boundaries and trees); a chunk is
//! re-meshed only when its yards change. Lot lines two yards share are
//! drawn once.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use citysim_sim::{Building, BuildingId, World, Zone, balance};

use super::land::RoadEdges;
use super::lot_props;
use super::mesh_util::MeshBuilder;
use super::noise::Stream;
use super::palette::{self, lin, mix, shade};
use super::props;
use super::road_geom::chunk_of;
use super::scene::WorldMaterials;
use super::vehicle_gen;
use crate::shared::{GameSet, SimWorld};

/// Chunk size, metres.
const CHUNK_M: f32 = 256.0;
/// Camera distance ranges for the near and far chunk meshes (to the chunk
/// centre, so generous).
const NEAR: (std::ops::Range<f32>, std::ops::Range<f32>) = (0.0..0.0, 560.0..610.0);
const FAR: (std::ops::Range<f32>, std::ops::Range<f32>) = (560.0..610.0, 1e6..1e6);
/// Height of the yard ground (under lot markers and the zone overlay).
const GROUND_Y: f32 = 0.028;
/// Clearance kept from the drawn roads, metres.
const ROAD_GAP: f32 = 0.4;
/// Furnished yard behind the building: a home's garden one and a half times
/// the house's depth (a house about 8 m deep at level 1, 10 m at level 2,
/// flats more), at most `GARDEN_MAX_M` (two rows); a car park or storage
/// yard at most `WORK_YARD_ROWS`.
const HOUSE_DEPTH_M: [f32; 3] = [8.0, 10.0, 12.0];
const GARDEN_MAX_M: f32 = 16.0;
const WORK_YARD_ROWS: f32 = 3.0;
/// Trees on the common green, per square metre.
const GREEN_TREES_PER_M2: f32 = 1.0 / 260.0;

/// One yard to draw.
#[derive(Debug, Clone)]
struct Yard {
    id: BuildingId,
    kind: Zone,
    level: u8,
    seed: u32,
    /// Front-left, front-right, back-right, back-left (front: towards the
    /// building), world `(x, z)`.
    quad: [Vec2; 4],
    /// Unit direction towards the building (and the road), world `(x, z)`.
    front: Vec2,
    /// The lot's land behind the furnished yard, drawn as open common
    /// green (same corner order).
    rest: Option<[Vec2; 4]>,
}

impl Yard {
    fn centre(&self) -> Vec2 {
        (self.quad[0] + self.quad[1] + self.quad[2] + self.quad[3]) * 0.25
    }

    /// Model-local frame: `+z` towards the building, `+x` = (front.y,
    /// -front.x) (as building models).
    fn local(&self, p: Vec2) -> Vec2 {
        let d = p - self.centre();
        Vec2::new(d.x * self.front.y - d.y * self.front.x, d.dot(self.front))
    }

    fn yaw(&self) -> f32 {
        std::f32::consts::FRAC_PI_2 - self.front.y.atan2(self.front.x)
    }

    /// The largest local rectangle inside the quad: `(x0, z0, x1, z1)`.
    fn inner(&self) -> (f32, f32, f32, f32) {
        let l = self.quad.map(|p| self.local(p));
        let (a, b, c, d) = (l[0], l[1], l[2], l[3]);
        let (x0, x1) = if a.x <= b.x {
            (a.x.max(d.x), b.x.min(c.x))
        } else {
            (b.x.max(c.x), a.x.min(d.x))
        };
        (x0, c.y.max(d.y), x1, a.y.min(b.y))
    }
}

/// The yard of a building's lot, if it has one.
fn yard_of(world: &World, id: BuildingId, b: &Building) -> Option<Yard> {
    let fp = &b.footprint;
    if fp.yard_depth() == 0 {
        return None;
    }
    let q = world.zoning.lot_quad(fp)?.map(Vec2::from);
    let rows = fp.size[1].max(1) as f32;
    let built = balance::BUILT_DEPTH as f32;
    let row_m = (q[3].distance(q[0]) + q[2].distance(q[1])) * 0.5 / rows;
    let cap = match b.kind {
        Zone::Residential => {
            let house = HOUSE_DEPTH_M[(b.level.clamp(1, 3) - 1) as usize];
            (1.5 * house).min(GARDEN_MAX_M) / row_m.max(1.0)
        }
        Zone::Commercial | Zone::Industrial => WORK_YARD_ROWS,
    };
    let t = built / rows;
    let tb = ((built + cap) / rows).min(1.0);
    let along = |s: f32| (q[0] + (q[3] - q[0]) * s, q[1] + (q[2] - q[1]) * s);
    let (fl, fr) = along(t);
    let (bl, br) = along(tb);
    Some(Yard {
        id,
        kind: b.kind,
        level: b.level,
        seed: b.seed,
        quad: [fl, fr, br, bl],
        front: Vec2::from(b.front_dir()).normalize_or(Vec2::Y),
        rest: (tb < 0.999).then_some([bl, br, q[2], q[3]]),
    })
}

/// Pull the quad's corners towards its centre until each is clear of the
/// roads (a corner lot's yard beside a side street).
fn clear_of_roads(quad: [Vec2; 4], edges: &RoadEdges) -> Option<[Vec2; 4]> {
    let c = (quad[0] + quad[1] + quad[2] + quad[3]) * 0.25;
    let mut out = quad;
    for p in &mut out {
        let mut k = 1.0f32;
        while edges.clearance(c + (*p - c) * k) < ROAD_GAP {
            k -= 0.1;
            if k < 0.35 {
                return None;
            }
        }
        *p = c + (*p - c) * k;
    }
    Some(out)
}

/// Car models by body colour and shape (built once).
#[derive(Default)]
struct Cars(HashMap<(usize, u32), MeshBuilder>);

impl Cars {
    fn get(&mut self, body: usize, shape: u32) -> &MeshBuilder {
        self.0
            .entry((body, shape))
            .or_insert_with(|| vehicle_gen::car_model(palette::CAR_BODIES[body], shape, super::building_gen::Lod::Mid))
    }
}

#[derive(Default)]
struct YardChunks {
    key: Option<[(u64, u64); 3]>,
    chunks: HashMap<IVec2, (u64, [Entity; 2])>,
    cars: Cars,
    lorry: Option<MeshBuilder>,
}

pub struct YardsPlugin;

impl Plugin for YardsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            rebuild_yards
                .in_set(GameSet::Render)
                .after(super::land::LandUpdate)
                .after(super::buildings::BuildingsSync),
        );
    }
}

fn yard_hash(yards: &[Yard]) -> u64 {
    let mut h = DefaultHasher::new();
    for y in yards {
        (y.id, y.kind, y.level, y.seed).hash(&mut h);
        for p in y.quad.iter().chain(y.rest.iter().flatten()) {
            p.x.to_bits().hash(&mut h);
            p.y.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

fn rebuild_yards(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mats: Res<WorldMaterials>,
    mut state: Local<YardChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    edges: Res<RoadEdges>,
) {
    let world = &sim.world;
    let key = [
        sim.key(world.versions.buildings),
        sim.key(world.versions.grid),
        sim.key(world.versions.roads),
    ];
    if state.key == Some(key) || edges.key != Some(sim.key(world.versions.roads)) {
        return;
    }
    state.key = Some(key);
    let mut by_chunk: HashMap<IVec2, Vec<Yard>> = HashMap::new();
    for (id, b) in world.buildings.iter() {
        if let Some(y) = yard_of(world, id, b) {
            by_chunk.entry(chunk_of(y.centre(), CHUNK_M)).or_default().push(y);
        }
    }
    let mut keys: Vec<IVec2> = by_chunk.keys().copied().collect();
    keys.extend(state.chunks.keys().copied());
    keys.sort_by_key(|c| (c.x, c.y));
    keys.dedup();
    let st = &mut *state;
    for c in keys {
        let yards = by_chunk.get(&c).map(Vec::as_slice).unwrap_or(&[]);
        let hash = yard_hash(yards);
        if st.chunks.get(&c).is_some_and(|(h, _)| *h == hash) {
            continue;
        }
        let mut near = MeshBuilder::new();
        let mut far = MeshBuilder { lod: true, ..default() };
        let mut lines: HashSet<[i32; 4]> = HashSet::new();
        for y in yards {
            draw_yard(&mut near, &mut far, &mut lines, y, &edges, &mut st.cars, &mut st.lorry);
        }
        // Meshes local to the chunk centre (the LOD fade is measured from
        // the entity origin).
        let centre = Vec3::new((c.x as f32 + 0.5) * CHUNK_M, 0.0, (c.y as f32 + 0.5) * CHUNK_M);
        let ents = match st.chunks.get(&c) {
            Some((_, e)) => *e,
            None => [NEAR, FAR].map(|range| {
                commands
                    .spawn((
                        Name::new("Yards"),
                        MeshMaterial3d(mats.lit.clone()),
                        Transform::from_translation(centre),
                        Visibility::default(),
                        VisibilityRange {
                            start_margin: range.0,
                            end_margin: range.1,
                            use_aabb: false,
                        },
                    ))
                    .id()
            }),
        };
        for (e, mut mb) in ents.into_iter().zip([near, far]) {
            mb.translate(-centre);
            if mb.is_empty() {
                // Never remove Mesh3d: see cursor.rs.
                commands.entity(e).insert(Visibility::Hidden);
            } else {
                commands
                    .entity(e)
                    .insert((Mesh3d(meshes.add(mb.build())), Visibility::Inherited));
            }
        }
        st.chunks.insert(c, (hash, ents));
    }
}

fn v3(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

/// A boundary along a lot line (hedge, fence, wire), drawn once per line.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Boundary {
    Hedge(f32),
    Wood,
    Picket,
    Wire,
    Kerb,
}

fn line_key(a: Vec2, b: Vec2) -> [i32; 4] {
    let q = |p: Vec2| [(p.x * 4.0).round() as i32, (p.y * 4.0).round() as i32];
    let (qa, qb) = (q(a), q(b));
    if qa <= qb {
        [qa[0], qa[1], qb[0], qb[1]]
    } else {
        [qb[0], qb[1], qa[0], qa[1]]
    }
}

fn boundary(m: &mut MeshBuilder, a: Vec2, b: Vec2, kind: Boundary) {
    let d = b - a;
    let len = d.length();
    if len < 0.5 {
        return;
    }
    let yaw = -d.y.atan2(d.x);
    let mid = (a + b) * 0.5;
    match kind {
        Boundary::Hedge(h) => {
            let c = palette::HEDGE;
            m.box_yaw(v3(mid, 0.0), Vec3::new(len, h, 0.55), yaw, shade(c, 0.85), c);
        }
        Boundary::Wood => {
            let c = Color::srgb_u32(0x8A6A4A);
            m.box_yaw(v3(mid, 0.0), Vec3::new(len, 1.35, 0.07), yaw, c, shade(c, 1.1));
        }
        Boundary::Picket => {
            let c = palette::FENCE;
            m.box_yaw(v3(mid, 0.0), Vec3::new(len, 0.85, 0.06), yaw, c, c);
        }
        Boundary::Wire => {
            // A post every few metres and a see-through-looking top rail.
            let c = Color::srgb_u32(0x8C9399);
            let n = (len / 3.0).ceil().max(1.0) as usize;
            for k in 0..=n {
                let p = a + d * (k as f32 / n as f32);
                m.box_yaw(v3(p, 0.0), Vec3::new(0.08, 2.0, 0.08), yaw, c, c);
            }
            for h in [0.05, 0.95, 1.9] {
                m.box_yaw(v3(mid, h), Vec3::new(len, 0.05, 0.05), yaw, c, c);
            }
        }
        Boundary::Kerb => {
            let c = palette::KERB;
            m.box_yaw(v3(mid, 0.0), Vec3::new(len, 0.14, 0.25), yaw, c, c);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_yard(
    near: &mut MeshBuilder,
    far: &mut MeshBuilder,
    lines: &mut HashSet<[i32; 4]>,
    y: &Yard,
    edges: &RoadEdges,
    cars: &mut Cars,
    lorry: &mut Option<MeshBuilder>,
) {
    if let Some(rest) = y.rest {
        common_green(near, far, rest, y.seed, edges);
    }
    let Some(q) = clear_of_roads(y.quad, edges) else { return };
    let mut rng = Stream::new(y.seed ^ 0x7a2d_51e3);
    let (x0, z0, x1, z1) = y.inner();
    let (w, d) = (x1 - x0, z1 - z0);
    if w < 2.0 || d < 2.0 {
        return;
    }
    let level = y.level.clamp(1, 3);
    // Ground, boundaries: in both levels of detail.
    let (ground, bound) = match y.kind {
        Zone::Residential => {
            let lawn = rng.pick(&[
                palette::GRASS_LIGHT,
                mix(palette::GRASS, palette::GRASS_LIGHT, 0.5),
                mix(palette::GRASS_LIGHT, Color::srgb_u32(0xA8C46E), 0.35),
                shade(palette::GRASS_LIGHT, 0.96),
            ]);
            let b = if level >= 3 {
                Boundary::Hedge(1.0)
            } else {
                match rng.below(10) {
                    0..=4 => Boundary::Hedge(rng.range(0.9, 1.3)),
                    5..=7 => Boundary::Wood,
                    _ => Boundary::Picket,
                }
            };
            (lawn, b)
        }
        Zone::Commercial => (palette::ASPHALT, Boundary::Kerb),
        Zone::Industrial => (
            rng.pick(&[
                palette::YARD_CONCRETE,
                palette::GRAVEL,
                shade(palette::YARD_CONCRETE, 0.92),
            ]),
            Boundary::Wire,
        ),
    };
    for m in [&mut *near, &mut *far] {
        m.quad_up(q.map(|p| v3(p, GROUND_Y)), lin(ground));
    }
    // Back and both sides; the front (the building's back) stays open.
    // Gardens are hedged at the back; between neighbours mostly fences.
    let side = match bound {
        Boundary::Hedge(_) if y.kind == Zone::Residential && level < 3 => {
            if rng.chance(0.3) {
                bound
            } else {
                Boundary::Wood
            }
        }
        b => b,
    };
    for (k, (a, b)) in [(q[3], q[2]), (q[0], q[3]), (q[1], q[2])].into_iter().enumerate() {
        let mid = (a + b) * 0.5;
        if edges.clearance(mid) < ROAD_GAP + 0.6 || !lines.insert(line_key(a, b)) {
            continue;
        }
        let kind = if k == 0 { bound } else { side };
        boundary(near, a, b, kind);
        if !matches!(kind, Boundary::Wire | Boundary::Picket | Boundary::Wood) {
            boundary(far, a, b, kind);
        }
    }
    // The rest in the yard's own frame, appended in place.
    let mut m = MeshBuilder::new();
    let mut f = MeshBuilder { lod: true, ..default() };
    let (ix0, iz0, ix1, iz1) = (x0 + 0.9, z0 + 0.9, x1 - 0.9, z1 - 0.4);
    match y.kind {
        Zone::Residential => garden(&mut m, &mut f, &mut rng, level, (ix0, iz0, ix1, iz1)),
        Zone::Commercial => car_park(&mut m, &mut f, &mut rng, level, (ix0, iz0, ix1, iz1), cars),
        Zone::Industrial => storage(&mut m, &mut rng, level, (ix0, iz0, ix1, iz1), lorry),
    }
    let at = v3(y.centre(), 0.0);
    near.append(&m, at, y.yaw(), Vec3::ONE, [1.0; 4]);
    far.append(&f, at, y.yaw(), Vec3::ONE, [1.0; 4]);
}

/// Open common green on the back of a deep lot: grass in one shade for
/// every lot (neighbouring greens join up) and a few trees; no boundaries.
fn common_green(near: &mut MeshBuilder, far: &mut MeshBuilder, quad: [Vec2; 4], seed: u32, edges: &RoadEdges) {
    let Some(q) = clear_of_roads(quad, edges) else { return };
    let green = lin(mix(palette::GRASS, palette::GRASS_LIGHT, 0.35));
    for m in [&mut *near, &mut *far] {
        m.quad_up(q.map(|p| v3(p, GROUND_Y)), green);
    }
    let area = 0.5 * ((q[2] - q[0]).perp_dot(q[3] - q[1])).abs();
    let mut rng = Stream::new(seed ^ 0x6c3e_9a11);
    let want = area * GREEN_TREES_PER_M2;
    let n = want.floor() as usize + usize::from(rng.chance(want.fract()));
    for _ in 0..n {
        // A point inside the quad (bilinear), kept off its edges.
        let (u, v) = (rng.range(0.15, 0.85), rng.range(0.15, 0.85));
        let p = (q[0].lerp(q[1], u)).lerp(q[3].lerp(q[2], u), v);
        if edges.clearance(p) < 3.0 {
            continue;
        }
        tree(near, far, &mut rng, p);
    }
}

fn tree(m: &mut MeshBuilder, f: &mut MeshBuilder, rng: &mut Stream, p: Vec2) {
    let canopy = rng.pick(&palette::CANOPIES);
    let s = rng.range(0.5, 0.78);
    let yaw = rng.range(0.0, std::f32::consts::TAU);
    if rng.chance(0.75) {
        props::round_tree(m, v3(p, 0.0), s, canopy, yaw);
        props::round_tree(f, v3(p, 0.0), s, canopy, yaw);
    } else {
        props::pine_tree(m, v3(p, 0.0), s * 0.9, canopy, yaw);
        props::pine_tree(f, v3(p, 0.0), s * 0.9, canopy, yaw);
    }
}

/// A back garden (local frame: `+z` towards the house).
fn garden(m: &mut MeshBuilder, f: &mut MeshBuilder, rng: &mut Stream, level: u8, r: (f32, f32, f32, f32)) {
    let (x0, z0, x1, z1) = r;
    let (w, d) = (x1 - x0, z1 - z0);
    if level >= 3 {
        // Flats: a shared garden with a path down the middle and benches.
        let cx = (x0 + x1) * 0.5;
        lot_props::patch(m, cx - 0.7, z0, cx + 0.7, z1, GROUND_Y + 0.01, palette::PATH);
        let n = ((d / 7.0) as i32).clamp(1, 4);
        for k in 0..n {
            let z = z0 + (k as f32 + 0.5) * d / n as f32;
            tree(m, f, rng, Vec2::new(x0 + w * 0.22, z));
            tree(m, f, rng, Vec2::new(x1 - w * 0.22, z));
            if k % 2 == 0 {
                let c = Color::srgb_u32(0x7A5A3E);
                m.box_yaw(
                    Vec3::new(cx + 1.3, 0.0, z),
                    Vec3::new(0.5, 0.45, 1.6),
                    0.0,
                    c,
                    shade(c, 1.1),
                );
            }
        }
        return;
    }
    // Trees at the back.
    let n = ((w * d / 60.0) as i32).clamp(1, 4);
    for k in 0..n {
        let s = (k as f32 + 0.5) / n as f32;
        let p = Vec2::new(
            x0 + w * (0.15 + 0.7 * s) + rng.range(-1.0, 1.0),
            z0 + rng.range(1.2, (d * 0.45).max(1.3)),
        );
        tree(m, f, rng, p);
    }
    let left = rng.chance(0.5);
    // A shed in a back corner.
    if d > 5.0 && rng.chance(0.6) {
        let x = if left { x0 + 1.4 } else { x1 - 1.4 };
        lot_props::shed(m, rng, Vec2::new(x, z0 + 1.2), 1.0);
    }
    // A patio by the house, or a pool on bigger plots of nicer homes.
    if level >= 2 && w * d > 90.0 && rng.chance(0.3) {
        let (pw, pd) = ((w * 0.5).min(7.0), (d * 0.35).min(4.5));
        let (cx, cz) = ((x0 + x1) * 0.5, z1 - pd * 0.5 - 1.2);
        lot_props::patch(
            m,
            cx - pw * 0.5 - 0.6,
            cz - pd * 0.5 - 0.6,
            cx + pw * 0.5 + 0.6,
            cz + pd * 0.5 + 0.6,
            GROUND_Y + 0.01,
            palette::PAVING,
        );
        lot_props::patch(
            m,
            cx - pw * 0.5,
            cz - pd * 0.5,
            cx + pw * 0.5,
            cz + pd * 0.5,
            GROUND_Y + 0.02,
            Color::srgb_u32(0x4FB3D9),
        );
    } else if rng.chance(0.45) {
        let pw = (w * 0.45).min(5.0);
        let px = if left { x1 - pw - 0.3 } else { x0 + 0.3 };
        lot_props::patio(m, rng, px, z1 - 3.2, px + pw, z1 - 0.3);
    }
    if w * d > 70.0 && rng.chance(0.35) {
        let bx = if left { x1 - 3.4 } else { x0 + 0.4 };
        lot_props::veg_beds(m, rng, bx, z0 + d * 0.35, bx + 3.0, z0 + d * 0.35 + 2.6);
    }
    if rng.chance(0.3) {
        let z = z0 + d * 0.55;
        lot_props::washing_line(
            m,
            rng,
            Vec2::new(x0 + 0.8, z),
            Vec2::new((x0 + w * 0.6).min(x1 - 0.8), z),
        );
    }
}

/// A car park behind shops and offices: bays along the back, cars in some.
fn car_park(
    m: &mut MeshBuilder,
    f: &mut MeshBuilder,
    rng: &mut Stream,
    level: u8,
    r: (f32, f32, f32, f32),
    cars: &mut Cars,
) {
    let (x0, z0, x1, z1) = r;
    let (w, d) = (x1 - x0, z1 - z0);
    let bay = 2.6;
    let fill = [0.45, 0.65, 0.85][(level - 1) as usize];
    // Rows of bays 5 m deep, an aisle between two rows.
    let mut rows: Vec<(f32, bool)> = Vec::new();
    let mut z = z0;
    let mut facing = true;
    while z + 5.0 <= z1 - 0.2 {
        rows.push((z, facing));
        z += 5.0;
        if !facing {
            z += 6.0;
        }
        facing = !facing;
    }
    let n = (w / bay).floor() as i32;
    let xs = x0 + (w - n as f32 * bay) * 0.5;
    let line = palette::MARKING;
    for &(rz, fwd) in &rows {
        for k in 0..=n {
            let x = xs + k as f32 * bay;
            lot_props::patch(m, x - 0.05, rz + 0.3, x + 0.05, rz + 4.7, GROUND_Y + 0.01, line);
        }
        for k in 0..n {
            if !rng.chance(fill) {
                continue;
            }
            let x = xs + (k as f32 + 0.5) * bay;
            let body = vehicle_gen::paint(rng.next_u32());
            let shape = [0, 1, 4, 5, 0, 1, 2, 5][rng.below(8) as usize];
            let yaw = if fwd { 0.0 } else { std::f32::consts::PI } + rng.range(-0.04, 0.04);
            let car = cars.get(body, shape);
            let start = m.pos.len();
            m.append(car, Vec3::new(x, 0.04, rz + 2.5), yaw, Vec3::ONE, [1.0; 4]);
            // Parked: dark lights.
            for uv in &mut m.uv[start..] {
                *uv = [0.0, 0.0];
            }
        }
    }
    // A tree or two in the corners, a lamp post.
    if d > 6.0 {
        tree(m, f, rng, Vec2::new(x0 + 0.4, z1 - 0.6));
        if w > 12.0 {
            tree(m, f, rng, Vec2::new(x1 - 0.4, z1 - 0.6));
        }
    }
    let p = Vec3::new((x0 + x1) * 0.5, 0.0, z0 + d * 0.5);
    m.cylinder(p, 0.08, 6.0, 5, palette::LAMP, palette::LAMP);
    m.box_yaw(
        p + Vec3::Y * 6.0,
        Vec3::new(0.7, 0.18, 0.3),
        0.0,
        palette::LAMP,
        palette::LAMP_GLASS,
    );
}

/// A storage yard behind industry.
fn storage(m: &mut MeshBuilder, rng: &mut Stream, level: u8, r: (f32, f32, f32, f32), lorry: &mut Option<MeshBuilder>) {
    let (x0, z0, x1, z1) = r;
    let (w, d) = (x1 - x0, z1 - z0);
    let mut z = z0 + 0.3;
    // Containers along the back, stacked at higher levels.
    let cont = (w / 3.0).floor() as i32;
    let liveries = [
        Color::srgb_u32(0xB5483D),
        Color::srgb_u32(0x2F6FA3),
        Color::srgb_u32(0x3E8A5B),
        Color::srgb_u32(0xC9A23A),
        Color::srgb_u32(0x8A8F94),
    ];
    if d > 7.0 {
        for k in 0..cont {
            if !rng.chance(0.55 + 0.15 * level as f32) {
                continue;
            }
            let x = x0 + (k as f32 + 0.5) * w / cont.max(1) as f32;
            let stack = 1 + u32::from(level >= 2 && rng.chance(0.5)) + u32::from(level >= 3 && rng.chance(0.4));
            for s in 0..stack {
                let c = rng.pick(&liveries);
                m.box_yaw(
                    Vec3::new(x, s as f32 * 2.6, z + 3.05),
                    Vec3::new(2.4, 2.55, 6.0),
                    rng.range(-0.02, 0.02),
                    c,
                    shade(c, 1.12),
                );
            }
        }
        z += 6.6;
    }
    // Pallets, crates and a tank or two in the rest.
    let rest = z1 - z;
    if rest > 3.0 {
        let n = ((w * rest / 30.0) as i32).clamp(1, 6);
        for _ in 0..n {
            let p = Vec2::new(rng.range(x0 + 1.0, x1 - 1.0), rng.range(z + 0.8, z1 - 0.8));
            match rng.below(3) {
                0 => lot_props::pallets(m, rng, p),
                1 => {
                    for k in 0..rng.below(3) + 1 {
                        props::crate_box(
                            m,
                            Vec3::new(p.x + k as f32 * 0.9, 0.0, p.y),
                            rng.range(0.7, 1.0),
                            rng.range(0.0, 0.6),
                        );
                    }
                }
                _ => m.cylinder(
                    Vec3::new(p.x, 0.0, p.y),
                    1.1,
                    3.2,
                    8,
                    palette::TANK,
                    shade(palette::TANK, 1.1),
                ),
            }
        }
    }
    // A lorry waiting at bigger works.
    if level >= 2 && rest > 9.0 && w > 5.0 && rng.chance(0.5) {
        let l = lorry.get_or_insert_with(|| {
            let (cab, cargo, stripe) = palette::TRUCK_LIVERIES[0];
            vehicle_gen::truck_model(cab, cargo, stripe, 0, super::building_gen::Lod::Mid)
        });
        let start = m.pos.len();
        m.append(
            l,
            Vec3::new((x0 + x1) * 0.5, 0.04, z1 - 4.5),
            std::f32::consts::FRAC_PI_2,
            Vec3::ONE,
            [1.0; 4],
        );
        for uv in &mut m.uv[start..] {
            *uv = [0.0, 0.0];
        }
    }
}
