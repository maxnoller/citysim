use citysim_sim::*;
use std::time::Instant;

fn main() {
    let mut w = World::new(5);
    w.economy.money = 100_000_000;
    let t = Instant::now();
    let spacing: i32 = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(4);
    for x in (12..=250).step_by(spacing as usize) {
        let _ = w.apply(Command::BuildRoad {
            from: Cell::new(x, 4),
            to: Cell::new(x, 250),
        });
    }
    for y in (4..=250).step_by(spacing as usize) {
        let _ = w.apply(Command::BuildRoad {
            from: Cell::new(12, y),
            to: Cell::new(250, y),
        });
    }
    let _ = w.apply(Command::BuildRoad {
        from: Cell::new(11, 128),
        to: Cell::new(12, 128),
    });
    eprintln!(
        "mesh {} segs {} nodes built in {:?}",
        w.roads.segments.len(),
        w.roads.nodes.len(),
        t.elapsed()
    );
    let t = Instant::now();
    let _ = w.apply(Command::BuildRoad {
        from: Cell::new(13, 5),
        to: Cell::new(250, 5),
    });
    eprintln!("one long road in {:?}", t.elapsed());
    for size in [5, 20, 60, 255] {
        let cmd = Command::Bulldoze {
            min: Cell::new(0, 0),
            max: Cell::new(size, size),
        };
        let t = Instant::now();
        let p = w.preview(&cmd).unwrap();
        eprintln!("preview bulldoze {size}: {} cells in {:?}", p.cells.len(), t.elapsed());
    }
    let t = Instant::now();
    let cmd = Command::SetZone {
        min: Cell::new(0, 0),
        max: Cell::new(255, 255),
        zone: Some(Zone::Residential),
    };
    w.preview(&cmd).unwrap();
    eprintln!("preview zone all {:?}", t.elapsed());
    let t = Instant::now();
    w.apply(Command::Bulldoze {
        min: Cell::new(0, 0),
        max: Cell::new(60, 60),
    })
    .unwrap();
    eprintln!("apply bulldoze 60: {:?}", t.elapsed());
    let t = Instant::now();
    w.apply(Command::Bulldoze {
        min: Cell::new(0, 0),
        max: Cell::new(255, 255),
    })
    .unwrap();
    eprintln!(
        "apply bulldoze all: {:?} segs left {}",
        t.elapsed(),
        w.roads.segments.len()
    );
}
