//! Putting a demolished building back (the interface's undo).
//!
//! Before the interface sends a command that demolishes buildings it takes
//! a [`BuildingRecord`] of each ([`World::building_record`]): the building as
//! it stood, the cells of its lot, its households with their members, and
//! the people who worked there. Undo sends [`crate::Command::RestoreBuilding`]
//! with the record. The record lives only in the interface's undo list
//! (nothing of it is saved).
//!
//! A restore is refused ([`RestoreProblem`]) when the lot is no longer what
//! it was: the land under it changed shape (the road beside it was redrawn
//! differently), another building stands there, the cells are zoned for
//! something else or not at all, or no road connects it any more. Otherwise
//! the same building (kind, level, footprint, facing, seed, age) comes back
//! under a new id; its households come back with their members (residents of
//! a demolished home left the city, so nobody else has them); members whose
//! old workplace still stands with room get their job back, the rest look
//! for work; former staff who are still in the city and still without a job
//! are taken on again. The demolition cost of the building is refunded.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::citizens::{Activity, Citizen, CitizenId, Location, Shift};
use crate::grid::Zone;
use crate::households::Household;
use crate::zoning::CellRef;
use crate::{Building, BuildingId, Footprint, World, balance, geom};

/// How far (metres) a lot cell's centre may be from where it was and still
/// count as the same land.
const SAME_LAND_M: f32 = 0.3;

/// Everything needed to put a building back as it was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildingRecord {
    /// The building as it stood (`occupied` is recounted on restore).
    pub building: Building,
    /// Its lot's cells with their centres (sim metres), to find the same
    /// land again if the lattice was made anew.
    pub cells: Vec<(CellRef, [f32; 2])>,
    /// Households living there, with their members.
    pub households: Vec<HouseholdRecord>,
    /// People who worked there.
    pub workers: Vec<WorkerRecord>,
}

/// A household of a demolished home.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HouseholdRecord {
    pub household: Household,
    pub members: Vec<Citizen>,
}

/// A worker of a demolished workplace.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorkerRecord {
    pub citizen: CitizenId,
    /// The citizen's stable seed (ids are generational, this is a second
    /// check that it is the same person).
    pub seed: u32,
    pub shift: Shift,
}

impl BuildingRecord {
    /// Residents of the building when it was demolished.
    pub fn residents(&self) -> usize {
        self.households.iter().map(|h| h.members.len()).sum()
    }

    /// Where the building stood (sim metres).
    pub fn center(&self) -> [f32; 2] {
        self.building.center()
    }
}

/// Why a building cannot be put back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestoreProblem {
    /// The zoning land under the lot is gone or has another shape.
    LandChanged,
    /// Another building stands on the lot.
    BuiltOn,
    /// The lot is zoned for something else, or not zoned.
    Rezoned,
    /// No road connects the lot to the highway (or none runs in front).
    NoRoad,
}

impl fmt::Display for RestoreProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RestoreProblem::LandChanged => "the land there has changed",
            RestoreProblem::BuiltOn => "another building stands there now",
            RestoreProblem::Rezoned => "the lot is no longer zoned for it",
            RestoreProblem::NoRoad => "no road reaches the lot any more",
        })
    }
}

