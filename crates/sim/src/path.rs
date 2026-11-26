//! A* over the road graph. Edge cost = expected travel time in seconds at
//! the current loads, so routes avoid jams and use new roads immediately.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use slotmap::Key;

use crate::derived::Access;
use crate::idmap::key_index;
use crate::roads::{NodeId, RoadNetwork, RoadSegment, SegmentId};
use crate::{balance, geom};

/// Reusable search buffers (indexed by node slot; generation-stamped so
/// they never need clearing).
#[derive(Clone, Default)]
pub(crate) struct PathScratch {
    g: Vec<f32>,
    prev: Vec<Option<(SegmentId, NodeId)>>,
    seen: Vec<u32>,
    done: Vec<u32>,
    generation: u32,
    heap: BinaryHeap<Reverse<(u32, u32, NodeId)>>,
}

/// Where a search ends.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Target {
    Node(NodeId),
    /// A point inside a segment; the route ends with that segment.
    Access(Access),
}

pub(crate) struct Found {
    /// Source node the route starts from.
    pub first: NodeId,
    /// Segments to drive after reaching `first`; for an `Access` target the
    /// last one is the target segment.
    pub segments: Vec<SegmentId>,
    /// Last node passed (entry node of the target segment, or the target).
    pub last_node: NodeId,
}

/// Speed factor of the volume-delay curve at congestion `x` (0..=1).
pub(crate) fn vdf(x: f32) -> f32 {
    1.0 / (1.0 + balance::VDF_ALPHA * libm::powf(x.max(0.0), balance::VDF_BETA))
}

/// Seconds to drive `dist` metres along `seg` in direction `forward`.
pub(crate) fn drive_time(seg: &RoadSegment, forward: bool, dist: f32) -> f32 {
    dist / (seg.speed_limit * vdf(seg.congestion_in(forward)))
}

/// Expected seconds to traverse `s` in direction `forward`, including the
/// junction at its far end. `delays[slot][dir]` is the expected wait at
/// that junction for this approach (its control); without an entry a flat
/// `ROUTE_JUNCTION_COST_S`.
pub(crate) fn edge_cost(roads: &RoadNetwork, delays: &[[f32; 2]], s: SegmentId, forward: bool) -> f32 {
    let seg = &roads.segments[s];
    let mut t = drive_time(seg, forward, seg.length);
    let end = if forward { seg.b } else { seg.a };
    if roads.nodes[end].segments.len() >= 3 {
        let d = RoadSegment::dir(forward);
        let wait = delays
            .get(key_index(s))
            .map_or(balance::ROUTE_JUNCTION_COST_S, |w| w[d]);
        t += wait + seg.load[d] * seg.congestion_in(forward) * balance::ROUTE_QUEUE_COST_S;
    }
    t
}

const TARGET_SLOT: u32 = u32::MAX;

impl PathScratch {
    fn prepare(&mut self, n: usize) {
        if self.g.len() < n {
            self.g.resize(n, 0.0);
            self.prev.resize(n, None);
            self.seen.resize(n, 0);
            self.done.resize(n, 0);
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.seen.iter_mut().for_each(|x| *x = 0);
            self.done.iter_mut().for_each(|x| *x = 0);
            self.generation = 1;
        }
        self.heap.clear();
    }

    fn relax(&mut self, n: NodeId, cost: f32, h: f32, prev: Option<(SegmentId, NodeId)>) {
        let i = key_index(n);
        if self.seen[i] != self.generation || cost < self.g[i] {
            self.seen[i] = self.generation;
            self.g[i] = cost;
            self.prev[i] = prev;
            self.heap.push(Reverse(((cost + h).to_bits(), i as u32, n)));
        }
    }

    /// Cheapest route from any of `sources` (node, cost to reach it) to
    /// `target`. Each node expansion costs one unit of `budget` (the search
    /// always completes; the budget only limits how many searches a caller
    /// starts).
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        &mut self,
        roads: &RoadNetwork,
        delays: &[[f32; 2]],
        turns: &[Vec<f32>],
        sources: &[(NodeId, f32)],
        target: Target,
        budget: &mut i64,
    ) -> Option<Found> {
        let (goal_pos, end_seg) = match target {
            Target::Node(n) => (roads.nodes.get(n)?.pos, None),
            Target::Access(a) => {
                let seg = roads.segments.get(a.segment)?;
                (crate::derived::access_pos(roads, a)?, Some((a, seg.a, seg.b)))
            }
        };
        let max_speed = balance::HIGHWAY_SPEED_LIMIT.max(balance::STREET_SPEED_LIMIT);
        let h = |p: [f32; 2]| geom::dist(p, goal_pos) / max_speed;
        self.prepare(roads.nodes.slot_count());
        for &(n, c) in sources {
            if roads.nodes.contains_key(n) {
                self.relax(n, c, h(roads.nodes[n].pos), None);
            }
        }
        let mut best_target = f32::INFINITY;
        let mut via: Option<NodeId> = None;
        while let Some(Reverse((fbits, slot, u))) = self.heap.pop() {
            if slot == TARGET_SLOT {
                if fbits == best_target.to_bits() {
                    break;
                }
                continue;
            }
            let ui = slot as usize;
            if self.done[ui] == self.generation {
                continue;
            }
            self.done[ui] = self.generation;
            *budget -= 1;
            let gu = self.g[ui];
            match target {
                Target::Node(n) if n == u => {
                    via = Some(u);
                    break;
                }
                Target::Node(_) => {}
                Target::Access(_) => {
                    let (a, na, nb) = end_seg.expect("access target");
                    let seg = &roads.segments[a.segment];
                    for (end, fwd, dist) in [(na, true, a.from_a), (nb, false, seg.length - a.from_a)] {
                        if end == u {
                            let t = gu + drive_time(seg, fwd, dist.max(0.0));
                            if t < best_target {
                                best_target = t;
                                via = Some(u);
                                self.heap.push(Reverse((t.to_bits(), TARGET_SLOT, NodeId::null())));
                            }
                        }
                    }
                }
            }
            let came_by = self.prev[ui].map(|p| p.0);
            let segs = &roads.nodes[u].segments;
            let deg = segs.len();
            let table = turns.get(ui).filter(|t| t.len() == deg * deg);
            let from_pos = came_by.and_then(|c| segs.iter().position(|&x| x == c));
            for (po, &s) in segs.iter().enumerate() {
                let seg = &roads.segments[s];
                let fwd = seg.a == u;
                let v = if fwd { seg.b } else { seg.a };
                let mut c = gu + edge_cost(roads, delays, s, fwd);
                if came_by == Some(s) {
                    // Turning back where we came from.
                    c += balance::ROUTE_UTURN_COST_S;
                }
                if let (Some(t), Some(pi)) = (table, from_pos) {
                    c += t[pi * deg + po];
                }
                self.relax(v, c, h(roads.nodes[v].pos), Some((s, u)));
            }
        }
        let last_node = via?;
        let mut segments = Vec::new();
        let mut node = last_node;
        while let Some((s, p)) = self.prev[key_index(node)] {
            segments.push(s);
            node = p;
            if segments.len() > roads.segments.len() {
                // Back-pointers form a cycle: only possible with negative
                // edge costs (a corrupt speed limit). No usable route.
                return None;
            }
        }
        segments.reverse();
        if let Target::Access(a) = target {
            segments.push(a.segment);
        }
        Some(Found {
            first: node,
            segments,
            last_node,
        })
    }
}
