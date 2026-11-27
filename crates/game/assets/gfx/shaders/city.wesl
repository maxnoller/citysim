// The one shared material for everything vertex-coloured: StandardMaterial
// lighting plus night glow (windows, street lamps, car lights, signs, signal
// lenses) selected by the vertex `uv.x` tag (`gfx::mesh_util::Glow`), and soft
// drifting cloud shadows on the sunlight.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_functions::{get_world_from_local, get_tag},
    mesh_view_bindings::{globals, view},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};

@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;

// Shared by every city material (a storage buffer, so the time of day can
// change without touching the materials; see `city_material.rs`).
struct CityParams {
    // 0 by day .. 1 at night: lamps and headlights.
    night: f32,
    // Share of windows lit right now (time of day).
    windows: f32,
    // Cloud shadow strength (0 = off).
    clouds: f32,
    unused: f32,
    // Cloud drift speed, m/s (x, y), cloud size, m (z), unused (w).
    wind: vec4<f32>,
    // People's colours: skin from 0, hair from 8, clothes on top from 16,
    // below from 32 (`city_material::PERSON_COLOURS_AT`).
    people: array<vec4<f32>>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<storage, read> city: CityParams;

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(123.34, 456.21));
    q = q + dot(q, q + 45.32);
    return fract(q.x * q.y);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// 1 in the open, lower under a cloud.
fn cloud_shadow(xz: vec2<f32>) -> f32 {
    if city.clouds <= 0.0 {
        return 1.0;
    }
    let p = (xz - city.wind.xy * globals.time) / city.wind.z;
    let n = vnoise(p) * 0.65 + vnoise(p * 2.3 + vec2<f32>(17.0, 3.0)) * 0.35;
    return 1.0 - city.clouds * smoothstep(0.52, 0.72, n);
}

