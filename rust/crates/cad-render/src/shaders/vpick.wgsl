// B-rep vertices of every instance of a prototype as small screen-space squares, for the id buffer only.
@group(1) @binding(0) var<storage, read> pts: array<vec4<f32>>;

struct PIn {
    @builtin(vertex_index) vi: u32,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) m3: vec4<f32>,
    @location(10) id: u32,
};

struct POut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) @interpolate(flat) ids: vec2<u32>,
};

const HALF_PX: f32 = 3.0;

@vertex
fn vs_main(v: PIn) -> POut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let point = v.vi / 6u;
    let corner = corners[v.vi % 6u];
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let w = model * vec4<f32>(pts[point].xyz, 1.0);
    var c = g.vp_full * w;
    let off = corner * HALF_PX * 2.0 / g.viewport.xy;
    c = vec4<f32>(c.xy + off * c.w, c.z, c.w);
    var o: POut;
    o.clip = g.crop * c;
    o.world = w.xyz;
    o.ids = vec2<u32>(v.id, point);
    return o;
}

@fragment
fn fs_pick(in: POut) -> @location(0) vec2<u32> {
    if (clipped(in.world)) {
        discard;
    }
    return vec2<u32>(in.ids.x, pack_entity(KIND_VERTEX, in.ids.y));
}
