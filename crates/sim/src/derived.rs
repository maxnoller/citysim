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

    /// Is `c` a road cell (grid says road) connected to the entry?
    pub fn connected_road_cell(&self, grid: &Grid, c: Cell) -> bool {
        grid.is_road(c) && cell_index(c).is_some_and(|i| self.road_cell_connected.get(i).copied().unwrap_or(false))
    }

    /// Growth candidates for zone `z`: zoned, empty *frontage* cells (a
    /// side touches a road cell connected to the entry). Requires
    /// `ensure_roads`.
    pub fn candidates(&mut self, grid: &Grid, grid_version: u64, z: Zone) -> &[Cell] {
        let key = (grid_version, self.roads_key.unwrap_or_default());
        if self.cand_key != Some(key) {
            self.cand_key = Some(key);
            for v in &mut self.candidates {
                v.clear();
            }
            for y in 0..MAP_CELLS {
                for x in 0..MAP_CELLS {
                    let cd = &grid.cells[(y * MAP_CELLS + x) as usize];
                    if let Some(zone) = cd.zone
                        && cd.occupant == Occupant::Empty
                    {
                        let c = Cell::new(x, y);
                        let front = [(1, 0), (0, 1), (-1, 0), (0, -1)]
                            .iter()
                            .any(|&(dx, dy)| self.connected_road_cell(grid, c.offset(dx, dy)));
                        if front {
                            self.candidates[zone_index(zone)].push(c);
                        }
                    }
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
            self.avenue_cells = roads
                .segments
                .iter()
                .filter(|(_, s)| s.kind == crate::RoadKind::Avenue)
                .map(|(id, _)| {
                    let (a, b) = roads.segment_ends(id);
                    let d = geom::sub(b, a);
                    (d[0].abs().max(d[1].abs()) / CELL_SIZE).round() as u32
                })
                .sum();
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
    pub fn access_at(&self, r: Option<CellRect>) -> (f32, f32) {
        let Some(r) = r else { return (1.0e7, 1.0e7) };
        let c = Cell::from_pos(rect_center(r));
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

    /// Number of road cells (cached per grid version).
    pub fn road_cells(&mut self, grid: &Grid, grid_version: u64) -> u32 {
        if self.road_cells_key != Some(grid_version) {
            self.road_cells_key = Some(grid_version);
            self.road_cells = grid.cells.iter().filter(|c| c.occupant == Occupant::Road).count() as u32;
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
