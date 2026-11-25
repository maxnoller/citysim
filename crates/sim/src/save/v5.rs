//! The world-grid layouts of rounds 1-3, frozen: copies of the round-3
//! definitions of every type whose layout changed with free-form roads (road
//! segments without curves, buildings on a world-grid rectangle, the world
//! grid, junction turn paths as quadratics). Never edit them. Formats 4 and
//! 3 decode through these types. Format 5 itself (the unreleased first
//! citizens layout) no longer loads: its people types changed with the
//! 16-minute day.
//!
//! Conversions: roads become straight curves (serials in slot order),
//! buildings stand on lattice 0, zone cells and buildings move onto the
//! blocks of their roads after loading (`zoning::adopt_grid`), junction
//! geometry is rebuilt (settings and timers kept by the live rebuild).

use serde::{Deserialize, Serialize};

use crate::geom::CellRect;
use crate::growth::{BuildingMeta, GrowthState};
use crate::junction::{Arm, JunctionControl, JunctionSetting, JunctionState, Movement, SignalPlan, SignalState, Turn};
use crate::roads::{NodeId, RoadKind, RoadNetwork, RoadNode, RoadSegment, SegmentId};
use crate::side::SideTable;
use crate::traffic::{JunctionMove, Vehicle, VehicleId};
use crate::{Building, BuildingId, Facing, Footprint, HouseholdId, IdMap, Versions, World, Zone};

// ------------------------------------------------------------------ roads ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RoadSegmentV5 {
    pub a: NodeId,
    pub b: NodeId,
    pub kind: RoadKind,
    pub length: f32,
    pub speed_limit: f32,
    pub capacity: [f32; 2],
    pub load: [f32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RoadNetworkV5 {
    pub nodes: IdMap<NodeId, RoadNode>,
    pub segments: IdMap<SegmentId, RoadSegmentV5>,
    pub entry: Option<NodeId>,
}

impl RoadNetworkV5 {
    /// Straight segments between the node positions; serials in slot
    /// order (the order of the slots is the closest thing to build order
    /// these saves have).
    pub(crate) fn upgrade(self) -> RoadNetwork {
        let nodes = self.nodes;
        let mut serial = 0u64;
        let segments = self.segments.map_values(|s| {
            let (pa, pb) = (
                nodes.get(s.a).map_or([0.0; 2], |n| n.pos),
                nodes.get(s.b).map_or([0.0; 2], |n| n.pos),
            );
            let mut seg = RoadSegment::with_curve(s.a, s.b, s.kind, crate::Curve::straight(pa, pb), serial, s.load);
            serial += 1;
            // Keep the stored capacity (equal for straight lattice roads).
            if s.capacity.iter().all(|c| c.is_finite() && *c > 0.0) {
                seg.capacity = s.capacity;
            }
            seg
        });
        RoadNetwork {
            nodes,
            segments,
            entry: self.entry,
            next_serial: serial,
        }
    }

    pub(crate) fn of(r: &RoadNetwork) -> RoadNetworkV5 {
        RoadNetworkV5 {
            nodes: r.nodes.clone(),
            segments: r.segments.clone().map_values(|s| RoadSegmentV5 {
                a: s.a,
                b: s.b,
                kind: s.kind,
                length: s.length,
                speed_limit: s.speed_limit,
                capacity: s.capacity,
                load: s.load,
            }),
            entry: r.entry,
        }
    }
}

// --------------------------------------------------------------- the grid ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum OccupantV5 {
    Empty,
    Road,
    Building(BuildingId),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct CellDataV5 {
    pub zone: Option<Zone>,
    pub occupant: OccupantV5,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GridV5 {
    pub size: i32,
    pub cells: Vec<CellDataV5>,
}

impl GridV5 {
    /// Zoned cells (zones under roads were never kept).
    pub(crate) fn zones(&self) -> Vec<(crate::Cell, Zone)> {
        let n = self.size.max(1);
        self.cells
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let z = c.zone?;
                (c.occupant != OccupantV5::Road).then_some((crate::Cell::new(i as i32 % n, i as i32 / n), z))
            })
            .collect()
    }

    /// The world grid of a live world (lattice 0, as old saves held it).
    pub(crate) fn of_world(w: &World) -> GridV5 {
        let view = w.grid_view();
        let n = crate::MAP_CELLS;
        let mut cells = Vec::with_capacity((n * n) as usize);
        for y in 0..n {
            for x in 0..n {
                let c = view.get(crate::Cell::new(x, y)).expect("in bounds");
                cells.push(CellDataV5 {
                    zone: c.zone,
                    occupant: match c.occupant {
                        crate::Occupant::Empty => OccupantV5::Empty,
                        crate::Occupant::Road => OccupantV5::Road,
                        crate::Occupant::Building(b) => OccupantV5::Building(b),
                    },
                });
            }
        }
        GridV5 { size: n, cells }
    }
}

// -------------------------------------------------------------- buildings ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BuildingV5 {
    pub kind: Zone,
    pub footprint: CellRect,
    pub level: u8,
    pub capacity: u32,
    pub occupied: u32,
    pub facing: Facing,
    pub seed: u32,
    pub created_tick: u64,
}

