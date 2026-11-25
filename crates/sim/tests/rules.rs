//! Road, zoning, bulldoze, preview and save/load rules.

use citysim_sim::*;

fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

fn road(w: &mut World, a: (i32, i32), b: (i32, i32)) -> Result<Outcome, CommandError> {
    w.apply(Command::BuildRoad {
        from: c(a.0, a.1),
        to: c(b.0, b.1),
        kind: citysim_sim::RoadKind::Street,
    })
}

/// World without the highway stub, for exact graph counts.
fn blank() -> World {
    let mut w = World::new(1);
    w.roads = RoadNetwork::new();
    for cd in &mut w.grid.cells {
        cd.occupant = Occupant::Empty;
    }
    w
}

fn streets(w: &World) -> usize {
    w.roads.segments.values().filter(|s| s.kind == RoadKind::Street).count()
}

fn degree_at(w: &World, cell: Cell) -> Option<usize> {
    w.roads.node_at(cell.center()).map(|n| w.roads.nodes[n].segments.len())
}

/// Invariants restricted to a window (the full check is O(map * graph)).
fn check(w: &World) {
    w.roads.validate().unwrap();
    for y in 0..40 {
        for x in 0..40 {
            let cell = c(x, y);
            let covered = w.roads.covers(cell.center());
            assert_eq!(w.grid.is_road(cell), covered, "grid/graph mismatch at {cell:?}");
            if covered {
                assert!(w.grid.get(cell).unwrap().zone.is_none());
            }
        }
    }
}

#[test]
fn new_world_has_highway_stub() {
    let w = World::new(42);
    w.check_invariants().unwrap();
    assert_eq!(w.roads.segments.len(), 1);
    let seg = w.roads.segments.values().next().unwrap();
    assert_eq!(seg.kind, RoadKind::Highway);
    assert_eq!(seg.length, (balance::HIGHWAY_STUB_CELLS - 1) as f32 * CELL_SIZE);
    let entry = w.roads.entry.unwrap();
    assert_eq!(w.roads.nodes[entry].pos, World::highway_entry_cell().center());
    assert_eq!(World::highway_entry_cell().x, 0);
    assert!(w.grid.is_road(c(0, 128)) && w.grid.is_road(c(11, 128)) && !w.grid.is_road(c(12, 128)));
    assert_eq!(w.economy.money, balance::STARTING_MONEY);
    assert_eq!(w.stats.road_cells, balance::HIGHWAY_STUB_CELLS as u32);
}

#[test]
fn coordinate_convention() {
    assert_eq!(c(0, 0).center(), [4.0, 4.0]);
    assert_eq!(c(3, 10).center(), [28.0, 84.0]);
    assert_eq!(Cell::from_pos([28.0, 84.0]), c(3, 10));
    assert_eq!(MAP_CELLS, 256);
}

#[test]
fn straight_road_cost_and_cells() {
    let mut w = blank();
    let out = road(&mut w, (2, 5), (10, 5)).unwrap();
    assert_eq!(out.cost, 8 * balance::ROAD_COST_PER_CELL);
    assert_eq!(out.cells.len(), 9);
    assert_eq!(w.economy.money, balance::STARTING_MONEY - out.cost);
    assert_eq!(w.roads.segments.len(), 1);
    assert_eq!(w.roads.nodes.len(), 2);
    let seg = w.roads.segments.values().next().unwrap();
    assert_eq!(seg.length, 64.0);
    assert_eq!(seg.speed_limit, balance::STREET_SPEED_LIMIT);
    assert!(seg.capacity[0] > 0.0 && seg.capacity[0] == seg.capacity[1]);
    for x in 2..=10 {
        assert!(w.grid.is_road(c(x, 5)));
    }
    check(&w);
}

#[test]
fn vertical_and_reversed_roads() {
    let mut w = blank();
    road(&mut w, (5, 10), (5, 2)).unwrap();
    assert_eq!(w.roads.segments.len(), 1);
    assert!(w.grid.is_road(c(5, 2)) && w.grid.is_road(c(5, 10)));
    check(&w);
}

