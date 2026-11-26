//! Round 2 rules: road kinds and upgrades, junction control and signals,
//! conflict-free junction boxes, lots with frontage, money with
//! consequences, demand on the first frame, round-1 saves, determinism.

mod common;

use citysim_sim::junction::{self, Aspect, JunctionControl, JunctionSetting, Turn};
use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::*;
use common::*;

fn build(w: &mut World, a: (i32, i32), b: (i32, i32), kind: RoadKind) -> Result<Outcome, CommandError> {
    w.apply(Command::BuildRoad {
        from: c(a.0, a.1),
        to: c(b.0, b.1),
        kind,
    })
}

fn kinds_along(w: &World, from: i32, to: i32, y: i32) -> Vec<RoadKind> {
    (from..to)
        .map(|x| w.roads.segments[w.roads.piece(c(x, y), c(x + 1, y)).expect("piece")].kind)
        .collect()
}

// ------------------------------------------------------------- roads ----

#[test]
fn avenues_upgrade_and_downgrade_in_place() {
    let mut w = World::new(1);
    build(&mut w, (11, 128), (40, 128), RoadKind::Street).unwrap();
    let money = w.economy.money;
    // Part of it becomes an avenue, and the line runs on past the street.
    let cmd = Command::BuildRoad {
        from: c(15, 128),
        to: c(45, 128),
        kind: RoadKind::Avenue,
    };
    let p = w.preview(&cmd).unwrap();
    let diff = balance::AVENUE_COST_PER_CELL - balance::ROAD_COST_PER_CELL;
    assert_eq!((p.upgraded, p.new_pieces, p.downgraded), (25, 5, 0));
    assert_eq!(p.cost, 25 * diff + 5 * balance::AVENUE_COST_PER_CELL);
    let o = w.apply(cmd).unwrap();
    assert_eq!(o.cost, p.cost);
    assert_eq!(w.economy.money, money - p.cost);
    let k = kinds_along(&w, 11, 45, 128);
    assert!(k[..4].iter().all(|&k| k == RoadKind::Street), "{k:?}");
    assert!(k[4..].iter().all(|&k| k == RoadKind::Avenue), "{k:?}");
    w.check_invariants().unwrap();
    // Same kind again: nothing to do, free.
    let again = w
        .preview(&Command::BuildRoad {
            from: c(20, 128),
            to: c(30, 128),
            kind: RoadKind::Avenue,
        })
        .unwrap();
    assert_eq!((again.cost, again.upgraded, again.new_pieces), (0, 0, 0));
    // Downgrade a stretch: free, no refund.
    let money = w.economy.money;
    let o = build(&mut w, (20, 128), (25, 128), RoadKind::Street).unwrap();
    assert_eq!((o.cost, o.downgraded, o.upgraded), (0, 5, 0));
    assert_eq!(w.economy.money, money);
    let k = kinds_along(&w, 11, 45, 128);
    assert!(k[9..14].iter().all(|&k| k == RoadKind::Street), "{k:?}");
    assert!(k[14..].iter().all(|&k| k == RoadKind::Avenue), "{k:?}");
    w.check_invariants().unwrap();
    // Segments are merged per kind: street | avenue | street | avenue.
    let kinds: Vec<RoadKind> = w.roads.segments.values().map(|s| s.kind).collect();
    assert_eq!(kinds.iter().filter(|&&k| k == RoadKind::Avenue).count(), 2);
    // The highway is not buildable and never changes kind.
    assert!(matches!(
        build(&mut w, (2, 128), (6, 128), RoadKind::Highway),
        Err(CommandError::InvalidRoadKind(RoadKind::Highway))
    ));
    let o = build(&mut w, (2, 128), (14, 128), RoadKind::Avenue).unwrap();
    assert_eq!(o.upgraded, 3, "only the street part of the line upgrades");
    let hw = w
        .roads
        .segments
        .values()
        .filter(|s| s.kind == RoadKind::Highway)
        .count();
    assert_eq!(hw, 1);
    // Upkeep: avenues cost more per day.
    assert!(w.stats.avenue_cells > 0);
    let b = economy::budget_now(&w);
    assert_eq!(
        b.road_upkeep,
        w.stats.road_cells as i64 * balance::ROAD_UPKEEP_PER_CELL_PER_DAY
            + w.stats.avenue_cells as i64
                * (balance::AVENUE_UPKEEP_PER_CELL_PER_DAY - balance::ROAD_UPKEEP_PER_CELL_PER_DAY)
            + economy::running_costs(&w).signals
    );
    // Not enough money for an upgrade: refused, nothing changes.
    w.economy.money = 10;
    let before = w.clone();
    assert!(matches!(
        build(&mut w, (20, 128), (25, 128), RoadKind::Avenue),
        Err(CommandError::InsufficientFunds { .. })
    ));
    assert!(w == before);
}

