//! Mesh building helpers.
//!
//! [`MeshBuilder`] makes flat-shaded, vertex-coloured triangle meshes (every
//! triangle has its own vertices and face normal), which is the whole look of
//! the diorama: one shared white material, colour in the vertices, so
//! thousands of objects batch together.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::palette::lin;

/// What a vertex glows as at night (`uv.x` in the city shader; keep in
/// sync with `assets/gfx/shaders/city.wesl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Glow {
    // 0: no glow (the default tag).
    /// A window: lit in the evening, a varying share per building.
    Window = 1,
    /// Street lamp head: on from dusk to dawn.
    Lamp = 2,
    /// Car headlight: on at night.
    Headlight = 3,
    /// Tail light: dim at night, bright when braking.
    TailLight = 4,
    /// Shop sign / shop window: its own colour, glowing at night.
    Sign = 5,
    /// Always glowing in its own colour (signal lenses).
    Always = 6,
    /// Shop front / lobby glass: warmly lit after dark.
    Shop = 7,
    /// Not a light: the ground, which the shader gives grass detail,
    /// wildflowers near the camera and farmland beyond the map.
    Ground = 8,
    /// Body of a distant building (far level of detail): at night it takes
    /// on a faint warm glow by the share of lit windows, so a town zoomed
    /// all the way out (windows a pixel or less) still reads as lit.
    Body = 9,
    // Surfaces (not lights): a faint procedural pattern in the shader,
    // faded out with distance. Only the detailed models carry them.
    /// Roof tiles: courses along the slope, a little variation per tile.
    Roof = 10,
    /// Weatherboard / siding: horizontal boards.
    Siding = 11,
    /// Brick: courses and staggered joints.
    Brick = 12,
    /// Profiled metal cladding: vertical ribs.
    Cladding = 13,
    /// Flat roof membrane / concrete: faint panel seams.
    Membrane = 14,
}

