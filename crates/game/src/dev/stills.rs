//! `CITYSIM_STILLS=dir`: windowless screenshots of the 3D scene (and the
//! HUD on top of it), several from one start, then exit.
//!
//! The app runs without a window (`main.rs`, as for `CITYSIM_BENCH` and
//! `CITYSIM_RECORD`): no display, no Xvfb, no slow presentation. The world
//! camera renders to an image of `CITYSIM_SHOT_SIZE` (default 1920x1080),
//! which is also the interface's default camera, so the HUD is drawn as in
//! a 1920x1080 window (it sees no cursor: nothing is hovered).
//!
//! `CITYSIM_STILLS_LIST="name=view;name=view+overlay;..."` (default
//! `wide=wide`) names the shots; each is saved as `dir/name.png`. A view is
//! `wide`, `street`, `close` (the autoshot presets for the demo) or an
//! exact pose `x,z,yaw,pitch,distance` (sim metres, radians, metres, as
//! `CITYSIM_GFX_POSE`); `+zones` / `+traffic` shows that overlay. The time
//! of day is the demo's (`CITYSIM_DEMO_HOUR`) or pinned with
//! `CITYSIM_GFX_HOUR`. `CITYSIM_STILLS_SETTLE=s` seconds between moving the
//! camera and the capture (default 0.75, and at least 20 frames: overlays
//! refresh four times a second), `CITYSIM_SHOT_DELAY` seconds of running
//! after the scene is ready before the first shot (default 1). The process
//! exits with 0 once every file is written.

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::ui::IsDefaultUiCamera;

use super::{DemoKind, ShotView};
use crate::shared::{CameraOverride, CameraPose, GameSet, OverlayMode, SceneReady, SimCommand, WorldCamera};

/// `CITYSIM_STILLS` is set: the app runs windowless (see `main.rs`).
pub fn enabled() -> bool {
    std::env::var_os("CITYSIM_STILLS").is_some_and(|v| !v.is_empty())
}

#[derive(Debug, Clone, PartialEq)]
enum View {
    Preset(ShotView),
    Pose([f32; 5]),
}

#[derive(Debug, Clone, PartialEq)]
struct Still {
    name: String,
    view: View,
    overlay: OverlayMode,
}

fn parse_list(s: &str) -> Vec<Still> {
    s.split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .filter_map(|entry| {
            let (name, spec) = entry.split_once('=').unwrap_or((entry, entry));
            let (view, overlay) = match spec.split_once('+') {
                Some((v, "zones")) => (v, OverlayMode::Zones),
                Some((v, "traffic")) => (v, OverlayMode::Traffic),
                Some((v, o)) => {
                    error!("stills: unknown overlay {o:?} in {entry:?} (zones or traffic)");
                    (v, OverlayMode::None)
                }
                None => (spec, OverlayMode::None),
            };
            let view = match view.trim() {
                "wide" => View::Preset(ShotView::Wide),
                "street" => View::Preset(ShotView::Street),
                "close" => View::Preset(ShotView::Close),
                pose => {
                    let v: Vec<f32> = pose.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if v.len() != 5 {
                        error!("stills: {entry:?}: a view is wide, street, close or x,z,yaw,pitch,distance");
                        return None;
                    }
                    View::Pose([v[0], v[1], v[2], v[3], v[4]])
                }
            };
            Some(Still {
                name: name.trim().to_string(),
                view,
                overlay,
            })
        })
        .collect()
}

#[derive(Resource)]
struct Stills {
    dir: std::path::PathBuf,
    size: (u32, u32),
    list: Vec<Still>,
    demo: Option<DemoKind>,
    /// Seconds between moving the camera and the capture.
    settle: f32,
    delay: f32,
    image: Option<Handle<Image>>,
    /// Seconds since the scene was ready.
    t: f32,
    /// Index of the shot being prepared; frames and seconds since its
    /// camera was set.
    index: usize,
    frames: u32,
    since: f32,
    started: bool,
    /// Shots handed to the screenshot system and not yet written.
    pending: u32,
    sent_sample: bool,
}

pub struct StillsPlugin {
    pub demo: Option<DemoKind>,
}

