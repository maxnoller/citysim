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