impl BuildingV5 {
    pub(crate) fn upgrade(self) -> Building {
        Building {
            kind: self.kind,
            footprint: Footprint::world(self.footprint, self.facing),
            level: self.level,
            capacity: self.capacity,
            occupied: self.occupied,
            facing: self.facing,
            seed: self.seed,
            created_tick: self.created_tick,
        }
    }

    pub(crate) fn of(b: &Building) -> BuildingV5 {
        BuildingV5 {
            kind: b.kind,
            footprint: b.footprint.rect(),
            level: b.level,
            capacity: b.capacity,
            occupied: b.occupied,
            facing: b.facing,
            seed: b.seed,
            created_tick: b.created_tick,
        }
    }
}

pub(crate) fn upgrade_buildings(b: IdMap<BuildingId, BuildingV5>) -> IdMap<BuildingId, Building> {
    b.map_values(BuildingV5::upgrade)
}

pub(crate) fn buildings_of(b: &IdMap<BuildingId, Building>) -> IdMap<BuildingId, BuildingV5> {
    b.clone().map_values(|x| BuildingV5::of(&x))
}

// -------------------------------------------------------------- junctions ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ArmV5 {
    pub segment: SegmentId,
    pub dir: [f32; 2],
    pub kind: RoadKind,
    pub setback: f32,
    pub lanes_in: u8,
    pub lanes_out: u8,
    pub major: bool,
    pub opposite: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MovementV5 {
    pub from_arm: u8,
    pub to_arm: u8,
    pub from_lane: u8,
    pub to_lane: u8,
    pub turn: Turn,
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub p2: [f32; 2],
    pub length: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JunctionStateV5 {
    pub setting: JunctionSetting,
    pub control: JunctionControl,
    pub arms: Vec<ArmV5>,
    pub movements: Vec<MovementV5>,
    pub conflicts: Vec<u128>,
    pub plan: Option<SignalPlan>,
    pub signal: SignalState,
    pub queue: Vec<u16>,
    pub entered_minute: u16,
    pub peak_minute_today: u16,
    pub peak_minute_yesterday: u16,
    pub busy: bool,
    pub clear_at: u64,
    pub last_served: u8,
    pub phase_count: u16,
}

impl JunctionStateV5 {
    /// A live state with the old arms and turn paths (rebuilt from the
    /// live geometry right after loading; settings and timers survive).
    pub(crate) fn upgrade(self) -> JunctionState {
        JunctionState {
            setting: self.setting,
            control: self.control,
            arms: self
                .arms
                .into_iter()
                .map(|a| Arm {
                    segment: a.segment,
                    dir: a.dir,
                    kind: a.kind,
                    setback: a.setback,
                    lanes_in: a.lanes_in,
                    lanes_out: a.lanes_out,
                    major: a.major,
                    opposite: a.opposite,
                })
                .collect(),
            movements: self
                .movements
                .into_iter()
                .map(|m| Movement::quadratic(m.from_arm, m.to_arm, m.from_lane, m.to_lane, m.turn, m.p0, m.p1, m.p2))
                .collect(),
            conflicts: self.conflicts,
            plan: self.plan,
            signal: self.signal,
            queue: self.queue,
            entered_minute: self.entered_minute,
            peak_minute_today: self.peak_minute_today,
            peak_minute_yesterday: self.peak_minute_yesterday,
            busy: self.busy,
            clear_at: self.clear_at,
            last_served: self.last_served,
            phase_count: self.phase_count,
        }
    }

    pub(crate) fn of(j: &JunctionState) -> JunctionStateV5 {
        JunctionStateV5 {
            setting: j.setting,
            control: j.control,
            arms: j
                .arms
                .iter()
                .map(|a| ArmV5 {
                    segment: a.segment,
                    dir: a.dir,
                    kind: a.kind,
                    setback: a.setback,
                    lanes_in: a.lanes_in,
                    lanes_out: a.lanes_out,
                    major: a.major,
                    opposite: a.opposite,
                })
                .collect(),
            movements: j
                .movements
                .iter()
                .map(|m| {
                    let c = m.curve.p;
                    // The quadratic control point of a degree-elevated
                    // cubic.
                    let p1 = [c[0][0] + (c[1][0] - c[0][0]) * 1.5, c[0][1] + (c[1][1] - c[0][1]) * 1.5];
                    MovementV5 {
                        from_arm: m.from_arm,
                        to_arm: m.to_arm,
                        from_lane: m.from_lane,
                        to_lane: m.to_lane,
                        turn: m.turn,
                        p0: c[0],
                        p1,
                        p2: c[3],
                        length: m.length,
                    }
                })
                .collect(),
            conflicts: j.conflicts.clone(),
            plan: j.plan.clone(),
            signal: j.signal,
            queue: j.queue.clone(),
            entered_minute: j.entered_minute,
            peak_minute_today: j.peak_minute_today,
            peak_minute_yesterday: j.peak_minute_yesterday,
            busy: j.busy,
            clear_at: j.clear_at,
            last_served: j.last_served,
            phase_count: j.phase_count,
        }
    }
}

pub(crate) fn upgrade_junctions(t: SideTable<NodeId, JunctionStateV5>) -> SideTable<NodeId, JunctionState> {
    let mut out = SideTable::default();
    for (n, j) in t.iter() {
        out.insert(n, j.clone().upgrade());
    }
    out
}

pub(crate) fn junctions_of(t: &SideTable<NodeId, JunctionState>) -> SideTable<NodeId, JunctionStateV5> {
    let mut out = SideTable::default();
    for (n, j) in t.iter() {
        out.insert(n, JunctionStateV5::of(j));
    }
    out
}

// --------------------------------------------------------------- vehicles ----

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct JunctionMoveV5 {
    pub node: NodeId,
    pub from: SegmentId,
    pub to: SegmentId,
    pub to_forward: bool,
    pub movement: u16,
    pub turn: Turn,
    pub from_lane: u8,
    pub to_lane: u8,
    pub distance: f32,
    pub length: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct VehicleV5 {
    pub pos: [f32; 2],
    pub heading: f32,
    pub segment: Option<SegmentId>,
    pub progress: f32,
    pub forward: bool,
    pub speed: f32,
    pub route: Vec<SegmentId>,
    pub destination: Option<NodeId>,
    pub household: Option<HouseholdId>,
    pub seed: u32,
    pub lane: u8,
    pub lane_shift: f32,
    pub junction: Option<JunctionMoveV5>,
    pub braking: bool,
    pub go: bool,
}

impl VehicleV5 {
    pub(crate) fn upgrade(self) -> Vehicle {
        Vehicle {
            pos: self.pos,
            heading: self.heading,
            segment: self.segment,
            progress: self.progress,
            forward: self.forward,
            speed: self.speed,
            route: self.route,
            destination: self.destination,
            household: self.household,
            seed: self.seed,
            lane: self.lane,
            lane_shift: self.lane_shift,
            junction: self.junction.map(|j| JunctionMove {
                node: j.node,
                from: j.from,
                to: j.to,
                to_forward: j.to_forward,
                movement: j.movement,
                turn: j.turn,
                from_lane: j.from_lane,
                to_lane: j.to_lane,
                distance: j.distance,
                length: j.length,
            }),
            braking: self.braking,
            go: self.go,
        }
    }

    pub(crate) fn of(v: &Vehicle) -> VehicleV5 {
        VehicleV5 {
            pos: v.pos,
            heading: v.heading,
            segment: v.segment,
            progress: v.progress,
            forward: v.forward,
            speed: v.speed,
            route: v.route.clone(),
            destination: v.destination,
            household: v.household,
            seed: v.seed,
            lane: v.lane,
            lane_shift: v.lane_shift,
            junction: v.junction.map(|j| JunctionMoveV5 {
                node: j.node,
                from: j.from,
                to: j.to,
                to_forward: j.to_forward,
                movement: j.movement,
                turn: j.turn,
                from_lane: j.from_lane,
                to_lane: j.to_lane,
                distance: j.distance,
                length: j.length,
            }),
            braking: v.braking,
            go: v.go,
        }
    }
}

pub(crate) fn upgrade_vehicles(v: IdMap<VehicleId, VehicleV5>) -> IdMap<VehicleId, Vehicle> {
    v.map_values(VehicleV5::upgrade)
}

pub(crate) fn vehicles_of(v: &IdMap<VehicleId, Vehicle>) -> IdMap<VehicleId, VehicleV5> {
    v.clone().map_values(|x| VehicleV5::of(&x))
}

// ---------------------------------------------------------------- growth ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct GrowthStateV5 {
    pub acc: [f32; 3],
    pub move_in_acc: f32,
    pub meta: SideTable<BuildingId, BuildingMeta>,
    pub abandoned_total: u32,
    pub attractiveness: f32,
    pub legacy_lots_merged: u32,
}

impl GrowthStateV5 {
    pub(crate) fn upgrade(self) -> GrowthState {
        GrowthState {
            acc: self.acc,
            move_in_acc: self.move_in_acc,
            meta: self.meta,
            abandoned_total: self.abandoned_total,
            attractiveness: self.attractiveness,
            legacy_lots_merged: self.legacy_lots_merged,
        }
    }

    pub(crate) fn of(g: &GrowthState) -> GrowthStateV5 {
        GrowthStateV5 {
            acc: g.acc,
            move_in_acc: g.move_in_acc,
            meta: g.meta.clone(),
            abandoned_total: g.abandoned_total,
            attractiveness: g.attractiveness,
            legacy_lots_merged: g.legacy_lots_merged,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct VersionsV5 {
    pub roads: u64,
    pub grid: u64,
    pub buildings: u64,
}

impl VersionsV5 {
    /// Live counters, bumped so every renderer rebuilds.
    pub(crate) fn upgrade(self) -> Versions {
        Versions {
            roads: self.roads + 1,
            grid: self.grid + 1,
            buildings: self.buildings + 1,
        }
    }

    pub(crate) fn of(v: &Versions) -> VersionsV5 {
        VersionsV5 {
            roads: v.roads,
            grid: v.grid,
            buildings: v.buildings,
        }
    }
}
