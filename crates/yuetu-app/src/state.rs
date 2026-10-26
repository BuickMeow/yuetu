//! Application state that outlives a single frame.

use egui::{TextureFilter, TextureHandle, TextureOptions, TextureWrapMode};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use yuetu_core::resize::resize;
use yuetu_core::{
    Color, ConversionConfig, ConversionResult, ImageSequence, IndexedImage, KeyMap, Palette,
    ResizeAlgorithm, RgbaImage, TimeAxis, generate_notes_on_grid, load_image_sequence,
    normalize_degrees, quantize, resolve_time_rows, rotate_image, rotated_dimensions,
};
use yuetu_vector::VectorDocument;

use crate::widgets::midi_gpu::MidiGpuData;
use crate::widgets::vector_gpu::VectorGpu;

/// Longest side of the texture uploaded to the GPU. The conversion still uses
/// the full-resolution CPU image; this is display-only.
const MAX_TEXTURE_SIDE: u32 = 4096;
/// Longest side of a frame-strip thumbnail before rotation. Small enough that
/// generating a few visible thumbnails while scrolling is cheap.
const THUMBNAIL_MAX_SIDE: u32 = 96;
/// Working-resolution source used by stage previews while scrubbing. The
/// final conversion still uses the original image.
const INTERACTION_MAX_SIDE: u32 = 1024;
/// Small vector raster used only while the rotation slider is moving. The
/// full interaction/display rasters are rebuilt once the drag ends.
const LIVE_ROTATION_PREVIEW_SIDE: u32 = 256;
/// Rasterization width used by the existing pixel pipeline for SVG sources.
/// Display rendering is independent and follows the viewport zoom.
const VECTOR_RASTER_LONG_SIDE: u32 = 2048;
/// Wait for zoom to settle before re-rasterizing the vector source. The old
/// texture is reused (at the same aspect) while the gesture is active.
const VECTOR_RENDER_DEBOUNCE: Duration = Duration::from_millis(80);
/// Full-page texture used only while a new viewport crop is pending. Higher
/// than the normal source preview keeps intermediate zoom frames sharper.
const VECTOR_FALLBACK_SIDE: u32 = 4096;

struct Thumbnail {
    texture: TextureHandle,
    size: egui::Vec2,
}

struct VectorViewCache {
    key: VectorViewKey,
    texture: TextureHandle,
}

