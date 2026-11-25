//! Player commands: validation, cost, preview and execution.
//!
//! Every command goes through [`plan`], which validates against the current
//! state without mutating it. `World::preview` returns the plan's summary;
//! `World::apply` executes the plan. So `preview(c).is_ok()` exactly when
//! `apply(c)` would succeed, and both report the same cost and cells.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::build::{self, Part, RoadPlan};
use crate::curve::Curve;
use crate::geom::{Cell, CellRect};
use crate::grid::Zone;
use crate::junction::{JunctionControl, JunctionSetting};
use crate::roads::{NodeId, RoadKind, RoadNetwork, SegmentId};
use crate::zoning::CellRef;
use crate::{BuildingId, World, balance, geom};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Shorthand: a straight road of `kind` between two cell centres
    /// (horizontal, vertical or 45°). Pieces already built as another kind
    /// are changed in place: a street becomes an avenue for the price
    /// difference, an avenue becomes a street for free (no refund). The
    /// highway stub never changes. `kind` must be buildable
    /// (`RoadKind::BUILDABLE`).
    BuildRoad { from: Cell, to: Cell, kind: RoadKind },
    /// A road of `kind` along `curve` (sim metres), anywhere on the map. An
    /// end within `balance::SNAP_NODE_M` of a node joins it; an end on an
    /// existing road splits it into a T; every crossing with an existing
    /// road becomes a junction splitting both. A straight curve laid exactly
    /// along an existing straight road changes that stretch's kind like
    /// `BuildRoad`. Refused (with the position) when too short, too tight
    /// for the kind, making arms closer than `MIN_ARM_ANGLE_DEG`, running
    /// along or too close beside another road, crossing itself, a building
    /// in the way, off the map, or not affordable. Priced by metre.
    BuildCurve { curve: Curve, kind: RoadKind },
    /// Change the kind of the part of `segment` from `from_m` to `to_m`
    /// metres from its `a` node (`0..=length` for all of it; the segment is
    /// split at interior ends). Upgrades cost the price difference by metre,
    /// downgrades are free.
    SetRoadKind {
        segment: SegmentId,
        from_m: f32,
        to_m: f32,
        kind: RoadKind,
    },
    /// Remove the part of `segment` from `from_m` to `to_m` metres from its
    /// `a` node (`0..=length` for all of it). Never refused for money.
    RemoveRoad { segment: SegmentId, from_m: f32, to_m: f32 },
    /// Shorthand: remove road pieces touching cells in the rectangle, and
    /// buildings overlapping it. The highway stub is not removable. Allowed
    /// without the money for it (the cost then pushes money negative), so a
    /// city in debt can still demolish roads to cut upkeep.
    Bulldoze { min: Cell, max: Cell },
    /// Remove everything in a convex quad (sim metres, either winding): the
    /// parts of roads whose centre line runs inside it and the buildings
    /// overlapping it. Never refused for money.
    BulldozeArea { quad: [[f32; 2]; 4] },
    /// Shorthand: zone (or with `None`, unzone) the eligible cells whose
    /// centre lies in the rectangle of world cells.
    SetZone { min: Cell, max: Cell, zone: Option<Zone> },
    /// Zone (or unzone) every eligible cell whose centre lies inside a
    /// convex quad (sim metres, either winding).
    ZoneArea { quad: [[f32; 2]; 4], zone: Option<Zone> },
    /// Zone (or unzone) these cells (those that are eligible).
    ZoneCells { cells: Vec<CellRef>, zone: Option<Zone> },
    /// Fraction in `0.0..=balance::MAX_TAX_RATE`.
    SetTaxRate(f32),
    /// Choose how the junction at this cell's centre (a node with three or
    /// more arms) is controlled. `Auto` lets the sim decide.
    SetJunctionControl { cell: Cell, control: JunctionSetting },
    /// The same for a junction node anywhere.
    SetNodeControl { node: NodeId, control: JunctionSetting },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandError {
    /// A cell (or the whole rectangle) lies outside the map.
    OutOfBounds,
    /// Road start and end are the same cell.
    ZeroLength,
    /// `BuildRoad`: not horizontal, vertical or exactly diagonal.
    NotStraight,
    /// A building stands on this cell of the road.
    BlockedByBuilding(Cell),
    /// No longer produced (crossing diagonals now make a junction); kept so
    /// matches on old variants still compile.
    DiagonalCrossing(Cell),
    InsufficientFunds {
        cost: i64,
        money: i64,
    },
    InvalidTaxRate(f32),
    /// The road kind cannot be built by the player (the highway).
    InvalidRoadKind(RoadKind),
    /// No junction (node with three or more arms) at this cell.
    NotAJunction(Cell),
    /// A road (or a piece it would leave between two nodes) shorter than
    /// `min` metres, at `at`.
    TooShort {
        at: [f32; 2],
        length: f32,
        min: f32,
    },
    /// Curve radius `radius` at `at` is below the kind's minimum.
    TooTight {
        at: [f32; 2],
        radius: f32,
        min: f32,
    },
    /// Two arms at the node `at` would meet at `degrees`, below the
    /// minimum.
    SharpAngle {
        at: [f32; 2],
        degrees: f32,
        min: f32,
    },
    /// Runs along or too close beside an existing road (or a node) at `at`.
    TooClose {
        at: [f32; 2],
    },
    /// The curve crosses itself at `at`.
    SelfCrossing {
        at: [f32; 2],
    },
    /// A building stands in the way at `at`.
    BuildingInTheWay {
        at: [f32; 2],
        building: BuildingId,
    },
    /// The point `at` is off the map.
    OffMap {
        at: [f32; 2],
    },
    /// The segment does not exist (or the range is empty).
    NoSuchRoad,
    /// No junction at that node.
    NotANode,
    /// A node would get `arms` arms, more than `balance::MAX_NODE_ARMS`.
    TooManyArms {
        at: [f32; 2],
        arms: u8,
    },
}

