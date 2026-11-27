//! Buildings appearing, densifying and being abandoned on zoned land.
//!
//! # Lots
//!
//! A building stands on cells of one zoning lattice and always fronts a
//! road: the cells along its front side (`facing`, in lattice axes) are in
//! the front row of a block of that road. Blocks are two cells deep, so a
//! zoned block has a front row and a second row. Growth only starts
//! buildings on front-row cells whose road is connected to the highway; the
//! free second-row cell of the same zone right behind is absorbed into a
//! deep 1x2 lot, and at level 2+ a lot may widen to 2x2 into free cells
//! beside it (commercial and industrial lots may start 2x2). Second-row
//! cells that no front lot can absorb stay empty. Corner cells of a shared
//! lattice front two roads; the lot picks one (preferring a deep lot, then
//! a random rotation).
//!
//! Every `GROWTH_INTERVAL_TICKS` each zone accumulates `demand *
//! GROWTH_PER_HOUR` growth events; each event builds one building on a
//! random zoned, empty cell with road access to the highway, or (sometimes,
//! or when no land is left) upgrades a full building one level. Buildings
//! that stayed empty for several midnights, or that have been cut off from
//! the highway for longer than `ABANDON_UNREACHABLE_GRACE_TICKS` (checked
//! every in-game hour), are abandoned and removed (the zoning stays).

use serde::{Deserialize, Serialize};

use crate::blocks::opposite;
use crate::derived::zone_index;
use crate::side::SideTable;
use crate::zoning::CellRef;
use crate::{Building, BuildingId, Facing, Footprint, World, Zone, balance, households};

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
    world.cache.ensure_zoning(&world.zoning);
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

fn free_for(world: &World, c: CellRef, z: Zone) -> bool {
    world.zoning.cell(c).is_some_and(|x| x.free() && x.zone == Some(z))
}

/// Is `c` in the front row of a block on side `f` whose road is connected
/// to the highway?
fn fronts(world: &World, c: CellRef, f: Facing) -> bool {
    world.cache.front_connected(c, f)
}

/// Does `c` have a road in front on any side?
fn touches_road(world: &World, c: CellRef) -> bool {
    world.zoning.cell(c).is_some_and(|x| x.fronts != 0)
}

/// A free second-row cell of zone `z`: zoned, empty, no road in front.
fn second_row(world: &World, c: CellRef, z: Zone) -> bool {
    free_for(world, c, z) && !touches_road(world, c)
}

fn behind(c: CellRef, f: Facing) -> CellRef {
    c.step(opposite(f))
}

/// The two sideways directions of a facing.
fn sideways(f: Facing) -> [Facing; 2] {
    match f {
        Facing::PlusX | Facing::MinusX => [Facing::PlusY, Facing::MinusY],
        Facing::PlusY | Facing::MinusY => [Facing::MinusX, Facing::PlusX],
    }
}

fn rect_of(cells: &[CellRef]) -> ([i32; 2], [i32; 2]) {
    let mut lo = [cells[0].i, cells[0].j];
    let mut hi = lo;
    for c in &cells[1..] {
        lo = [lo[0].min(c.i), lo[1].min(c.j)];
        hi = [hi[0].max(c.i), hi[1].max(c.j)];
    }
    (lo, hi)
}

/// Front-row cells of a footprint facing `f`.
pub fn front_cells(fp: &Footprint, f: Facing) -> Vec<CellRef> {
    fp.cells().filter(|&c| !fp.contains(c.step(f))).collect()
}

/// Does a building have road frontage on its `facing` side (every front
/// cell is in the front row of a block on that side)?
pub fn has_frontage(world: &World, fp: &Footprint, f: Facing) -> bool {
    front_cells(fp, f)
        .into_iter()
        .all(|c| world.zoning.cell(c).is_some_and(|x| x.fronts_on(f)))
}

