<img src="assets/icon.svg" width="64" height="64" alt="">

# CitySim

A small, original 3D city-builder written in Rust with [Bevy](https://bevy.org) 0.20. Start on an empty map with a
highway stub, draw roads, paint residential, commercial and industrial zones beside them, and watch a town grow by
demand. Every household has a home and a job and drives to work on the shortest route; streets slow down as they fill,
so a bad road layout shows up as real traffic jams that you fix by rebuilding roads. Taxes, road upkeep and city
services drive the budget.

![A town of about 5,000 residents](docs/screenshot.png)

## Features

- Straight and 45° roads drawn from the highway stub, with live preview, price and the reason when a road is not
  possible; bulldozer for roads and buildings.
- Residential, commercial and industrial zoning within two cells of a road; buildings grow by demand and level up from
  small houses, shops and sheds to apartment blocks and offices.
- Agent traffic: commuters, shopping trips and delivery trucks drive in real time through junctions that work like
  simple traffic lights; a traffic overlay shows each road direction green / amber / red.
- Budget by category (taxes per zone type, road upkeep, city services), a daily history chart and a tax slider.
- Pause and three speeds (1x, 3x, 6x; a day lasts 4 minutes at 1x), one save slot, graphics quality presets.

Out of scope for this version: terrain height, water, curved roads, transit, pedestrians, utilities, land value,
pollution and audio.

## Build and run

Needs a recent stable Rust toolchain (edition 2024). On Linux, Bevy needs the usual system packages
(`libasound2-dev libudev-dev libxkbcommon-x11-0` or your distribution's equivalents) and a Vulkan- or GL-capable driver
(software rendering works, slowly).

```sh
cargo run -p citysim --release      # play (the first build takes about 10 minutes)
cargo test --workspace              # simulation and game tests
```

The save slot is `~/.local/share/citysim/city.citysave` on Linux (`~/Library/Application Support/CitySim` on macOS,
`%APPDATA%\CitySim` on Windows); `CITYSIM_SAVE_DIR` overrides it.

## Controls

| Input | Action |
|---|---|
| Left click / drag | Use the current tool (roads: click start and end, or drag; zones and bulldozer: drag a rectangle) |
| Right click, Esc | Cancel the current action; Esc again returns to Inspect, then opens the pause menu |
| I | Inspect (hover buildings and roads for live numbers) |
| R | Road tool |
| Z | Zone tool (press again to cycle residential / commercial / industrial / de-zone) |
| B | Bulldozer |
| T | Traffic overlay · O cycles overlays (zones, traffic) |
| Space · 1 2 3 | Pause · speed 1x / 3x / 6x |
| WASD or arrows · Q / E | Pan · rotate (Shift pans faster) |
| Mouse wheel · middle drag | Zoom · rotate and tilt |
| G | Graphics quality (Low / Medium / High) |
| F5 · F9 | Save · load |

## Project layout

- `crates/sim` — the simulation: all state and rules, pure Rust (no Bevy), deterministic, driven by `Command`s and a
  fixed-step `World::step()`.
- `crates/game` — the Bevy front-end: `gfx/` (3D drawing and camera), `ui/` (interface, tools, input, save/load),
  `shared.rs` (the only contract between the two), `dev/` (development scenarios, screenshots, scripted play).
- `scripts/play.py` — scripted play-through driving the real game with OS input (development).

See `CLAUDE.md` for developer notes and dev flags, and `THIRD_PARTY_NOTICES.md` for bundled font licences.

## Development shortcuts

```sh
CITYSIM_DEMO=city cargo run -p citysim --release    # start in a ~5,000-resident town (also: town, jam)
CITYSIM_DEMO=town CITYSIM_AUTOSHOT=/tmp/t.png cargo run -p citysim --release   # screenshot and exit
cargo run -p citysim_sim --release --example headless   # balance runs of scripted cities, no window
```