#[test]
fn lanes_and_widths_per_kind() {
    assert_eq!(RoadKind::Street.lane_count(), 1);
    assert_eq!(RoadKind::Avenue.lane_count(), 2);
    assert_eq!(RoadKind::Highway.lane_count(), 2);
    assert!(RoadKind::Avenue.speed_limit() > RoadKind::Street.speed_limit());
    assert!(RoadKind::Avenue.half_width_m() * 2.0 >= 12.5);
    // Lane 0 is the kerb lane: further right than lane 1.
    assert!(RoadKind::Avenue.lane_offset_m(0) > RoadKind::Avenue.lane_offset_m(1));
    // A segment's capacity doubles with two lanes.
    let mut w = World::new(1);
    build(&mut w, (11, 128), (30, 128), RoadKind::Street).unwrap();
    build(&mut w, (30, 128), (49, 128), RoadKind::Avenue).unwrap();
    let cap = |k: RoadKind| {
        w.roads
            .segments
            .values()
            .find(|s| s.kind == k)
            .map(|s| s.capacity[0] / s.length)
            .unwrap()
    };
    assert!((cap(RoadKind::Avenue) / cap(RoadKind::Street) - 2.0).abs() < 1e-3);
}

// --------------------------------------------------------- junctions ----

/// A street crossing at (30, 128), a T at (40, 128), an avenue crossing at
/// (50, 128) with a street, avenues meeting at (60, 128).
fn junction_town() -> World {
    let mut w = World::new(3);
    scenarios::prebuilt(&mut w, |w| {
        build(w, (11, 128), (45, 128), RoadKind::Street).unwrap();
        build(w, (45, 128), (70, 128), RoadKind::Avenue).unwrap();
        build(w, (30, 118), (30, 138), RoadKind::Street).unwrap();
        build(w, (40, 128), (40, 138), RoadKind::Street).unwrap();
        build(w, (50, 118), (50, 138), RoadKind::Street).unwrap();
        build(w, (60, 118), (60, 138), RoadKind::Avenue).unwrap();
    });
    w
}

#[test]
fn default_controls_by_road_class() {
    let w = junction_town();
    let ctl = |x, y| junction::junction_control(&w, junction::junction_at(&w, c(x, y)).unwrap());
    // Equal streets crossing: give way, first come first served (no major).
    assert_eq!(ctl(30, 128), Some(JunctionControl::Priority));
    let j = junction::junction(&w, junction::junction_at(&w, c(30, 128)).unwrap()).unwrap();
    assert!(j.arms.iter().all(|a| !a.major));
    // A T: the through road is major.
    let j = junction::junction(&w, junction::junction_at(&w, c(40, 128)).unwrap()).unwrap();
    assert_eq!(j.control, JunctionControl::Priority);
    assert_eq!(j.arms.iter().filter(|a| a.major).count(), 2);
    // A street crossing an avenue gives way to it until it gets busy.
    assert_eq!(ctl(50, 128), Some(JunctionControl::Priority));
    let j = junction::junction(&w, junction::junction_at(&w, c(50, 128)).unwrap()).unwrap();
    assert!(j.arms.iter().all(|a| a.major == (a.kind == RoadKind::Avenue)));
    // Two avenues: signals.
    assert_eq!(ctl(60, 128), Some(JunctionControl::Signal));
    // Opposite approaches share a phase; avenue approaches with a turn
    // pocket get a protected left phase (`POCKET_LEFT_PHASES`, called once
    // the pocket fills); two avenues crossing are served alike and not
    // coordinated.
    let j = junction::junction(&w, junction::junction_at(&w, c(60, 128)).unwrap()).unwrap();
    let plan = j.plan.as_ref().unwrap();
    let lefts = if balance::SIGNAL_LEFT_PHASES || balance::POCKET_LEFT_PHASES {
        2
    } else {
        0
    };
    assert_eq!(plan.phases.len(), 2 + lefts, "{plan:?}");
    assert!(plan.phases.iter().filter(|p| p.left_only).count() == lefts);
    assert!(plan.phases.iter().all(|p| p.arms.len() == 2));
    assert!(!plan.coordinated);
    let through: Vec<f32> = plan
        .phases
        .iter()
        .filter(|p| !p.left_only)
        .map(|p| p.max_green_s)
        .collect();
    let main_max = if balance::POCKET_LEFT_PHASES {
        balance::POCKET_MAIN_MAX_GREEN_S
    } else {
        balance::SIGNAL_MAIN_MAX_GREEN_S
    };
    assert_eq!(through, vec![main_max; 2]);
    // Nodes with fewer than three arms are no junction.
    assert!(junction::junction_at(&w, c(20, 128)).is_none());
}

