//! Windowless video capture: `CITYSIM_RECORD=dir`.
//!
//! Like the benchmark the app runs without a window (`main.rs`) and the
//! world camera renders off screen (`CITYSIM_RECORD_SIZE`, default
//! 1920x1080). Time advances by exactly one frame of the video per app
//! update (`CITYSIM_RECORD_FPS`, default 30), so the simulation, the cars
//! and the day run as they would live, however long each frame takes to
//! render and save. The camera flies a Catmull-Rom path through
//! `CITYSIM_RECORD_PATH` keyframes `"t,x,z,yaw,pitch,distance;..."`
//! (seconds, sim metres, radians, metres) and every frame is saved as
//! `dir/frame_00000.png`; the run ends after the last keyframe. Join the
//! frames with e.g. `ffmpeg -framerate 30 -i dir/frame_%05d.png -c:v
//! libx264 -pix_fmt yuv420p out.mp4`.
//!
//! Following (with `CITYSIM_FOLLOW=car|arrive|walker`, which makes the
//! interface pick something and follow it): the first keyframe is the view
//! until the follow starts (then the camera is the player's, zoomed to
//! `CITYSIM_RECORD_FOLLOW_DIST`, 70 m), and the run records
//! `CITYSIM_RECORD_SECONDS` (15) from there. Also:
//! * `CITYSIM_RECORD_SPEED=1|3|6|12`: the simulation speed;
//! * `CITYSIM_RECORD_DT="8,25"`: frame times (ms) to cycle through instead
//!   of `1 / fps` (uneven frames, for measurements);
//! * `CITYSIM_RECORD_PAUSE="a,b"`: paused from `a` to `b` seconds;
//! * `CITYSIM_RECORD_SAVE=0`: no frames written (measurements only);
//! * `CITYSIM_RECORD_UI=1`: the interface is drawn into the frames;
//! * `CITYSIM_RECORD_LOG=file.csv`: per frame, the followed car's drawn
//!   body, the sim's point for it, the ring, the diamond, the off-map
//!   traveller's marker, the camera focus, and where they are on a 1920x1080
//!   screen (`docs` of the columns in the header).

use std::time::Duration;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::time::TimeUpdateStrategy;

use std::io::Write as _;

use bevy::diagnostic::FrameCount;

use super::camera::CameraRig;
use super::vehicles::VehicleEntities;
use crate::shared::{
    CameraFollow, CameraGoTo, CameraOverride, CameraPose, GameSet, SceneReady, SimClock, SimWorld, WorldCamera,
    WorldHighlight, WorldTarget,
};

/// `CITYSIM_RECORD` is set: the app runs windowless (see `main.rs`).
pub fn enabled() -> bool {
    std::env::var_os("CITYSIM_RECORD").is_some_and(|v| !v.is_empty())
}

/// Keyframe: (seconds, focus x, focus z, yaw, pitch, distance).
type Key = [f32; 6];

#[derive(Resource)]
struct Recorder {
    dir: std::path::PathBuf,
    size: (u32, u32),
    fps: f32,
    keys: Vec<Key>,
    image: Option<Handle<Image>>,
    /// Frames rendered since the scene was ready (warm-up first).
    frame: u32,
    saved: u32,
    done_at: Option<u32>,
    follow: Option<Follow>,
    speed: usize,
    dts: Vec<f32>,
    pause: Option<(f32, f32)>,
    save: bool,
    ui: bool,
    log: Option<std::fs::File>,
}

/// Following something (`CITYSIM_FOLLOW`).
struct Follow {
    distance: f32,
    seconds: f32,
    /// Frame (after warm-up) the follow started on.
    started: Option<u32>,
}

const WARMUP: u32 = 90;

pub struct RecordPlugin;