fn footprint_of(world: &World, cells: &[CellRef], f: Facing) -> Footprint {
    let (lo, hi) = rect_of(cells);
    let l = world.zoning.lattice(cells[0].lattice).expect("cell lattice");
    placed(world, Footprint::new(cells[0].lattice, l, lo, hi, f))
}

/// A lot's rectangle moved clear of every road's reserve. Lots are chosen
/// by cell centres; on a curved road side the straight rectangle over
/// curved cells reaches a little into the reserve on the inside of a bend
/// (up to about a metre), which would make the very road beside it
/// impossible to build again (undo). Rectangles on straight lattices are
/// already clear and stay as they are; a lot that cannot be cleared by a
/// small move (squeezed between roads) keeps its least intruding place.
pub(crate) fn placed(world: &World, fp: Footprint) -> Footprint {
    let (mut into, mut away) = crate::build::reserve_intrusion(world, &fp.corners());
    let mut best = (into, fp);
    let mut cur = fp;
    for _ in 0..6 {
        if into <= 0.01 {
            break;
        }
        let step = into + 0.02;
        cur.center = [cur.center[0] + away[0] * step, cur.center[1] + away[1] * step];
        (into, away) = crate::build::reserve_intrusion(world, &cur.corners());
        if into < best.0 {
            best = (into, cur);
        }
    }
    best.1
}

