//! Save / load.
//!
//! Format: `b"CSIM"`, format version (u32 LE), postcard body of the
//! [`World`], then optionally an extension block (postcard, starts with its
//! own version number).
//!
//! Version 7 (zoning on strips bent along curved roads, blocks six rows
//! deep: the body is laid out as version 6, the bent lattices travel in
//! extension block 4; block 5 adds the traffic losses of every booked day)
//! is written now. Version 6 (citizens, household money
//! and goods, the population bookkeeping; the 16-minute day, people bound
//! to their cars, trips that name their buildings, party and cargo) loads
//! with the same body; its zoning blocks are made again the current way
//! (`blocks::renew_all`). Version 5 was the
//! unreleased first citizens layout and does not load. Older saves still
//! load (their clock keeps its time of day on the longer day):
//!
//! * Version 4 (round 2: avenues, lanes, junction control, fiscal state)
//!   decodes into the frozen round-2 types in [`v4`].
//! * Version 3 (round 1) decodes into the frozen round-1 types in [`v3`] and
//!   is upgraded (vehicles on lane 0, junctions on `Auto`, second-row
//!   buildings merged into the lot in front where possible).
//!
//! Households from both get plausible citizens, money, goods and cars
//! generated on load (`households::adopt`): the member count, home and job
//! are kept (the job goes to the first adult), a car on the road gets its
//! household's worker as driver, shops start with full shelves.
//!
//! The extension block carries state added after format version 3 without
//! breaking it: those fields are `#[serde(skip)]` in the World's types, so
//! the body keeps exactly the version-3 layout. Saves written before the
//! block existed load with defaults for it, and older builds read newer
//! saves too (postcard ignores trailing bytes).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Budget, BuildingId, MAP_CELLS, World, balance};

pub mod v3;
pub mod v4;
pub mod v5;

pub const MAGIC: &[u8; 4] = b"CSIM";
/// Bump when the World layout changes incompatibly.
pub const FORMAT_VERSION: u32 = 7;
/// Versions between the oldest loadable and this one that do not load
/// (5: the unreleased first citizens layout).
const UNLOADABLE_VERSIONS: [u32; 1] = [5];
/// Oldest version that still loads.
pub const OLDEST_LOADABLE_VERSION: u32 = 3;
/// Version of the extension block after the body (see the module docs).
pub const EXTENSION_VERSION: u32 = 6;

/// Every field of a [`Budget`], including those its own serde skips.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct BudgetRecord {
    residential_tax: i64,
    commercial_tax: i64,
    industrial_tax: i64,
    road_upkeep: i64,
    services: i64,
}

impl From<Budget> for BudgetRecord {
    fn from(b: Budget) -> Self {
        BudgetRecord {
            residential_tax: b.residential_tax,
            commercial_tax: b.commercial_tax,
            industrial_tax: b.industrial_tax,
            road_upkeep: b.road_upkeep,
            services: b.services,
        }
    }
}

impl From<BudgetRecord> for Budget {
    fn from(b: BudgetRecord) -> Self {
        Budget {
            residential_tax: b.residential_tax,
            commercial_tax: b.commercial_tax,
            industrial_tax: b.industrial_tax,
            road_upkeep: b.road_upkeep,
            services: b.services,
        }
    }
}

/// Extension block, version 1. Append new fields only in a new version.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExtensionV1 {
    /// `Economy::last_day`.
    last_day: BudgetRecord,
    /// `DaySummary::budget` of every `Economy::history` entry, in order.
    day_budgets: Vec<BudgetRecord>,
    /// `BuildingMeta::unreachable_since` of every building that has one.
    unreachable_since: Vec<(BuildingId, u64)>,
}

impl ExtensionV1 {
    pub(crate) fn of(w: &World) -> ExtensionV1 {
        ExtensionV1 {
            last_day: w.economy.last_day.into(),
            day_budgets: w.economy.history.iter().map(|d| d.budget.into()).collect(),
            unreachable_since: w
                .buildings
                .keys()
                .filter_map(|id| Some((id, w.growth.meta.get(id)?.unreachable_since?)))
                .collect(),
        }
    }

