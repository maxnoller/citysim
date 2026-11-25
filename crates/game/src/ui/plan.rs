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
use citysim_sim::{BuildingId, CellRef, Command, CommandError, Curve, Preview, RoadKind, World, Zone};

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
}

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
            let (segment, from_m, to_m) = part_of(w, c).ok_or(CommandError::NoSuchRoad)?;
            Ok(Command::SetRoadKind {
                segment,
                from_m,
                to_m,
                kind: *kind,
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

    /// Buildings demolished.
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
        Command::RemoveRoad { .. } | Command::BulldozeArea { .. } | Command::Bulldoze { .. } => w
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

/// Check `ops` in order on a copy of `base`; steps that fail are left out
/// and the rest carry on (so the valid part can still be drawn). Returns
/// the checked plan and the world after it.
pub fn evaluate(base: &World, ops: &[Op]) -> (Evaluated, World) {
    let mut w = base.clone();
    let mut out = Evaluated::default();
    for (i, op) in ops.iter().enumerate() {
        if let Op::RemoveSerials(lo, hi) = op {
            // One segment at a time: a removal may merge the others.
            for _ in 0..10_000 {
                let Some((s, len)) = w
                    .roads
                    .segments
                    .iter()
                    .find(|(_, s)| (*lo..*hi).contains(&s.serial))
                    .map(|(s, seg)| (s, seg.length))
                else {
                    break;
                };
                let cmd = Command::RemoveRoad {
                    segment: s,
                    from_m: 0.0,
                    to_m: len,
                };
                let kinds = removed_kinds(&w, &cmd);
                let res = w.apply(cmd.clone());
                let failed = res.is_err();
                out.push_step(i, cmd, res, kinds);
                if failed {
                    break;
                }
            }
            continue;
        }
        match resolve(&w, op) {
            Ok(cmd) => {
                let kinds = removed_kinds(&w, &cmd);
                let res = w.apply(cmd.clone());
                out.push_step(i, cmd, res, kinds);
            }
            Err(e) => out.steps.push(Step {
                op: i,
                cmd: None,
                result: Err(e),
                removed_kinds: Vec::new(),
            }),
        }
    }
    (out, w)
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
        /// Buildings the action demolished (cannot come back).
        lost_buildings: usize,
    },
    Zones {
        label: String,
        restore: Vec<(CellRef, Option<Zone>)>,
    },
}

impl UndoRecord {
    pub fn label(&self) -> &str {
        match self {
            UndoRecord::Roads { label, .. } | UndoRecord::Zones { label, .. } => label,
        }
    }
}

/// The undo record of a road action (build, change of kind, removal)
/// checked as `eval` on `before`, leaving `after`.
pub fn undo_for_roads(before: &World, after: &World, eval: &Evaluated, label: String) -> UndoRecord {
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
    UndoRecord::Roads {
        label,
        new_serials: (before.roads.next_serial, after.roads.next_serial),
        restore_kinds,
        rebuild: eval.removed(),
        lost_buildings: eval.buildings().len(),
    }
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
        let rec = undo_for_roads(&before, &after, &e, "road".into());
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
        let rec = undo_for_roads(&before, &after, &e, "avenue".into());
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
        let rec = undo_for_roads(&before, &after, &e, "removal".into());
        let (u, _) = evaluate(&scratch(&w), &undo_ops(&w, &rec));
        assert!(u.ok(), "{:?}", u.first_error());
        for c in u.cmds() {
            w.apply(c).unwrap();
        }
        assert_eq!(kind_along(&w, &part), Some(RoadKind::Avenue));
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
}