/// Flat-shaded, vertex-coloured triangle soup.
#[derive(Default, Clone, Debug)]
pub struct MeshBuilder {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub col: Vec<[f32; 4]>,
    /// Per-vertex glow tag read by the city shader (`uv.x` = [`Glow`] kind,
    /// `uv.y` = a random value per lit element, e.g. per window).
    pub uv: Vec<[f32; 2]>,
    pub idx: Vec<u32>,
    /// Tag written into `uv` for every vertex pushed while it is set.
    pub tag: [f32; 2],
    /// When > 0, vertical faces get darker towards the ground (baked
    /// ambient occlusion), by up to this fraction at `y = 0`.
    pub ground_ao: f32,
    /// Building the distant level of detail: generators skip small things.
    pub lod: bool,
    /// Building the near level of detail: generators add fine trim (sills,
    /// glazing bars, door panels, small props). Never set with `lod`.
    pub fine: bool,
    /// Smoke emitters (chimney tops) in model space, for `smoke`.
    pub emitters: Vec<Vec3>,
    /// Driveways meeting the lot's front edge (`+z`): their model-space x
    /// ranges, so street furniture keeps clear of them.
    pub driveways: Vec<(f32, f32)>,
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ground_ao(ao: f32) -> Self {
        MeshBuilder {
            ground_ao: ao,
            ..default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }

    /// Move every vertex by `v`.
    pub fn translate(&mut self, v: Vec3) {
        for p in &mut self.pos {
            p[0] += v.x;
            p[1] += v.y;
            p[2] += v.z;
        }
    }

    /// Current size, for [`MeshBuilder::rollback`].
    pub fn mark(&self) -> (usize, usize) {
        (self.pos.len(), self.idx.len())
    }

    /// Drop everything added since `mark`.
    pub fn rollback(&mut self, mark: (usize, usize)) {
        self.pos.truncate(mark.0);
        self.nrm.truncate(mark.0);
        self.col.truncate(mark.0);
        self.uv.truncate(mark.0);
        self.idx.truncate(mark.1);
    }

    /// Run `f`, keeping its geometry only in the detailed model (random
    /// numbers are still drawn so both levels of detail stay identical).
    pub fn detail_only(&mut self, f: impl FnOnce(&mut MeshBuilder)) {
        let mark = self.mark();
        f(self);
        if self.lod {
            self.rollback(mark);
        }
    }

    /// Run `f`, keeping its geometry only in the near model (`fine`).
    /// Random numbers are drawn either way.
    pub fn fine_only(&mut self, f: impl FnOnce(&mut MeshBuilder)) {
        let mark = self.mark();
        f(self);
        if !self.fine {
            self.rollback(mark);
        }
    }

    /// Run `f` with its surfaces tagged `kind` (a pattern in the shader),
    /// except in the distant model, which keeps its `Body` tag.
    pub fn surface(&mut self, kind: Glow, f: impl FnOnce(&mut MeshBuilder)) {
        if self.lod {
            f(self);
        } else {
            self.glow(kind, 0.0, f);
        }
    }

    #[cfg(test)]
    pub fn triangle_count(&self) -> usize {
        self.idx.len() / 3
    }

    fn ao(&self, p: Vec3, n: Vec3) -> f32 {
        if self.ground_ao <= 0.0 || n.y.abs() > 0.5 {
            return 1.0;
        }
        1.0 - self.ground_ao * (-p.y.max(0.0) / 1.1).exp()
    }

    fn push(&mut self, p: Vec3, n: Vec3, c: [f32; 4]) -> u32 {
        let k = self.ao(p, n);
        let i = self.pos.len() as u32;
        self.pos.push(p.to_array());
        self.nrm.push(n.to_array());
        self.col.push([c[0] * k, c[1] * k, c[2] * k, c[3]]);
        self.uv.push(self.tag);
        i
    }

    /// Run `f` with every vertex tagged as glowing element `kind` (random
    /// value `r` in `0..1`, used by windows to decide which ones are lit).
    pub fn glow(&mut self, kind: Glow, r: f32, f: impl FnOnce(&mut MeshBuilder)) {
        let old = self.tag;
        self.tag = [kind as u32 as f32, r];
        f(self);
        self.tag = old;
    }

    /// Triangle with per-vertex linear colours; front face is counter-clockwise.
    pub fn tri_c(&mut self, a: Vec3, b: Vec3, c: Vec3, cols: [[f32; 4]; 3]) {
        let n = (b - a).cross(c - a).normalize_or(Vec3::Y);
        let i = self.push(a, n, cols[0]);
        self.push(b, n, cols[1]);
        self.push(c, n, cols[2]);
        self.idx.extend([i, i + 1, i + 2]);
    }

    pub fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3, color: Color) {
        let l = lin(color);
        self.tri_c(a, b, c, [l; 3]);
    }