impl Plugin for StillsPlugin {
    fn build(&self, app: &mut App) {
        let dir = std::path::PathBuf::from(std::env::var("CITYSIM_STILLS").unwrap_or_default());
        let _ = std::fs::create_dir_all(&dir);
        let list = parse_list(&std::env::var("CITYSIM_STILLS_LIST").unwrap_or_else(|_| "wide=wide".into()));
        let size = std::env::var("CITYSIM_SHOT_SIZE")
            .ok()
            .and_then(|s| {
                let (a, b) = s.split_once('x')?;
                Some((a.parse().ok()?, b.parse().ok()?))
            })
            .unwrap_or((1920u32, 1080u32));
        let env_num = |name: &str| std::env::var(name).ok().and_then(|s| s.parse::<f32>().ok());
        app.insert_resource(Stills {
            dir,
            size,
            list,
            demo: self.demo,
            settle: env_num("CITYSIM_STILLS_SETTLE").unwrap_or(0.75),
            delay: env_num("CITYSIM_SHOT_DELAY").unwrap_or(1.0),
            image: None,
            t: 0.0,
            index: 0,
            frames: 0,
            since: 0.0,
            started: false,
            pending: 0,
            sent_sample: false,
        })
        .add_systems(Update, run_stills.in_set(GameSet::Input));
    }
}

#[derive(Component)]
struct StillShot;

#[allow(clippy::too_many_arguments)]
fn run_stills(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut st: ResMut<Stills>,
    ready: Res<SceneReady>,
    mut camera: ResMut<CameraOverride>,
    mut overlay: ResMut<OverlayMode>,
    mut images: ResMut<Assets<Image>>,
    cams: Query<(Entity, Option<&RenderTarget>), With<WorldCamera>>,
    in_flight: Query<(), With<StillShot>>,
    mut out: MessageWriter<SimCommand>,
    mut exit: MessageWriter<AppExit>,
) {
    if !st.sent_sample && st.demo.is_none() {
        st.sent_sample = true;
        for cmd in super::autoshot::sample_city() {
            out.write(SimCommand(cmd));
        }
    }
    for (cam, target) in &cams {
        if !matches!(target, Some(RenderTarget::Image(_))) {
            let (w, h) = st.size;
            let image = images.add(Image::new_target_texture(w, h, TextureFormat::Bgra8UnormSrgb, None));
            commands
                .entity(cam)
                .insert((RenderTarget::Image(image.clone().into()), IsDefaultUiCamera));
            st.image = Some(image);
        }
    }
    let Some(still) = st.list.get(st.index).cloned() else {
        // All shots handed over: exit once the last one is on disk (without
        // the renderer's teardown, which once crashed after the files were
        // written: scripts rely on the exit code).
        if in_flight.is_empty() {
            info!("stills: {} shots in {}", st.list.len(), st.dir.display());
            std::process::exit(0);
        }
        return;
    };
    let pose = match still.view {
        View::Preset(v) => super::autoshot::view_pose(v, st.demo),
        View::Pose([x, z, yaw, pitch, distance]) => CameraPose {
            focus: Vec3::new(x, 0.0, z),
            yaw,
            pitch,
            distance,
        },
    };
    camera.0 = Some(pose);
    overlay.set_if_neq(still.overlay);
    if !ready.0 {
        return;
    }
    let dt = time.delta_secs().min(0.25);
    st.t += dt;
    if st.t > st.delay + 120.0 {
        error!("stills: timed out");
        exit.write(AppExit::error());
        return;
    }
    if !st.started {
        if st.t < st.delay {
            return;
        }
        st.started = true;
        st.frames = 0;
        st.since = 0.0;
    }
    st.frames += 1;
    st.since += dt;
    if st.frames < 20 || st.since < st.settle {
        return;
    }
    let Some(image) = st.image.clone() else { return };
    let path = st.dir.join(format!("{}.png", still.name));
    info!("stills: saving {}", path.display());
    st.pending += 1;
    commands
        .spawn((Screenshot::image(image), StillShot))
        .observe(save_to_disk(path))
        .observe(|ev: On<ScreenshotCaptured>, mut commands: Commands| {
            commands.entity(ev.entity).despawn();
        });
    st.index += 1;
    st.frames = 0;
    st.since = 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_parses_presets_poses_and_overlays() {
        let l = parse_list("a=wide; b=street+traffic;c=100,200,0.5,0.9,300+zones;;bad=1,2");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].view, View::Preset(ShotView::Wide));
        assert_eq!(l[1].overlay, OverlayMode::Traffic);
        assert_eq!(l[2].view, View::Pose([100.0, 200.0, 0.5, 0.9, 300.0]));
        assert_eq!(l[2].overlay, OverlayMode::Zones);
        assert_eq!(parse_list("close")[0].name, "close");
    }
}
