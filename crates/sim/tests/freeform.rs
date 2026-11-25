//! Free-form roads, zone blocks, rotated lots, and the round-3 save format.

use citysim_sim::scenarios::GridPlayer;
use citysim_sim::{World, balance, save};

const DAY: u32 = balance::TICKS_PER_DAY;
const HOUR: u32 = balance::TICKS_PER_HOUR;

fn grown(seed: u64, days: u32) -> World {
    let mut w = World::new(seed);
    let mut p = GridPlayer::hierarchy();
    for _ in 0..days * DAY {
        p.act(&mut w);
        w.step();
    }
    w
}

/// Round-3 saves (format 5) load: roads become straight curves, buildings
/// keep their place, zoning and people; the town keeps running.
#[test]
fn round3_saves_load_and_keep_running() {
    let w = grown(17, 1);
    assert!(w.stats.population > 100, "{}", w.stats.population);
    let old = save::v5::save_as_v5(&w);
    assert_eq!(&old[4..8], &5u32.to_le_bytes());
    let mut l = World::load(&old).expect("round-3 save loads");
    assert_eq!(l.stats.population, w.stats.population);
    assert_eq!(l.citizens.len(), w.citizens.len());
    assert_eq!(l.roads.segments.len(), w.roads.segments.len());
    for (id, s) in w.roads.segments.iter() {
        let t = &l.roads.segments[id];
        assert_eq!((t.a, t.b, t.kind), (s.a, s.b, s.kind));
        assert!((t.length - s.length).abs() < 1e-3);
        assert!(t.curve.is_straight());
    }
    assert_eq!(l.buildings.len(), w.buildings.len());
    for (id, b) in w.buildings.iter() {
        let lb = &l.buildings[id];
        assert_eq!(lb.footprint.rect(), b.footprint.rect());
        assert_eq!(lb.center(), b.center());
        assert_eq!(lb.rotation(), b.rotation());
    }
    let zoned = |w: &World| w.zoning.cells().filter(|(_, c)| c.zone.is_some()).count();
    assert_eq!(zoned(&l), zoned(&w));
    l.check_invariants().unwrap();
    l.check_sim_invariants().unwrap();
    for _ in 0..2 * HOUR {
        l.step();
    }
    l.check_invariants().unwrap();
    l.check_sim_invariants().unwrap();
    let new = l.save();
    assert_eq!(&new[4..8], &save::FORMAT_VERSION.to_le_bytes());
    assert!(World::load(&new).unwrap() == l);
}

/// The organic town is laid out with world-space commands only, none
/// refused, on more than one lattice, and keeps all invariants while it
/// grows.
#[test]
fn organic_town_builds_and_grows() {
    let mut w = World::new(11);
    let mut refused = Vec::new();
    citysim_sim::scenarios::prebuilt(&mut w, |w| refused = citysim_sim::scenarios::organic(w));
    assert!(refused.is_empty(), "{refused:?}");
    assert!(w.zoning.lattices().count() > 20);
    assert!(w.roads.segments.values().any(|s| !s.curve.is_straight()));
    w.check_invariants().unwrap();
    for _ in 0..DAY / 2 {
        w.step();
    }
    assert!(w.buildings.len() > 20, "{}", w.buildings.len());
    // Buildings stand at their road's angle: some are turned off the axes.
    let turned = w.buildings.values().filter(|b| {
        let r = b.rotation().to_degrees().rem_euclid(90.0);
        r > 1.0 && r < 89.0
    });
    assert!(turned.count() > 5);
    w.check_invariants().unwrap();
    w.check_sim_invariants().unwrap();
}

// ------------------------------------------------- random free-form edits ----

use citysim_sim::{Command, CommandError, Curve, Rng, RoadKind, World as W, Zone};

