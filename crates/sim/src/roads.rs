//! The road network: a general graph of nodes (metre positions) joined by
//! straight segments.
//!
//! Pathfinding, vehicles and rendering rely only on this graph, never on the
//! grid, so other build tools (free-form, curved) can be added later by only
//! changing how commands edit the graph. Segments are two-way.

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::IdMap;

use crate::balance;
use crate::geom::{self, Cell};

new_key_type! {
    /// Stable id of a [`RoadNode`].
    pub struct NodeId;
    /// Stable id of a [`RoadSegment`].
    pub struct SegmentId;
}

/// Road class. Variants are only ever appended (saves store the index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RoadKind {
    /// One lane each way, 50 km/h. About 8 m wide with sidewalks.
    Street,
    /// The pre-built stub connecting the map to the outside. Not bulldozable,
    /// two lanes each way.
    Highway,
    /// Two lanes each way, 60 km/h, much more capacity; costs more to build
    /// and keep. Drawn about 13 m wide but still occupies one line of cells.
    Avenue,
}

impl RoadKind {
    /// Kinds a player can build with `Command::BuildRoad`.
    pub const BUILDABLE: [RoadKind; 2] = [RoadKind::Street, RoadKind::Avenue];

    /// Metres per second.
    pub fn speed_limit(self) -> f32 {
        match self {
            RoadKind::Street => balance::STREET_SPEED_LIMIT,
            RoadKind::Highway => balance::HIGHWAY_SPEED_LIMIT,
            RoadKind::Avenue => balance::AVENUE_SPEED_LIMIT,
        }
    }

    /// Lanes per direction (as a float, for capacity maths).
    pub fn lanes(self) -> f32 {
        self.lane_count() as f32
    }

    /// Lanes per direction. Lane 0 is the kerb-side (right-hand) lane,
    /// higher indices are further left (towards the centre line).
    pub fn lane_count(self) -> u8 {
        match self {
            RoadKind::Street => balance::STREET_LANES,
            RoadKind::Highway => balance::HIGHWAY_LANES,
            RoadKind::Avenue => balance::AVENUE_LANES,
        }
    }

    /// Lateral offset of lane `lane`'s centre from the road's centre line,
    /// metres, to the right of travel (traffic drives on the right; in sim
    /// space "right" of a travel direction `d` is `(-d.y, d.x)`). Lanes
    /// beyond `lane_count` are clamped to the leftmost lane.
    pub fn lane_offset_m(self, lane: u8) -> f32 {
        let l = lane.min(self.lane_count().saturating_sub(1));
        match self {
            RoadKind::Street => 1.4,
            // 13 m: 0.5 m median strip each side of the centre, 3 m lanes.
            RoadKind::Avenue => {
                if l == 0 {
                    4.75
                } else {
                    1.75
                }
            }
            RoadKind::Highway => {
                if l == 0 {
                    3.6
                } else {
                    1.3
                }
            }
        }
    }

    /// Half the carriageway width (kerb to centre line), metres. Junction
    /// boxes and stop lines are laid out from these.
    pub fn half_width_m(self) -> f32 {
        match self {
            RoadKind::Street => 2.75,
            RoadKind::Avenue => 6.5,
            RoadKind::Highway => 5.0,
        }
    }

    /// Construction cost per cell (`Command::BuildRoad`).
    pub fn cost_per_cell(self) -> i64 {
        match self {
            RoadKind::Street => balance::ROAD_COST_PER_CELL,
            RoadKind::Avenue => balance::AVENUE_COST_PER_CELL,
            RoadKind::Highway => balance::AVENUE_COST_PER_CELL,
        }
    }

    /// Daily upkeep per cell.
    pub fn upkeep_per_cell(self) -> i64 {
        match self {
            RoadKind::Street => balance::ROAD_UPKEEP_PER_CELL_PER_DAY,
            RoadKind::Avenue => balance::AVENUE_UPKEEP_PER_CELL_PER_DAY,
            RoadKind::Highway => balance::ROAD_UPKEEP_PER_CELL_PER_DAY,
        }
    }