#[test]
fn invalid_roads_rejected() {
    let mut w = blank();
    assert_eq!(road(&mut w, (2, 2), (5, 3)), Err(CommandError::NotStraight));
    assert_eq!(road(&mut w, (2, 2), (2, 2)), Err(CommandError::ZeroLength));
    assert_eq!(road(&mut w, (-1, 2), (5, 2)), Err(CommandError::OutOfBounds));
    assert_eq!(road(&mut w, (250, 2), (256, 2)), Err(CommandError::OutOfBounds));
    assert_eq!(road(&mut w, (0, 0), (0, MAP_CELLS)), Err(CommandError::OutOfBounds));
    assert!(road(&mut w, (0, 0), (MAP_CELLS - 1, MAP_CELLS - 1)).is_ok());
    assert_eq!(w.roads.segments.len(), 1);
}

#[test]
fn collinear_extension_merges() {
    let mut w = blank();
    road(&mut w, (0, 3), (5, 3)).unwrap();
    road(&mut w, (5, 3), (9, 3)).unwrap();
    assert_eq!(w.roads.segments.len(), 1);
    assert_eq!(w.roads.nodes.len(), 2);
    assert_eq!(w.roads.segments.values().next().unwrap().length, 72.0);
    check(&w);
}

#[test]
fn crossing_creates_four_way_intersection() {
    let mut w = blank();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    road(&mut w, (5, 0), (5, 10)).unwrap();
    assert_eq!(w.roads.segments.len(), 4);
    assert_eq!(w.roads.nodes.len(), 5);
    assert_eq!(degree_at(&w, c(5, 5)), Some(4));
    let total: f32 = w.roads.segments.values().map(|s| s.length).sum();
    assert_eq!(total, 160.0);
    check(&w);
}

#[test]
fn crossing_cost_counts_only_new_pieces() {
    let mut w = blank();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    let before = w.economy.money;
    let out = road(&mut w, (5, 0), (5, 10)).unwrap();
    assert_eq!(out.cost, 10 * balance::ROAD_COST_PER_CELL);
    assert_eq!(before - w.economy.money, out.cost);
}

#[test]
fn t_junction_splits_existing_segment() {
    let mut w = blank();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    let original = *w.roads.segments.keys().collect::<Vec<_>>().first().unwrap();
    road(&mut w, (4, 5), (4, 12)).unwrap();
    assert_eq!(w.roads.segments.len(), 3);
    assert_eq!(degree_at(&w, c(4, 5)), Some(3));
    // The split keeps the original id for one half.
    assert!(w.roads.segments.contains_key(original));
    check(&w);
    // And from the other side, ending on the road.
    road(&mut w, (7, 0), (7, 5)).unwrap();
    assert_eq!(degree_at(&w, c(7, 5)), Some(3));
    assert_eq!(w.roads.segments.len(), 5);
    check(&w);
}

#[test]
fn road_ending_at_existing_endpoint_reuses_node() {
    let mut w = blank();
    road(&mut w, (0, 0), (5, 0)).unwrap();
    road(&mut w, (5, 0), (5, 5)).unwrap();
    assert_eq!(w.roads.nodes.len(), 3);
    assert_eq!(degree_at(&w, c(5, 0)), Some(2));
    check(&w);
}

#[test]
fn overlap_is_not_duplicated() {
    let mut w = blank();
    road(&mut w, (2, 5), (8, 5)).unwrap();
    let v = w.versions.roads;
    // Fully contained: nothing to build, free, no version bump.
    let out = road(&mut w, (3, 5), (6, 5)).unwrap();
    assert_eq!(out.cost, 0);
    assert_eq!(w.versions.roads, v);
    assert_eq!(w.roads.segments.len(), 1);
    // Partial overlap on both sides: only the new pieces are built and paid.
    let out = road(&mut w, (0, 5), (12, 5)).unwrap();
    assert_eq!(out.cost, 6 * balance::ROAD_COST_PER_CELL);
    assert_eq!(w.roads.segments.len(), 1);
    assert_eq!(w.roads.segments.values().next().unwrap().length, 96.0);
    check(&w);
}

#[test]
fn overlap_through_intersection() {
    let mut w = blank();
    road(&mut w, (5, 0), (5, 10)).unwrap();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    assert_eq!(w.roads.segments.len(), 4);
    check(&w);
}

