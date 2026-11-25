//! Settings card (from the pause menu): graphics quality, interface scale,
//! edge scrolling, lot markers for zoned-but-empty land, and a key
//! reference. Display settings gfx honours live in `shared::DisplaySettings`.
//!
//! The choices persist between sessions in `settings.cfg` in the platform
//! config directory (`$XDG_CONFIG_HOME/citysim`, default `~/.config/citysim`
//! on Linux, `~/Library/Application Support/CitySim` on macOS,
//! `%APPDATA%\CitySim` on Windows; `CITYSIM_CONFIG_DIR` overrides it).

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;

use super::UiState;
use super::budget::close_button;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, UiAction, UiButton, approach, label, layout, panel, spawn_button, spawn_key_badge,
};
use crate::shared::{DisplaySettings, GameSet, GraphicsQuality};

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
        format!(
            "# CitySim settings\nquality={q}\ninterface_size={}\nedge_scroll={}\nlot_markers={}\n",
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
                "lot_markers" => self.lot_markers = v.parse().unwrap_or(self.lot_markers),
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

#[derive(Component)]
struct SettingsRoot;

#[derive(Component)]
struct SettingsCard;

/// Key reference, grouped.
const KEYS: [(&str, &[(&str, &str)]); 3] = [
    (
        "CAMERA",
        &[
            ("WASD", "Pan (Shift: faster)"),
            ("Q  E", "Rotate"),
            ("Wheel", "Zoom"),
            ("Middle drag", "Orbit and tilt"),
        ],
    ),
    (
        "BUILDING",
        &[
            ("I", "Inspect (click pins the card)"),
            ("R", "Road · again: street / avenue"),
            ("J", "Junctions: lights or priority"),
            ("Z", "Zone · again: next zone type"),
            ("B", "Bulldoze"),
            ("Tab", "Next mode of the tool"),
            ("Enter", "Build the path (or double-click)"),
            ("Backspace", "Remove the last corner"),
            ("Alt", "Hold: place without snapping"),
            ("[  ]", "Block size (block tool)"),
            ("Ctrl+Z", "Undo the last road or zoning"),
            ("Esc", "Cancel · back to Inspect · menu"),
        ],
    ),
    (
        "GAME",
        &[
            ("Space", "Pause"),
            ("1  2  3", "Speed"),
            ("T  O", "Traffic overlay · cycle overlays"),
            ("G", "Graphics quality"),
            ("F5  F9", "Save · load"),
        ],
    ),
];

fn heading(p: &mut ChildSpawnerCommands, fonts: &UiFonts, glyph: char, text: &str, key: Option<&str>) {
    p.spawn(layout(Node {
        align_items: AlignItems::Center,
        column_gap: px(t::S3 + 2.0),
        ..default()
    }))
    .with_children(|r| {
        r.spawn(super::widgets::icon(fonts, glyph, t::ICON_SM + 1.0, t::TEXT_MUTED));
        r.spawn((
            label(fonts, text, t::FONT_BODY, t::W_MEDIUM, t::TEXT),
            Node {
                flex_grow: 1.0,
                ..default()
            },
        ));
        if let Some(k) = key {
            spawn_key_badge(r, fonts, k);
        }
    });
}

fn segmented(p: &mut ChildSpawnerCommands, fonts: &UiFonts, items: &[(UiAction, &'static str)]) {
    p.spawn(layout(Node {
        padding: UiRect::all(px(3)),
        column_gap: px(3),
        margin: UiRect::bottom(px(t::S4)),
        border_radius: BorderRadius::all(px(t::RADIUS_SM + 1.0)),
        ..default()
    }))
    .insert(BackgroundColor(t::WELL))
    .with_children(|seg| {
        for &(action, name) in items {
            spawn_button(
                seg,
                UiButton {
                    action,
                    kind: ButtonKind::Segment,
                    accent: t::ACCENT,
                },
                Node {
                    flex_grow: 1.0,
                    flex_basis: px(0),
                    height: px(28),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                None,
                |b, id| {
                    b.spawn((
                        label(fonts, name, t::FONT_SMALL + 0.5, t::W_MEDIUM, t::TEXT_MUTED),
                        ButtonFg {
                            owner: id,
                            base: t::TEXT_MUTED,
                        },
                    ));
                },
            );
        }
    });
}

fn spawn_settings(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Settings"),
            SettingsRoot,
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
            // Above toasts (300): the settings are modal.
            GlobalZIndex(320),
        ))
        .with_children(|root| {
            root.spawn((
                SettingsCard,
                panel(Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(px(t::S5 + 4.0), px(t::S5 + 4.0), px(t::S4 + 2.0), px(t::S5 + 2.0)),
                    ..default()
                }),
                UiTransform::default(),
            ))
            .insert(BackgroundColor(t::PANEL_RAISED))
            .with_children(|c| {
                c.spawn(layout(Node {
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    margin: UiRect::bottom(px(t::S4)),
                    ..default()
                }))
                .with_children(|h| {
                    h.spawn(label(fonts, "Settings", 22.0, t::W_BOLD, t::TEXT));
                    close_button(h, fonts, UiAction::CloseSettings);
                });
                c.spawn(layout(Node {
                    column_gap: px(t::S5 + 12.0),
                    ..default()
                }))
                .with_children(|cols| {
                    // Left: settings.
                    cols.spawn(layout(Node {
                        flex_direction: FlexDirection::Column,
                        width: px(268),
                        row_gap: px(t::S3),
                        ..default()
                    }))
                    .with_children(|l| {
                        heading(l, fonts, icon::MONITOR, "Graphics quality", Some("G"));
                        segmented(
                            l,
                            fonts,
                            &[
                                (UiAction::Quality(GraphicsQuality::Low), "Low"),
                                (UiAction::Quality(GraphicsQuality::Medium), "Medium"),
                                (UiAction::Quality(GraphicsQuality::High), "High"),
                            ],
                        );
                        heading(l, fonts, icon::SCALING, "Interface size", None);
                        let scales: Vec<(UiAction, &'static str)> = UI_SCALES
                            .iter()
                            .enumerate()
                            .map(|(i, &(_, n))| (UiAction::UiScale(i), n))
                            .collect();
                        segmented(l, fonts, &scales);
                        heading(l, fonts, icon::MOVE, "Edge scrolling", None);
                        segmented(
                            l,
                            fonts,
                            &[(UiAction::EdgeScroll(true), "On"), (UiAction::EdgeScroll(false), "Off")],
                        );
                        heading(l, fonts, icon::ZONE, "Mark empty zoned lots", None);
                        segmented(
                            l,
                            fonts,
                            &[
                                (UiAction::LotMarkers(true), "Always"),
                                (UiAction::LotMarkers(false), "While zoning"),
                            ],
                        );
                        l.spawn(layout(Node {
                            flex_grow: 1.0,
                            ..default()
                        }));
                        spawn_button(
                            l,
                            UiButton {
                                action: UiAction::CloseSettings,
                                kind: ButtonKind::Menu,
                                accent: t::ACCENT,
                            },
                            Node {
                                height: px(38),
                                padding: UiRect::axes(px(t::S3), px(0)),
                                align_items: AlignItems::Center,
                                column_gap: px(t::S3 + 2.0),
                                ..default()
                            },
                            None,
                            |b, id| {
                                b.spawn((
                                    super::widgets::icon(fonts, icon::CHEVRON_LEFT, t::ICON_MD, t::TEXT_MUTED),
                                    ButtonFg {
                                        owner: id,
                                        base: t::TEXT_MUTED,
                                    },
                                ));
                                b.spawn((
                                    label(fonts, "Back to the menu", t::FONT_LABEL, t::W_MEDIUM, t::TEXT),
                                    ButtonFg {
                                        owner: id,
                                        base: t::TEXT,
                                    },
                                    Node {
                                        flex_grow: 1.0,
                                        ..default()
                                    },
                                ));
                                spawn_key_badge(b, fonts, "Esc");
                            },
                        );
                    });
                    // Right: keys.
                    cols.spawn(layout(Node {
                        flex_direction: FlexDirection::Column,
                        width: px(318),
                        row_gap: px(5),
                        ..default()
                    }))
                    .with_children(|r| {
                        for (gi, (group, keys)) in KEYS.iter().enumerate() {
                            r.spawn((
                                label(fonts, *group, t::FONT_CAPTION + 0.5, t::W_SEMIBOLD, t::TEXT_FAINT),
                                Node {
                                    margin: UiRect::new(px(0), px(0), px(if gi == 0 { 0.0 } else { t::S3 }), px(1)),
                                    ..default()
                                },
                            ));
                            for (k, what) in keys.iter() {
                                r.spawn(layout(Node {
                                    align_items: AlignItems::Center,
                                    column_gap: px(t::S3),
                                    ..default()
                                }))
                                .with_children(|row| {
                                    row.spawn(layout(Node {
                                        width: px(92),
                                        flex_shrink: 0.0,
                                        ..default()
                                    }))
                                    .with_children(|kc| spawn_key_badge(kc, fonts, k));
                                    row.spawn(label(fonts, *what, t::FONT_SMALL, t::W_REGULAR, t::TEXT_MUTED));
                                });
                            }
                        }
                    });
                });
            });
        });
}

fn show_settings(
    time: Res<Time<Real>>,
    state: Res<UiState>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut root: Query<&mut Node, With<SettingsRoot>>,
    mut card: Query<(&mut UiTransform, &ComputedNode), With<SettingsCard>>,
    mut anim: Local<f32>,
) {
    let Ok(mut node) = root.single_mut() else { return };
    let open = state.menu_open && state.settings_open;
    let d = if open { Display::Flex } else { Display::None };
    if node.display != d {
        node.display = d;
        *anim = 0.0;
    }
    if !open {
        return;
    }
    if *anim < 1.0 {
        *anim += (1.0 - *anim) * approach(time.delta_secs(), 0.07);
        if *anim > 0.995 {
            *anim = 1.0;
        }
    }
    if let Ok((mut tf, computed)) = card.single_mut() {
        // Shrink to fit a small window at a large interface size.
        let fit = match window.single() {
            Ok(w) if computed.size.y > 0.0 => (w.physical_height() as f32 * 0.96 / computed.size.y)
                .min(w.physical_width() as f32 * 0.96 / computed.size.x)
                .min(1.0),
            _ => 1.0,
        };
        let scale = Vec2::splat((0.96 + 0.04 * *anim) * fit);
        if tf.scale != scale {
            tf.scale = scale;
        }
        let shift = Val2::px(0.0, (1.0 - *anim) * 8.0);
        if tf.translation != shift {
            tf.translation = shift;
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
        };
        let base = Saved {
            quality: GraphicsQuality::High,
            ui_scale: 1,
            edge_scroll: false,
            lot_markers: true,
        };
        assert_eq!(base.read(&s.to_text()), s);
        assert_eq!(
            base.read("quality=ultra\ninterface_size=9\nedge_scroll=maybe\nnonsense"),
            base
        );
    }
}