impl World {
    /// A record of building `id` as it is now, to put it back after it is
    /// demolished (`Command::RestoreBuilding`).
    pub fn building_record(&self, id: BuildingId) -> Option<BuildingRecord> {
        let b = self.buildings.get(id)?;
        let cells = b
            .footprint
            .cells()
            .filter_map(|c| Some((c, self.zoning.cell_center(c)?)))
            .collect();
        let place = self.people.places.get(id);
        let households = place
            .map(|p| p.households.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter_map(|&hid| {
                let h = self.households.get(hid)?;
                Some(HouseholdRecord {
                    household: h.clone(),
                    members: h
                        .citizens
                        .iter()
                        .filter_map(|&c| self.citizens.get(c).cloned())
                        .collect(),
                })
            })
            .collect();
        let workers = place
            .map(|p| p.workers.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter_map(|&cid| {
                let c = self.citizens.get(cid)?;
                Some(WorkerRecord {
                    citizen: cid,
                    seed: c.seed,
                    shift: c.job?.shift,
                })
            })
            .collect();
        Some(BuildingRecord {
            building: b.clone(),
            cells,
            households,
            workers,
        })
    }
}

/// Money given back by a restore: what demolishing the building cost.
pub(crate) fn refund(rec: &BuildingRecord) -> i64 {
    rec.building.footprint.area() as i64 * balance::BULLDOZE_COST_PER_CELL
}

/// The footprint the building would stand on now, or why it cannot.
pub(crate) fn plan(world: &World, rec: &BuildingRecord) -> Result<Footprint, RestoreProblem> {
    let b = &rec.building;
    let fp = b.footprint;
    if rec.cells.is_empty() || rec.cells.len() != fp.area() as usize {
        return Err(RestoreProblem::LandChanged);
    }
    let same = |c: CellRef, at: [f32; 2]| {
        world
            .zoning
            .cell_center(c)
            .is_some_and(|p| geom::dist(p, at) < SAME_LAND_M)
            && world.zoning.cell(c).is_some()
    };
    let fp = if rec.cells.iter().all(|&(c, at)| same(c, at)) {
        fp
    } else {
        // The lattice was made again (the road was removed and rebuilt):
        // the same land under other cell names.
        let mut now: Vec<CellRef> = Vec::with_capacity(rec.cells.len());
        for &(_, at) in &rec.cells {
            let c = world.zoning.cell_at(at).ok_or(RestoreProblem::LandChanged)?;
            if !same(c, at) {
                return Err(RestoreProblem::LandChanged);
            }
            now.push(c);
        }
        let lattice = now[0].lattice;
        if now.iter().any(|c| c.lattice != lattice) {
            return Err(RestoreProblem::LandChanged);
        }
        let lo = [
            now.iter().map(|c| c.i).min().unwrap_or(0),
            now.iter().map(|c| c.j).min().unwrap_or(0),
        ];
        let hi = [
            now.iter().map(|c| c.i).max().unwrap_or(0),
            now.iter().map(|c| c.j).max().unwrap_or(0),
        ];
        let l = world.zoning.lattice(lattice).ok_or(RestoreProblem::LandChanged)?;
        let nf = crate::growth::placed(world, Footprint::new(lattice, l, lo, hi, b.facing));
        if nf.area() != fp.area() {
            return Err(RestoreProblem::LandChanged);
        }
        nf
    };
    let mut problem = None;
    for c in fp.cells() {
        let Some(x) = world.zoning.cell(c) else {
            return Err(RestoreProblem::LandChanged);
        };
        if x.building.is_some() {
            return Err(RestoreProblem::BuiltOn);
        }
        if !x.usable() {
            problem = problem.or(Some(RestoreProblem::LandChanged));
        } else if x.zone != Some(b.kind) {
            problem = problem.or(Some(RestoreProblem::Rezoned));
        }
    }
    if let Some(p) = problem {
        return Err(p);
    }
    if !crate::growth::has_frontage(world, &fp, b.facing) || !connected(world, &fp) {
        return Err(RestoreProblem::NoRoad);
    }
    Ok(fp)
}

/// Is a cell of `fp` in a block whose road is connected to the highway
/// entry? (Worked out from the roads, not the lazily built index, which a
/// `&World` cannot bring up to date.)
fn connected(world: &World, fp: &Footprint) -> bool {
    let roads = &world.roads;
    let Some(entry) = roads.entry.filter(|&e| roads.nodes.contains_key(e)) else {
        return false;
    };
    let segs: Vec<crate::SegmentId> = world
        .zoning
        .blocks
        .iter()
        .filter(|bl| fp.cells().any(|c| bl.contains(c)))
        .map(|bl| bl.segment)
        .filter(|&s| roads.segments.contains_key(s))
        .collect();
    if segs.is_empty() {
        return false;
    }
    let mut seen = std::collections::BTreeSet::from([entry]);
    let mut stack = vec![entry];
    while let Some(n) = stack.pop() {
        for &s in &roads.nodes[n].segments {
            if segs.contains(&s) {
                return true;
            }
            let Some(seg) = roads.segments.get(s) else { continue };
            let other = if seg.a == n { seg.b } else { seg.a };
            if seen.insert(other) {
                stack.push(other);
            }
        }
    }
    false
}

/// Put the building back on `fp` (from [`plan`]). Returns its new id.
pub(crate) fn execute(world: &mut World, rec: &BuildingRecord, fp: Footprint) -> BuildingId {
    let id = world.buildings.insert(Building {
        footprint: fp,
        occupied: 0,
        ..rec.building.clone()
    });
    world.growth.meta.insert(id, Default::default());
    world.versions.buildings += 1;
    crate::growth::place_cells(world, id);
    world.versions.grid += 1;
    if rec.building.kind == Zone::Commercial {
        crate::errands::stock_up(world, id);
    }
    world.economy.money += refund(rec);
    let now = world.time.tick;
    let capacity = rec.building.capacity;
    for hr in &rec.households {
        let mut h = hr.household.clone();
        h.home = id;
        h.citizens.clear();
        h.members = 0;
        h.job = None;
        h.vehicle = None;
        h.shopper = None;
        h.state = Default::default();
        let hid = crate::households::insert(world, h);
        for m in &hr.members {
            if world.buildings[id].occupied >= capacity {
                break;
            }
            let old_job = m.job;
            let c = Citizen {
                household: hid,
                job: None,
                activity: Activity::Home,
                location: Location::In(id),
                until_tick: now,
                has_car: false,
                ..m.clone()
            };
            let cid = crate::households::add_member(world, c);
            if let Some(j) = old_job
                && world
                    .buildings
                    .get(j.workplace)
                    .is_some_and(|w| w.occupied < w.capacity && w.kind != Zone::Residential)
            {
                crate::people::hire(world, cid, j.workplace, j.shift);
                crate::people::update(world, cid, |c| {
                    if let Some(now) = c.job.as_mut() {
                        now.since_day = j.since_day;
                    }
                });
            }
        }
        if world.households[hid].citizens.is_empty() {
            world.households.remove(hid);
            if let Some(p) = world.people.places.get_mut(id) {
                p.households.retain(|&x| x != hid);
            }
            world.people.known_households = world.households.len();
        } else {
            crate::people::refresh_household(world, hid);
        }
    }
    if rec.building.kind != Zone::Residential {
        for w in &rec.workers {
            let free = world.buildings[id].occupied < capacity;
            if free
                && world
                    .citizens
                    .get(w.citizen)
                    .is_some_and(|c| c.seed == w.seed && c.job_seeker())
            {
                crate::people::hire(world, w.citizen, id, w.shift);
            }
        }
    }
    id
}
