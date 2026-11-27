//! `Command::RemoveRoads`: several parts in one command.
use citysim_sim::*;

fn grid_town() -> (World, u64) {
    let mut w = World::new(4);
    w.economy.money = 10_000_000;
    w.apply(Command::BuildCurve {
        curve: Curve::straight([92.0, 1028.0], [500.0, 1028.0]),
        kind: RoadKind::Street,
    })
    .unwrap();
    let first = w.roads.next_serial;
    for j in 0..4 {
        let y = 1028.0 + 40.0 * j as f32;
        w.apply(Command::BuildCurve {
            curve: Curve::straight([200.0, y], [400.0, y]),
            kind: RoadKind::Street,
        })
        .ok();
    }
    for i in 0..6 {
        let x = 200.0 + 40.0 * i as f32;
        w.apply(Command::BuildCurve {
            curve: Curve::straight([x, 1028.0], [x, 1148.0]),
            kind: RoadKind::Street,
        })
        .unwrap();
    }
    (w, first)
}

fn new_parts(w: &World, first: u64) -> Vec<(SegmentId, f32, f32)> {
    w.roads
        .segments
        .iter()
        .filter(|(_, s)| s.serial >= first)
        .map(|(id, s)| (id, 0.0, s.length))
        .collect()
}

#[test]
fn removing_parts_at_once_leaves_what_removing_them_one_by_one_does() {
    let (w, first) = grid_town();
    let parts = new_parts(&w, first);
    assert!(parts.len() > 20, "{} parts", parts.len());
    let mut batch = w.clone();
    let cmd = Command::RemoveRoads { parts: parts.clone() };
    let preview = batch.preview(&cmd).unwrap();
    let out = batch.apply(cmd).unwrap();
    assert_eq!(preview.removed, out.removed);
    assert_eq!(out.removed.len(), parts.len());
    let mut single = w.clone();
    loop {
        let Some(&(segment, from_m, to_m)) = new_parts(&single, first).first() else {
            break;
        };
        single.apply(Command::RemoveRoad { segment, from_m, to_m }).unwrap();
    }
    let shape = |w: &World| {
        let mut v: Vec<([i32; 2], [i32; 2], RoadKind)> = w
            .roads
            .segments
            .values()
            .map(|s| {
                let r = |p: [f32; 2]| [(p[0] * 10.0).round() as i32, (p[1] * 10.0).round() as i32];
                let (a, b) = (r(s.curve.start()), r(s.curve.end()));
                (a.min(b), a.max(b), s.kind)
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(shape(&batch), shape(&single));
    assert_eq!(batch.roads.nodes.len(), single.roads.nodes.len());
    assert_eq!(batch.economy.money, single.economy.money, "the same bulldozing cost");
    batch.check_sim_invariants().unwrap();
}

#[test]
fn a_part_listed_twice_or_a_missing_segment_is_refused() {
    let (w, first) = grid_town();
    let parts = new_parts(&w, first);
    let twice = Command::RemoveRoads {
        parts: vec![parts[0], parts[1], parts[0]],
    };
    assert!(w.preview(&twice).is_err());
    assert!(w.clone().apply(twice).is_err());
    assert!(w.preview(&Command::RemoveRoads { parts: Vec::new() }).is_err());
    let mut gone = w.clone();
    let (s, a, b) = parts[0];
    gone.apply(Command::RemoveRoad {
        segment: s,
        from_m: a,
        to_m: b,
    })
    .unwrap();
    let stale = Command::RemoveRoads {
        parts: vec![parts[0], parts[1]],
    };
    assert!(gone.preview(&stale).is_err());
    assert!(gone.apply(stale).is_err());
}
