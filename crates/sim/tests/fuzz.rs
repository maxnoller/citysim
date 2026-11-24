//! Adversarial tests: long randomised command/step fuzzing on a growing city
//! with consistency checks after every command, determinism and save/load
//! lockstep under the same fuzz, and extreme road layouts.

use std::collections::BTreeSet;

use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

const DAY: u64 = balance::TICKS_PER_DAY as u64;
const HOUR: u64 = balance::TICKS_PER_HOUR as u64;

fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

// ------------------------------------------------------------- checks ----

/// Consistency checks beyond `check_sim_invariants`, cheap enough to run
/// after every command. `after_step`: the world was just stepped (segment
/// loads and junction tables are fresh).
fn check_consistency(w: &World, after_step: bool) {
    if let Err(e) = w.check_sim_invariants() {
        panic!("sim invariants at tick {}: {e}", w.time.tick);
    }
    let tick = w.time.tick;
    // Every vehicle has a trip, its pose matches its segment and progress.
    for (vid, v) in w.vehicles.iter() {
        let trip = w.traffic.trips.get(vid);
        assert!(trip.is_some(), "tick {tick}: {vid:?} has no trip");
        let t = trip.unwrap();
        if t.needs_route {
            assert!(t.queued, "tick {tick}: {vid:?} needs a route but is not queued");
        }
        if t.queued {
            assert!(
                w.traffic.replan.contains(&vid),
                "tick {tick}: {vid:?} marked queued but not in the queue"
            );
        }
        let s = v.segment.unwrap();
        let seg = &w.roads.segments[s];
        let pa = w.roads.nodes[seg.a].pos;
        let pb = w.roads.nodes[seg.b].pos;
        let (from, to) = if v.forward { (pa, pb) } else { (pb, pa) };
        let f = (v.progress / seg.length).clamp(0.0, 1.0);
        let want = [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f];
        let d = ((want[0] - v.pos[0]).powi(2) + (want[1] - v.pos[1]).powi(2)).sqrt();
        assert!(d < 0.05, "tick {tick}: {vid:?} pose off by {d} m");
        assert!(
            v.speed.is_finite() && v.speed >= 0.0,
            "tick {tick}: {vid:?} speed {}",
            v.speed
        );
        assert!(v.heading.is_finite());
    }
    // Households: a commuting household has a car or a waiting departure.
    for (hid, h) in w.households.iter() {
        let commuting = matches!(h.state, HouseholdState::CommutingToWork | HouseholdState::CommutingHome);
        if h.vehicle.is_some() {
            assert!(commuting, "tick {tick}: {hid:?} has a car but is {:?}", h.state);
        } else if commuting {
            let waiting = w.traffic.pending.iter().any(|d| d.household == Some(hid));
            assert!(
                waiting,
                "tick {tick}: {hid:?} is {:?} with no car and no departure",
                h.state
            );
        }
        let n = w.traffic.pending.iter().filter(|d| d.household == Some(hid)).count();
        assert!(
            n + usize::from(h.vehicle.is_some()) <= 1,
            "tick {tick}: {hid:?} has {n} departures and car {:?}",
            h.vehicle
        );
        assert!(h.members >= 1);
    }
    // Buildings: footprint cells point back and stay zoned for the kind.
    for (bid, b) in w.buildings.iter() {
        assert!(b.level >= 1 && b.capacity > 0, "tick {tick}: {bid:?} {b:?}");
        for cell in b.footprint.cells() {
            let cd = w.grid.get(cell).expect("footprint in bounds");
            assert_eq!(cd.occupant, Occupant::Building(bid), "tick {tick}: {cell:?}");
            assert_eq!(cd.zone, Some(b.kind), "tick {tick}: {cell:?} zone under {bid:?}");
        }
    }
    // Every building cell belongs to a live building.
    let building_cells: usize = w.buildings.values().map(|b| (b.size().0 * b.size().1) as usize).sum();
    let marked = w
        .grid
        .cells
        .iter()
        .filter(|cd| matches!(cd.occupant, Occupant::Building(_)))
        .count();
    assert_eq!(building_cells, marked, "tick {tick}: stray building cells");
    // Stats match the state.
    let s = &w.stats;
    assert_eq!(s.households as usize, w.households.len());
    assert_eq!(s.buildings as usize, w.buildings.len());
    assert_eq!(s.vehicles_on_road as usize, w.vehicles.len());
    let pop: u32 = w.households.values().map(|h| h.members as u32).sum();
    assert_eq!(s.population, pop);
    let roads = w.grid.cells.iter().filter(|cd| cd.occupant == Occupant::Road).count();
    assert_eq!(
        s.road_cells as usize, roads,
        "tick {tick}: cached road cell count stale"
    );
    for x in [
        w.demand.residential,
        w.demand.commercial,
        w.demand.industrial,
        w.growth.attractiveness,
    ] {
        assert!(x.is_finite() && (-1.0..=1.0).contains(&x), "tick {tick}: demand {x}");
    }
    assert!(w.traffic.avg_commute_s.is_finite() && w.traffic.avg_commute_s >= 0.0);
    // The highway stub is always there.
    let entry = w.roads.entry.expect("entry");
    assert!(w.roads.nodes.contains_key(entry));
    for x in 0..balance::HIGHWAY_STUB_CELLS {
        assert!(w.grid.is_road(c(x, 128)), "tick {tick}: highway cell {x} lost");
    }
    if after_step {
        assert!(w.traffic.junctions.len() >= w.roads.nodes.slot_count());
        let mut load: std::collections::BTreeMap<(SegmentId, usize), f32> = Default::default();
        for v in w.vehicles.values() {
            *load
                .entry((v.segment.unwrap(), RoadSegment::dir(v.forward)))
                .or_default() += 1.0;
        }
        for (s, seg) in w.roads.segments.iter() {
            for d in 0..2 {
                let want = load.get(&(s, d)).copied().unwrap_or(0.0);
                assert_eq!(seg.load[d], want, "tick {tick}: {s:?} load[{d}]");
            }
        }
    }
}