#[test]
fn player_override_with_preview() {
    let mut w = junction_town();
    let cell = c(30, 128);
    let n = junction::junction_at(&w, cell).unwrap();
    let cmd = Command::SetJunctionControl {
        cell,
        control: JunctionSetting::Signal,
    };
    let p = w.preview(&cmd).unwrap();
    let ch = p.junction.unwrap();
    assert_eq!(
        (ch.node, ch.setting_before, ch.setting_after),
        (n, JunctionSetting::Auto, JunctionSetting::Signal)
    );
    assert_eq!(
        (ch.control_before, ch.control_after),
        (JunctionControl::Priority, JunctionControl::Signal)
    );
    assert_eq!(p.cost, 0);
    assert_eq!(p.cells, vec![cell]);
    let o = w.apply(cmd).unwrap();
    assert_eq!(o.junction, p.junction);
    let view = junction::junction_view(&w, n).unwrap();
    assert_eq!(view.control, JunctionControl::Signal);
    assert_eq!(view.setting, JunctionSetting::Signal);
    assert_eq!(view.approaches.len(), 4);
    assert!(view.approaches.iter().all(|a| a.aspect.is_some()));
    // Exactly the approaches of one phase are green.
    let green = view
        .approaches
        .iter()
        .filter(|a| a.aspect == Some(Aspect::Green))
        .count();
    assert_eq!(green, 2);
    // The override survives an unrelated road edit and a save / load.
    build(&mut w, (30, 118), (40, 118), RoadKind::Street).unwrap();
    let w2 = World::load(&w.save()).unwrap();
    assert_eq!(junction::junction_control(&w2, n), Some(JunctionControl::Signal));
    // Priority on the avenues' crossing, then back to Auto (signals).
    let m = junction::junction_at(&w, c(60, 128)).unwrap();
    w.apply(Command::SetJunctionControl {
        cell: c(60, 128),
        control: JunctionSetting::Priority,
    })
    .unwrap();
    assert_eq!(junction::junction_control(&w, m), Some(JunctionControl::Priority));
    let p = w
        .preview(&Command::SetJunctionControl {
            cell: c(60, 128),
            control: JunctionSetting::Auto,
        })
        .unwrap();
    assert_eq!(p.junction.unwrap().control_after, JunctionControl::Signal);
    // Not a junction.
    for cell in [c(20, 128), c(100, 100), c(-1, 5)] {
        let e = w
            .apply(Command::SetJunctionControl {
                cell,
                control: JunctionSetting::Signal,
            })
            .unwrap_err();
        assert!(
            matches!(e, CommandError::NotAJunction(_) | CommandError::OutOfBounds),
            "{e:?}"
        );
    }
}

/// Feed steady traffic through every arm of the junction town.
fn feed(w: &mut World, k: &mut usize) {
    let ends = [
        (c(14, 128), c(68, 128)),
        (c(68, 128), c(14, 128)),
        (c(30, 120), c(30, 136)),
        (c(30, 136), c(60, 120)),
        (c(50, 120), c(50, 136)),
        (c(50, 136), c(14, 128)),
        (c(60, 120), c(60, 136)),
        (c(60, 136), c(40, 136)),
        (c(40, 136), c(60, 120)),
        (c(68, 128), c(50, 120)),
    ];
    if w.time.tick.is_multiple_of(9) && w.traffic.pending.len() < 30 {
        let (a, b) = ends[*k % ends.len()];
        *k += 1;
        traffic::debug_request(w, a.center(), b.center());
    }
}

