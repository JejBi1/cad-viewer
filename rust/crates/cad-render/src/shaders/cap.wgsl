// Pass 3 of the section cap: moves the depth of every stencil-marked pixel to the section plane, so that edges and
// surfaces behind the cap are hidden by it as by a real cap.
struct COut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> COut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: COut;
    o.clip = vec4<f32>(p[vi], 0.0, 1.0);
    o.ndc = p[vi];
    return o;
}

struct CapOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: COut) -> CapOut {
    var o: CapOut;
    o.color = vec4<f32>(0.0);
    // g.cam_extent: ortho = world half sizes of the view, perspective = tangents of the half angles
    var origin = g.eye.xyz;
    var dir = g.view_dir.xyz;
    if (g.eye.w > 0.5) {
        dir = normalize(g.view_dir.xyz + g.cam_right.xyz * (in.ndc.x * g.cam_extent.x) + g.cam_up.xyz * (in.ndc.y * g.cam_extent.y));
    } else {
        origin = origin + g.cam_right.xyz * (in.ndc.x * g.cam_extent.x) + g.cam_up.xyz * (in.ndc.y * g.cam_extent.y);
    }
    let denom = dot(g.clip.xyz, dir);
    var d = 0.0;
    if (abs(denom) > 1e-9) {
        let t = -(dot(g.clip.xyz, origin) + g.clip.w) / denom;
        let c = g.vp_full * vec4<f32>(origin + dir * t, 1.0);
        d = c.z / c.w;
    }
    o.depth = clamp(d, 0.0, 1.0);
    return o;
}