/// Lot for a new building of zone `z` on front-row cell `c`: facing, then
/// the cells (deep when the cell behind is free second row; 2x2 when
/// `wide` and the neighbours allow; then as many whole rows of free yard
/// behind as the block has).
fn plan_lot(world: &mut World, c: CellRef, z: Zone, wide: bool) -> Option<(Facing, Vec<CellRef>)> {
    // Facings with a connected road in front; prefer one with a deep lot
    // behind, then the first in a per-cell rotation (corner lots vary).
    let start = world.rng.below(4) as usize;
    let mut best: Option<(u8, Facing)> = None;
    for k in 0..4 {
        let f = Facing::ALL[(start + k) % 4];
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
    let mut front = vec![c];
    if wide && deep {
        let side = sideways(f);
        let pick = world.rng.below(2) as usize;
        for k in 0..2 {
            let s = c.step(side[(pick + k) % 2]);
            if free_for(world, s, z) && fronts(world, s, f) && second_row(world, behind(s, f), z) {
                front.push(s);
                break;
            }
        }
    }
    let mut cells = front.clone();
    if deep {
        cells.extend(rows_behind(world, &front, f, z, 1));
    }
    Some((f, cells))
}

/// Whole rows of free second-row cells of zone `z` behind a lot whose
/// front row is `front` (facing `f`), starting `have` rows back, while the
/// lot stays within `balance::BLOCK_DEPTH` rows.
fn rows_behind(world: &World, front: &[CellRef], f: Facing, z: Zone, have: usize) -> Vec<CellRef> {
    let mut out = Vec::new();
    let back = |c: CellRef, n: usize| (0..n).fold(c, |c, _| behind(c, f));
    for depth in have..balance::BLOCK_DEPTH as usize {
        let row: Vec<CellRef> = front.iter().map(|&c| back(c, depth)).collect();
        if !row
            .iter()
            .all(|&c| second_row(world, c, z) && !nearer_other_road(world, c, f, depth))
        {
            break;
        }
        out.extend(row);
    }
    out
}

/// Is cell `c`, `depth` rows behind a lot's front (facing `f`), nearer to
/// another road of its lattice (a road behind it or beside it) than to the
/// lot's own? Such land is left to the lots of that road.
fn nearer_other_road(world: &World, c: CellRef, f: Facing, depth: usize) -> bool {
    Facing::ALL.into_iter().filter(|&g| g != f).any(|g| {
        let mut x = c;
        (1..depth).any(|_| {
            x = x.step(g);
            world.zoning.cell(x).is_some_and(|y| y.fronts_on(g))
        })
    })
}

/// Mark cells as held by a building (or freed, with `None`).
pub(crate) fn claim(world: &mut World, cells: impl Iterator<Item = CellRef>, b: Option<BuildingId>) {
    for c in cells {
        let Some(l) = world.zoning.lattice_mut(c.lattice) else {
            continue;
        };
        let e = l.cells.entry((c.i, c.j)).or_default();
        e.building = b;
        if b.is_none() && !e.covered && e.zone.is_none() {
            l.cells.remove(&(c.i, c.j));
        }
    }
}

/// Mark the cells of building `id` (for tests and tools that insert
/// buildings by hand), and block other lattices' cells under it.
pub fn place_cells(world: &mut World, id: BuildingId) {
    let Some(b) = world.buildings.get(id) else { return };
    let fp = b.footprint;
    claim(world, fp.cells(), Some(id));
    refresh_around(world, &[fp]);
}

/// Recompute the cells around footprints (other lattices' cells under a
/// building are blocked, and free again when it goes).
fn refresh_around(world: &mut World, fps: &[Footprint]) {
    let boxes: Vec<([f32; 2], [f32; 2])> = fps
        .iter()
        .map(|f| {
            let c = f.corners();
            let lo = c
                .iter()
                .fold([f32::INFINITY; 2], |a, p| [a[0].min(p[0]), a[1].min(p[1])]);
            let hi = c
                .iter()
                .fold([f32::NEG_INFINITY; 2], |a, p| [a[0].max(p[0]), a[1].max(p[1])]);
            (lo, hi)
        })
        .collect();
    if boxes.is_empty() {
        return;
    }
    // Only cells of other lattices can change; a building on lattice 0 next
    // to lattice-0 cells only needs the version bump.
    let multi = world.zoning.lattices().nth(1).is_some();
    if multi {
        let doomed = crate::blocks::refresh_boxes(world, &boxes);
        debug_assert!(doomed.is_empty(), "a building over a road");
    } else {
        world.versions.grid += 1;
    }
}

fn spawn(world: &mut World, z: Zone) -> bool {
    let n = world.cache.candidates(&world.zoning, z).len();
    if n == 0 {
        return false;
    }
    // A random free candidate: a few random tries (the list also holds
    // built-up cells), then the next free one after the last try.
    let mut pick = None;
    let mut at = 0;
    for _ in 0..24 {
        at = world.rng.below(n as u32) as usize;
        let c = world.cache.candidates_built(z)[at];
        if free_for(world, c, z) {
            pick = Some(c);
            break;
        }
    }
    let list = world.cache.candidates_built(z);
    let Some(c) = pick.or_else(|| (0..n).map(|k| list[(at + k) % n]).find(|&c| free_for(world, c, z))) else {
        return false;
    };
    let zi = zone_index(z);
    let wide = world.rng.below(100) < balance::BIG_FOOTPRINT_PERCENT[zi];
    let Some((facing, cells)) = plan_lot(world, c, z, wide) else {
        return false;
    };
    let footprint = footprint_of(world, &cells, facing);
    let b = Building {
        kind: z,
        footprint,
        level: 1,
        capacity: capacity(z, 1, footprint.built_area()),
        occupied: 0,
        facing,
        seed: world.rng.next_u64() as u32,
        created_tick: world.time.tick,
    };
    debug_assert!(
        has_frontage(world, &footprint, facing),
        "{footprint:?} {facing:?} has no frontage"
    );
    let id = world.buildings.insert(b);
    claim(world, cells.into_iter(), Some(id));
    world.growth.meta.insert(id, BuildingMeta::default());
    world.versions.buildings += 1;
    refresh_around(world, &[footprint]);
    crate::errands::stock_up(world, id);
    true
}

/// Grow a building's lot on upgrade: a shallow lot absorbs the free rows
/// behind it; a one-wide lot at level 2+ widens to two where the free
/// cells beside it allow (as deep as it is). Returns the new footprint, if
/// it changed.
fn grow_lot(world: &mut World, id: BuildingId) -> Option<Footprint> {
    let b = &world.buildings[id];
    let (fp, f, z) = (b.footprint, b.facing, b.kind);
    let (breadth, depth) = (fp.size[0] as usize, fp.size[1] as usize);
    let front = front_cells(&fp, f);
    let mut add: Vec<CellRef> = Vec::new();
    if depth == 1 {
        add = rows_behind(world, &front, f, z, 1);
    } else if breadth == 1 && b.level >= 2 {
        let side = sideways(f);
        let pick = world.rng.below(2) as usize;
        for k in 0..2 {
            let s = front[0].step(side[(pick + k) % 2]);
            let col: Vec<CellRef> = (0..depth)
                .scan(s, |c, _| {
                    let here = *c;
                    *c = behind(here, f);
                    Some(here)
                })
                .collect();
            if free_for(world, s, z) && fronts(world, s, f) && col[1..].iter().all(|&c| second_row(world, c, z)) {
                add = col;
                break;
            }
        }
    }
    if add.is_empty() {
        return None;
    }
    let mut cells: Vec<CellRef> = fp.cells().collect();
    cells.extend(add.iter().copied());
    let nf = footprint_of(world, &cells, f);
    claim(world, add.into_iter(), Some(id));
    world.buildings[id].footprint = nf;
    refresh_around(world, &[nf]);
    Some(nf)
}

/// After zoning blocks were made again (an older save): buildings standing
/// on cells no block covers any more move onto the free lot of the new
/// cells under their front, as wide and up to as deep as they were (at
/// least the built rows), keeping their people. Those that do not fit stay
/// where they are. Returns how many moved.
pub(crate) fn reseat(world: &mut World) -> u32 {
    let ids: Vec<BuildingId> = world.buildings.keys().collect();
    let mut moved = 0;
    for id in ids {
        let b = &world.buildings[id];
        let (fp, kind) = (b.footprint, b.kind);
        if fp.cells().all(|c| world.zoning.cell(c).is_some_and(|x| x.covered)) {
            continue;
        }
        let Some(q) = world.zoning.lot_quad(&fp) else { continue };
        let fd = b.front_dir();
        let probe = [
            (q[0][0] + q[1][0]) * 0.5 - fd[0] * 2.0,
            (q[0][1] + q[1][1]) * 0.5 - fd[1] * 2.0,
        ];
        let Some(c) = world.zoning.cell_at(probe).filter(|c| c.lattice != fp.lattice) else {
            continue;
        };
        let Some(cell) = world.zoning.cell(c).copied() else {
            continue;
        };
        if !cell.free() || cell.fronts == 0 {
            continue;
        }
        let Some(l) = world.zoning.lattice(c.lattice) else {
            continue;
        };
        let x = c.i as f32 + 0.5;
        let Some(f) = Facing::ALL
            .into_iter()
            .filter(|&f| cell.fronts_on(f))
            .max_by(|a, b| geom_dot(l.dir_at(x, *a), fd).total_cmp(&geom_dot(l.dir_at(x, *b), fd)))
        else {
            continue;
        };
        let usable_free = |w: &World, c: CellRef| w.zoning.cell(c).is_some_and(|x| x.free());
        let mut front = vec![c];
        if fp.size[0] >= 2 {
            let ctr = b.center();
            let side = sideways(f)
                .into_iter()
                .map(|s| c.step(s))
                .filter(|&s| usable_free(world, s) && world.zoning.cell(s).is_some_and(|x| x.fronts_on(f)))
                .min_by(|a, b| {
                    let da = world
                        .zoning
                        .cell_center(*a)
                        .map_or(f32::MAX, |p| crate::geom::dist(p, ctr));
                    let db = world
                        .zoning
                        .cell_center(*b)
                        .map_or(f32::MAX, |p| crate::geom::dist(p, ctr));
                    da.total_cmp(&db)
                });
            match side {
                Some(s) => front.push(s),
                None => continue,
            }
        }
        let mut cells = front.clone();
        let want = fp.size[1] as usize;
        for depth in 1..want {
            let row: Vec<CellRef> = front.iter().map(|&c| (0..depth).fold(c, |c, _| behind(c, f))).collect();
            if !row
                .iter()
                .all(|&c| usable_free(world, c) && world.zoning.cell(c).is_some_and(|x| x.fronts == 0))
            {
                break;
            }
            cells.extend(row);
        }
        let rows = cells.len() / front.len();
        if rows < want.min(balance::BUILT_DEPTH as usize) {
            continue;
        }
        // Move: free the old cells, zone and take the new ones.
        let old: Vec<CellRef> = fp.cells().collect();
        claim(world, old.into_iter(), None);
        for &n in &cells {
            if let Some(x) = world.zoning.cell_mut(n) {
                x.zone = Some(kind);
            }
        }
        let nf = footprint_of(world, &cells, f);
        claim(world, cells.into_iter(), Some(id));
        let b = &mut world.buildings[id];
        b.footprint = nf;
        b.facing = f;
        refresh_around(world, &[fp, nf]);
        moved += 1;
    }
    if moved > 0 {
        world.versions.buildings += 1;
        world.zoning.zones_version += 1;
    }
    moved
}

fn geom_dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
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
        let fp = back.footprint;
        if fp.cells().any(|c| touches_road(world, c)) || fp.area() != 1 {
            continue;
        }
        let c = fp.cells().next().expect("one cell");
        let kind = back.kind;
        let mut front: Option<BuildingId> = None;
        for f in Facing::ALL {
            let n = c.step(f);
            if let Some(fid) = world.zoning.cell(n).and_then(|x| x.building)
                && let Some(fb) = world.buildings.get(fid)
                && fb.kind == kind
                && fb.facing == f
                && fb.footprint.area() == 1
                && fb.footprint.contains(n)
                && has_frontage(world, &fb.footprint, f)
            {
                front = Some(fid);
                break;
            }
        }
        let Some(fid) = front else { continue };
        let back = world.buildings.remove(back_id).expect("exists");
        world.growth.meta.remove(back_id);
        let fcell = world.buildings[fid].footprint.cells().next().expect("one cell");
        let facing = world.buildings[fid].facing;
        let nf = footprint_of(world, &[fcell, c], facing);
        let fb = &mut world.buildings[fid];
        fb.footprint = nf;
        fb.capacity += back.capacity;
        fb.occupied += back.occupied;
        fb.level = fb.level.max(back.level);
        claim(world, std::iter::once(c), Some(fid));
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
    b.capacity = capacity(z, b.level, b.footprint.built_area()).max(b.capacity);
    world.versions.buildings += 1;
    // Better jobs pay better.
    crate::people::retier(world, id);
    true
}

/// Every hour: track road access and abandon buildings cut off for longer
/// than the grace period. At midnight also count empty / unreachable days
/// and abandon buildings that stayed empty too long.
fn abandon(world: &mut World) {
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    world.cache.ensure_zoning(&world.zoning);
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

/// Is there a road connected to the highway next to a footprint (its own
/// road, or any within reach)?
pub(crate) fn footprint_connected(world: &World, f: crate::Footprint) -> bool {
    f.cells().any(|c| world.cache.cell_connected(c))
}

/// Remove buildings (keeping zoning) and let their households react.
pub(crate) fn remove_buildings(world: &mut World, ids: &[BuildingId]) {
    if ids.is_empty() {
        return;
    }
    let mut fps = Vec::new();
    for &id in ids {
        if let Some(b) = world.buildings.remove(id) {
            let held: Vec<CellRef> = b
                .footprint
                .cells()
                .filter(|&c| world.zoning.cell(c).is_some_and(|x| x.building == Some(id)))
                .collect();
            claim(world, held.into_iter(), None);
            fps.push(b.footprint);
        }
        world.growth.meta.remove(id);
    }
    world.versions.buildings += 1;
    refresh_around(world, &fps);
    world.versions.grid += 1;
    households::buildings_removed(world, ids);
}