#[test]
fn parallel_adjacent_roads_stay_separate() {
    let mut w = blank();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    road(&mut w, (0, 6), (10, 6)).unwrap();
    assert_eq!(w.roads.segments.len(), 2);
    assert_eq!(w.roads.nodes.len(), 4);
    // A one-piece connector between them makes two T-junctions.
    road(&mut w, (4, 5), (4, 6)).unwrap();
    assert_eq!(w.roads.segments.len(), 5);
    assert_eq!(degree_at(&w, c(4, 5)), Some(3));
    assert_eq!(degree_at(&w, c(4, 6)), Some(3));
    check(&w);
}

#[test]
fn diagonal_road() {
    let mut w = blank();
    let out = road(&mut w, (1, 1), (6, 6)).unwrap();
    assert_eq!(out.cells.len(), 6);
    assert_eq!(out.cost, 5 * balance::ROAD_COST_PER_CELL);
    let seg = w.roads.segments.values().next().unwrap();
    assert!((seg.length - 5.0 * CELL_SIZE * 2f32.sqrt()).abs() < 1e-3);
    assert!(w.grid.is_road(c(3, 3)) && !w.grid.is_road(c(3, 4)));
    // Anti-diagonal direction works too.
    road(&mut w, (20, 0), (10, 10)).unwrap();
    check(&w);
}

#[test]
fn diagonals_crossing_at_a_cell_centre_intersect() {
    let mut w = blank();
    road(&mut w, (0, 0), (4, 4)).unwrap();
    road(&mut w, (0, 4), (4, 0)).unwrap();
    assert_eq!(degree_at(&w, c(2, 2)), Some(4));
    assert_eq!(w.roads.segments.len(), 4);
    check(&w);
}

#[test]
fn diagonals_crossing_between_centres_rejected() {
    let mut w = blank();
    road(&mut w, (0, 0), (5, 5)).unwrap();
    // (0,5)->(5,0) has odd parity: it would cross between centres.
    assert_eq!(
        road(&mut w, (0, 5), (5, 0)),
        Err(CommandError::DiagonalCrossing(c(2, 3)))
    );
    // The minimal X inside one 2x2 block.
    let mut w = blank();
    road(&mut w, (3, 3), (4, 4)).unwrap();
    assert_eq!(
        road(&mut w, (4, 3), (3, 4)),
        Err(CommandError::DiagonalCrossing(c(4, 3)))
    );
    assert_eq!(
        road(&mut w, (3, 4), (4, 3)),
        Err(CommandError::DiagonalCrossing(c(3, 4)))
    );
    assert_eq!(w.roads.segments.len(), 1);
    // Preview reports the same error.
    assert!(
        w.preview(&Command::BuildRoad {
            from: c(4, 3),
            to: c(3, 4),
            kind: citysim_sim::RoadKind::Street,
        })
        .is_err()
    );
}

#[test]
fn diagonal_meets_straight_road_at_centre() {
    let mut w = blank();
    road(&mut w, (0, 3), (8, 3)).unwrap();
    road(&mut w, (0, 0), (6, 6)).unwrap();
    assert_eq!(degree_at(&w, c(3, 3)), Some(4));
    road(&mut w, (5, 0), (5, 8)).unwrap();
    assert_eq!(degree_at(&w, c(5, 5)), Some(4));
    assert_eq!(degree_at(&w, c(5, 3)), Some(4));
    check(&w);
}

#[test]
fn road_blocked_by_building() {
    let mut w = blank();
    road(&mut w, (0, 0), (10, 0)).unwrap();
    let id = w.buildings.insert(Building {
        kind: Zone::Residential,
        footprint: citysim_sim::Footprint::world(CellRect::single(c(5, 1)), Facing::MinusY),
        level: 1,
        capacity: 4,
        occupied: 0,
        facing: Facing::MinusY,
        seed: 7,
        created_tick: 0,
    });
    w.grid.get_mut(c(5, 1)).unwrap().occupant = Occupant::Building(id);
    assert_eq!(
        road(&mut w, (5, 0), (5, 5)),
        Err(CommandError::BlockedByBuilding(c(5, 1)))
    );
    assert_eq!(
        road(&mut w, (0, 1), (8, 1)),
        Err(CommandError::BlockedByBuilding(c(5, 1)))
    );
    assert!(road(&mut w, (6, 0), (6, 5)).is_ok());
}