    pub(crate) fn restore(self, w: &mut World) {
        w.economy.last_day = self.last_day.into();
        for (d, b) in w.economy.history.iter_mut().zip(self.day_budgets) {
            d.budget = b.into();
        }
        for (id, t) in self.unreachable_since {
            if w.buildings.contains_key(id) {
                w.growth.meta.entry(id).unreachable_since = Some(t);
            }
        }
    }
}

/// Extension block, version 2: version 1 plus the junctions' minor-road
/// counters (`JunctionState::minor_*`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExtensionV2 {
    v1: ExtensionV1,
    junction_minor: Vec<(crate::NodeId, [u16; 3])>,
}

impl ExtensionV2 {
    pub(crate) fn of(w: &World) -> ExtensionV2 {
        ExtensionV2 {
            v1: ExtensionV1::of(w),
            junction_minor: w
                .traffic
                .junction_states
                .iter()
                .filter(|(_, j)| j.minor_minute > 0 || j.minor_peak_today > 0 || j.minor_peak_yesterday > 0)
                .map(|(n, j)| (n, [j.minor_minute, j.minor_peak_today, j.minor_peak_yesterday]))
                .collect(),
        }
    }

    pub(crate) fn restore(self, w: &mut World) {
        self.v1.restore(w);
        for (n, [m, t, y]) in self.junction_minor {
            if let Some(j) = w.traffic.junction_states.get_mut(n) {
                j.minor_minute = m;
                j.minor_peak_today = t;
                j.minor_peak_yesterday = y;
            }
        }
    }
}

/// Extension block, version 3: version 2 plus the traffic losses
/// (`Economy::traffic_today`, `last_day_traffic`), the journey times and
/// the households that gave up on their commute (`People::trip_times_*`,
/// `commute_moves_*`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExtensionV3 {
    v2: ExtensionV2,
    traffic_today: crate::economy::TrafficLoss,
    last_day_traffic: crate::economy::TrafficLoss,
    trip_times: [crate::people::TripTimes; 2],
    commute_moves: [crate::people::CommuteMoves; 2],
}

impl ExtensionV3 {
    pub(crate) fn of(w: &World) -> ExtensionV3 {
        ExtensionV3 {
            v2: ExtensionV2::of(w),
            traffic_today: w.economy.traffic_today,
            last_day_traffic: w.economy.last_day_traffic,
            trip_times: [w.people.trip_times_today, w.people.trip_times_yesterday],
            commute_moves: [w.people.commute_moves_today, w.people.commute_moves_yesterday],
        }
    }

    pub(crate) fn restore(self, w: &mut World) {
        self.v2.restore(w);
        w.economy.traffic_today = self.traffic_today;
        w.economy.last_day_traffic = self.last_day_traffic;
        [w.people.trip_times_today, w.people.trip_times_yesterday] = self.trip_times;
        [w.people.commute_moves_today, w.people.commute_moves_yesterday] = self.commute_moves;
    }
}

/// Extension block, version 4: version 3 plus the bent lattices of
/// curved roads (`Lattice::strip`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExtensionV4 {
    v3: ExtensionV3,
    strips: Vec<(u16, crate::zoning::Strip)>,
}

impl ExtensionV4 {
    pub(crate) fn of(w: &World) -> ExtensionV4 {
        ExtensionV4 {
            v3: ExtensionV3::of(w),
            strips: w
                .zoning
                .lattices()
                .filter_map(|(id, l)| Some((id.0, l.strip.clone()?)))
                .collect(),
        }
    }

    pub(crate) fn restore(self, w: &mut World) {
        self.v3.restore(w);
        for (id, st) in self.strips {
            if let Some(Some(l)) = w.zoning.lattices.get_mut(id as usize) {
                l.strip = Some(st);
            }
        }
    }
}

