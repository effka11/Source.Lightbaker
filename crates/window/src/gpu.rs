use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};

const POS_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
const COLOR_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x3];
const TEX_ATTR: [wgpu::VertexAttribute; 5] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 12,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32,
        offset: 20,
        shader_location: 2,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 24,
        shader_location: 3,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 32,
        shader_location: 4,
    },
];
const LIGHT_UV_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Float32x2];
const PLAIN_LIGHT_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![3 => Float32x4];
const PLAIN_NORMAL_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![4 => Float32x3];
const TEX_LIGHT_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![5 => Float32x4];
const TEX_NORMAL_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![6 => Float32x3];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TexVert {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub blend: f32,
    pub detail_scale: f32,
    pub detail_factor: f32,
    pub light_uv: [f32; 2],
}

#[derive(Clone, Copy)]
pub struct Draw {
    pub start: u32,
    pub count: u32,
    pub base: u32,
    pub second: u32,
    pub detail: u32,
}

pub struct Scene {
    pub verts: Vec<TexVert>,
    pub draws: Vec<Draw>,
    pub images: Arc<Vec<map::Image>>,
    pub plain_positions: Vec<[f32; 3]>,
    pub plain_colors: Vec<[f32; 3]>,
    pub plain_uvs: Vec<[f32; 2]>,
    /// Luxel of each plain vertex. `u32::MAX` samples the lightmap.
    pub plain_luxels: Vec<u32>,
    /// 1 when the plain vertex is a prop, so a lamp figure can still tint it.
    pub plain_props: Vec<f32>,
    /// Outward normal of each plain vertex. The figure tints the side facing its center.
    pub plain_normals: Vec<[f32; 3]>,
    /// Luxel of each textured vertex, parallel to [`Self::verts`].
    pub tex_luxels: Vec<u32>,
    /// 1 when the textured vertex is a prop.
    pub tex_props: Vec<f32>,
    /// Outward normal of each textured vertex, parallel to [`Self::verts`].
    pub tex_normals: Vec<[f32; 3]>,
}

impl Scene {
    pub fn empty() -> Self {
        Self {
            verts: Vec::new(),
            draws: Vec::new(),
            images: Arc::new(Vec::new()),
            plain_positions: Vec::new(),
            plain_colors: Vec::new(),
            plain_uvs: Vec::new(),
            plain_luxels: Vec::new(),
            plain_props: Vec::new(),
            plain_normals: Vec::new(),
            tex_luxels: Vec::new(),
            tex_props: Vec::new(),
            tex_normals: Vec::new(),
        }
    }
}

pub struct RoomCallback {
    pub scene: Arc<Scene>,
    pub scene_id: u64,
    pub marker_positions: Arc<Vec<[f32; 3]>>,
    pub marker_colors: Arc<Vec<[f32; 3]>>,
    pub marker_id: u64,
    pub view_proj: [[f32; 4]; 4],
    pub width: u32,
    pub height: u32,
    pub slot: usize,
    /// Luxel atlas, RGBA16F, tightly packed. `lightmap_size` is its width and height.
    pub lightmap: Arc<Vec<u16>>,
    pub lightmap_size: u32,
    pub light_id: u64,
    /// Solved color per plain vertex. `x < 0` samples the lightmap. `w > 0` is a prop.
    pub plain_lights: Arc<Vec<[f32; 4]>>,
    /// Solved color per textured vertex. `x < 0` samples the lightmap. `w > 0` is a prop.
    pub tex_lights: Arc<Vec<[f32; 4]>>,
    /// On, textures stay at least fullbright and placed lights still brighten
    /// them. Off, the world is lit only by those lights.
    pub map_light: bool,
    /// Fixture meshes inside these boxes take the lamp color.
    pub glow: Vec<crate::project::FigureGlow>,
}

const GLOW_CAP: usize = 64;