impl CommandError {
    /// Where the problem is, sim metres, when it has a place.
    pub fn at(&self) -> Option<[f32; 2]> {
        match *self {
            CommandError::BlockedByBuilding(c) | CommandError::DiagonalCrossing(c) | CommandError::NotAJunction(c) => {
                Some(c.center())
            }
            CommandError::TooShort { at, .. }
            | CommandError::TooTight { at, .. }
            | CommandError::SharpAngle { at, .. }
            | CommandError::TooClose { at }
            | CommandError::SelfCrossing { at }
            | CommandError::BuildingInTheWay { at, .. }
            | CommandError::OffMap { at }
            | CommandError::TooManyArms { at, .. } => Some(at),
            _ => None,
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::OutOfBounds => write!(f, "outside the map"),
            CommandError::ZeroLength => write!(f, "road needs two different cells"),
            CommandError::NotStraight => write!(f, "roads must be straight or 45 degrees"),
            CommandError::BlockedByBuilding(c) => write!(f, "building in the way at ({}, {})", c.x, c.y),
            CommandError::DiagonalCrossing(c) => write!(f, "diagonal roads cannot cross near ({}, {})", c.x, c.y),
            CommandError::InsufficientFunds { cost, money } => write!(f, "costs ${cost}, you have ${money}"),
            CommandError::InvalidTaxRate(r) => write!(f, "tax rate {r} out of range"),
            CommandError::InvalidRoadKind(k) => write!(f, "{k:?} roads cannot be built"),
            CommandError::NotAJunction(c) => write!(f, "no junction at ({}, {})", c.x, c.y),
            CommandError::TooShort { length, min, .. } => {
                write!(f, "too short: {length:.1} m, roads need at least {min:.0} m")
            }
            CommandError::TooTight { radius, min, .. } => {
                write!(f, "curve too tight: radius {radius:.0} m, this road needs {min:.0} m")
            }
            CommandError::SharpAngle { degrees, min, .. } => {
                write!(f, "roads meet at {degrees:.0}°, at least {min:.0}° needed")
            }
            CommandError::TooClose { .. } => write!(f, "too close to another road"),
            CommandError::SelfCrossing { .. } => write!(f, "the road crosses itself"),
            CommandError::BuildingInTheWay { .. } => write!(f, "a building is in the way"),
            CommandError::OffMap { .. } => write!(f, "outside the map"),
            CommandError::NoSuchRoad => write!(f, "no such road"),
            CommandError::NotANode => write!(f, "no junction there"),
            CommandError::TooManyArms { arms, .. } => write!(f, "{arms} roads cannot meet at one junction"),
        }
    }
}

