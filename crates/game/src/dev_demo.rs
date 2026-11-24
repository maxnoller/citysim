//! THROWAWAY — DELETE WHEN THE REAL SIMULATION LANDS.
//!
//! `CITYSIM_DEMO=town` / `CITYSIM_DEMO=stress`: fakes a living city so gfx
//! and ui have something to show before phase-2 sim logic exists. It builds
//! roads and zones through `Command`s, then writes fake buildings, vehicles,
//! segment loads, demand, money and stats straight into the `World` every
//! sim tick. None of this is game logic; the integration agent removes this
//! file (and its two lines in `dev/mod.rs` / `dev/autoshot.rs`).

use bevy::prelude::*;
use citysim_sim::{
    Building, Cell, CellRect, Command, Facing, Occupant, Rng, SegmentId, TICKS_PER_SECOND, Vehicle, World, Zone,
};
use slotmap::Key;

use crate::shared::{SimClock, SimStep, SimWorld};

/// Camera focus for the town.
pub const TOWN_CENTER: Cell = Cell::new(55, 128);
/// A street-level intersection in the town (also used for stress).
pub const TOWN_STREET_VIEW: Cell = Cell::new(35, 128);
/// Camera focus for the stress map.
pub const STRESS_CENTER: Cell = Cell::new(114, 128);

/// Sim seconds over which buildings appear.
const GROW_SECONDS: f32 = 20.0;

pub struct DevDemoPlugin {
    pub stress: bool,
}

#[derive(Resource)]
struct Demo {
    stress: bool,
    rng: Rng,
    candidates: Vec<Cell>,
    next: usize,
    total_buildings: usize,
    total_vehicles: usize,
    ticks: u64,
    money_acc: f64,
}

impl Plugin for DevDemoPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Demo {
            stress: self.stress,
            rng: Rng::new(0xC17E),
            candidates: Vec::new(),
            next: 0,
            total_buildings: if self.stress { 6000 } else { 350 },
            total_vehicles: if self.stress { 3000 } else { 300 },
            ticks: 0,
            money_acc: 0.0,
        })
        .add_systems(Startup, build_town)
        .add_systems(FixedUpdate, demo_tick.after(SimStep));
    }
}

fn apply(world: &mut World, cmd: Command) {
    if let Err(e) = world.apply(cmd) {
        warn!("dev demo: {cmd:?} failed: {e}");
    }
}

fn road(world: &mut World, a: (i32, i32), b: (i32, i32)) {
    apply(
        world,
        Command::BuildRoad {
            from: Cell::new(a.0, a.1),
            to: Cell::new(b.0, b.1),
        },
    );
}

fn zone(world: &mut World, a: (i32, i32), b: (i32, i32), z: Zone) {
    apply(
        world,
        Command::SetZone {
            min: Cell::new(a.0, a.1),
            max: Cell::new(b.0, b.1),
            zone: Some(z),
        },
    );
}