macro_rules! with_light {
    ($tail:literal) => {
        concat!(
            r#"
struct Lamp {
    center: vec4<f32>,
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
    axis_z: vec4<f32>,
    color: vec4<f32>,
}

struct Uniforms {
    view_proj: mat4x4<f32>,
    fullbright: f32,
    unlit_x: f32,
    unlit_y: f32,
    lamp_count: u32,
    lamps: array<Lamp, 64>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var light_tex: texture_2d<f32>;
@group(0) @binding(2) var light_samp: sampler;

fn shade(color: vec3<f32>, light: vec3<f32>) -> vec3<f32> {
    let base = max(color, vec3<f32>(0.0));
    let energy = base * max(light, vec3<f32>(0.0));
    let lit = min(energy, vec3<f32>(1.0));
    // Off, a moderate wash still leaves the texture readable, so it looks like
    // that texture kept its light. Squaring drops the wash. Past a modest
    // lamp the shoulder bends, instead of a flat white disk on the ceiling.
    let curve = energy * energy;
    let lamp = curve / (vec3<f32>(1.0) + max(curve - vec3<f32>(0.25), vec3<f32>(0.0)));
    return mix(lamp, max(base, lit), uniforms.fullbright);
}

fn bare(uv: vec2<f32>) -> bool {
    return abs(uv.x - uniforms.unlit_x) < 1e-4 && abs(uv.y - uniforms.unlit_y) < 1e-4;
}

fn lit(uv: vec2<f32>) -> vec3<f32> {
    // Samples stay in uniform control flow. The bare texel is a face with no grid.
    let texel = 1.0 / max(f32(textureDimensions(light_tex).x), 1.0);
    var sum = vec3<f32>(0.0);
    for (var y: i32 = -1; y <= 1; y++) {
        for (var x: i32 = -1; x <= 1; x++) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel;
            sum += textureSample(light_tex, light_samp, uv + offset).rgb;
        }
    }
    let raw = textureSample(light_tex, light_samp, uv).rgb;
    if (bare(uv)) {
        return raw;
    }
    return sum / 9.0;
}

fn shell_light(world: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    var light = vec3<f32>(0.0);
    let count = min(uniforms.lamp_count, 64u);
    let n = normal * inverseSqrt(max(dot(normal, normal), 1e-8));
    for (var i = 0u; i < 64u; i++) {
        if (i >= count) {
            break;
        }
        let lamp = uniforms.lamps[i];
        let offset = world - lamp.center.xyz;
        let hx = dot(lamp.axis_x.xyz, lamp.axis_x.xyz);
        let hy = dot(lamp.axis_y.xyz, lamp.axis_y.xyz);
        let hz = dot(lamp.axis_z.xyz, lamp.axis_z.xyz);
        if (hx < 1e-8 || hy < 1e-8 || hz < 1e-8) {
            continue;
        }
        let u = abs(dot(offset, lamp.axis_x.xyz)) / hx;
        let v = abs(dot(offset, lamp.axis_y.xyz)) / hy;
        let w = abs(dot(offset, lamp.axis_z.xyz)) / hz;
        let box = max(u, max(v, w));
        // center.w is the interior strength, axis_z.w is where the glow ends.
        let gain = lamp.center.w;
        if (gain <= 0.0) {
            continue;
        }
        // The mesh sits inside the slack box, so a fade that starts at the box
        // edge never reaches the cage. 0 cuts the glow inside the figure, 1
        // keeps it bright through the shell and softens past it.
        let rim = clamp(lamp.axis_z.w, 0.0, 1.0);
        let inner = mix(0.2, 1.5, rim);
        let outer = mix(0.45, 2.2, rim);
        if (box > outer) {
            continue;
        }
        let edge = clamp((outer - box) / max(outer - inner, 0.02), 0.0, 1.0);
        let toward = -offset;
        let reach = max(dot(toward, toward), 1e-6);
        // axis_x.w is how much the metal cage keeps. axis_y.w is how far the
        // bright center reaches. A hard zero on the shell draws a black stroke.
        let mesh = clamp(lamp.axis_x.w, 0.0, 1.0);
        let spread = max(clamp(lamp.axis_y.w, 0.0, 1.0), 0.02);
        let ndotl = dot(n, toward) * inverseSqrt(reach);
        let facing = clamp(ndotl * 0.7 + 0.3, mesh, 1.0);
        let core = clamp(1.0 - box / spread, 0.0, 1.0);
        let weight = max(facing, mix(mesh, 1.0, core));
        let dist2 = max(dot(offset, offset), 1e-4);
        // Up to 40 the shoulder matches the old glass. Past that the same
        // slider keeps climbing, instead of flattening near white.
        let energy = lamp.color.w / (dist2 + 48.0 * 48.0) * gain * 0.35 * edge * weight;
        let shaped = energy / (1.0 + max(energy - 0.35, 0.0));
        let boost = 1.0 + max(gain - 0.4, 0.0) / 0.6 * 3.0;
        light += lamp.color.xyz * shaped * boost;
    }
    return light;
}

fn with_figure(albedo: vec3<f32>, irradiance: vec3<f32>, uv: vec2<f32>, world: vec3<f32>, prop: f32, normal: vec3<f32>) -> vec3<f32> {
    let shaded = shade(albedo, irradiance);
    // Walls keep the lightmap. The fixture still glows when the map is lit,
    // including a near-black texel in the cage that a plain multiply would leave dark.
    if (prop < 0.5 && !bare(uv)) {
        return shaded;
    }
    let glow = shell_light(world, normal);
    let peak = max(albedo.r, max(albedo.g, albedo.b));
    let cover = mix(vec3<f32>(0.65), albedo, clamp(peak * 2.5, 0.0, 1.0));
    return min(shaded + cover * glow, vec3<f32>(1.0));
}
"#,
            $tail
        )
    };
}

