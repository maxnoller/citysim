//! Simulation clock.

use serde::{Deserialize, Serialize};

use crate::{TICKS_PER_SECOND, balance};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimTime {
    /// Ticks since the game started.
    pub tick: u64,
    /// Day number, starting at 1.
    pub day: u32,
    /// Ticks since midnight, `0..balance::TICKS_PER_DAY`.
    pub tick_of_day: u32,
}

impl Default for SimTime {
    fn default() -> Self {
        SimTime {
            tick: 0,
            day: 1,
            tick_of_day: balance::START_TICK_OF_DAY,
        }
    }
}

impl SimTime {
    /// Advance one tick. Returns true when a new day started.
    pub fn advance(&mut self) -> bool {
        self.tick += 1;
        self.tick_of_day += 1;
        if self.tick_of_day >= balance::TICKS_PER_DAY {
            self.tick_of_day = 0;
            self.day += 1;
            true
        } else {
            false
        }
    }

    /// Time of day in hours, `0.0..24.0`.
    pub fn hour(&self) -> f32 {
        self.tick_of_day as f32 / balance::TICKS_PER_DAY as f32 * 24.0
    }

    /// Seconds of sim time elapsed (at 1x, equals real seconds).
    pub fn seconds(&self) -> f64 {
        self.tick as f64 / TICKS_PER_SECOND as f64
    }
}
