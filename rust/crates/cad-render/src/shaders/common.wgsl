// Shared by all 3D shaders. Colours are sRGB-encoded and shaded in gamma space (like the thumbnail renderer).
struct Globals {
    view_proj: mat4x4<f32>,
    vp_full: mat4x4<f32>,
    crop: mat4x4<f32>,
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    view_dir: vec4<f32>,
    eye: vec4<f32>,
    clip: vec4<f32>,
    viewport: vec4<f32>,
    cam_extent: vec4<f32>,
    default_color: vec4<f32>,
    edge_color: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> g: Globals;

const KIND_FACE: u32 = 0u;
const KIND_EDGE: u32 = 1u;
const KIND_VERTEX: u32 = 2u;
const INDEX_MASK: u32 = 0x3fffffffu;

fn pack_entity(kind: u32, index: u32) -> u32 {
    return (kind << 30u) | (index & INDEX_MASK);
}

// true when the fragment lies on the removed side of the section plane
fn clipped(world: vec3<f32>) -> bool {
    return g.flags.x != 0u && (dot(g.clip.xyz, world) + g.clip.w) < 0.0;
}

// Two-sided Lambert with the light of the thumbnail renderer: shade = 0.30 + 0.70 * lambda, specular 0.08 * lambda^8.
fn shade(base: vec3<f32>, n_world: vec3<f32>, cap: bool) -> vec3<f32> {
    var n = n_world;
    if (cap) {
        n = -g.clip.xyz;
    }
    let nv = vec3<f32>(dot(n, g.cam_right.xyz), dot(n, g.cam_up.xyz), dot(n, -g.view_dir.xyz));
    let l = normalize(vec3<f32>(-0.35, 0.55, 0.75));
    let lam = abs(dot(nv, l));
    return clamp(base * (0.30 + 0.70 * lam) + vec3<f32>(0.08 * pow(lam, 8.0)), vec3<f32>(0.0), vec3<f32>(1.0));
}
