//! Planning road edits on the graph: building a road along any curve
//! (joins, T-splits, crossings, collinear upgrades), changing the kind of a
//! stretch, removing parts of roads. Pure functions of the world; the plan
//! is executed by `command::execute`.

use crate::command::{CommandError, NodeChange, PieceChange, PlannedNode, PlannedPiece};
use crate::curve::{self, Curve};
use crate::roads::{NodeId, RoadKind, RoadNetwork, SegmentId};
use crate::{BuildingId, MAP_SIZE_M, World, balance, geom};

/// What sits at a stop along a new road.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StopNode {
    Existing(NodeId),
    /// Made by splitting this segment at this distance from its `a`.
    Split(SegmentId, f32),
    New,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Stop {
    /// Metres along the new curve.
    pub d: f32,
    pub pos: [f32; 2],
    pub node: StopNode,
    pub change: Option<NodeChange>,
}

/// One piece between two consecutive stops.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Piece {
    New(Curve),
    /// Along an existing straight segment (collinear): change its kind.
    Existing {
        old: RoadKind,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct RoadPlan {
    pub kind: RoadKind,
    pub stops: Vec<Stop>,
    /// `pieces[k]` runs from `stops[k]` to `stops[k + 1]`.
    pub pieces: Vec<Piece>,
    pub cost: i64,
    pub new_m: f32,
    pub upgraded_m: f32,
    pub downgraded_m: f32,
}

impl RoadPlan {
    /// Existing segments to split, with distances from `a` and positions.
    pub(crate) fn splits(&self) -> Vec<(SegmentId, f32, [f32; 2])> {
        let mut out: Vec<(SegmentId, f32, [f32; 2])> = self
            .stops
            .iter()
            .filter_map(|s| match s.node {
                StopNode::Split(seg, d) => Some((seg, d, s.pos)),
                _ => None,
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        out.dedup_by(|a, b| a.0 == b.0 && (a.1 - b.1).abs() < 1e-3);
        out
    }

    pub(crate) fn preview_pieces(&self, world: &World) -> Vec<PlannedPiece> {
        let mut out = Vec::new();
        for (k, p) in self.pieces.iter().enumerate() {
            let (a, b) = (self.stops[k].pos, self.stops[k + 1].pos);
            match *p {
                Piece::New(c) => out.push(PlannedPiece {
                    curve: c,
                    kind: self.kind,
                    change: PieceChange::New,
                }),
                Piece::Existing { old } if old != self.kind && old != RoadKind::Highway => {
                    let _ = world;
                    out.push(PlannedPiece {
                        curve: Curve::straight(a, b),
                        kind: self.kind,
                        change: if self.kind.cost_per_cell() >= old.cost_per_cell() {
                            PieceChange::Upgrade
                        } else {
                            PieceChange::Downgrade
                        },
                    })
                }
                Piece::Existing { .. } => {}
            }
        }
        out
    }

    pub(crate) fn preview_nodes(&self) -> Vec<PlannedNode> {
        self.stops
            .iter()
            .filter_map(|s| s.change.map(|change| PlannedNode { pos: s.pos, change }))
            .collect()
    }
}

/// Price of `m` metres of a kind that costs `per_cell` per cell.
pub(crate) fn price(m: f32, per_cell: i64) -> i64 {
    (m as f64 * per_cell as f64 / crate::CELL_SIZE as f64).round() as i64
}

fn in_map(p: [f32; 2]) -> bool {
    (0.0..MAP_SIZE_M).contains(&p[0]) && (0.0..MAP_SIZE_M).contains(&p[1])
}

/// Segments whose bounds come within `margin` of the box `lo..hi`.
fn near_box(roads: &RoadNetwork, lo: [f32; 2], hi: [f32; 2], margin: f32) -> Vec<SegmentId> {
    roads
        .segments
        .iter()
        .filter(|(_, s)| {
            let (a, b) = s.curve.bounds();
            a[0] <= hi[0] + margin && b[0] >= lo[0] - margin && a[1] <= hi[1] + margin && b[1] >= lo[1] - margin
        })
        .map(|(id, _)| id)
        .collect()
}

/// Smallest angle between two unit directions, degrees.
fn angle_deg(a: [f32; 2], b: [f32; 2]) -> f32 {
    curve::angle_between(a, b).abs().to_degrees()
}

/// Distance from a point to a convex polygon (0 inside).
fn point_polygon_dist(p: [f32; 2], poly: &[[f32; 2]; 4]) -> f32 {
    if crate::zoning::in_quad(poly, p) {
        return 0.0;
    }
    let mut best = f32::INFINITY;
    for k in 0..4 {
        best = best.min(point_seg_dist(p, poly[k], poly[(k + 1) % 4]));
    }
    best
}

pub(crate) fn point_seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = geom::sub(b, a);
    let l2 = geom::dot(ab, ab);
    let t = if l2 > 0.0 {
        (geom::dot(geom::sub(p, a), ab) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    geom::dist(p, [a[0] + ab[0] * t, a[1] + ab[1] * t])
}

/// Smallest distance between segment `a0-a1` and segment `b0-b1`.
fn seg_seg_dist(a0: [f32; 2], a1: [f32; 2], b0: [f32; 2], b1: [f32; 2]) -> f32 {
    if curve::segments_meet(a0, a1, b0, b1).is_some() {
        return 0.0;
    }
    point_seg_dist(a0, b0, b1)
        .min(point_seg_dist(a1, b0, b1))
        .min(point_seg_dist(b0, a0, a1))
        .min(point_seg_dist(b1, a0, a1))
}

/// The building whose footprint comes closer than `reach` to the curve,
/// with the curve point nearest to it.
pub(crate) fn building_near(world: &World, c: &Curve, reach: f32) -> Option<(BuildingId, [f32; 2])> {
    let (lo, hi) = c.bounds();
    let pts = c.flatten(0.05);
    let mut best: Option<(f32, BuildingId, [f32; 2])> = None;
    for (id, b) in world.buildings.iter() {
        let poly = b.corners();
        let blo = poly
            .iter()
            .fold([f32::INFINITY; 2], |a, p| [a[0].min(p[0]), a[1].min(p[1])]);
        let bhi = poly
            .iter()
            .fold([f32::NEG_INFINITY; 2], |a, p| [a[0].max(p[0]), a[1].max(p[1])]);
        if blo[0] > hi[0] + reach || bhi[0] < lo[0] - reach || blo[1] > hi[1] + reach || bhi[1] < lo[1] - reach {
            continue;
        }
        for w in pts.windows(2) {
            let mut d = f32::INFINITY;
            for k in 0..4 {
                d = d.min(seg_seg_dist(w[0], w[1], poly[k], poly[(k + 1) % 4]));
            }
            d = d.min(point_polygon_dist(w[0], &poly));
            if d < reach && best.is_none_or(|x| d < x.0) {
                // The curve point nearest the building's centre.
                let q = c.nearest(b.center()).2;
                best = Some((d, id, q));
            }
        }
    }
    best.map(|b| (b.1, b.2))
}

/// Plan a road of `kind` along `curve`. `shorthand`: the cell command
/// (`BuildRoad`), whose errors keep their old cell form.
pub(crate) fn plan_road(world: &World, curve: Curve, kind: RoadKind) -> Result<RoadPlan, CommandError> {
    let roads = &world.roads;
    if !RoadKind::BUILDABLE.contains(&kind) {
        return Err(CommandError::InvalidRoadKind(kind));
    }
    let mut curve = curve;
    for p in curve.p {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err(CommandError::OffMap { at: [0.0, 0.0] });
        }
    }
    // Snap the ends: onto a node, else onto a road (T).
    let mut ends: [StopNode; 2] = [StopNode::New, StopNode::New];
    let mut snapped = [curve.start(), curve.end()];
    for k in 0..2 {
        let p = snapped[k];
        let node = roads
            .nodes
            .iter()
            .filter(|(_, n)| geom::dist(n.pos, p) <= balance::SNAP_NODE_M)
            .min_by(|a, b| {
                geom::dist(a.1.pos, p)
                    .total_cmp(&geom::dist(b.1.pos, p))
                    .then(a.0.cmp(&b.0))
            });
        if let Some((n, node)) = node {
            ends[k] = StopNode::Existing(n);
            snapped[k] = node.pos;
        } else if let Some((s, d, dist)) = roads.nearest_segment(p, balance::SNAP_ROAD_M) {
            let _ = dist;
            ends[k] = StopNode::Split(s, d);
            snapped[k] = roads.segments[s].pose(d, 0.0).pos;
        }
    }
    curve = curve.with_ends(snapped[0], snapped[1]);
    let flat = curve.flatten(0.1);
    if let Some(&p) = flat.iter().find(|&&p| !in_map(p)) {
        return Err(CommandError::OffMap { at: p });
    }
    let len = curve.length();
    if len < balance::MIN_SEGMENT_M {
        return Err(CommandError::TooShort {
            at: curve.start(),
            length: len,
            min: balance::MIN_SEGMENT_M,
        });
    }
    if let (StopNode::Existing(a), StopNode::Existing(b)) = (ends[0], ends[1])
        && a == b
    {
        return Err(CommandError::TooShort {
            at: curve.start(),
            length: 0.0,
            min: balance::MIN_SEGMENT_M,
        });
    }
    if let Some(at) = curve.self_intersection() {
        return Err(CommandError::SelfCrossing { at });
    }
    let (radius, rd) = curve.min_radius();
    if radius < kind.min_radius_m() {
        return Err(CommandError::TooTight {
            at: curve.pose_at(rd, 0.0).pos,
            radius,
            min: kind.min_radius_m(),
        });
    }
    let straight = curve.is_straight();
    let (lo, hi) = curve.bounds();
    let cands = near_box(roads, lo, hi, balance::MIN_ROAD_SPACING_M + 1.0);

    // Stops: ends, existing nodes on the curve, crossings.
    let mut stops: Vec<Stop> = Vec::new();
    for (k, &e) in ends.iter().enumerate() {
        let d = if k == 0 { 0.0 } else { len };
        let change = match e {
            StopNode::Existing(_) => NodeChange::Join,
            StopNode::Split(..) => NodeChange::Split,
            StopNode::New => NodeChange::End,
        };
        stops.push(Stop {
            d,
            pos: snapped[k],
            node: e,
            change: Some(change),
        });
    }
    // Existing straight segments collinear with a straight new road: the
    // overlap is an upgrade / downgrade, not a conflict. (d0, d1, seg).
    let mut overlaps: Vec<(f32, f32, SegmentId)> = Vec::new();
    if straight {
        let dir = curve::unit(geom::sub(curve.end(), curve.start()));
        for &s in &cands {
            let seg = &roads.segments[s];
            if !seg.curve.is_straight() {
                continue;
            }
            let (pa, pb) = (seg.curve.start(), seg.curve.end());
            let off = |p: [f32; 2]| geom::cross(dir, geom::sub(p, curve.start())).abs();
            if off(pa) > 0.01 || off(pb) > 0.01 {
                continue;
            }
            let (ta, tb) = (
                geom::dot(dir, geom::sub(pa, curve.start())),
                geom::dot(dir, geom::sub(pb, curve.start())),
            );
            let (t0, t1) = (ta.min(tb).max(0.0), ta.max(tb).min(len));
            if t1 - t0 > 0.01 {
                overlaps.push((t0, t1, s));
            }
        }
        overlaps.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    let in_overlap = |d: f32| overlaps.iter().any(|&(a, b, _)| d > a + 0.01 && d < b - 0.01);
    let overlap_seg = |s: SegmentId| overlaps.iter().any(|&(_, _, o)| o == s);
    // Overlap ends inside an existing segment split it.
    for &(t0, t1, s) in &overlaps {
        let seg = &roads.segments[s];
        for t in [t0, t1] {
            let p = curve.pose_at(t, 0.0).pos;
            if roads.node_at(p).is_none() && stops.iter().all(|st| geom::dist(st.pos, p) > 0.01) {
                let (da, _, _) = seg.nearest(p);
                stops.push(Stop {
                    d: t,
                    pos: p,
                    node: StopNode::Split(s, da),
                    change: None,
                });
            }
        }
    }
    // Existing nodes on the curve.
    for (n, node) in roads.nodes.iter() {
        if stops.iter().any(|st| st.node == StopNode::Existing(n)) {
            continue;
        }
        let p = node.pos;
        if p[0] < lo[0] - 0.1 || p[0] > hi[0] + 0.1 || p[1] < lo[1] - 0.1 || p[1] > hi[1] + 0.1 {
            continue;
        }
        let (d, dist, _) = curve.nearest(p);
        if dist < 0.02 {
            stops.push(Stop {
                d,
                pos: p,
                node: StopNode::Existing(n),
                change: (!in_overlap(d)).then_some(NodeChange::Join),
            });
        }
    }
    // Crossings with other segments.
    for &s in &cands {
        if overlap_seg(s) {
            continue;
        }
        let seg = &roads.segments[s];
        for (d_new, d_old, p) in curve.intersections(&seg.curve) {
            if stops.iter().any(|st| geom::dist(st.pos, p) < 0.5) {
                continue;
            }
            if d_old < balance::MIN_SEGMENT_M || d_old > seg.length - balance::MIN_SEGMENT_M {
                return Err(CommandError::TooShort {
                    at: p,
                    length: d_old.min(seg.length - d_old),
                    min: balance::MIN_SEGMENT_M,
                });
            }
            // A crossing exactly at a new end was snapped as a T already.
            stops.push(Stop {
                d: d_new,
                pos: p,
                node: StopNode::Split(s, d_old),
                change: Some(NodeChange::Crossing),
            });
        }
    }
    stops.sort_by(|a, b| a.d.total_cmp(&b.d));
    // T splits must leave pieces long enough on the split road.
    for st in &stops {
        if let StopNode::Split(s, d) = st.node {
            let l = roads.segments[s].length;
            if d < balance::MIN_SEGMENT_M - 0.01 || d > l - balance::MIN_SEGMENT_M + 0.01 {
                return Err(CommandError::TooShort {
                    at: st.pos,
                    length: d.min(l - d),
                    min: balance::MIN_SEGMENT_M,
                });
            }
        }
    }
    // Pieces between stops.
    let mut pieces = Vec::new();
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mid = (a.d + b.d) * 0.5;
        if let Some(&(_, _, s)) = overlaps.iter().find(|&&(t0, t1, _)| mid > t0 && mid < t1) {
            pieces.push(Piece::Existing {
                old: roads.segments[s].kind,
            });
            continue;
        }
        if b.d - a.d < balance::MIN_SEGMENT_M - 0.01 {
            return Err(CommandError::TooShort {
                at: a.pos,
                length: b.d - a.d,
                min: balance::MIN_SEGMENT_M,
            });
        }
        pieces.push(Piece::New(curve.sub(a.d, b.d).with_ends(a.pos, b.pos)));
    }
    // A new piece between two nodes some road will already join (a curve
    // beside a straight between the same junctions, a road crossing another
    // twice) gets a node in its middle: the graph keeps one segment per pair
    // of nodes. Node keys: existing nodes, or the stop that makes the node.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Key {
        Node(NodeId),
        Stop(usize),
    }
    let key_of = |k: usize, st: &Stop| match st.node {
        StopNode::Existing(n) => Key::Node(n),
        _ => Key::Stop(k),
    };
    let mut joined: Vec<(Key, Key)> = Vec::new();
    let mut split_segs: Vec<SegmentId> = stops
        .iter()
        .filter_map(|st| match st.node {
            StopNode::Split(sg, _) => Some(sg),
            _ => None,
        })
        .collect();
    split_segs.sort();
    split_segs.dedup();
    for &sg in &split_segs {
        let seg = &roads.segments[sg];
        let mut pts: Vec<(f32, Key)> = vec![(0.0, Key::Node(seg.a)), (seg.length, Key::Node(seg.b))];
        for (k, st) in stops.iter().enumerate() {
            if let StopNode::Split(s2, d) = st.node
                && s2 == sg
            {
                pts.push((d, key_of(k, st)));
            }
        }
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        for w in pts.windows(2) {
            joined.push((w[0].1.min(w[1].1), w[0].1.max(w[1].1)));
        }
    }
    fn joined_now(roads: &RoadNetwork, joined: &[(Key, Key)], split: &[SegmentId], a: Key, b: Key) -> bool {
        let (a, b) = (a.min(b), a.max(b));
        joined.contains(&(a, b))
            || match (a, b) {
                (Key::Node(x), Key::Node(y)) => roads.segment_between(x, y).is_some_and(|sg| !split.contains(&sg)),
                _ => false,
            }
    }
    let mut k = 0;
    while k < pieces.len() {
        if let Piece::New(c) = pieces[k]
            && joined_now(
                roads,
                &joined,
                &split_segs,
                key_of(k, &stops[k]),
                key_of(k + 1, &stops[k + 1]),
            )
        {
            let l = c.length();
            let mid = c.pose_at(l * 0.5, 0.0).pos;
            if l < 2.0 * balance::MIN_SEGMENT_M {
                return Err(CommandError::TooClose { at: mid });
            }
            let (c1, c2) = c.split_at(l * 0.5);
            // Later stops shift by one: their keys must not change meaning.
            for (a, b) in joined.iter_mut() {
                for x in [a, b] {
                    if let Key::Stop(i) = x
                        && *i > k
                    {
                        *i += 1;
                    }
                }
            }
            stops.insert(
                k + 1,
                Stop {
                    d: (stops[k].d + stops[k + 1].d) * 0.5,
                    pos: c1.end(),
                    node: StopNode::New,
                    change: None,
                },
            );
            pieces[k] = Piece::New(c1);
            pieces.insert(k + 1, Piece::New(c2));
            k += 2;
            continue;
        }
        k += 1;
    }
    // Arm angles at every stop the new road uses.
    let mut allow: Vec<([f32; 2], f32)> = Vec::new();
    for (k, st) in stops.iter().enumerate() {
        let mut old: Vec<[f32; 2]> = Vec::new();
        match st.node {
            StopNode::Existing(n) => {
                for &s in &roads.nodes[n].segments {
                    old.push(roads.segments[s].dir_from(n));
                }
            }
            StopNode::Split(s, d) => {
                let t = roads.segments[s].pose(d, 0.0).dir;
                old.push(t);
                old.push([-t[0], -t[1]]);
            }
            StopNode::New => {}
        }
        let mut new: Vec<[f32; 2]> = Vec::new();
        if k > 0 && matches!(pieces[k - 1], Piece::New(_)) {
            let t = curve.pose_at(st.d, 0.0).dir;
            new.push([-t[0], -t[1]]);
        }
        if k < pieces.len() && matches!(pieces[k], Piece::New(_)) {
            new.push(curve.pose_at(st.d, 0.0).dir);
        }
        if new.is_empty() {
            if st.node != StopNode::New {
                allow.push((st.pos, 2.0 * balance::MIN_ROAD_SPACING_M + 1.0));
            }
            continue;
        }
        let total = old.len() + new.len();
        if total > balance::MAX_NODE_ARMS && st.node != StopNode::New {
            return Err(CommandError::TooManyArms {
                at: st.pos,
                arms: total as u8,
            });
        }
        let mut min_angle: f32 = 180.0;
        for (i, &a) in new.iter().enumerate() {
            for &b in old.iter().chain(new[i + 1..].iter()) {
                min_angle = min_angle.min(angle_deg(a, b));
            }
        }
        if min_angle < balance::MIN_ARM_ANGLE_DEG {
            return Err(CommandError::SharpAngle {
                at: st.pos,
                degrees: min_angle,
                min: balance::MIN_ARM_ANGLE_DEG,
            });
        }
        if st.node != StopNode::New {
            let sin = libm::sinf(min_angle.min(90.0).to_radians()).max(0.2);
            allow.push((st.pos, balance::MIN_ROAD_SPACING_M / sin + 1.0));
        }
    }
    // Running along or too close beside other roads.
    let spacing = balance::MIN_ROAD_SPACING_M - 0.05;
    let others: Vec<SegmentId> = cands.iter().copied().filter(|&s| !overlap_seg(s)).collect();
    if !others.is_empty() {
        let n = (len / 1.0).ceil().max(1.0) as usize;
        for k in 0..=n {
            let d = len * k as f32 / n as f32;
            if in_overlap(d) {
                continue;
            }
            let p = curve.pose_at(d, 0.0).pos;
            if allow.iter().any(|&(q, r)| geom::dist(p, q) < r) {
                continue;
            }
            for &s in &others {
                let seg = &roads.segments[s];
                let (blo, bhi) = seg.curve.bounds();
                if p[0] < blo[0] - spacing
                    || p[0] > bhi[0] + spacing
                    || p[1] < blo[1] - spacing
                    || p[1] > bhi[1] + spacing
                {
                    continue;
                }
                if seg.nearest(p).1 < spacing {
                    return Err(CommandError::TooClose { at: p });
                }
            }
        }
    }
    // Buildings in the way of the new pieces.
    for p in &pieces {
        if let Piece::New(c) = p
            && let Some((building, at)) = building_near(world, c, kind.reserve_half_width_m() - 0.05)
        {
            return Err(CommandError::BuildingInTheWay { at, building });
        }
    }
    let mut new_m = 0.0;
    let mut up = 0.0;
    let mut down = 0.0;
    let mut cost = 0;
    let mut change_cost = 0.0f64;
    for (k, p) in pieces.iter().enumerate() {
        let l = stops[k + 1].d - stops[k].d;
        match *p {
            Piece::New(c) => new_m += c.length(),
            Piece::Existing { old } if old != kind && old != RoadKind::Highway => {
                let diff = kind.cost_per_cell() - old.cost_per_cell();
                if diff > 0 {
                    up += l;
                    change_cost += l as f64 * diff as f64 / crate::CELL_SIZE as f64;
                } else {
                    down += l;
                }
            }
            Piece::Existing { .. } => {}
        }
    }
    cost += price(new_m, kind.cost_per_cell()) + change_cost.round() as i64;
    Ok(RoadPlan {
        kind,
        stops,
        pieces,
        cost,
        new_m,
        upgraded_m: up,
        downgraded_m: down,
    })
}

/// Build a planned road.
pub(crate) fn execute_road(world: &mut World, plan: &RoadPlan) {
    let roads = &mut world.roads;
    for (s, d, p) in plan.splits() {
        if roads.segments.contains_key(s) && roads.node_at(p).is_none() {
            roads.split_at(s, d, Some(p));
        }
    }
    let nodes: Vec<NodeId> = plan
        .stops
        .iter()
        .map(|st| match st.node {
            StopNode::Existing(n) => n,
            _ => roads.node_at(st.pos).unwrap_or_else(|| roads.add_node(st.pos)),
        })
        .collect();
    for (k, p) in plan.pieces.iter().enumerate() {
        let (a, b) = (nodes[k], nodes[k + 1]);
        match *p {
            Piece::New(c) => {
                roads.add_curve(a, b, plan.kind, c);
            }
            Piece::Existing { .. } => {
                if let Some(s) = roads.segment_between(a, b)
                    && roads.segments[s].kind != RoadKind::Highway
                    && roads.segments[s].kind != plan.kind
                {
                    roads.segments[s].set_kind(plan.kind);
                }
            }
        }
    }
    for &n in &nodes {
        roads.simplify_node(n);
    }
}

// ----------------------------------------------------------------- removal ----

/// A part of a segment: `from_a` metres `lo..hi`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Part {
    pub segment: SegmentId,
    pub lo: f32,
    pub hi: f32,
}

/// Normalise a requested part: clamped, snapped to the ends when within
/// half a metre, widened to an end when the rest would be shorter than
/// `MIN_SEGMENT_M`.
pub(crate) fn part(roads: &RoadNetwork, s: SegmentId, a: f32, b: f32) -> Option<Part> {
    let seg = roads.segments.get(s)?;
    let l = seg.length;
    let (mut lo, mut hi) = (a.min(b).clamp(0.0, l), a.max(b).clamp(0.0, l));
    if lo < balance::MIN_SEGMENT_M.min(l * 0.5) {
        lo = 0.0;
    }
    if l - hi < balance::MIN_SEGMENT_M.min(l * 0.5) {
        hi = l;
    }
    (hi - lo > 0.01).then_some(Part { segment: s, lo, hi })
}

/// Clip the segment `a`-`b` to a convex quad: the parameter range inside.
fn clip(a: [f32; 2], b: [f32; 2], quad: &[[f32; 2]; 4]) -> Option<(f32, f32)> {
    // Orientation of the quad.
    let area: f32 = (0..4).map(|k| geom::cross(quad[k], quad[(k + 1) % 4])).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let d = geom::sub(b, a);
    for k in 0..4 {
        let (p, q) = (quad[k], quad[(k + 1) % 4]);
        let e = geom::sub(q, p);
        // Inside: sign * cross(e, x - p) >= 0.
        let f0 = sign * geom::cross(e, geom::sub(a, p));
        let fd = sign * geom::cross(e, d);
        if fd.abs() < 1e-9 {
            if f0 < 0.0 {
                return None;
            }
            continue;
        }
        let t = -f0 / fd;
        if fd > 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

/// The parts of segment `s` whose centre line runs inside the convex quad.
/// `cells`: widen each part by half a cell along the road at both ends (the
/// cell shorthand removes whole cell-to-cell pieces with an end inside).
pub(crate) fn parts_in_quad(roads: &RoadNetwork, s: SegmentId, quad: &[[f32; 2]; 4], cells: bool) -> Vec<Part> {
    let seg = &roads.segments[s];
    let pts = seg.curve.flatten_offset(0.0, 0.05);
    let mut spans: Vec<(f32, f32)> = Vec::new();
    for w in pts.windows(2) {
        let ((d0, a), (d1, b)) = (w[0], w[1]);
        if let Some((t0, t1)) = clip(a, b, quad) {
            let (x0, x1) = (d0 + (d1 - d0) * t0, d0 + (d1 - d0) * t1);
            match spans.last_mut() {
                Some(last) if x0 <= last.1 + 1e-3 => last.1 = last.1.max(x1),
                _ => spans.push((x0, x1)),
            }
        }
    }
    let half = |d: f32| {
        let t = seg.pose(d, 0.0).dir;
        crate::CELL_SIZE * 0.5 / t[0].abs().max(t[1].abs()).max(0.5)
    };
    spans
        .into_iter()
        // Touching a corner or an edge is not running inside.
        .filter(|&(lo, hi)| hi - lo > 0.05)
        .filter_map(|(lo, hi)| {
            if cells {
                part(roads, s, lo - half(lo), hi + half(hi))
            } else {
                part(roads, s, lo, hi)
            }
        })
        .collect()
}

/// Remove parts of segments (any number per segment, not overlapping):
/// split where needed, drop the parts, tidy the nodes. Returns the removed
/// curves.
pub(crate) fn execute_remove(world: &mut World, parts: &[Part]) -> Vec<Curve> {
    let roads = &mut world.roads;
    let mut removed = Vec::new();
    let mut by_seg: Vec<Part> = parts.to_vec();
    by_seg.sort_by(|a, b| a.segment.cmp(&b.segment).then(b.lo.total_cmp(&a.lo)));
    let mut touched: Vec<NodeId> = Vec::new();
    let mut k = 0;
    while k < by_seg.len() {
        let s = by_seg[k].segment;
        let mut e = k;
        while e < by_seg.len() && by_seg[e].segment == s {
            e += 1;
        }
        // From the far end backwards, so `s` keeps its `a` side.
        for p in &by_seg[k..e] {
            let Some(seg) = roads.segments.get(s) else { break };
            let l = seg.length;
            let mut target = s;
            if p.hi < l - 0.01 {
                let n = roads.split_at(s, p.hi, None);
                touched.push(n);
            }
            if p.lo > 0.01 {
                let n = roads.split_at(s, p.lo, None);
                touched.push(n);
                target = roads.nodes[n]
                    .segments
                    .iter()
                    .copied()
                    .find(|&x| x != s)
                    .expect("split adds the far half");
            }
            let seg = &roads.segments[target];
            touched.push(seg.a);
            touched.push(seg.b);
            removed.push(seg.curve);
            roads.remove_segment(target);
        }
        k = e;
    }
    touched.sort();
    touched.dedup();
    for n in touched {
        roads.simplify_node(n);
    }
    removed
}

/// Change the kind of a part of a segment (split at interior ends).
pub(crate) fn execute_kind(world: &mut World, p: Part, kind: RoadKind) {
    let roads = &mut world.roads;
    let Some(seg) = roads.segments.get(p.segment) else {
        return;
    };
    let l = seg.length;
    let mut target = p.segment;
    let mut ends = Vec::new();
    if p.hi < l - 0.01 {
        ends.push(roads.split_at(p.segment, p.hi, None));
    }
    if p.lo > 0.01 {
        let n = roads.split_at(p.segment, p.lo, None);
        ends.push(n);
        target = roads.nodes[n]
            .segments
            .iter()
            .copied()
            .find(|&x| x != p.segment)
            .expect("split adds the far half");
    }
    if roads.segments[target].kind != RoadKind::Highway {
        roads.segments[target].set_kind(kind);
    }
    let seg = &roads.segments[target];
    ends.push(seg.a);
    ends.push(seg.b);
    for n in ends {
        roads.simplify_node(n);
    }
}
