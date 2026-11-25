//! What a construction tool would do, as pure functions of the world:
//! multi-leg road paths and block grids validated on a scratch copy of the
//! world (so a path that crosses itself or runs out of money midway is
//! judged exactly as the sim will judge it), road stretches between
//! junctions, frontage / block-fill zoning, bulldozing along roads, and an
//! honest undo for road building and zoning.
//!
//! Everything a tool sends is a list of ordinary `Command`s; a plan is only
//! sent when every one of them is accepted on the scratch world, so the
//! player never gets half a path.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use citysim_sim::{CELL_SIZE, Cell, CellRect, Command, CommandError, Occupant, RoadKind, World, Zone, balance};

use super::snap::RoadIndex;
use crate::shared::{PathLeg, RoadChoice, RoadStretch};

/// The command that builds one straight leg of the chosen kind.
pub fn road_command(from: Cell, to: Cell, _kind: RoadChoice) -> Command {
    Command::BuildRoad { from, to }
}

/// A copy of the parts of `world` that commands read and change (roads,
/// grid, buildings, money), without households or traffic, so applying
/// commands to it is cheap. Previews on it equal previews on `world`.
pub fn scratch(world: &World) -> World {
    let mut w = World::new(world.seed);
    w.roads = world.roads.clone();
    w.grid = world.grid.clone();
    w.buildings = world.buildings.clone();
    w.growth = world.growth.clone();
    w.economy = world.economy.clone();
    w.time = world.time.clone();
    w.versions = world.versions.clone();
    // Distinct from the fresh world's versions so its lazily built indices
    // are rebuilt for the copied roads and grid.
    w.versions.roads += 1;
    w.versions.grid += 1;
    w.versions.buildings += 1;
    w
}

/// Length of a leg in metres.
pub fn leg_length(from: Cell, to: Cell) -> f32 {
    let (dx, dy) = ((to.x - from.x).abs(), (to.y - from.y).abs());
    let steps = dx.max(dy) as f32;
    if dx != 0 && dy != 0 {
        steps * CELL_SIZE * std::f32::consts::SQRT_2
    } else {
        steps * CELL_SIZE
    }
}

/// Legs between consecutive points (zero-length ones dropped).
pub fn legs_of(points: &[Cell]) -> Vec<(Cell, Cell)> {
    points
        .windows(2)
        .filter(|w| w[0] != w[1])
        .map(|w| (w[0], w[1]))
        .collect()
}

/// A validated list of commands.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Evaluated {
    pub cmds: Vec<Command>,
    /// Per command: `None` when accepted (after the ones before it).
    pub errors: Vec<Option<CommandError>>,
    pub cost: i64,
    /// Cells reported by the accepted commands (deduplicated, sorted).
    pub cells: Vec<Cell>,
}

impl Evaluated {
    pub fn first_error(&self) -> Option<&CommandError> {
        self.errors.iter().flatten().next()
    }

    pub fn ok(&self) -> bool {
        !self.cmds.is_empty() && self.first_error().is_none()
    }
}

/// Apply `cmds` in order to a copy of `base`; returns the verdict and the
/// world after the accepted ones.
pub fn evaluate(base: &World, cmds: Vec<Command>) -> (Evaluated, World) {
    let mut w = base.clone();
    let mut errors = Vec::with_capacity(cmds.len());
    let mut cost = 0;
    let mut cells = BTreeSet::new();
    for &cmd in &cmds {
        match w.apply(cmd) {
            Ok(o) => {
                cost += o.cost;
                cells.extend(o.cells);
                errors.push(None);
            }
            Err(e) => errors.push(Some(e)),
        }
    }
    (
        Evaluated {
            cmds,
            errors,
            cost,
            cells: cells.into_iter().collect(),
        },
        w,
    )
}

/// The evaluated road path / block grid, for `PathPreview`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RoadPlan {
    pub eval: Evaluated,
    pub legs: Vec<PathLeg>,
    pub length_m: f32,
}

pub fn plan_roads(base: &World, legs: &[(Cell, Cell)], kind: RoadChoice) -> RoadPlan {
    let cmds = legs.iter().map(|&(a, b)| road_command(a, b, kind)).collect();
    let (eval, _) = evaluate(base, cmds);
    let legs = legs
        .iter()
        .zip(&eval.errors)
        .map(|(&(from, to), e)| PathLeg {
            from,
            to,
            error: e.clone(),
            upgrade_cells: Vec::new(),
        })
        .collect::<Vec<_>>();
    let length_m = legs.iter().map(|l| leg_length(l.from, l.to)).sum();
    RoadPlan { eval, legs, length_m }
}

