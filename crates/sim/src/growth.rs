//! Buildings appearing, densifying and being abandoned on zoned land.
//!
//! Every `GROWTH_INTERVAL_TICKS` each zone accumulates `demand *
//! GROWTH_PER_HOUR` growth events; each event builds one building on a
//! random zoned, empty cell with road access to the highway, or (sometimes,
//! or when no land is left) upgrades a full building one level. At
//! midnight buildings that stayed empty or lost their road connection for
//! too long are abandoned and removed (the zoning stays).

use serde::{Deserialize, Serialize};

use crate::derived::{rect_center, zone_index};
use crate::geom::{self, Cell, CellRect};
use crate::grid::Occupant;
use crate::side::SideTable;
use crate::{Building, BuildingId, Facing, World, Zone, balance, households};

/// Per-building bookkeeping not needed by renderers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BuildingMeta {
    /// Consecutive midnights with nobody in it.
    pub empty_days: u8,
    /// Consecutive midnights without a road connection to the highway.
    pub unreachable_days: u8,
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
}

fn demand_of(world: &World, z: Zone) -> f32 {
    match z {
        Zone::Residential => world.demand.residential,
        Zone::Commercial => world.demand.commercial,
        Zone::Industrial => world.demand.industrial,
    }
}

pub(crate) fn tick(world: &mut World) {
    if world.time.tick_of_day == 0 {
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

/// Facing towards the nearest road point.
fn facing_for(world: &World, r: CellRect) -> Facing {
    let centre = rect_center(r);
    let Some(a) = world.cache.access_near(&world.roads, centre) else {
        return Facing::default();
    };
    let Some(p) = crate::derived::access_pos(&world.roads, a) else {
        return Facing::default();
    };
    let d = geom::sub(p, centre);
    let mut best = (f32::MIN, Facing::default());
    for f in Facing::ALL {
        let (dx, dy) = f.offset();
        let score = d[0] * dx as f32 + d[1] * dy as f32;
        if score > best.0 + 1e-3 {
            best = (score, f);
        }
    }
    best.1
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
    let mut footprint = CellRect::single(c);
    if world.rng.below(100) < balance::BIG_FOOTPRINT_PERCENT[zi] {
        let start = world.rng.below(4);
        for k in 0..4 {
            let (dx, dy) = [(0, 0), (-1, 0), (0, -1), (-1, -1)][((start + k) % 4) as usize];
            let r = CellRect::from_corners(c.offset(dx, dy), c.offset(dx + 1, dy + 1));
            if r.cells().all(|x| free_for(world, x, z)) {
                footprint = r;
                break;
            }
        }
    }
    let area = (footprint.width() * footprint.height()) as u32;
    let b = Building {
        kind: z,
        footprint,
        level: 1,
        capacity: capacity(z, 1, area),
        occupied: 0,
        facing: facing_for(world, footprint),
        seed: world.rng.next_u64() as u32,
        created_tick: world.time.tick,
    };
    let id = world.buildings.insert(b);
    for x in footprint.cells() {
        world.grid.get_mut(x).expect("free cell in bounds").occupant = Occupant::Building(id);
    }
    world.growth.meta.insert(id, BuildingMeta::default());
    world.versions.buildings += 1;
    world.versions.grid += 1;
    true
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
    let b = &mut world.buildings[id];
    b.level += 1;
    let area = (b.footprint.width() * b.footprint.height()) as u32;
    b.capacity = capacity(z, b.level, area).max(b.capacity);
    world.versions.buildings += 1;
    true
}

/// Midnight: count empty / unreachable days and remove abandoned buildings.
fn abandon(world: &mut World) {
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    let now = world.time.tick;
    let mut doomed = Vec::new();
    let ids: Vec<BuildingId> = world.buildings.keys().collect();
    for id in ids {
        let b = &world.buildings[id];
        let old_enough = now.saturating_sub(b.created_tick) >= balance::TICKS_PER_DAY as u64;
        let empty = b.occupied == 0 && old_enough;
        let reachable = households::connected(world, b.footprint);
        let m = world.growth.meta.entry(id);
        m.empty_days = if empty { m.empty_days.saturating_add(1) } else { 0 };
        m.unreachable_days = if reachable {
            0
        } else {
            m.unreachable_days.saturating_add(1)
        };
        if m.empty_days >= balance::ABANDON_EMPTY_DAYS || m.unreachable_days >= balance::ABANDON_UNREACHABLE_DAYS {
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