/// Segments cross only where they share a node; arms at every node are at
/// least the minimum angle apart.
fn check_geometry(w: &W) {
    let segs: Vec<_> = w.roads.segments.iter().collect();
    for (i, (sa, a)) in segs.iter().enumerate() {
        let (alo, ahi) = a.curve.bounds();
        for (sb, b) in &segs[i + 1..] {
            let (blo, bhi) = b.curve.bounds();
            if alo[0] > bhi[0] + 0.1 || blo[0] > ahi[0] + 0.1 || alo[1] > bhi[1] + 0.1 || blo[1] > ahi[1] + 0.1 {
                continue;
            }
            for (_, _, p) in a.curve.intersections(&b.curve) {
                let shared = [a.a, a.b]
                    .iter()
                    .any(|&n| (n == b.a || n == b.b) && citysim_sim::geom::dist(w.roads.nodes[n].pos, p) < 0.1);
                assert!(shared, "{sa:?} and {sb:?} cross at {p:?} without a node");
            }
        }
    }
    for (n, node) in w.roads.nodes.iter() {
        let dirs: Vec<[f32; 2]> = node.segments.iter().map(|&s| w.roads.segments[s].dir_from(n)).collect();
        for i in 0..dirs.len() {
            for j in i + 1..dirs.len() {
                let d = citysim_sim::geom::dot(dirs[i], dirs[j]).clamp(-1.0, 1.0);
                let deg = d.acos().to_degrees();
                assert!(deg >= balance::MIN_ARM_ANGLE_DEG - 0.5, "{n:?} arms {deg} deg apart");
            }
        }
    }
}

fn rand_pt(r: &mut Rng) -> [f32; 2] {
    [200.0 + r.below(400) as f32, 800.0 + r.below(400) as f32]
}

fn rand_command(w: &W, r: &mut Rng) -> Command {
    let segs: Vec<_> = w.roads.segments.iter().map(|(id, s)| (id, s.length)).collect();
    match r.below(12) {
        0..=4 => {
            let a = rand_pt(r);
            let b = rand_pt(r);
            let curve = match r.below(3) {
                0 => Curve::straight(a, b),
                1 => Curve::through_corner(a, rand_pt(r), b),
                _ => {
                    let d = [r.below(200) as f32 - 100.0, r.below(200) as f32 - 100.0];
                    Curve::continuing(a, d, b)
                }
            };
            let kind = if r.below(4) == 0 {
                RoadKind::Avenue
            } else {
                RoadKind::Street
            };
            Command::BuildCurve { curve, kind }
        }
        5 if !segs.is_empty() => {
            let (s, l) = segs[r.below(segs.len() as u32) as usize];
            let a = l * r.below(100) as f32 / 100.0;
            Command::SetRoadKind {
                segment: s,
                from_m: a,
                to_m: a + l * 0.5,
                kind: if r.below(2) == 0 {
                    RoadKind::Avenue
                } else {
                    RoadKind::Street
                },
            }
        }
        6 if !segs.is_empty() => {
            let (s, l) = segs[r.below(segs.len() as u32) as usize];
            let a = l * r.below(100) as f32 / 100.0;
            Command::RemoveRoad {
                segment: s,
                from_m: a,
                to_m: a + 20.0,
            }
        }
        7 => {
            let p = rand_pt(r);
            let (dx, dy) = (10.0 + r.below(40) as f32, 10.0 + r.below(40) as f32);
            Command::BulldozeArea {
                quad: [
                    p,
                    [p[0] + dx, p[1] + 5.0],
                    [p[0] + dx - 5.0, p[1] + dy],
                    [p[0], p[1] + dy],
                ],
            }
        }
        8..=9 => {
            let p = rand_pt(r);
            let s = 30.0 + r.below(120) as f32;
            let z = [Zone::Residential, Zone::Commercial, Zone::Industrial][r.below(3) as usize];
            Command::ZoneArea {
                quad: [p, [p[0] + s, p[1]], [p[0] + s, p[1] + s], [p[0], p[1] + s]],
                zone: Some(z),
            }
        }
        _ => {
            let cells: Vec<_> = w.zoning.cells().map(|(c, _)| c).collect();
            let pick: Vec<_> = if cells.is_empty() {
                Vec::new()
            } else {
                (0..10).map(|_| cells[r.below(cells.len() as u32) as usize]).collect()
            };
            Command::ZoneCells {
                cells: pick,
                zone: if r.below(3) == 0 { None } else { Some(Zone::Residential) },
            }
        }
    }
}

