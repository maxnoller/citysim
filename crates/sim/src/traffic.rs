//! Vehicles, pathfinding and congestion. Uses only the road graph.
//!
//! PHASE2: everything here. Note: building or bulldozing roads may split,
//! merge or remove segments, so vehicles must tolerate their `segment` or
//! `route` entries disappearing (check `world.roads.segments.contains_key`).

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::{HouseholdId, NodeId, RoadNetwork, SegmentId, World};

new_key_type! {
    /// Stable id of a [`Vehicle`].
    pub struct VehicleId;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    /// Position in sim metres.
    pub pos: [f32; 2],
    /// Heading in radians, `atan2(dy, dx)` in sim space.
    pub heading: f32,
    /// Segment currently driven on.
    pub segment: Option<SegmentId>,
    /// Metres travelled along `segment` from the node it entered by.
    pub progress: f32,
    /// True when driving from the segment's `a` node towards `b`.
    pub forward: bool,
    /// Current speed, m/s.
    pub speed: f32,
    /// Remaining segments after the current one.
    pub route: Vec<SegmentId>,
    /// Final node of the trip.
    pub destination: Option<NodeId>,
    pub household: Option<HouseholdId>,
}

/// Shortest route (by travel time) from `from` to `to` as a list of segments.
pub fn shortest_path(_roads: &RoadNetwork, _from: NodeId, _to: NodeId) -> Option<Vec<SegmentId>> {
    // PHASE2: Dijkstra / A* over the road graph, cost = length / effective speed.
    None
}

/// Per-tick vehicle movement and segment load bookkeeping.
pub(crate) fn tick(_world: &mut World) {
    // PHASE2: move vehicles along routes, slow down by segment load/capacity,
    // update `RoadSegment::load`, despawn on arrival.
}
