//! Render pipelines, bind group layouts and the shaders of the 3D renderer.
use std::borrow::Cow;

use eframe::wgpu;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Depth of the id passes.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Depth and stencil of the scene (the stencil marks the section cap).
pub const SCENE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;
pub const PICK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;

pub const COMMON_WGSL: &str = include_str!("shaders/common.wgsl");

fn module(device: &wgpu::Device, label: &str, parts: &[&str]) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(parts.concat())),
    })
}

/// Layout of the per-instance buffer ([`crate::scene3d::InstanceRaw`]) with shader locations starting at `first`.
pub fn instance_attributes(first: u32) -> [wgpu::VertexAttribute; 8] {
    use wgpu::{VertexAttribute as A, VertexFormat as F};
    [
        A { format: F::Float32x4, offset: 0, shader_location: first },
        A { format: F::Float32x4, offset: 16, shader_location: first + 1 },
        A { format: F::Float32x4, offset: 32, shader_location: first + 2 },
        A { format: F::Float32x4, offset: 48, shader_location: first + 3 },
        A { format: F::Unorm8x4, offset: 64, shader_location: first + 4 },
        A { format: F::Unorm8x4, offset: 68, shader_location: first + 5 },
        A { format: F::Uint32, offset: 72, shader_location: first + 6 },
        A { format: F::Uint32, offset: 76, shader_location: first + 7 },
    ]
}

pub const INSTANCE_STRIDE: u64 = 80;

pub struct Layouts {
    pub globals: wgpu::BindGroupLayout,
    pub atlas: wgpu::BindGroupLayout,
    pub vpick: wgpu::BindGroupLayout,
    pub blit: wgpu::BindGroupLayout,
    /// Texture and sampler of one picture of the 2D drawing.
    pub picture: wgpu::BindGroupLayout,
}

impl Layouts {
    pub fn new(device: &wgpu::Device) -> Self {
        let globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let atlas = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        let blit = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        let vpick = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vpick"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let picture = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("picture"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        Self { globals, atlas, blit, vpick, picture }
    }
}

fn premultiplied() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

fn depth_state(
    format: wgpu::TextureFormat,
    write: bool,
    compare: wgpu::CompareFunction,
    bias: wgpu::DepthBiasState,
) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias,
    }
}

fn stencil(compare: wgpu::CompareFunction, pass_op: wgpu::StencilOperation) -> wgpu::StencilState {
    let face = wgpu::StencilFaceState {
        compare,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op,
    };
    wgpu::StencilState { front: face, back: face, read_mask: 0xff, write_mask: 0xff }
}

struct Spec<'a> {
    label: &'a str,
    module: &'a wgpu::ShaderModule,
    vs: &'a str,
    fs: &'a str,
    layout: &'a wgpu::PipelineLayout,
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    topology: wgpu::PrimitiveTopology,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    write_mask: wgpu::ColorWrites,
    depth: Option<wgpu::DepthStencilState>,
    samples: u32,
}