impl Plugin for RecordPlugin {
    fn build(&self, app: &mut App) {
        if !enabled() {
            return;
        }
        let dir = std::path::PathBuf::from(std::env::var("CITYSIM_RECORD").unwrap_or_default());
        let _ = std::fs::create_dir_all(&dir);
        let size = std::env::var("CITYSIM_RECORD_SIZE")
            .ok()
            .and_then(|s| {
                let (a, b) = s.split_once('x')?;
                Some((a.parse().ok()?, b.parse().ok()?))
            })
            .unwrap_or((1920u32, 1080u32));
        let fps: f32 = std::env::var("CITYSIM_RECORD_FPS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30.0);
        let keys = parse_keys(&std::env::var("CITYSIM_RECORD_PATH").unwrap_or_default());
        let num = |k: &str| std::env::var(k).ok().and_then(|v| v.trim().parse::<f32>().ok());
        let list = |k: &str| -> Vec<f32> {
            std::env::var(k)
                .unwrap_or_default()
                .split(',')
                .filter_map(|x| x.trim().parse().ok())
                .collect()
        };
        let follow = std::env::var("CITYSIM_FOLLOW")
            .is_ok_and(|v| !v.is_empty() && v != "0")
            .then(|| Follow {
                distance: num("CITYSIM_RECORD_FOLLOW_DIST").unwrap_or(70.0),
                seconds: num("CITYSIM_RECORD_SECONDS").unwrap_or(15.0),
                started: None,
            });
        if follow.is_none() && keys.len() < 2 {
            error!("CITYSIM_RECORD_PATH needs at least two keyframes \"t,x,z,yaw,pitch,distance;...\"");
        }
        let speed = match num("CITYSIM_RECORD_SPEED").unwrap_or(1.0) as u32 {
            3 => 1,
            6 => 2,
            12 => 3,
            _ => 0,
        };
        let dts: Vec<f32> = list("CITYSIM_RECORD_DT").into_iter().map(|ms| ms / 1000.0).collect();
        let pause = match list("CITYSIM_RECORD_PAUSE")[..] {
            [a, b] => Some((a, b)),
            _ => None,
        };
        let log = std::env::var("CITYSIM_RECORD_LOG")
            .ok()
            .filter(|p| !p.is_empty())
            .and_then(|p| std::fs::File::create(p).ok())
            .map(|mut f| {
                let _ = writeln!(f, "{LOG_HEADER}");
                f
            });
        let first_dt = dts.first().copied().unwrap_or(1.0 / fps);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(first_dt)))
            .insert_resource(Recorder {
                dir,
                size,
                fps,
                keys,
                image: None,
                frame: 0,
                saved: 0,
                done_at: None,
                follow,
                speed,
                dts,
                pause,
                save: std::env::var("CITYSIM_RECORD_SAVE").map_or(true, |v| v != "0"),
                ui: std::env::var("CITYSIM_RECORD_UI").is_ok_and(|v| v == "1"),
                log,
            })
            .add_systems(Update, record.in_set(GameSet::Input))
            .add_systems(Last, (log_frame, next_dt).chain());
    }
}

fn parse_keys(s: &str) -> Vec<Key> {
    s.split(';')
        .filter_map(|k| {
            let v: Vec<f32> = k.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() == 6).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
        })
        .collect()
}

/// Catmull-Rom through the keyframes (distance in log space), with the
/// pace eased in and out at the two ends.
fn pose_at(keys: &[Key], t: f32) -> CameraPose {
    let n = keys.len();
    if n == 0 {
        return CameraPose {
            focus: Vec3::ZERO,
            yaw: 0.0,
            pitch: 1.0,
            distance: 500.0,
        };
    }
    let k = keys
        .iter()
        .rposition(|f| f[0] <= t)
        .unwrap_or(0)
        .min(n.saturating_sub(2));
    let at = |i: isize| keys[i.clamp(0, n as isize - 1) as usize];
    let (p0, p1, p2, p3) = (
        at(k as isize - 1),
        at(k as isize),
        at(k as isize + 1),
        at(k as isize + 2),
    );
    let u = ((t - p1[0]) / (p2[0] - p1[0]).max(1e-3)).clamp(0.0, 1.0);
    let cr = |a: f32, b: f32, c: f32, d: f32| {
        0.5 * (2.0 * b
            + (-a + c) * u
            + (2.0 * a - 5.0 * b + 4.0 * c - d) * u * u
            + (-a + 3.0 * b - 3.0 * c + d) * u * u * u)
    };
    let ch = |i: usize| cr(p0[i], p1[i], p2[i], p3[i]);
    let dist = cr(p0[5].ln(), p1[5].ln(), p2[5].ln(), p3[5].ln()).exp();
    CameraPose {
        focus: Vec3::new(ch(1), 0.0, ch(2)),
        yaw: ch(3),
        pitch: ch(4),
        distance: dist,
    }
}

