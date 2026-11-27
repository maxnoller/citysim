//! The city window: the pages of figures (statistics, people, budget) in
//! one large plate between the top edge and the bar, centred (left of the
//! side panel while that is open). A header with the page's name, a live
//! line and the close button; a side list of the pages; the page itself
//! scrolls. Charts of the history the simulation keeps (`chart.rs`), live
//! figures as tiles, and the tables the drawers used to show.
//!
//! The game keeps running behind it (the clock, speed and the side panel
//! stay usable, the map can still be clicked around it); picking a build
//! tool closes it. Esc closes it, Tab / Shift+Tab change the page, F6 moves
//! the keyboard into it (arrows, Enter). P, U and the statistics button
//! open it on their page, again closes it.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use citysim_sim::citizens::LifeStage;
use citysim_sim::{TripPurpose, World};

use super::chart::{self, ChartData, ChartNodes, Column};
use super::format;
use super::people;
use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ActiveMark, ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, caps, figure, label, layout, plate_node,
    plate_shadow, spawn_button, spawn_key,
};
use super::{CityPage, UiState};
use crate::shared::{GameSet, SimWorld};

pub struct CityPlugin;

impl Plugin for CityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindowArea>()
            .init_resource::<LossHistory>()
            .add_systems(Startup, spawn_window)
            .add_systems(
                Update,
                window_keys.in_set(GameSet::Input).after(super::controls::control_keys),
            )
            .add_systems(
                Update,
                (
                    perf_room,
                    place_window.after(perf_room),
                    update_header,
                    update_tiles,
                    record_losses,
                    update_charts,
                )
                    .in_set(GameSet::Render),
            )
            .add_systems(PostUpdate, window_area.after(bevy::ui::UiSystems::Layout));
    }
}

/// Business tax lost to traffic per booked day, read from the history
/// (`DaySummary::traffic`; days booked by older saves have none and are
/// left out): (day, tax lost, time lost per commute).
#[derive(Resource, Debug, Default)]
pub struct LossHistory {
    key: Option<(u64, usize)>,
    pub days: Vec<(u32, i64, f32)>,
}

fn record_losses(sim: Res<SimWorld>, mut h: ResMut<LossHistory>) {
    let e = &sim.world.economy;
    let key = Some((sim.generation, e.history.len()));
    if h.key == key {
        return;
    }
    h.key = key;
    h.days = e
        .history
        .iter()
        .filter_map(|d| d.traffic.map(|t| (d.day, t.business_tax, t.per_commute_s())))
        .collect();
    let extra = h.days.len().saturating_sub(DAYS);
    h.days.drain(..extra);
}

/// Where the window's plate is on screen while it shows, in interface
/// (logical) px and in physical px: the cursor readout keeps off it, the
/// keyboard focus stays in it.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct WindowArea {
    pub logical: Option<Rect>,
    pub physical: Option<Rect>,
}

#[derive(Component)]
struct CityWindow {
    /// 0 closed .. 1 open.
    t: f32,
    shown: Option<CityPage>,
}

#[derive(Component)]
pub struct WindowPlate;

/// The body of a page (shown while its page is).
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct PageBody(pub CityPage);

/// Where the city's people sheet goes (filled by `panel.rs`).
#[derive(Component)]
pub struct CitySheetBody;

/// Where the budget's tables go (filled by `budget.rs`).
#[derive(Component)]
pub struct BudgetBody;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum HeaderText {
    Title,
    Subtitle,
    Clock,
}

/// A live figure tile: page, index, caption (else value).
#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct Tile(CityPage, usize, bool);

/// The line under the statistics tiles: why people are out of work.
#[derive(Component)]
struct JobsNote;

/// The page's scrolling body.
#[derive(Component)]
struct PageScroll;

/// Margins around the window (logical px).
const TOP: f32 = 60.0;
const SIDE: f32 = 18.0;
const BOTTOM: f32 = t::BAR_H + 12.0;
/// The window's largest size.
const MAX_W: f32 = 1480.0;
const MAX_H: f32 = 940.0;
/// Width of the side list of pages.
const LIST_W: f32 = 196.0;

/// Days the charts show (the newest on the right).
pub const DAYS: usize = 30;

// Chart ids.
const C_POPULATION: u32 = 1;
const C_JOBS: u32 = 2;
const C_MONEY: u32 = 3;
const C_COMMUTE: u32 = 4;
const C_TRAFFIC: u32 = 5;
const C_BUILDINGS: u32 = 6;
const C_STAGES: u32 = 10;
const C_HAPPY: u32 = 11;
const C_HOURS: u32 = 12;
const C_HOUSEHOLDS: u32 = 13;
pub const C_FLOWS: u32 = 20;
const C_LOSS: u32 = 21;

const STATS_TILES: usize = 12;
const PEOPLE_TILES: usize = 6;

