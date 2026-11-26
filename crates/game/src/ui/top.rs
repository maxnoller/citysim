//! The top edge: the city plate (name, size, a line towards the next
//! population milestone) hanging from the top-left corner, the alert
//! ribbon from the top centre (refused commands, debt, the rush hour,
//! pause), and the system buttons in the top-right corner (statistics,
//! maps, pause menu).

use bevy::prelude::*;

use super::UiState;
use super::context::error_text;
use super::format;
use super::plate::{PlateAnim, PlateCache, PlateMaterial, PlateStyle};
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, caps, info_tip, layout, plate_node, plate_shadow,
    spawn_button,
};
use crate::shared::{GameSet, LastCommand, SimClock, SimWorld};

pub struct TopPlugin;

impl Plugin for TopPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Alert>()
            .init_resource::<Alerts>()
            .init_resource::<PinnedAlert>()
            .add_systems(Startup, spawn_top)
            .add_systems(
                Update,
                (command_alerts, update_city, update_ribbon)
                    .chain()
                    .in_set(GameSet::Render),
            );
    }
}

/// How an alert reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertKind {
    /// Neutral (paused).
    Note,
    /// Amber (rush hour, deficit).
    Warning,
    /// Red (refused, debt).
    Danger,
}

/// Show a short-lived alert ribbon (refusals that are not at the cursor).
#[derive(Message, Debug, Clone)]
pub struct Alert {
    pub kind: AlertKind,
    pub text: String,
}

/// An alert that stays up regardless of the city (screenshots of a
/// moment; dev tooling only).
#[derive(Resource, Default)]
pub struct PinnedAlert(pub Option<Alert>);

#[derive(Resource, Default)]
struct Alerts {
    /// The latest transient alert and its age (seconds).
    transient: Option<(Alert, f32)>,
}

const TRANSIENT_S: f32 = 5.0;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum TopText {
    City,
    Milestone,
    Ribbon,
}

#[derive(Component)]
struct MilestoneFill;

#[derive(Component)]
struct RibbonRoot {
    /// 0 hidden .. 1 fully down.
    t: f32,
    shown: Option<(AlertKind, String)>,
}

#[derive(Component)]
struct RibbonPlate;

#[derive(Component)]
struct RibbonIcon;

/// City sizes and the population each starts at.
pub const TIERS: [(&str, u32); 7] = [
    ("Hamlet", 0),
    ("Village", 250),
    ("Town", 1_500),
    ("City", 7_500),
    ("Large city", 20_000),
    ("Metropolis", 50_000),
    ("Megacity", 120_000),
];

/// The city's size, the next one and how far along the way there it is.
pub fn milestone(pop: u32) -> (&'static str, Option<(&'static str, u32)>, f32) {
    let i = TIERS.iter().rposition(|(_, at)| pop >= *at).unwrap_or(0);
    let (name, at) = TIERS[i];
    match TIERS.get(i + 1) {
        Some(&(next, next_at)) => {
            let f = (pop - at) as f32 / (next_at - at) as f32;
            (name, Some((next, next_at)), f.clamp(0.0, 1.0))
        }
        None => (name, None, 1.0),
    }
}

/// A name for the city from its world seed.
pub fn city_name(seed: u64) -> String {
    const HEADS: [&str; 16] = [
        "Ash", "Elm", "Brook", "Wren", "Stone", "Mill", "Oak", "Hollow", "Lark", "Fern", "Clay", "Marsh", "Holt",
        "Pine", "Kings", "Alder",
    ];
    const TAILS: [&str; 12] = [
        "ford", "field", "haven", "bury", "wick", "dale", "port", "ton", "mere", "stead", "worth", "gate",
    ];
    let h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17);
    let head = HEADS[(h % HEADS.len() as u64) as usize];
    let tail = TAILS[((h >> 20) % TAILS.len() as u64) as usize];
    format!("{head}{tail}")
}

