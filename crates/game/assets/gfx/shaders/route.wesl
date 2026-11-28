// The selected trip's route (`gfx/route.rs`): unlit and vertex-coloured, with
// the line trimmed to the drawn car (or off-map traveller) every frame
// without touching the mesh or the material: the entity's `MeshTag` holds
// the cut (metres along the route, as f32 bits), a vertex's `uv.x` its
// distance along the route minus the lead it keeps from the cut, `uv.y` the
// length it fades in over (0: never trimmed: rings, pins, planned trips).

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_functions::get_tag,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
};

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
@if(VERTEX_UVS_A) {
    if in.uv.y > 0.0 {
        let cut = bitcast<f32>(get_tag(in.instance_index));
        let rel = in.uv.x - cut;
        if rel < 0.0 {
            discard;
        }
        let a = pow(clamp(rel / in.uv.y, 0.0, 1.0), 0.8);
        pbr_input.material.base_color.a *= a;
    }
}
    var out: FragmentOutput;
    out.color = pbr_input.material.base_color;
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