#[test]
fn road_clears_zoning() {
    let mut w = blank();
    road(&mut w, (0, 0), (10, 0)).unwrap();
    w.apply(Command::SetZone {
        min: c(0, 1),
        max: c(10, 2),
        zone: Some(Zone::Residential),
    })
    .unwrap();
    assert_eq!(w.grid.get(c(4, 2)).unwrap().zone, Some(Zone::Residential));
    road(&mut w, (4, 0), (4, 6)).unwrap();
    assert_eq!(w.grid.get(c(4, 2)).unwrap().zone, None);
    assert_eq!(w.grid.get(c(4, 2)).unwrap().occupant, Occupant::Road);
    assert_eq!(w.grid.get(c(3, 2)).unwrap().zone, Some(Zone::Residential));
    check(&w);
}

#[test]
fn insufficient_funds() {
    let mut w = blank();
    let per = balance::ROAD_COST_PER_CELL;
    w.economy.money = 2 * per + per / 2;
    let cmd = Command::BuildRoad {
        from: c(0, 0),
        to: c(5, 0),
        kind: citysim_sim::RoadKind::Street,
    };
    let err = CommandError::InsufficientFunds {
        cost: 5 * per,
        money: 2 * per + per / 2,
    };
    assert_eq!(w.preview(&cmd), Err(err.clone()));
    assert_eq!(w.apply(cmd), Err(err));
    assert!(w.roads.segments.is_empty());
    assert_eq!(w.economy.money, 2 * per + per / 2);
    // Exactly affordable is fine.
    assert!(road(&mut w, (0, 0), (2, 0)).is_ok());
    assert_eq!(w.economy.money, per / 2);
}

#[test]
fn bulldozing_is_allowed_in_debt() {
    let mut w = blank();
    road(&mut w, (0, 0), (8, 0)).unwrap();
    w.economy.money = -5_000;
    assert!(road(&mut w, (0, 2), (3, 2)).is_err());
    let cmd = Command::Bulldoze {
        min: c(3, 0),
        max: c(5, 0),
    };
    let cost = w.preview(&cmd).unwrap().cost;
    assert!(cost > 0);
    assert_eq!(w.apply(cmd).unwrap().cost, cost);
    assert_eq!(w.economy.money, -5_000 - cost);
    check(&w);
}

#[test]
fn zoning_needs_nearby_road() {
    let mut w = blank();
    road(&mut w, (10, 10), (20, 10)).unwrap();
    let v = w.versions.grid;
    let out = w
        .apply(Command::SetZone {
            min: c(10, 5),
            max: c(12, 15),
            zone: Some(Zone::Commercial),
        })
        .unwrap();
    // Rows 8, 9, 11, 12 are within 2 cells; row 10 is road; others too far.
    assert_eq!(out.cells.len(), 3 * 4);
    assert_eq!(out.cost, 0);
    assert!(w.versions.grid > v);
    assert_eq!(w.grid.get(c(11, 8)).unwrap().zone, Some(Zone::Commercial));
    assert_eq!(w.grid.get(c(11, 7)).unwrap().zone, None);
    assert_eq!(w.grid.get(c(11, 10)).unwrap().zone, None);
    // Corner reach is Chebyshev: (8, 8) is 2 from (10, 10).
    let out = w
        .apply(Command::SetZone {
            min: c(7, 7),
            max: c(8, 8),
            zone: Some(Zone::Industrial),
        })
        .unwrap();
    assert_eq!(out.cells, vec![c(8, 8)]);
    // Re-zoning the same zone changes nothing.
    let out = w
        .apply(Command::SetZone {
            min: c(10, 8),
            max: c(12, 9),
            zone: Some(Zone::Commercial),
        })
        .unwrap();
    assert!(out.cells.is_empty());
    // Changing zone type is allowed.
    let out = w
        .apply(Command::SetZone {
            min: c(10, 8),
            max: c(12, 9),
            zone: Some(Zone::Residential),
        })
        .unwrap();
    assert_eq!(out.cells.len(), 6);
    check(&w);
}

