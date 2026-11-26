//! Settings plate (from the pause menu): graphics quality, interface
//! scale, edge scrolling, lot markers for zoned-but-empty land, the
//! performance panel; and the key reference plate. Display settings gfx honours live in `shared::DisplaySettings`.
//!
//! The choices persist between sessions in `settings.cfg` in the platform
//! config directory (`$XDG_CONFIG_HOME/citysim`, default `~/.config/citysim`
//! on Linux, `~/Library/Application Support/CitySim` on macOS,
//! `%APPDATA%\CitySim` on Windows; `CITYSIM_CONFIG_DIR` overrides it).

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;
use bevy::ui::ComputedNode;

use super::UiState;
use super::menu::{MenuIndex, menu_row, modal_header};
use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonKind, UiAction, UiButton, approach, caps, label, layout, plate_node, plate_shadow, spawn_button, spawn_key,
};
use crate::shared::{DisplaySettings, GameSet, GraphicsQuality, PerfDetail};

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (load_settings, spawn_settings))
            .add_systems(Update, (show_settings, save_settings).in_set(GameSet::Render));
    }
}

// ---------------------------------------------------------------- persistence

/// What is remembered between sessions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Saved {
    pub quality: GraphicsQuality,
    pub ui_scale: usize,
    pub edge_scroll: bool,
    pub lot_markers: bool,
    pub perf: PerfDetail,
}

/// Directory of the settings file.
pub fn config_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Some(dir) = std::env::var_os("CITYSIM_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "windows") {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("CitySim");
        }
    } else if cfg!(target_os = "macos") {
        if let Some(h) = home() {
            return h.join("Library/Application Support/CitySim");
        }
    } else if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(xdg).join("citysim");
    } else if let Some(h) = home() {
        return h.join(".config/citysim");
    }
    PathBuf::from(".")
}

const SETTINGS_FILE: &str = "settings.cfg";

impl Saved {
    pub fn to_text(self) -> String {
        let q = match self.quality {
            GraphicsQuality::Low => "low",
            GraphicsQuality::Medium => "medium",
            GraphicsQuality::High => "high",
        };
        let perf = match self.perf {
            PerfDetail::Off => "off",
            PerfDetail::Small => "small",
            PerfDetail::Full => "full",
        };
        format!(
            "# CitySim settings\nquality={q}\ninterface_size={}\nedge_scroll={}\nlot_markers_always={}\nperformance_panel={perf}\n",
            self.ui_scale, self.edge_scroll, self.lot_markers
        )
    }

    /// Read `key=value` lines over `self`; unknown or broken lines are
    /// ignored.
    pub fn read(mut self, text: &str) -> Self {
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "quality" => {
                    self.quality = match v {
                        "low" => GraphicsQuality::Low,
                        "medium" => GraphicsQuality::Medium,
                        "high" => GraphicsQuality::High,
                        _ => self.quality,
                    }
                }
                "interface_size" => {
                    if let Ok(i) = v.parse::<usize>()
                        && i < UI_SCALES.len()
                    {
                        self.ui_scale = i;
                    }
                }
                "edge_scroll" => self.edge_scroll = v.parse().unwrap_or(self.edge_scroll),
                "lot_markers_always" => self.lot_markers = v.parse().unwrap_or(self.lot_markers),
                "performance_panel" => {
                    self.perf = match v {
                        "off" => PerfDetail::Off,
                        "small" => PerfDetail::Small,
                        "full" => PerfDetail::Full,
                        _ => self.perf,
                    }
                }
                _ => {}
            }
        }
        self
    }
}

/// Dev captures drive the settings themselves: never write the player's file.
fn persistence_enabled() -> bool {
    ["CITYSIM_UI_SHOTS", "CITYSIM_UI_TOUR", "CITYSIM_AUTOSHOT"]
        .iter()
        .all(|k| std::env::var_os(k).is_none_or(|v| v.is_empty()))
}