/// Extension block, version 5: version 4 plus what traffic cost the city
/// on every booked day (`DaySummary::traffic` of each `Economy::history`
/// entry, in order) and the money credited ahead of today's booking
/// (`Economy::advanced_today`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExtensionV5 {
    v4: ExtensionV4,
    day_traffic: Vec<Option<crate::economy::TrafficLoss>>,
    /// `Economy::advanced_today`.
    advanced_today: i64,
}

impl ExtensionV5 {
    pub(crate) fn of(w: &World) -> ExtensionV5 {
        ExtensionV5 {
            v4: ExtensionV4::of(w),
            day_traffic: w.economy.history.iter().map(|d| d.traffic).collect(),
            advanced_today: w.economy.advanced_today,
        }
    }

    pub(crate) fn restore(self, w: &mut World) {
        self.v4.restore(w);
        for (d, t) in w.economy.history.iter_mut().zip(self.day_traffic) {
            d.traffic = t;
        }
        w.economy.advanced_today = self.advanced_today;
    }
}

/// Extension block, version 6: version 5 plus people on foot (the walkers
/// under way, the crossings' turn-taking timers by crossing, the day's
/// walking counts).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ExtensionV6 {
    v5: ExtensionV5,
    walkers: Vec<(crate::CitizenId, crate::walk::Walker)>,
    timers: Vec<(crate::walk::CrossingKey, crate::walk::CrossTimers)>,
    stats: crate::walk::WalkStats,
    /// `walk::Walking::routes_for`.
    routes_for: u64,
}

impl ExtensionV6 {
    pub(crate) fn of(w: &World) -> ExtensionV6 {
        let net = &w.walking.net;
        ExtensionV6 {
            v5: ExtensionV5::of(w),
            walkers: w.walking.walkers.iter().map(|(k, v)| (*k, v.clone())).collect(),
            timers: net
                .crossings
                .iter()
                .zip(&w.walking.timers)
                .filter(|(_, t)| **t != crate::walk::CrossTimers::default())
                .map(|(c, t)| (c.key, *t))
                .collect(),
            stats: w.walking.stats.clone(),
            routes_for: w.walking.routes_for,
        }
    }
}

/// Blocks before version 5 kept the traffic loss of the last booked day
/// only (`Economy::last_day_traffic`): it goes to that day, the days
/// before stay empty.
fn restore_last_day_traffic(w: &mut World) {
    let t = w.economy.last_day_traffic;
    if let Some(d) = w.economy.history.last_mut()
        && t != crate::economy::TrafficLoss::default()
    {
        d.traffic = Some(t);
    }
}

