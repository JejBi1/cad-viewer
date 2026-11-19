// Segments of the 2D drawing as capsules. The width is in drawing units (hairline = 0 draws min_width pixels), or in
// pixels when width_mode is 1 (2: pixels times px_scale, the highlight of a selection).
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

struct LIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pts: vec4<f32>,   // p0.xy, p1.xy
    @location(1) width: f32,
    @location(2) color: vec4<f32>,
};

struct LOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) px: vec2<f32>,
    @location(1) @interpolate(flat) s0: vec2<f32>,
    @location(2) @interpolate(flat) s1: vec2<f32>,
    @location(3) @interpolate(flat) hw: f32,
    @location(4) @interpolate(flat) color: vec4<f32>,
};

fn to_px(p: vec2<f32>) -> vec2<f32> {
    let d = (p - g.center) * g.scale;
    return vec2<f32>(g.viewport.x * 0.5 + d.x, g.viewport.y * 0.5 - d.y);
}

@vertex
fn vs_main(v: LIn) -> LOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let cc = corners[v.vi];
    let s0 = to_px(v.pts.xy);
    let s1 = to_px(v.pts.zw);
    var wpx = v.width * g.scale;
    if (g.width_mode == 1u) {
        wpx = v.width;
    }
    if (g.width_mode == 2u) {
        wpx = v.width * g.px_scale;
    }
    wpx = max(wpx, g.min_width);
    let hw = 0.5 * wpx;
    let margin = hw + 1.0;
    let delta = s1 - s0;
    let len = length(delta);
    var d = vec2<f32>(1.0, 0.0);
    if (len > 1e-6) {
        d = delta / len;
    }
    let nrm = vec2<f32>(-d.y, d.x);
    let base = mix(s0, s1, cc.x);
    let p = base + d * ((cc.x * 2.0 - 1.0) * margin) + nrm * (cc.y * margin);
    var o: LOut;
    o.clip = vec4<f32>(p.x / g.viewport.x * 2.0 - 1.0, 1.0 - p.y / g.viewport.y * 2.0, 0.0, 1.0);
    o.px = p;
    o.s0 = s0;
    o.s1 = s1;
    o.hw = hw;
    o.color = palette(v.color);
    return o;
}

@fragment
fn fs_main(in: LOut) -> @location(0) vec4<f32> {
    let delta = in.s1 - in.s0;
    let len2 = dot(delta, delta);
    var t = 0.0;
    if (len2 > 1e-12) {
        t = clamp(dot(in.px - in.s0, delta) / len2, 0.0, 1.0);
    }
    let dist = distance(in.px, in.s0 + delta * t);
    let a = clamp(in.hw + 0.5 - dist, 0.0, 1.0) * in.color.a;
    if (a <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb * a, a);
}