/// Heavier structural checks (road graph, grid vs graph).
fn check_full(w: &World) {
    if let Err(e) = w.check_invariants() {
        panic!("invariants at tick {}: {e}", w.time.tick);
    }
}

// --------------------------------------------------------------- fuzz ----

fn rand_range(r: &mut Rng, lo: i32, hi: i32) -> i32 {
    lo + r.below((hi - lo + 1) as u32) as i32
}

/// A random cell, mostly in and around the city, sometimes anywhere, rarely
/// off the map.
fn rand_cell(r: &mut Rng) -> Cell {
    match r.below(20) {
        0 => c(rand_range(r, -300, 600), rand_range(r, -300, 600)),
        1..=3 => c(rand_range(r, 0, 255), rand_range(r, 0, 255)),
        4..=5 => c(rand_range(r, 0, 16), rand_range(r, 122, 134)),
        _ => c(rand_range(r, 5, 110), rand_range(r, 95, 160)),
    }
}

fn rand_command(r: &mut Rng) -> Command {
    const DIRS: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];
    match r.below(100) {
        0..=34 => {
            let from = rand_cell(r);
            let to = match r.below(10) {
                0 => rand_cell(r), // usually not straight
                1 => from,
                _ => {
                    let (dx, dy) = DIRS[r.below(8) as usize];
                    let n = if r.below(4) == 0 {
                        rand_range(r, 1, 120)
                    } else {
                        rand_range(r, 1, 12)
                    };
                    from.offset(dx * n, dy * n)
                }
            };
            Command::BuildRoad { from, to }
        }
        35..=49 => {
            let min = rand_cell(r);
            let (w, h) = match r.below(60) {
                0 => (rand_range(r, 100, 600), rand_range(r, 100, 600)),
                1..=3 => (rand_range(r, 5, 30), rand_range(r, 5, 30)),
                5..=6 => (rand_range(r, -10, 0), rand_range(r, -10, 0)),
                _ => (rand_range(r, 0, 3), rand_range(r, 0, 3)),
            };
            let max = min.offset(w, h);
            // Corners in either order.
            if r.below(2) == 0 {
                Command::Bulldoze { min, max }
            } else {
                Command::Bulldoze { min: max, max: min }
            }
        }
        50..=84 => {
            let min = rand_cell(r);
            let max = min.offset(rand_range(r, -8, 15), rand_range(r, -8, 15));
            let zone = match r.below(5) {
                0 => None,
                1 => Some(Zone::Commercial),
                2 => Some(Zone::Industrial),
                _ => Some(Zone::Residential),
            };
            Command::SetZone { min, max, zone }
        }
        85..=94 => {
            let rate = match r.below(8) {
                0 => f32::NAN,
                1 => f32::INFINITY,
                2 => -0.01,
                3 => balance::MAX_TAX_RATE + 0.01,
                4 => -0.0,
                _ => r.next_f32() * balance::MAX_TAX_RATE,
            };
            Command::SetTaxRate(rate)
        }
        _ => {
            // Aim at the highway stub.
            let min = c(rand_range(r, -2, 14), rand_range(r, 125, 131));
            if r.below(2) == 0 {
                Command::Bulldoze {
                    min,
                    max: min.offset(rand_range(r, 0, 12), rand_range(r, 0, 3)),
                }
            } else {
                let (dx, dy) = DIRS[r.below(8) as usize];
                let n = rand_range(r, 1, 10);
                Command::BuildRoad {
                    from: min,
                    to: min.offset(dx * n, dy * n),
                }
            }
        }
    }
}