fn spawn_top(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
) {
    let fonts = fonts.into_inner();
    // City plate: right side slanted.
    let city_mat = cache.get(&mut mats, PlateStyle::PANEL.slanted([0.0, 0.0, 0.0, 0.4]));
    commands
        .spawn((
            Name::new("City plate"),
            plate_node(
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: px(0),
                    height: px(60),
                    padding: UiRect::new(px(26), px(52), px(0), px(0)),
                    align_items: AlignItems::Center,
                    column_gap: px(20),
                    ..default()
                },
                UiRect::new(px(0), px(t::LW), px(0), px(t::LW)),
                [0.0; 4],
            ),
            city_mat,
            plate_shadow(),
            GlobalZIndex(25),
            info_tip(
                "Your city",
                "Its size grows with the population; the line fills towards the next size.",
            ),
        ))
        .with_children(|c| {
            c.spawn((
                Text::new(""),
                fonts.figures(25.0, t::W_BOLD),
                t::caps_spacing(25.0, 0.16),
                TextColor(t::TEXT),
                TextLayout::no_wrap(),
                Pickable::IGNORE,
                TopText::City,
            ));
            c.spawn(layout(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            }))
            .with_children(|m| {
                m.spawn((caps(&fonts, "", t::FONT_CAP, t::TEXT_MUTED), TopText::Milestone));
                m.spawn((
                    layout(Node {
                        width: px(150),
                        height: px(5),
                        ..default()
                    }),
                    BackgroundColor(t::LINE),
                ))
                .with_child((
                    MilestoneFill,
                    layout(Node {
                        width: percent(0),
                        height: percent(100),
                        ..default()
                    }),
                    BackgroundColor(t::ACCENT),
                ));
            });
        });

    // Alert ribbon, hanging from the top centre.
    commands
        .spawn((
            Name::new("Alert ribbon"),
            RibbonRoot { t: 0.0, shown: None },
            layout(Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                top: px(0),
                justify_content: JustifyContent::Center,
                ..default()
            }),
            UiTransform::default(),
            GlobalZIndex(24),
            Visibility::Hidden,
        ))
        .with_children(|r| {
            r.spawn((
                RibbonPlate,
                plate_node(
                    Node {
                        height: px(40),
                        padding: UiRect::horizontal(px(44)),
                        align_items: AlignItems::Center,
                        column_gap: px(10),
                        ..default()
                    },
                    UiRect::DEFAULT,
                    [0.0; 4],
                ),
                PlateAnim::new(ribbon_style(AlertKind::Warning)),
                plate_shadow(),
            ))
            .with_children(|p| {
                p.spawn((
                    super::widgets::icon(&fonts, icon::ALERT, 18.0, t::WARNING_INK),
                    RibbonIcon,
                ));
                p.spawn((
                    Text::new(""),
                    fonts.text(14.0, t::W_BOLD),
                    t::caps_spacing(14.0, t::CAPS_EM),
                    TextColor(t::WARNING_INK),
                    TextLayout::no_wrap(),
                    Pickable::IGNORE,
                    TopText::Ribbon,
                ));
            });
        });

    // System buttons: statistics, maps, menu.
    let sys_mat = cache.get(&mut mats, PlateStyle::PANEL.slanted([0.0, 0.5, 0.0, 0.0]));
    commands
        .spawn((
            Name::new("System buttons"),
            plate_node(
                Node {
                    position_type: PositionType::Absolute,
                    right: px(0),
                    top: px(0),
                    height: px(48),
                    padding: UiRect::new(px(34), px(10), px(0), px(0)),
                    align_items: AlignItems::Center,
                    column_gap: px(2),
                    ..default()
                },
                UiRect::new(px(t::LW), px(0), px(0), px(t::LW)),
                [0.0; 4],
            ),
            sys_mat,
            plate_shadow(),
            GlobalZIndex(25),
        ))
        .with_children(|s| {
            for (action, glyph, title, desc, key) in [
                (
                    UiAction::Stats,
                    icon::CHART,
                    "Statistics",
                    "Population, work, commutes and traffic at a glance.",
                    None,
                ),
                (
                    UiAction::Maps,
                    icon::EYE,
                    "Maps",
                    "Traffic and zone maps over the city.",
                    Some("O"),
                ),
                (
                    UiAction::OpenMenu,
                    icon::SETTINGS,
                    "Menu",
                    "Pause, save and load, settings, keys.",
                    Some("Esc"),
                ),
            ] {
                let mut tip = Tooltip::new(title, desc);
                tip.key = key;
                spawn_button(
                    s,
                    UiButton::new(action, ButtonKind::Sys),
                    Node {
                        width: px(42),
                        height: px(36),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    Some(tip),
                    |b, id| {
                        b.spawn((
                            super::widgets::icon(&fonts, glyph, 20.0, t::SLOT_INK),
                            ButtonFg {
                                owner: id,
                                base: t::SLOT_INK,
                            },
                        ));
                    },
                );
            }
        });
}

