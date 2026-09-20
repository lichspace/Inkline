use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use bytemuck::{Pod, Zeroable};
use egui::PaintCallbackInfo;
use egui_wgpu::CallbackTrait;
use wgpu::util::DeviceExt as _;

use crate::geometry::{tessellate_stroke, GpuVertex};
use crate::model::{BlendMode, Document, LayerId};

const LAYER_TEXTURE_SCALE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct SolidUniform {
    canvas_size: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CompositeUniform {
    opacity: f32,
    _pad: [f32; 7],
}

struct LayerGpu {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    opacity_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    opacity: f32,
}

pub struct GpuRenderState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    solid_pipeline: wgpu::RenderPipeline,
    solid_bind_group: wgpu::BindGroup,
    solid_uniform_buffer: wgpu::Buffer,
    composite_bind_group_layout: wgpu::BindGroupLayout,
    composite_pipelines: HashMap<BlendMode, wgpu::RenderPipeline>,
    sampler: wgpu::Sampler,
    layers: HashMap<LayerId, LayerGpu>,
    canvas_width: f32,
    canvas_height: f32,
    rendered_revision: u64,
}

impl GpuRenderState {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../shaders/solid.wgsl"));
        let composite_shader =
            device.create_shader_module(wgpu::include_wgsl!("../shaders/composite.wgsl"));

        let solid_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("solid bind group layout"),
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

        let composite_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("composite bind group layout"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let solid_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("solid pipeline layout"),
                bind_group_layouts: &[Some(&solid_bind_group_layout)],
                immediate_size: 0,
            });

        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("composite pipeline layout"),
                bind_group_layouts: &[Some(&composite_bind_group_layout)],
                immediate_size: 0,
            });

        let solid_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("solid stroke pipeline"),
            layout: Some(&solid_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let solid_uniform = SolidUniform {
            canvas_size: [1.0, 1.0],
            _pad: [0.0, 0.0],
        };
        let solid_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("solid uniform"),
            contents: bytemuck::cast_slice(&[solid_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let solid_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("solid bind group"),
            layout: &solid_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: solid_uniform_buffer.as_entire_binding(),
            }],
        });

        let solid_uniform_buffer = solid_uniform_buffer;

        let mut composite_pipelines = HashMap::new();
        for blend_mode in BlendMode::ALL {
            let blend = blend_state(blend_mode);
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("composite pipeline"),
                layout: Some(&composite_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &composite_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: 1,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &composite_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            composite_pipelines.insert(blend_mode, pipeline);
        }

        Self {
            device,
            queue,
            solid_pipeline,
            solid_bind_group,
            solid_uniform_buffer,
            composite_bind_group_layout,
            composite_pipelines,
            sampler,
            layers: HashMap::new(),
            canvas_width: 0.0,
            canvas_height: 0.0,
            rendered_revision: 0,
        }
    }

    fn prepare(
        &mut self,
        document: &Document,
        dirty_layers: &HashSet<LayerId>,
        revision: u64,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let canvas_changed = (self.canvas_width - document.width).abs() > f32::EPSILON
            || (self.canvas_height - document.height).abs() > f32::EPSILON;
        let revision_changed = self.rendered_revision != revision;

        if canvas_changed {
            self.canvas_width = document.width;
            self.canvas_height = document.height;
            let uniform = SolidUniform {
                canvas_size: [document.width, document.height],
                _pad: [0.0, 0.0],
            };
            self.queue.write_buffer(
                &self.solid_uniform_buffer,
                0,
                bytemuck::cast_slice(&[uniform]),
            );
            let layer_ids: Vec<LayerId> = self.layers.keys().copied().collect();
            for layer_id in layer_ids {
                if let Some(layer) = self.layers.get_mut(&layer_id) {
                    recreate_layer_texture(
                        &self.device,
                        &self.composite_bind_group_layout,
                        &self.sampler,
                        layer,
                        document.width,
                        document.height,
                    );
                }
            }
        }

        let document_ids: HashSet<LayerId> = document.layers.iter().map(|layer| layer.id).collect();
        self.layers
            .retain(|layer_id, _| document_ids.contains(layer_id));

        for layer in &document.layers {
            if !self.layers.contains_key(&layer.id) {
                let texture = create_layer_texture(&self.device, document.width, document.height);
                let view = texture.create_view(&Default::default());
                let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("empty layer vertex buffer"),
                    size: 4,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let index_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("empty layer index buffer"),
                    size: 4,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let opacity_buffer = create_opacity_buffer(&self.device, layer.opacity);
                let bind_group = create_composite_bind_group(
                    &self.device,
                    &self.composite_bind_group_layout,
                    &view,
                    &self.sampler,
                    &opacity_buffer,
                );
                self.layers.insert(
                    layer.id,
                    LayerGpu {
                        texture,
                        view,
                        vertex_buffer,
                        index_buffer,
                        index_count: 0,
                        opacity_buffer,
                        bind_group,
                        opacity: layer.opacity,
                    },
                );
            }

            let dirty = canvas_changed
                || (revision_changed && dirty_layers.is_empty())
                || dirty_layers.contains(&layer.id);

            if let Some(layer_gpu) = self.layers.get_mut(&layer.id) {
                if (layer_gpu.opacity - layer.opacity).abs() > f32::EPSILON {
                    self.queue.write_buffer(
                        &layer_gpu.opacity_buffer,
                        0,
                        bytemuck::cast_slice(&[CompositeUniform {
                            opacity: layer.opacity,
                            _pad: [0.0; 7],
                        }]),
                    );
                    layer_gpu.bind_group = create_composite_bind_group(
                        &self.device,
                        &self.composite_bind_group_layout,
                        &layer_gpu.view,
                        &self.sampler,
                        &layer_gpu.opacity_buffer,
                    );
                    layer_gpu.opacity = layer.opacity;
                }

                if dirty {
                    update_layer_geometry(&self.device, &self.queue, layer_gpu, &layer.strokes);
                    render_layer_offscreen(
                        &self.solid_pipeline,
                        &self.solid_bind_group,
                        encoder,
                        layer_gpu,
                    );
                }
            }
        }

        self.rendered_revision = revision;
    }
}

