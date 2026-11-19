// Thick, optionally dashed segments with round caps, expanded to screen-space quads. Used for measurement overlays,
// highlighted edges, the scale bar and the view cube outline.
struct LIn {
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec4<f32>,      // p0.xyz, width px
    @location(1) p1: vec3<f32>,
    @location(2) flags: u32,
    @location(3) color: vec4<f32>,
    @location(4) dash: vec2<f32>,   // on, off in px (on == 0: solid)
};

struct LOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) px: vec2<f32>,
    @location(1) @interpolate(flat) s0: vec2<f32>,
    @location(2) @interpolate(flat) s1: vec2<f32>,
    @location(3) @interpolate(flat) hw: f32,
    @location(4) @interpolate(flat) color: vec4<f32>,
    @location(5) @interpolate(flat) dash: vec2<f32>,
};

const EPS_W: f32 = 1e-5;

@vertex
fn vs_main(v: LIn) -> LOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let cc = corners[v.vi];
    let size = g.viewport.xy;
    var s0: vec2<f32>;
    var s1: vec2<f32>;
    var z0 = 0.0;
    var z1 = 0.0;
    var o: LOut;
    if ((v.flags & 1u) != 0u) {
        s0 = v.a.xy;
        s1 = v.p1.xy;
    } else {
        var c0 = g.vp_full * vec4<f32>(v.a.xyz, 1.0);
        var c1 = g.vp_full * vec4<f32>(v.p1, 1.0);
        if (c0.w < EPS_W && c1.w < EPS_W) {
            o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
            o.px = vec2<f32>(0.0);
            o.s0 = vec2<f32>(0.0);
            o.s1 = vec2<f32>(0.0);
            o.hw = 0.0;
            o.color = vec4<f32>(0.0);
            o.dash = vec2<f32>(0.0);
            return o;
        }
        if (c0.w < EPS_W) {
            c0 = mix(c0, c1, (EPS_W - c0.w) / (c1.w - c0.w));
        } else if (c1.w < EPS_W) {
            c1 = mix(c1, c0, (EPS_W - c1.w) / (c0.w - c1.w));
        }
        let n0 = c0.xyz / c0.w;
        let n1 = c1.xyz / c1.w;
        s0 = vec2<f32>((n0.x * 0.5 + 0.5) * size.x, (0.5 - n0.y * 0.5) * size.y);
        s1 = vec2<f32>((n1.x * 0.5 + 0.5) * size.x, (0.5 - n1.y * 0.5) * size.y);
        z0 = n0.z;
        z1 = n1.z;
    }
    let hw = 0.5 * v.a.w;
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
    let ndc = vec2<f32>(p.x / size.x * 2.0 - 1.0, 1.0 - p.y / size.y * 2.0);
    o.clip = g.crop * vec4<f32>(ndc, mix(z0, z1, cc.x), 1.0);
    o.px = p;
    o.s0 = s0;
    o.s1 = s1;
    o.hw = hw;
    o.color = v.color;
    o.dash = v.dash;
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
    let q = in.s0 + delta * t;
    let dist = distance(in.px, q);
    let cover = clamp(in.hw + 0.5 - dist, 0.0, 1.0);
    if (in.dash.x > 0.0) {
        let along = t * sqrt(len2);
        let period = in.dash.x + max(in.dash.y, 0.0);
        if (fract(along / period) * period > in.dash.x) {
            discard;
        }
    }
    let a = cover * in.color.a;
    if (a <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb * a, a);
}
