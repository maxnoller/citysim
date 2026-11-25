//! Buildings appearing, densifying and being abandoned on zoned land.
//!
//! # Lots
//!
//! A building always fronts a road: the cells along its front side
//! (`facing`) each touch a road cell. Zoning reaches two cells from a road,
//! so a zoned block has a front row and a second row. Growth only starts
//! buildings on front-row cells; the free second-row cell of the same zone
//! right behind is absorbed into a deep 1x2 lot, and at level 2+ a lot may
//! widen to 2x2 into free cells beside it (commercial and industrial lots
//! may start 2x2). Second-row cells that no front lot can absorb (the cell
//! in front is unzoned, of another zone, or already holds a shallow
//! building that never upgrades) stay empty.
//!
//! Every `GROWTH_INTERVAL_TICKS` each zone accumulates `demand *
//! GROWTH_PER_HOUR` growth events; each event builds one building on a
//! random zoned, empty cell with road access to the highway, or (sometimes,
//! or when no land is left) upgrades a full building one level. Buildings
//! that stayed empty for several midnights, or that have been cut off from
//! the highway for longer than `ABANDON_UNREACHABLE_GRACE_TICKS` (checked
//! every in-game hour), are abandoned and removed (the zoning stays).

use serde::{Deserialize, Serialize};

use crate::derived::zone_index;
use crate::geom::{Cell, CellRect};
use crate::grid::Occupant;
use crate::side::SideTable;
use crate::{Building, BuildingId, Facing, World, Zone, balance, households};

/// Per-building bookkeeping not needed by renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BuildingMeta {
    /// Consecutive midnights with nobody in it.
    pub empty_days: u8,
    /// Consecutive midnights without a road connection to the highway.
    /// Informational; abandonment uses `unreachable_since`.
    pub unreachable_days: u8,
    /// Tick at which the building was first found without a road
    /// connection to the highway (checked every
    /// `balance::REACHABILITY_CHECK_INTERVAL_TICKS`); `None` while
    /// connected. The building is abandoned once
    /// `balance::ABANDON_UNREACHABLE_GRACE_TICKS` have passed since.
    /// (Saved in the save file's extension block.)
    #[serde(skip)]
    pub unreachable_since: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GrowthState {
    /// Pending growth events per zone `[R, C, I]`.
    pub acc: [f32; 3],
    /// Pending move-ins (used by `households`).
    pub move_in_acc: f32,
    pub meta: SideTable<BuildingId, BuildingMeta>,
    /// Buildings abandoned so far.
    pub abandoned_total: u32,
    /// Smoothed residential demand ignoring empty housing: how much
    /// newcomers want to move in (`demand` module).
    pub attractiveness: f32,
    /// Round-1 second-row buildings merged into the lot in front on load.
    pub legacy_lots_merged: u32,
}

fn demand_of(world: &World, z: Zone) -> f32 {
    match z {
        Zone::Residential => world.demand.residential,
        Zone::Commercial => world.demand.commercial,
        Zone::Industrial => world.demand.industrial,
    }
}

pub(crate) fn tick(world: &mut World) {
    if world.time.tick_of_day == 0
        || world
            .time
            .tick
            .is_multiple_of(balance::REACHABILITY_CHECK_INTERVAL_TICKS)
    {
        abandon(world);
    }
    if !world.time.tick.is_multiple_of(balance::GROWTH_INTERVAL_TICKS) {
        return;
    }
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    let per_tick = balance::GROWTH_INTERVAL_TICKS as f32 / balance::TICKS_PER_HOUR as f32;
    for z in Zone::ALL {
        let zi = zone_index(z);
        let d = demand_of(world, z).max(0.0);
        let acc = &mut world.growth.acc[zi];
        *acc = if d > 0.0 {
            (*acc + d * balance::GROWTH_PER_HOUR[zi] * per_tick).min(3.0)
        } else {
            0.0
        };
        while world.growth.acc[zi] >= 1.0 {
            world.growth.acc[zi] -= 1.0;
            let upgrade_first = world.rng.next_f32() < balance::UPGRADE_SHARE;
            let ok = if upgrade_first {
                upgrade(world, z) || spawn(world, z)
            } else {
                let built = spawn(world, z);
                built || upgrade(world, z)
            };
            if !ok {
                world.growth.acc[zi] = world.growth.acc[zi].min(0.0);
                break;
            }
        }
    }
}

