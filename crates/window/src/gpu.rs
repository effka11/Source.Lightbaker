use std::sync::Arc;

use eframe::egui;
use eframe::egui_wgpu::{self, wgpu};

const POS_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
const COLOR_ATTR: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x3];

pub struct RoomCallback {
    pub positions: Arc<Vec<[f32; 3]>>,
    pub position_id: u64,
    pub colors: Arc<Vec<[f32; 3]>>,
    pub color_id: u64,
    pub marker_positions: Arc<Vec<[f32; 3]>>,
    pub marker_colors: Arc<Vec<[f32; 3]>>,
    pub marker_id: u64,
    pub view_proj: [[f32; 4]; 4],
    pub width: u32,
    pub height: u32,
    pub slot: usize,
}

const SCENE_WGSL: &str = r#"
struct Uniforms {
    view_proj: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) color: vec3<f32>) -> VsOut {
    var output: VsOut;
    output.clip = uniforms.view_proj * vec4<f32>(position, 1.0);
    output.color = color;
    return output;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(input.color, 1.0);
}
"#;

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

struct Gpu {
    scene_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    uniform_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    positions: wgpu::Buffer,
    position_capacity: u64,
    position_id: u64,
    colors: wgpu::Buffer,
    color_capacity: u64,
    color_id: u64,
    marker_positions: wgpu::Buffer,
    marker_position_capacity: u64,
    marker_position_id: u64,
    marker_colors: wgpu::Buffer,
    marker_color_capacity: u64,
    marker_color_id: u64,
    slots: Vec<ViewSlot>,
}

impl Gpu {
    fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
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
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
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
        let positions = vertex_buffer(device, "room-positions", 64);
        let colors = vertex_buffer(device, "room-colors", 64);
        let marker_positions = vertex_buffer(device, "room-marker-positions", 64);
        let marker_colors = vertex_buffer(device, "room-marker-colors", 64);
        Self {
            scene_pipeline,
            blit_pipeline,
            uniform_layout,
            blit_layout,
            sampler,
            positions,
            position_capacity: 64,
            position_id: u64::MAX,
            colors,
            color_capacity: 64,
            color_id: u64::MAX,
            marker_positions,
            marker_position_capacity: 64,
            marker_position_id: u64::MAX,
            marker_colors,
            marker_color_capacity: 64,
            marker_color_id: u64::MAX,
            slots: Vec::new(),
        }
    }

    fn make_slot(&self, device: &wgpu::Device, width: u32, height: u32) -> ViewSlot {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("room-uniform"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scene_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("room-uniforms"),
            layout: &self.uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
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
}

fn vertex_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn write_stream(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    stored_id: &mut u64,
    id: u64,
    label: &str,
    data: &[[f32; 3]],
) {
    if *stored_id == id {
        return;
    }
    let bytes = (data.len().max(1) * std::mem::size_of::<[f32; 3]>()) as u64;
    if bytes > *capacity {
        *buffer = vertex_buffer(device, label, bytes);
        *capacity = bytes;
    }
    if !data.is_empty() {
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(data));
    }
    *stored_id = id;
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
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
    let gpu = Gpu::new(&state.device, state.target_format);
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
        write_stream(
            device,
            queue,
            &mut gpu.positions,
            &mut gpu.position_capacity,
            &mut gpu.position_id,
            self.position_id,
            "room-positions",
            &self.positions,
        );
        write_stream(
            device,
            queue,
            &mut gpu.colors,
            &mut gpu.color_capacity,
            &mut gpu.color_id,
            self.color_id,
            "room-colors",
            &self.colors,
        );
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
        let luxels = self.positions.len().min(self.colors.len()) as u32;
        let markers = self.marker_positions.len().min(self.marker_colors.len()) as u32;
        let slot = &gpu.slots[self.slot];
        queue.write_buffer(&slot.uniform, 0, bytemuck::cast_slice(&self.view_proj));
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
            pass.set_pipeline(&gpu.scene_pipeline);
            pass.set_bind_group(0, &scene_bind, &[]);
            if luxels > 0 {
                pass.set_vertex_buffer(0, gpu.positions.slice(..));
                pass.set_vertex_buffer(1, gpu.colors.slice(..));
                pass.draw(0..luxels, 0..1);
            }
            if markers > 0 {
                pass.set_vertex_buffer(0, gpu.marker_positions.slice(..));
                pass.set_vertex_buffer(1, gpu.marker_colors.slice(..));
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
