//! The city's fiscal health in the player's words, from the sim's
//! `economy.fiscal` (updated at midnight): what is wrong, what it costs the
//! city, and what would fix it. Used by the top bar and the budget panel;
//! state changes become toasts.

use bevy::prelude::*;
use citysim_sim::{FiscalReason, FiscalState, World};

use super::format;
use super::theme as t;
use super::toasts::{Toast, ToastKind};
use crate::shared::{GameSet, SimWorld};

pub struct FiscalPlugin;

impl Plugin for FiscalPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, fiscal_toasts.in_set(GameSet::Render));
    }
}

/// What the HUD says about the money.
#[derive(Debug, Clone, PartialEq)]
pub struct FiscalNote {
    /// Short label for the top bar ("At a loss", "In debt").
    pub short: &'static str,
    /// Headline for the budget panel.
    pub title: &'static str,
    /// What is degrading and why.
    pub body: String,
    /// What would fix it.
    pub fix: &'static str,
    pub color: Color,
}

/// `None` while the city is healthy.
pub fn fiscal_note(world: &World) -> Option<FiscalNote> {
    let f = &world.economy.fiscal;
    let services = format!("City services at {}", format::percent(f.service_level));
    let (short, title, body, fix, color) = match (f.state, f.reason) {
        (FiscalState::Healthy, FiscalReason::Recovering) => (
            "Recovering",
            "Recovering",
            format!("{services}; back to full over the next days"),
            "Keep the budget in the black",
            t::ACCENT_TEXT,
        ),
        (FiscalState::Healthy, _) => return None,
        (FiscalState::Deficit, FiscalReason::TaxTooLow) => (
            "Taxes too low",
            "Taxes don't cover the running costs",
            format!("Losing money every day ({} days so far)", f.days_in_deficit.max(1)),
            "Raise the tax rate a little",
            t::WARNING,
        ),
        (FiscalState::Deficit, _) => (
            "At a loss",
            "Running at a loss",
            format!(
                "Road upkeep and services cost more than taxes bring in ({} days)",
                f.days_in_deficit.max(1)
            ),
            "Raise taxes, zone more, or remove roads nobody uses",
            t::WARNING,
        ),
        (FiscalState::InDebt, _) => (
            "In debt",
            "In debt: services cut",
            format!("{services}, so all demand is lower"),
            "Raise taxes or remove unused roads until money is above zero",
            t::NEGATIVE,
        ),
        (FiscalState::Crisis, _) => (
            "Residents leaving",
            "Debt crisis",
            if f.left_last_night > 0 {
                format!(
                    "{services}; {} households moved away last night",
                    format::group(f.left_last_night as u64)
                )
            } else {
                format!("{services}; households move away every night")
            },
            "Raise taxes now and bulldoze roads you don't need",
            t::NEGATIVE,
        ),
    };
    Some(FiscalNote {
        short,
        title,
        body,
        fix,
        color,
    })
}

/// Toast when the fiscal state changes (checked at most once a frame; the
/// sim changes it at midnight).
fn fiscal_toasts(
    sim: Res<SimWorld>,
    mut last: Local<Option<(u64, FiscalState, FiscalReason)>>,
    mut out: MessageWriter<Toast>,
) {
    let f = &sim.world.economy.fiscal;
    let now = (sim.generation, f.state, f.reason);
    let Some(prev) = last.replace(now) else {
        return;
    };
    if prev.0 != now.0 || (prev.1, prev.2) == (now.1, now.2) {
        return;
    }
    let toast = match (prev.1, now.1) {
        (_, FiscalState::Crisis) => Toast::new(
            ToastKind::Error,
            "Debt crisis: residents are moving away. Raise taxes and cut road upkeep",
        ),
        (FiscalState::Healthy | FiscalState::Deficit, FiscalState::InDebt) => Toast::new(
            ToastKind::Warning,
            "The city is in debt: services are cut until money is above zero",
        ),
        (FiscalState::Healthy, FiscalState::Deficit) => Toast::new(
            ToastKind::Warning,
            match now.2 {
                FiscalReason::TaxTooLow => "Taxes no longer cover the running costs",
                _ => "The city ran at a loss yesterday",
            },
        ),
        (FiscalState::InDebt | FiscalState::Crisis, FiscalState::Healthy | FiscalState::Deficit) => {
            Toast::new(ToastKind::Success, "Out of debt: services are recovering")
        }
        (FiscalState::Deficit, FiscalState::Healthy) => {
            Toast::new(ToastKind::Success, "The budget is back in the black")
        }
        _ => return,
    };
    out.write(toast);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_city_has_no_note_and_debt_names_the_fix() {
        let mut w = World::new(1);
        assert!(fiscal_note(&w).is_none());
        w.economy.fiscal.state = FiscalState::InDebt;
        w.economy.fiscal.reason = FiscalReason::ServicesCut;
        w.economy.fiscal.service_level = 0.7;
        let n = fiscal_note(&w).unwrap();
        assert_eq!(n.short, "In debt");
        assert!(n.body.contains("70%"), "{}", n.body);
        assert!(n.fix.contains("Raise taxes"));
    }
}