fn spawn_window(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
) {
    let fonts = fonts.into_inner();
    let mat = cache.get(&mut mats, PlateStyle::PANEL);
    commands
        .spawn((
            Name::new("City window"),
            CityWindow { t: 0.0, shown: None },
            layout(Node {
                position_type: PositionType::Absolute,
                left: px(SIDE),
                top: px(TOP),
                width: px(MAX_W),
                height: px(600),
                ..default()
            }),
            UiTransform::default(),
            GlobalZIndex(40),
            Visibility::Hidden,
        ))
        .with_children(|root| {
            root.spawn((
                WindowPlate,
                plate_node(
                    Node {
                        width: percent(100),
                        height: percent(100),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    UiRect::all(px(t::LW)),
                    [t::CUT, 0.0, t::CUT, 0.0],
                ),
                mat,
                plate_shadow(),
                // Catches the pointer: the map under it is not hovered.
                Pickable::default(),
            ))
            .with_children(|plate| {
                header(plate, fonts);
                plate
                    .spawn(layout(Node {
                        flex_grow: 1.0,
                        min_height: px(0),
                        ..default()
                    }))
                    .with_children(|body| {
                        side_list(body, fonts);
                        body.spawn((
                            PageScroll,
                            Node {
                                flex_grow: 1.0,
                                min_width: px(0),
                                flex_direction: FlexDirection::Column,
                                overflow: Overflow::scroll_y(),
                                padding: UiRect::new(px(26), px(24), px(20), px(24)),
                                ..default()
                            },
                            ScrollPosition::default(),
                            Hovered::default(),
                        ))
                        .with_children(|pages| {
                            stats_page(pages, fonts);
                            people_page(pages, fonts);
                            budget_page(pages, fonts);
                        });
                    });
            });
        });
}

/// The header strip: accent bar, the page's name, a live line, close.
fn header(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    p.spawn((
        layout(Node {
            height: px(54),
            padding: UiRect::new(px(26), px(10), px(0), px(0)),
            column_gap: px(14),
            align_items: AlignItems::Center,
            border: UiRect::bottom(px(t::LW)),
            flex_shrink: 0.0,
            ..default()
        }),
        // No fill of its own: the plate's chamfered corner shows.
        BorderColor::all(t::LINE),
    ))
    .with_children(|h| {
        h.spawn((
            layout(Node {
                width: px(5),
                height: px(24),
                flex_shrink: 0.0,
                ..default()
            }),
            BackgroundColor(t::ACCENT),
        ));
        h.spawn((
            Text::new(""),
            fonts.figures(t::FONT_TITLE, t::W_BOLD),
            t::caps_spacing(t::FONT_TITLE, 0.12),
            TextColor(t::TEXT),
            TextLayout::no_wrap(),
            Pickable::IGNORE,
            HeaderText::Title,
        ));
        h.spawn((
            label(fonts, "", t::FONT_BODY, t::W_MEDIUM, t::TEXT_MUTED),
            Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: px(0),
                overflow: Overflow::clip(),
                ..default()
            },
            HeaderText::Subtitle,
        ));
        // Live: a lit dot and the clock.
        h.spawn((
            Node {
                align_items: AlignItems::Center,
                column_gap: px(8),
                flex_shrink: 0.0,
                ..default()
            },
            Tooltip::new(
                "Live",
                "The city keeps running behind this window; every figure follows it.",
            ),
            Hovered::default(),
        ))
        .with_children(|l| {
            l.spawn((
                layout(Node {
                    width: px(7),
                    height: px(7),
                    ..default()
                }),
                BackgroundColor(t::POSITIVE),
                BoxShadow(vec![ShadowStyle {
                    color: t::alpha(t::POSITIVE, 0.8),
                    x_offset: px(0),
                    y_offset: px(0),
                    spread_radius: px(0),
                    blur_radius: px(7),
                }]),
            ));
            l.spawn((caps(fonts, "", t::FONT_CAP, t::SLOT_INK), HeaderText::Clock));
        });
        h.spawn(layout(Node {
            width: px(t::LW),
            height: px(26),
            margin: UiRect::horizontal(px(4)),
            ..default()
        }))
        .insert(BackgroundColor(t::LINE));
        let id = spawn_button(
            h,
            UiButton::new(UiAction::CloseWindow, ButtonKind::Ghost),
            Node {
                height: px(36),
                padding: UiRect::horizontal(px(9)),
                column_gap: px(8),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                flex_shrink: 0.0,
                ..default()
            },
            Some(Tooltip::new("Close", "Back to the city.").key("Esc")),
            |b, id| {
                spawn_key(b, fonts, "Esc", t::TEXT_MUTED);
                b.spawn((
                    super::widgets::icon(fonts, icon::CLOSE, 20.0, t::TEXT_MUTED),
                    ButtonFg {
                        owner: id,
                        base: t::TEXT_MUTED,
                    },
                ));
            },
        );
        h.commands().entity(id).insert(Name::new("Close window"));
    });
}

fn page_icon(p: CityPage) -> char {
    match p {
        CityPage::Stats => icon::CHART,
        CityPage::People => icon::USERS,
        CityPage::Budget => icon::WALLET,
    }
}

fn page_key(p: CityPage) -> Option<&'static str> {
    match p {
        CityPage::Stats => None,
        CityPage::People => Some("P"),
        CityPage::Budget => Some("U"),
    }
}

fn page_desc(p: CityPage) -> &'static str {
    match p {
        CityPage::Stats => "Population, work, money, commutes and traffic, day by day.",
        CityPage::People => "Ages, households, work, happiness and its reasons, trips.",
        CityPage::Budget => "Income and costs by line and by day, the tax rate.",
    }
}

/// The side list: one row per page, a hint at the foot.
fn side_list(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    p.spawn((
        layout(Node {
            width: px(LIST_W),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            padding: UiRect::new(px(0), px(0), px(14), px(14)),
            border: UiRect::right(px(1)),
            row_gap: px(2),
            ..default()
        }),
        BorderColor::all(t::RULE),
        BackgroundColor(t::alpha(t::WELL, 0.35)),
    ))
    .with_children(|l| {
        for page in CityPage::ALL {
            let mut tip = Tooltip::new(page.title(), page_desc(page));
            tip.key = page_key(page);
            let id = spawn_button(
                l,
                UiButton::new(UiAction::CityPage(page), ButtonKind::Row),
                Node {
                    height: px(46),
                    padding: UiRect::new(px(20), px(14), px(0), px(0)),
                    column_gap: px(12),
                    align_items: AlignItems::Center,
                    ..default()
                },
                Some(tip),
                |b, id| {
                    b.spawn((
                        ActiveMark { owner: id },
                        layout(Node {
                            position_type: PositionType::Absolute,
                            left: px(0),
                            top: px(8),
                            bottom: px(8),
                            width: px(4),
                            ..default()
                        }),
                        BackgroundColor(t::ACCENT),
                        Visibility::Hidden,
                    ));
                    b.spawn((
                        super::widgets::icon(fonts, page_icon(page), t::ICON_MD, t::SLOT_INK),
                        ButtonFg {
                            owner: id,
                            base: t::SLOT_INK,
                        },
                    ));
                    b.spawn((
                        label(fonts, page.title(), t::FONT_ROW + 1.0, t::W_SEMIBOLD, t::SLOT_INK),
                        ButtonFg {
                            owner: id,
                            base: t::SLOT_INK,
                        },
                        Node {
                            flex_grow: 1.0,
                            ..default()
                        },
                    ));
                    if let Some(k) = page_key(page) {
                        spawn_key(b, fonts, k, t::TEXT_MUTED);
                    }
                },
            );
            l.commands()
                .entity(id)
                .insert(Name::new(format!("Page {}", page.title())));
        }
        l.spawn(layout(Node {
            flex_grow: 1.0,
            ..default()
        }));
        l.spawn((
            super::widgets::paragraph(
                fonts,
                "Tab: next page\nF6: keyboard into the window\nThe city keeps running",
                t::FONT_SMALL - 0.5,
                t::W_MEDIUM,
                t::TEXT_FAINT,
            ),
            Node {
                margin: UiRect::horizontal(px(20)),
                ..default()
            },
        ));
    });
}