fn build(device: &wgpu::Device, s: Spec<'_>) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(s.label),
        layout: Some(s.layout),
        vertex: wgpu::VertexState {
            module: s.module,
            entry_point: Some(s.vs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: s.buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology: s.topology,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: s.depth,
        multisample: wgpu::MultisampleState { count: s.samples, mask: !0, alpha_to_coverage_enabled: false },
        fragment: Some(wgpu::FragmentState {
            module: s.module,
            entry_point: Some(s.fs),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState { format: s.format, blend: s.blend, write_mask: s.write_mask })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// Pipelines that depend on the MSAA sample count.
pub struct Pipelines {
    pub bg: wgpu::RenderPipeline,
    pub mesh_opaque: wgpu::RenderPipeline,
    /// Section plane, pass 1: front faces.
    pub cap_front: wgpu::RenderPipeline,
    /// Section plane, pass 2: the cap (back faces seen through the cut), marks the stencil.
    pub cap_mark: wgpu::RenderPipeline,
    /// Section plane, pass 3: moves the marked pixels to the depth of the plane.
    pub cap_depth: wgpu::RenderPipeline,
    pub mesh_transparent: wgpu::RenderPipeline,
    pub mesh_overlay: wgpu::RenderPipeline,
    pub edges: wgpu::RenderPipeline,
    pub lines: wgpu::RenderPipeline,
    pub points: wgpu::RenderPipeline,
    pub screen: wgpu::RenderPipeline,
}

/// Pipelines of the id passes (single sample, `Rg32Uint`).
pub struct PickPipelines {
    pub mesh: wgpu::RenderPipeline,
    pub edges: wgpu::RenderPipeline,
    pub vertices: wgpu::RenderPipeline,
}

fn mesh_vertex_attrs() -> [wgpu::VertexAttribute; 4] {
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Float32x3, 3 => Unorm8x4]
}

fn edge_vertex_attrs() -> [wgpu::VertexAttribute; 2] {
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32]
}

/// Face bias that lets the edges win the depth test against the faces they lie on.
fn face_bias() -> wgpu::DepthBiasState {
    wgpu::DepthBiasState { constant: 2, slope_scale: 1.0, clamp: 0.0 }
}

impl Pipelines {
    pub fn new(device: &wgpu::Device, layouts: &Layouts, samples: u32) -> Self {
        use wgpu::CompareFunction as C;
        let none = wgpu::DepthBiasState::default();
        let g_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("globals only"),
            bind_group_layouts: &[Some(&layouts.globals)],
            immediate_size: 0,
        });
        let s_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("screen"),
            bind_group_layouts: &[Some(&layouts.globals), Some(&layouts.atlas)],
            immediate_size: 0,
        });
        let mesh_src = module(device, "mesh", &[COMMON_WGSL, include_str!("shaders/mesh.wgsl")]);
        let edge_src = module(device, "edges", &[COMMON_WGSL, include_str!("shaders/edges.wgsl")]);
        let line_src = module(device, "lines", &[COMMON_WGSL, include_str!("shaders/lines.wgsl")]);
        let point_src = module(device, "points", &[COMMON_WGSL, include_str!("shaders/points.wgsl")]);
        let screen_src = module(device, "screen", &[COMMON_WGSL, include_str!("shaders/screen.wgsl")]);
        let bg_src = module(device, "background", &[COMMON_WGSL, include_str!("shaders/background.wgsl")]);
        let cap_src = module(device, "cap", &[COMMON_WGSL, include_str!("shaders/cap.wgsl")]);

        let mv = mesh_vertex_attrs();
        let inst4 = instance_attributes(4);
        let mesh_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 32,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &mv,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &inst4,
            }),
        ];
        let ev = edge_vertex_attrs();
        let inst2 = instance_attributes(2);
        let edge_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ev,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE_STRIDE,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &inst2,
            }),
        ];
        // the LineInst struct has p1 at offset 16, flags at 28, color at 32, dash at 36
        let line_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 28, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 32, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 36, shader_location: 4 },
        ];
        let line_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 48,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &line_attrs,
        })];
        let point_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 20, shader_location: 2 },
        ];
        let point_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 32,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &point_attrs,
        })];
        let screen_attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4];
        let screen_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 20,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &screen_attrs,
        })];

        let tri = wgpu::PrimitiveTopology::TriangleList;
        let mk = |label: &str, m: &wgpu::ShaderModule, vs: &str, fs: &str, layout: &wgpu::PipelineLayout, buffers: &[Option<wgpu::VertexBufferLayout>], topology, blend, depth| {
            build(
                device,
                Spec { label, module: m, vs, fs, layout, buffers, topology, format: COLOR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL, depth: Some(depth), samples },
            )
        };
        Self {
            bg: mk("background", &bg_src, "vs_main", "fs_main", &g_layout, &[], tri, None, depth_state(SCENE_DEPTH_FORMAT, false, C::Always, none)),
            mesh_opaque: mk("mesh opaque", &mesh_src, "vs_main", "fs_main", &g_layout, &mesh_buffers, tri, None, depth_state(SCENE_DEPTH_FORMAT, true, C::LessEqual, face_bias())),
            cap_front: mk("cap front", &mesh_src, "vs_main", "fs_front", &g_layout, &mesh_buffers, tri, None, depth_state(SCENE_DEPTH_FORMAT, true, C::LessEqual, face_bias())),
            cap_mark: mk("cap mark", &mesh_src, "vs_main", "fs_cap", &g_layout, &mesh_buffers, tri, None, wgpu::DepthStencilState { stencil: stencil(C::Always, wgpu::StencilOperation::Replace), ..depth_state(SCENE_DEPTH_FORMAT, true, C::LessEqual, none) }),
            cap_depth: build(
                device,
                Spec {
                    label: "cap depth",
                    module: &cap_src,
                    vs: "vs_main",
                    fs: "fs_main",
                    layout: &g_layout,
                    buffers: &[],
                    topology: tri,
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                    depth: Some(wgpu::DepthStencilState { stencil: stencil(C::Equal, wgpu::StencilOperation::Keep), ..depth_state(SCENE_DEPTH_FORMAT, true, C::Always, none) }),
                    samples,
                },
            ),
            mesh_transparent: mk("mesh transparent", &mesh_src, "vs_main", "fs_main", &g_layout, &mesh_buffers, tri, Some(premultiplied()), depth_state(SCENE_DEPTH_FORMAT, false, C::LessEqual, none)),
            mesh_overlay: mk("mesh overlay", &mesh_src, "vs_main", "fs_main", &g_layout, &mesh_buffers, tri, Some(premultiplied()), depth_state(SCENE_DEPTH_FORMAT, true, C::LessEqual, none)),
            edges: mk("edges", &edge_src, "vs_main", "fs_main", &g_layout, &edge_buffers, wgpu::PrimitiveTopology::LineList, None, depth_state(SCENE_DEPTH_FORMAT, true, C::LessEqual, none)),
            lines: mk("lines", &line_src, "vs_main", "fs_main", &g_layout, &line_buffers, tri, Some(premultiplied()), depth_state(SCENE_DEPTH_FORMAT, false, C::Always, none)),
            points: mk("points", &point_src, "vs_main", "fs_main", &g_layout, &point_buffers, tri, Some(premultiplied()), depth_state(SCENE_DEPTH_FORMAT, false, C::Always, none)),
            screen: mk("screen", &screen_src, "vs_main", "fs_main", &s_layout, &screen_buffers, tri, Some(premultiplied()), depth_state(SCENE_DEPTH_FORMAT, false, C::Always, none)),
        }
    }
}