fn render_layer_offscreen(
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    encoder: &mut wgpu::CommandEncoder,
    layer: &LayerGpu,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("layer offscreen pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &layer.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.set_vertex_buffer(0, layer.vertex_buffer.slice(..));
    pass.set_index_buffer(layer.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    if layer.index_count > 0 {
        pass.draw_indexed(0..layer.index_count, 0, 0..1);
    }
}

fn recreate_layer_texture(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    layer: &mut LayerGpu,
    width: f32,
    height: f32,
) {
    let texture = create_layer_texture(device, width, height);
    let view = texture.create_view(&Default::default());
    layer.texture = texture;
    layer.view = view;
    layer.bind_group =
        create_composite_bind_group(device, layout, &layer.view, sampler, &layer.opacity_buffer);
}

pub struct CanvasCallback {
    pub document: Arc<RwLock<Document>>,
    pub render_state: Arc<Mutex<GpuRenderState>>,
    pub dirty_layers: HashSet<LayerId>,
    pub revision: u64,
}

impl CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        _callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Ok(mut state) = self.render_state.lock() {
            if let Ok(document) = self.document.read() {
                state.prepare(&document, &self.dirty_layers, self.revision, encoder);
            }
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Ok(state) = self.render_state.lock() else {
            return;
        };

        let Ok(document) = self.document.read() else {
            return;
        };

        for layer in &document.layers {
            if !layer.visible {
                continue;
            }
            let Some(layer_gpu) = state.layers.get(&layer.id) else {
                continue;
            };
            let Some(pipeline) = state.composite_pipelines.get(&layer.blend_mode) else {
                continue;
            };
            render_pass.set_pipeline(pipeline);
            render_pass.set_bind_group(0, &layer_gpu.bind_group, &[]);
            render_pass.draw(0..4, 0..1);
        }
    }
}

fn create_layer_texture(device: &wgpu::Device, width: f32, height: f32) -> wgpu::Texture {
    let width = ((width * LAYER_TEXTURE_SCALE as f32).max(1.0) as u32).max(1);
    let height = ((height * LAYER_TEXTURE_SCALE as f32).max(1.0) as u32).max(1);
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("layer texture"),
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
    })
}