impl std::error::Error for CommandError {}

/// What a command would do. Returned by `World::preview`; `World::apply`
/// returns the same ([`Outcome`]).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Preview {
    /// Money that will be spent.
    pub cost: i64,
    /// World-grid cells the command affects: for `BuildRoad` every cell of
    /// the line, for zoning the world cells of the cells that get (un)zoned,
    /// for bulldoze the cells cleared, for junction control the junction's
    /// cell.
    pub cells: Vec<Cell>,
    /// Roads: new road length in cells (`length_m / CELL_SIZE`, rounded).
    pub new_pieces: u32,
    /// Roads: changed road length in cells, to a pricier kind.
    pub upgraded: u32,
    /// Roads: changed road length in cells, to a cheaper kind (no refund).
    pub downgraded: u32,
    /// Junction control: what changes.
    pub junction: Option<JunctionChange>,
    /// Roads: the pieces built or changed, as they will be (split at every
    /// node they will have).
    pub pieces: Vec<PlannedPiece>,
    /// Roads: the nodes the command makes or joins.
    pub nodes: Vec<PlannedNode>,
    /// Roads: metres of new road.
    pub length_m: f32,
    /// Removal: the road parts removed.
    pub removed: Vec<Curve>,
    /// Removal: buildings demolished.
    pub buildings: Vec<BuildingId>,
    /// Zoning: the cells that get (un)zoned.
    pub zone_cells: Vec<CellRef>,
}

/// One road piece of a [`Preview`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlannedPiece {
    pub curve: Curve,
    pub kind: RoadKind,
    pub change: PieceChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PieceChange {
    New,
    /// An existing piece becomes a pricier kind.
    Upgrade,
    /// An existing piece becomes a cheaper kind.
    Downgrade,
}

/// A node a road command makes or uses.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlannedNode {
    pub pos: [f32; 2],
    pub change: NodeChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeChange {
    /// A new dead end.
    End,
    /// Joins an existing node (a new arm).
    Join,
    /// Splits an existing road into a T.
    Split,
    /// A crossing with an existing road (both split).
    Crossing,
}

/// Effect of `Command::SetJunctionControl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JunctionChange {
    pub node: NodeId,
    pub setting_before: JunctionSetting,
    pub setting_after: JunctionSetting,
    pub control_before: JunctionControl,
    pub control_after: JunctionControl,
}

/// What a command did. Returned by `World::apply`; equals the preview.
pub type Outcome = Preview;

/// A validated command, ready to execute.
pub(crate) enum Plan {
    Road {
        plan: RoadPlan,
        /// World cells for `Preview::cells`.
        cells: Vec<Cell>,
    },
    Kind {
        part: Part,
        kind: RoadKind,
        cost: i64,
        up_m: f32,
        down_m: f32,
        old: RoadKind,
        curve: Curve,
    },
    Remove {
        parts: Vec<Part>,
        removed: Vec<Curve>,
        buildings: Vec<BuildingId>,
        cells: Vec<Cell>,
        cost: i64,
    },
    Zone {
        cells: Vec<CellRef>,
        zone: Option<Zone>,
    },
    Tax(f32),
    Junction {
        cell: Cell,
        change: JunctionChange,
    },
}

fn cells_m(m: f32) -> u32 {
    (m / crate::CELL_SIZE).round() as u32
}