/// Unit road pieces `(p, q, highway)` with `p < q`.
fn pieces(w: &World) -> BTreeSet<(Cell, Cell, bool)> {
    let mut out = BTreeSet::new();
    for (s, seg) in w.roads.segments.iter() {
        let (pa, pb) = w.roads.segment_ends(s);
        let line = Cell::from_pos(pa)
            .straight_line_to(Cell::from_pos(pb))
            .expect("straight");
        for p in line.windows(2) {
            let (a, b) = if p[0] <= p[1] { (p[0], p[1]) } else { (p[1], p[0]) };
            out.insert((a, b, seg.kind == RoadKind::Highway));
        }
    }
    out
}

/// A bulldoze removed exactly the street pieces with an end in the
/// rectangle and the buildings overlapping it, reported exactly the cells it
/// cleared, and left grid road cells matching the remaining pieces.
fn check_bulldoze(before: &World, after: &World, rect: CellRect, cells: &[Cell]) {
    let Some(rect) = rect.clamp_to_map() else { return };
    let old = pieces(before);
    let want: BTreeSet<_> = old
        .iter()
        .copied()
        .filter(|&(a, b, hw)| hw || !(rect.contains(a) || rect.contains(b)))
        .collect();
    let got = pieces(after);
    assert_eq!(got, want, "bulldoze {rect:?} left the wrong pieces");
    let mut reported: BTreeSet<Cell> = old
        .difference(&want)
        .flat_map(|&(a, b, _)| [a, b])
        .filter(|&x| rect.contains(x))
        .collect();
    for b in before.buildings.values().filter(|b| b.footprint.intersects(&rect)) {
        reported.extend(b.footprint.cells());
        assert!(!after.buildings.values().any(|a| a == b), "{b:?} survived");
    }
    assert_eq!(cells, reported.into_iter().collect::<Vec<_>>().as_slice());
    after.roads.validate().unwrap();
    let covered: BTreeSet<Cell> = got.iter().flat_map(|&(a, b, _)| [a, b]).collect();
    for (i, cd) in after.grid.cells.iter().enumerate() {
        let cell = c(i as i32 % MAP_CELLS, i as i32 / MAP_CELLS);
        assert_eq!(
            cd.occupant == Occupant::Road,
            covered.contains(&cell),
            "{cell:?} after {rect:?}"
        );
    }
}

