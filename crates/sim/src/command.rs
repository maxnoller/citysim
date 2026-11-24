//! Player commands: validation, cost, preview and execution.
//!
//! Every command goes through [`plan`], which validates against the current
//! state without mutating it. `World::preview` returns the plan's summary;
//! `World::apply` executes the plan. So `preview(c).is_ok()` exactly when
//! `apply(c)` would succeed, and both report the same cost and cells.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::geom::{Cell, CellRect};
use crate::grid::{Occupant, Zone};
use crate::roads::RoadKind;
use crate::{BuildingId, World, balance};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Straight road between two cell centres (horizontal, vertical or 45°).
    BuildRoad { from: Cell, to: Cell },
    /// Remove road pieces touching cells in the rectangle, and buildings
    /// overlapping it. The highway stub is not removable.
    Bulldoze { min: Cell, max: Cell },
    /// Zone (or with `None`, unzone) the eligible cells in the rectangle.
    SetZone { min: Cell, max: Cell, zone: Option<Zone> },
    /// Fraction in `0.0..=balance::MAX_TAX_RATE`.
    SetTaxRate(f32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandError {
    /// A cell (or the whole rectangle) lies outside the map.
    OutOfBounds,
    /// Road start and end are the same cell.
    ZeroLength,
    /// Road is not horizontal, vertical or exactly diagonal.
    NotStraight,
    /// A building stands on this cell of the road.
    BlockedByBuilding(Cell),
    /// The diagonal step starting at this cell would cross another diagonal
    /// road between cell centres.
    DiagonalCrossing(Cell),
    InsufficientFunds {
        cost: i64,
        money: i64,
    },
    InvalidTaxRate(f32),
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
        }
    }
}

impl std::error::Error for CommandError {}

/// What a command would do. Returned by `World::preview`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Preview {
    /// Money that will be spent.
    pub cost: i64,
    /// Cells the command affects: for roads every cell of the line, for
    /// zoning the cells that get (un)zoned, for bulldoze the cells cleared.
    pub cells: Vec<Cell>,
}

/// What a command did. Returned by `World::apply`; equals the preview.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Outcome {
    pub cost: i64,
    pub cells: Vec<Cell>,
}

/// A validated command, ready to execute.
pub(crate) enum Plan {
    Road {
        cells: Vec<Cell>,
        /// Unit pieces (adjacent cell pairs) not yet built.
        new_pieces: Vec<(Cell, Cell)>,
        cost: i64,
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
}

impl Plan {
    pub(crate) fn preview(&self) -> Preview {
        match self {
            Plan::Road { cells, cost, .. } | Plan::Bulldoze { cells, cost, .. } => Preview {
                cost: *cost,
                cells: cells.clone(),
            },
            Plan::Zone { cells, .. } => Preview {
                cost: 0,
                cells: cells.clone(),
            },
            Plan::Tax(_) => Preview::default(),
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

pub(crate) fn plan(world: &World, cmd: &Command) -> Result<Plan, CommandError> {
    match *cmd {
        Command::BuildRoad { from, to } => plan_road(world, from, to),
        Command::Bulldoze { min, max } => plan_bulldoze(world, CellRect::from_corners(min, max)),
        Command::SetZone { min, max, zone } => plan_zone(world, CellRect::from_corners(min, max), zone),
        Command::SetTaxRate(r) => {
            if r.is_finite() && (0.0..=balance::MAX_TAX_RATE).contains(&r) {
                Ok(Plan::Tax(r))
            } else {
                Err(CommandError::InvalidTaxRate(r))
            }
        }
    }
}

fn plan_road(world: &World, from: Cell, to: Cell) -> Result<Plan, CommandError> {
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
    for w in cells.windows(2) {
        let (p, q) = (w[0], w[1]);
        if world.roads.piece(p, q).is_some() {
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
    let cost = new_pieces.len() as i64 * balance::ROAD_COST_PER_CELL;
    afford(world, cost)?;
    Ok(Plan::Road {
        cells,
        new_pieces,
        cost,
    })
}

fn plan_bulldoze(world: &World, rect: CellRect) -> Result<Plan, CommandError> {
    let rect = rect.clamp_to_map().ok_or(CommandError::OutOfBounds)?;
    let mut pieces = BTreeSet::new();
    let mut cells = BTreeSet::new();
    for c in rect.cells() {
        if !world.grid.is_road(c) {
            continue;
        }
        for (dx, dy) in DIRS8 {
            let q = c.offset(dx, dy);
            if let Some(s) = world.roads.piece(c, q)
                && world.roads.segments[s].kind != RoadKind::Highway
            {
                pieces.insert(ordered(c, q));
                cells.insert(c);
            }
        }
    }
    let mut buildings = Vec::new();
    for (id, b) in &world.buildings {
        if b.footprint.intersects(&rect) {
            buildings.push(id);
            cells.extend(b.footprint.cells());
        }
    }
    let cells: Vec<Cell> = cells.into_iter().collect();
    let cost = cells.len() as i64 * balance::BULLDOZE_COST_PER_CELL;
    afford(world, cost)?;
    Ok(Plan::Bulldoze {
        pieces: pieces.into_iter().collect(),
        buildings,
        cells,
        cost,
    })
}

fn plan_zone(world: &World, rect: CellRect, zone: Option<Zone>) -> Result<Plan, CommandError> {
    let rect = rect.clamp_to_map().ok_or(CommandError::OutOfBounds)?;
    let cells = rect
        .cells()
        .filter(|&c| {
            let cd = world.grid.get(c).expect("clamped");
            cd.occupant == Occupant::Empty
                && cd.zone != zone
                && (zone.is_none() || world.grid.near_road(c, balance::ZONE_ROAD_REACH))
        })
        .collect();
    Ok(Plan::Zone { cells, zone })
}

pub(crate) fn execute(world: &mut World, plan: Plan) -> Outcome {
    let preview = plan.preview();
    match plan {
        Plan::Road {
            cells,
            new_pieces,
            cost,
        } => {
            let roads = &mut world.roads;
            for &(p, q) in &new_pieces {
                let a = roads.ensure_node(p.center());
                let b = roads.ensure_node(q.center());
                roads.add_segment(a, b, RoadKind::Street);
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
            if !new_pieces.is_empty() {
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
            let roads = &mut world.roads;
            for &(p, q) in &pieces {
                let a = roads.ensure_node(p.center());
                let b = roads.ensure_node(q.center());
                if let Some(s) = roads.segment_between(a, b) {
                    roads.remove_segment(s);
                }
            }
            let touched: BTreeSet<Cell> = pieces.iter().flat_map(|&(p, q)| [p, q]).collect();
            for &c in &touched {
                if let Some(n) = roads.node_at(c.center()) {
                    roads.simplify_node(n);
                }
            }
            for &c in &touched {
                let covered = roads.covers(c.center());
                let cd = world.grid.get_mut(c).expect("in bounds");
                if !covered && cd.occupant == Occupant::Road {
                    cd.occupant = Occupant::Empty;
                }
            }
            for id in &buildings {
                if let Some(b) = world.buildings.remove(*id) {
                    for c in b.footprint.cells() {
                        if let Some(cd) = world.grid.get_mut(c) {
                            cd.occupant = Occupant::Empty;
                        }
                    }
                }
            }
            // PHASE2: households whose home/job disappeared must react.
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
    }
    Outcome {
        cost: preview.cost,
        cells: preview.cells,
    }
}
