//! Player commands: validation, cost, preview and execution.
//!
//! Every command goes through [`plan`], which validates against the current
//! state without mutating it. `World::preview` returns the plan's summary;
//! `World::apply` executes the plan. So `preview(c).is_ok()` exactly when
//! `apply(c)` would succeed, and both report the same cost and cells.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::curve::Curve;
use crate::geom::{Cell, CellRect};
use crate::grid::{Occupant, Zone};
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
            | CommandError::OffMap { at } => Some(at),
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
        cells: Vec<Cell>,
        kind: RoadKind,
        /// Unit pieces (adjacent cell pairs) not yet built.
        new_pieces: Vec<(Cell, Cell)>,
        /// Built pieces of another (changeable) kind.
        changed: Vec<(Cell, Cell)>,
        upgraded: u32,
        downgraded: u32,
        cost: i64,
        pieces: Vec<PlannedPiece>,
        nodes: Vec<PlannedNode>,
    },
    Bulldoze {
        pieces: Vec<(Cell, Cell)>,
        buildings: Vec<BuildingId>,
        cells: Vec<Cell>,
        cost: i64,
    },
    Zone {
        cells: Vec<Cell>,
        zone: Option<Zone>,
    },
    Tax(f32),
    Junction {
        cell: Cell,
        change: JunctionChange,
    },
}

impl Plan {
    pub(crate) fn preview(&self) -> Preview {
        match self {
            Plan::Road {
                cells,
                cost,
                new_pieces,
                upgraded,
                downgraded,
                pieces,
                nodes,
                ..
            } => Preview {
                cost: *cost,
                cells: cells.clone(),
                new_pieces: new_pieces.len() as u32,
                upgraded: *upgraded,
                downgraded: *downgraded,
                pieces: pieces.clone(),
                nodes: nodes.clone(),
                length_m: pieces
                    .iter()
                    .filter(|p| p.change == PieceChange::New)
                    .map(|p| p.curve.length())
                    .sum(),
                ..Preview::default()
            },
            Plan::Bulldoze {
                cells,
                cost,
                pieces,
                buildings,
            } => Preview {
                cost: *cost,
                cells: cells.clone(),
                removed: pieces
                    .iter()
                    .map(|(p, q)| Curve::straight(p.center(), q.center()))
                    .collect(),
                buildings: buildings.clone(),
                ..Preview::default()
            },
            Plan::Zone { cells, .. } => Preview {
                cost: 0,
                cells: cells.clone(),
                zone_cells: cells.iter().map(|&c| CellRef::world(c)).collect(),
                ..Preview::default()
            },
            Plan::Tax(_) => Preview::default(),
            Plan::Junction { cell, change } => Preview {
                cells: vec![*cell],
                junction: Some(*change),
                ..Preview::default()
            },
        }
    }
}

const DIRS8: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

