//! A free-form sample town built through ordinary commands (`BuildCurve`,
//! `ZoneArea`): a curved avenue from the highway, a grid rotated 25 degrees
//! north of it meeting a straight grid south of it, a ring road with a
//! street through it, and curved cul-de-sacs. Used by `CITYSIM_DEMO=organic`
//! until the sim's own `scenarios::organic` exists.

use citysim_sim::{Command, Curve, RoadKind, World, Zone};

type P = [f32; 2];

fn add(a: P, b: P) -> P {
    [a[0] + b[0], a[1] + b[1]]
}

fn mul(a: P, k: f32) -> P {
    [a[0] * k, a[1] * k]
}

/// Apply a command, noting a refusal.
fn run(w: &mut World, cmd: Command, log: &mut Vec<String>) -> bool {
    let what = format!("{cmd:?}");
    match w.apply(cmd) {
        Ok(_) => true,
        Err(e) => {
            log.push(format!("{e}: {}", &what[..what.len().min(140)]));
            false
        }
    }
}

fn road(w: &mut World, curve: Curve, kind: RoadKind, log: &mut Vec<String>) -> bool {
    run(w, Command::BuildCurve { curve, kind }, log)
}

fn street(w: &mut World, a: P, b: P, log: &mut Vec<String>) -> bool {
    road(w, Curve::straight(a, b), RoadKind::Street, log)
}

fn zone(w: &mut World, quad: [P; 4], z: Zone, log: &mut Vec<String>) {
    run(w, Command::ZoneArea { quad, zone: Some(z) }, log);
}