impl PickPipelines {
    pub fn new(device: &wgpu::Device, layouts: &Layouts) -> Self {
        use wgpu::CompareFunction as C;
        let none = wgpu::DepthBiasState::default();
        let g_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pick globals"),
            bind_group_layouts: &[Some(&layouts.globals)],
            immediate_size: 0,
        });
        let v_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pick vertices"),
            bind_group_layouts: &[Some(&layouts.globals), Some(&layouts.vpick)],
            immediate_size: 0,
        });
        let mesh_src = module(device, "mesh", &[COMMON_WGSL, include_str!("shaders/mesh.wgsl")]);
        let edge_src = module(device, "edges", &[COMMON_WGSL, include_str!("shaders/edges.wgsl")]);
        let vpick_src = module(device, "vpick", &[COMMON_WGSL, include_str!("shaders/vpick.wgsl")]);
        let mv = mesh_vertex_attrs();
        let inst4 = instance_attributes(4);
        let mesh_buffers = [
            Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &mv }),
            Some(wgpu::VertexBufferLayout { array_stride: INSTANCE_STRIDE, step_mode: wgpu::VertexStepMode::Instance, attributes: &inst4 }),
        ];
        let ev = edge_vertex_attrs();
        let inst2 = instance_attributes(2);
        let edge_buffers = [
            Some(wgpu::VertexBufferLayout { array_stride: 16, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ev }),
            Some(wgpu::VertexBufferLayout { array_stride: INSTANCE_STRIDE, step_mode: wgpu::VertexStepMode::Instance, attributes: &inst2 }),
        ];
        // the vertex pass reads positions from a storage buffer and only needs matrix + id of the instance
        let vp_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 4 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 5 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 6 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 7 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 72, shader_location: 10 },
        ];
        let vp_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: INSTANCE_STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &vp_attrs,
        })];
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let mk = |label: &str, m: &wgpu::ShaderModule, fs: &str, layout: &wgpu::PipelineLayout, buffers: &[Option<wgpu::VertexBufferLayout>], topology, depth| {
            build(
                device,
                Spec { label, module: m, vs: "vs_main", fs, layout, buffers, topology, format: PICK_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL, depth: Some(depth), samples: 1 },
            )
        };
        Self {
            mesh: mk("pick faces", &mesh_src, "fs_pick", &g_layout, &mesh_buffers, tri, depth_state(DEPTH_FORMAT, true, C::LessEqual, face_bias())),
            edges: mk("pick edges", &edge_src, "fs_pick", &g_layout, &edge_buffers, wgpu::PrimitiveTopology::LineList, depth_state(DEPTH_FORMAT, true, C::LessEqual, none)),
            vertices: mk("pick vertices", &vpick_src, "fs_pick", &v_layout, &vp_buffers, tri, depth_state(DEPTH_FORMAT, true, C::LessEqual, none)),
        }
    }
}

