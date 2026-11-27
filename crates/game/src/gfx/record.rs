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
//! `dir/frame_00000.png`; the run ends after the last keyframe. A
//! keyframe may carry a seventh value, the sim speed from it on (an index
//! into `SPEEDS`: 0 = 1x, 2 = 6x). `CITYSIM_RECORD_FOLLOW="t0,t1"`: from
//! `t0` to `t1` seconds the camera's focus follows a person walking on the
//! map (picked at `t0`: on the way to work if anyone is, nearest the focus;
//! their drawn figure), the rest of the pose still from the path. Join the
//! frames with e.g. `ffmpeg -framerate 30 -i dir/frame_%05d.png -c:v
//! libx264 -pix_fmt yuv420p out.mp4`.

use std::time::Duration;

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::time::TimeUpdateStrategy;

use crate::shared::{CameraOverride, CameraPose, GameSet, SceneReady, SimClock, WorldCamera};

/// `CITYSIM_RECORD` is set: the app runs windowless (see `main.rs`).
pub fn enabled() -> bool {
    std::env::var_os("CITYSIM_RECORD").is_some_and(|v| !v.is_empty())
}

/// Keyframe: (seconds, focus x, focus z, yaw, pitch, distance, speed
/// index or -1).
type Key = [f32; 7];

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
    /// `CITYSIM_RECORD_FOLLOW` window, and who is followed.
    follow: Option<(f32, f32)>,
    followed: Option<citysim_sim::CitizenId>,
    last_focus: Option<Vec3>,
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
        if keys.len() < 2 {
            error!("CITYSIM_RECORD_PATH needs at least two keyframes \"t,x,z,yaw,pitch,distance;...\"");
        }
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(1.0 / fps)))
            .insert_resource(Recorder {
                dir,
                size,
                fps,
                keys,
                image: None,
                frame: 0,
                saved: 0,
                done_at: None,
                follow: std::env::var("CITYSIM_RECORD_FOLLOW").ok().and_then(|s| {
                    let (a, b) = s.split_once(',')?;
                    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
                }),
                followed: None,
                last_focus: None,
            })
            .add_systems(Update, record.in_set(GameSet::Input));
    }
}

fn parse_keys(s: &str) -> Vec<Key> {
    s.split(';')
        .filter_map(|k| {
            let v: Vec<f32> = k.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() == 6 || v.len() == 7).then(|| [v[0], v[1], v[2], v[3], v[4], v[5], *v.get(6).unwrap_or(&-1.0)])
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
    sim: Res<crate::shared::SimWorld>,
    people: Query<&super::people::PersonVisual>,
    mut images: ResMut<Assets<Image>>,
    cams: Query<(Entity, Option<&RenderTarget>), With<WorldCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    clock.paused = false;
    for (cam, target) in &cams {
        if !matches!(target, Some(RenderTarget::Image(_))) {
            let (w, h) = rec.size;
            let image = images.add(Image::new_target_texture(w, h, TextureFormat::Bgra8UnormSrgb, None));
            commands.entity(cam).insert(RenderTarget::Image(image.clone().into()));
            rec.image = Some(image);
        }
    }
    let end = rec.keys.last().map_or(0.0, |k| k[0]);
    let t = rec.frame.saturating_sub(WARMUP) as f32 / rec.fps;
    let mut pose = pose_at(&rec.keys, t);
    // The speed of the last keyframe passed that sets one.
    if let Some(k) = rec.keys.iter().rev().find(|k| k[0] <= t && k[6] >= 0.0) {
        clock.speed = (k[6] as usize).min(crate::shared::SPEEDS.len() - 1);
    }
    if let Some((t0, t1)) = rec.follow
        && (t0..=t1).contains(&t)
        && rec.frame > WARMUP
    {
        let w = &sim.world;
        if rec
            .followed
            .is_none_or(|c| citysim_sim::walk::walker_of(w, c).is_none())
            && rec.last_focus.is_none()
        {
            let f = pose.focus;
            let to_work = |id: &citysim_sim::CitizenId| {
                w.citizens
                    .get(*id)
                    .and_then(|c| c.location.journey())
                    .is_some_and(|j| j.purpose == citysim_sim::TripPurpose::ToWork)
            };
            rec.followed = w
                .walking
                .walkers
                .iter()
                .filter(|(_, x)| x.left_m() > 60.0 && x.done_m() > 2.0)
                .min_by(|a, b| {
                    let d = |x: &citysim_sim::walk::Walker| Vec2::from(x.pos).distance(Vec2::new(f.x, f.z));
                    (!to_work(a.0), d(a.1)).partial_cmp(&(!to_work(b.0), d(b.1))).unwrap()
                })
                .map(|(id, _)| *id);
            if let Some(c) = rec.followed {
                info!("record: following {c:?}");
            }
        }
        let at = rec.followed.and_then(|c| {
            let (leader, _) = citysim_sim::walk::walker_of(w, c)?;
            people
                .iter()
                .find(|v| v.leader == leader && v.slot == 0)
                .map(|v| v.drawn)
        });
        if let Some(p) = at {
            rec.last_focus = Some(Vec3::new(p.x, 0.0, p.z));
        }
        if let Some(f) = rec.last_focus {
            pose.focus = f;
        }
    }
    camera.0 = Some(pose);
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
    if rec.frame <= WARMUP {
        return;
    }
    if t > end {
        rec.done_at = Some(rec.frame);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_passes_through_its_keyframes() {
        let keys = parse_keys("0,0,0,0,1,100; 5,100,0,1,0.8,200,2; 10,100,100,2,0.6,50");
        assert_eq!(keys.len(), 3);
        for k in &keys {
            let p = pose_at(&keys, k[0]);
            assert!((p.focus.x - k[1]).abs() < 1e-3 && (p.focus.z - k[2]).abs() < 1e-3);
            assert!((p.distance - k[5]).abs() < 1e-2);
        }
    }
}
