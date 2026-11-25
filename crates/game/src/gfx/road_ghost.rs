//! The road tool's preview: the road that would be built, drawn with the
//! real road geometry (carriageway, kerbs, sidewalks, lane markings) along
//! each planned leg.

use bevy::prelude::*;
use citysim_sim::{Cell, RoadKind, RoadNetwork};

use super::mesh_util::MeshBuilder;
use super::road_geom;

/// One planned straight leg.
pub struct GhostLeg {
    pub from: Cell,
    pub to: Cell,
    pub valid: bool,
}

/// The preview meshes: (road, outline of the offending legs).
pub struct GhostMeshes {
    pub road: MeshBuilder,
    /// Thin outline along the path's outer edge (valid legs).
    pub outline: MeshBuilder,
    /// Outline and soft fill of legs the sim would refuse.
    pub bad: MeshBuilder,
}

/// Mesh the planned legs as the road they will be (each leg on its own,
/// as a straight curve between cell centres) plus outlines.
pub fn ghost_meshes(_roads: &RoadNetwork, legs: &[GhostLeg], kind: RoadKind) -> GhostMeshes {
    let mut road = MeshBuilder::new();
    for leg in legs {
        if leg.from == leg.to {
            continue;
        }
        let curve = citysim_sim::Curve::straight(leg.from.center(), leg.to.center());
        road_geom::mesh_segment_with(&mut road, &road_geom::SegLayout::bare(&curve, kind), false);
    }
    road.translate(Vec3::Y * 0.04);
    // Outlines along the outer edge of every leg (not across junctions).
    let outer = road_geom::style(kind).outer();
    let mut outline = MeshBuilder::new();
    let mut bad = MeshBuilder::new();
    let y = road_geom::WALK_Y + 0.08;
    for leg in legs {
        let (a, b) = (Vec2::from(leg.from.center()), Vec2::from(leg.to.center()));
        let d = (b - a).normalize_or_zero();
        if d == Vec2::ZERO {
            continue;
        }
        let n = Vec2::new(-d.y, d.x);
        let line = |m: &mut MeshBuilder, off: f32, w: f32, c: [f32; 4]| {
            let (p, q) = (a + n * off, b + n * off);
            let h = n * w * 0.5;
            m.quad_up([p - h, q - h, q + h, p + h].map(|v| Vec3::new(v.x, y, v.y)), c);
        };
        if leg.valid {
            for s in [1.0f32, -1.0] {
                line(&mut outline, s * (outer - 0.15), 0.32, [1.0, 1.0, 1.0, 0.95]);
            }
        } else {
            for s in [1.0f32, -1.0] {
                line(&mut bad, s * (outer - 0.15), 0.3, [1.0, 1.0, 1.0, 1.0]);
            }
            let h = n * outer;
            bad.quad_up(
                [a - h, b - h, b + h, a + h].map(|v| Vec3::new(v.x, y - 0.01, v.y)),
                [1.0, 1.0, 1.0, 0.18],
            );
        }
    }
    GhostMeshes { road, outline, bad }
}