#[test]
fn zoning_rectangle_any_corner_order_and_clamped() {
    let mut w = blank();
    road(&mut w, (0, 1), (5, 1)).unwrap();
    let out = w
        .apply(Command::SetZone {
            min: c(5, 0),
            max: c(-3, 0),
            zone: Some(Zone::Residential),
        })
        .unwrap();
    assert_eq!(out.cells.len(), 6);
    assert_eq!(
        w.apply(Command::SetZone {
            min: c(-5, -5),
            max: c(-1, -1),
            zone: Some(Zone::Residential)
        }),
        Err(CommandError::OutOfBounds)
    );
}

#[test]
fn unzoning() {
    let mut w = blank();
    road(&mut w, (0, 1), (5, 1)).unwrap();
    w.apply(Command::SetZone {
        min: c(0, 0),
        max: c(5, 3),
        zone: Some(Zone::Industrial),
    })
    .unwrap();
    let out = w
        .apply(Command::SetZone {
            min: c(0, 0),
            max: c(2, 3),
            zone: None,
        })
        .unwrap();
    assert_eq!(out.cells.len(), 9);
    assert_eq!(w.grid.get(c(1, 0)).unwrap().zone, None);
    assert_eq!(w.grid.get(c(4, 0)).unwrap().zone, Some(Zone::Industrial));
}

#[test]
fn bulldoze_middle_of_road_splits_it() {
    let mut w = blank();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    let out = w
        .apply(Command::Bulldoze {
            min: c(5, 5),
            max: c(5, 5),
        })
        .unwrap();
    assert_eq!(out.cells, vec![c(5, 5)]);
    assert_eq!(out.cost, balance::BULLDOZE_COST_PER_CELL);
    assert_eq!(w.roads.segments.len(), 2);
    assert!(!w.grid.is_road(c(5, 5)));
    assert!(w.grid.is_road(c(4, 5)) && w.grid.is_road(c(6, 5)));
    check(&w);
}

#[test]
fn bulldoze_intersection_and_cleanup() {
    let mut w = blank();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    road(&mut w, (5, 0), (5, 10)).unwrap();
    w.apply(Command::Bulldoze {
        min: c(5, 5),
        max: c(5, 5),
    })
    .unwrap();
    assert_eq!(w.roads.segments.len(), 4);
    assert_eq!(w.roads.nodes.len(), 8);
    check(&w);
    // Remove a whole arm: its end node disappears.
    w.apply(Command::Bulldoze {
        min: c(0, 4),
        max: c(4, 6),
    })
    .unwrap();
    assert_eq!(w.roads.segments.len(), 3);
    check(&w);
}

#[test]
fn bulldoze_single_piece_removes_both_nodes() {
    let mut w = blank();
    road(&mut w, (3, 3), (4, 3)).unwrap();
    w.apply(Command::Bulldoze {
        min: c(3, 3),
        max: c(3, 3),
    })
    .unwrap();
    assert!(w.roads.segments.is_empty());
    assert!(w.roads.nodes.is_empty());
    assert!(!w.grid.is_road(c(4, 3)));
    check(&w);
}

#[test]
fn bulldoze_t_junction_stem_merges_main_road() {
    let mut w = blank();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    road(&mut w, (5, 5), (5, 10)).unwrap();
    assert_eq!(w.roads.segments.len(), 3);
    w.apply(Command::Bulldoze {
        min: c(5, 6),
        max: c(5, 10),
    })
    .unwrap();
    // The stem's piece touching (5,5) is gone; the main road is whole again.
    assert_eq!(w.roads.segments.len(), 1);
    assert_eq!(w.roads.nodes.len(), 2);
    assert!(w.grid.is_road(c(5, 5)));
    check(&w);
}

#[test]
fn bulldoze_everything_and_empty_area() {
    let mut w = blank();
    road(&mut w, (0, 0), (8, 8)).unwrap();
    road(&mut w, (0, 4), (8, 4)).unwrap();
    let v = w.versions.roads;
    let out = w
        .apply(Command::Bulldoze {
            min: c(30, 30),
            max: c(35, 35),
        })
        .unwrap();
    assert!(out.cells.is_empty() && out.cost == 0);
    assert_eq!(w.versions.roads, v);
    w.apply(Command::Bulldoze {
        min: c(0, 0),
        max: c(8, 8),
    })
    .unwrap();
    assert!(w.roads.segments.is_empty() && w.roads.nodes.is_empty());
    assert!(w.versions.roads > v);
    check(&w);
    assert_eq!(
        w.apply(Command::Bulldoze {
            min: c(300, 0),
            max: c(310, 5)
        }),
        Err(CommandError::OutOfBounds)
    );
}