    /// Planar quad, corners counter-clockwise seen from the front.
    pub fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: Color) {
        let l = lin(color);
        self.quad_c([a, b, c, d], [l; 4]);
    }

    /// Planar quad with per-corner linear colours.
    pub fn quad_c(&mut self, q: [Vec3; 4], cols: [[f32; 4]; 4]) {
        let n = (q[1] - q[0]).cross(q[2] - q[0]);
        let n = if n.length_squared() > 1e-12 {
            n.normalize()
        } else {
            (q[2] - q[0]).cross(q[3] - q[0]).normalize_or(Vec3::Y)
        };
        let i = self.push(q[0], n, cols[0]);
        self.push(q[1], n, cols[1]);
        self.push(q[2], n, cols[2]);
        self.push(q[3], n, cols[3]);
        self.idx.extend([i, i + 1, i + 2, i, i + 2, i + 3]);
    }

    /// Triangle whose front faces `n` whatever the input winding.
    pub fn tri_facing(&mut self, a: Vec3, b: Vec3, c: Vec3, n: Vec3, color: Color) {
        if (b - a).cross(c - a).dot(n) >= 0.0 {
            self.tri(a, b, c, color);
        } else {
            self.tri(a, c, b, color);
        }
    }

    /// Planar quad whose front faces `n` whatever the input winding.
    pub fn quad_facing(&mut self, q: [Vec3; 4], n: Vec3, color: Color) {
        if (q[1] - q[0]).cross(q[2] - q[0]).dot(n) >= 0.0 {
            self.quad(q[0], q[1], q[2], q[3], color);
        } else {
            self.quad(q[3], q[2], q[1], q[0], color);
        }
    }

    /// Horizontal triangle facing +Y whatever the input winding.
    pub fn tri_up(&mut self, a: Vec3, b: Vec3, c: Vec3, color: [f32; 4]) {
        if (b - a).cross(c - a).y >= 0.0 {
            self.tri_c(a, b, c, [color; 3]);
        } else {
            self.tri_c(a, c, b, [color; 3]);
        }
    }

    /// Horizontal convex quad facing +Y whatever the input winding.
    pub fn quad_up(&mut self, q: [Vec3; 4], color: [f32; 4]) {
        self.tri_up(q[0], q[1], q[2], color);
        self.tri_up(q[0], q[2], q[3], color);
    }

    /// Fan of upward triangles from `center` around a closed ring.
    pub fn fan_up(&mut self, center: Vec3, ring: &[Vec3], color: [f32; 4]) {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            if (a - center).cross(b - center).y.abs() > 1e-6 {
                self.tri_up(center, a, b, color);
            }
        }
    }

    /// Vertical rectangle above the horizontal edge `a -> b`, from `y0` to
    /// `y1`. Its outward normal is `(b - a) x Y` (to the right of `a -> b`
    /// seen from above with +Z towards the viewer... i.e. `dir.cross(Vec3::Y)`).
    pub fn wall(&mut self, a: Vec2, b: Vec2, y0: f32, y1: f32, color: Color) {
        let (a0, b0) = (Vec3::new(a.x, y0, a.y), Vec3::new(b.x, y0, b.y));
        let (a1, b1) = (Vec3::new(a.x, y1, a.y), Vec3::new(b.x, y1, b.y));
        // Split tall walls so the baked ground AO stays near the ground.
        if self.ground_ao > 0.0 && y0 < 0.5 && y1 - y0 > 3.0 {
            let ym = y0 + 1.8;
            let (am, bm) = (Vec3::new(a.x, ym, a.y), Vec3::new(b.x, ym, b.y));
            self.quad(a0, b0, bm, am, color);
            self.quad(am, bm, b1, a1, color);
        } else {
            self.quad(a0, b0, b1, a1, color);
        }
    }

    /// Prism over a convex or simple polygon `outline` (any winding), from
    /// `y0` to `y1`, with a top face (fan from the first vertex: convex only).
    pub fn prism(&mut self, outline: &[Vec2], y0: f32, y1: f32, walls: Color, top: Option<Color>) {
        let ccw = signed_area(outline) > 0.0;
        let n = outline.len();
        for i in 0..n {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            // In Bevy (x, z) seen from above, positive signed area means the
            // outward side of edge a->b is `dir.cross(Y)` reversed.
            if ccw {
                self.wall(b, a, y0, y1, walls);
            } else {
                self.wall(a, b, y0, y1, walls);
            }
        }
        if let Some(top) = top {
            let l = lin(top);
            let pts: Vec<Vec3> = outline.iter().map(|p| Vec3::new(p.x, y1, p.y)).collect();
            for i in 1..n.saturating_sub(1) {
                self.tri_up(pts[0], pts[i], pts[i + 1], l);
            }
        }
    }

    /// Axis-aligned box without a bottom face.
    pub fn cuboid(&mut self, min: Vec3, max: Vec3, color: Color) {
        self.cuboid2(min, max, color, color);
    }

    /// Axis-aligned box without a bottom face, separate top colour.
    pub fn cuboid2(&mut self, min: Vec3, max: Vec3, sides: Color, top: Color) {
        let r = [
            Vec2::new(min.x, min.z),
            Vec2::new(max.x, min.z),
            Vec2::new(max.x, max.z),
            Vec2::new(min.x, max.z),
        ];
        self.prism(&r, min.y, max.y, sides, Some(top));
    }

    /// Box rotated by `yaw` around +Y, centred at `center` (bottom at
    /// `center.y`), size `(w, h, d)`.
    pub fn box_yaw(&mut self, center: Vec3, size: Vec3, yaw: f32, sides: Color, top: Color) {
        let rot = Vec2::from_angle(-yaw);
        let hw = size.x * 0.5;
        let hd = size.z * 0.5;
        let c = Vec2::new(center.x, center.z);
        let r = [
            c + rot.rotate(Vec2::new(-hw, -hd)),
            c + rot.rotate(Vec2::new(hw, -hd)),
            c + rot.rotate(Vec2::new(hw, hd)),
            c + rot.rotate(Vec2::new(-hw, hd)),
        ];
        self.prism(&r, center.y, center.y + size.y, sides, Some(top));
    }

    /// Low-poly vertical cylinder (`sides` facets) with a top cap.
    pub fn cylinder(&mut self, center: Vec3, radius: f32, height: f32, sides: u32, walls: Color, top: Color) {
        let ring: Vec<Vec2> = (0..sides)
            .map(|i| {
                let a = i as f32 / sides as f32 * std::f32::consts::TAU;
                Vec2::new(center.x + radius * a.cos(), center.z + radius * a.sin())
            })
            .collect();
        self.prism(&ring, center.y, center.y + height, walls, Some(top));
    }

    /// Cone (`sides` facets) from a ring at `base` up to an apex.
    pub fn cone(&mut self, base: Vec3, radius: f32, height: f32, sides: u32, color: Color, phase: f32) {
        let apex = base + Vec3::Y * height;
        let l = lin(color);
        let dark = lin(super::palette::shade(color, 0.85));
        for i in 0..sides {
            let a0 = (i as f32 / sides as f32 + phase) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / sides as f32 + phase) * std::f32::consts::TAU;
            let p0 = base + Vec3::new(a0.cos(), 0.0, a0.sin()) * radius;
            let p1 = base + Vec3::new(a1.cos(), 0.0, a1.sin()) * radius;
            // Counter-clockwise seen from outside.
            self.tri_c(p1, p0, apex, [dark, dark, l]);
        }
    }

    /// Append `other`, transformed (rotation about Y, uniform scale, then
    /// translation), optionally tinted (linear multiply).
    pub fn append(&mut self, other: &MeshBuilder, translation: Vec3, yaw: f32, scale: Vec3, tint: [f32; 4]) {
        let q = Quat::from_rotation_y(yaw);
        let base = self.pos.len() as u32;
        self.uv.extend_from_slice(&other.uv);
        for ((p, n), c) in other.pos.iter().zip(&other.nrm).zip(&other.col) {
            let p = q * (Vec3::from(*p) * scale) + translation;
            let n = (q * (Vec3::from(*n) / scale)).normalize_or(Vec3::Y);
            self.pos.push(p.to_array());
            self.nrm.push(n.to_array());
            self.col
                .push([c[0] * tint[0], c[1] * tint[1], c[2] * tint[2], c[3] * tint[3]]);
        }
        self.idx.extend(other.idx.iter().map(|i| i + base));
    }

    pub fn build(self) -> Mesh {
        self.build_with(RenderAssetUsages::default())
    }

    /// The mesh for the GPU only: Bevy drops its CPU copy once uploaded
    /// (it keeps the bounding box for culling). Use for models nothing
    /// reads back (buildings, vehicles); [`gpu_mesh_stats`] still counts
    /// them.
    pub fn build_gpu(self) -> Mesh {
        let tris = self.idx.len() / 3;
        let bytes = self.pos.len() * (12 + 12 + 16 + 8) + self.idx.len() * 4;
        GPU_ONLY_TRIS.fetch_add(tris, Ordering::Relaxed);
        GPU_ONLY_BYTES.fetch_add(bytes, Ordering::Relaxed);
        self.build_with(RenderAssetUsages::RENDER_WORLD)
    }

    fn build_with(self, usage: RenderAssetUsages) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, usage)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.nrm)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.col)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
            .with_inserted_indices(Indices::U32(self.idx))
    }
}