fn create_opacity_buffer(device: &wgpu::Device, opacity: f32) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("composite uniform"),
        contents: bytemuck::cast_slice(&[CompositeUniform {
            opacity,
            _pad: [0.0; 7],
        }]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

fn create_composite_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    opacity_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("composite bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: opacity_buffer.as_entire_binding(),
            },
        ],
    })
}

fn update_layer_geometry(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layer: &mut LayerGpu,
    strokes: &[crate::model::Stroke],
) {
    let mut geometry = crate::geometry::GpuGeometry::default();
    for stroke in strokes {
        geometry.append(tessellate_stroke(stroke));
    }

    layer.index_count = geometry.indices.len() as u32;

    if !geometry.vertices.is_empty() {
        ensure_buffer_size(
            device,
            &mut layer.vertex_buffer,
            geometry.vertices.len() * std::mem::size_of::<GpuVertex>(),
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            "layer vertex buffer",
        );
        queue.write_buffer(
            &layer.vertex_buffer,
            0,
            bytemuck::cast_slice(&geometry.vertices),
        );
    }
    if !geometry.indices.is_empty() {
        ensure_buffer_size(
            device,
            &mut layer.index_buffer,
            geometry.indices.len() * std::mem::size_of::<u32>(),
            wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            "layer index buffer",
        );
        queue.write_buffer(
            &layer.index_buffer,
            0,
            bytemuck::cast_slice(&geometry.indices),
        );
    }
}

fn ensure_buffer_size(
    device: &wgpu::Device,
    buffer: &mut wgpu::Buffer,
    size: usize,
    usage: wgpu::BufferUsages,
    label: &'static str,
) {
    if buffer.size() < size.max(4) as u64 {
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size.max(4) as u64,
            usage,
            mapped_at_creation: false,
        });
    }
}

fn blend_state(blend_mode: BlendMode) -> wgpu::BlendState {
    let alpha = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    };

    let color = match blend_mode {
        BlendMode::Normal => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        BlendMode::Multiply => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Dst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
        BlendMode::Screen => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        BlendMode::Add => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        BlendMode::Subtract => wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Subtract,
        },
    };

    wgpu::BlendState { color, alpha }
}

struct ExportLayer {
    _texture: wgpu::Texture,
    _vertex_buffer: wgpu::Buffer,
    _index_buffer: wgpu::Buffer,
    _index_count: u32,
    bind_group: wgpu::BindGroup,
    blend_mode: BlendMode,
}

fn wgpu_color(color: [f32; 4]) -> wgpu::Color {
    wgpu::Color {
        r: color[0].clamp(0.0, 1.0) as f64,
        g: color[1].clamp(0.0, 1.0) as f64,
        b: color[2].clamp(0.0, 1.0) as f64,
        a: color[3].clamp(0.0, 1.0) as f64,
    }
}