fn page(p: &mut ChildSpawnerCommands, which: CityPage, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn((
        PageBody(which),
        Name::new(format!("City page {which:?}")),
        layout(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(26),
            display: Display::None,
            width: percent(100),
            flex_shrink: 0.0,
            ..default()
        }),
    ))
    .with_children(f);
}

/// A titled group of a page.
fn group(p: &mut ChildSpawnerCommands, fonts: &UiFonts, title: &str, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(12),
        width: percent(100),
        ..default()
    }))
    .with_children(|g| {
        if !title.is_empty() {
            g.spawn(layout(Node {
                align_items: AlignItems::Center,
                column_gap: px(10),
                ..default()
            }))
            .with_children(|h| {
                h.spawn(caps(fonts, title, t::FONT_CAP + 0.5, t::TEXT_MUTED));
                h.spawn((
                    layout(Node {
                        flex_grow: 1.0,
                        height: px(1),
                        ..default()
                    }),
                    BackgroundColor(t::RULE),
                ));
            });
        }
        f(g);
    });
}

/// A grid of cells at least `min` px wide that share the row.
fn grid(p: &mut ChildSpawnerCommands, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn(layout(Node {
        flex_wrap: FlexWrap::Wrap,
        column_gap: px(26),
        row_gap: px(22),
        width: percent(100),
        align_items: AlignItems::FlexStart,
        ..default()
    }))
    .with_children(f);
}

/// A cell of a grid: grows to share its row, never narrower than `min`.
pub fn cell(p: &mut ChildSpawnerCommands, min: f32, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Column,
        flex_grow: 1.0,
        flex_shrink: 1.0,
        flex_basis: px(min),
        min_width: px(min),
        ..default()
    }))
    .with_children(f);
}

/// A row of live figure tiles.
fn tiles(p: &mut ChildSpawnerCommands, fonts: &UiFonts, page: CityPage, n: usize) {
    p.spawn(layout(Node {
        flex_wrap: FlexWrap::Wrap,
        column_gap: px(10),
        row_gap: px(10),
        width: percent(100),
        ..default()
    }))
    .with_children(|r| {
        for i in 0..n {
            r.spawn((
                layout(Node {
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::Center,
                    row_gap: px(6),
                    flex_grow: 1.0,
                    flex_basis: px(150),
                    min_width: px(140),
                    height: px(66),
                    padding: UiRect::horizontal(px(14)),
                    border: UiRect::left(px(3)),
                    ..default()
                }),
                BackgroundColor(t::alpha(t::WELL, 0.55)),
                BorderColor::all(t::LINE),
            ))
            .with_children(|c| {
                c.spawn((figure(fonts, "", 24.0, t::TEXT), Tile(page, i, false)));
                c.spawn((caps(fonts, "", 10.5, t::TEXT_MUTED), Tile(page, i, true)));
            });
        }
    });
}

fn stats_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    page(p, CityPage::Stats, |pg| {
        group(pg, fonts, "Now", |g| {
            tiles(g, fonts, CityPage::Stats, STATS_TILES);
            // Why people are out of work (with the jobs figures).
            g.spawn(layout(Node {
                align_items: AlignItems::Center,
                column_gap: px(10),
                ..default()
            }))
            .with_children(|r| {
                r.spawn(caps(fonts, "Work", t::FONT_CAP, t::TEXT_MUTED));
                r.spawn((label(fonts, "", t::FONT_BODY, t::W_MEDIUM, t::SLOT_INK), JobsNote));
            });
        });
        group(pg, fonts, "Day by day", |g| {
            grid(g, |gr| {
                let s = &t::CHART_SERIES;
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(c, fonts, C_POPULATION, "Residents", DAYS + 1, 132.0, &[])
                });
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(
                        c,
                        fonts,
                        C_JOBS,
                        "Jobs",
                        DAYS + 1,
                        132.0,
                        &[("Filled", s[0]), ("Vacant", s[4])],
                    )
                });
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(c, fonts, C_MONEY, "Money in the bank", DAYS + 1, 132.0, &[])
                });
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(c, fonts, C_COMMUTE, "Average commute", DAYS + 1, 132.0, &[])
                });
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(
                        c,
                        fonts,
                        C_TRAFFIC,
                        "Cars on the road at the peak",
                        DAYS,
                        132.0,
                        &[
                            ("Flowing", t::POSITIVE),
                            ("Busy day", t::WARNING),
                            ("Jammed day", t::NEGATIVE),
                        ],
                    )
                });
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(c, fonts, C_BUILDINGS, "Buildings", DAYS + 1, 132.0, &[])
                });
            });
        });
    });
}

fn people_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    page(p, CityPage::People, |pg| {
        group(pg, fonts, "Now", |g| tiles(g, fonts, CityPage::People, PEOPLE_TILES));
        group(pg, fonts, "Day by day", |g| {
            grid(g, |gr| {
                let s = &t::CHART_SERIES;
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(
                        c,
                        fonts,
                        C_STAGES,
                        "Residents by age",
                        DAYS,
                        132.0,
                        &[
                            ("Children", s[1]),
                            ("Teenagers", s[2]),
                            ("Adults", s[0]),
                            ("Seniors", s[3]),
                        ],
                    )
                });
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(c, fonts, C_HAPPY, "Happiness", DAYS, 132.0, &[])
                });
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(
                        c,
                        fonts,
                        C_HOUSEHOLDS,
                        "Households",
                        DAYS,
                        132.0,
                        &[("Single", s[1]), ("Couple", s[2]), ("Family", s[0]), ("Seniors", s[3])],
                    )
                });
                cell(gr, 380.0, |c| {
                    chart::spawn_chart(
                        c,
                        fonts,
                        C_HOURS,
                        "Trips today by hour",
                        24,
                        132.0,
                        &[
                            ("Work", s[0]),
                            ("Home", s[1]),
                            ("Shops & leisure", s[2]),
                            ("Goods", s[4]),
                            ("Moving", s[3]),
                        ],
                    )
                });
            });
        });
        group(pg, fonts, "The people", |g| {
            g.spawn((
                CitySheetBody,
                layout(Node {
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(26),
                    row_gap: px(16),
                    width: percent(100),
                    align_items: AlignItems::FlexStart,
                    ..default()
                }),
            ));
        });
    });
}

