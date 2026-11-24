use citysim_sim::*;
fn main() {
    let b = std::fs::read("/tmp/bad_keep.sav").unwrap();
    let mut w = World::load(&b).unwrap();
    eprintln!("inv {:?} roads {:?}", w.check_sim_invariants(), w.roads.validate());
    eprintln!(
        "slots nodes {} segs {}, junctions {}",
        w.roads.nodes.slot_count(),
        w.roads.segments.slot_count(),
        w.traffic.junctions.len()
    );
    for t in 0..300 {
        if t % 50 == 0 {
            eprintln!("step {t}");
        }
        w.step();
    }
    let _ = w.apply(Command::Bulldoze {
        min: Cell::new(0, 0),
        max: Cell::new(255, 255),
    });
    eprintln!("bulldozed");
}