#[test]
fn junction_boxes_never_hold_conflicting_cars_and_lanes_never_overlap() {
    let mut w = junction_town();
    let mut k = 0;
    let mut greens: std::collections::BTreeMap<(NodeId, usize), u32> = Default::default();
    let mut turns = [0u32; 4];
    let mut entered = 0u32;
    let mut was_in: Vec<(VehicleId, NodeId)> = Vec::new();
    for t in 0..12 * 60 * TICKS_PER_SECOND as u64 {
        feed(&mut w, &mut k);
        w.step();
        // Conflicting movements share a box only once one car is past
        // where their paths meet (never both still in each other's way).
        let mut boxed: Vec<(NodeId, usize, f32)> = Vec::new();
        let mut now_in: Vec<(VehicleId, NodeId)> = Vec::new();
        for (id, v) in w.vehicles.iter() {
            if let Some(jm) = v.junction {
                boxed.push((jm.node, jm.movement as usize, jm.distance));
                now_in.push((id, jm.node));
                if !was_in.contains(&(id, jm.node)) {
                    entered += 1;
                    turns[match jm.turn {
                        Turn::Straight => 0,
                        Turn::Right => 1,
                        Turn::Left => 2,
                        Turn::UTurn => 3,
                    }] += 1;
                }
            }
        }
        was_in = now_in;
        for (i, a) in boxed.iter().enumerate() {
            for b in &boxed[i + 1..] {
                if a.0 == b.0 {
                    let j = junction::junction(&w, a.0)
                        .or_else(|| w.traffic.junction_states.get(a.0))
                        .unwrap();
                    assert!(
                        !(j.blocks(a.1, a.2, b.1) && j.blocks(b.1, b.2, a.1)),
                        "tick {t}: {:?} and {:?} conflict in {:?}",
                        j.movements[a.1],
                        j.movements[b.1],
                        a.0
                    );
                }
            }
        }
        // Cars on one lane keep their distance; cars on one movement too.
        let mut lanes: std::collections::BTreeMap<(SegmentId, bool, u8), Vec<f32>> = Default::default();
        let mut moves: std::collections::BTreeMap<(NodeId, u16), Vec<f32>> = Default::default();
        for v in w.vehicles.values() {
            match v.junction {
                Some(jm) => moves.entry((jm.node, jm.movement)).or_default().push(jm.distance),
                None => lanes
                    .entry((v.segment.unwrap(), v.forward, v.lane))
                    .or_default()
                    .push(v.progress),
            }
        }
        let groups = lanes
            .into_iter()
            .map(|(k, v)| (format!("lane {k:?}"), v))
            .chain(moves.into_iter().map(|(k, v)| (format!("movement {k:?}"), v)));
        for (key, mut ps) in groups {
            ps.sort_by(f32::total_cmp);
            for p in ps.windows(2) {
                assert!(
                    p[1] - p[0] > balance::MIN_GAP_M * 0.5,
                    "tick {t}: cars {:.2} m apart on {key}",
                    p[1] - p[0]
                );
            }
        }
        // Which approaches see green.
        for n in junction::junction_nodes(&w) {
            if let Some(v) = junction::junction_view(&w, n) {
                for (i, a) in v.approaches.iter().enumerate() {
                    if a.aspect == Some(Aspect::Green) {
                        *greens.entry((n, i)).or_default() += 1;
                    }
                }
            }
        }
    }
    assert!(entered > 300, "only {entered} cars crossed junctions");
    assert!(turns[0] > 0 && turns[1] > 0 && turns[2] > 0, "{turns:?}");
    // Every approach of every signal got green at some point.
    for n in junction::junction_nodes(&w) {
        let j = junction::junction(&w, n).unwrap();
        if j.control == JunctionControl::Signal {
            for i in 0..j.arms.len() {
                assert!(
                    greens.get(&(n, i)).copied().unwrap_or(0) > 0,
                    "{n:?} arm {i} never green"
                );
            }
        }
    }
    assert_eq!(w.traffic.deadlocks_resolved, 0);
    w.check_sim_invariants().unwrap();
}