/// Run `n` random commands (with some simulation between them) from seed
/// `seed`, checking everything after each.
fn random_run(seed: u64, n: usize, check: bool) -> W {
    random_run_counted(seed, n, check).0
}

/// Also returns how many commands were applied.
fn random_run_counted(seed: u64, n: usize, check: bool) -> (W, usize) {
    let mut ok = 0;
    let mut w = W::new(seed);
    w.economy.money = 10_000_000;
    let mut r = Rng::new(seed ^ 0x5eed);
    // A link from the highway into the test area.
    w.apply(Command::BuildCurve {
        curve: Curve::straight(w.roads.nodes[w.roads.entry.unwrap()].pos, [180.0, 1000.0]),
        kind: RoadKind::Street,
    })
    .ok();
    for k in 0..n {
        let cmd = rand_command(&w, &mut r);
        let preview = w.preview(&cmd);
        let before = if check { Some(w.clone()) } else { None };
        let got = w.apply(cmd.clone());
        ok += usize::from(got.is_ok());
        if check && let Err(e) = w.roads.validate() {
            panic!("after {cmd:?} -> {got:?}: {e}");
        }
        match (&preview, &got) {
            (Ok(p), Ok(o)) => assert_eq!(p, o, "{cmd:?}"),
            (Err(a), Err(b)) => assert_eq!(a, b),
            _ => panic!("preview {preview:?} but apply {got:?} for {cmd:?}"),
        }
        if let (Err(e), Some(b)) = (&got, &before) {
            assert!(*b == w, "failed {cmd:?} ({e}) changed the world");
            // Every refusal says where (except money and bad ids).
            if !matches!(e, CommandError::InsufficientFunds { .. } | CommandError::NoSuchRoad) {
                assert!(
                    e.at().is_some() || matches!(e, CommandError::InvalidRoadKind(_)),
                    "{e:?}"
                );
            }
        }
        for _ in 0..(r.below(4) * 40) {
            w.step();
        }
        if check && k % 5 == 0 {
            w.check_invariants().unwrap();
            check_geometry(&w);
        }
    }
    (w, ok)
}

#[test]
fn random_free_form_edits_keep_the_world_valid() {
    for seed in [1, 2, 3, 4, 5, 6] {
        let (w, ok) = random_run_counted(seed, 200, true);
        assert!(ok > 50, "only {ok} of 200 random commands applied");
        w.check_invariants().unwrap();
        w.check_sim_invariants().unwrap();
        check_geometry(&w);
        assert!(w.roads.segments.len() > 10);
    }
}

#[test]
fn random_free_form_edits_are_deterministic_and_survive_save_load() {
    let a = random_run(9, 80, false);
    let b = random_run(9, 80, false);
    assert!(a == b, "same commands, different worlds");
    let mut l = W::load(&a.save()).unwrap();
    assert!(l == a);
    let mut a = a;
    for _ in 0..HOUR {
        a.step();
        l.step();
    }
    assert!(a == l, "save/load lost lockstep");
}

/// Buildings that grew after the last road removal all front a road.
#[test]
fn grown_buildings_front_their_road() {
    let mut w = World::new(4);
    citysim_sim::scenarios::prebuilt(&mut w, |w| {
        citysim_sim::scenarios::organic(w);
    });
    for _ in 0..DAY {
        w.step();
    }
    assert!(w.buildings.len() > 50);
    for (id, b) in w.buildings.iter() {
        assert!(
            citysim_sim::growth::has_frontage(&w, &b.footprint, b.facing),
            "{id:?} at {:?} has no road in front",
            b.center()
        );
        assert_eq!(b.cells().count() as u32, b.footprint.area());
    }
}