const SCENE_WGSL: &str = with_light!(
    r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) light_uv: vec2<f32>,
    @location(2) world: vec3<f32>,
    @location(3) vertex_light: vec4<f32>,
    @location(4) normal: vec3<f32>,
}

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) light_uv: vec2<f32>,
    @location(3) vertex_light: vec4<f32>,
    @location(4) normal: vec3<f32>,
) -> VsOut {
    var output: VsOut;
    output.clip = uniforms.view_proj * vec4<f32>(position, 1.0);
    output.color = color;
    output.light_uv = light_uv;
    output.world = position;
    output.vertex_light = vertex_light;
    output.normal = normal;
    return output;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    if (input.light_uv.x < 0.0) {
        return vec4<f32>(input.color, 1.0);
    }
    let mapped = lit(input.light_uv);
    let irradiance = select(mapped, input.vertex_light.xyz, input.vertex_light.x >= 0.0);
    return vec4<f32>(with_figure(input.color, irradiance, input.light_uv, input.world, input.vertex_light.w, input.normal), 1.0);
}
"#
);

const TEXTURE_WGSL: &str = with_light!(
    r#"
@group(1) @binding(0) var base_tex: texture_2d<f32>;
@group(1) @binding(1) var base_samp: sampler;
@group(1) @binding(2) var second_tex: texture_2d<f32>;
@group(1) @binding(3) var detail_tex: texture_2d<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) blend: f32,
    @location(2) detail_scale: f32,
    @location(3) detail_factor: f32,
    @location(4) light_uv: vec2<f32>,
    @location(5) world: vec3<f32>,
    @location(6) vertex_light: vec4<f32>,
    @location(7) normal: vec3<f32>,
}

@vertex
fn vs(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) blend: f32,
    @location(3) detail: vec2<f32>,
    @location(4) light_uv: vec2<f32>,
    @location(5) vertex_light: vec4<f32>,
    @location(6) normal: vec3<f32>,
) -> VsOut {
    var output: VsOut;
    output.clip = uniforms.view_proj * vec4<f32>(position, 1.0);
    output.uv = uv;
    output.blend = blend;
    output.detail_scale = detail.x;
    output.detail_factor = detail.y;
    output.light_uv = light_uv;
    output.world = position;
    output.vertex_light = vertex_light;
    output.normal = normal;
    return output;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    let dims = textureDimensions(base_tex);
    let size = max(vec2<f32>(f32(dims.x), f32(dims.y)), vec2<f32>(1.0));
    let uv = input.uv / size;
    let primary = textureSample(base_tex, base_samp, uv).rgb;
    let secondary = textureSample(second_tex, base_samp, uv).rgb;
    var color = mix(primary, secondary, clamp(input.blend, 0.0, 1.0));
    let detail = textureSample(detail_tex, base_samp, uv * input.detail_scale).rgb;
    let modulated = color * detail * 2.0;
    color = mix(color, modulated, clamp(input.detail_factor, 0.0, 1.0));
    let mapped = lit(input.light_uv);
    let irradiance = select(mapped, input.vertex_light.xyz, input.vertex_light.x >= 0.0);
    return vec4<f32>(with_figure(color, irradiance, input.light_uv, input.world, input.vertex_light.w, input.normal), 1.0);
}
"#
);

const BLIT_WGSL: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

@vertex
fn vs(@builtin(vertex_index) index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let clip = vec4<f32>(positions[index], 0.0, 1.0);
    var output: VsOut;
    output.clip = clip;
    output.uv = vec2<f32>(clip.x, -clip.y) * 0.5 + vec2<f32>(0.5, 0.5);
    return output;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, input.uv);
}
"#;

struct ViewSlot {
    uniform: wgpu::Buffer,
    scene_bind: wgpu::BindGroup,
    color: wgpu::Texture,
    depth: wgpu::Texture,
    blit_bind: wgpu::BindGroup,
    size: (u32, u32),
}

