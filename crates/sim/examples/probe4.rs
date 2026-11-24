use citysim_sim::*;
const DAY: u64 = balance::TICKS_PER_DAY as u64;

fn main() {
    let mut w = World::new(9);
    let road = |w: &mut World, a: (i32, i32), b: (i32, i32)| {
        w.apply(Command::BuildRoad {
            from: Cell::new(a.0, a.1),
            to: Cell::new(b.0, b.1),
        })
        .unwrap();
    };
    // Long snake: homes near the highway, jobs at the far end.
    road(&mut w, (11, 128), (250, 128));
    road(&mut w, (250, 128), (250, 140));
    road(&mut w, (250, 140), (12, 140));
    w.apply(Command::SetZone {
        min: Cell::new(12, 126),
        max: Cell::new(40, 127),
        zone: Some(Zone::Residential),
    })
    .unwrap();
    w.apply(Command::SetZone {
        min: Cell::new(12, 141),
        max: Cell::new(40, 142),
        zone: Some(Zone::Commercial),
    })
    .unwrap();
    w.apply(Command::SetZone {
        min: Cell::new(12, 138),
        max: Cell::new(40, 139),
        zone: Some(Zone::Industrial),
    })
    .unwrap();
    for d in 0..12 {
        for _ in 0..DAY {
            w.step();
        }
        eprintln!(
            "day {} pop {} hh {} bld {} avg_commute {:.0} attract {:.2} demand {:?}",
            w.time.day,
            w.stats.population,
            w.households.len(),
            w.buildings.len(),
            w.traffic.avg_commute_s,
            w.growth.attractiveness,
            w.demand
        );
        if d == 4 {
            // jobs district bulldozed
            w.apply(Command::Bulldoze {
                min: Cell::new(12, 138),
                max: Cell::new(40, 142),
            })
            .unwrap();
            eprintln!("bulldozed jobs");
        }
    }
}
