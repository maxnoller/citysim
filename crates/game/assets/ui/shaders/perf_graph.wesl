// The performance panel's frame-time graph: one column per time slice
// (the slowest frame in it), as a fraction of the graph's scale, drawn as a
// stepped area in a recessed well with a dashed budget line. Columns over
// the warning threshold take the warning colour; a column above the scale
// is cut at the top and capped with a brighter edge.
//
// Measured in physical pixels so the lines stay one pixel wide at every
// interface scale.

import bevy_ui_render::ui_vertex_output::UiVertexOutput;

struct Graph {
    // Up to 128 columns, four per vector, oldest first; negative = no data.
    values: array<vec4<f32>, 32>,
    bar: vec4<f32>,
    warn: vec4<f32>,
    back: vec4<f32>,
    line: vec4<f32>,
    // x: columns, y: budget line (fraction of the height), z: warning
    // threshold (fraction), w: horizontal grid lines (count).
    params: vec4<f32>,
}

@group(1) @binding(0)
var<uniform> graph: Graph;

fn value(i: u32) -> f32 {
    return graph.values[i / 4u][i % 4u];
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let size = max(in.size, vec2<f32>(1.0, 1.0));
    let p = in.uv * size;
    let n = max(graph.params.x, 1.0);
    let col = min(floor(in.uv.x * n), n - 1.0);
    let v = value(u32(col));
    // Height from the bottom, physical px.
    let y = size.y - p.y;

    var c = graph.back;

    // Faint horizontal grid.
    let rows = graph.params.w;
    if rows > 0.0 {
        let step = size.y / (rows + 1.0);
        let k = fract(y / step);
        if k * step < 1.0 && y > 1.0 && y < size.y - 1.0 {
            c = mix(c, graph.line, 0.18);
        }
    }

    if v > 0.0 {
        let top = min(v, 1.0) * size.y;
        let hot = v >= graph.params.z;
        let fill = select(graph.bar, graph.warn, hot);
        if y <= top {
            // Body darker towards the bottom, a bright 1 px top edge.
            let edge = top - y < 1.5;
            let body = mix(fill * vec4<f32>(0.55, 0.55, 0.55, 1.0), fill * vec4<f32>(0.85, 0.85, 0.85, 1.0), y / size.y);
            c = select(body, fill, edge);
            // Cut off above the scale: a 3 px cap in the warning colour.
            if v > 1.0 && top - y < 3.0 {
                c = graph.warn;
            }
        }
    }

    // Dashed budget line.
    let ly = graph.params.y * size.y;
    if graph.params.y > 0.0 && graph.params.y < 1.0 && abs(y - ly) < 0.75 && fract(p.x / 8.0) < 0.55 {
        c = mix(c, graph.line, graph.line.a);
    }
    return vec4<f32>(c.rgb, 1.0);
}
