//! Data derived from the serialised state, rebuilt lazily when the state it
//! depends on changes (keyed by `world.versions`). Never serialised and
//! ignored by `World`'s `PartialEq`: everything here is a pure function of
//! the real state, so a loaded world rebuilds exactly the same thing and
//! the simulation stays deterministic across save/load.
//!
//! Note: code that mutates `world.roads` or `world.grid` directly (instead
//! of through `World::apply`) must bump `world.versions` as usual.

use std::fmt;

use crate::geom::{Cell, CellRect};
use crate::idmap::key_index;
use crate::path::PathScratch;
use crate::roads::{NodeId, RoadNetwork, SegmentId};
use crate::zoning::{CellRef, Zone, Zoning};
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
    /// `(zoning.roads_version, roads key)` the connected-cell sets were
    /// built for.
    zoning_key: Option<(u64, RoadsKey)>,
    /// Cells of blocks whose road is connected to the entry.
    conn_cells: std::collections::BTreeSet<CellRef>,
    /// Front-row cells of such blocks with the side the road is on.
    conn_fronts: Vec<(CellRef, u8)>,
    /// Cell -> segments passing through it (CSR layout over all map cells).
    cell_start: Vec<u32>,
    cell_segs: Vec<SegmentId>,
    /// By node slot: in the same connected piece of road as the entry.
    node_connected: Vec<bool>,
    /// By segment slot: its `a` node.
    seg_a: Vec<Option<NodeId>>,
    cand_key: Option<(u64, (u64, RoadsKey))>,
    /// Zoned, empty front cells with road access to the entry, per zone.
    candidates: [Vec<CellRef>; 3],
    road_cells_key: Option<RoadsKey>,
    road_cells: u32,
    avenue_key: Option<RoadsKey>,
    avenue_cells: u32,
    pub path: PathScratch,
    /// Stop lines per segment slot (`drive::build_setbacks`), for
    /// `traffic.junctions_version` `setbacks_key`.
    pub setbacks: Vec<[f32; 2]>,
    pub setbacks_key: Option<u64>,
    /// Expected junction wait per segment slot and direction, for routing
    /// (`drive::build_route_delays`), for `(junctions_version,
    /// control_version)` `route_delay_key`.
    pub route_delay: Vec<[f32; 2]>,
    pub route_delay_key: Option<(u64, u64)>,
    places_key: Option<(u64, usize)>,
    /// Buildings by kind, in slot order (for random picks of shops, venues,
    /// factories and free homes), for `versions.buildings`.
    pub homes: Vec<crate::BuildingId>,
    pub shops: Vec<crate::BuildingId>,
    pub factories: Vec<crate::BuildingId>,
    access_key: Option<(u64, usize)>,
    /// `Citizen::until_tick` by citizen slot (a compact copy for the
    /// decision buckets); rebuilt when `until_ok` is false.
    pub until: Vec<u64>,
    pub until_ok: bool,
    /// Distance (m) from each access tile to the nearest shop / leisure
    /// venue (`ACCESS_TILE` cells per tile side).
    access_shop: Vec<f32>,
    access_venue: Vec<f32>,
    /// Debug counters: front cars at the stop line refused entry, by reason
    /// `[conflict in box, no room beyond, signal, yielding]`.
    pub denials: [u64; 4],
    /// Debug counters: junction entries by turn `[straight, right, left, u-turn]`.
    pub turns: [u64; 4],
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
            let steps = (seg.length / 2.0).ceil().max(1.0) as usize;
            for k in 0..=steps {
                let p = seg.pose(seg.length * k as f32 / steps as f32, 0.0).pos;
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

        self.seg_a.clear();
        self.seg_a.resize(roads.segments.slot_count(), None);
        for (s, seg) in roads.segments.iter() {
            self.seg_a[key_index(s)] = Some(seg.a);
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

    #[allow(dead_code)]
    pub fn segment_connected(&self, roads: &RoadNetwork, s: SegmentId) -> bool {
        roads.segments.get(s).is_some_and(|seg| self.node_connected(seg.a))
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
                    let (from_a, d, _) = seg.nearest(pos);
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
                        best = Some((rank.0, d, rank.2, Access { segment: s, from_a }));
                    }
                }
            }
        }
        best.map(|b| b.3)
    }

    /// Rebuild the connected-cell sets if the blocks or the road
    /// connectivity changed. Requires `ensure_roads`.
    pub fn ensure_zoning(&mut self, zoning: &Zoning) {
        let key = (zoning.roads_version, self.roads_key.unwrap_or_default());
        if self.zoning_key == Some(key) {
            return;
        }
        self.zoning_key = Some(key);
        self.conn_cells.clear();
        let mut fronts = std::collections::BTreeSet::new();
        for b in &zoning.blocks {
            let connected = self.node_connected_seg(b.segment);
            if !connected {
                continue;
            }
            for c in b.cells() {
                self.conn_cells.insert(c);
                if b.in_front_row(c) {
                    fronts.insert((c, b.front as u8));
                }
            }
        }
        self.conn_fronts = fronts.into_iter().collect();
    }

    fn node_connected_seg(&self, s: SegmentId) -> bool {
        self.seg_a
            .get(key_index(s))
            .copied()
            .flatten()
            .is_some_and(|n| self.node_connected(n))
    }

    /// Is cell `c` in a block whose road is connected to the entry?
    /// Requires `ensure_zoning`.
    pub fn cell_connected(&self, c: CellRef) -> bool {
        self.conn_cells.contains(&c)
    }

    /// Is `c` in the front row of such a block on side `f`?
    pub fn front_connected(&self, c: CellRef, f: crate::Facing) -> bool {
        self.conn_fronts.binary_search(&(c, f as u8)).is_ok()
    }

    /// Growth candidates for zone `z`: zoned, empty, usable front cells
    /// of blocks whose road is connected to the entry. Requires
    /// `ensure_zoning`.
    pub fn candidates(&mut self, zoning: &Zoning, grid_version: u64, z: Zone) -> &[CellRef] {
        let key = (grid_version, self.zoning_key.unwrap_or_default());
        if self.cand_key != Some(key) {
            self.cand_key = Some(key);
            for v in &mut self.candidates {
                v.clear();
            }
            let mut last: Option<CellRef> = None;
            for &(c, _) in &self.conn_fronts {
                if last == Some(c) {
                    continue;
                }
                last = Some(c);
                if let Some(x) = zoning.cell(c)
                    && x.free()
                    && let Some(zone) = x.zone
                {
                    self.candidates[zone_index(zone)].push(c);
                }
            }
        }
        &self.candidates[zone_index(z)]
    }

    /// Cell-to-cell pieces of avenue (cached per road version).
    pub fn avenue_cells(&mut self, roads: &RoadNetwork, version: u64) -> u32 {
        let key = Self::roads_key(roads, version);
        if self.avenue_key != Some(key) {
            self.avenue_key = Some(key);
            let m: f64 = roads
                .segments
                .values()
                .filter(|s| s.kind == crate::RoadKind::Avenue)
                .map(|s| s.length as f64)
                .sum();
            self.avenue_cells = (m / CELL_SIZE as f64).round() as u32;
        }
        self.avenue_cells
    }

    /// Lists of homes, shops and factories (cached per buildings version).
    pub fn ensure_places(&mut self, buildings: &crate::IdMap<crate::BuildingId, crate::Building>, version: u64) {
        let key = (version, buildings.len());
        if self.places_key == Some(key) {
            return;
        }
        self.places_key = Some(key);
        self.homes.clear();
        self.shops.clear();
        self.factories.clear();
        for (id, b) in buildings.iter() {
            match b.kind {
                Zone::Residential => self.homes.push(id),
                Zone::Commercial => self.shops.push(id),
                Zone::Industrial => self.factories.push(id),
            }
        }
    }

    /// Distance fields to the nearest shop and leisure venue (any
    /// commercial building; venues of level 1 count as farther), on tiles of
    /// `ACCESS_TILE` cells, by a two-pass chamfer transform (cached per
    /// buildings version).
    pub fn ensure_access(&mut self, buildings: &crate::IdMap<crate::BuildingId, crate::Building>, version: u64) {
        let key = (version, buildings.len());
        if self.access_key == Some(key) {
            return;
        }
        self.access_key = Some(key);
        let n = ACCESS_TILES * ACCESS_TILES;
        let tile_m = (ACCESS_TILE as f32) * CELL_SIZE;
        const FAR: f32 = 1.0e7;
        self.access_shop.clear();
        self.access_shop.resize(n, FAR);
        self.access_venue.clear();
        self.access_venue.resize(n, FAR);
        for b in buildings.values() {
            if b.kind != Zone::Commercial {
                continue;
            }
            let c = Cell::from_pos(rect_center(b.footprint));
            let (tx, ty) = (c.x / ACCESS_TILE, c.y / ACCESS_TILE);
            if !(0..ACCESS_TILES as i32).contains(&tx) || !(0..ACCESS_TILES as i32).contains(&ty) {
                continue;
            }
            let i = ty as usize * ACCESS_TILES + tx as usize;
            self.access_shop[i] = 0.0;
            let venue = if b.level >= 2 {
                0.0
            } else {
                balance::ACCESS_SMALL_VENUE_M
            };
            self.access_venue[i] = self.access_venue[i].min(venue);
        }
        for f in [&mut self.access_shop, &mut self.access_venue] {
            chamfer(f, ACCESS_TILES, tile_m);
        }
    }

    /// Distances (m) from a footprint to the nearest shop and venue.
    /// Requires `ensure_access`; far away when unknown.
    pub fn access_at(&self, r: Option<crate::Footprint>) -> (f32, f32) {
        let Some(r) = r else { return (1.0e7, 1.0e7) };
        let c = Cell::from_pos(r.center);
        let (tx, ty) = (c.x / ACCESS_TILE, c.y / ACCESS_TILE);
        if !(0..ACCESS_TILES as i32).contains(&tx) || !(0..ACCESS_TILES as i32).contains(&ty) {
            return (1.0e7, 1.0e7);
        }
        let i = ty as usize * ACCESS_TILES + tx as usize;
        (
            self.access_shop.get(i).copied().unwrap_or(1.0e7),
            self.access_venue.get(i).copied().unwrap_or(1.0e7),
        )
    }

    /// Road length in cells (metres / `CELL_SIZE`, rounded; cached per
    /// road version).
    pub fn road_cells(&mut self, roads: &RoadNetwork, version: u64) -> u32 {
        let key = Self::roads_key(roads, version);
        if self.road_cells_key != Some(key) {
            self.road_cells_key = Some(key);
            let m: f64 = roads.segments.values().map(|s| s.length as f64).sum();
            self.road_cells = (m / CELL_SIZE as f64).round() as u32;
        }
        self.road_cells
    }
}