#[test]
fn highway_cannot_be_bulldozed_but_streets_attached_can() {
    let mut w = World::new(1);
    road(&mut w, (11, 128), (20, 128)).unwrap();
    assert_eq!(w.roads.segments.len(), 2);
    w.apply(Command::Bulldoze {
        min: c(0, 120),
        max: c(30, 135),
    })
    .unwrap();
    assert_eq!(w.roads.segments.len(), 1);
    assert_eq!(w.roads.segments.values().next().unwrap().kind, RoadKind::Highway);
    assert!(w.grid.is_road(c(11, 128)) && !w.grid.is_road(c(12, 128)));
    w.check_invariants().unwrap();
}

#[test]
fn street_does_not_merge_into_highway() {
    let mut w = World::new(1);
    road(&mut w, (11, 128), (30, 128)).unwrap();
    assert_eq!(w.roads.segments.len(), 2);
    let entry = w.roads.entry.unwrap();
    assert_eq!(w.roads.nodes[entry].segments.len(), 1);
    w.check_invariants().unwrap();
}

#[test]
fn bulldoze_removes_buildings_and_keeps_zone() {
    let mut w = blank();
    road(&mut w, (0, 0), (10, 0)).unwrap();
    w.apply(Command::SetZone {
        min: c(0, 1),
        max: c(10, 2),
        zone: Some(Zone::Residential),
    })
    .unwrap();
    let fp = CellRect::from_corners(c(2, 1), c(3, 2));
    let id = w.buildings.insert(Building {
        kind: Zone::Residential,
        footprint: citysim_sim::Footprint::world(fp, Facing::MinusY),
        level: 1,
        capacity: 8,
        occupied: 0,
        facing: Facing::MinusY,
        seed: 7,
        created_tick: 0,
    });
    for cell in fp.cells() {
        w.grid.get_mut(cell).unwrap().occupant = Occupant::Building(id);
    }
    let vb = w.versions.buildings;
    let out = w
        .apply(Command::Bulldoze {
            min: c(3, 2),
            max: c(3, 2),
        })
        .unwrap();
    assert_eq!(out.cells.len(), 4);
    assert!(w.buildings.is_empty());
    assert!(w.versions.buildings > vb);
    assert_eq!(w.grid.get(c(2, 1)).unwrap().occupant, Occupant::Empty);
    assert_eq!(w.grid.get(c(2, 1)).unwrap().zone, Some(Zone::Residential));
    assert_eq!(w.roads.segments.len(), 1);
    w.check_invariants().unwrap();
}

#[test]
fn preview_matches_apply_and_does_not_mutate() {
    let mut w = World::new(7);
    road(&mut w, (11, 128), (40, 128)).unwrap();
    let cmds = [
        Command::BuildRoad {
            from: c(20, 120),
            to: c(20, 140),
            kind: citysim_sim::RoadKind::Street,
        },
        Command::SetZone {
            min: c(12, 126),
            max: c(40, 130),
            zone: Some(Zone::Residential),
        },
        Command::Bulldoze {
            min: c(30, 128),
            max: c(32, 128),
        },
        Command::SetTaxRate(0.12),
    ];
    for cmd in cmds {
        let before = w.save();
        let preview = w.preview(&cmd).unwrap();
        assert_eq!(w.save(), before, "preview mutated the world");
        let out = w.apply(cmd).unwrap();
        assert_eq!(preview.cost, out.cost);
        assert_eq!(preview.cells, out.cells);
        w.check_invariants().unwrap();
    }
    assert_eq!(w.economy.tax_rate, 0.12);
}

