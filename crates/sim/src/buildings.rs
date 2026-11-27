//! Buildings grown on zoned land.
//!
//! A building stands on a rectangle of cells of one zoning lattice
//! ([`Footprint`]): its centre in metres, its rotation, its size in cells
//! and the cells it occupies. Its front faces the road it was grown for.

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::zoning::{CellRef, Lattice, LatticeId};
use crate::{CELL_SIZE, CellRect, Zone};

new_key_type! {
    /// Stable id of a [`Building`].
    pub struct BuildingId;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Building {
    /// Zone type the building was grown for.
    pub kind: Zone,
    /// Where it stands: lattice cells, centre, rotation, size.
    pub footprint: Footprint,
    /// Density level, 1 = smallest. Renderer derives height from it.
    pub level: u8,
    /// Residents (residential) or jobs (commercial/industrial) it can hold.
    pub capacity: u32,
    /// Residents or workers currently there.
    pub occupied: u32,
    /// Which side of the footprint faces the road it is accessed from, in
    /// its lattice's axes (`PlusX` = the lattice's `+i`). The world angle of
    /// the front is `footprint.rotation`.
    #[serde(default)]
    pub facing: Facing,
    /// Stable random number for purely visual variation (model, colour...).
    #[serde(default)]
    pub seed: u32,
    /// `world.time.tick` when the building appeared (for grow-in animations).
    #[serde(default)]
    pub created_tick: u64,
}

impl Building {
    /// Size in cells `(width along the front, depth away from the road)`.
    pub fn size(&self) -> (i32, i32) {
        (self.footprint.size[0] as i32, self.footprint.size[1] as i32)
    }

    /// Does the building make way for a road drawn through its lot? A road
    /// that clears its way (`Command::Clearing`) takes such buildings down
    /// as the bulldozer would; any other building in the way refuses the
    /// road (`CommandError::BuildingInTheWay`). Every building grown by
    /// itself on zoned land gives way, and that is every building today;
    /// buildings the player places by hand will not.
    pub fn gives_way_to_roads(&self) -> bool {
        true
    }

    /// Centre in sim metres.
    pub fn center(&self) -> [f32; 2] {
        self.footprint.center
    }

    /// World direction the front faces, radians (`atan2` in sim axes).
    pub fn rotation(&self) -> f32 {
        self.footprint.rotation
    }

    /// Unit vector the front faces.
    pub fn front_dir(&self) -> [f32; 2] {
        [libm::cosf(self.footprint.rotation), libm::sinf(self.footprint.rotation)]
    }

    /// Corners in sim metres: front-left, front-right, back-right,
    /// back-left (left / right seen from inside looking out of the front).
    pub fn corners(&self) -> [[f32; 2]; 4] {
        self.footprint.corners()
    }

    /// The cells it stands on.
    pub fn cells(&self) -> impl Iterator<Item = CellRef> + '_ {
        self.footprint.cells()
    }
}

/// Where a building stands. All its cells are in one lattice.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Footprint {
    pub lattice: LatticeId,
    /// Inclusive cell rectangle in the lattice.
    pub min: [i32; 2],
    pub max: [i32; 2],
    /// Centre, sim metres.
    pub center: [f32; 2],
    /// Direction the front faces, radians (`atan2` in sim axes).
    pub rotation: f32,
    /// `[width along the front, depth]` in cells.
    pub size: [u8; 2],
}

impl Footprint {
    /// The footprint of cells `min..=max` of `lattice` (whose geometry is
    /// `l`) with its front towards `facing`.
    pub fn new(lattice: LatticeId, l: &Lattice, min: [i32; 2], max: [i32; 2], facing: Facing) -> Footprint {
        let cx = (min[0] + max[0] + 1) as f32 * 0.5;
        let center = l.world_at(cx, (min[1] + max[1] + 1) as f32 * 0.5);
        let d = l.dir_at(cx, facing);
        let (w, h) = ((max[0] - min[0] + 1) as u8, (max[1] - min[1] + 1) as u8);
        let size = match facing {
            Facing::PlusX | Facing::MinusX => [h, w],
            Facing::PlusY | Facing::MinusY => [w, h],
        };
        Footprint {
            lattice,
            min,
            max,
            center,
            rotation: libm::atan2f(d[1], d[0]),
            size,
        }
    }

