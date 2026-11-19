// Pictures of the 2D drawing (DXF IMAGE): textured quads in drawing coordinates relative to the scene origin. Alpha
// already holds the clipping mask and the transparency of the entity; the output is premultiplied.
struct G2 {
    center: vec2<f32>,
    viewport: vec2<f32>,
    scale: f32,
    width_mode: u32,
    min_width: f32,
    pad: f32,
};
@group(0) @binding(0) var<uniform> g: G2;
@group(1) @binding(0) var picture: texture_2d<f32>;
@group(1) @binding(1) var picture_sampler: sampler;

struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    let px = (v.pos - g.center) * g.scale;
    o.clip = vec4<f32>(px.x / (g.viewport.x * 0.5), px.y / (g.viewport.y * 0.5), 0.0, 1.0);
    o.uv = v.uv;
    return o;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let c = textureSample(picture, picture_sampler, in.uv);
    return vec4<f32>(c.rgb * c.a, c.a);
}