/// Apply `cmd` the way the game does, checking that `preview` agrees with
/// `apply` and that a failed command changes nothing.
fn apply_checked(w: &mut World, cmd: Command) {
    let preview = w.preview(&cmd);
    let before = w.clone();
    let got = w.apply(cmd);
    match (&preview, &got) {
        (Ok(p), Ok(o)) => {
            assert_eq!((p.cost, &p.cells), (o.cost, &o.cells), "{cmd:?}");
            if let Command::Bulldoze { min, max } = cmd {
                check_bulldoze(&before, w, CellRect::from_corners(min, max), &o.cells);
            }
        }
        (Err(a), Err(b)) => {
            assert_eq!(format!("{a:?}"), format!("{b:?}"), "{cmd:?}");
            assert!(*w == before, "failed {cmd:?} changed the world");
        }
        _ => panic!("preview {preview:?} but apply {got:?} for {cmd:?}"),
    }
}

/// What the fuzz run does with the world at each step boundary.
#[derive(Clone, Copy)]
struct FuzzOpts {
    seed: u64,
    ticks: u64,
    /// Check consistency after every command and step burst.
    check: bool,
    /// Full structural check every this many commands (0 = never).
    full_every: u32,
    /// Replace the world by `load(save())` with this chance per burst
    /// (per mille).
    reload_permille: u32,
}

/// Run a randomised game: a growing city (GridPlayer) plus random player
/// commands between bursts of steps. Returns saves taken at fixed ticks.
fn fuzz(o: FuzzOpts, checkpoints: &[u64]) -> (World, Vec<Vec<u8>>) {
    let mut w = World::new(o.seed);
    let mut cmd_rng = Rng::new(o.seed ^ 0xF022);
    let mut player = GridPlayer::new(&Zone::ALL);
    let mut saves = Vec::new();
    let mut next_cp = 0;
    let mut commands = 0u32;
    while w.time.tick < o.ticks {
        // A burst of steps (up to ~1 in-game hour), interrupted at
        // checkpoints so all runs save at the same ticks.
        let burst = match cmd_rng.below(10) {
            0 => 0,
            1..=6 => cmd_rng.below(60) as u64,
            _ => cmd_rng.below(HOUR as u32) as u64,
        };
        let end = (w.time.tick + burst).min(o.ticks);
        while w.time.tick < end {
            if next_cp < checkpoints.len() && w.time.tick == checkpoints[next_cp] {
                saves.push(w.save());
                next_cp += 1;
            }
            player.act(&mut w);
            w.step();
        }
        if o.check && burst > 0 {
            check_consistency(&w, true);
        }
        if cmd_rng.below(1000) < o.reload_permille {
            let bytes = w.save();
            let loaded = World::load(&bytes).expect("own save loads");
            assert!(loaded == w, "load(save()) differs at tick {}", w.time.tick);
            w = loaded;
        }
        // Keep the city funded so the fuzz keeps building.
        if w.economy.money < 20_000 {
            w.economy.money += 100_000;
        }
        // Few edits while the town is young, more once it has grown.
        let n = if w.time.tick < DAY && cmd_rng.below(4) != 0 {
            0
        } else {
            cmd_rng.below(3)
        };
        for _ in 0..n {
            let cmd = rand_command(&mut cmd_rng);
            // A whole-map wipe now and then, not often.
            let cmd = match cmd {
                Command::Bulldoze { min, max } if (max.x - min.x).abs() > 90 && cmd_rng.below(8) != 0 => {
                    Command::Bulldoze { min, max: min }
                }
                c => c,
            };
            if o.check {
                apply_checked(&mut w, cmd);
                check_consistency(&w, false);
            } else {
                let _ = w.apply(cmd);
            }
            commands += 1;
            if o.full_every > 0 && commands.is_multiple_of(o.full_every) {
                check_full(&w);
            }
        }
    }
    while next_cp < checkpoints.len() && w.time.tick == checkpoints[next_cp] {
        saves.push(w.save());
        next_cp += 1;
    }
    (w, saves)
}