fn budget_page(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    page(p, CityPage::Budget, |pg| {
        pg.spawn((
            BudgetBody,
            layout(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(26),
                width: percent(100),
                ..default()
            }),
        ));
        group(pg, fonts, "Day by day", |g| {
            let s = &t::CHART_SERIES;
            chart::spawn_chart(
                g,
                fonts,
                C_FLOWS,
                "Income and costs per day",
                DAYS,
                150.0,
                &[
                    ("Residential tax", t::ZONE_RESIDENTIAL),
                    ("Commercial tax", t::ZONE_COMMERCIAL),
                    ("Industrial tax", t::ZONE_INDUSTRIAL),
                    ("Road upkeep", s[4]),
                    ("City services", s[3]),
                ],
            );
            chart::spawn_chart(
                g,
                fonts,
                C_LOSS,
                "Business tax lost to traffic per day",
                DAYS,
                110.0,
                &[],
            );
        });
    });
}

// ---------------------------------------------------------------- systems

/// Tab / Shift+Tab: the next / previous page.
fn window_keys(keys: Res<ButtonInput<KeyCode>>, mut ui: ResMut<UiState>) {
    if ui.menu_open {
        return;
    }
    if let Some(p) = ui.window
        && keys.just_pressed(KeyCode::Tab)
    {
        let back = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        ui.window = Some(p.next(if back { -1 } else { 1 }));
    }
}

/// Room the window needs beside the full performance panel (logical px).
const BESIDE_PERF_W: f32 = 880.0;

/// The window's right edge (logical px): left of the side panel while it
/// is open, else the screen's.
fn right_edge(panel: &super::panel::PanelArea, screen: Vec2) -> f32 {
    match panel.0 {
        Some(r) => r.min.x - 14.0,
        None => screen.x - SIDE,
    }
}

/// The performance panel's form on screen: the player's choice, except the
/// full panel steps down to its small strip while the city window is open
/// and there is no room for both side by side (the window then sits below
/// the strip); it comes back full when the window closes. Whatever else
/// sets `UiState::perf` (F3, the panel's buttons, the settings, the
/// environment) sets the choice.
fn perf_room(
    mut ui: ResMut<UiState>,
    ui_scale: Res<UiScale>,
    panel: Res<super::panel::PanelArea>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut written: Local<Option<crate::shared::PerfDetail>>,
) {
    use crate::shared::PerfDetail;
    if *written != Some(ui.perf) && ui.perf_choice != ui.perf {
        ui.perf_choice = ui.perf;
    }
    let Ok(win) = window.single() else { return };
    let screen = Vec2::new(win.width(), win.height()) / ui_scale.0.max(0.01);
    let room = right_edge(&panel, screen) - (super::perf::FULL_W + 14.0) >= BESIDE_PERF_W;
    let crowded = ui.window.is_some() && !ui.menu_open && !room;
    let shown = if ui.perf_choice == PerfDetail::Full && crowded {
        PerfDetail::Small
    } else {
        ui.perf_choice
    };
    if ui.perf != shown {
        ui.perf = shown;
    }
    *written = Some(shown);
}

/// Size and place the window, slide it in and out, show its page.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn place_window(
    time: Res<Time<Real>>,
    ui: Res<UiState>,
    ui_scale: Res<UiScale>,
    panel: Res<super::panel::PanelArea>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut root: Query<(&mut CityWindow, &mut Node, &mut UiTransform, &mut Visibility)>,
    mut pages: Query<(&PageBody, &mut Node), Without<CityWindow>>,
    mut scroll: Query<&mut ScrollPosition, With<PageScroll>>,
    perf_full: Query<(&super::perf::FullRoot, &ComputedNode)>,
) {
    let Ok((mut w, mut node, mut tf, mut vis)) = root.single_mut() else {
        return;
    };
    let want = ui.window.filter(|_| !ui.menu_open);
    if want.is_some() && want != w.shown && (w.t <= 0.001 || w.shown.is_some()) {
        w.shown = want;
        for (p, mut n) in &mut pages {
            let d = if Some(p.0) == want {
                Display::Flex
            } else {
                Display::None
            };
            if n.display != d {
                n.display = d;
            }
        }
        if let Ok(mut s) = scroll.single_mut() {
            s.0 = Vec2::ZERO;
        }
    }
    let target = if want.is_some() { 1.0 } else { 0.0 };
    let tau = if target > w.t { 0.06 } else { 0.035 };
    w.t += (target - w.t) * approach(time.delta_secs(), tau);
    if (target - w.t).abs() < 0.003 {
        w.t = target;
    }
    if w.t == 0.0 {
        w.shown = None;
    }
    vis.set_if_neq(if w.t > 0.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    });
    let e = 1.0 - (1.0 - w.t).powi(2);
    let y = Val2::px(0.0, (1.0 - e) * 18.0);
    if tf.translation != y {
        tf.translation = y;
    }
    // Size: as large as the room between the top edge and the bar, left of
    // the side panel while it is open, centred when there is room.
    let Ok(win) = window.single() else { return };
    let screen = Vec2::new(win.width(), win.height()) / ui_scale.0.max(0.01);
    let right_edge = right_edge(&panel, screen);
    // Beside the full performance panel (docked left) when there is room
    // for a useful window, else over it; below the small one.
    let mut left_edge = SIDE;
    let mut top_edge = TOP;
    match ui.perf {
        crate::shared::PerfDetail::Full => {
            let w = perf_full
                .single()
                .map(|(r, c)| c.size.x * c.inverse_scale_factor * r.fit)
                .unwrap_or(super::perf::FULL_W);
            if right_edge - (w + 14.0) >= BESIDE_PERF_W {
                left_edge = w + 14.0;
            }
        }
        crate::shared::PerfDetail::Small => {
            top_edge = top_edge.max(super::perf::TOP + super::perf::SMALL_H + 8.0);
        }
        crate::shared::PerfDetail::Off => {}
    }
    let avail_w = (right_edge - left_edge).max(400.0);
    let width = avail_w.min(MAX_W);
    let left = (screen.x * 0.5 - width * 0.5).clamp(left_edge, (right_edge - width).max(left_edge));
    let avail_h = (screen.y - top_edge - BOTTOM).max(300.0);
    let height = avail_h.min(MAX_H);
    let top = top_edge + (avail_h - height) * 0.5;
    let (l, tp, wd, ht) = (px(left.round()), px(top.round()), px(width.round()), px(height.round()));
    if node.left != l || node.top != tp || node.width != wd || node.height != ht {
        node.left = l;
        node.top = tp;
        node.width = wd;
        node.height = ht;
    }
}