/// Cells per side of an access tile.
const ACCESS_TILE: i32 = 4;
const ACCESS_TILES: usize = (MAP_CELLS / ACCESS_TILE) as usize;

/// Two-pass chamfer distance transform (3-4 weights scaled to metres) over
/// a square grid of `n` tiles of `tile_m` metres, in place.
fn chamfer(f: &mut [f32], n: usize, tile_m: f32) {
    let (a, d) = (tile_m, tile_m * std::f32::consts::SQRT_2);
    let ix = |x: usize, y: usize| y * n + x;
    for y in 0..n {
        for x in 0..n {
            let mut v = f[ix(x, y)];
            if x > 0 {
                v = v.min(f[ix(x - 1, y)] + a);
            }
            if y > 0 {
                v = v.min(f[ix(x, y - 1)] + a);
                if x > 0 {
                    v = v.min(f[ix(x - 1, y - 1)] + d);
                }
                if x + 1 < n {
                    v = v.min(f[ix(x + 1, y - 1)] + d);
                }
            }
            f[ix(x, y)] = v;
        }
    }
    for y in (0..n).rev() {
        for x in (0..n).rev() {
            let mut v = f[ix(x, y)];
            if x + 1 < n {
                v = v.min(f[ix(x + 1, y)] + a);
            }
            if y + 1 < n {
                v = v.min(f[ix(x, y + 1)] + a);
                if x + 1 < n {
                    v = v.min(f[ix(x + 1, y + 1)] + d);
                }
                if x > 0 {
                    v = v.min(f[ix(x - 1, y + 1)] + d);
                }
            }
            f[ix(x, y)] = v;
        }
    }
}

/// Centre of a footprint in sim metres.
pub fn rect_center(f: crate::Footprint) -> [f32; 2] {
    f.center
}

/// Centre of a world-grid cell rectangle in sim metres.
pub fn cell_rect_center(r: CellRect) -> [f32; 2] {
    let a = r.min.center();
    let b = r.max.center();
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

/// Point of an access on the road, metres.
pub fn access_pos(roads: &RoadNetwork, a: Access) -> Option<[f32; 2]> {
    let seg = roads.segments.get(a.segment)?;
    Some(seg.pose(a.from_a.clamp(0.0, seg.length), 0.0).pos)
}