impl Plan {
    pub(crate) fn preview(&self, world: &World) -> Preview {
        match self {
            Plan::Road { plan, cells } => Preview {
                cost: plan.cost,
                cells: cells.clone(),
                new_pieces: cells_m(plan.new_m),
                upgraded: cells_m(plan.upgraded_m),
                downgraded: cells_m(plan.downgraded_m),
                pieces: plan.preview_pieces(world),
                nodes: plan.preview_nodes(),
                length_m: plan.new_m,
                ..Preview::default()
            },
            Plan::Kind {
                cost,
                up_m,
                down_m,
                kind,
                old,
                curve,
                ..
            } => Preview {
                cost: *cost,
                upgraded: cells_m(*up_m),
                downgraded: cells_m(*down_m),
                pieces: if old == kind {
                    Vec::new()
                } else {
                    vec![PlannedPiece {
                        curve: *curve,
                        kind: *kind,
                        change: if kind.cost_per_cell() >= old.cost_per_cell() {
                            PieceChange::Upgrade
                        } else {
                            PieceChange::Downgrade
                        },
                    }]
                },
                ..Preview::default()
            },
            Plan::Remove {
                removed,
                buildings,
                cells,
                cost,
                ..
            } => Preview {
                cost: *cost,
                cells: cells.clone(),
                removed: removed.clone(),
                buildings: buildings.clone(),
                ..Preview::default()
            },
            Plan::Zone { cells, .. } => {
                let mut world_cells: Vec<Cell> = cells
                    .iter()
                    .filter_map(|&c| world.zoning.cell_center(c))
                    .map(Cell::from_pos)
                    .collect();
                world_cells.sort_by_key(|c| (c.y, c.x));
                world_cells.dedup();
                Preview {
                    cost: 0,
                    cells: world_cells,
                    zone_cells: cells.clone(),
                    ..Preview::default()
                }
            }
            Plan::Tax(_) => Preview::default(),
            Plan::Junction { cell, change } => Preview {
                cells: vec![*cell],
                junction: Some(*change),
                ..Preview::default()
            },
        }
    }
}

fn afford(world: &World, cost: i64) -> Result<(), CommandError> {
    if cost > 0 && cost > world.economy.money {
        Err(CommandError::InsufficientFunds {
            cost,
            money: world.economy.money,
        })
    } else {
        Ok(())
    }
}

fn quad_of_rect(r: CellRect) -> [[f32; 2]; 4] {
    let (x0, y0) = (r.min.x as f32 * crate::CELL_SIZE, r.min.y as f32 * crate::CELL_SIZE);
    let (x1, y1) = (
        (r.max.x + 1) as f32 * crate::CELL_SIZE,
        (r.max.y + 1) as f32 * crate::CELL_SIZE,
    );
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// Do two convex quads overlap by more than touching (`a` is shrunk by a
/// few centimetres first)?
fn quads_overlap(a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]) -> bool {
    let c = [
        (a[0][0] + a[1][0] + a[2][0] + a[3][0]) * 0.25,
        (a[0][1] + a[1][1] + a[2][1] + a[3][1]) * 0.25,
    ];
    let a: [[f32; 2]; 4] = a.map(|p| {
        let d = geom::sub(c, p);
        let l = geom::dot(d, d).sqrt().max(1e-6);
        [p[0] + d[0] / l * 0.05, p[1] + d[1] / l * 0.05]
    });
    // Separating axis test over both quads' edge normals.
    for poly in [&a, b] {
        for k in 0..4 {
            let e = geom::sub(poly[(k + 1) % 4], poly[k]);
            let n = [-e[1], e[0]];
            let proj = |q: &[[f32; 2]; 4]| {
                q.iter()
                    .map(|p| geom::dot(*p, n))
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)))
            };
            let (alo, ahi) = proj(&a);
            let (blo, bhi) = proj(b);
            if ahi < blo || bhi < alo {
                return false;
            }
        }
    }
    true
}

