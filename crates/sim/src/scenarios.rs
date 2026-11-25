//! Scripted cities, built only through [`Command`]s. Used by tests, by the
//! `headless` example for balancing, and usable by dev tooling.
//!
//! The highway stub ends at cell `(11, 128)`. All layouts here grow east of
//! it around the avenue `y = 128`.

use crate::{Cell, Command, RoadKind, World, Zone};

fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

/// Build a straight street; returns false if it failed (money, obstacles).
pub fn road(w: &mut World, a: (i32, i32), b: (i32, i32)) -> bool {
    road_of(w, a, b, RoadKind::Street)
}

/// Build (or upgrade to) a straight avenue.
pub fn avenue(w: &mut World, a: (i32, i32), b: (i32, i32)) -> bool {
    road_of(w, a, b, RoadKind::Avenue)
}

/// Build a straight road of `kind`.
pub fn road_of(w: &mut World, a: (i32, i32), b: (i32, i32), kind: RoadKind) -> bool {
    w.apply(Command::BuildRoad {
        from: c(a.0, a.1),
        to: c(b.0, b.1),
        kind,
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

/// How a [`GridPlayer`] lays out its roads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Streets only (the round-1 player).
    Streets,
    /// A road hierarchy: the trunk `y = 128` and every third block row
    /// (`y = 113, 143, ...`), and every second block column (`x = 20, 40,
    /// 60, ...`) are avenues; the streets inside the blocks connect to them.
    Hierarchy,
    /// Everything hangs off one spine at `x = 20`: block rows are linked
    /// only through it (no north-south streets elsewhere), so all traffic
    /// funnels through the junction `(20, 128)`.
    SingleArtery,
}

impl Layout {
    fn horizontal(self, y: i32) -> RoadKind {
        match self {
            Layout::Hierarchy if (y - 128).rem_euclid(15) == 0 => RoadKind::Avenue,
            _ => RoadKind::Street,
        }
    }

    fn vertical(self, x: i32) -> Option<RoadKind> {
        match self {
            Layout::Streets => Some(RoadKind::Street),
            Layout::Hierarchy if (x - 20) % 20 == 0 => Some(RoadKind::Avenue),
            Layout::Hierarchy => Some(RoadKind::Street),
            Layout::SingleArtery => (x == 20).then_some(RoadKind::Street),
        }
    }
}

/// A block of the progressive grid: `x0..=x0+10`, `y0..=y0+5` (cells), roads
/// on its border, the inside zoned.
#[derive(Debug, Clone, Copy)]
struct Block {
    x0: i32,
    y0: i32,
}

impl Block {
    fn build(&self, w: &mut World, z: Zone, layout: Layout) -> bool {
        let (x0, y0, x1, y1) = (self.x0, self.y0, self.x0 + 10, self.y0 + 5);
        let mut ok = road_of(w, (x0, y0), (x1, y0), layout.horizontal(y0))
            && road_of(w, (x0, y1), (x1, y1), layout.horizontal(y1));
        for x in [x0, x1] {
            if let Some(k) = layout.vertical(x) {
                ok = ok && road_of(w, (x, y0), (x, y1), k);
            }
        }
        if ok {
            zone(w, (x0 + 1, y0 + 1), (x1 - 1, y1 - 1), z);
        }
        ok
    }

    /// Rough cost if nothing were built yet.
    fn max_cost(layout: Layout) -> i64 {
        match layout {
            Layout::Hierarchy => 20 * crate::balance::ROAD_COST_PER_CELL + 10 * crate::balance::AVENUE_COST_PER_CELL,
            _ => 30 * crate::balance::ROAD_COST_PER_CELL,
        }
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
    /// Road layout.
    pub layout: Layout,
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
            layout: Layout::Streets,
            blocks_built: 0,
            money_wait_ticks: 0,
            longest_money_wait_ticks: 0,
            waiting_since: None,
        }
    }

    /// The sensible player with a road hierarchy (avenues as arterials,
    /// streets inside the blocks), all zone types.
    pub fn hierarchy() -> GridPlayer {
        GridPlayer {
            layout: Layout::Hierarchy,
            ..GridPlayer::new(&Zone::ALL)
        }
    }

    /// Same blocks, but every row hangs off one spine (all zone types).
    pub fn single_artery() -> GridPlayer {
        GridPlayer {
            layout: Layout::SingleArtery,
            ..GridPlayer::new(&Zone::ALL)
        }
    }

    /// The road from the highway stub to the first block (re-laid if it
    /// was bulldozed; costs nothing when it is there).
    fn link(&self, w: &mut World) {
        road_of(w, (11, 128), (20, 128), self.layout.horizontal(128));
    }

    /// Free zoned, empty cells per zone.
    fn free_cells(w: &World) -> [u32; 3] {
        let mut n = [0; 3];
        for (_, cd) in w.zoning.cells() {
            if cd.free()
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
        self.link(w);
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
            if w.economy.money <= Block::max_cost(self.layout) + 1_000 {
                short_of_money = true;
            } else if b.build(w, z, self.layout) {
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
        self.link(w);
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
                if !b.build(w, z, self.layout) {
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

// ---------------------------------------------------------------- organic ----

/// Build a road along `c`; returns false if it was refused.
pub fn curve_road(w: &mut World, c: crate::Curve, kind: RoadKind) -> bool {
    w.apply(Command::BuildCurve { curve: c, kind }).is_ok()
}

/// Zone the eligible cells inside a convex quad.
pub fn zone_quad(w: &mut World, quad: [[f32; 2]; 4], z: Zone) {
    let _ = w.apply(Command::ZoneArea { quad, zone: Some(z) });
}

fn add(a: [f32; 2], b: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] + b[0] * k, a[1] + b[1] * k]
}

fn dir_deg(deg: f32) -> [f32; 2] {
    let r = deg.to_radians();
    [libm::cosf(r), libm::sinf(r)]
}

/// Where a straight probe from `p` along `d` first meets an existing road
/// (within `reach` metres), if anywhere.
pub fn first_hit(w: &World, p: [f32; 2], d: [f32; 2], reach: f32) -> Option<[f32; 2]> {
    let probe = crate::Curve::straight(add(p, d, 0.5), add(p, d, reach));
    let mut best: Option<(f32, [f32; 2])> = None;
    for seg in w.roads.segments.values() {
        for (dp, _, q) in probe.intersections(&seg.curve) {
            if best.is_none_or(|b| dp < b.0) {
                best = Some((dp, q));
            }
        }
    }
    best.map(|b| b.1)
}

/// A town laid out only with world-space commands: a curved main avenue
/// from the highway, a district on a street grid turned by 17° hanging off
/// it, a ring road with streets across it, a cul-de-sac estate on curved
/// streets, and an industrial quarter by the highway. Zoned for about
/// 4,000 residents with their jobs. The roads cost more than a new game's
/// money: build it inside [`prebuilt`]. Returns the road commands that
/// were refused, with why (empty when the layout is sound).
pub fn organic(w: &mut World) -> Vec<(crate::Curve, crate::CommandError)> {
    use crate::Curve;
    let mut refused = Vec::new();
    let mut road = |w: &mut World, c: Curve, k: RoadKind| {
        if let Err(e) = w.apply(Command::BuildCurve { curve: c, kind: k }) {
            refused.push((c, e));
        }
    };
    let av = RoadKind::Avenue;
    let st = RoadKind::Street;
    let start = World::highway_entry_cell()
        .offset(crate::balance::HIGHWAY_STUB_CELLS - 1, 0)
        .center();
    let y0 = start[1];
    // The main avenue: straight off the highway, then a long S.
    let p1 = [236.0, y0];
    road(w, Curve::straight(start, p1), av);
    let p2 = [540.0, y0 - 76.0];
    let d2 = dir_deg(-20.0);
    road(w, Curve::from_tangents(p1, [1.0, 0.0], p2, d2).expect("S part 1"), av);
    let p3 = [860.0, y0 - 76.0];
    let d3 = dir_deg(20.0);
    road(w, Curve::from_tangents(p2, d2, p3, d3).expect("S part 2"), av);
    let p4 = [1160.0, y0 + 6.0];
    road(w, Curve::from_tangents(p3, d3, p4, [1.0, 0.0]).expect("S part 3"), av);
    road(w, Curve::straight(p4, [1400.0, y0 + 6.0]), av);

    // The turned grid north of the avenue: rows along u, columns along v.
    let th = 17.0f32;
    let (u, v) = (dir_deg(th), dir_deg(th + 90.0));
    let o = [330.0, 560.0];
    let (cols, rows) = (5, 6);
    let (bw, bh) = (80.0, 40.0);
    for m in 0..rows {
        let a = add(o, v, m as f32 * bh);
        road(w, Curve::straight(a, add(a, u, (cols - 1) as f32 * bw)), st);
    }
    for k in 0..cols {
        let a = add(o, u, k as f32 * bw);
        let b = add(a, v, (rows - 1) as f32 * bh);
        road(w, Curve::straight(a, b), st);
        // On down to the avenue (a T at an odd angle).
        if let Some(q) = first_hit(w, b, v, 400.0) {
            road(w, Curve::straight(b, q), st);
        }
    }

    // The ring south-east of the avenue, with streets across it.
    let rc = [760.0, 1270.0];
    let r = 120.0;
    let ring_pt = |deg: f32| add(rc, dir_deg(deg), r);
    for q in 0..4 {
        let (a0, a1) = (q as f32 * 90.0, (q + 1) as f32 * 90.0);
        let c = Curve::from_tangents(ring_pt(a0), dir_deg(a0 + 90.0), ring_pt(a1), dir_deg(a1 + 90.0))
            .expect("ring quarter");
        road(w, c, st);
    }
    road(w, Curve::straight(ring_pt(270.0), ring_pt(90.0)), st);
    road(w, Curve::straight(ring_pt(180.0), ring_pt(0.0)), st);
    // Up from the ring to the avenue, and out to the east.
    if let Some(q) = first_hit(w, ring_pt(270.0), [0.0, -1.0], 600.0) {
        road(w, Curve::straight(ring_pt(270.0), q), av);
    }
    road(w, Curve::straight(ring_pt(0.0), [1060.0, 1270.0]), st);
    if let Some(q) = first_hit(w, [1060.0, 1270.0], [0.0, -1.0], 600.0) {
        road(w, Curve::straight([1060.0, 1270.0], q), st);
    }

    // The cul-de-sac estate: a curved collector south off the avenue, with
    // curved dead-end closes either side.
    let c0 = [180.0, y0];
    let c1 = [380.0, y0 + 230.0];
    let collector = Curve::from_tangents(c0, [0.0, 1.0], c1, [1.0, 0.0]).expect("collector");
    road(w, collector, st);
    road(w, Curve::continuing(c1, [1.0, 0.0], [520.0, y0 + 300.0]), st);
    let len = collector.length();
    for (k, &d) in [60.0f32, 120.0, 180.0, 240.0].iter().enumerate() {
        let pose = collector.pose_at(d.min(len - 10.0), 0.0);
        let right = [-pose.dir[1], pose.dir[0]];
        for side in [1.0f32, -1.0] {
            // A close: out sideways, bending back along the collector.
            let out = add(pose.pos, right, side * 26.0);
            let end = add(
                add(out, right, side * 40.0),
                pose.dir,
                if k % 2 == 0 { 30.0 } else { -30.0 },
            );
            let c = Curve::through_corner(pose.pos, add(out, right, side * 14.0), end);
            road(w, c, st);
        }
    }

    // Industry by the highway: straight streets on the world grid north of
    // the avenue.
    let ix = [124.0, 156.0 + 40.0];
    let _ = ix;
    let n0 = [140.0, y0 - 200.0];
    road(w, Curve::straight([140.0, y0], n0), st);
    road(w, Curve::straight(n0, [292.0, y0 - 200.0]), st);
    road(w, Curve::straight([140.0, y0 - 100.0], [292.0, y0 - 100.0]), st);

    // Zoning.
    let quad = |a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]| [a, b, c, d];
    let g = |k: f32, m: f32| add(add(o, u, k * bw), v, m * bh);
    // The turned grid: housing, one row of shops near the avenue.
    zone_quad(
        w,
        quad(g(-0.5, -0.5), g(4.5, -0.5), g(4.5, 3.5), g(-0.5, 3.5)),
        Zone::Residential,
    );
    zone_quad(
        w,
        quad(g(-0.5, 3.5), g(4.5, 3.5), g(4.5, 5.6), g(-0.5, 5.6)),
        Zone::Commercial,
    );
    // Shops along the avenue (both sides).
    zone_quad(
        w,
        quad(
            [236.0, y0 - 120.0],
            [1400.0, y0 - 120.0],
            [1400.0, y0 + 30.0],
            [236.0, y0 + 30.0],
        ),
        Zone::Commercial,
    );
    // The ring: shops inside, housing outside.
    zone_quad(
        w,
        quad(
            add(rc, [-1.0, -1.0], 80.0),
            add(rc, [1.0, -1.0], 80.0),
            add(rc, [1.0, 1.0], 80.0),
            add(rc, [-1.0, 1.0], 80.0),
        ),
        Zone::Commercial,
    );
    zone_quad(
        w,
        quad(
            add(rc, [-1.0, -1.0], 150.0),
            add(rc, [1.0, -1.0], 150.0),
            add(rc, [1.0, 1.0], 150.0),
            add(rc, [-1.0, 1.0], 150.0),
        ),
        Zone::Residential,
    );
    // The estate.
    zone_quad(
        w,
        quad(
            [60.0, y0 + 20.0],
            [560.0, y0 + 20.0],
            [560.0, y0 + 360.0],
            [60.0, y0 + 360.0],
        ),
        Zone::Residential,
    );
    // Industry, and a strip of it east of the ring.
    zone_quad(
        w,
        quad(
            [100.0, y0 - 240.0],
            [330.0, y0 - 240.0],
            [330.0, y0 - 8.0],
            [100.0, y0 - 8.0],
        ),
        Zone::Industrial,
    );
    zone_quad(
        w,
        quad(
            [1000.0, y0 + 60.0],
            [1120.0, y0 + 60.0],
            [1120.0, 1300.0],
            [1000.0, 1300.0],
        ),
        Zone::Industrial,
    );
    refused
}
