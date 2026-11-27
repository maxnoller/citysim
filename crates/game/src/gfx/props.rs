//! Small low-poly props shared by buildings, trees and scenery: trees,
//! bushes, crates. All write into a [`MeshBuilder`] at a given spot.

use bevy::prelude::*;

use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, shade};

/// Unit icosahedron vertices and faces.
fn icosahedron() -> ([Vec3; 12], [[usize; 3]; 20]) {
    let t = (1.0 + 5f32.sqrt()) * 0.5;
    let v = [
        Vec3::new(-1.0, t, 0.0),
        Vec3::new(1.0, t, 0.0),
        Vec3::new(-1.0, -t, 0.0),
        Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t),
        Vec3::new(0.0, 1.0, t),
        Vec3::new(0.0, -1.0, -t),
        Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0),
        Vec3::new(t, 0.0, 1.0),
        Vec3::new(-t, 0.0, -1.0),
        Vec3::new(-t, 0.0, 1.0),
    ]
    .map(|p| p.normalize());
    let f = [
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    (v, f)
}

/// Faceted blob (icosahedron) centred at `c` with radii `r`; the lower
/// half is shaded darker so it reads as a rounded canopy.
pub fn blob(m: &mut MeshBuilder, c: Vec3, r: Vec3, color: Color, yaw: f32) {
    let (v, f) = icosahedron();
    let q = Quat::from_rotation_y(yaw);
    let top = lin(color);
    let low = lin(shade(color, 0.78));
    for tri in f {
        let p = tri.map(|i| c + q * (v[i] * r));
        let cols = tri.map(|i| if v[i].y > 0.2 { top } else { low });
        // Icosahedron faces above are listed counter-clockwise from outside.
        m.tri_c(p[0], p[1], p[2], cols);
    }
}

/// Eight-sided canopy for distant trees.
pub fn octa_blob(m: &mut MeshBuilder, c: Vec3, r: Vec3, color: Color, yaw: f32) {
    let q = Quat::from_rotation_y(yaw);
    let top = c + Vec3::Y * r.y;
    let bottom = c - Vec3::Y * r.y;
    let ring: Vec<Vec3> = (0..4)
        .map(|i| {
            let a = i as f32 * std::f32::consts::FRAC_PI_2;
            c + q * Vec3::new(a.cos() * r.x, 0.0, a.sin() * r.z)
        })
        .collect();
    let low = shade(color, 0.78);
    for i in 0..4 {
        let (a, b) = (ring[i], ring[(i + 1) % 4]);
        m.tri(top, b, a, color);
        m.tri(bottom, a, b, low);
    }
}

/// Broadleaf tree: trunk and one or two canopy blobs. `s` scales it
/// (1.0 is about 7 m tall).
pub fn round_tree(m: &mut MeshBuilder, base: Vec3, s: f32, canopy: Color, yaw: f32) {
    m.cylinder(base, 0.18 * s, 2.6 * s, 5, palette::TRUNK, palette::TRUNK);
    blob(m, base + Vec3::Y * 4.4 * s, Vec3::new(2.3, 2.2, 2.3) * s, canopy, yaw);
}

/// A garden or street tree of a `species` (0 round, 1 tall oval, 2 birch,
/// 3 conifer), `s` scales it (1.0 is about 7 m tall).
pub fn garden_tree(m: &mut MeshBuilder, base: Vec3, s: f32, canopy: Color, yaw: f32, species: u32) {
    match species % 4 {
        3 => pine_tree(m, base, s, canopy, yaw),
        _ => round_tree(m, base, s, canopy, yaw),
    }
}

/// Conifer: trunk and two stacked cones.
pub fn pine_tree(m: &mut MeshBuilder, base: Vec3, s: f32, canopy: Color, yaw: f32) {
    let phase = yaw / std::f32::consts::TAU;
    m.cylinder(base, 0.16 * s, 1.4 * s, 5, palette::TRUNK, palette::TRUNK);
    m.cone(base + Vec3::Y * 1.0 * s, 1.9 * s, 4.2 * s, 7, shade(canopy, 0.9), phase);
    m.cone(base + Vec3::Y * 3.4 * s, 1.35 * s, 3.6 * s, 7, canopy, phase + 0.07);
}

/// Low garden bush.
pub fn bush(m: &mut MeshBuilder, base: Vec3, s: f32, color: Color, yaw: f32) {
    m.detail_only(|m| blob(m, base + Vec3::Y * 0.5 * s, Vec3::new(0.8, 0.58, 0.8) * s, color, yaw));
}

/// Wooden crate.
pub fn crate_box(m: &mut MeshBuilder, base: Vec3, size: f32, yaw: f32) {
    m.detail_only(|m| m.box_yaw(base, Vec3::splat(size), yaw, palette::CRATE, shade(palette::CRATE, 1.1)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_faces_point_outward_and_props_stand_on_the_ground() {
        let mut m = MeshBuilder::new();
        blob(&mut m, Vec3::ZERO, Vec3::ONE, Color::WHITE, 0.3);
        for t in m.idx.chunks(3) {
            let p: Vec<Vec3> = t.iter().map(|&i| Vec3::from(m.pos[i as usize])).collect();
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(n.dot(p[0] + p[1] + p[2]) > 0.0);
        }
        let mut m = MeshBuilder::new();
        octa_blob(&mut m, Vec3::ZERO, Vec3::ONE, Color::WHITE, 0.4);
        for t in m.idx.chunks(3) {
            let p: Vec<Vec3> = t.iter().map(|&i| Vec3::from(m.pos[i as usize])).collect();
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(n.dot(p[0] + p[1] + p[2]) > 0.0, "octa face inward");
        }
        let mut m = MeshBuilder::new();
        round_tree(&mut m, Vec3::ZERO, 1.0, Color::WHITE, 0.0);
        pine_tree(&mut m, Vec3::ZERO, 1.0, Color::WHITE, 0.0);
        bush(&mut m, Vec3::ZERO, 1.0, Color::WHITE, 0.0);
        crate_box(&mut m, Vec3::ZERO, 1.0, 0.4);
        assert!(m.pos.iter().all(|p| p[1] >= -1e-4), "below ground");
    }
}
