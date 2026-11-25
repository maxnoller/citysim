//! Scene readiness. For the first seconds after start (longer on a software
//! renderer) the render pipelines compile and nothing but the clear colour
//! shows. The render world reports how many pipelines are built and how many
//! are still pending; `shared::SceneReady` turns true once everything queued
//! so far is built (ui shows a loading card until then).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use bevy::prelude::*;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache};
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::shared::SceneReady;

/// (pipelines built, pipelines pending), written by the render world.
#[derive(Resource, Clone, Default)]
struct Progress(Arc<(AtomicU32, AtomicU32)>);

/// Give up waiting after this long (seconds of real time).
const MAX_WAIT_S: f32 = 25.0;
/// Pipelines all built for this many frames in a row.
const STABLE_FRAMES: u32 = 4;
/// At least this many pipelines (sky, ground, city, ui ...) before trusting
/// "nothing pending": the first frames have not queued anything yet.
const MIN_PIPELINES: u32 = 6;

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        let progress = Progress::default();
        app.insert_resource(progress.clone()).add_systems(Update, mark_ready);
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .insert_resource(progress)
                .add_systems(Render, count_pipelines.in_set(RenderSystems::Cleanup));
        }
    }
}

fn count_pipelines(cache: Res<PipelineCache>, progress: Res<Progress>) {
    let (mut ok, mut pending) = (0u32, 0u32);
    for p in cache.pipelines() {
        match p.state {
            CachedPipelineState::Ok(_) => ok += 1,
            CachedPipelineState::Queued | CachedPipelineState::Creating(_) => pending += 1,
            CachedPipelineState::Err(_) => {}
        }
    }
    pending += cache.waiting_pipelines().count() as u32;
    progress.0.0.store(ok, Ordering::Relaxed);
    progress.0.1.store(pending, Ordering::Relaxed);
}

fn mark_ready(time: Res<Time<Real>>, progress: Res<Progress>, mut ready: ResMut<SceneReady>, mut stable: Local<u32>) {
    if ready.0 {
        return;
    }
    let ok = progress.0.0.load(Ordering::Relaxed);
    let pending = progress.0.1.load(Ordering::Relaxed);
    if ok >= MIN_PIPELINES && pending == 0 {
        *stable += 1;
    } else {
        *stable = 0;
    }
    if *stable >= STABLE_FRAMES || time.elapsed_secs() > MAX_WAIT_S {
        info!("scene ready after {:.1} s ({ok} pipelines)", time.elapsed_secs());
        ready.0 = true;
    }
}