// ---------------------------------------------------------------- block tool

/// Streets of a block grid over the rectangle `a`..`b`: its outline plus,
/// with `spacing`, inner streets every `spacing` cells (lines further than
/// that from the far edge keep the last block a little larger rather than
/// leaving a sliver). A one-cell-wide rectangle is a single street.
pub fn block_legs(a: Cell, b: Cell, spacing: Option<i32>) -> Vec<(Cell, Cell)> {
    let r = CellRect::from_corners(a, b);
    let (x0, y0, x1, y1) = (r.min.x, r.min.y, r.max.x, r.max.y);
    if x0 == x1 && y0 == y1 {
        return Vec::new();
    }
    if x0 == x1 || y0 == y1 {
        return vec![(r.min, r.max)];
    }
    let mut legs = vec![
        (Cell::new(x0, y0), Cell::new(x1, y0)),
        (Cell::new(x1, y0), Cell::new(x1, y1)),
        (Cell::new(x1, y1), Cell::new(x0, y1)),
        (Cell::new(x0, y1), Cell::new(x0, y0)),
    ];
    if let Some(s) = spacing.filter(|&s| s >= 2) {
        let inner = |lo: i32, hi: i32| {
            let mut v = Vec::new();
            let mut k = lo + s;
            while k <= hi - 2 {
                v.push(k);
                k += s;
            }
            v
        };
        for x in inner(x0, x1) {
            legs.push((Cell::new(x, y0), Cell::new(x, y1)));
        }
        for y in inner(y0, y1) {
            legs.push((Cell::new(x0, y), Cell::new(x1, y)));
        }
    }
    legs
}

/// The block tool's far corner: whole blocks of `spacing` from `start`
/// (at least one), so grids come out even.
pub fn block_corner(start: Cell, cursor: Cell, spacing: Option<i32>) -> Cell {
    let Some(s) = spacing.filter(|&s| s >= 2) else {
        return cursor;
    };
    let round = |d: i32| {
        let n = ((d.abs() as f32 / s as f32).round() as i32).max(1);
        d.signum() * n * s
    };
    let (dx, dy) = (cursor.x - start.x, cursor.y - start.y);
    let c = Cell::new(
        start.x + if dx == 0 { 0 } else { round(dx) },
        start.y + if dy == 0 { 0 } else { round(dy) },
    );
    // Pull back onto the map a block at a time.
    let mut c = c;
    while c.x < 0 {
        c.x += s;
    }
    while c.y < 0 {
        c.y += s;
    }
    while c.x >= citysim_sim::MAP_CELLS {
        c.x -= s;
    }
    while c.y >= citysim_sim::MAP_CELLS {
        c.y -= s;
    }
    c
}

// ---------------------------------------------------------------- road stretches

/// Road cell adjacency (unit pieces) from the lattice index.
#[derive(Debug, Clone, Default)]
pub struct RoadCells {
    pub adj: HashMap<Cell, Vec<Cell>>,
    /// Node degree per node cell (junctions have 3 or more).
    pub degree: HashMap<Cell, usize>,
    /// Cells on the highway stub.
    pub highway: BTreeSet<Cell>,
}

impl RoadCells {
    pub fn build(index: &RoadIndex) -> RoadCells {
        let mut adj: HashMap<Cell, Vec<Cell>> = HashMap::new();
        let mut highway = BTreeSet::new();
        for l in &index.lines {
            let cells = l.a.straight_line_to(l.b).unwrap_or_default();
            for w in cells.windows(2) {
                adj.entry(w[0]).or_default().push(w[1]);
                adj.entry(w[1]).or_default().push(w[0]);
            }
            if l.kind == RoadKind::Highway {
                highway.extend(cells);
            }
        }
        for v in adj.values_mut() {
            v.sort_by_key(|c| (c.x, c.y));
            v.dedup();
        }
        let degree = index.nodes.iter().map(|n| (n.cell, n.arms.len())).collect();
        RoadCells { adj, degree, highway }
    }

    pub fn is_road(&self, c: Cell) -> bool {
        self.adj.contains_key(&c)
    }

