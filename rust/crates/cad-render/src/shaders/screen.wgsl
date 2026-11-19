// Screen-space layer: glyph quads (text) and solid polygons (view cube, plates) from the glyph atlas.
@group(1) @binding(0) var atlas: texture_2d<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct SIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct SOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(v: SIn) -> SOut {
    var o: SOut;
    let size = g.viewport.xy;
    o.clip = vec4<f32>(v.pos.x / size.x * 2.0 - 1.0, 1.0 - v.pos.y / size.y * 2.0, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    return o;
}

@fragment
fn fs_main(in: SOut) -> @location(0) vec4<f32> {
    let cov = textureSample(atlas, atlas_sampler, in.uv).r;
    let a = in.color.a * cov;
    return vec4<f32>(in.color.rgb * a, a);
}