/// Point of `curve` whose x is `x` (the curve runs left to right).
fn at_x(curve: &Curve, x: f32) -> P {
    let len = curve.length();
    let (mut lo, mut hi) = (0.0, len);
    for _ in 0..40 {
        let mid = (lo + hi) * 0.5;
        if curve.point_at(mid, 0.0)[0] < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve.point_at(lo, 0.0)
}

/// Build the layout and zone it. Returns the refused commands.
pub fn organic(w: &mut World) -> Vec<String> {
    let mut log = Vec::new();
    // The inner end of the highway stub.
    let entry = w
        .roads
        .entry
        .and_then(|n| {
            let s = *w.roads.nodes[n].segments.first()?;
            let seg = &w.roads.segments[s];
            Some(w.roads.nodes[seg.other(n)].pos)
        })
        .unwrap_or([92.0, 1028.0]);
    // The curved main avenue, in two pieces (an S-bend).
    let av1 = Curve::new(entry, [300.0, entry[1]], [430.0, 900.0], [640.0, 880.0]);
    let av2 = Curve::new([640.0, 880.0], [850.0, 860.0], [930.0, 1010.0], [1140.0, 1000.0]);
    road(w, av1, RoadKind::Avenue, &mut log);
    road(w, av2, RoadKind::Avenue, &mut log);

    // Straight grid south of the avenue, its north ends on the avenue.
    let xs = [170.0f32, 235.0, 300.0, 365.0];
    for &x in &xs {
        let top = at_x(&av1, x);
        street(w, top, [x, 1290.0], &mut log);
    }
    for y in [1110.0f32, 1200.0, 1290.0] {
        street(w, [xs[0], y], [xs[3], y], &mut log);
    }

    // A grid rotated 25 degrees north of the avenue; its cross streets run
    // down to the avenue.
    let ang = 25f32.to_radians();
    let u: P = [ang.cos(), -ang.sin()];
    let v: P = [ang.sin(), ang.cos()];
    let origin: P = [250.0, 610.0];
    let at = |i: f32, j: f32| add(origin, add(mul(u, i), mul(v, j)));
    for j in [0.0f32, 72.0, 144.0, 216.0] {
        street(w, at(-24.0, j), at(384.0, j), &mut log);
    }
    for i in [0.0f32, 72.0, 144.0, 216.0, 288.0, 360.0] {
        // Down to the avenue: where the line along v meets it.
        let mut end = at(i, 216.0);
        for k in 0..200 {
            let p = at(i, 216.0 + k as f32 * 2.0);
            let (d, dist, q) = av1.nearest(p);
            let (_, dist2, q2) = av2.nearest(p);
            let _ = d;
            if dist < 1.0 {
                end = q;
                break;
            }
            if dist2 < 1.0 {
                end = q2;
                break;
            }
        }
        street(w, at(i, 0.0), end, &mut log);
    }
    // Curved cul-de-sacs off the rotated grid's north street.
    for (i, bend) in [(36.0f32, 1.0f32), (180.0, -1.0), (324.0, 1.0)] {
        let s = at(i, 0.0);
        let e = add(s, add(mul(v, -70.0), mul(u, 40.0 * bend)));
        if let Some(c) = Curve::from_tangents(s, mul(v, -1.0), e, add(mul(v, -0.3), mul(u, bend))) {
            road(w, c, RoadKind::Street, &mut log);
        }
    }

    // A ring road south of the avenue's second piece, a street through it.
    let (c, r) = ([900.0f32, 1250.0f32], 110.0f32);
    let k = 0.5523 * r;
    let pts = [[c[0] + r, c[1]], [c[0], c[1] + r], [c[0] - r, c[1]], [c[0], c[1] - r]];
    let tans: [P; 4] = [[0.0, 1.0], [-1.0, 0.0], [0.0, -1.0], [1.0, 0.0]];
    for q in 0..4 {
        let (a, b) = (pts[q], pts[(q + 1) % 4]);
        let (ta, tb) = (tans[q], tans[(q + 1) % 4]);
        road(
            w,
            Curve::new(a, add(a, mul(ta, k)), add(b, mul(tb, -k)), b),
            RoadKind::Street,
            &mut log,
        );
    }
    let top = at_x(&av2, 900.0);
    street(w, top, [900.0, 1440.0], &mut log);
    street(w, [790.0, 1250.0], [700.0, 1250.0], &mut log);
    street(w, [1010.0, 1250.0], [1120.0, 1320.0], &mut log);

    // Odd angles east of the rotated grid: a five-arm junction, a street
    // meeting the avenue at 30 degrees, and a T on the outside of a bend.
    let star: P = [760.0, 560.0];
    for deg in [10.0f32, 80.0, 150.0, 220.0, 290.0] {
        let d = [deg.to_radians().cos(), deg.to_radians().sin()];
        street(w, star, add(star, mul(d, 110.0)), &mut log);
    }
    // Leaves the avenue at 30 degrees to its direction there.
    let (d30, _, foot) = av2.nearest([1000.0, 990.0]);
    let tan = av2.pose_at(d30, 0.0).dir;
    let a30 = 30f32.to_radians();
    let dir = [
        tan[0] * a30.cos() - tan[1] * a30.sin(),
        tan[0] * a30.sin() + tan[1] * a30.cos(),
    ];
    street(w, foot, add(foot, mul(dir, 150.0)), &mut log);
    // A bend, and a street meeting it on the outside.
    let bend = Curve::new([1000.0, 520.0], [1100.0, 520.0], [1150.0, 560.0], [1150.0, 680.0]);
    road(w, bend, RoadKind::Street, &mut log);
    let (dm, _, _) = bend.nearest([1120.0, 540.0]);
    let pose = bend.pose_at(dm, 0.0);
    let outward = [pose.dir[1], -pose.dir[0]];
    street(w, pose.pos, add(pose.pos, mul(outward, 90.0)), &mut log);
    zone(
        w,
        [[690.0, 480.0], [880.0, 480.0], [880.0, 650.0], [690.0, 650.0]],
        Zone::Residential,
        &mut log,
    );
    zone(
        w,
        [[980.0, 440.0], [1200.0, 440.0], [1200.0, 700.0], [980.0, 700.0]],
        Zone::Residential,
        &mut log,
    );

    // Zoning: houses in the rotated grid and the south grid, shops along
    // the avenue, works east of the ring.
    zone(
        w,
        [at(0.0, 0.0), at(360.0, 0.0), at(360.0, 216.0), at(0.0, 216.0)],
        Zone::Residential,
        &mut log,
    );
    zone(
        w,
        [at(-40.0, -90.0), at(400.0, -90.0), at(400.0, 0.0), at(-40.0, 0.0)],
        Zone::Residential,
        &mut log,
    );
    zone(
        w,
        [[150.0, 1040.0], [390.0, 1000.0], [390.0, 1310.0], [150.0, 1310.0]],
        Zone::Residential,
        &mut log,
    );
    zone(
        w,
        [[100.0, 990.0], [660.0, 840.0], [700.0, 930.0], [100.0, 1070.0]],
        Zone::Commercial,
        &mut log,
    );
    zone(
        w,
        [[640.0, 840.0], [1150.0, 950.0], [1150.0, 1050.0], [640.0, 930.0]],
        Zone::Commercial,
        &mut log,
    );
    zone(
        w,
        [[770.0, 1120.0], [1030.0, 1120.0], [1030.0, 1380.0], [770.0, 1380.0]],
        Zone::Residential,
        &mut log,
    );
    zone(
        w,
        [[1030.0, 1100.0], [1250.0, 1100.0], [1250.0, 1400.0], [1030.0, 1400.0]],
        Zone::Industrial,
        &mut log,
    );
    zone(
        w,
        [[600.0, 1100.0], [770.0, 1100.0], [770.0, 1400.0], [600.0, 1400.0]],
        Zone::Industrial,
        &mut log,
    );
    log
}