struct VectorFallbackCache {
    key: VectorFallbackKey,
    texture: TextureHandle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VectorViewKey {
    width: u32,
    height: u32,
    page: u32,
    rotation_bits: u32,
    content: [i32; 4],
    viewport: [i32; 4],
    ppp_bits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VectorFallbackKey {
    page: u32,
    rotation_bits: u32,
    width: u32,
    height: u32,
}

/// One texture draw for the source stage. Vector sources can cover the whole
/// viewport with a cropped render instead of stretching a full-page texture.
pub struct VectorDraw {
    pub texture: egui::TextureId,
    pub rect: egui::Rect,
}

/// A live vector source. The raster fields on `SourceImage` remain the
/// deterministic conversion image; this cache drives the source-stage display.
pub struct VectorSource {
    document: VectorDocument,
    page: usize,
    cache: RefCell<Option<VectorViewCache>>,
    fallback: RefCell<Option<VectorFallbackCache>>,
    pending: RefCell<Option<(VectorViewKey, Instant)>>,
    gpu: RefCell<Option<VectorGpu>>,
    gpu_key: Cell<Option<VectorViewKey>>,
    gpu_texture: Cell<Option<egui::TextureId>>,
}

impl VectorSource {
    fn new(
        document: VectorDocument,
        gpu_render_state: Option<Arc<eframe::egui_wgpu::RenderState>>,
    ) -> Self {
        Self {
            document,
            page: 0,
            cache: RefCell::new(None),
            fallback: RefCell::new(None),
            pending: RefCell::new(None),
            gpu: RefCell::new(gpu_render_state.map(VectorGpu::new)),
            gpu_key: Cell::new(None),
            gpu_texture: Cell::new(None),
        }
    }

    pub fn page_count(&self) -> usize {
        self.document.page_count()
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn page_has_alpha(&self) -> bool {
        self.document.page_has_alpha()
    }

    fn rotation_interaction_image(&self, max_side: u32, rotation: f32) -> Option<Arc<RgbaImage>> {
        self.document
            .render_page_rotated_tight(self.page, max_side, rotation)
            .ok()
            .map(Arc::new)
    }

    fn page_size(&self) -> Option<(f32, f32)> {
        self.document.page_size(self.page)
    }

    fn set_page(&mut self, page: usize) {
        if page < self.document.page_count() {
            self.page = page;
            *self.cache.borrow_mut() = None;
            *self.fallback.borrow_mut() = None;
            *self.pending.borrow_mut() = None;
            self.gpu_key.set(None);
            self.gpu_texture.set(None);
        }
    }

    fn ensure_fallback(
        &self,
        ctx: &egui::Context,
        display_size: (u32, u32),
        rotation_deg: f32,
        max_side: u32,
    ) -> Option<egui::TextureId> {
        let rotation = normalize_degrees(rotation_deg);
        let (width, height) = fallback_dimensions(display_size, max_side);
        if width == 0 || height == 0 {
            return None;
        }
        let key = VectorFallbackKey {
            page: self.page as u32,
            rotation_bits: rotation.to_bits(),
            width,
            height,
        };
        if let Some(cache) = self.fallback.borrow().as_ref() {
            if cache.key == key {
                return Some(cache.texture.id());
            }
        }
        let image = match self
            .document
            .render_page_rotated_to_fit(self.page, width, height, rotation)
        {
            Ok(image) => image,
            Err(error) => {
                tracing::warn!("failed to render vector fallback: {error}");
                return None;
            }
        };
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            &image.pixels,
        );
        let options = vector_texture_options();
        let mut fallback = self.fallback.borrow_mut();
        if let Some(cache) = fallback.as_mut() {
            cache.texture.set(color, options);
            cache.key = key;
            Some(cache.texture.id())
        } else {
            let texture = ctx.load_texture("yuetu.vector.fallback", color, options);
            let id = texture.id();
            *fallback = Some(VectorFallbackCache { key, texture });
            Some(id)
        }
    }

    pub fn draw(
        &self,
        ctx: &egui::Context,
        content: egui::Rect,
        viewport: egui::Rect,
        display_size: (u32, u32),
        rotation_deg: f32,
    ) -> Option<VectorDraw> {
        if !content.is_positive() || !viewport.is_positive() {
            return None;
        }
        let ppp = ctx.pixels_per_point();
        let max_side = ctx
            .input(|input| input.max_texture_side)
            .clamp(1, MAX_TEXTURE_SIDE as usize) as u32;
        let rotation = normalize_degrees(rotation_deg);
        let fallback = || self.ensure_fallback(ctx, display_size, rotation, max_side);
        if !self.document.supports_view_render() {
            return Some(VectorDraw {
                texture: fallback()?,
                rect: content,
            });
        }
        let page_size = self.page_size()?;
        let visible = content.intersect(viewport);
        if !visible.is_positive() {
            return Some(VectorDraw {
                texture: fallback()?,
                rect: content,
            });
        }
        let (width, height) = capped_texture_size(visible.size(), ppp, max_side);
        if width == 0 || height == 0 {
            return None;
        }
        let key = VectorViewKey {
            width,
            height,
            page: self.page as u32,
            rotation_bits: rotation.to_bits(),
            content: [
                (content.left() * ppp).round() as i32,
                (content.top() * ppp).round() as i32,
                (content.right() * ppp).round() as i32,
                (content.bottom() * ppp).round() as i32,
            ],
            viewport: [
                (visible.left() * ppp).round() as i32,
                (visible.top() * ppp).round() as i32,
                (visible.right() * ppp).round() as i32,
                (visible.bottom() * ppp).round() as i32,
            ],
            ppp_bits: ppp.to_bits(),
        };
        if let (Some(gpu_key), Some(texture)) = (self.gpu_key.get(), self.gpu_texture.get()) {
            if gpu_key == key {
                *self.pending.borrow_mut() = None;
                return Some(VectorDraw {
                    texture,
                    rect: visible,
                });
            }
        }

        let out_scale_x = width as f32 / (visible.width() * ppp).max(1e-6);
        let out_scale_y = height as f32 / (visible.height() * ppp).max(1e-6);
        let transform = vector_view_transform(
            page_size,
            content,
            visible,
            rotation,
            ppp,
            out_scale_x,
            out_scale_y,
        );
        // GPU path rendering is fast enough to run during a zoom or pan. Doing
        // it before the fallback debounce keeps SVG sources sharp for the whole
        // gesture instead of showing an upscaled page raster for 80 ms.
        let gpu_texture = self
            .gpu
            .borrow_mut()
            .as_mut()
            .and_then(|gpu| gpu.render(&self.document, self.page, width, height, transform));
        if let Some(texture) = gpu_texture {
            *self.pending.borrow_mut() = None;
            self.gpu_key.set(Some(key));
            self.gpu_texture.set(Some(texture));
            return Some(VectorDraw {
                texture,
                rect: visible,
            });
        }

        if let Some(cache) = self.cache.borrow().as_ref() {
            if cache.key == key {
                *self.pending.borrow_mut() = None;
                return Some(VectorDraw {
                    texture: cache.texture.id(),
                    rect: visible,
                });
            }
        }

        let now = Instant::now();
        {
            let mut pending = self.pending.borrow_mut();
            match pending.as_ref() {
                Some((pending_key, since)) if *pending_key == key => {
                    let elapsed = now.saturating_duration_since(*since);
                    if elapsed < VECTOR_RENDER_DEBOUNCE {
                        ctx.request_repaint_after(VECTOR_RENDER_DEBOUNCE - elapsed);
                        return Some(VectorDraw {
                            texture: fallback()?,
                            rect: content,
                        });
                    }
                }
                _ => {
                    *pending = Some((key, now));
                    ctx.request_repaint_after(VECTOR_RENDER_DEBOUNCE);
                    return Some(VectorDraw {
                        texture: fallback()?,
                        rect: content,
                    });
                }
            }
        }

        let image = match self
            .document
            .render_page_view(self.page, width, height, transform)
        {
            Ok(image) => image,
            Err(error) => {
                tracing::warn!("failed to render vector viewport: {error}");
                *self.pending.borrow_mut() = None;
                return Some(VectorDraw {
                    texture: fallback()?,
                    rect: content,
                });
            }
        };
        *self.pending.borrow_mut() = None;
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            &image.pixels,
        );
        let options = vector_texture_options();
        let mut cache = self.cache.borrow_mut();
        let id = if let Some(cache) = cache.as_mut() {
            cache.texture.set(color, options);
            cache.key = key;
            cache.texture.id()
        } else {
            let texture = ctx.load_texture("yuetu.vector.view", color, options);
            let id = texture.id();
            *cache = Some(VectorViewCache { key, texture });
            id
        };
        Some(VectorDraw {
            texture: id,
            rect: visible,
        })
    }
}

pub struct SourceImage {
    pub path: PathBuf,
    pub name: String,
    /// The currently selected frame, before rotation. Conversion applies
    /// `rotation_deg` in the worker so changing the angle never blocks the UI.
    pub image: Arc<RgbaImage>,
    /// Rotated selected frame used by the source stage. For large rotations
    /// this may be a display-sized preview; `display_size` is the full canvas.
    pub display_image: Arc<RgbaImage>,
    /// Full rotated canvas dimensions, used by rulers and hit-testing.
    pub display_size: (u32, u32),
    /// Low-resolution rotated source used for stage 2/3 interaction previews.
    interaction_image: Arc<RgbaImage>,
    pub has_transparency: bool,
    vector: Option<VectorSource>,
    frames: Vec<Arc<RgbaImage>>,
    delays_ms: Vec<u32>,
    original_palette: Option<Palette>,
    frame_index: usize,
    rotation_deg: f32,
    /// A live vector rotation updated only the cheap geometry fields. The
    /// display/interaction rasters are rebuilt once the drag ends.
    rotation_preview_dirty: bool,
    thumbnails: Vec<Option<Thumbnail>>,
    pub texture: TextureHandle,
    texture_dirty: bool,
}

impl SourceImage {
    #[cfg(test)]
    pub fn load(ctx: &egui::Context, path: PathBuf) -> yuetu_core::Result<Self> {
        Self::load_with_gpu(ctx, path, None)
    }