/// Record where the window's plate is (logical px), while it is up.
fn window_area(
    roots: Query<(&CityWindow, &InheritedVisibility)>,
    plate: Query<(&ComputedNode, &UiGlobalTransform), With<WindowPlate>>,
    mut area: ResMut<WindowArea>,
) {
    let open = roots.iter().any(|(r, v)| v.get() && r.t > 0.05);
    let got = plate.single().ok().filter(|_| open);
    let new = WindowArea {
        logical: got.map(|(n, tf)| {
            let inv = n.inverse_scale_factor;
            Rect::from_center_size(tf.translation * inv, n.size * inv)
        }),
        physical: got.map(|(n, tf)| Rect::from_center_size(tf.translation, n.size)),
    };
    area.set_if_neq(new);
}

#[allow(clippy::type_complexity)]
fn update_header(
    ui: Res<UiState>,
    sim: Res<SimWorld>,
    mut texts: Query<(&HeaderText, &mut Text)>,
    mut last: Local<Option<(Option<CityPage>, u32, u32, u32, u32)>>,
) {
    let Some(page) = ui.window else {
        *last = None;
        return;
    };
    let w = &sim.world;
    let step = ((w.time.hour() * 6.0) as u32).min(143);
    let key = (
        Some(page),
        w.time.day,
        step,
        w.stats.population,
        w.people.stats.households,
    );
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    for (which, mut text) in &mut texts {
        let v = match which {
            HeaderText::Title => page.title().to_uppercase(),
            HeaderText::Subtitle => subtitle(w, page),
            HeaderText::Clock => format!(
                "Live · {} · {:02}:{:02}",
                super::bar::day_label(w.time.day),
                step / 6,
                step % 6 * 10
            )
            .to_uppercase(),
        };
        if text.0 != v {
            text.0 = v;
        }
    }
}

fn subtitle(w: &World, page: CityPage) -> String {
    let name = super::top::city_name(w.seed);
    match page {
        CityPage::Stats => format!(
            "{name} · {} residents · {} jobs",
            format::group(w.stats.population as u64),
            format::group(w.stats.jobs as u64)
        ),
        CityPage::People => format!(
            "{name} · {} residents in {} households",
            format::group(w.people.stats.population as u64),
            format::group(w.people.stats.households as u64)
        ),
        CityPage::Budget if w.economy.history.is_empty() => format!(
            "{name} · {} in the bank · the first day is booked at midnight",
            format::money(w.economy.money)
        ),
        CityPage::Budget => format!(
            "{name} · {} in the bank · {} an hour, settled at midnight",
            format::money(w.economy.money),
            format::signed_money(w.economy.last_day.net() / 24)
        ),
    }
}

/// A per-tick share eased over a few seconds (it jumps with every light).
#[derive(Default)]
struct Smoothed(f32, bool);

impl Smoothed {
    fn add(&mut self, v: f32, dt: f32) {
        if !self.1 {
            *self = Smoothed(v, true);
        } else {
            self.0 += (v - self.0) * (dt / 3.0).min(1.0);
        }
    }
}

/// "+40%": how much longer car trips take than on empty roads.
pub fn delay_stat(w: &World) -> (&'static str, String, Color) {
    let f = citysim_sim::traffic::delay_factor(w);
    let extra = f - 1.0;
    (
        "Trip delay",
        if w.stats.vehicles_on_road == 0 && extra < 0.005 {
            "\u{2014}".into()
        } else {
            format!("+{}", super::context::pct(extra))
        },
        if extra > 1.0 {
            t::NEGATIVE
        } else if extra > 0.5 {
            t::WARNING
        } else {
            t::TEXT
        },
    )
}

/// Trips made on foot today against all trips started (by car, lift or on
/// foot): yesterday's until today has a few.
fn walk_stat(w: &World) -> (&'static str, String, Color) {
    let st = &w.walking.stats;
    let (on_foot, other) = if st.walked_today + st.other_today >= 50 {
        (st.walked_today, st.other_today)
    } else {
        (st.walked_yesterday, st.other_yesterday)
    };
    let all = on_foot + other;
    (
        "Trips on foot today",
        if all == 0 {
            "\u{2014}".into()
        } else {
            format!(
                "{} · {}",
                super::context::pct(on_foot as f32 / all as f32),
                format::group(on_foot as u64)
            )
        },
        t::TEXT,
    )
}