struct Uploaded {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

#[repr(C, align(16))]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuLamp {
    center: [f32; 4],
    axis_x: [f32; 4],
    axis_y: [f32; 4],
    axis_z: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Frame {
    view_proj: [[f32; 4]; 4],
    fullbright: f32,
    unlit_x: f32,
    unlit_y: f32,
    lamp_count: u32,
    lamps: [GpuLamp; GLOW_CAP],
}

struct Gpu {
    scene_pipeline: wgpu::RenderPipeline,
    texture_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    repeat: wgpu::Sampler,
    positions: wgpu::Buffer,
    position_capacity: u64,
    colors: wgpu::Buffer,
    color_capacity: u64,
    plain_uvs: wgpu::Buffer,
    plain_uv_capacity: u64,
    plain_lights: wgpu::Buffer,
    plain_light_capacity: u64,
    plain_light_id: u64,
    plain_normals: wgpu::Buffer,
    plain_normal_capacity: u64,
    textured: wgpu::Buffer,
    textured_capacity: u64,
    tex_lights: wgpu::Buffer,
    tex_light_capacity: u64,
    tex_light_id: u64,
    tex_normals: wgpu::Buffer,
    tex_normal_capacity: u64,
    scene_id: u64,
    marker_positions: wgpu::Buffer,
    marker_position_capacity: u64,
    marker_position_id: u64,
    marker_colors: wgpu::Buffer,
    marker_color_capacity: u64,
    marker_color_id: u64,
    marker_uvs: wgpu::Buffer,
    marker_uv_capacity: u64,
    marker_uv_id: u64,
    marker_lights: wgpu::Buffer,
    marker_light_capacity: u64,
    marker_light_id: u64,
    marker_normals: wgpu::Buffer,
    marker_normal_capacity: u64,
    marker_normal_id: u64,
    lightmap: wgpu::Texture,
    lightmap_view: wgpu::TextureView,
    light_sampler: wgpu::Sampler,
    lightmap_size: u32,
    light_id: u64,
    images: Vec<Uploaded>,
    image_ptr: usize,
    white: wgpu::TextureView,
    gray: wgpu::TextureView,
    binds: HashMap<(u32, u32, u32), wgpu::BindGroup>,
    slots: Vec<ViewSlot>,
}

impl Gpu {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let scene_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("room-scene"),
            source: wgpu::ShaderSource::Wgsl(SCENE_WGSL.into()),
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("room-blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("room-uniforms"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("room-scene"),
            bind_group_layouts: &[&uniform_layout],
            push_constant_ranges: &[],
        });
        let scene_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("room-scene"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &scene_shader,
                entry_point: Some("vs"),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 3]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &POS_ATTR,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 3]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &COLOR_ATTR,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 2]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &LIGHT_UV_ATTR,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 4]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &PLAIN_LIGHT_ATTR,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 3]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &PLAIN_NORMAL_ATTR,
                    },
                ],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &scene_shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("room-blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("room-blit"),
            bind_group_layouts: &[&blit_layout],
            push_constant_ranges: &[],
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("room-blit"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("room-blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("room-repeat"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            // A floor toward the camera spans many texels per pixel. Sampling
            // past the first mip turns that wall into one flat color.
            lod_max_clamp: 0.0,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let texture_layout = texture_layout(device);
        let texture_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("room-texture"),
            source: wgpu::ShaderSource::Wgsl(TEXTURE_WGSL.into()),
        });
        let texture_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("room-texture"),
                bind_group_layouts: &[&uniform_layout, &texture_layout],
                push_constant_ranges: &[],
            });
        let texture_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("room-texture"),
            layout: Some(&texture_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &texture_shader,
                entry_point: Some("vs"),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<TexVert>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &TEX_ATTR,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 4]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &TEX_LIGHT_ATTR,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 3]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &TEX_NORMAL_ATTR,
                    },
                ],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &texture_shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        let positions = vertex_buffer(device, "room-positions", 64);
        let colors = vertex_buffer(device, "room-colors", 64);
        let plain_uvs = vertex_buffer(device, "room-plain-uvs", 64);
        let plain_lights = vertex_buffer(device, "room-plain-lights", 64);
        let plain_normals = vertex_buffer(device, "room-plain-normals", 64);
        let textured = vertex_buffer(device, "room-textured", 64);
        let tex_lights = vertex_buffer(device, "room-tex-lights", 64);
        let tex_normals = vertex_buffer(device, "room-tex-normals", 64);
        let marker_positions = vertex_buffer(device, "room-marker-positions", 64);
        let marker_colors = vertex_buffer(device, "room-marker-colors", 64);
        let marker_uvs = vertex_buffer(device, "room-marker-uvs", 64);
        let marker_lights = vertex_buffer(device, "room-marker-lights", 64);
        let marker_normals = vertex_buffer(device, "room-marker-normals", 64);
        let light_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("room-light"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let (lightmap, lightmap_view) = lightmap_texture(device, queue, 32);
        let white = solid_texture(device, queue, [255, 255, 255, 255]);
        let gray = solid_texture(device, queue, [128, 128, 128, 255]);
        Self {
            scene_pipeline,
            texture_pipeline,
            blit_pipeline,
            uniform_layout,
            texture_layout,
            blit_layout,
            sampler,
            repeat,
            positions,
            position_capacity: 64,
            colors,
            color_capacity: 64,
            plain_uvs,
            plain_uv_capacity: 64,
            plain_lights,
            plain_light_capacity: 64,
            plain_light_id: u64::MAX,
            plain_normals,
            plain_normal_capacity: 64,
            textured,
            textured_capacity: 64,
            tex_lights,
            tex_light_capacity: 64,
            tex_light_id: u64::MAX,
            tex_normals,
            tex_normal_capacity: 64,
            scene_id: u64::MAX,
            marker_positions,
            marker_position_capacity: 64,
            marker_position_id: u64::MAX,
            marker_colors,
            marker_color_capacity: 64,
            marker_color_id: u64::MAX,
            marker_uvs,
            marker_uv_capacity: 64,
            marker_uv_id: u64::MAX,
            marker_lights,
            marker_light_capacity: 64,
            marker_light_id: u64::MAX,
            marker_normals,
            marker_normal_capacity: 64,
            marker_normal_id: u64::MAX,
            lightmap,
            lightmap_view,
            light_sampler,
            lightmap_size: 32,
            light_id: u64::MAX,
            images: Vec::new(),
            image_ptr: 0,
            white,
            gray,
            binds: HashMap::new(),
            slots: Vec::new(),
        }
    }

    fn make_slot(&self, device: &wgpu::Device, width: u32, height: u32) -> ViewSlot {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("room-uniform"),
            size: std::mem::size_of::<Frame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scene_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("room-uniforms"),
            layout: &self.uniform_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.lightmap_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.light_sampler),
                },
            ],
        });
        let (color, depth, blit_bind) =
            allocate_targets(device, &self.blit_layout, &self.sampler, width, height);
        ViewSlot {
            uniform,
            scene_bind,
            color,
            depth,
            blit_bind,
            size: (width, height),
        }
    }

    fn ensure_slot(&mut self, device: &wgpu::Device, index: usize, width: u32, height: u32) {
        while self.slots.len() <= index {
            self.slots.push(self.make_slot(device, 1, 1));
        }
        if self.slots[index].size != (width, height) {
            let (color, depth, blit_bind) =
                allocate_targets(device, &self.blit_layout, &self.sampler, width, height);
            let slot = &mut self.slots[index];
            slot.color = color;
            slot.depth = depth;
            slot.blit_bind = blit_bind;
            slot.size = (width, height);
        }
    }

    fn material_bind(
        &mut self,
        device: &wgpu::Device,
        base: u32,
        second: u32,
        detail: u32,
    ) -> wgpu::BindGroup {
        let key = (base, second, detail);
        if let Some(bind) = self.binds.get(&key) {
            return bind.clone();
        }
        let bind = {
            let base_view = self
                .images
                .get(base as usize)
                .map(|image| &image.view)
                .unwrap_or(&self.white);
            let second_view = self
                .images
                .get(second as usize)
                .map(|image| &image.view)
                .unwrap_or(&self.white);
            let detail_view = self
                .images
                .get(detail as usize)
                .map(|image| &image.view)
                .unwrap_or(&self.gray);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("room-material"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(base_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.repeat),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(second_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(detail_view),
                    },
                ],
            })
        };
        self.binds.insert(key, bind.clone());
        bind
    }
}

