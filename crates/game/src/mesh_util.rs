//! Building flat, upward-facing meshes from quads and polygons.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

#[derive(Default)]
pub struct FlatMeshBuilder {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
}

impl FlatMeshBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        // Make the triangle face +Y regardless of the input winding.
        let up = (c - a).cross(b - a).y >= 0.0;
        let base = self.positions.len() as u32;
        let (b, c) = if up { (b, c) } else { (c, b) };
        self.positions.extend([a.to_array(), c.to_array(), b.to_array()]);
        self.indices.extend([base, base + 1, base + 2]);
    }

    /// Convex quad, corners in order around the edge.
    pub fn quad(&mut self, q: [Vec3; 4]) {
        self.tri(q[0], q[1], q[2]);
        self.tri(q[0], q[2], q[3]);
    }

    /// Regular polygon (disc) around `center`.
    pub fn disc(&mut self, center: Vec3, radius: f32, sides: u32) {
        for i in 0..sides {
            let a0 = i as f32 / sides as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / sides as f32 * std::f32::consts::TAU;
            let p0 = center + Vec3::new(a0.cos(), 0.0, a0.sin()) * radius;
            let p1 = center + Vec3::new(a1.cos(), 0.0, a1.sin()) * radius;
            self.tri(center, p0, p1);
        }
    }

    /// Thick line on the ground from `a` to `b`.
    pub fn strip(&mut self, a: Vec3, b: Vec3, width: f32) {
        let d = (b - a).with_y(0.0);
        if d.length_squared() < 1e-6 {
            return;
        }
        let side = Vec3::new(-d.z, 0.0, d.x).normalize() * (width * 0.5);
        self.quad([a + side, b + side, b - side, a - side]);
    }

    pub fn build(self) -> Mesh {
        let n = self.positions.len();
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n])
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; n])
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    #[test]
    fn triangles_face_up_whatever_the_input_winding() {
        let mut b = FlatMeshBuilder::new();
        b.quad([Vec3::ZERO, Vec3::X, Vec3::new(1.0, 0.0, 1.0), Vec3::Z]);
        b.quad([Vec3::ZERO, Vec3::Z, Vec3::new(1.0, 0.0, 1.0), Vec3::X]);
        b.strip(Vec3::ZERO, Vec3::new(10.0, 0.0, 3.0), 2.0);
        b.disc(Vec3::ZERO, 1.0, 8);
        let mesh = b.build();
        let Some(VertexAttributeValues::Float32x3(pos)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
            panic!("positions");
        };
        for t in pos.chunks(3) {
            let (a, b, c) = (Vec3::from(t[0]), Vec3::from(t[1]), Vec3::from(t[2]));
            // Counter-clockwise seen from above => normal (b-a)x(c-a) points +Y.
            assert!((b - a).cross(c - a).y > 0.0, "triangle faces down: {t:?}");
        }
    }
}