/// No car body ever overlaps another (oriented rectangles at the exact
/// drawing poses) on a curved approach to a signal, at a crossing at 35
/// degrees and at a five-arm junction, with heavy traffic.
#[test]
fn car_bodies_never_overlap_on_curves_and_odd_angles() {
    let mut w = W::new(3);
    w.economy.money = 10_000_000;
    let b = |w: &mut W, c: Curve, k: RoadKind| w.apply(Command::BuildCurve { curve: c, kind: k }).unwrap();
    // Crossing of avenues with a quarter-circle south arm.
    b(
        &mut w,
        Curve::straight([440.0, 640.0], [840.0, 640.0]),
        RoadKind::Avenue,
    );
    b(
        &mut w,
        Curve::straight([640.0, 440.0], [640.0, 640.0]),
        RoadKind::Avenue,
    );
    b(
        &mut w,
        Curve::from_tangents([470.0, 800.0], [1.0, 0.0], [640.0, 640.0], [0.0, -1.0]).unwrap(),
        RoadKind::Avenue,
    );
    // A street across the east arm at 35 degrees, and a five-arm junction.
    let d = [35f32.to_radians().cos(), 35f32.to_radians().sin()];
    b(
        &mut w,
        Curve::straight(
            [760.0 - 80.0 * d[0], 640.0 - 80.0 * d[1]],
            [760.0 + 80.0 * d[0], 640.0 + 80.0 * d[1]],
        ),
        RoadKind::Street,
    );
    let hub = [640.0, 360.0];
    b(&mut w, Curve::straight([640.0, 440.0], hub), RoadKind::Street);
    for deg in [0.0f32, 150.0, 215.0, 290.0] {
        let r = deg.to_radians();
        b(
            &mut w,
            Curve::straight(hub, [hub[0] + 70.0 * r.cos(), hub[1] + 70.0 * r.sin()]),
            RoadKind::Street,
        );
    }
    let ends = [
        [470.0, 800.0],
        [450.0, 640.0],
        [830.0, 640.0],
        [hub[0] + 65.0, hub[1]],
        [760.0 - 75.0 * d[0], 640.0 - 75.0 * d[1]],
        [760.0 + 75.0 * d[0], 640.0 + 75.0 * d[1]],
        [
            hub[0] + 65.0 * 150f32.to_radians().cos(),
            hub[1] + 65.0 * 150f32.to_radians().sin(),
        ],
    ];
    let mut k = 0usize;
    let mut worst = 0;
    let mut max_cars = 0;
    for t in 0..6 * 60 * citysim_sim::TICKS_PER_SECOND {
        if t % 6 == 0 {
            let (a, z) = (ends[k % ends.len()], ends[(k * 3 + 1) % ends.len()]);
            if a != z {
                citysim_sim::traffic::debug_request(&mut w, a, z);
            }
            k += 1;
        }
        w.step();
        let bodies = citysim_sim::traffic::car_bodies(&w);
        max_cars = max_cars.max(bodies.len());
        let hits = citysim_sim::traffic::overlapping_bodies(&bodies, 0.02);
        worst = worst.max(hits.len());
        if let Some(&(a, b, depth)) = hits.first() {
            let show = |i: usize| {
                let v = &w.vehicles[bodies[i].0];
                format!(
                    "seg {:?} fwd {} lane {} shift {:.2} prog {:.2} v {:.1} box {:?} pos {:?}",
                    v.segment,
                    v.forward,
                    v.lane,
                    v.lane_shift,
                    v.progress,
                    v.speed,
                    v.junction
                        .map(|j| (j.node, j.movement, j.turn, j.distance, j.from_lane, j.to_lane)),
                    v.pos
                )
            };
            panic!(
                "tick {t}: {} overlapping pairs, depth {depth:.2}: {} / {}",
                hits.len(),
                show(a),
                show(b)
            );
        }
    }
    assert!(max_cars > 40, "{max_cars}");
    let _ = worst;
}
