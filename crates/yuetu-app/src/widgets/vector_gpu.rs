//! Reusable Vello target registered with egui as a native texture.
//!
//! The scene is rebuilt and rendered on every draw. The target texture is only
//! a display surface; it is recreated when the requested size changes.

use std::sync::Arc;

use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use vello_gpu::{
    ClearSettings, RenderSize, RenderTargetConfig, Renderer, Resources, Scene, TargetInit,
    TextureBindings,
};
use yuetu_vector::{VectorDocument, ViewTransform};

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct VectorGpu {
    render_state: Arc<RenderState>,
    target: Option<Target>,
}

impl Drop for VectorGpu {
    fn drop(&mut self) {
        if let Some(target) = self.target.take() {
            self.render_state
                .renderer
                .write()
                .free_texture(&target.texture_id);
        }
    }
}

struct Target {
    width: u32,
    height: u32,
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    renderer: Renderer,
    resources: Resources,
    texture_id: egui::TextureId,
}

impl VectorGpu {
    pub fn new(render_state: Arc<RenderState>) -> Self {
        Self {
            render_state,
            target: None,
        }
    }

    /// Render the requested viewport and return the native egui texture.
    /// Returns `None` when this document cannot be represented by the current
    /// GPU scene builder or the GPU target failed.
    pub fn render(
        &mut self,
        document: &VectorDocument,
        page: usize,
        width: u32,
        height: u32,
        transform: ViewTransform,
    ) -> Option<egui::TextureId> {
        if width == 0 || height == 0 || width > u32::from(u16::MAX) || height > u32::from(u16::MAX)
        {
            return None;
        }
        let scene = document.build_gpu_scene(page, width, height, transform)?;

        if self
            .target
            .as_ref()
            .is_none_or(|target| target.width != width || target.height != height)
        {
            if let Some(old) = self.target.take() {
                self.render_state
                    .renderer
                    .write()
                    .free_texture(&old.texture_id);
            }
            self.target = Some(Target::new(&self.render_state, width, height));
        }

        let target = self.target.as_mut()?;
        if let Err(error) = target.render(&self.render_state, &scene) {
            tracing::warn!("vector GPU render failed: {error}");
            return None;
        }
        Some(target.texture_id)
    }
}

impl Target {
    fn new(render_state: &RenderState, width: u32, height: u32) -> Self {
        let device = &render_state.device;
        let size = RenderSize {
            width: width as u16,
            height: height as u16,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yuetu.vector.gpu"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = Renderer::create_depth_texture_view(device, &size);
        let (renderer, resources) = Renderer::new(
            device,
            &RenderTargetConfig {
                format: TARGET_FORMAT,
                width: size.width,
                height: size.height,
            },
        );
        let texture_id = render_state.renderer.write().register_native_texture(
            device,
            &view,
            wgpu::FilterMode::Linear,
        );

        Self {
            width,
            height,
            _texture: texture,
            view,
            depth,
            renderer,
            resources,
            texture_id,
        }
    }

    fn render(&mut self, render_state: &RenderState, scene: &Scene) -> Result<(), String> {
        let size = RenderSize {
            width: self.width as u16,
            height: self.height as u16,
        };
        let mut encoder =
            render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("yuetu.vector.gpu encoder"),
                });
        self.renderer
            .render(
                scene,
                &mut self.resources,
                &render_state.device,
                &render_state.queue,
                &mut encoder,
                &size,
                &self.view,
                Some(&self.depth),
                &TextureBindings::new(),
                TargetInit::Clear(ClearSettings::default()),
            )
            .map_err(|error| error.to_string())?;
        render_state.queue.submit([encoder.finish()]);
        Ok(())
    }
}