#[allow(clippy::too_many_arguments)]
fn record(
    mut commands: Commands,
    mut rec: ResMut<Recorder>,
    ready: Res<SceneReady>,
    mut clock: ResMut<SimClock>,
    mut camera: ResMut<CameraOverride>,
    follow: Res<CameraFollow>,
    mut goto: MessageWriter<CameraGoTo>,
    mut images: ResMut<Assets<Image>>,
    cams: Query<(Entity, Option<&RenderTarget>), With<WorldCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    clock.speed = rec.speed;
    for (cam, target) in &cams {
        if !matches!(target, Some(RenderTarget::Image(_))) {
            let (w, h) = rec.size;
            let image = images.add(Image::new_target_texture(w, h, TextureFormat::Bgra8UnormSrgb, None));
            commands.entity(cam).insert(RenderTarget::Image(image.clone().into()));
            if rec.ui {
                commands.entity(cam).insert(bevy::ui::IsDefaultUiCamera);
            }
            rec.image = Some(image);
        }
    }
    let frame = rec.frame;
    let fps = rec.fps;
    let first = rec.keys.first().map(|k| pose_at(&rec.keys, k[0]));
    // Following: the first keyframe until the follow starts, then the
    // player's camera; the clock starts with the follow.
    let (t, end) = match rec.follow.as_mut() {
        Some(f) => {
            if f.started.is_none() && follow.focus.is_some() && frame > WARMUP {
                f.started = Some(frame);
                goto.write(CameraGoTo {
                    focus: follow.focus.unwrap_or_default(),
                    distance: Some(f.distance),
                });
            }
            let t = f.started.map_or(-1.0, |s| (frame - s) as f32 / fps);
            camera.0 = if f.started.is_none() { first } else { None };
            (t, f.seconds)
        }
        None => {
            let t = frame.saturating_sub(WARMUP) as f32 / rec.fps;
            camera.0 = Some(pose_at(&rec.keys, t));
            (t, rec.keys.last().map_or(0.0, |k| k[0]))
        }
    };
    clock.paused = rec.pause.is_some_and(|(a, b)| t >= a && t < b);
    if !ready.0 {
        return;
    }
    rec.frame += 1;
    if let Some(done) = rec.done_at {
        // Let the last screenshots finish saving.
        if rec.frame > done + 20 {
            info!("record: {} frames in {}", rec.saved, rec.dir.display());
            exit.write(AppExit::Success);
        }
        return;
    }
    if rec.frame <= WARMUP || t < 0.0 {
        return;
    }
    if t > end {
        rec.done_at = Some(rec.frame);
        return;
    }
    if !rec.save {
        return;
    }
    if let Some(image) = rec.image.clone() {
        let path = rec.dir.join(format!("frame_{:05}.png", rec.saved));
        rec.saved += 1;
        commands.spawn(Screenshot::image(image)).observe(save_to_disk(path));
        if rec.saved.is_multiple_of(150) {
            info!("record: {} frames ({t:.1} s of {end:.1} s)", rec.saved);
        }
    }
}

/// The next frame's time step (`CITYSIM_RECORD_DT` cycles).
fn next_dt(rec: Res<Recorder>, frame: Res<FrameCount>, mut strategy: ResMut<TimeUpdateStrategy>) {
    if rec.dts.len() > 1 {
        let dt = rec.dts[(frame.0 as usize + 1) % rec.dts.len()];
        *strategy = TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(dt));
    }
}

const LOG_HEADER: &str = "frame,t,dt,paused,speed,vehicle,body_x,body_z,heading,sim_x,sim_z,ring_shown,ring_x,ring_z,\
dia_shown,dia_x,dia_z,trav_shown,trav_x,trav_z,focus_x,focus_z,yaw,pitch,distance,follow_x,follow_z,\
body_sx,body_sy,ring_sx,ring_sy,dia_sx,dia_sy,trav_sx,trav_sy";

/// Where a ground point is on a 1920x1080 screen through `cam`.
fn screen(cam: &GlobalTransform, fov: f32, p: Vec3) -> Vec2 {
    let v = cam.affine().inverse().transform_point3(p);
    let t = (fov * 0.5).tan();
    let aspect = 1920.0 / 1080.0;
    let x = (v.x / -v.z) / (t * aspect);
    let y = (v.y / -v.z) / t;
    Vec2::new((x + 1.0) * 0.5 * 1920.0, (1.0 - y) * 0.5 * 1080.0)
}