#[test]
fn signals_cycle_with_yellow_and_all_red_and_skip_empty_phases() {
    let mut w = junction_town();
    let n = junction::junction_at(&w, c(60, 128)).unwrap();
    // Nobody around: the signal rests on its main phase.
    let start = junction::junction(&w, n).unwrap().signal;
    for _ in 0..30 * TICKS_PER_SECOND {
        w.step();
    }
    assert_eq!(junction::junction(&w, n).unwrap().signal.phase, start.phase);
    // With traffic on every arm it goes green -> yellow -> all-red -> next.
    let mut k = 0;
    let mut seen = Vec::new();
    for _ in 0..240 * TICKS_PER_SECOND {
        feed(&mut w, &mut k);
        w.step();
        let s = junction::junction(&w, n).unwrap().signal;
        if seen.last() != Some(&(s.phase, s.stage)) {
            seen.push((s.phase, s.stage));
        }
    }
    use junction::SignalStage::*;
    for win in seen.windows(2) {
        let ok = matches!(
            (win[0].1, win[1].1),
            (Green, Yellow) | (Yellow, AllRed) | (AllRed, Green)
        );
        assert!(ok, "{:?} -> {:?}", win[0], win[1]);
    }
    let phases: std::collections::BTreeSet<u8> = seen.iter().map(|s| s.0).collect();
    assert!(phases.len() >= 2, "{seen:?}");
    // The view reports a time to the next change for the green approaches.
    let v = junction::junction_view(&w, n).unwrap();
    assert!(v.approaches.iter().any(|a| a.time_to_change_s.is_some()));
}

#[test]
fn coordinated_avenue_signals_are_offset_along_the_avenue() {
    let mut w = World::new(5);
    scenarios::prebuilt(&mut w, |w| {
        build(w, (11, 128), (90, 128), RoadKind::Avenue).unwrap();
        for x in [30, 45, 60, 75] {
            build(w, (x, 118), (x, 138), RoadKind::Street).unwrap();
        }
    });
    let offsets: Vec<f32> = [30, 45, 60, 75]
        .iter()
        .map(|&x| {
            let j = junction::junction(&w, junction::junction_at(&w, c(x, 128)).unwrap()).unwrap();
            let p = j.plan.as_ref().unwrap();
            assert!(p.coordinated);
            p.offset_s
        })
        .collect();
    // Neighbours 120 m apart are offset by the travel time at progression
    // speed (mod the cycle), so they do not all turn green at once.
    for w2 in offsets.windows(2) {
        let d = (w2[1] - w2[0]).rem_euclid(balance::SIGNAL_COORD_CYCLE_S);
        let want = (15.0 * CELL_SIZE / balance::SIGNAL_PROGRESSION_SPEED).rem_euclid(balance::SIGNAL_COORD_CYCLE_S);
        assert!((d - want).abs() < 0.01, "{offsets:?}");
    }
}

#[test]
fn deadlock_breaker_almost_never_fires_in_a_scripted_city() {
    let mut w = World::new(7);
    let mut p = GridPlayer::hierarchy();
    let mut trips = 0u64;
    let mut last = 0u32;
    for _ in 0..2 * DAY {
        p.act(&mut w);
        w.step();
        let t = w.traffic.trips_today;
        if t < last {
            trips += last as u64;
        }
        last = t;
    }
    trips += last as u64;
    assert!(trips > 1_000, "{trips} trips");
    eprintln!(
        "{trips} trips, {} deadlocks, {} stuck",
        w.traffic.deadlocks_resolved, w.traffic.stuck_removed
    );
    let fired = w.traffic.deadlocks_resolved as u64;
    assert!(fired * 1000 < trips, "{fired} deadlocks in {trips} trips");
    assert!(
        (w.traffic.stuck_removed as u64) * 50 < trips,
        "{} stuck",
        w.traffic.stuck_removed
    );
}

// -------------------------------------------------------------- lots ----

/// Every building fronts a road with its facing side.
fn assert_frontage(w: &World) {
    for (id, b) in w.buildings.iter() {
        assert!(
            growth::has_frontage(w, &b.footprint, b.facing),
            "{id:?} {:?} facing {:?} has no frontage",
            b.footprint.rect(),
            b.facing
        );
        // One or two lots wide, up to a block deep (the rows behind the
        // building are its yard).
        let (wd, h) = b.size();
        let ok = (1..=2).contains(&wd) && (1..=balance::BLOCK_DEPTH as i32).contains(&h);
        assert!(ok, "{id:?} footprint {wd}x{h}");
        assert!(wd == 1 || h >= 2, "{id:?} a wide lot is deep too");
        // `size` is width along the front, depth along the facing.
        let r = b.footprint.rect();
        let along_x = matches!(b.facing, Facing::PlusX | Facing::MinusX);
        let (across, deep) = if along_x {
            (r.height(), r.width())
        } else {
            (r.width(), r.height())
        };
        assert_eq!((across, deep), (wd, h), "{id:?} {r:?} {:?}", b.facing);
    }
}

