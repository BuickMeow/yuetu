//! Instanced wgpu renderer for piano-roll notes.
//!
//! The instance buffer is rebuilt only when a new `ConversionResult` arrives.
//! Pan/zoom changes only update a small uniform; note geometry stays on the
//! GPU and is drawn in one instanced call.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytemuck::{Pod, Zeroable};
use eframe::egui::{self, PaintCallbackInfo};
use eframe::egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use yuetu_core::{Color, ConversionResult};

static NEXT_DATA_ID: AtomicU64 = AtomicU64::new(1);

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct MidiNoteInstance {
    column: f32,
    start: f32,
    end: f32,
    _pad: f32,
    color: [f32; 4],
}

pub struct MidiGpuData {
    pub id: u64,
    instances: Vec<MidiNoteInstance>,
}

impl MidiGpuData {
    pub fn from_result(result: &ConversionResult) -> Self {
        let mut key_columns = [None; 128];
        for column in 0..result.keymap.width() {
            if let Some(key) = result.keymap.key_at(column) {
                key_columns[key as usize] = Some(column as f32);
            }
        }

        let mut instances = Vec::with_capacity(result.notes.len());
        for note in &result.notes {
            let Some(column) = key_columns[note.key as usize] else {
                continue;
            };
            let color = result
                .palette
                .colors
                .get(note.track as usize)
                .copied()
                .unwrap_or(Color::WHITE);
            instances.push(MidiNoteInstance {
                column,
                start: note.start as f32,
                end: note.end as f32,
                _pad: 0.0,
                color: [
                    f32::from(color.r) / 255.0,
                    f32::from(color.g) / 255.0,
                    f32::from(color.b) / 255.0,
                    f32::from(color.a) / 255.0,
                ],
            });
        }

        Self {
            id: NEXT_DATA_ID.fetch_add(1, Ordering::Relaxed),
            instances,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct MidiUniform {
    viewport_min: [f32; 2],
    viewport_size: [f32; 2],
    content_left: f32,
    content_bottom: f32,
    scale_x: f32,
    scale_y: f32,
    horizontal: f32,
    _pad: [f32; 3],
}

pub struct MidiGpuCallback {
    data: Arc<MidiGpuData>,
    uniform: MidiUniform,
    target_format: wgpu::TextureFormat,
}

impl MidiGpuCallback {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        data: Arc<MidiGpuData>,
        viewport: egui::Rect,
        content: egui::Rect,
        scale_x: f32,
        scale_y: f32,
        horizontal: bool,
        target_format: wgpu::TextureFormat,
        pixels_per_point: f32,
    ) -> Self {
        // Keep the shader in physical pixels so note edges can be snapped to
        // the real framebuffer grid, independent of the display scale.
        let dpr = pixels_per_point.max(0.1);
        Self {
            data,
            uniform: MidiUniform {
                viewport_min: [viewport.min.x * dpr, viewport.min.y * dpr],
                viewport_size: [
                    viewport.width().max(1.0) * dpr,
                    viewport.height().max(1.0) * dpr,
                ],
                content_left: content.left() * dpr,
                content_bottom: content.bottom() * dpr,
                scale_x: scale_x * dpr,
                scale_y: scale_y * dpr,
                horizontal: if horizontal { 1.0 } else { 0.0 },
                _pad: [0.0; 3],
            },
            target_format,
        }
    }
}

impl CallbackTrait for MidiGpuCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources = callback_resources
            .entry::<MidiGpuResources>()
            .or_insert_with(|| MidiGpuResources::new(device, self.target_format));
        resources.prepare(device, queue, &self.data, &self.uniform);
        Vec::new()
    }

    fn paint(
        &self,
        _info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        if self.data.instances.is_empty() {
            return;
        }
        let Some(resources) = callback_resources.get::<MidiGpuResources>() else {
            return;
        };

        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &resources.bind_group, &[]);
        render_pass.set_vertex_buffer(0, resources.instance_buffer.slice(..));
        render_pass.draw(0..6, 0..self.data.instances.len() as u32);
    }
}

struct MidiGpuResources {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    uploaded_id: Option<u64>,
}

impl MidiGpuResources {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("yuetu midi notes shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("midi_gpu.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuetu midi notes bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<MidiUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yuetu midi notes pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        const ATTRS: [wgpu::VertexAttribute; 4] = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 4,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 8,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 3,
            },
        ];
        let vertex_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MidiNoteInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &ATTRS,
        })];

        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let targets = [Some(wgpu::ColorTargetState {
            format: target_format,
            blend: Some(blend),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let fragment_entry = if target_format.is_srgb() {
            "fs_main_srgb"
        } else {
            "fs_main_gamma"
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yuetu midi notes pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &vertex_buffers,
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fragment_entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniform = MidiUniform::zeroed();
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("yuetu midi notes uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuetu midi notes bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let instance_capacity = std::mem::size_of::<MidiNoteInstance>() as u64;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("yuetu midi note instances"),
            size: instance_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group,
            uniform_buffer,
            instance_buffer,
            instance_capacity,
            uploaded_id: None,
        }
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &MidiGpuData,
        uniform: &MidiUniform,
    ) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(uniform));

        if self.uploaded_id == Some(data.id) {
            return;
        }

        let required = std::mem::size_of_val(data.instances.as_slice()) as u64;
        if required > self.instance_capacity {
            self.instance_capacity = required.next_power_of_two().max(32);
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("yuetu midi note instances"),
                size: self.instance_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if required > 0 {
            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(data.instances.as_slice()),
            );
        }
        self.uploaded_id = Some(data.id);
    }
}
