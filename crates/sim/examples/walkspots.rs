//! Where people walk in a saved city: the busiest 40 m squares of walkers
//! (for camera poses), and a few walkers' places.
//!
//! `cargo run -p citysim_sim --release --example walkspots -- city.citysave [steps]`
use citysim_sim::*;
use std::collections::BTreeMap;

fn main() {
    let path = std::env::args().nth(1).expect("save file");
    let steps: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut w = World::load(&std::fs::read(path).unwrap()).unwrap();
    for _ in 0..steps {
        w.step();
    }
    let mut grid: BTreeMap<(i32, i32), (u32, u32, u32)> = BTreeMap::new();
    for x in w.walking.walkers.values() {
        let k = ((x.pos[0] / 40.0) as i32, (x.pos[1] / 40.0) as i32);
        let e = grid.entry(k).or_default();
        e.0 += 1;
        if x.state == walk::WalkState::Waiting {
            e.1 += 1;
        }
        if x.state == walk::WalkState::Crossing {
            e.2 += 1;
        }
    }
    let mut v: Vec<_> = grid.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1.0));
    println!("{} walkers at {:.2} h", w.walking.walkers.len(), w.time.hour());
    for ((x, y), (n, wt, cr)) in v.iter().take(12) {
        println!(
            "square centre ({}, {}): {n} walkers, {wt} waiting, {cr} crossing",
            x * 40 + 20,
            y * 40 + 20
        );
    }
    if let Some(net) = walk::network(&w) {
        for c in net.crossings.iter().filter(|c| c.node.is_none()).take(6) {
            println!("mid-block zebra at {:?}", c.center);
        }
    }
    for n in junction::junction_nodes(&w).take(400) {
        if junction::junction_control(&w, n) == Some(JunctionControl::Signal) {
            let p = w.roads.nodes[n].pos;
            let near = w
                .walking
                .walkers
                .values()
                .filter(|x| (x.pos[0] - p[0]).abs() < 30.0 && (x.pos[1] - p[1]).abs() < 30.0)
                .count();
            if near > 0 {
                println!("signal at {:?}: {near} walkers near", p);
            }
        }
    }
}
