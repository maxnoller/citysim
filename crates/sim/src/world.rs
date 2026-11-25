//! The [`World`]: all simulation state, plus the command/step entry points.

use crate::IdMap;
use serde::{Deserialize, Serialize};

use crate::citizens::{Citizen, CitizenId, Location};
use crate::command::{self, Command, CommandError, Outcome, Preview};
use crate::derived::Derived;
use crate::geom::Cell;
use crate::grid::Occupant;
use crate::growth::GrowthState;
use crate::people::People;
use crate::roads::RoadKind;
use crate::traffic::TrafficState;
use crate::{
    Building, BuildingId, Demand, Economy, Grid, Household, HouseholdId, MAP_CELLS, Rng, RoadNetwork, SimTime, Stats,
    Vehicle, VehicleId, Zone, balance, demand, economy, growth, households, save, stats, traffic,
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
    /// Every resident (round 3). Each belongs to one of `households`.
    pub citizens: IdMap<CitizenId, Citizen>,
    /// Population bookkeeping: who lives and works where, counters, daily
    /// trip and life-event counts, city statistics (`people.stats`).
    pub people: People,
    /// Zone blocks, lattices and cells (see [`crate::zoning`]).
    #[serde(skip)]
    pub zoning: crate::Zoning,
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
            citizens: IdMap::new(),
            people: People::default(),
            zoning: crate::Zoning::default(),
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
        crate::zoning::mirror_grid(&mut world);
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
        crate::zoning::mirror_grid(self);
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
        if new_day {
            // Payday and the day's budget, before the night's life events.
            economy::end_of_day(self);
        }
        households::tick(self);
        crate::errands::tick(self);
        traffic::tick(self);
        stats::update(self);
        crate::zoning::mirror_grid(self);
    }

    pub fn save(&self) -> Vec<u8> {
        save::save(self)
    }

    pub fn load(bytes: &[u8]) -> Result<World, save::LoadError> {
        save::load(bytes)
    }

    /// Check the simulation's own invariants (cheap enough for big worlds):
    /// households point at existing buildings and list their citizens, every
    /// citizen is in exactly one place, building occupancy equals what the
    /// citizens say, the population counters match a recount, household
    /// money matches its ledger, vehicles sit on existing segments within
    /// their length and point back at their household and driver.
    pub fn check_sim_invariants(&self) -> Result<(), String> {
        let mut occ: std::collections::BTreeMap<BuildingId, u32> = Default::default();
        for (hid, h) in self.households.iter() {
            if !self.buildings.contains_key(h.home) {
                return Err(format!("{hid:?} has missing home"));
            }
            if h.citizens.is_empty() {
                // Legacy household not adopted yet (counts as before).
                *occ.entry(h.home).or_default() += h.members as u32;
                if let Some(j) = h.job {
                    *occ.entry(j).or_default() += 1;
                }
                continue;
            }
            if h.members as usize != h.citizens.len() {
                return Err(format!(
                    "{hid:?} members {} but {} citizens",
                    h.members,
                    h.citizens.len()
                ));
            }
            *occ.entry(h.home).or_default() += h.citizens.len() as u32;
            for &c in &h.citizens {
                match self.citizens.get(c) {
                    Some(ci) if ci.household == hid => {}
                    _ => return Err(format!("{hid:?} lists {c:?} which is not its member")),
                }
                if let Some(j) = self.citizens[c].job {
                    *occ.entry(j.workplace).or_default() += 1;
                }
            }
            if h.money != h.ledger.net() {
                return Err(format!("{hid:?} money {} but ledger says {}", h.money, h.ledger.net()));
            }
            let want_job = h
                .citizens
                .iter()
                .find_map(|&c| self.citizens[c].job.map(|j| j.workplace));
            if h.job != want_job {
                return Err(format!("{hid:?} job {:?} but members say {want_job:?}", h.job));
            }
            if let Some(v) = h.vehicle
                && self.vehicles.get(v).and_then(|v| v.household) != Some(hid)
            {
                return Err(format!("{hid:?} vehicle {v:?} does not point back"));
            }
            let out = h.citizens.iter().filter(|&&c| self.citizens[c].has_car).count();
            if out > h.cars as usize {
                return Err(format!("{hid:?} has {out} cars out of {}", h.cars));
            }
        }
        if self.households.len() != self.people.known_households {
            return Err("households changed behind the population model".into());
        }
        crate::people::check(self)?;
        for (cid, c) in self.citizens.iter() {
            if let Location::Travelling(j) = &c.location
                && j.mode == crate::citizens::TravelMode::Passenger
                && j.arrive_tick.is_none()
            {
                let ok = j.driver.and_then(|d| self.citizens.get(d)).is_some_and(|d| {
                    d.household == c.household
                        && matches!(d.location, Location::Travelling(dj) if dj.arrive_tick.is_none())
                });
                if !ok {
                    return Err(format!("{cid:?} rides with a driver who is not on the road"));
                }
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
            if b.kind == Zone::Commercial
                && let Some(p) = self.people.places.get(bid)
                && !(p.stock.is_finite() && p.stock >= 0.0)
            {
                return Err(format!("{bid:?} stock {}", p.stock));
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
                && !self.households.contains_key(h)
            {
                return Err(format!("{vid:?} household {h:?} is gone"));
            }
            if let Some(c) = self.traffic.trips.get(vid).and_then(|t| t.citizen) {
                let Some(ci) = self.citizens.get(c) else {
                    return Err(format!("{vid:?} driver {c:?} is gone"));
                };
                match &ci.location {
                    Location::Travelling(j) if j.vehicle == Some(vid) => {}
                    _ => return Err(format!("{vid:?} driver {c:?} is not in it")),
                }
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
                        Some(b) if b.footprint.rect().contains(c) => {}
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