static GPU_ONLY_TRIS: AtomicUsize = AtomicUsize::new(0);
static GPU_ONLY_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Triangles and bytes of every mesh built with
/// [`MeshBuilder::build_gpu`] so far (their CPU copies are gone; for the
/// benchmark's geometry totals).
pub fn gpu_mesh_stats() -> (usize, usize) {
    (
        GPU_ONLY_TRIS.load(Ordering::Relaxed),
        GPU_ONLY_BYTES.load(Ordering::Relaxed),
    )
}

fn gpu_registry() -> &'static std::sync::Mutex<std::collections::HashMap<AssetId<Mesh>, usize>> {
    static R: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<AssetId<Mesh>, usize>>> =
        std::sync::OnceLock::new();
    R.get_or_init(Default::default)
}

/// Add a mesh built with [`MeshBuilder::build_gpu`], noting its triangle
/// count (its index data is gone from the main world once uploaded).
pub fn add_gpu(meshes: &mut Assets<Mesh>, mesh: Mesh) -> Handle<Mesh> {
    let tris = mesh.try_indices_option().ok().flatten().map_or(0, |i| i.len() / 3);
    let h = meshes.add(mesh);
    if let Ok(mut r) = gpu_registry().lock() {
        r.insert(h.id(), tris);
    }
    h
}