    pub fn load_with_gpu(
        ctx: &egui::Context,
        path: PathBuf,
        gpu_render_state: Option<Arc<eframe::egui_wgpu::RenderState>>,
    ) -> yuetu_core::Result<Self> {
        if is_vector(&path) {
            return Self::load_vector(ctx, path, gpu_render_state);
        }
        let ImageSequence {
            frames,
            delays_ms,
            original_palette,
        } = load_image_sequence(&path)?;
        let frames: Vec<Arc<RgbaImage>> = frames.into_iter().map(Arc::new).collect();
        Self::from_sequence(ctx, path, frames, delays_ms, original_palette, None)
    }

    fn load_vector(
        ctx: &egui::Context,
        path: PathBuf,
        gpu_render_state: Option<Arc<eframe::egui_wgpu::RenderState>>,
    ) -> yuetu_core::Result<Self> {
        let document = VectorDocument::load(&path)
            .map_err(|error| yuetu_core::Error::InvalidImage(error.to_string()))?;
        let raster = document
            .render_to_long_side(VECTOR_RASTER_LONG_SIDE)
            .map_err(|error| yuetu_core::Error::InvalidImage(error.to_string()))?;
        let frames = vec![Arc::new(raster)];
        Self::from_sequence(
            ctx,
            path,
            frames,
            vec![0],
            None,
            Some(VectorSource::new(document, gpu_render_state)),
        )
    }

    fn from_sequence(
        ctx: &egui::Context,
        path: PathBuf,
        frames: Vec<Arc<RgbaImage>>,
        delays_ms: Vec<u32>,
        original_palette: Option<Palette>,
        vector: Option<VectorSource>,
    ) -> yuetu_core::Result<Self> {
        let image = frames[0].clone();
        let (display_image, display_size) = prepare_display(ctx, &image, 0.0)?;
        let interaction_image = prepare_interaction_from_display(&display_image)?;
        let has_transparency = image_has_transparency(&display_image);
        let texture_name = format!("source:{}:0", path.display());
        let texture = upload_texture(ctx, &texture_name, &display_image)?;

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image")
            .to_owned();
        let thumbnail_count = frames.len();

        Ok(Self {
            path,
            name,
            image,
            display_image,
            display_size,
            interaction_image,
            has_transparency,
            vector,
            frames,
            delays_ms,
            original_palette,
            frame_index: 0,
            rotation_deg: 0.0,
            rotation_preview_dirty: false,
            thumbnails: std::iter::repeat_with(|| None)
                .take(thumbnail_count)
                .collect(),
            texture,
            texture_dirty: false,
        })
    }

    pub fn vector(&self) -> Option<&VectorSource> {
        self.vector.as_ref()
    }

    pub fn rotation_deg(&self) -> f32 {
        self.rotation_deg
    }

    pub fn vector_page_count(&self) -> usize {
        self.vector
            .as_ref()
            .map(VectorSource::page_count)
            .unwrap_or(1)
    }

    pub fn vector_page(&self) -> usize {
        self.vector.as_ref().map(VectorSource::page).unwrap_or(0)
    }