fn capacity(z: Zone, level: u8, area: u32) -> u32 {
    balance::CAPACITY_PER_CELL[zone_index(z)][(level.clamp(1, 3) - 1) as usize] * area
}

fn free_for(world: &World, c: Cell, z: Zone) -> bool {
    world
        .grid
        .get(c)
        .is_some_and(|cd| cd.zone == Some(z) && cd.occupant == Occupant::Empty)
}

const DIRS4: [Facing; 4] = [Facing::PlusX, Facing::PlusY, Facing::MinusX, Facing::MinusY];

/// Does `c` touch a connected road cell on its `f` side?
fn fronts(world: &World, c: Cell, f: Facing) -> bool {
    let (dx, dy) = f.offset();
    world.cache.connected_road_cell(&world.grid, c.offset(dx, dy))
}

/// Does `c` touch any road cell on one of its four sides?
fn touches_road(world: &World, c: Cell) -> bool {
    DIRS4.iter().any(|f| {
        let (dx, dy) = f.offset();
        world.grid.is_road(c.offset(dx, dy))
    })
}

/// A free second-row cell of zone `z`: zoned, empty, no road beside it.
fn second_row(world: &World, c: Cell, z: Zone) -> bool {
    free_for(world, c, z) && !touches_road(world, c)
}

fn behind(c: Cell, f: Facing) -> Cell {
    let (dx, dy) = f.offset();
    c.offset(-dx, -dy)
}

/// The two sideways directions of a facing.
fn sideways(f: Facing) -> [(i32, i32); 2] {
    let (dx, dy) = f.offset();
    [(-dy, dx), (dy, -dx)]
}

fn rect_of(cells: &[Cell]) -> CellRect {
    let mut r = CellRect::single(cells[0]);
    for &c in &cells[1..] {
        r = CellRect::from_corners(
            Cell::new(r.min.x.min(c.x), r.min.y.min(c.y)),
            Cell::new(r.max.x.max(c.x), r.max.y.max(c.y)),
        );
    }
    r
}

/// Front-row cells of a footprint facing `f`.
pub fn front_cells(r: CellRect, f: Facing) -> Vec<Cell> {
    r.cells()
        .filter(|&c| {
            let (dx, dy) = f.offset();
            !r.contains(c.offset(dx, dy))
        })
        .collect()
}

/// Does a footprint have road frontage on its `facing` side (every front
/// cell touches a road cell)?
pub fn has_frontage(world: &World, r: CellRect, f: Facing) -> bool {
    front_cells(r, f).into_iter().all(|c| {
        let (dx, dy) = f.offset();
        world.grid.is_road(c.offset(dx, dy))
    })
}

/// Lot for a new building of zone `z` on front-row cell `c`: facing, then
/// the footprint (deep when the cell behind is free second row; 2x2 when
/// `wide` and the neighbours allow).
fn plan_lot(world: &mut World, c: Cell, z: Zone, wide: bool) -> Option<(Facing, CellRect)> {
    // Facings with a connected road in front; prefer one with a deep lot
    // behind, then the first in a per-cell rotation (corner lots vary).
    let start = world.rng.below(4) as usize;
    let mut best: Option<(u8, Facing)> = None;
    for k in 0..4 {
        let f = DIRS4[(start + k) % 4];
        if !fronts(world, c, f) {
            continue;
        }
        let score = u8::from(second_row(world, behind(c, f), z));
        if best.is_none_or(|b| score > b.0) {
            best = Some((score, f));
        }
    }
    let (deep, f) = best?;
    let deep = deep == 1;
    let mut cells = vec![c];
    if deep {
        cells.push(behind(c, f));
    }
    if wide && deep {
        let side = sideways(f);
        let pick = world.rng.below(2) as usize;
        for k in 0..2 {
            let (sx, sy) = side[(pick + k) % 2];
            let s = c.offset(sx, sy);
            if free_for(world, s, z) && fronts(world, s, f) && second_row(world, behind(s, f), z) {
                cells.push(s);
                cells.push(behind(s, f));
                break;
            }
        }
    }
    Some((f, rect_of(&cells)))
}