/// One line of `CITYSIM_RECORD_LOG` per frame once following.
#[allow(clippy::too_many_arguments)]
fn log_frame(
    mut rec: ResMut<Recorder>,
    frame: Res<FrameCount>,
    time: Res<Time>,
    sim: Res<SimWorld>,
    clock: Res<SimClock>,
    hl: Res<WorldHighlight>,
    follow: Res<CameraFollow>,
    entities: Res<VehicleEntities>,
    cams: Query<(&GlobalTransform, &Projection, &CameraRig), With<WorldCamera>>,
    named: Query<(&Name, &GlobalTransform, &InheritedVisibility)>,
    tfs: Query<&GlobalTransform>,
) {
    let started = rec.follow.as_ref().and_then(|f| f.started);
    let fps = rec.fps;
    let Some(log) = rec.log.as_mut() else { return };
    let Some(start) = started else { return };
    let Ok((cam, proj, rig)) = cams.single() else { return };
    let fov = match proj {
        Projection::Perspective(p) => p.fov,
        _ => 0.7,
    };
    let w = &sim.world;
    let vehicle = match &hl.selected {
        Some(WorldTarget::Vehicle(v)) => Some(*v),
        _ => None,
    };
    let body = vehicle
        .and_then(|v| entities.0.get(&v))
        .and_then(|e| tfs.get(*e).ok())
        .map(|t| (t.translation(), t.rotation().to_euler(EulerRot::YXZ).0));
    let sim_pt = vehicle
        .and_then(|v| w.vehicles.get(v))
        .map(|v| citysim_sim::drive::vehicle_pose(w, v).0);
    let find = |name: &str| {
        named
            .iter()
            .find(|(n, ..)| n.as_str() == name)
            .map(|(_, t, v)| (v.get(), t.translation().with_y(0.0)))
    };
    let ring = find("Vehicle ring");
    let dia = find("Vehicle marker");
    let trav = find("Route traveller rings");
    let f = rig.current.focus;
    let nan = Vec3::splat(f32::NAN);
    let b = body.map_or(nan, |b| b.0.with_y(0.0));
    let shown = |x: Option<(bool, Vec3)>| x.filter(|x| x.0).map_or(nan, |x| x.1);
    let (r, d, tr) = (shown(ring), shown(dia), shown(trav));
    let sc = |p: Vec3| {
        if p.is_nan() {
            Vec2::splat(f32::NAN)
        } else {
            screen(cam, fov, p)
        }
    };
    let (bs, rs, ds, ts) = (sc(b), sc(r), sc(d), sc(tr));
    let fo = follow.focus.unwrap_or([f32::NAN; 2]);
    let sp = sim_pt.unwrap_or([f32::NAN; 2]);
    let _ = writeln!(
        log,
        "{},{:.5},{:.5},{},{},{},{:.4},{:.4},{:.5},{:.4},{:.4},{},{:.4},{:.4},{},{:.4},{:.4},{},{:.4},{:.4},{:.4},{:.4},{:.5},{:.5},{:.4},{:.4},{:.4},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3}",
        frame.0,
        rec_t(frame.0, start, fps),
        time.delta_secs(),
        clock.paused as u8,
        clock.speed_steps(),
        vehicle.map_or(String::from("-"), |v| format!("{v:?}").replace(',', ";")),
        b.x,
        b.z,
        body.map_or(f32::NAN, |b| b.1),
        sp[0],
        sp[1],
        ring.is_some_and(|x| x.0) as u8,
        r.x,
        r.z,
        dia.is_some_and(|x| x.0) as u8,
        d.x,
        d.z,
        trav.is_some_and(|x| x.0) as u8,
        tr.x,
        tr.z,
        f.x,
        f.z,
        rig.current.yaw,
        rig.current.pitch,
        rig.current.distance,
        fo[0],
        fo[1],
        bs.x,
        bs.y,
        rs.x,
        rs.y,
        ds.x,
        ds.y,
        ts.x,
        ts.y,
    );
}

/// Seconds since the follow started (by frames of the video).
fn rec_t(frame: u32, start: u32, fps: f32) -> f32 {
    (frame as f32 - start as f32) / fps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_passes_through_its_keyframes() {
        let keys = parse_keys("0,0,0,0,1,100; 5,100,0,1,0.8,200; 10,100,100,2,0.6,50");
        assert_eq!(keys.len(), 3);
        for k in &keys {
            let p = pose_at(&keys, k[0]);
            assert!((p.focus.x - k[1]).abs() < 1e-3 && (p.focus.z - k[2]).abs() < 1e-3);
            assert!((p.distance - k[5]).abs() < 1e-2);
        }
    }
}