#[test]
fn tax_rate_validation() {
    let mut w = World::new(0);
    assert!(w.apply(Command::SetTaxRate(0.0)).is_ok());
    assert!(w.apply(Command::SetTaxRate(balance::MAX_TAX_RATE)).is_ok());
    assert!(matches!(
        w.apply(Command::SetTaxRate(-0.1)),
        Err(CommandError::InvalidTaxRate(_))
    ));
    assert!(matches!(
        w.apply(Command::SetTaxRate(0.9)),
        Err(CommandError::InvalidTaxRate(_))
    ));
    assert!(matches!(
        w.apply(Command::SetTaxRate(f32::NAN)),
        Err(CommandError::InvalidTaxRate(_))
    ));
    assert_eq!(w.economy.tax_rate, balance::MAX_TAX_RATE);
}

#[test]
fn step_advances_time_and_days() {
    let mut w = World::new(0);
    assert_eq!(w.time.day, 1);
    assert_eq!(w.time.hour(), 9.0);
    w.step();
    assert_eq!(w.time.tick, 1);
    for _ in 0..balance::TICKS_PER_DAY {
        w.step();
    }
    assert_eq!(w.time.day, 2);
    assert_eq!(w.time.tick, balance::TICKS_PER_DAY as u64 + 1);
}

#[test]
fn save_load_round_trip() {
    let mut w = World::new(99);
    road(&mut w, (11, 128), (40, 128)).unwrap();
    road(&mut w, (25, 110), (25, 150)).unwrap();
    road(&mut w, (25, 110), (45, 130)).unwrap();
    w.apply(Command::SetZone {
        min: c(12, 120),
        max: c(45, 140),
        zone: Some(Zone::Commercial),
    })
    .unwrap();
    w.apply(Command::Bulldoze {
        min: c(30, 128),
        max: c(31, 128),
    })
    .unwrap();
    for _ in 0..100 {
        w.step();
    }
    let bytes = w.save();
    let loaded = World::load(&bytes).unwrap();
    assert_eq!(loaded.save(), bytes);
    assert_eq!(loaded, w);
    assert_eq!(loaded.time, w.time);
    assert_eq!(loaded.versions, w.versions);
    assert_eq!(loaded.grid, w.grid);
    loaded.check_invariants().unwrap();
    // The loaded world keeps working and stays in lockstep.
    let (mut a, mut b) = (w, loaded);
    let cmd = Command::BuildRoad {
        from: c(40, 128),
        to: c(40, 100),
        kind: citysim_sim::RoadKind::Street,
    };
    assert_eq!(a.apply(cmd.clone()), b.apply(cmd));
    a.step();
    b.step();
    assert_eq!(a, b);
    assert_eq!(a.save(), b.save());
}

#[test]
fn load_rejects_garbage() {
    assert_eq!(World::load(b"").unwrap_err(), LoadError::BadMagic);
    assert_eq!(World::load(b"NOPE1234").unwrap_err(), LoadError::BadMagic);
    let mut bytes = World::new(1).save();
    bytes[4] = 77;
    assert_eq!(World::load(&bytes).unwrap_err(), LoadError::UnsupportedVersion(77));
    let bytes = World::new(1).save();
    assert!(matches!(
        World::load(&bytes[..bytes.len() / 2]),
        Err(LoadError::Corrupt(_))
    ));
}

#[test]
fn random_edits_keep_graph_valid() {
    let mut w = blank();
    w.economy.money = i64::MAX / 2;
    let mut rng = Rng::new(12345);
    for i in 0..400 {
        let a = c(rng.below(30) as i32, rng.below(30) as i32);
        let len = 1 + rng.below(10) as i32;
        let (dx, dy) = [(1, 0), (0, 1), (1, 1), (1, -1), (-1, 0), (0, -1), (-1, -1), (-1, 1)][rng.below(8) as usize];
        let b = a.offset(dx * len, dy * len);
        let cmd = if rng.below(5) == 0 {
            Command::Bulldoze {
                min: a,
                max: a.offset(rng.below(3) as i32, rng.below(3) as i32),
            }
        } else {
            Command::BuildRoad {
                from: a,
                to: b,
                kind: citysim_sim::RoadKind::Street,
            }
        };
        let preview = w.preview(&cmd);
        let res = w.apply(cmd);
        assert_eq!(
            preview.map(|p| (p.cost, p.cells)),
            res.clone().map(|o| (o.cost, o.cells))
        );
        if let Err(e) = res {
            assert!(
                matches!(e, CommandError::OutOfBounds | CommandError::DiagonalCrossing(_)),
                "step {i}: unexpected {e:?}"
            );
        }
        check(&w);
    }
    assert!(streets(&w) > 10);
}