    pub fn select_vector_page(
        &mut self,
        ctx: &egui::Context,
        page: usize,
        upload_display_texture: bool,
    ) -> yuetu_core::Result<bool> {
        let Some(vector) = self.vector.as_mut() else {
            return Ok(false);
        };
        if page >= vector.page_count() || page == vector.page {
            return Ok(false);
        }
        let raster = vector
            .document
            .render_page_to_long_side(page, VECTOR_RASTER_LONG_SIDE)
            .map_err(|error| yuetu_core::Error::InvalidImage(error.to_string()))?;
        vector.set_page(page);
        let image = Arc::new(raster);
        let (display_image, display_size) = prepare_display(ctx, &image, self.rotation_deg)?;
        let interaction_image = prepare_interaction_from_display(&display_image)?;
        if upload_display_texture {
            let has_transparency = image_has_transparency(&display_image);
            let texture_name = self.texture_name(0);
            self.texture = upload_texture(ctx, &texture_name, &display_image)?;
            self.has_transparency = has_transparency;
            self.texture_dirty = false;
        } else {
            self.texture_dirty = true;
        }
        self.image = image;
        self.display_image = display_image;
        self.display_size = display_size;
        self.interaction_image = interaction_image;
        self.frame_index = 0;
        if let Some(frame) = self.frames.first_mut() {
            *frame = self.image.clone();
        }
        Ok(true)
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn frame_index(&self) -> usize {
        self.frame_index
    }

    pub fn delays_ms(&self) -> &[u32] {
        &self.delays_ms
    }

    pub fn palette_frames(&self) -> &[Arc<RgbaImage>] {
        &self.frames
    }

    pub fn original_palette(&self) -> Option<&Palette> {
        self.original_palette.as_ref()
    }

    pub fn interaction_image(&self) -> &Arc<RgbaImage> {
        &self.interaction_image
    }

    /// Sample the display preview using full-canvas pixel coordinates.
    pub fn display_pixel(&self, x: u32, y: u32) -> Color {
        let (full_width, full_height) = self.display_size;
        if full_width == 0 || full_height == 0 || x >= full_width || y >= full_height {
            return Color::TRANSPARENT;
        }
        let preview_width = self.display_image.width.max(1);
        let preview_height = self.display_image.height.max(1);
        let sx = (((x as f64 + 0.5) * f64::from(preview_width) / f64::from(full_width)).floor()
            as u32)
            .min(preview_width - 1);
        let sy = (((y as f64 + 0.5) * f64::from(preview_height) / f64::from(full_height)).floor()
            as u32)
            .min(preview_height - 1);
        self.display_image
            .pixel(sx, sy)
            .unwrap_or(Color::TRANSPARENT)
    }

    /// Lazily build a small display texture for the frame strip. Only visible
    /// cells ask for thumbnails, so a 1000-frame GIF does not upload 1000
    /// textures up front.
    pub fn thumbnail(
        &mut self,
        ctx: &egui::Context,
        index: usize,
    ) -> Option<(egui::TextureId, egui::Vec2)> {
        let index = index.min(self.frames.len().checked_sub(1)?);
        if self.thumbnails[index].is_none() {
            let frame = self.frames.get(index)?;
            let scale = (THUMBNAIL_MAX_SIDE as f32 / frame.width.max(1) as f32)
                .min(THUMBNAIL_MAX_SIDE as f32 / frame.height.max(1) as f32)
                .min(1.0);
            let width = (frame.width as f32 * scale).round().max(1.0) as u32;
            let height = (frame.height as f32 * scale).round().max(1.0) as u32;
            let small = resize(frame, width, height, ResizeAlgorithm::Nearest).ok()?;
            let rotated = if normalize_degrees(self.rotation_deg) == 0.0 {
                small
            } else {
                rotate_image(&small, self.rotation_deg).ok()?
            };
            let name = format!(
                "thumb:{}:{}:{:.2}",
                self.path.display(),
                index,
                self.rotation_deg
            );
            let texture = upload_texture(ctx, &name, &rotated).ok()?;
            self.thumbnails[index] = Some(Thumbnail {
                texture,
                size: egui::vec2(rotated.width as f32, rotated.height as f32),
            });
        }

        self.thumbnails[index]
            .as_ref()
            .map(|thumbnail| (thumbnail.texture.id(), thumbnail.size))
    }

    pub fn select_frame(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        upload_display_texture: bool,
    ) -> yuetu_core::Result<bool> {
        if self.frames.len() <= 1 {
            return Ok(false);
        }
        let index = index.min(self.frames.len() - 1);
        if index == self.frame_index {
            return Ok(false);
        }

        let image = self.frames[index].clone();
        let (display_image, display_size) = prepare_display(ctx, &image, self.rotation_deg)?;
        let interaction_image = prepare_interaction_from_display(&display_image)?;
        if upload_display_texture {
            let has_transparency = image_has_transparency(&display_image);
            let texture_name = self.texture_name(index);
            let texture = upload_texture(ctx, &texture_name, &display_image)?;
            self.texture = texture;
            self.has_transparency = has_transparency;
            self.texture_dirty = false;
        } else {
            self.texture_dirty = true;
        }
        self.image = image;
        self.display_image = display_image;
        self.display_size = display_size;
        self.interaction_image = interaction_image;
        self.frame_index = index;
        Ok(true)
    }

    /// Change the source rotation. Only the selected frame is rotated here for
    /// the source stage; conversion rotates the selected frame in its worker
    /// and builds the shared palette from the unrotated sequence.
    pub fn set_rotation(
        &mut self,
        ctx: &egui::Context,
        degrees: f32,
        upload_display_texture: bool,
        interactive: bool,
    ) -> yuetu_core::Result<bool> {
        let degrees = normalize_degrees(degrees);
        let changed = (degrees - self.rotation_deg).abs() >= 1e-4;
        if !changed && !(self.rotation_preview_dirty && !interactive) {
            return Ok(false);
        }

        self.rotation_deg = degrees;
        if interactive && self.vector.is_some() {
            // Vector stages do not need the 2048px display raster while the
            // slider is moving. The source stage draws through Vello directly;
            // the key-map/palette stages only need a small interaction image.
            // Rotating and uploading the full display image every frame was
            // what made live rotation feel frozen.
            self.display_size = rotated_dimensions(self.image.width, self.image.height, degrees);
            self.rotation_preview_dirty = true;
            if !upload_display_texture {
                if let Some(image) = self.vector.as_ref().and_then(|vector| {
                    vector.rotation_interaction_image(LIVE_ROTATION_PREVIEW_SIDE, degrees)
                }) {
                    self.interaction_image = image;
                }
            }
            for thumbnail in &mut self.thumbnails {
                *thumbnail = None;
            }
            return Ok(true);
        }

        self.rebuild_rotation_preview(ctx, upload_display_texture)?;
        Ok(true)
    }

    /// Rebuild the CPU display/interaction rasters after a live vector drag.
    pub fn finalize_rotation_preview(&mut self, ctx: &egui::Context) -> yuetu_core::Result<bool> {
        if !self.rotation_preview_dirty {
            return Ok(false);
        }
        self.rebuild_rotation_preview(ctx, true)?;
        Ok(true)
    }

    fn rebuild_rotation_preview(
        &mut self,
        ctx: &egui::Context,
        upload_display_texture: bool,
    ) -> yuetu_core::Result<()> {
        let (display_image, display_size) = prepare_display(ctx, &self.image, self.rotation_deg)?;
        let interaction_image = prepare_interaction_from_display(&display_image)?;
        if upload_display_texture {
            let has_transparency = image_has_transparency(&display_image);
            let texture_name = format!(
                "source:{}:{}:{:.3}",
                self.path.display(),
                self.frame_index,
                self.rotation_deg
            );
            let texture = upload_texture(ctx, &texture_name, &display_image)?;
            self.texture = texture;
            self.has_transparency = has_transparency;
            self.texture_dirty = false;
        } else {
            self.texture_dirty = true;
        }
        self.display_image = display_image;
        self.display_size = display_size;
        self.interaction_image = interaction_image;
        self.rotation_preview_dirty = false;
        // Frame-strip thumbnails are cached in the old orientation.
        for thumbnail in &mut self.thumbnails {
            *thumbnail = None;
        }
        Ok(())
    }

    /// Upload the current display image if frame scrubbing skipped it while
    /// the source stage was hidden.
    pub fn ensure_display_texture(&mut self, ctx: &egui::Context) -> yuetu_core::Result<()> {
        if !self.texture_dirty {
            return Ok(());
        }
        let has_transparency = image_has_transparency(&self.display_image);
        let texture_name = self.texture_name(self.frame_index);
        let texture = upload_texture(ctx, &texture_name, &self.display_image)?;
        self.texture = texture;
        self.has_transparency = has_transparency;
        self.texture_dirty = false;
        Ok(())
    }

    fn texture_name(&self, frame_index: usize) -> String {
        format!(
            "source:{}:{}:{:.3}",
            self.path.display(),
            frame_index,
            self.rotation_deg
        )
    }
}

fn capped_texture_size(size: egui::Vec2, ppp: f32, max_side: u32) -> (u32, u32) {
    let mut width = (size.x * ppp).round().max(1.0) as u32;
    let mut height = (size.y * ppp).round().max(1.0) as u32;
    let max_dim = width.max(height);
    if max_dim > max_side {
        let scale = max_side as f32 / max_dim as f32;
        width = (width as f32 * scale).round().max(1.0) as u32;
        height = (height as f32 * scale).round().max(1.0) as u32;
    }
    (width, height)
}

fn vector_texture_options() -> TextureOptions {
    TextureOptions {
        magnification: TextureFilter::Linear,
        minification: TextureFilter::Linear,
        wrap_mode: TextureWrapMode::ClampToEdge,
        mipmap_mode: None,
    }
}

fn fallback_dimensions(display_size: (u32, u32), max_side: u32) -> (u32, u32) {
    let max_side = max_side.min(VECTOR_FALLBACK_SIDE).max(1);
    let width = display_size.0.max(1);
    let height = display_size.1.max(1);
    if width >= height {
        (
            max_side,
            ((max_side as f32 * height as f32 / width as f32).round() as u32).max(1),
        )
    } else {
        (
            ((max_side as f32 * width as f32 / height as f32).round() as u32).max(1),
            max_side,
        )
    }
}

fn vector_view_transform(
    page_size: (f32, f32),
    content: egui::Rect,
    viewport: egui::Rect,
    rotation: f32,
    pixels_per_point: f32,
    out_scale_x: f32,
    out_scale_y: f32,
) -> yuetu_vector::ViewTransform {
    let (page_w, page_h) = page_size;
    let radians = rotation.to_radians();
    let (sin, cos) = radians.sin_cos();
    let rotated_w = page_w * cos.abs() + page_h * sin.abs();
    let rotated_h = page_w * sin.abs() + page_h * cos.abs();
    let sx = content.width() / rotated_w.max(1e-6) * pixels_per_point * out_scale_x;
    let sy = content.height() / rotated_h.max(1e-6) * pixels_per_point * out_scale_y;
    let ex = rotated_w * 0.5 - cos * page_w * 0.5 + sin * page_h * 0.5;
    let ey = rotated_h * 0.5 - sin * page_w * 0.5 - cos * page_h * 0.5;
    let tx = (content.left() - viewport.left()) * pixels_per_point * out_scale_x;
    let ty = (content.top() - viewport.top()) * pixels_per_point * out_scale_y;
    yuetu_vector::ViewTransform {
        a: sx * cos,
        b: sy * sin,
        c: -sx * sin,
        d: sy * cos,
        e: sx * ex + tx,
        f: sy * ey + ty,
    }
}

fn is_vector(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "svg" | "pdf" | "ai" | "eps"
            )
        })
}

