// Copies the offscreen picture into the egui render pass (premultiplied alpha, 1:1 pixels).
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

struct BOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> BOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: BOut;
    o.clip = vec4<f32>(p[vi], 0.0, 1.0);
    o.uv = vec2<f32>(p[vi].x * 0.5 + 0.5, 0.5 - p[vi].y * 0.5);
    return o;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

// target format is gamma-encoded (egui's preferred framebuffer): straight copy
@fragment
fn fs_gamma(in: BOut) -> @location(0) vec4<f32> {
    return textureSample(src, src_sampler, in.uv);
}

// target format is an sRGB format: the hardware encodes, so hand it linear values
@fragment
fn fs_linear(in: BOut) -> @location(0) vec4<f32> {
    let c = textureSample(src, src_sampler, in.uv);
    var rgb = vec3<f32>(0.0);
    if (c.a > 0.0) {
        rgb = to_linear(c.rgb / c.a) * c.a;
    }
    return vec4<f32>(rgb, c.a);
}
