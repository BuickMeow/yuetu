//! Application state that outlives a single frame.

use egui::{TextureFilter, TextureHandle, TextureOptions, TextureWrapMode};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use yuetu_core::resize::resize;
use yuetu_core::{
    Color, ConversionConfig, ConversionResult, ImageSequence, IndexedImage, KeyMap, Palette,
    ResizeAlgorithm, RgbaImage, TimeAxis, generate_notes_on_grid, load_image_sequence,
    normalize_degrees, quantize, resolve_time_rows, rotate_image, rotated_dimensions,
};

use crate::widgets::midi_gpu::MidiGpuData;

/// Longest side of the texture uploaded to the GPU. The conversion still uses
/// the full-resolution CPU image; this is display-only.
const MAX_TEXTURE_SIDE: u32 = 4096;
/// Longest side of a frame-strip thumbnail before rotation. Small enough that
/// generating a few visible thumbnails while scrolling is cheap.
const THUMBNAIL_MAX_SIDE: u32 = 96;
/// Working-resolution source used by stage previews while scrubbing. The
/// final conversion still uses the original image.
const INTERACTION_MAX_SIDE: u32 = 1024;

struct Thumbnail {
    texture: TextureHandle,
    size: egui::Vec2,
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
    frames: Vec<Arc<RgbaImage>>,
    delays_ms: Vec<u32>,
    original_palette: Option<Palette>,
    frame_index: usize,
    rotation_deg: f32,
    thumbnails: Vec<Option<Thumbnail>>,
    pub texture: TextureHandle,
    texture_dirty: bool,
}

impl SourceImage {
    pub fn load(ctx: &egui::Context, path: PathBuf) -> yuetu_core::Result<Self> {
        let ImageSequence {
            frames,
            delays_ms,
            original_palette,
        } = load_image_sequence(&path)?;
        let frames: Vec<Arc<RgbaImage>> = frames.into_iter().map(Arc::new).collect();
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
            frames,
            delays_ms,
            original_palette,
            frame_index: 0,
            rotation_deg: 0.0,
            thumbnails: std::iter::repeat_with(|| None)
                .take(thumbnail_count)
                .collect(),
            texture,
            texture_dirty: false,
        })
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
    ) -> yuetu_core::Result<bool> {
        let degrees = normalize_degrees(degrees);
        if (degrees - self.rotation_deg).abs() < 1e-4 {
            return Ok(false);
        }

        let (display_image, display_size) = prepare_display(ctx, &self.image, degrees)?;
        let interaction_image = prepare_interaction_from_display(&display_image)?;
        if upload_display_texture {
            let has_transparency = image_has_transparency(&display_image);
            let texture_name = format!(
                "source:{}:{}:{:.3}",
                self.path.display(),
                self.frame_index,
                degrees
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
        self.rotation_deg = degrees;
        // Frame-strip thumbnails are cached in the old orientation.
        for thumbnail in &mut self.thumbnails {
            *thumbnail = None;
        }
        Ok(true)
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
            frames,
            delays_ms: vec![100, 100],
            original_palette: None,
            frame_index: 0,
            rotation_deg: 0.0,
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
    fn bottom_left_origin_flips_the_displayed_row() {
        assert_eq!(CoordinateOrigin::TopLeft.display_y(0, 100), 0);
        assert_eq!(CoordinateOrigin::TopLeft.display_y(99, 100), 99);
        assert_eq!(CoordinateOrigin::BottomLeft.display_y(0, 100), 99);
        assert_eq!(CoordinateOrigin::BottomLeft.display_y(99, 100), 0);
        assert_eq!(CoordinateOrigin::BottomLeft.display_y(0, 0), 0);
    }
}