fn current(quality: GraphicsQuality, state: &UiState, display: &DisplaySettings) -> Saved {
    Saved {
        quality,
        ui_scale: state.ui_scale,
        edge_scroll: display.edge_scroll,
        lot_markers: display.lot_markers,
        perf: state.perf,
    }
}

fn load_settings(
    mut quality: ResMut<GraphicsQuality>,
    mut state: ResMut<UiState>,
    mut display: ResMut<DisplaySettings>,
) {
    if !persistence_enabled() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(config_dir().join(SETTINGS_FILE)) else {
        return;
    };
    let s = current(*quality, &state, &display).read(&text);
    // A quality forced for development wins over the saved one.
    if std::env::var_os("CITYSIM_GFX_QUALITY").is_none() {
        *quality = s.quality;
    }
    state.ui_scale = s.ui_scale;
    display.edge_scroll = s.edge_scroll;
    display.lot_markers = s.lot_markers;
    state.perf = s.perf;
    info!("settings loaded from {}", config_dir().display());
}

/// Write the file whenever a remembered choice changes (not at startup).
fn save_settings(
    quality: Res<GraphicsQuality>,
    state: Res<UiState>,
    display: Res<DisplaySettings>,
    mut last: Local<Option<Saved>>,
) {
    let now = current(*quality, &state, &display);
    match *last {
        None => *last = Some(now),
        Some(prev) if prev == now => {}
        Some(_) => {
            *last = Some(now);
            if !persistence_enabled() {
                return;
            }
            let dir = config_dir();
            let res =
                std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(SETTINGS_FILE), now.to_text()));
            if let Err(e) = res {
                warn!("cannot save settings to {}: {e}", dir.display());
            }
        }
    }
}

/// Interface scale steps offered (multiplied with the automatic scale).
pub const UI_SCALES: [(f32, &str); 4] = [(0.85, "85%"), (1.0, "100%"), (1.15, "115%"), (1.3, "130%")];

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Modal {
    Settings,
    Keys,
}

#[derive(Component)]
struct ModalCard;

/// Key reference, grouped.
const KEYS: [(&str, &[(&str, &str)]); 3] = [
    (
        "Camera",
        &[
            ("WASD", "Pan (Shift: faster)"),
            ("Q  E", "Rotate"),
            ("Wheel", "Zoom"),
            ("Middle drag", "Orbit and tilt"),
            ("F", "Follow the person or car shown"),
        ],
    ),
    (
        "Building",
        &[
            ("I", "Inspect · click homes, shops, cars"),
            ("R", "Roads · again: street / avenue"),
            ("J", "Junctions: lights or priority"),
            ("Z", "Zones · again: next zone type"),
            ("B", "Bulldozer on / off · back to the tool"),
            ("Tab", "Next mode of the tool · window page"),
            ("Enter", "Build the road (or double-click)"),
            ("Backspace", "Remove the last point"),
            ("Alt", "Hold: place without snapping"),
            ("[  ]", "Grid block size"),
            ("Ctrl+Z", "Undo the last road, zoning or demolition"),
            ("Esc", "Step back · Inspect · menu"),
        ],
    ),
    (
        "Game",
        &[
            ("Space", "Pause"),
            ("1  2  3  4", "Speed"),
            ("P", "City window: people"),
            ("U", "City window: budget"),
            ("O", "Maps · cycle"),
            ("T", "Traffic map"),
            ("G", "Graphics quality"),
            ("F3", "Performance panel · small, full, off"),
            ("F6", "Keyboard into window / drawer"),
            ("F5  F9", "Save · load"),
        ],
    ),
];

