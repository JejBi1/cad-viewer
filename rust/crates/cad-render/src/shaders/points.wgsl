// Round markers of constant pixel size at world positions.
struct PIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pos_size: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) flags: u32,
};

struct POut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) size: f32,
    @location(2) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn vs_main(v: PIn) -> POut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let corner = corners[v.vi];
    var c = g.vp_full * vec4<f32>(v.pos_size.xyz, 1.0);
    var o: POut;
    if (c.w < 1e-5) {
        o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        o.local = vec2<f32>(0.0);
        o.size = 0.0;
        o.color = vec4<f32>(0.0);
        return o;
    }
    let half_px = v.pos_size.w * 0.5 + 1.0;
    let off = corner * half_px * 2.0 / g.viewport.xy;
    c = vec4<f32>(c.xy + off * c.w, c.z, c.w);
    o.clip = g.crop * c;
    o.local = corner * half_px;
    o.size = v.pos_size.w;
    o.color = v.color;
    return o;
}

@fragment
fn fs_main(in: POut) -> @location(0) vec4<f32> {
    let d = length(in.local);
    let a = clamp(in.size * 0.5 + 0.5 - d, 0.0, 1.0) * in.color.a;
    if (a <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb * a, a);
}
