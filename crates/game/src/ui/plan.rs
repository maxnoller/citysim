//! Plans: what a tool would do, as a list of operations checked in order on
//! a scratch copy of the world, and undo records. A plan is sent only when
//! every operation is accepted, so a road is never built in part.
//!
//! Operations on parts of existing roads name the part by its centre line
//! ([`Op::Remove`], [`Op::Kind`]) and are resolved to the segment there when
//! their turn comes: an earlier operation of the same plan may have split
//! or merged segments. Resolving on the scratch copy and on the real world
//! gives the same commands (the scratch copy has the same road graph).

use std::collections::BTreeMap;

use citysim_sim::command::{NodeChange, PieceChange};
use citysim_sim::{BuildingId, BuildingRecord, CellRef, Command, CommandError, Curve, Preview, RoadKind, World, Zone};

use super::fsnap::dist;

/// A copy of the parts of `world` that commands read and change (roads,
/// zoning, buildings, money), without people or traffic, so applying
/// commands to it is cheap. Previews on it equal previews on `world`.
pub fn scratch(world: &World) -> World {
    let mut w = World::new(world.seed);
    w.roads = world.roads.clone();
    w.zoning = world.zoning.clone();
    w.buildings = world.buildings.clone();
    w.growth = world.growth.clone();
    w.economy = world.economy.clone();
    w.time = world.time.clone();
    w.versions = world.versions.clone();
    // Distinct from the fresh world's versions so its lazily built indices
    // are rebuilt for the copied roads and zoning.
    w.versions.roads += 1;
    w.versions.grid += 1;
    w.versions.buildings += 1;
    w
}

/// One step of a plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Cmd(Command),
    /// Remove the part of a road along this centre line.
    Remove(Curve),
    /// Change the part of a road along this centre line to a kind.
    Kind(Curve, RoadKind),
    /// Remove every segment whose serial is in `lo..hi` (undo of new road).
    RemoveSerials(u64, u64),
    /// Zone the free, unzoned cells at these centres (undo: the zoning a
    /// road's land took, back before the buildings it took).
    ZoneAt(Vec<[f32; 2]>, Zone),
}

/// The tools' new road: a road along `curve` that takes down the grown
/// buildings in its way (`Command::Clearing`), as the bulldozer would.
pub fn road_op(curve: Curve, kind: RoadKind) -> Op {
    Op::Cmd(Command::Clearing(Box::new(Command::BuildCurve { curve, kind })))
}

/// The curve of a road-building operation (clearing its way or not).
pub fn road_curve(op: &Op) -> Option<Curve> {
    match op {
        Op::Cmd(Command::BuildCurve { curve, .. }) => Some(*curve),
        Op::Cmd(Command::Clearing(c)) => match &**c {
            Command::BuildCurve { curve, .. } => Some(*curve),
            _ => None,
        },
        _ => None,
    }
}

/// How far (metres) a cell's centre may be from a recorded one and still be
/// the same land (as `RestoreBuilding` matches lots).
const SAME_LAND_M: f32 = 0.3;

/// The segment carrying the centre line `c` and the range of it, metres
/// from its `a` node.
fn part_of(w: &World, c: &Curve) -> Option<(citysim_sim::SegmentId, f32, f32)> {
    let mid = c.point_at(c.length() * 0.5, 0.0);
    let (s, _, _) = w.roads.nearest_segment(mid, 0.6)?;
    let seg = &w.roads.segments[s];
    let (d0, e0, _) = seg.nearest(c.start());
    let (d1, e1, _) = seg.nearest(c.end());
    if e0 > 1.0 || e1 > 1.0 {
        return None;
    }
    let (a, b) = (d0.min(d1), d0.max(d1));
    let snap = |v: f32| {
        if v < 0.3 {
            0.0
        } else if v > seg.length - 0.3 {
            seg.length
        } else {
            v
        }
    };
    Some((s, snap(a), snap(b)))
}

/// Kind of the road under the middle of `c` in `w`.
pub fn kind_along(w: &World, c: &Curve) -> Option<RoadKind> {
    let mid = c.point_at(c.length() * 0.5, 0.0);
    let (s, _, _) = w.roads.nearest_segment(mid, 0.6)?;
    Some(w.roads.segments[s].kind)
}

/// The command an operation stands for in `w` now.
pub fn resolve(w: &World, op: &Op) -> Result<Command, CommandError> {
    match op {
        Op::Cmd(c) => Ok(c.clone()),
        Op::Remove(c) => {
            let (segment, from_m, to_m) = part_of(w, c).ok_or(CommandError::NoSuchRoad)?;
            Ok(Command::RemoveRoad { segment, from_m, to_m })
        }
        Op::Kind(c, kind) => {
            // Clearing its way: a kind that needs a wider reserve takes the
            // grown buildings in it down (none today: every kind keeps the
            // same reserve).
            let (segment, from_m, to_m) = part_of(w, c).ok_or(CommandError::NoSuchRoad)?;
            Ok(Command::Clearing(Box::new(Command::SetRoadKind {
                segment,
                from_m,
                to_m,
                kind: *kind,
            })))
        }
        Op::ZoneAt(points, zone) => {
            let mut cells: Vec<CellRef> = points
                .iter()
                .filter_map(|&p| {
                    let c = w.zoning.cell_at(p)?;
                    let x = w.zoning.cell(c)?;
                    let same = w.zoning.cell_center(c).is_some_and(|q| dist(p, q) < SAME_LAND_M);
                    (same && x.usable() && x.zone.is_none() && x.building.is_none()).then_some(c)
                })
                .collect();
            cells.sort();
            cells.dedup();
            Ok(Command::ZoneCells {
                cells,
                zone: Some(*zone),
            })
        }
        Op::RemoveSerials(..) => Err(CommandError::NoSuchRoad),
    }
}

