//! `CITYSIM_DEMO=models`: draws the model sheet — every generated model
//! lined up on the grass of an empty map, at the places `dev::sheet` lays
//! out (and names camera views for): each building kind at each level and
//! lot size in several seeds, once per level of detail; every vehicle
//! (detailed and distant, some braking); trees and bushes; yard and street
//! props; building sites. The models come straight from the generators,
//! without `VisibilityRange`, so every level of detail shows at any
//! distance.

use bevy::mesh::MeshTag;
use bevy::prelude::*;
use citysim_sim::Zone;

use super::building_gen::{self, ModelKey};
use super::city_material::TAG_BRAKE;
use super::lot_props;
use super::mesh_util::MeshBuilder;
use super::noise::Stream;
use super::palette;
use super::props;
use super::scene::WorldMaterials;
use super::trees;
use super::vehicle_gen;
use crate::dev::sheet;

/// The model sheet is on (`CITYSIM_DEMO=models`).
pub fn enabled() -> bool {
    std::env::var("CITYSIM_DEMO").is_ok_and(|v| v == "models")
}

/// `p` (sim metres) lies on the sheet (scattered trees keep out).
pub fn covers(p: Vec2) -> bool {
    enabled() && p.cmpge(sheet::AREA.0).all() && p.cmple(sheet::AREA.1).all()
}

pub struct ModelSheetPlugin;

impl Plugin for ModelSheetPlugin {
    fn build(&self, app: &mut App) {
        if enabled() {
            app.add_systems(Update, spawn_sheet);
        }
    }
}

fn at(p: Vec2) -> Transform {
    Transform::from_xyz(p.x, 0.0, p.y)
}

fn spawn_sheet(
    mut commands: Commands,
    mut done: Local<bool>,
    mats: Option<Res<WorldMaterials>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Some(mats) = mats else { return };
    if *done {
        return;
    }
    *done = true;
    let mut put = |m: MeshBuilder, tf: Transform, tag: u32| {
        if m.is_empty() {
            return;
        }
        commands.spawn((
            Mesh3d(meshes.add(m.build())),
            MeshMaterial3d(mats.lit.clone()),
            MeshTag::new(tag),
            tf,
        ));
    };
    // Buildings: kinds side by side, levels of detail one below the other.
    for kind in Zone::ALL {
        for (r, &(level, cells)) in sheet::rows(kind).iter().enumerate() {
            for seed in 0..sheet::SEEDS {
                let key = ModelKey {
                    kind,
                    cells,
                    level,
                    variant: seed,
                };
                for lod in 0..sheet::LODS {
                    if let Some(m) = building_gen::generate_lod(key, lod) {
                        put(m, at(sheet::slot(kind, r, seed, lod)), 0);
                    }
                }
            }
        }
    }
    // Vehicles: one row per body, detailed then distant; every other one
    // with its brake lights on.
    for (row, models) in vehicle_gen::sheet_models().into_iter().enumerate() {
        for (i, (near, far)) in models.into_iter().enumerate() {
            let p = sheet::VEHICLES + Vec2::new(i as f32 * sheet::VEH_COL, row as f32 * sheet::VEH_ROW);
            let brake = if i % 2 == 1 { TAG_BRAKE } else { 0 };
            // Noses towards +z (the views look from there).
            let yaw = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
            put(near, at(p).with_rotation(yaw), brake);
            put(far, at(p + Vec2::X * sheet::VEH_FAR_DX).with_rotation(yaw), brake);
        }
    }
    // Trees and green.
    for (i, (near, far)) in trees::sheet_models().into_iter().enumerate() {
        let p = sheet::GREEN + Vec2::new((i % 12) as f32 * 7.0, (i / 12) as f32 * 16.0);
        put(near, at(p), 0);
        put(far, at(p + Vec2::new(0.0, 8.0)), 0);
    }
    // Props.
    let mut rng = Stream::new(7);
    let mut slot = 0;
    let mut next = |m: MeshBuilder| {
        let p = sheet::PROPS + Vec2::new((slot % 10) as f32 * 7.0, (slot / 10) as f32 * 8.0);
        slot += 1;
        (m, at(p))
    };
    let mut items: Vec<(MeshBuilder, Transform)> = Vec::new();
    for k in 0..4 {
        let mut m = MeshBuilder::with_ground_ao(0.25);
        lot_props::shed(&mut m, &mut rng, Vec2::ZERO, if k % 2 == 0 { 1.0 } else { -1.0 });
        items.push(next(m));
    }
    for _ in 0..2 {
        let mut m = MeshBuilder::new();
        lot_props::patio(&mut m, &mut rng, -1.5, -1.2, 1.5, 1.2);
        items.push(next(m));
    }
    {
        let mut m = MeshBuilder::new();
        lot_props::washing_line(&mut m, &mut rng, Vec2::new(-2.5, 0.0), Vec2::new(2.5, 0.4));
        items.push(next(m));
        let mut m = MeshBuilder::new();
        lot_props::veg_beds(&mut m, &mut rng, -1.5, -1.2, 1.5, 1.2);
        items.push(next(m));
        let mut m = MeshBuilder::new();
        lot_props::flower_bed(&mut m, &mut rng, -1.5, 1.5, -0.3, 0.3);
        items.push(next(m));
        let mut m = MeshBuilder::new();
        lot_props::bins(&mut m, &mut rng, Vec2::ZERO);
        items.push(next(m));
    }
    for _ in 0..2 {
        let mut m = MeshBuilder::new();
        lot_props::pallets(&mut m, &mut rng, Vec2::ZERO);
        items.push(next(m));
    }
    {
        let mut m = MeshBuilder::new();
        lot_props::cafe_seating(&mut m, &mut rng, -3.0, 3.0, 0.0, palette::ACCENTS[0]);
        items.push(next(m));
        let mut m = MeshBuilder::new();
        lot_props::solar_panels(
            &mut m,
            Vec3::new(-1.5, 0.3, 0.0),
            Vec3::X * 3.0,
            Vec3::new(0.0, 1.2, -1.2),
            Vec3::new(0.0, 0.7, 0.7).normalize(),
        );
        items.push(next(m));
        let mut m = MeshBuilder::new();
        lot_props::parked_car(&mut m, &mut rng, Vec3::ZERO, 0.0);
        items.push(next(m));
        let mut m = MeshBuilder::new();
        for k in 0..3 {
            props::crate_box(&mut m, Vec3::new(k as f32 * 1.0 - 1.0, 0.0, 0.0), 0.85, 0.1 * k as f32);
        }
        items.push(next(m));
    }
    items.extend(lot_props::sheet_extras().into_iter().map(&mut next));
    for (m, tf) in items {
        put(m, tf, 0);
    }
    // Building sites: foundations and frame of a few lots.
    let sites = [
        (Zone::Residential, (1, 2), 1),
        (Zone::Residential, (2, 2), 3),
        (Zone::Commercial, (1, 2), 3),
        (Zone::Commercial, (2, 2), 3),
        (Zone::Industrial, (2, 2), 2),
    ];
    for (i, &(kind, cells, level)) in sites.iter().enumerate() {
        for stage in 0..2u8 {
            let key = ModelKey {
                kind,
                cells,
                level,
                variant: i as u8,
            };
            let p = sheet::SITES + Vec2::new(i as f32 * 24.0, stage as f32 * 26.0);
            put(building_gen::site(key, stage), at(p), 0);
        }
    }
    info!("model sheet: spawned");
}