fn build_town(mut sim: ResMut<SimWorld>, mut demo: ResMut<Demo>) {
    let w = &mut sim.world;
    if demo.stress {
        w.economy.money = 10_000_000;
        let (x0, x1, y0, y1) = (14, 214, 28, 228);
        road(w, (11, 128), (x0, 128));
        for x in (x0..=x1).step_by(8) {
            road(w, (x, y0), (x, y1));
        }
        for y in (y0..=y1).step_by(8) {
            road(w, (x0, y), (x1, y));
        }
        zone(w, (x0, y0), (x1, y1), Zone::Residential);
        for y in (y0..=y1).step_by(40) {
            zone(w, (x0, y - 2), (x1, y + 2), Zone::Commercial);
        }
        zone(w, (150, y0), (x1, y1), Zone::Industrial);
    } else {
        road(w, (11, 128), (95, 128));
        for x in [20, 35, 50, 65, 80] {
            road(w, (x, 100), (x, 156));
        }
        for y in [100, 114, 142, 156] {
            road(w, (20, y), (80, y));
        }
        road(w, (80, 128), (95, 113));
        road(w, (95, 113), (95, 128));
        zone(w, (21, 101), (79, 155), Zone::Residential);
        zone(w, (12, 124), (95, 132), Zone::Commercial);
        zone(w, (66, 101), (79, 155), Zone::Industrial);
        zone(w, (81, 112), (94, 127), Zone::Industrial);
    }
    // Candidate cells for buildings, shuffled.
    let mut cands: Vec<Cell> = (0..citysim_sim::MAP_CELLS)
        .flat_map(|y| (0..citysim_sim::MAP_CELLS).map(move |x| Cell::new(x, y)))
        .filter(|&c| {
            w.grid
                .get(c)
                .is_some_and(|cd| cd.zone.is_some() && cd.occupant == Occupant::Empty)
        })
        .collect();
    for i in (1..cands.len()).rev() {
        let j = demo.rng.below(i as u32 + 1) as usize;
        cands.swap(i, j);
    }
    demo.candidates = cands;
    // Vehicles.
    let segs: Vec<SegmentId> = w.roads.segments.keys().collect();
    for _ in 0..demo.total_vehicles {
        let s = segs[demo.rng.below(segs.len() as u32) as usize];
        let seg = &w.roads.segments[s];
        let v = Vehicle {
            pos: [0.0; 2],
            heading: 0.0,
            segment: Some(s),
            progress: demo.rng.next_f32() * seg.length,
            forward: demo.rng.below(2) == 0,
            speed: seg.speed_limit * (0.6 + 0.4 * demo.rng.next_f32()),
            route: Vec::new(),
            destination: None,
            household: None,
            seed: demo.rng.next_u64() as u32,
        };
        w.vehicles.insert(v);
    }
    info!(
        "dev demo: {} roads, {} candidate cells, {} vehicles",
        w.roads.segments.len(),
        demo.candidates.len(),
        w.vehicles.len()
    );
}

fn demo_tick(mut sim: ResMut<SimWorld>, clock: Res<SimClock>, mut demo: ResMut<Demo>) {
    for _ in 0..clock.steps_per_tick() {
        tick(&mut sim.world, &mut demo);
    }
}

fn facing_for(w: &World, r: CellRect, rng: &mut Rng) -> Facing {
    let start = rng.below(4) as usize;
    for dist in 1..=2 {
        for k in 0..4 {
            let f = Facing::ALL[(start + k) % 4];
            let side: Vec<Cell> = match f {
                Facing::PlusX => (r.min.y..=r.max.y).map(|y| Cell::new(r.max.x + dist, y)).collect(),
                Facing::MinusX => (r.min.y..=r.max.y).map(|y| Cell::new(r.min.x - dist, y)).collect(),
                Facing::PlusY => (r.min.x..=r.max.x).map(|x| Cell::new(x, r.max.y + dist)).collect(),
                Facing::MinusY => (r.min.x..=r.max.x).map(|x| Cell::new(x, r.min.y - dist)).collect(),
            };
            if side.iter().any(|&c| w.grid.is_road(c)) {
                return f;
            }
        }
    }
    Facing::PlusY
}

fn spawn_building(w: &mut World, demo: &mut Demo, c: Cell) {
    let Some(kind) = w
        .grid
        .get(c)
        .and_then(|cd| (cd.occupant == Occupant::Empty).then_some(cd.zone).flatten())
    else {
        return;
    };
    let rng = &mut demo.rng;
    let mut footprint = CellRect::single(c);
    if kind != Zone::Residential && rng.below(10) < 3 {
        let big = CellRect::from_corners(c, c.offset(1, 1));
        if big.cells().all(|x| {
            w.grid
                .get(x)
                .is_some_and(|cd| cd.zone == Some(kind) && cd.occupant == Occupant::Empty)
        }) {
            footprint = big;
        }
    }
    let level = 1 + match kind {
        Zone::Residential => rng.below(3),
        Zone::Commercial => rng.below(4),
        Zone::Industrial => rng.below(2),
    } as u8;
    let per = match kind {
        Zone::Residential => 4,
        Zone::Commercial => 6,
        Zone::Industrial => 10,
    };
    let area = (footprint.width() * footprint.height()) as u32;
    let capacity = per * level as u32 * area;
    let b = Building {
        kind,
        footprint,
        level,
        capacity,
        occupied: rng.below(capacity / 2 + 1),
        facing: facing_for(w, footprint, rng),
        seed: rng.next_u64() as u32,
        created_tick: w.time.tick,
    };
    let id = w.buildings.insert(b);
    for x in footprint.cells() {
        if let Some(cd) = w.grid.get_mut(x) {
            cd.occupant = Occupant::Building(id);
        }
    }
    w.versions.buildings += 1;
    w.versions.grid += 1;
}