/// Triangles of a mesh: from its data, or from the registry when it is a
/// GPU-only mesh.
pub fn mesh_triangles(id: AssetId<Mesh>, mesh: &Mesh) -> usize {
    match mesh.try_indices_option() {
        Ok(i) => i.map_or(0, |i| i.len() / 3),
        Err(_) => gpu_registry()
            .lock()
            .ok()
            .and_then(|r| r.get(&id).copied())
            .unwrap_or(0),
    }
}

/// Signed area of a polygon in the (x, z) plane (shoelace on `Vec2(x, z)`).
pub fn signed_area(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f32>() * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    fn positions(mesh: &Mesh) -> Vec<[f32; 3]> {
        let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
            panic!("positions");
        };
        pos.clone()
    }

    #[test]
    fn upward_triangles_face_up_whatever_the_input_winding() {
        let mut b = MeshBuilder::new();
        b.quad_up([Vec3::ZERO, Vec3::X, Vec3::new(1.0, 0.0, 1.0), Vec3::Z], [1.0; 4]);
        b.quad_up([Vec3::ZERO, Vec3::Z, Vec3::new(1.0, 0.0, 1.0), Vec3::X], [1.0; 4]);
        let ring = [Vec3::X, Vec3::Z, -Vec3::X, -Vec3::Z];
        b.fan_up(Vec3::ZERO, &ring, [1.0; 4]);
        let mesh = b.build();
        let pos = positions(&mesh);
        let Some(Indices::U32(idx)) = mesh.indices() else {
            panic!("indices")
        };
        for t in idx.chunks(3) {
            let (a, b, c) = (
                Vec3::from(pos[t[0] as usize]),
                Vec3::from(pos[t[1] as usize]),
                Vec3::from(pos[t[2] as usize]),
            );
            assert!((b - a).cross(c - a).y > 0.0, "triangle faces down: {t:?}");
        }
    }

    #[test]
    fn prism_faces_point_outward_for_either_winding() {
        for reverse in [false, true] {
            let mut outline = vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(4.0, 0.0),
                Vec2::new(4.0, 3.0),
                Vec2::new(0.0, 3.0),
            ];
            if reverse {
                outline.reverse();
            }
            let mut b = MeshBuilder::new();
            b.prism(&outline, 0.0, 2.0, Color::WHITE, Some(Color::WHITE));
            let centre = Vec3::new(2.0, 1.0, 1.5);
            for t in b.idx.chunks(3) {
                let p: Vec<Vec3> = t.iter().map(|&i| Vec3::from(b.pos[i as usize])).collect();
                let n = (p[1] - p[0]).cross(p[2] - p[0]);
                let mid = (p[0] + p[1] + p[2]) / 3.0;
                assert!(n.dot(mid - centre) > 0.0, "face points inward (reverse={reverse})");
                // Stored normal agrees with the winding.
                let stored = Vec3::from(b.nrm[t[0] as usize]);
                assert!(stored.dot(n) > 0.0);
            }
        }
    }

    #[test]
    fn ground_ao_darkens_only_low_vertical_faces() {
        let mut b = MeshBuilder::with_ground_ao(0.3);
        b.cuboid(Vec3::ZERO, Vec3::new(1.0, 2.0, 1.0), Color::WHITE);
        for ((p, n), c) in b.pos.iter().zip(&b.nrm).zip(&b.col) {
            if n[1] > 0.5 || p[1] > 1.5 {
                assert!(c[0] > 0.85, "{p:?} {c:?}");
            } else {
                assert!(c[0] < 0.75, "{p:?} {c:?}");
            }
        }
    }
}
