//! Scripted cities, built only through [`Command`]s. Used by tests, by the
//! `headless` example for balancing, and usable by dev tooling.
//!
//! The highway stub ends at cell `(11, 128)`. All layouts here grow east of
//! it around the avenue `y = 128`.

use crate::{Cell, Command, World, Zone};

fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

/// Build a straight road; returns false if it failed (money, obstacles).
pub fn road(w: &mut World, a: (i32, i32), b: (i32, i32)) -> bool {
    w.apply(Command::BuildRoad {
        from: c(a.0, a.1),
        to: c(b.0, b.1),
    })
    .is_ok()
}

/// Zone a rectangle (only eligible cells change).
pub fn zone(w: &mut World, a: (i32, i32), b: (i32, i32), z: Zone) {
    let _ = w.apply(Command::SetZone {
        min: c(a.0, a.1),
        max: c(b.0, b.1),
        zone: Some(z),
    });
}

/// Run a scripted layout with its construction paid for: money is the same
/// afterwards as before. The static layouts ([`two_districts`]) build a
/// whole town at once, which costs more than a new game starts with; use
/// this to compare them on traffic and running costs.
pub fn prebuilt(w: &mut World, build: impl FnOnce(&mut World)) {
    let money = w.economy.money;
    w.economy.money = i64::MAX / 4;
    build(w);
    w.economy.money = money;
}

/// Rows (y) of the east-west streets of the static district layout.
const NORTH_RIBS: [i32; 5] = [123, 118, 113, 108, 103];
const SOUTH_RIBS: [i32; 5] = [133, 138, 143, 148, 153];
const WEST_X: i32 = 20;
const EAST_X: i32 = 80;

/// A two-district city: housing north of the avenue, shops and industry
/// south of it. Every street hangs off a spine at `x = 20`.
///
/// * `grid = true`: north-south streets every 10 cells cross both districts
///   and the avenue — a sensible grid with many alternatives.
/// * `grid = false`: the spines are the only links, so all commuter traffic
///   passes the single junction `(20, 128)` — a deliberately bad layout.
///
/// Zoning is identical up to the few cells the extra grid streets occupy.
/// The roads cost more than `STARTING_MONEY`: build it inside [`prebuilt`]
/// unless the world has the money.
pub fn two_districts(w: &mut World, grid: bool) {
    road(w, (11, 128), (EAST_X + 5, 128));
    road(w, (WEST_X, NORTH_RIBS[4]), (WEST_X, SOUTH_RIBS[4]));
    for y in NORTH_RIBS.into_iter().chain(SOUTH_RIBS) {
        road(w, (WEST_X, y), (EAST_X, y));
    }
    if grid {
        for x in (WEST_X + 10..=EAST_X).step_by(10) {
            road(w, (x, NORTH_RIBS[4]), (x, SOUTH_RIBS[4]));
        }
    }
    zone(w, (WEST_X - 2, NORTH_RIBS[4] - 2), (EAST_X + 2, 126), Zone::Residential);
    zone(w, (WEST_X - 2, 130), (EAST_X + 2, 137), Zone::Commercial);
    zone(w, (WEST_X - 2, 139), (EAST_X + 2, SOUTH_RIBS[4] + 2), Zone::Industrial);
    if !grid {
        // Keep the cells the grid streets would use free (and unzoned), so
        // both layouts have exactly the same zoning and the fix can be built.
        for x in (WEST_X + 10..=EAST_X).step_by(10) {
            let _ = w.apply(Command::SetZone {
                min: c(x, NORTH_RIBS[4]),
                max: c(x, SOUTH_RIBS[4]),
                zone: None,
            });
        }
    }
}

/// Fix for the bad layout: link the far ends of all streets and add a
/// middle north-south street, giving traffic alternatives to the one
/// junction.
pub fn add_connections(w: &mut World) {
    road(w, (EAST_X, NORTH_RIBS[4]), (EAST_X, SOUTH_RIBS[4]));
    road(w, (50, NORTH_RIBS[4]), (50, SOUTH_RIBS[4]));
}

/// A block of the progressive grid: `x0..=x0+10`, `y0..=y0+5` (cells), roads
/// on its border, the inside zoned.
#[derive(Debug, Clone, Copy)]
struct Block {
    x0: i32,
    y0: i32,
}

impl Block {
    fn build(&self, w: &mut World, z: Zone) -> bool {
        let (x0, y0, x1, y1) = (self.x0, self.y0, self.x0 + 10, self.y0 + 5);
        let ok = road(w, (x0, y0), (x1, y0)) && road(w, (x0, y1), (x1, y1));
        let ok = ok && road(w, (x0, y0), (x0, y1)) && road(w, (x1, y0), (x1, y1));
        if ok {
            zone(w, (x0 + 1, y0 + 1), (x1 - 1, y1 - 1), z);
        }
        ok
    }

    /// Rough cost if nothing were built yet.
    fn max_cost() -> i64 {
        30 * crate::balance::ROAD_COST_PER_CELL
    }
}

/// A scripted "sensible player": grows a grid of 10x5 blocks east of the
/// highway, following the demand bars. Each in-game hour, for every zone
/// type whose demand is clearly positive and that has little free zoned
/// land left, it builds and zones the next block for that type (if it can
/// afford it). Rows of housing, shops and industry alternate on both sides
/// of the avenue so every neighbourhood has jobs nearby; the city grows east
/// column by column, then outwards.
#[derive(Debug, Clone)]
pub struct GridPlayer {
    /// Next block index per zone `[R, C, I]`.
    next: [usize; 3],
    lists: [Vec<Block>; 3],
    /// Zone types this player uses.
    pub zones: Vec<Zone>,
    /// Blocks built so far.
    pub blocks_built: u32,
    /// Ticks during which the player wanted to build its next block but
    /// could not afford it (summed over all such stretches).
    pub money_wait_ticks: u64,
    /// Longest single stretch of such waiting, ticks.
    pub longest_money_wait_ticks: u64,
    /// Start of the current stretch of waiting for money, if any.
    waiting_since: Option<u64>,
}

