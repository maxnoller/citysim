//! The [`World`]: all simulation state, plus the command/step entry points.

use crate::IdMap;
use serde::{Deserialize, Serialize};

use crate::command::{self, Command, CommandError, Outcome, Preview};
use crate::derived::Derived;
use crate::geom::Cell;
use crate::grid::Occupant;
use crate::growth::GrowthState;
use crate::roads::RoadKind;
use crate::traffic::TrafficState;
use crate::{
    Building, BuildingId, Demand, Economy, Grid, Household, HouseholdId, MAP_CELLS, Rng, RoadNetwork, SimTime, Stats,
    Vehicle, VehicleId, balance, demand, economy, growth, households, save, stats, traffic,
};

/// Change counters. Each is bumped whenever that part of the state changes,
/// so the renderer can rebuild meshes only when needed. They start at 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Versions {
    /// Road graph (nodes / segments) changed.
    pub roads: u64,
    /// Any cell's zone or occupant changed.
    pub grid: u64,
    /// Buildings added, removed, or a drawn field changed (kind, footprint,
    /// level, facing, seed). Occupancy changes do not bump it.
    pub buildings: u64,
}

impl Default for Versions {
    fn default() -> Self {
        Versions {
            roads: 1,
            grid: 1,
            buildings: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct World {
    pub seed: u64,
    /// The only source of randomness in the sim.
    pub rng: Rng,
    pub time: SimTime,
    pub roads: RoadNetwork,
    pub grid: Grid,
    pub buildings: IdMap<BuildingId, Building>,
    pub households: IdMap<HouseholdId, Household>,
    pub vehicles: IdMap<VehicleId, Vehicle>,
    pub economy: Economy,
    pub demand: Demand,
    pub stats: Stats,
    pub versions: Versions,
    /// Trips, junction state, re-plan queue (everything traffic needs
    /// besides `vehicles`).
    pub traffic: TrafficState,
    /// Growth accumulators and per-building bookkeeping.
    pub growth: GrowthState,
    /// Lazily rebuilt indices derived from the state above. Not saved,
    /// ignored by `==`.
    #[serde(skip)]
    pub(crate) cache: Derived,
}

impl World {
    /// Empty map with a short highway stub entering at the middle of the
    /// west edge (cells `(0, 128)..=(11, 128)`); its outer node is
    /// `roads.entry`.
    pub fn new(seed: u64) -> World {
        let mut world = World {
            seed,
            rng: Rng::new(seed),
            time: SimTime::default(),
            roads: RoadNetwork::new(),
            grid: Grid::new(),
            buildings: IdMap::new(),
            households: IdMap::new(),
            vehicles: IdMap::new(),
            economy: Economy::default(),
            demand: Demand::default(),
            stats: Stats::default(),
            versions: Versions::default(),
            traffic: TrafficState::default(),
            growth: GrowthState::default(),
            cache: Derived::default(),
        };
        let start = World::highway_entry_cell();
        let end = start.offset(balance::HIGHWAY_STUB_CELLS - 1, 0);
        let a = world.roads.add_node(start.center());
        let b = world.roads.add_node(end.center());
        world.roads.add_segment(a, b, RoadKind::Highway);
        world.roads.entry = Some(a);
        for c in start.straight_line_to(end).expect("horizontal") {
            world.grid.get_mut(c).expect("in bounds").occupant = Occupant::Road;
        }
        traffic::sync_junctions(&mut world);
        stats::update(&mut world);
        demand::refresh(&mut world);
        world
    }

    /// Map-edge cell where the highway stub starts.
    pub fn highway_entry_cell() -> Cell {
        Cell::new(0, MAP_CELLS / 2)
    }

    /// Validate `cmd` and report its cost and affected cells, without
    /// changing anything. Ok exactly when `apply` would succeed.
    pub fn preview(&self, cmd: &Command) -> Result<Preview, CommandError> {
        command::plan(self, cmd).map(|p| p.preview())
    }

    /// Validate and execute `cmd`. On error nothing changes.
    pub fn apply(&mut self, cmd: Command) -> Result<Outcome, CommandError> {
        let plan = command::plan(self, &cmd)?;
        let outcome = command::execute(self, plan);
        let mut ended = Vec::new();
        traffic::sync_network(self, &mut ended);
        for e in ended {
            households::trip_ended(self, e);
        }
        stats::update(self);
        if self.time.tick == 0 {
            // The clock never ran (a new game, paused from the start):
            // demand follows every command at once.
            demand::refresh(self);
        }
        Ok(outcome)
    }

    /// Advance exactly one fixed tick (1 / TICKS_PER_SECOND of sim time).
    pub fn step(&mut self) {
        let new_day = self.time.advance();
        demand::tick(self);
        growth::tick(self);
        households::tick(self);
        crate::errands::tick(self);
        traffic::tick(self);
        if new_day {
            economy::end_of_day(self);
        }
        stats::update(self);
    }

    pub fn save(&self) -> Vec<u8> {
        save::save(self)
    }

    pub fn load(bytes: &[u8]) -> Result<World, save::LoadError> {
        save::load(bytes)
    }

    /// Check the simulation's own invariants (cheap enough for big worlds):
    /// households point at existing buildings, building occupancy equals
    /// what the households say, vehicles sit on existing segments within
    /// their length, every car of a household is known to it.
    pub fn check_sim_invariants(&self) -> Result<(), String> {
        let mut occ: std::collections::BTreeMap<BuildingId, u32> = Default::default();
        for (hid, h) in self.households.iter() {
            if !self.buildings.contains_key(h.home) {
                return Err(format!("{hid:?} has missing home"));
            }
            *occ.entry(h.home).or_default() += h.members as u32;
            if let Some(j) = h.job {
                if !self.buildings.contains_key(j) {
                    return Err(format!("{hid:?} has missing job"));
                }
                *occ.entry(j).or_default() += 1;
            }
            if let Some(v) = h.vehicle
                && self.vehicles.get(v).and_then(|v| v.household) != Some(hid)
            {
                return Err(format!("{hid:?} vehicle {v:?} does not point back"));
            }
        }
        for (bid, b) in self.buildings.iter() {
            let want = occ.get(&bid).copied().unwrap_or(0);
            if b.occupied != want {
                return Err(format!("{bid:?} occupied {} but households say {want}", b.occupied));
            }
            if b.occupied > b.capacity {
                return Err(format!("{bid:?} over capacity"));
            }
        }
        for (vid, v) in self.vehicles.iter() {
            let Some(seg) = v.segment.and_then(|s| self.roads.segments.get(s)) else {
                return Err(format!("{vid:?} not on an existing segment"));
            };
            if !(-0.01..=seg.length + 0.01).contains(&v.progress) || !v.pos[0].is_finite() {
                return Err(format!("{vid:?} progress {} outside 0..{}", v.progress, seg.length));
            }
            if let Some(h) = v.household
                && self.households.get(h).and_then(|h| h.vehicle) != Some(vid)
            {
                return Err(format!("{vid:?} household {h:?} does not point back"));
            }
        }
        Ok(())
    }

    /// Check cross-structure invariants (road graph valid, grid road cells
    /// match the graph, building cells match buildings). For tests/debug.
    pub fn check_invariants(&self) -> Result<(), String> {
        self.roads.validate()?;
        for y in 0..MAP_CELLS {
            for x in 0..MAP_CELLS {
                let c = Cell::new(x, y);
                let cd = self.grid.get(c).expect("in bounds");
                let covered = self.roads.covers(c.center());
                match cd.occupant {
                    Occupant::Road if !covered => return Err(format!("{c:?} marked road but no road there")),
                    Occupant::Road => {}
                    _ if covered => return Err(format!("{c:?} has road but is not marked road")),
                    Occupant::Building(id) => match self.buildings.get(id) {
                        Some(b) if b.footprint.contains(c) => {}
                        _ => return Err(format!("{c:?} references bad building {id:?}")),
                    },
                    Occupant::Empty => {}
                }
                if cd.occupant == Occupant::Road && cd.zone.is_some() {
                    return Err(format!("{c:?} is road and zoned"));
                }
            }
        }
        Ok(())
    }
}
