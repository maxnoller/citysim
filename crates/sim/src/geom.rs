//! Cells, rectangles and small geometry helpers.

use serde::{Deserialize, Serialize};

use crate::{CELL_SIZE, MAP_CELLS};

/// A map cell. `(0, 0)` is the cell at the sim-space origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct Cell {
    pub x: i32,
    pub y: i32,
}

impl Cell {
    pub const fn new(x: i32, y: i32) -> Cell {
        Cell { x, y }
    }

    /// Centre of the cell in sim metres.
    pub fn center(self) -> [f32; 2] {
        [(self.x as f32 + 0.5) * CELL_SIZE, (self.y as f32 + 0.5) * CELL_SIZE]
    }

    /// The cell containing a point in sim metres (may be out of bounds).
    pub fn from_pos(pos: [f32; 2]) -> Cell {
        Cell {
            x: (pos[0] / CELL_SIZE).floor() as i32,
            y: (pos[1] / CELL_SIZE).floor() as i32,
        }
    }

    pub fn in_bounds(self) -> bool {
        (0..MAP_CELLS).contains(&self.x) && (0..MAP_CELLS).contains(&self.y)
    }

    pub fn offset(self, dx: i32, dy: i32) -> Cell {
        Cell::new(self.x + dx, self.y + dy)
    }

    /// Chebyshev (king-move) distance.
    pub fn chebyshev(self, other: Cell) -> i32 {
        (self.x - other.x).abs().max((self.y - other.y).abs())
    }

    /// Cells on the straight line from `self` to `to`, inclusive, if the line
    /// is horizontal, vertical or exactly 45 degrees. `None` otherwise.
    /// A zero-length line yields just `[self]`.
    pub fn straight_line_to(self, to: Cell) -> Option<Vec<Cell>> {
        let dx = to.x - self.x;
        let dy = to.y - self.y;
        if dx != 0 && dy != 0 && dx.abs() != dy.abs() {
            return None;
        }
        let n = dx.abs().max(dy.abs());
        let (sx, sy) = (dx.signum(), dy.signum());
        Some((0..=n).map(|k| self.offset(sx * k, sy * k)).collect())
    }
}

/// An inclusive rectangle of cells. Constructed normalised (`min <= max`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellRect {
    pub min: Cell,
    pub max: Cell,
}

impl CellRect {
    /// Rectangle spanned by two corners in any order.
    pub fn from_corners(a: Cell, b: Cell) -> CellRect {
        CellRect {
            min: Cell::new(a.x.min(b.x), a.y.min(b.y)),
            max: Cell::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    pub fn single(c: Cell) -> CellRect {
        CellRect { min: c, max: c }
    }

    pub fn contains(&self, c: Cell) -> bool {
        (self.min.x..=self.max.x).contains(&c.x) && (self.min.y..=self.max.y).contains(&c.y)
    }

    pub fn intersects(&self, other: &CellRect) -> bool {
        self.min.x <= other.max.x && other.min.x <= self.max.x && self.min.y <= other.max.y && other.min.y <= self.max.y
    }

    /// Intersection with the map, or `None` if entirely outside.
    pub fn clamp_to_map(&self) -> Option<CellRect> {
        let min = Cell::new(self.min.x.max(0), self.min.y.max(0));
        let max = Cell::new(self.max.x.min(MAP_CELLS - 1), self.max.y.min(MAP_CELLS - 1));
        (min.x <= max.x && min.y <= max.y).then_some(CellRect { min, max })
    }

    /// All cells, row by row (y outer, x inner).
    pub fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        (self.min.y..=self.max.y).flat_map(move |y| (self.min.x..=self.max.x).map(move |x| Cell::new(x, y)))
    }

    pub fn width(&self) -> i32 {
        self.max.x - self.min.x + 1
    }

    pub fn height(&self) -> i32 {
        self.max.y - self.min.y + 1
    }
}

/// Tolerance for comparing positions in metres.
pub const POS_EPS: f32 = 1e-3;

pub fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

pub fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

pub fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

pub fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    let d = sub(a, b);
    dot(d, d).sqrt()
}

pub fn same_pos(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < POS_EPS && (a[1] - b[1]).abs() < POS_EPS
}

/// Is `p` on the closed segment `a`-`b`?
pub fn on_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    let ab = sub(b, a);
    let ap = sub(p, a);
    let len2 = dot(ab, ab);
    if len2 < POS_EPS * POS_EPS {
        return same_pos(p, a);
    }
    let len = len2.sqrt();
    if (cross(ab, ap) / len).abs() > POS_EPS {
        return false;
    }
    let t = dot(ap, ab) / len;
    t >= -POS_EPS && t <= len + POS_EPS
}

/// Is `p` strictly inside segment `a`-`b` (on it, but not at an end)?
pub fn on_segment_interior(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> bool {
    on_segment(p, a, b) && !same_pos(p, a) && !same_pos(p, b)
}
