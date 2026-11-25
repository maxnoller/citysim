//! Save / load.
//!
//! Format: `b"CSIM"`, format version (u32 LE), postcard body of the
//! [`World`], then optionally an extension block (postcard, starts with its
//! own version number).
//!
//! Version 6 (citizens, household money and goods, the population
//! bookkeeping; the 16-minute day, people bound to their cars, trips that
//! name their buildings, party and cargo) is written now. Version 5 was the
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
pub const FORMAT_VERSION: u32 = 6;
/// Versions between the oldest loadable and this one that do not load
/// (5: the unreleased first citizens layout).
const UNLOADABLE_VERSIONS: [u32; 1] = [5];
/// Oldest version that still loads.
pub const OLDEST_LOADABLE_VERSION: u32 = 3;
/// Version of the extension block after the body (see the module docs).
pub const EXTENSION_VERSION: u32 = 1;

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
    let ext = postcard::to_stdvec(&(EXTENSION_VERSION, ExtensionV1::of(world))).expect("serialisable");
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
    if !rest.is_empty() {
        let (ext_version, rest): (u32, &[u8]) =
            postcard::take_from_bytes(rest).map_err(|e| LoadError::Corrupt(format!("extension: {e}")))?;
        // A newer block than this build knows: keep the defaults.
        if ext_version == 1 {
            let ext: ExtensionV1 =
                postcard::from_bytes(rest).map_err(|e| LoadError::Corrupt(format!("extension: {e}")))?;
            ext.restore(&mut world);
        }
    }
    restore_derived(&mut world);
    world.roads.refresh_tables();
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    world.cache.ensure_zoning(&world.zoning);
    if version == 3 {
        crate::growth::convert_legacy_lots(&mut world);
        crate::traffic::sync_junctions(&mut world);
    }
    if version < 5 {
        upgrade_people(&mut world);
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
}