fn sanity(w: &World) {
    // The fuzz must actually exercise a living city.
    assert!(w.time.tick > 0);
    eprintln!(
        "tick {} day {}: {} buildings, {} households, {} cars, {} segments, money {}",
        w.time.tick,
        w.time.day,
        w.buildings.len(),
        w.households.len(),
        w.vehicles.len(),
        w.roads.segments.len(),
        w.economy.money
    );
}

#[test]
fn fuzz_commands_and_steps_keep_the_world_consistent() {
    for seed in [1, 2] {
        let (w, _) = fuzz(
            FuzzOpts {
                seed,
                ticks: 2 * DAY,
                check: true,
                full_every: 150,
                reload_permille: 0,
            },
            &[],
        );
        sanity(&w);
        assert!(w.households.len() > 20, "fuzz city never grew: {}", w.households.len());
        check_full(&w);
    }
}

#[test]
#[ignore = "long: run with --ignored --release"]
fn fuzz_long() {
    for seed in 100..112 {
        let (w, _) = fuzz(
            FuzzOpts {
                seed,
                ticks: 10 * DAY,
                check: true,
                full_every: 400,
                reload_permille: 20,
            },
            &[],
        );
        sanity(&w);
        check_full(&w);
    }
}

fn checkpoints(seed: u64, ticks: u64, n: usize) -> Vec<u64> {
    let mut r = Rng::new(seed ^ 0xC0FFEE);
    // Always one in the morning rush of day 2 (08:00).
    let mut v: Vec<u64> = vec![DAY + 2 * HOUR];
    v.extend((0..n).map(|_| 1 + r.below(ticks as u32 - 1) as u64));
    v.retain(|&t| t < ticks);
    v.sort();
    v.dedup();
    v
}

#[test]
fn fuzz_is_deterministic_and_save_load_keeps_lockstep() {
    let ticks = 2 * DAY;
    let cps = checkpoints(7, ticks, 6);
    let opts = FuzzOpts {
        seed: 7,
        ticks,
        check: false,
        full_every: 0,
        reload_permille: 0,
    };
    let (a, sa) = fuzz(opts, &cps);
    let (b, sb) = fuzz(opts, &cps);
    sanity(&a);
    assert_eq!(sa.len(), cps.len());
    for (i, (x, y)) in sa.iter().zip(&sb).enumerate() {
        assert!(x == y, "same inputs diverged by tick {}", cps[i]);
    }
    assert!(a == b);
    // Same run, but the world is replaced by load(save()) at many random
    // points (often mid-rush): every checkpoint must match byte for byte.
    let (c, sc) = fuzz(
        FuzzOpts {
            reload_permille: 150,
            ..opts
        },
        &cps,
    );
    for (i, (x, y)) in sa.iter().zip(&sc).enumerate() {
        assert!(x == y, "save/load broke lockstep by tick {}", cps[i]);
    }
    assert!(a == c);
    assert_eq!(a.save(), c.save());
}

#[test]
#[ignore = "long: run with --ignored --release"]
fn fuzz_determinism_long() {
    for seed in [21, 22, 23] {
        let ticks = 6 * DAY;
        let cps = checkpoints(seed, ticks, 20);
        let opts = FuzzOpts {
            seed,
            ticks,
            check: false,
            full_every: 0,
            reload_permille: 0,
        };
        let (_, sa) = fuzz(opts, &cps);
        let (_, sb) = fuzz(opts, &cps);
        let (_, sc) = fuzz(
            FuzzOpts {
                reload_permille: 200,
                ..opts
            },
            &cps,
        );
        for i in 0..cps.len() {
            assert!(sa[i] == sb[i], "seed {seed}: diverged by tick {}", cps[i]);
            assert!(sa[i] == sc[i], "seed {seed}: save/load diverged by tick {}", cps[i]);
        }
    }
}