    pub fn is_junction(&self, c: Cell) -> bool {
        self.degree.get(&c).is_some_and(|&d| d >= 3)
    }

    fn neighbours(&self, c: Cell) -> &[Cell] {
        self.adj.get(&c).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// The stretch through road cell `c` between junctions or dead ends,
    /// as an ordered list of cells (ends included). `None` off the road or
    /// on a junction.
    pub fn stretch_cells(&self, c: Cell) -> Option<Vec<Cell>> {
        if !self.is_road(c) || self.is_junction(c) {
            return None;
        }
        let ns = self.neighbours(c).to_vec();
        let mut halves: Vec<Vec<Cell>> = Vec::new();
        let mut seen: BTreeSet<Cell> = BTreeSet::from([c]);
        for &first in ns.iter().take(2) {
            let mut run = Vec::new();
            let (mut prev, mut cur) = (c, first);
            loop {
                if !seen.insert(cur) {
                    break;
                }
                run.push(cur);
                if self.is_junction(cur) {
                    break;
                }
                let next = self
                    .neighbours(cur)
                    .iter()
                    .copied()
                    .find(|&n| n != prev && !seen.contains(&n));
                match next {
                    Some(n) => {
                        prev = cur;
                        cur = n;
                    }
                    None => break,
                }
            }
            halves.push(run);
        }
        let mut out: Vec<Cell> = halves.first().cloned().unwrap_or_default();
        out.reverse();
        out.push(c);
        if let Some(h) = halves.get(1) {
            out.extend(h);
        }
        Some(out)
    }

    /// Shortest run of road cells from `a` to `b` along the road (both
    /// included), searching at most `limit` cells.
    pub fn route(&self, a: Cell, b: Cell, limit: usize) -> Option<Vec<Cell>> {
        if !self.is_road(a) || !self.is_road(b) {
            return None;
        }
        let mut prev: HashMap<Cell, Cell> = HashMap::new();
        let mut q = VecDeque::from([a]);
        prev.insert(a, a);
        while let Some(c) = q.pop_front() {
            if c == b {
                let mut path = vec![b];
                let mut cur = b;
                while cur != a {
                    cur = prev[&cur];
                    path.push(cur);
                }
                path.reverse();
                return Some(path);
            }
            if prev.len() > limit {
                return None;
            }
            for &n in self.neighbours(c) {
                if let std::collections::hash_map::Entry::Vacant(e) = prev.entry(n) {
                    e.insert(c);
                    q.push_back(n);
                }
            }
        }
        None
    }
}

/// `RoadStretch` (for `WorldHighlight`) from an ordered cell run.
pub fn stretch_info(world: &World, cells: &[Cell]) -> RoadStretch {
    let roads = &world.roads;
    let mut segments = Vec::new();
    for w in cells.windows(2) {
        if let Some(s) = roads.piece(w[0], w[1])
            && segments.last() != Some(&s)
            && !segments.contains(&s)
        {
            segments.push(s);
        }
    }
    // Polyline: the ends and every change of direction.
    let mut points: Vec<[f32; 2]> = Vec::new();
    for (i, &c) in cells.iter().enumerate() {
        let turn = i == 0
            || i + 1 == cells.len()
            || (cells[i].x - cells[i - 1].x, cells[i].y - cells[i - 1].y)
                != (cells[i + 1].x - cells[i].x, cells[i + 1].y - cells[i].y);
        if turn {
            points.push(c.center());
        }
    }
    RoadStretch {
        segments,
        points,
        cells: cells.to_vec(),
    }
}

// ---------------------------------------------------------------- zoning

/// Which side of travel along `cells` the point `p` lies on: +1 or -1
/// (judged against the piece nearest to `p`).
pub fn side_of(cells: &[Cell], p: [f32; 2]) -> i32 {
    let mut best: Option<(f32, i32)> = None;
    for w in cells.windows(2) {
        let (a, b) = (w[0].center(), w[1].center());
        let d = [b[0] - a[0], b[1] - a[1]];
        let m = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
        let r = [p[0] - m[0], p[1] - m[1]];
        let dist = r[0] * r[0] + r[1] * r[1];
        let cross = d[0] * r[1] - d[1] * r[0];
        let s = if cross >= 0.0 { 1 } else { -1 };
        if best.is_none_or(|(bd, _)| dist < bd) {
            best = Some((dist, s));
        }
    }
    best.map(|(_, s)| s).unwrap_or(1)
}

/// Land beside the road run `cells` on `side` (+1 / -1 of travel), up to
/// `depth` cells from the road, not on a road. Sorted.
pub fn frontage(cells: &[Cell], side: i32, depth: i32, road: &RoadCells) -> Vec<Cell> {
    let mut out = BTreeSet::new();
    for w in cells.windows(2) {
        let (p, q) = (w[0], w[1]);
        let d = (q.x - p.x, q.y - p.y);
        let dd = d.0 * d.0 + d.1 * d.1;
        let len = (dd as f32).sqrt();
        for oy in -depth - 1..=depth + 1 {
            for ox in -depth - 1..=depth + 1 {
                let c = p.offset(ox, oy);
                let cr = d.0 * oy - d.1 * ox;
                if cr.signum() != side {
                    continue;
                }
                let dist = cr.abs() as f32 / len;
                let along = d.0 * ox + d.1 * oy;
                if dist <= depth as f32 + 0.01 && (0..=dd).contains(&along) && c.in_bounds() && !road.is_road(c) {
                    out.insert(c);
                }
            }
        }
    }
    out.into_iter().collect()
}

/// The enclosed block containing `start`: non-road cells reachable without
/// crossing a road (4-neighbourhood, so diagonal roads close a block too).
/// `None` when the area reaches the map edge or is larger than `limit`.
pub fn enclosed_block(start: Cell, road: &RoadCells, limit: usize) -> Option<Vec<Cell>> {
    if !start.in_bounds() || road.is_road(start) {
        return None;
    }
    let mut seen = BTreeSet::from([start]);
    let mut q = VecDeque::from([start]);
    while let Some(c) = q.pop_front() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let n = c.offset(dx, dy);
            if !n.in_bounds() {
                return None;
            }
            if road.is_road(n) || seen.contains(&n) {
                continue;
            }
            seen.insert(n);
            if seen.len() > limit {
                return None;
            }
            q.push_back(n);
        }
    }
    Some(seen.into_iter().collect())
}

