//! Per-cell land use: zoning and what occupies the cell.
//!
//! The grid is used by the build tools and zoning rules. Pathfinding,
//! vehicles and rendering of roads must use [`crate::RoadNetwork`] instead.

use serde::{Deserialize, Serialize};

use crate::{BuildingId, Cell, MAP_CELLS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Zone {
    Residential,
    Commercial,
    Industrial,
}

impl Zone {
    pub const ALL: [Zone; 3] = [Zone::Residential, Zone::Commercial, Zone::Industrial];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Occupant {
    #[default]
    Empty,
    /// A road passes through the cell centre.
    Road,
    Building(BuildingId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellData {
    pub zone: Option<Zone>,
    pub occupant: Occupant,
}

/// `MAP_CELLS * MAP_CELLS` cells, row-major (`index = y * size + x`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub size: i32,
    pub cells: Vec<CellData>,
}

impl Default for Grid {
    fn default() -> Self {
        Grid::new()
    }
}

impl Grid {
    pub fn new() -> Grid {
        Grid {
            size: MAP_CELLS,
            cells: vec![CellData::default(); (MAP_CELLS * MAP_CELLS) as usize],
        }
    }

    pub fn index(&self, c: Cell) -> Option<usize> {
        (c.x >= 0 && c.y >= 0 && c.x < self.size && c.y < self.size).then(|| (c.y * self.size + c.x) as usize)
    }

    pub fn get(&self, c: Cell) -> Option<&CellData> {
        self.index(c).map(|i| &self.cells[i])
    }

    pub fn get_mut(&mut self, c: Cell) -> Option<&mut CellData> {
        self.index(c).map(|i| &mut self.cells[i])
    }

    pub fn is_road(&self, c: Cell) -> bool {
        matches!(
            self.get(c),
            Some(CellData {
                occupant: Occupant::Road,
                ..
            })
        )
    }

    /// Is there a road cell within `reach` (Chebyshev) of `c`?
    pub fn near_road(&self, c: Cell, reach: i32) -> bool {
        (-reach..=reach).any(|dy| (-reach..=reach).any(|dx| self.is_road(c.offset(dx, dy))))
    }
}
