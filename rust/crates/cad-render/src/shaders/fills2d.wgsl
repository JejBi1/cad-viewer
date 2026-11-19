// Filled triangles of the 2D drawing in world coordinates (relative to the scene origin).
struct G2 {
    center: vec2<f32>,
    viewport: vec2<f32>,
    scale: f32,
    width_mode: u32,
    min_width: f32,
    px_scale: f32,
    fg_mark: vec4<f32>,
    fg: vec4<f32>,
    bg_mark: vec4<f32>,
    bg: vec4<f32>,
};
@group(0) @binding(0) var<uniform> g: G2;

// The foreground / background colours of the layout stand in the batches as marker colours (see Globals2d).
fn palette(c: vec4<f32>) -> vec4<f32> {
    let eps = vec3<f32>(0.3 / 255.0);
    if (g.fg_mark.w > 0.5 && all(abs(c.rgb - g.fg_mark.rgb) < eps)) {
        return vec4<f32>(g.fg.rgb, c.a);
    }
    if (g.bg_mark.w > 0.5 && all(abs(c.rgb - g.bg_mark.rgb) < eps)) {
        return vec4<f32>(g.bg.rgb, c.a);
    }
    return c;
}

struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    let px = (v.pos - g.center) * g.scale;
    o.clip = vec4<f32>(px.x / (g.viewport.x * 0.5), px.y / (g.viewport.y * 0.5), 0.0, 1.0);
    o.color = palette(v.color);
    return o;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color.rgb * in.color.a, in.color.a);
}
