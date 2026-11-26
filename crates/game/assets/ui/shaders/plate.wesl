// An angular interface plate: a convex shape with chamfered corners and/or
// slanted sides, a vertical fill gradient, an edge line, optional hatching.
//
// Everything is measured in physical pixels so the shape is crisp at every
// scale: the node's border widths are the edge line, its border radii
// (top-left, top-right, bottom-right, bottom-left) are the corner cuts. Slants are insets as a fraction of the height.

import bevy_ui_render::ui_vertex_output::UiVertexOutput;

struct Plate {
    fill_top: vec4<f32>,
    fill_bottom: vec4<f32>,
    line: vec4<f32>,
    // Left inset at the top, left inset at the bottom, right inset at the
    // top, right inset at the bottom (fractions of the height).
    slant: vec4<f32>,
    // x: hatch strength, y: hatch period (fraction of the height),
    // z: top-edge highlight strength, w: unused. Hatching fades every
    // other diagonal stripe of the fill.
    extra: vec4<f32>,
}

@group(1) @binding(0)
var<uniform> plate: Plate;

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let size = in.size;
    let p = in.uv * size;
    let h = max(size.y, 1.0);
    // Bevy 0.20 passes the border widths in physical pixels, ordered left,
    // top, right, bottom (despite its doc comment); reorder to l, r, t, b.
    let bw = vec4<f32>(in.border_widths.x, in.border_widths.z, in.border_widths.y, in.border_widths.w);
    let cut = in.border_radius_x;

    // Slanted sides: x of the edge at this height, perpendicular distance.
    let ls = plate.slant.y - plate.slant.x;
    let rs = plate.slant.w - plate.slant.z;
    let lx = (plate.slant.x + ls * (p.y / h)) * h;
    let rx = size.x - (plate.slant.z + rs * (p.y / h)) * h;
    let d_left = (lx - p.x) / sqrt(1.0 + ls * ls);
    let d_right = (p.x - rx) / sqrt(1.0 + rs * rs);
    let d_top = -p.y;
    let d_bottom = p.y - size.y;

    var d = max(max(d_left, d_right), max(d_top, d_bottom));
    var di = max(max(d_left + bw.x, d_right + bw.y), max(d_top + bw.z, d_bottom + bw.w));

    // 45° corner cuts.
    let s = 0.70710678;
    if cut.x > 0.0 {
        let c = (cut.x - p.x - p.y) * s;
        d = max(d, c);
        di = max(di, c + 0.5 * (bw.x + bw.z));
    }
    if cut.y > 0.0 {
        let c = (cut.y - (size.x - p.x) - p.y) * s;
        d = max(d, c);
        di = max(di, c + 0.5 * (bw.y + bw.z));
    }
    if cut.z > 0.0 {
        let c = (cut.z - (size.x - p.x) - (size.y - p.y)) * s;
        d = max(d, c);
        di = max(di, c + 0.5 * (bw.y + bw.w));
    }
    if cut.w > 0.0 {
        let c = (cut.w - p.x - (size.y - p.y)) * s;
        d = max(d, c);
        di = max(di, c + 0.5 * (bw.x + bw.w));
    }

    let outer = clamp(0.5 - d, 0.0, 1.0);
    let inner = clamp(0.5 - di, 0.0, 1.0);

    var fill = mix(plate.fill_top, plate.fill_bottom, in.uv.y);
    // A soft lighter band under the top edge reads as brushed metal.
    if plate.extra.z > 0.0 {
        let k = clamp(1.0 - (p.y - bw.z) / (0.18 * h), 0.0, 1.0);
        fill = vec4<f32>(fill.rgb + vec3<f32>(plate.extra.z * k * k), fill.a);
    }
    // Diagonal hatching (locked items): every other stripe fades out.
    if plate.extra.x > 0.0 {
        let period = max(plate.extra.y * h, 4.0);
        let t = fract((p.x + p.y) / period);
        let aa = 1.0 / period;
        let band = smoothstep(0.5 - aa, 0.5 + aa, t) * (1.0 - smoothstep(1.0 - aa, 1.0, t));
        fill = vec4<f32>(fill.rgb, fill.a * (1.0 - plate.extra.x * band));
    }

    let col = mix(plate.line, fill, inner);
    return vec4<f32>(col.rgb, col.a * outer);
}
