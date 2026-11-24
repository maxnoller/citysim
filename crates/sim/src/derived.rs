//! Data derived from the serialised state, rebuilt lazily when the state it
//! depends on changes (keyed by `world.versions`). Never serialised and
//! ignored by `World`'s `PartialEq`: everything here is a pure function of
//! the real state, so a loaded world rebuilds exactly the same thing and
//! the simulation stays deterministic across save/load.
//!
//! Note: code that mutates `world.roads` or `world.grid` directly (instead
//! of through `World::apply`) must bump `world.versions` as usual.

use std::fmt;

use crate::geom::{self, Cell, CellRect};
use crate::grid::{Grid, Occupant, Zone};
use crate::idmap::key_index;
use crate::path::PathScratch;
use crate::roads::{NodeId, RoadNetwork, SegmentId};
use crate::{CELL_SIZE, MAP_CELLS, balance};

/// A point on the road network: `from_a` metres along `segment` from its
/// `a` node.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Access {
    pub segment: SegmentId,
    pub from_a: f32,
}

type RoadsKey = (u64, usize, usize);

#[derive(Clone, Default)]
pub(crate) struct Derived {
    roads_key: Option<RoadsKey>,
    /// Cell -> segments passing through it (CSR layout over all map cells).
    cell_start: Vec<u32>,
    cell_segs: Vec<SegmentId>,
    /// By node slot: in the same connected piece of road as the entry.
    node_connected: Vec<bool>,
    /// By cell: some road connected to the entry passes through it.
    road_cell_connected: Vec<bool>,
    cand_key: Option<(u64, RoadsKey)>,
    /// Zoned, empty cells with road access to the entry, per zone, row-major.
    candidates: [Vec<Cell>; 3],
    road_cells_key: Option<u64>,
    road_cells: u32,
    pub path: PathScratch,
}

impl PartialEq for Derived {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl fmt::Debug for Derived {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Derived { .. }")
    }
}

fn cell_index(c: Cell) -> Option<usize> {
    c.in_bounds().then(|| (c.y * MAP_CELLS + c.x) as usize)
}

pub(crate) fn zone_index(z: Zone) -> usize {
    match z {
        Zone::Residential => 0,
        Zone::Commercial => 1,
        Zone::Industrial => 2,
    }
}

impl Derived {
    fn roads_key(roads: &RoadNetwork, version: u64) -> RoadsKey {
        (version, roads.nodes.slot_count(), roads.segments.len())
    }