/// Blit into the egui pass: format, MSAA count and depth format of that pass.
pub fn blit_pipeline(
    device: &wgpu::Device,
    layouts: &Layouts,
    format: wgpu::TextureFormat,
    samples: u32,
    depth: Option<wgpu::TextureFormat>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("blit"),
        bind_group_layouts: &[Some(&layouts.blit)],
        immediate_size: 0,
    });
    let src = module(device, "blit", &[include_str!("shaders/blit.wgsl")]);
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("blit"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &src,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: depth.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState { count: samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
        fragment: Some(wgpu::FragmentState {
            module: &src,
            entry_point: Some(if format.is_srgb() { "fs_linear" } else { "fs_gamma" }),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: Some(premultiplied()), write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn line_attributes() -> [wgpu::VertexAttribute; 5] {
    [
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 16, shader_location: 1 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 28, shader_location: 2 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 32, shader_location: 3 },
        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 36, shader_location: 4 },
    ]
}

/// Screen-space pipelines (thick lines in pixels, glyph and polygon layer) for targets without a depth buffer.
pub struct UiPipelines {
    pub lines: wgpu::RenderPipeline,
    pub screen: wgpu::RenderPipeline,
}

impl UiPipelines {
    pub fn new(device: &wgpu::Device, layouts: &Layouts, samples: u32) -> Self {
        let g_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui globals"),
            bind_group_layouts: &[Some(&layouts.globals)],
            immediate_size: 0,
        });
        let s_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui screen"),
            bind_group_layouts: &[Some(&layouts.globals), Some(&layouts.atlas)],
            immediate_size: 0,
        });
        let line_src = module(device, "lines", &[COMMON_WGSL, include_str!("shaders/lines.wgsl")]);
        let screen_src = module(device, "screen", &[COMMON_WGSL, include_str!("shaders/screen.wgsl")]);
        let la = line_attributes();
        let line_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 48,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &la,
        })];
        let sa = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4];
        let screen_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 20,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &sa,
        })];
        let mk = |label: &str, m: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout, buffers: &[Option<wgpu::VertexBufferLayout>]| {
            build(
                device,
                Spec {
                    label,
                    module: m,
                    vs: "vs_main",
                    fs: "fs_main",
                    layout,
                    buffers,
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    format: COLOR_FORMAT,
                    blend: Some(premultiplied()),
                    write_mask: wgpu::ColorWrites::ALL,
                    depth: None,
                    samples,
                },
            )
        };
        Self {
            lines: mk("ui lines", &line_src, &g_layout, &line_buffers),
            screen: mk("ui screen", &screen_src, &s_layout, &screen_buffers),
        }
    }
}

/// World-space pipelines of the 2D drawing: filled triangles and capsule lines whose width is in drawing units.
pub struct Scene2dPipelines {
    pub fills: wgpu::RenderPipeline,
    pub lines: wgpu::RenderPipeline,
    /// Pictures (DXF IMAGE): textured quads, globals in group 0 and the picture in group 1.
    pub pictures: wgpu::RenderPipeline,
}

impl Scene2dPipelines {
    pub fn new(device: &wgpu::Device, layouts: &Layouts, samples: u32) -> Self {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene2d"),
            bind_group_layouts: &[Some(&layouts.globals)],
            immediate_size: 0,
        });
        let src = module(device, "fills2d", &[include_str!("shaders/fills2d.wgsl")]);
        let lsrc = module(device, "lines2d", &[include_str!("shaders/lines2d.wgsl")]);
        let fa = wgpu::vertex_attr_array![0 => Float32x2, 1 => Unorm8x4];
        let fill_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 12,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &fa,
        })];
        let la = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32, 2 => Unorm8x4];
        let line_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: 24,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &la,
        })];
        let mk = |label: &str, m: &wgpu::ShaderModule, buffers: &[Option<wgpu::VertexBufferLayout>]| {
            build(
                device,
                Spec {
                    label,
                    module: m,
                    vs: "vs_main",
                    fs: "fs_main",
                    layout: &layout,
                    buffers,
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    format: COLOR_FORMAT,
                    blend: Some(premultiplied()),
                    write_mask: wgpu::ColorWrites::ALL,
                    depth: None,
                    samples,
                },
            )
        };
        let picture_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene2d pictures"),
            bind_group_layouts: &[Some(&layouts.globals), Some(&layouts.picture)],
            immediate_size: 0,
        });
        let psrc = module(device, "image2d", &[include_str!("shaders/image2d.wgsl")]);
        let pa = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
        let picture_buffers = [Some(wgpu::VertexBufferLayout { array_stride: 16, step_mode: wgpu::VertexStepMode::Vertex, attributes: &pa })];
        let pictures = build(
            device,
            Spec {
                label: "image2d",
                module: &psrc,
                vs: "vs_main",
                fs: "fs_main",
                layout: &picture_layout,
                buffers: &picture_buffers,
                topology: wgpu::PrimitiveTopology::TriangleList,
                format: COLOR_FORMAT,
                blend: Some(premultiplied()),
                write_mask: wgpu::ColorWrites::ALL,
                depth: None,
                samples,
            },
        );
        Self { fills: mk("fills2d", &src, &fill_buffers), lines: mk("lines2d", &lsrc, &line_buffers), pictures }
    }
}
