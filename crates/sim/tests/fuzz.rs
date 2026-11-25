//! Adversarial tests: long randomised command/step fuzzing on a growing city
//! with consistency checks after every command, determinism and save/load
//! lockstep under the same fuzz.

mod common;

use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;
use common::*;

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

/// Mostly streets, some avenues, now and then the (unbuildable) highway.
fn rand_kind(r: &mut Rng) -> RoadKind {
    match r.below(20) {
        0..=13 => RoadKind::Street,
        14..=18 => RoadKind::Avenue,
        _ => RoadKind::Highway,
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
            Command::BuildRoad {
                from,
                to,
                kind: rand_kind(r),
            }
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
        85..=89 => {
            let control = match r.below(3) {
                0 => JunctionSetting::Auto,
                1 => JunctionSetting::Signal,
                _ => JunctionSetting::Priority,
            };
            Command::SetJunctionControl {
                cell: rand_cell(r),
                control,
            }
        }
        90..=94 => {
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
                    kind: rand_kind(r),
                }
            }
        }
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
            assert_eq!(
                (p.new_pieces, p.upgraded, p.downgraded, p.junction),
                (o.new_pieces, o.upgraded, o.downgraded, o.junction),
                "{cmd:?}"
            );
            if let Command::SetJunctionControl { cell, control } = cmd {
                let n = junction::junction_at(w, cell).expect("applied at a junction");
                let j = junction::junction(w, n).expect("junction state");
                assert_eq!(j.setting, control);
                assert_eq!(Some(j.control), p.junction.map(|c| c.control_after));
            }
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
    /// No random commands before this tick (lets a busy city grow first).
    calm_until: u64,
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
        let n = if w.time.tick < o.calm_until || (w.time.tick < DAY && cmd_rng.below(4) != 0) {
            0
        } else {
            cmd_rng.below(3)
        };
        for _ in 0..n {
            let cmd = rand_command(&mut cmd_rng);
            // Junction commands: usually aimed at a real junction.
            let cmd = match cmd {
                Command::SetJunctionControl { control, .. } if cmd_rng.below(4) != 0 => {
                    let nodes: Vec<NodeId> = junction::junction_nodes(&w).collect();
                    if nodes.is_empty() {
                        cmd
                    } else {
                        let n = nodes[cmd_rng.below(nodes.len() as u32) as usize];
                        Command::SetJunctionControl {
                            cell: Cell::from_pos(w.roads.nodes[n].pos),
                            control,
                        }
                    }
                }
                c => c,
            };
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
                calm_until: 0,
            },
            &[],
        );
        sanity(&w);
        assert!(
            w.households.len() > 20,
            "fuzz city never grew: {} {:?} {:?} money {} tax {}",
            w.households.len(),
            w.economy.fiscal,
            w.demand,
            w.economy.money,
            w.economy.tax_rate
        );
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
                calm_until: 0,
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
        calm_until: 0,
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
            calm_until: 0,
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

/// A grown city (several hundred households) torn at by random edits right
/// through the morning rush, checked after every command.
#[test]
fn fuzz_a_busy_city_through_rush_hour() {
    let (w, _) = fuzz(
        FuzzOpts {
            seed: 42,
            ticks: 2 * DAY + 7 * HOUR,
            check: true,
            full_every: 60,
            reload_permille: 0,
            calm_until: 2 * DAY + HOUR,
        },
        &[],
    );
    sanity(&w);
    assert!(
        w.households.len() > 200,
        "{} {:?} {:?} money {} tax {} bldg {} day {}",
        w.households.len(),
        w.economy.fiscal,
        w.demand,
        w.economy.money,
        w.economy.tax_rate,
        w.buildings.len(),
        w.time.day
    );
}

/// Same busy city: identical inputs give identical saves, and reloading at
/// random points (mid-rush) does not change anything.
#[test]
fn busy_city_save_load_lockstep() {
    let ticks = 2 * DAY + 6 * HOUR;
    let mut cps: Vec<u64> = checkpoints(43, ticks, 12)
        .into_iter()
        .filter(|&t| t > 2 * DAY)
        .collect();
    cps.push(2 * DAY + 2 * HOUR + 7);
    cps.sort();
    let opts = FuzzOpts {
        seed: 43,
        ticks,
        check: false,
        full_every: 0,
        reload_permille: 0,
        calm_until: 2 * DAY,
    };
    let (a, sa) = fuzz(opts, &cps);
    sanity(&a);
    assert!(a.households.len() > 200, "{}", a.households.len());
    let (b, sb) = fuzz(
        FuzzOpts {
            reload_permille: 300,
            ..opts
        },
        &cps,
    );
    for (i, (x, y)) in sa.iter().zip(&sb).enumerate() {
        assert!(x == y, "save/load broke lockstep by tick {}", cps[i]);
    }
    assert!(a == b, "{}", common::diff(&a, &b));
}

#[test]
#[ignore = "long: run with --ignored --release"]
fn fuzz_busy_cities_long() {
    for seed in 200..216 {
        let (w, _) = fuzz(
            FuzzOpts {
                seed,
                ticks: 6 * DAY,
                check: true,
                full_every: 200,
                reload_permille: 30,
                calm_until: 2 * DAY + seed % 8 * HOUR,
            },
            &[],
        );
        sanity(&w);
    }
}