/// Row runs of a cell set as rectangles (each holds only cells of the set).
pub fn row_runs(cells: &[Cell]) -> Vec<CellRect> {
    let mut sorted: Vec<Cell> = cells.to_vec();
    sorted.sort_by_key(|c| (c.y, c.x));
    sorted.dedup();
    let mut out = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1].y == start.y && sorted[i + 1].x == end.x + 1 {
            i += 1;
            end = sorted[i];
        }
        out.push(CellRect::from_corners(start, end));
        i += 1;
    }
    out
}

/// Zoning commands for exactly `cells`.
pub fn zone_commands(cells: &[Cell], zone: Option<Zone>) -> Vec<Command> {
    row_runs(cells)
        .into_iter()
        .map(|r| Command::SetZone {
            min: r.min,
            max: r.max,
            zone,
        })
        .collect()
}

/// Land depth zoned beside a road by the frontage tools.
pub const LOT_DEPTH: i32 = balance::ZONE_ROAD_REACH;

// ---------------------------------------------------------------- bulldozing along roads

/// Cells to bulldoze so that exactly the pieces of the run `cells` go,
/// without touching any other road: every cell of the run except junctions
/// (bulldozing a junction cell would cut the crossing road too). The run's
/// two ends stay unless they are dead ends, so a drag removes the road
/// between the two points.
pub fn bulldoze_run_cells(cells: &[Cell], road: &RoadCells, keep_ends: bool) -> Vec<Cell> {
    let n = cells.len();
    cells
        .iter()
        .enumerate()
        .filter(|&(i, &c)| {
            let end = i == 0 || i + 1 == n;
            let dead_end = road.degree.get(&c) == Some(&1);
            !road.is_junction(c) && !road.highway.contains(&c) && (!end || !keep_ends || dead_end)
        })
        .map(|(_, &c)| c)
        .collect()
}

/// Bulldoze commands for exactly `cells` (row and column runs merged).
pub fn bulldoze_commands(cells: &[Cell]) -> Vec<Command> {
    let set: BTreeSet<Cell> = cells.iter().copied().collect();
    let mut done: BTreeSet<Cell> = BTreeSet::new();
    let mut out = Vec::new();
    for &c in &set {
        if done.contains(&c) {
            continue;
        }
        // Grow along x, else along y.
        let mut ex = c;
        while set.contains(&ex.offset(1, 0)) && !done.contains(&ex.offset(1, 0)) {
            ex = ex.offset(1, 0);
        }
        let mut ey = c;
        while set.contains(&ey.offset(0, 1)) && !done.contains(&ey.offset(0, 1)) {
            ey = ey.offset(0, 1);
        }
        let end = if ex.x - c.x >= ey.y - c.y { ex } else { ey };
        for k in CellRect::from_corners(c, end).cells() {
            done.insert(k);
        }
        out.push(Command::Bulldoze { min: c, max: end });
    }
    out
}