/// The statistics page's live figures (what the statistics drawer showed).
fn stats_tiles(w: &World, standing: f32) -> [(&'static str, String, Color); STATS_TILES] {
    let s = &w.stats;
    let g = |v: u32| format::group(v as u64);
    [
        ("Residents", g(s.population), t::TEXT),
        walk_stat(w),
        ("Workforce", g(s.workforce), t::TEXT),
        ("Employed", g(s.employed), t::TEXT),
        (
            "Jobless",
            g(s.unemployed),
            if s.unemployment > 0.15 { t::NEGATIVE } else { t::TEXT },
        ),
        ("Jobs", g(s.jobs), t::TEXT),
        ("Vacant jobs", g(s.vacant_jobs), t::TEXT),
        (
            "Avg commute",
            super::bar::commute_text(s.avg_commute_s),
            super::bar::commute_color(s.avg_commute_s),
        ),
        (
            "Cars · people on foot",
            format!("{} · {}", g(s.vehicles_on_road), g(w.walking.walkers.len() as u32)),
            t::TEXT,
        ),
        (
            "Cars standing",
            super::context::pct(standing),
            if standing > 0.5 { t::WARNING } else { t::TEXT },
        ),
        delay_stat(w),
        (
            "Homes · shops · industry",
            format!(
                "{} · {} · {}",
                s.buildings_by_kind[0], s.buildings_by_kind[1], s.buildings_by_kind[2]
            ),
            t::TEXT,
        ),
    ]
}

fn people_tiles(w: &World) -> [(&'static str, String, Color); PEOPLE_TILES] {
    let st = &w.people.stats;
    let g = |v: u32| format::group(v as u64);
    let h = st.avg_happiness;
    [
        ("Residents", g(st.population), t::TEXT),
        ("Households", g(st.households), t::TEXT),
        (
            "Happiness",
            format!("{} · {}", (h * 100.0).round() as i64, people::mood_word(h)),
            mood_color(h),
        ),
        (
            "Unhappy",
            g(st.unhappy),
            if st.unhappy as f32 > st.population as f32 * 0.15 {
                t::WARNING
            } else {
                t::TEXT
            },
        ),
        ("Average savings", format::money(st.avg_money as i64), t::TEXT),
        ("Rent of income", format::percent(st.avg_rent_burden), t::TEXT),
    ]
}

/// Why people are out of work, with the free jobs and the land for work.
fn jobs_note(w: &World) -> (String, Color) {
    use citysim_sim::demand::JoblessReason as R;
    let j = citysim_sim::demand::jobs_view(w);
    let mut parts = vec![format!(
        "{} jobless · {}",
        format::percent((j.rate() * 1000.0).round() / 1000.0),
        j.reason.text()
    )];
    if j.vacancies > 0 {
        parts.push(if j.vacancies_reachable < j.vacancies {
            format!(
                "{} free jobs, {} within reach",
                format::group(j.vacancies as u64),
                format::group(j.vacancies_reachable as u64)
            )
        } else {
            format!("{} free jobs", format::group(j.vacancies as u64))
        });
    }
    if let Some(z) = j.short_of {
        parts.push(format!(
            "short of {} land",
            super::widgets::zone_name(Some(z)).to_lowercase()
        ));
    }
    let c = match j.reason {
        R::None => t::TEXT_MUTED,
        R::Searching => t::SLOT_INK,
        _ => t::WARNING,
    };
    (parts.join(" · "), c)
}

pub fn mood_color(h: f32) -> Color {
    if h >= 0.7 {
        t::POSITIVE
    } else if h >= citysim_sim::balance::UNHAPPY_BELOW {
        t::WARNING
    } else {
        t::NEGATIVE
    }
}

fn update_tiles(
    time: Res<Time<Real>>,
    ui: Res<UiState>,
    sim: Res<SimWorld>,
    mut tiles: Query<(&Tile, &mut Text, &mut TextColor), Without<JobsNote>>,
    mut note: Query<(&mut Text, &mut TextColor), With<JobsNote>>,
    mut age: Local<f32>,
    mut smooth: Local<Smoothed>,
) {
    smooth.add(sim.world.traffic.stopped_share, time.delta_secs());
    let Some(page) = ui.window else { return };
    *age += time.delta_secs();
    if *age < 0.3 && !ui.is_changed() {
        return;
    }
    *age = 0.0;
    let w = &sim.world;
    let stats = (page == CityPage::Stats).then(|| stats_tiles(w, smooth.0));
    if page == CityPage::Stats
        && let Ok((mut text, mut color)) = note.single_mut()
    {
        let (v, c) = jobs_note(w);
        if text.0 != v {
            text.0 = v;
        }
        color.set_if_neq(TextColor(c));
    }
    let ppl = (page == CityPage::People).then(|| people_tiles(w));
    for (tile, mut text, mut color) in &mut tiles {
        let row = match tile.0 {
            CityPage::Stats => stats.as_ref().and_then(|s| s.get(tile.1)),
            CityPage::People => ppl.as_ref().and_then(|s| s.get(tile.1)),
            CityPage::Budget => None,
        };
        let Some((c, v, col)) = row else { continue };
        let (v, col) = if tile.2 {
            (c.to_uppercase(), t::TEXT_MUTED)
        } else {
            (v.clone(), *col)
        };
        if text.0 != v {
            text.0 = v;
        }
        color.set_if_neq(TextColor(col));
    }
}

// ---------------------------------------------------------------- chart data

fn day_label(d: u32) -> String {
    format!("Day {d}")
}

fn axis(first: Option<u32>, n: usize, live: bool) -> (String, String) {
    match first {
        Some(d) if n > 0 => (
            day_label(d),
            if live {
                "Now".into()
            } else {
                day_label(d + n as u32 - 1)
            },
        ),
        _ => (String::new(), String::new()),
    }
}

const NO_DAYS: &str = "The first day's figures come at midnight";

/// One chart's data from the economy's day history, plus a live column.
fn daily(
    w: &World,
    value: impl Fn(&citysim_sim::DaySummary) -> (Vec<(f32, Color)>, String),
    now: Option<(Vec<(f32, Color)>, String)>,
    top: impl Fn(f32) -> String,
) -> ChartData {
    let h = &w.economy.history;
    let days = &h[h.len().saturating_sub(DAYS)..];
    let mut columns: Vec<Column> = days
        .iter()
        .map(|d| {
            let (parts, value) = value(d);
            Column {
                parts,
                label: day_label(d.day),
                value,
            }
        })
        .collect();
    let live = now.is_some();
    if let Some((parts, value)) = now {
        columns.push(Column {
            parts,
            label: "Now".into(),
            value,
        });
    }
    let max = columns
        .iter()
        .map(|c| c.parts.iter().map(|p| p.0.max(0.0)).sum::<f32>())
        .fold(0.0, f32::max);
    let (first, last) = axis(days.first().map(|d| d.day), days.len(), live);
    ChartData {
        first,
        last,
        top: if max > 0.0 { top(max) } else { String::new() },
        empty: NO_DAYS.into(),
        live_last: live,
        pick: None,
        columns: if days.is_empty() && !live { Vec::new() } else { columns },
    }
}

fn people_daily(
    w: &World,
    value: impl Fn(&citysim_sim::PeopleDay) -> (Vec<(f32, Color)>, String),
    top: impl Fn(f32) -> String,
) -> ChartData {
    let h = &w.people.history;
    let days = &h[h.len().saturating_sub(DAYS)..];
    let columns: Vec<Column> = days
        .iter()
        .map(|d| {
            let (parts, value) = value(d);
            Column {
                parts,
                label: day_label(d.day),
                value,
            }
        })
        .collect();
    let max = columns
        .iter()
        .map(|c| c.parts.iter().map(|p| p.0.max(0.0)).sum::<f32>())
        .fold(0.0, f32::max);
    let (first, last) = axis(days.first().map(|d| d.day), days.len(), false);
    ChartData {
        first,
        last,
        top: if max > 0.0 { top(max) } else { String::new() },
        empty: NO_DAYS.into(),
        live_last: false,
        pick: None,
        columns,
    }
}

fn group_top(v: f32) -> String {
    format::group(v.round() as u64)
}

/// A commute's column: the accent while fine, amber and red when long.
fn commute_bar(secs: f32) -> Color {
    let c = super::bar::commute_color(secs);
    if c == t::TEXT { t::CHART_SERIES[0] } else { c }
}

fn traffic_color(peak_congestion: f32) -> Color {
    if peak_congestion > 0.25 {
        t::NEGATIVE
    } else if peak_congestion > 0.1 {
        t::WARNING
    } else {
        t::POSITIVE
    }
}

/// The data of chart `id` now.
pub fn chart_data(id: u32, w: &World, losses: &[(u32, i64, f32)]) -> ChartData {
    let s = &t::CHART_SERIES;
    let st = &w.stats;
    match id {
        C_POPULATION => daily(
            w,
            |d| {
                (
                    vec![(d.population as f32, s[0])],
                    format!("{} residents", format::group(d.population as u64)),
                )
            },
            Some((
                vec![(st.population as f32, s[0])],
                format!("{} residents", format::group(st.population as u64)),
            )),
            group_top,
        ),
        C_JOBS => daily(
            w,
            |d| {
                let vacant = d.jobs.saturating_sub(d.employed);
                (
                    vec![(d.employed as f32, s[0]), (vacant as f32, s[4])],
                    format!(
                        "{} jobs · {} vacant",
                        format::group(d.jobs as u64),
                        format::group(vacant as u64)
                    ),
                )
            },
            Some((
                vec![
                    (st.jobs.saturating_sub(st.vacant_jobs) as f32, s[0]),
                    (st.vacant_jobs as f32, s[4]),
                ],
                format!(
                    "{} jobs · {} vacant",
                    format::group(st.jobs as u64),
                    format::group(st.vacant_jobs as u64)
                ),
            )),
            group_top,
        ),
        C_MONEY => {
            let col = |m: i64| if m < 0 { t::NEGATIVE } else { s[0] };
            daily(
                w,
                |d| (vec![(d.money as f32, col(d.money))], format::money(d.money)),
                Some((
                    vec![(w.economy.money as f32, col(w.economy.money))],
                    format::money(w.economy.money),
                )),
                |v| format::money(v.round() as i64),
            )
        }
        C_COMMUTE => daily(
            w,
            |d| {
                (
                    vec![(d.avg_commute_s, commute_bar(d.avg_commute_s))],
                    super::bar::commute_text(d.avg_commute_s),
                )
            },
            Some((
                vec![(st.avg_commute_s, commute_bar(st.avg_commute_s))],
                super::bar::commute_text(st.avg_commute_s),
            )),
            super::bar::commute_text,
        ),
        C_TRAFFIC => daily(
            w,
            |d| {
                (
                    vec![(d.peak_vehicles as f32, traffic_color(d.peak_congestion))],
                    format!(
                        "{} cars · {} busy",
                        format::group(d.peak_vehicles as u64),
                        super::context::pct(d.peak_congestion)
                    ),
                )
            },
            None,
            group_top,
        ),
        C_BUILDINGS => daily(
            w,
            |d| {
                (
                    vec![(d.buildings as f32, s[1])],
                    format!("{} buildings", format::group(d.buildings as u64)),
                )
            },
            Some((
                vec![(st.buildings as f32, s[1])],
                format!("{} buildings", format::group(st.buildings as u64)),
            )),
            group_top,
        ),
        C_STAGES => people_daily(
            w,
            |d| {
                let order = [
                    (LifeStage::Child, s[1]),
                    (LifeStage::Teen, s[2]),
                    (LifeStage::Adult, s[0]),
                    (LifeStage::Senior, s[3]),
                ];
                (
                    order.iter().map(|(k, c)| (d.by_stage[k.index()] as f32, *c)).collect(),
                    format!(
                        "{} · {} children · {} seniors",
                        format::group(d.population as u64),
                        format::group(d.by_stage[LifeStage::Child.index()] as u64),
                        format::group(d.by_stage[LifeStage::Senior.index()] as u64)
                    ),
                )
            },
            group_top,
        ),
        C_HAPPY => people_daily(
            w,
            |d| {
                let h = d.avg_happiness;
                (
                    vec![(h * 100.0, mood_color(h))],
                    format!("{} · {}", (h * 100.0).round() as i64, people::mood_word(h)),
                )
            },
            |_| "100".into(),
        )
        .with_top_fixed(100.0),
        C_HOUSEHOLDS => people_daily(
            w,
            |d| {
                let kinds = citysim_sim::HouseholdKind::ALL;
                let cols = [s[1], s[2], s[0], s[3]];
                (
                    kinds
                        .iter()
                        .zip(cols)
                        .map(|(k, c)| (d.by_kind[k.index()] as f32, c))
                        .collect(),
                    format!("{} households", format::group(d.by_kind.iter().sum::<u32>() as u64)),
                )
            },
            group_top,
        ),
        C_HOURS => {
            let by = &w.people.trips_by_hour;
            let hour_now = (w.time.hour() as usize).min(23);
            let groups: [(&[TripPurpose], Color); 5] = [
                (&[TripPurpose::ToWork], s[0]),
                (&[TripPurpose::ToHome], s[1]),
                (&[TripPurpose::Shopping, TripPurpose::Leisure], s[2]),
                (&[TripPurpose::Goods], s[4]),
                (&[TripPurpose::MoveIn, TripPurpose::MoveOut], s[3]),
            ];
            let columns: Vec<Column> = (0..24)
                .map(|hr| {
                    let row = &by[hr];
                    let parts: Vec<(f32, Color)> = if hr <= hour_now {
                        groups
                            .iter()
                            .map(|(ps, c)| (ps.iter().map(|p| row[p.index()]).sum::<u32>() as f32, *c))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let total: f32 = parts.iter().map(|p| p.0).sum();
                    Column {
                        parts,
                        label: format!("{hr:02}:00"),
                        value: if hr <= hour_now {
                            format!("{} trips", format::group(total as u64))
                        } else {
                            "Still to come".into()
                        },
                    }
                })
                .collect();
            let max = columns
                .iter()
                .map(|c| c.parts.iter().map(|p| p.0).sum::<f32>())
                .fold(0.0, f32::max);
            ChartData {
                first: "00:00".into(),
                last: "23:00".into(),
                top: if max > 0.0 { group_top(max) } else { String::new() },
                empty: String::new(),
                live_last: false,
                pick: Some(hour_now),
                columns,
            }
        }
        C_FLOWS => {
            let h = &w.economy.history;
            let days = &h[h.len().saturating_sub(DAYS)..];
            let columns: Vec<Column> = days
                .iter()
                .map(|d| {
                    let b = &d.budget;
                    Column {
                        parts: vec![
                            (b.residential_tax as f32, t::ZONE_RESIDENTIAL),
                            (b.commercial_tax as f32, t::ZONE_COMMERCIAL),
                            (b.industrial_tax as f32, t::ZONE_INDUSTRIAL),
                            (-(b.road_upkeep as f32), s[4]),
                            (-(b.services as f32), s[3]),
                        ],
                        label: day_label(d.day),
                        value: format!(
                            "{} in · {} out · {} net",
                            format::money(d.income),
                            format::money(d.expenses),
                            format::signed_money(d.income - d.expenses)
                        ),
                    }
                })
                .collect();
            let max = columns
                .iter()
                .map(|c| c.parts.iter().map(|p| p.0.max(0.0)).sum::<f32>())
                .fold(0.0, f32::max);
            let (first, last) = axis(days.first().map(|d| d.day), days.len(), false);
            ChartData {
                first,
                last,
                top: if max > 0.0 {
                    format::money(max.round() as i64)
                } else {
                    String::new()
                },
                empty: NO_DAYS.into(),
                live_last: false,
                pick: None,
                columns,
            }
        }
        C_LOSS => {
            let columns: Vec<Column> = losses
                .iter()
                .map(|&(day, lost, per)| Column {
                    parts: vec![(lost as f32, t::WARNING)],
                    label: day_label(day),
                    value: format!(
                        "{} lost · {} a commute",
                        format::money(lost),
                        super::bar::commute_text(per)
                    ),
                })
                .collect();
            let max = columns.iter().map(|c| c.parts[0].0).fold(0.0, f32::max);
            ChartData {
                first: losses.first().map(|d| day_label(d.0)).unwrap_or_default(),
                last: losses.last().map(|d| day_label(d.0)).unwrap_or_default(),
                top: if max > 0.0 {
                    format::money(max.round() as i64)
                } else {
                    String::new()
                },
                empty: "Booked at each midnight".into(),
                live_last: false,
                pick: None,
                columns,
            }
        }
        _ => ChartData::default(),
    }
}

impl ChartFixed for ChartData {
    fn with_top_fixed(mut self, top: f32) -> Self {
        // An invisible part keeps the scale's top.
        if let Some(c) = self.columns.first_mut() {
            let v: f32 = c.parts.iter().map(|p| p.0).sum();
            if v < top {
                c.parts.push((top - v, Color::NONE));
            }
        }
        self
    }
}

trait ChartFixed {
    /// Keep the scale's top at `top` at least (a share out of 100).
    fn with_top_fixed(self, top: f32) -> Self;
}

/// Which charts each page shows.
fn charts_of(page: CityPage) -> &'static [u32] {
    match page {
        CityPage::Stats => &[C_POPULATION, C_JOBS, C_MONEY, C_COMMUTE, C_TRAFFIC, C_BUILDINGS],
        CityPage::People => &[C_STAGES, C_HAPPY, C_HOUSEHOLDS, C_HOURS],
        CityPage::Budget => &[C_FLOWS, C_LOSS],
    }
}

/// Refill the open page's charts when their data or the hovered column
/// changed (checked twice a second; hovering answers at once).
fn update_charts(
    time: Res<Time<Real>>,
    ui: Res<UiState>,
    sim: Res<SimWorld>,
    losses: Res<LossHistory>,
    mut nodes: ChartNodes,
    mut shown: Local<Vec<(u32, ChartData, Option<usize>)>>,
    mut age: Local<f32>,
) {
    let Some(page) = ui.window else {
        shown.clear();
        return;
    };
    *age += time.delta_secs();
    let ids = charts_of(page);
    let hovers: Vec<Option<usize>> = ids.iter().map(|&id| nodes.hovered(id)).collect();
    let hover_changed = ids
        .iter()
        .zip(&hovers)
        .any(|(id, h)| shown.iter().find(|s| s.0 == *id).is_none_or(|s| s.2 != *h));
    if *age < 0.5 && !hover_changed && !ui.is_changed() {
        return;
    }
    *age = 0.0;
    for (&id, &hover) in ids.iter().zip(&hovers) {
        let data = chart_data(id, &sim.world, &losses.days);
        let same = shown.iter().any(|s| s.0 == id && s.1 == data && s.2 == hover);
        if same {
            continue;
        }
        nodes.fill(id, &data, hover);
        shown.retain(|s| s.0 != id);
        shown.push((id, data, hover));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_cycle_both_ways() {
        assert_eq!(CityPage::Stats.next(1), CityPage::People);
        assert_eq!(CityPage::Budget.next(1), CityPage::Stats);
        assert_eq!(CityPage::Stats.next(-1), CityPage::Budget);
    }

    #[test]
    fn charts_read_a_new_city_without_history() {
        let w = World::new(1);
        for page in CityPage::ALL {
            for &id in charts_of(page) {
                let d = chart_data(id, &w, &[]);
                // No day booked yet: either the live column or the note.
                assert!(d.columns.len() <= DAYS + 1, "{id}");
                if d.columns.is_empty() {
                    assert!(!d.empty.is_empty(), "{id}");
                }
            }
        }
    }
}