    /// Rebuild the road index and connectivity if the road graph changed.
    pub fn ensure_roads(&mut self, roads: &RoadNetwork, version: u64) {
        let key = Self::roads_key(roads, version);
        if self.roads_key == Some(key) {
            return;
        }
        self.roads_key = Some(key);
        let ncells = (MAP_CELLS * MAP_CELLS) as usize;

        // Connectivity from the entry (BFS over nodes).
        self.node_connected.clear();
        self.node_connected.resize(roads.nodes.slot_count(), false);
        if let Some(e) = roads.entry.filter(|&e| roads.nodes.contains_key(e)) {
            let mut stack: Vec<NodeId> = vec![e];
            self.node_connected[key_index(e)] = true;
            while let Some(n) = stack.pop() {
                for &s in &roads.nodes[n].segments {
                    let m = roads.segments[s].other(n);
                    if !self.node_connected[key_index(m)] {
                        self.node_connected[key_index(m)] = true;
                        stack.push(m);
                    }
                }
            }
        }

        // Cell index: sample each segment every 2 m.
        let mut pairs: Vec<(u32, SegmentId)> = Vec::new();
        for (s, seg) in roads.segments.iter() {
            let (pa, pb) = roads.segment_ends(s);
            let steps = (seg.length / 2.0).ceil().max(1.0) as usize;
            for k in 0..=steps {
                let t = k as f32 / steps as f32;
                let p = [pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t];
                if let Some(i) = cell_index(Cell::from_pos(p)) {
                    pairs.push((i as u32, s));
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        self.cell_start.clear();
        self.cell_start.resize(ncells + 1, 0);
        for &(i, _) in &pairs {
            self.cell_start[i as usize + 1] += 1;
        }
        for i in 0..ncells {
            self.cell_start[i + 1] += self.cell_start[i];
        }
        self.cell_segs = pairs.iter().map(|&(_, s)| s).collect();

        self.road_cell_connected.clear();
        self.road_cell_connected.resize(ncells, false);
        for &(i, s) in &pairs {
            if self.segment_connected(roads, s) {
                self.road_cell_connected[i as usize] = true;
            }
        }
    }

    /// Segments passing through cell `c`. Requires `ensure_roads`.
    pub fn segments_in(&self, c: Cell) -> &[SegmentId] {
        match cell_index(c) {
            Some(i) if i + 1 < self.cell_start.len() => {
                &self.cell_segs[self.cell_start[i] as usize..self.cell_start[i + 1] as usize]
            }
            _ => &[],
        }
    }

    pub fn node_connected(&self, n: NodeId) -> bool {
        self.node_connected.get(key_index(n)).copied().unwrap_or(false)
    }

    pub fn segment_connected(&self, roads: &RoadNetwork, s: SegmentId) -> bool {
        roads.segments.get(s).is_some_and(|seg| self.node_connected(seg.a))
    }

    /// Is a road connected to the entry within `reach` cells of `rect`?
    pub fn connected_near(&self, rect: CellRect, reach: i32) -> bool {
        for y in rect.min.y - reach..=rect.max.y + reach {
            for x in rect.min.x - reach..=rect.max.x + reach {
                if let Some(i) = cell_index(Cell::new(x, y))
                    && self.road_cell_connected.get(i).copied().unwrap_or(false)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Nearest point on the road network to `pos` within
    /// `balance::ACCESS_SEARCH_CELLS`, preferring roads connected to the
    /// entry. Requires `ensure_roads`.
    pub fn access_near(&self, roads: &RoadNetwork, pos: [f32; 2]) -> Option<Access> {
        let c = Cell::from_pos(pos);
        let r = balance::ACCESS_SEARCH_CELLS;
        let mut best: Option<(u8, f32, usize, Access)> = None;
        for y in c.y - r..=c.y + r {
            for x in c.x - r..=c.x + r {
                for &s in self.segments_in(Cell::new(x, y)) {
                    let Some(seg) = roads.segments.get(s) else { continue };
                    let (pa, pb) = roads.segment_ends(s);
                    let ab = geom::sub(pb, pa);
                    let t = (geom::dot(geom::sub(pos, pa), ab) / geom::dot(ab, ab)).clamp(0.0, 1.0);
                    let p = [pa[0] + ab[0] * t, pa[1] + ab[1] * t];
                    let d = geom::dist(p, pos);
                    if d > (r as f32 + 0.5) * CELL_SIZE {
                        continue;
                    }
                    // Rank: connected first, then nearer, then lower slot.
                    let rank = (u8::from(!self.node_connected(seg.a)), d, key_index(s));
                    let better = match &best {
                        None => true,
                        Some((bc, bd, bi, _)) => (rank.0, rank.1, rank.2) < (*bc, *bd, *bi),
                    };
                    if better {
                        best = Some((
                            rank.0,
                            d,
                            rank.2,
                            Access {
                                segment: s,
                                from_a: t * seg.length,
                            },
                        ));
                    }
                }
            }
        }
        best.map(|b| b.3)
    }

    /// Growth candidates for zone `z`. Requires `ensure_roads`.
    pub fn candidates(&mut self, grid: &Grid, grid_version: u64, z: Zone) -> &[Cell] {
        let key = (grid_version, self.roads_key.unwrap_or_default());
        if self.cand_key != Some(key) {
            self.cand_key = Some(key);
            for v in &mut self.candidates {
                v.clear();
            }
            let reach = balance::ZONE_ROAD_REACH;
            for y in 0..MAP_CELLS {
                for x in 0..MAP_CELLS {
                    let cd = &grid.cells[(y * MAP_CELLS + x) as usize];
                    if let Some(zone) = cd.zone
                        && cd.occupant == Occupant::Empty
                    {
                        let c = Cell::new(x, y);
                        if self.connected_near(CellRect::single(c), reach) {
                            self.candidates[zone_index(zone)].push(c);
                        }
                    }
                }
            }
        }
        &self.candidates[zone_index(z)]
    }

    /// Number of road cells (cached per grid version).
    pub fn road_cells(&mut self, grid: &Grid, grid_version: u64) -> u32 {
        if self.road_cells_key != Some(grid_version) {
            self.road_cells_key = Some(grid_version);
            self.road_cells = grid.cells.iter().filter(|c| c.occupant == Occupant::Road).count() as u32;
        }
        self.road_cells
    }
}

/// Centre of a footprint in sim metres.
pub fn rect_center(r: CellRect) -> [f32; 2] {
    let a = r.min.center();
    let b = r.max.center();
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

/// Point of an access on the road, metres.
pub fn access_pos(roads: &RoadNetwork, a: Access) -> Option<[f32; 2]> {
    let seg = roads.segments.get(a.segment)?;
    let (pa, pb) = roads.segment_ends(a.segment);
    let t = if seg.length > 0.0 {
        (a.from_a / seg.length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some([pa[0] + (pb[0] - pa[0]) * t, pa[1] + (pb[1] - pa[1]) * t])
}