fn prepare_display(
    ctx: &egui::Context,
    image: &Arc<RgbaImage>,
    degrees: f32,
) -> yuetu_core::Result<(Arc<RgbaImage>, (u32, u32))> {
    let degrees = normalize_degrees(degrees);
    let full_size = rotated_dimensions(image.width, image.height, degrees);
    let max_side = ctx
        .input(|input| input.max_texture_side)
        .clamp(256, MAX_TEXTURE_SIDE as usize) as u32;
    let scale = (max_side as f64 / full_size.0.max(full_size.1).max(1) as f64).min(1.0);
    let width = (image.width as f64 * scale).round().max(1.0) as u32;
    let height = (image.height as f64 * scale).round().max(1.0) as u32;
    let small = if width == image.width && height == image.height {
        Arc::clone(image)
    } else {
        Arc::new(resize(image, width, height, ResizeAlgorithm::Area)?)
    };
    if degrees == 0.0 {
        return Ok((small, full_size));
    }
    let rotated = rotate_image(&small, degrees)?;
    Ok((Arc::new(rotated), full_size))
}

fn prepare_interaction_from_display(
    display: &Arc<RgbaImage>,
) -> yuetu_core::Result<Arc<RgbaImage>> {
    let max_side = display.width.max(display.height);
    if max_side <= INTERACTION_MAX_SIDE {
        return Ok(Arc::clone(display));
    }
    let scale = INTERACTION_MAX_SIDE as f64 / f64::from(max_side.max(1));
    let width = (f64::from(display.width) * scale).round().max(1.0) as u32;
    let height = (f64::from(display.height) * scale).round().max(1.0) as u32;
    Ok(Arc::new(resize(
        display,
        width,
        height,
        ResizeAlgorithm::Nearest,
    )?))
}

fn upload_texture(
    ctx: &egui::Context,
    name: &str,
    image: &RgbaImage,
) -> yuetu_core::Result<TextureHandle> {
    Ok(ctx.load_texture(
        name,
        texture_color_image(ctx, image)?,
        display_texture_options(),
    ))
}