    /// A footprint on the world grid (lattice 0).
    pub fn world(r: CellRect, facing: Facing) -> Footprint {
        Footprint::new(
            LatticeId::WORLD,
            &Lattice::world(),
            [r.min.x, r.min.y],
            [r.max.x, r.max.y],
            facing,
        )
    }

    /// The cell rectangle in lattice coordinates (a world-grid rectangle
    /// for lattice 0).
    pub fn rect(&self) -> CellRect {
        CellRect::from_corners(
            crate::Cell::new(self.min[0], self.min[1]),
            crate::Cell::new(self.max[0], self.max[1]),
        )
    }

    pub fn contains(&self, c: CellRef) -> bool {
        c.lattice == self.lattice
            && (self.min[0]..=self.max[0]).contains(&c.i)
            && (self.min[1]..=self.max[1]).contains(&c.j)
    }

    /// Cells, row by row.
    pub fn cells(&self) -> impl Iterator<Item = CellRef> + '_ {
        (self.min[1]..=self.max[1])
            .flat_map(move |j| (self.min[0]..=self.max[0]).map(move |i| CellRef::new(self.lattice, i, j)))
    }

    /// Number of cells.
    pub fn area(&self) -> u32 {
        self.size[0] as u32 * self.size[1] as u32
    }

    /// Cells the building itself stands on: its front
    /// `balance::BUILT_DEPTH` rows (deeper rows are its yard).
    pub fn built_area(&self) -> u32 {
        self.size[0] as u32 * self.size[1].min(crate::balance::BUILT_DEPTH) as u32
    }

    /// Rows of yard behind the building.
    pub fn yard_depth(&self) -> u8 {
        self.size[1].saturating_sub(crate::balance::BUILT_DEPTH)
    }

    /// Corners in sim metres: front-left, front-right, back-right,
    /// back-left.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let f = [libm::cosf(self.rotation), libm::sinf(self.rotation)];
        // Right of the front direction, seen from inside: (-f.y, f.x) is
        // the right of travel *towards* the front.
        let r = [-f[1], f[0]];
        let hw = self.size[0] as f32 * CELL_SIZE * 0.5;
        let hd = self.size[1] as f32 * CELL_SIZE * 0.5;
        let c = self.center;
        let at = |a: f32, b: f32| [c[0] + f[0] * a + r[0] * b, c[1] + f[1] * a + r[1] * b];
        [at(hd, -hw), at(hd, hw), at(-hd, hw), at(-hd, -hw)]
    }
}

/// One of the four sides of a cell or footprint, in sim axes
/// (sim `+y` is Bevy `+z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Facing {
    /// Front faces `+x`.
    PlusX,
    /// Front faces `+y`.
    #[default]
    PlusY,
    /// Front faces `-x`.
    MinusX,
    /// Front faces `-y`.
    MinusY,
}

impl Facing {
    pub const ALL: [Facing; 4] = [Facing::PlusX, Facing::PlusY, Facing::MinusX, Facing::MinusY];

    /// Unit cell offset towards the front.
    pub fn offset(self) -> (i32, i32) {
        match self {
            Facing::PlusX => (1, 0),
            Facing::PlusY => (0, 1),
            Facing::MinusX => (-1, 0),
            Facing::MinusY => (0, -1),
        }
    }

    /// Direction of the front in sim space, radians, `atan2(dy, dx)`.
    pub fn angle(self) -> f32 {
        let (dx, dy) = self.offset();
        libm::atan2f(dy as f32, dx as f32)
    }
}