/// Road cells that are road in `before` and not in `after`.
pub fn cleared_road_cells(before: &World, after: &World, cells: &[Cell]) -> Vec<Cell> {
    let is_road = |w: &World, c: Cell| w.grid.get(c).is_some_and(|cd| cd.occupant == Occupant::Road);
    let set: BTreeSet<Cell> = cells.iter().copied().collect();
    set.into_iter()
        .filter(|&c| is_road(before, c) && !is_road(after, c))
        .collect()
}

// ---------------------------------------------------------------- undo

/// Every unit road piece of the network (ordered cell pairs).
pub fn pieces(world: &World) -> BTreeSet<(Cell, Cell)> {
    let mut out = BTreeSet::new();
    for l in RoadIndex::build(world).lines {
        if let Some(cells) = l.a.straight_line_to(l.b) {
            for w in cells.windows(2) {
                out.insert(if w[0] <= w[1] { (w[0], w[1]) } else { (w[1], w[0]) });
            }
        }
    }
    out
}

/// How to take back one construction action.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoRecord {
    /// Roads built: bulldoze `cells` (exactly the new pieces), then give
    /// back the zoning the road cleared.
    Roads {
        new_pieces: BTreeSet<(Cell, Cell)>,
        cells: Vec<Cell>,
        rezone: Vec<(Cell, Zone)>,
        label: String,
    },
    /// Zoning changed: put each cell's previous zone back.
    Zones {
        restore: Vec<(Cell, Option<Zone>)>,
        label: String,
    },
}

impl UndoRecord {
    pub fn label(&self) -> &str {
        match self {
            UndoRecord::Roads { label, .. } | UndoRecord::Zones { label, .. } => label,
        }
    }
}