impl GridPlayer {
    pub fn new(zones: &[Zone]) -> GridPlayer {
        let cols = 9;
        // Rows `(index, north)`, filled column by column (east-growing band).
        let order_mixed = |rows: &[(i32, bool)]| {
            let mut v: Vec<(i32, Block)> = Vec::new();
            for (k, &(row, north)) in rows.iter().enumerate() {
                for j in 0..cols {
                    let x0 = 20 + 10 * j;
                    let y0 = if north { 128 - 5 * (row + 1) } else { 128 + 5 * row };
                    let band = row / 3;
                    v.push((band * 1000 + j * 10 + k as i32, Block { x0, y0 }));
                }
            }
            v.sort_by_key(|(d, b)| (*d, b.x0, b.y0));
            v.into_iter().map(|(_, b)| b).collect::<Vec<_>>()
        };
        // Mixed neighbourhoods: every housing row is next to a row of jobs.
        GridPlayer {
            next: [0; 3],
            lists: [
                order_mixed(&[
                    (0, true),
                    (0, false),
                    (2, true),
                    (2, false),
                    (3, true),
                    (5, true),
                    (5, false),
                    (6, true),
                ]),
                order_mixed(&[(1, false), (4, true), (7, false)]),
                order_mixed(&[(1, true), (3, false), (4, false), (6, false), (7, true)]),
            ],
            zones: zones.to_vec(),
            blocks_built: 0,
            money_wait_ticks: 0,
            longest_money_wait_ticks: 0,
            waiting_since: None,
        }
    }

    /// Free zoned, empty cells per zone.
    fn free_cells(w: &World) -> [u32; 3] {
        let mut n = [0; 3];
        for cd in &w.grid.cells {
            if cd.occupant == crate::Occupant::Empty
                && let Some(z) = cd.zone
            {
                n[crate::derived::zone_index(z)] += 1;
            }
        }
        n
    }

    /// Call every tick; acts once per in-game hour.
    pub fn act(&mut self, w: &mut World) {
        if !w.time.tick.is_multiple_of(crate::balance::TICKS_PER_HOUR as u64) {
            return;
        }
        if w.roads.segments.len() == 1 {
            road(w, (11, 128), (20, 128));
        }
        let free = Self::free_cells(w);
        let d = [w.demand.residential, w.demand.commercial, w.demand.industrial];
        let mut short_of_money = false;
        for z in self.zones.clone() {
            let zi = crate::derived::zone_index(z);
            // Always keep a little land of each used type available early on.
            let want = d[zi] > 0.1 || (free[zi] == 0 && self.next[zi] == 0);
            let Some(b) = self.lists[zi].get(self.next[zi]).copied() else {
                continue;
            };
            if !want || free[zi] >= 24 {
                continue;
            }
            if w.economy.money <= Block::max_cost() + 1_000 {
                short_of_money = true;
            } else if b.build(w, z) {
                self.next[zi] += 1;
                self.blocks_built += 1;
            }
        }
        let now = w.time.tick;
        match (short_of_money, self.waiting_since) {
            (true, None) => self.waiting_since = Some(now),
            (false, Some(t0)) => {
                self.waiting_since = None;
                self.money_wait_ticks += now - t0;
            }
            _ => {}
        }
        if let Some(t0) = self.waiting_since {
            self.longest_money_wait_ticks = self.longest_money_wait_ticks.max(now - t0);
        }
    }

    /// Lay out the next `blocks[z]` blocks of each zone type `[R, C, I]`
    /// right now, paying for them (a player who builds a whole district on
    /// day one instead of following demand). Stops at the first block it
    /// cannot afford; returns the number built. `act` then carries on from
    /// there.
    pub fn build_ahead(&mut self, w: &mut World, blocks: [usize; 3]) -> u32 {
        let mut built = 0;
        if w.roads.segments.len() == 1 {
            road(w, (11, 128), (20, 128));
        }
        // Interleave the zone types so a short budget still gives a mix.
        for round in 0..blocks.into_iter().max().unwrap_or(0) {
            for z in self.zones.clone() {
                let zi = crate::derived::zone_index(z);
                if round >= blocks[zi] {
                    continue;
                }
                let Some(b) = self.lists[zi].get(self.next[zi]).copied() else {
                    continue;
                };
                if !b.build(w, z) {
                    return built;
                }
                self.next[zi] += 1;
                self.blocks_built += 1;
                built += 1;
            }
        }
        built
    }

    /// Ticks the player has been waiting for money for its next block, 0
    /// when it is not waiting.
    pub fn waiting_for_money_ticks(&self, w: &World) -> u64 {
        self.waiting_since.map_or(0, |t0| w.time.tick - t0)
    }
}

/// An over-eager builder: lays a dense mesh of roads far beyond what the
/// city needs, then zones a little housing. Paying for it, it stops when
/// the money runs out (a few hundred cells); inside [`prebuilt`] it lays
/// the whole ~9,000-cell mesh, whose upkeep the town can never carry.
pub fn overbuilt(w: &mut World) {
    road(w, (11, 128), (200, 128));
    for x in (20..=200).step_by(6) {
        road(w, (x, 40), (x, 216));
    }
    for y in (40..=216).step_by(8) {
        road(w, (20, y), (200, y));
    }
    zone(w, (20, 118), (60, 138), Zone::Residential);
    zone(w, (61, 118), (80, 138), Zone::Commercial);
    zone(w, (81, 118), (100, 138), Zone::Industrial);
}