fn ribbon_style(kind: AlertKind) -> PlateStyle {
    let slant = [0.0, 0.5, 0.0, 0.5];
    match kind {
        AlertKind::Warning => PlateStyle::fill(t::WARNING, t::mix(t::WARNING, Color::BLACK, 0.08), t::WARNING),
        AlertKind::Danger => PlateStyle::fill(
            t::mix(t::NEGATIVE, Color::BLACK, 0.12),
            t::mix(t::NEGATIVE, Color::BLACK, 0.25),
            t::NEGATIVE,
        ),
        AlertKind::Note => PlateStyle::fill(t::PLATE_TOP, t::PLATE_BOTTOM, t::LINE),
    }
    .slanted(slant)
}

/// Rejected commands: the tools refuse at the cursor themselves; anything
/// the simulation still refuses shows on the ribbon.
fn command_alerts(last: Res<LastCommand>, mut out: MessageWriter<Alert>) {
    if !last.is_changed() {
        return;
    }
    if let Some((_, Err(e))) = &last.result {
        out.write(Alert {
            kind: AlertKind::Danger,
            text: error_text(e),
        });
    }
}

fn update_city(
    sim: Res<SimWorld>,
    time: Res<Time<Real>>,
    mut texts: Query<(&TopText, &mut Text)>,
    mut fill: Query<&mut Node, With<MilestoneFill>>,
    mut shown: Local<Option<f32>>,
) {
    let w = &sim.world;
    let pop = w.stats.population;
    let (tier, next, f) = milestone(pop);
    let cur = shown.get_or_insert(f);
    *cur += (f - *cur) * approach(time.delta_secs(), 0.4);
    if (f - *cur).abs() < 0.001 {
        *cur = f;
    }
    for (which, mut text) in &mut texts {
        let v = match which {
            TopText::City => city_name(w.seed).to_uppercase(),
            TopText::Milestone => match next {
                Some((n, at)) => format!("{tier} · next: {n} at {}", format::group(at as u64)).to_uppercase(),
                None => tier.to_uppercase(),
            },
            TopText::Ribbon => continue,
        };
        if text.0 != v {
            text.0 = v;
        }
    }
    if let Ok(mut n) = fill.single_mut() {
        let wv = percent(*cur * 100.0);
        if n.width != wv {
            n.width = wv;
        }
    }
}