fn update_texture(
    texture: &mut TextureHandle,
    ctx: &egui::Context,
    image: &RgbaImage,
) -> yuetu_core::Result<()> {
    texture.set(texture_color_image(ctx, image)?, display_texture_options());
    Ok(())
}

fn texture_color_image(
    ctx: &egui::Context,
    image: &RgbaImage,
) -> yuetu_core::Result<egui::ColorImage> {
    // Never assume 4096: some backends (and egui's defaults) cap at 2048.
    // Loading a larger texture is a debug assertion/panic inside egui.
    let max_texture_side = ctx
        .input(|input| input.max_texture_side)
        .clamp(256, MAX_TEXTURE_SIDE as usize) as u32;
    if image.width.max(image.height) > max_texture_side {
        let scale = max_texture_side as f64 / f64::from(image.width.max(image.height));
        let width = (f64::from(image.width) * scale)
            .round()
            .clamp(1.0, f64::from(max_texture_side)) as u32;
        let height = (f64::from(image.height) * scale)
            .round()
            .clamp(1.0, f64::from(max_texture_side)) as u32;
        let display = resize(image, width, height, ResizeAlgorithm::Area)?;
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [display.width as usize, display.height as usize],
            display.as_rgba(),
        ))
    } else {
        Ok(egui::ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            image.as_rgba(),
        ))
    }
}

