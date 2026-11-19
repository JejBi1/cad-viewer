// Instanced shaded triangles with per-face colours.
struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) face: u32,
    @location(2) normal: vec3<f32>,
    @location(3) fcolor: vec4<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) m3: vec4<f32>,
    @location(8) icolor: vec4<f32>,
    @location(9) tint: vec4<f32>,
    @location(10) id: u32,
    @location(11) opacity: u32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) fcolor: vec4<f32>,
    @location(3) icolor: vec4<f32>,
    @location(4) tint: vec4<f32>,
    @location(5) @interpolate(flat) ids: vec3<u32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
    let model = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let w = model * vec4<f32>(v.pos, 1.0);
    var o: VOut;
    o.clip = g.view_proj * w;
    o.world = w.xyz;
    o.normal = (model * vec4<f32>(v.normal, 0.0)).xyz;
    o.fcolor = v.fcolor;
    o.icolor = v.icolor;
    o.tint = v.tint;
    o.ids = vec3<u32>(v.id, v.face, v.opacity);
    return o;
}

fn is_back(in: VOut) -> bool {
    let n = normalize(in.normal);
    let to_eye = select(-g.view_dir.xyz, normalize(g.eye.xyz - in.world), g.eye.w > 0.5);
    return dot(n, to_eye) < 0.0;
}

fn frag_color(in: VOut, back: bool) -> vec4<f32> {
    let n = normalize(in.normal);
    var base = g.default_color.rgb;
    if (in.icolor.a > 0.0) {
        base = in.icolor.rgb;
    }
    if (in.fcolor.a > 0.0) {
        base = in.fcolor.rgb;
    }
    if (in.tint.a > 0.0) {
        base = in.tint.rgb;
    }
    // back faces seen through the section plane are the cut: flat, in the colour of the part
    let rgb = shade(base, n, back && g.flags.x != 0u);
    let a = f32(in.ids.z) / 255.0;
    return vec4<f32>(rgb * a, a);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    if (clipped(in.world)) {
        discard;
    }
    let back = is_back(in);
    if (back && in.ids.z < 255u && in.ids.x != 0u) {
        // transparent solids show their front faces only (overlay meshes have id 0 and stay two-sided)
        discard;
    }
    return frag_color(in, back);
}

// Opaque surfaces while a section plane is active, pass 1: front faces only.
@fragment
fn fs_front(in: VOut) -> @location(0) vec4<f32> {
    if (clipped(in.world) || is_back(in)) {
        discard;
    }
    return frag_color(in, false);
}

// Pass 2: back faces seen through the cut are the cap, flat in the colour of the part. They also mark the stencil.
@fragment
fn fs_cap(in: VOut) -> @location(0) vec4<f32> {
    if (clipped(in.world) || !is_back(in)) {
        discard;
    }
    return frag_color(in, true);
}

@fragment
fn fs_pick(in: VOut) -> @location(0) vec2<u32> {
    if (clipped(in.world)) {
        discard;
    }
    return vec2<u32>(in.ids.x, pack_entity(KIND_FACE, in.ids.y));
}