/// The alert that matters most right now.
fn current_alert(w: &citysim_sim::World, clock: &SimClock, ui: &UiState, transient: Option<&Alert>) -> Option<Alert> {
    if ui.menu_open {
        return None;
    }
    if let Some(a) = transient {
        return Some(a.clone());
    }
    if let Some(n) = super::fiscal::fiscal_note(w)
        && n.color == t::NEGATIVE
    {
        let money = format::money(w.economy.money);
        return Some(Alert {
            kind: AlertKind::Danger,
            text: format!("{} · {money} · {}", n.title, n.fix),
        });
    }
    if w.economy.money < 0 {
        return Some(Alert {
            kind: AlertKind::Danger,
            text: format!(
                "In debt · {} · new roads wait until money is above zero",
                format::money(w.economy.money)
            ),
        });
    }
    if clock.paused {
        return Some(Alert {
            kind: AlertKind::Note,
            text: "Paused · Space to resume".into(),
        });
    }
    let hour = w.time.hour();
    let share = w.stats.congestion_share;
    let rush = if (6.5..9.5).contains(&hour) {
        Some("Morning rush")
    } else if (16.0..19.5).contains(&hour) {
        Some("Evening rush")
    } else {
        None
    };
    if let Some(r) = rush
        && share >= 0.12
        && w.stats.population >= 300
    {
        let busy = (share * 100.0).round() as i64;
        return Some(Alert {
            kind: AlertKind::Warning,
            text: format!("{r} is building · {busy}% of roads busy"),
        });
    }
    None
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_ribbon(
    time: Res<Time<Real>>,
    sim: Res<SimWorld>,
    clock: Res<SimClock>,
    ui: Res<UiState>,
    mut alerts: ResMut<Alerts>,
    pinned: Res<PinnedAlert>,
    mut incoming: MessageReader<Alert>,
    mut root: Query<(&mut RibbonRoot, &mut UiTransform, &mut Visibility)>,
    mut plate: Query<&mut PlateAnim, With<RibbonPlate>>,
    mut icon_q: Query<(&mut Text, &mut TextColor), (With<RibbonIcon>, Without<TopText>)>,
    mut texts: Query<(&TopText, &mut Text, &mut TextColor), Without<RibbonIcon>>,
) {
    let dt = time.delta_secs();
    for a in incoming.read() {
        alerts.transient = Some((a.clone(), 0.0));
    }
    if let Some((_, age)) = &mut alerts.transient {
        *age += dt;
        if *age > TRANSIENT_S {
            alerts.transient = None;
        }
    }
    let transient = pinned.0.as_ref().or(alerts.transient.as_ref().map(|(a, _)| a));
    let want = current_alert(&sim.world, &clock, &ui, transient);
    let Ok((mut r, mut tf, mut vis)) = root.single_mut() else {
        return;
    };
    let want_key = want.as_ref().map(|a| (a.kind, a.text.clone()));
    // A new alert replaces the shown one once the old has slid up.
    let target = if want_key.is_some() && want_key == r.shown {
        1.0
    } else {
        0.0
    };
    r.t += (target - r.t) * approach(dt, 0.08);
    if (target - r.t).abs() < 0.004 {
        r.t = target;
    }
    if r.t == 0.0 && want_key != r.shown {
        r.shown = want_key.clone();
        if let Some(a) = &want {
            let (glyph, ink) = match a.kind {
                AlertKind::Warning => (icon::ALERT, t::WARNING_INK),
                AlertKind::Danger => (icon::ALERT, t::TEXT),
                AlertKind::Note => (icon::PAUSE, t::TEXT),
            };
            if let Ok(mut p) = plate.single_mut() {
                p.style = ribbon_style(a.kind);
            }
            for (mut tx, mut c) in &mut icon_q {
                tx.0 = glyph.to_string();
                c.0 = ink;
            }
            for (which, mut tx, mut c) in &mut texts {
                if *which == TopText::Ribbon {
                    tx.0 = a.text.to_uppercase();
                    c.0 = ink;
                }
            }
        }
    }
    let y = Val2::px(0.0, -48.0 * (1.0 - r.t));
    if tf.translation != y {
        tf.translation = y;
    }
    vis.set_if_neq(if r.t > 0.0 && r.shown.is_some() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milestones() {
        assert_eq!(milestone(0).0, "Hamlet");
        let (tier, next, f) = milestone(5_120);
        assert_eq!(tier, "Town");
        assert_eq!(next, Some(("City", 7_500)));
        assert!((f - (5_120.0 - 1_500.0) / 6_000.0).abs() < 1e-5);
        assert_eq!(milestone(200_000).1, None);
    }

    #[test]
    fn city_names_are_stable() {
        assert_eq!(city_name(7), city_name(7));
        assert!(city_name(1).len() >= 5);
    }
}