    /// Road class for priority rules: higher wins (Street 0, Avenue 1,
    /// Highway 2).
    pub fn rank(self) -> u8 {
        match self {
            RoadKind::Street => 0,
            RoadKind::Avenue => 1,
            RoadKind::Highway => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadNode {
    /// Position in sim metres.
    pub pos: [f32; 2],
    /// Segments attached to this node (its degree is `segments.len()`).
    pub segments: Vec<SegmentId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadSegment {
    pub a: NodeId,
    pub b: NodeId,
    pub kind: RoadKind,
    /// Metres. Straight for now: `dist(a.pos, b.pos)`.
    pub length: f32,
    /// Metres per second.
    pub speed_limit: f32,
    /// Vehicles that fit per direction before it is jammed, indexed by
    /// [`RoadSegment::dir`]: `[a -> b, b -> a]`.
    pub capacity: [f32; 2],
    /// Vehicles currently on the segment per direction, `[a -> b, b -> a]`.
    /// PHASE2: maintained by traffic.
    pub load: [f32; 2],
}

impl RoadSegment {
    /// A segment of `kind` between two nodes `length` metres apart.
    pub fn new(a: NodeId, b: NodeId, kind: RoadKind, length: f32, load: [f32; 2]) -> RoadSegment {
        RoadSegment {
            a,
            b,
            kind,
            length,
            speed_limit: kind.speed_limit(),
            capacity: capacity_for(kind, length),
            load,
        }
    }

    /// Change the kind in place (upgrade / downgrade), keeping length and
    /// load.
    pub fn set_kind(&mut self, kind: RoadKind) {
        self.kind = kind;
        self.speed_limit = kind.speed_limit();
        self.capacity = capacity_for(kind, self.length);
    }

    /// Lanes per direction.
    pub fn lane_count(&self) -> u8 {
        self.kind.lane_count()
    }

    pub fn other(&self, n: NodeId) -> NodeId {
        if self.a == n { self.b } else { self.a }
    }

    /// Index into `load` / `capacity`: 0 for travel `a -> b` (`forward`),
    /// 1 for `b -> a`.
    pub fn dir(forward: bool) -> usize {
        if forward { 0 } else { 1 }
    }

    /// `load / capacity` in one direction: 0 empty, 1 jammed.
    pub fn congestion_in(&self, forward: bool) -> f32 {
        let d = Self::dir(forward);
        if self.capacity[d] > 0.0 {
            (self.load[d] / self.capacity[d]).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The worse of the two directions.
    pub fn congestion(&self) -> f32 {
        self.congestion_in(true).max(self.congestion_in(false))
    }

    /// Both directions together.
    pub fn total_load(&self) -> f32 {
        self.load[0] + self.load[1]
    }
}

/// Per-direction capacity.
fn capacity_for(kind: RoadKind, length: f32) -> [f32; 2] {
    let c = (length * kind.lanes() / balance::VEHICLE_SPACING_M).max(1.0);
    [c, c]
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RoadNetwork {
    pub nodes: IdMap<NodeId, RoadNode>,
    pub segments: IdMap<SegmentId, RoadSegment>,
    /// Where newcomers arrive: the outer end of the highway stub.
    pub entry: Option<NodeId>,
}

impl RoadNetwork {
    pub fn new() -> RoadNetwork {
        RoadNetwork::default()
    }

    pub fn node_pos(&self, n: NodeId) -> [f32; 2] {
        self.nodes[n].pos
    }

    /// Both end positions of a segment.
    pub fn segment_ends(&self, s: SegmentId) -> ([f32; 2], [f32; 2]) {
        let seg = &self.segments[s];
        (self.nodes[seg.a].pos, self.nodes[seg.b].pos)
    }

    /// Node exactly at `pos`, if any.
    pub fn node_at(&self, pos: [f32; 2]) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|(_, n)| geom::same_pos(n.pos, pos))
            .map(|(id, _)| id)
    }

    /// Segment whose interior (not its ends) passes through `pos`.
    pub fn segment_through(&self, pos: [f32; 2]) -> Option<SegmentId> {
        self.segments.keys().find(|&s| {
            let (a, b) = self.segment_ends(s);
            geom::on_segment_interior(pos, a, b)
        })
    }

    /// Does any node or segment lie at `pos`?
    pub fn covers(&self, pos: [f32; 2]) -> bool {
        self.node_at(pos).is_some() || self.segment_through(pos).is_some()
    }

    /// A segment containing both points (closed). With no overlapping
    /// segments this is the road piece from `p` to `q`, if built.
    pub fn segment_containing(&self, p: [f32; 2], q: [f32; 2]) -> Option<SegmentId> {
        self.segments.keys().find(|&s| {
            let (a, b) = self.segment_ends(s);
            geom::on_segment(p, a, b) && geom::on_segment(q, a, b)
        })
    }

    /// Segment directly joining two nodes.
    pub fn segment_between(&self, a: NodeId, b: NodeId) -> Option<SegmentId> {
        self.nodes[a]
            .segments
            .iter()
            .copied()
            .find(|&s| self.segments[s].other(a) == b)
    }

    /// The segment carrying the road piece between two cell centres.
    pub fn piece(&self, p: Cell, q: Cell) -> Option<SegmentId> {
        self.segment_containing(p.center(), q.center())
    }

    pub fn add_node(&mut self, pos: [f32; 2]) -> NodeId {
        self.nodes.insert(RoadNode {
            pos,
            segments: Vec::new(),
        })
    }

    /// Join two distinct nodes with a straight segment.
    pub fn add_segment(&mut self, a: NodeId, b: NodeId, kind: RoadKind) -> SegmentId {
        assert_ne!(a, b, "zero-length segment");
        let length = geom::dist(self.nodes[a].pos, self.nodes[b].pos);
        let s = self.segments.insert(RoadSegment::new(a, b, kind, length, [0.0; 2]));
        self.nodes[a].segments.push(s);
        self.nodes[b].segments.push(s);
        s
    }

    /// Remove a segment; its nodes stay (call [`Self::simplify_node`]).
    pub fn remove_segment(&mut self, s: SegmentId) {
        if let Some(seg) = self.segments.remove(s) {
            for n in [seg.a, seg.b] {
                if let Some(node) = self.nodes.get_mut(n) {
                    node.segments.retain(|&x| x != s);
                }
            }
        }
    }

    /// The node at `pos`: an existing one, one made by splitting the segment
    /// passing through `pos`, or a new unconnected node.
    pub fn ensure_node(&mut self, pos: [f32; 2]) -> NodeId {
        if let Some(n) = self.node_at(pos) {
            return n;
        }
        if let Some(s) = self.segment_through(pos) {
            return self.split_segment(s, pos);
        }
        self.add_node(pos)
    }

    /// Split `s` at interior point `pos`. `s` keeps its id and becomes the
    /// `a`-side half; a new segment is the `b`-side half.
    pub fn split_segment(&mut self, s: SegmentId, pos: [f32; 2]) -> NodeId {
        let mid = self.add_node(pos);
        let seg = self.segments[s].clone();
        let len_a = geom::dist(self.nodes[seg.a].pos, pos);
        let len_b = geom::dist(pos, self.nodes[seg.b].pos);
        let frac_a = len_a / (len_a + len_b);
        {
            let first = &mut self.segments[s];
            first.b = mid;
            first.length = len_a;
            first.capacity = capacity_for(seg.kind, len_a);
            first.load = seg.load.map(|l| l * frac_a);
        }
        self.nodes[seg.b].segments.retain(|&x| x != s);
        self.nodes[mid].segments.push(s);
        let second = self.add_segment(mid, seg.b, seg.kind);
        self.segments[second].load = seg.load.map(|l| l * (1.0 - frac_a));
        mid
    }

    /// Tidy a node after edits: delete it if it has no segments (unless it
    /// is the entry), or merge its two segments into one if they are
    /// collinear and of the same kind. Returns true if the node is gone.
    pub fn simplify_node(&mut self, n: NodeId) -> bool {
        let Some(node) = self.nodes.get(n) else {
            return true;
        };
        if Some(n) == self.entry {
            return false;
        }
        match node.segments.len() {
            0 => {
                self.nodes.remove(n);
                true
            }
            2 => {
                let (s1, s2) = (node.segments[0], node.segments[1]);
                let (seg1, seg2) = (&self.segments[s1], &self.segments[s2]);
                if seg1.kind != seg2.kind {
                    return false;
                }
                let (end1, end2) = (seg1.other(n), seg2.other(n));
                if end1 == end2 {
                    return false;
                }
                let p = node.pos;
                let d1 = geom::sub(self.nodes[end1].pos, p);
                let d2 = geom::sub(self.nodes[end2].pos, p);
                let l1 = geom::dot(d1, d1).sqrt();
                let l2 = geom::dot(d2, d2).sqrt();
                let collinear = (geom::cross(d1, d2) / (l1 * l2)).abs() < 1e-4 && geom::dot(d1, d2) < 0.0;
                if !collinear {
                    return false;
                }
                // Keep s1, extend it to end2 through n; drop s2.
                let seg2 = self.segments.remove(s2).expect("s2 exists");
                let s2_dir = geom::sub(self.nodes[seg2.b].pos, self.nodes[seg2.a].pos);
                self.nodes[end2].segments.retain(|&x| x != s2);
                self.nodes[end2].segments.push(s1);
                let first = &mut self.segments[s1];
                if first.a == n {
                    first.a = end2;
                } else {
                    first.b = end2;
                }
                first.length += seg2.length;
                first.capacity = capacity_for(first.kind, first.length);
                // Add s2's load per direction, matching travel directions.
                let (fa, fb) = (first.a, first.b);
                let s1_dir = geom::sub(self.nodes[fb].pos, self.nodes[fa].pos);
                let first = &mut self.segments[s1];
                if geom::dot(s1_dir, s2_dir) > 0.0 {
                    first.load[0] += seg2.load[0];
                    first.load[1] += seg2.load[1];
                } else {
                    first.load[0] += seg2.load[1];
                    first.load[1] += seg2.load[0];
                }
                self.nodes.remove(n);
                true
            }
            _ => false,
        }
    }

    /// Check graph invariants. Used by tests; cheap enough for debug asserts
    /// on small maps (O(nodes * segments)).
    pub fn validate(&self) -> Result<(), String> {
        for (s, seg) in &self.segments {
            if seg.a == seg.b {
                return Err(format!("{s:?} is a loop"));
            }
            for n in [seg.a, seg.b] {
                let node = self.nodes.get(n).ok_or(format!("{s:?} references missing {n:?}"))?;
                if !node.segments.contains(&s) {
                    return Err(format!("{n:?} does not list {s:?}"));
                }
            }
            let (pa, pb) = self.segment_ends(s);
            let d = geom::dist(pa, pb);
            if d < geom::POS_EPS {
                return Err(format!("{s:?} has zero length"));
            }
            if (d - seg.length).abs() > 0.01 {
                return Err(format!("{s:?} length {} != {d}", seg.length));
            }
            for (n, node) in &self.nodes {
                if geom::on_segment_interior(node.pos, pa, pb) {
                    return Err(format!("{n:?} lies inside {s:?} (unsplit intersection/overlap)"));
                }
            }
        }
        for (n, node) in &self.nodes {
            for &s in &node.segments {
                let seg = self.segments.get(s).ok_or(format!("{n:?} lists missing {s:?}"))?;
                if seg.a != n && seg.b != n {
                    return Err(format!("{n:?} lists unrelated {s:?}"));
                }
            }
            let mut sorted = node.segments.clone();
            sorted.sort();
            sorted.dedup();
            if sorted.len() != node.segments.len() {
                return Err(format!("{n:?} lists a segment twice"));
            }
            if node.segments.is_empty() && Some(n) != self.entry {
                return Err(format!("{n:?} is dangling"));
            }
            for (m, other) in &self.nodes {
                if m != n && geom::same_pos(node.pos, other.pos) {
                    return Err(format!("{n:?} and {m:?} are duplicates"));
                }
            }
            // Duplicate segments between the same pair of nodes.
            let mut ends: Vec<NodeId> = node.segments.iter().map(|&s| self.segments[s].other(n)).collect();
            ends.sort();
            let len = ends.len();
            ends.dedup();
            if ends.len() != len {
                return Err(format!("{n:?} has two segments to the same node"));
            }
        }
        if let Some(e) = self.entry
            && !self.nodes.contains_key(e)
        {
            return Err("entry node missing".into());
        }
        Ok(())
    }
}