/// One checked step.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// The operation it came from (index into the plan).
    pub op: usize,
    /// The command it resolved to (`None` when it could not be resolved).
    pub cmd: Option<Command>,
    pub result: Result<Preview, CommandError>,
    /// Kinds of the roads a removal takes away (parallel to
    /// `Preview::removed`).
    pub removed_kinds: Vec<RoadKind>,
}

/// A plan checked step by step.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Evaluated {
    pub steps: Vec<Step>,
    pub cost: i64,
    /// Metres of new road.
    pub length_m: f32,
}

impl Evaluated {
    pub fn first_error(&self) -> Option<&CommandError> {
        self.steps.iter().find_map(|s| s.result.as_ref().err())
    }

    #[cfg(test)]
    pub fn ok(&self) -> bool {
        self.first_error().is_none()
    }

    /// The commands to send (only meaningful when `ok`).
    #[cfg(test)]
    pub fn cmds(&self) -> Vec<Command> {
        self.steps.iter().filter_map(|s| s.cmd.clone()).collect()
    }

    fn previews(&self) -> impl Iterator<Item = &Preview> {
        self.steps.iter().filter_map(|s| s.result.as_ref().ok())
    }

    /// Road pieces as they will be.
    pub fn pieces(&self) -> Vec<(Curve, PieceChange)> {
        self.previews()
            .flat_map(|p| p.pieces.iter().map(|pc| (pc.curve, pc.change)))
            .collect()
    }

    /// Junctions made or used (one per place).
    pub fn nodes(&self) -> Vec<([f32; 2], NodeChange)> {
        let mut out: Vec<([f32; 2], NodeChange)> = Vec::new();
        for (p, c) in self.previews().flat_map(|p| p.nodes.iter().map(|n| (n.pos, n.change))) {
            // A later piece starting where an earlier one ended: one node.
            if let Some(o) = out.iter_mut().find(|o| dist(o.0, p) < 0.5) {
                if o.1 == NodeChange::End {
                    o.1 = c;
                }
            } else {
                out.push((p, c));
            }
        }
        out
    }