pub(crate) fn plan(world: &World, cmd: &Command) -> Result<Plan, CommandError> {
    match cmd {
        &Command::BuildRoad { from, to, kind } => plan_cell_road(world, from, to, kind),
        &Command::BuildCurve { curve, kind } => {
            let plan = build::plan_road(world, curve, kind)?;
            afford(world, plan.cost)?;
            let mut cells: Vec<Cell> = Vec::new();
            for p in curve.flatten(0.1).windows(2) {
                let n = (geom::dist(p[0], p[1]) / 2.0).ceil().max(1.0) as usize;
                for k in 0..=n {
                    let q = [
                        p[0][0] + (p[1][0] - p[0][0]) * k as f32 / n as f32,
                        p[0][1] + (p[1][1] - p[0][1]) * k as f32 / n as f32,
                    ];
                    let c = Cell::from_pos(q);
                    if c.in_bounds() && cells.last() != Some(&c) && !cells.contains(&c) {
                        cells.push(c);
                    }
                }
            }
            Ok(Plan::Road { plan, cells })
        }
        &Command::SetRoadKind {
            segment,
            from_m,
            to_m,
            kind,
        } => {
            if !RoadKind::BUILDABLE.contains(&kind) {
                return Err(CommandError::InvalidRoadKind(kind));
            }
            let p = build::part(&world.roads, segment, from_m, to_m).ok_or(CommandError::NoSuchRoad)?;
            let seg = &world.roads.segments[segment];
            if seg.kind == RoadKind::Highway {
                return Err(CommandError::InvalidRoadKind(RoadKind::Highway));
            }
            let l = p.hi - p.lo;
            let diff = kind.cost_per_cell() - seg.kind.cost_per_cell();
            let cost = if diff > 0 { build::price(l, diff) } else { 0 };
            afford(world, cost)?;
            Ok(Plan::Kind {
                part: p,
                kind,
                cost,
                up_m: if diff > 0 { l } else { 0.0 },
                down_m: if diff < 0 { l } else { 0.0 },
                old: seg.kind,
                curve: seg.curve.sub(p.lo, p.hi),
            })
        }
        &Command::RemoveRoad { segment, from_m, to_m } => {
            let p = build::part(&world.roads, segment, from_m, to_m).ok_or(CommandError::NoSuchRoad)?;
            let seg = &world.roads.segments[segment];
            if seg.kind == RoadKind::Highway {
                return Err(CommandError::InvalidRoadKind(RoadKind::Highway));
            }
            let curve = seg.curve.sub(p.lo, p.hi);
            Ok(Plan::Remove {
                cost: (cells_m(p.hi - p.lo).max(1) as i64) * balance::BULLDOZE_COST_PER_CELL,
                parts: vec![p],
                removed: vec![curve],
                buildings: Vec::new(),
                cells: Vec::new(),
            })
        }
        Command::BulldozeArea { quad } => plan_area(world, quad, None),
        &Command::Bulldoze { min, max } => {
            let rect = CellRect::from_corners(min, max)
                .clamp_to_map()
                .ok_or(CommandError::OutOfBounds)?;
            plan_area(world, &quad_of_rect(rect), Some(rect))
        }
        &Command::SetZone { min, max, zone } => {
            let rect = CellRect::from_corners(min, max)
                .clamp_to_map()
                .ok_or(CommandError::OutOfBounds)?;
            Ok(zone_list(
                world,
                crate::zoning::cells_in_quad(world, &quad_of_rect(rect)),
                zone,
            ))
        }
        Command::ZoneArea { quad, zone } => Ok(zone_list(world, crate::zoning::cells_in_quad(world, quad), *zone)),
        Command::ZoneCells { cells, zone } => Ok(zone_list(world, cells.clone(), *zone)),
        &Command::SetTaxRate(r) => {
            if r.is_finite() && (0.0..=balance::MAX_TAX_RATE).contains(&r) {
                Ok(Plan::Tax(r))
            } else {
                Err(CommandError::InvalidTaxRate(r))
            }
        }
        &Command::SetJunctionControl { cell, control } => {
            if !cell.in_bounds() {
                return Err(CommandError::OutOfBounds);
            }
            let node = crate::junction::junction_at(world, cell).ok_or(CommandError::NotAJunction(cell))?;
            plan_junction(world, node, cell, control)
        }
        &Command::SetNodeControl { node, control } => {
            let n = world.roads.nodes.get(node).ok_or(CommandError::NotANode)?;
            if n.segments.len() < 3 {
                return Err(CommandError::NotANode);
            }
            plan_junction(world, node, Cell::from_pos(n.pos), control)
        }
    }
}