/// Derived fields that are not stored at all, recomputed exactly as
/// `stats::update` computes them.
fn restore_derived(w: &mut World) {
    let (_, _, workforce) = crate::stats::labour(w);
    let s = &mut w.stats;
    s.workforce = workforce;
    s.vacant_jobs = s.jobs.saturating_sub(s.employed);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// Not a CitySim save.
    BadMagic,
    UnsupportedVersion(u32),
    Corrupt(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::BadMagic => write!(f, "not a CitySim save file"),
            LoadError::UnsupportedVersion(v) => write!(f, "unsupported save version {v}"),
            LoadError::Corrupt(e) => write!(f, "corrupt save: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

pub(crate) fn save(world: &World) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 * 1024);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    let body = postcard::to_stdvec(world).expect("World is always serialisable");
    out.extend_from_slice(&body);
    let ext = postcard::to_stdvec(&(EXTENSION_VERSION, ExtensionV6::of(world))).expect("serialisable");
    out.extend_from_slice(&ext);
    out
}

pub(crate) fn load(bytes: &[u8]) -> Result<World, LoadError> {
    if bytes.len() < 8 || &bytes[..4] != MAGIC {
        return Err(LoadError::BadMagic);
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
    if !(OLDEST_LOADABLE_VERSION..=FORMAT_VERSION).contains(&version) || UNLOADABLE_VERSIONS.contains(&version) {
        return Err(LoadError::UnsupportedVersion(version));
    }
    let corrupt = |e: postcard::Error| LoadError::Corrupt(e.to_string());
    let (mut world, rest): (World, &[u8]) = match version {
        3 => {
            let (old, rest): (v3::WorldV3, &[u8]) = postcard::take_from_bytes(&bytes[8..]).map_err(corrupt)?;
            (old.upgrade(), rest)
        }
        4 => {
            let (old, rest): (v4::WorldV4, &[u8]) = postcard::take_from_bytes(&bytes[8..]).map_err(corrupt)?;
            (old.upgrade(), rest)
        }
        _ => postcard::take_from_bytes(&bytes[8..]).map_err(corrupt)?,
    };
    check_loaded(&world).map_err(LoadError::Corrupt)?;
    if version >= 5 {
        check_people(&world).map_err(LoadError::Corrupt)?;
    }
    let mut walking: Option<ExtensionV6> = None;
    if !rest.is_empty() {
        let (ext_version, rest): (u32, &[u8]) =
            postcard::take_from_bytes(rest).map_err(|e| LoadError::Corrupt(format!("extension: {e}")))?;
        // A newer block than this build knows: keep the defaults.
        let bad = |e: postcard::Error| LoadError::Corrupt(format!("extension: {e}"));
        match ext_version {
            1 => postcard::from_bytes::<ExtensionV1>(rest)
                .map_err(bad)?
                .restore(&mut world),
            2 => postcard::from_bytes::<ExtensionV2>(rest)
                .map_err(bad)?
                .restore(&mut world),
            3 => {
                postcard::from_bytes::<ExtensionV3>(rest)
                    .map_err(bad)?
                    .restore(&mut world);
                restore_last_day_traffic(&mut world);
            }
            4 => {
                postcard::from_bytes::<ExtensionV4>(rest)
                    .map_err(bad)?
                    .restore(&mut world);
                restore_last_day_traffic(&mut world);
            }
            5 => postcard::from_bytes::<ExtensionV5>(rest)
                .map_err(bad)?
                .restore(&mut world),
            6 => {
                let mut e = postcard::from_bytes::<ExtensionV6>(rest).map_err(bad)?;
                std::mem::take(&mut e.v5).restore(&mut world);
                walking = Some(e);
            }
            _ => {}
        }
    }
    restore_derived(&mut world);
    world.roads.refresh_tables();
    // Junctions saved before turn pockets: today's geometry.
    crate::traffic::upgrade_junction_geometry(&mut world);
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    world.cache.ensure_zoning(&world.zoning);
    // Block grids are not stored: found again, and made where a save from
    // before them has per-road lattices.
    if version >= 7 && crate::blocks::derive_faces(&mut world) > 0 {
        world.cache.ensure_zoning(&world.zoning);
    }
    if version == 6 {
        // Zoning blocks of the version-6 kind (two rows, straight blocks
        // along curves): made again the current way; zones move onto the
        // new cells and buildings onto the new lots where they fit.
        crate::blocks::renew_all(&mut world);
        world.cache.ensure_zoning(&world.zoning);
    }
    if version == 3 {
        crate::growth::convert_legacy_lots(&mut world);
        crate::traffic::sync_junctions(&mut world);
    }
    if version < 5 {
        upgrade_people(&mut world);
    }
    // People on foot: the network for these roads, then the walkers under
    // way (older saves: their walks stay timed off screen).
    if let Some(e) = walking {
        crate::walk::restore(&mut world, e.walkers, e.timers, e.stats, e.routes_for);
    }
    Ok(world)
}

/// Older saves: give their households citizens, stock the shops, recompute
/// the statistics.
fn upgrade_people(w: &mut World) {
    // Households whose home is gone cannot be adopted.
    let homeless: Vec<crate::HouseholdId> = w
        .households
        .iter()
        .filter(|(_, h)| !w.buildings.contains_key(h.home))
        .map(|(id, _)| id)
        .collect();
    for h in homeless {
        w.households.remove(h);
    }
    for v in w.vehicles.values_mut() {
        if v.household.is_some_and(|h| !w.households.contains_key(h)) {
            v.household = None;
        }
    }
    for d in w.traffic.pending.iter_mut() {
        if d.household.is_some_and(|h| !w.households.contains_key(h)) {
            d.household = None;
        }
    }
    // Occupancy as the old households say (a damaged save may disagree).
    for b in w.buildings.values_mut() {
        b.occupied = 0;
    }
    let mut over = Vec::new();
    for (hid, h) in w.households.iter() {
        let b = &mut w.buildings[h.home];
        if b.occupied + h.members as u32 > b.capacity || h.members == 0 {
            over.push(hid);
            continue;
        }
        b.occupied += h.members as u32;
        if let Some(j) = h.job
            && let Some(jb) = w.buildings.get_mut(j)
            && jb.kind != crate::Zone::Residential
            && jb.occupied < jb.capacity
        {
            jb.occupied += 1;
        }
    }
    for hid in over {
        w.households.remove(hid);
    }
    for h in w.households.values_mut() {
        if h.job
            .is_some_and(|j| w.buildings.get(j).is_none_or(|b| b.kind == crate::Zone::Residential))
        {
            h.job = None;
        }
    }
    let shops: Vec<BuildingId> = w.buildings.keys().collect();
    for b in shops {
        crate::errands::stock_up(w, b);
    }
    w.cache.ensure_roads(&w.roads, w.versions.roads);
    w.cache.ensure_zoning(&w.zoning);
    crate::households::adopt(w);
    crate::inspect::refresh(w);
    crate::stats::update(w);
}

/// References between citizens, households and buildings in a version-5
/// save (a damaged one could index out of range later).
fn check_people(w: &World) -> Result<(), String> {
    for (id, c) in w.citizens.iter() {
        let Some(h) = w.households.get(c.household) else {
            return Err(format!("{id:?} has no household"));
        };
        if !h.citizens.contains(&id) {
            return Err(format!("{id:?} not in its household"));
        }
        if c.job.is_some_and(|j| !w.buildings.contains_key(j.workplace)) {
            return Err(format!("{id:?} works nowhere"));
        }
        if !c.leisure_need.is_finite() || !c.happiness.total.is_finite() {
            return Err(format!("{id:?} has bad numbers"));
        }
    }
    for (hid, h) in w.households.iter() {
        if !w.buildings.contains_key(h.home) {
            return Err(format!("{hid:?} has no home"));
        }
        for &c in &h.citizens {
            if w.citizens.get(c).is_none_or(|c| c.household != hid) {
                return Err(format!("{hid:?} lists a stranger"));
            }
        }
        if !h.goods.is_finite() {
            return Err(format!("{hid:?} has bad goods"));
        }
    }
    for p in w.people.places.values() {
        if !p.stock.is_finite() {
            return Err("bad stock".into());
        }
    }
    Ok(())
}

/// Checks on a decoded world for damage the simulation cannot survive
/// (index panics, a road graph pointing at missing nodes, negative or
/// non-finite road numbers that break path search), so a damaged file is
/// reported as corrupt instead of crashing or hanging the game later.
/// Deliberately not the full invariants: worlds put together by writing
/// public fields (tests, dev tooling) must still load.
fn check_loaded(w: &World) -> Result<(), String> {
    let cells = (MAP_CELLS * MAP_CELLS) as usize;
    let _ = cells;
    if w.zoning.lattice(crate::LatticeId::WORLD).is_none() {
        return Err("zoning has no world lattice".into());
    }
    for b in &w.zoning.blocks {
        if w.zoning.lattice(b.lattice).is_none() || !w.roads.segments.contains_key(b.segment) {
            return Err("a zone block points nowhere".into());
        }
    }
    for (c, x) in w.zoning.cells() {
        if x.building.is_some_and(|b| !w.buildings.contains_key(b)) {
            return Err(format!("{c:?} holds a missing building"));
        }
    }
    for (n, node) in w.roads.nodes.iter() {
        if !node.pos.iter().all(|x| x.is_finite()) {
            return Err(format!("{n:?} has a non-finite position"));
        }
    }
    for (s, seg) in w.roads.segments.iter() {
        let positive = |x: f32| x.is_finite() && x > 0.0;
        if !positive(seg.length) || !positive(seg.speed_limit) || !seg.capacity.into_iter().all(positive) {
            return Err(format!("{s:?} has a bad length, speed limit or capacity"));
        }
        if !seg.load.iter().all(|x| x.is_finite() && *x >= 0.0) {
            return Err(format!("{s:?} has a bad load"));
        }
    }
    w.roads.validate()?;
    let tax = w.economy.tax_rate;
    if !(tax.is_finite() && (0.0..=balance::MAX_TAX_RATE).contains(&tax)) {
        return Err(format!("tax rate {tax}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, Command};

    /// A save written before the extension block existed: just the body.
    fn body_only(w: &World) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&postcard::to_stdvec(w).unwrap());
        out
    }

    fn town() -> World {
        let mut w = World::new(5);
        let cmd = Command::BuildRoad {
            from: Cell::new(11, 128),
            to: Cell::new(30, 128),
            kind: crate::RoadKind::Street,
        };
        w.apply(cmd).unwrap();
        w.apply(Command::SetZone {
            min: Cell::new(12, 126),
            max: Cell::new(30, 127),
            zone: Some(crate::Zone::Residential),
        })
        .unwrap();
        for _ in 0..balance::TICKS_PER_DAY + 10 {
            w.step();
        }
        w
    }

    #[test]
    fn saves_without_the_extension_block_still_load() {
        let w = town();
        assert!(w.economy.last_day.services > 0);
        let loaded = load(&body_only(&w)).unwrap();
        // Everything but the extension fields comes back.
        assert_eq!(loaded.economy.last_day.services, 0);
        assert_eq!(loaded.economy.history[0].budget, Budget::default());
        assert_eq!(loaded.stats, w.stats);
        assert_eq!(loaded.buildings, w.buildings);
        // And it keeps running.
        let mut loaded = loaded;
        for _ in 0..100 {
            loaded.step();
        }
        loaded.check_invariants().unwrap();
    }

    #[test]
    fn the_body_alone_is_a_version_3_save() {
        // What an older build reads: the body decodes on its own and the
        // extension block is just trailing bytes.
        let w = town();
        let bytes = save(&w);
        let body = body_only(&w);
        assert!(bytes.len() > body.len());
        assert_eq!(&bytes[..body.len()], &body[..]);
        let old: World = postcard::from_bytes(&bytes[8..]).unwrap();
        assert_eq!(old.buildings, w.buildings);
        assert_eq!(load(&bytes).unwrap(), w);
    }

    #[test]
    fn traffic_losses_are_kept_per_day() {
        let mut w = town();
        for _ in 0..balance::TICKS_PER_DAY {
            w.step();
        }
        assert_eq!(w.economy.history.len(), 2);
        assert!(w.economy.history.iter().all(|d| d.traffic.is_some()));
        assert_eq!(w.economy.history[1].traffic, Some(w.economy.last_day_traffic));
        let loaded = load(&save(&w)).unwrap();
        assert_eq!(loaded.economy.history, w.economy.history);
        // A block of version 4 kept yesterday's loss only: the last day
        // gets it, the days before stay empty.
        let mut old = body_only(&w);
        old.extend_from_slice(&postcard::to_stdvec(&(4u32, ExtensionV4::of(&w))).unwrap());
        let loaded = load(&old).unwrap();
        assert_eq!(loaded.economy.history[0].traffic, None);
        let last = loaded.economy.history[1].traffic;
        assert_eq!(
            last,
            (w.economy.last_day_traffic != Default::default()).then_some(w.economy.last_day_traffic)
        );
        assert_eq!(loaded.economy.history[1].budget, w.economy.history[1].budget);
    }
}
