//! A read-only view of the world grid (lattice 0) as zoning sees it, for
//! tools, tests and anything that thinks in world cells. The authority for
//! zoning is [`crate::zoning`]; roads are [`crate::RoadNetwork`].

use serde::{Deserialize, Serialize};

pub use crate::zoning::Zone;
use crate::zoning::{CellRef, LatticeId};
use crate::{BuildingId, Cell, MAP_CELLS, SegmentId, World};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Occupant {
    #[default]
    Empty,
    /// A road's reserve covers the cell centre.
    Road,
    Building(BuildingId),
}

/// One world cell: the zone of the lattice-0 cell there and what occupies
/// it (a road, any lattice's building, or nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellData {
    pub zone: Option<Zone>,
    pub occupant: Occupant,
}

/// See the module docs. Build one with `World::grid_view` (it indexes the
/// roads once) and query many cells.
pub struct GridView<'a> {
    world: &'a World,
    /// Segments by tile of `TILE` cells (bounds plus one cell of margin).
    buckets: Vec<Vec<SegmentId>>,
}

const TILE: i32 = 8;
const TILES: i32 = MAP_CELLS / TILE;

impl<'a> GridView<'a> {
    pub(crate) fn new(world: &'a World) -> GridView<'a> {
        let n = TILES as usize;
        let mut buckets: Vec<Vec<SegmentId>> = vec![Vec::new(); n * n];
        for (s, seg) in world.roads.segments.iter() {
            let (lo, hi) = seg.curve.bounds();
            let (a, b) = (Cell::from_pos(lo), Cell::from_pos(hi));
            let t = |v: i32| (v / TILE).clamp(0, TILES - 1);
            for y in t(a.y - 1)..=t(b.y + 1) {
                for x in t(a.x - 1)..=t(b.x + 1) {
                    buckets[y as usize * n + x as usize].push(s);
                }
            }
        }
        GridView { world, buckets }
    }

    /// Does a road's reserve cover the centre of `c`?
    pub fn is_road(&self, c: Cell) -> bool {
        if !c.in_bounds() {
            return false;
        }
        let k = (c.y / TILE) as usize * TILES as usize + (c.x / TILE) as usize;
        crate::command::road_over(&self.world.roads, &self.buckets[k], c)
    }

    pub fn get(&self, c: Cell) -> Option<CellData> {
        if !c.in_bounds() {
            return None;
        }
        let z = &self.world.zoning;
        let zone = z.cell(CellRef::new(LatticeId::WORLD, c.x, c.y)).and_then(|x| x.zone);
        let occupant = if self.is_road(c) {
            Occupant::Road
        } else if let Some(b) = crate::zoning::building_at(self.world, c.center()) {
            Occupant::Building(b)
        } else {
            Occupant::Empty
        };
        Some(CellData { zone, occupant })
    }
}
