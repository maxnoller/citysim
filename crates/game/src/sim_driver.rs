//! Owns the sim World, applies commands and steps it on the fixed timestep.

use bevy::prelude::*;
use citysim_sim::{Command, CommandError, Outcome, TICKS_PER_SECOND, World};

use crate::GameSet;

/// The simulation. Read it anywhere; mutate it only through `SimCommand`s
/// (or by replacing the whole world, e.g. on load — then bump `generation`).
#[derive(Resource)]
pub struct SimWorld {
    pub world: World,
    /// Bumped whenever `world` is replaced wholesale (load / new game), so
    /// renderers rebuild even if the version counters happen to match.
    pub generation: u64,
}

impl SimWorld {
    /// Render-cache key: changes whenever `version` or the world changes.
    pub fn key(&self, version: u64) -> (u64, u64) {
        (self.generation, version)
    }
}

/// Pause state and speed (sim steps per fixed update).
#[derive(Resource, Debug)]
pub struct SimClock {
    pub paused: bool,
    /// Index into [`SPEEDS`].
    pub speed: usize,
}

/// Steps per fixed update for speed levels 1, 2, 3.
pub const SPEEDS: [u32; 3] = [1, 3, 6];

impl SimClock {
    pub fn steps_per_tick(&self) -> u32 {
        if self.paused { 0 } else { SPEEDS[self.speed] }
    }
}

/// A command for the sim, applied in `GameSet::Sim`.
#[derive(Message, Debug, Clone, Copy)]
pub struct SimCommand(pub Command);

/// Result of the most recent command (for HUD feedback).
#[derive(Resource, Default)]
pub struct LastCommand {
    pub result: Option<(Command, Result<Outcome, CommandError>)>,
    /// Real time (seconds since start) when it was applied.
    pub at: f32,
}

/// Quick save slot path.
const SAVE_PATH: &str = "citysim.citysave";

pub struct SimDriverPlugin;

impl Plugin for SimDriverPlugin {
    fn build(&self, app: &mut App) {
        let seed = std::env::var("CITYSIM_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        app.insert_resource(SimWorld {
            world: World::new(seed),
            generation: 0,
        })
        .insert_resource(SimClock {
            paused: false,
            speed: 0,
        })
        .insert_resource(Time::<Fixed>::from_hz(TICKS_PER_SECOND as f64))
        .init_resource::<LastCommand>()
        .add_message::<SimCommand>()
        .add_systems(FixedUpdate, step_sim)
        .add_systems(Update, (clock_keys, save_load_keys).in_set(GameSet::Input))
        .add_systems(Update, apply_commands.in_set(GameSet::Sim));
    }
}

fn step_sim(mut sim: ResMut<SimWorld>, clock: Res<SimClock>) {
    for _ in 0..clock.steps_per_tick() {
        sim.world.step();
    }
}

fn apply_commands(
    mut sim: ResMut<SimWorld>,
    mut cmds: MessageReader<SimCommand>,
    mut last: ResMut<LastCommand>,
    time: Res<Time<Real>>,
) {
    for SimCommand(cmd) in cmds.read() {
        let res = sim.world.apply(*cmd);
        if let Err(e) = &res {
            debug!("command {cmd:?} rejected: {e}");
        }
        last.result = Some((*cmd, res));
        last.at = time.elapsed_secs();
    }
}

fn clock_keys(keys: Res<ButtonInput<KeyCode>>, mut clock: ResMut<SimClock>) {
    if keys.just_pressed(KeyCode::Space) {
        clock.paused = !clock.paused;
    }
    for (i, key) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3]
        .into_iter()
        .enumerate()
    {
        if keys.just_pressed(key) {
            clock.speed = i;
            clock.paused = false;
        }
    }
}

/// F5 quick-save, F9 quick-load (one slot in the working directory).
fn save_load_keys(keys: Res<ButtonInput<KeyCode>>, mut sim: ResMut<SimWorld>) {
    if keys.just_pressed(KeyCode::F5) {
        match std::fs::write(SAVE_PATH, sim.world.save()) {
            Ok(()) => info!("saved to {SAVE_PATH}"),
            Err(e) => error!("save failed: {e}"),
        }
    }
    if keys.just_pressed(KeyCode::F9) {
        match std::fs::read(SAVE_PATH)
            .map_err(|e| e.to_string())
            .and_then(|b| World::load(&b).map_err(|e| e.to_string()))
        {
            Ok(world) => {
                sim.world = world;
                sim.generation += 1;
                info!("loaded {SAVE_PATH}");
            }
            Err(e) => error!("load failed: {e}"),
        }
    }
}
