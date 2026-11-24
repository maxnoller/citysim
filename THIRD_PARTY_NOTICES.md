# Third-party notices

CitySim's own code is licensed MIT OR Apache-2.0 (see `Cargo.toml`). All 3D models, textures and colours are generated
in code; the only bundled third-party files are two fonts used by the interface. Their licence texts ship next to them
and must stay there.

| File | What | Licence | Licence text |
|---|---|---|---|
| `crates/game/assets/ui/fonts/InterVariable.ttf` | Inter (variable), interface text. Copyright (c) 2016 The Inter Project Authors, <https://github.com/rsms/inter> | SIL Open Font License 1.1 | `crates/game/assets/ui/fonts/Inter-LICENSE.txt` |
| `crates/game/assets/ui/fonts/lucide.ttf` | Lucide icon font, interface icons. Copyright (c) 2026 Lucide Icons and Contributors; icons derived from Feather Copyright (c) 2013-present Cole Bemis | ISC (Lucide), MIT (Feather-derived icons) | `crates/game/assets/ui/fonts/Lucide-LICENSE.txt` |

Rust dependencies (Bevy and its dependency tree, serde, slotmap, postcard) are fetched by Cargo and are licensed under
their own terms, mostly MIT and/or Apache-2.0; `cargo tree` lists them.