/// Undo record for building roads: `before` and `after` are the world
/// before and after the action. `Err` with the reason when the new road
/// cannot be removed without touching roads that were there before (a
/// one-cell link between two existing roads).
pub fn undo_for_roads(before: &World, after: &World, label: String) -> Result<UndoRecord, String> {
    let (old, new) = (pieces(before), pieces(after));
    let new_pieces: BTreeSet<(Cell, Cell)> = new.difference(&old).copied().collect();
    if new_pieces.is_empty() {
        return Err("nothing new was built".into());
    }
    let mut touching: BTreeMap<Cell, (usize, usize)> = BTreeMap::new();
    for &(p, q) in &new {
        let fresh = new_pieces.contains(&(p, q));
        for c in [p, q] {
            let e = touching.entry(c).or_default();
            if fresh {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
    }
    let cells: Vec<Cell> = touching
        .iter()
        .filter(|(_, (fresh, old))| *fresh > 0 && *old == 0)
        .map(|(&c, _)| c)
        .collect();
    let set: BTreeSet<Cell> = cells.iter().copied().collect();
    if new_pieces.iter().any(|(p, q)| !set.contains(p) && !set.contains(q)) {
        return Err("a new piece joins two roads that were already there".into());
    }
    let rezone = cells
        .iter()
        .filter_map(|&c| {
            let z = before.grid.get(c)?.zone?;
            Some((c, z))
        })
        .collect();
    let rec = UndoRecord::Roads {
        new_pieces,
        cells,
        rezone,
        label,
    };
    // Prove it on the after-world.
    check_undo(after, &rec)?;
    Ok(rec)
}

/// Commands that undo `rec` on `world`, or why it cannot be undone any
/// more (the roads or zoning changed since). Road undo bulldozes: it costs
/// the usual demolition fee and nothing is refunded.
pub fn undo_commands(world: &World, rec: &UndoRecord) -> Result<Vec<Command>, String> {
    match rec {
        UndoRecord::Roads { cells, rezone, .. } => {
            check_undo(world, rec)?;
            let mut cmds = bulldoze_commands(cells);
            let mut by_zone: BTreeMap<u8, (Zone, Vec<Cell>)> = BTreeMap::new();
            for &(c, z) in rezone {
                by_zone.entry(z as u8).or_insert((z, Vec::new())).1.push(c);
            }
            for (_, (z, cs)) in by_zone {
                cmds.extend(zone_commands(&cs, Some(z)));
            }
            Ok(cmds)
        }
        UndoRecord::Zones { restore, .. } => {
            let mut by_zone: BTreeMap<Option<u8>, (Option<Zone>, Vec<Cell>)> = BTreeMap::new();
            for &(c, z) in restore {
                let now = world.grid.get(c).map(|cd| (cd.zone, cd.occupant));
                if now.is_some_and(|(zn, occ)| zn != z && occ == Occupant::Empty) {
                    by_zone.entry(z.map(|z| z as u8)).or_insert((z, Vec::new())).1.push(c);
                }
            }
            if by_zone.is_empty() {
                return Err("the zoning has changed since (built on or rezoned)".into());
            }
            Ok(by_zone
                .into_values()
                .flat_map(|(z, cs)| zone_commands(&cs, z))
                .collect())
        }
    }
}

fn check_undo(world: &World, rec: &UndoRecord) -> Result<(), String> {
    let UndoRecord::Roads { new_pieces, cells, .. } = rec else {
        return Ok(());
    };
    let now = pieces(world);
    if !new_pieces.is_subset(&now) {
        return Err("part of that road is already gone".into());
    }
    let base = scratch(world);
    let (eval, after) = evaluate(&base, bulldoze_commands(cells));
    if !eval.ok() {
        return Err("the sim would refuse it".into());
    }
    let want: BTreeSet<(Cell, Cell)> = now.difference(new_pieces).copied().collect();
    if pieces(&after) != want {
        return Err("roads built since are in the way".into());
    }
    Ok(())
}

/// Undo record for a zoning action: each changed cell's previous zone.
pub fn undo_for_zones(before: &World, changed: &[Cell], label: String) -> UndoRecord {
    UndoRecord::Zones {
        restore: changed
            .iter()
            .filter_map(|&c| Some((c, before.grid.get(c)?.zone)))
            .collect(),
        label,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rich() -> World {
        let mut w = World::new(1);
        w.economy.money = 10_000_000;
        w
    }

    fn build(w: &mut World, legs: &[(Cell, Cell)]) {
        for &(a, b) in legs {
            w.apply(Command::BuildRoad { from: a, to: b }).expect("road");
        }
    }

    const STUB_END: Cell = Cell::new(11, 128);

    #[test]
    fn path_is_judged_leg_after_leg() {
        let w = rich();
        let base = scratch(&w);
        let pts = [STUB_END, Cell::new(20, 128), Cell::new(20, 120), Cell::new(28, 120)];
        let plan = plan_roads(&base, &legs_of(&pts), RoadChoice::Street);
        assert!(plan.eval.ok());
        assert_eq!(plan.eval.cost, 25 * balance::ROAD_COST_PER_CELL);
        assert_eq!(plan.length_m, 25.0 * CELL_SIZE);
        // A path that doubles back over itself pays for each piece once.
        let pts = [STUB_END, Cell::new(20, 128), Cell::new(15, 128)];
        let plan = plan_roads(&base, &legs_of(&pts), RoadChoice::Street);
        assert_eq!(plan.eval.cost, 9 * balance::ROAD_COST_PER_CELL);
    }

    #[test]
    fn path_that_crosses_itself_diagonally_is_refused_at_that_leg() {
        let base = scratch(&rich());
        let pts = [
            Cell::new(30, 30),
            Cell::new(34, 34),
            Cell::new(34, 33),
            Cell::new(33, 34),
        ];
        let plan = plan_roads(&base, &legs_of(&pts), RoadChoice::Street);
        assert!(plan.legs[0].error.is_none());
        assert!(matches!(plan.legs[2].error, Some(CommandError::DiagonalCrossing(_))));
        assert!(!plan.eval.ok());
    }

    #[test]
    fn path_running_out_of_money_midway_is_refused() {
        let mut w = World::new(1);
        w.economy.money = 20 * balance::ROAD_COST_PER_CELL;
        let base = scratch(&w);
        let pts = [STUB_END, Cell::new(26, 128), Cell::new(26, 120)];
        let plan = plan_roads(&base, &legs_of(&pts), RoadChoice::Street);
        assert!(plan.legs[0].error.is_none());
        assert!(matches!(
            plan.legs[1].error,
            Some(CommandError::InsufficientFunds { .. })
        ));
    }

    #[test]
    fn scratch_previews_equal_the_real_world() {
        let mut w = rich();
        build(&mut w, &[(STUB_END, Cell::new(30, 128))]);
        w.apply(Command::SetZone {
            min: Cell::new(12, 126),
            max: Cell::new(30, 127),
            zone: Some(Zone::Residential),
        })
        .unwrap();
        let s = scratch(&w);
        for cmd in [
            Command::BuildRoad {
                from: Cell::new(20, 120),
                to: Cell::new(20, 135),
            },
            Command::Bulldoze {
                min: Cell::new(14, 128),
                max: Cell::new(16, 128),
            },
            Command::SetZone {
                min: Cell::new(10, 120),
                max: Cell::new(40, 140),
                zone: Some(Zone::Commercial),
            },
        ] {
            assert_eq!(w.preview(&cmd), s.preview(&cmd), "{cmd:?}");
        }
    }

    #[test]
    fn block_grid_has_outline_and_inner_streets() {
        let legs = block_legs(Cell::new(10, 10), Cell::new(20, 25), Some(5));
        assert_eq!(legs.len(), 4 + 1 + 2, "{legs:?}");
        assert!(legs.contains(&(Cell::new(15, 10), Cell::new(15, 25))));
        assert!(legs.contains(&(Cell::new(10, 15), Cell::new(20, 15))));
        assert!(legs.contains(&(Cell::new(10, 20), Cell::new(20, 20))));
        assert_eq!(block_legs(Cell::new(1, 1), Cell::new(1, 9), Some(5)).len(), 1);
        assert!(block_legs(Cell::new(1, 1), Cell::new(1, 1), None).is_empty());
        assert_eq!(
            block_corner(Cell::new(10, 10), Cell::new(22, 7), Some(5)),
            Cell::new(20, 5)
        );
        assert_eq!(
            block_corner(Cell::new(10, 10), Cell::new(11, 10), Some(5)),
            Cell::new(15, 10)
        );
        // A block grid builds as one valid plan.
        let plan = plan_roads(
            &scratch(&rich()),
            &block_legs(Cell::new(40, 40), Cell::new(50, 50), Some(5)),
            RoadChoice::Street,
        );
        assert!(plan.eval.ok());
        assert_eq!(plan.eval.cost, (4 * 10 + 2 * 10) * balance::ROAD_COST_PER_CELL);
    }

    fn town() -> World {
        let mut w = rich();
        build(
            &mut w,
            &[
                (STUB_END, Cell::new(20, 128)),
                (Cell::new(20, 128), Cell::new(20, 120)),
                (Cell::new(20, 120), Cell::new(30, 120)),
                (Cell::new(30, 120), Cell::new(30, 128)),
                (Cell::new(30, 128), Cell::new(20, 128)),
                (Cell::new(30, 128), Cell::new(40, 128)),
            ],
        );
        w
    }

    #[test]
    fn stretches_run_between_junctions() {
        let w = town();
        let rc = RoadCells::build(&RoadIndex::build(&w));
        // (20, 128) and (30, 128) are junctions; the loop's top is one stretch
        // from (20, 128) round the corners to (30, 128).
        let s = rc.stretch_cells(Cell::new(25, 120)).unwrap();
        assert_eq!(s.first(), Some(&Cell::new(20, 128)));
        assert_eq!(s.last(), Some(&Cell::new(30, 128)));
        assert_eq!(s.len(), 9 + 10 + 8);
        assert!(rc.stretch_cells(Cell::new(20, 128)).is_none(), "junction");
        let east = rc.stretch_cells(Cell::new(35, 128)).unwrap();
        assert_eq!(
            (east[0], *east.last().unwrap()),
            (Cell::new(30, 128), Cell::new(40, 128))
        );
        let r = rc.route(Cell::new(25, 120), Cell::new(35, 128), 500).unwrap();
        assert_eq!(r.first(), Some(&Cell::new(25, 120)));
        assert_eq!(r.last(), Some(&Cell::new(35, 128)));
    }

    #[test]
    fn frontage_and_block_fill() {
        let w = town();
        let rc = RoadCells::build(&RoadIndex::build(&w));
        let east = rc.stretch_cells(Cell::new(35, 128)).unwrap();
        // North of the eastbound road (y < 128): side of a point above it.
        let side = side_of(&east, [35.5 * CELL_SIZE, 125.0 * CELL_SIZE]);
        let f = frontage(&east, side, LOT_DEPTH, &rc);
        assert!(f.iter().all(|c| c.y == 126 || c.y == 127), "{f:?}");
        assert!(f.contains(&Cell::new(35, 126)) && f.contains(&Cell::new(40, 127)));
        assert!(!f.iter().any(|c| c.x < 30));
        // The loop's inside is an enclosed block of 9 x 7 cells.
        let inside = enclosed_block(Cell::new(25, 125), &rc, 5000).unwrap();
        assert_eq!(inside.len(), 9 * 7);
        assert!(enclosed_block(Cell::new(60, 60), &rc, 5000).is_none(), "open land");
        let cmds = zone_commands(&inside, Some(Zone::Residential));
        assert_eq!(cmds.len(), 7);
        let (eval, _) = evaluate(&scratch(&w), cmds);
        // Within two cells of a road: all but the 5 x 3 middle.
        assert_eq!(eval.cells.len(), 9 * 7 - 5 * 3);
    }

    #[test]
    fn bulldozing_a_stretch_leaves_other_roads_alone() {
        let w = town();
        let rc = RoadCells::build(&RoadIndex::build(&w));
        let s = rc.stretch_cells(Cell::new(25, 120)).unwrap();
        let cells = bulldoze_run_cells(&s, &rc, false);
        let (eval, after) = evaluate(&scratch(&w), bulldoze_commands(&cells));
        assert!(eval.ok());
        let gone: BTreeSet<_> = pieces(&w).difference(&pieces(&after)).copied().collect();
        assert_eq!(gone.len(), s.len() - 1);
        // Everything still there except that stretch.
        assert!(!pieces(&after).contains(&(Cell::new(20, 127), Cell::new(20, 128))));
        assert!(pieces(&after).contains(&(Cell::new(29, 128), Cell::new(30, 128))));
        // A partial drag between two points keeps the points.
        let run = rc.route(Cell::new(22, 120), Cell::new(26, 120), 100).unwrap();
        let cells = bulldoze_run_cells(&run, &rc, true);
        assert_eq!(cells, vec![Cell::new(23, 120), Cell::new(24, 120), Cell::new(25, 120)]);
    }

    #[test]
    fn undo_removes_exactly_the_new_road() {
        let mut w = town();
        w.apply(Command::SetZone {
            min: Cell::new(31, 126),
            max: Cell::new(50, 127),
            zone: Some(Zone::Commercial),
        })
        .unwrap();
        let before = w.clone();
        // A T off the existing road, with a corner, through zoned land.
        build(
            &mut w,
            &[
                (Cell::new(35, 128), Cell::new(45, 118)),
                (Cell::new(45, 118), Cell::new(45, 110)),
            ],
        );
        let rec = undo_for_roads(&before, &w, "road".into()).unwrap();
        let cmds = undo_commands(&w, &rec).unwrap();
        for c in cmds {
            w.apply(c).unwrap();
        }
        assert_eq!(pieces(&w), pieces(&before));
        assert_eq!(
            before.grid.get(Cell::new(37, 126)).unwrap().zone,
            Some(Zone::Commercial)
        );
        assert_eq!(w.grid.get(Cell::new(37, 126)).unwrap().zone, Some(Zone::Commercial));
        w.check_invariants().unwrap();
        // A one-piece link between two old roads cannot be taken back alone.
        let mut w = town();
        build(
            &mut w,
            &[
                (Cell::new(20, 110), Cell::new(30, 110)),
                (Cell::new(20, 111), Cell::new(30, 111)),
            ],
        );
        let before = w.clone();
        build(&mut w, &[(Cell::new(25, 110), Cell::new(25, 111))]);
        assert!(undo_for_roads(&before, &w, "x".into()).is_err());
    }

    #[test]
    fn row_runs_cover_exactly_the_cells() {
        let cells = vec![Cell::new(1, 1), Cell::new(2, 1), Cell::new(4, 1), Cell::new(1, 2)];
        let runs = row_runs(&cells);
        assert_eq!(runs.len(), 3);
        let n: i32 = runs.iter().map(|r| r.width() * r.height()).sum();
        assert_eq!(n, 4);
    }
}