    /// Zoning cells that change.
    pub fn zone_cells(&self) -> Vec<CellRef> {
        let mut out: Vec<CellRef> = self.previews().flat_map(|p| p.zone_cells.iter().copied()).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Roads removed, with their kinds.
    pub fn removed(&self) -> Vec<(Curve, RoadKind)> {
        self.steps
            .iter()
            .filter_map(|s| s.result.as_ref().ok().map(|p| (p, &s.removed_kinds)))
            .flat_map(|(p, k)| p.removed.iter().copied().zip(k.iter().copied()))
            .collect()
    }

    /// What the road commands pay for road (not for the buildings they
    /// take down), by command: `Command::Refund` gives it back.
    pub fn road_payments(&self) -> Vec<i64> {
        self.previews().map(|p| p.road_cost).filter(|&c| c > 0).collect()
    }

    /// Buildings demolished (bulldozed, or in the way of a road).
    pub fn buildings(&self) -> Vec<BuildingId> {
        let mut out: Vec<BuildingId> = self.previews().flat_map(|p| p.buildings.iter().copied()).collect();
        out.sort();
        out.dedup();
        out
    }

    fn push_step(&mut self, op: usize, cmd: Command, res: Result<Preview, CommandError>, kinds: Vec<RoadKind>) {
        if let Ok(p) = &res {
            self.cost += p.cost;
            self.length_m += p.length_m;
        }
        self.steps.push(Step {
            op,
            cmd: Some(cmd),
            result: res,
            removed_kinds: kinds,
        });
    }
}

/// Kinds of the roads `cmd` would remove from `w`, in `Preview::removed`
/// order.
fn removed_kinds(w: &World, cmd: &Command) -> Vec<RoadKind> {
    match cmd {
        Command::RemoveRoad { .. }
        | Command::RemoveRoads { .. }
        | Command::BulldozeArea { .. }
        | Command::Bulldoze { .. } => w
            .preview(cmd)
            .map(|p| {
                p.removed
                    .iter()
                    .map(|c| kind_along(w, c).unwrap_or(RoadKind::Street))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Segments an undo takes away per command (`Command::RemoveRoads`).
const REMOVE_BATCH: usize = 32;

/// Check operation `i` on `w` (applying what is accepted), adding its
/// steps to `steps`.
fn eval_op(w: &mut World, i: usize, op: &Op, steps: &mut Vec<Step>) {
    let mut push = |cmd: Command, res: Result<Preview, CommandError>, kinds: Vec<RoadKind>| {
        steps.push(Step {
            op: i,
            cmd: Some(cmd),
            result: res,
            removed_kinds: kinds,
        })
    };
    if let Op::RemoveSerials(lo, hi) = op {
        // A batch of segments at a time (`REMOVE_BATCH`): one road change
        // each, so a large road goes in a few commands. Looked up again for
        // each batch: a removal may merge the others.
        for _ in 0..10_000 {
            let mut parts: Vec<(citysim_sim::SegmentId, f32, f32)> = w
                .roads
                .segments
                .iter()
                .filter(|(_, s)| (*lo..*hi).contains(&s.serial))
                .map(|(s, seg)| (s, 0.0, seg.length))
                .collect();
            if parts.is_empty() {
                break;
            }
            parts.truncate(REMOVE_BATCH);
            let cmd = if parts.len() == 1 {
                let (segment, from_m, to_m) = parts[0];
                Command::RemoveRoad { segment, from_m, to_m }
            } else {
                Command::RemoveRoads { parts }
            };
            let kinds = removed_kinds(w, &cmd);
            let res = w.apply(cmd.clone());
            let failed = res.is_err();
            push(cmd, res, kinds);
            if failed {
                break;
            }
        }
        return;
    }
    match resolve(w, op) {
        Ok(cmd) => {
            let kinds = removed_kinds(w, &cmd);
            let res = w.apply(cmd.clone());
            push(cmd, res, kinds);
        }
        Err(e) => steps.push(Step {
            op: i,
            cmd: None,
            result: Err(e),
            removed_kinds: Vec::new(),
        }),
    }
}

impl Evaluated {
    fn of(steps: Vec<Step>) -> Evaluated {
        let mut out = Evaluated::default();
        for st in steps {
            match st.cmd.clone() {
                Some(cmd) => out.push_step(st.op, cmd, st.result, st.removed_kinds),
                None => out.steps.push(st),
            }
        }
        out
    }
}

/// Check `ops` in order on a copy of `base`; steps that fail are left out
/// and the rest carry on (so the valid part can still be drawn). Returns
/// the checked plan and the world after it. The reference for
/// [`Prefixes::evaluate`].
#[cfg_attr(not(test), allow(dead_code))]
pub fn evaluate(base: &World, ops: &[Op]) -> (Evaluated, World) {
    let mut w = base.clone();
    let mut steps = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        eval_op(&mut w, i, op, &mut steps);
    }
    (Evaluated::of(steps), w)
}

/// Snapshots kept at most (one per operation of the last plan).
const MAX_SNAPSHOTS: usize = 96;

/// The last plan checked on a base world, with the world after each of
/// its operations: a plan that starts with the same operations on the
/// same base carries on from there (a chain of pieces where only the live
/// one changes is one command to check, not all of them). Gives exactly
/// what [`evaluate`] gives: the world after an operation depends only on
/// the base and the operations before it.
#[derive(Default)]
pub struct Prefixes {
    base: Option<u64>,
    ops: Vec<Op>,
    /// World after operation `i` (`after.len() <= ops.len()`).
    after: Vec<World>,
    /// Steps of operation `i`, and the time it took (ms).
    steps: Vec<(Vec<Step>, f32)>,
}

impl Prefixes {
    /// Check `ops` on `base` (identified by `base_id`: a new id for every
    /// new base world). Returns the plan, the world after it and each
    /// step's time to check (ms, parallel to the steps).
    pub fn evaluate(&mut self, base_id: u64, base: &World, ops: &[Op]) -> (Evaluated, World, Vec<f32>) {
        if self.base != Some(base_id) {
            *self = Prefixes {
                base: Some(base_id),
                ..Default::default()
            };
        }
        let keep = self
            .ops
            .iter()
            .zip(ops)
            .take_while(|(a, b)| a == b)
            .count()
            .min(self.after.len());
        self.ops.truncate(keep);
        self.after.truncate(keep);
        self.steps.truncate(keep);
        let mut w = match keep {
            0 => base.clone(),
            k => self.after[k - 1].clone(),
        };
        for (i, op) in ops.iter().enumerate().skip(keep) {
            let t = std::time::Instant::now();
            let mut steps = Vec::new();
            eval_op(&mut w, i, op, &mut steps);
            let ms = t.elapsed().as_secs_f32() * 1e3;
            self.ops.push(op.clone());
            self.steps.push((steps, ms));
            if self.after.len() + 1 == self.ops.len() && self.after.len() < MAX_SNAPSHOTS && i + 1 < ops.len() {
                self.after.push(w.clone());
            }
        }
        // The last world is not kept as a snapshot (the caller owns it):
        // the next plan usually differs in its last operation.
        let mut all = Vec::new();
        let mut times = Vec::new();
        for (steps, ms) in &self.steps {
            let n = steps.len().max(1) as f32;
            times.extend(std::iter::repeat_n(ms / n, steps.len()));
            all.extend(steps.iter().cloned());
        }
        (Evaluated::of(all), w, times)
    }
}

// ---------------------------------------------------------------- undo

/// How to take an action back.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoRecord {
    Roads {
        label: String,
        /// Serials of the segments the action built.
        new_serials: (u64, u64),
        /// Parts whose kind changed, with the kind before.
        restore_kinds: Vec<(Curve, RoadKind)>,
        /// Road the action removed, with its kind.
        rebuild: Vec<(Curve, RoadKind)>,
        /// Buildings the action demolished (bulldozed, or in the way of a
        /// road's new land), as they were: undo puts them back after the
        /// roads (`Command::RestoreBuilding`).
        buildings: Vec<BuildingRecord>,
        /// Zoning the action's road land took (cell centres): put back
        /// before the buildings, which need their lots zoned.
        zones: Vec<([f32; 2], Zone)>,
        /// What the action paid for road, and when: refunded last
        /// (`Command::Refund`).
        paid: Vec<(u64, i64)>,
    },
    Zones {
        label: String,
        restore: Vec<(CellRef, Option<Zone>)>,
    },
    /// A junction tool click: the roundabout and setting before it, the
    /// buildings a ring took down and the zones of the land it took.
    Junction {
        label: String,
        node: citysim_sim::NodeId,
        ring: Option<citysim_sim::RoundaboutSize>,
        setting: citysim_sim::JunctionSetting,
        buildings: Vec<BuildingRecord>,
        /// Zoning the ring's land took (cell centres), as for roads.
        zones: Vec<([f32; 2], Zone)>,
        /// What the ring cost and when (`Command::Refund`).
        paid: Vec<(u64, i64)>,
    },
}

impl UndoRecord {
    pub fn label(&self) -> &str {
        match self {
            UndoRecord::Roads { label, .. } | UndoRecord::Zones { label, .. } | UndoRecord::Junction { label, .. } => {
                label
            }
        }
    }
}

/// Buildings standing in `before` and gone in `after` (bulldozed, or doomed
/// by new road land), as records taken from `real` (the world the commands
/// will be sent to; `before` may be its scratch copy, with the same ids).
pub fn demolished(real: &World, before: &World, after: &World) -> Vec<BuildingRecord> {
    before
        .buildings
        .keys()
        .filter(|id| !after.buildings.contains_key(*id))
        .filter_map(|id| real.building_record(id))
        .collect()
}

/// The undo record of a road action (build, change of kind, removal)
/// checked as `eval` on `before` (at the real world's tick: road payments
/// are refunded by it), leaving `after`; `buildings`: what it
/// demolishes ([`demolished`]).
pub fn undo_for_roads(
    before: &World,
    after: &World,
    eval: &Evaluated,
    label: String,
    buildings: Vec<BuildingRecord>,
) -> UndoRecord {
    let mut restore_kinds = Vec::new();
    for seg in after.roads.segments.values() {
        if seg.serial >= before.roads.next_serial {
            continue;
        }
        if let Some(k) = kind_along(before, &seg.curve)
            && k != seg.kind
        {
            restore_kinds.push((seg.curve, k));
        }
    }
    let tick = before.time.tick;
    UndoRecord::Roads {
        label,
        new_serials: (before.roads.next_serial, after.roads.next_serial),
        restore_kinds,
        rebuild: eval.removed(),
        buildings,
        zones: lost_zones(before, after),
        paid: eval.road_payments().into_iter().map(|a| (tick, a)).collect(),
    }
}

/// Zones that cells of `before` no longer have in `after` (a road's
/// reserve took them), by cell centre.
pub fn lost_zones(before: &World, after: &World) -> Vec<([f32; 2], Zone)> {
    if before.versions.roads == after.versions.roads {
        return Vec::new();
    }
    before
        .zoning
        .cells()
        .filter_map(|(c, x)| Some((c, x.zone?)))
        .filter(|&(c, z)| after.zoning.cell(c).is_none_or(|y| y.zone != Some(z)))
        .filter_map(|(c, z)| Some((before.zoning.cell_center(c)?, z)))
        .filter(|&(p, z)| {
            // Unless the same land is zoned so under another name.
            !after
                .zoning
                .cell_at(p)
                .and_then(|c| Some((after.zoning.cell_center(c)?, after.zoning.cell(c)?.zone)))
                .is_some_and(|(q, now)| dist(p, q) < SAME_LAND_M && now == Some(z))
        })
        .collect()
}

/// The undo record of a zoning action changing `cells`.
pub fn undo_for_zones(before: &World, cells: &[CellRef], label: String) -> UndoRecord {
    UndoRecord::Zones {
        label,
        restore: cells
            .iter()
            .filter_map(|&c| Some((c, before.zoning.cell(c)?.zone)))
            .collect(),
    }
}

/// The operations that take `rec` back in `world` now.
pub fn undo_ops(world: &World, rec: &UndoRecord) -> Vec<Op> {
    match rec {
        UndoRecord::Roads {
            new_serials,
            restore_kinds,
            rebuild,
            buildings,
            zones,
            paid,
            ..
        } => {
            let mut ops = Vec::new();
            if new_serials.1 > new_serials.0 {
                ops.push(Op::RemoveSerials(new_serials.0, new_serials.1));
            }
            for (c, k) in restore_kinds {
                if kind_along(world, c).is_some_and(|now| now != *k) {
                    ops.push(Op::Kind(*c, *k));
                }
            }
            for (c, k) in rebuild {
                ops.push(Op::Cmd(Command::BuildCurve { curve: *c, kind: *k }));
            }
            let mut by_zone: BTreeMap<u8, (Zone, Vec<[f32; 2]>)> = BTreeMap::new();
            for &(p, z) in zones {
                by_zone.entry(z as u8).or_insert((z, Vec::new())).1.push(p);
            }
            for (z, points) in by_zone.into_values() {
                ops.push(Op::ZoneAt(points, z));
            }
            for b in buildings {
                ops.push(Op::Cmd(Command::RestoreBuilding(Box::new(b.clone()))));
            }
            for &(tick, amount) in paid {
                ops.push(Op::Cmd(Command::Refund { tick, amount }));
            }
            ops
        }
        UndoRecord::Junction {
            node,
            ring,
            setting,
            buildings,
            zones,
            paid,
            ..
        } => {
            let mut ops = Vec::new();
            let Some(j) = citysim_sim::junction::junction(world, *node) else {
                return ops;
            };
            if j.ring != *ring {
                ops.push(Op::Cmd(Command::SetRoundabout {
                    node: *node,
                    size: *ring,
                }));
            }
            if j.setting != *setting {
                ops.push(Op::Cmd(Command::SetNodeControl {
                    node: *node,
                    control: *setting,
                }));
            }
            // The land a ring took gets its zones back, then its buildings,
            // then the money.
            let mut by_zone: BTreeMap<u8, (Zone, Vec<[f32; 2]>)> = BTreeMap::new();
            for &(p, z) in zones {
                by_zone.entry(z as u8).or_insert((z, Vec::new())).1.push(p);
            }
            for (z, points) in by_zone.into_values() {
                ops.push(Op::ZoneAt(points, z));
            }
            for b in buildings {
                ops.push(Op::Cmd(Command::RestoreBuilding(Box::new(b.clone()))));
            }
            for &(tick, amount) in paid {
                ops.push(Op::Cmd(Command::Refund { tick, amount }));
            }
            ops
        }
        UndoRecord::Zones { restore, .. } => {
            let mut by_zone: BTreeMap<u8, (Option<Zone>, Vec<CellRef>)> = BTreeMap::new();
            for &(c, z) in restore {
                let now = world.zoning.cell(c);
                if now.is_some_and(|zc| zc.zone != z && zc.building.is_none()) {
                    let key = z.map_or(0, |z| z as u8 + 1);
                    by_zone.entry(key).or_insert((z, Vec::new())).1.push(c);
                }
            }
            by_zone
                .into_values()
                .map(|(zone, cells)| Op::Cmd(Command::ZoneCells { cells, zone }))
                .collect()
        }
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

    fn road(a: [f32; 2], b: [f32; 2]) -> Op {
        Op::Cmd(Command::BuildCurve {
            curve: Curve::straight(a, b),
            kind: RoadKind::Street,
        })
    }

    #[test]
    fn a_grid_a_hair_off_a_junction_still_builds() {
        // A grid whose corner was snapped onto the road 5 cm off the cell
        // centre: its last street started 5 cm from the junction where a
        // curve leaves, and ran 3.5 cm past the ends of its own rows, which
        // the sim refuses as crossings too close to a road's end. The tool
        // moves the grid onto the junction.
        let mut w = rich();
        let Op::Cmd(c) = road([92.0, 1028.0], [292.0, 1028.0]) else {
            unreachable!()
        };
        w.apply(c).unwrap();
        w.apply(Command::BuildCurve {
            curve: Curve::through_corner([292.0, 1028.0], [468.0, 1028.0], [532.0, 868.0]),
            kind: RoadKind::Street,
        })
        .unwrap();
        let g = crate::ui::shapes::GridFrame {
            a: [132.04666, 1028.0],
            u: [1.0, 0.0],
            v: [0.0, 1.0],
            nu: 4,
            nv: 4,
            block_m: 40.0,
        };
        let ops = |g: &crate::ui::shapes::GridFrame| -> Vec<Op> {
            g.streets()
                .into_iter()
                .map(|curve| {
                    Op::Cmd(Command::BuildCurve {
                        curve,
                        kind: RoadKind::Street,
                    })
                })
                .collect()
        };
        let (raw, _) = evaluate(&scratch(&w), &ops(&g));
        assert!(!raw.ok(), "the near miss itself is refused by the sim");
        let fixed = crate::ui::tools::align_grid(&w, g);
        assert!(dist(fixed.a, [132.0, 1028.0]) < 1e-3, "{:?}", fixed.a);
        let (e, _) = evaluate(&scratch(&w), &ops(&fixed));
        assert!(e.ok(), "{:?}", e.first_error());
    }

    #[test]
    fn a_chain_checks_each_piece_after_the_ones_before() {
        let w = rich();
        let ops = [
            road([92.0, 1028.0], [200.0, 1028.0]),
            road([200.0, 1028.0], [200.0, 940.0]),
            // Runs right beside the second piece: refused, the rest stays.
            road([203.0, 960.0], [203.0, 1000.0]),
        ];
        let (e, after) = evaluate(&scratch(&w), &ops);
        assert_eq!(e.steps.len(), 3);
        assert!(e.steps[0].result.is_ok() && e.steps[1].result.is_ok());
        assert!(e.steps[2].result.is_err());
        assert!(!e.ok());
        assert!((e.length_m - 196.0).abs() < 0.5, "{}", e.length_m);
        assert_eq!(after.roads.segments.len(), 3);
        // The second piece joins the first one's end: one node there.
        assert_eq!(e.nodes().iter().filter(|n| dist(n.0, [200.0, 1028.0]) < 0.5).count(), 1);
    }

    #[test]
    fn undo_takes_a_road_back_and_restores_a_kind() {
        let mut w = rich();
        let before = scratch(&w);
        let ops = [road([92.0, 1028.0], [300.0, 1028.0])];
        let (e, after) = evaluate(&before, &ops);
        for c in e.cmds() {
            w.apply(c).unwrap();
        }
        let rec = undo_for_roads(&before, &after, &e, "road".into(), Vec::new());
        let (u, _) = evaluate(&scratch(&w), &undo_ops(&w, &rec));
        assert!(u.ok(), "{:?}", u.first_error());
        for c in u.cmds() {
            w.apply(c).unwrap();
        }
        assert_eq!(w.roads.segments.len(), 1, "only the highway stub is left");
        // An avenue over part of a street, then undo.
        let mut w = rich();
        w.apply(Command::BuildCurve {
            curve: Curve::straight([92.0, 1028.0], [300.0, 1028.0]),
            kind: RoadKind::Street,
        })
        .unwrap();
        let before = scratch(&w);
        let part = Curve::straight([150.0, 1028.0], [250.0, 1028.0]);
        let (e, after) = evaluate(&before, &[Op::Kind(part, RoadKind::Avenue)]);
        assert!(e.ok(), "{:?}", e.first_error());
        assert!(e.cost > 0);
        for c in e.cmds() {
            w.apply(c).unwrap();
        }
        assert_eq!(kind_along(&w, &part), Some(RoadKind::Avenue));
        let rec = undo_for_roads(&before, &after, &e, "avenue".into(), Vec::new());
        let (u, _) = evaluate(&scratch(&w), &undo_ops(&w, &rec));
        assert!(u.ok(), "{:?}", u.first_error());
        for c in u.cmds() {
            w.apply(c).unwrap();
        }
        assert_eq!(kind_along(&w, &part), Some(RoadKind::Street));
    }

    #[test]
    fn undo_of_a_removal_rebuilds_the_road() {
        let mut w = rich();
        w.apply(Command::BuildCurve {
            curve: Curve::straight([92.0, 1028.0], [300.0, 1028.0]),
            kind: RoadKind::Avenue,
        })
        .unwrap();
        let before = scratch(&w);
        let part = Curve::straight([200.0, 1028.0], [300.0, 1028.0]);
        let (e, after) = evaluate(&before, &[Op::Remove(part)]);
        assert!(e.ok(), "{:?}", e.first_error());
        assert_eq!(e.removed().len(), 1);
        assert_eq!(e.removed()[0].1, RoadKind::Avenue);
        for c in e.cmds() {
            w.apply(c).unwrap();
        }
        let rec = undo_for_roads(&before, &after, &e, "removal".into(), Vec::new());
        let (u, _) = evaluate(&scratch(&w), &undo_ops(&w, &rec));
        assert!(u.ok(), "{:?}", u.first_error());
        for c in u.cmds() {
            w.apply(c).unwrap();
        }
        assert_eq!(kind_along(&w, &part), Some(RoadKind::Avenue));
    }

    #[test]
    fn undo_of_an_area_demolition_brings_road_and_buildings_back() {
        let mut w = World::new(5);
        citysim_sim::scenarios::prebuilt(&mut w, |w| citysim_sim::scenarios::two_districts(w, true));
        while w.time.tick < citysim_sim::balance::TICKS_PER_DAY as u64 {
            w.step();
        }
        let (home, b) = w
            .buildings
            .iter()
            .filter(|(id, b)| {
                b.kind == Zone::Residential && w.people.places.get(*id).is_some_and(|p| !p.households.is_empty())
            })
            .map(|(id, b)| (id, b.clone()))
            .next()
            .expect("a home");
        let (s, _, _) = w.roads.nearest_segment(b.center(), 40.0).unwrap();
        let p = w.roads.segments[s].nearest(b.center()).2;
        let c = b.center();
        let (lo, hi) = (
            [p[0].min(c[0]) - 6.0, p[1].min(c[1]) - 6.0],
            [p[0].max(c[0]) + 6.0, p[1].max(c[1]) + 6.0],
        );
        let quad = [[lo[0], lo[1]], [hi[0], lo[1]], [hi[0], hi[1]], [lo[0], hi[1]]];
        let before = scratch(&w);
        let (e, after) = evaluate(&before, &[Op::Cmd(Command::BulldozeArea { quad })]);
        assert!(e.ok(), "{:?}", e.first_error());
        let lost = demolished(&w, &before, &after);
        assert!(lost.iter().any(|r| r.residents() > 0));
        let people = w.citizens.len();
        let rec = undo_for_roads(&before, &after, &e, "demolition".into(), lost.clone());
        for c in e.cmds() {
            w.apply(c).unwrap();
        }
        assert!(!w.buildings.contains_key(home));
        assert!(w.citizens.len() < people);
        let (u, _) = evaluate(&scratch(&w), &undo_ops(&w, &rec));
        assert!(u.ok(), "{:?}", u.first_error());
        for c in u.cmds() {
            w.apply(c).unwrap();
        }
        assert_eq!(w.citizens.len(), people, "everyone is back");
        for r in &lost {
            assert!(w.buildings.values().any(|b| b.seed == r.building.seed));
        }
        w.check_sim_invariants().unwrap();
    }

    #[test]
    fn zoning_undo_puts_the_old_zones_back() {
        let mut w = rich();
        w.apply(Command::BuildCurve {
            curve: Curve::straight([92.0, 1028.0], [300.0, 1028.0]),
            kind: RoadKind::Street,
        })
        .unwrap();
        let quad = [[150.0, 1010.0], [250.0, 1010.0], [250.0, 1024.0], [150.0, 1024.0]];
        let before = scratch(&w);
        let (e, _) = evaluate(
            &before,
            &[Op::Cmd(Command::ZoneArea {
                quad,
                zone: Some(Zone::Residential),
            })],
        );
        let cells = e.zone_cells();
        assert!(!cells.is_empty());
        for c in e.cmds() {
            w.apply(c).unwrap();
        }
        let rec = undo_for_zones(&before, &cells, "zoning".into());
        let (u, _) = evaluate(&scratch(&w), &undo_ops(&w, &rec));
        for c in u.cmds() {
            w.apply(c).unwrap();
        }
        assert!(cells.iter().all(|&c| w.zoning.cell(c).unwrap().zone.is_none()));
    }

    /// A tiny deterministic generator for the random plans below.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    #[test]
    fn prefixes_check_plans_exactly_as_the_plain_evaluation() {
        // A town to draw on, then plans the way the tools change them: a
        // chain growing piece by piece, its live end moving, pieces taken
        // back, and plans that have nothing in common.
        let mut w = World::new(9);
        citysim_sim::scenarios::prebuilt(&mut w, |w| citysim_sim::scenarios::two_districts(w, true));
        for _ in 0..600 {
            w.step();
        }
        let base = scratch(&w);
        let mut rng = Lcg(42);
        let mut pre = Prefixes::default();
        let mut ops: Vec<Op> = Vec::new();
        let mut checked = 0;
        for round in 0..60 {
            let r = rng.next();
            if r < 0.15 && !ops.is_empty() {
                ops.truncate((rng.next() * ops.len() as f32) as usize);
            } else if r < 0.2 {
                ops.clear();
            } else if r < 0.5 {
                ops.pop();
            }
            // New pieces: straight or curved streets and avenues across
            // the town (many cross roads, some are refused).
            for _ in 0..1 + (rng.next() * 3.0) as usize {
                let p = |rng: &mut Lcg| [60.0 + rng.next() * 500.0, 860.0 + rng.next() * 340.0];
                let (a, c, b) = (p(&mut rng), p(&mut rng), p(&mut rng));
                let curve = if rng.next() < 0.6 {
                    Curve::straight(a, b)
                } else {
                    Curve::through_corner(a, c, b)
                };
                let kind = if rng.next() < 0.8 {
                    RoadKind::Street
                } else {
                    RoadKind::Avenue
                };
                ops.push(if rng.next() < 0.1 {
                    Op::Remove(Curve::straight(a, b))
                } else {
                    Op::Cmd(Command::BuildCurve { curve, kind })
                });
            }
            let (want, want_after) = evaluate(&base, &ops);
            let (got, got_after, times) = pre.evaluate(1, &base, &ops);
            assert_eq!(got, want, "round {round}");
            assert_eq!(times.len(), got.steps.len());
            assert!(got_after == want_after, "round {round}: the world after differs");
            assert_eq!(got_after.roads.segments.len(), want_after.roads.segments.len());
            checked += ops.len();
        }
        assert!(checked > 100);
        // A new base drops what was kept.
        let (got, _, _) = pre.evaluate(2, &scratch(&w), &ops);
        assert_eq!(got, evaluate(&base, &ops).0);
    }

    // ------------------------------------------- roads through buildings

    /// The two-district town grown for a day (homes, shops, works).
    fn grown() -> World {
        let mut w = World::new(5);
        citysim_sim::scenarios::prebuilt(&mut w, |w| citysim_sim::scenarios::two_districts(w, true));
        while w.time.tick < citysim_sim::balance::TICKS_PER_DAY as u64 {
            w.step();
        }
        w.economy.money = 1_000_000;
        w
    }

    fn unit(v: [f32; 2]) -> [f32; 2] {
        let l = (v[0] * v[0] + v[1] * v[1]).sqrt().max(1e-6);
        [v[0] / l, v[1] / l]
    }

    /// For each home: the point on its road in front of it, the direction
    /// from there to the home, and the road's direction.
    fn fronts(w: &World) -> Vec<([f32; 2], [f32; 2], [f32; 2])> {
        w.buildings
            .values()
            .filter(|b| b.kind == Zone::Residential && b.occupied > 0)
            .filter_map(|b| {
                let c = b.center();
                let (s, d, _) = w.roads.nearest_segment(c, 40.0)?;
                let seg = &w.roads.segments[s];
                let p = seg.nearest(c).2;
                Some((p, unit([c[0] - p[0], c[1] - p[1]]), seg.pose(d, 0.0).dir))
            })
            .collect()
    }

    /// What `commit` does: check the plan on a scratch copy, record the undo,
    /// send the commands. Returns the undo record and the checked plan.
    fn commit(w: &mut World, ops: &[Op]) -> (UndoRecord, Evaluated) {
        let before = scratch(w);
        let (e, after) = evaluate(&before, ops);
        assert!(e.ok(), "{:?}", e.first_error());
        let lost = demolished(w, &before, &after);
        let rec = undo_for_roads(&before, &after, &e, "road".into(), lost);
        for c in e.cmds() {
            w.apply(c).unwrap();
        }
        (rec, e)
    }

    /// What Ctrl+Z does with the record.
    fn take_back(w: &mut World, rec: &UndoRecord) {
        let (u, _) = evaluate(&scratch(w), &undo_ops(w, rec));
        assert!(u.ok(), "{:?}", u.first_error());
        for c in u.cmds() {
            w.apply(c).unwrap();
        }
    }

    /// Build `ops` through grown buildings, check what went is what was
    /// marked and paid for, undo, check everything is back.
    fn through_and_back(w0: &World, ops: &[Op]) -> usize {
        let mut w = w0.clone();
        let (money, people, n) = (w.economy.money, w.citizens.len(), w.buildings.len());
        let (rec, e) = commit(&mut w, ops);
        let marked = e.buildings();
        assert!(!marked.is_empty(), "the road takes buildings");
        let gone: Vec<BuildingId> = w0.buildings.keys().filter(|b| !w.buildings.contains_key(*b)).collect();
        assert_eq!(gone, marked, "what goes is what was marked");
        assert_eq!(w.economy.money, money - e.cost, "road and demolitions paid");
        let demolition: i64 = e
            .steps
            .iter()
            .filter_map(|s| s.result.as_ref().ok())
            .map(|p| p.demolition_cost)
            .sum();
        assert!(demolition > 0 && e.cost > demolition);
        assert!(w.citizens.len() < people, "residents moved out");
        let UndoRecord::Roads { buildings, paid, .. } = &rec else {
            unreachable!()
        };
        assert_eq!(buildings.len(), marked.len());
        assert!(!paid.is_empty());
        take_back(&mut w, &rec);
        assert_eq!(w.buildings.len(), n, "every building is back");
        assert_eq!(w.citizens.len(), people, "everyone is back");
        assert_eq!(w.roads.segments.len(), w0.roads.segments.len(), "the road is gone");
        // Road and demolitions refunded; taking the road away again costs
        // the bulldozer's fee (a dollar a cell).
        let fee = money - w.economy.money;
        let len: f32 = e.pieces().iter().map(|(c, _)| c.length()).sum();
        assert!(fee >= 0 && fee <= (len / 4.0) as i64 + 4, "fee {fee} for {len} m");
        w.check_sim_invariants().unwrap();
        marked.len()
    }

    #[test]
    fn a_street_through_homes_from_a_road_then_undo() {
        // A T on the road in front of a home, straight out through its lot
        // (splits the existing road).
        let w = grown();
        let mut done = 0;
        for (p, d, _) in fronts(&w) {
            let ops = [road_op(
                Curve::straight(p, [p[0] + d[0] * 30.0, p[1] + d[1] * 30.0]),
                RoadKind::Street,
            )];
            if evaluate(&scratch(&w), &ops).0.ok() {
                through_and_back(&w, &ops);
                done += 1;
                if done == 2 {
                    break;
                }
            }
        }
        assert_eq!(done, 2);
    }

    #[test]
    fn a_street_along_a_row_of_lots_and_a_curve_then_undo() {
        // A street parallel to a road through its first row of lots (two
        // T's at the cross streets would be refused for spacing: a free
        // piece in the middle of the block), and a curve through a block.
        let w = grown();
        let (mut row, mut curve) = (0, 0);
        for (p, d, t) in fronts(&w) {
            let q = [p[0] + d[0] * 12.0, p[1] + d[1] * 12.0];
            let a = [q[0] - t[0] * 14.0, q[1] - t[1] * 14.0];
            let b = [q[0] + t[0] * 14.0, q[1] + t[1] * 14.0];
            if row == 0 {
                let ops = [road_op(Curve::straight(a, b), RoadKind::Street)];
                if evaluate(&scratch(&w), &ops).0.ok() {
                    row = through_and_back(&w, &ops);
                }
            }
            if curve == 0 {
                let end = [p[0] + d[0] * 30.0 + t[0] * 24.0, p[1] + d[1] * 30.0 + t[1] * 24.0];
                let corner = [p[0] + d[0] * 30.0, p[1] + d[1] * 30.0];
                let ops = [road_op(Curve::through_corner(p, corner, end), RoadKind::Street)];
                if evaluate(&scratch(&w), &ops).0.ok() {
                    curve = through_and_back(&w, &ops);
                }
            }
            if row > 0 && curve > 0 {
                break;
            }
        }
        assert!(row > 0 && curve > 0, "row {row} curve {curve}");
    }

    #[test]
    fn a_grid_through_homes_then_undo() {
        let w = grown();
        for (p, d, t) in fronts(&w) {
            for block in [24.0, 32.0] {
                let g = crate::ui::shapes::GridFrame {
                    a: [p[0] - t[0] * block * 0.5, p[1] - t[1] * block * 0.5],
                    u: t,
                    v: d,
                    nu: 1,
                    nv: 1,
                    block_m: block,
                };
                let g = crate::ui::tools::align_grid(&w, g);
                let ops: Vec<Op> = g.streets().into_iter().map(|c| road_op(c, RoadKind::Street)).collect();
                let (e, _) = evaluate(&scratch(&w), &ops);
                if e.ok() && !e.buildings().is_empty() {
                    through_and_back(&w, &ops);
                    return;
                }
            }
        }
        panic!("no grid fits over the homes");
    }

    #[test]
    fn an_upgrade_is_refunded_by_undo() {
        let mut w = grown();
        let money = w.economy.money;
        let c = w
            .roads
            .segments
            .values()
            .find(|s| s.kind == RoadKind::Street && s.length > 30.0)
            .map(|s| s.curve)
            .unwrap();
        let (rec, e) = commit(&mut w, &[Op::Kind(c, RoadKind::Avenue)]);
        assert!(
            e.cost > 0 && e.buildings().is_empty(),
            "every kind keeps the same reserve"
        );
        assert_eq!(kind_along(&w, &c), Some(RoadKind::Avenue));
        take_back(&mut w, &rec);
        assert_eq!(kind_along(&w, &c), Some(RoadKind::Street));
        assert_eq!(w.economy.money, money, "the upgrade is refunded");
    }

    #[test]
    fn a_refused_or_cancelled_road_takes_nothing() {
        let w = grown();
        let (p, d, _) = fronts(&w)[0];
        let through = road_op(
            Curve::straight(p, [p[0] + d[0] * 30.0, p[1] + d[1] * 30.0]),
            RoadKind::Street,
        );
        // Checking a plan (the preview) changes nothing.
        let before = scratch(&w);
        let (e, _) = evaluate(&before, std::slice::from_ref(&through));
        assert!(!e.buildings().is_empty());
        assert_eq!(before.buildings.len(), w.buildings.len());
        // A second piece refused (runs back along the first): the plan is
        // refused as a whole and still shows the reason.
        let back = road_op(
            Curve::straight(
                [p[0] + d[0] * 30.0, p[1] + d[1] * 30.0],
                [p[0] + d[0] * 2.0, p[1] + d[1] * 2.0],
            ),
            RoadKind::Street,
        );
        let (e, _) = evaluate(&before, &[through.clone(), back]);
        assert!(!e.ok());
        assert!(e.first_error().is_some());
        // Not enough money for road and demolitions.
        let mut poor = w.clone();
        poor.economy.money = 10;
        let (e, _) = evaluate(&scratch(&poor), &[through]);
        assert!(matches!(e.first_error(), Some(CommandError::InsufficientFunds { .. })));
        assert!(
            e.buildings().is_empty(),
            "nothing is marked for a road that can't be paid"
        );
    }
}
