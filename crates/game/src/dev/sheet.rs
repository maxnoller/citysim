//! `CITYSIM_DEMO=models`: the model sheet. An empty map with a small road
//! junction (an avenue crossing an avenue with signals and a street, so
//! lamps, signals and road furniture show) and, drawn by gfx
//! (`gfx/model_sheet.rs`), every generated model lined up on the grass:
//! each building kind at each level and lot size in several seeds (three
//! blocks of rows, one per level of detail), every vehicle, trees, props
//! and building sites.
//!
//! This module is the layout both sides follow (pure data) and the named
//! camera views `CITYSIM_STILLS_LIST` accepts (`sheet`, `res`, `com`, `ind`,
//! `r1`..`i2`, `cars`, `green`, `props`, `sites`, `junction`, the `-far` /
//! `-mid` variants of the building views, ...): see [`view`].

// The layout is read by gfx: a ui-only build uses only the views.
#![cfg_attr(not(feature = "gfx"), allow(dead_code))]

use bevy::prelude::*;
use citysim_sim::{Cell, Command, RoadKind, World, Zone, scenarios};

use crate::shared::CameraPose;

/// Seeds shown per row (model variants `0..SEEDS`).
pub const SEEDS: u8 = 8;
/// Spacing of the slots along a row (metres, slot centre to centre).
pub const COL: f32 = 26.0;
/// Spacing of the rows (metres).
pub const ROW: f32 = 30.0;
/// Where the rows of the detailed models start (centre of the first slot).
pub const ORIGIN: Vec2 = Vec2::new(420.0, 330.0);
/// The kinds' blocks side by side (x offset per kind).
pub const KIND_DX: f32 = 236.0;
/// The levels of detail one below the other (z offset per level; the
/// detailed models first).
pub const LOD_DZ: f32 = 260.0;
/// Number of levels of detail shown.
pub const LODS: usize = 3;

/// Rows of buildings per kind: (level, lot cells across and deep).
pub fn rows(kind: Zone) -> &'static [(u8, (u8, u8))] {
    match kind {
        Zone::Residential | Zone::Commercial => &[
            (1, (1, 1)),
            (1, (1, 2)),
            (1, (2, 2)),
            (2, (1, 2)),
            (2, (2, 2)),
            (3, (1, 2)),
            (3, (2, 2)),
        ],
        Zone::Industrial => &[(1, (1, 2)), (1, (2, 2)), (2, (1, 2)), (2, (2, 2))],
    }
}

fn kind_index(kind: Zone) -> usize {
    match kind {
        Zone::Residential => 0,
        Zone::Commercial => 1,
        Zone::Industrial => 2,
    }
}

/// Centre of a building slot (the lot's centre; fronts face +z).
pub fn slot(kind: Zone, row: usize, seed: u8, lod: usize) -> Vec2 {
    ORIGIN
        + Vec2::new(
            kind_index(kind) as f32 * KIND_DX + seed as f32 * COL,
            row as f32 * ROW + lod as f32 * LOD_DZ,
        )
}

/// The vehicles, green and props, sites: to the right of the buildings.
pub const VEHICLES: Vec2 = Vec2::new(1180.0, 330.0);
/// Spacing of vehicles along a row and of the rows.
pub const VEH_COL: f32 = 3.2;
pub const VEH_ROW: f32 = 7.5;
/// The distant models of the vehicles: this far to the right.
pub const VEH_FAR_DX: f32 = 40.0;
/// Trees, bushes, hedges.
pub const GREEN: Vec2 = Vec2::new(1180.0, 470.0);
/// Yard and street props.
pub const PROPS: Vec2 = Vec2::new(1180.0, 560.0);
/// Building sites (stage 0 and 1 of a few lots).
pub const SITES: Vec2 = Vec2::new(1180.0, 650.0);

/// The junction's centre (sim metres): avenues along row 30 and column 70,
/// a street crossing the row-30 avenue at column 55.
pub const JUNCTION: Vec2 = Vec2::new(70.5 * 8.0, 30.5 * 8.0);

/// Area the sheet covers (gfx keeps scattered trees out of it).
pub const AREA: (Vec2, Vec2) = (Vec2::new(150.0, 120.0), Vec2::new(1500.0, 1150.0));

/// The sheet's world: empty land and the road junction, money as a new game.
pub fn world(seed: u64) -> World {
    let mut w = World::new(seed);
    scenarios::prebuilt(&mut w, |w| {
        let cmds = [
            Command::BuildRoad {
                from: Cell::new(40, 30),
                to: Cell::new(100, 30),
                kind: RoadKind::Avenue,
            },
            Command::BuildRoad {
                from: Cell::new(70, 16),
                to: Cell::new(70, 38),
                kind: RoadKind::Avenue,
            },
            Command::BuildRoad {
                from: Cell::new(55, 18),
                to: Cell::new(55, 38),
                kind: RoadKind::Street,
            },
        ];
        for c in cmds {
            if let Err(e) = w.apply(c.clone()) {
                warn!("model sheet: refused {c:?}: {e}");
            }
        }
    });
    w
}

