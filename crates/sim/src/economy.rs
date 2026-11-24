//! Money, taxes and upkeep.

use serde::{Deserialize, Serialize};

use crate::{World, balance};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Economy {
    pub money: i64,
    /// Fraction, `0.0..=balance::MAX_TAX_RATE`.
    pub tax_rate: f32,
    /// Income booked at the end of the previous day.
    pub last_day_income: i64,
    /// Expenses booked at the end of the previous day.
    pub last_day_expenses: i64,
}

impl Default for Economy {
    fn default() -> Self {
        Economy {
            money: balance::STARTING_MONEY,
            tax_rate: balance::DEFAULT_TAX_RATE,
            last_day_income: 0,
            last_day_expenses: 0,
        }
    }
}

/// Called once when a new day starts.
pub(crate) fn end_of_day(_world: &mut World) {
    // PHASE2: income = taxes from residents and jobs at `tax_rate`;
    // expenses = road upkeep (`balance::ROAD_UPKEEP_PER_CELL_PER_DAY`).
}
