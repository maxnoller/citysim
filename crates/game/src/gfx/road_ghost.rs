//! The road tool's preview: the road that would be built, drawn with the
//! real road geometry (carriageway, kerbs, sidewalks, lane markings and the
//! junction surfaces it makes with existing roads) on a copy of the road
//! graph with the planned legs added, so corners, joins and junction
//! footprints sit exactly where the built road will have them.

use std::collections::HashSet;

use bevy::prelude::*;
use citysim_sim::{Cell, NodeId, RoadKind, RoadNetwork, SegmentId};

use super::mesh_util::MeshBuilder;
use super::road_geom;

/// One planned straight leg.
pub struct GhostLeg {
    pub from: Cell,
    pub to: Cell,
    pub valid: bool,
}

/// Cells of a straight or 45 degree leg, in order.
pub fn leg_cells(from: Cell, to: Cell) -> Vec<Cell> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let n = dx.abs().max(dy.abs());
    if n == 0 {
        return vec![from];
    }
    // Not straight: just the ends (the ui only sends straight legs).
    if dx != 0 && dy != 0 && dx.abs() != dy.abs() {
        return vec![from, to];
    }
    (0..=n)
        .map(|k| Cell::new(from.x + dx.signum() * k, from.y + dy.signum() * k))
        .collect()
}

/// The preview meshes: (road, outline of the offending legs).
pub struct GhostMeshes {
    pub road: MeshBuilder,
    /// Thin outline along the path's outer edge (valid legs).
    pub outline: MeshBuilder,
    /// Outline and soft fill of legs the sim would refuse.
    pub bad: MeshBuilder,
}

/// Add the legs to a copy of `roads` the way the sim builds them (pieces
/// between cell centres, splitting crossed segments, upgrading pieces of a
/// different kind) and mesh everything on the path.
pub fn ghost_meshes(roads: &RoadNetwork, legs: &[GhostLeg], kind: RoadKind) -> GhostMeshes {
    let mut net = roads.clone();
    let mut path_cells: Vec<Cell> = Vec::new();
    for leg in legs {
        let cells = leg_cells(leg.from, leg.to);
        for w in cells.windows(2) {
            let (p, q) = (w[0], w[1]);
            let a = net.ensure_node(p.center());
            let b = net.ensure_node(q.center());
            if a == b {
                continue;
            }
            match net.segment_between(a, b) {
                Some(s) => {
                    if net.segments[s].kind != kind && net.segments[s].kind != RoadKind::Highway {
                        net.segments[s].set_kind(kind);
                    }
                }
                None => {
                    net.add_segment(a, b, kind);
                }
            }
        }
        path_cells.extend(cells);
    }
    // Merge straight runs again, like the sim does after building.
    let nodes: Vec<NodeId> = path_cells.iter().filter_map(|c| net.node_at(c.center())).collect();
    for n in nodes {
        net.simplify_node(n);
    }
    let touched_nodes: HashSet<NodeId> = path_cells.iter().filter_map(|c| net.node_at(c.center())).collect();
    let mut touched_segs: HashSet<SegmentId> = HashSet::new();
    for leg in legs {
        let cells = leg_cells(leg.from, leg.to);
        for w in cells.windows(2) {
            if let Some(s) = net.piece(w[0], w[1]) {
                touched_segs.insert(s);
            }
        }
    }
    let layout = road_geom::layout(&net);
    let mut road = MeshBuilder::new();
    for n in &layout.nodes {
        if touched_nodes.contains(&n.id) && !n.entry {
            road_geom::mesh_node(&mut road, n);
        }
    }
    for s in &layout.segs {
        if s.id.is_some_and(|id| touched_segs.contains(&id)) {
            road_geom::mesh_segment_with(&mut road, s, false);
        }
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
                line(&mut outline, s * (outer - 0.1), 0.2, [1.0, 1.0, 1.0, 0.9]);
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
