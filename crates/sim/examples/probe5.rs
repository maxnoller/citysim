use citysim_sim::*;
const HOUR: u64 = balance::TICKS_PER_HOUR as u64;
fn build(w: &mut World, a: (i32, i32), b: (i32, i32)) -> bool {
    w.apply(Command::BuildRoad {
        from: Cell::new(a.0, a.1),
        to: Cell::new(b.0, b.1),
    })
    .is_ok()
}
fn zone(w: &mut World, a: (i32, i32), b: (i32, i32), z: Zone) {
    w.apply(Command::SetZone {
        min: Cell::new(a.0, a.1),
        max: Cell::new(b.0, b.1),
        zone: Some(z),
    })
    .unwrap();
}
fn main() {
    let mut w = World::new(32);
    w.economy.money = 10_000_000;
    build(&mut w, (11, 128), (40, 128));
    for (a, b) in [
        ((40, 100), (110, 100)),
        ((110, 100), (110, 156)),
        ((110, 156), (40, 156)),
        ((40, 156), (40, 100)),
    ] {
        assert!(build(&mut w, a, b));
    }
    zone(&mut w, (38, 98), (112, 127), Zone::Residential);
    zone(&mut w, (38, 129), (112, 140), Zone::Commercial);
    zone(&mut w, (38, 141), (112, 158), Zone::Industrial);
    w.economy.money = 50_000;
    let mut total = 0;
    for h in 0..60 {
        if h == 26 {
            w.economy.money = 10_000_000;
            w.apply(Command::Bulldoze {
                min: Cell::new(105, 110),
                max: Cell::new(115, 146),
            })
            .unwrap();
            eprintln!("BULLDOZE");
        }
        if h == 28 {
            assert!(build(&mut w, (110, 100), (110, 156)));
            eprintln!("REBUILD");
        }
        for _ in 0..HOUR {
            let b = w.traffic.trips_today;
            w.step();
            total += w.traffic.trips_today.saturating_sub(b);
        }
        let states = w.households.values().fold([0; 4], |mut a, h| {
            a[h.state as usize] += 1;
            a
        });
        eprintln!(
            "{:5.1}h hh {} states {:?} cars {} pending {} trips {} total {} hopeless {} bld {}",
            w.time.hour(),
            w.households.len(),
            states,
            w.vehicles.len(),
            w.traffic.pending.len(),
            w.traffic.trips_today,
            total,
            w.households.values().filter(|h| h.hopeless_commutes > 0).count(),
            w.buildings.len()
        );
        let _ = h;
    }
}