fn pose(x: f32, z: f32, yaw: f32, pitch: f32, distance: f32) -> CameraPose {
    CameraPose {
        focus: Vec3::new(x, 0.0, z),
        yaw,
        pitch,
        distance,
    }
}

/// A named camera view of the sheet. `NAME-mid` / `NAME-far` show the
/// same buildings in the middle / distant level of detail (seen from the
/// same distance, to compare).
pub fn view(name: &str) -> Option<CameraPose> {
    let (base, lod) = match name.rsplit_once('-') {
        Some((b, "mid")) => (b, 1),
        Some((b, "far")) => (b, 2),
        _ => (name, 0),
    };
    let dz = lod as f32 * LOD_DZ;
    let block = |kind: Zone| {
        let n = rows(kind).len();
        let c = (slot(kind, 0, 0, lod) + slot(kind, n - 1, SEEDS - 1, lod)) * 0.5;
        pose(c.x, c.y + 10.0, 0.32, 0.62, 300.0)
    };
    let row = |kind: Zone, level: u8, cells: (u8, u8)| {
        let r = rows(kind).iter().position(|&x| x == (level, cells))?;
        let a = slot(kind, r, 0, lod);
        let b = slot(kind, r, 3, lod);
        Some(pose((a.x + b.x) * 0.5, a.y + 2.0, 0.42, 0.42, 72.0))
    };
    Some(match base {
        "sheet" => pose(830.0, 640.0, 0.2, 0.95, 820.0),
        "res" => block(Zone::Residential),
        "com" => block(Zone::Commercial),
        "ind" => block(Zone::Industrial),
        "r1" => row(Zone::Residential, 1, (1, 2))?,
        "r1w" => row(Zone::Residential, 1, (2, 2))?,
        "r2" => row(Zone::Residential, 2, (1, 2))?,
        "r2w" => row(Zone::Residential, 2, (2, 2))?,
        "r3" => row(Zone::Residential, 3, (1, 2))?,
        "r3w" => row(Zone::Residential, 3, (2, 2))?,
        "c1" => row(Zone::Commercial, 1, (1, 2))?,
        "c1w" => row(Zone::Commercial, 1, (2, 2))?,
        "c2" => row(Zone::Commercial, 2, (1, 2))?,
        "c2w" => row(Zone::Commercial, 2, (2, 2))?,
        "c3" => row(Zone::Commercial, 3, (1, 2))?,
        "c3w" => row(Zone::Commercial, 3, (2, 2))?,
        "i1" => row(Zone::Industrial, 1, (1, 2))?,
        "i1w" => row(Zone::Industrial, 1, (2, 2))?,
        "i2" => row(Zone::Industrial, 2, (1, 2))?,
        "i2w" => row(Zone::Industrial, 2, (2, 2))?,
        "cars" => pose(
            VEHICLES.x + 9.0 + dz / LOD_DZ * VEH_FAR_DX * 0.5,
            VEHICLES.y + 22.0,
            0.3,
            0.5,
            48.0,
        ),
        "cars-close" => pose(VEHICLES.x + 5.0, VEHICLES.y + 4.0, 0.45, 0.32, 16.0),
        "trucks" => pose(
            VEHICLES.x + 9.0 + dz / LOD_DZ * VEH_FAR_DX * 0.5,
            VEHICLES.y + 7.0 * VEH_ROW,
            0.45,
            0.45,
            30.0,
        ),
        "green" => pose(GREEN.x + 30.0, GREEN.y + 10.0, 0.35, 0.5, 70.0),
        "props" => pose(PROPS.x + 30.0, PROPS.y + 8.0, 0.35, 0.55, 60.0),
        "sites" => pose(SITES.x + 40.0, SITES.y + 10.0, 0.35, 0.6, 110.0),
        "junction" => pose(JUNCTION.x - 10.0, JUNCTION.y + 6.0, 0.6, 0.42, 60.0),
        "street" => pose(JUNCTION.x - 120.0, JUNCTION.y, 0.9, 0.3, 50.0),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_view_resolves_and_slots_stay_in_the_sheet() {
        for v in [
            "sheet", "res", "com-mid", "ind-far", "r1", "c3w-far", "i2", "cars", "junction",
        ] {
            assert!(view(v).is_some(), "{v}");
        }
        assert!(view("nope").is_none());
        for kind in Zone::ALL {
            for r in 0..rows(kind).len() {
                for lod in 0..LODS {
                    let p = slot(kind, r, SEEDS - 1, lod);
                    assert!(p.cmpge(AREA.0).all() && p.cmple(AREA.1).all(), "{kind:?} {r} {lod} {p}");
                }
            }
        }
        let w = world(1);
        assert!(w.roads.segments.len() >= 5, "{}", w.roads.segments.len());
    }
}