fn tick(w: &mut World, demo: &mut Demo) {
    demo.ticks += 1;
    let t = demo.ticks as f32 / TICKS_PER_SECOND as f32;
    let dt = 1.0 / TICKS_PER_SECOND as f32;

    // Buildings appear over the first GROW_SECONDS.
    let target = (demo.total_buildings as f32 * (t / GROW_SECONDS).min(1.0)) as usize;
    let before = w.buildings.len();
    while w.buildings.len() < target && demo.next < demo.candidates.len() {
        let c = demo.candidates[demo.next];
        demo.next += 1;
        spawn_building(w, demo, c);
    }
    if before < demo.total_buildings && w.buildings.len() >= demo.total_buildings {
        info!("dev demo: all {} buildings placed at sim t={t:.1}s", w.buildings.len());
    }

    // Occupancy creeps up.
    for b in w.buildings.values_mut() {
        if b.occupied < b.capacity && demo.rng.below(200) == 0 {
            b.occupied += 1;
        }
    }

    // Segment loads: vehicles on them plus a slowly moving background.
    let phase = |s: SegmentId| (s.data().as_ffi() % 997) as f32 * 0.37;
    for (s, seg) in w.roads.segments.iter_mut() {
        let bg = 0.5 + 0.5 * (t * 0.05 + phase(s)).sin();
        let bg = bg * bg * 0.95;
        seg.load = [bg * seg.capacity[0], (1.0 - bg) * 0.7 * seg.capacity[1]];
    }
    // Vehicles drive along the graph, turning randomly at nodes.
    let roads = &mut w.roads;
    for v in w.vehicles.values_mut() {
        let Some(mut s) = v.segment.filter(|s| roads.segments.contains_key(*s)) else {
            v.segment = roads.segments.keys().next();
            v.progress = 0.0;
            continue;
        };
        let seg = &roads.segments[s];
        let speed = v.speed * (1.0 - 0.7 * seg.congestion_in(v.forward));
        v.progress += speed * dt;
        let mut len = seg.length;
        while v.progress >= len {
            v.progress -= len;
            let seg = &roads.segments[s];
            let node = if v.forward { seg.b } else { seg.a };
            let options: Vec<SegmentId> = roads.nodes[node].segments.iter().copied().filter(|&x| x != s).collect();
            let next = if options.is_empty() {
                s
            } else {
                options[demo.rng.below(options.len() as u32) as usize]
            };
            s = next;
            v.forward = roads.segments[s].a == node;
            len = roads.segments[s].length;
        }
        v.segment = Some(s);
        let seg = &roads.segments[s];
        let (pa, pb) = (roads.nodes[seg.a].pos, roads.nodes[seg.b].pos);
        let (from, to) = if v.forward { (pa, pb) } else { (pb, pa) };
        let f = (v.progress / len).clamp(0.0, 1.0);
        v.pos = [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f];
        v.heading = (to[1] - from[1]).atan2(to[0] - from[0]);
    }
    for v in w.vehicles.values() {
        if let Some(s) = v.segment
            && let Some(seg) = roads.segments.get_mut(s)
        {
            seg.load[if v.forward { 0 } else { 1 }] += 1.0;
        }
    }

    // Demand, stats, money.
    w.demand.residential = 0.7 * (t * 0.05).sin();
    w.demand.commercial = 0.5 * (t * 0.07 + 1.0).sin();
    w.demand.industrial = 0.6 * (t * 0.04 + 2.0).sin();
    let (mut pop, mut jobs) = (0u32, 0u32);
    for b in w.buildings.values() {
        match b.kind {
            Zone::Residential => pop += b.occupied,
            _ => jobs += b.capacity,
        }
    }
    w.stats.population = pop;
    w.stats.jobs = jobs;
    w.stats.employed = pop.min(jobs) * 9 / 10;
    w.stats.households = pop / 3;
    let income = (pop as f32 * w.economy.tax_rate * 20.0) as i64;
    let expenses = w.stats.road_cells as i64;
    w.economy.last_day_income = income;
    w.economy.last_day_expenses = expenses;
    demo.money_acc += (income - expenses) as f64 / citysim_sim::balance::TICKS_PER_DAY as f64;
    let whole = demo.money_acc.trunc();
    w.economy.money += whole as i64;
    demo.money_acc -= whole;
}