/// The cell shorthand: a straight road between two cell centres.
fn plan_cell_road(world: &World, from: Cell, to: Cell, kind: RoadKind) -> Result<Plan, CommandError> {
    if !RoadKind::BUILDABLE.contains(&kind) {
        return Err(CommandError::InvalidRoadKind(kind));
    }
    if !from.in_bounds() || !to.in_bounds() {
        return Err(CommandError::OutOfBounds);
    }
    if from == to {
        return Err(CommandError::ZeroLength);
    }
    let cells = from.straight_line_to(to).ok_or(CommandError::NotStraight)?;
    let plan = match build::plan_road(world, Curve::straight(from.center(), to.center()), kind) {
        Ok(p) => p,
        Err(CommandError::BuildingInTheWay { at, building }) => {
            // The first cell of the line the building stands on.
            let corners = world.buildings[building].corners();
            let c = cells
                .iter()
                .copied()
                .find(|c| crate::zoning::in_quad(&corners, c.center()))
                .unwrap_or(Cell::from_pos(at));
            return Err(CommandError::BlockedByBuilding(c));
        }
        Err(e) => return Err(e),
    };
    afford(world, plan.cost)?;
    Ok(Plan::Road { plan, cells })
}

/// Remove the road parts inside a quad (each widened by `extend` metres)
/// and the buildings overlapping it. `rect`: the cell shorthand, whose
/// cost counts the cells cleared.
fn plan_area(world: &World, quad: &[[f32; 2]; 4], rect: Option<CellRect>) -> Result<Plan, CommandError> {
    let lo = quad
        .iter()
        .fold([f32::INFINITY; 2], |a, p| [a[0].min(p[0]), a[1].min(p[1])]);
    let hi = quad
        .iter()
        .fold([f32::NEG_INFINITY; 2], |a, p| [a[0].max(p[0]), a[1].max(p[1])]);
    let mut parts = Vec::new();
    let mut removed = Vec::new();
    for (s, seg) in world.roads.segments.iter() {
        if seg.kind == RoadKind::Highway {
            continue;
        }
        let (a, b) = seg.curve.bounds();
        if a[0] > hi[0] || b[0] < lo[0] || a[1] > hi[1] || b[1] < lo[1] {
            continue;
        }
        parts.extend(build::parts_in_quad(&world.roads, s, quad, rect.is_some()));
    }
    let parts = build::merge_parts(&world.roads, parts);
    for p in &parts {
        removed.push(world.roads.segments[p.segment].curve.sub(p.lo, p.hi));
    }
    let mut buildings = Vec::new();
    let mut bcells: Vec<Cell> = Vec::new();
    let mut building_cells = 0u32;
    for (id, b) in world.buildings.iter() {
        if quads_overlap(&b.corners(), quad) {
            buildings.push(id);
            building_cells += b.footprint.area();
            if b.footprint.lattice == crate::zoning::LatticeId::WORLD {
                bcells.extend(b.footprint.rect().cells());
            } else {
                bcells.push(Cell::from_pos(b.center()));
            }
        }
    }
    // Never refused for money: a player in debt must be able to demolish
    // roads to cut their upkeep. The small cost may push money negative.
    let (cells, cost) = match rect {
        Some(r) => {
            let mut cells: std::collections::BTreeSet<Cell> = bcells.into_iter().collect();
            // Cells of the rectangle whose centre a removed part passes.
            for cv in &removed {
                let l = cv.length();
                let n = (l / 0.5).ceil().max(1.0) as usize;
                let mut last: Option<Cell> = None;
                for k in 0..=n {
                    let c = Cell::from_pos(cv.pose_at(l * k as f32 / n as f32, 0.0).pos);
                    if last == Some(c) {
                        continue;
                    }
                    last = Some(c);
                    if r.contains(c) && cv.nearest(c.center()).1 < 0.02 {
                        cells.insert(c);
                    }
                }
            }
            let cells: Vec<Cell> = cells.into_iter().collect();
            let cost = cells.len() as i64 * balance::BULLDOZE_COST_PER_CELL;
            (cells, cost)
        }
        None => {
            let m: f32 = removed.iter().map(|c| c.length()).sum();
            let mut cells = bcells;
            cells.sort();
            cells.dedup();
            (
                cells,
                (cells_m(m) + building_cells) as i64 * balance::BULLDOZE_COST_PER_CELL,
            )
        }
    };
    Ok(Plan::Remove {
        parts,
        removed,
        buildings,
        cells,
        cost,
    })
}