/// A setting: caps caption above a row of joined segments.
fn setting(p: &mut ChildSpawnerCommands, fonts: &UiFonts, glyph: char, text: &str, items: &[(UiAction, &'static str)]) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(7),
        margin: UiRect::bottom(px(14)),
        ..default()
    }))
    .with_children(|c| {
        c.spawn(layout(Node {
            align_items: AlignItems::Center,
            column_gap: px(8),
            ..default()
        }))
        .with_children(|r| {
            r.spawn(super::widgets::icon(fonts, glyph, 14.0, t::TEXT_MUTED));
            r.spawn(caps(fonts, text, t::FONT_CAP, t::TEXT_MUTED));
        });
        c.spawn(layout(Node::default())).with_children(|seg| {
            for (i, &(action, name)) in items.iter().enumerate() {
                spawn_button(
                    seg,
                    UiButton::new(action, ButtonKind::Segment),
                    Node {
                        flex_grow: 1.0,
                        flex_basis: px(0),
                        height: px(34),
                        margin: UiRect::left(px(if i == 0 { 0.0 } else { -1.0 })),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    None,
                    |b, id| super::widgets::button_face(b, fonts, id, None, name),
                );
            }
        });
    });
}

fn modal_root(which: Modal, name: &'static str) -> impl Bundle {
    (
        Name::new(name),
        which,
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(0),
            top: px(0),
            bottom: px(0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            display: Display::None,
            ..default()
        },
        BackgroundColor(t::BACKDROP),
        // Above toasts (300): modal.
        GlobalZIndex(320),
    )
}

fn spawn_settings(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
) {
    let fonts = fonts.into_inner();
    let mat = cache.get(&mut mats, PlateStyle::PANEL);
    commands
        .spawn(modal_root(Modal::Settings, "Settings"))
        .with_children(|root| {
            root.spawn((
                ModalCard,
                plate_node(
                    Node {
                        width: px(420),
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::bottom(px(12)),
                        ..default()
                    },
                    UiRect::all(px(t::LW)),
                    [t::CUT, 0.0, t::CUT, 0.0],
                ),
                mat.clone(),
                plate_shadow(),
                UiTransform::default(),
            ))
            .with_children(|c| {
                modal_header(c, fonts, "Settings");
                c.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(px(26), px(26), px(18), px(4)),
                    ..default()
                }))
                .with_children(|l| {
                    setting(
                        l,
                        fonts,
                        icon::MONITOR,
                        "Graphics quality · G",
                        &[
                            (UiAction::Quality(GraphicsQuality::Low), "Low"),
                            (UiAction::Quality(GraphicsQuality::Medium), "Medium"),
                            (UiAction::Quality(GraphicsQuality::High), "High"),
                        ],
                    );
                    let scales: Vec<(UiAction, &'static str)> = UI_SCALES
                        .iter()
                        .enumerate()
                        .map(|(i, &(_, n))| (UiAction::UiScale(i), n))
                        .collect();
                    setting(l, fonts, icon::SCALING, "Interface size", &scales);
                    setting(
                        l,
                        fonts,
                        icon::MOVE,
                        "Edge scrolling",
                        &[(UiAction::EdgeScroll(true), "On"), (UiAction::EdgeScroll(false), "Off")],
                    );
                    setting(
                        l,
                        fonts,
                        icon::ZONE,
                        "Mark empty zoned lots",
                        &[
                            (UiAction::LotMarkers(false), "While zoning"),
                            (UiAction::LotMarkers(true), "Always"),
                        ],
                    );
                    setting(
                        l,
                        fonts,
                        icon::GAUGE,
                        "Performance panel · F3",
                        &[
                            (UiAction::Perf(PerfDetail::Off), "Off"),
                            (UiAction::Perf(PerfDetail::Small), "Small"),
                            (UiAction::Perf(PerfDetail::Full), "Full"),
                        ],
                    );
                });
                menu_row(
                    c,
                    fonts,
                    UiAction::CloseSettings,
                    icon::CHEVRON_LEFT,
                    "Back to the menu",
                    Some("Esc"),
                    ButtonKind::Row,
                    0,
                );
            });
        });
    commands.spawn(modal_root(Modal::Keys, "Keys")).with_children(|root| {
        root.spawn((
            ModalCard,
            plate_node(
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::bottom(px(12)),
                    ..default()
                },
                UiRect::all(px(t::LW)),
                [t::CUT, 0.0, t::CUT, 0.0],
            ),
            mat,
            plate_shadow(),
            UiTransform::default(),
        ))
        .with_children(|c| {
            modal_header(c, fonts, "Keys");
            c.spawn(layout(Node {
                column_gap: px(34),
                padding: UiRect::new(px(26), px(26), px(16), px(8)),
                ..default()
            }))
            .with_children(|cols| {
                for (group, keys) in KEYS.iter() {
                    cols.spawn(layout(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: px(6),
                        ..default()
                    }))
                    .with_children(|r| {
                        r.spawn(layout(Node {
                            align_items: AlignItems::Center,
                            column_gap: px(8),
                            margin: UiRect::bottom(px(4)),
                            ..default()
                        }))
                        .with_children(|h| {
                            h.spawn((
                                layout(Node {
                                    width: px(3),
                                    height: px(11),
                                    ..default()
                                }),
                                BackgroundColor(t::ACCENT),
                            ));
                            h.spawn(caps(fonts, *group, t::FONT_CAP, t::TEXT));
                        });
                        for (k, what) in keys.iter() {
                            r.spawn(layout(Node {
                                align_items: AlignItems::Center,
                                column_gap: px(10),
                                ..default()
                            }))
                            .with_children(|row| {
                                row.spawn(layout(Node {
                                    width: px(84),
                                    flex_shrink: 0.0,
                                    ..default()
                                }))
                                .with_children(|kc| {
                                    spawn_key(kc, fonts, k, t::SLOT_INK);
                                });
                                row.spawn(label(fonts, *what, t::FONT_BODY, t::W_MEDIUM, t::TEXT_MUTED));
                            });
                        }
                    });
                }
            });
            menu_row(
                c,
                fonts,
                UiAction::Keys,
                icon::CHEVRON_LEFT,
                "Back to the menu",
                Some("Esc"),
                ButtonKind::Row,
                0,
            );
        });
    });
    let _ = MenuIndex(0);
}