fn spawn(world: &mut World, z: Zone) -> bool {
    let n = world.cache.candidates(&world.grid, world.versions.grid, z).len();
    if n == 0 {
        return false;
    }
    let c = world.cache.candidates(&world.grid, world.versions.grid, z)[world.rng.below(n as u32) as usize];
    if !free_for(world, c, z) {
        return false;
    }
    let zi = zone_index(z);
    let wide = world.rng.below(100) < balance::BIG_FOOTPRINT_PERCENT[zi];
    let Some((facing, footprint)) = plan_lot(world, c, z, wide) else {
        return false;
    };
    let area = (footprint.width() * footprint.height()) as u32;
    let b = Building {
        kind: z,
        footprint,
        level: 1,
        capacity: capacity(z, 1, area),
        occupied: 0,
        facing,
        seed: world.rng.next_u64() as u32,
        created_tick: world.time.tick,
    };
    debug_assert!(
        has_frontage(world, footprint, facing),
        "{footprint:?} {facing:?} has no frontage"
    );
    let id = world.buildings.insert(b);
    for x in footprint.cells() {
        world.grid.get_mut(x).expect("free cell in bounds").occupant = Occupant::Building(id);
    }
    world.growth.meta.insert(id, BuildingMeta::default());
    world.versions.buildings += 1;
    world.versions.grid += 1;
    true
}

/// Grow a building's lot on upgrade: a shallow lot absorbs the free second
/// row behind it; a deep lot at level 2+ widens to 2x2 where free cells
/// beside it allow. Returns the new footprint, if it changed.
fn grow_lot(world: &mut World, id: BuildingId) -> Option<CellRect> {
    let b = &world.buildings[id];
    let (r, f, z) = (b.footprint, b.facing, b.kind);
    let (w, h) = (r.width(), r.height());
    let depth = match f {
        Facing::PlusX | Facing::MinusX => w,
        Facing::PlusY | Facing::MinusY => h,
    };
    let breadth = (w * h) / depth;
    let mut add: Vec<Cell> = Vec::new();
    if depth == 1 && breadth == 1 {
        let c = r.min;
        if second_row(world, behind(c, f), z) {
            add.push(behind(c, f));
        }
    } else if depth == 2 && breadth == 1 && b.level >= 2 {
        let front = front_cells(r, f)[0];
        let side = sideways(f);
        let pick = world.rng.below(2) as usize;
        for k in 0..2 {
            let (sx, sy) = side[(pick + k) % 2];
            let s = front.offset(sx, sy);
            if free_for(world, s, z) && fronts(world, s, f) && second_row(world, behind(s, f), z) {
                add.push(s);
                add.push(behind(s, f));
                break;
            }
        }
    }
    if add.is_empty() {
        return None;
    }
    let mut cells: Vec<Cell> = r.cells().collect();
    cells.extend(add.iter().copied());
    let nr = rect_of(&cells);
    for c in add {
        world.grid.get_mut(c).expect("in bounds").occupant = Occupant::Building(id);
    }
    world.buildings[id].footprint = nr;
    world.versions.grid += 1;
    Some(nr)
}

/// Round-1 saves may hold buildings in a second row (no road in front).
/// Merge each into the shallow lot of the same kind right in front of it
/// (that building's residents / jobs and the absorbed one's move together);
/// any that cannot be merged are kept as they are. Returns the number
/// merged.
pub(crate) fn convert_legacy_lots(world: &mut World) -> u32 {
    let mut merged = 0;
    let ids: Vec<BuildingId> = world.buildings.keys().collect();
    for back_id in ids {
        let Some(back) = world.buildings.get(back_id) else {
            continue;
        };
        if back.footprint.cells().any(|c| touches_road(world, c)) {
            continue;
        }
        if back.footprint.width() != 1 || back.footprint.height() != 1 {
            continue;
        }
        let c = back.footprint.min;
        let kind = back.kind;
        let mut front: Option<BuildingId> = None;
        for f in DIRS4 {
            let (dx, dy) = f.offset();
            let n = c.offset(dx, dy);
            if let Some(crate::grid::CellData {
                occupant: Occupant::Building(fid),
                ..
            }) = world.grid.get(n).copied()
                && let Some(fb) = world.buildings.get(fid)
                && fb.kind == kind
                && fb.facing == f
                && fb.footprint == CellRect::single(n)
                && has_frontage(world, fb.footprint, f)
            {
                front = Some(fid);
                break;
            }
        }
        let Some(fid) = front else { continue };
        let back = world.buildings.remove(back_id).expect("exists");
        world.growth.meta.remove(back_id);
        let fb = &mut world.buildings[fid];
        fb.footprint = rect_of(&[fb.footprint.min, c]);
        fb.capacity += back.capacity;
        fb.occupied += back.occupied;
        fb.level = fb.level.max(back.level);
        world.grid.get_mut(c).expect("in bounds").occupant = Occupant::Building(fid);
        for h in world.households.values_mut() {
            if h.home == back_id {
                h.home = fid;
            }
            if h.job == Some(back_id) {
                h.job = Some(fid);
            }
        }
        merged += 1;
    }
    if merged > 0 {
        world.versions.buildings += 1;
        world.versions.grid += 1;
    }
    world.growth.legacy_lots_merged += merged;
    merged
}

