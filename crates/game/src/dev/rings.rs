// `CITYSIM_DEMO=rings` (also included by the sim's `ringjam` example).
// `CITYSIM_DEMO=rings`: a town with roundabouts of every size, built
// through ordinary commands: two avenues crossing at a large ring, a
// street crossing an avenue at a medium one, a grid of streets with a
// small ring and a mini ring among the houses, a five-arm junction of
// streets as a small ring; the streets' T junctions on the avenue are
// left as give-way junctions (to compare, and to upgrade in play).

use citysim_sim::roundabout::RoundaboutSize as R;
use citysim_sim::{Cell, Command, Curve, RoadKind, World, Zone};

fn at(x: i32, y: i32) -> [f32; 2] {
    Cell::new(x, y).center()
}

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

fn road(w: &mut World, a: [f32; 2], b: [f32; 2], kind: RoadKind, log: &mut Vec<String>) {
    run(
        w,
        Command::BuildCurve {
            curve: Curve::straight(a, b),
            kind,
        },
        log,
    );
}

fn zone(w: &mut World, a: (i32, i32), b: (i32, i32), z: Zone, log: &mut Vec<String>) {
    let q = [at(a.0, a.1), at(b.0, a.1), at(b.0, b.1), at(a.0, b.1)];
    run(w, Command::ZoneArea { quad: q, zone: Some(z) }, log);
}

fn ring(w: &mut World, p: [f32; 2], size: R, log: &mut Vec<String>) {
    match w.roads.node_at(p) {
        Some(n) => {
            run(
                w,
                Command::SetRoundabout {
                    node: n,
                    size: Some(size),
                },
                log,
            );
        }
        None => log.push(format!("no junction at {p:?} for a {size:?} roundabout")),
    }
}

/// Where the five-arm junction is.
pub const HUB: [f32; 2] = [196.0, 1172.0];

/// Build the layout (paid for by the caller); returns refusals.
pub fn rings(w: &mut World) -> Vec<String> {
    let mut log = Vec::new();
    let (s, a) = (RoadKind::Street, RoadKind::Avenue);
    // Avenues.
    road(w, at(11, 128), at(120, 128), a, &mut log);
    road(w, at(64, 94), at(64, 162), a, &mut log);
    // A street across the western avenue stretch.
    road(w, at(40, 100), at(40, 160), s, &mut log);
    // Grid of streets north-east.
    road(w, at(64, 112), at(112, 112), s, &mut log);
    road(w, at(64, 100), at(112, 100), s, &mut log);
    road(w, at(80, 100), at(80, 128), s, &mut log);
    road(w, at(96, 100), at(96, 128), s, &mut log);
    road(w, at(112, 100), at(112, 128), s, &mut log);
    // South-east: industry streets.
    road(w, at(80, 128), at(80, 150), s, &mut log);
    road(w, at(64, 144), at(112, 144), s, &mut log);
    road(w, at(96, 128), at(96, 144), s, &mut log);
    // West: streets off the cross street.
    road(w, at(16, 112), at(40, 112), s, &mut log);
    road(w, at(24, 100), at(24, 128), s, &mut log);
    // The five-arm junction south-west, joined to the cross street.
    road(w, HUB, at(40, 146), s, &mut log);
    for deg in [72.0f32, 144.0, 216.0, 288.0] {
        let d = [deg.to_radians().cos(), deg.to_radians().sin()];
        road(w, HUB, [HUB[0] + d[0] * 72.0, HUB[1] + d[1] * 72.0], s, &mut log);
    }
    // Roundabouts.
    ring(w, at(64, 128), R::Large, &mut log);
    ring(w, at(40, 128), R::Medium, &mut log);
    ring(w, at(80, 112), R::Small, &mut log);
    ring(w, at(96, 112), R::Mini, &mut log);
    ring(w, HUB, R::Small, &mut log);
    // Zoning.
    zone(w, (66, 98), (114, 126), Zone::Residential, &mut log);
    zone(w, (14, 98), (39, 126), Zone::Residential, &mut log);
    zone(w, (8, 134), (39, 162), Zone::Residential, &mut log);
    zone(w, (41, 129), (63, 140), Zone::Commercial, &mut log);
    zone(w, (41, 116), (63, 127), Zone::Commercial, &mut log);
    zone(w, (66, 129), (114, 152), Zone::Industrial, &mut log);
    zone(w, (41, 141), (63, 160), Zone::Residential, &mut log);
    log
}