fn lightmap_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let size = size.max(32);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("room-lightmap"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let blank = vec![0u16; (size as usize) * (size as usize) * 4];
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&blank),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 8),
            rows_per_image: Some(size),
        },
        texture.size(),
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn scene_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform: &wgpu::Buffer,
    light: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("room-uniforms"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(light),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn vertex_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn write_stream<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    stored_id: &mut u64,
    id: u64,
    label: &str,
    data: &[T],
) {
    if *stored_id == id {
        return;
    }
    let bytes = (data.len().max(1) * std::mem::size_of::<T>()) as u64;
    if bytes > *capacity {
        *buffer = vertex_buffer(device, label, bytes);
        *capacity = bytes;
    }
    if !data.is_empty() {
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(data));
    }
    *stored_id = id;
}

fn replace_stream<T: bytemuck::Pod>(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    label: &str,
    data: &[T],
) {
    replace_bytes(
        device,
        queue,
        buffer,
        capacity,
        label,
        bytemuck::cast_slice(data),
    );
}

fn replace_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    label: &str,
    data: &[u8],
) {
    let bytes = (data.len().max(4)) as u64;
    if bytes > *capacity {
        *buffer = vertex_buffer(device, label, bytes);
        *capacity = bytes;
    }
    if !data.is_empty() {
        queue.write_buffer(buffer, 0, data);
    }
}