fn upgrade(world: &mut World, z: Zone) -> bool {
    let zi = zone_index(z);
    let now = world.time.tick;
    let eligible: Vec<BuildingId> = world
        .buildings
        .iter()
        .filter(|(_, b)| {
            b.kind == z
                && b.level < balance::MAX_LEVEL[zi]
                && b.occupied as f32 >= b.capacity as f32 * balance::UPGRADE_MIN_FULL
                && now.saturating_sub(b.created_tick) >= balance::UPGRADE_MIN_AGE_TICKS
        })
        .map(|(id, _)| id)
        .collect();
    if eligible.is_empty() {
        return false;
    }
    let id = eligible[world.rng.below(eligible.len() as u32) as usize];
    world.buildings[id].level += 1;
    grow_lot(world, id);
    let b = &mut world.buildings[id];
    let area = (b.footprint.width() * b.footprint.height()) as u32;
    b.capacity = capacity(z, b.level, area).max(b.capacity);
    world.versions.buildings += 1;
    true
}

/// Every hour: track road access and abandon buildings cut off for longer
/// than the grace period. At midnight also count empty / unreachable days
/// and abandon buildings that stayed empty too long.
fn abandon(world: &mut World) {
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    let now = world.time.tick;
    let midnight = world.time.tick_of_day == 0;
    let mut doomed = Vec::new();
    let ids: Vec<BuildingId> = world.buildings.keys().collect();
    for id in ids {
        let b = &world.buildings[id];
        let old_enough = now.saturating_sub(b.created_tick) >= balance::TICKS_PER_DAY as u64;
        let empty = b.occupied == 0 && old_enough;
        let reachable = households::connected(world, b.footprint);
        let m = world.growth.meta.entry(id);
        if reachable {
            m.unreachable_since = None;
        } else if m.unreachable_since.is_none() {
            m.unreachable_since = Some(now);
        }
        if midnight {
            m.empty_days = if empty { m.empty_days.saturating_add(1) } else { 0 };
            m.unreachable_days = if reachable {
                0
            } else {
                m.unreachable_days.saturating_add(1)
            };
        }
        let cut_off_too_long = m
            .unreachable_since
            .is_some_and(|t| now.saturating_sub(t) >= balance::ABANDON_UNREACHABLE_GRACE_TICKS);
        if m.empty_days >= balance::ABANDON_EMPTY_DAYS || cut_off_too_long {
            doomed.push(id);
        }
    }
    remove_buildings(world, &doomed);
    world.growth.abandoned_total += doomed.len() as u32;
}

/// Remove buildings (keeping zoning) and let their households react.
pub(crate) fn remove_buildings(world: &mut World, ids: &[BuildingId]) {
    if ids.is_empty() {
        return;
    }
    for &id in ids {
        if let Some(b) = world.buildings.remove(id) {
            for c in b.footprint.cells() {
                if let Some(cd) = world.grid.get_mut(c)
                    && cd.occupant == Occupant::Building(id)
                {
                    cd.occupant = Occupant::Empty;
                }
            }
        }
        world.growth.meta.remove(id);
    }
    world.versions.buildings += 1;
    world.versions.grid += 1;
    households::buildings_removed(world, ids);
}