pub fn render_document_to_rgba(
    state: &mut GpuRenderState,
    document: &Document,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let width = width.max(1);
    let height = height.max(1);

    let final_texture = state.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("raster export texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let final_view = final_texture.create_view(&Default::default());

    let composite_shader = state
        .device
        .create_shader_module(wgpu::include_wgsl!("../shaders/composite.wgsl"));
    let composite_layout = state
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("raster export composite layout"),
            bind_group_layouts: &[Some(&state.composite_bind_group_layout)],
            immediate_size: 0,
        });
    let composite_pipelines: HashMap<BlendMode, wgpu::RenderPipeline> = BlendMode::ALL
        .into_iter()
        .map(|mode| {
            let blend = blend_state(mode);
            let pipeline = state
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("raster export composite pipeline"),
                    layout: Some(&composite_layout),
                    vertex: wgpu::VertexState {
                        module: &composite_shader,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleStrip,
                        strip_index_format: None,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: None,
                        unclipped_depth: false,
                        polygon_mode: wgpu::PolygonMode::Fill,
                        conservative: false,
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState {
                        count: 1,
                        mask: !0,
                        alpha_to_coverage_enabled: false,
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &composite_shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8UnormSrgb,
                            blend: Some(blend),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
            (mode, pipeline)
        })
        .collect();

    let mut encoder = state
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("raster export encoder"),
        });

    let mut export_layers = Vec::new();
    for layer in &document.layers {
        if !layer.visible {
            continue;
        }

        let texture = create_export_layer_texture(&state.device, width, height);
        let view = texture.create_view(&Default::default());
        let mut geometry = crate::geometry::GpuGeometry::default();
        for stroke in &layer.strokes {
            geometry.append(tessellate_stroke(stroke));
        }

        let vertex_buffer = if geometry.vertices.is_empty() {
            state.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("raster export vertex buffer"),
                size: 4,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        } else {
            state
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("raster export vertex buffer"),
                    contents: bytemuck::cast_slice(&geometry.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        };
        let index_buffer = if geometry.indices.is_empty() {
            state.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("raster export index buffer"),
                size: 4,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        } else {
            state
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("raster export index buffer"),
                    contents: bytemuck::cast_slice(&geometry.indices),
                    usage: wgpu::BufferUsages::INDEX,
                })
        };
        let index_count = geometry.indices.len() as u32;

        let uniform = SolidUniform {
            canvas_size: [document.width, document.height],
            _pad: [0.0, 0.0],
        };
        let solid_uniform_buffer =
            state
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("raster export solid uniform"),
                    contents: bytemuck::cast_slice(&[uniform]),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
        let solid_layout = state.solid_pipeline.get_bind_group_layout(0);
        let solid_bind_group = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("raster export solid bind group"),
            layout: &solid_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: solid_uniform_buffer.as_entire_binding(),
            }],
        });

        let opacity_buffer = create_opacity_buffer(&state.device, layer.opacity);
        let composite_bind_group = create_composite_bind_group(
            &state.device,
            &state.composite_bind_group_layout,
            &view,
            &state.sampler,
            &opacity_buffer,
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("raster export layer pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&state.solid_pipeline);
            pass.set_bind_group(0, &solid_bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            if index_count > 0 {
                pass.draw_indexed(0..index_count, 0, 0..1);
            }
        }

        export_layers.push(ExportLayer {
            _texture: texture,
            _vertex_buffer: vertex_buffer,
            _index_buffer: index_buffer,
            _index_count: index_count,
            bind_group: composite_bind_group,
            blend_mode: layer.blend_mode,
        });
    }

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("raster export composite pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &final_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu_color(document.background_color)),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        for layer in &export_layers {
            if let Some(pipeline) = composite_pipelines.get(&layer.blend_mode) {
                pass.set_pipeline(pipeline);
            }
            pass.set_bind_group(0, &layer.bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
    }

    let bytes_per_pixel = 4u32;
    let padded_bytes_per_row = bytes_per_pixel
        .checked_mul(width)
        .ok_or_else(|| "raster width overflow".to_owned())?
        .next_multiple_of(256);
    let readback_size = padded_bytes_per_row as u64 * height as u64;
    let readback_buffer = state.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("raster export readback buffer"),
        size: readback_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &final_texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let command_buffer = encoder.finish();
    state.queue.submit(Some(command_buffer));

    let slice = readback_buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    state.device.poll(wgpu::PollType::wait_indefinitely())?;
    receiver.recv()??;

    let mapped = slice.get_mapped_range();
    let mut rgba = Vec::with_capacity((width * height * bytes_per_pixel) as usize);
    for row in 0..height {
        let start = (row * padded_bytes_per_row) as usize;
        let end = start + (width * bytes_per_pixel) as usize;
        rgba.extend_from_slice(&mapped[start..end]);
    }
    drop(mapped);
    readback_buffer.unmap();

    Ok(rgba)
}

fn create_export_layer_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("raster export layer texture"),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Stroke, StrokePoint};

    #[test]
    #[ignore = "requires an available GPU adapter"]
    fn raster_export_smoke() {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("no GPU adapter available");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .expect("GPU device request failed");

            let mut state = GpuRenderState::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb);
            let mut document = Document::new(128.0, 128.0);
            document.layer_mut(1).unwrap().strokes.push(Stroke {
                id: 1,
                points: vec![
                    StrokePoint::new(16.0, 16.0, 1.0),
                    StrokePoint::new(112.0, 112.0, 0.5),
                ],
                color: [1.0, 0.0, 0.0, 1.0],
                width_scale: 1.0,
            });

            let pixels = render_document_to_rgba(&mut state, &document, 64, 64).unwrap();

            assert_eq!(pixels.len(), 64 * 64 * 4);
            assert!(pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
        });
    }
}