fn texture_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("room-material"),
        entries: &[
            texture(0),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            texture(2),
            texture(3),
        ],
    })
}

fn solid_texture(device: &wgpu::Device, queue: &wgpu::Queue, pixel: [u8; 4]) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("room-solid"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixel,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: None,
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&Default::default())
}

fn upload_images(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    images: &[map::Image],
) -> Vec<Uploaded> {
    images
        .iter()
        .map(|image| upload_image(device, queue, image))
        .collect()
}

fn upload_image(device: &wgpu::Device, queue: &wgpu::Queue, image: &map::Image) -> Uploaded {
    let mips = image.mips.len().max(1) as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("room-map"),
        size: wgpu::Extent3d {
            width: image.width.max(1),
            height: image.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, pixels) in image.mips.iter().enumerate() {
        let width = (image.width >> level).max(1);
        let height = (image.height >> level).max(1);
        write_rgba(queue, &texture, level as u32, width, height, pixels);
    }
    let view = texture.create_view(&Default::default());
    Uploaded {
        _texture: texture,
        view,
    }
}

fn write_rgba(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mip: u32,
    width: u32,
    height: u32,
    pixels: &[u8],
) {
    let row = width.saturating_mul(4);
    let copy = wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: mip,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    };
    let size = wgpu::Extent3d {
        width,
        height: height.max(1),
        depth_or_array_layers: 1,
    };
    if height <= 1 {
        queue.write_texture(
            copy,
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: None,
                rows_per_image: None,
            },
            size,
        );
        return;
    }
    let stride = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let mut padded = Vec::with_capacity((stride * (height - 1) + row) as usize);
    for y in 0..height {
        let start = (y * row) as usize;
        let end = start + row as usize;
        if end > pixels.len() {
            return;
        }
        padded.extend_from_slice(&pixels[start..end]);
        if y + 1 != height {
            padded.resize(padded.len() + (stride - row) as usize, 0);
        }
    }
    queue.write_texture(
        copy,
        &padded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(height),
        },
        size,
    );
}