fn plan_junction(world: &World, node: NodeId, cell: Cell, setting: JunctionSetting) -> Result<Plan, CommandError> {
    let fresh;
    let state = match world.traffic.junction_states.get(node) {
        Some(j) if j.arms.len() == world.roads.nodes[node].segments.len() => j,
        old => {
            fresh = crate::junction::build_state(&world.roads, node, old);
            &fresh
        }
    };
    Ok(Plan::Junction {
        cell,
        change: JunctionChange {
            node,
            setting_before: state.setting,
            setting_after: setting,
            control_before: state.control,
            control_after: crate::junction::control_under(state, setting),
        },
    })
}

/// The eligible cells of a list: zoning needs a usable cell without a
/// building, unzoning any cell with a zone and without a building.
fn zone_list(world: &World, cells: Vec<CellRef>, zone: Option<Zone>) -> Plan {
    let mut cells: Vec<CellRef> = cells
        .into_iter()
        .filter(|&c| {
            world
                .zoning
                .cell(c)
                .is_some_and(|x| x.building.is_none() && x.zone != zone && (zone.is_none() || x.usable()))
        })
        .collect();
    cells.sort();
    cells.dedup();
    Plan::Zone { cells, zone }
}

pub(crate) fn execute(world: &mut World, plan: Plan) -> Outcome {
    let preview = plan.preview(world);
    match plan {
        Plan::Road { plan, .. } => {
            // Redrawing a road as it is changes nothing (and splits nothing).
            if !preview.pieces.is_empty() {
                build::execute_road(world, &plan);
                world.versions.roads += 1;
            }
            world.economy.money -= plan.cost;
        }
        Plan::Kind {
            part, kind, cost, old, ..
        } => {
            if old != kind {
                build::execute_kind(world, part, kind);
                world.versions.roads += 1;
            }
            world.economy.money -= cost;
        }
        Plan::Remove {
            parts,
            removed,
            buildings,
            cost,
            ..
        } => {
            build::execute_remove(world, &parts);
            let _ = removed;
            if !buildings.is_empty() {
                // Residents of removed buildings leave, their workers lose
                // the job; the zoning stays.
                crate::growth::remove_buildings(world, &buildings);
            }
            world.economy.money -= cost;
            if !parts.is_empty() {
                world.versions.roads += 1;
            }
        }
        Plan::Zone { cells, zone } => {
            for &c in &cells {
                if let Some(x) = world.zoning.cell_mut(c) {
                    x.zone = zone;
                }
                if zone.is_none()
                    && let Some(l) = world.zoning.lattice_mut(c.lattice)
                    && l.cells
                        .get(&(c.i, c.j))
                        .is_some_and(|x| !x.covered && x.building.is_none())
                {
                    l.cells.remove(&(c.i, c.j));
                }
            }
            if !cells.is_empty() {
                world.versions.grid += 1;
                world.zoning.zones_version += 1;
            }
        }
        Plan::Tax(r) => world.economy.tax_rate = r,
        Plan::Junction { change, .. } => {
            crate::traffic::sync_junctions(world);
            if let Some(j) = world.traffic.junction_states.get_mut(change.node) {
                crate::junction::set_setting(j, change.setting_after);
            }
            world.traffic.control_version += 1;
        }
    }
    preview
}

/// Is a road's reserve (half a cell either side of any centre line) over
/// the centre of world cell `c`?
pub(crate) fn road_over(roads: &RoadNetwork, near: &[SegmentId], c: Cell) -> bool {
    let p = c.center();
    near.iter().any(|&s| {
        roads.segments.get(s).is_some_and(|seg| {
            let (a, b) = seg.curve.bounds();
            let r = seg.kind.reserve_half_width_m();
            p[0] >= a[0] - r && p[0] <= b[0] + r && p[1] >= a[1] - r && p[1] <= b[1] + r && seg.nearest(p).1 < r - 0.01
        })
    })
}