#[allow(clippy::type_complexity)]
fn show_settings(
    time: Res<Time<Real>>,
    state: Res<UiState>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut roots: Query<(&Modal, &mut Node, &Children)>,
    mut cards: Query<(&mut UiTransform, &ComputedNode), With<ModalCard>>,
    mut anim: Local<f32>,
) {
    for (which, mut node, children) in &mut roots {
        let open = state.menu_open
            && match which {
                Modal::Settings => state.settings_open,
                Modal::Keys => state.keys_open,
            };
        let d = if open { Display::Flex } else { Display::None };
        if node.display != d {
            node.display = d;
            if open {
                *anim = 0.0;
            }
        }
        if !open {
            continue;
        }
        if *anim < 1.0 {
            *anim += (1.0 - *anim) * approach(time.delta_secs(), 0.07);
            if *anim > 0.995 {
                *anim = 1.0;
            }
        }
        for c in children.iter() {
            let Ok((mut tf, computed)) = cards.get_mut(c) else {
                continue;
            };
            // Shrink to fit a small window at a large interface size.
            let fit = match window.single() {
                Ok(w) if computed.size.y > 0.0 => (w.physical_height() as f32 * 0.96 / computed.size.y)
                    .min(w.physical_width() as f32 * 0.96 / computed.size.x)
                    .min(1.0),
                _ => 1.0,
            };
            let scale = Vec2::splat(fit);
            if tf.scale != scale {
                tf.scale = scale;
            }
            let shift = Val2::px(0.0, (1.0 - *anim) * 14.0);
            if tf.translation != shift {
                tf.translation = shift;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_ignore_junk() {
        let s = Saved {
            quality: GraphicsQuality::Medium,
            ui_scale: 2,
            edge_scroll: true,
            lot_markers: false,
            perf: PerfDetail::Full,
        };
        let base = Saved {
            quality: GraphicsQuality::High,
            ui_scale: 1,
            edge_scroll: false,
            lot_markers: true,
            perf: PerfDetail::Off,
        };
        assert_eq!(base.read(&s.to_text()), s);
        assert_eq!(
            base.read("quality=ultra\ninterface_size=9\nedge_scroll=maybe\nperformance_panel=huge\nnonsense"),
            base
        );
    }
}
