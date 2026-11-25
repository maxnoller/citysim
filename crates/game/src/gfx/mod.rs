//! GFX: everything drawn in 3D, and the camera. Talks to the rest of the
//! game only through `crate::shared` (never import from `crate::ui`).

mod building_gen;
mod buildings;
mod camera;
mod city_material;
mod cursor;
mod daynight;
mod guides;
mod lots;
mod mesh_util;
mod noise;
mod overlay;
mod palette;
mod props;
mod road_geom;
mod road_ghost;
mod roads;
mod scene;
mod signals;
mod stats;
mod trees;
mod vehicle_gen;
mod vehicles;
mod zones;

use bevy::prelude::*;

/// Heights above the ground for flat overlays, to avoid z-fighting.
pub(crate) mod heights {
    pub const ZONE: f32 = 0.045;
    pub const GHOST: f32 = 0.24;
    pub const HIGHLIGHT: f32 = 0.27;
}

pub struct GfxPlugin;

impl Plugin for GfxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            city_material::CityMaterialPlugin,
            scene::ScenePlugin,
            daynight::DayNightPlugin,
            camera::CameraPlugin,
            roads::RoadsPlugin,
            zones::ZonesPlugin,
            buildings::BuildingsPlugin,
            vehicles::VehiclesPlugin,
            cursor::CursorPlugin,
            overlay::OverlayPlugin,
            trees::TreesPlugin,
            stats::StatsPlugin,
            signals::SignalsPlugin,
            lots::LotsPlugin,
            guides::GuidesPlugin,
        ));
    }
}

#[cfg(test)]
mod reload_tests {
    use bevy::prelude::*;
    use citysim_sim::{Cell, Command, World, Zone};

    use super::scene::WorldMaterials;
    use crate::shared::{ActiveTool, GraphicsQuality, OverlayMode, SimWorld};

    type Line = ((i32, i32), (i32, i32));

    fn world(seed: u64, roads: &[Line], zone: (Cell, Cell), days: u32) -> World {
        let mut w = World::new(seed);
        w.economy.money = 1_000_000;
        for &(a, b) in roads {
            w.apply(Command::BuildRoad {
                from: Cell::new(a.0, a.1),
                to: Cell::new(b.0, b.1),
            })
            .expect("road");
        }
        w.apply(Command::SetZone {
            min: zone.0,
            max: zone.1,
            zone: Some(Zone::Residential),
        })
        .expect("zone");
        for _ in 0..citysim_sim::balance::TICKS_PER_DAY * days {
            w.step();
        }
        w
    }

    fn app(w: World) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<super::city_material::CityMaterial>()
            .insert_resource(SimWorld {
                world: w,
                generation: 0,
            })
            .insert_resource(GraphicsQuality::Low)
            .insert_resource(OverlayMode::Zones)
            .insert_resource(ActiveTool::Inspect)
            .add_plugins((
                super::roads::RoadsPlugin,
                super::zones::ZonesPlugin,
                super::buildings::BuildingsPlugin,
                super::vehicles::VehiclesPlugin,
                super::trees::TreesPlugin,
            ));
        let lit = app
            .world_mut()
            .resource_mut::<Assets<super::city_material::CityMaterial>>()
            .add(super::city_material::city_material(StandardMaterial::default()));
        app.insert_resource(WorldMaterials {
            lit: lit.clone(),
            lit_brake: lit.clone(),
            ground: lit,
            sky: Handle::default(),
        });
        app
    }

    /// (road vertices, zone vertices, buildings, vehicles, trees) as drawn.
    fn snapshot(app: &mut App) -> (usize, usize, usize, usize, usize) {
        let w = app.world_mut();
        let mut q = w.query::<(&Name, &Mesh3d, Option<&Visibility>)>();
        let meshes = w.resource::<Assets<Mesh>>();
        let (mut roads, mut zones) = (0, 0);
        // Hidden entities keep their last mesh (Mesh3d is never removed).
        for (name, mesh, _) in q.iter(w).filter(|(_, _, v)| v.is_none_or(|v| *v != Visibility::Hidden)) {
            let n = meshes.get(&mesh.0).map_or(0, |m| m.count_vertices());
            if name.as_str().starts_with("Roads") {
                roads += n;
            } else if name.as_str() == "Zones" {
                zones += n;
            }
        }
        let buildings = w
            .query_filtered::<(), (
                With<super::buildings::BuildingVisual>,
                Without<super::buildings::Shrinking>,
            )>()
            .iter(w)
            .count();
        let vehicles = w
            .query_filtered::<(), With<super::vehicles::VehicleVisual>>()
            .iter(w)
            .count();
        let trees = w.resource::<super::trees::TreeCount>().0;
        (roads, zones, buildings, vehicles, trees)
    }

    fn settle(app: &mut App) {
        for _ in 0..3 {
            app.update();
        }
        // Removed buildings shrink out over a moment of real time.
        std::thread::sleep(std::time::Duration::from_millis(500));
        for _ in 0..2 {
            app.update();
        }
    }

    #[test]
    fn loading_a_world_rebuilds_everything_like_a_fresh_start() {
        let a = world(
            1,
            &[((11, 128), (60, 128)), ((30, 110), (30, 150)), ((45, 128), (60, 113))],
            (Cell::new(31, 120), Cell::new(44, 140)),
            2,
        );
        let b = || {
            world(
                2,
                &[((11, 128), (40, 128)), ((20, 120), (20, 140))],
                (Cell::new(12, 129), Cell::new(19, 135)),
                1,
            )
        };
        let mut fresh = app(b());
        settle(&mut fresh);
        let want = snapshot(&mut fresh);
        assert!(want.0 > 0 && want.1 > 0 && want.2 > 0, "{want:?}");

        let mut loaded = app(a);
        settle(&mut loaded);
        let before = snapshot(&mut loaded);
        assert_ne!(before, want);
        loaded.world_mut().resource_mut::<SimWorld>().replace(b());
        settle(&mut loaded);
        assert_eq!(snapshot(&mut loaded), want);
    }
}
