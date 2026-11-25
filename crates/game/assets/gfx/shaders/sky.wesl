// Sky dome: a gradient from the horizon haze (= fog colour) to the zenith,
// a warm glow and disc towards the sun, and a moon and stars at night. All
// colours come from `gfx::daynight` as uniforms; the dome mesh only gives
// directions.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::view,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
};

struct SkyParams {
    horizon: vec4<f32>,
    mid: vec4<f32>,
    zenith: vec4<f32>,
    // Glow towards the sun (rgb), its strength (a).
    glow: vec4<f32>,
    // Unit direction towards the sun (xyz), sun disc brightness (w).
    sun: vec4<f32>,
    // Unit direction towards the moon (xyz), moon brightness (w).
    moon: vec4<f32>,
    // Stars brightness (x), unused (yzw).
    stars: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<uniform> sky: SkyParams;

fn hash31(p: vec3<f32>) -> f32 {
    var q = fract(p * 0.1031);
    q = q + dot(q, q.zyx + 31.32);
    return fract((q.x + q.y) * q.z);
}

fn stars(d: vec3<f32>) -> f32 {
    let cells = 260.0;
    let g = d * cells;
    let i = floor(g);
    let h = hash31(i);
    if h < 0.985 {
        return 0.0;
    }
    let c = i + vec3<f32>(hash31(i + 1.7), hash31(i + 5.3), hash31(i + 9.1)) * 0.6 + 0.2;
    let dist = length(g - c);
    let size = 0.10 + 0.12 * fract(h * 97.0);
    return smoothstep(size, 0.0, dist) * (0.4 + 0.6 * fract(h * 311.0));
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let d = normalize(in.world_position.xyz - view.world_position);
    let h = max(d.y, 0.0);
    let low = pow(clamp(h / 0.22, 0.0, 1.0), 0.55);
    let high = clamp((h - 0.22) / 0.78, 0.0, 1.0);
    var c = mix(sky.horizon.rgb, sky.mid.rgb, low);
    if h >= 0.22 {
        c = mix(sky.mid.rgb, sky.zenith.rgb, high);
    }
    // Warm glow on the horizon towards the sun, and a halo around it.
    let sd = dot(d, sky.sun.xyz);
    let toward = max(dot(normalize(vec2<f32>(d.x, d.z) + vec2<f32>(1e-5, 0.0)),
        normalize(vec2<f32>(sky.sun.x, sky.sun.z) + vec2<f32>(1e-5, 0.0))), 0.0);
    let glow = pow(toward, 4.0) * (1.0 - low * 0.85) * sky.glow.a + pow(max(sd, 0.0), 24.0) * 0.5 * sky.glow.a;
    c = mix(c, sky.glow.rgb, clamp(glow, 0.0, 1.0));
    // Sun disc.
    c = c + vec3<f32>(1.0, 0.92, 0.75) * smoothstep(0.9993, 0.9997, sd) * sky.sun.w;
    // Moon: disc and soft halo.
    let md = dot(d, sky.moon.xyz);
    c = c + vec3<f32>(0.85, 0.9, 1.0) * smoothstep(0.99955, 0.99975, md) * sky.moon.w * 3.0;
    c = c + vec3<f32>(0.35, 0.45, 0.7) * pow(max(md, 0.0), 300.0) * sky.moon.w * 0.5;
    // Stars, fading towards the horizon haze.
    if sky.stars.x > 0.0 {
        c = c + vec3<f32>(0.9, 0.93, 1.0) * stars(d) * sky.stars.x * smoothstep(0.04, 0.3, d.y);
    }
    pbr_input.material.base_color = vec4<f32>(c, 1.0);
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4<f32>(c, 1.0));
    return out;
}