fn ordered(p: Cell, q: Cell) -> (Cell, Cell) {
    if p <= q { (p, q) } else { (q, p) }
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

/// The world cell whose centre is `p`, if `p` is a cell centre.
fn centre_cell(p: [f32; 2]) -> Option<Cell> {
    let c = Cell::from_pos(p);
    geom::same_pos(c.center(), p).then_some(c)
}

/// Map a world-space command onto the cell-based shorthand while only the
/// lattice is supported.
fn lattice_road(world: &World, curve: &Curve) -> Result<(Cell, Cell), CommandError> {
    let _ = world;
    for p in [curve.start(), curve.end()] {
        if !(0.0..crate::MAP_SIZE_M).contains(&p[0]) || !(0.0..crate::MAP_SIZE_M).contains(&p[1]) {
            return Err(CommandError::OffMap { at: p });
        }
    }
    match (
        curve.is_straight(),
        centre_cell(curve.start()),
        centre_cell(curve.end()),
    ) {
        (true, Some(a), Some(b)) => Ok((a, b)),
        _ => Err(CommandError::NotStraight),
    }
}

/// World cells of a part of a straight lattice segment.
fn lattice_part(world: &World, s: SegmentId, from_m: f32, to_m: f32) -> Result<(Cell, Cell), CommandError> {
    let seg = world.roads.segments.get(s).ok_or(CommandError::NoSuchRoad)?;
    let (lo, hi) = (from_m.min(to_m).max(0.0), from_m.max(to_m).min(seg.length));
    if hi - lo < 1e-3 {
        return Err(CommandError::NoSuchRoad);
    }
    let a = seg.pose(lo, 0.0).pos;
    let b = seg.pose(hi, 0.0).pos;
    match (seg.curve.is_straight(), centre_cell(a), centre_cell(b)) {
        (true, Some(a), Some(b)) => Ok((a, b)),
        _ => Err(CommandError::NotStraight),
    }
}

fn quad_cells(quad: &[[f32; 2]; 4]) -> Vec<Cell> {
    let lo = quad
        .iter()
        .fold([f32::INFINITY; 2], |a, p| [a[0].min(p[0]), a[1].min(p[1])]);
    let hi = quad
        .iter()
        .fold([f32::NEG_INFINITY; 2], |a, p| [a[0].max(p[0]), a[1].max(p[1])]);
    let r = CellRect::from_corners(Cell::from_pos(lo), Cell::from_pos(hi));
    match r.clamp_to_map() {
        Some(r) => r.cells().filter(|c| crate::zoning::in_quad(quad, c.center())).collect(),
        None => Vec::new(),
    }
}

pub(crate) fn plan(world: &World, cmd: &Command) -> Result<Plan, CommandError> {
    match cmd {
        &Command::BuildRoad { from, to, kind } => plan_road(world, from, to, kind),
        Command::BuildCurve { curve, kind } => {
            let (a, b) = lattice_road(world, curve)?;
            plan_road(world, a, b, *kind)
        }
        &Command::SetRoadKind {
            segment,
            from_m,
            to_m,
            kind,
        } => {
            let (a, b) = lattice_part(world, segment, from_m, to_m)?;
            plan_road(world, a, b, kind)
        }
        &Command::RemoveRoad { segment, from_m, to_m } => {
            let (a, b) = lattice_part(world, segment, from_m, to_m)?;
            let line = a.straight_line_to(b).ok_or(CommandError::NotStraight)?;
            let pieces: Vec<(Cell, Cell)> = line.windows(2).map(|w| ordered(w[0], w[1])).collect();
            let cost = line.len() as i64 * balance::BULLDOZE_COST_PER_CELL;
            Ok(Plan::Bulldoze {
                pieces,
                buildings: Vec::new(),
                cells: line,
                cost,
            })
        }
        Command::BulldozeArea { quad } => {
            let cells = quad_cells(quad);
            let lo = cells.iter().fold(None::<CellRect>, |r, &c| {
                Some(match r {
                    None => CellRect::single(c),
                    Some(r) => CellRect::from_corners(
                        Cell::new(r.min.x.min(c.x), r.min.y.min(c.y)),
                        Cell::new(r.max.x.max(c.x), r.max.y.max(c.y)),
                    ),
                })
            });
            let rect = lo.ok_or(CommandError::OutOfBounds)?;
            plan_bulldoze(world, rect)
        }
        &Command::Bulldoze { min, max } => plan_bulldoze(world, CellRect::from_corners(min, max)),
        &Command::SetZone { min, max, zone } => plan_zone(world, CellRect::from_corners(min, max), zone),
        Command::ZoneArea { quad, zone } => Ok(zone_list(world, quad_cells(quad), *zone)),
        Command::ZoneCells { cells, zone } => Ok(zone_list(
            world,
            cells
                .iter()
                .filter(|c| c.lattice == crate::zoning::LatticeId::WORLD)
                .map(|c| Cell::new(c.i, c.j))
                .filter(|c| c.in_bounds())
                .collect(),
            *zone,
        )),
        &Command::SetNodeControl { node, control } => {
            let pos = world.roads.nodes.get(node).ok_or(CommandError::NotANode)?.pos;
            let cell = centre_cell(pos).ok_or(CommandError::NotANode)?;
            plan_junction(world, cell, control)
        }
        &Command::SetTaxRate(r) => {
            if r.is_finite() && (0.0..=balance::MAX_TAX_RATE).contains(&r) {
                Ok(Plan::Tax(r))
            } else {
                Err(CommandError::InvalidTaxRate(r))
            }
        }
        &Command::SetJunctionControl { cell, control } => plan_junction(world, cell, control),
    }
}

fn plan_junction(world: &World, cell: Cell, setting: JunctionSetting) -> Result<Plan, CommandError> {
    if !cell.in_bounds() {
        return Err(CommandError::OutOfBounds);
    }
    let node = crate::junction::junction_at(world, cell).ok_or(CommandError::NotAJunction(cell))?;
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

fn plan_road(world: &World, from: Cell, to: Cell, kind: RoadKind) -> Result<Plan, CommandError> {
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
    for &c in &cells {
        if let Some(cd) = world.grid.get(c)
            && matches!(cd.occupant, Occupant::Building(_))
        {
            return Err(CommandError::BlockedByBuilding(c));
        }
    }
    let mut new_pieces = Vec::new();
    let mut changed = Vec::new();
    let (mut upgraded, mut downgraded, mut change_cost) = (0u32, 0u32, 0i64);
    for w in cells.windows(2) {
        let (p, q) = (w[0], w[1]);
        if let Some(s) = world.roads.piece(p, q) {
            let old = world.roads.segments[s].kind;
            if old != kind && old != RoadKind::Highway {
                changed.push((p, q));
                let diff = kind.cost_per_cell() - old.cost_per_cell();
                if diff > 0 {
                    upgraded += 1;
                    change_cost += diff;
                } else {
                    downgraded += 1;
                }
            }
            continue;
        }
        let (dx, dy) = (q.x - p.x, q.y - p.y);
        if dx != 0 && dy != 0 {
            // The other diagonal of the same 2x2 block.
            let (r, s) = (p.offset(dx, 0), p.offset(0, dy));
            if world.roads.piece(r, s).is_some() {
                return Err(CommandError::DiagonalCrossing(p));
            }
        }
        new_pieces.push((p, q));
    }
    let cost = new_pieces.len() as i64 * kind.cost_per_cell() + change_cost;
    afford(world, cost)?;
    let mut pieces: Vec<PlannedPiece> = new_pieces
        .iter()
        .map(|&(p, q)| PlannedPiece {
            curve: Curve::straight(p.center(), q.center()),
            kind,
            change: PieceChange::New,
        })
        .collect();
    for &(p, q) in &changed {
        let old = world.roads.piece(p, q).map(|s| world.roads.segments[s].kind);
        pieces.push(PlannedPiece {
            curve: Curve::straight(p.center(), q.center()),
            kind,
            change: if old.is_some_and(|o| o.cost_per_cell() > kind.cost_per_cell()) {
                PieceChange::Downgrade
            } else {
                PieceChange::Upgrade
            },
        });
    }
    let mut nodes = Vec::new();
    for (k, &c) in cells.iter().enumerate() {
        let p = c.center();
        let end = k == 0 || k + 1 == cells.len();
        let change = if world.roads.node_at(p).is_some() {
            Some(NodeChange::Join)
        } else if world.roads.segment_through(p).is_some() {
            Some(if end { NodeChange::Split } else { NodeChange::Crossing })
        } else if end {
            Some(NodeChange::End)
        } else {
            None
        };
        // Only nodes the new pieces touch.
        let touched = new_pieces.iter().any(|&(a, b)| a == c || b == c);
        if let Some(change) = change.filter(|_| touched) {
            nodes.push(PlannedNode { pos: p, change });
        }
    }
    Ok(Plan::Road {
        cells,
        kind,
        new_pieces,
        changed,
        upgraded,
        downgraded,
        cost,
        pieces,
        nodes,
    })
}

/// Cells a segment passes through, from its `a` end to its `b` end, when
/// both ends are cell centres on one straight (8-way) line, as for every
/// road built by commands. Its unit pieces are the consecutive pairs.
fn segment_cells(roads: &RoadNetwork, s: SegmentId) -> Option<Vec<Cell>> {
    let (pa, pb) = roads.segment_ends(s);
    let (ca, cb) = (Cell::from_pos(pa), Cell::from_pos(pb));
    if !geom::same_pos(ca.center(), pa) || !geom::same_pos(cb.center(), pb) {
        return None;
    }
    ca.straight_line_to(cb).filter(|l| l.len() >= 2)
}

/// Bounding rectangle of the cells a segment touches.
fn segment_bounds(roads: &RoadNetwork, s: SegmentId) -> CellRect {
    let (pa, pb) = roads.segment_ends(s);
    CellRect::from_corners(Cell::from_pos(pa), Cell::from_pos(pb))
}

fn plan_bulldoze(world: &World, rect: CellRect) -> Result<Plan, CommandError> {
    let rect = rect.clamp_to_map().ok_or(CommandError::OutOfBounds)?;
    // Street pieces with an end in the rectangle. Walk each segment once
    // (looking pieces up per cell scanned every segment per cell and
    // direction: seconds for a big selection on a dense map).
    let mut pieces = BTreeSet::new();
    let mut cells = BTreeSet::new();
    for (s, seg) in world.roads.segments.iter() {
        let bounds = segment_bounds(&world.roads, s);
        if seg.kind == RoadKind::Highway || !bounds.intersects(&rect) {
            continue;
        }
        if let Some(line) = segment_cells(&world.roads, s) {
            for w in line.windows(2) {
                let (p, q) = (w[0], w[1]);
                let (p_in, q_in) = (rect.contains(p), rect.contains(q));
                if p_in || q_in {
                    pieces.insert(ordered(p, q));
                }
                if p_in {
                    cells.insert(p);
                }
                if q_in {
                    cells.insert(q);
                }
            }
            continue;
        }
        // Not on cell centres (never built by commands): test each cell.
        let (pa, pb) = world.roads.segment_ends(s);
        let min = Cell::new(bounds.min.x.max(rect.min.x), bounds.min.y.max(rect.min.y));
        let max = Cell::new(bounds.max.x.min(rect.max.x), bounds.max.y.min(rect.max.y));
        for c in CellRect::from_corners(min, max).cells() {
            if !geom::on_segment(c.center(), pa, pb) {
                continue;
            }
            for (dx, dy) in DIRS8 {
                let q = c.offset(dx, dy);
                if geom::on_segment(q.center(), pa, pb) {
                    pieces.insert(ordered(c, q));
                    cells.insert(c);
                }
            }
        }
    }
    let mut buildings = Vec::new();
    for (id, b) in &world.buildings {
        if b.footprint.rect().intersects(&rect) {
            buildings.push(id);
            cells.extend(b.footprint.rect().cells());
        }
    }
    let cells: Vec<Cell> = cells.into_iter().collect();
    // Never refused for money: a player in debt must be able to demolish
    // roads to cut their upkeep. The small cost may push money negative.
    let cost = cells.len() as i64 * balance::BULLDOZE_COST_PER_CELL;
    Ok(Plan::Bulldoze {
        pieces: pieces.into_iter().collect(),
        buildings,
        cells,
        cost,
    })
}

fn plan_zone(world: &World, rect: CellRect, zone: Option<Zone>) -> Result<Plan, CommandError> {
    let rect = rect.clamp_to_map().ok_or(CommandError::OutOfBounds)?;
    Ok(zone_list(world, rect.cells().collect(), zone))
}

/// The eligible cells of a list.
fn zone_list(world: &World, cells: Vec<Cell>, zone: Option<Zone>) -> Plan {
    let mut cells: Vec<Cell> = cells
        .into_iter()
        .filter(|&c| {
            world.grid.get(c).is_some_and(|cd| {
                cd.occupant == Occupant::Empty
                    && cd.zone != zone
                    && (zone.is_none() || world.grid.near_road(c, balance::ZONE_ROAD_REACH))
            })
        })
        .collect();
    cells.sort_by_key(|c| (c.y, c.x));
    cells.dedup();
    Plan::Zone { cells, zone }
}

pub(crate) fn execute(world: &mut World, plan: Plan) -> Outcome {
    let preview = plan.preview();
    match plan {
        Plan::Road {
            cells,
            kind,
            new_pieces,
            changed,
            cost,
            ..
        } => {
            let roads = &mut world.roads;
            for &(p, q) in &new_pieces {
                let a = roads.ensure_node(p.center());
                let b = roads.ensure_node(q.center());
                roads.add_segment(a, b, kind);
            }
            if !changed.is_empty() {
                // Cut the road at both ends of the line, so every segment
                // carrying a changed piece lies inside the line.
                for c in [cells[0], cells[cells.len() - 1]] {
                    if roads.covers(c.center()) {
                        roads.ensure_node(c.center());
                    }
                }
                for &(p, q) in &changed {
                    if let Some(s) = roads.piece(p, q)
                        && roads.segments[s].kind != RoadKind::Highway
                    {
                        roads.segments[s].set_kind(kind);
                    }
                }
            }
            for &c in &cells {
                if let Some(n) = roads.node_at(c.center()) {
                    roads.simplify_node(n);
                }
            }
            let mut grid_changed = false;
            for &c in &cells {
                let cd = world.grid.get_mut(c).expect("validated in bounds");
                if cd.occupant != Occupant::Road || cd.zone.is_some() {
                    cd.occupant = Occupant::Road;
                    cd.zone = None;
                    grid_changed = true;
                }
            }
            world.economy.money -= cost;
            if !new_pieces.is_empty() || !changed.is_empty() {
                world.versions.roads += 1;
            }
            if grid_changed {
                world.versions.grid += 1;
            }
        }
        Plan::Bulldoze {
            pieces,
            buildings,
            cost,
            ..
        } => {
            remove_pieces(world, &pieces);
            for id in &buildings {
                if let Some(b) = world.buildings.remove(*id) {
                    for c in b.footprint.rect().cells() {
                        if let Some(cd) = world.grid.get_mut(c) {
                            cd.occupant = Occupant::Empty;
                        }
                    }
                }
                world.growth.meta.remove(*id);
            }
            // Residents of removed buildings leave, their workers lose the job.
            crate::households::buildings_removed(world, &buildings);
            world.economy.money -= cost;
            if !pieces.is_empty() {
                world.versions.roads += 1;
                world.versions.grid += 1;
            }
            if !buildings.is_empty() {
                world.versions.buildings += 1;
                world.versions.grid += 1;
            }
        }
        Plan::Zone { cells, zone } => {
            for &c in &cells {
                world.grid.get_mut(c).expect("in bounds").zone = zone;
            }
            if !cells.is_empty() {
                world.versions.grid += 1;
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

/// Remove unit road pieces (bulldoze), tidy the nodes at their ends and
/// clear the grid cells no road passes any more.
///
/// Each segment carrying removed pieces is cut once per run of removed
/// pieces, so the work is proportional to the road length touched (looking
/// nodes and segments up by position per piece scans the whole network:
/// seconds for a big selection on a dense map).
fn remove_pieces(world: &mut World, pieces: &[(Cell, Cell)]) {
    if pieces.is_empty() {
        return;
    }
    let doomed: BTreeSet<(Cell, Cell)> = pieces.iter().copied().collect();
    let area = pieces.iter().fold(CellRect::single(pieces[0].0), |r, &(p, q)| {
        CellRect::from_corners(
            Cell::new(r.min.x.min(p.x).min(q.x), r.min.y.min(p.y).min(q.y)),
            Cell::new(r.max.x.max(p.x).max(q.x), r.max.y.max(p.y).max(q.y)),
        )
    });
    let roads = &mut world.roads;
    let mut handled: BTreeSet<(Cell, Cell)> = BTreeSet::new();
    // Inner cells of removed runs: only that segment passed their centre
    // (any other road through a centre meets it at a node).
    let mut cleared: Vec<Cell> = Vec::new();
    // Nodes at the ends of removed runs.
    let mut ends: BTreeMap<Cell, NodeId> = BTreeMap::new();
    let ids: Vec<SegmentId> = roads.segments.keys().collect();
    for s in ids {
        if roads.segments[s].kind == RoadKind::Highway || !segment_bounds(roads, s).intersects(&area) {
            continue;
        }
        let Some(line) = segment_cells(roads, s) else { continue };
        let removed: Vec<bool> = line.windows(2).map(|w| doomed.contains(&ordered(w[0], w[1]))).collect();
        if !removed.contains(&true) {
            continue;
        }
        // `cur` runs from `line[k]` (node `start`) to the segment's far end.
        let (mut cur, mut start, mut k) = (s, roads.segments[s].a, 0);
        while k < removed.len() {
            let mut e = k + 1;
            while e < removed.len() && removed[e] == removed[k] {
                e += 1;
            }
            // Cut `cur` at line[e] unless that is its end; `cur` keeps the
            // part from line[k], the rest becomes `rest`.
            let (end, rest) = if e < removed.len() {
                let mid = roads.split_segment(cur, line[e].center());
                let rest = roads.nodes[mid]
                    .segments
                    .iter()
                    .copied()
                    .find(|&x| x != cur)
                    .expect("split adds the far half");
                (mid, Some(rest))
            } else {
                (roads.segments[cur].b, None)
            };
            if removed[k] {
                for w in line[k..=e].windows(2) {
                    handled.insert(ordered(w[0], w[1]));
                }
                cleared.extend_from_slice(&line[k + 1..e]);
                ends.insert(line[k], start);
                ends.insert(line[e], end);
                roads.remove_segment(cur);
            }
            if let Some(r) = rest {
                cur = r;
            }
            start = end;
            k = e;
        }
    }
    // Pieces of segments off the cell centres (never built by commands):
    // look them up by position.
    let mut touched: BTreeSet<Cell> = BTreeSet::new();
    for &(p, q) in doomed.difference(&handled) {
        let a = roads.ensure_node(p.center());
        let b = roads.ensure_node(q.center());
        if let Some(s) = roads.segment_between(a, b) {
            roads.remove_segment(s);
        }
        touched.extend([p, q]);
    }
    for (&c, &n) in &ends {
        let degree = roads.nodes.get(n).map(|node| node.segments.len());
        // A merged node's cell is still covered by the merged segment.
        let gone = roads.simplify_node(n);
        if gone && degree != Some(2) {
            cleared.push(c);
        }
    }
    for &c in &touched {
        if let Some(n) = roads.node_at(c.center()) {
            roads.simplify_node(n);
        }
    }
    cleared.extend(touched.iter().copied().filter(|&c| !roads.covers(c.center())));
    for c in cleared {
        let cd = world.grid.get_mut(c).expect("in bounds");
        if cd.occupant == Occupant::Road {
            cd.occupant = Occupant::Empty;
        }
    }
}