#[test]
fn a_large_zoned_block_fills_with_lots_that_all_front_a_road() {
    let mut w = World::new(9);
    scenarios::prebuilt(&mut w, |w| {
        build(w, (11, 128), (70, 128), RoadKind::Street).unwrap();
        // A big block of four rows, two diagonal streets cutting a corner.
        build(w, (20, 128), (20, 108), RoadKind::Street).unwrap();
        build(w, (20, 108), (60, 108), RoadKind::Street).unwrap();
        build(w, (60, 108), (60, 128), RoadKind::Street).unwrap();
        build(w, (20, 118), (60, 118), RoadKind::Street).unwrap();
        build(w, (44, 128), (56, 116), RoadKind::Street).unwrap_or_default();
        build(w, (30, 108), (40, 118), RoadKind::Street).unwrap_or_default();
        scenarios::zone(w, (18, 106), (62, 127), Zone::Residential);
        // Jobs so the town keeps growing.
        build(w, (20, 128), (20, 140), RoadKind::Street).unwrap();
        build(w, (20, 140), (60, 140), RoadKind::Street).unwrap();
        build(w, (60, 128), (60, 140), RoadKind::Street).unwrap();
        build(w, (40, 128), (40, 140), RoadKind::Street).unwrap();
        scenarios::zone(w, (21, 129), (39, 139), Zone::Commercial);
        scenarios::zone(w, (41, 129), (59, 139), Zone::Industrial);
    });
    for _ in 0..4 * DAY {
        w.step();
    }
    assert!(w.buildings.len() > 150, "{}", w.buildings.len());
    assert_frontage(&w);
    let deep = w.buildings.values().filter(|b| b.size().0 * b.size().1 == 2).count();
    let wide = w.buildings.values().filter(|b| b.size().0 * b.size().1 == 4).count();
    assert!(deep > 40 && wide > 3, "deep {deep} wide {wide}");
    // Along the diagonals too: some lots stand on a diagonal road's own
    // lattice, turned to face it.
    let diag = w.buildings.values().any(|b| {
        let r = b.rotation().to_degrees().rem_euclid(90.0);
        b.footprint.lattice != LatticeId::WORLD && (r - 45.0).abs() < 1.0
    });
    assert!(diag, "no lot along a diagonal");
    // No building ever stands in a second row: every occupied cell is in a
    // lot whose front touches a road.
    w.check_invariants().unwrap();
}

#[test]
fn second_row_cells_without_a_front_lot_stay_empty() {
    let mut w = World::new(2);
    build(&mut w, (11, 128), (30, 128), RoadKind::Street).unwrap();
    // Only the second row is zoned: no building may appear there.
    scenarios::zone(&mut w, (12, 126), (29, 126), Zone::Residential);
    for _ in 0..DAY {
        w.step();
    }
    assert_eq!(w.buildings.len(), 0);
    // Zone the front row too: deep lots absorb the second row.
    scenarios::zone(&mut w, (12, 127), (29, 127), Zone::Residential);
    for _ in 0..DAY {
        w.step();
    }
    assert!(!w.buildings.is_empty());
    assert_frontage(&w);
    assert!(
        w.buildings
            .values()
            .any(|b| b.footprint.rect().height() == 2 && b.facing == Facing::PlusY)
    );
}

// ---------------------------------------------------------------- money ----

/// A small working town that pays its way.
fn small_town(seed: u64) -> (World, GridPlayer) {
    let mut w = World::new(seed);
    let mut p = GridPlayer::new(&Zone::ALL);
    for _ in 0..3 * DAY {
        p.act(&mut w);
        w.step();
    }
    (w, p)
}

