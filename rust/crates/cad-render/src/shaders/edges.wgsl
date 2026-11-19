// B-rep edge polylines as instanced 1 px line lists (the same instance buffer as the faces).
struct EIn {
    @location(0) pos: vec3<f32>,
    @location(1) edge: u32,
    @location(2) m0: vec4<f32>,
    @location(3) m1: vec4<f32>,
    @location(4) m2: vec4<f32>,
    @location(5) m3: vec4<f32>,
    @location(6) icolor: vec4<f32>,
    @location(7) tint: vec4<f32>,
    @location(8) id: u32,
    @location(9) opacity: u32,
};

struct EOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) icolor: vec4<f32>,
    @location(2) tint: vec4<f32>,
    @location(3) @interpolate(flat) ids: vec2<u32>,
};

@vertex
fn vs_main(v: EIn) -> EOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let w = model * vec4<f32>(v.pos, 1.0);
    var o: EOut;
    o.clip = g.view_proj * w;
    o.world = w.xyz;
    o.icolor = v.icolor;
    o.tint = v.tint;
    o.ids = vec2<u32>(v.id, v.edge);
    return o;
}

@fragment
fn fs_main(in: EOut) -> @location(0) vec4<f32> {
    if (clipped(in.world)) {
        discard;
    }
    var c = g.edge_color.rgb;
    if (g.flags.y != 0u) {
        // wireframe: the lines carry the colour of the part
        c = g.default_color.rgb;
        if (in.icolor.a > 0.0) {
            c = in.icolor.rgb;
        }
        if (in.tint.a > 0.0) {
            c = in.tint.rgb;
        }
    }
    return vec4<f32>(c, 1.0);
}

@fragment
fn fs_pick(in: EOut) -> @location(0) vec2<u32> {
    if (clipped(in.world)) {
        discard;
    }
    return vec2<u32>(in.ids.x, pack_entity(KIND_EDGE, in.ids.y));
}