fn display_texture_options() -> TextureOptions {
    // Nearest on both sides of the 1:1 line: no filtering switch at 50% on
    // Retina, no LOD/thumbnail illusion, and pixel boundaries stay crisp.
    TextureOptions {
        magnification: TextureFilter::Nearest,
        minification: TextureFilter::Nearest,
        wrap_mode: TextureWrapMode::ClampToEdge,
        mipmap_mode: None,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PipelineStage {
    #[default]
    Source,
    KeyMap,
    Palette,
    Midi,
}

impl PipelineStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "源图",
            Self::KeyMap => "键位映射",
            Self::Palette => "调色板",
            Self::Midi => "MIDI 生成",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Source => 0,
            Self::KeyMap => 1,
            Self::Palette => 2,
            Self::Midi => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ZoomPreset {
    #[default]
    Fit,
    Fill,
    Actual,
}

impl ZoomPreset {
    pub fn next(self) -> Self {
        match self {
            Self::Fit => Self::Fill,
            Self::Fill => Self::Actual,
            Self::Actual => Self::Fit,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fit => "适应",
            Self::Fill => "填充",
            Self::Actual => "100%",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CoordinateOrigin {
    #[default]
    TopLeft,
    BottomLeft,
}

impl CoordinateOrigin {
    /// Convert a top-left pixel row into the displayed row for this origin.
    pub fn display_y(self, y: u32, height: u32) -> u32 {
        match self {
            Self::TopLeft => y,
            Self::BottomLeft => height.saturating_sub(1).saturating_sub(y),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StageStop {
    pub stage: PipelineStage,
    pub center_y: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum StageHover {
    Source {
        pos: (u32, u32),
        color: Color,
        height: u32,
    },
    KeyMap {
        pos: (u32, u32),
        color: Color,
        key: Option<u8>,
        height: u32,
        time_axis: TimeAxis,
    },
    Palette {
        index: i32,
        color: Color,
    },
    Midi {
        key: u8,
        tick: u32,
    },
}

/// Interaction-resolution image for the stage currently being scrubbed.
/// Only the visible stage is built, so key-map scrubbing never pays for
/// quantization and palette scrubbing never uploads a resized texture.
pub enum StageSnapshot {
    KeyMap {
        time_axis: TimeAxis,
        resized_image: RgbaImage,
        resized_texture: TextureHandle,
        resized_has_transparency: bool,
        resized_size: (u32, u32),
        keymap: KeyMap,
    },
    Palette {
        indexed_image: IndexedImage,
        indexed_texture: TextureHandle,
        indexed_has_transparency: bool,
        palette: Palette,
    },
}

pub struct Preview {
    pub result: ConversionResult,
    /// The config that produced this preview; used to gate export while an
    /// automatic update is still running.
    pub config: ConversionConfig,
    /// Source generation, so selecting another GIF frame also invalidates it.
    pub source_revision: u64,
    /// Display texture for the resized (key-mapped) image.
    pub resized_texture: TextureHandle,
    pub resized_has_transparency: bool,
    /// Display texture for the palette-indexed image.
    pub indexed_texture: TextureHandle,
    pub indexed_has_transparency: bool,
    /// Persistent GPU instance data for the MIDI stage.
    pub midi_gpu: Arc<MidiGpuData>,
    /// Temporary interaction-resolution key-map/palette snapshot. The full
    /// result stays untouched, so MIDI/export never see mixed stages.
    pub interaction: Option<StageSnapshot>,
}

impl Preview {
    pub fn new(
        ctx: &egui::Context,
        result: ConversionResult,
        config: ConversionConfig,
        source_revision: u64,
    ) -> yuetu_core::Result<Self> {
        let resized_has_transparency = image_has_transparency(&result.resized_image);
        let resized_name = format!("stage:resized:{source_revision}");
        let resized_texture = upload_texture(ctx, &resized_name, &result.resized_image)?;

        let indexed_has_transparency = result.indexed_image.indices.iter().any(|&index| index < 0);
        let indexed_rgba = indexed_to_rgba(&result.indexed_image, &result.palette)?;
        let indexed_name = format!("stage:indexed:{source_revision}");
        let indexed_texture = upload_texture(ctx, &indexed_name, &indexed_rgba)?;

        let midi_gpu = Arc::new(MidiGpuData::from_result(&result));

        Ok(Self {
            result,
            config,
            source_revision,
            resized_texture,
            resized_has_transparency,
            indexed_texture,
            indexed_has_transparency,
            midi_gpu,
            interaction: None,
        })
    }

    /// Intermediate image refresh used while scrubbing frames on the
    /// key-map or palette stage. It uses the same resize algorithm as the
    /// final conversion so releasing the mouse does not change the picture.
    /// Notes and MIDI instances stay untouched until the final conversion.
    /// Generate notes on demand from the cached indexed image. Used when a
    /// preview was produced while the MIDI stage was not visible.
    pub fn ensure_notes(&mut self, config: &ConversionConfig) -> yuetu_core::Result<()> {
        if self.result.notes_generated {
            return Ok(());
        }
        let started = Instant::now();
        let (notes, note_count_by_track) = generate_notes_on_grid(
            &self.result.indexed_image,
            &self.result.keymap,
            self.result.palette.len(),
            config.max_note_length,
            config.measure_from_image_start,
            self.result.time_axis,
        );
        self.result.notes = notes;
        self.result.note_count_by_track = note_count_by_track;
        self.result.notes_generated = true;
        self.result.timings.notes_ms = started.elapsed().as_secs_f32() * 1000.0;
        self.midi_gpu = Arc::new(MidiGpuData::from_result(&self.result));
        Ok(())
    }

    /// Intermediate stage snapshot used while scrubbing frames on the
    /// key-map or palette stage. It uses the same resize algorithm as the
    /// final conversion so releasing the mouse does not change the picture.
    /// The full result (notes, MIDI instances, export metadata) is untouched.
    pub fn update_scrub_frame(
        &mut self,
        ctx: &egui::Context,
        image: &RgbaImage,
        full_source_size: (u32, u32),
        config: &ConversionConfig,
        palette: &Palette,
        stage: PipelineStage,
    ) -> yuetu_core::Result<()> {
        let keymap = KeyMap::build(
            config.key_start,
            config.key_end,
            config.key_filter,
            config.column_mapping,
        )?;
        let key_count = keymap.width().max(1);
        let time_rows = resolve_time_rows(
            full_source_size.0,
            full_source_size.1,
            key_count,
            config.time_axis,
            config.time_length,
        );
        let target_size = config.time_axis.grid_size(key_count, time_rows);
        let resized = resize(image, target_size.0, target_size.1, config.resize)?;

        self.interaction = Some(match stage {
            PipelineStage::KeyMap => {
                let texture = if let Some(StageSnapshot::KeyMap {
                    resized_texture, ..
                }) = self.interaction.as_ref()
                {
                    let mut texture = resized_texture.clone();
                    update_texture(&mut texture, ctx, &resized)?;
                    texture
                } else {
                    let name = format!("stage:resized:interaction:{}", self.source_revision);
                    upload_texture(ctx, &name, &resized)?
                };
                StageSnapshot::KeyMap {
                    time_axis: config.time_axis,
                    resized_has_transparency: image_has_transparency(&resized),
                    resized_image: resized,
                    resized_texture: texture,
                    resized_size: target_size,
                    keymap,
                }
            }
            PipelineStage::Palette => {
                let indexed = quantize(&resized, palette);
                let indexed_rgba = indexed_to_rgba(&indexed, palette)?;
                let texture = if let Some(StageSnapshot::Palette {
                    indexed_texture, ..
                }) = self.interaction.as_ref()
                {
                    let mut texture = indexed_texture.clone();
                    update_texture(&mut texture, ctx, &indexed_rgba)?;
                    texture
                } else {
                    let name = format!("stage:indexed:interaction:{}", self.source_revision);
                    upload_texture(ctx, &name, &indexed_rgba)?
                };
                StageSnapshot::Palette {
                    indexed_has_transparency: indexed.indices.iter().any(|&index| index < 0),
                    indexed_image: indexed,
                    indexed_texture: texture,
                    palette: palette.clone(),
                }
            }
            PipelineStage::Source | PipelineStage::Midi => return Ok(()),
        });
        Ok(())
    }
}

fn image_has_transparency(image: &RgbaImage) -> bool {
    image.pixels.chunks_exact(4).any(|pixel| pixel[3] < 255)
}

fn indexed_to_rgba(indexed: &IndexedImage, palette: &Palette) -> yuetu_core::Result<RgbaImage> {
    let mut pixels = vec![0u8; indexed.width as usize * indexed.height as usize * 4];
    for (pixel, &index) in pixels.chunks_exact_mut(4).zip(&indexed.indices) {
        if index < 0 {
            continue;
        }
        if let Some(color) = palette.colors.get(index as usize) {
            pixel.copy_from_slice(&[color.r, color.g, color.b, color.a]);
        }
    }
    RgbaImage::from_rgba(indexed.width, indexed.height, pixels)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct StatusMessage {
    pub kind: StatusKind,
    pub text: String,
    pub detail: Option<String>,
}

impl StatusMessage {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Info,
            text: text.into(),
            detail: None,
        }
    }

    pub fn success(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Success,
            text: text.into(),
            detail: None,
        }
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Warning,
            text: text.into(),
            detail: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Error,
            text: text.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl Default for StatusMessage {
    fn default() -> Self {
        Self::info("就绪")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_a_frame_updates_interaction_pixels_without_uploading_display() {
        let ctx = egui::Context::default();
        let red = Arc::new(RgbaImage::new(2, 2, Color::rgb(255, 0, 0)));
        let blue = Arc::new(RgbaImage::new(2, 2, Color::rgb(0, 0, 255)));
        let frames = vec![red.clone(), blue.clone()];
        let image = frames[0].clone();
        let (display_image, display_size) = prepare_display(&ctx, &image, 0.0).unwrap();
        let interaction_image = prepare_interaction_from_display(&display_image).unwrap();
        let texture = upload_texture(&ctx, "source-frame-test", &display_image).unwrap();
        let mut source = SourceImage {
            path: PathBuf::from("test.gif"),
            name: "test.gif".to_owned(),
            image,
            display_image,
            display_size,
            interaction_image,
            has_transparency: false,
            vector: None,
            frames,
            delays_ms: vec![100, 100],
            original_palette: None,
            frame_index: 0,
            rotation_deg: 0.0,
            rotation_preview_dirty: false,
            thumbnails: vec![None, None],
            texture,
            texture_dirty: false,
        };

        assert_eq!(
            source.interaction_image().pixel(0, 0),
            Some(Color::rgb(255, 0, 0))
        );
        assert!(source.select_frame(&ctx, 1, false).unwrap());
        assert_eq!(
            source.interaction_image().pixel(0, 0),
            Some(Color::rgb(0, 0, 255))
        );
        assert!(source.texture_dirty);
        source.ensure_display_texture(&ctx).unwrap();
        assert!(!source.texture_dirty);
    }

    #[test]
    fn live_vector_rotation_defers_the_display_raster_until_release() {
        let ctx = egui::Context::default();
        let source_path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/vector-red.svg"
        ));
        let mut source = SourceImage::load(&ctx, source_path).unwrap();
        let original_display = source.display_image.width;
        assert!(source.set_rotation(&ctx, 31.0, false, true).unwrap());
        assert!(source.rotation_preview_dirty);
        assert_eq!(source.display_image.width, original_display);
        assert_ne!(source.display_size, (100, 100));
        assert!(source.finalize_rotation_preview(&ctx).unwrap());
        assert!(!source.rotation_preview_dirty);
    }

    #[test]
    fn svg_loads_as_a_vector_source_and_builds_a_conversion_raster() {
        let ctx = egui::Context::default();
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/vector.svg"
        ));
        let source = SourceImage::load(&ctx, path).unwrap();
        assert!(source.vector().is_some());
        assert_eq!(source.frame_count(), 1);
        assert_eq!(source.vector_page_count(), 1);
        assert_eq!(source.display_size, (2048, 1536));
        assert_eq!(source.image.pixel(100, 100).map(|color| color.a), Some(255));
    }

    #[test]
    fn pdf_pages_can_be_selected() {
        let ctx = egui::Context::default();
        let path = PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/vector.pdf"
        ));
        let mut source = SourceImage::load(&ctx, path).unwrap();
        assert_eq!(source.vector_page_count(), 2);
        assert!(source.select_vector_page(&ctx, 1, false).unwrap());
        assert_eq!(source.vector_page(), 1);
        assert_eq!(source.display_size, (2048, 1536));
    }

    #[test]
    fn ai_sources_dispatch_to_pdf_or_postscript_backends() {
        let ctx = egui::Context::default();
        for (name, pages) in [("vector.ai", 2), ("vector-eps.ai", 1)] {
            let path =
                PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata")).join(name);
            let source = SourceImage::load(&ctx, path).unwrap();
            assert_eq!(source.vector_page_count(), pages, "{name}");
        }
    }

    #[test]
    fn vector_view_texture_places_the_page_at_the_content_rect() {
        let document = yuetu_vector::VectorDocument::load(&PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/vector.pdf"
        )))
        .unwrap();
        let content = egui::Rect::from_min_max(egui::pos2(300.0, 200.0), egui::pos2(791.0, 569.0));
        let viewport = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 800.0));
        let transform = vector_view_transform(
            document.page_size(0).unwrap(),
            content,
            viewport,
            0.0,
            1.0,
            1.0,
            1.0,
        );
        let image = document.render_page_view(0, 1000, 800, transform).unwrap();
        // The first PDF page's red circle is centered at (130, 160); the
        // content mapping says that point belongs near (459, 396).
        let pixel = image.pixel(459, 396).unwrap();
        assert!(pixel.r > 150, "{pixel:?}");
        assert!(pixel.a > 200, "{pixel:?}");
        // The checkerboard area outside the page remains transparent.
        assert_eq!(image.pixel(0, 0).unwrap().a, 0);
    }

    #[test]
    fn vector_view_transform_keeps_the_page_center_centered() {
        let content = egui::Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(900.0, 650.0));
        let viewport = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 800.0));
        for rotation in [0.0, 37.0, 90.0, -45.0] {
            let transform =
                vector_view_transform((400.0, 300.0), content, viewport, rotation, 1.0, 1.0, 1.0);
            let x = transform.a * 200.0 + transform.c * 150.0 + transform.e;
            let y = transform.b * 200.0 + transform.d * 150.0 + transform.f;
            assert!((x - content.center().x).abs() < 0.01, "{rotation}");
            assert!((y - content.center().y).abs() < 0.01, "{rotation}");
        }
    }

    #[test]
    fn vector_view_transform_includes_device_pixel_scale() {
        let content = egui::Rect::from_min_max(egui::pos2(100.0, 50.0), egui::pos2(900.0, 650.0));
        let viewport = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 800.0));
        let transform =
            vector_view_transform((400.0, 300.0), content, viewport, 0.0, 2.0, 1.0, 1.0);
        assert!((transform.a - 4.0).abs() < 0.01);
        assert!((transform.d - 4.0).abs() < 0.01);
        assert!((transform.e - 200.0).abs() < 0.01);
        assert!((transform.f - 100.0).abs() < 0.01);
    }

    #[test]
    fn capping_the_vector_texture_preserves_aspect() {
        let size = egui::vec2(2252.8, 1689.6);
        let (width, height) = capped_texture_size(size, 2.0, 4096);
        assert_eq!(width, 4096);
        assert_eq!(height, 3072);
        assert!((width as f32 / height as f32 - 4.0 / 3.0).abs() < 1e-4);
    }

    #[test]
    fn bottom_left_origin_flips_the_displayed_row() {
        assert_eq!(CoordinateOrigin::TopLeft.display_y(0, 100), 0);
        assert_eq!(CoordinateOrigin::TopLeft.display_y(99, 100), 99);
        assert_eq!(CoordinateOrigin::BottomLeft.display_y(0, 100), 99);
        assert_eq!(CoordinateOrigin::BottomLeft.display_y(99, 100), 0);
        assert_eq!(CoordinateOrigin::BottomLeft.display_y(0, 0), 0);
    }
}