#[test]
fn debt_cuts_services_then_residents_leave_and_it_recovers() {
    let (mut w, _) = small_town(3);
    assert_eq!(w.economy.fiscal.state, economy::FiscalState::Healthy);
    let pop0 = w.stats.population;
    assert!(pop0 > 500, "{pop0}");
    let attract0 = demand::target(&w).1;
    // Deep in debt.
    w.economy.money = -60_000;
    for _ in 0..DAY + 10 {
        w.step();
    }
    let f = w.economy.fiscal;
    assert_eq!(f.state, economy::FiscalState::InDebt);
    assert!(f.service_level < 1.0);
    assert_eq!(f.reason, economy::FiscalReason::ServicesCut);
    assert!(!f.reason.text().is_empty());
    assert!(demand::target(&w).1 < attract0 - 0.2, "services cut: demand falls");
    let mut peak = w.stats.population;
    for _ in 0..3 * DAY {
        w.step();
        peak = peak.max(w.stats.population);
    }
    let f = w.economy.fiscal;
    assert_eq!(f.state, economy::FiscalState::Crisis);
    assert!(f.days_in_debt >= balance::DEBT_CRISIS_DAYS);
    assert!(f.left_last_night > 0);
    assert!(
        w.stats.population < peak * 9 / 10,
        "residents leave: {} of {peak}",
        w.stats.population
    );
    // No sudden game over: the town is still there.
    assert!(w.stats.population > pop0 / 4);
    // Fixed: money back in the black, services recover, nobody leaves.
    w.economy.money = 20_000;
    for _ in 0..4 * DAY {
        w.step();
    }
    let f = w.economy.fiscal;
    assert!(matches!(
        f.state,
        economy::FiscalState::Healthy | economy::FiscalState::Deficit
    ));
    assert!((f.service_level - 1.0).abs() < 1e-6, "{f:?}");
    assert_eq!(f.left_last_night, 0);
    w.check_sim_invariants().unwrap();
}

#[test]
fn zero_tax_is_no_growth_button_and_high_tax_slows_growth() {
    let base = demand::tax_effect(balance::DEFAULT_TAX_RATE);
    assert_eq!(base, 0.0);
    assert!(demand::tax_effect(0.0) <= balance::LOW_TAX_DEMAND_MAX + 1e-6);
    assert!(demand::tax_effect(balance::MAX_TAX_RATE) < -0.5);
    // Independent towns, run side by side (the test's run time): one at
    // zero tax, and one town grown once and then copied to run on at
    // default, zero and maximum tax (a copy runs exactly like a rebuild).
    let (zero_tax_town, [normal, zero, high]) = std::thread::scope(|s| {
        let zero_tax_town = s.spawn(|| {
            let (mut w, _) = small_town(4);
            w.apply(Command::SetTaxRate(0.0)).unwrap();
            let money = w.economy.money;
            for _ in 0..2 * DAY {
                w.step();
            }
            (w, money)
        });
        let town = small_town(5);
        let grow = |(mut w, mut p): (World, GridPlayer), rate: f32| {
            w.apply(Command::SetTaxRate(rate)).unwrap();
            for _ in 0..3 * DAY {
                p.act(&mut w);
                w.step();
            }
            w.stats.population
        };
        let grown = [balance::DEFAULT_TAX_RATE, 0.0, balance::MAX_TAX_RATE].map(|rate| {
            let town = town.clone();
            s.spawn(move || grow(town, rate))
        });
        (zero_tax_town.join().unwrap(), grown.map(|h| h.join().unwrap()))
    });
    // A town at zero tax runs a loss every day and ends up in debt.
    let (w, money) = zero_tax_town;
    assert!(w.economy.money < money);
    assert!(w.economy.history.last().unwrap().budget.net() < 0);
    assert_ne!(w.economy.fiscal.state, economy::FiscalState::Healthy);
    // Growth: the same town at default, zero and maximum tax.
    assert!(high < normal * 9 / 10, "high tax {high} vs {normal}");
    assert!(
        zero < normal * 12 / 10,
        "zero tax {zero} vs {normal}: not a growth button"
    );
}

#[test]
fn demand_is_valid_from_the_first_frame_and_while_paused() {
    let mut w = World::new(1);
    assert!(w.demand.residential > 0.2, "{:?}", w.demand);
    // Paused (no steps): commands still update demand on a new game.
    build(&mut w, (11, 128), (30, 128), RoadKind::Street).unwrap();
    let before = w.demand;
    w.apply(Command::SetTaxRate(balance::MAX_TAX_RATE)).unwrap();
    assert!(w.demand.residential < before.residential - 0.5, "{:?}", w.demand);
    assert_eq!(w.time.tick, 0);
}

// ---------------------------------------------------------------- saves ----