fn allocate_targets(
    device: &wgpu::Device,
    blit_layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::Texture, wgpu::BindGroup) {
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("room-color"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("room-depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = color.create_view(&Default::default());
    let blit_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("room-blit"),
        layout: blit_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    (color, depth, blit_bind)
}

pub fn init(cc: &eframe::CreationContext<'_>) {
    let state = cc.wgpu_render_state.as_ref().expect("eframe wgpu renderer");
    let gpu = Gpu::new(&state.device, &state.queue, state.target_format);
    state.renderer.write().callback_resources.insert(gpu);
}

impl egui_wgpu::CallbackTrait for RoomCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let gpu = resources.get_mut::<Gpu>().expect("room gpu resources");
        let width = self.width.max(1);
        let height = self.height.max(1);
        gpu.ensure_slot(device, self.slot, width, height);
        if gpu.scene_id != self.scene_id {
            replace_stream(
                device,
                queue,
                &mut gpu.positions,
                &mut gpu.position_capacity,
                "room-positions",
                &self.scene.plain_positions,
            );
            replace_stream(
                device,
                queue,
                &mut gpu.colors,
                &mut gpu.color_capacity,
                "room-colors",
                &self.scene.plain_colors,
            );
            replace_stream(
                device,
                queue,
                &mut gpu.plain_uvs,
                &mut gpu.plain_uv_capacity,
                "room-plain-uvs",
                &self.scene.plain_uvs,
            );
            replace_stream(
                device,
                queue,
                &mut gpu.plain_normals,
                &mut gpu.plain_normal_capacity,
                "room-plain-normals",
                &self.scene.plain_normals,
            );
            replace_bytes(
                device,
                queue,
                &mut gpu.textured,
                &mut gpu.textured_capacity,
                "room-textured",
                bytemuck::cast_slice(&self.scene.verts),
            );
            replace_stream(
                device,
                queue,
                &mut gpu.tex_normals,
                &mut gpu.tex_normal_capacity,
                "room-tex-normals",
                &self.scene.tex_normals,
            );
            let image_ptr = Arc::as_ptr(&self.scene.images) as usize;
            if gpu.image_ptr != image_ptr {
                gpu.binds.clear();
                gpu.images = upload_images(device, queue, &self.scene.images);
                gpu.image_ptr = image_ptr;
            }
            gpu.scene_id = self.scene_id;
        }
        let plain = self
            .scene
            .plain_positions
            .len()
            .min(self.scene.plain_colors.len())
            .min(self.scene.plain_uvs.len())
            .min(self.scene.plain_normals.len())
            .min(self.plain_lights.len()) as u32;
        let markers = self.marker_positions.len().min(self.marker_colors.len()) as u32;
        write_stream(
            device,
            queue,
            &mut gpu.marker_positions,
            &mut gpu.marker_position_capacity,
            &mut gpu.marker_position_id,
            self.marker_id,
            "room-marker-positions",
            &self.marker_positions,
        );
        write_stream(
            device,
            queue,
            &mut gpu.marker_colors,
            &mut gpu.marker_color_capacity,
            &mut gpu.marker_color_id,
            self.marker_id,
            "room-marker-colors",
            &self.marker_colors,
        );
        if gpu.marker_uv_id != self.marker_id {
            let marker_uvs = vec![[-1.0_f32, -1.0]; self.marker_positions.len()];
            write_stream(
                device,
                queue,
                &mut gpu.marker_uvs,
                &mut gpu.marker_uv_capacity,
                &mut gpu.marker_uv_id,
                self.marker_id,
                "room-marker-uvs",
                &marker_uvs,
            );
        }
        if gpu.marker_light_id != self.marker_id {
            let marker_lights = vec![[-1.0_f32, -1.0, -1.0, 0.0]; self.marker_positions.len()];
            write_stream(
                device,
                queue,
                &mut gpu.marker_lights,
                &mut gpu.marker_light_capacity,
                &mut gpu.marker_light_id,
                self.marker_id,
                "room-marker-lights",
                &marker_lights,
            );
        }
        if gpu.marker_normal_id != self.marker_id {
            let marker_normals = vec![[0.0_f32, 0.0, 1.0]; self.marker_positions.len()];
            write_stream(
                device,
                queue,
                &mut gpu.marker_normals,
                &mut gpu.marker_normal_capacity,
                &mut gpu.marker_normal_id,
                self.marker_id,
                "room-marker-normals",
                &marker_normals,
            );
        }
        if gpu.lightmap_size != self.lightmap_size {
            let size = self.lightmap_size.max(32);
            let (texture, view) = lightmap_texture(device, queue, size);
            let binds: Vec<_> = gpu
                .slots
                .iter()
                .map(|slot| {
                    scene_bind(
                        device,
                        &gpu.uniform_layout,
                        &slot.uniform,
                        &view,
                        &gpu.light_sampler,
                    )
                })
                .collect();
            for (slot, bind) in gpu.slots.iter_mut().zip(binds) {
                slot.scene_bind = bind;
            }
            gpu.lightmap = texture;
            gpu.lightmap_view = view;
            gpu.lightmap_size = size;
            gpu.light_id = u64::MAX;
        }
        if gpu.light_id != self.light_id {
            let size = gpu.lightmap_size;
            let expected = (size as usize)
                .saturating_mul(size as usize)
                .saturating_mul(4);
            if self.lightmap.len() == expected {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &gpu.lightmap,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&self.lightmap),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size * 8),
                        rows_per_image: Some(size),
                    },
                    wgpu::Extent3d {
                        width: size,
                        height: size,
                        depth_or_array_layers: 1,
                    },
                );
            }
            write_stream(
                device,
                queue,
                &mut gpu.plain_lights,
                &mut gpu.plain_light_capacity,
                &mut gpu.plain_light_id,
                self.light_id,
                "room-plain-lights",
                &self.plain_lights,
            );
            write_stream(
                device,
                queue,
                &mut gpu.tex_lights,
                &mut gpu.tex_light_capacity,
                &mut gpu.tex_light_id,
                self.light_id,
                "room-tex-lights",
                &self.tex_lights,
            );
            gpu.light_id = self.light_id;
        }
        let mut texture_binds = Vec::with_capacity(self.scene.draws.len());
        for draw in &self.scene.draws {
            texture_binds.push(gpu.material_bind(device, draw.base, draw.second, draw.detail));
        }
        let slot = &gpu.slots[self.slot];
        let size = self.lightmap_size.max(1) as f32;
        let mut lamps = [bytemuck::Zeroable::zeroed(); GLOW_CAP];
        let count = self.glow.len().min(GLOW_CAP);
        for (index, glow) in self.glow.iter().take(count).enumerate() {
            lamps[index] = GpuLamp {
                center: [glow.center.x, glow.center.y, glow.center.z, glow.inside],
                axis_x: [glow.axis_x.x, glow.axis_x.y, glow.axis_x.z, glow.mesh],
                axis_y: [glow.axis_y.x, glow.axis_y.y, glow.axis_y.z, glow.spread],
                axis_z: [glow.axis_z.x, glow.axis_z.y, glow.axis_z.z, glow.rim],
                color: [glow.color.x, glow.color.y, glow.color.z, glow.intensity],
            };
        }
        let frame = Frame {
            view_proj: self.view_proj,
            fullbright: if self.map_light { 1.0 } else { 0.0 },
            unlit_x: 0.5 / size,
            unlit_y: 0.5 / size,
            lamp_count: count as u32,
            lamps,
        };
        queue.write_buffer(&slot.uniform, 0, bytemuck::bytes_of(&frame));
        let scene_bind = slot.scene_bind.clone();
        let color_view = slot.color.create_view(&Default::default());
        let depth_view = slot.depth.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("room"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("room"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.12,
                            g: 0.12,
                            b: 0.13,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            if self.scene.verts.len() == self.tex_lights.len()
                && self.scene.verts.len() == self.scene.tex_normals.len()
                && !self.scene.verts.is_empty()
            {
                pass.set_pipeline(&gpu.texture_pipeline);
                pass.set_bind_group(0, &scene_bind, &[]);
                pass.set_vertex_buffer(0, gpu.textured.slice(..));
                pass.set_vertex_buffer(1, gpu.tex_lights.slice(..));
                pass.set_vertex_buffer(2, gpu.tex_normals.slice(..));
                for (draw, bind) in self.scene.draws.iter().zip(&texture_binds) {
                    pass.set_bind_group(1, bind, &[]);
                    pass.draw(draw.start..draw.start + draw.count, 0..1);
                }
            }
            pass.set_pipeline(&gpu.scene_pipeline);
            pass.set_bind_group(0, &scene_bind, &[]);
            if plain > 0 {
                pass.set_vertex_buffer(0, gpu.positions.slice(..));
                pass.set_vertex_buffer(1, gpu.colors.slice(..));
                pass.set_vertex_buffer(2, gpu.plain_uvs.slice(..));
                pass.set_vertex_buffer(3, gpu.plain_lights.slice(..));
                pass.set_vertex_buffer(4, gpu.plain_normals.slice(..));
                pass.draw(0..plain, 0..1);
            }
            if markers > 0 {
                pass.set_vertex_buffer(0, gpu.marker_positions.slice(..));
                pass.set_vertex_buffer(1, gpu.marker_colors.slice(..));
                pass.set_vertex_buffer(2, gpu.marker_uvs.slice(..));
                pass.set_vertex_buffer(3, gpu.marker_lights.slice(..));
                pass.set_vertex_buffer(4, gpu.marker_normals.slice(..));
                pass.draw(0..markers, 0..1);
            }
        }
        vec![encoder.finish()]
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let gpu = resources.get::<Gpu>().expect("room gpu resources");
        let Some(slot) = gpu.slots.get(self.slot) else {
            return;
        };
        let viewport = info.viewport_in_pixels();
        if viewport.width_px <= 0 || viewport.height_px <= 0 {
            return;
        }
        render_pass.set_viewport(
            viewport.left_px as f32,
            viewport.top_px as f32,
            viewport.width_px as f32,
            viewport.height_px as f32,
            0.0,
            1.0,
        );
        render_pass.set_pipeline(&gpu.blit_pipeline);
        render_pass.set_bind_group(0, &slot.blit_bind, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod shader {
    use super::{Frame, GpuLamp, GLOW_CAP, SCENE_WGSL, TEXTURE_WGSL};

    fn valid(source: &str) {
        let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|err| panic!("{err:?}"));
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .unwrap_or_else(|err| panic!("{err:?}"));
    }

    #[test]
    fn room_shaders_compile() {
        valid(SCENE_WGSL);
        valid(TEXTURE_WGSL);
    }

    #[test]
    fn the_lamp_block_lines_up_with_the_shader() {
        assert_eq!(GLOW_CAP, 64);
        assert_eq!(std::mem::size_of::<GpuLamp>(), 80);
        assert_eq!(std::mem::align_of::<GpuLamp>(), 16);
        let lamps = std::mem::offset_of!(Frame, lamps);
        assert_eq!(lamps, 80);
        assert_eq!(std::mem::size_of::<Frame>(), 80 + 64 * 80);
    }
}
