//! Application state that outlives a single frame.

use egui::{TextureFilter, TextureHandle, TextureOptions, TextureWrapMode};
use std::path::PathBuf;
use std::sync::Arc;
use yuetu_core::resize::resize;
use yuetu_core::{
    Color, ConversionConfig, ConversionResult, IndexedImage, Palette, ResizeAlgorithm, RgbaImage,
    load_image_sequence,
};

use crate::widgets::midi_gpu::MidiGpuData;

/// Longest side of the texture uploaded to the GPU. The conversion still uses
/// the full-resolution CPU image; this is display-only.
const MAX_TEXTURE_SIDE: u32 = 4096;

pub struct SourceImage {
    pub path: PathBuf,
    pub name: String,
    /// The currently selected frame, shared with the conversion job.
    pub image: Arc<RgbaImage>,
    pub has_transparency: bool,
    frames: Vec<Arc<RgbaImage>>,
    frame_index: usize,
    pub texture: TextureHandle,
}

impl SourceImage {
    pub fn load(ctx: &egui::Context, path: PathBuf) -> yuetu_core::Result<Self> {
        let sequence = load_image_sequence(&path)?;
        let frames: Vec<Arc<RgbaImage>> = sequence.frames.into_iter().map(Arc::new).collect();
        let image = frames[0].clone();
        let has_transparency = image_has_transparency(&image);
        let texture_name = format!("source:{}:0", path.display());
        let texture = upload_texture(ctx, &texture_name, &image)?;

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image")
            .to_owned();

        Ok(Self {
            path,
            name,
            image,
            has_transparency,
            frames,
            frame_index: 0,
            texture,
        })
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn frame_index(&self) -> usize {
        self.frame_index
    }

    pub fn select_frame(&mut self, ctx: &egui::Context, index: usize) -> yuetu_core::Result<bool> {
        if self.frames.len() <= 1 {
            return Ok(false);
        }
        let index = index.min(self.frames.len() - 1);
        if index == self.frame_index {
            return Ok(false);
        }

        let image = self.frames[index].clone();
        let has_transparency = image_has_transparency(&image);
        let texture_name = format!("source:{}:{index}", self.path.display());
        let texture = upload_texture(ctx, &texture_name, &image)?;
        self.image = image;
        self.has_transparency = has_transparency;
        self.frame_index = index;
        self.texture = texture;
        Ok(true)
    }
}

fn upload_texture(
    ctx: &egui::Context,
    name: &str,
    image: &RgbaImage,
) -> yuetu_core::Result<TextureHandle> {
    // Never assume 4096: some backends (and egui's defaults) cap at 2048.
    // Loading a larger texture is a debug assertion/panic inside egui.
    let max_texture_side = ctx
        .input(|input| input.max_texture_side)
        .clamp(256, MAX_TEXTURE_SIDE as usize) as u32;
    let display = if image.width.max(image.height) > max_texture_side {
        let scale = max_texture_side as f64 / f64::from(image.width.max(image.height));
        let width = (f64::from(image.width) * scale)
            .round()
            .clamp(1.0, f64::from(max_texture_side)) as u32;
        let height = (f64::from(image.height) * scale)
            .round()
            .clamp(1.0, f64::from(max_texture_side)) as u32;
        resize(image, width, height, ResizeAlgorithm::Area)?
    } else {
        image.clone()
    };

    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [display.width as usize, display.height as usize],
        display.as_rgba(),
    );
    // Crisp texels when zoomed in, smooth reduction when zoomed out.
    let options = TextureOptions {
        magnification: TextureFilter::Nearest,
        minification: TextureFilter::Linear,
        wrap_mode: TextureWrapMode::ClampToEdge,
        mipmap_mode: None,
    };
    Ok(ctx.load_texture(name, color_image, options))
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
            Self::Midi => "MIDI 输出",
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
    },
    KeyMap {
        pos: (u32, u32),
        color: Color,
        key: Option<u8>,
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
        })
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
}

impl StatusMessage {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Info,
            text: text.into(),
        }
    }

    pub fn success(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Success,
            text: text.into(),
        }
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Warning,
            text: text.into(),
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            kind: StatusKind::Error,
            text: text.into(),
        }
    }
}

impl Default for StatusMessage {
    fn default() -> Self {
        Self::info("就绪")
    }
}
