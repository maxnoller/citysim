//! GFX: everything drawn in 3D, and the camera. Talks to the rest of the
//! game only through `crate::shared` (never import from `crate::ui`).

mod bench;
mod bench_script;
mod building_gen;
mod buildings;
mod camera;
mod city_material;
mod cursor;
mod daynight;
mod greens;
mod guides;
mod land;
mod loading;
mod lot_props;
mod lots;
mod mesh_util;
mod noise;
mod overlay;
mod palette;
mod perf;
mod props;
mod record;
mod road_geom;
mod road_tool;
mod roads;
mod route;
mod scene;
mod signals;
mod smoke;
mod stats;
mod trees;
mod vehicle_gen;
mod vehicles;
mod yards;
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
        ));
        app.add_plugins((
            signals::SignalsPlugin,
            lots::LotsPlugin,
            guides::GuidesPlugin,
            smoke::SmokePlugin,
            loading::LoadingPlugin,
            land::LandPlugin,
            road_tool::RoadToolPlugin,
            bench::BenchPlugin,
            perf::PerfPlugin,
            record::RecordPlugin,
            route::RoutePlugin,
            yards::YardsPlugin,
            greens::GreensPlugin,
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
                kind: citysim_sim::RoadKind::Street,
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
                super::land::LandPlugin,
                super::roads::RoadsPlugin,
                super::zones::ZonesPlugin,
                super::buildings::BuildingsPlugin,
                super::vehicles::VehiclesPlugin,
                super::trees::TreesPlugin,
                super::greens::GreensPlugin,
                super::yards::YardsPlugin,
            ));
        let lit = app
            .world_mut()
            .resource_mut::<Assets<super::city_material::CityMaterial>>()
            .add(super::city_material::city_material(
                StandardMaterial::default(),
                Handle::default(),
            ));
        app.insert_resource(WorldMaterials {
            lit: lit.clone(),
            ground: lit,
            sky: Handle::default(),
            day: Handle::default(),
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

/// Following a car: the systems as the game runs them, on a scripted clock
/// (uneven frame times, 1x and 12x, a pause): the ring and the diamond sit
/// on the drawn body every frame, and the followed body holds still in the
/// camera's view once the camera has glided onto it.
#[cfg(test)]
mod follow_tests {
    use std::time::Duration;

    use bevy::prelude::*;
    use bevy::time::TimeUpdateStrategy;
    use citysim_sim::{VehicleId, World, Zone};

    use super::scene::WorldMaterials;
    use crate::shared::{
        CameraFollow, DrawnPoses, FollowOn, GraphicsQuality, SharedPlugin, SimClock, SimWorld, WorldCamera,
        WorldHighlight, WorldTarget,
    };

    fn town() -> World {
        let mut w = World::new(3);
        let mut player =
            citysim_sim::scenarios::GridPlayer::new(&[Zone::Residential, Zone::Commercial, Zone::Industrial]);
        while w.stats.population < 1_000 && w.time.day < 30 {
            player.act(&mut w);
            w.step();
        }
        // The morning rush.
        let rush = citysim_sim::balance::TICKS_PER_DAY / 3;
        while w.time.tick_of_day != rush {
            w.step();
        }
        w
    }

    fn app(w: World) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::input::InputPlugin,
            bevy::transform::TransformPlugin,
        ))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<super::city_material::CityMaterial>()
        .add_plugins(SharedPlugin)
        .insert_resource(SimWorld {
            world: w,
            generation: 0,
        })
        .insert_resource(GraphicsQuality::Low)
        .add_plugins((
            super::land::LandPlugin,
            super::buildings::BuildingsPlugin,
            super::vehicles::VehiclesPlugin,
            super::camera::CameraPlugin,
            super::guides::GuidesPlugin,
        ));
        let lit = app
            .world_mut()
            .resource_mut::<Assets<super::city_material::CityMaterial>>()
            .add(super::city_material::city_material(
                StandardMaterial::default(),
                Handle::default(),
            ));
        app.insert_resource(WorldMaterials {
            lit: lit.clone(),
            ground: lit,
            sky: Handle::default(),
            day: Handle::default(),
        });
        app
    }

    fn named(app: &mut App, name: &str) -> (Vec3, bool) {
        let w = app.world_mut();
        let mut q = w.query::<(&Name, &GlobalTransform, &Visibility)>();
        q.iter(w)
            .find(|(n, ..)| n.as_str() == name)
            .map(|(_, t, v)| (t.translation(), *v != Visibility::Hidden))
            .expect(name)
    }

    /// The camera's view of a ground point (camera space).
    fn in_view(app: &mut App, p: Vec3) -> Vec3 {
        let w = app.world_mut();
        let mut q = w.query_filtered::<&GlobalTransform, With<WorldCamera>>();
        let cam = q.single(w).expect("camera");
        cam.affine().inverse().transform_point3(p)
    }

    /// Follow a moving car for `frames` frames of alternating 8 and 25 ms
    /// at sim speed index `speed` (paused over `pause`): (frames checked,
    /// largest marker offset, largest frame-to-frame movement in view).
    fn follow(speed: usize, frames: usize, pause: std::ops::Range<usize>) -> (usize, f32, f32) {
        let mut app = app(town());
        app.world_mut().resource_mut::<SimClock>().speed = speed;
        let step = |app: &mut App, i: usize| {
            let dt = if i.is_multiple_of(2) { 0.008 } else { 0.025 };
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(dt)));
            app.update();
        };
        for i in 0..30 {
            step(&mut app, i);
        }
        // A car that is moving and will keep going for a while.
        let w = &app.world().resource::<SimWorld>().world;
        let id: VehicleId = w
            .vehicles
            .iter()
            .filter(|(_, v)| v.speed > 3.0)
            .max_by_key(|(_, v)| v.route.len())
            .map(|(id, _)| id)
            .unwrap_or_else(|| {
                panic!(
                    "no moving car: pop {}, day {}, {} vehicles, hour tick {}",
                    w.stats.population,
                    w.time.day,
                    w.vehicles.len(),
                    w.time.tick_of_day
                )
            });
        let (mut checked, mut worst_mark, mut worst_move) = (0, 0.0f32, 0.0f32);
        let mut last_view: Option<Vec3> = None;
        for i in 0..frames {
            // As ui does: pin it, follow it (the sim's point as the fallback).
            let world = app.world_mut();
            let Some(focus) = world
                .resource::<SimWorld>()
                .world
                .vehicles
                .get(id)
                .map(|v| citysim_sim::drive::vehicle_pose(&world.resource::<SimWorld>().world, v).0)
            else {
                break;
            };
            world.resource_mut::<WorldHighlight>().selected = Some(WorldTarget::Vehicle(id));
            *world.resource_mut::<CameraFollow>() = CameraFollow {
                focus: Some(focus),
                on: FollowOn::Vehicle(id),
            };
            world.resource_mut::<SimClock>().paused = pause.contains(&i);
            step(&mut app, i);
            let Some(d) = app.world().resource::<DrawnPoses>().vehicle(id) else {
                last_view = None;
                continue;
            };
            let body = Vec3::new(d.pos[0], 0.0, d.pos[1]);
            let (ring, ring_shown) = named(&mut app, "Vehicle ring");
            let (dia, dia_shown) = named(&mut app, "Vehicle marker");
            assert!(ring_shown && dia_shown, "frame {i}: marks hidden");
            let mark = body
                .with_y(0.0)
                .distance(ring.with_y(0.0))
                .max(body.distance(dia.with_y(0.0)));
            worst_mark = worst_mark.max(mark);
            // After the glide in (2 s of frames), the body holds still in
            // the camera's view.
            let view = in_view(&mut app, body);
            if i > 120
                && let Some(last) = last_view
            {
                worst_move = worst_move.max(view.distance(last));
                checked += 1;
            }
            last_view = Some(view);
        }
        (checked, worst_mark, worst_move)
    }

    #[test]
    fn a_followed_car_holds_still_with_its_marks_on_it() {
        for (speed, pause) in [(0, 200..260), (3, 0..0)] {
            let (checked, mark, moved) = follow(speed, 400, pause);
            println!("speed {speed}: {checked} frames, marks {mark:.5} m off, body moved {moved:.5} m in view");
            assert!(checked > 100, "speed {speed}: only {checked} frames followed");
            assert!(mark < 1e-3, "speed {speed}: a mark is {mark} m off the body");
            assert!(moved < 2e-3, "speed {speed}: the body moved {moved} m in the view");
        }
    }
}