#[test]
fn round1_saves_load_and_second_row_buildings_are_merged() {
    // A round-1 town with a building in the second row behind a house.
    let mut w = World::new(6);
    build(&mut w, (11, 128), (30, 128), RoadKind::Street).unwrap();
    scenarios::zone(&mut w, (12, 126), (29, 127), Zone::Residential);
    for _ in 0..DAY / 2 {
        w.step();
    }
    let front = c(25, 127);
    let back = c(25, 126);
    for cell in [front, back] {
        assert_eq!(
            w.grid_view().get(cell).unwrap().occupant,
            Occupant::Empty,
            "{cell:?} free"
        );
    }
    let mk = |cell: Cell, w: &mut World| {
        let id = w.buildings.insert(Building {
            kind: Zone::Residential,
            footprint: citysim_sim::Footprint::world(CellRect::single(cell), Facing::PlusY),
            level: 1,
            capacity: 4,
            occupied: 0,
            facing: Facing::PlusY,
            seed: 1,
            created_tick: 0,
        });
        citysim_sim::growth::place_cells(w, id);
        w.growth.meta.insert(id, Default::default());
        id
    };
    let f = mk(front, &mut w);
    let b = mk(back, &mut w);
    let hh = w.households.insert(Household::without_citizens(b, 2, 3));
    w.buildings[b].occupied = 2;
    w.versions.buildings += 1;
    w.versions.grid += 1;
    let old = citysim_sim::save::v3::save_as_v3(&w);
    assert_eq!(&old[4..8], &3u32.to_le_bytes());
    let mut loaded = World::load(&old).expect("round-1 save loads");
    assert!(!loaded.buildings.contains_key(b), "second-row building merged");
    let fb = &loaded.buildings[f];
    assert_eq!(fb.footprint.rect(), CellRect::from_corners(back, front));
    assert_eq!((fb.capacity, fb.occupied), (8, 2));
    assert_eq!(loaded.households[hh].home, f);
    assert_eq!(loaded.growth.legacy_lots_merged, 1);
    loaded.check_invariants().unwrap();
    loaded.check_sim_invariants().unwrap();
    // It keeps running, and saves in the new format.
    for _ in 0..HOUR {
        loaded.step();
    }
    loaded.check_sim_invariants().unwrap();
    let new = loaded.save();
    assert_eq!(&new[4..8], &save::FORMAT_VERSION.to_le_bytes());
    assert!(World::load(&new).unwrap() == loaded);
}

#[test]
fn a_real_round1_save_still_loads() {
    // Written by the round-1 build (commit b848ce3): a town of ~1,150
    // residents in the morning rush, cars on the road.
    let bytes = include_bytes!("data/round1_town.citysave");
    let mut w = World::load(bytes).expect("round-1 save loads");
    assert!(w.stats.population > 500, "{}", w.stats.population);
    assert!(!w.vehicles.is_empty());
    w.check_invariants().unwrap();
    w.check_sim_invariants().unwrap();
    let pop = w.stats.population;
    for _ in 0..2 * HOUR {
        w.step();
    }
    w.check_sim_invariants().unwrap();
    assert!(w.stats.population > pop / 2);
    assert!(junction::junction_nodes(&w).count() > 0);
}

// -------------------------------------------------------- determinism ----

/// FNV-1a over a save: a checksum of the whole state.
fn checksum(w: &World) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in w.save() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The state after a long scripted run is pinned: the simulation uses only
/// IEEE-exact arithmetic (`sqrt`, `+ - * /`) and the pure-Rust `libm` for
/// anything transcendental, so every machine must reproduce it bit for bit.
/// If a deliberate rule change moves it, update the constant.
#[test]
fn long_scripted_run_has_a_pinned_checksum() {
    let mut w = World::new(2024);
    let mut p = GridPlayer::hierarchy();
    for _ in 0..DAY / 2 {
        p.act(&mut w);
        w.step();
    }
    assert!(w.stats.population > 400);
    let sum = checksum(&w);
    eprintln!("checksum {sum:#018x} pop {}", w.stats.population);
    assert_eq!(sum, PINNED_CHECKSUM, "state checksum changed: {sum:#018x}");
}

/// Moved by six-row zoning blocks and save format 7 (blocks and the save
/// layout are part of the state), then by extension block 5 (the traffic
/// losses per booked day; the state itself was unchanged).
const PINNED_CHECKSUM: u64 = 0xdce4ee73a136bade;
