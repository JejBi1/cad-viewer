// Vertical gradient background (top to bottom), drawn first.
struct BOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> BOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: BOut;
    o.clip = vec4<f32>(p[vi], 0.0, 1.0);
    return o;
}

@fragment
fn fs_main(in: BOut) -> @location(0) vec4<f32> {
    let t = clamp(in.clip.y / g.viewport.y, 0.0, 1.0);
    return vec4<f32>(mix(g.bg_top.rgb, g.bg_bottom.rgb, t), 1.0);
}