#[test]
fn per_direction_load_survives_split_and_merge() {
    let mut w = blank();
    road(&mut w, (0, 5), (10, 5)).unwrap();
    let s = w.roads.segments.keys().next().unwrap();
    w.roads.segments[s].load = [4.0, 2.0];
    // T-junction splits the segment at x=5: loads split proportionally.
    road(&mut w, (5, 5), (5, 9)).unwrap();
    let main: Vec<_> = w
        .roads
        .segments
        .values()
        .filter(|g| w.roads.node_pos(g.a)[1] == w.roads.node_pos(g.b)[1])
        .collect();
    let total: [f32; 2] = main
        .iter()
        .fold([0.0, 0.0], |acc, g| [acc[0] + g.load[0], acc[1] + g.load[1]]);
    assert!(
        (total[0] - 4.0).abs() < 1e-4 && (total[1] - 2.0).abs() < 1e-4,
        "{total:?}"
    );
    // Removing the stem merges the halves back; per-direction totals kept.
    w.apply(Command::Bulldoze {
        min: c(5, 6),
        max: c(5, 9),
    })
    .unwrap();
    assert_eq!(w.roads.segments.len(), 1);
    let g = w.roads.segments.values().next().unwrap();
    let (pa, pb) = (w.roads.node_pos(g.a), w.roads.node_pos(g.b));
    let l = if pa[0] < pb[0] { g.load } else { [g.load[1], g.load[0]] };
    assert!((l[0] - 4.0).abs() < 1e-4 && (l[1] - 2.0).abs() < 1e-4, "{l:?}");
    assert!(g.congestion_in(true) >= 0.0 && g.congestion() <= 1.0);
}

#[test]
fn merge_of_opposite_oriented_segments_matches_directions() {
    let mut w = blank();
    // Two roads drawn towards each other: segments point in opposite ways.
    road(&mut w, (0, 3), (5, 3)).unwrap();
    road(&mut w, (10, 3), (6, 3)).unwrap();
    let west = w.roads.piece(c(0, 3), c(1, 3)).unwrap();
    let east = w.roads.piece(c(9, 3), c(10, 3)).unwrap();
    // Load travelling +x on both: index 0 on the west one, 1 on the east one.
    w.roads.segments[west].load = [3.0, 0.0];
    w.roads.segments[east].load = [0.0, 5.0];
    road(&mut w, (5, 3), (6, 3)).unwrap();
    assert_eq!(w.roads.segments.len(), 1);
    let g = w.roads.segments.values().next().unwrap();
    let plus_x = if w.roads.node_pos(g.a)[0] < w.roads.node_pos(g.b)[0] {
        g.load[0]
    } else {
        g.load[1]
    };
    assert!((plus_x - 8.0).abs() < 1e-4, "{:?}", g.load);
    assert!((g.total_load() - 8.0).abs() < 1e-4);
}

#[test]
fn facing_helpers() {
    assert_eq!(Facing::PlusX.offset(), (1, 0));
    assert_eq!(Facing::MinusY.offset(), (0, -1));
    assert!((Facing::PlusY.angle() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert_eq!(Facing::default(), Facing::PlusY);
}

#[test]
fn building_visual_fields_round_trip() {
    let mut w = blank();
    let id = w.buildings.insert(Building {
        kind: Zone::Commercial,
        footprint: citysim_sim::Footprint::world(CellRect::from_corners(c(1, 1), c(2, 3)), Facing::MinusX),
        level: 3,
        capacity: 20,
        occupied: 5,
        facing: Facing::MinusX,
        seed: 0xDEAD_BEEF,
        created_tick: 1234,
    });
    let loaded = World::load(&w.save()).unwrap();
    let b = &loaded.buildings[id];
    assert_eq!(
        (b.facing, b.seed, b.created_tick, b.size()),
        (Facing::MinusX, 0xDEAD_BEEF, 1234, (2, 3))
    );
}