// Grass detail, wildflowers near the camera and farmland beyond the map.
fn ground_color(base: vec3<f32>, wp: vec3<f32>) -> vec3<f32> {
    let p = wp.xz;
    let n1 = vnoise(p / 23.0);
    let n2 = vnoise(p / 6.5 + vec2<f32>(31.0, 7.0));
    var c = base * (0.96 + 0.16 * (n1 - 0.5) + 0.1 * (n2 - 0.5));
    // Drier, yellower patches.
    let dry = smoothstep(0.66, 0.86, vnoise(p / 57.0 + vec2<f32>(7.0, 3.0)));
    c = mix(c, c * vec3<f32>(1.1, 1.04, 0.78), dry * 0.4);
    // Farmland patchwork beyond the playable map (2048 m square).
    let out = max(max(-p.x, p.x - 2048.0), max(-p.y, p.y - 2048.0));
    let farm = smoothstep(70.0, 260.0, out);
    if farm > 0.0 {
        let size = vec2<f32>(170.0, 120.0);
        let q = p + vec2<f32>(vnoise(p / 90.0) * 30.0, vnoise(p / 90.0 + 11.0) * 30.0);
        let id = floor(q / size);
        let f = fract(q / size);
        let h = hash21(id + vec2<f32>(17.0, 41.0));
        var crop = c;
        if h < 0.22 {
            crop = vec3<f32>(0.62, 0.5, 0.2);
        } else if h < 0.4 {
            crop = vec3<f32>(0.3, 0.42, 0.12);
        } else if h < 0.55 {
            crop = vec3<f32>(0.3, 0.2, 0.11);
        } else if h < 0.68 {
            crop = vec3<f32>(0.46, 0.5, 0.16);
        }
        // Furrows across the field.
        let along = select(q.x, q.y, hash21(id + 3.0) < 0.5);
        crop = crop * (0.93 + 0.07 * step(0.5, fract(along / 2.6)));
        // Hedgerows on the field edges.
        let edge = min(min(f.x, 1.0 - f.x) * size.x, min(f.y, 1.0 - f.y) * size.y);
        crop = mix(vec3<f32>(0.08, 0.16, 0.05), crop, smoothstep(1.2, 2.4, edge));
        c = mix(c, crop, farm);
    }
    // Wildflowers: tiny specks, only near the camera.
    let dist = distance(view.world_position, wp);
    if dist < 150.0 && farm < 0.5 {
        let cell = floor(p / 1.1);
        let h = hash21(cell);
        let meadow = smoothstep(0.45, 0.7, vnoise(p / 31.0 + vec2<f32>(5.0, 9.0)));
        if h > 1.0 - 0.1 * meadow {
            let o = vec2<f32>(hash21(cell + 1.3), hash21(cell + 7.1)) * 0.5 + 0.25;
            let l = length(fract(p / 1.1) - o);
            if l < 0.2 {
                let k = fract(h * 113.0);
                var fl = vec3<f32>(0.9, 0.85, 0.75);
                if k < 0.3 {
                    fl = vec3<f32>(0.85, 0.7, 0.1);
                } else if k < 0.55 {
                    fl = vec3<f32>(0.55, 0.3, 0.75);
                } else if k < 0.7 {
                    fl = vec3<f32>(0.85, 0.25, 0.3);
                }
                c = mix(c, fl, 1.0 - smoothstep(80.0, 150.0, dist));
            }
        }
    }
    return c;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
@if(VISIBILITY_RANGE_DITHER)
    visibility_range_dither(in.position, in.visibility_range_dither);

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let wp = in.world_position.xyz;
    pbr_input.directional_shadow_factor = cloud_shadow(wp.xz);

    var glow = vec3<f32>(0.0);
@if(VERTEX_UVS_A) {
    let kind = u32(in.uv.x + 0.5);
    let base = pbr_input.material.base_color.rgb;
    if kind == 1u {
        // Window: lit share varies per building (hash of the model origin).
        let o = get_world_from_local(in.instance_index)[3].xyz;
        let hb = hash21(floor(o.xz * 0.37) + vec2<f32>(3.1, 7.7));
        let share = clamp(city.windows * (0.3 + 1.4 * hb), 0.0, 0.92);
        let r = fract(in.uv.y * 7.31 + hb * 3.17);
        if r < share {
            let warm = vec3<f32>(1.0, 0.66, 0.32);
            let cool = vec3<f32>(0.92, 0.86, 0.72);
            let tint = mix(warm, cool, step(0.72, fract(in.uv.y * 13.7)));
            let k = 0.7 + 0.5 * fract(in.uv.y * 29.3);
            // Brighter from afar so a town at night still sparkles when
            // zoomed out (windows are only a pixel or two there).
            let far = clamp(distance(view.world_position, wp) / 500.0, 0.0, 2.5);
            glow = tint * (2.6 * k * city.night * (1.0 + far));
            pbr_input.material.base_color = vec4<f32>(base * (1.0 - 0.7 * city.night), 1.0);
        }
    } else if kind == 2u {
        glow = vec3<f32>(1.0, 0.76, 0.42) * (9.0 * city.night);
    } else if kind == 3u {
        glow = vec3<f32>(1.0, 0.95, 0.82) * (7.0 * city.night);
    } else if kind == 4u {
        // Tail lights: bright when the car's `MeshTag` says it brakes.
        let brake = f32(get_tag(in.instance_index) & 1u);
        glow = vec3<f32>(1.0, 0.06, 0.03) * (2.2 * city.night + 6.0 * brake);
    } else if kind == 5u {
        glow = base * (2.2 * city.night);
    } else if kind == 6u {
        glow = base * 5.0;
    } else if kind == 8u {
        pbr_input.material.base_color = vec4<f32>(ground_color(base, wp), 1.0);
    } else if kind == 9u {
        // Distant building body (the far model, past the LOD switch): at
        // night the walls carry the average light of windows too small to
        // draw, plus scattered warm specks, so a big city does not turn into
        // dark blocks when zoomed out.
        let far = smoothstep(400.0, 900.0, distance(view.world_position, wp));
        let wall = 1.0 - abs(in.world_normal.y);
        let warm = vec3<f32>(1.0, 0.62, 0.3);
        glow = warm * (0.32 * city.windows * city.night * wall * (0.5 + 0.5 * far));
        let h = hash21(floor(wp.xz / 2.6) + vec2<f32>(floor(wp.y / 2.6) * 7.3, 1.7));
        if h < city.windows * (0.14 + 0.4 * wall) {
            glow += warm * (2.4 * city.night * (0.4 + 0.6 * far));
        }
    } else if kind >= 10u && kind <= 13u {
        // A person's colour slot, picked by the entity's tag
        // (`person_gen::person_tag`); the vertex colour shades it.
        let tag = get_tag(in.instance_index);
        var idx = 32u + ((tag >> 5u) & 7u);
        if kind == 10u {
            idx = ((tag >> 8u) & 7u) % 6u;
        } else if kind == 11u {
            idx = 8u + ((tag >> 11u) & 7u);
        } else if kind == 12u {
            idx = 16u + ((tag >> 1u) & 15u);
        }
        pbr_input.material.base_color = vec4<f32>(base * city.people[idx].rgb, 1.0);
        } else if kind == 7u {
        // Shop fronts: a warm interior glow, kept low so a whole glazed
        // ground floor does not bloom into a white panel.
        // Panes along the facade (whichever way it faces) with mullions
        // between them, each lit a little differently.
        let along = (wp.x + wp.z) * select(1.0, -1.0, abs(in.world_normal.x - in.world_normal.z) < 0.2) + wp.x - wp.z;
        let pane = along / 2.4;
        let mullion = smoothstep(0.03, 0.09, fract(pane)) * (1.0 - smoothstep(0.91, 0.97, fract(pane)));
        let h = hash21(vec2<f32>(floor(pane), floor(wp.y / 3.0) + 5.0));
        let lit = select(0.12, 0.35 + 0.65 * h, h > 0.22);
        glow = vec3<f32>(1.0, 0.72, 0.44) * (0.42 * city.night * lit * mullion);
        pbr_input.material.base_color = vec4<f32>(base * (1.0 - 0.6 * city.night), 1.0);
    }
}
    pbr_input.material.emissive = vec4<f32>(glow, 0.0);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
